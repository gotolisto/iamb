//! Import and export of a profile's library as JSON or CSV (task 1.7).
//!
//! [`export`] writes the live folders, snippets and tags of one profile. [`import`] reads such a
//! file into a profile, giving every row a fresh id. Both work on whole files: an import reads and
//! checks the entire file before it writes, and the writes are one transaction ([`Store::write`]),
//! so a failed import leaves the store unchanged.
//!
//! JSON is the lossless format. It is one document with a `format` and `version` header and the
//! model types' own serde form for `folders`, `snippets`, `tags` and `snippet_tags`. When rich
//! snippets reference attachments (task 3.8), an `attachments` array carries each one the
//! profile has, once: `sha256`, `mime`, `file_name` and the bytes as standard base64 in `data`.
//! An import checks every attachment (type, size and hash, `crate::attachments`) before it
//! writes anything, keeps the bytes in the target profile under the same hash (so the snippets'
//! `attachment:` references stay valid) and links the imported snippets to them. A file
//! without the array is read as before; an older build ignores it. CSV carries
//! snippets only, with folders and tags written as text, for spreadsheets and hand editing.
//!
//! # CSV escaping
//!
//! The `folder` cell joins folder names with `" / "` (space, slash, space) and the `tags` cell
//! joins tag names with `;`. A name may itself contain those characters, so each name is escaped
//! when written and unescaped when read. A backslash followed by a backslash, a slash, a
//! semicolon or a whitespace character stands for that character alone; a backslash followed by
//! anything else (or at the end of the cell) is an ordinary backslash, so hand-made files with
//! Windows paths keep working. On export:
//!
//! - `\` becomes `\\` in every name;
//! - in a folder name, a `/` that follows a space becomes `\/`, so `R / D` is written `R \/ D`
//!   and never reads as two levels (a `/` not after a space, as in `TCP/IP`, is left alone);
//! - in a tag name, `;` becomes `\;`;
//! - leading and trailing whitespace becomes `\` plus the character, because import trims
//!   unescaped whitespace around each name.
//!
//! An empty name cannot be written and fails the export. On import, names are split at the
//! unescaped separators, trimmed (escaped whitespace is kept) and unescaped.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use iamb_core::model::{
    Abbreviation, CaseMode, Folder, FolderDefaults, Id, Sha256, Snippet, SnippetKind, SnippetTag,
    SyncMeta, Tag, Timestamp, Trigger,
};
use serde_json::{Map, Value};

use crate::{Library, ListOptions, MAX_FOLDER_DEPTH, Store, StoreError, WriteTx};

/// The `format` value of a JSON library file.
const JSON_FORMAT: &str = "iamb-library";
/// The only JSON `version` this build reads and writes.
const JSON_VERSION: u64 = 1;
/// The CSV header, in the order columns are written.
const CSV_HEADER: [&str; 10] = [
    "abbreviation",
    "label",
    "body",
    "folder",
    "tags",
    "trigger",
    "case_mode",
    "word_start",
    "enabled",
    "kind",
];
/// Separates folder names in the CSV `folder` column.
const FOLDER_SEPARATOR: &str = " / ";
/// Separates tag names in the CSV `tags` column.
const TAG_SEPARATOR: char = ';';
/// Folder for CSV rows whose `folder` cell is empty.
const DEFAULT_FOLDER: &str = "Imported";

const TRIGGER_CHOICES: [(&str, Trigger); 2] = [
    ("immediate", Trigger::Immediate),
    ("delimiter", Trigger::Delimiter),
];
const CASE_CHOICES: [(&str, CaseMode); 3] = [
    ("exact", CaseMode::Exact),
    ("insensitive", CaseMode::Insensitive),
    ("adaptive", CaseMode::Adaptive),
];
const KIND_CHOICES: [(&str, SnippetKind); 2] =
    [("plain", SnippetKind::Plain), ("rich", SnippetKind::Rich)];
const BOOL_CHOICES: [(&str, bool); 4] =
    [("true", true), ("false", false), ("1", true), ("0", false)];

/// The file format of an export or import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The lossless JSON library document.
    Json,
    /// Snippets as CSV, one row per snippet.
    Csv,
}

/// What an import does with an abbreviation that the profile already uses (by case-folded key).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnConflict {
    /// Leave the existing snippet alone, count the new row as skipped and carry on.
    #[default]
    Skip,
    /// Stop, roll back the whole import and report the conflict.
    Fail,
}

/// What an export wrote.
///
/// CSV has no folder or tag rows, so for CSV `folders` and `tags` are 0 and `snippet_tags` counts
/// the tag names written into snippet rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportSummary {
    /// Snippets written.
    pub snippets: usize,
    /// Folders written (JSON only).
    pub folders: usize,
    /// Tags written (JSON only).
    pub tags: usize,
    /// Snippet–tag links written.
    pub snippet_tags: usize,
}

/// What an import created or skipped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportSummary {
    /// Folders created.
    pub folders_created: usize,
    /// Snippets written.
    pub snippets_imported: usize,
    /// Snippets not written because their abbreviation was already in use (with [`OnConflict::Skip`]).
    pub snippets_skipped: usize,
    /// Tags created.
    pub tags_created: usize,
}

/// Why an export or import failed. Nothing is written when an import returns an error.
#[derive(Debug)]
#[non_exhaustive]
pub enum TransferError {
    /// The file could not be read or written.
    Io {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A JSON file is not well-formed JSON (or not UTF-8).
    Json {
        /// Line of the error, 0 if unknown.
        line: usize,
        /// Column of the error, 0 if unknown.
        column: usize,
        /// What the parser reported.
        message: String,
    },
    /// A CSV file is malformed (unterminated quote, wrong number of fields, bad encoding).
    Csv {
        /// Line of the error, if the parser knows it.
        line: Option<u64>,
        /// What the parser reported.
        message: String,
    },
    /// The JSON `format` is not `"iamb-library"`.
    WrongFormat {
        /// The `format` found, as text.
        found: String,
    },
    /// The JSON `version` is not one this build reads.
    UnsupportedVersion {
        /// The `version` found, as text.
        found: String,
    },
    /// A CSV file has no header for a required column.
    MissingColumn {
        /// The column name.
        column: &'static str,
    },
    /// A value is not valid: an abbreviation, an enum word, a name or a reference.
    InvalidValue {
        /// Where it is: `row 3 (line 4)` for CSV, `snippets[2]` for JSON.
        location: String,
        /// What is wrong with it.
        message: String,
    },
    /// A folder path would nest deeper than the store allows.
    FolderTooDeep {
        /// Where it is.
        location: String,
        /// The depth it would reach.
        depth: usize,
    },
    /// An abbreviation is already in use, and the import was asked to fail on that.
    Conflict {
        /// Where the conflicting row is.
        location: String,
        /// The abbreviation as the row gives it (or its case-folded key, from the store).
        abbreviation: String,
        /// The snippet that already holds it.
        existing: Id,
    },
    /// The store reported an error.
    Store(StoreError),
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "cannot read or write {}: {source}", path.display())
            }
            Self::Json { message, .. } => write!(f, "invalid JSON: {message}"),
            Self::Csv {
                line: Some(line),
                message,
            } => write!(f, "invalid CSV at line {line}: {message}"),
            Self::Csv {
                line: None,
                message,
            } => write!(f, "invalid CSV: {message}"),
            Self::WrongFormat { found } => write!(
                f,
                "not an Iamb library file: format is {found}, expected \"{JSON_FORMAT}\""
            ),
            Self::UnsupportedVersion { found } => write!(
                f,
                "unsupported library file version {found}; this build reads version {JSON_VERSION}"
            ),
            Self::MissingColumn { column } => write!(f, "CSV file has no \"{column}\" column"),
            Self::InvalidValue { location, message } => write!(f, "{location}: {message}"),
            Self::FolderTooDeep { location, depth } => write!(
                f,
                "{location}: folder would be nested {depth} levels deep (at most {MAX_FOLDER_DEPTH})"
            ),
            Self::Conflict {
                location,
                abbreviation,
                existing,
            } => write!(
                f,
                "{location}: abbreviation {abbreviation:?} is already used by snippet {existing}"
            ),
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for TransferError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<StoreError> for TransferError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// Writes the live folders, snippets and tags of `profile` to `path`.
///
/// Rows are written in a stable order (by folder path, then abbreviation), so exporting the same
/// library twice gives the same bytes. The file is UTF-8 with LF line endings.
///
/// # Errors
///
/// [`TransferError::Io`] if the file cannot be written; [`TransferError::InvalidValue`] for CSV if
/// a folder or tag name is empty (separators and surrounding spaces are escaped, see the module
/// docs); [`TransferError::Store`] on store errors.
pub fn export(
    store: &Store,
    profile: Id,
    path: &Path,
    format: Format,
) -> Result<ExportSummary, TransferError> {
    let lib = scoped(&store.load_library()?, profile);
    let (bytes, summary) = match format {
        Format::Json => encode_json(&lib, &exported_attachments(store, profile, &lib)?)?,
        Format::Csv => encode_csv(&lib)?,
    };
    fs::write(path, bytes).map_err(|source| io_error(path, source))?;
    Ok(summary)
}

/// Reads a file written by [`export`] (or a hand-made CSV) into `profile`.
///
/// Every imported row gets a fresh id from `new_id` and the time `now`. JSON keeps folder, tag
/// and link structure by remapping ids; a live folder with the same name under the same parent,
/// and a live tag with the same name, are reused rather than created again, and folders and tags
/// are created only for snippets that are written (an import that skips every snippet creates
/// nothing), so importing one file twice adds nothing the second time. CSV creates folders from the `folder` paths (reusing a
/// live folder of the same name at the same level), creates tags by name when missing, and fills
/// missing optional columns from [`Snippet::new_defaults`]. A UTF-8 byte-order mark and CRLF line
/// endings are accepted.
///
/// The import is one transaction: on any error nothing is written. The store is taken mutably
/// because the transaction needs it.
///
/// # Errors
///
/// [`TransferError::Io`] if the file cannot be read; [`TransferError::Json`] or
/// [`TransferError::Csv`] for malformed input; [`TransferError::WrongFormat`],
/// [`TransferError::UnsupportedVersion`], [`TransferError::MissingColumn`],
/// [`TransferError::InvalidValue`] and [`TransferError::FolderTooDeep`] for input the model
/// rejects; [`TransferError::Conflict`] with [`OnConflict::Fail`]; [`TransferError::Store`]
/// otherwise.
pub fn import(
    store: &mut Store,
    profile: Id,
    path: &Path,
    format: Format,
    now: Timestamp,
    new_id: &mut dyn FnMut() -> Id,
    on_conflict: OnConflict,
) -> Result<ImportSummary, TransferError> {
    let bytes = fs::read(path).map_err(|source| io_error(path, source))?;
    let data = strip_bom(&bytes);
    let ctx = Ctx {
        profile,
        now,
        on_conflict,
    };
    match format {
        Format::Json => import_json(store, ctx, data, new_id),
        Format::Csv => import_csv(store, ctx, data, new_id),
    }
}

// ---- shared -------------------------------------------------------------------------------

/// The settings of one import.
#[derive(Debug, Clone, Copy)]
struct Ctx {
    profile: Id,
    now: Timestamp,
    on_conflict: OnConflict,
}

fn io_error(path: &Path, source: std::io::Error) -> TransferError {
    TransferError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn invalid(location: &str, message: impl Into<String>) -> TransferError {
    TransferError::InvalidValue {
        location: location.to_owned(),
        message: message.into(),
    }
}

/// Maps a store error from a write at `location` to the matching transfer error.
fn at<T>(result: Result<T, StoreError>, location: &str) -> Result<T, TransferError> {
    result.map_err(|e| match e {
        StoreError::AbbreviationConflict { key, existing } => TransferError::Conflict {
            location: location.to_owned(),
            abbreviation: key,
            existing,
        },
        StoreError::FolderTooDeep { depth, .. } => TransferError::FolderTooDeep {
            location: location.to_owned(),
            depth,
        },
        other => TransferError::Store(other),
    })
}

/// Runs an import body in one write transaction. A [`TransferError`] from the body rolls the
/// transaction back and is returned as itself; store errors are returned as
/// [`TransferError::Store`].
fn apply(
    store: &mut Store,
    body: impl FnOnce(&mut WriteTx<'_>) -> Result<ImportSummary, TransferError>,
) -> Result<ImportSummary, TransferError> {
    let mut failure = None;
    let outcome = store.write(|w| {
        body(w).map_err(|e| match e {
            TransferError::Store(inner) => inner,
            other => {
                failure = Some(other);
                StoreError::InvalidData("import stopped".to_owned())
            }
        })
    });
    match outcome {
        Ok(summary) => Ok(summary),
        Err(e) => Err(failure.unwrap_or(TransferError::Store(e))),
    }
}

/// Whether a row with this abbreviation should be written. `Ok(false)` means skip it.
fn admit(
    on_conflict: OnConflict,
    taken: &HashMap<String, Id>,
    abbreviation: &Abbreviation,
    location: &str,
) -> Result<bool, TransferError> {
    match taken.get(&abbreviation.fold_key()) {
        None => Ok(true),
        Some(&existing) => match on_conflict {
            OnConflict::Skip => Ok(false),
            OnConflict::Fail => Err(TransferError::Conflict {
                location: location.to_owned(),
                abbreviation: abbreviation.to_string(),
                existing,
            }),
        },
    }
}

/// Fold keys of the live snippets of `profile`, with their ids.
fn profile_abbreviations(w: &WriteTx<'_>, profile: Id) -> Result<HashMap<String, Id>, StoreError> {
    let folders: HashSet<Id> = w
        .list::<Folder>(ListOptions::LIVE)?
        .into_iter()
        .filter(|f| f.profile_id == profile)
        .map(|f| f.meta.id)
        .collect();
    Ok(w.list::<Snippet>(ListOptions::LIVE)?
        .into_iter()
        .filter(|s| folders.contains(&s.folder_id))
        .map(|s| (s.abbreviation.fold_key(), s.meta.id))
        .collect())
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes)
}

// ---- export -------------------------------------------------------------------------------

/// The rows of `lib` that belong to `profile`.
fn scoped(lib: &Library, profile: Id) -> Library {
    let folders: Vec<Folder> = lib
        .folders
        .iter()
        .filter(|f| f.profile_id == profile)
        .cloned()
        .collect();
    let folder_ids: HashSet<Id> = folders.iter().map(|f| f.meta.id).collect();
    let tags: Vec<Tag> = lib
        .tags
        .iter()
        .filter(|t| t.profile_id == profile)
        .cloned()
        .collect();
    let tag_ids: HashSet<Id> = tags.iter().map(|t| t.meta.id).collect();
    let snippets: Vec<Snippet> = lib
        .snippets
        .iter()
        .filter(|s| folder_ids.contains(&s.folder_id))
        .cloned()
        .collect();
    let snippet_ids: HashSet<Id> = snippets.iter().map(|s| s.meta.id).collect();
    let snippet_tags = lib
        .snippet_tags
        .iter()
        .filter(|l| snippet_ids.contains(&l.snippet_id) && tag_ids.contains(&l.tag_id))
        .cloned()
        .collect();
    Library {
        folders,
        snippets,
        tags,
        snippet_tags,
    }
}

/// The names from the root down to each folder.
fn folder_paths(folders: &[Folder]) -> HashMap<Id, Vec<String>> {
    let by_id: HashMap<Id, &Folder> = folders.iter().map(|f| (f.meta.id, f)).collect();
    folders
        .iter()
        .map(|f| {
            let mut names = Vec::new();
            let mut current = Some(f);
            while let Some(folder) = current {
                // A parent cycle cannot come from the store, but never loop forever on one.
                if names.len() > MAX_FOLDER_DEPTH {
                    break;
                }
                names.push(folder.name.clone());
                current = folder.parent_id.and_then(|p| by_id.get(&p).copied());
            }
            names.reverse();
            (f.meta.id, names)
        })
        .collect()
}

fn path_of(paths: &HashMap<Id, Vec<String>>, folder: Id) -> Vec<String> {
    paths.get(&folder).cloned().unwrap_or_default()
}

/// The attachments the rich snippets of `lib` reference that `profile` has, with their bytes,
/// as JSON values in hash order. Content whose bytes are not on this device is left out.
fn exported_attachments(
    store: &Store,
    profile: Id,
    lib: &Library,
) -> Result<Vec<Value>, TransferError> {
    use base64::Engine as _;
    let mut refs: Vec<String> = Vec::new();
    for s in lib.snippets.iter().filter(|s| s.kind == SnippetKind::Rich) {
        for sha in iamb_core::rich::attachment_refs(&s.body) {
            refs.push(sha.to_string());
        }
    }
    refs.sort();
    refs.dedup();
    let mut out = Vec::new();
    for text in refs {
        let Ok(sha) = text.parse::<Sha256>() else {
            continue;
        };
        let Some(row) = store.find_attachment(profile, &sha)? else {
            continue;
        };
        let Some(bytes) = store.read_attachment(profile, &sha)? else {
            continue;
        };
        let mut item = Map::new();
        item.insert("sha256".to_owned(), Value::from(text));
        item.insert("mime".to_owned(), Value::from(row.mime));
        item.insert("file_name".to_owned(), Value::from(row.file_name));
        item.insert(
            "data".to_owned(),
            Value::from(base64::engine::general_purpose::STANDARD.encode(bytes)),
        );
        out.push(Value::Object(item));
    }
    Ok(out)
}

fn encode_json(
    lib: &Library,
    attachments: &[Value],
) -> Result<(Vec<u8>, ExportSummary), TransferError> {
    let paths = folder_paths(&lib.folders);
    let mut folders = lib.folders.clone();
    folders.sort_by_key(|f| (path_of(&paths, f.meta.id), f.meta.id));
    let mut snippets = lib.snippets.clone();
    snippets.sort_by_key(|s| {
        (
            path_of(&paths, s.folder_id),
            s.abbreviation.fold_key(),
            s.meta.id,
        )
    });
    let mut tags = lib.tags.clone();
    tags.sort_by(|a, b| a.name.cmp(&b.name).then(a.meta.id.cmp(&b.meta.id)));
    let snippet_keys: HashMap<Id, String> = snippets
        .iter()
        .map(|s| (s.meta.id, s.abbreviation.fold_key()))
        .collect();
    let tag_names: HashMap<Id, String> = tags.iter().map(|t| (t.meta.id, t.name.clone())).collect();
    let mut links = lib.snippet_tags.clone();
    links.sort_by_key(|l| {
        (
            snippet_keys.get(&l.snippet_id).cloned().unwrap_or_default(),
            tag_names.get(&l.tag_id).cloned().unwrap_or_default(),
            l.tag_id,
        )
    });

    let mut doc = Map::new();
    doc.insert("format".to_owned(), Value::from(JSON_FORMAT));
    doc.insert("version".to_owned(), Value::from(JSON_VERSION));
    doc.insert(
        "folders".to_owned(),
        serde_json::to_value(&folders).map_err(|e| json_error(&e))?,
    );
    doc.insert(
        "snippets".to_owned(),
        serde_json::to_value(&snippets).map_err(|e| json_error(&e))?,
    );
    doc.insert(
        "tags".to_owned(),
        serde_json::to_value(&tags).map_err(|e| json_error(&e))?,
    );
    doc.insert(
        "snippet_tags".to_owned(),
        serde_json::to_value(&links).map_err(|e| json_error(&e))?,
    );
    if !attachments.is_empty() {
        doc.insert("attachments".to_owned(), Value::Array(attachments.to_vec()));
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(doc)).map_err(|e| json_error(&e))?;
    text.push('\n');
    let summary = ExportSummary {
        snippets: snippets.len(),
        folders: folders.len(),
        tags: tags.len(),
        snippet_tags: links.len(),
    };
    Ok((text.into_bytes(), summary))
}

fn encode_csv(lib: &Library) -> Result<(Vec<u8>, ExportSummary), TransferError> {
    let paths = folder_paths(&lib.folders);
    let tag_names: HashMap<Id, &str> = lib
        .tags
        .iter()
        .map(|t| (t.meta.id, t.name.as_str()))
        .collect();
    let mut tags_of: HashMap<Id, Vec<&str>> = HashMap::new();
    for link in &lib.snippet_tags {
        if let Some(name) = tag_names.get(&link.tag_id) {
            tags_of.entry(link.snippet_id).or_default().push(name);
        }
    }

    let mut rows: Vec<(Vec<String>, &Snippet)> = lib
        .snippets
        .iter()
        .map(|s| (path_of(&paths, s.folder_id), s))
        .collect();
    rows.sort_by(|(pa, a), (pb, b)| {
        pa.cmp(pb)
            .then_with(|| a.abbreviation.fold_key().cmp(&b.abbreviation.fold_key()))
            .then_with(|| a.meta.id.cmp(&b.meta.id))
    });

    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer.write_record(CSV_HEADER).map_err(|e| csv_error(&e))?;
    let mut links = 0;
    for (path, s) in &rows {
        let folder = csv_folder_cell(path)?;
        let mut names = tags_of.get(&s.meta.id).cloned().unwrap_or_default();
        names.sort_unstable();
        let tags = csv_tags_cell(&names)?;
        links += names.len();
        writer
            .write_record([
                s.abbreviation.as_str(),
                s.label.as_str(),
                s.body.as_str(),
                folder.as_str(),
                tags.as_str(),
                trigger_text(s.trigger),
                case_text(s.case_mode),
                bool_text(s.word_start),
                bool_text(s.enabled),
                kind_text(s.kind),
            ])
            .map_err(|e| csv_error(&e))?;
    }
    let bytes = writer.into_inner().map_err(|e| TransferError::Csv {
        line: None,
        message: e.to_string(),
    })?;
    let summary = ExportSummary {
        snippets: rows.len(),
        folders: 0,
        tags: 0,
        snippet_tags: links,
    };
    Ok((bytes, summary))
}

/// The `folder` cell for a path. Names must survive the import's trimming and splitting.
fn csv_folder_cell(path: &[String]) -> Result<String, TransferError> {
    let mut escaped = Vec::with_capacity(path.len());
    for name in path {
        if name.is_empty() {
            return Err(invalid(
                "folder \"\"",
                "a folder name must not be empty, so it cannot be written to CSV",
            ));
        }
        escaped.push(escape_name(name, |prev, c| c == '/' && prev == Some(' ')));
    }
    Ok(escaped.join(FOLDER_SEPARATOR))
}

fn csv_tags_cell(names: &[&str]) -> Result<String, TransferError> {
    let mut escaped = Vec::with_capacity(names.len());
    for name in names {
        if name.is_empty() {
            return Err(invalid(
                "tag \"\"",
                "a tag name must not be empty, so it cannot be written to CSV",
            ));
        }
        escaped.push(escape_name(name, |_, c| c == TAG_SEPARATOR));
    }
    Ok(escaped.join(&TAG_SEPARATOR.to_string()))
}

/// Escapes one name for a CSV cell (see the module docs). `special(previous, c)` says whether
/// `c` needs a backslash in front of it besides the backslash itself and the edge whitespace.
fn escape_name(name: &str, special: impl Fn(Option<char>, char) -> bool) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 2);
    let mut previous = None;
    for (i, &c) in chars.iter().enumerate() {
        let edge = c.is_whitespace() && (i == 0 || i + 1 == chars.len());
        if c == '\\' || edge || special(previous, c) {
            out.push('\\');
        }
        out.push(c);
        previous = Some(c);
    }
    out
}

/// Whether a backslash in front of `c` is an escape.
fn escapable(c: char) -> bool {
    c == '\\' || c == '/' || c == TAG_SEPARATOR || c.is_whitespace()
}

/// Splits `text` at the unescaped occurrences of `separator`, trims unescaped whitespace around
/// each part and removes the escapes.
fn split_escaped(text: &str, separator: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let sep: Vec<char> = separator.chars().collect();
    let mut parts: Vec<Vec<(char, bool)>> = vec![Vec::new()];
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c == '\\'
            && let Some(&next) = chars.get(i + 1)
            && escapable(next)
        {
            if let Some(p) = parts.last_mut() {
                p.push((next, true));
            }
            i += 2;
        } else if chars[i..].starts_with(&sep) {
            parts.push(Vec::new());
            i += sep.len();
        } else {
            if let Some(p) = parts.last_mut() {
                p.push((c, false));
            }
            i += 1;
        }
    }
    parts
        .into_iter()
        .map(|part| {
            let keep = |&(c, escaped): &(char, bool)| escaped || !c.is_whitespace();
            let start = part.iter().position(keep).unwrap_or(part.len());
            let end = part.iter().rposition(keep).map_or(start, |e| e + 1);
            part[start..end].iter().map(|&(c, _)| c).collect()
        })
        .collect()
}

const fn trigger_text(t: Trigger) -> &'static str {
    match t {
        Trigger::Immediate => "immediate",
        Trigger::Delimiter => "delimiter",
    }
}

const fn case_text(c: CaseMode) -> &'static str {
    match c {
        CaseMode::Exact => "exact",
        CaseMode::Insensitive => "insensitive",
        CaseMode::Adaptive => "adaptive",
    }
}

const fn kind_text(k: SnippetKind) -> &'static str {
    match k {
        SnippetKind::Plain => "plain",
        SnippetKind::Rich => "rich",
    }
}

const fn bool_text(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

fn json_error(e: &serde_json::Error) -> TransferError {
    TransferError::Json {
        line: e.line(),
        column: e.column(),
        message: e.to_string(),
    }
}

fn csv_error(e: &csv::Error) -> TransferError {
    TransferError::Csv {
        line: e.position().map(csv::Position::line),
        message: e.to_string(),
    }
}

// ---- JSON import --------------------------------------------------------------------------

/// The rows of a JSON library file, as the model types read them.
#[derive(Debug)]
struct JsonDoc {
    folders: Vec<Folder>,
    tags: Vec<Tag>,
    snippets: Vec<Snippet>,
    links: Vec<SnippetTag>,
    attachments: Vec<ImportedAttachment>,
}

/// One checked attachment of a JSON library file.
struct ImportedAttachment {
    sha: Sha256,
    mime: &'static str,
    file_name: String,
    bytes: Vec<u8>,
}

impl std::fmt::Debug for ImportedAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedAttachment")
            .field("sha", &self.sha)
            .field("mime", &self.mime)
            .field("len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Reads and checks the `attachments` array (module docs).
fn parse_attachments(obj: &Map<String, Value>) -> Result<Vec<ImportedAttachment>, TransferError> {
    use base64::Engine as _;
    let Some(value) = obj.get("attachments") else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| invalid("attachments", "expected an array"))?;
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let location = format!("attachments[{i}]");
        let field = |name: &str| -> Result<&str, TransferError> {
            item.get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| invalid(&location, format!("expected a text `{name}`")))
        };
        let sha: Sha256 = field("sha256")?
            .parse()
            .map_err(|e: iamb_core::model::ModelError| invalid(&location, e.to_string()))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(field("data")?)
            .map_err(|e| invalid(&location, format!("`data` is not base64: {e}")))?;
        let mime =
            crate::attachments::check(&bytes).map_err(|e| invalid(&location, e.to_string()))?;
        if crate::attachments::sha256_of(&bytes) != sha {
            return Err(invalid(
                &location,
                crate::AttachmentError::HashMismatch(sha).to_string(),
            ));
        }
        let file_name = crate::attachments::clean_file_name(
            item.get("file_name").and_then(Value::as_str).unwrap_or(""),
            "attachment",
        );
        out.push(ImportedAttachment {
            sha,
            mime,
            file_name,
            bytes,
        });
    }
    Ok(out)
}

fn import_json(
    store: &mut Store,
    ctx: Ctx,
    data: &[u8],
    new_id: &mut dyn FnMut() -> Id,
) -> Result<ImportSummary, TransferError> {
    let text = std::str::from_utf8(data).map_err(|e| TransferError::Json {
        line: 0,
        column: 0,
        message: format!("file is not valid UTF-8: {e}"),
    })?;
    let doc: Value = serde_json::from_str(text).map_err(|e| json_error(&e))?;
    let plan = JsonPlan::new(parse_json(&doc)?, new_id)?;
    // Attachment bytes go to their place before the transaction: they are named by their
    // content, so bytes left there by an import that then fails harm nothing.
    if !plan.doc.attachments.is_empty() {
        let dir = store
            .data_dir()
            .map(Path::to_path_buf)
            .ok_or(StoreError::Attachment(
                crate::AttachmentError::NoDataDirectory,
            ))?;
        for a in &plan.doc.attachments {
            crate::attachments::write_file(&dir, ctx.profile, &a.sha, &a.bytes)?;
        }
    }
    apply(store, |w| {
        let summary = write_json(w, ctx, &plan, new_id)?;
        for a in &plan.doc.attachments {
            w.ensure_attachment(
                new_id(),
                ctx.profile,
                a.sha,
                a.mime,
                a.bytes.len() as u64,
                &a.file_name,
                ctx.now,
            )?;
        }
        if !plan.doc.attachments.is_empty() {
            for &snippet in plan.snippets.values() {
                w.sync_snippet_attachments(snippet, new_id, ctx.now)?;
            }
        }
        Ok(summary)
    })
}

fn parse_json(doc: &Value) -> Result<JsonDoc, TransferError> {
    let obj = doc
        .as_object()
        .ok_or_else(|| invalid("document", "expected a JSON object"))?;
    let format = obj.get("format").cloned().unwrap_or(Value::Null);
    if format.as_str() != Some(JSON_FORMAT) {
        return Err(TransferError::WrongFormat {
            found: describe(&format),
        });
    }
    let version = obj.get("version").cloned().unwrap_or(Value::Null);
    if version.as_u64() != Some(JSON_VERSION) {
        return Err(TransferError::UnsupportedVersion {
            found: describe(&version),
        });
    }
    Ok(JsonDoc {
        folders: rows(obj, "folders", serde_json::from_value::<Folder>)?,
        tags: rows(obj, "tags", serde_json::from_value::<Tag>)?,
        snippets: rows(obj, "snippets", serde_json::from_value::<Snippet>)?,
        links: rows(obj, "snippet_tags", serde_json::from_value::<SnippetTag>)?,
        attachments: parse_attachments(obj)?,
    })
}

fn describe(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_owned)
}

/// Reads one array of rows; a missing array is empty.
fn rows<T>(
    obj: &Map<String, Value>,
    key: &str,
    read: fn(Value) -> Result<T, serde_json::Error>,
) -> Result<Vec<T>, TransferError> {
    let Some(value) = obj.get(key) else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| invalid(key, "expected an array"))?;
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            read(item.clone()).map_err(|e| invalid(&format!("{key}[{i}]"), e.to_string()))
        })
        .collect()
}

/// A parsed JSON file with every row checked for references and given fresh ids.
struct JsonPlan {
    doc: JsonDoc,
    /// Indices into `doc.folders`, parents before children.
    folder_order: Vec<usize>,
    folders: HashMap<Id, Id>,
    tags: HashMap<Id, Id>,
    snippets: HashMap<Id, Id>,
}

impl JsonPlan {
    fn new(doc: JsonDoc, new_id: &mut dyn FnMut() -> Id) -> Result<Self, TransferError> {
        let folder_order = parent_first(&doc.folders)?;
        let folders = fresh_ids("folders", doc.folders.iter().map(|f| f.meta.id), new_id)?;
        let tags = fresh_ids("tags", doc.tags.iter().map(|t| t.meta.id), new_id)?;
        let snippets = fresh_ids("snippets", doc.snippets.iter().map(|s| s.meta.id), new_id)?;
        for (i, s) in doc.snippets.iter().enumerate() {
            if !folders.contains_key(&s.folder_id) {
                return Err(invalid(
                    &format!("snippets[{i}]"),
                    format!("folder {} is not in the file", s.folder_id),
                ));
            }
        }
        for (i, link) in doc.links.iter().enumerate() {
            if !snippets.contains_key(&link.snippet_id) {
                return Err(invalid(
                    &format!("snippet_tags[{i}]"),
                    format!("snippet {} is not in the file", link.snippet_id),
                ));
            }
            if !tags.contains_key(&link.tag_id) {
                return Err(invalid(
                    &format!("snippet_tags[{i}]"),
                    format!("tag {} is not in the file", link.tag_id),
                ));
            }
        }
        Ok(Self {
            doc,
            folder_order,
            folders,
            tags,
            snippets,
        })
    }
}

impl JsonPlan {
    /// Ids (in the file) of the folders the import has to have: those that hold a written
    /// snippet or lie above one, and, unless every snippet was skipped, branches that hold no
    /// snippet at all in the file (so an empty folder survives a round trip).
    fn needed_folders(&self, admitted: &[bool], fully_skipped: bool) -> HashSet<Id> {
        let parent: HashMap<Id, Option<Id>> = self
            .doc
            .folders
            .iter()
            .map(|f| (f.meta.id, f.parent_id))
            .collect();
        let with_ancestors = |start: Id, into: &mut HashSet<Id>| {
            let mut cur = Some(start);
            while let Some(id) = cur {
                if !into.insert(id) {
                    break;
                }
                cur = parent.get(&id).copied().flatten();
            }
        };
        let mut needed = HashSet::new();
        let mut occupied = HashSet::new();
        for (s, &ok) in self.doc.snippets.iter().zip(admitted) {
            with_ancestors(s.folder_id, &mut occupied);
            if ok {
                with_ancestors(s.folder_id, &mut needed);
            }
        }
        if !fully_skipped {
            for f in &self.doc.folders {
                if !occupied.contains(&f.meta.id) {
                    with_ancestors(f.meta.id, &mut needed);
                }
            }
        }
        needed
    }

    /// Ids (in the file) of the tags the import has to have: those linked to a written snippet
    /// and, unless every snippet was skipped, tags with no link in the file.
    fn needed_tags(&self, admitted: &[bool], fully_skipped: bool) -> HashSet<Id> {
        let ok: HashSet<Id> = self
            .doc
            .snippets
            .iter()
            .zip(admitted)
            .filter(|&(_, &ok)| ok)
            .map(|(s, _)| s.meta.id)
            .collect();
        let linked: HashSet<Id> = self.doc.links.iter().map(|l| l.tag_id).collect();
        let mut needed: HashSet<Id> = self
            .doc
            .links
            .iter()
            .filter(|l| ok.contains(&l.snippet_id))
            .map(|l| l.tag_id)
            .collect();
        if !fully_skipped {
            needed.extend(
                self.doc
                    .tags
                    .iter()
                    .map(|t| t.meta.id)
                    .filter(|id| !linked.contains(id)),
            );
        }
        needed
    }
}

/// Indices of `folders` ordered so that every folder comes after its parent.
fn parent_first(folders: &[Folder]) -> Result<Vec<usize>, TransferError> {
    let mut order = Vec::with_capacity(folders.len());
    let mut placed: HashSet<Id> = HashSet::new();
    let mut pending: Vec<usize> = (0..folders.len()).collect();
    while !pending.is_empty() {
        let before = pending.len();
        let mut still = Vec::new();
        for &i in &pending {
            let folder = &folders[i];
            if folder.parent_id.is_none_or(|p| placed.contains(&p)) {
                placed.insert(folder.meta.id);
                order.push(i);
            } else {
                still.push(i);
            }
        }
        if still.len() == before {
            let i = still[0];
            return Err(invalid(
                &format!("folders[{i}]"),
                "parent folder is not in the file, or the folders form a cycle",
            ));
        }
        pending = still;
    }
    Ok(order)
}

/// A fresh id for each old id; a repeated old id is an error.
fn fresh_ids(
    section: &str,
    old: impl Iterator<Item = Id>,
    new_id: &mut dyn FnMut() -> Id,
) -> Result<HashMap<Id, Id>, TransferError> {
    let mut map = HashMap::new();
    for id in old {
        if map.insert(id, new_id()).is_some() {
            return Err(invalid(section, format!("id {id} appears more than once")));
        }
    }
    Ok(map)
}

fn lookup(map: &HashMap<Id, Id>, old: Id, location: &str) -> Result<Id, TransferError> {
    map.get(&old)
        .copied()
        .ok_or_else(|| invalid(location, format!("id {old} is not in the file")))
}

fn write_json(
    w: &mut WriteTx<'_>,
    ctx: Ctx,
    plan: &JsonPlan,
    new_id: &mut dyn FnMut() -> Id,
) -> Result<ImportSummary, TransferError> {
    let now = ctx.now;
    let mut summary = ImportSummary::default();

    // Decide which snippets are written first, so that folders and tags are created only for
    // what the import really adds.
    let mut taken = profile_abbreviations(w, ctx.profile)?;
    let mut admitted = Vec::with_capacity(plan.doc.snippets.len());
    for (i, src) in plan.doc.snippets.iter().enumerate() {
        let location = format!("snippets[{i}]");
        let ok = admit(ctx.on_conflict, &taken, &src.abbreviation, &location)?;
        if ok {
            taken.insert(src.abbreviation.fold_key(), plan.snippets[&src.meta.id]);
        } else {
            summary.snippets_skipped += 1;
        }
        admitted.push(ok);
    }
    let fully_skipped = summary.snippets_skipped > 0 && !admitted.contains(&true);
    let needed_folders = plan.needed_folders(&admitted, fully_skipped);
    let needed_tags = plan.needed_tags(&admitted, fully_skipped);

    // Folders: reuse a live folder with the same name under the same (mapped) parent.
    let existing = Folders::load(w, ctx.profile)?;
    let mut folder_ids: HashMap<Id, Id> = HashMap::new();
    for &i in &plan.folder_order {
        let src = &plan.doc.folders[i];
        if !needed_folders.contains(&src.meta.id) {
            continue;
        }
        let location = format!("folders[{i}]");
        let parent_id = src
            .parent_id
            .map(|p| lookup(&folder_ids, p, &location))
            .transpose()?;
        if let Some(&id) = existing.by_key.get(&(parent_id, src.name.clone())) {
            folder_ids.insert(src.meta.id, id);
            continue;
        }
        let id = lookup(&plan.folders, src.meta.id, &location)?;
        let folder = Folder {
            meta: SyncMeta::new(id, now),
            profile_id: ctx.profile,
            parent_id,
            name: src.name.clone(),
            position: src.position,
            member_permission: src.member_permission,
            defaults: src.defaults.clone(),
        };
        at(w.put(&folder, now), &location)?;
        folder_ids.insert(src.meta.id, id);
        summary.folders_created += 1;
    }

    // Tags: reuse a tag with the same name.
    let live_tags = load_tags(w, ctx.profile)?;
    let mut tag_ids: HashMap<Id, Id> = HashMap::new();
    for (i, src) in plan.doc.tags.iter().enumerate() {
        if !needed_tags.contains(&src.meta.id) {
            continue;
        }
        let location = format!("tags[{i}]");
        if let Some(&id) = live_tags.get(&src.name) {
            tag_ids.insert(src.meta.id, id);
            continue;
        }
        let id = lookup(&plan.tags, src.meta.id, &location)?;
        let tag = Tag {
            meta: SyncMeta::new(id, now),
            profile_id: ctx.profile,
            name: src.name.clone(),
            color: src.color.clone(),
        };
        at(w.put(&tag, now), &location)?;
        tag_ids.insert(src.meta.id, id);
        summary.tags_created += 1;
    }

    let mut written: HashSet<Id> = HashSet::new();
    for (i, src) in plan.doc.snippets.iter().enumerate() {
        if !admitted[i] {
            continue;
        }
        let location = format!("snippets[{i}]");
        let id = lookup(&plan.snippets, src.meta.id, &location)?;
        let mut snippet = src.clone();
        snippet.meta = SyncMeta::new(id, now);
        snippet.folder_id = lookup(&folder_ids, src.folder_id, &location)?;
        at(w.put(&snippet, now), &location)?;
        written.insert(src.meta.id);
        summary.snippets_imported += 1;
    }
    for (i, link) in plan.doc.links.iter().enumerate() {
        if !written.contains(&link.snippet_id) {
            continue;
        }
        let location = format!("snippet_tags[{i}]");
        let row = SnippetTag {
            meta: SyncMeta::new(new_id(), now),
            snippet_id: lookup(&plan.snippets, link.snippet_id, &location)?,
            tag_id: lookup(&tag_ids, link.tag_id, &location)?,
        };
        at(w.put(&row, now), &location)?;
    }
    Ok(summary)
}

// ---- CSV import ---------------------------------------------------------------------------

/// One validated CSV row.
#[derive(Debug)]
struct CsvRow {
    location: String,
    abbreviation: Abbreviation,
    label: String,
    body: String,
    folder: Vec<String>,
    tags: Vec<String>,
    trigger: Option<Trigger>,
    case_mode: Option<CaseMode>,
    kind: Option<SnippetKind>,
    word_start: Option<bool>,
    enabled: Option<bool>,
}

/// Column positions in the header; `None` for an optional column the file does not have.
struct Columns {
    abbreviation: usize,
    body: usize,
    label: Option<usize>,
    folder: Option<usize>,
    tags: Option<usize>,
    trigger: Option<usize>,
    case_mode: Option<usize>,
    kind: Option<usize>,
    word_start: Option<usize>,
    enabled: Option<usize>,
}

fn import_csv(
    store: &mut Store,
    ctx: Ctx,
    data: &[u8],
    new_id: &mut dyn FnMut() -> Id,
) -> Result<ImportSummary, TransferError> {
    let rows = parse_csv(data)?;
    apply(store, |w| write_csv(w, ctx, &rows, new_id))
}

/// Fails if a quoted field is never closed. The `csv` crate reads such a field to the end of the
/// file without an error, so the quotes are checked here first.
fn check_quotes(data: &[u8]) -> Result<(), TransferError> {
    let mut in_quotes = false;
    let mut at_field_start = true;
    let mut line: u64 = 1;
    let mut quote_line: u64 = 1;
    let mut i = 0;
    while let Some(&b) = data.get(i) {
        i += 1;
        if in_quotes {
            match b {
                b'"' if data.get(i) == Some(&b'"') => i += 1,
                b'"' => in_quotes = false,
                b'\n' => line += 1,
                _ => {}
            }
        } else {
            match b {
                b'"' if at_field_start => {
                    in_quotes = true;
                    quote_line = line;
                    at_field_start = false;
                }
                b',' => at_field_start = true,
                b'\n' => {
                    line += 1;
                    at_field_start = true;
                }
                _ => at_field_start = false,
            }
        }
    }
    if in_quotes {
        return Err(TransferError::Csv {
            line: Some(quote_line),
            message: "a quoted field is never closed".to_owned(),
        });
    }
    Ok(())
}

fn parse_csv(data: &[u8]) -> Result<Vec<CsvRow>, TransferError> {
    check_quotes(data)?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(data);
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| csv_error(&e))?
        .iter()
        .map(|h| h.trim().to_ascii_lowercase())
        .collect();
    let find = |name: &str| headers.iter().position(|h| h == name);
    let required =
        |name: &'static str| find(name).ok_or(TransferError::MissingColumn { column: name });
    let cols = Columns {
        abbreviation: required("abbreviation")?,
        body: required("body")?,
        label: find("label"),
        folder: find("folder"),
        tags: find("tags"),
        trigger: find("trigger"),
        case_mode: find("case_mode"),
        kind: find("kind"),
        word_start: find("word_start"),
        enabled: find("enabled"),
    };
    let mut rows = Vec::new();
    for (n, record) in reader.records().enumerate() {
        let record = record.map_err(|e| csv_error(&e))?;
        let location = match record.position().map(csv::Position::line) {
            Some(line) => format!("row {} (line {line})", n + 1),
            None => format!("row {}", n + 1),
        };
        rows.push(parse_row(&record, &cols, &location)?);
    }
    Ok(rows)
}

/// The text of a cell; an empty string when the column is missing.
fn cell(record: &csv::StringRecord, col: Option<usize>) -> &str {
    col.and_then(|i| record.get(i)).unwrap_or("")
}

fn parse_row(
    record: &csv::StringRecord,
    cols: &Columns,
    location: &str,
) -> Result<CsvRow, TransferError> {
    let abbreviation = Abbreviation::new(cell(record, Some(cols.abbreviation)))
        .map_err(|e| invalid(location, format!("abbreviation: {e}")))?;
    Ok(CsvRow {
        location: location.to_owned(),
        abbreviation,
        label: cell(record, cols.label).to_owned(),
        body: cell(record, Some(cols.body)).to_owned(),
        folder: folder_segments(cell(record, cols.folder), location)?,
        tags: tag_names(cell(record, cols.tags)),
        trigger: choice(
            cell(record, cols.trigger),
            location,
            "trigger",
            &TRIGGER_CHOICES,
        )?,
        case_mode: choice(
            cell(record, cols.case_mode),
            location,
            "case_mode",
            &CASE_CHOICES,
        )?,
        kind: choice(cell(record, cols.kind), location, "kind", &KIND_CHOICES)?,
        word_start: choice(
            cell(record, cols.word_start),
            location,
            "word_start",
            &BOOL_CHOICES,
        )?,
        enabled: choice(
            cell(record, cols.enabled),
            location,
            "enabled",
            &BOOL_CHOICES,
        )?,
    })
}

/// The folder path in a `folder` cell; an empty cell means [`DEFAULT_FOLDER`].
fn folder_segments(text: &str, location: &str) -> Result<Vec<String>, TransferError> {
    if text.trim().is_empty() {
        return Ok(vec![DEFAULT_FOLDER.to_owned()]);
    }
    let segments = split_escaped(text, FOLDER_SEPARATOR);
    if segments.iter().any(String::is_empty) {
        return Err(invalid(location, "folder path has an empty folder name"));
    }
    if segments.len() > MAX_FOLDER_DEPTH {
        return Err(TransferError::FolderTooDeep {
            location: location.to_owned(),
            depth: segments.len(),
        });
    }
    Ok(segments)
}

/// The distinct tag names in a `tags` cell, trimmed; empty names are dropped.
fn tag_names(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in split_escaped(text, &TAG_SEPARATOR.to_string()) {
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Matches an enum word, case-insensitively. An empty cell is `None` (use the default).
fn choice<T: Copy>(
    text: &str,
    location: &str,
    column: &str,
    options: &[(&str, T)],
) -> Result<Option<T>, TransferError> {
    let wanted = text.trim().to_ascii_lowercase();
    if wanted.is_empty() {
        return Ok(None);
    }
    if let Some((_, value)) = options.iter().find(|(name, _)| *name == wanted) {
        return Ok(Some(*value));
    }
    let allowed: Vec<&str> = options.iter().map(|(name, _)| *name).collect();
    Err(invalid(
        location,
        format!(
            "{column} must be one of {} (found {text:?})",
            allowed.join(", ")
        ),
    ))
}

/// Folders created or reused during one CSV import, keyed by parent and name.
#[derive(Default)]
struct Folders {
    by_key: HashMap<(Option<Id>, String), Id>,
    defaults: HashMap<Id, FolderDefaults>,
}

impl Folders {
    fn load(w: &WriteTx<'_>, profile: Id) -> Result<Self, StoreError> {
        let mut out = Self::default();
        for f in w.list::<Folder>(ListOptions::LIVE)? {
            if f.profile_id == profile {
                out.by_key.insert((f.parent_id, f.name.clone()), f.meta.id);
                out.defaults.insert(f.meta.id, f.defaults);
            }
        }
        Ok(out)
    }

    /// The id of the folder at the end of `path`, creating missing folders on the way.
    fn ensure_path(
        &mut self,
        w: &mut WriteTx<'_>,
        ctx: Ctx,
        new_id: &mut dyn FnMut() -> Id,
        path: &[String],
        location: &str,
        summary: &mut ImportSummary,
    ) -> Result<Id, TransferError> {
        let mut parent = None;
        let mut leaf = None;
        for name in path {
            let key = (parent, name.clone());
            let existing = self.by_key.get(&key).copied();
            let id = if let Some(id) = existing {
                id
            } else {
                let id = new_id();
                let folder = Folder {
                    meta: SyncMeta::new(id, ctx.now),
                    profile_id: ctx.profile,
                    parent_id: parent,
                    name: name.clone(),
                    position: 0,
                    member_permission: None,
                    defaults: FolderDefaults::default(),
                };
                at(w.put(&folder, ctx.now), location)?;
                summary.folders_created += 1;
                self.by_key.insert(key, id);
                self.defaults.insert(id, FolderDefaults::default());
                id
            };
            parent = Some(id);
            leaf = Some(id);
        }
        leaf.ok_or_else(|| invalid(location, "folder path is empty"))
    }
}

fn load_tags(w: &WriteTx<'_>, profile: Id) -> Result<HashMap<String, Id>, StoreError> {
    Ok(w.list::<Tag>(ListOptions::LIVE)?
        .into_iter()
        .filter(|t| t.profile_id == profile)
        .map(|t| (t.name, t.meta.id))
        .collect())
}

fn ensure_tag(
    w: &mut WriteTx<'_>,
    ctx: Ctx,
    tags: &mut HashMap<String, Id>,
    name: &str,
    location: &str,
    new_id: &mut dyn FnMut() -> Id,
    summary: &mut ImportSummary,
) -> Result<Id, TransferError> {
    if let Some(id) = tags.get(name).copied() {
        return Ok(id);
    }
    let id = new_id();
    let tag = Tag {
        meta: SyncMeta::new(id, ctx.now),
        profile_id: ctx.profile,
        name: name.to_owned(),
        color: None,
    };
    at(w.put(&tag, ctx.now), location)?;
    tags.insert(name.to_owned(), id);
    summary.tags_created += 1;
    Ok(id)
}

/// Applies the optional columns on top of [`Snippet::new_defaults`]. A `trigger` cell also sets
/// `word_start` to match, unless the file gives `word_start` too.
fn apply_overrides(snippet: &mut Snippet, row: &CsvRow) {
    if let Some(trigger) = row.trigger {
        snippet.trigger = trigger;
        snippet.word_start = trigger == Trigger::Delimiter;
    }
    if let Some(case_mode) = row.case_mode {
        snippet.case_mode = case_mode;
    }
    if let Some(kind) = row.kind {
        snippet.kind = kind;
    }
    if let Some(word_start) = row.word_start {
        snippet.word_start = word_start;
    }
    if let Some(enabled) = row.enabled {
        snippet.enabled = enabled;
    }
}

fn write_csv(
    w: &mut WriteTx<'_>,
    ctx: Ctx,
    rows: &[CsvRow],
    new_id: &mut dyn FnMut() -> Id,
) -> Result<ImportSummary, TransferError> {
    let mut summary = ImportSummary::default();
    let mut folders = Folders::load(w, ctx.profile)?;
    let mut tags = load_tags(w, ctx.profile)?;
    let mut taken = profile_abbreviations(w, ctx.profile)?;
    for row in rows {
        // Decide before creating anything for the row, so a skipped row leaves no folders behind.
        if !admit(ctx.on_conflict, &taken, &row.abbreviation, &row.location)? {
            summary.snippets_skipped += 1;
            continue;
        }
        let folder_id =
            folders.ensure_path(w, ctx, new_id, &row.folder, &row.location, &mut summary)?;
        let id = new_id();
        let mut snippet = Snippet::new_defaults(
            SyncMeta::new(id, ctx.now),
            folder_id,
            row.abbreviation.clone(),
            row.label.clone(),
            row.body.clone(),
            folders.defaults.get(&folder_id),
        );
        apply_overrides(&mut snippet, row);
        at(w.put(&snippet, ctx.now), &row.location)?;
        taken.insert(row.abbreviation.fold_key(), id);
        summary.snippets_imported += 1;
        for name in &row.tags {
            let tag_id = ensure_tag(w, ctx, &mut tags, name, &row.location, new_id, &mut summary)?;
            let link = SnippetTag {
                meta: SyncMeta::new(new_id(), ctx.now),
                snippet_id: id,
                tag_id,
            };
            at(w.put(&link, ctx.now), &row.location)?;
        }
    }
    Ok(summary)
}
