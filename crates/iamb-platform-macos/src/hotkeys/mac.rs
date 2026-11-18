//! The Carbon side of the hotkeys, and [`Hotkeys`] for [`Platform`]. See the parent module
//! for the design; every Carbon call here takes a [`MainThreadMarker`].

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::fmt;
use std::panic::catch_unwind;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use iamb_platform::{Chord, HotkeyId, Hotkeys, PlatformError, Result};
use objc2::MainThreadMarker;

use super::table::{Backend, Combo, Table};
use crate::Platform;
use crate::listener::main_thread;
use crate::listener::sys::{self, EventHotKeyID, EventHotKeyRef, EventTypeSpec, OSStatus};
use crate::listener::{Request, Shared};

/// How long `register` and `unregister` wait for the main thread.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the listener's `stop` waits for the main thread to release the chords.
const DETACH_TIMEOUT: Duration = Duration::from_millis(500);

/// The signature of Iamb's hotkey ids (`'iamb'`).
const SIGNATURE: u32 = u32::from_be_bytes(*b"iamb");

/// Where a press of one registered chord goes.
struct Route {
    listener: Weak<Shared>,
    id: HotkeyId,
}

/// The routes of every live registration of this process, by the number the system reports
/// a press with. One table for the process, because the Carbon event handler is one.
static ROUTES: Mutex<BTreeMap<u32, Route>> = Mutex::new(BTreeMap::new());
static NEXT_SLOT: AtomicU32 = AtomicU32::new(1);
static HANDLER_INSTALLED: AtomicBool = AtomicBool::new(false);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // The maps behind these mutexes change by one whole entry at a time.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A live registration: the system's reference and the number its presses carry.
pub(crate) struct CarbonHandle {
    hot_key: EventHotKeyRef,
    slot: u32,
}

// SAFETY: an `EventHotKeyRef` is an opaque token. This crate never dereferences it and only
// passes it to `UnregisterEventHotKey`, in `Carbon::unregister`, which runs on the main
// thread. Moving the token between threads inside the table's mutex touches nothing.
unsafe impl Send for CarbonHandle {}

/// The system hotkey API, usable on the main thread only.
struct Carbon {
    mtm: MainThreadMarker,
    /// The listener whose sink gets the presses.
    listener: Weak<Shared>,
}

/// Installs the process's one handler for hotkey presses, the first time it is needed.
fn install_handler(_mtm: MainThreadMarker) -> Result<()> {
    if HANDLER_INSTALLED.load(Ordering::Acquire) {
        return Ok(());
    }
    let pressed = EventTypeSpec {
        eventClass: sys::EVENT_CLASS_KEYBOARD,
        eventKind: sys::EVENT_HOT_KEY_PRESSED,
    };
    // SAFETY: called on the main thread, which the marker proves. `hot_key_pressed` is a
    // function that stays valid for the life of the process and uses no user data; `pressed`
    // is one valid entry, copied by the call; a null `outRef` is allowed and means the handler
    // is never removed.
    let status = unsafe {
        sys::InstallEventHandler(
            sys::GetApplicationEventTarget(),
            hot_key_pressed,
            1,
            &raw const pressed,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if status != sys::NO_ERR {
        return Err(PlatformError::os(i64::from(status), "InstallEventHandler"));
    }
    HANDLER_INSTALLED.store(true, Ordering::Release);
    Ok(())
}

/// The Carbon handler for hotkey presses. The application event loop calls it on the main
/// thread; it only looks up the route and queues the press for the listener thread.
unsafe extern "C" fn hot_key_pressed(
    _call: sys::EventHandlerCallRef,
    event: sys::EventRef,
    _user_data: *mut c_void,
) -> OSStatus {
    // Unwinding must not reach the system frames below this handler.
    let handled = catch_unwind(|| {
        let mut pressed = EventHotKeyID {
            signature: 0,
            id: 0,
        };
        // SAFETY: `event` is the event the system is dispatching, valid for this call;
        // `pressed` is a buffer of exactly the size given, for the type asked for.
        let status = unsafe {
            sys::GetEventParameter(
                event,
                sys::EVENT_PARAM_DIRECT_OBJECT,
                sys::TYPE_EVENT_HOT_KEY_ID,
                ptr::null_mut(),
                size_of::<EventHotKeyID>(),
                ptr::null_mut(),
                (&raw mut pressed).cast(),
            )
        };
        if status != sys::NO_ERR || pressed.signature != SIGNATURE {
            return false;
        }
        let Some((listener, id)) = lock(&ROUTES)
            .get(&pressed.id)
            .map(|route| (route.listener.clone(), route.id))
        else {
            return false;
        };
        if let Some(listener) = listener.upgrade() {
            listener.send(Request::Hotkey(id));
        }
        true
    });
    if matches!(handled, Ok(true)) {
        sys::NO_ERR
    } else {
        sys::EVENT_NOT_HANDLED_ERR
    }
}

impl Backend for Carbon {
    type Handle = CarbonHandle;

    fn register(&mut self, id: HotkeyId, combo: Combo) -> Result<CarbonHandle> {
        install_handler(self.mtm)?;
        let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
        // The route first: the registration can fire as soon as it exists.
        lock(&ROUTES).insert(
            slot,
            Route {
                listener: self.listener.clone(),
                id,
            },
        );
        let mut hot_key: EventHotKeyRef = ptr::null_mut();
        // SAFETY: called on the main thread, which the marker in `self` proves. The id is
        // passed by value, the target is the application's, and `hot_key` is valid for the
        // write of the reference.
        let status = unsafe {
            sys::RegisterEventHotKey(
                combo.key_code,
                combo.modifiers,
                EventHotKeyID {
                    signature: SIGNATURE,
                    id: slot,
                },
                sys::GetApplicationEventTarget(),
                sys::EVENT_HOT_KEY_EXCLUSIVE,
                &raw mut hot_key,
            )
        };
        if status == sys::NO_ERR && !hot_key.is_null() {
            return Ok(CarbonHandle { hot_key, slot });
        }
        lock(&ROUTES).remove(&slot);
        Err(if status == sys::EVENT_HOT_KEY_EXISTS_ERR {
            PlatformError::HotkeyUnavailable
        } else {
            PlatformError::os(i64::from(status), "RegisterEventHotKey")
        })
    }

    fn unregister(&mut self, handle: CarbonHandle) {
        lock(&ROUTES).remove(&handle.slot);
        // SAFETY: called on the main thread, which the marker in `self` proves. `hot_key` is
        // a reference `RegisterEventHotKey` returned and that is released here, once: the
        // handle is consumed. A failure leaves nothing to do; the route is gone either way.
        let _ = unsafe { sys::UnregisterEventHotKey(handle.hot_key) };
    }
}

#[derive(Default)]
struct Inner {
    table: Table<CarbonHandle>,
    /// The running listener, whose sink gets the presses.
    listener: Weak<Shared>,
    /// True from the listener's start to its stop. The table becomes active a moment later,
    /// when the main thread has registered the wanted chords; in between, a registration
    /// must still go to the main thread, so that it is answered by the system and not merely
    /// noted.
    listening: bool,
}

/// The wanted chords and their live registrations, shared with the main thread.
#[derive(Default)]
pub(crate) struct State {
    inner: Arc<Mutex<Inner>>,
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("live", &lock(&self.inner).table.live_count())
            .finish_non_exhaustive()
    }
}

impl State {
    /// The listener started: presses go to `listener`, and every wanted chord is registered
    /// with the system as soon as the main thread gets to it.
    pub(crate) fn attach(&self, listener: Weak<Shared>) {
        lock(&self.inner).listening = true;
        let inner = Arc::clone(&self.inner);
        main_thread::post(move |mtm| {
            let mut inner = lock(&inner);
            inner.listener.clone_from(&listener);
            inner.table.activate(&mut Carbon { mtm, listener });
        });
    }

    /// The listener stops: every registration is released. Waits a moment for the main
    /// thread when there is something to release, so that the chords are free when `stop`
    /// returns; a main thread that does not answer releases them when it next runs.
    pub(crate) fn detach(&self) {
        let inner = Arc::clone(&self.inner);
        let release = move |mtm: MainThreadMarker| {
            let mut inner = lock(&inner);
            let listener = std::mem::take(&mut inner.listener);
            inner.table.deactivate(&mut Carbon { mtm, listener });
        };
        let live = {
            let mut inner = lock(&self.inner);
            inner.listening = false;
            inner.table.live_count()
        };
        if live == 0 {
            main_thread::post(release);
        } else {
            wait_then_leave_queued(release);
        }
    }
}

/// Runs `work` on the main thread, waiting at most [`DETACH_TIMEOUT`]; when the main thread
/// does not get to it in time, the work stays queued instead of being cancelled.
fn wait_then_leave_queued(work: impl FnOnce(MainThreadMarker) + Send + 'static) {
    if let Some(mtm) = MainThreadMarker::new() {
        main_thread::run_pending(mtm);
        work(mtm);
        return;
    }
    let (done, is_done) = std::sync::mpsc::sync_channel(1);
    main_thread::post(move |mtm| {
        work(mtm);
        let _ = done.try_send(());
    });
    let _ = is_done.recv_timeout(DETACH_TIMEOUT);
}

impl Hotkeys for Platform {
    fn register(&self, id: HotkeyId, chord: Chord) -> Result<()> {
        {
            let mut inner = lock(&self.hotkeys.inner);
            if inner.listening {
                // What is wrong with the chord itself shows without the system.
                inner.table.check(id, chord)?;
            } else if inner.table.try_record(id, chord)? {
                return Ok(());
            }
        }
        let inner = Arc::clone(&self.hotkeys.inner);
        main_thread::call(MAIN_THREAD_TIMEOUT, move |mtm| {
            let mut inner = lock(&inner);
            let listener = inner.listener.clone();
            inner
                .table
                .register(id, chord, &mut Carbon { mtm, listener })
        })
        .unwrap_or(Err(PlatformError::Timeout))
    }

    fn unregister(&self, id: HotkeyId) -> Result<()> {
        {
            let mut inner = lock(&self.hotkeys.inner);
            if !inner.listening && inner.table.try_forget(id) {
                return Ok(());
            }
        }
        let inner = Arc::clone(&self.hotkeys.inner);
        main_thread::call(MAIN_THREAD_TIMEOUT, move |mtm| {
            let mut inner = lock(&inner);
            let listener = inner.listener.clone();
            inner.table.unregister(id, &mut Carbon { mtm, listener });
        })
        .ok_or(PlatformError::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> Chord {
        Chord::parse(text).expect("a valid chord")
    }

    // With the listener stopped no system call is made, so these run anywhere, including
    // under the test harness, whose main thread serves no jobs.

    #[test]
    fn registrations_are_kept_while_the_listener_is_stopped() {
        let platform = Platform::new();
        assert_eq!(
            platform.register(HotkeyId(1), chord("Ctrl+Shift+Space")),
            Ok(())
        );
        assert_eq!(
            platform.register(HotkeyId(1), chord("Ctrl+Shift+Space")),
            Ok(())
        );
        assert_eq!(platform.register(HotkeyId(1), chord("Ctrl+Alt+K")), Ok(()));
        assert_eq!(
            platform.register(HotkeyId(2), chord("Ctrl+Alt+K")),
            Err(PlatformError::HotkeyUnavailable)
        );
        assert_eq!(platform.register(HotkeyId(2), chord("Meta+Alt+P")), Ok(()));
        assert_eq!(platform.unregister(HotkeyId(1)), Ok(()));
        assert_eq!(platform.unregister(HotkeyId(1)), Ok(()));
        assert_eq!(platform.unregister(HotkeyId(99)), Ok(()));
        // The chord is free again.
        assert_eq!(platform.register(HotkeyId(3), chord("Ctrl+Alt+K")), Ok(()));
        assert_eq!(format!("{:?}", platform.hotkeys), "State { live: 0, .. }");
    }

    #[test]
    fn a_key_a_mac_does_not_have_is_invalid() {
        let platform = Platform::new();
        assert!(matches!(
            platform.register(HotkeyId(1), chord("F24")),
            Err(PlatformError::Invalid { .. })
        ));
    }

    #[test]
    fn the_signature_and_event_constants_are_the_four_character_codes() {
        assert_eq!(SIGNATURE, 0x6961_6D62);
        assert_eq!(sys::EVENT_CLASS_KEYBOARD, 0x6B65_7962);
        assert_eq!(sys::EVENT_PARAM_DIRECT_OBJECT, 0x2D2D_2D2D);
        assert_eq!(sys::TYPE_EVENT_HOT_KEY_ID, 0x686B_6964);
        assert_eq!(size_of::<EventHotKeyID>(), 8);
        assert_eq!(size_of::<EventTypeSpec>(), 8);
    }
}
