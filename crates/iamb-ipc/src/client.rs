//! The blocking client.
//!
//! ```no_run
//! use std::time::Duration;
//! use iamb_ipc::{Client, RequestBody, Role, native_endpoint};
//! use iamb_platform::{EndpointKind, EndpointName, PeerPolicy};
//!
//! let endpoint = native_endpoint();
//! let name = EndpointName::new(EndpointKind::Service, "/home/ada/.local/share/iamb/run");
//! let mut client = Client::connect(
//!     endpoint.as_ref(), &name, PeerPolicy::for_build(), Role::Cli, "0.0.0", Duration::from_secs(1),
//! )?;
//! let status = client.call(RequestBody::GetStatus {})?;
//! # Ok::<(), iamb_ipc::IpcError>(())
//! ```

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iamb_platform::{EndpointName, LocalEndpoint, LocalStream, PeerIdentity, PeerPolicy};

use crate::error::IpcError;
use crate::frame::{FrameError, read_frame, write_frame};
use crate::log::{Direction, MessageLog, MessageRecord, NoLog};
use crate::messages::{EventBody, RequestBody, ResponseBody};
use crate::protocol::{Hello, PROTOCOL_VERSION, Request, Role, Wire};

/// How long a call waits for its response unless [`Client::set_call_timeout`] says otherwise.
/// Imports of large files are the slowest requests.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_mins(1);

/// A connection to a server, after the handshake. One call at a time; events that arrive
/// while a call waits are queued for [`next_event`](Self::next_event).
pub struct Client {
    stream: Box<dyn LocalStream>,
    role: Role,
    server_version: String,
    next_id: u64,
    events: VecDeque<EventBody>,
    call_timeout: Duration,
    log: Arc<dyn MessageLog>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("role", &self.role)
            .field("server_version", &self.server_version)
            .field("queued_events", &self.events.len())
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Connects to `name` (the endpoint verifies the server first) and says hello as `role`.
    ///
    /// # Errors
    ///
    /// [`IpcError::Platform`] with `EndpointNotFound` when nothing listens (the command-line
    /// tool then opens the store itself, §6.3), `PeerRejected` or `Timeout`; the errors of
    /// [`handshake`](Self::handshake).
    pub fn connect(
        endpoint: &dyn LocalEndpoint,
        name: &EndpointName,
        policy: PeerPolicy,
        role: Role,
        client_version: &str,
        timeout: Duration,
    ) -> Result<Self, IpcError> {
        let stream = endpoint.connect(name, policy, timeout)?;
        Self::handshake(stream, role, client_version, timeout)
    }

    /// Says hello on a connected stream and waits up to `timeout` for the welcome.
    ///
    /// # Errors
    ///
    /// [`IpcError::Remote`] when the server refuses the hello (another protocol version);
    /// frame, decode and protocol errors.
    pub fn handshake(
        stream: Box<dyn LocalStream>,
        role: Role,
        client_version: &str,
        timeout: Duration,
    ) -> Result<Self, IpcError> {
        let mut client = Self {
            stream,
            role,
            server_version: String::new(),
            next_id: 1,
            events: VecDeque::new(),
            call_timeout: DEFAULT_CALL_TIMEOUT,
            log: Arc::new(NoLog),
        };
        client.send(&Wire::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            role,
            client_version: client_version.to_owned(),
        }))?;
        match client.receive(Some(timeout))? {
            Wire::Welcome(welcome) => {
                client.server_version = welcome.server_version;
                Ok(client)
            }
            Wire::Refused { error } => Err(IpcError::Remote(error)),
            _ => Err(IpcError::Protocol("the server did not answer the hello")),
        }
    }

    /// Sends the client's frame records to `log`.
    #[must_use]
    pub fn with_log(mut self, log: Arc<dyn MessageLog>) -> Self {
        self.log = log;
        self
    }

    /// How long [`call`](Self::call) waits for a response.
    pub const fn set_call_timeout(&mut self, timeout: Duration) {
        self.call_timeout = timeout;
    }

    /// The role this connection said hello as.
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    /// The server's build version, from its welcome.
    #[must_use]
    pub fn server_version(&self) -> &str {
        &self.server_version
    }

    /// The verified server process.
    #[must_use]
    pub fn peer(&self) -> &PeerIdentity {
        self.stream.peer()
    }

    fn send(&mut self, wire: &Wire) -> Result<(), IpcError> {
        let payload = wire.encode().map_err(IpcError::Remote)?;
        write_frame(&mut self.stream, &payload)?;
        self.log.record(&MessageRecord {
            direction: Direction::Out,
            name: wire.name(),
            size: payload.len(),
            duration: None,
            error: None,
        });
        Ok(())
    }

    fn receive(&mut self, timeout: Option<Duration>) -> Result<Wire, IpcError> {
        self.stream.set_read_timeout(timeout)?;
        let payload = read_frame(&mut self.stream)?;
        let wire = Wire::decode(&payload)?;
        self.log.record(&MessageRecord {
            direction: Direction::In,
            name: wire.name(),
            size: payload.len(),
            duration: None,
            error: None,
        });
        Ok(wire)
    }

    /// Sends a request and waits for its response.
    ///
    /// # Errors
    ///
    /// [`IpcError::Remote`] with the server's error; [`IpcError::Frame`] with `Timeout` when
    /// no response came within the call timeout; frame, decode and protocol errors.
    pub fn call(&mut self, body: RequestBody) -> Result<ResponseBody, IpcError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&Wire::Request(Request { id, body }))?;
        let deadline = Instant::now() + self.call_timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(IpcError::Frame(FrameError::Timeout));
            }
            match self.receive(Some(left))? {
                Wire::Response(response) if response.id == id => {
                    return response.into_result().map_err(IpcError::Remote);
                }
                Wire::Response(response) if response.id == 0 => {
                    // The server could not read the request far enough to know its id.
                    return response.into_result().map_err(IpcError::Remote);
                }
                Wire::Event(event) => self.events.push_back(event),
                _ => {
                    return Err(IpcError::Protocol(
                        "unexpected frame while waiting for a response",
                    ));
                }
            }
        }
    }

    /// The next event: a queued one, else waits up to `timeout` (`None`: for ever) for one.
    /// `Ok(None)` when the timeout passed.
    ///
    /// # Errors
    ///
    /// Frame (`Eof` when the server closed), decode and protocol errors.
    pub fn next_event(&mut self, timeout: Option<Duration>) -> Result<Option<EventBody>, IpcError> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        match self.receive(timeout) {
            Ok(Wire::Event(event)) => Ok(Some(event)),
            Ok(_) => Err(IpcError::Protocol(
                "unexpected frame while waiting for an event",
            )),
            Err(IpcError::Frame(FrameError::Timeout)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Closes the connection.
    pub fn close(self) {
        let _ = self.stream.shutdown();
    }
}
