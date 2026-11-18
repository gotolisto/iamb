//! The listener thread: the two low-level hooks, the hotkeys and the message loop that serves
//! them (architecture §3.1).
//!
//! The system calls a low-level hook procedure on the thread that installed the hook, while
//! that thread retrieves messages. So everything the procedures touch lives in this thread's
//! own storage, with no lock: the [`Pipeline`], the sink and the layout tables.
//!
//! # What a hook call does
//!
//! Read the event, hand it to the pipeline (which translates, takes a sequence number and
//! delivers), release the state, pass the call on to the hooks installed before this one,
//! return their verdict. The call is always passed on: the listener never swallows a key
//! (ADR-0010). The state is released *before* the call is passed on, because a later hook may
//! inject a key from inside its own procedure, and the system then calls this procedure again
//! while the first call is still waiting.
//!
//! If the state cannot be had (a call nested where none is expected) or the pipeline panics,
//! the event is still passed on, and the next event the listener does process is preceded by
//! `Overflow`.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::Instant;

use iamb_platform::{EventSink, ListenerEvent, PlatformError, Result};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::HC_ACTION;

use super::Shared;
use super::deadkey::{Composition, KeyQuery, Probe, Translation};
use super::keys::is_button_down;
use super::layouts::Layouts;
use super::os::{self, Hook, HookKind, Message};
use super::pipeline::{Foreground, Host, Pipeline, RawButton, RawKey};
use crate::hotkeys::Registration;

/// What a starting listener thread is given.
pub(super) struct Startup {
    /// Where events go.
    pub(super) sink: Box<dyn EventSink>,
    /// What the thread shares with the callers of the listener.
    pub(super) shared: Arc<Shared>,
    /// The hotkeys to register.
    pub(super) hotkeys: Vec<Registration>,
    /// Commands for the thread; a [`WM_WAKE`](os::WM_WAKE) message says that some wait.
    pub(super) commands: Receiver<Command>,
    /// Where the thread says that the hooks are installed, or why not.
    pub(super) ready: SyncSender<Result<Ready>>,
}

/// What a listener thread says once its hooks are installed.
pub(super) struct Ready {
    /// The thread's id, for posting messages to it.
    pub(super) thread_id: u32,
    /// The system ids of the hotkeys the system refused.
    pub(super) refused_hotkeys: Vec<i32>,
}

/// Something another thread asks the listener thread to do. Each carries the place for the
/// answer; a command whose sender has given up waiting is undone.
pub(super) enum Command {
    /// Register a hotkey.
    Register(Registration, SyncSender<Result<()>>),
    /// Unregister the hotkey with this system id.
    Unregister(i32, SyncSender<Result<()>>),
    /// Remove the hooks and install them again (after a failed self-test).
    Reinstall(SyncSender<Result<()>>),
}

/// The listener's state, owned by the listener thread.
struct Hooked {
    pipeline: Pipeline,
    system: System,
}

/// The pipeline's [`Host`]: the system calls and the sink.
struct System {
    sink: Box<dyn EventSink>,
    shared: Arc<Shared>,
    layouts: Layouts,
    /// A layout whose table is not read yet: read it once the hook call has returned.
    unread_layout: Option<usize>,
    /// The layout to translate with while the system names none: the last one it named.
    fallback_layout: usize,
    thread_id: u32,
    /// When the thread started; the pipeline's clock counts from here.
    started: Instant,
}

impl System {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn deliver(&mut self, event: ListenerEvent) {
        if let ListenerEvent::Composing { on, .. } = &event {
            self.shared.composing.store(*on, Ordering::Relaxed);
        }
        self.sink.deliver(event);
    }

    /// After a hook call's work: asks for the table of a layout met for the first time.
    fn finish(&mut self) {
        if let Some(layout) = self.unread_layout.take() {
            // If the message cannot be posted, the layout is asked for again at its next key.
            let _ = os::post_thread_message(self.thread_id, os::WM_LAYOUT, layout);
        }
    }
}

impl Host for System {
    fn foreground(&mut self) -> Foreground {
        let front = os::foreground();
        if front.layout != 0 {
            self.fallback_layout = front.layout;
        }
        Foreground {
            thread: front.thread,
            layout: self.fallback_layout,
        }
    }

    fn translate(&mut self, layout: usize, key: &KeyQuery) -> Translation {
        os::translate(layout, key)
    }

    fn compose(&mut self, layout: usize, dead: u16, base: u16) -> Composition {
        self.layouts.compose(layout, dead, base)
    }

    fn probe(&mut self, layout: usize) -> Option<Probe> {
        if self.layouts.wants_load(layout) {
            self.unread_layout = Some(layout);
        }
        self.layouts.probe(layout)
    }

    fn physically_down(&mut self, vk: u32) -> bool {
        os::physically_down(vk)
    }

    fn ask_input_method(&mut self, id: u32) -> bool {
        os::ask_input_method(id, input_method_answered)
    }

    fn report<F: FnOnce(u64) -> ListenerEvent>(&mut self, event: F) {
        let event = event(self.shared.seq.next());
        self.deliver(event);
    }

    fn self_test_seen(&mut self, nonce: u32) {
        self.shared.self_test.seen(nonce);
    }
}

thread_local! {
    /// The state of the listener that runs on this thread.
    static HOOKED: RefCell<Option<Hooked>> = const { RefCell::new(None) };
    /// An event went by unprocessed: `Overflow` is owed.
    static LOST: Cell<bool> = const { Cell::new(false) };
    /// How many hook calls of this thread are waiting for the hooks after them.
    static WAITING: Cell<u32> = const { Cell::new(0) };
}

/// Runs `work` on this thread's listener state. Never panics and never blocks: when the state
/// is in use, missing, or `work` panics, the event counts as lost.
fn with_state(work: impl FnOnce(&mut Hooked)) {
    let done = catch_unwind(AssertUnwindSafe(|| {
        HOOKED
            .try_with(|state| {
                let Ok(mut state) = state.try_borrow_mut() else {
                    return false;
                };
                // No state: the thread is installing or removing its hooks.
                let Some(hooked) = state.as_mut() else {
                    return true;
                };
                if LOST.replace(false) {
                    hooked.pipeline.lost();
                    hooked.system.deliver(ListenerEvent::Overflow);
                }
                work(hooked);
                hooked.system.finish();
                true
            })
            .unwrap_or(false)
    }));
    if !matches!(done, Ok(true)) {
        // Thread-local storage that is being torn down has nobody left to tell.
        let _ = LOST.try_with(|lost| lost.set(true));
    }
}

/// Passes a hook call on to the hooks installed before this thread's.
fn pass_on(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let waiting = |change: fn(u32) -> u32| {
        let _ = WAITING.try_with(|count| count.set(change(count.get())));
    };
    waiting(|count| count.saturating_add(1));
    let verdict = os::call_next_hook(code, wparam, lparam);
    waiting(|count| count.saturating_sub(1));
    verdict
}

/// True for the code with which a hook procedure may look at its event.
fn is_action(code: i32) -> bool {
    u32::try_from(code) == Ok(HC_ACTION)
}

/// The message a low-level hook was called with.
fn message_of(wparam: WPARAM) -> u32 {
    u32::try_from(wparam).unwrap_or(0)
}

/// The low-level keyboard hook procedure.
unsafe extern "system" fn keyboard_procedure(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    #[cfg(test)]
    let entered = Instant::now();
    if is_action(code) {
        // SAFETY: this is a `WH_KEYBOARD_LL` hook procedure called with `HC_ACTION`, and it
        // has not returned: `lparam` points to the event.
        let event = unsafe { os::keyboard_event(lparam) };
        let nested = WAITING.try_with(Cell::get).unwrap_or(0) > 0;
        with_state(|hooked| {
            let raw = RawKey {
                vk: event.vkCode,
                scan: event.scanCode,
                flags: event.flags,
                extra: event.dwExtraInfo as u64,
                message: message_of(wparam),
                time_ms: hooked.system.now_ms(),
                nested,
            };
            hooked.pipeline.on_key(&raw, &mut hooked.system);
        });
    }
    #[cfg(test)]
    let own = entered.elapsed();
    let verdict = pass_on(code, wparam, lparam);
    #[cfg(test)]
    timing::record(own, entered.elapsed());
    verdict
}

/// The low-level mouse hook procedure. It is called for every mouse movement, so it looks at
/// nothing but button presses.
unsafe extern "system" fn mouse_procedure(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let message = message_of(wparam);
    if is_action(code) && is_button_down(message) {
        // SAFETY: this is a `WH_MOUSE_LL` hook procedure called with `HC_ACTION`, and it has
        // not returned: `lparam` points to the event.
        let event = unsafe { os::mouse_event(lparam) };
        with_state(|hooked| {
            let raw = RawButton {
                message,
                flags: event.flags,
                extra: event.dwExtraInfo as u64,
            };
            hooked.pipeline.on_button(&raw, &mut hooked.system);
        });
    }
    pass_on(code, wparam, lparam)
}

/// Receives an input method's answer to
/// [`ask_input_method`](os::ask_input_method), on the listener thread.
unsafe extern "system" fn input_method_answered(
    _window: HWND,
    _message: u32,
    id: usize,
    answer: LRESULT,
) {
    let Ok(id) = u32::try_from(id) else { return };
    with_state(|hooked| {
        hooked
            .pipeline
            .input_method_answered(id, answer != 0, &mut hooked.system);
    });
}

/// The two hooks.
struct Hooks {
    keyboard: Hook,
    mouse: Hook,
}

impl Hooks {
    fn install() -> Result<Self> {
        let keyboard = Hook::install(HookKind::Keyboard, keyboard_procedure)?;
        // If the mouse hook fails, dropping `keyboard` removes it again.
        let mouse = Hook::install(HookKind::Mouse, mouse_procedure)?;
        Ok(Self { keyboard, mouse })
    }

    fn remove(self) -> Result<()> {
        let keyboard = self.keyboard.remove();
        let mouse = self.mouse.remove();
        keyboard.and(mouse)
    }
}

/// The hotkeys the system holds for this thread.
struct Held(Vec<Registration>);

impl Held {
    /// Tells the pipeline which key presses the system will take away.
    fn publish(&self) {
        let bindings = self.0.iter().map(Registration::binding).collect();
        with_state(|hooked| hooked.pipeline.set_hotkeys(bindings));
    }

    fn register(&mut self, registration: Registration) -> Result<()> {
        os::register_hotkey(
            registration.os_id,
            registration.modifier_flags(),
            registration.virtual_key(),
        )?;
        self.0.push(registration);
        self.publish();
        Ok(())
    }

    fn unregister(&mut self, os_id: i32) -> Result<()> {
        let Some(position) = self.0.iter().position(|held| held.os_id == os_id) else {
            return Ok(());
        };
        self.0.remove(position);
        self.publish();
        os::unregister_hotkey(os_id)
    }

    fn pressed(&self, os_id: i32) {
        if let Some(held) = self.0.iter().find(|held| held.os_id == os_id) {
            let id = held.id;
            with_state(|hooked| {
                hooked
                    .system
                    .report(|seq| ListenerEvent::Hotkey { id, seq });
            });
        }
    }

    fn release_all(&mut self) {
        for held in self.0.drain(..) {
            // The system drops a thread's hotkeys when the thread ends anyway.
            let _ = os::unregister_hotkey(held.os_id);
        }
    }
}

/// The body of the listener thread. Returns once the loop has ended and the hooks are
/// removed, with the system's refusal if it did not let a hook go.
pub(super) fn run(startup: Startup) -> Result<()> {
    let Startup {
        sink,
        shared,
        hotkeys,
        commands,
        ready,
    } = startup;
    os::create_message_queue();
    let thread_id = os::current_thread_id();

    // Everything slow happens before the hooks exist.
    let mut layouts = Layouts::default();
    let own_layout = os::layout_of_thread(0);
    for layout in os::keyboard_layouts().into_iter().chain([own_layout]) {
        layouts.load(layout);
    }
    let mut system = System {
        sink,
        shared: Arc::clone(&shared),
        layouts,
        unread_layout: None,
        fallback_layout: own_layout,
        thread_id,
        started: Instant::now(),
    };
    let mut pipeline = Pipeline::new(shared.composing.load(Ordering::Relaxed), 0);
    let now = system.now_ms();
    pipeline.seed(&mut system, os::caps_lock_on(), now);
    HOOKED.set(Some(Hooked { pipeline, system }));

    let mut hooks = match Hooks::install() {
        Ok(hooks) => Some(hooks),
        Err(error) => {
            drop(HOOKED.take());
            // `start` reports the error; the thread itself ended in order.
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    let mut held = Held(Vec::new());
    let mut refused_hotkeys = Vec::new();
    for registration in hotkeys {
        if held.register(registration).is_err() {
            refused_hotkeys.push(registration.os_id);
        }
    }

    let mut outcome = Ok(());
    let announced = ready.send(Ok(Ready {
        thread_id,
        refused_hotkeys,
    }));
    // Nobody waits for the announcement only if `start` itself is gone.
    while announced.is_ok() {
        match os::next_message() {
            Message::Stop => break,
            Message::Failed(code) => {
                outcome = Err(PlatformError::os(code, "GetMessageW"));
                break;
            }
            Message::Hotkey(os_id) => held.pressed(os_id),
            Message::Wake => {
                while let Ok(command) = commands.try_recv() {
                    obey(command, &mut held, &mut hooks);
                }
            }
            Message::Layout(layout) => with_state(|hooked| hooked.system.layouts.load(layout)),
            Message::Other => {}
        }
    }

    held.release_all();
    let removed = hooks.map_or(Ok(()), Hooks::remove);
    // Dropping the state drops the sink.
    drop(HOOKED.take());
    outcome.and(removed)
}

fn obey(command: Command, held: &mut Held, hooks: &mut Option<Hooks>) {
    match command {
        Command::Register(registration, answer) => {
            let result = held.register(registration);
            let registered = result.is_ok();
            if answer.send(result).is_err() && registered {
                // The caller gave up waiting and believes the hotkey is not registered.
                let _ = held.unregister(registration.os_id);
            }
        }
        Command::Unregister(os_id, answer) => {
            let _ = answer.send(held.unregister(os_id));
        }
        Command::Reinstall(answer) => {
            // The old hooks may be ones the system has already removed: a refusal to remove
            // them again says nothing.
            if let Some(old) = hooks.take() {
                let _ = old.remove();
            }
            // Before the new hooks exist: reading the toggle takes a moment.
            let caps = os::caps_lock_on();
            let result = Hooks::install().map(|new| *hooks = Some(new));
            // Whatever went by while the hook was dead is lost, and what the listener
            // remembered is stale.
            with_state(|hooked| {
                hooked.pipeline.lost();
                let now = hooked.system.now_ms();
                hooked.pipeline.seed(&mut hooked.system, caps, now);
                hooked.system.deliver(ListenerEvent::Overflow);
            });
            let _ = answer.send(result);
        }
    }
}

/// How long each call of the keyboard hook procedure took, for the latency test: fixed
/// arrays of atomics, so recording allocates nothing and takes no lock.
#[cfg(test)]
pub(super) mod timing {
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
    use std::time::Duration;

    /// Room for the samples of one test run.
    const CAPACITY: usize = 65_536;

    /// The procedure's own work: from its entry until it passes the call on.
    static OWN: [AtomicU32; CAPACITY] = [const { AtomicU32::new(0) }; CAPACITY];
    /// The whole call, with the hooks after the listener's.
    static WHOLE: [AtomicU32; CAPACITY] = [const { AtomicU32::new(0) }; CAPACITY];
    static COUNT: AtomicUsize = AtomicUsize::new(0);

    fn nanoseconds(duration: Duration) -> u32 {
        u32::try_from(duration.as_nanos()).unwrap_or(u32::MAX)
    }

    /// Records one call. Calls beyond the capacity overwrite the oldest.
    pub(super) fn record(own: Duration, whole: Duration) {
        let index = COUNT.fetch_add(1, Ordering::Relaxed) % CAPACITY;
        OWN[index].store(nanoseconds(own), Ordering::Relaxed);
        WHOLE[index].store(nanoseconds(whole), Ordering::Relaxed);
    }

    /// Forgets every sample.
    pub(crate) fn reset() {
        COUNT.store(0, Ordering::Relaxed);
    }

    /// The samples recorded since [`reset`], in nanoseconds: the procedure's own work, and
    /// the whole call.
    pub(crate) fn samples() -> (Vec<u32>, Vec<u32>) {
        let count = COUNT.load(Ordering::Relaxed).min(CAPACITY);
        let read = |samples: &[AtomicU32]| {
            samples[..count]
                .iter()
                .map(|sample| sample.load(Ordering::Relaxed))
                .collect()
        };
        (read(&OWN), read(&WHOLE))
    }
}
