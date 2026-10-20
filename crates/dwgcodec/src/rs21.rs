//! The Reed–Solomon codes of R2007 files (spec §5.13): (255, 251) for data pages over the
//! primitive polynomial x⁸ + x⁴ + x³ + x² + 1, (255, 239) for system pages over
//! x⁸ + x⁶ + x⁵ + x³ + 1; generator roots α¹ onwards.
//!
//! A stored block is its k data bytes followed by the parity; the code runs over the block
//! reversed (observed: the parity of every block in files of each kind checks only that way).
//! Blocks are interleaved (block j's bytes at positions j, j + n, …, for n blocks) or, not
//! interleaved, all data blocks followed by all parity blocks (spec §5.13.1–2).

/// A code: field tables and generator polynomial.
pub struct Code {
    exp: [u8; 512],
    log: [u8; 256],
    /// Generator coefficients, highest degree first (the leading 1 left out).
    gen_: Vec<u8>,
    /// Data bytes per block.
    pub k: usize,
}

/// Codeword size.
pub const N: usize = 255;

impl Code {
    fn new(poly: u16, k: usize) -> Code {
        let mut exp = [0u8; 512];
        let mut log = [0u8; 256];
        let mut x: u16 = 1;
        for (i, e) in exp.iter_mut().take(255).enumerate() {
            *e = x as u8;
            log[usize::from(x)] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= poly;
            }
        }
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }
        let nroots = N - k;
        let mut g: Vec<u8> = vec![1];
        for i in 0..nroots {
            let root = exp[(1 + i) % 255];
            let mut next = vec![0u8; g.len() + 1];
            for (j, &c) in g.iter().enumerate() {
                next[j] ^= c;
                if c != 0 {
                    next[j + 1] ^= exp[usize::from(log[usize::from(c)]) + usize::from(log[usize::from(root)])];
                }
            }
            g = next;
        }
        g.remove(0);
        Code { exp, log, gen_: g, k }
    }

    /// Data pages' code.
    pub fn data() -> Code {
        Code::new(0x11D, 251)
    }

    /// System pages' code.
    pub fn system() -> Code {
        Code::new(0x169, 239)
    }

    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 { 0 } else { self.exp[usize::from(self.log[usize::from(a)]) + usize::from(self.log[usize::from(b)])] }
    }

    /// The parity of `data` (k bytes), in stored order.
    pub fn parity(&self, data: &[u8]) -> Vec<u8> {
        let nroots = N - self.k;
        let mut par = vec![0u8; nroots];
        for &d in data.iter().rev() {
            let fb = d ^ par[0];
            par.rotate_left(1);
            if let Some(last) = par.last_mut() {
                *last = 0;
            }
            if fb != 0 {
                for (p, &g) in par.iter_mut().zip(&self.gen_) {
                    *p ^= self.mul(fb, g);
                }
            }
        }
        par.reverse();
        par
    }

    /// Whether a stored block (k data bytes and the parity) is a codeword.
    pub fn check(&self, block: &[u8]) -> bool {
        block.len() == N && block.get(self.k..).is_some_and(|p| p == self.parity(&block[..self.k]))
    }

    /// `data` encoded as `blocks` interleaved blocks (data padded with `pad` to fill them).
    pub fn encode_interleaved(&self, data: &[u8], blocks: usize, pad: &mut impl FnMut() -> u8) -> Vec<u8> {
        let mut out = vec![0u8; blocks * N];
        for j in 0..blocks {
            let mut block: Vec<u8> = (0..self.k).map(|i| data.get(j * self.k + i).copied().unwrap_or_else(&mut *pad)).collect();
            block.extend(self.parity(&block));
            for (i, b) in block.into_iter().enumerate() {
                if let Some(slot) = out.get_mut(i * blocks + j) {
                    *slot = b;
                }
            }
        }
        out
    }

    /// `data` encoded as `blocks` blocks, not interleaved: the data blocks, then their parity.
    pub fn encode_plain(&self, data: &[u8], blocks: usize, pad: &mut impl FnMut() -> u8) -> Vec<u8> {
        let mut out = Vec::with_capacity(blocks * N);
        let mut parity = Vec::with_capacity(blocks * (N - self.k));
        for j in 0..blocks {
            let block: Vec<u8> = (0..self.k).map(|i| data.get(j * self.k + i).copied().unwrap_or_else(&mut *pad)).collect();
            parity.extend(self.parity(&block));
            out.extend(block);
        }
        out.extend(parity);
        out
    }
}

/// The data of `blocks` interleaved blocks of `k` data bytes.
pub fn deinterleave(enc: &[u8], blocks: usize, k: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(blocks * k);
    for j in 0..blocks {
        for i in 0..k {
            out.push(enc.get(i * blocks + j).copied().unwrap_or(0));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_blocks_are_codewords_and_deinterleave() {
        for code in [Code::data(), Code::system()] {
            let data: Vec<u8> = (0..3 * code.k).map(|i| (i * 31 % 251) as u8).collect();
            let enc = code.encode_interleaved(&data, 3, &mut || 0);
            for j in 0..3 {
                let block: Vec<u8> = (0..N).map(|i| enc[i * 3 + j]).collect();
                assert!(code.check(&block));
            }
            assert_eq!(deinterleave(&enc, 3, code.k), data);
            let plain = code.encode_plain(&data, 3, &mut || 0);
            assert_eq!(&plain[..data.len()], &data[..]);
        }
    }
}
