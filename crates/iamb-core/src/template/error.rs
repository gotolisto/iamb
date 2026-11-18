//! Error codes of the template language (spec §5).

use std::fmt;

/// One error code of spec §5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// `E_UNCLOSED_MACRO`: a macro or a quoted value has no end.
    UnclosedMacro,
    /// `E_BAD_SYNTAX`: malformed attribute.
    BadSyntax,
    /// `E_UNKNOWN_ATTR`: attribute not accepted by this macro or field type.
    UnknownAttr,
    /// `E_DUPLICATE_ATTR`: attribute given twice.
    DuplicateAttr,
    /// `E_MISSING_ATTR`: required attribute absent.
    MissingAttr,
    /// `E_BAD_VALUE`: value empty where one is required, or outside the allowed set or range.
    BadValue,
    /// `E_BAD_FORMAT`: unknown date or time specifier, or a lone `%`.
    BadFormat,
    /// `E_BAD_SHIFT`: shift term without a sign, number or known unit.
    BadShift,
    /// `E_UNBALANCED_IF`: `{if}`, `{else}`, `{endif}` do not pair up.
    UnbalancedIf,
    /// `E_TOO_DEEP`: more than 8 levels of `{if}` or of nested snippets.
    TooDeep,
    /// `E_SNIPPET_NOT_FOUND`: `{snippet}` names an abbreviation that is not in the library.
    SnippetNotFound,
    /// `E_SNIPPET_CYCLE`: a snippet includes itself, directly or indirectly.
    SnippetCycle,
    /// `E_FIELD_CONFLICT`: one field name defined with two types or option lists.
    FieldConflict,
    /// `E_FIELD_UNDEFINED`: `{if}` with `is` or `not` on a field that has no `{field}`.
    FieldUndefined,
    /// `E_FORM_TOO_LARGE`: the resolved form has more than 100 fields.
    FormTooLarge,
    /// `E_DATE_RANGE`: a shifted date falls outside the years 1 to 9999.
    DateRange,
    /// `E_CURSOR_BEFORE_KEY`: a key action follows the cursor in the output.
    CursorBeforeKey,
    /// `E_WAIT_TOTAL`: waits that run total more than 30000 ms.
    WaitTotal,
    /// `E_OUTPUT_TOO_LARGE`: the typed text exceeds 100,000 characters (or a safety bound).
    OutputTooLarge,
}

impl ErrorKind {
    /// Every error kind, in the order of the spec's table.
    pub const ALL: [ErrorKind; 19] = [
        Self::UnclosedMacro,
        Self::BadSyntax,
        Self::UnknownAttr,
        Self::DuplicateAttr,
        Self::MissingAttr,
        Self::BadValue,
        Self::BadFormat,
        Self::BadShift,
        Self::UnbalancedIf,
        Self::TooDeep,
        Self::SnippetNotFound,
        Self::SnippetCycle,
        Self::FieldConflict,
        Self::FieldUndefined,
        Self::FormTooLarge,
        Self::DateRange,
        Self::CursorBeforeKey,
        Self::WaitTotal,
        Self::OutputTooLarge,
    ];

    /// The stable code, e.g. `"E_BAD_FORMAT"`.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::UnclosedMacro => "E_UNCLOSED_MACRO",
            Self::BadSyntax => "E_BAD_SYNTAX",
            Self::UnknownAttr => "E_UNKNOWN_ATTR",
            Self::DuplicateAttr => "E_DUPLICATE_ATTR",
            Self::MissingAttr => "E_MISSING_ATTR",
            Self::BadValue => "E_BAD_VALUE",
            Self::BadFormat => "E_BAD_FORMAT",
            Self::BadShift => "E_BAD_SHIFT",
            Self::UnbalancedIf => "E_UNBALANCED_IF",
            Self::TooDeep => "E_TOO_DEEP",
            Self::SnippetNotFound => "E_SNIPPET_NOT_FOUND",
            Self::SnippetCycle => "E_SNIPPET_CYCLE",
            Self::FieldConflict => "E_FIELD_CONFLICT",
            Self::FieldUndefined => "E_FIELD_UNDEFINED",
            Self::FormTooLarge => "E_FORM_TOO_LARGE",
            Self::DateRange => "E_DATE_RANGE",
            Self::CursorBeforeKey => "E_CURSOR_BEFORE_KEY",
            Self::WaitTotal => "E_WAIT_TOTAL",
            Self::OutputTooLarge => "E_OUTPUT_TOO_LARGE",
        }
    }

    /// Looks a kind up by its code.
    #[must_use]
    pub fn from_code(code: &str) -> Option<ErrorKind> {
        Self::ALL.into_iter().find(|k| k.code() == code)
    }

    /// True for the errors that only rendering can find (they depend on the clock, the form
    /// values or the size of the output).
    #[must_use]
    pub fn is_render_time(self) -> bool {
        matches!(
            self,
            Self::DateRange | Self::CursorBeforeKey | Self::WaitTotal | Self::OutputTooLarge
        )
    }

    fn describe(self) -> &'static str {
        match self {
            Self::UnclosedMacro => "a macro or a quoted value has no end",
            Self::BadSyntax => "malformed attribute",
            Self::UnknownAttr => "attribute not accepted here",
            Self::DuplicateAttr => "attribute given twice",
            Self::MissingAttr => "required attribute missing",
            Self::BadValue => "value empty or outside the allowed set or range",
            Self::BadFormat => "unknown date or time specifier",
            Self::BadShift => "invalid shift term",
            Self::UnbalancedIf => "if, else and endif do not pair up",
            Self::TooDeep => "more than 8 levels of if or of nested snippets",
            Self::SnippetNotFound => "no snippet with that abbreviation",
            Self::SnippetCycle => "a snippet includes itself",
            Self::FieldConflict => "one field name with two types or option lists",
            Self::FieldUndefined => "condition on a field that is not defined",
            Self::FormTooLarge => "the form has more than 100 fields",
            Self::DateRange => "date outside the years 1 to 9999",
            Self::CursorBeforeKey => "a key press follows the cursor",
            Self::WaitTotal => "waits total more than 30000 ms",
            Self::OutputTooLarge => "output too large",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// A template error: the code, and where it is.
///
/// It never contains field values, clipboard content or other user-entered text. The nested
/// snippet's abbreviation is library content, like the snippet name in a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError {
    kind: ErrorKind,
    position: Option<usize>,
    snippet: Option<String>,
}

impl TemplateError {
    /// An error with no position.
    #[must_use]
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            position: None,
            snippet: None,
        }
    }

    pub(crate) fn at(kind: ErrorKind, position: usize) -> Self {
        Self {
            kind,
            position: Some(position),
            snippet: None,
        }
    }

    /// Marks the error as found in the body of the nested snippet `abbreviation`, unless it
    /// already names a (deeper) snippet.
    pub(crate) fn in_snippet(mut self, abbreviation: Option<&str>) -> Self {
        if self.snippet.is_none() {
            self.snippet = abbreviation.map(str::to_owned);
        }
        self
    }

    /// The error kind.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The stable code, e.g. `"E_BAD_FORMAT"`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }

    /// Offset in `char`s into the source of the template that contains the mistake (see
    /// [`snippet`](Self::snippet)), before line-ending normalisation. Set for every parse and
    /// resolve error; `None` for render errors.
    ///
    /// Parse errors point at the offending character (`E_BAD_SYNTAX`), the attribute name
    /// (`E_UNKNOWN_ATTR`, `E_DUPLICATE_ATTR`), the start of the value (`E_BAD_VALUE`,
    /// `E_BAD_FORMAT`, `E_BAD_SHIFT`, and `E_UNCLOSED_MACRO` for a quoted value), or the `{` of
    /// the macro (everything else). Resolve errors point at the `{snippet}`, `{field}` or `{if}`
    /// (`E_FORM_TOO_LARGE`: the first `{field}` or `{if}` naming the 101st field).
    #[must_use]
    pub fn position(&self) -> Option<usize> {
        self.position
    }

    /// The abbreviation of the nested snippet whose body contains the mistake; `None` when it is
    /// in the template itself (or for render errors).
    #[must_use]
    pub fn snippet(&self) -> Option<&str> {
        self.snippet.as_deref()
    }
}

impl From<ErrorKind> for TemplateError {
    fn from(kind: ErrorKind) -> Self {
        Self::new(kind)
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.code(), self.kind.describe())?;
        if let Some(p) = self.position {
            write!(f, " at character {p}")?;
        }
        if let Some(s) = &self.snippet {
            write!(f, " in nested snippet {s:?}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TemplateError {}
