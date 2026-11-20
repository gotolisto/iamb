//! The store's error type.

use std::path::PathBuf;

use iamb_core::model::Id;

use crate::entity::EntityKind;

/// Everything that can go wrong in the store.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// Another process (or another `Store` in this process) holds `run/store.lock`.
    #[error("the store is locked by another process ({})", path.display())]
    Locked {
        /// The lock file.
        path: PathBuf,
    },
    /// The database was written by a newer version of Iamb.
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    SchemaTooNew {
        /// Highest version recorded in `schema_migrations`.
        found: u32,
        /// Highest version this build knows.
        supported: u32,
    },
    /// A row that the operation needs does not exist or is soft-deleted.
    #[error("{entity} {id} not found")]
    NotFound {
        /// Kind of the missing row.
        entity: EntityKind,
        /// Its id.
        id: Id,
    },
    /// Another live snippet in the same profile has the same case-folded abbreviation.
    #[error("abbreviation {key:?} is already used by snippet {existing}")]
    AbbreviationConflict {
        /// The case-folded abbreviation.
        key: String,
        /// The snippet that holds it.
        existing: Id,
    },
    /// Inserting or moving the folder would nest folders deeper than
    /// [`MAX_FOLDER_DEPTH`](crate::MAX_FOLDER_DEPTH) levels.
    #[error("folder {folder} would be nested {depth} levels deep (at most {max})", max = crate::MAX_FOLDER_DEPTH)]
    FolderTooDeep {
        /// The folder being written.
        folder: Id,
        /// The depth of the deepest folder in its subtree after the write.
        depth: usize,
    },
    /// A value breaks a rule of the data model or cannot be decoded from the database.
    #[error("invalid data: {0}")]
    InvalidData(String),
    /// Bytes refused as an attachment, or no place to keep them.
    #[error("attachment refused: {0}")]
    Attachment(#[from] crate::attachments::AttachmentError),
    /// File-system error while creating the data directory or the lock file.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// Error reported by `SQLite`.
    #[error("sqlite error: {0}")]
    Sqlite(rusqlite::Error),
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::FromSqlConversionFailure(column, ty, source) => {
                Self::InvalidData(format!("column {column} ({ty}): {source}"))
            }
            rusqlite::Error::ToSqlConversionFailure(source) => {
                Self::InvalidData(source.to_string())
            }
            other => Self::Sqlite(other),
        }
    }
}
