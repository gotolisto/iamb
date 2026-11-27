//! A model of the server (feature `model`): the rules of `docs/sync-design.md` §8 (C1 to C25) as
//! the functions of `api` apply them, in memory, speaking the same JSON as
//! `server/supabase/migrations/*.sql`.
//!
//! It is what the scenario replay (`crates/iamb-testkit/tests/sync_replay.rs`) and the two-device
//! simulation (`tests/simulation.rs`) run the real store and the real agent against. It is not a
//! second server: there is one process, one transaction at a time (which is what the locks of C24
//! make the real server equivalent to), no rate limits and no attachment bytes. What it models:
//!
//! | Area | Rules |
//! | --- | --- |
//! | Revisions, last write wins, the whole row | C1, C2, C4 |
//! | History and conflicts, three writers, `follows` | C3, §5.6 |
//! | Deletes, a folder's delete and its cascade answer, restoring folders on the way up | C5, C6, C7 |
//! | The tree kept valid (`adjusted`), rows that would change profile (`blocked: invalid`) | C8 |
//! | Links, attachments that never change | C10, C19 (rows only) |
//! | Shares that never win by being late | C11 |
//! | Permission judged on arrival, §4.2 and §4.3 | C12 |
//! | Rows the server does not know, size limits | C13, C14 |
//! | Receipts (`replayed`) | C15 |
//! | Account calls, marks, memberships | C16 |
//! | Who wrote a snippet's content | C17 |
//! | Backups, restores, epochs, `lost`, re-pushes, the operator's replay | C18 |
//! | Visibility: `folder_visible`, `folder_revoked` with `except`, the feed | C21, §5.5 |
//! | Thirty days | C22 |
//! | Protocol versions | C23 |
//! | Sessions, devices, positions | C25 |
//!
//! After every call it checks I7 (no cycle, at most four levels, no live row under a deleted
//! folder) and records a violation in [`Server::violations`].

#![allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::many_single_char_names,
    clippy::unused_self,
    clippy::type_complexity,
    clippy::struct_field_names,
    clippy::field_reassign_with_default,
    clippy::single_match_else,
    clippy::manual_let_else,
    clippy::collapsible_match,
    clippy::needless_pass_by_value,
    reason = "test support: one function per rule of the design, kept close to its text"
)]

mod account;
mod device;
mod push;

pub use device::StoreDevice;

use std::collections::{BTreeMap, BTreeSet};

use iamb_core::model::{Id, SyncMeta, Timestamp};
use iamb_core::sync::{Access, AnyRow, RowKind, RowRef};
use serde_json::{Value, json};

use crate::api::{CallError, Transport};

/// Who is calling: the account and the authentication session of the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caller {
    /// The account.
    pub user: Id,
    /// The session the token belongs to.
    pub session: Id,
}

/// A row as the server stores it: the model row and the server-only columns (§5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    /// The row with its `rev`, times, author and `deleted_at`.
    pub row: AnyRow,
    /// The revision at which the server first accepted the row.
    pub created_rev: u64,
    /// The device that wrote the revision.
    pub updated_device: Option<Id>,
    /// The pushed change that wrote the revision.
    pub updated_op: Option<Id>,
    /// The highest lost revision the row took in again after a restore.
    pub lost_rev: Option<u64>,
    /// Who last changed the snippet's content (C17).
    pub content_by: Option<Id>,
    /// When.
    pub content_at: Option<i64>,
    /// An attachment row whose creator could read the file (C19).
    pub backed: bool,
}

/// A version a write replaced (§5.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hist {
    /// The row.
    pub key: RowRef,
    /// Its revision.
    pub rev: u64,
    /// The version as it was.
    pub stored: Stored,
    /// The revision that replaced it.
    pub replaced_by: u64,
}

/// One version a conflict offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offered {
    /// The version's revision.
    pub rev: u64,
    /// Its author.
    pub user: Option<Id>,
    /// Its device.
    pub device: Option<Id>,
    /// Taken over from a conflict this one replaced.
    pub taken_over: bool,
}

/// A recorded conflict (§5.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Its id.
    pub id: Id,
    /// The row.
    pub key: RowRef,
    /// `edit_edit`, `edit_over_delete`, `delete_over_edit`, `deleted_with_folder`,
    /// `folder_restored`.
    pub kind: String,
    /// The winner's revision.
    pub winner_rev: u64,
    /// The winner's author.
    pub winner_user: Id,
    /// The winner's device.
    pub winner_device: Option<Id>,
    /// `open`, `resolved`, `replaced`.
    pub state: String,
    /// The versions it offers; the first is the one the conflict's write replaced.
    pub versions: Vec<Offered>,
}

/// One entry of a user's feed (§5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedItem {
    /// A row changed.
    Row(RowRef),
    /// A tree became readable.
    Visible(Id),
    /// A tree stopped being readable.
    Revoked(Id, Vec<Id>),
    /// A conflict changed.
    Conflict(Id, RowRef),
    /// An account changed.
    Account,
    /// An invitation changed.
    Invitations,
}

/// A user's feed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feed {
    /// The last `seq` given out.
    pub head: u64,
    /// The highest `seq` deleted by retention.
    pub floor: u64,
    /// The entries by `seq`, one per row (§5.5).
    pub entries: BTreeMap<u64, (FeedItem, i64)>,
}

/// A device row (§5.3, C25).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    /// The account.
    pub user: Id,
    /// The session it belongs to.
    pub session: Id,
    /// Revoked.
    pub revoked: bool,
    /// The position issued last.
    pub position: Option<String>,
    /// The position that the call it was issued to had presented.
    pub position_before: Option<String>,
    /// The protocol agreed.
    pub protocol: u32,
}

/// A membership is a synced row; an invitation is not (§5.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation {
    /// The group.
    pub group: Id,
    /// The invited address.
    pub email: String,
    /// `admin` or `member`.
    pub role: String,
    /// `pending`, `accepted`, `declined`, `withdrawn`.
    pub status: String,
}

/// An epoch (§5.11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Epoch {
    /// Its id.
    pub id: String,
    /// When it began.
    pub started_at: i64,
    /// Its first revision.
    pub first_rev: u64,
    /// Each profile's `head_rev` when it began.
    pub backup_rev: BTreeMap<Id, u64>,
}

/// Everything a backup of the database holds.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// The revision counter.
    pub rev_seq: u64,
    /// The epochs, the current last.
    pub epochs: Vec<Epoch>,
    /// The synced rows, tombstones included.
    pub rows: BTreeMap<RowRef, Stored>,
    /// Replaced versions.
    pub history: Vec<Hist>,
    /// Conflicts.
    pub conflicts: Vec<Conflict>,
    /// Receipts: (device, op id) → (status, reason, conflict, adjusted).
    pub receipts: BTreeMap<(Id, Id), (String, Option<String>, Option<Id>, Vec<String>)>,
    /// Feeds by user.
    pub feeds: BTreeMap<Id, Feed>,
    /// Devices by id.
    pub devices: BTreeMap<Id, DeviceRow>,
    /// The authentication service's sessions (restored with the database, §11.6).
    pub sessions: BTreeSet<Id>,
    /// Accounts: user → (personal profile, email).
    pub users: BTreeMap<Id, (Id, String)>,
    /// Invitations.
    pub invitations: BTreeMap<Id, Invitation>,
    /// Offers of ownership: group → user.
    pub offers: BTreeMap<Id, Id>,
    /// When each user's access to each folder began (`granted_at`).
    pub granted: BTreeMap<(Id, Id), i64>,
    /// The revision at which that access began (`since_rev`, §5.4; ADR-0031).
    pub granted_rev: BTreeMap<(Id, Id), u64>,
    /// Each profile's `head_rev` (C18).
    pub head_rev: BTreeMap<Id, u64>,
    /// Protocol range.
    pub protocol: (u32, u32),
    /// Plan limits that are set (`shared_folders_max`, `snippets_max`).
    pub limits: BTreeMap<String, i64>,
    /// Maintenance (C18).
    pub maintenance: bool,
}

/// A line of the function log, kept outside the database (§11.1): what the operator's replay
/// reads after a restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// The server's clock.
    pub at: i64,
    /// The call.
    pub call: String,
    /// The caller.
    pub user: Id,
    /// The group.
    pub group: Option<Id>,
    /// The person, invitation, device or share concerned.
    pub subject: Option<Id>,
    /// The arguments to make it again.
    pub args: Value,
}

/// An op the server processed (instrumentation for the simulation's ledger, not part of the
/// server).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Processed {
    /// The device.
    pub device: Id,
    /// The op id.
    pub op_id: Id,
    /// The row.
    pub key: RowRef,
    /// The row as sent (`None` for a delete).
    pub row: Option<AnyRow>,
    /// The status answered.
    pub status: String,
    /// The revision written, if any.
    pub rev: Option<u64>,
    /// The server's clock.
    pub at: i64,
}

/// Size limits (C14) and the bound of a cascade (C6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// `body` in bytes.
    pub body_max: usize,
    /// `label` and names in bytes.
    pub name_max: usize,
    /// `cascade_max`.
    pub cascade_max: usize,
    /// Feed entries kept (days, in milliseconds).
    pub retention_ms: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            body_max: 400_000,
            name_max: 1_024,
            cascade_max: 2_000,
            retention_ms: 30 * 86_400_000,
        }
    }
}

/// The model of the server.
#[derive(Debug, Clone)]
pub struct Server {
    /// The database.
    pub st: State,
    /// Backups by name.
    pub backups: BTreeMap<String, (State, i64)>,
    /// The server's clock, milliseconds.
    pub now: i64,
    /// The function log.
    pub log: Vec<LogLine>,
    /// Ops processed (instrumentation).
    pub processed: Vec<Processed>,
    /// I7 violations seen after a call.
    pub violations: Vec<String>,
    /// Size limits.
    pub limits: Limits,
    /// Pokes sent: (user, epoch, head), drained by the caller (the live channel).
    pub pokes: Vec<(Id, String, u64)>,
    next_id: u64,
    seed: u64,
}

/// The access a user has to one folder (§4.2, `folder_access`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grant {
    /// The permission.
    pub access: Access,
    /// `own`, `group` or `share`.
    pub via: &'static str,
    /// The user's access root for the folder.
    pub root: Id,
}

/// A transaction under way: the readers of each touched row before it, and each user's readable
/// folders before it (C21).
#[derive(Debug, Default)]
pub(crate) struct Tx {
    pub(crate) before: BTreeMap<RowRef, BTreeSet<Id>>,
    pub(crate) access_before: BTreeMap<Id, BTreeMap<Id, Grant>>,
    pub(crate) conflicts: Vec<(Id, BTreeSet<Id>, RowRef)>,
    pub(crate) account: BTreeSet<Id>,
    pub(crate) folders_before: BTreeSet<Id>,
}

fn rank(a: Access) -> u8 {
    match a {
        Access::View => 1,
        Access::Edit => 2,
        Access::Manage => 3,
    }
}

impl Server {
    /// A server with no accounts, in its first epoch, at `now`.
    #[must_use]
    pub fn new(now: i64, seed: u64) -> Self {
        let mut s = Self {
            st: State {
                protocol: (1, 1),
                ..State::default()
            },
            backups: BTreeMap::new(),
            now,
            log: Vec::new(),
            processed: Vec::new(),
            violations: Vec::new(),
            limits: Limits::default(),
            pokes: Vec::new(),
            next_id: 1,
            seed,
        };
        let id = s.new_id().to_string();
        s.st.epochs.push(Epoch {
            id,
            started_at: now,
            first_rev: 1,
            backup_rev: BTreeMap::new(),
        });
        s.st.rev_seq = 0;
        s
    }

    /// A new UUID version 7 id, deterministic for the seed.
    pub fn new_id(&mut self) -> Id {
        self.next_id += 1;
        let mut r = crate::schedule::SplitMix(self.seed ^ self.next_id.wrapping_mul(0x9E37));
        let mut random = [0u8; 10];
        for b in &mut random {
            *b = u8::try_from(r.next_u64() & 0xff).unwrap_or(0);
        }
        Id::v7(u64::try_from(self.now.max(0)).unwrap_or(0), random)
    }

    /// The current epoch.
    #[must_use]
    pub fn epoch(&self) -> &Epoch {
        &self.st.epochs[self.st.epochs.len() - 1]
    }

    /// Creates an account with its personal profile and a session; answers (user, personal
    /// profile, session).
    pub fn add_user(&mut self, email: &str) -> (Id, Id, Id) {
        let user = self.new_id();
        let personal = self.new_id();
        let session = self.new_id();
        let rev = self.next_rev(personal);
        let row = AnyRow::Profile(iamb_core::model::Profile {
            meta: iamb_core::model::SyncMeta {
                id: personal,
                created_at: Timestamp(self.now),
                updated_at: Timestamp(self.now),
                updated_by: Some(user),
                rev,
                deleted_at: None,
            },
            kind: iamb_core::model::ProfileKind::Personal,
            name: "Personal".into(),
            owner_user_id: user,
        });
        self.st.rows.insert(
            row.key(),
            Stored {
                row,
                created_rev: rev,
                updated_device: None,
                updated_op: None,
                lost_rev: None,
                content_by: None,
                content_at: None,
                backed: false,
            },
        );
        self.st.users.insert(user, (personal, email.to_owned()));
        self.st.sessions.insert(session);
        self.st.feeds.entry(user).or_default();
        let mut tx = Tx::default();
        tx.before.insert(
            RowRef {
                kind: RowKind::Profile,
                id: personal,
            },
            BTreeSet::new(),
        );
        self.commit(tx);
        (user, personal, session)
    }

    /// A new session for an account (a sign-in).
    pub fn add_session(&mut self) -> Id {
        let s = self.new_id();
        self.st.sessions.insert(s);
        s
    }

    /// Draws the next revision under `profile`'s lock and records it as the profile's `head_rev`.
    pub fn next_rev(&mut self, profile: Id) -> u64 {
        self.st.rev_seq += 1;
        let r = self.st.rev_seq;
        self.st.head_rev.insert(profile, r);
        r
    }

    // ---- access (§4.2) ----------------------------------------------------------------------

    /// The live membership role of `user` in `group`.
    #[must_use]
    pub fn role(&self, user: Id, group: Id) -> Option<String> {
        self.st.rows.values().find_map(|s| match &s.row {
            AnyRow::Membership(m)
                if m.profile_id == group && m.user_id == user && m.meta.deleted_at.is_none() =>
            {
                Some(
                    match m.role {
                        iamb_core::model::Role::Owner => "owner",
                        iamb_core::model::Role::Admin => "admin",
                        iamb_core::model::Role::Member => "member",
                    }
                    .to_owned(),
                )
            }
            _ => None,
        })
    }

    fn profile_row(&self, id: Id) -> Option<&iamb_core::model::Profile> {
        match self.st.rows.get(&RowRef {
            kind: RowKind::Profile,
            id,
        }) {
            Some(Stored {
                row: AnyRow::Profile(p),
                ..
            }) => Some(p),
            _ => None,
        }
    }

    /// Whether `user` manages `profile`: its owner, or owner or admin of the group (§4.2).
    #[must_use]
    pub fn manages(&self, user: Id, profile: Id) -> bool {
        let Some(p) = self.profile_row(profile) else {
            return false;
        };
        if p.meta.deleted_at.is_some() {
            return false;
        }
        if p.kind == iamb_core::model::ProfileKind::Personal {
            return p.owner_user_id == user;
        }
        matches!(self.role(user, profile).as_deref(), Some("owner" | "admin"))
    }

    pub(crate) fn folder(&self, id: Id) -> Option<&iamb_core::model::Folder> {
        match self.st.rows.get(&RowRef {
            kind: RowKind::Folder,
            id,
        }) {
            Some(Stored {
                row: AnyRow::Folder(f),
                ..
            }) => Some(f),
            _ => None,
        }
    }

    pub(crate) fn snippet(&self, id: Id) -> Option<&iamb_core::model::Snippet> {
        match self.st.rows.get(&RowRef {
            kind: RowKind::Snippet,
            id,
        }) {
            Some(Stored {
                row: AnyRow::Snippet(s),
                ..
            }) => Some(s),
            _ => None,
        }
    }

    /// The folders from `id` up to the top (itself first).
    pub(crate) fn chain(&self, id: Id) -> Vec<Id> {
        let mut out = Vec::new();
        let mut cur = Some(id);
        while let Some(f) = cur {
            if out.contains(&f) || out.len() > 16 {
                break;
            }
            out.push(f);
            cur = self.folder(f).and_then(|x| x.parent_id);
        }
        out
    }

    /// Every folder `user` may read, with the permission and the root (§4.2, §5.4). Deleted
    /// folders keep the entries they would have if live.
    #[must_use]
    pub fn access_map(&self, user: Id) -> BTreeMap<Id, Grant> {
        let groups: BTreeMap<Id, String> = self
            .st
            .rows
            .values()
            .filter_map(|s| match &s.row {
                AnyRow::Membership(m) if m.user_id == user && m.meta.deleted_at.is_none() => {
                    let live = self
                        .profile_row(m.profile_id)
                        .is_some_and(|p| p.meta.deleted_at.is_none());
                    live.then(|| {
                        (
                            m.profile_id,
                            match m.role {
                                iamb_core::model::Role::Member => "member",
                                _ => "manage",
                            }
                            .to_owned(),
                        )
                    })
                }
                _ => None,
            })
            .collect();
        let shares: Vec<(Id, Id, Access)> = self
            .st
            .rows
            .values()
            .filter_map(|s| match &s.row {
                AnyRow::FolderShare(x) if x.meta.deleted_at.is_none() => Some((
                    x.folder_id,
                    x.profile_id,
                    match x.permission {
                        iamb_core::model::Permission::Edit => Access::Edit,
                        iamb_core::model::Permission::View => Access::View,
                    },
                )),
                _ => None,
            })
            .collect();
        let mut perm: BTreeMap<Id, (Access, &'static str)> = BTreeMap::new();
        for s in self.st.rows.values() {
            let AnyRow::Folder(f) = &s.row else { continue };
            let mut best: Option<(Access, &'static str)> = None;
            let mut take = |a: Access, via: &'static str| {
                if best.is_none_or(|(b, _)| rank(a) > rank(b)) {
                    best = Some((a, via));
                }
            };
            if let Some(p) = self.profile_row(f.profile_id) {
                if p.kind == iamb_core::model::ProfileKind::Personal && p.owner_user_id == user {
                    take(Access::Manage, "own");
                } else if let Some(role) = groups.get(&f.profile_id) {
                    if role == "manage" {
                        take(Access::Manage, "group");
                    } else {
                        let chain = self.chain(f.meta.id);
                        let top = chain.last().and_then(|t| self.folder(*t));
                        let mp = top.and_then(|t| t.member_permission);
                        take(
                            if mp == Some(iamb_core::model::Permission::Edit) {
                                Access::Edit
                            } else {
                                Access::View
                            },
                            "group",
                        );
                    }
                }
            }
            for anc in self.chain(f.meta.id) {
                for (sf, g, a) in &shares {
                    if *sf == anc && groups.contains_key(g) {
                        take(*a, "share");
                    }
                }
            }
            if let Some(b) = best {
                perm.insert(f.meta.id, b);
            }
        }
        let mut out = BTreeMap::new();
        for (&id, &(access, via)) in &perm {
            let root = self
                .chain(id)
                .into_iter()
                .take_while(|f| perm.contains_key(f))
                .last()
                .unwrap_or(id);
            out.insert(id, Grant { access, via, root });
        }
        out
    }

    /// `user`'s permission on a folder, if any.
    #[must_use]
    pub fn permission(&self, user: Id, folder: Id) -> Option<Grant> {
        self.access_map(user).get(&folder).copied()
    }

    fn has_edit_in(&self, user: Id, profile: Id) -> bool {
        let map = self.access_map(user);
        map.iter().any(|(f, g)| {
            rank(g.access) >= 2 && self.folder(*f).is_some_and(|x| x.profile_id == profile)
        })
    }

    fn own_profiles(&self, user: Id) -> BTreeSet<Id> {
        let mut out: BTreeSet<Id> = self
            .st
            .rows
            .values()
            .filter_map(|s| match &s.row {
                AnyRow::Membership(m) if m.user_id == user && m.meta.deleted_at.is_none() => {
                    Some(m.profile_id)
                }
                _ => None,
            })
            .collect();
        if let Some((p, _)) = self.st.users.get(&user) {
            out.insert(*p);
        }
        out
    }

    /// Whether `user` may read the row now (§5.13).
    #[must_use]
    pub fn can_read(&self, user: Id, key: RowRef) -> bool {
        let map = self.access_map(user);
        self.can_read_with(user, key, &map)
    }

    /// Whether `user` reads `meta`'s row through the access entry for `folder` (§5.4): the row
    /// is live, or `user` deleted it, or it was deleted after that entry began. An entry of the
    /// transaction under way has not begun and reads no tombstone but its user's own.
    fn reads_through(&self, user: Id, folder: Id, meta: &SyncMeta) -> bool {
        meta.deleted_at.is_none()
            || meta.updated_by == Some(user)
            || self
                .st
                .granted_rev
                .get(&(user, folder))
                .is_some_and(|since| meta.rev > *since)
    }

    /// Whether `user` reads a row of `profile` as its owner or as a member of the group: a
    /// tombstone of a group only when the membership began before the delete (§5.13).
    fn reads_in_profile(&self, user: Id, profile: Id, meta: &SyncMeta) -> bool {
        if self.st.users.get(&user).is_some_and(|(p, _)| *p == profile) {
            return true;
        }
        self.st.rows.values().any(|s| {
            matches!(&s.row, AnyRow::Membership(m)
                if m.profile_id == profile && m.user_id == user && m.meta.deleted_at.is_none())
                && (meta.deleted_at.is_none() || s.created_rev < meta.rev)
        })
    }

    fn can_read_with(&self, user: Id, key: RowRef, map: &BTreeMap<Id, Grant>) -> bool {
        let Some(s) = self.st.rows.get(&key) else {
            return false;
        };
        let meta = s.row.meta();
        // The folder of a snippet the user can read (a tombstone under the rule of §5.4).
        let snippet_folder = |sid: Id| {
            self.snippet(sid).and_then(|sn| {
                (map.contains_key(&sn.folder_id)
                    && self.reads_through(user, sn.folder_id, &sn.meta))
                .then_some(sn.folder_id)
            })
        };
        // A live link from a live snippet the user can read. It gives the row while the row is
        // live: a deleted tag or attachment of someone else's profile is not read through a
        // link (§5.13).
        let live_link_from_readable = |kind: RowKind, target: Id| {
            meta.deleted_at.is_none()
                && self.st.rows.values().any(|x| match (&x.row, kind) {
                    (AnyRow::SnippetTag(l), RowKind::Tag) => {
                        l.tag_id == target
                            && l.meta.deleted_at.is_none()
                            && self
                                .snippet(l.snippet_id)
                                .is_some_and(|sn| sn.meta.deleted_at.is_none())
                            && snippet_folder(l.snippet_id).is_some()
                    }
                    (AnyRow::SnippetAttachment(l), RowKind::Attachment) => {
                        l.attachment_id == target
                            && l.meta.deleted_at.is_none()
                            && self
                                .snippet(l.snippet_id)
                                .is_some_and(|sn| sn.meta.deleted_at.is_none())
                            && snippet_folder(l.snippet_id).is_some()
                    }
                    _ => false,
                })
        };
        match &s.row {
            AnyRow::Folder(_) => {
                map.contains_key(&key.id) && self.reads_through(user, key.id, meta)
            }
            AnyRow::Snippet(sn) => {
                map.contains_key(&sn.folder_id) && self.reads_through(user, sn.folder_id, meta)
            }
            AnyRow::FolderShare(x) => {
                (map.get(&x.folder_id)
                    .is_some_and(|g| g.access == Access::Manage)
                    || (self.role(user, x.profile_id).is_some() && map.contains_key(&x.folder_id)))
                    && self.reads_through(user, x.folder_id, meta)
            }
            AnyRow::SnippetTag(l) => {
                snippet_folder(l.snippet_id).is_some_and(|f| self.reads_through(user, f, meta))
            }
            AnyRow::SnippetAttachment(l) => {
                snippet_folder(l.snippet_id).is_some_and(|f| self.reads_through(user, f, meta))
            }
            AnyRow::Tag(t) => {
                self.reads_in_profile(user, t.profile_id, meta)
                    || live_link_from_readable(RowKind::Tag, t.meta.id)
                    || (t.meta.updated_by == Some(user) && self.has_edit_in(user, t.profile_id))
            }
            AnyRow::Attachment(a) => {
                self.reads_in_profile(user, a.profile_id, meta)
                    || live_link_from_readable(RowKind::Attachment, a.meta.id)
                    || (a.meta.updated_by == Some(user) && self.has_edit_in(user, a.profile_id))
            }
            AnyRow::Profile(p) => {
                (p.kind == iamb_core::model::ProfileKind::Personal && p.owner_user_id == user)
                    || self.role(user, p.meta.id).is_some()
                    || map
                        .keys()
                        .any(|f| self.folder(*f).is_some_and(|x| x.profile_id == p.meta.id))
            }
            AnyRow::Membership(m) => {
                m.user_id == user
                    || (m.meta.deleted_at.is_none() && self.role(user, m.profile_id).is_some())
            }
        }
    }

    /// The users who may read the row now.
    #[must_use]
    pub fn readers(&self, key: RowRef) -> BTreeSet<Id> {
        self.st
            .users
            .keys()
            .copied()
            .filter(|u| self.can_read(*u, key))
            .collect()
    }

    /// The row as `user` may read it, in the wire form, with its origin; `None` when unreadable.
    #[must_use]
    pub fn wire_row(&self, user: Id, key: RowRef) -> Option<(Value, Value)> {
        let map = self.access_map(user);
        if !self.can_read_with(user, key, &map) {
            return None;
        }
        let s = self.st.rows.get(&key)?;
        let mut row = s.row.clone();
        if let AnyRow::Folder(f) = &mut row
            && let Some(p) = f.parent_id
            && !self.can_read_with(
                user,
                RowRef {
                    kind: RowKind::Folder,
                    id: p,
                },
                &map,
            )
        {
            f.parent_id = None;
        }
        let origin = match &s.row {
            AnyRow::Snippet(_) => json!({
                "content_by": s.content_by, "content_at": s.content_at, "created_rev": s.created_rev
            }),
            _ => Value::Null,
        };
        Some((crate::api::row_to_wire(&row), origin))
    }

    // ---- transactions and the feed (§5.5, C21) ------------------------------------------------

    pub(crate) fn begin(&self) -> Tx {
        let mut tx = Tx::default();
        tx.folders_before = self
            .st
            .rows
            .keys()
            .filter(|k| k.kind == RowKind::Folder)
            .map(|k| k.id)
            .collect();
        for &u in self.st.users.keys() {
            tx.access_before.insert(u, self.access_map(u));
        }
        tx
    }

    /// Notes the readers of a row before the transaction first changes it.
    pub(crate) fn touch(&self, tx: &mut Tx, key: RowRef) {
        if !tx.before.contains_key(&key) {
            let readers: BTreeSet<Id> = tx
                .access_before
                .iter()
                .filter(|(u, map)| self.can_read_with(**u, key, map))
                .map(|(u, _)| *u)
                .collect();
            tx.before.insert(key, readers);
        }
    }

    fn queue(&mut self, user: Id, item: FeedItem) {
        let now = self.now;
        let feed = self.st.feeds.entry(user).or_default();
        if let FeedItem::Row(key) = &item {
            // One entry per row (§5.5): a newer entry replaces the older one.
            let old: Vec<u64> = feed
                .entries
                .iter()
                .filter(|(_, (i, _))| matches!(i, FeedItem::Row(k) if k == key))
                .map(|(s, _)| *s)
                .collect();
            for s in old {
                feed.entries.remove(&s);
            }
        }
        feed.head += 1;
        let seq = feed.head;
        feed.entries.insert(seq, (item, now));
    }

    /// Writes the transaction's feed entries last (§5.5): rows to everyone who could read them
    /// before or can now, visibility entries from the change of each user's readable folders, and
    /// pokes.
    pub(crate) fn commit(&mut self, tx: Tx) {
        self.commit_inner(tx);
    }

    fn commit_inner(&mut self, tx: Tx) {
        let users: Vec<Id> = self.st.users.keys().copied().collect();
        let mut touched_users: BTreeSet<Id> = BTreeSet::new();
        let mut row_items: Vec<(Id, RowRef)> = Vec::new();
        // A snippet that is live after the write brings its live links, and with them (closed
        // under references, §6.4) the tags and attachments they name: a snippet that comes back,
        // or becomes readable through a move, is whole again on every reader's device (§5.5).
        let mut tx = tx;
        let snippets: Vec<Id> = tx
            .before
            .keys()
            .filter(|k| k.kind == RowKind::Snippet)
            .map(|k| k.id)
            .filter(|id| {
                self.snippet(*id)
                    .is_some_and(|s| s.meta.deleted_at.is_none())
            })
            .collect();
        for sid in snippets {
            let links: Vec<RowRef> = self
                .st
                .rows
                .values()
                .filter(|s| match &s.row {
                    AnyRow::SnippetTag(l) => l.snippet_id == sid && l.meta.deleted_at.is_none(),
                    AnyRow::SnippetAttachment(l) => {
                        l.snippet_id == sid && l.meta.deleted_at.is_none()
                    }
                    _ => false,
                })
                .map(|s| s.row.key())
                .collect();
            for k in links {
                let readers: BTreeSet<Id> = tx
                    .before
                    .get(&RowRef {
                        kind: RowKind::Snippet,
                        id: sid,
                    })
                    .cloned()
                    .unwrap_or_default();
                tx.before.entry(k).or_insert(readers);
            }
        }
        // So does a tag or an attachment row that is live after the write: who reads it through
        // a link lost it, and its links with it, while it was deleted (§5.5, §5.13).
        let targets: Vec<RowRef> = tx
            .before
            .keys()
            .filter(|k| matches!(k.kind, RowKind::Tag | RowKind::Attachment))
            .copied()
            .filter(|k| {
                self.st
                    .rows
                    .get(k)
                    .is_some_and(|s| s.row.meta().deleted_at.is_none())
            })
            .collect();
        for t in targets {
            let links: Vec<RowRef> = self
                .st
                .rows
                .values()
                .filter(|s| match &s.row {
                    AnyRow::SnippetTag(l) => l.tag_id == t.id && l.meta.deleted_at.is_none(),
                    AnyRow::SnippetAttachment(l) => {
                        l.attachment_id == t.id && l.meta.deleted_at.is_none()
                    }
                    _ => false,
                })
                .map(|s| s.row.key())
                .collect();
            for k in links {
                let readers: BTreeSet<Id> = tx.before.get(&t).cloned().unwrap_or_default();
                tx.before.entry(k).or_insert(readers);
            }
        }
        for (key, before) in &tx.before {
            let after = self.readers(*key);
            for u in before.union(&after) {
                row_items.push((*u, *key));
            }
        }
        for (u, key) in row_items {
            self.queue(u, FeedItem::Row(key));
            touched_users.insert(u);
        }
        for u in &users {
            let before = tx.access_before.get(u).cloned().unwrap_or_default();
            let after = self.access_map(*u);
            // Tops of trees that became readable.
            // A folder the transaction created reaches its readers as a row (§5.5), not as a tree
            // that became readable.
            for (f, _) in after
                .iter()
                .filter(|(f, _)| !before.contains_key(*f) && tx.folders_before.contains(*f))
            {
                let parent_new = self
                    .folder(*f)
                    .and_then(|x| x.parent_id)
                    .is_some_and(|p| after.contains_key(&p) && !before.contains_key(&p));
                if !parent_new {
                    self.queue(*u, FeedItem::Visible(*f));
                    touched_users.insert(*u);
                }
            }
            // Tops of trees that stopped being readable while the folder still exists.
            for (f, _) in before.iter().filter(|(f, _)| !after.contains_key(*f)) {
                if self.folder(*f).is_none() {
                    continue;
                }
                let parent_lost = self
                    .folder(*f)
                    .and_then(|x| x.parent_id)
                    .is_some_and(|p| before.contains_key(&p) && !after.contains_key(&p));
                if parent_lost {
                    continue;
                }
                // The parts inside that stay readable through another grant.
                let except: Vec<Id> = after
                    .keys()
                    .filter(|g| {
                        let chain = self.chain(**g);
                        chain.contains(f)
                            && self
                                .folder(**g)
                                .and_then(|x| x.parent_id)
                                .is_none_or(|p| !after.contains_key(&p))
                    })
                    .copied()
                    .collect();
                self.queue(*u, FeedItem::Revoked(*f, except));
                touched_users.insert(*u);
            }
            // granted_at: the start of each continuous access.
            let now = self.now;
            let since = self.st.rev_seq;
            for f in after.keys() {
                self.st.granted.entry((*u, *f)).or_insert(now);
                self.st.granted_rev.entry((*u, *f)).or_insert(since);
            }
            self.st
                .granted
                .retain(|(uu, ff), _| uu != u || after.contains_key(ff));
            self.st
                .granted_rev
                .retain(|(uu, ff), _| uu != u || after.contains_key(ff));
        }
        for (cid, parties, key) in &tx.conflicts {
            // A party takes part only while they can read the row (ADR-0031).
            let readers: Vec<Id> = parties
                .iter()
                .copied()
                .filter(|p| self.can_read(*p, *key))
                .collect();
            for p in &readers {
                self.queue(*p, FeedItem::Conflict(*cid, *key));
                touched_users.insert(*p);
            }
        }
        for u in &tx.account {
            self.queue(*u, FeedItem::Account);
            touched_users.insert(*u);
        }
        let epoch = self.epoch().id.clone();
        for u in touched_users {
            let head = self.st.feeds.get(&u).map_or(0, |f| f.head);
            self.pokes.push((u, epoch.clone(), head));
        }
        self.check_tree();
    }

    /// I7: no cycle, at most four levels, no live row under a deleted folder.
    fn check_tree(&mut self) {
        let mut problems = Vec::new();
        for s in self.st.rows.values() {
            match &s.row {
                AnyRow::Folder(f) => {
                    let chain = self.chain(f.meta.id);
                    if chain.len() > 4 {
                        problems.push(format!(
                            "folder {} is {} levels deep",
                            f.meta.id,
                            chain.len()
                        ));
                    }
                    if let Some(p) = f.parent_id
                        && self.chain(p).contains(&f.meta.id)
                    {
                        problems.push(format!("folder {} is in a cycle", f.meta.id));
                    }
                    if f.meta.deleted_at.is_none()
                        && let Some(p) = f.parent_id
                        && self.folder(p).is_some_and(|x| x.meta.deleted_at.is_some())
                    {
                        problems.push(format!("live folder {} under a deleted one", f.meta.id));
                    }
                }
                AnyRow::Snippet(sn) => {
                    if sn.meta.deleted_at.is_none()
                        && self
                            .folder(sn.folder_id)
                            .is_none_or(|x| x.meta.deleted_at.is_some())
                    {
                        problems.push(format!(
                            "live snippet {} under a deleted folder",
                            sn.meta.id
                        ));
                    }
                }
                _ => {}
            }
        }
        self.violations.extend(problems);
    }

    // ---- enter and positions (C23, C25) -----------------------------------------------------

    fn err(&self, code: &str) -> Value {
        json!({ "error": code, "epoch": self.epoch().id })
    }

    /// `iamb_private.enter`: the session, the device row, the protocol, maintenance.
    fn enter(&self, c: Caller, device: Option<Id>, protocol: u32, call: &str) -> Option<Value> {
        if self.st.maintenance {
            return Some(self.err("maintenance"));
        }
        if !self.st.sessions.contains(&c.session) {
            return Some(self.err("session_refused"));
        }
        if protocol < self.st.protocol.0 && call != "visible_folders" {
            return Some(self.err("upgrade_required"));
        }
        if call == "hello" {
            return None;
        }
        let row = self.st.devices.iter().find(|(_, d)| d.session == c.session);
        match row {
            None => Some(self.err("device_unknown")),
            Some((_, d)) if d.revoked => Some(self.err("device_revoked")),
            Some((id, _)) if Some(*id) != device => Some(self.err("device_conflict")),
            Some(_) => None,
        }
    }

    fn parse_position(&self, p: Option<&str>) -> Option<(String, u64)> {
        let p = p?;
        let parts: Vec<&str> = p.split(':').collect();
        if parts.len() != 3 {
            return None;
        }
        Some((parts[0].to_owned(), parts[1].parse().ok()?))
    }

    /// `iamb_private.position` (C25).
    fn position(
        &mut self,
        c: Caller,
        device: Id,
        presented: Option<&str>,
        call: &str,
        new_seq: Option<u64>,
    ) -> Result<(Option<String>, Option<(String, u64)>), Value> {
        let epoch = self.epoch().id.clone();
        let feed = self.st.feeds.get(&c.user).cloned().unwrap_or_default();
        let Some(d) = self.st.devices.get(&device).cloned() else {
            return Err(self.err("device_unknown"));
        };
        let parsed = self.parse_position(presented);
        let ok = match call {
            "pull" | "push" => {
                let Some((pe, ps)) = &parsed else {
                    return Err(
                        json!({"error": "reset_required", "reason": "expired", "epoch": epoch}),
                    );
                };
                if *pe != epoch {
                    return Err(
                        json!({"error": "reset_required", "reason": "epoch", "epoch": epoch}),
                    );
                }
                if *ps < feed.floor || d.position.is_none() {
                    return Err(
                        json!({"error": "reset_required", "reason": "expired", "epoch": epoch}),
                    );
                }
                if *ps > feed.head {
                    return Err(self.err("server_behind"));
                }
                presented == d.position.as_deref() || presented == d.position_before.as_deref()
            }
            _ => {
                d.position.is_none()
                    || parsed
                        .as_ref()
                        .is_none_or(|(pe, ps)| *pe != epoch || *ps < feed.floor)
                    || presented == d.position.as_deref()
                    || presented == d.position_before.as_deref()
            }
        };
        if !ok {
            return Err(self.err("device_conflict"));
        }
        let mut newpos = None;
        if let Some(seq) = new_seq {
            let token = self.new_id().to_string().replace('-', "");
            let p = format!("{epoch}:{seq}:{token}");
            if let Some(dd) = self.st.devices.get_mut(&device) {
                dd.position_before = presented.map(str::to_owned);
                dd.position = Some(p.clone());
            }
            newpos = Some(p);
        }
        Ok((newpos, parsed))
    }

    // ---- the read functions (§6.4, §6.5) ------------------------------------------------------

    fn item(&self, user: Id, key: RowRef) -> Value {
        let (row, origin) = self
            .wire_row(user, key)
            .map_or((Value::Null, Value::Null), |(r, o)| (r, o));
        json!({ "entity": key.kind.as_str(), "id": key.id, "row": row, "origin": origin })
    }

    fn refs_of(&self, row: &AnyRow) -> Vec<RowRef> {
        let mut out = row.references();
        if let AnyRow::Folder(f) = row {
            for a in self.chain(f.meta.id).into_iter().skip(1) {
                out.push(RowRef {
                    kind: RowKind::Folder,
                    id: a,
                });
            }
        }
        if let AnyRow::Snippet(s) = row {
            for a in self.chain(s.folder_id) {
                out.push(RowRef {
                    kind: RowKind::Folder,
                    id: a,
                });
                if let Some(f) = self.folder(a) {
                    out.push(RowRef {
                        kind: RowKind::Profile,
                        id: f.profile_id,
                    });
                }
            }
        }
        out
    }

    fn pull(&mut self, c: Caller, args: &Value) -> Value {
        let device = arg_id(args, "device");
        let after = args.get("after").and_then(Value::as_str).map(str::to_owned);
        let limit = usize::try_from(args.get("limit").and_then(Value::as_u64).unwrap_or(500))
            .unwrap_or(500)
            .clamp(1, 500);
        let Some(device) = device else {
            return self.err("invalid");
        };
        let parsed = match self.position(c, device, after.as_deref(), "pull", None) {
            Ok((_, p)) => p,
            Err(e) => return e,
        };
        let aseq = parsed.map_or(0, |p| p.1);
        let feed = self.st.feeds.get(&c.user).cloned().unwrap_or_default();
        let mut entries = Vec::new();
        let mut last = aseq;
        let mut more = false;
        let mut known: BTreeSet<RowRef> = BTreeSet::new();
        for (seq, (item, _)) in feed.entries.range(aseq + 1..) {
            if entries.len() >= limit {
                more = true;
                break;
            }
            let v = match item {
                FeedItem::Row(key) => {
                    known.insert(*key);
                    let mut it = self.item(c.user, *key);
                    if let Value::Object(m) = &mut it {
                        m.insert("seq".into(), json!(seq));
                        m.insert("kind".into(), json!("row"));
                    }
                    it
                }
                FeedItem::Visible(f) => json!({"seq": seq, "kind": "folder_visible", "folder": f}),
                FeedItem::Revoked(f, except) => {
                    json!({"seq": seq, "kind": "folder_revoked", "folder": f, "except": except})
                }
                FeedItem::Conflict(cid, key) => json!({"seq": seq, "kind": "conflict",
                    "conflict": cid, "entity": key.kind.as_str(), "id": key.id}),
                FeedItem::Account => json!({"seq": seq, "kind": "account"}),
                FeedItem::Invitations => json!({"seq": seq, "kind": "invitations"}),
            };
            entries.push(v);
            last = *seq;
        }
        // A page is closed under references (§6.4).
        let mut refs = Vec::new();
        let mut frontier: Vec<RowRef> = known.iter().copied().collect();
        for _ in 0..8 {
            let mut next = Vec::new();
            for key in &frontier {
                let Some(s) = self.st.rows.get(key) else {
                    continue;
                };
                if !self.can_read(c.user, *key) {
                    continue;
                }
                for r in self.refs_of(&s.row) {
                    if known.insert(r) && self.can_read(c.user, r) {
                        refs.push(self.item(c.user, r));
                        next.push(r);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        let next = match self.position(c, device, after.as_deref(), "pull", Some(last)) {
            Ok((p, _)) => p,
            Err(e) => return e,
        };
        json!({ "error": null, "epoch": self.epoch().id, "next": next, "head": feed.head,
                "more": more, "entries": entries, "refs": refs })
    }

    fn rows_call(&self, c: Caller, args: &Value) -> Value {
        let mut out = Vec::new();
        for r in args
            .get("refs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(kind), Some(id)) = (
                r.get("entity")
                    .and_then(Value::as_str)
                    .and_then(RowKind::parse),
                arg_id(r, "id"),
            ) else {
                continue;
            };
            out.push(self.item(c.user, RowRef { kind, id }));
        }
        json!({ "error": null, "epoch": self.epoch().id, "rows": out })
    }

    fn fetch_folder(&self, c: Caller, args: &Value) -> Value {
        let Some(folder) = arg_id(args, "folder") else {
            return self.err("invalid");
        };
        if self
            .folder(folder)
            .is_none_or(|f| f.meta.deleted_at.is_some())
        {
            return self.err("not_found");
        }
        let map = self.access_map(c.user);
        let mut keys: Vec<RowRef> = Vec::new();
        let tree: Vec<Id> = self
            .st
            .rows
            .values()
            .filter_map(|s| match &s.row {
                AnyRow::Folder(f)
                    if f.meta.deleted_at.is_none() && self.chain(f.meta.id).contains(&folder) =>
                {
                    Some(f.meta.id)
                }
                _ => None,
            })
            .collect();
        let mut tree_sorted = tree.clone();
        tree_sorted.sort_by_key(|f| self.chain(*f).len());
        if let Some(f) = self.folder(folder) {
            keys.push(RowRef {
                kind: RowKind::Profile,
                id: f.profile_id,
            });
        }
        for f in &tree_sorted {
            keys.push(RowRef {
                kind: RowKind::Folder,
                id: *f,
            });
        }
        for s in self.st.rows.values() {
            match &s.row {
                AnyRow::FolderShare(x)
                    if x.meta.deleted_at.is_none() && tree.contains(&x.folder_id) =>
                {
                    keys.push(s.row.key());
                }
                AnyRow::Snippet(sn)
                    if sn.meta.deleted_at.is_none() && tree.contains(&sn.folder_id) =>
                {
                    keys.push(s.row.key());
                }
                _ => {}
            }
        }
        let snippets: BTreeSet<Id> = keys
            .iter()
            .filter(|k| k.kind == RowKind::Snippet)
            .map(|k| k.id)
            .collect();
        let mut links = Vec::new();
        for s in self.st.rows.values() {
            match &s.row {
                AnyRow::SnippetTag(l)
                    if l.meta.deleted_at.is_none() && snippets.contains(&l.snippet_id) =>
                {
                    links.push(RowRef {
                        kind: RowKind::Tag,
                        id: l.tag_id,
                    });
                    links.push(s.row.key());
                }
                AnyRow::SnippetAttachment(l)
                    if l.meta.deleted_at.is_none() && snippets.contains(&l.snippet_id) =>
                {
                    links.push(RowRef {
                        kind: RowKind::Attachment,
                        id: l.attachment_id,
                    });
                    links.push(s.row.key());
                }
                _ => {}
            }
        }
        keys.extend(links);
        let mut seen = BTreeSet::new();
        let rows: Vec<Value> = keys
            .into_iter()
            .filter(|k| seen.insert(*k) && self.can_read_with(c.user, *k, &map))
            .map(|k| self.item(c.user, k))
            .collect();
        let head = self.st.feeds.get(&c.user).map_or(0, |f| f.head);
        json!({ "error": null, "epoch": self.epoch().id, "rows": rows, "cursor": null, "head": head })
    }

    fn snapshot(&mut self, c: Caller, args: &Value) -> Value {
        let Some(device) = arg_id(args, "device") else {
            return self.err("invalid");
        };
        let presented = args
            .get("position")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if args.get("cursor").is_some_and(|v| !v.is_null()) {
            return json!({ "error": null, "epoch": self.epoch().id, "rows": [], "cursor": null });
        }
        let head = self.st.feeds.get(&c.user).map_or(0, |f| f.head);
        let (start, parsed) =
            match self.position(c, device, presented.as_deref(), "snapshot", Some(head)) {
                Ok(x) => x,
                Err(e) => return e,
            };
        let map = self.access_map(c.user);
        let mut keys: Vec<RowRef> = self
            .st
            .rows
            .keys()
            .copied()
            .filter(|k| self.can_read_with(c.user, *k, &map))
            .collect();
        keys.sort_by_key(|k| {
            let depth = if k.kind == RowKind::Folder {
                self.chain(k.id).len()
            } else {
                0
            };
            (RowKind::ALL.iter().position(|x| *x == k.kind), depth, k.id)
        });
        let rows: Vec<Value> = keys.into_iter().map(|k| self.item(c.user, k)).collect();
        let mut out = json!({ "error": null, "epoch": self.epoch().id, "start": start,
                              "head": head, "rows": rows, "cursor": null });
        // After a restore: what the restore lost, per profile (§6.5, C18).
        if let Some((pe, _)) = parsed
            && pe != self.epoch().id
        {
            let idx = self.st.epochs.iter().position(|e| e.id == pe);
            let ep = idx
                .and_then(|i| self.st.epochs.get(i + 1))
                .unwrap_or_else(|| self.epoch())
                .clone();
            let mut profiles: BTreeSet<Id> = self.own_profiles(c.user);
            for f in map.keys() {
                if let Some(x) = self.folder(*f) {
                    profiles.insert(x.profile_id);
                }
            }
            let lost: Vec<Value> = profiles
                .into_iter()
                .map(|p| json!({ "profile": p, "backup_rev": ep.backup_rev.get(&p).copied().unwrap_or(0) }))
                .collect();
            if let Value::Object(m) = &mut out {
                m.insert(
                    "lost".into(),
                    json!({ "epoch": ep.id, "started_at": ep.started_at, "profiles": lost }),
                );
            }
        }
        out
    }

    fn visible_folders(&self, c: Caller) -> Value {
        // A deleted folder is among them only where the caller may read its tombstone (§5.4).
        let map = self.access_map(c.user);
        let folders: Vec<Id> = map
            .keys()
            .copied()
            .filter(|f| {
                self.can_read_with(
                    c.user,
                    RowRef {
                        kind: RowKind::Folder,
                        id: *f,
                    },
                    &map,
                )
            })
            .collect();
        json!({ "error": null, "epoch": self.epoch().id, "folders": folders })
    }

    fn access(&self, c: Caller) -> Value {
        let map = self.access_map(c.user);
        let roots: Vec<Value> = map
            .iter()
            .filter(|(f, g)| {
                g.root == **f
                    && g.via != "own"
                    && self.can_read_with(
                        c.user,
                        RowRef {
                            kind: RowKind::Folder,
                            id: **f,
                        },
                        &map,
                    )
            })
            .map(|(f, g)| {
                json!({ "root": f, "profile": self.folder(*f).map(|x| x.profile_id),
                        "permission": g.access.as_str(), "via": g.via,
                        "granted_at": self.st.granted.get(&(c.user, *f)).copied().unwrap_or(0) })
            })
            .collect();
        let groups: Vec<Value> = self
            .own_profiles(c.user)
            .into_iter()
            .filter_map(|g| {
                self.role(c.user, g)
                    .map(|r| json!({ "group": g, "role": r }))
            })
            .collect();
        json!({ "error": null, "epoch": self.epoch().id, "roots": roots, "groups": groups })
    }

    /// Calls a function of `api` as `caller`.
    pub fn call(&mut self, caller: Caller, function: &str, args: &Value) -> Value {
        let device = if function == "hello" {
            args.get("device").and_then(|d| arg_id(d, "id"))
        } else {
            arg_id(args, "device")
        };
        let protocol = if function == "hello" {
            args.get("protocols")
                .and_then(Value::as_array)
                .and_then(|a| a.last())
                .and_then(Value::as_u64)
        } else {
            args.get("protocol").and_then(Value::as_u64)
        };
        let protocol = u32::try_from(protocol.unwrap_or(1)).unwrap_or(1);
        if let Some(e) = self.enter(caller, device, protocol, function) {
            return e;
        }
        match function {
            "hello" => self.hello(caller, args),
            "push" => self.push(caller, args),
            "pull" => self.pull(caller, args),
            "rows" => self.rows_call(caller, args),
            "fetch_folder" => self.fetch_folder(caller, args),
            "snapshot" => self.snapshot(caller, args),
            "visible_folders" => self.visible_folders(caller),
            "access" => self.access(caller),
            "entitlements" => {
                json!({ "error": null, "epoch": self.epoch().id, "entitlements": { "sync": "true" } })
            }
            other => self.account_call(caller, other, args),
        }
    }

    // ---- clock and retention (C22) ------------------------------------------------------------

    /// Moves the clock and runs the daily job: feed entries, receipts and tombstones older than
    /// 30 days go.
    pub fn advance(&mut self, ms: i64) {
        self.now += ms;
        let cutoff = self.now - self.limits.retention_ms;
        for feed in self.st.feeds.values_mut() {
            let old: Vec<u64> = feed
                .entries
                .iter()
                .filter(|(_, (_, at))| *at < cutoff)
                .map(|(s, _)| *s)
                .collect();
            for s in old {
                feed.entries.remove(&s);
                feed.floor = feed.floor.max(s);
            }
        }
        let purge: Vec<RowRef> = self
            .st
            .rows
            .iter()
            .filter(|(_, s)| s.row.meta().deleted_at.is_some_and(|d| d.0 < cutoff))
            .map(|(k, _)| *k)
            .collect();
        for k in purge {
            self.st.rows.remove(&k);
            self.st.history.retain(|h| h.key != k);
        }
        // Links whose ends are gone go with them.
        let gone: Vec<RowRef> = self
            .st
            .rows
            .iter()
            .filter(|(_, s)| {
                let missing = |kind, id| !self.st.rows.contains_key(&RowRef { kind, id });
                match &s.row {
                    AnyRow::SnippetTag(l) => {
                        missing(RowKind::Snippet, l.snippet_id) || missing(RowKind::Tag, l.tag_id)
                    }
                    AnyRow::SnippetAttachment(l) => {
                        missing(RowKind::Snippet, l.snippet_id)
                            || missing(RowKind::Attachment, l.attachment_id)
                    }
                    _ => false,
                }
            })
            .map(|(k, _)| *k)
            .collect();
        for k in gone {
            self.st.rows.remove(&k);
        }
    }
}

pub(crate) fn arg_id(v: &Value, key: &str) -> Option<Id> {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
}

/// A transport that calls the model in process as one caller.
pub struct ModelTransport<'a> {
    /// The model.
    pub server: &'a mut Server,
    /// Who calls.
    pub caller: Caller,
}

impl Transport for ModelTransport<'_> {
    fn call(&mut self, function: &str, args: Value) -> Result<Value, CallError> {
        // Through the JSON text, as on the wire.
        let text = args.to_string();
        let args: Value =
            serde_json::from_str(&text).map_err(|e| CallError::Malformed(e.to_string()))?;
        let answer = self.server.call(self.caller, function, &args);
        let text = answer.to_string();
        serde_json::from_str(&text).map_err(|e| CallError::Malformed(e.to_string()))
    }
}

/// A transport to a model shared by several devices, as one caller.
#[derive(Clone)]
pub struct SharedTransport {
    /// The model.
    pub server: std::rc::Rc<std::cell::RefCell<Server>>,
    /// Who calls.
    pub caller: Caller,
}

impl Transport for SharedTransport {
    fn call(&mut self, function: &str, args: Value) -> Result<Value, CallError> {
        let text = args.to_string();
        let args: Value =
            serde_json::from_str(&text).map_err(|e| CallError::Malformed(e.to_string()))?;
        let answer = self.server.borrow_mut().call(self.caller, function, &args);
        let text = answer.to_string();
        serde_json::from_str(&text).map_err(|e| CallError::Malformed(e.to_string()))
    }
}
