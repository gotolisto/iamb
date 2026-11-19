//! Contract tests over a connection: the handshake, protocol versions, malformed and oversized
//! frames, and role enforcement. The endpoint is the mock platform's in-memory one, so these
//! run identically on every OS.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use iamb_ipc::{
    Client, Direction, ErrorCode, FrameError, Group, HELLO_TIMEOUT, IpcError, MAX_FRAME_LEN,
    MessageLog, MessageRecord, PROTOCOL_VERSION, RequestBody, ResponseBody, Role, Secret,
    ServerConnection, Wire, read_frame, write_frame,
};
use iamb_platform::mock::MockPlatform;
use iamb_platform::{EndpointKind, EndpointName, LocalEndpoint, LocalStream, PeerPolicy};

const T: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Recorder(Mutex<Vec<MessageRecord>>);

impl MessageLog for Recorder {
    fn record(&self, record: &MessageRecord) {
        self.0.lock().expect("not poisoned").push(*record);
    }
}

fn name() -> EndpointName {
    EndpointName::new(EndpointKind::Service, "/mock/run")
}

/// A server on the mock endpoint that answers every permitted request with `Done`, serving one
/// connection per accept, and the records its log saw.
fn server(os: &MockPlatform) -> Arc<Recorder> {
    let listener = os
        .bind(&name(), PeerPolicy::SameUser)
        .expect("endpoint binds");
    let log = Arc::new(Recorder::default());
    let server_log: Arc<dyn MessageLog> = log.clone();
    thread::spawn(move || {
        while let Ok(stream) = listener.accept() {
            let log = Arc::clone(&server_log);
            thread::spawn(move || {
                if let Ok(conn) = ServerConnection::accept(stream, "9.9.9", HELLO_TIMEOUT, log) {
                    let _ = conn.serve(|_, _| Ok(ResponseBody::Done {}));
                }
            });
        }
    });
    log
}

fn raw(os: &MockPlatform) -> Box<dyn LocalStream> {
    os.connect(&name(), PeerPolicy::SameUser, T)
        .expect("connects")
}

fn client(os: &MockPlatform, role: Role) -> Client {
    Client::handshake(raw(os), role, "1.0.0", T).expect("handshake")
}

fn send_raw(stream: &mut Box<dyn LocalStream>, json: &str) -> Wire {
    write_frame(stream, json.as_bytes()).expect("written");
    stream.set_read_timeout(Some(T)).expect("timeout");
    let payload = read_frame(stream).expect("answer");
    Wire::decode(&payload).expect("answer decodes")
}

fn hello(stream: &mut Box<dyn LocalStream>, protocol: u32, role: &str) -> Wire {
    send_raw(
        stream,
        &format!(
            r#"{{"kind":"hello","protocol":{protocol},"role":"{role}","client_version":"x"}}"#
        ),
    )
}

#[test]
fn a_matching_hello_is_welcomed() {
    let os = MockPlatform::new();
    server(&os);
    let mut c = client(&os, Role::Cli);
    assert_eq!(c.server_version(), "9.9.9");
    assert_eq!(c.role(), Role::Cli);
    assert_eq!(c.peer().user, "mock-user");
    assert_eq!(
        c.call(RequestBody::GetStatus {}).expect("answered"),
        ResponseBody::Done {}
    );
}

#[test]
fn another_protocol_version_is_refused_with_a_clear_error() {
    let os = MockPlatform::new();
    server(&os);
    let mut stream = raw(&os);
    // A client of the previous version (it would misread the answer to `PutExclusion`).
    match hello(&mut stream, 1, "ui") {
        Wire::Refused { error } => {
            assert_eq!(error.code, ErrorCode::UnsupportedProtocol);
            assert!(error.message.contains("protocol 2"), "{}", error.message);
        }
        other => panic!("expected a refusal, got {}", other.name()),
    }
    // The server closed the connection.
    assert_eq!(read_frame(&mut stream), Err(FrameError::Eof));
}

#[test]
fn a_first_frame_that_is_not_a_hello_is_refused() {
    let os = MockPlatform::new();
    server(&os);
    for first in [
        r#"{"kind":"request","id":1,"body":{"type":"get_status"}}"#,
        "not json at all",
        r#"{"kind":"hello","protocol":2,"role":"admin","client_version":"x"}"#,
    ] {
        let mut stream = raw(&os);
        match send_raw(&mut stream, first) {
            Wire::Refused { error } => assert_eq!(error.code, ErrorCode::BadHello, "{first}"),
            other => panic!("expected a refusal, got {}", other.name()),
        }
    }
}

#[test]
fn the_client_reports_a_refusal() {
    let os = MockPlatform::new();
    let listener = os.bind(&name(), PeerPolicy::SameUser).expect("binds");
    let t = thread::spawn(move || {
        let mut stream = listener.accept().expect("accepted");
        let _ = read_frame(&mut stream);
        let refusal =
            br#"{"kind":"refused","error":{"code":"unsupported_protocol","message":"speaks 7"}}"#;
        write_frame(&mut stream, refusal).expect("written");
    });
    let err = Client::handshake(raw(&os), Role::Ui, "1", T).expect_err("refused");
    assert_eq!(err.remote_code(), Some(ErrorCode::UnsupportedProtocol));
    t.join().expect("server thread");
}

#[test]
fn malformed_frames_get_an_error_and_the_connection_stays_usable() {
    let os = MockPlatform::new();
    server(&os);
    let mut stream = raw(&os);
    assert!(matches!(
        hello(&mut stream, PROTOCOL_VERSION, "ui"),
        Wire::Welcome(_)
    ));

    let bad = |stream: &mut Box<dyn LocalStream>, json: &str| match send_raw(stream, json) {
        Wire::Response(r) => (r.id, r.error.map(|e| e.code)),
        other => panic!("expected a response, got {}", other.name()),
    };
    assert_eq!(
        bad(&mut stream, "{ broken"),
        (0, Some(ErrorCode::BadRequest))
    );
    assert_eq!(bad(&mut stream, ""), (0, Some(ErrorCode::BadRequest)));
    assert_eq!(
        bad(
            &mut stream,
            r#"{"kind":"request","id":7,"body":{"type":"make_coffee"}}"#
        ),
        (7, Some(ErrorCode::UnknownRequest))
    );
    assert_eq!(
        bad(
            &mut stream,
            r#"{"kind":"request","id":8,"body":{"type":"get_snippet"}}"#
        ),
        (8, Some(ErrorCode::UnknownRequest)),
        "a known type with a missing field cannot be told apart from an unknown one"
    );
    assert_eq!(
        bad(
            &mut stream,
            r#"{"kind":"welcome","protocol":2,"server_version":"x"}"#
        ),
        (0, Some(ErrorCode::BadRequest))
    );
    // Still in step: a good request works.
    match send_raw(
        &mut stream,
        r#"{"kind":"request","id":9,"body":{"type":"get_status"}}"#,
    ) {
        Wire::Response(r) => {
            assert_eq!(r.id, 9);
            assert_eq!(r.result, Some(ResponseBody::Done {}));
        }
        other => panic!("expected a response, got {}", other.name()),
    }
}

#[test]
fn an_oversized_frame_is_refused_and_the_connection_closed() {
    let os = MockPlatform::new();
    server(&os);
    let mut stream = raw(&os);
    assert!(matches!(
        hello(&mut stream, PROTOCOL_VERSION, "cli"),
        Wire::Welcome(_)
    ));
    // Only the header: the server must not wait for 8 MB of body.
    let len = u32::try_from(MAX_FRAME_LEN + 1).expect("fits");
    stream.write_all(&len.to_le_bytes()).expect("written");
    stream.set_read_timeout(Some(T)).expect("timeout");
    let answer = Wire::decode(&read_frame(&mut stream).expect("answer")).expect("decodes");
    match answer {
        Wire::Response(r) => assert_eq!(r.error.map(|e| e.code), Some(ErrorCode::BadRequest)),
        other => panic!("expected an error response, got {}", other.name()),
    }
    assert_eq!(read_frame(&mut stream), Err(FrameError::Eof));
}

#[test]
fn a_client_never_sends_an_oversized_request() {
    let os = MockPlatform::new();
    server(&os);
    let mut c = client(&os, Role::Ui);
    let huge = "x".repeat(MAX_FRAME_LEN);
    let err = c
        .call(RequestBody::Search {
            text: Secret::new(huge),
            limit: 1,
        })
        .expect_err("too large");
    assert!(matches!(err, IpcError::Frame(FrameError::TooLarge(_))));
}

/// One request of every group.
fn one_of_each_group() -> Vec<(Group, RequestBody)> {
    vec![
        (Group::Library, RequestBody::ListFolders {}),
        (Group::EngineControl, RequestBody::GetStatus {}),
        (Group::Forms, RequestBody::ReopenLastForm {}),
        (
            Group::Templates,
            RequestBody::ParseTemplate {
                source: String::new(),
            },
        ),
        (Group::Diagnostics, RequestBody::RecentDiagnostics {}),
        (Group::Settings, RequestBody::GetSettings {}),
        (Group::Sync, RequestBody::MissingAttachments {}),
    ]
}

#[test]
fn each_role_reaches_its_groups_only() {
    let os = MockPlatform::new();
    server(&os);
    for role in Role::ALL {
        let mut c = client(&os, role);
        for (group, body) in one_of_each_group() {
            assert_eq!(body.group(), group);
            let outcome = c.call(body);
            if role.may_call(group) {
                assert_eq!(outcome, Ok(ResponseBody::Done {}), "{role} {group:?}");
            } else {
                assert_eq!(
                    outcome.map_err(|e| e.remote_code()),
                    Err(Some(ErrorCode::NotPermitted)),
                    "{role} {group:?}"
                );
            }
        }
    }
}

#[test]
fn the_log_sees_names_and_sizes_never_bodies() {
    let os = MockPlatform::new();
    let log = server(&os);
    let mut c = client(&os, Role::Ui);
    c.call(RequestBody::Search {
        text: Secret::new("canary-words".to_owned()),
        limit: 3,
    })
    .expect("answered");
    // The server records a response after writing it, so the client can be faster.
    let mut records = Vec::new();
    for _ in 0..100 {
        records = log.0.lock().expect("not poisoned").clone();
        if records.len() >= 4 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let names: Vec<(Direction, &str)> = records.iter().map(|r| (r.direction, r.name)).collect();
    assert_eq!(
        names,
        [
            (Direction::In, "hello"),
            (Direction::Out, "welcome"),
            (Direction::In, "search"),
            (Direction::Out, "done"),
        ]
    );
    let search = &records[2];
    assert!(search.size > 0);
    assert!(records[3].duration.is_some(), "responses carry a duration");
    // A record is plain data: its Debug cannot hold the body.
    assert!(!format!("{records:?}").contains("canary"));
}
