//! The sync loop (§6.7, D11): `hello`, pull, push, pull, and what each answer asks for.
//!
//! [`Agent::sync`] runs one round until nothing is left to do: it pulls until the feed has no
//! more, pushes until the outbox is empty (in a new epoch, what took access away first, §6.7),
//! and pulls again. It handles the refusals of §6.1 and returns the ones the caller must act on
//! (a call without an answer, an expired token, a stop, a sign-out). When to call it again is the
//! caller's: [`crate::schedule::Schedule`] says when.
//!
//! The agent holds no rows. Everything it learns goes to the [`Device`] in the same order the
//! server gave it, and the store applies the rules.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use iamb_core::model::Id;
use iamb_core::sync::{
    AccessRemoval, AnyRow, ApplyMode, ApplyStatus, Change, LostLine, OpStatus, OutboxPhase,
    ReconcileScope, RemoteRow, RowKind, RowRef,
};
use serde_json::{Value, json};

use crate::api::{Api, CallError, DeviceInfo, Entry, Failure, Hello, Refusal, Transport};
use crate::device::{Device, DeviceError};

/// How the agent describes itself and what it speaks.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// The device's name ("Mac").
    pub device_name: String,
    /// `macos`, `windows` or `linux`.
    pub platform: String,
    /// The app's version.
    pub app_version: String,
    /// The oldest and newest protocol versions this build speaks.
    pub protocols: (u32, u32),
    /// Ops per push (200, §6.3).
    pub push_ops: usize,
    /// Entries per pull (500, §6.4).
    pub pull_limit: u32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            device_name: "Computer".to_owned(),
            platform: std::env::consts::OS.to_owned(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocols: (1, 1),
            push_ops: 200,
            pull_limit: 500,
        }
    }
}

/// Why the agent stopped syncing (D12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The server no longer accepts this build's protocol.
    UpgradeRequired,
    /// The server speaks only older versions; `hello` again every hour.
    ServerTooOld,
    /// The plan does not include sync.
    Plan,
    /// The device holds more than the server; `hello` again every hour.
    ServerBehind,
}

/// What ends a round early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundError {
    /// A call got no answer: try again after the backoff (§6.8).
    Retry(CallError),
    /// HTTP 401: refresh the access token and try again (§6.1).
    Unauthorized,
    /// The server asked to wait.
    RateLimited(u64),
    /// Syncing stopped (D12).
    Stopped(Stop),
    /// The device was signed out (D14); `confirmed` when the server's word named the device's
    /// epoch.
    SignedOut {
        /// The sign-out was confirmed by the server.
        confirmed: bool,
        /// The refusal (`device_revoked`, `session_refused`, `device_conflict`).
        reason: String,
    },
    /// The device side failed.
    Device(DeviceError),
    /// The server answered something the agent cannot act on.
    Protocol(String),
}

impl fmt::Display for RoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Retry(e) => write!(f, "no answer: {e}"),
            Self::Unauthorized => f.write_str("the access token is no longer good"),
            Self::RateLimited(ms) => write!(f, "rate limited for {ms} ms"),
            Self::Stopped(s) => write!(f, "stopped: {s:?}"),
            Self::SignedOut { reason, .. } => write!(f, "signed out: {reason}"),
            Self::Device(e) => e.fmt(f),
            Self::Protocol(e) => write!(f, "protocol: {e}"),
        }
    }
}

impl std::error::Error for RoundError {}

impl From<DeviceError> for RoundError {
    fn from(e: DeviceError) -> Self {
        Self::Device(e)
    }
}

/// What a round did, in counts (never content).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoundReport {
    /// Pages pulled.
    pub pages: u32,
    /// Rows applied from pulls and whole reads.
    pub rows: u32,
    /// Changes pushed.
    pub pushed: u32,
    /// Push requests sent.
    pub requests: u32,
    /// A resync ran.
    pub resynced: bool,
    /// The resync found the server restored.
    pub new_epoch: bool,
    /// Conflicts the feed reported.
    pub conflicts: u32,
    /// Access removals made again after a restore.
    pub removals_made_again: u32,
}

/// Why a resync runs (D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resync {
    /// No position yet.
    Initial,
    /// `reset_required: expired`, or the store refused a page three times.
    Expired,
    /// `reset_required: epoch`, or `hello` named another epoch.
    Epoch,
}

/// The sync loop of one signed-in account on one device.
pub struct Agent<T: Transport, D: Device> {
    /// How the agent reaches the server.
    pub transport: T,
    /// The service's side.
    pub device: D,
    config: AgentConfig,
    new_id: Box<dyn FnMut() -> Id + Send>,
    hello: Option<Hello>,
    device_id: Option<Id>,
    position: Option<String>,
    stop: Option<Stop>,
    resync: Option<Resync>,
    needs_access: bool,
    needs_entitlements: bool,
    epoch_order: bool,
    restore_calls: Vec<AccessRemoval>,
    push_failures: u32,
    push_limit: usize,
    refused_pages: u32,
    /// The last epoch any answer named.
    pub last_epoch: Option<String>,
}

fn epoch_of(position: Option<&str>) -> Option<&str> {
    position.and_then(|p| p.split(':').next())
}

fn is_narrowable(e: &CallError) -> bool {
    matches!(e, CallError::Timeout | CallError::Status(_))
}

impl<T: Transport, D: Device> Agent<T, D> {
    /// An agent; `new_id` makes UUID version 7 ids (a new device id after `device_conflict`).
    pub fn new(
        transport: T,
        device: D,
        config: AgentConfig,
        new_id: Box<dyn FnMut() -> Id + Send>,
    ) -> Self {
        let push_limit = config.push_ops;
        Self {
            transport,
            device,
            config,
            new_id,
            hello: None,
            device_id: None,
            position: None,
            stop: None,
            resync: None,
            needs_access: true,
            needs_entitlements: true,
            epoch_order: false,
            restore_calls: Vec::new(),
            push_failures: 0,
            push_limit,
            refused_pages: 0,
            last_epoch: None,
        }
    }

    /// Why the agent stopped, if it did.
    #[must_use]
    pub const fn stopped(&self) -> Option<Stop> {
        self.stop
    }

    /// Clears a stop so that the next round says `hello` again (the hourly retry of D12).
    pub fn clear_stop(&mut self) {
        self.stop = None;
        self.hello = None;
    }

    /// Forgets what the agent had under way (a restart): the next round says `hello` again.
    pub fn forget(&mut self) {
        self.hello = None;
        self.device_id = None;
        self.position = None;
        self.resync = None;
        self.push_failures = 0;
        self.push_limit = self.config.push_ops;
    }

    /// The `hello` answer of this run, once there is one.
    #[must_use]
    pub const fn hello_info(&self) -> Option<&Hello> {
        self.hello.as_ref()
    }

    fn api(&mut self) -> Api<'_, T> {
        let device = self.device_id.unwrap_or_else(|| Id::from_bytes([0; 16]));
        let protocol = self
            .hello
            .as_ref()
            .map_or(self.config.protocols.1, |h| h.protocol);
        Api::new(&mut self.transport, device, protocol)
    }

    /// One round of the loop (§6.7 steps 1 to 5).
    ///
    /// # Errors
    ///
    /// [`RoundError`] for what the caller must act on.
    pub fn sync(&mut self) -> Result<RoundReport, RoundError> {
        if let Some(s) = self.stop {
            return Err(RoundError::Stopped(s));
        }
        let mut report = RoundReport::default();
        self.ensure_hello()?;
        if let Some(reason) = self.resync.take() {
            self.run_resync(reason, &mut report)?;
        }
        self.pull_all(&mut report)?;
        let pushed_before = report.pushed;
        self.push_all(&mut report)?;
        if report.pushed > pushed_before || report.resynced {
            self.pull_all(&mut report)?;
        }
        if self.needs_access {
            self.read_access()?;
        }
        if self.needs_entitlements {
            self.read_entitlements()?;
        }
        Ok(report)
    }

    // ---- refusals -----------------------------------------------------------------------

    /// Acts on a refusal any call can get; returns the error that ends the round.
    fn refused(&mut self, r: &Refusal) -> RoundError {
        if let Some(e) = &r.epoch {
            self.last_epoch = Some(e.clone());
        }
        match r.code.as_str() {
            "upgrade_required" => self.stop_with(Stop::UpgradeRequired),
            "server_too_old" => self.stop_with(Stop::ServerTooOld),
            "plan" => self.stop_with(Stop::Plan),
            "server_behind" => self.stop_with(Stop::ServerBehind),
            "rate_limited" => RoundError::RateLimited(r.retry_after_ms.unwrap_or(1_000)),
            "maintenance" => RoundError::Retry(CallError::Status(503)),
            "device_unknown" => {
                self.hello = None;
                RoundError::Retry(CallError::Network("device_unknown".into()))
            }
            "device_revoked" | "session_refused" => self.sign_out(r),
            other => RoundError::Protocol(format!("unexpected refusal {other}")),
        }
    }

    fn stop_with(&mut self, s: Stop) -> RoundError {
        self.stop = Some(s);
        RoundError::Stopped(s)
    }

    /// D14: signed out without having chosen it. Confirmed when the refusal names the device's
    /// own epoch.
    fn sign_out(&mut self, r: &Refusal) -> RoundError {
        let confirmed = r.epoch.as_deref().is_some()
            && r.epoch.as_deref() == epoch_of(self.position.as_deref());
        if let Err(e) = self.device.signed_out(confirmed, true) {
            return RoundError::Device(e);
        }
        self.hello = None;
        RoundError::SignedOut {
            confirmed,
            reason: r.code.clone(),
        }
    }

    fn fail(&mut self, f: Failure) -> RoundError {
        match f {
            Failure::Call(CallError::Unauthorized) => RoundError::Unauthorized,
            Failure::Call(e) => RoundError::Retry(e),
            Failure::Refused(r) => self.refused(&r),
        }
    }

    // ---- hello --------------------------------------------------------------------------

    fn ensure_hello(&mut self) -> Result<(), RoundError> {
        if self.hello.is_some() {
            return Ok(());
        }
        let info = self.device.info()?;
        let mut device = if let Some(d) = info.device {
            d
        } else {
            let d = (self.new_id)();
            self.device.set_device_id(d)?;
            d
        };
        let mut conflicts = 0;
        let hello = loop {
            let desc = DeviceInfo {
                id: device,
                name: self.config.device_name.clone(),
                platform: self.config.platform.clone(),
                app_version: self.config.app_version.clone(),
            };
            match Api::hello(
                &mut self.transport,
                &desc,
                self.config.protocols,
                info.max_rev,
            ) {
                Ok(h) => break h,
                Err(Failure::Refused(r)) if r.code == "device_conflict" => {
                    conflicts += 1;
                    if conflicts > 1 {
                        // The session itself is the other installation's (C25, D14).
                        self.position.clone_from(&info.position);
                        return Err(self.sign_out(&r));
                    }
                    device = (self.new_id)();
                    self.device.set_device_id(device)?;
                }
                Err(f) => {
                    self.position.clone_from(&info.position);
                    return Err(self.fail(f));
                }
            }
        };
        self.last_epoch = Some(hello.epoch.clone());
        if info.account.is_none() {
            self.device
                .set_account(hello.user, hello.personal_profile, device)?;
        } else if info.account != Some(hello.user) {
            return Err(RoundError::Protocol(
                "the library belongs to another account".into(),
            ));
        }
        self.device_id = Some(device);
        self.position.clone_from(&info.position);
        self.resync = match &info.position {
            None => Some(Resync::Initial),
            Some(p) if epoch_of(Some(p)) != Some(hello.epoch.as_str()) => Some(Resync::Epoch),
            // D14: signing in again resyncs; it is what brings back what a confirmed sign-out
            // removed and settles what an unconfirmed one set aside.
            Some(_) if info.signed_out_at.is_some() => self.resync.or(Some(Resync::Expired)),
            Some(_) => self.resync,
        };
        if conflicts > 0 && self.resync.is_none() {
            self.resync = Some(Resync::Expired);
        }
        self.needs_access = true;
        self.needs_entitlements = true;
        self.hello = Some(hello);
        Ok(())
    }

    fn device_conflict(&mut self) -> Result<(), RoundError> {
        // Another installation uses this id: a new id, `hello` once more, then a resync.
        let d = (self.new_id)();
        self.device.set_device_id(d)?;
        self.hello = None;
        self.ensure_hello()?;
        self.resync = Some(Resync::Expired);
        Ok(())
    }

    // ---- pull ---------------------------------------------------------------------------

    fn pull_all(&mut self, report: &mut RoundReport) -> Result<(), RoundError> {
        for _ in 0..10_000 {
            if let Some(reason) = self.resync.take() {
                self.run_resync(reason, report)?;
            }
            let Some(pos) = self.position.clone() else {
                self.resync = Some(Resync::Initial);
                continue;
            };
            let limit = self.config.pull_limit;
            let page = match self.api().pull(&pos, limit) {
                Ok(p) => p,
                Err(Failure::Refused(r)) if r.code == "reset_required" => {
                    self.resync = Some(if r.reason.as_deref() == Some("epoch") {
                        Resync::Epoch
                    } else {
                        Resync::Expired
                    });
                    continue;
                }
                Err(Failure::Refused(r)) if r.code == "device_conflict" => {
                    self.device_conflict()?;
                    continue;
                }
                Err(f) => return Err(self.fail(f)),
            };
            if let Some(e) = &page.epoch {
                self.last_epoch = Some(e.clone());
            }
            report.pages += 1;
            let mut rows: Vec<RemoteRow> = page.refs.clone();
            let mut access_rows = false;
            for e in &page.entries {
                if let Entry::Row(r) = e {
                    if matches!(
                        r.key.kind,
                        RowKind::FolderShare | RowKind::Membership | RowKind::Profile
                    ) {
                        access_rows = true;
                    }
                    rows.push((**r).clone());
                }
            }
            if !rows.is_empty() {
                let status = self.device.apply(&rows, None, ApplyMode::Feed);
                match status {
                    Ok(s) => {
                        report.rows += u32::try_from(s.len()).unwrap_or(u32::MAX);
                        self.refused_pages = 0;
                    }
                    Err(e) => {
                        // The store refused the page: the position does not move; after three
                        // tries, resync (D7, §6.9).
                        self.refused_pages += 1;
                        if self.refused_pages >= 3 {
                            self.refused_pages = 0;
                            self.resync = Some(Resync::Expired);
                            continue;
                        }
                        return Err(RoundError::Device(e));
                    }
                }
            }
            // Visibility entries one by one, in the order of the feed, after the page's rows (D6).
            for e in &page.entries {
                match e {
                    Entry::FolderVisible(f) => {
                        self.fetch_tree(*f, report)?;
                        access_rows = true;
                    }
                    Entry::FolderRevoked(f, except) => {
                        self.device.purge_folder(*f, except)?;
                        let held: BTreeSet<Id> =
                            self.device.foreign_folders()?.into_iter().collect();
                        for x in except {
                            if !held.contains(x) {
                                self.fetch_tree(*x, report)?;
                            }
                        }
                        access_rows = true;
                    }
                    Entry::Conflict(_) => report.conflicts += 1,
                    Entry::Entitlements => self.needs_entitlements = true,
                    _ => {}
                }
            }
            if access_rows {
                self.needs_access = true;
            }
            self.device.set_position(Some(&page.next))?;
            self.position = Some(page.next.clone());
            if !page.more {
                return Ok(());
            }
        }
        Err(RoundError::Protocol("the feed did not end".into()))
    }

    /// `folder_visible` (D6): the tree whole, between `ReconcileBegin` and `ReconcileEnd`.
    fn fetch_tree(&mut self, folder: Id, report: &mut RoundReport) -> Result<(), RoundError> {
        let token = self
            .device
            .reconcile_begin(ReconcileScope::Folder(folder), None)?;
        let mut cursor: Option<String> = None;
        loop {
            let page = match self.api().fetch_folder(folder, cursor.as_deref()) {
                Ok(p) => p,
                Err(Failure::Refused(r)) if r.code == "not_found" => break,
                Err(f) => return Err(self.fail(f)),
            };
            let n = page.rows.len();
            self.device.apply(&page.rows, None, ApplyMode::Reconcile)?;
            report.rows += u32::try_from(n).unwrap_or(u32::MAX);
            match page.cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        self.device.reconcile_end(&token, None)?;
        Ok(())
    }

    // ---- resync -------------------------------------------------------------------------

    fn run_resync(&mut self, reason: Resync, report: &mut RoundReport) -> Result<(), RoundError> {
        report.resynced = true;
        let old = self.position.clone();
        let first = match self.api().snapshot(old.as_deref(), None) {
            Ok(p) => p,
            Err(Failure::Refused(r)) if r.code == "device_conflict" => {
                self.device_conflict()?;
                return self.run_resync(reason, report);
            }
            Err(f) => {
                self.resync = Some(reason);
                return Err(self.fail(f));
            }
        };
        let start = first
            .start
            .clone()
            .ok_or_else(|| RoundError::Protocol("snapshot without start".into()))?;
        let lost: Option<LostLine> = first.lost.clone().map(|mut l| {
            epoch_of(old.as_deref())
                .unwrap_or_default()
                .clone_into(&mut l.device_epoch);
            l
        });
        let mode = if lost.is_some() {
            ApplyMode::Epoch
        } else {
            ApplyMode::Reconcile
        };
        let token = self
            .device
            .reconcile_begin(ReconcileScope::All, lost.as_ref())?;
        let mut page = first;
        loop {
            let n = page.rows.len();
            self.device.apply(&page.rows, None, mode)?;
            report.rows += u32::try_from(n).unwrap_or(u32::MAX);
            let Some(c) = page.cursor.clone() else { break };
            page = match self.api().snapshot(None, Some(&c)) {
                Ok(p) => p,
                Err(f) => {
                    self.resync = Some(reason);
                    return Err(self.fail(f));
                }
            };
        }
        let removals = self.device.reconcile_end(&token, Some(&start))?;
        self.position = Some(start);
        if lost.is_some() {
            report.new_epoch = true;
            self.epoch_order = true;
            self.restore_calls = removals;
        }
        self.access_check()?;
        self.needs_access = true;
        Ok(())
    }

    /// The access check (D7): folders of other profiles the server no longer shows go.
    fn access_check(&mut self) -> Result<(), RoundError> {
        let visible: BTreeSet<Id> = match self.api().visible_folders() {
            Ok(v) => v.into_iter().collect(),
            Err(f) => return Err(self.fail(f)),
        };
        for f in self.device.foreign_folders()? {
            if !visible.contains(&f) {
                self.device.purge_folder(f, &[])?;
            }
        }
        Ok(())
    }

    // ---- push ---------------------------------------------------------------------------

    fn make_removals_again(&mut self, report: &mut RoundReport) -> Result<(), RoundError> {
        while let Some(entry) = self.restore_calls.first().cloned() {
            let (function, args) = match entry.call.as_str() {
                "remove_member" => (
                    "remove_member",
                    json!({ "group": entry.profile_id, "user": entry.subject }),
                ),
                "leave_group" => ("leave_group", json!({ "group": entry.profile_id })),
                "withdraw_invitation" => ("withdraw_invitation", json!({ "id": entry.subject })),
                "set_member_role" => (
                    "set_member_role",
                    json!({ "group": entry.profile_id, "user": entry.subject, "role": "member" }),
                ),
                "transfer_ownership" => (
                    "transfer_ownership",
                    json!({ "group": entry.profile_id, "user": Value::Null }),
                ),
                "delete_group" => ("delete_group", json!({ "group": entry.profile_id })),
                "revoke_device" => ("revoke_device", json!({ "target": entry.subject })),
                _ => {
                    self.restore_calls.remove(0);
                    continue;
                }
            };
            match self.api().account_call(function, args) {
                Ok(mark) => {
                    let mut e = entry.clone();
                    e.mark = mark;
                    self.device.note_access_removal(&e, false)?;
                    report.removals_made_again += 1;
                }
                Err(Failure::Refused(r))
                    if !matches!(
                        r.code.as_str(),
                        "rate_limited" | "maintenance" | "device_revoked" | "session_refused"
                    ) =>
                {
                    // The user may no longer make it: named in the notice, not made (D7).
                    self.device.note_access_removal(&entry, true)?;
                }
                Err(f) => return Err(self.fail(f)),
            }
            self.restore_calls.remove(0);
        }
        Ok(())
    }

    fn push_all(&mut self, report: &mut RoundReport) -> Result<(), RoundError> {
        let info = self.device.info()?;
        if info.adoption.is_some() {
            // While an adoption is under way nothing is pushed (D9).
            return Ok(());
        }
        if self.epoch_order {
            self.make_removals_again(report)?;
        }
        let phases: &[OutboxPhase] = if self.epoch_order {
            &[
                OutboxPhase::ShareDeletes,
                OutboxPhase::Folders,
                OutboxPhase::NotShares,
                OutboxPhase::All,
            ]
        } else {
            &[OutboxPhase::All]
        };
        for &phase in phases {
            // In a new epoch each step is answered before the next is sent (§6.7).
            let limit_cap = if self.epoch_order && phase == OutboxPhase::Folders {
                1
            } else {
                usize::MAX
            };
            for _ in 0..100_000 {
                if let Some(reason) = self.resync.take() {
                    self.run_resync(reason, report)?;
                }
                let limit = self.push_limit.min(limit_cap).max(1);
                let batch = self.device.outbox_read(limit, phase)?;
                if !batch.behind.is_empty() {
                    self.fetch_rows(&batch.behind)?;
                }
                let changes = batch.changes;
                if changes.is_empty() {
                    break;
                }
                self.push_once(&changes, report)?;
            }
        }
        if self.epoch_order {
            self.epoch_order = false;
            self.device.clear_lost_lines()?;
        }
        Ok(())
    }

    fn push_once(
        &mut self,
        changes: &[Change],
        report: &mut RoundReport,
    ) -> Result<(), RoundError> {
        let Some(pos) = self.position.clone() else {
            self.resync = Some(Resync::Initial);
            return Ok(());
        };
        report.requests += 1;
        match self.api().push(&pos, changes) {
            Ok(pushed) => {
                self.push_failures = 0;
                self.push_limit = self.config.push_ops;
                report.pushed += u32::try_from(changes.len()).unwrap_or(u32::MAX);
                let final_results: Vec<_> = pushed.results.clone();
                let ack = self.device.outbox_ack(&final_results)?;
                if !ack.behind.is_empty() {
                    self.fetch_rows(&ack.behind)?;
                }
                if pushed.results.iter().any(|r| {
                    r.status == OpStatus::Applied
                        && matches!(r.key.kind, RowKind::FolderShare | RowKind::Folder)
                }) {
                    self.needs_access = true;
                }
                Ok(())
            }
            Err(Failure::Refused(r)) if r.code == "reset_required" => {
                self.resync = Some(if r.reason.as_deref() == Some("epoch") {
                    Resync::Epoch
                } else {
                    Resync::Expired
                });
                Ok(())
            }
            Err(Failure::Refused(r)) if r.code == "device_conflict" => self.device_conflict(),
            Err(Failure::Refused(r)) if r.code == "too_large" => {
                self.push_limit = (changes.len() / 2).max(1);
                Ok(())
            }
            // An op failed in the database and the whole request was undone (4.3): half the
            // request next, and a single change that fails so is set aside as blocked (D17), so
            // that the rest of the outbox is not held up by it.
            Err(Failure::Refused(r)) if r.code == "invalid" => {
                if changes.len() <= 1 {
                    let seqs: Vec<i64> = changes.iter().flat_map(|c| c.seqs.clone()).collect();
                    self.device.set_blocked(&seqs, "invalid")?;
                    self.push_limit = self.config.push_ops;
                } else {
                    self.push_limit = (changes.len() / 2).max(1);
                }
                Ok(())
            }
            Err(Failure::Call(e)) if is_narrowable(&e) => {
                // D17: after three failures of the same request, send half; a single change that
                // fails three times is set aside as blocked.
                self.push_failures += 1;
                if self.push_failures < 3 {
                    return Err(RoundError::Retry(e));
                }
                self.push_failures = 0;
                if changes.len() <= 1 {
                    let seqs: Vec<i64> = changes.iter().flat_map(|c| c.seqs.clone()).collect();
                    self.device.set_blocked(&seqs, "server_error")?;
                    self.push_limit = self.config.push_ops;
                    Ok(())
                } else {
                    self.push_limit = (changes.len() / 2).max(1);
                    Err(RoundError::Retry(e))
                }
            }
            Err(f) => Err(self.fail(f)),
        }
    }

    /// D15: rows behind what the device skipped, read with `rows` and applied like pulled rows.
    fn fetch_rows(&mut self, refs: &[RowRef]) -> Result<(), RoundError> {
        for chunk in refs.chunks(500) {
            let rows = match self.api().rows(chunk) {
                Ok(r) => r,
                Err(f) => return Err(self.fail(f)),
            };
            self.apply_following(rows, 3)?;
        }
        Ok(())
    }

    /// Applies rows read with `rows`; a row refused because a row it refers to is missing (a
    /// link whose tag the clean-up of D16 removed meanwhile) is applied again in one batch with
    /// what it refers to, read too, up to `depth` levels (in one batch, so that the clean-up at
    /// its end does not take the tag away again before the link is there).
    fn apply_following(&mut self, rows: Vec<RemoteRow>, depth: u8) -> Result<(), RoundError> {
        let status = self.device.apply(&rows, None, ApplyMode::Feed)?;
        let mut refused: Vec<RemoteRow> = rows
            .into_iter()
            .zip(status)
            .filter(|(_, st)| *st == ApplyStatus::Refused)
            .map(|(r, _)| r)
            .collect();
        for _ in 0..depth {
            let have: BTreeSet<RowRef> = refused.iter().map(|r| r.key).collect();
            let wanted: Vec<RowRef> = refused
                .iter()
                .filter_map(|r| r.row.as_ref())
                .flat_map(AnyRow::references)
                .filter(|k| !have.contains(k))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            if wanted.is_empty() {
                break;
            }
            let mut batch = Vec::new();
            for chunk in wanted.chunks(500) {
                match self.api().rows(chunk) {
                    Ok(r) => batch.extend(r),
                    Err(f) => return Err(self.fail(f)),
                }
            }
            batch.append(&mut refused);
            let status = self.device.apply(&batch, None, ApplyMode::Feed)?;
            refused = batch
                .into_iter()
                .zip(status)
                .filter(|(_, st)| *st == ApplyStatus::Refused)
                .map(|(r, _)| r)
                .collect();
            if refused.is_empty() {
                break;
            }
        }
        Ok(())
    }

    // ---- access and entitlements ----------------------------------------------------------

    fn read_access(&mut self) -> Result<(), RoundError> {
        match self.api().access() {
            Ok(a) => {
                self.device.set_access(&a.roots)?;
                self.needs_access = false;
                Ok(())
            }
            Err(f) => Err(self.fail(f)),
        }
    }

    fn read_entitlements(&mut self) -> Result<(), RoundError> {
        match self.api().entitlements() {
            Ok(map) => {
                self.device.set_entitlements(&map)?;
                self.needs_entitlements = false;
                // A change of plan offers blocked rows again (D5).
                self.device.unblock(Some("plan_limit"))?;
                Ok(())
            }
            Err(f) => Err(self.fail(f)),
        }
    }

    // ---- account calls --------------------------------------------------------------------

    /// An account call (§5.14). A call that takes access away is noted before it is sent and
    /// again with its mark (D7).
    ///
    /// # Errors
    ///
    /// [`RoundError`]; a refusal of the call itself is `Ok(Err(code))`.
    pub fn account_call(
        &mut self,
        function: &str,
        args: Value,
        removal: Option<(Option<Id>, Option<Id>)>,
        now: iamb_core::model::Timestamp,
    ) -> Result<Result<Option<u64>, String>, RoundError> {
        self.ensure_hello()?;
        let entry = removal.map(|(profile_id, subject)| AccessRemoval {
            id: (self.new_id)(),
            call: function.to_owned(),
            profile_id,
            subject,
            mark: None,
            made_at: now,
        });
        if let Some(e) = &entry {
            self.device.note_access_removal(e, false)?;
        }
        match self.api().account_call(function, args) {
            Ok(mark) => {
                if let Some(mut e) = entry {
                    e.mark = mark;
                    self.device.note_access_removal(&e, false)?;
                }
                Ok(Ok(mark))
            }
            Err(Failure::Refused(r))
                if !matches!(
                    r.code.as_str(),
                    "rate_limited"
                        | "maintenance"
                        | "device_revoked"
                        | "session_refused"
                        | "upgrade_required"
                        | "server_behind"
                        | "device_unknown"
                ) =>
            {
                if let Some(e) = entry {
                    self.device.note_access_removal(&e, true)?;
                }
                Ok(Err(r.code))
            }
            // An entry whose answer never came stays, without a mark.
            Err(f) => Err(self.fail(f)),
        }
    }

    /// Any other call of `api` with this device's id and protocol.
    ///
    /// # Errors
    ///
    /// [`RoundError`].
    pub fn raw_call(&mut self, function: &str, args: Value) -> Result<Value, RoundError> {
        self.ensure_hello()?;
        match self.api().raw(function, args) {
            Ok(v) => Ok(v),
            Err(f) => Err(self.fail(f)),
        }
    }
}

/// The rows of a set of changes by key, for diagnostics.
#[must_use]
pub fn change_keys(changes: &[Change]) -> BTreeMap<RowRef, Option<&AnyRow>> {
    changes
        .iter()
        .map(|c| (c.row_ref(), c.row.as_ref()))
        .collect()
}
