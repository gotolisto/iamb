//! The request handlers, run on the reader thread of the client that asked.
//!
//! Reads, search, health and dry runs are answered from the current [`View`] (the snapshot)
//! without touching the store thread; writes go to the store thread; engine control goes to
//! the engine thread. The role check has already happened in `iamb-ipc`.
//!
//! Templates are answered from the snapshot too ([`crate::templates`]). Not implemented in this
//! build (answered with `not_implemented`): the Sync group (the sync agent arrives with task
//! 4.4).

use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::channel;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use iamb_core::datetime::LocalDateTime;
use iamb_core::engine::{Environment, ExclusionReason, FormSessionId, dry_run};
use iamb_core::model::{
    Abbreviation, Folder, Id, Snippet, SnippetKind, SnippetTag, SyncMeta, Tag, Trigger,
    fold_abbreviation,
};
use iamb_core::sensitive::Sensitive;
use iamb_core::template::{FormValues, overwrite};
use iamb_ipc::{
    ChangedRows, ConflictPolicy, ErrorBody, ErrorCode, ExcludedEntry, GrantState, HealthReport,
    RequestBody, ResponseBody, Role, SUMMARY_LINE_CHARS, SearchField, SearchHit, ShadowedEntry,
    SnippetQuery, SnippetSummary, TAG_COLORS, TagEntry, TransferFormat, UnusedEntry, WireAction,
    WireAttachment, WireDiagnostic, WireExclusion, WireValues,
};
use iamb_platform::{Chord, INJECTION_SETTINGS_KEY};
use iamb_store::health::{self, Why};
use iamb_store::transfer::{self, Format, OnConflict, TransferError};
use iamb_store::{AppExclusion, ListOptions, StoreError, Usage};

use super::store::{StoreCtx, new_id};
use super::{EngineMsg, Explicit, FocusMsg, PALETTE_HOTKEY, Shared};
use crate::clock;
use crate::library::{AppIdRule, View, engine_settings};
use crate::templates;

/// The most summaries one `ListSnippetSummaries` returns.
const MAX_SUMMARIES: u32 = 1000;

/// The longest tag name, in characters.
const MAX_TAG_NAME: usize = 64;

/// How long a reader waits for the engine thread to take in a `PanelFocus`.
const PANEL_FOCUS_WAIT: Duration = Duration::from_secs(2);

type Answer = Result<ResponseBody, ErrorBody>;

fn err(code: ErrorCode, message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(code, message)
}

fn unavailable() -> ErrorBody {
    err(ErrorCode::Unavailable, "the service is shutting down")
}

fn not_implemented(what: &str) -> ErrorBody {
    err(
        ErrorCode::NotImplemented,
        format!("{what} is not implemented in this build"),
    )
}

/// A store error as the client sees it.
fn store_error(e: &StoreError) -> ErrorBody {
    match e {
        StoreError::AbbreviationConflict { .. } => err(ErrorCode::Conflict, e.to_string()),
        StoreError::NotFound { .. } => err(ErrorCode::NotFound, e.to_string()),
        StoreError::FolderTooDeep { .. }
        | StoreError::InvalidData(_)
        | StoreError::Attachment(_) => err(ErrorCode::Invalid, e.to_string()),
        _ => err(ErrorCode::Internal, e.to_string()),
    }
}

fn transfer_error(e: &TransferError) -> ErrorBody {
    match e {
        TransferError::Store(s) => store_error(s),
        TransferError::Conflict { .. } => err(ErrorCode::Conflict, e.to_string()),
        TransferError::Io { .. } => err(ErrorCode::NotFound, e.to_string()),
        _ => err(ErrorCode::Invalid, e.to_string()),
    }
}

/// Runs a write on the store thread.
fn write<R: Send + 'static>(
    shared: &Shared,
    f: impl FnOnce(&mut StoreCtx) -> Result<R, ErrorBody> + Send + 'static,
) -> Result<R, ErrorBody> {
    shared.store.call(f).unwrap_or_else(|| Err(unavailable()))
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "the common success answer of many arms"
)]
fn done() -> Answer {
    Ok(ResponseBody::Done {})
}

/// Answers one request.
#[allow(clippy::too_many_lines, reason = "one arm per request of §6.2")]
pub(crate) fn handle(shared: &Arc<Shared>, _role: Role, body: RequestBody) -> Answer {
    let view = shared.view.get();
    match body {
        // ---- Library ------------------------------------------------------------------------
        RequestBody::ListFolders {} => Ok(ResponseBody::Folders {
            profile: view.profile,
            folders: view.folders.as_ref().clone(),
        }),
        RequestBody::ListSnippets { query } => {
            let snippets: Vec<Snippet> = matching(&view, &query).cloned().collect();
            let tags = snippets
                .iter()
                .filter_map(|s| {
                    let tags = view.tags_of(s.meta.id);
                    (!tags.is_empty()).then(|| (s.meta.id, tags.to_vec()))
                })
                .collect();
            Ok(ResponseBody::Snippets { snippets, tags })
        }
        RequestBody::GetSnippet { id } => view
            .snippets()
            .find(|s| s.meta.id == id)
            .map(|s| ResponseBody::Snippet {
                snippet: s.clone(),
                tags: view.tags_of(id).to_vec(),
            })
            .ok_or_else(|| err(ErrorCode::NotFound, format!("snippet {id} not found"))),
        RequestBody::PutSnippet { snippet, tags } => put_snippet(shared, &view, snippet, tags),
        RequestBody::DeleteSnippet { id } => {
            if !view.snippet_ids.contains(&id) {
                return Err(err(ErrorCode::NotFound, format!("snippet {id} not found")));
            }
            write(shared, move |ctx| {
                ctx.store
                    .delete_snippet(id, clock::timestamp())
                    .map_err(|e| store_error(&e))?;
                ctx.rows_changed(ChangedRows {
                    snippets: vec![id],
                    ..ChangedRows::default()
                });
                Ok(())
            })
            .and_then(|()| done())
        }
        RequestBody::PutFolder { folder } => put_folder(shared, &view, folder),
        RequestBody::DeleteFolder { id } => {
            if !view.folders.iter().any(|f| f.meta.id == id) {
                return Err(err(ErrorCode::NotFound, format!("folder {id} not found")));
            }
            write(shared, move |ctx| {
                ctx.store
                    .delete_folder(id, clock::timestamp())
                    .map_err(|e| store_error(&e))?;
                ctx.library_changed();
                Ok(())
            })
            .and_then(|()| done())
        }
        RequestBody::PutTag { tag } => put_tag(shared, &view, tag),
        RequestBody::ListTags {} => Ok(ResponseBody::Tags {
            tags: list_tags(&view),
        }),
        RequestBody::DeleteTag { id } => delete_tag(shared, &view, id),
        RequestBody::ListSnippetSummaries {
            query,
            offset,
            limit,
        } => summaries(shared, &view, &query, offset, limit),
        RequestBody::CheckAbbreviation {
            abbreviation,
            trigger,
            snippet,
        } => check_abbreviation(&view, &abbreviation, &trigger, snippet),
        RequestBody::Search { text, limit } => Ok(ResponseBody::SearchResults {
            hits: search(
                &view,
                text.expose(),
                usize::try_from(limit).unwrap_or(usize::MAX),
            ),
        }),
        RequestBody::Import {
            path,
            format,
            on_conflict,
        } => {
            if !path.is_absolute() {
                return Err(err(ErrorCode::Invalid, "the import path must be absolute"));
            }
            write(shared, move |ctx| {
                let profile = ctx.profile;
                let summary = transfer::import(
                    &mut ctx.store,
                    profile,
                    &path,
                    format_of(format),
                    clock::timestamp(),
                    &mut new_id,
                    match on_conflict {
                        ConflictPolicy::Skip => OnConflict::Skip,
                        ConflictPolicy::Fail => OnConflict::Fail,
                    },
                )
                .map_err(|e| transfer_error(&e))?;
                ctx.library_changed();
                Ok(summary)
            })
            .map(|s| {
                let n = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
                ResponseBody::Imported {
                    snippets_imported: n(s.snippets_imported),
                    snippets_skipped: n(s.snippets_skipped),
                    folders_created: n(s.folders_created),
                    tags_created: n(s.tags_created),
                }
            })
        }
        RequestBody::Export { path, format } => {
            if !path.is_absolute() {
                return Err(err(ErrorCode::Invalid, "the export path must be absolute"));
            }
            write(shared, move |ctx| {
                transfer::export(&ctx.store, ctx.profile, &path, format_of(format))
                    .map_err(|e| transfer_error(&e))
            })
            .map(|s| ResponseBody::Exported {
                snippets: u64::try_from(s.snippets).unwrap_or(u64::MAX),
            })
        }
        RequestBody::Health {} => health(shared, &view),
        RequestBody::RecentSnippets { limit } => recent(shared, &view, limit),
        RequestBody::ResetUsage {} => write(shared, |ctx| {
            let n = ctx
                .store
                .reset_usage(clock::timestamp())
                .map_err(|e| store_error(&e))?;
            // Counts only: which snippets they were is the user's business (ADR-0015).
            Ok(n)
        })
        .and_then(|n| {
            shared
                .logger
                .info(format_args!("usage reset: {n} snippets had counts"));
            done()
        }),
        // Only snippets written by other members need review, and other members' snippets
        // arrive with sync (task 4.4); until then there are none.
        RequestBody::ListPendingReviews {} => Ok(ResponseBody::PendingReviews {
            reviews: Vec::new(),
        }),
        RequestBody::PutAttachment {
            path,
            file_name,
            folder,
        } => put_attachment(shared, &view, &path, file_name, folder),
        RequestBody::ApproveSnippet { id, rev } => write(shared, move |ctx| {
            ctx.store
                .put_approval(id, rev, clock::timestamp())
                .map_err(|e| store_error(&e))?;
            ctx.library_changed();
            Ok(())
        })
        .and_then(|()| done()),

        // ---- Engine control -----------------------------------------------------------------
        RequestBody::GetStatus {} => status(shared),
        RequestBody::SetPaused { paused } => {
            if !shared.to_engine(EngineMsg::SetPaused(paused)) {
                return Err(unavailable());
            }
            status(shared)
        }
        RequestBody::ExpandSnippet { id, focus } => {
            if shared.to_engine(EngineMsg::Request {
                request: Explicit::Expand(id),
                focus: focus.map(Into::into),
            }) {
                done()
            } else {
                Err(unavailable())
            }
        }
        RequestBody::DryRun {
            typed,
            now,
            clipboard,
            values,
        } => dry(
            shared,
            &view,
            typed.expose(),
            now.as_deref(),
            clipboard,
            values,
        ),
        RequestBody::GetPermissions {} => {
            let status = shared.platform.permission_status();
            Ok(ResponseBody::Permissions {
                accessibility: GrantState::from(status.accessibility),
                input_monitoring: GrantState::from(status.input_monitoring),
            })
        }
        RequestBody::OpenPermissionSettings { kind } => shared
            .platform
            .open_settings(kind.into())
            .map_err(|e| err(ErrorCode::Unavailable, e.to_string()))
            .and_then(|()| done()),
        RequestBody::SetTestField { window } => {
            *shared
                .test_field
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = window.map(Into::into);
            let _ = shared.focus_tx.send(FocusMsg::Rejudge);
            done()
        }

        // ---- Forms --------------------------------------------------------------------------
        RequestBody::FormAck { session } => {
            engine(shared, EngineMsg::FormAck(FormSessionId::from_raw(session)))
        }
        RequestBody::FormSubmit { session, values } => {
            let mut wire = values.into_inner();
            let values: FormValues = wire.iter().collect();
            // The copy that came over the wire is overwritten, not only dropped (§4.5).
            for value in wire.values_mut() {
                overwrite(value);
            }
            drop(wire);
            engine(
                shared,
                EngineMsg::Request {
                    request: Explicit::Submit {
                        session: FormSessionId::from_raw(session),
                        values: Sensitive::new(values),
                    },
                    focus: None,
                },
            )
        }
        RequestBody::FormCancel { session } => engine(
            shared,
            EngineMsg::FormCancel(FormSessionId::from_raw(session)),
        ),
        RequestBody::ReopenLastForm {} => engine(
            shared,
            EngineMsg::Request {
                request: Explicit::Reopen,
                focus: None,
            },
        ),

        RequestBody::PanelFocus { panel, shown } => {
            let (reply, answered) = channel();
            if !shared.to_engine(EngineMsg::PanelFocus {
                panel,
                shown,
                reply,
            }) {
                return Err(unavailable());
            }
            // Answered once the engine took it in, so the UI host can show the panel then.
            answered
                .recv_timeout(PANEL_FOCUS_WAIT)
                .map_err(|_| unavailable())
                .and_then(|()| done())
        }

        // ---- Templates ----------------------------------------------------------------------
        RequestBody::ParseTemplate { source } => {
            let (tokens, error) = templates::parse(&view.snapshot, &source);
            Ok(ResponseBody::TemplateTokens { tokens, error })
        }
        RequestBody::RenderPreview {
            source,
            values,
            rich,
        } => {
            let answer = if rich {
                templates::preview_rich(
                    &view.snapshot,
                    &source,
                    values.expose(),
                    (shared.clock)(),
                    &|sha| crate::rich::attachment_info(&shared.data_root, view.profile, sha),
                )
            } else {
                templates::preview(&view.snapshot, &source, values.expose(), (shared.clock)())
            };
            Ok(ResponseBody::Preview {
                parts: answer.parts,
                cursor: answer.cursor,
                fields: answer.fields,
                error: answer.error,
            })
        }

        // ---- Diagnostics --------------------------------------------------------------------
        RequestBody::RecentDiagnostics {} => Ok(ResponseBody::Diagnostics {
            entries: shared.diagnostics.all(),
        }),

        // ---- Settings -----------------------------------------------------------------------
        RequestBody::GetSettings {} => Ok(ResponseBody::Settings {
            settings: view.settings.as_ref().clone(),
        }),
        RequestBody::PutSetting { key, value } => put_setting(shared, &view, key, value),
        RequestBody::ListExclusions {} => Ok(ResponseBody::Exclusions {
            exclusions: view
                .exclusions
                .iter()
                .map(|e| WireExclusion {
                    app_id: e.app_id.clone(),
                    display_name: e.display_name.clone(),
                    created_at: e.created_at,
                })
                .collect(),
        }),
        RequestBody::PutExclusion {
            app_id,
            display_name,
            excluded,
        } => put_exclusion(shared, &app_id, display_name, excluded),

        // ---- Sync ---------------------------------------------------------------------------
        RequestBody::OutboxRead { .. }
        | RequestBody::OutboxAck { .. }
        | RequestBody::ApplyRemote { .. }
        | RequestBody::PurgeFolder { .. }
        | RequestBody::AttachmentAvailable { .. }
        | RequestBody::MissingAttachments {}
        | RequestBody::SetSyncStatus { .. }
        | RequestBody::SetEntitlements { .. } => Err(not_implemented("sync")),
    }
}

fn engine(shared: &Shared, msg: EngineMsg) -> Answer {
    if shared.to_engine(msg) {
        done()
    } else {
        Err(unavailable())
    }
}

fn status(shared: &Shared) -> Answer {
    let engine = shared.engine_status().ok_or_else(unavailable)?;
    Ok(ResponseBody::Status {
        status: shared.status(engine),
    })
}

const fn format_of(f: TransferFormat) -> Format {
    match f {
        TransferFormat::Json => Format::Json,
        TransferFormat::Csv => Format::Csv,
    }
}

/// Adds or removes an exclusion, stored in the form the focus inspector reports
/// ([`AppIdRule`]), and answers with the rows written or removed: the client sees the id that
/// will be compared. An id that can never be reported is refused; removing one still removes
/// a row stored under it before this rule existed.
fn put_exclusion(shared: &Shared, app_id: &str, display_name: String, excluded: bool) -> Answer {
    let key = AppIdRule::this_os().normalise(app_id);
    let wire = |e: AppExclusion| WireExclusion {
        app_id: e.app_id,
        display_name: e.display_name,
        created_at: e.created_at,
    };
    if excluded {
        let key = key.map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
        let row = write(shared, move |ctx| {
            let row = AppExclusion {
                app_id: key,
                display_name,
                created_at: clock::timestamp(),
            };
            ctx.store
                .put_app_exclusion(&row)
                .map_err(|e| store_error(&e))?;
            ctx.settings_changed();
            Ok(row)
        })?;
        return Ok(ResponseBody::Exclusions {
            exclusions: vec![wire(row)],
        });
    }
    let mut ids = vec![app_id.to_owned()];
    if let Ok(key) = key
        && key != app_id
    {
        ids.push(key);
    }
    let removed = write(shared, move |ctx| {
        let mut removed = Vec::new();
        for id in ids {
            if let Some(row) = ctx
                .store
                .get_app_exclusion(&id)
                .map_err(|e| store_error(&e))?
            {
                ctx.store
                    .delete_app_exclusion(&id)
                    .map_err(|e| store_error(&e))?;
                removed.push(row);
            }
        }
        ctx.settings_changed();
        Ok(removed)
    })?;
    Ok(ResponseBody::Exclusions {
        exclusions: removed.into_iter().map(wire).collect(),
    })
}

/// The snippets of `view` that `query` selects, in the library's order.
fn matching<'v>(view: &'v View, query: &SnippetQuery) -> impl Iterator<Item = &'v Snippet> {
    let (folder, tag) = (query.folder, query.tag);
    view.snippets()
        .filter(move |s| folder.is_none_or(|f| s.folder_id == f))
        .filter(move |s| tag.is_none_or(|t| view.tags_of(s.meta.id).contains(&t)))
}

fn put_snippet(shared: &Shared, view: &View, snippet: Snippet, tags: Option<Vec<Id>>) -> Answer {
    if !view.folders.iter().any(|f| f.meta.id == snippet.folder_id) {
        return Err(err(
            ErrorCode::Invalid,
            "the snippet's folder is not a folder of the personal profile",
        ));
    }
    if let Some(tags) = &tags
        && let Some(unknown) = tags
            .iter()
            .find(|t| !view.tags.iter().any(|tag| tag.meta.id == **t))
    {
        return Err(err(
            ErrorCode::Invalid,
            format!("{unknown} is not a tag of the personal profile"),
        ));
    }
    let id = snippet.meta.id;
    write(shared, move |ctx| {
        let now = clock::timestamp();
        // One transaction: the row (a deleted one comes back) and its links.
        let meta = ctx
            .store
            .write(|tx| {
                let meta = tx.put(&snippet, now)?;
                if let Some(tags) = &tags {
                    let want: BTreeSet<Id> = tags.iter().copied().collect();
                    let links: Vec<SnippetTag> = tx
                        .list::<SnippetTag>(ListOptions::LIVE)?
                        .into_iter()
                        .filter(|l| l.snippet_id == id)
                        .collect();
                    let mut have = BTreeSet::new();
                    for link in links {
                        if !want.contains(&link.tag_id) || !have.insert(link.tag_id) {
                            tx.delete::<SnippetTag>(link.meta.id, now)?;
                        }
                    }
                    for tag in want.difference(&have) {
                        tx.put(
                            &SnippetTag {
                                meta: SyncMeta::new(new_id(), now),
                                snippet_id: id,
                                tag_id: *tag,
                            },
                            now,
                        )?;
                    }
                }
                // The attachments its Markdown references (task 3.8).
                tx.sync_snippet_attachments(id, &mut new_id, now)?;
                Ok(meta)
            })
            .map_err(|e| store_error(&e))?;
        ctx.rows_changed(ChangedRows {
            snippets: vec![id],
            ..ChangedRows::default()
        });
        Ok(meta)
    })
    .map(|meta| ResponseBody::Saved { meta })
}

/// The bytes of the file a `PutAttachment` names, at most the media limit (the Phase 3 UI
/// review's finding 10). Only a regular file is read: a FIFO or a device would pass a size check
/// (its length is 0) and then block this thread, or never end. The kind is checked before the
/// file is opened (opening a FIFO waits for a writer) and again on the open handle, and the
/// read stops one byte past the limit whatever the length said.
fn read_attachment_file(path: &std::path::Path) -> Result<Vec<u8>, ErrorBody> {
    use std::io::Read;
    let limit = iamb_store::attachments::MAX_MEDIA_BYTES;
    let too_large = |size: u64| {
        store_error(&StoreError::Attachment(
            iamb_store::AttachmentError::TooLarge { size, limit },
        ))
    };
    let unreadable = || err(ErrorCode::NotFound, "the file cannot be read");
    let not_a_file = || err(ErrorCode::Invalid, "the attachment must be a regular file");
    let meta = std::fs::metadata(path).map_err(|_| unreadable())?;
    if !meta.is_file() {
        return Err(not_a_file());
    }
    if meta.len() > limit {
        return Err(too_large(meta.len()));
    }
    let file = std::fs::File::open(path).map_err(|_| unreadable())?;
    if !file.metadata().map_err(|_| unreadable())?.is_file() {
        return Err(not_a_file());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unreadable())?;
    if bytes.len() as u64 > limit {
        return Err(too_large(bytes.len() as u64));
    }
    Ok(bytes)
}

/// `PutAttachment` (task 3.8): reads the file here, on the client's reader thread, and writes
/// it on the store thread.
fn put_attachment(
    shared: &Shared,
    view: &View,
    path: &std::path::Path,
    file_name: String,
    folder: Id,
) -> Answer {
    let Some(profile) = view
        .folders
        .iter()
        .find(|f| f.meta.id == folder)
        .map(|f| f.profile_id)
    else {
        return Err(err(
            ErrorCode::Invalid,
            "the folder is not a folder of the personal profile",
        ));
    };
    if !path.is_absolute() {
        return Err(err(
            ErrorCode::Invalid,
            "the attachment path must be absolute",
        ));
    }
    let bytes = read_attachment_file(path)?;
    write(shared, move |ctx| {
        let row = ctx
            .store
            .add_attachment(new_id(), profile, &bytes, &file_name, clock::timestamp())
            .map_err(|e| store_error(&e))?;
        let path = ctx.store.attachment_path(profile, &row.sha256);
        Ok(WireAttachment {
            sha256: row.sha256,
            mime: row.mime,
            size: row.size,
            file_name: row.file_name,
            path,
        })
    })
    .map(|attachment| ResponseBody::Attachment { attachment })
}

fn put_folder(shared: &Shared, view: &View, folder: Folder) -> Answer {
    if folder.profile_id != view.profile {
        return Err(err(
            ErrorCode::Invalid,
            "the folder must belong to the personal profile",
        ));
    }
    let id = folder.meta.id;
    write(shared, move |ctx| {
        let meta = ctx
            .store
            .put_folder(&folder, clock::timestamp())
            .map_err(|e| store_error(&e))?;
        ctx.rows_changed(ChangedRows {
            folders: vec![id],
            ..ChangedRows::default()
        });
        Ok(meta)
    })
    .map(|meta| ResponseBody::Saved { meta })
}

/// Checks a tag's name and colour (the UX specification's eight colours) and that no other
/// tag of the profile has the name.
fn checked_tag(view: &View, mut tag: Tag) -> Result<Tag, ErrorBody> {
    if tag.profile_id != view.profile {
        return Err(err(
            ErrorCode::Invalid,
            "the tag must belong to the personal profile",
        ));
    }
    let name = tag.name.trim().to_owned();
    if name.is_empty() {
        return Err(err(ErrorCode::Invalid, "the tag name is empty"));
    }
    if name.chars().count() > MAX_TAG_NAME || name.chars().any(char::is_control) {
        return Err(err(
            ErrorCode::Invalid,
            format!("a tag name has at most {MAX_TAG_NAME} characters and no line break"),
        ));
    }
    if !tag
        .color
        .as_deref()
        .is_some_and(|c| TAG_COLORS.contains(&c))
    {
        return Err(err(
            ErrorCode::Invalid,
            format!("the tag colour must be one of {}", TAG_COLORS.join(", ")),
        ));
    }
    let folded = name.to_lowercase();
    if let Some(other) = view
        .tags
        .iter()
        .find(|t| t.meta.id != tag.meta.id && t.name.trim().to_lowercase() == folded)
    {
        return Err(err(
            ErrorCode::Conflict,
            format!("the tag {} has that name already", other.meta.id),
        ));
    }
    tag.name = name;
    Ok(tag)
}

fn put_tag(shared: &Shared, view: &View, tag: Tag) -> Answer {
    let tag = checked_tag(view, tag)?;
    let id = tag.meta.id;
    write(shared, move |ctx| {
        let meta = ctx
            .store
            .put_tag(&tag, clock::timestamp())
            .map_err(|e| store_error(&e))?;
        ctx.rows_changed(ChangedRows {
            tags: vec![id],
            ..ChangedRows::default()
        });
        Ok(meta)
    })
    .map(|meta| ResponseBody::Saved { meta })
}

/// The tags, by name, each with the number of snippets that carry it.
fn list_tags(view: &View) -> Vec<TagEntry> {
    let mut counts: HashMap<Id, u64> = HashMap::new();
    for tags in view.snippet_tags.values() {
        for tag in tags {
            *counts.entry(*tag).or_default() += 1;
        }
    }
    view.tags
        .iter()
        .map(|tag| TagEntry {
            tag: tag.clone(),
            snippets: counts.get(&tag.meta.id).copied().unwrap_or(0),
        })
        .collect()
}

/// Deletes a tag and its links in one transaction.
fn delete_tag(shared: &Shared, view: &View, id: Id) -> Answer {
    if !view.tags.iter().any(|t| t.meta.id == id) {
        return Err(err(ErrorCode::NotFound, format!("tag {id} not found")));
    }
    write(shared, move |ctx| {
        let now = clock::timestamp();
        let snippets = ctx
            .store
            .write(|tx| {
                let mut snippets = Vec::new();
                for link in tx.list::<SnippetTag>(ListOptions::LIVE)? {
                    if link.tag_id == id {
                        tx.delete::<SnippetTag>(link.meta.id, now)?;
                        snippets.push(link.snippet_id);
                    }
                }
                tx.delete::<Tag>(id, now)?;
                Ok(snippets)
            })
            .map_err(|e| store_error(&e))?;
        ctx.rows_changed(ChangedRows {
            snippets,
            tags: vec![id],
            ..ChangedRows::default()
        });
        Ok(())
    })
    .and_then(|()| done())
}

/// One page of summaries (no bodies beyond the first line).
fn summaries(
    shared: &Shared,
    view: &View,
    query: &SnippetQuery,
    offset: u32,
    limit: u32,
) -> Answer {
    if limit == 0 || limit > MAX_SUMMARIES {
        return Err(err(
            ErrorCode::Invalid,
            format!("the limit must be 1 to {MAX_SUMMARIES}"),
        ));
    }
    let excluded: HashMap<Id, &ExclusionReason> = view
        .snapshot
        .excluded()
        .iter()
        .map(|e| (e.snippet, &e.reason))
        .collect();
    let all: Vec<&Snippet> = matching(view, query).collect();
    // Usage is device-local and is not part of the view (it changes at every expansion without
    // a rebuild), so it is read from the store, once per page.
    let usage: HashMap<Id, Usage> = write(shared, |ctx| {
        ctx.store.list_usage().map_err(|e| store_error(&e))
    })?
    .into_iter()
    .map(|u| (u.snippet_id, u))
    .collect();
    let start = usize::try_from(offset).unwrap_or(usize::MAX).min(all.len());
    let end = start
        .saturating_add(usize::try_from(limit).unwrap_or(usize::MAX))
        .min(all.len());
    let page: Vec<SnippetSummary> = all[start..end]
        .iter()
        .map(|s| {
            let broken = match (excluded.get(&s.meta.id), view.snapshot.compiled(s.meta.id)) {
                (Some(ExclusionReason::Duplicate { .. }), _) => Some("duplicate".to_owned()),
                (Some(ExclusionReason::Template(e)), _) | (None, Some(Err(e))) => {
                    Some(e.code().to_owned())
                }
                _ => None,
            };
            SnippetSummary {
                id: s.meta.id,
                folder: s.folder_id,
                abbreviation: s.abbreviation.as_str().to_owned(),
                label: s.label.clone(),
                first_line: s
                    .body
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(SUMMARY_LINE_CHARS)
                    .collect(),
                kind: match s.kind {
                    SnippetKind::Plain => "plain",
                    SnippetKind::Rich => "rich",
                }
                .to_owned(),
                trigger: trigger_name(s.trigger).to_owned(),
                enabled: s.enabled,
                tags: view.tags_of(s.meta.id).to_vec(),
                broken,
                uses: Some(usage.get(&s.meta.id).map_or(0, |u| u.count)),
                last_used: usage.get(&s.meta.id).and_then(|u| u.last_used),
                updated_at: s.meta.updated_at,
            }
        })
        .collect();
    Ok(ResponseBody::SnippetSummaries {
        revision: view.revision,
        total: u64::try_from(all.len()).unwrap_or(u64::MAX),
        summaries: page,
        next: (end < all.len()).then(|| u32::try_from(end).unwrap_or(u32::MAX)),
    })
}

const fn trigger_name(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Immediate => "immediate",
        Trigger::Delimiter => "delimiter",
    }
}

/// What saving `abbreviation` would run into (§4.4), from the snapshot.
fn check_abbreviation(
    view: &View,
    abbreviation: &str,
    trigger: &str,
    snippet: Option<Id>,
) -> Answer {
    let abbreviation =
        Abbreviation::new(abbreviation).map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
    let trigger = match trigger {
        "immediate" => Trigger::Immediate,
        "delimiter" => Trigger::Delimiter,
        _ => {
            return Err(err(
                ErrorCode::Invalid,
                "the trigger must be immediate or delimiter",
            ));
        }
    };
    let key = abbreviation.fold_key();
    let conflict = view
        .snippets()
        .find(|s| Some(s.meta.id) != snippet && s.abbreviation.fold_key() == key)
        .map(|s| s.meta.id);
    let (hides, hidden_by) =
        view.snapshot
            .matcher()
            .shadowing_of(abbreviation.as_str(), trigger, snippet);
    Ok(ResponseBody::AbbreviationCheck {
        conflict,
        hides,
        hidden_by,
    })
}

fn put_setting(shared: &Shared, view: &View, key: String, value: Option<String>) -> Answer {
    if key.is_empty() {
        return Err(err(ErrorCode::Invalid, "the setting key is empty"));
    }
    // A value the engine cannot use is refused before it is stored.
    let mut next = view.settings.as_ref().clone();
    match &value {
        Some(v) => next.insert(key.clone(), v.clone()),
        None => next.remove(&key),
    };
    engine_settings(&next).map_err(|m| err(ErrorCode::Invalid, m))?;
    if key == PALETTE_HOTKEY_KEY {
        // The chord is taken from the system before it is stored: a chord the system refuses
        // is the client's error, and the one registered before stays.
        palette_hotkey(shared, view, value.as_deref())?;
    }
    let for_injector = key == INJECTION_SETTINGS_KEY;
    write(shared, move |ctx| {
        // The injector checks its settings by taking them (ADR-0025): a refusal is the
        // client's error, and nothing is stored.
        let before = for_injector.then(|| ctx.injection.checkpoint());
        if for_injector {
            ctx.injection
                .try_apply(value.as_deref().unwrap_or(""))
                .map_err(|e| {
                    err(
                        ErrorCode::Invalid,
                        format!("invalid {INJECTION_SETTINGS_KEY} setting: {e}"),
                    )
                })?;
        }
        let stored = match &value {
            Some(v) => ctx.store.put_setting(&key, v),
            None => ctx.store.delete_setting(&key).map(|_| ()),
        };
        if let Err(e) = stored {
            if let Some(before) = before {
                ctx.injection.restore(before);
            }
            return Err(store_error(&e));
        }
        ctx.settings_changed();
        Ok(())
    })
    .and_then(|()| done())
}

/// The setting that holds the palette's chord.
const PALETTE_HOTKEY_KEY: &str = "palette_hotkey";

/// Registers `value` as the palette's chord (none: unregisters it). A chord that does not parse
/// is `invalid`; one the system refuses (another application has it) is `conflict`, and the
/// chord of the current setting is registered again.
fn palette_hotkey(shared: &Shared, view: &View, value: Option<&str>) -> Result<(), ErrorBody> {
    let chord = value
        .map(|text| {
            Chord::parse(text).map_err(|_| {
                err(
                    ErrorCode::Invalid,
                    "the palette_hotkey setting is not a valid chord",
                )
            })
        })
        .transpose()?;
    let platform = &shared.platform;
    let _ = platform.unregister(PALETTE_HOTKEY);
    let Some(chord) = chord else {
        return Ok(());
    };
    match platform.register(PALETTE_HOTKEY, chord) {
        Ok(()) => Ok(()),
        Err(e) => {
            shared
                .logger
                .warn(format_args!("cannot register the palette hotkey: {e}"));
            if let Some(old) = view
                .settings
                .get(PALETTE_HOTKEY_KEY)
                .and_then(|t| Chord::parse(t).ok())
            {
                let _ = platform.register(PALETTE_HOTKEY, old);
            }
            Err(err(
                ErrorCode::Conflict,
                "the system refused this shortcut; another application may use it",
            ))
        }
    }
}

/// The palette search over the snapshot: fuzzy matching on the abbreviation, the label, the
/// body and the names of the snippet's tags, best first; each hit says which field matched and
/// where (`iamb_core::search`, which holds the ranking).
fn search(view: &View, query: &str, limit: usize) -> Vec<SearchHit> {
    let tag_names: HashMap<Id, &str> = view
        .tags
        .iter()
        .map(|t| (t.meta.id, t.name.as_str()))
        .collect();
    let docs = view.snippets().map(|s| iamb_core::search::Doc {
        id: s.meta.id,
        abbreviation: s.abbreviation.as_str(),
        label: s.label.as_str(),
        body: s.body.as_str(),
        tags: view.tags_of(s.meta.id),
    });
    iamb_core::search::search(query, limit, docs, &|id| tag_names.get(&id).copied())
        .into_iter()
        .map(|h| SearchHit {
            snippet: h.id,
            score: h.score,
            field: Some(match h.field {
                iamb_core::search::Field::Abbreviation => SearchField::Abbreviation,
                iamb_core::search::Field::Label => SearchField::Label,
                iamb_core::search::Field::Body => SearchField::Body,
                iamb_core::search::Field::Tag => SearchField::Tag,
            }),
            ranges: h.ranges,
        })
        .collect()
}

/// Library health (spec §4.9): the findings of [`iamb_store::health`] over the view and the
/// device-local usage.
fn health(shared: &Shared, view: &View) -> Answer {
    let (usage, since) = write(shared, |ctx| {
        let usage = ctx.store.list_usage().map_err(|e| store_error(&e))?;
        let since = ctx.store.usage_reset_at().map_err(|e| store_error(&e))?;
        Ok((usage, since))
    })?;
    let found = health::analyse(
        &view.snapshot,
        view.snippets(),
        &usage,
        since,
        clock::timestamp(),
    );
    Ok(ResponseBody::Health {
        report: HealthReport {
            shadowed: found
                .shadowed
                .into_iter()
                .map(|s| ShadowedEntry {
                    by: s.by,
                    hidden: s.hidden,
                })
                .collect(),
            excluded: found
                .excluded
                .into_iter()
                .map(|e| match e.why {
                    Why::Duplicate { kept } => ExcludedEntry {
                        snippet: e.snippet,
                        reason: "duplicate".to_owned(),
                        kept: Some(kept),
                        reference: None,
                        included: None,
                    },
                    Why::Template {
                        code,
                        reference,
                        included,
                    } => ExcludedEntry {
                        snippet: e.snippet,
                        reason: code.to_owned(),
                        kept: None,
                        reference,
                        included,
                    },
                })
                .collect(),
            unused: found
                .unused
                .into_iter()
                .map(|u| UnusedEntry {
                    snippet: u.snippet,
                    last_used: u.last_used,
                })
                .collect(),
        },
    })
}

/// The snippets used most recently, newest first (the palette with nothing typed).
fn recent(shared: &Shared, view: &View, limit: u32) -> Answer {
    if limit == 0 || limit > MAX_SUMMARIES {
        return Err(err(
            ErrorCode::Invalid,
            format!("the limit must be 1 to {MAX_SUMMARIES}"),
        ));
    }
    // More than asked for: a snippet that left the personal library since is skipped.
    let want = usize::try_from(limit).unwrap_or(usize::MAX);
    let used = write(shared, move |ctx| {
        ctx.store
            .recent_usage(want.saturating_mul(2).saturating_add(8))
            .map_err(|e| store_error(&e))
    })?;
    let hits = used
        .into_iter()
        .filter(|u| view.snapshot.snippet(u.snippet_id).is_some())
        .take(want)
        .map(|u| SearchHit {
            snippet: u.snippet_id,
            score: 0,
            field: None,
            ranges: Vec::new(),
        })
        .collect();
    Ok(ResponseBody::SearchResults { hits })
}

struct DryEnv {
    now: LocalDateTime,
    clipboard: Option<String>,
}

impl Environment for DryEnv {
    fn now(&self) -> LocalDateTime {
        self.now
    }

    fn clipboard_text(&self) -> Option<String> {
        self.clipboard.clone()
    }
}

fn dry(
    shared: &Shared,
    view: &View,
    typed: &str,
    now: Option<&str>,
    clipboard: Option<iamb_ipc::Secret<String>>,
    values: Option<iamb_ipc::Secret<WireValues>>,
) -> Answer {
    let now = match now {
        Some(text) => LocalDateTime::parse_iso(text)
            .map_err(|_| err(ErrorCode::Invalid, "now must be YYYY-MM-DDTHH:MM:SS"))?,
        None => (shared.clock)(),
    };
    let env = DryEnv {
        now,
        clipboard: clipboard.map(iamb_ipc::Secret::into_inner),
    };
    let values: Option<FormValues> = values.map(|v| v.into_inner().into_iter().collect());
    let actions = dry_run(&view.snapshot, typed, &env, values.as_ref());
    let expanded = actions
        .iter()
        .any(|a| matches!(a, iamb_core::engine::Action::Expanded { .. }));
    let diagnosed = actions
        .iter()
        .any(|a| matches!(a, iamb_core::engine::Action::Diagnostic(_)));
    let hints = if expanded || diagnosed {
        Vec::new()
    } else {
        parse_error_hints(view, typed)
    };
    let at = clock::timestamp();
    Ok(ResponseBody::DryRun {
        actions: actions
            .iter()
            .map(|a| WireAction::from_engine(a, at))
            .collect(),
        hints,
    })
}

/// A snippet whose template does not parse is not matchable (architecture §4.4), so the engine
/// stays silent about it; name its parse error when the typed text ends with its abbreviation
/// (alone or followed by a delimiter).
fn parse_error_hints(view: &View, typed: &str) -> Vec<WireDiagnostic> {
    let snapshot = &view.snapshot;
    let typed_fold = fold_abbreviation(typed);
    let body = typed_fold.strip_suffix(|c| snapshot.settings().delimiters.contains(c));
    let mut out = Vec::new();
    for ex in snapshot.excluded() {
        if let (ExclusionReason::Template(e), Some(sn)) = (&ex.reason, snapshot.snippet(ex.snippet))
        {
            let key = sn.abbreviation.fold_key();
            if typed_fold.ends_with(&key) || body.is_some_and(|b| b.ends_with(&key)) {
                let mut d = WireDiagnostic::code(e.code(), clock::timestamp());
                d.snippet = Some(sn.meta.id);
                out.push(d);
            }
        }
    }
    out
}
