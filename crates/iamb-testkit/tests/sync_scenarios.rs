//! The sync conflict scenarios of task 4.1 (`spec/sync/scenarios.toml`) are complete: at least
//! 30 of them, unique ids, and a defined outcome for the server and every device (the KPI
//! "every scenario has a defined outcome"). The design document writes out the same scenarios
//! and defines every rule they cite. The check itself is tested on files broken on purpose, so
//! that it is known to fail for each of the reasons it is there for.

#![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

use std::fs;
use std::path::Path;

use iamb_testkit::sync_scenarios::{
    MIN_SCENARIOS, REQUIRED_COVERAGE, ScenarioFile, load, parse, scenarios_path, validate,
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

/// A file of `count` copies of one small, valid scenario, numbered from S01, each covering
/// one of the required tags in turn so that the set as a whole covers all of them.
fn valid_file(count: usize) -> String {
    let mut text = String::from("version = 1\n");
    for index in 0..count {
        let cover = REQUIRED_COVERAGE[index % REQUIRED_COVERAGE.len()];
        text.push_str(
            &SCENARIO
                .replace("S01", &format!("S{:02}", index + 1))
                .replace(
                    "covers = [\"edit-same-field\"]",
                    &format!("covers = [\"{cover}\"]"),
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
    let start = SCENARIO.find("[scenario.outcome.server]").unwrap();
    let end = SCENARIO.find("[scenario.outcome.device.A]").unwrap();
    let broken = format!("{}{}", &SCENARIO[..start], &SCENARIO[end..]);
    let text = valid_file(MIN_SCENARIOS).replacen(SCENARIO, &broken, 1);
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

/// One scenario of the valid file with a recorded conflict: B's stale edit wins over A's.
fn with_conflict(text: &str) -> String {
    text.replacen(
        "answers = { s1 = \"applied\" }\n",
        "answers = { s1 = \"applied\" }\n\n[[scenario.step]]\nid = \"b1\"\nat = \"B\"\ndo = \"edit\"\n\
         row = \"s1\"\nfields = { body = \"other\" }\nanswers = { s1 = \"applied+conflict\" }\n",
        1,
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
            "same_as = \"server\"\nshown = []\n",
            "same_as = \"server\"\nshown = []\nkept = [\"b1\"]\n",
            1,
        ),
        "S01: device A: kept or blocked names b1, not a step of this device",
    );
}

#[test]
fn a_required_case_that_no_scenario_covers_fails() {
    // 30 scenarios cover the 27 required tags; with the last tag renamed everywhere it is
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
