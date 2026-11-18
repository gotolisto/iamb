//! The per-application strategy table (architecture §4.2; compatibility matrix §5).
//!
//! How text reaches an application differs per application: some send a message on Return,
//! some rewrite typed words, some cannot keep up with fast key events, and a remote-desktop
//! client forwards key codes but not the Unicode strings that carry the text. A [`Strategy`]
//! says how to send one expansion; the [`StrategyTable`] picks it by bundle identifier.
//!
//! # Fields
//!
//! | Field | Meaning | Default |
//! | --- | --- | --- |
//! | `method` | `auto`: key events, or a paste above `paste_threshold`; `keys`: always key events; `paste`: always a paste | `auto` |
//! | `paste_threshold` | `auto` pastes text with more characters (Unicode scalar values) than this | 300 |
//! | `line_break` | how a line break in typed text is sent: `enter`, `shift+enter`, `option+enter`, `control+option+enter` (each presses Return), or `paste` (text with a line break is pasted whole) | `enter` |
//! | `key_delay_ms` | pause between two synthetic key presses, for applications that drop fast input | 0 |
//! | `paste_wait_ms` | how long the pasted content stays on the pasteboard after the paste shortcut before the previous content is put back | 250 |
//! | `chunk_units` | the most UTF-16 code units in one key event (1–20); macOS only | 20 |
//!
//! # Precedence
//!
//! Lowest to highest: the built-in defaults, the user's `defaults`, the built-in entry for the
//! application, the user's entry for the application, and the request's
//! [`InjectionOverrides`] (`text_method`, `key_delay_ms`). A user entry changes only the
//! fields it names. Bundle identifiers are compared without regard to ASCII case, as macOS
//! does.
//!
//! # The user's settings
//!
//! The service hands the `injection_overrides` setting to
//! [`Injector::configure`](iamb_platform::Injector::configure). It is read with
//! [`InjectionSettings::parse`] in its macOS dialect: the format both systems share
//! (`iamb_platform::injection_settings`, ADR-0025), with `chunk_units` and the macOS line
//! breaks. Every member is optional and unknown names are an error:
//!
//! ```json
//! {
//!   "defaults": { "paste_threshold": 500 },
//!   "apps": {
//!     "com.example.SlowEditor": { "key_delay_ms": 4, "chunk_units": 1 },
//!     "com.example.Chat": { "line_break": "shift+enter", "method": "paste" }
//!   }
//! }
//! ```
//!
//! Limits: `paste_threshold` at most 1,000,000; `key_delay_ms` at most 1,000; `paste_wait_ms`
//! 20 to 5,000 (a paste needs the content on the pasteboard long enough for the application
//! to read it); `chunk_units` 1 to 20; at most 1,000 application entries. The Windows-only
//! fields (`paste_keys`, `batch_delay_ms`) are errors here.
//!
//! # The built-in table
//!
//! Small and conservative: an entry exists only where the default is known to give the wrong
//! text, and each says why ([`BUILT_IN`]). Applications that fail a compatibility-matrix cell
//! get an entry here when an override fixes the cell (matrix §5).

use std::time::Duration;

use iamb_platform::core_types::AppId;
use iamb_platform::{
    InjectionEntry, InjectionMethod, InjectionOverrides, InjectionSettings, LineBreakChoice,
    SettingsDialect, SettingsError, TextMethod,
};

/// How `TypeText` is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    /// Key events, or a paste for text longer than the threshold.
    Auto,
    /// Always key events.
    Keys,
    /// Always a paste.
    Paste,
}

/// Modifiers held with a key. Command is never held for output; only the paste shortcut
/// uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per modifier key, as in the system's event flags"
)]
pub(crate) struct Mods {
    pub(crate) shift: bool,
    pub(crate) option: bool,
    pub(crate) control: bool,
    pub(crate) command: bool,
}

impl Mods {
    pub(crate) const NONE: Self = Self {
        shift: false,
        option: false,
        control: false,
        command: false,
    };
    pub(crate) const SHIFT: Self = Self {
        shift: true,
        ..Self::NONE
    };
    pub(crate) const OPTION: Self = Self {
        option: true,
        ..Self::NONE
    };
    pub(crate) const CONTROL_OPTION: Self = Self {
        option: true,
        control: true,
        ..Self::NONE
    };
    pub(crate) const COMMAND: Self = Self {
        command: true,
        ..Self::NONE
    };
}

/// How a line break in typed text is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineBreak {
    /// Return, with these modifiers.
    Return(Mods),
    /// Text that contains a line break is pasted whole. When key events are forced, plain
    /// Return is pressed.
    Paste,
}

impl From<LineBreakChoice> for LineBreak {
    fn from(choice: LineBreakChoice) -> Self {
        match choice {
            LineBreakChoice::Enter => Self::Return(Mods::NONE),
            LineBreakChoice::ShiftEnter => Self::Return(Mods::SHIFT),
            LineBreakChoice::OptionEnter => Self::Return(Mods::OPTION),
            LineBreakChoice::ControlOptionEnter => Self::Return(Mods::CONTROL_OPTION),
            LineBreakChoice::Paste => Self::Paste,
        }
    }
}

/// How to send one expansion to one application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Strategy {
    pub(crate) method: Method,
    pub(crate) paste_threshold: usize,
    pub(crate) line_break: LineBreak,
    pub(crate) key_delay_ms: u32,
    pub(crate) paste_wait_ms: u32,
    pub(crate) chunk_units: u8,
}

impl Strategy {
    /// The built-in defaults.
    pub(crate) const DEFAULT: Self = Self {
        method: Method::Auto,
        paste_threshold: 300,
        line_break: LineBreak::Return(Mods::NONE),
        key_delay_ms: 0,
        paste_wait_ms: 250,
        chunk_units: 20,
    };

    pub(crate) fn key_delay(&self) -> Duration {
        Duration::from_millis(u64::from(self.key_delay_ms))
    }

    pub(crate) fn paste_wait(&self) -> Duration {
        Duration::from_millis(u64::from(self.paste_wait_ms))
    }

    fn apply(&mut self, patch: &Patch) {
        let Patch {
            method,
            paste_threshold,
            line_break,
            key_delay_ms,
            paste_wait_ms,
            chunk_units,
        } = *patch;
        self.method = method.unwrap_or(self.method);
        self.paste_threshold = paste_threshold.unwrap_or(self.paste_threshold);
        self.line_break = line_break.unwrap_or(self.line_break);
        self.key_delay_ms = key_delay_ms.unwrap_or(self.key_delay_ms);
        self.paste_wait_ms = paste_wait_ms.unwrap_or(self.paste_wait_ms);
        self.chunk_units = chunk_units.unwrap_or(self.chunk_units);
    }
}

impl Default for Strategy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Some fields of a [`Strategy`]; `None` keeps what a lower layer said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Patch {
    pub(crate) method: Option<Method>,
    pub(crate) paste_threshold: Option<usize>,
    pub(crate) line_break: Option<LineBreak>,
    pub(crate) key_delay_ms: Option<u32>,
    pub(crate) paste_wait_ms: Option<u32>,
    pub(crate) chunk_units: Option<u8>,
}

impl Patch {
    const NONE: Self = Self {
        method: None,
        paste_threshold: None,
        line_break: None,
        key_delay_ms: None,
        paste_wait_ms: None,
        chunk_units: None,
    };
}

/// One row of the built-in table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BuiltIn {
    pub(crate) bundle_id: &'static str,
    pub(crate) patch: Patch,
    pub(crate) reason: &'static str,
}

/// The built-in table. Keep it small: an entry needs a known failure of the defaults.
pub(crate) const BUILT_IN: &[BuiltIn] = &[
    BuiltIn {
        bundle_id: "com.tinyspeck.slackmacgap",
        patch: Patch {
            line_break: Some(LineBreak::Return(Mods::SHIFT)),
            ..Patch::NONE
        },
        reason: "Return sends the message; Shift+Return starts a new line (matrix M6)",
    },
    BuiltIn {
        bundle_id: "com.apple.MobileSMS",
        patch: Patch {
            line_break: Some(LineBreak::Paste),
            ..Patch::NONE
        },
        reason: "Return sends the message; a paste keeps the line breaks (matrix M17)",
    },
    BuiltIn {
        bundle_id: "com.microsoft.Excel",
        patch: Patch {
            line_break: Some(LineBreak::Return(Mods::CONTROL_OPTION)),
            ..Patch::NONE
        },
        reason: "Return commits the cell; a line break inside a cell is Control+Option+Return \
                 (matrix M9)",
    },
    BuiltIn {
        bundle_id: "com.microsoft.VSCode",
        patch: Patch {
            line_break: Some(LineBreak::Paste),
            ..Patch::NONE
        },
        reason: "Return auto-indents the next line, so typed lines gain indentation; a paste \
                 inserts the lines as written (matrix M7)",
    },
    BuiltIn {
        bundle_id: "com.apple.dt.Xcode",
        patch: Patch {
            line_break: Some(LineBreak::Paste),
            ..Patch::NONE
        },
        reason: "Return accepts a completion and auto-indents; a paste inserts the lines as \
                 written (matrix M12)",
    },
    BuiltIn {
        bundle_id: "com.microsoft.Word",
        patch: Patch {
            method: Some(Method::Paste),
            ..Patch::NONE
        },
        reason: "AutoCorrect rewrites typed text (quotes, capitals, replacements); pasted text \
                 is left alone (matrix M8)",
    },
    BuiltIn {
        bundle_id: "org.libreoffice.script",
        patch: Patch {
            method: Some(Method::Paste),
            ..Patch::NONE
        },
        reason: "AutoCorrect rewrites typed text, as in Word; pasted text is left alone \
                 (matrix M14)",
    },
    BuiltIn {
        bundle_id: "com.microsoft.rdc.macos",
        patch: Patch {
            method: Some(Method::Paste),
            paste_wait_ms: Some(600),
            ..Patch::NONE
        },
        reason: "the remote session receives key codes, not the Unicode strings that carry the \
                 text, so only a paste arrives intact; the pasteboard is forwarded to the \
                 remote side, which takes longer to read it (matrix M18)",
    },
];

/// The table: built-in rows and the user's settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct StrategyTable {
    user_defaults: Patch,
    user_apps: Vec<(String, Patch)>,
}

impl StrategyTable {
    /// The built-in table without user settings.
    #[cfg(test)]
    pub(crate) fn built_in() -> Self {
        Self::default()
    }

    /// The built-in table with the user's settings (see the module docs for the format). An
    /// empty or all-blank string means no settings.
    pub(crate) fn with_user_settings(settings: &str) -> Result<Self, SettingsError> {
        let parsed = InjectionSettings::parse(settings, SettingsDialect::MacOs)?;
        Ok(Self {
            user_defaults: patch(&parsed.defaults),
            user_apps: parsed
                .apps
                .iter()
                .map(|(bundle_id, entry)| (bundle_id.clone(), patch(entry)))
                .collect(),
        })
    }

    /// The strategy for an expansion into `app` with the request's `overrides`.
    pub(crate) fn resolve(&self, app: Option<&AppId>, overrides: &InjectionOverrides) -> Strategy {
        let mut strategy = Strategy::DEFAULT;
        strategy.apply(&self.user_defaults);
        if let Some(AppId(bundle_id)) = app {
            if let Some(row) = BUILT_IN
                .iter()
                .find(|row| row.bundle_id.eq_ignore_ascii_case(bundle_id))
            {
                strategy.apply(&row.patch);
            }
            if let Some((_, patch)) = self
                .user_apps
                .iter()
                .find(|(known, _)| known.eq_ignore_ascii_case(bundle_id))
            {
                strategy.apply(patch);
            }
        }
        match overrides.text_method {
            Some(TextMethod::Keys) => strategy.method = Method::Keys,
            Some(TextMethod::Paste) => strategy.method = Method::Paste,
            None => {}
        }
        if let Some(delay) = overrides.key_delay_ms {
            strategy.key_delay_ms = delay.min(MAX_KEY_DELAY_MS);
        }
        strategy
    }
}

/// The longest key delay a setting, or a request, may ask for.
const MAX_KEY_DELAY_MS: u32 = SettingsDialect::MacOs.key_delay_ms().max;

/// One entry of the user's settings as a layer of the table. The macOS dialect refuses the
/// Windows-only fields, so there are none to drop.
fn patch(entry: &InjectionEntry) -> Patch {
    Patch {
        method: entry.method.map(|method| match method {
            InjectionMethod::Auto => Method::Auto,
            InjectionMethod::Keys => Method::Keys,
            InjectionMethod::Paste => Method::Paste,
        }),
        paste_threshold: entry.paste_threshold,
        line_break: entry.line_break.map(LineBreak::from),
        key_delay_ms: entry.key_delay_ms,
        paste_wait_ms: entry.paste_wait_ms,
        chunk_units: entry.chunk_units,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> AppId {
        AppId(id.to_owned())
    }

    fn none() -> InjectionOverrides {
        InjectionOverrides::default()
    }

    #[test]
    fn unknown_and_missing_applications_get_the_defaults() {
        let table = StrategyTable::built_in();
        assert_eq!(table.resolve(None, &none()), Strategy::DEFAULT);
        assert_eq!(
            table.resolve(Some(&app("com.example.Unknown")), &none()),
            Strategy::DEFAULT
        );
        assert_eq!(Strategy::default(), Strategy::DEFAULT);
        assert_eq!(Strategy::DEFAULT.key_delay(), Duration::ZERO);
        assert_eq!(Strategy::DEFAULT.paste_wait(), Duration::from_millis(250));
    }

    #[test]
    fn built_in_rows_apply_by_bundle_identifier_in_any_case() {
        let table = StrategyTable::built_in();
        let slack = table.resolve(Some(&app("com.tinyspeck.slackmacgap")), &none());
        assert_eq!(slack.line_break, LineBreak::Return(Mods::SHIFT));
        assert_eq!(slack.method, Method::Auto);
        let shouting = table.resolve(Some(&app("COM.TinySpeck.SlackMacGap")), &none());
        assert_eq!(shouting, slack);
        let word = table.resolve(Some(&app("com.microsoft.Word")), &none());
        assert_eq!(word.method, Method::Paste);
        let remote = table.resolve(Some(&app("com.microsoft.rdc.macos")), &none());
        assert_eq!((remote.method, remote.paste_wait_ms), (Method::Paste, 600));
        let excel = table.resolve(Some(&app("com.microsoft.Excel")), &none());
        assert_eq!(excel.line_break, LineBreak::Return(Mods::CONTROL_OPTION));
    }

    #[test]
    fn the_built_in_table_is_well_formed() {
        for (i, row) in BUILT_IN.iter().enumerate() {
            assert!(!row.reason.is_empty(), "{} has a reason", row.bundle_id);
            assert_ne!(
                row.patch,
                Patch::NONE,
                "{} changes something",
                row.bundle_id
            );
            assert!(
                BUILT_IN[..i]
                    .iter()
                    .all(|other| !other.bundle_id.eq_ignore_ascii_case(row.bundle_id)),
                "{} is listed once",
                row.bundle_id
            );
            assert!(row.bundle_id.contains('.') && !row.bundle_id.contains(' '));
        }
    }

    #[test]
    fn user_settings_layer_over_the_built_in_rows() {
        let table = StrategyTable::with_user_settings(
            r#"{
                "defaults": { "paste_threshold": 500, "key_delay_ms": 2 },
                "apps": {
                    "com.tinyspeck.slackmacgap": { "method": "keys" },
                    "com.example.Slow": { "chunk_units": 1, "paste_wait_ms": 800 },
                    "com.microsoft.word": { "method": "auto", "line_break": "option+enter" }
                }
            }"#,
        )
        .expect("valid settings");
        // Defaults reach every application.
        let other = table.resolve(Some(&app("com.example.Other")), &none());
        assert_eq!((other.paste_threshold, other.key_delay_ms), (500, 2));
        // A user row changes only what it names; the built-in row still applies.
        let slack = table.resolve(Some(&app("com.tinyspeck.slackmacgap")), &none());
        assert_eq!(slack.method, Method::Keys);
        assert_eq!(slack.line_break, LineBreak::Return(Mods::SHIFT));
        assert_eq!(slack.paste_threshold, 500);
        let slow = table.resolve(Some(&app("com.example.slow")), &none());
        assert_eq!((slow.chunk_units, slow.paste_wait_ms), (1, 800));
        // A user row beats the built-in row, matched in any case.
        let word = table.resolve(Some(&app("com.microsoft.Word")), &none());
        assert_eq!(word.method, Method::Auto);
        assert_eq!(word.line_break, LineBreak::Return(Mods::OPTION));
    }

    #[test]
    fn request_overrides_win() {
        let table = StrategyTable::built_in();
        let mut overrides = InjectionOverrides::default();
        overrides.text_method = Some(TextMethod::Keys);
        overrides.key_delay_ms = Some(7);
        let word = table.resolve(Some(&app("com.microsoft.Word")), &overrides);
        assert_eq!((word.method, word.key_delay_ms), (Method::Keys, 7));
        overrides.text_method = Some(TextMethod::Paste);
        overrides.key_delay_ms = Some(u32::MAX);
        let any = table.resolve(None, &overrides);
        assert_eq!(
            (any.method, any.key_delay_ms),
            (Method::Paste, MAX_KEY_DELAY_MS)
        );
    }

    #[test]
    fn every_line_break_name() {
        for (name, expected) in [
            ("enter", LineBreak::Return(Mods::NONE)),
            ("shift+enter", LineBreak::Return(Mods::SHIFT)),
            ("option+enter", LineBreak::Return(Mods::OPTION)),
            (
                "control+option+enter",
                LineBreak::Return(Mods::CONTROL_OPTION),
            ),
            ("paste", LineBreak::Paste),
        ] {
            let table = StrategyTable::with_user_settings(&format!(
                r#"{{"defaults": {{"line_break": "{name}"}}}}"#
            ))
            .expect("valid");
            assert_eq!(table.resolve(None, &none()).line_break, expected);
        }
    }

    #[test]
    fn empty_settings_are_the_built_in_table() {
        for blank in ["", "  \n", "{}", r#"{"apps": {}}"#, r#"{"defaults": {}}"#] {
            assert_eq!(
                StrategyTable::with_user_settings(blank),
                Ok(StrategyTable::built_in())
            );
        }
    }

    #[test]
    fn bad_settings_are_refused_with_a_reason_and_no_content() {
        // The full list of refusals is tested with the shared format in `iamb-platform`;
        // these are the ones that matter on macOS.
        let cases: &[(&str, &str)] = &[
            ("[]", "injection settings must be a JSON object"),
            (
                r#"{"colour": 1}"#,
                "unknown top-level member (expected \"defaults\" or \"apps\")",
            ),
            (
                r#"{"apps": {"a.b": {}, "A.B": {}}}"#,
                "the same application is listed twice",
            ),
            (r#"{"defaults": {"speed": 1}}"#, "unknown field in an entry"),
            (
                r#"{"defaults": {"method": "fast"}}"#,
                "\"method\" must be auto, keys or paste",
            ),
            // The line-break names of earlier development builds (ADR-0025).
            (
                r#"{"defaults": {"line_break": "shift+return"}}"#,
                "unknown \"line_break\" value (expected enter, shift+enter, option+enter, \
                 control+option+enter or paste)",
            ),
            (
                r#"{"defaults": {"paste_keys": "ctrl+v"}}"#,
                "\"paste_keys\" is a Windows-only field",
            ),
            (
                r#"{"defaults": {"batch_delay_ms": 10}}"#,
                "\"batch_delay_ms\" is a Windows-only field",
            ),
            (
                r#"{"defaults": {"key_delay_ms": 1001}}"#,
                "\"key_delay_ms\" must be a whole number from 0 to 1000",
            ),
            (
                r#"{"defaults": {"paste_wait_ms": 5}}"#,
                "\"paste_wait_ms\" must be a whole number from 20 to 5000",
            ),
            (
                r#"{"defaults": {"chunk_units": 0}}"#,
                "\"chunk_units\" must be a whole number from 1 to 20",
            ),
            (
                r#"{"defaults": {"chunk_units": 21}}"#,
                "\"chunk_units\" must be a whole number from 1 to 20",
            ),
        ];
        for (settings, what) in cases {
            assert_eq!(
                StrategyTable::with_user_settings(settings),
                Err(SettingsError::new(what)),
                "{settings}"
            );
        }
        let malformed = StrategyTable::with_user_settings("{\"apps\": ").expect_err("malformed");
        assert_eq!(malformed.at, Some(9));
    }
}
