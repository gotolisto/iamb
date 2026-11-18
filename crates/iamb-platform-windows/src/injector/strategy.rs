//! The per-application strategy table (architecture §4.2): how text reaches each application.
//!
//! The table is data. Everything the injector decides per application comes out of one
//! resolved [`Strategy`]; no other code asks which application is in front.
//!
//! # Resolution
//!
//! For the application an expansion is for (its executable name, compared without case, with
//! or without `.exe`, and without a directory), the strategy is built in layers, each setting
//! only the fields it names:
//!
//! 1. [`Strategy::DEFAULT`];
//! 2. the user's `"default"` entry from settings;
//! 3. the built-in entry for the application ([`BUILT_IN`]), so a general user default cannot
//!    switch a terminal to pasting by accident;
//! 4. the user's entry for the application;
//! 5. the request's own [`InjectionOverrides`] (text method, key delay).
//!
//! # User overrides
//!
//! Settings keep the overrides as a JSON string (the service stores it; this module parses
//! it):
//!
//! ```json
//! {
//!   "default": { "paste_threshold": 400 },
//!   "apps": {
//!     "myeditor.exe": { "typing": "keys", "line_break": "shift_enter" },
//!     "slowremote":   { "key_delay_ms": 5, "batch_delay_ms": 30, "paste_wait_ms": 400 }
//!   }
//! }
//! ```
//!
//! | Field | Values | Meaning |
//! | --- | --- | --- |
//! | `typing` | `"auto"`, `"keys"`, `"paste"` | `auto`: Unicode key events, a paste above `paste_threshold` characters; `keys`: never paste text (a rich paste types its plain text); `paste`: always paste |
//! | `paste_threshold` | 0 to 1 000 000 | characters of one text step above which `auto` pastes |
//! | `line_break` | `"enter"`, `"shift_enter"` | the key a typed line break presses |
//! | `paste_keys` | `"ctrl_v"`, `"shift_insert"` | the paste shortcut |
//! | `key_delay_ms` | 0 to 10 000 | pause after every key (gives up the single batch) |
//! | `batch_delay_ms` | 0 to 10 000 | pause between batches |
//! | `paste_wait_ms` | 0 to 10 000 | how long the application gets to read a paste before the clipboard is restored |
//!
//! Unknown fields and values out of range are errors, so a typo is reported and not ignored.
//! The clipboard is always restored after a paste (§8 rule 8); no field turns that off.

use iamb_platform::{InjectionOverrides, PlatformError, TextMethod};

use super::json::{self, Json};

/// Characters of one text step above which `auto` pastes.
pub(crate) const DEFAULT_PASTE_THRESHOLD: usize = 200;
/// How long a target gets to read a paste before the clipboard is restored.
pub(crate) const DEFAULT_PASTE_WAIT_MS: u32 = 150;
/// The longest delay a setting may ask for.
pub(crate) const MAX_DELAY_MS: u32 = 10_000;
/// The largest paste threshold a setting may ask for.
pub(crate) const MAX_PASTE_THRESHOLD: usize = 1_000_000;

/// How text steps are sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Typing {
    /// Unicode key events; a paste for text above the threshold.
    Auto,
    /// Unicode key events only.
    Keys,
    /// Always a paste.
    Paste,
}

/// What a line break in typed text presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineBreak {
    /// Enter.
    Enter,
    /// Shift+Enter: for applications where Enter sends the message.
    ShiftEnter,
}

/// The paste shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasteKeys {
    /// Ctrl+V.
    CtrlV,
    /// Shift+Insert.
    ShiftInsert,
}

/// Everything the injector decides per application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Strategy {
    pub(crate) typing: Typing,
    pub(crate) paste_threshold: usize,
    pub(crate) line_break: LineBreak,
    pub(crate) paste_keys: PasteKeys,
    pub(crate) key_delay_ms: u32,
    pub(crate) batch_delay_ms: u32,
    pub(crate) paste_wait_ms: u32,
}

impl Strategy {
    /// The strategy of an application nobody has said anything about.
    pub(crate) const DEFAULT: Self = Self {
        typing: Typing::Auto,
        paste_threshold: DEFAULT_PASTE_THRESHOLD,
        line_break: LineBreak::Enter,
        paste_keys: PasteKeys::CtrlV,
        key_delay_ms: 0,
        batch_delay_ms: 0,
        paste_wait_ms: DEFAULT_PASTE_WAIT_MS,
    };

    /// True when a text step of `chars` characters is sent by paste.
    pub(crate) const fn pastes_text(&self, chars: usize) -> bool {
        match self.typing {
            Typing::Auto => chars > self.paste_threshold,
            Typing::Keys => false,
            Typing::Paste => true,
        }
    }

    /// True when a `Paste` step whose content has plain text is typed instead: the
    /// application must not be pasted into.
    pub(crate) const fn types_pastes(&self) -> bool {
        matches!(self.typing, Typing::Keys)
    }
}

impl Default for Strategy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Some fields of a strategy: one layer of the resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Patch {
    pub(crate) typing: Option<Typing>,
    pub(crate) paste_threshold: Option<usize>,
    pub(crate) line_break: Option<LineBreak>,
    pub(crate) paste_keys: Option<PasteKeys>,
    pub(crate) key_delay_ms: Option<u32>,
    pub(crate) batch_delay_ms: Option<u32>,
    pub(crate) paste_wait_ms: Option<u32>,
}

impl Patch {
    /// A patch that changes nothing.
    pub(crate) const EMPTY: Self = Self {
        typing: None,
        paste_threshold: None,
        line_break: None,
        paste_keys: None,
        key_delay_ms: None,
        batch_delay_ms: None,
        paste_wait_ms: None,
    };

    /// Sets the fields this patch names.
    pub(crate) fn apply(&self, strategy: &mut Strategy) {
        if let Some(v) = self.typing {
            strategy.typing = v;
        }
        if let Some(v) = self.paste_threshold {
            strategy.paste_threshold = v;
        }
        if let Some(v) = self.line_break {
            strategy.line_break = v;
        }
        if let Some(v) = self.paste_keys {
            strategy.paste_keys = v;
        }
        if let Some(v) = self.key_delay_ms {
            strategy.key_delay_ms = v;
        }
        if let Some(v) = self.batch_delay_ms {
            strategy.batch_delay_ms = v;
        }
        if let Some(v) = self.paste_wait_ms {
            strategy.paste_wait_ms = v;
        }
    }
}

/// One entry of the built-in table.
#[derive(Debug)]
pub(crate) struct BuiltIn {
    /// Executable names, lower case, without `.exe`.
    pub(crate) apps: &'static [&'static str],
    /// What differs from the default.
    pub(crate) patch: Patch,
    /// Why, for whoever changes the table (and a settings screen, once one shows it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) why: &'static str,
}

/// The built-in table. Conservative on purpose: an entry exists only where the default is
/// known to go wrong, and changes only what goes wrong.
pub(crate) const BUILT_IN: &[BuiltIn] = &[
    // Terminals: Ctrl+V is not paste in every console (and is a control character in a
    // shell), and pasted text can trigger multi-line paste warnings or bracketed-paste
    // handling. Typing behaves like the user typing.
    BuiltIn {
        apps: &[
            "windowsterminal",
            "openconsole",
            "conhost",
            "cmd",
            "powershell",
            "pwsh",
            "mintty",
            "putty",
            "alacritty",
            "wezterm-gui",
        ],
        patch: Patch {
            typing: Some(Typing::Keys),
            ..Patch::EMPTY
        },
        why: "terminals: Ctrl+V is not reliably paste and pasted text can trigger paste \
              warnings; type instead",
    },
    // Remote-desktop clients forward keys to another machine. Their clipboard is synced
    // asynchronously, so a paste can insert stale content and the restore can race the sync;
    // and the remote side drops keys that arrive too fast.
    BuiltIn {
        apps: &["mstsc", "msrdc"],
        patch: Patch {
            typing: Some(Typing::Keys),
            batch_delay_ms: Some(20),
            key_delay_ms: Some(2),
            ..Patch::EMPTY
        },
        why: "remote desktop: the clipboard syncs asynchronously and keys sent too quickly \
              are lost; type, slowly",
    },
    // Chat clients where Enter sends the message: a line break inside an expansion must not
    // send half of it.
    BuiltIn {
        apps: &[
            "slack", "teams", "ms-teams", "discord", "signal", "telegram", "whatsapp",
        ],
        patch: Patch {
            line_break: Some(LineBreak::ShiftEnter),
            ..Patch::EMPTY
        },
        why: "chat clients: Enter sends the message, Shift+Enter starts a new line",
    },
];

/// The executable name in the form the table compares: the file name only, lower case,
/// without `.exe`.
pub(crate) fn normalize_app(name: &str) -> String {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name).trim();
    let lower = file.to_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => lower,
    }
}

/// The built-in entry for an application, if any.
pub(crate) fn built_in(app: &str) -> Option<&'static BuiltIn> {
    let key = normalize_app(app);
    BUILT_IN
        .iter()
        .find(|entry| entry.apps.contains(&key.as_str()))
}

/// The strategy table: the built-in entries with the user's overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StrategyTable {
    user_default: Patch,
    /// Normalised executable name and the user's patch.
    user_apps: Vec<(String, Patch)>,
}

impl StrategyTable {
    /// The built-in table, with no user overrides.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The built-in table with the user's overrides from settings (see the module docs). An
    /// empty or blank string means no overrides.
    pub(crate) fn from_settings(settings: &str) -> Result<Self, PlatformError> {
        if settings.trim().is_empty() {
            return Ok(Self::new());
        }
        let invalid = |context| PlatformError::Invalid { context };
        let root = json::parse(settings).map_err(invalid)?;
        let Json::Object(members) = root else {
            return Err(invalid("strategy overrides: expected a JSON object"));
        };
        let mut table = Self::new();
        for (key, value) in &members {
            match key.as_str() {
                "default" => table.user_default = patch(value)?,
                "apps" => {
                    let Json::Object(apps) = value else {
                        return Err(invalid("strategy overrides: \"apps\" must be an object"));
                    };
                    for (app, value) in apps {
                        let name = normalize_app(app);
                        if name.is_empty() {
                            return Err(invalid("strategy overrides: empty application name"));
                        }
                        if table.user_apps.iter().any(|(n, _)| *n == name) {
                            return Err(invalid(
                                "strategy overrides: the same application is named twice",
                            ));
                        }
                        table.user_apps.push((name, patch(value)?));
                    }
                }
                _ => return Err(invalid("strategy overrides: unknown top-level field")),
            }
        }
        Ok(table)
    }

    /// The strategy for `app` (an executable name; `None` when unknown) with the request's
    /// overrides on top.
    pub(crate) fn resolve(&self, app: Option<&str>, overrides: &InjectionOverrides) -> Strategy {
        let mut strategy = Strategy::DEFAULT;
        self.user_default.apply(&mut strategy);
        if let Some(app) = app {
            let key = normalize_app(app);
            if let Some(entry) = built_in(&key) {
                entry.patch.apply(&mut strategy);
            }
            if let Some((_, user)) = self.user_apps.iter().find(|(n, _)| *n == key) {
                user.apply(&mut strategy);
            }
        }
        match overrides.text_method {
            Some(TextMethod::Keys) => strategy.typing = Typing::Keys,
            Some(TextMethod::Paste) => strategy.typing = Typing::Paste,
            None => {}
        }
        if let Some(delay) = overrides.key_delay_ms {
            strategy.key_delay_ms = delay.min(MAX_DELAY_MS);
        }
        strategy
    }
}

/// One entry of the user's overrides.
fn patch(value: &Json) -> Result<Patch, PlatformError> {
    let invalid = |context| PlatformError::Invalid { context };
    let Json::Object(fields) = value else {
        return Err(invalid("strategy overrides: an entry must be an object"));
    };
    let mut patch = Patch::EMPTY;
    for (field, value) in fields {
        match field.as_str() {
            "typing" => {
                patch.typing = Some(match text(value)? {
                    "auto" => Typing::Auto,
                    "keys" => Typing::Keys,
                    "paste" => Typing::Paste,
                    _ => return Err(invalid("strategy overrides: unknown \"typing\" value")),
                });
            }
            "line_break" => {
                patch.line_break = Some(match text(value)? {
                    "enter" => LineBreak::Enter,
                    "shift_enter" => LineBreak::ShiftEnter,
                    _ => return Err(invalid("strategy overrides: unknown \"line_break\" value")),
                });
            }
            "paste_keys" => {
                patch.paste_keys = Some(match text(value)? {
                    "ctrl_v" => PasteKeys::CtrlV,
                    "shift_insert" => PasteKeys::ShiftInsert,
                    _ => return Err(invalid("strategy overrides: unknown \"paste_keys\" value")),
                });
            }
            "paste_threshold" => {
                let n = whole(value, MAX_PASTE_THRESHOLD as u64)?;
                patch.paste_threshold =
                    Some(usize::try_from(n).map_err(|_| invalid("strategy overrides: too big"))?);
            }
            "key_delay_ms" => patch.key_delay_ms = Some(delay(value)?),
            "batch_delay_ms" => patch.batch_delay_ms = Some(delay(value)?),
            "paste_wait_ms" => patch.paste_wait_ms = Some(delay(value)?),
            _ => return Err(invalid("strategy overrides: unknown field in an entry")),
        }
    }
    Ok(patch)
}

fn text(value: &Json) -> Result<&str, PlatformError> {
    match value {
        Json::String(s) => Ok(s),
        _ => Err(PlatformError::Invalid {
            context: "strategy overrides: expected a string value",
        }),
    }
}

/// A whole number from 0 to `max`.
fn whole(value: &Json, max: u64) -> Result<u64, PlatformError> {
    let invalid = PlatformError::Invalid {
        context: "strategy overrides: expected a whole number in range",
    };
    let Json::Number(n) = *value else {
        return Err(invalid);
    };
    if n.fract() != 0.0 || !(0.0..=1e15).contains(&n) {
        return Err(invalid);
    }
    // Whole and within 0..=1e15, so the conversion is exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = n as u64;
    if n > max { Err(invalid) } else { Ok(n) }
}

fn delay(value: &Json) -> Result<u32, PlatformError> {
    let n = whole(value, u64::from(MAX_DELAY_MS))?;
    u32::try_from(n).map_err(|_| PlatformError::Invalid {
        context: "strategy overrides: delay too long",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(table: &StrategyTable, app: &str) -> Strategy {
        table.resolve(Some(app), &InjectionOverrides::default())
    }

    #[test]
    fn unknown_and_absent_applications_get_the_default() {
        let table = StrategyTable::new();
        assert_eq!(resolve(&table, "notepad.exe"), Strategy::DEFAULT);
        assert_eq!(
            table.resolve(None, &InjectionOverrides::default()),
            Strategy::DEFAULT
        );
        assert_eq!(Strategy::DEFAULT.paste_threshold, 200);
        assert_eq!(Strategy::DEFAULT.paste_wait_ms, 150);
        assert_eq!(Strategy::DEFAULT.typing, Typing::Auto);
        assert_eq!(Strategy::DEFAULT.line_break, LineBreak::Enter);
    }

    #[test]
    fn names_match_without_case_extension_or_directory() {
        for name in [
            "WindowsTerminal.exe",
            "windowsterminal",
            "WINDOWSTERMINAL.EXE",
            r"C:\Program Files\WindowsApps\WindowsTerminal.exe",
            "  windowsterminal.exe ",
        ] {
            assert_eq!(normalize_app(name), "windowsterminal", "{name}");
            assert_eq!(
                resolve(&StrategyTable::new(), name).typing,
                Typing::Keys,
                "{name}"
            );
        }
        // Only the whole name matches.
        assert_eq!(
            resolve(&StrategyTable::new(), "cmdlet-helper.exe"),
            Strategy::DEFAULT
        );
        assert_eq!(normalize_app("app.exe.exe"), "app.exe");
    }

    #[test]
    fn the_built_in_table_is_well_formed() {
        let mut seen = Vec::new();
        for entry in BUILT_IN {
            assert!(!entry.why.is_empty());
            assert_ne!(entry.patch, Patch::EMPTY);
            for app in entry.apps {
                assert_eq!(normalize_app(app), *app, "{app} is not normalised");
                assert!(!seen.contains(app), "{app} is listed twice");
                seen.push(app);
                assert!(built_in(app).is_some());
            }
        }
    }

    #[test]
    fn built_in_entries() {
        let table = StrategyTable::new();
        for terminal in ["cmd.exe", "pwsh.exe", "WindowsTerminal.exe", "putty.exe"] {
            let s = resolve(&table, terminal);
            assert_eq!(s.typing, Typing::Keys, "{terminal}");
            assert!(!s.pastes_text(100_000));
            assert!(s.types_pastes());
        }
        let rdp = resolve(&table, "mstsc.exe");
        assert_eq!(rdp.typing, Typing::Keys);
        assert!(rdp.batch_delay_ms > 0 && rdp.key_delay_ms > 0);
        for chat in ["slack.exe", "ms-teams.exe", "Discord.exe"] {
            let s = resolve(&table, chat);
            assert_eq!(s.line_break, LineBreak::ShiftEnter, "{chat}");
            assert_eq!(s.typing, Typing::Auto, "{chat}");
        }
    }

    #[test]
    fn typing_or_paste_by_length() {
        let s = Strategy::DEFAULT;
        assert!(!s.pastes_text(0));
        assert!(!s.pastes_text(200));
        assert!(s.pastes_text(201));
        assert!(!s.types_pastes());
        let always = Strategy {
            typing: Typing::Paste,
            ..s
        };
        assert!(always.pastes_text(1) && always.pastes_text(0));
        assert!(!always.types_pastes());
    }

    #[test]
    fn user_overrides_layer_over_the_table() {
        let table = StrategyTable::from_settings(
            r#"{
                "default": { "paste_threshold": 50, "typing": "paste" },
                "apps": {
                    "Notepad.EXE": { "line_break": "shift_enter", "paste_wait_ms": 300 },
                    "cmd": { "key_delay_ms": 4 },
                    "slack.exe": { "line_break": "enter", "paste_keys": "shift_insert" }
                }
            }"#,
        )
        .expect("valid overrides");

        // The user's default applies to applications without entries ...
        let other = resolve(&table, "wordpad.exe");
        assert_eq!(other.paste_threshold, 50);
        assert_eq!(other.typing, Typing::Paste);
        // ... and the user's entry on top of it.
        let notepad = resolve(&table, "notepad.exe");
        assert_eq!(notepad.line_break, LineBreak::ShiftEnter);
        assert_eq!(notepad.paste_wait_ms, 300);
        assert_eq!(notepad.paste_threshold, 50);
        // A built-in entry beats the user's general default: a terminal still types.
        let cmd = resolve(&table, "cmd.exe");
        assert_eq!(cmd.typing, Typing::Keys);
        assert_eq!(cmd.key_delay_ms, 4);
        // The user's entry for an application beats the built-in one.
        let slack = resolve(&table, "slack.exe");
        assert_eq!(slack.line_break, LineBreak::Enter);
        assert_eq!(slack.paste_keys, PasteKeys::ShiftInsert);
    }

    #[test]
    fn request_overrides_come_last() {
        let table = StrategyTable::from_settings(r#"{"apps": {"cmd": {"key_delay_ms": 4}}}"#)
            .expect("valid");
        let mut overrides = InjectionOverrides::default();
        overrides.text_method = Some(TextMethod::Paste);
        overrides.key_delay_ms = Some(9);
        let s = table.resolve(Some("cmd.exe"), &overrides);
        assert_eq!(s.typing, Typing::Paste);
        assert_eq!(s.key_delay_ms, 9);
        overrides.text_method = Some(TextMethod::Keys);
        overrides.key_delay_ms = Some(u32::MAX);
        let s = table.resolve(None, &overrides);
        assert_eq!(s.typing, Typing::Keys);
        assert_eq!(s.key_delay_ms, MAX_DELAY_MS);
    }

    #[test]
    fn blank_settings_mean_no_overrides() {
        assert_eq!(StrategyTable::from_settings(""), Ok(StrategyTable::new()));
        assert_eq!(
            StrategyTable::from_settings("  \n"),
            Ok(StrategyTable::new())
        );
        assert_eq!(StrategyTable::from_settings("{}"), Ok(StrategyTable::new()));
    }

    #[test]
    fn bad_overrides_are_reported() {
        for bad in [
            "[]",
            "nonsense",
            r#"{"defaults": {}}"#,
            r#"{"apps": []}"#,
            r#"{"apps": {"": {}}}"#,
            r#"{"apps": {"a.exe": {}, "A": {}}}"#,
            r#"{"apps": {"a": 1}}"#,
            r#"{"apps": {"a": {"typing": "fast"}}}"#,
            r#"{"apps": {"a": {"typing": 1}}}"#,
            r#"{"apps": {"a": {"line_break": "ctrl_enter"}}}"#,
            r#"{"apps": {"a": {"paste_keys": "ctrl_shift_v"}}}"#,
            r#"{"apps": {"a": {"paste_wait_ms": -1}}}"#,
            r#"{"apps": {"a": {"paste_wait_ms": 1.5}}}"#,
            r#"{"apps": {"a": {"paste_wait_ms": 10001}}}"#,
            r#"{"apps": {"a": {"paste_threshold": 1000001}}}"#,
            r#"{"apps": {"a": {"paste_threshold": "200"}}}"#,
            r#"{"apps": {"a": {"restore": false}}}"#,
            r#"{"default": {"typo": 1}}"#,
        ] {
            assert!(
                matches!(
                    StrategyTable::from_settings(bad),
                    Err(PlatformError::Invalid { .. })
                ),
                "accepted {bad}"
            );
        }
    }

    #[test]
    fn errors_never_quote_the_settings() {
        let err = StrategyTable::from_settings(r#"{"apps": {"private-app": {"x": 1}}}"#)
            .expect_err("unknown field");
        assert!(!err.to_string().contains("private"));
    }

    #[test]
    fn largest_values_are_accepted() {
        let table = StrategyTable::from_settings(
            r#"{"default": {"paste_threshold": 1000000, "key_delay_ms": 10000,
                "batch_delay_ms": 0, "paste_wait_ms": 10000, "typing": "auto"}}"#,
        )
        .expect("valid");
        let s = table.resolve(None, &InjectionOverrides::default());
        assert_eq!(s.paste_threshold, MAX_PASTE_THRESHOLD);
        assert_eq!(s.key_delay_ms, MAX_DELAY_MS);
        assert_eq!(s.paste_wait_ms, MAX_DELAY_MS);
    }
}
