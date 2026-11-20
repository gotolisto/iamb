//! Attachments: the images and media of rich snippets (task 3.8, ADR-0018, architecture §3.3,
//! §7, §9).
//!
//! # Model
//!
//! - **Content-addressed.** An attachment is named by the SHA-256 of its bytes. The bytes live
//!   in `attachments/<profile id>/<sha256>` under the data directory (§3.3), one file per
//!   profile and content, written once (temporary file, flush, rename) with mode `0600` in
//!   directories of mode `0700` on Unix. A rich snippet's Markdown names an attachment as
//!   `attachment:<sha256>` (`iamb_core::rich`), so a body stays valid wherever the bytes are.
//! - **Metadata** is the synced `attachment` row (profile, hash, media type, size, original file
//!   name); adding the same content to a profile twice gives back the row that exists.
//!   The `snippet_attachment` rows link each rich snippet to the attachments its Markdown
//!   references ([`WriteTx::sync_snippet_attachments`]), so sync (task 4.6) knows what a
//!   snippet needs. The tables are in the first migration; no unique index is added, because a
//!   remote apply may briefly hold two rows for one content (rows sync one by one) and the
//!   store dedupes on write instead.
//! - **Types and sizes.** The type is read from the bytes, never from the file name:
//!
//!   | Kind | Types | At most |
//!   | --- | --- | --- |
//!   | Image (pasted with the HTML, embedded as a data URI) | PNG, JPEG, GIF, WebP | [`MAX_IMAGE_BYTES`] |
//!   | Media (offered as a file when it is all a snippet holds) | PDF, MP4, MP3, WAV | [`MAX_MEDIA_BYTES`] |
//!
//!   The limits keep a pasted email under the common 25 MB message limits once the image is
//!   base64 in the HTML (a third larger), and the media limit is that message limit itself.
//!   Task 4.6 sets what sync accepts and may lower them.
//! - **Use.** [`Store::read_attachment`] checks the bytes against their hash before handing
//!   them out (§8 rule 10): a missing or damaged file reads as missing, and the expansion falls
//!   back to plain text.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use iamb_core::model::{
    Attachment, Id, Sha256, Snippet, SnippetAttachment, SnippetKind, SyncMeta, Timestamp,
};
use sha2::Digest;

use crate::{ListOptions, Store, StoreError, WriteTx, create_private_dirs};

/// The directory under the data directory that holds attachment bytes.
pub const ATTACHMENTS_DIR: &str = "attachments";

/// The largest image, in bytes.
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// The largest media file, in bytes.
pub const MAX_MEDIA_BYTES: u64 = 25 * 1024 * 1024;

/// The longest original file name kept, in characters.
pub const MAX_FILE_NAME_CHARS: usize = 255;

/// Why bytes cannot become an attachment.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AttachmentError {
    /// The bytes are not one of the accepted types (or are empty).
    #[error("only PNG, JPEG, GIF and WebP images and PDF, MP4, MP3 and WAV files can be attached")]
    UnsupportedType,
    /// The bytes are larger than the limit for their kind.
    #[error("the file has {size} bytes; at most {limit} are allowed for its type")]
    TooLarge {
        /// The size of the bytes.
        size: u64,
        /// The limit for their kind.
        limit: u64,
    },
    /// The store has no data directory (an in-memory store) to keep the bytes in.
    #[error("this store has no data directory for attachments")]
    NoDataDirectory,
    /// Bytes do not match the hash they were given with (an import).
    #[error("the bytes of attachment {0} do not match its hash")]
    HashMismatch(Sha256),
}

/// The media type of `bytes`, read from their first bytes, when it is an accepted type.
#[must_use]
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    if starts(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if starts(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && starts(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.len() >= 12 && starts(b"RIFF") && &bytes[8..12] == b"WAVE" {
        Some("audio/wav")
    } else if starts(b"%PDF-") {
        Some("application/pdf")
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        Some("video/mp4")
    } else if starts(b"ID3") || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0) {
        Some("audio/mpeg")
    } else {
        None
    }
}

/// Whether attachments of this media type are images (embedded in the HTML when pasted).
#[must_use]
pub fn is_image(mime: &str) -> bool {
    mime.starts_with("image/")
}

/// The size limit for a media type.
#[must_use]
pub fn limit_for(mime: &str) -> u64 {
    if is_image(mime) {
        MAX_IMAGE_BYTES
    } else {
        MAX_MEDIA_BYTES
    }
}

/// The SHA-256 of `bytes`.
#[must_use]
pub fn sha256_of(bytes: &[u8]) -> Sha256 {
    let digest = sha2::Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Sha256::from_bytes(out)
}

/// Checks `bytes` and gives their media type.
///
/// # Errors
///
/// [`AttachmentError::UnsupportedType`] or [`AttachmentError::TooLarge`].
pub fn check(bytes: &[u8]) -> Result<&'static str, AttachmentError> {
    let mime = sniff(bytes).ok_or(AttachmentError::UnsupportedType)?;
    let size = bytes.len() as u64;
    let limit = limit_for(mime);
    if size > limit {
        return Err(AttachmentError::TooLarge { size, limit });
    }
    Ok(mime)
}

/// The file name kept for an attachment: the last path component, trimmed, without control
/// characters, at most [`MAX_FILE_NAME_CHARS`] characters; `fallback` when nothing is left.
#[must_use]
pub fn clean_file_name(name: &str, fallback: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_FILE_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        fallback.to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// Where the bytes of `sha` for `profile` live under `data_dir`.
#[must_use]
pub fn path_in(data_dir: &Path, profile: Id, sha: &Sha256) -> PathBuf {
    data_dir
        .join(ATTACHMENTS_DIR)
        .join(profile.to_string())
        .join(sha.to_string())
}

/// The bytes of `sha` for `profile` under `data_dir`, checked against the hash: `None` when the
/// file is missing or its content does not match. For readers without a [`Store`] (the
/// service's engine thread builds the clipboard content with it).
///
/// # Errors
///
/// I/O errors other than a missing file.
pub fn read_verified(
    data_dir: &Path,
    profile: Id,
    sha: &Sha256,
) -> std::io::Result<Option<Vec<u8>>> {
    match fs::read(path_in(data_dir, profile, sha)) {
        Ok(bytes) if sha256_of(&bytes) == *sha => Ok(Some(bytes)),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes `bytes` (whose hash is `sha`) to their place unless a file with the right content is
/// there already.
pub(crate) fn write_file(
    data_dir: &Path,
    profile: Id,
    sha: &Sha256,
    bytes: &[u8],
) -> Result<PathBuf, StoreError> {
    let path = path_in(data_dir, profile, sha);
    if let Ok(existing) = fs::read(&path)
        && sha256_of(&existing) == *sha
    {
        return Ok(path);
    }
    let dir = path.parent().unwrap_or(data_dir);
    create_private_dirs(dir)?;
    let tmp = dir.join(format!("{sha}.{}.tmp", std::process::id()));
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(path)
}

impl Store {
    /// The data directory, or `None` for an in-memory store.
    #[must_use]
    pub fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    /// Where the bytes of `sha` for `profile` live; `None` for an in-memory store. The file may
    /// not exist (an attachment not downloaded yet).
    #[must_use]
    pub fn attachment_path(&self, profile: Id, sha: &Sha256) -> Option<PathBuf> {
        self.data_dir().map(|d| path_in(d, profile, sha))
    }

    /// Adds `bytes` as an attachment of `profile`: checks the type and size, writes the bytes
    /// (module docs) and the metadata row (with id `id`), or gives back the live row that holds
    /// the same content already.
    ///
    /// # Errors
    ///
    /// [`StoreError::Attachment`] for bytes that are refused or an in-memory store;
    /// [`StoreError::NotFound`] when the profile does not exist; I/O and `SQLite` errors.
    pub fn add_attachment(
        &mut self,
        id: Id,
        profile: Id,
        bytes: &[u8],
        file_name: &str,
        now: Timestamp,
    ) -> Result<Attachment, StoreError> {
        let mime = check(bytes)?;
        let data_dir = self
            .data_dir
            .clone()
            .ok_or(AttachmentError::NoDataDirectory)?;
        let sha = sha256_of(bytes);
        write_file(&data_dir, profile, &sha, bytes)?;
        let file_name = clean_file_name(file_name, default_name(mime));
        let size = bytes.len() as u64;
        self.write(|w| w.ensure_attachment(id, profile, sha, mime, size, &file_name, now))
    }

    /// The live attachment row of `profile` for `sha`, if any.
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn find_attachment(
        &self,
        profile: Id,
        sha: &Sha256,
    ) -> Result<Option<Attachment>, StoreError> {
        Ok(self
            .list::<Attachment>(ListOptions::LIVE)?
            .into_iter()
            .find(|a| a.profile_id == profile && a.sha256 == *sha))
    }

    /// The bytes of `sha` for `profile`, checked against the hash: `None` when the file is
    /// missing or its content does not match (module docs).
    ///
    /// # Errors
    ///
    /// I/O errors other than a missing file.
    pub fn read_attachment(
        &self,
        profile: Id,
        sha: &Sha256,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let Some(dir) = self.data_dir() else {
            return Ok(None);
        };
        Ok(read_verified(dir, profile, sha)?)
    }

    /// The profile that owns a snippet (its folder's profile).
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] when the snippet does not exist; `SQLite` errors.
    pub fn snippet_profile(&self, snippet: Id) -> Result<Id, StoreError> {
        snippet_profile(&self.conn, snippet)
    }

    /// Brings the attachment links of `snippet` in line with its Markdown; see
    /// [`WriteTx::sync_snippet_attachments`].
    ///
    /// # Errors
    ///
    /// As [`WriteTx::sync_snippet_attachments`].
    pub fn sync_snippet_attachments(
        &mut self,
        snippet: Id,
        new_id: &mut dyn FnMut() -> Id,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        self.write(|w| w.sync_snippet_attachments(snippet, new_id, now))
    }
}

fn snippet_profile(conn: &rusqlite::Connection, snippet: Id) -> Result<Id, StoreError> {
    use rusqlite::OptionalExtension;
    let bytes: Option<Vec<u8>> = conn
        .query_row(
            "SELECT profile_id FROM snippet WHERE id = ?1",
            [snippet.as_bytes().as_slice()],
            |r| r.get(0),
        )
        .optional()?;
    let bytes = bytes.ok_or(StoreError::NotFound {
        entity: crate::EntityKind::Snippet,
        id: snippet,
    })?;
    let array: [u8; 16] = bytes
        .try_into()
        .map_err(|_| StoreError::InvalidData("snippet profile id is not 16 bytes".to_owned()))?;
    Ok(Id::from_bytes(array))
}

/// A file name for content that came without one.
fn default_name(mime: &str) -> &'static str {
    match mime {
        "image/png" => "image.png",
        "image/jpeg" => "image.jpg",
        "image/gif" => "image.gif",
        "image/webp" => "image.webp",
        "application/pdf" => "document.pdf",
        "video/mp4" => "video.mp4",
        "audio/mpeg" => "audio.mp3",
        "audio/wav" => "audio.wav",
        _ => "attachment",
    }
}

impl WriteTx<'_> {
    /// The live attachment row of `profile` for `sha`, or a new one with these values.
    ///
    /// # Errors
    ///
    /// `SQLite` errors; [`StoreError::NotFound`] when the profile does not exist.
    #[allow(clippy::too_many_arguments, reason = "the columns of one row")]
    pub fn ensure_attachment(
        &mut self,
        id: Id,
        profile: Id,
        sha: Sha256,
        mime: &str,
        size: u64,
        file_name: &str,
        now: Timestamp,
    ) -> Result<Attachment, StoreError> {
        if let Some(found) = self
            .list::<Attachment>(ListOptions::LIVE)?
            .into_iter()
            .find(|a| a.profile_id == profile && a.sha256 == sha)
        {
            return Ok(found);
        }
        let mut row = Attachment {
            meta: SyncMeta::new(id, now),
            profile_id: profile,
            sha256: sha,
            mime: mime.to_owned(),
            size,
            file_name: file_name.to_owned(),
        };
        row.meta = self.put(&row, now)?;
        Ok(row)
    }

    /// Brings the attachment links of `snippet` in line with its Markdown: a link for every
    /// attachment of the snippet's profile that an image references
    /// (`iamb_core::rich::attachment_refs`), and no other live link. A plain snippet has none. A
    /// reference to content the profile has no row for gets no link (expanding it falls back
    /// to plain text). A deleted or unknown snippet is left alone.
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn sync_snippet_attachments(
        &mut self,
        snippet: Id,
        new_id: &mut dyn FnMut() -> Id,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let Some(row) = self.get::<Snippet>(snippet)? else {
            return Ok(());
        };
        let refs = if row.kind == SnippetKind::Rich {
            iamb_core::rich::attachment_refs(&row.body)
        } else {
            Vec::new()
        };
        let profile = snippet_profile(&self.tx, snippet)?;
        let attachments = self.list::<Attachment>(ListOptions::LIVE)?;
        let wanted: Vec<Id> = refs
            .iter()
            .filter_map(|sha| {
                attachments
                    .iter()
                    .find(|a| a.profile_id == profile && a.sha256 == *sha)
                    .map(|a| a.meta.id)
            })
            .collect();
        let links: Vec<SnippetAttachment> = self
            .list::<SnippetAttachment>(ListOptions::LIVE)?
            .into_iter()
            .filter(|l| l.snippet_id == snippet)
            .collect();
        for link in &links {
            if !wanted.contains(&link.attachment_id) {
                self.delete::<SnippetAttachment>(link.meta.id, now)?;
            }
        }
        for attachment in wanted {
            if !links.iter().any(|l| l.attachment_id == attachment) {
                let link = SnippetAttachment {
                    meta: SyncMeta::new(new_id(), now),
                    snippet_id: snippet,
                    attachment_id: attachment,
                };
                self.put(&link, now)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_are_read_from_the_bytes() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), Some("audio/wav"));
        assert_eq!(sniff(b"%PDF-1.7"), Some("application/pdf"));
        assert_eq!(sniff(b"\0\0\0\x18ftypmp42"), Some("video/mp4"));
        assert_eq!(sniff(b"ID3\x04"), Some("audio/mpeg"));
        assert_eq!(sniff(b"<svg xmlns="), None, "SVG can carry script");
        assert_eq!(sniff(b"<html>"), None);
        assert_eq!(sniff(b""), None);
        assert_eq!(check(b""), Err(AttachmentError::UnsupportedType));
    }

    #[test]
    fn limits_follow_the_kind() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.resize(usize::try_from(MAX_IMAGE_BYTES).expect("fits") + 1, 0);
        assert_eq!(
            check(&png),
            Err(AttachmentError::TooLarge {
                size: MAX_IMAGE_BYTES + 1,
                limit: MAX_IMAGE_BYTES
            })
        );
        png.truncate(usize::try_from(MAX_IMAGE_BYTES).expect("fits"));
        assert_eq!(check(&png), Ok("image/png"));
    }

    #[test]
    fn file_names_are_cleaned() {
        assert_eq!(clean_file_name("C:\\Users\\kim\\logo.png", "x"), "logo.png");
        assert_eq!(clean_file_name("/tmp/../a\u{7}b.gif", "x"), "ab.gif");
        assert_eq!(clean_file_name("  ", "image.png"), "image.png");
        assert_eq!(clean_file_name("..", "image.png"), "image.png");
        assert_eq!(
            clean_file_name(&"n".repeat(300), "x").chars().count(),
            MAX_FILE_NAME_CHARS
        );
    }

    #[test]
    fn hashes_are_sha256() {
        assert_eq!(
            sha256_of(b"abc").to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
