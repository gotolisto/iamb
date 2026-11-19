//! The bridge against a real worker (mock platform, in this process): the connection and its
//! status, the typed requests, the events forwarded to the front end, reconnecting when the
//! service restarts, and no user-entered value in either process's log.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, Instant};

use common::{Endpoint, LIBRARY, Service, T, id};
use iamb_core::model::{Abbreviation, Snippet, SyncMeta, Timestamp};
use iamb_desktop::bridge::CallError;
use iamb_desktop::contract::{ConnectionState, UiEvent};
use iamb_ipc::{RequestBody, ResponseBody, Role, Secret};

#[test]
fn the_bridge_connects_as_the_ui_host_and_reports_the_status() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    let view = seen.wait_connected();
    let status = view.service.clone().expect("service status");
    assert_eq!(
        status.snippets,
        u64::try_from(LIBRARY.len()).expect("small")
    );
    assert!(!status.paused);
    assert_eq!(bridge.status(), view);
    // The service counts it as the UI host.
    let mut cli = service.client(Role::Cli);
    match cli.call(RequestBody::GetStatus {}).expect("status") {
        ResponseBody::Status { status } => assert!(status.ui_connected),
        other => panic!("unexpected {other:?}"),
    }
    bridge.stop();
}

#[test]
fn pause_goes_through_the_bridge_and_comes_back_as_status() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let view = bridge.set_paused(true).expect("paused");
    assert!(view.paused());
    seen.wait_status(iamb_desktop::contract::StatusView::paused);
    service.type_text("brb ");
    assert_eq!(service.text(), "brb ", "paused: nothing expands");
    assert!(!bridge.set_paused(false).expect("resumed").paused());
    service.type_text("brb ");
    assert_eq!(service.text(), "brb be right back ");
    let refreshed = bridge.refresh_status().expect("status");
    assert!(!refreshed.paused());
}

#[test]
fn snippets_are_listed_and_read() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    let mut list = bridge.list_snippets(None).expect("list");
    list.sort_by(|a, b| a.abbreviation.cmp(&b.abbreviation));
    let abbreviations: Vec<&str> = list.iter().map(|s| s.abbreviation.as_str()).collect();
    assert_eq!(abbreviations, ["brb", "hi", "omw"]);
    assert!(list.iter().all(|s| s.enabled && s.kind == "plain"));
    let folder = list[0].folder_id.parse().expect("folder id");
    assert_eq!(bridge.list_snippets(Some(folder)).expect("folder").len(), 3);

    let one = bridge.get_snippet(id(1)).expect("snippet");
    assert_eq!(one.body, "be right back");
    assert_eq!(one.summary.label, "label brb");
    assert_eq!(one.trigger, "delimiter");
    let missing = bridge.get_snippet(id(77)).expect_err("no such snippet");
    assert_eq!(missing.code, "not_found");
}

#[test]
fn service_events_reach_the_front_end() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();

    // A form request: the field names travel, nothing typed does.
    service.type_text("hi ");
    let form = seen.wait_event(|e| matches!(e, UiEvent::FormRequested { .. }));
    let UiEvent::FormRequested {
        snippet, fields, ..
    } = form
    else {
        unreachable!()
    };
    assert_eq!(snippet, id(3).to_string());
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "Name");
    assert_eq!(fields[0].kind, "text");

    // A library change made by another client.
    let mut cli = service.client(Role::Cli);
    let folder = match cli.call(RequestBody::ListFolders {}).expect("folders") {
        ResponseBody::Folders { folders, .. } => folders[0].meta.id,
        other => panic!("unexpected {other:?}"),
    };
    let snippet = Snippet::new_defaults(
        SyncMeta::new(id(9), Timestamp(50)),
        folder,
        Abbreviation::new("ty").expect("abbreviation"),
        "",
        "thank you",
        None,
    );
    cli.call(RequestBody::PutSnippet { snippet })
        .expect("saved");
    seen.wait_event(|e| matches!(e, UiEvent::LibraryChanged { revision } if *revision >= 2));
    // The service reports no status change for a library change; the front end asks again.
    let view = bridge.refresh_status().expect("status");
    assert_eq!(view.service.map(|s| s.snippets), Some(4));
    bridge.stop();
}

#[test]
fn the_bridge_reconnects_when_the_service_restarts() {
    let mut service = Service::start(Endpoint::Mock);
    // The UI host starts before the service.
    service.stop_worker();
    let (bridge, seen) = service.bridge();
    seen.wait_status(|v| v.connection == ConnectionState::Disconnected);
    assert_eq!(
        bridge.call(RequestBody::GetStatus {}),
        Err(CallError::Disconnected)
    );
    service.restart();
    seen.wait_connected();
    assert!(bridge.list_snippets(None).is_ok());

    // The service crashes and comes back: the bridge follows within its backoff.
    seen.clear();
    service.stop_worker();
    seen.wait_status(|v| v.connection != ConnectionState::Connected);
    assert!(bridge.status().service.is_none());
    let restarted = Instant::now();
    service.restart();
    seen.wait_connected();
    assert!(restarted.elapsed() < Duration::from_secs(3));
    assert_eq!(bridge.list_snippets(None).expect("list").len(), 3);
    bridge.stop();
    // A stopped bridge stays down.
    assert_eq!(
        bridge.call(RequestBody::GetStatus {}),
        Err(CallError::Disconnected)
    );
}

#[test]
fn form_values_pass_through_the_bridge_without_reaching_a_log() {
    const CANARY: &str = "zqxformcanary";
    let service = Service::start(Endpoint::Mock);
    let (bridge, seen) = service.bridge();
    seen.wait_connected();
    service.type_text("hi ");
    let UiEvent::FormRequested { session, .. } =
        seen.wait_event(|e| matches!(e, UiEvent::FormRequested { .. }))
    else {
        unreachable!()
    };
    bridge
        .call(RequestBody::FormAck { session })
        .expect("acknowledged");
    bridge
        .call(RequestBody::FormSubmit {
            session,
            values: Secret::new(BTreeMap::from([("Name".to_owned(), CANARY.to_owned())])),
        })
        .expect("submitted");
    let deadline = Instant::now() + T;
    while !service.text().contains(CANARY) && Instant::now() < deadline {
        service.worker().settle();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(service.text(), format!("Hello {CANARY}! "));
    bridge.stop();
    for entry in fs::read_dir(service.dirs().logs()).expect("logs") {
        let path = entry.expect("entry").path();
        let text = fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !text.contains(CANARY),
            "{} holds a form value",
            path.display()
        );
    }
}
