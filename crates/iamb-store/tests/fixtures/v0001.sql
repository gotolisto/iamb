-- Fixture rows valid at schema version 1, written as a release at version 1 would have left
-- them. Never edit this file once released; a later migration gets its own fixture.
--
-- Ids: X'01900000000070008000000000000aNN' (NN distinguishes rows).

INSERT INTO profile (id, created_at, updated_at, updated_by, rev, deleted_at, kind, name, owner_user_id)
VALUES (X'01900000000070008000000000000a01', 1000, 1100, X'01900000000070008000000000000aff', 3, NULL,
        'personal', 'Me', X'01900000000070008000000000000aff'),
       (X'01900000000070008000000000000a02', 1000, 1000, NULL, 0, NULL,
        'family', 'Family', X'01900000000070008000000000000aff');

INSERT INTO membership (id, created_at, updated_at, updated_by, rev, deleted_at, profile_id, user_id, role)
VALUES (X'01900000000070008000000000000a03', 1000, 1000, NULL, 1, NULL,
        X'01900000000070008000000000000a02', X'01900000000070008000000000000aff', 'owner');

INSERT INTO folder (id, created_at, updated_at, updated_by, rev, deleted_at, profile_id, parent_id, name,
                    position, member_permission, default_trigger, default_case_mode, abbreviation_prefix)
VALUES (X'01900000000070008000000000000a10', 1000, 1000, NULL, 2, NULL,
        X'01900000000070008000000000000a01', NULL, 'Work', 0, NULL, 'immediate', 'adaptive', ';'),
       (X'01900000000070008000000000000a11', 1000, 1000, NULL, 0, NULL,
        X'01900000000070008000000000000a01', X'01900000000070008000000000000a10', 'Replies', 1, NULL,
        NULL, NULL, NULL),
       (X'01900000000070008000000000000a12', 1000, 2000, NULL, 0, 2000,
        X'01900000000070008000000000000a01', NULL, 'Old', 2, NULL, NULL, NULL, NULL);

INSERT INTO folder_share (id, created_at, updated_at, updated_by, rev, deleted_at, folder_id, profile_id, permission)
VALUES (X'01900000000070008000000000000a13', 1000, 1000, NULL, 0, NULL,
        X'01900000000070008000000000000a10', X'01900000000070008000000000000a02', 'edit');

INSERT INTO snippet (id, created_at, updated_at, updated_by, rev, deleted_at, folder_id, profile_id,
                     abbreviation, abbreviation_key, label, kind, body, trigger_mode, case_mode,
                     word_start, enabled)
VALUES (X'01900000000070008000000000000a20', 1000, 1500, NULL, 5, NULL,
        X'01900000000070008000000000000a11', X'01900000000070008000000000000a01',
        ';Sig', ';sig', 'Signature', 'plain', 'Best regards,
{cursor}', 'immediate', 'adaptive', 0, 1),
       (X'01900000000070008000000000000a21', 1000, 2000, NULL, 0, 2000,
        X'01900000000070008000000000000a12', X'01900000000070008000000000000a01',
        ';sig', ';sig', 'Old signature', 'rich', '**old**', 'delimiter', 'exact', 1, 0);

INSERT INTO tag (id, created_at, updated_at, updated_by, rev, deleted_at, profile_id, name, color)
VALUES (X'01900000000070008000000000000a30', 1000, 1000, NULL, 0, NULL,
        X'01900000000070008000000000000a01', 'email', '#112233');

INSERT INTO snippet_tag (id, created_at, updated_at, updated_by, rev, deleted_at, snippet_id, tag_id)
VALUES (X'01900000000070008000000000000a31', 1000, 1000, NULL, 0, NULL,
        X'01900000000070008000000000000a20', X'01900000000070008000000000000a30');

INSERT INTO attachment (id, created_at, updated_at, updated_by, rev, deleted_at, profile_id, sha256, mime,
                        size, file_name)
VALUES (X'01900000000070008000000000000a40', 1000, 1000, NULL, 0, NULL,
        X'01900000000070008000000000000a01',
        'abababababababababababababababababababababababababababababababab', 'image/png', 42, 'logo.png');

INSERT INTO snippet_attachment (id, created_at, updated_at, updated_by, rev, deleted_at, snippet_id,
                                attachment_id)
VALUES (X'01900000000070008000000000000a41', 1000, 1000, NULL, 0, NULL,
        X'01900000000070008000000000000a20', X'01900000000070008000000000000a40');

INSERT INTO usage (snippet_id, count, chars_saved, last_used_at)
VALUES (X'01900000000070008000000000000a20', 12, 340, 1700);

INSERT INTO setting (key, value) VALUES ('delimiters', ' .,;');

INSERT INTO app_exclusion (app_id, display_name, created_at) VALUES ('com.example.vault', 'Vault', 1000);

INSERT INTO approval (snippet_id, approved_rev, approved_at)
VALUES (X'01900000000070008000000000000a20', 5, 1600);

INSERT INTO outbox (entity, row_id, op, base_rev, created_at)
VALUES ('snippet', X'01900000000070008000000000000a20', 'put', 5, 1500);

INSERT INTO sync_state (id, position, updated_at) VALUES (1, 'feed:7', 1800);

INSERT INTO entitlement (feature, value, updated_at) VALUES ('sync', 'true', 1900);
