//! Resolver: links `{snippet}` macros to shared parsed bodies and builds the form (spec §4.4,
//! §4.5, §4.8).
//!
//! Two passes, in the order of the reference implementation:
//!
//! 1. Expansion, depth first in source order (through both branches of every `{if}`). For each
//!    `{snippet}`: not found, then cycle, then depth, then the body's parse errors, then its own
//!    snippets. Each distinct abbreviation is parsed and expanded once; a later `{snippet}` that
//!    names an abbreviation which already expanded cleanly links to the same body and is checked
//!    only for depth, using the body's memoised height. That cannot change which error is
//!    reported first: a body that expanded cleanly once has no missing snippet, no parse error
//!    and no cycle anywhere below it (a cycle through it would have been found the first time),
//!    so depth is the only thing that can differ between two inclusions.
//! 2. The form, walking the same graph in the same order but entering each body only once: a
//!    second visit could add no new field and cannot conflict with itself.

use super::error::{ErrorKind, TemplateError};
use super::parse::{Body, FormEvent, parse_body};
use super::{FieldKind, FieldSpec, MAX_DEPTH, Resolved, SnippetSource};
use std::collections::HashMap;
use std::sync::Arc;

/// A parsed body with its links into the resolved graph.
#[derive(Debug, Clone)]
pub(crate) struct Linked {
    pub(crate) body: Arc<Body>,
    /// For each `{snippet}` of the body, the index of the linked body in `Resolved::bodies`.
    pub(crate) snippets: Vec<usize>,
    /// For each field slot of the body, the index of the field in the form.
    pub(crate) fields: Vec<usize>,
    /// Offset of this body's date slots in the render-time date cache.
    pub(crate) date_base: usize,
}

enum State {
    /// On the current inclusion chain.
    Expanding,
    /// Expanded cleanly into this body index.
    Done(usize),
}

struct Expander<'s> {
    source: &'s dyn SnippetSource,
    bodies: Vec<Linked>,
    /// Abbreviation of each body; `None` for the template itself.
    abbrs: Vec<Option<String>>,
    /// Height of each expanded body: 1 plus the largest height of the bodies it includes.
    heights: Vec<usize>,
    states: HashMap<String, State>,
}

impl Expander<'_> {
    fn push(&mut self, body: Arc<Body>, abbr: Option<String>) -> usize {
        self.bodies.push(Linked {
            body,
            snippets: Vec::new(),
            fields: Vec::new(),
            date_base: 0,
        });
        self.abbrs.push(abbr);
        self.heights.push(1);
        self.bodies.len() - 1
    }

    fn error(&self, kind: ErrorKind, body: usize, at: usize) -> TemplateError {
        TemplateError::at(kind, at).in_snippet(self.abbrs[body].as_deref())
    }

    /// Expands body `b`, which sits `depth` snippets below the template. Returns its height.
    fn expand(&mut self, b: usize, depth: usize) -> Result<usize, TemplateError> {
        let body = Arc::clone(&self.bodies[b].body);
        let mut below = 0;
        for r in &body.snippet_refs {
            let child = match self.states.get(r.abbr.as_str()) {
                Some(&State::Done(child)) => {
                    if depth + self.heights[child] > MAX_DEPTH {
                        return Err(self.error(ErrorKind::TooDeep, b, r.at));
                    }
                    child
                }
                state => {
                    let expanding = matches!(state, Some(State::Expanding));
                    let Some(src) = self.source.body(&r.abbr) else {
                        return Err(self.error(ErrorKind::SnippetNotFound, b, r.at));
                    };
                    if expanding {
                        return Err(self.error(ErrorKind::SnippetCycle, b, r.at));
                    }
                    if depth >= MAX_DEPTH {
                        return Err(self.error(ErrorKind::TooDeep, b, r.at));
                    }
                    let parsed = parse_body(src).map_err(|e| e.in_snippet(Some(&r.abbr)))?;
                    let child = self.push(Arc::new(parsed), Some(r.abbr.clone()));
                    self.states.insert(r.abbr.clone(), State::Expanding);
                    let height = self.expand(child, depth + 1)?;
                    self.heights[child] = height;
                    self.states.insert(r.abbr.clone(), State::Done(child));
                    child
                }
            };
            below = below.max(self.heights[child]);
            self.bodies[b].snippets.push(child);
        }
        Ok(below + 1)
    }
}

struct Entry {
    spec: FieldSpec,
    /// Created by a bare `{if}` and not (yet) replaced by a `{field}`.
    implicit: bool,
    /// For an implicit toggle: the first `{if}` on it with `is` or `not` (body, position).
    conditioned_at: Option<(usize, usize)>,
}

struct FormBuilder<'e> {
    bodies: &'e [Linked],
    abbrs: &'e [Option<String>],
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
    visited: Vec<bool>,
}

impl FormBuilder<'_> {
    fn collect(&mut self, b: usize) -> Result<(), TemplateError> {
        self.visited[b] = true;
        let linked = &self.bodies[b];
        for ev in &linked.body.form_events {
            match ev {
                FormEvent::Field {
                    slot,
                    kind,
                    default,
                    at,
                } => {
                    let name = &linked.body.field_names[*slot];
                    match self.index.get(name.as_str()) {
                        None => {
                            self.index.insert(name.clone(), self.entries.len());
                            self.entries.push(Entry {
                                spec: FieldSpec {
                                    name: name.clone(),
                                    kind: kind.clone(),
                                    default: default.clone(),
                                },
                                implicit: false,
                                conditioned_at: None,
                            });
                        }
                        Some(&i) => {
                            let entry = &mut self.entries[i];
                            if entry.implicit {
                                entry.spec.kind = kind.clone();
                                entry.spec.default.clone_from(default);
                                entry.implicit = false;
                            } else if entry.spec.kind != *kind {
                                return Err(TemplateError::at(ErrorKind::FieldConflict, *at)
                                    .in_snippet(self.abbrs[b].as_deref()));
                            }
                        }
                    }
                }
                FormEvent::If { slot, bare, at } => {
                    let name = &linked.body.field_names[*slot];
                    let conditioned = (!bare).then_some((b, *at));
                    match self.index.get(name.as_str()) {
                        None => {
                            self.index.insert(name.clone(), self.entries.len());
                            self.entries.push(Entry {
                                spec: FieldSpec {
                                    name: name.clone(),
                                    kind: FieldKind::Toggle,
                                    default: "no".to_owned(),
                                },
                                implicit: true,
                                conditioned_at: conditioned,
                            });
                        }
                        Some(&i) => {
                            let entry = &mut self.entries[i];
                            if entry.implicit && entry.conditioned_at.is_none() {
                                entry.conditioned_at = conditioned;
                            }
                        }
                    }
                }
                FormEvent::Snippet(index) => {
                    let child = linked.snippets[*index];
                    if !self.visited[child] {
                        self.collect(child)?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn resolve(
    root: Arc<Body>,
    source: &dyn SnippetSource,
) -> Result<Resolved, TemplateError> {
    let mut ex = Expander {
        source,
        bodies: Vec::new(),
        abbrs: Vec::new(),
        heights: Vec::new(),
        states: HashMap::new(),
    };
    let root = ex.push(root, None);
    ex.expand(root, 0)?;

    let mut form = FormBuilder {
        bodies: &ex.bodies,
        abbrs: &ex.abbrs,
        entries: Vec::new(),
        index: HashMap::new(),
        visited: vec![false; ex.bodies.len()],
    };
    form.collect(root)?;
    for entry in &form.entries {
        if let (true, Some((b, at))) = (entry.implicit, entry.conditioned_at) {
            return Err(
                TemplateError::at(ErrorKind::FieldUndefined, at).in_snippet(ex.abbrs[b].as_deref())
            );
        }
    }
    let FormBuilder { entries, index, .. } = form;

    let mut bodies = ex.bodies;
    let mut date_slots = 0;
    for linked in &mut bodies {
        linked.fields = linked
            .body
            .field_names
            .iter()
            .map(|n| index.get(n.as_str()).copied().unwrap_or(0))
            .collect();
        linked.date_base = date_slots;
        date_slots += linked.body.date_count;
    }
    Ok(Resolved {
        needs_clipboard: bodies.iter().any(|l| l.body.has_clipboard),
        uses_cursor: bodies.iter().any(|l| l.body.has_cursor),
        form: entries.into_iter().map(|e| e.spec).collect(),
        bodies,
        date_slots,
    })
}
