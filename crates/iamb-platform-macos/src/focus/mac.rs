//! [`FocusInspector`] for [`Platform`]: the inspection thread, the watcher and the system
//! calls put together. See the parent module for the behaviour.

use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use iamb_platform::{
    AppIdentity, FieldSecurity, FocusInspector, FocusSink, FocusToken, PlatformError,
    RESTORE_FOCUS_TIMEOUT, Result, UnknownReason,
};

use super::inspect::{Inspector, Probe};
use super::restore::{self, Ops};
use super::watcher::{self, Watching};
use super::{MAIN_RUN_LOOP_NOT_RUNNING, missing_for_watch, system};
use crate::Platform;
use crate::listener::{main_thread, start_gate};
use crate::permissions::SystemProbe;

/// How long `watch` waits for the main thread to register for the activation notification.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(2);

/// The focus state: the inspection thread, started with the first `secure_state`, and the
/// running watcher.
#[derive(Default)]
pub(crate) struct State {
    /// `None` inside when the thread could not be created.
    inspector: OnceLock<Option<Inspector>>,
    watching: Mutex<Option<Watching>>,
}

impl State {
    fn inspector(&self) -> Option<&Inspector> {
        self.inspector
            .get_or_init(|| Inspector::spawn(System).ok())
            .as_ref()
    }

    fn watching(&self) -> MutexGuard<'_, Option<Watching>> {
        // The slot is replaced whole; a panic cannot leave it torn.
        self.watching.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Two facts about the threads; nothing about what has the focus.
impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Focus")
            .field(
                "inspecting",
                &self.inspector.get().is_some_and(Option::is_some),
            )
            .field("watching", &self.watching().is_some())
            .finish()
    }
}

/// The real system, for the inspection and for giving the focus back.
struct System;

impl Probe for System {
    fn front(&self) -> Option<FocusToken> {
        system::front().map(|(_, token)| token)
    }

    fn secure_input(&self, deadline: Instant) -> Option<bool> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        system::secure_input(left)
    }

    fn focused_field(&self, pid: u32, deadline: Instant) -> FieldSecurity {
        system::focused_field(pid, deadline)
    }
}

impl Ops for System {
    fn front(&self) -> Option<FocusToken> {
        system::front().map(|(_, token)| token)
    }

    fn running(&self, pid: u32) -> bool {
        system::running(pid)
    }

    fn window_exists(&self, token: FocusToken) -> bool {
        system::window_exists(token)
    }

    fn activate(&self, pid: u32) -> bool {
        system::activate(pid)
    }

    fn pause(&self, duration: Duration) {
        system::pause(duration);
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}

impl FocusInspector for Platform {
    fn focused_app(&self) -> Result<AppIdentity> {
        system::frontmost().ok_or(PlatformError::NoFocus)
    }

    fn secure_state(&self, target: &FocusToken, timeout: Duration) -> FieldSecurity {
        match self.focus.inspector() {
            Some(inspector) => inspector.ask(*target, timeout),
            // No thread to ask on: fail closed.
            None => FieldSecurity::Unknown(UnknownReason::NotInspectable),
        }
    }

    fn focus_token(&self) -> Result<FocusToken> {
        match system::front() {
            Some((_, token)) if token.window() != 0 => Ok(token),
            _ => Err(PlatformError::NoFocus),
        }
    }

    fn restore_focus(&self, token: &FocusToken) -> Result<()> {
        restore::restore(&System, *token, RESTORE_FOCUS_TIMEOUT)
    }

    fn allow_foreground(&self, _pid: u32) -> Result<()> {
        // The form and the palette are non-activating panels (§4.5 step 3): nobody has to
        // be allowed anything.
        Ok(())
    }

    fn watch(&self, sink: Box<dyn FocusSink>) -> Result<()> {
        if self.focus.watching().is_some() {
            return Err(PlatformError::AlreadyRunning);
        }
        let missing = missing_for_watch(&SystemProbe);
        // The wait for the main thread happens before the lock is taken, as in the
        // listener's `start`.
        let observing = missing.is_some()
            || main_thread::call(MAIN_THREAD_TIMEOUT, watcher::observe_activations).is_some();
        let mut watching = self.focus.watching();
        start_gate(&mut watching, missing, || {
            if !observing {
                return Err(PlatformError::os(0, MAIN_RUN_LOOP_NOT_RUNNING));
            }
            watcher::start(sink)
        })
    }

    fn is_watching(&self) -> bool {
        self.focus
            .watching()
            .as_ref()
            .is_some_and(Watching::is_running)
    }

    fn unwatch(&self) -> Result<()> {
        let Some(watching) = self.focus.watching().take() else {
            return Ok(());
        };
        watching.stop()
    }
}
