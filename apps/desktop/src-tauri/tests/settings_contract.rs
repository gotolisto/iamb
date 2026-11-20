//! The settings and diagnostics requests and response mapping against `iamb-ipc`'s golden files
//! (`settings.rs`, task 3.9), and the front end's fixtures for their payloads.
//!
//! `src/library/settings/settings.fixtures.json` (next to the front end's guards in `api.ts`)
//! holds one sample of each payload the front end receives from these commands. This test fails
//! when it and the Rust types differ; `IAMB_BLESS=1` rewrites it after a deliberate change.

use std::fs;
use std::path::PathBuf;

use iamb_desktop::settings::{self, REQUESTS, TestView};
use iamb_ipc::{Request, RequestBody, Role, Wire};
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

fn ok(name: &str) -> iamb_ipc::ResponseBody {
    match golden(name) {
        Wire::Response(r) => r.into_result().expect("ok"),
        _ => panic!("{name} is not a response"),
    }
}

#[test]
fn the_requests_match_their_golden_files() {
    for (file, body) in [
        ("request_get_settings", settings::get_settings()),
        (
            "request_put_setting",
            settings::put_setting_request("date_format", Some("%d.%m.%Y")),
        ),
        ("request_list_exclusions", settings::list_exclusions()),
        (
            "request_put_exclusion",
            settings::put_exclusion_request("org.example.bank", "Bank", true),
        ),
        ("request_recent_diagnostics", settings::recent_diagnostics()),
    ] {
        let gold = golden_json(file);
        assert_eq!(
            encoded(body, gold["id"].as_u64().expect("id")),
            gold,
            "{file}"
        );
    }
    // The dry run carries the typed text and nothing else: no clock, no clipboard, no values.
    let dry = encoded(settings::dry_run_request("brb "), 22);
    assert_eq!(dry["body"]["typed"], "brb ");
    for absent in ["now", "clipboard", "values"] {
        assert!(dry["body"].get(absent).is_none(), "{absent}");
    }
    // Removing a setting sends no value.
    assert!(
        encoded(settings::put_setting_request("locale", None), 1)["body"]
            .get("value")
            .is_none()
    );
}

#[test]
fn every_request_the_settings_send_is_a_ui_request() {
    for name in REQUESTS {
        assert!(RequestBody::NAMES.contains(name), "{name}");
    }
    for file in [
        "request_get_settings",
        "request_put_setting",
        "request_list_exclusions",
        "request_put_exclusion",
        "request_recent_diagnostics",
        "request_dry_run",
    ] {
        let Wire::Request(r) = golden(file) else {
            panic!("{file}");
        };
        assert!(Role::Ui.may_call(r.body.group()), "{file}");
        assert!(REQUESTS.contains(&r.body.name()), "{file}");
    }
}

#[test]
fn diagnostics_are_listed_newest_first() {
    use iamb_core::model::Timestamp;
    use iamb_ipc::{ResponseBody, WireDiagnostic};
    let entries = vec![
        WireDiagnostic::code("form_open", Timestamp(10)),
        WireDiagnostic::code("not_a_delimiter", Timestamp(20)),
    ];
    let view = settings::diagnostics_from(ResponseBody::Diagnostics { entries }).expect("list");
    assert_eq!(
        view.iter().map(|d| d.code.as_str()).collect::<Vec<_>>(),
        ["not_a_delimiter", "form_open"]
    );
}

fn samples() -> Value {
    let test = TestView {
        outcome: "expands".to_owned(),
        snippet: Some("01010101-0101-0101-0101-010101010101".to_owned()),
        label: Some("Thanks for your order".to_owned()),
        abbreviation: Some(";rty".to_owned()),
        error: None,
    };
    json!({
        "settings": settings::settings_from(ok("response_settings")).expect("settings"),
        "exclusions": settings::exclusions_from(ok("response_exclusions")).expect("exclusions"),
        "diagnostics": settings::diagnostics_from(ok("response_diagnostics")).expect("diagnostics"),
        "test": test,
    })
}

#[test]
fn the_front_end_fixtures_match_what_rust_sends() {
    let path = app_dir().join("../src/library/settings/settings.fixtures.json");
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
        "the settings fixtures are stale: rerun with IAMB_BLESS=1 and update src/library/settings/api.ts"
    );
}

#[test]
fn a_response_of_the_wrong_type_is_reported() {
    assert_eq!(
        settings::settings_from(ok("response_exclusions")).map_err(|e| e.got),
        Err("exclusions")
    );
    assert_eq!(
        settings::exclusions_from(ok("response_settings")).map_err(|e| e.got),
        Err("settings")
    );
    assert_eq!(
        settings::diagnostics_from(ok("response_done")).map_err(|e| e.got),
        Err("done")
    );
}
