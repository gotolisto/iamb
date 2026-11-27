//! Global hotkeys (architecture §4.5, §5): the search palette's shortcut and its like.
//!
//! A hotkey uses the OS hotkey API, never the keyboard hook, so the listener stays
//! observe-only (ADR-0010). The press arrives on the listener thread and is delivered through
//! the listener's sink as [`ListenerEvent::Hotkey`](crate::ListenerEvent::Hotkey), in its place
//! in the key stream.

use std::fmt;

use iamb_core::template::KeyName;

use crate::error::Result;

/// The service's name for one registered hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HotkeyId(pub u32);

/// The modifier keys of a chord.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    /// No modifier.
    pub const NONE: Self = Self(0);
    /// Control.
    pub const CTRL: Self = Self(1);
    /// Alt on Windows, Option on macOS.
    pub const ALT: Self = Self(2);
    /// Shift.
    pub const SHIFT: Self = Self(4);
    /// The Windows key, or Command on macOS.
    pub const META: Self = Self(8);

    const NAMES: [(Self, &'static str); 4] = [
        (Self::CTRL, "Ctrl"),
        (Self::ALT, "Alt"),
        (Self::SHIFT, "Shift"),
        (Self::META, "Meta"),
    ];

    /// Both sets together.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// True when every modifier of `other` is in this set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// True for the empty set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Debug for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = Self::NAMES
            .iter()
            .filter(|(m, _)| self.contains(*m))
            .map(|(_, name)| *name)
            .collect();
        write!(f, "Modifiers({})", names.join("+"))
    }
}

/// The key of a chord, independent of the keyboard layout's shifted symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyKey {
    /// A letter key, `A` to `Z` (stored upper case).
    Letter(char),
    /// A digit key of the main row, 0 to 9.
    Digit(u8),
    /// A function key, 1 to 24.
    Function(u8),
    /// A named key (Space, Enter, the arrows, …).
    Named(KeyName),
}

/// A modifier set and a key, e.g. `Ctrl+Shift+Space`.
///
/// A valid chord cannot be typed by accident: a letter, digit or named key needs Ctrl, Alt or
/// Meta (Shift alone would swallow ordinary typing); a function key may stand alone. The system
/// hotkey API takes a registered chord away from every application, so these are refused too
/// (the beta-gap review's finding 4), by the service and not only by the recorder:
///
/// - the clipboard and undo chords, Ctrl or Meta alone with C, V, X, Z, A or Y, and the
///   word and line deletions, Ctrl or Meta alone with Backspace or Delete
///   ([`ChordError::Reserved`]): Iamb's own paste sends one of them, and they are everyday
///   editing;
/// - chords that type a character ([`ChordError::TypesCharacter`]): Alt (Option), with or
///   without Shift, and a letter or digit, which types a character on macOS and opens a menu on
///   Windows; and on Windows Ctrl+Alt, with or without Shift, and a letter or digit unless Meta
///   is held too, because `AltGr` is Ctrl+Alt there (`AltGr`+Q is `@` on a German layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    modifiers: Modifiers,
    key: HotkeyKey,
}

/// Why a chord is not acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChordError {
    /// No key was named.
    NoKey,
    /// More than one key was named.
    TwoKeys,
    /// A part is neither a modifier nor a key.
    UnknownKey,
    /// A modifier appears twice.
    DuplicateModifier,
    /// The key needs Ctrl, Alt or Meta.
    NeedsModifier,
    /// The key is outside its range (a letter that is not `A`–`Z`, a digit above 9, a function
    /// key outside 1–24).
    OutOfRange,
    /// A clipboard, undo or deletion chord (see [`Chord`]).
    Reserved,
    /// The chord types a character on this system (see [`Chord`]).
    TypesCharacter,
}

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoKey => "the shortcut names no key",
            Self::TwoKeys => "the shortcut names more than one key",
            Self::UnknownKey => "the shortcut names an unknown key",
            Self::DuplicateModifier => "the shortcut names a modifier twice",
            Self::NeedsModifier => "the shortcut needs Ctrl, Alt or Meta",
            Self::OutOfRange => "the shortcut's key is out of range",
            Self::Reserved => "the shortcut is an editing shortcut every application uses",
            Self::TypesCharacter => "the shortcut types a character on this keyboard",
        })
    }
}

impl std::error::Error for ChordError {}

impl Chord {
    /// A chord of `modifiers` and `key`.
    ///
    /// # Errors
    ///
    /// [`ChordError::OutOfRange`], [`ChordError::NeedsModifier`], [`ChordError::Reserved`] or
    /// [`ChordError::TypesCharacter`].
    pub fn new(modifiers: Modifiers, key: HotkeyKey) -> std::result::Result<Self, ChordError> {
        let key = match key {
            HotkeyKey::Letter(c) if c.is_ascii_alphabetic() => {
                HotkeyKey::Letter(c.to_ascii_uppercase())
            }
            HotkeyKey::Digit(d) if d <= 9 => key,
            HotkeyKey::Function(n) if (1..=24).contains(&n) => key,
            HotkeyKey::Named(_) => key,
            _ => return Err(ChordError::OutOfRange),
        };
        let strong = modifiers.contains(Modifiers::CTRL)
            || modifiers.contains(Modifiers::ALT)
            || modifiers.contains(Modifiers::META);
        if !strong && !matches!(key, HotkeyKey::Function(_)) {
            return Err(ChordError::NeedsModifier);
        }
        if let Some(e) = conflict(modifiers, key, cfg!(windows)) {
            return Err(e);
        }
        Ok(Self { modifiers, key })
    }

    /// Parses the form stored in settings: parts joined by `+`, modifiers first or last, in
    /// any letter case. Modifiers: `Ctrl`/`Control`, `Alt`/`Option`, `Shift`,
    /// `Meta`/`Cmd`/`Command`/`Win`/`Super`. Keys: a letter, a digit, `F1`–`F24`, or a key name
    /// of the template language (`space`, `enter`, `pageup`, …).
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn parse(text: &str) -> std::result::Result<Self, ChordError> {
        let mut modifiers = Modifiers::NONE;
        let mut key = None;
        for part in text.split('+') {
            let part = part.trim().to_ascii_lowercase();
            let modifier = match part.as_str() {
                "ctrl" | "control" => Some(Modifiers::CTRL),
                "alt" | "option" => Some(Modifiers::ALT),
                "shift" => Some(Modifiers::SHIFT),
                "meta" | "cmd" | "command" | "win" | "super" => Some(Modifiers::META),
                _ => None,
            };
            if let Some(m) = modifier {
                if modifiers.contains(m) {
                    return Err(ChordError::DuplicateModifier);
                }
                modifiers = modifiers.with(m);
            } else if key.is_some() {
                return Err(ChordError::TwoKeys);
            } else {
                key = Some(parse_key(&part)?);
            }
        }
        Self::new(modifiers, key.ok_or(ChordError::NoKey)?)
    }

    /// The modifiers.
    #[must_use]
    pub const fn modifiers(self) -> Modifiers {
        self.modifiers
    }

    /// The key.
    #[must_use]
    pub const fn key(self) -> HotkeyKey {
        self.key
    }
}

/// Why `modifiers` + `key` would take an editing chord or a character away from every
/// application (see [`Chord`]); `windows` for the Windows rule.
fn conflict(modifiers: Modifiers, key: HotkeyKey, windows: bool) -> Option<ChordError> {
    let only = |m: Modifiers| modifiers == m;
    let text_key = matches!(key, HotkeyKey::Letter(_) | HotkeyKey::Digit(_));
    let editing = matches!(
        key,
        HotkeyKey::Letter('C' | 'V' | 'X' | 'Z' | 'A' | 'Y')
            | HotkeyKey::Named(KeyName::Backspace | KeyName::Delete)
    );
    if (only(Modifiers::CTRL) || only(Modifiers::META)) && editing {
        return Some(ChordError::Reserved);
    }
    let alt_shift = Modifiers::ALT.with(Modifiers::SHIFT);
    if text_key && (only(Modifiers::ALT) || only(alt_shift)) {
        return Some(ChordError::TypesCharacter);
    }
    let ctrl_alt = Modifiers::CTRL.with(Modifiers::ALT);
    if windows && text_key && modifiers.contains(ctrl_alt) && !modifiers.contains(Modifiers::META) {
        return Some(ChordError::TypesCharacter);
    }
    None
}

fn parse_key(part: &str) -> std::result::Result<HotkeyKey, ChordError> {
    let mut chars = part.chars();
    match (chars.next(), chars.next()) {
        (None, _) => Err(ChordError::NoKey),
        (Some(c), None) if c.is_ascii_alphabetic() => Ok(HotkeyKey::Letter(c.to_ascii_uppercase())),
        (Some(c), None) => c
            .to_digit(10)
            .and_then(|d| u8::try_from(d).ok())
            .map(HotkeyKey::Digit)
            .ok_or(ChordError::UnknownKey),
        (Some('f'), Some(d)) if d.is_ascii_digit() => part[1..]
            .parse()
            .map(HotkeyKey::Function)
            .map_err(|_| ChordError::OutOfRange),
        _ => KeyName::from_name(part)
            .map(HotkeyKey::Named)
            .ok_or(ChordError::UnknownKey),
    }
}

/// The canonical form: `Ctrl+Alt+Shift+Meta+Key`, which [`Chord::parse`] reads back.
impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (modifier, name) in Modifiers::NAMES {
            if self.modifiers.contains(modifier) {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            HotkeyKey::Letter(c) => write!(f, "{c}"),
            HotkeyKey::Digit(d) => write!(f, "{d}"),
            HotkeyKey::Function(n) => write!(f, "F{n}"),
            HotkeyKey::Named(key) => {
                let name = key.as_str();
                let mut chars = name.chars();
                match chars.next() {
                    Some(first) => write!(f, "{}{}", first.to_ascii_uppercase(), chars.as_str()),
                    None => Ok(()),
                }
            }
        }
    }
}

/// Registers global hotkeys with the OS hotkey API.
///
/// Registrations are served by the listener's thread: they may be made before
/// [`KeyListener::start`](crate::KeyListener::start), take effect while the listener runs, and
/// are applied again when it restarts.
pub trait Hotkeys: Send + Sync {
    /// Registers `chord` under `id`. Registering an id again replaces its chord.
    ///
    /// # Errors
    ///
    /// [`HotkeyUnavailable`](crate::PlatformError::HotkeyUnavailable) when another application
    /// (or another id) holds the chord; [`Invalid`](crate::PlatformError::Invalid) when the OS
    /// cannot express it.
    fn register(&self, id: HotkeyId, chord: Chord) -> Result<()>;

    /// Removes the registration of `id`. Unregistering an unknown id is not an error.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the OS refuses.
    fn unregister(&self, id: HotkeyId) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> Chord {
        Chord::parse(text).expect("valid chord")
    }

    #[test]
    fn parses_and_prints_the_canonical_form() {
        for (input, canonical) in [
            ("Ctrl+Shift+Space", "Ctrl+Shift+Space"),
            ("shift + CTRL + space", "Ctrl+Shift+Space"),
            ("cmd+k", "Meta+K"),
            ("Win+Alt+7", "Alt+Meta+7"),
            ("option+command+pageup", "Alt+Meta+Pageup"),
            ("F5", "F5"),
            ("shift+f12", "Shift+F12"),
            ("control+super+f24", "Ctrl+Meta+F24"),
            ("Alt+Enter", "Alt+Enter"),
        ] {
            let parsed = chord(input);
            assert_eq!(parsed.to_string(), canonical, "{input}");
            assert_eq!(chord(canonical), parsed, "round trip of {canonical}");
        }
    }

    #[test]
    fn accessors() {
        let c = chord("Ctrl+Alt+Meta+k");
        assert_eq!(c.key(), HotkeyKey::Letter('K'));
        assert!(c.modifiers().contains(Modifiers::CTRL.with(Modifiers::ALT)));
        assert!(!c.modifiers().contains(Modifiers::SHIFT));
        assert!(!c.modifiers().is_empty());
        assert!(Modifiers::NONE.is_empty());
        assert_eq!(Modifiers::default(), Modifiers::NONE);
        assert_eq!(format!("{:?}", c.modifiers()), "Modifiers(Ctrl+Alt+Meta)");
        assert_eq!(format!("{:?}", Modifiers::NONE), "Modifiers()");
        assert_eq!(chord("meta+space").key(), HotkeyKey::Named(KeyName::Space));
        assert_eq!(chord("ctrl+0").key(), HotkeyKey::Digit(0));
        assert_eq!(chord("f1").key(), HotkeyKey::Function(1));
    }

    #[test]
    fn rejects_bad_chords() {
        for (input, error) in [
            ("", ChordError::NoKey),
            ("Ctrl+", ChordError::NoKey),
            ("Ctrl+Shift", ChordError::NoKey),
            ("Ctrl+A+B", ChordError::TwoKeys),
            ("Ctrl+Ctrl+A", ChordError::DuplicateModifier),
            ("control+ctrl+A", ChordError::DuplicateModifier),
            ("Ctrl+Hyper", ChordError::UnknownKey),
            ("Ctrl+é", ChordError::UnknownKey),
            ("Ctrl+-", ChordError::UnknownKey),
            ("A", ChordError::NeedsModifier),
            ("Shift+A", ChordError::NeedsModifier),
            ("Shift+Space", ChordError::NeedsModifier),
            ("F0", ChordError::OutOfRange),
            ("F25", ChordError::OutOfRange),
            ("F999", ChordError::OutOfRange),
        ] {
            assert_eq!(Chord::parse(input), Err(error), "{input:?}");
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn new_validates_ranges() {
        let ctrl = Modifiers::CTRL;
        assert_eq!(
            Chord::new(ctrl, HotkeyKey::Letter('q')).map(Chord::key),
            Ok(HotkeyKey::Letter('Q'))
        );
        assert_eq!(
            Chord::new(ctrl, HotkeyKey::Letter('1')),
            Err(ChordError::OutOfRange)
        );
        assert_eq!(
            Chord::new(ctrl, HotkeyKey::Digit(10)),
            Err(ChordError::OutOfRange)
        );
        assert_eq!(
            Chord::new(ctrl, HotkeyKey::Function(0)),
            Err(ChordError::OutOfRange)
        );
        assert!(Chord::new(Modifiers::NONE, HotkeyKey::Function(24)).is_ok());
        assert_eq!(
            Chord::new(Modifiers::SHIFT, HotkeyKey::Named(KeyName::Enter)),
            Err(ChordError::NeedsModifier)
        );
    }

    #[test]
    fn editing_chords_and_chords_that_type_are_refused() {
        for input in [
            "Ctrl+V",
            "Meta+V",
            "Ctrl+C",
            "Cmd+C",
            "Ctrl+X",
            "Ctrl+Z",
            "Meta+Z",
            "Ctrl+A",
            "Ctrl+Y",
            "Ctrl+Backspace",
            "Meta+Backspace",
            "Ctrl+Delete",
        ] {
            assert_eq!(Chord::parse(input), Err(ChordError::Reserved), "{input}");
        }
        for input in ["Alt+Q", "Option+E", "Alt+Shift+2", "Alt+7"] {
            assert_eq!(
                Chord::parse(input),
                Err(ChordError::TypesCharacter),
                "{input}"
            );
        }
        // With another modifier they are ordinary chords.
        for input in [
            "Ctrl+Shift+V",
            "Meta+Shift+Z",
            "Ctrl+Alt+Meta+V",
            "Alt+Meta+Q",
            "Alt+F4",
            "Alt+Space",
            "Ctrl+Shift+Backspace",
            "Ctrl+B",
        ] {
            assert!(Chord::parse(input).is_ok(), "{input}");
        }
        assert!(!ChordError::Reserved.to_string().is_empty());
        assert!(!ChordError::TypesCharacter.to_string().is_empty());
    }

    #[test]
    fn altgr_combinations_are_refused_on_windows_only() {
        let key = |c| HotkeyKey::Letter(c);
        let ctrl_alt = Modifiers::CTRL.with(Modifiers::ALT);
        let with_shift = ctrl_alt.with(Modifiers::SHIFT);
        for (modifiers, k) in [
            (ctrl_alt, key('Q')),
            (ctrl_alt, key('A')),
            (with_shift, key('A')),
            (ctrl_alt, HotkeyKey::Digit(2)),
        ] {
            assert_eq!(
                conflict(modifiers, k, true),
                Some(ChordError::TypesCharacter),
                "{modifiers:?} {k:?}"
            );
            assert_eq!(conflict(modifiers, k, false), None, "{modifiers:?} {k:?}");
        }
        // Meta with them, or a key that types nothing, is fine on Windows too.
        assert_eq!(
            conflict(ctrl_alt.with(Modifiers::META), key('Q'), true),
            None
        );
        assert_eq!(conflict(ctrl_alt, HotkeyKey::Function(5), true), None);
        assert_eq!(
            conflict(ctrl_alt, HotkeyKey::Named(KeyName::Space), true),
            None
        );
        assert_eq!(
            conflict(Modifiers::CTRL.with(Modifiers::SHIFT), key('Q'), true),
            None
        );
    }
}
