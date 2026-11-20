//! Usage statistics and library health through the service endpoint (task 3.10, spec §4.9 and
//! §11.2 entry 4): an expansion counts once and only on this computer, the counts come back in
//! the list answer and in the recently used list, a reset forgets them, and health reports the
//! planted problems.

mod common;

use common::{Entry, Harness, LIBRARY, id};
use iamb_ipc::{Client, ErrorCode, HealthReport, RequestBody, ResponseBody, Role, SnippetQuery};
use iamb_store::Store;

fn call(c: &mut Client, body: RequestBody) -> ResponseBody {
    c.call(body).expect("answered")
}

fn uses(c: &mut Client) -> Vec<(u8, Option<u64>, bool)> {
    match call(
        c,
        RequestBody::ListSnippetSummaries {
            query: SnippetQuery::default(),
            offset: 0,
            limit: 100,
        },
    ) {
        ResponseBody::SnippetSummaries { summaries, .. } => {
            let mut v: Vec<_> = summaries
                .iter()
                .map(|s| (s.id.as_bytes()[0], s.uses, s.last_used.is_some()))
                .collect();
            v.sort_unstable();
            v
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn recent(c: &mut Client, limit: u32) -> Vec<u8> {
    match call(c, RequestBody::RecentSnippets { limit }) {
        ResponseBody::SearchResults { hits } => {
            assert!(
                hits.iter()
                    .all(|h| h.field.is_none() && h.ranges.is_empty())
            );
            hits.iter().map(|h| h.snippet.as_bytes()[0]).collect()
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn health(c: &mut Client) -> HealthReport {
    match call(c, RequestBody::Health {}) {
        ResponseBody::Health { report } => report,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn an_expansion_counts_once_and_is_reported_in_the_list_and_the_recent_list() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    assert_eq!(
        uses(&mut ui),
        [
            (1, Some(0), false),
            (2, Some(0), false),
            (3, Some(0), false),
            (4, Some(0), false)
        ]
    );
    assert_eq!(recent(&mut ui, 8), Vec::<u8>::new());

    h.os.type_text("brb ");
    h.settle();
    h.os.type_text("omw.");
    h.settle();
    h.os.type_text("brb ");
    h.settle();
    h.worker().settle();
    // The usage record is made by the store thread; `Health` goes through it, so once it has
    // answered the records before it are in.
    let _ = health(&mut ui);
    assert_eq!(
        uses(&mut ui),
        [
            (1, Some(2), true),
            (2, Some(1), true),
            (3, Some(0), false),
            (4, Some(0), false)
        ]
    );
    // Newest first.
    assert_eq!(recent(&mut ui, 8), [1, 2]);
    assert_eq!(recent(&mut ui, 1), [1]);
    assert_eq!(
        ui.call(RequestBody::RecentSnippets { limit: 0 })
            .err()
            .and_then(|e| e.remote_code()),
        Some(ErrorCode::Invalid)
    );
}

#[test]
fn usage_is_not_a_sync_change_and_a_reset_forgets_it() {
    let h = Harness::start(LIBRARY);
    let mut ui = h.client(Role::Ui);
    h.os.type_text("brb ");
    h.settle();
    let _ = health(&mut ui);
    assert_eq!(uses(&mut ui)[0], (1, Some(1), true));

    assert!(matches!(
        call(&mut ui, RequestBody::ResetUsage {}),
        ResponseBody::Done {}
    ));
    assert_eq!(
        uses(&mut ui),
        [
            (1, Some(0), false),
            (2, Some(0), false),
            (3, Some(0), false),
            (4, Some(0), false)
        ]
    );
    assert_eq!(recent(&mut ui, 8), Vec::<u8>::new());
    // The CLI may not reset: it is a Settings request.
    let mut cli = h.client(Role::Cli);
    assert_eq!(
        cli.call(RequestBody::ResetUsage {})
            .err()
            .and_then(|e| e.remote_code()),
        Some(ErrorCode::NotPermitted)
    );

    let dir = h.stop();
    let store = Store::open(dir.path()).expect("store free after stop");
    assert_eq!(store.list_usage().expect("usage"), Vec::new());
    assert!(store.usage_reset_at().expect("reset time").is_some());
    // A library seeded the same way and never used has the same outbox: neither the
    // expansion nor the reset wrote a sync change.
    let untouched = tempfile::TempDir::new().expect("temp dir");
    common::seed(untouched.path(), LIBRARY);
    let reference = Store::open(untouched.path()).expect("store");
    assert_eq!(
        store.outbox_len().expect("outbox"),
        reference.outbox_len().expect("outbox")
    );
}

const PLANTED: &[Entry] = &[
    (1, ";ooo", "Away. {snippet abbr=\";sig2\"}", false),
    (2, ";sig", "Kind regards", false),
    (3, ";rl", "later", true),
    (4, ";rlate", "Late delivery", false),
    (5, ";a", "{snippet abbr=\";b\"}", false),
    (6, ";b", "{snippet abbr=\";a\"}", false),
    (7, ";bad", "oops {date", false),
];

#[test]
fn health_reports_the_planted_problems_with_what_they_point_at() {
    let h = Harness::start(PLANTED);
    let mut ui = h.client(Role::Ui);
    let report = health(&mut ui);
    let mut errors: Vec<(u8, String, Option<String>)> = report
        .excluded
        .iter()
        .map(|e| {
            (
                e.snippet.as_bytes()[0],
                e.reason.clone(),
                e.reference.clone(),
            )
        })
        .collect();
    errors.sort();
    assert_eq!(
        errors,
        [
            (
                1,
                "E_SNIPPET_NOT_FOUND".to_owned(),
                Some(";sig2".to_owned())
            ),
            (5, "E_SNIPPET_CYCLE".to_owned(), Some(";b".to_owned())),
            (6, "E_SNIPPET_CYCLE".to_owned(), Some(";a".to_owned())),
            (7, "E_UNCLOSED_MACRO".to_owned(), None),
        ]
    );
    assert_eq!(report.shadowed.len(), 1);
    assert_eq!(
        (report.shadowed[0].by, report.shadowed[0].hidden),
        (id(3), id(4))
    );
    // Made long ago (the seed's clock is 1970) and never expanded: all but the reported ones.
    let mut unused: Vec<u8> = report
        .unused
        .iter()
        .map(|u| u.snippet.as_bytes()[0])
        .collect();
    unused.sort_unstable();
    assert_eq!(unused, [2, 3]);
    assert!(report.unused.iter().all(|u| u.last_used.is_none()));

    // The list says the same through each row's `broken`, and the reset starts the 90 days
    // again, so nothing is unused right after it.
    assert!(matches!(
        call(&mut ui, RequestBody::ResetUsage {}),
        ResponseBody::Done {}
    ));
    assert_eq!(health(&mut ui).unused, Vec::new());
}
