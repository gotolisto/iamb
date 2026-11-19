//! The probe process: the shared [`Probe`] state the surface's events go into, the
//! [`Surface`] the trials are typed into, and the [`serve`] loop that answers the driver.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::protocol::{Command, Reply, encode, parse_command};
use super::session::{Sample, Session, Trial};

/// How often a waiting command looks at the deadlines.
const TICK: Duration = Duration::from_millis(10);

/// What a trial is typed into: the native window or the browser page.
pub trait Surface: Send + Sync {
    /// `native` or `browser`.
    fn kind(&self) -> &'static str;
    /// The page to open, for the browser.
    fn url(&self) -> Option<String> {
        None
    }
    /// Waits until the surface can be typed into; false after `timeout`.
    fn wait_ready(&self, timeout: Duration) -> bool {
        let _ = timeout;
        true
    }
    /// Empties the control and gives it the focus; `Ok(false)` when it did not get the focus.
    fn reset(&self) -> Result<bool, String>;
}

/// The probe's state, shared by the surface (which reports events) and the control loop.
#[derive(Debug)]
pub struct Probe {
    epoch: Instant,
    session: Mutex<Session>,
    changed: Condvar,
}

impl Probe {
    /// A probe whose surface delivers events within `slack` of when they happened.
    pub fn new(slack: Duration) -> Self {
        Self {
            epoch: Instant::now(),
            session: Mutex::new(Session::new(slack)),
            changed: Condvar::new(),
        }
    }

    /// `at` as nanoseconds since the probe started: the native window's stamps.
    pub fn ns(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.epoch).as_nanos()).unwrap_or(u64::MAX)
    }

    /// Now, as [`ns`](Self::ns).
    pub fn now_ns(&self) -> u64 {
        self.ns(Instant::now())
    }

    fn lock(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A key that produced `ch`, stamped `stamp` on the surface's clock.
    pub fn key(&self, stamp: u64, ch: char) {
        let now = self.now_ns();
        self.lock().key(stamp, now, ch);
        self.changed.notify_all();
    }

    /// A key at `at` on this process's clock (the native window).
    pub fn key_at(&self, at: Instant, ch: char) {
        self.key(self.ns(at), ch);
    }

    /// The control's text changed, stamped `stamp` on the surface's clock.
    pub fn text(&self, stamp: u64, text: &str) {
        let now = self.now_ns();
        self.lock().changed(stamp, now, text);
        self.changed.notify_all();
    }

    /// A change at `at` on this process's clock (the native window).
    pub fn text_at(&self, at: Instant, text: &str) {
        self.text(self.ns(at), text);
    }

    /// Arms `trial`.
    pub fn arm(&self, trial: Trial, key_wait: Duration) -> Result<(), String> {
        let now = self.now_ns();
        self.lock().arm(trial, now, key_wait)
    }

    /// Cancels any armed trial.
    pub fn cancel(&self) {
        self.lock().cancel();
    }

    /// Waits for the armed trial to finish; after a match, watches the text for `settle` more.
    pub fn wait(&self, settle: Duration) -> Result<Sample, String> {
        let mut session = self.lock();
        let sample = loop {
            let now = self.now_ns();
            session.expire(now);
            if let Some(sample) = session.take_finished() {
                break sample;
            }
            if !session.busy() {
                return Err("no trial is armed".to_owned());
            }
            session = self
                .changed
                .wait_timeout(session, TICK)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        };
        drop(session);
        if !sample.pass || settle.is_zero() {
            return Ok(sample);
        }
        std::thread::sleep(settle);
        let mut session = self.lock();
        let stable = session.latest_matches().unwrap_or(false);
        Ok(session.mark_stable(stable).unwrap_or(sample))
    }

    /// Every trial so far.
    pub fn report(&self) -> super::session::Report {
        self.lock().report()
    }
}

/// How the loop behaves.
#[derive(Debug, Clone, Default)]
pub struct ServeOptions {
    /// Also write the recorder's sorted samples here (one integer per line, the `.samples`
    /// format of `iamb-bench`) at each `report`.
    pub samples_file: Option<PathBuf>,
}

/// The probe's answer to one command (what [`serve`] does for each line).
pub fn answer(
    probe: &Probe,
    surface: &dyn Surface,
    command: Command,
    opts: &ServeOptions,
) -> Reply {
    let error = |message: String| Reply::Error { message };
    match command {
        Command::Hello => Reply::Hello {
            pid: std::process::id(),
            surface: surface.kind().to_owned(),
            url: surface.url(),
        },
        Command::WaitReady { timeout_ms } => {
            if surface.wait_ready(Duration::from_millis(timeout_ms)) {
                Reply::Ok { focused: None }
            } else {
                error(format!(
                    "the {} surface was not ready within {timeout_ms} ms",
                    surface.kind()
                ))
            }
        }
        Command::Reset => {
            probe.cancel();
            match surface.reset() {
                Ok(focused) => Reply::Ok {
                    focused: Some(focused),
                },
                Err(e) => error(e),
            }
        }
        Command::Expect {
            cell,
            typed,
            expected,
            key_wait_ms,
        } => match probe.arm(
            Trial {
                cell,
                typed,
                expected,
            },
            Duration::from_millis(key_wait_ms),
        ) {
            Ok(()) => Reply::Ok { focused: None },
            Err(e) => error(e),
        },
        Command::Wait { settle_ms } => match probe.wait(Duration::from_millis(settle_ms)) {
            Ok(sample) => Reply::Sample { sample },
            Err(e) => error(e),
        },
        Command::Report => {
            let report = probe.report();
            if let Some(path) = &opts.samples_file {
                let mut text = String::new();
                for ns in &report.sorted_ns {
                    text.push_str(&ns.to_string());
                    text.push('\n');
                }
                if let Err(e) = std::fs::write(path, text) {
                    return error(format!("cannot write {}: {e}", path.display()));
                }
            }
            Reply::Report { report }
        }
        Command::Quit => Reply::Ok { focused: None },
    }
}

/// Answers the driver's commands from `input` on `output` until `quit` or the end of `input`.
pub fn serve(
    probe: &Probe,
    surface: &dyn Surface,
    input: impl BufRead,
    mut output: impl Write,
    opts: &ServeOptions,
) -> Result<(), String> {
    for line in input.lines() {
        let line = line.map_err(|e| format!("cannot read a command: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let (reply, quit) = match parse_command(&line) {
            Ok(command) => {
                let quit = command == Command::Quit;
                (answer(probe, surface, command, opts), quit)
            }
            Err(message) => (Reply::Error { message }, false),
        };
        writeln!(output, "{}", encode(&reply))
            .and_then(|()| output.flush())
            .map_err(|e| format!("cannot answer: {e}"))?;
        if quit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::compat::probe::protocol::parse_reply;
    use crate::compat::probe::session::{Failure, TIMEOUT_NS};

    struct Fake {
        resets: AtomicU32,
    }

    impl Surface for Fake {
        fn kind(&self) -> &'static str {
            "native"
        }
        fn reset(&self) -> Result<bool, String> {
            self.resets.fetch_add(1, Ordering::Relaxed);
            Ok(true)
        }
    }

    fn run(probe: &Probe, lines: &[&str], opts: &ServeOptions) -> Vec<Reply> {
        let fake = Fake {
            resets: AtomicU32::new(0),
        };
        let input = lines.join("\n");
        let mut out = Vec::new();
        serve(probe, &fake, input.as_bytes(), &mut out, opts).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| parse_reply(l).unwrap())
            .collect()
    }

    #[test]
    fn the_loop_answers_each_command_and_stops_at_quit() {
        let probe = Probe::new(Duration::ZERO);
        let replies = run(
            &probe,
            &[
                r#"{"op":"hello"}"#,
                r#"{"op":"reset"}"#,
                "garbage",
                r#"{"op":"wait"}"#,
                r#"{"op":"quit"}"#,
                r#"{"op":"hello"}"#,
            ],
            &ServeOptions::default(),
        );
        assert_eq!(replies.len(), 5, "nothing is answered after quit");
        assert!(
            matches!(&replies[0], Reply::Hello { surface, url: None, .. } if surface == "native")
        );
        assert_eq!(
            replies[1],
            Reply::Ok {
                focused: Some(true)
            }
        );
        assert!(matches!(&replies[2], Reply::Error { .. }));
        assert!(matches!(&replies[3], Reply::Error { message } if message.contains("no trial")));
        assert_eq!(replies[4], Reply::Ok { focused: None });
    }

    #[test]
    fn a_trial_from_another_thread_is_timed_and_reported() {
        let probe = Arc::new(Probe::new(Duration::ZERO));
        let dir = tempfile::tempdir().unwrap();
        let samples = dir.path().join("probe.samples.txt");
        let opts = ServeOptions {
            samples_file: Some(samples.clone()),
        };
        // The "surface": types the abbreviation once armed, then the expander replaces it.
        let typist = {
            let probe = Arc::clone(&probe);
            std::thread::spawn(move || {
                while !probe.lock().busy() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                for ch in ";lab".chars() {
                    probe.key_at(Instant::now(), ch);
                }
                probe.text_at(Instant::now(), ";lab");
                std::thread::sleep(Duration::from_millis(5));
                probe.text_at(Instant::now(), "");
                probe.text_at(Instant::now(), "one\r");
                probe.text_at(Instant::now(), "one\rtwo");
            })
        };
        let replies = run(
            &probe,
            &[
                r#"{"op":"expect","cell":"L0007","typed":";lab","expected":"one\ntwo"}"#,
                r#"{"op":"wait","settle_ms":20}"#,
                r#"{"op":"report"}"#,
            ],
            &opts,
        );
        typist.join().unwrap();
        let Reply::Sample { sample } = &replies[1] else {
            panic!("{:?}", replies[1]);
        };
        assert!(sample.pass);
        assert_eq!(sample.cell, "L0007");
        assert_eq!(sample.stable, Some(true));
        assert!(sample.latency_ns >= 5_000_000, "{}", sample.latency_ns);
        assert!(sample.latency_ns < TIMEOUT_NS);
        let Reply::Report { report } = &replies[2] else {
            panic!("{:?}", replies[2]);
        };
        assert_eq!(report.sorted_ns, vec![sample.latency_ns]);
        let written = std::fs::read_to_string(samples).unwrap();
        assert_eq!(written, format!("{}\n", sample.latency_ns));
    }

    #[test]
    fn a_trial_that_never_expands_waits_two_seconds() {
        let probe = Probe::new(Duration::ZERO);
        probe
            .arm(
                Trial {
                    cell: "c".to_owned(),
                    typed: "k".to_owned(),
                    expected: "never".to_owned(),
                },
                Duration::from_secs(10),
            )
            .unwrap();
        probe.key_at(Instant::now(), 'k');
        let started = Instant::now();
        let sample = probe.wait(Duration::from_millis(50)).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(1900));
        assert_eq!(sample.failure, Some(Failure::Timeout));
        assert_eq!(sample.latency_ns, TIMEOUT_NS);
        assert_eq!(sample.stable, None);
    }
}
