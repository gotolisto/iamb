//! The mock's local endpoints: bound names, backlogs and in-memory byte pipes.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::endpoint::{EndpointName, LocalListener, LocalStream, PeerIdentity, PeerPolicy};
use crate::error::{PlatformError, Result};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One direction of a connection.
#[derive(Default)]
struct Pipe {
    state: Mutex<PipeState>,
    readable: Condvar,
}

#[derive(Default)]
struct PipeState {
    data: VecDeque<u8>,
    closed: bool,
}

impl Pipe {
    fn close(&self) {
        lock(&self.state).closed = true;
        self.readable.notify_all();
    }
}

/// One end of a connection, shared by a stream and its clones. Dropping the last handle
/// closes both directions, as closing a socket does.
struct Conn {
    incoming: Arc<Pipe>,
    outgoing: Arc<Pipe>,
    read_timeout: Mutex<Option<Duration>>,
}

impl Conn {
    fn close(&self) {
        self.incoming.close();
        self.outgoing.close();
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.close();
    }
}

/// An in-memory [`LocalStream`].
pub(super) struct MockStream {
    conn: Arc<Conn>,
    peer: PeerIdentity,
}

impl Read for MockStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let timeout = *lock(&self.conn.read_timeout);
        let pipe = &self.conn.incoming;
        let waiting = |s: &mut PipeState| s.data.is_empty() && !s.closed;
        let mut state = match timeout {
            None => pipe
                .readable
                .wait_while(lock(&pipe.state), waiting)
                .unwrap_or_else(PoisonError::into_inner),
            Some(limit) => {
                pipe.readable
                    .wait_timeout_while(lock(&pipe.state), limit, waiting)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
        };
        if state.data.is_empty() {
            return if state.closed {
                Ok(0)
            } else {
                Err(io::ErrorKind::TimedOut.into())
            };
        }
        let n = buf.len().min(state.data.len());
        for (slot, byte) in buf.iter_mut().zip(state.data.drain(..n)) {
            *slot = byte;
        }
        Ok(n)
    }
}

impl Write for MockStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let pipe = &self.conn.outgoing;
        let mut state = lock(&pipe.state);
        if state.closed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        state.data.extend(buf);
        drop(state);
        pipe.readable.notify_all();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl LocalStream for MockStream {
    fn peer(&self) -> &PeerIdentity {
        &self.peer
    }

    fn try_clone(&self) -> Result<Box<dyn LocalStream>> {
        Ok(Box::new(Self {
            conn: Arc::clone(&self.conn),
            peer: self.peer.clone(),
        }))
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        *lock(&self.conn.read_timeout) = timeout;
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        self.conn.close();
        Ok(())
    }
}

/// Connections waiting for `accept`.
struct Backlog {
    state: Mutex<BacklogState>,
    ready: Condvar,
    server: PeerIdentity,
    policy: PeerPolicy,
}

#[derive(Default)]
struct BacklogState {
    pending: VecDeque<MockStream>,
    closed: bool,
}

/// Every bound endpoint of one mock machine.
#[derive(Default)]
pub(super) struct Registry {
    bound: Mutex<HashMap<EndpointName, Arc<Backlog>>>,
}

impl Registry {
    pub(super) fn bind(
        self: &Arc<Self>,
        name: &EndpointName,
        policy: PeerPolicy,
        server: &PeerIdentity,
    ) -> Result<Box<dyn LocalListener>> {
        let mut bound = lock(&self.bound);
        if bound.contains_key(name) {
            return Err(PlatformError::EndpointInUse);
        }
        let backlog = Arc::new(Backlog {
            state: Mutex::default(),
            ready: Condvar::new(),
            server: server.clone(),
            policy,
        });
        bound.insert(name.clone(), Arc::clone(&backlog));
        Ok(Box::new(MockListener {
            registry: Arc::clone(self),
            name: name.clone(),
            backlog,
        }))
    }

    pub(super) fn connect(
        &self,
        name: &EndpointName,
        policy: PeerPolicy,
        client: &PeerIdentity,
    ) -> Result<Box<dyn LocalStream>> {
        let backlog = lock(&self.bound)
            .get(name)
            .cloned()
            .ok_or(PlatformError::EndpointNotFound)?;
        // The client verifies the server before anything is sent.
        policy
            .check(&client.user, &backlog.server)
            .map_err(PlatformError::PeerRejected)?;
        let to_server = Arc::new(Pipe::default());
        let to_client = Arc::new(Pipe::default());
        let server_end = MockStream {
            conn: Arc::new(Conn {
                incoming: Arc::clone(&to_server),
                outgoing: Arc::clone(&to_client),
                read_timeout: Mutex::new(None),
            }),
            peer: client.clone(),
        };
        let client_end = MockStream {
            conn: Arc::new(Conn {
                incoming: to_client,
                outgoing: to_server,
                read_timeout: Mutex::new(None),
            }),
            peer: backlog.server.clone(),
        };
        let mut state = lock(&backlog.state);
        if state.closed {
            return Err(PlatformError::EndpointNotFound);
        }
        state.pending.push_back(server_end);
        drop(state);
        backlog.ready.notify_all();
        Ok(Box::new(client_end))
    }
}

struct MockListener {
    registry: Arc<Registry>,
    name: EndpointName,
    backlog: Arc<Backlog>,
}

impl LocalListener for MockListener {
    fn accept(&self) -> Result<Box<dyn LocalStream>> {
        let mut state = self
            .backlog
            .ready
            .wait_while(lock(&self.backlog.state), |s| {
                s.pending.is_empty() && !s.closed
            })
            .unwrap_or_else(PoisonError::into_inner);
        let Some(stream) = state.pending.pop_front() else {
            return Err(PlatformError::Closed);
        };
        drop(state);
        // The server verifies each client; a rejected one is dropped, which closes it.
        self.backlog
            .policy
            .check(&self.backlog.server.user, stream.peer())
            .map_err(PlatformError::PeerRejected)?;
        Ok(Box::new(stream))
    }

    fn close(&self) -> Result<()> {
        let mut bound = lock(&self.registry.bound);
        if bound
            .get(&self.name)
            .is_some_and(|b| Arc::ptr_eq(b, &self.backlog))
        {
            bound.remove(&self.name);
        }
        drop(bound);
        let mut state = lock(&self.backlog.state);
        state.closed = true;
        // Connections nobody accepted are closed, so their clients read the end.
        state.pending.clear();
        drop(state);
        self.backlog.ready.notify_all();
        Ok(())
    }
}

impl Drop for MockListener {
    fn drop(&mut self) {
        // Closing the mock listener cannot fail.
        let _ = self.close();
    }
}
