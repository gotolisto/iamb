-- Fixture rows valid at schema version 3. Never edit this file once released.
--
-- Ids: X'01900000000070008000000000000cNN'. Two live snippets of one profile share an
-- abbreviation (rows from the server are applied as they are, C9), an outbox entry carries the
-- columns of D1, and the device-local sync tables hold one row each.

INSERT INTO snippet (id, created_at, updated_at, updated_by, rev, deleted_at, folder_id, profile_id,
                     abbreviation, abbreviation_key, label, kind, body, trigger_mode, case_mode,
                     word_start, enabled)
VALUES (X'01900000000070008000000000000c20', 4000, 4000, NULL, 41, NULL,
        X'01900000000070008000000000000a10', X'01900000000070008000000000000a01',
        ';dup', ';dup', 'First', 'plain', 'one', 'delimiter', 'insensitive', 1, 1),
       (X'01900000000070008000000000000c21', 4000, 4000, NULL, 42, NULL,
        X'01900000000070008000000000000a10', X'01900000000070008000000000000a01',
        ';DUP', ';dup', 'Second', 'plain', 'two', 'delimiter', 'insensitive', 1, 1);

INSERT INTO outbox (entity, row_id, op, base_rev, created_at, op_id, handed_out, cascade_of,
                    cascade_op, blocked, tries, repush, repush_rev, repush_epoch)
VALUES ('snippet', X'01900000000070008000000000000c21', 'put', 42, 4100,
        X'01900000000070008000000000000c99', 1, NULL, NULL, NULL, 0, NULL, NULL, NULL);

INSERT INTO snippet_origin (snippet_id, content_by, content_at, created_rev)
VALUES (X'01900000000070008000000000000c20', X'01900000000070008000000000000aff', 3900, 40);

INSERT INTO kept_change (id, entity, row_id, kind, content, held, place, reason, made_at, kept_at)
VALUES (X'01900000000070008000000000000c30', 'snippet', X'01900000000070008000000000000c22',
        'delete', NULL, 0, 'Work', 'forbidden', 4200, 4300);

INSERT INTO folder_access (folder_id, profile_id, permission, via, granted_at)
VALUES (X'01900000000070008000000000000a10', X'01900000000070008000000000000a01', 'manage', 'own', 4400);
