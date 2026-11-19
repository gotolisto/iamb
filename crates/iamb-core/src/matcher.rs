//! Abbreviation matcher (task 1.2).
//!
//! Implements `docs/architecture.md` §4.4. The engine keeps a [`KeyBuffer`] of the last typed
//! characters and, after each `Char` event has been pushed, asks a [`Matcher`] whether an
//! abbreviation ends at the caret:
//!
//! ```
//! use iamb_core::matcher::{CasePattern, Delimiters, KeyBuffer, MatchEntry, Matcher};
//! use iamb_core::model::{Abbreviation, CaseMode, Id, Trigger};
//!
//! let brb = MatchEntry {
//!     snippet: Id::from_bytes([1; 16]),
//!     abbreviation: Abbreviation::new("brb")?,
//!     trigger: Trigger::Delimiter,
//!     case_mode: CaseMode::Adaptive,
//!     word_start: true,
//! };
//! let matcher = Matcher::build([brb], Delimiters::default());
//!
//! let mut buffer = KeyBuffer::new();
//! for ch in "ok BRB".chars() {
//!     buffer.push(ch);
//!     assert_eq!(matcher.lookup(&buffer), None);
//! }
//! buffer.push(' ');
//! let m = matcher.lookup(&buffer).expect("delimiter typed");
//! assert_eq!((m.typed_chars, m.delimiter, m.case), (3, Some(' '), CasePattern::Upper));
//! assert_eq!(m.backspaces(), 4);
//! # Ok::<(), iamb_core::model::ModelError>(())
//! ```
//!
//! # Rules
//!
//! - **Folding.** Matching compares characters folded one at a time with
//!   [`char::to_lowercase`], with final sigma `ς` folded to `σ`, exactly as
//!   [`Abbreviation::fold_key`] folds: [`str::to_lowercase`] would pick between the two sigmas
//!   from the surrounding letters, which a matcher reading the caret backwards cannot see, so
//!   `ΟΔΟΣ`, `οδοσ` and `οδος` all match each other. No other folding (such as
//!   `ß` versus `ss`) is applied. Folding may turn one character into several (`İ` → `i` +
//!   U+0307); a match always covers whole typed characters and [`Match::typed_chars`] counts
//!   typed characters.
//! - **Triggers.** An [`Trigger::Immediate`] snippet matches when the buffer ends with its
//!   abbreviation. A [`Trigger::Delimiter`] snippet matches when the last buffer character is in
//!   the [`Delimiters`] set and the characters before it end with the abbreviation.
//! - **Case.** [`CaseMode::Exact`] compares the typed characters with the abbreviation as
//!   written; [`CaseMode::Insensitive`] and [`CaseMode::Adaptive`] compare folded characters.
//!   Only adaptive snippets report a typed [`CasePattern`]; the others report
//!   [`CasePattern::AsWritten`].
//! - **Word start.** With `word_start`, the character before the abbreviation must be a
//!   delimiter or whitespace, or the abbreviation must begin at the start of the buffer. The
//!   start counts only if no character has fallen off the front since the last clear: after an
//!   overflow the real preceding character is unknown, and the matcher fails closed.
//! - **Several candidates.** Only candidates that satisfy every rule above compete. The one
//!   with the most typed characters wins. If an immediate and a delimiter candidate match on the
//!   same keystroke (the last character is a delimiter that also ends an immediate
//!   abbreviation), the longer abbreviation wins and on equal length the immediate one wins.
//!   Two different candidates of the same trigger can never have the same length, because their
//!   folded keys would be equal and the second would have been dropped as a duplicate.
//! - **Shadowing.** Because lookup runs after every keystroke, an immediate abbreviation that is
//!   a prefix of a longer one fires first and hides it (`ad` hides `add`). [`Matcher::shadowed`]
//!   lists such pairs for library health.
//! - **Duplicates.** The store keeps abbreviations unique by fold key, but [`Matcher::build`]
//!   does not rely on it: of two entries with the same folded key (which includes every pair
//!   with equal fold keys), the first is kept and the rest are listed by
//!   [`Matcher::duplicates`]. Build never fails.

mod buffer;
mod trie;

use std::collections::BTreeMap;
use std::fmt;

pub use buffer::{KEY_BUFFER_CAPACITY, KeyBuffer};

use crate::model::{Abbreviation, CaseMode, Id, Snippet, Trigger};
use trie::{Trie, TrieBuilder};

// ---------------------------------------------------------------------------------------------
// Delimiters
// ---------------------------------------------------------------------------------------------

/// The set of characters that end a word and fire [`Trigger::Delimiter`] snippets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delimiters {
    /// Sorted, without duplicates.
    chars: Box<[char]>,
}

impl Delimiters {
    /// The default set (architecture §4.4): space and `. , ; : ! ? ) ] } " '`.
    ///
    /// Enter (`'\n'`) and Tab (`'\t'`) are not in it. The listener never holds a key back, so the
    /// application acts on them before the expansion arrives: Tab moves to the next field of a
    /// form, which can be a password field, and Enter sends a message or closes a dialog. The
    /// expansion would land wherever the key took the focus (privacy review 2.8, finding 1). A
    /// user can add them to the `delimiters` setting.
    pub const DEFAULT_CHARS: [char; 12] =
        [' ', '.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

    /// A set of the given characters. Order and repeats do not matter.
    #[must_use]
    pub fn new(chars: impl IntoIterator<Item = char>) -> Self {
        let mut v: Vec<char> = chars.into_iter().collect();
        v.sort_unstable();
        v.dedup();
        Self {
            chars: v.into_boxed_slice(),
        }
    }

    /// Whether `c` is a delimiter.
    #[must_use]
    pub fn contains(&self, c: char) -> bool {
        self.chars.binary_search(&c).is_ok()
    }

    /// The delimiters in ascending code point order.
    pub fn iter(&self) -> impl Iterator<Item = char> + '_ {
        self.chars.iter().copied()
    }
}

impl Default for Delimiters {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CHARS)
    }
}

// ---------------------------------------------------------------------------------------------
// Inputs and outputs
// ---------------------------------------------------------------------------------------------

/// One matchable snippet, as the matcher needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchEntry {
    /// The snippet that fires.
    pub snippet: Id,
    /// Its abbreviation.
    pub abbreviation: Abbreviation,
    /// When it fires.
    pub trigger: Trigger,
    /// How case is compared.
    pub case_mode: CaseMode,
    /// Whether the abbreviation must start a word.
    pub word_start: bool,
}

impl MatchEntry {
    /// The entry for `snippet`. Does not check `enabled` or deletion; the caller builds the
    /// matchable set (architecture §4.4).
    #[must_use]
    pub fn from_snippet(snippet: &Snippet) -> Self {
        Self {
            snippet: snippet.meta.id,
            abbreviation: snippet.abbreviation.clone(),
            trigger: snippet.trigger,
            case_mode: snippet.case_mode,
            word_start: snippet.word_start,
        }
    }
}

/// The case pattern of the typed abbreviation, used by [`CaseMode::Adaptive`] expansions.
///
/// Cased letters are those for which [`char::is_uppercase`] or [`char::is_lowercase`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CasePattern {
    /// Expand as written: the cased letters were all lower case, there were none, or they were
    /// mixed (`abc`, `a1`, `aBc`, `ABc`). Also reported for non-adaptive snippets.
    AsWritten,
    /// Capitalise the first letter: the first cased letter was upper case and every other one
    /// lower case (`Abc`, `A`, `1Ab`).
    Capitalized,
    /// Upper-case the expansion: at least two cased letters, all upper case (`ABC`, `A1B`).
    Upper,
}

impl CasePattern {
    /// Classifies typed characters.
    fn of(typed: &[char]) -> Self {
        let mut cased = typed
            .iter()
            .filter(|c| c.is_uppercase() || c.is_lowercase());
        let Some(first) = cased.next() else {
            return Self::AsWritten;
        };
        let (mut upper, mut lower) = (0_usize, 0_usize);
        for c in cased {
            if c.is_uppercase() {
                upper += 1;
            } else {
                lower += 1;
            }
        }
        match (first.is_uppercase(), upper, lower) {
            (true, 0, _) => Self::Capitalized,
            (true, _, 0) => Self::Upper,
            _ => Self::AsWritten,
        }
    }
}

/// A snippet whose abbreviation ends at the caret.
///
/// `Debug` shows whether a delimiter was typed but not which one, so formatting a match never
/// prints a typed character.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// The snippet to expand.
    pub snippet: Id,
    /// Number of typed characters (one per `Char` event) that make up the abbreviation. Can
    /// differ from the abbreviation's own length when case folding changes length.
    pub typed_chars: usize,
    /// The delimiter typed after the abbreviation, for [`Trigger::Delimiter`] snippets.
    pub delimiter: Option<char>,
    /// The typed case pattern; [`CasePattern::AsWritten`] unless the snippet is adaptive.
    pub case: CasePattern,
}

impl Match {
    /// Number of Backspace presses that remove the abbreviation and its delimiter.
    #[must_use]
    pub const fn backspaces(&self) -> usize {
        self.typed_chars + if self.delimiter.is_some() { 1 } else { 0 }
    }
}

impl fmt::Debug for Match {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Match")
            .field("snippet", &self.snippet)
            .field("typed_chars", &self.typed_chars)
            .field(
                "delimiter",
                &format_args!(
                    "{}",
                    if self.delimiter.is_some() {
                        "Some(..)"
                    } else {
                        "None"
                    }
                ),
            )
            .field("case", &self.case)
            .finish()
    }
}

/// A build input that was dropped because an earlier entry holds the same abbreviation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Duplicate {
    /// The earlier entry, which stays matchable.
    pub kept: Id,
    /// The later entry, which never matches.
    pub dropped: Id,
}

/// An immediate abbreviation that fires before a longer one can be typed to completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shadowing {
    /// The immediate snippet whose abbreviation is a proper prefix of the other.
    pub by: Id,
    /// The snippet that is hidden.
    pub hidden: Id,
}

// ---------------------------------------------------------------------------------------------
// Matcher
// ---------------------------------------------------------------------------------------------

/// A kept entry. `chars` is the abbreviation as written.
struct Entry {
    snippet: Id,
    chars: Box<[char]>,
    trigger: Trigger,
    case_mode: CaseMode,
    word_start: bool,
}

/// Folds one typed or abbreviation character for matching (see the module docs). The same
/// function builds [`Abbreviation::fold_key`].
fn fold(c: char) -> std::char::ToLowercase {
    crate::model::fold_char(c)
}

/// The folded character sequence the tries are keyed on.
fn match_key(chars: impl IntoIterator<Item = char>) -> Vec<char> {
    chars.into_iter().flat_map(fold).collect()
}

/// Matches the keystroke buffer against one library snapshot.
///
/// Built once per snapshot and immutable afterwards, so it can be shared between threads
/// (`Send + Sync`). Internally two tries (one per trigger) over the reversed, folded
/// abbreviations, walked from the caret backwards. See the [module docs](self) for the rules.
pub struct Matcher {
    entries: Box<[Entry]>,
    immediate: Trie,
    delimited: Trie,
    delimiters: Delimiters,
    duplicates: Box<[Duplicate]>,
}

impl Matcher {
    /// Builds a matcher from the matchable set and the delimiter setting.
    ///
    /// An entry whose folded abbreviation equals an earlier entry's is dropped (the first
    /// wins) and listed in [`duplicates`](Self::duplicates). Equal [`Abbreviation::fold_key`]s
    /// always fold equal, and the converse holds too: both use one folding function.
    ///
    /// # Panics
    ///
    /// Only if the tries would need more than `u32::MAX` nodes (over 22 million abbreviations).
    #[must_use]
    pub fn build(entries: impl IntoIterator<Item = MatchEntry>, delimiters: Delimiters) -> Self {
        let mut kept: Vec<Entry> = Vec::new();
        let mut duplicates = Vec::new();
        let mut by_match_key: BTreeMap<Vec<char>, usize> = BTreeMap::new();
        let mut immediate = TrieBuilder::new();
        let mut delimited = TrieBuilder::new();

        for e in entries {
            let key = match_key(e.abbreviation.as_str().chars());
            if let Some(&i) = by_match_key.get(&key) {
                duplicates.push(Duplicate {
                    kept: kept[i].snippet,
                    dropped: e.snippet,
                });
                continue;
            }
            let index = kept.len();
            let trie = match e.trigger {
                Trigger::Immediate => &mut immediate,
                Trigger::Delimiter => &mut delimited,
            };
            let clash = trie.insert(key.iter().rev().copied(), index);
            debug_assert!(clash.is_none(), "match keys are deduplicated above");
            by_match_key.insert(key, index);
            kept.push(Entry {
                snippet: e.snippet,
                chars: e.abbreviation.as_str().chars().collect(),
                trigger: e.trigger,
                case_mode: e.case_mode,
                word_start: e.word_start,
            });
        }

        Self {
            entries: kept.into_boxed_slice(),
            immediate: immediate.freeze(),
            delimited: delimited.freeze(),
            delimiters,
            duplicates: duplicates.into_boxed_slice(),
        }
    }

    /// The snippet whose abbreviation ends at the caret, if any. Call after each `Char` was
    /// pushed to `buffer`.
    #[must_use]
    pub fn lookup(&self, buffer: &KeyBuffer) -> Option<Match> {
        let chars = buffer.chars();
        let (&last, before_last) = chars.split_last()?;
        let truncated = buffer.truncated();

        let imm = self.longest(&self.immediate, chars, truncated);
        let delim = if self.delimiters.contains(last) {
            self.longest(&self.delimited, before_last, truncated)
        } else {
            None
        };
        let (typed_chars, index, delimiter) = match (imm, delim) {
            (Some((ki, _)), Some((kd, d))) if kd > ki => (kd, d, Some(last)),
            (Some((k, i)), _) => (k, i, None),
            (None, Some((k, d))) => (k, d, Some(last)),
            (None, None) => return None,
        };

        let entry = &self.entries[index];
        let end = chars.len() - usize::from(delimiter.is_some());
        let case = if entry.case_mode == CaseMode::Adaptive {
            CasePattern::of(&chars[end - typed_chars..end])
        } else {
            CasePattern::AsWritten
        };
        Some(Match {
            snippet: entry.snippet,
            typed_chars,
            delimiter,
            case,
        })
    }

    /// Walks `trie` backwards from the end of `typed`. Returns the typed length and entry index
    /// of the longest entry that satisfies every rule.
    fn longest(&self, trie: &Trie, typed: &[char], truncated: bool) -> Option<(usize, usize)> {
        let mut node = Trie::ROOT;
        let mut best = None;
        for (consumed, &c) in typed.iter().rev().enumerate() {
            for f in fold(c).rev() {
                node = match trie.child(node, f) {
                    Some(n) => n,
                    None => return best,
                };
            }
            // Only whole typed characters can end a match.
            if let Some(index) = trie.entry(node) {
                let start = typed.len() - (consumed + 1);
                if self.accepts(&self.entries[index], typed, start, truncated) {
                    best = Some((consumed + 1, index));
                }
            }
        }
        best
    }

    /// Checks the case and word-start rules for `entry` matched at `typed[start..]`.
    fn accepts(&self, entry: &Entry, typed: &[char], start: usize, truncated: bool) -> bool {
        if entry.case_mode == CaseMode::Exact && typed[start..] != *entry.chars {
            return false;
        }
        if !entry.word_start {
            return true;
        }
        match start.checked_sub(1) {
            None => !truncated,
            Some(i) => typed[i].is_whitespace() || self.delimiters.contains(typed[i]),
        }
    }

    /// Entries dropped at build time because an earlier entry holds the same abbreviation, in
    /// input order.
    #[must_use]
    pub fn duplicates(&self) -> &[Duplicate] {
        &self.duplicates
    }

    /// Immediate abbreviations that are a proper prefix of a longer abbreviation, so the longer
    /// one fires only if typed in a way the shorter does not match (for example with a case the
    /// exact shorter one rejects, or mid-word when the shorter one needs a word start).
    ///
    /// Compared on folded characters, at whole-character boundaries of the longer abbreviation.
    /// Ordered by the shadowing entry's build position, then by the hidden abbreviation.
    #[must_use]
    pub fn shadowed(&self) -> Vec<Shadowing> {
        let mut keys: Vec<(Vec<char>, usize)> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (match_key(e.chars.iter().copied()), i))
            .collect();
        keys.sort_unstable();
        let mut out = Vec::new();
        for entry in &self.entries {
            if entry.trigger != Trigger::Immediate {
                continue;
            }
            let key = match_key(entry.chars.iter().copied());
            let from = keys.partition_point(|(k, _)| k.as_slice() <= key.as_slice());
            for (other_key, j) in &keys[from..] {
                if !other_key.starts_with(&key) {
                    break;
                }
                if ends_on_char_boundary(&self.entries[*j].chars, key.len()) {
                    out.push(Shadowing {
                        by: entry.snippet,
                        hidden: self.entries[*j].snippet,
                    });
                }
            }
        }
        out
    }

    /// The delimiter set the matcher was built with.
    #[must_use]
    pub const fn delimiters(&self) -> &Delimiters {
        &self.delimiters
    }

    /// Number of matchable entries (duplicates excluded).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no entry is matchable.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Whether folding a prefix of `chars` yields exactly `folded_len` characters.
fn ends_on_char_boundary(chars: &[char], folded_len: usize) -> bool {
    let mut n = 0;
    for c in chars {
        if n == folded_len {
            return true;
        }
        n += fold(*c).count();
        if n > folded_len {
            return false;
        }
    }
    n == folded_len
}

impl fmt::Debug for Matcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Matcher")
            .field("entries", &self.entries.len())
            .field("duplicates", &self.duplicates.len())
            .field("delimiters", &self.delimiters)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{CasePattern, ends_on_char_boundary};

    fn pattern(s: &str) -> CasePattern {
        CasePattern::of(&s.chars().collect::<Vec<_>>())
    }

    #[test]
    fn case_patterns() {
        use CasePattern::{AsWritten, Capitalized, Upper};
        for (typed, want) in [
            ("abc", AsWritten),
            ("", AsWritten),
            ("12;", AsWritten),
            ("Abc", Capitalized),
            ("A", Capitalized),
            ("1Ab", Capitalized),
            ("ABC", Upper),
            ("A1B", Upper),
            ("aBc", AsWritten),
            ("ABc", AsWritten),
            ("AbC", AsWritten),
            ("ẞX", Upper),
            ("Éa", Capitalized),
        ] {
            assert_eq!(pattern(typed), want, "{typed:?}");
        }
    }

    #[test]
    fn boundaries() {
        let chars: Vec<char> = "İab".chars().collect();
        assert!(ends_on_char_boundary(&chars, 0));
        assert!(!ends_on_char_boundary(&chars, 1));
        assert!(ends_on_char_boundary(&chars, 2));
        assert!(ends_on_char_boundary(&chars, 4));
        assert!(!ends_on_char_boundary(&chars, 5));
    }
}
