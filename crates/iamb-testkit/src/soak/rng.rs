//! The seeded generator of the key stream and the fixture library: `SplitMix64` (Steele, Lea
//! and Flood), integer arithmetic only, so a seed gives the same run on every OS.

/// A small, fast, seedable generator. Not for secrets (canaries use the OS generator).
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// A generator started at `seed`.
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 bits.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n` (`0` when `n` is 0). The modulo bias is below 2^-40 for the sizes
    /// used here.
    pub const fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    /// An index into a slice of length `n`.
    pub fn index(&mut self, n: usize) -> usize {
        usize::try_from(self.below(u64::try_from(n).unwrap_or(u64::MAX))).unwrap_or(0)
    }

    /// True with probability `percent` / 100.
    pub const fn percent(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// A uniform number in `[0, 1)` with 53 bits.
    #[allow(
        clippy::cast_precision_loss,
        reason = "53 bits fit an f64 mantissa exactly"
    )]
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1_u64 << 53) as f64
    }

    /// An exponentially distributed number with mean `mean`.
    pub fn exponential(&mut self, mean: f64) -> f64 {
        -(1.0 - self.unit()).ln() * mean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_numbers() {
        let mut a = SplitMix64::new(7);
        let mut b = SplitMix64::new(7);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        // The first value of seed 0 is the published reference value.
        assert_eq!(SplitMix64::new(0).next_u64(), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn ranges_hold() {
        let mut r = SplitMix64::new(1);
        for _ in 0..1000 {
            assert!(r.below(10) < 10);
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
            assert!(r.exponential(0.5) >= 0.0);
        }
        assert_eq!(r.below(0), 0);
    }
}
