-- Fixture rows valid at schema version 2. Never edit this file once released.
--
-- Ids: X'01900000000070008000000000000bNN'. Keys are written with version 2 folding (ς -> σ).

INSERT INTO snippet (id, created_at, updated_at, updated_by, rev, deleted_at, folder_id, profile_id,
                     abbreviation, abbreviation_key, label, kind, body, trigger_mode, case_mode,
                     word_start, enabled)
VALUES (X'01900000000070008000000000000b20', 3000, 3000, NULL, 0, NULL,
        X'01900000000070008000000000000a10', X'01900000000070008000000000000a01',
        'ΔΡΟΜΟΣ', 'δρομοσ', 'Greek road', 'plain', 'road', 'delimiter', 'insensitive', 1, 1);
