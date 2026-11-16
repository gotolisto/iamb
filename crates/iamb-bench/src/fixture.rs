//! Deterministic fixture generation.
//!
//! The same seed always yields the same library, on every OS and toolchain, because the
//! generator is a fixed `SplitMix64` and only integer arithmetic is used.

use std::collections::HashSet;

/// `SplitMix64` pseudo-random generator (Steele, Lea and Flood). Small, fast, and good enough
/// for fixtures. Not for anything security related.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Creates a generator from a seed.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Returns the next 64-bit value.
    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a value in `0..bound`. `bound` must be non-zero.
    ///
    /// Uses the multiply-high method; the bias is negligible for fixture purposes.
    ///
    /// # Panics
    ///
    /// Panics if `bound` is zero.
    pub fn below(&mut self, bound: u64) -> u64 {
        assert!(bound > 0, "bound must be non-zero");
        let wide = u128::from(self.next_u64()) * u128::from(bound);
        // The high half of a 64x64 multiplication always fits in 64 bits.
        u64::try_from(wide >> 64).unwrap_or(u64::MAX)
    }

    /// Returns an index in `0..len`. `len` must be non-zero.
    ///
    /// # Panics
    ///
    /// Panics if `len` is zero.
    pub fn index(&mut self, len: usize) -> usize {
        let bound = u64::try_from(len).unwrap_or(u64::MAX);
        usize::try_from(self.below(bound)).unwrap_or(0)
    }

    /// Returns a value in `lo..=hi`.
    ///
    /// # Panics
    ///
    /// Panics if `lo > hi`.
    pub fn range_inclusive(&mut self, lo: usize, hi: usize) -> usize {
        assert!(lo <= hi, "empty range");
        lo + self.index(hi - lo + 1)
    }
}

/// One generated snippet: an abbreviation and a plain-text body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureSnippet {
    /// Unique abbreviation, 3 to 9 ASCII characters, starting with a prefix character.
    pub abbreviation: String,
    /// Plain-text body, 20 to 500 characters, sometimes with newlines.
    pub body: String,
}

/// Prefix characters commonly used to mark abbreviations.
const PREFIXES: &[char] = &[';', '/', ',', 'x', 'q'];
const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const WORDS: &[&str] = &[
    "thanks", "for", "your", "message", "regards", "meeting", "tomorrow", "please", "find",
    "attached", "the", "invoice", "schedule", "call", "address", "street", "support", "ticket",
    "order", "number", "best", "kind", "hello", "team", "update", "project", "review", "draft",
];

/// Shortest body generated, in characters.
pub const MIN_BODY_CHARS: usize = 20;
/// Longest body generated, in characters (the plain-text KPI limit).
pub const MAX_BODY_CHARS: usize = 500;

/// Generates `count` snippets with unique abbreviations from `seed`.
///
/// The result depends only on `seed` and `count`.
#[must_use]
pub fn generate_library(seed: u64, count: usize) -> Vec<FixtureSnippet> {
    let mut rng = SplitMix64::new(seed);
    let mut taken = HashSet::with_capacity(count);
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let abbreviation = gen_abbreviation(&mut rng);
        if !taken.insert(abbreviation.clone()) {
            continue;
        }
        let body = gen_body(&mut rng);
        out.push(FixtureSnippet { abbreviation, body });
    }
    out
}

/// Picks `count` abbreviations from `library` in a deterministic order, for lookup loops.
/// Roughly one in ten keys is a miss (an abbreviation not in the library).
#[must_use]
pub fn lookup_keys(library: &[FixtureSnippet], seed: u64, count: usize) -> Vec<String> {
    let mut rng = SplitMix64::new(seed ^ 0x5EED_F100_C0F5);
    (0..count)
        .map(|_| {
            if library.is_empty() || rng.below(10) == 0 {
                format!("~miss{}", rng.below(1_000_000))
            } else {
                library[rng.index(library.len())].abbreviation.clone()
            }
        })
        .collect()
}

fn gen_abbreviation(rng: &mut SplitMix64) -> String {
    let len = rng.range_inclusive(2, 8);
    let mut s = String::with_capacity(len + 1);
    s.push(PREFIXES[rng.index(PREFIXES.len())]);
    for _ in 0..len {
        s.push(char::from(ALPHABET[rng.index(ALPHABET.len())]));
    }
    s
}

fn gen_body(rng: &mut SplitMix64) -> String {
    let target = rng.range_inclusive(MIN_BODY_CHARS, MAX_BODY_CHARS);
    let mut s = String::with_capacity(target + 16);
    while s.len() < target {
        if !s.is_empty() {
            s.push(if rng.below(12) == 0 { '\n' } else { ' ' });
        }
        s.push_str(WORDS[rng.index(WORDS.len())]);
    }
    // All characters are ASCII, so byte length equals character count.
    s.truncate(target);
    s
}

#[cfg(test)]
mod tests {
    use super::{MAX_BODY_CHARS, MIN_BODY_CHARS, SplitMix64, generate_library, lookup_keys};
    use std::collections::HashSet;

    #[test]
    fn splitmix_matches_reference_output() {
        // Reference values of SplitMix64 for seed 0.
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn below_stays_in_bounds() {
        let mut rng = SplitMix64::new(7);
        for bound in [1_u64, 2, 3, 10, 1000] {
            for _ in 0..1000 {
                assert!(rng.below(bound) < bound);
            }
        }
    }

    #[test]
    fn same_seed_same_library() {
        assert_eq!(generate_library(42, 500), generate_library(42, 500));
    }

    #[test]
    fn different_seed_different_library() {
        assert_ne!(generate_library(1, 100), generate_library(2, 100));
    }

    #[test]
    fn library_has_unique_abbreviations_and_bounded_bodies() {
        let lib = generate_library(crate::DEFAULT_SEED, crate::KPI_LIBRARY_SIZE);
        assert_eq!(lib.len(), crate::KPI_LIBRARY_SIZE);
        let unique: HashSet<_> = lib.iter().map(|s| s.abbreviation.as_str()).collect();
        assert_eq!(unique.len(), lib.len());
        for s in &lib {
            let n = s.body.chars().count();
            assert!(
                (MIN_BODY_CHARS..=MAX_BODY_CHARS).contains(&n),
                "body length {n}"
            );
            assert!((3..=9).contains(&s.abbreviation.len()));
        }
    }

    #[test]
    fn lookup_keys_are_deterministic_and_mostly_hits() {
        let lib = generate_library(3, 1000);
        let a = lookup_keys(&lib, 3, 2000);
        assert_eq!(a, lookup_keys(&lib, 3, 2000));
        let hits = a.iter().filter(|k| !k.starts_with('~')).count();
        assert!(hits > 1600 && hits < 1950, "hits {hits}");
    }
}
