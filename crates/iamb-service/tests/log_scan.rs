//! Logs never contain typed text (architecture §8): canary strings typed through the mock
//! keyboard, sent as form values, dry-run input, palette queries and clipboard text must not
//! appear in any file under the data root's `logs/` directory, nor in `run/`: neither as one
//! string nor one character at a time (at a constant stride, or one character per log line),
//! which is how a log that records every key would hold them (finding 2 of the Phase 2 privacy
//! review). The per-character detectors are the soak scan's own, compiled here by path.

mod common;

#[allow(dead_code, reason = "the soak scan uses more of it than this test")]
#[path = "../../iamb-testkit/src/soak/spread.rs"]
mod spread;

use std::collections::BTreeMap;
use std::fs;

use common::{Harness, LIBRARY, files_under, wait_event};
use iamb_ipc::{EventBody, RequestBody, Role, Secret};
use iamb_platform::ClipboardContent;
use iamb_platform::mock::{InjectionPoint, MockKey, MockWindow};
use iamb_service::log::Logger;

const TYPED: &str = "zqxtypedcanary";
const AHEAD: &str = "zqxahead";
const SECRET_FIELD: &str = "zqxpasswordcanary";
const FORM: &str = "zqxformcanary";
const DRY: &str = "zqxdrycanary";
const QUERY: &str = "zqxquerycanary";
const CLIP: &str = "zqxclipcanary";

const ALL: [&str; 7] = [TYPED, AHEAD, SECRET_FIELD, FORM, DRY, QUERY, CLIP];

/// Every canary of `ALL` that `bytes` hold, with the form: `whole`, `stride-<s>` or `per-line`.
fn leaks(bytes: &[u8]) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for canary in ALL {
        if bytes.windows(canary.len()).any(|w| w == canary.as_bytes()) {
            out.push((canary, "whole".to_owned()));
        }
    }
    let detectors = spread::Spread::new(&ALL);
    detectors.strided(bytes, bytes.len(), &mut |i, _, stride| {
        out.push((ALL[i], format!("stride-{stride}")));
    });
    let mut lines = detectors.lines();
    lines.feed(bytes, &mut |i, _| out.push((ALL[i], "per-line".to_owned())));
    lines.finish(&mut |i, _| out.push((ALL[i], "per-line".to_owned())));
    let mut numbers = detectors.numbers();
    numbers.feed(bytes, &mut |i, _| {
        out.push((ALL[i], "per-line-decimal".to_owned()));
    });
    numbers.finish(&mut |i, _| {
        out.push((ALL[i], "per-line-decimal".to_owned()));
    });
    out
}

#[test]
fn no_log_file_contains_what_the_user_typed_or_entered() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);

    // Typed into a normal field, around expansions, and typed ahead during one.
    h.os.type_text(&format!("{TYPED} brb "));
    h.os.interleave(InjectionPoint::BeforeStartMarker, MockKey::text(AHEAD));
    h.os.type_text("omw ");
    h.settle();
    // Typed into a password field.
    let bank =
        h.os.open_window(MockWindow::new("org.example.bank", 7).secure());
    h.os.focus(bank).expect("open");
    h.worker().sync();
    h.os.type_text(SECRET_FIELD);
    h.os.focus(h.editor).expect("open");
    h.worker().sync();

    // A form value.
    h.os.type_text("hi ");
    let EventBody::FormRequested { session, .. } =
        wait_event(&mut ui, |e| matches!(e, EventBody::FormRequested { .. }))
    else {
        unreachable!("matched above")
    };
    ui.call(RequestBody::FormAck { session }).expect("ack");
    ui.call(RequestBody::FormSubmit {
        session,
        values: Secret::new(BTreeMap::from([("Name".to_owned(), FORM.to_owned())])),
    })
    .expect("submit");
    h.settle();
    assert!(h.text(h.editor).contains(FORM), "the form value was typed");

    // Dry-run input, a palette query, a clipboard text.
    h.os.copy(ClipboardContent::text(CLIP));
    ui.call(RequestBody::DryRun {
        typed: Secret::new(format!("{DRY} brb ")),
        now: None,
        clipboard: Some(Secret::new(CLIP.to_owned())),
        values: None,
    })
    .expect("dry run");
    ui.call(RequestBody::Search {
        text: Secret::new(QUERY.to_owned()),
        limit: 3,
    })
    .expect("search");
    drop(ui);
    let dir = h.stop();

    let mut scanned = 0;
    let mut logged = String::new();
    for root in ["logs", "run"] {
        for file in files_under(&dir.path().join(root)) {
            let bytes = fs::read(&file).expect("readable");
            let text = String::from_utf8_lossy(&bytes);
            let found = leaks(&bytes);
            assert!(
                found.is_empty(),
                "{} contains canaries, as: {:?}",
                file.display(),
                found.iter().map(|(_, form)| form).collect::<Vec<_>>()
            );
            if root == "logs" {
                logged.push_str(&text);
            }
            scanned += 1;
        }
    }
    assert!(scanned > 0, "there were files to scan");
    // The log did record the traffic, by name only.
    for expected in [
        "ipc in form_submit",
        "ipc in dry_run",
        "ipc in search",
        "worker ready",
    ] {
        assert!(logged.contains(expected), "the log lacks {expected:?}");
    }
}

/// The detectors see a build that logs every key: the service's own logger, one line per typed
/// character between other lines, as a debugging line in the engine thread would write it.
/// No line holds a whole canary, so the whole-string check alone passes this log.
/// The follow-up review's N3: the same build, writing each key as its decimal code.
#[test]
fn a_log_that_records_every_key_as_a_number_is_caught() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let log = Logger::open(dir.path(), "worker");
    log.info(format_args!("worker ready"));
    let mut seq = 0_u64;
    for canary in ALL {
        for (k, c) in format!("note {canary} ").chars().enumerate() {
            log.info(format_args!("key {} seq {seq}", u32::from(c)));
            seq += 1;
            if k % 4 == 1 {
                log.info(format_args!("ipc out status 243 bytes {seq} us"));
            }
        }
    }
    drop(log);
    let bytes = fs::read(dir.path().join("worker.log")).expect("log");
    let found = leaks(&bytes);
    for canary in ALL {
        assert!(
            found
                .iter()
                .any(|(c, form)| *c == canary && form == "per-line-decimal"),
            "a canary logged one key per line as a number is found: {found:?}"
        );
    }
}

#[test]
fn a_log_that_records_every_key_is_caught() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let log = Logger::open(dir.path(), "worker");
    log.info(format_args!("worker ready"));
    let mut seq = 0_u64;
    for canary in ALL {
        for (k, c) in format!("note {canary} ").chars().enumerate() {
            log.info(format_args!("key {c:?} seq {seq}"));
            seq += 1;
            if k % 4 == 1 {
                log.info(format_args!("ipc in get_status 55 bytes"));
                log.info(format_args!("ipc out status 243 bytes {seq} us"));
            }
        }
    }
    drop(log);
    let bytes = fs::read(dir.path().join("worker.log")).expect("log");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        ALL.iter().all(|c| !text.contains(c)),
        "no whole canary in the log"
    );
    let found = leaks(&bytes);
    for canary in ALL {
        assert!(
            found
                .iter()
                .any(|(c, form)| *c == canary && form == "per-line"),
            "a canary logged one key per line is found"
        );
    }
}
