//! Salvaging damaged DWG files (AutoCAD 2004 and later: AC1018, AC1024, AC1027, AC1032), from the
//! Open Design Alliance's public specification of the format.
//!
//! These files are a container of pages: data pages hold the sections (header variables, classes,
//! objects, the object map…), and two system pages at the end — the section map and the page
//! map — say which page is which. A file cut short loses those maps, so the whole file reads as
//! nothing, and a damaged page used to take its whole section with it.
//!
//! [`salvage`] reads what is left page by page instead: it finds the pages by their headers,
//! decompresses each as far as it is sound, names the sections from the section map or, when that
//! is gone, from what they hold, and rebuilds the object map by walking the object records (each
//! has its own checksum, so a damaged one is left out rather than misread). It then writes a new,
//! sound container of what it recovered, for the DWG reader to read like any other file. Nothing
//! here trusts a number from the file: sizes are capped, reads are checked, and the work spent on
//! damaged parts is bounded.

use std::collections::{BTreeMap, BTreeSet};

/// Page types (the first word of a page header).
const DATA_PAGE: u32 = 0x4163_043b;
const SECTION_MAP: u32 = 0x4163_003b;
const PAGE_MAP: u32 = 0x4163_0e3b;
/// Data page headers are encrypted with this word, xor the page's file offset.
const PAGE_MASK: u32 = 0x4164_536b;
/// Pages start here, after the file header.
const FIRST_PAGE: usize = 0x100;
/// The encrypted file header: where and how long.
const HEADER_AT: usize = 0x80;
const HEADER_LEN: usize = 0x6c;
/// Most a page decompresses to, and the page size compressed sections use.
const PAGE_DATA: usize = 0x7400;
/// Largest compressed page we accept (bigger is damage).
const MAX_PAGE: usize = 0x2_0000;
/// Largest object record we accept while walking the objects.
const MAX_RECORD: usize = 1 << 20;
/// Bytes of checksumming spent looking for object records in damaged data, at most.
const SCAN_BUDGET: usize = 256 << 20;
/// Sections a file may name, at most.
const MAX_SECTIONS: usize = 64;

/// The sentinels that open some sections (public specification).
const HEADER_SENTINEL: [u8; 16] = [0xcf, 0x7b, 0x1f, 0x23, 0xfd, 0xde, 0x38, 0xa9, 0x5f, 0x7c, 0x68, 0xb8, 0x4e, 0x6d, 0x33, 0x5f];
const CLASSES_SENTINEL: [u8; 16] = [0x8d, 0xa1, 0xc4, 0xb8, 0xc4, 0xa9, 0xf8, 0xc5, 0xc0, 0xdc, 0xf4, 0x5f, 0xe7, 0xcf, 0xb6, 0x8a];
const PREVIEW_SENTINEL: [u8; 16] = [0x1f, 0x25, 0x6d, 0x07, 0xd4, 0x36, 0x28, 0x28, 0x9d, 0x57, 0xca, 0x3f, 0x9d, 0x44, 0x10, 0x2b];

/// What [`salvage`] recovered.
#[derive(Debug, Clone)]
pub struct Salvaged {
    /// A sound DWG file of what could be read.
    pub bytes: Vec<u8>,
    /// What was damaged and what was rebuilt, for the drawing's read notes.
    pub notes: Vec<String>,
    /// Object records recovered.
    pub objects: usize,
}

/// Whether a file [`salvage`] knows has damage its pages show: a page whose checksum fails, or one
/// cut short. (A reader that doesn't check them reads damaged objects as if they were sound.)
pub fn damaged(bytes: &[u8]) -> bool {
    can_salvage(bytes) && find_pages(bytes).0.iter().any(|p| !p.sound)
}

/// Whether [`salvage`] knows the file's version.
pub fn can_salvage(bytes: &[u8]) -> bool {
    matches!(bytes.get(..6), Some(b"AC1018" | b"AC1024" | b"AC1027" | b"AC1032"))
}

// ---------------- small readers ----------------

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    let s = b.get(o..o.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from(u32_at(b, o)?) | (u64::from(u32_at(b, o + 4)?) << 32))
}

fn u16_be(b: &[u8], o: usize) -> Option<u16> {
    let s = b.get(o..o.checked_add(2)?)?;
    Some(u16::from_be_bytes([s[0], s[1]]))
}

fn u16_le(b: &[u8], o: usize) -> Option<u16> {
    let s = b.get(o..o.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

/// The file header's encryption: a byte stream from the specification's generator, xored in (the
/// same call decrypts and encrypts).
fn crypt_header(h: &mut [u8]) {
    let mut seed: u32 = 1;
    for b in h.iter_mut() {
        seed = seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        *b ^= (seed >> 16) as u8;
    }
}

/// The page checksum of the specification (an Adler-32 variant with a seed).
fn checksum(seed: u32, data: &[u8]) -> u32 {
    let (mut s1, mut s2) = (seed & 0xffff, seed >> 16);
    for chunk in data.chunks(0x15b0) {
        for &b in chunk {
            s1 += u32::from(b);
            s2 += s1;
        }
        s1 %= 0xfff1;
        s2 %= 0xfff1;
    }
    (s2 << 16) | (s1 & 0xffff)
}

const fn crc16_table() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u16;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xa001 } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}
static CRC16: [u16; 256] = crc16_table();

/// The 16-bit CRC object records and the object map carry (seed 0xC0C1).
fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0xc0c1u16, |crc, &b| (crc >> 8) ^ CRC16[usize::from((crc as u8) ^ b)])
}

const fn crc32_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xedb8_8320 } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}
static CRC32: [u32; 256] = crc32_table();

/// The file header's CRC-32 (the common one).
fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, &b| (crc >> 8) ^ CRC32[((crc as u8) ^ b) as usize])
}

// ---------------- compression ----------------

/// A literal run's length after `op`.
fn literal_length(op: u8, src: &[u8], p: &mut usize) -> Option<usize> {
    let mut n = usize::from(op & 0x0f);
    if n == 0 {
        n = 0x0f;
        loop {
            let b = *src.get(*p)?;
            *p += 1;
            if b != 0 {
                n += usize::from(b);
                break;
            }
            n += 0xff;
            if n > MAX_PAGE {
                return None;
            }
        }
    }
    Some(n + 3)
}

/// A long match length (opcodes 0x10 and 0x20).
fn long_length(src: &[u8], p: &mut usize) -> Option<usize> {
    let mut n = 0usize;
    loop {
        let b = *src.get(*p)?;
        *p += 1;
        if b != 0 {
            return Some(n + usize::from(b));
        }
        n += 0xff;
        if n > MAX_PAGE {
            return None;
        }
    }
}

/// Decompress a page (the specification's LZ77 variant) into at most `max` bytes. Returns what was
/// decoded and whether the whole stream was sound: decoding runs in order, so what comes before
/// damage is still good.
fn decompress(src: &[u8], max: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(max.min(PAGE_DATA));
    let ok = decompress_into(src, max, &mut out).is_some();
    (out, ok)
}

fn decompress_into(src: &[u8], max: usize, out: &mut Vec<u8>) -> Option<()> {
    let mut p = 0usize;
    let literals = |out: &mut Vec<u8>, p: &mut usize, n: usize| -> Option<()> {
        let s = src.get(*p..p.checked_add(n)?)?;
        if out.len() + n > max {
            return None;
        }
        out.extend_from_slice(s);
        *p += n;
        Some(())
    };
    let mut op = *src.get(p)?;
    p += 1;
    if op & 0xf0 == 0 {
        let n = literal_length(op, src, &mut p)?;
        literals(out, &mut p, n)?;
        op = *src.get(p)?;
        p += 1;
    }
    loop {
        let (len, offset, lits) = match op {
            0x11 => return Some(()),
            0x10 | 0x12..=0x1f => {
                // Low bits 0 (0x10, 0x18): the length follows.
                let len = if op & 7 == 0 { long_length(src, &mut p)? + 9 } else { usize::from(op & 7) + 2 };
                let (b1, b2) = (*src.get(p)?, *src.get(p + 1)?);
                p += 2;
                let off = (usize::from(b1) >> 2 | usize::from(b2) << 6) + 0x4000 + (usize::from(op & 8) << 11);
                (len, off, b1 & 3)
            }
            0x20..=0x3f => {
                let len = if op == 0x20 { long_length(src, &mut p)? + 0x21 } else { usize::from(op) - 0x1e };
                let (b1, b2) = (*src.get(p)?, *src.get(p + 1)?);
                p += 2;
                (len, (usize::from(b1) >> 2 | usize::from(b2) << 6) + 1, b1 & 3)
            }
            0x40..=0xff => {
                let b2 = *src.get(p)?;
                p += 1;
                (usize::from(op >> 4) - 1, (usize::from(b2) << 2 | usize::from(op & 0x0c) >> 2) + 1, op & 3)
            }
            _ => return None,
        };
        let start = out.len().checked_sub(offset)?;
        if out.len() + len > max {
            return None;
        }
        for k in 0..len {
            let b = *out.get(start + k)?;
            out.push(b);
        }
        if lits == 0 {
            op = *src.get(p)?;
            p += 1;
            if op & 0xf0 == 0 {
                let n = literal_length(op, src, &mut p)?;
                literals(out, &mut p, n)?;
                op = *src.get(p)?;
                p += 1;
            }
        } else {
            literals(out, &mut p, usize::from(lits))?;
            op = *src.get(p)?;
            p += 1;
        }
    }
}

/// `data` as a compressed stream of one literal run (sound for any reader of the format; `data`
/// is at least 4 bytes).
fn literal_stream(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let m = data.len().saturating_sub(3);
    if (1..=0x0f).contains(&m) {
        out.push(m as u8);
    } else {
        out.push(0);
        let mut m = m - 0x0f;
        while m > 0xff {
            out.push(0);
            m -= 0xff;
        }
        out.push(m as u8);
    }
    out.extend_from_slice(data);
    out.extend_from_slice(&[0x11, 0, 0]);
    out
}

// ---------------- object records ----------------

/// A modular short (15 bits per little-endian word, the top bit continues).
fn read_ms(b: &[u8], p: &mut usize) -> Option<usize> {
    let mut v = 0usize;
    for shift in [0, 15, 30] {
        let w = u16_le(b, *p)?;
        *p += 2;
        v |= usize::from(w & 0x7fff) << shift;
        if w & 0x8000 == 0 {
            return Some(v);
        }
    }
    None
}

/// A modular char (7 bits per byte, the top bit continues); `signed`: the last byte's bit 6 is
/// the sign.
fn read_mc(b: &[u8], p: &mut usize, signed: bool) -> Option<i64> {
    let mut v = 0i64;
    for shift in (0..64).step_by(7) {
        let c = *b.get(*p)?;
        *p += 1;
        if c & 0x80 != 0 {
            v |= i64::from(c & 0x7f) << shift;
            continue;
        }
        return Some(if signed {
            let mag = v | (i64::from(c & 0x3f) << shift);
            if c & 0x40 != 0 { -mag } else { mag }
        } else {
            v | (i64::from(c & 0x7f) << shift)
        });
    }
    None
}

fn write_umc(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let c = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(c);
            return;
        }
        out.push(c | 0x80);
    }
}

fn write_smc(out: &mut Vec<u8>, v: i64) {
    let neg = v < 0;
    let mut m = v.unsigned_abs();
    loop {
        if m > 0x3f {
            out.push((m & 0x7f) as u8 | 0x80);
            m >>= 7;
        } else {
            out.push(m as u8 | if neg { 0x40 } else { 0 });
            return;
        }
    }
}

/// A reader of bits, most significant first.
struct Bits<'a> {
    b: &'a [u8],
    bit: usize,
}

impl Bits<'_> {
    fn take(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.b.get(self.bit >> 3)?;
            v = (v << 1) | u32::from((byte >> (7 - (self.bit & 7))) & 1);
            self.bit += 1;
        }
        Some(v)
    }
    fn byte(&mut self) -> Option<u32> {
        self.take(8)
    }
    fn short(&mut self) -> Option<u32> {
        Some(self.byte()? | self.byte()? << 8)
    }
    /// A bit short.
    fn bs(&mut self) -> Option<u32> {
        match self.take(2)? {
            0 => self.short(),
            1 => self.byte(),
            2 => Some(0),
            _ => Some(256),
        }
    }
    /// An object type (2010 and later).
    fn ot(&mut self) -> Option<u32> {
        match self.take(2)? {
            0 => self.byte(),
            1 => Some(self.byte()? + 0x1f0),
            _ => self.short(),
        }
    }
    /// A handle's value.
    fn handle(&mut self) -> Option<u64> {
        let _code = self.take(4)?;
        let count = self.take(4)?;
        let mut v = 0u64;
        for _ in 0..count {
            v = (v << 8) | u64::from(self.byte()?);
        }
        Some(v)
    }
}

/// The object record at `p` in the objects section: its handle and where the next one starts,
/// when its checksum is sound.
fn record_at(objs: &[u8], p: usize, r2010: bool, budget: &mut usize) -> Option<(u64, usize)> {
    let mut q = p;
    let size = read_ms(objs, &mut q)?;
    if size == 0 || size > MAX_RECORD {
        return None;
    }
    let data = if r2010 {
        let mut q2 = q;
        read_mc(objs, &mut q2, false)?;
        q2
    } else {
        q
    };
    let end = data.checked_add(size)?;
    let stored = u16_le(objs, end)?;
    *budget = budget.checked_sub(end - p)?;
    if crc16(objs.get(p..end)?) != stored {
        return None;
    }
    let mut bits = Bits { b: objs.get(data..end)?, bit: 0 };
    if r2010 {
        bits.ot()?;
    } else {
        bits.bs()?;
        bits.take(32)?;
    }
    Some((bits.handle()?, end + 2))
}

/// Walk the objects section's records (from offset 4, after its 0x0DCA word), stepping over
/// damage byte by byte until the next sound record: (handle, offset) of each.
fn walk_records(objs: &[u8], r2010: bool) -> Vec<(u64, usize)> {
    let mut out = Vec::new();
    let mut budget = SCAN_BUDGET;
    let mut p = 4;
    while p + 4 < objs.len() && budget > 0 {
        match record_at(objs, p, r2010, &mut budget) {
            Some((h, next)) => {
                if h != 0 {
                    out.push((h, p));
                }
                p = next;
            }
            None => p += 1,
        }
    }
    out
}

/// The entries of an object map (the Handles section): (handle, offset) pairs, as far as its
/// chunks are sound.
fn read_object_map(map: &[u8]) -> Vec<(u64, usize)> {
    let mut out = Vec::new();
    let mut p = 0;
    while let Some(size) = u16_be(map, p).map(usize::from) {
        if size <= 2 {
            break;
        }
        let Some(chunk) = map.get(p..p + size) else { break };
        if u16_be(map, p + size) != Some(crc16(chunk)) {
            break;
        }
        let (mut q, mut h, mut off) = (2usize, 0i64, 0i64);
        while q < size {
            let (Some(dh), Some(doff)) = (read_mc(chunk, &mut q, false), read_mc(chunk, &mut q, true)) else { break };
            h = h.saturating_add(dh);
            off = off.saturating_add(doff);
            if let (Ok(h), Ok(o)) = (u64::try_from(h), usize::try_from(off)) {
                out.push((h, o));
            }
        }
        p += size + 2;
    }
    out
}

/// An object map of `entries` (sorted by handle).
fn write_object_map(entries: &[(u64, usize)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let mut chunk = Vec::new();
        let (mut ph, mut po) = (0u64, 0i64);
        while let Some(&(h, o)) = entries.get(i) {
            if chunk.len() > 2032 - 16 {
                break;
            }
            let o = i64::try_from(o).unwrap_or(i64::MAX);
            write_umc(&mut chunk, h - ph);
            write_smc(&mut chunk, o - po);
            (ph, po) = (h, o);
            i += 1;
        }
        let mut c = ((chunk.len() + 2) as u16).to_be_bytes().to_vec();
        c.extend_from_slice(&chunk);
        let crc = crc16(&c);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    let end = 2u16.to_be_bytes();
    out.extend_from_slice(&end);
    out.extend_from_slice(&crc16(&end).to_be_bytes());
    out
}

// ---------------- pages and sections ----------------

/// A data page found in the file.
struct Page<'a> {
    section: u32,
    /// Where its data goes in the section.
    start: u64,
    /// Its compressed bytes (cut short when the file was).
    data: &'a [u8],
    /// Its data checksum held.
    sound: bool,
}

/// A section as the section map describes it.
#[derive(Clone, Debug)]
struct Section {
    name: String,
    size: u64,
    /// Compressed (2) or stored (1).
    compressed: bool,
    /// Bytes per page, decompressed.
    page_data: usize,
    encrypted: u32,
}

/// Find the pages: data pages by their (encrypted) headers, the section map by its type. Damage
/// between pages is stepped over.
fn find_pages(b: &[u8]) -> (Vec<Page<'_>>, Vec<&[u8]>) {
    let (mut pages, mut maps) = (Vec::new(), Vec::new());
    let mut pos = FIRST_PAGE;
    while pos + 20 <= b.len() {
        let t = u32_at(b, pos).unwrap_or(0);
        if t == SECTION_MAP || t == PAGE_MAP {
            let csz = u32_at(b, pos + 8).map(|v| v as usize).unwrap_or(usize::MAX);
            if csz <= MAX_PAGE {
                if t == SECTION_MAP
                    && let Some(data) = b.get(pos + 20..(pos + 20 + csz).min(b.len()))
                {
                    maps.push(data);
                }
                pos = (pos + 20 + csz + 0x1f) & !0x1f;
                continue;
            }
        }
        let mask = PAGE_MASK ^ (pos as u32);
        let w = |i: usize| u32_at(b, pos + 4 * i).map(|v| v ^ mask);
        if w(0) == Some(DATA_PAGE)
            && let (Some(section), Some(csz), Some(lo), Some(hi), Some(dchk)) = (w(1), w(2), w(4), w(5), w(7))
            && (csz as usize) <= MAX_PAGE
        {
            let csz = csz as usize;
            let data = b.get(pos + 32..(pos + 32 + csz).min(b.len())).unwrap_or_default();
            let sound = data.len() == csz && checksum(0, data) == dchk;
            pages.push(Page { section, start: u64::from(lo) | (u64::from(hi) << 32), data, sound });
            pos = (pos + 32 + csz + 0x1f) & !0x1f;
            continue;
        }
        pos += 4;
    }
    (pages, maps)
}

/// The sections a readable section map names, by id.
fn read_section_map(map: &[u8]) -> Option<BTreeMap<u32, Section>> {
    let (data, ok) = decompress(map, 1 << 20);
    if !ok {
        return None;
    }
    let n = u32_at(&data, 0)? as usize;
    if n > MAX_SECTIONS {
        return None;
    }
    let mut out = BTreeMap::new();
    let mut o = 20usize;
    for _ in 0..n {
        let size = u64_at(&data, o)?;
        let pages = u32_at(&data, o + 8)? as usize;
        let page_data = u32_at(&data, o + 12)? as usize;
        let compressed = u32_at(&data, o + 20)? == 2;
        let id = u32_at(&data, o + 24)?;
        let encrypted = u32_at(&data, o + 28)?;
        let name = data.get(o + 32..o + 96)?;
        let name = String::from_utf8_lossy(name.split(|&c| c == 0).next().unwrap_or_default()).into_owned();
        o = o.checked_add(96)?.checked_add(pages.checked_mul(16)?)?;
        if page_data == 0 || page_data > MAX_PAGE {
            return None;
        }
        out.insert(id, Section { name, size, compressed, page_data, encrypted });
    }
    Some(out)
}

/// A section's name from what it holds, when the section map is gone.
fn name_from_content(d: &[u8]) -> Option<&'static str> {
    if d.starts_with(&HEADER_SENTINEL) {
        return Some("AcDb:Header");
    }
    if d.starts_with(&CLASSES_SENTINEL) {
        return Some("AcDb:Classes");
    }
    if d.starts_with(&PREVIEW_SENTINEL) {
        return Some("AcDb:Preview");
    }
    if d.starts_with(&[0xca, 0x0d, 0, 0]) {
        return Some("AcDb:AcDbObjects");
    }
    if d.starts_with(&[0xff, 0x77, 0x01]) {
        return Some("AcDb:AuxHeader");
    }
    // The object map: a first chunk whose CRC holds.
    let size = usize::from(u16_be(d, 0)?);
    if size > 2 && size <= 2040 && u16_be(d, size) == Some(crc16(d.get(..size)?)) {
        return Some("AcDb:Handles");
    }
    None
}

/// Salvage a damaged DWG file (2004 and later). `None` when it isn't such a file, or nothing of
/// its objects could be found.
pub fn salvage(b: &[u8]) -> Option<Salvaged> {
    if !can_salvage(b) || b.len() < FIRST_PAGE {
        return None;
    }
    let r2010 = b.get(..6).is_some_and(|v| v >= b"AC1024".as_slice());
    let mut notes = Vec::new();
    let (pages, maps) = find_pages(b);
    let damaged_pages = pages.iter().filter(|p| !p.sound).count();
    // The sections: from the section map, or named from their first page.
    let mut sections = maps.iter().find_map(|m| read_section_map(m)).unwrap_or_default();
    if sections.is_empty() {
        notes.push("its section map was lost and was rebuilt from the pages that remain".to_string());
        let ids: BTreeSet<u32> = pages.iter().map(|p| p.section).collect();
        for id in ids.into_iter().take(MAX_SECTIONS) {
            let Some(first) = pages.iter().filter(|p| p.section == id).min_by_key(|p| (p.start, !p.sound)) else { continue };
            let (d, _) = decompress(first.data, PAGE_DATA);
            let (name, compressed, page_data) = match name_from_content(&d) {
                Some(n) => (n, true, PAGE_DATA),
                // A section stored without compression.
                None => match name_from_content(first.data) {
                    Some(n) => (n, false, first.data.len().max(1)),
                    None => continue,
                },
            };
            sections.insert(id, Section { name: name.to_string(), size: 0, compressed, page_data, encrypted: 0 });
        }
    }
    // Each section's bytes: its pages decompressed in place (as far as each is sound); what
    // can't be read stays zero.
    let mut content: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    for (id, s) in sections.iter_mut() {
        let mut buf: Vec<u8> = Vec::new();
        let mut known = s.size;
        for p in pages.iter().filter(|p| p.section == *id) {
            let d = if s.compressed { decompress(p.data, s.page_data).0 } else { p.data.to_vec() };
            let Ok(start) = usize::try_from(p.start) else { continue };
            let end = start.saturating_add(d.len());
            if end > 1 << 31 {
                continue;
            }
            if buf.len() < end {
                buf.resize(end, 0);
            }
            if let Some(slot) = buf.get_mut(start..end) {
                slot.copy_from_slice(&d);
            }
        }
        if known == 0 || known > buf.len() as u64 {
            known = known.max(buf.len() as u64);
        }
        buf.resize(usize::try_from(known).unwrap_or(buf.len()).min(1 << 31), 0);
        s.size = buf.len() as u64;
        content.insert(*id, buf);
    }
    let id_of = |name: &str, sections: &BTreeMap<u32, Section>| sections.iter().find(|(_, s)| s.name == name).map(|(id, _)| *id);
    let objects_id = id_of("AcDb:AcDbObjects", &sections)?;
    let objs = content.get(&objects_id)?;
    // The object map: the file's own where its entries point at sound records, and every sound
    // record found by walking the objects.
    let walked = walk_records(objs, r2010);
    let mut entries: BTreeMap<u64, usize> = walked.iter().copied().collect();
    let mut kept_old = 0usize;
    if let Some(map) = id_of("AcDb:Handles", &sections).and_then(|id| content.get(&id)) {
        let mut budget = SCAN_BUDGET;
        for (h, o) in read_object_map(map) {
            if !entries.contains_key(&h) && record_at(objs, o, r2010, &mut budget).is_some() {
                entries.insert(h, o);
                kept_old += 1;
            }
        }
    }
    if entries.is_empty() {
        return None;
    }
    let entries: Vec<(u64, usize)> = entries.into_iter().collect();
    let objects = entries.len();
    let handles_id = match id_of("AcDb:Handles", &sections) {
        Some(id) => id,
        None => {
            let id = sections.keys().next_back().map_or(1, |k| k + 1);
            sections.insert(id, Section { name: "AcDb:Handles".into(), size: 0, compressed: true, page_data: PAGE_DATA, encrypted: 0 });
            id
        }
    };
    let map = write_object_map(&entries);
    if let Some(s) = sections.get_mut(&handles_id) {
        s.size = map.len() as u64;
    }
    content.insert(handles_id, map);
    if damaged_pages > 0 {
        notes.push(format!("{damaged_pages} of its {} pages were damaged or cut short and were read as far as they were sound", pages.len()));
    }
    notes.push(format!("its object map was rebuilt: {objects} objects were found ({kept_old} of them through what was left of the old one)"));
    let bytes = write_file(b, &sections, &content)?;
    Some(Salvaged { bytes, notes, objects })
}

// ---------------- writing the repaired file ----------------

/// A system page (section map, page map): its header and one literal run.
fn system_page(kind: u32, data: &[u8]) -> Vec<u8> {
    let mut padded = data.to_vec();
    padded.resize(padded.len().max(4), 0);
    let comp = literal_stream(&padded);
    let mut head = Vec::with_capacity(20);
    for v in [kind, padded.len() as u32, comp.len() as u32, 2, 0] {
        head.extend_from_slice(&v.to_le_bytes());
    }
    let chk = checksum(checksum(0, &head), &comp);
    head.truncate(16);
    head.extend_from_slice(&chk.to_le_bytes());
    head.extend_from_slice(&comp);
    let total = (head.len() + 0x1f) & !0x1f;
    head.resize(total, 0);
    head
}

/// A new file: the original's first page header area, the sections' pages, the section map, the
/// page map and the encrypted header pointing at them.
fn write_file(orig: &[u8], sections: &BTreeMap<u32, Section>, content: &BTreeMap<u32, Vec<u8>>) -> Option<Vec<u8>> {
    let mut out = orig.get(..FIRST_PAGE)?.to_vec();
    let mut page_sizes: Vec<(u32, u32)> = Vec::new();
    let mut number = 0u32;
    let mut descriptions = Vec::new();
    for (id, s) in sections {
        let data = content.get(id).map(Vec::as_slice).unwrap_or_default();
        let mut list = Vec::new();
        let step = s.page_data.max(1);
        let mut start = 0usize;
        while start < data.len() {
            // A compressed page holds at least 4 bytes: the last one takes from the one before.
            let mut end = (start + step).min(data.len());
            if s.compressed && data.len() - end > 0 && data.len() - end < 4 {
                end = data.len() - 4;
            }
            let chunk = data.get(start..end)?;
            let comp = if s.compressed {
                let mut c = chunk.to_vec();
                c.resize(c.len().max(4), 0);
                literal_stream(&c)
            } else {
                chunk.to_vec()
            };
            number += 1;
            let pos = out.len();
            let total = (32 + comp.len() + 0x1f) & !0x1f;
            let dchk = checksum(0, &comp);
            let mut words = [DATA_PAGE, *id, comp.len() as u32, total as u32, start as u32, (start as u64 >> 32) as u32, 0, dchk];
            let head: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            words[6] = checksum(dchk, &head);
            let mask = PAGE_MASK ^ (pos as u32);
            out.extend(words.iter().flat_map(|w| (w ^ mask).to_le_bytes()));
            out.extend_from_slice(&comp);
            out.resize(pos + total, 0);
            page_sizes.push((number, total as u32));
            list.push((number, comp.len() as u32, start as u64));
            start = end;
        }
        descriptions.push((*id, s, list));
    }
    // The section map.
    let mut sm = Vec::new();
    for v in [descriptions.len() as u32, 2, PAGE_DATA as u32, 0, descriptions.len() as u32] {
        sm.extend_from_slice(&v.to_le_bytes());
    }
    for (id, s, list) in &descriptions {
        sm.extend_from_slice(&s.size.to_le_bytes());
        for v in [list.len() as u32, s.page_data as u32, 1, if s.compressed { 2 } else { 1 }, *id, s.encrypted] {
            sm.extend_from_slice(&v.to_le_bytes());
        }
        let mut name = s.name.as_bytes().to_vec();
        name.resize(64, 0);
        sm.extend_from_slice(&name);
        for (n, size, start) in list {
            sm.extend_from_slice(&n.to_le_bytes());
            sm.extend_from_slice(&size.to_le_bytes());
            sm.extend_from_slice(&start.to_le_bytes());
        }
    }
    number += 1;
    let section_map_id = number;
    let smp = system_page(SECTION_MAP, &sm);
    out.extend_from_slice(&smp);
    page_sizes.push((number, smp.len() as u32));
    // The page map lists itself too: its size settles in a round or two.
    number += 1;
    let page_map_id = number;
    let mut own = 0u32;
    let mut pmp = Vec::new();
    for _ in 0..4 {
        let mut pm = Vec::new();
        for (n, size) in page_sizes.iter().chain(std::iter::once(&(page_map_id, own))) {
            pm.extend_from_slice(&n.to_le_bytes());
            pm.extend_from_slice(&size.to_le_bytes());
        }
        pmp = system_page(PAGE_MAP, &pm);
        if pmp.len() as u32 == own {
            break;
        }
        own = pmp.len() as u32;
    }
    let page_map_at = out.len();
    out.extend_from_slice(&pmp);
    let end = out.len();
    // The header: the original's, pointing at the new maps.
    let mut h = orig.get(HEADER_AT..HEADER_AT + HEADER_LEN)?.to_vec();
    crypt_header(&mut h);
    if !h.starts_with(b"AcFssFcAJMB") {
        // The header itself is damaged: start from the specification's constants.
        h = vec![0; HEADER_LEN];
        h.get_mut(..12)?.copy_from_slice(b"AcFssFcAJMB\0");
        for (o, v) in [(0x10, 0x6cu32), (0x14, 4), (0x24, 1), (0x44, 0x20), (0x48, 0x80), (0x4c, 0x40)] {
            h.get_mut(o..o + 4)?.copy_from_slice(&v.to_le_bytes());
        }
    }
    let rel = (page_map_at - FIRST_PAGE) as u64;
    let put32 = |h: &mut Vec<u8>, o: usize, v: u32| h.get_mut(o..o + 4).map(|s| s.copy_from_slice(&v.to_le_bytes()));
    let put64 = |h: &mut Vec<u8>, o: usize, v: u64| h.get_mut(o..o + 8).map(|s| s.copy_from_slice(&v.to_le_bytes()));
    put32(&mut h, 0x28, page_map_id)?;
    put64(&mut h, 0x2c, rel)?;
    put64(&mut h, 0x34, end as u64)?;
    put32(&mut h, 0x3c, 0)?;
    put32(&mut h, 0x40, page_map_id)?;
    put32(&mut h, 0x50, page_map_id)?;
    put64(&mut h, 0x54, rel)?;
    put32(&mut h, 0x5c, section_map_id)?;
    put32(&mut h, 0x60, page_map_id)?;
    put32(&mut h, 0x64, 0)?;
    put32(&mut h, 0x68, 0)?;
    let crc = crc32(&h);
    put32(&mut h, 0x68, crc)?;
    crypt_header(&mut h);
    out.get_mut(HEADER_AT..HEADER_AT + HEADER_LEN)?.copy_from_slice(&h);
    out.extend_from_slice(&h);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_streams_decompress_to_their_data() {
        for n in [4usize, 5, 18, 19, 20, 274, 275, 276, 530, 0x7400] {
            let data: Vec<u8> = (0..n).map(|i| (i * 7 % 251) as u8).collect();
            let (back, ok) = decompress(&literal_stream(&data), PAGE_DATA);
            assert!(ok, "{n}");
            assert_eq!(back, data, "{n}");
        }
    }

    #[test]
    fn object_maps_round_trip() {
        let entries: Vec<(u64, usize)> = (0..2000u64).map(|i| (0x100 + i * 3, 4 + i as usize * 41)).collect();
        assert_eq!(read_object_map(&write_object_map(&entries)), entries);
    }

    #[test]
    fn hostile_bytes_never_panic() {
        let mut b = b"AC1032".to_vec();
        b.resize(0x400, 0);
        for seed in 0..200u32 {
            let mut x = seed.wrapping_mul(2_654_435_761);
            for v in b.iter_mut().skip(6) {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                *v = x as u8;
            }
            let _ = salvage(&b);
            let _ = decompress(&b, PAGE_DATA);
            let _ = read_object_map(&b);
            let _ = walk_records(&b, seed % 2 == 0);
        }
    }
}
