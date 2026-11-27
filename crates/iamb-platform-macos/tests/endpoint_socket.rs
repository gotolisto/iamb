//! The macOS endpoint on a real socket: exclusive bind, stale sockets, permissions, peer
//! identity, and a round trip in both directions. Runs on macOS only (CI's macOS leg).

#![cfg(target_os = "macos")]

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use iamb_platform::{
    EndpointKind, EndpointName, LocalEndpoint, LocalListener, PeerPolicy, PlatformError,
};
use iamb_platform_macos::Platform;

const T: Duration = Duration::from_secs(5);

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("iamb-endpoint-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    fn name(&self) -> EndpointName {
        EndpointName::new(EndpointKind::Service, self.0.join("run"))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn bind_is_private_and_exclusive() {
    let dir = Dir::new("bind");
    let p = Platform::new();
    let first = p.bind(&dir.name(), PeerPolicy::SameUser).expect("binds");
    assert_eq!(
        fs::metadata(dir.name().run_dir()).expect("run").mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(dir.name().socket_path()).expect("sock").mode() & 0o777,
        0o600
    );
    assert!(matches!(
        p.bind(&dir.name(), PeerPolicy::SameUser),
        Err(PlatformError::EndpointInUse)
    ));
    first.close().expect("closes");
    drop(first);
    let _again = p
        .bind(&dir.name(), PeerPolicy::SameUser)
        .expect("free again");
}

#[test]
fn a_stale_socket_is_replaced_and_a_live_one_is_not() {
    let dir = Dir::new("stale");
    let name = dir.name();
    fs::create_dir_all(name.run_dir()).expect("run");
    fs::set_permissions(name.run_dir(), fs::Permissions::from_mode(0o700)).expect("chmod");
    let squatter = UnixListener::bind(name.socket_path()).expect("squats");
    let p = Platform::new();
    assert!(matches!(
        p.bind(&name, PeerPolicy::SameUser),
        Err(PlatformError::EndpointInUse)
    ));
    drop(squatter);
    let _l = p
        .bind(&name, PeerPolicy::SameUser)
        .expect("stale socket replaced");
}

#[test]
fn nothing_listening_is_not_found() {
    let dir = Dir::new("none");
    assert!(matches!(
        Platform::new().connect(&dir.name(), PeerPolicy::SameUser, T),
        Err(PlatformError::EndpointNotFound)
    ));
}

#[test]
fn a_round_trip_with_verified_peers() {
    let dir = Dir::new("trip");
    let p = Platform::new();
    let listener: Arc<dyn LocalListener> =
        Arc::from(p.bind(&dir.name(), PeerPolicy::SameUser).expect("binds"));
    let server = {
        let listener = Arc::clone(&listener);
        thread::spawn(move || {
            let mut stream = listener.accept().expect("accepted");
            assert_eq!(stream.peer().pid, std::process::id());
            let mut buf = [0_u8; 4];
            stream.read_exact(&mut buf).expect("read");
            stream.write_all(&buf).expect("echo");
        })
    };
    let mut client = p
        .connect(&dir.name(), PeerPolicy::SameUser, T)
        .expect("connects");
    assert_eq!(client.peer().pid, std::process::id());
    assert!(client.peer().executable.is_some());
    client.write_all(b"ping").expect("write");
    let mut buf = [0_u8; 4];
    client.read_exact(&mut buf).expect("read");
    assert_eq!(&buf, b"ping");
    server.join().expect("server");

    // An unsigned test binary is not the installed Iamb: the release policy refuses it.
    assert!(matches!(
        p.connect(&dir.name(), PeerPolicy::InstalledBinary, T),
        Err(PlatformError::PeerRejected(_))
    ));
    listener.close().expect("closes");
}

#[test]
fn close_wakes_accept_after_the_run_directory_is_gone() {
    // A data folder removed while the service runs must not leave accept blocked (the beta-gap
    // review's finding 10): the wake-up does not go through the socket's path.
    let dir = Dir::new("gone");
    let p = Platform::new();
    let listener: Arc<dyn LocalListener> = p
        .bind(&dir.name(), PeerPolicy::SameUser)
        .expect("binds")
        .into();
    let accepting = Arc::clone(&listener);
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(accepting.accept().err());
    });
    thread::sleep(Duration::from_millis(100));
    fs::remove_dir_all(dir.name().run_dir()).expect("removed");
    listener.close().expect("closed");
    let woken = rx.recv_timeout(T);
    assert!(
        matches!(woken, Ok(Some(PlatformError::Closed))),
        "accept was not woken: {woken:?}"
    );
}
