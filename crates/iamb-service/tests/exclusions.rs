//! App exclusions are stored and compared in the one form the focus inspector reports (the
//! 2.8 review's finding 6): another spelling of the same application still applies, and an id
//! that can never be reported is refused instead of stored.

mod common;

use common::{Harness, LIBRARY, diagnostic_codes, seed};
use iamb_core::model::Timestamp;
use iamb_ipc::{Client, ErrorCode, IpcError, RequestBody, ResponseBody, Role, WireExclusion};
use iamb_store::{AppExclusion, Store};

fn put(c: &mut Client, app_id: &str, excluded: bool) -> Result<Vec<WireExclusion>, IpcError> {
    c.call(RequestBody::PutExclusion {
        app_id: app_id.to_owned(),
        display_name: "Editor".to_owned(),
        excluded,
    })
    .map(|answer| match answer {
        ResponseBody::Exclusions { exclusions } => exclusions,
        other => panic!("unexpected {other:?}"),
    })
}

fn listed(c: &mut Client) -> Vec<String> {
    match c.call(RequestBody::ListExclusions {}) {
        Ok(ResponseBody::Exclusions { exclusions }) => {
            exclusions.into_iter().map(|e| e.app_id).collect()
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn refused(result: Result<Vec<WireExclusion>, IpcError>) -> bool {
    matches!(result, Err(IpcError::Remote(e)) if e.code == ErrorCode::Invalid)
}

#[test]
fn an_exclusion_in_another_spelling_applies_and_is_answered_as_stored() {
    let h = Harness::start(LIBRARY);
    let mut c = h.client(Role::Ui);
    let stored = put(&mut c, "  Org.Example.EDITOR ", true).expect("stored");
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].app_id, "org.example.editor",
        "the form that is compared"
    );
    h.worker().sync();
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb ", "the editor is excluded");
    assert!(diagnostic_codes(&mut c).contains(&"suspended_excluded_app".to_owned()));
    assert_eq!(listed(&mut c), ["org.example.editor"]);

    // Removed in yet another spelling.
    let removed = put(&mut c, "ORG.EXAMPLE.EDITOR", false).expect("removed");
    assert_eq!(removed.len(), 1);
    assert!(listed(&mut c).is_empty());
    assert!(
        put(&mut c, "org.example.editor", false)
            .expect("nothing to remove")
            .is_empty()
    );
    h.worker().sync();
    h.os.type_text("omw ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb on my way ");
}

#[test]
fn an_id_that_can_never_be_reported_is_refused() {
    let h = Harness::start(LIBRARY);
    let mut c = h.client(Role::Ui);
    for id in ["", "   ", "org.example\u{7}editor"] {
        assert!(refused(put(&mut c, id, true)), "{id:?}");
    }
    #[cfg(target_os = "macos")]
    {
        assert!(refused(put(&mut c, "/Applications/Bank.app", true)));
        // A program without a bundle identifier is reported by its file name, which may hold
        // a space (the follow-up review's N2): it can be excluded.
        let stored = put(&mut c, "Bank App", true).expect("stored");
        assert_eq!(stored[0].app_id, "bank app");
        assert!(put(&mut c, "Bank App", false).is_ok());
    }
    #[cfg(windows)]
    {
        let stored = put(&mut c, r"C:\Program Files\Bank\MyBank.EXE", true).expect("stored");
        assert_eq!(stored[0].app_id, "mybank.exe");
        assert!(refused(put(&mut c, "?", true)));
        assert!(refused(put(&mut c, "bank*.exe", true)));
    }
    #[cfg(not(windows))]
    assert!(listed(&mut c).is_empty());
}

#[test]
fn exclusions_stored_in_another_spelling_are_rewritten_at_start() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    seed(dir.path(), LIBRARY);
    {
        let mut store = Store::open(dir.path()).expect("store opens");
        for app_id in [
            "Org.Example.Editor",
            "ORG.EXAMPLE.EDITOR",
            "org.example\u{1}x",
        ] {
            store
                .put_app_exclusion(&AppExclusion {
                    app_id: app_id.to_owned(),
                    display_name: "Editor".to_owned(),
                    created_at: Timestamp(5),
                })
                .expect("stored");
        }
    }
    let h = Harness::start_in(dir, |_| {});
    let mut c = h.client(Role::Ui);
    let mut ids = listed(&mut c);
    ids.sort();
    // The two spellings became one row; the id that can never match is left for the user to
    // remove, and can be.
    assert_eq!(ids, ["org.example\u{1}x", "org.example.editor"]);
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb ", "the editor is excluded");
    let removed = put(&mut c, "org.example\u{1}x", false).expect("removed");
    assert_eq!(removed.len(), 1);
    assert_eq!(listed(&mut c), ["org.example.editor"]);
}
