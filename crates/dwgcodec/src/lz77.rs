//! The R2004+ LZ77 variant (spec §4.7): a literal run, then opcodes that copy earlier output
//! ("compressed bytes" from "compOffset" back) followed by more literals, ending with 0x11.
//!
//! One detail the spec leaves out: the distance back is compOffset + 1 (a compOffset of 0 copies
//! the byte just written). Pages written by other programs decode to the sizes their headers
//! state, and pass their checksums, only this way.
//!
//! The far copies (opcodes 0x10–0x1F) keep the length in their low three bits (0: a "Long
//! Compression Offset" + 9 follows) and the distance's bit 14 in bit 3; 0x11 (length 3, bit 14
//! clear) is the end. Reading bit 3 as length makes copies of 10 to 17 bytes come from 16 KB
//! too near, which the ODA's reader rejects.
//!
//! The encoder is greedy, with hash chains over 3-byte prefixes. It never starts a match in the
//! first four bytes, because the leading literal run can only be 0 or at least 4 bytes long.

use crate::error::corrupt;
use crate::{Error, Result};

/// The terminating opcode.
const END: u8 = 0x11;
/// Longest distance of each opcode family (distance = compOffset + 1, plus 0x3FFF for 0x10–0x1F).
const NEAR_SHORT: usize = 0x400;
const NEAR: usize = 0x4000;
const FAR: usize = 0x3FFF + 0x4000;
/// Longest match the encoder emits, and how many chain links it follows.
const MAX_MATCH: usize = 0x1000;
const MAX_CHAIN: usize = 48;

struct In<'a> {
    d: &'a [u8],
    p: usize,
}

impl In<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self.d.get(self.p).ok_or(Error::Truncated("compressed data"))?;
        self.p += 1;
        Ok(b)
    }

    fn peek(&self) -> Option<u8> {
        self.d.get(self.p).copied()
    }

    /// A "Literal Length": 0 when the next byte is an opcode (0x10 or more).
    fn literal_length(&mut self) -> Result<usize> {
        let Some(b) = self.peek() else { return Ok(0) };
        match b {
            0 => {
                self.p += 1;
                let mut total = 0x0Fusize;
                loop {
                    let b = self.byte()?;
                    if b == 0 {
                        total += 0xFF;
                    } else {
                        return Ok(total + usize::from(b) + 3);
                    }
                }
            }
            1..=0x0F => {
                self.p += 1;
                Ok(usize::from(b) + 3)
            }
            _ => Ok(0),
        }
    }

    /// A "Long Compression Offset".
    fn long_count(&mut self) -> Result<usize> {
        let b = self.byte()?;
        if b != 0 {
            return Ok(usize::from(b));
        }
        let mut total = 0xFFusize;
        loop {
            let b = self.byte()?;
            if b == 0 {
                total += 0xFF;
            } else {
                return Ok(total + usize::from(b));
            }
        }
    }

    /// A "Two Byte Offset": (offset, literal count).
    fn two_byte(&mut self) -> Result<(usize, usize)> {
        let a = self.byte()?;
        let b = self.byte()?;
        Ok((usize::from(a >> 2) | usize::from(b) << 6, usize::from(a & 3)))
    }
}

/// Decompresses `data`, which must give at most `max_out` bytes.
pub fn decompress(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(max_out.min(1 << 24));
    let mut r = In { d: data, p: 0 };
    let literals = |r: &mut In, out: &mut Vec<u8>, n: usize| -> Result<()> {
        if out.len() + n > max_out {
            return Err(corrupt("compressed data larger than its stated size"));
        }
        let lit = r.d.get(r.p..r.p + n).ok_or(Error::Truncated("compressed data"))?;
        out.extend_from_slice(lit);
        r.p += n;
        Ok(())
    };
    let n = r.literal_length()?;
    literals(&mut r, &mut out, n)?;
    loop {
        let Some(op) = r.peek() else { return Ok(out) };
        r.p += 1;
        let (count, offset, lit) = match op {
            END => return Ok(out),
            0x10 | 0x12..=0x1F => {
                let count = match op & 7 {
                    0 => r.long_count()? + 9,
                    n => usize::from(n) + 2,
                };
                let (o, l) = r.two_byte()?;
                (count, o + 0x3FFF + (usize::from(op & 8) << 11), l)
            }
            0x20 => {
                let count = r.long_count()? + 0x21;
                let (o, l) = r.two_byte()?;
                (count, o, l)
            }
            0x21..=0x3F => {
                let (o, l) = r.two_byte()?;
                (usize::from(op) - 0x1E, o, l)
            }
            0x40..=0xFF => {
                let op2 = r.byte()?;
                (usize::from(op >> 4) - 1, usize::from(op2) << 2 | usize::from((op & 0x0C) >> 2), usize::from(op & 3))
            }
            _ => return Err(corrupt(format!("compression opcode {op:#x}"))),
        };
        let lit = if lit == 0 { r.literal_length()? } else { lit };
        let distance = offset + 1;
        if distance > out.len() {
            return Err(corrupt("a compressed copy from before the start"));
        }
        if out.len() + count > max_out {
            return Err(corrupt("compressed data larger than its stated size"));
        }
        let from = out.len() - distance;
        // Overlapping copies repeat the bytes just written (length may exceed distance).
        for i in 0..count {
            let b = out.get(from + i).copied().unwrap_or(0);
            out.push(b);
        }
        literals(&mut r, &mut out, lit)?;
    }
}

fn push_long_count(out: &mut Vec<u8>, v: usize) {
    if (1..=0xFF).contains(&v) {
        out.push(v as u8);
        return;
    }
    out.push(0);
    let mut rem = v.saturating_sub(0xFF);
    while rem > 0xFF {
        out.push(0);
        rem -= 0xFF;
    }
    out.push(rem.max(1) as u8);
}

/// A literal run of `n` bytes (0 or at least 4) as a "Literal Length".
fn push_literal_length(out: &mut Vec<u8>, n: usize) {
    match n {
        0 => {}
        4..=0x12 => out.push((n - 3) as u8),
        _ => {
            out.push(0);
            let mut rem = (n - 3).saturating_sub(0x0F);
            while rem > 0xFF {
                out.push(0);
                rem -= 0xFF;
            }
            out.push(rem.max(1) as u8);
        }
    }
}

fn push_two_byte(out: &mut Vec<u8>, offset: usize, lit: usize) {
    out.push(((offset & 0x3F) << 2 | lit) as u8);
    out.push((offset >> 6) as u8);
}

/// The match's opcode bytes; `lit` (0–3) is the literal count carried in them.
fn push_match(out: &mut Vec<u8>, count: usize, distance: usize, lit: usize) {
    let off = distance - 1;
    if distance <= NEAR_SHORT && count <= 14 {
        out.push(((count + 1) << 4 | (off & 3) << 2 | lit) as u8);
        out.push((off >> 2) as u8);
    } else if distance <= NEAR {
        if count <= 0x21 {
            out.push((count + 0x1E) as u8);
        } else {
            out.push(0x20);
            push_long_count(out, count - 0x21);
        }
        push_two_byte(out, off, lit);
    } else {
        // Distances up to 0x7FFF: bit 14 of the stored offset (opcode bit 3) stays clear.
        let off = off - 0x3FFF;
        if count <= 9 {
            out.push(0x10 | (count - 2) as u8);
        } else {
            out.push(0x10);
            push_long_count(out, count - 9);
        }
        push_two_byte(out, off, lit);
    }
}

/// Compresses `data`. Data of one to three bytes cannot be expressed (the first literal run is 0
/// or at least 4 bytes, and three bytes cannot repeat) and is an error.
pub fn compress(data: &[u8]) -> Result<Vec<u8>> {
    if (1..4).contains(&data.len()) {
        return Err(Error::Unsupported("compressing fewer than four bytes".into()));
    }
    const HASH_BITS: u32 = 15;
    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut prev = vec![usize::MAX; data.len()];
    let hash = |p: usize| -> usize {
        let v = u32::from(data[p]) | u32::from(data[p + 1]) << 8 | u32::from(data[p + 2]) << 16;
        (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
    };
    let insert = |p: usize, head: &mut Vec<usize>, prev: &mut Vec<usize>| {
        if p + 3 <= data.len() {
            let h = hash(p);
            prev[p] = head[h];
            head[h] = p;
        }
    };
    let mut out = Vec::with_capacity(data.len() / 2 + 16);
    // Matches found: (start, count, distance).
    let mut matches: Vec<(usize, usize, usize)> = Vec::new();
    let mut p = 0;
    while p + 3 <= data.len() {
        let mut best = (0usize, 0usize);
        if p >= 4 {
            let mut cand = head[hash(p)];
            let mut steps = 0;
            while cand != usize::MAX && steps < MAX_CHAIN {
                let distance = p - cand;
                if distance > FAR {
                    break;
                }
                let limit = (data.len() - p).min(MAX_MATCH);
                let mut n = 0;
                while n < limit && data[cand + n] == data[p + n] {
                    n += 1;
                }
                // Far matches need at least four bytes (opcodes 0x12 up).
                let usable = if distance > NEAR { n >= 4 } else { n >= 3 };
                if usable && n > best.0 {
                    best = (n, distance);
                    if n == limit {
                        break;
                    }
                }
                cand = prev[cand];
                steps += 1;
            }
        }
        if best.0 >= 3 {
            matches.push((p, best.0, best.1));
            for q in p..p + best.0 {
                insert(q, &mut head, &mut prev);
            }
            p += best.0;
        } else {
            insert(p, &mut head, &mut prev);
            p += 1;
        }
    }
    // Emit: the first literal run, then each match with the literals after it.
    let first = matches.first().map_or(data.len(), |m| m.0);
    push_literal_length(&mut out, first);
    out.extend_from_slice(&data[..first]);
    for (i, &(start, count, distance)) in matches.iter().enumerate() {
        let lit_start = start + count;
        let lit_end = matches.get(i + 1).map_or(data.len(), |m| m.0);
        let lit = lit_end - lit_start;
        push_match(&mut out, count, distance, if (1..=3).contains(&lit) { lit } else { 0 });
        if lit >= 4 {
            push_literal_length(&mut out, lit);
        }
        out.extend_from_slice(&data[lit_start..lit_end]);
    }
    out.push(END);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(data: &[u8]) {
        let c = compress(data).unwrap();
        let back = decompress(&c, data.len()).unwrap();
        assert!(back == data, "round trip of {} bytes", data.len());
    }

    #[test]
    fn the_spec_examples_decode() {
        // Spec §4.7: literal length 0x05 is 8 bytes; 0x00 0x02 is 0x14.
        let mut r = In { d: &[0x05], p: 0 };
        assert_eq!(r.literal_length().unwrap(), 8);
        let mut r = In { d: &[0x00, 0x02], p: 0 };
        assert_eq!(r.literal_length().unwrap(), 0x14);
        // Long compression offsets 0xDD and 0x00 0x00 0x34 (0x232).
        let mut r = In { d: &[0xDD], p: 0 };
        assert_eq!(r.long_count().unwrap(), 0xDD);
        let mut r = In { d: &[0x00, 0x00, 0x34], p: 0 };
        assert_eq!(r.long_count().unwrap(), 0x232);
        // Four literals, then copy 3 bytes from 4 back (distance = offset + 1), then end.
        let d = decompress(&[0x01, b'a', b'b', b'c', b'd', 0x40 | 3 << 2, 0x00, END], 100).unwrap();
        assert_eq!(d, b"abcdabc");
    }

    #[test]
    fn data_round_trips() {
        round_trip(&[]);
        round_trip(b"abcd");
        round_trip(&[0; 100_000]);
        round_trip(b"abcdefgh".repeat(5000).as_slice());
        // Every distance family and count range: repeats at many spacings.
        let mut seed = 7u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        };
        let noise: Vec<u8> = (0..40_000).map(|_| rnd()).collect();
        round_trip(&noise);
        let mut mixed = Vec::new();
        for spacing in [5usize, 300, 1000, 5000, 16_000, 20_000, 30_000] {
            let block: Vec<u8> = (0..spacing).map(|_| rnd()).collect();
            mixed.extend_from_slice(&block);
            mixed.extend_from_slice(&block[..block.len().min(700)]);
            mixed.extend_from_slice(&[rnd(), rnd()]);
        }
        round_trip(&mixed);
        for len in 4..300 {
            let d: Vec<u8> = (0..len).map(|i| (i % 7) as u8 ^ (rnd() % 2)).collect();
            round_trip(&d);
        }
        assert!(compress(b"ab").is_err());
        assert!(compress(&[0; 100_000]).unwrap().len() < 1000, "zeros compress");
    }

    #[test]
    fn far_copies_keep_the_length_in_three_bits() {
        // A far copy (distance over 0x4000) of 12 bytes: 0x10 with an extended length (12 - 9),
        // never 0x1A, which other readers take as length 2 + 2 with distance bit 14 set.
        let mut seed = 3u32;
        let block: Vec<u8> = (0..0x5000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let mut data = block.clone();
        data.extend_from_slice(&block[..12]);
        let c = compress(&data).unwrap();
        // 0x10, length 12 - 9, offset 0x5000 - 1 - 0x3FFF as a two-byte offset, the end.
        assert_eq!(c.get(c.len() - 5..), Some(&[0x10, 0x03, 0x00, 0x40, END][..]));
        assert_eq!(decompress(&c, data.len()).unwrap(), data);
        // Decoding: 0x1A is a copy of (2 + 2) bytes from 0x3FFF + 0x4000 + offset + 1 back.
        let mut want: Vec<u8> = (0..0x8100u32).map(|i| (i % 251) as u8).collect();
        let mut lit = Vec::new();
        push_literal_length(&mut lit, want.len());
        lit.extend_from_slice(&want);
        // Offset 1: distance 1 + 0x3FFF + 0x4000 + 1.
        lit.extend_from_slice(&[0x1A, 0x04, 0x00, END]);
        let from = want.len() - 0x8001;
        let copied: Vec<u8> = want[from..from + 4].to_vec();
        want.extend_from_slice(&copied);
        assert_eq!(decompress(&lit, want.len()).unwrap(), want);
    }

    #[test]
    fn hostile_data_is_an_error() {
        assert!(decompress(&[0x40, 0x00], 100).is_err(), "copy before the start");
        assert!(decompress(&[0x0F, 1, 2], 100).is_err(), "truncated literals");
        assert!(decompress(&[0x05, 1, 2, 3, 4, 5, 6, 7, 8], 4).is_err(), "over the stated size");
        assert!(decompress(&[0x00, 0x00, 0x00], 100).is_err());
        let big = compress(&[0; 100_000]).unwrap();
        assert!(decompress(&big, 1000).is_err());
        let mut seed = 99u32;
        for _ in 0..3000 {
            let d: Vec<u8> = (0..(seed % 64))
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (seed >> 16) as u8
                })
                .collect();
            let _ = decompress(&d, 4096);
        }
    }
}
