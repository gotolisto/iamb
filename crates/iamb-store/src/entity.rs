//! The synced entities and how each maps onto its table.

use std::fmt;

use iamb_core::model::{
    Abbreviation, Attachment, Folder, FolderDefaults, FolderShare, Membership, Profile, Sha256,
    Snippet, SnippetAttachment, SnippetTag, SyncMeta, Tag,
};
use rusqlite::types::{Type, Value};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::MAX_FOLDER_DEPTH;
use crate::codec::{
    bool_val, decode_err, enum_at, enum_val, id_at, id_val, int_val, meta_at, opt_enum_at,
    opt_enum_val, opt_id_at, opt_id_val, opt_text_val, text_val, u64_at, u64_val,
};
use crate::error::StoreError;

/// [`MAX_FOLDER_DEPTH`] as an SQL integer (bounds the subtree walk).
const MAX_DEPTH_SQL: i64 = 4;

/// The kind of a synced row, as written in `outbox.entity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntityKind {
    /// [`Profile`].
    Profile,
    /// [`Membership`].
    Membership,
    /// [`Folder`].
    Folder,
    /// [`FolderShare`].
    FolderShare,
    /// [`Snippet`].
    Snippet,
    /// [`Tag`].
    Tag,
    /// [`SnippetTag`].
    SnippetTag,
    /// [`Attachment`].
    Attachment,
    /// [`SnippetAttachment`].
    SnippetAttachment,
}

impl EntityKind {
    /// Every kind, in dependency order (a row only refers to kinds before it).
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

    /// The table name, which is also the text stored in `outbox.entity`.
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

    /// Position in [`ALL`](Self::ALL); used to order remote batches.
    pub(crate) fn rank(self) -> usize {
        Self::ALL
            .iter()
            .position(|k| *k == self)
            .unwrap_or(usize::MAX)
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a row is being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// A local edit: full validation, outbox entry.
    Local,
    /// A row from the server: written as given; references may arrive later in the same batch.
    Remote,
}

/// A synced entity the store can put, get, list and delete.
///
/// Implemented for every entity in `iamb_core::model` that has [`SyncMeta`]; sealed.
pub trait Entity: sealed::Row + Clone {
    /// Which kind of row this is.
    const KIND: EntityKind;
    /// The common sync fields.
    fn meta(&self) -> &SyncMeta;
}

pub(crate) mod sealed {
    use iamb_core::model::Id;
    use rusqlite::types::Value;
    use rusqlite::{Connection, Row as SqlRow};

    use super::WriteMode;
    use crate::error::StoreError;

    /// Table mapping of an entity. Not public API.
    pub trait Row: Sized {
        /// Table name.
        const TABLE: &'static str;
        /// Columns read after the six common ones, in `from_row` order.
        const READ_COLUMNS: &'static [&'static str];
        /// Columns written after the six common ones, in `write_values` order.
        const WRITE_COLUMNS: &'static [&'static str];
        /// Decodes a row selected as common columns followed by `READ_COLUMNS`.
        fn from_row(row: &SqlRow<'_>) -> rusqlite::Result<Self>;
        /// Row id.
        fn row_id(&self) -> Id;
        /// Validates the value against the database and returns the `WRITE_COLUMNS` values.
        fn write_values(
            &self,
            conn: &Connection,
            mode: WriteMode,
            live: bool,
        ) -> Result<Vec<Value>, StoreError>;
        /// Keeps store-maintained copies in other tables in step after a write.
        fn after_write(&self, _conn: &Connection) -> Result<(), StoreError> {
            Ok(())
        }
    }
}

/// Fails with `NotFound` unless `id` is a live row of `kind`.
pub(crate) fn require_live(
    conn: &Connection,
    kind: EntityKind,
    id: iamb_core::model::Id,
) -> Result<(), StoreError> {
    let sql = format!("SELECT deleted_at FROM {} WHERE id = ?1", kind.as_str());
    let found: Option<Option<i64>> = conn
        .prepare_cached(&sql)?
        .query_row([id.as_bytes()], |r| r.get(0))
        .optional()?;
    match found {
        Some(None) => Ok(()),
        _ => Err(StoreError::NotFound { entity: kind, id }),
    }
}

macro_rules! entity_impl {
    ($ty:ty, $kind:ident) => {
        impl Entity for $ty {
            const KIND: EntityKind = EntityKind::$kind;
            fn meta(&self) -> &SyncMeta {
                &self.meta
            }
        }
    };
}

entity_impl!(Profile, Profile);
entity_impl!(Membership, Membership);
entity_impl!(Folder, Folder);
entity_impl!(FolderShare, FolderShare);
entity_impl!(Snippet, Snippet);
entity_impl!(Tag, Tag);
entity_impl!(SnippetTag, SnippetTag);
entity_impl!(Attachment, Attachment);
entity_impl!(SnippetAttachment, SnippetAttachment);

// ---- profile --------------------------------------------------------------------------------

impl sealed::Row for Profile {
    const TABLE: &'static str = "profile";
    const READ_COLUMNS: &'static [&'static str] = &["kind", "name", "owner_user_id"];
    const WRITE_COLUMNS: &'static [&'static str] = Self::READ_COLUMNS;

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            meta: meta_at(row)?,
            kind: enum_at(row, 6)?,
            name: row.get(7)?,
            owner_user_id: id_at(row, 8)?,
        })
    }

    fn row_id(&self) -> iamb_core::model::Id {
        self.meta.id
    }

    fn write_values(
        &self,
        _: &Connection,
        _: WriteMode,
        _: bool,
    ) -> Result<Vec<Value>, StoreError> {
        Ok(vec![
            enum_val(self.kind),
            text_val(&self.name),
            id_val(self.owner_user_id),
        ])
    }
}

// ---- membership -----------------------------------------------------------------------------

impl sealed::Row for Membership {
    const TABLE: &'static str = "membership";
    const READ_COLUMNS: &'static [&'static str] = &["profile_id", "user_id", "role"];
    const WRITE_COLUMNS: &'static [&'static str] = Self::READ_COLUMNS;

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            meta: meta_at(row)?,
            profile_id: id_at(row, 6)?,
            user_id: id_at(row, 7)?,
            role: enum_at(row, 8)?,
        })
    }

    fn row_id(&self) -> iamb_core::model::Id {
        self.meta.id
    }

    fn write_values(
        &self,
        conn: &Connection,
        mode: WriteMode,
        _: bool,
    ) -> Result<Vec<Value>, StoreError> {
        if mode == WriteMode::Local {
            require_live(conn, EntityKind::Profile, self.profile_id)?;
        }
        Ok(vec![
            id_val(self.profile_id),
            id_val(self.user_id),
            enum_val(self.role),
        ])
    }
}

// ---- folder ---------------------------------------------------------------------------------

/// Parent and profile of a folder row, and whether it is live.
fn folder_links(
    conn: &Connection,
    id: iamb_core::model::Id,
) -> Result<Option<(Option<iamb_core::model::Id>, iamb_core::model::Id, bool)>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT parent_id, profile_id, deleted_at IS NULL FROM folder WHERE id = ?1",
        )?
        .query_row([id.as_bytes()], |r| {
            Ok((opt_id_at(r, 0)?, id_at(r, 1)?, r.get::<_, bool>(2)?))
        })
        .optional()?)
}

/// Checks parent, profile, cycles and depth of a folder for a local write.
fn check_placement(folder: &Folder, conn: &Connection) -> Result<(), StoreError> {
    let id = folder.meta.id;
    // Depth of the folder itself: 1 at the top level.
    let mut depth = 1usize;
    if let Some(parent) = folder.parent_id {
        let mut cur = parent;
        let mut first = true;
        loop {
            if cur == id {
                return Err(StoreError::InvalidData(format!(
                    "folder {id} cannot be its own ancestor"
                )));
            }
            let Some((up, profile, live)) = folder_links(conn, cur)? else {
                return Err(StoreError::NotFound {
                    entity: EntityKind::Folder,
                    id: cur,
                });
            };
            if first {
                if !live {
                    return Err(StoreError::NotFound {
                        entity: EntityKind::Folder,
                        id: cur,
                    });
                }
                if profile != folder.profile_id {
                    return Err(StoreError::InvalidData(format!(
                        "folder {id} must belong to the profile of its parent {cur}"
                    )));
                }
                first = false;
            }
            depth += 1;
            match up {
                Some(next) if depth <= MAX_FOLDER_DEPTH + 1 => cur = next,
                _ => break,
            }
        }
    }
    // Height of the folder's existing live subtree (0 when it has no sub-folders).
    let height: i64 = conn
        .prepare_cached(
            "WITH RECURSIVE sub(id, lvl) AS (
                     SELECT ?1, 0
                     UNION ALL
                     SELECT f.id, sub.lvl + 1 FROM folder f JOIN sub ON f.parent_id = sub.id
                     WHERE f.deleted_at IS NULL AND sub.lvl < ?2
                 )
                 SELECT max(lvl) FROM sub",
        )?
        .query_row(params![id.as_bytes(), MAX_DEPTH_SQL], |r| r.get(0))?;
    let total = depth + usize::try_from(height).unwrap_or(0);
    if total > MAX_FOLDER_DEPTH {
        return Err(StoreError::FolderTooDeep {
            folder: id,
            depth: total,
        });
    }
    Ok(())
}

/// When an existing folder changes profile: refuse if it has live sub-folders, and check that
/// its snippets' abbreviations are free in the new profile.
fn check_profile_change(
    folder: &Folder,
    conn: &Connection,
    mode: WriteMode,
) -> Result<(), StoreError> {
    let id = folder.meta.id;
    let Some((_, old_profile, _)) = folder_links(conn, id)? else {
        return Ok(());
    };
    if old_profile == folder.profile_id {
        return Ok(());
    }
    if mode == WriteMode::Local {
        let has_children: bool = conn
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM folder WHERE parent_id = ?1 AND deleted_at IS NULL)",
            )?
            .query_row([id.as_bytes()], |r| r.get(0))?;
        if has_children {
            return Err(StoreError::InvalidData(format!(
                "folder {id} has sub-folders and cannot move to another profile"
            )));
        }
    }
    let clash = conn
        .prepare_cached(
            "SELECT o.abbreviation_key, o.id FROM snippet s
                 JOIN snippet o ON o.profile_id = ?2 AND o.abbreviation_key = s.abbreviation_key
                     AND o.deleted_at IS NULL AND o.id <> s.id
                 WHERE s.folder_id = ?1 AND s.deleted_at IS NULL
                 LIMIT 1",
        )?
        .query_row([id.as_bytes(), folder.profile_id.as_bytes()], |r| {
            Ok((r.get::<_, String>(0)?, id_at(r, 1)?))
        })
        .optional()?;
    if let Some((key, existing)) = clash {
        return Err(StoreError::AbbreviationConflict { key, existing });
    }
    Ok(())
}

impl sealed::Row for Folder {
    const TABLE: &'static str = "folder";
    const READ_COLUMNS: &'static [&'static str] = &[
        "profile_id",
        "parent_id",
        "name",
        "position",
        "member_permission",
        "default_trigger",
        "default_case_mode",
        "abbreviation_prefix",
    ];
    const WRITE_COLUMNS: &'static [&'static str] = Self::READ_COLUMNS;

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            meta: meta_at(row)?,
            profile_id: id_at(row, 6)?,
            parent_id: opt_id_at(row, 7)?,
            name: row.get(8)?,
            position: row.get(9)?,
            member_permission: opt_enum_at(row, 10)?,
            defaults: FolderDefaults {
                trigger: opt_enum_at(row, 11)?,
                case_mode: opt_enum_at(row, 12)?,
                abbreviation_prefix: row.get(13)?,
            },
        })
    }

    fn row_id(&self) -> iamb_core::model::Id {
        self.meta.id
    }

    fn write_values(
        &self,
        conn: &Connection,
        mode: WriteMode,
        _: bool,
    ) -> Result<Vec<Value>, StoreError> {
        if mode == WriteMode::Local {
            require_live(conn, EntityKind::Profile, self.profile_id)?;
            check_placement(self, conn)?;
        }
        check_profile_change(self, conn, mode)?;
        Ok(vec![
            id_val(self.profile_id),
            opt_id_val(self.parent_id),
            text_val(&self.name),
            int_val(self.position),
            opt_enum_val(self.member_permission),
            opt_enum_val(self.defaults.trigger),
            opt_enum_val(self.defaults.case_mode),
            opt_text_val(self.defaults.abbreviation_prefix.as_deref()),
        ])
    }

    fn after_write(&self, conn: &Connection) -> Result<(), StoreError> {
        conn.prepare_cached(
            "UPDATE snippet SET profile_id = ?2 WHERE folder_id = ?1 AND profile_id <> ?2",
        )?
        .execute([self.meta.id.as_bytes(), self.profile_id.as_bytes()])?;
        Ok(())
    }
}

// ---- folder share ---------------------------------------------------------------------------

impl sealed::Row for FolderShare {
    const TABLE: &'static str = "folder_share";
    const READ_COLUMNS: &'static [&'static str] = &["folder_id", "profile_id", "permission"];
    const WRITE_COLUMNS: &'static [&'static str] = Self::READ_COLUMNS;

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            meta: meta_at(row)?,
            folder_id: id_at(row, 6)?,
            profile_id: id_at(row, 7)?,
            permission: enum_at(row, 8)?,
        })
    }

    fn row_id(&self) -> iamb_core::model::Id {
        self.meta.id
    }

    fn write_values(
        &self,
        conn: &Connection,
        mode: WriteMode,
        _: bool,
    ) -> Result<Vec<Value>, StoreError> {
        if mode == WriteMode::Local {
            require_live(conn, EntityKind::Folder, self.folder_id)?;
            require_live(conn, EntityKind::Profile, self.profile_id)?;
        }
        Ok(vec![
            id_val(self.folder_id),
            id_val(self.profile_id),
            enum_val(self.permission),
        ])
    }
}

// ---- snippet --------------------------------------------------------------------------------

impl sealed::Row for Snippet {
    const TABLE: &'static str = "snippet";
    const READ_COLUMNS: &'static [&'static str] = &[
        "folder_id",
        "abbreviation",
        "label",
        "kind",
        "body",
        "trigger_mode",
        "case_mode",
        "word_start",
        "enabled",
    ];
    const WRITE_COLUMNS: &'static [&'static str] = &[
        "folder_id",
        "profile_id",
        "abbreviation",
        "abbreviation_key",
        "label",
        "kind",
        "body",
        "trigger_mode",
        "case_mode",
        "word_start",
        "enabled",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let abbreviation = {
            let s = row.get_ref(7)?.as_str()?;
            Abbreviation::new(s).map_err(|e| decode_err(7, Type::Text, e.to_string()))?
        };
        Ok(Self {
            meta: meta_at(row)?,
            folder_id: id_at(row, 6)?,
            abbreviation,
            label: row.get(8)?,
            kind: enum_at(row, 9)?,
            body: row.get(10)?,
            trigger: enum_at(row, 11)?,
            case_mode: enum_at(row, 12)?,
            word_start: row.get(13)?,
            enabled: row.get(14)?,
        })
    }

    fn row_id(&self) -> iamb_core::model::Id {
        self.meta.id
    }

    fn write_values(
        &self,
        conn: &Connection,
        mode: WriteMode,
        live: bool,
    ) -> Result<Vec<Value>, StoreError> {
        let Some((_, profile_id, folder_live)) = folder_links(conn, self.folder_id)? else {
            return Err(StoreError::NotFound {
                entity: EntityKind::Folder,
                id: self.folder_id,
            });
        };
        if mode == WriteMode::Local && !folder_live {
            return Err(StoreError::NotFound {
                entity: EntityKind::Folder,
                id: self.folder_id,
            });
        }
        // An update that leaves the abbreviation (and the profile) as stored keeps the stored
        // key and skips the conflict check: migration 2 leaves the old key on one row of a pair
        // that now folds alike, and that row must stay editable (label, enabled, ...).
        let unchanged_key: Option<String> = conn
            .prepare_cached(
                "SELECT abbreviation_key FROM snippet
                 WHERE id = ?1 AND deleted_at IS NULL AND profile_id = ?2 AND abbreviation = ?3",
            )?
            .query_row(
                params![
                    self.meta.id.as_bytes(),
                    profile_id.as_bytes(),
                    self.abbreviation.as_str()
                ],
                |r| r.get(0),
            )
            .optional()?;
        let key = unchanged_key
            .clone()
            .unwrap_or_else(|| self.abbreviation.fold_key());
        if live && unchanged_key.is_none() {
            let existing = conn
                .prepare_cached(
                    "SELECT id FROM snippet
                     WHERE profile_id = ?1 AND abbreviation_key = ?2 AND deleted_at IS NULL AND id <> ?3",
                )?
                .query_row(params![profile_id.as_bytes(), key, self.meta.id.as_bytes()], |r| id_at(r, 0))
                .optional()?;
            if let Some(existing) = existing {
                return Err(StoreError::AbbreviationConflict { key, existing });
            }
        }
        Ok(vec![
            id_val(self.folder_id),
            id_val(profile_id),
            text_val(self.abbreviation.as_str()),
            Value::Text(key),
            text_val(&self.label),
            enum_val(self.kind),
            text_val(&self.body),
            enum_val(self.trigger),
            enum_val(self.case_mode),
            bool_val(self.word_start),
            bool_val(self.enabled),
        ])
    }
}

// ---- tag ------------------------------------------------------------------------------------

impl sealed::Row for Tag {
    const TABLE: &'static str = "tag";
    const READ_COLUMNS: &'static [&'static str] = &["profile_id", "name", "color"];
    const WRITE_COLUMNS: &'static [&'static str] = Self::READ_COLUMNS;

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            meta: meta_at(row)?,
            profile_id: id_at(row, 6)?,
            name: row.get(7)?,
            color: row.get(8)?,
        })
    }

    fn row_id(&self) -> iamb_core::model::Id {
        self.meta.id
    }

    fn write_values(
        &self,
        conn: &Connection,
        mode: WriteMode,
        _: bool,
    ) -> Result<Vec<Value>, StoreError> {
        if mode == WriteMode::Local {
            require_live(conn, EntityKind::Profile, self.profile_id)?;
        }
        Ok(vec![
            id_val(self.profile_id),
            text_val(&self.name),
            opt_text_val(self.color.as_deref()),
        ])
    }
}

// ---- snippet tag ----------------------------------------------------------------------------

impl sealed::Row for SnippetTag {
    const TABLE: &'static str = "snippet_tag";
    const READ_COLUMNS: &'static [&'static str] = &["snippet_id", "tag_id"];
    const WRITE_COLUMNS: &'static [&'static str] = Self::READ_COLUMNS;

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            meta: meta_at(row)?,
            snippet_id: id_at(row, 6)?,
            tag_id: id_at(row, 7)?,
        })
    }

    fn row_id(&self) -> iamb_core::model::Id {
        self.meta.id
    }

    fn write_values(
        &self,
        conn: &Connection,
        mode: WriteMode,
        _: bool,
    ) -> Result<Vec<Value>, StoreError> {
        if mode == WriteMode::Local {
            require_live(conn, EntityKind::Snippet, self.snippet_id)?;
            require_live(conn, EntityKind::Tag, self.tag_id)?;
        }
        Ok(vec![id_val(self.snippet_id), id_val(self.tag_id)])
    }
}

// ---- attachment -----------------------------------------------------------------------------

impl sealed::Row for Attachment {
    const TABLE: &'static str = "attachment";
    const READ_COLUMNS: &'static [&'static str] =
        &["profile_id", "sha256", "mime", "size", "file_name"];
    const WRITE_COLUMNS: &'static [&'static str] = Self::READ_COLUMNS;

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let sha256 = {
            let s = row.get_ref(7)?.as_str()?;
            s.parse::<Sha256>()
                .map_err(|e| decode_err(7, Type::Text, e.to_string()))?
        };
        Ok(Self {
            meta: meta_at(row)?,
            profile_id: id_at(row, 6)?,
            sha256,
            mime: row.get(8)?,
            size: u64_at(row, 9)?,
            file_name: row.get(10)?,
        })
    }

    fn row_id(&self) -> iamb_core::model::Id {
        self.meta.id
    }

    fn write_values(
        &self,
        conn: &Connection,
        mode: WriteMode,
        _: bool,
    ) -> Result<Vec<Value>, StoreError> {
        if mode == WriteMode::Local {
            require_live(conn, EntityKind::Profile, self.profile_id)?;
        }
        Ok(vec![
            id_val(self.profile_id),
            Value::Text(self.sha256.to_string()),
            text_val(&self.mime),
            u64_val(self.size, "attachment size")?,
            text_val(&self.file_name),
        ])
    }
}

// ---- snippet attachment ---------------------------------------------------------------------

impl sealed::Row for SnippetAttachment {
    const TABLE: &'static str = "snippet_attachment";
    const READ_COLUMNS: &'static [&'static str] = &["snippet_id", "attachment_id"];
    const WRITE_COLUMNS: &'static [&'static str] = Self::READ_COLUMNS;

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            meta: meta_at(row)?,
            snippet_id: id_at(row, 6)?,
            attachment_id: id_at(row, 7)?,
        })
    }

    fn row_id(&self) -> iamb_core::model::Id {
        self.meta.id
    }

    fn write_values(
        &self,
        conn: &Connection,
        mode: WriteMode,
        _: bool,
    ) -> Result<Vec<Value>, StoreError> {
        if mode == WriteMode::Local {
            require_live(conn, EntityKind::Snippet, self.snippet_id)?;
            require_live(conn, EntityKind::Attachment, self.attachment_id)?;
        }
        Ok(vec![id_val(self.snippet_id), id_val(self.attachment_id)])
    }
}
