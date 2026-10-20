//! The R2007 compression (spec §5.10), another LZ77 variant: runs of literal bytes (stored in a
//! shuffled order) alternate with copies from the output already written.
//!
//! The literal order is spec §5.10.1's table applied within each listed block in turn (observed:
//! a 2-byte block is reversed inside a 10-byte run; only that reproduces the stored file header
//! CRC). Writing stores data uncompressed, as the spec allows when compression doesn't help, so
//! [`literal_run`] only exists to test the reader against the format.

use crate::error::corrupt;
use crate::{Error, Result};

/// Spec §5.10.1: for each run length 1–32, its blocks (size, source offset) in output order.
const LITERAL: [&[(usize, usize)]; 32] = [
    &[(1, 0)],
    &[(1, 1), (1, 0)],
    &[(1, 2), (1, 1), (1, 0)],
    &[(1, 0), (1, 1), (1, 2), (1, 3)],
    &[(1, 4), (4, 0)],
    &[(1, 5), (4, 1), (1, 0)],
    &[(2, 5), (4, 1), (1, 0)],
    &[(4, 0), (4, 4)],
    &[(1, 8), (8, 0)],
    &[(1, 9), (8, 1), (1, 0)],
    &[(2, 9), (8, 1), (1, 0)],
    &[(4, 8), (8, 0)],
    &[(1, 12), (4, 8), (8, 0)],
    &[(1, 13), (4, 9), (8, 1), (1, 0)],
    &[(2, 13), (4, 9), (8, 1), (1, 0)],
    &[(8, 8), (8, 0)],
    &[(8, 9), (1, 8), (8, 0)],
    &[(1, 17), (16, 1), (1, 0)],
    &[(3, 16), (16, 0)],
    &[(4, 16), (16, 0)],
    &[(1, 20), (4, 16), (16, 0)],
    &[(2, 20), (4, 16), (16, 0)],
    &[(3, 20), (4, 16), (16, 0)],
    &[(8, 16), (16, 0)],
    &[(8, 17), (1, 16), (16, 0)],
    &[(1, 25), (8, 17), (1, 16), (16, 0)],
    &[(2, 25), (8, 17), (1, 16), (16, 0)],
    &[(4, 24), (8, 16), (16, 0)],
    &[(1, 28), (4, 24), (8, 16), (16, 0)],
    &[(2, 28), (4, 24), (8, 16), (16, 0)],
    &[(1, 30), (4, 26), (8, 18), (16, 2), (2, 0)],
    &[(16, 16), (16, 0)],
];

/// The source offsets of an `n`-byte block (1–32) in output order.
fn order(n: usize, base: usize, out: &mut Vec<usize>) {
    if n == 1 {
        out.push(base);
        return;
    }
    let Some(blocks) = n.checked_sub(1).and_then(|i| LITERAL.get(i)) else { return };
    for &(size, at) in *blocks {
        order(size, base + at, out);
    }
}

/// Appends `src`'s literal run to `out`: 32 bytes at a time, then the rest.
fn copy_literal(src: &[u8], out: &mut Vec<u8>) {
    let mut idx = Vec::with_capacity(32);
    for chunk in src.chunks(32) {
        idx.clear();
        order(chunk.len(), 0, &mut idx);
        out.extend(idx.iter().filter_map(|&i| chunk.get(i)));
    }
}

struct Reader<'a> {
    src: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self.src.get(self.at).ok_or(Error::Truncated("compressed data"))?;
        self.at += 1;
        Ok(b)
    }
    fn done(&self) -> bool {
        self.at >= self.src.len()
    }
}

/// A literal run's length from its opcode (spec §5.10.1 ReadLiteralLength).
fn literal_length(r: &mut Reader, op: u8) -> Result<usize> {
    let mut len = usize::from(op) + 8;
    if len == 0x17 {
        let n = r.byte()?;
        len += usize::from(n);
        if n == 0xff {
            loop {
                let n = u16::from_le_bytes([r.byte()?, r.byte()?]);
                len += usize::from(n);
                if n != 0xffff {
                    break;
                }
            }
        }
    }
    Ok(len)
}

/// A copy's (offset back, length) and the opcode after it (spec §5.10.2 ReadInstructions).
fn instructions(r: &mut Reader, op: u8) -> Result<(usize, usize, u8)> {
    let op = usize::from(op);
    Ok(match op >> 4 {
        0 => {
            let mut length = (op & 0xf) + 0x13;
            let off = usize::from(r.byte()?);
            let next = usize::from(r.byte()?);
            length += (next >> 3) & 0x10;
            (((next & 0x78) << 5) + 1 + off, length, next as u8)
        }
        1 => {
            let length = (op & 0xf) + 3;
            let off = usize::from(r.byte()?);
            let next = usize::from(r.byte()?);
            (((next & 0xf8) << 5) + 1 + off, length, next as u8)
        }
        2 => {
            let mut off = usize::from(r.byte()?);
            off |= usize::from(r.byte()?) << 8;
            let mut length = op & 7;
            let next;
            if op & 8 == 0 {
                next = usize::from(r.byte()?);
                length += next & 0xf8;
            } else {
                off += 1;
                length += usize::from(r.byte()?) << 3;
                next = usize::from(r.byte()?);
                length += ((next & 0xf8) << 8) + 0x100;
            }
            (off, length, next as u8)
        }
        _ => {
            let length = op >> 4;
            let next = usize::from(r.byte()?);
            ((((next & 0xf8) << 1) + (op & 15)) + 1, length, next as u8)
        }
    })
}

/// Decompresses `src` into exactly `size` bytes.
pub fn decompress(src: &[u8], size: usize) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(size.min(1 << 24));
    let mut r = Reader { src, at: 0 };
    let mut op = r.byte()?;
    let mut length = 0;
    if op >> 4 == 2 {
        r.at = 3;
        length = usize::from(*src.get(2).ok_or(Error::Truncated("compressed data"))? & 7);
    }
    while !r.done() {
        if length == 0 {
            length = literal_length(&mut r, op)?;
        }
        let lit = src.get(r.at..r.at.checked_add(length).ok_or_else(|| corrupt("a literal length"))?).ok_or(Error::Truncated("compressed data"))?;
        if out.len() + length > size {
            return Err(corrupt("compressed data longer than its stated size"));
        }
        copy_literal(lit, &mut out);
        r.at += length;
        if r.done() {
            break;
        }
        op = r.byte()?;
        let (mut off, mut len, next) = instructions(&mut r, op)?;
        op = next;
        loop {
            if off == 0 || off > out.len() || out.len() + len > size {
                return Err(corrupt("a compressed copy outside the data"));
            }
            for _ in 0..len {
                let b = out[out.len() - off];
                out.push(b);
            }
            length = usize::from(op & 7);
            if length != 0 || r.done() {
                break;
            }
            op = r.byte()?;
            if op >> 4 == 0 {
                break;
            }
            if op >> 4 == 15 {
                op &= 15;
            }
            (off, len, op) = instructions(&mut r, op)?;
        }
    }
    if out.len() != size {
        return Err(corrupt(format!("compressed data makes {} bytes, not {size}", out.len())));
    }
    Ok(out)
}

/// `data` (at least 8 bytes) as one literal run: what [`decompress`] reads back as `data`.
pub fn literal_run(data: &[u8]) -> Option<Vec<u8>> {
    let n = data.len().checked_sub(8)?;
    let mut out = Vec::with_capacity(data.len() + 8);
    if n < 0x0f {
        out.push(n as u8);
    } else {
        out.push(0x0f);
        let mut rest = n - 0x0f;
        if rest < 0xff {
            out.push(rest as u8);
        } else {
            out.push(0xff);
            rest -= 0xff;
            loop {
                let k = rest.min(0xffff);
                out.extend_from_slice(&(k as u16).to_le_bytes());
                rest -= k;
                if k != 0xffff {
                    break;
                }
            }
        }
    }
    // The bytes in the order that the reader's shuffle puts back in place.
    for chunk in data.chunks(32) {
        let mut idx = Vec::with_capacity(32);
        order(chunk.len(), 0, &mut idx);
        let mut stored = vec![0u8; chunk.len()];
        for (pos, &src) in idx.iter().enumerate() {
            if let (Some(slot), Some(&b)) = (stored.get_mut(src), chunk.get(pos)) {
                *slot = b;
            }
        }
        out.extend_from_slice(&stored);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_block_order_is_a_permutation() {
        for n in 1..=32 {
            let mut idx = Vec::new();
            order(n, 0, &mut idx);
            let mut s = idx.clone();
            s.sort_unstable();
            assert_eq!(s, (0..n).collect::<Vec<_>>(), "{n}");
        }
    }

    #[test]
    fn literal_runs_read_back() {
        for n in [8usize, 9, 22, 23, 24, 31, 32, 33, 64, 300, 70_000] {
            let data: Vec<u8> = (0..n).map(|i| (i * 7 + i / 13) as u8).collect();
            let c = literal_run(&data).unwrap();
            assert_eq!(decompress(&c, n).unwrap(), data, "{n}");
        }
    }

    #[test]
    fn hostile_input_is_an_error() {
        for src in [&[][..], &[0x0f][..], &[0x20, 0, 0][..], &[0x00, 0x01][..], &[0x01, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x40, 0xff][..]] {
            let _ = decompress(src, 64);
        }
        assert!(decompress(&[0x00, 1, 2, 3, 4, 5, 6, 7, 8], 4).is_err());
    }
}
