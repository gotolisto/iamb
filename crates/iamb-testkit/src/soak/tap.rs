//! The IPC frame tap: a [`LocalEndpoint`] that wraps another one and copies every byte each
//! accepted connection reads or writes to two files, `<prefix>-conn<N>-in.bin` and
//! `-out.bin`. Each file holds one direction of one connection contiguously, so a frame (a
//! 4-byte length and its JSON) is in it exactly as it crossed the endpoint and a canary split
//! across two reads is still found by the scan.
//!
//! The worker of the soak run serves its service endpoint through this wrapper. Nothing in
//! `iamb-ipc` or the service changes: the worker takes any `LocalEndpoint`
//! ([`iamb_service::worker::WorkerConfig::endpoint`]).

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iamb_platform::{
    EndpointName, LocalEndpoint, LocalListener, LocalStream, PeerIdentity, PeerPolicy, Result,
};

/// Wraps `inner`; taps go to `dir`, file names start with `prefix`.
pub struct TapEndpoint {
    inner: Arc<dyn LocalEndpoint>,
    dir: PathBuf,
    prefix: String,
    next: Arc<AtomicU64>,
}

impl TapEndpoint {
    /// A tap of `inner` writing into `dir` (created if missing).
    pub fn new(inner: Arc<dyn LocalEndpoint>, dir: &Path, prefix: &str) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            inner,
            dir: dir.to_path_buf(),
            prefix: prefix.to_owned(),
            next: Arc::default(),
        })
    }
}

impl LocalEndpoint for TapEndpoint {
    fn bind(&self, name: &EndpointName, policy: PeerPolicy) -> Result<Box<dyn LocalListener>> {
        Ok(Box::new(TapListener {
            inner: self.inner.bind(name, policy)?,
            dir: self.dir.clone(),
            prefix: self.prefix.clone(),
            next: Arc::clone(&self.next),
        }))
    }

    fn connect(
        &self,
        name: &EndpointName,
        policy: PeerPolicy,
        timeout: Duration,
    ) -> Result<Box<dyn LocalStream>> {
        // Outgoing connections are not the service endpoint's traffic.
        self.inner.connect(name, policy, timeout)
    }
}

struct TapListener {
    inner: Box<dyn LocalListener>,
    dir: PathBuf,
    prefix: String,
    next: Arc<AtomicU64>,
}

fn open(path: &Path) -> Option<Arc<Mutex<File>>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
        .map(|f| Arc::new(Mutex::new(f)))
}

impl LocalListener for TapListener {
    fn accept(&self) -> Result<Box<dyn LocalStream>> {
        let stream = self.inner.accept()?;
        let n = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let base = self.dir.join(format!("{}-conn{n}", self.prefix));
        let path = |dir: &str| PathBuf::from(format!("{}-{dir}.bin", base.display()));
        Ok(Box::new(TapStream {
            inner: stream,
            reads: open(&path("in")),
            writes: open(&path("out")),
        }))
    }

    fn close(&self) -> Result<()> {
        self.inner.close()
    }
}

/// One tapped connection. Its clones share the two files.
struct TapStream {
    inner: Box<dyn LocalStream>,
    reads: Option<Arc<Mutex<File>>>,
    writes: Option<Arc<Mutex<File>>>,
}

fn copy(to: Option<&Arc<Mutex<File>>>, bytes: &[u8]) {
    if let Some(file) = to
        && !bytes.is_empty()
    {
        // A tap that cannot write must not change what the connection does.
        let _ = file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write_all(bytes);
    }
}

impl Read for TapStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        copy(self.reads.as_ref(), &buf[..n]);
        Ok(n)
    }
}

impl Write for TapStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        copy(self.writes.as_ref(), &buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl LocalStream for TapStream {
    fn peer(&self) -> &PeerIdentity {
        self.inner.peer()
    }

    fn try_clone(&self) -> Result<Box<dyn LocalStream>> {
        Ok(Box::new(Self {
            inner: self.inner.try_clone()?,
            reads: self.reads.clone(),
            writes: self.writes.clone(),
        }))
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    fn shutdown(&self) -> Result<()> {
        self.inner.shutdown()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iamb_ipc::{Client, NoLog, RequestBody, ResponseBody, Role, ServerConnection};
    use iamb_platform::EndpointKind;
    use iamb_platform::mock::MockPlatform;

    /// Every frame of a connection, both ways, lands in the tap files byte for byte.
    #[test]
    fn the_tap_holds_every_frame_both_ways() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let os = MockPlatform::new();
        let tap = TapEndpoint::new(Arc::new(os.clone()), dir.path(), "t").expect("tap");
        let name = EndpointName::new(EndpointKind::Service, "/mock/run");
        let listener = tap.bind(&name, PeerPolicy::SameUser).expect("bind");
        let server = std::thread::spawn(move || {
            let stream = listener.accept().expect("accept");
            let conn =
                ServerConnection::accept(stream, "9.9.9", Duration::from_secs(5), Arc::new(NoLog))
                    .expect("hello");
            conn.serve(|_, _| Ok(ResponseBody::Done {})).expect("serve");
        });
        let stream = os
            .connect(&name, PeerPolicy::SameUser, Duration::from_secs(5))
            .expect("connect");
        let mut client =
            Client::handshake(stream, Role::Cli, "tap-test-client", Duration::from_secs(5))
                .expect("handshake");
        client.call(RequestBody::GetStatus {}).expect("call");
        client.close();
        server.join().expect("server");
        let read = std::fs::read(dir.path().join("t-conn1-in.bin")).expect("in");
        let written = std::fs::read(dir.path().join("t-conn1-out.bin")).expect("out");
        let contains = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
        assert!(contains(&read, b"tap-test-client"), "the hello was tapped");
        assert!(contains(&read, b"get_status"), "the request was tapped");
        assert!(contains(&written, b"9.9.9"), "the welcome was tapped");
        assert!(contains(&written, b"done"), "the response was tapped");
    }
}
