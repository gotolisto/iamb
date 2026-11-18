//! Runs every scripted typing session in `crates/iamb-testkit/sessions/` (task 1.6 KPI: at
//! least 50 sessions, all passing) and prints each failure with expected and actual actions.
//!
//! The testkit's session runner and fake environment are compiled into this test with
//! `#[path]` rather than used as a dev-dependency: `iamb-testkit` depends on `iamb-core`, and
//! including the source keeps this crate's manifest unchanged while letting
//! `cargo llvm-cov -p iamb-core` count the sessions towards the core's coverage.

#[path = "../../iamb-testkit/src/env.rs"]
#[allow(
    dead_code,
    reason = "shared with the testkit crate; not every helper is used here"
)]
mod env;
#[path = "../../iamb-testkit/src/session.rs"]
#[allow(
    dead_code,
    reason = "shared with the testkit crate; not every helper is used here"
)]
mod session;

#[test]
fn every_session_passes() {
    let sessions = session::load_dir(&session::sessions_dir()).expect("sessions load");
    let mut failures = Vec::new();
    for (name, s) in &sessions {
        if let Err(e) = session::run(s) {
            failures.push(format!("--- {name}: {}\n{e}", s.description));
        }
    }
    eprintln!("{} sessions ran, {} failed", sessions.len(), failures.len());
    for f in &failures {
        eprintln!("{f}\n");
    }
    assert!(sessions.len() >= 50, "only {} sessions", sessions.len());
    assert!(
        failures.is_empty(),
        "{} of {} sessions failed",
        failures.len(),
        sessions.len()
    );
}
