//! Settings and diagnostics against a real worker on the mock platform, reached through the
//! bridge as the Settings view's commands reach it (task 3.9).
//!
//! - One test per settings key proves it persists: set through the bridge, read back, and
//!   still there after the worker restarts over the same data root (`persists_*`).
//! - `every_settings_key_has_a_persistence_test` is the KPI ("every setting covered by a
//!   test"): it fails when a key the UI host or the service knows has no `persists_*` test
//!   here, when the service's source reads a key `settings::keys` does not name, and when the
//!   front end's list of keys differs from the Rust one.
//! - The exclusions, the hotkey's errors, the injection overrides, the diagnostics log and the
//!   dry run behind "Test an abbreviation" run against the worker too. No test puts typed
//!   text into a log or a diagnostic.

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use common::{Endpoint, Service};
use iamb_core::template::KeyName;
use iamb_desktop::contract::UiEvent;
use iamb_desktop::settings::{self, keys};
use iamb_platform::Chord;
use iamb_platform::mock::MockWindow;

fn app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn code<T: std::fmt::Debug>(r: Result<T, iamb_desktop::contract::CommandError>) -> String {
    r.expect_err("an error").code
}

/// Sets `key` to `value` through a bridge, reads it back, restarts the worker over the same data
/// root and reads it again; then removes it and sees it gone, also after a restart.
fn persists(key: &str, value: &str) {
    let mut service = Service::start(Endpoint::Mock);
    {
        let (bridge, seen) = service.bridge();
        seen.wait_connected();
        let before = settings::settings(&bridge).expect("settings");
        assert_eq!(before.get(key), None, "{key} starts unset");
        seen.clear();
        settings::put_setting(&bridge, key, Some(value)).expect("put");
        seen.wait_event(|e| matches!(e, UiEvent::SettingsChanged {}));
        assert_eq!(
            settings::settings(&bridge)
                .expect("settings")
                .get(key)
                .map(String::as_str),
            Some(value),
            "{key} reads back"
        );
        bridge.stop();
    }
    service.restart();
    {
        let (bridge, seen) = service.bridge();
        seen.wait_connected();
        assert_eq!(
            settings::settings(&bridge)
                .expect("settings")
                .get(key)
                .map(String::as_str),
            Some(value),
            "{key} survives a restart of the worker"
        );
        // Removing it (the control back to its default) is stored as well.
        settings::put_setting(&bridge, key, None).expect("remove");
        assert_eq!(
            settings::settings(&bridge).expect("settings").get(key),
            None
        );
        bridge.stop();
    }
    service.restart();
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    assert_eq!(
        settings::settings(&bridge).expect("settings").get(key),
        None,
        "{key} stays removed after a restart"
    );
    bridge.stop();
}

#[test]
fn persists_delimiters() {
    // Enter and Tab added to the default set (ADR-0026).
    persists("delimiters", " .,;:!?)]}\"'\n\t");
}

#[test]
fn persists_date_format() {
    persists("date_format", "%d/%m/%Y");
}

#[test]
fn persists_time_format() {
    persists("time_format", "%-I:%M %p");
}

#[test]
fn persists_locale() {
    persists("locale", "de");
}

#[test]
fn persists_palette_hotkey() {
    persists("palette_hotkey", "Ctrl+Shift+K");
}

#[test]
fn persists_reopen_form_hotkey() {
    persists("reopen_form_hotkey", "Ctrl+Shift+F");
}

#[test]
fn persists_pause_hotkey() {
    persists("pause_hotkey", "Ctrl+Shift+P");
}

#[test]
fn persists_injection_overrides() {
    persists(
        "injection_overrides",
        r#"{"apps":{"chat.exe":{"method":"paste","line_break":"shift+enter","key_delay_ms":5}}}"#,
    );
}

#[test]
fn persists_ui_onboarding_done() {
    persists("ui.onboarding_done", "1");
}

#[test]
fn persists_ui_theme() {
    persists("ui.theme", "dark");
}

#[test]
fn persists_ui_tray_icon() {
    persists("ui.tray_icon", "off");
}

#[test]
fn persists_ui_new_trigger() {
    persists("ui.new_trigger", "immediate");
}

#[test]
fn persists_ui_new_case_mode() {
    persists("ui.new_case_mode", "adaptive");
}

#[test]
fn persists_ui_crash_reports() {
    persists("ui.crash_reports", "on");
}

/// The keys the service's own source reads: `map.get("…")` and `settings.get("…")` of the
/// engine's settings and the worker's start, the worker's `HOTKEY_SETTINGS` table, and the `…_KEY`
/// constant of the platform crate.
fn keys_the_service_reads() -> BTreeSet<String> {
    let root = app_dir().join("../../..");
    let mut found = BTreeSet::new();
    let mut take = |text: &str, marker: &str, close: &str| {
        let mut rest = text;
        while let Some(at) = rest.find(marker) {
            rest = &rest[at + marker.len()..];
            if let Some(end) = rest.find(close) {
                let key = &rest[..end];
                if !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                    found.insert(key.to_owned());
                }
            }
        }
    };
    for file in [
        "crates/iamb-service/src/library.rs",
        "crates/iamb-service/src/worker/mod.rs",
    ] {
        let text = read(&root.join(file));
        take(&text, "map.get(\"", "\")");
        take(&text, "settings.get(\"", "\")");
    }
    // The shortcuts' settings: the keys of the worker's `HOTKEY_SETTINGS` table.
    let worker = read(&root.join("crates/iamb-service/src/worker/mod.rs"));
    let table = &worker[worker
        .find("HOTKEY_SETTINGS: [")
        .expect("HOTKEY_SETTINGS not found")..];
    let table = &table[..table.find("];").expect("HOTKEY_SETTINGS end")];
    take(table, "(\"", "\"");
    take(
        &read(&root.join("crates/iamb-platform/src/injection_settings.rs")),
        "INJECTION_SETTINGS_KEY: &str = \"",
        "\";",
    );
    found
}

/// The quoted names of `export const <name> = [ … ]` blocks of `api.ts`.
fn ts_keys(text: &str, name: &str) -> Vec<String> {
    let start = text
        .find(&format!("export const {name} = ["))
        .unwrap_or_else(|| panic!("{name} not found"));
    let rest = &text[start..];
    let open = rest.find('[').expect("list");
    let close = rest[open..].find(']').expect("list end") + open;
    rest[open + 1..close]
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

#[test]
fn every_settings_key_has_a_persistence_test() {
    let all: BTreeSet<String> = keys().map(str::to_owned).collect();

    // Every key the service reads is named by `settings::keys` (and so by the UI host).
    let read_by_service = keys_the_service_reads();
    assert!(
        read_by_service.len() >= 6,
        "the scan found too little: {read_by_service:?}"
    );
    for key in &read_by_service {
        assert!(
            all.contains(key),
            "the service reads {key}, which settings::keys omits"
        );
    }

    // Every key has a test here...
    let this_file = read(&app_dir().join("tests/settings.rs"));
    for key in &all {
        let name = format!("fn persists_{}()", key.replace('.', "_"));
        assert!(
            this_file.contains(&name),
            "no persistence test for {key}: add {name}"
        );
        let squeezed: String = this_file.split_whitespace().collect();
        assert!(
            squeezed.contains(&format!("persists(\"{key}\",")),
            "{name} does not test {key}"
        );
    }
    // ...and the front end lists the same keys, with a case for each in its own tests.
    let api = read(&app_dir().join("../src/library/settings/api.ts"));
    let mut front: BTreeSet<String> = ts_keys(&api, "SERVICE_KEYS").into_iter().collect();
    front.extend(ts_keys(&api, "UI_KEYS"));
    assert_eq!(front, all, "api.ts lists other keys than settings::keys");
    let view_tests = read(&app_dir().join("../src/library/settings/SettingsView.test.tsx"));
    for key in &all {
        assert!(
            view_tests.contains(&format!("\"{key}\":")) || view_tests.contains(&format!("{key}:")),
            "SettingsView.test.tsx has no case for {key}"
        );
    }
}

#[test]
fn values_the_service_cannot_use_are_refused_and_not_stored() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();

    // A date code Iamb does not know.
    assert_eq!(
        code(settings::put_setting(&bridge, "date_format", Some("%Q"))),
        "invalid"
    );
    assert_eq!(
        code(settings::put_setting(&bridge, "time_format", Some(""))),
        "invalid"
    );
    // A chord that does not parse, and one that needs a modifier.
    assert_eq!(
        code(settings::put_setting(
            &bridge,
            "palette_hotkey",
            Some("Ctrl")
        )),
        "invalid"
    );
    assert_eq!(
        code(settings::put_setting(&bridge, "palette_hotkey", Some("K"))),
        "invalid"
    );
    // Injection settings the injector refuses: a typo is reported, not ignored.
    assert_eq!(
        code(settings::put_setting(
            &bridge,
            "injection_overrides",
            Some(r#"{"apps":{"a.exe":{"methd":"keys"}}}"#)
        )),
        "invalid"
    );
    assert_eq!(
        code(settings::put_setting(&bridge, "", Some("x"))),
        "invalid"
    );
    let stored = settings::settings(&bridge).expect("settings");
    for key in [
        "date_format",
        "time_format",
        "palette_hotkey",
        "injection_overrides",
    ] {
        assert_eq!(stored.get(key), None, "{key} was not stored");
    }
}

#[test]
fn a_chord_another_app_holds_is_a_conflict_and_the_old_one_stays() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();

    settings::put_setting(&bridge, "palette_hotkey", Some("Ctrl+Shift+Space")).expect("registered");
    service
        .os
        .reserve_chord(Chord::parse("Ctrl+Shift+J").expect("chord"));
    assert_eq!(
        code(settings::put_setting(
            &bridge,
            "palette_hotkey",
            Some("Ctrl+Shift+J")
        )),
        "conflict"
    );
    // The setting is the old chord, and it is still registered.
    assert_eq!(
        settings::settings(&bridge)
            .expect("settings")
            .get("palette_hotkey")
            .map(String::as_str),
        Some("Ctrl+Shift+Space")
    );
    let registered: Vec<String> = service
        .os
        .registered_hotkeys()
        .iter()
        .map(|(_, c)| c.to_string())
        .collect();
    assert_eq!(registered, ["Ctrl+Shift+Space"]);
    // Clearing the setting unregisters it.
    settings::put_setting(&bridge, "palette_hotkey", None).expect("cleared");
    assert!(service.os.registered_hotkeys().is_empty());
}

#[test]
fn exclusions_are_added_removed_normalised_and_kept() {
    let mut service = Service::start(Endpoint::Mock);
    {
        let (bridge, seen) = service.bridge();
        seen.wait_connected();
        assert!(settings::exclusions(&bridge).expect("list").is_empty());
        let added =
            settings::put_exclusion(&bridge, "  MyBank.EXE ", "My bank", true).expect("add");
        // The service answers with the row as it stores it: the form the focus inspector reports.
        assert_eq!(added.len(), 1);
        let stored_id = added[0].app_id.clone();
        assert_eq!(stored_id, stored_id.to_lowercase());
        assert_eq!(added[0].display_name, "My bank");
        assert!(added[0].created_at > 0);
        // An id no app can have is refused.
        assert_eq!(
            code(settings::put_exclusion(&bridge, "   ", "x", true)),
            "invalid"
        );
        assert_eq!(settings::exclusions(&bridge).expect("list"), added);
        bridge.stop();
    }
    service.restart();
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let kept = settings::exclusions(&bridge).expect("list");
    assert_eq!(kept.len(), 1, "the exclusion survives a restart");
    let removed = settings::put_exclusion(&bridge, &kept[0].app_id, "", false).expect("remove");
    assert_eq!(removed, kept);
    assert!(settings::exclusions(&bridge).expect("list").is_empty());
    // Removing what is not there removes nothing.
    assert!(
        settings::put_exclusion(&bridge, "nothing.exe", "", false)
            .expect("remove")
            .is_empty()
    );
}

#[test]
fn an_excluded_app_is_left_alone_and_the_log_says_so_without_the_text() {
    const CANARY: &str = "zqxsettingscanary";
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();

    // The mock editor reports this application id.
    settings::put_exclusion(&bridge, "org.example.editor", "Example editor", true)
        .expect("exclude");
    seen.wait_event(|e| matches!(e, UiEvent::SettingsChanged {}));
    // The verdict is read when the focus moves into the app: leave it and come back.
    let other = service
        .os
        .open_window(MockWindow::new("org.example.other", 200));
    service.os.focus(other).expect("window is open");
    service.os.focus(service.editor).expect("window is open");
    service.worker().settle();
    service.type_text(&format!("{CANARY} brb "));
    assert!(
        service.text().contains("brb"),
        "nothing expanded in an excluded app"
    );
    let event = seen.wait_event(
        |e| matches!(e, UiEvent::DiagnosticAdded { code, .. } if code == "suspended_excluded_app"),
    );
    let UiEvent::DiagnosticAdded {
        app, at, snippet, ..
    } = event
    else {
        panic!("a diagnostic");
    };
    assert!(
        app.as_deref().is_some_and(|a| a.contains("editor")),
        "the app is named: {app:?}"
    );
    assert!(at > 0);
    assert_eq!(snippet, None, "no snippet is named while Iamb is held back");

    let entries = settings::diagnostics(&bridge).expect("diagnostics");
    assert!(entries.iter().any(|d| d.code == "suspended_excluded_app"));
    // Newest first, and no entry holds anything typed.
    assert!(entries.windows(2).all(|w| w[0].at >= w[1].at));
    assert!(!format!("{entries:?}").contains(CANARY));
    assert!(!service.worker_log().contains(CANARY));
    assert!(
        !fs::read_to_string(service.dirs().logs().join("desktop.log"))
            .unwrap_or_default()
            .contains(CANARY)
    );
}

#[test]
fn enter_after_an_abbreviation_leaves_the_diagnostic_not_a_delimiter() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();

    service.type_text("brb");
    service.os.press(KeyName::Enter);
    service.worker().settle();
    assert!(
        !service.text().contains("be right back"),
        "Enter is not a delimiter"
    );
    let event = seen.wait_event(
        |e| matches!(e, UiEvent::DiagnosticAdded { code, .. } if code == "not_a_delimiter"),
    );
    let UiEvent::DiagnosticAdded { snippet, at, .. } = event else {
        panic!("a diagnostic");
    };
    assert_eq!(
        snippet,
        Some(common::id(1).to_string()),
        "it names the snippet"
    );
    assert!(at > 0);
    let entries = settings::diagnostics(&bridge).expect("diagnostics");
    assert_eq!(
        entries.first().map(|d| d.code.as_str()),
        Some("not_a_delimiter")
    );
}

#[test]
fn test_an_abbreviation_says_what_would_happen() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();

    let expands = settings::test_abbreviation(&bridge, "brb ").expect("test");
    assert_eq!(expands.outcome, "expands");
    assert_eq!(expands.abbreviation.as_deref(), Some("brb"));
    assert_eq!(expands.label.as_deref(), Some("label brb"));

    // Without the space: not yet.
    let waits = settings::test_abbreviation(&bridge, "brb").expect("test");
    assert_eq!(waits.outcome, "not_yet");
    assert_eq!(waits.snippet, expands.snippet);

    let none = settings::test_abbreviation(&bridge, "zzz").expect("test");
    assert_eq!(none.outcome, "no_snippet");
    assert_eq!(none.snippet, None);

    // A snippet with an error in its template.
    let folder = iamb_desktop::library::folders(&bridge)
        .expect("folders")
        .folders[0]
        .id
        .clone();
    iamb_desktop::library::save_snippet(
        &bridge,
        &iamb_desktop::library::SnippetDraft {
            id: None,
            folder_id: folder,
            abbreviation: ";bad".to_owned(),
            label: "Out of office".to_owned(),
            kind: "plain".to_owned(),
            body: "Back soon {date".to_owned(),
            trigger: "delimiter".to_owned(),
            case_mode: "insensitive".to_owned(),
            word_start: true,
            enabled: true,
            tags: vec![],
        },
    )
    .expect("saved");
    let broken = settings::test_abbreviation(&bridge, ";bad").expect("test");
    assert_eq!(broken.outcome, "broken");
    assert_eq!(broken.error.as_deref(), Some("E_UNCLOSED_MACRO"));
    assert_eq!(broken.label.as_deref(), Some("Out of office"));
    // Nothing was typed anywhere.
    assert_eq!(service.text(), "");
}

#[test]
fn enter_and_tab_as_delimiters_make_the_test_say_expands() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    settings::put_setting(&bridge, "delimiters", Some(" .\n")).expect("put");
    // The answer comes before the engine has the new snapshot; `SettingsChanged` is sent after
    // it is queued for the engine, ahead of any key typed from here.
    seen.wait_event(|e| matches!(e, UiEvent::SettingsChanged {}));
    // The abbreviation followed by Enter now expands on a real keyboard too.
    service.type_text("omw\n");
    assert!(service.text().contains("on my way"));
}

#[test]
fn pausing_is_answered_with_the_status() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    assert!(bridge.set_paused(true).expect("paused").paused());
    service.type_text("brb ");
    assert!(
        service.text().contains("brb"),
        "nothing expands while paused"
    );
    assert!(!bridge.set_paused(false).expect("resumed").paused());
}
