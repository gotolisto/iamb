//! An in-memory operating system for tests (architecture §5).
//!
//! [`MockPlatform`] implements every platform trait against a small fake desktop: windows
//! with one text field each, a keyboard whose keys reach both the focused field and the
//! listener, a clipboard with a history, a hotkey registry, permissions, a login item and
//! local endpoints. It is always compiled (not test-only) so the service's tests, the testkit
//! and the compatibility harness can use it on every OS.
//!
//! ```
//! use std::sync::{Arc, Mutex};
//! use iamb_platform::mock::{MockPlatform, MockWindow};
//! use iamb_platform::{KeyListener, ListenerEvent};
//!
//! let os = MockPlatform::new();
//! let editor = os.open_window(MockWindow::new("org.example.editor", 100));
//! os.focus(editor)?;
//!
//! let seen = Arc::new(Mutex::new(Vec::new()));
//! let log = Arc::clone(&seen);
//! os.start(Box::new(move |event: ListenerEvent| log.lock().expect("not poisoned").push(event)))?;
//!
//! os.type_text("hi");
//! assert_eq!(os.text(editor).as_deref(), Some("hi"));
//! assert_eq!(
//!     *seen.lock().expect("not poisoned"),
//!     [ListenerEvent::char('h', 1), ListenerEvent::char('i', 2)]
//! );
//! # Ok::<(), iamb_platform::PlatformError>(())
//! ```
//!
//! # The key stream
//!
//! A real key ([`type_text`](MockPlatform::type_text), [`press`](MockPlatform::press),
//! [`click`](MockPlatform::click), [`play`](MockPlatform::play)) changes the focused field and
//! is reported to the listener's sink under one lock, so the sink sees keys in the order the
//! fields did, whichever threads type. Injected output changes the field the same way and is
//! not reported (the listener drops its own output); the two injection markers are reported.
//!
//! The sink is called with that lock held. It may do anything but call the mock's keyboard or
//! [`inject`](crate::Injector::inject) again; the usual sink pushes to a channel.
//!
//! A real key names the window it went to, as the Windows listener does
//! ([`EventSink::deliver_to`] with a [`KeyTarget`]). Focus changes reach the watcher at once,
//! unless [`hold_focus_reports`](MockPlatform::hold_focus_reports) holds them back, as a real
//! watcher hears of a change some time after the keys typed right after it.
//!
//! # Keys during an injection
//!
//! [`interleave`](MockPlatform::interleave) scripts real keys that arrive at a chosen point of
//! the next injection: before the start marker (the injector compensates for them, §4.6),
//! right after it, before a given step or before the end marker (the keys land wherever the
//! caret is at that moment, and the engine reports `typed_during_injection`). With
//! [`set_atomic_batches`](MockPlatform::set_atomic_batches) the mock behaves like an OS that
//! delivers a synthetic batch without interleaving: keys scripted inside the output arrive
//! after the end marker instead.
//!
//! [`script_at`](MockPlatform::script_at) scripts a focus change at such a point (the
//! application moved the focus on the trigger key, a window took it). Right before the start
//! marker the injector compares the focused window with the request's
//! [`target`](crate::InjectionRequest::target) and does not start when it is another one;
//! from there it notes the focused window and its field and stops with
//! [`FocusMoved`](crate::CancelReason::FocusMoved) before the first step that would go
//! elsewhere ([`FocusGuard`]).
//!
//! # What it does not model
//!
//! Layouts, dead keys, grapheme clusters, selections, timing (a `Wait` step is recorded and
//! added to [`waited_ms`](MockPlatform::waited_ms), not slept) and signatures (a peer's
//! `trusted_binary` flag is whatever the test says).

mod endpoint;
mod field;

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use iamb_core::engine::{BoundaryKind, ExpansionId};
use iamb_core::sensitive::Sensitive;
use iamb_core::template::KeyName;

use crate::clipboard::{
    ClipboardAccess, ClipboardContent, ClipboardFlavour, ClipboardFormat, ClipboardSnapshot,
};
use crate::endpoint::{
    EndpointName, LocalEndpoint, LocalListener, LocalStream, PeerIdentity, PeerPolicy,
};
use crate::error::{PlatformError, Result};
use crate::focus::{
    AppIdentity, FieldSecurity, FocusChange, FocusInspector, FocusSink, FocusToken, UnknownReason,
};
use crate::handoff::{CancelReason, HandoffStatus, START_MARKER_TIMEOUT, TypedHandoff};
use crate::hotkeys::{Chord, HotkeyId, Hotkeys};
use crate::inject::{
    FocusGuard, InjectionOutcome, InjectionRequest, InjectionStep, Injector, TextMethod, compensate,
};
use crate::injection_settings::{InjectionSettings, SettingsDialect};
use crate::listener::{EventSink, KeyListener, KeyTarget, ListenerEvent, SeqSource};
use crate::login_item::{LoginItem, LoginItemStatus};
use crate::marker::Marker;
use crate::memory::{MemoryLock, MemoryRegion};
use crate::permissions::{Grant, PermissionKind, PermissionStatus, Permissions};
use crate::session::{SessionEvent, SessionEvents, SessionSink};

use field::Field;

const FORMAT_TEXT: &str = "mock/text";
const FORMAT_HTML: &str = "mock/html";
const FORMAT_FILES: &str = "mock/files";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One real key of a scripted stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockKey {
    /// A key that produces this character.
    Char(char),
    /// A named key. Space types a space, Enter a line break and Tab a tab into the field;
    /// the listener reports them as [`ListenerEvent::named_key`] does.
    Key(KeyName),
    /// A mouse click in the focused field (the caret stays where it is).
    Click,
    /// A shortcut chord that produces no text and is not a registered hotkey.
    Shortcut,
}

impl MockKey {
    /// The keys a person presses to type `text`: Enter for `\n`, Tab for `\t`, a character
    /// key for everything else.
    #[must_use]
    pub fn text(text: &str) -> Vec<Self> {
        text.chars()
            .map(|ch| match ch {
                '\n' => Self::Key(KeyName::Enter),
                '\t' => Self::Key(KeyName::Tab),
                ch => Self::Char(ch),
            })
            .collect()
    }

    fn event(self, seq: u64) -> ListenerEvent {
        match self {
            Self::Char(ch) => ListenerEvent::char(ch, seq),
            Self::Key(key) => ListenerEvent::named_key(key, seq),
            Self::Click => ListenerEvent::Boundary {
                kind: BoundaryKind::Click,
                seq,
            },
            Self::Shortcut => ListenerEvent::Boundary {
                kind: BoundaryKind::Shortcut,
                seq,
            },
        }
    }
}

/// A point of an injection at which scripted real keys arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InjectionPoint {
    /// After the trigger, before the injector posts its start marker. The injector counts
    /// these keys and types them again.
    BeforeStartMarker,
    /// Right after the start marker, before any output.
    AfterStartMarker,
    /// Before the step with this index in the request's steps.
    BeforeStep(usize),
    /// After the last output, before the end marker.
    BeforeEndMarker,
}

/// A window to open on the mock desktop: an application, a process and one text field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockWindow {
    app: String,
    pid: u32,
    own: bool,
    security: FieldSecurity,
    inspect_delay: Duration,
    elevated: bool,
    text: String,
}

impl MockWindow {
    /// A window of application `app` (a bundle identifier or executable name) in process
    /// `pid`, with an empty, ordinary text field.
    #[must_use]
    pub fn new(app: impl Into<String>, pid: u32) -> Self {
        Self {
            app: app.into(),
            pid,
            own: false,
            security: FieldSecurity::Normal,
            inspect_delay: Duration::ZERO,
            elevated: false,
            text: String::new(),
        }
    }

    /// One of Iamb's own windows.
    #[must_use]
    pub const fn own(mut self) -> Self {
        self.own = true;
        self
    }

    /// The field is a password field.
    #[must_use]
    pub const fn secure(mut self) -> Self {
        self.security = FieldSecurity::Secure;
        self
    }

    /// The field's security, as the OS would report it.
    #[must_use]
    pub const fn security(mut self, security: FieldSecurity) -> Self {
        self.security = security;
        self
    }

    /// Inspecting the field takes this long: a `secure_state` call with a shorter timeout
    /// reports a timeout.
    #[must_use]
    pub const fn inspect_delay(mut self, delay: Duration) -> Self {
        self.inspect_delay = delay;
        self
    }

    /// The window runs at a higher privilege level: it cannot be inspected and refuses
    /// injection.
    #[must_use]
    pub const fn elevated(mut self) -> Self {
        self.elevated = true;
        self
    }

    /// The field starts with `text`, the caret at its end.
    #[must_use]
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }
}

struct Window {
    spec: MockWindow,
    field: Field,
}

impl Window {
    fn identity(&self) -> AppIdentity {
        let identity = AppIdentity::new(self.spec.app.clone(), self.spec.pid);
        if self.spec.own {
            identity.own()
        } else {
            identity
        }
    }
}

/// One entry of the mock injector's log, in the order things were sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Injected {
    /// The start marker of an expansion.
    StartMarker(ExpansionId),
    /// Backspace, this many times (the expansion's own deletion plus the compensated keys).
    DeleteBack(usize),
    /// Text sent as key events.
    TypeText(String),
    /// Content sent through the clipboard (a `Paste` step, or text under a paste override).
    Paste(ClipboardContent),
    /// A named key.
    PressKey {
        /// The key.
        key: KeyName,
        /// Shift was held.
        shift: bool,
        /// How many presses.
        times: u8,
    },
    /// Left, this many times.
    MoveCaretBack(usize),
    /// A pause of this many milliseconds (recorded, not slept).
    Wait(u32),
    /// This many characters typed after the trigger were typed again (the log never holds
    /// them; the field shows them).
    Retype(usize),
    /// The end marker of an expansion.
    EndMarker(ExpansionId),
    /// The expansion was not sent.
    Cancelled(ExpansionId, CancelReason),
    /// The expansion stopped because the mock OS refused.
    Failed(ExpansionId, PlatformError),
}

/// The listener's side of the keyboard. Its mutex is the key stream's order.
#[derive(Default)]
struct Stream {
    sink: Option<Box<dyn EventSink>>,
    hook_dead: bool,
}

struct Desk {
    windows: BTreeMap<u64, Window>,
    next_window: u64,
    focused: Option<u64>,
    secure_script: VecDeque<FieldSecurity>,
    refuse_restore: bool,
    allowed_foreground: Vec<u32>,
    clipboard: ClipboardContent,
    clipboard_history: Vec<ClipboardContent>,
    clipboard_busy: bool,
    hotkeys: BTreeMap<HotkeyId, Chord>,
    reserved_chords: Vec<Chord>,
    permissions: PermissionStatus,
    opened_settings: Vec<PermissionKind>,
    login_item: LoginItemStatus,
    locked: Vec<MemoryRegion>,
    interleave: Vec<(InjectionPoint, Vec<MockKey>)>,
    scripted: Vec<(InjectionPoint, Scripted)>,
    atomic_batches: bool,
    marker_timeout: Duration,
    injected: Vec<Injected>,
    waited_ms: u64,
    self_tests: u32,
    synthetic_events: usize,
    settings_dialect: SettingsDialect,
    configured: Vec<String>,
    injection_settings: String,
    /// A window that takes the focus right after the next `focused_app` read
    /// ([`MockPlatform::focus_after_next_app_read`]).
    focus_after_app_read: Option<FocusToken>,
}

impl Desk {
    fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
            next_window: 1,
            focused: None,
            secure_script: VecDeque::new(),
            refuse_restore: false,
            allowed_foreground: Vec::new(),
            clipboard: ClipboardContent::default(),
            clipboard_history: Vec::new(),
            clipboard_busy: false,
            hotkeys: BTreeMap::new(),
            reserved_chords: Vec::new(),
            permissions: PermissionStatus {
                accessibility: Grant::Granted,
                input_monitoring: Grant::Granted,
            },
            opened_settings: Vec::new(),
            login_item: LoginItemStatus::Disabled,
            locked: Vec::new(),
            interleave: Vec::new(),
            scripted: Vec::new(),
            atomic_batches: false,
            marker_timeout: START_MARKER_TIMEOUT,
            injected: Vec::new(),
            waited_ms: 0,
            self_tests: 0,
            synthetic_events: 0,
            settings_dialect: SettingsDialect::Windows,
            configured: Vec::new(),
            injection_settings: String::new(),
            focus_after_app_read: None,
        }
    }

    fn focused_window(&self) -> Option<&Window> {
        self.focused.and_then(|id| self.windows.get(&id))
    }

    fn focused_field(&mut self) -> Option<&mut Field> {
        let id = self.focused?;
        self.windows.get_mut(&id).map(|w| &mut w.field)
    }

    fn focus_change(&self) -> Option<FocusChange> {
        let id = self.focused?;
        let window = self.windows.get(&id)?;
        Some(FocusChange {
            app: window.identity(),
            token: FocusToken::from_raw(window.spec.pid, id),
        })
    }

    fn clipboard(&self) -> Result<&ClipboardContent> {
        if self.clipboard_busy {
            Err(PlatformError::os(
                5,
                "mock clipboard is held by another program",
            ))
        } else {
            Ok(&self.clipboard)
        }
    }

    /// What a real key does to the focused field.
    fn apply_key(&mut self, key: MockKey) {
        if let Some(field) = self.focused_field() {
            match key {
                MockKey::Char(ch) => field.insert(ch),
                MockKey::Key(key) => field.press(key, 1),
                MockKey::Click | MockKey::Shortcut => {}
            }
        }
    }
}

/// Something a test scripted for a point of an injection ([`MockPlatform::script_at`]).
type Scripted = Box<dyn FnOnce(&MockPlatform) + Send>;

struct Os {
    stream: Mutex<Stream>,
    desk: Mutex<Desk>,
    focus_sink: Mutex<Option<Box<dyn FocusSink>>>,
    session_sink: Mutex<Option<Box<dyn SessionSink>>>,
    /// While focus reports are held back ([`MockPlatform::hold_focus_reports`]): the changes
    /// not reported yet, oldest first.
    held_focus: Mutex<Option<Vec<FocusChange>>>,
    seq: SeqSource,
    endpoints: Arc<endpoint::Registry>,
}

/// The in-memory platform. See the [module docs](self).
///
/// Cloning gives another handle to the same fake machine, so a test keeps one to script and
/// inspect while the code under test holds another as `Arc<dyn Platform>`.
#[derive(Clone)]
pub struct MockPlatform {
    os: Arc<Os>,
    identity: PeerIdentity,
}

impl Default for MockPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for MockPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockPlatform")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl MockPlatform {
    /// A machine with no windows, an empty clipboard, every permission granted, and this
    /// process running as user `mock-user` from a trusted binary.
    #[must_use]
    pub fn new() -> Self {
        Self {
            os: Arc::new(Os {
                stream: Mutex::default(),
                desk: Mutex::new(Desk::new()),
                focus_sink: Mutex::new(None),
                session_sink: Mutex::new(None),
                held_focus: Mutex::new(None),
                seq: SeqSource::new(),
                endpoints: Arc::default(),
            }),
            identity: PeerIdentity {
                pid: 1000,
                user: "mock-user".to_owned(),
                executable: Some("/mock/iamb-service".into()),
                trusted_binary: true,
            },
        }
    }

    /// A handle to the same machine for another process: endpoints bound or connected through
    /// it present `identity` to their peers.
    #[must_use]
    pub fn with_identity(&self, identity: PeerIdentity) -> Self {
        Self {
            os: Arc::clone(&self.os),
            identity,
        }
    }

    /// The identity this handle's endpoints present.
    #[must_use]
    pub const fn identity(&self) -> &PeerIdentity {
        &self.identity
    }

    fn desk(&self) -> MutexGuard<'_, Desk> {
        lock(&self.os.desk)
    }

    // -----------------------------------------------------------------------------------------
    // Windows and focus
    // -----------------------------------------------------------------------------------------

    /// Opens a window without focusing it and returns its token.
    #[must_use]
    pub fn open_window(&self, window: MockWindow) -> FocusToken {
        let mut desk = self.desk();
        let id = desk.next_window;
        desk.next_window += 1;
        let token = FocusToken::from_raw(window.pid, id);
        let field = Field::with_text(&window.text);
        desk.windows.insert(
            id,
            Window {
                spec: window,
                field,
            },
        );
        token
    }

    /// Gives the keyboard focus to a window, as the user would by clicking its title bar, and
    /// reports the change to the focus watcher.
    ///
    /// # Errors
    ///
    /// [`PlatformError::FocusLost`] when the window is not open.
    pub fn focus(&self, token: FocusToken) -> Result<()> {
        let change = {
            let mut desk = self.desk();
            if !desk.windows.contains_key(&token.window()) {
                return Err(PlatformError::FocusLost);
            }
            desk.focused = Some(token.window());
            desk.focus_change()
        };
        self.report_focus(change);
        Ok(())
    }

    /// Closes a window. If it had the focus, nothing has it afterwards.
    pub fn close_window(&self, token: FocusToken) {
        let mut desk = self.desk();
        desk.windows.remove(&token.window());
        if desk.focused == Some(token.window()) {
            desk.focused = None;
        }
    }

    /// Changes what the OS reports about a window's field, as if the user moved to another
    /// field of the same window. Reported to the focus watcher when the window is focused.
    pub fn set_security(&self, token: FocusToken, security: FieldSecurity) {
        let change = {
            let mut desk = self.desk();
            let Some(window) = desk.windows.get_mut(&token.window()) else {
                return;
            };
            window.spec.security = security;
            if desk.focused == Some(token.window()) {
                desk.focus_change()
            } else {
                None
            }
        };
        self.report_focus(change);
    }

    fn report_focus(&self, change: Option<FocusChange>) {
        let Some(change) = change else { return };
        if let Some(held) = lock(&self.os.held_focus).as_mut() {
            held.push(change);
            return;
        }
        if let Some(sink) = lock(&self.os.focus_sink).as_mut() {
            sink.focus_changed(change);
        }
    }

    /// With `true`, focus changes are not reported to the focus watcher until
    /// `hold_focus_reports(false)`, which reports the held ones in order: a stand-in for the
    /// time a real focus event takes to reach the watcher (an out-of-context window event on
    /// Windows, a notification through two run loops on macOS). Keys typed meanwhile go to the
    /// newly focused window and tell the listener so ([`KeyTarget`]).
    pub fn hold_focus_reports(&self, hold: bool) {
        let waiting = {
            let mut queue = lock(&self.os.held_focus);
            if hold {
                queue.get_or_insert_with(Vec::new);
                return;
            }
            queue.take().unwrap_or_default()
        };
        if let Some(sink) = lock(&self.os.focus_sink).as_mut() {
            for change in waiting {
                sink.focus_changed(change);
            }
        }
    }

    /// Makes `token`'s window take the focus right after the next
    /// [`focused_app`](FocusInspector::focused_app) has answered, reported like
    /// [`focus`](Self::focus) (and held back with the other reports while they are held): a
    /// focus change that falls between two reads of the focus (the close-out review's C3).
    pub fn focus_after_next_app_read(&self, token: FocusToken) {
        self.desk().focus_after_app_read = Some(token);
    }

    /// The text of a window's field; `None` when the window is not open.
    #[must_use]
    pub fn text(&self, token: FocusToken) -> Option<String> {
        self.desk()
            .windows
            .get(&token.window())
            .map(|w| w.field.text())
    }

    /// The caret position of a window's field, in characters from the start.
    #[must_use]
    pub fn caret(&self, token: FocusToken) -> Option<usize> {
        self.desk()
            .windows
            .get(&token.window())
            .map(|w| w.field.caret())
    }

    /// Replaces the text of a window's field, with the caret at the end, without any key.
    pub fn set_text(&self, token: FocusToken, text: &str) {
        if let Some(window) = self.desk().windows.get_mut(&token.window()) {
            window.field = Field::with_text(text);
        }
    }

    /// Queues answers for the next [`secure_state`](FocusInspector::secure_state) calls, used
    /// before the focused window's own state.
    pub fn script_secure_states(&self, states: impl IntoIterator<Item = FieldSecurity>) {
        self.desk().secure_script.extend(states);
    }

    /// Makes [`restore_focus`](FocusInspector::restore_focus) time out (`true`) or work again.
    pub fn refuse_focus_restore(&self, refuse: bool) {
        self.desk().refuse_restore = refuse;
    }

    /// The process ids passed to [`allow_foreground`](FocusInspector::allow_foreground).
    #[must_use]
    pub fn allowed_foreground(&self) -> Vec<u32> {
        self.desk().allowed_foreground.clone()
    }

    // -----------------------------------------------------------------------------------------
    // The keyboard
    // -----------------------------------------------------------------------------------------

    /// Delivers one event to the listener, stamped with the next sequence number, if the
    /// listener runs and its hook is alive.
    fn report(&self, stream: &mut Stream, event: impl FnOnce(u64) -> ListenerEvent) {
        self.report_to(stream, None, event);
    }

    /// As [`report`](Self::report), with the window the key went to when there is one.
    fn report_to(
        &self,
        stream: &mut Stream,
        target: Option<KeyTarget>,
        event: impl FnOnce(u64) -> ListenerEvent,
    ) {
        if !stream.hook_dead
            && let Some(sink) = stream.sink.as_mut()
        {
            let event = event(self.os.seq.next());
            match target {
                Some(target) => sink.deliver_to(event, target),
                None => sink.deliver(event),
            }
        }
    }

    /// A real key goes to the focused window, and the listener is told which one, as the
    /// Windows listener is (`KeyTarget`).
    fn real_key(&self, stream: &mut Stream, key: MockKey) {
        let target = {
            let mut desk = self.desk();
            desk.apply_key(key);
            desk.focus_change()
                .map(|change| KeyTarget::window(change.token))
        };
        self.report_to(stream, target, |seq| key.event(seq));
    }

    /// Presses the keys of a scripted stream, in order.
    pub fn play(&self, keys: impl IntoIterator<Item = MockKey>) {
        for key in keys {
            let mut stream = lock(&self.os.stream);
            self.real_key(&mut stream, key);
        }
    }

    /// Types `text` on the keyboard ([`MockKey::text`]).
    pub fn type_text(&self, text: &str) {
        self.play(MockKey::text(text));
    }

    /// Presses a named key.
    pub fn press(&self, key: KeyName) {
        self.play([MockKey::Key(key)]);
    }

    /// Clicks in the focused field.
    pub fn click(&self) {
        self.play([MockKey::Click]);
    }

    /// An input method starts (`true`) or finishes composing.
    pub fn set_composing(&self, on: bool) {
        let mut stream = lock(&self.os.stream);
        self.report(&mut stream, |seq| ListenerEvent::Composing { on, seq });
    }

    /// The OS lost key events: the listener reports `Overflow`.
    pub fn lose_events(&self) {
        let mut stream = lock(&self.os.stream);
        self.report(&mut stream, |_| ListenerEvent::Overflow);
    }

    /// The OS silently removes the listener's hook: keys still reach the fields, nothing
    /// reaches the sink and the self-test times out, until the listener is stopped and started
    /// again.
    pub fn kill_hook(&self) {
        lock(&self.os.stream).hook_dead = true;
    }

    /// True while the listener is started (whether or not its hook is alive).
    #[must_use]
    pub fn listener_running(&self) -> bool {
        lock(&self.os.stream).sink.is_some()
    }

    /// How many self-tests succeeded.
    #[must_use]
    pub fn self_tests(&self) -> u32 {
        self.desk().self_tests
    }

    /// Presses a chord. A registered hotkey is delivered as `Hotkey` and consumed (`true`);
    /// anything else is a shortcut the focused application sees (`false`).
    #[must_use]
    pub fn press_chord(&self, chord: Chord) -> bool {
        let mut stream = lock(&self.os.stream);
        let registered = self
            .desk()
            .hotkeys
            .iter()
            .find(|(_, c)| **c == chord)
            .map(|(id, _)| *id);
        if let Some(id) = registered {
            self.report(&mut stream, |seq| ListenerEvent::Hotkey { id, seq });
            true
        } else {
            self.real_key(&mut stream, MockKey::Shortcut);
            false
        }
    }

    // -----------------------------------------------------------------------------------------
    // Injection scripting and inspection
    // -----------------------------------------------------------------------------------------

    /// Scripts real keys that arrive at `point` of the next injection. Several calls for the
    /// same point add up; every script is used once.
    pub fn interleave(&self, point: InjectionPoint, keys: impl IntoIterator<Item = MockKey>) {
        self.desk()
            .interleave
            .push((point, keys.into_iter().collect()));
    }

    /// Scripts something else that happens at `point` of the next injection: a focus change
    /// (`|os| os.focus(other)`), a move to another field (`set_security`), as an application
    /// reacting to the trigger key or a window taking the focus would. It runs after the keys
    /// scripted for the same point, also with atomic batches: the focus can change while a
    /// batch is under way. Several calls for the same point run in order; each runs once.
    pub fn script_at(&self, point: InjectionPoint, action: impl FnOnce(&Self) + Send + 'static) {
        self.desk().scripted.push((point, Box::new(action)));
    }

    /// With `true`, the mock delivers the output of an injection as one batch that real keys
    /// cannot interleave (as Windows does): keys scripted for
    /// [`BeforeStep`](InjectionPoint::BeforeStep) and
    /// [`BeforeEndMarker`](InjectionPoint::BeforeEndMarker) arrive after the end marker.
    pub fn set_atomic_batches(&self, atomic: bool) {
        self.desk().atomic_batches = atomic;
    }

    /// How long the mock injector waits for its start marker
    /// ([`START_MARKER_TIMEOUT`] unless set).
    pub fn set_marker_timeout(&self, timeout: Duration) {
        self.desk().marker_timeout = timeout;
    }

    /// Everything the injector sent so far, in order.
    #[must_use]
    pub fn injected(&self) -> Vec<Injected> {
        self.desk().injected.clone()
    }

    /// Returns the injector's log and empties it.
    #[must_use]
    pub fn take_injected(&self) -> Vec<Injected> {
        std::mem::take(&mut self.desk().injected)
    }

    /// The total of all `Wait` steps so far, in milliseconds.
    #[must_use]
    pub fn waited_ms(&self) -> u64 {
        self.desk().waited_ms
    }

    /// How many synthetic key events the injector produced (each one carried the output
    /// marker and was dropped by the listener).
    #[must_use]
    pub fn synthetic_events(&self) -> usize {
        self.desk().synthetic_events
    }

    /// Which system's variant of the injection settings format
    /// [`configure`](Injector::configure) accepts ([`SettingsDialect::Windows`] unless set).
    pub fn set_settings_dialect(&self, dialect: SettingsDialect) {
        self.desk().settings_dialect = dialect;
    }

    /// Every string [`configure`](Injector::configure) was given, accepted or not, in order.
    #[must_use]
    pub fn configured(&self) -> Vec<String> {
        self.desk().configured.clone()
    }

    /// The injection settings in force: the last string `configure` accepted (empty at
    /// first).
    #[must_use]
    pub fn injection_settings(&self) -> String {
        self.desk().injection_settings.clone()
    }

    // -----------------------------------------------------------------------------------------
    // Clipboard, hotkeys, permissions, login item, memory
    // -----------------------------------------------------------------------------------------

    /// The user copies `content`: it replaces the clipboard and enters the clipboard history.
    pub fn copy(&self, content: ClipboardContent) {
        let mut desk = self.desk();
        desk.clipboard_history.push(content.clone());
        desk.clipboard = content;
    }

    /// What is on the clipboard now.
    #[must_use]
    pub fn clipboard(&self) -> ClipboardContent {
        self.desk().clipboard.clone()
    }

    /// Everything that entered the clipboard history: user copies and non-transient writes.
    #[must_use]
    pub fn clipboard_history(&self) -> Vec<ClipboardContent> {
        self.desk().clipboard_history.clone()
    }

    /// Another program holds the clipboard open (`true`): every clipboard call fails.
    pub fn set_clipboard_busy(&self, busy: bool) {
        self.desk().clipboard_busy = busy;
    }

    /// Another application holds `chord`: registering it fails.
    pub fn reserve_chord(&self, chord: Chord) {
        self.desk().reserved_chords.push(chord);
    }

    /// The registered hotkeys, by id.
    #[must_use]
    pub fn registered_hotkeys(&self) -> Vec<(HotkeyId, Chord)> {
        self.desk()
            .hotkeys
            .iter()
            .map(|(id, c)| (*id, *c))
            .collect()
    }

    /// Sets the state of a permission.
    pub fn set_permission(&self, kind: PermissionKind, grant: Grant) {
        let mut desk = self.desk();
        match kind {
            PermissionKind::Accessibility => desk.permissions.accessibility = grant,
            PermissionKind::InputMonitoring => desk.permissions.input_monitoring = grant,
        }
    }

    /// The settings pages opened so far.
    #[must_use]
    pub fn opened_settings(&self) -> Vec<PermissionKind> {
        self.desk().opened_settings.clone()
    }

    /// Sets the login item's state directly (for `RequiresApproval` and `NotFound`).
    pub fn set_login_item_status(&self, status: LoginItemStatus) {
        self.desk().login_item = status;
    }

    /// The regions locked now.
    #[must_use]
    pub fn locked_regions(&self) -> Vec<MemoryRegion> {
        self.desk().locked.clone()
    }

    /// Reports a session event (a screen lock, a sign-out, …) to the session watcher, as the
    /// OS would. Returns false when nothing watches.
    #[must_use = "false: nothing watches the session"]
    pub fn session_event(&self, event: SessionEvent) -> bool {
        match lock(&self.os.session_sink).as_mut() {
            Some(sink) => {
                sink.session_event(event);
                true
            }
            None => false,
        }
    }

    /// True while a session watcher is installed.
    #[must_use]
    pub fn watching_session(&self) -> bool {
        lock(&self.os.session_sink).is_some()
    }

    // -----------------------------------------------------------------------------------------
    // The injector's parts
    // -----------------------------------------------------------------------------------------

    fn log(&self, entry: Injected) {
        self.desk().injected.push(entry);
    }

    /// Plays the keys scripted for `point`; in atomic mode, keys inside the output are put
    /// aside in `after_batch`.
    fn interleaved(&self, point: InjectionPoint, after_batch: &mut Vec<MockKey>) {
        let (keys, defer) = {
            let mut desk = self.desk();
            let mut keys = Vec::new();
            desk.interleave.retain(|(p, k)| {
                if *p == point {
                    keys.extend(k.iter().copied());
                }
                *p != point
            });
            let inside_output = matches!(
                point,
                InjectionPoint::BeforeStep(_) | InjectionPoint::BeforeEndMarker
            );
            (keys, desk.atomic_batches && inside_output)
        };
        if defer {
            after_batch.extend(keys);
        } else {
            self.play(keys);
        }
        let actions: Vec<Scripted> = {
            let mut desk = self.desk();
            let (now, later) = std::mem::take(&mut desk.scripted)
                .into_iter()
                .partition(|(p, _)| *p == point);
            desk.scripted = later;
            now.into_iter().map(|(_, action)| action).collect()
        };
        for action in actions {
            action(self);
        }
    }

    /// Where the injector's keys go, for its [`FocusGuard`]: the focused window (with its
    /// process) and what its field is (a move to another field of the window is a change of
    /// its security here).
    fn keys_go_to(&self) -> Option<(FocusToken, FieldSecurity)> {
        let desk = self.desk();
        let id = desk.focused?;
        desk.windows
            .get(&id)
            .map(|w| (FocusToken::from_raw(w.spec.pid, id), w.spec.security))
    }

    /// Posts a marked no-op event. The listener does what a real one does with it: decodes the
    /// marker field and reports the injection markers in stream order.
    fn post_marker(&self, marker: Marker) {
        let mut stream = lock(&self.os.stream);
        self.desk().synthetic_events += 1;
        match Marker::decode(marker.encode()) {
            Some(Marker::InjectionStart(expansion)) => {
                self.report(&mut stream, |seq| ListenerEvent::InjectionStarted {
                    expansion,
                    seq,
                });
            }
            Some(Marker::InjectionEnd(expansion)) => {
                self.report(&mut stream, |seq| ListenerEvent::InjectionFinished {
                    expansion,
                    seq,
                });
            }
            Some(Marker::Output | Marker::SelfTest(_)) | None => {}
        }
    }

    /// Synthetic output: changes the focused field under the stream lock and counts the
    /// events; nothing is reported, because the listener drops its own output.
    fn output(&self, events: usize, edit: impl FnOnce(&mut Field)) {
        let _stream = lock(&self.os.stream);
        let mut desk = self.desk();
        desk.synthetic_events += events;
        if let Some(field) = desk.focused_field() {
            edit(field);
        }
    }

    fn paste(&self, content: &ClipboardContent) -> Result<()> {
        let saved = self.snapshot()?;
        self.write(content, true)?;
        // The paste shortcut: two synthetic key events.
        self.output(2, |field| {
            if let Some(text) = &content.text {
                field.insert_str(text);
            }
        });
        self.restore(saved)
    }

    fn send_step(&self, request: &InjectionRequest, step: &InjectionStep) -> Result<()> {
        match step {
            InjectionStep::DeleteBack(n) => {
                self.output(*n, |field| field.delete_back(*n));
                self.log(Injected::DeleteBack(*n));
            }
            InjectionStep::TypeText(text) => {
                if request.overrides.text_method == Some(TextMethod::Paste) {
                    let content = ClipboardContent::text(text.clone());
                    self.paste(&content)?;
                    self.log(Injected::Paste(content));
                } else {
                    self.output(text.chars().count(), |field| field.insert_str(text));
                    self.log(Injected::TypeText(text.clone()));
                }
            }
            InjectionStep::Paste(content) => {
                self.paste(content)?;
                self.log(Injected::Paste(content.clone()));
            }
            InjectionStep::PressKey { key, shift, times } => {
                self.output(usize::from(*times), |field| {
                    field.press(*key, usize::from(*times));
                });
                self.log(Injected::PressKey {
                    key: *key,
                    shift: *shift,
                    times: *times,
                });
            }
            InjectionStep::MoveCaretBack(n) => {
                self.output(*n, |field| field.left(*n));
                self.log(Injected::MoveCaretBack(*n));
            }
            InjectionStep::Wait(ms) => {
                let mut desk = self.desk();
                desk.waited_ms += u64::from(*ms);
                desk.injected.push(Injected::Wait(*ms));
            }
        }
        Ok(())
    }

    /// Steps 4 and 5 of the injector protocol, after the hand-off sealed with `k` characters.
    ///
    /// The mock sends step by step, so the guard is asked before every step and before the
    /// retyped characters.
    fn send_output(
        &self,
        request: &InjectionRequest,
        typed: &TypedHandoff,
        k: usize,
        guard: &mut FocusGuard<Option<(FocusToken, FieldSecurity)>>,
        after_batch: &mut Vec<MockKey>,
    ) -> std::result::Result<(), InjectionOutcome> {
        let failed = InjectionOutcome::Failed;
        let check = |guard: &mut FocusGuard<_>| {
            guard
                .check(|| self.keys_go_to())
                .map_err(InjectionOutcome::Cancelled)
        };
        let plan = compensate(&request.steps, k);
        let own_delete = request.steps.len() - plan.rest.len();
        if own_delete == 1 {
            self.interleaved(InjectionPoint::BeforeStep(0), after_batch);
        }
        if plan.delete_first > 0 {
            check(guard)?;
            self.send_step(request, &InjectionStep::DeleteBack(plan.delete_first))
                .map_err(failed)?;
        }
        for (i, step) in plan.rest.iter().enumerate() {
            self.interleaved(InjectionPoint::BeforeStep(i + own_delete), after_batch);
            check(guard)?;
            self.send_step(request, step).map_err(failed)?;
            if step.may_move_focus() {
                guard.own_move();
            }
        }
        if plan.retype > 0 {
            check(guard)?;
            // The mock's input batch: filled inside the closure, sent outside it, zeroed.
            let mut batch = typed
                .read(request.expansion, <[char]>::to_vec)
                .map_err(InjectionOutcome::Cancelled)?;
            self.output(batch.len(), |field| {
                for &ch in &batch {
                    field.insert(ch);
                }
            });
            self.log(Injected::Retype(batch.len()));
            batch.fill('\0');
        }
        Ok(())
    }
}

impl KeyListener for MockPlatform {
    fn start(&self, sink: Box<dyn EventSink>) -> Result<()> {
        if self.desk().permissions.input_monitoring == Grant::Denied {
            return Err(PlatformError::PermissionDenied(
                PermissionKind::InputMonitoring,
            ));
        }
        let mut stream = lock(&self.os.stream);
        if stream.sink.is_some() {
            return Err(PlatformError::AlreadyRunning);
        }
        stream.sink = Some(sink);
        stream.hook_dead = false;
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        let mut stream = lock(&self.os.stream);
        stream.sink = None;
        stream.hook_dead = false;
        Ok(())
    }

    fn self_test(&self, _timeout: Duration) -> Result<()> {
        let stream = lock(&self.os.stream);
        if stream.sink.is_none() {
            return Err(PlatformError::NotRunning);
        }
        if stream.hook_dead {
            return Err(PlatformError::Timeout);
        }
        // The marked no-op event went through the hook: counted, never delivered.
        let mut desk = self.desk();
        desk.self_tests += 1;
        desk.synthetic_events += 1;
        Ok(())
    }
}

impl MockPlatform {
    /// Logs how an injection ended early and returns the outcome.
    fn stopped(&self, expansion: ExpansionId, outcome: InjectionOutcome) -> InjectionOutcome {
        match &outcome {
            InjectionOutcome::Cancelled(reason) => {
                self.log(Injected::Cancelled(expansion, *reason));
            }
            InjectionOutcome::Failed(error) => {
                self.log(Injected::Failed(expansion, error.clone()));
            }
            InjectionOutcome::Finished { .. } => {}
        }
        outcome
    }
}

impl Injector for MockPlatform {
    /// Records `settings_json`, reads it with [`InjectionSettings::parse`] in the dialect of
    /// [`set_settings_dialect`](MockPlatform::set_settings_dialect), and keeps it when it is
    /// valid. The mock's output does not depend on the settings.
    fn configure(&self, settings_json: &str) -> Result<()> {
        let mut desk = self.desk();
        desk.configured.push(settings_json.to_owned());
        InjectionSettings::parse(settings_json, desk.settings_dialect)?;
        settings_json.clone_into(&mut desk.injection_settings);
        Ok(())
    }

    fn inject(&self, request: &InjectionRequest, typed: &TypedHandoff) -> InjectionOutcome {
        let expansion = request.expansion;
        let refusal = {
            let desk = self.desk();
            if desk.permissions.accessibility == Grant::Denied {
                Some(PlatformError::PermissionDenied(
                    PermissionKind::Accessibility,
                ))
            } else if desk.focused_window().is_some_and(|w| w.spec.elevated) {
                Some(PlatformError::InjectionRefused)
            } else {
                None
            }
        };
        if let Some(error) = refusal {
            return self.stopped(expansion, InjectionOutcome::Failed(error));
        }
        if let HandoffStatus::Cancelled(e, reason) = typed.status()
            && e == expansion
        {
            return self.stopped(expansion, InjectionOutcome::Cancelled(reason));
        }

        let mut after_batch = Vec::new();
        self.interleaved(InjectionPoint::BeforeStartMarker, &mut after_batch);
        // The start check (N1): the keys must still go to the trigger's window.
        let mut guard = match FocusGuard::for_request(request.target, self.keys_go_to(), |now| {
            now.map(|(token, _)| KeyTarget::window(token))
        }) {
            Ok(guard) => guard,
            Err(reason) => return self.stopped(expansion, InjectionOutcome::Cancelled(reason)),
        };
        self.post_marker(Marker::InjectionStart(expansion));
        self.log(Injected::StartMarker(expansion));
        self.interleaved(InjectionPoint::AfterStartMarker, &mut after_batch);

        let timeout = self.desk().marker_timeout;
        let k = match typed.wait_sealed(expansion, timeout) {
            Ok(k) => k,
            Err(reason) => return self.stopped(expansion, InjectionOutcome::Cancelled(reason)),
        };
        if let Err(outcome) = self.send_output(request, typed, k, &mut guard, &mut after_batch) {
            return self.stopped(expansion, outcome);
        }

        self.interleaved(InjectionPoint::BeforeEndMarker, &mut after_batch);
        self.post_marker(Marker::InjectionEnd(expansion));
        self.log(Injected::EndMarker(expansion));
        self.play(after_batch);
        InjectionOutcome::Finished { retyped: k }
    }
}

impl FocusInspector for MockPlatform {
    fn focused_app(&self) -> Result<AppIdentity> {
        let (answer, then) = {
            let mut desk = self.desk();
            let answer = desk
                .focused_window()
                .map(Window::identity)
                .ok_or(PlatformError::NoFocus);
            (answer, desk.focus_after_app_read.take())
        };
        if let Some(token) = then {
            let _ = self.focus(token);
        }
        answer
    }

    fn secure_state(&self, target: &FocusToken, timeout: Duration) -> FieldSecurity {
        let mut desk = self.desk();
        if let Some(scripted) = desk.secure_script.pop_front() {
            return scripted;
        }
        match desk.focused_window() {
            None => FieldSecurity::Unknown(UnknownReason::NoFocus),
            Some(_) if desk.focused != Some(target.window()) => {
                FieldSecurity::Unknown(UnknownReason::FocusMoved)
            }
            Some(w) if w.spec.elevated => FieldSecurity::Unknown(UnknownReason::Elevated),
            Some(w) if w.spec.inspect_delay > timeout => {
                FieldSecurity::Unknown(UnknownReason::Timeout)
            }
            Some(w) => w.spec.security,
        }
    }

    fn focus_token(&self) -> Result<FocusToken> {
        self.desk()
            .focus_change()
            .map(|change| change.token)
            .ok_or(PlatformError::NoFocus)
    }

    fn restore_focus(&self, token: &FocusToken) -> Result<()> {
        if self.desk().refuse_restore {
            return Err(PlatformError::Timeout);
        }
        self.focus(*token)
    }

    fn allow_foreground(&self, pid: u32) -> Result<()> {
        self.desk().allowed_foreground.push(pid);
        Ok(())
    }

    fn watch(&self, mut sink: Box<dyn FocusSink>) -> Result<()> {
        let mut slot = lock(&self.os.focus_sink);
        if slot.is_some() {
            return Err(PlatformError::AlreadyRunning);
        }
        let current = self.desk().focus_change();
        if let Some(change) = current {
            sink.focus_changed(change);
        }
        *slot = Some(sink);
        Ok(())
    }

    fn unwatch(&self) -> Result<()> {
        *lock(&self.os.focus_sink) = None;
        Ok(())
    }

    fn is_watching(&self) -> bool {
        lock(&self.os.focus_sink).is_some()
    }
}

impl SessionEvents for MockPlatform {
    fn watch_session(&self, sink: Box<dyn SessionSink>) -> Result<()> {
        let mut slot = lock(&self.os.session_sink);
        if slot.is_some() {
            return Err(PlatformError::AlreadyRunning);
        }
        *slot = Some(sink);
        Ok(())
    }

    fn unwatch_session(&self) -> Result<()> {
        *lock(&self.os.session_sink) = None;
        Ok(())
    }
}

impl ClipboardAccess for MockPlatform {
    fn read_text(&self) -> Result<Option<Sensitive<String>>> {
        Ok(self.desk().clipboard()?.text.clone().map(Sensitive::new))
    }

    fn snapshot(&self) -> Result<ClipboardSnapshot> {
        let desk = self.desk();
        let content = desk.clipboard()?;
        let flavour = |name: &str, bytes: Vec<u8>| ClipboardFlavour {
            format: ClipboardFormat::Name(name.to_owned()),
            data: Sensitive::new(bytes),
        };
        let mut flavours = Vec::new();
        if let Some(text) = &content.text {
            flavours.push(flavour(FORMAT_TEXT, text.clone().into_bytes()));
        }
        if let Some(html) = &content.html {
            flavours.push(flavour(FORMAT_HTML, html.clone().into_bytes()));
        }
        if !content.files.is_empty() {
            let list: Vec<String> = content
                .files
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            flavours.push(flavour(FORMAT_FILES, list.join("\n").into_bytes()));
        }
        Ok(ClipboardSnapshot::new(flavours, true))
    }

    fn write(&self, content: &ClipboardContent, transient: bool) -> Result<()> {
        let mut desk = self.desk();
        desk.clipboard()?;
        desk.clipboard = content.clone();
        if !transient {
            desk.clipboard_history.push(content.clone());
        }
        Ok(())
    }

    fn restore(&self, snapshot: ClipboardSnapshot) -> Result<()> {
        let mut content = ClipboardContent::default();
        for flavour in snapshot.flavours() {
            let text = String::from_utf8_lossy(flavour.data.expose()).into_owned();
            match &flavour.format {
                ClipboardFormat::Name(name) if name == FORMAT_TEXT => content.text = Some(text),
                ClipboardFormat::Name(name) if name == FORMAT_HTML => content.html = Some(text),
                ClipboardFormat::Name(name) if name == FORMAT_FILES => {
                    content.files = text.lines().map(Into::into).collect();
                }
                _ => {}
            }
        }
        let mut desk = self.desk();
        desk.clipboard()?;
        desk.clipboard = content;
        Ok(())
    }
}

impl Hotkeys for MockPlatform {
    fn register(&self, id: HotkeyId, chord: Chord) -> Result<()> {
        let mut desk = self.desk();
        let taken = desk.reserved_chords.contains(&chord)
            || desk.hotkeys.iter().any(|(i, c)| *i != id && *c == chord);
        if taken {
            return Err(PlatformError::HotkeyUnavailable);
        }
        desk.hotkeys.insert(id, chord);
        Ok(())
    }

    fn unregister(&self, id: HotkeyId) -> Result<()> {
        self.desk().hotkeys.remove(&id);
        Ok(())
    }
}

impl Permissions for MockPlatform {
    fn permission_status(&self) -> PermissionStatus {
        self.desk().permissions
    }

    fn open_settings(&self, kind: PermissionKind) -> Result<()> {
        self.desk().opened_settings.push(kind);
        Ok(())
    }
}

impl LoginItem for MockPlatform {
    fn enable(&self) -> Result<()> {
        let mut desk = self.desk();
        match desk.login_item {
            LoginItemStatus::NotFound => Err(PlatformError::os(2, "mock login item helper")),
            LoginItemStatus::RequiresApproval => Ok(()),
            LoginItemStatus::Enabled | LoginItemStatus::Disabled => {
                desk.login_item = LoginItemStatus::Enabled;
                Ok(())
            }
        }
    }

    fn disable(&self) -> Result<()> {
        let mut desk = self.desk();
        if desk.login_item != LoginItemStatus::NotFound {
            desk.login_item = LoginItemStatus::Disabled;
        }
        Ok(())
    }

    fn login_item_status(&self) -> Result<LoginItemStatus> {
        Ok(self.desk().login_item)
    }
}

impl MemoryLock for MockPlatform {
    fn lock(&self, region: MemoryRegion) -> Result<()> {
        if region.is_empty() {
            return Err(PlatformError::Invalid {
                context: "empty memory region",
            });
        }
        let mut desk = self.desk();
        if !desk.locked.contains(&region) {
            desk.locked.push(region);
        }
        Ok(())
    }

    fn unlock(&self, region: MemoryRegion) -> Result<()> {
        self.desk().locked.retain(|r| *r != region);
        Ok(())
    }
}

impl LocalEndpoint for MockPlatform {
    fn bind(&self, name: &EndpointName, policy: PeerPolicy) -> Result<Box<dyn LocalListener>> {
        self.os.endpoints.bind(name, policy, &self.identity)
    }

    fn connect(
        &self,
        name: &EndpointName,
        policy: PeerPolicy,
        _timeout: Duration,
    ) -> Result<Box<dyn LocalStream>> {
        self.os.endpoints.connect(name, policy, &self.identity)
    }
}

#[cfg(test)]
mod tests;
