//! The object map (spec §23): each object's handle and where its record starts. R2000 files
//! give absolute file addresses; R2004+ give offsets into the AcDb:AcDbObjects section.
//!
//! The map is a run of chunks. Each chunk is a big-endian size (counting itself), pairs of
//! modular chars (handle delta, unsigned; location delta, signed), then a big-endian CRC of the
//! chunk (16-bit CRC, seed 0xC0C1). A chunk of size 2 (nothing in it) ends the map. Chunks are at
//! most 2032 bytes (spec §23.1). Both deltas start again from 0 in every chunk: the spec does not
//! say so, but files from other writers decode only that way, and are written the same way here.

use crate::bits::{BitReader, BitWriter};
use crate::crc::crc8;
use crate::error::corrupt;
use crate::{Error, Result};

/// Largest chunk, counting its size field (spec §23.1).
pub const MAX_CHUNK: usize = 2032;
const SEED: u16 = 0xC0C1;

/// One object's place: its handle and where its record starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub handle: u64,
    pub offset: u64,
}

/// Reads an object map from `data` (the map's bytes onward). Returns the entries in stored order
/// and the number of bytes the map took.
pub fn read(data: &[u8], max_entries: usize) -> Result<(Vec<Entry>, usize)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        let size_bytes = data.get(pos..pos + 2).ok_or(Error::Truncated("the object map"))?;
        let size = usize::from(u16::from_be_bytes([size_bytes[0], size_bytes[1]]));
        if size < 2 {
            return Err(corrupt("an object map chunk smaller than its size field"));
        }
        let chunk = data.get(pos..pos + size).ok_or(Error::Truncated("the object map"))?;
        let crc_bytes = data.get(pos + size..pos + size + 2).ok_or(Error::Truncated("the object map"))?;
        let stored = u16::from_be_bytes([crc_bytes[0], crc_bytes[1]]);
        let computed = crc8(SEED, chunk);
        if stored != computed {
            return Err(Error::BadCrc { what: "object map chunk", stored: u32::from(stored), computed: u32::from(computed) });
        }
        pos += size + 2;
        if size == 2 {
            return Ok((out, pos));
        }
        let mut r = BitReader::new(&chunk[2..]);
        let (mut handle, mut offset) = (0u64, 0i64);
        while r.remaining_bits() >= 8 {
            let dh = r.mc_unsigned()?;
            let dl = r.mc()?;
            handle = handle.checked_add(dh).ok_or_else(|| corrupt("an object map handle overflows"))?;
            offset = offset.checked_add(dl).ok_or_else(|| corrupt("an object map offset overflows"))?;
            let offset = u64::try_from(offset).map_err(|_| corrupt("a negative object map offset"))?;
            if out.len() >= max_entries {
                return Err(Error::TooLarge("the object map"));
            }
            out.push(Entry { handle, offset });
        }
    }
}

/// Writes an object map for `entries` (written in the order given).
pub fn write(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let mut body = BitWriter::new();
        let (mut handle, mut offset) = (0u64, 0i64);
        while let Some(e) = entries.get(i) {
            let mut one = BitWriter::new();
            one.mc_unsigned(e.handle.wrapping_sub(handle));
            let off = i64::try_from(e.offset).unwrap_or(i64::MAX);
            one.mc(off.wrapping_sub(offset));
            if 2 + body.as_bytes().len() + one.as_bytes().len() > MAX_CHUNK {
                break;
            }
            body.bytes(one.as_bytes());
            handle = e.handle;
            offset = off;
            i += 1;
        }
        if body.as_bytes().is_empty() {
            // An entry too large for a chunk cannot happen (at most 20 bytes); stop rather than loop.
            break;
        }
        push_chunk(&mut out, body.as_bytes());
    }
    push_chunk(&mut out, &[]);
    out
}

fn push_chunk(out: &mut Vec<u8>, body: &[u8]) {
    let size = (body.len() + 2) as u16;
    let start = out.len();
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(body);
    let crc = crc8(SEED, out.get(start..).unwrap_or_default());
    out.extend_from_slice(&crc.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_round_trip_across_chunks() {
        let entries: Vec<Entry> = (1..5000u64).map(|h| Entry { handle: h * 3, offset: (h * 97) % 100_000 + 0x200 }).collect();
        let bytes = write(&entries);
        let (back, used) = read(&bytes, usize::MAX).unwrap();
        assert_eq!(back, entries);
        assert_eq!(used, bytes.len());
        assert!(bytes.len() > 3 * MAX_CHUNK, "several chunks");
        let (empty, used) = read(&write(&[]), 10).unwrap();
        assert!(empty.is_empty() && used == 4);
    }

    #[test]
    fn damage_is_reported() {
        let mut bytes = write(&[Entry { handle: 1, offset: 100 }, Entry { handle: 2, offset: 50 }]);
        assert!(read(&bytes, 1).is_err(), "too many entries");
        bytes[3] ^= 1;
        assert!(matches!(read(&bytes, 10), Err(Error::BadCrc { .. })));
        assert!(read(&[0, 1], 10).is_err());
        assert!(read(&[0x10, 0], 10).is_err());
    }
}
