//! The settings and diagnostics side of the contract (task 3.9): `GetSettings`, `PutSetting`,
//! the app exclusions, the diagnostics ring and the dry run behind "Test an abbreviation".
//!
//! The front end never speaks IPC (architecture §6.2). It calls the `settings_*` and
//! `diagnostics_*` commands in `app/settings_commands.rs`, which call the functions here; the
//! types are mirrored by hand in `apps/desktop/src/library/settings/api.ts`.
//!
//! | Command | Request sent | Response read |
//! | --- | --- | --- |
//! | `settings_get` | `GetSettings` | `Settings` |
//! | `settings_put` | `PutSetting` | `Done` |
//! | `settings_list_exclusions` | `ListExclusions` | `Exclusions` |
//! | `settings_put_exclusion` | `PutExclusion` | `Exclusions` (the row written, or the rows removed) |
//! | `diagnostics_recent` | `RecentDiagnostics` | `Diagnostics` |
//! | `diagnostics_test` | `DryRun` (once or twice), `GetSnippet` | `DryRun`, `Snippet` |
//!
//! # Settings keys
//!
//! [`SERVICE_KEYS`] are the keys the service reads; [`UI_KEYS`] are the interface's own, under
//! the prefix `ui.` (the service stores unknown keys as they are, specification §11.1). The
//! test `tests/settings.rs` fails when a key has no persistence test, and when the service's
//! source reads a key this list does not name.
//!
//! # Privacy
//!
//! A diagnostic carries a code, the snippet it named, the app it happened in and a time: never
//! typed text (architecture §8 rule 4). What is typed into "Test an abbreviation" travels only
//! in the one `DryRun` frame (as a `Secret`) and is dropped when the call returns; the answer
//! names a snippet and says what would happen, nothing more. These functions do not log.

use std::collections::BTreeMap;

use iamb_ipc::{
    DiagnosticDetail, RequestBody, ResponseBody, Secret, WireAction, WireDiagnostic, WireExclusion,
};
use serde::{Deserialize, Serialize};

use crate::bridge::{Bridge, command};
use crate::contract::{CommandError, Unexpected};
use crate::library;

/// The requests the settings functions send, by wire name.
pub const REQUESTS: &[&str] = &[
    "get_settings",
    "put_setting",
    "list_exclusions",
    "put_exclusion",
    "recent_diagnostics",
    "dry_run",
];

/// The settings the service reads (architecture §7, specification §11.1).
pub const SERVICE_KEYS: &[&str] = &[
    "delimiters",
    "date_format",
    "time_format",
    "locale",
    "palette_hotkey",
    "reopen_form_hotkey",
    "pause_hotkey",
    "injection_overrides",
];

/// The interface's own settings (specification §11.1).
pub const UI_KEYS: &[&str] = &[
    "ui.onboarding_done",
    "ui.theme",
    "ui.tray_icon",
    "ui.expansion_sound",
    "ui.new_trigger",
    "ui.new_case_mode",
    "ui.crash_reports",
];

/// Every key, the service's first.
pub fn keys() -> impl Iterator<Item = &'static str> {
    SERVICE_KEYS.iter().chain(UI_KEYS).copied()
}

/// The setting that says whether the tray or menu-bar icon is shown (`off` hides it).
pub const TRAY_KEY: &str = "ui.tray_icon";

type Res<T> = Result<T, CommandError>;

fn call(bridge: &Bridge, body: RequestBody) -> Res<ResponseBody> {
    bridge.call(body).map_err(command)
}

// ---------------------------------------------------------------------------------------------
// What the front end receives
// ---------------------------------------------------------------------------------------------

/// An app where Iamb does not expand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExclusionView {
    /// The id in the form the focus inspector reports it (normalised by the service).
    pub app_id: String,
    /// The name shown.
    pub display_name: String,
    /// When it was added (UTC milliseconds).
    pub created_at: i64,
}

impl From<&WireExclusion> for ExclusionView {
    fn from(e: &WireExclusion) -> Self {
        Self {
            app_id: e.app_id.clone(),
            display_name: e.display_name.clone(),
            created_at: e.created_at.0,
        }
    }
}

/// One entry of the diagnostics ring: codes and ids only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticView {
    /// The reason code.
    pub code: String,
    /// The snippet it named, if any.
    pub snippet: Option<String>,
    /// The app, if any.
    pub app: Option<String>,
    /// The technical detail of the reason, if any (for an unknown focus: why the field could
    /// not be judged, as a code).
    pub detail: Option<String>,
    /// When it was recorded (UTC milliseconds).
    pub at: i64,
}

impl From<&WireDiagnostic> for DiagnosticView {
    fn from(d: &WireDiagnostic) -> Self {
        Self {
            code: d.code.clone(),
            snippet: d.snippet.map(|id| id.to_string()),
            app: d.app.clone(),
            detail: d.detail.and_then(DiagnosticDetail::code).map(str::to_owned),
            at: d.at.0,
        }
    }
}

/// What "Test an abbreviation" says would happen (specification §4.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestView {
    /// `expands`, `not_yet`, `no_snippet` or `broken`.
    pub outcome: String,
    /// The snippet, for every outcome but `no_snippet`.
    pub snippet: Option<String>,
    /// Its label (library content).
    pub label: Option<String>,
    /// Its abbreviation.
    pub abbreviation: Option<String>,
    /// The template error code, for `broken`.
    pub error: Option<String>,
}

impl TestView {
    fn none() -> Self {
        Self {
            outcome: "no_snippet".to_owned(),
            snippet: None,
            label: None,
            abbreviation: None,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------------------------

/// `GetSettings`.
#[must_use]
pub const fn get_settings() -> RequestBody {
    RequestBody::GetSettings {}
}

/// `PutSetting`; no value removes the setting.
#[must_use]
pub fn put_setting_request(key: &str, value: Option<&str>) -> RequestBody {
    RequestBody::PutSetting {
        key: key.to_owned(),
        value: value.map(ToOwned::to_owned),
    }
}

/// `ListExclusions`.
#[must_use]
pub const fn list_exclusions() -> RequestBody {
    RequestBody::ListExclusions {}
}

/// `PutExclusion`.
#[must_use]
pub fn put_exclusion_request(app_id: &str, display_name: &str, excluded: bool) -> RequestBody {
    RequestBody::PutExclusion {
        app_id: app_id.to_owned(),
        display_name: display_name.to_owned(),
        excluded,
    }
}

/// `RecentDiagnostics`.
#[must_use]
pub const fn recent_diagnostics() -> RequestBody {
    RequestBody::RecentDiagnostics {}
}

/// `DryRun` of `typed` with the service's clock and an empty clipboard.
#[must_use]
pub fn dry_run_request(typed: &str) -> RequestBody {
    RequestBody::DryRun {
        typed: Secret::new(typed.to_owned()),
        now: None,
        clipboard: None,
        values: None,
    }
}

// ---------------------------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------------------------

/// The settings in a `Settings` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn settings_from(response: ResponseBody) -> Result<BTreeMap<String, String>, Unexpected> {
    match response {
        ResponseBody::Settings { settings } => Ok(settings),
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The exclusions in an `Exclusions` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn exclusions_from(response: ResponseBody) -> Result<Vec<ExclusionView>, Unexpected> {
    match response {
        ResponseBody::Exclusions { exclusions } => {
            Ok(exclusions.iter().map(ExclusionView::from).collect())
        }
        other => Err(Unexpected { got: other.name() }),
    }
}

/// The entries in a `Diagnostics` response, newest first.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn diagnostics_from(response: ResponseBody) -> Result<Vec<DiagnosticView>, Unexpected> {
    match response {
        ResponseBody::Diagnostics { entries } => {
            Ok(entries.iter().rev().map(DiagnosticView::from).collect())
        }
        other => Err(Unexpected { got: other.name() }),
    }
}

/// What a dry run found: the snippet that would expand, or the snippet with a template error.
enum Found {
    Expands(String),
    Broken(String, String),
    Nothing,
}

fn found_in(response: ResponseBody) -> Result<Found, Unexpected> {
    let ResponseBody::DryRun { actions, hints } = response else {
        return Err(Unexpected {
            got: response.name(),
        });
    };
    for action in &actions {
        match action {
            WireAction::Expanded { snippet, .. } | WireAction::ShowForm { snippet, .. } => {
                return Ok(Found::Expands(snippet.to_string()));
            }
            _ => {}
        }
    }
    let broken = actions
        .iter()
        .filter_map(|a| match a {
            WireAction::Diagnostic { diagnostic } => Some(diagnostic),
            _ => None,
        })
        .chain(hints.iter())
        .find(|d| d.code.starts_with("E_") && d.snippet.is_some());
    Ok(match broken {
        Some(d) => Found::Broken(
            d.snippet.map(|id| id.to_string()).unwrap_or_default(),
            d.code.clone(),
        ),
        None => Found::Nothing,
    })
}

// ---------------------------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------------------------

/// Every setting.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn settings(bridge: &Bridge) -> Res<BTreeMap<String, String>> {
    Ok(settings_from(call(bridge, get_settings())?)?)
}

/// Sets a setting, or removes it when `value` is `None`.
///
/// # Errors
///
/// `invalid` for a value the service cannot use (a date format it does not know, a chord that
/// does not parse, injection settings it refuses), `conflict` for a chord the system refuses,
/// the call's other errors, or an unexpected response.
pub fn put_setting(bridge: &Bridge, key: &str, value: Option<&str>) -> Res<()> {
    match call(bridge, put_setting_request(key, value))? {
        ResponseBody::Done {} => Ok(()),
        other => Err(Unexpected { got: other.name() }.into()),
    }
}

/// The apps where Iamb does not expand.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn exclusions(bridge: &Bridge) -> Res<Vec<ExclusionView>> {
    Ok(exclusions_from(call(bridge, list_exclusions())?)?)
}

/// Adds or removes an exclusion. The answer is the row written (spelled the way the service
/// stores it) or the rows removed.
///
/// # Errors
///
/// `invalid` for an id no application of this system can have, the call's other errors, or an
/// unexpected response.
pub fn put_exclusion(
    bridge: &Bridge,
    app_id: &str,
    display_name: &str,
    excluded: bool,
) -> Res<Vec<ExclusionView>> {
    Ok(exclusions_from(call(
        bridge,
        put_exclusion_request(app_id, display_name, excluded),
    )?)?)
}

/// The diagnostics ring, newest first.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn diagnostics(bridge: &Bridge) -> Res<Vec<DiagnosticView>> {
    Ok(diagnostics_from(call(bridge, recent_diagnostics())?)?)
}

fn described(bridge: &Bridge, outcome: &str, id: &str, error: Option<String>) -> TestView {
    // A snippet deleted since the dry run read its snapshot is still an answer.
    let (label, abbreviation) = match library::snippet(bridge, id) {
        Ok(s) => (Some(s.label), Some(s.abbreviation)),
        Err(_) => (None, None),
    };
    TestView {
        outcome: outcome.to_owned(),
        snippet: Some(id.to_owned()),
        label,
        abbreviation,
        error,
    }
}

/// Says what typing `typed` in an app would do, without typing anywhere.
///
/// The text is fed to a fresh engine (`DryRun`). When nothing expands it is fed once more with
/// a space after it: a snippet that expands then is one that waits for the end of the word.
///
/// # Errors
///
/// The call's errors, or an unexpected response.
pub fn test_abbreviation(bridge: &Bridge, typed: &str) -> Res<TestView> {
    match found_in(call(bridge, dry_run_request(typed))?)? {
        Found::Expands(id) => return Ok(described(bridge, "expands", &id, None)),
        Found::Broken(id, code) => return Ok(described(bridge, "broken", &id, Some(code))),
        Found::Nothing => {}
    }
    let spaced = format!("{typed} ");
    match found_in(call(bridge, dry_run_request(&spaced))?)? {
        Found::Expands(id) => Ok(described(bridge, "not_yet", &id, None)),
        Found::Broken(id, code) => Ok(described(bridge, "broken", &id, Some(code))),
        Found::Nothing => Ok(TestView::none()),
    }
}

// ---------------------------------------------------------------------------------------------
// About
// ---------------------------------------------------------------------------------------------

/// What Settings → About shows besides the licence and the source (specification §4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutView {
    /// This build's version.
    pub version: String,
    /// The folder of the log files (the UI host's and the service's, architecture §3.3).
    pub log_folder: String,
}

/// The systems [`folder_opener`] knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    /// macOS: Finder, through `/usr/bin/open`.
    Mac,
    /// Windows: File Explorer, `explorer.exe` in the Windows directory.
    Windows,
    /// Linux (development only): the desktop's default, `xdg-open`.
    Linux,
}

impl System {
    /// The system this build runs on.
    #[must_use]
    pub const fn this() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// The program and arguments that show `folder` in the system's file manager. The folder is one
/// argument, never a shell line, and the program is named by its full path on macOS and Windows
/// (the beta-gap review's finding 7): `/usr/bin/open`, and `explorer.exe` in `windows_dir` (the
/// Windows directory; `None` when it is not known). On Linux, a development system, `xdg-open` is
/// looked up as the desktop sets it up.
#[must_use]
pub fn folder_opener(
    system: System,
    windows_dir: Option<&std::path::Path>,
    folder: &std::path::Path,
) -> Option<(std::path::PathBuf, Vec<std::ffi::OsString>)> {
    let program = match system {
        System::Mac => std::path::PathBuf::from("/usr/bin/open"),
        System::Windows => windows_dir
            .filter(|d| d.is_absolute())?
            .join("explorer.exe"),
        System::Linux => std::path::PathBuf::from("xdg-open"),
    };
    Some((program, vec![folder.as_os_str().to_owned()]))
}

/// The Windows directory, on Windows.
fn windows_dir() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        iamb_platform_windows::windows_directory()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Shows `folder` in the file manager ("Open the log folder"), creating it if it is missing.
///
/// # Errors
///
/// `unavailable` when the folder cannot be made or the file manager cannot be started.
pub fn open_folder(folder: &std::path::Path) -> Res<()> {
    crate::paths::create_private_dir(folder)
        .map_err(|e| CommandError::new("unavailable", format!("the log folder: {e}")))?;
    let (program, args) = folder_opener(System::this(), windows_dir().as_deref(), folder)
        .ok_or_else(|| CommandError::new("unavailable", "Iamb could not open the folder"))?;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| CommandError::new("unavailable", "Iamb could not open the folder"))?;
    // Reaped on a thread of its own: the file manager may stay open.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod about_tests {
    use super::*;

    #[test]
    fn the_folder_is_one_argument_to_the_file_manager() {
        use std::path::Path;
        let folder = Path::new("/data/iamb/logs; rm -rf x");
        let windows = if cfg!(windows) {
            Path::new(r"C:\Windows")
        } else {
            Path::new("/windows")
        };
        for system in [System::Mac, System::Windows, System::Linux] {
            let (_, args) = folder_opener(system, Some(windows), folder).expect("an opener");
            assert_eq!(args, [folder.as_os_str()]);
        }
    }

    #[test]
    fn the_file_manager_is_named_by_its_full_path() {
        use std::path::Path;
        let folder = Path::new("/data/iamb/logs");
        let program =
            |system, dir: Option<&Path>| folder_opener(system, dir, folder).map(|(p, _)| p);
        assert_eq!(
            program(System::Mac, None),
            Some(Path::new("/usr/bin/open").to_path_buf())
        );
        // On Windows the Windows directory is asked, never `PATH`; without it, nothing starts.
        let windows = if cfg!(windows) {
            Path::new(r"C:\Windows")
        } else {
            Path::new("/windows")
        };
        assert_eq!(
            program(System::Windows, Some(windows)),
            Some(windows.join("explorer.exe"))
        );
        assert_eq!(program(System::Windows, None), None);
        assert_eq!(program(System::Windows, Some(Path::new("Windows"))), None);
        assert_eq!(
            program(System::Linux, None),
            Some(Path::new("xdg-open").to_path_buf())
        );
    }

    #[test]
    fn the_about_view_is_camel_case() {
        let view = AboutView {
            version: "0.1.0".into(),
            log_folder: "/data/iamb/logs".into(),
        };
        let json = serde_json::to_value(&view).expect("json");
        assert_eq!(json["version"], "0.1.0");
        assert_eq!(json["logFolder"], "/data/iamb/logs");
    }
}
