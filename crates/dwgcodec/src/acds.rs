//! The data storage section (AcDb:AcDsPrototype_1b): from AutoCAD 2013 on, the ACIS (ASM) data of
//! 3D solids, regions and bodies is kept here, one record per entity bound by its handle, not in
//! the entity's own object.
//!
//! The layout is the one the files we have seen use (observed in files of another writer, a
//! black box; no AutoCAD file was at hand): a 0x80-byte header, then segments each with a 0x30
//! byte header (the signature 0xD5AC, a six-character name, its number, its size, and 0x55 fill)
//! and its data padded with `p`: two `_data_` segments (the records, then an empty one), `datidx`
//! (where each record is), `schdat` and `schidx` (the schemas: fixed here), `search` (the
//! records by handle) and `segidx` (where each segment is). A record is a 20-byte entry (its size,
//! 1, the handle, the offset of its data) in the first `_data_` segment, and its data (a length,
//! then the bytes) after the entries, which are padded to 16 bytes with `b`.
//!
//! [`decode`] reads what that layout holds (the handles and their data) and refuses anything else;
//! [`encode`] writes it. A file that isn't read is reported, not guessed at.

use crate::error::corrupt;
use crate::{Error, Result};

/// Most records one section holds.
pub const MAX_RECORDS: usize = 1 << 20;

const HEADER: usize = 0x80;
const SEGMENT_HEADER: usize = 0x30;
const SIGNATURE: [u8; 2] = [0xAC, 0xD5];
const FILE_SIGNATURE: u32 = 0x6472_616A;
const FILL: u8 = 0x55;
const PAD: u8 = b'p';
const BLOCK_FILL: u8 = b'b';
const RECORD: usize = 20;

const SCHEMA_DATA: [u8; 393] = [
    0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x02, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00,
    0x02, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0f, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00,
    0x02, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x0f, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00, 0x00, 0x06, 0x00,
    0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x73, 0x73, 0x73,
    0x07, 0x00, 0x00, 0x00, 0x41, 0x63, 0x44, 0x62, 0x44, 0x73, 0x3a, 0x3a, 0x49, 0x44, 0x00, 0x54,
    0x68, 0x75, 0x6d, 0x62, 0x6e, 0x61, 0x69, 0x6c, 0x5f, 0x44, 0x61, 0x74, 0x61, 0x00, 0x41, 0x53,
    0x4d, 0x5f, 0x44, 0x61, 0x74, 0x61, 0x00, 0x41, 0x63, 0x44, 0x62, 0x44, 0x73, 0x3a, 0x3a, 0x54,
    0x72, 0x65, 0x61, 0x74, 0x65, 0x64, 0x41, 0x73, 0x4f, 0x62, 0x6a, 0x65, 0x63, 0x74, 0x44, 0x61,
    0x74, 0x61, 0x00, 0x41, 0x63, 0x44, 0x62, 0x44, 0x73, 0x3a, 0x3a, 0x4c, 0x65, 0x67, 0x61, 0x63,
    0x79, 0x00, 0x41, 0x63, 0x44, 0x73, 0x3a, 0x49, 0x6e, 0x64, 0x65, 0x78, 0x61, 0x62, 0x6c, 0x65,
    0x00, 0x41, 0x63, 0x44, 0x62, 0x44, 0x73, 0x3a, 0x3a, 0x48, 0x61, 0x6e, 0x64, 0x6c, 0x65, 0x41,
    0x74, 0x74, 0x72, 0x69, 0x62, 0x75, 0x74, 0x65, 0x00,
];

const SCHEMA_INDEX: [u8; 354] = [
    0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
    0x40, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00,
    0x02, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00,
    0x05, 0x00, 0x00, 0x00, 0xd2, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
    0xe4, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0xf6, 0x00, 0x00, 0x00,
    0x0c, 0xf1, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
    0x08, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00,
    0x04, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
    0x05, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
    0x28, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x30, 0x00, 0x00, 0x00,
    0x04, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x38, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
    0x06, 0x00, 0x00, 0x00, 0x41, 0x63, 0x44, 0x62, 0x5f, 0x54, 0x68, 0x75, 0x6d, 0x62, 0x6e, 0x61,
    0x69, 0x6c, 0x5f, 0x53, 0x63, 0x68, 0x65, 0x6d, 0x61, 0x00, 0x41, 0x63, 0x44, 0x62, 0x33, 0x44,
    0x53, 0x6f, 0x6c, 0x69, 0x64, 0x5f, 0x41, 0x53, 0x4d, 0x5f, 0x44, 0x61, 0x74, 0x61, 0x00, 0x41,
    0x63, 0x44, 0x62, 0x44, 0x73, 0x3a, 0x3a, 0x54, 0x72, 0x65, 0x61, 0x74, 0x65, 0x64, 0x41, 0x73,
    0x4f, 0x62, 0x6a, 0x65, 0x63, 0x74, 0x44, 0x61, 0x74, 0x61, 0x53, 0x63, 0x68, 0x65, 0x6d, 0x61,
    0x00, 0x41, 0x63, 0x44, 0x62, 0x44, 0x73, 0x3a, 0x3a, 0x4c, 0x65, 0x67, 0x61, 0x63, 0x79, 0x53,
    0x63, 0x68, 0x65, 0x6d, 0x61, 0x00, 0x41, 0x63, 0x44, 0x62, 0x44, 0x73, 0x3a, 0x3a, 0x49, 0x6e,
    0x64, 0x65, 0x78, 0x65, 0x64, 0x50, 0x72, 0x6f, 0x70, 0x65, 0x72, 0x74, 0x79, 0x53, 0x63, 0x68,
    0x65, 0x6d, 0x61, 0x00, 0x41, 0x63, 0x44, 0x62, 0x44, 0x73, 0x3a, 0x3a, 0x48, 0x61, 0x6e, 0x64,
    0x6c, 0x65, 0x41, 0x74, 0x74, 0x72, 0x69, 0x62, 0x75, 0x74, 0x65, 0x53, 0x63, 0x68, 0x65, 0x6d,
    0x61, 0x00,
];

fn align16(n: usize) -> usize {
    n.div_ceil(16) * 16
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// One segment: its header and `payload`, padded with `p` to at least `min` bytes and a multiple
/// of 16.
fn segment(out: &mut Vec<u8>, name: &[u8; 6], number: u32, (a, b): (u32, u32), payload: &[u8], min: usize) {
    let size = SEGMENT_HEADER + align16(payload.len()).max(min);
    out.extend_from_slice(&SIGNATURE);
    out.extend_from_slice(name);
    for v in [number, 0, u32::try_from(size).unwrap_or(u32::MAX), 0, 1, 0, a, b] {
        put32(out, v);
    }
    out.extend_from_slice(&[FILL; 8]);
    out.extend_from_slice(payload);
    out.resize(out.len() + size - SEGMENT_HEADER - payload.len(), PAD);
}

fn words(list: &[u32]) -> Vec<u8> {
    list.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// The order a balanced search tree of `lo..=hi` is walked in, parents first (what the search
/// segment lists).
fn tree_order(lo: u32, hi: u32, out: &mut Vec<u32>) {
    if lo > hi {
        return;
    }
    let mid = lo + (hi - lo) / 2;
    out.push(mid);
    if mid > lo {
        tree_order(lo, mid - 1, out);
    }
    tree_order(mid + 1, hi, out);
}

/// The section for `records` (handle and ASM data), or `None` for none.
pub fn encode(records: &[(u64, Vec<u8>)]) -> Option<Vec<u8>> {
    let n = records.len();
    if n == 0 || n > MAX_RECORDS {
        return None;
    }
    // The records: their 20-byte entries, then (from a multiple of 16) each one's data.
    let mut entries = Vec::with_capacity(n * RECORD);
    let mut blobs: Vec<u8> = Vec::new();
    for (h, data) in records {
        put32(&mut entries, RECORD as u32);
        put32(&mut entries, 1);
        entries.extend_from_slice(&h.to_le_bytes());
        put32(&mut entries, u32::try_from(blobs.len()).ok()?);
        put32(&mut blobs, u32::try_from(data.len()).ok()?);
        blobs.extend_from_slice(data);
    }
    let mut records_seg = entries;
    let start = align16(records_seg.len());
    records_seg.resize(start, BLOCK_FILL);
    records_seg.extend_from_slice(&blobs);
    let mut datidx = vec![u32::try_from(n).ok()?, 0];
    let mut search = vec![2, 1, u32::try_from(n).ok()?, 0, 0, 0, 1];
    let mut order = Vec::new();
    tree_order(1, u32::try_from(n).ok()?, &mut order);
    for k in order {
        search.extend([0, k]);
    }
    for (i, (h, _)) in records.iter().enumerate() {
        datidx.extend([2, u32::try_from(i.checked_mul(RECORD)?).ok()?, 1]);
        search.extend([(*h & 0xFFFF_FFFF) as u32, (*h >> 32) as u32, 1, 0, u32::try_from(i).ok()?, 0]);
    }
    search.extend([0, 0, 0, 1, 0, 0]);

    let mut segs: Vec<(usize, usize)> = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    let mut put = |body: &mut Vec<u8>, name: &[u8; 6], number: u32, ab: (u32, u32), payload: &[u8], min: usize| {
        let at = HEADER + body.len();
        segment(body, name, number, ab, payload, min);
        segs.push((at, HEADER + body.len() - at));
    };
    put(&mut body, b"_data_", 2, (0, u32::try_from(n).ok()?.checked_add(4)?), &records_seg, 0);
    put(&mut body, b"_data_", 3, (0, 4), &[BLOCK_FILL; 16], 0);
    put(&mut body, b"datidx", 4, (0, 0), &words(&datidx), 80);
    put(&mut body, b"schdat", 5, (0x14, 0), &SCHEMA_DATA, 400);
    put(&mut body, b"schidx", 6, (0x0f, 0), &SCHEMA_INDEX, 400);
    put(&mut body, b"search", 7, (0, 0), &words(&search), 144);
    // The segment index lists itself first, then the others in the file's order.
    let segidx_at = HEADER + body.len();
    let mut index = vec![0, 0, 0];
    let idx_size = SEGMENT_HEADER + 144;
    for (at, size) in std::iter::once((segidx_at, idx_size)).chain(segs.iter().copied()) {
        index.extend([u32::try_from(at).ok()?, 0, u32::try_from(size).ok()?]);
    }
    segment(&mut body, b"segidx", 1, (0, 0), &words(&index), 144);
    let total = HEADER + body.len();
    let mut out = Vec::with_capacity(total);
    for v in [FILE_SIGNATURE, HEADER as u32, 2, 2, 0, 1, u32::try_from(segidx_at).ok()?, 0, 8, 6, 4, 7, 0, u32::try_from(total).ok()?] {
        put32(&mut out, v);
    }
    out.resize(HEADER, 0);
    out.extend_from_slice(&body);
    Some(out)
}

fn u32_at(d: &[u8], at: usize) -> Result<u32> {
    let b = d.get(at..at.checked_add(4).ok_or_else(|| corrupt("the data storage"))?).ok_or(Error::Truncated("the data storage"))?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// A segment: its name, number and data (after its header).
struct Segment<'a> {
    name: [u8; 6],
    number: u32,
    data: &'a [u8],
}

fn segments(d: &[u8]) -> Result<Vec<Segment<'_>>> {
    if u32_at(d, 0)? != FILE_SIGNATURE || usize::try_from(u32_at(d, 4)?).ok() != Some(HEADER) {
        return Err(corrupt("the data storage's header"));
    }
    let mut out = Vec::new();
    let mut at = HEADER;
    while at < d.len() {
        if d.get(at..at + 2) != Some(&SIGNATURE) {
            return Err(corrupt("a data storage segment's signature"));
        }
        let size = usize::try_from(u32_at(d, at + 0x10)?).map_err(|_| corrupt("a data storage segment's size"))?;
        let end = at.checked_add(size).filter(|e| *e <= d.len() && size >= SEGMENT_HEADER).ok_or_else(|| corrupt("a data storage segment's size"))?;
        let mut name = [0u8; 6];
        name.copy_from_slice(d.get(at + 2..at + 8).ok_or(Error::Truncated("a data storage segment"))?);
        out.push(Segment { name, number: u32_at(d, at + 8)?, data: d.get(at + SEGMENT_HEADER..end).ok_or(Error::Truncated("a data storage segment"))? });
        at = end;
        if out.len() > 1 << 16 {
            return Err(Error::TooLarge("the data storage's segments"));
        }
    }
    Ok(out)
}

/// The records of a data storage section: each one's handle and ASM data.
pub fn decode(d: &[u8]) -> Result<Vec<(u64, Vec<u8>)>> {
    let segs = segments(d)?;
    let index = segs.iter().find(|s| &s.name == b"datidx").ok_or_else(|| corrupt("no record index in the data storage"))?;
    let n = usize::try_from(u32_at(index.data, 0)?).map_err(|_| corrupt("the data storage's record count"))?;
    if n > MAX_RECORDS || n.checked_mul(12).and_then(|b| b.checked_add(8)).is_none_or(|b| b > index.data.len()) {
        return Err(corrupt("the data storage's record count"));
    }
    // Each segment's records: where they are, and how many there are in it (the data follows
    // the entries, from a multiple of 16).
    let mut places: Vec<(u32, usize)> = Vec::with_capacity(n);
    let mut per_segment: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for k in 0..n {
        let at = 8 + 12 * k;
        let (seg, off) = (u32_at(index.data, at)?, usize::try_from(u32_at(index.data, at + 4)?).map_err(|_| corrupt("a record's place"))?);
        places.push((seg, off));
        *per_segment.entry(seg).or_default() += 1;
    }
    let mut out = Vec::with_capacity(n);
    for (seg, off) in places {
        let s = segs.iter().find(|s| &s.name == b"_data_" && s.number == seg).ok_or_else(|| corrupt("a record's segment"))?;
        let count = per_segment.get(&seg).copied().unwrap_or(0);
        let entry = s.data.get(off..off.checked_add(RECORD).ok_or_else(|| corrupt("a record"))?).ok_or(Error::Truncated("a record"))?;
        if u32_at(entry, 0)? as usize != RECORD || u32_at(entry, 4)? != 1 {
            return Err(corrupt("a record's entry"));
        }
        let handle = u64::from(u32_at(entry, 8)?) | u64::from(u32_at(entry, 12)?) << 32;
        let blob_at = align16(count.checked_mul(RECORD).ok_or_else(|| corrupt("a record"))?)
            .checked_add(usize::try_from(u32_at(entry, 16)?).map_err(|_| corrupt("a record's data"))?)
            .ok_or_else(|| corrupt("a record's data"))?;
        let len = usize::try_from(u32_at(s.data, blob_at)?).map_err(|_| corrupt("a record's data size"))?;
        let from = blob_at + 4;
        let bytes = s.data.get(from..from.checked_add(len).ok_or_else(|| corrupt("a record's data size"))?).ok_or(Error::Truncated("a record's data"))?;
        out.push((handle, bytes.to_vec()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> Vec<(u64, Vec<u8>)> {
        (0..n).map(|i| (0x2A + i as u64, (0..(10 + i * 37)).map(|b| (b * 7 + 3) as u8).collect())).collect()
    }

    #[test]
    fn records_round_trip() {
        for n in [1, 2, 3, 4, 7, 20] {
            let recs = sample(n);
            let bytes = encode(&recs).unwrap();
            assert_eq!(bytes.len() % 16, 0, "{n}");
            assert_eq!(decode(&bytes).unwrap(), recs, "{n}");
        }
        // A handle past 32 bits, and an empty blob.
        let recs = vec![(0x1_0000_00FF, vec![]), (7, vec![1, 2, 3])];
        assert_eq!(decode(&encode(&recs).unwrap()).unwrap(), recs);
        assert!(encode(&[]).is_none());
    }

    #[test]
    fn damaged_sections_are_errors_not_panics() {
        let bytes = encode(&sample(3)).unwrap();
        for cut in [0, 3, 0x7f, 0x80, 0x90, bytes.len() / 2, bytes.len() - 1] {
            let _ = decode(&bytes[..cut]);
        }
        let mut x = 0x2545_f491u32;
        for _ in 0..4000 {
            let mut b = bytes.clone();
            for _ in 0..3 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let i = x as usize % b.len();
                b[i] ^= (x >> 8) as u8 | 1;
            }
            let _ = decode(&b);
        }
    }
}
