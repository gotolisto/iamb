//! Local `SQLite` store (task 1.5): schema and migrations, the synced entities with soft deletes
//! and an outbox, usage statistics, settings and app exclusions.
//!
//! See `docs/architecture.md` §3.3 (files), §6.3 (the lock rule), §7 (data model) and §9 (sync),
//! and ADR-0007. One process writes the library at a time: [`Store::open`] takes an exclusive
//! lock on `run/store.lock` and holds it for the life of the [`Store`].
//!
//! The store never reads the clock: every write that records a time takes it as a parameter.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use iamb_core::model::{
    Attachment, Folder, FolderShare, Id, Membership, Profile, Snippet, SnippetAttachment,
    SnippetTag, SyncMeta, Tag, Timestamp,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

pub mod attachments;
mod codec;
mod entity;
mod error;
pub mod health;
pub mod migrations;
mod ops;
pub mod sync;
pub mod transfer;

pub use attachments::AttachmentError;
pub use entity::{Entity, EntityKind};
pub use error::StoreError;

use crate::codec::{id_at, opt_ts_at, ts_at, u64_at, u64_int};

/// The setting that holds when the usage counts were last reset (milliseconds, as text).
pub const USAGE_RESET_KEY: &str = "usage_reset_at";

/// Folders nest at most this many levels (a top-level folder is level 1).
pub const MAX_FOLDER_DEPTH: usize = 4;

/// File name of the library inside the data directory.
pub const LIBRARY_FILE: &str = "library.db";

/// How long `SQLite` waits on its own locks before reporting busy. With the lock file there is
/// one writer, so this only covers readers that open the file directly (diagnostics, backups).
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Pause between attempts to take the lock in [`Store::open_with_timeout`].
const LOCK_RETRY: Duration = Duration::from_millis(20);

/// Whether list and get operations include soft-deleted rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListOptions {
    /// Include rows whose `deleted_at` is set.
    pub include_deleted: bool,
}

impl ListOptions {
    /// Live rows only (the default).
    pub const LIVE: Self = Self {
        include_deleted: false,
    };
    /// Live and soft-deleted rows.
    pub const ALL: Self = Self {
        include_deleted: true,
    };
}

/// Everything the engine needs to build a snapshot: all live folders, snippets, tags and the
/// links between live snippets and live tags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    /// Live folders.
    pub folders: Vec<Folder>,
    /// Live snippets.
    pub snippets: Vec<Snippet>,
    /// Live tags.
    pub tags: Vec<Tag>,
    /// Live links whose snippet and tag are both live.
    pub snippet_tags: Vec<SnippetTag>,
}

/// What a local change did to a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboxOp {
    /// Inserted or updated.
    Put,
    /// Soft-deleted.
    Delete,
}

impl OutboxOp {
    /// Text stored in `outbox.op`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Delete => "delete",
        }
    }

    /// Parses [`as_str`](Self::as_str) output.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "put" => Some(Self::Put),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// A pending local change for the sync agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    /// Position in the outbox; strictly increasing, never reused.
    pub seq: i64,
    /// Which table changed.
    pub entity: EntityKind,
    /// Which row changed.
    pub row_id: Id,
    /// What happened to it.
    pub op: OutboxOp,
    /// The server revision the row had before the change (0 if never synced).
    pub base_rev: u64,
    /// When the change was made.
    pub created_at: Timestamp,
}

/// Device-local usage statistics of one snippet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// The snippet.
    pub snippet_id: Id,
    /// Number of expansions.
    pub count: u64,
    /// Total characters saved.
    pub chars_saved: u64,
    /// Time of the last expansion.
    pub last_used: Option<Timestamp>,
}

/// An app in which Iamb does not expand or buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppExclusion {
    /// Platform app identifier (bundle id on macOS, executable name on Windows).
    pub app_id: String,
    /// Name shown to the user.
    pub display_name: String,
    /// When the exclusion was added.
    pub created_at: Timestamp,
}

/// A row received from the server, for [`Store::apply_remote`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
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

impl Record {
    /// The kind of row.
    #[must_use]
    pub const fn kind(&self) -> EntityKind {
        match self {
            Self::Profile(_) => EntityKind::Profile,
            Self::Membership(_) => EntityKind::Membership,
            Self::Folder(_) => EntityKind::Folder,
            Self::FolderShare(_) => EntityKind::FolderShare,
            Self::Snippet(_) => EntityKind::Snippet,
            Self::Tag(_) => EntityKind::Tag,
            Self::SnippetTag(_) => EntityKind::SnippetTag,
            Self::Attachment(_) => EntityKind::Attachment,
            Self::SnippetAttachment(_) => EntityKind::SnippetAttachment,
        }
    }

    /// The row's sync fields.
    #[must_use]
    pub fn meta(&self) -> &SyncMeta {
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

    fn write_remote(&self, conn: &Connection) -> Result<(), StoreError> {
        match self {
            Self::Profile(v) => ops::put_remote(conn, v),
            Self::Membership(v) => ops::put_remote(conn, v),
            Self::Folder(v) => ops::put_remote(conn, v),
            Self::FolderShare(v) => ops::put_remote(conn, v),
            Self::Snippet(v) => ops::put_remote(conn, v),
            Self::Tag(v) => ops::put_remote(conn, v),
            Self::SnippetTag(v) => ops::put_remote(conn, v),
            Self::Attachment(v) => ops::put_remote(conn, v),
            Self::SnippetAttachment(v) => ops::put_remote(conn, v),
        }
    }
}

/// Per-entity named wrappers around the generic `put`, `get`, `list` and `delete`.
macro_rules! named_entity_methods {
    ($($ty:ident, $what:literal, $put:ident, $get:ident, $list:ident, $delete:ident;)*) => {
        $(
            #[doc = concat!("Inserts or updates ", $what, "; see [`put`](Self::put).")]
            ///
            /// # Errors
            ///
            /// See [`put`](Self::put).
            pub fn $put(&mut self, value: &$ty, now: Timestamp) -> Result<SyncMeta, StoreError> {
                self.put(value, now)
            }

            #[doc = concat!("Reads ", $what, " by id; `None` if absent or soft-deleted.")]
            ///
            /// # Errors
            ///
            /// `SQLite` or decoding errors.
            pub fn $get(&self, id: Id) -> Result<Option<$ty>, StoreError> {
                self.get(id)
            }

            #[doc = concat!("Lists every row of this kind (", $what, "), live only unless `opts` says otherwise.")]
            ///
            /// # Errors
            ///
            /// `SQLite` or decoding errors.
            pub fn $list(&self, opts: ListOptions) -> Result<Vec<$ty>, StoreError> {
                self.list(opts)
            }

            #[doc = concat!("Soft-deletes ", $what, "; see [`delete`](Self::delete).")]
            ///
            /// # Errors
            ///
            /// See [`delete`](Self::delete).
            pub fn $delete(&mut self, id: Id, now: Timestamp) -> Result<(), StoreError> {
                self.delete::<$ty>(id, now)
            }
        )*
    };
}

macro_rules! all_named_entity_methods {
    () => {
        named_entity_methods! {
            Profile, "a profile", put_profile, get_profile, list_profiles, delete_profile;
            Membership, "a membership", put_membership, get_membership, list_memberships, delete_membership;
            Folder, "a folder", put_folder, get_folder, list_folders, delete_folder;
            FolderShare, "a folder share", put_folder_share, get_folder_share, list_folder_shares, delete_folder_share;
            Snippet, "a snippet", put_snippet, get_snippet, list_snippets, delete_snippet;
            Tag, "a tag", put_tag, get_tag, list_tags, delete_tag;
            SnippetTag, "a snippet–tag link", put_snippet_tag, get_snippet_tag, list_snippet_tags, delete_snippet_tag;
            Attachment, "attachment metadata", put_attachment, get_attachment, list_attachments, delete_attachment;
            SnippetAttachment, "a snippet–attachment link", put_snippet_attachment, get_snippet_attachment,
                list_snippet_attachments, delete_snippet_attachment;
        }
    };
}

/// The open library. Holds `run/store.lock` until dropped.
#[derive(Debug)]
pub struct Store {
    // Field order matters: the connection closes before the lock is released.
    conn: Connection,
    lock: Option<File>,
    /// The data directory (`None` in memory): attachments live under it.
    data_dir: Option<PathBuf>,
}

impl Store {
    /// Opens (creating if needed) the store in `data_dir`, failing at once with
    /// [`StoreError::Locked`] if another holder has `run/store.lock`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Locked`], [`StoreError::SchemaTooNew`], I/O and `SQLite` errors.
    pub fn open(data_dir: &Path) -> Result<Self, StoreError> {
        Self::open_with_timeout(data_dir, Duration::ZERO)
    }

    /// Like [`open`](Self::open), but keeps trying to take the lock for up to `timeout`
    /// (architecture §6.3: the worker waits 5 s, the command-line tool 3 s).
    ///
    /// Creates `data_dir` and `data_dir/run/` if missing, opens `library.db` in WAL mode with
    /// foreign keys on, and applies pending migrations. On Unix, directories it creates get mode
    /// `0700` and `library.db` (so also its WAL and shared-memory files) and the lock file mode
    /// `0600`; an existing data directory, `run/`, database, WAL, shared-memory and lock file
    /// that is more permissive is tightened to the same modes, so other users of the machine cannot
    /// read the library (architecture §8). On
    /// Windows the default per-user ACL of the parent applies.
    ///
    /// # Errors
    ///
    /// [`StoreError::Locked`] if the lock is still held when `timeout` runs out;
    /// [`StoreError::SchemaTooNew`]; I/O and `SQLite` errors.
    pub fn open_with_timeout(data_dir: &Path, timeout: Duration) -> Result<Self, StoreError> {
        let run = data_dir.join("run");
        create_private_dirs(&run)?;
        tighten(data_dir, 0o700)?;
        tighten(&run, 0o700)?;
        let lock_path = run.join("store.lock");
        let lock = take_lock(&lock_path, timeout)?;
        tighten(&lock_path, 0o600)?;
        let db_path = data_dir.join(LIBRARY_FILE);
        // Create the file first so it gets mode 0600 on Unix; SQLite gives its `-wal` and
        // `-shm` files the mode of the database file.
        create_private_file(&db_path)?;
        tighten(&db_path, 0o600)?;
        let conn = Connection::open(&db_path)?;
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::InvalidData(format!(
                "could not enable WAL (journal mode is {mode})"
            )));
        }
        // Files left by an earlier build or another tool: the WAL and shared-memory files only
        // exist once the database is open, and keep whatever mode they were created with.
        for suffix in ["-wal", "-shm"] {
            let mut name = db_path.clone().into_os_string();
            name.push(suffix);
            tighten(Path::new(&name), 0o600)?;
        }
        Self::finish(conn, Some(lock), Some(data_dir.to_path_buf()))
    }

    /// An empty store in memory, with no lock file. For tests.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::finish(Connection::open_in_memory()?, None, None)
    }

    fn finish(
        mut conn: Connection,
        lock: Option<File>,
        data_dir: Option<PathBuf>,
    ) -> Result<Self, StoreError> {
        // synchronous = FULL rather than NORMAL. In WAL mode NORMAL is safe against a killed
        // process (a commit is in the OS once the WAL write returns) and never corrupts the
        // file, but on power loss or an OS crash it can roll back transactions that had already
        // committed: the WAL is only fsynced at checkpoints. Those are edits the UI showed as
        // saved, with their outbox entries, so they would be lost silently, against ADR-0016.
        // FULL fsyncs the WAL at every commit. Writes are human-paced (an edit, or one usage
        // update per expansion, on the store thread and off the typing path), so the extra
        // fsync costs nothing the user can notice; reads, including `load_library`, are
        // unaffected.
        //
        // On macOS a plain fsync does not make the drive flush its cache, so FULL alone does
        // not survive power loss there. `fullfsync` and `checkpoint_fullfsync` make SQLite use
        // `F_FULLFSYNC` for commits and checkpoints. The SQLite documentation says both pragmas
        // only have an effect on macOS, so they are set on every platform.
        conn.pragma_update(None, "fullfsync", "ON")?;
        conn.pragma_update(None, "checkpoint_fullfsync", "ON")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.set_prepared_statement_cache_capacity(64);
        migrations::migrate_to(&mut conn, migrations::LATEST_VERSION)?;
        Ok(Self {
            conn,
            lock,
            data_dir,
        })
    }

    /// Whether this store holds the lock file (false for [`open_in_memory`](Self::open_in_memory)).
    #[must_use]
    pub const fn holds_lock(&self) -> bool {
        self.lock.is_some()
    }

    /// The schema version of the open database.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn schema_version(&self) -> Result<u32, StoreError> {
        migrations::current_version(&self.conn)
    }

    /// Runs `PRAGMA integrity_check`; a healthy database returns `["ok"]`.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn integrity_check(&self) -> Result<Vec<String>, StoreError> {
        let mut stmt = self.conn.prepare("PRAGMA integrity_check")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<String>, _>>()?)
    }

    /// Runs `f` inside one write transaction, committed if `f` returns `Ok` and rolled back
    /// otherwise. Use it to group several changes (an import, a bulk edit).
    ///
    /// # Errors
    ///
    /// Whatever `f` returns, and `SQLite` errors from beginning or committing.
    pub fn write<T>(
        &mut self,
        f: impl FnOnce(&mut WriteTx<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut w = WriteTx { tx };
        let out = f(&mut w)?;
        w.tx.commit()?;
        Ok(out)
    }

    /// Inserts or updates a row by id, in its own transaction, and writes one outbox entry
    /// `put` with the row's previous `rev` (0 for a new row) as base revision.
    ///
    /// On insert `created_at` and `updated_at` become `now` and `rev` 0; on update
    /// `created_at` and `rev` are kept and `updated_at` becomes `now`. `updated_by` is taken from
    /// `value`. The row becomes live (a put on a soft-deleted row restores it). The stored
    /// metadata is returned; the other fields of `value.meta()` are ignored.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] if a row it refers to is absent or deleted;
    /// [`StoreError::AbbreviationConflict`]; [`StoreError::FolderTooDeep`];
    /// [`StoreError::InvalidData`] (a folder moved under itself or into another profile's
    /// folder, a value out of range); `SQLite` errors.
    pub fn put<E: Entity>(&mut self, value: &E, now: Timestamp) -> Result<SyncMeta, StoreError> {
        self.write(|w| w.put(value, now))
    }

    /// Reads a live row by id.
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn get<E: Entity>(&self, id: Id) -> Result<Option<E>, StoreError> {
        ops::get(&self.conn, id, ListOptions::LIVE)
    }

    /// Reads a row by id whether live or soft-deleted.
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn get_any<E: Entity>(&self, id: Id) -> Result<Option<E>, StoreError> {
        ops::get(&self.conn, id, ListOptions::ALL)
    }

    /// Lists all rows of one kind.
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn list<E: Entity>(&self, opts: ListOptions) -> Result<Vec<E>, StoreError> {
        ops::list(&self.conn, opts)
    }

    /// Soft-deletes a row (sets `deleted_at` and `updated_at` to `now`) and writes an outbox
    /// entry `delete`. Deleting a folder also soft-deletes all its sub-folders and their
    /// snippets in the same transaction, each with its own outbox entry. Deleting a row that is
    /// already deleted does nothing.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] if no row has this id; `SQLite` errors.
    pub fn delete<E: Entity>(&mut self, id: Id, now: Timestamp) -> Result<(), StoreError> {
        self.write(|w| w.delete::<E>(id, now))
    }

    all_named_entity_methods!();

    /// Hard-deletes rows soft-deleted before `cutoff`, with their links, usage and approvals.
    /// A deleted row that a surviving row still refers to is kept for a later purge. No outbox
    /// entries. Returns the number of rows removed (not counting cascaded link rows).
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn purge_deleted_before(&mut self, cutoff: Timestamp) -> Result<usize, StoreError> {
        self.write(|w| ops::purge_deleted_before(&w.tx, cutoff))
    }

    /// All live folders, snippets, tags and snippet–tag links, read in one transaction.
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn load_library(&self) -> Result<Library, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let lib = ops::load_library(&tx)?;
        tx.commit()?;
        Ok(lib)
    }

    // ---- sync -----------------------------------------------------------------------------

    /// Writes rows received from the server exactly as given (their `rev`, times and
    /// `deleted_at`), in one transaction, without outbox entries.
    ///
    /// Rows are applied in dependency order (profiles first, links last), and foreign keys are
    /// checked at commit, so a batch may carry a row before the row it refers to. A snippet's
    /// folder must be in the database or earlier in the batch. Folder depth is not checked: the
    /// server is the authority for remote rows.
    ///
    /// # Errors
    ///
    /// [`StoreError::AbbreviationConflict`] if a live remote snippet collides with a live local
    /// one; [`StoreError::NotFound`] for a snippet whose folder is unknown; `SQLite` errors
    /// (including foreign keys still broken at commit). Nothing is written on error.
    pub fn apply_remote(&mut self, rows: &[Record]) -> Result<(), StoreError> {
        let mut ordered: Vec<&Record> = rows.iter().collect();
        ordered.sort_by_key(|r| r.kind().rank());
        self.write(|w| {
            w.tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
            for r in ordered {
                r.write_remote(&w.tx)?;
            }
            Ok(())
        })
    }

    /// Up to `limit` pending outbox entries, oldest first.
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn outbox_read(&self, limit: usize) -> Result<Vec<OutboxEntry>, StoreError> {
        ops::outbox_read(&self.conn, limit)
    }

    /// Removes acknowledged outbox entries; unknown sequence numbers are ignored. Returns how
    /// many entries were removed.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn outbox_ack(&mut self, seqs: &[i64]) -> Result<usize, StoreError> {
        self.write(|w| {
            let mut stmt = w.tx.prepare_cached("DELETE FROM outbox WHERE seq = ?1")?;
            let mut n = 0;
            for seq in seqs {
                n += stmt.execute([seq])?;
            }
            Ok(n)
        })
    }

    /// Number of pending outbox entries.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn outbox_len(&self) -> Result<u64, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM outbox", [], |r| u64_at(r, 0))?)
    }

    /// The user's position in the server's change feed, if any.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn sync_position(&self) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT position FROM sync_state WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Records the user's position in the server's change feed.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn put_sync_position(&mut self, position: &str, now: Timestamp) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO sync_state (id, position, updated_at) VALUES (1, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET position = excluded.position, updated_at = excluded.updated_at",
            params![position, now.0],
        )?;
        Ok(())
    }

    /// The plan features last reported by the sync agent.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn entitlements(&self) -> Result<BTreeMap<String, String>, StoreError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT feature, value FROM entitlement")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<BTreeMap<_, _>, _>>()?)
    }

    /// Replaces the stored plan features.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn put_entitlements(
        &mut self,
        features: &BTreeMap<String, String>,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        self.write(|w| {
            w.tx.execute("DELETE FROM entitlement", [])?;
            let mut stmt = w.tx.prepare_cached(
                "INSERT INTO entitlement (feature, value, updated_at) VALUES (?1, ?2, ?3)",
            )?;
            for (k, v) in features {
                stmt.execute(params![k, v, now.0])?;
            }
            Ok(())
        })
    }

    // ---- device-local ---------------------------------------------------------------------

    /// Records one expansion of a snippet: count + 1, `chars_saved` added, last used = `at`
    /// (never moved backwards). Touches neither the snippet row nor the outbox.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] if the snippet is absent or deleted; `SQLite` errors.
    pub fn record_expansion(
        &mut self,
        snippet_id: Id,
        chars_saved: u64,
        at: Timestamp,
    ) -> Result<(), StoreError> {
        let chars = u64_int(chars_saved, "chars_saved")?;
        self.write(|w| {
            entity::require_live(&w.tx, EntityKind::Snippet, snippet_id)?;
            w.tx.prepare_cached(
                "INSERT INTO usage (snippet_id, count, chars_saved, last_used_at) VALUES (?1, 1, ?2, ?3)
                 ON CONFLICT (snippet_id) DO UPDATE SET
                     count = count + 1,
                     chars_saved = chars_saved + excluded.chars_saved,
                     last_used_at = max(coalesce(last_used_at, excluded.last_used_at), excluded.last_used_at)",
            )?
            .execute(params![snippet_id.as_bytes(), chars, at.0])?;
            Ok(())
        })
    }

    /// Usage statistics of one snippet; `None` if it was never expanded.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn usage(&self, snippet_id: Id) -> Result<Option<Usage>, StoreError> {
        Ok(self
            .conn
            .prepare_cached("SELECT snippet_id, count, chars_saved, last_used_at FROM usage WHERE snippet_id = ?1")?
            .query_row([snippet_id.as_bytes()], usage_row)
            .optional()?)
    }

    /// Forgets every snippet's usage (Settings → Privacy) and remembers when, so that a snippet
    /// is not called unused for the time before the counts began. Returns how many snippets had
    /// usage. Touches neither the snippets nor the outbox.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn reset_usage(&mut self, at: Timestamp) -> Result<u64, StoreError> {
        self.write(|w| {
            let n = w.tx.execute("DELETE FROM usage", [])?;
            w.tx.execute(
                "INSERT INTO setting (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![USAGE_RESET_KEY, at.0.to_string()],
            )?;
            Ok(u64::try_from(n).unwrap_or(u64::MAX))
        })
    }

    /// When the usage counts were last reset on this computer; `None` if never.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn usage_reset_at(&self) -> Result<Option<Timestamp>, StoreError> {
        Ok(self
            .get_setting(USAGE_RESET_KEY)?
            .and_then(|v| v.parse().ok())
            .map(Timestamp))
    }

    /// The snippets used most recently, newest first, at most `limit`.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn recent_usage(&self, limit: usize) -> Result<Vec<Usage>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT u.snippet_id, u.count, u.chars_saved, u.last_used_at FROM usage u
             JOIN snippet s ON s.id = u.snippet_id
             WHERE u.last_used_at IS NOT NULL AND s.deleted_at IS NULL
             ORDER BY u.last_used_at DESC, u.snippet_id LIMIT ?1",
        )?;
        let rows = stmt.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], usage_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Usage statistics of every snippet that was expanded at least once.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn list_usage(&self) -> Result<Vec<Usage>, StoreError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT snippet_id, count, chars_saved, last_used_at FROM usage")?;
        let rows = stmt.query_map([], usage_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The value of a setting.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM setting WHERE key = ?1")?
            .query_row([key], |r| r.get(0))
            .optional()?)
    }

    /// Sets a setting, replacing any previous value.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn put_setting(&mut self, key: &str, value: &str) -> Result<(), StoreError> {
        self.conn
            .prepare_cached(
                "INSERT INTO setting (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )?
            .execute([key, value])?;
        Ok(())
    }

    /// Removes a setting; returns whether it existed.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn delete_setting(&mut self, key: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn
            .execute("DELETE FROM setting WHERE key = ?1", [key])?
            > 0)
    }

    /// All settings, by key.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn list_settings(&self) -> Result<BTreeMap<String, String>, StoreError> {
        let mut stmt = self.conn.prepare_cached("SELECT key, value FROM setting")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<BTreeMap<_, _>, _>>()?)
    }

    /// Adds or updates an app exclusion (by `app_id`).
    ///
    /// # Errors
    ///
    /// [`StoreError::InvalidData`] for an empty `app_id`; `SQLite` errors.
    pub fn put_app_exclusion(&mut self, exclusion: &AppExclusion) -> Result<(), StoreError> {
        if exclusion.app_id.is_empty() {
            return Err(StoreError::InvalidData(
                "app exclusion needs an app id".to_owned(),
            ));
        }
        self.conn
            .prepare_cached(
                "INSERT INTO app_exclusion (app_id, display_name, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (app_id) DO UPDATE SET display_name = excluded.display_name",
            )?
            .execute(params![
                exclusion.app_id,
                exclusion.display_name,
                exclusion.created_at.0
            ])?;
        Ok(())
    }

    /// One app exclusion.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn get_app_exclusion(&self, app_id: &str) -> Result<Option<AppExclusion>, StoreError> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT app_id, display_name, created_at FROM app_exclusion WHERE app_id = ?1",
            )?
            .query_row([app_id], exclusion_row)
            .optional()?)
    }

    /// All app exclusions, ordered by app id.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn list_app_exclusions(&self) -> Result<Vec<AppExclusion>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT app_id, display_name, created_at FROM app_exclusion ORDER BY app_id",
        )?;
        let rows = stmt.query_map([], exclusion_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Removes an app exclusion; returns whether it existed.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn delete_app_exclusion(&mut self, app_id: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn
            .execute("DELETE FROM app_exclusion WHERE app_id = ?1", [app_id])?
            > 0)
    }

    /// Records that the user approved revision `rev` of a snippet written by someone else
    /// (architecture §8).
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] if the snippet is absent or deleted; `SQLite` errors.
    pub fn put_approval(
        &mut self,
        snippet_id: Id,
        rev: u64,
        at: Timestamp,
    ) -> Result<(), StoreError> {
        let rev = u64_int(rev, "rev")?;
        self.write(|w| {
            entity::require_live(&w.tx, EntityKind::Snippet, snippet_id)?;
            w.tx.execute(
                "INSERT INTO approval (snippet_id, approved_rev, approved_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (snippet_id) DO UPDATE SET
                     approved_rev = excluded.approved_rev, approved_at = excluded.approved_at",
                params![snippet_id.as_bytes(), rev, at.0],
            )?;
            Ok(())
        })
    }

    /// The approved revision of a snippet, if any.
    ///
    /// # Errors
    ///
    /// `SQLite` errors.
    pub fn approved_rev(&self, snippet_id: Id) -> Result<Option<u64>, StoreError> {
        Ok(self
            .conn
            .prepare_cached("SELECT approved_rev FROM approval WHERE snippet_id = ?1")?
            .query_row([snippet_id.as_bytes()], |r| u64_at(r, 0))
            .optional()?)
    }
}

fn usage_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Usage> {
    Ok(Usage {
        snippet_id: id_at(r, 0)?,
        count: u64_at(r, 1)?,
        chars_saved: u64_at(r, 2)?,
        last_used: opt_ts_at(r, 3)?,
    })
}

fn exclusion_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AppExclusion> {
    Ok(AppExclusion {
        app_id: r.get(0)?,
        display_name: r.get(1)?,
        created_at: ts_at(r, 2)?,
    })
}

/// Opens (creating) the lock file and takes an exclusive lock on it, retrying until `timeout`.
/// Creates `path` and missing parents. On Unix the new directories get mode `0700`.
fn create_private_dirs(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Makes sure the file exists. On Unix a new file gets mode `0600`; an existing one is kept.
fn create_private_file(path: &Path) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map(drop)
}

/// On Unix, sets `path` to `mode` if group or others have any permission on it. A missing path
/// is ignored. Does nothing on other platforms.
#[cfg(unix)]
fn tighten(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(path) {
        Ok(meta) if meta.permissions().mode() & 0o077 != 0 => {
            fs::set_permissions(path, fs::Permissions::from_mode(mode))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn tighten(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

fn take_lock(path: &Path, timeout: Duration) -> Result<File, StoreError> {
    let mut file = OpenOptions::new();
    file.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        file.mode(0o600);
    }
    let file = file.open(path)?;
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {
                let now = Instant::now();
                if now >= deadline {
                    return Err(StoreError::Locked {
                        path: PathBuf::from(path),
                    });
                }
                thread::sleep(LOCK_RETRY.min(deadline - now));
            }
            Err(TryLockError::Error(e)) => return Err(StoreError::Io(e)),
        }
    }
}

/// A write transaction opened by [`Store::write`]. Every change made through it commits or
/// rolls back together, outbox entries included.
#[derive(Debug)]
pub struct WriteTx<'a> {
    tx: Transaction<'a>,
}

impl WriteTx<'_> {
    /// Inserts or updates a row; see [`Store::put`].
    ///
    /// # Errors
    ///
    /// See [`Store::put`].
    pub fn put<E: Entity>(&mut self, value: &E, now: Timestamp) -> Result<SyncMeta, StoreError> {
        ops::put_local(&self.tx, value, now)
    }

    /// Reads a live row by id, seeing this transaction's changes.
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn get<E: Entity>(&self, id: Id) -> Result<Option<E>, StoreError> {
        ops::get(&self.tx, id, ListOptions::LIVE)
    }

    /// Lists rows of one kind, seeing this transaction's changes.
    ///
    /// # Errors
    ///
    /// `SQLite` or decoding errors.
    pub fn list<E: Entity>(&self, opts: ListOptions) -> Result<Vec<E>, StoreError> {
        ops::list(&self.tx, opts)
    }

    /// Soft-deletes a row; see [`Store::delete`].
    ///
    /// # Errors
    ///
    /// See [`Store::delete`].
    pub fn delete<E: Entity>(&mut self, id: Id, now: Timestamp) -> Result<(), StoreError> {
        ops::delete_local::<E>(&self.tx, id, now)
    }

    all_named_entity_methods!();
}

#[cfg(test)]
mod tests {
    use super::Store;

    /// Review 1.9 row 17: the durability pragmas that make `SQLite` use `F_FULLFSYNC` on macOS
    /// read back as 1 after [`Store::open`], and `synchronous` is FULL (2). They are per
    /// connection, so they are read through the store's own connection.
    #[test]
    fn durability_pragmas_are_on_after_open() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let pragma = |name: &str| -> rusqlite::Result<i64> {
            store
                .conn
                .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
        };
        assert_eq!(pragma("fullfsync")?, 1, "fullfsync");
        assert_eq!(pragma("checkpoint_fullfsync")?, 1, "checkpoint_fullfsync");
        assert_eq!(pragma("synchronous")?, 2, "synchronous = FULL");
        assert_eq!(pragma("foreign_keys")?, 1, "foreign_keys");
        Ok(())
    }
}
