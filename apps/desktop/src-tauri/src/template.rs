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
//! | `template_preview` | `RenderPreview` (with `rich` for a formatted snippet) | `Preview` |
//! | `template_add_attachment` | `PutAttachment` | `Attachment` |
//!
//! # Formatted snippets (task 3.8)
//!
//! The preview of a formatted snippet is the service's sanitised HTML (`iamb_core::rich`, UX
//! specification §9 rule 8). Its images reference attachments as `attachment:<sha256>`; here
//! each is replaced with the image's bytes as a `data:` URI, read from the path the service
//! named (the UI loads no remote content, and the web view's policy allows `data:` images
//! only). An image that is not on this device, or not an image, shows as its alt text.
//! `template_add_attachment` takes the bytes of an image the user picked, pasted or dropped, writes
//! them to a private temporary file (bulk data travels as a path, architecture §6.1), asks the
//! service to keep them, removes the file and answers the Markdown that shows the image.
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

use std::path::Path;

use iamb_core::model::Id;
use iamb_ipc::{
    PreviewBlank, PreviewCursor, PreviewPart, RequestBody, ResponseBody, Secret, TemplateErrorInfo,
    TemplateToken, WireAttachment,
};
use serde::{Deserialize, Serialize};

use crate::bridge::{Bridge, command};
use crate::contract::{CommandError, FieldView, Unexpected};

/// The requests the template functions send, by wire name.
pub const REQUESTS: &[&str] = &["parse_template", "render_preview", "put_attachment"];

/// The largest image the preview embeds, in bytes (the service accepts images up to 5 MiB).
pub const PREVIEW_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

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
    /// A run of a formatted snippet, as it would be pasted: sanitised HTML with its images
    /// embedded (module docs), and the plain-text version.
    Formatted {
        /// The HTML.
        html: String,
        /// The plain text.
        plain: String,
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
            PreviewPart::Formatted {
                html,
                plain,
                attachments,
            } => Self::Formatted {
                html: embed_images(html, attachments),
                plain: plain.clone(),
            },
        }
    }
}

/// The media type of an image's first bytes, for the image types the service accepts.
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The bytes of an attachment's file as a `data:` URI, when it is an image of a size the
/// preview shows.
fn data_uri(path: &Path) -> Option<String> {
    use base64::Engine as _;
    if std::fs::metadata(path).ok()?.len() > PREVIEW_IMAGE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let mime = image_type(&bytes)?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// The service's HTML with each `<img src="attachment:<sha256>" alt="…">` (the form the core
/// writes) turned into an embedded image, or into its alt text when the image cannot be shown.
#[must_use]
pub fn embed_images(html: &str, attachments: &[WireAttachment]) -> String {
    const START: &str = "<img src=\"attachment:";
    const ALT: &str = "\" alt=\"";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(START) {
        out.push_str(&rest[..at]);
        let tail = &rest[at + START.len()..];
        let parsed = tail.get(..64).and_then(|hex| {
            let after = tail.get(64..)?.strip_prefix(ALT)?;
            let end = after.find("\">")?;
            Some((hex, &after[..end], &after[end + 2..]))
        });
        let Some((hex, alt, next)) = parsed else {
            out.push_str(START);
            rest = tail;
            continue;
        };
        let uri = attachments
            .iter()
            .find(|a| a.sha256.to_string() == hex)
            .and_then(|a| a.path.as_deref())
            .and_then(data_uri);
        match uri {
            Some(uri) => {
                out.push_str("<img src=\"");
                out.push_str(&uri);
                out.push_str(ALT);
                out.push_str(alt);
                out.push_str("\">");
            }
            None => out.push_str(alt),
        }
        rest = next;
    }
    out.push_str(rest);
    out
}

/// What `template_add_attachment` answers: the attachment and the Markdown that shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentView {
    /// The hash that names it.
    pub sha256: String,
    /// Its media type.
    pub mime: String,
    /// Its size in bytes.
    pub size: u64,
    /// The name kept with it.
    pub file_name: String,
    /// `![alt](attachment:<sha256>)`: the alt text is the file name without its extension for an
    /// image, the whole file name for another file.
    pub markdown: String,
}

/// The answer of `template_add_attachment` from the service's attachment.
#[must_use]
pub fn attachment_view(a: &WireAttachment) -> AttachmentView {
    let alt = if a.mime.starts_with("image/") {
        Path::new(&a.file_name)
            .file_stem()
            .map_or_else(|| a.file_name.clone(), |s| s.to_string_lossy().into_owned())
    } else {
        a.file_name.clone()
    };
    AttachmentView {
        sha256: a.sha256.to_string(),
        mime: a.mime.clone(),
        size: a.size,
        file_name: a.file_name.clone(),
        markdown: iamb_core::rich::image_markdown(&alt, &a.sha256),
    }
}

/// Decodes `%XX` escapes (what JavaScript's `encodeURIComponent` writes); a malformed escape
/// stays as written, and bytes that are not UTF-8 become replacement characters.
#[must_use]
pub fn decode_uri_component(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `PutAttachment`.
#[must_use]
pub fn attachment_request(path: std::path::PathBuf, file_name: String, folder: Id) -> RequestBody {
    RequestBody::PutAttachment {
        path,
        file_name,
        folder,
    }
}

/// The attachment in an `Attachment` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn attachment_from(response: ResponseBody) -> Result<AttachmentView, Unexpected> {
    match response {
        ResponseBody::Attachment { attachment } => Ok(attachment_view(&attachment)),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// Keeps `bytes` as an attachment of the profile of `folder` (module docs). The temporary file
/// is created for this call only (owner-only on Unix) and removed whatever the answer.
///
/// # Errors
///
/// `invalid` when the bytes are refused or the temporary file cannot be written; the call's
/// errors otherwise.
pub fn add_attachment(
    bridge: &Bridge,
    folder: Id,
    file_name: &str,
    bytes: &[u8],
) -> Result<AttachmentView, CommandError> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("iamb-attachment-{}-{n}", std::process::id()));
    let written = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&path);
        return Err(CommandError::new(
            "invalid",
            "the image could not be prepared",
        ));
    }
    let answer = bridge.call(attachment_request(
        path.clone(),
        file_name.to_owned(),
        folder,
    ));
    let _ = std::fs::remove_file(&path);
    Ok(attachment_from(answer.map_err(command)?)?)
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

/// `RenderPreview`, with the test values; `rich` for a formatted snippet.
#[must_use]
pub fn preview_request(
    source: String,
    values: BTreeMap<String, String>,
    rich: bool,
) -> RequestBody {
    RequestBody::RenderPreview {
        source,
        values: Secret::new(values),
        rich,
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
    preview_as(bridge, source, values, false)
}

/// As [`preview`], as a formatted snippet when `rich` (task 3.8).
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn preview_as(
    bridge: &Bridge,
    source: String,
    values: BTreeMap<String, String>,
    rich: bool,
) -> Result<PreviewView, CommandError> {
    Ok(preview_from(
        bridge
            .call(preview_request(source, values, rich))
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
        let RequestBody::RenderPreview {
            source,
            values,
            rich,
        } = preview_request(
            "b".to_owned(),
            BTreeMap::from([("X".into(), "1".into())]),
            true,
        )
        else {
            panic!("a preview request");
        };
        assert_eq!(source, "b");
        assert!(rich);
        assert_eq!(values.expose().get("X").map(String::as_str), Some("1"));
        for body in [
            parse_request(String::new()),
            preview_request(String::new(), BTreeMap::new(), false),
            attachment_request("/tmp/x".into(), "x.png".to_owned(), Id::from_bytes([1; 16])),
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
