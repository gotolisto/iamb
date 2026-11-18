//! The system's keyboard layout, read on the main thread and published to the listener.
//!
//! # Translation without touching the application
//!
//! A keyboard layout's `uchr` data and `UCKeyTranslate` form a pure function: key, modifiers
//! and a dead-key state in, text and the next state out. [`UchrLayout`] holds a private copy
//! of the data, and the listener passes its own state variable
//! ([`Translator`](super::translate::Translator)), so translating a key in the tap callback
//! cannot disturb the dead-key state of the focused application (architecture §4.1). The
//! event's own text (`CGEventKeyboardGetUnicodeString`) and the `NSEvent` conversion are
//! deliberately not used: they go through the text input system that the application also
//! uses.
//!
//! # Reading the layout
//!
//! The Text Input Sources functions are main-thread only (`sys.rs`). [`SystemInputs`] is the
//! process-wide implementation of [`InputSources`]: every read happens in a job on the main
//! thread ([`main_thread`]), which copies the layout data into a
//! [`UchrLayout`] and publishes an [`InputSnapshot`] under a new generation number. The tap
//! callback only loads that number.
//!
//! A read is triggered
//!
//! - when a listener starts ([`SystemInputs::prime`]);
//! - by the system's notification that the selected input source changed, which is delivered
//!   on the main thread;
//! - by the listener's watchdog after keys or clicks, as a safety net for a notification that
//!   did not arrive.
//!
//! The snapshot is kept between listener restarts, so only the first start of a process has
//! to wait for the main thread.
//!
//! # Limits
//!
//! - A key typed in the instant between a layout switch and the main thread's read is
//!   translated with the previous layout.
//! - A layout without `uchr` data (none ships with current systems) gives no text: its keys
//!   are boundaries.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_core_foundation::{CFData, CFRetained, CFString, CFType};

use super::inputs::{InputSnapshot, InputSources};
use super::main_thread;
use super::sys;
use super::translate::{KeyLayout, KeyPress, MAX_UNITS};

/// The smallest `uchr` resource that can be real: the fixed header and one keyboard-type
/// entry.
const MIN_UCHR_LEN: usize = 40;

/// A private copy of a keyboard layout's `uchr` data.
pub(crate) struct UchrLayout {
    /// The data, in 64-bit words so that it is aligned for every field `UCKeyTranslate` reads.
    words: Box<[u64]>,
    /// The keyboard type to translate with when an event names none.
    default_keyboard_type: u32,
}

impl UchrLayout {
    /// Copies `data`; `None` when it is too short to be a layout.
    pub(crate) fn from_bytes(data: &[u8], default_keyboard_type: u32) -> Option<Self> {
        if data.len() < MIN_UCHR_LEN {
            return None;
        }
        let words = data
            .chunks(size_of::<u64>())
            .map(|chunk| {
                let mut word = [0_u8; size_of::<u64>()];
                for (to, from) in word.iter_mut().zip(chunk) {
                    *to = *from;
                }
                u64::from_ne_bytes(word)
            })
            .collect();
        Some(Self {
            words,
            default_keyboard_type,
        })
    }
}

impl KeyLayout for UchrLayout {
    fn translate(
        &self,
        press: &KeyPress,
        dead_state: &mut u32,
        out: &mut [u16; MAX_UNITS],
    ) -> Option<usize> {
        let keyboard_type = if press.keyboard_type == 0 {
            self.default_keyboard_type
        } else {
            press.keyboard_type
        };
        let mut len = 0_usize;
        // SAFETY: `words` is a complete `uchr` resource as the system handed it out, at an
        // address aligned for all its fields, and it lives as long as `self`.
        // `UCKeyTranslate` only reads it. `dead_state`, `len` and `out` are valid for writes,
        // and the function writes at most `maxStringLength` (`MAX_UNITS`) units to `out`.
        // The function is a pure function of its arguments and may run on any thread.
        let status = unsafe {
            sys::UCKeyTranslate(
                self.words.as_ptr().cast(),
                press.code,
                press.action.code(),
                press.modifiers,
                keyboard_type,
                0,
                dead_state,
                MAX_UNITS,
                &raw mut len,
                out.as_mut_ptr(),
            )
        };
        (status == sys::NO_ERR && len <= MAX_UNITS).then_some(len)
    }
}

/// A property of an input source as a Core Foundation object of type `T`; `None` when the
/// source has no such property or it is of another type.
fn property<T: objc2_core_foundation::ConcreteType>(
    _mtm: MainThreadMarker,
    source: &CFType,
    key: &CFString,
) -> Option<CFRetained<T>> {
    // SAFETY: both arguments are valid objects for the duration of the call, and the marker
    // proves this is the main thread, which the function requires.
    let value = unsafe { sys::TISGetInputSourceProperty(source, key) };
    // SAFETY: a non-null result is a Core Foundation object owned by the input source, which
    // `source` keeps alive for this function; it is retained before the borrow ends.
    let value = unsafe { value.cast::<CFType>().as_ref() }?;
    value.downcast_ref::<T>().map(T::retain)
}

/// The keyboard layout an input source carries, with the source's identifier.
fn layout_of(mtm: MainThreadMarker, source: &CFType) -> (Option<String>, Option<UchrLayout>) {
    // SAFETY: the framework initialises these constants when it is loaded; they never change.
    let (id_key, data_key) = unsafe {
        (
            sys::kTISPropertyInputSourceID,
            sys::kTISPropertyUnicodeKeyLayoutData,
        )
    };
    let id = property::<CFString>(mtm, source, id_key).map(|id| id.to_string());
    // SAFETY: no arguments; the marker proves this is the main thread.
    let keyboard_type = u32::from(unsafe { sys::LMGetKbdType() });
    let layout = property::<CFData>(mtm, source, data_key)
        .and_then(|data| UchrLayout::from_bytes(&data.to_vec(), keyboard_type));
    (id, layout)
}

/// What the main thread read: the layout in use, and whether an input method is selected.
struct Reading {
    layout_id: Option<String>,
    layout: Option<UchrLayout>,
    input_method: bool,
}

/// Takes ownership of an input source returned by a `TISCopy…` function.
///
/// # Safety
///
/// `source` must be null or a retained Core Foundation object whose reference the caller
/// gives up.
unsafe fn take_source(source: *mut CFType) -> Option<CFRetained<CFType>> {
    let source = std::ptr::NonNull::new(source)?;
    // SAFETY: non-null, and the caller hands over its retained reference.
    Some(unsafe { CFRetained::from_raw(source) })
}

fn read(mtm: MainThreadMarker, known_layout_id: Option<&str>) -> Reading {
    // SAFETY: no arguments; the marker proves this is the main thread. The result is null or
    // a retained object ("Copy" rule), which `take_source` takes over.
    let layout_source = unsafe { take_source(sys::TISCopyCurrentKeyboardLayoutInputSource()) };
    // SAFETY: as above.
    let selected = unsafe { take_source(sys::TISCopyCurrentKeyboardInputSource()) };

    // SAFETY: the framework initialises these constants when it is loaded; they never change.
    let (id_key, type_key, layout_type) = unsafe {
        (
            sys::kTISPropertyInputSourceID,
            sys::kTISPropertyInputSourceType,
            sys::kTISTypeKeyboardLayout,
        )
    };
    // Anything that is not a plain keyboard layout composes text of its own.
    let input_method = selected.as_deref().is_some_and(|selected| {
        property::<CFString>(mtm, selected, type_key).is_some_and(|kind| *kind != *layout_type)
    });

    let Some(source) = layout_source.as_deref() else {
        return Reading {
            layout_id: None,
            layout: None,
            input_method,
        };
    };
    let id = property::<CFString>(mtm, source, id_key).map(|id| id.to_string());
    if id.is_some() && id.as_deref() == known_layout_id {
        // The same layout as before: no need to copy its data again.
        return Reading {
            layout_id: id,
            layout: None,
            input_method,
        };
    }
    let (layout_id, layout) = layout_of(mtm, source);
    Reading {
        layout_id,
        layout,
        input_method,
    }
}

/// The published state and what it was read from.
#[derive(Default)]
struct Published {
    snapshot: Option<Arc<InputSnapshot>>,
    layout_id: Option<String>,
    layout: Option<Arc<dyn KeyLayout>>,
}

/// Something to wake when a new snapshot is published: a listener thread.
pub(crate) trait Wake: Send + Sync {
    /// Called on the main thread after a new snapshot was published.
    fn inputs_changed(&self);
}

/// The process-wide source of the keyboard layout and input-method state.
pub(crate) struct SystemInputs {
    published: Mutex<Published>,
    arrived: Condvar,
    generation: AtomicU64,
    refresh_queued: AtomicBool,
    observing: AtomicBool,
    listeners: Mutex<Vec<Weak<dyn Wake>>>,
}

static SYSTEM: SystemInputs = SystemInputs {
    published: Mutex::new(Published {
        snapshot: None,
        layout_id: None,
        layout: None,
    }),
    arrived: Condvar::new(),
    generation: AtomicU64::new(0),
    refresh_queued: AtomicBool::new(false),
    observing: AtomicBool::new(false),
    listeners: Mutex::new(Vec::new()),
};

/// The system's input sources.
pub(crate) fn system() -> &'static SystemInputs {
    &SYSTEM
}

/// The system's input sources as the listener core takes them.
pub(crate) fn system_sources() -> Arc<dyn InputSources> {
    struct System;

    impl InputSources for System {
        fn generation(&self) -> u64 {
            system().generation()
        }

        fn current(&self) -> Option<Arc<InputSnapshot>> {
            system().current()
        }

        fn request_refresh(&self) {
            system().request_refresh();
        }
    }

    Arc::new(System)
}

impl SystemInputs {
    fn published(&self) -> MutexGuard<'_, Published> {
        // Every writer replaces whole values; a panic cannot leave the state half-written.
        self.published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes sure a snapshot exists before a listener starts, waiting at most `timeout` for
    /// the main thread. Returns whether one exists.
    ///
    /// On the main thread the layout is read at once. From another thread with a snapshot
    /// already published this only asks for a refresh and returns. `false` means the main
    /// thread did not serve the request: it is not running its run loop.
    pub(crate) fn prime(&'static self, timeout: Duration) -> bool {
        if let Some(mtm) = MainThreadMarker::new() {
            self.refresh(mtm);
            return true;
        }
        self.request_refresh();
        let deadline = Instant::now() + timeout;
        let mut published = self.published();
        while published.snapshot.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            published = self
                .arrived
                .wait_timeout(published, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Registers something to wake when a new snapshot is published. Entries whose owner is
    /// gone are dropped.
    pub(crate) fn subscribe(&self, listener: Weak<dyn Wake>) {
        let mut listeners = self
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        listeners.retain(|known| known.strong_count() > 0);
        listeners.push(listener);
    }

    /// Reads the current input source and publishes a snapshot when it differs from the last
    /// one. Main thread only, which the marker proves.
    pub(crate) fn refresh(&'static self, mtm: MainThreadMarker) {
        self.observe_changes(mtm);
        let known_id = self.published().layout_id.clone();
        let reading = read(mtm, known_id.as_deref());

        let mut published = self.published();
        // An input source without an identifier cannot be told from the one before it, unless
        // neither has a layout.
        let same_layout = reading.layout_id == published.layout_id
            && (reading.layout_id.is_some()
                || (reading.layout.is_none() && published.layout.is_none()));
        let layout = if same_layout {
            published.layout.clone()
        } else {
            reading
                .layout
                .map(|layout| Arc::new(layout) as Arc<dyn KeyLayout>)
        };
        let unchanged = same_layout
            && published
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.input_method == reading.input_method);
        if unchanged {
            return;
        }
        published.layout_id = reading.layout_id;
        published.layout.clone_from(&layout);
        published.snapshot = Some(Arc::new(InputSnapshot {
            layout,
            input_method: reading.input_method,
        }));
        self.generation.fetch_add(1, Ordering::Release);
        drop(published);
        self.arrived.notify_all();

        let listeners: Vec<Weak<dyn Wake>> = self
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for listener in listeners.iter().filter_map(Weak::upgrade) {
            listener.inputs_changed();
        }
    }

    /// Asks the system, once per process, to say when the selected input source changes.
    fn observe_changes(&self, _mtm: MainThreadMarker) {
        if self.observing.swap(true, Ordering::AcqRel) {
            return;
        }
        // SAFETY: the centre is a process-wide object that is never released; the observer and
        // object are null, which the function accepts; `input_source_changed` is a function
        // that stays valid for the life of the process; the name is a constant the framework
        // initialised when it was loaded.
        unsafe {
            sys::CFNotificationCenterAddObserver(
                sys::CFNotificationCenterGetDistributedCenter(),
                std::ptr::null(),
                input_source_changed,
                sys::kTISNotifySelectedKeyboardInputSourceChanged,
                std::ptr::null(),
                sys::NOTIFICATION_DELIVER_IMMEDIATELY,
            );
        }
    }
}

/// The system's notice that the selected input source changed. Distributed notifications
/// arrive on the main thread; should that ever not hold, the read is handed to it.
unsafe extern "C" fn input_source_changed(
    _center: *mut c_void,
    _observer: *mut c_void,
    _name: *const c_void,
    _object: *const c_void,
    _user_info: *const c_void,
) {
    // Unwinding must not reach the Core Foundation frames below this callback.
    let _ = std::panic::catch_unwind(|| match MainThreadMarker::new() {
        Some(mtm) => system().refresh(mtm),
        None => system().request_refresh(),
    });
}

impl InputSources for SystemInputs {
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn current(&self) -> Option<Arc<InputSnapshot>> {
        self.published().snapshot.clone()
    }

    fn request_refresh(&self) {
        if self.refresh_queued.swap(true, Ordering::AcqRel) {
            // One is on its way already.
            return;
        }
        main_thread::post(|mtm| {
            system().refresh_queued.store(false, Ordering::Release);
            system().refresh(mtm);
        });
    }
}

/// Reading layouts by identifier, for the translation tests.
#[cfg(test)]
pub(crate) mod by_id {
    use objc2_core_foundation::{CFArray, CFDictionary};

    use super::*;

    /// Loads the layout of the installed input source `id` (such as
    /// `com.apple.keylayout.German`), whether or not the user has enabled it, without
    /// selecting it.
    ///
    /// # Errors
    ///
    /// A sentence saying which step failed.
    pub(crate) fn load(mtm: MainThreadMarker, id: &str) -> Result<UchrLayout, String> {
        let wanted = CFString::from_str(id);
        // SAFETY: a constant the framework initialises when it is loaded.
        let id_key = unsafe { sys::kTISPropertyInputSourceID };
        let filter = CFDictionary::<CFString, CFString>::from_slices(&[id_key], &[&*wanted]);
        // SAFETY: the filter is a valid dictionary for the duration of the call, and the marker
        // proves this is the main thread. The result is null or a retained array.
        let list = unsafe { sys::TISCreateInputSourceList(Some(filter.as_opaque()), 1) };
        let list = std::ptr::NonNull::new(list)
            .ok_or_else(|| format!("no input source with the identifier {id} is installed"))?;
        // SAFETY: non-null and retained by the "Create" rule; the array holds input sources,
        // which are Core Foundation objects.
        let list: CFRetained<CFArray<CFType>> =
            unsafe { CFRetained::cast_unchecked(CFRetained::from_raw(list)) };
        let source = list
            .get(0)
            .ok_or_else(|| format!("the list of input sources named {id} is empty"))?;
        let (found_id, layout) = layout_of(mtm, &source);
        if found_id.as_deref() != Some(id) {
            return Err(format!("asked for {id} and was given another input source"));
        }
        layout.ok_or_else(|| format!("the input source {id} has no Unicode layout data"))
    }
}
