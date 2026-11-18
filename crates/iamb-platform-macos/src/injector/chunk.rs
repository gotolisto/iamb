//! Text into key-event chunks.
//!
//! macOS puts text into a key event as UTF-16 (`CGEventKeyboardSetUnicodeString`), and
//! applications read at most about 20 code units from one event. [`TextChunks`] packs text
//! into chunks of at most [`MAX_CHUNK_UNITS`] (or the strategy's smaller limit), cutting only
//! where [`Cuts`] allows, so that no grapheme cluster is split between two events. A cluster
//! longer than the limit (rare: a long run of a script with marks and no space) is cut between
//! two scalar values; a surrogate pair is never split, even with a limit of one unit.
//!
//! [`pieces`] first separates line breaks and tabs, which are sent as keys, from the runs of
//! text between them.
//!
//! A [`TextChunk`] is a fixed array, not a heap allocation, and is zeroed when dropped: it may
//! hold characters typed by the user (the compensation of §4.6) or form values (§8 rules 1
//! and 2). Its `Debug` shows the length only.

use std::fmt;
use std::hint::black_box;

use super::segment::Cuts;

/// The most UTF-16 code units Iamb puts in one key event.
pub(crate) const MAX_CHUNK_UNITS: usize = 20;

/// The text of one key event: at most [`MAX_CHUNK_UNITS`] UTF-16 code units, never a split
/// surrogate pair.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TextChunk {
    units: [u16; MAX_CHUNK_UNITS],
    len: usize,
}

impl TextChunk {
    const fn empty() -> Self {
        Self {
            units: [0; MAX_CHUNK_UNITS],
            len: 0,
        }
    }

    /// The code units.
    pub(crate) fn units(&self) -> &[u16] {
        &self.units[..self.len]
    }

    /// The chunk as a string (tests only: the injector never turns a chunk back into text).
    #[cfg(test)]
    pub(crate) fn to_text(&self) -> String {
        String::from_utf16(self.units()).expect("a chunk never splits a surrogate pair")
    }

    fn push(&mut self, units: &[u16]) {
        self.units[self.len..self.len + units.len()].copy_from_slice(units);
        self.len += units.len();
    }

    /// Splits off the first `at` units into a new chunk; the rest moves to the front.
    fn take_front(&mut self, at: usize) -> Self {
        let mut front = Self::empty();
        front.push(&self.units[..at]);
        self.units.copy_within(at..self.len, 0);
        let rest = self.len - at;
        zero(&mut self.units[rest..]);
        self.len = rest;
        front
    }

    fn take_all(&mut self) -> Self {
        std::mem::replace(self, Self::empty())
    }
}

impl Drop for TextChunk {
    fn drop(&mut self) {
        zero(&mut self.units);
    }
}

impl fmt::Debug for TextChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextChunk")
            .field("units", &self.len)
            .finish_non_exhaustive()
    }
}

/// Overwrites `units` with zeros in a way the optimiser keeps.
fn zero(units: &mut [u16]) {
    for unit in units.iter_mut() {
        *unit = 0;
    }
    black_box(units);
}

/// Packs characters into [`TextChunk`]s, one character at a time.
pub(crate) struct TextChunks {
    limit: usize,
    cuts: Cuts,
    current: TextChunk,
    /// The last unit index in `current` where a cut is allowed; 0 for none.
    last_cut: usize,
}

impl TextChunks {
    /// A packer for chunks of at most `limit` code units (clamped to 1..=[`MAX_CHUNK_UNITS`];
    /// a character of two units always fits).
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit: limit.clamp(1, MAX_CHUNK_UNITS),
            cuts: Cuts::new(),
            current: TextChunk::empty(),
            last_cut: 0,
        }
    }

    /// Adds `c`, handing every chunk that is full to `emit`.
    pub(crate) fn push(&mut self, c: char, emit: &mut impl FnMut(TextChunk)) {
        let mut buf = [0u16; 2];
        let units = c.encode_utf16(&mut buf);
        if self.cuts.before(c) {
            self.last_cut = self.current.len;
        }
        if self.current.len + units.len() > self.limit {
            if self.last_cut > 0 {
                // Cut at the last cluster boundary; the rest is the start of `c`'s cluster.
                emit(self.current.take_front(self.last_cut));
                self.last_cut = 0;
            }
            if self.current.len > 0 && self.current.len + units.len() > self.limit {
                // One cluster longer than an event: cut between two scalar values.
                emit(self.current.take_all());
            }
        }
        self.current.push(units);
        zero(&mut buf);
    }

    /// Hands the last, partly filled chunk to `emit`.
    pub(crate) fn finish(mut self, emit: &mut impl FnMut(TextChunk)) {
        if self.current.len > 0 {
            emit(self.current.take_all());
        }
    }
}

/// Packs `chars` into chunks of at most `limit` code units.
pub(crate) fn chunk_chars(chars: impl IntoIterator<Item = char>, limit: usize) -> Vec<TextChunk> {
    let mut out = Vec::new();
    let mut emit = |chunk| out.push(chunk);
    let mut chunks = TextChunks::new(limit);
    for c in chars {
        chunks.push(c, &mut emit);
    }
    chunks.finish(&mut emit);
    out
}

/// A part of a text to type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Piece<'a> {
    /// Text without line breaks or tabs.
    Run(&'a str),
    /// A line break: `\n`, `\r\n` or a lone `\r`.
    LineBreak,
    /// A tab.
    Tab,
}

/// Splits `text` into runs, line breaks and tabs, in order. Empty runs are left out.
pub(crate) fn pieces(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let piece = match c {
            '\n' => Piece::LineBreak,
            '\r' => {
                if chars.peek().is_some_and(|&(_, next)| next == '\n') {
                    chars.next();
                }
                Piece::LineBreak
            }
            '\t' => Piece::Tab,
            _ => continue,
        };
        if start < i {
            out.push(Piece::Run(&text[start..i]));
        }
        out.push(piece);
        start = chars.peek().map_or(text.len(), |&(next, _)| next);
    }
    if start < text.len() {
        out.push(Piece::Run(&text[start..]));
    }
    out
}

/// True when `text` contains a line break.
pub(crate) fn has_line_break(text: &str) -> bool {
    text.contains(['\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(chunks: &[TextChunk]) -> Vec<String> {
        chunks.iter().map(TextChunk::to_text).collect()
    }

    fn assert_well_formed(text: &str, chunks: &[TextChunk], limit: usize) {
        assert_eq!(texts(chunks).concat(), text, "nothing lost, nothing added");
        for chunk in chunks {
            assert!(!chunk.units().is_empty());
            assert!(chunk.units().len() <= limit.max(2));
            // `to_text` panics on a split surrogate pair.
            let _ = chunk.to_text();
        }
    }

    #[test]
    fn short_text_is_one_chunk() {
        let chunks = chunk_chars("Kind regards".chars(), MAX_CHUNK_UNITS);
        assert_eq!(texts(&chunks), ["Kind regards"]);
        assert!(chunk_chars("".chars(), 20).is_empty());
    }

    #[test]
    fn long_ascii_fills_every_event() {
        let text = "a".repeat(45);
        let chunks = chunk_chars(text.chars(), MAX_CHUNK_UNITS);
        let lengths: Vec<usize> = chunks.iter().map(|c| c.units().len()).collect();
        assert_eq!(lengths, [20, 20, 5]);
        assert_well_formed(&text, &chunks, 20);
    }

    #[test]
    fn a_cluster_is_never_split_between_events() {
        // 19 letters, then e + combining acute: the cluster moves to the next event whole.
        let text = format!("{}e\u{301}x", "a".repeat(19));
        let chunks = chunk_chars(text.chars(), 20);
        assert_eq!(
            texts(&chunks),
            ["a".repeat(19), "e\u{301}x".to_owned()],
            "the accent stays with its letter"
        );
    }

    #[test]
    fn emoji_sequences_and_flags_are_never_split() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}"; // 11 units
        let waving = "\u{1F44B}\u{1F3FD}"; // 4 units
        let japan = "\u{1F1EF}\u{1F1F5}"; // 4 units
        let text = format!("hello {family}{family} {waving}{japan}{japan}{japan}{japan}{japan}");
        let chunks = chunk_chars(text.chars(), 20);
        assert_well_formed(&text, &chunks, 20);
        let joined = texts(&chunks);
        for chunk in &joined {
            // Every chunk boundary is a cluster boundary: no chunk starts with a joiner, a
            // skin tone, a variation selector, or the second half of a flag.
            let first = chunk.chars().next().expect("chunks are not empty");
            assert!(first != '\u{200D}' && !('\u{1F3FB}'..='\u{1F3FF}').contains(&first));
            assert_eq!(
                chunk
                    .chars()
                    .filter(|c| ('\u{1F1E6}'..='\u{1F1FF}').contains(c))
                    .count()
                    % 2,
                0,
                "flags stay in pairs: {chunk:?}"
            );
            assert!(!chunk.ends_with('\u{200D}'));
        }
        assert!(joined.iter().any(|c| c.contains(family)));
    }

    #[test]
    fn tag_flags_and_skin_tones() {
        let scotland = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}"; // 14 units
        let text = format!("ab{scotland}{scotland}\u{1F44D}\u{1F3FF}");
        let chunks = chunk_chars(text.chars(), 20);
        assert_eq!(
            texts(&chunks),
            [
                format!("ab{scotland}"),
                format!("{scotland}\u{1F44D}\u{1F3FF}")
            ]
        );
    }

    #[test]
    fn cjk_fills_events() {
        let text = "日本語のテキストを入力します。".repeat(3);
        let chunks = chunk_chars(text.chars(), 20);
        assert_well_formed(&text, &chunks, 20);
        assert_eq!(chunks[0].units().len(), 20);
        // Supplementary ideographs are two units each: 10 per event.
        let text = "\u{20000}".repeat(11);
        let chunks = chunk_chars(text.chars(), 20);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].units().len(), 20);
        assert_well_formed(&text, &chunks, 20);
    }

    #[test]
    fn a_cluster_longer_than_an_event_is_cut_between_scalars() {
        // Ten family emoji joined by ZWJ: one "cluster" by the conservative rule, 29 units.
        let woman = "\u{1F469}";
        let text = [woman; 10].join("\u{200D}");
        let chunks = chunk_chars(text.chars(), 20);
        assert_well_formed(&text, &chunks, 20);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn small_limits_never_split_a_surrogate_pair() {
        let text = "a\u{1F600}b\u{1F1EF}\u{1F1F5}é";
        for limit in [0, 1, 2, 3, 5] {
            let chunks = chunk_chars(text.chars(), limit);
            assert_well_formed(text, &chunks, limit.max(1));
        }
        let chunks = chunk_chars(text.chars(), 1);
        assert_eq!(
            texts(&chunks),
            ["a", "\u{1F600}", "b", "\u{1F1EF}", "\u{1F1F5}", "é"]
        );
        // With room for a whole flag, the flag goes in one event.
        let chunks = chunk_chars("\u{1F1EF}\u{1F1F5}\u{1F1EB}\u{1F1F7}".chars(), 4);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn every_prefix_of_a_mixed_text_chunks_well() {
        let text = "Café déjà vu — naïve façade, Ærøskøbing, 日本語, 👋🏽 👩‍💻 ✅ 🇯🇵🇫🇷 e\u{301}";
        let chars: Vec<char> = text.chars().collect();
        for end in 0..=chars.len() {
            let prefix: String = chars[..end].iter().collect();
            for limit in [1, 4, 7, 20] {
                let chunks = chunk_chars(prefix.chars(), limit);
                assert_well_formed(&prefix, &chunks, limit);
            }
        }
    }

    #[test]
    fn debug_shows_no_text() {
        let chunks = chunk_chars("secret".chars(), 20);
        assert_eq!(format!("{:?}", chunks[0]), "TextChunk { units: 6, .. }");
    }

    #[test]
    fn pieces_separate_line_breaks_and_tabs() {
        assert_eq!(
            pieces("a\nb\r\nc\rd\te"),
            [
                Piece::Run("a"),
                Piece::LineBreak,
                Piece::Run("b"),
                Piece::LineBreak,
                Piece::Run("c"),
                Piece::LineBreak,
                Piece::Run("d"),
                Piece::Tab,
                Piece::Run("e"),
            ]
        );
        assert_eq!(
            pieces("\n\nx\n"),
            [
                Piece::LineBreak,
                Piece::LineBreak,
                Piece::Run("x"),
                Piece::LineBreak
            ]
        );
        assert_eq!(pieces("\r\n"), [Piece::LineBreak]);
        assert!(pieces("").is_empty());
        assert_eq!(pieces("plain"), [Piece::Run("plain")]);
        assert!(has_line_break("a\rb") && has_line_break("a\n") && !has_line_break("a\tb"));
    }
}
