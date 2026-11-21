//! The device side over a store in process (feature `model`): what the service's Sync-group
//! handlers do, one store transaction per call, on a clock the caller sets.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use iamb_core::model::{Id, Timestamp};
use iamb_core::sync::{
    AccessRemoval, AccessRoot, AckOutcome, ApplyMode, ApplyStatus, Change, LostLine, OpResult,
    OutboxPhase, ReconcileScope, RemoteRow, SyncInfo,
};
use iamb_ipc::SyncStatus;
use iamb_store::Store;

use crate::device::{Device, DeviceError, DeviceResult};

/// A store and the device's clock (milliseconds; a skewed clock is the device's own).
pub struct StoreDevice {
    /// The store.
    pub store: Store,
    /// The device's clock.
    pub clock: Rc<Cell<i64>>,
    /// Entitlements last set.
    pub entitlements: BTreeMap<String, String>,
    /// The status last set.
    pub status: Option<SyncStatus>,
}

impl StoreDevice {
    /// A device over `store` with `clock`.
    #[must_use]
    pub fn new(store: Store, clock: Rc<Cell<i64>>) -> Self {
        Self {
            store,
            clock,
            entitlements: BTreeMap::new(),
            status: None,
        }
    }

    fn now(&self) -> Timestamp {
        Timestamp(self.clock.get())
    }
}

fn e(err: iamb_store::StoreError) -> DeviceError {
    DeviceError(err.to_string())
}

impl Device for StoreDevice {
    fn info(&mut self) -> DeviceResult<SyncInfo> {
        self.store.sync_info().map_err(e)
    }

    fn set_account(&mut self, user: Id, personal_profile: Id, device: Id) -> DeviceResult<()> {
        self.store
            .set_account(user, personal_profile, device)
            .map_err(e)
    }

    fn set_device_id(&mut self, device: Id) -> DeviceResult<()> {
        self.store.set_device_id(device).map_err(e)
    }

    fn set_position(&mut self, position: Option<&str>) -> DeviceResult<()> {
        let now = self.now();
        self.store.set_sync_position(position, now).map_err(e)
    }

    fn outbox_read(&mut self, limit: usize, phase: OutboxPhase) -> DeviceResult<Vec<Change>> {
        self.store.sync_outbox_read(limit, phase).map_err(e)
    }

    fn outbox_ack(&mut self, results: &[OpResult]) -> DeviceResult<AckOutcome> {
        let now = self.now();
        self.store.sync_outbox_ack(results, now).map_err(e)
    }

    fn apply(
        &mut self,
        rows: &[RemoteRow],
        position: Option<&str>,
        mode: ApplyMode,
    ) -> DeviceResult<Vec<ApplyStatus>> {
        let now = self.now();
        self.store.sync_apply(rows, position, mode, now).map_err(e)
    }

    fn purge_folder(&mut self, folder: Id, except: &[Id]) -> DeviceResult<()> {
        let now = self.now();
        self.store.purge_folder(folder, except, now).map_err(e)
    }

    fn reconcile_begin(
        &mut self,
        scope: ReconcileScope,
        lost: Option<&LostLine>,
    ) -> DeviceResult<String> {
        self.store.reconcile_begin(scope, lost).map_err(e)
    }

    fn reconcile_end(
        &mut self,
        token: &str,
        position: Option<&str>,
    ) -> DeviceResult<Vec<AccessRemoval>> {
        let now = self.now();
        self.store.reconcile_end(token, position, now).map_err(e)
    }

    fn clear_lost_lines(&mut self) -> DeviceResult<()> {
        self.store.clear_lost_lines().map_err(e)
    }

    fn set_access(&mut self, roots: &[AccessRoot]) -> DeviceResult<()> {
        self.store.set_access(roots).map_err(e)
    }

    fn set_blocked(&mut self, seqs: &[i64], reason: &str) -> DeviceResult<()> {
        self.store.set_blocked(seqs, reason).map_err(e)
    }

    fn unblock(&mut self, reason: Option<&str>) -> DeviceResult<()> {
        self.store.unblock(reason).map(drop).map_err(e)
    }

    fn note_access_removal(&mut self, entry: &AccessRemoval, refused: bool) -> DeviceResult<()> {
        self.store.note_access_removal(entry, refused).map_err(e)
    }

    fn signed_out(&mut self, confirmed: bool, keep: bool) -> DeviceResult<()> {
        let now = self.now();
        self.store.signed_out(confirmed, keep, now).map_err(e)
    }

    fn foreign_folders(&mut self) -> DeviceResult<Vec<Id>> {
        self.store.foreign_folders().map_err(e)
    }

    fn max_rev(&mut self) -> DeviceResult<u64> {
        self.store.max_rev().map_err(e)
    }

    fn set_entitlements(&mut self, entitlements: &BTreeMap<String, String>) -> DeviceResult<()> {
        self.entitlements.clone_from(entitlements);
        Ok(())
    }

    fn set_status(&mut self, status: &SyncStatus) -> DeviceResult<()> {
        self.status = Some(status.clone());
        Ok(())
    }
}
