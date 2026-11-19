//! Serving the service endpoint, also when its name cannot be had at start (the 2.8 review's
//! finding 12).
//!
//! Plain-text expansion needs neither the UI host nor the sync agent (architecture §3), so it
//! must not need the endpoint either. When the bind fails (on Windows another local user could
//! once create the pipe's name first; any system may refuse for a reason of its own), the
//! worker starts without it: the listener, the engine and the injector run as usual, and the
//! accept thread tries the bind again every [`ENDPOINT_RETRY`](super::ENDPOINT_RETRY) until it
//! succeeds or the worker stops. The state is reported where it can be seen:
//!
//! - the log says that the endpoint is not served and why (the error, never anything typed),
//!   repeats it once a minute while that lasts, and says when it is served;
//! - the diagnostic `endpoint_unavailable` goes into the ring, so the first client that
//!   connects once the bind succeeds can list it (`RecentDiagnostics`);
//! - `worker ready` names whether the endpoint was served at start.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use iamb_ipc::WireDiagnostic;
use iamb_platform::{EndpointName, LocalEndpoint, LocalListener, PeerPolicy, Result};

use super::{Shared, WakeCounters, ipc};

/// While the bind keeps failing, the log repeats that once per this many attempts (once a
/// minute with the default retry).
const LOG_EVERY: u32 = 12;

#[derive(Default)]
struct Slot {
    listener: Option<Arc<dyn LocalListener>>,
    stopping: bool,
}

/// The bound listener, once there is one, and the stop request the retrying thread waits on.
#[derive(Default)]
pub(crate) struct Served {
    slot: Mutex<Slot>,
    changed: Condvar,
}

impl Served {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Closes the listener if one is bound, and ends a thread that is still trying to bind.
    pub(crate) fn stop(&self) {
        let listener = {
            let mut slot = self.lock();
            slot.stopping = true;
            slot.listener.take()
        };
        if let Some(listener) = listener {
            let _ = listener.close();
        }
        self.changed.notify_all();
    }

    /// True while the endpoint is served.
    pub(crate) fn is_served(&self) -> bool {
        self.lock().listener.is_some()
    }

    /// Keeps `listener` unless the worker is stopping; false then.
    fn install(&self, listener: &Arc<dyn LocalListener>) -> bool {
        let mut slot = self.lock();
        if slot.stopping {
            return false;
        }
        slot.listener = Some(Arc::clone(listener));
        true
    }

    /// Waits `timeout`, or less when the worker stops; false when it stops.
    fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut slot = self.lock();
        while !slot.stopping {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            slot = self
                .changed
                .wait_timeout(slot, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        false
    }
}

/// What the accept thread needs to bind again.
pub(crate) struct Binder {
    pub endpoint: Arc<dyn LocalEndpoint>,
    pub name: EndpointName,
    pub policy: PeerPolicy,
    pub retry: Duration,
}

/// The accept thread: serves `first` when the bind at start succeeded, else tries again every
/// `binder.retry` until it succeeds (module docs), then accepts clients until the worker
/// stops.
pub(crate) fn run(
    shared: &Arc<Shared>,
    served: &Served,
    binder: &Binder,
    first: Result<Box<dyn LocalListener>>,
) {
    let mut attempt = first;
    let mut attempts: u32 = 1;
    let listener: Arc<dyn LocalListener> = loop {
        match attempt {
            Ok(listener) => break Arc::from(listener),
            Err(e) => {
                if attempts == 1 {
                    shared.logger.warn(format_args!(
                        "the service endpoint cannot be served: {e}; expansion goes on without \
                         it, and the endpoint is tried again every {} ms",
                        binder.retry.as_millis()
                    ));
                    shared.diagnostic(WireDiagnostic::code("endpoint_unavailable"));
                } else if attempts.is_multiple_of(LOG_EVERY) {
                    shared.logger.warn(format_args!(
                        "the service endpoint still cannot be served after {attempts} \
                         attempts: {e}"
                    ));
                }
            }
        }
        if !served.wait(binder.retry) {
            return;
        }
        WakeCounters::bump(&shared.wakes.accept);
        attempts = attempts.saturating_add(1);
        attempt = binder.endpoint.bind(&binder.name, binder.policy);
    };
    if !served.install(&listener) {
        let _ = listener.close();
        return;
    }
    if attempts > 1 {
        shared.logger.info(format_args!(
            "the service endpoint is served, after {attempts} attempts"
        ));
    }
    ipc::accept(shared, &listener);
}
