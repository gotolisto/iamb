//! Data model: snippets, folders, tags, profiles (task 1.1).
//!
//! Mirrors `docs/architecture.md` §7. Everything here is plain data with `serde` support. Ids and
//! timestamps are always supplied by the caller; this module performs no I/O, reads no clock and
//! uses no randomness.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Maximum length of an [`Abbreviation`], in Unicode scalar values.
pub const ABBREVIATION_MAX_CHARS: usize = 64;

/// A rule of the data model was broken. One variant per rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// An id string is not exactly 36 characters long.
    IdLength {
        /// Length found, in characters.
        found: usize,
    },
    /// An id string has no hyphen where the canonical form requires one.
    IdHyphen {
        /// Character index of the expected hyphen.
        index: usize,
    },
    /// An id string has a non-hexadecimal digit.
    IdHexDigit {
        /// Character index of the offending character.
        index: usize,
    },
    /// A SHA-256 string is not exactly 64 characters long.
    Sha256Length {
        /// Length found, in characters.
        found: usize,
    },
    /// A SHA-256 string has a character that is not a hexadecimal digit.
    Sha256HexDigit {
        /// Character index of the offending character.
        index: usize,
    },
    /// An abbreviation is empty.
    AbbreviationEmpty,
    /// An abbreviation is longer than [`ABBREVIATION_MAX_CHARS`].
    AbbreviationTooLong {
        /// Length found, in Unicode scalar values.
        found: usize,
    },
    /// An abbreviation contains whitespace.
    AbbreviationWhitespace {
        /// The offending character.
        found: char,
    },
    /// An abbreviation contains a control character.
    AbbreviationControl {
        /// The offending character.
        found: char,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdLength { found } => write!(f, "id must be 36 characters, found {found}"),
            Self::IdHyphen { index } => write!(f, "id must have a hyphen at position {index}"),
            Self::IdHexDigit { index } => {
                write!(f, "id has a non-hexadecimal character at position {index}")
            }
            Self::Sha256Length { found } => {
                write!(f, "sha256 must be 64 hex characters, found {found}")
            }
            Self::Sha256HexDigit { index } => {
                write!(
                    f,
                    "sha256 has a non-hexadecimal character at position {index}"
                )
            }
            Self::AbbreviationEmpty => f.write_str("abbreviation must not be empty"),
            Self::AbbreviationTooLong { found } => write!(
                f,
                "abbreviation must be at most {ABBREVIATION_MAX_CHARS} characters, found {found}"
            ),
            Self::AbbreviationWhitespace { found } => {
                write!(f, "abbreviation must not contain whitespace ({found:?})")
            }
            Self::AbbreviationControl { found } => {
                write!(
                    f,
                    "abbreviation must not contain control characters ({found:?})"
                )
            }
        }
    }
}

impl std::error::Error for ModelError {}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex_val(c: char) -> Option<u8> {
    c.to_digit(16).and_then(|d| u8::try_from(d).ok())
}

// ---------------------------------------------------------------------------------------------
// Id
// ---------------------------------------------------------------------------------------------

/// A 128-bit UUID. Text form is canonical lowercase hyphenated (`8-4-4-4-12`).
///
/// The derived ordering compares the bytes, so UUID version 7 ids sort by creation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Id([u8; 16]);

impl Id {
    /// Wraps raw bytes without checking version or variant.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Builds a UUID version 7 from caller-supplied time and randomness.
    ///
    /// `unix_ms` is truncated to its low 48 bits. Of the 80 random bits, 74 are used; the other
    /// six are replaced by the version and variant markers.
    #[must_use]
    pub fn v7(unix_ms: u64, random: [u8; 10]) -> Self {
        let ms = unix_ms.to_be_bytes();
        let mut b = [0u8; 16];
        b[..6].copy_from_slice(&ms[2..]);
        b[6] = 0x70 | (random[0] & 0x0f);
        b[7] = random[1];
        b[8] = 0x80 | (random[2] & 0x3f);
        b[9..].copy_from_slice(&random[3..]);
        Self(b)
    }

    /// The UUID version nibble.
    #[must_use]
    pub const fn version(&self) -> u8 {
        self.0[6] >> 4
    }

    /// The Unix time in milliseconds embedded in a version 7 id; `None` for other versions.
    #[must_use]
    pub fn timestamp_ms(&self) -> Option<u64> {
        if self.version() != 7 {
            return None;
        }
        let mut ms = [0u8; 8];
        ms[2..].copy_from_slice(&self.0[..6]);
        Some(u64::from_be_bytes(ms))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::with_capacity(36);
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                out.push('-');
            }
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        f.write_str(&out)
    }
}

impl FromStr for Id {
    type Err = ModelError;

    /// Parses the hyphenated form. Upper-case hex digits are accepted; output is always lower.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0u8; 16];
        let mut nibbles = 0usize;
        let mut count = 0usize;
        for (i, c) in s.chars().enumerate() {
            count = i + 1;
            if count > 36 {
                break;
            }
            if matches!(i, 8 | 13 | 18 | 23) {
                if c != '-' {
                    return Err(ModelError::IdHyphen { index: i });
                }
                continue;
            }
            let v = hex_val(c).ok_or(ModelError::IdHexDigit { index: i })?;
            let slot = &mut bytes[nibbles / 2];
            *slot = if nibbles.is_multiple_of(2) {
                v << 4
            } else {
                *slot | v
            };
            nibbles += 1;
        }
        if count != 36 {
            // Count the rest so the error reports the true length.
            return Err(ModelError::IdLength {
                found: s.chars().count(),
            });
        }
        Ok(Self(bytes))
    }
}

impl TryFrom<String> for Id {
    type Error = ModelError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Id> for String {
    fn from(id: Id) -> Self {
        id.to_string()
    }
}

// ---------------------------------------------------------------------------------------------
// Timestamp
// ---------------------------------------------------------------------------------------------

/// UTC milliseconds since the Unix epoch. Serialised as an integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// The number of milliseconds since the Unix epoch.
    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.0
    }
}

// ---------------------------------------------------------------------------------------------
// Sha256
// ---------------------------------------------------------------------------------------------

/// A SHA-256 digest. Serialised as 64 lower-case hex characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256([u8; 32]);

impl Sha256 {
    /// Wraps a digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        f.write_str(&out)
    }
}

impl FromStr for Sha256 {
    type Err = ModelError;

    /// Parses 64 hex characters (either case).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let found = s.chars().count();
        if found != 64 {
            return Err(ModelError::Sha256Length { found });
        }
        let mut bytes = [0u8; 32];
        for (i, c) in s.chars().enumerate() {
            let v = hex_val(c).ok_or(ModelError::Sha256HexDigit { index: i })?;
            let slot = &mut bytes[i / 2];
            *slot = if i.is_multiple_of(2) {
                v << 4
            } else {
                *slot | v
            };
        }
        Ok(Self(bytes))
    }
}

impl TryFrom<String> for Sha256 {
    type Error = ModelError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Sha256> for String {
    fn from(h: Sha256) -> Self {
        h.to_string()
    }
}

// ---------------------------------------------------------------------------------------------
// Abbreviation
// ---------------------------------------------------------------------------------------------

/// A validated snippet abbreviation: 1 to 64 Unicode scalar values, no whitespace, no control
/// characters. Deserialisation validates.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Abbreviation(String);

impl Abbreviation {
    /// Validates and wraps `text`.
    ///
    /// # Errors
    ///
    /// Returns the [`ModelError`] for the first rule broken.
    pub fn new(text: &str) -> Result<Self, ModelError> {
        let mut chars = 0usize;
        for c in text.chars() {
            if c.is_control() {
                return Err(ModelError::AbbreviationControl { found: c });
            }
            if c.is_whitespace() {
                return Err(ModelError::AbbreviationWhitespace { found: c });
            }
            chars += 1;
        }
        if chars == 0 {
            return Err(ModelError::AbbreviationEmpty);
        }
        if chars > ABBREVIATION_MAX_CHARS {
            return Err(ModelError::AbbreviationTooLong { found: chars });
        }
        Ok(Self(text.to_owned()))
    }

    /// The abbreviation as typed.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length in Unicode scalar values.
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.0.chars().count()
    }

    /// The case-folded form used for uniqueness and matching.
    ///
    /// The folding rule is the matcher's, applied one character at a time: each character is
    /// lower-cased on its own with [`char::to_lowercase`] (Unicode default lower-casing, no
    /// normalisation, no context), after mapping final sigma `ς` to `σ`. So `ΟΔΟΣ`, `οδοσ` and
    /// `οδος` all fold to `οδοσ`, where [`str::to_lowercase`] would give `οδος` for the first
    /// (it picks the final form at the end of a word). Two abbreviations collide when their fold
    /// keys are equal, and exactly then the matcher treats them as one: the store's uniqueness
    /// index and the matcher use the same key (review 1.9 finding 12).
    #[must_use]
    pub fn fold_key(&self) -> String {
        fold_abbreviation(&self.0)
    }
}

/// Folds one character of an abbreviation or of typed text (see [`Abbreviation::fold_key`]).
pub(crate) fn fold_char(c: char) -> std::char::ToLowercase {
    if c == 'ς' { 'σ' } else { c }.to_lowercase()
}

/// Folds abbreviation text the way [`Abbreviation::fold_key`] does, for text that has not been
/// validated as an abbreviation (for example the `abbr` of a `{snippet}` macro).
#[must_use]
pub fn fold_abbreviation(text: &str) -> String {
    text.chars().flat_map(fold_char).collect()
}

impl fmt::Display for Abbreviation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Abbreviation {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for Abbreviation {
    type Err = ModelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for Abbreviation {
    type Error = ModelError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(&s)
    }
}

impl From<Abbreviation> for String {
    fn from(a: Abbreviation) -> Self {
        a.0
    }
}

// ---------------------------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------------------------

/// Kind of a [`Profile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// A user's own profile.
    Personal,
    /// A family group.
    Family,
    /// A company group.
    Company,
}

/// Role of a user in a group profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Owns the group.
    Owner,
    /// Manages members and content.
    Admin,
    /// Ordinary member.
    Member,
}

/// Permission granted on a shared or group-owned folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Read only.
    View,
    /// Read and write.
    Edit,
}

/// Whether a snippet body is plain text or Markdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnippetKind {
    /// Plain text template.
    Plain,
    /// Markdown template.
    Rich,
}

/// When a snippet fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// On the last character of the abbreviation.
    Immediate,
    /// When a delimiter follows the abbreviation.
    Delimiter,
}

/// How case is treated when matching and expanding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseMode {
    /// Case-sensitive match.
    Exact,
    /// Case-insensitive match.
    Insensitive,
    /// Case-insensitive match; the expansion follows the typed case pattern.
    Adaptive,
}

// ---------------------------------------------------------------------------------------------
// SyncMeta and entities
// ---------------------------------------------------------------------------------------------

/// Fields common to every synced row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncMeta {
    /// Row id (UUID version 7, created on the device).
    pub id: Id,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last modification time.
    pub updated_at: Timestamp,
    /// User who last modified the row; `None` before the first sign-in.
    pub updated_by: Option<Id>,
    /// Revision assigned by the server; 0 until first sync.
    pub rev: u64,
    /// Soft-delete time, if deleted.
    pub deleted_at: Option<Timestamp>,
}

impl SyncMeta {
    /// Metadata for a row created locally at `now`: unsynced, not deleted, no author yet.
    #[must_use]
    pub const fn new(id: Id, now: Timestamp) -> Self {
        Self {
            id,
            created_at: now,
            updated_at: now,
            updated_by: None,
            rev: 0,
            deleted_at: None,
        }
    }

    /// Whether the row is soft-deleted.
    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// A profile: personal, family or company.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Common fields.
    #[serde(flatten)]
    pub meta: SyncMeta,
    /// Kind of profile.
    pub kind: ProfileKind,
    /// Display name.
    pub name: String,
    /// User that owns the profile.
    pub owner_user_id: Id,
}

/// A user's membership of a group profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    /// Common fields.
    #[serde(flatten)]
    pub meta: SyncMeta,
    /// The group profile.
    pub profile_id: Id,
    /// The member.
    pub user_id: Id,
    /// The member's role.
    pub role: Role,
}

/// Defaults a folder applies to snippets created in it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderDefaults {
    /// Default trigger mode.
    pub trigger: Option<Trigger>,
    /// Default case mode.
    pub case_mode: Option<CaseMode>,
    /// Text put in front of the abbreviation when a snippet is created.
    pub abbreviation_prefix: Option<String>,
}

impl FolderDefaults {
    /// Applies the folder's abbreviation prefix (if any) to `typed` and validates the result.
    ///
    /// # Errors
    ///
    /// Returns the [`ModelError`] if the combined text is not a valid abbreviation.
    pub fn prefixed_abbreviation(&self, typed: &str) -> Result<Abbreviation, ModelError> {
        match &self.abbreviation_prefix {
            Some(prefix) => Abbreviation::new(&format!("{prefix}{typed}")),
            None => Abbreviation::new(typed),
        }
    }
}

/// A folder of snippets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// Common fields.
    #[serde(flatten)]
    pub meta: SyncMeta,
    /// Owning profile.
    pub profile_id: Id,
    /// Parent folder, if nested.
    pub parent_id: Option<Id>,
    /// Display name.
    pub name: String,
    /// Sort position among siblings.
    pub position: i64,
    /// Permission members get (group-owned folders only).
    pub member_permission: Option<Permission>,
    /// Defaults for new snippets.
    pub defaults: FolderDefaults,
}

/// A personally owned folder shared to a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderShare {
    /// Common fields.
    #[serde(flatten)]
    pub meta: SyncMeta,
    /// The shared folder.
    pub folder_id: Id,
    /// The group it is shared to.
    pub profile_id: Id,
    /// Permission the group gets.
    pub permission: Permission,
}

/// A text snippet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    /// Common fields.
    #[serde(flatten)]
    pub meta: SyncMeta,
    /// Containing folder.
    pub folder_id: Id,
    /// What the user types.
    pub abbreviation: Abbreviation,
    /// Human label.
    pub label: String,
    /// Plain or rich body.
    pub kind: SnippetKind,
    /// Template source.
    pub body: String,
    /// Trigger mode.
    pub trigger: Trigger,
    /// Case mode.
    pub case_mode: CaseMode,
    /// The character before the abbreviation must be a boundary.
    pub word_start: bool,
    /// Whether the snippet is matchable.
    pub enabled: bool,
}

impl Snippet {
    /// Creates a plain, enabled snippet with defaults applied (architecture §4.4).
    ///
    /// Trigger is the folder default, else [`Trigger::Delimiter`]. Case mode is the folder
    /// default, else [`CaseMode::Insensitive`]. `word_start` is true for delimiter mode and false
    /// for immediate mode. The folder's abbreviation prefix is NOT applied here; use
    /// [`FolderDefaults::prefixed_abbreviation`] to build `abbreviation`.
    #[must_use]
    pub fn new_defaults(
        meta: SyncMeta,
        folder_id: Id,
        abbreviation: Abbreviation,
        label: impl Into<String>,
        body: impl Into<String>,
        folder_defaults: Option<&FolderDefaults>,
    ) -> Self {
        let trigger = folder_defaults
            .and_then(|d| d.trigger)
            .unwrap_or(Trigger::Delimiter);
        let case_mode = folder_defaults
            .and_then(|d| d.case_mode)
            .unwrap_or(CaseMode::Insensitive);
        Self {
            meta,
            folder_id,
            abbreviation,
            label: label.into(),
            kind: SnippetKind::Plain,
            body: body.into(),
            trigger,
            case_mode,
            word_start: trigger == Trigger::Delimiter,
            enabled: true,
        }
    }
}

/// A tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    /// Common fields.
    #[serde(flatten)]
    pub meta: SyncMeta,
    /// Owning profile.
    pub profile_id: Id,
    /// Display name.
    pub name: String,
    /// Optional colour (free text, e.g. `#aabbcc`).
    pub color: Option<String>,
}

/// Link between a snippet and a tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetTag {
    /// Common fields.
    #[serde(flatten)]
    pub meta: SyncMeta,
    /// The snippet.
    pub snippet_id: Id,
    /// The tag.
    pub tag_id: Id,
}

/// Metadata of a stored file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Common fields.
    #[serde(flatten)]
    pub meta: SyncMeta,
    /// Owning profile.
    pub profile_id: Id,
    /// Content hash.
    pub sha256: Sha256,
    /// Media type.
    pub mime: String,
    /// Size in bytes.
    pub size: u64,
    /// Original file name.
    pub file_name: String,
}

/// Link between a snippet and an attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetAttachment {
    /// Common fields.
    #[serde(flatten)]
    pub meta: SyncMeta,
    /// The snippet.
    pub snippet_id: Id,
    /// The attachment.
    pub attachment_id: Id,
}
