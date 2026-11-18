//! Resolver: links `{snippet}` macros to shared parsed bodies and builds the form (spec §4.4,
//! §4.5, §4.8).
//!
//! Two passes, in the order of the reference implementation:
//!
//! 1. Expansion, depth first in source order (through both branches of every `{if}`). For each
//!    `{snippet}`: not found, then cycle, then depth, then the body's parse errors, then its own
//!    snippets. A later `{snippet}` that names an abbreviation which already expanded cleanly in
//!    this resolve links to the same body and is checked only for depth, using the body's
//!    memoised height. That cannot change which error is reported first: a body that expanded
//!    cleanly once has no missing snippet, no parse error and no cycle anywhere below it (a
//!    cycle through it would have been found the first time), so depth is the only thing that
//!    can differ between two inclusions.
//! 2. The form, walking the same graph in the same order but entering each body only once: a
//!    second visit could add no new field and cannot conflict with itself. Bodies with no field
//!    and no `{if}` anywhere below them are not entered at all.
//!
//! # Sharing between resolves
//!
//! A [`Compiler`] resolves many templates against one library (a snapshot build) and shares work
//! between them: each abbreviation is looked up and parsed at most once, and each body that
//! expanded cleanly is kept as one [`Expanded`] node that every later resolve links to. A
//! library where many snippets include one hub that includes many others therefore costs time
//! and memory linear in its size, not (snippets × included bodies).
//!
//! Forms are shared the same way where the form is the same: when a body's only form content is
//! one included body (other `{snippet}`s below it ask nothing, and it has no `{field}` or `{if}`
//! of its own), its form is that body's form, built once per compiler and shared by every
//! template that includes it. Walking such a body from an empty form enters only that one child
//! (the others are skipped as having no form), and the child cannot include its parent, so the
//! form, and any form error with its snippet and position, is exactly the child's.
//!
//! Every resolve still reports exactly the error a resolve on its own would report, snippet and
//! position included. A body that expanded cleanly in an earlier resolve, but has not been seen
//! in this one, is linked without walking it when it fits (`depth + height ≤ MAX_DEPTH`): a walk
//! could find no error then, by the argument above, and the walk would only have marked bodies
//! as seen. When it does not fit, the walk is replayed: if the body is below one linked earlier
//! in this resolve, a walk would already have seen it, so the error is at this inclusion;
//! otherwise it is walked exactly as on a first visit, which finds the same deeper error.

use super::error::{ErrorKind, TemplateError};
use super::parse::{Body, FormEvent, parse_body};
use super::{FieldKind, FieldSpec, MAX_DEPTH, Resolved, SnippetSource};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A body that expanded cleanly, linked to the bodies its `{snippet}`s include. Shared by every
/// template of a [`Compiler`] that includes it.
#[derive(Debug)]
pub(crate) struct Expanded {
    pub(crate) body: Arc<Body>,
    /// The snippet whose body this is; `None` for the template being resolved.
    abbr: Option<String>,
    /// For each `{snippet}` of the body, in order, the included body.
    pub(crate) snippets: Vec<Arc<Expanded>>,
    /// 1 plus the largest height of the included bodies.
    height: usize,
    /// A `{clipboard}` in this body or any body below it.
    has_clipboard: bool,
    /// A `{cursor}` in this body or any body below it.
    has_cursor: bool,
    /// A `{field}` or `{if}` in this body or any body below it.
    has_form: bool,
}

impl Expanded {
    fn new(body: Arc<Body>, abbr: Option<&str>, snippets: Vec<Arc<Expanded>>) -> Self {
        let own_form = body
            .form_events
            .iter()
            .any(|e| !matches!(e, FormEvent::Snippet(_)));
        Self {
            height: 1 + snippets.iter().map(|s| s.height).max().unwrap_or(0),
            has_clipboard: body.has_clipboard || snippets.iter().any(|s| s.has_clipboard),
            has_cursor: body.has_cursor || snippets.iter().any(|s| s.has_cursor),
            has_form: own_form || snippets.iter().any(|s| s.has_form),
            abbr: abbr.map(str::to_owned),
            body,
            snippets,
        }
    }
}

/// The fill-in form of a resolved template: shared between templates whose form is the same
/// included body's (see the [module docs](self)).
#[derive(Debug, Default)]
pub(crate) struct Form {
    /// The fields, in order of first appearance.
    pub(crate) fields: Vec<FieldSpec>,
    /// Field name → index in `fields`.
    pub(crate) index: HashMap<String, usize>,
}

/// A form built for one shared body, kept with the body so that its address stays its key.
type BuiltForm = (Arc<Expanded>, Result<Arc<Form>, TemplateError>);

/// Resolves templates against one library, sharing lookups, parsed bodies and clean expansions
/// between them (see the [module docs](self)).
pub(crate) struct Compiler<'s> {
    source: &'s dyn SnippetSource,
    /// Abbreviation → the parsed body (`None`: not in the library). Parse errors are kept without
    /// a snippet name; each use names the snippet.
    lookups: HashMap<String, Option<Result<Arc<Body>, TemplateError>>>,
    /// Abbreviation → its body, expanded cleanly in some resolve of this compiler.
    clean: HashMap<String, Arc<Expanded>>,
    /// Included body → its form, for bodies whose form a template's form was taken from.
    forms: HashMap<*const Expanded, BuiltForm>,
    /// The form of every template that asks nothing.
    empty: Arc<Form>,
}

impl<'s> Compiler<'s> {
    pub(crate) fn new(source: &'s dyn SnippetSource) -> Self {
        Self {
            source,
            lookups: HashMap::new(),
            clean: HashMap::new(),
            forms: HashMap::new(),
            empty: Arc::default(),
        }
    }

    /// The parsed body of the snippet `abbr` (`None` when it is not in the library), looked up
    /// and parsed once per compiler.
    pub(crate) fn lookup(&mut self, abbr: &str) -> Option<Result<Arc<Body>, TemplateError>> {
        if let Some(found) = self.lookups.get(abbr) {
            return found.clone();
        }
        let found = self
            .source
            .body(abbr)
            .map(|src| parse_body(src).map(Arc::new));
        self.lookups.insert(abbr.to_owned(), found.clone());
        found
    }

    /// Parses and resolves the template `source`, which the library holds under exactly the
    /// abbreviation `abbr` (`source.body(abbr) == Some(source)`, as for every snippet that holds
    /// its abbreviation in a snapshot). The parse is shared with the `{snippet}` lookups of other
    /// templates.
    ///
    /// The outer error is a parse error of `source` itself; the inner result is the resolve.
    pub(crate) fn resolve_snippet(
        &mut self,
        abbr: &str,
        source: &str,
    ) -> Result<Result<Resolved, TemplateError>, TemplateError> {
        let root = if let Some(Some(parsed)) = self.lookups.get(abbr) {
            parsed.clone()
        } else {
            let parsed = parse_body(source).map(Arc::new);
            self.lookups.insert(abbr.to_owned(), Some(parsed.clone()));
            parsed
        }?;
        Ok(self.resolve(root))
    }

    /// Resolves the template whose parsed body is `root`.
    pub(crate) fn resolve(&mut self, root: Arc<Body>) -> Result<Resolved, TemplateError> {
        let mut walk = Walk {
            c: self,
            local: HashMap::new(),
            linked: Vec::new(),
            reach: HashSet::new(),
        };
        let root = walk.expand(root, None, 0)?;
        let form = self.form_of(&root)?;
        Ok(Resolved {
            needs_clipboard: root.has_clipboard,
            uses_cursor: root.has_cursor,
            form,
            root,
        })
    }

    /// The form of the expanded body `node`, shared with the body it is taken from when its only
    /// form content is one included body (see the [module docs](self)).
    fn form_of(&mut self, node: &Arc<Expanded>) -> Result<Arc<Form>, TemplateError> {
        if !node.has_form {
            return Ok(Arc::clone(&self.empty));
        }
        let key = Arc::as_ptr(node);
        if let Some((_, done)) = self.forms.get(&key) {
            return done.clone();
        }
        let done = match sole_form_child(node) {
            Some(child) => self.form_of(&Arc::clone(child)),
            None => build_form(node).map(Arc::new),
        };
        // Only included bodies are shared; a template's own root is not included by anything.
        if node.abbr.is_some() {
            self.forms.insert(key, (Arc::clone(node), done.clone()));
        }
        done
    }
}

/// The one included body that holds all of `node`'s form content, if there is exactly one and
/// `node` has no `{field}` or `{if}` of its own.
fn sole_form_child(node: &Expanded) -> Option<&Arc<Expanded>> {
    let mut sole: Option<&Arc<Expanded>> = None;
    for ev in &node.body.form_events {
        let FormEvent::Snippet(index) = ev else {
            return None;
        };
        let child = &node.snippets[*index];
        if !child.has_form {
            continue;
        }
        match sole {
            None => sole = Some(child),
            Some(s) if Arc::ptr_eq(s, child) => {}
            Some(_) => return None,
        }
    }
    sole
}

/// The state of one resolve.
struct Walk<'c, 's> {
    c: &'c mut Compiler<'s>,
    /// Abbreviations on the current inclusion chain (`None`) or expanded in this resolve.
    local: HashMap<String, Option<Arc<Expanded>>>,
    /// Bodies linked in this resolve without being walked whose descendants are not yet in
    /// `reach`.
    linked: Vec<Arc<Expanded>>,
    /// Bodies below a body linked without being walked: a walk would have seen them.
    reach: HashSet<*const Expanded>,
}

impl Walk<'_, '_> {
    /// Expands `body` (of snippet `abbr`, `None` for the template), which sits `depth` snippets
    /// below the template.
    fn expand(
        &mut self,
        body: Arc<Body>,
        abbr: Option<&str>,
        depth: usize,
    ) -> Result<Arc<Expanded>, TemplateError> {
        let error = |kind, at| TemplateError::at(kind, at).in_snippet(abbr);
        let mut snippets = Vec::with_capacity(body.snippet_refs.len());
        for r in &body.snippet_refs {
            let name = r.abbr.as_str();
            let child = match self.local.get(name) {
                Some(Some(done)) => {
                    if depth + done.height > MAX_DEPTH {
                        return Err(error(ErrorKind::TooDeep, r.at));
                    }
                    Arc::clone(done)
                }
                state => {
                    let expanding = state.is_some();
                    let Some(parsed) = self.c.lookup(name) else {
                        return Err(error(ErrorKind::SnippetNotFound, r.at));
                    };
                    if expanding {
                        return Err(error(ErrorKind::SnippetCycle, r.at));
                    }
                    if let Some(clean) = self.c.clean.get(name).cloned() {
                        if depth + clean.height <= MAX_DEPTH {
                            self.local.insert(name.to_owned(), Some(Arc::clone(&clean)));
                            self.linked.push(Arc::clone(&clean));
                            clean
                        } else if depth >= MAX_DEPTH || self.walk_would_have_seen(&clean) {
                            return Err(error(ErrorKind::TooDeep, r.at));
                        } else {
                            self.descend(name, Arc::clone(&clean.body), depth)?
                        }
                    } else {
                        if depth >= MAX_DEPTH {
                            return Err(error(ErrorKind::TooDeep, r.at));
                        }
                        let parsed = parsed.map_err(|e| e.in_snippet(Some(name)))?;
                        self.descend(name, parsed, depth)?
                    }
                }
            };
            snippets.push(child);
        }
        Ok(Arc::new(Expanded::new(body, abbr, snippets)))
    }

    /// Walks the body of snippet `name`, included from a body at `depth`.
    fn descend(
        &mut self,
        name: &str,
        body: Arc<Body>,
        depth: usize,
    ) -> Result<Arc<Expanded>, TemplateError> {
        self.local.insert(name.to_owned(), None);
        let done = self.expand(body, Some(name), depth + 1)?;
        // Keep the node an earlier resolve made, so every template shares it.
        let done = Arc::clone(self.c.clean.entry(name.to_owned()).or_insert(done));
        self.local.insert(name.to_owned(), Some(Arc::clone(&done)));
        Ok(done)
    }

    /// Whether `node` lies below a body linked without a walk in this resolve. Only asked on the
    /// way to a depth error, so the descendants are collected lazily.
    fn walk_would_have_seen(&mut self, node: &Arc<Expanded>) -> bool {
        while let Some(next) = self.linked.pop() {
            for child in &next.snippets {
                if self.reach.insert(Arc::as_ptr(child)) {
                    self.linked.push(Arc::clone(child));
                }
            }
        }
        self.reach.contains(&Arc::as_ptr(node))
    }
}

struct Entry {
    spec: FieldSpec,
    /// Created by a bare `{if}` and not (yet) replaced by a `{field}`.
    implicit: bool,
    /// For an implicit toggle: the first `{if}` on it with `is` or `not` (snippet, position).
    conditioned_at: Option<(Option<String>, usize)>,
}

#[derive(Default)]
struct FormBuilder {
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
    visited: HashSet<*const Expanded>,
}

impl FormBuilder {
    fn collect(&mut self, node: &Arc<Expanded>) -> Result<(), TemplateError> {
        self.visited.insert(Arc::as_ptr(node));
        let abbr = node.abbr.as_deref();
        for ev in &node.body.form_events {
            match ev {
                FormEvent::Field {
                    slot,
                    kind,
                    default,
                    at,
                } => {
                    let name = &node.body.field_names[*slot];
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
                                    .in_snippet(abbr));
                            }
                        }
                    }
                }
                FormEvent::If { slot, bare, at } => {
                    let name = &node.body.field_names[*slot];
                    let conditioned = (!bare).then(|| (abbr.map(str::to_owned), *at));
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
                    let child = &node.snippets[*index];
                    if child.has_form && !self.visited.contains(&Arc::as_ptr(child)) {
                        self.collect(child)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Builds the form of the expanded body `root` by walking it.
fn build_form(root: &Arc<Expanded>) -> Result<Form, TemplateError> {
    let mut form = FormBuilder::default();
    if root.has_form {
        form.collect(root)?;
    }
    for entry in &form.entries {
        if let (true, Some((abbr, at))) = (entry.implicit, &entry.conditioned_at) {
            return Err(
                TemplateError::at(ErrorKind::FieldUndefined, *at).in_snippet(abbr.as_deref())
            );
        }
    }
    let FormBuilder { entries, index, .. } = form;
    Ok(Form {
        fields: entries.into_iter().map(|e| e.spec).collect(),
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The form as a plain walk from the root builds it, without taking it from a shared body.
    fn walked(c: &mut Compiler<'_>, source: &str) -> Result<Vec<FieldSpec>, TemplateError> {
        let body = parse_body(source).map(Arc::new)?;
        let mut walk = Walk {
            c,
            local: HashMap::new(),
            linked: Vec::new(),
            reach: HashSet::new(),
        };
        let root = walk.expand(body, None, 0)?;
        build_form(&root).map(|f| f.fields)
    }

    const N: usize = 8;

    /// Bodies that wrap each other (a lone `{snippet}`, possibly beside formless text and
    /// snippets) as well as bodies with their own fields, `{if}`s and conflicts.
    fn arb_piece(i: usize) -> impl Strategy<Value = String> {
        let later = (i + 1).min(N - 1)..N;
        prop_oneof![
            6 => later.prop_map(|j| format!("{{snippet abbr=s{j}}}")),
            2 => Just("x{snippet abbr=plain}".to_owned()),
            1 => prop_oneof![Just("text"), Just("area")]
                .prop_map(|t| format!("{{field name=F type={t}}}")),
            1 => Just("{if field=G is=a}y{endif}".to_owned()),
            1 => Just("{if field=H}z{endif}".to_owned()),
            1 => Just("{field name=G type=choice options=\"a|b\"}".to_owned()),
        ]
    }

    fn arb_library() -> impl Strategy<Value = HashMap<String, String>> {
        (0..N)
            .map(|i| proptest::collection::vec(arb_piece(i), 0..3).prop_map(|p| p.concat()))
            .collect::<Vec<_>>()
            .prop_map(|bodies| {
                let mut lib: HashMap<String, String> = bodies
                    .into_iter()
                    .enumerate()
                    .map(|(i, b)| (format!("s{i}"), b))
                    .collect();
                lib.insert("plain".to_owned(), "text".to_owned());
                lib
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        /// A form taken from a shared body is the form a walk from the root builds, error,
        /// snippet and position included, whatever was compiled before.
        #[test]
        fn shared_forms_match_a_walk(lib in arb_library()) {
            let mut c = Compiler::new(&lib);
            let mut names: Vec<&String> = lib.keys().collect();
            names.sort();
            for name in names {
                let shared = c
                    .resolve_snippet(name, &lib[name])
                    .and_then(|r| r)
                    .map(|r| r.form().to_vec());
                let mut alone = Compiler::new(&lib);
                prop_assert_eq!(shared, walked(&mut alone, &lib[name]), "{}: {}", name, &lib[name]);
            }
        }
    }

    /// Every snippet that includes one hub with fields shares the hub's form.
    #[test]
    fn wrappers_share_one_form() {
        let lib: HashMap<String, String> = [
            ("hub", "{field name=A}{if field=B}b{endif}"),
            ("w1", "Dear {snippet abbr=hub}{snippet abbr=plain}"),
            ("w2", "{snippet abbr=hub} and {snippet abbr=hub}"),
            ("w3", "{snippet abbr=w1}"),
            ("own", "{field name=C}{snippet abbr=hub}"),
            ("plain", "text"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let mut c = Compiler::new(&lib);
        let mut form = |name: &str| {
            c.resolve_snippet(name, &lib[name])
                .and_then(|r| r)
                .expect("resolves")
                .form
        };
        let (hub, w1, w2, w3, own, plain) = (
            form("hub"),
            form("w1"),
            form("w2"),
            form("w3"),
            form("own"),
            form("plain"),
        );
        assert!(Arc::ptr_eq(&w1, &w2) && Arc::ptr_eq(&w1, &w3));
        assert_eq!(w1.fields, hub.fields);
        assert!(!Arc::ptr_eq(&own, &w1));
        assert_eq!(own.fields.len(), 3);
        assert!(plain.fields.is_empty());
    }
}
