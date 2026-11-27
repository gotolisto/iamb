//! The contract between the Rust side and the TypeScript front end (`apps/desktop/src/ipc.ts`).
//!
//! - The command names: `contract::COMMANDS`, the `generate_handler!` list in `src/app/mod.rs`
//!   and `COMMANDS` in `ipc.ts` are the same list.
//! - The event names: `ipc.ts` listens to `contract::STATUS_EVENT` and `contract::UI_EVENT`.
//! - The payloads: one sample of each type the front end receives, built from `iamb-ipc`'s
//!   golden messages through the bridge's mapping, is written to
//!   `apps/desktop/src/contract.fixtures.json`; `ipc.test.ts` checks that the front end's
//!   guards accept each one. This test fails when the samples and the file differ; run it with
//!   `IAMB_BLESS=1` to rewrite the file after a deliberate change, and review the diff.

use std::fs;
use std::path::PathBuf;

use iamb_desktop::contract::{
    self, COMMANDS, CommandError, ConnectionState, Forward, STATUS_EVENT, StatusView, UI_EVENT,
};
use iamb_desktop::{form, template};
use iamb_ipc::{EventBody, Wire};
use serde_json::{Value, json};

fn app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &PathBuf) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn golden(name: &str) -> Wire {
    let path = app_dir().join(format!("../../../crates/iamb-ipc/tests/golden/{name}.json"));
    Wire::decode(read(&path).as_bytes()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The quoted names inside the first `[` … `]` after `marker` in `text`.
fn quoted_list(text: &str, marker: &str) -> Vec<String> {
    let start = text
        .find(marker)
        .unwrap_or_else(|| panic!("{marker} not found"));
    let rest = &text[start..];
    let open = rest.find('[').expect("list start");
    let close = rest[open..].find(']').expect("list end") + open;
    rest[open + 1..close]
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

#[test]
fn the_command_lists_agree() {
    let ts = read(&app_dir().join("../src/ipc.ts"));
    // As sets: tasks add their commands to each list independently.
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    let expected = sorted(COMMANDS.iter().map(|c| (*c).to_owned()).collect());
    assert_eq!(
        sorted(quoted_list(&ts, "export const COMMANDS")),
        expected,
        "ipc.ts"
    );

    let app = read(&app_dir().join("src/app/mod.rs"));
    // Handlers are `module::name`; the command's name is the part after the last `::`.
    let handlers: Vec<String> = quoted_list(&app, "generate_handler!")
        .into_iter()
        .map(|h| h.rsplit("::").next().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        sorted(handlers),
        expected,
        "generate_handler! in src/app/mod.rs"
    );

    for event in [STATUS_EVENT, UI_EVENT] {
        assert!(ts.contains(&format!("\"{event}\"")), "ipc.ts names {event}");
    }
}

fn samples() -> Value {
    let status = match golden("response_status") {
        Wire::Response(r) => contract::status_from(r.into_result().expect("ok")).expect("status"),
        _ => panic!("not a response"),
    };
    let statuses: Vec<StatusView> = vec![
        StatusView::new(ConnectionState::Connecting, None),
        StatusView::new(ConnectionState::Disconnected, None),
        StatusView::new(ConnectionState::Connected, Some(&status)),
    ];
    let events: Vec<Value> = EventBody::NAMES
        .iter()
        .filter_map(|name| match golden(&format!("event_{name}")) {
            Wire::Event(e) => match contract::forward(&e) {
                Forward::Ui(ui) => Some(serde_json::to_value(ui).expect("json")),
                Forward::Status(_) | Forward::Ignore => None,
            },
            _ => panic!("not an event"),
        })
        .collect();
    let snippets = match golden("response_snippets") {
        Wire::Response(r) => contract::snippets_from(r.into_result().expect("ok")).expect("list"),
        _ => panic!("not a response"),
    };
    let snippet = match golden("response_snippet") {
        Wire::Response(r) => contract::snippet_from(r.into_result().expect("ok")).expect("one"),
        _ => panic!("not a response"),
    };
    let error: CommandError = match golden("response_error") {
        Wire::Response(r) => r.into_result().expect_err("error").into(),
        _ => panic!("not a response"),
    };
    // The template editor's answers (task 3.4).
    let parsed = match golden("response_template_tokens") {
        Wire::Response(r) => template::parsed_from(r.into_result().expect("ok")).expect("tokens"),
        _ => panic!("not a response"),
    };
    let preview = match golden("response_preview") {
        Wire::Response(r) => template::preview_from(r.into_result().expect("ok")).expect("preview"),
        _ => panic!("not a response"),
    };
    // The refusal of a link that may not be opened (`link.rs`).
    let link_refused: CommandError = iamb_desktop::link::checked("javascript:alert(1)")
        .expect_err("refused")
        .into();
    json!({
        "statuses": statuses,
        "events": events,
        "snippets": snippets,
        "snippet": snippet,
        "error": error,
        "template": { "parsed": parsed, "preview": preview, "link_refused": link_refused },
        "form": form_samples(),
    })
}

/// The fill-in form panel's payloads (task 3.5): the form `form_current` answers, built from the
/// golden `FormRequested` and the golden snippet's header, its events, and the error of an
/// Insert whose focus could not be given back.
fn form_samples() -> Value {
    let Wire::Event(EventBody::FormRequested {
        session,
        snippet,
        spec,
        last_values,
        ..
    }) = golden("event_form_requested")
    else {
        panic!("not a form request")
    };
    let header = match golden("response_snippet") {
        Wire::Response(r) => form::header_from(&r.into_result().expect("ok")).expect("header"),
        _ => panic!("not a response"),
    };
    let open = form::form_open(session, snippet, &spec, last_values.as_ref(), &header, None);
    let named = form::form_open(
        session,
        snippet,
        &spec,
        None,
        &header,
        Some("Mail".to_owned()),
    );
    json!({
        "open": open,
        "openWithoutValues": named,
        "events": [
            form::FormEvent::Open { session },
            form::FormEvent::Shown { session },
            form::FormEvent::TargetGone { session },
            form::FormEvent::Closed { session },
        ],
        "focusLost": form::focus_lost(),
    })
}

#[test]
fn the_front_end_fixtures_match_what_rust_sends() {
    let path = app_dir().join("../src/contract.fixtures.json");
    let ours = samples();
    if std::env::var_os("IAMB_BLESS").is_some() {
        let text = serde_json::to_string_pretty(&ours).expect("json") + "\n";
        fs::write(&path, text).expect("fixtures written");
        return;
    }
    let theirs: Value = serde_json::from_str(&read(&path)).expect("fixtures JSON");
    assert_eq!(
        ours, theirs,
        "the front end's fixtures are stale: rerun with IAMB_BLESS=1 and update src/ipc.ts"
    );
    assert_eq!(ours["events"].as_array().map(Vec::len), Some(5));
}
