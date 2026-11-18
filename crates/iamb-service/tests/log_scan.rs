//! Logs never contain typed text (architecture §8): canary strings typed through the mock
//! keyboard, sent as form values, dry-run input, palette queries and clipboard text must not
//! appear in any file under the data root's `logs/` directory, nor in `run/`.

mod common;

use std::collections::BTreeMap;
use std::fs;

use common::{Harness, LIBRARY, files_under, wait_event};
use iamb_ipc::{EventBody, RequestBody, Role, Secret};
use iamb_platform::ClipboardContent;
use iamb_platform::mock::{InjectionPoint, MockKey, MockWindow};

const TYPED: &str = "zqxtypedcanary";
const AHEAD: &str = "zqxahead";
const SECRET_FIELD: &str = "zqxpasswordcanary";
const FORM: &str = "zqxformcanary";
const DRY: &str = "zqxdrycanary";
const QUERY: &str = "zqxquerycanary";
const CLIP: &str = "zqxclipcanary";

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
            for canary in [TYPED, AHEAD, SECRET_FIELD, FORM, DRY, QUERY, CLIP] {
                assert!(
                    !text.contains(canary),
                    "{} contains a canary",
                    file.display()
                );
            }
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
