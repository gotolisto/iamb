//! The request handlers, run on the reader thread of the client that asked.
//!
//! Reads, search, health and dry runs are answered from the current [`View`] (the snapshot)
//! without touching the store thread; writes go to the store thread; engine control goes to
//! the engine thread. The role check has already happened in `iamb-ipc`.
//!
//! Not implemented in this build (answered with `not_implemented`): the Templates group (the
//! editor arrives with task 3.4) and the Sync group (the sync agent arrives with task 4.4).

use std::sync::{Arc, PoisonError};

use iamb_core::datetime::LocalDateTime;
use iamb_core::engine::{Environment, ExclusionReason, FormSessionId, dry_run};
use iamb_core::model::{Folder, Snippet, Tag, fold_abbreviation};
use iamb_core::sensitive::Sensitive;
use iamb_core::template::FormValues;
use iamb_ipc::{
    ConflictPolicy, ErrorBody, ErrorCode, ExcludedEntry, GrantState, HealthReport, RequestBody,
    ResponseBody, Role, SearchHit, ShadowedEntry, TransferFormat, WireAction, WireDiagnostic,
    WireExclusion, WireValues,
};
use iamb_platform::INJECTION_SETTINGS_KEY;
use iamb_store::transfer::{self, Format, OnConflict, TransferError};
use iamb_store::{AppExclusion, StoreError};

use super::store::{StoreCtx, new_id};
use super::{EngineMsg, Explicit, FocusMsg, Shared};
use crate::clock;
use crate::library::{AppIdRule, View, engine_settings};

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
        StoreError::FolderTooDeep { .. } | StoreError::InvalidData(_) => {
            err(ErrorCode::Invalid, e.to_string())
        }
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
        RequestBody::ListSnippets { query } => Ok(ResponseBody::Snippets {
            snippets: view
                .snippets()
                .filter(|s| query.folder.is_none_or(|f| s.folder_id == f))
                .cloned()
                .collect(),
        }),
        RequestBody::GetSnippet { id } => view
            .snippets()
            .find(|s| s.meta.id == id)
            .map(|s| ResponseBody::Snippet { snippet: s.clone() })
            .ok_or_else(|| err(ErrorCode::NotFound, format!("snippet {id} not found"))),
        RequestBody::PutSnippet { snippet } => put_snippet(shared, &view, snippet),
        RequestBody::DeleteSnippet { id } => {
            if !view.snippet_ids.contains(&id) {
                return Err(err(ErrorCode::NotFound, format!("snippet {id} not found")));
            }
            write(shared, move |ctx| {
                ctx.store
                    .delete_snippet(id, clock::timestamp())
                    .map_err(|e| store_error(&e))?;
                ctx.library_changed();
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
        RequestBody::Health {} => Ok(ResponseBody::Health {
            report: health(&view),
        }),
        // Only snippets written by other members need review, and other members' snippets
        // arrive with sync (task 4.4); until then there are none.
        RequestBody::ListPendingReviews {} => Ok(ResponseBody::PendingReviews {
            reviews: Vec::new(),
        }),
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
            let values: FormValues = values.into_inner().into_iter().collect();
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

        // ---- Templates ----------------------------------------------------------------------
        RequestBody::ParseTemplate { .. } | RequestBody::RenderPreview { .. } => {
            Err(not_implemented("the template editor"))
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

fn put_snippet(shared: &Shared, view: &View, snippet: Snippet) -> Answer {
    if !view.folders.iter().any(|f| f.meta.id == snippet.folder_id) {
        return Err(err(
            ErrorCode::Invalid,
            "the snippet's folder is not a folder of the personal profile",
        ));
    }
    write(shared, move |ctx| {
        let meta = ctx
            .store
            .put_snippet(&snippet, clock::timestamp())
            .map_err(|e| store_error(&e))?;
        ctx.library_changed();
        Ok(meta)
    })
    .map(|meta| ResponseBody::Saved { meta })
}

fn put_folder(shared: &Shared, view: &View, folder: Folder) -> Answer {
    if folder.profile_id != view.profile {
        return Err(err(
            ErrorCode::Invalid,
            "the folder must belong to the personal profile",
        ));
    }
    write(shared, move |ctx| {
        let meta = ctx
            .store
            .put_folder(&folder, clock::timestamp())
            .map_err(|e| store_error(&e))?;
        ctx.library_changed();
        Ok(meta)
    })
    .map(|meta| ResponseBody::Saved { meta })
}

fn put_tag(shared: &Shared, view: &View, tag: Tag) -> Answer {
    if tag.profile_id != view.profile {
        return Err(err(
            ErrorCode::Invalid,
            "the tag must belong to the personal profile",
        ));
    }
    write(shared, move |ctx| {
        let meta = ctx
            .store
            .put_tag(&tag, clock::timestamp())
            .map_err(|e| store_error(&e))?;
        ctx.library_changed();
        Ok(meta)
    })
    .map(|meta| ResponseBody::Saved { meta })
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

/// The palette search over the snapshot: case-folded substring matching on the abbreviation,
/// the label and the body, best first. Task 3.6 replaces it with the fuzzy search of
/// `iamb-core`.
fn search(view: &View, query: &str, limit: usize) -> Vec<SearchHit> {
    let q = fold_abbreviation(query.trim());
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<SearchHit> = view
        .snippets()
        .filter_map(|s| {
            let abbr = s.abbreviation.fold_key();
            let score = if abbr == q {
                100
            } else if abbr.starts_with(&q) {
                80
            } else if abbr.contains(&q) {
                60
            } else if fold_abbreviation(&s.label).contains(&q) {
                40
            } else if fold_abbreviation(&s.body).contains(&q) {
                20
            } else {
                return None;
            };
            Some(SearchHit {
                snippet: s.meta.id,
                score,
            })
        })
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.score));
    hits.truncate(limit);
    hits
}

fn health(view: &View) -> HealthReport {
    HealthReport {
        shadowed: view
            .snapshot
            .shadowed()
            .into_iter()
            .map(|s| ShadowedEntry {
                by: s.by,
                hidden: s.hidden,
            })
            .collect(),
        excluded: view
            .snapshot
            .excluded()
            .iter()
            .map(|e| match &e.reason {
                ExclusionReason::Duplicate { kept } => ExcludedEntry {
                    snippet: e.snippet,
                    reason: "duplicate".to_owned(),
                    kept: Some(*kept),
                },
                ExclusionReason::Template(t) => ExcludedEntry {
                    snippet: e.snippet,
                    reason: t.code().to_owned(),
                    kept: None,
                },
            })
            .collect(),
    }
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
    Ok(ResponseBody::DryRun {
        actions: actions.iter().map(WireAction::from).collect(),
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
                let mut d = WireDiagnostic::code(e.code());
                d.snippet = Some(sn.meta.id);
                out.push(d);
            }
        }
    }
    out
}
