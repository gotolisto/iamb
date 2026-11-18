//! Lexical layer: line-ending normalisation, positions, and the tokenizer (spec §2).
//!
//! The tokenizer works on bytes of the normalised text: every character with a syntactic role is
//! ASCII, and UTF-8 continuation bytes never equal an ASCII byte.

use super::error::ErrorKind;
use std::borrow::Cow;
use std::ops::Range;

/// The twelve macro names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacroName {
    /// `{date}`
    Date,
    /// `{time}`
    Time,
    /// `{clipboard}`
    Clipboard,
    /// `{cursor}`
    Cursor,
    /// `{snippet}`
    Snippet,
    /// `{field}`
    Field,
    /// `{key}`
    Key,
    /// `{wait}`
    Wait,
    /// `{if}`
    If,
    /// `{else}`
    Else,
    /// `{endif}`
    Endif,
    /// `{lbrace}`
    Lbrace,
}

impl MacroName {
    /// Every macro name.
    pub const ALL: [MacroName; 12] = [
        Self::Date,
        Self::Time,
        Self::Clipboard,
        Self::Cursor,
        Self::Snippet,
        Self::Field,
        Self::Key,
        Self::Wait,
        Self::If,
        Self::Else,
        Self::Endif,
        Self::Lbrace,
    ];

    /// The name as written, e.g. `snippet`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Date => "date",
            Self::Time => "time",
            Self::Clipboard => "clipboard",
            Self::Cursor => "cursor",
            Self::Snippet => "snippet",
            Self::Field => "field",
            Self::Key => "key",
            Self::Wait => "wait",
            Self::If => "if",
            Self::Else => "else",
            Self::Endif => "endif",
            Self::Lbrace => "lbrace",
        }
    }

    /// Looks a macro up by its exact name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<MacroName> {
        Some(match name {
            "date" => Self::Date,
            "time" => Self::Time,
            "clipboard" => Self::Clipboard,
            "cursor" => Self::Cursor,
            "snippet" => Self::Snippet,
            "field" => Self::Field,
            "key" => Self::Key,
            "wait" => Self::Wait,
            "if" => Self::If,
            "else" => Self::Else,
            "endif" => Self::Endif,
            "lbrace" => Self::Lbrace,
            _ => return None,
        })
    }

    fn allows(self, attr: AttrName) -> bool {
        use AttrName as A;
        match self {
            Self::Date | Self::Time => matches!(attr, A::Fmt | A::Shift),
            Self::Clipboard => attr == A::Trim,
            Self::Snippet => attr == A::Abbr,
            Self::Field => matches!(
                attr,
                A::Name | A::Type | A::Default | A::Options | A::On | A::Off
            ),
            Self::Key => matches!(attr, A::Name | A::Shift | A::Times),
            Self::Wait => attr == A::Ms,
            Self::If => matches!(attr, A::Field | A::Is | A::Not),
            Self::Cursor | Self::Else | Self::Endif | Self::Lbrace => false,
        }
    }
}

/// Every attribute name any macro accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttrName {
    Fmt,
    Shift,
    Trim,
    Abbr,
    Name,
    Type,
    Default,
    Options,
    On,
    Off,
    Times,
    Ms,
    Field,
    Is,
    Not,
}

impl AttrName {
    fn from_name(name: &str) -> Option<AttrName> {
        Some(match name {
            "fmt" => Self::Fmt,
            "shift" => Self::Shift,
            "trim" => Self::Trim,
            "abbr" => Self::Abbr,
            "name" => Self::Name,
            "type" => Self::Type,
            "default" => Self::Default,
            "options" => Self::Options,
            "on" => Self::On,
            "off" => Self::Off,
            "times" => Self::Times,
            "ms" => Self::Ms,
            "field" => Self::Field,
            "is" => Self::Is,
            "not" => Self::Not,
            _ => return None,
        })
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Fmt => "fmt",
            Self::Shift => "shift",
            Self::Trim => "trim",
            Self::Abbr => "abbr",
            Self::Name => "name",
            Self::Type => "type",
            Self::Default => "default",
            Self::Options => "options",
            Self::On => "on",
            Self::Off => "off",
            Self::Times => "times",
            Self::Ms => "ms",
            Self::Field => "field",
            Self::Is => "is",
            Self::Not => "not",
        }
    }
}

/// One attribute of a macro token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// The attribute name, e.g. `fmt`.
    pub name: &'static str,
    /// The value with quotes removed and `\"`, `\\` unescaped.
    pub value: String,
}

/// A piece of a template's source, for the editor's chips (task 3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// Literal text, typed as written.
    Text {
        /// `char` range in the source.
        range: Range<usize>,
    },
    /// A macro, including `{lbrace}`, `{else}` and `{endif}`.
    Macro {
        /// The macro name.
        name: MacroName,
        /// The attributes, in source order.
        attributes: Vec<Attribute>,
        /// `char` range in the source, from `{` to `}` inclusive.
        range: Range<usize>,
    },
}

/// Replaces CRLF and lone CR with LF (spec §2.3).
pub(crate) fn normalize_newlines(s: &str) -> Cow<'_, str> {
    if s.contains('\r') {
        Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(s)
    }
}

/// Normalised text plus what is needed to map its positions back to the source.
pub(crate) struct Normalized<'a> {
    pub(crate) text: Cow<'a, str>,
    /// `char` indices in `text` of the LFs that replaced a CRLF (two source chars).
    collapsed: Vec<usize>,
}

impl<'a> Normalized<'a> {
    pub(crate) fn new(source: &'a str) -> Self {
        if !source.contains('\r') {
            return Self {
                text: Cow::Borrowed(source),
                collapsed: Vec::new(),
            };
        }
        let mut text = String::with_capacity(source.len());
        let mut collapsed = Vec::new();
        let mut chars = source.chars().peekable();
        let mut index = 0;
        while let Some(c) = chars.next() {
            if c == '\r' {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    collapsed.push(index);
                }
                text.push('\n');
            } else {
                text.push(c);
            }
            index += 1;
        }
        Self {
            text: Cow::Owned(text),
            collapsed,
        }
    }

    /// A converter from byte offsets in the normalised text to `char` offsets in the source.
    pub(crate) fn positions(&self) -> Positions<'_> {
        Positions {
            text: &self.text,
            collapsed: &self.collapsed,
            byte: 0,
            chars: 0,
        }
    }
}

/// Converts byte offsets of normalised text into `char` offsets of the source. Calls with
/// increasing offsets cost time proportional to the distance moved.
pub(crate) struct Positions<'a> {
    text: &'a str,
    collapsed: &'a [usize],
    byte: usize,
    chars: usize,
}

impl Positions<'_> {
    pub(crate) fn at(&mut self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        if byte < self.byte {
            self.byte = 0;
            self.chars = 0;
        }
        self.chars += self.text[self.byte..byte].chars().count();
        self.byte = byte;
        self.chars + self.collapsed.partition_point(|&k| k < self.chars)
    }
}

/// A tokenizer error: kind and byte offset in the normalised text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyntaxError {
    pub(crate) kind: ErrorKind,
    pub(crate) at: usize,
}

const fn err(kind: ErrorKind, at: usize) -> SyntaxError {
    SyntaxError { kind, at }
}

/// One attribute as written. Offsets are bytes in the normalised text.
#[derive(Debug)]
pub(crate) struct RawAttr<'a> {
    pub(crate) name: AttrName,
    pub(crate) name_at: usize,
    pub(crate) value: Cow<'a, str>,
    pub(crate) value_at: usize,
}

/// One macro as written. Offsets are bytes in the normalised text.
#[derive(Debug)]
pub(crate) struct RawMacro<'a> {
    pub(crate) name: MacroName,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) attrs: Vec<RawAttr<'a>>,
}

impl<'a> RawMacro<'a> {
    pub(crate) fn attr(&self, name: AttrName) -> Option<&RawAttr<'a>> {
        self.attrs.iter().find(|a| a.name == name)
    }
}

#[derive(Debug)]
pub(crate) enum RawToken<'a> {
    Text(Range<usize>),
    Macro(RawMacro<'a>),
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n')
}

fn is_bare(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'.' | b'%' | b':' | b'/')
}

fn lower_run(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && bytes[at].is_ascii_lowercase() {
        at += 1;
    }
    at
}

/// Splits normalised text into text runs and macros (spec §2.1, §2.2). Attribute names are
/// checked against the macro here; attribute values are checked by the parser.
pub(crate) fn tokenize(src: &str) -> Result<Vec<RawToken<'_>>, SyntaxError> {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut out = Vec::new();
    let mut text_start = 0;
    let mut at = 0;
    while at < len {
        let Some(off) = bytes[at..].iter().position(|&c| c == b'{') else {
            break;
        };
        at += off;
        let name_end = lower_run(bytes, at + 1);
        let Some(name) = MacroName::from_name(&src[at + 1..name_end]) else {
            // Not a macro name: the `{` is literal, and so are the letters after it.
            at = name_end.max(at + 1);
            continue;
        };
        if name_end >= len {
            return Err(err(ErrorKind::UnclosedMacro, at));
        }
        if bytes[name_end] != b'}' && !is_ws(bytes[name_end]) {
            at = name_end;
            continue;
        }
        if text_start < at {
            out.push(RawToken::Text(text_start..at));
        }
        let (attrs, end) = parse_attrs(src, name_end, name, at)?;
        out.push(RawToken::Macro(RawMacro {
            name,
            start: at,
            end,
            attrs,
        }));
        at = end;
        text_start = end;
    }
    if text_start < len {
        out.push(RawToken::Text(text_start..len));
    }
    Ok(out)
}

/// Reads the attributes of the macro starting at byte `start`, from byte `at` (just after the
/// name) to the closing `}`. Returns them and the offset just past the `}`.
fn parse_attrs(
    src: &str,
    mut at: usize,
    name: MacroName,
    start: usize,
) -> Result<(Vec<RawAttr<'_>>, usize), SyntaxError> {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut attrs: Vec<RawAttr<'_>> = Vec::new();
    loop {
        while at < len && is_ws(bytes[at]) {
            at += 1;
        }
        if at >= len {
            return Err(err(ErrorKind::UnclosedMacro, start));
        }
        if bytes[at] == b'}' {
            return Ok((attrs, at + 1));
        }
        let name_at = at;
        let key_end = lower_run(bytes, at);
        if key_end >= len {
            return Err(err(ErrorKind::UnclosedMacro, start));
        }
        if key_end == at || bytes[key_end] != b'=' {
            return Err(err(ErrorKind::BadSyntax, key_end));
        }
        let key = &src[at..key_end];
        at = key_end + 1;
        if at >= len {
            return Err(err(ErrorKind::UnclosedMacro, start));
        }
        let value_at = at;
        let (value, value_end) = if bytes[at] == b'"' {
            quoted_value(src, at)?
        } else {
            let mut end = at;
            while end < len && is_bare(bytes[end]) {
                end += 1;
            }
            if end == at {
                return Err(err(ErrorKind::BadSyntax, at));
            }
            (Cow::Borrowed(&src[at..end]), end)
        };
        at = value_end;
        if at < len && !is_ws(bytes[at]) && bytes[at] != b'}' {
            return Err(err(ErrorKind::BadSyntax, at));
        }
        let attr = AttrName::from_name(key)
            .filter(|&a| name.allows(a))
            .ok_or(err(ErrorKind::UnknownAttr, name_at))?;
        if attrs.iter().any(|a| a.name == attr) {
            return Err(err(ErrorKind::DuplicateAttr, name_at));
        }
        attrs.push(RawAttr {
            name: attr,
            name_at,
            value,
            value_at,
        });
    }
}

/// Reads a quoted value whose opening quote is at byte `quote`. Returns the unescaped value and
/// the offset just past the closing quote.
fn quoted_value(src: &str, quote: usize) -> Result<(Cow<'_, str>, usize), SyntaxError> {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut at = quote + 1;
    let mut owned: Option<String> = None;
    let mut seg = at;
    loop {
        if at >= len {
            return Err(err(ErrorKind::UnclosedMacro, quote));
        }
        match bytes[at] {
            b'\\' if at + 1 < len && matches!(bytes[at + 1], b'"' | b'\\') => {
                let o = owned.get_or_insert_with(String::new);
                o.push_str(&src[seg..at]);
                o.push(char::from(bytes[at + 1]));
                at += 2;
                seg = at;
            }
            b'"' => break,
            _ => at += 1,
        }
    }
    let value = match owned {
        Some(mut o) => {
            o.push_str(&src[seg..at]);
            Cow::Owned(o)
        }
        None => Cow::Borrowed(&src[seg..at]),
    };
    Ok((value, at + 1))
}

/// The token list of a source that is known to tokenize; an error ends the list early.
pub(crate) fn tokens(source: &str) -> Vec<Token> {
    let norm = Normalized::new(source);
    let mut pos = norm.positions();
    let Ok(raw) = tokenize(&norm.text) else {
        return Vec::new();
    };
    raw.into_iter()
        .map(|t| match t {
            RawToken::Text(r) => Token::Text {
                range: pos.at(r.start)..pos.at(r.end),
            },
            RawToken::Macro(m) => Token::Macro {
                name: m.name,
                range: pos.at(m.start)..pos.at(m.end),
                attributes: m
                    .attrs
                    .into_iter()
                    .map(|a| Attribute {
                        name: a.name.as_str(),
                        value: a.value.into_owned(),
                    })
                    .collect(),
            },
        })
        .collect()
}
