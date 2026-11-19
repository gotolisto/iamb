//! The Windows endpoint on a real named pipe: exclusive bind, not found, a round trip with the
//! peer identified in both directions, read timeouts, shutdown and close, and the pipe's
//! secret name (the 2.8 review's finding 12). Runs on Windows only (CI's Windows leg).

#![cfg(windows)]

use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use iamb_platform::{
    EndpointKind, EndpointName, LocalEndpoint, LocalListener, PeerPolicy, PlatformError,
};
use iamb_platform_windows::Platform;

const T: Duration = Duration::from_secs(5);

/// A name that no other test or installed service uses.
fn name(tag: &str) -> EndpointName {
    let dir = std::env::temp_dir().join(format!("iamb-pipe-{tag}-{}", std::process::id()));
    EndpointName::new(EndpointKind::Service, dir.join("run"))
}

#[test]
fn a_second_bind_fails_while_the_first_lives() {
    let p = Platform::new();
    let n = name("bind");
    let first = p.bind(&n, PeerPolicy::SameUser).expect("binds");
    assert!(matches!(
        p.bind(&n, PeerPolicy::SameUser),
        Err(PlatformError::EndpointInUse)
    ));
    first.close().expect("closes");
    drop(first);
    let _again = p.bind(&n, PeerPolicy::SameUser).expect("free again");
}

#[test]
fn nothing_listening_is_not_found() {
    assert!(matches!(
        Platform::new().connect(&name("none"), PeerPolicy::SameUser, T),
        Err(PlatformError::EndpointNotFound)
    ));
}

#[test]
fn a_round_trip_with_verified_peers_and_timeouts() {
    let p = Platform::new();
    let n = name("trip");
    let listener: Arc<dyn LocalListener> =
        Arc::from(p.bind(&n, PeerPolicy::SameUser).expect("binds"));
    let server = {
        let listener = Arc::clone(&listener);
        thread::spawn(move || {
            let mut stream = listener.accept().expect("accepted");
            assert_eq!(stream.peer().pid, std::process::id());
            let mut buf = [0_u8; 4];
            stream.read_exact(&mut buf).expect("read");
            stream.write_all(&buf).expect("echo");
            // Then a large write, read in pieces on the other side.
            stream
                .write_all(&vec![7_u8; 300 * 1024])
                .expect("large write");
            stream.shutdown().expect("shutdown");
        })
    };
    let mut client = p.connect(&n, PeerPolicy::SameUser, T).expect("connects");
    assert_eq!(client.peer().pid, std::process::id());
    assert!(client.peer().user.starts_with("S-1-"));
    client.write_all(b"ping").expect("write");
    let mut buf = [0_u8; 4];
    client.read_exact(&mut buf).expect("read");
    assert_eq!(&buf, b"ping");
    let mut big = vec![0_u8; 300 * 1024];
    client.read_exact(&mut big).expect("large read");
    assert!(big.iter().all(|b| *b == 7));
    server.join().expect("server");
    // The server disconnected: the end of the stream.
    let mut rest = [0_u8; 1];
    assert_eq!(client.read(&mut rest).expect("end"), 0);

    // An unsigned test binary is not the installed Iamb.
    assert!(matches!(
        p.connect(&n, PeerPolicy::InstalledBinary, T),
        Err(PlatformError::PeerRejected(_))
    ));
    listener.close().expect("closes");
}

#[test]
fn a_read_times_out_and_close_wakes_accept() {
    let p = Platform::new();
    let n = name("timeout");
    let listener: Arc<dyn LocalListener> =
        Arc::from(p.bind(&n, PeerPolicy::SameUser).expect("binds"));
    let accepted = {
        let listener = Arc::clone(&listener);
        thread::spawn(move || listener.accept())
    };
    let client = p.connect(&n, PeerPolicy::SameUser, T).expect("connects");
    // Keep the server's end open: dropping it would close the pipe, and the client's read
    // would then see end-of-file instead of waiting for data.
    let _server = accepted.join().expect("thread").expect("accepts");
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("timeout");
    let mut c = client.try_clone().expect("clone");
    let start = Instant::now();
    let mut buf = [0_u8; 1];
    let err = c.read(&mut buf).expect_err("nothing to read");
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(2));

    let waiting = {
        let listener = Arc::clone(&listener);
        thread::spawn(move || listener.accept().map(|_| ()))
    };
    thread::sleep(Duration::from_millis(100));
    listener.close().expect("closes");
    assert_eq!(waiting.join().expect("thread"), Err(PlatformError::Closed));
}

/// Finding 12: the pipe's name ends in a secret that only the user's run directory names, so
/// another user cannot create it first. Each endpoint of a data root has its own, and a bind
/// after a close draws a new one.
#[test]
fn the_pipe_name_is_a_secret_kept_in_the_run_directory() {
    let p = Platform::new();
    let service = name("secret");
    let sync = EndpointName::new(EndpointKind::Sync, service.run_dir());
    let a = p
        .bind(&service, PeerPolicy::SameUser)
        .expect("service binds");
    let b = p
        .bind(&sync, PeerPolicy::SameUser)
        .expect("sync binds beside it");
    let read =
        |file: &str| std::fs::read_to_string(service.run_dir().join(file)).expect("name file");
    let (one, other) = (read("service.pipe"), read("sync.pipe"));
    for secret in [&one, &other] {
        let secret = secret.trim_end();
        assert_eq!(secret.len(), 32, "{secret:?}");
        assert!(secret.bytes().all(|c| c.is_ascii_hexdigit()), "{secret:?}");
    }
    assert_ne!(one, other);
    // A client finds each through its file.
    let listener: Arc<dyn LocalListener> = Arc::from(a);
    let server = Arc::clone(&listener);
    let accepted = thread::spawn(move || server.accept().map(|s| s.peer().pid));
    let client = p
        .connect(&service, PeerPolicy::SameUser, T)
        .expect("connects");
    assert_eq!(client.peer().pid, std::process::id());
    assert_eq!(accepted.join().expect("thread"), Ok(std::process::id()));
    listener.close().expect("closes");
    drop(listener);
    // A file that names no pipe is nothing to connect to.
    std::fs::write(service.run_dir().join("service.pipe"), "not a secret\n").expect("written");
    assert!(matches!(
        p.connect(&service, PeerPolicy::SameUser, T),
        Err(PlatformError::EndpointNotFound)
    ));
    let again = p.bind(&service, PeerPolicy::SameUser).expect("binds again");
    assert_ne!(read("service.pipe"), one, "a new secret");
    again.close().expect("closes");
    b.close().expect("closes");
}
