//! The library's requests and response mapping against `iamb-ipc`'s golden files
//! (`library.rs`, task 3.3), and the front end's fixtures for the library's payloads.
//!
//! `src/library/library.fixtures.json` (next to the front end's guards in `api.ts`) holds one
//! sample of each payload the front end receives from a library command. This test fails when
//! it and the Rust types differ; `IAMB_BLESS=1` rewrites it after a deliberate change.

use std::fs;
use std::path::PathBuf;

use iamb_core::model::Id;
use iamb_desktop::library::{self, REQUESTS, SnippetDraft};
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

fn id(n: u8) -> Id {
    Id::from_bytes([n; 16])
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
        ("request_list_folders", library::list_folders()),
        ("request_list_tags", library::list_tags()),
        ("request_get_snippet", library::get_snippet(id(1))),
        ("request_delete_snippet", library::delete_snippet(id(1))),
        ("request_delete_folder", library::delete_folder(id(2))),
        ("request_delete_tag", library::delete_tag(id(3))),
        ("request_health", library::health_request()),
        ("request_reset_usage", library::reset_usage_request()),
        (
            "request_check_abbreviation",
            library::check_abbreviation(
                ";rl",
                "immediate",
                Some("01010101-0101-0101-0101-010101010101"),
            )
            .expect("valid"),
        ),
    ] {
        let gold = golden_json(file);
        assert_eq!(
            encoded(body, gold["id"].as_u64().expect("id")),
            gold,
            "{file}"
        );
    }

    // A page of the whole library: the golden file's, without its tag filter.
    let mut gold = golden_json("request_list_snippet_summaries");
    gold["body"]["query"] = json!({});
    gold["body"]["offset"] = json!(200);
    gold["body"]["limit"] = json!(100);
    assert_eq!(encoded(library::list_summaries(200, 100), 11), gold);
    // The service's page limit is respected.
    let big = encoded(library::list_summaries(0, 5000), 1);
    assert_eq!(big["body"]["limit"], 1000);
}

#[test]
fn a_saved_snippet_is_the_golden_row_with_its_tags() {
    let draft = SnippetDraft {
        id: Some(id(1).to_string()),
        folder_id: id(2).to_string(),
        abbreviation: "brb".to_owned(),
        label: "Back soon".to_owned(),
        kind: "plain".to_owned(),
        body: "be right back".to_owned(),
        trigger: "delimiter".to_owned(),
        case_mode: "insensitive".to_owned(),
        word_start: true,
        enabled: true,
        tags: vec![id(3).to_string()],
    };
    let mut ours = encoded(library::put_snippet(&draft).expect("valid"), 4);
    let mut gold = golden_json("request_put_snippet");
    // The times are the clock's; the store ignores them on an update.
    for v in [&mut ours, &mut gold] {
        for key in ["created_at", "updated_at"] {
            v["body"]["snippet"][key] = json!(0);
        }
    }
    assert_eq!(ours, gold);
    assert!(Role::Ui.may_call(library::put_snippet(&draft).expect("valid").group()));

    // A draft the model refuses is reported before anything is sent.
    for bad in [
        SnippetDraft {
            abbreviation: "a b".to_owned(),
            ..draft.clone()
        },
        SnippetDraft {
            abbreviation: String::new(),
            ..draft.clone()
        },
        SnippetDraft {
            trigger: "now".to_owned(),
            ..draft.clone()
        },
        SnippetDraft {
            folder_id: "nope".to_owned(),
            ..draft.clone()
        },
    ] {
        assert_eq!(
            library::put_snippet(&bad).map_err(|e| e.code),
            Err("invalid".to_owned())
        );
    }
    // A new snippet gets a version 7 id of its own.
    let new = SnippetDraft { id: None, ..draft };
    let RequestBody::PutSnippet { snippet, .. } = library::put_snippet(&new).expect("valid") else {
        panic!("not a put");
    };
    assert_eq!(snippet.meta.id.version(), 7);
}

#[test]
fn every_request_the_library_sends_is_a_ui_request() {
    for name in REQUESTS {
        assert!(RequestBody::NAMES.contains(name), "{name}");
    }
    for file in [
        "request_list_folders",
        "request_list_tags",
        "request_list_snippet_summaries",
        "request_get_snippet",
        "request_put_snippet",
        "request_delete_snippet",
        "request_put_folder",
        "request_delete_folder",
        "request_put_tag",
        "request_delete_tag",
        "request_check_abbreviation",
        "request_health",
        "request_reset_usage",
    ] {
        let Wire::Request(r) = golden(file) else {
            panic!("{file}");
        };
        assert!(Role::Ui.may_call(r.body.group()), "{file}");
        assert!(REQUESTS.contains(&r.body.name()), "{file}");
    }
}

fn samples() -> Value {
    json!({
        "folders": library::folders_from(ok("response_folders")).expect("folders"),
        "tags": library::tags_from(ok("response_tags")).expect("tags"),
        "page": library::summaries_from(ok("response_snippet_summaries")).expect("page"),
        "snippet": library::full_from(ok("response_snippet")).expect("snippet"),
        "saved": library::saved_from(ok("response_saved")).expect("saved"),
        "check": library::check_from(ok("response_abbreviation_check")).expect("check"),
        "health": library::health_from(ok("response_health")).expect("health"),
    })
}

#[test]
fn the_front_end_fixtures_match_what_rust_sends() {
    let path = app_dir().join("../src/library/library.fixtures.json");
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
        "the library fixtures are stale: rerun with IAMB_BLESS=1 and update src/library/api.ts"
    );
}

#[test]
fn a_response_of_the_wrong_type_is_reported() {
    assert_eq!(
        library::folders_from(ok("response_done")).map_err(|e| e.got),
        Err("done")
    );
    assert_eq!(
        library::saved_from(ok("response_tags")).map_err(|e| e.got),
        Err("tags")
    );
}
