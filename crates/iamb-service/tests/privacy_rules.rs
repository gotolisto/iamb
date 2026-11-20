//! The privacy rules of `docs/architecture.md` §8 that the code is held to, kept in step with
//! the code: rule 1 must list every place typed characters live (the 2.8 review's finding 9),
//! and the findings of that review left to later tasks must say where each is tracked.

use std::fs;
use std::path::Path;

fn architecture() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    fs::read_to_string(root.join("docs").join("architecture.md")).expect("architecture.md")
}

/// The text of rule 1 of §8, from its number to rule 2, on one line.
fn rule_one(doc: &str) -> String {
    let section = &doc[doc.find("## 8. Privacy and security rules").expect("§8")..];
    let start = section.find("\n1. ").expect("rule 1");
    let end = section[start..].find("\n2. ").expect("rule 2") + start;
    section[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn rule_one_names_every_place_typed_characters_live() {
    let rule = rule_one(&architecture());
    for (what, phrase) in [
        (
            "the key queue",
            "the key queue between the listener and the engine thread",
        ),
        ("the keystroke buffer", "66-character keystroke buffer"),
        ("the hand-off", "typed-characters hand-off"),
        (
            "the hand-off under pause, for explicit requests",
            "made while paused",
        ),
        ("the Windows listener's dead key", "pending dead character"),
        (
            "the Windows listener's half character",
            "outside the Basic Multilingual Plane",
        ),
        (
            "the macOS listener's dead-key state",
            "32-bit dead-key state",
        ),
        ("copies on the stacks", "stack"),
        ("the injector's batch", "the injector's batch"),
    ] {
        assert!(rule.contains(phrase), "rule 1 does not name {what}: {rule}");
    }
    // The old count is gone: the rule lists, it does not claim a number of places in all.
    assert!(!rule.contains("exactly three places"), "{rule}");
    assert!(!rule.contains("exactly two places"), "{rule}");
}

#[test]
fn findings_left_to_later_tasks_say_where_they_are_tracked() {
    let doc = architecture();
    let start = doc
        .find("**Open findings of the Phase 2 privacy review.**")
        .expect("the open findings in §8");
    let table = &doc[start..doc[start..].find("\n## ").map_or(doc.len(), |n| start + n)];
    for (finding, task) in [
        ("| 11,", "Phase 5"),
        ("| 13,", "Task 3.5"),
        // Both panels send `PanelFocus` (tasks 3.5 and 3.6); what is left is the macOS panel
        // function and the check on a Mac.
        ("| 16,", "macOS panel function"),
    ] {
        let row = table
            .lines()
            .find(|line| line.starts_with(finding))
            .unwrap_or_else(|| panic!("no row for finding {finding}"));
        assert!(
            row.contains(task),
            "finding {finding} is not tracked in {task}: {row}"
        );
    }
}
