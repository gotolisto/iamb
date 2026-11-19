//! The store thread: owns the `SQLite` connection (and so `run/store.lock`), performs every
//! write, records usage, and after a change to library content or settings builds the next
//! [`View`] and tells the engine (`SnapshotChanged`), the injector (a changed
//! `injection_overrides` setting, [`injection`](super::injection)) and the clients
//! (`LibraryChanged`, `SettingsChanged`). Usage records do not rebuild anything (ADR-0007).

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use iamb_core::model::Id;
use iamb_ipc::{ChangedRows, EventBody};
use iamb_store::{Store, StoreError};

use super::injection::InjectionSettings;
use super::{EngineMsg, FocusMsg, Shared, WakeCounters};
use crate::clock;
use crate::library::{
    AppIdRule, ExclusionMigration, View, build_view, ensure_personal_profile, migrate_exclusions,
};

/// A store error as the log shows it: its kind with the numbers and ids it carries, never a
/// value from the database or a path (the log's rule is names, sizes, counts, ids and codes;
/// the 2.8 review's finding 19). `InvalidData` can quote a stored value, `AbbreviationConflict`
/// an abbreviation, `Locked` a path with the user's name, `Sqlite` and `Io` messages of their
/// own.
pub(crate) struct Logged<'a>(pub(crate) &'a StoreError);

impl fmt::Display for Logged<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            StoreError::Locked { .. } => f.write_str("the store is locked by another process"),
            StoreError::SchemaTooNew { found, supported } => write!(
                f,
                "database schema version {found} is newer than this build supports ({supported})"
            ),
            StoreError::NotFound { entity, id } => write!(f, "{entity} {id} not found"),
            StoreError::AbbreviationConflict { existing, .. } => {
                write!(f, "abbreviation already used by snippet {existing}")
            }
            StoreError::FolderTooDeep { folder, depth } => {
                write!(f, "folder {folder} would be nested {depth} levels deep")
            }
            StoreError::InvalidData(_) => f.write_str("invalid data"),
            StoreError::Io(e) => write!(f, "i/o error ({:?})", e.kind()),
            StoreError::Sqlite(_) => f.write_str("sqlite error"),
            _ => f.write_str("store error"),
        }
    }
}

/// A fresh id: UUID version 7 from the clock and ten bytes of `RandomState` (seeded by the OS),
/// mixed with a counter so two ids in one millisecond differ.
pub(crate) fn new_id() -> Id {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let state = RandomState::new();
    let a = state.hash_one(n).to_le_bytes();
    let b = state.hash_one(n ^ 0x9E37_79B9_7F4A_7C15).to_le_bytes();
    let mut random = [0_u8; 10];
    random[..8].copy_from_slice(&a);
    random[8..].copy_from_slice(&b[..2]);
    Id::v7(u64::try_from(clock::now_ms()).unwrap_or(0), random)
}

/// The store's state on its thread.
pub(crate) struct StoreCtx {
    pub store: Store,
    pub profile: Id,
    /// The injector's per-app settings, applied from the `injection_overrides` setting.
    pub injection: InjectionSettings,
    library_changed: bool,
    /// The rows the change touched, while every change so far named its rows.
    changed: Option<ChangedRows>,
    settings_changed: bool,
}

impl StoreCtx {
    /// Library content changed, perhaps anywhere: rebuild the snapshot and send
    /// `LibraryChanged` without rows (the client reloads).
    pub(crate) fn library_changed(&mut self) {
        self.library_changed = true;
        self.changed = None;
    }

    /// Library content changed in these rows only: rebuild the snapshot and send
    /// `LibraryChanged` naming them.
    pub(crate) fn rows_changed(&mut self, rows: ChangedRows) {
        if !self.library_changed {
            self.library_changed = true;
            self.changed = Some(ChangedRows::default());
        }
        if let Some(all) = &mut self.changed {
            all.snippets.extend(rows.snippets);
            all.folders.extend(rows.folders);
            all.tags.extend(rows.tags);
        }
    }

    /// Settings or exclusions changed: rebuild the snapshot and send `SettingsChanged`.
    pub(crate) fn settings_changed(&mut self) {
        self.settings_changed = true;
    }
}

/// A job runs on the connection and returns its reply, which is sent only after the change is
/// published, so a client always reads its own write.
type Job = Box<dyn FnOnce(&mut StoreCtx) -> Reply + Send>;
type Reply = Box<dyn FnOnce() + Send>;

/// Messages to the store thread.
pub(crate) enum StoreMsg {
    /// Run a job on the connection.
    Run(Job),
    /// One finished expansion.
    RecordUsage {
        snippet: Id,
        chars_saved: usize,
    },
    Stop,
}

/// Sends work to the store thread.
#[derive(Clone)]
pub(crate) struct StoreHandle(Sender<StoreMsg>);

impl StoreHandle {
    pub(crate) const fn new(tx: Sender<StoreMsg>) -> Self {
        Self(tx)
    }

    pub(crate) fn send(&self, msg: StoreMsg) {
        let _ = self.0.send(msg);
    }

    /// Runs `f` on the store thread and waits for its result; `None` if the thread is gone.
    pub(crate) fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut StoreCtx) -> R + Send + 'static,
    ) -> Option<R> {
        let (tx, rx) = channel();
        self.0
            .send(StoreMsg::Run(Box::new(move |ctx| {
                let result = f(ctx);
                Box::new(move || {
                    let _ = tx.send(result);
                }) as Reply
            })))
            .ok()?;
        rx.recv().ok()
    }
}

/// Makes sure the personal profile exists, stores the exclusions in the form this system's
/// focus inspector reports, and builds the first view.
pub(crate) fn initial_view(
    store: &mut Store,
) -> Result<(Id, View, ExclusionMigration), StoreError> {
    let profile = ensure_personal_profile(store, &mut new_id, clock::timestamp())?;
    let migrated = migrate_exclusions(store, AppIdRule::this_os())?;
    let view = build_view(store, profile, 1)?;
    Ok((profile, view, migrated))
}

/// The thread's loop.
pub(crate) fn run(
    shared: &Shared,
    store: Store,
    profile: Id,
    injection: InjectionSettings,
    rx: &Receiver<StoreMsg>,
) {
    let mut ctx = StoreCtx {
        store,
        profile,
        injection,
        library_changed: false,
        changed: None,
        settings_changed: false,
    };
    loop {
        let msg = rx.recv();
        WakeCounters::bump(&shared.wakes.store);
        match msg {
            Ok(StoreMsg::Run(job)) => {
                let reply = job(&mut ctx);
                publish(shared, &mut ctx);
                reply();
            }
            Ok(StoreMsg::RecordUsage {
                snippet,
                chars_saved,
            }) => {
                let saved = u64::try_from(chars_saved).unwrap_or(u64::MAX);
                if let Err(e) = ctx
                    .store
                    .record_expansion(snippet, saved, clock::timestamp())
                {
                    shared.logger.warn(format_args!(
                        "cannot record usage of {snippet}: {}",
                        Logged(&e)
                    ));
                }
            }
            Ok(StoreMsg::Stop) | Err(_) => break,
        }
    }
    shared.logger.info(format_args!("store closed"));
}

/// After a job: rebuild and announce, if it changed anything.
fn publish(shared: &Shared, ctx: &mut StoreCtx) {
    let (library, settings) = (ctx.library_changed, ctx.settings_changed);
    if !library && !settings {
        return;
    }
    ctx.library_changed = false;
    ctx.settings_changed = false;
    let changed = ctx.changed.take();
    let revision = shared.view.get().revision + 1;
    match build_view(&ctx.store, ctx.profile, revision) {
        Ok(view) => {
            let snapshot = view.snapshot.clone();
            if settings {
                ctx.injection.follow(&view.settings);
            }
            shared.view.set(view);
            shared.to_engine(EngineMsg::Snapshot(snapshot));
            if library {
                shared
                    .clients
                    .broadcast(&EventBody::LibraryChanged { revision, changed });
            }
            if settings {
                shared.clients.broadcast(&EventBody::SettingsChanged {});
                // An exclusion may have changed the verdict for the focused app.
                let _ = shared.focus_tx.send(FocusMsg::Rejudge);
            }
        }
        Err(e) => shared
            .logger
            .error(format_args!("cannot rebuild the snapshot: {}", Logged(&e))),
    }
}

#[cfg(test)]
mod tests {
    use iamb_store::EntityKind;

    use super::*;

    #[test]
    fn a_logged_store_error_holds_no_value() {
        let id = Id::from_bytes([7; 16]);
        let cases = [
            (
                StoreError::InvalidData("column 3 (TEXT): Secret Value".to_owned()),
                "invalid data".to_owned(),
            ),
            (
                StoreError::AbbreviationConflict {
                    key: "secretabbr".to_owned(),
                    existing: id,
                },
                format!("abbreviation already used by snippet {id}"),
            ),
            (
                StoreError::Locked {
                    path: "/home/ada/iamb/run/store.lock".into(),
                },
                "the store is locked by another process".to_owned(),
            ),
            (
                StoreError::Io(std::io::Error::other("/home/ada/secret")),
                "i/o error (Other)".to_owned(),
            ),
            (
                StoreError::NotFound {
                    entity: EntityKind::Snippet,
                    id,
                },
                format!("{} {id} not found", EntityKind::Snippet),
            ),
            (
                StoreError::SchemaTooNew {
                    found: 9,
                    supported: 3,
                },
                "database schema version 9 is newer than this build supports (3)".to_owned(),
            ),
            (
                StoreError::FolderTooDeep {
                    folder: id,
                    depth: 9,
                },
                format!("folder {id} would be nested 9 levels deep"),
            ),
        ];
        for (error, shown) in cases {
            assert_eq!(Logged(&error).to_string(), shown);
            for word in ["Secret", "secretabbr", "ada"] {
                assert!(!Logged(&error).to_string().contains(word));
            }
        }
    }
}
