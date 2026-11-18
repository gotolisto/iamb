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

UPDATE snippet
SET abbreviation_key = replace(abbreviation_key, 'ς', 'σ')
WHERE instr(abbreviation_key, 'ς') > 0
  AND (
    deleted_at IS NOT NULL
    OR NOT EXISTS (
      SELECT 1 FROM snippet AS other
      WHERE other.profile_id = snippet.profile_id
        AND other.deleted_at IS NULL
        AND other.id <> snippet.id
        AND replace(other.abbreviation_key, 'ς', 'σ') = replace(snippet.abbreviation_key, 'ς', 'σ')
    )
  );
