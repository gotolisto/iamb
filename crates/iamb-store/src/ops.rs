//! Generic row operations, shared by [`Store`](crate::Store) and [`WriteTx`](crate::WriteTx).
//!
//! Every function takes a `&Connection`; writers are always called inside a transaction.

use std::fmt::Write as _;

use iamb_core::model::{Folder, Id, Snippet, SnippetTag, SyncMeta, Tag, Timestamp};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::codec::{
    id_at, id_val, opt_id_val, opt_ts_val, ts_at, ts_val, u64_at, u64_int, u64_val,
};
use crate::entity::{Entity, EntityKind, WriteMode};
use crate::error::StoreError;
use crate::{Library, ListOptions, OutboxEntry, OutboxOp};

const COMMON_COLUMNS: [&str; 6] = [
    "id",
    "created_at",
    "updated_at",
    "updated_by",
    "rev",
    "deleted_at",
];

/// `SELECT <common>, <read columns> FROM <table> <alias>`.
fn select_sql<E: Entity>(alias: &str) -> String {
    let mut sql = String::from("SELECT ");
    for (i, col) in COMMON_COLUMNS.iter().chain(E::READ_COLUMNS).enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        let _ = write!(sql, "{alias}.{col}");
    }
    let _ = write!(sql, " FROM {} {alias}", E::TABLE);
    sql
}

pub(crate) fn get<E: Entity>(
    conn: &Connection,
    id: Id,
    opts: ListOptions,
) -> Result<Option<E>, StoreError> {
    let mut sql = select_sql::<E>("t");
    sql.push_str(" WHERE t.id = ?1");
    if !opts.include_deleted {
        sql.push_str(" AND t.deleted_at IS NULL");
    }
    Ok(conn
        .prepare_cached(&sql)?
        .query_row([id.as_bytes()], |r| E::from_row(r))
        .optional()?)
}

pub(crate) fn list<E: Entity>(conn: &Connection, opts: ListOptions) -> Result<Vec<E>, StoreError> {
    let mut sql = select_sql::<E>("t");
    if !opts.include_deleted {
        sql.push_str(" WHERE t.deleted_at IS NULL");
    }
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], |r| E::from_row(r))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// `(created_at, rev, deleted)` of an existing row.
fn existing(
    conn: &Connection,
    table: &str,
    id: Id,
) -> Result<Option<(Timestamp, u64, bool)>, StoreError> {
    let sql = format!("SELECT created_at, rev, deleted_at IS NOT NULL FROM {table} WHERE id = ?1");
    Ok(conn
        .prepare_cached(&sql)?
        .query_row([id.as_bytes()], |r| {
            Ok((ts_at(r, 0)?, u64_at(r, 1)?, r.get(2)?))
        })
        .optional()?)
}

pub(crate) fn outbox_push(
    conn: &Connection,
    kind: EntityKind,
    id: Id,
    op: OutboxOp,
    base_rev: u64,
    now: Timestamp,
) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT INTO outbox (entity, row_id, op, base_rev, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![
        kind.as_str(),
        id.as_bytes(),
        op.as_str(),
        u64_int(base_rev, "rev")?,
        now.0
    ])?;
    Ok(())
}

/// Local insert-or-update. `created_at` and `rev` of an existing row are kept; `updated_at` is
/// `now`; the row becomes live. Writes one outbox entry.
pub(crate) fn put_local<E: Entity>(
    conn: &Connection,
    value: &E,
    now: Timestamp,
) -> Result<SyncMeta, StoreError> {
    let given = value.meta();
    let id = given.id;
    let prior = existing(conn, E::TABLE, id)?;
    let values = value.write_values(conn, WriteMode::Local, true)?;
    let meta = SyncMeta {
        id,
        created_at: prior.map_or(now, |p| p.0),
        updated_at: now,
        updated_by: given.updated_by,
        rev: prior.map_or(0, |p| p.1),
        deleted_at: None,
    };
    if prior.is_some() {
        let mut sql = format!(
            "UPDATE {} SET updated_at = ?1, updated_by = ?2, deleted_at = NULL",
            E::TABLE
        );
        for (i, col) in E::WRITE_COLUMNS.iter().enumerate() {
            let _ = write!(sql, ", {col} = ?{}", i + 3);
        }
        let _ = write!(sql, " WHERE id = ?{}", E::WRITE_COLUMNS.len() + 3);
        let params = [ts_val(now), opt_id_val(meta.updated_by)]
            .into_iter()
            .chain(values)
            .chain([id_val(id)]);
        conn.prepare_cached(&sql)?
            .execute(params_from_iter(params))?;
    } else {
        let sql = insert_sql::<E>("");
        let params = common_values(&meta)?.into_iter().chain(values);
        conn.prepare_cached(&sql)?
            .execute(params_from_iter(params))?;
    }
    value.after_write(conn)?;
    outbox_push(conn, E::KIND, id, OutboxOp::Put, meta.rev, now)?;
    Ok(meta)
}

fn common_values(meta: &SyncMeta) -> Result<[Value; 6], StoreError> {
    Ok([
        id_val(meta.id),
        ts_val(meta.created_at),
        ts_val(meta.updated_at),
        opt_id_val(meta.updated_by),
        u64_val(meta.rev, "rev")?,
        opt_ts_val(meta.deleted_at),
    ])
}

fn insert_sql<E: Entity>(suffix: &str) -> String {
    let cols: Vec<&str> = COMMON_COLUMNS
        .iter()
        .copied()
        .chain(E::WRITE_COLUMNS.iter().copied())
        .collect();
    let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
    format!(
        "INSERT INTO {} ({}) VALUES ({}){suffix}",
        E::TABLE,
        cols.join(", "),
        placeholders.join(", ")
    )
}

/// Writes a row from the server exactly as given (including `rev`, times and `deleted_at`).
/// No outbox entry.
pub(crate) fn put_remote<E: Entity>(conn: &Connection, value: &E) -> Result<(), StoreError> {
    let meta = value.meta();
    let values = value.write_values(conn, WriteMode::Remote, meta.deleted_at.is_none())?;
    let mut set = String::new();
    for (i, col) in COMMON_COLUMNS
        .iter()
        .skip(1)
        .chain(E::WRITE_COLUMNS)
        .enumerate()
    {
        if i > 0 {
            set.push_str(", ");
        }
        let _ = write!(set, "{col} = excluded.{col}");
    }
    let sql = insert_sql::<E>(&format!(" ON CONFLICT (id) DO UPDATE SET {set}"));
    let params = common_values(meta)?.into_iter().chain(values);
    conn.prepare_cached(&sql)?
        .execute(params_from_iter(params))?;
    value.after_write(conn)?;
    Ok(())
}

/// Soft-deletes one row and writes its outbox entry.
fn soft_delete(
    conn: &Connection,
    kind: EntityKind,
    id: Id,
    base_rev: u64,
    now: Timestamp,
) -> Result<(), StoreError> {
    let sql = format!(
        "UPDATE {} SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
        kind.as_str()
    );
    conn.prepare_cached(&sql)?
        .execute(params![now.0, id.as_bytes()])?;
    outbox_push(conn, kind, id, OutboxOp::Delete, base_rev, now)
}

/// Local soft delete. Deleting a folder also deletes its sub-folders and their snippets.
/// Deleting a row that is already deleted changes nothing.
pub(crate) fn delete_local<E: Entity>(
    conn: &Connection,
    id: Id,
    now: Timestamp,
) -> Result<(), StoreError> {
    let Some((_, rev, deleted)) = existing(conn, E::TABLE, id)? else {
        return Err(StoreError::NotFound {
            entity: E::KIND,
            id,
        });
    };
    if deleted {
        return Ok(());
    }
    soft_delete(conn, E::KIND, id, rev, now)?;
    if E::KIND == EntityKind::Folder {
        cascade_folder(conn, id, now)?;
    }
    Ok(())
}

fn cascade_folder(conn: &Connection, id: Id, now: Timestamp) -> Result<(), StoreError> {
    const SUBTREE: &str = "WITH RECURSIVE sub(id) AS (
                               SELECT ?1
                               UNION
                               SELECT f.id FROM folder f JOIN sub ON f.parent_id = sub.id
                           )";
    let collect = |sql: &str| -> Result<Vec<(Id, u64)>, StoreError> {
        let mut stmt = conn.prepare_cached(sql)?;
        let rows = stmt.query_map([id.as_bytes()], |r| Ok((id_at(r, 0)?, u64_at(r, 1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    };
    let folders = collect(&format!(
        "{SUBTREE} SELECT id, rev FROM folder WHERE id IN (SELECT id FROM sub) AND id <> ?1 AND deleted_at IS NULL"
    ))?;
    let snippets = collect(&format!(
        "{SUBTREE} SELECT id, rev FROM snippet WHERE folder_id IN (SELECT id FROM sub) AND deleted_at IS NULL"
    ))?;
    for (fid, rev) in folders {
        soft_delete(conn, EntityKind::Folder, fid, rev, now)?;
    }
    for (sid, rev) in snippets {
        soft_delete(conn, EntityKind::Snippet, sid, rev, now)?;
    }
    Ok(())
}

/// Hard-deletes rows soft-deleted before `cutoff`, children first. A row that a surviving row
/// still refers to is kept. Returns the number of rows deleted directly (link rows, usage and
/// approvals removed by `ON DELETE CASCADE` are not counted).
pub(crate) fn purge_deleted_before(
    conn: &Connection,
    cutoff: Timestamp,
) -> Result<usize, StoreError> {
    let mut removed = 0usize;
    let mut run = |sql: &str| -> Result<usize, StoreError> {
        let n = conn.prepare_cached(sql)?.execute([cutoff.0])?;
        removed += n;
        Ok(n)
    };
    run("DELETE FROM snippet_tag WHERE deleted_at < ?1")?;
    run("DELETE FROM snippet_attachment WHERE deleted_at < ?1")?;
    run("DELETE FROM folder_share WHERE deleted_at < ?1")?;
    run("DELETE FROM membership WHERE deleted_at < ?1")?;
    run("DELETE FROM snippet WHERE deleted_at < ?1")?;
    // Leaves first; each pass removes one level.
    while run("DELETE FROM folder WHERE deleted_at < ?1
             AND NOT EXISTS (SELECT 1 FROM folder c WHERE c.parent_id = folder.id)
             AND NOT EXISTS (SELECT 1 FROM snippet s WHERE s.folder_id = folder.id)")?
        > 0
    {}
    run("DELETE FROM tag WHERE deleted_at < ?1")?;
    run("DELETE FROM attachment WHERE deleted_at < ?1")?;
    run("DELETE FROM profile WHERE deleted_at < ?1
             AND NOT EXISTS (SELECT 1 FROM folder x WHERE x.profile_id = profile.id)
             AND NOT EXISTS (SELECT 1 FROM snippet x WHERE x.profile_id = profile.id)
             AND NOT EXISTS (SELECT 1 FROM tag x WHERE x.profile_id = profile.id)
             AND NOT EXISTS (SELECT 1 FROM attachment x WHERE x.profile_id = profile.id)
             AND NOT EXISTS (SELECT 1 FROM membership x WHERE x.profile_id = profile.id)
             AND NOT EXISTS (SELECT 1 FROM folder_share x WHERE x.profile_id = profile.id)")?;
    Ok(removed)
}

pub(crate) fn load_library(conn: &Connection) -> Result<Library, StoreError> {
    let folders = list::<Folder>(conn, ListOptions::LIVE)?;
    let snippets = list::<Snippet>(conn, ListOptions::LIVE)?;
    let tags = list::<Tag>(conn, ListOptions::LIVE)?;
    let mut sql = select_sql::<SnippetTag>("st");
    sql.push_str(
        " JOIN snippet s ON s.id = st.snippet_id JOIN tag g ON g.id = st.tag_id
          WHERE st.deleted_at IS NULL AND s.deleted_at IS NULL AND g.deleted_at IS NULL",
    );
    let snippet_tags = {
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([], |r| {
            <SnippetTag as crate::entity::sealed::Row>::from_row(r)
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    Ok(Library {
        folders,
        snippets,
        tags,
        snippet_tags,
    })
}

pub(crate) fn outbox_read(conn: &Connection, limit: usize) -> Result<Vec<OutboxEntry>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq, entity, row_id, op, base_rev, created_at FROM outbox ORDER BY seq LIMIT ?1",
    )?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = stmt.query_map([limit], |r| {
        let entity: String = r.get(1)?;
        let op: String = r.get(3)?;
        Ok(OutboxEntry {
            seq: r.get(0)?,
            entity: EntityKind::parse(&entity).ok_or_else(|| {
                crate::codec::decode_err(
                    1,
                    rusqlite::types::Type::Text,
                    format!("unknown entity {entity:?}"),
                )
            })?,
            row_id: id_at(r, 2)?,
            op: OutboxOp::parse(&op).ok_or_else(|| {
                crate::codec::decode_err(
                    3,
                    rusqlite::types::Type::Text,
                    format!("unknown op {op:?}"),
                )
            })?,
            base_rev: u64_at(r, 4)?,
            created_at: ts_at(r, 5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
