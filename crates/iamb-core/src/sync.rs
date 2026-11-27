//! The data that crosses between the sync agent and the service (`docs/sync-design.md` §7.2).
//!
//! The service owns the store and has no network code (ADR-0008); the agent (`iamb-sync`) has the
//! network and no store. These types are what the two say to each other about rows: the changes
//! the store hands out ([`Change`], D1), the server's answer to each ([`OpResult`], D4 and D5), the
//! rows a pull brings ([`RemoteRow`], D2 and D3) and the state the agent and the interface read.
//! They are plain data: the store implements the rules, the IPC crate carries them, the agent fills
//! them from the server's answers. Nothing here performs I/O.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::{
    Attachment, Folder, FolderShare, Id, Membership, Profile, Snippet, SnippetAttachment,
    SnippetTag, SyncMeta, Tag, Timestamp,
};

/// The nine synced tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    /// A profile (personal or group).
    Profile,
    /// A membership of a group.
    Membership,
    /// A folder.
    Folder,
    /// A share of a folder with a group.
    FolderShare,
    /// A snippet.
    Snippet,
    /// A tag.
    Tag,
    /// A snippet–tag link.
    SnippetTag,
    /// Attachment metadata.
    Attachment,
    /// A snippet–attachment link.
    SnippetAttachment,
}

impl RowKind {
    /// Every kind, in the store's dependency order (a row refers only to kinds before it, except
    /// that links refer to tags and attachments).
    pub const ALL: [Self; 9] = [
        Self::Profile,
        Self::Membership,
        Self::Folder,
        Self::FolderShare,
        Self::Snippet,
        Self::Tag,
        Self::SnippetTag,
        Self::Attachment,
        Self::SnippetAttachment,
    ];

    /// The table name, as the server and the store spell it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Membership => "membership",
            Self::Folder => "folder",
            Self::FolderShare => "folder_share",
            Self::Snippet => "snippet",
            Self::Tag => "tag",
            Self::SnippetTag => "snippet_tag",
            Self::Attachment => "attachment",
            Self::SnippetAttachment => "snippet_attachment",
        }
    }

    /// Parses [`as_str`](Self::as_str) output.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Whether only the server writes rows of this kind (C16).
    #[must_use]
    pub const fn server_only(self) -> bool {
        matches!(self, Self::Profile | Self::Membership)
    }
}

impl fmt::Display for RowKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One synced row of any kind, tagged on the wire by `entity` with the row under `row`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entity", content = "row", rename_all = "snake_case")]
pub enum AnyRow {
    /// A profile.
    Profile(Profile),
    /// A membership.
    Membership(Membership),
    /// A folder.
    Folder(Folder),
    /// A folder share.
    FolderShare(FolderShare),
    /// A snippet.
    Snippet(Snippet),
    /// A tag.
    Tag(Tag),
    /// A snippet–tag link.
    SnippetTag(SnippetTag),
    /// Attachment metadata.
    Attachment(Attachment),
    /// A snippet–attachment link.
    SnippetAttachment(SnippetAttachment),
}

impl AnyRow {
    /// The kind of row.
    #[must_use]
    pub const fn kind(&self) -> RowKind {
        match self {
            Self::Profile(_) => RowKind::Profile,
            Self::Membership(_) => RowKind::Membership,
            Self::Folder(_) => RowKind::Folder,
            Self::FolderShare(_) => RowKind::FolderShare,
            Self::Snippet(_) => RowKind::Snippet,
            Self::Tag(_) => RowKind::Tag,
            Self::SnippetTag(_) => RowKind::SnippetTag,
            Self::Attachment(_) => RowKind::Attachment,
            Self::SnippetAttachment(_) => RowKind::SnippetAttachment,
        }
    }

    /// The row's sync fields.
    #[must_use]
    pub const fn meta(&self) -> &SyncMeta {
        match self {
            Self::Profile(v) => &v.meta,
            Self::Membership(v) => &v.meta,
            Self::Folder(v) => &v.meta,
            Self::FolderShare(v) => &v.meta,
            Self::Snippet(v) => &v.meta,
            Self::Tag(v) => &v.meta,
            Self::SnippetTag(v) => &v.meta,
            Self::Attachment(v) => &v.meta,
            Self::SnippetAttachment(v) => &v.meta,
        }
    }

    /// The row's sync fields, for changing them.
    pub const fn meta_mut(&mut self) -> &mut SyncMeta {
        match self {
            Self::Profile(v) => &mut v.meta,
            Self::Membership(v) => &mut v.meta,
            Self::Folder(v) => &mut v.meta,
            Self::FolderShare(v) => &mut v.meta,
            Self::Snippet(v) => &mut v.meta,
            Self::Tag(v) => &mut v.meta,
            Self::SnippetTag(v) => &mut v.meta,
            Self::Attachment(v) => &mut v.meta,
            Self::SnippetAttachment(v) => &mut v.meta,
        }
    }

    /// The row's id.
    #[must_use]
    pub const fn id(&self) -> Id {
        self.meta().id
    }

    /// Kind and id.
    #[must_use]
    pub const fn key(&self) -> RowRef {
        RowRef {
            kind: self.kind(),
            id: self.id(),
        }
    }

    /// The profile that owns the row, where the row itself names it (folders, tags, attachments,
    /// profiles, memberships); `None` for rows whose profile is their folder's or snippet's.
    #[must_use]
    pub const fn own_profile(&self) -> Option<Id> {
        match self {
            Self::Profile(v) => Some(v.meta.id),
            Self::Membership(v) => Some(v.profile_id),
            Self::Folder(v) => Some(v.profile_id),
            Self::Tag(v) => Some(v.profile_id),
            Self::Attachment(v) => Some(v.profile_id),
            Self::FolderShare(_)
            | Self::Snippet(_)
            | Self::SnippetTag(_)
            | Self::SnippetAttachment(_) => None,
        }
    }

    /// The rows this row refers to (its folder, parent, snippet, tag, attachment, profile).
    #[must_use]
    pub fn references(&self) -> Vec<RowRef> {
        let r = |kind, id| RowRef { kind, id };
        match self {
            Self::Profile(_) => Vec::new(),
            Self::Membership(v) => vec![r(RowKind::Profile, v.profile_id)],
            Self::Folder(v) => {
                let mut out = vec![r(RowKind::Profile, v.profile_id)];
                if let Some(p) = v.parent_id {
                    out.push(r(RowKind::Folder, p));
                }
                out
            }
            Self::FolderShare(v) => vec![
                r(RowKind::Folder, v.folder_id),
                r(RowKind::Profile, v.profile_id),
            ],
            Self::Snippet(v) => vec![r(RowKind::Folder, v.folder_id)],
            Self::Tag(v) => vec![r(RowKind::Profile, v.profile_id)],
            Self::SnippetTag(v) => {
                vec![r(RowKind::Snippet, v.snippet_id), r(RowKind::Tag, v.tag_id)]
            }
            Self::Attachment(v) => vec![r(RowKind::Profile, v.profile_id)],
            Self::SnippetAttachment(v) => vec![
                r(RowKind::Snippet, v.snippet_id),
                r(RowKind::Attachment, v.attachment_id),
            ],
        }
    }
}

/// A row named by kind and id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RowRef {
    /// The table.
    pub kind: RowKind,
    /// The row.
    pub id: Id,
}

impl fmt::Display for RowRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, self.id)
    }
}

/// Who wrote a snippet's content and when the server first accepted the row (C17, D13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Origin {
    /// The author of the content; `None` is "nobody", the server cannot say (C17 case 4).
    #[serde(default)]
    pub content_by: Option<Id>,
    /// When the content was last changed, by the server's clock.
    #[serde(default)]
    pub content_at: Option<Timestamp>,
    /// The revision at which the server first accepted the row.
    #[serde(default)]
    pub created_rev: u64,
}

/// A row as the server sends it in a pull, a whole read or an answer: present, or `None` when the
/// row is no longer there for this user (purged, or no longer readable; D3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRow {
    /// Which row.
    pub key: RowRef,
    /// The row as the user may read it now, or `None`.
    pub row: Option<AnyRow>,
    /// A snippet's origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

/// How [`RemoteRow`]s are applied (§7.2 `ApplyRemote`, D3, D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyMode {
    /// From the feed: only a higher revision is written.
    #[default]
    Feed,
    /// A resync in the same epoch: equal revisions are written too.
    Reconcile,
    /// A resync that found the server restored: a row the device holds above its profile's lost
    /// line, and may write or holds in a version its own user wrote, is left as it is and queued
    /// again; every other row is taken as the server has it.
    Epoch,
}

/// What [`ApplyMode`] did with one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    /// Written.
    Applied,
    /// Left alone because of an unsent change (D2), noted as skipped.
    Shadowed,
    /// Not newer than what the device holds.
    Stale,
    /// The local row was removed.
    Removed,
    /// Could not be applied (a row it refers to is missing).
    Refused,
    /// Held above the lost line and queued to be sent again (D7).
    Requeued,
}

/// The kind of re-push (§6.3, C18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepushKind {
    /// The op is the lost version itself.
    Row,
    /// The op is a change made on top of a lost version.
    Base,
}

/// A re-push after a restore of the server (C18, D7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repush {
    /// Row or base.
    pub kind: RepushKind,
    /// The revision of the lost version.
    pub rev: u64,
    /// The epoch of the position the device held when it found the restore.
    pub epoch: String,
}

/// A row a folder's delete took with it on the device (C6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CascadeItem {
    /// The row (a folder or a snippet).
    pub key: RowRef,
    /// The revision the device had when it deleted it.
    pub base_rev: u64,
}

/// One change ready to send: one row as it is now, or its deletion (D1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// The id of the change on the wire: that of the row's newest entry.
    pub op_id: Id,
    /// Earlier entries of the row that were handed out and never answered.
    #[serde(default)]
    pub follows: Vec<Id>,
    /// Which table.
    pub kind: RowKind,
    /// The row's id.
    pub id: Id,
    /// The row as it is now; `None` for a delete.
    pub row: Option<AnyRow>,
    /// The revision the device had when it made its first unsent change to the row.
    pub base_rev: u64,
    /// For a folder's delete: the rows deleted with it.
    #[serde(default)]
    pub cascade: Vec<CascadeItem>,
    /// For a re-push after a restore.
    #[serde(default)]
    pub repush: Option<Repush>,
    /// The device holds its own user as the author of the snippet's content, or as the writer of
    /// the attachment row (C17, C19); said only with a re-push.
    #[serde(default)]
    pub author_self: bool,
    /// The outbox entries the change covers, the cascade's included.
    pub seqs: Vec<i64>,
}

impl Change {
    /// Whether this is a delete.
    #[must_use]
    pub const fn is_delete(&self) -> bool {
        self.row.is_none()
    }

    /// Kind and id.
    #[must_use]
    pub const fn row_ref(&self) -> RowRef {
        RowRef {
            kind: self.kind,
            id: self.id,
        }
    }
}

/// What `OutboxRead` answers: the changes to send, and the rows whose last entry was dropped (a
/// row created and deleted before any entry was handed out, D1) and that are behind what a pull
/// skipped (D15): read them with `rows`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxBatch {
    /// The changes, in dependency order.
    pub changes: Vec<Change>,
    /// Rows now behind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub behind: Vec<RowRef>,
}

/// The status of one op's result (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpStatus {
    /// A new revision was written.
    Applied,
    /// The row already was what was sent.
    Unchanged,
    /// A stale change to a share, or a re-push another re-push preceded: the server's row stands.
    Superseded,
    /// A folder's delete that is not finished; send it again.
    Partial,
    /// Refused for good.
    Rejected,
    /// Cannot be stored as it is; stays on the device.
    Blocked,
}

impl OpStatus {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Unchanged => "unchanged",
            Self::Superseded => "superseded",
            Self::Partial => "partial",
            Self::Rejected => "rejected",
            Self::Blocked => "blocked",
        }
    }

    /// Parses [`as_str`](Self::as_str) output.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Applied,
            Self::Unchanged,
            Self::Superseded,
            Self::Partial,
            Self::Rejected,
            Self::Blocked,
        ]
        .into_iter()
        .find(|v| v.as_str() == s)
    }
}

/// The answer for one row of a folder delete's cascade list (§6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CascadeResult {
    /// The row.
    pub key: RowRef,
    /// Its revision on the server after the request; `None` when gone.
    #[serde(default)]
    pub rev: Option<u64>,
    /// Whether it is deleted after the request.
    #[serde(default)]
    pub deleted: bool,
    /// The caller may not read it, or it does not exist.
    #[serde(default)]
    pub gone: bool,
    /// The whole row did not fit; read it with `rows` (D15).
    #[serde(default)]
    pub fetch: bool,
    /// The whole row, where the device does not hold that content.
    #[serde(default)]
    pub row: Option<AnyRow>,
    /// Its origin, for a snippet.
    #[serde(default)]
    pub origin: Option<Origin>,
}

/// The server's result for one [`Change`], with the outbox entries it covered (D4, D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpResult {
    /// The change's op id.
    pub op_id: Id,
    /// Which row.
    pub key: RowRef,
    /// What the server did.
    pub status: OpStatus,
    /// Why, for `rejected` and `blocked`.
    #[serde(default)]
    pub reason: Option<String>,
    /// The conflict the write recorded.
    #[serde(default)]
    pub conflict: Option<Id>,
    /// Fields the server set differently (C8).
    #[serde(default)]
    pub adjusted: Vec<String>,
    /// The op id was seen before (C15).
    #[serde(default)]
    pub replayed: bool,
    /// The row as the server has it now and the caller may read it.
    #[serde(default)]
    pub row: Option<AnyRow>,
    /// A snippet's origin.
    #[serde(default)]
    pub origin: Option<Origin>,
    /// For a folder's delete: an entry for every row of the cascade list.
    #[serde(default)]
    pub cascade: Vec<CascadeResult>,
    /// The outbox entries the change covered ([`Change::seqs`]).
    pub seqs: Vec<i64>,
    /// The change was a re-push after a restore (D5: a refused re-push is kept only when the
    /// device's own user wrote that version).
    #[serde(default)]
    pub repush: bool,
}

/// What `OutboxAck` reports back (§7.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckOutcome {
    /// Rows now behind what the device skipped (D15): read them with `rows`.
    #[serde(default)]
    pub behind: Vec<RowRef>,
    /// Rows a resync had left alone that are now removed (D7).
    #[serde(default)]
    pub removed: Vec<RowRef>,
}

/// A permission on a folder as the server computed it (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Read.
    View,
    /// Read and change what is inside.
    Edit,
    /// Everything, the folder itself included.
    Manage,
}

impl Access {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Edit => "edit",
            Self::Manage => "manage",
        }
    }

    /// Parses [`as_str`](Self::as_str) output.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "view" => Some(Self::View),
            "edit" => Some(Self::Edit),
            "manage" => Some(Self::Manage),
            _ => None,
        }
    }
}

/// One access root of the user outside their personal profile (§6.5 `access`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRoot {
    /// The root folder.
    pub folder_id: Id,
    /// Its profile.
    pub profile_id: Id,
    /// The user's permission there.
    pub permission: Access,
    /// `own`, `group` or `share`.
    pub via: String,
    /// When access to the root began, by the server's clock.
    pub granted_at: Timestamp,
}

/// The line of a restore for one profile: the last revision of it the restored database held (C18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LostLine {
    /// The epoch that followed the device's.
    pub epoch: String,
    /// When that epoch began.
    pub started_at: Timestamp,
    /// The epoch of the position the device held when it found the restore.
    pub device_epoch: String,
    /// Profile → the last revision of it the backup held.
    pub profiles: BTreeMap<Id, u64>,
}

/// What a reconcile covers (§7.2 `ReconcileBegin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "folder")]
pub enum ReconcileScope {
    /// Everything (a resync, D7).
    All,
    /// One tree (a folder that became visible, D6).
    Folder(Id),
}

/// An account call by which the device's user took access away (D7, §7.1 `access_removal`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRemoval {
    /// The entry's id.
    pub id: Id,
    /// The call (`remove_member`, `leave_group`, `withdraw_invitation`, `set_member_role`,
    /// `transfer_ownership`, `delete_group`, `revoke_device`).
    pub call: String,
    /// The group it concerned.
    pub profile_id: Option<Id>,
    /// The person, invitation or device it concerned.
    pub subject: Option<Id>,
    /// The mark the server answered; `None` while unanswered.
    pub mark: Option<u64>,
    /// When the entry was written.
    pub made_at: Timestamp,
}

/// A change the server refused, or that lost its folder, kept for the user (D5, §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptChange {
    /// The entry's id.
    pub id: Id,
    /// Which row.
    pub key: RowRef,
    /// `put` or `delete`.
    pub delete: bool,
    /// The device's version of the row when it was kept (for a delete, the row that was not
    /// deleted).
    pub content: Option<AnyRow>,
    /// The content was waiting for review when it was kept (D10).
    pub held: bool,
    /// The folder's name, for the list.
    pub place: String,
    /// Why (`forbidden`, `gone`, `parent_gone`, `plan_limit`, `revoked`, …).
    pub reason: String,
    /// When the change was made.
    pub made_at: Timestamp,
    /// When it was kept.
    pub kept_at: Timestamp,
}

/// The adoption phase of a first sign-in (D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adoption {
    /// The library was adopted; the resync has not finished.
    Adopted,
    /// The resync finished; the merge has not.
    Merging,
}

/// The sync state the store keeps (§7.2 `SyncState`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SyncInfo {
    /// The account the library belongs to.
    pub account: Option<Id>,
    /// This installation's device id.
    pub device: Option<Id>,
    /// The account's personal profile.
    pub personal_profile: Option<Id>,
    /// The position in the feed.
    pub position: Option<String>,
    /// The adoption phase.
    pub adoption: Option<Adoption>,
    /// The time of a sign-out the device could not confirm with the server (D14).
    pub set_aside_at: Option<Timestamp>,
    /// The time of the last sign-out, confirmed or not, until the resync after the next sign-in
    /// (D14: signing in again resyncs). An optional addition to `SyncState`.
    #[serde(default)]
    pub signed_out_at: Option<Timestamp>,
    /// Changes waiting to be sent.
    pub waiting: u64,
    /// Changes the server cannot store yet.
    pub blocked: u64,
    /// Changes kept for the user.
    pub kept: u64,
    /// The highest revision of any row the device holds (`hello`'s `max_rev`, §6.2).
    #[serde(default)]
    pub max_rev: u64,
}

/// What `OutboxRead` selects (§6.7: in a new epoch, what took access away goes first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxPhase {
    /// Everything ready to send.
    #[default]
    All,
    /// Deletes of shares only.
    ShareDeletes,
    /// Folders only.
    Folders,
    /// Everything but live shares.
    NotShares,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_kind_round_trips() {
        for k in RowKind::ALL {
            assert_eq!(RowKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(RowKind::parse("nope"), None);
    }

    #[test]
    fn op_status_round_trips() {
        for s in [
            "applied",
            "unchanged",
            "superseded",
            "partial",
            "rejected",
            "blocked",
        ] {
            assert_eq!(OpStatus::parse(s).map(OpStatus::as_str), Some(s));
        }
    }
}
