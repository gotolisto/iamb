//! Template parser and renderer (task 1.3).
//!
//! Implements `docs/template-language.md`; the examples in `spec/template/examples/` are the test
//! suite (`tests/template_spec.rs`) and `spec/template/reference.py` is the oracle this module
//! follows wherever the prose leaves room.
//!
//! The pipeline has three steps, and every error belongs to exactly one of them:
//!
//! 1. [`Template::parse`] reads one template: syntax, attributes, values, formats, shifts and
//!    `{if}` nesting. A template that parses can be shown as chips with [`Template::tokens`].
//! 2. [`Template::resolve`] looks up every `{snippet}` in a [`SnippetSource`] (the reference set),
//!    parses those bodies, detects cycles and excessive nesting, and builds the fill-in form.
//!    Every error except the four run-time ones is found here, before a form is shown.
//! 3. [`Resolved::render`] takes the clock, settings, clipboard and form values and produces the
//!    [`Rendered`] actions. Only `E_DATE_RANGE`, `E_CURSOR_BEFORE_KEY`, `E_WAIT_TOTAL` and
//!    `E_OUTPUT_TOO_LARGE` can happen here.
//!
//! ```
//! use iamb_core::datetime::{Format, LocalDateTime, Locale};
//! use iamb_core::template::{FormValues, NoSnippets, RenderContext, RenderedAction, Template};
//!
//! let template = Template::parse("Hi {field name=\"Name\"}, see you {date fmt=\"%A\" shift=\"+1d\"}.")?;
//! let resolved = template.resolve(&NoSnippets)?;
//! assert_eq!(resolved.form()[0].name, "Name");
//!
//! let (date_format, time_format) = (Format::parse("%Y-%m-%d")?, Format::parse("%H:%M")?);
//! let ctx = RenderContext {
//!     now: LocalDateTime::new(2026, 3, 18, 14, 5, 9)?,
//!     locale: Locale::from_tag("en"),
//!     date_format: &date_format,
//!     time_format: &time_format,
//!     clipboard: None,
//! };
//! let values: FormValues = [("Name", "Kim")].into_iter().collect();
//! let rendered = resolved.render(&ctx, &values)?;
//! let RenderedAction::Text { text, verbatim } = &rendered.actions[0] else { unreachable!() };
//! assert_eq!(text, "Hi Kim, see you Thursday.");
//! assert_eq!(verbatim, &[3..6]); // "Kim" came from the form: adaptive case leaves it alone
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Design notes
//!
//! - **Shared bodies.** A resolved template is a graph, not a tree: each distinct snippet body is
//!   parsed once and every `{snippet}` that names it links to the same parsed body. A library in
//!   which every snippet includes the next one many times therefore resolves in time linear in
//!   the size of the distinct bodies. Cycle and depth checks follow the reference implementation's
//!   depth-first order exactly, so the first error reported is the same; a body that already
//!   resolved cleanly is re-checked only for depth, using its memoised height.
//! - **Bounded rendering.** Rendering walks only taken branches, but repeated inclusion can still
//!   make the walk long (a library where each of 8 levels includes the next 50 times is 50^7
//!   inclusions). Besides the 100,000-character limit, a render fails with `E_OUTPUT_TOO_LARGE`
//!   as soon as it visits more than [`MAX_RENDER_STEPS`] template nodes (only reachable through
//!   repeated inclusion, typically of content that types little or nothing) or types more than
//!   [`MAX_OUTPUT_BYTES`] bytes (over 40 bytes per character on average, which ordinary text
//!   never needs). Each distinct `{date}`/`{time}` node is computed at most once per render.
//!   The character limit itself is checked exactly, but lazily: grapheme clusters are counted
//!   only once more than 100,000 bytes are typed, and only before another render error and at
//!   the end, which reports the same first error as checking after every piece of text.
//! - **Form values.** A submitted value is checked against its field when the template renders
//!   (review 1.9 finding 14): a `choice` value that is not one of the options renders as the
//!   field's default; a `toggle` value other than `yes` counts as unchecked (`no`), also in
//!   `{if}` comparisons; a `text` (single-line) value has each line break replaced by a space.
//!   `area` values are used as given. Nothing is reported: the form normally cannot produce
//!   such values, and a stale or hand-made answer should still type something sensible.
//! - **Characters.** Limits and `cursor_back` count extended grapheme clusters (§3), using
//!   `unicode-segmentation`. Field names are limited to 64 grapheme clusters.
//! - **Positions.** Error positions and token ranges are offsets in Unicode scalar values
//!   (`char`s) into the original source of the template that contains them, before line-ending
//!   normalisation.
//! - **Privacy.** Field values and clipboard text are user-entered values (architecture §8): the
//!   types that hold them ([`FormValues`], [`RenderContext`], [`Rendered`], [`RenderedAction`])
//!   have `Debug` implementations that print lengths, never content, and nothing here logs.

mod error;
mod parse;
mod render;
mod resolve;
mod syntax;

pub(crate) use resolve::Compiler;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::hash::BuildHasher;
use std::sync::Arc;

pub use error::{ErrorKind, TemplateError};
pub use render::{RenderContext, Rendered, RenderedAction};
pub use syntax::{Attribute, MacroName, Token};

/// Maximum nesting of `{if}` inside one template body, and of `{snippet}` below the template.
pub const MAX_DEPTH: usize = 8;
/// Maximum total of the waits that run in one expansion, in milliseconds.
pub const MAX_WAIT_TOTAL_MS: u32 = 30_000;
/// Maximum typed text of one expansion, in extended grapheme clusters.
pub const MAX_OUTPUT_CHARS: usize = 100_000;
/// Safety bound on the typed text of one expansion, in bytes (`E_OUTPUT_TOO_LARGE`). A single
/// grapheme cluster has no size limit in Unicode, so the character limit alone does not bound
/// memory.
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
/// Safety bound on the number of template nodes one render may visit (`E_OUTPUT_TOO_LARGE`).
pub const MAX_RENDER_STEPS: usize = 300_000;

/// The library a template's `{snippet}` macros are looked up in: the reference set of
/// architecture §4.4 (every non-deleted snippet, including disabled ones).
pub trait SnippetSource {
    /// The template body of the snippet with exactly this abbreviation (case-sensitive), if any.
    fn body(&self, abbreviation: &str) -> Option<&str>;
}

/// An empty library: every `{snippet}` is `E_SNIPPET_NOT_FOUND`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSnippets;

impl SnippetSource for NoSnippets {
    fn body(&self, _abbreviation: &str) -> Option<&str> {
        None
    }
}

impl<S: BuildHasher> SnippetSource for HashMap<String, String, S> {
    fn body(&self, abbreviation: &str) -> Option<&str> {
        self.get(abbreviation).map(String::as_str)
    }
}

impl SnippetSource for BTreeMap<String, String> {
    fn body(&self, abbreviation: &str) -> Option<&str> {
        self.get(abbreviation).map(String::as_str)
    }
}

/// A key that `{key}` can press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyName {
    /// `enter`
    Enter,
    /// `tab`
    Tab,
    /// `escape`
    Escape,
    /// `backspace`
    Backspace,
    /// `delete`
    Delete,
    /// `space`
    Space,
    /// `up`
    Up,
    /// `down`
    Down,
    /// `left`
    Left,
    /// `right`
    Right,
    /// `home`
    Home,
    /// `end`
    End,
    /// `pageup`
    PageUp,
    /// `pagedown`
    PageDown,
}

impl KeyName {
    /// Every key, in the order of the specification's table.
    pub const ALL: [KeyName; 14] = [
        Self::Enter,
        Self::Tab,
        Self::Escape,
        Self::Backspace,
        Self::Delete,
        Self::Space,
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::Home,
        Self::End,
        Self::PageUp,
        Self::PageDown,
    ];

    /// The name as written in a template, e.g. `pageup`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::Tab => "tab",
            Self::Escape => "escape",
            Self::Backspace => "backspace",
            Self::Delete => "delete",
            Self::Space => "space",
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::Home => "home",
            Self::End => "end",
            Self::PageUp => "pageup",
            Self::PageDown => "pagedown",
        }
    }

    /// Looks a key up by its template name (exact, lower case).
    #[must_use]
    pub fn from_name(name: &str) -> Option<KeyName> {
        Self::ALL.into_iter().find(|k| k.as_str() == name)
    }
}

impl fmt::Display for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The type of a fill-in field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FieldKind {
    /// One line of text.
    Text,
    /// Several lines of text.
    Area,
    /// One of a fixed list of options, in the order written.
    Choice(Vec<String>),
    /// A checkbox; its value is `yes` or `no`.
    Toggle,
}

impl FieldKind {
    /// The type as written in a template: `text`, `area`, `choice` or `toggle`.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Area => "area",
            Self::Choice(_) => "choice",
            Self::Toggle => "toggle",
        }
    }
}

/// One field of the fill-in form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSpec {
    /// The label shown in the form; also the key in [`FormValues`].
    pub name: String,
    /// The field type.
    pub kind: FieldKind,
    /// The value used when the form does not supply one: the text for `text` and `area`, an
    /// option for `choice`, `yes` or `no` for `toggle`.
    pub default: String,
}

/// Values entered in the fill-in form: field name → value. A field without a value keeps its
/// default. Line endings are normalised to LF on insertion. Values that do not fit their field
/// are corrected when the template renders (see the module docs, "Form values").
///
/// Values are user-entered text (architecture §8): `Debug` prints only how many there are.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FormValues {
    values: HashMap<String, String>,
}

impl FormValues {
    /// No values: every field keeps its default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the value of field `name`, normalising CRLF and CR to LF. Returns the old value.
    pub fn insert(&mut self, name: impl Into<String>, value: &str) -> Option<String> {
        self.values
            .insert(name.into(), syntax::normalize_newlines(value).into_owned())
    }

    /// The value of field `name`, if one was entered.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Number of values.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// True when no value has been entered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl<K: Into<String>, V: AsRef<str>> FromIterator<(K, V)> for FormValues {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut values = Self::new();
        for (k, v) in iter {
            values.insert(k, v.as_ref());
        }
        values
    }
}

impl fmt::Debug for FormValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormValues")
            .field("len", &self.values.len())
            .finish_non_exhaustive()
    }
}

/// A parsed template. Immutable and cheap to clone.
#[derive(Debug, Clone)]
pub struct Template {
    source: Arc<str>,
    body: Arc<parse::Body>,
}

impl Template {
    /// Parses one template. Nested `{snippet}` bodies are not looked at until
    /// [`resolve`](Self::resolve).
    ///
    /// # Errors
    /// The first parse error in the template (§5, "Found: Parse"), with its position.
    pub fn parse(source: &str) -> Result<Template, TemplateError> {
        let body = parse::parse_body(source)?;
        Ok(Template {
            source: Arc::from(source),
            body: Arc::new(body),
        })
    }

    /// The template's literal text runs and macros, in order, with `char` ranges into the
    /// source: what the editor shows as text and chips.
    #[must_use]
    pub fn tokens(&self) -> Vec<Token> {
        syntax::tokens(&self.source)
    }

    /// The source this template was parsed from.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Inlines every `{snippet}` from `snippets`, checks the whole graph (including branches that
    /// may not be taken) and builds the form.
    ///
    /// # Errors
    /// The first resolve error (`E_SNIPPET_NOT_FOUND`, `E_SNIPPET_CYCLE`, `E_TOO_DEEP`, a parse
    /// error in a nested body, then `E_FIELD_CONFLICT`, then `E_FIELD_UNDEFINED`), with the
    /// snippet whose body contains it.
    pub fn resolve(&self, snippets: &dyn SnippetSource) -> Result<Resolved, TemplateError> {
        resolve::Compiler::new(snippets).resolve(Arc::clone(&self.body))
    }
}

/// A template with its snippets inlined and its form built. Every parse and resolve error has
/// been ruled out; only the run-time limits can still fail in [`render`](Self::render).
///
/// Nested bodies are shared, not copied: resolves made by one snapshot build link to the same
/// parsed and expanded bodies (see the module docs).
#[derive(Debug, Clone)]
pub struct Resolved {
    root: Arc<resolve::Expanded>,
    /// Shared with other templates whose form is the same included body's.
    form: Arc<resolve::Form>,
    needs_clipboard: bool,
    uses_cursor: bool,
}

impl Resolved {
    /// The fill-in fields, in order of first appearance. Empty when there is nothing to ask.
    #[must_use]
    pub fn form(&self) -> &[FieldSpec] {
        &self.form.fields
    }

    /// True when a `{clipboard}` appears anywhere in the resolved template (taken branch or not),
    /// so the clipboard must be read at match time.
    #[must_use]
    pub fn needs_clipboard(&self) -> bool {
        self.needs_clipboard
    }

    /// True when a `{cursor}` appears anywhere in the resolved template (taken branch or not).
    /// Whether one was actually rendered is [`Rendered::cursor_back`].
    #[must_use]
    pub fn uses_cursor(&self) -> bool {
        self.uses_cursor
    }

    /// Produces the actions for one expansion.
    ///
    /// # Errors
    /// `E_DATE_RANGE`, `E_CURSOR_BEFORE_KEY`, `E_WAIT_TOTAL` or `E_OUTPUT_TOO_LARGE`, whichever
    /// happens first in output order.
    pub fn render(
        &self,
        ctx: &RenderContext<'_>,
        values: &FormValues,
    ) -> Result<Rendered, TemplateError> {
        render::render(self, ctx, values)
    }
}

/// Parses, resolves and renders `source` in one call (tests, CLI dry run, previews).
///
/// # Errors
/// The first error of any step.
pub fn render_template(
    source: &str,
    snippets: &dyn SnippetSource,
    ctx: &RenderContext<'_>,
    values: &FormValues,
) -> Result<Rendered, TemplateError> {
    Template::parse(source)?
        .resolve(snippets)?
        .render(ctx, values)
}
