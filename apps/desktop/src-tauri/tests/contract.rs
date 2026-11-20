//! Message contract tests: the bridge's hello, its requests and the responses and events it
//! reads, against `iamb-ipc`'s golden files (`crates/iamb-ipc/tests/golden/`, one per message).
//!
//! Each request the bridge builds must encode to the golden file of its message; each golden
//! response and event the bridge reads must decode and map to what the front end is promised.
//! A protocol change regenerates the golden files (`IAMB_BLESS=1` in `iamb-ipc`), and this test
//! then fails until the bridge follows. The role table is checked too: the `ui` role may call
//! every request the bridge sends and hears every event it forwards.

use std::fs;
use std::path::PathBuf;

use iamb_core::model::Id;
use iamb_desktop::bridge::connection::hello;
use iamb_desktop::contract::{
    self, ChangedView, CommandError, EVENTS, FieldView, Forward, REQUESTS, ServiceView, UiEvent,
};
use iamb_ipc::{ErrorCode, EventBody, PROTOCOL_VERSION, Request, RequestBody, Role, Wire};
use serde_json::Value;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../crates/iamb-ipc/tests/golden")
}

fn golden(name: &str) -> Value {
    let path = golden_dir().join(format!("{name}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("golden JSON")
}

fn decode(name: &str) -> Wire {
    Wire::decode(golden(name).to_string().as_bytes())
        .unwrap_or_else(|e| panic!("{name} no longer decodes: {e}"))
}

fn id(n: u8) -> Id {
    Id::from_bytes([n; 16])
}

fn encoded(wire: &Wire) -> Value {
    serde_json::from_slice(&wire.encode().expect("encodes")).expect("JSON")
}

/// The id of a golden request, so the bridge's request can be compared whole.
fn golden_id(value: &Value) -> u64 {
    value["id"].as_u64().expect("id")
}

#[test]
fn the_hello_is_the_ui_role_at_the_current_protocol() {
    let ours = encoded(&hello("0.0.0"));
    assert_eq!(ours, golden("hello"));
    assert_eq!(ours["role"], "ui");
    assert_eq!(ours["protocol"], PROTOCOL_VERSION);
}

#[test]
fn every_request_the_bridge_sends_matches_its_golden_file() {
    let samples = [
        ("request_get_status", contract::get_status()),
        ("request_set_paused", contract::set_paused(true)),
        (
            "request_list_snippets",
            contract::list_snippets(Some(id(2))),
        ),
        ("request_get_snippet", contract::get_snippet(id(1))),
    ];
    let mut names = Vec::new();
    for (file, body) in samples {
        let gold = golden(file);
        names.push(body.name());
        assert!(
            Role::Ui.may_call(body.group()),
            "the ui role may not call {}",
            body.name()
        );
        let ours = encoded(&Wire::Request(Request {
            id: golden_id(&gold),
            body,
        }));
        assert_eq!(ours, gold, "{file}");
    }
    assert_eq!(
        names, REQUESTS,
        "contract::REQUESTS lists what the bridge sends"
    );
    assert!(REQUESTS.iter().all(|r| RequestBody::NAMES.contains(r)));
}

#[test]
fn the_requests_the_bridge_passes_through_are_ui_requests() {
    // The form requests the bridge sends for the panels (tasks 3.5, 3.6) and in tests.
    for file in [
        "request_form_ack",
        "request_form_submit",
        "request_form_cancel",
    ] {
        let Wire::Request(request) = decode(file) else {
            panic!("{file} is not a request");
        };
        assert!(Role::Ui.may_call(request.body.group()), "{file}");
    }
}

#[test]
fn the_responses_the_bridge_reads_map_to_the_front_ends_types() {
    let response = |file: &str| match decode(file) {
        Wire::Response(r) => r.into_result(),
        other => panic!("{file} is a {} frame", other.name()),
    };

    let status = contract::status_from(response("response_status").expect("ok")).expect("status");
    assert_eq!(
        ServiceView::from(&status),
        ServiceView {
            version: "0.0.0".to_owned(),
            paused: false,
            suspended: true,
            snippets: 12,
            revision: 3,
            listener: true,
            memory_locked: true,
        }
    );

    let snippets =
        contract::snippets_from(response("response_snippets").expect("ok")).expect("snippets");
    assert_eq!(snippets.len(), 1);
    assert_eq!(snippets[0].id, id(1).to_string());
    assert_eq!(snippets[0].folder_id, id(2).to_string());
    assert_eq!(snippets[0].abbreviation, "brb");
    assert_eq!(snippets[0].label, "Back soon");
    assert_eq!(snippets[0].kind, "plain");
    assert!(snippets[0].enabled);

    let detail =
        contract::snippet_from(response("response_snippet").expect("ok")).expect("snippet");
    assert_eq!(detail.summary.abbreviation, "brb");
    assert_eq!(detail.body, "be right back");
    assert_eq!(detail.trigger, "delimiter");

    // A response of the wrong type is reported, not misread.
    let done = response("response_done").expect("ok");
    assert_eq!(contract::status_from(done).map_err(|e| e.got), Err("done"));

    // An error keeps its code for the front end.
    let error = response("response_error").expect_err("error");
    let error: CommandError = error.into();
    assert_eq!(error.code, ErrorCode::NotPermitted.as_str());
    assert_eq!(error.message, "not for this role");
}

#[test]
fn every_event_the_ui_role_hears_is_forwarded_as_promised() {
    let event = |name: &str| match decode(&format!("event_{name}")) {
        Wire::Event(e) => e,
        other => panic!("event_{name} is a {} frame", other.name()),
    };
    // The bridge handles exactly the events its role hears.
    let heard: Vec<&str> = EventBody::NAMES
        .iter()
        .copied()
        .filter(|name| Role::Ui.may_call(event(name).group()))
        .collect();
    assert_eq!(
        heard, EVENTS,
        "contract::EVENTS lists the events the ui role hears"
    );

    match contract::forward(&event("status_changed")) {
        Forward::Status(status) => assert_eq!(status.snippets, 12),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        contract::forward(&event("library_changed")),
        Forward::Ui(UiEvent::LibraryChanged {
            revision: 4,
            changed: Some(ChangedView {
                snippets: vec![id(1).to_string()],
                folders: vec![],
                tags: vec![id(3).to_string()],
            }),
        })
    );
    assert_eq!(
        contract::forward(&event("form_requested")),
        Forward::Ui(UiEvent::FormRequested {
            session: 2,
            snippet: id(1).to_string(),
            fields: vec![FieldView {
                name: "Size".to_owned(),
                kind: "choice".to_owned(),
                options: vec!["S".to_owned(), "M".to_owned()],
                default: "M".to_owned(),
            }],
        })
    );
    assert_eq!(
        contract::forward(&event("palette_requested")),
        Forward::Ui(UiEvent::PaletteRequested {})
    );
    assert_eq!(
        contract::forward(&event("diagnostic_added")),
        Forward::Ui(UiEvent::DiagnosticAdded {
            code: "injection_cancelled".to_owned(),
            snippet: Some(id(1).to_string()),
            app: None,
            at: 1_763_400_000_500,
        })
    );
    assert_eq!(
        contract::forward(&event("settings_changed")),
        Forward::Ui(UiEvent::SettingsChanged {})
    );
    // The sync group's event is not the UI's.
    assert!(!Role::Ui.may_call(event("outbox_changed").group()));
    assert_eq!(contract::forward(&event("outbox_changed")), Forward::Ignore);
}

#[test]
fn a_form_request_never_hands_values_or_the_focus_token_to_the_front_end() {
    let Wire::Event(event) = decode("event_form_requested") else {
        panic!("not an event");
    };
    let Forward::Ui(ui) = contract::forward(&event) else {
        panic!("not forwarded");
    };
    let json = serde_json::to_string(&ui).expect("json");
    // The golden file's last values ("Ada") and focus token (pid 100) stay in Rust.
    assert!(!json.contains("Ada"), "{json}");
    assert!(!json.contains("focus") && !json.contains("100"), "{json}");
}
