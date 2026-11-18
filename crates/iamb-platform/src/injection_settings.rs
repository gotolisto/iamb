//! The user's per-application injection settings: one format on both systems (ADR-0025).
//!
//! The settings are one JSON string in the device-local `setting` table under
//! [`INJECTION_SETTINGS_KEY`]. Settings never sync (architecture §7), so a device only holds
//! settings for its own system. The service reads the string at start and whenever it changes
//! and hands it to [`Injector::configure`](crate::Injector::configure); the OS crates read it
//! with [`InjectionSettings::parse`], so both accept the same document and refuse the same
//! mistakes. Each injector layers the result over its built-in strategy table.
//!
//! # Format
//!
//! ```json
//! {
//!   "defaults": { "paste_threshold": 400 },
//!   "apps": {
//!     "myeditor.exe": { "method": "keys", "line_break": "shift+enter" },
//!     "com.example.Chat": { "method": "paste", "line_break": "shift+enter" }
//!   }
//! }
//! ```
//!
//! Both members are optional. `defaults` applies to every application; an entry under `apps`
//! applies to one application and changes only the fields it names. The application id is the
//! executable name on Windows (compared without case, with or without `.exe`, without a
//! directory) and the bundle identifier on macOS (compared without ASCII case). An empty
//! string, or one of only white space, means no settings.
//!
//! Shared fields, with the same names and values on both systems:
//!
//! | Field | Values | Windows | macOS |
//! | --- | --- | --- | --- |
//! | `method` | `auto`: key events, a paste above `paste_threshold`; `keys`: always key events; `paste`: always a paste | | |
//! | `paste_threshold` | characters of one text step above which `auto` pastes | 0 to 1,000,000 | 0 to 1,000,000 |
//! | `line_break` | the key a line break in typed text presses: `enter`, `shift+enter` | | also `option+enter`, `control+option+enter`, and `paste` (text with a line break is pasted whole) |
//! | `key_delay_ms` | pause between synthetic key presses | 0 to 10,000 | 0 to 1,000 |
//! | `paste_wait_ms` | how long the application gets to read a paste before the clipboard is restored | 0 to 10,000 | 20 to 5,000 |
//!
//! Fields of one system only:
//!
//! | Field | System | Values |
//! | --- | --- | --- |
//! | `paste_keys` | Windows | the paste shortcut: `ctrl+v` or `shift+insert` |
//! | `batch_delay_ms` | Windows | pause between input batches, 0 to 10,000 |
//! | `chunk_units` | macOS | the most UTF-16 code units in one key event, 1 to 20 |
//!
//! At most [`MAX_APPS`] application entries, and at most 64 KiB of text.
//!
//! # Errors
//!
//! Malformed JSON, an unknown member or field, a field or `line_break` value of the other
//! system, a value of the wrong type and a value out of range are all errors: a typo is
//! reported, not ignored. [`SettingsError`] names what was wrong and never quotes the
//! settings. An injector that refuses a document keeps the settings it had.

mod json;

use std::fmt;

use crate::error::PlatformError;
use json::Value;

/// The key of the setting that holds the injection settings.
pub const INJECTION_SETTINGS_KEY: &str = "injection_overrides";

/// The most application entries one document may hold.
pub const MAX_APPS: usize = 1000;

/// Which system's variant of the format a document is read as: the shared fields and that
/// system's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsDialect {
    /// Windows: application ids are executable names; adds `paste_keys` and `batch_delay_ms`.
    Windows,
    /// macOS: application ids are bundle identifiers; adds `chunk_units` and three more
    /// `line_break` values.
    MacOs,
}

/// The smallest and largest value a numeric field accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsBounds {
    /// The smallest value accepted.
    pub min: u32,
    /// The largest value accepted.
    pub max: u32,
}

impl SettingsBounds {
    const fn new(min: u32, max: u32) -> Self {
        Self { min, max }
    }
}

impl SettingsDialect {
    /// `paste_threshold`.
    #[must_use]
    pub const fn paste_threshold(self) -> SettingsBounds {
        SettingsBounds::new(0, 1_000_000)
    }

    /// `key_delay_ms`.
    #[must_use]
    pub const fn key_delay_ms(self) -> SettingsBounds {
        match self {
            Self::Windows => SettingsBounds::new(0, 10_000),
            Self::MacOs => SettingsBounds::new(0, 1_000),
        }
    }

    /// `paste_wait_ms`. On macOS a paste needs the content on the pasteboard long enough for
    /// the application to read it, hence the minimum.
    #[must_use]
    pub const fn paste_wait_ms(self) -> SettingsBounds {
        match self {
            Self::Windows => SettingsBounds::new(0, 10_000),
            Self::MacOs => SettingsBounds::new(20, 5_000),
        }
    }

    /// `batch_delay_ms` (Windows).
    #[must_use]
    pub const fn batch_delay_ms(self) -> SettingsBounds {
        SettingsBounds::new(0, 10_000)
    }

    /// `chunk_units` (macOS).
    #[must_use]
    pub const fn chunk_units(self) -> SettingsBounds {
        SettingsBounds::new(1, 20)
    }

    /// The form in which this system compares application ids, for the keys of
    /// [`InjectionSettings::apps`] and for looking an application up. Windows: the file name
    /// only, trimmed, lower case, without `.exe`. macOS: ASCII lower case.
    #[must_use]
    pub fn app_key(self, app: &str) -> String {
        match self {
            Self::Windows => {
                let file = app.rsplit(['\\', '/']).next().unwrap_or(app).trim();
                let lower = file.to_lowercase();
                match lower.strip_suffix(".exe") {
                    Some(stem) => stem.to_owned(),
                    None => lower,
                }
            }
            Self::MacOs => app.to_ascii_lowercase(),
        }
    }
}

/// `method`: how text is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InjectionMethod {
    /// Key events, or a paste for text longer than `paste_threshold`.
    Auto,
    /// Always key events.
    Keys,
    /// Always a paste.
    Paste,
}

/// `line_break`: how a line break in typed text is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineBreakChoice {
    /// `enter`.
    Enter,
    /// `shift+enter`: for applications where Enter sends the message.
    ShiftEnter,
    /// `option+enter` (macOS).
    OptionEnter,
    /// `control+option+enter` (macOS): a line break inside a spreadsheet cell.
    ControlOptionEnter,
    /// `paste` (macOS): text with a line break is pasted whole.
    Paste,
}

/// `paste_keys` (Windows): the paste shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PasteShortcut {
    /// `ctrl+v`.
    CtrlV,
    /// `shift+insert`.
    ShiftInsert,
}

/// One entry: `defaults` or the entry of one application. `None` leaves the field to the
/// layer below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InjectionEntry {
    /// `method`.
    pub method: Option<InjectionMethod>,
    /// `paste_threshold`.
    pub paste_threshold: Option<usize>,
    /// `line_break`.
    pub line_break: Option<LineBreakChoice>,
    /// `key_delay_ms`.
    pub key_delay_ms: Option<u32>,
    /// `paste_wait_ms`.
    pub paste_wait_ms: Option<u32>,
    /// `paste_keys` (Windows only).
    pub paste_keys: Option<PasteShortcut>,
    /// `batch_delay_ms` (Windows only).
    pub batch_delay_ms: Option<u32>,
    /// `chunk_units` (macOS only).
    pub chunk_units: Option<u8>,
}

/// A document read with [`parse`](Self::parse).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InjectionSettings {
    /// `defaults`.
    pub defaults: InjectionEntry,
    /// `apps`, in document order, keyed by [`SettingsDialect::app_key`].
    pub apps: Vec<(String, InjectionEntry)>,
}

/// Why a document was refused. Carries no part of the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsError {
    /// What was wrong, written in the source.
    pub what: &'static str,
    /// The byte offset, when the JSON itself was malformed.
    pub at: Option<usize>,
}

impl SettingsError {
    /// An error without a position.
    #[must_use]
    pub const fn new(what: &'static str) -> Self {
        Self { what, at: None }
    }
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.at {
            Some(at) => write!(f, "{} at byte {at}", self.what),
            None => f.write_str(self.what),
        }
    }
}

impl std::error::Error for SettingsError {}

impl From<json::JsonError> for SettingsError {
    fn from(error: json::JsonError) -> Self {
        Self {
            what: error.what,
            at: Some(error.at),
        }
    }
}

impl From<SettingsError> for PlatformError {
    fn from(error: SettingsError) -> Self {
        Self::Invalid {
            context: error.what,
        }
    }
}

impl InjectionSettings {
    /// Reads `text` as `dialect`'s variant of the format (see the [module docs](self)).
    ///
    /// # Errors
    ///
    /// [`SettingsError`] naming the first thing that was wrong.
    pub fn parse(text: &str, dialect: SettingsDialect) -> Result<Self, SettingsError> {
        let mut settings = Self::default();
        if text.trim().is_empty() {
            return Ok(settings);
        }
        let Value::Object(members) = json::parse(text)? else {
            return Err(SettingsError::new(
                "injection settings must be a JSON object",
            ));
        };
        for (name, value) in &members {
            match name.as_str() {
                "defaults" => settings.defaults = entry(value, dialect)?,
                "apps" => {
                    let Value::Object(apps) = value else {
                        return Err(SettingsError::new("\"apps\" must be an object"));
                    };
                    if apps.len() > MAX_APPS {
                        return Err(SettingsError::new(
                            "too many application entries (at most 1000)",
                        ));
                    }
                    for (app, value) in apps {
                        let key = dialect.app_key(app);
                        if key.is_empty() {
                            return Err(SettingsError::new("empty application id"));
                        }
                        if settings.apps.iter().any(|(known, _)| *known == key) {
                            return Err(SettingsError::new("the same application is listed twice"));
                        }
                        settings.apps.push((key, entry(value, dialect)?));
                    }
                }
                _ => {
                    return Err(SettingsError::new(
                        "unknown top-level member (expected \"defaults\" or \"apps\")",
                    ));
                }
            }
        }
        Ok(settings)
    }

    /// The entry for `app`, looked up the way `dialect` compares application ids.
    #[must_use]
    pub fn app(&self, app: &str, dialect: SettingsDialect) -> Option<&InjectionEntry> {
        let key = dialect.app_key(app);
        self.apps
            .iter()
            .find(|(known, _)| *known == key)
            .map(|(_, entry)| entry)
    }
}

/// A whole number within `bounds`; `what` otherwise.
fn number(value: &Value, bounds: SettingsBounds, what: &'static str) -> Result<u32, SettingsError> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (bounds.min..=bounds.max).contains(n))
        .ok_or(SettingsError::new(what))
}

fn string<'v>(value: &'v Value, what: &'static str) -> Result<&'v str, SettingsError> {
    match value {
        Value::String(s) => Ok(s),
        _ => Err(SettingsError::new(what)),
    }
}

/// One entry, as `dialect` reads it.
fn entry(value: &Value, dialect: SettingsDialect) -> Result<InjectionEntry, SettingsError> {
    use SettingsDialect::{MacOs, Windows};
    let Value::Object(fields) = value else {
        return Err(SettingsError::new("an entry must be an object"));
    };
    let mut entry = InjectionEntry::default();
    for (name, value) in fields {
        match name.as_str() {
            "method" => {
                entry.method = Some(
                    match string(value, "\"method\" must be auto, keys or paste")? {
                        "auto" => InjectionMethod::Auto,
                        "keys" => InjectionMethod::Keys,
                        "paste" => InjectionMethod::Paste,
                        _ => {
                            return Err(SettingsError::new(
                                "\"method\" must be auto, keys or paste",
                            ));
                        }
                    },
                );
            }
            "paste_threshold" => {
                let n = number(
                    value,
                    dialect.paste_threshold(),
                    "\"paste_threshold\" must be a whole number from 0 to 1000000",
                )?;
                entry.paste_threshold = Some(usize::try_from(n).unwrap_or(usize::MAX));
            }
            "line_break" => entry.line_break = Some(line_break(value, dialect)?),
            "key_delay_ms" => {
                entry.key_delay_ms = Some(number(
                    value,
                    dialect.key_delay_ms(),
                    match dialect {
                        Windows => "\"key_delay_ms\" must be a whole number from 0 to 10000",
                        MacOs => "\"key_delay_ms\" must be a whole number from 0 to 1000",
                    },
                )?);
            }
            "paste_wait_ms" => {
                entry.paste_wait_ms = Some(number(
                    value,
                    dialect.paste_wait_ms(),
                    match dialect {
                        Windows => "\"paste_wait_ms\" must be a whole number from 0 to 10000",
                        MacOs => "\"paste_wait_ms\" must be a whole number from 20 to 5000",
                    },
                )?);
            }
            "paste_keys" => {
                if dialect != Windows {
                    return Err(SettingsError::new("\"paste_keys\" is a Windows-only field"));
                }
                let what = "\"paste_keys\" must be ctrl+v or shift+insert";
                entry.paste_keys = Some(match string(value, what)? {
                    "ctrl+v" => PasteShortcut::CtrlV,
                    "shift+insert" => PasteShortcut::ShiftInsert,
                    _ => return Err(SettingsError::new(what)),
                });
            }
            "batch_delay_ms" => {
                if dialect != Windows {
                    return Err(SettingsError::new(
                        "\"batch_delay_ms\" is a Windows-only field",
                    ));
                }
                entry.batch_delay_ms = Some(number(
                    value,
                    dialect.batch_delay_ms(),
                    "\"batch_delay_ms\" must be a whole number from 0 to 10000",
                )?);
            }
            "chunk_units" => {
                if dialect != MacOs {
                    return Err(SettingsError::new("\"chunk_units\" is a macOS-only field"));
                }
                let n = number(
                    value,
                    dialect.chunk_units(),
                    "\"chunk_units\" must be a whole number from 1 to 20",
                )?;
                entry.chunk_units = Some(u8::try_from(n).unwrap_or(u8::MAX));
            }
            _ => return Err(SettingsError::new("unknown field in an entry")),
        }
    }
    Ok(entry)
}

fn line_break(value: &Value, dialect: SettingsDialect) -> Result<LineBreakChoice, SettingsError> {
    let name = string(value, "\"line_break\" must be a string")?;
    let choice = match name {
        "enter" => LineBreakChoice::Enter,
        "shift+enter" => LineBreakChoice::ShiftEnter,
        "option+enter" => LineBreakChoice::OptionEnter,
        "control+option+enter" => LineBreakChoice::ControlOptionEnter,
        "paste" => LineBreakChoice::Paste,
        _ => {
            return Err(SettingsError::new(match dialect {
                SettingsDialect::Windows => {
                    "unknown \"line_break\" value (expected enter or shift+enter)"
                }
                SettingsDialect::MacOs => {
                    "unknown \"line_break\" value (expected enter, shift+enter, option+enter, \
                     control+option+enter or paste)"
                }
            }));
        }
    };
    let shared = matches!(choice, LineBreakChoice::Enter | LineBreakChoice::ShiftEnter);
    if !shared && dialect == SettingsDialect::Windows {
        return Err(SettingsError::new(
            "this \"line_break\" value exists on macOS only (expected enter or shift+enter)",
        ));
    }
    Ok(choice)
}

#[cfg(test)]
mod tests {
    use super::*;

    use SettingsDialect::{MacOs, Windows};

    const BOTH: [SettingsDialect; 2] = [Windows, MacOs];

    fn refused(text: &str, dialect: SettingsDialect) -> &'static str {
        match InjectionSettings::parse(text, dialect) {
            Ok(parsed) => panic!("accepted {text} as {dialect:?}: {parsed:?}"),
            Err(e) => e.what,
        }
    }

    #[test]
    fn blank_and_empty_documents_are_no_settings() {
        for dialect in BOTH {
            for blank in ["", "  \n", "{}", r#"{"apps": {}}"#, r#"{"defaults": {}}"#] {
                assert_eq!(
                    InjectionSettings::parse(blank, dialect),
                    Ok(InjectionSettings::default()),
                    "{blank:?}"
                );
            }
        }
    }

    #[test]
    fn the_shared_fields_read_alike_on_both_systems() {
        let text = r#"{
            "defaults": { "paste_threshold": 400, "method": "auto" },
            "apps": {
                "Chat": { "method": "paste", "line_break": "shift+enter", "key_delay_ms": 3 },
                "Other": { "method": "keys", "line_break": "enter", "paste_wait_ms": 300 }
            }
        }"#;
        for dialect in BOTH {
            let parsed = InjectionSettings::parse(text, dialect).expect("valid");
            assert_eq!(
                parsed.defaults,
                InjectionEntry {
                    paste_threshold: Some(400),
                    method: Some(InjectionMethod::Auto),
                    ..InjectionEntry::default()
                }
            );
            assert_eq!(
                parsed.app("chat", dialect),
                Some(&InjectionEntry {
                    method: Some(InjectionMethod::Paste),
                    line_break: Some(LineBreakChoice::ShiftEnter),
                    key_delay_ms: Some(3),
                    ..InjectionEntry::default()
                })
            );
            let other = parsed.app("OTHER", dialect).expect("listed");
            assert_eq!(other.method, Some(InjectionMethod::Keys));
            assert_eq!(other.line_break, Some(LineBreakChoice::Enter));
            assert_eq!(other.paste_wait_ms, Some(300));
            assert_eq!(parsed.app("absent", dialect), None);
        }
    }

    #[test]
    fn application_ids_are_compared_as_each_system_does() {
        assert_eq!(Windows.app_key(r"C:\Tools\MyEditor.EXE "), "myeditor");
        assert_eq!(Windows.app_key("windowsterminal"), "windowsterminal");
        assert_eq!(Windows.app_key("app.exe.exe"), "app.exe");
        assert_eq!(MacOs.app_key("com.Example.Chat"), "com.example.chat");
        let parsed =
            InjectionSettings::parse(r#"{"apps": {"Editor.exe": {}}}"#, Windows).expect("valid");
        assert!(parsed.app(r"D:\editor", Windows).is_some());
        assert_eq!(
            refused(r#"{"apps": {"a.exe": {}, "A": {}}}"#, Windows),
            "the same application is listed twice"
        );
        assert_eq!(
            refused(r#"{"apps": {"a.b": {}, "A.B": {}}}"#, MacOs),
            "the same application is listed twice"
        );
        for dialect in BOTH {
            assert_eq!(
                refused(r#"{"apps": {"": {}}}"#, dialect),
                "empty application id"
            );
        }
    }

    #[test]
    fn windows_fields() {
        let parsed = InjectionSettings::parse(
            r#"{"defaults": {"paste_keys": "shift+insert", "batch_delay_ms": 30}}"#,
            Windows,
        )
        .expect("valid");
        assert_eq!(parsed.defaults.paste_keys, Some(PasteShortcut::ShiftInsert));
        assert_eq!(parsed.defaults.batch_delay_ms, Some(30));
        let parsed = InjectionSettings::parse(r#"{"defaults": {"paste_keys": "ctrl+v"}}"#, Windows)
            .expect("valid");
        assert_eq!(parsed.defaults.paste_keys, Some(PasteShortcut::CtrlV));
        assert_eq!(
            refused(r#"{"defaults": {"paste_keys": "ctrl_v"}}"#, Windows),
            "\"paste_keys\" must be ctrl+v or shift+insert"
        );
        assert_eq!(
            refused(r#"{"defaults": {"chunk_units": 3}}"#, Windows),
            "\"chunk_units\" is a macOS-only field"
        );
        for value in ["option+enter", "control+option+enter", "paste"] {
            assert_eq!(
                refused(
                    &format!(r#"{{"defaults": {{"line_break": "{value}"}}}}"#),
                    Windows
                ),
                "this \"line_break\" value exists on macOS only (expected enter or shift+enter)",
                "{value}"
            );
        }
    }

    #[test]
    fn macos_fields() {
        for (name, expected) in [
            ("enter", LineBreakChoice::Enter),
            ("shift+enter", LineBreakChoice::ShiftEnter),
            ("option+enter", LineBreakChoice::OptionEnter),
            ("control+option+enter", LineBreakChoice::ControlOptionEnter),
            ("paste", LineBreakChoice::Paste),
        ] {
            let parsed = InjectionSettings::parse(
                &format!(r#"{{"defaults": {{"line_break": "{name}"}}}}"#),
                MacOs,
            )
            .expect("valid");
            assert_eq!(parsed.defaults.line_break, Some(expected), "{name}");
        }
        let parsed =
            InjectionSettings::parse(r#"{"defaults": {"chunk_units": 1}}"#, MacOs).expect("valid");
        assert_eq!(parsed.defaults.chunk_units, Some(1));
        assert_eq!(
            refused(r#"{"defaults": {"paste_keys": "ctrl+v"}}"#, MacOs),
            "\"paste_keys\" is a Windows-only field"
        );
        assert_eq!(
            refused(r#"{"defaults": {"batch_delay_ms": 0}}"#, MacOs),
            "\"batch_delay_ms\" is a Windows-only field"
        );
    }

    #[test]
    fn the_shapes_of_the_earlier_parsers_are_refused() {
        // Development builds wrote these (ADR-0025, consequences).
        for dialect in BOTH {
            assert_eq!(
                refused(r#"{"default": {"paste_threshold": 1}}"#, dialect),
                "unknown top-level member (expected \"defaults\" or \"apps\")"
            );
            assert_eq!(
                refused(r#"{"apps": {"a": {"typing": "keys"}}}"#, dialect),
                "unknown field in an entry"
            );
        }
        for old in ["shift_enter", "return", "shift+return", "option+return"] {
            let text = format!(r#"{{"defaults": {{"line_break": "{old}"}}}}"#);
            assert!(refused(&text, Windows).starts_with("unknown \"line_break\" value"));
            assert!(refused(&text, MacOs).starts_with("unknown \"line_break\" value"));
        }
    }

    #[test]
    fn ranges_are_enforced_per_system() {
        let cases: &[(&str, SettingsDialect, bool)] = &[
            (
                r#"{"defaults": {"paste_threshold": 1000000}}"#,
                Windows,
                true,
            ),
            (
                r#"{"defaults": {"paste_threshold": 1000001}}"#,
                MacOs,
                false,
            ),
            (r#"{"defaults": {"paste_threshold": -1}}"#, Windows, false),
            (r#"{"defaults": {"key_delay_ms": 10000}}"#, Windows, true),
            (r#"{"defaults": {"key_delay_ms": 10001}}"#, Windows, false),
            (r#"{"defaults": {"key_delay_ms": 1000}}"#, MacOs, true),
            (r#"{"defaults": {"key_delay_ms": 1001}}"#, MacOs, false),
            (r#"{"defaults": {"key_delay_ms": 1.5}}"#, MacOs, false),
            (r#"{"defaults": {"paste_wait_ms": 0}}"#, Windows, true),
            (r#"{"defaults": {"paste_wait_ms": 10001}}"#, Windows, false),
            (r#"{"defaults": {"paste_wait_ms": 19}}"#, MacOs, false),
            (r#"{"defaults": {"paste_wait_ms": 20}}"#, MacOs, true),
            (r#"{"defaults": {"paste_wait_ms": 5000}}"#, MacOs, true),
            (r#"{"defaults": {"paste_wait_ms": 6000}}"#, MacOs, false),
            (r#"{"defaults": {"batch_delay_ms": 10000}}"#, Windows, true),
            (r#"{"defaults": {"batch_delay_ms": 10001}}"#, Windows, false),
            (r#"{"defaults": {"chunk_units": 0}}"#, MacOs, false),
            (r#"{"defaults": {"chunk_units": 20}}"#, MacOs, true),
            (r#"{"defaults": {"chunk_units": 21}}"#, MacOs, false),
            (
                r#"{"defaults": {"paste_wait_ms": 99999999999999999999}}"#,
                Windows,
                false,
            ),
            (
                r#"{"defaults": {"paste_threshold": "200"}}"#,
                Windows,
                false,
            ),
            (r#"{"defaults": {"paste_threshold": 3e2}}"#, Windows, false),
        ];
        for (text, dialect, ok) in cases {
            assert_eq!(
                InjectionSettings::parse(text, *dialect).is_ok(),
                *ok,
                "{text} as {dialect:?}"
            );
        }
        assert_eq!(
            refused(r#"{"defaults": {"key_delay_ms": 1001}}"#, MacOs),
            "\"key_delay_ms\" must be a whole number from 0 to 1000"
        );
        assert_eq!(
            refused(r#"{"defaults": {"paste_wait_ms": 5}}"#, MacOs),
            "\"paste_wait_ms\" must be a whole number from 20 to 5000"
        );
        assert_eq!(
            refused(r#"{"defaults": {"chunk_units": 21}}"#, MacOs),
            "\"chunk_units\" must be a whole number from 1 to 20"
        );
    }

    #[test]
    fn bad_documents_are_refused_with_a_reason() {
        let cases: &[(&str, &str)] = &[
            ("[]", "injection settings must be a JSON object"),
            ("oops", "malformed JSON: unexpected character"),
            (
                r#"{"colour": 1}"#,
                "unknown top-level member (expected \"defaults\" or \"apps\")",
            ),
            (r#"{"apps": []}"#, "\"apps\" must be an object"),
            (r#"{"defaults": 3}"#, "an entry must be an object"),
            (r#"{"apps": {"a": 1}}"#, "an entry must be an object"),
            (r#"{"defaults": {"speed": 1}}"#, "unknown field in an entry"),
            (
                r#"{"defaults": {"restore": false}}"#,
                "unknown field in an entry",
            ),
            (
                r#"{"defaults": {"method": "fast"}}"#,
                "\"method\" must be auto, keys or paste",
            ),
            (
                r#"{"defaults": {"method": 1}}"#,
                "\"method\" must be auto, keys or paste",
            ),
            (
                r#"{"defaults": {"line_break": 1}}"#,
                "\"line_break\" must be a string",
            ),
        ];
        for dialect in BOTH {
            for (text, what) in cases {
                assert_eq!(refused(text, dialect), *what, "{text} as {dialect:?}");
            }
            let malformed =
                InjectionSettings::parse("{\"apps\": ", dialect).expect_err("malformed");
            assert_eq!(malformed.at, Some(9));
            assert_eq!(
                malformed.to_string(),
                "malformed JSON: unexpected end at byte 9"
            );
            let many: Vec<String> = (0..=MAX_APPS).map(|i| format!("\"a.{i}\": {{}}")).collect();
            assert_eq!(
                refused(&format!("{{\"apps\": {{{}}}}}", many.join(",")), dialect),
                "too many application entries (at most 1000)"
            );
        }
    }

    #[test]
    fn errors_never_quote_the_settings() {
        for dialect in BOTH {
            let err =
                InjectionSettings::parse(r#"{"apps": {"private-app": {"x-private": 1}}}"#, dialect)
                    .expect_err("unknown field");
            assert!(!err.to_string().contains("private"));
            let platform = PlatformError::from(err);
            assert!(matches!(platform, PlatformError::Invalid { .. }));
            assert!(!platform.to_string().contains("private"));
        }
    }
}
