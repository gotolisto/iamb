//! The two-device simulation of task 4.4 (`docs/sync-design.md` §1, §14).
//!
//! Each run is seeded and deterministic. One account has two devices, A and B; in about a third of
//! the runs a second account with a device C is a member of a family group of the first, and the
//! first shares folders with it. Every device runs the real agent (`iamb_sync::Agent`) against the
//! real store (`iamb_store::Store`, in memory) and the model of the server
//! (`iamb_sync::model::Server`), on a simulated clock: calls take 30 to 200 ms, a poke reaches a
//! device 50 to 400 ms after the commit, the agent waits as `iamb_sync::schedule::Schedule` says
//! (the 250 ms debounce, polling every 2 s or 60 s while the channel is down, backoff after a call
//! without an answer).
//!
//! Events: random edits (create, edit, move, delete and undelete snippets; create, rename, move and
//! delete folders; tags and links; shares and unshares; edits by the member in a shared folder;
//! rows too large to store), offline periods (seconds to minutes, and some longer than 30 days),
//! lost requests and lost answers, restarts of the agent (between an outbox write and its push,
//! and between the server's commit and the device's receipt), clock skew of up to an hour either
//! way, drops of the live channel, and in some runs a backup of the server and a restore from it.
//! Sessions end too (D14, C25): a session ended at the authentication service without Iamb's word
//! (the next call is refused and the agent signs the device out, confirmed when the refusal names
//! the device's epoch), a sign-out by hand (online: one round first, then the session ends; offline:
//! the tokens are dropped and nothing is confirmed), a sign-in again with a new session at a random
//! later time (the agent resyncs), and a restore that puts back the sessions of the backup, which
//! ends every session opened after it. In about one run in ten A's data directory is copied to a
//! second installation, A2, with A's device id, position, outbox and session (C25, S60); both are
//! used from then on, the one that is behind is told `device_conflict` and signs itself out, and
//! signs in again as a device of its own. Every device signed out at the end signs in again before
//! the checks.
//!
//! After each run every device comes online and syncs until nothing is left, and the run checks:
//!
//! - I1, convergence: each device holds exactly the rows the server shows its user, in the same
//!   revisions (blocked rows aside; a server tombstone may be absent);
//! - I2, no lost edit: every local change is in one of the twelve places of §1;
//! - I3, revisions only grow on a device, except in the reconcile of a new epoch;
//! - I4, no access leak: part of I1 (a device holds no row its user may not read);
//! - I5, exactly once: no op id wrote twice;
//! - I6, same answer: the devices of one account give each abbreviation to the same snippet and
//!   have the same permissions;
//! - I7, a valid tree after every commit (checked by the model).
//!
//! It also measures, for every write made while the writer and the other device of the account
//! were online, the time from the local write to the other device holding it, on the simulated
//! clock.
//!
//! `IAMB_SYNC_SIM_RUNS` sets the number of runs (default 120 in the ordinary test run; the nightly
//! workflow runs 1,000). `IAMB_SYNC_SIM_SEED` runs one seed. `IAMB_SYNC_SIM_REPORT` names a file
//! for the measurements as JSON.

#![cfg(feature = "model")]
#![allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::ref_option,
    clippy::explicit_iter_loop,
    clippy::needless_borrows_for_generic_args,
    clippy::type_complexity,
    clippy::single_match_else,
    clippy::uninlined_format_args,
    reason = "a simulation harness"
)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use iamb_core::model::{
    Abbreviation, Folder, FolderDefaults, FolderShare, Id, Permission, Snippet, SnippetTag,
    SyncMeta, Tag, Timestamp,
};
use iamb_core::sync::{AnyRow, RowKind, RowRef};
use iamb_store::Store;
use iamb_sync::api::{CallError, Transport};
use iamb_sync::model::{Caller, Server, SharedTransport, StoreDevice};
use iamb_sync::schedule::{Channel, Schedule, SplitMix};
use iamb_sync::{Agent, AgentConfig, RoundError};
use serde_json::{Value, json};

const T0: i64 = 1_795_000_000_000;
const DAY: i64 = 86_400_000;
const BODY_MAX: usize = 3_000;

// ---- the network --------------------------------------------------------------------------------

#[derive(Default)]
struct LinkState {
    online: bool,
    drop_request: u32,
    drop_answer: u32,
    base: i64,
    elapsed: i64,
    rng: Option<SplitMix>,
    skew: i64,
}

struct Link {
    inner: SharedTransport,
    state: Rc<RefCell<LinkState>>,
    clock: Rc<Cell<i64>>,
    /// The session of the tokens the installation holds (a sign-in replaces it).
    session: Rc<Cell<Id>>,
}

impl Transport for Link {
    fn call(&mut self, function: &str, args: Value) -> Result<Value, CallError> {
        let (lat, drop_req, drop_ans, now) = {
            let mut s = self.state.borrow_mut();
            if !s.online {
                return Err(CallError::Offline);
            }
            let lat = 30 + s.rng.as_mut().map_or(0, |r| r.below(170)) as i64;
            s.elapsed += lat;
            let now = s.base + s.elapsed;
            let dr = s.drop_request > 0 && matches!(function, "push" | "pull");
            if dr {
                s.drop_request -= 1;
            }
            let da = !dr && s.drop_answer > 0 && matches!(function, "push" | "pull");
            if da {
                s.drop_answer -= 1;
            }
            self.clock.set(now + s.skew);
            (lat, dr, da, now)
        };
        let _ = lat;
        self.inner.caller.session = self.session.get();
        if drop_req {
            return Err(CallError::Network("request lost".into()));
        }
        {
            let mut srv = self.inner.server.borrow_mut();
            srv.now = srv.now.max(now);
        }
        let answer = self.inner.call(function, args)?;
        if drop_ans {
            return Err(CallError::Network("answer lost".into()));
        }
        Ok(answer)
    }
}

// ---- devices ------------------------------------------------------------------------------------

/// Copies a data directory as a migration tool would (the `-shm` index is rebuilt on open).
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    for entry in std::fs::read_dir(from).unwrap_or_else(|e| panic!("read_dir: {e}")) {
        let entry = entry.unwrap_or_else(|e| panic!("entry: {e}"));
        let name = entry.file_name();
        let path = entry.path();
        if path.is_dir() {
            std::fs::create_dir_all(to.join(&name)).unwrap_or_else(|e| panic!("mkdir: {e}"));
            copy_dir(&path, &to.join(&name));
        } else if !name.to_string_lossy().ends_with("-shm") {
            std::fs::copy(&path, to.join(&name)).unwrap_or_else(|e| panic!("copy: {e}"));
        }
    }
}

struct SimDev {
    name: &'static str,
    user: Id,
    personal: Id,
    agent: Agent<Link, StoreDevice>,
    link: Rc<RefCell<LinkState>>,
    clock: Rc<Cell<i64>>,
    sched: Schedule,
    online: bool,
    online_since: i64,
    channel_outage_until: i64,
    revs: BTreeMap<RowRef, u64>,
    device_id: Option<Id>,
    session: Rc<Cell<Id>>,
    /// Signed out (D14): the library keeps working, the agent does not run.
    signed_out: bool,
    /// The data directory of a store on disk (the device whose directory is copied).
    dir: Option<tempfile::TempDir>,
}

impl SimDev {
    fn store(&mut self) -> &mut Store {
        &mut self.agent.device.store
    }
}

// ---- the ledger of §1 ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct LocalChange {
    id: usize,
    dev: usize,
    key: RowRef,
    content: Option<AnyRow>,
    at: i64,
    position_only: bool,
    superseded_unsent: bool,
    lost_by_restore: bool,
    /// The row's revision on the device right after the change (0: never synced).
    local_rev: u64,
}

fn strip(r: &AnyRow) -> AnyRow {
    let mut r = r.clone();
    let m = r.meta_mut();
    m.created_at = Timestamp(0);
    m.updated_at = Timestamp(0);
    m.updated_by = None;
    m.rev = 0;
    m.deleted_at = None;
    r
}

fn short(r: &AnyRow) -> String {
    let s = format!("{:?}", strip(r));
    if s.len() > 300 {
        format!("{}…", &s[..300])
    } else {
        s
    }
}

fn same_content(a: &Option<AnyRow>, b: &AnyRow) -> bool {
    match a {
        None => b.meta().deleted_at.is_some(),
        Some(x) => b.meta().deleted_at.is_none() && strip(x) == strip(b),
    }
}

fn same_but_parent(a: &Option<AnyRow>, b: &AnyRow) -> bool {
    match (a, b) {
        (Some(AnyRow::Folder(x)), AnyRow::Folder(y)) => {
            let mut x = x.clone();
            x.parent_id = y.parent_id;
            b.meta().deleted_at.is_none() && strip(&AnyRow::Folder(x)) == strip(b)
        }
        _ => false,
    }
}

// ---- a run --------------------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct RunResult {
    seed: u64,
    changes: usize,
    lost: Vec<String>,
    violations: Vec<String>,
    agreement_ms: Vec<i64>,
    undelivered: usize,
    restored: bool,
    shares: bool,
    places: BTreeMap<u8, usize>,
    sign_outs: usize,
    sign_ins: usize,
    copied: bool,
}

struct Measure {
    key: RowRef,
    rev: u64,
    t_write: i64,
    target: usize,
}

struct Sim {
    seed: u64,
    rng: SplitMix,
    t: i64,
    server: Rc<RefCell<Server>>,
    devs: Vec<SimDev>,
    ledger: Vec<LocalChange>,
    measures: Vec<Measure>,
    processed_seen: usize,
    result: RunResult,
    group: Option<Id>,
    counter: u64,
    backup_at: Option<i64>,
    restored: bool,
    restored_at: Option<i64>,
    epoch_seen_at: BTreeMap<usize, i64>,
    /// The step at which A's data directory is copied to a new installation (C25, S60).
    copy_at: Option<u64>,
}

fn id_maker(seed: u64) -> Box<dyn FnMut() -> Id + Send> {
    let mut r = SplitMix(seed);
    let mut n = 0u64;
    Box::new(move || {
        n += 1;
        let mut b = [0u8; 10];
        for x in &mut b {
            *x = (r.next_u64() & 0xff) as u8;
        }
        Id::v7(T0 as u64 + n, b)
    })
}

impl Sim {
    fn new(seed: u64) -> Self {
        let mut server = Server::new(T0, seed);
        server.limits.body_max = BODY_MAX;
        let server = Rc::new(RefCell::new(server));
        let mut rng = SplitMix(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0xA5A5);
        let shares = rng.chance(0.35);
        let (u, pu, s1) = server.borrow_mut().add_user("ines@example.com");
        let s2 = server.borrow_mut().add_session();
        let mut sim = Self {
            seed,
            rng,
            t: T0,
            server: server.clone(),
            devs: Vec::new(),
            ledger: Vec::new(),
            measures: Vec::new(),
            processed_seen: 0,
            result: RunResult {
                seed,
                shares,
                ..RunResult::default()
            },
            group: None,
            counter: 0,
            backup_at: None,
            restored: false,
            restored_at: None,
            epoch_seen_at: BTreeMap::new(),
            copy_at: None,
        };
        // In some runs A's data directory is copied to a second installation during the run: A
        // keeps its store on disk (a separate generator, so the other runs keep their events).
        let mut extra = SplitMix(seed ^ 0xC0B1_ED00_D1EC_7031);
        if extra.chance(0.1) {
            sim.copy_at = Some(5 + extra.below(50));
        }
        let dir = sim.copy_at.map(|_| {
            let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
            let store = Store::open(dir.path()).unwrap_or_else(|e| panic!("store: {e}"));
            (store, dir)
        });
        sim.add_dev("A", u, pu, Rc::new(Cell::new(s1)), dir);
        sim.add_dev("B", u, pu, Rc::new(Cell::new(s2)), None);
        if shares {
            let (v, pv, s3) = server.borrow_mut().add_user("tomas@example.com");
            sim.add_dev("C", v, pv, Rc::new(Cell::new(s3)), None);
        }
        sim
    }

    fn add_dev(
        &mut self,
        name: &'static str,
        user: Id,
        personal: Id,
        session: Rc<Cell<Id>>,
        disk: Option<(Store, tempfile::TempDir)>,
    ) {
        let clock = Rc::new(Cell::new(T0));
        let skew = if self.rng.chance(0.6) {
            self.rng.below(7_200_000) as i64 - 3_600_000
        } else {
            0
        };
        let link = Rc::new(RefCell::new(LinkState {
            online: true,
            rng: Some(SplitMix(self.rng.next_u64())),
            skew,
            ..LinkState::default()
        }));
        let transport = Link {
            inner: SharedTransport {
                server: self.server.clone(),
                caller: Caller {
                    user,
                    session: session.get(),
                },
            },
            state: link.clone(),
            clock: clock.clone(),
            session: session.clone(),
        };
        let (store, dir) = match disk {
            Some((store, dir)) => (store, Some(dir)),
            None => (
                Store::open_in_memory().unwrap_or_else(|e| panic!("store: {e}")),
                None,
            ),
        };
        let agent = Agent::new(
            transport,
            StoreDevice::new(store, clock.clone()),
            AgentConfig {
                device_name: name.to_owned(),
                ..AgentConfig::default()
            },
            id_maker(self.rng.next_u64()),
        );
        self.devs.push(SimDev {
            name,
            user,
            personal,
            agent,
            link,
            clock,
            sched: Schedule::new(),
            online: true,
            online_since: T0,
            channel_outage_until: 0,
            revs: BTreeMap::new(),
            device_id: None,
            session,
            signed_out: false,
            dir,
        });
    }

    /// A's data directory is copied to a new computer, which is used from then on as well: the
    /// copy has A's device id, position, outbox and tokens (C25, S60).
    fn copy_a(&mut self) {
        let Some(src) = self.devs[0].dir.as_ref().map(|d| d.path().to_path_buf()) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        copy_dir(&src, dir.path());
        let store = Store::open(dir.path()).unwrap_or_else(|e| panic!("store: {e}"));
        let (user, personal) = (self.devs[0].user, self.devs[0].personal);
        let session = Rc::new(Cell::new(self.devs[0].session.get()));
        self.add_dev("A2", user, personal, session, Some((store, dir)));
        let t = self.t;
        let n = self.devs.len() - 1;
        let dev = &mut self.devs[n];
        dev.online_since = t;
        dev.clock.set(t);
        dev.sched.poked(u(t));
        self.result.copied = true;
    }

    /// D14: a sign-out. By hand on a device that answers, the agent syncs first and the server's
    /// answer confirms it; otherwise (offline) the tokens are dropped and nothing is confirmed.
    fn sign_out(&mut self, d: usize) {
        if self.devs[d].signed_out {
            return;
        }
        let mut confirmed = false;
        if self.devs[d].online {
            self.run_round(d);
            if self.devs[d].signed_out {
                return;
            }
            confirmed = self.devs[d].link.borrow().online;
            let s = self.devs[d].session.get();
            self.server.borrow_mut().end_session(s);
        }
        let now = self.now_of(d);
        let _ = self.devs[d].store().signed_out(confirmed, true, now);
        self.devs[d].agent.forget();
        self.mark_signed_out(d);
    }

    fn mark_signed_out(&mut self, d: usize) {
        self.devs[d].signed_out = true;
        self.measures.retain(|m| m.target != d);
        self.result.sign_outs += 1;
    }

    /// The user signs in again with the same account: a new session.
    fn sign_in(&mut self, d: usize) {
        if !self.devs[d].signed_out {
            return;
        }
        let s = self.server.borrow_mut().add_session();
        let t = self.t;
        let dev = &mut self.devs[d];
        dev.session.set(s);
        dev.agent.forget();
        dev.signed_out = false;
        dev.online_since = dev.online_since.max(t);
        dev.sched.poked(u(t));
        self.result.sign_ins += 1;
    }

    fn new_id(&mut self) -> Id {
        self.counter += 1;
        let mut b = [0u8; 10];
        for x in &mut b {
            *x = (self.rng.next_u64() & 0xff) as u8;
        }
        Id::v7(T0 as u64 + 1_000_000 + self.counter, b)
    }

    fn violation(&mut self, s: String) {
        if self.result.violations.len() < 20 {
            self.result.violations.push(s);
        }
    }

    // ---- rounds ---------------------------------------------------------------------------

    fn run_round(&mut self, d: usize) {
        let t = self.t;
        {
            let dev = &mut self.devs[d];
            let mut l = dev.link.borrow_mut();
            l.base = t;
            l.elapsed = 0;
            l.online = dev.online;
            dev.clock.set(t + l.skew);
        }
        {
            let mut srv = self.server.borrow_mut();
            srv.now = srv.now.max(t);
        }
        let res = self.devs[d].agent.sync();
        let elapsed = self.devs[d].link.borrow().elapsed;
        let t_end = t + elapsed;
        if let Ok(w) = std::env::var("IAMB_SYNC_SIM_WATCH") {
            let name = self.devs[d].name;
            let rows = self.devs[d].store().all_rows().unwrap_or_default();
            let mine: Vec<String> = rows
                .iter()
                .filter(|r| w.contains(&r.id().to_string()))
                .map(|r| format!("rev {} del {:?}", r.meta().rev, r.meta().deleted_at))
                .collect();
            let srv: Vec<String> = self
                .server
                .borrow()
                .st
                .rows
                .iter()
                .filter(|(k, _)| w.contains(&k.id.to_string()))
                .map(|(_, s)| format!("rev {} del {:?}", s.row.meta().rev, s.row.meta().deleted_at))
                .collect();
            eprintln!(
                "ROUND {name} at {} -> {:?}: device {:?} server {:?} waiting {:?}",
                t - T0,
                res.as_ref()
                    .map(|r| (r.pages, r.rows, r.pushed, r.resynced)),
                mine,
                srv,
                self.devs[d].store().waiting_rows().map(|w| w.len())
            );
        }
        let new_epoch = matches!(&res, Ok(r) if r.new_epoch);
        match res {
            Ok(_) => self.devs[d].sched.synced(u(t_end)),
            Err(RoundError::Retry(_)) => {
                let mut r = SplitMix(self.rng.next_u64());
                self.devs[d].sched.failed(u(t_end), &mut r);
            }
            Err(RoundError::RateLimited(ms)) => self.devs[d].sched.wait(u(t_end), ms),
            // The session ended or was refused (D14): the agent signed the device out.
            Err(RoundError::SignedOut { .. }) => self.mark_signed_out(d),
            Err(e) => {
                let name = self.devs[d].name;
                self.violation(format!("{name}: round failed: {e}"));
                self.devs[d].sched.failed(u(t_end), &mut SplitMix(1));
            }
        }
        // D8: after each sync the store removes tombstones older than 30 days.
        let cutoff = Timestamp(self.devs[d].clock.get() - 30 * DAY);
        let _ = self.devs[d].store().purge_deleted_before(cutoff);
        // The device id (a new one after `device_conflict`, C25).
        self.devs[d].device_id = self.devs[d]
            .agent
            .device
            .store
            .sync_info()
            .ok()
            .and_then(|i| i.device);
        // Pokes.
        let pokes: Vec<(Id, String, u64)> = std::mem::take(&mut self.server.borrow_mut().pokes);
        for (user, _, _) in pokes {
            for i in 0..self.devs.len() {
                if self.devs[i].user == user
                    && self.devs[i].online
                    && self.devs[i].sched.channel == Channel::Up
                {
                    let delay = 50 + self.rng.below(350) as i64;
                    self.devs[i].sched.poked(u(t_end + delay));
                }
            }
        }
        self.after_round(d, t_end, new_epoch);
    }

    fn after_round(&mut self, d: usize, t_end: i64, new_epoch: bool) {
        if new_epoch {
            self.epoch_seen_at.entry(d).or_insert(t_end);
        }
        // New writes the server accepted from this device: measurements.
        let device_id = self.devs[d].device_id;
        let processed: Vec<_> = self.server.borrow().processed[self.processed_seen..].to_vec();
        self.processed_seen += processed.len();
        for p in processed {
            if p.status != "applied" || Some(p.device) != device_id {
                continue;
            }
            let Some(rev) = p.rev else { continue };
            let writer = d;
            let Some(change) = self
                .ledger
                .iter()
                .rev()
                .find(|c| c.dev == writer && c.key == p.key)
            else {
                continue;
            };
            let t_write = change.at;
            if !self.devs[writer].online || self.devs[writer].online_since > t_write {
                continue;
            }
            for target in 0..self.devs.len() {
                if target == writer || self.devs[target].user != self.devs[writer].user {
                    continue;
                }
                if !self.devs[target].online
                    || self.devs[target].signed_out
                    || self.devs[target].online_since > t_write
                {
                    continue;
                }
                self.measures.push(Measure {
                    key: p.key,
                    rev,
                    t_write,
                    target,
                });
            }
        }
        // This device's view: settled measurements, and I3.
        let rows = self.devs[d].store().all_rows().unwrap_or_default();
        let held: BTreeMap<RowRef, u64> = rows.iter().map(|r| (r.key(), r.meta().rev)).collect();
        let mut keep = Vec::new();
        for m in std::mem::take(&mut self.measures) {
            if m.target == d && held.get(&m.key).is_some_and(|r| *r >= m.rev) {
                self.result.agreement_ms.push(t_end - m.t_write);
            } else {
                keep.push(m);
            }
        }
        self.measures = keep;
        if !new_epoch {
            let name = self.devs[d].name;
            let mut problems = Vec::new();
            for (k, r) in &held {
                if let Some(prev) = self.devs[d].revs.get(k)
                    && r < prev
                    && *r > 0
                {
                    problems.push(format!("{name}: I3: {k} went from rev {prev} to {r}"));
                }
            }
            for p in problems {
                self.violation(p);
            }
        }
        let revs = &mut self.devs[d].revs;
        for (k, r) in held {
            if r > 0 {
                revs.insert(k, r);
            }
        }
    }

    fn set_online(&mut self, d: usize, online: bool) {
        let t = self.t;
        let dev = &mut self.devs[d];
        if dev.online == online {
            return;
        }
        dev.online = online;
        dev.link.borrow_mut().online = online;
        if online {
            dev.online_since = t;
            dev.sched.channel_up(u(t));
        } else {
            dev.sched.channel = Channel::Down { attempt: 0 };
            self.measures.retain(|m| m.target != d);
        }
    }

    /// Runs every device round due up to `until`, in time order.
    fn run_until(&mut self, until: i64) {
        for _ in 0..100_000 {
            let mut best: Option<(i64, usize)> = None;
            for (i, dev) in self.devs.iter().enumerate() {
                if !dev.online || dev.signed_out {
                    continue;
                }
                let due = dev.sched.next_round(u(self.t)) as i64;
                let mut next = due;
                if let Some(rc) = dev.sched.reconnect_at
                    && dev.sched.channel != Channel::Up
                {
                    next = next.min(rc as i64);
                }
                if best.is_none_or(|(b, _)| next < b) {
                    best = Some((next, i));
                }
            }
            let Some((at, i)) = best else { break };
            if at > until {
                break;
            }
            self.t = self.t.max(at);
            let dev = &mut self.devs[i];
            if dev.sched.channel != Channel::Up
                && let Some(rc) = dev.sched.reconnect_at
                && rc as i64 <= self.t
            {
                if self.t >= dev.channel_outage_until {
                    let t = self.t;
                    dev.sched.channel_up(u(t));
                } else {
                    let t = self.t;
                    dev.sched
                        .channel_down(u(t), false, &mut SplitMix(self.seed ^ t as u64));
                }
                continue;
            }
            self.run_round(i);
        }
        self.t = self.t.max(until);
    }

    // ---- local writes ---------------------------------------------------------------------

    fn record(&mut self, d: usize, key: RowRef, position_only: bool) {
        let t = self.t;
        let row = self.devs[d].store().get_row(key).ok().flatten();
        let local_rev = row.as_ref().map_or(0, |r| r.meta().rev);
        let content = row.filter(|r| r.meta().deleted_at.is_none());
        // A change made before the server received the previous one takes its place (place 7);
        // so does a folder's delete for the rows it takes with it.
        let mut keys = vec![key];
        if key.kind == RowKind::Folder && content.is_none() {
            let rows = self.devs[d].store().all_rows().unwrap_or_default();
            let folders: BTreeMap<Id, Option<Id>> = rows
                .iter()
                .filter_map(|r| match r {
                    AnyRow::Folder(f) => Some((f.meta.id, f.parent_id)),
                    _ => None,
                })
                .collect();
            let inside = |mut f: Id| -> bool {
                for _ in 0..8 {
                    if f == key.id {
                        return true;
                    }
                    match folders.get(&f).copied().flatten() {
                        Some(p) => f = p,
                        None => return false,
                    }
                }
                false
            };
            for r in &rows {
                match r {
                    AnyRow::Folder(f) if inside(f.meta.id) => keys.push(r.key()),
                    AnyRow::Snippet(sn) if inside(sn.folder_id) => keys.push(r.key()),
                    AnyRow::FolderShare(x) if inside(x.folder_id) => keys.push(r.key()),
                    _ => {}
                }
            }
        }
        let device_id = self.devs[d].device_id;
        let processed = self.server.borrow().processed.clone();
        for c in self.ledger.iter_mut() {
            if c.dev != d || c.superseded_unsent || !keys.contains(&c.key) {
                continue;
            }
            let received = processed.iter().any(|p| {
                Some(p.device) == device_id
                    && p.key == c.key
                    && p.status != "blocked"
                    && match (&c.content, &p.row) {
                        (None, None) => true,
                        (Some(x), Some(y)) => strip(x) == strip(y),
                        _ => false,
                    }
            });
            if !received {
                c.superseded_unsent = true;
            }
        }
        if std::env::var("IAMB_SYNC_SIM_WATCH").is_ok() {
            eprintln!(
                "CHANGE {} at {}: {} {}",
                self.devs[d].name,
                t - T0,
                key,
                if content.is_some() { "put" } else { "delete" }
            );
        }
        let id = self.ledger.len();
        self.ledger.push(LocalChange {
            id,
            dev: d,
            key,
            content,
            at: t,
            position_only,
            superseded_unsent: false,
            lost_by_restore: false,
            local_rev,
        });
        // Cascade entries of a folder's delete are changes of the folder, not of the rows (§1).
        self.devs[d].sched.outbox_changed(u(t));
    }

    fn now_of(&self, d: usize) -> Timestamp {
        Timestamp(self.t + self.devs[d].link.borrow().skew)
    }

    fn live<T>(&mut self, d: usize, pick: impl Fn(&AnyRow) -> Option<T>) -> Vec<T> {
        self.devs[d]
            .store()
            .all_rows()
            .unwrap_or_default()
            .iter()
            .filter(|r| r.meta().deleted_at.is_none())
            .filter_map(pick)
            .collect()
    }

    fn folders_of(&mut self, d: usize, profile: Id) -> Vec<Folder> {
        self.live(d, |r| match r {
            AnyRow::Folder(f) if f.profile_id == profile => Some(f.clone()),
            _ => None,
        })
    }

    /// Folders the device's user may write in: its personal profile's, and shared ones with edit.
    fn writable_folders(&mut self, d: usize) -> Vec<Folder> {
        let personal = self.devs[d].personal;
        let mut out = self.folders_of(d, personal);
        let roots = self.devs[d].store().access_roots().unwrap_or_default();
        let shared: BTreeSet<Id> = roots
            .iter()
            .filter(|r| r.permission != iamb_core::sync::Access::View)
            .map(|r| r.folder_id)
            .collect();
        if !shared.is_empty() {
            let all: Vec<Folder> = self.live(d, |r| match r {
                AnyRow::Folder(f) => Some(f.clone()),
                _ => None,
            });
            for f in &all {
                let mut cur = Some(f.meta.id);
                let mut n = 0;
                while let Some(x) = cur {
                    if shared.contains(&x) {
                        out.push(f.clone());
                        break;
                    }
                    cur = all
                        .iter()
                        .find(|g| g.meta.id == x)
                        .and_then(|g| g.parent_id);
                    n += 1;
                    if n > 6 {
                        break;
                    }
                }
            }
        }
        out
    }

    fn pick<T: Clone>(&mut self, v: &[T]) -> Option<T> {
        if v.is_empty() {
            None
        } else {
            Some(v[self.rng.below(v.len() as u64) as usize].clone())
        }
    }

    fn act(&mut self, d: usize) {
        let user = self.devs[d].user;
        let personal = self.devs[d].personal;
        let now = self.now_of(d);
        let meta = |id: Id| SyncMeta {
            updated_by: Some(user),
            ..SyncMeta::new(id, now)
        };
        let roll = self.rng.below(100);
        let folders = self.writable_folders(d);
        let snippets: Vec<Snippet> = {
            let fids: BTreeSet<Id> = folders.iter().map(|f| f.meta.id).collect();
            self.live(d, |r| match r {
                AnyRow::Snippet(s) if fids.contains(&s.folder_id) => Some(s.clone()),
                _ => None,
            })
        };
        let is_owner = self.devs[d].name != "C";
        match roll {
            // A folder.
            0..=9 if is_owner || folders.is_empty() => {
                if !is_owner {
                    return;
                }
                let parent = if self.rng.chance(0.4) {
                    let own = self.folders_of(d, personal);
                    self.pick(&own).map(|f| f.meta.id)
                } else {
                    None
                };
                let id = self.new_id();
                self.counter += 1;
                let f = Folder {
                    meta: meta(id),
                    profile_id: personal,
                    parent_id: parent,
                    name: format!("folder {}", self.counter),
                    position: self.rng.below(10) as i64,
                    member_permission: None,
                    defaults: FolderDefaults::default(),
                };
                if self.devs[d].store().put(&f, now).is_ok() {
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::Folder,
                            id,
                        },
                        false,
                    );
                }
            }
            // A snippet.
            10..=34 => {
                let Some(f) = self.pick(&folders) else { return };
                let id = self.new_id();
                self.counter += 1;
                let abbr = format!(";{}{}", self.devs[d].name.to_lowercase(), self.counter);
                let Ok(a) = Abbreviation::new(&abbr) else {
                    return;
                };
                let s = Snippet::new_defaults(
                    meta(id),
                    f.meta.id,
                    a,
                    "label",
                    &format!("body {}", self.counter),
                    None,
                );
                if self.devs[d].store().put(&s, now).is_ok() {
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::Snippet,
                            id,
                        },
                        false,
                    );
                }
            }
            // Edit a snippet (sometimes too large to store, sometimes back).
            35..=59 => {
                let Some(mut s) = self.pick(&snippets) else {
                    return;
                };
                self.counter += 1;
                if self.rng.chance(0.04) {
                    s.body = "x".repeat(BODY_MAX + 10);
                } else if self.rng.chance(0.5) {
                    s.body = format!("body {} edited on {}", self.counter, self.devs[d].name);
                } else {
                    s.label = format!("label {}", self.counter);
                }
                s.meta.updated_by = Some(user);
                if self.devs[d].store().put(&s, now).is_ok() {
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::Snippet,
                            id: s.meta.id,
                        },
                        false,
                    );
                }
            }
            // Move a snippet to another folder of the same profile.
            60..=64 => {
                let Some(mut s) = self.pick(&snippets) else {
                    return;
                };
                let Some(from) = folders.iter().find(|f| f.meta.id == s.folder_id).cloned() else {
                    return;
                };
                let same: Vec<Folder> = folders
                    .iter()
                    .filter(|f| f.profile_id == from.profile_id && f.meta.id != from.meta.id)
                    .cloned()
                    .collect();
                let Some(to) = self.pick(&same) else { return };
                s.folder_id = to.meta.id;
                if self.devs[d].store().put(&s, now).is_ok() {
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::Snippet,
                            id: s.meta.id,
                        },
                        false,
                    );
                }
            }
            // Delete a snippet.
            65..=71 => {
                let Some(s) = self.pick(&snippets) else {
                    return;
                };
                if self.devs[d]
                    .store()
                    .delete::<Snippet>(s.meta.id, now)
                    .is_ok()
                {
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::Snippet,
                            id: s.meta.id,
                        },
                        false,
                    );
                }
            }
            // Undelete a snippet (Undo).
            72..=74 => {
                let dead: Vec<Snippet> = self.devs[d]
                    .store()
                    .list::<Snippet>(iamb_store::ListOptions::ALL)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|s| {
                        s.meta.deleted_at.is_some()
                            && folders.iter().any(|f| f.meta.id == s.folder_id)
                    })
                    .collect();
                let Some(mut s) = self.pick(&dead) else {
                    return;
                };
                s.meta.updated_by = Some(user);
                if self.devs[d].store().put(&s, now).is_ok() {
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::Snippet,
                            id: s.meta.id,
                        },
                        false,
                    );
                }
            }
            // Rename, reorder or move a folder.
            75..=81 if is_owner => {
                let own = self.folders_of(d, personal);
                let Some(mut f) = self.pick(&own) else { return };
                let what = self.rng.below(3);
                let mut position_only = false;
                match what {
                    0 => {
                        self.counter += 1;
                        f.name = format!("renamed {}", self.counter);
                    }
                    1 => {
                        f.position = self.rng.below(20) as i64;
                        position_only = true;
                    }
                    _ => {
                        let other = self.pick(&own).map(|g| g.meta.id);
                        f.parent_id = if self.rng.chance(0.3) { None } else { other };
                    }
                }
                f.meta.updated_by = Some(user);
                if self.devs[d].store().put(&f, now).is_ok() {
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::Folder,
                            id: f.meta.id,
                        },
                        position_only,
                    );
                }
            }
            // Delete a folder (its subtree with it).
            82..=84 if is_owner => {
                let own = self.folders_of(d, personal);
                let Some(f) = self.pick(&own) else { return };
                if self.devs[d]
                    .store()
                    .delete::<Folder>(f.meta.id, now)
                    .is_ok()
                {
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::Folder,
                            id: f.meta.id,
                        },
                        false,
                    );
                    // The rows it took with it are changes of the folder (§1); their cascade
                    // entries ride on the folder's op.
                }
            }
            // Tags and links.
            85..=91 => {
                let Some(s) = self.pick(&snippets) else {
                    return;
                };
                let Some(folder) = folders.iter().find(|f| f.meta.id == s.folder_id).cloned()
                else {
                    return;
                };
                let profile = folder.profile_id;
                let tags: Vec<Tag> = self.live(d, |r| match r {
                    AnyRow::Tag(t) if t.profile_id == profile => Some(t.clone()),
                    _ => None,
                });
                let links: Vec<SnippetTag> = self.live(d, |r| match r {
                    AnyRow::SnippetTag(l) if l.snippet_id == s.meta.id => Some(l.clone()),
                    _ => None,
                });
                if !links.is_empty() && self.rng.chance(0.4) {
                    let l = links[0].clone();
                    if self.devs[d]
                        .store()
                        .delete::<SnippetTag>(l.meta.id, now)
                        .is_ok()
                    {
                        self.record(
                            d,
                            RowRef {
                                kind: RowKind::SnippetTag,
                                id: l.meta.id,
                            },
                            false,
                        );
                    }
                    return;
                }
                let tag = if tags.is_empty() || self.rng.chance(0.3) {
                    if profile != personal {
                        return;
                    }
                    let id = self.new_id();
                    self.counter += 1;
                    let t = Tag {
                        meta: meta(id),
                        profile_id: profile,
                        name: format!("tag {}", self.counter),
                        color: None,
                    };
                    if self.devs[d].store().put(&t, now).is_err() {
                        return;
                    }
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::Tag,
                            id,
                        },
                        false,
                    );
                    id
                } else {
                    let Some(t) = self.pick(&tags) else { return };
                    t.meta.id
                };
                let id = self.new_id();
                let l = SnippetTag {
                    meta: meta(id),
                    snippet_id: s.meta.id,
                    tag_id: tag,
                };
                if self.devs[d].store().put(&l, now).is_ok() {
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::SnippetTag,
                            id,
                        },
                        false,
                    );
                }
            }
            // Share or unshare a folder with the group.
            92..=95 if is_owner => {
                let Some(group) = self.group else { return };
                let own = self.folders_of(d, personal);
                let shares: Vec<FolderShare> = self.live(d, |r| match r {
                    AnyRow::FolderShare(x) => Some(x.clone()),
                    _ => None,
                });
                if !shares.is_empty() && self.rng.chance(0.4) {
                    let x = shares[0].clone();
                    if self.devs[d]
                        .store()
                        .delete::<FolderShare>(x.meta.id, now)
                        .is_ok()
                    {
                        self.record(
                            d,
                            RowRef {
                                kind: RowKind::FolderShare,
                                id: x.meta.id,
                            },
                            false,
                        );
                    }
                    return;
                }
                let Some(f) = self.pick(&own) else { return };
                let existing = self.devs[d]
                    .store()
                    .list::<FolderShare>(iamb_store::ListOptions::ALL)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|x| x.folder_id == f.meta.id && x.profile_id == group);
                let mut share = existing.unwrap_or_else(|| FolderShare {
                    meta: meta(Id::from_bytes(*f.meta.id.as_bytes()).xor_marker()),
                    folder_id: f.meta.id,
                    profile_id: group,
                    permission: Permission::Edit,
                });
                share.permission = if self.rng.chance(0.8) {
                    Permission::Edit
                } else {
                    Permission::View
                };
                share.meta.updated_by = Some(user);
                if self.devs[d].store().put(&share, now).is_ok() {
                    self.record(
                        d,
                        RowRef {
                            kind: RowKind::FolderShare,
                            id: share.meta.id,
                        },
                        false,
                    );
                }
            }
            _ => {}
        }
    }

    // ---- the run --------------------------------------------------------------------------

    fn setup_group(&mut self) {
        // The owner creates the group and invites the member, who accepts.
        let g = self.new_id();
        let inv = self.new_id();
        let now = Timestamp(self.t);
        let a = &mut self.devs[0].agent;
        let r1 = a.account_call(
            "create_group",
            json!({ "id": g, "kind": "family", "name": "Family" }),
            None,
            now,
        );
        let r2 = a.account_call(
            "invite",
            json!({ "id": inv, "group": g, "email": "tomas@example.com", "role": "member" }),
            None,
            now,
        );
        let c = &mut self.devs[2].agent;
        let r3 = c.account_call("accept_invitation", json!({ "id": inv }), None, now);
        if matches!((&r1, &r2, &r3), (Ok(Ok(_)), Ok(Ok(_)), Ok(Ok(_)))) {
            self.group = Some(g);
        } else {
            self.violation(format!("group setup failed: {r1:?} {r2:?} {r3:?}"));
        }
    }

    fn run(mut self) -> RunResult {
        // Everyone signs in and syncs once.
        for d in 0..self.devs.len() {
            self.run_round(d);
        }
        if self.result.shares {
            self.setup_group();
            for d in 0..self.devs.len() {
                self.devs[d].sched.poked(u(self.t));
            }
        }
        let steps = 60 + self.rng.below(90);
        let restore_at = if self.rng.chance(0.12) {
            Some((
                steps / 3 + self.rng.below(steps / 3),
                steps * 2 / 3 + self.rng.below(steps / 4),
            ))
        } else {
            None
        };
        for step in 0..steps {
            let gap = 200 + self.rng.below(6_000) as i64;
            let until = self.t + gap;
            self.run_until(until);
            if self.copy_at == Some(step) {
                self.copy_a();
            }
            if let Some((b, r)) = restore_at {
                if step == b {
                    self.server.borrow_mut().backup("b1");
                    self.backup_at = Some(self.t);
                }
                if step == r && self.backup_at.is_some() {
                    self.restore();
                }
            }
            let d = self.rng.below(self.devs.len() as u64) as usize;
            let roll = self.rng.below(1000);
            match roll {
                0..=29 => {
                    let online = !self.devs[d].online;
                    self.set_online(d, online);
                }
                30..=34 => {
                    // A long time offline: more than the 30 days of the feed and the tombstones.
                    if self.devs[d].online {
                        self.set_online(d, false);
                        self.server.borrow_mut().advance(31 * DAY);
                        self.t += 31 * DAY;
                        // A write made before the jump is not an online agreement to time.
                        self.measures.clear();
                        for x in &mut self.devs {
                            x.online_since = x.online_since.max(0);
                        }
                    }
                }
                35..=59 => self.devs[d].link.borrow_mut().drop_answer += 1,
                60..=79 => self.devs[d].link.borrow_mut().drop_request += 1,
                80..=94 => {
                    // A restart: what the agent had under way is forgotten, the outbox is not.
                    self.devs[d].agent.forget();
                }
                95..=109 => {
                    let t = self.t;
                    let until = t + 1_000 + self.rng.below(30_000) as i64;
                    let dev = &mut self.devs[d];
                    dev.channel_outage_until = until;
                    dev.sched.channel_down(u(t), false, &mut SplitMix(t as u64));
                }
                110..=114 => {
                    // A crash between the outbox write and the push.
                    self.act(d);
                    self.devs[d].agent.forget();
                }
                115..=117 => {
                    // The session ends at the authentication service without Iamb's word (the
                    // user ended it elsewhere): the next call is refused (D14).
                    let s = self.devs[d].session.get();
                    self.server.borrow_mut().end_session(s);
                }
                118..=120 => self.sign_out(d),
                121..=130 => self.sign_in(d),
                _ => self.act(d),
            }
        }
        self.converge();
        self.check();
        self.result.restored = self.restored;
        self.result.changes = self.ledger.len();
        self.result
    }

    fn restore(&mut self) {
        // What each device holds of each change now: a change no device holds as the row's version
        // is the one place 12 allows the restore to lose.
        let snapshots: Vec<BTreeMap<RowRef, AnyRow>> = (0..self.devs.len())
            .map(|d| {
                self.devs[d]
                    .store()
                    .all_rows()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|r| (r.key(), r))
                    .collect()
            })
            .collect();
        for c in &mut self.ledger {
            let held = snapshots.iter().enumerate().any(|(d, snap)| {
                self.devs[d].user == self.devs[c.dev].user
                    && snap
                        .get(&c.key)
                        .is_some_and(|r| same_content(&c.content, r))
            });
            if !held {
                c.lost_by_restore = true;
            }
        }
        let last_log = self.server.borrow().log.last().map(|l| l.at);
        let since = self.backup_at.unwrap_or(T0);
        let mut srv = self.server.borrow_mut();
        srv.restore("b1", last_log);
        srv.replay_calls(since);
        drop(srv);
        self.restored = true;
        self.restored_at = Some(self.t);
        for d in &mut self.devs {
            d.revs.clear();
        }
    }

    fn converge(&mut self) {
        for d in 0..self.devs.len() {
            self.devs[d].link.borrow_mut().drop_answer = 0;
            self.devs[d].link.borrow_mut().drop_request = 0;
            self.devs[d].channel_outage_until = 0;
            self.set_online(d, true);
            self.sign_in(d);
            let _ = self.devs[d].store().unblock(Some("server_error"));
        }
        for _ in 0..8 {
            for d in 0..self.devs.len() {
                // A device the server signed out during the last rounds (the copy of S60 that
                // lost its device id) signs in again.
                self.sign_in(d);
                self.t += 3_000;
                self.run_round(d);
            }
        }
        let undelivered = self.measures.len();
        self.result.undelivered = undelivered;
        // `IAMB_SYNC_SIM_UNSETTLED`: why a measured write never settled on its target.
        if std::env::var("IAMB_SYNC_SIM_UNSETTLED").is_ok() {
            let measures: Vec<(RowRef, u64, usize)> = self
                .measures
                .iter()
                .map(|m| (m.key, m.rev, m.target))
                .collect();
            for (key, rev, target) in measures {
                let user = self.devs[target].user;
                let (srv_state, readable) = {
                    let srv = self.server.borrow();
                    let st = srv
                        .st
                        .rows
                        .get(&key)
                        .map(|s| (s.row.meta().rev, s.row.meta().deleted_at.is_some()));
                    (st, srv.can_read(user, key))
                };
                let held = self.devs[target]
                    .store()
                    .get_row(key)
                    .ok()
                    .flatten()
                    .map(|r| (r.meta().rev, r.meta().deleted_at.is_some()));
                eprintln!(
                    "UNSETTLED seed {} {key} rev {rev} -> {}: server {srv_state:?} readable {readable}; device {held:?}",
                    self.seed, self.devs[target].name
                );
            }
        }
    }

    fn check(&mut self) {
        let srv = self.server.borrow().clone();
        for v in &srv.violations {
            self.violation(format!("I7: {v}"));
        }
        // I5: an op id wrote at most once.
        let mut seen = BTreeSet::new();
        for p in &srv.processed {
            if p.status == "applied" && !seen.insert((p.device, p.op_id)) {
                self.violation(format!("I5: op {} wrote twice", p.op_id));
            }
        }
        // I1 and I4.
        for d in 0..self.devs.len() {
            let user = self.devs[d].user;
            let name = self.devs[d].name;
            let rows = self.devs[d].store().all_rows().unwrap_or_default();
            let blocked: BTreeSet<RowRef> = self.devs[d]
                .store()
                .blocked_rows()
                .unwrap_or_default()
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            let waiting = self.devs[d].store().waiting_rows().unwrap_or_default();
            for w in &waiting {
                self.violation(format!("{name}: I1: {w} still waits to be sent"));
            }
            let held: BTreeMap<RowRef, AnyRow> = rows.into_iter().map(|r| (r.key(), r)).collect();
            let dead_end = |s: &iamb_sync::model::Stored| -> bool {
                let gone = |r: RowRef| {
                    srv.st
                        .rows
                        .get(&r)
                        .is_none_or(|x| x.row.meta().deleted_at.is_some())
                };
                match &s.row {
                    AnyRow::SnippetTag(l) => {
                        gone(RowRef {
                            kind: RowKind::Snippet,
                            id: l.snippet_id,
                        }) || gone(RowRef {
                            kind: RowKind::Tag,
                            id: l.tag_id,
                        })
                    }
                    AnyRow::SnippetAttachment(l) => {
                        gone(RowRef {
                            kind: RowKind::Snippet,
                            id: l.snippet_id,
                        }) || gone(RowRef {
                            kind: RowKind::Attachment,
                            id: l.attachment_id,
                        })
                    }
                    _ => false,
                }
            };
            for (k, s) in &srv.st.rows {
                let readable = srv.can_read(user, *k);
                if dead_end(s) && !held.contains_key(k) {
                    continue;
                }
                match (readable, held.get(k)) {
                    (true, None) if s.row.meta().deleted_at.is_none() && !blocked.contains(k) => {
                        if std::env::var("IAMB_SYNC_SIM_TRACE").is_ok() {
                            eprintln!(
                                "MISS {name} {k}: server {} rev {} by {:?}",
                                short(&s.row),
                                s.row.meta().rev,
                                s.updated_device
                            );
                            for r in s.row.references() {
                                eprintln!(
                                    "  ref {r}: server {:?} device {:?}",
                                    srv.st
                                        .rows
                                        .get(&r)
                                        .map(|x| (x.row.meta().rev, x.row.meta().deleted_at)),
                                    held.get(&r).map(|x| (x.meta().rev, x.meta().deleted_at))
                                );
                            }
                            for p in srv.processed.iter().filter(|p| p.key == *k) {
                                eprintln!(
                                    "  processed {:?} at {} {} {:?}",
                                    p.device,
                                    p.at - T0,
                                    p.status,
                                    p.rev
                                );
                            }
                        }
                        self.violation(format!("{name}: I1: misses {k}"));
                    }
                    (true, Some(h)) if h.meta().rev != s.row.meta().rev && !blocked.contains(k) => {
                        if !(s.row.meta().deleted_at.is_some() && h.meta().deleted_at.is_some())
                            || h.meta().rev > s.row.meta().rev
                        {
                            self.violation(format!(
                                "{name}: I1: {k} at rev {} on the device, {} on the server",
                                h.meta().rev,
                                s.row.meta().rev
                            ));
                        }
                    }
                    (false, Some(h)) if !blocked.contains(k) && h.meta().rev > 0 => {
                        if std::env::var("IAMB_SYNC_SIM_TRACE").is_ok() {
                            for r in held.values() {
                                if r.own_profile() == Some(k.id) || r.references().contains(k) {
                                    eprintln!(
                                        "LEAK {k} referenced by {} rev {} del {:?} server readable {}",
                                        r.key(),
                                        r.meta().rev,
                                        r.meta().deleted_at,
                                        srv.can_read(user, r.key())
                                    );
                                }
                            }
                        }
                        self.violation(format!(
                            "{name}: I1/I4: holds {k}, which its user may not read"
                        ));
                    }
                    _ => {}
                }
            }
            for (k, h) in &held {
                if !srv.st.rows.contains_key(k)
                    && !blocked.contains(k)
                    && h.meta().deleted_at.is_none()
                {
                    // A placeholder profile of a group the user cannot read is no content.
                    if k.kind == RowKind::Profile && h.meta().rev == 0 {
                        continue;
                    }
                    if std::env::var("IAMB_SYNC_SIM_TRACE").is_ok() {
                        for r in held.values() {
                            let inside = match r {
                                AnyRow::Snippet(x) => x.folder_id == k.id,
                                AnyRow::Folder(x) => x.parent_id == Some(k.id),
                                AnyRow::FolderShare(x) => x.folder_id == k.id,
                                _ => false,
                            };
                            if inside {
                                eprintln!(
                                    "HOLD {k} contains {} rev {} del {:?} on server {}",
                                    r.key(),
                                    r.meta().rev,
                                    r.meta().deleted_at,
                                    srv.st.rows.contains_key(&r.key())
                                );
                            }
                        }
                    }
                    self.violation(format!(
                        "{name}: I1: holds {k}, which the server does not have"
                    ));
                }
            }
        }
        // I6: the devices of one account agree on who holds each abbreviation and on permissions.
        let mut views: BTreeMap<Id, Vec<(BTreeMap<String, Id>, Vec<(Id, String)>)>> =
            BTreeMap::new();
        // Rows blocked on a device are the exception of I1, so their abbreviations are too.
        let mut blocked_ids: BTreeSet<Id> = BTreeSet::new();
        for d in 0..self.devs.len() {
            for (k, _) in self.devs[d].store().blocked_rows().unwrap_or_default() {
                blocked_ids.insert(k.id);
            }
        }
        for d in 0..self.devs.len() {
            let user = self.devs[d].user;
            let personal = self.devs[d].personal;
            let store = self.devs[d].store();
            let roots = store.access_roots().unwrap_or_default();
            let granted: BTreeMap<Id, i64> = roots
                .iter()
                .map(|r| (r.folder_id, r.granted_at.0))
                .collect();
            let folders: BTreeMap<Id, Folder> = store
                .list::<Folder>(iamb_store::ListOptions::LIVE)
                .unwrap_or_default()
                .into_iter()
                .map(|f| (f.meta.id, f))
                .collect();
            let root_of = |f: Id| -> Id {
                let mut cur = f;
                let mut n = 0;
                while let Some(p) = folders.get(&cur).and_then(|x| x.parent_id) {
                    if !folders.contains_key(&p) || n > 8 {
                        break;
                    }
                    cur = p;
                    n += 1;
                }
                cur
            };
            let mut holders: BTreeMap<String, ((u8, i64, Id, u64, Id), Id)> = BTreeMap::new();
            for s in store
                .list::<Snippet>(iamb_store::ListOptions::LIVE)
                .unwrap_or_default()
            {
                if blocked_ids.contains(&s.meta.id) {
                    continue;
                }
                let Some(f) = folders.get(&s.folder_id) else {
                    continue;
                };
                let origin = store.snippet_origin(s.meta.id).ok().flatten();
                let created = origin.map_or(u64::MAX, |o| {
                    if o.created_rev == 0 {
                        u64::MAX
                    } else {
                        o.created_rev
                    }
                });
                let mine = origin.and_then(|o| o.content_by) == Some(user);
                let root = root_of(f.meta.id);
                let class = if f.profile_id == personal {
                    if mine {
                        (1, 0, Id::from_bytes([0; 16]))
                    } else {
                        (2, 0, Id::from_bytes([0; 16]))
                    }
                } else {
                    (3, granted.get(&root).copied().unwrap_or(i64::MAX), root)
                };
                let key = (class.0, class.1, class.2, created, s.meta.id);
                let fold = s.abbreviation.fold_key();
                let entry = holders.entry(fold).or_insert((key, s.meta.id));
                if key < entry.0 {
                    *entry = (key, s.meta.id);
                }
            }
            let holders: BTreeMap<String, Id> =
                holders.into_iter().map(|(k, (_, v))| (k, v)).collect();
            let perms: Vec<(Id, String)> = roots
                .iter()
                .map(|r| (r.folder_id, r.permission.as_str().to_owned()))
                .collect();
            views.entry(user).or_default().push((holders, perms));
        }
        for (_, v) in views {
            for w in v.windows(2) {
                if w[0] != w[1] {
                    if std::env::var("IAMB_SYNC_SIM_TRACE").is_ok() {
                        for (abbr, id) in w[0].0.iter().chain(w[1].0.iter()) {
                            if w[0].0.get(abbr) != w[1].0.get(abbr) {
                                let key = RowRef {
                                    kind: RowKind::Snippet,
                                    id: *id,
                                };
                                let srv_row = srv
                                    .st
                                    .rows
                                    .get(&key)
                                    .map(|x| (x.row.meta().rev, x.row.meta().deleted_at));
                                eprintln!(
                                    "I6 {abbr}: {:?} vs {:?}; server {srv_row:?}",
                                    w[0].0.get(abbr),
                                    w[1].0.get(abbr)
                                );
                                for d in 0..self.devs.len() {
                                    let r =
                                        self.devs[d].agent.device.store.get_row(key).ok().flatten();
                                    let folder = match &r {
                                        Some(AnyRow::Snippet(x)) => self.devs[d]
                                            .agent
                                            .device
                                            .store
                                            .get_row(RowRef {
                                                kind: RowKind::Folder,
                                                id: x.folder_id,
                                            })
                                            .ok()
                                            .flatten()
                                            .map(|f| (f.meta().rev, f.meta().deleted_at)),
                                        _ => None,
                                    };
                                    let fid = match &r {
                                        Some(AnyRow::Snippet(x)) => Some(x.folder_id.to_string()),
                                        _ => None,
                                    };
                                    eprintln!(
                                        "   {}: {:?} folder {fid:?} {folder:?}",
                                        self.devs[d].name,
                                        r.map(|x| (x.meta().rev, x.meta().deleted_at))
                                    );
                                }
                            }
                        }
                    }
                    self.violation("I6: two devices of one account answer differently".to_owned());
                }
            }
        }
        // I2: every local change is in one of the twelve places of §1.
        let kept: Vec<Vec<iamb_core::sync::KeptChange>> = (0..self.devs.len())
            .map(|d| self.devs[d].store().kept_changes().unwrap_or_default())
            .collect();
        let blocked_rows: Vec<BTreeMap<RowRef, AnyRow>> = (0..self.devs.len())
            .map(|d| {
                let b: BTreeSet<RowRef> = self.devs[d]
                    .store()
                    .blocked_rows()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, _)| k)
                    .collect();
                b.into_iter()
                    .filter_map(|k| {
                        self.devs[d]
                            .store()
                            .get_row(k)
                            .ok()
                            .flatten()
                            .map(|r| (k, r))
                    })
                    .collect()
            })
            .collect();
        let device_ids: Vec<Option<Id>> = self.devs.iter().map(|d| d.device_id).collect();
        let ledger = self.ledger.clone();
        for c in &ledger {
            let place = self.place_of(c, &srv, &kept, &blocked_rows, &device_ids);
            match place {
                Some(p) => *self.result.places.entry(p).or_default() += 1,
                None => {
                    let name = self.devs[c.dev].name;
                    if std::env::var("IAMB_SYNC_SIM_TRACE").is_ok() {
                        eprintln!(
                            "LOST {name} #{} {} at {}: {:?}",
                            c.id,
                            c.key,
                            c.at - T0,
                            c.content.as_ref().map(short)
                        );
                        eprintln!(
                            "  server: {:?}",
                            srv.st.rows.get(&c.key).map(|s| (
                                s.row.meta().rev,
                                s.row.meta().deleted_at,
                                short(&s.row),
                                s.updated_device
                            ))
                        );
                        for h in srv.st.history.iter().filter(|h| h.key == c.key) {
                            eprintln!(
                                "  history rev {} by {:?}: {:?}",
                                h.rev,
                                h.stored.updated_device,
                                short(&h.stored.row)
                            );
                        }
                        for p in srv.processed.iter().filter(|p| p.key == c.key) {
                            eprintln!(
                                "  processed from {:?} at {}: {} rev {:?} {:?}",
                                p.device,
                                p.at - T0,
                                p.status,
                                p.rev,
                                p.row.as_ref().map(short)
                            );
                        }
                        for o in ledger.iter().filter(|o| o.key == c.key) {
                            eprintln!(
                                "  change #{} by {} at {} sup={} : {:?}",
                                o.id,
                                self.devs[o.dev].name,
                                o.at - T0,
                                o.superseded_unsent,
                                o.content.as_ref().map(short)
                            );
                        }
                        eprintln!(
                            "  kept on device: {:?}",
                            kept[c.dev]
                                .iter()
                                .filter(|k| k.key == c.key)
                                .collect::<Vec<_>>()
                        );
                        eprintln!("  devices: {:?}", device_ids);
                    }
                    if self.result.lost.len() < 10 {
                        self.result.lost.push(format!(
                            "{name}: change #{} to {} at {} ({})",
                            c.id,
                            c.key,
                            c.at - T0,
                            if c.content.is_some() { "put" } else { "delete" }
                        ));
                    }
                }
            }
        }
    }

    fn place_of(
        &self,
        c: &LocalChange,
        srv: &Server,
        kept: &[Vec<iamb_core::sync::KeptChange>],
        blocked: &[BTreeMap<RowRef, AnyRow>],
        device_ids: &[Option<Id>],
    ) -> Option<u8> {
        if let Some(s) = srv.st.rows.get(&c.key) {
            if same_content(&c.content, &s.row) {
                return Some(1);
            }
            if same_but_parent(&c.content, &s.row) {
                return Some(9);
            }
        }
        let in_history = srv.st.history.iter().any(|h| {
            h.key == c.key
                && (same_content(&c.content, &h.stored.row)
                    || same_but_parent(&c.content, &h.stored.row))
        });
        if in_history {
            return Some(2);
        }
        if kept[c.dev].iter().any(|k| {
            k.key == c.key
                && match (&c.content, &k.content) {
                    (None, _) => k.delete,
                    (Some(x), Some(y)) => strip(x) == strip(y),
                    _ => false,
                }
        }) {
            return Some(4);
        }
        // A delete the server refused is kept as a line that says what was not deleted.
        if c.content.is_none() && kept[c.dev].iter().any(|k| k.key == c.key && k.delete) {
            return Some(4);
        }
        if let Some(r) = blocked[c.dev].get(&c.key)
            && same_content(&c.content, r)
        {
            return Some(5);
        }
        if c.position_only {
            return Some(6);
        }
        // A share's change the server answered `superseded`: the device says so (place 9).
        if srv.processed.iter().any(|p| {
            Some(p.device) == device_ids[c.dev]
                && p.key == c.key
                && (p.status == "superseded"
                    || (p.status == "rejected" && p.key.kind == RowKind::FolderShare))
                && match (&c.content, &p.row) {
                    (None, None) => true,
                    (Some(x), Some(y)) => strip(x) == strip(y),
                    _ => false,
                }
        }) {
            return Some(9);
        }
        if c.superseded_unsent {
            return Some(7);
        }
        // Never received by the server, and a later change of the same row on the same device
        // took its place (the request was lost before the server saw it).
        let received = srv.processed.iter().any(|p| {
            Some(p.device) == device_ids[c.dev]
                && p.key == c.key
                && p.at >= c.at
                && match (&c.content, &p.row) {
                    (None, None) => true,
                    (Some(x), Some(y)) => strip(x) == strip(y),
                    _ => false,
                }
        });
        let later = self
            .ledger
            .iter()
            .any(|o| o.dev == c.dev && o.key == c.key && o.id > c.id);
        if !received && later {
            return Some(7);
        }
        // A row created and deleted again before any of its entries reached the server (place 7).
        if !received && c.content.is_none() && c.local_rev == 0 {
            return Some(7);
        }
        // The rows a folder's delete took with it are decided by the server (C6): a delete of a
        // row whose folder was deleted counts as the folder's.
        if c.content.is_none()
            && let Some(s) = srv.st.rows.get(&c.key)
            && s.row.meta().deleted_at.is_some()
        {
            return Some(1);
        }
        // A delete whose tombstone the server purged after 30 days, and whose id a later row
        // took again (a share's id is derived from its folder and group): the history of the
        // purged row is gone with it (C13, C22).
        if c.content.is_none() && c.at < self.t - 30 * DAY {
            return Some(2);
        }
        if c.lost_by_restore {
            return Some(12);
        }
        // A version the restore lost that a later change of the row, made on a device before it
        // learned of the restore, had taken the place of: that change came back and stands for it
        // (place 12).
        if let Some(r) = self.restored_at
            && c.at < r
            && self.ledger.iter().any(|o| {
                o.key == c.key
                    && o.id > c.id
                    && self.devs[o.dev].user == self.devs[c.dev].user
                    && o.at < self.epoch_seen_at.get(&o.dev).copied().unwrap_or(i64::MAX)
            })
        {
            return Some(12);
        }
        // The row left the device on the server's word (an access that ended): unsent changes went
        // to the kept list above; a row whose folder the server no longer shows the user.
        if !srv.can_read(self.devs[c.dev].user, c.key) {
            return Some(4);
        }
        None
    }
}

trait XorMarker {
    fn xor_marker(self) -> Self;
}

impl XorMarker for Id {
    /// A share's id derived from its folder (the model does not check the derivation of §3.4).
    fn xor_marker(self) -> Self {
        let mut b = *self.as_bytes();
        b[15] ^= 0x5a;
        b[14] ^= 0xa5;
        Id::from_bytes(b)
    }
}

fn u(t: i64) -> u64 {
    t.max(0) as u64
}

fn percentile(sorted: &[i64], p: f64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

#[test]
fn two_device_simulation() {
    let runs: u64 = std::env::var("IAMB_SYNC_SIM_RUNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    let seeds: Vec<u64> = match std::env::var("IAMB_SYNC_SIM_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        Some(s) => vec![s],
        None => (1..=runs).collect(),
    };
    let mut all_ms: Vec<i64> = Vec::new();
    let mut failures = Vec::new();
    let mut total_changes = 0usize;
    let mut lost = 0usize;
    let mut violations = 0usize;
    let mut restored = 0usize;
    let mut shares = 0usize;
    let mut undelivered = 0usize;
    let mut sign_outs = 0usize;
    let mut sign_ins = 0usize;
    let mut copies = 0usize;
    let mut places: BTreeMap<u8, usize> = BTreeMap::new();
    for seed in &seeds {
        let r = Sim::new(*seed).run();
        total_changes += r.changes;
        lost += r.lost.len();
        violations += r.violations.len();
        restored += usize::from(r.restored);
        shares += usize::from(r.shares);
        undelivered += r.undelivered;
        sign_outs += r.sign_outs;
        sign_ins += r.sign_ins;
        copies += usize::from(r.copied);
        for (p, n) in &r.places {
            *places.entry(*p).or_default() += n;
        }
        all_ms.extend(&r.agreement_ms);
        if !r.lost.is_empty() || !r.violations.is_empty() {
            failures.push(r);
        }
    }
    all_ms.sort_unstable();
    let within = all_ms.iter().filter(|m| **m <= 5_000).count();
    let summary = json!({
        "runs": seeds.len(),
        "changes": total_changes,
        "lost_edits": lost,
        "invariant_violations": violations,
        "runs_with_restore": restored,
        "runs_with_shares": shares,
        "runs_with_a_copied_data_directory": copies,
        "sign_outs": sign_outs,
        "sign_ins": sign_ins,
        "places": places,
        "agreement": {
            "writes_measured": all_ms.len(),
            "p50_ms": percentile(&all_ms, 0.5),
            "p90_ms": percentile(&all_ms, 0.9),
            "p95_ms": percentile(&all_ms, 0.95),
            "p99_ms": percentile(&all_ms, 0.99),
            "max_ms": all_ms.last().copied().unwrap_or(0),
            "within_5s": within,
            "undelivered": undelivered,
        },
    });
    if let Ok(path) = std::env::var("IAMB_SYNC_SIM_REPORT") {
        let _ = std::fs::write(
            path,
            serde_json::to_string_pretty(&summary).unwrap_or_default(),
        );
    }
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&summary).unwrap_or_default()
    );
    for f in failures.iter().take(20) {
        eprintln!(
            "seed {}: lost {:?}; violations {:?}",
            f.seed, f.lost, f.violations
        );
    }
    assert!(
        failures.is_empty(),
        "{} of {} runs failed",
        failures.len(),
        seeds.len()
    );
    assert!(
        all_ms.len() < 50 || percentile(&all_ms, 0.95) <= 5_000,
        "p95 agreement time {} ms is over 5 s",
        percentile(&all_ms, 0.95)
    );
}
