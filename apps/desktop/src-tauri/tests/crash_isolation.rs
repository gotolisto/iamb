//! A UI host that crashes does not stop expansion (task 3.2; architecture §3: "If the UI host
//! or the sync agent crashes or is not installed, plain-text expansion continues unchanged").
//!
//! Two ways a UI host can die are tested against a real worker on the mock platform:
//!
//! - its connection ends abruptly while a request is in flight and a fill-in form it was asked
//!   to show is pending (in this process, on the mock endpoint: every OS);
//! - its process is killed (`SIGKILL`) in the same state (another process, on the operating
//!   system's endpoint: Linux, like the command-line tool's service test; the OS endpoints are
//!   tested on their own systems by their crates).
//!
//! In both, the pending form is abandoned after the service's 2 s acknowledgement timeout, as
//! when no UI host is connected (architecture §4.5 step 2), typing an abbreviation then expands
//! as before, and the worker's log shows the client's connection and its end and nothing else:
//! no warning, no error, no crash report.
//!
//! What does not recover yet: a UI host that dies after acknowledging a form leaves that form
//! session open in the engine, and while a session is open the engine drops every typed match
//! (`form_open`), so expansion stays off until the worker restarts. The test of that case is
//! ignored until the service cancels the open form session when its UI client goes (see the
//! task report and `forms_and_expansion_recover_after_the_ui_host_died_with_a_form_open`).

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{Endpoint, Service, T, files_named, problems};

/// A little more than the service's form acknowledgement timeout (`FORM_ACK_TIMEOUT`, 2 s).
const ACK_TIMEOUT: Duration = Duration::from_millis(2_500);
use iamb_desktop::contract::UiEvent;
use iamb_ipc::{
    EventBody, PROTOCOL_VERSION, Request, RequestBody, ResponseBody, Role, Wire, read_frame,
    write_frame,
};
use iamb_platform::PeerPolicy;

/// Set in the child process of `killing_the_ui_process_leaves_expansion_running`: the data
/// root of the parent's worker.
const CHILD_ENV: &str = "IAMB_DESKTOP_TEST_CHILD_DATA";

/// The worker's log lines that mention client `n`'s connection.
fn client_lines(log: &str) -> Vec<String> {
    log.lines()
        .filter(|l| l.contains("client "))
        .map(str::to_owned)
        .collect()
}

/// Expansion works and the UI host is gone from the service's point of view.
fn assert_expansion_unaffected(service: &Service, before: &str) {
    service.type_text("omw ");
    assert!(
        service.text().ends_with("on my way "),
        "plain expansion continues: {:?}",
        service.text()
    );
    let mut cli = service.client(Role::Cli);
    let deadline = Instant::now() + T;
    loop {
        match cli.call(RequestBody::GetStatus {}).expect("status") {
            ResponseBody::Status { status } if !status.ui_connected => break,
            ResponseBody::Status { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "the service still counts a UI host"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let log = service.worker_log();
    assert_eq!(
        problems(&log),
        problems(before),
        "no warning or error after the UI host died"
    );
    assert!(files_named(&service.dirs().logs(), "crash-").is_empty());
}

#[test]
fn a_connection_dropped_mid_request_with_a_form_pending_leaves_expansion_running() {
    let service = Service::start(Endpoint::Mock);
    let before = service.worker_log();
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    service.type_text("brb ");
    assert_eq!(service.text(), "be right back ");

    // A form is requested from the UI host; it dies before showing it.
    service.type_text("hi ");
    seen.wait_event(|e| matches!(e, UiEvent::FormRequested { .. }));

    // A second UI connection sends a request and vanishes before the answer.
    let mut raw = service
        .client_endpoint()
        .connect(&service.dirs().service_endpoint(), PeerPolicy::SameUser, T)
        .expect("connects");
    let hello = Wire::Hello(iamb_ipc::Hello {
        protocol: PROTOCOL_VERSION,
        role: Role::Ui,
        client_version: "crash-test".to_owned(),
    });
    write_frame(&mut raw, &hello.encode().expect("hello")).expect("hello sent");
    raw.set_read_timeout(Some(T)).expect("timeout");
    let welcome = read_frame(&mut raw).expect("welcome");
    assert!(matches!(Wire::decode(&welcome), Ok(Wire::Welcome(_))));
    let request = Wire::Request(Request {
        id: 1,
        body: RequestBody::ListSnippets {
            query: iamb_ipc::SnippetQuery::default(),
        },
    });
    write_frame(&mut raw, &request.encode().expect("request")).expect("request sent");
    raw.shutdown().expect("dropped");
    drop(raw);

    // The bridge's own connection ends abruptly too (the stream is shut down, as when the
    // process dies), with the form still pending.
    bridge.stop();

    std::thread::sleep(ACK_TIMEOUT);
    service.type_text("brb ");
    assert!(
        service.text().ends_with("be right back "),
        "{:?}",
        service.text()
    );
    assert_expansion_unaffected(&service, &before);
    let lines = client_lines(&service.worker_log());
    assert!(
        lines.iter().all(|l| l.contains(" connected: role ")
            || l.contains(" disconnected")
            || l.contains(" dropped: ")),
        "only connections and their ends: {lines:#?}"
    );
    assert!(lines.iter().filter(|l| l.contains("role ui")).count() >= 2);
}

/// The UI host of `killing_the_ui_process_leaves_expansion_running`, in a child process: the
/// real bridge on the operating system's endpoint. Does nothing in an ordinary test run.
#[cfg(target_os = "linux")]
#[test]
fn child_ui_host() {
    use std::io::Write as _;
    use std::sync::Arc;

    use iamb_desktop::bridge::{Bridge, BridgeConfig};
    use iamb_desktop::contract::ConnectionState;
    use iamb_platform::{EndpointKind, EndpointName};

    let Ok(root) = std::env::var(CHILD_ENV) else {
        return;
    };
    let say = |line: &str| {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    let recorder = Arc::new(common::Recorder::default());
    let mut cfg = BridgeConfig::new(
        iamb_ipc::native_endpoint(),
        EndpointName::new(
            EndpointKind::Service,
            std::path::Path::new(&root).join("run"),
        ),
        Arc::new(iamb_desktop::log::Logger::discard()),
    );
    cfg.policy = PeerPolicy::SameUser;
    let bridge = Bridge::start(cfg, recorder.clone()).expect("bridge");
    recorder.wait_status(|v| v.connection == ConnectionState::Connected);
    say("CHILD-CONNECTED");
    recorder.wait_event(|e| matches!(e, UiEvent::FormRequested { .. }));
    // A request in flight when the process dies.
    let busy = bridge.clone();
    std::thread::spawn(move || {
        loop {
            let _ = busy.call(RequestBody::ListSnippets {
                query: iamb_ipc::SnippetQuery::default(),
            });
        }
    });
    say("CHILD-FORM");
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn killing_the_ui_process_leaves_expansion_running() {
    let service = Service::start(Endpoint::Native);
    let before = service.worker_log();
    let mut child = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "child_ui_host",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, service.dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("child starts");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let mut wait_for = |marker: &str| {
        for line in lines.by_ref() {
            if line.expect("child output").contains(marker) {
                return;
            }
        }
        panic!("the child ended before {marker}");
    };
    wait_for("CHILD-CONNECTED");
    service.type_text("hi ");
    wait_for("CHILD-FORM");

    child.kill().expect("killed");
    let status = child.wait().expect("reaped");
    assert!(!status.success());

    std::thread::sleep(ACK_TIMEOUT);
    service.type_text("brb ");
    assert!(
        service.text().ends_with("be right back "),
        "{:?}",
        service.text()
    );
    assert_expansion_unaffected(&service, &before);
    let ui_lines: Vec<String> = client_lines(&service.worker_log())
        .into_iter()
        .filter(|l| !l.contains("role cli"))
        .collect();
    assert!(
        ui_lines.iter().any(|l| l.contains("connected: role ui")),
        "{ui_lines:#?}"
    );
    assert!(
        ui_lines
            .iter()
            .any(|l| l.contains("disconnected") || l.contains("dropped")),
        "the end of the connection is logged: {ui_lines:#?}"
    );
}

/// A UI host that dies after acknowledging a form: expansion and forms must recover without
/// restarting the worker. Needs the service to cancel the open form session when the UI client
/// that was sent it disconnects (`crates/iamb-service/src/worker/ipc.rs`, `serve`, telling the
/// engine thread as `FormCancel` does); today the session stays open and the engine drops every
/// typed match with `form_open`.
#[test]
#[ignore = "needs the service to cancel an open form when its UI client goes (task 3.2 report)"]
fn forms_and_expansion_recover_after_the_ui_host_died_with_a_form_open() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    service.type_text("hi ");
    let UiEvent::FormRequested { session, .. } =
        seen.wait_event(|e| matches!(e, UiEvent::FormRequested { .. }))
    else {
        unreachable!()
    };
    bridge
        .call(RequestBody::FormAck { session })
        .expect("acknowledged");
    bridge.stop();

    std::thread::sleep(ACK_TIMEOUT);
    service.type_text("brb ");
    assert!(
        service.text().ends_with("be right back "),
        "expansion is off while the dead host's form is open: {:?}",
        service.text()
    );

    let mut next = service.client(Role::Ui);
    service.type_text("hi ");
    let deadline = Instant::now() + T;
    loop {
        match next
            .next_event(Some(Duration::from_millis(100)))
            .expect("events")
        {
            Some(EventBody::FormRequested { .. }) => break,
            Some(EventBody::DiagnosticAdded { entry }) => {
                assert_ne!(
                    entry.code, "form_open",
                    "the dead host's form is still open"
                );
            }
            _ => {}
        }
        assert!(Instant::now() < deadline, "no form for the new UI host");
    }
}
