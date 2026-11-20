//! The template editor's side of the contract (task 3.4): parsing a template into the tokens
//! the editor draws as text and chips, and rendering its preview.
//!
//! The front end never speaks IPC and never parses or renders a template itself (architecture
//! §6.2, ADR-0017): it calls the `template_*` commands in `app/template_commands.rs`, which call
//! the functions here; the types are mirrored by hand in
//! `apps/desktop/src/library/template/api.ts`.
//!
//! | Command | Request sent | Response read |
//! | --- | --- | --- |
//! | `template_parse` | `ParseTemplate` | `TemplateTokens` |
//! | `template_preview` | `RenderPreview` | `Preview` |
//!
//! Positions are UTF-16 offsets, as on the wire (`iamb_ipc::messages`, "Positions in text"),
//! which is what JavaScript strings index by.
//!
//! # Privacy
//!
//! The source is library content, or what the user is writing into the library; the preview's
//! values are test values the user typed. Both travel only in the request and its answer: these
//! functions do not log at all, and the bridge logs message names only (architecture §8 rule 3).
//! The preview never reads the clipboard; the service shows the sample "Clipboard".

use std::collections::BTreeMap;

use iamb_ipc::{
    PreviewBlank, PreviewCursor, PreviewPart, RequestBody, ResponseBody, Secret, TemplateErrorInfo,
    TemplateToken,
};
use serde::{Deserialize, Serialize};

use crate::bridge::{Bridge, command};
use crate::contract::{CommandError, FieldView, Unexpected};

/// The requests the template functions send, by wire name.
pub const REQUESTS: &[&str] = &["parse_template", "render_preview"];

/// One attribute of a macro, as written (quotes removed, escapes resolved).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeView {
    /// The attribute's name (`fmt`, `name`, …).
    pub name: String,
    /// Its value.
    pub value: String,
}

/// One token: literal text, or one macro with its attributes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenView {
    /// `text`, or the macro's name (`date`, `field`, `lbrace`, …).
    pub kind: String,
    /// UTF-16 offset of the start in the source.
    pub start: usize,
    /// UTF-16 offset after the end.
    pub end: usize,
    /// A macro's attributes, in source order; empty for text.
    pub attributes: Vec<AttributeView>,
}

impl From<&TemplateToken> for TokenView {
    fn from(t: &TemplateToken) -> Self {
        Self {
            kind: t.kind.clone(),
            start: t.start,
            end: t.end,
            attributes: t
                .attributes
                .iter()
                .map(|a| AttributeView {
                    name: a.name.clone(),
                    value: a.value.clone(),
                })
                .collect(),
        }
    }
}

/// A template error: its code and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateErrorView {
    /// The error code (`E_…`).
    pub code: String,
    /// UTF-16 offset of the start of the mistake in the source; `None` for errors found only by
    /// rendering and for errors inside an included snippet.
    pub start: Option<usize>,
    /// UTF-16 offset after the mistake.
    pub end: Option<usize>,
    /// The abbreviation of the included snippet whose body holds the mistake.
    pub snippet: Option<String>,
}

impl From<&TemplateErrorInfo> for TemplateErrorView {
    fn from(e: &TemplateErrorInfo) -> Self {
        Self {
            code: e.code.clone(),
            start: e.start,
            end: e.end,
            snippet: e.snippet.clone(),
        }
    }
}

/// The answer of `template_parse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedView {
    /// The tokens, covering the source from start to end; empty when the source does not split
    /// into tokens (an unclosed or malformed macro: then `error` says where).
    pub tokens: Vec<TokenView>,
    /// The first parse or resolve error.
    pub error: Option<TemplateErrorView>,
}

/// A range of preview text that a fill-in field produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlankView {
    /// UTF-16 offset of the start in the part's text.
    pub start: usize,
    /// UTF-16 offset after the end.
    pub end: usize,
    /// The field's name.
    pub field: String,
    /// No value was given: the text is the field's name, drawn as a grey name tag.
    pub sample: bool,
}

impl From<&PreviewBlank> for BlankView {
    fn from(b: &PreviewBlank) -> Self {
        Self {
            start: b.start,
            end: b.end,
            field: b.field.clone(),
            sample: b.sample,
        }
    }
}

/// One part of a preview, in output order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum PartView {
    /// Text that would be typed.
    Text {
        /// The text.
        text: String,
        /// The ranges fill-in fields produced.
        blanks: Vec<BlankView>,
        /// The ranges `[start, end)` that came from the clipboard (the sample "Clipboard").
        clipboard: Vec<[usize; 2]>,
    },
    /// A key press.
    Key {
        /// The key's template name (`enter`, `tab`, …).
        key: String,
        /// Shift is held.
        shift: bool,
        /// How many presses.
        times: u8,
    },
    /// A wait.
    Wait {
        /// Milliseconds.
        ms: u32,
    },
}

impl From<&PreviewPart> for PartView {
    fn from(p: &PreviewPart) -> Self {
        match p {
            PreviewPart::Text {
                text,
                blanks,
                clipboard,
            } => Self::Text {
                text: text.clone(),
                blanks: blanks.iter().map(BlankView::from).collect(),
                clipboard: clipboard.clone(),
            },
            PreviewPart::Key { key, shift, times } => Self::Key {
                key: key.clone(),
                shift: *shift,
                times: *times,
            },
            PreviewPart::Wait { ms } => Self::Wait { ms: *ms },
        }
    }
}

/// Where the cursor of a preview is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorView {
    /// The part it is in; the number of parts when no text surrounds it.
    pub part: usize,
    /// The UTF-16 offset in that part's text.
    pub offset: usize,
}

impl From<&PreviewCursor> for CursorView {
    fn from(c: &PreviewCursor) -> Self {
        Self {
            part: c.part,
            offset: c.offset,
        }
    }
}

/// The answer of `template_preview`: empty parts with the error when the template has one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewView {
    /// The parts, in output order.
    pub parts: Vec<PartView>,
    /// Where the cursor would be.
    pub cursor: Option<CursorView>,
    /// The fill-in fields, in form order.
    pub fields: Vec<FieldView>,
    /// The error, if the template has one.
    pub error: Option<TemplateErrorView>,
}

/// `ParseTemplate`.
#[must_use]
pub fn parse_request(source: String) -> RequestBody {
    RequestBody::ParseTemplate { source }
}

/// `RenderPreview`, with the test values.
#[must_use]
pub fn preview_request(source: String, values: BTreeMap<String, String>) -> RequestBody {
    RequestBody::RenderPreview {
        source,
        values: Secret::new(values),
    }
}

/// The tokens and error in a `TemplateTokens` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn parsed_from(response: ResponseBody) -> Result<ParsedView, Unexpected> {
    match response {
        ResponseBody::TemplateTokens { tokens, error } => Ok(ParsedView {
            tokens: tokens.iter().map(TokenView::from).collect(),
            error: error.as_ref().map(TemplateErrorView::from),
        }),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The preview in a `Preview` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn preview_from(response: ResponseBody) -> Result<PreviewView, Unexpected> {
    match response {
        ResponseBody::Preview {
            parts,
            cursor,
            fields,
            error,
        } => Ok(PreviewView {
            parts: parts.iter().map(PartView::from).collect(),
            cursor: cursor.as_ref().map(CursorView::from),
            fields: fields.iter().map(FieldView::from).collect(),
            error: error.as_ref().map(TemplateErrorView::from),
        }),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// Parses `source` in the service.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn parse(bridge: &Bridge, source: String) -> Result<ParsedView, CommandError> {
    Ok(parsed_from(
        bridge.call(parse_request(source)).map_err(command)?,
    )?)
}

/// Renders the preview of `source` with `values` in the service.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn preview(
    bridge: &Bridge,
    source: String,
    values: BTreeMap<String, String>,
) -> Result<PreviewView, CommandError> {
    Ok(preview_from(
        bridge
            .call(preview_request(source, values))
            .map_err(command)?,
    )?)
}

#[cfg(test)]
mod tests {
    use iamb_ipc::TemplateAttribute;

    use super::*;

    #[test]
    fn requests_carry_the_source_and_the_values() {
        assert_eq!(
            parse_request("a".to_owned()).name(),
            "parse_template",
            "the wire name"
        );
        let RequestBody::RenderPreview { source, values } =
            preview_request("b".to_owned(), BTreeMap::from([("X".into(), "1".into())]))
        else {
            panic!("a preview request");
        };
        assert_eq!(source, "b");
        assert_eq!(values.expose().get("X").map(String::as_str), Some("1"));
        for body in [
            parse_request(String::new()),
            preview_request(String::new(), BTreeMap::new()),
        ] {
            assert!(REQUESTS.contains(&body.name()));
        }
    }

    #[test]
    fn payloads_have_stable_shapes() {
        let parsed = parsed_from(ResponseBody::TemplateTokens {
            tokens: vec![
                TemplateToken {
                    kind: "text".to_owned(),
                    start: 0,
                    end: 3,
                    attributes: Vec::new(),
                },
                TemplateToken {
                    kind: "key".to_owned(),
                    start: 3,
                    end: 18,
                    attributes: vec![TemplateAttribute {
                        name: "name".to_owned(),
                        value: "tab".to_owned(),
                    }],
                },
            ],
            error: None,
        })
        .expect("tokens");
        assert_eq!(
            serde_json::to_value(&parsed).expect("json"),
            serde_json::json!({
                "tokens": [
                    {"kind": "text", "start": 0, "end": 3, "attributes": []},
                    {"kind": "key", "start": 3, "end": 18,
                     "attributes": [{"name": "name", "value": "tab"}]},
                ],
                "error": null,
            })
        );
        let preview = preview_from(ResponseBody::Preview {
            parts: vec![PreviewPart::Wait { ms: 5 }],
            cursor: None,
            fields: Vec::new(),
            error: None,
        })
        .expect("preview");
        assert_eq!(
            serde_json::to_value(&preview).expect("json"),
            serde_json::json!({
                "parts": [{"part": "wait", "ms": 5}],
                "cursor": null,
                "fields": [],
                "error": null,
            })
        );
        assert_eq!(
            parsed_from(ResponseBody::Done {}).map_err(|e| e.got),
            Err("done")
        );
        assert_eq!(
            preview_from(ResponseBody::Done {}).map_err(|e| e.got),
            Err("done")
        );
    }
}
