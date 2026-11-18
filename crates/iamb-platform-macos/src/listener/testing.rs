//! Test doubles for the listener's seams: keyboard layouts, the tap switch, the input
//! sources and the sink. Compiled for tests only, on every system.
//!
//! The fake layouts follow the contract of `UCKeyTranslate` (see
//! [`KeyLayout`](super::translate::KeyLayout)) but are **not** copies of Apple's layouts: a
//! few keys each, enough to exercise the rules. What the real US, Spanish, German and French
//! layouts do is tested against the system's own layout data on macOS.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use iamb_platform::{EventSink, ListenerEvent, SeqSource};

use super::core::{CoreParts, ListenerCore, RawEvent};
use super::inputs::{InputSnapshot, InputSources};
use super::keycodes::vk;
use super::modifiers::layout_state::{CAPS_LOCK, OPTION, SHIFT};
use super::recovery::TapSwitch;
use super::selftest::SelfTests;
use super::stats::ListenerStats;
use super::translate::{KeyLayout, KeyOutput, KeyPress, MAX_UNITS};

/// The text of a key output, for assertions. Test data only.
pub(crate) fn typed(output: &KeyOutput) -> String {
    output.text().map(|ch| *ch.expose()).collect()
}

/// Events as short strings, for assertions: `a#2`, `boundary:Enter#3`, `overflow`. Test data
/// only: this is the one place that prints a "typed" character.
pub(crate) fn shown(events: &[ListenerEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            ListenerEvent::Char { ch, seq } => format!("{}#{seq}", ch.expose()),
            ListenerEvent::Backspace { seq } => format!("backspace#{seq}"),
            ListenerEvent::Boundary { kind, seq } => format!("boundary:{kind:?}#{seq}"),
            ListenerEvent::Composing { on, seq } => format!("composing:{on}#{seq}"),
            ListenerEvent::InjectionStarted { expansion, seq } => {
                format!("started:{}#{seq}", expansion.raw())
            }
            ListenerEvent::InjectionFinished { expansion, seq } => {
                format!("finished:{}#{seq}", expansion.raw())
            }
            ListenerEvent::Hotkey { id, seq } => format!("hotkey:{}#{seq}", id.0),
            ListenerEvent::Overflow => "overflow".to_owned(),
        })
        .collect()
}

/// What a key of a fake layout does.
#[derive(Clone)]
enum Stroke {
    /// Types text.
    Text(String),
    /// A dead key with this state.
    Dead(u32),
    /// The layout cannot translate the key.
    Fail,
    /// Writes these units and claims this length.
    Raw(Vec<u16>, usize),
}

/// A small keyboard layout with dead keys.
pub(crate) struct FakeLayout {
    keys: HashMap<(u16, u32), Stroke>,
    /// What a dead-key state types on its own.
    accents: HashMap<u32, char>,
    compositions: HashMap<(u32, char), char>,
    calls: AtomicU64,
    last: Mutex<Option<(u16, u32)>>,
}

const ACUTE: u32 = 1;
const CIRCUMFLEX: u32 = 2;

/// Letters at their US positions.
const QWERTY: [(u16, char); 26] = [
    (vk::ANSI_A, 'a'),
    (vk::ANSI_B, 'b'),
    (vk::ANSI_C, 'c'),
    (vk::ANSI_D, 'd'),
    (vk::ANSI_E, 'e'),
    (vk::ANSI_F, 'f'),
    (vk::ANSI_G, 'g'),
    (vk::ANSI_H, 'h'),
    (vk::ANSI_I, 'i'),
    (vk::ANSI_J, 'j'),
    (vk::ANSI_K, 'k'),
    (vk::ANSI_L, 'l'),
    (vk::ANSI_M, 'm'),
    (vk::ANSI_N, 'n'),
    (vk::ANSI_O, 'o'),
    (vk::ANSI_P, 'p'),
    (vk::ANSI_Q, 'q'),
    (vk::ANSI_R, 'r'),
    (vk::ANSI_S, 's'),
    (vk::ANSI_T, 't'),
    (vk::ANSI_U, 'u'),
    (vk::ANSI_V, 'v'),
    (vk::ANSI_W, 'w'),
    (vk::ANSI_X, 'x'),
    (vk::ANSI_Y, 'y'),
    (vk::ANSI_Z, 'z'),
];

impl FakeLayout {
    /// The Spanish layout's dead acute accent (the key right of Ñ).
    pub(crate) const SPANISH_ACUTE: u16 = 0x27;
    /// The French layout's dead circumflex (the key right of P).
    pub(crate) const FRENCH_CIRCUMFLEX: u16 = 0x21;
    /// A key every fake layout fails to translate.
    pub(crate) const FAILING_KEY: u16 = 0x2A;
    /// A key whose only output is a control code.
    pub(crate) const CONTROL_CODE_KEY: u16 = 0x2B;
    /// A key that types one character outside the basic plane.
    pub(crate) const EMOJI_KEY: u16 = 0x2C;
    /// A key that writes half a surrogate pair.
    pub(crate) const LONE_SURROGATE_KEY: u16 = 0x2F;
    /// A key that claims to have written more than the buffer holds.
    pub(crate) const OVERLONG_KEY: u16 = 0x29;

    fn with_letters(swaps: &[(char, char)]) -> Self {
        let mut layout = Self {
            keys: HashMap::new(),
            accents: HashMap::from([(ACUTE, '´'), (CIRCUMFLEX, '^')]),
            compositions: HashMap::from([
                ((ACUTE, 'e'), 'é'),
                ((ACUTE, 'E'), 'É'),
                ((ACUTE, 'a'), 'á'),
                ((CIRCUMFLEX, 'e'), 'ê'),
                ((CIRCUMFLEX, 'E'), 'Ê'),
                ((CIRCUMFLEX, 'o'), 'ô'),
            ]),
            calls: AtomicU64::new(0),
            last: Mutex::new(None),
        };
        for (code, letter) in QWERTY {
            let letter = swaps
                .iter()
                .find_map(|&(a, b)| match letter {
                    l if l == a => Some(b),
                    l if l == b => Some(a),
                    _ => None,
                })
                .unwrap_or(letter);
            let upper = letter.to_ascii_uppercase();
            layout.text(code, 0, letter);
            for state in [SHIFT, CAPS_LOCK, SHIFT | CAPS_LOCK] {
                layout.text(code, state, upper);
            }
        }
        for (code, digit) in [(vk::ANSI_1, '1'), (vk::ANSI_2, '2'), (vk::ANSI_7, '7')] {
            layout.text(code, 0, digit);
        }
        layout.text(vk::SPACE, 0, ' ');
        layout.text(vk::SPACE, SHIFT, ' ');
        layout.keys.insert((Self::FAILING_KEY, 0), Stroke::Fail);
        layout.text(Self::CONTROL_CODE_KEY, 0, '\u{10}');
        layout.text(Self::EMOJI_KEY, 0, '😀');
        layout
            .keys
            .insert((Self::LONE_SURROGATE_KEY, 0), Stroke::Raw(vec![0xD83D], 1));
        layout.keys.insert(
            (Self::OVERLONG_KEY, 0),
            Stroke::Raw(vec![u16::from(b'x'); MAX_UNITS], 99),
        );
        layout
    }

    fn text(&mut self, code: u16, state: u32, ch: char) {
        self.keys
            .insert((code, state), Stroke::Text(ch.to_string()));
    }

    fn dead(&mut self, code: u16, state: u32, dead: u32) {
        self.keys.insert((code, state), Stroke::Dead(dead));
    }

    /// A US-like layout: Option+E is a dead acute accent.
    pub(crate) fn us() -> Self {
        let mut layout = Self::with_letters(&[]);
        layout.text(vk::ANSI_1, SHIFT, '!');
        layout.text(vk::ANSI_2, SHIFT, '@');
        layout.dead(vk::ANSI_E, OPTION, ACUTE);
        layout.text(vk::ANSI_L, OPTION, '¬');
        layout.text(vk::SPACE, OPTION, '\u{a0}');
        layout.text(vk::ANSI_K, OPTION | SHIFT, '\u{f8ff}');
        layout
    }

    /// A German-like layout: Y and Z swapped, `@` on Option+L.
    pub(crate) fn german() -> Self {
        let mut layout = Self::with_letters(&[('y', 'z')]);
        layout.text(vk::ANSI_L, OPTION, '@');
        layout.text(vk::ANSI_7, OPTION | SHIFT, '\\');
        layout
    }

    /// A Spanish-like layout: a dead acute accent on its own key.
    pub(crate) fn spanish() -> Self {
        let mut layout = Self::with_letters(&[]);
        layout.dead(Self::SPANISH_ACUTE, 0, ACUTE);
        layout
    }

    /// A French-like layout: A and Q swapped, a dead circumflex on its own key.
    pub(crate) fn french() -> Self {
        let mut layout = Self::with_letters(&[('a', 'q'), ('z', 'w')]);
        layout.dead(Self::FRENCH_CIRCUMFLEX, 0, CIRCUMFLEX);
        layout
    }

    /// How many presses the layout was asked to translate.
    pub(crate) fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    fn last(&self) -> Option<(u16, u32)> {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The `keyAction` of the last press.
    pub(crate) fn last_action(&self) -> Option<u16> {
        self.last().map(|(action, _)| action)
    }

    /// The keyboard type of the last press.
    pub(crate) fn last_keyboard_type(&self) -> Option<u32> {
        self.last().map(|(_, keyboard_type)| keyboard_type)
    }
}

fn write(out: &mut [u16; MAX_UNITS], text: impl Iterator<Item = char>) -> usize {
    let mut len = 0;
    for ch in text {
        let mut units = [0; 2];
        for unit in ch.encode_utf16(&mut units) {
            if let Some(slot) = out.get_mut(len) {
                *slot = *unit;
                len += 1;
            }
        }
    }
    len
}

impl KeyLayout for FakeLayout {
    fn translate(
        &self,
        press: &KeyPress,
        dead_state: &mut u32,
        out: &mut [u16; MAX_UNITS],
    ) -> Option<usize> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((press.action.code(), press.keyboard_type));
        let previous = std::mem::take(dead_state);
        // Like the system's layouts: only the low word is the waiting dead key, and a key that
        // ends one leaves the ended state in the high word (observed: 0x1, then E, gives 0x10000).
        let pending = previous & 0xFFFF;
        let accent = self.accents.get(&pending).copied();
        let typed = match self.keys.get(&(press.code, press.modifiers)) {
            None => {
                // A key without output leaves a pending dead key pending.
                *dead_state = previous;
                Some(0)
            }
            Some(Stroke::Fail) => None,
            Some(Stroke::Raw(units, claimed)) => {
                for (slot, unit) in out.iter_mut().zip(units) {
                    *slot = *unit;
                }
                Some(*claimed)
            }
            Some(Stroke::Dead(dead)) => match accent {
                None => {
                    *dead_state = *dead;
                    Some(0)
                }
                // Two dead keys in a row type both accents.
                Some(first) => {
                    let second = self.accents.get(dead).copied();
                    Some(write(out, [Some(first), second].into_iter().flatten()))
                }
            },
            Some(Stroke::Text(text)) => {
                let mut chars = text.chars();
                let single = match (chars.next(), chars.next()) {
                    (Some(ch), None) => Some(ch),
                    _ => None,
                };
                let combined = single.and_then(|ch| self.compositions.get(&(pending, ch)));
                Some(match (accent, combined) {
                    (_, Some(combined)) => write(out, [*combined].into_iter()),
                    // Space after a dead key types the accent alone.
                    (Some(accent), None) if text == " " => write(out, [accent].into_iter()),
                    (Some(accent), None) => write(out, [accent].into_iter().chain(text.chars())),
                    (None, None) => write(out, text.chars()),
                })
            }
        };
        if pending != 0 && *dead_state == 0 {
            *dead_state = pending << 16;
        }
        typed
    }
}

#[derive(Debug, Default)]
struct TapState {
    enabled: AtomicBool,
    refuses: AtomicBool,
    enable_calls: AtomicU32,
}

/// The switch of a tap that does not exist. Clones share the state, so a test keeps one
/// while the core owns the other.
#[derive(Debug, Clone)]
pub(crate) struct FakeTap {
    state: Arc<TapState>,
}

impl FakeTap {
    /// An enabled tap.
    pub(crate) fn enabled() -> Self {
        let tap = Self {
            state: Arc::default(),
        };
        tap.state.enabled.store(true, Ordering::Relaxed);
        tap
    }

    /// The system disables the tap.
    pub(crate) fn system_disables(&self) {
        self.state.enabled.store(false, Ordering::Relaxed);
    }

    /// While set, `enable` has no effect: the permission is gone.
    pub(crate) fn refuse_to_enable(&self, refuse: bool) {
        self.state.refuses.store(refuse, Ordering::Relaxed);
    }

    /// How often `enable` was called.
    pub(crate) fn enable_calls(&self) -> u32 {
        self.state.enable_calls.load(Ordering::Relaxed)
    }
}

impl TapSwitch for FakeTap {
    fn enable(&mut self) {
        self.state.enable_calls.fetch_add(1, Ordering::Relaxed);
        if !self.state.refuses.load(Ordering::Relaxed) {
            self.state.enabled.store(true, Ordering::Relaxed);
        }
    }

    fn is_enabled(&self) -> bool {
        self.state.enabled.load(Ordering::Relaxed)
    }
}

/// Input sources a test sets by hand.
pub(crate) struct FakeInputs {
    snapshot: Mutex<Option<Arc<InputSnapshot>>>,
    generation: AtomicU64,
    refresh_requests: AtomicU64,
}

impl FakeInputs {
    fn with(snapshot: Option<InputSnapshot>) -> Arc<Self> {
        Arc::new(Self {
            snapshot: Mutex::new(snapshot.map(Arc::new)),
            generation: AtomicU64::new(0),
            refresh_requests: AtomicU64::new(0),
        })
    }

    /// A keyboard layout is selected.
    pub(crate) fn layout(layout: impl KeyLayout + 'static) -> Arc<Self> {
        Self::with(Some(InputSnapshot {
            layout: Some(Arc::new(layout)),
            input_method: false,
        }))
    }

    /// An input method is selected; `layout` is the layout it types through.
    pub(crate) fn input_method(layout: impl KeyLayout + 'static) -> Arc<Self> {
        Self::with(Some(InputSnapshot {
            layout: Some(Arc::new(layout)),
            input_method: true,
        }))
    }

    /// A keyboard layout that is shared with the test is selected.
    #[cfg(target_os = "macos")]
    pub(crate) fn shared_layout(layout: Arc<dyn KeyLayout>) -> Arc<Self> {
        Self::with(Some(InputSnapshot {
            layout: Some(layout),
            input_method: false,
        }))
    }

    /// Nothing is known yet.
    pub(crate) fn unknown() -> Arc<Self> {
        Self::with(None)
    }

    /// Publishes a new snapshot.
    pub(crate) fn publish(&self, layout: Option<FakeLayout>, input_method: bool) {
        let layout = layout.map(|layout| Arc::new(layout) as Arc<dyn KeyLayout>);
        *self.snapshot.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(Arc::new(InputSnapshot {
                layout,
                input_method,
            }));
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// How often a refresh was asked for.
    pub(crate) fn refresh_requests(&self) -> u64 {
        self.refresh_requests.load(Ordering::Relaxed)
    }
}

impl InputSources for FakeInputs {
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    fn current(&self) -> Option<Arc<InputSnapshot>> {
        self.snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn request_refresh(&self) {
        self.refresh_requests.fetch_add(1, Ordering::Relaxed);
    }
}

/// A sink that keeps what it is given.
#[derive(Clone, Default)]
pub(crate) struct Recorder {
    events: Arc<Mutex<Vec<ListenerEvent>>>,
}

impl Recorder {
    /// Takes the events recorded so far.
    pub(crate) fn take(&self) -> Vec<ListenerEvent> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl EventSink for Recorder {
    fn deliver(&mut self, event: ListenerEvent) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

/// A listener core with every seam faked, and handles on the fakes.
pub(crate) struct Rig {
    pub(crate) core: ListenerCore,
    pub(crate) tap: FakeTap,
    pub(crate) inputs: Arc<FakeInputs>,
    pub(crate) self_tests: Arc<SelfTests>,
    pub(crate) stats: Arc<ListenerStats>,
    pub(crate) seq: Arc<SeqSource>,
    recorder: Recorder,
}

impl Rig {
    /// A core that records what it delivers.
    pub(crate) fn new(inputs: Arc<FakeInputs>) -> Self {
        let recorder = Recorder::default();
        let mut rig = Self::with_sink(inputs, Box::new(recorder.clone()));
        rig.recorder = recorder;
        rig
    }

    /// A core that delivers to `sink`; [`take`](Self::take) then returns nothing.
    pub(crate) fn with_sink(inputs: Arc<FakeInputs>, sink: Box<dyn EventSink>) -> Self {
        let tap = FakeTap::enabled();
        let self_tests = Arc::new(SelfTests::new());
        let stats = Arc::new(ListenerStats::default());
        let seq = Arc::new(SeqSource::new());
        let core = ListenerCore::new(CoreParts {
            seq: Arc::clone(&seq),
            sink,
            tap: Box::new(tap.clone()),
            inputs: Arc::clone(&inputs) as Arc<dyn InputSources>,
            self_tests: Arc::clone(&self_tests),
            stats: Arc::clone(&stats),
        });
        Self {
            core,
            tap,
            inputs,
            self_tests,
            stats,
            seq,
            recorder: Recorder::default(),
        }
    }

    /// Feeds one event to the core, as the tap callback would.
    pub(crate) fn send(&mut self, raw: &RawEvent) {
        self.core.handle(raw, Instant::now());
    }

    /// The events delivered since the last call, as short strings.
    pub(crate) fn take(&mut self) -> Vec<String> {
        shown(&self.recorder.take())
    }
}
