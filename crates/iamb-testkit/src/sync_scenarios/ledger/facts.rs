//! What a scenario's own data say about the content of each version of a row, and the checks
//! that follow from it.
//!
//! The ledger follows versions; this module follows what is in them, as far as the steps say
//! it: the fields a step wrote (a version holds the fields of the version its device held,
//! with the step's on top, because a write carries the whole row, C2), the folder a move put a
//! row in, and who wrote a snippet's content (C17: the writer of a step that changed the
//! abbreviation, kind, body, trigger, case mode or word-start flag, otherwise the author of the
//! version the device held). From that it requires:
//!
//! - a field an outcome states for a row is what the steps left in that version;
//! - a snippet whose content someone else wrote, and that must be reviewed (D10), is listed as
//!   held on every device of a reader that has not approved it, and on no device of its author;
//! - of several snippets with one abbreviation on a device, the one stated to expand is not
//!   behind another under D13, as far as the scenario decides it: content the user wrote before
//!   content of others in the user's own profile, the user's own profile before every other,
//!   and inside the user's own profile the row the server accepted first;
//! - a device that ends online holds every live row of its user's own personal profile.
//!
//! Nothing here is derived from the rules of the design beyond those three facts. What an
//! account call writes into a row, what an adoption makes of a local row and where the server
//! puts a folder it had to place elsewhere are not known here, and such versions are skipped.
//!
//! The module also reads off the scenario which group each membership and share belongs to
//! (`row_groups`), from which the ledger tells which profiles a user's memberships and shares
//! reach.

use std::collections::BTreeMap;

use super::Ledger;
use crate::sync_scenarios::{Refs, RowOutcome, Scenario, Step};

/// The fields that decide what a snippet does (C17).
const CONTENT_FIELDS: [&str; 7] = [
    "abbreviation",
    "kind",
    "body",
    "trigger",
    "trigger_mode",
    "case_mode",
    "word_start",
];

/// What a template holds when its snippet waits for review after someone else wrote it (D10).
const REVIEWED: [&str; 5] = ["{key", "{wait", "{clipboard", "{snippet", "\n"];

type Outcomes<'a> = BTreeMap<&'a str, Vec<RowOutcome>>;

/// What one version of a row holds.
#[derive(Clone, Default)]
pub(super) struct Facts<'a> {
    /// The fields the scenario has stated, and the folder or parent the row is in.
    pub(super) fields: BTreeMap<String, String>,
    /// The account that wrote a snippet's content.
    author: Option<&'a str>,
    /// The step that last changed that content, or `start`.
    content: &'a str,
}

/// A field's value as text, so that a row's id and a string compare alike.
fn render(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The fields of a row as a step or the start states them, with where the row is.
fn stated(fields: &BTreeMap<String, toml::Value>, refs: Refs<'_>) -> BTreeMap<String, String> {
    let mut found: BTreeMap<String, String> = fields
        .iter()
        .map(|(key, value)| (key.clone(), render(value)))
        .collect();
    for (key, target) in [("folder", refs.folder), ("parent", refs.parent)] {
        if let Some(target) = target {
            found.insert(key.to_owned(), target.to_owned());
        }
    }
    found
}

/// Row → the profile that owns it, for the rows of the start and those the steps create. A row
/// never changes its profile (C8).
pub(super) fn row_profiles(scenario: &Scenario) -> BTreeMap<&str, &str> {
    let mut rows: Vec<(&str, &str, Refs<'_>)> = scenario
        .start
        .rows
        .iter()
        .map(|row| (row.id.as_str(), row.entity.as_str(), row.refs()))
        .collect();
    for step in &scenario.steps {
        match (step.action.as_str(), &step.row, &step.entity, &step.creates) {
            ("create", Some(row), Some(entity), _) => rows.push((row, entity, step.refs())),
            ("copy", _, _, Some(row)) => {
                let refs = Refs {
                    folder: step.to.as_deref(),
                    ..Refs::default()
                };
                rows.push((row, "snippet", refs));
            }
            _ => {}
        }
    }
    let mut profiles = BTreeMap::new();
    // A row may refer to one listed after it; three passes reach a link of a snippet of a
    // folder.
    for _ in 0..3 {
        for (row, entity, refs) in &rows {
            let profile = match *entity {
                "folder" | "tag" | "attachment" => refs.profile,
                "snippet" | "folder_share" => refs.folder.and_then(|f| profiles.get(f).copied()),
                "snippet_tag" | "snippet_attachment" => {
                    refs.snippet.and_then(|s| profiles.get(s).copied())
                }
                _ => None,
            };
            if let Some(profile) = profile {
                profiles.insert(*row, profile);
            }
        }
    }
    profiles
}

/// Membership or share → its group, and membership → its account, for the rows of the start and
/// those the steps create. A membership is created by the acceptance of an invitation, and has
/// that invitation's group and account.
pub(super) fn row_groups(scenario: &Scenario) -> (BTreeMap<&str, &str>, BTreeMap<&str, &str>) {
    let mut groups = BTreeMap::new();
    let mut members = BTreeMap::new();
    // Invitation → its group and the account invited.
    let mut invited: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
    for row in &scenario.start.rows {
        let (Some(group), id) = (row.group.as_deref(), row.id.as_str()) else {
            continue;
        };
        match (row.entity.as_str(), row.user.as_deref()) {
            ("folder_share", _) => {
                groups.insert(id, group);
            }
            ("membership", Some(user)) => {
                groups.insert(id, group);
                members.insert(id, user);
            }
            ("invitation", Some(user)) => {
                invited.insert(id, (group, user));
            }
            _ => {}
        }
    }
    for step in &scenario.steps {
        let (group, user) = (step.group.as_deref(), step.user.as_deref());
        match (step.action.as_str(), &step.row, &step.creates) {
            ("create", Some(row), _) if step.entity.as_deref() == Some("folder_share") => {
                if let Some(group) = group {
                    groups.insert(row.as_str(), group);
                }
            }
            ("invite", _, Some(row)) => {
                if let (Some(group), Some(user)) = (group, user) {
                    invited.insert(row.as_str(), (group, user));
                }
            }
            ("accept", Some(invitation), Some(row)) => {
                if let Some((group, user)) = invited.get(invitation.as_str()) {
                    groups.insert(row.as_str(), group);
                    members.insert(row.as_str(), user);
                }
            }
            _ => {}
        }
    }
    (groups, members)
}

impl<'a> Ledger<'a> {
    // ---- the walk -------------------------------------------------------------------------------

    /// The account that owns `profile` when it is a personal profile on the server.
    pub(super) fn personal_owner(&self, profile: &str) -> Option<&'a str> {
        let found = self
            .scenario
            .start
            .profiles
            .iter()
            .find(|p| p.id == profile && p.kind == "personal" && p.local_to.is_none())?;
        Some(&found.owner)
    }

    /// What the rows of the start hold.
    pub(super) fn start_facts(&mut self) {
        let scenario = self.scenario;
        for row in &scenario.start.rows {
            let owner = self.profiles.get(row.id.as_str()).and_then(|profile| {
                let found = scenario.start.profiles.iter().find(|p| p.id == *profile)?;
                Some(found.owner.as_str())
            });
            let facts = Facts {
                fields: stated(&row.fields, row.refs()),
                author: row.by.as_deref().or(owner),
                content: "start",
            };
            self.facts.insert((&row.id, "start"), facts);
        }
    }

    /// The version of `row` that `device` holds: its own unsent change, or what it has seen.
    fn device_version(&self, device: &str, row: &str) -> Option<&'a str> {
        let key = (device, row);
        if let Some(change) = self.waiting.get(&key).or_else(|| self.flying.get(&key)) {
            return Some(change.id);
        }
        self.seen.get(device)?.get(row).copied()
    }

    /// What `device` holds of `row`, if the scenario says.
    fn device_facts(&self, device: &str, row: &'a str) -> Option<Facts<'a>> {
        let version = self.device_version(device, row)?;
        self.facts.get(&(row, version)).cloned()
    }

    /// A change made on a device: the version `id` of `row` holds what the device held, with
    /// the step's change on top. Called before the change is queued.
    pub(super) fn note_local(&mut self, step: &'a Step, row: &'a str, id: &'a str) {
        let at = step.at.as_str();
        let user = self.devices.get(at).map(|device| device.user);
        let facts = match step.action.as_str() {
            "create" => Some(Facts {
                fields: stated(&step.fields, step.refs()),
                author: user,
                content: id,
            }),
            // A copy is a new row, and the creator of a row is the author of its content
            // (C17). Which abbreviation the copy gets is the interface's choice.
            "copy" => step.row.as_deref().and_then(|source| {
                let mut facts = self.device_facts(at, source)?;
                facts.fields.remove("abbreviation");
                if let Some(folder) = &step.to {
                    facts.fields.insert("folder".to_owned(), folder.clone());
                }
                facts.author = user;
                facts.content = id;
                Some(facts)
            }),
            _ => self.device_facts(at, row).map(|mut facts| {
                self.change(&mut facts, step, id, user);
                facts
            }),
        };
        match facts {
            Some(facts) => self.facts.insert((row, id), facts),
            None => self.facts.remove(&(row, id)),
        };
    }

    /// Applies an edit or a move to what the device held.
    fn change(&self, facts: &mut Facts<'a>, step: &'a Step, id: &'a str, user: Option<&'a str>) {
        for (key, value) in &step.fields {
            facts.fields.insert(key.clone(), render(value));
        }
        if step.action == "move" {
            let folder = step.row.as_deref().map(|row| self.entity(row)) == Some("folder");
            let key = if folder { "parent" } else { "folder" };
            match step.to.as_deref() {
                Some(to) if to != "top" => facts.fields.insert(key.to_owned(), to.to_owned()),
                _ => facts.fields.remove(key),
            };
        }
        let content = |key: &String| CONTENT_FIELDS.contains(&key.as_str());
        if step.fields.keys().any(content) {
            facts.author = user;
            facts.content = id;
        }
    }

    /// The version `to` of `row` holds what the version `from` held: a row a write takes along,
    /// a version restored from a conflict.
    pub(super) fn carry(&mut self, row: &'a str, from: &'a str, to: &'a str) {
        if let Some(facts) = self.facts.get(&(row, from)).cloned() {
            self.facts.insert((row, to), facts);
        }
    }

    /// The user approves, on one device, the content that device holds of a snippet.
    pub(super) fn note_approval(&mut self, step: &'a Step) {
        let Some(row) = step.row.as_deref() else {
            return;
        };
        if let Some(facts) = self.device_facts(&step.at, row) {
            self.approved.push((&step.at, row, facts.content));
        }
    }

    // ---- the outcome ----------------------------------------------------------------------------

    /// The devices that end online, each with its user and its rows.
    fn online<'o>(&self, outcomes: &'o Outcomes<'a>) -> Vec<(&'a str, &'a str, &'o [RowOutcome])> {
        self.devices
            .iter()
            .filter(|(_, device)| device.syncing())
            .filter_map(|(id, device)| Some((*id, device.user, outcomes.get(id)?.as_slice())))
            .collect()
    }

    /// What a live snippet a device holds is known to hold.
    fn snippet_facts(&self, row: &RowOutcome) -> Option<&Facts<'a>> {
        if row.state != "live" || self.entity(&row.row) != "snippet" {
            return None;
        }
        let version = row.version.as_deref()?;
        // Not `get`: a key borrowed from the outcome would shorten the life of what is found.
        self.facts
            .iter()
            .find(|((name, step), _)| *name == row.row && *step == version)
            .map(|(_, facts)| facts)
    }

    /// A field an outcome states for a row is what the steps left in that version.
    pub(super) fn stated_fields(&mut self, outcomes: &Outcomes<'a>) {
        let server = self.scenario.outcome.server.rows.as_slice();
        let places = std::iter::once(("the server", server))
            .chain(outcomes.iter().map(|(id, rows)| (*id, rows.as_slice())));
        let mut problems = Vec::new();
        for (place, rows) in places {
            for row in rows {
                let Some(version) = row.version.as_deref() else {
                    continue;
                };
                let Some(facts) = self.facts.get(&(row.row.as_str(), version)) else {
                    continue;
                };
                for (field, value) in &row.fields {
                    let left = match field.as_str() {
                        "content_by" => facts.author.map(str::to_owned),
                        "updated_by" => self.user_of(version).map(str::to_owned),
                        other => facts.fields.get(other).cloned(),
                    };
                    let value = render(value);
                    if left.as_ref().is_some_and(|left| *left != value) {
                        problems.push(format!(
                            "{place}: {} is stated with {field} = {value}, but {version} left \
                             it as {}",
                            row.row,
                            left.unwrap_or_default()
                        ));
                    }
                }
            }
        }
        self.problems.append(&mut problems);
    }

    /// A snippet whose content someone else wrote, and that must be reviewed, is held on every
    /// device of a reader that has not approved it, and never on a device of its author (D10).
    pub(super) fn reviews(&mut self, outcomes: &Outcomes<'a>) {
        let mut problems = Vec::new();
        for (id, user, rows) in self.online(outcomes) {
            let Some(found) = self.scenario.outcome.device.get(id) else {
                continue;
            };
            for row in rows {
                let Some(facts) = self.snippet_facts(row) else {
                    continue;
                };
                let Some(author) = facts.author else {
                    continue;
                };
                let name = row.row.as_str();
                let listed = found.review.iter().any(|held| held == name);
                let approved = self.approved.iter().any(|(at, held, content)| {
                    *at == id && *held == name && *content == facts.content
                });
                let reviewed = facts
                    .fields
                    .get("body")
                    .is_some_and(|body| REVIEWED.iter().any(|mark| body.contains(mark)));
                if author != user && reviewed && !approved && !listed {
                    problems.push(format!(
                        "{id}: {name} holds content that {author} wrote and that must be \
                         reviewed, and is not listed as held for review"
                    ));
                } else if author == user && listed {
                    problems.push(format!(
                        "{id}: {name} is listed as held for review, but the device's own user \
                         wrote its content"
                    ));
                }
            }
        }
        self.problems.append(&mut problems);
    }

    /// Where a snippet stands among those with its abbreviation for `user` (D13): 1 for content
    /// the user wrote in the user's own profile, 2 for other snippets of that profile, 3 for
    /// every other profile.
    fn class(&self, row: &str, facts: &Facts<'a>, user: &str) -> u8 {
        let own = self
            .profiles
            .get(row)
            .is_some_and(|profile| self.personal_owner(profile) == Some(user));
        match (own, facts.author == Some(user)) {
            (true, true) => 1,
            (true, false) => 2,
            (false, _) => 3,
        }
    }

    /// When the server first accepted `row`: 0 for a row of the start, otherwise by the order
    /// of the writes that reached it.
    fn accepted(&self, row: &str) -> Option<usize> {
        let at_start = self
            .scenario
            .start
            .rows
            .iter()
            .any(|r| r.id == row && r.rev > 0);
        if at_start {
            return Some(0);
        }
        let first = self.writes.iter().position(|write| write.row == row)?;
        Some(first + 1)
    }

    /// Whether `first` comes before `second` for `user` under D13, as far as the scenario
    /// decides it. Between two rows of other people's profiles it does not: that order is by
    /// the time access began.
    fn before(&self, first: &RowOutcome, second: &RowOutcome, user: &str) -> bool {
        let (Some(a), Some(b)) = (self.snippet_facts(first), self.snippet_facts(second)) else {
            return false;
        };
        let (row_a, row_b) = (first.row.as_str(), second.row.as_str());
        let (class_a, class_b) = (self.class(row_a, a, user), self.class(row_b, b, user));
        if class_a != class_b {
            return class_a < class_b;
        }
        if class_a == 3 {
            return false;
        }
        match (self.accepted(row_a), self.accepted(row_b)) {
            (Some(x), Some(y)) => x < y,
            _ => false,
        }
    }

    /// Of several snippets with one abbreviation on a device, the one stated to expand is not
    /// behind another (D13), and which one expands is stated.
    pub(super) fn precedence(&mut self, outcomes: &Outcomes<'a>) {
        let mut problems = Vec::new();
        for (id, user, rows) in self.online(outcomes) {
            let Some(found) = self.scenario.outcome.device.get(id) else {
                continue;
            };
            // Abbreviations are compared folded: `;sig` and `;SIG` are one.
            let mut groups: BTreeMap<String, Vec<&RowOutcome>> = BTreeMap::new();
            for row in rows {
                let abbreviation = self
                    .snippet_facts(row)
                    .and_then(|facts| facts.fields.get("abbreviation"));
                if let Some(abbreviation) = abbreviation {
                    groups
                        .entry(abbreviation.to_lowercase())
                        .or_default()
                        .push(row);
                }
            }
            for (abbreviation, group) in groups {
                let stated = found
                    .expands
                    .iter()
                    .find(|(key, _)| key.to_lowercase() == abbreviation);
                let Some((_, stated)) = stated else {
                    if group.len() > 1 {
                        let names: Vec<&str> = group.iter().map(|r| r.row.as_str()).collect();
                        problems.push(format!(
                            "{id} holds {} with the abbreviation {abbreviation}, and the \
                             outcome does not say which of them expands",
                            names.join(" and ")
                        ));
                    }
                    continue;
                };
                let Some(winner) = group.iter().find(|row| row.row == *stated) else {
                    continue;
                };
                for other in group
                    .iter()
                    .filter(|other| self.before(other, winner, user))
                {
                    problems.push(format!(
                        "{id}: {abbreviation} is stated to expand {stated}, but {} comes \
                         before it for {user} (D13)",
                        other.row
                    ));
                }
            }
        }
        self.problems.append(&mut problems);
    }

    /// The owner of a personal profile reads all of it: a device that ends online holds every
    /// live row of its user's own profile.
    pub(super) fn own_profile(&mut self, outcomes: &Outcomes<'a>) {
        let mut problems = Vec::new();
        for (id, user, rows) in self.online(outcomes) {
            for (row, held) in &self.server {
                let own = self
                    .profiles
                    .get(row)
                    .is_some_and(|profile| self.personal_owner(profile) == Some(user));
                let holds = rows.iter().any(|r| r.row == *row && r.state == "live");
                if own && !held.deleted && !holds {
                    problems.push(format!(
                        "{id} ends online without {row}, a live row of its user's own profile"
                    ));
                }
            }
        }
        self.problems.append(&mut problems);
    }
}
