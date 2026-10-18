//! The .cdy container: a fixed header, tagged chunks each with its own schema version, length
//! and CRC-32, an index chunk and a trailer that points at it (see `docs/cdy-format.md`).
//!
//! Reading checks every length against the bytes that are there and against caps before it
//! allocates; a chunk whose CRC doesn't match is reported and left out. A file whose trailer or
//! index is damaged is read by walking the chunks from the header.

use serde::{Deserialize, Serialize};

/// The first bytes of every .cdy file: the name, then bytes that a text-mode copy would mangle.
pub const MAGIC: [u8; 8] = *b"CADDY\x1a\r\n";
/// The container's own version (chunk layouts carry theirs).
pub const FORMAT_VERSION: u16 = 1;
/// Header: magic, format version, flags, reserved.
pub const HEADER_LEN: usize = 16;
/// Chunk header: tag (4), schema version (2), flags (2), stored length (8), CRC-32 (4), raw length (8).
pub const CHUNK_HEADER_LEN: usize = 28;
/// Trailer: index chunk offset (8), then this.
pub const TRAILER_MAGIC: [u8; 8] = *b"CDYINDEX";
pub const TRAILER_LEN: usize = 16;
/// Chunk flag: the payload is deflate-compressed.
pub const FLAG_DEFLATE: u16 = 1;
/// Most bytes one chunk inflates to (a hostile file can't ask for more memory than this).
pub const MAX_RAW: u64 = 1 << 31;
/// Most chunks one file holds.
pub const MAX_CHUNKS: usize = 1_000_000;

/// One chunk as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub tag: [u8; 4],
    pub version: u16,
    /// The payload as the writer produced it (uncompressed).
    pub data: Vec<u8>,
}

/// Where a chunk is in the file (the index).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub tag: [u8; 4],
    pub version: u16,
    pub offset: u64,
    /// Stored length (compressed when deflated).
    pub len: u64,
    pub raw_len: u64,
}

/// Writes a .cdy file chunk by chunk.
pub struct Writer {
    out: Vec<u8>,
    index: Vec<IndexEntry>,
}

impl Writer {
    pub fn new() -> Self {
        let mut out = Vec::with_capacity(1 << 16);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        Writer { out, index: Vec::new() }
    }

    /// Append a chunk; `deflate` compresses its payload (small or already compressed data, like a
    /// PNG, is stored as it is).
    pub fn chunk(&mut self, tag: [u8; 4], version: u16, data: &[u8], deflate: bool) {
        let packed = if deflate { Some(miniz_oxide::deflate::compress_to_vec(data, 3)) } else { None };
        let (flags, stored): (u16, &[u8]) = match &packed {
            Some(p) if p.len() < data.len() => (FLAG_DEFLATE, p),
            _ => (0, data),
        };
        let offset = self.out.len() as u64;
        self.out.extend_from_slice(&tag);
        self.out.extend_from_slice(&version.to_le_bytes());
        self.out.extend_from_slice(&flags.to_le_bytes());
        self.out.extend_from_slice(&(stored.len() as u64).to_le_bytes());
        self.out.extend_from_slice(&crc32fast::hash(stored).to_le_bytes());
        self.out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        self.out.extend_from_slice(stored);
        self.index.push(IndexEntry { tag, version, offset, len: stored.len() as u64, raw_len: data.len() as u64 });
    }

    /// Finish: the index chunk and the trailer.
    pub fn finish(mut self) -> Vec<u8> {
        let index = postcard::to_allocvec(&self.index).unwrap_or_default();
        let at = self.out.len() as u64;
        self.chunk(*b"INDX", 1, &index, false);
        self.out.extend_from_slice(&at.to_le_bytes());
        self.out.extend_from_slice(&TRAILER_MAGIC);
        self.out
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether `bytes` start like a .cdy file.
pub fn is_cdy(bytes: &[u8]) -> bool {
    bytes.get(..MAGIC.len()) == Some(&MAGIC[..])
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}
fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

/// A chunk read from a file, or why it couldn't be.
pub enum Read {
    Ok(Chunk),
    /// Its CRC or compression is damaged (tag and version as far as they could be read).
    Damaged {
        tag: [u8; 4],
        why: String,
    },
}

/// The chunk at `at`, and where the next one starts.
fn chunk_at(b: &[u8], at: usize) -> Option<(Read, usize)> {
    let tag: [u8; 4] = b.get(at..at.checked_add(4)?)?.try_into().ok()?;
    let version = u16_at(b, at + 4)?;
    let flags = u16_at(b, at + 6)?;
    let len = u64_at(b, at + 8)?;
    let crc = u32_at(b, at + 16)?;
    let raw_len = u64_at(b, at + 20)?;
    let start = at.checked_add(CHUNK_HEADER_LEN)?;
    let end = start.checked_add(usize::try_from(len).ok()?)?;
    let stored = b.get(start..end)?;
    if crc32fast::hash(stored) != crc {
        return Some((Read::Damaged { tag, why: "its checksum doesn't match".into() }, end));
    }
    if raw_len > MAX_RAW {
        return Some((Read::Damaged { tag, why: "it is larger than a chunk may be".into() }, end));
    }
    let data = if flags & FLAG_DEFLATE != 0 {
        match miniz_oxide::inflate::decompress_to_vec_with_limit(stored, raw_len as usize) {
            Ok(d) if d.len() as u64 == raw_len => d,
            _ => return Some((Read::Damaged { tag, why: "it doesn't decompress".into() }, end)),
        }
    } else {
        if len != raw_len {
            return Some((Read::Damaged { tag, why: "its lengths disagree".into() }, end));
        }
        stored.to_vec()
    };
    Some((Read::Ok(Chunk { tag, version, data }), end))
}

/// The file's format version, or why it isn't a .cdy file this reader can open.
pub fn header(b: &[u8]) -> Result<u16, String> {
    if !is_cdy(b) {
        return Err("not a CADaddy drawing (.cdy)".into());
    }
    let v = u16_at(b, 8).ok_or("the file is cut short")?;
    if v > FORMAT_VERSION {
        return Err(format!("made by a newer CADaddy (file format {v}; this one reads up to {FORMAT_VERSION})"));
    }
    Ok(v)
}

/// Every chunk of the file in order: through the index when trailer and index are sound, else by
/// walking the chunks from the header (a damaged or cut-short file). The second value says how.
pub fn chunks(b: &[u8]) -> Result<(Vec<Read>, bool), String> {
    header(b)?;
    if let Some(list) = by_index(b) {
        return Ok((list, true));
    }
    // Walk: chunk after chunk until the bytes run out or stop making sense.
    let mut at = HEADER_LEN;
    let mut out = Vec::new();
    while out.len() < MAX_CHUNKS {
        let Some((r, next)) = chunk_at(b, at) else { break };
        if matches!(&r, Read::Ok(c) if &c.tag == b"INDX") {
            break;
        }
        out.push(r);
        at = next;
    }
    Ok((out, false))
}

/// The first chunk tagged `tag`, decoding only that one when the index is sound (a preview or the
/// manifest without reading the drawing); `None` when there is none or it is damaged.
pub fn find(b: &[u8], tag: &[u8; 4]) -> Option<Chunk> {
    header(b).ok()?;
    let found = match index(b) {
        Some(entries) => {
            let e = entries.iter().find(|e| &e.tag == tag)?;
            chunk_at(b, usize::try_from(e.offset).ok()?)?.0
        }
        None => chunks(b).ok()?.0.into_iter().find(|r| matches!(r, Read::Ok(c) if &c.tag == tag))?,
    };
    match found {
        Read::Ok(c) if &c.tag == tag => Some(c),
        _ => None,
    }
}

/// The index entries, when trailer and index are sound.
fn index(b: &[u8]) -> Option<Vec<IndexEntry>> {
    let t = b.len().checked_sub(TRAILER_LEN)?;
    if b.get(t + 8..)? != TRAILER_MAGIC {
        return None;
    }
    let at = usize::try_from(u64_at(b, t)?).ok()?;
    let (Read::Ok(ix), _) = chunk_at(b, at)? else { return None };
    if &ix.tag != b"INDX" {
        return None;
    }
    let entries: Vec<IndexEntry> = postcard::from_bytes(&ix.data).ok()?;
    (entries.len() <= MAX_CHUNKS).then_some(entries)
}

fn by_index(b: &[u8]) -> Option<Vec<Read>> {
    let entries = index(b)?;
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let (r, _) = chunk_at(b, usize::try_from(e.offset).ok()?)?;
        out.push(r);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_roundtrip_through_the_index_and_by_walking() {
        let mut w = Writer::new();
        w.chunk(*b"AAAA", 1, b"hello hello hello hello hello hello", true);
        w.chunk(*b"BBBB", 3, &[0u8; 5], false);
        let bytes = w.finish();
        let (list, indexed) = chunks(&bytes).unwrap();
        assert!(indexed);
        let got: Vec<(String, u16, Vec<u8>)> = list
            .into_iter()
            .filter_map(|r| match r {
                Read::Ok(c) => Some((String::from_utf8_lossy(&c.tag).into_owned(), c.version, c.data)),
                Read::Damaged { .. } => None,
            })
            .collect();
        assert_eq!(got[0], ("AAAA".into(), 1, b"hello hello hello hello hello hello".to_vec()));
        assert_eq!(got[1], ("BBBB".into(), 3, vec![0; 5]));
        // Trailer gone: the chunks are found by walking.
        let cut = &bytes[..bytes.len() - 3];
        let (list, indexed) = chunks(cut).unwrap();
        assert!(!indexed);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn a_damaged_chunk_is_reported_and_the_rest_reads() {
        let mut w = Writer::new();
        w.chunk(*b"AAAA", 1, b"first", false);
        w.chunk(*b"BBBB", 1, b"second", false);
        let mut bytes = w.finish();
        // Flip a payload byte of the first chunk.
        bytes[HEADER_LEN + CHUNK_HEADER_LEN] ^= 0xFF;
        let (list, _) = chunks(&bytes).unwrap();
        assert!(matches!(&list[0], Read::Damaged { tag, .. } if tag == b"AAAA"));
        assert!(matches!(&list[1], Read::Ok(c) if c.data == b"second"));
    }

    #[test]
    fn newer_files_and_garbage_are_refused_cleanly() {
        let mut bytes = Writer::new().finish();
        bytes[8] = 9;
        assert!(header(&bytes).unwrap_err().contains("newer CADaddy"));
        assert!(chunks(b"not a drawing").is_err());
        assert!(chunks(&MAGIC).is_err(), "cut short");
        // Hostile lengths: claims of huge chunks never allocate them.
        let mut evil = Writer::new().finish();
        evil.truncate(HEADER_LEN);
        evil.extend_from_slice(b"EVIL");
        evil.extend_from_slice(&[1, 0, 1, 0]);
        evil.extend_from_slice(&u64::MAX.to_le_bytes());
        evil.extend_from_slice(&[0; 12]);
        let (list, _) = chunks(&evil).unwrap();
        assert!(list.is_empty());
    }
}
