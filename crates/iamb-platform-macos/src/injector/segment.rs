//! Where text may be cut between two key events without splitting a character the user sees.
//!
//! `CGEventKeyboardSetUnicodeString` carries a short string per key event, so long text goes
//! out in chunks. A cut must not fall inside an extended grapheme cluster (UAX #29): an
//! accent, a skin tone, a variation selector, a flag's second half or the rest of a ZWJ
//! sequence would otherwise arrive in a separate event, and some applications render or
//! store the two halves apart.
//!
//! # A conservative rule, without tables
//!
//! [`Cuts`] allows a cut before a character only when both of these hold:
//!
//! - the character is one that never attaches to what comes before it, from a short list of
//!   ranges: printable ASCII, the Latin, Greek and Cyrillic letters, general punctuation and
//!   symbols, CJK, kana, Hangul syllables, fullwidth forms, the emoji blocks (without the skin
//!   tones) and regional indicators;
//! - the character before it does not attach to what follows: not a zero width joiner, not a
//!   prepended concatenation mark, not the first half of a regional-indicator pair, not a
//!   Hangul leading consonant before a syllable.
//!
//! Every cut this allows is a grapheme cluster boundary of Unicode 17; the ranges were checked
//! against the Unicode grapheme break property data, and the unit tests pin the cases the
//! injector cares about. It allows fewer cuts than UAX #29 does: Arabic, Hebrew and the Indic
//! scripts, which interleave marks with letters, are cut only at spaces and ASCII
//! punctuation, which is where text in those scripts breaks into words anyway. A run with no
//! allowed cut that is longer than one key event can carry is cut between two scalar values
//! (never inside a surrogate pair) by [`chunk`](super::chunk).
//!
//! A full UAX #29 segmenter needs the property tables of the `unicode-segmentation` crate
//! (already used by `iamb-core`); this rule exists so that the crate needs no new dependency.

/// The zero width joiner: never cut after it (emoji ZWJ sequences, `GB11`).
const ZWJ: char = '\u{200D}';

/// Characters that start a new grapheme cluster whatever came before them (unless the
/// character before glues forward). None of them has the grapheme break property `Extend`,
/// `SpacingMark` or `ZWJ`.
const STARTERS: &[(char, char)] = &[
    ('\u{20}', '\u{7E}'),       // printable ASCII
    ('\u{A0}', '\u{2FF}'),      // Latin-1, Latin Extended-A and -B, IPA, spacing modifiers
    ('\u{370}', '\u{482}'),     // Greek, Coptic, Cyrillic letters
    ('\u{48A}', '\u{52F}'),     // Cyrillic letters after the combining marks
    ('\u{2010}', '\u{2027}'),   // dashes, quotes, bullets
    ('\u{2030}', '\u{205E}'),   // per mille .. four dots
    ('\u{20A0}', '\u{20C0}'),   // currency symbols
    ('\u{2100}', '\u{2BFF}'),   // letterlike, arrows, maths, technical, symbols, dingbats
    ('\u{3000}', '\u{3029}'),   // CJK punctuation before the tone marks
    ('\u{3030}', '\u{303F}'),   // CJK punctuation after the tone marks
    ('\u{3041}', '\u{3096}'),   // hiragana (the voicing marks after it attach)
    ('\u{309B}', '\u{30FF}'),   // spacing voicing marks, katakana
    ('\u{3400}', '\u{4DBF}'),   // CJK extension A
    ('\u{4E00}', '\u{9FFF}'),   // CJK unified ideographs
    ('\u{AC00}', '\u{D7A3}'),   // Hangul syllables
    ('\u{FF01}', '\u{FF60}'),   // fullwidth forms (the halfwidth voicing marks attach)
    ('\u{1F1E6}', '\u{1F1FF}'), // regional indicators (paired, see `Cuts`)
    ('\u{1F300}', '\u{1F3FA}'), // pictographs before the skin tones
    ('\u{1F400}', '\u{1FAFF}'), // pictographs, emoticons, transport, symbols
    ('\u{20000}', '\u{3FFFF}'), // CJK extensions B and later
];

/// Characters with the grapheme break property `Prepend`: they attach to what follows
/// (`GB9b`), so no cut may follow them.
const PREPEND: &[(char, char)] = &[
    ('\u{600}', '\u{605}'),
    ('\u{6DD}', '\u{6DD}'),
    ('\u{70F}', '\u{70F}'),
    ('\u{890}', '\u{891}'),
    ('\u{8E2}', '\u{8E2}'),
    ('\u{D4E}', '\u{D4E}'),
    ('\u{110BD}', '\u{110BD}'),
    ('\u{110CD}', '\u{110CD}'),
    ('\u{111C2}', '\u{111C3}'),
    ('\u{113D1}', '\u{113D1}'),
    ('\u{1193F}', '\u{1193F}'),
    ('\u{11941}', '\u{11941}'),
    ('\u{11A84}', '\u{11A89}'),
    ('\u{11D46}', '\u{11D46}'),
    ('\u{11F02}', '\u{11F02}'),
];

fn in_ranges(c: char, ranges: &[(char, char)]) -> bool {
    ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(&c))
}

const fn is_regional_indicator(c: char) -> bool {
    matches!(c, '\u{1F1E6}'..='\u{1F1FF}')
}

/// A Hangul leading consonant (grapheme break property `L`): it joins a following syllable
/// (`GB6`).
const fn is_hangul_leading(c: char) -> bool {
    matches!(c, '\u{1100}'..='\u{115F}' | '\u{A960}'..='\u{A97C}')
}

const fn is_hangul_syllable(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}')
}

/// The cut points of a text, found one character at a time.
#[derive(Debug, Default, Clone)]
pub(crate) struct Cuts {
    prev: Option<char>,
    /// How many regional indicators end the text seen so far.
    indicators: usize,
}

impl Cuts {
    /// Cut points for a new text.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Takes the next character and says whether the text may be cut just before it. The first
    /// character of a text has nothing before it: the answer is `false`, and nothing is lost,
    /// because there is nothing to cut off.
    pub(crate) fn before(&mut self, c: char) -> bool {
        let allowed = self.prev.is_some_and(|prev| self.allowed(prev, c));
        if is_regional_indicator(c) {
            self.indicators += 1;
        } else {
            self.indicators = 0;
        }
        self.prev = Some(c);
        allowed
    }

    fn allowed(&self, prev: char, next: char) -> bool {
        if prev == ZWJ || in_ranges(prev, PREPEND) {
            return false;
        }
        if is_regional_indicator(next) && is_regional_indicator(prev) {
            // `GB12`/`GB13`: indicators pair up from the start of the run.
            return self.indicators.is_multiple_of(2);
        }
        if is_hangul_syllable(next) && is_hangul_leading(prev) {
            return false;
        }
        in_ranges(next, STARTERS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text split at every allowed cut.
    fn pieces(text: &str) -> Vec<&str> {
        let mut cuts = Cuts::new();
        let mut out = Vec::new();
        let mut start = 0;
        for (i, c) in text.char_indices() {
            if cuts.before(c) {
                out.push(&text[start..i]);
                start = i;
            }
        }
        if start < text.len() {
            out.push(&text[start..]);
        }
        out
    }

    #[test]
    fn plain_text_can_be_cut_anywhere() {
        assert_eq!(pieces("ab c"), ["a", "b", " ", "c"]);
        assert_eq!(pieces(""), Vec::<&str>::new());
        assert_eq!(pieces("x"), ["x"]);
    }

    #[test]
    fn precomposed_and_combining_accents() {
        assert_eq!(pieces("café"), ["c", "a", "f", "é"]);
        // e + COMBINING ACUTE ACCENT, and two marks on one letter.
        assert_eq!(pieces("cafe\u{301}!"), ["c", "a", "f", "e\u{301}", "!"]);
        assert_eq!(pieces("a\u{323}\u{302}b"), ["a\u{323}\u{302}", "b"]);
        // A mark after a space stays with the space.
        assert_eq!(pieces("x \u{301}y"), ["x", " \u{301}", "y"]);
    }

    #[test]
    fn emoji_sequences_stay_whole() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        assert_eq!(pieces(&format!("a{family}b")), ["a", family, "b"]);
        let waving = "\u{1F44B}\u{1F3FD}"; // skin tone
        assert_eq!(pieces(&format!("{waving}{waving}")), [waving, waving]);
        let technologist = "\u{1F469}\u{200D}\u{1F4BB}";
        assert_eq!(pieces(technologist), [technologist]);
        let heart = "\u{2764}\u{FE0F}"; // variation selector
        assert_eq!(pieces(&format!("{heart}x")), [heart, "x"]);
        let keycap = "1\u{FE0F}\u{20E3}";
        assert_eq!(pieces(&format!("{keycap}{keycap}")), [keycap, keycap]);
        let scotland = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}";
        assert_eq!(pieces(&format!("{scotland}.")), [scotland, "."]);
    }

    #[test]
    fn flags_pair_up() {
        let japan = "\u{1F1EF}\u{1F1F5}";
        let france = "\u{1F1EB}\u{1F1F7}";
        assert_eq!(pieces(&format!("{japan}{france}")), [japan, france]);
        // An odd indicator at the end is a cluster of its own.
        assert_eq!(
            pieces(&format!("{japan}{france}\u{1F1E6}")),
            [japan, france, "\u{1F1E6}"]
        );
        assert_eq!(pieces(&format!("x{japan}")), ["x", japan]);
    }

    #[test]
    fn cjk_kana_and_hangul() {
        assert_eq!(pieces("日本語"), ["日", "本", "語"]);
        assert_eq!(pieces("한국어"), ["한", "국", "어"]);
        // A conjoining jamo sequence is one cluster, and a leading consonant joins a syllable.
        assert_eq!(
            pieces("\u{1100}\u{1161}\u{11A8}a"),
            ["\u{1100}\u{1161}\u{11A8}", "a"]
        );
        assert_eq!(pieces("\u{1100}\u{AC00}"), ["\u{1100}\u{AC00}"]);
        // Kana with a combining voicing mark.
        assert_eq!(pieces("か\u{3099}き"), ["か\u{3099}", "き"]);
        // Supplementary ideographs.
        assert_eq!(pieces("\u{20000}\u{20001}"), ["\u{20000}", "\u{20001}"]);
    }

    #[test]
    fn scripts_with_marks_are_cut_only_at_spaces() {
        // Devanagari with a virama conjunct and a vowel sign: kept whole between spaces.
        let word = "क्षत्रिय";
        let spaced = format!(" {word}");
        assert_eq!(pieces(&format!("{word} {word}")), [word, spaced.as_str()]);
        // Arabic with a prepended number sign: nothing is cut after the sign.
        assert_eq!(pieces("\u{600}12"), ["\u{600}1", "2"]);
        // Nothing is cut after a joiner, whatever follows.
        assert_eq!(pieces("a\u{200D}b"), ["a\u{200D}b"]);
    }
}
