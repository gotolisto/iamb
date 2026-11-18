//! Parser: validates macros and builds the node tree of one template body (spec §2, §4).
//!
//! The order in which errors are found follows `spec/template/reference.py`: the whole body is
//! tokenized first (syntax, unknown and repeated attributes), then each macro is validated and
//! placed in the `{if}` structure in source order, then an unclosed `{if}` is reported.

use super::error::{ErrorKind, TemplateError};
use super::syntax::{
    self, AttrName, MacroName, Normalized, Positions, RawAttr, RawMacro, RawToken, SyntaxError,
};
use super::{FieldKind, KeyName, MAX_DEPTH};
use crate::datetime::{DateError, Format, Shift};
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;

/// Longest field name, in grapheme clusters.
const MAX_FIELD_NAME: usize = 64;

/// A parsed template body.
#[derive(Debug)]
pub(crate) struct Body {
    pub(crate) nodes: Vec<Node>,
    /// One entry per `{snippet}`, in source order (which is also tree order).
    pub(crate) snippet_refs: Vec<SnippetRef>,
    /// Distinct field names used by `{field}` and `{if}`; nodes refer to them by slot.
    pub(crate) field_names: Vec<String>,
    /// `{field}`, `{if}` and `{snippet}` in source order, for building the form.
    pub(crate) form_events: Vec<FormEvent>,
    /// Number of `{date}` and `{time}` nodes; each has a slot below this.
    pub(crate) date_count: usize,
    pub(crate) has_clipboard: bool,
    pub(crate) has_cursor: bool,
}

#[derive(Debug)]
pub(crate) struct SnippetRef {
    pub(crate) abbr: String,
    /// `char` offset of the macro in the source.
    pub(crate) at: usize,
}

#[derive(Debug)]
pub(crate) enum FormEvent {
    /// A `{field}`: its slot, type, default and position.
    Field {
        slot: usize,
        kind: FieldKind,
        default: String,
        at: usize,
    },
    /// An `{if}`: its field slot, whether it has neither `is` nor `not`, and its position.
    If { slot: usize, bare: bool, at: usize },
    /// The `index`-th `{snippet}` of the body.
    Snippet(usize),
}

#[derive(Debug)]
pub(crate) enum Node {
    Text(String),
    Date {
        time: bool,
        format: Option<Format>,
        shift: Shift,
        slot: usize,
    },
    Clipboard {
        trim: bool,
    },
    Cursor,
    /// The `index`-th `{snippet}` of the body.
    Snippet(usize),
    Field {
        slot: usize,
        toggle: Option<Toggle>,
    },
    Key {
        key: KeyName,
        shift: bool,
        times: u8,
    },
    Wait {
        ms: u32,
    },
    If {
        slot: usize,
        cond: Cond,
        then: Vec<Node>,
        otherwise: Vec<Node>,
    },
}

/// The texts a toggle field inserts.
#[derive(Debug)]
pub(crate) struct Toggle {
    pub(crate) on: String,
    pub(crate) off: String,
}

#[derive(Debug)]
pub(crate) enum Cond {
    Truthy,
    Is(String),
    Not(String),
}

struct OpenIf {
    slot: usize,
    cond: Cond,
    at: usize,
    then: Vec<Node>,
    otherwise: Vec<Node>,
    in_else: bool,
}

impl OpenIf {
    fn current(&mut self) -> &mut Vec<Node> {
        if self.in_else {
            &mut self.otherwise
        } else {
            &mut self.then
        }
    }
}

struct Builder {
    root: Vec<Node>,
    open: Vec<OpenIf>,
    slots: HashMap<String, usize>,
    body: Body,
}

impl Builder {
    fn current(&mut self) -> &mut Vec<Node> {
        match self.open.last_mut() {
            Some(open) => open.current(),
            None => &mut self.root,
        }
    }

    fn push_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let cur = self.current();
        if let Some(Node::Text(t)) = cur.last_mut() {
            t.push_str(s);
        } else {
            cur.push(Node::Text(s.to_owned()));
        }
    }

    fn slot(&mut self, name: &str) -> usize {
        if let Some(&slot) = self.slots.get(name) {
            return slot;
        }
        let slot = self.body.field_names.len();
        self.body.field_names.push(name.to_owned());
        self.slots.insert(name.to_owned(), slot);
        slot
    }
}

/// Parses one body. Errors carry `char` positions in `source`.
pub(crate) fn parse_body(source: &str) -> Result<Body, TemplateError> {
    let norm = Normalized::new(source);
    let mut pos = norm.positions();
    parse_normalized(&norm.text, &mut pos).map_err(|e| {
        let mut fresh = norm.positions();
        TemplateError::at(e.kind, fresh.at(e.at))
    })
}

fn parse_normalized(s: &str, pos: &mut Positions<'_>) -> Result<Body, SyntaxError> {
    let tokens = syntax::tokenize(s)?;
    let mut b = Builder {
        root: Vec::new(),
        open: Vec::new(),
        slots: HashMap::new(),
        body: Body {
            nodes: Vec::new(),
            snippet_refs: Vec::new(),
            field_names: Vec::new(),
            form_events: Vec::new(),
            date_count: 0,
            has_clipboard: false,
            has_cursor: false,
        },
    };
    for tok in tokens {
        match tok {
            RawToken::Text(r) => b.push_text(&s[r]),
            RawToken::Macro(m) => place(&mut b, &m, pos)?,
        }
    }
    if let Some(open) = b.open.last() {
        return Err(SyntaxError {
            kind: ErrorKind::UnbalancedIf,
            at: open.at,
        });
    }
    let mut body = b.body;
    body.nodes = b.root;
    Ok(body)
}

fn fail(kind: ErrorKind, at: usize) -> SyntaxError {
    SyntaxError { kind, at }
}

/// Validates one macro and places it in the tree.
fn place(b: &mut Builder, m: &RawMacro<'_>, pos: &mut Positions<'_>) -> Result<(), SyntaxError> {
    let need = |name: AttrName| -> Result<&RawAttr<'_>, SyntaxError> {
        let a = m.attr(name).ok_or(fail(ErrorKind::MissingAttr, m.start))?;
        if a.value.is_empty() {
            return Err(fail(ErrorKind::BadValue, a.value_at));
        }
        Ok(a)
    };
    match m.name {
        MacroName::Date | MacroName::Time => place_date(b, m)?,
        MacroName::Clipboard => {
            let trim = yes_no(m.attr(AttrName::Trim), false)?;
            b.body.has_clipboard = true;
            b.current().push(Node::Clipboard { trim });
        }
        MacroName::Cursor => {
            b.body.has_cursor = true;
            b.current().push(Node::Cursor);
        }
        MacroName::Lbrace => b.push_text("{"),
        MacroName::Snippet => {
            let abbr = need(AttrName::Abbr)?.value.to_string();
            let index = b.body.snippet_refs.len();
            let at = pos.at(m.start);
            b.body.snippet_refs.push(SnippetRef { abbr, at });
            b.body.form_events.push(FormEvent::Snippet(index));
            b.current().push(Node::Snippet(index));
        }
        MacroName::Field => place_field(b, m, pos, need(AttrName::Name)?)?,
        MacroName::Key => {
            let a = need(AttrName::Name)?;
            let key = KeyName::from_name(&a.value).ok_or(fail(ErrorKind::BadValue, a.value_at))?;
            let shift = yes_no(m.attr(AttrName::Shift), false)?;
            let times = match m.attr(AttrName::Times) {
                None => 1,
                Some(a) => int_in(a, 1, 50)?,
            };
            let times = u8::try_from(times).map_err(|_| fail(ErrorKind::BadValue, m.start))?;
            b.current().push(Node::Key { key, shift, times });
        }
        MacroName::Wait => {
            let ms = int_in(need(AttrName::Ms)?, 1, 10_000)?;
            b.current().push(Node::Wait { ms });
        }
        MacroName::If => place_if(b, m, pos, need(AttrName::Field)?)?,
        MacroName::Else => match b.open.last_mut() {
            Some(open) if !open.in_else => open.in_else = true,
            _ => return Err(fail(ErrorKind::UnbalancedIf, m.start)),
        },
        MacroName::Endif => {
            let open = b.open.pop().ok_or(fail(ErrorKind::UnbalancedIf, m.start))?;
            b.current().push(Node::If {
                slot: open.slot,
                cond: open.cond,
                then: open.then,
                otherwise: open.otherwise,
            });
        }
    }
    Ok(())
}

fn place_date(b: &mut Builder, m: &RawMacro<'_>) -> Result<(), SyntaxError> {
    let format = match m.attr(AttrName::Fmt) {
        None => None,
        Some(a) => Some(Format::parse(&a.value).map_err(|e| {
            let kind = if e == DateError::EmptyFormat {
                ErrorKind::BadValue
            } else {
                ErrorKind::BadFormat
            };
            fail(kind, a.value_at)
        })?),
    };
    let shift = match m.attr(AttrName::Shift) {
        None => Shift::parse("").map_err(|_| fail(ErrorKind::BadShift, m.start))?,
        Some(a) => Shift::parse(&separator_spaces(&a.value))
            .map_err(|_| fail(ErrorKind::BadShift, a.value_at))?,
    };
    let slot = b.body.date_count;
    b.body.date_count += 1;
    b.current().push(Node::Date {
        time: m.name == MacroName::Time,
        format,
        shift,
        slot,
    });
    Ok(())
}

fn place_if(
    b: &mut Builder,
    m: &RawMacro<'_>,
    pos: &mut Positions<'_>,
    field: &RawAttr<'_>,
) -> Result<(), SyntaxError> {
    let cond = match (m.attr(AttrName::Is), m.attr(AttrName::Not)) {
        (Some(is), Some(not)) => {
            return Err(fail(ErrorKind::BadValue, is.value_at.max(not.value_at)));
        }
        (Some(is), None) => Cond::Is(is.value.to_string()),
        (None, Some(not)) => Cond::Not(not.value.to_string()),
        (None, None) => Cond::Truthy,
    };
    if b.open.len() >= MAX_DEPTH {
        return Err(fail(ErrorKind::TooDeep, m.start));
    }
    let slot = b.slot(&field.value);
    b.body.form_events.push(FormEvent::If {
        slot,
        bare: matches!(cond, Cond::Truthy),
        at: pos.at(m.start),
    });
    b.open.push(OpenIf {
        slot,
        cond,
        at: m.start,
        then: Vec::new(),
        otherwise: Vec::new(),
        in_else: false,
    });
    Ok(())
}

fn place_field(
    b: &mut Builder,
    m: &RawMacro<'_>,
    pos: &mut Positions<'_>,
    name: &RawAttr<'_>,
) -> Result<(), SyntaxError> {
    if name.value.contains('\n') || name.value.graphemes(true).nth(MAX_FIELD_NAME).is_some() {
        return Err(fail(ErrorKind::BadValue, name.value_at));
    }
    let kind_attr = m.attr(AttrName::Type);
    let type_name = kind_attr.map_or("text", |a| &*a.value);
    if !matches!(type_name, "text" | "area" | "choice" | "toggle") {
        let at = kind_attr.map_or(m.start, |a| a.value_at);
        return Err(fail(ErrorKind::BadValue, at));
    }
    if let Some(extra) = m.attrs.iter().find(|a| match a.name {
        AttrName::Name | AttrName::Type | AttrName::Default => false,
        AttrName::Options => type_name != "choice",
        AttrName::On | AttrName::Off => type_name != "toggle",
        _ => true,
    }) {
        return Err(fail(ErrorKind::UnknownAttr, extra.name_at));
    }
    let default_attr = m.attr(AttrName::Default);
    let (kind, default, toggle) = match type_name {
        "choice" => {
            let a = m
                .attr(AttrName::Options)
                .ok_or(fail(ErrorKind::MissingAttr, m.start))?;
            let options: Vec<String> = a.value.split('|').map(str::to_owned).collect();
            let has_duplicate = options
                .iter()
                .enumerate()
                .any(|(i, o)| options[..i].contains(o));
            if options.iter().any(String::is_empty) || has_duplicate {
                return Err(fail(ErrorKind::BadValue, a.value_at));
            }
            let default = match default_attr {
                None => options[0].clone(),
                Some(d) if options.iter().any(|o| *o == d.value) => d.value.to_string(),
                Some(d) => return Err(fail(ErrorKind::BadValue, d.value_at)),
            };
            (FieldKind::Choice(options), default, None)
        }
        "toggle" => {
            let checked = yes_no(default_attr, false)?;
            let text = |n: AttrName| m.attr(n).map_or_else(String::new, |a| a.value.to_string());
            let toggle = Toggle {
                on: text(AttrName::On),
                off: text(AttrName::Off),
            };
            let default = if checked { "yes" } else { "no" };
            (FieldKind::Toggle, default.to_owned(), Some(toggle))
        }
        _ => {
            let default = default_attr.map_or_else(String::new, |a| a.value.to_string());
            let area = type_name == "area";
            if !area && default.contains('\n') {
                let at = default_attr.map_or(m.start, |a| a.value_at);
                return Err(fail(ErrorKind::BadValue, at));
            }
            let kind = if area {
                FieldKind::Area
            } else {
                FieldKind::Text
            };
            (kind, default, None)
        }
    };
    let slot = b.slot(&name.value);
    b.body.form_events.push(FormEvent::Field {
        slot,
        kind,
        default,
        at: pos.at(m.start),
    });
    b.current().push(Node::Field { slot, toggle });
    Ok(())
}

fn yes_no(attr: Option<&RawAttr<'_>>, default: bool) -> Result<bool, SyntaxError> {
    match attr {
        None => Ok(default),
        Some(a) => match &*a.value {
            "yes" => Ok(true),
            "no" => Ok(false),
            _ => Err(fail(ErrorKind::BadValue, a.value_at)),
        },
    }
}

/// A whole number written with 1 to 6 ASCII digits, within `lo..=hi`.
fn int_in(a: &RawAttr<'_>, lo: u32, hi: u32) -> Result<u32, SyntaxError> {
    let v = &*a.value;
    if v.is_empty() || v.len() > 6 || !v.bytes().all(|c| c.is_ascii_digit()) {
        return Err(fail(ErrorKind::BadValue, a.value_at));
    }
    v.parse::<u32>()
        .ok()
        .filter(|n| (lo..=hi).contains(n))
        .ok_or(fail(ErrorKind::BadValue, a.value_at))
}

/// The reference implementation splits shift terms with Python's `str.split()`, which also
/// treats U+001C..U+001F as white space; Rust's `split_whitespace` does not. Map them to spaces
/// so both agree.
fn separator_spaces(s: &str) -> std::borrow::Cow<'_, str> {
    if s.chars().any(is_info_separator) {
        std::borrow::Cow::Owned(
            s.chars()
                .map(|c| if is_info_separator(c) { ' ' } else { c })
                .collect(),
        )
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// U+001C..U+001F, the characters Python counts as white space and Rust does not.
pub(crate) fn is_info_separator(c: char) -> bool {
    ('\u{1c}'..='\u{1f}').contains(&c)
}
