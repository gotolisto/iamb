//! The probe window (feature `probe`) against the real system, with a real main loop: it
//! opens, becomes the key window, receives a short abbreviation-free string as synthetic key
//! events posted to its own queue, stamps the last key (T0) and the first change after which
//! the view holds the string (T1). A test with its own `main`: the window needs the main
//! thread. Runs with `cargo test -p iamb-platform-macos --features probe --test probe`.
//!
//! A check that needs what the session does not have (a window that can become key) writes
//! `SKIPPED` and why; a failed one writes `FAILED` and the process exits with a failure.

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    mac::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod mac {
    use std::io::Write;
    use std::process::ExitCode;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use iamb_platform_macos::probe::{Event, ProbeWindow, Sink};
    use iamb_platform_macos::run_with_main_loop;

    const PASSED: i32 = 42;
    /// What the test types: no abbreviation of any library.
    const TYPED: &str = "probe 42";

    #[derive(Debug, Clone)]
    enum Seen {
        Key(Instant, char),
        Changed(Instant, String),
    }

    fn say(line: &str) {
        let _ = writeln!(std::io::stderr(), "probe: {line}");
    }

    fn lock(log: &Mutex<Vec<Seen>>) -> std::sync::MutexGuard<'_, Vec<Seen>> {
        log.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// T0: the key that completed `TYPED`; T1: the first change after it whose text is it.
    fn stamps(log: &[Seen]) -> Option<(Instant, Instant)> {
        let mut typed = String::new();
        let mut t0 = None;
        for item in log {
            match item {
                Seen::Key(at, ch) if t0.is_none() => {
                    typed.push(*ch);
                    if typed.ends_with(TYPED) {
                        t0 = Some(*at);
                    }
                }
                Seen::Changed(at, text) => {
                    if let Some(t0) = t0
                        && text == TYPED
                    {
                        return Some((t0, *at));
                    }
                }
                Seen::Key(..) => {}
            }
        }
        None
    }

    fn checks() -> i32 {
        let log = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&log);
        let sink: Sink = Arc::new(move |event: Event<'_>| {
            let item = match event {
                Event::Key { at, ch } => Seen::Key(at, ch),
                Event::Changed { at, text } => Seen::Changed(at, text.to_owned()),
            };
            lock(&into).push(item);
        });
        let window = match ProbeWindow::open("Iamb probe test", sink) {
            Ok(w) => w,
            Err(e) => {
                say(&format!("FAILED: open: {e}"));
                return 1;
            }
        };
        say("ok: the probe window opened");
        match window.reset() {
            Ok(true) => {}
            Ok(false) => {
                say(
                    "SKIPPED typing: the window could not become the key window (no interactive session)",
                );
                return PASSED;
            }
            Err(e) => {
                say(&format!("FAILED: reset: {e}"));
                return 1;
            }
        }
        lock(&log).clear();
        if let Err(e) = window.type_synthetic(TYPED) {
            say(&format!("FAILED: typing: {e}"));
            return 1;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let found = loop {
            let found = stamps(&lock(&log));
            if found.is_some() || Instant::now() > deadline {
                break found;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let keys = lock(&log)
            .iter()
            .filter(|s| matches!(s, Seen::Key(..)))
            .count();
        let Some((t0, t1)) = found else {
            say(&format!(
                "FAILED: no T0/T1: {keys} key event(s), the text never equalled the typed string"
            ));
            return 1;
        };
        let mut failed = false;
        if keys != TYPED.chars().count() {
            say(&format!(
                "FAILED: {keys} key events for {} characters",
                TYPED.chars().count()
            ));
            failed = true;
        }
        if t1 < t0 || t1 - t0 >= Duration::from_secs(2) {
            say("FAILED: T1 is not within 2 s after T0");
            failed = true;
        }
        if failed {
            return 1;
        }
        say(&format!(
            "ok: T0 and T1 recorded, the text matched, T1 - T0 = {} µs",
            (t1 - t0).as_micros()
        ));
        PASSED
    }

    pub(super) fn main() -> ExitCode {
        if run_with_main_loop(checks) == PASSED {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}
