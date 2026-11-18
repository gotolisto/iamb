//! Canaries and the scan for them (`docs/test-strategy.md` §5.3).
//!
//! A canary is `qz` + 14 random lower-case letters from the OS generator, never an
//! abbreviation of the fixture library. After the run every file under each scanned location is
//! searched for every canary in these forms:
//!
//! | Form | Bytes searched |
//! | --- | --- |
//! | `utf8` | The canary's UTF-8 bytes |
//! | `utf16le` | Its UTF-16LE bytes |
//! | `base64` | Its UTF-8 bytes in standard Base64, at each of the three alignments a surrounding stream can give (only the characters that do not depend on neighbouring bytes) |
//! | `base64-utf16le` | The same for its UTF-16LE bytes |
//! | `base64url`, `base64url-utf16le` | The two above in the URL-safe alphabet |
//!
//! A hit is reported by canary number, the context it was typed in, the form, the location and
//! the byte offset; never by the canary itself or the bytes around it.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    let mut out = Vec::new();
    for (i, c) in canaries.iter().enumerate() {
        let utf8 = c.text.as_bytes().to_vec();
        let utf16 = utf16le(&c.text);
        out.push(Pattern {
            canary: i,
            form: "utf8",
            bytes: utf8.clone(),
        });
        out.push(Pattern {
            canary: i,
            form: "utf16le",
            bytes: utf16.clone(),
        });
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

/// Scans one file, streaming it in chunks that overlap by the longest pattern.
fn scan_file(
    file: &Path,
    matcher: &Matcher<'_>,
    found: &mut dyn FnMut(usize, u64),
) -> io::Result<u64> {
    let mut f = File::open(file)?;
    let keep = matcher.longest.saturating_sub(1);
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK + keep);
    let mut base = 0_u64; // file offset of buf[0]
    let mut total = 0_u64;
    let mut chunk = vec![0_u8; CHUNK];
    loop {
        let n = f.read(&mut chunk)?;
        let eof = n == 0;
        buf.extend_from_slice(&chunk[..n]);
        total += u64::try_from(n).unwrap_or(0);
        // Positions below `limit` have every byte a pattern could need, or the file ended.
        let limit = if eof {
            buf.len()
        } else {
            buf.len().saturating_sub(keep)
        };
        matcher.find(&buf, limit, |i, pos| {
            found(i, base + u64::try_from(pos).unwrap_or(0));
        });
        if eof {
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
    let matcher = Matcher::new(&patterns);
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
            let mut seen: Vec<(usize, &'static str)> = Vec::new();
            let mut hits: Vec<Hit> = Vec::new();
            let result = scan_file(&file, &matcher, &mut |i, offset| {
                let p = &patterns[i];
                if !seen.contains(&(p.canary, p.form)) {
                    seen.push((p.canary, p.form));
                    hits.push(Hit {
                        canary: canaries[p.canary].number,
                        context: canaries[p.canary].context,
                        form: p.form.to_owned(),
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
