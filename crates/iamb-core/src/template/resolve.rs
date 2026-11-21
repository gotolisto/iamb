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
//! 2. The form, in the order a walk of the same graph would find it (each body entered once,
//!    in source order), but made from a [`Summary`] of each included body instead of a walk:
//!    see [`Summary`] for why merging summaries finds what the walk finds. A form with more than
//!    [`MAX_FORM_FIELDS`] distinct names is `E_FORM_TOO_LARGE` (reported before any other form
//!    error, at the first `{field}` or `{if}` of the 101st name); otherwise the first
//!    `E_FIELD_CONFLICT` in walk order, then the first `E_FIELD_UNDEFINED` in form order.
//!
//! # Sharing between resolves
//!
//! A [`Compiler`] resolves many templates against one library (a snapshot build) and shares work
//! between them: each abbreviation is looked up and parsed at most once, and each body that
//! expanded cleanly is kept as one [`Expanded`] node that every later resolve links to. A
//! library where many snippets include one hub that includes many others therefore costs time
//! and memory linear in its size, not (snippets × included bodies).
//!
//! Every included body with form content gets one summary per compiler, made from its own
//! events and its children's summaries, and holding at most [`MAX_FORM_FIELDS`] + 1 names. So
//! the forms of a whole library cost time linear in its size: a template's form costs its own
//! events plus at most [`MAX_FORM_FIELDS`] + 1 names per included body, however many fields
//! lie below (review 1.9 follow-up N1: copying or walking the forms of included bodies made a
//! library of 10,001 snippets take 10 s and 5.5 GiB). The fields themselves are made from the
//! summary when first asked for.
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
use super::{FieldKind, FieldSpec, MAX_DEPTH, MAX_FORM_FIELDS, Resolved, SnippetSource};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, OnceLock};

/// A body that expanded cleanly, linked to the bodies its `{snippet}`s include. Shared by every
/// template of a [`Compiler`] that includes it.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about one body, each read on its own"
)]
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
    /// The snippet whose body this is is rich (its literal text is Markdown). False for the
    /// template being resolved: the caller picks how to render it.
    pub(crate) rich: bool,
}

impl Expanded {
    /// A `{field}` or `{if}` in this body or any body below it.
    pub(crate) const fn has_form(&self) -> bool {
        self.has_form
    }

    fn new(body: Arc<Body>, abbr: Option<&str>, snippets: Vec<Arc<Expanded>>, rich: bool) -> Self {
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
            rich,
        }
    }
}

/// The fill-in form of a resolved template: shared between templates whose form is the same
/// included body's (see the [module docs](self)).
///
/// A snapshot build compiles every template, and most forms are never shown, so the fields
/// are made from the `{field}` and `{if}` events that define them when first asked for.
#[derive(Default)]
pub(crate) struct Form {
    /// For each field, in order: the body and event where it first appears, and the body and
    /// event of the `{field}` that defines it (`None`: an implicit toggle).
    source: Vec<(Event, Option<Event>)>,
    fields: OnceLock<Vec<FieldSpec>>,
    /// Field name → index in `fields`.
    index: OnceLock<HashMap<String, usize>>,
}

/// A `{field}` or `{if}` event of a body.
type Event = (Arc<Expanded>, usize);

impl std::fmt::Debug for Form {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Form")
            .field("fields", &self.source.len())
            .finish_non_exhaustive()
    }
}

impl Form {
    /// The fields, in order of first appearance.
    pub(crate) fn fields(&self) -> &[FieldSpec] {
        self.fields.get_or_init(|| {
            (self.source.iter())
                .map(|((node, first), field)| {
                    let name = match &node.body.form_events[*first] {
                        FormEvent::Field { slot, .. } | FormEvent::If { slot, .. } => {
                            node.body.field_names[*slot].clone()
                        }
                        FormEvent::Snippet(_) => String::new(),
                    };
                    let (kind, default) = match field
                        .as_ref()
                        .map(|(node, event)| &node.body.form_events[*event])
                    {
                        Some(FormEvent::Field { kind, default, .. }) => {
                            (kind.clone(), default.clone())
                        }
                        _ => (FieldKind::Toggle, "no".to_owned()),
                    };
                    FieldSpec {
                        name,
                        kind,
                        default,
                    }
                })
                .collect()
        })
    }

    /// The index in `fields` of the field `name`.
    pub(crate) fn position(&self, name: &str) -> Option<usize> {
        self.index
            .get_or_init(|| {
                (self.fields().iter().enumerate())
                    .map(|(i, f)| (f.name.clone(), i))
                    .collect()
            })
            .get(name)
            .copied()
    }
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
    /// Included body → the summary of its form content.
    summaries: HashMap<*const Expanded, Arc<Summary>>,
    /// The bodies summaries point into (this also keeps every summarised body alive, so its
    /// address stays its key).
    nodes: Nodes,
    /// The form of every template that asks nothing.
    empty: Arc<Form>,
    /// The summary of every body with no form content.
    no_summary: Arc<Summary>,
}

impl<'s> Compiler<'s> {
    pub(crate) fn new(source: &'s dyn SnippetSource) -> Self {
        Self {
            source,
            lookups: HashMap::new(),
            clean: HashMap::new(),
            forms: HashMap::new(),
            summaries: HashMap::new(),
            nodes: Nodes::default(),
            empty: Arc::default(),
            no_summary: Arc::default(),
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
        let done = if let Some(child) = sole_form_child(node) {
            self.form_of(&Arc::clone(child))
        } else {
            let summary = self.summary_of(node);
            form_from(&self.nodes, &summary).map(Arc::new)
        };
        // Only included bodies are shared; a template's own root is not included by anything.
        if node.abbr.is_some() {
            self.forms.insert(key, (Arc::clone(node), done.clone()));
        }
        done
    }

    /// The summary of the form content of `node`, made once per included body. A body whose
    /// only form content is one included body has that body's summary.
    fn summary_of(&mut self, node: &Arc<Expanded>) -> Arc<Summary> {
        if !node.has_form {
            return Arc::clone(&self.no_summary);
        }
        let key = Arc::as_ptr(node);
        if let Some(done) = self.summaries.get(&key) {
            return Arc::clone(done);
        }
        let done = if let Some(child) = sole_form_child(node) {
            self.summary_of(&Arc::clone(child))
        } else {
            let children: Vec<Option<Arc<Summary>>> = node
                .snippets
                .iter()
                .map(|c| c.has_form.then(|| self.summary_of(c)))
                .collect();
            let id = self.nodes.id(node);
            Arc::new(summarize(&self.nodes, id, &children))
        };
        if node.abbr.is_some() {
            self.nodes.id(node);
            self.summaries.insert(key, Arc::clone(&done));
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
        let rich = abbr.is_some_and(|a| self.c.source.is_rich(a));
        Ok(Arc::new(Expanded::new(body, abbr, snippets, rich)))
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

/// The FNV-1a hash of a field name. Summaries find names by this hash and compare the names
/// only when hashes are equal, so a collision costs a scan of at most
/// [`MAX_FORM_FIELDS`] + 1 slots, never a wrong answer.
fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Hashes the name hashes a summary is indexed by: they are hashes already.
#[derive(Clone, Copy, Default)]
struct Prehashed(u64);

impl Hasher for Prehashed {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 << 8) | u64::from(b);
        }
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
}

/// The bodies summaries point into, by index, so that a summary is plain data that copies
/// without touching reference counts.
#[derive(Default)]
struct Nodes {
    list: Vec<Arc<Expanded>>,
    ids: HashMap<*const Expanded, usize>,
}

impl Nodes {
    fn id(&mut self, node: &Arc<Expanded>) -> usize {
        let next = self.list.len();
        let id = *self.ids.entry(Arc::as_ptr(node)).or_insert(next);
        if id == next {
            self.list.push(Arc::clone(node));
        }
        id
    }
}

/// One `{field}` or `{if}` event of a body (`node` in [`Nodes`]), and where it falls in the
/// walk of the body whose [`Summary`] holds it.
#[derive(Clone, Copy)]
struct At {
    node: usize,
    /// Index into the body's `form_events`; always a `Field` or an `If`.
    event: usize,
    /// Walk order among the events of one summary (only meaningful for `field` and `other`).
    rank: usize,
}

impl At {
    fn ranked(self, rank: usize) -> Self {
        Self { rank, ..self }
    }
}

/// Reads [`At`]s.
#[derive(Clone, Copy)]
struct View<'n>(&'n Nodes);

impl<'n> View<'n> {
    fn event(self, at: At) -> (&'n Expanded, &'n FormEvent) {
        let node = &*self.0.list[at.node];
        (node, &node.body.form_events[at.event])
    }

    /// The field slot and the position of the event.
    fn slot_at(self, at: At) -> (usize, usize) {
        match self.event(at).1 {
            FormEvent::Field { slot, at, .. } | FormEvent::If { slot, at, .. } => (*slot, *at),
            // An `At` is only made for a `Field` or an `If`.
            FormEvent::Snippet(_) => (0, 0),
        }
    }

    fn name(self, at: At) -> &'n str {
        self.event(at).0.body.field_names[self.slot_at(at).0].as_str()
    }

    /// The type and default of a `{field}` event.
    fn field(self, at: At) -> Option<(&'n FieldKind, &'n str)> {
        match self.event(at).1 {
            FormEvent::Field { kind, default, .. } => Some((kind, default.as_str())),
            _ => None,
        }
    }

    fn kind(self, at: Option<At>) -> Option<&'n FieldKind> {
        at.and_then(|at| self.field(at)).map(|(kind, _)| kind)
    }

    fn error(self, at: At, kind: ErrorKind) -> TemplateError {
        TemplateError::at(kind, self.slot_at(at).1).in_snippet(self.event(at).0.abbr.as_deref())
    }
}

/// What the walk of one body found for one field name: enough to merge the body into any
/// includer exactly as walking it there would (see [`Summary`]).
#[derive(Clone, Copy)]
struct Slot {
    /// [`name_hash`] of the field name.
    hash: u64,
    /// The first event naming the field: where it joins the form.
    first: At,
    /// The first `{field}` defining it: its type and default are the form field's.
    field: Option<At>,
    /// The first `{field}` with a type or options other than `field`'s (`E_FIELD_CONFLICT`).
    other: Option<At>,
    /// The first `{if}` on it with `is` or `not` (`E_FIELD_UNDEFINED` when there is no `field`).
    cond: Option<At>,
}

/// The form content of an expanded body, as a walk from that body finds it: one [`Slot`] per
/// field name in order of first appearance, at most [`MAX_FORM_FIELDS`] + 1 of them (more
/// than [`MAX_FORM_FIELDS`] means `E_FORM_TOO_LARGE`, and then only the first names matter).
///
/// A body's summary is made from its own events and the summaries of the bodies it includes,
/// never by walking them again, so making every summary of a library costs time linear in its
/// size. Merging an included body's summary gives what the walk in the includer finds, although
/// that walk skips bodies it has already entered: the skipped bodies' names are already in the
/// form, every `{field}` in them that did not cause a conflict has the type the form already
/// has, and every `{if}` in them changes nothing that is still open; so for each name, the
/// first `{field}` that conflicts, the first defining `{field}` and the first conditioned
/// `{if}` are the same with or without them, and so is the order of the names that are new.
/// A name that already has a conflict keeps the earlier one.
///
/// Ranks order the `field` and `other` events across names. An included body takes a block of
/// its includer's ranks; when they grow past [`RANK_BOUND`] they are renumbered from 0, so they
/// stay small however deep the inclusion.
#[derive(Default)]
struct Summary {
    slots: Vec<Slot>,
    /// Every rank in `slots` is below this.
    ranks: usize,
}

/// The ranks of a summary are renumbered when they would reach beyond this (see [`Summary`]).
const RANK_BOUND: usize = 4 * (MAX_FORM_FIELDS + 1);

/// Builds the summary of one body.
struct SummaryBuilder<'n> {
    view: View<'n>,
    slots: Vec<Slot>,
    /// Name hash → the first slot with that hash.
    index: HashMap<u64, usize, BuildHasherDefault<Prehashed>>,
    /// The next rank; each merged body takes a block of ranks.
    rank: usize,
}

impl<'n> SummaryBuilder<'n> {
    fn new(nodes: &'n Nodes, capacity: usize) -> Self {
        let capacity = capacity.min(MAX_FORM_FIELDS + 1);
        Self {
            view: View(nodes),
            slots: Vec::with_capacity(capacity),
            index: HashMap::with_capacity_and_hasher(capacity, BuildHasherDefault::default()),
            rank: 0,
        }
    }

    fn full(&self) -> bool {
        self.slots.len() > MAX_FORM_FIELDS
    }

    /// The slot for `name` (whose hash is `new.hash`), or `None` after adding `new` as its slot.
    fn slot(&mut self, name: &str, new: Slot) -> Option<&mut Slot> {
        let next = self.slots.len();
        let view = self.view;
        let i = match self.index.get(&new.hash) {
            None => {
                self.index.insert(new.hash, next);
                next
            }
            Some(&i) if view.name(self.slots[i].first) == name => i,
            Some(_) => (self.slots.iter())
                .position(|s| s.hash == new.hash && view.name(s.first) == name)
                .unwrap_or(next),
        };
        if i == next {
            self.slots.push(new);
            return None;
        }
        Some(&mut self.slots[i])
    }

    fn field(&mut self, at: At) {
        let at = at.ranked(self.rank);
        self.rank += 1;
        let view = self.view;
        let name = view.name(at);
        let new = Slot {
            hash: name_hash(name),
            first: at,
            field: Some(at),
            other: None,
            cond: None,
        };
        if let Some(s) = self.slot(name, new) {
            match s.field {
                None => s.field = Some(at),
                Some(f) if s.other.is_none() && view.kind(Some(f)) != view.kind(Some(at)) => {
                    s.other = Some(at);
                }
                Some(_) => {}
            }
        }
    }

    fn cond(&mut self, at: At, bare: bool) {
        let cond = (!bare).then_some(at);
        let name = self.view.name(at);
        let new = Slot {
            hash: name_hash(name),
            first: at,
            field: None,
            other: None,
            cond,
        };
        if let Some(s) = self.slot(name, new)
            && s.cond.is_none()
        {
            s.cond = cond;
        }
    }

    /// Merges the summary of an included body, as walking that body here would.
    fn merge(&mut self, child: &Summary) {
        let base = self.rank;
        let view = self.view;
        let shift = |at: Option<At>| at.map(|a| a.ranked(base + a.rank));
        for e in &child.slots {
            if self.full() {
                break;
            }
            let new = Slot {
                field: shift(e.field),
                other: shift(e.other),
                ..*e
            };
            let Some(s) = self.slot(view.name(e.first), new) else {
                continue;
            };
            match (s.field, e.field) {
                // The form already has the field: the included body's first `{field}` of
                // another type conflicts, or else its own first conflict does.
                (Some(f), Some(ef)) if s.other.is_none() => {
                    s.other = if view.kind(Some(f)) == view.kind(Some(ef)) {
                        shift(e.other)
                    } else {
                        shift(e.field)
                    };
                }
                (Some(_), _) => {}
                // An implicit toggle so far: the included body's first `{field}` defines it.
                (None, Some(_)) => {
                    s.field = shift(e.field);
                    s.other = shift(e.other);
                }
                (None, None) => {
                    if s.cond.is_none() {
                        s.cond = e.cond;
                    }
                }
            }
        }
        self.rank = base + child.ranks;
    }

    fn finish(self) -> Summary {
        let mut slots = self.slots;
        slots.truncate(MAX_FORM_FIELDS + 1);
        if self.rank <= RANK_BOUND {
            return Summary {
                slots,
                ranks: self.rank,
            };
        }
        // Renumber the ranks from 0, keeping their order: then they are below twice the
        // number of slots, so below `RANK_BOUND`.
        let mut ranks: Vec<usize> = slots
            .iter()
            .flat_map(|s| [s.field, s.other])
            .filter_map(|a| a.map(|a| a.rank))
            .collect();
        ranks.sort_unstable();
        ranks.dedup();
        for s in &mut slots {
            for at in [&mut s.field, &mut s.other].into_iter().flatten() {
                at.rank = ranks.binary_search(&at.rank).unwrap_or_default();
            }
        }
        Summary {
            slots,
            ranks: ranks.len(),
        }
    }
}

/// Summarises the body `node` (`id` in `nodes`) from its own events and `children`, the
/// summaries of its included bodies (by snippet index; `None` for bodies with no form content).
fn summarize(nodes: &Nodes, id: usize, children: &[Option<Arc<Summary>>]) -> Summary {
    let node = &nodes.list[id];
    let capacity = node.body.form_events.len()
        + children
            .iter()
            .flatten()
            .map(|c| c.slots.len())
            .sum::<usize>();
    let mut b = SummaryBuilder::new(nodes, capacity);
    let mut merged: HashSet<*const Expanded> = HashSet::new();
    for (event, ev) in node.body.form_events.iter().enumerate() {
        if b.full() {
            break;
        }
        let at = At {
            node: id,
            event,
            rank: 0,
        };
        match ev {
            FormEvent::Field { .. } => b.field(at),
            FormEvent::If { bare, .. } => b.cond(at, *bare),
            FormEvent::Snippet(index) => {
                // A body entered once already adds nothing the second time.
                let ptr = Arc::as_ptr(&node.snippets[*index]);
                if let Some(child) = &children[*index]
                    && merged.insert(ptr)
                {
                    b.merge(child);
                }
            }
        }
    }
    b.finish()
}

/// The form a summary describes, or its first error: too many fields, then the first conflict
/// in walk order, then the first implicit toggle that an `{if}` compares.
fn form_from(nodes: &Nodes, summary: &Summary) -> Result<Form, TemplateError> {
    let view = View(nodes);
    if let Some(s) = summary.slots.get(MAX_FORM_FIELDS) {
        return Err(view.error(s.first, ErrorKind::FormTooLarge));
    }
    if let Some(at) = summary
        .slots
        .iter()
        .filter_map(|s| s.other)
        .min_by_key(|a| a.rank)
    {
        return Err(view.error(at, ErrorKind::FieldConflict));
    }
    if let Some(at) = summary
        .slots
        .iter()
        .find(|s| s.field.is_none() && s.cond.is_some())
        .and_then(|s| s.cond)
    {
        return Err(view.error(at, ErrorKind::FieldUndefined));
    }
    let event = |at: At| (Arc::clone(&nodes.list[at.node]), at.event);
    let source = (summary.slots.iter())
        .map(|s| (event(s.first), s.field.map(event)))
        .collect();
    Ok(Form {
        source,
        ..Form::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::fmt::Write as _;

    /// `{field name=<prefix><k>}` for each `k` in `range`.
    fn fields(prefix: &str, range: std::ops::Range<usize>) -> String {
        range.fold(String::new(), |mut s, k| {
            write!(s, "{{field name={prefix}{k}}}").expect("write to a string");
            s
        })
    }

    /// Every `{field}` and `{if}` event below `node`, each body entered once, in source order.
    fn events(
        node: &Arc<Expanded>,
        nodes: &mut Nodes,
        seen: &mut HashSet<*const Expanded>,
        out: &mut Vec<At>,
    ) {
        seen.insert(Arc::as_ptr(node));
        let id = nodes.id(node);
        for (event, ev) in node.body.form_events.iter().enumerate() {
            match ev {
                FormEvent::Snippet(index) => {
                    let child = &node.snippets[*index];
                    if !seen.contains(&Arc::as_ptr(child)) {
                        events(child, nodes, seen, out);
                    }
                }
                _ => out.push(At {
                    node: id,
                    event,
                    rank: 0,
                }),
            }
        }
    }

    /// The form as a plain walk from the root builds it (the order of the reference
    /// implementation): every body entered once, in source order; too many names first, then
    /// the first conflict, then the first compared implicit toggle.
    fn walked(c: &mut Compiler<'_>, source: &str) -> Result<Vec<FieldSpec>, TemplateError> {
        let body = parse_body(source).map(Arc::new)?;
        let mut walk = Walk {
            c,
            local: HashMap::new(),
            linked: Vec::new(),
            reach: HashSet::new(),
        };
        let root = walk.expand(body, None, 0)?;
        let mut nodes = Nodes::default();
        let mut all = Vec::new();
        events(&root, &mut nodes, &mut HashSet::new(), &mut all);
        let view = View(&nodes);
        let mut names: Vec<&str> = Vec::new();
        for &at in &all {
            if !names.contains(&view.name(at)) {
                names.push(view.name(at));
                if names.len() > MAX_FORM_FIELDS {
                    return Err(view.error(at, ErrorKind::FormTooLarge));
                }
            }
        }
        // (spec, implicit, first conditioned `{if}`), in order of first appearance
        let mut form: Vec<(FieldSpec, bool, Option<At>)> = Vec::new();
        for &at in &all {
            let name = view.name(at);
            let i = form.iter().position(|(f, ..)| f.name == name);
            match (view.field(at), i) {
                (Some((kind, default)), None) => form.push((
                    FieldSpec {
                        name: name.to_owned(),
                        kind: kind.clone(),
                        default: default.to_owned(),
                    },
                    false,
                    None,
                )),
                (Some((kind, default)), Some(i)) => {
                    let (spec, implicit, _) = &mut form[i];
                    if *implicit {
                        spec.kind = kind.clone();
                        default.clone_into(&mut spec.default);
                        *implicit = false;
                    } else if spec.kind != *kind {
                        return Err(view.error(at, ErrorKind::FieldConflict));
                    }
                }
                (None, i) => {
                    let FormEvent::If { bare, .. } = view.event(at).1 else {
                        continue;
                    };
                    let cond = (!bare).then_some(at);
                    match i {
                        None => form.push((
                            FieldSpec {
                                name: name.to_owned(),
                                kind: FieldKind::Toggle,
                                default: "no".to_owned(),
                            },
                            true,
                            cond,
                        )),
                        Some(i) => {
                            let (_, implicit, first) = &mut form[i];
                            if *implicit && first.is_none() {
                                *first = cond;
                            }
                        }
                    }
                }
            }
        }
        if let Some(&(_, _, Some(at))) = form
            .iter()
            .find(|(_, implicit, c)| *implicit && c.is_some())
        {
            return Err(view.error(at, ErrorKind::FieldUndefined));
        }
        Ok(form.into_iter().map(|(f, ..)| f).collect())
    }

    const N: usize = 8;

    /// Bodies that wrap each other (a lone `{snippet}`, possibly beside formless text and
    /// snippets) as well as bodies with their own fields, `{if}`s and conflicts, and bodies
    /// with many fields, so that forms near and over the limit are common.
    fn arb_piece(i: usize) -> impl Strategy<Value = String> {
        let later = (i + 1).min(N - 1)..N;
        prop_oneof![
            5 => later.prop_map(|j| format!("{{snippet abbr=s{j}}}")),
            1 => Just("x{snippet abbr=plain}".to_owned()),
            1 => prop_oneof![Just("text"), Just("area")]
                .prop_map(|t| format!("{{field name=F type={t}}}")),
            1 => Just("{if field=G is=a}y{endif}".to_owned()),
            1 => Just("{if field=H}z{endif}".to_owned()),
            1 => Just("{field name=G type=choice options=\"a|b\"}".to_owned()),
            4 => (0..10usize, 25..60usize)
                .prop_map(|(from, count)| fields("m", from * 12..from * 12 + count)),
            2 => (0..150usize).prop_map(|k| {
                format!("{{if field=m{k} not=a}}{{endif}}{{field name=m{k} type=area}}")
            }),
            2 => (0..150usize).prop_map(|k| format!("{{if field=m{k}}}{{endif}}")),
        ]
    }

    fn arb_library() -> impl Strategy<Value = HashMap<String, String>> {
        (0..N)
            .map(|i| proptest::collection::vec(arb_piece(i), 0..4).prop_map(|p| p.concat()))
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

        /// A form made from summaries, or taken from a shared body, is the form a walk from the
        /// root builds, error, snippet and position included, whatever was compiled before.
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

    fn library(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|&(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }

    /// Every snippet that includes one hub with fields shares the hub's form.
    #[test]
    fn wrappers_share_one_form() {
        let lib = library(&[
            ("hub", "{field name=A}{if field=B}b{endif}"),
            ("w1", "Dear {snippet abbr=hub}{snippet abbr=plain}"),
            ("w2", "{snippet abbr=hub} and {snippet abbr=hub}"),
            ("w3", "{snippet abbr=w1}"),
            ("own", "{field name=C}{snippet abbr=hub}"),
            ("plain", "text"),
        ]);
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
        assert_eq!(w1.fields(), hub.fields());
        assert!(!Arc::ptr_eq(&own, &w1));
        assert_eq!(own.fields().len(), 3);
        assert!(plain.fields().is_empty());
    }

    /// A conflict found only in the includer's context: the included body is consistent on its
    /// own, but its first `{field}` has another type than the includer's. And a body whose own
    /// conflict comes after a field the includer already has with the first type.
    #[test]
    fn merged_conflicts_are_found_where_a_walk_finds_them() {
        let lib = library(&[
            ("a", "{field name=X type=area}{field name=Y}"),
            ("b", "{field name=Y}{field name=Y type=area}"),
            ("t1", "{field name=X}{snippet abbr=a}"),
            ("t2", "{field name=Y}{snippet abbr=b}"),
            ("t3", "{field name=Y type=area}{snippet abbr=b}"),
            ("t4", "{if field=X not=1}{endif}{snippet abbr=a}"),
            (
                "t5",
                "{if field=Q not=1}{endif}{snippet abbr=a}{snippet abbr=b}",
            ),
        ]);
        let mut c = Compiler::new(&lib);
        for (name, want) in [
            ("t1", Some((ErrorKind::FieldConflict, Some("a"), 0))),
            ("t2", Some((ErrorKind::FieldConflict, Some("b"), 14))),
            ("t3", Some((ErrorKind::FieldConflict, Some("b"), 0))),
            ("t4", None),
            ("t5", Some((ErrorKind::FieldConflict, Some("b"), 14))),
        ] {
            let got = c.resolve_snippet(name, &lib[name]).and_then(|r| r);
            let mut alone = Compiler::new(&lib);
            assert_eq!(
                got.as_ref()
                    .map(|r| r.form().to_vec())
                    .map_err(Clone::clone),
                walked(&mut alone, &lib[name]),
                "{name}"
            );
            let got = got
                .err()
                .map(|e| (e.kind(), e.snippet().map(str::to_owned), e.position()));
            assert_eq!(
                got,
                want.map(|(k, s, p)| (k, s.map(str::to_owned), Some(p))),
                "{name}"
            );
        }
    }

    /// The 101st distinct name is the error, at its first appearance; 100 are fine.
    #[test]
    fn form_limit_counts_distinct_names_across_snippets() {
        let hundred = fields("f", 0..100);
        let lib = library(&[
            ("hub", &hundred),
            (
                "again",
                "{snippet abbr=hub}{field name=f5}{if field=f7}x{endif}",
            ),
            ("more", "{snippet abbr=hub}{if field=extra}x{endif}"),
            ("first", "{field name=extra}{snippet abbr=hub}"),
        ]);
        let mut c = Compiler::new(&lib);
        let mut resolve = |name: &str| c.resolve_snippet(name, &lib[name]).and_then(|r| r);
        assert_eq!(resolve("hub").expect("100 fields").form().len(), 100);
        assert_eq!(resolve("again").expect("same names").form().len(), 100);
        let e = resolve("more").expect_err("101 fields");
        assert_eq!(
            (e.kind(), e.snippet(), e.position()),
            (ErrorKind::FormTooLarge, None, Some(18))
        );
        let e = resolve("first").expect_err("101 fields");
        assert_eq!(
            (e.kind(), e.snippet(), e.position()),
            (
                ErrorKind::FormTooLarge,
                Some("hub"),
                hundred.find("{field name=f99}")
            )
        );
    }
}
