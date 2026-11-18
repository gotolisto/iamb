//! Dead keys, tracked by the listener itself (architecture §4.1).
//!
//! A dead key (´ on a Spanish keyboard, ^ on a French one) types nothing by itself: the next
//! key decides. The listener may not ask the system to keep that state for it, because the
//! system keeps one dead key per layout for every program at once: a translation that stored
//! or used it would change what the application in front receives (the classic symptom is a
//! doubled accent). So the listener translates each key in the mode that leaves that state
//! alone ([`Layout::translate`]) and composes with the layout's own dead-key table
//! ([`Layout::compose`], read from the layout's file, never by typing at the system).
//!
//! # The rules, which are the system's own
//!
//! With a dead character `d` waiting and a key that translates to the character `c`:
//!
//! - the table has `(d, c)`: the application receives the composed character (´ + e → é);
//! - the table marks the result as dead again: nothing is received, the result waits;
//! - the table has no entry: the application receives `d`, then `c` (´ + q → ´q). The key may
//!   be anything that yields a character, another dead key (´ + ´ → ´´) or a control
//!   character (Backspace after ´ inserts ´ and deletes it again) included;
//! - a key that yields no character (an arrow, a function key, a modifier) changes nothing:
//!   `d` keeps waiting.
//!
//! # A character injected while a dead key waits
//!
//! A character another program injects arrives as the pseudo key `VK_PACKET`, the character in
//! the event instead of on a key of the layout. The table plays no part: the system hands the
//! character over by itself. What it does with a waiting `d` is not documented. The system's
//! own stateful translation, given a dead key and then a `VK_PACKET`, returns the packet's
//! character alone, without `d` (Windows CI, run 44331151855: a Spanish `` ` `` and then
//! `VK_PACKET` gave U+0000 alone); whether `d` was dropped or still waits for the key after it
//! was not observed, nor what an application that does not use the system's translation for
//! `VK_PACKET` receives. So the listener does not guess:
//! [`character_injected`](DeadKeys::character_injected) says a boundary is due, and `d` becomes
//! uncertain, so that the next key that yields text is not guessed either.
//!
//! # Whether the stateless mode reads the system's dead key
//!
//! The documentation of the mode says the keyboard state "is not changed". It does not say
//! whether a dead key that the *application's* own translation has stored is still *used*: a
//! system that reads it would hand the listener `é` for the E key once the application has
//! processed the ´, and `e` before. The listener cannot be tested against that from the
//! build machine, so it is correct either way: each layout that has dead keys supplies one of
//! them as a [`Probe`], and a key is translated between two translations of the probe. A
//! probe that does not come back as the plain dead key means the system is applying a stored
//! dead key right now, and then what it returns for the key already is what the application
//! will receive. If the system never reads the stored dead key, the probe always comes back
//! plain and the table decides.
//!
//! When the listener cannot know what the application received (no table for the layout, a
//! command in the middle of a sequence, the system composing with a dead key the listener is
//! not tracking), it says so ([`Resolved::Unknown`]) and the listener reports a boundary. It
//! never guesses a character.
//!
//! # Privacy
//!
//! The waiting dead character is a typed character. It is held in a
//! [`Sensitive`], overwritten when it is used, and never formatted. The probe is a property of
//! the layout, not something the user typed.

use std::fmt;

use iamb_platform::core_types::Sensitive;

use super::keys::{VK_LCONTROL, VK_LMENU, VK_LSHIFT};
use super::keystate::KeyState;

/// The most UTF-16 units one key press delivers. Layouts define ligatures of up to five.
pub(crate) const MAX_UNITS: usize = 8;

/// How often a translation is repeated when the system's dead-key state changes under it.
const ATTEMPTS: usize = 3;

/// The UTF-16 units one key press delivers to the application, in order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Units {
    len: usize,
    units: [u16; MAX_UNITS],
}

impl Units {
    /// The first [`MAX_UNITS`] of `units`.
    pub(crate) fn new(units: &[u16]) -> Self {
        let mut copy = [0; MAX_UNITS];
        let len = units.len().min(MAX_UNITS);
        copy[..len].copy_from_slice(&units[..len]);
        Self { len, units: copy }
    }

    /// The units.
    pub(crate) fn as_slice(&self) -> &[u16] {
        &self.units[..self.len]
    }
}

impl fmt::Debug for Units {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Units")
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

/// What the layout says a key press produces while no dead key is waiting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Translation {
    /// No character.
    None,
    /// The key is a dead key; this is its character.
    Dead(u16),
    /// These units.
    Text(Units),
}

impl Translation {
    /// A translation to `units`.
    pub(crate) fn text(units: &[u16]) -> Self {
        Self::Text(Units::new(units))
    }
}

impl fmt::Debug for Translation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Dead(_) => "Dead(..)",
            Self::Text(_) => "Text(..)",
        })
    }
}

/// What the layout's dead-key table says about a dead character followed by a character.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Composition {
    /// They compose to this character.
    Char(u16),
    /// They compose to this character, which is dead again.
    Dead(u16),
    /// The table has no entry: both characters are delivered.
    None,
    /// The table of this layout is not available.
    Unknown,
}

impl fmt::Debug for Composition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Char(_) => "Char(..)",
            Self::Dead(_) => "Dead(..)",
            Self::None => "None",
            Self::Unknown => "Unknown",
        })
    }
}

/// One key press to translate: the key and the keys held with it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyQuery {
    /// The virtual key.
    pub(crate) vk: u32,
    /// The scan code.
    pub(crate) scan: u32,
    /// The keyboard at that moment.
    pub(crate) keys: KeyState,
}

impl fmt::Debug for KeyQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyQuery").finish_non_exhaustive()
    }
}

/// A dead key of a layout and the character it translates to while nothing is waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Probe {
    /// The key.
    pub(crate) key: KeyQuery,
    /// Its dead character.
    pub(crate) ch: u16,
}

/// The layout of the application in front: the thin calls to the system.
pub(crate) trait Layout {
    /// What `key` produces, without changing the system's keyboard state.
    fn translate(&mut self, key: &KeyQuery) -> Translation;

    /// The layout's dead-key table entry for `dead` followed by `base`.
    fn compose(&mut self, dead: u16, base: u16) -> Composition;

    /// A dead key of this layout, if it has any and it is known.
    fn probe(&mut self) -> Option<Probe>;
}

/// Finds a dead key of a layout by translating every key with no modifier, Shift, `AltGr` and
/// both. `translate` must not change the system's keyboard state. A candidate counts only if
/// it translates the same way twice and `is_dead` accepts its character (the layout's table
/// lists it), so a dead key the system happens to be holding cannot produce a false probe.
pub(crate) fn find_probe(
    mut translate: impl FnMut(&KeyQuery) -> Translation,
    mut is_dead: impl FnMut(u16) -> bool,
) -> Option<Probe> {
    let states = [
        KeyState::holding(&[]),
        KeyState::holding(&[VK_LSHIFT]),
        KeyState::holding(&[VK_LCONTROL, VK_LMENU]),
        KeyState::holding(&[VK_LSHIFT, VK_LCONTROL, VK_LMENU]),
    ];
    for keys in states {
        // Space to the last layout-defined key; state keys and non-keys translate to nothing.
        for vk in 0x20..0xFF {
            let key = KeyQuery { vk, scan: 0, keys };
            if let Translation::Dead(ch) = translate(&key)
                && is_dead(ch)
                && translate(&key) == Translation::Dead(ch)
            {
                return Some(Probe { key, ch });
            }
        }
    }
    None
}

/// A layout's dead-key table: which pairs compose, and to what.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct DeadKeyTable {
    /// Sorted by `(dead, base)`; the first entry of a pair wins, as in the system's search.
    entries: Vec<Entry>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Entry {
    dead: u16,
    base: u16,
    composed: u16,
    dead_again: bool,
}

impl DeadKeyTable {
    /// A table of `(dead, base, composed, dead again)` entries, in the layout's order.
    pub(crate) fn new(entries: impl IntoIterator<Item = (u16, u16, u16, bool)>) -> Self {
        let mut entries: Vec<Entry> = entries
            .into_iter()
            .map(|(dead, base, composed, dead_again)| Entry {
                dead,
                base,
                composed,
                dead_again,
            })
            .collect();
        // A stable sort keeps the layout's order among duplicates; the first one wins.
        entries.sort_by_key(|e| (e.dead, e.base));
        entries.dedup_by_key(|e| (e.dead, e.base));
        Self { entries }
    }

    /// The entry for `dead` followed by `base`. Never [`Composition::Unknown`].
    pub(crate) fn compose(&self, dead: u16, base: u16) -> Composition {
        match self
            .entries
            .binary_search_by_key(&(dead, base), |e| (e.dead, e.base))
        {
            Ok(i) if self.entries[i].dead_again => Composition::Dead(self.entries[i].composed),
            Ok(i) => Composition::Char(self.entries[i].composed),
            Err(_) => Composition::None,
        }
    }

    /// True when `ch` is a dead character of this layout: the first of some pair, or the
    /// dead-again result of one.
    pub(crate) fn is_dead(&self, ch: u16) -> bool {
        self.entries
            .iter()
            .any(|e| e.dead == ch || (e.dead_again && e.composed == ch))
    }

    /// Number of pairs.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// True for a layout without dead keys.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl fmt::Debug for DeadKeyTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeadKeyTable")
            .field("pairs", &self.entries.len())
            .finish()
    }
}

/// How often the search for a layout's probe is repeated before the listener gives up.
const PROBE_SEARCHES: u8 = 3;

/// What the listener knows about the dead keys of one layout: the table from the layout's
/// file and the probe.
///
/// A layout that has dead keys needs both. Without the probe the listener could not tell
/// whether the system has already applied a stored dead key to a translation (see the module
/// documentation), so until a probe is found every composition is [`Composition::Unknown`].
/// The search can come up empty for a moment (it must not mistake a dead key the system
/// happens to be holding for the plain state), so it is repeated a few times.
#[derive(Debug)]
pub(crate) struct LayoutDeadKeys {
    /// `None` when the layout's file could not be read.
    table: Option<DeadKeyTable>,
    probe: Option<Probe>,
    searches_left: u8,
}

impl LayoutDeadKeys {
    /// What is known about a layout with this `table`, whose keys `translate` translates
    /// without changing the system's keyboard state.
    pub(crate) fn new(
        table: Option<DeadKeyTable>,
        translate: impl FnMut(&KeyQuery) -> Translation,
    ) -> Self {
        let mut known = Self {
            table,
            probe: None,
            searches_left: PROBE_SEARCHES,
        };
        known.search(translate);
        known
    }

    /// True while the layout has dead keys, no probe has been found, and searching again is
    /// worth it.
    pub(crate) fn wants_search(&self) -> bool {
        self.searches_left > 0
            && self.probe.is_none()
            && self.table.as_ref().is_some_and(|table| !table.is_empty())
    }

    /// Looks for the probe, if one is still wanted.
    pub(crate) fn search(&mut self, translate: impl FnMut(&KeyQuery) -> Translation) {
        if !self.wants_search() {
            return;
        }
        self.searches_left -= 1;
        if let Some(table) = &self.table {
            self.probe = find_probe(translate, |ch| table.is_dead(ch));
        }
    }

    /// The probe.
    pub(crate) const fn probe(&self) -> Option<Probe> {
        self.probe
    }

    /// The table entry for `dead` followed by `base`; [`Composition::Unknown`] without the
    /// table, and without the probe of a layout that has dead keys.
    pub(crate) fn compose(&self, dead: u16, base: u16) -> Composition {
        match &self.table {
            Some(table) if table.is_empty() || self.probe.is_some() => table.compose(dead, base),
            Some(_) | None => Composition::Unknown,
        }
    }

    /// The table, if the layout's file could be read.
    #[cfg(test)]
    pub(crate) const fn table(&self) -> Option<&DeadKeyTable> {
        self.table.as_ref()
    }
}

/// What one key press delivers to the application, as far as the listener can tell.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// No character. A waiting dead key keeps waiting.
    Nothing,
    /// The key was a dead key (or composed to one): nothing yet.
    Held,
    /// These units, in order.
    Text(Units),
    /// Something, but the listener cannot know what.
    Unknown,
}

impl fmt::Debug for Resolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Nothing => "Nothing",
            Self::Held => "Held",
            Self::Text(_) => "Text(..)",
            Self::Unknown => "Unknown",
        })
    }
}

#[derive(Default)]
enum State {
    /// No dead key is waiting.
    #[default]
    Clear,
    /// This dead character is waiting.
    Pending(Sensitive<u16>),
    /// A dead key was waiting and something the listener cannot follow may have used it.
    Uncertain,
}

/// The state as a value, for one decision.
#[derive(Clone, Copy)]
enum Waiting {
    Nothing,
    Dead(u16),
    Unknown,
}

/// The listener's own dead-key state for the layout in front.
#[derive(Default)]
pub(crate) struct DeadKeys {
    state: State,
}

impl fmt::Debug for DeadKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeadKeys")
            .field("waiting", &self.is_waiting())
            .finish_non_exhaustive()
    }
}

impl DeadKeys {
    /// True while a dead key waits for the next key, or may still be waiting.
    pub(crate) const fn is_waiting(&self) -> bool {
        !matches!(self.state, State::Clear)
    }

    /// Forgets everything: the layout changed, or events were lost.
    pub(crate) fn reset(&mut self) {
        self.set(State::Clear);
    }

    /// Something the listener cannot follow happened to the application's dead-key state (a
    /// command chord, an injected key): a waiting dead key may or may not have been used.
    pub(crate) fn disturb(&mut self) {
        if self.is_waiting() {
            self.set(State::Uncertain);
        }
    }

    /// A character another program injected (`VK_PACKET`) went by. True when a dead key was
    /// waiting, or may have been: what the application received for the character is then
    /// not known, and the listener reports a boundary instead (see the module documentation).
    /// The waiting dead key becomes uncertain.
    pub(crate) fn character_injected(&mut self) -> bool {
        let waiting = self.is_waiting();
        self.disturb();
        waiting
    }

    /// Replaces the state, overwriting a waiting character first.
    fn set(&mut self, next: State) {
        if let State::Pending(ch) = &mut self.state {
            *ch.expose_mut() = std::hint::black_box(0);
        }
        self.state = next;
    }

    /// What pressing `key` delivers to the application.
    pub(crate) fn resolve(&mut self, layout: &mut impl Layout, key: &KeyQuery) -> Resolved {
        let Some(observed) = observe(layout, key) else {
            // The system's dead-key state kept changing under the translation.
            self.set(State::Uncertain);
            return Resolved::Unknown;
        };
        let translation = observed.translation;
        if translation == Translation::None {
            return Resolved::Nothing;
        }
        let waiting = match &self.state {
            State::Clear => Waiting::Nothing,
            State::Pending(ch) => Waiting::Dead(*ch.expose()),
            State::Uncertain => Waiting::Unknown,
        };
        match (waiting, observed.applying) {
            // Nothing waits anywhere: the translation is the answer.
            (Waiting::Nothing, None) => match translation {
                Translation::Dead(ch) => self.hold(ch),
                Translation::Text(units) => Resolved::Text(units),
                Translation::None => Resolved::Nothing,
            },
            // The listener's dead key waits and the system has not stored it (yet, or ever):
            // the table decides.
            (Waiting::Dead(dead), None) => match translation {
                Translation::Dead(base) => self.combine(layout, dead, base),
                Translation::Text(units) => {
                    if let [base] = *units.as_slice() {
                        self.combine(layout, dead, base)
                    } else {
                        // A ligature after a dead key: the table has no rule for it. The
                        // application's dead key is used up either way.
                        self.reset();
                        Resolved::Unknown
                    }
                }
                Translation::None => Resolved::Nothing,
            },
            // The system has stored the listener's dead key and already applied it.
            (Waiting::Dead(dead), Some((probe, seen))) if stored_is(layout, dead, probe, seen) => {
                match translation {
                    Translation::Dead(ch) => self.hold(ch),
                    Translation::Text(units) => {
                        self.reset();
                        Resolved::Text(units)
                    }
                    Translation::None => Resolved::Nothing,
                }
            }
            // The listener lost track, and the system is not composing: a key that is not a
            // dead key leaves nothing waiting in the application, whatever was before.
            (Waiting::Unknown, None) => {
                if matches!(translation, Translation::Text(_)) {
                    self.reset();
                }
                Resolved::Unknown
            }
            // The system is composing with a dead key that is not the one the listener
            // tracks: the application is still working through earlier keys. What this key
            // will give it, and whether a dead key waits afterwards, cannot be known.
            (_, Some(_)) => {
                self.set(State::Uncertain);
                Resolved::Unknown
            }
        }
    }

    fn hold(&mut self, ch: u16) -> Resolved {
        self.set(State::Pending(Sensitive::new(ch)));
        Resolved::Held
    }

    fn combine(&mut self, layout: &mut impl Layout, dead: u16, base: u16) -> Resolved {
        match layout.compose(dead, base) {
            Composition::Char(composed) => {
                self.reset();
                Resolved::Text(Units::new(&[composed]))
            }
            Composition::Dead(composed) => self.hold(composed),
            Composition::None => {
                self.reset();
                Resolved::Text(Units::new(&[dead, base]))
            }
            Composition::Unknown => {
                self.reset();
                Resolved::Unknown
            }
        }
    }
}

/// One key translated, and what the translation showed about the system's stored dead key.
struct Observed {
    /// What the key translates to.
    translation: Translation,
    /// When the system applied a stored dead key: the probe, and what the probe translated
    /// to.
    applying: Option<(Probe, Translation)>,
}

/// Translates `key` between two translations of the layout's probe. `None` when the system's
/// dead-key state changed during every attempt.
fn observe(layout: &mut impl Layout, key: &KeyQuery) -> Option<Observed> {
    let Some(probe) = layout.probe() else {
        return Some(Observed {
            translation: layout.translate(key),
            applying: None,
        });
    };
    for _ in 0..ATTEMPTS {
        let before = layout.translate(&probe.key);
        let translation = layout.translate(key);
        let after = layout.translate(&probe.key);
        if before == after {
            let plain = before == Translation::Dead(probe.ch);
            return Some(Observed {
                translation,
                applying: (!plain).then_some((probe, before)),
            });
        }
    }
    None
}

/// True when `seen`, the probe's translation, is what the system returns for the probe while
/// it has stored the dead character `dead`.
fn stored_is(layout: &mut impl Layout, dead: u16, probe: Probe, seen: Translation) -> bool {
    let expected = match layout.compose(dead, probe.ch) {
        Composition::Char(composed) => Translation::text(&[composed]),
        Composition::Dead(composed) => Translation::Dead(composed),
        Composition::None => Translation::text(&[dead, probe.ch]),
        Composition::Unknown => return false,
    };
    seen == expected
}

#[cfg(test)]
pub(crate) mod testing {
    //! A layout for tests: a key map, a dead-key table, and an application whose own
    //! translation stores and uses dead keys the way the system does.

    use std::collections::VecDeque;

    use super::*;

    /// What a key of the fake layout produces by itself.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Raw {
        /// A dead key with this character.
        Dead(char),
        /// This character.
        Char(char),
        /// These two characters (a ligature).
        Pair(char, char),
    }

    /// A keyboard with Shift, `AltGr` (Ctrl and Alt), both or neither held.
    pub(crate) fn held(shift: bool, altgr: bool) -> KeyState {
        let mut keys = Vec::new();
        if shift {
            keys.push(VK_LSHIFT);
        }
        if altgr {
            keys.extend([VK_LCONTROL, VK_LMENU]);
        }
        KeyState::holding(&keys)
    }

    /// The unit of a character of the Basic Multilingual Plane.
    pub(crate) fn unit(ch: char) -> u16 {
        u16::try_from(u32::from(ch)).expect("a character of the Basic Multilingual Plane")
    }

    /// A fake layout and the application in front of it.
    #[derive(Debug)]
    pub(crate) struct FakeLayout {
        /// `(vk, shift, altgr)` → what the key produces.
        pub(crate) keys: Vec<((u32, bool, bool), Raw)>,
        /// The dead-key table; `None` when the layout's file could not be read.
        pub(crate) table: Option<DeadKeyTable>,
        /// The probe the layout offers.
        pub(crate) probe: Option<Probe>,
        /// True when the stateless translation still reads the dead key the application's
        /// translation stored.
        pub(crate) reads_stored: bool,
        /// The dead character the application's translation has stored.
        pub(crate) stored: Option<u16>,
        /// Keys the application has not translated yet.
        pub(crate) backlog: VecDeque<KeyQuery>,
        /// When set, the application translates one key of its backlog after this many
        /// further stateless translations (a race with the listener).
        pub(crate) race_after: Option<usize>,
        /// Number of stateless translations so far.
        pub(crate) translations: usize,
    }

    impl FakeLayout {
        /// A layout with these keys and table entries, a probe for its first dead key, a
        /// stateless translation that does not read the stored dead key, and an application
        /// that has translated everything.
        pub(crate) fn new(
            keys: &[((u32, bool, bool), Raw)],
            table: &[(char, char, char, bool)],
        ) -> Self {
            let table = DeadKeyTable::new(table.iter().map(|&(dead, base, composed, again)| {
                (unit(dead), unit(base), unit(composed), again)
            }));
            let mut layout = Self {
                keys: keys.to_vec(),
                table: Some(table),
                probe: None,
                reads_stored: false,
                stored: None,
                backlog: VecDeque::new(),
                race_after: None,
                translations: 0,
            };
            layout.probe = layout.keys.iter().find_map(|&((vk, shift, altgr), raw)| {
                let Raw::Dead(ch) = raw else { return None };
                Some(Probe {
                    key: KeyQuery {
                        vk,
                        scan: 0,
                        keys: held(shift, altgr),
                    },
                    ch: unit(ch),
                })
            });
            layout
        }

        /// A Spanish-like layout: the acute accent and the diaeresis on one dead key, the
        /// grave accent and the circumflex on another, and an `AltGr` level.
        pub(crate) fn spanish() -> Self {
            Self::new(
                &[
                    ((0xDE, false, false), Raw::Dead('´')),
                    ((0xDE, true, false), Raw::Dead('¨')),
                    ((0xBA, false, false), Raw::Dead('`')),
                    ((0xBA, true, false), Raw::Dead('^')),
                    ((0x41, false, false), Raw::Char('a')),
                    ((0x41, true, false), Raw::Char('A')),
                    ((0x45, false, false), Raw::Char('e')),
                    ((0x45, true, false), Raw::Char('E')),
                    ((0x45, false, true), Raw::Char('€')),
                    ((0x51, false, false), Raw::Char('q')),
                    ((0x32, false, true), Raw::Char('@')),
                    ((0x20, false, false), Raw::Char(' ')),
                    ((0x08, false, false), Raw::Char('\u{8}')),
                    ((0x0D, false, false), Raw::Char('\r')),
                ],
                &[
                    ('´', 'a', 'á', false),
                    ('´', 'e', 'é', false),
                    ('´', 'E', 'É', false),
                    ('´', ' ', '´', false),
                    ('¨', 'a', 'ä', false),
                    ('¨', 'e', 'ë', false),
                    ('¨', ' ', '¨', false),
                    ('`', 'a', 'à', false),
                    ('`', 'e', 'è', false),
                    ('`', ' ', '`', false),
                    ('^', 'a', 'â', false),
                    ('^', 'e', 'ê', false),
                    ('^', ' ', '^', false),
                ],
            )
        }

        /// What `key` produces by itself.
        pub(crate) fn raw(&self, key: &KeyQuery) -> Option<Raw> {
            let altgr = key.keys.ctrl() && key.keys.alt();
            if key.keys.ctrl() != key.keys.alt() {
                return None;
            }
            self.keys
                .iter()
                .find(|(k, _)| *k == (key.vk, key.keys.shift(), altgr))
                .map(|(_, raw)| *raw)
        }

        fn table(&self) -> &DeadKeyTable {
            self.table.as_ref().expect("the fake layout has a table")
        }

        /// The system's translation of `raw` with `stored` waiting: what it returns, and
        /// what is stored afterwards.
        fn system(&self, raw: Option<Raw>, stored: Option<u16>) -> (Translation, Option<u16>) {
            let (base, is_dead) = match raw {
                None => return (Translation::None, stored),
                Some(Raw::Pair(a, b)) => return (Translation::text(&[unit(a), unit(b)]), stored),
                Some(Raw::Dead(ch)) => (unit(ch), true),
                Some(Raw::Char(ch)) => (unit(ch), false),
            };
            match stored {
                None if is_dead => (Translation::Dead(base), Some(base)),
                None => (Translation::text(&[base]), None),
                Some(dead) => match self.table().compose(dead, base) {
                    Composition::Char(c) => (Translation::text(&[c]), None),
                    Composition::Dead(c) => (Translation::Dead(c), Some(c)),
                    Composition::None | Composition::Unknown => {
                        (Translation::text(&[dead, base]), None)
                    }
                },
            }
        }

        /// The application translates the oldest key of its backlog; returns the units it
        /// receives for it.
        pub(crate) fn app_translates_one(&mut self) -> Option<Vec<u16>> {
            let key = self.backlog.pop_front()?;
            let (translation, stored) = self.system(self.raw(&key), self.stored);
            self.stored = stored;
            Some(match translation {
                Translation::Text(units) => units.as_slice().to_vec(),
                Translation::None | Translation::Dead(_) => Vec::new(),
            })
        }
    }

    impl Layout for FakeLayout {
        fn translate(&mut self, key: &KeyQuery) -> Translation {
            if let Some(left) = self.race_after {
                if left == 0 {
                    self.race_after = None;
                    let _ = self.app_translates_one();
                } else {
                    self.race_after = Some(left - 1);
                }
            }
            self.translations += 1;
            let stored = if self.reads_stored { self.stored } else { None };
            self.system(self.raw(key), stored).0
        }

        fn compose(&mut self, dead: u16, base: u16) -> Composition {
            self.table
                .as_ref()
                .map_or(Composition::Unknown, |table| table.compose(dead, base))
        }

        fn probe(&mut self) -> Option<Probe> {
            self.probe
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{FakeLayout, Raw, held, unit};
    use super::*;

    /// A result that can be printed: the test data is not anybody's typing.
    #[derive(Debug, PartialEq, Eq)]
    enum Got {
        Nothing,
        Held,
        Text(String),
        Unknown,
    }

    fn got(resolved: Resolved) -> Got {
        match resolved {
            Resolved::Nothing => Got::Nothing,
            Resolved::Held => Got::Held,
            Resolved::Text(units) => Got::Text(String::from_utf16_lossy(units.as_slice())),
            Resolved::Unknown => Got::Unknown,
        }
    }

    fn text(s: &str) -> Got {
        Got::Text(s.to_owned())
    }

    fn key(vk: u32) -> KeyQuery {
        KeyQuery {
            vk,
            scan: 0,
            keys: KeyState::default(),
        }
    }

    fn shifted(vk: u32) -> KeyQuery {
        KeyQuery {
            vk,
            scan: 0,
            keys: held(true, false),
        }
    }

    fn altgr(vk: u32) -> KeyQuery {
        KeyQuery {
            vk,
            scan: 0,
            keys: held(false, true),
        }
    }

    const ACUTE: u32 = 0xDE;
    const GRAVE: u32 = 0xBA;
    const A: u32 = 0x41;
    const E: u32 = 0x45;
    const Q: u32 = 0x51;
    const SPACE: u32 = 0x20;
    const BACK: u32 = 0x08;
    const LEFT: u32 = 0x25;

    /// Presses `keys` with an application that translates each key at once.
    fn press(layout: &mut FakeLayout, dead: &mut DeadKeys, keys: &[KeyQuery]) -> Vec<Got> {
        keys.iter()
            .map(|k| {
                let resolved = got(dead.resolve(layout, k));
                layout.backlog.push_back(*k);
                let _ = layout.app_translates_one();
                resolved
            })
            .collect()
    }

    #[test]
    fn a_dead_key_types_nothing_and_the_next_key_composes() {
        for reads_stored in [false, true] {
            let mut layout = FakeLayout::spanish();
            layout.reads_stored = reads_stored;
            let mut dead = DeadKeys::default();
            assert_eq!(
                press(&mut layout, &mut dead, &[key(ACUTE), key(E)]),
                [Got::Held, text("é")]
            );
            assert!(!dead.is_waiting());
            assert_eq!(
                press(&mut layout, &mut dead, &[shifted(GRAVE), key(A), key(A)]),
                [Got::Held, text("â"), text("a")]
            );
            assert_eq!(
                press(&mut layout, &mut dead, &[key(ACUTE), shifted(E)]),
                [Got::Held, text("É")]
            );
        }
    }

    #[test]
    fn a_pair_that_does_not_compose_delivers_both_characters() {
        for reads_stored in [false, true] {
            let mut layout = FakeLayout::spanish();
            layout.reads_stored = reads_stored;
            let mut dead = DeadKeys::default();
            assert_eq!(
                press(&mut layout, &mut dead, &[key(ACUTE), key(Q)]),
                [Got::Held, text("´q")]
            );
            // The dead key twice: two accents, both delivered by the second press.
            assert_eq!(
                press(&mut layout, &mut dead, &[key(ACUTE), key(ACUTE)]),
                [Got::Held, text("´´")]
            );
            // Two different dead keys.
            assert_eq!(
                press(&mut layout, &mut dead, &[key(ACUTE), key(GRAVE), key(E)]),
                [Got::Held, text("´`"), text("e")]
            );
            // An AltGr character after a dead key.
            assert_eq!(
                press(&mut layout, &mut dead, &[key(GRAVE), altgr(E)]),
                [Got::Held, text("`€")]
            );
            assert!(!dead.is_waiting());
        }
    }

    #[test]
    fn space_after_a_dead_key_gives_the_accent_itself() {
        let mut layout = FakeLayout::spanish();
        let mut dead = DeadKeys::default();
        assert_eq!(
            press(&mut layout, &mut dead, &[shifted(ACUTE), key(SPACE)]),
            [Got::Held, text("¨")]
        );
    }

    #[test]
    fn a_control_character_after_a_dead_key_delivers_the_accent_first() {
        // Backspace after ´: the application inserts ´ and deletes it again.
        let mut layout = FakeLayout::spanish();
        let mut dead = DeadKeys::default();
        assert_eq!(
            press(&mut layout, &mut dead, &[key(ACUTE), key(BACK)]),
            [Got::Held, text("´\u{8}")]
        );
    }

    #[test]
    fn a_key_without_a_character_leaves_the_dead_key_waiting() {
        for reads_stored in [false, true] {
            let mut layout = FakeLayout::spanish();
            layout.reads_stored = reads_stored;
            let mut dead = DeadKeys::default();
            assert_eq!(
                press(
                    &mut layout,
                    &mut dead,
                    &[key(ACUTE), key(LEFT), key(LEFT), key(A)]
                ),
                [Got::Held, Got::Nothing, Got::Nothing, text("á")]
            );
        }
    }

    #[test]
    fn a_composition_that_is_dead_again_keeps_waiting() {
        // A layout with a chained dead key: ^ then ¨ waits as ῀, which composes with a.
        let keys = [
            ((0xDD, false, false), Raw::Dead('^')),
            ((0xDC, false, false), Raw::Dead('¨')),
            ((A, false, false), Raw::Char('a')),
            ((Q, false, false), Raw::Char('q')),
        ];
        let table = [
            ('^', '¨', '῀', true),
            ('῀', 'a', 'ẫ', false),
            ('^', 'a', 'â', false),
            ('¨', 'a', 'ä', false),
        ];
        for reads_stored in [false, true] {
            let mut layout = FakeLayout::new(&keys, &table);
            layout.reads_stored = reads_stored;
            let mut dead = DeadKeys::default();
            assert_eq!(
                press(&mut layout, &mut dead, &[key(0xDD), key(0xDC), key(A)]),
                [Got::Held, Got::Held, text("ẫ")]
            );
            assert_eq!(
                press(&mut layout, &mut dead, &[key(0xDD), key(0xDC), key(Q)]),
                [Got::Held, Got::Held, text("῀q")]
            );
            assert!(!dead.is_waiting());
        }
    }

    #[test]
    fn a_ligature_after_a_dead_key_is_not_guessed() {
        let keys = [
            ((ACUTE, false, false), Raw::Dead('´')),
            ((0x4C, false, false), Raw::Pair('l', 'a')),
        ];
        let mut layout = FakeLayout::new(&keys, &[('´', 'a', 'á', false)]);
        let mut dead = DeadKeys::default();
        assert_eq!(got(dead.resolve(&mut layout, &key(0x4C))), text("la"));
        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
        assert_eq!(got(dead.resolve(&mut layout, &key(0x4C))), Got::Unknown);
        assert!(!dead.is_waiting());
    }

    #[test]
    fn without_the_table_a_sequence_is_unknown_and_plain_keys_still_translate() {
        let mut layout = FakeLayout::spanish();
        layout.table = None;
        let mut dead = DeadKeys::default();
        assert_eq!(got(dead.resolve(&mut layout, &key(A))), text("a"));
        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), Got::Unknown);
        assert!(!dead.is_waiting());
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), text("e"));
    }

    #[test]
    fn a_disturbed_sequence_is_unknown_once() {
        let mut layout = FakeLayout::spanish();
        let mut dead = DeadKeys::default();
        dead.disturb();
        assert!(!dead.is_waiting(), "nothing was waiting");
        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
        dead.disturb();
        assert!(dead.is_waiting());
        // A key without a character does not settle it.
        assert_eq!(got(dead.resolve(&mut layout, &key(LEFT))), Got::Nothing);
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), Got::Unknown);
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), text("e"));

        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
        dead.reset();
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), text("e"));
    }

    #[test]
    fn a_character_injected_while_a_dead_key_waits_is_not_guessed() {
        let mut layout = FakeLayout::spanish();
        let mut dead = DeadKeys::default();
        assert!(
            !dead.character_injected(),
            "nothing waits: the character is the answer"
        );
        assert!(!dead.is_waiting());

        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
        assert!(
            dead.character_injected(),
            "a dead key waits: a boundary is due"
        );
        // Whether the application still holds the ´ is not known: neither é nor e is guessed.
        assert!(dead.is_waiting());
        assert!(dead.character_injected(), "still uncertain");
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), Got::Unknown);
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), text("e"));
        assert!(!dead.character_injected());
    }

    #[test]
    fn a_layout_without_dead_keys_is_translated_once_per_key() {
        let mut layout = FakeLayout::new(&[((A, false, false), Raw::Char('a'))], &[]);
        assert!(layout.probe.is_none());
        let mut dead = DeadKeys::default();
        assert_eq!(got(dead.resolve(&mut layout, &key(A))), text("a"));
        assert_eq!(got(dead.resolve(&mut layout, &key(Q))), Got::Nothing);
        assert_eq!(layout.translations, 2);
    }

    #[test]
    fn a_system_that_reads_the_stored_dead_key_is_noticed() {
        let mut layout = FakeLayout::spanish();
        layout.reads_stored = true;
        let mut dead = DeadKeys::default();

        // The application is in step: it has stored the ´ by the time E is pressed, and the
        // system hands the listener the composed character.
        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
        layout.backlog.push_back(key(ACUTE));
        assert_eq!(layout.app_translates_one(), Some(Vec::new()));
        assert_eq!(layout.stored, Some(unit('´')));
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), text("é"));
        assert_eq!(
            layout.stored,
            Some(unit('´')),
            "the listener changed nothing"
        );
        layout.backlog.push_back(key(E));
        assert_eq!(layout.app_translates_one(), Some(vec![unit('é')]));

        // The application lags: it has translated neither key when E is pressed.
        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), text("é"));
        // A third key arrives when the application has stored the ´ but not used it: what
        // the system returns for it now is not what the application will receive.
        layout.backlog.extend([key(ACUTE), key(E), key(A)]);
        let _ = layout.app_translates_one();
        assert_eq!(got(dead.resolve(&mut layout, &key(A))), Got::Unknown);
        assert!(
            dead.is_waiting(),
            "and neither whether a dead key waits now"
        );
        // Once the application has caught up, the first key that is not a dead key settles
        // it, and keys translate again.
        assert_eq!(layout.app_translates_one(), Some(vec![unit('é')]));
        assert_eq!(got(dead.resolve(&mut layout, &key(A))), Got::Unknown);
        assert!(!dead.is_waiting());
        assert_eq!(got(dead.resolve(&mut layout, &key(A))), text("a"));
    }

    #[test]
    fn a_stored_dead_key_the_listener_never_saw_is_not_guessed() {
        // The dead key was pressed before the listener started.
        let mut layout = FakeLayout::spanish();
        layout.reads_stored = true;
        layout.stored = Some(unit('`'));
        let mut dead = DeadKeys::default();
        assert_eq!(got(dead.resolve(&mut layout, &key(LEFT))), Got::Nothing);
        assert!(!dead.is_waiting());
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), Got::Unknown);
        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Unknown);
        assert!(
            dead.is_waiting(),
            "in doubt until the system stops composing"
        );
        // The application uses the stored dead key up.
        layout.stored = None;
        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Unknown);
        assert!(dead.is_waiting(), "a dead key does not settle the doubt");
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), Got::Unknown);
        assert!(!dead.is_waiting());
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), text("e"));
    }

    #[test]
    fn a_stored_dead_key_that_is_not_the_listeners_is_not_trusted() {
        // The application lags two dead keys behind: ` e ´ were typed, it has translated the
        // ` only. The system would compose the next key with `, the application will
        // compose it with ´.
        let mut layout = FakeLayout::spanish();
        layout.reads_stored = true;
        let mut dead = DeadKeys::default();
        assert_eq!(got(dead.resolve(&mut layout, &key(GRAVE))), Got::Held);
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), text("è"));
        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
        layout.backlog.extend([key(GRAVE), key(E), key(ACUTE)]);
        let _ = layout.app_translates_one();
        assert_eq!(layout.stored, Some(unit('`')));
        assert_eq!(got(dead.resolve(&mut layout, &key(A))), Got::Unknown);

        // The same dead key twice is no trouble: the result does not depend on which of the
        // two presses the system has stored.
        let mut layout = FakeLayout::spanish();
        layout.reads_stored = true;
        let mut dead = DeadKeys::default();
        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), text("é"));
        assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
        layout.backlog.extend([key(ACUTE), key(E), key(ACUTE)]);
        let _ = layout.app_translates_one();
        assert_eq!(got(dead.resolve(&mut layout, &key(A))), text("á"));
    }

    #[test]
    fn a_change_of_the_stored_dead_key_during_a_translation_is_retried() {
        for race_after in 0..3 {
            let mut layout = FakeLayout::spanish();
            layout.reads_stored = true;
            let mut dead = DeadKeys::default();
            assert_eq!(got(dead.resolve(&mut layout, &key(ACUTE))), Got::Held);
            // The application stores the ´ while the listener is translating the E.
            layout.backlog.push_back(key(ACUTE));
            layout.race_after = Some(race_after);
            assert_eq!(
                got(dead.resolve(&mut layout, &key(E))),
                text("é"),
                "race after {race_after} translations"
            );
        }
    }

    #[test]
    fn a_state_that_never_settles_is_unknown() {
        /// A layout whose probe never translates the same way twice.
        struct Restless(u16);
        impl Layout for Restless {
            fn translate(&mut self, _key: &KeyQuery) -> Translation {
                self.0 += 1;
                Translation::Dead(self.0)
            }
            fn compose(&mut self, _dead: u16, _base: u16) -> Composition {
                Composition::None
            }
            fn probe(&mut self) -> Option<Probe> {
                Some(Probe {
                    key: key(ACUTE),
                    ch: 1,
                })
            }
        }
        let mut dead = DeadKeys::default();
        let mut layout = Restless(0);
        assert_eq!(got(dead.resolve(&mut layout, &key(E))), Got::Unknown);
        assert_eq!(layout.0, 9, "three attempts of three translations");
        assert!(dead.is_waiting(), "in doubt");
    }

    /// The reference: what the application receives for each key when it translates them in
    /// order with the system's stateful translation.
    fn application_receives(keys: &[KeyQuery]) -> Vec<Vec<u16>> {
        let mut layout = FakeLayout::spanish();
        keys.iter()
            .map(|k| {
                layout.backlog.push_back(*k);
                layout.app_translates_one().expect("one key in the backlog")
            })
            .collect()
    }

    /// Random typing against an application that lags by a random number of keys, for both
    /// behaviours of the stateless translation: whenever the listener reports characters,
    /// they are the ones the application receives for that key.
    #[test]
    fn whatever_the_listener_reports_is_what_the_application_receives() {
        let pool = [
            key(ACUTE),
            shifted(ACUTE),
            key(GRAVE),
            shifted(GRAVE),
            key(A),
            shifted(A),
            key(E),
            shifted(E),
            altgr(E),
            key(Q),
            key(SPACE),
            key(BACK),
            key(LEFT),
            altgr(0x32),
        ];
        let mut seed: u64 = 0x1A3B_5C7D_9E0F_1234;
        let mut next = move |bound: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(seed >> 33).expect("31 bits fit") % bound
        };
        for reads_stored in [false, true] {
            for lagging in [false, true] {
                let mut unknown = 0_usize;
                let mut reported = 0_usize;
                for _ in 0..400 {
                    let keys: Vec<KeyQuery> = (0..12).map(|_| pool[next(pool.len())]).collect();
                    let reference = application_receives(&keys);
                    let mut layout = FakeLayout::spanish();
                    layout.reads_stored = reads_stored;
                    let mut dead = DeadKeys::default();
                    for (i, k) in keys.iter().enumerate() {
                        if lagging {
                            // The application catches up by a random amount, sometimes in
                            // the middle of the listener's translation.
                            for _ in 0..next(3) {
                                let _ = layout.app_translates_one();
                            }
                            if next(4) == 0 {
                                layout.race_after = Some(next(3));
                            }
                        }
                        let resolved = dead.resolve(&mut layout, k);
                        layout.race_after = None;
                        layout.backlog.push_back(*k);
                        if !lagging {
                            let _ = layout.app_translates_one();
                        }
                        match resolved {
                            Resolved::Unknown => unknown += 1,
                            Resolved::Nothing | Resolved::Held => assert!(
                                reference[i].is_empty(),
                                "key {i}: the application received characters"
                            ),
                            Resolved::Text(units) => {
                                reported += 1;
                                assert_eq!(
                                    units.as_slice(),
                                    reference[i].as_slice(),
                                    "key {i} (reads stored: {reads_stored}, lagging: {lagging})"
                                );
                            }
                        }
                    }
                }
                assert!(reported > 1_000, "the run reported characters");
                if !(reads_stored && lagging) {
                    assert_eq!(
                        unknown, 0,
                        "nothing is unknown (reads stored: {reads_stored}, lagging: {lagging})"
                    );
                }
            }
        }
    }

    #[test]
    fn the_table_keeps_the_first_entry_of_a_pair() {
        let table = DeadKeyTable::new([
            (1, 2, 30, false),
            (1, 2, 31, true),
            (5, 6, 7, true),
            (1, 1, 9, false),
        ]);
        assert_eq!(table.len(), 3);
        assert_eq!(table.compose(1, 2), Composition::Char(30));
        assert_eq!(table.compose(5, 6), Composition::Dead(7));
        assert_eq!(table.compose(1, 1), Composition::Char(9));
        assert_eq!(table.compose(2, 1), Composition::None);
        assert!(table.is_dead(1) && table.is_dead(5) && table.is_dead(7));
        assert!(!table.is_dead(2) && !table.is_dead(30));
        assert_eq!(format!("{table:?}"), "DeadKeyTable { pairs: 3 }");
        assert_eq!(DeadKeyTable::default().compose(1, 2), Composition::None);
    }

    #[test]
    fn find_probe_takes_the_first_confirmed_dead_key() {
        let layout = FakeLayout::spanish();
        let table = layout.table.clone().expect("a table");
        let mut calls = 0;
        let probe = find_probe(
            |k| {
                calls += 1;
                match layout.raw(k) {
                    Some(Raw::Dead(ch)) => Translation::Dead(unit(ch)),
                    Some(Raw::Char(ch)) => Translation::text(&[unit(ch)]),
                    Some(Raw::Pair(a, b)) => Translation::text(&[unit(a), unit(b)]),
                    None => Translation::None,
                }
            },
            |ch| table.is_dead(ch),
        )
        .expect("the layout has dead keys");
        // 0xBA comes before 0xDE.
        assert_eq!(probe.key.vk, GRAVE);
        assert_eq!(probe.ch, unit('`'));
        assert!(calls < 0x100);

        // A dead character the table does not list is not trusted, and neither is one that
        // does not repeat.
        assert_eq!(find_probe(|_| Translation::Dead(7), |_| false), None);
        let mut flip = 0_u16;
        assert_eq!(
            find_probe(
                |_| {
                    flip += 1;
                    Translation::Dead(flip)
                },
                |_| true
            ),
            None
        );
        assert_eq!(find_probe(|_| Translation::None, |_| true), None);
    }

    #[test]
    fn a_layout_with_dead_keys_composes_only_once_its_probe_is_found() {
        let layout = FakeLayout::spanish();
        let table = layout.table.clone().expect("a table");
        let clean = |k: &KeyQuery| match layout.raw(k) {
            Some(Raw::Dead(ch)) => Translation::Dead(unit(ch)),
            Some(Raw::Char(ch)) => Translation::text(&[unit(ch)]),
            Some(Raw::Pair(a, b)) => Translation::text(&[unit(a), unit(b)]),
            None => Translation::None,
        };
        // While the system holds a dead key, no key translates as a plain dead key.
        let holding = |k: &KeyQuery| match clean(k) {
            Translation::Dead(ch) => Translation::text(&[unit('´'), ch]),
            other => other,
        };
        let composed = Composition::Char(unit('é'));

        let known = LayoutDeadKeys::new(Some(table.clone()), clean);
        assert!(known.probe().is_some() && !known.wants_search());
        assert_eq!(known.compose(unit('´'), unit('e')), composed);
        assert_eq!(known.compose(unit('´'), unit('q')), Composition::None);

        // Read at a moment when the system held a dead key: nothing is composed until a
        // later search finds the probe.
        let mut known = LayoutDeadKeys::new(Some(table.clone()), holding);
        assert!(known.probe().is_none() && known.wants_search());
        assert_eq!(known.compose(unit('´'), unit('e')), Composition::Unknown);
        known.search(holding);
        assert!(known.wants_search());
        known.search(clean);
        assert!(known.probe().is_some() && !known.wants_search());
        assert_eq!(known.compose(unit('´'), unit('e')), composed);

        // The search is not repeated for ever.
        let mut known = LayoutDeadKeys::new(Some(table), holding);
        known.search(holding);
        known.search(holding);
        assert!(!known.wants_search());
        known.search(clean);
        assert!(known.probe().is_none(), "given up");
        assert_eq!(known.compose(unit('´'), unit('e')), Composition::Unknown);
    }

    #[test]
    fn a_layout_without_dead_keys_or_without_its_file_needs_no_probe() {
        let mut searched = 0;
        let mut count = |_: &KeyQuery| {
            searched += 1;
            Translation::None
        };
        let none = LayoutDeadKeys::new(Some(DeadKeyTable::default()), &mut count);
        assert!(none.probe().is_none() && !none.wants_search());
        assert_eq!(none.compose(1, 2), Composition::None);
        assert!(none.table().is_some_and(DeadKeyTable::is_empty));

        let unread = LayoutDeadKeys::new(None, &mut count);
        assert!(unread.probe().is_none() && !unread.wants_search());
        assert_eq!(unread.compose(1, 2), Composition::Unknown);
        assert!(unread.table().is_none());
        assert_eq!(searched, 0, "nothing was translated");
    }

    #[test]
    fn debug_never_shows_a_character() {
        let mut dead = DeadKeys::default();
        let mut layout = FakeLayout::spanish();
        let _ = dead.resolve(&mut layout, &key(ACUTE));
        assert_eq!(format!("{dead:?}"), "DeadKeys { waiting: true, .. }");
        assert_eq!(format!("{:?}", Translation::Dead(0xB4)), "Dead(..)");
        assert_eq!(format!("{:?}", Translation::text(&[0x61])), "Text(..)");
        assert_eq!(format!("{:?}", Translation::None), "None");
        assert_eq!(
            format!("{:?}", Units::new(&[0x61, 0x62])),
            "Units { len: 2, .. }"
        );
        assert_eq!(format!("{:?}", Composition::Char(0xE9)), "Char(..)");
        assert_eq!(format!("{:?}", Composition::Dead(0xB4)), "Dead(..)");
        assert_eq!(
            format!("{:?}", Resolved::Text(Units::new(&[0x61]))),
            "Text(..)"
        );
        assert_eq!(format!("{:?}", Resolved::Held), "Held");
        assert_eq!(format!("{:?}", Resolved::Nothing), "Nothing");
        assert_eq!(format!("{:?}", Resolved::Unknown), "Unknown");
        assert_eq!(format!("{:?}", key(0x41)), "KeyQuery { .. }");
        assert_eq!(Units::new(&[1; 12]).as_slice().len(), MAX_UNITS);
    }
}
