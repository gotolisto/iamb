//! Every scripted session parses and passes when run through the testkit crate itself.
//! (`crates/iamb-core/tests/engine_sessions.rs` runs the same sessions inside `iamb-core`'s own
//! tests, so coverage of the core counts them.)

use iamb_testkit::session::{load_dir, run, sessions_dir};

#[test]
fn all_sessions_pass() {
    let sessions = load_dir(&sessions_dir()).expect("sessions load");
    let failures: Vec<String> = sessions
        .iter()
        .filter_map(|(name, s)| run(s).err().map(|e| format!("{name}: {e}")))
        .collect();
    assert!(sessions.len() >= 50, "only {} sessions", sessions.len());
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
