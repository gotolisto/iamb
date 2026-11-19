//! The bridge between the UI host and the service (architecture §6.2, ADR-0017).
//!
//! One thread (`iamb-bridge`) keeps a [`Connection`] to the service endpoint with the role
//! `ui`. While the service is not there (not started yet, restarting after a crash) it retries
//! after [`BACKOFF`]: 200 ms, 500 ms, 1 s, then every 2 s, so that the UI is back within about
//! 2 s of the service's own restart (the supervisor restarts a worker in under 2 s). Each change
//! of the connection, and each status the service reports, goes to the [`BridgeSink`] as a
//! [`StatusView`]; the tray shows it and the front end hears it on `iamb:status`. Every other
//! event the UI needs goes to the sink as it arrives (see `contract` for which).
//!
//! The bridge is the only place in the UI host that speaks IPC. The Tauri commands call
//! [`Bridge::call`] and the typed helpers on Tauri's blocking pool; nothing here touches Tauri,
//! so the bridge runs in tests without a window.
//!
//! A UI host that crashes or is killed takes its connection with it; the service notices the
//! end of the stream, logs that the client disconnected, and carries on
//! (`tests/crash_isolation.rs`).

pub mod connection;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use iamb_core::model::Id;
use iamb_ipc::{EventBody, IpcError, RequestBody, ResponseBody, ServiceStatus};
use iamb_platform::{EndpointName, LocalEndpoint, PeerPolicy, PlatformError};

pub use connection::{CallError, Closed, Connection};

use crate::contract::{self, ConnectionState, Forward, SnippetDetail, SnippetSummary, StatusView};
use crate::log::Logger;

/// The delays before reconnect attempt 1, 2, 3 and every later one.
pub const BACKOFF: [Duration; 4] = [
    Duration::from_millis(200),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];

/// How long connecting and the handshake may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a call waits for its response. The bridge's requests are answered from the
/// worker's snapshot; an import (later tasks) may take longer and passes its own.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// How often a service that stays unreachable is mentioned in the log.
const UNREACHABLE_LOG_EVERY: Duration = Duration::from_mins(5);

/// The delay before reconnect attempt `failures` (1 for the first).
#[must_use]
pub fn backoff_delay(schedule: &[Duration], failures: usize) -> Duration {
    let i = failures
        .saturating_sub(1)
        .min(schedule.len().saturating_sub(1));
    schedule.get(i).copied().unwrap_or_default()
}

/// Where the bridge reports. Called on the bridge's threads; must not block for long.
pub trait BridgeSink: Send + Sync + 'static {
    /// The connection or the service's status changed.
    fn status(&self, view: &StatusView);
    /// A service event the UI needs ([`contract::forward`] said [`Forward::Ui`]).
    fn event(&self, event: &contract::UiEvent);
    /// The raw service event, for the parts of the UI host that act in Rust (the form and
    /// palette panels will take the focus token from here, tasks 3.5 and 3.6). Default: none.
    fn service_event(&self, _event: &EventBody) {}
}

/// How the bridge connects.
#[derive(Clone)]
pub struct BridgeConfig {
    /// The endpoint implementation (the OS's, or the mock's in tests).
    pub endpoint: Arc<dyn LocalEndpoint>,
    /// The service endpoint of the data root.
    pub name: EndpointName,
    /// Who the service must be ([`PeerPolicy::for_build`]).
    pub policy: PeerPolicy,
    /// This build's version, sent in the hello.
    pub client_version: String,
    /// Normally [`CONNECT_TIMEOUT`].
    pub connect_timeout: Duration,
    /// Normally [`CALL_TIMEOUT`].
    pub call_timeout: Duration,
    /// Normally [`BACKOFF`].
    pub backoff: Vec<Duration>,
    /// The UI host's log.
    pub logger: Arc<Logger>,
}

impl BridgeConfig {
    /// The defaults for the service endpoint `name` on `endpoint`.
    #[must_use]
    pub fn new(endpoint: Arc<dyn LocalEndpoint>, name: EndpointName, logger: Arc<Logger>) -> Self {
        Self {
            endpoint,
            name,
            policy: PeerPolicy::for_build(),
            client_version: crate::VERSION.to_owned(),
            connect_timeout: CONNECT_TIMEOUT,
            call_timeout: CALL_TIMEOUT,
            backoff: BACKOFF.to_vec(),
            logger,
        }
    }
}

struct State {
    connection: ConnectionState,
    service: Option<ServiceStatus>,
    conn: Option<Arc<Connection>>,
    /// The current connection ended (set by its reader thread).
    closed: bool,
    stop: bool,
}

struct Inner {
    cfg: BridgeConfig,
    sink: Arc<dyn BridgeSink>,
    state: Mutex<State>,
    wake: Condvar,
    stopped: AtomicBool,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn view(state: &State) -> StatusView {
        StatusView::new(state.connection, state.service.as_ref())
    }

    /// Sets the connection state and reports it when it changed.
    fn set_connection(&self, connection: ConnectionState) {
        let view = {
            let mut state = self.lock();
            if state.connection == connection {
                return;
            }
            state.connection = connection;
            if connection != ConnectionState::Connected {
                state.service = None;
            }
            Self::view(&state)
        };
        self.sink.status(&view);
    }

    /// Records a status the service reported and passes it on.
    fn set_service(&self, status: ServiceStatus) -> StatusView {
        let view = {
            let mut state = self.lock();
            state.service = Some(status);
            Self::view(&state)
        };
        self.sink.status(&view);
        view
    }

    fn on_event(&self, event: &EventBody) {
        self.sink.service_event(event);
        match contract::forward(event) {
            Forward::Status(status) => {
                self.set_service(status);
            }
            Forward::Ui(ui) => self.sink.event(&ui),
            Forward::Ignore => {}
        }
    }

    /// Waits up to `delay` unless the bridge is stopped; true when it was.
    fn sleep(&self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        let mut state = self.lock();
        while !state.stop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .wake
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Waits until the current connection ends or the bridge is stopped; true when stopped.
    fn wait_closed(&self) -> bool {
        let mut state = self.lock();
        while !state.stop && !state.closed {
            state = self
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.stop
    }

    fn connect(self: &Arc<Self>) -> Result<Arc<Connection>, IpcError> {
        self.lock().closed = false;
        let events = Arc::downgrade(self);
        let closes = Arc::downgrade(self);
        Connection::open(
            self.cfg.endpoint.as_ref(),
            &self.cfg.name,
            self.cfg.policy,
            &self.cfg.client_version,
            self.cfg.connect_timeout,
            Box::new(move |event| {
                if let Some(inner) = events.upgrade() {
                    inner.on_event(&event);
                }
            }),
            Box::new(move |why| {
                if let Some(inner) = closes.upgrade() {
                    if !inner.stopped.load(Ordering::SeqCst) {
                        inner
                            .cfg
                            .logger
                            .info(format_args!("connection to the service ended: {why}"));
                    }
                    inner.lock().closed = true;
                    inner.wake.notify_all();
                }
            }),
        )
    }

    fn run(self: &Arc<Self>) {
        let log = Arc::clone(&self.cfg.logger);
        let mut failures = 0_usize;
        let mut last_logged: Option<Instant> = None;
        loop {
            if self.lock().stop {
                break;
            }
            if failures == 0 {
                self.set_connection(ConnectionState::Connecting);
            }
            match self.connect() {
                Ok(conn) => {
                    last_logged = None;
                    log.info(format_args!(
                        "connected to the service, version {}",
                        conn.server_version()
                    ));
                    self.lock().conn = Some(Arc::clone(&conn));
                    match conn
                        .call(contract::get_status(), self.cfg.call_timeout)
                        .map(contract::status_from)
                    {
                        Ok(Ok(status)) => {
                            self.lock().service = Some(status);
                        }
                        Ok(Err(e)) => log.warn(format_args!(
                            "the service answered get_status with {}",
                            e.got
                        )),
                        Err(e) => log.warn(format_args!("get_status failed: {}", e.code())),
                    }
                    self.set_connection(ConnectionState::Connected);
                    let stop = self.wait_closed();
                    self.lock().conn = None;
                    if stop {
                        conn.close();
                        break;
                    }
                    self.set_connection(ConnectionState::Connecting);
                    // The service was there a moment ago: likely restarting.
                    failures = 1;
                }
                Err(e) => {
                    failures += 1;
                    if last_logged.is_none_or(|t| t.elapsed() >= UNREACHABLE_LOG_EVERY) {
                        last_logged = Some(Instant::now());
                        log.info(format_args!(
                            "the service is not reachable: {}",
                            describe(&e)
                        ));
                    }
                    self.set_connection(ConnectionState::Disconnected);
                }
            }
            if self.sleep(backoff_delay(&self.cfg.backoff, failures)) {
                break;
            }
        }
    }
}

/// A connection error for the log: names and codes only.
fn describe(e: &IpcError) -> String {
    match e {
        IpcError::Platform(PlatformError::EndpointNotFound) => "no service is listening".to_owned(),
        IpcError::Remote(body) => format!("refused ({})", body.code.as_str()),
        other => other.to_string(),
    }
}

/// The running bridge. Cheap to clone; [`stop`](Self::stop) ends it for every clone.
#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("connection", &self.inner.lock().connection)
            .finish_non_exhaustive()
    }
}

impl Bridge {
    /// Starts the bridge thread.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started.
    pub fn start(cfg: BridgeConfig, sink: Arc<dyn BridgeSink>) -> std::io::Result<Self> {
        let inner = Arc::new(Inner {
            cfg,
            sink,
            state: Mutex::new(State {
                connection: ConnectionState::Connecting,
                service: None,
                conn: None,
                closed: false,
                stop: false,
            }),
            wake: Condvar::new(),
            stopped: AtomicBool::new(false),
        });
        let runner = Arc::clone(&inner);
        let handle = thread::Builder::new()
            .name("iamb-bridge".to_owned())
            .spawn(move || runner.run())?;
        Ok(Self {
            inner,
            thread: Arc::new(Mutex::new(Some(handle))),
        })
    }

    /// The connection and the last status the service reported.
    #[must_use]
    pub fn status(&self) -> StatusView {
        Inner::view(&self.inner.lock())
    }

    /// Sends `body` on the current connection.
    ///
    /// # Errors
    ///
    /// [`CallError::Disconnected`] while there is none; the call's errors.
    pub fn call(&self, body: RequestBody) -> Result<ResponseBody, CallError> {
        let conn = self.inner.lock().conn.clone();
        match conn {
            Some(conn) => conn.call(body, self.inner.cfg.call_timeout),
            None => Err(CallError::Disconnected),
        }
    }

    /// Asks the service for its status and reports it.
    ///
    /// # Errors
    ///
    /// The call's errors, or an unexpected response.
    pub fn refresh_status(&self) -> Result<StatusView, contract::CommandError> {
        let status = contract::status_from(self.call(contract::get_status()).map_err(command)?)?;
        Ok(self.inner.set_service(status))
    }

    /// Turns global pause on or off; returns and reports the new status.
    ///
    /// # Errors
    ///
    /// The call's errors, or an unexpected response.
    pub fn set_paused(&self, paused: bool) -> Result<StatusView, contract::CommandError> {
        let status =
            contract::status_from(self.call(contract::set_paused(paused)).map_err(command)?)?;
        Ok(self.inner.set_service(status))
    }

    /// The snippets of `folder`, or of every folder.
    ///
    /// # Errors
    ///
    /// The call's errors, or an unexpected response.
    pub fn list_snippets(
        &self,
        folder: Option<Id>,
    ) -> Result<Vec<SnippetSummary>, contract::CommandError> {
        Ok(contract::snippets_from(
            self.call(contract::list_snippets(folder))
                .map_err(command)?,
        )?)
    }

    /// One snippet with its template.
    ///
    /// # Errors
    ///
    /// The call's errors (`not_found`), or an unexpected response.
    pub fn get_snippet(&self, id: Id) -> Result<SnippetDetail, contract::CommandError> {
        Ok(contract::snippet_from(
            self.call(contract::get_snippet(id)).map_err(command)?,
        )?)
    }

    /// Stops the bridge and closes its connection; waits for the thread.
    pub fn stop(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        let conn = {
            let mut state = self.inner.lock();
            state.stop = true;
            state.conn.clone()
        };
        if let Some(conn) = conn {
            conn.close();
        }
        self.inner.wake.notify_all();
        let handle = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

/// A call error as the front end receives it.
#[must_use]
pub fn command(e: CallError) -> contract::CommandError {
    match e {
        CallError::Remote(body) => body.into(),
        other => contract::CommandError::new(other.code(), other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_reaches_two_seconds_and_stays() {
        let delays: Vec<u128> = (1..=6)
            .map(|n| backoff_delay(&BACKOFF, n).as_millis())
            .collect();
        assert_eq!(delays, [200, 500, 1000, 2000, 2000, 2000]);
        assert_eq!(backoff_delay(&[], 3), Duration::ZERO);
    }
}
