//! Rich snippets in the service (task 3.8, ADR-0018, architecture §9): the clipboard content of
//! a `Paste`, built from the core's sanitised HTML and the attachments on disk.
//!
//! The engine's `Paste` action carries HTML whose images reference attachments as
//! `<img src="attachment:<sha256>" alt="…">` (exactly that form: `iamb_core::rich` writes it).
//! Before the injector runs, [`prepare_steps`] reads each attachment (checked against its hash,
//! `iamb_store::attachments::read_verified`) and builds what the clipboard carries:
//!
//! | Content | Clipboard |
//! | --- | --- |
//! | Text and formatting | the HTML as the core wrote it, and the plain-text version |
//! | An image (PNG, JPEG, GIF, WebP) | embedded in the HTML as a `data:` URI |
//! | A media file (PDF, MP4, MP3, WAV) among other content | its name (the alt text) in the HTML |
//! | A media file that is all the content | its name in the HTML and the file itself, offered as a file list (a copy named after the alt text in `run/paste/<sha256>/`, which the worker empties at start) |
//!
//! Each OS crate writes these flavours in its own formats (Windows: `CF_UNICODETEXT`, the HTML
//! format with its header of byte offsets, `CF_HDROP`; macOS: `public.utf8-plain-text`,
//! `public.html`, file URLs), marks them for clipboard history and puts the user's clipboard
//! back afterwards (architecture §8 rule 8).
//!
//! An attachment that is missing or damaged turns that `Paste` into typing its plain text
//! (`TypeText`), and the worker records the diagnostic `attachment_missing` for the snippet.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use base64::Engine as _;
use iamb_core::model::{Id, Sha256};
use iamb_ipc::WireAttachment;
use iamb_platform::{ClipboardContent, InjectionStep};
use iamb_store::attachments;

/// The start of an attachment image in the core's HTML.
const IMG_START: &str = "<img src=\"attachment:";
/// Between the hash and the alt text.
const IMG_ALT: &str = "\" alt=\"";
/// The end of the tag.
const IMG_END: &str = "\">";

/// Makes the copy of a lone media file to offer (its hash, the file name, the bytes) and gives
/// its path.
pub type Offer<'a> = dyn FnMut(&Sha256, &str, &[u8]) -> Option<PathBuf> + 'a;

/// The directory under `run/` that holds the copies of media files offered as files.
pub const PASTE_DIR: &str = "paste";

/// One attachment image of the core's HTML.
struct ImageTag<'a> {
    /// Byte range of the whole tag in the HTML.
    range: std::ops::Range<usize>,
    sha: Sha256,
    /// The alt text, HTML-escaped as the core wrote it.
    alt: &'a str,
}

fn image_tags(html: &str) -> Vec<ImageTag<'_>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = html[from..].find(IMG_START) {
        let start = from + found;
        let after = start + IMG_START.len();
        let parsed = (|| {
            let hex = html.get(after..after + 64)?;
            let sha = Sha256::from_str(hex).ok()?;
            let rest = html.get(after + 64..)?.strip_prefix(IMG_ALT)?;
            let alt_len = rest.find(IMG_END)?;
            let alt = &rest[..alt_len];
            let end = after + 64 + IMG_ALT.len() + alt_len + IMG_END.len();
            Some(ImageTag {
                range: start..end,
                sha,
                alt,
            })
        })();
        match parsed {
            Some(tag) => {
                from = tag.range.end;
                out.push(tag);
            }
            None => from = after,
        }
    }
    out
}

fn unescape_html(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The clipboard content for a `Paste` whose HTML may reference attachments (module docs).
/// `read` gives an attachment's checked bytes; `offer` makes the copy of a lone media file and
/// gives its path.
///
/// # Errors
///
/// The first attachment `read` cannot give.
pub fn resolve_content(
    content: &ClipboardContent,
    read: &mut dyn FnMut(&Sha256) -> Option<Vec<u8>>,
    offer: &mut Offer<'_>,
) -> Result<ClipboardContent, Sha256> {
    let Some(html) = &content.html else {
        return Ok(content.clone());
    };
    let tags = image_tags(html);
    if tags.is_empty() {
        return Ok(content.clone());
    }
    let lone = tags.len() == 1
        && html
            .strip_prefix("<p>")
            .and_then(|r| r.strip_suffix("</p>"))
            == Some(&html[tags[0].range.clone()]);
    let mut out = String::with_capacity(html.len());
    let mut files = content.files.clone();
    let mut at = 0;
    for tag in &tags {
        let bytes = read(&tag.sha).ok_or(tag.sha)?;
        out.push_str(&html[at..tag.range.start]);
        match attachments::sniff(&bytes) {
            Some(mime) if attachments::is_image(mime) => {
                out.push_str("<img src=\"data:");
                out.push_str(mime);
                out.push_str(";base64,");
                out.push_str(&base64::engine::general_purpose::STANDARD.encode(&bytes));
                out.push_str(IMG_ALT);
                out.push_str(tag.alt);
                out.push_str(IMG_END);
            }
            mime => {
                out.push_str(tag.alt);
                if lone {
                    let fallback = match mime {
                        Some("application/pdf") => "document.pdf",
                        Some("video/mp4") => "video.mp4",
                        Some("audio/mpeg") => "audio.mp3",
                        Some("audio/wav") => "audio.wav",
                        _ => "attachment",
                    };
                    let name = attachments::clean_file_name(&unescape_html(tag.alt), fallback);
                    if let Some(path) = offer(&tag.sha, &name, &bytes) {
                        files.push(path);
                    }
                }
            }
        }
        at = tag.range.end;
    }
    out.push_str(&html[at..]);
    Ok(ClipboardContent {
        text: content.text.clone(),
        html: Some(out),
        files,
    })
}

/// Replaces every `Paste` step's content with the resolved one (module docs), reading the
/// attachments of `profile` under the data root `root`. A `Paste` whose attachment cannot be
/// read becomes `TypeText` of its plain text. Returns the attachments that were missing.
pub fn prepare_steps(steps: &mut [InjectionStep], root: &Path, profile: Id) -> Vec<Sha256> {
    let mut missing = Vec::new();
    for step in steps.iter_mut() {
        let InjectionStep::Paste(content) = step else {
            continue;
        };
        let mut read = |sha: &Sha256| {
            attachments::read_verified(root, profile, sha)
                .ok()
                .flatten()
        };
        let mut offer = |sha: &Sha256, name: &str, bytes: &[u8]| offer_file(root, sha, name, bytes);
        match resolve_content(content, &mut read, &mut offer) {
            Ok(resolved) => *content = resolved,
            Err(sha) => {
                missing.push(sha);
                *step = InjectionStep::TypeText(content.text.clone().unwrap_or_default());
            }
        }
    }
    missing
}

/// The copy of a lone media file to offer as a file: `run/paste/<sha256>/<name>`.
fn offer_file(root: &Path, sha: &Sha256, name: &str, bytes: &[u8]) -> Option<PathBuf> {
    let dir = root.join("run").join(PASTE_DIR).join(sha.to_string());
    let path = dir.join(name);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
        return Some(path);
    }
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// Removes the copies of earlier runs (best effort): the worker calls it at start.
pub fn clear_offered_files(root: &Path) {
    let _ = std::fs::remove_dir_all(root.join("run").join(PASTE_DIR));
}

/// An attachment of `profile` for the editor's preview: its media type and size read from the
/// file, and its path when the file is there. The preview only shows the image, so the hash is
/// not checked here (an expansion checks it); the UI host reads the file from the path.
#[must_use]
pub fn attachment_info(root: &Path, profile: Id, sha: &Sha256) -> WireAttachment {
    let path = attachments::path_in(root, profile, sha);
    let head = std::fs::File::open(&path).ok().and_then(|mut f| {
        use std::io::Read;
        let mut buf = [0u8; 16];
        let n = f.read(&mut buf).ok()?;
        let size = f.metadata().ok()?.len();
        Some((buf, n, size))
    });
    match head {
        Some((buf, n, size)) => WireAttachment {
            sha256: *sha,
            mime: attachments::sniff(&buf[..n]).unwrap_or_default().to_owned(),
            size,
            file_name: String::new(),
            path: Some(path),
        },
        None => WireAttachment {
            sha256: *sha,
            mime: String::new(),
            size: 0,
            file_name: String::new(),
            path: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend_from_slice(&[7; 8]);
        b
    }

    fn sha_of(bytes: &[u8]) -> Sha256 {
        attachments::sha256_of(bytes)
    }

    #[test]
    fn images_become_data_uris() {
        let bytes = png();
        let sha = sha_of(&bytes);
        let html = format!("<p>Hi <img src=\"attachment:{sha}\" alt=\"Logo &amp; co\"></p>");
        let content = ClipboardContent::rich("Hi Logo & co", html);
        let out = resolve_content(&content, &mut |_| Some(bytes.clone()), &mut |_, _, _| None)
            .expect("resolves");
        let html = out.html.expect("html");
        assert!(html.starts_with("<p>Hi <img src=\"data:image/png;base64,iVBORw0KGgo"));
        assert!(html.ends_with("\" alt=\"Logo &amp; co\"></p>"));
        assert!(!html.contains("attachment:"));
        assert_eq!(out.text.as_deref(), Some("Hi Logo & co"));
        assert!(
            out.files.is_empty(),
            "a lone image is not offered as a file"
        );
    }

    #[test]
    fn a_lone_media_file_is_also_a_file() {
        let pdf = b"%PDF-1.7 tiny".to_vec();
        let sha = sha_of(&pdf);
        let html = format!("<p><img src=\"attachment:{sha}\" alt=\"Price list.pdf\"></p>");
        let content = ClipboardContent::rich("Price list.pdf", html);
        let mut offered = Vec::new();
        let out = resolve_content(&content, &mut |_| Some(pdf.clone()), &mut |s, name, _| {
            offered.push((*s, name.to_owned()));
            Some(PathBuf::from(format!("/run/paste/{s}/{name}")))
        })
        .expect("resolves");
        assert_eq!(out.html.as_deref(), Some("<p>Price list.pdf</p>"));
        assert_eq!(offered, [(sha, "Price list.pdf".to_owned())]);
        assert_eq!(out.files.len(), 1);

        // Among other content it is only named.
        let html = format!("<p>See <img src=\"attachment:{sha}\" alt=\"list.pdf\"></p>");
        let content = ClipboardContent::rich("See list.pdf", html);
        let out = resolve_content(&content, &mut |_| Some(pdf.clone()), &mut |_, _, _| {
            panic!("not offered")
        })
        .expect("resolves");
        assert_eq!(out.html.as_deref(), Some("<p>See list.pdf</p>"));
        assert!(out.files.is_empty());
    }

    #[test]
    fn a_missing_attachment_types_the_plain_text() {
        let dir = tempfile::tempdir().expect("dir");
        let profile = Id::from_bytes([3; 16]);
        let sha = sha_of(&png());
        let html = format!("<p><strong>Hi</strong> <img src=\"attachment:{sha}\" alt=\"x\"></p>");
        let mut steps = vec![
            InjectionStep::Paste(ClipboardContent::rich("Hi x", html)),
            InjectionStep::Paste(ClipboardContent::rich("plain", "<p>plain</p>")),
        ];
        let missing = prepare_steps(&mut steps, dir.path(), profile);
        assert_eq!(missing, [sha]);
        assert_eq!(steps[0], InjectionStep::TypeText("Hi x".to_owned()));
        assert_eq!(
            steps[1],
            InjectionStep::Paste(ClipboardContent::rich("plain", "<p>plain</p>"))
        );

        // With the bytes in place (and only when they match the hash) it pastes.
        let path = attachments::path_in(dir.path(), profile, &sha);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nother").expect("write");
        let html = format!("<p><img src=\"attachment:{sha}\" alt=\"x\"></p>");
        let mut steps = vec![InjectionStep::Paste(ClipboardContent::rich(
            "x",
            html.clone(),
        ))];
        assert_eq!(prepare_steps(&mut steps, dir.path(), profile), [sha]);
        std::fs::write(&path, png()).expect("write");
        let mut steps = vec![InjectionStep::Paste(ClipboardContent::rich("x", html))];
        assert!(prepare_steps(&mut steps, dir.path(), profile).is_empty());
        let InjectionStep::Paste(content) = &steps[0] else {
            panic!("still a paste");
        };
        assert!(
            content
                .html
                .as_deref()
                .is_some_and(|h| h.contains("data:image/png"))
        );
    }

    #[test]
    fn malformed_references_are_left_alone() {
        let html = "<p>&lt;img src=\"attachment:zz\" alt=\"x\"&gt;</p>";
        let content = ClipboardContent::rich("t", html);
        let out = resolve_content(&content, &mut |_| None, &mut |_, _, _| None).expect("nothing");
        assert_eq!(out, content);
    }
}
