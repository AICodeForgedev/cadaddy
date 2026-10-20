//! Sections framed by sentinels (spec §9, §10): a 16-byte start sentinel, an RL size, from R2010
//! (maintenance version above 3) or R2018 a second RL (perhaps the high half of the size), the
//! data, a 16-bit CRC (seed 0xC0C1, over the size fields and the data), and the end sentinel.

use crate::crc::crc8;
use crate::error::corrupt;
use crate::{Error, Result};

/// The header variables section's sentinels (spec §9).
pub const HEADER_START: [u8; 16] = [0xCF, 0x7B, 0x1F, 0x23, 0xFD, 0xDE, 0x38, 0xA9, 0x5F, 0x7C, 0x68, 0xB8, 0x4E, 0x6D, 0x33, 0x5F];
pub const HEADER_END: [u8; 16] = [0x30, 0x84, 0xE0, 0xDC, 0x02, 0x21, 0xC7, 0x56, 0xA0, 0x83, 0x97, 0x47, 0xB1, 0x92, 0xCC, 0xA0];
/// The classes section's sentinels (spec §10).
pub const CLASSES_START: [u8; 16] = [0x8D, 0xA1, 0xC4, 0xB8, 0xC4, 0xA9, 0xF8, 0xC5, 0xC0, 0xDC, 0xF4, 0x5F, 0xE7, 0xCF, 0xB6, 0x8A];
pub const CLASSES_END: [u8; 16] = [0x72, 0x5E, 0x3B, 0x47, 0x3B, 0x56, 0x07, 0x3A, 0x3F, 0x23, 0x0B, 0xA0, 0x18, 0x30, 0x49, 0x75];

const SEED: u16 = 0xC0C1;

/// A framed section's contents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Framed {
    /// The data between the size field(s) and the CRC.
    pub body: Vec<u8>,
    /// The second size field, when the version has one.
    pub hi: Option<u32>,
    /// Bytes after the end sentinel that belong to the section (R2004+: e.g. 8 bytes after the
    /// classes).
    pub trailer: Vec<u8>,
}

/// Reads a framed section from the start of `data`. `has_hi`: the version stores the second RL.
/// Bytes after the end sentinel go to [`Framed::trailer`] when `keep_trailer`.
pub fn read(data: &[u8], start: &[u8; 16], end: &[u8; 16], has_hi: bool, keep_trailer: bool, what: &'static str) -> Result<(Framed, usize)> {
    if data.get(..16) != Some(start.as_slice()) {
        return Err(Error::Corrupt(format!("{what}: start sentinel missing")));
    }
    let rl = |at: usize| -> Result<u32> {
        let b = data.get(at..at + 4).ok_or(Error::Truncated(what))?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let size = usize::try_from(rl(16)?).map_err(|_| corrupt("a section size"))?;
    let (hi, body_at): (Option<u32>, usize) = if has_hi { (Some(rl(20)?), 24) } else { (None, 20) };
    let body_end = body_at.checked_add(size).ok_or(Error::Truncated(what))?;
    let body = data.get(body_at..body_end).ok_or(Error::Truncated(what))?;
    let crc_bytes = data.get(body_end..body_end + 2).ok_or(Error::Truncated(what))?;
    let stored = u16::from_le_bytes([crc_bytes[0], crc_bytes[1]]);
    let computed = crc8(SEED, data.get(16..body_end).unwrap_or_default());
    if stored != computed {
        return Err(Error::BadCrc { what, stored: u32::from(stored), computed: u32::from(computed) });
    }
    if data.get(body_end + 2..body_end + 18) != Some(end.as_slice()) {
        return Err(Error::Corrupt(format!("{what}: end sentinel missing")));
    }
    let used = body_end + 18;
    let trailer = if keep_trailer { data.get(used..).unwrap_or_default().to_vec() } else { Vec::new() };
    Ok((Framed { body: body.to_vec(), hi, trailer }, used))
}

/// Writes a framed section (with its trailer).
pub fn write(f: &Framed, start: &[u8; 16], end: &[u8; 16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(f.body.len() + 64);
    out.extend_from_slice(start);
    out.extend_from_slice(&(f.body.len() as u32).to_le_bytes());
    if let Some(hi) = f.hi {
        out.extend_from_slice(&hi.to_le_bytes());
    }
    out.extend_from_slice(&f.body);
    let crc = crc8(SEED, out.get(16..).unwrap_or_default());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(end);
    out.extend_from_slice(&f.trailer);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_sections_round_trip_and_check() {
        for hi in [None, Some(0)] {
            let f = Framed { body: (0..200u8).collect(), hi, trailer: vec![0; 8] };
            let bytes = write(&f, &HEADER_START, &HEADER_END);
            let (back, used) = read(&bytes, &HEADER_START, &HEADER_END, hi.is_some(), true, "header").unwrap();
            assert_eq!((back, used), (f.clone(), bytes.len() - 8));
            let mut bad = bytes.clone();
            bad[30] ^= 0x10;
            assert!(matches!(read(&bad, &HEADER_START, &HEADER_END, hi.is_some(), true, "header"), Err(Error::BadCrc { .. })));
            assert!(read(&bytes, &CLASSES_START, &CLASSES_END, hi.is_some(), true, "classes").is_err());
            assert!(read(&bytes[..100], &HEADER_START, &HEADER_END, hi.is_some(), true, "header").is_err());
        }
    }
}
