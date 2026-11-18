//! Synthetic raw events through the pipeline, against a fake layout and a fake keyboard.

use iamb_platform::core_types::ExpansionId;
use iamb_platform::{CancelReason, HandoffStatus, SeqSource, TypedHandoff};

use super::super::deadkey::testing::{FakeLayout, Raw, held};
use super::super::keys::{
    VK_BACK, VK_CAPITAL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F10, VK_HOME, VK_KANA,
    VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LWIN, VK_NEXT, VK_NONE, VK_NUMLOCK, VK_PACKET, VK_PRIOR,
    VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_SPACE, VK_TAB, VK_UNASSIGNED, VK_UP, WM_KEYDOWN,
    WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
};
use super::*;

const A: u32 = 0x41;
const C: u32 = 0x43;
const E: u32 = 0x45;
const F: u32 = 0x46;
const H: u32 = 0x48;
const I: u32 = 0x49;
const K: u32 = 0x4B;
const Q: u32 = 0x51;
const R: u32 = 0x52;
const ACUTE: u32 = 0xDD;
const CIRCUMFLEX: u32 = 0xDC;
const F22: u32 = 0x85;

const US: usize = 0x0409_0409;
const GERMAN: usize = 0x0407_0407;
const JAPANESE: usize = 0x0411_0411;

/// What the listener reported, printable: the test data is nobody's typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    Char(char),
    Backspace,
    Boundary(BoundaryKind),
    Composing(bool),
    Started(u64),
    Finished(u64),
    Other,
}

use BoundaryKind::{Arrow, Click, Enter, Escape, Navigation, Shortcut, Tab};
use Seen::{Backspace, Boundary, Char, Composing, Finished, Started};

const OTHER: Seen = Boundary(BoundaryKind::Other);

fn chars(text: &str) -> Vec<Seen> {
    text.chars().map(Char).collect()
}

/// The letters, Space and the keys that deliver a control character.
fn common_keys() -> Vec<((u32, bool, bool), Raw)> {
    let mut keys = Vec::new();
    for (vk, lower) in (A..=0x5A).zip('a'..='z') {
        keys.push(((vk, false, false), Raw::Char(lower)));
        keys.push(((vk, true, false), Raw::Char(lower.to_ascii_uppercase())));
    }
    for shift in [false, true] {
        keys.push(((VK_SPACE, shift, false), Raw::Char(' ')));
        keys.push(((VK_RETURN, shift, false), Raw::Char('\r')));
        keys.push(((VK_TAB, shift, false), Raw::Char('\t')));
        keys.push(((VK_ESCAPE, shift, false), Raw::Char('\u{1b}')));
        keys.push(((VK_BACK, shift, false), Raw::Char('\u{8}')));
    }
    keys
}

/// A layout without dead keys or an `AltGr` level.
fn us() -> FakeLayout {
    FakeLayout::new(&common_keys(), &[])
}

/// A layout with an `AltGr` level and two dead keys.
fn german() -> FakeLayout {
    let mut keys = common_keys();
    keys.extend([
        ((Q, false, true), Raw::Char('@')),
        ((E, false, true), Raw::Char('€')),
        ((ACUTE, false, false), Raw::Dead('´')),
        ((ACUTE, true, false), Raw::Dead('`')),
        ((CIRCUMFLEX, false, false), Raw::Dead('^')),
    ]);
    FakeLayout::new(
        &keys,
        &[
            ('´', 'e', 'é', false),
            ('´', 'E', 'É', false),
            ('´', ' ', '´', false),
            ('`', 'e', 'è', false),
            ('`', ' ', '`', false),
            ('^', 'e', 'ê', false),
            ('^', ' ', '^', false),
        ],
    )
}

struct FakeHost {
    layout: FakeLayout,
    foreground: Foreground,
    /// The keys that are physically down.
    physical: Vec<u32>,
    /// How often the physical keyboard was asked about.
    physical_questions: usize,
    seq: SeqSource,
    events: Vec<ListenerEvent>,
    self_tests: Vec<u32>,
    /// The questions sent to the input method.
    asked: Vec<u32>,
    can_ask: bool,
}

impl Host for FakeHost {
    fn foreground(&mut self) -> Foreground {
        self.foreground
    }

    fn translate(&mut self, layout: usize, key: &KeyQuery) -> Translation {
        assert_eq!(layout, self.foreground.layout);
        if key.keys.ctrl() != key.keys.alt() {
            return Translation::None;
        }
        // Caps Lock shifts the letters.
        let letter = (A..=0x5A).contains(&key.vk);
        let shift = key.keys.shift() != (key.keys.caps() && letter);
        let effective = KeyQuery {
            keys: held(shift, key.keys.ctrl() && key.keys.alt()),
            ..*key
        };
        self.layout.translate(&effective)
    }

    fn compose(&mut self, layout: usize, dead: u16, base: u16) -> Composition {
        assert_eq!(layout, self.foreground.layout);
        self.layout.compose(dead, base)
    }

    fn probe(&mut self, layout: usize) -> Option<Probe> {
        assert_eq!(layout, self.foreground.layout);
        self.layout.probe()
    }

    fn physically_down(&mut self, vk: u32) -> bool {
        self.physical_questions += 1;
        self.physical.contains(&vk)
    }

    fn ask_input_method(&mut self, id: u32) -> bool {
        if self.can_ask {
            self.asked.push(id);
        }
        self.can_ask
    }

    fn report<F: FnOnce(u64) -> ListenerEvent>(&mut self, event: F) {
        let event = event(self.seq.next());
        self.events.push(event);
    }

    fn self_test_seen(&mut self, nonce: u32) {
        self.self_tests.push(nonce);
    }
}

/// A pipeline, its fake host and a clock.
struct Rig {
    pipeline: Pipeline,
    host: FakeHost,
    now: u64,
    /// The sequence number of the last event taken out with [`Rig::seen`].
    last_seq: u64,
}

impl Rig {
    fn new(layout: FakeLayout, handle: usize) -> Self {
        Self {
            pipeline: Pipeline::new(false, 0),
            host: FakeHost {
                layout,
                foreground: Foreground {
                    thread: 1,
                    layout: handle,
                },
                physical: Vec::new(),
                physical_questions: 0,
                seq: SeqSource::new(),
                events: Vec::new(),
                self_tests: Vec::new(),
                asked: Vec::new(),
                can_ask: true,
            },
            now: 0,
            last_seq: 0,
        }
    }

    fn us() -> Self {
        Self::new(us(), US)
    }

    fn german() -> Self {
        Self::new(german(), GERMAN)
    }

    fn raw(&self, vk: u32, down: bool) -> RawKey {
        RawKey {
            vk,
            scan: 0,
            flags: if down { 0 } else { LLKHF_UP },
            extra: 0,
            message: if down { WM_KEYDOWN } else { WM_KEYUP },
            time_ms: self.now,
            nested: false,
        }
    }

    /// Feeds one event; the system then updates the physical keyboard, unless a hook after
    /// the listener's swallowed the event.
    fn feed(&mut self, raw: RawKey, swallowed: bool) {
        self.now += 10;
        let raw = RawKey {
            time_ms: self.now,
            ..raw
        };
        self.pipeline.on_key(&raw, &mut self.host);
        if !swallowed {
            self.host.physical.retain(|vk| *vk != raw.vk);
            if raw.flags & LLKHF_UP == 0 {
                self.host.physical.push(raw.vk);
            }
        }
    }

    fn down(&mut self, vk: u32) {
        self.feed(self.raw(vk, true), false);
    }

    fn up(&mut self, vk: u32) {
        self.feed(self.raw(vk, false), false);
    }

    fn tap(&mut self, vk: u32) {
        self.down(vk);
        self.up(vk);
    }

    /// A key pressed while Alt is held without Ctrl arrives as a system key.
    fn sys_tap(&mut self, vk: u32) {
        self.feed(
            RawKey {
                message: WM_SYSKEYDOWN,
                ..self.raw(vk, true)
            },
            false,
        );
        self.feed(
            RawKey {
                message: WM_SYSKEYUP,
                ..self.raw(vk, false)
            },
            false,
        );
    }

    /// Presses and releases `vk` while `modifiers` are held.
    fn chord(&mut self, modifiers: &[u32], vk: u32) {
        for modifier in modifiers {
            self.down(*modifier);
        }
        self.tap(vk);
        for modifier in modifiers.iter().rev() {
            self.up(*modifier);
        }
    }

    /// An injected event that carries `extra`.
    fn injected(&mut self, vk: u32, down: bool, extra: u64) {
        self.feed(
            RawKey {
                flags: LLKHF_INJECTED | if down { 0 } else { LLKHF_UP },
                extra,
                ..self.raw(vk, down)
            },
            false,
        );
    }

    /// A character another program injects: a press and a release of `VK_PACKET`.
    fn packet(&mut self, unit: u16) {
        for down in [true, false] {
            self.feed(
                RawKey {
                    scan: u32::from(unit),
                    flags: LLKHF_INJECTED | if down { 0 } else { LLKHF_UP },
                    ..self.raw(VK_PACKET, down)
                },
                false,
            );
        }
    }

    fn click(&mut self, message: u32) {
        self.pipeline.on_button(
            &RawButton {
                message,
                flags: 0,
                extra: 0,
            },
            &mut self.host,
        );
    }

    /// What was reported since the last call. Checks that every event took the next
    /// sequence number.
    fn seen(&mut self) -> Vec<Seen> {
        let events = std::mem::take(&mut self.host.events);
        events
            .into_iter()
            .map(|event| {
                self.last_seq += 1;
                assert_eq!(
                    event.seq(),
                    Some(self.last_seq),
                    "sequence numbers have no gap"
                );
                match event {
                    ListenerEvent::Char { ch, .. } => Char(*ch.expose()),
                    ListenerEvent::Backspace { .. } => Backspace,
                    ListenerEvent::Boundary { kind, .. } => Boundary(kind),
                    ListenerEvent::Composing { on, .. } => Composing(on),
                    ListenerEvent::InjectionStarted { expansion, .. } => Started(expansion.raw()),
                    ListenerEvent::InjectionFinished { expansion, .. } => Finished(expansion.raw()),
                    ListenerEvent::Hotkey { .. } | ListenerEvent::Overflow => Seen::Other,
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Typing
// ---------------------------------------------------------------------------------------------

#[test]
fn keys_that_type_are_characters_and_releases_are_nothing() {
    let mut rig = Rig::us();
    rig.tap(H);
    rig.tap(I);
    rig.tap(VK_SPACE);
    assert_eq!(rig.seen(), chars("hi "));
    // Rollover: the second key goes down before the first is released.
    rig.down(H);
    rig.down(I);
    rig.up(H);
    rig.up(I);
    assert_eq!(rig.seen(), chars("hi"));
    assert_eq!(rig.host.seq.last(), 5, "one number per reported event");
}

#[test]
fn a_repeated_key_down_is_a_key() {
    let mut rig = Rig::us();
    for _ in 0..4 {
        rig.down(A);
    }
    rig.up(A);
    assert_eq!(rig.seen(), chars("aaaa"));
    for _ in 0..3 {
        rig.down(VK_BACK);
    }
    rig.up(VK_BACK);
    assert_eq!(rig.seen(), [Backspace; 3]);
}

#[test]
fn shift_and_caps_lock_choose_the_character() {
    let mut rig = Rig::us();
    rig.chord(&[VK_LSHIFT], A);
    rig.chord(&[VK_RSHIFT], A);
    rig.tap(A);
    assert_eq!(rig.seen(), chars("AAa"));
    rig.tap(VK_CAPITAL);
    rig.tap(A);
    rig.chord(&[VK_LSHIFT], A);
    rig.tap(VK_CAPITAL);
    rig.tap(A);
    assert_eq!(rig.seen(), chars("Aaa"));
    // Shift, Ctrl, Alt, the Windows key and the lock keys are never reported themselves.
    for vk in [VK_LSHIFT, VK_LCONTROL, VK_LMENU, VK_LWIN, VK_NUMLOCK] {
        rig.tap(vk);
    }
    // Neither are non-keys.
    rig.tap(VK_NONE);
    rig.tap(0);
    assert_eq!(rig.seen(), []);
}

#[test]
fn altgr_is_a_character_modifier() {
    let mut rig = Rig::german();
    // The AltGr key arrives as a left Ctrl followed by the right Alt.
    rig.chord(&[VK_LCONTROL, VK_RMENU], Q);
    rig.chord(&[VK_LCONTROL, VK_RMENU], E);
    // Ctrl+Alt typed with the left keys means the same to the layout.
    rig.chord(&[VK_LCONTROL, VK_LMENU], E);
    rig.tap(E);
    assert_eq!(rig.seen(), chars("@€€e"));
    // A key without an AltGr level is a Ctrl+Alt shortcut.
    rig.chord(&[VK_LCONTROL, VK_RMENU], K);
    assert_eq!(rig.seen(), [Boundary(Shortcut)]);
}

#[test]
fn command_chords_are_shortcuts_even_when_the_layout_has_text_for_them() {
    let mut rig = Rig::us();
    // Ctrl+C, Ctrl+Space, Ctrl+Backspace, Ctrl+Enter.
    for vk in [C, VK_SPACE, VK_BACK, VK_RETURN] {
        rig.chord(&[VK_LCONTROL], vk);
        rig.chord(&[VK_RCONTROL], vk);
    }
    assert_eq!(rig.seen(), [Boundary(Shortcut); 8]);
    // Win+R, Win+Shift+S.
    rig.chord(&[VK_LWIN], R);
    rig.chord(&[VK_LWIN, VK_LSHIFT], 0x53);
    assert_eq!(rig.seen(), [Boundary(Shortcut); 2]);
    // Alt+F arrives as a system key.
    rig.down(VK_LMENU);
    rig.sys_tap(F);
    rig.up(VK_LMENU);
    assert_eq!(rig.seen(), [Boundary(Shortcut)]);
    // The system says "system key" although the listener did not see Alt go down.
    rig.sys_tap(F);
    rig.sys_tap(VK_F10);
    assert_eq!(rig.seen(), [Boundary(Shortcut); 2]);
    // Ctrl+Alt+K on a layout without an AltGr level.
    rig.chord(&[VK_LCONTROL, VK_LMENU], K);
    // Ctrl+Shift+A.
    rig.chord(&[VK_LCONTROL, VK_LSHIFT], A);
    assert_eq!(rig.seen(), [Boundary(Shortcut); 2]);
    // And afterwards keys type again.
    rig.tap(A);
    assert_eq!(rig.seen(), chars("a"));
}

#[test]
fn keys_without_text_are_backspace_or_the_boundary_of_their_kind() {
    let mut rig = Rig::us();
    let table: [(u32, Seen); 16] = [
        (VK_RETURN, Boundary(Enter)),
        (VK_TAB, Boundary(Tab)),
        (VK_ESCAPE, Boundary(Escape)),
        (VK_BACK, Backspace),
        (VK_LEFT, Boundary(Arrow)),
        (VK_RIGHT, Boundary(Arrow)),
        (VK_UP, Boundary(Arrow)),
        (VK_DOWN, Boundary(Arrow)),
        (VK_HOME, Boundary(Navigation)),
        (VK_END, Boundary(Navigation)),
        (VK_PRIOR, Boundary(Navigation)),
        (VK_NEXT, Boundary(Navigation)),
        (VK_DELETE, OTHER),
        (VK_F1, OTHER),
        (0x2D, OTHER),          // Insert
        (VK_UNASSIGNED, OTHER), // a key nobody has
    ];
    for (vk, expected) in table {
        rig.tap(vk);
        assert_eq!(rig.seen(), [expected], "{vk:#x}");
    }
    // Shift does not make a command of them.
    for (vk, expected) in [
        (VK_RETURN, Boundary(Enter)),
        (VK_TAB, Boundary(Tab)),
        (VK_BACK, Backspace),
        (VK_LEFT, Boundary(Arrow)),
    ] {
        rig.chord(&[VK_LSHIFT], vk);
        assert_eq!(rig.seen(), [expected], "Shift+{vk:#x}");
    }
}

#[test]
fn a_space_key_that_types_nothing_is_not_a_space() {
    let mut rig = Rig::new(
        FakeLayout::new(&[((A, false, false), Raw::Char('a'))], &[]),
        US,
    );
    rig.tap(VK_SPACE);
    rig.tap(VK_RETURN);
    rig.tap(VK_BACK);
    assert_eq!(rig.seen(), [OTHER, Boundary(Enter), Backspace]);
}

// ---------------------------------------------------------------------------------------------
// Dead keys
// ---------------------------------------------------------------------------------------------

#[test]
fn a_dead_key_reports_nothing_and_composes_with_the_next_key() {
    let mut rig = Rig::german();
    rig.tap(ACUTE);
    assert_eq!(rig.seen(), []);
    rig.tap(E);
    rig.tap(CIRCUMFLEX);
    rig.tap(E);
    rig.chord(&[VK_LSHIFT], ACUTE);
    rig.tap(E);
    rig.tap(ACUTE);
    rig.chord(&[VK_LSHIFT], E);
    assert_eq!(rig.seen(), chars("éêèÉ"));
}

#[test]
fn a_pair_that_does_not_compose_is_two_characters() {
    let mut rig = Rig::german();
    rig.tap(ACUTE);
    rig.tap(Q);
    assert_eq!(rig.seen(), chars("´q"));
    rig.tap(ACUTE);
    rig.tap(ACUTE);
    assert_eq!(rig.seen(), chars("´´"));
    rig.tap(CIRCUMFLEX);
    rig.tap(VK_SPACE);
    assert_eq!(rig.seen(), chars("^"));
    // A dead key, then an AltGr character.
    rig.tap(ACUTE);
    rig.chord(&[VK_LCONTROL, VK_RMENU], Q);
    assert_eq!(rig.seen(), chars("´@"));
}

#[test]
fn keys_with_a_control_character_flush_a_waiting_dead_key() {
    let mut rig = Rig::german();
    // The application inserts the accent and then deletes it again.
    rig.tap(ACUTE);
    rig.tap(VK_BACK);
    assert_eq!(rig.seen(), [Char('´'), Backspace]);
    rig.tap(CIRCUMFLEX);
    rig.tap(VK_RETURN);
    assert_eq!(rig.seen(), [Char('^'), Boundary(Enter)]);
    rig.tap(ACUTE);
    rig.tap(VK_ESCAPE);
    assert_eq!(rig.seen(), [Char('´'), Boundary(Escape)]);
    rig.tap(E);
    assert_eq!(rig.seen(), chars("e"));
}

#[test]
fn keys_without_a_character_leave_a_dead_key_waiting() {
    let mut rig = Rig::german();
    rig.tap(CIRCUMFLEX);
    rig.tap(VK_LEFT);
    rig.tap(VK_F1);
    rig.tap(VK_LSHIFT);
    rig.click(WM_LBUTTONDOWN);
    rig.tap(E);
    assert_eq!(
        rig.seen(),
        [Boundary(Arrow), OTHER, Boundary(Click), Char('ê')]
    );
}

#[test]
fn a_command_in_the_middle_of_a_dead_key_sequence_is_not_guessed() {
    let mut rig = Rig::german();
    rig.tap(ACUTE);
    rig.chord(&[VK_LCONTROL], C);
    // Whether the application's dead key survived the command cannot be known.
    rig.tap(E);
    rig.tap(E);
    assert_eq!(rig.seen(), [Boundary(Shortcut), OTHER, Char('e')]);
}

#[test]
fn a_change_of_layout_forgets_a_waiting_dead_key() {
    let mut rig = Rig::german();
    rig.tap(ACUTE);
    // Another window, with another layout handle (the fake translates alike).
    rig.host.foreground = Foreground {
        thread: 2,
        layout: 0x0C07_0407,
    };
    rig.tap(E);
    assert_eq!(rig.seen(), chars("e"));
}

#[test]
fn without_the_layouts_table_a_dead_key_sequence_is_a_boundary() {
    let mut layout = german();
    layout.table = None;
    let mut rig = Rig::new(layout, GERMAN);
    rig.tap(A);
    rig.tap(ACUTE);
    rig.tap(E);
    rig.tap(E);
    assert_eq!(rig.seen(), [Char('a'), OTHER, Char('e')]);
}

// ---------------------------------------------------------------------------------------------
// Iamb's own events and other injected input
// ---------------------------------------------------------------------------------------------

#[test]
fn iambs_own_output_is_dropped_and_its_markers_are_reported() {
    let mut rig = Rig::us();
    let output = Marker::Output.encode();
    let expansion = ExpansionId::from_raw(7);
    let start = Marker::InjectionStart(expansion).encode();
    let end = Marker::InjectionEnd(expansion).encode();

    rig.tap(H);
    rig.injected(VK_UNASSIGNED, false, start);
    for vk in [VK_BACK, VK_BACK, A, VK_RETURN, VK_LEFT] {
        rig.injected(vk, true, output);
        rig.injected(vk, false, output);
    }
    rig.injected(VK_UNASSIGNED, false, end);
    rig.tap(I);
    assert_eq!(rig.seen(), [Char('h'), Started(7), Finished(7), Char('i')]);
}

#[test]
fn a_marker_posted_as_press_and_release_is_reported_once() {
    let mut rig = Rig::us();
    for id in [1, 2] {
        let expansion = ExpansionId::from_raw(id);
        for marker in [
            Marker::InjectionStart(expansion),
            Marker::InjectionEnd(expansion),
        ] {
            rig.injected(VK_UNASSIGNED, true, marker.encode());
            rig.injected(VK_UNASSIGNED, false, marker.encode());
        }
    }
    assert_eq!(
        rig.seen(),
        [Started(1), Finished(1), Started(2), Finished(2)]
    );
}

#[test]
fn a_self_test_marker_completes_the_self_test_and_reports_nothing() {
    let mut rig = Rig::us();
    rig.injected(VK_UNASSIGNED, false, Marker::SelfTest(0xFEED).encode());
    assert_eq!(rig.host.self_tests, [0xFEED]);
    assert_eq!(rig.seen(), []);
    assert_eq!(rig.host.seq.last(), 0, "no sequence number was taken");
}

#[test]
fn injected_input_without_an_iamb_marker_is_real_typing() {
    let mut rig = Rig::us();
    // A remote-control tool: no extra information, or its own.
    rig.injected(A, true, 0);
    rig.injected(A, false, 0);
    rig.injected(VK_RETURN, true, 0xDEAD_BEEF);
    rig.injected(VK_RETURN, false, 0xDEAD_BEEF);
    assert_eq!(rig.seen(), [Char('a'), Boundary(Enter)]);
}

#[test]
fn a_marker_value_on_a_key_that_was_not_injected_is_a_real_key() {
    // Hardware never carries Iamb's marker; if a driver's value happened to look like one,
    // the key would still be the user's.
    let mut rig = Rig::us();
    let raw = RawKey {
        extra: Marker::Output.encode(),
        ..rig.raw(A, true)
    };
    rig.feed(raw, false);
    assert_eq!(rig.seen(), chars("a"));
}

#[test]
fn iambs_own_modifiers_are_followed_but_not_reported() {
    // A paste: Ctrl down, V, Ctrl up, all marked. A real key in between (which a batch does
    // not allow, but a stuck injector would) is a shortcut for the application.
    let mut rig = Rig::us();
    let output = Marker::Output.encode();
    rig.injected(VK_LCONTROL, true, output);
    rig.injected(0x56, true, output);
    rig.tap(A);
    rig.injected(0x56, false, output);
    rig.injected(VK_LCONTROL, false, output);
    rig.tap(A);
    assert_eq!(rig.seen(), [Boundary(Shortcut), Char('a')]);
}

#[test]
fn an_injected_key_of_iambs_own_makes_a_waiting_dead_key_uncertain() {
    let mut rig = Rig::german();
    let output = Marker::Output.encode();
    rig.tap(ACUTE);
    // An injected Backspace goes through the application's translation.
    rig.injected(VK_BACK, true, output);
    rig.injected(VK_BACK, false, output);
    rig.tap(E);
    rig.tap(E);
    assert_eq!(rig.seen(), [OTHER, Char('e')]);

    // Injected characters too: what the system does with a waiting dead key at a
    // `VK_PACKET` is not known (see `deadkey`).
    rig.tap(ACUTE);
    own_packet(&mut rig, 0x78);
    rig.tap(E);
    rig.tap(E);
    assert_eq!(rig.seen(), [OTHER, Char('e')]);

    // Without a dead key waiting, they change nothing.
    own_packet(&mut rig, 0x78);
    rig.tap(ACUTE);
    rig.tap(E);
    assert_eq!(rig.seen(), chars("é"));
}

/// A character Iamb injects itself: a press and a release of `VK_PACKET`, marked as output.
fn own_packet(rig: &mut Rig, unit: u16) {
    for down in [true, false] {
        rig.feed(
            RawKey {
                scan: u32::from(unit),
                flags: LLKHF_INJECTED | if down { 0 } else { LLKHF_UP },
                extra: Marker::Output.encode(),
                ..rig.raw(VK_PACKET, down)
            },
            false,
        );
    }
}

#[test]
fn characters_injected_by_other_programs_are_characters() {
    let mut rig = Rig::us();
    rig.packet(0x00E9); // é
    rig.packet(0x4E2D); // 中
    // 😀 is U+1F600: a surrogate pair, one event each.
    rig.packet(0xD83D);
    assert_eq!(rig.seen(), chars("é中"));
    rig.packet(0xDE00);
    assert_eq!(rig.seen(), chars("😀"));
    // Control characters are the keys they stand for.
    rig.packet(0x000D);
    rig.packet(0x000A);
    rig.packet(0x0009);
    rig.packet(0x0008);
    rig.packet(0x001B);
    rig.packet(0x0001);
    assert_eq!(
        rig.seen(),
        [
            Boundary(Enter),
            Boundary(Enter),
            Boundary(Tab),
            Backspace,
            Boundary(Escape),
            OTHER
        ]
    );
    // Half a pair is nothing.
    rig.packet(0xDE00);
    rig.packet(0xD83D);
    rig.packet(0x0061);
    rig.packet(0xDE00);
    assert_eq!(rig.seen(), chars("a"));
}

#[test]
fn a_character_injected_while_a_dead_key_waits_is_a_boundary() {
    let mut rig = Rig::german();
    rig.tap(ACUTE);
    // An on-screen keyboard types x. The system's own translation delivers it without the ´,
    // but whether the ´ still waits, and what an application that translates on its own
    // receives, is not known: no character is guessed.
    rig.packet(0x0078);
    assert_eq!(rig.seen(), [OTHER]);
    // Nor for the key after it, which may or may not compose with the ´.
    rig.tap(E);
    rig.tap(E);
    assert_eq!(rig.seen(), [OTHER, Char('e')]);

    // A character outside the Basic Multilingual Plane is one boundary, decided at its second
    // half.
    rig.tap(ACUTE);
    rig.packet(0xD83D);
    assert_eq!(rig.seen(), []);
    rig.packet(0xDE00);
    assert_eq!(rig.seen(), [OTHER]);

    // A control character is not the key it stands for either: an Enter after a ´ that may
    // have been typed first is no delimiter.
    rig.tap(E);
    rig.tap(ACUTE);
    rig.packet(0x000D);
    rig.tap(E);
    assert_eq!(rig.seen(), [OTHER, OTHER, OTHER]);

    // Once nothing waits, injected characters are characters again.
    rig.packet(0x00E9);
    assert_eq!(rig.seen(), chars("é"));
}

// ---------------------------------------------------------------------------------------------
// A dead key and an expansion in flight
// ---------------------------------------------------------------------------------------------

/// The start marker of `id`, posted as a press and a release, as the injector may.
fn start_marker(rig: &mut Rig, id: u64) {
    let marker = Marker::InjectionStart(ExpansionId::from_raw(id)).encode();
    rig.injected(VK_UNASSIGNED, true, marker);
    rig.injected(VK_UNASSIGNED, false, marker);
}

/// Feeds what the listener has reported and not yet handed to [`Rig::seen`] to `handoff`, in
/// order, as the service's engine thread does: the hand-off is armed for expansion 1 before the
/// event at index `armed_at` (the engine expands at the trigger).
fn through_handoff(rig: &Rig, handoff: &TypedHandoff, armed_at: usize) {
    for (i, event) in rig.host.events.iter().enumerate() {
        if i == armed_at {
            handoff.arm(ExpansionId::from_raw(1));
        }
        handoff.observe(event);
    }
}

#[test]
fn a_dead_key_pressed_while_an_expansion_is_pending_cancels_it() {
    let mut rig = Rig::german();
    // The abbreviation and its trigger; the engine expands at the space.
    for vk in [K, VK_SPACE] {
        rig.tap(vk);
    }
    // A dead key before the start marker: the application now holds it.
    rig.tap(ACUTE);
    start_marker(&mut rig, 1);
    let handoff = TypedHandoff::new();
    through_handoff(&rig, &handoff, 2);
    assert_eq!(
        rig.seen(),
        [Char('k'), Char(' '), OTHER, Started(1)],
        "the boundary comes once, right before the marker"
    );
    let expansion = ExpansionId::from_raw(1);
    assert_eq!(
        handoff.status(),
        HandoffStatus::Cancelled(expansion, CancelReason::Boundary)
    );
    assert_eq!(
        handoff.wait_sealed(expansion, std::time::Duration::ZERO),
        Err(CancelReason::Boundary),
        "the injector sends nothing"
    );
    // So the application still holds the ´, and so does the listener.
    rig.tap(E);
    assert_eq!(rig.seen(), chars("é"));
}

#[test]
fn a_dead_key_that_composed_before_the_start_marker_lets_the_expansion_go_ahead() {
    let mut rig = Rig::german();
    for vk in [K, VK_SPACE, ACUTE, E] {
        rig.tap(vk);
    }
    start_marker(&mut rig, 1);
    let handoff = TypedHandoff::new();
    through_handoff(&rig, &handoff, 2);
    assert_eq!(rig.seen(), [Char('k'), Char(' '), Char('é'), Started(1)]);
    let expansion = ExpansionId::from_raw(1);
    assert_eq!(handoff.status(), HandoffStatus::Sealed(expansion));
    assert_eq!(
        handoff.read(expansion, |typed| typed.iter().collect::<String>()),
        Ok("é".to_owned()),
        "the é is typed again after the expansion"
    );
}

#[test]
fn a_dead_key_that_may_wait_at_the_start_marker_cancels_too() {
    let mut rig = Rig::german();
    rig.tap(ACUTE);
    // A command in the middle of the sequence: the ´ may or may not still wait.
    rig.chord(&[VK_LCONTROL], C);
    start_marker(&mut rig, 1);
    assert_eq!(rig.seen(), [Boundary(Shortcut), OTHER, Started(1)]);
}

#[test]
fn a_dead_key_with_no_expansion_in_flight_reports_nothing() {
    // The engine's buffer keeps the abbreviation typed so far: the ´ may compose a character
    // of it.
    let mut rig = Rig::german();
    rig.tap(K);
    rig.tap(ACUTE);
    assert_eq!(rig.seen(), chars("k"));
    rig.tap(E);
    assert_eq!(rig.seen(), chars("é"));
    // And with nothing waiting, a start marker is just the marker; so is the end marker
    // while a dead key waits.
    start_marker(&mut rig, 2);
    rig.tap(ACUTE);
    let end = Marker::InjectionEnd(ExpansionId::from_raw(2)).encode();
    rig.injected(VK_UNASSIGNED, false, end);
    rig.tap(E);
    assert_eq!(rig.seen(), [Started(2), Finished(2), Char('é')]);
}

// ---------------------------------------------------------------------------------------------
// The mouse and hotkeys
// ---------------------------------------------------------------------------------------------

#[test]
fn a_mouse_button_going_down_is_a_click() {
    let mut rig = Rig::us();
    for message in [
        WM_LBUTTONDOWN,
        WM_RBUTTONDOWN,
        WM_MBUTTONDOWN,
        WM_XBUTTONDOWN,
    ] {
        rig.click(message);
    }
    assert_eq!(rig.seen(), [Boundary(Click); 4]);
    // Moves, releases and the wheel are nothing.
    for message in [0x0200, 0x0202, 0x0205, 0x020A] {
        rig.click(message);
    }
    // Neither is a click Iamb injected itself.
    rig.pipeline.on_button(
        &RawButton {
            message: WM_LBUTTONDOWN,
            flags: LLMHF_INJECTED,
            extra: Marker::Output.encode(),
        },
        &mut rig.host,
    );
    assert_eq!(rig.seen(), []);
    // Another program's injected click is a click.
    rig.pipeline.on_button(
        &RawButton {
            message: WM_LBUTTONDOWN,
            flags: LLMHF_INJECTED,
            extra: 0,
        },
        &mut rig.host,
    );
    assert_eq!(rig.seen(), [Boundary(Click)]);
}

#[test]
fn a_registered_hotkey_is_taken_by_the_system_and_not_reported() {
    let mut rig = Rig::us();
    rig.pipeline.set_hotkeys(vec![
        HotkeyBinding {
            modifiers: Modifiers::CTRL.with(Modifiers::SHIFT),
            vk: VK_SPACE,
        },
        HotkeyBinding {
            modifiers: Modifiers::NONE,
            vk: F22,
        },
    ]);
    rig.chord(&[VK_LCONTROL, VK_LSHIFT], VK_SPACE);
    rig.tap(F22);
    assert_eq!(rig.seen(), []);
    // Only the exact modifiers trigger a hotkey.
    rig.chord(&[VK_LCONTROL], VK_SPACE);
    rig.chord(&[VK_LCONTROL, VK_LSHIFT, VK_LMENU], VK_SPACE);
    rig.chord(&[VK_LSHIFT], F22);
    rig.tap(VK_SPACE);
    assert_eq!(
        rig.seen(),
        [Boundary(Shortcut), Boundary(Shortcut), OTHER, Char(' ')]
    );
    // Unregistered again.
    rig.pipeline.set_hotkeys(Vec::new());
    rig.tap(F22);
    assert_eq!(rig.seen(), [OTHER]);
}

// ---------------------------------------------------------------------------------------------
// Keeping the picture of the keyboard true
// ---------------------------------------------------------------------------------------------

#[test]
fn modifiers_held_when_the_hook_is_installed_count() {
    let mut rig = Rig::us();
    rig.host.physical = vec![VK_RSHIFT];
    rig.pipeline.seed(&mut rig.host, true, 0);
    // Shift is held and Caps Lock is on: they cancel for a letter.
    rig.tap(A);
    rig.up(VK_RSHIFT);
    rig.tap(A);
    assert_eq!(rig.seen(), chars("aA"));
}

#[test]
fn releases_the_hook_never_saw_are_noticed_after_a_pause() {
    let mut rig = Rig::us();
    // Ctrl+Shift+Esc opens the task manager, which the hook cannot see into.
    rig.down(VK_LCONTROL);
    rig.down(VK_LSHIFT);
    rig.down(VK_ESCAPE);
    assert_eq!(rig.seen(), [Boundary(Shortcut)]);
    // The keys are released there.
    rig.host.physical.clear();
    // Seconds later the user is back and types.
    rig.now += 5_000;
    rig.tap(A);
    rig.tap(A);
    assert_eq!(rig.seen(), chars("aa"));
}

#[test]
fn a_stale_alt_is_noticed_at_once_when_the_system_sends_an_ordinary_key() {
    let mut rig = Rig::us();
    rig.down(VK_LMENU);
    rig.sys_tap(F);
    assert_eq!(rig.seen(), [Boundary(Shortcut)]);
    // Alt is released where the hook cannot see it, and the user types on without a pause.
    rig.host.physical.clear();
    rig.tap(A);
    rig.tap(A);
    assert_eq!(rig.seen(), chars("aa"));

    // The other way round: Alt went down unseen, and the system sends a system key.
    rig.host.physical = vec![VK_RMENU];
    rig.sys_tap(F);
    rig.host.physical.clear();
    rig.up(VK_RMENU);
    rig.tap(A);
    assert_eq!(rig.seen(), [Boundary(Shortcut), Char('a')]);
}

#[test]
fn without_a_pause_the_physical_keyboard_is_asked_once_per_modifier_press() {
    let mut rig = Rig::us();
    rig.down(VK_LSHIFT);
    for _ in 0..20 {
        rig.tap(A);
    }
    rig.up(VK_LSHIFT);
    assert_eq!(rig.seen(), [Char('A'); 20]);
    assert_eq!(rig.host.physical_questions, 1);
}

#[test]
fn a_caps_lock_that_a_remapping_tool_turns_into_ctrl_does_not_flip_the_toggle() {
    let mut rig = Rig::us();
    for _ in 0..3 {
        // The physical Caps Lock: the tool's hook, which runs after the listener's, swallows
        // it and injects a Ctrl while the listener's hook call is still waiting.
        rig.feed(rig.raw(VK_CAPITAL, true), true);
        rig.feed(
            RawKey {
                flags: LLKHF_INJECTED,
                nested: true,
                ..rig.raw(VK_LCONTROL, true)
            },
            false,
        );
        rig.tap(C);
        rig.feed(rig.raw(VK_CAPITAL, false), true);
        rig.feed(
            RawKey {
                flags: LLKHF_INJECTED | LLKHF_UP,
                nested: true,
                ..rig.raw(VK_LCONTROL, false)
            },
            false,
        );
        assert_eq!(rig.seen(), [Boundary(Shortcut)]);
        // Lower case, every time.
        rig.tap(A);
        assert_eq!(rig.seen(), chars("a"));
    }
}

#[test]
fn a_modifier_swallowed_by_a_later_hook_does_not_count() {
    let mut rig = Rig::german();
    // The physical left Alt is swallowed and nothing replaces it.
    rig.feed(rig.raw(VK_LMENU, true), true);
    rig.tap(E);
    rig.feed(rig.raw(VK_LMENU, false), true);
    assert_eq!(rig.seen(), chars("e"));
}

// ---------------------------------------------------------------------------------------------
// Input methods
// ---------------------------------------------------------------------------------------------

#[test]
fn an_input_method_layout_is_composing_until_the_input_method_says_closed() {
    let mut rig = Rig::new(us(), JAPANESE);
    rig.tap(A);
    assert_eq!(rig.seen(), [Composing(true), Char('a')]);
    assert_eq!(rig.host.asked, [1]);
    rig.pipeline.input_method_answered(1, false, &mut rig.host);
    assert_eq!(rig.seen(), [Composing(false)]);
    assert!(!rig.pipeline.composing());
    rig.tap(A);
    rig.tap(A);
    assert_eq!(rig.seen(), chars("aa"));
    // The mode key: asked at once; the answer says the input method is open now.
    rig.tap(VK_KANA);
    let question = *rig.host.asked.last().expect("a question");
    rig.pipeline
        .input_method_answered(question, true, &mut rig.host);
    assert_eq!(rig.seen(), [Composing(true)]);
    assert!(rig.pipeline.composing());
}

#[test]
fn a_question_that_cannot_be_sent_keeps_the_layout_composing() {
    let mut rig = Rig::new(us(), JAPANESE);
    rig.host.can_ask = false;
    rig.tap(A);
    rig.tap(A);
    assert_eq!(rig.seen(), [Composing(true), Char('a'), Char('a')]);
    assert!(rig.host.asked.is_empty());
}

#[test]
fn a_process_key_is_an_input_method_at_work() {
    let mut rig = Rig::us();
    rig.tap(VK_PROCESSKEY);
    assert_eq!(rig.seen(), [Composing(true)]);
    rig.tap(VK_PROCESSKEY);
    rig.tap(A);
    assert_eq!(rig.seen(), [Composing(false), Char('a')]);
    assert!(
        rig.host.asked.is_empty(),
        "an ordinary layout is never asked"
    );
}

#[test]
fn ordinary_layouts_never_report_composition() {
    let mut rig = Rig::german();
    rig.tap(A);
    rig.tap(VK_KANA);
    rig.chord(&[VK_LCONTROL], VK_SPACE);
    rig.click(WM_LBUTTONDOWN);
    assert_eq!(rig.seen(), [Char('a'), Boundary(Shortcut), Boundary(Click)]);
    assert!(rig.host.asked.is_empty());
}

// ---------------------------------------------------------------------------------------------
// After a loss, and privacy
// ---------------------------------------------------------------------------------------------

#[test]
fn after_a_loss_nothing_remembered_is_trusted() {
    let mut rig = Rig::german();
    rig.down(VK_LCONTROL);
    rig.tap(ACUTE);
    // Events were lost; Ctrl was released meanwhile.
    rig.host.physical.clear();
    rig.pipeline.lost();
    rig.tap(E);
    assert_eq!(rig.seen(), [Boundary(Shortcut), Char('e')]);
}

#[test]
fn debug_shows_no_key_and_no_character() {
    let mut rig = Rig::german();
    rig.down(H);
    rig.tap(ACUTE);
    let shown = format!("{:?} {:?}", rig.pipeline, rig.raw(H, true));
    assert_eq!(
        shown,
        "Pipeline { keys: KeyTracker { state: KeyState { shift: false, ctrl: false, \
         alt: false, win: false, caps: false, .. }, .. }, dead: DeadKeys { waiting: true, .. }, \
         composing: false, hotkeys: 0, .. } RawKey { .. }"
    );
}
