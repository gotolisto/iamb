//! Property tests: random typing never crashes or false-matches.
//!
//! Random small libraries and random key sequences (characters, backspaces, clears) are run
//! through [`Matcher::lookup`] and through a deliberately naive reference written here: a linear
//! scan over all entries that applies the rules of architecture §4.4 literally on `String`s.
//! After every keystroke both must agree exactly.
#![allow(clippy::unwrap_used)]

use iamb_core::matcher::{CasePattern, Delimiters, KeyBuffer, Match, MatchEntry, Matcher};
use iamb_core::model::{ABBREVIATION_MAX_CHARS, Abbreviation, CaseMode, Id, Trigger};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, TestRunner};

/// Number of cases of the main property.
const CASES: u32 = 2_000;

/// The default delimiters, written out independently of `Delimiters::DEFAULT_CHARS`.
const DELIMS: &[char] = &[
    ' ', '\n', '\t', '.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'',
];

/// Small alphabet: mixed case, delimiters, length-changing folds (`İ`), `ß`/`ẞ`, the three
/// sigmas and a lone combining dot.
const ABBR_ALPHABET: &[char] = &[
    'a', 'a', 'b', 'b', 'A', 'B', 'ß', 'ẞ', 'İ', 'i', '\u{307}', '.', ';', 'σ', 'Σ', 'ς',
];

#[derive(Debug, Clone, Copy)]
enum Key {
    Char(char),
    Backspace,
    Clear,
}

// ---------------------------------------------------------------------------------------------
// Reference implementation
// ---------------------------------------------------------------------------------------------

/// The reference keystroke buffer: a `Vec` plus the "something fell off" flag.
#[derive(Default)]
struct RefBuffer {
    chars: Vec<char>,
    truncated: bool,
}

impl RefBuffer {
    fn apply(&mut self, key: Key) {
        match key {
            Key::Char(c) => {
                self.chars.push(c);
                if self.chars.len() > ABBREVIATION_MAX_CHARS + 2 {
                    self.chars.remove(0);
                    self.truncated = true;
                }
            }
            Key::Backspace => {
                self.chars.pop();
            }
            Key::Clear => *self = Self::default(),
        }
    }
}

/// Folding as documented: lower-case each typed character, then treat final sigma as sigma.
fn fold_typed(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().to_string())
        .collect::<String>()
        .replace('ς', "σ")
}

/// Folding of an abbreviation as documented: its fold key with final sigma as sigma.
fn fold_abbr(a: &Abbreviation) -> String {
    a.fold_key().replace('ς', "σ")
}

fn pattern(typed: &str) -> CasePattern {
    let cased: Vec<char> = typed
        .chars()
        .filter(|c| c.is_uppercase() || c.is_lowercase())
        .collect();
    if cased.iter().all(|c| c.is_lowercase()) {
        CasePattern::AsWritten
    } else if cased[0].is_uppercase() && cased[1..].iter().all(|c| c.is_lowercase()) {
        CasePattern::Capitalized
    } else if cased.len() >= 2 && cased.iter().all(|c| c.is_uppercase()) {
        CasePattern::Upper
    } else {
        CasePattern::AsWritten
    }
}

/// Keeps the first entry of every folded abbreviation.
fn reference_kept(entries: &[MatchEntry]) -> Vec<MatchEntry> {
    let mut kept: Vec<MatchEntry> = Vec::new();
    for e in entries {
        if !kept
            .iter()
            .any(|k| fold_abbr(&k.abbreviation) == fold_abbr(&e.abbreviation))
        {
            kept.push(e.clone());
        }
    }
    kept
}

fn reference_lookup(kept: &[MatchEntry], buf: &RefBuffer) -> Option<Match> {
    let b = &buf.chars;
    let &last = b.last()?;
    // (typed_chars, is_immediate, match)
    let mut best: Option<(usize, bool, Match)> = None;
    for e in kept {
        let (end, delimiter) = match e.trigger {
            Trigger::Immediate => (b.len(), None),
            Trigger::Delimiter if DELIMS.contains(&last) => (b.len() - 1, Some(last)),
            Trigger::Delimiter => continue,
        };
        for k in 1..=end {
            let start = end - k;
            let typed: String = b[start..end].iter().collect();
            let case_ok = match e.case_mode {
                CaseMode::Exact => typed == e.abbreviation.as_str(),
                CaseMode::Insensitive | CaseMode::Adaptive => {
                    fold_typed(&typed) == fold_abbr(&e.abbreviation)
                }
            };
            if !case_ok {
                continue;
            }
            if e.word_start {
                let boundary = if start == 0 {
                    !buf.truncated
                } else {
                    b[start - 1].is_whitespace() || DELIMS.contains(&b[start - 1])
                };
                if !boundary {
                    continue;
                }
            }
            let case = if e.case_mode == CaseMode::Adaptive {
                pattern(&typed)
            } else {
                CasePattern::AsWritten
            };
            let immediate = e.trigger == Trigger::Immediate;
            let better = match best {
                None => true,
                Some((bk, bimm, _)) => k > bk || (k == bk && immediate && !bimm),
            };
            if better {
                best = Some((
                    k,
                    immediate,
                    Match {
                        snippet: e.snippet,
                        typed_chars: k,
                        delimiter,
                        case,
                    },
                ));
            }
        }
    }
    best.map(|(_, _, m)| m)
}

// ---------------------------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------------------------

fn arb_trigger() -> impl Strategy<Value = Trigger> {
    prop_oneof![Just(Trigger::Immediate), Just(Trigger::Delimiter)]
}

fn arb_case() -> impl Strategy<Value = CaseMode> {
    prop_oneof![
        Just(CaseMode::Exact),
        Just(CaseMode::Insensitive),
        Just(CaseMode::Adaptive)
    ]
}

fn arb_entry(abbr: impl Strategy<Value = String>) -> impl Strategy<Value = MatchEntry> {
    (
        abbr.prop_filter_map("valid abbreviation", |s| Abbreviation::new(&s).ok()),
        arb_trigger(),
        arb_case(),
        any::<bool>(),
    )
        .prop_map(
            |(abbreviation, trigger, case_mode, word_start)| MatchEntry {
                snippet: Id::from_bytes([0; 16]),
                abbreviation,
                trigger,
                case_mode,
                word_start,
            },
        )
}

fn number(mut entries: Vec<MatchEntry>) -> Vec<MatchEntry> {
    for (i, e) in entries.iter_mut().enumerate() {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&u64::try_from(i + 1).unwrap().to_be_bytes());
        e.snippet = Id::from_bytes(bytes);
    }
    entries
}

fn small_abbr() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(ABBR_ALPHABET), 1..=4)
        .prop_map(|v| v.into_iter().collect())
}

fn arb_keys(pool: Vec<char>, max: usize) -> impl Strategy<Value = Vec<Key>> {
    prop::collection::vec(
        prop_oneof![
            20 => prop::sample::select(pool).prop_map(Key::Char),
            3 => Just(Key::Backspace),
            1 => Just(Key::Clear),
        ],
        0..=max,
    )
}

/// How a generated key sequence spells a whole library abbreviation.
#[derive(Debug, Clone, Copy)]
enum Spelling {
    AsWritten,
    Upper,
    Lower,
    Capitalized,
}

/// Inserts the abbreviation of `entries[index]`, spelled as `spelling`, into `out`.
fn spell(
    out: &mut Vec<Key>,
    entries: &[MatchEntry],
    index: prop::sample::Index,
    spelling: Spelling,
) {
    let text = entries[index.index(entries.len())].abbreviation.as_str();
    let spelled = match spelling {
        Spelling::AsWritten => text.to_owned(),
        Spelling::Upper => text.to_uppercase(),
        Spelling::Lower => text.to_lowercase(),
        Spelling::Capitalized => {
            let mut chars = text.chars();
            chars.next().map_or_else(String::new, |first| {
                first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect()
            })
        }
    };
    out.extend(spelled.chars().map(Key::Char));
}

/// A small library over the small alphabet, and keys over the same alphabet plus space and an
/// ordinary letter, with whole abbreviations (in several spellings) mixed in.
fn small_case() -> impl Strategy<Value = (Vec<MatchEntry>, Vec<Key>)> {
    let mut pool: Vec<char> = ABBR_ALPHABET.to_vec();
    pool.extend([' ', ' ', 'x', 'a', 'b']);
    let spelling = prop_oneof![
        Just(Spelling::AsWritten),
        Just(Spelling::Upper),
        Just(Spelling::Lower),
        Just(Spelling::Capitalized),
    ];
    (
        prop::collection::vec(arb_entry(small_abbr()), 1..=8).prop_map(number),
        arb_keys(pool, 100),
        prop::collection::vec(
            (any::<prop::sample::Index>(), spelling, 0..=100_usize),
            0..=8,
        ),
    )
        .prop_map(|(entries, keys, inserts)| {
            // Splice whole abbreviations in at the chosen positions, keeping at most 100 keys
            // from the random part plus the inserted abbreviations (each at most 4 characters).
            let mut out = Vec::with_capacity(keys.len() + 64);
            let mut inserts = inserts;
            inserts.sort_by_key(|i| i.2);
            let mut next = inserts.into_iter().peekable();
            for (pos, key) in keys.iter().enumerate() {
                while let Some((index, spelling, _)) = next.next_if(|i| i.2 <= pos) {
                    spell(&mut out, &entries, index, spelling);
                }
                out.push(*key);
            }
            for (index, spelling, _) in next {
                spell(&mut out, &entries, index, spelling);
            }
            (entries, out)
        })
}

/// A library of arbitrary Unicode abbreviations; keys drawn from their characters (in both
/// cases), some arbitrary characters, and the delimiters.
fn unicode_case() -> impl Strategy<Value = (Vec<MatchEntry>, Vec<Key>)> {
    prop::collection::vec(arb_entry("\\PC{1,6}"), 1..=6)
        .prop_map(number)
        .prop_flat_map(|entries| {
            let mut pool: Vec<char> = DELIMS.to_vec();
            for e in &entries {
                for c in e.abbreviation.as_str().chars() {
                    pool.push(c);
                    pool.extend(c.to_uppercase());
                    pool.extend(c.to_lowercase());
                }
            }
            let keys = prop::collection::vec(
                prop_oneof![
                    20 => prop::sample::select(pool).prop_map(Key::Char),
                    2 => any::<char>().prop_map(Key::Char),
                    3 => Just(Key::Backspace),
                    1 => Just(Key::Clear),
                ],
                0..=100,
            );
            (Just(entries), keys)
        })
}

/// Runs both implementations over `keys`. Returns the number of keystrokes that matched, or a
/// description of the first disagreement.
fn run(entries: &[MatchEntry], keys: &[Key]) -> Result<usize, String> {
    let matcher = Matcher::build(entries.iter().cloned(), Delimiters::default());
    let kept = reference_kept(entries);
    if matcher.len() != kept.len() || matcher.duplicates().len() != entries.len() - kept.len() {
        return Err(format!(
            "kept {} (duplicates {}), reference kept {}",
            matcher.len(),
            matcher.duplicates().len(),
            kept.len()
        ));
    }
    let mut buf = KeyBuffer::new();
    let mut model = RefBuffer::default();
    let mut hits = 0;
    for (step, &key) in keys.iter().enumerate() {
        match key {
            Key::Char(c) => buf.push(c),
            Key::Backspace => buf.backspace(),
            Key::Clear => buf.clear(),
        }
        model.apply(key);
        if buf.len() != model.chars.len() {
            return Err(format!("step {step}: buffer length {}", buf.len()));
        }
        if !matches!(key, Key::Char(_)) {
            continue;
        }
        let got = matcher.lookup(&buf);
        let want = reference_lookup(&kept, &model);
        if got != want {
            return Err(format!(
                "step {step}: buffer {:?} truncated {}: matcher {got:?} / {:?}, reference \
                 {want:?} / {:?}",
                model.chars,
                model.truncated,
                got.map(|m| m.delimiter),
                want.map(|m| m.delimiter),
            ));
        }
        hits += usize::from(got.is_some());
    }
    Ok(hits)
}

proptest! {
    #![proptest_config(Config::with_cases(CASES))]

    #[test]
    fn matcher_agrees_with_reference((entries, keys) in small_case()) {
        let r = run(&entries, &keys);
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

proptest! {
    #![proptest_config(Config::with_cases(500))]

    #[test]
    fn matcher_agrees_with_reference_on_any_unicode((entries, keys) in unicode_case()) {
        let r = run(&entries, &keys);
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

/// Guards against a vacuous property: the generators must produce plenty of matches of every
/// kind, overflow the buffer, and hit duplicates.
#[test]
fn generators_exercise_matches() {
    let mut runner = TestRunner::deterministic();
    let strategy = small_case();
    let (mut keystrokes, mut hits, mut cases_with_hits, mut dup_cases) = (0, 0, 0, 0);
    for _ in 0..500 {
        let (entries, keys) = strategy.new_tree(&mut runner).unwrap().current();
        let h = run(&entries, &keys).unwrap();
        keystrokes += keys.len();
        hits += h;
        cases_with_hits += usize::from(h > 0);
        let m = Matcher::build(entries.iter().cloned(), Delimiters::default());
        dup_cases += usize::from(!m.duplicates().is_empty());
    }
    assert!(
        hits * 25 > keystrokes,
        "hits {hits} of {keystrokes} keystrokes"
    );
    eprintln!("hits {hits} of {keystrokes}, cases with hits {cases_with_hits}, dup {dup_cases}");
    assert!(cases_with_hits > 300, "cases with hits {cases_with_hits}");
    assert!(dup_cases > 10, "cases with duplicates {dup_cases}");
}

/// The reference itself: a few hand-checked answers, so a bug shared by both is less likely.
#[test]
fn reference_sanity() {
    let e = |abbr: &str, trigger, case_mode, word_start| MatchEntry {
        snippet: Id::from_bytes([1; 16]),
        abbreviation: Abbreviation::new(abbr).unwrap(),
        trigger,
        case_mode,
        word_start,
    };
    let buf = |s: &str| RefBuffer {
        chars: s.chars().collect(),
        truncated: false,
    };
    let lib = [e("İb", Trigger::Delimiter, CaseMode::Adaptive, true)];
    let got = reference_lookup(&lib, &buf("x İB.")).unwrap();
    assert_eq!(
        (got.typed_chars, got.delimiter, got.case),
        (2, Some('.'), CasePattern::Upper)
    );
    assert_eq!(reference_lookup(&lib, &buf("xİB.")), None);
    assert_eq!(pattern("Ab"), CasePattern::Capitalized);
    assert_eq!(pattern("1"), CasePattern::AsWritten);
}
