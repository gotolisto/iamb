-- Migration 2: abbreviation keys use the matcher's case folding.
--
-- Version 1 stored `abbreviation_key` as `str::to_lowercase(abbreviation)`, which turns a
-- word-final capital sigma into the final form ς. The matcher folds one character at a time and
-- treats ς and σ as the same letter (ADR-0024), so `οδος` and `οδοσ` could both be stored while
-- only one of them could ever be typed. `Abbreviation::fold_key` now folds per character and maps
-- ς to σ; the two rules differ only in that letter, so replacing ς with σ in each stored key gives
-- exactly the new key.
--
-- Live rows whose new key would collide with another live row in the same profile keep their old
-- key: rewriting them would break the uniqueness index. Such a pair already could not both be
-- typed; library health reports the duplicate, and the user keeps both snippets.

-- The work is linear: the new key of every affected row is computed once into a temporary table
-- that is indexed on (profile_id, new_key); a live row is rewritten only when no other live row
-- of its profile ends up with the same key, either unchanged (looked up through the
-- `snippet_abbreviation` index) or also affected (looked up through the temporary index).
-- The decisions are all made before the first row is changed, so the order of the updates does
-- not matter.

CREATE TEMP TABLE fold_candidate (
    id         BLOB    NOT NULL PRIMARY KEY,
    profile_id BLOB    NOT NULL,
    new_key    TEXT    NOT NULL,
    live       INTEGER NOT NULL
) WITHOUT ROWID;

INSERT INTO fold_candidate (id, profile_id, new_key, live)
SELECT id, profile_id, replace(abbreviation_key, 'ς', 'σ'), deleted_at IS NULL
FROM snippet
WHERE instr(abbreviation_key, 'ς') > 0;

CREATE INDEX fold_candidate_key ON fold_candidate (profile_id, new_key) WHERE live = 1;

CREATE TEMP TABLE fold_apply (
    id      BLOB NOT NULL PRIMARY KEY,
    new_key TEXT NOT NULL
) WITHOUT ROWID;

INSERT INTO fold_apply (id, new_key)
SELECT c.id, c.new_key
FROM fold_candidate AS c
WHERE c.live = 0
   OR (
       NOT EXISTS (
           SELECT 1 FROM fold_candidate AS o
           WHERE o.live = 1 AND o.profile_id = c.profile_id AND o.new_key = c.new_key
             AND o.id <> c.id
       )
       AND NOT EXISTS (
           SELECT 1 FROM snippet AS other
           WHERE other.profile_id = c.profile_id AND other.abbreviation_key = c.new_key
             AND other.deleted_at IS NULL AND other.id <> c.id
       )
   );

UPDATE snippet
SET abbreviation_key = (SELECT new_key FROM fold_apply WHERE fold_apply.id = snippet.id)
WHERE id IN (SELECT id FROM fold_apply);

DROP TABLE fold_apply;
DROP TABLE fold_candidate;
