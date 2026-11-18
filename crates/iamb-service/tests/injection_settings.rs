//! The `injection_overrides` setting reaches the injector (ADR-0025): at start, after
//! `PutSetting`, and never when the injector refuses it.

mod common;

use std::fs;

use common::{Harness, LIBRARY, wait_event};
use iamb_ipc::{ErrorCode, EventBody, RequestBody, ResponseBody, Role};
use iamb_platform::INJECTION_SETTINGS_KEY;
use iamb_store::Store;

const KEYS: &str = r#"{"apps": {"zqxprivateapp.exe": {"method": "keys"}}}"#;
const PASTE: &str = r#"{"defaults": {"method": "paste", "paste_wait_ms": 300}}"#;
/// The shape of an earlier development build: refused.
const OLD_SHAPE: &str = r#"{"default": {"typing": "zqxprivatevalue"}}"#;

fn put(ui: &mut iamb_ipc::Client, value: Option<&str>) -> Result<(), Option<ErrorCode>> {
    ui.call(RequestBody::PutSetting {
        key: INJECTION_SETTINGS_KEY.to_owned(),
        value: value.map(str::to_owned),
    })
    .map(|_| ())
    .map_err(|e| e.remote_code())
}

fn stored(ui: &mut iamb_ipc::Client) -> Option<String> {
    match ui.call(RequestBody::GetSettings {}) {
        Ok(ResponseBody::Settings { settings }) => settings.get(INJECTION_SETTINGS_KEY).cloned(),
        other => panic!("unexpected {other:?}"),
    }
}

/// A worker started over a store that already holds `value`.
fn start_with_stored(value: &str) -> Harness {
    let value = value.to_owned();
    Harness::start_with(LIBRARY, move |config| {
        let mut store = Store::open(config.dirs.root()).expect("store opens");
        store
            .put_setting(INJECTION_SETTINGS_KEY, &value)
            .expect("setting");
    })
}

fn worker_log(h: &Harness) -> String {
    fs::read_to_string(h.dirs().logs().join("worker.log")).expect("worker log")
}

#[test]
fn without_the_setting_the_injector_gets_the_empty_string_at_start() {
    let h = Harness::start(LIBRARY);
    assert_eq!(h.os.configured(), [""]);
}

#[test]
fn the_stored_setting_is_applied_at_start() {
    let h = start_with_stored(KEYS);
    assert_eq!(h.os.configured(), [KEYS]);
    assert_eq!(h.os.injection_settings(), KEYS);
    // Expansions still work with it in force.
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back ");
}

#[test]
fn a_refused_stored_setting_is_logged_and_the_worker_runs_without_it() {
    let h = start_with_stored(OLD_SHAPE);
    assert_eq!(h.os.configured(), [OLD_SHAPE]);
    assert_eq!(h.os.injection_settings(), "");
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back ");
    let log = worker_log(&h);
    assert!(
        log.contains("the injection_overrides setting was refused"),
        "{log}"
    );
    // The log names what was wrong, never the setting's text.
    assert!(!log.contains("zqxprivate"), "{log}");
}

#[test]
fn put_setting_applies_the_new_value_once_and_announces_it() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert_eq!(put(&mut ui, Some(KEYS)), Ok(()));
    wait_event(&mut ui, |e| matches!(e, EventBody::SettingsChanged {}));
    assert_eq!(h.os.injection_settings(), KEYS);
    assert_eq!(stored(&mut ui).as_deref(), Some(KEYS));

    // Another setting changes: the injector is not configured again.
    ui.call(RequestBody::PutSetting {
        key: "locale".to_owned(),
        value: Some("de".to_owned()),
    })
    .expect("stored");
    wait_event(&mut ui, |e| matches!(e, EventBody::SettingsChanged {}));
    assert_eq!(put(&mut ui, Some(PASTE)), Ok(()));
    assert_eq!(put(&mut ui, None), Ok(()));
    h.settle();
    assert_eq!(h.os.configured(), ["", KEYS, PASTE, ""]);
    assert_eq!(h.os.injection_settings(), "");
    assert_eq!(stored(&mut ui), None);
}

#[test]
fn a_refused_value_is_a_settings_error_and_the_last_good_settings_stay() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert_eq!(put(&mut ui, Some(KEYS)), Ok(()));
    for bad in [
        OLD_SHAPE,
        "{",
        r#"{"defaults": {"key_delay_ms": 999999}}"#,
        // A macOS-only line break, while the mock reads the Windows dialect.
        r#"{"defaults": {"line_break": "option+enter"}}"#,
    ] {
        assert_eq!(
            put(&mut ui, Some(bad)),
            Err(Some(ErrorCode::Invalid)),
            "{bad}"
        );
    }
    h.settle();
    assert_eq!(h.os.injection_settings(), KEYS, "the last good settings");
    assert_eq!(stored(&mut ui).as_deref(), Some(KEYS), "nothing was stored");
    assert_eq!(
        h.os.configured().len(),
        6,
        "each value was handed over once"
    );
    let log = worker_log(&h);
    assert!(!log.contains("zqxprivate"), "{log}");
}
