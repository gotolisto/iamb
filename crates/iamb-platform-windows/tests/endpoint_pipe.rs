//! The Windows endpoint on a real named pipe: exclusive bind, not found, a round trip with the
//! peer identified in both directions, read timeouts, shutdown and close. Runs on Windows only
//! (CI's Windows leg).

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
        thread::spawn(move || listener.accept().map(|_| ()))
    };
    let client = p.connect(&n, PeerPolicy::SameUser, T).expect("connects");
    assert_eq!(accepted.join().expect("thread"), Ok(()));
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
