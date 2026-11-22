//! What the store does for sync (task 4.4; `docs/sync-design.md` §7 and the rules D1 to D16).
//!
//! The sync agent never touches the store: it asks the service, which calls these methods, each in
//! one transaction. The data crossing between them is in [`iamb_core::sync`].
//!
//! | Method | Rule |
//! | --- | --- |
//! | [`Store::sync_outbox_read`] | D1: one change per row, handed-out marks, `follows`, cascade lists |
//! | [`Store::sync_outbox_ack`] | D4, D5, D15, D7 (left-alone rows) |
//! | [`Store::sync_apply`] | D2, D3, D7 (`reconcile` and `epoch` modes), D16 |
//! | [`Store::purge_folder`] | D6, D16 |
//! | [`Store::reconcile_begin`], [`Store::reconcile_end`] | D7 |
//! | [`Store::signed_out`] | D14 |
//! | [`Store::adopt`], [`Store::adopt_merge`] | D9 |
//!
//! Every rule that removes a folder removes what the device holds in it, and moves unsent changes
//! there to the list of kept changes first (D3, D6).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use iamb_core::model::{
    Attachment, Folder, FolderShare, Id, Membership, Profile, ProfileKind, Snippet,
    SnippetAttachment, SnippetTag, Tag, Timestamp,
};
use iamb_core::sync::{
    AccessRemoval, AccessRoot, AckOutcome, Adoption, AnyRow, ApplyMode, ApplyStatus, CascadeItem,
    CascadeResult, Change, KeptChange, LostLine, OpResult, OpStatus, Origin, OutboxBatch,
    OutboxPhase, ReconcileScope, RemoteRow, Repush, RepushKind, RowKind, RowRef, SyncInfo,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::codec::{id_at, opt_id_at, u64_at};
use crate::entity::EntityKind;
use crate::error::StoreError;
use crate::{ListOptions, Store, ops};

/// Thirty days in milliseconds (`retention_days`, §5.12).
pub const RETENTION_MS: i64 = 30 * 86_400_000;

/// The most rows a dependency closure adds beyond the limit of [`Store::sync_outbox_read`].
const CLOSURE_MAX: usize = 2_000;

impl From<RowKind> for EntityKind {
    fn from(k: RowKind) -> Self {
        match k {
            RowKind::Profile => Self::Profile,
            RowKind::Membership => Self::Membership,
            RowKind::Folder => Self::Folder,
            RowKind::FolderShare => Self::FolderShare,
            RowKind::Snippet => Self::Snippet,
            RowKind::Tag => Self::Tag,
            RowKind::SnippetTag => Self::SnippetTag,
            RowKind::Attachment => Self::Attachment,
            RowKind::SnippetAttachment => Self::SnippetAttachment,
        }
    }
}

impl From<EntityKind> for RowKind {
    fn from(k: EntityKind) -> Self {
        match k {
            EntityKind::Profile => Self::Profile,
            EntityKind::Membership => Self::Membership,
            EntityKind::Folder => Self::Folder,
            EntityKind::FolderShare => Self::FolderShare,
            EntityKind::Snippet => Self::Snippet,
            EntityKind::Tag => Self::Tag,
            EntityKind::SnippetTag => Self::SnippetTag,
            EntityKind::Attachment => Self::Attachment,
            EntityKind::SnippetAttachment => Self::SnippetAttachment,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rows of any kind
// ---------------------------------------------------------------------------------------------

/// A row of any kind by kind and id, tombstones included.
pub(crate) fn get_any(conn: &Connection, key: RowRef) -> Result<Option<AnyRow>, StoreError> {
    let id = key.id;
    let all = ListOptions::ALL;
    Ok(match key.kind {
        RowKind::Profile => ops::get::<Profile>(conn, id, all)?.map(AnyRow::Profile),
        RowKind::Membership => ops::get::<Membership>(conn, id, all)?.map(AnyRow::Membership),
        RowKind::Folder => ops::get::<Folder>(conn, id, all)?.map(AnyRow::Folder),
        RowKind::FolderShare => ops::get::<FolderShare>(conn, id, all)?.map(AnyRow::FolderShare),
        RowKind::Snippet => ops::get::<Snippet>(conn, id, all)?.map(AnyRow::Snippet),
        RowKind::Tag => ops::get::<Tag>(conn, id, all)?.map(AnyRow::Tag),
        RowKind::SnippetTag => ops::get::<SnippetTag>(conn, id, all)?.map(AnyRow::SnippetTag),
        RowKind::Attachment => ops::get::<Attachment>(conn, id, all)?.map(AnyRow::Attachment),
        RowKind::SnippetAttachment => {
            ops::get::<SnippetAttachment>(conn, id, all)?.map(AnyRow::SnippetAttachment)
        }
    })
}

/// Writes a row from the server exactly as given.
fn put_any(conn: &Connection, row: &AnyRow) -> Result<(), StoreError> {
    match row {
        AnyRow::Profile(v) => ops::put_remote(conn, v),
        AnyRow::Membership(v) => ops::put_remote(conn, v),
        AnyRow::Folder(v) => ops::put_remote(conn, v),
        AnyRow::FolderShare(v) => ops::put_remote(conn, v),
        AnyRow::Snippet(v) => ops::put_remote(conn, v),
        AnyRow::Tag(v) => ops::put_remote(conn, v),
        AnyRow::SnippetTag(v) => ops::put_remote(conn, v),
        AnyRow::Attachment(v) => ops::put_remote(conn, v),
        AnyRow::SnippetAttachment(v) => ops::put_remote(conn, v),
    }
}

/// Every row of the store of one kind, tombstones included.
fn all_of(conn: &Connection, kind: RowKind) -> Result<Vec<AnyRow>, StoreError> {
    let all = ListOptions::ALL;
    Ok(match kind {
        RowKind::Profile => ops::list::<Profile>(conn, all)?
            .into_iter()
            .map(AnyRow::Profile)
            .collect(),
        RowKind::Membership => ops::list::<Membership>(conn, all)?
            .into_iter()
            .map(AnyRow::Membership)
            .collect(),
        RowKind::Folder => ops::list::<Folder>(conn, all)?
            .into_iter()
            .map(AnyRow::Folder)
            .collect(),
        RowKind::FolderShare => ops::list::<FolderShare>(conn, all)?
            .into_iter()
            .map(AnyRow::FolderShare)
            .collect(),
        RowKind::Snippet => ops::list::<Snippet>(conn, all)?
            .into_iter()
            .map(AnyRow::Snippet)
            .collect(),
        RowKind::Tag => ops::list::<Tag>(conn, all)?
            .into_iter()
            .map(AnyRow::Tag)
            .collect(),
        RowKind::SnippetTag => ops::list::<SnippetTag>(conn, all)?
            .into_iter()
            .map(AnyRow::SnippetTag)
            .collect(),
        RowKind::Attachment => ops::list::<Attachment>(conn, all)?
            .into_iter()
            .map(AnyRow::Attachment)
            .collect(),
        RowKind::SnippetAttachment => ops::list::<SnippetAttachment>(conn, all)?
            .into_iter()
            .map(AnyRow::SnippetAttachment)
            .collect(),
    })
}

fn exists(conn: &Connection, key: RowRef) -> Result<bool, StoreError> {
    let sql = format!(
        "SELECT EXISTS (SELECT 1 FROM {} WHERE id = ?1)",
        key.kind.as_str()
    );
    Ok(conn
        .prepare_cached(&sql)?
        .query_row([key.id.as_bytes()], |r| r.get(0))?)
}

fn i64_of(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn set_rev(conn: &Connection, key: RowRef, rev: u64) -> Result<(), StoreError> {
    let sql = format!("UPDATE {} SET rev = ?1 WHERE id = ?2", key.kind.as_str());
    conn.prepare_cached(&sql)?
        .execute(params![i64_of(rev), key.id.as_bytes()])?;
    Ok(())
}

fn set_deleted(
    conn: &Connection,
    key: RowRef,
    deleted_at: Option<Timestamp>,
) -> Result<(), StoreError> {
    let sql = format!(
        "UPDATE {} SET deleted_at = ?1 WHERE id = ?2",
        key.kind.as_str()
    );
    conn.prepare_cached(&sql)?
        .execute(params![deleted_at.map(|t| t.0), key.id.as_bytes()])?;
    Ok(())
}

fn put_origin(conn: &Connection, snippet: Id, origin: &Origin) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT INTO snippet_origin (snippet_id, content_by, content_at, created_rev)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (snippet_id) DO UPDATE SET content_by = excluded.content_by,
             content_at = excluded.content_at, created_rev = excluded.created_rev",
    )?
    .execute(params![
        snippet.as_bytes(),
        origin.content_by.map(|u| u.as_bytes().to_vec()),
        origin.content_at.map(|t| t.0),
        i64_of(origin.created_rev),
    ])?;
    Ok(())
}

fn get_origin(conn: &Connection, snippet: Id) -> Result<Option<Origin>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT content_by, content_at, created_rev FROM snippet_origin WHERE snippet_id = ?1",
        )?
        .query_row([snippet.as_bytes()], |r| {
            Ok(Origin {
                content_by: opt_id_at(r, 0)?,
                content_at: r.get::<_, Option<i64>>(1)?.map(Timestamp),
                created_rev: u64_at(r, 2)?,
            })
        })
        .optional()?)
}

/// Writes a row from the server, with its origin.
fn write_remote(
    conn: &Connection,
    row: &AnyRow,
    origin: Option<&Origin>,
) -> Result<(), StoreError> {
    put_any(conn, row)?;
    if let (AnyRow::Snippet(s), Some(o)) = (row, origin) {
        put_origin(conn, s.meta.id, o)?;
    }
    Ok(())
}

/// Whether the rows a remote row refers to are in the store (so that it can be written).
fn refs_present(conn: &Connection, row: &AnyRow) -> Result<bool, StoreError> {
    for r in row.references() {
        // A folder's profile and a share's group may be rows the user cannot read (a group the
        // user is not in): profiles are not required.
        if r.kind == RowKind::Profile {
            continue;
        }
        if !exists(conn, r)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Ensures the profile row a remote row names exists, inserting a placeholder when the server did
/// not send it (a group the user cannot read, which a share names).
fn ensure_profiles(conn: &Connection, row: &AnyRow) -> Result<(), StoreError> {
    for r in row.references() {
        if r.kind != RowKind::Profile || exists(conn, r)? {
            continue;
        }
        conn.prepare_cached(
            "INSERT INTO profile (id, created_at, updated_at, updated_by, rev, deleted_at, kind, name, owner_user_id)
             VALUES (?1, 0, 0, NULL, 0, NULL, 'family', '', ?1)",
        )?
        .execute([r.id.as_bytes()])?;
    }
    Ok(())
}

/// The profile a row belongs to: a snippet's is its folder's, a link's its snippet's, a share's
/// its folder's.
fn profile_of(conn: &Connection, row: &AnyRow) -> Result<Option<Id>, StoreError> {
    if let Some(p) = row.own_profile() {
        return Ok(Some(p));
    }
    let folder_profile = |f: Id| -> Result<Option<Id>, StoreError> {
        Ok(conn
            .prepare_cached("SELECT profile_id FROM folder WHERE id = ?1")?
            .query_row([f.as_bytes()], |r| id_at(r, 0))
            .optional()?)
    };
    let snippet_profile = |s: Id| -> Result<Option<Id>, StoreError> {
        Ok(conn
            .prepare_cached("SELECT profile_id FROM snippet WHERE id = ?1")?
            .query_row([s.as_bytes()], |r| id_at(r, 0))
            .optional()?)
    };
    match row {
        AnyRow::Snippet(s) => folder_profile(s.folder_id),
        AnyRow::FolderShare(s) => folder_profile(s.folder_id),
        AnyRow::SnippetTag(l) => snippet_profile(l.snippet_id),
        AnyRow::SnippetAttachment(l) => snippet_profile(l.snippet_id),
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------------------------
// Outbox entries
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Entry {
    seq: i64,
    key: RowRef,
    base_rev: u64,
    op_id: Id,
    handed_out: bool,
    cascade_of: Option<i64>,
    cascade_op: Option<Id>,
    blocked: Option<String>,
    repush: Option<RepushKind>,
    repush_rev: Option<u64>,
    repush_epoch: Option<String>,
    created_at: i64,
}

fn read_entries(conn: &Connection, key: Option<RowRef>) -> Result<Vec<Entry>, StoreError> {
    let mut sql = String::from(
        "SELECT seq, entity, row_id, base_rev, op_id, handed_out, cascade_of, cascade_op, blocked,
                repush, repush_rev, repush_epoch, created_at
         FROM outbox",
    );
    if key.is_some() {
        sql.push_str(" WHERE entity = ?1 AND row_id = ?2");
    }
    sql.push_str(" ORDER BY seq");
    let mut stmt = conn.prepare_cached(&sql)?;
    let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Entry> {
        let entity: String = r.get(1)?;
        let kind = RowKind::parse(&entity).unwrap_or(RowKind::Profile);
        let repush: Option<String> = r.get(9)?;
        Ok(Entry {
            seq: r.get(0)?,
            key: RowRef {
                kind,
                id: id_at(r, 2)?,
            },
            base_rev: u64_at(r, 3)?,
            op_id: opt_id_at(r, 4)?.unwrap_or(Id::from_bytes([0; 16])),
            handed_out: r.get(5)?,
            cascade_of: r.get(6)?,
            cascade_op: opt_id_at(r, 7)?,
            blocked: r.get(8)?,
            repush: match repush.as_deref() {
                Some("row") => Some(RepushKind::Row),
                Some("base") => Some(RepushKind::Base),
                _ => None,
            },
            repush_rev: r
                .get::<_, Option<i64>>(10)?
                .and_then(|v| u64::try_from(v).ok()),
            repush_epoch: r.get(11)?,
            created_at: r.get(12)?,
        })
    };
    let rows = match key {
        Some(k) => stmt
            .query_map(params![k.kind.as_str(), k.id.as_bytes()], map)?
            .collect::<Result<Vec<_>, _>>()?,
        None => stmt.query_map([], map)?.collect::<Result<Vec<_>, _>>()?,
    };
    Ok(rows)
}

fn has_entries(conn: &Connection, key: RowRef) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM outbox WHERE entity = ?1 AND row_id = ?2)")?
        .query_row(params![key.kind.as_str(), key.id.as_bytes()], |r| r.get(0))?)
}

fn delete_entries(conn: &Connection, seqs: &[i64]) -> Result<(), StoreError> {
    let mut stmt = conn.prepare_cached("DELETE FROM outbox WHERE seq = ?1")?;
    for s in seqs {
        stmt.execute([s])?;
    }
    Ok(())
}

fn delete_row_entries(conn: &Connection, key: RowRef) -> Result<(), StoreError> {
    conn.prepare_cached("DELETE FROM outbox WHERE entity = ?1 AND row_id = ?2")?
        .execute(params![key.kind.as_str(), key.id.as_bytes()])?;
    Ok(())
}

fn note_skipped(conn: &Connection, key: RowRef, rev: u64, gone: bool) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT INTO skipped (entity, row_id, rev, gone) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (entity, row_id) DO UPDATE SET
             rev = max(skipped.rev, excluded.rev), gone = excluded.gone",
    )?
    .execute(params![
        key.kind.as_str(),
        key.id.as_bytes(),
        i64_of(rev),
        gone
    ])?;
    Ok(())
}

fn clear_skipped(conn: &Connection, key: RowRef) -> Result<(), StoreError> {
    conn.prepare_cached("DELETE FROM skipped WHERE entity = ?1 AND row_id = ?2")?
        .execute(params![key.kind.as_str(), key.id.as_bytes()])?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Sync state
// ---------------------------------------------------------------------------------------------

fn ensure_state(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO sync_state (id, position, updated_at) VALUES (1, NULL, 0)
         ON CONFLICT (id) DO NOTHING",
        [],
    )?;
    Ok(())
}

fn account(conn: &Connection) -> Result<Option<Id>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT account_user_id FROM sync_state WHERE id = 1",
            [],
            |r| opt_id_at(r, 0),
        )
        .optional()?
        .flatten())
}

fn personal_profile(conn: &Connection) -> Result<Option<Id>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT personal_profile_id FROM sync_state WHERE id = 1",
            [],
            |r| opt_id_at(r, 0),
        )
        .optional()?
        .flatten())
}

fn set_position(
    conn: &Connection,
    position: Option<&str>,
    now: Timestamp,
) -> Result<(), StoreError> {
    ensure_state(conn)?;
    conn.prepare_cached("UPDATE sync_state SET position = ?1, updated_at = ?2 WHERE id = 1")?
        .execute(params![position, now.0])?;
    Ok(())
}

/// The groups in which the account holds a live membership.
fn member_groups(conn: &Connection) -> Result<BTreeSet<Id>, StoreError> {
    let Some(user) = account(conn)? else {
        return Ok(BTreeSet::new());
    };
    let mut stmt = conn.prepare_cached(
        "SELECT profile_id FROM membership WHERE user_id = ?1 AND deleted_at IS NULL",
    )?;
    let rows = stmt.query_map([user.as_bytes()], |r| id_at(r, 0))?;
    Ok(rows.collect::<Result<BTreeSet<_>, _>>()?)
}

/// The profiles the user owns or belongs to: their personal profile and their groups.
fn own_profiles(conn: &Connection) -> Result<BTreeSet<Id>, StoreError> {
    let mut out = member_groups(conn)?;
    if let Some(p) = personal_profile(conn)? {
        out.insert(p);
    }
    // Before the first sign-in the local personal profile is the user's.
    let mut stmt =
        conn.prepare_cached("SELECT id FROM profile WHERE kind = 'personal' AND rev = 0")?;
    for id in stmt.query_map([], |r| id_at(r, 0))? {
        out.insert(id?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Trees
// ---------------------------------------------------------------------------------------------

/// The folders of the tree under `root` (the root included), parents before children, without
/// the sub-trees whose tops are in `except`.
fn subtree(conn: &Connection, root: Id, except: &BTreeSet<Id>) -> Result<Vec<Id>, StoreError> {
    let mut out = Vec::new();
    let mut queue = vec![root];
    let mut seen = BTreeSet::new();
    let mut stmt = conn.prepare_cached("SELECT id FROM folder WHERE parent_id = ?1")?;
    while let Some(f) = queue.pop() {
        if !seen.insert(f) || (f != root && except.contains(&f)) {
            continue;
        }
        out.push(f);
        for c in stmt.query_map([f.as_bytes()], |r| id_at(r, 0))? {
            queue.push(c?);
        }
    }
    Ok(out)
}

fn ids_where(conn: &Connection, sql: &str, id: Id) -> Result<Vec<Id>, StoreError> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map([id.as_bytes()], |r| id_at(r, 0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// The tops of the sub-trees inside `root` (not `root` itself) that are shared by themselves with
/// a group the user is in (D3).
fn self_shared_inside(conn: &Connection, root: Id) -> Result<BTreeSet<Id>, StoreError> {
    let groups = member_groups(conn)?;
    let mut out = BTreeSet::new();
    for f in subtree(conn, root, &BTreeSet::new())? {
        if f == root {
            continue;
        }
        let shared_to = ids_where(
            conn,
            "SELECT profile_id FROM folder_share WHERE folder_id = ?1 AND deleted_at IS NULL",
            f,
        )?;
        if shared_to.iter().any(|g| groups.contains(g)) {
            out.insert(f);
        }
    }
    Ok(out)
}

/// The rows of a set of folders: the folders, their shares, their snippets and those snippets'
/// links.
fn rows_of_folders(conn: &Connection, folders: &[Id]) -> Result<Vec<RowRef>, StoreError> {
    let mut out = Vec::new();
    for &f in folders {
        for s in ids_where(conn, "SELECT id FROM snippet WHERE folder_id = ?1", f)? {
            for l in ids_where(conn, "SELECT id FROM snippet_tag WHERE snippet_id = ?1", s)? {
                out.push(RowRef {
                    kind: RowKind::SnippetTag,
                    id: l,
                });
            }
            for l in ids_where(
                conn,
                "SELECT id FROM snippet_attachment WHERE snippet_id = ?1",
                s,
            )? {
                out.push(RowRef {
                    kind: RowKind::SnippetAttachment,
                    id: l,
                });
            }
            out.push(RowRef {
                kind: RowKind::Snippet,
                id: s,
            });
        }
        for x in ids_where(conn, "SELECT id FROM folder_share WHERE folder_id = ?1", f)? {
            out.push(RowRef {
                kind: RowKind::FolderShare,
                id: x,
            });
        }
    }
    // Folders last, children before parents (the list is parents first).
    for &f in folders.iter().rev() {
        out.push(RowRef {
            kind: RowKind::Folder,
            id: f,
        });
    }
    Ok(out)
}

/// Hard-deletes one row (and, by `ON DELETE CASCADE`, its usage, approval, origin and links).
fn hard_delete(conn: &Connection, key: RowRef) -> Result<(), StoreError> {
    match key.kind {
        RowKind::Snippet => {
            conn.prepare_cached("DELETE FROM snippet_tag WHERE snippet_id = ?1")?
                .execute([key.id.as_bytes()])?;
            conn.prepare_cached("DELETE FROM snippet_attachment WHERE snippet_id = ?1")?
                .execute([key.id.as_bytes()])?;
        }
        RowKind::Folder => {
            conn.prepare_cached("DELETE FROM folder_share WHERE folder_id = ?1")?
                .execute([key.id.as_bytes()])?;
            // A sub-folder the purge leaves (shared by itself) keeps no parent it cannot have.
            conn.prepare_cached("UPDATE folder SET parent_id = NULL WHERE parent_id = ?1")?
                .execute([key.id.as_bytes()])?;
        }
        RowKind::Tag => {
            conn.prepare_cached("DELETE FROM snippet_tag WHERE tag_id = ?1")?
                .execute([key.id.as_bytes()])?;
        }
        RowKind::Attachment => {
            conn.prepare_cached("DELETE FROM snippet_attachment WHERE attachment_id = ?1")?
                .execute([key.id.as_bytes()])?;
        }
        RowKind::Profile => {
            conn.prepare_cached("DELETE FROM membership WHERE profile_id = ?1")?
                .execute([key.id.as_bytes()])?;
        }
        _ => {}
    }
    let sql = format!("DELETE FROM {} WHERE id = ?1", key.kind.as_str());
    conn.prepare_cached(&sql)?.execute([key.id.as_bytes()])?;
    clear_skipped(conn, key)?;
    conn.prepare_cached("DELETE FROM left_alone WHERE entity = ?1 AND row_id = ?2")?
        .execute(params![key.kind.as_str(), key.id.as_bytes()])?;
    Ok(())
}

/// The name of the folder a row is in, for the list of kept changes.
fn place_of(conn: &Connection, row: Option<&AnyRow>) -> Result<String, StoreError> {
    let folder = match row {
        Some(AnyRow::Folder(f)) => Some(f.meta.id),
        Some(AnyRow::Snippet(s)) => Some(s.folder_id),
        Some(AnyRow::FolderShare(s)) => Some(s.folder_id),
        _ => None,
    };
    let Some(f) = folder else {
        return Ok(String::new());
    };
    Ok(conn
        .prepare_cached("SELECT name FROM folder WHERE id = ?1")?
        .query_row([f.as_bytes()], |r| r.get(0))
        .optional()?
        .unwrap_or_default())
}

/// Whether a snippet is held for review on this device (D10): its content was written by someone
/// else and it differs from what was approved.
fn held(conn: &Connection, row: Option<&AnyRow>) -> Result<bool, StoreError> {
    let Some(AnyRow::Snippet(s)) = row else {
        return Ok(false);
    };
    let me = account(conn)?;
    let by = get_origin(conn, s.meta.id)?.and_then(|o| o.content_by);
    Ok(me.is_some() && by.is_some() && by != me && s.meta.rev > 0)
}

/// Records a change in the list of kept changes (D5).
fn keep(
    conn: &Connection,
    key: RowRef,
    content: Option<&AnyRow>,
    delete: bool,
    reason: &str,
    made_at: i64,
    now: Timestamp,
) -> Result<(), StoreError> {
    let place = place_of(conn, content)?;
    let is_held = held(conn, content)?;
    let json = content
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| StoreError::InvalidData(e.to_string()))?;
    conn.prepare_cached(
        "INSERT INTO kept_change (id, entity, row_id, kind, content, held, place, reason, made_at, kept_at)
         VALUES (randomblob(16), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?
    .execute(params![
        key.kind.as_str(),
        key.id.as_bytes(),
        if delete { "delete" } else { "put" },
        json,
        is_held,
        place,
        reason,
        made_at,
        now.0
    ])?;
    Ok(())
}

/// Moves the unsent changes of `rows` to the list of kept changes (or drops a re-push of a version
/// someone else wrote, D7) and removes their entries.
fn keep_unsent(
    conn: &Connection,
    rows: &[RowRef],
    reason: &str,
    discard: bool,
    now: Timestamp,
) -> Result<(), StoreError> {
    let me = account(conn)?;
    for &key in rows {
        let entries = read_entries(conn, Some(key))?;
        if entries.is_empty() {
            continue;
        }
        let row = get_any(conn, key)?;
        let only_repush = entries.iter().all(|e| e.repush.is_some());
        let own_version = row
            .as_ref()
            .is_some_and(|r| r.meta().updated_by == me && me.is_some());
        // A cascade entry of a folder that leaves with it is part of the folder's change.
        let only_cascade = entries.iter().all(|e| e.cascade_of.is_some());
        if !discard
            && (!only_repush || own_version)
            && (!only_cascade || key.kind == RowKind::Folder)
        {
            let deleted = row.as_ref().is_none_or(|r| r.meta().is_deleted());
            let made = entries.last().map_or(now.0, |e| e.created_at);
            keep(conn, key, row.as_ref(), deleted, reason, made, now)?;
        }
        delete_row_entries(conn, key)?;
    }
    Ok(())
}

/// Removes a tree from the device (D3, D6): unsent changes in it go to the kept list first; the
/// sub-trees in `except` stay. Then the clean-up of D16.
fn purge_tree(
    conn: &Connection,
    root: Id,
    except: &BTreeSet<Id>,
    reason: &str,
    discard: bool,
    now: Timestamp,
) -> Result<(), StoreError> {
    let folders = subtree(conn, root, except)?;
    let rows = rows_of_folders(conn, &folders)?;
    keep_unsent(conn, &rows, reason, discard, now)?;
    for key in rows {
        // A row a pull skipped for an unsent change may be readable elsewhere by now (moved out
        // of the tree on the server): its note stays, and the row is read with `rows` (D15).
        let note: Option<(i64, bool)> = conn
            .prepare_cached("SELECT rev, gone FROM skipped WHERE entity = ?1 AND row_id = ?2")?
            .query_row(params![key.kind.as_str(), key.id.as_bytes()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        hard_delete(conn, key)?;
        if let Some((rev, false)) = note {
            note_skipped(conn, key, u64::try_from(rev).unwrap_or(0), false)?;
        }
    }
    for e in except {
        // A part that stays readable becomes an access root of its own: shown at the top.
        if folders.iter().any(|f| {
            conn.prepare_cached("SELECT parent_id FROM folder WHERE id = ?1")
                .and_then(|mut s| s.query_row([e.as_bytes()], |r| opt_id_at(r, 0)))
                .ok()
                .flatten()
                == Some(*f)
        }) {
            conn.prepare_cached("UPDATE folder SET parent_id = NULL WHERE id = ?1")?
                .execute([e.as_bytes()])?;
        }
    }
    cleanup_unused(conn)?;
    Ok(())
}

/// D16: the tags, attachment rows and profile rows of profiles the user neither owns nor belongs
/// to that nothing the device still holds uses. A link with an unsent change keeps its tag or
/// attachment until the change is answered: the link's row refers to it, and the change must not
/// leave with it.
fn cleanup_unused(conn: &Connection) -> Result<(), StoreError> {
    let own = own_profiles(conn)?;
    let mut profiles: Vec<Id> = Vec::new();
    {
        let mut stmt = conn.prepare_cached("SELECT id FROM profile")?;
        for p in stmt.query_map([], |r| id_at(r, 0))? {
            let p = p?;
            if !own.contains(&p) {
                profiles.push(p);
            }
        }
    }
    for p in profiles {
        let tags = ids_where(
            conn,
            "SELECT t.id FROM tag t WHERE t.profile_id = ?1
               AND NOT EXISTS (SELECT 1 FROM snippet_tag l JOIN snippet s ON s.id = l.snippet_id
                               WHERE l.tag_id = t.id AND l.deleted_at IS NULL AND s.deleted_at IS NULL)
               AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.entity = 'tag' AND o.row_id = t.id)
               AND NOT EXISTS (SELECT 1 FROM snippet_tag l JOIN outbox o
                                   ON o.entity = 'snippet_tag' AND o.row_id = l.id
                               WHERE l.tag_id = t.id)",
            p,
        )?;
        for t in tags {
            hard_delete(
                conn,
                RowRef {
                    kind: RowKind::Tag,
                    id: t,
                },
            )?;
        }
        let atts = ids_where(
            conn,
            "SELECT a.id FROM attachment a WHERE a.profile_id = ?1
               AND NOT EXISTS (SELECT 1 FROM snippet_attachment l JOIN snippet s ON s.id = l.snippet_id
                               WHERE l.attachment_id = a.id AND l.deleted_at IS NULL AND s.deleted_at IS NULL)
               AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.entity = 'attachment' AND o.row_id = a.id)
               AND NOT EXISTS (SELECT 1 FROM snippet_attachment l JOIN outbox o
                                   ON o.entity = 'snippet_attachment' AND o.row_id = l.id
                               WHERE l.attachment_id = a.id)",
            p,
        )?;
        for a in atts {
            hard_delete(
                conn,
                RowRef {
                    kind: RowKind::Attachment,
                    id: a,
                },
            )?;
        }
        let used: bool = conn
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM folder WHERE profile_id = ?1)
                     OR EXISTS (SELECT 1 FROM snippet WHERE profile_id = ?1)
                     OR EXISTS (SELECT 1 FROM tag WHERE profile_id = ?1)
                     OR EXISTS (SELECT 1 FROM attachment WHERE profile_id = ?1)
                     OR EXISTS (SELECT 1 FROM folder_share WHERE profile_id = ?1)",
            )?
            .query_row([p.as_bytes()], |r| r.get(0))?;
        // The user's own membership of a group that ended stays as a tombstone, with the group's
        // row it refers to (D6).
        let mine: bool = match account(conn)? {
            Some(u) => conn
                .prepare_cached(
                    "SELECT EXISTS (SELECT 1 FROM membership WHERE profile_id = ?1 AND user_id = ?2)",
                )?
                .query_row(params![p.as_bytes(), u.as_bytes()], |r| r.get(0))?,
            None => false,
        };
        if !used && !mine {
            hard_delete(
                conn,
                RowRef {
                    kind: RowKind::Profile,
                    id: p,
                },
            )?;
        }
    }
    Ok(())
}

/// Removes a row the server no longer shows the user: a folder with its tree (D3), anything else
/// by itself.
/// The rows of a folder's subtree (the folder included) that the server once gave (`rev > 0`).
fn synced_in_tree(conn: &Connection, folder: Id) -> Result<Vec<RowRef>, StoreError> {
    let except = self_shared_inside(conn, folder)?;
    let folders = subtree(conn, folder, &except)?;
    let mut keys: Vec<RowRef> = folders
        .iter()
        .map(|f| RowRef {
            kind: RowKind::Folder,
            id: *f,
        })
        .collect();
    keys.extend(rows_of_folders(conn, &folders)?);
    let mut out = Vec::new();
    for k in keys {
        if get_any(conn, k)?.is_some_and(|r| r.meta().rev > 0) {
            out.push(k);
        }
    }
    Ok(out)
}

fn remove_row(conn: &Connection, key: RowRef, now: Timestamp) -> Result<(), StoreError> {
    if key.kind == RowKind::Folder {
        let except = self_shared_inside(conn, key.id)?;
        purge_tree(conn, key.id, &except, "gone", false, now)
    } else {
        hard_delete(conn, key)
    }
}

// ---------------------------------------------------------------------------------------------
// D7: what a device may write, and what a restore lost
// ---------------------------------------------------------------------------------------------

/// The access root entry above a folder, if any.
fn access_of_folder(conn: &Connection, folder: Id) -> Result<Option<String>, StoreError> {
    let mut cur = Some(folder);
    let mut steps = 0;
    while let Some(f) = cur {
        if let Some(p) = conn
            .prepare_cached("SELECT permission FROM folder_access WHERE folder_id = ?1")?
            .query_row([f.as_bytes()], |r| r.get::<_, String>(0))
            .optional()?
        {
            return Ok(Some(p));
        }
        cur = conn
            .prepare_cached("SELECT parent_id FROM folder WHERE id = ?1")?
            .query_row([f.as_bytes()], |r| opt_id_at(r, 0))
            .optional()?
            .flatten();
        steps += 1;
        if steps > 8 {
            break;
        }
    }
    Ok(None)
}

/// D7's "may write", read generously from what the device knew before the resync.
fn may_write(conn: &Connection, row: &AnyRow) -> Result<bool, StoreError> {
    let Some(profile) = profile_of(conn, row)? else {
        return Ok(false);
    };
    if personal_profile(conn)? == Some(profile) {
        return Ok(true);
    }
    let edit = |p: Option<String>| matches!(p.as_deref(), Some("edit" | "manage"));
    let folder = match row {
        AnyRow::Folder(f) => Some(f.meta.id),
        AnyRow::Snippet(s) => Some(s.folder_id),
        AnyRow::FolderShare(s) => Some(s.folder_id),
        AnyRow::SnippetTag(l) => conn
            .prepare_cached("SELECT folder_id FROM snippet WHERE id = ?1")?
            .query_row([l.snippet_id.as_bytes()], |r| id_at(r, 0))
            .optional()?,
        AnyRow::SnippetAttachment(l) => conn
            .prepare_cached("SELECT folder_id FROM snippet WHERE id = ?1")?
            .query_row([l.snippet_id.as_bytes()], |r| id_at(r, 0))
            .optional()?,
        AnyRow::Tag(_) | AnyRow::Attachment(_) => {
            let any: bool = conn
                .prepare_cached(
                    "SELECT EXISTS (SELECT 1 FROM folder_access WHERE profile_id = ?1
                                    AND permission IN ('edit', 'manage'))",
                )?
                .query_row([profile.as_bytes()], |r| r.get(0))?;
            return Ok(any);
        }
        AnyRow::Profile(_) | AnyRow::Membership(_) => return Ok(false),
    };
    match folder {
        Some(f) => Ok(edit(access_of_folder(conn, f)?)),
        None => Ok(false),
    }
}

/// Whether a row the device holds lies above its profile's lost line (D7).
fn above_line(conn: &Connection, row: &AnyRow) -> Result<bool, StoreError> {
    if row.meta().rev == 0 {
        return Ok(false);
    }
    let Some(profile) = profile_of(conn, row)? else {
        return Ok(false);
    };
    let line: Option<i64> = conn
        .prepare_cached("SELECT backup_rev FROM lost_line WHERE profile_id = ?1")?
        .query_row([profile.as_bytes()], |r| r.get(0))
        .optional()?;
    if let Some(line) = line {
        return Ok(i64_of(row.meta().rev) > line);
    }
    // A profile the snapshot does not name: rows written in the retention window before the new
    // epoch began count as lost.
    let started: Option<i64> = conn
        .query_row(
            "SELECT lost_started_at FROM sync_state WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(started.is_some_and(|s| row.meta().updated_at.0 >= s - RETENTION_MS))
}

/// Queues a row again to be re-pushed after a restore (D7).
fn requeue(conn: &Connection, row: &AnyRow, now: Timestamp) -> Result<(), StoreError> {
    let key = row.key();
    let epoch: Option<String> = conn
        .query_row(
            "SELECT lost_device_epoch FROM sync_state WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let entries = read_entries(conn, Some(key))?;
    if entries.is_empty() {
        let op = if row.meta().is_deleted() {
            crate::OutboxOp::Delete
        } else {
            crate::OutboxOp::Put
        };
        let seq = ops::outbox_push(conn, key.kind.into(), key.id, op, row.meta().rev, now)?;
        conn.prepare_cached(
            "UPDATE outbox SET repush = 'row', repush_rev = ?1, repush_epoch = ?2 WHERE seq = ?3",
        )?
        .execute(params![i64_of(row.meta().rev), epoch, seq])?;
    } else {
        // A change made on top of a lost version: its base is the lost revision.
        for e in entries {
            if e.repush.is_none() {
                conn.prepare_cached(
                    "UPDATE outbox SET repush = 'base', repush_rev = base_rev, repush_epoch = ?1
                     WHERE seq = ?2 AND base_rev > 0",
                )?
                .execute(params![epoch, e.seq])?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The store's sync API
// ---------------------------------------------------------------------------------------------

impl Store {
    fn sync_tx(&mut self) -> Result<Transaction<'_>, StoreError> {
        Ok(self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?)
    }

    /// The sync state (§7.2 `SyncState`).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn sync_info(&self) -> Result<SyncInfo, StoreError> {
        let c = &self.conn;
        let mut info = c
            .query_row(
                "SELECT account_user_id, device_id, personal_profile_id, position, adoption, set_aside_at,
                        signed_out_at
                 FROM sync_state WHERE id = 1",
                [],
                |r| {
                    Ok(SyncInfo {
                        account: opt_id_at(r, 0)?,
                        device: opt_id_at(r, 1)?,
                        personal_profile: opt_id_at(r, 2)?,
                        position: r.get::<_, Option<String>>(3)?.filter(|p| !p.is_empty()),
                        adoption: match r.get::<_, Option<String>>(4)?.as_deref() {
                            Some("adopted") => Some(Adoption::Adopted),
                            Some("merging") => Some(Adoption::Merging),
                            _ => None,
                        },
                        set_aside_at: r.get::<_, Option<i64>>(5)?.map(Timestamp),
                        signed_out_at: r.get::<_, Option<i64>>(6)?.map(Timestamp),
                        ..SyncInfo::default()
                    })
                },
            )
            .optional()?
            .unwrap_or_default();
        info.waiting = c.query_row(
            "SELECT count(DISTINCT entity || hex(row_id)) FROM outbox WHERE blocked IS NULL",
            [],
            |r| u64_at(r, 0),
        )?;
        info.blocked = c.query_row(
            "SELECT count(DISTINCT entity || hex(row_id)) FROM outbox WHERE blocked IS NOT NULL",
            [],
            |r| u64_at(r, 0),
        )?;
        info.kept = c.query_row("SELECT count(*) FROM kept_change", [], |r| u64_at(r, 0))?;
        info.max_rev = self.max_rev()?;
        Ok(info)
    }

    /// Records the account a library that already belongs to it is synced with, and the device id
    /// (a store that was never adopted uses [`adopt`](Self::adopt) instead).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn set_account(
        &mut self,
        user: Id,
        personal_profile: Id,
        device: Id,
    ) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        ensure_state(&tx)?;
        tx.execute(
            "UPDATE sync_state SET account_user_id = ?1, personal_profile_id = ?2, device_id = ?3 WHERE id = 1",
            params![user.as_bytes(), personal_profile.as_bytes(), device.as_bytes()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Replaces the device id (after `device_conflict`, C25).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn set_device_id(&mut self, device: Id) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        ensure_state(&tx)?;
        tx.execute(
            "UPDATE sync_state SET device_id = ?1 WHERE id = 1",
            [device.as_bytes()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Stores the position in the feed (after the effects of a page, D11).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn set_sync_position(
        &mut self,
        position: Option<&str>,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        set_position(&tx, position, now)?;
        tx.commit()?;
        Ok(())
    }

    /// Changes ready to send, at most `limit` rows and the rows they refer to that the server has
    /// never seen, built under D1; marks every entry they cover as handed out in the same
    /// transaction.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    #[allow(clippy::too_many_lines)]
    pub fn sync_outbox_read(
        &mut self,
        limit: usize,
        phase: OutboxPhase,
    ) -> Result<OutboxBatch, StoreError> {
        let tx = self.sync_tx()?;
        let me = account(&tx)?;
        let entries = read_entries(&tx, None)?;
        // Group by row, in the order of each row's oldest entry.
        let mut order: Vec<RowRef> = Vec::new();
        let mut groups: HashMap<RowRef, Vec<Entry>> = HashMap::new();
        for e in entries {
            let g = groups.entry(e.key).or_default();
            if g.is_empty() {
                order.push(e.key);
            }
            g.push(e);
        }
        let blocked: BTreeSet<RowRef> = groups
            .iter()
            .filter(|(_, es)| es.iter().any(|e| e.blocked.is_some()))
            .map(|(k, _)| *k)
            .collect();
        // Pass 1: drop what is never sent (C16 rows; rows created and deleted before any entry was
        // handed out, place 7; entries of rows that no longer exist).
        let mut rows: HashMap<RowRef, AnyRow> = HashMap::new();
        let mut dropped: BTreeSet<RowRef> = BTreeSet::new();
        let mut garbage: Vec<RowRef> = Vec::new();
        for key in &order {
            let es = &groups[key];
            let row = get_any(&tx, *key)?;
            let never_out = es.iter().all(|e| !e.handed_out && e.repush.is_none());
            let drop = key.kind.server_only()
                || row.is_none()
                || row
                    .as_ref()
                    .is_some_and(|r| r.meta().rev == 0 && r.meta().is_deleted() && never_out);
            if drop {
                delete_row_entries(&tx, *key)?;
                dropped.insert(*key);
                if row
                    .as_ref()
                    .is_some_and(|r| r.meta().rev == 0 && r.meta().is_deleted())
                {
                    garbage.push(*key);
                }
            } else if let Some(r) = row {
                rows.insert(*key, r);
            }
        }
        // A row created and deleted before any entry was handed out leaves nothing behind (D1,
        // place 7 of §1): the tombstone goes, leaves first.
        drop_garbage(&tx, &garbage)?;
        // The folder entries' seqs, to find cascade children.
        let mut folder_of_seq: HashMap<i64, RowRef> = HashMap::new();
        for key in &order {
            if key.kind == RowKind::Folder && !dropped.contains(key) {
                for e in &groups[key] {
                    folder_of_seq.insert(e.seq, *key);
                }
            }
        }
        let deleted = |k: &RowRef| rows.get(k).is_some_and(|r| r.meta().is_deleted());
        // A row whose newest entry is a cascade entry of a folder that is sent as a delete goes in
        // the folder's cascade list.
        let mut cascade_children: HashMap<RowRef, Vec<RowRef>> = HashMap::new();
        let mut in_cascade: BTreeSet<RowRef> = BTreeSet::new();
        for key in &order {
            if dropped.contains(key) {
                continue;
            }
            let newest = groups[key].last().map(|e| e.cascade_of);
            if let Some(Some(fs)) = newest
                && let Some(folder) = folder_of_seq.get(&fs)
                && deleted(folder)
                && deleted(key)
                && !blocked.contains(folder)
            {
                cascade_children.entry(*folder).or_default().push(*key);
                in_cascade.insert(*key);
            }
        }
        let wanted = |k: &RowRef| -> bool {
            match phase {
                OutboxPhase::All => true,
                OutboxPhase::ShareDeletes => k.kind == RowKind::FolderShare && deleted(k),
                OutboxPhase::Folders => k.kind == RowKind::Folder,
                OutboxPhase::NotShares => k.kind != RowKind::FolderShare || deleted(k),
            }
        };
        let build = |key: RowRef| -> Result<Change, StoreError> {
            let es = &groups[&key];
            let newest = &es[es.len() - 1];
            let mut follows: Vec<Id> = Vec::new();
            for e in &es[..es.len() - 1] {
                if e.handed_out {
                    let id = e.cascade_op.unwrap_or(e.op_id);
                    if !follows.contains(&id) {
                        follows.push(id);
                    }
                }
            }
            if newest.handed_out
                && let Some(c) = newest.cascade_op
                && !follows.contains(&c)
            {
                follows.push(c);
            }
            let row = rows[&key].clone();
            let mut seqs: Vec<i64> = es.iter().map(|e| e.seq).collect();
            let mut cascade = Vec::new();
            if key.kind == RowKind::Folder && row.meta().is_deleted() {
                for child in cascade_children.get(&key).into_iter().flatten() {
                    let ces = &groups[child];
                    cascade.push(CascadeItem {
                        key: *child,
                        base_rev: ces[0].base_rev,
                    });
                    seqs.extend(ces.iter().map(|e| e.seq));
                }
            }
            let repush = es.iter().find(|e| e.repush.is_some()).map(|e| Repush {
                kind: e.repush.unwrap_or(RepushKind::Row),
                rev: e.repush_rev.unwrap_or(e.base_rev),
                epoch: e.repush_epoch.clone().unwrap_or_default(),
            });
            let author_self = repush.is_some()
                && me.is_some()
                && match &row {
                    AnyRow::Snippet(s) => {
                        get_origin(&tx, s.meta.id)?.and_then(|o| o.content_by) == me
                    }
                    AnyRow::Attachment(a) => a.meta.updated_by == me,
                    _ => false,
                };
            let base_rev = es[0].base_rev;
            let live = !row.meta().is_deleted();
            Ok(Change {
                op_id: newest.op_id,
                follows,
                kind: key.kind,
                id: key.id,
                row: live.then_some(row),
                base_rev,
                cascade,
                repush,
                author_self,
                seqs,
            })
        };
        let mut chosen: Vec<RowRef> = Vec::new();
        let mut chosen_set: BTreeSet<RowRef> = BTreeSet::new();
        for key in &order {
            if chosen.len() >= limit {
                break;
            }
            if dropped.contains(key)
                || in_cascade.contains(key)
                || blocked.contains(key)
                || !wanted(key)
            {
                continue;
            }
            chosen.push(*key);
            chosen_set.insert(*key);
        }
        // The rows those refer to that the server has never seen, so that the request is complete
        // in itself; a row that refers to a blocked row is blocked with it (D1).
        let mut newly_blocked: BTreeSet<RowRef> = BTreeSet::new();
        let mut i = 0;
        while i < chosen.len() && chosen.len() < limit + CLOSURE_MAX {
            let key = chosen[i];
            i += 1;
            let Some(row) = rows.get(&key) else { continue };
            for r in row.references() {
                if blocked.contains(&r) || newly_blocked.contains(&r) {
                    newly_blocked.insert(key);
                    continue;
                }
                if chosen_set.contains(&r) || !groups.contains_key(&r) || dropped.contains(&r) {
                    continue;
                }
                if rows.get(&r).is_some_and(|x| x.meta().rev == 0) && !in_cascade.contains(&r) {
                    chosen.push(r);
                    chosen_set.insert(r);
                }
            }
        }
        let mut out = Vec::new();
        for key in chosen {
            if newly_blocked.contains(&key) {
                tx.prepare_cached(
                    "UPDATE outbox SET blocked = 'parent_blocked' WHERE entity = ?1 AND row_id = ?2",
                )?
                .execute(params![key.kind.as_str(), key.id.as_bytes()])?;
                continue;
            }
            out.push(build(key)?);
        }
        {
            let mut mark = tx.prepare_cached("UPDATE outbox SET handed_out = 1 WHERE seq = ?1")?;
            for c in &out {
                for s in &c.seqs {
                    mark.execute([s])?;
                }
            }
        }
        // Dependency order (§6.3): puts parents first, then deletes children first.
        let depth = |c: &Change| -> usize {
            match &c.row {
                Some(AnyRow::Folder(f)) => {
                    let mut d = 0usize;
                    let mut cur = f.parent_id;
                    while let Some(p) = cur {
                        d += 1;
                        if d > 8 {
                            break;
                        }
                        cur = out
                            .iter()
                            .find_map(|o| match &o.row {
                                Some(AnyRow::Folder(x)) if x.meta.id == p => Some(x.parent_id),
                                _ => None,
                            })
                            .flatten();
                    }
                    d
                }
                _ => 0,
            }
        };
        let rank = |c: &Change| -> (u8, usize, usize) {
            let k = RowKind::ALL.iter().position(|x| *x == c.kind).unwrap_or(9);
            if c.is_delete() {
                (1, 9 - k, 0)
            } else {
                (0, k, depth(c))
            }
        };
        let mut keyed: Vec<((u8, usize, usize), Change)> =
            out.iter().map(|c| (rank(c), c.clone())).collect();
        keyed.sort_by_key(|(r, _)| *r);
        // D15: a row without entries that is behind what a pull skipped (one dropped here, one
        // whose read failed before) is read with `rows`.
        let mut behind = Vec::new();
        let notes: Vec<(String, Id, i64, bool)> = {
            let mut stmt = tx.prepare_cached("SELECT entity, row_id, rev, gone FROM skipped")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get(0)?, id_at(r, 1)?, r.get(2)?, r.get(3)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        for (entity, id, rev, gone) in notes {
            let Some(kind) = RowKind::parse(&entity) else {
                continue;
            };
            let key = RowRef { kind, id };
            if has_entries(&tx, key)? {
                continue;
            }
            let local = get_any(&tx, key)?;
            // A row the device no longer holds (dropped here) that a pull brought is behind too.
            let is_behind = match &local {
                Some(l) => gone || i64_of(l.meta().rev) < rev,
                None => !gone,
            };
            if is_behind {
                behind.push(key);
            } else {
                clear_skipped(&tx, key)?;
            }
        }
        let _ = &dropped;
        tx.commit()?;
        let changes = keyed.into_iter().map(|(_, c)| c).collect();
        Ok(OutboxBatch { changes, behind })
    }

    /// Acts on the server's result for each change (D4, D5) in one transaction; answers the rows
    /// that are now behind what the device skipped (D15) and the left-alone rows it removed (D7).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    #[allow(clippy::too_many_lines)]
    pub fn sync_outbox_ack(
        &mut self,
        results: &[OpResult],
        now: Timestamp,
    ) -> Result<AckOutcome, StoreError> {
        let tx = self.sync_tx()?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let me = account(&tx)?;
        let mut settled: BTreeSet<RowRef> = BTreeSet::new();
        let mut behind: BTreeSet<RowRef> = BTreeSet::new();
        for res in results {
            let covered: BTreeSet<i64> = res.seqs.iter().copied().collect();
            let key = res.key;
            let entries = read_entries(&tx, Some(key))?;
            let mine: Vec<i64> = entries
                .iter()
                .filter(|e| covered.contains(&e.seq))
                .map(|e| e.seq)
                .collect();
            let newer: Vec<&Entry> = entries
                .iter()
                .filter(|e| !covered.contains(&e.seq))
                .collect();
            match res.status {
                OpStatus::Partial => {}
                OpStatus::Blocked => {
                    let reason = res.reason.clone().unwrap_or_else(|| "invalid".into());
                    let mut stmt =
                        tx.prepare_cached("UPDATE outbox SET blocked = ?1 WHERE seq = ?2")?;
                    for e in &entries {
                        stmt.execute(params![reason, e.seq])?;
                    }
                }
                OpStatus::Applied | OpStatus::Unchanged | OpStatus::Superseded => {
                    delete_entries(&tx, &mine)?;
                    if newer.is_empty() {
                        match &res.row {
                            Some(r) => {
                                if refs_present(&tx, r)? {
                                    ensure_profiles(&tx, r)?;
                                    write_remote(&tx, r, res.origin.as_ref())?;
                                }
                            }
                            None => {
                                if get_any(&tx, key)?.is_some() {
                                    remove_row(&tx, key, now)?;
                                }
                            }
                        }
                        settled.insert(key);
                    } else if let Some(r) = &res.row {
                        // The user changed the row again while the push was under way: keep it,
                        // take only the revision, and base the remaining entries on it (D4).
                        let rev = r.meta().rev;
                        set_rev(&tx, key, rev)?;
                        let mut stmt =
                            tx.prepare_cached("UPDATE outbox SET base_rev = ?1 WHERE seq = ?2")?;
                        for e in &newer {
                            stmt.execute(params![i64_of(rev), e.seq])?;
                        }
                        if let (AnyRow::Snippet(_), Some(o)) = (r, &res.origin)
                            && let Some(mut cur) = get_origin(&tx, key.id)?.or(Some(*o))
                        {
                            cur.created_rev = o.created_rev;
                            put_origin(&tx, key.id, &cur)?;
                        }
                    }
                    for c in &res.cascade {
                        if let Some(k) = self::apply_cascade(&tx, c, &covered, now, &mut behind)? {
                            settled.insert(k);
                        }
                    }
                }
                OpStatus::Rejected => {
                    let share = key.kind == RowKind::FolderShare;
                    let current = get_any(&tx, key)?;
                    let own_version = current
                        .as_ref()
                        .is_some_and(|r| me.is_some() && r.meta().updated_by == me);
                    if !share && (!res.repush || own_version) {
                        let deleted = current.as_ref().is_none_or(|r| r.meta().is_deleted());
                        let made = entries.last().map_or(now.0, |e| e.created_at);
                        let reason = res.reason.clone().unwrap_or_else(|| "forbidden".into());
                        keep(&tx, key, current.as_ref(), deleted, &reason, made, now)?;
                    }
                    delete_row_entries(&tx, key)?;
                    match &res.row {
                        Some(r) => {
                            if refs_present(&tx, r)? {
                                ensure_profiles(&tx, r)?;
                                write_remote(&tx, r, res.origin.as_ref())?;
                            } else {
                                remove_row(&tx, key, now)?;
                            }
                        }
                        None => {
                            if current.is_some() {
                                // The device's tree under a folder need not be the server's: a
                                // folder the server moved out may still hang under it here,
                                // held by an unsent change, with rows a pull just brought. What
                                // the purge takes that the server once gave is read again with
                                // `rows` (D15); what the server no longer has stays away.
                                if key.kind == RowKind::Folder {
                                    for k in synced_in_tree(&tx, key.id)? {
                                        if k != key {
                                            behind.insert(k);
                                        }
                                    }
                                }
                                remove_row(&tx, key, now)?;
                            }
                        }
                    }
                    settled.insert(key);
                    for c in &res.cascade {
                        if let Some(k) = self::apply_cascade(&tx, c, &covered, now, &mut behind)? {
                            settled.insert(k);
                        }
                    }
                }
            }
        }
        // D15: a row whose last entry is gone and that is behind what the pull skipped.
        for key in settled {
            if has_entries(&tx, key)? {
                continue;
            }
            let note: Option<(i64, bool)> = tx
                .prepare_cached("SELECT rev, gone FROM skipped WHERE entity = ?1 AND row_id = ?2")?
                .query_row(params![key.kind.as_str(), key.id.as_bytes()], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            if let Some((rev, gone)) = note {
                let local = get_any(&tx, key)?;
                let is_behind = match &local {
                    Some(l) => gone || i64_of(l.meta().rev) < rev,
                    None => false,
                };
                if is_behind {
                    behind.insert(key);
                } else {
                    clear_skipped(&tx, key)?;
                }
            }
        }
        // A left-alone row that the server's answer brought is the server's again (D7).
        {
            let mut stmt =
                tx.prepare_cached("DELETE FROM left_alone WHERE entity = ?1 AND row_id = ?2")?;
            for res in results {
                if res.row.is_some() {
                    stmt.execute(params![res.key.kind.as_str(), res.key.id.as_bytes()])?;
                }
                for c in &res.cascade {
                    if !c.gone {
                        stmt.execute(params![c.key.kind.as_str(), c.key.id.as_bytes()])?;
                    }
                }
            }
        }
        let removed = remove_left_alone(&tx, now)?;
        cleanup_unused(&tx)?;
        tx.commit()?;
        Ok(AckOutcome {
            behind: behind.into_iter().collect(),
            removed,
        })
    }

    /// Applies rows from the server (D2, D3; D7 in the modes `reconcile` and `epoch`); stores
    /// `position` in the same transaction when given; answers per row. Runs the clean-up of D16 at
    /// the end.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    #[allow(clippy::too_many_lines)]
    pub fn sync_apply(
        &mut self,
        rows: &[RemoteRow],
        position: Option<&str>,
        mode: ApplyMode,
        now: Timestamp,
    ) -> Result<Vec<ApplyStatus>, StoreError> {
        let tx = self.sync_tx()?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let me = account(&tx)?;
        let reconciling: bool = tx
            .query_row(
                "SELECT reconcile_token IS NOT NULL FROM sync_state WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        let mut status = vec![ApplyStatus::Stale; rows.len()];
        // Present rows in dependency order, folders parents first; then the null rows.
        let mut order: Vec<usize> = (0..rows.len()).collect();
        let folder_depth = |r: &RemoteRow| -> usize {
            let mut d = 0usize;
            if let Some(AnyRow::Folder(f)) = &r.row {
                let mut cur = f.parent_id;
                while let Some(p) = cur {
                    d += 1;
                    if d > 8 {
                        break;
                    }
                    cur = rows
                        .iter()
                        .find_map(|x| match &x.row {
                            Some(AnyRow::Folder(g)) if g.meta.id == p => Some(g.parent_id),
                            _ => None,
                        })
                        .flatten();
                }
            }
            d
        };
        order.sort_by_key(|&i| {
            let r = &rows[i];
            let k = RowKind::ALL
                .iter()
                .position(|x| *x == r.key.kind)
                .unwrap_or(9);
            (r.row.is_none(), k, folder_depth(r))
        });
        let mut seen = tx.prepare_cached(
            "INSERT INTO reconcile_seen (entity, row_id) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
        )?;
        for i in order {
            let rr = &rows[i];
            let key = rr.key;
            if reconciling && rr.row.is_some() {
                seen.execute(params![key.kind.as_str(), key.id.as_bytes()])?;
            }
            let local = get_any(&tx, key)?;
            let pending = has_entries(&tx, key)?;
            let Some(row) = &rr.row else {
                // A null row: the row is no longer there for this user.
                status[i] = match &local {
                    None => ApplyStatus::Stale,
                    Some(_) if key.kind == RowKind::Folder => {
                        let except = self_shared_inside(&tx, key.id)?;
                        purge_tree(&tx, key.id, &except, "revoked", false, now)?;
                        ApplyStatus::Removed
                    }
                    Some(_) if pending => {
                        note_skipped(&tx, key, 0, true)?;
                        ApplyStatus::Shadowed
                    }
                    // A profile goes once nothing the device holds refers to it (the clean-up
                    // of D16 at the end of the batch): its folders and shares leave through
                    // their own rows and `folder_revoked`, in whatever order they arrive.
                    Some(_) if key.kind == RowKind::Profile => ApplyStatus::Removed,
                    Some(_) => {
                        hard_delete(&tx, key)?;
                        ApplyStatus::Removed
                    }
                };
                continue;
            };
            let remote_rev = row.meta().rev;
            if mode == ApplyMode::Epoch
                && let Some(l) = &local
                && above_line(&tx, l)?
                && (may_write(&tx, l)? || (me.is_some() && l.meta().updated_by == me))
            {
                requeue(&tx, l, now)?;
                note_skipped(&tx, key, remote_rev, false)?;
                status[i] = ApplyStatus::Requeued;
                continue;
            }
            if pending {
                if mode == ApplyMode::Epoch
                    && let Some(l) = &local
                    && above_line(&tx, l)?
                {
                    requeue(&tx, l, now)?;
                }
                note_skipped(&tx, key, remote_rev, false)?;
                status[i] = ApplyStatus::Shadowed;
                continue;
            }
            if let Some(l) = &local {
                let lr = l.meta().rev;
                let stale = match mode {
                    ApplyMode::Feed => lr >= remote_rev,
                    ApplyMode::Reconcile => lr > remote_rev,
                    ApplyMode::Epoch => false,
                };
                if stale {
                    // The origin may still be news (a whole read after an older build).
                    if let (AnyRow::Snippet(_), Some(o)) = (row, &rr.origin)
                        && lr == remote_rev
                    {
                        put_origin(&tx, key.id, o)?;
                    }
                    continue;
                }
            }
            if !refs_present(&tx, row)? {
                status[i] = ApplyStatus::Refused;
                continue;
            }
            ensure_profiles(&tx, row)?;
            write_remote(&tx, row, rr.origin.as_ref())?;
            clear_skipped(&tx, key)?;
            status[i] = ApplyStatus::Applied;
        }
        drop(seen);
        // The user's own membership ended: the group's profile row and its other memberships go
        // (D6); its folders go through `folder_revoked`.
        if let Some(user) = me {
            let ended = ids_where(
                &tx,
                "SELECT profile_id FROM membership WHERE user_id = ?1 AND deleted_at IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM membership m2 WHERE m2.profile_id = membership.profile_id
                                   AND m2.user_id = ?1 AND m2.deleted_at IS NULL)",
                user,
            )?;
            for g in ended {
                tx.prepare_cached(
                    "DELETE FROM membership WHERE profile_id = ?1 AND user_id <> ?2",
                )?
                .execute(params![g.as_bytes(), user.as_bytes()])?;
            }
        }
        cleanup_unused(&tx)?;
        if let Some(p) = position {
            set_position(&tx, Some(p), now)?;
        }
        tx.commit()?;
        Ok(status)
    }

    /// Removes a tree whose access ended (D6): unsent changes in it go to the kept list first,
    /// then the tree, its approvals, usage and origins, and what nothing left uses; the sub-trees
    /// in `except` stay.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn purge_folder(
        &mut self,
        id: Id,
        except: &[Id],
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        if exists(
            &tx,
            RowRef {
                kind: RowKind::Folder,
                id,
            },
        )? {
            let except: BTreeSet<Id> = except.iter().copied().collect();
            purge_tree(&tx, id, &except, "revoked", false, now)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Begins a reconcile (D7): rows applied until [`reconcile_end`](Self::reconcile_end) are
    /// noted. With `lost` (after `reset_required: epoch`), the lines of the restore are kept for
    /// [`ApplyMode::Epoch`].
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn reconcile_begin(
        &mut self,
        scope: ReconcileScope,
        lost: Option<&LostLine>,
    ) -> Result<String, StoreError> {
        let tx = self.sync_tx()?;
        ensure_state(&tx)?;
        tx.execute("DELETE FROM reconcile_seen", [])?;
        tx.execute("DELETE FROM lost_line", [])?;
        let token: String = tx.query_row("SELECT lower(hex(randomblob(8)))", [], |r| r.get(0))?;
        let scope_blob = match scope {
            ReconcileScope::All => None,
            ReconcileScope::Folder(f) => Some(f.as_bytes().to_vec()),
        };
        tx.execute(
            "UPDATE sync_state SET reconcile_token = ?1, reconcile_scope = ?2, reconcile_epoch = ?3,
                 lost_epoch = ?4, lost_started_at = ?5, lost_device_epoch = ?6 WHERE id = 1",
            params![
                token,
                scope_blob,
                lost.is_some(),
                lost.map(|l| l.epoch.clone()),
                lost.map(|l| l.started_at.0),
                lost.map(|l| l.device_epoch.clone()),
            ],
        )?;
        if let Some(l) = lost {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO lost_line (profile_id, backup_rev) VALUES (?1, ?2)
                 ON CONFLICT (profile_id) DO UPDATE SET backup_rev = excluded.backup_rev",
            )?;
            for (p, rev) in &l.profiles {
                stmt.execute(params![p.as_bytes(), i64_of(*rev)])?;
            }
        }
        tx.commit()?;
        Ok(token)
    }

    /// Ends a reconcile (D7): synced rows of the scope that were not seen and have no unsent
    /// change are removed; those with one, and the folders above them, are left alone. In a new
    /// epoch a row held above its line that the device may write is queued again instead, and the
    /// answer is the list of access removals the restore lost. A resync of everything ends what a
    /// sign-out had set aside (D14).
    ///
    /// # Errors
    ///
    /// [`StoreError::InvalidData`] for an unknown token; `SQLite` errors.
    #[allow(clippy::too_many_lines)]
    pub fn reconcile_end(
        &mut self,
        token: &str,
        position: Option<&str>,
        now: Timestamp,
    ) -> Result<Vec<AccessRemoval>, StoreError> {
        let tx = self.sync_tx()?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let (cur, scope, epoch): (Option<String>, Option<Vec<u8>>, bool) = tx.query_row(
            "SELECT reconcile_token, reconcile_scope, reconcile_epoch FROM sync_state WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if cur.as_deref() != Some(token) {
            return Err(StoreError::InvalidData("unknown reconcile token".into()));
        }
        let me = account(&tx)?;
        let seen: BTreeSet<(String, [u8; 16])> = {
            let mut stmt = tx.prepare_cached("SELECT entity, row_id FROM reconcile_seen")?;
            let rows = stmt.query_map([], |r| {
                let e: String = r.get(0)?;
                let id = id_at(r, 1)?;
                Ok((e, *id.as_bytes()))
            })?;
            rows.collect::<Result<_, _>>()?
        };
        let was_seen = |k: RowRef| seen.contains(&(k.kind.as_str().to_owned(), *k.id.as_bytes()));
        // The rows of the scope.
        let mut scope_rows: Vec<AnyRow> = Vec::new();
        match &scope {
            None => {
                for kind in [
                    RowKind::SnippetTag,
                    RowKind::SnippetAttachment,
                    RowKind::Snippet,
                    RowKind::FolderShare,
                    RowKind::Folder,
                    RowKind::Tag,
                    RowKind::Attachment,
                    RowKind::Membership,
                    RowKind::Profile,
                ] {
                    scope_rows.extend(all_of(&tx, kind)?);
                }
            }
            Some(bytes) => {
                let root = Id::from_bytes(bytes.clone().try_into().unwrap_or([0; 16]));
                let folders = subtree(&tx, root, &BTreeSet::new())?;
                for key in rows_of_folders(&tx, &folders)? {
                    if let Some(r) = get_any(&tx, key)? {
                        scope_rows.push(r);
                    }
                }
            }
        }
        // Folders kept because something below them waits to be sent.
        let mut keep_folders: BTreeSet<Id> = BTreeSet::new();
        let mut left: Vec<RowRef> = Vec::new();
        let mut remove: Vec<RowRef> = Vec::new();
        let folder_scope = scope.is_some();
        for r in &scope_rows {
            let key = r.key();
            let synced = r.meta().rev > 0;
            // A tombstone the server never saw, with no entry left, is nothing to keep (D8).
            let local_garbage = !synced && r.meta().is_deleted() && !has_entries(&tx, key)?;
            if local_garbage && !folder_scope {
                remove.push(key);
                continue;
            }
            if !synced || was_seen(key) {
                continue;
            }
            // A whole read of a tree (`fetch_folder`) holds its live rows only: tombstones, and
            // links whose snippet is a tombstone, are not judged by it.
            if folder_scope && (r.meta().is_deleted() || !link_ends_live(&tx, r)?) {
                continue;
            }
            if epoch
                && above_line(&tx, r)?
                && (may_write(&tx, r)? || (me.is_some() && r.meta().updated_by == me))
            {
                // Unknown to the restored server: given back as a re-push (C18 case 4).
                requeue(&tx, r, now)?;
                left.push(key);
            } else if has_entries(&tx, key)? {
                left.push(key);
            } else {
                remove.push(key);
            }
        }
        for key in &left {
            let folder = match get_any(&tx, *key)? {
                Some(AnyRow::Snippet(s)) => Some(s.folder_id),
                Some(AnyRow::Folder(f)) => f.parent_id,
                Some(AnyRow::FolderShare(s)) => Some(s.folder_id),
                _ => None,
            };
            let mut cur = folder;
            let mut steps = 0;
            while let Some(f) = cur {
                keep_folders.insert(f);
                cur = tx
                    .prepare_cached("SELECT parent_id FROM folder WHERE id = ?1")?
                    .query_row([f.as_bytes()], |r| opt_id_at(r, 0))
                    .optional()?
                    .flatten();
                steps += 1;
                if steps > 8 {
                    break;
                }
            }
        }
        let mut busy_folders = Vec::new();
        for key in &remove {
            if key.kind == RowKind::Folder && keep_folders.contains(&key.id) {
                busy_folders.push(*key);
                continue;
            }
            if !exists(&tx, *key)? {
                continue;
            }
            if key.kind == RowKind::Profile || key.kind == RowKind::Membership {
                // Only when nothing the device keeps refers to it.
                let referenced: bool = key.kind == RowKind::Profile
                    && tx
                        .prepare_cached(
                            "SELECT EXISTS (SELECT 1 FROM folder WHERE profile_id = ?1)
                                 OR EXISTS (SELECT 1 FROM tag WHERE profile_id = ?1)
                                 OR EXISTS (SELECT 1 FROM attachment WHERE profile_id = ?1)
                                 OR EXISTS (SELECT 1 FROM folder_share WHERE profile_id = ?1)
                                 OR EXISTS (SELECT 1 FROM membership WHERE profile_id = ?1 AND user_id = ?2)",
                        )?
                        .query_row(
                            params![key.id.as_bytes(), me.map(|u| u.as_bytes().to_vec())],
                            |r| r.get(0),
                        )?;
                if referenced {
                    continue;
                }
            }
            if key.kind == RowKind::Folder {
                // Children not seen were removed before it (the list is children first); what
                // remains under it was seen, and keeps its place at the top.
                tx.prepare_cached("UPDATE folder SET parent_id = NULL WHERE parent_id = ?1")?
                    .execute([key.id.as_bytes()])?;
                let snippets =
                    ids_where(&tx, "SELECT id FROM snippet WHERE folder_id = ?1", key.id)?;
                if !snippets.is_empty() {
                    // Something the device keeps is still in it: left alone until that goes.
                    busy_folders.push(*key);
                    continue;
                }
            }
            hard_delete(&tx, *key)?;
        }
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO left_alone (entity, row_id) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            )?;
            for key in left.iter().chain(busy_folders.iter()) {
                stmt.execute(params![key.kind.as_str(), key.id.as_bytes()])?;
            }
        }
        let mut lost_calls = Vec::new();
        if epoch {
            let lines: BTreeMap<Id, i64> = {
                let mut stmt = tx.prepare_cached("SELECT profile_id, backup_rev FROM lost_line")?;
                let rows = stmt.query_map([], |r| Ok((id_at(r, 0)?, r.get(1)?)))?;
                rows.collect::<Result<_, _>>()?
            };
            for a in read_access_removals(&tx)? {
                let lost = match (a.mark, a.profile_id) {
                    (None, _) | (_, None) => true,
                    (Some(m), Some(p)) => lines.get(&p).is_none_or(|line| i64_of(m) > *line),
                };
                if lost {
                    lost_calls.push(a);
                }
            }
            // A new epoch: the files this device sent are forgotten (§6.11).
            tx.execute("DELETE FROM attachment_state", [])?;
        }
        if scope.is_none() {
            tx.execute(
                "UPDATE sync_state SET set_aside_at = NULL, signed_out_at = NULL WHERE id = 1",
                [],
            )?;
        }
        tx.execute(
            "UPDATE sync_state SET reconcile_token = NULL, reconcile_scope = NULL, reconcile_epoch = 0
             WHERE id = 1",
            [],
        )?;
        tx.execute("DELETE FROM reconcile_seen", [])?;
        cleanup_unused(&tx)?;
        if let Some(p) = position {
            set_position(&tx, Some(p), now)?;
        }
        tx.commit()?;
        Ok(lost_calls)
    }

    /// Forgets the lines of a restore once the device has given back what it lost.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn clear_lost_lines(&mut self) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        tx.execute("DELETE FROM lost_line", [])?;
        tx.execute(
            "UPDATE sync_state SET lost_epoch = NULL, lost_started_at = NULL, lost_device_epoch = NULL WHERE id = 1",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Replaces the user's access roots (§6.5 `access`, `SetAccess`).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn set_access(&mut self, roots: &[AccessRoot]) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        tx.execute("DELETE FROM folder_access", [])?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO folder_access (folder_id, profile_id, permission, via, granted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (folder_id) DO UPDATE SET
                     permission = excluded.permission, via = excluded.via, granted_at = excluded.granted_at",
            )?;
            for r in roots {
                stmt.execute(params![
                    r.folder_id.as_bytes(),
                    r.profile_id.as_bytes(),
                    r.permission.as_str(),
                    r.via,
                    r.granted_at.0
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The user's access roots as last set.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn access_roots(&self) -> Result<Vec<AccessRoot>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT folder_id, profile_id, permission, via, granted_at FROM folder_access ORDER BY folder_id",
        )?;
        let rows = stmt.query_map([], |r| {
            let p: String = r.get(2)?;
            Ok(AccessRoot {
                folder_id: id_at(r, 0)?,
                profile_id: id_at(r, 1)?,
                permission: iamb_core::sync::Access::parse(&p)
                    .unwrap_or(iamb_core::sync::Access::View),
                via: r.get(3)?,
                granted_at: Timestamp(r.get(4)?),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Marks the entries of a change as blocked (D17 `SetBlocked`), with every change that
    /// depends on it.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn set_blocked(&mut self, seqs: &[i64], reason: &str) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        {
            let mut stmt = tx.prepare_cached(
                "UPDATE outbox SET blocked = ?1, tries = tries + 1
                 WHERE entity || hex(row_id) IN (SELECT entity || hex(row_id) FROM outbox WHERE seq = ?2)",
            )?;
            for s in seqs {
                stmt.execute(params![reason, s])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Offers blocked changes again (when they change, when the entitlements change, once a day,
    /// D5, D17). With a reason, only changes blocked for it.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn unblock(&mut self, reason: Option<&str>) -> Result<usize, StoreError> {
        let tx = self.sync_tx()?;
        let n = match reason {
            Some(r) => tx.execute("UPDATE outbox SET blocked = NULL WHERE blocked = ?1", [r])?,
            None => tx.execute(
                "UPDATE outbox SET blocked = NULL WHERE blocked IS NOT NULL",
                [],
            )?,
        };
        tx.commit()?;
        Ok(n)
    }

    /// The rows with blocked changes.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn blocked_rows(&self) -> Result<Vec<(RowRef, String)>, StoreError> {
        let mut out = BTreeMap::new();
        for e in read_entries(&self.conn, None)? {
            if let Some(b) = e.blocked {
                out.insert(e.key, b);
            }
        }
        Ok(out.into_iter().collect())
    }

    /// The rows with waiting (not blocked) changes.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn waiting_rows(&self) -> Result<Vec<RowRef>, StoreError> {
        let mut out = BTreeSet::new();
        let mut blocked = BTreeSet::new();
        for e in read_entries(&self.conn, None)? {
            if e.blocked.is_some() {
                blocked.insert(e.key);
            } else {
                out.insert(e.key);
            }
        }
        Ok(out.difference(&blocked).copied().collect())
    }

    /// Whether the row has an outbox entry that was not handed out yet, besides the newest (the
    /// test harness's "a later change took its place before it was sent", place 7 of §1).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn unsent_entries(&self, key: RowRef) -> Result<usize, StoreError> {
        Ok(read_entries(&self.conn, Some(key))?
            .iter()
            .filter(|e| !e.handed_out)
            .count())
    }

    /// The kept changes (`ListKeptChanges`).
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn kept_changes(&self) -> Result<Vec<KeptChange>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, entity, row_id, kind, content, held, place, reason, made_at, kept_at
             FROM kept_change ORDER BY kept_at, id",
        )?;
        let rows = stmt.query_map([], |r| {
            let e: String = r.get(1)?;
            let kind: String = r.get(3)?;
            let content: Option<String> = r.get(4)?;
            Ok((
                id_at(r, 0)?,
                e,
                id_at(r, 2)?,
                kind,
                content,
                r.get::<_, bool>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, i64>(9)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, e, row_id, kind, content, is_held, place, reason, made, kept) = row?;
            out.push(KeptChange {
                id,
                key: RowRef {
                    kind: RowKind::parse(&e).unwrap_or(RowKind::Snippet),
                    id: row_id,
                },
                delete: kind == "delete",
                content: content
                    .map(|c| serde_json::from_str(&c))
                    .transpose()
                    .map_err(|e| StoreError::InvalidData(e.to_string()))?,
                held: is_held,
                place,
                reason,
                made_at: Timestamp(made),
                kept_at: Timestamp(kept),
            });
        }
        Ok(out)
    }

    /// Removes a kept change (`DismissKeptChange`).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn dismiss_kept_change(&mut self, id: Id) -> Result<bool, StoreError> {
        let tx = self.sync_tx()?;
        let n = tx.execute("DELETE FROM kept_change WHERE id = ?1", [id.as_bytes()])?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// Notes an account call that takes access away, before it is sent (without a mark) and again
    /// when it is answered `ok` (with the mark); `None` for the mark of a refused call removes the
    /// entry (D7 `NoteAccessRemoval`).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn note_access_removal(
        &mut self,
        entry: &AccessRemoval,
        refused: bool,
    ) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        if refused {
            tx.execute(
                "DELETE FROM access_removal WHERE id = ?1",
                [entry.id.as_bytes()],
            )?;
        } else {
            tx.execute(
                "INSERT INTO access_removal (id, call, profile_id, subject, mark, made_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (id) DO UPDATE SET mark = excluded.mark",
                params![
                    entry.id.as_bytes(),
                    entry.call,
                    entry.profile_id.map(|p| p.as_bytes().to_vec()),
                    entry.subject.map(|p| p.as_bytes().to_vec()),
                    entry.mark.map(i64_of),
                    entry.made_at.0
                ],
            )?;
        }
        // Kept 90 days (§7.1).
        tx.execute(
            "DELETE FROM access_removal WHERE made_at < ?1",
            [entry.made_at.0 - 3 * RETENTION_MS],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The access removals noted (`AccessRemovals`).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn access_removals(&self) -> Result<Vec<AccessRemoval>, StoreError> {
        read_access_removals(&self.conn)
    }

    /// Drops the access removals of a group and person once the device sees the same access given
    /// back (D7).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn drop_access_removals(&mut self, profile: Id, subject: Id) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        tx.execute(
            "DELETE FROM access_removal WHERE profile_id = ?1 AND subject = ?2",
            params![profile.as_bytes(), subject.as_bytes()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// After a sign-out (D14): confirmed by the server, the rows outside the personal profile are
    /// purged, their unsent changes kept (`keep`) or discarded; otherwise they are set aside.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn signed_out(
        &mut self,
        confirmed: bool,
        keep: bool,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        ensure_state(&tx)?;
        // Whichever kind of sign-out, the next sign-in resyncs (D14).
        tx.execute(
            "UPDATE sync_state SET signed_out_at = coalesce(signed_out_at, ?1) WHERE id = 1",
            [now.0],
        )?;
        if confirmed {
            remove_foreign(&tx, !keep, now)?;
            tx.execute("UPDATE sync_state SET set_aside_at = NULL WHERE id = 1", [])?;
        } else {
            tx.execute(
                "UPDATE sync_state SET set_aside_at = coalesce(set_aside_at, ?1) WHERE id = 1",
                [now.0],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Ends what a sign-out set aside more than 30 days before `now` (D8, D14 case 2): the rows
    /// outside the personal profile are purged, and every row whose version the device's own user
    /// wrote in the 30 days before the sign-out is kept as a change that was not synced.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn end_set_aside(&mut self, now: Timestamp, force: bool) -> Result<bool, StoreError> {
        let tx = self.sync_tx()?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let at: Option<i64> = tx
            .query_row(
                "SELECT set_aside_at FROM sync_state WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let Some(at) = at else {
            return Ok(false);
        };
        if !force && now.0 - at < RETENTION_MS {
            return Ok(false);
        }
        let me = account(&tx)?;
        let personal = personal_profile(&tx)?;
        // Rows of other profiles the user wrote in the 30 days before the sign-out.
        for kind in [RowKind::Folder, RowKind::Snippet] {
            for r in all_of(&tx, kind)? {
                let p = profile_of(&tx, &r)?;
                if p.is_some()
                    && p != personal
                    && me.is_some()
                    && r.meta().updated_by == me
                    && r.meta().updated_at.0 >= at - RETENTION_MS
                    && !has_entries(&tx, r.key())?
                {
                    keep(
                        &tx,
                        r.key(),
                        Some(&r),
                        r.meta().is_deleted(),
                        "signed_out",
                        r.meta().updated_at.0,
                        now,
                    )?;
                }
            }
        }
        remove_foreign(&tx, false, now)?;
        tx.execute("UPDATE sync_state SET set_aside_at = NULL WHERE id = 1", [])?;
        tx.commit()?;
        Ok(true)
    }

    /// The first sign-in on a device (D9 step 1): the local library becomes the account's.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn adopt(
        &mut self,
        user: Id,
        personal: Id,
        device: Id,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let data_dir = self.data_dir.clone();
        let tx = self.sync_tx()?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        ensure_state(&tx)?;
        let local: Vec<Id> = {
            let mut stmt = tx.prepare_cached(
                "SELECT id FROM profile WHERE kind = 'personal' AND rev = 0 AND id <> ?1",
            )?;
            let rows = stmt.query_map([personal.as_bytes()], |r| id_at(r, 0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        tx.execute(
            "INSERT INTO profile (id, created_at, updated_at, updated_by, rev, deleted_at, kind, name, owner_user_id)
             VALUES (?1, ?2, ?2, ?3, 0, NULL, 'personal', 'Personal', ?3)
             ON CONFLICT (id) DO NOTHING",
            params![personal.as_bytes(), now.0, user.as_bytes()],
        )?;
        for p in &local {
            for table in ["folder", "snippet", "tag", "attachment"] {
                tx.execute(
                    &format!("UPDATE {table} SET profile_id = ?1 WHERE profile_id = ?2"),
                    params![personal.as_bytes(), p.as_bytes()],
                )?;
            }
            tx.execute("DELETE FROM profile WHERE id = ?1", [p.as_bytes()])?;
            if let Some(dir) = &data_dir {
                let from = dir.join("attachments").join(p.to_string());
                let to = dir.join("attachments").join(personal.to_string());
                if from.exists() && !to.exists() {
                    std::fs::rename(&from, &to)?;
                }
            }
        }
        // Tombstones go; every row never synced gets an entry.
        for table in [
            "snippet_tag",
            "snippet_attachment",
            "folder_share",
            "snippet",
            "tag",
            "attachment",
        ] {
            tx.execute(
                &format!(
                    "DELETE FROM {table} WHERE deleted_at IS NOT NULL AND rev = 0
                     AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.entity = '{table}' AND o.row_id = {table}.id AND o.handed_out = 1)"
                ),
                [],
            )?;
        }
        tx.execute(
            "DELETE FROM outbox WHERE handed_out = 0 AND entity IN ('profile', 'membership')",
            [],
        )?;
        for kind in [
            RowKind::Folder,
            RowKind::Tag,
            RowKind::Attachment,
            RowKind::Snippet,
            RowKind::SnippetTag,
            RowKind::SnippetAttachment,
            RowKind::FolderShare,
        ] {
            for r in all_of(&tx, kind)? {
                if r.meta().rev == 0 && !r.meta().is_deleted() && !has_entries(&tx, r.key())? {
                    ops::outbox_push(&tx, kind.into(), r.id(), crate::OutboxOp::Put, 0, now)?;
                }
            }
        }
        tx.execute(
            "UPDATE sync_state SET account_user_id = ?1, personal_profile_id = ?2, device_id = ?3,
                 adoption = 'adopted' WHERE id = 1",
            params![user.as_bytes(), personal.as_bytes(), device.as_bytes()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Sets the adoption phase (`merging` once the resync of D9 step 2 is done).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn set_adoption(&mut self, phase: Option<Adoption>) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        ensure_state(&tx)?;
        tx.execute(
            "UPDATE sync_state SET adoption = ?1 WHERE id = 1",
            [match phase {
                Some(Adoption::Adopted) => Some("adopted"),
                Some(Adoption::Merging) => Some("merging"),
                None => None,
            }],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// D9 step 3: a never-synced local folder with the name and parent of a folder of the account
    /// gives its content to it and is dropped; a local tag with the name of an account tag likewise;
    /// a local snippet equal to one of the same folder is dropped. Clears the phase.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn adopt_merge(&mut self, now: Timestamp) -> Result<(), StoreError> {
        let tx = self.sync_tx()?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        // Folders, top down.
        loop {
            let pair: Option<(Id, Id)> = tx
                .query_row(
                    "SELECT l.id, a.id FROM folder l JOIN folder a
                       ON a.profile_id = l.profile_id AND a.name = l.name AND a.id <> l.id
                      AND (a.parent_id IS l.parent_id) AND a.rev > 0 AND a.deleted_at IS NULL
                     WHERE l.rev = 0 AND l.deleted_at IS NULL
                     ORDER BY l.id LIMIT 1",
                    [],
                    |r| Ok((id_at(r, 0)?, id_at(r, 1)?)),
                )
                .optional()?;
            let Some((local, target)) = pair else { break };
            tx.execute(
                "UPDATE folder SET parent_id = ?1 WHERE parent_id = ?2",
                params![target.as_bytes(), local.as_bytes()],
            )?;
            tx.execute(
                "UPDATE snippet SET folder_id = ?1 WHERE folder_id = ?2",
                params![target.as_bytes(), local.as_bytes()],
            )?;
            // The moved rows are changes like any other.
            for s in ids_where(
                &tx,
                "SELECT id FROM snippet WHERE folder_id = ?1 AND rev = 0",
                target,
            )? {
                let key = RowRef {
                    kind: RowKind::Snippet,
                    id: s,
                };
                if !has_entries(&tx, key)? {
                    ops::outbox_push(&tx, EntityKind::Snippet, s, crate::OutboxOp::Put, 0, now)?;
                }
            }
            let key = RowRef {
                kind: RowKind::Folder,
                id: local,
            };
            delete_row_entries(&tx, key)?;
            hard_delete(&tx, key)?;
        }
        // Tags.
        loop {
            let pair: Option<(Id, Id)> = tx
                .query_row(
                    "SELECT l.id, a.id FROM tag l JOIN tag a
                       ON a.profile_id = l.profile_id AND a.name = l.name AND a.id <> l.id
                      AND a.rev > 0 AND a.deleted_at IS NULL
                     WHERE l.rev = 0 AND l.deleted_at IS NULL ORDER BY l.id LIMIT 1",
                    [],
                    |r| Ok((id_at(r, 0)?, id_at(r, 1)?)),
                )
                .optional()?;
            let Some((local, target)) = pair else { break };
            tx.execute(
                "UPDATE snippet_tag SET tag_id = ?1 WHERE tag_id = ?2",
                params![target.as_bytes(), local.as_bytes()],
            )?;
            let key = RowRef {
                kind: RowKind::Tag,
                id: local,
            };
            delete_row_entries(&tx, key)?;
            hard_delete(&tx, key)?;
        }
        // Snippets equal to one of the same folder.
        let dups = {
            let mut stmt = tx.prepare_cached(
                "SELECT l.id FROM snippet l JOIN snippet a
                   ON a.folder_id = l.folder_id AND a.id <> l.id AND a.rev > 0 AND a.deleted_at IS NULL
                  AND a.abbreviation = l.abbreviation AND a.label = l.label AND a.kind = l.kind
                  AND a.body = l.body AND a.trigger_mode = l.trigger_mode AND a.case_mode = l.case_mode
                  AND a.word_start = l.word_start AND a.enabled = l.enabled
                 WHERE l.rev = 0 AND l.deleted_at IS NULL",
            )?;
            let rows = stmt.query_map([], |r| id_at(r, 0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for s in dups {
            let key = RowRef {
                kind: RowKind::Snippet,
                id: s,
            };
            delete_row_entries(&tx, key)?;
            hard_delete(&tx, key)?;
        }
        tx.execute("UPDATE sync_state SET adoption = NULL WHERE id = 1", [])?;
        tx.commit()?;
        Ok(())
    }

    /// A snippet's origin, if the server has said.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn snippet_origin(&self, snippet: Id) -> Result<Option<Origin>, StoreError> {
        get_origin(&self.conn, snippet)
    }

    /// Every synced row the store holds, tombstones included (for checks and diagnostics).
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn all_rows(&self) -> Result<Vec<AnyRow>, StoreError> {
        let mut out = Vec::new();
        for kind in RowKind::ALL {
            out.extend(all_of(&self.conn, kind)?);
        }
        Ok(out)
    }

    /// One row of any kind, tombstones included.
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn get_row(&self, key: RowRef) -> Result<Option<AnyRow>, StoreError> {
        get_any(&self.conn, key)
    }

    /// The highest revision of any row the store holds (`hello`'s `max_rev`, §6.2).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn max_rev(&self) -> Result<u64, StoreError> {
        let mut max = 0u64;
        for kind in RowKind::ALL {
            let v: i64 = self.conn.query_row(
                &format!("SELECT coalesce(max(rev), 0) FROM {}", kind.as_str()),
                [],
                |r| r.get(0),
            )?;
            max = max.max(u64::try_from(v).unwrap_or(0));
        }
        Ok(max)
    }

    /// The top folders the store holds of profiles other than the personal one (for the access
    /// check of D7).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn foreign_folders(&self) -> Result<Vec<Id>, StoreError> {
        let personal = personal_profile(&self.conn)?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, profile_id FROM folder ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((id_at(r, 0)?, id_at(r, 1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, p) = row?;
            if Some(p) != personal {
                out.push(id);
            }
        }
        Ok(out)
    }
}

/// Removes local tombstones the server never saw, rows that nothing still refers to first.
fn drop_garbage(conn: &Connection, keys: &[RowRef]) -> Result<(), StoreError> {
    let mut left: Vec<RowRef> = keys.to_vec();
    loop {
        let mut progress = false;
        let mut next = Vec::new();
        for key in left {
            let busy: bool = match key.kind {
                RowKind::Folder => conn
                    .prepare_cached(
                        "SELECT EXISTS (SELECT 1 FROM folder WHERE parent_id = ?1)
                             OR EXISTS (SELECT 1 FROM snippet WHERE folder_id = ?1)",
                    )?
                    .query_row([key.id.as_bytes()], |r| r.get(0))?,
                RowKind::Snippet => conn
                    .prepare_cached(
                        "SELECT EXISTS (SELECT 1 FROM snippet_tag WHERE snippet_id = ?1)
                             OR EXISTS (SELECT 1 FROM snippet_attachment WHERE snippet_id = ?1)",
                    )?
                    .query_row([key.id.as_bytes()], |r| r.get(0))?,
                RowKind::Tag => conn
                    .prepare_cached("SELECT EXISTS (SELECT 1 FROM snippet_tag WHERE tag_id = ?1)")?
                    .query_row([key.id.as_bytes()], |r| r.get(0))?,
                RowKind::Attachment => conn
                    .prepare_cached(
                        "SELECT EXISTS (SELECT 1 FROM snippet_attachment WHERE attachment_id = ?1)",
                    )?
                    .query_row([key.id.as_bytes()], |r| r.get(0))?,
                _ => false,
            };
            if busy {
                next.push(key);
            } else {
                // A pull's note on the row stays: the server's version is read afterwards (D15).
                let note: Option<(i64, bool)> = conn
                    .prepare_cached(
                        "SELECT rev, gone FROM skipped WHERE entity = ?1 AND row_id = ?2",
                    )?
                    .query_row(params![key.kind.as_str(), key.id.as_bytes()], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?;
                hard_delete(conn, key)?;
                if let Some((rev, gone)) = note {
                    note_skipped(conn, key, u64::try_from(rev).unwrap_or(0), gone)?;
                }
                progress = true;
            }
        }
        if !progress || next.is_empty() {
            return Ok(());
        }
        left = next;
    }
}

/// Whether both ends of a link are live on the device (true for rows that are not links).
fn link_ends_live(conn: &Connection, row: &AnyRow) -> Result<bool, StoreError> {
    let live = |k: RowRef| -> Result<bool, StoreError> {
        Ok(get_any(conn, k)?.is_some_and(|r| !r.meta().is_deleted()))
    };
    match row {
        AnyRow::SnippetTag(l) => Ok(live(RowRef {
            kind: RowKind::Snippet,
            id: l.snippet_id,
        })? && live(RowRef {
            kind: RowKind::Tag,
            id: l.tag_id,
        })?),
        AnyRow::SnippetAttachment(l) => Ok(live(RowRef {
            kind: RowKind::Snippet,
            id: l.snippet_id,
        })? && live(RowRef {
            kind: RowKind::Attachment,
            id: l.attachment_id,
        })?),
        _ => Ok(true),
    }
}

/// Applies one cascade entry of a folder delete's answer (D4, D5).
fn apply_cascade(
    tx: &Connection,
    c: &CascadeResult,
    covered: &BTreeSet<i64>,
    now: Timestamp,
    behind: &mut BTreeSet<RowRef>,
) -> Result<Option<RowRef>, StoreError> {
    let key = c.key;
    let entries = read_entries(tx, Some(key))?;
    let mine: Vec<i64> = entries
        .iter()
        .filter(|e| covered.contains(&e.seq))
        .map(|e| e.seq)
        .collect();
    let newer = entries.iter().any(|e| !covered.contains(&e.seq));
    // A change of its own the row had before the folder took it (a move into the folder, an
    // edit): the device does not hold the content of the base the cascade list named.
    let changed_before = entries
        .iter()
        .any(|e| covered.contains(&e.seq) && e.cascade_of.is_none());
    delete_entries(tx, &mine)?;
    if newer {
        // The user undid the delete while the push was under way: keep the content, take the
        // revision; its own change follows (D4).
        if let Some(rev) = c.rev {
            set_rev(tx, key, rev)?;
            tx.prepare_cached("UPDATE outbox SET base_rev = ?1 WHERE entity = ?2 AND row_id = ?3")?
                .execute(params![i64_of(rev), key.kind.as_str(), key.id.as_bytes()])?;
        }
        return Ok(None);
    }
    if c.gone {
        if get_any(tx, key)?.is_some() {
            remove_row(tx, key, now)?;
        }
        return Ok(Some(key));
    }
    if c.fetch {
        note_skipped(tx, key, c.rev.unwrap_or(u64::MAX >> 1), false)?;
        behind.insert(key);
        return Ok(Some(key));
    }
    if let Some(r) = &c.row {
        if refs_present(tx, r)? {
            write_remote(tx, r, c.origin.as_ref())?;
        }
        return Ok(Some(key));
    }
    if let Some(rev) = c.rev
        && get_any(tx, key)?.is_some()
    {
        if !c.deleted && changed_before {
            // Live on the server in a version the device does not hold: read it with `rows`
            // (D15). Until then the row counts as never synced, so the read writes it.
            set_rev(tx, key, 0)?;
            note_skipped(tx, key, rev, false)?;
            behind.insert(key);
            return Ok(Some(key));
        }
        set_rev(tx, key, rev)?;
        if c.deleted {
            tx.prepare_cached(&format!(
                "UPDATE {} SET deleted_at = coalesce(deleted_at, ?1) WHERE id = ?2",
                key.kind.as_str()
            ))?
            .execute(params![now.0, key.id.as_bytes()])?;
        } else {
            // Live in the revision the device had: the delete did not take it.
            set_deleted(tx, key, None)?;
        }
    }
    Ok(Some(key))
}

/// D7: left-alone rows that no longer have an entry, and no row with an entry below them, are
/// removed, children before parents.
fn remove_left_alone(tx: &Connection, now: Timestamp) -> Result<Vec<RowRef>, StoreError> {
    let mut removed = Vec::new();
    loop {
        let list: Vec<RowRef> = {
            let mut stmt = tx.prepare_cached("SELECT entity, row_id FROM left_alone")?;
            let rows = stmt.query_map([], |r| {
                let e: String = r.get(0)?;
                Ok(RowRef {
                    kind: RowKind::parse(&e).unwrap_or(RowKind::Snippet),
                    id: id_at(r, 1)?,
                })
            })?;
            rows.collect::<Result<_, _>>()?
        };
        let mut progress = false;
        for key in list {
            if has_entries(tx, key)? {
                continue;
            }
            if key.kind == RowKind::Folder {
                let busy: bool = tx
                    .prepare_cached(
                        "SELECT EXISTS (SELECT 1 FROM folder WHERE parent_id = ?1)
                             OR EXISTS (SELECT 1 FROM snippet WHERE folder_id = ?1)",
                    )?
                    .query_row([key.id.as_bytes()], |r| r.get(0))?;
                if busy {
                    continue;
                }
            }
            tx.prepare_cached("DELETE FROM left_alone WHERE entity = ?1 AND row_id = ?2")?
                .execute(params![key.kind.as_str(), key.id.as_bytes()])?;
            // The server's answer may have brought the row back (it is then synced and stays).
            let skipped: bool = tx
                .prepare_cached(
                    "SELECT EXISTS (SELECT 1 FROM skipped WHERE entity = ?1 AND row_id = ?2)",
                )?
                .query_row(params![key.kind.as_str(), key.id.as_bytes()], |r| r.get(0))?;
            if !skipped && exists(tx, key)? {
                remove_row(tx, key, now)?;
                removed.push(key);
            }
            progress = true;
        }
        if !progress {
            break;
        }
    }
    Ok(removed)
}

/// Removes everything outside the personal profile (a confirmed sign-out, D14).
fn remove_foreign(tx: &Connection, discard: bool, now: Timestamp) -> Result<(), StoreError> {
    let personal = personal_profile(tx)?;
    let tops = {
        let mut stmt = tx.prepare_cached("SELECT id, profile_id, parent_id FROM folder")?;
        let rows = stmt.query_map([], |r| Ok((id_at(r, 0)?, id_at(r, 1)?, opt_id_at(r, 2)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let foreign: BTreeSet<Id> = tops
        .iter()
        .filter(|(_, p, _)| Some(*p) != personal)
        .map(|(id, _, _)| *id)
        .collect();
    for (id, _, parent) in &tops {
        if foreign.contains(id) && parent.is_none_or(|p| !foreign.contains(&p)) {
            purge_tree(tx, *id, &BTreeSet::new(), "signed_out", discard, now)?;
        }
    }
    tx.execute("DELETE FROM membership", [])?;
    tx.execute("DELETE FROM folder_access", [])?;
    if let Some(p) = personal {
        for table in ["tag", "attachment"] {
            let ids = ids_where(
                tx,
                &format!("SELECT id FROM {table} WHERE profile_id <> ?1"),
                p,
            )?;
            for id in ids {
                let key = RowRef {
                    kind: RowKind::parse(table).unwrap_or(RowKind::Tag),
                    id,
                };
                keep_unsent(tx, &[key], "signed_out", discard, now)?;
                hard_delete(tx, key)?;
            }
        }
        // The profile of a group the user shares a folder of their own with stays: the share is
        // part of the user's own library.
        let ids = ids_where(
            tx,
            "SELECT id FROM profile WHERE id <> ?1
                 AND NOT EXISTS (SELECT 1 FROM folder_share WHERE folder_share.profile_id = profile.id)",
            p,
        )?;
        for id in ids {
            hard_delete(
                tx,
                RowRef {
                    kind: RowKind::Profile,
                    id,
                },
            )?;
        }
    }
    Ok(())
}

fn read_access_removals(conn: &Connection) -> Result<Vec<AccessRemoval>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, call, profile_id, subject, mark, made_at FROM access_removal ORDER BY made_at, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(AccessRemoval {
            id: id_at(r, 0)?,
            call: r.get(1)?,
            profile_id: opt_id_at(r, 2)?,
            subject: opt_id_at(r, 3)?,
            mark: r
                .get::<_, Option<i64>>(4)?
                .and_then(|v| u64::try_from(v).ok()),
            made_at: Timestamp(r.get(5)?),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Whether a profile is a group (for callers that only hold its id).
#[must_use]
pub const fn is_group(kind: ProfileKind) -> bool {
    !matches!(kind, ProfileKind::Personal)
}
