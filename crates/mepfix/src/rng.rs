//! A small seeded generator (SplitMix64): the same seed gives the same fixtures on every machine
//! and every build, with no dependency whose output could change under us.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x6D45_5046_4958_2031)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0..n (0 when n is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }

    /// lo..=hi.
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi.saturating_sub(lo) + 1)
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64) < p
    }

    /// One of `xs` (the default value when `xs` is empty).
    pub fn pick<T: Clone + Default>(&mut self, xs: &[T]) -> T {
        let i = self.below(xs.len());
        xs.get(i).cloned().unwrap_or_default()
    }

    /// An independent generator for one part of the fixture set, so adding draws to one part
    /// doesn't change the others.
    pub fn fork(&mut self, salt: u64) -> Rng {
        Rng(self.next_u64() ^ salt.wrapping_mul(0xD1B5_4A32_D192_ED03))
    }
}
