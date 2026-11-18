//! The store thread: owns the `SQLite` connection (and so `run/store.lock`), performs every
//! write, records usage, and after a change to library content or settings builds the next
//! [`View`] and tells the engine (`SnapshotChanged`), the injector (a changed
//! `injection_overrides` setting, [`injection`](super::injection)) and the clients
//! (`LibraryChanged`, `SettingsChanged`). Usage records do not rebuild anything (ADR-0007).

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use iamb_core::model::Id;
use iamb_ipc::EventBody;
use iamb_store::{Store, StoreError};

use super::injection::InjectionSettings;
use super::{EngineMsg, FocusMsg, Shared, WakeCounters};
use crate::clock;
use crate::library::{View, build_view, ensure_personal_profile};

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
    settings_changed: bool,
}

impl StoreCtx {
    /// Library content changed: rebuild the snapshot and send `LibraryChanged`.
    pub(crate) fn library_changed(&mut self) {
        self.library_changed = true;
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

/// Makes sure the personal profile exists and builds the first view.
pub(crate) fn initial_view(store: &mut Store) -> Result<(Id, View), StoreError> {
    let profile = ensure_personal_profile(store, &mut new_id, clock::timestamp())?;
    let view = build_view(store, profile, 1)?;
    Ok((profile, view))
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
                    shared
                        .logger
                        .warn(format_args!("cannot record usage of {snippet}: {e}"));
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
                    .broadcast(&EventBody::LibraryChanged { revision });
            }
            if settings {
                shared.clients.broadcast(&EventBody::SettingsChanged {});
                // An exclusion may have changed the verdict for the focused app.
                let _ = shared.focus_tx.send(FocusMsg::Rejudge);
            }
        }
        Err(e) => shared
            .logger
            .error(format_args!("cannot rebuild the snapshot: {e}")),
    }
}
