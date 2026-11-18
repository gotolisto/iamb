//! The soak run (`docs/test-strategy.md` §5): seeds the library, starts the supervisor, types
//! the key stream, edits the library, samples, kills the worker, types the canaries, stops the
//! service, scans and applies the pass rules.
//!
//! # Files of a run
//!
//! | Path (under `--out`) | Holds |
//! | --- | --- |
//! | `samples.jsonl` | One [`Sample`] per line |
//! | `report.json` | Options, totals, recoveries, expansion failures, canary contexts, the rule verdicts |
//! | `canary-scan.json` | The [`ScanReport`]: locations, files and bytes scanned, hits by canary number, form, location and offset |
//! | `summary.txt` | The verdicts in plain text (also printed) |
//! | `input/canaries.json` | The driver's own input: the only file the harness writes canaries to |
//! | `work/data/` | The data root (library, `logs/`, `run/`) |
//! | `work/tmp/` | The temp directory of the service processes (`TMPDIR`, `TMP`, `TEMP`) |
//! | `work/ipc-tap/` | Every byte of every service-endpoint connection |
//! | `work/service-stderr.log` | Standard error of the supervisor and the workers |
//!
//! Nothing the driver prints or writes outside `input/` holds a canary: reports name canaries by
//! number and context only.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use iamb_core::model::{Id, Timestamp};
use iamb_ipc::{Client, RequestBody, ResponseBody, Role, ServiceStatus};
use iamb_platform::{LocalEndpoint, PeerPolicy};
use iamb_service::library::{build_view, ensure_personal_profile};
use iamb_service::paths::DataDirs;
use iamb_store::Store;
use serde::Serialize;

use super::canary::{self, Canary, Context, Location, ScanReport};
use super::control::{self, Command as Ctl, ControlClient, Window};
use super::fixture::{self, Library, Shape, Token};
use super::host::HostDirs;
use super::netcheck::{self, NetCheck};
use super::probe::{self, ProcessSample};
use super::rng::SplitMix64;
use super::rules::{self, Evidence, Recovery, RuleResult, Sample, Verdict};
use super::{millis, unix_ms};

/// The schema of a samples line.
pub const SAMPLE_SCHEMA: &str = "iamb-soak-sample/1";

/// The schema of `report.json`.
pub const REPORT_SCHEMA: &str = "iamb-soak-report/1";

/// How long a recovery may take before the run gives up on it.
const GIVE_UP: Duration = Duration::from_secs(30);

/// When the kills happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kills {
    /// The run length divided by 12 (6 h for 72 h), at least 90 s.
    Auto,
    /// Every this long.
    Every(Duration),
    /// No kills.
    Off,
}

/// How a run goes.
#[derive(Debug, Clone)]
pub struct Options {
    /// The soak binary (the service processes are started from it).
    pub exe: PathBuf,
    /// The run length.
    pub duration: Duration,
    /// Where the files of the run go.
    pub out: PathBuf,
    /// The seed of the key stream, the library and the edits.
    pub seed: u64,
    /// Fixture snippets.
    pub library: usize,
    /// The average typing rate (bursts are five times faster).
    pub keys_per_second: f64,
    /// The sample interval.
    pub sample_every: Duration,
    /// The library edit interval.
    pub edit_every: Duration,
    /// The kill schedule.
    pub kills: Kills,
    /// Canary rounds (each types one canary per context).
    pub canary_rounds: usize,
    /// Run the networking-crate check in this workspace.
    pub net_check: Option<PathBuf>,
    /// More locations to scan for canaries (the lab adds the user profile, crash dumps and the
    /// packet capture).
    pub extra_scan: Vec<PathBuf>,
    /// No progress lines.
    pub quiet: bool,
}

/// The shortest gap between two kills: the supervisor restarts at once only after a worker ran
/// for 60 s (`HEALTHY_AFTER`); a closer kill would measure its 1 s backoff.
pub const MIN_KILL_GAP: Duration = Duration::from_secs(65);

/// The kill times for a run. Kills stay out of the first life's warm-up `W` plus four samples,
/// and out of the last `W` plus four samples, so the leak rules see a settled stretch of the
/// first worker and of the last one (each kill starts a new warm-up). `Auto` spreads up to 12
/// kills (§5.1: every 6 h of 72 h) evenly over that stretch, at least [`MIN_KILL_GAP`] apart;
/// `Every(d)` kills at the multiples of `d` inside it. A run too short for the stretch gets one
/// kill in the middle, unless kills are off.
pub fn kill_times(
    duration: Duration,
    sample_every: Duration,
    edit_every: Duration,
    kills: Kills,
) -> Vec<Duration> {
    let w = rules::window(duration, sample_every, edit_every);
    let from = w + sample_every * 4;
    let to = duration.saturating_sub(w + sample_every * 4);
    let mut out = Vec::new();
    match kills {
        Kills::Off => return out,
        Kills::Every(d) if d.is_zero() => return out,
        Kills::Every(d) => {
            let mut t = d;
            while t <= to {
                if t >= from {
                    out.push(t);
                }
                t += d;
            }
        }
        Kills::Auto if to > from => {
            let span = to.saturating_sub(from);
            let fit = u32::try_from(span.as_secs() / MIN_KILL_GAP.as_secs()).unwrap_or(u32::MAX);
            let n = fit.saturating_add(1).min(12);
            if n == 1 {
                out.push(from + span / 2);
            } else {
                out.extend((0..n).map(|i| from + span * i / (n - 1)));
            }
        }
        Kills::Auto => {}
    }
    if out.is_empty() {
        out.push(duration / 2);
    }
    out
}

/// The canary round times: evenly inside the run, at `(k + ½) / rounds`.
pub fn canary_times(duration: Duration, rounds: usize) -> Vec<Duration> {
    let n = u32::try_from(rounds).unwrap_or(u32::MAX);
    (0..n).map(|k| duration * (2 * k + 1) / (2 * n)).collect()
}

/// What a canary's typing showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanaryTyped {
    /// Its number.
    pub number: usize,
    /// Its context.
    pub context: Context,
    /// When, ms since the start.
    pub at_ms: u64,
    /// The field held exactly what was typed (an abbreviation typed with it in a suspended
    /// context did not expand; the one typed after it in `before-expansion` did).
    pub typed_as_expected: bool,
    /// The service reported the expected suspension state (`true` for secure field, excluded
    /// app and paused; `false` otherwise).
    pub suspension_as_expected: bool,
}

/// `report.json`.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// The schema.
    pub schema: &'static str,
    /// Wall clock at the start.
    pub started_unix_ms: u64,
    /// The OS family and architecture.
    pub os: String,
    /// `GITHUB_SHA` when set.
    pub git_sha: Option<String>,
    /// The options that shape the run.
    pub options: ReportOptions,
    /// Measured length.
    pub ran_ms: u64,
    /// Totals.
    pub totals: Totals,
    /// Injected kills.
    pub recoveries: Vec<Recovery>,
    /// Unexpected events.
    pub unexpected: Vec<String>,
    /// The first expansion failures (snippet number, shape, case mode).
    pub expansion_failures: Vec<String>,
    /// Per canary: context and checks (never the text).
    pub canaries: Vec<CanaryTyped>,
    /// The networking-crate check, when run.
    pub net_check: Option<NetCheck>,
    /// Every rule.
    pub rules: Vec<RuleResult>,
    /// No rule failed.
    pub passed: bool,
}

/// The options in the report.
#[derive(Debug, Clone, Serialize)]
pub struct ReportOptions {
    /// Planned length, s.
    pub duration_s: u64,
    /// Seed.
    pub seed: u64,
    /// Library size.
    pub library: usize,
    /// Average typing rate asked for.
    pub keys_per_second: f64,
    /// Sample interval, s.
    pub sample_every_s: u64,
    /// Edit interval, s.
    pub edit_every_s: u64,
    /// Planned kill times, s.
    pub kill_times_s: Vec<u64>,
    /// Canary rounds.
    pub canary_rounds: usize,
    /// The rules' window `W`, s.
    pub window_s: u64,
}

/// Totals of the run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Totals {
    /// Keys typed.
    pub keys: u64,
    /// Keys per second achieved while typing.
    pub keys_per_second: f64,
    /// Expansions checked and correct.
    pub expansions_ok: u64,
    /// Expansions checked and wrong.
    pub expansions_failed: u64,
    /// Library edits.
    pub edits: u64,
    /// Samples taken.
    pub samples: u64,
    /// Worker restarts the supervisor logged.
    pub supervisor_logged_restarts: u64,
    /// Worker CPU time over the run (all lives), ms.
    pub worker_cpu_ms: u64,
    /// Supervisor CPU time, ms.
    pub supervisor_cpu_ms: u64,
    /// Max worker memory (private, else resident), KiB.
    pub worker_max_memory_kb: u64,
    /// Max supervisor memory, KiB.
    pub supervisor_max_memory_kb: u64,
}

struct Paths {
    out: PathBuf,
    input: PathBuf,
    work: PathBuf,
    data: PathBuf,
    tmp: PathBuf,
    tap: PathBuf,
    control: PathBuf,
    stderr: PathBuf,
}

impl Paths {
    fn new(out: &Path) -> Self {
        let work = out.join("work");
        Self {
            out: out.to_path_buf(),
            input: out.join("input").join("canaries.json"),
            data: work.join("data"),
            tmp: work.join("tmp"),
            tap: work.join("ipc-tap"),
            control: work.join("control"),
            stderr: work.join("service-stderr.log"),
            work,
        }
    }

    fn host(&self) -> HostDirs<'_> {
        HostDirs {
            data: &self.data,
            control: &self.control,
            tap: &self.tap,
        }
    }
}

fn io_err(what: &str, path: &Path) -> impl Fn(std::io::Error) -> String {
    let p = path.display().to_string();
    let what = what.to_owned();
    move |e| format!("{what} {p}: {e}")
}

/// Puts the fixture library into the store at `root`.
fn seed_library(root: &Path, library: &Library) -> Result<(), String> {
    let mut store = Store::open(root).map_err(|e| format!("cannot open the store: {e}"))?;
    let mut n = 0_u64;
    let mut new_id = || {
        n += 1;
        let mut b = [0_u8; 16];
        b[..4].copy_from_slice(b"seed");
        b[8..].copy_from_slice(&n.to_be_bytes());
        Id::from_bytes(b)
    };
    let now = Timestamp(i64::try_from(unix_ms()).unwrap_or(0));
    let profile = ensure_personal_profile(&mut store, &mut new_id, now)
        .map_err(|e| format!("cannot create the profile: {e}"))?;
    let folder = build_view(&store, profile, 1)
        .map_err(|e| format!("cannot read the library: {e}"))?
        .folders
        .first()
        .map(|f| f.meta.id)
        .ok_or("the profile has no folder")?;
    let snippets = library
        .snippets
        .iter()
        .map(|s| s.to_snippet(folder, now))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("bad fixture snippet: {e}"))?;
    store
        .write(|w| {
            for s in &snippets {
                w.put(s, now)?;
            }
            Ok(())
        })
        .map_err(|e| format!("cannot write the library: {e}"))
}

/// The next time of a schedule after `due` passed at `now`: one interval later, or one
/// interval after `now` when the run fell behind.
fn next_after(due: Duration, every: Duration, now: Duration) -> Duration {
    let next = due + every;
    if next <= now { now + every } else { next }
}

struct Run<'a> {
    opts: &'a Options,
    paths: Paths,
    start: Instant,
    library: Library,
    rng: SplitMix64,
    endpoint: Arc<dyn LocalEndpoint>,
    control: ControlClient,
    client: Client,
    supervisor: Child,
    worker_pid: u32,
    worker_life: u32,
    life_started_ms: u64,
    samples_file: BufWriter<File>,
    samples: Vec<Sample>,
    ok: u64,
    failed: u64,
    keys: u64,
    typing: Duration,
    edits: u64,
    failures: Vec<String>,
    recoveries: Vec<Recovery>,
    unexpected: Vec<String>,
    canaries: Vec<Canary>,
    typed: Vec<CanaryTyped>,
    next_sample: Duration,
    next_edit: Duration,
    kill_at: VecDeque<Duration>,
    canary_at: VecDeque<(Duration, usize)>,
    /// The editor's expected text in the current unit.
    field: String,
    last_progress: Instant,
}

fn connect_control(
    endpoint: &dyn LocalEndpoint,
    dir: &Path,
) -> Result<(ControlClient, u32), String> {
    let mut c = ControlClient::connect(endpoint, dir, Duration::from_millis(500))?;
    let pid = c.pid()?;
    Ok((c, pid))
}

fn connect_ipc(endpoint: &dyn LocalEndpoint, data: &Path) -> Result<Client, String> {
    let mut client = Client::connect(
        endpoint,
        &DataDirs::new(data).service_endpoint(),
        PeerPolicy::SameUser,
        Role::Ui,
        env!("CARGO_PKG_VERSION"),
        Duration::from_secs(2),
    )
    .map_err(|e| format!("service endpoint: {e}"))?;
    client.set_call_timeout(Duration::from_secs(30));
    Ok(client)
}

#[allow(clippy::cast_precision_loss, reason = "a progress line")]
fn megabytes(p: &ProcessSample) -> String {
    p.memory_kb().map_or_else(
        || "?".to_owned(),
        |kb| format!("{:.1} MB", kb as f64 / 1024.0),
    )
}

const fn worker_of(s: &Sample) -> &ProcessSample {
    &s.worker
}

const fn supervisor_of(s: &Sample) -> &ProcessSample {
    &s.supervisor
}

fn status(client: &mut Client) -> Result<ServiceStatus, String> {
    match client.call(RequestBody::GetStatus {}) {
        Ok(ResponseBody::Status { status }) => Ok(status),
        Ok(_) => Err("unexpected answer to get_status".to_owned()),
        Err(e) => Err(format!("get_status: {e}")),
    }
}

impl Run<'_> {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    fn at_ms(&self) -> u64 {
        millis(self.elapsed())
    }

    fn say(&self, line: &str) {
        if !self.opts.quiet {
            eprintln!("[{:>7.1} s] {line}", self.elapsed().as_secs_f64());
        }
    }

    /// The pause before the next key of a token: bursts at five times the average rate for
    /// one token in five, else exponential gaps; the mean is the average rate.
    fn key_gap(&mut self, burst: bool) -> Duration {
        let scale = self.opts.keys_per_second / 3.0;
        let secs = if burst {
            1.0 / 15.0 / scale
        } else {
            self.rng.exponential(0.4 / scale).min(3.0 / scale)
        };
        Duration::from_secs_f64(secs)
    }

    fn type_keys(&mut self, keys: &str) -> Result<(), String> {
        let burst = self.rng.percent(20);
        self.type_at(keys, burst)
    }

    /// Canaries go in one burst, so a round does not hold up the samples for long.
    fn type_burst(&mut self, keys: &str) -> Result<(), String> {
        self.type_at(keys, true)
    }

    fn type_at(&mut self, keys: &str, burst: bool) -> Result<(), String> {
        for ch in keys.chars() {
            let gap = self.key_gap(burst);
            if gap >= Duration::from_millis(1) {
                thread::sleep(gap);
            }
            let t = Instant::now();
            self.control.ok(&Ctl::Type {
                text: ch.to_string(),
            })?;
            self.typing += gap + t.elapsed();
            self.keys += 1;
        }
        Ok(())
    }

    fn reset(&mut self) -> Result<(), String> {
        self.control.ok(&Ctl::Reset)?;
        self.field.clear();
        Ok(())
    }

    /// Checks the editor against `expected`; on a mismatch records snippet `index`.
    fn check_expansion(&mut self, index: usize, expected: &str) -> Result<bool, String> {
        let actual = self.control.text(Window::Editor)?;
        if actual == expected {
            self.ok += 1;
            Ok(true)
        } else {
            self.failed += 1;
            let s = &self.library.snippets[index];
            if self.failures.len() < 50 {
                self.failures.push(format!(
                    "at {} s: snippet {} ({}, {}) left {} chars, expected {}",
                    self.at_ms() / 1000,
                    s.index,
                    s.shape.as_str(),
                    fixture::case_name(s.case),
                    actual.chars().count(),
                    expected.chars().count()
                ));
            }
            Ok(false)
        }
    }

    /// Types one token; false when the unit must stop (an expansion failed).
    fn token(&mut self, token: &Token) -> Result<bool, String> {
        match token {
            Token::Word(w) => {
                let keys = format!("{w} ");
                self.type_keys(&keys)?;
                self.field.push_str(&keys);
                Ok(true)
            }
            Token::Abbreviation { snippet, typed } => {
                let s = self.library.snippets[*snippet].clone();
                let expansion = s.expansion(typed);
                if s.shape == Shape::Immediate {
                    self.type_keys(typed)?;
                    let expected = format!("{}{expansion}", self.field);
                    if !self.check_expansion(s.index, &expected)? {
                        return Ok(false);
                    }
                    self.type_keys(" ")?;
                    self.field = expected + " ";
                } else {
                    self.type_keys(&format!("{typed} "))?;
                    let expected = format!("{}{expansion} ", self.field);
                    if !self.check_expansion(s.index, &expected)? {
                        return Ok(false);
                    }
                    self.field = expected;
                }
                Ok(true)
            }
        }
    }

    fn drain_events(&mut self) {
        while let Ok(Some(_)) = self.client.next_event(Some(Duration::from_millis(1))) {}
    }

    fn supervisor_alive(&mut self) -> bool {
        matches!(self.supervisor.try_wait(), Ok(None))
    }

    fn sample(&mut self) -> Result<(), String> {
        if !self.supervisor_alive() {
            self.unexpected
                .push(format!("supervisor exited at {} s", self.at_ms() / 1000));
            return Err("the supervisor exited".to_owned());
        }
        let st = status(&mut self.client)?;
        if st.pid != self.worker_pid {
            return Err(format!(
                "the worker changed from {} to {}",
                self.worker_pid, st.pid
            ));
        }
        self.drain_events();
        let sample = Sample {
            schema: SAMPLE_SCHEMA.to_owned(),
            t_ms: self.at_ms(),
            unix_ms: unix_ms(),
            supervisor: probe::sample(self.supervisor.id()),
            worker: probe::sample(self.worker_pid),
            worker_life: self.worker_life,
            life_started_ms: self.life_started_ms,
            expansions_ok: self.ok,
            expansions_failed: self.failed,
            keys: self.keys,
            edits: self.edits,
            kills: u64::try_from(self.recoveries.len()).unwrap_or(0),
        };
        let line = serde_json::to_string(&sample).map_err(|e| e.to_string())?;
        writeln!(self.samples_file, "{line}")
            .and_then(|()| self.samples_file.flush())
            .map_err(|e| format!("cannot write the samples: {e}"))?;
        if self.last_progress.elapsed() >= Duration::from_mins(1) || self.samples.is_empty() {
            self.last_progress = Instant::now();
            self.say(&format!(
                "worker {} (life {}), supervisor {}; expansions {}/{}; keys {}; edits {}; kills {}",
                megabytes(&sample.worker),
                self.worker_life,
                megabytes(&sample.supervisor),
                self.ok,
                self.ok + self.failed,
                self.keys,
                self.edits,
                self.recoveries.len()
            ));
        }
        self.samples.push(sample);
        Ok(())
    }

    fn edit(&mut self) -> Result<(), String> {
        let index = self.rng.index(self.library.snippets.len());
        let body = fixture::body(&mut self.rng);
        let id = self.library.snippets[index].id;
        let before = status(&mut self.client)?.revision;
        let mut snippet = match self.client.call(RequestBody::GetSnippet { id }) {
            Ok(ResponseBody::Snippet { snippet }) => snippet,
            other => return Err(format!("get_snippet: {other:?}")),
        };
        snippet.body.clone_from(&body);
        match self.client.call(RequestBody::PutSnippet { snippet }) {
            Ok(ResponseBody::Saved { .. }) => {}
            other => return Err(format!("put_snippet: {other:?}")),
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while status(&mut self.client)?.revision == before {
            if Instant::now() > deadline {
                return Err("the edit never reached the snapshot".to_owned());
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.library.snippets[index].body = body;
        self.edits += 1;
        self.drain_events();
        Ok(())
    }

    /// Waits for a new worker after `old` died: control endpoint, IPC `Health`, a probe
    /// expansion. Returns the times from `since`.
    fn recover(&mut self, old: u32, since: Instant) -> Recovery {
        let at_ms = millis(since.duration_since(self.start));
        let mut r = Recovery {
            at_ms,
            old_pid: old,
            new_pid: 0,
            control_ms: 0,
            health_ms: 0,
            probe_ms: 0,
            recovered: false,
        };
        let probe = self
            .library
            .first_of(Shape::Delimiter)
            .cloned()
            .unwrap_or_else(|| self.library.snippets[0].clone());
        let typed = probe.abbreviation.clone();
        while since.elapsed() < GIVE_UP {
            if r.control_ms == 0 {
                match connect_control(self.endpoint.as_ref(), &self.paths.control) {
                    Ok((c, pid)) if pid != old => {
                        self.control = c;
                        r.new_pid = pid;
                        r.control_ms = millis(since.elapsed()).max(1);
                    }
                    _ => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                }
            }
            if r.health_ms == 0 {
                let healthy =
                    connect_ipc(self.endpoint.as_ref(), &self.paths.data).and_then(|mut c| match c
                        .call(RequestBody::Health {})
                    {
                        Ok(ResponseBody::Health { .. }) => Ok(c),
                        other => Err(format!("health: {other:?}")),
                    });
                let Ok(c) = healthy else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                self.client = c;
                r.health_ms = millis(since.elapsed()).max(1);
            }
            let expected = format!("probe {} ", probe.expansion(&typed));
            let ok = self
                .control
                .ok(&Ctl::Focus {
                    window: Window::Editor,
                })
                .and_then(|()| self.control.ok(&Ctl::Reset))
                .and_then(|()| {
                    self.control.ok(&Ctl::Type {
                        text: format!("probe {typed} "),
                    })
                })
                .and_then(|()| self.control.text(Window::Editor));
            match ok {
                Ok(text) if text == expected => {
                    r.probe_ms = millis(since.elapsed()).max(1);
                    r.recovered = true;
                    break;
                }
                Ok(_) => thread::sleep(Duration::from_millis(5)),
                Err(_) => {
                    // The new worker went away too: start over.
                    r.control_ms = 0;
                    r.health_ms = 0;
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
        if r.recovered {
            self.worker_pid = r.new_pid;
            self.worker_life += 1;
            self.life_started_ms = at_ms + r.probe_ms;
            self.field.clear();
            let _ = self.control.ok(&Ctl::Reset);
        }
        r
    }

    fn kill(&mut self) -> Result<(), String> {
        let old = self.worker_pid;
        let since = Instant::now();
        probe::hard_kill(old)?;
        let r = self.recover(old, since);
        self.say(&format!(
            "killed worker {old}: control {} ms, health {} ms, probe expansion {} ms",
            r.control_ms, r.health_ms, r.probe_ms
        ));
        let recovered = r.recovered;
        self.recoveries.push(r);
        if recovered {
            Ok(())
        } else {
            Err(format!(
                "the worker did not come back within {} s",
                GIVE_UP.as_secs()
            ))
        }
    }

    /// The control or IPC connection broke outside a kill: record it and wait for a worker.
    fn lost_worker(&mut self, why: &str) -> Result<(), String> {
        if !self.supervisor_alive() {
            self.unexpected
                .push(format!("supervisor exited at {} s", self.at_ms() / 1000));
            return Err("the supervisor exited".to_owned());
        }
        let old = self.worker_pid;
        let since = Instant::now();
        let r = self.recover(old, since);
        self.unexpected.push(format!(
            "worker {old} lost at {} s ({why}); {}",
            r.at_ms / 1000,
            if r.recovered {
                format!("worker {} answered after {} ms", r.new_pid, r.probe_ms)
            } else {
                "no worker came back".to_owned()
            }
        ));
        if r.recovered {
            Ok(())
        } else {
            Err("the worker did not come back".to_owned())
        }
    }

    fn suspended(&mut self) -> Result<bool, String> {
        Ok(status(&mut self.client)?.suspended)
    }

    fn type_canary(&mut self, c: &Canary) -> Result<CanaryTyped, String> {
        let probe = self
            .library
            .first_of(Shape::Delimiter)
            .cloned()
            .ok_or("no delimiter snippet")?;
        let anywhere = self
            .library
            .snippets
            .iter()
            .find(|s| s.shape == Shape::DelimiterAnywhere)
            .cloned();
        self.control.ok(&Ctl::Focus {
            window: Window::Editor,
        })?;
        self.reset()?;
        let mut record = CanaryTyped {
            number: c.number,
            context: c.context,
            at_ms: self.at_ms(),
            typed_as_expected: false,
            suspension_as_expected: false,
        };
        let literal = format!("{} {} ", c.text, probe.abbreviation);
        match c.context {
            Context::Ordinary => {
                record.suspension_as_expected = !self.suspended()?;
                let keys = format!("note {} ", c.text);
                self.type_burst(&keys)?;
                record.typed_as_expected = self.control.text(Window::Editor)? == keys;
            }
            Context::BeforeExpansion => {
                record.suspension_as_expected = !self.suspended()?;
                if let Some(s) = anywhere {
                    let typed = s.abbreviation.clone();
                    self.type_burst(&format!("note {}{typed} ", c.text))?;
                    let expected = format!("note {}{} ", c.text, s.expansion(&typed));
                    record.typed_as_expected = self.check_expansion(s.index, &expected)?;
                } else {
                    // A library too small for the shape: type it as ordinary text.
                    let keys = format!("note {} ", c.text);
                    self.type_burst(&keys)?;
                    record.typed_as_expected = self.control.text(Window::Editor)? == keys;
                }
            }
            Context::SecureField | Context::ExcludedApp => {
                let window = if c.context == Context::SecureField {
                    Window::Secure
                } else {
                    Window::Excluded
                };
                self.control.ok(&Ctl::Focus { window })?;
                record.suspension_as_expected = self.suspended()?;
                self.type_burst(&literal)?;
                record.typed_as_expected = self.control.text(window)? == literal;
                self.control.ok(&Ctl::Focus {
                    window: Window::Editor,
                })?;
            }
            Context::Paused => {
                match self.client.call(RequestBody::SetPaused { paused: true }) {
                    Ok(ResponseBody::Status { .. }) => {}
                    other => return Err(format!("set_paused: {other:?}")),
                }
                let st = status(&mut self.client)?;
                record.suspension_as_expected = st.paused && st.suspended;
                self.type_burst(&literal)?;
                record.typed_as_expected = self.control.text(Window::Editor)? == literal;
                match self.client.call(RequestBody::SetPaused { paused: false }) {
                    Ok(ResponseBody::Status { .. }) => {}
                    other => return Err(format!("set_paused: {other:?}")),
                }
                record.suspension_as_expected &= !self.suspended()?;
            }
        }
        self.reset()?;
        Ok(record)
    }

    fn canary_round(&mut self, round: usize) -> Result<(), String> {
        let these: Vec<Canary> = self
            .canaries
            .iter()
            .filter(|c| c.round == round)
            .cloned()
            .collect();
        for c in &these {
            self.light()?;
            let record = self.type_canary(c)?;
            self.typed.push(record);
        }
        self.say(&format!(
            "canary round {} typed ({} canaries)",
            round + 1,
            these.len()
        ));
        Ok(())
    }

    fn heavy_due(&self) -> bool {
        let now = self.elapsed();
        self.kill_at.front().is_some_and(|t| now >= *t)
            || self.canary_at.front().is_some_and(|(t, _)| now >= *t)
    }

    /// Samples and edits when due (between tokens).
    fn light(&mut self) -> Result<(), String> {
        if self.elapsed() >= self.next_sample {
            self.sample()?;
            self.next_sample = next_after(self.next_sample, self.opts.sample_every, self.elapsed());
        }
        if self.elapsed() >= self.next_edit {
            self.edit()?;
            self.next_edit = next_after(self.next_edit, self.opts.edit_every, self.elapsed());
        }
        Ok(())
    }

    /// Kills and canary rounds when due (between units).
    fn heavy(&mut self) -> Result<(), String> {
        let now = self.elapsed();
        if self.canary_at.front().is_some_and(|(t, _)| now >= *t)
            && let Some((_, round)) = self.canary_at.pop_front()
        {
            self.canary_round(round)?;
        }
        if self.kill_at.front().is_some_and(|t| now >= *t) {
            self.kill_at.pop_front();
            // A sample just before, so the rules see the old life up to the kill.
            self.sample()?;
            self.kill()?;
        }
        Ok(())
    }

    fn unit(&mut self) -> Result<(), String> {
        let unit = fixture::unit(&mut self.rng, &self.library);
        self.reset()?;
        for token in &unit.tokens {
            if self.heavy_due() || self.elapsed() >= self.opts.duration {
                break;
            }
            self.light()?;
            if !self.token(token)? {
                break;
            }
        }
        Ok(())
    }

    /// One step of the loop; a broken connection becomes an unexpected worker loss.
    fn step(&mut self) -> Result<(), String> {
        match self.unit().and_then(|()| self.heavy()) {
            Ok(()) => Ok(()),
            Err(e) if self.recoveries.last().is_some_and(|r| !r.recovered) => Err(e),
            Err(e) => self.lost_worker(&e),
        }
    }
}

/// Runs a soak and writes its files; the report says whether it passed.
#[allow(clippy::too_many_lines, reason = "the run, start to finish")]
pub fn run(opts: &Options) -> Result<Report, String> {
    let paths = Paths::new(&opts.out);
    for dir in [&paths.out, &paths.work, &paths.tmp, &paths.tap] {
        fs::create_dir_all(dir).map_err(io_err("cannot create", dir))?;
    }
    // A Unix socket path holds at most 104 (macOS) or 108 (Linux) bytes.
    for socket in [
        DataDirs::new(&paths.data).service_endpoint().socket_path(),
        control::endpoint_name(&paths.control).socket_path(),
    ] {
        if cfg!(unix) && socket.as_os_str().len() > 100 {
            return Err(format!(
                "{} is too long for a Unix socket: use a shorter (or relative) --out",
                socket.display()
            ));
        }
    }
    if paths.data.exists() {
        return Err(format!(
            "{} exists: give every run an --out of its own",
            paths.data.display()
        ));
    }
    let started_unix_ms = unix_ms();
    let library = Library::generate(opts.seed, opts.library)?;
    seed_library(&paths.data, &library)?;
    let canaries = canary::generate(opts.canary_rounds, |t| library.would_match(t))?;
    let input_dir = paths.input.parent().unwrap_or(&paths.out).to_path_buf();
    fs::create_dir_all(&input_dir).map_err(io_err("cannot create", &input_dir))?;
    fs::write(
        &paths.input,
        serde_json::to_vec_pretty(&canaries).map_err(|e| e.to_string())?,
    )
    .map_err(io_err("cannot write", &paths.input))?;

    let stderr = File::create(&paths.stderr).map_err(io_err("cannot create", &paths.stderr))?;
    let supervisor = Command::new(&opts.exe)
        .args(paths.host().supervisor_args())
        .env("TMPDIR", &paths.tmp)
        .env("TMP", &paths.tmp)
        .env("TEMP", &paths.tmp)
        .env_remove("IAMB_TEST_FAULT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("cannot start the supervisor: {e}"))?;
    let mut supervisor = Some(supervisor);

    let endpoint: Arc<dyn LocalEndpoint> = iamb_ipc::native_endpoint();
    let begin = Instant::now();
    let connected = loop {
        let attempt = connect_control(endpoint.as_ref(), &paths.control).and_then(|(c, pid)| {
            connect_ipc(endpoint.as_ref(), &paths.data).map(|client| (c, pid, client))
        });
        match attempt {
            Ok(x) => break Ok(x),
            Err(e) if begin.elapsed() > Duration::from_mins(1) => {
                break Err(format!("the service did not start: {e}"));
            }
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    };
    let (control, worker_pid, client) = match connected {
        Ok(x) => x,
        Err(e) => {
            if let Some(mut s) = supervisor.take() {
                let _ = s.kill();
                let _ = s.wait();
            }
            return Err(e);
        }
    };
    let samples_path = paths.out.join("samples.jsonl");
    let samples_file = BufWriter::new(
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&samples_path)
            .map_err(io_err("cannot create", &samples_path))?,
    );
    let kills = kill_times(
        opts.duration,
        opts.sample_every,
        opts.edit_every,
        opts.kills,
    );
    let mut r = Run {
        opts,
        paths,
        start: Instant::now(),
        library,
        rng: SplitMix64::new(opts.seed),
        endpoint,
        control,
        client,
        supervisor: supervisor.take().ok_or("no supervisor")?,
        worker_pid,
        worker_life: 1,
        life_started_ms: 0,
        samples_file,
        samples: Vec::new(),
        ok: 0,
        failed: 0,
        keys: 0,
        typing: Duration::ZERO,
        edits: 0,
        failures: Vec::new(),
        recoveries: Vec::new(),
        unexpected: Vec::new(),
        canaries,
        typed: Vec::new(),
        next_sample: Duration::ZERO,
        next_edit: opts.edit_every,
        kill_at: kills.iter().copied().collect(),
        canary_at: canary_times(opts.duration, opts.canary_rounds)
            .into_iter()
            .zip(0..)
            .collect(),
        field: String::new(),
        last_progress: Instant::now(),
    };
    r.say(&format!(
        "soak started: {} s, {} snippets, seed {}, {} kills planned, {} canaries",
        opts.duration.as_secs(),
        opts.library,
        opts.seed,
        kills.len(),
        r.canaries.len()
    ));

    let mut aborted: Option<String> = None;
    let setup = r
        .client
        .call(RequestBody::PutExclusion {
            app_id: control::EXCLUDED_APP.to_owned(),
            display_name: "Soak excluded app".to_owned(),
            excluded: true,
        })
        .map_err(|e| format!("put_exclusion: {e}"))
        .and_then(|_| r.sample());
    if let Err(e) = setup {
        aborted = Some(e);
    }
    r.next_sample = opts.sample_every;
    while aborted.is_none() && r.elapsed() < opts.duration {
        if let Err(e) = r.step() {
            aborted = Some(e);
        }
    }
    // Rounds or kills the loop did not reach (a very short run): do them now.
    while aborted.is_none()
        && let Some((_, round)) = r.canary_at.pop_front()
    {
        if let Err(e) = r.canary_round(round) {
            aborted = Some(e);
        }
    }
    if aborted.is_none()
        && let Err(e) = r.sample()
    {
        aborted = Some(e);
    }
    let ran_ms = r.at_ms();

    // Stop: the worker exits 0, so the supervisor stops too.
    let mut clean_stop = r.control.ok(&Ctl::Stop).is_ok();
    let stop_deadline = Instant::now() + Duration::from_secs(15);
    let supervisor_status = loop {
        match r.supervisor.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < stop_deadline => thread::sleep(Duration::from_millis(20)),
            _ => break None,
        }
    };
    if supervisor_status.is_none_or(|s| !s.success()) {
        clean_stop = false;
        let _ = r.supervisor.kill();
        let _ = r.supervisor.wait();
    }
    let worker_gone = Instant::now() + Duration::from_secs(5);
    while probe::sample(r.worker_pid).alive && Instant::now() < worker_gone {
        thread::sleep(Duration::from_millis(20));
    }
    if probe::sample(r.worker_pid).alive {
        clean_stop = false;
        let _ = probe::hard_kill(r.worker_pid);
    }
    let _ = r.samples_file.flush();

    // The supervisor's own count of worker restarts.
    let mut logged_restarts = 0_u64;
    if let Ok(entries) = fs::read_dir(r.paths.data.join("logs")) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("supervisor")
                && let Ok(text) = fs::read_to_string(e.path())
            {
                logged_restarts += u64::try_from(
                    text.lines()
                        .filter(|l| {
                            l.contains("exited abnormally")
                                || l.contains("no heartbeat from worker")
                        })
                        .count(),
                )
                .unwrap_or(0);
            }
        }
    }
    let injected = u64::try_from(r.recoveries.len()).unwrap_or(0);
    if logged_restarts > injected {
        r.unexpected.push(format!(
            "the supervisor logged {logged_restarts} worker restarts for {injected} injected kills"
        ));
    }

    // The scan.
    let mut locations = vec![
        Location {
            label: "data-root".to_owned(),
            path: r.paths.data.clone(),
        },
        Location {
            label: "temp-dir".to_owned(),
            path: r.paths.tmp.clone(),
        },
        Location {
            label: "ipc-tap".to_owned(),
            path: r.paths.tap.clone(),
        },
        Location {
            label: "service-stderr".to_owned(),
            path: r.paths.stderr.clone(),
        },
        Location {
            label: "control-dir".to_owned(),
            path: r.paths.control.clone(),
        },
        Location {
            label: "harness-samples".to_owned(),
            path: samples_path.clone(),
        },
    ];
    for (i, p) in opts.extra_scan.iter().enumerate() {
        locations.push(Location {
            label: format!("extra-{}", i + 1),
            path: p.clone(),
        });
    }
    let scan = canary::scan(
        &r.canaries,
        &locations,
        std::slice::from_ref(&r.paths.input),
    );
    // The scanner must find what is there: every canary in the driver's own input.
    let control_scan = canary::scan(
        &r.canaries,
        &[Location {
            label: "driver-input".to_owned(),
            path: r.paths.input.clone(),
        }],
        &[],
    );
    let found_in_input = r
        .canaries
        .iter()
        .filter(|c| {
            control_scan
                .hits
                .iter()
                .any(|h| h.canary == c.number && h.form == "utf8")
        })
        .count();
    write_json(&r.paths.out.join("canary-scan.json"), &scan)?;

    let net_check = opts.net_check.as_deref().map(netcheck::run);

    // The rules.
    let evidence = Evidence {
        duration: opts.duration,
        sample_every: opts.sample_every,
        edit_every: opts.edit_every,
        samples: r.samples.clone(),
        recoveries: r.recoveries.clone(),
        kills_disabled: kills.is_empty(),
        unexpected: r.unexpected.clone(),
        expansions_ok: r.ok,
        expansions_failed: r.failed,
    };
    let mut results = rules::evaluate(&evidence);
    results.extend(canary_rules(&r.canaries, &r.typed, &scan, found_in_input));
    if let Some(check) = &net_check {
        results.push(RuleResult {
            rule: "no-networking-crate".to_owned(),
            verdict: if check.passed() {
                Verdict::Pass
            } else {
                Verdict::Fail
            },
            value: check.error.clone().unwrap_or_else(|| {
                if check.denied.is_empty() {
                    format!("{} packages, none denied", check.packages)
                } else {
                    format!("denied: {}", check.denied.join(", "))
                }
            }),
            limit: "none of the deny list".to_owned(),
        });
    }
    results.push(RuleResult {
        rule: "run-completed".to_owned(),
        verdict: if aborted.is_none() {
            Verdict::Pass
        } else {
            Verdict::Fail
        },
        value: aborted
            .clone()
            .unwrap_or_else(|| format!("{} s", ran_ms / 1000)),
        limit: format!("{} s", opts.duration.as_secs()),
    });
    results.push(RuleResult {
        rule: "clean-stop".to_owned(),
        verdict: if clean_stop {
            Verdict::Pass
        } else {
            Verdict::Fail
        },
        value: if clean_stop {
            "worker and supervisor exited with status 0".to_owned()
        } else {
            "had to be killed".to_owned()
        },
        limit: "exit status 0".to_owned(),
    });
    let passed = results.iter().all(|x| x.verdict != Verdict::Fail);

    let max_mem = |f: fn(&Sample) -> &ProcessSample| -> u64 {
        r.samples
            .iter()
            .filter_map(|s| f(s).memory_kb())
            .max()
            .unwrap_or(0)
    };
    let cpu = |f: fn(&Sample) -> &ProcessSample, per_life: bool| -> u64 {
        // Sum of each life's last CPU reading (CPU time restarts with a new process).
        let mut total = 0;
        let mut last: Option<(u32, u64)> = None;
        for s in &r.samples {
            let life = if per_life { s.worker_life } else { 1 };
            if let Some(ms) = f(s).cpu_ms {
                if let Some((l, v)) = last
                    && l != life
                {
                    total += v;
                }
                last = Some((life, ms));
            }
        }
        total + last.map_or(0, |(_, v)| v)
    };
    #[allow(clippy::cast_precision_loss, reason = "key counts are far below 2^52")]
    let kps = if r.typing.is_zero() {
        0.0
    } else {
        r.keys as f64 / r.typing.as_secs_f64()
    };
    let report = Report {
        schema: REPORT_SCHEMA,
        started_unix_ms,
        os: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        git_sha: std::env::var("GITHUB_SHA").ok(),
        options: ReportOptions {
            duration_s: opts.duration.as_secs(),
            seed: opts.seed,
            library: opts.library,
            keys_per_second: opts.keys_per_second,
            sample_every_s: opts.sample_every.as_secs(),
            edit_every_s: opts.edit_every.as_secs(),
            kill_times_s: kills.iter().map(Duration::as_secs).collect(),
            canary_rounds: opts.canary_rounds,
            window_s: rules::window(opts.duration, opts.sample_every, opts.edit_every).as_secs(),
        },
        ran_ms,
        totals: Totals {
            keys: r.keys,
            keys_per_second: (kps * 100.0).round() / 100.0,
            expansions_ok: r.ok,
            expansions_failed: r.failed,
            edits: r.edits,
            samples: u64::try_from(r.samples.len()).unwrap_or(0),
            supervisor_logged_restarts: logged_restarts,
            worker_cpu_ms: cpu(worker_of, true),
            supervisor_cpu_ms: cpu(supervisor_of, false),
            worker_max_memory_kb: max_mem(worker_of),
            supervisor_max_memory_kb: max_mem(supervisor_of),
        },
        recoveries: r.recoveries.clone(),
        unexpected: r.unexpected.clone(),
        expansion_failures: r.failures.clone(),
        canaries: r.typed.clone(),
        net_check,
        rules: results,
        passed,
    };
    write_json(&r.paths.out.join("report.json"), &report)?;
    let summary = summary(&report, &scan);
    fs::write(r.paths.out.join("summary.txt"), &summary)
        .map_err(io_err("cannot write", &r.paths.out.join("summary.txt")))?;
    Ok(report)
}

fn canary_rules(
    canaries: &[Canary],
    typed: &[CanaryTyped],
    scan: &ScanReport,
    found_in_input: usize,
) -> Vec<RuleResult> {
    let mut out = Vec::new();
    let mut files: Vec<String> = scan
        .hits
        .iter()
        .map(|h| format!("{}/{}", h.location, h.file))
        .collect();
    files.sort();
    files.dedup();
    out.push(RuleResult {
        rule: "canary-hits".to_owned(),
        verdict: if scan.hits.is_empty() && scan.unreadable.is_empty() {
            Verdict::Pass
        } else {
            Verdict::Fail
        },
        value: if scan.unreadable.is_empty() {
            format!(
                "{} hits in {} files ({} files, {} bytes scanned)",
                scan.hits.len(),
                files.len(),
                scan.locations.iter().map(|l| l.files).sum::<u64>(),
                scan.locations.iter().map(|l| l.bytes).sum::<u64>()
            )
        } else {
            format!("{} files could not be read", scan.unreadable.len())
        },
        limit: "0 outside the driver's input".to_owned(),
    });
    let all_typed = canaries.iter().all(|c| {
        typed
            .iter()
            .any(|t| t.number == c.number && t.typed_as_expected && t.suspension_as_expected)
    });
    out.push(RuleResult {
        rule: "canaries-typed".to_owned(),
        verdict: if all_typed && !canaries.is_empty() {
            Verdict::Pass
        } else {
            Verdict::Fail
        },
        value: format!(
            "{} of {} typed and checked in their context",
            typed
                .iter()
                .filter(|t| t.typed_as_expected && t.suspension_as_expected)
                .count(),
            canaries.len()
        ),
        limit: "all".to_owned(),
    });
    out.push(RuleResult {
        rule: "canary-scanner-control".to_owned(),
        verdict: if found_in_input == canaries.len() {
            Verdict::Pass
        } else {
            Verdict::Fail
        },
        value: format!(
            "{found_in_input} of {} found in the driver's input",
            canaries.len()
        ),
        limit: "all".to_owned(),
    });
    out
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(io_err("cannot write", path))
}

/// The plain-text verdicts.
pub fn summary(report: &Report, scan: &ScanReport) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "iamb soak: {} ({} of {} s, seed {}, {} snippets, {})",
        if report.passed { "PASS" } else { "FAIL" },
        report.ran_ms / 1000,
        report.options.duration_s,
        report.options.seed,
        report.options.library,
        report.os
    );
    let _ = writeln!(
        s,
        "keys {} ({:.2}/s while typing), expansions {}/{}, edits {}, kills {}, samples {}",
        report.totals.keys,
        report.totals.keys_per_second,
        report.totals.expansions_ok,
        report.totals.expansions_ok + report.totals.expansions_failed,
        report.totals.edits,
        report.recoveries.len(),
        report.totals.samples
    );
    if !report.recoveries.is_empty() {
        let times: Vec<String> = report
            .recoveries
            .iter()
            .map(|r| format!("{}", r.probe_ms))
            .collect();
        let _ = writeln!(s, "recovery after each kill (ms): {}", times.join(", "));
    }
    let _ = writeln!(
        s,
        "canary scan: {} canaries x {} patterns, {} hits",
        scan.canaries,
        scan.patterns,
        scan.hits.len()
    );
    for l in &scan.locations {
        let _ = writeln!(
            s,
            "  {:<16} {:>6} files {:>12} bytes{}",
            l.label,
            l.files,
            l.bytes,
            if l.exists { "" } else { "  (absent)" }
        );
    }
    for h in &scan.hits {
        let _ = writeln!(
            s,
            "  HIT canary #{} ({}) as {} in {}/{} at byte {}",
            h.canary,
            h.context.as_str(),
            h.form,
            h.location,
            h.file,
            h.offset
        );
    }
    for rule in &report.rules {
        let _ = writeln!(
            s,
            "{:<8} {:<40} {} (limit {})",
            match rule.verdict {
                Verdict::Pass => "pass",
                Verdict::Fail => "FAIL",
                Verdict::Skipped => "skipped",
            },
            rule.rule,
            rule.value,
            rule.limit
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kills_avoid_the_rule_windows_and_scale() {
        let m = Duration::from_mins(1);
        let lab = kill_times(Duration::from_hours(72), m, m, Kills::Auto);
        assert_eq!(lab.len(), 12);
        assert_eq!(lab[0], Duration::from_mins(64));
        assert_eq!(
            lab[11],
            Duration::from_hours(72).saturating_sub(Duration::from_mins(64))
        );
        let ci = kill_times(Duration::from_mins(30), m, m, Kills::Auto);
        assert_eq!(ci.len(), 7);
        assert_eq!(ci.first(), Some(&Duration::from_secs(690)));
        assert_eq!(ci.last(), Some(&Duration::from_secs(1110)));
        assert!(
            ci.windows(2)
                .all(|p| p[1].saturating_sub(p[0]) >= MIN_KILL_GAP)
        );
        let ci_every = kill_times(
            Duration::from_mins(30),
            m,
            m,
            Kills::Every(Duration::from_mins(3)),
        );
        assert_eq!(
            ci_every,
            [
                Duration::from_mins(12),
                Duration::from_mins(15),
                Duration::from_mins(18)
            ]
        );
        let (s1, s2) = (Duration::from_secs(1), Duration::from_secs(2));
        let smoke = kill_times(Duration::from_secs(12), s1, s2, Kills::Auto);
        assert_eq!(smoke, [Duration::from_secs(6)]);
        assert!(kill_times(Duration::from_mins(30), m, m, Kills::Off).is_empty());
        assert!(kill_times(Duration::from_mins(30), m, m, Kills::Every(Duration::ZERO)).is_empty());
    }

    #[test]
    fn canary_rounds_are_spread() {
        let t = canary_times(Duration::from_mins(10), 3);
        assert_eq!(
            t,
            [
                Duration::from_secs(100),
                Duration::from_mins(5),
                Duration::from_secs(500)
            ]
        );
        assert!(canary_times(Duration::from_mins(10), 0).is_empty());
    }
}
