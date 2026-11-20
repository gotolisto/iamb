//! The template editor's commands against a worker on the mock platform (task 3.4), and the
//! token fixtures of the editor's round-trip test.
//!
//! `spec_tokens_fixture` runs every template of `spec/template/examples/*.toml` (the template
//! language's examples) through `ParseTemplate` in the worker and writes the source with the
//! tokens and error the service answers to
//! `apps/desktop/src/library/template/spec.fixtures.json`. The front end's test
//! (`library/template/model.test.ts`) loads each template into the editor's model with those
//! tokens, writes it back, and requires the same bytes. This test fails when the file is stale;
//! run it with `IAMB_BLESS=1` to rewrite it, and review the diff.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use common::{Endpoint, Service};
use iamb_desktop::template::{self, PartView};
use serde_json::{Value, json};

fn app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Reads one TOML string value written on one line (a basic or a literal string), as every
/// `id` and `template` of the examples is; returns the value.
fn toml_string(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('\'') {
        let end = rest.find('\'').expect("literal string ends");
        return rest[..end].to_owned();
    }
    let rest = raw.strip_prefix('"').expect("a string");
    let mut out = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next().expect("basic string ends") {
            '"' => return out,
            '\\' => match chars.next().expect("escape") {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'e' => out.push('\u{1b}'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                c @ ('u' | 'U' | 'x') => {
                    let n = match c {
                        'u' => 4,
                        'U' => 8,
                        _ => 2,
                    };
                    let hex: String = chars.by_ref().take(n).collect();
                    let code = u32::from_str_radix(&hex, 16).expect("hex escape");
                    out.push(char::from_u32(code).expect("scalar value"));
                }
                other => panic!("unknown escape \\{other}"),
            },
            c => out.push(c),
        }
    }
}

/// Every example's id and template, file by file in name order.
fn examples() -> Vec<(String, String)> {
    let dir = app_dir().join("../../../spec/template/examples");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("examples directory")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).expect("example file");
        let mut id = None;
        for line in text.lines() {
            if line.trim() == "[[example]]" {
                id = None;
            } else if let Some(v) = line.strip_prefix("id = ") {
                id = Some(toml_string(v));
            } else if let Some(v) = line.strip_prefix("template = ") {
                out.push((id.take().expect("id before template"), toml_string(v)));
            }
        }
    }
    out
}

#[test]
fn the_examples_are_read_completely() {
    let all = examples();
    // The template language's §8 table: 173 examples.
    assert_eq!(all.len(), 173);
    let mut ids: Vec<&str> = all.iter().map(|(id, _)| id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 173, "unique ids");
    // Escapes are resolved: syntax examples hold quotes, backslashes and line breaks.
    assert!(all.iter().any(|(_, t)| t.contains('\n')));
    assert!(all.iter().any(|(_, t)| t.contains("\\\"")));
    assert!(all.iter().any(|(_, t)| t.contains('\r')));
}

#[test]
fn spec_tokens_fixture() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, recorder) = service.bridge();
    recorder.wait_connected();

    let mut entries = Vec::new();
    for (id, source) in examples() {
        let parsed = template::parse(&bridge, source.clone()).expect("parsed");
        // The tokens tile the source exactly, or there are none (a malformed macro).
        let units = source.encode_utf16().count();
        if parsed.tokens.is_empty() {
            assert!(
                source.is_empty() || parsed.error.is_some(),
                "{id}: no tokens without an error"
            );
        } else {
            let mut at = 0;
            for t in &parsed.tokens {
                assert_eq!(t.start, at, "{id}: tokens are contiguous");
                at = t.end;
            }
            assert_eq!(at, units, "{id}: tokens reach the end");
        }
        entries.push(json!({
            "id": id,
            "template": source,
            "tokens": parsed.tokens,
            "error": parsed.error,
        }));
    }
    bridge.stop();
    let ours = json!({ "examples": entries });

    let path = app_dir().join("../src/library/template/spec.fixtures.json");
    if std::env::var_os("IAMB_BLESS").is_some() {
        let text = serde_json::to_string_pretty(&ours).expect("json") + "\n";
        fs::write(&path, text).expect("fixtures written");
        return;
    }
    let text = fs::read_to_string(&path).unwrap_or_default();
    let theirs: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    assert!(
        ours == theirs,
        "src/library/template/spec.fixtures.json is stale: rerun with IAMB_BLESS=1"
    );
}

#[test]
fn parse_and_preview_through_the_bridge() {
    let service = Service::start(Endpoint::Mock);
    let (bridge, recorder) = service.bridge();
    recorder.wait_connected();

    // Chips: tokens with attributes, UTF-16 offsets.
    let parsed = template::parse(&bridge, "Hé 😀 {key name=tab times=2}".to_owned()).expect("ok");
    assert_eq!(parsed.error, None);
    assert_eq!(parsed.tokens.len(), 2);
    assert_eq!(
        (parsed.tokens[1].kind.as_str(), parsed.tokens[1].start),
        ("key", 6)
    );
    assert_eq!(parsed.tokens[1].attributes.len(), 2);

    // A snippet of the library resolves; a missing one is an error with a span.
    assert_eq!(
        template::parse(&bridge, "{snippet abbr=brb}".to_owned())
            .expect("ok")
            .error,
        None
    );
    let error = template::parse(&bridge, "x {snippet abbr=nope}".to_owned())
        .expect("ok")
        .error
        .expect("an error");
    assert_eq!(error.code, "E_SNIPPET_NOT_FOUND");
    assert_eq!((error.start, error.end), (Some(2), Some(21)));

    // The preview: parts, a sample blank, the clipboard sample, the cursor, the fields.
    let preview = template::preview(
        &bridge,
        "Hi {field name=Name}, {clipboard}{cursor}{wait ms=500}".to_owned(),
        BTreeMap::new(),
    )
    .expect("ok");
    assert_eq!(preview.error, None);
    let Some(PartView::Text {
        text,
        blanks,
        clipboard,
    }) = preview.parts.first()
    else {
        panic!("text first");
    };
    assert_eq!(text, "Hi Name, Clipboard");
    assert!(blanks[0].sample);
    assert_eq!(clipboard, &[[9, 18]]);
    assert_eq!(preview.parts.get(1), Some(&PartView::Wait { ms: 500 }));
    assert_eq!(preview.fields.len(), 1);
    assert!(preview.cursor.is_some());

    // Values given are used and are not samples.
    let preview = template::preview(
        &bridge,
        "Hi {field name=Name}".to_owned(),
        BTreeMap::from([("Name".to_owned(), "Ada".to_owned())]),
    )
    .expect("ok");
    assert!(matches!(
        preview.parts.first(),
        Some(PartView::Text { text, blanks, .. }) if text == "Hi Ada" && !blanks[0].sample
    ));

    // A broken template previews as its error.
    let preview =
        template::preview(&bridge, "{if field=A}open".to_owned(), BTreeMap::new()).expect("ok");
    assert!(preview.parts.is_empty());
    assert_eq!(
        preview.error.map(|e| e.code).as_deref(),
        Some("E_UNBALANCED_IF")
    );

    // Neither the source nor a value reaches the UI host's log.
    bridge.stop();
    let log = fs::read_to_string(service.dirs().logs().join("desktop.log")).unwrap_or_default();
    assert!(!log.contains("Ada") && !log.contains("field name"), "{log}");
}

#[test]
fn without_a_service_the_commands_say_so() {
    let mut service = Service::start(Endpoint::Mock);
    let (bridge, recorder) = service.bridge();
    recorder.wait_connected();
    service.stop_worker();
    recorder.wait_status(|v| v.service.is_none());
    let error = template::parse(&bridge, "x".to_owned()).expect_err("no service");
    assert!(
        ["disconnected", "timeout", "unavailable"].contains(&error.code.as_str()),
        "{error:?}"
    );
    bridge.stop();
}
