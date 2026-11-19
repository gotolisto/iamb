//! One connection to the service endpoint with the role `ui`, usable from several threads.
//!
//! `iamb_ipc::Client` serves one call at a time and reads events only while a call waits or
//! when asked. The UI host needs both at once: commands from the front end arrive on Tauri's
//! threads while events (a form request, a status change) must reach the front end at any
//! moment. So this connection splits the stream ([`LocalStream::try_clone`]): a reader thread
//! reads every frame, hands each response to the call that waits for its id and each event to
//! the event callback; calls write their request under a lock and wait on a channel. The
//! frames, the handshake and the message types are `iamb-ipc`'s.
//!
//! A frame that does not decode is skipped: a newer service may send an event this build does
//! not know, which the protocol allows (architecture §6.1). A call whose response is lost that
//! way ends with its timeout.
//!
//! Nothing here logs or keeps a message body (architecture §8 rule 3).

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use iamb_ipc::{
    ErrorBody, EventBody, FrameError, Hello, IpcError, PROTOCOL_VERSION, Request, RequestBody,
    Response, ResponseBody, Role, Wire, read_frame, write_frame,
};
use iamb_platform::{EndpointName, LocalEndpoint, LocalStream, PeerPolicy};

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// No connection, or it ended before the response came.
    Disconnected,
    /// No response within the call's timeout.
    Timeout,
    /// The service answered with an error.
    Remote(ErrorBody),
}

impl CallError {
    /// A stable code: `disconnected`, `timeout`, or the service's error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Timeout => "timeout",
            Self::Remote(e) => e.code.as_str(),
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => f.write_str("the service is not connected"),
            Self::Timeout => f.write_str("the service did not answer in time"),
            Self::Remote(e) => write!(f, "{} ({})", e.message, e.code.as_str()),
        }
    }
}

impl std::error::Error for CallError {}

/// Why a connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Closed {
    /// [`Connection::close`] was called, or the service closed the connection cleanly.
    Eof,
    /// The stream failed or broke the protocol.
    Failed(String),
}

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eof => f.write_str("closed"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}

type Waiter = SyncSender<Result<ResponseBody, ErrorBody>>;

struct Pending {
    open: bool,
    waiters: HashMap<u64, Waiter>,
}

/// A connection after its handshake. See the [module docs](self).
pub struct Connection {
    writer: Mutex<Box<dyn LocalStream>>,
    /// A handle for [`close`](Self::close) that a blocked write does not hold.
    closer: Mutex<Box<dyn LocalStream>>,
    pending: Mutex<Pending>,
    next_id: AtomicU64,
    server_version: String,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("server_version", &self.server_version)
            .finish_non_exhaustive()
    }
}

/// Called with each event, on the reader thread.
pub type OnEvent = Box<dyn FnMut(EventBody) + Send>;

/// Called once when the connection ends, on the reader thread.
pub type OnClosed = Box<dyn FnOnce(Closed) + Send>;

impl Connection {
    /// Connects to `name`, says hello as `ui` and starts the reader thread.
    ///
    /// # Errors
    ///
    /// [`IpcError::Platform`] (`EndpointNotFound` while the service is not running,
    /// `PeerRejected`, `Timeout`), [`IpcError::Remote`] when the service refuses the hello
    /// (another protocol version), frame and decode errors.
    pub fn open(
        endpoint: &dyn LocalEndpoint,
        name: &EndpointName,
        policy: PeerPolicy,
        client_version: &str,
        timeout: Duration,
        on_event: OnEvent,
        on_closed: OnClosed,
    ) -> Result<Arc<Self>, IpcError> {
        let mut stream = endpoint.connect(name, policy, timeout)?;
        let server_version = handshake(&mut stream, client_version, timeout)?;
        let reader = stream.try_clone()?;
        let closer = stream.try_clone()?;
        // Applies to every clone: the reader waits for frames for ever.
        reader.set_read_timeout(None)?;
        let conn = Arc::new(Self {
            writer: Mutex::new(stream),
            closer: Mutex::new(closer),
            pending: Mutex::new(Pending {
                open: true,
                waiters: HashMap::new(),
            }),
            next_id: AtomicU64::new(1),
            server_version,
        });
        let theirs = Arc::clone(&conn);
        thread::Builder::new()
            .name("iamb-bridge-reader".to_owned())
            .spawn(move || theirs.read_loop(reader, on_event, on_closed))
            .map_err(|_| IpcError::Protocol("cannot start the reader thread"))?;
        Ok(conn)
    }

    /// The service's build version, from its welcome.
    #[must_use]
    pub fn server_version(&self) -> &str {
        &self.server_version
    }

    /// Whether the connection is still open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.lock_pending().open
    }

    fn lock_pending(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends `body` and waits up to `timeout` for its response.
    ///
    /// # Errors
    ///
    /// [`CallError`].
    pub fn call(&self, body: RequestBody, timeout: Duration) -> Result<ResponseBody, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = sync_channel(1);
        {
            let mut pending = self.lock_pending();
            if !pending.open {
                return Err(CallError::Disconnected);
            }
            pending.waiters.insert(id, tx);
        }
        let payload = match (Wire::Request(Request { id, body })).encode() {
            Ok(p) => p,
            Err(e) => {
                self.lock_pending().waiters.remove(&id);
                return Err(CallError::Remote(e));
            }
        };
        let written = {
            let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
            write_frame(&mut *writer, &payload)
        };
        if written.is_err() {
            self.lock_pending().waiters.remove(&id);
            return Err(CallError::Disconnected);
        }
        match rx.recv_timeout(timeout) {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) => Err(CallError::Remote(e)),
            Err(RecvTimeoutError::Timeout) => {
                self.lock_pending().waiters.remove(&id);
                Err(CallError::Timeout)
            }
            Err(RecvTimeoutError::Disconnected) => Err(CallError::Disconnected),
        }
    }

    /// Closes the connection: the reader thread ends and waiting calls fail with
    /// [`CallError::Disconnected`].
    pub fn close(&self) {
        let closer = self.closer.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = closer.shutdown();
    }

    fn deliver(&self, response: Response) {
        // Id 0: the service could not read a request far enough to know its id; the call it
        // belonged to ends with its timeout.
        let waiter = self.lock_pending().waiters.remove(&response.id);
        if let Some(waiter) = waiter {
            let _ = waiter.send(response.into_result());
        }
    }

    fn read_loop(
        &self,
        mut reader: Box<dyn LocalStream>,
        mut on_event: OnEvent,
        on_closed: OnClosed,
    ) {
        let closed = loop {
            match read_frame(&mut reader) {
                Ok(payload) => match Wire::decode(&payload) {
                    Ok(Wire::Response(response)) => self.deliver(response),
                    Ok(Wire::Event(event)) => on_event(event),
                    Ok(other) => {
                        break Closed::Failed(format!("unexpected {} frame", other.name()));
                    }
                    // An event or response this build cannot read (module docs).
                    Err(_) => {}
                },
                Err(FrameError::Eof) => break Closed::Eof,
                Err(e) => break Closed::Failed(e.to_string()),
            }
        };
        let _ = reader.shutdown();
        {
            let mut pending = self.lock_pending();
            pending.open = false;
            // Dropping the senders wakes every waiting call with `Disconnected`.
            pending.waiters.clear();
        }
        on_closed(closed);
    }
}

/// Says hello as `ui` and returns the service's version.
fn handshake(
    stream: &mut Box<dyn LocalStream>,
    client_version: &str,
    timeout: Duration,
) -> Result<String, IpcError> {
    let hello = hello(client_version).encode().map_err(IpcError::Remote)?;
    write_frame(stream, &hello)?;
    stream.set_read_timeout(Some(timeout))?;
    let payload = read_frame(stream)?;
    match Wire::decode(&payload)? {
        Wire::Welcome(welcome) => Ok(welcome.server_version),
        Wire::Refused { error } => Err(IpcError::Remote(error)),
        _ => Err(IpcError::Protocol("the service did not answer the hello")),
    }
}

/// The bridge's first frame.
#[must_use]
pub fn hello(client_version: &str) -> Wire {
    Wire::Hello(Hello {
        protocol: PROTOCOL_VERSION,
        role: Role::Ui,
        client_version: client_version.to_owned(),
    })
}
