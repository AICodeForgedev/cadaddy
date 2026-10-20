//! The R2007 (AC1021) file layout (spec §5): 0x80 bytes of meta data, the file header (a
//! Reed–Solomon coded page at 0x80, repeated at the end of the file), then pages from 0x480: the
//! data sections' pages, the section map and the page map (system pages), each with a copy.
//!
//! - **File header** (§5.2): three interleaved (255, 239) blocks; the data holds a checking
//!   sequence, CRCs and the 0x110-byte header, compressed or not, which gives the page map's and
//!   section map's places, sizes, CRCs and repeat counts.
//! - **System pages** (§5.3): the data (compressed or not, padded to 8) repeated as often as fits,
//!   coded as interleaved (255, 239) blocks.
//! - **Data pages** (§5.4): encoding 4, interleaved (255, 251) blocks; encoding 1, the data as is
//!   (observed: no parity at all, the page padded to 0x20).
//!
//! Writing makes the layout anew from the sections' contents: pages in the order read (or the
//! spec's for a new file), each page stored uncompressed (the spec allows it when compression
//! doesn't help: [`crate::lz21`] reads both), every checksum, CRC and seed as §5.2–§5.4 give them,
//! the random values and padding from the CRC random encoding (§5.11) seeded 0.

use crate::crc64;
use crate::dwg::{Dwg, Layout as AnyLayout, Section};
use crate::error::corrupt;
use crate::frame;
use crate::lz21;
use crate::objects::{self, RawObject};
use crate::rand21::Random;
use crate::rs21::{self, Code};
use crate::{Error, Limits, Result, Version, objmap};

pub const HEADER: &str = "AcDb:Header";
pub const CLASSES: &str = "AcDb:Classes";
pub const OBJECTS: &str = "AcDb:AcDbObjects";
pub const HANDLES: &str = "AcDb:Handles";
const OBJECTS_MAGIC: u32 = 0x0DCA;
/// Where the pages start.
const PAGES_AT: usize = 0x480;
/// The decompressed file header's size.
const FILE_HEADER_LEN: usize = 0x110;
const MAX_PAGES: usize = 1 << 20;
const MAX_SECTIONS: usize = 256;

/// A section as the section map describes it (spec §5.2).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Desc {
    name: String,
    max_size: u64,
    encryption: u64,
    hash: u64,
    unknown: u64,
    encoding: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    /// The meta data (bytes 0..0x80).
    head: Vec<u8>,
    /// The sections in section-map order.
    descs: Vec<Desc>,
    /// The sections' order in the file (indices into `descs`).
    stream: Vec<usize>,
}

impl Layout {
    /// The code page number (file header byte 0x13).
    pub(crate) fn code_page(&self) -> u16 {
        self.head.get(0x13..0x15).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
    }
}

fn u64_at(b: &[u8], at: usize) -> Result<u64> {
    let s = b.get(at..at + 8).ok_or(Error::Truncated("the file header"))?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Ok(u64::from_le_bytes(a))
}

fn size(v: u64, what: &'static str, max: usize) -> Result<usize> {
    usize::try_from(v).ok().filter(|n| *n <= max).ok_or(Error::TooLarge(what))
}

/// A system page's data (spec §5.3): `blocks` interleaved (255, 239) blocks repeat the stored
/// data `factor` times; the first copy, decompressed when it is shorter than `usize`.
fn system_page(bytes: &[u8], at: usize, csize: usize, usize_: usize, factor: u64) -> Result<Vec<u8>> {
    let aligned = csize.checked_add(7).ok_or(Error::TooLarge("a system page"))? & !7;
    let factor = usize::try_from(factor).ok().filter(|f| (1..=4096).contains(f)).ok_or_else(|| corrupt("a system page's repeat count"))?;
    let total = aligned.checked_mul(factor).ok_or(Error::TooLarge("a system page"))?;
    let blocks = total.div_ceil(239);
    let enc = bytes.get(at..at.checked_add(blocks * rs21::N).ok_or(Error::TooLarge("a system page"))?).ok_or(Error::Truncated("a system page"))?;
    let data = rs21::deinterleave(enc, blocks, 239);
    let stored = data.get(..csize).ok_or(Error::Truncated("a system page"))?;
    if csize < usize_ { lz21::decompress(stored, usize_) } else { Ok(stored.to_vec()) }
}

/// A data page's content (spec §5.4).
fn data_page(bytes: &[u8], at: usize, csize: usize, usize_: usize, encoding: u64) -> Result<Vec<u8>> {
    let stored = if encoding == 4 {
        let aligned = csize.checked_add(7).ok_or(Error::TooLarge("a data page"))? & !7;
        let blocks = aligned.div_ceil(251);
        let enc = bytes.get(at..at.checked_add(blocks * rs21::N).ok_or(Error::TooLarge("a data page"))?).ok_or(Error::Truncated("a data page"))?;
        let mut d = rs21::deinterleave(enc, blocks, 251);
        d.truncate(csize);
        d
    } else {
        bytes.get(at..at.checked_add(csize).ok_or(Error::TooLarge("a data page"))?).ok_or(Error::Truncated("a data page"))?.to_vec()
    };
    if csize < usize_ { lz21::decompress(&stored, usize_) } else { Ok(stored) }
}

/// The file header's 0x110 bytes from the page at 0x80 (spec §5.2).
fn file_header(bytes: &[u8]) -> Result<Vec<u8>> {
    let page = bytes.get(0x80..0x80 + 3 * rs21::N).ok_or(Error::Truncated("the file header"))?;
    let data = rs21::deinterleave(page, 3, 239);
    let n = u64_at(&data, 24)? as i64;
    let stored_len = usize::try_from(n.unsigned_abs()).map_err(|_| corrupt("the file header's size"))?;
    let stored = data
        .get(32..32usize.checked_add(stored_len).ok_or_else(|| corrupt("the file header's size"))?)
        .ok_or_else(|| corrupt("the file header's size"))?;
    let h = if n > 0 { lz21::decompress(stored, FILE_HEADER_LEN)? } else { stored.to_vec() };
    if h.len() < FILE_HEADER_LEN {
        return Err(corrupt("the file header is short"));
    }
    let stored_crc = u64_at(&h, 0x108)?;
    let mut zeroed = h.clone();
    zeroed[0x108..0x110].fill(0);
    let computed = crc64::normal(crc64::seed2(0, FILE_HEADER_LEN as u64), &zeroed);
    if stored_crc != computed {
        return Err(corrupt("the file header's CRC"));
    }
    Ok(h)
}

pub(crate) fn read(bytes: &[u8], limits: &Limits) -> Result<Dwg> {
    let head = bytes.get(..0x80).ok_or(Error::Truncated("the meta data"))?.to_vec();
    let h = file_header(bytes)?;
    let f = |at: usize| u64_at(&h, at);
    // The page map: (size, id) pairs; pages from 0x480 on.
    let pm_at = PAGES_AT.checked_add(size(f(0x38)?, "the page map address", bytes.len())?).ok_or(Error::TooLarge("the page map"))?;
    let pm = system_page(
        bytes,
        pm_at,
        size(f(0x50)?, "the page map", limits.max_section_bytes)?,
        size(f(0x58)?, "the page map", limits.max_section_bytes)?,
        f(0x18)?,
    )?;
    let mut pages: std::collections::HashMap<u64, (usize, usize)> = std::collections::HashMap::new();
    let mut offset = 0usize;
    for pair in pm.as_chunks::<16>().0 {
        let page_size = size(u64_at(pair, 0)?, "a page", bytes.len())?;
        let id = (u64_at(pair, 8)? as i64).unsigned_abs();
        if pages.len() >= MAX_PAGES {
            return Err(Error::TooLarge("the page map"));
        }
        pages.insert(id, (PAGES_AT + offset, page_size));
        offset = offset.checked_add(page_size).ok_or(Error::TooLarge("the page map"))?;
    }
    let page_at = |id: u64| pages.get(&id).map(|p| p.0).ok_or_else(|| corrupt(format!("page {id} is not in the page map")));

    // The section map.
    let sm = system_page(
        bytes,
        page_at(f(0xC0)?)?,
        size(f(0xB0)?, "the section map", limits.max_section_bytes)?,
        size(f(0xC8)?, "the section map", limits.max_section_bytes)?,
        f(0xD8)?,
    )?;
    let mut descs = Vec::new();
    let mut contents = Vec::new();
    let mut first_page = Vec::new();
    let mut p = 0usize;
    while p + 64 <= sm.len() {
        if descs.len() >= MAX_SECTIONS {
            return Err(Error::TooLarge("the section map"));
        }
        let v = |k: usize| u64_at(&sm, p + 8 * k);
        let data_size = size(v(0)?, "a section", limits.max_section_bytes)?;
        let name_len = size(v(4)?, "a section name", 512)?;
        let npages = size(v(7)?, "a section", MAX_PAGES)?;
        let mut d = Desc { name: String::new(), max_size: v(1)?, encryption: v(2)?, hash: v(3)?, unknown: v(5)?, encoding: v(6)? };
        p += 64;
        let name = sm.get(p..p + name_len).ok_or(Error::Truncated("the section map"))?;
        d.name = String::from_utf16_lossy(&name.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect::<Vec<_>>())
            .trim_end_matches('\0')
            .to_string();
        p += name_len;
        if d.encryption == 1 {
            return Err(Error::Unsupported(format!("encrypted DWG sections ({})", d.name)));
        }
        let mut out = vec![0u8; data_size];
        let mut first = u64::MAX;
        for _ in 0..npages {
            let w = |k: usize| u64_at(&sm, p + 8 * k);
            let start = size(w(0)?, "a page offset", data_size)?;
            let id = w(2)?;
            let usize_ = size(w(3)?, "a page", limits.max_section_bytes)?;
            let csize = size(w(4)?, "a page", limits.max_section_bytes)?;
            p += 56;
            first = first.min(id);
            let content = data_page(bytes, page_at(id)?, csize, usize_, d.encoding)?;
            let n = content.len().min(data_size - start);
            if let (Some(slot), Some(src)) = (out.get_mut(start..start + n), content.get(..n)) {
                slot.copy_from_slice(src);
            }
        }
        descs.push(d);
        contents.push(out);
        first_page.push(first);
    }
    let mut stream: Vec<usize> = (0..descs.len()).collect();
    stream.sort_by_key(|&i| first_page.get(i).copied().unwrap_or(u64::MAX));

    let by_name = |name: &str| descs.iter().position(|d| d.name == name).and_then(|i| contents.get(i));
    let (header, _) = frame::read(
        by_name(HEADER).ok_or_else(|| corrupt("no header variables section"))?,
        &frame::HEADER_START,
        &frame::HEADER_END,
        false,
        true,
        "header variables",
    )?;
    let (classes, _) = frame::read(
        by_name(CLASSES).ok_or_else(|| corrupt("no classes section"))?,
        &frame::CLASSES_START,
        &frame::CLASSES_END,
        false,
        true,
        "classes",
    )?;
    let objects_data = by_name(OBJECTS).ok_or_else(|| corrupt("no objects section"))?;
    let (entries, _) = objmap::read(by_name(HANDLES).ok_or_else(|| corrupt("no object map"))?, limits.max_objects)?;
    let mut placed: Vec<(u64, RawObject)> = Vec::with_capacity(entries.len());
    for e in &entries {
        let at = usize::try_from(e.offset).map_err(|_| corrupt("an object offset"))?;
        let (o, _) = objects::read(objects_data, at, false, e.handle)?;
        placed.push((e.offset, o));
    }
    placed.sort_by_key(|p| p.0);
    let objects = placed.into_iter().map(|p| p.1).collect();
    let sections = descs
        .iter()
        .zip(&contents)
        .filter(|(d, _)| ![HEADER, CLASSES, OBJECTS, HANDLES].contains(&d.name.as_str()))
        .map(|(d, c)| Section { name: d.name.clone(), data: c.clone() })
        .collect();
    let layout = Layout { head, descs, stream };
    Ok(Dwg { version: Version::R2007, header, classes, objects, sections, layout: AnyLayout::R2007(Box::new(layout)) })
}

/// The sections of a new file, in section-map order, with the spec §5.2 table's properties
/// (name, page size, hash code, encoding); and their order in the file.
pub(crate) fn fresh() -> Layout {
    let table: [(&str, u64, u64, u64); 10] = [
        ("AcDb:Preview", 0x400, 0x40aa_0473, 1),
        ("AcDb:AcDbObjects", 0xf800, 0x674c_05a9, 4),
        ("AcDb:ObjFreeSpace", 0xf800, 0x77e2_061f, 4),
        ("AcDb:Template", 0x400, 0x4a14_04ce, 4),
        (HANDLES, 0xf800, 0x3f6e_0450, 4),
        (CLASSES, 0xf800, 0x3f54_045f, 4),
        ("AcDb:AuxHeader", 0x800, 0x54f0_050a, 4),
        (HEADER, 0x800, 0x32b8_03d9, 4),
        ("AcDb:SummaryInfo", 0x80, 0x717a_060f, 1),
        ("AcDb:AppInfo", 0x300, 0x3fa0_043e, 1),
    ];
    let descs: Vec<Desc> = table
        .iter()
        .map(|&(name, max_size, hash, encoding)| Desc { name: name.into(), max_size, encryption: 0, hash, unknown: 0, encoding })
        .collect();
    // In the file: preview first, then the objects … the header (spec §5.1).
    let stream = (0..descs.len()).collect();
    let mut head = vec![0u8; 0x80];
    head[..6].copy_from_slice(b"AC1021");
    // Maintenance version, 3, the writing version and its maintenance version, code page 30.
    head[0x0B] = 0x19;
    head[0x0C] = 3;
    head[0x11] = 0x1B;
    head[0x12] = 0x19;
    head[0x13] = 30;
    head[0x28..0x2C].copy_from_slice(&0x80u32.to_le_bytes());
    Layout { head, descs, stream }
}

/// A page being placed.
struct Page {
    id: u64,
    bytes: Vec<u8>,
}

/// A data page of `chunk` (spec §5.4): (file bytes, checksum, CRC).
fn encode_data_page(chunk: &[u8], encoding: u64, code: &Code, rnd: &mut Random) -> (Vec<u8>, u64, u64) {
    let checksum = u64::from(page_checksum(chunk));
    let crc = crc64::mirrored(crc64::seed1(0, chunk.len() as u64), chunk);
    let mut padded = chunk.to_vec();
    padded.resize(padded.len().div_ceil(8) * 8, 0);
    let mut out = if encoding == 4 {
        let blocks = padded.len().div_ceil(code.k).max(1);
        code.encode_interleaved(&padded, blocks, &mut || rnd.pad())
    } else {
        padded
    };
    out.resize(out.len().div_ceil(0x20) * 0x20, 0);
    (out, checksum, crc)
}

/// A system page of `data` (spec §5.3): (file bytes, repeat count, CRC of the stored data).
fn encode_system_page(data: &[u8], code: &Code, rnd: &mut Random) -> (Vec<u8>, u64, u64) {
    let aligned = data.len().div_ceil(8) * 8;
    let page = system_page_size(data.len());
    let max_blocks = page / rs21::N;
    let factor = (max_blocks * code.k / aligned.max(1)).max(1);
    let mut buf = Vec::with_capacity(aligned * factor);
    for _ in 0..factor {
        buf.extend_from_slice(data);
        buf.resize(buf.len().div_ceil(8) * 8, 0);
    }
    let blocks = buf.len().div_ceil(code.k);
    let mut out = code.encode_interleaved(&buf, blocks, &mut || rnd.pad());
    while out.len() < page {
        out.push(rnd.pad());
    }
    let crc = crc64::mirrored(crc64::seed1(0, data.len() as u64), data);
    (out, factor as u64, crc)
}

/// Spec §5.3.1 GetSystemPageSize.
fn system_page_size(data: usize) -> usize {
    let aligned = data.div_ceil(8) * 8;
    let size = (aligned * 2).div_ceil(239) * rs21::N;
    if size < 0x400 { 0x400 } else { size.div_ceil(0x20) * 0x20 }
}

/// Spec §5.4.1: the data page checksum (seed 0).
fn page_checksum(data: &[u8]) -> u32 {
    let seed = (data.len() as u64).wrapping_mul(0x343fd).wrapping_add(0x269ec3);
    let (mut s1, mut s2) = ((seed & 0xffff) as u32, ((seed >> 16) & 0xffff) as u32);
    for big in data.chunks(0x15b0) {
        let (blocks, rest) = big.as_chunks::<8>();
        for b in blocks {
            for i in [6, 7, 4, 5, 2, 3, 0, 1] {
                s1 += u32::from(b[i]);
                s2 += s1;
            }
        }
        let order: &[usize] = match rest.len() {
            1 => &[0],
            2 => &[0, 1],
            3 => &[0, 1, 2],
            4 => &[2, 3, 0, 1],
            5 => &[2, 3, 0, 1, 4],
            6 => &[2, 3, 0, 1, 4, 5],
            7 => &[2, 3, 0, 1, 4, 5, 6],
            _ => &[],
        };
        for &i in order {
            s1 += u32::from(rest.get(i).copied().unwrap_or(0));
            s2 += s1;
        }
        s1 %= 0xfff1;
        s2 %= 0xfff1;
    }
    (s2 << 16) | (s1 & 0xffff)
}

/// Each section's content for writing.
fn content(dwg: &Dwg, name: &str) -> Option<Vec<u8>> {
    Some(match name {
        HEADER => frame::write(&dwg.header, &frame::HEADER_START, &frame::HEADER_END),
        CLASSES => frame::write(&dwg.classes, &frame::CLASSES_START, &frame::CLASSES_END),
        OBJECTS | HANDLES => {
            let mut out = OBJECTS_MAGIC.to_le_bytes().to_vec();
            let mut entries = Vec::with_capacity(dwg.objects.len());
            for o in &dwg.objects {
                entries.push(objmap::Entry { handle: o.handle, offset: out.len() as u64 });
                objects::write(&mut out, o, false);
            }
            if name == HANDLES {
                entries.sort_by_key(|e| e.handle);
                objmap::write(&entries)
            } else {
                out
            }
        }
        _ => dwg.section(name)?.data.clone(),
    })
}

pub(crate) fn write(dwg: &Dwg, l: &Layout) -> Result<Vec<u8>> {
    let data_code = Code::data();
    let sys_code = Code::system();
    let mut rnd = Random::new(0);
    // The encoded map seeds come first from the random values (spec §5.2.1.1).
    let sm_seed = rnd.encode(0);
    let pm_seed = rnd.encode(0);

    // Data pages, in the file's section order.
    let mut pages: Vec<Page> = Vec::new();
    // Per section (map order): its page records.
    let mut records: Vec<Vec<[u64; 7]>> = vec![Vec::new(); l.descs.len()];
    let mut sizes: Vec<u64> = vec![0; l.descs.len()];
    for &i in &l.stream {
        let Some(d) = l.descs.get(i) else { continue };
        let Some(data) = content(dwg, &d.name) else { continue };
        if let Some(s) = sizes.get_mut(i) {
            *s = data.len() as u64;
        }
        let max = usize::try_from(d.max_size).ok().filter(|m| *m > 0).ok_or_else(|| corrupt("a section's page size"))?;
        for (k, chunk) in data.chunks(max).enumerate() {
            let id = pages.len() as u64 + 1;
            let (bytes, checksum, crc) = encode_data_page(chunk, d.encoding, &data_code, &mut rnd);
            if let Some(r) = records.get_mut(i) {
                r.push([(k * max) as u64, d.max_size, id, chunk.len() as u64, chunk.len() as u64, checksum, crc]);
            }
            pages.push(Page { id, bytes });
        }
    }

    // The section map.
    let mut sm = Vec::new();
    for (i, d) in l.descs.iter().enumerate() {
        let name: Vec<u8> = d.name.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        let recs = records.get(i).map(Vec::as_slice).unwrap_or_default();
        for v in [sizes.get(i).copied().unwrap_or(0), d.max_size, d.encryption, d.hash, name.len() as u64, d.unknown, d.encoding, recs.len() as u64] {
            sm.extend_from_slice(&v.to_le_bytes());
        }
        sm.extend_from_slice(&name);
        for r in recs {
            for v in r {
                sm.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    let (sm_bytes, sm_factor, sm_crc) = encode_system_page(&sm, &sys_code, &mut rnd);
    let sm_id = pages.len() as u64 + 1;
    pages.push(Page { id: sm_id, bytes: sm_bytes.clone() });
    pages.push(Page { id: sm_id + 1, bytes: sm_bytes });

    // The page map: every page (its own two included) and a closing empty pair.
    let pm_id = sm_id + 2;
    let pm_page_size = system_page_size((pages.len() + 3) * 16);
    let mut pm = Vec::new();
    for p in &pages {
        pm.extend_from_slice(&(p.bytes.len() as u64).to_le_bytes());
        pm.extend_from_slice(&p.id.to_le_bytes());
    }
    for id in [pm_id, pm_id + 1] {
        pm.extend_from_slice(&(pm_page_size as u64).to_le_bytes());
        pm.extend_from_slice(&id.to_le_bytes());
    }
    pm.extend_from_slice(&[0; 16]);
    let (pm_bytes, pm_factor, pm_crc) = encode_system_page(&pm, &sys_code, &mut rnd);
    if pm_bytes.len() != pm_page_size {
        return Err(corrupt("the page map's page size"));
    }
    let offset_of = |id: u64, pages: &[Page]| pages.iter().take_while(|p| p.id != id).map(|p| p.bytes.len() as u64).sum::<u64>();
    pages.push(Page { id: pm_id, bytes: pm_bytes.clone() });
    pages.push(Page { id: pm_id + 1, bytes: pm_bytes });
    let pm_off = offset_of(pm_id, &pages);
    let pm2_off = offset_of(pm_id + 1, &pages);
    let end = pages.iter().map(|p| p.bytes.len() as u64).sum::<u64>();

    // The file header (spec §5.2).
    let mut h = vec![0u8; FILE_HEADER_LEN];
    let fields: [(usize, u64); 34] = [
        (0x00, 0x70),
        (0x08, PAGES_AT as u64 + end + 0x400),
        (0x10, pm_crc),
        (0x18, pm_factor),
        (0x20, pm_seed),
        (0x28, pm2_off),
        (0x30, pm_id + 1),
        (0x38, pm_off),
        (0x40, pm_id),
        (0x48, end),
        (0x50, pm.len() as u64),
        (0x58, pm.len() as u64),
        (0x60, pm_id + 1),
        (0x68, pm_id + 1),
        (0x70, 0x20),
        (0x78, 0x40),
        (0x80, pm_crc),
        (0x88, 0xf800),
        (0x90, 4),
        (0x98, 1),
        (0xA0, l.descs.len() as u64 + 1),
        (0xA8, sm_crc),
        (0xB0, sm.len() as u64),
        (0xB8, sm_id + 1),
        (0xC0, sm_id),
        (0xC8, sm.len() as u64),
        (0xD0, sm_crc),
        (0xD8, sm_factor),
        (0xE0, sm_seed),
        (0xE8, 0x60100),
        (0xF0, 0),
        (0xF8, 0),
        (0x100, 0),
        (0x108, 0),
    ];
    let put = |h: &mut Vec<u8>, at: usize, v: u64| {
        if let Some(s) = h.get_mut(at..at + 8) {
            s.copy_from_slice(&v.to_le_bytes());
        }
    };
    for (at, v) in fields {
        put(&mut h, at, v);
    }
    // Check data: two random values, the encoded CRC seed, and two CRCs of them (§5.2.1.1.5).
    let r1 = rnd.next_u64();
    let r2 = rnd.next_u64();
    let seed_check = rnd.encode(0);
    put(&mut h, 0xF8, rnd.encode(0));
    let rot = |v: u64, c: u64| v.rotate_left((c & 0x1f) as u32);
    let words = |w: [u64; 8]| w.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let mut b = [0u64; 8];
    b[0] = rot(r1, r2);
    b[1] = rot(b[0], b[0]);
    b[2] = rot(r2, b[1]);
    b[3] = rot(b[2], b[2]);
    b[4] = rot(r1, b[3]);
    b[5] = rot(b[4], b[4]);
    b[6] = rot(b[5], b[5]);
    b[7] = rot(b[6], b[6]);
    let normal = crc64::normal(!r2, &words(b));
    b[0] = rot(r1, r2);
    b[1] = rot(normal, b[0]);
    b[2] = rot(r2, b[1]);
    b[3] = rot(normal, b[2]);
    b[4] = rot(r1, b[3]);
    b[5] = rot(normal, b[4]);
    b[6] = rot(r2, b[5]);
    b[7] = rot(b[6], b[6]);
    let mirrored = crc64::mirrored(!r1, &words(b));
    let header_crc = crc64::normal(crc64::seed2(0, FILE_HEADER_LEN as u64), &h);
    put(&mut h, 0x108, header_crc);

    // The page at 0x80: the checking sequence, the header (stored uncompressed), repeated.
    let first = rnd.next_u64();
    let mut seq = first.to_le_bytes().to_vec();
    seq.extend_from_slice(&rot(first, first).to_le_bytes());
    let seq_crc = crc64::normal(crc64::seed1(0, 16), &seq);
    let mut block = Vec::new();
    block.extend_from_slice(&seq_crc.to_le_bytes());
    block.extend_from_slice(&first.to_le_bytes());
    block.extend_from_slice(&crc64::normal(crc64::seed2(0, h.len() as u64), &h).to_le_bytes());
    block.extend_from_slice(&(-(FILE_HEADER_LEN as i64)).to_le_bytes());
    block.extend_from_slice(&h);
    while block.len() % 8 != 0 {
        block.push(rnd.pad());
    }
    let mut buf = Vec::with_capacity(3 * 239);
    while buf.len() + block.len() <= 3 * 239 {
        buf.extend_from_slice(&block);
    }
    while buf.len() < 3 * 239 {
        buf.push(rnd.pad());
    }
    let mut header_page = sys_code.encode_interleaved(&buf, 3, &mut || rnd.pad());
    while header_page.len() < 0x400 {
        header_page.push(rnd.pad());
    }
    header_page.truncate(0x3D8);
    for v in [normal, mirrored, r1, r2, seed_check] {
        header_page.extend_from_slice(&v.to_le_bytes());
    }

    let mut out = l.head.clone();
    out.resize(0x80, 0);
    // The meta data's addresses: preview, summary info and application info (0 when absent).
    let address = |name: &str| {
        let i = l.descs.iter().position(|d| d.name == name)?;
        let id = records.get(i)?.first()?[2];
        Some((PAGES_AT as u64 + offset_of(id, &pages)) as u32)
    };
    for (at, name) in [(0x0D, "AcDb:Preview"), (0x20, "AcDb:SummaryInfo"), (0x2C, "AcDb:AppInfo")] {
        if let Some(slot) = out.get_mut(at..at + 4) {
            slot.copy_from_slice(&address(name).unwrap_or(0).to_le_bytes());
        }
    }
    out.extend_from_slice(&header_page);
    for p in &pages {
        out.extend_from_slice(&p.bytes);
    }
    out.extend_from_slice(&header_page);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_page_sizes_follow_the_spec() {
        assert_eq!(system_page_size(0x110), 0x400);
        assert_eq!(system_page_size(0x704), 0x1000);
    }
}
