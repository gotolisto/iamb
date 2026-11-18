//! The pass rules of `docs/test-strategy.md` §5.2, scaled to the run length.
//!
//! The lab run lasts 72 h and its rules speak of "hour 1" and "the last hour". A run of length
//! `D` uses a window `W` in their place: `D / 72`, but at least five sample intervals and eight
//! edit intervals, and at most `D / 4`. That is 1 h for 72 h and 7.5 min for the CI run of
//! 30 min with 60 s samples and edits. The edit floor exists because the worker's memory climbs
//! over its first library edits (each edit builds a new snapshot while the old one is still in
//! use, and the allocator keeps the high-water mark). Measured on the mock with 10,000 snippets
//! (release build, Linux): 37 MB private before the first edit, +12 MB at the first, about +4 MB
//! at each of the next three, then flat at 57–60 MB; a window of two edits reports that climb as
//! a leak of 20 MB.
//!
//! Both leak rules treat the first `W` of every process life as warm-up, as §5.2 does for rule 1:
//! rule 2 compares the last `W` with the first `W` after the warm-up (`[W, 2W]`; hour 2 in the
//! lab) rather than with the warm-up itself, which would charge the climb above to the budget.
//! The driver keeps its kills out of the first and last `W` (plus four samples), so both windows
//! hold settled samples of one worker life.
//!
//! | Rule | Check |
//! | --- | --- |
//! | `no-unexpected-exit` | The supervisor never exited and the worker never restarted, apart from the injected kills |
//! | `restart-under-2s` | Every injected kill recovered (control endpoint, IPC `Health`, a probe expansion) in under 2 s |
//! | `leak-slope` | Per process: the least-squares slope of memory over time, pooled within each worker life (a kill resets the worker's memory), from `W` after the start of each life, times `D − W`, is under 5 MB |
//! | `leak-window` | Per process: mean memory of the last `W` minus mean of `[W, 2W]` is under 5 MB (samples within `W` after a restart left out) |
//! | `handles-threads-flat` | Per process: handle and thread counts stay within the range of the first `W`, give or take `max(2, 10%)`, except within `W` after a restart |
//! | `expansion-correctness` | At least 99.9% of the checked expansions left exactly the expected text |
//!
//! Memory is private memory where the OS gives it, else resident memory ([`ProcessSample::memory_kb`]).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::probe::ProcessSample;

/// The memory growth limit of both leak rules, KiB.
pub const LEAK_LIMIT_KB: f64 = 5.0 * 1024.0;

/// The restart limit.
pub const RECOVERY_LIMIT: Duration = Duration::from_secs(2);

/// The correctness target.
pub const CORRECTNESS: f64 = 0.999;

/// One line of the samples file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    /// The schema of the line.
    pub schema: String,
    /// Milliseconds since the run's start.
    pub t_ms: u64,
    /// Wall clock, ms since the Unix epoch.
    pub unix_ms: u64,
    /// The supervisor.
    pub supervisor: ProcessSample,
    /// The current worker.
    pub worker: ProcessSample,
    /// Which worker life (1 for the first worker, +1 per restart).
    pub worker_life: u32,
    /// When the current worker life began, ms since the run's start.
    pub life_started_ms: u64,
    /// Expansions that left the expected text, so far.
    pub expansions_ok: u64,
    /// Expansions that did not, so far.
    pub expansions_failed: u64,
    /// Keys typed so far.
    pub keys: u64,
    /// Library edits made so far.
    pub edits: u64,
    /// Injected kills so far.
    pub kills: u64,
}

/// One recovery from an injected kill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recovery {
    /// When the kill was sent, ms since the run's start.
    pub at_ms: u64,
    /// The old and new worker process ids.
    pub old_pid: u32,
    /// The new worker.
    pub new_pid: u32,
    /// Kill → control endpoint answers, ms.
    pub control_ms: u64,
    /// Kill → IPC `Health` answers, ms.
    pub health_ms: u64,
    /// Kill → the probe expansion left the expected text, ms (the recovery time).
    pub probe_ms: u64,
    /// Whether it recovered at all (within the give-up time).
    pub recovered: bool,
}

/// A rule's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Holds.
    Pass,
    /// Broken.
    Fail,
    /// Could not be evaluated (too few samples, not measured on this OS, turned off).
    Skipped,
}

/// One rule's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleResult {
    /// The rule (module docs), with the process in brackets where it applies.
    pub rule: String,
    /// Holds or not.
    pub verdict: Verdict,
    /// The measured value, in words.
    pub value: String,
    /// The limit, in words.
    pub limit: String,
}

impl RuleResult {
    fn new(rule: impl Into<String>, verdict: Verdict, value: String, limit: &str) -> Self {
        Self {
            rule: rule.into(),
            verdict,
            value,
            limit: limit.to_owned(),
        }
    }
}

/// Everything the rules look at.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    /// The planned run length.
    pub duration: Duration,
    /// The sample interval.
    pub sample_every: Duration,
    /// The library edit interval.
    pub edit_every: Duration,
    /// The samples, in time order.
    pub samples: Vec<Sample>,
    /// The injected kills.
    pub recoveries: Vec<Recovery>,
    /// Whether kills were turned off.
    pub kills_disabled: bool,
    /// Unexpected exits seen: (what, when ms), e.g. a supervisor exit or a worker restart that
    /// no kill explains.
    pub unexpected: Vec<String>,
    /// Expansions checked and correct.
    pub expansions_ok: u64,
    /// Expansions checked and wrong.
    pub expansions_failed: u64,
}

/// The window `W` (module docs).
pub fn window(duration: Duration, sample_every: Duration, edit_every: Duration) -> Duration {
    let w = (duration / 72).max(sample_every * 5).max(edit_every * 8);
    w.min(duration / 4)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Process {
    Supervisor,
    Worker,
}

impl Process {
    const fn name(self) -> &'static str {
        match self {
            Self::Supervisor => "supervisor",
            Self::Worker => "worker",
        }
    }

    fn of(self, s: &Sample) -> &ProcessSample {
        match self {
            Self::Supervisor => &s.supervisor,
            Self::Worker => &s.worker,
        }
    }

    /// The life and its start for `s`: the supervisor has one life.
    const fn life(self, s: &Sample) -> (u32, u64) {
        match self {
            Self::Supervisor => (1, 0),
            Self::Worker => (s.worker_life, s.life_started_ms),
        }
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "sample times and sizes are far below 2^52"
)]
fn f(n: u64) -> f64 {
    n as f64
}

/// Pooled within-life least-squares slope, KiB per ms, of `(life, t, y)` points.
fn pooled_slope(points: &[(u32, f64, f64)]) -> Option<f64> {
    let mut lives: Vec<u32> = points.iter().map(|p| p.0).collect();
    lives.sort_unstable();
    lives.dedup();
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for life in lives {
        let group: Vec<&(u32, f64, f64)> = points.iter().filter(|p| p.0 == life).collect();
        if group.len() < 2 {
            continue;
        }
        let n = f(u64::try_from(group.len()).unwrap_or(1));
        let mx = group.iter().map(|p| p.1).sum::<f64>() / n;
        let my = group.iter().map(|p| p.2).sum::<f64>() / n;
        for p in group {
            sxy += (p.1 - mx) * (p.2 - my);
            sxx += (p.1 - mx) * (p.1 - mx);
        }
    }
    (sxx > 0.0).then(|| sxy / sxx)
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty())
        .then(|| values.iter().sum::<f64>() / f(u64::try_from(values.len()).unwrap_or(1)))
}

/// Applies every rule of the module docs.
#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "one block per rule; d and w are the run length and window of the module docs"
)]
pub fn evaluate(e: &Evidence) -> Vec<RuleResult> {
    let mut out = Vec::new();
    let d = super::millis(e.duration);
    let w = super::millis(window(e.duration, e.sample_every, e.edit_every));

    out.push(if e.unexpected.is_empty() {
        RuleResult::new("no-unexpected-exit", Verdict::Pass, "0".to_owned(), "0")
    } else {
        RuleResult::new(
            "no-unexpected-exit",
            Verdict::Fail,
            format!("{}: {}", e.unexpected.len(), e.unexpected.join("; ")),
            "0",
        )
    });

    let limit_ms = super::millis(RECOVERY_LIMIT);
    out.push(if e.recoveries.is_empty() {
        RuleResult::new(
            "restart-under-2s",
            if e.kills_disabled {
                Verdict::Skipped
            } else {
                Verdict::Fail
            },
            "no kill injected".to_owned(),
            "every kill < 2000 ms",
        )
    } else {
        let max = e.recoveries.iter().map(|r| r.probe_ms).max().unwrap_or(0);
        let all = e
            .recoveries
            .iter()
            .all(|r| r.recovered && r.probe_ms < limit_ms);
        RuleResult::new(
            "restart-under-2s",
            if all { Verdict::Pass } else { Verdict::Fail },
            format!("{} kills, max {max} ms", e.recoveries.len()),
            "every kill < 2000 ms",
        )
    });

    for process in [Process::Supervisor, Process::Worker] {
        let name = process.name();
        // Samples outside the warm-up of their life.
        let settled = |s: &Sample| {
            let (life, start) = process.life(s);
            s.t_ms >= w && (life <= 1 || s.t_ms >= start + w)
        };
        let points: Vec<(u32, f64, f64)> = e
            .samples
            .iter()
            .filter(|s| settled(s))
            .filter_map(|s| {
                let m = process.of(s).memory_kb()?;
                Some((process.life(s).0, f(s.t_ms), f(m)))
            })
            .collect();
        out.push(match pooled_slope(&points) {
            Some(slope) => {
                let growth = slope * f(d.saturating_sub(w));
                RuleResult::new(
                    format!("leak-slope [{name}]"),
                    if growth < LEAK_LIMIT_KB {
                        Verdict::Pass
                    } else {
                        Verdict::Fail
                    },
                    format!(
                        "{:.1} KiB over {} s ({} samples)",
                        growth,
                        d.saturating_sub(w) / 1000,
                        points.len()
                    ),
                    "< 5120 KiB",
                )
            }
            None => RuleResult::new(
                format!("leak-slope [{name}]"),
                Verdict::Skipped,
                format!("{} usable samples", points.len()),
                "< 5120 KiB",
            ),
        });

        // Leak rule 2: the first window after warm-up vs the last window.
        let in_restart_warmup = |s: &Sample| {
            let (life, start) = process.life(s);
            life > 1 && s.t_ms < start + w
        };
        let first: Vec<f64> = e
            .samples
            .iter()
            .filter(|s| s.t_ms >= w && s.t_ms <= 2 * w && !in_restart_warmup(s))
            .filter_map(|s| process.of(s).memory_kb().map(f))
            .collect();
        let last_from = e.samples.last().map_or(0, |s| s.t_ms.saturating_sub(w));
        let last: Vec<f64> = e
            .samples
            .iter()
            .filter(|s| s.t_ms >= last_from && !in_restart_warmup(s))
            .filter_map(|s| process.of(s).memory_kb().map(f))
            .collect();
        out.push(match (mean(&first), mean(&last)) {
            (Some(a), Some(b)) if e.samples.len() >= 3 => RuleResult::new(
                format!("leak-window [{name}]"),
                if b - a < LEAK_LIMIT_KB {
                    Verdict::Pass
                } else {
                    Verdict::Fail
                },
                format!(
                    "{:.1} KiB ({} first, {} last samples)",
                    b - a,
                    first.len(),
                    last.len()
                ),
                "< 5120 KiB",
            ),
            _ => RuleResult::new(
                format!("leak-window [{name}]"),
                Verdict::Skipped,
                "too few samples".to_owned(),
                "< 5120 KiB",
            ),
        });

        for (what, get) in [
            (
                "handles",
                (|p: &ProcessSample| p.handles) as fn(&ProcessSample) -> Option<u64>,
            ),
            ("threads", |p: &ProcessSample| p.threads),
        ] {
            let rule = format!("handles-threads-flat [{name} {what}]");
            let base: Vec<u64> = e
                .samples
                .iter()
                .filter(|s| s.t_ms <= w && !in_restart_warmup(s))
                .filter_map(|s| get(process.of(s)))
                .collect();
            let (Some(&lo), Some(&hi)) = (base.iter().min(), base.iter().max()) else {
                out.push(RuleResult::new(
                    rule,
                    Verdict::Skipped,
                    "not measured".to_owned(),
                    "-",
                ));
                continue;
            };
            let tol = (hi / 10).max(2);
            let (min_ok, max_ok) = (lo.saturating_sub(tol), hi + tol);
            let outside: Vec<String> = e
                .samples
                .iter()
                .filter(|s| !in_restart_warmup(s))
                .filter_map(|s| get(process.of(s)).map(|v| (s.t_ms, v)))
                .filter(|&(_, v)| v < min_ok || v > max_ok)
                .map(|(t, v)| format!("{v} at {} s", t / 1000))
                .collect();
            let seen_max = e
                .samples
                .iter()
                .filter_map(|s| get(process.of(s)))
                .max()
                .unwrap_or(0);
            out.push(RuleResult::new(
                rule,
                if outside.is_empty() {
                    Verdict::Pass
                } else {
                    Verdict::Fail
                },
                if outside.is_empty() {
                    format!("first window {lo}..{hi}, max seen {seen_max}")
                } else {
                    format!("first window {lo}..{hi}; outside: {}", outside.join(", "))
                },
                &format!("{min_ok}..{max_ok}"),
            ));
        }
    }

    let total = e.expansions_ok + e.expansions_failed;
    out.push(if total == 0 {
        RuleResult::new(
            "expansion-correctness",
            Verdict::Fail,
            "no expansion checked".to_owned(),
            ">= 99.9%",
        )
    } else {
        let rate = f(e.expansions_ok) / f(total);
        RuleResult::new(
            "expansion-correctness",
            if rate >= CORRECTNESS {
                Verdict::Pass
            } else {
                Verdict::Fail
            },
            format!("{} of {total} ({:.3}%)", e.expansions_ok, rate * 100.0),
            ">= 99.9%",
        )
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t_s: u64, life: u32, start_s: u64, sup_kb: u64, wrk_kb: u64) -> Sample {
        let p = |kb| ProcessSample {
            pid: 1,
            alive: true,
            rss_kb: Some(kb),
            private_kb: Some(kb),
            cpu_ms: Some(0),
            handles: Some(10),
            threads: Some(8),
        };
        Sample {
            t_ms: t_s * 1000,
            supervisor: p(sup_kb),
            worker: p(wrk_kb),
            worker_life: life,
            life_started_ms: start_s * 1000,
            ..Sample::default()
        }
    }

    fn evidence(samples: Vec<Sample>) -> Evidence {
        Evidence {
            duration: Duration::from_mins(30),
            sample_every: Duration::from_mins(1),
            edit_every: Duration::from_mins(1),
            samples,
            recoveries: vec![Recovery {
                at_ms: 1,
                old_pid: 1,
                new_pid: 2,
                control_ms: 5,
                health_ms: 6,
                probe_ms: 20,
                recovered: true,
            }],
            expansions_ok: 1000,
            ..Evidence::default()
        }
    }

    fn verdict(results: &[RuleResult], rule: &str) -> Verdict {
        results
            .iter()
            .find(|r| r.rule == rule)
            .unwrap_or_else(|| panic!("{rule} evaluated"))
            .verdict
    }

    #[test]
    fn the_window_scales() {
        let m = Duration::from_mins(1);
        assert_eq!(
            window(Duration::from_hours(72), m, m),
            Duration::from_hours(1)
        );
        assert_eq!(
            window(Duration::from_mins(30), m, m),
            Duration::from_secs(450)
        );
        assert_eq!(
            window(
                Duration::from_secs(12),
                Duration::from_secs(1),
                Duration::from_secs(2)
            ),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn flat_memory_passes_and_a_steady_leak_fails() {
        let flat: Vec<Sample> = (0..=30).map(|m| sample(m * 60, 1, 0, 2000, 9000)).collect();
        let r = evaluate(&evidence(flat));
        assert!(r.iter().all(|x| x.verdict == Verdict::Pass), "{r:#?}");

        // 400 KiB a minute in the worker: 8.8 MiB over the 22.5 minutes after warm-up, and
        // 5.9 MiB between the windows [7.5, 15] and [22.5, 30] minutes.
        let leaking: Vec<Sample> = (0..=30)
            .map(|m| sample(m * 60, 1, 0, 2000, 9000 + m * 400))
            .collect();
        let r = evaluate(&evidence(leaking));
        assert_eq!(verdict(&r, "leak-slope [worker]"), Verdict::Fail);
        assert_eq!(verdict(&r, "leak-window [worker]"), Verdict::Fail);
        assert_eq!(verdict(&r, "leak-slope [supervisor]"), Verdict::Pass);
    }

    #[test]
    fn a_kill_resetting_memory_neither_hides_a_leak_nor_fakes_one() {
        // A leak of 300 KiB/min that each restart resets: the pooled slope still sees it.
        let mut samples = Vec::new();
        for m in 0..=30_u64 {
            let start = m / 10 * 10;
            let life = u32::try_from(m / 10 + 1).expect("small");
            samples.push(sample(
                m * 60,
                life,
                start * 60,
                2000,
                9000 + (m - start) * 300,
            ));
        }
        let r = evaluate(&evidence(samples));
        assert_eq!(verdict(&r, "leak-slope [worker]"), Verdict::Fail);

        // Flat lives at different levels: no slope.
        let samples: Vec<Sample> = (0..=30)
            .map(|m| {
                let life = u32::try_from(m / 10 + 1).expect("small");
                sample(
                    m * 60,
                    life,
                    m / 10 * 600,
                    2000,
                    9000 + u64::from(life) * 100,
                )
            })
            .collect();
        let r = evaluate(&evidence(samples));
        assert_eq!(verdict(&r, "leak-slope [worker]"), Verdict::Pass);
    }

    #[test]
    fn slow_recovery_threads_and_correctness_fail() {
        let mut samples: Vec<Sample> = (0..=30).map(|m| sample(m * 60, 1, 0, 2000, 9000)).collect();
        samples[20].worker.threads = Some(30);
        let mut e = evidence(samples);
        e.recoveries[0].probe_ms = 2500;
        e.expansions_failed = 2;
        let r = evaluate(&e);
        assert_eq!(verdict(&r, "restart-under-2s"), Verdict::Fail);
        assert_eq!(
            verdict(&r, "handles-threads-flat [worker threads]"),
            Verdict::Fail
        );
        assert_eq!(verdict(&r, "expansion-correctness"), Verdict::Fail);
        e.unexpected.push("supervisor exited".to_owned());
        assert_eq!(verdict(&evaluate(&e), "no-unexpected-exit"), Verdict::Fail);
        e.recoveries.clear();
        assert_eq!(verdict(&evaluate(&e), "restart-under-2s"), Verdict::Fail);
        e.kills_disabled = true;
        assert_eq!(verdict(&evaluate(&e), "restart-under-2s"), Verdict::Skipped);
    }
}
