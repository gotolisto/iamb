//! The sync conflict scenarios of task 4.1 (`spec/sync/scenarios.toml`) are complete: at least
//! 30 of them, unique ids, and a defined outcome for the server and every device (the KPI
//! "every scenario has a defined outcome"), and each outcome agrees with the scenario's own
//! steps and answers. The design document writes out the same scenarios and defines every rule
//! they cite. The check itself is tested on files broken on purpose, so that it is known to
//! fail for each of the reasons it is there for; among them the eight wrong outcomes with which
//! the review of the design showed that the first, structural check accepted anything
//! (`docs/reviews/4.1-sync-design-review.md`, finding 15).

#![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::fs;
use std::path::Path;

use iamb_testkit::sync_scenarios::{
    MIN_SCENARIOS, REQUIRED_COVERAGE, RowOutcome, Scenario, ScenarioFile, load, parse,
    scenarios_path, validate,
};

fn scenarios() -> ScenarioFile {
    load(&scenarios_path()).unwrap()
}

fn design_document() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/sync-design.md");
    fs::read_to_string(path).unwrap()
}

/// `text` with every run of white space as one space, so that a sentence is found whatever
/// way its lines are wrapped.
fn unwrapped(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---- the file ---------------------------------------------------------------------------------

#[test]
fn every_scenario_has_a_defined_outcome() {
    let file = scenarios();
    let problems = validate(&file);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(
        file.scenarios.len() >= MIN_SCENARIOS,
        "only {} scenarios",
        file.scenarios.len()
    );
}

#[test]
fn the_scenarios_are_numbered_without_gaps() {
    let file = scenarios();
    for (index, scenario) in file.scenarios.iter().enumerate() {
        assert_eq!(
            scenario.id,
            format!("S{:02}", index + 1),
            "{}",
            scenario.title
        );
    }
}

#[test]
fn every_outcome_states_what_the_user_is_shown_in_words() {
    for scenario in &scenarios().scenarios {
        assert!(
            scenario.outcome.summary.split_whitespace().count() >= 8,
            "{}: the outcome's summary says too little: {}",
            scenario.id,
            scenario.outcome.summary
        );
    }
}

// ---- the design document ----------------------------------------------------------------------

#[test]
fn the_design_document_writes_out_every_scenario() {
    let doc = unwrapped(&design_document());
    for scenario in &scenarios().scenarios {
        let heading = format!("#### {} · {}", scenario.id, scenario.title);
        assert!(
            doc.contains(&heading),
            "docs/sync-design.md lacks {heading:?}"
        );
        let summary = unwrapped(&scenario.outcome.summary);
        assert!(
            doc.contains(&summary),
            "docs/sync-design.md does not give the outcome of {}: run \
             `python3 spec/sync/render_scenarios.py`",
            scenario.id
        );
    }
}

#[test]
fn the_design_document_defines_every_rule_a_scenario_cites() {
    let doc = design_document();
    for scenario in &scenarios().scenarios {
        for rule in &scenario.rules {
            assert!(
                doc.contains(&format!("**{rule}.**")),
                "{} cites rule {rule}, which docs/sync-design.md does not define",
                scenario.id
            );
        }
    }
}

// ---- the check can fail -----------------------------------------------------------------------

/// A file of `count` copies of one small, valid scenario, numbered from S01, which share the
/// required tags among them so that the set as a whole covers all of them.
fn valid_file(count: usize) -> String {
    let mut text = String::from("version = 2\n");
    for index in 0..count {
        let covers: Vec<String> = REQUIRED_COVERAGE
            .iter()
            .enumerate()
            .filter(|(n, _)| n % count == index)
            .map(|(_, tag)| format!("\"{tag}\""))
            .collect();
        text.push_str(
            &SCENARIO
                .replace("S01", &format!("S{:02}", index + 1))
                .replace(
                    "covers = [\"edit-same-field\"]",
                    &format!("covers = [{}]", covers.join(", ")),
                ),
        );
    }
    text
}

const SCENARIO: &str = r#"
[[scenario]]
id = "S01"
title = "Title S01"
category = "edit"
covers = ["edit-same-field"]
rules = ["C2"]
given = "Two devices in step."

[scenario.start]
users = ["ines"]
devices = [{ id = "A", user = "ines" }, { id = "B", user = "ines" }]
profiles = [{ id = "p-ines", kind = "personal", owner = "ines" }]
rows = [
  { id = "f1", entity = "folder", profile = "p-ines", rev = 3 },
  { id = "s1", entity = "snippet", folder = "f1", rev = 5 },
]

[[scenario.step]]
id = "a1"
at = "A"
do = "edit"
row = "s1"
fields = { body = "new" }
answers = { s1 = "applied" }

[scenario.outcome]
summary = "The edit is on the server and on both devices."

[scenario.outcome.server]
rows = [{ row = "f1", state = "live", version = "start" }, { row = "s1", state = "live", version = "a1" }]
history = [{ row = "s1", version = "start" }]
conflicts = []

[scenario.outcome.device.A]
same_as = "server"
shown = []

[scenario.outcome.device.B]
same_as = "server"
shown = []
"#;

/// The problems of `text`; a file that does not parse counts as one problem.
fn problems(text: &str) -> Vec<String> {
    match parse(text) {
        Ok(file) => validate(&file),
        Err(e) => vec![e],
    }
}

/// Asserts that `text` is refused with a problem that mentions `needle`.
fn refused(text: &str, needle: &str) {
    let found = problems(text);
    assert!(
        found.iter().any(|p| p.contains(needle)),
        "expected a problem mentioning {needle:?}, found {found:?}"
    );
}

#[test]
fn a_valid_file_passes() {
    assert_eq!(problems(&valid_file(MIN_SCENARIOS)), Vec::<String>::new());
}

#[test]
fn fewer_than_thirty_scenarios_fail() {
    refused(&valid_file(MIN_SCENARIOS - 1), "at least 30 are required");
}

#[test]
fn a_duplicated_id_fails() {
    let mut text = valid_file(MIN_SCENARIOS);
    text.push_str(&SCENARIO.replace("Title S01", "Another title"));
    refused(&text, "S01: the id is used twice");
}

#[test]
fn a_device_without_an_outcome_fails() {
    let text = valid_file(MIN_SCENARIOS).replacen(
        "[scenario.outcome.device.B]\nsame_as = \"server\"\nshown = []\n",
        "",
        1,
    );
    refused(&text, "S01: no outcome is defined for device B");
}

#[test]
fn a_missing_server_outcome_fails() {
    let valid = valid_file(MIN_SCENARIOS);
    let start = valid.find("[scenario.outcome.server]").unwrap();
    let end = valid.find("[scenario.outcome.device.A]").unwrap();
    let text = format!("{}{}", &valid[..start], &valid[end..]);
    refused(&text, "missing field `server`");
}

#[test]
fn a_row_the_outcome_does_not_state_fails() {
    // The server's outcome leaves out the snippet; the devices copy the server, so each of
    // the three places is reported.
    let text = valid_file(MIN_SCENARIOS).replacen(
        ", { row = \"s1\", state = \"live\", version = \"a1\" }]",
        "]",
        1,
    );
    let found = problems(&text);
    for place in ["the server", "device A", "device B"] {
        let wanted = format!("S01: {place}: no outcome is defined for s1");
        assert!(found.contains(&wanted), "{wanted:?} not in {found:?}");
    }
}

#[test]
fn an_outcome_without_what_the_user_is_shown_fails() {
    let text = valid_file(MIN_SCENARIOS).replacen(
        "same_as = \"server\"\nshown = []\n",
        "same_as = \"server\"\n",
        1,
    );
    refused(&text, "missing field `shown`");
}

#[test]
fn an_outcome_without_a_summary_fails() {
    let text = valid_file(MIN_SCENARIOS).replacen(
        "summary = \"The edit is on the server and on both devices.\"",
        "summary = \" \"",
        1,
    );
    refused(&text, "S01: the outcome has no summary");
}

#[test]
fn unknown_states_versions_and_answers_fail() {
    let base = valid_file(MIN_SCENARIOS);
    refused(
        &base.replacen(
            "state = \"live\", version = \"a1\"",
            "state = \"merged\", version = \"a1\"",
            1,
        ),
        "unknown state merged",
    );
    refused(
        &base.replacen(
            "state = \"live\", version = \"a1\"",
            "state = \"live\", version = \"zz\"",
            1,
        ),
        "names the unknown step zz",
    );
    refused(
        &base.replacen(
            "state = \"live\", version = \"a1\"",
            "state = \"absent\", version = \"a1\"",
            1,
        ),
        "the absent row s1 has a version",
    );
    refused(
        &base.replacen(
            "answers = { s1 = \"applied\" }",
            "answers = { s1 = \"maybe\" }",
            1,
        ),
        "unknown push answer maybe",
    );
    refused(
        &base.replacen("row = \"s1\"\nfields", "row = \"s9\"\nfields", 1),
        "row s9 does not exist yet",
    );
}

/// One scenario of the valid file with a recorded conflict: B goes offline, A's edit reaches
/// the server, and B's edit, made without it, arrives last and wins.
fn with_conflict(text: &str) -> String {
    text.replacen(
        "[[scenario.step]]\nid = \"a1\"",
        "[[scenario.step]]\nat = \"B\"\ndo = \"offline\"\n\n[[scenario.step]]\nid = \"a1\"",
        1,
    )
    .replacen(
        "answers = { s1 = \"applied\" }\n",
        "answers = { s1 = \"applied\" }\n\n[[scenario.step]]\nid = \"b1\"\nat = \"B\"\ndo = \"edit\"\n\
         row = \"s1\"\nfields = { body = \"other\" }\n\n[[scenario.step]]\nat = \"B\"\n\
         do = \"online\"\nanswers = { s1 = \"applied+conflict\" }\n",
        1,
    )
    .replacen(
        "same_as = \"server\"\nshown = []\n",
        "same_as = \"server\"\nshown = [\"conflict\"]\n",
        2,
    )
    .replacen("state = \"live\", version = \"a1\"", "state = \"live\", version = \"b1\"", 1)
    .replacen(
        "history = [{ row = \"s1\", version = \"start\" }]\nconflicts = []",
        "history = [{ row = \"s1\", version = \"start\" }, { row = \"s1\", version = \"a1\" }]\n\
         conflicts = [{ row = \"s1\", kind = \"edit_edit\", winner = \"b1\", loser = \"a1\", \
         parties = [\"ines\"], state = \"open\" }]",
        1,
    )
}

#[test]
fn a_conflict_must_agree_with_the_rows_and_the_history() {
    let base = with_conflict(&valid_file(MIN_SCENARIOS));
    assert_eq!(problems(&base), Vec::<String>::new());
    // The version the conflict offers back is not kept.
    refused(
        &base.replacen(", { row = \"s1\", version = \"a1\" }]", "]", 1),
        "S01: the conflict on s1 offers a1 back, which the history does not hold",
    );
    // The winner is not what the row is.
    refused(
        &base.replacen(
            "winner = \"b1\", loser = \"a1\"",
            "winner = \"a1\", loser = \"b1\"",
            1,
        ),
        "S01: the open conflict on s1 names a winner that is not the row's version",
    );
    // The history lists the version the row has.
    refused(
        &base.replacen(
            "{ row = \"s1\", version = \"a1\" }]",
            "{ row = \"s1\", version = \"a1\" }, { row = \"s1\", version = \"b1\" }]",
            1,
        ),
        "S01: the history holds b1 of s1, which is the row's current version",
    );
    // A change kept on a device that did not make it.
    refused(
        &base.replacen(
            "same_as = \"server\"\nshown = [\"conflict\"]\n",
            "same_as = \"server\"\nshown = [\"conflict\"]\nkept = [\"b1\"]\n",
            1,
        ),
        "S01: device A: kept or blocked names b1, not a step of this device",
    );
    // The same conflict when B had seen A's edit: nothing was replaced unseen.
    refused(
        &base.replacen(
            "at = \"B\"\ndo = \"offline\"",
            "at = \"B\"\ndo = \"pull\"",
            1,
        ),
        "S01: the conflict on s1: the device that wrote b1 had seen a1",
    );
}

#[test]
fn a_required_case_that_no_scenario_covers_fails() {
    // 30 scenarios share the required tags; with the last tag renamed everywhere it is
    // missing.
    let last = REQUIRED_COVERAGE[REQUIRED_COVERAGE.len() - 1];
    let text = valid_file(MIN_SCENARIOS).replace(last, "something-else");
    refused(&text, &format!("no scenario covers {last}"));
}

#[test]
fn an_unknown_field_fails() {
    let text = valid_file(MIN_SCENARIOS).replacen("given = ", "givn = ", 1);
    refused(&text, "unknown field `givn`");
}

// ---- the outcome agrees with the steps and the answers ------------------------------------------

/// The problems the check finds in the real scenarios once `alter` has changed one of them.
fn altered(id: &str, alter: impl FnOnce(&mut Scenario)) -> Vec<String> {
    let mut file = scenarios();
    let scenario = file.scenarios.iter_mut().find(|s| s.id == id).unwrap();
    alter(scenario);
    validate(&file)
}

/// Asserts that one of `found` mentions `needle`.
fn caught(found: &[String], needle: &str) {
    assert!(
        found.iter().any(|p| p.contains(needle)),
        "expected a problem mentioning {needle:?}, found {found:?}"
    );
}

fn row(name: &str, state: &str, version: Option<&str>) -> RowOutcome {
    RowOutcome {
        row: name.to_owned(),
        state: state.to_owned(),
        version: version.map(str::to_owned),
        fields: std::collections::BTreeMap::new(),
    }
}

/// Replaces the stated outcome of one row in a list.
fn restate(rows: &mut Vec<RowOutcome>, new: RowOutcome) {
    rows.retain(|r| r.row != new.row);
    rows.push(new);
}

// The eight wrong outcomes of the review (finding 15), each one change to a scenario of the
// real file. The structural check of the first design accepted all eight.

#[test]
fn review_1_a_replaced_version_the_server_did_not_keep_fails() {
    let found = altered("S01", |s| {
        s.outcome
            .server
            .history
            .retain(|v| !(v.row == "s1" && v.version == "start"));
    });
    caught(
        &found,
        "S01: the history lacks start of s1, which a later write replaced",
    );
}

#[test]
fn review_2_a_conflict_kind_that_contradicts_the_writes_fails() {
    let found = altered("S03", |s| {
        s.outcome.server.conflicts[0].kind = "edit_over_delete".to_owned();
    });
    caught(
        &found,
        "S03: the conflict on s1 is recorded as edit_over_delete, but b1 over a1 is \
         delete_over_edit",
    );
}

#[test]
fn review_3_a_refused_write_that_became_the_server_row_fails() {
    let found = altered("S31", |s| {
        restate(&mut s.outcome.server.rows, row("s1", "live", Some("c1")));
    });
    caught(
        &found,
        "S31: the server's s1 is at c1, but the last write that reached the server is start",
    );
}

#[test]
fn review_4_refused_changes_kept_nowhere_fail() {
    let found = altered("S30", |s| {
        s.outcome.device.get_mut("C").unwrap().kept.clear();
    });
    caught(
        &found,
        "S30: C ends online with its change c1 to s1 unanswered, and neither kept nor blocked: \
         a lost edit",
    );
    caught(&found, "its change c2 to s5 unanswered");
}

#[test]
fn review_5_two_online_devices_of_one_account_that_differ_fail() {
    let found = altered("S01", |s| {
        let device = s.outcome.device.get_mut("B").unwrap();
        device.rows.push(row("s1", "live", Some("a1")));
    });
    caught(
        &found,
        "S01: B ends online and holds s1 as live · a1, which is not what the server holds",
    );
    caught(
        &found,
        "S01: A and B are devices of ines, both online at the end, and differ on s1",
    );
}

#[test]
fn review_6_a_version_from_a_step_that_never_wrote_the_row_fails() {
    let found = altered("S22", |s| {
        restate(&mut s.outcome.server.rows, row("s3", "live", Some("b1")));
    });
    caught(
        &found,
        "S22: the server: s3 is at b1, a step that does not write that row",
    );
    caught(
        &found,
        "S22: the server's s3 is at b1, but the last write that reached the server is b2",
    );
}

#[test]
fn review_7_an_open_conflict_shown_to_nobody_fails() {
    let found = altered("S01", |s| {
        for device in s.outcome.device.values_mut() {
            device.shown.clear();
        }
    });
    caught(
        &found,
        "S01: A: the open conflict on s1 concerns its user and is not shown",
    );
    caught(
        &found,
        "S01: B: the open conflict on s1 concerns its user and is not shown",
    );
}

#[test]
fn review_8_an_unseen_row_deleted_with_its_folder_without_a_conflict_fails() {
    let found = altered("S16", |s| s.outcome.server.conflicts.clear());
    caught(
        &found,
        "S16: a1 replaced b1 of s3, which was written elsewhere and which its writer had not \
         seen, and no conflict offers b1 back: a lost edit",
    );
}

// Further ways in which an outcome can contradict its scenario.

#[test]
fn a_refused_change_the_device_does_not_keep_fails() {
    let found = altered("S31", |s| {
        s.outcome.device.get_mut("C").unwrap().kept.clear();
    });
    caught(
        &found,
        "S31: C: the server refused c1, and the device does not keep it for the user",
    );
}

#[test]
fn a_blocked_change_must_stay_listed_and_in_the_outbox() {
    let found = altered("S45", |s| {
        let device = s.outcome.device.get_mut("A").unwrap();
        device.blocked.clear();
        device.outbox = 0;
    });
    caught(
        &found,
        "S45: A: a2 was answered blocked and is not listed as blocked",
    );
    caught(
        &found,
        "S45: A: the outbox is stated to hold 0 changes, but 1 are under way, waiting or blocked",
    );
}

#[test]
fn a_device_that_lost_the_row_it_wrote_fails() {
    // Both devices of the account state the snippet as absent, so they agree with each other
    // and "as the server has it or not at all" holds; but B wrote the version the server has.
    let found = altered("S01", |s| {
        for device in s.outcome.device.values_mut() {
            device.rows.push(row("s1", "absent", None));
        }
    });
    caught(
        &found,
        "S01: B wrote the server's version of s1 and ends online without the row",
    );
}

#[test]
fn a_row_absent_on_the_server_without_a_purge_fails() {
    let found = altered("S05", |s| {
        restate(&mut s.outcome.server.rows, row("s1", "absent", None));
    });
    caught(
        &found,
        "S05: s1 is absent on the server, but a1 reached it and no purge is due",
    );
}

#[test]
fn a_deleted_row_stated_as_live_fails() {
    let found = altered("S05", |s| {
        restate(&mut s.outcome.server.rows, row("s1", "live", Some("a1")));
    });
    caught(
        &found,
        "S05: the server's s1 is live, but a1 left it deleted",
    );
}

#[test]
fn a_history_entry_nothing_replaced_fails() {
    let found = altered("S05", |s| {
        // The second delete was answered `unchanged`: it wrote nothing and replaced nothing.
        restate(&mut s.outcome.server.rows, row("s1", "deleted", Some("b1")));
    });
    caught(
        &found,
        "S05: the server's s1 is at b1, but the last write that reached the server is a1",
    );
}

#[test]
fn a_replaced_conflict_must_hand_its_loser_on() {
    let found = altered("S57", |s| s.outcome.server.conflicts[1].carried.clear());
    caught(
        &found,
        "S57: the replaced conflict on s1 offered c1 back, and no later conflict on the row \
         takes it over",
    );
}

#[test]
fn a_conflict_must_be_shown_to_the_authors_of_its_versions() {
    let found = altered("S55", |s| {
        s.outcome.server.conflicts[0].parties = vec!["ines".to_owned()];
    });
    caught(&found, "S55: the conflict on s1 is shown to");
}

#[test]
fn an_answer_without_a_change_fails() {
    let text = valid_file(MIN_SCENARIOS).replacen(
        "answers = { s1 = \"applied\" }",
        "answers = { s1 = \"applied\", f1 = \"applied\" }",
        1,
    );
    refused(
        &text,
        "S01: A is answered applied for f1, to which it has no unsent change",
    );
}

#[test]
fn a_cascade_must_name_known_rows_and_its_rows_carry_the_step() {
    let base = valid_file(MIN_SCENARIOS);
    refused(
        &base.replacen(
            "fields = { body = \"new\" }",
            "fields = { body = \"new\" }\ncascade = [\"s9\"]",
            1,
        ),
        "S01: step 1: `cascade` names the unknown row s9",
    );
    // A write that takes the folder with it leaves the folder at that step's version.
    refused(
        &base.replacen(
            "fields = { body = \"new\" }",
            "fields = { body = \"new\" }\ncascade = [\"f1\"]",
            1,
        ),
        "S01: the server's f1 is at start, but the last write that reached the server is a1",
    );
}

#[test]
fn an_invitation_has_no_revision_and_no_history() {
    let found = altered("S32", |s| {
        s.start
            .rows
            .iter_mut()
            .find(|r| r.id == "inv1")
            .unwrap()
            .rev = 6;
    });
    caught(
        &found,
        "S32: the invitation inv1 has a revision; invitations have none (rev = 0)",
    );
    let found = altered("S32", |s| {
        let mut kept = s.outcome.server.history.clone();
        kept.push(iamb_testkit::sync_scenarios::VersionRef {
            row: "inv1".to_owned(),
            version: "start".to_owned(),
        });
        s.outcome.server.history = kept;
    });
    caught(
        &found,
        "S32: the history holds the invitation inv1; invitations have no history",
    );
}
