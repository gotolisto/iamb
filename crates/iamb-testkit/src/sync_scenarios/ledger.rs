//! The consistency check of a scenario: does the outcome agree with the scenario's own steps and
//! answers, and with what every outcome must satisfy?
//!
//! The check walks the steps once and keeps a ledger:
//!
//! - which version every row has on the server, and whether that version is a tombstone, after
//!   each write that reached the server (an `applied` answer, a push whose answer was held or
//!   lost, an account call answered `ok`, the rows such a write takes with it);
//! - which version of every row each device had seen when it made a change, so that a write is
//!   known to be stale (made without the version it replaces) or not;
//! - which changes are still unsent, under way, refused or blocked on each device.
//!
//! It takes the answers from the scenario and does not derive them: it has no model of
//! permissions, content, the feed or the store. The replay of task 4.4 has; see the
//! documentation of the parent module for where the line is.

use std::collections::{BTreeMap, BTreeSet};

use super::{ACCOUNT_CALLS, ConflictOutcome, RowOutcome, Scenario, Step};

/// Days after which the server purges a tombstone.
const RETENTION_DAYS: u32 = 30;

/// The entities for which a stale write records a conflict; links, attachments, shares and the
/// rows only the server writes never get one.
const CONFLICT_ENTITIES: [&str; 3] = ["snippet", "folder", "tag"];

/// The steps that change a row on the device and queue the change.
const LOCAL_WRITES: [&str; 5] = ["create", "edit", "move", "delete", "undelete"];

/// Checks one scenario that has passed the structural check. `entities` maps every row of the
/// scenario to its entity; `outcomes` holds every device's rows with `same_as` followed.
pub(super) fn check<'a>(
    scenario: &'a Scenario,
    entities: &BTreeMap<&'a str, &'a str>,
    outcomes: &BTreeMap<&'a str, Vec<RowOutcome>>,
) -> Vec<String> {
    let mut ledger = Ledger::new(scenario, entities);
    for step in &scenario.steps {
        ledger.step(step);
    }
    ledger.server_rows();
    ledger.server_history();
    ledger.conflicts();
    ledger.device_changes(outcomes);
    ledger.device_rows(outcomes);
    ledger.problems
}

/// How a device is connected.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Online, syncing by itself.
    Auto,
    /// Online, syncing only when a step says so.
    Manual,
    /// No network.
    Offline,
}

/// A device as the walk knows it.
#[derive(Clone, Copy)]
struct Device<'a> {
    user: &'a str,
    mode: Mode,
    signed_in: bool,
    /// The server refuses its protocol version.
    stopped: bool,
}

impl Device<'_> {
    /// Whether the device talks to the server at all.
    fn connected(&self) -> bool {
        self.signed_in && !self.stopped
    }

    /// Whether the device syncs by itself.
    fn syncing(&self) -> bool {
        self.connected() && self.mode == Mode::Auto
    }
}

/// A row as the server holds it.
#[derive(Clone, Copy)]
struct Held<'a> {
    version: &'a str,
    deleted: bool,
    /// Days since it was deleted.
    days: u32,
}

/// What a device knew of a row when it changed it.
#[derive(Clone, Copy)]
enum Base<'a> {
    /// A write that is not judged against a base: an account call, an adoption.
    Unjudged,
    /// The version the device had seen, if any.
    Seen(Option<&'a str>),
}

/// A change made on a device and not yet answered.
#[derive(Clone)]
struct Change<'a> {
    step: &'a Step,
    id: &'a str,
    row: &'a str,
    base: Base<'a>,
    /// The rows the change takes with it on the server, each with what the device knew of it.
    cascade: Vec<(&'a str, Base<'a>)>,
    /// Whether the change has reached the server (a push whose answer was held or lost).
    arrived: bool,
}

/// One row written on the server.
struct Write<'a> {
    row: &'a str,
    version: &'a str,
    /// The device whose push wrote it; none for an account call.
    device: Option<&'a str>,
    replaced: Option<Held<'a>>,
    /// The writer had not seen the version it replaced.
    stale: bool,
    /// Written because of a change to another row.
    cascade: bool,
    deletes: bool,
}

impl Write<'_> {
    /// The kind of conflict this write records when it replaced a version unseen.
    fn conflict_kind(&self) -> &'static str {
        let over_tombstone = self.replaced.is_some_and(|held| held.deleted);
        match (self.cascade, self.deletes) {
            (true, true) => "deleted_with_folder",
            (true, false) => "folder_restored",
            (false, true) => "delete_over_edit",
            (false, false) if over_tombstone => "edit_over_delete",
            (false, false) => "edit_edit",
        }
    }
}

type Rows<'a> = BTreeMap<&'a str, Held<'a>>;
type Versions<'a> = BTreeMap<&'a str, BTreeSet<&'a str>>;

struct Ledger<'a> {
    scenario: &'a Scenario,
    entities: BTreeMap<&'a str, &'a str>,
    devices: BTreeMap<&'a str, Device<'a>>,
    /// Step id → the step.
    steps: BTreeMap<&'a str, &'a Step>,
    server: Rows<'a>,
    /// Rows the server has held at some time.
    ever: BTreeSet<&'a str>,
    /// The versions the server keeps because a later write replaced them.
    history: Versions<'a>,
    /// Device → the version of each row it has seen.
    seen: BTreeMap<&'a str, BTreeMap<&'a str, &'a str>>,
    /// Changes not yet sent, or sent before a restart, by device and row.
    waiting: BTreeMap<(&'a str, &'a str), Change<'a>>,
    /// Changes sent whose answer has not arrived.
    flying: BTreeMap<(&'a str, &'a str), Change<'a>>,
    writes: Vec<Write<'a>>,
    /// Device, step and row of each refused change.
    refused: Vec<(&'a str, &'a str, &'a str)>,
    /// Device, step and row of each blocked change.
    blocked: Vec<(&'a str, &'a str, &'a str)>,
    /// Step and row of each `applied+conflict`.
    conflict_answers: Vec<(&'a str, &'a str)>,
    backups: BTreeMap<&'a str, (Rows<'a>, Versions<'a>)>,
    /// What the server held before it was restored from a backup.
    lost: Rows<'a>,
    problems: Vec<String>,
}

impl<'a> Ledger<'a> {
    fn new(scenario: &'a Scenario, entities: &BTreeMap<&'a str, &'a str>) -> Self {
        let start = &scenario.start;
        let server: Rows<'a> = start
            .rows
            .iter()
            .filter(|row| row.rev > 0 || row.entity == "invitation")
            .map(|row| {
                let held = Held {
                    version: "start",
                    deleted: row.deleted,
                    days: 0,
                };
                (row.id.as_str(), held)
            })
            .collect();
        let versions: BTreeMap<&str, &str> = server.keys().map(|row| (*row, "start")).collect();
        let devices: BTreeMap<&str, Device<'a>> = start
            .devices
            .iter()
            .map(|spec| {
                let device = Device {
                    user: &spec.user,
                    mode: Mode::Auto,
                    signed_in: spec.signed_in,
                    stopped: false,
                };
                (spec.id.as_str(), device)
            })
            .collect();
        let seen = devices
            .iter()
            .filter(|(_, device)| device.signed_in)
            .map(|(id, _)| (*id, versions.clone()))
            .collect();
        let steps = scenario
            .steps
            .iter()
            .filter_map(|step| Some((step.id.as_deref()?, step)))
            .collect();
        Self {
            scenario,
            entities: entities.clone(),
            devices,
            steps,
            ever: server.keys().copied().collect(),
            server,
            history: BTreeMap::new(),
            seen,
            waiting: BTreeMap::new(),
            flying: BTreeMap::new(),
            writes: Vec::new(),
            refused: Vec::new(),
            blocked: Vec::new(),
            conflict_answers: Vec::new(),
            backups: BTreeMap::new(),
            lost: BTreeMap::new(),
            problems: Vec::new(),
        }
    }

    fn fail(&mut self, what: impl Into<String>) {
        self.problems.push(what.into());
    }

    fn entity(&self, row: &str) -> &'a str {
        self.entities.get(row).copied().unwrap_or("")
    }

    /// The account that made a step, when a device made it.
    fn user_of(&self, step: &str) -> Option<&'a str> {
        let at = self.steps.get(step)?.at.as_str();
        Some(self.devices.get(at)?.user)
    }

    /// The device whose push wrote `version` of `row`; none for a call or the start.
    fn writer_of(&self, row: &str, version: &str) -> Option<&'a str> {
        let write = self
            .writes
            .iter()
            .find(|w| w.row == row && w.version == version)?;
        write.device
    }

    // ---- the walk -------------------------------------------------------------------------------

    fn step(&mut self, step: &'a Step) {
        let at = step.at.as_str();
        let action = step.action.as_str();
        match action {
            "offline" => self.set(at, |device| device.mode = Mode::Offline),
            "online" | "auto" => self.set(at, |device| device.mode = Mode::Auto),
            "manual" => self.set(at, |device| device.mode = Mode::Manual),
            "sign_out" => self.set(at, |device| device.signed_in = false),
            "sign_in" => self.set(at, |device| device.signed_in = true),
            "upgrade" => self.set(at, |device| device.stopped = false),
            "restart" => self.restart(at),
            "advance" => self.advance(step.days.unwrap_or(0)),
            "backup" => {
                if let Some(id) = &step.id {
                    self.backups
                        .insert(id.as_str(), (self.server.clone(), self.history.clone()));
                }
            }
            "restore_backup" => self.restore(step),
            "copy" => {
                if let Some(row) = &step.creates {
                    self.local_write(step, row);
                }
            }
            _ if LOCAL_WRITES.contains(&action) => {
                if let Some(row) = &step.row {
                    self.local_write(step, row);
                }
            }
            _ if ACCOUNT_CALLS.contains(&action) && step.answer.as_deref() == Some("ok") => {
                self.call(step);
            }
            _ => {}
        }
        match step.answer.as_deref() {
            Some("upgrade_required") => self.set(at, |device| device.stopped = true),
            Some("device_revoked" | "error:device_conflict") => {
                self.set(at, |device| device.signed_in = false);
            }
            Some("held" | "lost") if action == "push" => self.send(at),
            _ => {}
        }
        for (row, answer) in &step.answers {
            self.answer(step, row, answer);
        }
        self.look(step);
    }

    fn set(&mut self, device: &str, change: impl FnOnce(&mut Device<'a>)) {
        if let Some(device) = self.devices.get_mut(device) {
            change(device);
        }
    }

    /// A change made on a device: it waits in the outbox, on the base of the row's oldest
    /// unsent change (D1), or on the device's own version under way (D4).
    fn local_write(&mut self, step: &'a Step, row: &'a str) {
        let Some(id) = step.id.as_deref() else {
            return;
        };
        let at = step.at.as_str();
        let seen = self.seen.get(at);
        let look = |row: &str| Base::Seen(seen.and_then(|rows| rows.get(row).copied()));
        let key = (at, row);
        let base = match (self.waiting.get(&key), self.flying.get(&key)) {
            (Some(unsent), _) => unsent.base,
            (None, Some(under_way)) => Base::Seen(Some(under_way.id)),
            (None, None) => look(row),
        };
        let cascade = step
            .cascade
            .iter()
            .map(|row| (row.as_str(), look(row)))
            .collect();
        let change = Change {
            step,
            id,
            row,
            base,
            cascade,
            arrived: false,
        };
        self.waiting.insert(key, change);
    }

    /// A push whose answer is held or lost: the server applied what the device had waiting.
    fn send(&mut self, device: &'a str) {
        let keys: Vec<_> = self
            .waiting
            .keys()
            .filter(|(at, _)| *at == device)
            .copied()
            .collect();
        for key in keys {
            let Some(mut change) = self.waiting.remove(&key) else {
                continue;
            };
            if !change.arrived {
                self.arrive(device, &change);
                change.arrived = true;
            }
            self.flying.insert(key, change);
        }
    }

    /// The agent stops and starts: what was under way is in the outbox again, already sent.
    fn restart(&mut self, device: &'a str) {
        let keys: Vec<_> = self
            .flying
            .keys()
            .filter(|(at, _)| *at == device)
            .copied()
            .collect();
        for key in keys {
            if let Some(change) = self.flying.remove(&key) {
                self.waiting.entry(key).or_insert(change);
            }
        }
    }

    fn answer(&mut self, step: &'a Step, row: &'a str, answer: &str) {
        let at = step.at.as_str();
        let key = (at, row);
        // `replayed`, and whatever `deliver` brings, answers a change that is under way.
        let later = answer == "replayed" || step.action == "deliver";
        let change = if later {
            let sent_before_restart = self.waiting.get(&key).is_some_and(|c| c.arrived);
            match self.flying.remove(&key) {
                Some(change) => Some(change),
                None if sent_before_restart => self.waiting.remove(&key),
                None => None,
            }
        } else {
            self.waiting.remove(&key)
        };
        let Some(change) = change else {
            self.unmatched(step, row, answer);
            return;
        };
        let applied = answer.starts_with("applied") || answer == "replayed";
        if change.arrived && !applied {
            self.fail(format!(
                "{at}: the change {} to {row} reached the server with a push whose answer was \
                 held or lost, which the scenario format takes as applied; it is answered \
                 {answer}",
                change.id
            ));
        }
        if applied && !change.arrived {
            self.arrive(at, &change);
        }
        if answer == "applied+conflict" {
            self.conflict_answers.push((change.id, row));
        } else if answer.starts_with("rejected:") {
            self.refused.push((at, change.id, row));
        } else if answer.starts_with("blocked:") {
            self.blocked.push((at, change.id, row));
        }
    }

    /// An answer for a row the device has no unsent change to: the rows a first sign-in adopts
    /// and pushes, or the rows a device sends again after a restore.
    fn unmatched(&mut self, step: &'a Step, row: &'a str, answer: &str) {
        let at = step.at.as_str();
        match (step.action.as_str(), step.id.as_deref()) {
            ("sign_in", Some(id)) => {
                if answer.starts_with("applied") {
                    self.put(row, id, Some(at), Base::Unjudged, false, false);
                } else if answer.starts_with("rejected:") {
                    self.refused.push((at, id, row));
                } else if answer.starts_with("blocked:") {
                    self.blocked.push((at, id, row));
                }
            }
            _ if step.answer.as_deref() == Some("reset_required:epoch") => {
                self.push_again(row, answer);
            }
            _ => self.fail(format!(
                "{at} is answered {answer} for {row}, to which it has no unsent change"
            )),
        }
    }

    /// A row sent again after a restore: the server has what it held before the restore.
    fn push_again(&mut self, row: &'a str, answer: &str) {
        if !answer.starts_with("applied") {
            return;
        }
        let Some(held) = self.lost.get(row).copied() else {
            self.fail(format!(
                "{row} is sent again after the restore, but the server did not hold it before"
            ));
            return;
        };
        if let Some(restored) = self.server.get(row)
            && restored.version != held.version
        {
            self.history
                .entry(row)
                .or_default()
                .insert(restored.version);
        }
        self.server.insert(row, Held { days: 0, ..held });
        self.ever.insert(row);
    }

    /// A change reaches the server and is applied, with the rows it takes with it.
    fn arrive(&mut self, device: &'a str, change: &Change<'a>) {
        let deletes = change.step.action == "delete";
        self.put(
            change.row,
            change.id,
            Some(device),
            change.base,
            deletes,
            false,
        );
        for (row, base) in &change.cascade {
            self.put(row, change.id, Some(device), *base, deletes, true);
        }
    }

    /// An account call that succeeded: the server writes its rows at once.
    fn call(&mut self, step: &'a Step) {
        let action = step.action.as_str();
        if action == "revoke_device" {
            if let Some(device) = &step.device {
                self.set(device, |device| device.signed_in = false);
            }
            return;
        }
        let Some(id) = step.id.as_deref() else {
            return;
        };
        if action == "resolve" {
            self.resolve(step, id);
            return;
        }
        let deletes = matches!(action, "remove_member" | "leave" | "delete_group");
        if let Some(row) = &step.row {
            self.put(row, id, None, Base::Unjudged, deletes, false);
        }
        if let Some(row) = &step.creates {
            self.put(row, id, None, Base::Unjudged, false, false);
        }
        for row in &step.cascade {
            self.put(row, id, None, Base::Unjudged, deletes, true);
        }
    }

    /// "Restore the other version": the server writes the version the last write replaced.
    fn resolve(&mut self, step: &'a Step, id: &'a str) {
        let (Some("restore"), Some(row)) = (step.choice.as_deref(), step.row.as_deref()) else {
            return;
        };
        let was_deleted = self
            .writes
            .iter()
            .rev()
            .find(|w| w.row == row)
            .is_some_and(|w| w.replaced.is_some_and(|held| held.deleted));
        self.put(row, id, None, Base::Unjudged, was_deleted, false);
    }

    fn put(
        &mut self,
        row: &'a str,
        version: &'a str,
        device: Option<&'a str>,
        base: Base<'a>,
        deletes: bool,
        cascade: bool,
    ) {
        let replaced = self.server.get(row).copied();
        if let Some(old) = replaced
            && old.version != version
        {
            self.history.entry(row).or_default().insert(old.version);
        }
        let stale = match base {
            Base::Unjudged => false,
            Base::Seen(seen) => seen != replaced.map(|held| held.version),
        };
        let held = Held {
            version,
            deleted: deletes,
            days: 0,
        };
        self.server.insert(row, held);
        self.ever.insert(row);
        self.writes.push(Write {
            row,
            version,
            device,
            replaced,
            stale,
            cascade,
            deletes,
        });
    }

    /// Days pass: the server purges tombstones older than the retention, with their history.
    fn advance(&mut self, days: u32) {
        let mut purged = Vec::new();
        for (row, held) in &mut self.server {
            if held.deleted {
                held.days += days;
                if held.days >= RETENTION_DAYS {
                    purged.push(*row);
                }
            }
        }
        for row in purged {
            self.server.remove(row);
            self.history.remove(row);
        }
    }

    fn restore(&mut self, step: &'a Step) {
        let backup = step.to.as_deref().and_then(|to| self.backups.get(to));
        if let Some((server, history)) = backup.cloned() {
            self.lost = std::mem::replace(&mut self.server, server);
            self.history = history;
        }
    }

    /// After a step, every device that syncs by itself, and the device of a `pull`, has seen
    /// the server's rows.
    fn look(&mut self, step: &'a Step) {
        let versions: BTreeMap<&str, &str> = self
            .server
            .iter()
            .map(|(row, held)| (*row, held.version))
            .collect();
        for (id, device) in &self.devices {
            let pulls = *id == step.at && step.action == "pull" && device.connected();
            if device.syncing() || pulls {
                self.seen.insert(id, versions.clone());
            }
        }
    }

    // ---- the outcome ----------------------------------------------------------------------------

    /// The server's row is the last write that reached it.
    fn server_rows(&mut self) {
        for row in &self.scenario.outcome.server.rows {
            let name = row.row.as_str();
            let stated = row.version.as_deref();
            let problem = match (self.server.get(name), row.state.as_str()) {
                (None, "absent") => None,
                (None, _) => Some(format!(
                    "the server ends with {name} at {}, but no write that reached the server \
                     left the row there",
                    stated.unwrap_or("no version")
                )),
                (Some(held), "absent") => Some(format!(
                    "{name} is absent on the server, but {} reached it and no purge is due",
                    held.version
                )),
                (Some(held), _) if stated != Some(held.version) => Some(format!(
                    "the server's {name} is at {}, but the last write that reached the server \
                     is {}",
                    stated.unwrap_or("no version"),
                    held.version
                )),
                (Some(held), state) if (state == "deleted") != held.deleted => Some(format!(
                    "the server's {name} is {state}, but {} left it {}",
                    held.version,
                    if held.deleted { "deleted" } else { "live" }
                )),
                (Some(_), _) => None,
            };
            if let Some(problem) = problem {
                self.fail(problem);
            }
        }
    }

    /// The history holds exactly the versions that later writes replaced.
    fn server_history(&mut self) {
        let mut stated: Versions<'a> = BTreeMap::new();
        for version in &self.scenario.outcome.server.history {
            stated
                .entry(&version.row)
                .or_default()
                .insert(&version.version);
        }
        let rows: Vec<&str> = self.entities.keys().copied().collect();
        for row in rows {
            if self.entity(row) == "invitation" {
                continue;
            }
            let expected = if self.server.contains_key(row) {
                self.history.get(row).cloned().unwrap_or_default()
            } else {
                BTreeSet::new()
            };
            let found = stated.remove(row).unwrap_or_default();
            for version in expected.difference(&found) {
                self.fail(format!(
                    "the history lacks {version} of {row}, which a later write replaced"
                ));
            }
            for version in found.difference(&expected) {
                self.fail(format!(
                    "the history holds {version} of {row}, which no write that reached the \
                     server replaced, or whose row was purged"
                ));
            }
        }
    }

    /// Every recorded conflict follows from a stale write, and every stale write over another
    /// device's version has its conflict.
    fn conflicts(&mut self) {
        let conflicts = &self.scenario.outcome.server.conflicts;
        for conflict in conflicts {
            self.conflict(conflict);
        }
        let mut missing = Vec::new();
        for write in &self.writes {
            let (Some(device), Some(old)) = (write.device, write.replaced) else {
                continue;
            };
            let judged = write.stale
                && CONFLICT_ENTITIES.contains(&self.entity(write.row))
                && old.version != "start"
                && self.writer_of(write.row, old.version) != Some(device)
                && !self.arrangement_only(old.version);
            let offered = conflicts
                .iter()
                .any(|c| c.row == write.row && c.winner == write.version && c.loser == old.version);
            if judged && !offered {
                missing.push(format!(
                    "{} replaced {} of {}, which was written elsewhere and which its writer \
                     had not seen, and no conflict offers {} back: a lost edit",
                    write.version, old.version, write.row, old.version
                ));
            }
        }
        self.problems.append(&mut missing);
        for (step, row) in self.conflict_answers.clone() {
            if !conflicts.iter().any(|c| c.row == row && c.winner == step) {
                self.fail(format!(
                    "{step} was answered applied+conflict, and no conflict on {row} names it as \
                     the winner"
                ));
            }
        }
    }

    /// Whether the step changed only a folder's position, which is replaced without a record.
    fn arrangement_only(&self, step: &str) -> bool {
        self.steps.get(step).is_some_and(|step| {
            step.action == "edit"
                && !step.fields.is_empty()
                && step.fields.keys().all(|field| field == "position")
        })
    }

    fn conflict(&mut self, conflict: &'a ConflictOutcome) {
        let row = conflict.row.as_str();
        let (winner, loser) = (conflict.winner.as_str(), conflict.loser.as_str());
        let Some(write) = self
            .writes
            .iter()
            .rev()
            .find(|w| w.row == row && w.version == winner)
        else {
            self.fail(format!(
                "the conflict on {row} names {winner} as the winner, which wrote no version of \
                 the row on the server"
            ));
            return;
        };
        let mut problems = Vec::new();
        let replaced = write.replaced.map(|held| held.version);
        if replaced != Some(loser) {
            problems.push(format!(
                "the conflict on {row} names {loser} as the loser, but {winner} replaced {}",
                replaced.unwrap_or("nothing")
            ));
        } else if !write.stale {
            problems.push(format!(
                "the conflict on {row}: the device that wrote {winner} had seen {loser}, so \
                 nothing was replaced unseen"
            ));
        } else if write.device.is_some() && self.writer_of(row, loser) == write.device {
            problems.push(format!(
                "the conflict on {row} is between a device and its own earlier write"
            ));
        }
        let kind = write.conflict_kind();
        if conflict.kind != kind {
            problems.push(format!(
                "the conflict on {row} is recorded as {}, but {winner} over {loser} is {kind}",
                conflict.kind
            ));
        }
        let authors: BTreeSet<&str> = [winner, loser]
            .into_iter()
            .chain(conflict.carried.iter().map(String::as_str))
            .filter_map(|step| self.user_of(step))
            .collect();
        let parties: BTreeSet<&str> = conflict.parties.iter().map(String::as_str).collect();
        if authors != parties {
            problems.push(format!(
                "the conflict on {row} is shown to {parties:?}, but the authors of its versions \
                 are {authors:?}"
            ));
        }
        if conflict.state == "open" {
            for (id, device) in &self.devices {
                let shown = self
                    .scenario
                    .outcome
                    .device
                    .get(*id)
                    .is_some_and(|found| found.shown.iter().any(|s| s == "conflict"));
                if device.syncing() && parties.contains(device.user) && !shown {
                    problems.push(format!(
                        "{id}: the open conflict on {row} concerns its user and is not shown"
                    ));
                }
            }
        }
        self.problems.append(&mut problems);
    }

    /// Every change of a device ends somewhere: applied, kept, blocked or still in the outbox.
    fn device_changes(&mut self, outcomes: &BTreeMap<&'a str, Vec<RowOutcome>>) {
        let mut problems = Vec::new();
        for (id, device) in &self.devices {
            let Some(found) = self.scenario.outcome.device.get(*id) else {
                continue;
            };
            let rows = outcomes.get(id).map_or(&[][..], Vec::as_slice);
            let holds = |row: &str| rows.iter().find(|r| r.row == row);
            let kept = |step: &str| found.kept.iter().any(|k| k == step);
            for (_, step, row) in self.refused.iter().filter(|(at, ..)| at == id) {
                if self.entity(row) != "folder_share" && !kept(step) {
                    problems.push(format!(
                        "{id}: the server refused {step}, and the device does not keep it for \
                         the user"
                    ));
                }
            }
            let blocked: Vec<_> = self.blocked.iter().filter(|(at, ..)| at == id).collect();
            for (_, step, row) in &blocked {
                if !found.blocked.iter().any(|b| b == step) {
                    problems.push(format!(
                        "{id}: {step} was answered blocked and is not listed as blocked"
                    ));
                }
                let stays = holds(row)
                    .is_some_and(|r| r.state == "live" && r.version.as_deref() == Some(*step));
                if !stays {
                    problems.push(format!(
                        "{id}: the blocked row {row} must stay on the device as {step} left it"
                    ));
                }
            }
            // What the outbox still holds: changes under way, blocked ones, and unsent ones
            // that are neither kept for the user nor dropped with a row no server ever saw.
            let mut waiting = blocked.len();
            waiting += self.flying.keys().filter(|(at, _)| at == id).count();
            let mut unsent = BTreeSet::new();
            for ((_, row), change) in self.waiting.iter().filter(|((at, _), _)| at == id) {
                if change.arrived {
                    waiting += 1;
                    continue;
                }
                unsent.insert(change.id);
                // A row that was created and deleted again without ever being sent is dropped
                // with its entries (D1): no server saw it and the device no longer holds it.
                let dropped = change.step.action == "delete"
                    && !self.ever.contains(row)
                    && holds(row).is_none_or(|r| r.state == "absent");
                if kept(change.id) || dropped {
                    continue;
                }
                waiting += 1;
                if device.syncing() {
                    problems.push(format!(
                        "{id} ends online with its change {} to {row} unanswered, and neither \
                         kept nor blocked: a lost edit",
                        change.id
                    ));
                }
            }
            if usize::try_from(found.outbox).ok() != Some(waiting) {
                problems.push(format!(
                    "{id}: the outbox is stated to hold {} changes, but {waiting} are under \
                     way, waiting or blocked",
                    found.outbox
                ));
            }
            for step in &found.kept {
                let refused = self.refused.iter().any(|(at, s, _)| at == id && s == step);
                if !refused && !unsent.contains(step.as_str()) {
                    problems.push(format!(
                        "{id}: kept names {step}, which the server did not refuse and which is \
                         not an unsent change"
                    ));
                }
            }
            for step in &found.blocked {
                if !blocked.iter().any(|(_, s, _)| s == step) {
                    problems.push(format!(
                        "{id}: blocked names {step}, which the server did not answer as blocked"
                    ));
                }
            }
            for (list, code) in [(&found.kept, "kept"), (&found.blocked, "blocked")] {
                if !list.is_empty() && !found.shown.iter().any(|s| s == code) {
                    problems.push(format!(
                        "{id}: changes are {code} on the device and `{code}` is not shown"
                    ));
                }
            }
        }
        self.problems.append(&mut problems);
    }

    /// Whether `row` is on `device` in a version of its own that the server does not have yet.
    fn own_unsent(&self, device: &str, row: &RowOutcome) -> bool {
        let Some(version) = row.version.as_deref() else {
            return false;
        };
        let key = (device, row.row.as_str());
        self.blocked
            .iter()
            .any(|(at, step, _)| *at == device && *step == version)
            || self.waiting.get(&key).is_some_and(|c| c.id == version)
            || self.flying.get(&key).is_some_and(|c| c.id == version)
    }

    /// A version names a step that writes the row; a device that ends online holds each row
    /// as the server has it or not at all, and holds what it wrote itself; devices of one
    /// account that end online agree.
    fn device_rows(&mut self, outcomes: &BTreeMap<&'a str, Vec<RowOutcome>>) {
        let scenario = self.scenario;
        let mut authors: Versions<'a> = BTreeMap::new();
        for step in &scenario.steps {
            let Some(id) = step.id.as_deref() else {
                continue;
            };
            let adopted = step.answers.keys().filter(|_| step.action == "sign_in");
            let rows = step.row.iter().chain(&step.creates).chain(&step.cascade);
            for row in rows.chain(adopted) {
                authors.entry(row.as_str()).or_default().insert(id);
            }
        }
        let server = &scenario.outcome.server.rows;
        let mut problems = Vec::new();
        let places = std::iter::once(("the server", server.as_slice()))
            .chain(outcomes.iter().map(|(id, rows)| (*id, rows.as_slice())));
        for (place, rows) in places {
            for row in rows {
                let Some(version) = row.version.as_deref().filter(|v| *v != "start") else {
                    continue;
                };
                let written = authors
                    .get(row.row.as_str())
                    .is_some_and(|steps| steps.contains(version));
                if !written {
                    problems.push(format!(
                        "{place}: {} is at {version}, a step that does not write that row",
                        row.row
                    ));
                }
            }
        }
        let same = |a: &RowOutcome, b: &RowOutcome| a.state == b.state && a.version == b.version;
        let online: Vec<(&str, &str, &[RowOutcome])> = self
            .devices
            .iter()
            .filter(|(_, device)| device.syncing())
            .filter_map(|(id, device)| Some((*id, device.user, outcomes.get(id)?.as_slice())))
            .collect();
        for (id, _, rows) in &online {
            for row in *rows {
                let on_server = server.iter().find(|r| r.row == row.row);
                let agrees = row.state == "absent"
                    || on_server.is_some_and(|r| same(r, row))
                    || self.own_unsent(id, row);
                if !agrees {
                    problems.push(format!(
                        "{id} ends online and holds {} as {}, which is not what the server \
                         holds",
                        row.row,
                        describe(row)
                    ));
                }
            }
        }
        // The device whose push wrote a row's current, live version can read what it wrote.
        for row in server.iter().filter(|r| r.state == "live") {
            let Some(version) = row.version.as_deref() else {
                continue;
            };
            let Some(writer) = self.writer_of(&row.row, version) else {
                continue;
            };
            let lacks = online.iter().any(|(id, _, rows)| {
                *id == writer && rows.iter().any(|r| r.row == row.row && r.state == "absent")
            });
            if lacks {
                problems.push(format!(
                    "{writer} wrote the server's version of {} and ends online without the row",
                    row.row
                ));
            }
        }
        for (index, (first, user, rows)) in online.iter().enumerate() {
            for (other, _, theirs) in online[index + 1..].iter().filter(|(_, u, _)| u == user) {
                for row in *rows {
                    let Some(their) = theirs.iter().find(|r| r.row == row.row) else {
                        continue;
                    };
                    let differ = !same(row, their)
                        && !self.own_unsent(first, row)
                        && !self.own_unsent(other, their);
                    if differ {
                        problems.push(format!(
                            "{first} and {other} are devices of {user}, both online at the end, \
                             and differ on {}",
                            row.row
                        ));
                    }
                }
            }
        }
        self.problems.append(&mut problems);
    }
}

/// A row's state and version in words.
fn describe(row: &RowOutcome) -> String {
    match &row.version {
        Some(version) => format!("{} · {version}", row.state),
        None => row.state.clone(),
    }
}
