//! [`KeyListener`] for [`Platform`]: the start gate, the keyboard layout, the tap thread and
//! the hotkeys put together. See the parent module for the behaviour.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use iamb_platform::{EventSink, KeyListener, PlatformError, Result, SeqSource};
use objc2::MainThreadMarker;

use super::layout::{self, Wake};
use super::tap::{self, Parts, Running};
use super::{main_thread, start_gate};
use crate::Platform;
use crate::permissions::SystemProbe;
use crate::permissions::policy::missing_for_listener;

/// How long `start` waits for the main thread to read the keyboard layout the first time.
const LAYOUT_TIMEOUT: Duration = Duration::from_secs(2);

/// The error context when the main thread did not read the keyboard layout.
pub(crate) const MAIN_RUN_LOOP_NOT_RUNNING: &str =
    "reading the keyboard layout: the main thread is not running its run loop";

/// The listener's state: its sequence numbers, which outlive a restart, and the running tap.
#[derive(Default)]
pub(crate) struct State {
    seq: Arc<SeqSource>,
    running: Mutex<Option<Running>>,
}

impl State {
    fn running(&self) -> MutexGuard<'_, Option<Running>> {
        // The slot is replaced whole; a panic cannot leave it torn.
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The listener's health report: whether it runs, the state of its tap, and its counters.
/// Numbers and states only; nothing a key or a character could be read from.
impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut report = f.debug_struct("Listener");
        report.field("last_seq", &self.seq.last());
        match self.running().as_ref() {
            None => report.field("running", &false),
            Some(listener) => {
                let (tap, recovery) = listener.tap_health();
                report
                    .field("running", &!listener.has_exited())
                    .field("tap", &tap)
                    .field("recovery", &recovery)
                    .field("stats", &listener.stats())
            }
        };
        report.finish()
    }
}

impl KeyListener for Platform {
    fn start(&self, sink: Box<dyn EventSink>) -> Result<()> {
        let missing = missing_for_listener(&SystemProbe);
        // The wait for the main thread happens before the lock is taken, so that the main
        // thread can still ask this listener something while another thread starts it.
        let layout_known = missing.is_some() || layout::system().prime(LAYOUT_TIMEOUT);
        let mut running = self.listener.running();
        start_gate(&mut running, missing, || {
            if !layout_known {
                return Err(PlatformError::os(0, MAIN_RUN_LOOP_NOT_RUNNING));
            }
            let listener = tap::start(Parts {
                seq: Arc::clone(&self.listener.seq),
                sink,
                inputs: layout::system_sources(),
            })?;
            let shared = Arc::downgrade(listener.shared());
            layout::system().subscribe(shared.clone() as Weak<dyn Wake>);
            self.hotkeys.attach(shared);
            Ok(listener)
        })?;
        drop(running);
        if let Some(mtm) = MainThreadMarker::new() {
            // Started from the main thread: register the hotkeys now instead of when the run
            // loop next runs.
            main_thread::run_pending(mtm);
        }
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        let Some(listener) = self.listener.running().take() else {
            return Ok(());
        };
        self.hotkeys.detach();
        listener.stop()
    }

    fn self_test(&self, timeout: Duration) -> Result<()> {
        let running = self.listener.running();
        let Some(listener) = running.as_ref() else {
            return Err(PlatformError::NotRunning);
        };
        // A permission revoked while the listener runs: say so, rather than time out.
        if let Some(kind) = missing_for_listener(&SystemProbe) {
            return Err(PlatformError::PermissionDenied(kind));
        }
        listener.self_test(timeout)
    }
}
