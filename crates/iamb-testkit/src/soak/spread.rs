//! Per-character forms of a canary: the shapes typed text takes when something writes it one
//! character at a time instead of as one string (finding 2 of the Phase 2 privacy review,
//! `docs/reviews/2.8-phase2-privacy-review.md`).
//!
//! The service handles typed text as `char` values, one key event at a time, so the most likely
//! leak is not a whole canary in a file but its characters spread out: one per log line, one per
//! fixed-size record, or one per four bytes (an array of `char` is UTF-32). Two detectors look
//! for those shapes. Both need every character of a canary, in order, and are built so that
//! ordinary output does not trigger them:
//!
//! | Form | What is searched | Catches |
//! | --- | --- | --- |
//! | `stride-<s>` | The canary's bytes at `p`, `p + s`, `p + 2s`, … for every stride `s` from 2 to [`MAX_STRIDE`] | UTF-16 and UTF-32 in both byte orders (a canary is ASCII), arrays of `char`, the debug form `['q', 'z', …]`, fixed-size records (a 96-byte queue slot), fixed-width log lines |
//! | `per-line` | After removing ASCII digits from every line (timestamps, process ids, sequence numbers and sizes change the width of a line), the lines of one template: the same bytes except at one column. The canary must stand at that column in consecutive lines of that template, with at most [`MAX_GAP_LINES`] lines of other templates between two of them | One key per log line (`… key 'q' seq 46`), one character per JSON or CSV record |
//!
//! Why random text does not trigger them: a strided match fixes all 16 bytes of a canary once
//! its start and stride are chosen, so in text of lower-case letters each start and stride
//! matches with a probability of 26^-15 (about 6 × 10^-22); a megabyte scanned at 511 strides
//! for 15 canaries stays below 10^-11. A line chain needs 16 lines that are identical apart from
//! one column, with the canary's letters at that column in order and every line of that
//! template in between continuing it: repeated log lines have the same letter at every column
//! (a chain stops at the first wrong letter), and lines that differ elsewhere are another
//! template. The unit tests feed both detectors random letters, random bytes, and the real logs
//! and frames of a soak run.
//!
//! Not covered (the module docs of `canary` list the whole-string forms): characters written as
//! numbers (`113` for `q`, which removing digits erases), one character per record of varying
//! length that is not a line, compressed or encrypted output, and characters in an order other
//! than the typed one.
//!
//! This file uses only the standard library: `crates/iamb-service/tests/log_scan.rs` compiles
//! it by path, so the log test and the soak scan apply the same rules.

use std::collections::HashMap;
use std::rc::Rc;

/// The largest stride searched, in bytes.
pub const MAX_STRIDE: usize = 512;

/// Lines longer than this (after removing digits) take no part in a line chain.
pub const MAX_LINE: usize = 1024;

/// At most this many lines of other templates may stand between two lines of a chain.
pub const MAX_GAP_LINES: u64 = 16;

/// Canaries shorter than this are refused: with fewer characters, chance matches become likely.
pub const MIN_LEN: usize = 8;

/// The two detectors for a set of canaries.
#[derive(Debug, Clone)]
pub struct Spread {
    canaries: Vec<Vec<u8>>,
    /// Canaries by their first two bytes.
    by_pair: HashMap<[u8; 2], Vec<usize>>,
    /// Canaries by their first byte.
    by_first: Vec<Vec<usize>>,
    /// Per first byte: which second bytes start a canary (saves a map lookup per stride).
    seconds: Vec<[bool; 256]>,
    longest: usize,
}

impl Spread {
    /// The detectors for `canaries` (byte strings, each at least [`MIN_LEN`] long; shorter
    /// ones are left out).
    pub fn new<T: AsRef<[u8]>>(canaries: &[T]) -> Self {
        let mut by_pair: HashMap<[u8; 2], Vec<usize>> = HashMap::new();
        let mut by_first = vec![Vec::new(); 256];
        let mut seconds = vec![[false; 256]; 256];
        let mut longest = 0;
        let canaries: Vec<Vec<u8>> = canaries.iter().map(|c| c.as_ref().to_vec()).collect();
        for (i, c) in canaries.iter().enumerate() {
            if c.len() < MIN_LEN {
                continue;
            }
            by_pair.entry([c[0], c[1]]).or_default().push(i);
            by_first[usize::from(c[0])].push(i);
            seconds[usize::from(c[0])][usize::from(c[1])] = true;
            longest = longest.max(c.len());
        }
        Self {
            canaries,
            by_pair,
            by_first,
            seconds,
            longest,
        }
    }

    /// How many bytes after a start position a strided match can reach: what a caller that
    /// streams a file in chunks must keep from one chunk to the next.
    pub const fn span(&self) -> usize {
        self.longest.saturating_sub(1) * MAX_STRIDE + 1
    }

    /// Calls `found(canary, position, stride)` for every strided match that starts at a
    /// position below `limit`.
    pub fn strided(&self, hay: &[u8], limit: usize, found: &mut dyn FnMut(usize, usize, usize)) {
        for pos in 0..limit.min(hay.len()) {
            let first = usize::from(hay[pos]);
            if self.by_first[first].is_empty() {
                continue;
            }
            let seconds = &self.seconds[first];
            for stride in 2..=MAX_STRIDE {
                let Some(&second) = hay.get(pos + stride) else {
                    break;
                };
                if !seconds[usize::from(second)] {
                    continue;
                }
                let Some(list) = self.by_pair.get(&[hay[pos], second]) else {
                    continue;
                };
                for &i in list {
                    let c = &self.canaries[i];
                    let end = pos + (c.len() - 1) * stride;
                    if end < hay.len()
                        && c.iter()
                            .enumerate()
                            .skip(2)
                            .all(|(k, &b)| hay[pos + k * stride] == b)
                    {
                        found(i, pos, stride);
                    }
                }
            }
        }
    }

    /// A fresh line-chain detector for one file.
    pub fn lines(&self) -> Lines<'_> {
        Lines {
            spread: self,
            chains: Vec::new(),
            line: 0,
            pending: Vec::new(),
            too_long: false,
            line_start: 0,
            offset: 0,
        }
    }
}

/// A canary partly found at one column of one template.
#[derive(Debug)]
struct Chain {
    canary: usize,
    /// The index of the next character to find.
    next: usize,
    column: usize,
    template: Rc<[u8]>,
    last_line: u64,
    /// The byte offset of the line holding the first character.
    start: u64,
}

/// The line-chain detector, fed a file's bytes in order.
#[derive(Debug)]
pub struct Lines<'a> {
    spread: &'a Spread,
    chains: Vec<Chain>,
    line: u64,
    /// The current line without its digits.
    pending: Vec<u8>,
    too_long: bool,
    line_start: u64,
    offset: u64,
}

impl Lines<'_> {
    /// Feeds the next bytes of the file; calls `found(canary, offset of its first line)`.
    pub fn feed(&mut self, bytes: &[u8], found: &mut dyn FnMut(usize, u64)) {
        for &b in bytes {
            self.offset += 1;
            if b == b'\n' {
                self.end_line(found);
                self.line_start = self.offset;
            } else if !b.is_ascii_digit() && !self.too_long {
                if self.pending.len() < MAX_LINE {
                    self.pending.push(b);
                } else {
                    self.too_long = true;
                    self.pending.clear();
                }
            }
        }
    }

    /// The end of the file: the last line counts even without a line break.
    pub fn finish(&mut self, found: &mut dyn FnMut(usize, u64)) {
        if !self.pending.is_empty() || self.too_long {
            self.end_line(found);
        }
    }

    fn end_line(&mut self, found: &mut dyn FnMut(usize, u64)) {
        let text: Option<Rc<[u8]>> = (!self.too_long).then(|| Rc::from(self.pending.as_slice()));
        self.pending.clear();
        self.too_long = false;
        let line = self.line;
        self.line += 1;
        let canaries = &self.spread.canaries;
        self.chains.retain_mut(|c| {
            if line - c.last_line > MAX_GAP_LINES + 1 {
                return false;
            }
            let Some(t) = &text else {
                return true;
            };
            let col = c.column;
            if t.len() != c.template.len()
                || t[..col] != c.template[..col]
                || t[col + 1..] != c.template[col + 1..]
            {
                // Another template: the chain waits.
                return true;
            }
            let canary = &canaries[c.canary];
            if t[col] != canary[c.next] {
                // The same template with another character: not this canary.
                return false;
            }
            c.next += 1;
            c.last_line = line;
            if c.next == canary.len() {
                found(c.canary, c.start);
                return false;
            }
            true
        });
        let Some(t) = text else {
            return;
        };
        for (column, &b) in t.iter().enumerate() {
            for &canary in &self.spread.by_first[usize::from(b)] {
                self.chains.push(Chain {
                    canary,
                    next: 1,
                    column,
                    template: Rc::clone(&t),
                    last_line: line,
                    start: self.line_start,
                });
            }
        }
    }
}
