//! The Sync group (`docs/sync-design.md` §7.2): what the sync agent asks of the store, and the
//! kept changes the UI host lists. Every request is one store transaction on the store thread;
//! rows from the server change the library, so the snapshot is rebuilt after them.

use iamb_ipc::{ErrorBody, RequestBody, ResponseBody};
use iamb_store::StoreError;

use super::Shared;
use super::store::StoreCtx;
use crate::clock;

type Answer = Result<ResponseBody, ErrorBody>;

fn on_store<R: Send + 'static>(
    shared: &Shared,
    f: impl FnOnce(&mut StoreCtx) -> Result<R, StoreError> + Send + 'static,
) -> Result<R, ErrorBody> {
    shared
        .store
        .call(move |ctx| f(ctx).map_err(|e| super::handlers::store_error(&e)))
        .unwrap_or_else(|| Err(super::handlers::unavailable()))
}

fn done<T>(r: Result<T, ErrorBody>) -> Answer {
    r.map(|_| ResponseBody::Done {})
}

/// Answers a request of the Sync group, or `ListKeptChanges` / `DismissKeptChange`; gives any
/// other request back.
#[allow(
    clippy::result_large_err,
    reason = "the request given back is moved, not copied, and is answered at once"
)]
pub(crate) fn handle(shared: &Shared, body: RequestBody) -> Result<Answer, RequestBody> {
    Ok(match body {
        RequestBody::OutboxRead { limit, phase } => on_store(shared, move |ctx| {
            ctx.store.sync_outbox_read(
                usize::try_from(limit).unwrap_or(usize::MAX),
                phase.unwrap_or_default(),
            )
        })
        .map(|b| ResponseBody::Changes {
            changes: b.changes,
            behind: b.behind,
        }),
        RequestBody::OutboxAck { results } => on_store(shared, move |ctx| {
            let r = ctx.store.sync_outbox_ack(&results, clock::timestamp());
            ctx.library_changed();
            r
        })
        .map(|outcome| ResponseBody::SyncAcked { outcome }),
        RequestBody::ApplyRemote {
            batch,
            position,
            mode,
        } => on_store(shared, move |ctx| {
            let r = ctx
                .store
                .sync_apply(&batch, position.as_deref(), mode, clock::timestamp());
            ctx.library_changed();
            r
        })
        .map(|status| ResponseBody::Applied { status }),
        RequestBody::PurgeFolder { id, except } => done(on_store(shared, move |ctx| {
            let r = ctx.store.purge_folder(id, &except, clock::timestamp());
            ctx.library_changed();
            r
        })),
        RequestBody::SyncState {} => on_store(shared, |ctx| ctx.store.sync_info())
            .map(|info| ResponseBody::SyncState { info }),
        RequestBody::SetAccount {
            user,
            personal_profile,
            device,
        } => done(on_store(shared, move |ctx| {
            ctx.store.set_account(user, personal_profile, device)
        })),
        RequestBody::SetDeviceId { device } => {
            done(on_store(shared, move |ctx| ctx.store.set_device_id(device)))
        }
        RequestBody::SetPosition { position } => done(on_store(shared, move |ctx| {
            ctx.store
                .set_sync_position(position.as_deref(), clock::timestamp())
        })),
        RequestBody::ReconcileBegin { scope, lost } => on_store(shared, move |ctx| {
            ctx.store.reconcile_begin(scope, lost.as_ref())
        })
        .map(|token| ResponseBody::ReconcileToken { token }),
        RequestBody::ReconcileEnd { token, position } => on_store(shared, move |ctx| {
            let r = ctx
                .store
                .reconcile_end(&token, position.as_deref(), clock::timestamp());
            ctx.library_changed();
            r
        })
        .map(|entries| ResponseBody::AccessRemovals { entries }),
        RequestBody::ClearLostLines {} => {
            done(on_store(shared, |ctx| ctx.store.clear_lost_lines()))
        }
        RequestBody::SetAccess { roots } => {
            done(on_store(shared, move |ctx| ctx.store.set_access(&roots)))
        }
        RequestBody::SetBlocked { seqs, reason } => done(on_store(shared, move |ctx| {
            ctx.store.set_blocked(&seqs, &reason)
        })),
        RequestBody::Unblock { reason } => done(on_store(shared, move |ctx| {
            ctx.store.unblock(reason.as_deref())
        })),
        RequestBody::NoteAccessRemoval { entry, refused } => done(on_store(shared, move |ctx| {
            ctx.store.note_access_removal(&entry, refused)
        })),
        RequestBody::SignedOut { confirmed, keep } => done(on_store(shared, move |ctx| {
            let r = ctx.store.signed_out(confirmed, keep, clock::timestamp());
            ctx.library_changed();
            r
        })),
        RequestBody::ForeignFolders {} => {
            on_store(shared, |ctx| ctx.store.foreign_folders()).map(|ids| ResponseBody::Ids { ids })
        }
        RequestBody::SetEntitlements { entitlements } => done(on_store(shared, move |ctx| {
            ctx.store
                .put_entitlements(&entitlements, clock::timestamp())
        })),
        RequestBody::ListKeptChanges {} => on_store(shared, |ctx| ctx.store.kept_changes())
            .map(|changes| ResponseBody::KeptChanges { changes }),
        RequestBody::DismissKeptChange { id } => done(on_store(shared, move |ctx| {
            ctx.store.dismiss_kept_change(id)
        })),
        other => return Err(other),
    })
}
