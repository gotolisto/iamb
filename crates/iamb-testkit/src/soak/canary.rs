//! Canaries and the scan for them (`docs/test-strategy.md` §5.3).
//!
//! A canary is `qz` + 14 random lower-case letters from the OS generator, never an
//! abbreviation of the fixture library. After the run every file under each scanned location is
//! searched for every canary in these whole-string forms:
//!
//! | Form | Bytes searched |
//! | --- | --- |
//! | `utf8` | The canary's UTF-8 bytes |
//! | `utf16le`, `utf16be` | Its UTF-16 bytes in either byte order |
//! | `utf32le`, `utf32be` | Its UTF-32 bytes in either byte order (how the service holds a `char`) |
//! | `upper` | Its UTF-8 bytes in upper case |
//! | `reversed` | Its UTF-8 bytes in reverse order |
//! | `hex`, `hex-upper` | Two hex digits per byte |
//! | `json-escape`, `json-escape-upper` | `\u0071\u007a…` |
//! | `percent`, `percent-upper` | `%71%7a…` |
//! | `decimal-bytes` | `113, 122, …` (the debug form of a byte slice) |
//! | `base64` | Its UTF-8 bytes in standard Base64, at each of the three alignments a surrounding stream can give (only the characters that do not depend on neighbouring bytes) |
//! | `base64-utf16le` | The same for its UTF-16LE bytes |
//! | `base64url`, `base64url-utf16le` | The two above in the URL-safe alphabet |
//!
//! and in the per-character forms of [`spread`](super::spread): `stride-<s>` (its bytes at a
//! constant distance: UTF-16, UTF-32, arrays of `char`, fixed-size records) and `per-line` (one
//! character per line of one template, as a log that records every key would write it).
//!
//! A hit is reported by canary number, the context it was typed in, the form, the location and
//! the byte offset (for `per-line`, of the line with the first character); never by the canary
//! itself or the bytes around it.
//!
//! [`self_test`] plants every form of every canary in files of its own and requires a hit in
//! each, so that a scanner that has stopped finding a form cannot pass (the soak's rule
//! `canary-scanner-control`).

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::spread::Spread;

/// The prefix of every canary.
pub const PREFIX: &str = "qz";

/// Random letters after the prefix.
pub const RANDOM_LETTERS: usize = 14;

/// Where a canary was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Context {
    /// Ordinary text in an ordinary field, the engine listening.
    Ordinary,
    /// Ordinary text directly followed by an abbreviation that needs no word start, so the
    /// canary is in the engine's buffer when the expansion fires.
    BeforeExpansion,
    /// A password field (the mock reports a secure field).
    SecureField,
    /// A window of an app on the exclusion list.
    ExcludedApp,
    /// An ordinary field while global pause is on.
    Paused,
}

impl Context {
    /// Every context, in the order a round types them.
    pub const ALL: [Self; 5] = [
        Self::Ordinary,
        Self::BeforeExpansion,
        Self::SecureField,
        Self::ExcludedApp,
        Self::Paused,
    ];

    /// The name in reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::BeforeExpansion => "before-expansion",
            Self::SecureField => "secure-field",
            Self::ExcludedApp => "excluded-app",
            Self::Paused => "paused",
        }
    }
}

/// One canary of a run. `Debug` does not print the text.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canary {
    /// Its number in the run (what reports name).
    pub number: usize,
    /// Where it is typed.
    pub context: Context,
    /// The round it belongs to.
    pub round: usize,
    /// The text. Only the driver's input file holds it.
    pub text: String,
}

impl std::fmt::Debug for Canary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canary")
            .field("number", &self.number)
            .field("context", &self.context)
            .field("round", &self.round)
            .finish_non_exhaustive()
    }
}

/// A fresh canary text from the OS generator, for which `reject` is false.
pub fn fresh_text(reject: impl Fn(&str) -> bool) -> Result<String, String> {
    for _ in 0..1000 {
        let mut text = String::from(PREFIX);
        while text.len() < PREFIX.len() + RANDOM_LETTERS {
            let mut bytes = [0_u8; 32];
            getrandom::fill(&mut bytes).map_err(|e| format!("no OS random numbers: {e}"))?;
            for b in bytes {
                // 234 = 9 × 26: no modulo bias.
                if b < 234 && text.len() < PREFIX.len() + RANDOM_LETTERS {
                    text.push(char::from(b'a' + b % 26));
                }
            }
        }
        if !reject(&text) {
            return Ok(text);
        }
    }
    Err("could not draw a canary that is not an abbreviation".to_owned())
}

/// `rounds` rounds of one canary per [`Context`], all different.
pub fn generate(rounds: usize, reject: impl Fn(&str) -> bool) -> Result<Vec<Canary>, String> {
    let mut out: Vec<Canary> = Vec::new();
    for round in 0..rounds {
        for context in Context::ALL {
            let text = fresh_text(|t| reject(t) || out.iter().any(|c| c.text == t))?;
            out.push(Canary {
                number: out.len() + 1,
                context,
                round,
                text,
            });
        }
    }
    Ok(out)
}

const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64 of `bytes` (no padding: only whole groups are used).
fn base64(bytes: &[u8], alphabet: &[u8; 64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let take = chunk.len() + 1;
        for i in 0..take {
            let index = usize::try_from((n >> (18 - 6 * i)) & 63).unwrap_or(0);
            out.push(alphabet[index]);
        }
    }
    out
}

/// The Base64 characters of `bytes` that are the same wherever `bytes` sit in a longer
/// stream that is `offset` bytes past a group boundary (0, 1 or 2).
fn base64_core(bytes: &[u8], offset: usize, alphabet: &[u8; 64]) -> Vec<u8> {
    let mut padded = vec![0_u8; offset];
    padded.extend_from_slice(bytes);
    // Only whole groups: a partial last group depends on the bytes after the canary.
    let whole = padded.len() / 3 * 3;
    let encoded = base64(&padded[..whole], alphabet);
    // The first characters cover the unknown bytes before the canary: 0, 2 or 3 of them.
    let skip = match offset {
        0 => 0,
        1 => 2,
        _ => 3,
    };
    encoded[skip.min(encoded.len())..].to_vec()
}

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn utf16be(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn utf32le(text: &str) -> Vec<u8> {
    text.chars()
        .flat_map(|c| u32::from(c).to_le_bytes())
        .collect()
}

fn utf32be(text: &str) -> Vec<u8> {
    text.chars()
        .flat_map(|c| u32::from(c).to_be_bytes())
        .collect()
}

/// Each UTF-8 byte of `text` written with `each`.
fn per_byte(text: &str, each: impl Fn(u8) -> String) -> Vec<u8> {
    text.bytes().map(each).collect::<String>().into_bytes()
}

/// The whole-string forms other than Base64 (module docs), in the order of the table.
fn plain_forms(text: &str) -> Vec<(&'static str, Vec<u8>)> {
    let decimal = text
        .bytes()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        ("utf8", text.as_bytes().to_vec()),
        ("utf16le", utf16le(text)),
        ("utf16be", utf16be(text)),
        ("utf32le", utf32le(text)),
        ("utf32be", utf32be(text)),
        ("upper", text.to_uppercase().into_bytes()),
        ("reversed", text.bytes().rev().collect()),
        ("hex", per_byte(text, |b| format!("{b:02x}"))),
        ("hex-upper", per_byte(text, |b| format!("{b:02X}"))),
        ("json-escape", per_byte(text, |b| format!("\\u{b:04x}"))),
        (
            "json-escape-upper",
            per_byte(text, |b| format!("\\u{b:04X}")),
        ),
        ("percent", per_byte(text, |b| format!("%{b:02x}"))),
        ("percent-upper", per_byte(text, |b| format!("%{b:02X}"))),
        ("decimal-bytes", decimal.into_bytes()),
    ]
}

/// One searched byte pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// Index into the canary list.
    pub canary: usize,
    /// The form's name (module docs).
    pub form: &'static str,
    /// The bytes.
    pub bytes: Vec<u8>,
}

/// Every pattern of every canary.
pub fn patterns(canaries: &[Canary]) -> Vec<Pattern> {
    let mut out: Vec<Pattern> = Vec::new();
    for (i, c) in canaries.iter().enumerate() {
        let utf8 = c.text.as_bytes().to_vec();
        let utf16 = utf16le(&c.text);
        for (form, bytes) in plain_forms(&c.text) {
            // A form that gives the same bytes as an earlier one (`hex` and `hex-upper` when no
            // byte has a letter digit) is searched once.
            if !out.iter().any(|p| p.canary == i && p.bytes == bytes) {
                out.push(Pattern {
                    canary: i,
                    form,
                    bytes,
                });
            }
        }
        for (form, source, alphabet) in [
            ("base64", &utf8, STANDARD),
            ("base64-utf16le", &utf16, STANDARD),
            ("base64url", &utf8, URL_SAFE),
            ("base64url-utf16le", &utf16, URL_SAFE),
        ] {
            for offset in 0..3 {
                let bytes = base64_core(source, offset, alphabet);
                // Letters-only canaries encode the same in both alphabets at some alignments.
                if !out
                    .iter()
                    .any(|p: &Pattern| p.canary == i && p.bytes == bytes)
                {
                    out.push(Pattern {
                        canary: i,
                        form,
                        bytes,
                    });
                }
            }
        }
    }
    out
}

/// One hit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    /// The canary's number.
    pub canary: usize,
    /// Where it was typed.
    pub context: Context,
    /// The form found.
    pub form: String,
    /// The location's label (`data-root`, `temp-dir`, `ipc-tap`, …).
    pub location: String,
    /// The file, relative to the location.
    pub file: String,
    /// The byte offset of the first occurrence in the file.
    pub offset: u64,
}

/// A location to scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// Its label.
    pub label: String,
    /// A file or a directory (scanned recursively).
    pub path: PathBuf,
}

/// What a scan found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReport {
    /// Canaries searched for.
    pub canaries: usize,
    /// Byte patterns searched for.
    pub patterns: usize,
    /// Per location: label, files and bytes scanned.
    pub locations: Vec<LocationSummary>,
    /// Every hit, at most one per canary, form and file.
    pub hits: Vec<Hit>,
    /// Files that could not be read (path relative to the location, error kind).
    pub unreadable: Vec<String>,
}

/// What was scanned in one location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationSummary {
    /// The label.
    pub label: String,
    /// The path.
    pub path: String,
    /// Whether it existed.
    pub exists: bool,
    /// Regular files read.
    pub files: u64,
    /// Bytes read.
    pub bytes: u64,
}

/// A multi-pattern matcher: patterns indexed by their first two bytes.
struct Matcher<'a> {
    patterns: &'a [Pattern],
    by_prefix: HashMap<[u8; 2], Vec<usize>>,
    longest: usize,
}

impl<'a> Matcher<'a> {
    fn new(patterns: &'a [Pattern]) -> Self {
        let mut by_prefix: HashMap<[u8; 2], Vec<usize>> = HashMap::new();
        let mut longest = 0;
        for (i, p) in patterns.iter().enumerate() {
            if p.bytes.len() >= 2 {
                by_prefix
                    .entry([p.bytes[0], p.bytes[1]])
                    .or_default()
                    .push(i);
                longest = longest.max(p.bytes.len());
            }
        }
        Self {
            patterns,
            by_prefix,
            longest,
        }
    }

    /// Calls `found(pattern, position)` for every match starting at a position below `limit`.
    fn find(&self, hay: &[u8], limit: usize, mut found: impl FnMut(usize, usize)) {
        if hay.len() < 2 {
            return;
        }
        for pos in 0..limit.min(hay.len() - 1) {
            if let Some(list) = self.by_prefix.get(&[hay[pos], hay[pos + 1]]) {
                for &i in list {
                    if hay[pos..].starts_with(&self.patterns[i].bytes) {
                        found(i, pos);
                    }
                }
            }
        }
    }
}

/// Files under `path` (itself when it is a file), sorted; symbolic links are not followed.
fn files_under(path: &Path, out: &mut Vec<PathBuf>) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.is_file() {
        out.push(path.to_path_buf());
    } else if meta.is_dir()
        && let Ok(entries) = std::fs::read_dir(path)
    {
        let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        children.sort();
        for child in children {
            files_under(&child, out);
        }
    }
}

const CHUNK: usize = 4 << 20;

/// The searches of one scan: the whole-string patterns and the per-character detectors.
struct Searches<'a> {
    patterns: &'a [Pattern],
    matcher: Matcher<'a>,
    spread: Spread,
}

impl<'a> Searches<'a> {
    fn new(canaries: &[Canary], patterns: &'a [Pattern]) -> Self {
        let texts: Vec<&[u8]> = canaries.iter().map(|c| c.text.as_bytes()).collect();
        Self {
            patterns,
            matcher: Matcher::new(patterns),
            spread: Spread::new(&texts),
        }
    }

    /// Bytes a chunk must keep for the next: the longest pattern or strided span, less one.
    fn keep(&self) -> usize {
        self.matcher
            .longest
            .max(self.spread.span())
            .saturating_sub(1)
    }
}

/// Scans one file, streaming it in chunks that overlap by the longest search;
/// calls `found(canary index, form, offset)`.
fn scan_file(
    file: &Path,
    searches: &Searches<'_>,
    found: &mut dyn FnMut(usize, &str, u64),
) -> io::Result<u64> {
    let mut f = File::open(file)?;
    let keep = searches.keep();
    let mut lines = searches.spread.lines();
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK + keep);
    let mut base = 0_u64; // file offset of buf[0]
    let mut total = 0_u64;
    let mut chunk = vec![0_u8; CHUNK];
    let offset = |base: u64, pos: usize| base + u64::try_from(pos).unwrap_or(0);
    loop {
        let n = f.read(&mut chunk)?;
        let eof = n == 0;
        buf.extend_from_slice(&chunk[..n]);
        total += u64::try_from(n).unwrap_or(0);
        lines.feed(&chunk[..n], &mut |i, at| found(i, "per-line", at));
        // Positions below `limit` have every byte a search could need, or the file ended.
        let limit = if eof {
            buf.len()
        } else {
            buf.len().saturating_sub(keep)
        };
        searches.matcher.find(&buf, limit, |i, pos| {
            found(
                searches.patterns[i].canary,
                searches.patterns[i].form,
                offset(base, pos),
            );
        });
        searches.spread.strided(&buf, limit, &mut |i, pos, stride| {
            found(i, &format!("stride-{stride}"), offset(base, pos));
        });
        if eof {
            lines.finish(&mut |i, at| found(i, "per-line", at));
            return Ok(total);
        }
        let drop = limit;
        buf.drain(..drop);
        base += u64::try_from(drop).unwrap_or(0);
    }
}

/// Scans `locations` for `canaries`, skipping the files in `skip` (the driver's own input).
pub fn scan(canaries: &[Canary], locations: &[Location], skip: &[PathBuf]) -> ScanReport {
    let patterns = patterns(canaries);
    let searches = Searches::new(canaries, &patterns);
    let mut report = ScanReport {
        canaries: canaries.len(),
        patterns: patterns.len(),
        ..ScanReport::default()
    };
    let skip: Vec<PathBuf> = skip
        .iter()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
        .collect();
    for location in locations {
        let mut files = Vec::new();
        files_under(&location.path, &mut files);
        let mut summary = LocationSummary {
            label: location.label.clone(),
            path: location.path.display().to_string(),
            exists: location.path.exists(),
            files: 0,
            bytes: 0,
        };
        for file in files {
            let canonical = file.canonicalize().unwrap_or_else(|_| file.clone());
            if skip.contains(&canonical) {
                continue;
            }
            let relative = file
                .strip_prefix(&location.path)
                .ok()
                .filter(|r| !r.as_os_str().is_empty())
                .map_or_else(
                    || {
                        file.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    },
                    |r| r.display().to_string(),
                );
            let mut hits: Vec<Hit> = Vec::new();
            let result = scan_file(&file, &searches, &mut |i, form, offset| {
                let number = canaries[i].number;
                if !hits.iter().any(|h| h.canary == number && h.form == form) {
                    hits.push(Hit {
                        canary: number,
                        context: canaries[i].context,
                        form: form.to_owned(),
                        location: location.label.clone(),
                        file: relative.clone(),
                        offset,
                    });
                }
            });
            match result {
                Ok(bytes) => {
                    summary.files += 1;
                    summary.bytes += bytes;
                    report.hits.extend(hits);
                }
                Err(e) => report.unreadable.push(format!(
                    "{}: {relative}: {:?}",
                    location.label,
                    e.kind()
                )),
            }
        }
        report.locations.push(summary);
    }
    report
}

/// What [`self_test`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfTest {
    /// Shapes planted per canary.
    pub shapes: usize,
    /// Files planted.
    pub planted: usize,
    /// Planted files without a hit for their canary: `<canary number> <shape>`.
    pub missed: Vec<String>,
}

impl SelfTest {
    /// Whether every planted shape was found.
    pub fn passed(&self) -> bool {
        self.planted > 0 && self.missed.is_empty()
    }
}

/// A log line in the service's format (`log.rs`), with `message`.
fn log_line(n: usize, message: &str) -> String {
    format!(
        "2026-11-18T22:35:{:02}.{:03}Z INFO worker[4103]: {message}\n",
        n / 1000 % 60,
        n % 1000
    )
}

/// The shapes [`self_test`] plants for `text`: every whole-string form and Base64 alignment
/// inside other text, and the per-character forms.
fn planted_shapes(text: &str) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let surround = |bytes: &[u8]| {
        let mut v = b"before ".to_vec();
        v.extend_from_slice(bytes);
        v.extend_from_slice(b" after");
        v
    };
    for (form, bytes) in plain_forms(text) {
        out.push((form.to_owned(), surround(&bytes)));
    }
    let utf16 = utf16le(text);
    for (form, source, alphabet) in [
        ("base64", text.as_bytes(), STANDARD),
        ("base64-utf16le", utf16.as_slice(), STANDARD),
        ("base64url", text.as_bytes(), URL_SAFE),
        ("base64url-utf16le", utf16.as_slice(), URL_SAFE),
    ] {
        for lead in 0..3 {
            let mut raw = vec![b'#'; lead];
            raw.extend_from_slice(source);
            out.push((format!("{form}-at-{lead}"), base64(&raw, alphabet)));
        }
    }
    // One character per fixed-size record (finding 3's queue slot) and in its debug form.
    let mut records = vec![0_u8; 96 * (text.len() + 2)];
    for (k, b) in text.bytes().enumerate() {
        records[96 * (k + 1) + 44] = b;
    }
    out.push(("records-96".to_owned(), records));
    let chars: Vec<char> = text.chars().collect();
    out.push(("char-debug".to_owned(), format!("{chars:?}").into_bytes()));
    // One key per log line, with other lines in between, as `log.rs` writes them.
    let mut log = log_line(0, "worker ready");
    for (k, c) in text.chars().enumerate() {
        log.push_str(&log_line(k * 37, &format!("key {c:?} seq {}", k + 8)));
        if k % 5 == 2 {
            log.push_str(&log_line(k * 37 + 1, "ipc in get_status 55 bytes"));
        }
    }
    out.push(("one-key-per-log-line".to_owned(), log.into_bytes()));
    out
}

/// Plants every shape of every canary in a file of its own under `dir` (created, and removed
/// again afterwards), scans `dir`, and lists the files without a hit for their canary.
pub fn self_test(canaries: &[Canary], dir: &Path) -> Result<SelfTest, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut expected: Vec<(String, usize)> = Vec::new();
    let mut shapes = 0;
    for c in canaries {
        let planted = planted_shapes(&c.text);
        shapes = planted.len();
        for (shape, bytes) in planted {
            let name = format!("{}-{shape}.bin", c.number);
            std::fs::write(dir.join(&name), bytes)
                .map_err(|e| format!("cannot write {}: {e}", dir.display()))?;
            expected.push((name, c.number));
        }
    }
    let report = scan(
        canaries,
        &[Location {
            label: "scanner-control".to_owned(),
            path: dir.to_path_buf(),
        }],
        &[],
    );
    let _ = std::fs::remove_dir_all(dir);
    let missed = expected
        .iter()
        .filter(|(name, number)| {
            !report
                .hits
                .iter()
                .any(|h| &h.file == name && h.canary == *number)
        })
        .map(|(name, _)| name.trim_end_matches(".bin").replacen('-', " ", 1))
        .collect();
    Ok(SelfTest {
        shapes,
        planted: expected.len(),
        missed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canary(text: &str) -> Canary {
        Canary {
            number: 1,
            context: Context::Ordinary,
            round: 0,
            text: text.to_owned(),
        }
    }

    #[test]
    fn canaries_have_the_shape_of_the_strategy() {
        let all = generate(2, |t| t.ends_with('a')).expect("canaries");
        assert_eq!(all.len(), 10);
        for (i, c) in all.iter().enumerate() {
            assert_eq!(c.number, i + 1);
            assert_eq!(c.text.len(), 16);
            assert!(c.text.starts_with("qz"));
            assert!(c.text.bytes().all(|b| b.is_ascii_lowercase()));
            assert!(!c.text.ends_with('a'), "the reject rule holds");
            assert!(!format!("{c:?}").contains(&c.text), "Debug hides the text");
        }
        let mut texts: Vec<&str> = all.iter().map(|c| c.text.as_str()).collect();
        texts.sort_unstable();
        texts.dedup();
        assert_eq!(texts.len(), 10);
    }

    #[test]
    fn base64_matches_a_reference() {
        assert_eq!(base64(b"Man", STANDARD), b"TWFu");
        assert_eq!(base64(b"Ma", STANDARD), b"TWE");
        assert_eq!(base64(b"\xfb\xff", STANDARD), b"+/8");
        assert_eq!(base64(b"\xfb\xff", URL_SAFE), b"-_8");
    }

    /// Plants `text` in every form inside larger streams and checks the scanner finds each one.
    #[test]
    fn every_form_is_found_at_every_alignment() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let c = canary("qzabcdefghijklmn");
        let mut n = 0;
        for lead in 0..3_usize {
            let mut raw = vec![b'#'; lead];
            raw.extend_from_slice(b"before ");
            raw.extend_from_slice(c.text.as_bytes());
            raw.extend_from_slice(b" after");
            let mut raw16 = vec![b'#'; lead];
            raw16.extend_from_slice(&utf16le(&format!("before {} after", c.text)));
            for (name, bytes) in [
                ("utf8", raw.clone()),
                ("utf16le", raw16.clone()),
                ("base64", base64(&raw, STANDARD)),
                ("base64-utf16le", base64(&raw16, STANDARD)),
                ("base64url", base64(&raw, URL_SAFE)),
                ("base64url-utf16le", base64(&raw16, URL_SAFE)),
            ] {
                n += 1;
                std::fs::write(dir.path().join(format!("{n}-{name}.bin")), bytes).expect("write");
            }
        }
        let report = scan(
            std::slice::from_ref(&c),
            &[Location {
                label: "test".to_owned(),
                path: dir.path().to_path_buf(),
            }],
            &[],
        );
        assert_eq!(report.locations[0].files, n);
        for file in 1..=n {
            assert!(
                report
                    .hits
                    .iter()
                    .any(|h| h.file.starts_with(&format!("{file}-"))),
                "file {file} has a hit"
            );
        }
        // Each hit names its location and form, never the canary.
        let json = serde_json::to_string(&report).expect("json");
        assert!(!json.contains(&c.text));
    }

    #[test]
    fn streaming_finds_a_canary_across_a_chunk_boundary_and_skips_input() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let c = canary("qzzyxwvutsrqponm");
        let mut big = vec![b'.'; CHUNK - 5];
        big.extend_from_slice(c.text.as_bytes());
        big.extend_from_slice(&[b'.'; 100]);
        std::fs::write(dir.path().join("big.bin"), &big).expect("write");
        std::fs::write(dir.path().join("input.json"), c.text.as_bytes()).expect("write");
        std::fs::write(dir.path().join("clean.txt"), b"nothing here").expect("write");
        let report = scan(
            std::slice::from_ref(&c),
            &[Location {
                label: "x".to_owned(),
                path: dir.path().to_path_buf(),
            }],
            &[dir.path().join("input.json")],
        );
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].file, "big.bin");
        assert_eq!(
            report.hits[0].offset,
            u64::try_from(CHUNK - 5).expect("fits")
        );
        assert_eq!(report.locations[0].files, 2, "the input file is skipped");
    }

    #[test]
    fn every_planted_shape_is_found() {
        let all = generate(1, |_| false).expect("canaries");
        let dir = tempfile::TempDir::new().expect("temp dir");
        let control = dir.path().join("control");
        let result = self_test(&all, &control).expect("self test");
        assert!(result.passed(), "missed: {:?}", result.missed);
        assert_eq!(result.planted, all.len() * result.shapes);
        assert_eq!(
            result.shapes, 29,
            "14 plain forms, 12 Base64 alignments, 3 per character"
        );
        assert!(!control.exists(), "the planted files are removed again");
        let json = serde_json::to_string(&result).expect("json");
        assert!(all.iter().all(|c| !json.contains(&c.text)));
    }

    /// The per-character shapes hold no whole-string form, so only the detectors of
    /// `spread` find them: a scanner without them (the one the 2.8 review measured) misses
    /// every one of these files.
    #[test]
    fn per_character_shapes_are_found_only_by_the_per_character_detectors() {
        let c = canary("qzkqjetdphptplmw");
        let whole = patterns(std::slice::from_ref(&c));
        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut planted = 0;
        for (shape, bytes) in planted_shapes(&c.text) {
            if !matches!(
                shape.as_str(),
                "records-96" | "char-debug" | "one-key-per-log-line"
            ) {
                continue;
            }
            assert!(
                whole
                    .iter()
                    .all(|p| !bytes.windows(p.bytes.len()).any(|w| w == p.bytes)),
                "{shape} holds a whole-string form"
            );
            std::fs::write(dir.path().join(&shape), bytes).expect("write");
            planted += 1;
        }
        assert_eq!(planted, 3);
        let report = scan(
            std::slice::from_ref(&c),
            &[Location {
                label: "x".to_owned(),
                path: dir.path().to_path_buf(),
            }],
            &[],
        );
        let form = |file: &str| -> Vec<&str> {
            report
                .hits
                .iter()
                .filter(|h| h.file == file)
                .map(|h| h.form.as_str())
                .collect()
        };
        assert_eq!(form("records-96"), ["stride-96"]);
        assert_eq!(form("char-debug"), ["stride-5"]);
        assert_eq!(form("one-key-per-log-line"), ["per-line"]);
    }

    #[test]
    fn a_strided_canary_across_a_chunk_boundary_is_found() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let c = canary("qzzyxwvutsrqponm");
        let mut big = vec![b'.'; CHUNK + 4096];
        let start = CHUNK - 2000;
        for (k, b) in c.text.bytes().enumerate() {
            big[start + k * 300] = b;
        }
        std::fs::write(dir.path().join("big.bin"), &big).expect("write");
        let report = scan(
            std::slice::from_ref(&c),
            &[Location {
                label: "x".to_owned(),
                path: dir.path().to_path_buf(),
            }],
            &[],
        );
        assert_eq!(report.hits.len(), 1, "{:?}", report.hits);
        assert_eq!(report.hits[0].form, "stride-300");
        assert_eq!(report.hits[0].offset, u64::try_from(start).expect("fits"));
    }

    #[test]
    fn a_missing_location_is_reported_not_fatal() {
        let report = scan(
            &[canary("qzaaaaaaaaaaaaaa")],
            &[Location {
                label: "gone".to_owned(),
                path: PathBuf::from("/nonexistent/iamb-soak"),
            }],
            &[],
        );
        assert!(!report.locations[0].exists);
        assert!(report.hits.is_empty());
    }
}
