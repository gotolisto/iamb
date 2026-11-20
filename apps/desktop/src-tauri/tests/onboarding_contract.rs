//! The onboarding requests and response mapping against `iamb-ipc`'s golden files
//! (`onboarding.rs`, task 3.7), and the front end's fixtures for their payloads.
//!
//! `src/onboarding/onboarding.fixtures.json` (next to the front end's guards in `api.ts`) holds
//! one sample of each payload the front end receives from these commands. This test fails when it
//! and the Rust types differ; `IAMB_BLESS=1` rewrites it after a deliberate change.

use std::fs;
use std::path::PathBuf;

use iamb_desktop::onboarding::{
    self, FinishView, Grant, LoginView, PermissionsView, REQUESTS, permission_named,
};
use iamb_ipc::{Request, RequestBody, Role, Wire};
use iamb_platform::FocusToken;
use serde_json::{Value, json};

fn app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &PathBuf) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn golden_json(name: &str) -> Value {
    let path = app_dir().join(format!("../../../crates/iamb-ipc/tests/golden/{name}.json"));
    serde_json::from_str(&read(&path)).expect("golden JSON")
}

fn golden(name: &str) -> Wire {
    Wire::decode(golden_json(name).to_string().as_bytes()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn encoded(body: RequestBody, id: u64) -> Value {
    serde_json::from_slice(
        &Wire::Request(Request { id, body })
            .encode()
            .expect("encodes"),
    )
    .expect("JSON")
}

#[test]
fn the_requests_match_their_golden_files() {
    for (file, body) in [
        ("request_get_permissions", onboarding::get_permissions()),
        (
            "request_open_permission_settings",
            onboarding::open_permission_settings_request(
                permission_named("input_monitoring").expect("known"),
            ),
        ),
        (
            "request_set_test_field",
            onboarding::set_test_field_request(Some(FocusToken::from_raw(7, 8))),
        ),
    ] {
        let gold = golden_json(file);
        assert_eq!(
            encoded(body, gold["id"].as_u64().expect("id")),
            gold,
            "{file}"
        );
    }
    // Clearing the test field sends no window.
    assert!(
        encoded(onboarding::set_test_field_request(None), 1)["body"]
            .get("window")
            .is_none()
    );
}

#[test]
fn every_request_onboarding_sends_is_a_ui_request() {
    for name in REQUESTS {
        assert!(RequestBody::NAMES.contains(name), "{name}");
    }
    for file in [
        "request_get_permissions",
        "request_open_permission_settings",
        "request_set_test_field",
    ] {
        let Wire::Request(r) = golden(file) else {
            panic!("{file}");
        };
        assert!(Role::Ui.may_call(r.body.group()), "{file}");
        assert!(REQUESTS.contains(&r.body.name()), "{file}");
    }
}

#[test]
fn the_permissions_response_maps_to_what_the_window_reads() {
    let Wire::Response(r) = golden("response_permissions") else {
        panic!("not a response");
    };
    let view = onboarding::permissions_from(r.into_result().expect("ok")).expect("permissions");
    assert_eq!(
        view,
        PermissionsView {
            accessibility: Grant::Granted,
            input_monitoring: Grant::Denied
        }
    );
    assert!(view.missing());
}

fn samples() -> Value {
    json!({
        "permissions": PermissionsView { accessibility: Grant::Granted, input_monitoring: Grant::Denied },
        "windows": PermissionsView { accessibility: Grant::NotRequired, input_monitoring: Grant::NotRequired },
        "finished": FinishView { palette_hotkey: Some("Ctrl+Shift+Space".to_owned()), hotkey_refused: false },
        "refused": FinishView { palette_hotkey: None, hotkey_refused: true },
        "login": [LoginView::Enabled, LoginView::Disabled, LoginView::RequiresApproval, LoginView::Unavailable],
    })
}

#[test]
fn the_front_end_fixtures_match_what_rust_sends() {
    let path = app_dir().join("../src/onboarding/onboarding.fixtures.json");
    let ours = samples();
    if std::env::var_os("IAMB_BLESS").is_some() {
        fs::write(
            &path,
            serde_json::to_string_pretty(&ours).expect("json") + "\n",
        )
        .expect("written");
        return;
    }
    let theirs: Value = serde_json::from_str(&read(&path)).expect("fixtures JSON");
    assert_eq!(
        ours, theirs,
        "src/onboarding/onboarding.fixtures.json is out of date; run it with IAMB_BLESS=1"
    );
}

#[test]
fn the_commands_are_registered_in_the_three_lists() {
    // `front_end_contract.rs` compares the lists as a whole; this one names the onboarding ones.
    for name in [
        "onboarding_permissions",
        "onboarding_open_permission_settings",
        "onboarding_test_field",
        "onboarding_restart_helper",
        "onboarding_finish",
        "onboarding_login_item",
        "onboarding_open_library",
        "onboarding_close",
    ] {
        assert!(iamb_desktop::contract::COMMANDS.contains(&name), "{name}");
    }
}
