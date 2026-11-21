//! The device side: what the agent asks of the service, which owns the store (§7.2).
//!
//! The agent has the network and no store (ADR-0008). In the shipped agent [`IpcDevice`] sends
//! each request to the service over the local endpoint with the role `sync`, which may call the
//! Sync group only (architecture §6.2). The scenario replay and the simulation implement the same
//! trait over `iamb_store::Store` directly, so the rules they exercise are the store's own.

use std::collections::BTreeMap;
use std::fmt;

use iamb_core::model::Id;
use iamb_core::sync::{
    AccessRemoval, AccessRoot, AckOutcome, ApplyMode, ApplyStatus, Change, LostLine, OpResult,
    OutboxPhase, ReconcileScope, RemoteRow, SyncInfo,
};
use iamb_ipc::{RequestBody, ResponseBody, SyncStatus};

/// A failure of the device side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceError(pub String);

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "device: {}", self.0)
    }
}

impl std::error::Error for DeviceError {}

/// The result of a device call.
pub type DeviceResult<T> = Result<T, DeviceError>;

/// What the agent asks of the service. Each call is one store transaction.
pub trait Device {
    /// `SyncState`.
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn info(&mut self) -> DeviceResult<SyncInfo>;
    /// Records the account, its personal profile and the device id of a library that is the
    /// account's already.
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn set_account(&mut self, user: Id, personal_profile: Id, device: Id) -> DeviceResult<()>;
    /// Replaces the device id (C25).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn set_device_id(&mut self, device: Id) -> DeviceResult<()>;
    /// Stores the position.
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn set_position(&mut self, position: Option<&str>) -> DeviceResult<()>;
    /// `OutboxRead` (D1).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn outbox_read(&mut self, limit: usize, phase: OutboxPhase) -> DeviceResult<Vec<Change>>;
    /// `OutboxAck` (D4, D5, D15).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn outbox_ack(&mut self, results: &[OpResult]) -> DeviceResult<AckOutcome>;
    /// `ApplyRemote` (D2, D3, D7).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn apply(
        &mut self,
        rows: &[RemoteRow],
        position: Option<&str>,
        mode: ApplyMode,
    ) -> DeviceResult<Vec<ApplyStatus>>;
    /// `PurgeFolder` (D6).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn purge_folder(&mut self, folder: Id, except: &[Id]) -> DeviceResult<()>;
    /// `ReconcileBegin` (D7).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn reconcile_begin(
        &mut self,
        scope: ReconcileScope,
        lost: Option<&LostLine>,
    ) -> DeviceResult<String>;
    /// `ReconcileEnd` (D7): the access removals a restore lost.
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn reconcile_end(
        &mut self,
        token: &str,
        position: Option<&str>,
    ) -> DeviceResult<Vec<AccessRemoval>>;
    /// Forgets the lines of a restore.
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn clear_lost_lines(&mut self) -> DeviceResult<()>;
    /// `SetAccess`.
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn set_access(&mut self, roots: &[AccessRoot]) -> DeviceResult<()>;
    /// `SetBlocked` (D17).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn set_blocked(&mut self, seqs: &[i64], reason: &str) -> DeviceResult<()>;
    /// Offers blocked changes again.
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn unblock(&mut self, reason: Option<&str>) -> DeviceResult<()>;
    /// `NoteAccessRemoval` (D7).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn note_access_removal(&mut self, entry: &AccessRemoval, refused: bool) -> DeviceResult<()>;
    /// `SignedOut` (D14).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn signed_out(&mut self, confirmed: bool, keep: bool) -> DeviceResult<()>;
    /// The folders the device holds outside the personal profile (the access check, D7).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn foreign_folders(&mut self) -> DeviceResult<Vec<Id>>;
    /// The highest revision the device holds (`hello`'s `max_rev`).
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn max_rev(&mut self) -> DeviceResult<u64>;
    /// `SetEntitlements`.
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn set_entitlements(&mut self, entitlements: &BTreeMap<String, String>) -> DeviceResult<()>;
    /// `SetSyncStatus`.
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    fn set_status(&mut self, status: &SyncStatus) -> DeviceResult<()>;
}

/// The shipped device side: requests to the service over the local endpoint.
pub struct IpcDevice {
    client: iamb_ipc::Client,
}

impl IpcDevice {
    /// Wraps a client connected with the role `sync`.
    #[must_use]
    pub const fn new(client: iamb_ipc::Client) -> Self {
        Self { client }
    }

    /// The next event the service pushed (`OutboxChanged`), waiting at most `timeout`.
    ///
    /// # Errors
    ///
    /// [`DeviceError`].
    pub fn next_event(
        &mut self,
        timeout: Option<std::time::Duration>,
    ) -> DeviceResult<Option<iamb_ipc::EventBody>> {
        self.client
            .next_event(timeout)
            .map_err(|e| DeviceError(e.to_string()))
    }

    fn call(&mut self, body: RequestBody) -> DeviceResult<ResponseBody> {
        self.client
            .call(body)
            .map_err(|e| DeviceError(e.to_string()))
    }
}

fn unexpected(r: &ResponseBody) -> DeviceError {
    DeviceError(format!("unexpected answer {}", r.name()))
}

impl Device for IpcDevice {
    fn info(&mut self) -> DeviceResult<SyncInfo> {
        match self.call(RequestBody::SyncState {})? {
            ResponseBody::SyncState { info } => Ok(info),
            r => Err(unexpected(&r)),
        }
    }

    fn set_account(&mut self, user: Id, personal_profile: Id, device: Id) -> DeviceResult<()> {
        self.call(RequestBody::SetAccount {
            user,
            personal_profile,
            device,
        })
        .map(drop)
    }

    fn set_device_id(&mut self, device: Id) -> DeviceResult<()> {
        self.call(RequestBody::SetDeviceId { device }).map(drop)
    }

    fn set_position(&mut self, position: Option<&str>) -> DeviceResult<()> {
        self.call(RequestBody::SetPosition {
            position: position.map(str::to_owned),
        })
        .map(drop)
    }

    fn outbox_read(&mut self, limit: usize, phase: OutboxPhase) -> DeviceResult<Vec<Change>> {
        match self.call(RequestBody::OutboxRead {
            limit: u32::try_from(limit).unwrap_or(u32::MAX),
            phase: Some(phase),
        })? {
            ResponseBody::Changes { changes } => Ok(changes),
            r => Err(unexpected(&r)),
        }
    }

    fn outbox_ack(&mut self, results: &[OpResult]) -> DeviceResult<AckOutcome> {
        match self.call(RequestBody::OutboxAck {
            results: results.to_vec(),
        })? {
            ResponseBody::SyncAcked { outcome } => Ok(outcome),
            r => Err(unexpected(&r)),
        }
    }

    fn apply(
        &mut self,
        rows: &[RemoteRow],
        position: Option<&str>,
        mode: ApplyMode,
    ) -> DeviceResult<Vec<ApplyStatus>> {
        match self.call(RequestBody::ApplyRemote {
            batch: rows.to_vec(),
            position: position.map(str::to_owned),
            mode,
        })? {
            ResponseBody::Applied { status } => Ok(status),
            r => Err(unexpected(&r)),
        }
    }

    fn purge_folder(&mut self, folder: Id, except: &[Id]) -> DeviceResult<()> {
        self.call(RequestBody::PurgeFolder {
            id: folder,
            except: except.to_vec(),
        })
        .map(drop)
    }

    fn reconcile_begin(
        &mut self,
        scope: ReconcileScope,
        lost: Option<&LostLine>,
    ) -> DeviceResult<String> {
        match self.call(RequestBody::ReconcileBegin {
            scope,
            lost: lost.cloned(),
        })? {
            ResponseBody::ReconcileToken { token } => Ok(token),
            r => Err(unexpected(&r)),
        }
    }

    fn reconcile_end(
        &mut self,
        token: &str,
        position: Option<&str>,
    ) -> DeviceResult<Vec<AccessRemoval>> {
        match self.call(RequestBody::ReconcileEnd {
            token: token.to_owned(),
            position: position.map(str::to_owned),
        })? {
            ResponseBody::AccessRemovals { entries } => Ok(entries),
            r => Err(unexpected(&r)),
        }
    }

    fn clear_lost_lines(&mut self) -> DeviceResult<()> {
        self.call(RequestBody::ClearLostLines {}).map(drop)
    }

    fn set_access(&mut self, roots: &[AccessRoot]) -> DeviceResult<()> {
        self.call(RequestBody::SetAccess {
            roots: roots.to_vec(),
        })
        .map(drop)
    }

    fn set_blocked(&mut self, seqs: &[i64], reason: &str) -> DeviceResult<()> {
        self.call(RequestBody::SetBlocked {
            seqs: seqs.to_vec(),
            reason: reason.to_owned(),
        })
        .map(drop)
    }

    fn unblock(&mut self, reason: Option<&str>) -> DeviceResult<()> {
        self.call(RequestBody::Unblock {
            reason: reason.map(str::to_owned),
        })
        .map(drop)
    }

    fn note_access_removal(&mut self, entry: &AccessRemoval, refused: bool) -> DeviceResult<()> {
        self.call(RequestBody::NoteAccessRemoval {
            entry: entry.clone(),
            refused,
        })
        .map(drop)
    }

    fn signed_out(&mut self, confirmed: bool, keep: bool) -> DeviceResult<()> {
        self.call(RequestBody::SignedOut { confirmed, keep })
            .map(drop)
    }

    fn foreign_folders(&mut self) -> DeviceResult<Vec<Id>> {
        match self.call(RequestBody::ForeignFolders {})? {
            ResponseBody::Ids { ids } => Ok(ids),
            r => Err(unexpected(&r)),
        }
    }

    fn max_rev(&mut self) -> DeviceResult<u64> {
        match self.call(RequestBody::SyncState {})? {
            ResponseBody::SyncState { info } => Ok(info.max_rev),
            r => Err(unexpected(&r)),
        }
    }

    fn set_entitlements(&mut self, entitlements: &BTreeMap<String, String>) -> DeviceResult<()> {
        self.call(RequestBody::SetEntitlements {
            entitlements: entitlements.clone(),
        })
        .map(drop)
    }

    fn set_status(&mut self, status: &SyncStatus) -> DeviceResult<()> {
        self.call(RequestBody::SetSyncStatus {
            status: status.clone(),
        })
        .map(drop)
    }
}
