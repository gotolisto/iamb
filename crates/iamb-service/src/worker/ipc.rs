//! Serving the service endpoint: the accept thread, one reader per client (at most
//! [`MAX_CLIENTS`]) and, per client, an event pump that writes pushed events so that a client
//! that stops reading can never block the engine thread.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use iamb_ipc::{ConnectionWriter, EventBody, HELLO_TIMEOUT, MessageLog, Role, ServerConnection};
use iamb_platform::{LocalListener, LocalStream, PlatformError};

use super::{Shared, WakeCounters, handlers};

/// The most clients served at once (ADR-0009).
pub const MAX_CLIENTS: usize = 8;

/// Events queued for one client before further ones are dropped.
const EVENT_QUEUE: usize = 64;

struct Client {
    id: u64,
    role: Role,
    pid: u32,
    events: SyncSender<EventBody>,
    writer: ConnectionWriter,
}

/// The connected clients.
#[derive(Default)]
pub(crate) struct Clients {
    list: Mutex<Vec<Client>>,
    next: AtomicU64,
    /// Connections accepted and not finished, handshakes included (for the limit).
    active: AtomicUsize,
}

impl Clients {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Client>> {
        self.list.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Clients after their handshake.
    pub(crate) fn count(&self) -> usize {
        self.lock().len()
    }

    /// A UI host is connected.
    pub(crate) fn ui_connected(&self) -> bool {
        self.lock().iter().any(|c| c.role == Role::Ui)
    }

    /// The process id of the connected UI host, when the OS reported it.
    pub(crate) fn ui_pid(&self) -> Option<u32> {
        self.lock()
            .iter()
            .find(|c| c.role == Role::Ui && c.pid != 0)
            .map(|c| c.pid)
    }

    /// Queues `event` for every client whose role hears its group. Never blocks: a client
    /// whose queue is full misses the event.
    pub(crate) fn broadcast(&self, event: &EventBody) {
        for c in self.lock().iter() {
            if c.role.may_call(event.group()) {
                // A full queue drops the event; a gone client is removed by its reader.
                let _ = c.events.try_send(event.clone());
            }
        }
    }

    /// Closes every connection (worker shutdown).
    pub(crate) fn shutdown_all(&self) {
        for c in self.lock().iter() {
            c.writer.shutdown();
        }
    }

    fn add(&self, role: Role, pid: u32, writer: ConnectionWriter) -> (u64, Receiver<EventBody>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (events, rx) = sync_channel(EVENT_QUEUE);
        self.lock().push(Client {
            id,
            role,
            pid,
            events,
            writer,
        });
        (id, rx)
    }

    fn remove(&self, id: u64) {
        self.lock().retain(|c| c.id != id);
    }
}

/// The accept thread.
pub(crate) fn accept(shared: &Arc<Shared>, listener: &Arc<dyn LocalListener>) {
    loop {
        let stream = listener.accept();
        WakeCounters::bump(&shared.wakes.accept);
        match stream {
            Ok(stream) => {
                if shared.clients.active.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
                    shared.clients.active.fetch_sub(1, Ordering::SeqCst);
                    shared.logger.warn(format_args!(
                        "refused a client: {MAX_CLIENTS} are connected already"
                    ));
                    let _ = stream.shutdown();
                    continue;
                }
                let reader = Arc::clone(shared);
                let spawned =
                    thread::Builder::new()
                        .name("iamb-reader".to_owned())
                        .spawn(move || {
                            serve(&reader, stream);
                            reader.clients.active.fetch_sub(1, Ordering::SeqCst);
                        });
                if spawned.is_err() {
                    shared
                        .logger
                        .error(format_args!("cannot start a reader thread"));
                }
            }
            Err(PlatformError::Closed) => break,
            Err(PlatformError::PeerRejected(why)) => {
                shared.logger.warn(format_args!("rejected a client: {why}"));
            }
            Err(e) => {
                shared.logger.error(format_args!("accept failed: {e}"));
                // Do not spin on a persistent error.
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// The most characters of a client's version string that reach the log.
const VERSION_SHOWN: usize = 32;

/// A client's version string as the log shows it: client text, so at most [`VERSION_SHOWN`]
/// characters, each outside `A-Z a-z 0-9 . + - _` replaced by `?`, and `...` when it was
/// longer (the 2.8 review's finding 10: a hello may carry up to 8 MB, and a line break in it
/// would forge log lines).
fn shown_version(version: &str) -> String {
    let mut shown: String = version
        .chars()
        .take(VERSION_SHOWN)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_') {
                c
            } else {
                '?'
            }
        })
        .collect();
    if version.chars().nth(VERSION_SHOWN).is_some() {
        shown.push_str("...");
    }
    shown
}

fn serve(shared: &Arc<Shared>, stream: Box<dyn LocalStream>) {
    let log: Arc<dyn MessageLog> = Arc::clone(&shared.logger) as Arc<dyn MessageLog>;
    // The client is registered before its welcome is written: a client that has its welcome
    // is counted already (a UI host that connects just before a form is shown gets the form).
    // Events queued before the welcome wait in the channel until the pump starts.
    let mut registered = None;
    // The role must be one the client's executable may take (finding 10, `iamb_ipc::roles`).
    let accepted = ServerConnection::accept_for(
        stream,
        crate::VERSION,
        HELLO_TIMEOUT,
        log,
        shared.policy,
        |c| {
            registered = Some(shared.clients.add(c.role(), c.peer().pid, c.writer()));
            Ok(())
        },
    );
    let conn = match accepted {
        Ok(conn) => conn,
        Err(e) => {
            if let Some((id, _)) = registered {
                shared.clients.remove(id);
            }
            shared.logger.warn(format_args!("handshake failed: {e}"));
            return;
        }
    };
    let Some((id, events)) = registered else {
        unreachable!("a welcome is sent only after the client is registered");
    };
    let role = conn.role();
    let pid = conn.peer().pid;
    let writer = conn.writer();
    shared.logger.info(format_args!(
        "client {id} connected: role {role}, pid {pid}, version {}",
        shown_version(conn.client_version())
    ));
    let pump = {
        let shared = Arc::clone(shared);
        let writer = writer.clone();
        thread::Builder::new()
            .name("iamb-events".to_owned())
            .spawn(move || {
                for event in events {
                    WakeCounters::bump(&shared.wakes.clients);
                    if writer.push(event).is_err() {
                        break;
                    }
                }
            })
    };
    let wakes = Arc::clone(shared);
    let result = conn.serve(|role, body| {
        WakeCounters::bump(&wakes.wakes.clients);
        handlers::handle(shared, role, body)
    });
    shared.clients.remove(id);
    writer.shutdown();
    if let Ok(pump) = pump {
        let _ = pump.join();
    }
    match result {
        Ok(()) => shared.logger.info(format_args!("client {id} disconnected")),
        Err(e) => shared.logger.info(format_args!("client {id} dropped: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_version_is_shown_short_and_plain() {
        assert_eq!(shown_version("0.3.1-beta+42"), "0.3.1-beta+42");
        assert_eq!(shown_version(""), "");
        assert_eq!(
            shown_version("1.0\r2026-01-01T00:00:00.000Z ERROR forged"),
            "1.0?2026-01-01T00?00?00.000Z?ERR..."
        );
        let long = "V".repeat(300_000);
        assert_eq!(
            shown_version(&long),
            format!("{}...", "V".repeat(VERSION_SHOWN))
        );
        assert_eq!(shown_version("ünï\ncode"), "?n??code");
    }
}
