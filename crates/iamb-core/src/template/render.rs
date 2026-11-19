//! Renderer: turns a resolved template, the context and the form values into actions (spec §3).

use super::error::{ErrorKind, TemplateError};
use super::parse::{Cond, Node, is_info_separator};
use super::resolve::Expanded;
use super::syntax::normalize_newlines;
use super::{
    FieldKind, FieldSpec, FormValues, KeyName, MAX_OUTPUT_BYTES, MAX_OUTPUT_CHARS,
    MAX_RENDER_STEPS, MAX_WAIT_TOTAL_MS, Resolved,
};
use crate::datetime::{Format, LocalDateTime, Locale, Shift};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

/// Everything outside the template that one expansion depends on, captured once at match time
/// (architecture §4.3).
#[derive(Clone, Copy)]
pub struct RenderContext<'a> {
    /// The local date and time, read once when the abbreviation was matched.
    pub now: LocalDateTime,
    /// Locale for month and weekday names.
    pub locale: Locale,
    /// The date-format setting, used by `{date}` without `fmt`.
    pub date_format: &'a Format,
    /// The time-format setting, used by `{time}` without `fmt`.
    pub time_format: &'a Format,
    /// The clipboard text read at match time; `None` when it was empty, held no text, or was not
    /// read because [`Resolved::needs_clipboard`](super::Resolved::needs_clipboard) was false.
    pub clipboard: Option<&'a str>,
}

impl fmt::Debug for RenderContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderContext")
            .field("now", &self.now)
            .field("locale", &self.locale)
            .field("date_format", &self.date_format)
            .field("time_format", &self.time_format)
            .field("clipboard", &self.clipboard.map(|_| Redacted))
            .finish()
    }
}

/// One action of the output.
#[derive(Clone, PartialEq, Eq)]
pub enum RenderedAction {
    /// Type plain text (architecture §4.2 `TypeText`).
    Text {
        /// The text, with LF line endings.
        text: String,
        /// Byte ranges of `text` that came from field values (`text`, `area`, `choice`) or the
        /// clipboard, ascending and non-touching. Adaptive case must not change them.
        verbatim: Vec<Range<usize>>,
    },
    /// Press a key (`PressKey`).
    Key {
        /// The key.
        key: KeyName,
        /// Hold Shift.
        shift: bool,
        /// How many presses, 1 to 50.
        times: u8,
    },
    /// Pause before the next action (`Wait`), 1 to 10000 ms.
    Wait {
        /// Milliseconds.
        ms: u32,
    },
}

impl fmt::Debug for RenderedAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { text, verbatim } => f
                .debug_struct("Text")
                .field("len", &text.len())
                .field("verbatim", verbatim)
                .finish_non_exhaustive(),
            Self::Key { key, shift, times } => f
                .debug_struct("Key")
                .field("key", key)
                .field("shift", shift)
                .field("times", times)
                .finish(),
            Self::Wait { ms } => f.debug_struct("Wait").field("ms", ms).finish(),
        }
    }
}

/// The output of one expansion.
///
/// The text may contain field values and clipboard content, so `Debug` prints lengths only.
#[derive(Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The actions, in order. Adjacent text is merged; keys and waits stay separate.
    pub actions: Vec<RenderedAction>,
    /// When a `{cursor}` was rendered: how many extended grapheme clusters of typed text follow
    /// the first one (architecture §4.2 `MoveCaretBack`).
    pub cursor_back: Option<usize>,
}

impl Rendered {
    /// All typed text, concatenated (for previews and tests).
    #[must_use]
    pub fn text(&self) -> String {
        self.actions
            .iter()
            .filter_map(|a| match a {
                RenderedAction::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

impl fmt::Debug for Rendered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rendered")
            .field("actions", &self.actions)
            .field("cursor_back", &self.cursor_back)
            .finish()
    }
}

/// Where a piece of rendered text came from ([`Provenance`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The value of a fill-in field: its index in [`Resolved::form`](super::Resolved::form).
    Field(usize),
    /// The clipboard.
    Clipboard,
}

/// One piece of text that came from a field or the clipboard, for the editor's preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// The index of the [`RenderedAction::Text`] in [`Rendered::actions`].
    pub action: usize,
    /// Byte range in that action's text.
    pub range: Range<usize>,
    /// Where it came from.
    pub origin: Origin,
}

/// A render for the editor's preview ([`Resolved::render_preview`](super::Resolved::render_preview)).
///
/// The text may contain field values and clipboard content, so `Debug` prints lengths only.
#[derive(Clone, PartialEq, Eq)]
pub struct Preview {
    /// The actions, exactly as [`Resolved::render`](super::Resolved::render) gives them.
    pub rendered: Rendered,
    /// Every piece of text that came from a field or the clipboard, in output order.
    pub origins: Vec<Provenance>,
    /// Where the first rendered `{cursor}` is: the index of an action and a byte offset in its
    /// text. The index is the number of actions when no text comes before the cursor and none
    /// after it.
    pub cursor: Option<(usize, usize)>,
}

impl fmt::Debug for Preview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Preview")
            .field("rendered", &self.rendered)
            .field("origins", &self.origins)
            .field("cursor", &self.cursor)
            .finish()
    }
}

struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

struct Renderer<'r> {
    res: &'r Resolved,
    ctx: &'r RenderContext<'r>,
    /// The value of each form field, in form order, after [`field_value`].
    values: &'r [Cow<'r, str>],
    /// The clipboard with LF line endings, and trimmed: computed once per render, however many
    /// `{clipboard}` nodes the walk visits.
    clipboard: &'r str,
    clipboard_trimmed: &'r str,
    /// Date text by (body, date slot): each distinct node is computed at most once per render.
    dates: HashMap<(usize, usize), Result<String, ErrorKind>>,
    actions: Vec<RenderedAction>,
    typed_bytes: usize,
    waited_ms: u32,
    /// Typed bytes before the first rendered cursor.
    cursor_at: Option<usize>,
    steps: usize,
    /// Where text from fields and the clipboard went, when a preview asked for it.
    origins: Option<Vec<Provenance>>,
}

/// The value a field renders with: the submitted value checked against the field, or the
/// default (see the module docs of [`template`](super), "Form values").
fn field_value<'v>(spec: &'v FieldSpec, submitted: Option<&'v str>) -> Cow<'v, str> {
    let Some(value) = submitted else {
        return Cow::Borrowed(&spec.default);
    };
    match &spec.kind {
        FieldKind::Choice(options) if !options.iter().any(|o| o == value) => {
            Cow::Borrowed(&spec.default)
        }
        FieldKind::Toggle => Cow::Borrowed(if value == "yes" { "yes" } else { "no" }),
        FieldKind::Text if value.contains('\n') => Cow::Owned(value.replace('\n', " ")),
        _ => Cow::Borrowed(value),
    }
}

pub(crate) fn render(
    res: &Resolved,
    ctx: &RenderContext<'_>,
    values: &FormValues,
) -> Result<Rendered, TemplateError> {
    run_render(res, ctx, values, false).map(|(rendered, _, _)| rendered)
}

pub(crate) fn render_preview(
    res: &Resolved,
    ctx: &RenderContext<'_>,
    values: &FormValues,
) -> Result<Preview, TemplateError> {
    let (rendered, origins, cursor_at) = run_render(res, ctx, values, true)?;
    let cursor = cursor_at.map(|at| {
        let mut before = 0;
        for (i, action) in rendered.actions.iter().enumerate() {
            if let RenderedAction::Text { text, .. } = action {
                if at <= before + text.len() {
                    return (i, at - before);
                }
                before += text.len();
            }
        }
        (rendered.actions.len(), 0)
    });
    Ok(Preview {
        rendered,
        origins,
        cursor,
    })
}

/// Renders; with `track`, also records where field and clipboard text went.
fn run_render(
    res: &Resolved,
    ctx: &RenderContext<'_>,
    values: &FormValues,
    track: bool,
) -> Result<(Rendered, Vec<Provenance>, Option<usize>), TemplateError> {
    let clipboard = if res.needs_clipboard {
        normalize_newlines(ctx.clipboard.unwrap_or(""))
    } else {
        Cow::Borrowed("")
    };
    let values: Vec<Cow<'_, str>> = res
        .form
        .fields()
        .iter()
        .map(|spec| field_value(spec, values.get(&spec.name)))
        .collect();
    let mut r = Renderer {
        res,
        ctx,
        values: &values,
        clipboard: &clipboard,
        clipboard_trimmed: clipboard
            .trim_matches(|c: char| c.is_whitespace() || is_info_separator(c)),
        dates: HashMap::new(),
        actions: Vec::new(),
        typed_bytes: 0,
        waited_ms: 0,
        cursor_at: None,
        steps: 0,
        origins: track.then(Vec::new),
    };
    r.run(&res.root, &res.root.body.nodes)
        .map_err(TemplateError::new)?;
    if r.over_char_limit() {
        return Err(TemplateError::new(ErrorKind::OutputTooLarge));
    }
    let cursor_back = r.cursor_at.map(|at| {
        let mut skip = at;
        let mut tail = String::new();
        for a in &r.actions {
            if let RenderedAction::Text { text, .. } = a {
                let from = skip.min(text.len());
                skip -= from;
                tail.push_str(&text[from..]);
            }
        }
        tail.graphemes(true).count()
    });
    let cursor_at = r.cursor_at;
    let origins = r.origins.take().unwrap_or_default();
    Ok((
        Rendered {
            actions: r.actions,
            cursor_back,
        },
        origins,
        cursor_at,
    ))
}

impl<'r> Renderer<'r> {
    /// Fails with `kind`, unless the text typed so far already exceeds the character limit: the
    /// reference implementation checks that limit on every piece of text, so it comes first.
    fn fail<T>(&self, kind: ErrorKind) -> Result<T, ErrorKind> {
        if self.over_char_limit() {
            Err(ErrorKind::OutputTooLarge)
        } else {
            Err(kind)
        }
    }

    /// Whether the typed text has more than [`MAX_OUTPUT_CHARS`] grapheme clusters. The count
    /// only grows as text is appended, so checking once before an error, and once at the end,
    /// gives the same result as checking after every piece.
    fn over_char_limit(&self) -> bool {
        if self.typed_bytes <= MAX_OUTPUT_CHARS {
            return false;
        }
        let texts = self.actions.iter().filter_map(|a| match a {
            RenderedAction::Text { text, .. } => Some(text.as_str()),
            _ => None,
        });
        let count = if self.actions.len() == 1 {
            texts.map(|t| t.graphemes(true).count()).sum()
        } else {
            texts.collect::<String>().graphemes(true).count()
        };
        count > MAX_OUTPUT_CHARS
    }

    /// Appends text. `origin` is set for text from a field value or the clipboard, which is
    /// verbatim (adaptive case leaves it alone).
    fn emit(&mut self, s: &str, origin: Option<Origin>) -> Result<(), ErrorKind> {
        if s.is_empty() {
            return Ok(());
        }
        self.typed_bytes += s.len();
        if self.typed_bytes > MAX_OUTPUT_BYTES {
            return Err(ErrorKind::OutputTooLarge);
        }
        let verbatim = origin.is_some();
        let range = if let Some(RenderedAction::Text {
            text,
            verbatim: ranges,
        }) = self.actions.last_mut()
        {
            let start = text.len();
            text.push_str(s);
            if verbatim {
                push_range(ranges, start..text.len());
            }
            start..text.len()
        } else {
            let mut ranges = Vec::new();
            if verbatim {
                ranges.push(0..s.len());
            }
            self.actions.push(RenderedAction::Text {
                text: s.to_owned(),
                verbatim: ranges,
            });
            0..s.len()
        };
        if let (Some(origins), Some(origin)) = (&mut self.origins, origin) {
            origins.push(Provenance {
                action: self.actions.len() - 1,
                range,
                origin,
            });
        }
        Ok(())
    }

    fn run(&mut self, at: &'r Expanded, nodes: &'r [Node]) -> Result<(), ErrorKind> {
        for node in nodes {
            self.steps += 1;
            if self.steps > MAX_RENDER_STEPS {
                return Err(ErrorKind::OutputTooLarge);
            }
            match node {
                Node::Text(t) => self.emit(t, None)?,
                Node::Date {
                    time,
                    format,
                    shift,
                    slot,
                } => {
                    let format = format.as_ref().unwrap_or(if *time {
                        self.ctx.time_format
                    } else {
                        self.ctx.date_format
                    });
                    let key = (Arc::as_ptr(&at.body).addr(), *slot);
                    match self.date_text(key, format, shift) {
                        Ok(text) => self.emit(&text, None)?,
                        Err(kind) => return self.fail(kind),
                    }
                }
                Node::Clipboard { trim } => {
                    let clip = if *trim {
                        self.clipboard_trimmed
                    } else {
                        self.clipboard
                    };
                    self.emit(clip, Some(Origin::Clipboard))?;
                }
                Node::Cursor => {
                    if self.cursor_at.is_none() {
                        self.cursor_at = Some(self.typed_bytes);
                    }
                }
                Node::Snippet(index) => {
                    let child: &'r Expanded = &at.snippets[*index];
                    self.run(child, &child.body.nodes)?;
                }
                Node::Field { slot, toggle } => {
                    let values = self.values;
                    let index = self.field(at, *slot);
                    let value: &str = &values[index];
                    match toggle {
                        Some(t) => {
                            let text = if value == "yes" { &t.on } else { &t.off };
                            self.emit(text, None)?;
                        }
                        None => self.emit(value, Some(Origin::Field(index)))?,
                    }
                }
                Node::Key { key, shift, times } => {
                    if self.cursor_at.is_some() {
                        return self.fail(ErrorKind::CursorBeforeKey);
                    }
                    self.actions.push(RenderedAction::Key {
                        key: *key,
                        shift: *shift,
                        times: *times,
                    });
                }
                Node::Wait { ms } => {
                    self.waited_ms = self.waited_ms.saturating_add(*ms);
                    if self.waited_ms > MAX_WAIT_TOTAL_MS {
                        return self.fail(ErrorKind::WaitTotal);
                    }
                    self.actions.push(RenderedAction::Wait { ms: *ms });
                }
                Node::If {
                    slot,
                    cond,
                    then,
                    otherwise,
                } => {
                    let taken = self.condition(self.field(at, *slot), cond);
                    self.run(at, if taken { then } else { otherwise })?;
                }
            }
        }
        Ok(())
    }
}

impl Renderer<'_> {
    /// The form index of field slot `slot` of the body `at`.
    fn field(&self, at: &Expanded, slot: usize) -> usize {
        self.res
            .form
            .position(at.body.field_names[slot].as_str())
            .unwrap_or(0)
    }

    /// The text of the date node `key` (body, slot), computed at most once per render.
    fn date_text(
        &mut self,
        key: (usize, usize),
        format: &Format,
        shift: &Shift,
    ) -> Result<String, ErrorKind> {
        if let Some(done) = self.dates.get(&key) {
            return done.clone();
        }
        let done = shift
            .apply(self.ctx.now)
            .map(|t| format.render(&t, &self.ctx.locale))
            .map_err(|_| ErrorKind::DateRange);
        self.dates.insert(key, done.clone());
        done
    }

    /// Whether an `{if}` on form field `field` takes its first branch (spec §4.8).
    fn condition(&self, field: usize, cond: &Cond) -> bool {
        let value: &str = &self.values[field];
        match cond {
            Cond::Truthy if self.res.form.fields()[field].kind == FieldKind::Toggle => {
                value == "yes"
            }
            Cond::Truthy => !value.is_empty(),
            Cond::Is(x) => value == x,
            Cond::Not(x) => value != x,
        }
    }
}

/// Appends `range` to ascending `ranges`, merging it with the last one when they touch.
fn push_range(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    match ranges.last_mut() {
        Some(last) if last.end == range.start => last.end = range.end,
        _ => ranges.push(range),
    }
}
