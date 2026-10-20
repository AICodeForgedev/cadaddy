//! Object records (spec §20.1): a modular-short size, that many bytes of bit-coded object data,
//! and a 16-bit CRC (seed 0xC0C1) over the size and the data. The data starts with the object's
//! type (spec §2.12), after an MC handle-stream size from R2010.
//!
//! From R2010 the size does not count that MC: the record is the size, the MC, `size` bytes of
//! data, then the CRC over all of them. The spec does not say so; in files from other writers
//! every record's CRC checks only this way (the bytes left over equal the MC's length, one or two).

use crate::bits::{BitReader, BitWriter};
use crate::crc::crc8;
use crate::{Error, Result};

const SEED: u16 = 0xC0C1;
/// Largest object record (spec §2.7: modular shorts allow 1 GB; real objects are far smaller).
pub const MAX_OBJECT_BYTES: u64 = 256 << 20;

/// One object as stored: its handle (from the object map), type number and data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawObject {
    pub handle: u64,
    /// The object type (spec §2.12, §20.3: below 500 a fixed type, from 500 a class number).
    pub type_code: u16,
    /// The record's data: everything between its size and its CRC (from R2010 starting with the
    /// MC handle-stream size).
    pub data: Vec<u8>,
}

/// The object type at the start of an object's data.
pub fn type_code(data: &[u8], r2010_plus: bool) -> Result<u16> {
    let mut r = BitReader::new(data);
    if r2010_plus {
        r.mc_unsigned()?;
    }
    r.ot(r2010_plus)
}

/// Reads the record at `at` in `buf`: the object's data and the record's length in bytes.
pub fn read(buf: &[u8], at: usize, r2010_plus: bool, handle: u64) -> Result<(RawObject, usize)> {
    let rest = buf.get(at..).ok_or(Error::Truncated("an object (its offset is past the data)"))?;
    let mut r = BitReader::new(rest);
    let size = r.ms()?;
    if size > MAX_OBJECT_BYTES {
        return Err(Error::TooLarge("an object"));
    }
    let start = r.byte_pos();
    let mc_len = if r2010_plus {
        r.mc_unsigned()?;
        r.byte_pos() - start
    } else {
        0
    };
    let end = start + mc_len + size as usize;
    let data = rest.get(start..end).ok_or(Error::Truncated("an object"))?;
    let crc = rest.get(end..end + 2).ok_or(Error::Truncated("an object"))?;
    let stored = u16::from_le_bytes([crc[0], crc[1]]);
    let computed = crc8(SEED, rest.get(..end).unwrap_or_default());
    if stored != computed {
        return Err(Error::BadCrc { what: "object", stored: u32::from(stored), computed: u32::from(computed) });
    }
    let type_code = type_code(data, r2010_plus)?;
    Ok((RawObject { handle, type_code, data: data.to_vec() }, end + 2))
}

/// Appends an object's record to `out`.
pub fn write(out: &mut Vec<u8>, o: &RawObject, r2010_plus: bool) {
    let start = out.len();
    let mc_len = if r2010_plus {
        let mut r = BitReader::new(&o.data);
        r.mc_unsigned().map_or(0, |_| r.byte_pos())
    } else {
        0
    };
    let mut w = BitWriter::new();
    w.ms(o.data.len().saturating_sub(mc_len) as u64);
    out.extend_from_slice(w.as_bytes());
    out.extend_from_slice(&o.data);
    let crc = crc8(SEED, out.get(start..).unwrap_or_default());
    out.extend_from_slice(&crc.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip() {
        for r2010 in [false, true] {
            let mut w = BitWriter::new();
            if r2010 {
                w.mc_unsigned(40);
            }
            w.ot(0x1F3, r2010);
            w.bytes(&[7; 70_000]);
            let o = RawObject { handle: 0x2A, type_code: 0x1F3, data: w.into_bytes() };
            let mut buf = vec![9, 9];
            write(&mut buf, &o, r2010);
            let (back, len) = read(&buf, 2, r2010, 0x2A).unwrap();
            assert_eq!((back, len), (o.clone(), buf.len() - 2));
            let last = buf.len() - 1;
            buf[last] ^= 1;
            assert!(read(&buf, 2, r2010, 0x2A).is_err());
            assert!(read(&buf, 100_000, r2010, 0x2A).is_err());
        }
    }
}
