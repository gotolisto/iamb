//! Pure scanning logic: tokenisation, normalisation, hashing and window matching.

use std::collections::HashSet;
use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};

/// A SHA-256 digest of a normalised term.
pub(crate) type Digest = [u8; 32];

/// Generic phrases describing our own product category. Windows made of exactly these
/// phrases (or overlapping them) are never reported through the no-separator rule.
pub(crate) const ALLOWED_PHRASES: [&str; 3] = ["text expander", "text expanders", "text expansion"];

/// Bytes inspected when deciding whether a file is binary.
const BINARY_SNIFF_BYTES: usize = 8192;

/// A lowercase alphanumeric token with the position of its first character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) text: String,
    pub(crate) line: usize,
    pub(crate) col: usize,
}

/// A banned-term hit: location of the first token plus the matching digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hit {
    pub(crate) line: usize,
    pub(crate) col: usize,
    pub(crate) digest: Digest,
}

fn flush(cur: &mut String, start: (usize, usize), out: &mut Vec<Token>) {
    if !cur.is_empty() {
        out.push(Token {
            text: std::mem::take(cur),
            line: start.0,
            col: start.1,
        });
    }
}

/// Lowercases (Unicode) and splits on any non-alphanumeric character.
pub(crate) fn tokenize(s: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = (1, 1);
    let (mut line, mut col) = (1_usize, 0_usize);
    for ch in s.chars() {
        col += 1;
        if ch == '\n' {
            flush(&mut cur, start, &mut out);
            line += 1;
            col = 0;
            continue;
        }
        for lc in ch.to_lowercase() {
            if lc.is_alphanumeric() {
                if cur.is_empty() {
                    start = (line, col);
                }
                cur.push(lc);
            } else {
                flush(&mut cur, start, &mut out);
            }
        }
    }
    flush(&mut cur, start, &mut out);
    out
}

/// Normal form of a term: tokens joined by a single space.
pub(crate) fn normalise(s: &str) -> String {
    let tokens: Vec<String> = tokenize(s).into_iter().map(|t| t.text).collect();
    tokens.join(" ")
}

/// SHA-256 digest of the normalised form of `term`.
pub(crate) fn digest_of(term: &str) -> Digest {
    Sha256::digest(normalise(term).as_bytes()).into()
}

/// Lowercase hex encoding of a digest.
pub(crate) fn to_hex(d: &Digest) -> String {
    d.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Parses a 64-character hex digest.
pub(crate) fn parse_hex(s: &str) -> Option<Digest> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0_u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Marks tokens that belong to an allow-listed generic phrase.
fn allowed_mask(tokens: &[Token]) -> Vec<bool> {
    let mut mask = vec![false; tokens.len()];
    for phrase in ALLOWED_PHRASES {
        let words: Vec<String> = tokenize(phrase).into_iter().map(|t| t.text).collect();
        if words.is_empty() {
            continue;
        }
        for (i, window) in tokens.windows(words.len()).enumerate() {
            if window.iter().zip(&words).all(|(t, w)| &t.text == w) {
                mask[i..i + words.len()].fill(true);
            }
        }
    }
    mask
}

fn hash_str(s: &str) -> Digest {
    Sha256::digest(s.as_bytes()).into()
}

/// Scans text for banned terms. Every window of 1 to 3 consecutive tokens is hashed
/// space-joined; 2- and 3-token windows are also hashed with no separator unless they
/// overlap an allow-listed generic phrase or contain a one-letter token.
pub(crate) fn scan_text(text: &str, digests: &HashSet<Digest>) -> Vec<Hit> {
    let tokens = tokenize(text);
    let mask = allowed_mask(&tokens);
    let mut hits = Vec::new();
    let mut buf = String::new();
    for i in 0..tokens.len() {
        let mut found = None;
        for n in 1..=3_usize {
            let Some(window) = tokens.get(i..i + n) else {
                break;
            };
            buf.clear();
            for (k, t) in window.iter().enumerate() {
                if k > 0 {
                    buf.push(' ');
                }
                buf.push_str(&t.text);
            }
            let d = hash_str(&buf);
            if digests.contains(&d) {
                found = Some(d);
                break;
            }
            // One-letter tokens are skipped here: ordinary prose such as an article followed
            // by a noun would otherwise collide with short one-word terms.
            let joinable = window.iter().all(|t| t.text.chars().count() >= 2);
            if n >= 2 && joinable && !mask[i..i + n].iter().any(|&m| m) {
                buf.clear();
                for t in window {
                    buf.push_str(&t.text);
                }
                let d = hash_str(&buf);
                if digests.contains(&d) {
                    found = Some(d);
                    break;
                }
            }
        }
        if let Some(digest) = found {
            hits.push(Hit {
                line: tokens[i].line,
                col: tokens[i].col,
                digest,
            });
        }
    }
    hits
}

/// True if the first 8 KB contain a NUL byte.
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_BYTES).any(|&b| b == 0)
}

/// Scans file contents; binary files yield no hits.
pub(crate) fn scan_bytes(bytes: &[u8], digests: &HashSet<Digest>) -> Vec<Hit> {
    if is_binary(bytes) {
        return Vec::new();
    }
    scan_text(&String::from_utf8_lossy(bytes), digests)
}

/// Formats a hit without ever including the matched text.
pub(crate) fn format_hit(path: &str, hit: &Hit) -> String {
    let hex = to_hex(&hit.digest);
    format!(
        "{path}:{}:{}: banned term (digest {}\u{2026})",
        hit.line,
        hit.col,
        &hex[..8]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLANTED: &str = "zz planted term";

    fn set() -> HashSet<Digest> {
        HashSet::from([digest_of("zzplantedterm")])
    }

    #[test]
    fn normalisation_lowercases_and_splits() {
        assert_eq!(normalise("  Foo--BAR_baz!! "), "foo bar baz");
        assert_eq!(normalise("ÄÖ Straße"), "äö straße");
        assert_eq!(digest_of("Zz Planted-Term"), digest_of("zz planted term"));
    }

    #[test]
    fn token_positions_are_one_based() {
        let t = tokenize("ab cd\n  ef");
        assert_eq!((t[1].line, t[1].col), (1, 4));
        assert_eq!((t[2].line, t[2].col), (2, 3));
    }

    #[test]
    fn planted_term_is_caught() {
        let d = HashSet::from([digest_of(PLANTED)]);
        let hits = scan_text("hello\nuse Zz-Planted_Term here", &d);
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].line, hits[0].col), (2, 5));
        assert_eq!(scan_text("zzplantedterm", &set()).len(), 1);
        assert_eq!(scan_text("ZzPlantedTerm", &set()).len(), 1);
    }

    #[test]
    fn windows_span_line_breaks() {
        let d = HashSet::from([digest_of(PLANTED)]);
        let hits = scan_text("zz\nplanted\n\nterm", &d);
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].line, hits[0].col), (1, 1));
    }

    #[test]
    fn split_forms_are_caught_via_no_separator_rule() {
        let d = set();
        assert_eq!(scan_text("zz planted term", &d).len(), 1);
        assert_eq!(scan_text("zz-planted-term", &d).len(), 1);
        assert_eq!(scan_text("zzplanted\nterm", &d).len(), 1);
        assert!(scan_text("zz planted", &d).is_empty());
    }

    #[test]
    fn generic_phrase_is_exempt() {
        // Built from single characters so this source never spells the listed words.
        let one_word =
            String::from_iter(['t', 'e', 'x', 't', 'e', 'x', 'p', 'a', 'n', 'd', 'e', 'r']);
        let short = String::from_iter(['a', 't', 'e', 'x', 't']);
        let d = HashSet::from([digest_of(&one_word), digest_of(&short)]);
        assert!(scan_text("a text expander for desktop", &d).is_empty());
        assert!(scan_text("Text\nExpander, text-expanders", &d).is_empty());
        assert_eq!(scan_text(&one_word.to_uppercase(), &d).len(), 1);
        // An article followed by a noun is ordinary prose, not the short one-word term.
        assert!(scan_text(&format!("{} {}", &short[..1], &short[1..]), &d).is_empty());
        assert_eq!(scan_text(&short, &d).len(), 1);
    }

    #[test]
    fn binary_files_are_skipped() {
        let mut bytes = b"zzplantedterm".to_vec();
        assert_eq!(scan_bytes(&bytes, &set()).len(), 1);
        bytes.insert(0, 0);
        assert!(scan_bytes(&bytes, &set()).is_empty());
    }

    #[test]
    fn hit_output_has_no_plaintext() {
        let d = set();
        let hit = scan_text("zzplantedterm", &d).remove(0);
        let line = format_hit("src/a.rs", &hit);
        assert!(line.starts_with("src/a.rs:1:1: banned term (digest "));
        assert!(!line.contains("planted"));
        assert!(line.contains(&to_hex(&hit.digest)[..8]));
    }

    #[test]
    fn hex_round_trip() {
        let d = digest_of("abc");
        assert_eq!(parse_hex(&to_hex(&d)), Some(d));
        assert_eq!(parse_hex("zz"), None);
    }
}
