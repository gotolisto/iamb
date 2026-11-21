-- Migration 3: what the store gains for sync (task 4.4; docs/sync-design.md §7.1).
--
-- Everything added here is device-local and never synced, like `usage`, `setting`,
-- `app_exclusion` and `approval`.

-- The outbox: the id of each change on the wire, whether it was handed out to the agent (D1),
-- the folder delete it was written by (C6), the reason it is blocked (D5, D17), and the re-push
-- marks of a restore of the server (C18, D7). Entries written before this migration get an id now.
ALTER TABLE outbox ADD COLUMN op_id BLOB;
ALTER TABLE outbox ADD COLUMN handed_out INTEGER NOT NULL DEFAULT 0 CHECK (handed_out IN (0, 1));
ALTER TABLE outbox ADD COLUMN cascade_of INTEGER;
ALTER TABLE outbox ADD COLUMN cascade_op BLOB;
ALTER TABLE outbox ADD COLUMN blocked TEXT;
ALTER TABLE outbox ADD COLUMN tries INTEGER NOT NULL DEFAULT 0 CHECK (tries >= 0);
ALTER TABLE outbox ADD COLUMN repush TEXT CHECK (repush IS NULL OR repush IN ('row', 'base'));
ALTER TABLE outbox ADD COLUMN repush_rev INTEGER;
ALTER TABLE outbox ADD COLUMN repush_epoch TEXT;
UPDATE outbox SET op_id = randomblob(16) WHERE op_id IS NULL;
CREATE INDEX outbox_row ON outbox (entity, row_id);

-- `sync_state` gains the account, the device, the personal profile, the adoption phase (D9), the
-- time of a sign-out the server did not confirm (D14) and the state of a reconcile under way (D7).
-- The position may now be absent (signed in, not yet synced).
CREATE TABLE sync_state_new (
    id                  INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    position            TEXT,
    updated_at          INTEGER NOT NULL,
    account_user_id     BLOB CHECK (account_user_id IS NULL OR length(account_user_id) = 16),
    device_id           BLOB CHECK (device_id IS NULL OR length(device_id) = 16),
    personal_profile_id BLOB CHECK (personal_profile_id IS NULL OR length(personal_profile_id) = 16),
    adoption            TEXT CHECK (adoption IS NULL OR adoption IN ('adopted', 'merging')),
    set_aside_at        INTEGER,
    reconcile_token     TEXT,
    reconcile_scope     BLOB,
    reconcile_epoch     INTEGER NOT NULL DEFAULT 0 CHECK (reconcile_epoch IN (0, 1)),
    lost_epoch          TEXT,
    lost_started_at     INTEGER,
    lost_device_epoch   TEXT
);
INSERT INTO sync_state_new (id, position, updated_at)
SELECT id, position, updated_at FROM sync_state;
DROP TABLE sync_state;
ALTER TABLE sync_state_new RENAME TO sync_state;

-- Pulled rows the store did not apply because of an unsent change (D2, D15).
CREATE TABLE skipped (
    entity TEXT    NOT NULL,
    row_id BLOB    NOT NULL CHECK (length(row_id) = 16),
    rev    INTEGER NOT NULL CHECK (rev >= 0),
    gone   INTEGER NOT NULL CHECK (gone IN (0, 1)),
    PRIMARY KEY (entity, row_id)
) WITHOUT ROWID;

-- Rows a resync did not find on the server and left because of an unsent change (D7).
CREATE TABLE left_alone (
    entity TEXT NOT NULL,
    row_id BLOB NOT NULL CHECK (length(row_id) = 16),
    PRIMARY KEY (entity, row_id)
) WITHOUT ROWID;

-- The rows a reconcile under way has seen (D7).
CREATE TABLE reconcile_seen (
    entity TEXT NOT NULL,
    row_id BLOB NOT NULL CHECK (length(row_id) = 16),
    PRIMARY KEY (entity, row_id)
) WITHOUT ROWID;

-- The lines of a restore of the server, per profile (C18, D7), while a reconcile in a new epoch is
-- under way.
CREATE TABLE lost_line (
    profile_id BLOB    NOT NULL PRIMARY KEY CHECK (length(profile_id) = 16),
    backup_rev INTEGER NOT NULL CHECK (backup_rev >= 0)
) WITHOUT ROWID;

-- The account calls by which this device's user took access away (D7).
CREATE TABLE access_removal (
    id         BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    call       TEXT    NOT NULL,
    profile_id BLOB,
    subject    BLOB,
    mark       INTEGER,
    made_at    INTEGER NOT NULL
) WITHOUT ROWID;

-- The user's access roots outside the personal profile, as the server computed them (§4.2).
CREATE TABLE folder_access (
    folder_id  BLOB    NOT NULL PRIMARY KEY CHECK (length(folder_id) = 16),
    profile_id BLOB    NOT NULL CHECK (length(profile_id) = 16),
    permission TEXT    NOT NULL CHECK (permission IN ('view', 'edit', 'manage')),
    via        TEXT    NOT NULL,
    granted_at INTEGER NOT NULL
) WITHOUT ROWID;

-- Who wrote a snippet's content, and when the server first accepted the row (C17, D10, D13).
CREATE TABLE snippet_origin (
    snippet_id  BLOB    NOT NULL PRIMARY KEY REFERENCES snippet (id) ON DELETE CASCADE,
    content_by  BLOB,
    content_at  INTEGER,
    created_rev INTEGER NOT NULL DEFAULT 0 CHECK (created_rev >= 0)
) WITHOUT ROWID;

-- What the user last allowed a held snippet to type (D10).
ALTER TABLE approval ADD COLUMN approved_content TEXT;

-- Changes the server refused, or that lost their folder, kept for the user (D5).
CREATE TABLE kept_change (
    id        BLOB    NOT NULL PRIMARY KEY CHECK (length(id) = 16),
    entity    TEXT    NOT NULL,
    row_id    BLOB    NOT NULL CHECK (length(row_id) = 16),
    kind      TEXT    NOT NULL CHECK (kind IN ('put', 'delete')),
    content   TEXT,
    held      INTEGER NOT NULL DEFAULT 0 CHECK (held IN (0, 1)),
    place     TEXT    NOT NULL,
    reason    TEXT    NOT NULL,
    made_at   INTEGER NOT NULL,
    kept_at   INTEGER NOT NULL
) WITHOUT ROWID;

-- Authors' and members' names.
CREATE TABLE known_user (
    user_id      BLOB NOT NULL PRIMARY KEY CHECK (length(user_id) = 16),
    display_name TEXT NOT NULL
) WITHOUT ROWID;

-- Which files this device has sent (§6.11).
CREATE TABLE attachment_state (
    profile_id BLOB    NOT NULL,
    sha256     TEXT    NOT NULL,
    uploaded   INTEGER NOT NULL CHECK (uploaded IN (0, 1)),
    PRIMARY KEY (profile_id, sha256)
) WITHOUT ROWID;

-- Rows from the server are applied as they are: abbreviations are no longer unique in the schema.
-- The store still refuses a duplicate in a local edit, and the service orders what sync brings
-- together by precedence (C9, D13).
DROP INDEX snippet_abbreviation;
CREATE INDEX snippet_abbreviation ON snippet (profile_id, abbreviation_key) WHERE deleted_at IS NULL;
