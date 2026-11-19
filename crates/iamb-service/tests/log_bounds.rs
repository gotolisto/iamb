//! What clients can put in the worker's log (the 2.8 review's finding 10): the version string
//! of a hello is client text of up to 8 MB.

mod common;

use common::{Harness, LIBRARY, T};
use iamb_ipc::{Client, RequestBody, Role};
use iamb_platform::{LocalEndpoint, PeerPolicy};

fn log_text(h: &Harness) -> String {
    std::fs::read_to_string(h.dirs().logs().join("worker.log")).unwrap_or_default()
}

#[test]
fn a_clients_version_string_reaches_the_log_short_and_plain() {
    let h = Harness::start(LIBRARY);
    let before = log_text(&h).len();
    for version in [
        format!("9.9.9 {}", "X".repeat(300_000)),
        "1.0\r\n2026-01-01T00:00:00.000Z ERROR worker[1]: forged line".to_owned(),
    ] {
        let stream =
            h.os.connect(&h.dirs().service_endpoint(), PeerPolicy::SameUser, T)
                .expect("connects");
        let mut c = Client::handshake(stream, Role::Cli, &version, T).expect("handshake");
        // The connection line is written before the first request is served.
        c.call(RequestBody::GetStatus {}).expect("served");
    }
    let text = log_text(&h);
    let grown = text.len() - before;
    assert!(grown < 1_000, "two hellos grew the log by {grown} bytes");
    assert!(text.lines().all(|l| l.len() < 300), "a line is too long");
    assert!(
        !text.lines().any(|l| l.contains("forged line")),
        "a client wrote a line of its own"
    );
    assert!(text.contains("version 9.9.9?XXXXXXXXXXXXXXXXXXXXXXXXXX..."));
}
