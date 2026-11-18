//! Forward-only, numbered schema migrations embedded in the binary.
//!
//! Each migration lives in `migrations/NNNN_name.sql`, runs in its own transaction and is
//! recorded in `schema_migrations`. Migrations are never edited once released: a schema change
//! is always a new file with the next number, plus a fixture in
//! `tests/migrations.rs` so the upgrade test covers it.

use rusqlite::{Connection, OptionalExtension};

use crate::error::StoreError;

/// One schema migration.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// Version reached after applying it; versions are 1, 2, 3, … without gaps.
    pub version: u32,
    /// Short name, matching the file name.
    pub name: &'static str,
    /// The SQL, run as one batch.
    pub sql: &'static str,
}

/// All migrations, in order.
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial",
    sql: include_str!("../migrations/0001_initial.sql"),
}];

/// The schema version this build creates and understands.
pub const LATEST_VERSION: u32 = MIGRATIONS[MIGRATIONS.len() - 1].version;

/// Created before any migration so the version can be read from any database.
const BOOTSTRAP: &str = "
-- Applied migration versions. `applied_at` is UTC milliseconds from SQLite's own clock, so the
-- store never reads the system clock itself.
CREATE TABLE IF NOT EXISTS schema_migrations (
    version    INTEGER NOT NULL PRIMARY KEY,
    applied_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000 AS INTEGER))
);";

/// The highest applied version, or 0 for a new database.
///
/// # Errors
///
/// Fails on a `SQLite` error.
pub fn current_version(conn: &Connection) -> Result<u32, StoreError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    let v: Option<u32> = conn
        .query_row("SELECT max(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .optional()?
        .flatten();
    Ok(v.unwrap_or(0))
}

/// Applies every known migration up to and including `target`, each in its own transaction.
/// Returns the version reached.
///
/// The store calls this with [`LATEST_VERSION`]; tests call it with older versions to build
/// databases as an earlier release would have left them.
///
/// # Errors
///
/// [`StoreError::SchemaTooNew`] if the database is already past [`LATEST_VERSION`];
/// [`StoreError::InvalidData`] if `target` is unknown; otherwise `SQLite` errors.
pub fn migrate_to(conn: &mut Connection, target: u32) -> Result<u32, StoreError> {
    conn.execute_batch(BOOTSTRAP)?;
    let current = current_version(conn)?;
    if current > LATEST_VERSION {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported: LATEST_VERSION,
        });
    }
    if target > LATEST_VERSION {
        return Err(StoreError::InvalidData(format!(
            "unknown schema version {target} (latest is {LATEST_VERSION})"
        )));
    }
    let mut reached = current;
    for m in MIGRATIONS
        .iter()
        .filter(|m| m.version > current && m.version <= target)
    {
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            [m.version],
        )?;
        tx.commit()?;
        reached = m.version;
    }
    Ok(reached)
}

#[cfg(test)]
mod tests {
    use super::{LATEST_VERSION, MIGRATIONS};

    #[test]
    fn versions_are_contiguous_from_one() {
        for (i, m) in MIGRATIONS.iter().enumerate() {
            assert_eq!(
                usize::try_from(m.version).ok(),
                Some(i + 1),
                "migration {}",
                m.name
            );
        }
        assert_eq!(usize::try_from(LATEST_VERSION).ok(), Some(MIGRATIONS.len()));
    }
}
