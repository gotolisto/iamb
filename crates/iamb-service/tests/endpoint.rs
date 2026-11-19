//! The service endpoint: a worker whose endpoint name is taken still expands and serves the
//! endpoint once it is free (the 2.8 review's finding 12), and a client may take only the role
//! its executable holds (finding 10).

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use common::{Harness, LIBRARY, T, diagnostic_codes};
use iamb_ipc::{Client, ErrorCode, Role};
use iamb_platform::{LocalEndpoint, LocalListener, PeerIdentity, PeerPolicy};

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + T;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_taken_endpoint_name_does_not_stop_expansion_and_is_served_once_free() {
    let squatter: Arc<Mutex<Option<Box<dyn LocalListener>>>> = Arc::default();
    let held = Arc::clone(&squatter);
    let h = Harness::start_with(LIBRARY, move |config| {
        // Someone else holds the endpoint's name before the worker starts.
        let taken = config
            .endpoint
            .bind(&config.dirs.service_endpoint(), PeerPolicy::SameUser)
            .expect("the name is free before the worker starts");
        *held.lock().unwrap_or_else(PoisonError::into_inner) = Some(taken);
        config.endpoint_retry = Duration::from_millis(20);
    });
    assert!(!h.worker().endpoint_served());

    // Expansion does not need the endpoint (architecture §3).
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back ");

    // Still taken a while later: the worker keeps trying, and keeps running.
    std::thread::sleep(Duration::from_millis(100));
    assert!(!h.worker().endpoint_served());

    // The name is freed: the worker serves it at its next attempt.
    let taken = squatter
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .expect("held");
    taken.close().expect("closes");
    drop(taken);
    wait_until("the endpoint", || h.worker().endpoint_served());
    let mut c = h.client(Role::Ui);
    assert!(
        diagnostic_codes(&mut c).contains(&"endpoint_unavailable".to_owned()),
        "the time without the endpoint is reported"
    );
    drop(c);
    let dir = h.stop();
    let log = std::fs::read_to_string(dir.path().join("logs").join("worker.log")).expect("log");
    assert!(
        log.contains("the service endpoint cannot be served"),
        "{log}"
    );
    assert!(
        log.contains("the service endpoint is served, after"),
        "{log}"
    );
    assert!(log.contains("endpoint false"), "{log}");
}

#[test]
fn a_worker_that_stops_while_it_waits_for_the_name_stops() {
    let squatter: Arc<Mutex<Option<Box<dyn LocalListener>>>> = Arc::default();
    let held = Arc::clone(&squatter);
    let h = Harness::start_with(LIBRARY, move |config| {
        let taken = config
            .endpoint
            .bind(&config.dirs.service_endpoint(), PeerPolicy::SameUser)
            .expect("free");
        *held.lock().unwrap_or_else(PoisonError::into_inner) = Some(taken);
        // Longer than the test: only the stop can end the wait.
        config.endpoint_retry = Duration::from_mins(10);
    });
    let started = Instant::now();
    drop(h.stop());
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// A handle to the mock machine for a process running `executable`; `signed` is what the OS
/// crate's signature check would have said.
fn as_program(h: &Harness, executable: &str, signed: bool) -> iamb_platform::mock::MockPlatform {
    h.os.with_identity(PeerIdentity {
        pid: 4242,
        user: h.os.identity().user.clone(),
        executable: Some(PathBuf::from(executable)),
        trusted_binary: signed,
    })
}

#[test]
fn a_client_may_take_only_the_role_of_its_executable() {
    let h = Harness::start(LIBRARY);
    let connect = |executable: &str, role: Role| {
        let stream = as_program(&h, executable, false)
            .connect(&h.dirs().service_endpoint(), PeerPolicy::SameUser, T)
            .expect("connects");
        Client::handshake(stream, role, "test", T)
    };
    // The sync agent, taken over, says it is the UI host.
    for role in [Role::Ui, Role::Cli] {
        let refused = connect("/Applications/Iamb.app/Contents/Helpers/iamb-sync", role)
            .expect_err("refused");
        assert_eq!(
            refused.remote_code(),
            Some(ErrorCode::NotPermitted),
            "{role}"
        );
    }
    let sync = connect(
        "/Applications/Iamb.app/Contents/Helpers/iamb-sync",
        Role::Sync,
    )
    .expect("the sync agent as sync");
    assert_eq!(sync.role(), Role::Sync);
    // The UI host and the command-line tool, by their names on Windows.
    connect(
        r"C:\Users\Ada\AppData\Local\Iamb\iamb-desktop.exe",
        Role::Ui,
    )
    .expect("ui");
    assert!(
        connect(
            r"C:\Users\Ada\AppData\Local\Iamb\iamb-desktop.exe",
            Role::Cli
        )
        .is_err()
    );
    connect(r"C:\Users\Ada\AppData\Local\Iamb\iamb.exe", Role::Cli).expect("cli");
    assert!(connect(r"C:\Users\Ada\AppData\Local\Iamb\iamb.exe", Role::Ui).is_err());
}

#[test]
fn release_builds_refuse_a_program_that_is_not_an_iamb_client() {
    let h = Harness::start_with(LIBRARY, |config| {
        config.policy = PeerPolicy::InstalledBinary;
    });
    // Both are signed binaries of the installation as far as the endpoint can tell (the OS
    // crates check that); the name decides which may connect.
    let stream = as_program(&h, "/tmp/some-tool", true)
        .connect(&h.dirs().service_endpoint(), PeerPolicy::SameUser, T)
        .expect("connects");
    let refused = Client::handshake(stream, Role::Cli, "test", T).expect_err("refused");
    assert_eq!(refused.remote_code(), Some(ErrorCode::NotPermitted));
    let stream = as_program(
        &h,
        "/Applications/Iamb.app/Contents/MacOS/iamb-desktop",
        true,
    )
    .connect(&h.dirs().service_endpoint(), PeerPolicy::SameUser, T)
    .expect("connects");
    Client::handshake(stream, Role::Ui, "test", T).expect("the UI host");
}
