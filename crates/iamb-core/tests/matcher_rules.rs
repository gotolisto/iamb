//! Table tests for the abbreviation matcher rules (architecture §4.4).
#![allow(clippy::unwrap_used)]

use iamb_core::matcher::{
    CasePattern, Delimiters, Duplicate, KEY_BUFFER_CAPACITY, KeyBuffer, Match, MatchEntry, Matcher,
    Shadowing,
};
use iamb_core::model::{
    ABBREVIATION_MAX_CHARS, Abbreviation, CaseMode, Id, Snippet, SyncMeta, Timestamp, Trigger,
};

use CaseMode::{Adaptive, Exact, Insensitive};
use Trigger::{Delimiter, Immediate};

fn id(n: u8) -> Id {
    Id::from_bytes([n; 16])
}

fn entry(n: u8, abbr: &str, trigger: Trigger, case_mode: CaseMode, word_start: bool) -> MatchEntry {
    MatchEntry {
        snippet: id(n),
        abbreviation: Abbreviation::new(abbr).unwrap(),
        trigger,
        case_mode,
        word_start,
    }
}

/// Immediate, insensitive, no word start (the defaults for immediate mode).
fn imm(n: u8, abbr: &str) -> MatchEntry {
    entry(n, abbr, Immediate, Insensitive, false)
}

/// Delimiter, insensitive, word start (the defaults for delimiter mode).
fn del(n: u8, abbr: &str) -> MatchEntry {
    entry(n, abbr, Delimiter, Insensitive, true)
}

fn matcher(entries: impl IntoIterator<Item = MatchEntry>) -> Matcher {
    Matcher::build(entries, Delimiters::default())
}

fn push_all(buf: &mut KeyBuffer, text: &str) {
    for c in text.chars() {
        buf.push(c);
    }
}

/// Types `text` into a fresh buffer, looking up after every character. Returns every
/// `(char index, match)` that fired.
fn fired(m: &Matcher, text: &str) -> Vec<(usize, Match)> {
    let mut buf = KeyBuffer::new();
    let mut out = Vec::new();
    for (i, c) in text.chars().enumerate() {
        buf.push(c);
        if let Some(hit) = m.lookup(&buf) {
            out.push((i, hit));
        }
    }
    out
}

/// The lookup after the last character of `text`, typed into a fresh buffer.
fn last(m: &Matcher, text: &str) -> Option<Match> {
    let mut buf = KeyBuffer::new();
    push_all(&mut buf, text);
    m.lookup(&buf)
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "compared with `Matcher::lookup`, which returns an `Option`"
)]
fn hit(n: u8, typed_chars: usize, delimiter: Option<char>, case: CasePattern) -> Option<Match> {
    Some(Match {
        snippet: id(n),
        typed_chars,
        delimiter,
        case,
    })
}

fn plain(n: u8, typed_chars: usize, delimiter: Option<char>) -> Option<Match> {
    hit(n, typed_chars, delimiter, CasePattern::AsWritten)
}

// ---------------------------------------------------------------------------------------------
// Triggers and delimiters
// ---------------------------------------------------------------------------------------------

#[test]
fn immediate_fires_on_the_last_character() {
    let m = matcher([imm(1, "brb")]);
    let fires = fired(&m, "brb");
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].0, 2);
    assert_eq!(Some(fires[0].1), plain(1, 3, None));
    assert_eq!(fires[0].1.backspaces(), 3);
    assert_eq!(last(&m, "br"), None);
    assert_eq!(
        last(&m, "brb "),
        None,
        "the space ends no immediate abbreviation"
    );
}

#[test]
fn delimiter_fires_only_when_a_delimiter_follows() {
    let m = matcher([del(1, "brb")]);
    assert_eq!(last(&m, "brb"), None);
    let found = last(&m, "brb ").unwrap();
    assert_eq!(Some(found), plain(1, 3, Some(' ')));
    assert_eq!(found.backspaces(), 4);
    assert_eq!(last(&m, "brbx"), None);
    assert_eq!(
        last(&m, "brb  "),
        None,
        "only the character right after counts"
    );
}

#[test]
fn every_default_delimiter_fires_and_others_do_not() {
    let m = matcher([del(1, "brb")]);
    assert_eq!(Delimiters::default().iter().count(), 12);
    for d in Delimiters::DEFAULT_CHARS {
        assert_eq!(last(&m, &format!("brb{d}")), plain(1, 3, Some(d)), "{d:?}");
    }
    for c in [
        'x', '-', '/', '(', '[', '{', '@', '\r', '\u{a0}', '\u{3000}', '0', '\n', '\t',
    ] {
        assert_eq!(last(&m, &format!("brb{c}")), None, "{c:?}");
    }
}

#[test]
fn custom_delimiter_set() {
    let set = Delimiters::new(['-', '-', '+']);
    assert_eq!(set.iter().collect::<Vec<_>>(), ['+', '-']);
    assert!(!set.contains(' '));
    let m = Matcher::build([del(1, "brb")], set);
    assert_eq!(last(&m, "brb-"), plain(1, 3, Some('-')));
    assert_eq!(last(&m, "brb "), None);
    // Whitespace still counts as a word boundary even when it is not a delimiter.
    assert_eq!(last(&m, "ok brb+"), plain(1, 3, Some('+')));
    assert_eq!(last(&m, "ok-brb+"), plain(1, 3, Some('+')));
    assert_eq!(last(&m, "ok.brb+"), None);
}

// ---------------------------------------------------------------------------------------------
// Word start
// ---------------------------------------------------------------------------------------------

#[test]
fn word_start_on_rejects_mid_word_typing() {
    let d = matcher([del(1, "brb")]);
    assert_eq!(last(&d, "xbrb "), None);
    assert_eq!(last(&d, "x brb "), plain(1, 3, Some(' ')));
    assert_eq!(
        last(&d, "brb "),
        plain(1, 3, Some(' ')),
        "buffer start counts"
    );
    assert_eq!(
        last(&d, "x)brb "),
        plain(1, 3, Some(' ')),
        "a delimiter counts"
    );
    assert_eq!(last(&d, "x\tbrb."), plain(1, 3, Some('.')));
    assert_eq!(
        last(&d, "x\u{a0}brb "),
        plain(1, 3, Some(' ')),
        "any whitespace"
    );
    assert_eq!(last(&d, "x(brb "), None, "'(' is not a delimiter");

    let i = matcher([entry(2, "brb", Immediate, Insensitive, true)]);
    assert!(fired(&i, "xbrb").is_empty());
    assert_eq!(last(&i, "x.brb"), plain(2, 3, None));
    assert_eq!(last(&i, "brb"), plain(2, 3, None));
}

#[test]
fn word_start_off_fires_mid_word() {
    let i = matcher([imm(1, "brb")]);
    assert_eq!(last(&i, "xbrb"), plain(1, 3, None));
    let d = matcher([entry(2, "brb", Delimiter, Insensitive, false)]);
    assert_eq!(last(&d, "xbrb "), plain(2, 3, Some(' ')));
}

#[test]
fn word_start_failure_lets_a_shorter_candidate_win() {
    let m = matcher([del(1, "brb"), entry(2, "rb", Delimiter, Insensitive, false)]);
    assert_eq!(last(&m, "xbrb "), plain(2, 2, Some(' ')));
    assert_eq!(last(&m, " brb "), plain(1, 3, Some(' ')));
}

// ---------------------------------------------------------------------------------------------
// Case modes
// ---------------------------------------------------------------------------------------------

#[test]
fn exact_compares_original_characters() {
    let m = matcher([entry(1, "Sig", Immediate, Exact, false)]);
    assert_eq!(last(&m, "Sig"), plain(1, 3, None));
    assert_eq!(last(&m, "sig"), None);
    assert_eq!(last(&m, "SIG"), None);
    assert_eq!(last(&m, "xSig"), plain(1, 3, None));
}

#[test]
fn exact_failure_lets_a_shorter_candidate_win() {
    let m = matcher([entry(1, "Sig", Immediate, Exact, false), imm(2, "ig")]);
    assert_eq!(last(&m, "Sig"), plain(1, 3, None));
    assert_eq!(last(&m, "sig"), plain(2, 2, None));
}

#[test]
fn insensitive_ignores_case_and_reports_as_written() {
    let m = matcher([entry(1, "Sig", Delimiter, Insensitive, true)]);
    for typed in ["sig ", "SIG ", "Sig ", "sIg "] {
        assert_eq!(last(&m, typed), plain(1, 3, Some(' ')), "{typed:?}");
    }
}

#[test]
fn adaptive_reports_the_typed_case_pattern() {
    use CasePattern::{AsWritten, Capitalized, Upper};
    let m = matcher([
        entry(1, "sig", Delimiter, Adaptive, true),
        entry(2, "a1", Immediate, Adaptive, false),
        entry(3, "x1y", Immediate, Adaptive, false),
        entry(4, "SoX", Immediate, Adaptive, true),
        entry(5, "42;", Immediate, Adaptive, false),
    ]);
    for (typed, want) in [
        ("sig ", hit(1, 3, Some(' '), AsWritten)),
        ("Sig ", hit(1, 3, Some(' '), Capitalized)),
        ("SIG ", hit(1, 3, Some(' '), Upper)),
        ("sIG ", hit(1, 3, Some(' '), AsWritten)),
        ("SiG ", hit(1, 3, Some(' '), AsWritten)),
        ("sIg ", hit(1, 3, Some(' '), AsWritten)),
        ("a1", hit(2, 2, None, AsWritten)),
        ("A1", hit(2, 2, None, Capitalized)),
        ("x1y", hit(3, 3, None, AsWritten)),
        ("X1Y", hit(3, 3, None, Upper)),
        ("X1y", hit(3, 3, None, Capitalized)),
        ("x1Y", hit(3, 3, None, AsWritten)),
        (" sox", hit(4, 3, None, AsWritten)),
        (" SOX", hit(4, 3, None, Upper)),
        ("xSOX", None),
        ("42;", hit(5, 3, None, AsWritten)),
    ] {
        assert_eq!(last(&m, typed), want, "{typed:?}");
    }
}

#[test]
fn non_adaptive_modes_always_report_as_written() {
    let m = matcher([entry(1, "SIG", Immediate, Exact, false), imm(2, "abc")]);
    assert_eq!(last(&m, "SIG").unwrap().case, CasePattern::AsWritten);
    assert_eq!(last(&m, "ABC").unwrap().case, CasePattern::AsWritten);
}

// ---------------------------------------------------------------------------------------------
// Several candidates
// ---------------------------------------------------------------------------------------------

#[test]
fn longest_abbreviation_wins() {
    let d = matcher([
        entry(1, "sig", Delimiter, Insensitive, false),
        del(2, "wsig"),
    ]);
    assert_eq!(last(&d, "wsig "), plain(2, 4, Some(' ')));
    assert_eq!(last(&d, "a sig "), plain(1, 3, Some(' ')));
    assert_eq!(last(&d, "xsig "), plain(1, 3, Some(' ')));

    // Build order does not matter.
    let i = matcher([imm(2, "wsig"), imm(1, "sig")]);
    assert_eq!(fired(&i, "wsig"), [(3, plain(2, 4, None).unwrap())]);
    assert_eq!(fired(&i, "xsig"), [(3, plain(1, 3, None).unwrap())]);
}

#[test]
fn immediate_prefix_fires_first_and_shadows_the_longer() {
    let m = matcher([imm(1, "ad"), del(2, "add"), imm(3, "adx")]);
    let fires = fired(&m, "add ");
    assert_eq!(fires[0], (1, plain(1, 2, None).unwrap()));
    assert_eq!(
        m.shadowed(),
        [
            Shadowing {
                by: id(1),
                hidden: id(2)
            },
            Shadowing {
                by: id(1),
                hidden: id(3)
            },
        ]
    );
}

#[test]
fn shadowing_report() {
    let m = matcher([
        del(1, "ad"),                           // delimiter: shadows nothing
        imm(2, "Ab"),                           // shadows "abc" (case is ignored)
        del(3, "abc"),                          //
        imm(4, "i"),                            // shadows "i\u{307}x", not "İy"
        del(5, "i\u{307}x"),                    //
        del(6, "İy"),                           //
        entry(7, "Q", Immediate, Exact, false), // shadows "q1" even though exact
        entry(8, "q1", Delimiter, Insensitive, true),
        imm(9, "zz"), // no longer abbreviation
    ]);
    assert_eq!(
        m.shadowed(),
        [
            Shadowing {
                by: id(2),
                hidden: id(3)
            },
            Shadowing {
                by: id(4),
                hidden: id(5)
            },
            Shadowing {
                by: id(7),
                hidden: id(8)
            },
        ]
    );
    // "İy" really is reachable: typing 'İ' does not fire "i".
    assert_eq!(last(&m, "İy "), plain(6, 2, Some(' ')));
}

#[test]
fn immediate_and_delimiter_on_the_same_keystroke() {
    // Immediate longer: immediate wins.
    let m = matcher([imm(1, "x."), entry(2, "x", Delimiter, Insensitive, false)]);
    assert_eq!(last(&m, "x."), plain(1, 2, None));
    // Equal length: immediate wins.
    let m = matcher([imm(1, "b."), entry(2, "ab", Delimiter, Insensitive, false)]);
    assert_eq!(last(&m, "ab."), plain(1, 2, None));
    let m = matcher([imm(1, "."), entry(2, "x", Delimiter, Insensitive, false)]);
    assert_eq!(last(&m, "x."), plain(1, 1, None));
    // Delimiter longer: delimiter wins.
    let m = matcher([imm(1, "c."), entry(2, "abc", Delimiter, Insensitive, false)]);
    assert_eq!(last(&m, "abc."), plain(2, 3, Some('.')));
}

// ---------------------------------------------------------------------------------------------
// Editing and the buffer
// ---------------------------------------------------------------------------------------------

#[test]
fn backspace_then_retype() {
    let m = matcher([imm(1, "brb"), del(2, "omw")]);
    let mut buf = KeyBuffer::new();
    push_all(&mut buf, "brx");
    assert_eq!(m.lookup(&buf), None);
    buf.backspace();
    buf.push('b');
    assert_eq!(m.lookup(&buf), plain(1, 3, None));

    buf.clear();
    buf.backspace(); // on empty: no-op
    assert_eq!(m.lookup(&buf), None);
    push_all(&mut buf, "omwx");
    buf.backspace();
    buf.push(' ');
    assert_eq!(m.lookup(&buf), plain(2, 3, Some(' ')));

    buf.clear();
    push_all(&mut buf, "omw");
    for _ in 0..10 {
        buf.backspace();
    }
    assert!(buf.is_empty());
    push_all(&mut buf, "omw ");
    assert_eq!(m.lookup(&buf), plain(2, 3, Some(' ')));
}

#[test]
fn buffer_overflow_keeps_the_latest_characters() {
    let m = matcher([del(1, "brb"), imm(2, "qq")]);
    let mut buf = KeyBuffer::new();
    for _ in 0..200 {
        buf.push('z');
        assert_eq!(m.lookup(&buf), None);
    }
    assert_eq!(buf.len(), KEY_BUFFER_CAPACITY);
    push_all(&mut buf, " brb ");
    assert_eq!(m.lookup(&buf), plain(1, 3, Some(' ')));
    push_all(&mut buf, "qq");
    assert_eq!(m.lookup(&buf), plain(2, 2, None));
    assert_eq!(buf.len(), KEY_BUFFER_CAPACITY);
}

#[test]
fn longest_abbreviation_64_chars() {
    let long: String = "abcdefgh".repeat(8);
    assert_eq!(long.chars().count(), ABBREVIATION_MAX_CHARS);
    let m = matcher([
        entry(1, &long, Immediate, Insensitive, true),
        del(2, &long.to_uppercase()[1..]),
    ]);
    // The second abbreviation (63 chars) is distinct and delimiter-triggered.
    assert_eq!(m.len(), 2);

    // Immediate with word start: the preceding space still fits (1 + 64 = 65 of 66).
    let mut buf = KeyBuffer::new();
    for _ in 0..100 {
        buf.push('z');
    }
    buf.push(' ');
    push_all(&mut buf, &long);
    assert_eq!(m.lookup(&buf), plain(1, 64, None));

    // Mid-word: no word start.
    buf.clear();
    push_all(&mut buf, "z");
    push_all(&mut buf, &long);
    assert_eq!(m.lookup(&buf), None);
}

#[test]
fn delimiter_64_char_abbreviation_with_word_start_fires_after_other_text() {
    let long: String = "0123456789".repeat(7)[..ABBREVIATION_MAX_CHARS].to_owned();
    let m = matcher([
        del(1, &long),
        entry(2, &long.replace('0', "x"), Delimiter, Exact, false),
    ]);
    // From a fresh buffer: 64 + delimiter = 65 characters, the abbreviation starts the buffer.
    assert_eq!(last(&m, &format!("{long} ")), plain(1, 64, Some(' ')));
    // After other typing, the buffer (66 characters) still holds the character before the
    // abbreviation, so a word-start snippet fires after a boundary (review 1.9 finding 6)...
    assert_eq!(last(&m, &format!("ok {long} ")), plain(1, 64, Some(' ')));
    // ...and not mid-word.
    assert_eq!(last(&m, &format!("ok{long} ")), None);
    // Even after a long run of typing (the buffer full and truncated), the character before a
    // 64-character abbreviation is still held, so the rule can always be decided.
    let mut buf = KeyBuffer::new();
    push_all(&mut buf, &format!("{} {long} ", "z".repeat(100)));
    assert_eq!(buf.len(), KEY_BUFFER_CAPACITY);
    assert_eq!(m.lookup(&buf), plain(1, 64, Some(' ')));
    // Without word start it fires regardless.
    let x = long.replace('0', "x");
    assert_eq!(last(&m, &format!("ok {x} ")), plain(2, 64, Some(' ')));
}

// ---------------------------------------------------------------------------------------------
// Unicode
// ---------------------------------------------------------------------------------------------

#[test]
fn multi_byte_and_emoji_abbreviations() {
    let m = matcher([
        imm(1, "→ok"),
        imm(2, "🙂x"),
        del(3, "日本"),
        imm(4, "👍🏽"),
        entry(5, "😀", Delimiter, Exact, false),
        entry(6, "é", Delimiter, Adaptive, true),
    ]);
    assert_eq!(last(&m, "a→ok"), plain(1, 3, None));
    assert_eq!(last(&m, "🙂x"), plain(2, 2, None));
    assert_eq!(last(&m, "🙂X"), plain(2, 2, None));
    assert_eq!(
        last(&m, "今 日本。"),
        None,
        "ideographic full stop is not a delimiter"
    );
    assert_eq!(last(&m, "今 日本!"), plain(3, 2, Some('!')));
    assert_eq!(
        last(&m, "👍🏽"),
        plain(4, 2, None),
        "two scalars, two Char events"
    );
    assert_eq!(last(&m, "👍"), None);
    assert_eq!(last(&m, "x😀 "), plain(5, 1, Some(' ')));
    assert_eq!(
        last(&m, "É "),
        hit(6, 1, Some(' '), CasePattern::Capitalized)
    );
}

#[test]
fn folding_that_changes_length_counts_typed_characters() {
    // 'İ' folds to "i" + U+0307 (two characters).
    let m = matcher([imm(1, "İst")]);
    assert_eq!(last(&m, "İst"), plain(1, 3, None));
    assert_eq!(last(&m, "İST"), plain(1, 3, None));
    assert_eq!(
        last(&m, "i\u{307}st"),
        plain(1, 4, None),
        "the folded spelling, typed"
    );
    assert_eq!(last(&m, "ist"), None);

    // The abbreviation in its folded spelling matches the one-character form too.
    let m = matcher([del(2, "i\u{307}st")]);
    assert_eq!(last(&m, "İST "), plain(2, 3, Some(' ')));

    // Exact compares the original characters.
    let m = matcher([entry(3, "İst", Immediate, Exact, false)]);
    assert_eq!(last(&m, "İst"), plain(3, 3, None));
    assert_eq!(last(&m, "i\u{307}st"), None);

    // A match never ends inside the folding of one typed character.
    let m = matcher([imm(4, "\u{307}")]);
    assert_eq!(last(&m, "İ"), None);
    assert_eq!(last(&m, "i\u{307}"), plain(4, 1, None));

    // 'ẞ' folds to 'ß'; no full case folding to "ss".
    let m = matcher([entry(5, "straße", Delimiter, Adaptive, true)]);
    assert_eq!(
        last(&m, "STRAẞE "),
        hit(5, 6, Some(' '), CasePattern::Upper)
    );
    assert_eq!(
        last(&m, "Straße "),
        hit(5, 6, Some(' '), CasePattern::Capitalized)
    );
    assert_eq!(last(&m, "STRASSE "), None);
    let m = matcher([imm(6, "ẞ")]);
    assert_eq!(last(&m, "ß"), plain(6, 1, None));
}

#[test]
fn greek_final_sigma_matches_sigma() {
    let m = matcher([entry(1, "ΟΔΟΣ", Delimiter, Adaptive, true)]);
    assert_eq!(
        last(&m, "οδος "),
        hit(1, 4, Some(' '), CasePattern::AsWritten)
    );
    assert_eq!(
        last(&m, "οδοσ "),
        hit(1, 4, Some(' '), CasePattern::AsWritten)
    );
    assert_eq!(last(&m, "ΟΔΟΣ "), hit(1, 4, Some(' '), CasePattern::Upper));
}

#[test]
fn abbreviation_containing_a_delimiter_with_immediate_trigger() {
    let m = matcher([imm(1, ";sig"), del(2, "sig"), del(3, "e.g")]);
    let fires = fired(&m, "hi ;sig");
    assert_eq!(fires, [(6, plain(1, 4, None).unwrap())]);
    // The delimiter snippet "sig" would need a delimiter after it; the immediate one has
    // already fired by then.
    assert_eq!(last(&m, "hi ;sig "), plain(2, 3, Some(' ')));
    assert_eq!(fired(&m, "e.g "), [(3, plain(3, 3, Some(' ')).unwrap())]);
}

// ---------------------------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------------------------

#[test]
fn duplicates_keep_the_first_entry() {
    let m = matcher([
        del(1, "brb"),
        imm(2, "BRB"),
        del(3, "omw"),
        entry(4, "Brb", Delimiter, Exact, false),
        del(5, "ας"),
        del(6, "ΑΣ"),
        del(7, "ασ"),
    ]);
    assert_eq!(m.len(), 3);
    assert!(!m.is_empty());
    assert_eq!(
        m.duplicates(),
        [
            Duplicate {
                kept: id(1),
                dropped: id(2)
            },
            Duplicate {
                kept: id(1),
                dropped: id(4)
            },
            Duplicate {
                kept: id(5),
                dropped: id(6)
            },
            Duplicate {
                kept: id(5),
                dropped: id(7)
            },
        ]
    );
    assert_eq!(last(&m, "BRB"), None, "the immediate duplicate was dropped");
    assert_eq!(last(&m, "BRB "), plain(1, 3, Some(' ')));
}

#[test]
fn empty_matcher_and_empty_buffer() {
    let m = matcher([]);
    assert!(m.is_empty());
    assert_eq!(last(&m, "anything "), None);
    assert!(m.shadowed().is_empty());
    let m = matcher([del(1, "brb")]);
    assert_eq!(m.lookup(&KeyBuffer::new()), None);
    assert_eq!(last(&m, " "), None);
    assert_eq!(m.delimiters(), &Delimiters::default());
}

#[test]
fn entry_from_snippet() {
    let meta = SyncMeta::new(id(9), Timestamp(0));
    let s = Snippet::new_defaults(
        meta,
        id(8),
        Abbreviation::new("brb").unwrap(),
        "label",
        "be right back",
        None,
    );
    let e = MatchEntry::from_snippet(&s);
    assert_eq!(e, entry(9, "brb", Delimiter, Insensitive, true));
}

#[test]
fn match_debug_does_not_print_the_typed_delimiter() {
    let m = matcher([del(1, "brb")]);
    let found = last(&m, "brb;").unwrap();
    let text = format!("{found:?}");
    assert!(text.contains("Some(..)"), "{text}");
    assert!(!text.contains(';'), "{text}");
    let text = format!("{m:?}");
    assert!(text.starts_with("Matcher"), "{text}");
}
