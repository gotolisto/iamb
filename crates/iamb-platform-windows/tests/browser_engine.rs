//! The secure-field answer for a real browser engine's window: Microsoft Edge (the engine of the
//! system's web view) in an application window showing a `<textarea>`, then an
//! `<input type="password">`. The engine switches its accessibility on only when it is first
//! asked and then builds its tree in the background, so the answer is asked again, as the
//! watcher does, until it settles; the textarea must come out `Normal` and the password field
//! `Secure`, never the other way round.
//!
//! **Not run in CI** (`#[ignore]`): it needs Edge, an interactive desktop that lets the new
//! window take the foreground, and a first start of a browser profile, which takes a few
//! seconds and varies from machine to machine. Run it by hand on Windows:
//!
//! ```text
//! cargo test -p iamb-platform-windows --test browser_engine -- --ignored --test-threads 1
//! ```
//!
//! It prints how long the answer took to settle and which unknown reasons came first, which is
//! what "Why didn't it expand?" shows as "Reason: …".

#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use iamb_platform::{FieldSecurity, FocusChange, FocusInspector, FocusToken, SECURE_STATE_TIMEOUT};
use iamb_platform_windows::Platform;

/// How long the window may take to come to the front, and the answer to settle.
const PATIENCE: Duration = Duration::from_secs(30);

/// Edge's executable, where the installer puts it.
fn edge() -> Option<PathBuf> {
    ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|dir| PathBuf::from(dir).join(r"Microsoft\Edge\Application\msedge.exe"))
        .find(|path| path.is_file())
}

/// A page with one field, focused as soon as it loads.
fn page(field: &str) -> String {
    // A data URL: no file and no server. `%23` is `#`, which would end the URL.
    format!(
        "data:text/html,<!doctype html><title>iamb</title><body>{field}\
         <script>document.querySelector('%23f').focus()</script>"
    )
}

/// Edge in an application window of a profile of its own, closed when dropped.
struct Browser {
    child: Child,
    profile: PathBuf,
}

impl Browser {
    fn open(edge: &PathBuf, field: &str, name: &str) -> Self {
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
            .arg(format!("--app={}", page(field)))
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

/// The first window of `msedge.exe` the watcher reports.
fn edge_window(changes: &Receiver<FocusChange>) -> Option<FocusToken> {
    let deadline = Instant::now() + PATIENCE;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match changes.recv_timeout(left) {
            Ok(change) if change.app.app.0 == "msedge.exe" => return Some(change.token),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

/// Asks as the service does (100 ms each time) until the answer is no longer unknown, and says
/// what came before.
fn settle(platform: &Platform, token: &FocusToken) -> FieldSecurity {
    let started = Instant::now();
    let mut reasons = Vec::new();
    loop {
        let state = platform.secure_state(token, SECURE_STATE_TIMEOUT);
        match state {
            FieldSecurity::Unknown(reason) if started.elapsed() < PATIENCE => {
                if reasons.last() != Some(&reason) {
                    reasons.push(reason);
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            _ => {
                eprintln!(
                    "settled as {state:?} after {:?}; unknown before: {reasons:?}",
                    started.elapsed()
                );
                return state;
            }
        }
    }
}

fn judged(field: &str, name: &str) -> Option<FieldSecurity> {
    let Some(edge) = edge() else {
        eprintln!("SKIPPED: Microsoft Edge is not installed");
        return None;
    };
    let platform = Platform::new();
    let (sink, changes) = mpsc::channel();
    platform
        .watch(Box::new(move |change: FocusChange| {
            let _ = sink.send(change);
        }))
        .expect("the watcher starts");
    let browser = Browser::open(&edge, field, name);
    let Some(token) = edge_window(&changes) else {
        eprintln!("SKIPPED: Edge's window did not come to the front");
        return None;
    };
    let state = settle(&platform, &token);
    let _ = platform.unwatch();
    drop(browser);
    Some(state)
}

#[test]
#[ignore = "needs Edge and an interactive desktop; run by hand (module docs)"]
fn a_text_area_of_a_browser_engine_is_normal() {
    if let Some(state) = judged("<textarea id=f></textarea>", "text") {
        assert_eq!(state, FieldSecurity::Normal);
    }
}

#[test]
#[ignore = "needs Edge and an interactive desktop; run by hand (module docs)"]
fn a_password_field_of_a_browser_engine_is_secure() {
    if let Some(state) = judged("<input id=f type=password>", "password") {
        assert_eq!(state, FieldSecurity::Secure);
    }
}
