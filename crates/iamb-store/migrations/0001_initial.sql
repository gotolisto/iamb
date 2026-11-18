-- Migration 0001: initial schema (task 1.5).
--
-- Conventions (docs/architecture.md §7, ADR-0007, ADR-0014):
--
-- * Ids are UUIDs stored as 16-byte BLOBs (`iamb_core::model::Id::as_bytes`). User ids are kept
--   as BLOBs too but have no foreign key: users live on the server.
-- * Times are INTEGER UTC milliseconds since the Unix epoch.
-- * Every synced table carries the common columns `id`, `created_at`, `updated_at`,
--   `updated_by`, `rev` (assigned by the server, 0 until first sync) and `deleted_at` (soft
--   delete; NULL while the row is live).
-- * Enums are TEXT in the snake_case spelling used by the serde form, guarded by CHECK.
-- * Synced tables are WITHOUT ROWID: the 16-byte id is the clustered key.
-- * Foreign keys are enforced (`PRAGMA foreign_keys = ON`). Link and device-local tables
--   cascade on hard delete so that purging a row also purges what hangs off it.
--
-- Typed text is never stored: there is no table for keystrokes and there must never be one.

-- ---------------------------------------------------------------------------------------------
-- Synced entities
-- ---------------------------------------------------------------------------------------------

-- A profile: the user's personal profile or a group (family, company).
CREATE TABLE profile (
    id            BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    updated_by    BLOB             CHECK (updated_by IS NULL OR length(updated_by) = 16),
    rev           INTEGER NOT NULL DEFAULT 0 CHECK (rev >= 0),
    deleted_at    INTEGER,
    kind          TEXT    NOT NULL CHECK (kind IN ('personal', 'family', 'company')),
    name          TEXT    NOT NULL,
    owner_user_id BLOB    NOT NULL CHECK (length(owner_user_id) = 16)
) WITHOUT ROWID;

-- A user's membership of a group profile.
CREATE TABLE membership (
    id          BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    updated_by  BLOB             CHECK (updated_by IS NULL OR length(updated_by) = 16),
    rev         INTEGER NOT NULL DEFAULT 0 CHECK (rev >= 0),
    deleted_at  INTEGER,
    profile_id  BLOB    NOT NULL REFERENCES profile (id),
    user_id     BLOB    NOT NULL CHECK (length(user_id) = 16),
    role        TEXT    NOT NULL CHECK (role IN ('owner', 'admin', 'member'))
) WITHOUT ROWID;
CREATE INDEX membership_profile ON membership (profile_id);

-- A folder. Belongs to exactly one profile; nests at most 4 levels (enforced by the store on
-- insert and move, because a CHECK cannot see other rows). A sub-folder belongs to the same
-- profile as its parent.
CREATE TABLE folder (
    id                  BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    updated_by          BLOB             CHECK (updated_by IS NULL OR length(updated_by) = 16),
    rev                 INTEGER NOT NULL DEFAULT 0 CHECK (rev >= 0),
    deleted_at          INTEGER,
    profile_id          BLOB    NOT NULL REFERENCES profile (id),
    parent_id           BLOB             REFERENCES folder (id) CHECK (parent_id IS NULL OR parent_id <> id),
    name                TEXT    NOT NULL,
    position            INTEGER NOT NULL,
    -- Permission members get; group-owned folders only.
    member_permission   TEXT             CHECK (member_permission IN ('view', 'edit')),
    -- Folder defaults (`FolderDefaults`) for snippets created in the folder.
    default_trigger     TEXT             CHECK (default_trigger IN ('immediate', 'delimiter')),
    default_case_mode   TEXT             CHECK (default_case_mode IN ('exact', 'insensitive', 'adaptive')),
    abbreviation_prefix TEXT
) WITHOUT ROWID;
CREATE INDEX folder_profile ON folder (profile_id);
CREATE INDEX folder_parent ON folder (parent_id);

-- A personally owned folder (with its sub-folders) shared to a group profile.
CREATE TABLE folder_share (
    id          BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    updated_by  BLOB             CHECK (updated_by IS NULL OR length(updated_by) = 16),
    rev         INTEGER NOT NULL DEFAULT 0 CHECK (rev >= 0),
    deleted_at  INTEGER,
    folder_id   BLOB    NOT NULL REFERENCES folder (id) ON DELETE CASCADE,
    profile_id  BLOB    NOT NULL REFERENCES profile (id),
    permission  TEXT    NOT NULL CHECK (permission IN ('view', 'edit'))
) WITHOUT ROWID;
CREATE INDEX folder_share_folder ON folder_share (folder_id);
CREATE INDEX folder_share_profile ON folder_share (profile_id);

-- A snippet.
--
-- `profile_id` and `abbreviation_key` are store-maintained copies, not synced fields:
-- `profile_id` is the profile of `folder_id` and `abbreviation_key` is
-- `Abbreviation::fold_key(abbreviation)`. Together they make abbreviations unique inside a
-- profile under case folding among live rows (architecture §4.4), via `snippet_abbreviation`.
CREATE TABLE snippet (
    id               BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    updated_by       BLOB             CHECK (updated_by IS NULL OR length(updated_by) = 16),
    rev              INTEGER NOT NULL DEFAULT 0 CHECK (rev >= 0),
    deleted_at       INTEGER,
    folder_id        BLOB    NOT NULL REFERENCES folder (id),
    profile_id       BLOB    NOT NULL REFERENCES profile (id),
    abbreviation     TEXT    NOT NULL CHECK (length(abbreviation) BETWEEN 1 AND 64),
    abbreviation_key TEXT    NOT NULL,
    label            TEXT    NOT NULL,
    kind             TEXT    NOT NULL CHECK (kind IN ('plain', 'rich')),
    body             TEXT    NOT NULL,
    trigger_mode     TEXT    NOT NULL CHECK (trigger_mode IN ('immediate', 'delimiter')),
    case_mode        TEXT    NOT NULL CHECK (case_mode IN ('exact', 'insensitive', 'adaptive')),
    word_start       INTEGER NOT NULL CHECK (word_start IN (0, 1)),
    enabled          INTEGER NOT NULL CHECK (enabled IN (0, 1))
) WITHOUT ROWID;
CREATE INDEX snippet_folder ON snippet (folder_id);
CREATE UNIQUE INDEX snippet_abbreviation
    ON snippet (profile_id, abbreviation_key) WHERE deleted_at IS NULL;

-- A tag. Belongs to the profile that owns the folders of the snippets carrying it.
CREATE TABLE tag (
    id          BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    updated_by  BLOB             CHECK (updated_by IS NULL OR length(updated_by) = 16),
    rev         INTEGER NOT NULL DEFAULT 0 CHECK (rev >= 0),
    deleted_at  INTEGER,
    profile_id  BLOB    NOT NULL REFERENCES profile (id),
    name        TEXT    NOT NULL,
    color       TEXT
) WITHOUT ROWID;
CREATE INDEX tag_profile ON tag (profile_id);

-- Link between a snippet and a tag. Synced as a row of its own.
CREATE TABLE snippet_tag (
    id          BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    updated_by  BLOB             CHECK (updated_by IS NULL OR length(updated_by) = 16),
    rev         INTEGER NOT NULL DEFAULT 0 CHECK (rev >= 0),
    deleted_at  INTEGER,
    snippet_id  BLOB    NOT NULL REFERENCES snippet (id) ON DELETE CASCADE,
    tag_id      BLOB    NOT NULL REFERENCES tag (id) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX snippet_tag_snippet ON snippet_tag (snippet_id);
CREATE INDEX snippet_tag_tag ON snippet_tag (tag_id);

-- Metadata of a stored file. The bytes live on disk under `attachments/<profile id>/<sha256>`.
CREATE TABLE attachment (
    id          BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    updated_by  BLOB             CHECK (updated_by IS NULL OR length(updated_by) = 16),
    rev         INTEGER NOT NULL DEFAULT 0 CHECK (rev >= 0),
    deleted_at  INTEGER,
    profile_id  BLOB    NOT NULL REFERENCES profile (id),
    -- 64 lower-case hex digits.
    sha256      TEXT    NOT NULL CHECK (length(sha256) = 64),
    mime        TEXT    NOT NULL,
    size        INTEGER NOT NULL CHECK (size >= 0),
    file_name   TEXT    NOT NULL
) WITHOUT ROWID;
CREATE INDEX attachment_profile ON attachment (profile_id, sha256);

-- Link between a snippet and an attachment.
CREATE TABLE snippet_attachment (
    id            BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    updated_by    BLOB             CHECK (updated_by IS NULL OR length(updated_by) = 16),
    rev           INTEGER NOT NULL DEFAULT 0 CHECK (rev >= 0),
    deleted_at    INTEGER,
    snippet_id    BLOB    NOT NULL REFERENCES snippet (id) ON DELETE CASCADE,
    attachment_id BLOB    NOT NULL REFERENCES attachment (id) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX snippet_attachment_snippet ON snippet_attachment (snippet_id);
CREATE INDEX snippet_attachment_attachment ON snippet_attachment (attachment_id);

-- ---------------------------------------------------------------------------------------------
-- Device-local tables (never synced)
-- ---------------------------------------------------------------------------------------------

-- Per-snippet usage statistics (ADR-0015). Written on every expansion; never touches the
-- snippet row or the outbox.
CREATE TABLE usage (
    snippet_id   BLOB    NOT NULL PRIMARY KEY REFERENCES snippet (id) ON DELETE CASCADE,
    count        INTEGER NOT NULL DEFAULT 0 CHECK (count >= 0),
    chars_saved  INTEGER NOT NULL DEFAULT 0 CHECK (chars_saved >= 0),
    last_used_at INTEGER
) WITHOUT ROWID;

-- Key/value settings: delimiters, hotkeys, formats, locale, pause state, start at login,
-- per-app injection overrides. Values are opaque text to the store.
CREATE TABLE setting (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;

-- Apps where Iamb does not expand or buffer. `app_id` is the platform's app identifier
-- (bundle id on macOS, executable name on Windows).
CREATE TABLE app_exclusion (
    app_id       TEXT    NOT NULL PRIMARY KEY CHECK (length(app_id) > 0),
    display_name TEXT    NOT NULL,
    created_at   INTEGER NOT NULL
) WITHOUT ROWID;

-- Per snippet received from someone else: the revision the user approved (architecture §8).
CREATE TABLE approval (
    snippet_id   BLOB    NOT NULL PRIMARY KEY REFERENCES snippet (id) ON DELETE CASCADE,
    approved_rev INTEGER NOT NULL CHECK (approved_rev >= 0),
    approved_at  INTEGER NOT NULL
) WITHOUT ROWID;

-- Pending local changes for the sync agent, in order. Written in the same transaction as the
-- change. `base_rev` is the server revision the local row had before the change.
CREATE TABLE outbox (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    entity     TEXT    NOT NULL CHECK (entity IN ('profile', 'membership', 'folder',
                   'folder_share', 'snippet', 'tag', 'snippet_tag', 'attachment',
                   'snippet_attachment')),
    row_id     BLOB    NOT NULL CHECK (length(row_id) = 16),
    op         TEXT    NOT NULL CHECK (op IN ('put', 'delete')),
    base_rev   INTEGER NOT NULL CHECK (base_rev >= 0),
    created_at INTEGER NOT NULL
);

-- The user's position in the server's change feed. One row at most.
CREATE TABLE sync_state (
    id         INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    position   TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Plan features last reported by the sync agent.
CREATE TABLE entitlement (
    feature    TEXT    NOT NULL PRIMARY KEY,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;
