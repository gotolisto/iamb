//! The fixture library and the seeded key stream (`docs/test-strategy.md` §5.1), with the exact
//! text each step must leave in the field.
//!
//! # The library
//!
//! Snippet `i` of `n` (at most [`MAX_LIBRARY`]) gets a code of three letters from `i` and one of
//! three shapes, so no abbreviation can fire inside ordinary words, inside another
//! abbreviation or inside a canary:
//!
//! | Share | Shape | Trigger | Word start | Example |
//! | --- | --- | --- | --- | --- |
//! | 60% | `v` + code | delimiter | yes | `vbcd` |
//! | 15% | `v` + code + `9` | delimiter | no (fires after any letters) | `vbcd9` |
//! | 25% | `;k` + code | immediate | no | `;kbcd` |
//!
//! Case modes cycle through exact, insensitive and adaptive within each shape. Bodies are plain
//! text from the word list (no template syntax), 20 to about 300 characters, some with a line
//! break. Ordinary words never start with `v` and hold letters only; canaries are lower-case
//! letters only; so a word-start abbreviation never equals a typed word, the `9` shape can never
//! be the end of a letters-only word, and the `;` shape never occurs in one.
//!
//! # The key stream
//!
//! A [`Unit`] starts in an empty field (the driver clicks and clears it) with an ordinary word,
//! then 7 to 15 more tokens: 30% abbreviations (typed in a case variant that its case mode
//! allows) and 70% ordinary words, some capitalised, some followed by `,` or `.`. Every token
//! ends with a space.

use iamb_core::model::{
    Abbreviation, CaseMode, Id, ModelError, Snippet, SyncMeta, Timestamp, Trigger,
};

use super::rng::SplitMix64;

/// The largest library: three letters of code give 26³ abbreviations per shape.
pub const MAX_LIBRARY: usize = 26 * 26 * 26;

/// Ordinary words: lower-case letters only, none starting with `v`.
pub const WORDS: &[&str] = &[
    "the",
    "of",
    "and",
    "to",
    "in",
    "is",
    "it",
    "that",
    "for",
    "was",
    "on",
    "are",
    "as",
    "with",
    "his",
    "they",
    "at",
    "be",
    "this",
    "have",
    "from",
    "or",
    "one",
    "had",
    "by",
    "word",
    "but",
    "not",
    "what",
    "all",
    "were",
    "we",
    "when",
    "your",
    "can",
    "said",
    "there",
    "use",
    "an",
    "each",
    "which",
    "she",
    "do",
    "how",
    "their",
    "if",
    "will",
    "up",
    "other",
    "about",
    "out",
    "many",
    "then",
    "them",
    "these",
    "so",
    "some",
    "her",
    "would",
    "make",
    "like",
    "him",
    "into",
    "time",
    "has",
    "look",
    "two",
    "more",
    "write",
    "go",
    "see",
    "number",
    "no",
    "way",
    "could",
    "people",
    "my",
    "than",
    "first",
    "water",
    "been",
    "call",
    "who",
    "oil",
    "its",
    "now",
    "find",
    "long",
    "down",
    "day",
    "did",
    "get",
    "come",
    "made",
    "may",
    "part",
    "over",
    "new",
    "sound",
    "take",
    "only",
    "little",
    "work",
    "know",
    "place",
    "year",
    "live",
    "me",
    "back",
    "give",
    "most",
    "after",
    "thing",
    "our",
    "just",
    "name",
    "good",
    "sentence",
    "man",
    "think",
    "say",
    "great",
    "where",
    "help",
    "through",
    "much",
    "before",
    "line",
    "right",
    "too",
    "mean",
    "old",
    "any",
    "same",
    "tell",
    "boy",
    "follow",
    "came",
    "want",
    "show",
    "also",
    "around",
    "form",
    "three",
    "small",
    "set",
    "put",
    "end",
    "does",
    "another",
    "well",
    "large",
    "must",
    "big",
    "even",
    "such",
    "because",
    "turn",
    "here",
    "why",
    "ask",
    "went",
    "men",
    "read",
    "need",
    "land",
    "different",
    "home",
    "us",
    "move",
    "try",
    "kind",
    "hand",
    "picture",
    "again",
    "change",
    "off",
    "play",
    "spell",
    "air",
    "away",
    "animal",
    "house",
    "point",
    "page",
    "letter",
    "mother",
    "answer",
    "found",
    "study",
    "still",
    "learn",
    "should",
    "america",
    "world",
    "meeting",
    "invoice",
    "schedule",
    "thanks",
    "regards",
    "project",
];

/// The shape of a fixture snippet (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `v` + code, delimiter trigger, word start.
    Delimiter,
    /// `v` + code + `9`, delimiter trigger, no word start.
    DelimiterAnywhere,
    /// `;k` + code, immediate trigger, no word start.
    Immediate,
}

impl Shape {
    /// A short name for reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delimiter => "delimiter",
            Self::DelimiterAnywhere => "delimiter-anywhere",
            Self::Immediate => "immediate",
        }
    }
}

/// The name of a case mode for reports.
pub const fn case_name(case: CaseMode) -> &'static str {
    match case {
        CaseMode::Exact => "exact",
        CaseMode::Insensitive => "insensitive",
        CaseMode::Adaptive => "adaptive",
    }
}

/// One fixture snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureSnippet {
    /// Its position in the library.
    pub index: usize,
    /// Its id.
    pub id: Id,
    /// The abbreviation as stored.
    pub abbreviation: String,
    /// Trigger and word start.
    pub shape: Shape,
    /// The case mode.
    pub case: CaseMode,
    /// The plain-text body.
    pub body: String,
}

impl FixtureSnippet {
    /// The core snippet in `folder`, created at `now`.
    pub fn to_snippet(&self, folder: Id, now: Timestamp) -> Result<Snippet, ModelError> {
        let mut s = Snippet::new_defaults(
            SyncMeta::new(self.id, now),
            folder,
            Abbreviation::new(&self.abbreviation)?,
            "",
            self.body.clone(),
            None,
        );
        s.case_mode = self.case;
        match self.shape {
            Shape::Delimiter => {
                s.trigger = Trigger::Delimiter;
                s.word_start = true;
            }
            Shape::DelimiterAnywhere => {
                s.trigger = Trigger::Delimiter;
                s.word_start = false;
            }
            Shape::Immediate => {
                s.trigger = Trigger::Immediate;
                s.word_start = false;
            }
        }
        Ok(s)
    }

    /// What the expansion types when the abbreviation was typed as `typed` (adaptive case,
    /// `iamb_core::matcher::CasePattern`).
    pub fn expansion(&self, typed: &str) -> String {
        if self.case != CaseMode::Adaptive {
            return self.body.clone();
        }
        match case_pattern(typed) {
            Pattern::AsWritten => self.body.clone(),
            Pattern::Upper => self.body.chars().flat_map(char::to_uppercase).collect(),
            Pattern::Capitalized => {
                let mut out = String::with_capacity(self.body.len());
                let mut done = false;
                for c in self.body.chars() {
                    if !done && (c.is_uppercase() || c.is_lowercase()) {
                        out.extend(c.to_uppercase());
                        done = true;
                    } else {
                        out.push(c);
                    }
                }
                out
            }
        }
    }

    /// A case variant of the abbreviation that its case mode expands.
    pub fn typed_variant(&self, rng: &mut SplitMix64) -> String {
        match self.case {
            CaseMode::Exact => self.abbreviation.clone(),
            CaseMode::Insensitive => self
                .abbreviation
                .chars()
                .map(|c| {
                    if rng.percent(40) {
                        c.to_ascii_uppercase()
                    } else {
                        c
                    }
                })
                .collect(),
            CaseMode::Adaptive => match rng.below(3) {
                0 => self.abbreviation.clone(),
                1 => capitalize_first_letter(&self.abbreviation),
                _ => self.abbreviation.to_ascii_uppercase(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    AsWritten,
    Capitalized,
    Upper,
}

/// The engine's rule (`CasePattern::of`): capitalised when the first cased letter is upper case
/// and every other one lower case; upper when at least two cased letters are all upper case.
fn case_pattern(typed: &str) -> Pattern {
    let mut cased = typed
        .chars()
        .filter(|c| c.is_uppercase() || c.is_lowercase());
    let Some(first) = cased.next() else {
        return Pattern::AsWritten;
    };
    let (mut upper, mut lower) = (0_usize, 0_usize);
    for c in cased {
        if c.is_uppercase() {
            upper += 1;
        } else {
            lower += 1;
        }
    }
    if first.is_uppercase() && upper == 0 {
        Pattern::Capitalized
    } else if first.is_uppercase() && lower == 0 {
        Pattern::Upper
    } else {
        Pattern::AsWritten
    }
}

fn capitalize_first_letter(text: &str) -> String {
    let mut done = false;
    text.chars()
        .map(|c| {
            if !done && c.is_ascii_alphabetic() {
                done = true;
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

/// Three letters for `n` (`aaa` for 0).
fn code(n: usize) -> String {
    let letter = |k: usize| char::from(b'a' + u8::try_from(k % 26).unwrap_or(0));
    [letter(n / 676), letter(n / 26), letter(n)]
        .iter()
        .collect()
}

/// A body of plain text from the word list.
pub fn body(rng: &mut SplitMix64) -> String {
    let words = 4 + rng.index(45);
    let break_at = if rng.percent(10) {
        Some(1 + rng.index(words - 1))
    } else {
        None
    };
    let mut out = String::new();
    for i in 0..words {
        if i > 0 {
            out.push(if Some(i) == break_at { '\n' } else { ' ' });
        }
        out.push_str(WORDS[rng.index(WORDS.len())]);
    }
    if rng.percent(50) {
        out.push('.');
    }
    // Bodies of at least 20 characters.
    while out.chars().count() < 20 {
        out.push_str(" and more");
    }
    out
}

/// The fixture library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    /// The snippets, by index.
    pub snippets: Vec<FixtureSnippet>,
}

impl Library {
    /// `n` snippets from `seed` (module docs).
    pub fn generate(seed: u64, n: usize) -> Result<Self, String> {
        if n == 0 || n > MAX_LIBRARY {
            return Err(format!("the library size must be 1 to {MAX_LIBRARY}"));
        }
        let mut rng = SplitMix64::new(seed ^ 0x11B2_A7E5);
        let snippets = (0..n)
            .map(|i| {
                let shape = match i % 20 {
                    0..=11 => Shape::Delimiter,
                    12..=14 => Shape::DelimiterAnywhere,
                    _ => Shape::Immediate,
                };
                let case = match (i / 20 + i) % 3 {
                    0 => CaseMode::Exact,
                    1 => CaseMode::Insensitive,
                    _ => CaseMode::Adaptive,
                };
                let c = code(i);
                let abbreviation = match shape {
                    Shape::Delimiter => format!("v{c}"),
                    Shape::DelimiterAnywhere => format!("v{c}9"),
                    Shape::Immediate => format!(";k{c}"),
                };
                let mut id = [0_u8; 16];
                id[..4].copy_from_slice(b"soak");
                id[8..].copy_from_slice(&u64::try_from(i).unwrap_or(0).to_be_bytes());
                FixtureSnippet {
                    index: i,
                    id: Id::from_bytes(id),
                    abbreviation,
                    shape,
                    case,
                    body: body(&mut rng),
                }
            })
            .collect();
        Ok(Self { snippets })
    }

    /// True when typing `text` followed by a space, after a space, could fire any snippet:
    /// it equals an abbreviation, ends with one that needs no word start, or contains an
    /// immediate one (all compared without case).
    pub fn would_match(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.snippets.iter().any(|s| {
            let abbr = s.abbreviation.to_lowercase();
            text == abbr
                || (s.shape == Shape::DelimiterAnywhere && text.ends_with(&abbr))
                || (s.shape == Shape::Immediate && text.contains(&abbr))
        })
    }

    /// The first snippet of `shape`, for probes.
    pub fn first_of(&self, shape: Shape) -> Option<&FixtureSnippet> {
        self.snippets.iter().find(|s| s.shape == shape)
    }
}

/// One token of a unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// An ordinary word with its punctuation, then a space.
    Word(String),
    /// Snippet `snippet` typed as `typed`, then a space.
    Abbreviation {
        /// The snippet's index.
        snippet: usize,
        /// The case variant typed.
        typed: String,
    },
}

/// A run of tokens typed into an empty field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The tokens, the first one a word.
    pub tokens: Vec<Token>,
}

/// An ordinary word: some capitalised, some followed by `,` or `.`.
pub fn word(rng: &mut SplitMix64) -> String {
    let mut w = WORDS[rng.index(WORDS.len())].to_owned();
    if rng.percent(12) {
        w = capitalize_first_letter(&w);
    }
    match rng.below(100) {
        0..=4 => w.push(','),
        5..=7 => w.push('.'),
        _ => {}
    }
    w
}

/// The next unit of the stream.
pub fn unit(rng: &mut SplitMix64, library: &Library) -> Unit {
    let n = 8 + rng.index(9);
    let mut tokens = vec![Token::Word(word(rng))];
    for _ in 1..n {
        if rng.percent(30) {
            let s = &library.snippets[rng.index(library.snippets.len())];
            tokens.push(Token::Abbreviation {
                snippet: s.index,
                typed: s.typed_variant(rng),
            });
        } else {
            tokens.push(Token::Word(word(rng)));
        }
    }
    Unit { tokens }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_is_deterministic_and_unique() {
        let a = Library::generate(5, 2000).expect("library");
        let b = Library::generate(5, 2000).expect("library");
        assert_eq!(a, b);
        let mut seen = std::collections::BTreeSet::new();
        for s in &a.snippets {
            assert!(seen.insert(s.abbreviation.to_lowercase()), "unique");
            assert!(!s.body.contains('{') && !s.body.contains('}'));
            assert!(s.body.chars().count() >= 20);
            s.to_snippet(Id::from_bytes([1; 16]), Timestamp(1))
                .expect("valid snippet");
        }
        for shape in [Shape::Delimiter, Shape::DelimiterAnywhere, Shape::Immediate] {
            for case in [CaseMode::Exact, CaseMode::Insensitive, CaseMode::Adaptive] {
                assert!(
                    a.snippets
                        .iter()
                        .any(|s| s.shape == shape && s.case == case),
                    "{shape:?} {case:?} present"
                );
            }
        }
        assert!(Library::generate(1, MAX_LIBRARY + 1).is_err());
    }

    #[test]
    fn ordinary_words_never_fire() {
        let lib = Library::generate(9, MAX_LIBRARY).expect("library");
        for w in WORDS {
            assert!(!w.starts_with('v'));
            assert!(w.chars().all(|c| c.is_ascii_lowercase()));
            assert!(!lib.would_match(w));
        }
        assert!(lib.would_match("vaaa"));
        assert!(lib.would_match("zzzvaam9"));
        assert!(lib.would_match("x;kaap"));
    }

    #[test]
    fn adaptive_case_follows_the_engine() {
        let s = FixtureSnippet {
            index: 0,
            id: Id::from_bytes([0; 16]),
            abbreviation: "vabc".to_owned(),
            shape: Shape::Delimiter,
            case: CaseMode::Adaptive,
            body: "be right back".to_owned(),
        };
        assert_eq!(s.expansion("vabc"), "be right back");
        assert_eq!(s.expansion("Vabc"), "Be right back");
        assert_eq!(s.expansion("VABC"), "BE RIGHT BACK");
        assert_eq!(s.expansion("vAbc"), "be right back");
        assert_eq!(s.expansion(";Kabc"), "Be right back");
        let exact = FixtureSnippet {
            case: CaseMode::Exact,
            ..s
        };
        assert_eq!(exact.expansion("VABC"), "be right back");
    }

    #[test]
    fn units_start_with_a_word_and_mix_abbreviations() {
        let lib = Library::generate(3, 500).expect("library");
        let mut rng = SplitMix64::new(3);
        let (mut words, mut abbrs) = (0, 0);
        for _ in 0..500 {
            let u = unit(&mut rng, &lib);
            assert!(matches!(u.tokens[0], Token::Word(_)));
            for t in &u.tokens[1..] {
                match t {
                    Token::Word(_) => words += 1,
                    Token::Abbreviation { .. } => abbrs += 1,
                }
            }
        }
        let share = abbrs * 100 / (words + abbrs);
        assert!((25..=35).contains(&share), "{share}% abbreviations");
    }
}
