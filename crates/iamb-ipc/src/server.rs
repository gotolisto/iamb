//! The blocking server side of one connection.
//!
//! The service runs one reader thread per client (architecture §3.1):
//!
//! ```text
//! let conn = ServerConnection::accept(stream, VERSION, HELLO_TIMEOUT, log)?;  // hello, welcome
//! let events = conn.writer();       // handed to whoever pushes events
//! conn.serve(|role, body| handle(role, body))?;   // until the client closes
//! ```
//!
//! [`ServerConnection::serve`] enforces the roles: a request whose group the client's role may
//! not call is answered with [`ErrorCode::NotPermitted`] and never reaches the handler. The
//! role itself must be one the client's executable may take ([`crate::roles`]): a hello that
//! states another is refused. Every
//! frame is reported to the [`MessageLog`] by name, size and duration; bodies never are.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use iamb_platform::{LocalStream, PeerIdentity, PeerPolicy};

use crate::error::IpcError;
use crate::frame::{FrameError, read_frame, write_frame};
use crate::log::{Direction, MessageLog, MessageRecord};
use crate::messages::{EventBody, RequestBody, ResponseBody};
use crate::protocol::{
    ErrorBody, ErrorCode, PROTOCOL_VERSION, Request, Response, Role, Welcome, Wire,
};
use crate::roles::check_role;

/// How long a server waits for a new connection's hello.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// The writing half of a connection, shared by the reader (responses) and whoever pushes
/// events. Each frame is written whole under a mutex.
#[derive(Clone)]
pub struct ConnectionWriter {
    stream: Arc<Mutex<Box<dyn LocalStream>>>,
    log: Arc<dyn MessageLog>,
}

impl std::fmt::Debug for ConnectionWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionWriter").finish_non_exhaustive()
    }
}

impl ConnectionWriter {
    fn write(&self, wire: &Wire, duration: Option<Duration>) -> Result<(), IpcError> {
        let payload = wire.encode().map_err(IpcError::Remote)?;
        {
            let mut stream = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
            write_frame(&mut *stream, &payload)?;
        }
        let error = match wire {
            Wire::Response(Response { error: Some(e), .. }) | Wire::Refused { error: e } => {
                Some(e.code.as_str())
            }
            _ => None,
        };
        self.log.record(&MessageRecord {
            direction: Direction::Out,
            name: wire.name(),
            size: payload.len(),
            duration,
            error,
        });
        Ok(())
    }

    /// Pushes an event.
    ///
    /// # Errors
    ///
    /// Frame errors (the client is gone).
    pub fn push(&self, event: EventBody) -> Result<(), IpcError> {
        self.write(&Wire::Event(event), None)
    }

    /// Answers request `id`.
    ///
    /// # Errors
    ///
    /// Frame errors (the client is gone).
    pub fn respond(
        &self,
        id: u64,
        outcome: Result<ResponseBody, ErrorBody>,
        took: Duration,
    ) -> Result<(), IpcError> {
        self.write(&Wire::Response(Response::new(id, outcome)), Some(took))
    }

    /// Closes the connection both ways; the reader's next read ends.
    pub fn shutdown(&self) {
        let stream = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = stream.shutdown();
    }
}

/// One accepted connection after a successful handshake.
pub struct ServerConnection {
    reader: Box<dyn LocalStream>,
    writer: ConnectionWriter,
    role: Role,
    client_version: String,
    log: Arc<dyn MessageLog>,
}

impl std::fmt::Debug for ServerConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerConnection")
            .field("role", &self.role)
            .field("client_version", &self.client_version)
            .finish_non_exhaustive()
    }
}

/// A request read from a client: valid, or answered already.
enum Incoming {
    /// Boxed: a request can be large (a whole snippet), the other variant is empty.
    Request(Box<Request>, usize),
    Answered,
}

impl ServerConnection {
    /// Reads the client's hello (waiting at most `hello_timeout`) and answers it: a welcome, or a
    /// refusal for another protocol major version or a first frame that is not a hello. A
    /// refused connection is closed.
    ///
    /// # Errors
    ///
    /// [`IpcError::Remote`] with the refusal that was sent; frame and decode errors.
    pub fn accept(
        stream: Box<dyn LocalStream>,
        server_version: &str,
        hello_timeout: Duration,
        log: Arc<dyn MessageLog>,
    ) -> Result<Self, IpcError> {
        Self::accept_with(stream, server_version, hello_timeout, log, |_| Ok(()))
    }

    /// Like [`ServerConnection::accept`], but runs `before_welcome` after a valid hello and
    /// before the welcome is written. A server registers the client there, so a client that has
    /// its welcome is known to the server already. If `before_welcome` fails, its error is
    /// returned and no welcome is sent.
    ///
    /// # Errors
    ///
    /// As [`ServerConnection::accept`], plus the error of `before_welcome`.
    pub fn accept_with(
        stream: Box<dyn LocalStream>,
        server_version: &str,
        hello_timeout: Duration,
        log: Arc<dyn MessageLog>,
        before_welcome: impl FnOnce(&Self) -> Result<(), IpcError>,
    ) -> Result<Self, IpcError> {
        Self::accept_for(
            stream,
            server_version,
            hello_timeout,
            log,
            PeerPolicy::SameUser,
            before_welcome,
        )
    }

    /// Like [`ServerConnection::accept_with`], and the role the hello states must be one the
    /// peer's executable may take under `policy`, the policy the endpoint verified the peer
    /// with ([`check_role`]); otherwise the hello is refused with
    /// [`ErrorCode::NotPermitted`]. [`accept`](Self::accept) and
    /// [`accept_with`](Self::accept_with) check under [`PeerPolicy::SameUser`]: Iamb's own
    /// clients are held to their roles, other programs (tests) are not.
    ///
    /// # Errors
    ///
    /// As [`ServerConnection::accept_with`].
    pub fn accept_for(
        stream: Box<dyn LocalStream>,
        server_version: &str,
        hello_timeout: Duration,
        log: Arc<dyn MessageLog>,
        policy: PeerPolicy,
        before_welcome: impl FnOnce(&Self) -> Result<(), IpcError>,
    ) -> Result<Self, IpcError> {
        let reader = stream.try_clone()?;
        let writer = ConnectionWriter {
            stream: Arc::new(Mutex::new(stream)),
            log: Arc::clone(&log),
        };
        let mut conn = Self {
            reader,
            writer,
            role: Role::Cli,
            client_version: String::new(),
            log,
        };
        conn.reader.set_read_timeout(Some(hello_timeout))?;
        let payload = read_frame(&mut conn.reader)?;
        let refusal = match Wire::decode(&payload) {
            Ok(Wire::Hello(hello)) => {
                conn.record_in("hello", payload.len());
                if hello.protocol == PROTOCOL_VERSION {
                    match check_role(hello.role, conn.reader.peer(), policy) {
                        Ok(()) => {
                            conn.role = hello.role;
                            conn.client_version = hello.client_version;
                            None
                        }
                        Err(why) => Some(ErrorBody::from(why)),
                    }
                } else {
                    Some(ErrorBody::new(
                        ErrorCode::UnsupportedProtocol,
                        format!(
                            "this service speaks protocol {PROTOCOL_VERSION}, the client protocol {}; update Iamb",
                            hello.protocol
                        ),
                    ))
                }
            }
            Ok(other) => {
                conn.record_in(other.name(), payload.len());
                Some(ErrorBody::new(
                    ErrorCode::BadHello,
                    "the first message must be a hello",
                ))
            }
            Err(_) => {
                conn.record_in("malformed", payload.len());
                Some(ErrorBody::new(
                    ErrorCode::BadHello,
                    "the first message must be a hello",
                ))
            }
        };
        if let Some(error) = refusal {
            let _ = conn.writer.write(
                &Wire::Refused {
                    error: error.clone(),
                },
                None,
            );
            conn.writer.shutdown();
            return Err(IpcError::Remote(error));
        }
        before_welcome(&conn)?;
        conn.writer.write(
            &Wire::Welcome(Welcome {
                protocol: PROTOCOL_VERSION,
                server_version: server_version.to_owned(),
            }),
            None,
        )?;
        conn.reader.set_read_timeout(None)?;
        Ok(conn)
    }

    fn record_in(&self, name: &'static str, size: usize) {
        self.log.record(&MessageRecord {
            direction: Direction::In,
            name,
            size,
            duration: None,
            error: None,
        });
    }

    /// The role from the hello.
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    /// The client's build version from the hello.
    #[must_use]
    pub fn client_version(&self) -> &str {
        &self.client_version
    }

    /// The verified client process.
    #[must_use]
    pub fn peer(&self) -> &PeerIdentity {
        self.reader.peer()
    }

    /// The writing half, for pushing events from another thread.
    #[must_use]
    pub fn writer(&self) -> ConnectionWriter {
        self.writer.clone()
    }

    /// Reads the next request. Frames that are not requests are answered with an error here.
    /// `Ok(None)` when the client closed the connection.
    fn next(&mut self) -> Result<Option<Incoming>, IpcError> {
        let payload = match read_frame(&mut self.reader) {
            Ok(payload) => payload,
            Err(FrameError::Eof) => return Ok(None),
            Err(FrameError::TooLarge(n)) => {
                // The stream is out of step: answer, then close.
                self.record_in("oversized", n);
                let _ = self.writer.write(
                    &Wire::Response(Response::new(
                        0,
                        Err(ErrorBody::new(
                            ErrorCode::BadRequest,
                            "the message is larger than 8 MB; pass bulk data as a file path",
                        )),
                    )),
                    None,
                );
                self.writer.shutdown();
                return Err(IpcError::Frame(FrameError::TooLarge(n)));
            }
            Err(e) => return Err(IpcError::Frame(e)),
        };
        let size = payload.len();
        match Wire::decode(&payload) {
            Ok(Wire::Request(request)) => Ok(Some(Incoming::Request(Box::new(request), size))),
            Ok(other) => {
                self.record_in(other.name(), size);
                self.writer.write(
                    &Wire::Response(Response::new(
                        0,
                        Err(ErrorBody::new(
                            ErrorCode::BadRequest,
                            "only requests may follow the hello",
                        )),
                    )),
                    None,
                )?;
                Ok(Some(Incoming::Answered))
            }
            Err(decode) => {
                self.record_in("malformed", size);
                // Recover the id if the frame is a request of an unknown type, so the client can
                // match the answer.
                let value: Option<serde_json::Value> = serde_json::from_slice(&payload).ok();
                let id = value
                    .as_ref()
                    .filter(|v| {
                        v.get("kind").and_then(serde_json::Value::as_str) == Some("request")
                    })
                    .and_then(|v| v.get("id"))
                    .and_then(serde_json::Value::as_u64);
                let error = match id {
                    Some(_) => ErrorBody::new(
                        ErrorCode::UnknownRequest,
                        "this service does not know that request; update Iamb",
                    ),
                    None => ErrorBody::new(ErrorCode::BadRequest, decode.to_string()),
                };
                self.writer.write(
                    &Wire::Response(Response::new(id.unwrap_or(0), Err(error))),
                    None,
                )?;
                Ok(Some(Incoming::Answered))
            }
        }
    }

    /// Answers requests with `handler` until the client closes the connection. Requests the
    /// role may not call are answered with [`ErrorCode::NotPermitted`] without calling the
    /// handler.
    ///
    /// # Errors
    ///
    /// Frame errors other than a clean close (the connection is then unusable).
    pub fn serve(
        mut self,
        mut handler: impl FnMut(Role, RequestBody) -> Result<ResponseBody, ErrorBody>,
    ) -> Result<(), IpcError> {
        while let Some(incoming) = self.next()? {
            let Incoming::Request(request, size) = incoming else {
                continue;
            };
            let started = Instant::now();
            self.record_in(request.body.name(), size);
            let outcome = if self.role.may_call(request.body.group()) {
                handler(self.role, request.body)
            } else {
                Err(ErrorBody::new(
                    ErrorCode::NotPermitted,
                    format!(
                        "the {} role may not call {}",
                        self.role,
                        request.body.name()
                    ),
                ))
            };
            self.writer
                .respond(request.id, outcome, started.elapsed())?;
        }
        self.writer.shutdown();
        Ok(())
    }
}
