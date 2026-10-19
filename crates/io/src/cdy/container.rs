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

/// Writes a .cdy file chunk by chunk, or the chunks appended to one (an incremental save).
pub struct Writer {
    out: Vec<u8>,
    index: Vec<IndexEntry>,
    /// Where `out` starts in the file: 0 for a new file, the old length when appending.
    base: u64,
}

impl Writer {
    pub fn new() -> Self {
        let mut out = Vec::with_capacity(1 << 16);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        Writer { out, index: Vec::new(), base: 0 }
    }

    /// Chunks to append to a file of `len` bytes; [`Writer::reuse`] lists the ones it already has.
    pub fn append(len: u64) -> Self {
        Writer { out: Vec::with_capacity(1 << 16), index: Vec::new(), base: len }
    }

    /// List a chunk the file already holds in the new index (unchanged since it was written).
    pub fn reuse(&mut self, e: &IndexEntry) {
        self.index.push(e.clone());
    }

    /// The index entries so far, in order.
    pub fn entries(&self) -> &[IndexEntry] {
        &self.index
    }

    /// Append a chunk; `deflate` compresses its payload (small or already compressed data, like a
    /// PNG, is stored as it is).
    pub fn chunk(&mut self, tag: [u8; 4], version: u16, data: &[u8], deflate: bool) {
        let packed = if deflate { Some(miniz_oxide::deflate::compress_to_vec(data, 3)) } else { None };
        let (flags, stored): (u16, &[u8]) = match &packed {
            Some(p) if p.len() < data.len() => (FLAG_DEFLATE, p),
            _ => (0, data),
        };
        let offset = self.base + self.out.len() as u64;
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
    pub fn finish(self) -> Vec<u8> {
        self.finish_split().0
    }

    /// [`Writer::finish`], with where in the bytes the index starts: a careful writer makes the
    /// chunks durable before the index and trailer that point at them.
    pub fn finish_split(mut self) -> (Vec<u8>, usize) {
        let index = postcard::to_allocvec(&self.index).unwrap_or_default();
        let split = self.out.len();
        let at = self.base + split as u64;
        self.chunk(*b"INDX", 1, &index, false);
        self.out.extend_from_slice(&at.to_le_bytes());
        self.out.extend_from_slice(&TRAILER_MAGIC);
        (self.out, split)
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
        /// What could still be read of its data (decompressed up to where that failed), for the
        /// reader to salvage whole objects from; empty when nothing could.
        salvage: Vec<u8>,
    },
}

/// The chunk at `at`, and where the next one starts.
pub(super) fn chunk_at(b: &[u8], at: usize) -> Option<(Read, usize)> {
    let tag: [u8; 4] = b.get(at..at.checked_add(4)?)?.try_into().ok()?;
    let version = u16_at(b, at + 4)?;
    let flags = u16_at(b, at + 6)?;
    let len = u64_at(b, at + 8)?;
    let crc = u32_at(b, at + 16)?;
    let raw_len = u64_at(b, at + 20)?;
    let start = at.checked_add(CHUNK_HEADER_LEN)?;
    let end = start.checked_add(usize::try_from(len).ok()?)?;
    let stored = b.get(start..end)?;
    if raw_len > MAX_RAW {
        return Some((Read::Damaged { tag, why: "it is larger than a chunk may be".into(), salvage: Vec::new() }, end));
    }
    if crc32fast::hash(stored) != crc {
        // Deflate decodes in order: what comes before the damage is still good.
        let salvage = if flags & FLAG_DEFLATE != 0 {
            match miniz_oxide::inflate::decompress_to_vec_with_limit(stored, raw_len as usize) {
                Ok(d) => d,
                Err(e) => e.output,
            }
        } else {
            stored.to_vec()
        };
        return Some((Read::Damaged { tag, why: "its checksum doesn't match".into(), salvage }, end));
    }
    let data = if flags & FLAG_DEFLATE != 0 {
        match miniz_oxide::inflate::decompress_to_vec_with_limit(stored, raw_len as usize) {
            Ok(d) if d.len() as u64 == raw_len => d,
            Ok(d) => return Some((Read::Damaged { tag, why: "it doesn't decompress".into(), salvage: d }, end)),
            Err(e) => return Some((Read::Damaged { tag, why: "it doesn't decompress".into(), salvage: e.output }, end)),
        }
    } else {
        if len != raw_len {
            return Some((Read::Damaged { tag, why: "its lengths disagree".into(), salvage: Vec::new() }, end));
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
///
/// A file saved incrementally holds several saves, each ending in its index and trailer. Walking
/// finds the last index whose chunks are all there (a damaged one is then reported) and reads
/// through it: a save cut short (the app or machine stopped while appending) leaves the file as
/// the save before it.
pub fn chunks(b: &[u8]) -> Result<(Vec<Read>, bool), String> {
    header(b)?;
    if let Some(list) = by_index(b) {
        return Ok((list, true));
    }
    let mut at = HEADER_LEN;
    let mut out = Vec::new();
    let mut last_index: Option<Vec<IndexEntry>> = None;
    let mut steps = 0usize;
    while steps < MAX_CHUNKS {
        steps += 1;
        // An earlier save's trailer: skip it.
        if b.get(at + 8..at + TRAILER_LEN) == Some(&TRAILER_MAGIC[..]) {
            at += TRAILER_LEN;
            continue;
        }
        let Some((r, next)) = chunk_at(b, at) else {
            // A header that doesn't read: look further on for the next chunk that does (whole and
            // checked), instead of losing every chunk after this one.
            match resync(b, at + 1) {
                Some(n) => {
                    at = n;
                    continue;
                }
                None => break,
            }
        };
        match r {
            Read::Ok(c) if &c.tag == b"INDX" => {
                if let Ok(entries) = postcard::from_bytes::<Vec<IndexEntry>>(&c.data)
                    && entries.len() <= MAX_CHUNKS
                    && entries.iter().all(|e| usize::try_from(e.offset).ok().and_then(|o| chunk_at(b, o)).is_some())
                {
                    last_index = Some(entries);
                }
            }
            r => out.push(r),
        }
        at = next;
    }
    if let Some(entries) = last_index {
        let list = entries.iter().filter_map(|e| Some(chunk_at(b, usize::try_from(e.offset).ok()?)?.0)).collect();
        return Ok((list, false));
    }
    Ok((out, false))
}

/// A chunk as found walking a file from its header, for checking it (`verify`).
pub struct Walked {
    pub offset: u64,
    pub tag: [u8; 4],
    pub version: u16,
    /// Stored length (compressed when deflated) and the payload's own length.
    pub stored: u64,
    pub raw: u64,
    pub deflated: bool,
    /// Why it is damaged; `None` when its checksum and compression are sound.
    pub damage: Option<String>,
}

/// Every chunk of the file walked from its header (each save's chunks, the superseded ones too),
/// and the spans of bytes where no chunk could be read.
pub(super) fn walk(b: &[u8]) -> (Vec<Walked>, Vec<(u64, u64)>) {
    let (mut out, mut lost) = (Vec::new(), Vec::new());
    let mut at = HEADER_LEN;
    let mut steps = 0usize;
    while at < b.len() && steps < MAX_CHUNKS {
        steps += 1;
        if b.get(at + 8..at + TRAILER_LEN) == Some(&TRAILER_MAGIC[..]) {
            at += TRAILER_LEN;
            continue;
        }
        let Some((r, next)) = chunk_at(b, at) else {
            let n = resync(b, at + 1).unwrap_or(b.len());
            lost.push((at as u64, n as u64));
            at = n;
            continue;
        };
        let (version, flags, stored, raw) = (u16_at(b, at + 4), u16_at(b, at + 6), u64_at(b, at + 8), u64_at(b, at + 20));
        let (tag, damage) = match r {
            Read::Ok(c) => (c.tag, None),
            Read::Damaged { tag, why, .. } => (tag, Some(why)),
        };
        out.push(Walked {
            offset: at as u64,
            tag,
            version: version.unwrap_or(0),
            stored: stored.unwrap_or(0),
            raw: raw.unwrap_or(0),
            deflated: flags.is_some_and(|f| f & FLAG_DEFLATE != 0),
            damage,
        });
        at = next;
    }
    (out, lost)
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
pub(super) fn index(b: &[u8]) -> Option<Vec<IndexEntry>> {
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

/// Where the next readable chunk starts at or after `from`: a header with a tag of letters whose
/// stored bytes are all there and pass their checksum.
fn resync(b: &[u8], from: usize) -> Option<usize> {
    (from..b.len().saturating_sub(CHUNK_HEADER_LEN))
        .find(|&at| b.get(at..at + 4).is_some_and(|t| t.iter().all(u8::is_ascii_alphanumeric)) && matches!(chunk_at(b, at), Some((Read::Ok(_), _))))
}

fn by_index(b: &[u8]) -> Option<Vec<Read>> {
    let entries = index(b)?;
    // Checksums and inflating, chunk by chunk on all cores (most of opening a large file).
    Some(crate::par_map(&entries, |e| {
        // A chunk whose header disagrees with the index (or can't be read) was damaged there:
        // the index still says what and where it is, so it is salvaged as that. One bad header
        // used to drop the index and, walking, every chunk after it (UI review 0.7.7d).
        match usize::try_from(e.offset).ok().and_then(|o| chunk_at(b, o)) {
            Some((Read::Ok(c), _)) if c.tag == e.tag => Read::Ok(c),
            Some((r @ Read::Damaged { tag, .. }, _)) if tag == e.tag => r,
            _ => from_entry(b, e),
        }
    }))
}

/// A chunk read by its index entry alone (its own header is damaged): damaged, with what its
/// stored bytes still give for salvage.
fn from_entry(b: &[u8], e: &IndexEntry) -> Read {
    let stored = usize::try_from(e.offset)
        .ok()
        .and_then(|o| o.checked_add(CHUNK_HEADER_LEN))
        .and_then(|start| Some((start, start.checked_add(usize::try_from(e.len).ok()?)?)))
        .and_then(|(start, end)| b.get(start..end))
        .unwrap_or_default();
    let raw = usize::try_from(e.raw_len.min(MAX_RAW)).unwrap_or(0);
    // Deflated or not isn't known (the flags are in the header): inflating tells.
    let salvage = match miniz_oxide::inflate::decompress_to_vec_with_limit(stored, raw) {
        Ok(d) => d,
        Err(err) if !err.output.is_empty() => err.output,
        Err(_) => stored.to_vec(),
    };
    Read::Damaged { tag: e.tag, why: "its header is damaged".into(), salvage }
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
