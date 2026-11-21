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
//! - which changes are still unsent, under way, refused or blocked on each device;
//! - what each version of a row holds, as far as the scenario's own steps say it: the fields a
//!   step wrote, and who wrote a snippet's content (the `facts` module);
//! - around a restore of the server: which versions the backup lacks, which version of each row
//!   each device held at that moment, which removals were made after the backup, and in which
//!   order the lost versions of a row had been written;
//! - which devices were signed out, when, and whether they chose it; and which memberships and
//!   shares the scenario has, so that a row on a device can be traced to what lets its user
//!   read the row's profile.
//!
//! It takes the answers from the scenario and does not derive them: it has no model of
//! permissions, the feed or the store. The replay of task 4.4 has; see the documentation of the
//! parent module for where the line is.

mod facts;

use std::collections::{BTreeMap, BTreeSet};

use self::facts::Facts;
use super::{ACCOUNT_CALLS, ConflictOutcome, RowOutcome, Scenario, Step};

/// Days after which the server purges a tombstone.
const RETENTION_DAYS: u32 = 30;

/// The entities for which a stale write records a conflict; links, attachments, shares and the
/// rows only the server writes never get one.
const CONFLICT_ENTITIES: [&str; 3] = ["snippet", "folder", "tag"];

/// The steps that change a row on the device and queue the change.
const LOCAL_WRITES: [&str; 5] = ["create", "edit", "move", "delete", "undelete"];

/// The account calls that end a membership. After a restore of the server that lost one, the
/// device that made it makes it again (D7).
const REMOVALS: [&str; 3] = ["remove_member", "leave", "delete_group"];

/// The rows only the server writes: no device sends them again after a restore.
const SERVER_WRITTEN: [&str; 2] = ["membership", "invitation"];

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
    ledger.shown_restored();
    ledger.device_rows(outcomes);
    ledger.signed_out(outcomes);
    ledger.reach(outcomes);
    ledger.after_restore(outcomes);
    ledger.stated_fields(outcomes);
    ledger.reviews(outcomes);
    ledger.precedence(outcomes);
    ledger.own_profile(outcomes);
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

/// What the ledger held when a backup was taken.
#[derive(Clone)]
struct Backup<'a> {
    server: Rows<'a>,
    history: Versions<'a>,
    /// How many writes, conflict answers and removals there were.
    writes: usize,
    conflict_answers: usize,
    removals: usize,
}

/// A membership ended by an account call.
#[derive(Clone, Copy)]
struct Removal<'a> {
    step: &'a str,
    action: &'a str,
    row: &'a str,
    device: &'a str,
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
    backups: BTreeMap<&'a str, Backup<'a>>,
    /// The writes a restore of the server lost, by their place in `writes`, and the row and
    /// version of each.
    lost_writes: BTreeSet<usize>,
    lost_versions: BTreeSet<(&'a str, &'a str)>,
    /// Device → the version of each row it held when the server was restored.
    held: BTreeMap<&'a str, BTreeMap<&'a str, &'a str>>,
    /// Row and version of each write since the server was last restored.
    since_restore: BTreeSet<(&'a str, &'a str)>,
    /// The memberships account calls ended, and those of them a restore lost.
    removals: Vec<Removal<'a>>,
    lost_removals: Vec<Removal<'a>>,
    /// Row, the version that stays and the version given back too late, for each re-push
    /// answered `superseded`.
    offers: Vec<(&'a str, &'a str, &'a str)>,
    /// Row → the latest of its lost versions, by its place in `writes`, that the server has
    /// taken in again since the restore.
    taken_in: BTreeMap<&'a str, usize>,
    /// The devices that have found the restore since it happened.
    resynced: BTreeSet<&'a str>,
    /// Device → how many writes had reached the server when it was signed out, and whether
    /// that was a session refused without the device or its user having chosen it. Removed
    /// when the device signs in again.
    signed_out_at: BTreeMap<&'a str, (usize, bool)>,
    /// Membership or share → its group, and membership → its account.
    groups: BTreeMap<&'a str, &'a str>,
    members: BTreeMap<&'a str, &'a str>,
    /// What each version of each row holds, as far as the steps say.
    facts: BTreeMap<(&'a str, &'a str), Facts<'a>>,
    /// Row → the profile that owns it.
    profiles: BTreeMap<&'a str, &'a str>,
    /// Device, row and content of each review a user approved.
    approved: Vec<(&'a str, &'a str, &'a str)>,
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
        let signed_out_at = devices
            .iter()
            .filter(|(_, device)| !device.signed_in)
            .map(|(id, _)| (*id, (0, false)))
            .collect();
        let (groups, members) = facts::row_groups(scenario);
        let mut ledger = Self {
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
            lost_writes: BTreeSet::new(),
            lost_versions: BTreeSet::new(),
            held: BTreeMap::new(),
            since_restore: BTreeSet::new(),
            removals: Vec::new(),
            lost_removals: Vec::new(),
            offers: Vec::new(),
            taken_in: BTreeMap::new(),
            resynced: BTreeSet::new(),
            signed_out_at,
            groups,
            members,
            facts: BTreeMap::new(),
            profiles: facts::row_profiles(scenario),
            approved: Vec::new(),
            problems: Vec::new(),
        };
        ledger.start_facts();
        ledger
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
            "sign_out" => self.sign_out(at, false),
            "sign_in" => {
                self.set(at, |device| device.signed_in = true);
                self.signed_out_at.remove(at);
            }
            "upgrade" => self.set(at, |device| device.stopped = false),
            "restart" => self.restart(at),
            "advance" => self.advance(step.days.unwrap_or(0)),
            "backup" => self.backup(step),
            "restore_backup" => self.restore(step),
            "replay_calls" => self.replay(step),
            "approve" => self.note_approval(step),
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
            Some("upgrade_required" | "error:server_behind") => {
                self.set(at, |device| device.stopped = true);
            }
            Some("device_revoked" | "error:device_conflict") => self.sign_out(at, false),
            Some("session_refused") => self.sign_out(at, true),
            Some("reset_required:epoch") => {
                self.resynced.insert(at);
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

    /// A device is signed out: by its user, by a revocation, or (`refused`) because its session
    /// was no longer good without anybody on the device having chosen that. What reaches the
    /// server from now on is not on this device until it signs in again.
    fn sign_out(&mut self, device: &str, refused: bool) {
        let Some((id, found)) = self.devices.get_key_value(device) else {
            return;
        };
        let (id, was_signed_in) = (*id, found.signed_in);
        if was_signed_in {
            self.signed_out_at.insert(id, (self.writes.len(), refused));
        }
        self.set(device, |device| device.signed_in = false);
    }

    /// A change made on a device: it waits in the outbox, on the base of the row's oldest
    /// unsent change (D1), or on the device's own version under way (D4).
    fn local_write(&mut self, step: &'a Step, row: &'a str) {
        let Some(id) = step.id.as_deref() else {
            return;
        };
        self.note_local(step, row, id);
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
        if answer.ends_with("+adjusted")
            && let Some(facts) = self.facts.get_mut(&(row, change.id))
        {
            // The server put the folder somewhere else than the device asked (C8).
            facts.fields.remove("parent");
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
        // A device that has found the restore sends again what it held and the restore lost: in
        // the step that finds it, or in a later one (a share held back until a removal is made
        // again, D7).
        let holds_lost = self
            .held
            .get(at)
            .and_then(|rows| rows.get(row))
            .is_some_and(|version| self.lost_versions.contains(&(row, *version)));
        let again = step.answer.as_deref() == Some("reset_required:epoch")
            || (self.resynced.contains(at) && holds_lost);
        match (step.action.as_str(), step.id.as_deref()) {
            _ if again => self.push_again(at, row, answer),
            ("sign_in", Some(id)) => {
                if answer.starts_with("applied") {
                    self.put(row, id, Some(at), Base::Unjudged, false, false);
                } else if answer.starts_with("rejected:") {
                    self.refused.push((at, id, row));
                } else if answer.starts_with("blocked:") {
                    self.blocked.push((at, id, row));
                }
            }
            _ => self.fail(format!(
                "{at} is answered {answer} for {row}, to which it has no unsent change"
            )),
        }
    }

    /// A row sent again after a restore of the server (C18): the version the device held at
    /// that moment, which the backup lacks, reaches the server as a write of that device.
    fn push_again(&mut self, device: &'a str, row: &'a str, answer: &str) {
        let held = self
            .held
            .get(device)
            .and_then(|rows| rows.get(row))
            .copied();
        let Some(version) = held.filter(|v| self.lost_versions.contains(&(row, *v))) else {
            if answer != "unchanged" {
                self.fail(format!(
                    "{device} sends {row} again after the restore, but it held no version of \
                     the row that the restore lost"
                ));
            }
            return;
        };
        let current = self.server.get(row).copied();
        // Where this version stood in the history the restore lost, and the latest of that
        // history the row has taken in again (kept across ordinary writes, C18).
        let place = self.lost_place(row, version);
        let taken_in = self.taken_in.get(row).copied();
        if answer == "superseded" {
            // A later version of the same lost history came back first: this one is kept in
            // the history and offered back.
            if let Some(stays) = current.filter(|held| held.version != version) {
                if place.is_some() && taken_in < place {
                    self.fail(format!(
                        "{device} gives {version} of {row} back and is answered superseded, but \
                         {version} is later than every lost version the row has taken in: the \
                         later one stays (C18)"
                    ));
                }
                self.history.entry(row).or_default().insert(version);
                self.offers.push((row, stays.version, version));
            }
            return;
        }
        if answer.starts_with("rejected:") {
            // A refused re-push is kept where the device's own user wrote that version, and
            // dropped otherwise: it is its writer's to give back (D7).
            if self.user_of(version) == self.devices.get(device).map(|d| d.user) {
                self.refused.push((device, version, row));
            }
            return;
        }
        if !answer.starts_with("applied") {
            return;
        }
        if place.is_some() && taken_in > place {
            self.fail(format!(
                "{device} gives {version} of {row} back over a later version of the history the \
                 restore lost, which the row had taken in already: the later one stays, and the \
                 earlier one is answered superseded (C18)"
            ));
        }
        if let Some(old) = current
            && old.version != version
        {
            self.history.entry(row).or_default().insert(old.version);
        }
        let deletes = self
            .writes
            .iter()
            .rev()
            .find(|w| w.row == row && w.version == version)
            .is_some_and(|w| w.deletes);
        if !deletes && self.entity(row) == "folder_share" {
            self.removals_first(device, row);
        }
        if let Some(place) = place {
            let latest = self.taken_in.entry(row).or_insert(place);
            *latest = (*latest).max(place);
        }
        // Whatever was written since the restore was written without the lost version.
        let stale = current.is_some_and(|old| {
            old.version != version && self.since_restore.contains(&(row, old.version))
        });
        let held = Held {
            version,
            deleted: deletes,
            days: 0,
        };
        self.server.insert(row, held);
        self.ever.insert(row);
        self.since_restore.insert((row, version));
        self.writes.push(Write {
            row,
            version,
            device: Some(device),
            replaced: current,
            stale,
            cascade: false,
            deletes,
        });
        if answer == "applied+conflict" {
            self.conflict_answers.push((version, row));
        }
    }

    /// Where `version` of `row` stood among the writes the restore lost.
    fn lost_place(&self, row: &str, version: &str) -> Option<usize> {
        self.writes.iter().enumerate().position(|(index, w)| {
            w.row == row && w.version == version && self.lost_writes.contains(&index)
        })
    }

    /// A device gives a share back only after it has made again the removals it had made from
    /// that group and the restore lost: otherwise the folder reaches the person who had been
    /// removed (D7).
    fn removals_first(&mut self, device: &str, share: &str) {
        let Some(group) = self.groups.get(share).copied() else {
            return;
        };
        let mut problems = Vec::new();
        for removal in &self.lost_removals {
            let same_group = self.groups.get(removal.row).copied() == Some(group);
            let still = self.server.get(removal.row).is_some_and(|h| !h.deleted);
            if removal.device == device && same_group && still {
                problems.push(format!(
                    "{device} gives the share {share} back before it has made again {}, its \
                     removal of {} that the restore lost: what took access away goes first (D7)",
                    removal.step, removal.row
                ));
            }
        }
        self.problems.append(&mut problems);
    }

    /// The operator's step of the restore procedure: the calls that took access away after the
    /// backup are made again from the function log, before any device is let in (C18).
    fn replay(&mut self, step: &'a Step) {
        let Some(id) = step.id.as_deref() else {
            return;
        };
        for row in &step.cascade {
            let lost = self.lost_removals.iter().any(|removal| {
                let with = self.steps.get(removal.step).map(|s| s.cascade.as_slice());
                removal.row == row || with.is_some_and(|rows| rows.contains(row))
            });
            if lost {
                self.put(row, id, None, Base::Unjudged, true, false);
            } else {
                self.fail(format!(
                    "the operator's replay {id} ends {row}, which no call that the restore lost \
                     had ended: the replay makes again what took access away, and nothing else"
                ));
            }
        }
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
                self.sign_out(device, false);
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
        let deletes = REMOVALS.contains(&action);
        if let Some(row) = &step.row {
            self.put(row, id, None, Base::Unjudged, deletes, false);
            if deletes {
                self.removals.push(Removal {
                    step: id,
                    action,
                    row,
                    device: &step.at,
                });
            }
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
        let restored = self
            .writes
            .iter()
            .rev()
            .find(|w| w.row == row)
            .and_then(|w| w.replaced);
        if let Some(version) = restored {
            self.carry(row, version.version, id);
        }
        let was_deleted = restored.is_some_and(|held| held.deleted);
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
        // A change a device made on top of a version the restore lost stands for that version:
        // the row has taken it in (C18, `repush: "base"`).
        if let Base::Seen(Some(seen)) = base
            && let Some(place) = self.lost_place(row, seen)
        {
            let latest = self.taken_in.entry(row).or_insert(place);
            *latest = (*latest).max(place);
        }
        if cascade
            && device.is_some()
            && let Some(old) = replaced
        {
            // A row a push takes along keeps what the server held of it (C6, C7).
            self.carry(row, old.version, version);
        }
        let held = Held {
            version,
            deleted: deletes,
            days: 0,
        };
        self.server.insert(row, held);
        self.ever.insert(row);
        self.since_restore.insert((row, version));
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

    fn backup(&mut self, step: &'a Step) {
        let Some(id) = step.id.as_deref() else {
            return;
        };
        let backup = Backup {
            server: self.server.clone(),
            history: self.history.clone(),
            writes: self.writes.len(),
            conflict_answers: self.conflict_answers.len(),
            removals: self.removals.len(),
        };
        self.backups.insert(id, backup);
    }

    /// The server is restored from a backup: every write since is lost, with the history and
    /// the conflicts of that time. Each device still holds what it held.
    fn restore(&mut self, step: &'a Step) {
        let backup = step.to.as_deref().and_then(|to| self.backups.get(to));
        let Some(backup) = backup.cloned() else {
            return;
        };
        self.server = backup.server;
        self.history = backup.history;
        for (index, write) in self.writes.iter().enumerate().skip(backup.writes) {
            self.lost_writes.insert(index);
            self.lost_versions.insert((write.row, write.version));
        }
        self.conflict_answers.truncate(backup.conflict_answers);
        let lost = self
            .removals
            .split_off(backup.removals.min(self.removals.len()));
        self.lost_removals.extend(lost);
        self.held = self.seen.clone();
        self.since_restore.clear();
        self.taken_in.clear();
        self.resynced.clear();
    }

    /// After a step, every device that syncs by itself, and the device of a `pull`, has seen
    /// the server's rows.
    fn look(&mut self, step: &'a Step) {
        let versions: BTreeMap<&str, &str> = self
            .server
            .iter()
            .map(|(row, held)| (*row, held.version))
            .collect();
        // A pull whose answer is lost shows the device nothing.
        let answered = step.answer.as_deref() != Some("lost");
        for (id, device) in &self.devices {
            let pulls = *id == step.at && step.action == "pull" && device.connected() && answered;
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
        for (index, write) in self.writes.iter().enumerate() {
            let (Some(device), Some(old)) = (write.device, write.replaced) else {
                continue;
            };
            // The conflicts of the time a restore lost are lost with it.
            let judged = write.stale
                && !self.lost_writes.contains(&index)
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
        for (row, stays, late) in &self.offers {
            let offered = conflicts.iter().any(|c| {
                c.row == *row
                    && c.winner == *stays
                    && (c.loser == *late || c.carried.iter().any(|carried| carried == late))
            });
            if CONFLICT_ENTITIES.contains(&self.entity(row)) && !offered {
                missing.push(format!(
                    "{late} of {row} was given back after the restore when {stays} was already \
                     there, and no conflict offers {late} back: a lost edit"
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
        if self.offers.contains(&(row, winner, loser)) {
            // A version given back after a later one of the same lost history (C18).
            return;
        }
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
                // What a device held when the server was restored, and the restore lost, is
                // unsent again (D7); it is kept where the device's own user wrote it.
                let given_back = self.user_of(step) == Some(device.user)
                    && self.held.get(id).is_some_and(|rows| {
                        rows.iter().any(|(row, version)| {
                            version == step && self.lost_versions.contains(&(*row, *version))
                        })
                    });
                if !refused && !given_back && !unsent.contains(step.as_str()) {
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

    /// A device says that the server was restored only in a scenario that restores it: nothing
    /// a client claims makes other devices resync (C18).
    fn shown_restored(&mut self) {
        let steps = &self.scenario.steps;
        if steps.iter().any(|step| step.action == "restore_backup") {
            return;
        }
        for (id, found) in &self.scenario.outcome.device {
            if found.shown.iter().any(|shown| shown == "restored") {
                self.problems.push(format!(
                    "{id} shows that the server was restored, and no step of the scenario \
                     restores it"
                ));
            }
        }
    }

    /// A device that was signed out or revoked, and not signed in again, receives nothing: it
    /// holds no version that reached the server afterwards, except what it wrote itself.
    fn signed_out(&mut self, outcomes: &BTreeMap<&'a str, Vec<RowOutcome>>) {
        let mut problems = Vec::new();
        for (id, (since, _)) in &self.signed_out_at {
            for row in outcomes.get(id).into_iter().flatten() {
                let Some(version) = row.version.as_deref().filter(|v| *v != "start") else {
                    continue;
                };
                let own = self.steps.get(version).is_some_and(|step| step.at == *id);
                let first = self
                    .writes
                    .iter()
                    .position(|w| w.row == row.row && w.version == version);
                if !own && first.is_some_and(|index| index >= *since) {
                    problems.push(format!(
                        "{id} was signed out before {version} of {} reached the server, was \
                         not signed in again, and ends holding it",
                        row.row
                    ));
                }
            }
        }
        self.problems.append(&mut problems);
    }

    /// Whether `user` is a member of `group`: now, by a membership that is live on the server,
    /// or at any time of the scenario.
    fn member(&self, user: &str, group: &str, now: bool) -> bool {
        self.members.iter().any(|(row, account)| {
            *account == user
                && self.groups.get(row).copied() == Some(group)
                && (!now || self.server.get(row).is_some_and(|held| !held.deleted))
        })
    }

    /// Whether a membership or a share lets `user` read rows of `profile`: now, or at any time
    /// of the scenario. Which folders of the profile a share covers is not known here.
    fn reaches(&self, user: &str, profile: &str, now: bool) -> bool {
        let profiles = &self.scenario.start.profiles;
        let Some(found) = profiles.iter().find(|p| p.id == profile) else {
            return true;
        };
        if found.owner == user || found.local_to.is_some() {
            return true;
        }
        if found.kind != "personal" {
            return self.member(user, profile, now);
        }
        self.entities.iter().any(|(row, entity)| {
            *entity == "folder_share"
                && self.profiles.get(row).copied() == Some(profile)
                && (!now || self.server.get(row).is_some_and(|held| !held.deleted))
                && self
                    .groups
                    .get(row)
                    .is_some_and(|group| self.member(user, group, now))
        })
    }

    /// A device holds a row of someone else's profile only where a membership or a share
    /// reaches that profile for its user: a live one when the device ends online, one that the
    /// scenario had at some time otherwise.
    fn reach(&mut self, outcomes: &BTreeMap<&'a str, Vec<RowOutcome>>) {
        let mut problems = Vec::new();
        for (id, device) in &self.devices {
            let now = device.syncing();
            for row in outcomes.get(id).into_iter().flatten() {
                let Some(profile) = self.profiles.get(row.row.as_str()).copied() else {
                    continue;
                };
                if row.state == "absent" || self.reaches(device.user, profile, now) {
                    continue;
                }
                let when = if now { "gives" } else { "ever gave" };
                problems.push(format!(
                    "{id} holds {}, a row of {profile}, and no membership or share {when} {} \
                     that profile",
                    row.row, device.user
                ));
            }
        }
        self.problems.append(&mut problems);
    }

    /// After a restore of the server nothing a device still held is lost, and what took access
    /// away is taken away again (C18, D7).
    fn after_restore(&mut self, outcomes: &BTreeMap<&'a str, Vec<RowOutcome>>) {
        let mut problems = Vec::new();
        // A device whose session was refused, and that was not signed in again, has set aside
        // what belongs to other people and removed nothing (D14): what its own user wrote, and
        // the restore lost, is still on it, or among its kept changes once the time is up.
        for (id, device) in &self.devices {
            if !matches!(self.signed_out_at.get(id), Some((_, true))) {
                continue;
            }
            let rows = outcomes.get(id).map_or(&[][..], Vec::as_slice);
            let found = self.scenario.outcome.device.get(*id);
            for (row, version) in self.held.get(id).into_iter().flatten() {
                let wrote = self.user_of(version) == Some(device.user);
                let lost = self.lost_versions.contains(&(*row, *version));
                if !wrote || !lost || SERVER_WRITTEN.contains(&self.entity(row)) {
                    continue;
                }
                let holds = rows
                    .iter()
                    .any(|r| r.row == *row && r.version.as_deref() == Some(*version));
                let listed = found.is_some_and(|f| f.kept.iter().any(|k| k == version));
                if !holds && !listed {
                    problems.push(format!(
                        "{id} held {version} of {row} when the server was restored, its own \
                         user wrote it, the backup does not have it, and the device, whose \
                         session was refused and which was not signed in again, ends without \
                         it and without keeping it: a lost edit"
                    ));
                }
            }
        }
        for (id, device) in self.devices.iter().filter(|(_, d)| d.syncing()) {
            for (row, version) in self.held.get(id).into_iter().flatten() {
                // Only a device that may write the row gives it back. The ledger knows one
                // such case for certain: a row of its user's own personal profile.
                let own = self
                    .profiles
                    .get(row)
                    .is_some_and(|profile| self.personal_owner(profile) == Some(device.user));
                // And one case in which the device gives it back or keeps it whatever it may
                // write: a version its own user wrote.
                let wrote = self.user_of(version) == Some(device.user);
                let lost = self.lost_versions.contains(&(*row, *version));
                if !(own || wrote) || !lost || SERVER_WRITTEN.contains(&self.entity(row)) {
                    continue;
                }
                let current = self.server.get(row).map(|held| held.version);
                let kept = current == Some(*version)
                    || self.history.get(row).is_some_and(|h| h.contains(version));
                // A later change the same device made to the row stands for the version it
                // was made on.
                let replaced_here = current.is_some_and(|now| {
                    self.since_restore.contains(&(*row, now))
                        && self.last_writer(row, now) == Some(*id)
                });
                if !own {
                    // The server may refuse it by now; then the device keeps it for the user.
                    let listed = self
                        .scenario
                        .outcome
                        .device
                        .get(*id)
                        .is_some_and(|found| found.kept.iter().any(|k| k == version));
                    if !kept && !replaced_here && !listed {
                        problems.push(format!(
                            "{id} held {version} of {row} when the server was restored, its \
                             own user wrote it, the backup does not have it, and it ends \
                             neither on the server nor among the device's kept changes: a lost \
                             edit"
                        ));
                    }
                    continue;
                }
                if !kept && !replaced_here {
                    problems.push(format!(
                        "{id} held {version} of {row} when the server was restored, the backup \
                         does not have it, and it ends neither as the server's row nor in the \
                         history: a lost edit"
                    ));
                }
            }
        }
        let after = self
            .scenario
            .steps
            .iter()
            .rev()
            .take_while(|step| step.action != "restore_backup");
        let again: Vec<&Step> = after.collect();
        for removal in &self.lost_removals {
            let online = self
                .devices
                .get(removal.device)
                .is_some_and(Device::syncing);
            let gone = self.server.get(removal.row).is_none_or(|held| held.deleted);
            let tried = again.iter().any(|step| {
                step.at == removal.device
                    && step.action == removal.action
                    && step.row.as_deref() == Some(removal.row)
            });
            if online && !gone && !tried {
                problems.push(format!(
                    "{} ended {} with {}, the restore lost that call, and it is not made \
                     again although the device ends online",
                    removal.device, removal.row, removal.step
                ));
            }
        }
        self.problems.append(&mut problems);
    }

    /// The device whose push last wrote `version` of `row`.
    fn last_writer(&self, row: &str, version: &str) -> Option<&'a str> {
        let write = self
            .writes
            .iter()
            .rev()
            .find(|w| w.row == row && w.version == version)?;
        write.device
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
