//! What the listener does with one raw event of a low-level hook (architecture §4.1, §5).
//!
//! [`Pipeline`] holds everything the listener remembers between events (the keys that are
//! down, a waiting dead key, the input-method policy) and turns each raw event into zero or
//! more [`ListenerEvent`]s. It makes no system call itself: the few it needs are the methods
//! of [`Host`], which the Windows-only code implements and the tests fake.
//!
//! # The decision, in order
//!
//! For a keyboard event:
//!
//! 1. **Iamb's own event** (injected, and its extra-info decodes as a
//!    [`Marker`]): output is dropped, an injection marker is reported,
//!    a self-test marker completes the self-test. A start marker that goes by while a dead key
//!    waits is preceded by `Boundary(Other)` (see below). An injected event *without* an Iamb
//!    marker is somebody else's (a remote-control tool, an on-screen keyboard, a dictation
//!    program, a key-remapping tool) and the application treats it exactly like a pressed key,
//!    so the listener does too.
//! 2. **Every key, down or up, updates the picture of the keyboard**
//!    ([`KeyTracker`]), which is checked against the physical keyboard after a pause, after a
//!    modifier press, and whenever the system's own view of Alt and Ctrl (it sends a key as
//!    a "system key" exactly when Alt is down and Ctrl is not) contradicts it. A release does
//!    nothing else.
//! 3. A **state key** (Shift, Ctrl, Alt, the Windows key, a lock key) or an input-method mode
//!    key is not reported.
//! 4. A **`VK_PACKET`** key carries a character another program injected: it is reported as
//!    that character. While a dead key waits, what the application received is not known
//!    ([`DeadKeys::character_injected`]): `Boundary(Other)`.
//! 5. A key that is one of Iamb's **registered hotkeys** is taken by the system and never
//!    reaches the application: nothing is reported (the hotkey message follows).
//! 6. A **command chord** is `Boundary(Shortcut)`: any key while a Windows key is held, while
//!    Ctrl is held without Alt, or while Alt is held without Ctrl (the system then sends the
//!    key as a system key). `AltGr` is Ctrl and Alt together and is *not* a command: see
//!    below.
//! 7. Otherwise the layout of the application in front **translates** the key, with the
//!    listener's own dead-key state ([`DeadKeys`]). Text becomes `Char`s. A key that yields no
//!    text is `Backspace` or the `Boundary` of its kind
//!    ([`ListenerEvent::named_key`]), `Boundary(Shortcut)` when Ctrl and Alt are held, and
//!    `Boundary(Other)` for any other key. When the listener cannot know what the
//!    application received, it reports `Boundary(Other)` and never a guessed character.
//!
//! A mouse button going down is `Boundary(Click)`.
//!
//! # A dead key pressed while an expansion is pending
//!
//! A dead key types nothing, so it is reported as nothing: the next key may compose a
//! character of an abbreviation, and a boundary would clear the engine's buffer for no reason.
//! But a dead key the application still holds when Iamb's output arrives takes the first
//! injected key: the first Backspace of the deletion is spent on it (it inserts the accent and
//! deletes it again), and one character of the abbreviation stays. The typed-characters
//! hand-off cannot count it, because there is no `Char`.
//!
//! The listener knows when that is about to happen: Iamb's start marker goes by, and a dead key
//! waits (or may wait, after something the listener could not follow). The start marker comes
//! after every key the application handles before the output, and only while an expansion is
//! in flight. So the listener reports `Boundary(Other)` right before `InjectionStarted`. In
//! stream order the hand-off sees the boundary while it is still capturing and cancels
//! ([`CancelReason::Boundary`](iamb_platform::CancelReason::Boundary)); the engine marks the
//! expansion disturbed, as for any boundary typed after a trigger, and the injector sends
//! nothing, so the dead key still waits in the application and in the listener. A dead key that
//! composed with a following key before the marker (´ then e) leaves nothing waiting: the `é`
//! is an ordinary character typed after the trigger, and the expansion goes ahead.
//!
//! # `AltGr`, and why Ctrl alone is a command
//!
//! "A key is a `Char` whenever translation yields text" (§4.1) exists for `AltGr`: Windows
//! reports the `AltGr` key as Ctrl+Alt, and Ctrl+Alt+Q on a German keyboard is `@`. So a key
//! pressed with Ctrl *and* Alt goes to the layout, and is a character if the layout has one
//! there.
//!
//! Ctrl without Alt, Alt without Ctrl and the Windows key are different. The layout often
//! still has "text" for them (Ctrl+Space translates to a space, Alt+F to `f`, and the
//! translation ignores the Windows key), but the application does not receive it as typed
//! text: Alt+F opens a menu, Win+R opens a dialog, Ctrl+Space is a command in most editors and
//! is taken away entirely when another program registered it as a hotkey. Reporting such a
//! key as a character would put a character in the engine's buffer that is not in the
//! document, and the next expansion would delete one character too many. These chords are
//! therefore always `Boundary(Shortcut)`.

use std::fmt;

use iamb_platform::core_types::{BoundaryKind, KeyName, Sensitive};
use iamb_platform::{ListenerEvent, Marker, Modifiers};

use super::deadkey::{
    Composition, DeadKeys, KeyQuery, Layout, Probe, Resolved, Translation, Units,
};
use super::ime::{Context, Ime, Step, is_input_method_layout};
use super::keys::{
    KeyClass, LLKHF_INJECTED, LLKHF_UP, LLMHF_INJECTED, VK_LSHIFT, VK_PROCESSKEY, VK_RSHIFT,
    VK_SHIFT, WM_SYSKEYDOWN, classify, is_button_down,
};
use super::keystate::{KeyState, KeyTracker, RESYNC_GAP_MS};

/// One call of the low-level keyboard hook.
#[derive(Clone, Copy)]
pub(crate) struct RawKey {
    /// `KBDLLHOOKSTRUCT.vkCode`.
    pub(crate) vk: u32,
    /// `KBDLLHOOKSTRUCT.scanCode`.
    pub(crate) scan: u32,
    /// `KBDLLHOOKSTRUCT.flags`.
    pub(crate) flags: u32,
    /// `KBDLLHOOKSTRUCT.dwExtraInfo`.
    pub(crate) extra: u64,
    /// The message the hook was called with (`WM_KEYDOWN`, `WM_SYSKEYDOWN`, …).
    pub(crate) message: u32,
    /// When the hook was called, in milliseconds on any steady clock.
    pub(crate) time_ms: u64,
    /// True when the hook was called while an earlier call was still waiting for the hooks
    /// after it: the event was injected by one of those hooks.
    pub(crate) nested: bool,
}

impl fmt::Debug for RawKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawKey").finish_non_exhaustive()
    }
}

/// One call of the low-level mouse hook.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RawButton {
    /// The message the hook was called with (`WM_LBUTTONDOWN`, `WM_MOUSEMOVE`, …).
    pub(crate) message: u32,
    /// `MSLLHOOKSTRUCT.flags`.
    pub(crate) flags: u32,
    /// `MSLLHOOKSTRUCT.dwExtraInfo`.
    pub(crate) extra: u64,
}

/// The window in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Foreground {
    /// Its thread; 0 when there is none.
    pub(crate) thread: u32,
    /// The keyboard layout handle its keys are translated with.
    pub(crate) layout: usize,
}

/// The thin calls to the system, and the way out for what the listener reports.
pub(crate) trait Host {
    /// The window in front and its layout.
    fn foreground(&mut self) -> Foreground;

    /// What `key` produces on `layout`, without changing the system's keyboard state.
    fn translate(&mut self, layout: usize, key: &KeyQuery) -> Translation;

    /// The dead-key table entry of `layout` for `dead` followed by `base`.
    fn compose(&mut self, layout: usize, dead: u16, base: u16) -> Composition;

    /// A dead key of `layout`, if it has any and it is known.
    fn probe(&mut self, layout: usize) -> Option<Probe>;

    /// Whether the key `vk` is physically down right now. Not valid for the key of the event
    /// in progress.
    fn physically_down(&mut self, vk: u32) -> bool;

    /// Asks the input method of the window in front whether it is open, without waiting; the
    /// answer comes back through [`Pipeline::input_method_answered`] with `id`. False when
    /// the question could not be sent.
    fn ask_input_method(&mut self, id: u32) -> bool;

    /// Takes the next sequence number, builds the event with it and delivers it.
    fn report<F: FnOnce(u64) -> ListenerEvent>(&mut self, event: F);

    /// The self-test event with this nonce went through the hook.
    fn self_test_seen(&mut self, nonce: u32);
}

/// A hotkey Iamb has registered with the system: the key and the exact modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HotkeyBinding {
    /// The modifiers of the chord.
    pub(crate) modifiers: Modifiers,
    /// The virtual key of the chord.
    pub(crate) vk: u32,
}

impl HotkeyBinding {
    /// True when pressing `vk` with `keys` held triggers this hotkey: the system requires
    /// exactly the registered modifiers.
    fn matches(self, keys: &KeyState, vk: u32) -> bool {
        self.vk == vk
            && self.modifiers.contains(Modifiers::CTRL) == keys.ctrl()
            && self.modifiers.contains(Modifiers::ALT) == keys.alt()
            && self.modifiers.contains(Modifiers::SHIFT) == keys.shift()
            && self.modifiers.contains(Modifiers::META) == keys.win()
    }
}

/// [`Host`] for one layout, as the dead-key logic wants it.
struct OnLayout<'a, H> {
    host: &'a mut H,
    layout: usize,
}

impl<H: Host> Layout for OnLayout<'_, H> {
    fn translate(&mut self, key: &KeyQuery) -> Translation {
        self.host.translate(self.layout, key)
    }

    fn compose(&mut self, dead: u16, base: u16) -> Composition {
        self.host.compose(self.layout, dead, base)
    }

    fn probe(&mut self) -> Option<Probe> {
        self.host.probe(self.layout)
    }
}

/// Everything the listener remembers between two events.
pub(crate) struct Pipeline {
    keys: KeyTracker,
    dead: DeadKeys,
    ime: Ime,
    /// The layout the dead-key state belongs to.
    layout: usize,
    /// When the previous keyboard event that was not Iamb's own went by.
    last_key_ms: u64,
    /// The injection marker reported last.
    last_marker: Option<Marker>,
    /// The first half of a character outside the Basic Multilingual Plane that another
    /// program is injecting, while `high_waiting`. The slot is overwritten with zero when the
    /// half is used or dropped and when the pipeline is dropped (the listener stops), so the
    /// half does not stay behind in the listener's memory (the 2.8 review's finding 9).
    high_surrogate: Sensitive<u16>,
    high_waiting: bool,
    /// Compare the picture of the keyboard with the physical one at the next event, whatever
    /// the clock says.
    resync_due: bool,
    hotkeys: Vec<HotkeyBinding>,
}

impl fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pipeline")
            .field("keys", &self.keys)
            .field("dead", &self.dead)
            .field("composing", &self.ime.reported())
            .field("hotkeys", &self.hotkeys.len())
            .finish_non_exhaustive()
    }
}

impl Drop for Pipeline {
    /// The listener stopped (the hook thread drops its pipeline).
    fn drop(&mut self) {
        self.forget_on_stop();
    }
}

fn boundary(host: &mut impl Host, kind: BoundaryKind) {
    host.report(|seq| ListenerEvent::Boundary { kind, seq });
}

impl Pipeline {
    /// A pipeline for a hook installed at `now_ms`. `composing` is the last `Composing` value
    /// the listener reported before (false for the first installation).
    pub(crate) fn new(composing: bool, now_ms: u64) -> Self {
        Self {
            keys: KeyTracker::default(),
            dead: DeadKeys::default(),
            ime: Ime::new(composing),
            layout: 0,
            last_key_ms: now_ms,
            last_marker: None,
            high_surrogate: Sensitive::new(0),
            high_waiting: false,
            resync_due: false,
            hotkeys: Vec::new(),
        }
    }

    /// Starts the picture of the keyboard from the physical one. `caps` is the Caps Lock
    /// toggle.
    pub(crate) fn seed(&mut self, host: &mut impl Host, caps: bool, now_ms: u64) {
        self.keys.seed(|vk| host.physically_down(vk), caps);
        self.last_key_ms = now_ms;
        self.resync_due = false;
    }

    /// The hook lost events or was installed again: what the listener remembered about the
    /// stream is no longer true. The picture of the keyboard is corrected with the next key.
    pub(crate) fn lost(&mut self) {
        self.dead.reset();
        self.ime.reset();
        self.forget_high_surrogate();
        self.resync_due = true;
    }

    /// The listener stops: the waiting dead character and the half of a surrogate pair are
    /// overwritten (the 2.8 review's finding 9). Also run when the pipeline is dropped.
    pub(crate) fn forget_on_stop(&mut self) {
        self.dead.reset();
        self.forget_high_surrogate();
    }

    /// Takes the waiting half of a surrogate pair, if any, and overwrites its slot.
    fn take_high_surrogate(&mut self) -> Option<u16> {
        let high = self.high_waiting.then(|| *self.high_surrogate.expose());
        self.forget_high_surrogate();
        high
    }

    /// Overwrites the slot of the half of a surrogate pair with zero; nothing waits after.
    fn forget_high_surrogate(&mut self) {
        *self.high_surrogate.expose_mut() = 0;
        std::hint::black_box(self.high_surrogate.expose_mut());
        self.high_waiting = false;
    }

    /// The slot of the half of a surrogate pair, as it is in memory (tests).
    #[cfg(test)]
    pub(crate) const fn high_surrogate_slot(&self) -> u16 {
        *self.high_surrogate.expose()
    }

    /// The dead character the listener holds, if one waits (tests).
    #[cfg(test)]
    pub(crate) fn dead_key_slot(&self) -> Option<u16> {
        self.dead.pending()
    }

    /// The last `Composing` value reported.
    #[cfg(test)]
    pub(crate) const fn composing(&self) -> bool {
        self.ime.reported()
    }

    /// The hotkeys the system currently holds for Iamb.
    pub(crate) fn set_hotkeys(&mut self, hotkeys: Vec<HotkeyBinding>) {
        self.hotkeys = hotkeys;
    }

    fn apply(&mut self, step: Step, host: &mut impl Host) {
        if let Some(on) = step.report {
            host.report(|seq| ListenerEvent::Composing { on, seq });
        }
        if let Some(id) = step.ask
            && !host.ask_input_method(id)
        {
            self.ime.unasked(id);
        }
    }

    /// The input method answered question `id`.
    pub(crate) fn input_method_answered(&mut self, id: u32, open: bool, host: &mut impl Host) {
        let step = self.ime.answer(id, open);
        self.apply(step, host);
    }

    /// One call of the mouse hook.
    pub(crate) fn on_button(&mut self, raw: &RawButton, host: &mut impl Host) {
        if !is_button_down(raw.message) {
            return;
        }
        if raw.flags & LLMHF_INJECTED != 0 && Marker::decode(raw.extra).is_some() {
            // Iamb's own: it never reacts to its own output (§5).
            return;
        }
        self.forget_high_surrogate();
        // A click can switch an input method on (its mode button) or move the focus.
        let step = self.ime.unsettle();
        self.apply(step, host);
        boundary(host, BoundaryKind::Click);
    }

    /// One call of the keyboard hook.
    pub(crate) fn on_key(&mut self, raw: &RawKey, host: &mut impl Host) {
        let down = raw.flags & LLKHF_UP == 0;
        if raw.flags & LLKHF_INJECTED != 0
            && let Some(marker) = Marker::decode(raw.extra)
        {
            // Injected keys change what the system reports for the keys after them, Iamb's
            // own included (a paste holds Ctrl), so the picture follows them too.
            self.keys.event(raw.vk, down);
            self.own(marker, raw.vk, down, host);
            return;
        }

        if !raw.nested {
            // The physical state of every key but this event's is settled.
            let paused = raw.time_ms.saturating_sub(self.last_key_ms) >= RESYNC_GAP_MS;
            if paused || std::mem::take(&mut self.resync_due) {
                self.keys.resync(raw.vk, |vk| host.physically_down(vk));
            } else {
                self.keys.verify(|vk| host.physically_down(vk));
            }
        }
        self.last_key_ms = raw.time_ms;
        self.keys.event(raw.vk, down);
        if !down {
            return;
        }

        match classify(raw.vk) {
            KeyClass::NoKey => {}
            KeyClass::State => {
                // Shift alone switches nothing; any other modifier or lock key may be the
                // start of the chord that switches an input method.
                if !matches!(raw.vk, VK_SHIFT | VK_LSHIFT | VK_RSHIFT) {
                    let step = self.ime.unsettle();
                    self.apply(step, host);
                }
            }
            KeyClass::InputMethod => {
                let step = if raw.vk == VK_PROCESSKEY {
                    self.ime.process_key()
                } else {
                    self.ime.unsettle()
                };
                self.apply(step, host);
            }
            KeyClass::Packet => self.packet(raw.scan, host),
            class @ (KeyClass::Named(_) | KeyClass::Other) => self.pressed(raw, class, host),
        }
    }

    /// An event Iamb injected itself.
    fn own(&mut self, marker: Marker, vk: u32, down: bool, host: &mut impl Host) {
        match marker {
            Marker::Output => {
                // An injected key goes through the application's translation like a pressed
                // one and uses up a waiting dead key there, or does not: the listener cannot
                // tell. That includes Iamb's own characters (`VK_PACKET`): what the system
                // does with a waiting dead key then is not known either (see `deadkey`).
                if down && classify(vk) != KeyClass::State {
                    self.dead.disturb();
                }
            }
            Marker::SelfTest(nonce) => host.self_test_seen(nonce),
            Marker::InjectionStart(_) | Marker::InjectionEnd(_) => {
                // An injector may post its no-op as a press and a release that both carry
                // the marker: the marker is reported once.
                if self.last_marker != Some(marker) && marker.listener_event(0).is_some() {
                    self.last_marker = Some(marker);
                    if matches!(marker, Marker::InjectionStart(_)) && self.dead.is_waiting() {
                        // The application would spend the first injected key on the dead key:
                        // the boundary cancels the expansion (see the module documentation).
                        boundary(host, BoundaryKind::Other);
                    }
                    host.report(|seq| {
                        marker
                            .listener_event(seq)
                            .unwrap_or(ListenerEvent::Overflow)
                    });
                }
            }
        }
    }

    /// A character another program injected (`VK_PACKET`): one UTF-16 unit in the scan code.
    /// The two halves of a character outside the Basic Multilingual Plane are one character:
    /// it is decided when the second half arrives.
    fn packet(&mut self, scan: u32, host: &mut impl Host) {
        let unit = u16::try_from(scan & 0xFFFF).unwrap_or(0);
        let high = self.take_high_surrogate();
        let ch = match unit {
            0xD800..=0xDBFF => {
                *self.high_surrogate.expose_mut() = unit;
                self.high_waiting = true;
                return;
            }
            0xDC00..=0xDFFF => {
                let Some(high) = high else { return };
                char::decode_utf16([high, unit]).next().and_then(Result::ok)
            }
            _ => char::from_u32(u32::from(unit)),
        };
        if self.dead.character_injected() {
            // A dead key waits: whether the application received the character alone, after
            // the dead character, or with the dead key still waiting, is not known.
            boundary(host, BoundaryKind::Other);
            return;
        }
        match ch {
            Some('\r' | '\n') => named(host, KeyName::Enter),
            Some('\t') => named(host, KeyName::Tab),
            Some('\u{8}') => named(host, KeyName::Backspace),
            Some('\u{1b}') => named(host, KeyName::Escape),
            Some(ch) if !ch.is_control() => host.report(|seq| ListenerEvent::char(ch, seq)),
            Some(_) | None => boundary(host, BoundaryKind::Other),
        }
    }

    /// A key that is not a state key went down.
    fn pressed(&mut self, raw: &RawKey, class: KeyClass, host: &mut impl Host) {
        // The system sends a key as a system key exactly when it has Alt down and Ctrl up.
        // If the picture says otherwise, the picture is wrong: correct it now.
        let system_key = raw.message == WM_SYSKEYDOWN;
        let pictured = self.keys.state();
        if system_key != (pictured.alt() && !pictured.ctrl()) && !raw.nested {
            self.keys.resync(raw.vk, |vk| host.physically_down(vk));
        }
        let keys = *self.keys.state();
        if self
            .hotkeys
            .iter()
            .any(|hotkey| hotkey.matches(&keys, raw.vk))
        {
            // The system takes the key for the hotkey; the application never sees it.
            return;
        }

        let foreground = host.foreground();
        if foreground.layout != self.layout {
            // The system keeps a waiting dead key per layout.
            self.layout = foreground.layout;
            self.dead.reset();
        }
        let command = !keys.types_text() || system_key;
        let context = is_input_method_layout(foreground.layout).then_some(Context {
            thread: foreground.thread,
            layout: foreground.layout,
        });
        let step = self.ime.key(context, !command);
        self.apply(step, host);

        if command {
            // Whether the application's translation saw this key, and used up a waiting dead
            // key, depends on the application.
            self.dead.disturb();
            boundary(host, BoundaryKind::Shortcut);
            return;
        }

        let query = KeyQuery {
            vk: raw.vk,
            scan: raw.scan,
            keys,
        };
        let mut layout = OnLayout {
            host: &mut *host,
            layout: self.layout,
        };
        match self.dead.resolve(&mut layout, &query) {
            Resolved::Held => {}
            Resolved::Unknown => boundary(host, BoundaryKind::Other),
            Resolved::Nothing => untyped(host, class, keys.chord()),
            Resolved::Text(units) => typed(host, units, class, keys.chord()),
        }
    }
}

fn named(host: &mut impl Host, key: KeyName) {
    host.report(|seq| ListenerEvent::named_key(key, seq));
}

/// Reports a key press that produced no text.
fn untyped(host: &mut impl Host, class: KeyClass, chord: bool) {
    match class {
        // Ctrl and Alt are held and the layout has no character there.
        _ if chord => boundary(host, BoundaryKind::Shortcut),
        // A Space key that types nothing is not a space.
        KeyClass::Named(key) if key != KeyName::Space => named(host, key),
        _ => boundary(host, BoundaryKind::Other),
    }
}

/// Reports the units a key press delivered: characters, and the key itself for a control
/// character (Enter delivers U+000D, Backspace U+0008).
fn typed(host: &mut impl Host, units: Units, class: KeyClass, chord: bool) {
    let mut key_reported = false;
    for decoded in char::decode_utf16(units.as_slice().iter().copied()) {
        match decoded {
            Ok(ch) if !ch.is_control() => host.report(|seq| ListenerEvent::char(ch, seq)),
            _ if !key_reported => {
                key_reported = true;
                untyped(host, class, chord);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
