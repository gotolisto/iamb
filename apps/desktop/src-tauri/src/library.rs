//! The library window's side of the contract (task 3.3): folders, tags, snippet summaries, and
//! saving and deleting rows.
//!
//! The front end never speaks IPC (architecture §6.2). It calls the `library_*` commands in
//! `app/library_commands.rs`, which call the functions here; the types are mirrored by hand in
//! `apps/desktop/src/library/api.ts`. Everything here is library content (labels, abbreviations,
//! bodies the user saved), never typed text outside a snippet. The log of the UI host records
//! none of it (architecture §8 rule 3): these functions do not log at all.
//!
//! | Command | Request sent | Response read |
//! | --- | --- | --- |
//! | `library_list_folders` | `ListFolders` | `Folders` |
//! | `library_list_tags` | `ListTags` | `Tags` |
//! | `library_list_summaries` | `ListSnippetSummaries` | `SnippetSummaries` |
//! | `library_get_snippet` | `GetSnippet` | `Snippet` |
//! | `library_save_snippet` | `PutSnippet` (with `tags`) | `Saved` |
//! | `library_delete_snippet` | `DeleteSnippet` | `Done` |
//! | `library_set_enabled` | `GetSnippet`, `PutSnippet` (tags untouched) | `Saved` |
//! | `library_save_folder` | `ListFolders`, `PutFolder` | `Saved` |
//! | `library_delete_folder` | `DeleteFolder` | `Done` |
//! | `library_save_tag` | `ListFolders` (for the profile), `ListTags`, `PutTag` | `Saved` |
//! | `library_delete_tag` | `DeleteTag` | `Done` |
//! | `library_check_abbreviation` | `CheckAbbreviation` | `AbbreviationCheck` |
//! | `library_health` | `Health` | `Health` |
//! | `library_reset_usage` | `ResetUsage` | `Done` |
//!
//! **Undo** of a delete is `library_save_snippet` with the deleted snippet's id: the service
//! restores the row with its usage and tags. The store keeps `created_at` and `rev` of an
//! existing row and takes only `updated_by` from the row sent, so the ids and the content are
//! what matter in a draft.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use iamb_core::model::{
    CaseMode, Folder, FolderDefaults, Id, Snippet, SnippetKind, SyncMeta, Tag, Timestamp, Trigger,
};
use iamb_ipc::{RequestBody, ResponseBody, SnippetQuery};
use serde::{Deserialize, Serialize};

use crate::bridge::{Bridge, command};
use crate::contract::{CommandError, Unexpected, parse_id};

/// The requests the library functions send, by wire name.
pub const REQUESTS: &[&str] = &[
    "list_folders",
    "list_tags",
    "list_snippet_summaries",
    "get_snippet",
    "put_snippet",
    "delete_snippet",
    "put_folder",
    "delete_folder",
    "put_tag",
    "delete_tag",
    "check_abbreviation",
    "health",
    "reset_usage",
];

/// The most summaries one page holds (the service's limit).
pub const PAGE_LIMIT: u32 = 1000;

type Res<T> = Result<T, CommandError>;

fn invalid(message: &str) -> CommandError {
    CommandError::new("invalid", message)
}

// ---------------------------------------------------------------------------------------------
// What the front end receives
// ---------------------------------------------------------------------------------------------

/// A folder's defaults for new snippets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsView {
    /// Text put in front of a new snippet's abbreviation.
    pub abbreviation_prefix: Option<String>,
    /// `immediate` or `delimiter`.
    pub trigger: Option<String>,
    /// `exact`, `insensitive` or `adaptive`.
    pub case_mode: Option<String>,
}

/// A folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderView {
    /// Its id.
    pub id: String,
    /// The parent folder, if nested.
    pub parent_id: Option<String>,
    /// The name.
    pub name: String,
    /// Position among its siblings.
    pub position: i64,
    /// Defaults for new snippets.
    pub defaults: DefaultsView,
}

/// The personal profile's folders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoldersView {
    /// The personal profile's id.
    pub profile: String,
    /// The live folders.
    pub folders: Vec<FolderView>,
}

/// A tag with its count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagView {
    /// Its id.
    pub id: String,
    /// The name.
    pub name: String,
    /// One of the eight colour names.
    pub color: String,
    /// How many snippets carry it.
    pub snippets: u64,
}

/// One row of the snippet list: no body beyond its first line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryView {
    /// Its id.
    pub id: String,
    /// Its folder.
    pub folder_id: String,
    /// What the user types.
    pub abbreviation: String,
    /// The label (may be empty).
    pub label: String,
    /// The first line of the content.
    pub first_line: String,
    /// `plain` or `rich`.
    pub kind: String,
    /// `immediate` or `delimiter`.
    pub trigger: String,
    /// Whether it expands.
    pub enabled: bool,
    /// Its tags.
    pub tags: Vec<String>,
    /// Expanding it asks for values: a `{field}` or `{if}` anywhere in it, in an included
    /// snippet too (the first line cannot tell).
    pub has_fields: bool,
    /// Why it does not expand although enabled: `duplicate` or an `E_…` code.
    pub broken: Option<String>,
    /// How many times it expanded on this computer; `None` when the service does not count.
    pub uses: Option<u64>,
    /// When it last expanded on this computer (UTC milliseconds); `None` when it has not.
    pub last_used: Option<i64>,
    /// When it last changed (UTC milliseconds).
    pub updated_at: i64,
}

/// One page of summaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryPage {
    /// The library revision the page was read from.
    pub revision: u64,
    /// How many snippets there are in all.
    pub total: u64,
    /// This page.
    pub summaries: Vec<SummaryView>,
    /// The offset of the next page; `None` on the last.
    pub next: Option<u32>,
}

/// An immediate abbreviation that hides a longer one ("Never reached").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowedView {
    /// The immediate snippet.
    pub by: String,
    /// The snippet that cannot be typed to its end.
    pub hidden: String,
}

/// A snippet that will not expand although it is on ("Will not expand").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedView {
    /// The snippet.
    pub snippet: String,
    /// `duplicate` or an `E_…` code.
    pub reason: String,
    /// For `duplicate`: the snippet that holds the abbreviation.
    pub kept: Option<String>,
    /// For a broken reference: the abbreviation the failing `{snippet}` names.
    pub reference: Option<String>,
    /// The abbreviation of the included snippet whose content holds the error.
    pub included: Option<String>,
}

/// A snippet not used on this computer for 90 days.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnusedView {
    /// The snippet.
    pub snippet: String,
    /// When it last expanded (UTC milliseconds); `None`: never since it was created.
    pub last_used: Option<i64>,
}

/// Library health: one list per section of spec §4.9.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    /// "Never reached".
    pub shadowed: Vec<ShadowedView>,
    /// "Will not expand".
    pub excluded: Vec<ExcludedView>,
    /// "Not used on this computer for 90 days".
    pub unused: Vec<UnusedView>,
}

/// A whole snippet, for the editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetFull {
    /// Its id.
    pub id: String,
    /// Its folder.
    pub folder_id: String,
    /// What the user types.
    pub abbreviation: String,
    /// The label (may be empty).
    pub label: String,
    /// `plain` or `rich`.
    pub kind: String,
    /// The content (template source).
    pub body: String,
    /// `immediate` or `delimiter`.
    pub trigger: String,
    /// `exact`, `insensitive` or `adaptive`.
    pub case_mode: String,
    /// Only at the start of a word.
    pub word_start: bool,
    /// Whether it expands.
    pub enabled: bool,
    /// Its tags.
    pub tags: Vec<String>,
    /// The library revision of the row.
    pub rev: u64,
    /// When it was created (UTC milliseconds).
    pub created_at: i64,
    /// When it last changed (UTC milliseconds).
    pub updated_at: i64,
}

/// What saving a row answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedView {
    /// The row's id (new for a new row).
    pub id: String,
    /// Its revision.
    pub rev: u64,
    /// When it was written (UTC milliseconds).
    pub updated_at: i64,
}

/// What an abbreviation would run into.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckView {
    /// The snippet that already has it: saving is refused.
    pub conflict: Option<String>,
    /// Snippets it would hide (it is immediate and theirs start with it).
    pub hides: Vec<String>,
    /// Snippets that would hide it.
    pub hidden_by: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// What the front end sends
// ---------------------------------------------------------------------------------------------

/// A snippet to write. With an `id` it replaces that row (or restores a deleted one); without,
/// it is new.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetDraft {
    /// The row to replace or restore; absent for a new snippet.
    #[serde(default)]
    pub id: Option<String>,
    /// Its folder.
    pub folder_id: String,
    /// What the user types.
    pub abbreviation: String,
    /// The label.
    pub label: String,
    /// `plain` or `rich`.
    pub kind: String,
    /// The content.
    pub body: String,
    /// `immediate` or `delimiter`.
    pub trigger: String,
    /// `exact`, `insensitive` or `adaptive`.
    pub case_mode: String,
    /// Only at the start of a word.
    pub word_start: bool,
    /// Whether it expands.
    pub enabled: bool,
    /// All its tags.
    pub tags: Vec<String>,
}

/// A folder to write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderDraft {
    /// The row to replace; absent for a new folder.
    #[serde(default)]
    pub id: Option<String>,
    /// The parent folder.
    pub parent_id: Option<String>,
    /// The name.
    pub name: String,
    /// Position among its siblings.
    pub position: i64,
    /// Defaults for new snippets.
    #[serde(default)]
    pub defaults: DefaultsView,
}

/// A tag to write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagDraft {
    /// The row to replace; absent for a new tag.
    #[serde(default)]
    pub id: Option<String>,
    /// The name.
    pub name: String,
    /// One of the eight colour names.
    pub color: String,
}

// ---------------------------------------------------------------------------------------------
// Words on the wire
// ---------------------------------------------------------------------------------------------

const fn trigger_name(t: Trigger) -> &'static str {
    match t {
        Trigger::Immediate => "immediate",
        Trigger::Delimiter => "delimiter",
    }
}

const fn case_name(c: CaseMode) -> &'static str {
    match c {
        CaseMode::Exact => "exact",
        CaseMode::Insensitive => "insensitive",
        CaseMode::Adaptive => "adaptive",
    }
}

const fn kind_name(k: SnippetKind) -> &'static str {
    match k {
        SnippetKind::Plain => "plain",
        SnippetKind::Rich => "rich",
    }
}

fn parse_trigger(s: &str) -> Res<Trigger> {
    match s {
        "immediate" => Ok(Trigger::Immediate),
        "delimiter" => Ok(Trigger::Delimiter),
        _ => Err(invalid("the trigger must be immediate or delimiter")),
    }
}

fn parse_case(s: &str) -> Res<CaseMode> {
    match s {
        "exact" => Ok(CaseMode::Exact),
        "insensitive" => Ok(CaseMode::Insensitive),
        "adaptive" => Ok(CaseMode::Adaptive),
        _ => Err(invalid(
            "the case mode must be exact, insensitive or adaptive",
        )),
    }
}

fn parse_kind(s: &str) -> Res<SnippetKind> {
    match s {
        "plain" => Ok(SnippetKind::Plain),
        "rich" => Ok(SnippetKind::Rich),
        _ => Err(invalid("the kind must be plain or rich")),
    }
}

fn ids(list: &[String]) -> Res<Vec<Id>> {
    list.iter().map(|s| parse_id(s)).collect()
}

fn texts(list: &[Id]) -> Vec<String> {
    list.iter().map(ToString::to_string).collect()
}

fn now() -> Timestamp {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    Timestamp(ms)
}

/// A fresh id: UUID version 7 from the clock and ten bytes of `RandomState` (seeded by the
/// operating system), mixed with a counter so two ids in one millisecond differ.
fn new_id() -> Id {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let state = RandomState::new();
    let a = state.hash_one(n).to_le_bytes();
    let b = state.hash_one(n ^ 0x9E37_79B9_7F4A_7C15).to_le_bytes();
    let mut random = [0_u8; 10];
    random[..8].copy_from_slice(&a);
    random[8..].copy_from_slice(&b[..2]);
    Id::v7(u64::try_from(now().0).unwrap_or(0), random)
}

// ---------------------------------------------------------------------------------------------
// Requests and the mapping of responses (pure; `tests/library_contract.rs` runs them against
// the golden files)
// ---------------------------------------------------------------------------------------------

/// `ListFolders`.
#[must_use]
pub const fn list_folders() -> RequestBody {
    RequestBody::ListFolders {}
}

/// `ListTags`.
#[must_use]
pub const fn list_tags() -> RequestBody {
    RequestBody::ListTags {}
}

/// `ListSnippetSummaries` over the whole library.
#[must_use]
pub fn list_summaries(offset: u32, limit: u32) -> RequestBody {
    RequestBody::ListSnippetSummaries {
        query: SnippetQuery::default(),
        offset,
        limit: limit.clamp(1, PAGE_LIMIT),
    }
}

/// `Health`.
#[must_use]
pub const fn health_request() -> RequestBody {
    RequestBody::Health {}
}

/// `ResetUsage`.
#[must_use]
pub const fn reset_usage_request() -> RequestBody {
    RequestBody::ResetUsage {}
}

/// `GetSnippet`.
#[must_use]
pub const fn get_snippet(id: Id) -> RequestBody {
    RequestBody::GetSnippet { id }
}

/// `DeleteSnippet`.
#[must_use]
pub const fn delete_snippet(id: Id) -> RequestBody {
    RequestBody::DeleteSnippet { id }
}

/// `DeleteFolder`.
#[must_use]
pub const fn delete_folder(id: Id) -> RequestBody {
    RequestBody::DeleteFolder { id }
}

/// `DeleteTag`.
#[must_use]
pub const fn delete_tag(id: Id) -> RequestBody {
    RequestBody::DeleteTag { id }
}

/// `CheckAbbreviation`.
///
/// # Errors
///
/// `invalid` for a trigger that is not `immediate` or `delimiter`, or an id that is not one.
pub fn check_abbreviation(
    abbreviation: &str,
    trigger: &str,
    snippet: Option<&str>,
) -> Res<RequestBody> {
    parse_trigger(trigger)?;
    Ok(RequestBody::CheckAbbreviation {
        abbreviation: abbreviation.to_owned(),
        trigger: trigger.to_owned(),
        snippet: snippet.map(parse_id).transpose()?,
    })
}

/// The `PutSnippet` of a draft, with its tags; `existing` is the row it replaces, if any.
///
/// # Errors
///
/// `invalid` for an abbreviation the model refuses, an unknown trigger, case mode or kind, or
/// an id that is not one.
pub fn put_snippet(draft: &SnippetDraft) -> Res<RequestBody> {
    let id = match &draft.id {
        Some(id) => parse_id(id)?,
        None => new_id(),
    };
    let abbreviation = iamb_core::model::Abbreviation::new(&draft.abbreviation)
        .map_err(|e| invalid(&e.to_string()))?;
    let snippet = Snippet {
        meta: SyncMeta::new(id, now()),
        folder_id: parse_id(&draft.folder_id)?,
        abbreviation,
        label: draft.label.clone(),
        kind: parse_kind(&draft.kind)?,
        body: draft.body.clone(),
        trigger: parse_trigger(&draft.trigger)?,
        case_mode: parse_case(&draft.case_mode)?,
        word_start: draft.word_start,
        enabled: draft.enabled,
    };
    Ok(RequestBody::PutSnippet {
        snippet,
        tags: Some(ids(&draft.tags)?),
    })
}

fn folder_from(f: &Folder) -> FolderView {
    FolderView {
        id: f.meta.id.to_string(),
        parent_id: f.parent_id.map(|p| p.to_string()),
        name: f.name.clone(),
        position: f.position,
        defaults: DefaultsView {
            abbreviation_prefix: f.defaults.abbreviation_prefix.clone(),
            trigger: f.defaults.trigger.map(|t| trigger_name(t).to_owned()),
            case_mode: f.defaults.case_mode.map(|c| case_name(c).to_owned()),
        },
    }
}

/// The folders in a `Folders` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn folders_from(response: ResponseBody) -> Result<FoldersView, Unexpected> {
    match response {
        ResponseBody::Folders { profile, folders } => Ok(FoldersView {
            profile: profile.to_string(),
            folders: folders.iter().map(folder_from).collect(),
        }),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The tags in a `Tags` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn tags_from(response: ResponseBody) -> Result<Vec<TagView>, Unexpected> {
    match response {
        ResponseBody::Tags { tags } => Ok(tags
            .iter()
            .map(|t| TagView {
                id: t.tag.meta.id.to_string(),
                name: t.tag.name.clone(),
                color: t.tag.color.clone().unwrap_or_else(|| "grey".to_owned()),
                snippets: t.snippets,
            })
            .collect()),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The page in a `SnippetSummaries` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn summaries_from(response: ResponseBody) -> Result<SummaryPage, Unexpected> {
    match response {
        ResponseBody::SnippetSummaries {
            revision,
            total,
            summaries,
            next,
        } => Ok(SummaryPage {
            revision,
            total,
            next,
            summaries: summaries
                .iter()
                .map(|s| SummaryView {
                    id: s.id.to_string(),
                    folder_id: s.folder.to_string(),
                    abbreviation: s.abbreviation.clone(),
                    label: s.label.clone(),
                    first_line: s.first_line.clone(),
                    kind: s.kind.clone(),
                    trigger: s.trigger.clone(),
                    enabled: s.enabled,
                    tags: texts(&s.tags),
                    has_fields: s.has_fields,
                    broken: s.broken.clone(),
                    uses: s.uses,
                    last_used: s.last_used.map(|t| t.0),
                    updated_at: s.updated_at.0,
                })
                .collect(),
        }),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The snippet in a `Snippet` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn full_from(response: ResponseBody) -> Result<SnippetFull, Unexpected> {
    match response {
        ResponseBody::Snippet { snippet: s, tags } => Ok(SnippetFull {
            id: s.meta.id.to_string(),
            folder_id: s.folder_id.to_string(),
            abbreviation: s.abbreviation.as_str().to_owned(),
            label: s.label.clone(),
            kind: kind_name(s.kind).to_owned(),
            body: s.body.clone(),
            trigger: trigger_name(s.trigger).to_owned(),
            case_mode: case_name(s.case_mode).to_owned(),
            word_start: s.word_start,
            enabled: s.enabled,
            tags: texts(&tags),
            rev: s.meta.rev,
            created_at: s.meta.created_at.0,
            updated_at: s.meta.updated_at.0,
        }),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The result of a `Saved` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn saved_from(response: ResponseBody) -> Result<SavedView, Unexpected> {
    match response {
        ResponseBody::Saved { meta } => Ok(SavedView {
            id: meta.id.to_string(),
            rev: meta.rev,
            updated_at: meta.updated_at.0,
        }),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// A `Done` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn done_from(response: ResponseBody) -> Result<(), Unexpected> {
    match response {
        ResponseBody::Done {} => Ok(()),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The check in an `AbbreviationCheck` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn check_from(response: ResponseBody) -> Result<CheckView, Unexpected> {
    match response {
        ResponseBody::AbbreviationCheck {
            conflict,
            hides,
            hidden_by,
        } => Ok(CheckView {
            conflict: conflict.map(|c| c.to_string()),
            hides: texts(&hides),
            hidden_by: texts(&hidden_by),
        }),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The report in a `Health` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn health_from(response: ResponseBody) -> Result<HealthView, Unexpected> {
    match response {
        ResponseBody::Health { report } => Ok(HealthView {
            shadowed: report
                .shadowed
                .iter()
                .map(|s| ShadowedView {
                    by: s.by.to_string(),
                    hidden: s.hidden.to_string(),
                })
                .collect(),
            excluded: report
                .excluded
                .iter()
                .map(|e| ExcludedView {
                    snippet: e.snippet.to_string(),
                    reason: e.reason.clone(),
                    kept: e.kept.map(|k| k.to_string()),
                    reference: e.reference.clone(),
                    included: e.included.clone(),
                })
                .collect(),
            unused: report
                .unused
                .iter()
                .map(|u| UnusedView {
                    snippet: u.snippet.to_string(),
                    last_used: u.last_used.map(|t| t.0),
                })
                .collect(),
        }),
        other => Err(Unexpected { got: other.name() }),
    }
}

// ---------------------------------------------------------------------------------------------
// The operations (what the commands run, and the end-to-end tests)
// ---------------------------------------------------------------------------------------------

/// The service answered `request` with a response of another type.
fn unexpected(request: &'static str) -> CommandError {
    CommandError::new(
        "unexpected_response",
        format!("the service's answer to {request} was not the one expected"),
    )
}

fn call(bridge: &Bridge, body: RequestBody) -> Res<ResponseBody> {
    bridge.call(body).map_err(command)
}

/// The personal profile's folders.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn folders(bridge: &Bridge) -> Res<FoldersView> {
    Ok(folders_from(call(bridge, list_folders())?)?)
}

/// The tags with their counts.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn tags(bridge: &Bridge) -> Res<Vec<TagView>> {
    Ok(tags_from(call(bridge, list_tags())?)?)
}

/// One page of summaries.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn summaries(bridge: &Bridge, offset: u32, limit: u32) -> Res<SummaryPage> {
    Ok(summaries_from(call(
        bridge,
        list_summaries(offset, limit),
    )?)?)
}

/// Library health.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn health(bridge: &Bridge) -> Res<HealthView> {
    Ok(health_from(call(bridge, health_request())?)?)
}

/// Forgets the usage counts of this computer.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn reset_usage(bridge: &Bridge) -> Res<()> {
    Ok(done_from(call(bridge, reset_usage_request())?)?)
}

/// One snippet with its tags.
///
/// # Errors
///
/// The call's errors (`not_found`), or an unexpected response.
pub fn snippet(bridge: &Bridge, id: &str) -> Res<SnippetFull> {
    Ok(full_from(call(bridge, get_snippet(parse_id(id)?))?)?)
}

/// Writes a snippet: creates it, replaces it, or restores a deleted one (Undo).
///
/// # Errors
///
/// `invalid` for a draft the model refuses, `conflict` when its abbreviation belongs to another
/// snippet, the call's other errors, or an unexpected response.
pub fn save_snippet(bridge: &Bridge, draft: &SnippetDraft) -> Res<SavedView> {
    Ok(saved_from(call(bridge, put_snippet(draft)?)?)?)
}

/// Deletes a snippet (the service keeps the row, so a save with its id restores it).
///
/// # Errors
///
/// `not_found` when it does not exist, the call's other errors, or an unexpected response.
pub fn delete_snippet_by_id(bridge: &Bridge, id: &str) -> Res<()> {
    Ok(done_from(call(bridge, delete_snippet(parse_id(id)?))?)?)
}

/// Turns a snippet on or off, leaving everything else, its tags included, as it is.
///
/// # Errors
///
/// As [`snippet`] and [`save_snippet`].
pub fn set_enabled(bridge: &Bridge, id: &str, enabled: bool) -> Res<SavedView> {
    let ResponseBody::Snippet { mut snippet, .. } = call(bridge, get_snippet(parse_id(id)?))?
    else {
        return Err(unexpected("get_snippet"));
    };
    snippet.enabled = enabled;
    Ok(saved_from(call(
        bridge,
        RequestBody::PutSnippet {
            snippet,
            tags: None,
        },
    )?)?)
}

/// Writes a folder.
///
/// # Errors
///
/// `invalid` for a draft the model or the store refuses (a cycle, more than four levels), the
/// call's other errors, or an unexpected response.
pub fn save_folder(bridge: &Bridge, draft: &FolderDraft) -> Res<SavedView> {
    let ResponseBody::Folders { profile, folders } = call(bridge, list_folders())? else {
        return Err(unexpected("list_folders"));
    };
    let name = draft.name.trim();
    if name.is_empty() {
        return Err(invalid("A folder needs a name."));
    }
    let id = draft.id.as_deref().map(parse_id).transpose()?;
    let meta = id
        .and_then(|id| folders.iter().find(|f| f.meta.id == id))
        .map_or_else(
            || SyncMeta::new(id.unwrap_or_else(new_id), now()),
            |f| f.meta.clone(),
        );
    let folder = Folder {
        meta,
        profile_id: profile,
        parent_id: draft.parent_id.as_deref().map(parse_id).transpose()?,
        name: name.to_owned(),
        position: draft.position,
        member_permission: None,
        defaults: FolderDefaults {
            trigger: draft
                .defaults
                .trigger
                .as_deref()
                .map(parse_trigger)
                .transpose()?,
            case_mode: draft
                .defaults
                .case_mode
                .as_deref()
                .map(parse_case)
                .transpose()?,
            abbreviation_prefix: draft
                .defaults
                .abbreviation_prefix
                .clone()
                .filter(|p| !p.is_empty()),
        },
    };
    Ok(saved_from(call(
        bridge,
        RequestBody::PutFolder { folder },
    )?)?)
}

/// Deletes a folder with its snippets (the service decides what happens to subfolders).
///
/// # Errors
///
/// `not_found`, the call's other errors, or an unexpected response.
pub fn delete_folder_by_id(bridge: &Bridge, id: &str) -> Res<()> {
    Ok(done_from(call(bridge, delete_folder(parse_id(id)?))?)?)
}

/// Writes a tag (creates, renames or recolours it).
///
/// # Errors
///
/// `invalid` for an empty name or a colour that is not one of the eight, `conflict` for a name
/// another tag has, the call's other errors, or an unexpected response.
pub fn save_tag(bridge: &Bridge, draft: &TagDraft) -> Res<SavedView> {
    let ResponseBody::Folders { profile, .. } = call(bridge, list_folders())? else {
        return Err(unexpected("list_folders"));
    };
    let id = draft.id.as_deref().map(parse_id).transpose()?;
    let existing = match id {
        Some(id) => {
            let ResponseBody::Tags { tags } = call(bridge, list_tags())? else {
                return Err(unexpected("list_tags"));
            };
            tags.into_iter().find(|t| t.tag.meta.id == id)
        }
        None => None,
    };
    let meta = existing.map_or_else(
        || SyncMeta::new(id.unwrap_or_else(new_id), now()),
        |t| t.tag.meta,
    );
    let tag = Tag {
        meta,
        profile_id: profile,
        name: draft.name.clone(),
        color: Some(draft.color.clone()),
    };
    Ok(saved_from(call(bridge, RequestBody::PutTag { tag })?)?)
}

/// Deletes a tag and detaches it from every snippet.
///
/// # Errors
///
/// `not_found`, the call's other errors, or an unexpected response.
pub fn delete_tag_by_id(bridge: &Bridge, id: &str) -> Res<()> {
    Ok(done_from(call(bridge, delete_tag(parse_id(id)?))?)?)
}

/// What saving `abbreviation` with `trigger` would run into; `snippet` is left out of the
/// comparison.
///
/// # Errors
///
/// `invalid` for a bad trigger or id, the call's errors, or an unexpected response.
pub fn check(
    bridge: &Bridge,
    abbreviation: &str,
    trigger: &str,
    snippet: Option<&str>,
) -> Res<CheckView> {
    Ok(check_from(call(
        bridge,
        check_abbreviation(abbreviation, trigger, snippet)?,
    )?)?)
}
