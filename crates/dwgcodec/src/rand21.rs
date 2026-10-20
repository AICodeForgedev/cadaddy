//! The CRC random encoding of R2007 files (spec §5.11): a table of 0x270 pseudo-random words from
//! a seed, giving random 64-bit values, 10-bit values hidden in random bits, and padding bytes.

const TABLE: usize = 0x270;

/// The encoder's state.
pub struct Random {
    table: Vec<u32>,
    index: usize,
    padding: Vec<u8>,
    pad_at: usize,
}

impl Random {
    pub fn new(seed: u64) -> Random {
        let mut table = vec![0u32; TABLE];
        table[0] = (seed as u32).wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        table[1] = ((seed >> 32) as u32).wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        let mut v = table[1];
        for (i, slot) in table.iter_mut().enumerate().skip(2) {
            v = ((v >> 0x1e) ^ v).wrapping_mul(0x6c078965).wrapping_add(i as u32);
            *slot = v;
        }
        let mut r = Random { table, index: 0, padding: Vec::new(), pad_at: 0 };
        let mut padding = Vec::with_capacity(0x200);
        for _ in 0..0x80 {
            r.wrap();
            padding.extend_from_slice(&r.table.get(r.index).copied().unwrap_or(0).to_le_bytes());
            r.index += 1;
        }
        r.padding = padding;
        r
    }

    fn wrap(&mut self) {
        if self.index >= TABLE {
            self.index = 0;
        }
    }

    /// The next random 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        self.index += 2;
        self.wrap();
        let lo = u64::from(self.table.get(self.index).copied().unwrap_or(0));
        let hi = u64::from(self.table.get((self.index + 1) % TABLE).copied().unwrap_or(0));
        lo | (hi << 32)
    }

    /// `value`'s 10 low bits spread over a random 64-bit value.
    pub fn encode(&mut self, value: u32) -> u64 {
        let r = self.next_u64();
        let mut lo = (r as u32) & 0xdf7d_f7df;
        let mut hi = ((r >> 32) as u32) & 0xf7df_7df7;
        for (bit, m) in [(0x200, 0x20), (0x100, 0x800), (0x80, 0x20000), (0x40, 0x80_0000), (0x20, 0x2000_0000)] {
            if value & bit != 0 {
                lo |= m;
            }
        }
        for (bit, m) in [(0x10, 0x8), (0x8, 0x200), (0x4, 0x8000), (0x2, 0x20_0000), (0x1, 0x800_0000)] {
            if value & bit != 0 {
                hi |= m;
            }
        }
        u64::from(lo) | (u64::from(hi) << 32)
    }

    /// The next padding byte (the padding table's bytes in turn).
    pub fn pad(&mut self) -> u8 {
        let b = self.padding.get(self.pad_at % self.padding.len().max(1)).copied().unwrap_or(0);
        self.pad_at += 1;
        b
    }
}

/// The 10-bit value hidden in `value` (spec §5.11 Decode).
pub fn decode(value: u64) -> u32 {
    let hi = (value >> 32) as u32;
    let lo = value as u32;
    let mut r = 0;
    for (m, bit) in [(0x800_0000, 0x1), (0x20_0000, 0x2), (0x8000, 0x4), (0x200, 0x8), (0x8, 0x10)] {
        if hi & m != 0 {
            r |= bit;
        }
    }
    for (m, bit) in [(0x2000_0000, 0x20), (0x80_0000, 0x40), (0x20000, 0x80), (0x800, 0x100), (0x20, 0x200)] {
        if lo & m != 0 {
            r |= bit;
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_values_decode() {
        let mut r = Random::new(0x1234_5678_9abc);
        for v in [0, 1, 0x155, 0x2aa, 0x3ff] {
            assert_eq!(decode(r.encode(v)), v);
        }
    }
}
