//! The sync conflict scenarios of task 4.1 (`spec/sync/scenarios.toml`): the file format, a
//! loader and the check behind the KPI "every scenario has a defined outcome".
//!
//! `docs/sync-design.md` §12 writes the same scenarios out in prose; the file is what the
//! two-device simulation of task 4.4 replays. This module does not run a scenario against a
//! store, a sync client or a server. It checks two things:
//!
//! 1. **Structure**: a scenario can be replayed and judged. Every row it names exists, every
//!    step is one the format knows, and the outcome says, for the server and for every device,
//!    what state every row of the scenario is in and what the user is shown.
//! 2. **Consistency** (the `ledger` module): the outcome agrees with the steps and the answers
//!    the scenario itself states, and satisfies the invariants every outcome must satisfy
//!    whatever the rules decide. The check follows each change from the device that made it to
//!    the answer the scenario gives for it, and keeps a ledger of which version each row has on
//!    the server and which version each device had seen. From that it requires:
//!    - the server's row is the last write that reached it, live or deleted as that write left
//!      it, or absent when nothing reached it or a purge is due; a refused or blocked write is
//!      never the server's row;
//!    - the history holds exactly the versions that later writes replaced;
//!    - a write that replaced a version its device had not seen, written on another device,
//!      has a conflict that names both, of the kind the two writes make, shown on the devices
//!      of both authors; a conflict that was replaced hands its loser on;
//!    - a refused change is kept on the device that made it, a blocked one stays there with
//!      its row and its outbox entry, and no change is left unanswered on a device that ends
//!      online: every change is on the server, in the history, in a conflict, kept, blocked or
//!      still waiting;
//!    - a device that ends online holds each row as the server has it, or not at all, holds
//!      what it wrote itself, holds every live row of its user's own personal profile, and two
//!      such devices of one account hold the same;
//!    - a field an outcome states for a row is what the scenario's own steps left in that
//!      version; the author of a snippet's content is the one the steps give (the writer of a
//!      step that changed the content, otherwise the author of the version the device held); a
//!      snippet whose content someone else wrote and that must be reviewed is held on every
//!      device of a reader that has not approved it, and on none of its author's; and of
//!      several snippets with one abbreviation on a device, the one stated to expand is not
//!      behind another, as far as the author of the content, the profile and the order in
//!      which the server accepted the rows decide it;
//!    - after a restore of the server, a version the backup lacks that a device of the
//!      profile's owner held ends as the server's row or in its history; one that a device of
//!      the user who wrote it held ends there or in that device's list of kept changes; a
//!      write over a version written since the restore has its conflict; and a removal made
//!      after the backup is made again when the device that made it ends online.
//!
//! **Where the line is.** The ledger takes from the scenario the answers (`applied`,
//! `rejected:forbidden`, …), the rows a write takes with it (`cascade`) and what a user may see
//! through a share or a membership; it does not derive them, because it has no model of
//! permissions, the feed or the store. What it knows beyond versions it reads off the
//! scenario's own data: which fields a step wrote, who owns a personal profile, who wrote a
//! snippet's content. Whether the rules of §8 and §9 give the stated answers, cascades and
//! visibility is checked when task 4.4 replays every scenario against the real store and sync
//! client and a model of the server, and compares each outcome field by field. Until then an
//! outcome that is consistent with its own answers and wrong under the rules still passes
//! here: a write the permissions forbid that is answered `applied`, a cascade the rules do not
//! give, a folder that stays on a member's device after its share ended. The tests hold one
//! example of each, as examples that pass.
//!
//! # Format
//!
//! ```toml
//! version = 2
//!
//! [[scenario]]
//! id = "S01"                       # "S" and two digits, unique
//! title = "…"                      # unique
//! category = "edit"                # one of CATEGORIES
//! covers = ["edit-same-field"]     # what it covers; REQUIRED_COVERAGE must all appear
//! rules = ["C2", "D2"]             # the rules of the design document it exercises
//! given = "…"                      # the starting state in words
//!
//! [scenario.start]
//! users = ["ines"]
//! devices = [{ id = "A", user = "ines" }]          # signed_in = false, protocol = 1, copy_of
//! profiles = [{ id = "p-ines", kind = "personal", owner = "ines" }]
//! rows = [
//!   { id = "f1", entity = "folder", profile = "p-ines", rev = 3, fields = { name = "Replies" } },
//! ]
//!
//! [[scenario.step]]
//! id = "a1"                        # the name of the version this step writes
//! at = "A"                         # a device, or "server"
//! do = "edit"                      # one of ACTIONS
//! row = "s1"
//! fields = { body = "…" }
//! cascade = []                     # other rows the server writes when this change arrives
//! answers = { s1 = "applied" }     # on a step that pushes: what the server answers per row
//!
//! [scenario.outcome]
//! summary = "…"                    # the result in words, with what the user is shown
//!
//! [scenario.outcome.server]
//! rows = [{ row = "s1", state = "live", version = "a1" }]   # every row of the scenario
//! history = [{ row = "s1", version = "start" }]             # versions kept as replaced
//! conflicts = []                                            # recorded conflicts
//!
//! [scenario.outcome.device.A]      # one per device
//! same_as = "server"               # or rows = [...]; with same_as, rows override single rows
//! shown = []                       # what the user was shown, from SHOWN
//! ```
//!
//! How a replay reads it (the contract with task 4.4):
//!
//! - Every signed-in device starts online and in step with the server. An online device syncs
//!   until nothing is left to do (pull, push, pull) after each of its own steps and after every
//!   change on the server that reaches its feed, in the order of the device ids, before the
//!   next step begins. `offline` stops that, `online` starts it again with a sync. `manual`
//!   keeps the network but leaves syncing to the `pull`, `push` and `deliver` steps until
//!   `auto`. A `push` answered `held` or `lost` reached the server and was applied there; the
//!   answer comes with `deliver`, with a later `replayed`, or never. `restart` stops and starts
//!   the sync agent: what it had under way is forgotten, the outbox is not.
//! - A version is named after the step whose write produced it; `start` is the content a row
//!   has in `start`. A row's `version` in an outcome is the step that wrote its current
//!   revision, a delete included. A write the server makes because of a step (the rows a
//!   folder's delete takes with it, a folder a write restores, the memberships an account call
//!   changes) carries that step's name, and the step lists those rows in `cascade`. A row that
//!   a device sends again after a restore keeps the name of the version it had: the answers of
//!   a step answered `reset_required:epoch` are those to the rows the device sends again, each
//!   in the version the device held when the server was restored.
//! - `absent` means the device or the server holds no row with that id: never received, purged,
//!   or not visible to that user. Invitations are not rows of the local store: one is "on" a
//!   device while the account endpoint lists it for that user. An invitation has no revision
//!   (`rev = 0`) and no history.
//! - `advance` moves the shared clock by whole days; the server's daily purge runs, and each
//!   device purges at its next sync.
//! - A device with `copy_of` is a second installation whose data directory, device id and
//!   tokens were copied from the device named.

mod ledger;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The `version` of the file format this module reads.
pub const FORMAT_VERSION: u32 = 2;

/// The fewest scenarios the file may hold (the test of task 4.1: "30 or more").
pub const MIN_SCENARIOS: usize = 30;

/// The kinds of row a scenario can name. Profiles are listed under `start.profiles`.
pub const ENTITIES: [&str; 9] = [
    "folder",
    "snippet",
    "tag",
    "snippet_tag",
    "attachment",
    "snippet_attachment",
    "folder_share",
    "membership",
    "invitation",
];

/// The values of `category`.
pub const CATEGORIES: [&str; 12] = [
    "edit",
    "delete",
    "structure",
    "abbreviation",
    "tags",
    "attachments",
    "offline",
    "clock",
    "sharing",
    "membership",
    "transport",
    "server",
];

/// What the brief of task 4.1 asks the scenarios to cover; each tag must appear in the `covers`
/// of at least one scenario.
pub const REQUIRED_COVERAGE: [&str; 55] = [
    "edit-same-field",
    "edit-different-fields",
    "edit-vs-delete-delete-last",
    "edit-vs-delete-edit-last",
    "delete-vs-delete",
    "move-vs-edit",
    "same-abbreviation-personal",
    "same-abbreviation-personal-vs-shared",
    "folder-rename-vs-move",
    "tag-rename-vs-assignment",
    "attachment-vs-snippet-delete",
    "long-offline-inside-window",
    "long-offline-beyond-window",
    "clock-ahead",
    "clock-behind",
    "missed-folder-revoked",
    "member-removed-with-pending",
    "viewer-write",
    "invitation-accept-vs-withdraw",
    "ownership-transfer",
    "profile-deletion",
    "two-shares-same-folder",
    "include-loses-access",
    "replayed-push",
    "duplicated-push",
    "server-restore",
    "version-mismatch",
    // The cases the review of the design asked for (docs/reviews/4.1-sync-design-review.md §4).
    "lost-answer-then-delete",
    "folder-delete-vs-move-out",
    "folder-delete-refused",
    "simultaneous-requests",
    "abbreviation-takeover",
    "stale-write-held-content",
    "copy-held-snippet",
    "three-writers",
    "restore-membership",
    "restore-single-holder",
    "copied-data-directory",
    "shared-folder-deleted",
    "move-across-share-edge",
    "tag-leaves-share",
    "duplicate-links",
    "nested-shares",
    "device-revoked-with-pending",
    "share-ends-and-begins",
    // The cases the follow-up review asked for (docs/reviews/4.1-sync-design-followup.md).
    "restore-lost-edit-vs-new-edit",
    "restore-line-per-profile",
    "restore-two-lost-versions",
    "restore-lost-edit-kept",
    "lost-pull-answers",
    "subfolder-leaves-share",
    "file-by-known-hash",
    "revoked-device-returns",
    "client-claims-newer-state",
    "delete-above-nested-share",
];

/// The states a row can be in at the end.
pub const ROW_STATES: [&str; 3] = ["live", "deleted", "absent"];

/// Steps a device takes that change no row.
const DEVICE_CONTROL: [&str; 10] = [
    "offline",
    "online",
    "manual",
    "auto",
    "pull",
    "push",
    "deliver",
    "channel_down",
    "channel_up",
    "restart",
];

/// Steps a device takes that write something, so each needs an `id`.
const DEVICE_WRITES: [&str; 19] = [
    "create",
    "edit",
    "move",
    "delete",
    "undelete",
    "copy",
    "invite",
    "accept",
    "decline",
    "withdraw",
    "remove_member",
    "set_role",
    "leave",
    "transfer_ownership",
    "accept_ownership",
    "delete_group",
    "resolve",
    "sign_in",
    "sign_out",
];

/// Other steps of a device.
const DEVICE_OTHER: [&str; 4] = ["clock", "revoke_device", "upgrade", "approve"];

/// Steps that are calls to the server and not pushed rows: answered at once, with `ok` or an
/// error, and never queued.
const ACCOUNT_CALLS: [&str; 12] = [
    "invite",
    "accept",
    "decline",
    "withdraw",
    "remove_member",
    "set_role",
    "leave",
    "transfer_ownership",
    "accept_ownership",
    "delete_group",
    "resolve",
    "revoke_device",
];

/// Steps of the server (`at = "server"`).
const SERVER_ACTIONS: [&str; 5] = [
    "advance",
    "backup",
    "restore_backup",
    "set_protocol",
    "set_limit",
];

/// What the server answers to one pushed row (`answers`).
pub const PUSH_ANSWERS: [&str; 16] = [
    "applied",
    "applied+conflict",
    "applied+adjusted",
    "unchanged",
    "unchanged+adjusted",
    "superseded",
    "replayed",
    "rejected:forbidden",
    "rejected:gone",
    "rejected:parent_gone",
    "rejected:plan_limit",
    "rejected:not_pushable",
    "blocked:too_large",
    "blocked:plan_limit",
    "blocked:invalid",
    "blocked:parent_blocked",
];

/// What a call or a whole sync is answered with (`answer`).
pub const CALL_ANSWERS: [&str; 18] = [
    "ok",
    "held",
    "lost",
    "reset_required:expired",
    "reset_required:epoch",
    "error:server_behind",
    "upgrade_required",
    "server_too_old",
    "rate_limited",
    "device_revoked",
    "error:device_conflict",
    "error:needs_review",
    "error:invitation_invalid",
    "error:already_accepted",
    "error:not_owner",
    "error:not_a_member",
    "error:owner_cannot_leave",
    "error:forbidden",
];

/// The kinds of recorded conflict.
pub const CONFLICT_KINDS: [&str; 5] = [
    "edit_edit",
    "edit_over_delete",
    "delete_over_edit",
    "deleted_with_folder",
    "folder_restored",
];

/// What a device can show its user (the wording belongs to task 4.5).
pub const SHOWN: [&str; 16] = [
    "conflict",
    "kept",
    "blocked",
    "shared",
    "revoked",
    "duplicate",
    "review",
    "broken",
    "adjusted",
    "superseded",
    "plan_limit",
    "upgrade",
    "invitation_invalid",
    "refused",
    "signed_out",
    "restored",
];

/// The states a recorded conflict can be in at the end; `replaced` means a newer conflict on
/// the same row took it over, with its loser.
pub const CONFLICT_STATES: [&str; 3] = ["open", "resolved", "replaced"];

/// The whole file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioFile {
    /// [`FORMAT_VERSION`].
    pub version: u32,
    /// The scenarios.
    #[serde(default, rename = "scenario")]
    pub scenarios: Vec<Scenario>,
}

/// One scenario.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// `S` and two digits.
    pub id: String,
    /// What it is about.
    pub title: String,
    /// One of [`CATEGORIES`].
    pub category: String,
    /// What it covers ([`REQUIRED_COVERAGE`] and free tags).
    #[serde(default)]
    pub covers: Vec<String>,
    /// The rules of `docs/sync-design.md` it exercises (`C3`, `D2`).
    #[serde(default)]
    pub rules: Vec<String>,
    /// The starting state in words.
    pub given: String,
    /// The starting state.
    pub start: Start,
    /// The steps, in order.
    #[serde(default, rename = "step")]
    pub steps: Vec<Step>,
    /// The defined outcome.
    pub outcome: Outcome,
}

/// The starting state of a scenario.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    /// The accounts.
    pub users: Vec<String>,
    /// The devices.
    pub devices: Vec<DeviceSpec>,
    /// The profiles.
    pub profiles: Vec<ProfileSpec>,
    /// The rows.
    #[serde(default)]
    pub rows: Vec<StartRow>,
}

/// A device.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSpec {
    /// Its name in the scenario.
    pub id: String,
    /// The account it belongs to.
    pub user: String,
    /// Whether it starts signed in (default) or with a local library only.
    #[serde(default = "yes")]
    pub signed_in: bool,
    /// The sync protocol version it speaks, when the scenario is about versions.
    pub protocol: Option<u32>,
    /// The device whose data directory, device id and tokens this installation was copied from.
    pub copy_of: Option<String>,
}

const fn yes() -> bool {
    true
}

/// A profile.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSpec {
    /// Its name in the scenario.
    pub id: String,
    /// `personal`, `family` or `company`.
    pub kind: String,
    /// The account that owns it.
    pub owner: String,
    /// For the local personal profile of a device that is not signed in: that device.
    pub local_to: Option<String>,
}

/// What a row refers to; which of these a row needs depends on its entity.
#[derive(Debug, Clone, Copy, Default)]
pub struct Refs<'a> {
    /// The owning profile (folder, tag, attachment).
    pub profile: Option<&'a str>,
    /// The parent folder (folder; absent for a top-level folder).
    pub parent: Option<&'a str>,
    /// The folder (snippet, folder share).
    pub folder: Option<&'a str>,
    /// The snippet (links).
    pub snippet: Option<&'a str>,
    /// The tag (snippet–tag link).
    pub tag: Option<&'a str>,
    /// The attachment (snippet–attachment link).
    pub attachment: Option<&'a str>,
    /// The group profile (folder share, membership, invitation).
    pub group: Option<&'a str>,
    /// The account (membership, invitation).
    pub user: Option<&'a str>,
}

/// The reference fields shared by [`StartRow`] and [`Step`], and the view of them.
macro_rules! refs_view {
    ($ty:ty) => {
        impl $ty {
            /// What the row refers to.
            #[must_use]
            pub fn refs(&self) -> Refs<'_> {
                Refs {
                    profile: self.profile.as_deref(),
                    parent: self.parent.as_deref(),
                    folder: self.folder.as_deref(),
                    snippet: self.snippet.as_deref(),
                    tag: self.tag.as_deref(),
                    attachment: self.attachment.as_deref(),
                    group: self.group.as_deref(),
                    user: self.user.as_deref(),
                }
            }
        }
    };
}

/// A row of the starting state.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRow {
    /// Its name in the scenario.
    pub id: String,
    /// One of [`ENTITIES`].
    pub entity: String,
    /// Its revision on the server; 0 for a row no server has seen.
    pub rev: u64,
    /// The owning profile (folder, tag, attachment).
    pub profile: Option<String>,
    /// The parent folder (folder; absent for a top-level folder).
    pub parent: Option<String>,
    /// The folder (snippet, folder share).
    pub folder: Option<String>,
    /// The snippet (links).
    pub snippet: Option<String>,
    /// The tag (snippet–tag link).
    pub tag: Option<String>,
    /// The attachment (snippet–attachment link).
    pub attachment: Option<String>,
    /// The group profile (folder share, membership, invitation; the group of an account call).
    pub group: Option<String>,
    /// The account (membership, invitation; the account an account call names).
    pub user: Option<String>,
    /// The fields the scenario cares about.
    #[serde(default)]
    pub fields: BTreeMap<String, toml::Value>,
    /// Whether it starts as a tombstone.
    #[serde(default)]
    pub deleted: bool,
    /// The account that wrote it, when not the profile's owner.
    pub by: Option<String>,
    /// The devices that hold it, when not every device whose account may read it.
    #[serde(default)]
    pub only_on: Vec<String>,
}

refs_view!(StartRow);

/// One step.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// The name of the version the step writes; required for steps that write.
    pub id: Option<String>,
    /// The device that acts, or `server`.
    pub at: String,
    /// What happens.
    #[serde(rename = "do")]
    pub action: String,
    /// The row acted on.
    pub row: Option<String>,
    /// For `create`: the kind of the new row.
    pub entity: Option<String>,
    /// The owning profile (folder, tag, attachment).
    pub profile: Option<String>,
    /// The parent folder (folder; absent for a top-level folder).
    pub parent: Option<String>,
    /// The folder (snippet, folder share).
    pub folder: Option<String>,
    /// The snippet (links).
    pub snippet: Option<String>,
    /// The tag (snippet–tag link).
    pub tag: Option<String>,
    /// The attachment (snippet–attachment link).
    pub attachment: Option<String>,
    /// The group profile (folder share, membership, invitation; the group of an account call).
    pub group: Option<String>,
    /// The account (membership, invitation; the account an account call names).
    pub user: Option<String>,
    /// For `create` and `edit`: the fields written.
    #[serde(default)]
    pub fields: BTreeMap<String, toml::Value>,
    /// The other rows the server writes when this step's change arrives, under this step's
    /// name: what a folder's delete takes with it, a folder a write restores, the memberships
    /// and shares an account call changes.
    #[serde(default)]
    pub cascade: Vec<String>,
    /// For `move` and `copy`: the new folder or parent, or `top`; for `restore_backup`: the
    /// backup step.
    pub to: Option<String>,
    /// For `advance`: whole days.
    pub days: Option<u32>,
    /// For `clock`: how far the device's clock is from true time.
    pub offset_ms: Option<i64>,
    /// For `resolve`: `restore` or `keep`.
    pub choice: Option<String>,
    /// For `invite` and `set_role`: the role.
    pub role: Option<String>,
    /// For `invite`, `accept` and `copy`: the row the step creates.
    pub creates: Option<String>,
    /// For `revoke_device`: the device revoked.
    pub device: Option<String>,
    /// For `upgrade`: the protocol version the device speaks afterwards.
    pub protocol: Option<u32>,
    /// For `set_protocol`: the oldest version the server accepts.
    pub min: Option<u32>,
    /// For `set_protocol`: the newest version the server speaks.
    pub max: Option<u32>,
    /// For `set_limit`: the limit's name.
    pub name: Option<String>,
    /// For `set_limit`: its value.
    pub value: Option<i64>,
    /// For `push`: `duplicate` delivers the request twice.
    pub fault: Option<String>,
    /// What the call, or the sync as a whole, is answered with ([`CALL_ANSWERS`]).
    pub answer: Option<String>,
    /// What the server answers per pushed row ([`PUSH_ANSWERS`]).
    #[serde(default)]
    pub answers: BTreeMap<String, String>,
    /// A remark for the reader.
    pub note: Option<String>,
}

refs_view!(Step);

/// The defined outcome of a scenario.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    /// The result in words, with what the user is shown.
    pub summary: String,
    /// The server's final state.
    pub server: ServerOutcome,
    /// Each device's final state, by device id.
    #[serde(default)]
    pub device: BTreeMap<String, DeviceOutcome>,
}

/// The server's final state.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerOutcome {
    /// Every row of the scenario.
    pub rows: Vec<RowOutcome>,
    /// The versions kept in the history table because a write replaced them.
    pub history: Vec<VersionRef>,
    /// The conflicts recorded.
    pub conflicts: Vec<ConflictOutcome>,
}

/// The final state of one row in one place.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowOutcome {
    /// The row.
    pub row: String,
    /// One of [`ROW_STATES`].
    pub state: String,
    /// The step that wrote its current revision, or `start`; none for an absent row.
    pub version: Option<String>,
    /// Fields worth stating, where the version alone does not say them.
    #[serde(default)]
    pub fields: BTreeMap<String, toml::Value>,
}

/// A version of a row.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionRef {
    /// The row.
    pub row: String,
    /// The step that wrote the version, or `start`.
    pub version: String,
}

/// A recorded conflict.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConflictOutcome {
    /// The row.
    pub row: String,
    /// One of [`CONFLICT_KINDS`].
    pub kind: String,
    /// The step whose write is the row now.
    pub winner: String,
    /// The step whose write was replaced and is offered back.
    pub loser: String,
    /// The losers taken over from the conflict this one replaced; offered back as well.
    #[serde(default)]
    pub carried: Vec<String>,
    /// The accounts that are shown the conflict.
    pub parties: Vec<String>,
    /// One of [`CONFLICT_STATES`].
    pub state: String,
}

/// A device's final state.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceOutcome {
    /// `server` or another device whose rows this device has too.
    pub same_as: Option<String>,
    /// Every row of the scenario; with `same_as`, only the rows that differ.
    #[serde(default)]
    pub rows: Vec<RowOutcome>,
    /// What the user was shown, in order of first appearance ([`SHOWN`]); may be empty, never
    /// left out.
    pub shown: Vec<String>,
    /// Changes still waiting in the outbox.
    #[serde(default)]
    pub outbox: u32,
    /// The steps whose changes the server refused and the device keeps for the user.
    #[serde(default)]
    pub kept: Vec<String>,
    /// The steps whose changes cannot be stored on the server yet and stay local.
    #[serde(default)]
    pub blocked: Vec<String>,
    /// Which snippet holds an abbreviation on this device.
    #[serde(default)]
    pub expands: BTreeMap<String, String>,
    /// The snippets held for review.
    #[serde(default)]
    pub review: Vec<String>,
    /// The snippets with a reference that no longer resolves.
    #[serde(default)]
    pub broken: Vec<String>,
    /// The user's permission on a folder: `view`, `edit`, `manage` or `none`.
    #[serde(default)]
    pub access: BTreeMap<String, String>,
}

/// Where the scenarios live, from any crate of the workspace under `crates/`.
#[must_use]
pub fn scenarios_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/sync/scenarios.toml")
}

/// Reads and parses a scenario file.
///
/// # Errors
///
/// The file cannot be read, or is not TOML of this format.
pub fn load(path: &Path) -> Result<ScenarioFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parses the text of a scenario file.
///
/// # Errors
///
/// The text is not TOML of this format.
pub fn parse(text: &str) -> Result<ScenarioFile, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

/// Checks a scenario file and returns everything wrong with it; nothing means that every
/// scenario can be replayed, has a defined outcome for the server and every device, and that
/// each outcome agrees with the scenario's own steps and answers (see the module's
/// documentation for what that covers and what waits for the replay of task 4.4).
#[must_use]
pub fn validate(file: &ScenarioFile) -> Vec<String> {
    let mut problems = Vec::new();
    if file.version != FORMAT_VERSION {
        problems.push(format!(
            "version is {}, this build reads {FORMAT_VERSION}",
            file.version
        ));
    }
    if file.scenarios.len() < MIN_SCENARIOS {
        problems.push(format!(
            "{} scenarios, at least {MIN_SCENARIOS} are required",
            file.scenarios.len()
        ));
    }
    let mut ids = BTreeSet::new();
    let mut titles = BTreeSet::new();
    for scenario in &file.scenarios {
        if !ids.insert(scenario.id.as_str()) {
            problems.push(format!("{}: the id is used twice", scenario.id));
        }
        if !titles.insert(scenario.title.as_str()) {
            problems.push(format!("{}: the title is used twice", scenario.id));
        }
        let mut check = Check::new(scenario);
        check.run();
        problems.append(&mut check.problems);
    }
    let covered: BTreeSet<&str> = file
        .scenarios
        .iter()
        .flat_map(|s| s.covers.iter().map(String::as_str))
        .collect();
    for tag in REQUIRED_COVERAGE {
        if !covered.contains(tag) {
            problems.push(format!("no scenario covers {tag}"));
        }
    }
    problems
}

/// Whether `id` is `S` and two digits.
fn is_scenario_id(id: &str) -> bool {
    id.len() == 3 && id.starts_with('S') && id[1..].chars().all(|c| c.is_ascii_digit())
}

/// Whether `id` names a rule of the design document: one or two capitals and a number.
fn is_rule_id(id: &str) -> bool {
    let letters = id.chars().take_while(char::is_ascii_uppercase).count();
    (1..=2).contains(&letters)
        && id.len() > letters
        && id[letters..].chars().all(|c| c.is_ascii_digit())
}

/// Whether `tag` is lower-case words joined by hyphens.
fn is_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .split('-')
            .all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase()))
}

/// The check of one scenario: what it has declared so far, and what is wrong.
struct Check<'a> {
    scenario: &'a Scenario,
    users: BTreeSet<&'a str>,
    devices: BTreeSet<&'a str>,
    /// Profile id → whether it is a group.
    profiles: BTreeMap<&'a str, bool>,
    /// Row id → entity, for rows of the start and rows created by the steps seen so far.
    rows: BTreeMap<&'a str, &'a str>,
    /// Rows of the starting state.
    start_rows: BTreeSet<&'a str>,
    /// Step id → where the step happens (a device or `server`).
    steps: BTreeMap<&'a str, &'a str>,
    problems: Vec<String>,
}

impl<'a> Check<'a> {
    fn new(scenario: &'a Scenario) -> Self {
        Self {
            scenario,
            users: BTreeSet::new(),
            devices: BTreeSet::new(),
            profiles: BTreeMap::new(),
            rows: BTreeMap::new(),
            start_rows: BTreeSet::new(),
            steps: BTreeMap::new(),
            problems: Vec::new(),
        }
    }

    fn fail(&mut self, what: impl AsRef<str>) {
        self.problems
            .push(format!("{}: {}", self.scenario.id, what.as_ref()));
    }

    fn run(&mut self) {
        self.header();
        self.start();
        self.steps();
        self.outcome();
        // The ledger follows references the structural check has just verified, so it runs
        // only on a scenario that is sound so far.
        if self.problems.is_empty() {
            let devices: BTreeMap<&str, Vec<RowOutcome>> = self
                .devices
                .iter()
                .filter_map(|device| Some((*device, self.resolve(device, 0)?)))
                .collect();
            for problem in ledger::check(self.scenario, &self.rows, &devices) {
                self.fail(problem);
            }
        }
    }

    fn header(&mut self) {
        let s = self.scenario;
        if !is_scenario_id(&s.id) {
            self.fail("the id must be S and two digits");
        }
        if s.title.trim().is_empty() {
            self.fail("the title is empty");
        }
        if s.given.trim().is_empty() {
            self.fail("`given` is empty");
        }
        if !CATEGORIES.contains(&s.category.as_str()) {
            self.fail(format!("unknown category {}", s.category));
        }
        if s.covers.is_empty() {
            self.fail("`covers` is empty");
        }
        for tag in &s.covers {
            if !is_tag(tag) {
                self.fail(format!("`covers` has a malformed tag {tag:?}"));
            }
        }
        if s.rules.is_empty() {
            self.fail("`rules` is empty");
        }
        for rule in &s.rules {
            if !is_rule_id(rule) {
                self.fail(format!("`rules` has a malformed rule id {rule:?}"));
            }
        }
    }

    fn start(&mut self) {
        let start = &self.scenario.start;
        for user in &start.users {
            if !self.users.insert(user) {
                self.fail(format!("user {user} is listed twice"));
            }
        }
        if self.users.is_empty() {
            self.fail("no users");
        }
        for device in &start.devices {
            if device.id == "server" || !self.devices.insert(&device.id) {
                self.fail(format!("device {} is listed twice or reserved", device.id));
            }
            if !self.users.contains(device.user.as_str()) {
                self.fail(format!("device {} has an unknown user", device.id));
            }
        }
        for device in &start.devices {
            if let Some(original) = &device.copy_of {
                let same_user = start
                    .devices
                    .iter()
                    .any(|d| &d.id == original && d.user == device.user && d.id != device.id);
                if !same_user {
                    self.fail(format!(
                        "device {} is a copy of {original}, which is not another device of the \
                         same user",
                        device.id
                    ));
                }
            }
        }
        if self.devices.is_empty() {
            self.fail("no devices");
        }
        self.start_profiles();
        self.start_rows();
    }

    fn start_profiles(&mut self) {
        let start = &self.scenario.start;
        for profile in &start.profiles {
            let group = match profile.kind.as_str() {
                "personal" => false,
                "family" | "company" => true,
                other => {
                    self.fail(format!("profile {} has unknown kind {other}", profile.id));
                    true
                }
            };
            if self.profiles.insert(&profile.id, group).is_some() {
                self.fail(format!("profile {} is listed twice", profile.id));
            }
            if !self.users.contains(profile.owner.as_str()) {
                self.fail(format!("profile {} has an unknown owner", profile.id));
            }
            if let Some(device) = &profile.local_to
                && !self.devices.contains(device.as_str())
            {
                self.fail(format!(
                    "profile {} is local to an unknown device",
                    profile.id
                ));
            }
        }
        for user in &start.users {
            let personal = start
                .profiles
                .iter()
                .filter(|p| p.kind == "personal" && &p.owner == user && p.local_to.is_none())
                .count();
            if personal != 1 {
                self.fail(format!(
                    "user {user} has {personal} personal profiles, not 1"
                ));
            }
        }
    }

    fn start_rows(&mut self) {
        let start = &self.scenario.start;
        // Two passes, so that a row may refer to one listed after it.
        for row in &start.rows {
            if !ENTITIES.contains(&row.entity.as_str()) {
                self.fail(format!("row {} has unknown entity {}", row.id, row.entity));
            }
            if self.rows.insert(&row.id, &row.entity).is_some() {
                self.fail(format!("row {} is listed twice", row.id));
            }
            if row.entity == "invitation" && row.rev != 0 {
                self.fail(format!(
                    "the invitation {} has a revision; invitations have none (rev = 0)",
                    row.id
                ));
            }
            self.start_rows.insert(&row.id);
        }
        for row in &start.rows {
            self.refs(&row.id, &row.entity, row.refs());
            if let Some(by) = &row.by
                && !self.users.contains(by.as_str())
            {
                self.fail(format!("row {} was written by an unknown user", row.id));
            }
            for device in &row.only_on {
                if !self.devices.contains(device.as_str()) {
                    self.fail(format!("row {} is on an unknown device {device}", row.id));
                }
            }
        }
    }

    /// Checks that a row of `entity` names what it must refer to, and that each exists.
    fn refs(&mut self, row: &str, entity: &str, refs: Refs<'_>) {
        match entity {
            "folder" => {
                self.profile_ref(row, refs.profile, false);
                if refs.parent.is_some() {
                    self.row_ref(row, "parent", refs.parent, "folder");
                }
            }
            "snippet" => self.row_ref(row, "folder", refs.folder, "folder"),
            "tag" | "attachment" => self.profile_ref(row, refs.profile, false),
            "snippet_tag" => {
                self.row_ref(row, "snippet", refs.snippet, "snippet");
                self.row_ref(row, "tag", refs.tag, "tag");
            }
            "snippet_attachment" => {
                self.row_ref(row, "snippet", refs.snippet, "snippet");
                self.row_ref(row, "attachment", refs.attachment, "attachment");
            }
            "folder_share" => {
                self.row_ref(row, "folder", refs.folder, "folder");
                self.profile_ref(row, refs.group, true);
            }
            "membership" | "invitation" => {
                self.profile_ref(row, refs.group, true);
                if !refs.user.is_some_and(|u| self.users.contains(u)) {
                    self.fail(format!("row {row} needs a known `user`"));
                }
            }
            _ => {}
        }
    }

    fn row_ref(&mut self, row: &str, key: &str, target: Option<&str>, entity: &str) {
        match target {
            Some(target) if self.rows.get(target).copied() == Some(entity) => {}
            Some(target) => self.fail(format!(
                "row {row}: `{key}` {target} is not a known {entity}"
            )),
            None => self.fail(format!("row {row} needs `{key}`")),
        }
    }

    fn profile_ref(&mut self, row: &str, target: Option<&str>, group: bool) {
        let key = if group { "group" } else { "profile" };
        match target.and_then(|t| self.profiles.get(t).copied()) {
            Some(is_group) if !group || is_group => {}
            Some(_) => self.fail(format!("row {row}: `{key}` must be a group profile")),
            None => self.fail(format!("row {row} needs a known `{key}`")),
        }
    }

    fn steps(&mut self) {
        let scenario = self.scenario;
        if scenario.steps.is_empty() {
            self.fail("no steps");
        }
        for (index, step) in scenario.steps.iter().enumerate() {
            let n = index + 1;
            if let Some(id) = &step.id
                && (id == "start" || self.steps.insert(id, &step.at).is_some())
            {
                self.fail(format!("step {n}: the id {id} is used twice or reserved"));
            }
            self.step(n, step);
        }
        // A cascade may name a row that a later step creates (a folder's delete that reaches
        // the server after another device's new snippet), so it is checked once all are known.
        for (index, step) in scenario.steps.iter().enumerate() {
            for row in &step.cascade {
                if !self.rows.contains_key(row.as_str()) {
                    self.fail(format!(
                        "step {}: `cascade` names the unknown row {row}",
                        index + 1
                    ));
                }
            }
        }
    }

    fn step(&mut self, n: usize, step: &'a Step) {
        let action = step.action.as_str();
        let on_server = SERVER_ACTIONS.contains(&action);
        let on_device = DEVICE_CONTROL.contains(&action)
            || DEVICE_WRITES.contains(&action)
            || DEVICE_OTHER.contains(&action);
        if !on_server && !on_device {
            self.fail(format!("step {n}: unknown action {action}"));
            return;
        }
        if on_server != (step.at == "server") {
            self.fail(format!("step {n}: {action} cannot happen at {}", step.at));
        } else if on_device && !self.devices.contains(step.at.as_str()) {
            self.fail(format!("step {n}: unknown device {}", step.at));
        }
        let writes = DEVICE_WRITES.contains(&action) || action == "backup";
        if writes && step.id.is_none() {
            self.fail(format!("step {n}: {action} writes, so it needs an id"));
        }
        if ACCOUNT_CALLS.contains(&action) && step.answer.is_none() {
            self.fail(format!(
                "step {n}: {action} is a call to the server, so it needs an `answer`"
            ));
        }
        self.step_arguments(n, step);
        self.step_answers(n, step);
    }

    fn step_arguments(&mut self, n: usize, step: &'a Step) {
        let missing = |what: &str| format!("step {n}: {} needs `{what}`", step.action);
        match step.action.as_str() {
            "create" => self.step_create(n, step),
            "edit" => {
                self.step_row(n, step);
                if step.fields.is_empty() {
                    self.fail(missing("fields"));
                }
            }
            "move" => {
                self.step_row(n, step);
                match step.to.as_deref() {
                    Some("top") => {}
                    Some(to) if self.rows.get(to).copied() == Some("folder") => {}
                    _ => self.fail(format!("step {n}: `to` must be a known folder or \"top\"")),
                }
            }
            "delete" | "undelete" | "decline" | "withdraw" | "remove_member" | "leave"
            | "approve" => {
                self.step_row(n, step);
            }
            "copy" => self.step_copy(n, step),
            "set_role" => {
                self.step_row(n, step);
                if step.role.is_none() {
                    self.fail(missing("role"));
                }
            }
            "resolve" => {
                self.step_row(n, step);
                if !matches!(step.choice.as_deref(), Some("restore" | "keep")) {
                    self.fail(format!("step {n}: `choice` must be restore or keep"));
                }
            }
            "accept" => {
                self.step_row(n, step);
                self.step_creates(n, step, "membership");
            }
            "invite" => {
                self.step_creates(n, step, "invitation");
                if step.role.is_none() {
                    self.fail(missing("role"));
                }
            }
            "transfer_ownership" | "accept_ownership" | "delete_group" => {
                self.step_group(n, step);
            }
            "sign_in" => {
                if !step.user.as_deref().is_some_and(|u| self.users.contains(u)) {
                    self.fail(missing("user"));
                }
            }
            other => self.step_plain_arguments(n, step, other),
        }
    }

    fn step_plain_arguments(&mut self, n: usize, step: &Step, action: &str) {
        let ok = match action {
            "clock" => step.offset_ms.is_some(),
            "advance" => step.days.is_some_and(|d| d > 0),
            "restore_backup" => step
                .to
                .as_deref()
                .is_some_and(|to| self.steps.contains_key(to)),
            "set_protocol" => step.min.is_some() && step.max.is_some(),
            "set_limit" => step.name.is_some() && step.value.is_some(),
            "upgrade" => step.protocol.is_some(),
            "revoke_device" => step
                .device
                .as_deref()
                .is_some_and(|d| self.devices.contains(d)),
            _ => true,
        };
        if !ok {
            self.fail(format!(
                "step {n}: {action} lacks an argument or names nothing known"
            ));
        }
        if let Some(fault) = &step.fault
            && (action != "push" || fault != "duplicate")
        {
            self.fail(format!("step {n}: unknown fault {fault}"));
        }
    }

    fn step_row(&mut self, n: usize, step: &Step) {
        match &step.row {
            Some(row) if self.rows.contains_key(row.as_str()) => {}
            Some(row) => self.fail(format!("step {n}: row {row} does not exist yet")),
            None => self.fail(format!("step {n}: {} needs `row`", step.action)),
        }
    }

    fn step_group(&mut self, n: usize, step: &Step) {
        match step.group.as_deref().and_then(|g| self.profiles.get(g)) {
            Some(true) => {}
            _ => self.fail(format!("step {n}: {} needs a known `group`", step.action)),
        }
        if step.action == "transfer_ownership"
            && !step.user.as_deref().is_some_and(|u| self.users.contains(u))
        {
            self.fail(format!("step {n}: transfer_ownership needs a known `user`"));
        }
    }

    fn step_create(&mut self, n: usize, step: &'a Step) {
        let (Some(row), Some(entity)) = (&step.row, &step.entity) else {
            self.fail(format!("step {n}: create needs `row` and `entity`"));
            return;
        };
        if !ENTITIES.contains(&entity.as_str()) {
            self.fail(format!("step {n}: unknown entity {entity}"));
            return;
        }
        match self.rows.get(row.as_str()).copied() {
            None => {
                self.rows.insert(row, entity);
            }
            // A share's id is derived from its folder and group, so two devices that share
            // the same folder with the same group create the same row.
            Some("folder_share") if entity == "folder_share" => {}
            Some(_) => self.fail(format!("step {n}: row {row} exists already")),
        }
        self.refs(row, entity, step.refs());
    }

    /// A copy of a snippet into another folder: a new row, unless the store refuses it.
    fn step_copy(&mut self, n: usize, step: &'a Step) {
        self.step_row(n, step);
        match step.to.as_deref() {
            Some(to) if self.rows.get(to).copied() == Some("folder") => {}
            _ => self.fail(format!("step {n}: `to` must be a known folder")),
        }
        let refused = step.answer.as_deref() == Some("error:needs_review");
        match (&step.creates, refused) {
            (None, true) => {}
            (Some(_), true) => {
                self.fail(format!("step {n}: a refused copy creates no row"));
            }
            (_, false) => self.step_creates(n, step, "snippet"),
        }
    }

    fn step_creates(&mut self, n: usize, step: &'a Step, entity: &'static str) {
        if entity == "invitation" {
            self.step_group(n, step);
            if !step.user.as_deref().is_some_and(|u| self.users.contains(u)) {
                self.fail(format!("step {n}: invite needs a known `user`"));
            }
        }
        match &step.creates {
            Some(row) if !self.rows.contains_key(row.as_str()) => {
                self.rows.insert(row, entity);
            }
            Some(row) => self.fail(format!("step {n}: row {row} exists already")),
            None => self.fail(format!("step {n}: {} needs `creates`", step.action)),
        }
    }

    fn step_answers(&mut self, n: usize, step: &Step) {
        if let Some(answer) = &step.answer
            && !CALL_ANSWERS.contains(&answer.as_str())
        {
            self.fail(format!("step {n}: unknown answer {answer}"));
        }
        for (row, answer) in &step.answers {
            if !self.rows.contains_key(row.as_str()) {
                self.fail(format!("step {n}: an answer names the unknown row {row}"));
            }
            if !PUSH_ANSWERS.contains(&answer.as_str()) {
                self.fail(format!("step {n}: unknown push answer {answer}"));
            }
        }
    }

    fn outcome(&mut self) {
        let outcome = &self.scenario.outcome;
        if outcome.summary.trim().is_empty() {
            self.fail("the outcome has no summary");
        }
        self.complete("the server", &outcome.server.rows);
        for version in &outcome.server.history {
            self.known_row("history", &version.row);
            self.version("history", &version.row, &version.version);
            if self.rows.get(version.row.as_str()).copied() == Some("invitation") {
                self.fail(format!(
                    "the history holds the invitation {}; invitations have no history",
                    version.row
                ));
            }
            // The history holds replaced versions only: a version is current or replaced.
            let current = outcome.server.rows.iter().find(|r| r.row == version.row);
            if current.and_then(|r| r.version.as_deref()) == Some(version.version.as_str()) {
                self.fail(format!(
                    "the history holds {} of {}, which is the row's current version",
                    version.version, version.row
                ));
            }
        }
        for conflict in &outcome.server.conflicts {
            self.conflict(conflict);
        }
        self.conflicts_per_row();
        let devices: Vec<&str> = self.devices.iter().copied().collect();
        for device in devices {
            match outcome.device.get(device) {
                Some(found) => self.device_outcome(device, found),
                None => self.fail(format!("no outcome is defined for device {device}")),
            }
        }
        for device in outcome.device.keys() {
            if !self.devices.contains(device.as_str()) {
                self.fail(format!("an outcome names the unknown device {device}"));
            }
        }
    }

    fn conflict(&mut self, conflict: &ConflictOutcome) {
        self.known_row("a conflict", &conflict.row);
        if !CONFLICT_KINDS.contains(&conflict.kind.as_str()) {
            self.fail(format!("unknown conflict kind {}", conflict.kind));
        }
        for step in [&conflict.winner, &conflict.loser]
            .into_iter()
            .chain(&conflict.carried)
        {
            if !self.steps.contains_key(step.as_str()) {
                self.fail(format!("a conflict names the unknown step {step}"));
            }
        }
        // The loser is what the user is offered back, so the server must still hold it; and
        // until the conflict is resolved the winner is what the row is.
        let server = &self.scenario.outcome.server;
        for offered in std::iter::once(&conflict.loser).chain(&conflict.carried) {
            let kept = server
                .history
                .iter()
                .any(|v| v.row == conflict.row && &v.version == offered);
            if !kept {
                self.fail(format!(
                    "the conflict on {} offers {offered} back, which the history does not hold",
                    conflict.row
                ));
            }
        }
        let current = server.rows.iter().find(|r| r.row == conflict.row);
        if conflict.state == "open"
            && current.and_then(|r| r.version.as_deref()) != Some(conflict.winner.as_str())
        {
            self.fail(format!(
                "the open conflict on {} names a winner that is not the row's version",
                conflict.row
            ));
        }
        if conflict.winner == conflict.loser {
            self.fail("a conflict has the same winner and loser");
        }
        if conflict.parties.is_empty() {
            self.fail("a conflict has no parties");
        }
        for party in &conflict.parties {
            if !self.users.contains(party.as_str()) {
                self.fail(format!("a conflict names the unknown user {party}"));
            }
        }
        if !CONFLICT_STATES.contains(&conflict.state.as_str()) {
            self.fail(format!("unknown conflict state {}", conflict.state));
        }
    }

    /// At most one conflict per row is open, and a conflict that a newer one replaced hands
    /// its losers on to it, so that no replaced version drops out of reach.
    fn conflicts_per_row(&mut self) {
        let conflicts = &self.scenario.outcome.server.conflicts;
        let mut open = BTreeSet::new();
        for conflict in conflicts.iter().filter(|c| c.state == "open") {
            if !open.insert(conflict.row.as_str()) {
                self.fail(format!("two conflicts on {} are open", conflict.row));
            }
        }
        for old in conflicts.iter().filter(|c| c.state == "replaced") {
            for offered in std::iter::once(&old.loser).chain(&old.carried) {
                let handed_on = conflicts.iter().any(|c| {
                    c.row == old.row
                        && c.state != "replaced"
                        && (&c.loser == offered || c.carried.contains(offered))
                });
                if !handed_on {
                    self.fail(format!(
                        "the replaced conflict on {} offered {offered} back, and no later \
                         conflict on the row takes it over",
                        old.row
                    ));
                }
            }
        }
    }

    fn device_outcome(&mut self, device: &str, found: &DeviceOutcome) {
        let place = format!("device {device}");
        match self.resolve(device, 0) {
            Some(rows) => self.complete(&place, &rows),
            None => self.fail(format!(
                "{place}: `same_as` does not lead to a list of rows"
            )),
        }
        for shown in &found.shown {
            if !SHOWN.contains(&shown.as_str()) {
                self.fail(format!("{place}: unknown `shown` value {shown}"));
            }
        }
        for step in found.kept.iter().chain(&found.blocked) {
            // Only a change made on this device can be kept or blocked on it.
            if self.steps.get(step.as_str()).copied() != Some(device) {
                self.fail(format!(
                    "{place}: kept or blocked names {step}, not a step of this device"
                ));
            }
        }
        for row in found
            .review
            .iter()
            .chain(&found.broken)
            .chain(found.expands.values())
            .chain(found.access.keys())
        {
            self.known_row(&place, row);
        }
        for permission in found.access.values() {
            if !matches!(permission.as_str(), "view" | "edit" | "manage" | "none") {
                self.fail(format!("{place}: unknown permission {permission}"));
            }
        }
    }

    /// The rows of a device's outcome with `same_as` followed; `None` when it leads nowhere.
    fn resolve(&self, device: &str, depth: usize) -> Option<Vec<RowOutcome>> {
        let outcome = &self.scenario.outcome;
        let found = outcome.device.get(device)?;
        let mut rows = match found.same_as.as_deref() {
            None => Vec::new(),
            Some("server") => outcome.server.rows.clone(),
            Some(other) if depth < 4 && other != device => self.resolve(other, depth + 1)?,
            Some(_) => return None,
        };
        for row in &found.rows {
            if found.same_as.is_some() {
                rows.retain(|r| r.row != row.row);
            }
            rows.push(row.clone());
        }
        Some(rows)
    }

    /// Checks that `rows` states every row of the scenario exactly once, each validly.
    fn complete(&mut self, place: &str, rows: &[RowOutcome]) {
        let mut seen = BTreeSet::new();
        for row in rows {
            self.known_row(place, &row.row);
            if !seen.insert(row.row.as_str()) {
                self.fail(format!("{place}: row {} is stated twice", row.row));
            }
            if !ROW_STATES.contains(&row.state.as_str()) {
                self.fail(format!(
                    "{place}: row {} has unknown state {}",
                    row.row, row.state
                ));
            }
            match (&row.version, row.state == "absent") {
                (None, true) => {}
                (Some(_), true) => {
                    self.fail(format!("{place}: the absent row {} has a version", row.row));
                }
                (None, false) => self.fail(format!("{place}: row {} has no version", row.row)),
                (Some(version), false) => self.version(place, &row.row, version),
            }
        }
        let missing: Vec<&str> = self
            .rows
            .keys()
            .copied()
            .filter(|row| !seen.contains(row))
            .collect();
        if !missing.is_empty() {
            self.fail(format!(
                "{place}: no outcome is defined for {}",
                missing.join(", ")
            ));
        }
    }

    fn known_row(&mut self, place: &str, row: &str) {
        if !self.rows.contains_key(row) {
            self.fail(format!("{place}: unknown row {row}"));
        }
    }

    fn version(&mut self, place: &str, row: &str, version: &str) {
        if version == "start" {
            if !self.start_rows.contains(row) {
                self.fail(format!("{place}: row {row} did not exist at the start"));
            }
        } else if !self.steps.contains_key(version) {
            self.fail(format!(
                "{place}: row {row} names the unknown step {version}"
            ));
        }
    }
}
