//! The Linux endpoint: exclusive bind, stale sockets, permissions, and a round trip with the
//! framing and handshake on top.

#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use iamb_ipc::linux::LinuxEndpoint;
use iamb_ipc::{
    Client, HELLO_TIMEOUT, NoLog, RequestBody, ResponseBody, Role, Secret, ServerConnection,
};
use iamb_platform::{
    EndpointKind, EndpointName, LocalEndpoint, PeerPolicy, PeerRejection, PlatformError,
};
use tempfile::TempDir;

const T: Duration = Duration::from_secs(5);

fn name(dir: &TempDir) -> EndpointName {
    EndpointName::new(EndpointKind::Service, dir.path().join("run"))
}

#[test]
fn bind_creates_a_private_directory_and_socket() {
    let dir = TempDir::new().expect("temp dir");
    let name = name(&dir);
    let _listener = LinuxEndpoint
        .bind(&name, PeerPolicy::SameUser)
        .expect("binds");
    let run = fs::metadata(name.run_dir()).expect("run dir");
    assert_eq!(run.mode() & 0o777, 0o700);
    let sock = fs::metadata(name.socket_path()).expect("socket");
    assert_eq!(sock.mode() & 0o777, 0o600);
}

#[test]
fn a_loose_run_directory_is_tightened() {
    let dir = TempDir::new().expect("temp dir");
    let name = name(&dir);
    fs::create_dir(name.run_dir()).expect("mkdir");
    fs::set_permissions(name.run_dir(), fs::Permissions::from_mode(0o755)).expect("chmod");
    let _listener = LinuxEndpoint
        .bind(&name, PeerPolicy::SameUser)
        .expect("binds");
    let run = fs::metadata(name.run_dir()).expect("run dir");
    assert_eq!(run.mode() & 0o777, 0o700);
}

#[test]
fn a_second_bind_fails_while_the_first_lives() {
    let dir = TempDir::new().expect("temp dir");
    let name = name(&dir);
    let first = LinuxEndpoint
        .bind(&name, PeerPolicy::SameUser)
        .expect("binds");
    assert!(matches!(
        LinuxEndpoint.bind(&name, PeerPolicy::SameUser),
        Err(PlatformError::EndpointInUse)
    ));
    // After the first closes, the name is free again.
    first.close().expect("closes");
    drop(first);
    let _again = LinuxEndpoint
        .bind(&name, PeerPolicy::SameUser)
        .expect("binds again");
}

#[test]
fn a_live_socket_of_another_process_is_not_replaced() {
    // A server that does not use the lock file (an older build, or a squatter of the same
    // user): something answers on the path, so bind must refuse rather than remove it.
    let dir = TempDir::new().expect("temp dir");
    let name = name(&dir);
    fs::create_dir(name.run_dir()).expect("mkdir");
    fs::set_permissions(name.run_dir(), fs::Permissions::from_mode(0o700)).expect("chmod");
    let _squatter = UnixListener::bind(name.socket_path()).expect("squats");
    assert!(matches!(
        LinuxEndpoint.bind(&name, PeerPolicy::SameUser),
        Err(PlatformError::EndpointInUse)
    ));
    assert!(name.socket_path().exists());
}

#[test]
fn a_stale_socket_is_replaced() {
    let dir = TempDir::new().expect("temp dir");
    let name = name(&dir);
    fs::create_dir(name.run_dir()).expect("mkdir");
    fs::set_permissions(name.run_dir(), fs::Permissions::from_mode(0o700)).expect("chmod");
    // A socket file nothing listens on: what a killed server leaves behind.
    drop(UnixListener::bind(name.socket_path()).expect("binds"));
    assert!(name.socket_path().exists());
    assert!(matches!(
        LinuxEndpoint.connect(&name, PeerPolicy::SameUser, T),
        Err(PlatformError::EndpointNotFound)
    ));
    let _listener = LinuxEndpoint
        .bind(&name, PeerPolicy::SameUser)
        .expect("replaces the stale socket");
}

#[test]
fn connecting_to_nothing_is_not_found() {
    let dir = TempDir::new().expect("temp dir");
    assert!(matches!(
        LinuxEndpoint.connect(&name(&dir), PeerPolicy::SameUser, T),
        Err(PlatformError::EndpointNotFound)
    ));
}

#[test]
fn a_run_directory_of_another_user_is_refused() {
    // `/` is owned by root; unless the tests run as root, it is another user's directory.
    let own = fs::metadata("/proc/self").expect("proc").uid();
    if own == 0 {
        return;
    }
    let name = EndpointName::new(EndpointKind::Service, "/");
    assert!(matches!(
        LinuxEndpoint.connect(&name, PeerPolicy::SameUser, T),
        Err(PlatformError::EndpointNotFound | PlatformError::PeerRejected(PeerRejection::OtherUser))
    ));
    assert!(matches!(
        LinuxEndpoint.bind(&name, PeerPolicy::SameUser),
        Err(PlatformError::PeerRejected(PeerRejection::OtherUser) | PlatformError::Os { .. })
    ));
}

#[test]
fn close_wakes_a_blocked_accept_and_removes_the_socket() {
    let dir = TempDir::new().expect("temp dir");
    let name = name(&dir);
    let listener: Arc<dyn iamb_platform::LocalListener> = Arc::from(
        LinuxEndpoint
            .bind(&name, PeerPolicy::SameUser)
            .expect("binds"),
    );
    let waiting = Arc::clone(&listener);
    let t = thread::spawn(move || waiting.accept().map(|_| ()));
    thread::sleep(Duration::from_millis(50));
    listener.close().expect("closes");
    assert_eq!(t.join().expect("thread"), Err(PlatformError::Closed));
    assert!(!name.socket_path().exists());
}

#[test]
fn a_round_trip_with_the_framing_and_handshake() {
    let dir = TempDir::new().expect("temp dir");
    let name = name(&dir);
    let listener = LinuxEndpoint
        .bind(&name, PeerPolicy::SameUser)
        .expect("binds");
    let server = thread::spawn(move || {
        let stream = listener.accept().expect("accepted");
        let own = fs::metadata("/proc/self").expect("proc").uid().to_string();
        assert_eq!(stream.peer().user, own);
        let conn = ServerConnection::accept(stream, "1.2.3", HELLO_TIMEOUT, Arc::new(NoLog))
            .expect("hello");
        assert_eq!(conn.role(), Role::Cli);
        conn.serve(|_, body| match body {
            RequestBody::SetPaused { paused } => Ok(ResponseBody::Acked {
                count: u64::from(paused),
            }),
            _ => Ok(ResponseBody::Done {}),
        })
        .expect("served");
    });
    let mut client = Client::connect(
        &LinuxEndpoint,
        &name,
        PeerPolicy::for_build(),
        Role::Cli,
        "0.0.1",
        T,
    )
    .expect("connects");
    assert_eq!(client.server_version(), "1.2.3");
    assert_eq!(
        client.call(RequestBody::SetPaused { paused: true }),
        Ok(ResponseBody::Acked { count: 1 })
    );
    // A large message crosses the socket in pieces and still arrives whole.
    let big = "y".repeat(3 * 1024 * 1024);
    assert_eq!(
        client.call(RequestBody::Search {
            text: Secret::new(big),
            limit: 1
        }),
        Ok(ResponseBody::Done {})
    );
    client.close();
    server.join().expect("server thread");
}
