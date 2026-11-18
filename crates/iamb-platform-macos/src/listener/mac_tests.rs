//! Tests of the listener against the real system. macOS only.
//!
//! # What runs where
//!
//! | Tests | Need | On a hosted CI runner |
//! | --- | --- | --- |
//! | `layouts::*`: translation with the system's own US, Spanish, German and French layouts | nothing | run |
//! | `events::*`: markers and key fields through real `CGEvent`s; the disabled-tap notice through the core | nothing | run |
//! | `with_a_tap::*`: the real tap, 10,000 keys, recovery, self-test, hotkeys | Input Monitoring and Accessibility for the process that runs the tests | skip, with a line on standard error that names the permission |
//!
//! A skipped test writes its reason straight to standard error, which the test harness does
//! not capture, so a skip is never silent.
//!
//! # The main thread
//!
//! Keyboard layouts must be read on the main thread, and the test harness keeps the main
//! thread for itself. [`load_on_main_thread`] is therefore a static constructor: the
//! loader runs it on the main thread before the harness starts. It loads the four layouts by
//! identifier, without selecting them, and reads the current layout through the same function
//! the listener's main-thread job uses, so that `Platform::start` finds a layout.

use std::sync::{Arc, OnceLock};

use objc2::MainThreadMarker;
use objc2_core_graphics::{CGEvent, CGEventField};

use super::keycodes::vk;
use super::layout::by_id::{self, Answer};
use super::layout::{self, UchrLayout};
use super::modifiers::layout_state::OPTION;
use super::translate::{KeyAction, KeyLayout, KeyPress, MAX_UNITS};

const US: &str = "com.apple.keylayout.US";
const SPANISH: &str = "com.apple.keylayout.Spanish-ISO";
const GERMAN: &str = "com.apple.keylayout.German";
const FRENCH: &str = "com.apple.keylayout.French";
const LAYOUT_IDS: [&str; 4] = [US, SPANISH, GERMAN, FRENCH];

/// `kVK_ANSI_Quote`: the key right of Ñ, the dead acute accent on the Spanish layout.
const SPANISH_ACUTE_KEY: u16 = 0x27;
/// `kVK_ANSI_LeftBracket`: the key right of P, the dead circumflex on the French layout.
const FRENCH_CIRCUMFLEX_KEY: u16 = 0x21;

/// A key and the modifier state it is pressed with.
type Stroke = (u16, u32);

/// The dead key each layout is documented to have, and E after it: what the probes ask
/// `UCKeyTranslate` directly, so that a failing dead-key test says what the system answered.
const DEAD_KEY_PROBES: [(&str, &str, Stroke); 3] = [
    (US, "Option+E", (vk::ANSI_E, OPTION)),
    (SPANISH, "the key right of Ñ", (SPANISH_ACUTE_KEY, 0)),
    (FRENCH, "the key right of P", (FRENCH_CIRCUMFLEX_KEY, 0)),
];

/// What the static constructor found.
struct Fixture {
    on_main_thread: bool,
    /// Whether secure keyboard entry was on when the test process started.
    secure_input: bool,
    layouts: Vec<(&'static str, Result<Arc<UchrLayout>, String>)>,
    /// Per layout, what `UCKeyTranslate` answered on the main thread for the dead key of
    /// [`DEAD_KEY_PROBES`] and E after it, one line per way of calling it.
    probes: Vec<(&'static str, Vec<String>)>,
}

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

/// Makes the loader call [`load_on_main_thread`] before `main`.
// SAFETY: the section is the Mach-O list of initialiser functions; the entry is a plain
// function without arguments that does not unwind and touches no state that needs `main` to
// have started.
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func,mod_init_funcs")]
static LOAD_ON_MAIN_THREAD: extern "C" fn() = load_on_main_thread;

extern "C" fn load_on_main_thread() {
    // A panic must not unwind out of an initialiser; the tests report a missing fixture.
    let _ = std::panic::catch_unwind(|| {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = FIXTURE.set(Fixture {
                on_main_thread: false,
                secure_input: false,
                layouts: Vec::new(),
                probes: Vec::new(),
            });
            return;
        };
        let layouts: Vec<_> = LAYOUT_IDS
            .iter()
            .map(|id| (*id, by_id::load(mtm, id).map(Arc::new)))
            .collect();
        let probes = probe_on_main_thread(mtm, &layouts);
        layout::system().refresh(mtm);
        let _ = FIXTURE.set(Fixture {
            on_main_thread: true,
            secure_input: super::tap::secure_input_enabled(mtm),
            layouts,
            probes,
        });
    });
}

/// A press as the probes make it: a key going down, from a keyboard of type `keyboard_type`
/// (0: the layout's fallback, `LMGetKbdType` when it was read).
const fn probe_press((code, modifiers): Stroke, keyboard_type: u32) -> KeyPress {
    KeyPress {
        code,
        action: KeyAction::Down,
        modifiers,
        keyboard_type,
    }
}

/// One answer of `UCKeyTranslate`, for a failure message. Layout data only, no user input.
fn show(answer: &Answer) -> String {
    let units: Vec<String> = answer
        .units
        .iter()
        .map(|unit| format!("U+{unit:04X}"))
        .collect();
    format!(
        "status {}, length {}, units [{}], state {:#x}",
        answer.status,
        answer.len,
        units.join(" "),
        answer.state
    )
}

/// The dead key `dead` from no dead-key state, then E from the state it left, through
/// `translate`: what each call answered.
fn two_presses(dead: Stroke, translate: impl Fn(Stroke, u32) -> Option<Answer>) -> String {
    let Some(first) = translate(dead, 0) else {
        return "the data is not a layout".to_owned();
    };
    let second = translate((vk::ANSI_E, 0), first.state).map_or_else(
        || "the data is not a layout".to_owned(),
        |answer| show(&answer),
    );
    format!("{}; then E: {second}", show(&first))
}

/// Asks `UCKeyTranslate`, on the main thread, about each layout's dead key in the ways that
/// differ from the listener's own call: on the system's bytes instead of the private copy,
/// with another keyboard type, and on the layout in use. (Run from the static constructor, so
/// it makes no event: [`probe_report`] adds the keyboard type of a real event.)
fn probe_on_main_thread(
    mtm: MainThreadMarker,
    layouts: &[(&'static str, Result<Arc<UchrLayout>, String>)],
) -> Vec<(&'static str, Vec<String>)> {
    let now = by_id::keyboard_type_now(mtm);
    let mut probes = Vec::new();
    for (id, name, dead) in DEAD_KEY_PROBES {
        let mut lines = vec![format!(
            "{id}, {name} then E (LMGetKbdType on the main thread before the tests: {now}):"
        )];
        if let Some((_, Ok(copy))) = layouts.iter().find(|(known, _)| *known == id) {
            let kind = copy.fallback_keyboard_type();
            lines.push(format!(
                "  main thread, private copy, keyboard type {kind}: {}",
                two_presses(dead, |stroke, state| Some(
                    copy.answer(&probe_press(stroke, 0), state)
                ))
            ));
        }
        match by_id::system_data(mtm, id) {
            Ok(data) => {
                for kind in [now, 0] {
                    lines.push(format!(
                        "  main thread, system bytes, keyboard type {kind}: {}",
                        two_presses(dead, |stroke, state| {
                            by_id::answer_on(&data, &probe_press(stroke, 0), kind, state)
                        })
                    ));
                }
            }
            Err(why) => lines.push(format!("  main thread, system bytes: {why}")),
        }
        if let Some((current, data)) = by_id::current_data(mtm) {
            lines.push(format!(
                "  main thread, the layout in use ({current}), system bytes, keyboard type \
                 {now}: {}",
                two_presses(dead, |stroke, state| {
                    by_id::answer_on(&data, &probe_press(stroke, 0), now, state)
                })
            ));
        }
        probes.push((id, lines));
    }
    probes
}

/// What the probes found for `id`, and what the private copy answers on this (test) thread,
/// as the listener thread calls it, as lines for a failure message. Each line is also written
/// to standard error.
fn probe_report(id: &str) -> String {
    let mut lines: Vec<String> = fixture()
        .probes
        .iter()
        .filter(|(known, _)| *known == id)
        .flat_map(|(_, lines)| lines.iter().cloned())
        .collect();
    if let Some((_, _, dead)) = DEAD_KEY_PROBES.iter().find(|(known, _, _)| *known == id) {
        let layout = system_layout(id);
        // The keyboard type a key event carries, which is what the listener translates with.
        let event_type = CGEvent::new_keyboard_event(None, vk::ANSI_E, true).map_or(0, |event| {
            let field =
                CGEvent::integer_value_field(Some(&event), CGEventField::KeyboardEventKeyboardType);
            u32::try_from(field).unwrap_or(0)
        });
        for kind in [0, event_type] {
            lines.push(format!(
                "  test thread, private copy, keyboard type {}: {}",
                if kind == 0 {
                    layout.fallback_keyboard_type()
                } else {
                    kind
                },
                two_presses(*dead, |stroke, state| Some(
                    layout.answer(&probe_press(stroke, kind), state)
                ))
            ));
        }
    }
    for line in &lines {
        note(&format!("PROBE listener::mac_tests: {line}"));
    }
    lines.join("\n")
}

fn fixture() -> &'static Fixture {
    let fixture = FIXTURE
        .get()
        .expect("the static constructor that loads the keyboard layouts did not run");
    assert!(
        fixture.on_main_thread,
        "the static constructor did not run on the main thread"
    );
    fixture
}

/// One of the four layouts, loaded from the system.
fn system_layout(id: &str) -> Arc<UchrLayout> {
    let (_, loaded) = fixture()
        .layouts
        .iter()
        .find(|(known, _)| *known == id)
        .expect("one of the four layouts");
    match loaded {
        Ok(layout) => Arc::clone(layout),
        Err(why) => panic!("the system layout {id} could not be loaded: {why}"),
    }
}

/// What the layout itself says a key types, starting from dead-key state `dead`: the text and
/// the state afterwards. `None` when the layout cannot translate the key.
fn raw(layout: &UchrLayout, code: u16, state: u32, dead: u32) -> Option<(String, u32)> {
    let mut dead_state = dead;
    let mut units = [0; MAX_UNITS];
    let press = KeyPress {
        code,
        action: KeyAction::Down,
        modifiers: state,
        keyboard_type: 0,
    };
    let len = layout.translate(&press, &mut dead_state, &mut units)?;
    Some((String::from_utf16_lossy(&units[..len]), dead_state))
}

/// Writes a line to standard error past the harness's capture.
fn note(line: &str) {
    use std::io::Write;
    let _ = std::io::stderr().write_all(format!("{line}\n").as_bytes());
}

mod layouts {
    use iamb_platform::core_types::{BoundaryKind, KeyName};

    use super::super::keycodes::{KeyClass, classify, vk};
    use super::super::modifiers::layout_state::{OPTION, SHIFT};
    use super::super::modifiers::{FLAG_COMMAND, FLAG_CONTROL, FLAG_OPTION, FLAG_SHIFT, Modifiers};
    use super::super::testing::typed;
    use super::super::translate::{KeyEffect, Translator};
    use super::*;

    const STATES: [u32; 4] = [0, SHIFT, OPTION, SHIFT | OPTION];

    /// Every stroke the layout decides about on the main block of the keyboard: the keys
    /// that are not named, function, modifier or numeric-keypad keys, with each combination
    /// of Shift and Option.
    fn strokes() -> impl Iterator<Item = Stroke> {
        const KEYPAD: std::ops::RangeInclusive<u16> = 0x41..=0x5C;
        (0..0x80_u16)
            .filter(|code| classify(*code) == KeyClass::Layout && !KEYPAD.contains(code))
            .flat_map(|code| STATES.into_iter().map(move |state| (code, state)))
    }

    /// The strokes that type exactly `text` when no dead key is pending.
    fn typing(layout: &UchrLayout, text: &str) -> Vec<Stroke> {
        strokes()
            .filter(|&(code, state)| {
                raw(layout, code, state, 0).is_some_and(|(out, dead)| out == text && dead == 0)
            })
            .collect()
    }

    /// The dead keys that, followed by `base`, type exactly `combined`.
    fn dead_keys_giving(layout: &UchrLayout, base: Stroke, combined: &str) -> Vec<Stroke> {
        strokes()
            .filter(|&(code, state)| {
                let Some((out, dead)) = raw(layout, code, state, 0) else {
                    return false;
                };
                out.is_empty()
                    && dead != 0
                    && raw(layout, base.0, base.1, dead)
                        .is_some_and(|(out, after)| out == combined && after == 0)
            })
            .collect()
    }

    fn modifiers((_, state): Stroke) -> Modifiers {
        let mut flags = 0;
        if state & SHIFT != 0 {
            flags |= FLAG_SHIFT;
        }
        if state & OPTION != 0 {
            flags |= FLAG_OPTION;
        }
        Modifiers::from_event_flags(flags)
    }

    /// Presses `stroke` through the listener's translator.
    fn press(
        translator: &mut Translator,
        layout: &UchrLayout,
        stroke: Stroke,
    ) -> (String, KeyEffect) {
        let output = translator.key_down(stroke.0, modifiers(stroke), false, 0, Some(layout));
        (typed(&output), output.key_effect())
    }

    /// The one unmodified key that types `letter`.
    fn plain_key(layout: &UchrLayout, id: &str, letter: &str) -> Stroke {
        let keys: Vec<Stroke> = typing(layout, letter)
            .into_iter()
            .filter(|(_, state)| *state == 0)
            .collect();
        assert_eq!(
            keys.len(),
            1,
            "{id}: expected one unmodified key for {letter:?}, the layout has {keys:?}"
        );
        keys[0]
    }

    #[test]
    fn the_four_layouts_load_from_the_system() {
        for id in LAYOUT_IDS {
            let layout = system_layout(id);
            // Every layout types the 26 letters without modifiers, somewhere.
            for letter in 'a'..='z' {
                let keys = typing(&layout, &letter.to_string());
                assert!(
                    keys.iter().any(|(_, state)| *state == 0),
                    "{id}: no unmodified key types {letter:?}"
                );
            }
        }
    }

    #[test]
    fn us_plain_shifted_and_option_keys() {
        let us = system_layout(US);
        let mut t = Translator::new();
        assert_eq!(plain_key(&us, US, "a"), (vk::ANSI_A, 0));
        assert_eq!(
            press(&mut t, &us, (vk::ANSI_A, 0)),
            ("a".to_owned(), KeyEffect::None)
        );
        assert_eq!(
            press(&mut t, &us, (vk::ANSI_A, SHIFT)),
            ("A".to_owned(), KeyEffect::None)
        );
        assert_eq!(
            press(&mut t, &us, (vk::ANSI_1, 0)),
            ("1".to_owned(), KeyEffect::None)
        );
        assert_eq!(
            press(&mut t, &us, (vk::ANSI_1, SHIFT)),
            ("!".to_owned(), KeyEffect::None)
        );
        assert_eq!(
            press(&mut t, &us, (vk::SPACE, 0)),
            (" ".to_owned(), KeyEffect::None)
        );
        // Option+Space is a no-break space: text, not a shortcut.
        assert_eq!(
            press(&mut t, &us, (vk::SPACE, OPTION)),
            ("\u{a0}".to_owned(), KeyEffect::None)
        );
    }

    #[test]
    fn us_option_e_is_a_dead_acute_accent() {
        let us = system_layout(US);
        let e = plain_key(&us, US, "e");
        let dead = dead_keys_giving(&us, e, "é");
        assert!(
            dead.contains(&(vk::ANSI_E, OPTION)),
            "US: Option+E then E should type é; the layout's dead keys for é are {dead:?}\n{}",
            probe_report(US)
        );
        // The US layout has no dead key without Option.
        assert!(dead.iter().all(|(_, state)| state & OPTION != 0));

        let mut t = Translator::new();
        assert_eq!(
            press(&mut t, &us, (vk::ANSI_E, OPTION)),
            (String::new(), KeyEffect::None)
        );
        assert!(t.dead_key_pending());
        assert_eq!(press(&mut t, &us, e), ("é".to_owned(), KeyEffect::None));
        assert!(!t.dead_key_pending());
    }

    /// The dead-key checks shared by the Spanish and French tests: `accent` then E types
    /// `combined`, through the layout and through the translator.
    fn check_dead_key(id: &str, combined: &str) -> (Arc<UchrLayout>, Stroke, Stroke) {
        let layout = system_layout(id);
        let e = plain_key(&layout, id, "e");
        let dead = dead_keys_giving(&layout, e, combined);
        let plain_dead: Vec<Stroke> = dead.iter().copied().filter(|(_, s)| *s == 0).collect();
        assert_eq!(
            plain_dead.len(),
            1,
            "{id}: expected one unmodified dead key that makes {combined:?} with E; the \
             layout's dead keys for it are {dead:?}\n{}",
            probe_report(id)
        );
        let accent = plain_dead[0];

        let mut t = Translator::new();
        assert_eq!(
            press(&mut t, &layout, accent),
            (String::new(), KeyEffect::None)
        );
        assert!(t.dead_key_pending(), "{id}: the dead key is pending");
        assert_eq!(
            press(&mut t, &layout, e),
            (combined.to_owned(), KeyEffect::None)
        );
        assert!(!t.dead_key_pending());

        // A key the accent does not combine with: the translator reports exactly what the
        // layout says, the accent and then the letter.
        let x = plain_key(&layout, id, "x");
        let (_, pending) = raw(&layout, accent.0, accent.1, 0).expect("the dead key translates");
        let (both, after) = raw(&layout, x.0, x.1, pending).expect("x translates");
        assert_eq!(after, 0);
        assert_eq!(
            both.chars().count(),
            2,
            "{id}: accent and letter, got {both:?}"
        );
        assert!(both.ends_with('x'));
        assert_eq!(
            press(&mut t, &layout, accent),
            (String::new(), KeyEffect::None)
        );
        assert_eq!(press(&mut t, &layout, x), (both, KeyEffect::None));
        (layout, accent, e)
    }

    #[test]
    fn spanish_acute_accent_then_e_is_e_acute() {
        let (spanish, accent, e) = check_dead_key(SPANISH, "é");
        // Shift on the letter gives the capital.
        let mut t = Translator::new();
        assert_eq!(
            press(&mut t, &spanish, accent),
            (String::new(), KeyEffect::None)
        );
        assert_eq!(
            press(&mut t, &spanish, (e.0, SHIFT)),
            ("É".to_owned(), KeyEffect::None)
        );
        // The layout has its own key for ñ.
        let _ = plain_key(&spanish, SPANISH, "ñ");
    }

    #[test]
    fn french_circumflex_then_e_is_e_circumflex() {
        let (french, _, _) = check_dead_key(FRENCH, "ê");
        // AZERTY: the key that types A is where Q is on a US keyboard.
        assert_eq!(plain_key(&french, FRENCH, "a"), (vk::ANSI_Q, 0));
        // Digits need Shift on the French layout, and are text all the same.
        let ones = typing(&french, "1");
        assert!(!ones.is_empty());
        assert!(ones.iter().all(|(_, state)| state & SHIFT != 0), "{ones:?}");
        let mut t = Translator::new();
        assert_eq!(
            press(&mut t, &french, ones[0]),
            ("1".to_owned(), KeyEffect::None)
        );
    }

    #[test]
    fn german_at_sign_needs_option_and_is_a_character() {
        let german = system_layout(GERMAN);
        let at = typing(&german, "@");
        assert!(!at.is_empty(), "German: no key types @");
        assert!(
            at.iter().all(|(_, state)| state & OPTION != 0),
            "German: @ is typed with Option; the layout has {at:?}"
        );
        if !at.contains(&(vk::ANSI_L, OPTION)) {
            note(&format!(
                "NOTE listener::mac_tests: this system's German layout types @ with {at:?}, \
                 not with Option+L"
            ));
        }
        let mut t = Translator::new();
        for stroke in at {
            assert_eq!(
                press(&mut t, &german, stroke),
                ("@".to_owned(), KeyEffect::None)
            );
        }
        // QWERTZ: Z is where Y is on a US keyboard.
        assert_eq!(plain_key(&german, GERMAN, "z"), (vk::ANSI_Y, 0));
        assert_eq!(plain_key(&german, GERMAN, "y"), (vk::ANSI_Z, 0));
        let _ = plain_key(&german, GERMAN, "ß");
    }

    #[test]
    fn the_translator_agrees_with_the_layout_on_every_stroke() {
        for id in LAYOUT_IDS {
            let layout = system_layout(id);
            let mut option_text = 0;
            for stroke in strokes() {
                let Some((text, dead)) = raw(&layout, stroke.0, stroke.1, 0) else {
                    continue;
                };
                let mut t = Translator::new();
                let (typed, effect) = press(&mut t, &layout, stroke);
                let printable = !text.is_empty() && !text.chars().any(char::is_control);
                if printable {
                    // Text is text whatever modifiers were held.
                    assert_eq!((typed, effect), (text, KeyEffect::None), "{id} {stroke:?}");
                    assert!(!t.dead_key_pending());
                    if stroke.1 & OPTION != 0 {
                        option_text += 1;
                    }
                } else if text.is_empty() && dead != 0 {
                    assert_eq!(
                        (typed, effect),
                        (String::new(), KeyEffect::None),
                        "{id} {stroke:?}"
                    );
                    assert!(t.dead_key_pending(), "{id} {stroke:?}");
                } else if text.is_empty() {
                    let kind = if stroke.1 & OPTION == 0 {
                        BoundaryKind::Other
                    } else {
                        BoundaryKind::Shortcut
                    };
                    assert_eq!(
                        (typed, effect),
                        (String::new(), KeyEffect::Boundary(kind)),
                        "{id} {stroke:?}"
                    );
                }
            }
            assert!(
                option_text > 20,
                "{id}: Option types characters ({option_text} found)"
            );
        }
    }

    #[test]
    fn command_and_control_chords_are_shortcuts_on_every_layout() {
        for id in LAYOUT_IDS {
            let layout = system_layout(id);
            let c = plain_key(&layout, id, "c");
            for flags in [FLAG_COMMAND, FLAG_CONTROL, FLAG_COMMAND | FLAG_SHIFT] {
                let mut t = Translator::new();
                let output = t.key_down(
                    c.0,
                    Modifiers::from_event_flags(flags),
                    false,
                    0,
                    Some(&*layout),
                );
                assert_eq!(typed(&output), "");
                assert_eq!(
                    output.key_effect(),
                    KeyEffect::Boundary(BoundaryKind::Shortcut)
                );
            }
            // Named keys do not depend on the layout.
            let mut t = Translator::new();
            let output = t.key_down(vk::RETURN, Modifiers::NONE, false, 0, Some(&*layout));
            assert_eq!(output.key_effect(), KeyEffect::Named(KeyName::Enter));
            let output = t.key_down(vk::DELETE, Modifiers::NONE, false, 0, Some(&*layout));
            assert_eq!(output.key_effect(), KeyEffect::Named(KeyName::Backspace));
        }
    }

    #[test]
    fn translating_changes_no_state_but_the_callers_own() {
        let spanish = system_layout(SPANISH);
        let e = plain_key(&spanish, SPANISH, "e");
        let accent = dead_keys_giving(&spanish, e, "é")
            .into_iter()
            .find(|(_, state)| *state == 0)
            .unwrap_or_else(|| panic!("the Spanish dead acute accent\n{}", probe_report(SPANISH)));

        // Two holders of dead-key state for the same layout, as the listener and the focused
        // application are: a dead key pressed in one is invisible to the other.
        let mut listener = Translator::new();
        let mut application = Translator::new();
        assert_eq!(press(&mut listener, &spanish, accent).0, "");
        assert_eq!(press(&mut application, &spanish, e).0, "e");
        assert_eq!(press(&mut listener, &spanish, e).0, "é");

        // And the layout itself keeps nothing: the same call gives the same answer.
        let first = raw(&spanish, accent.0, accent.1, 0);
        for _ in 0..3 {
            assert_eq!(raw(&spanish, accent.0, accent.1, 0), first);
            assert_eq!(raw(&spanish, e.0, e.1, 0), Some(("e".to_owned(), 0)));
        }
    }

    #[test]
    fn the_current_layout_was_published_for_the_listener() {
        use super::super::inputs::InputSources;

        let _ = fixture();
        let snapshot = layout::system()
            .current()
            .expect("the constructor read the current input source on the main thread");
        // Whatever layout this machine uses, a second read finds nothing new.
        let generation = layout::system().generation();
        assert!(generation >= 1);
        if !snapshot.input_method {
            assert!(
                snapshot.layout.is_some(),
                "a keyboard layout is selected and has Unicode layout data"
            );
        }
    }
}

mod events {
    use std::time::{Duration, Instant};

    use iamb_platform::Marker;
    use iamb_platform::core_types::ExpansionId;
    use objc2_core_graphics::{CGEvent, CGEventField, CGEventFlags, CGEventType};

    use super::super::core::{RawEvent, RawKind};
    use super::super::keycodes::vk;
    use super::super::modifiers::{FLAG_OPTION, FLAG_SHIFT, Modifiers};
    use super::super::recovery::{DisableReason, TapHealth, TapSwitch};
    use super::super::tap::raw_event;
    use super::super::testing::{FakeInputs, Rig};
    use super::*;

    /// A keyboard event with no modifier held, whatever the real keyboard is doing.
    fn key_event(code: u16, down: bool) -> objc2_core_foundation::CFRetained<CGEvent> {
        let event = CGEvent::new_keyboard_event(None, code, down)
            .expect("the system creates a keyboard event (is there a login session?)");
        CGEvent::set_flags(Some(&event), CGEventFlags(0));
        event
    }

    fn read(kind: CGEventType, event: &CGEvent) -> RawEvent {
        raw_event(kind, Some(event)).expect("an event with content")
    }

    #[test]
    fn a_marker_round_trips_through_an_events_user_data() {
        let markers = [
            Marker::Output,
            Marker::InjectionStart(ExpansionId::from_raw(1)),
            Marker::InjectionStart(ExpansionId::from_raw((1 << 40) - 1)),
            Marker::InjectionEnd(ExpansionId::from_raw(77)),
            Marker::SelfTest(0),
            Marker::SelfTest(u32::MAX),
        ];
        for marker in markers {
            for (kind, down) in [(CGEventType::KeyDown, true), (CGEventType::KeyUp, false)] {
                let event = key_event(vk::F20, down);
                // A fresh event carries no marker.
                assert_eq!(Marker::decode_i64(read(kind, &event).user_data), None);
                CGEvent::set_integer_value_field(
                    Some(&event),
                    CGEventField::EventSourceUserData,
                    marker.encode_i64(),
                );
                assert_eq!(
                    Marker::decode_i64(read(kind, &event).user_data),
                    Some(marker)
                );
                // A copy of the event, as the system hands to a tap, keeps it.
                let copy = CGEvent::new_copy(Some(&event)).expect("a copy of the event");
                assert_eq!(
                    Marker::decode_i64(read(kind, &copy).user_data),
                    Some(marker)
                );
            }
        }
    }

    #[test]
    fn the_key_fields_are_read_from_a_real_event() {
        let event = key_event(vk::ANSI_L, true);
        CGEvent::set_flags(Some(&event), CGEventFlags(FLAG_OPTION | FLAG_SHIFT));
        let raw = read(CGEventType::KeyDown, &event);
        assert!(raw.kind == RawKind::KeyDown);
        assert_eq!(raw.key_code, vk::ANSI_L);
        assert!(!raw.repeat);
        let modifiers = Modifiers::from_event_flags(raw.flags);
        assert!(modifiers.option() && modifiers.shift() && !modifiers.is_command_chord());

        CGEvent::set_integer_value_field(Some(&event), CGEventField::KeyboardEventAutorepeat, 1);
        CGEvent::set_integer_value_field(Some(&event), CGEventField::KeyboardEventKeyboardType, 41);
        let raw = read(CGEventType::KeyDown, &event);
        assert!(raw.repeat);
        assert_eq!(raw.keyboard_type, 41);

        // Only key presses are read in full; a release is not translated.
        let release = read(CGEventType::KeyUp, &key_event(vk::ANSI_L, false));
        assert!(release.kind == RawKind::KeyUp);
        assert_eq!(release.key_code, 0);
    }

    #[test]
    fn event_kinds_map_to_what_the_core_expects() {
        let event = key_event(vk::ANSI_A, true);
        for (kind, expected) in [
            (CGEventType::KeyDown, RawKind::KeyDown),
            (CGEventType::KeyUp, RawKind::KeyUp),
            (CGEventType::FlagsChanged, RawKind::FlagsChanged),
            (CGEventType::LeftMouseDown, RawKind::MouseDown),
            (CGEventType::RightMouseDown, RawKind::MouseDown),
            (CGEventType::OtherMouseDown, RawKind::MouseDown),
            (CGEventType::ScrollWheel, RawKind::Other),
        ] {
            assert!(read(kind, &event).kind == expected);
        }
        // The notices need no event at all.
        for (kind, reason) in [
            (CGEventType::TapDisabledByTimeout, DisableReason::Timeout),
            (
                CGEventType::TapDisabledByUserInput,
                DisableReason::UserInput,
            ),
        ] {
            let raw = raw_event(kind, None).expect("a notice");
            assert!(raw.kind == RawKind::TapDisabled(reason));
            let raw = raw_event(kind, Some(&event)).expect("a notice");
            assert!(raw.kind == RawKind::TapDisabled(reason));
        }
        assert!(raw_event(CGEventType::KeyDown, None).is_none());
    }

    #[test]
    fn real_events_through_the_core_with_the_systems_us_layout() {
        let us: Arc<dyn KeyLayout> = system_layout(US);
        let mut rig = Rig::new(FakeInputs::shared_layout(us));
        rig.core.started();
        assert_eq!(rig.take(), ["composing:false#1"]);

        for code in [vk::ANSI_H, vk::ANSI_I] {
            rig.send(&read(CGEventType::KeyDown, &key_event(code, true)));
            rig.send(&read(CGEventType::KeyUp, &key_event(code, false)));
        }
        let shifted = key_event(vk::ANSI_1, true);
        CGEvent::set_flags(Some(&shifted), CGEventFlags(FLAG_SHIFT));
        rig.send(&read(CGEventType::KeyDown, &shifted));
        rig.send(&read(CGEventType::KeyDown, &key_event(vk::RETURN, true)));
        assert_eq!(rig.take(), ["h#2", "i#3", "!#4", "boundary:Enter#5"]);

        // Iamb's own output is dropped; its markers are reported; nothing is translated.
        let own = key_event(vk::ANSI_X, true);
        CGEvent::set_integer_value_field(
            Some(&own),
            CGEventField::EventSourceUserData,
            Marker::Output.encode_i64(),
        );
        rig.send(&read(CGEventType::KeyDown, &own));
        let start = key_event(vk::F20, false);
        CGEvent::set_integer_value_field(
            Some(&start),
            CGEventField::EventSourceUserData,
            Marker::InjectionStart(ExpansionId::from_raw(9)).encode_i64(),
        );
        rig.send(&read(CGEventType::KeyUp, &start));
        assert_eq!(rig.take(), ["started:9#6"]);
    }

    #[test]
    fn the_systems_disable_notice_enables_the_tap_again_at_once() {
        let us: Arc<dyn KeyLayout> = system_layout(US);
        for kind in [
            CGEventType::TapDisabledByTimeout,
            CGEventType::TapDisabledByUserInput,
        ] {
            let mut rig = Rig::new(FakeInputs::shared_layout(Arc::clone(&us)));
            rig.core.started();
            rig.take();
            rig.send(&read(CGEventType::KeyDown, &key_event(vk::ANSI_A, true)));

            rig.tap.system_disables();
            let noticed = Instant::now();
            rig.send(&raw_event(kind, None).expect("a notice"));
            assert!(rig.tap.is_enabled());
            assert!(noticed.elapsed() < Duration::from_secs(1));
            rig.send(&read(CGEventType::KeyDown, &key_event(vk::ANSI_B, true)));
            assert_eq!(rig.take(), ["a#2", "overflow", "b#3"]);

            let (health, stats) = rig.core.tap_health();
            assert_eq!(health, TapHealth::Working);
            assert_eq!(stats.recoveries, 1);
            assert!(stats.longest_recovery.expect("one recovery") < Duration::from_secs(1));
        }
    }
}

mod with_a_tap {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{Receiver, channel, sync_channel};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use iamb_platform::{
        Chord, EventSink, HotkeyId, Hotkeys, KeyListener, ListenerEvent, PermissionKind,
        PlatformError, SeqSource,
    };
    use objc2_core_foundation::{CFMachPort, CFRunLoop};
    use objc2_core_graphics::{
        CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID,
        CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
    };

    use super::super::keycodes::vk;
    use super::super::main_thread::{self, common_modes, default_mode};
    use super::super::modifiers::{FLAG_CONTROL, FLAG_OPTION, FLAG_SHIFT};
    use super::super::recovery::TapHealth;
    use super::super::tap::{self, Parts, Request, Running};
    use super::super::testing::FakeInputs;
    use super::*;
    use crate::Platform;
    use crate::permissions::SystemProbe;
    use crate::permissions::policy::{self, missing_for_listener};

    /// The delivery target for the callback: under 1 ms at the 99th percentile. Debug builds
    /// get the allowance the other timing tests of the workspace give them.
    const CALLBACK_P99_LIMIT: Duration = if cfg!(debug_assertions) {
        Duration::from_millis(25)
    } else {
        Duration::from_millis(1)
    };

    /// The tag of this file's test keys in the event-source user data. Not an Iamb marker, so
    /// the listener takes the keys for real typing.
    const TEST_TAG: i64 = 0x7E57_4B45_5953_0001;

    /// Says why a test is skipped, on standard error past the harness's capture.
    fn skip(test: &str, why: &str) {
        note(&format!(
            "SKIPPED listener::mac_tests::with_a_tap::{test}: {why}"
        ));
    }

    /// True when this process may create the tap and the tap can see keys; otherwise the
    /// skip is announced.
    fn permitted(test: &str) -> bool {
        let missing = policy::status(&SystemProbe).missing();
        if missing.is_empty() {
            if fixture().secure_input {
                skip(
                    test,
                    "secure keyboard entry was on when the tests started (a password field \
                     had the focus, or the terminal's Secure Keyboard Entry is enabled): the \
                     system keeps keyboard events from every event tap while it is on",
                );
                return false;
            }
            return true;
        }
        let names: Vec<String> = missing.iter().map(ToString::to_string).collect();
        skip(
            test,
            &format!(
                "the process that runs the tests lacks the permission(s): {}. Grant Input \
                 Monitoring and Accessibility in System Settings, Privacy & Security, to the \
                 terminal or IDE that runs `cargo test`",
                names.join(", ")
            ),
        );
        false
    }

    /// A sink that never drops: what the listener delivers is what arrives.
    fn unbounded_sink() -> (Box<dyn EventSink>, Receiver<ListenerEvent>) {
        let (sender, receiver) = channel();
        let sink = move |event: ListenerEvent| {
            let _ = sender.send(event);
        };
        (Box::new(sink), receiver)
    }

    /// A listener on the system's US layout, whatever layout this machine uses.
    fn start_on_us_layout() -> (Running, Receiver<ListenerEvent>) {
        let us: Arc<dyn KeyLayout> = system_layout(US);
        let (sink, events) = unbounded_sink();
        let running = tap::start(Parts {
            seq: Arc::new(SeqSource::new()),
            sink,
            inputs: FakeInputs::shared_layout(us),
        })
        .expect("the tap starts with both permissions granted");
        (running, events)
    }

    /// Disables the listener's tap behind its back and returns once that is done.
    fn disable_tap(running: &Running) {
        let (done, is_done) = sync_channel(1);
        running.shared().send(Request::DisableTap(done));
        is_done
            .recv_timeout(Duration::from_secs(2))
            .expect("the listener thread disables its tap");
    }

    fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        while !done() {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
        true
    }

    #[test]
    fn without_the_permissions_start_is_refused_and_nothing_runs() {
        let Some(missing) = missing_for_listener(&SystemProbe) else {
            skip(
                "without_the_permissions_start_is_refused_and_nothing_runs",
                "both permissions are granted here, so the refusal cannot be shown with the \
                 real system; `listener::tests` covers it with a fake probe",
            );
            return;
        };
        let platform = Platform::new();
        let (sink, events) = unbounded_sink();
        assert_eq!(
            platform.start(sink),
            Err(PlatformError::PermissionDenied(missing))
        );
        // Input Monitoring is named first: it is what the tap itself needs.
        if !policy::status(&SystemProbe).input_monitoring.is_satisfied() {
            assert_eq!(missing, PermissionKind::InputMonitoring);
        }
        // The refused start left nothing behind: no listener, and the sink was dropped.
        assert_eq!(
            platform.self_test(Duration::from_millis(50)),
            Err(PlatformError::NotRunning)
        );
        assert_eq!(platform.stop(), Ok(()));
        assert!(events.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(format!("{platform:?}").contains("running: false"));
    }

    #[test]
    fn a_stopped_listener_reports_not_running_and_stops_again_without_error() {
        let platform = Platform::new();
        assert_eq!(platform.stop(), Ok(()));
        assert_eq!(platform.stop(), Ok(()));
        assert_eq!(
            platform.self_test(Duration::from_millis(10)),
            Err(PlatformError::NotRunning)
        );
    }

    #[test]
    fn start_self_test_stop_and_restart_with_the_real_tap() {
        if !permitted("start_self_test_stop_and_restart_with_the_real_tap") {
            return;
        }
        let platform = Platform::new();
        let (sink, events) = unbounded_sink();
        assert_eq!(platform.start(sink), Ok(()));
        let (second, _) = unbounded_sink();
        assert_eq!(platform.start(second), Err(PlatformError::AlreadyRunning));

        // The state of the input method is the first thing the listener says.
        let first = events
            .recv_timeout(Duration::from_secs(2))
            .expect("the composing state");
        let ListenerEvent::Composing { seq: first_seq, .. } = first else {
            panic!("expected the composing state first, got {first:?}");
        };

        let started = Instant::now();
        assert_eq!(platform.self_test(Duration::from_secs(1)), Ok(()));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(format!("{platform:?}").contains("running: true"));

        assert_eq!(platform.stop(), Ok(()));
        assert_eq!(platform.stop(), Ok(()));
        assert_eq!(
            platform.self_test(Duration::from_secs(1)),
            Err(PlatformError::NotRunning)
        );
        // The sink was dropped with the listener: the channel ends once it is drained. The
        // self-test's event was not delivered; a key the person at the machine pressed in
        // the meantime may have been.
        let rest: Vec<ListenerEvent> = events.iter().collect();
        assert!(!rest.contains(&ListenerEvent::Overflow));

        // A restart works, and the sequence numbers go on.
        let (sink, events) = unbounded_sink();
        assert_eq!(platform.start(sink), Ok(()));
        let again = events
            .recv_timeout(Duration::from_secs(2))
            .expect("the composing state");
        assert!(again.seq().expect("a sequence number") > first_seq);
        assert_eq!(platform.self_test(Duration::from_secs(1)), Ok(()));
        assert_eq!(platform.stop(), Ok(()));
    }

    /// What the event filter shares with its callback.
    struct FilterShared {
        swallowed: AtomicU64,
        disabled: AtomicBool,
        stop: AtomicBool,
    }

    /// An active tap behind the listener's that removes this file's test keys from the event
    /// stream, so that they never reach the application that has the keyboard focus.
    struct EventFilter {
        shared: Arc<FilterShared>,
        thread: Option<JoinHandle<()>>,
    }

    /// The filter's callback: test keys go no further; everything else passes unchanged.
    unsafe extern "C-unwind" fn filter_callback(
        _proxy: CGEventTapProxy,
        kind: CGEventType,
        event: *mut CGEvent,
        info: *mut c_void,
    ) -> *mut CGEvent {
        // SAFETY: `info` is the `FilterShared` that `filter_thread` keeps alive until it has
        // invalidated the tap's port.
        let shared = unsafe { &*info.cast::<FilterShared>() };
        if matches!(
            kind,
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
        ) {
            shared.disabled.store(true, Ordering::Release);
            return event;
        }
        // SAFETY: the system passes null or an event that is valid for the duration of the
        // call.
        let Some(event_ref) = (unsafe { event.as_ref() }) else {
            return event;
        };
        let tag = CGEvent::integer_value_field(Some(event_ref), CGEventField::EventSourceUserData);
        if tag == TEST_TAG {
            shared.swallowed.fetch_add(1, Ordering::Relaxed);
            return std::ptr::null_mut();
        }
        event
    }

    fn filter_thread(shared: &Arc<FilterShared>, ready: &std::sync::mpsc::SyncSender<bool>) {
        let info = Arc::as_ptr(shared).cast_mut().cast::<c_void>();
        let mask = (1 << CGEventType::KeyDown.0) | (1 << CGEventType::KeyUp.0);
        // SAFETY: `filter_callback` returns the event it was given or null, as an active tap
        // may; `info` points to `shared`, which this function holds until after the port is
        // invalidated below.
        let port = unsafe {
            tap::create_tap(
                CGEventTapPlacement::TailAppendEventTap,
                CGEventTapOptions::Default,
                mask,
                filter_callback,
                info,
            )
        };
        let installed = port.and_then(|port| {
            let source = CFMachPort::new_run_loop_source(None, Some(&port), 0)?;
            Some((port, source, CFRunLoop::current()?))
        });
        let Some((port, source, run_loop)) = installed else {
            let _ = ready.send(false);
            return;
        };
        let (common_modes, default_mode) = (common_modes(), default_mode());
        run_loop.add_source(Some(&source), common_modes);
        CGEvent::tap_enable(&port, true);
        let _ = ready.send(true);
        while !shared.stop.load(Ordering::Acquire) {
            CFRunLoop::run_in_mode(default_mode, 0.05, false);
        }
        CGEvent::tap_enable(&port, false);
        run_loop.remove_source(Some(&source), common_modes);
        port.invalidate();
    }

    impl EventFilter {
        /// Installs the filter and checks, with a key release that does nothing, that it
        /// really removes tagged events. `None` when it does not.
        fn start() -> Option<Self> {
            let shared = Arc::new(FilterShared {
                swallowed: AtomicU64::new(0),
                disabled: AtomicBool::new(false),
                stop: AtomicBool::new(false),
            });
            let (ready, is_ready) = sync_channel(1);
            let thread_shared = Arc::clone(&shared);
            let thread = thread::spawn(move || filter_thread(&thread_shared, &ready));
            let filter = Self {
                shared,
                thread: Some(thread),
            };
            if !is_ready
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or(false)
            {
                return None;
            }
            post_test_key(vk::F20, false);
            wait_for(Duration::from_secs(2), || filter.swallowed() == 1).then_some(filter)
        }

        fn swallowed(&self) -> u64 {
            self.shared.swallowed.load(Ordering::Relaxed)
        }

        fn was_disabled(&self) -> bool {
            self.shared.disabled.load(Ordering::Acquire)
        }
    }

    impl Drop for EventFilter {
        fn drop(&mut self) {
            self.shared.stop.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// Posts one tagged key event into the session's event stream, with no modifier held.
    fn post_test_key(code: u16, down: bool) {
        let source = CGEventSource::new(CGEventSourceStateID::Private);
        let event = CGEvent::new_keyboard_event(source.as_deref(), code, down)
            .expect("the system creates a keyboard event");
        CGEvent::set_flags(Some(&event), CGEventFlags(0));
        CGEvent::set_integer_value_field(Some(&event), CGEventField::EventSourceUserData, TEST_TAG);
        CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&event));
    }

    const KEYS: usize = 10_000;
    const KEYS_SEEN: u64 = 10_000;

    /// The keys of the run: the 26 letter positions, over and over.
    const RUN_KEYS: [u16; 26] = [
        vk::ANSI_A,
        vk::ANSI_B,
        vk::ANSI_C,
        vk::ANSI_D,
        vk::ANSI_E,
        vk::ANSI_F,
        vk::ANSI_G,
        vk::ANSI_H,
        vk::ANSI_I,
        vk::ANSI_J,
        vk::ANSI_K,
        vk::ANSI_L,
        vk::ANSI_M,
        vk::ANSI_N,
        vk::ANSI_O,
        vk::ANSI_P,
        vk::ANSI_Q,
        vk::ANSI_R,
        vk::ANSI_S,
        vk::ANSI_T,
        vk::ANSI_U,
        vk::ANSI_V,
        vk::ANSI_W,
        vk::ANSI_X,
        vk::ANSI_Y,
        vk::ANSI_Z,
    ];

    #[test]
    fn ten_thousand_keys_none_dropped_none_doubled_and_the_callback_stays_fast() {
        const TEST: &str =
            "ten_thousand_keys_none_dropped_none_doubled_and_the_callback_stays_fast";
        if !permitted(TEST) {
            return;
        }
        // The filter goes in first and at the tail, the listener after it at the head: every
        // test key passes the listener and stops at the filter.
        let Some(filter) = EventFilter::start() else {
            skip(
                TEST,
                "the event filter that keeps the test keys away from the focused application \
                 could not be installed or did not remove a tagged event",
            );
            return;
        };
        let us = system_layout(US);
        let expected: Vec<char> = RUN_KEYS
            .iter()
            .map(|code| {
                let (text, _) = raw(&us, *code, 0, 0).expect("the US layout translates a letter");
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    (Some(ch), None) => ch,
                    _ => panic!("the US layout types one character per letter key"),
                }
            })
            .collect();
        let (running, events) = start_on_us_layout();

        for i in 0..KEYS {
            assert!(
                !filter.was_disabled(),
                "the system disabled the event filter after {i} keys; the run was stopped so \
                 that no test key reaches an application"
            );
            post_test_key(RUN_KEYS[i % RUN_KEYS.len()], true);
            // A thousand keys a second: far above any typist, far below a flood.
            thread::sleep(Duration::from_millis(1));
        }
        let all_seen = wait_for(Duration::from_secs(10), || {
            running.stats().key_downs >= KEYS_SEEN
        });
        let stats = running.stats();
        let (health, recovery) = running.tap_health();
        running.stop().expect("the listener stops");
        let delivered: Vec<ListenerEvent> = events.iter().collect();

        assert!(
            all_seen,
            "the tap saw {} of {KEYS} keys: keys were dropped",
            stats.key_downs
        );
        check_run(&delivered, &expected);
        assert_eq!(
            stats.key_downs, KEYS_SEEN,
            "the tap saw keys that were not the test's (do not type during this test)"
        );
        // The filter saw exactly the keys of the run and its one probe.
        assert_eq!(filter.swallowed(), KEYS_SEEN + 1);
        assert_eq!(stats.panics, 0);
        assert_eq!(health, TapHealth::Working);
        assert_eq!(
            recovery.recoveries, 0,
            "the tap was disabled during the run"
        );
        note(&format!(
            "listener callback over {} calls: p99 below {:?}, slowest {:?} (limit {:?})",
            stats.callbacks, stats.callback_p99, stats.callback_slowest, CALLBACK_P99_LIMIT
        ));
        assert!(
            stats.callback_p99 < CALLBACK_P99_LIMIT,
            "callback p99 {:?} is not under {CALLBACK_P99_LIMIT:?}",
            stats.callback_p99
        );
    }

    /// Checks that `delivered` is the run, key for key: nothing dropped, doubled or out of
    /// order, on the sink's side as on the tap's.
    fn check_run(delivered: &[ListenerEvent], expected: &[char]) {
        assert!(
            !delivered.contains(&ListenerEvent::Overflow),
            "the listener reported lost events"
        );
        let mut last_seq = 0;
        let mut count = 0;
        for event in delivered {
            match event {
                ListenerEvent::Composing { seq, .. } => last_seq = *seq,
                ListenerEvent::Char { ch, seq } => {
                    assert_eq!(
                        *seq,
                        last_seq + 1,
                        "a gap or a repeat in the sequence numbers at key {count}"
                    );
                    last_seq = *seq;
                    assert!(
                        *ch.expose() == expected[count % expected.len()],
                        "key {count} is not the key that was sent (do not type during this test)"
                    );
                    count += 1;
                }
                other => panic!("an event that is not a typed character: {other:?}"),
            }
        }
        assert_eq!(count, KEYS, "delivered characters");
    }

    #[test]
    fn a_tap_disabled_without_notice_is_back_within_a_second() {
        const TEST: &str = "a_tap_disabled_without_notice_is_back_within_a_second";
        if !permitted(TEST) {
            return;
        }
        let (running, events) = start_on_us_layout();
        assert_eq!(running.self_test(Duration::from_secs(1)), Ok(()));

        // The system announces a disable that this process asks for; `disable_tap` keeps that
        // notice from the listener (see `Request::DisableTap`), so only the watchdog can find
        // the tap disabled.
        disable_tap(&running);
        let disabled = Instant::now();
        let back = wait_for(Duration::from_secs(5), || {
            running.tap_health().1.recoveries >= 1
        });
        let took = disabled.elapsed();
        assert!(back, "the tap did not come back");
        assert!(
            took < Duration::from_secs(1),
            "recovery took {took:?}, the limit is one second"
        );
        let (health, stats) = running.tap_health();
        assert_eq!(health, TapHealth::Working);
        // Found by the watchdog.
        assert_eq!(stats.unannounced, 1, "{stats:?}");
        assert!(stats.longest_recovery.expect("one recovery") < Duration::from_secs(1));

        // The tap sees events again, and the loss was announced.
        assert_eq!(running.self_test(Duration::from_secs(1)), Ok(()));
        let hidden = running.hidden_disable_notices();
        running.stop().expect("the listener stops");
        let delivered: Vec<ListenerEvent> = events.iter().collect();
        assert!(delivered.contains(&ListenerEvent::Overflow));
        note(&format!(
            "listener tap recovery after a silent disable: {took:?} (disable notices the \
             system sent for the test's own disable, kept from the listener: {hidden})"
        ));
    }

    #[test]
    fn a_self_test_enables_a_disabled_tap_and_passes() {
        const TEST: &str = "a_self_test_enables_a_disabled_tap_and_passes";
        if !permitted(TEST) {
            return;
        }
        let (running, events) = start_on_us_layout();
        disable_tap(&running);
        // The self-test's first event goes past the disabled tap; the test then has the tap
        // looked at, which enables it unless the watchdog already did, and posts once more.
        let started = Instant::now();
        assert_eq!(running.self_test(Duration::from_secs(1)), Ok(()));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(running.tap_health().0, TapHealth::Working);

        running.stop().expect("the listener stops");
        // After the stop a self-test cannot pass: the tap is gone.
        let delivered: Vec<ListenerEvent> = events.iter().collect();
        assert!(delivered.contains(&ListenerEvent::Overflow));
    }

    #[test]
    fn a_registered_hotkey_is_delivered_through_the_listeners_sink() {
        const TEST: &str = "a_registered_hotkey_is_delivered_through_the_listeners_sink";
        if !permitted(TEST) {
            return;
        }
        let platform = Platform::new();
        let (sink, events) = unbounded_sink();
        assert_eq!(platform.start(sink), Ok(()));
        // Hotkeys are registered and dispatched on the main thread. Is anyone there?
        if main_thread::call(Duration::from_millis(500), |_| ()).is_none() {
            skip(
                TEST,
                "hotkeys are registered and dispatched on the main thread, which the standard \
                 test harness keeps for itself; this needs a process whose main thread runs \
                 the application event loop",
            );
            // Without the main thread a registration is refused, not silently noted.
            let chord = Chord::parse("Ctrl+Alt+Shift+F19").expect("a valid chord");
            assert_eq!(
                platform.register(HotkeyId(41), chord),
                Err(PlatformError::Timeout)
            );
            assert_eq!(platform.stop(), Ok(()));
            return;
        }
        let id = HotkeyId(41);
        let chord = Chord::parse("Ctrl+Alt+Shift+F19").expect("a valid chord");
        assert_eq!(platform.register(id, chord), Ok(()));

        // Press the chord the way a keyboard would: at the hardware end of the stream.
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState);
        for down in [true, false] {
            let event = CGEvent::new_keyboard_event(source.as_deref(), vk::F19, down)
                .expect("the system creates a keyboard event");
            CGEvent::set_flags(
                Some(&event),
                CGEventFlags(FLAG_CONTROL | FLAG_OPTION | FLAG_SHIFT),
            );
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut pressed = false;
        while let Ok(event) =
            events.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            if matches!(event, ListenerEvent::Hotkey { id: seen, .. } if seen == id) {
                pressed = true;
                break;
            }
        }
        assert_eq!(platform.unregister(id), Ok(()));
        assert_eq!(platform.stop(), Ok(()));
        assert!(
            pressed,
            "the hotkey press did not arrive in the listener's sink"
        );
    }
}
