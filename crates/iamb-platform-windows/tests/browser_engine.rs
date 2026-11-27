//! The secure-field answer for a real browser engine's window: Microsoft Edge (the engine of the
//! system's web view) in an application window. Three pages: a `<textarea>`, which must come out
//! `Normal`; an `<input type="password">`, which must come out `Secure`; and a text field that
//! the page turns into a password field a few seconds after it loads, with no focus change,
//! after which no answer may be `Normal` (the late-answer review's finding 1).
//!
//! The engine switches its accessibility on only when it is first asked and then builds its
//! tree in the background, so the answer is asked again until it settles, through the watcher's
//! own reports again (a sink that judges each report, as the service's focus thread does) and
//! not on a schedule of the test's own.
//!
//! **Not run in CI** (`#[ignore]`): it needs Edge, an interactive desktop that lets the new
//! window take the foreground, and a first start of a browser profile, which takes a few
//! seconds and varies from machine to machine. Run it by hand on Windows:
//!
//! ```text
//! cargo test -p iamb-platform-windows --test browser_engine -- --ignored --test-threads 1
//! ```
//!
//! A run that cannot look (no Edge, no window in front) **fails**: it was asked for. Set
//! `IAMB_ALLOW_SKIP=1` to have it say `SKIPPED` and pass instead. It prints how long the answer
//! took to settle and which unknown reasons came first, which is what "Why didn't it expand?"
//! shows as "Reason: …".

#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use iamb_platform::{FieldSecurity, FocusChange, FocusInspector, SECURE_STATE_TIMEOUT};
use iamb_platform_windows::Platform;

/// How long the window may take to come to the front, and the answer to settle.
const PATIENCE: Duration = Duration::from_secs(30);

/// When the changing page turns its field into a password field, after it loaded.
const TURNS_AFTER: Duration = Duration::from_secs(8);

/// Says why the test could not look: a failure, unless `IAMB_ALLOW_SKIP` is set.
fn cannot_look(why: &str) {
    assert!(
        std::env::var_os("IAMB_ALLOW_SKIP").is_some(),
        "this test was asked for, but {why} (set IAMB_ALLOW_SKIP=1 to skip instead)"
    );
    eprintln!("SKIPPED: {why}");
}

/// Edge's executable, where the installer puts it.
fn edge() -> Option<PathBuf> {
    ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|dir| PathBuf::from(dir).join(r"Microsoft\Edge\Application\msedge.exe"))
        .find(|path| path.is_file())
}

/// A page with one field, focused as soon as it loads, and a script of its own.
fn page(field: &str, script: &str) -> String {
    // A data URL: no file and no server. `%23` is `#`, which would end the URL.
    format!(
        "data:text/html,<!doctype html><title>iamb</title><body>{field}\
         <script>document.querySelector('%23f').focus();{script}</script>"
    )
}

/// Edge in an application window of a profile of its own, closed when dropped.
struct Browser {
    child: Child,
    profile: PathBuf,
}

impl Browser {
    fn open(edge: &PathBuf, url: &str, name: &str) -> Self {
        let profile =
            std::env::temp_dir().join(format!("iamb-engine-{}-{name}", std::process::id()));
        let child = Command::new(edge)
            .arg(format!("--user-data-dir={}", profile.display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-sync",
                "--new-window",
            ])
            .arg(format!("--app={url}"))
            .spawn()
            .expect("Edge starts");
        Self { child, profile }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // The engine's helper processes go with the browser process.
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &self.child.id().to_string()])
            .output();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// One verdict of the sink: when it was given, and what it said.
type Verdict = (Instant, FieldSecurity);

/// Watches the focus and judges every report of an Edge window, as the service's focus thread
/// does (so the watcher's reports again, after a pause or a late answer, are what is measured).
/// Returns the platform (to keep watching) and the verdicts.
fn judging() -> (Arc<Platform>, Receiver<Verdict>) {
    let platform = Arc::new(Platform::new());
    let (verdicts, heard) = mpsc::channel();
    // The sink runs on the watcher's thread and must not block it: it hands each report to a
    // thread that judges it.
    let (reports, queue) = mpsc::channel::<FocusChange>();
    let judge = Arc::clone(&platform);
    std::thread::spawn(move || {
        while let Ok(change) = queue.recv() {
            if change.app.app.0 == "msedge.exe" {
                let state = judge.secure_state(&change.token, SECURE_STATE_TIMEOUT);
                if verdicts.send((Instant::now(), state)).is_err() {
                    return;
                }
            }
        }
    });
    platform
        .watch(Box::new(move |change: FocusChange| {
            let _ = reports.send(change);
        }))
        .expect("the watcher starts");
    (platform, heard)
}

/// The verdicts until one is not unknown (or `PATIENCE` is over), said on the way.
fn settle(heard: &Receiver<Verdict>) -> Option<FieldSecurity> {
    let started = Instant::now();
    let mut reasons = Vec::new();
    while let Some(left) = (started + PATIENCE).checked_duration_since(Instant::now()) {
        let Ok((_, state)) = heard.recv_timeout(left) else {
            break;
        };
        if let FieldSecurity::Unknown(reason) = state {
            if reasons.last() != Some(&reason) {
                reasons.push(reason);
            }
        } else {
            eprintln!(
                "settled as {state:?} after {:?}; unknown before: {reasons:?}",
                started.elapsed()
            );
            return Some(state);
        }
    }
    eprintln!("did not settle; unknown: {reasons:?}");
    None
}

/// Opens `url`, waits for the first verdict about Edge's window, and gives the verdicts.
fn open(url: &str, name: &str) -> Option<(Browser, Arc<Platform>, Receiver<Verdict>, Instant)> {
    let Some(edge) = edge() else {
        cannot_look("Microsoft Edge is not installed");
        return None;
    };
    let (platform, heard) = judging();
    let browser = Browser::open(&edge, url, name);
    let opened = Instant::now();
    let Ok(first) = heard.recv_timeout(PATIENCE) else {
        cannot_look("Edge's window did not come to the front");
        return None;
    };
    // Put the first verdict back in front of the others.
    let (again, rest) = mpsc::channel();
    let _ = again.send(first);
    std::thread::spawn(move || {
        while let Ok(verdict) = heard.recv() {
            if again.send(verdict).is_err() {
                return;
            }
        }
    });
    Some((browser, platform, rest, opened))
}

#[test]
#[ignore = "needs Edge and an interactive desktop; run by hand (module docs)"]
fn a_text_area_of_a_browser_engine_is_normal() {
    if let Some((_browser, platform, heard, _)) =
        open(&page("<textarea id=f></textarea>", ""), "text")
    {
        assert_eq!(settle(&heard), Some(FieldSecurity::Normal));
        let _ = platform.unwatch();
    }
}

#[test]
#[ignore = "needs Edge and an interactive desktop; run by hand (module docs)"]
fn a_password_field_of_a_browser_engine_is_secure() {
    if let Some((_browser, platform, heard, _)) =
        open(&page("<input id=f type=password>", ""), "password")
    {
        assert_eq!(settle(&heard), Some(FieldSecurity::Secure));
        let _ = platform.unwatch();
    }
}

#[test]
#[ignore = "needs Edge and an interactive desktop; run by hand (module docs)"]
fn a_field_that_turns_into_a_password_field_is_never_normal_afterwards() {
    let turn = format!(
        "setTimeout(()=>{{document.querySelector('%23f').type='password'}},{})",
        TURNS_AFTER.as_millis()
    );
    let Some((_browser, platform, heard, opened)) =
        open(&page("<input id=f type=text>", &turn), "turns")
    else {
        return;
    };
    // Before the turn: the text field, if the engine answers in time for that.
    let before = settle(&heard);
    assert!(
        matches!(before, Some(FieldSecurity::Normal) | None),
        "a text field: {before:?}"
    );
    // After the turn the user moves nowhere, so only the page changed. Judge the window as
    // the service would on its next report (the watcher may report nothing: no focus event),
    // and take every verdict the watcher's reports bring.
    let turned = opened + TURNS_AFTER + Duration::from_secs(2);
    std::thread::sleep(turned.saturating_duration_since(Instant::now()));
    let Ok(token) = platform.focus_token() else {
        cannot_look("Edge's window lost the front");
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut secure = false;
    while Instant::now() < deadline {
        let state = platform.secure_state(&token, SECURE_STATE_TIMEOUT);
        assert_ne!(state, FieldSecurity::Normal, "asked after the turn");
        while let Ok((at, state)) = heard.try_recv() {
            if at > turned {
                assert_ne!(state, FieldSecurity::Normal, "a report after the turn");
            }
        }
        secure |= state == FieldSecurity::Secure;
        std::thread::sleep(Duration::from_millis(150));
    }
    assert!(secure, "the password field was never seen as one");
    let _ = platform.unwatch();
}
