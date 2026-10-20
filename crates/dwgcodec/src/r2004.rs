//! The R2004 file layout, also used by R2010, R2013 and R2018 (spec §4, §6–§8).
//!
//! A 0x100-byte file header: 0x80 plain bytes, then 0x6C bytes of "file header data" XORed with
//! a fixed sequence (spec §4.1; its CRC-32 is taken with its own CRC field zeroed), then 0x14
//! more bytes of that sequence. Then pages, each on a 0x20 boundary, padded with the same
//! sequence: data pages with a 32-byte header masked by the page's address (spec §4.6), and two
//! system pages (spec §4.3), the section map (spec §4.5) and the page map (spec §4.4), whose
//! page numbers and the page map's address the file header data gives. The file ends with a copy
//! of the 0x6C encrypted bytes.
//!
//! Sections are split into pages of their own page size, each LZ77-compressed
//! ([`crate::lz77`]) or stored. Writing reuses each page whose section did not change, so an
//! unchanged file comes back byte for byte; a changed section is split, compressed and checked
//! again, and the maps, addresses and file header follow.

use std::collections::HashMap;

use crate::crc::{crc32, page_checksum};
use crate::dwg::{Dwg, Layout as AnyLayout, Section};
use crate::error::corrupt;
use crate::frame::{self, Framed};
use crate::objects::{self, RawObject};
use crate::{Error, Limits, Result, Version, lz77, objmap};

const PAGE_MAP: u32 = 0x4163_0E3B;
const SECTION_MAP: u32 = 0x4163_003B;
const DATA_PAGE: u32 = 0x4163_043B;
const HEADER_DATA_LEN: usize = 0x6C;
/// Most pages and sections read.
const MAX_PAGES: usize = 1 << 20;
const MAX_SECTIONS: usize = 256;

pub const HEADER: &str = "AcDb:Header";
pub const CLASSES: &str = "AcDb:Classes";
pub const OBJECTS: &str = "AcDb:AcDbObjects";
pub const HANDLES: &str = "AcDb:Handles";
pub const PREVIEW: &str = "AcDb:Preview";
pub const SUMMARY: &str = "AcDb:SummaryInfo";
pub const VBA: &str = "AcDb:VBAProject";
/// The value the objects section starts with (spec §20).
const OBJECTS_MAGIC: u32 = 0x0DCA;

/// The sequence the file header data is XORed with and pages are padded with (spec §4.1).
fn magic(n: usize) -> Vec<u8> {
    let mut seed: u32 = 1;
    (0..n)
        .map(|_| {
            seed = seed.wrapping_mul(0x343FD).wrapping_add(0x269EC3);
            (seed >> 16) as u8
        })
        .collect()
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    let s = b.get(at..at + 4).ok_or(Error::Truncated("an R2004 header"))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u64_at(b: &[u8], at: usize) -> Result<u64> {
    let s = b.get(at..at + 8).ok_or(Error::Truncated("an R2004 header"))?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Ok(u64::from_le_bytes(a))
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    if let Some(s) = b.get_mut(at..at + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

fn put_u64(b: &mut [u8], at: usize, v: u64) {
    if let Some(s) = b.get_mut(at..at + 8) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

/// A data section's description in the section map (spec §4.5).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Desc {
    size: u64,
    max_size: u32,
    unknown: u32,
    compressed: u32,
    id: u32,
    encrypted: u32,
    name: [u8; 64],
    /// (page number, data size, start offset).
    pages: Vec<(i32, u32, u64)>,
}

impl Desc {
    fn name(&self) -> String {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(64);
        String::from_utf8_lossy(&self.name[..end]).into_owned()
    }
}

/// One page as stored: its number (negative: a gap) and its bytes.
#[derive(Clone, Debug, PartialEq)]
struct Page {
    id: i32,
    /// The page's whole extent in the file (its size in the page map).
    bytes: Vec<u8>,
    /// A gap's extra fields (parent, left, right, 0).
    gap: Option<[u32; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    version: Version,
    /// The file's first 0x80 bytes.
    head: Vec<u8>,
    /// The decrypted file header data.
    data: Vec<u8>,
    /// The pages in file order.
    pages: Vec<Page>,
    /// The section map's first five longs.
    map_head: [u32; 5],
    descs: Vec<Desc>,
    /// Each section's content as read (length, CRC-32), to see what changed.
    original: Vec<(usize, u32)>,
    page_map_content: Vec<u8>,
    section_map_content: Vec<u8>,
    /// Where the closing copy of the header data starts relative to the end of the last page
    /// (other writers put it where the page map's data ends, which can be inside that page's
    /// extent), the bytes between when it is after, and the bytes after the copy.
    copy_shift: i64,
    gap_before_copy: Vec<u8>,
    tail: Vec<u8>,
    /// Where the header data's start addresses pointed, as read (preview, summary, VBA).
    starts: [u32; 3],
}

/// Header-variables and classes sections have the second size field from R2010 (maintenance
/// version above 3) and in R2018 (spec §9, §10.2).
fn has_hi(version: Version, maintenance: u8) -> bool {
    version >= Version::R2018 || (version >= Version::R2010 && maintenance > 3)
}

/// Decompresses a system page at `at`: (type, content).
fn system_page(bytes: &[u8], at: usize, limits: &Limits) -> Result<(u32, Vec<u8>)> {
    let ty = u32_at(bytes, at)?;
    let size = u32_at(bytes, at + 4)? as usize;
    let csize = u32_at(bytes, at + 8)? as usize;
    let stored = u32_at(bytes, at + 16)?;
    if size > limits.max_section_bytes {
        return Err(Error::TooLarge("a system page"));
    }
    let data = bytes.get(at + 20..(at + 20).checked_add(csize).ok_or(Error::Truncated("a system page"))?).ok_or(Error::Truncated("a system page"))?;
    let mut head = bytes.get(at..at + 20).ok_or(Error::Truncated("a system page"))?.to_vec();
    put_u32(&mut head, 16, 0);
    let computed = page_checksum(page_checksum(0, &head), data);
    if stored != computed {
        return Err(Error::BadCrc { what: "system page", stored, computed });
    }
    let content = lz77::decompress(data, size)?;
    if content.len() != size {
        return Err(corrupt("a system page shorter than its stated size"));
    }
    Ok((ty, content))
}

/// A compressed system page's bytes (not padded).
fn make_system_page(ty: u32, content: &[u8]) -> Result<Vec<u8>> {
    let c = lz77::compress(content)?;
    let mut page = Vec::with_capacity(c.len() + 20);
    page.extend_from_slice(&ty.to_le_bytes());
    page.extend_from_slice(&(content.len() as u32).to_le_bytes());
    page.extend_from_slice(&(c.len() as u32).to_le_bytes());
    page.extend_from_slice(&2u32.to_le_bytes());
    page.extend_from_slice(&0u32.to_le_bytes());
    let sum = page_checksum(page_checksum(0, &page), &c);
    put_u32(&mut page, 16, sum);
    page.extend_from_slice(&c);
    Ok(page)
}

/// Pads `page` to a 0x20 boundary with the sequence.
fn pad(mut page: Vec<u8>) -> Vec<u8> {
    let n = page.len().next_multiple_of(0x20) - page.len();
    page.extend(magic(n));
    page
}

/// XORs a data page's 32-byte header with its address mask (spec §4.6).
fn mask(header: &mut [u8], address: u64) {
    let m = 0x4164_536B ^ (address as u32);
    for w in header.chunks_mut(4).take(8) {
        if w.len() == 4 {
            let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]) ^ m;
            w.copy_from_slice(&v.to_le_bytes());
        }
    }
}

/// A data page's decoded header fields.
struct DataHeader {
    section: u32,
    data_size: u32,
    start: u64,
}

/// Decodes and checks the data page at `address` (its extent in `page`, possibly followed by
/// more file bytes in `rest`).
fn data_page(file: &[u8], address: usize) -> Result<(DataHeader, &[u8])> {
    let mut h = file.get(address..address + 32).ok_or(Error::Truncated("a data page"))?.to_vec();
    mask(&mut h, address as u64);
    if u32_at(&h, 0)? != DATA_PAGE {
        return Err(corrupt("a data page of the wrong type"));
    }
    let data_size = u32_at(&h, 8)?;
    let start = u64_at(&h, 16)?;
    let header_sum = u32_at(&h, 24)?;
    let data_sum = u32_at(&h, 28)?;
    let data = file
        .get(address + 32..(address + 32).checked_add(data_size as usize).ok_or(Error::Truncated("a data page"))?)
        .ok_or(Error::Truncated("a data page"))?;
    let computed = page_checksum(0, data);
    if computed != data_sum {
        return Err(Error::BadCrc { what: "data page", stored: data_sum, computed });
    }
    put_u32(&mut h, 24, 0);
    let computed = page_checksum(data_sum, &h);
    if computed != header_sum {
        return Err(Error::BadCrc { what: "data page header", stored: header_sum, computed });
    }
    Ok((DataHeader { section: u32_at(&h, 4)?, data_size, start }, data))
}

/// A data page's bytes (header masked for `address`), padded.
fn make_data_page(section: u32, start: u64, data: &[u8], address: u64) -> Vec<u8> {
    let mut h = vec![0u8; 32];
    put_u32(&mut h, 0, DATA_PAGE);
    put_u32(&mut h, 4, section);
    put_u32(&mut h, 8, data.len() as u32);
    put_u32(&mut h, 12, (32 + data.len()).next_multiple_of(0x20) as u32);
    put_u64(&mut h, 16, start);
    let data_sum = page_checksum(0, data);
    put_u32(&mut h, 28, data_sum);
    let header_sum = page_checksum(data_sum, &h);
    put_u32(&mut h, 24, header_sum);
    mask(&mut h, address);
    h.extend_from_slice(data);
    pad(h)
}

pub(crate) fn read(bytes: &[u8], version: Version, limits: &Limits) -> Result<Dwg> {
    let head = bytes.get(..0x80).ok_or(Error::Truncated("the file header"))?.to_vec();
    let enc = bytes.get(0x80..0x80 + HEADER_DATA_LEN).ok_or(Error::Truncated("the file header"))?;
    let data: Vec<u8> = enc.iter().zip(magic(HEADER_DATA_LEN)).map(|(a, b)| a ^ b).collect();
    if data.get(..11) != Some(b"AcFssFcAJMB".as_slice()) {
        return Err(corrupt("the file header data's id is missing"));
    }
    let stored = u32_at(&data, 0x68)?;
    let mut zeroed = data.clone();
    put_u32(&mut zeroed, 0x68, 0);
    let computed = crc32(0, &zeroed);
    if stored != computed {
        return Err(Error::BadCrc { what: "file header data", stored, computed });
    }
    let maintenance = head.get(0x0B).copied().unwrap_or(0);

    // The page map: page numbers and sizes, addresses from 0x100 on.
    let page_map_at = usize::try_from(u64_at(&data, 0x54)?.checked_add(0x100).ok_or_else(|| corrupt("the page map address"))?)
        .map_err(|_| corrupt("the page map address"))?;
    let (ty, page_map_content) = system_page(bytes, page_map_at, limits)?;
    if ty != PAGE_MAP {
        return Err(corrupt("the page map has the wrong page type"));
    }
    let mut pages = Vec::new();
    let mut address_of: HashMap<i32, usize> = HashMap::new();
    let mut at = 0x100usize;
    let mut p = 0;
    while p + 8 <= page_map_content.len() {
        let id = u32_at(&page_map_content, p)? as i32;
        let size = u32_at(&page_map_content, p + 4)? as usize;
        p += 8;
        let gap = if id < 0 {
            let g = [
                u32_at(&page_map_content, p)?,
                u32_at(&page_map_content, p + 4)?,
                u32_at(&page_map_content, p + 8)?,
                u32_at(&page_map_content, p + 12)?,
            ];
            p += 16;
            Some(g)
        } else {
            None
        };
        if pages.len() >= MAX_PAGES {
            return Err(Error::TooLarge("the page map"));
        }
        let extent = bytes.get(at..at.checked_add(size).ok_or_else(|| corrupt("a page size"))?).ok_or(Error::Truncated("a page"))?.to_vec();
        address_of.insert(id, at);
        pages.push(Page { id, bytes: extent, gap });
        at += size;
    }
    let pages_end = at;

    // The section map.
    let section_map_id = u32_at(&data, 0x5C)? as i32;
    let map_at = *address_of.get(&section_map_id).ok_or_else(|| corrupt("the section map's page is not in the page map"))?;
    let (ty, section_map_content) = system_page(bytes, map_at, limits)?;
    if ty != SECTION_MAP {
        return Err(corrupt("the section map has the wrong page type"));
    }
    let m = &section_map_content;
    let map_head = [u32_at(m, 0)?, u32_at(m, 4)?, u32_at(m, 8)?, u32_at(m, 12)?, u32_at(m, 16)?];
    let count = map_head[0] as usize;
    if count > MAX_SECTIONS {
        return Err(Error::TooLarge("the section map"));
    }
    let mut descs = Vec::with_capacity(count);
    let mut p = 20;
    for _ in 0..count {
        let mut name = [0u8; 64];
        name.copy_from_slice(m.get(p + 32..p + 96).ok_or(Error::Truncated("the section map"))?);
        let mut d = Desc {
            size: u64_at(m, p)?,
            max_size: u32_at(m, p + 12)?,
            unknown: u32_at(m, p + 16)?,
            compressed: u32_at(m, p + 20)?,
            id: u32_at(m, p + 24)?,
            encrypted: u32_at(m, p + 28)?,
            name,
            pages: Vec::new(),
        };
        let page_count = u32_at(m, p + 8)? as usize;
        p += 96;
        if page_count > MAX_PAGES {
            return Err(Error::TooLarge("a section"));
        }
        for _ in 0..page_count {
            d.pages.push((u32_at(m, p)? as i32, u32_at(m, p + 4)?, u64_at(m, p + 8)?));
            p += 16;
        }
        descs.push(d);
    }

    // Each section's content.
    let mut contents = Vec::with_capacity(descs.len());
    for d in &descs {
        let size = usize::try_from(d.size).map_err(|_| Error::TooLarge("a section"))?;
        if size > limits.max_section_bytes {
            return Err(Error::TooLarge("a section"));
        }
        if d.encrypted == 1 {
            return Err(Error::Unsupported(format!("encrypted DWG sections ({})", d.name())));
        }
        let mut out = vec![0u8; size];
        for &(id, _, start) in &d.pages {
            let at = *address_of.get(&id).ok_or_else(|| corrupt("a section page not in the page map"))?;
            let (h, data) = data_page(bytes, at)?;
            if h.section != d.id || h.start != start || h.data_size as usize != data.len() {
                return Err(corrupt(format!("a page of {} disagrees with the section map", d.name())));
            }
            let content = if d.compressed == 2 { lz77::decompress(data, d.max_size as usize)? } else { data.to_vec() };
            let start = usize::try_from(start).map_err(|_| corrupt("a page start"))?;
            // A page holds at most the section's remaining bytes (spec §4.5).
            let n = content.len().min(size.saturating_sub(start));
            if let Some(slot) = out.get_mut(start..start + n) {
                slot.copy_from_slice(&content[..n]);
            }
        }
        contents.push(out);
    }
    let original = contents.iter().map(|c| (c.len(), crc32(0, c))).collect();

    // The closing copy of the header data.
    let copy_at = usize::try_from(u64_at(&data, 0x34)?).unwrap_or(usize::MAX);
    let (copy_shift, gap_before_copy, tail) =
        if copy_at >= 0x100 && copy_at + 0x20 >= pages_end && bytes.get(copy_at..copy_at + HEADER_DATA_LEN) == Some(enc) {
            (
                copy_at as i64 - pages_end as i64,
                bytes.get(pages_end..copy_at).unwrap_or_default().to_vec(),
                bytes.get(copy_at + HEADER_DATA_LEN..).unwrap_or_default().to_vec(),
            )
        } else {
            (0, Vec::new(), Vec::new())
        };

    // Interpret the sections this layer knows.
    let hi = has_hi(version, maintenance);
    let r2010 = version >= Version::R2010;
    let by_name = |name: &str| descs.iter().position(|d| d.name() == name).and_then(|i| contents.get(i));
    let (header, _) = frame::read(
        by_name(HEADER).ok_or_else(|| corrupt("no header variables section"))?,
        &frame::HEADER_START,
        &frame::HEADER_END,
        hi,
        true,
        "header variables",
    )?;
    let (classes, _) =
        frame::read(by_name(CLASSES).ok_or_else(|| corrupt("no classes section"))?, &frame::CLASSES_START, &frame::CLASSES_END, hi, true, "classes")?;
    let objects_data = by_name(OBJECTS).ok_or_else(|| corrupt("no objects section"))?;
    let (entries, _) = objmap::read(by_name(HANDLES).ok_or_else(|| corrupt("no object map"))?, limits.max_objects)?;
    let mut placed: Vec<(u64, RawObject)> = Vec::with_capacity(entries.len());
    for e in &entries {
        let at = usize::try_from(e.offset).map_err(|_| corrupt("an object offset"))?;
        let (o, _) = objects::read(objects_data, at, r2010, e.handle)?;
        placed.push((e.offset, o));
    }
    placed.sort_by_key(|p| p.0);
    let objects = placed.into_iter().map(|p| p.1).collect();
    let sections = descs
        .iter()
        .zip(&contents)
        .filter(|(d, _)| ![HEADER, CLASSES, OBJECTS, HANDLES].contains(&d.name().as_str()))
        .map(|(d, c)| Section { name: d.name(), data: c.clone() })
        .collect();
    let starts = [u32_at(&head, 0x0D)?, u32_at(&head, 0x20)?, u32_at(&head, 0x24)?];
    let layout = Layout {
        version,
        head,
        data,
        pages,
        map_head,
        descs,
        original,
        page_map_content,
        section_map_content,
        copy_shift,
        gap_before_copy,
        tail,
        starts,
    };
    Ok(Dwg { version, header, classes, objects, sections, layout: AnyLayout::R2004(Box::new(layout)) })
}

/// The objects section's content and the object map's entries.
fn objects_content(dwg: &Dwg) -> (Vec<u8>, Vec<objmap::Entry>) {
    let mut out = OBJECTS_MAGIC.to_le_bytes().to_vec();
    let mut entries = Vec::with_capacity(dwg.objects.len());
    for o in &dwg.objects {
        entries.push(objmap::Entry { handle: o.handle, offset: out.len() as u64 });
        objects::write(&mut out, o, dwg.version >= Version::R2010);
    }
    entries.sort_by_key(|e| e.handle);
    (out, entries)
}

/// A section's new content.
fn content(dwg: &Dwg, name: &str, objects: &(Vec<u8>, Vec<objmap::Entry>)) -> Vec<u8> {
    let framed = |f: &Framed, s: &[u8; 16], e: &[u8; 16]| frame::write(f, s, e);
    match name {
        HEADER => framed(&dwg.header, &frame::HEADER_START, &frame::HEADER_END),
        CLASSES => framed(&dwg.classes, &frame::CLASSES_START, &frame::CLASSES_END),
        OBJECTS => objects.0.clone(),
        HANDLES => objmap::write(&objects.1),
        _ => dwg.section(name).map(|s| s.data.clone()).unwrap_or_default(),
    }
}

/// A changed section's pages: (data page payload, start offset).
fn split(d: &Desc, content: &[u8]) -> Result<Vec<(Vec<u8>, u64)>> {
    let page = (d.max_size as usize).max(1);
    let mut out = Vec::new();
    for (i, chunk) in content.chunks(page).enumerate() {
        let start = (i * page) as u64;
        let payload = if d.compressed == 2 {
            if chunk.len() < 4 {
                // Too short for the compressor: pad with zeros (the section's size cuts them off).
                let mut c = chunk.to_vec();
                c.resize(4, 0);
                lz77::compress(&c)?
            } else {
                lz77::compress(chunk)?
            }
        } else {
            // Stored pages are whole pages (spec §4.5).
            let mut c = chunk.to_vec();
            c.resize(page, 0);
            c
        };
        out.push((payload, start));
    }
    Ok(out)
}

pub(crate) fn write(dwg: &Dwg, l: &Layout) -> Result<Vec<u8>> {
    let objects = objects_content(dwg);
    let contents: Vec<Vec<u8>> = l.descs.iter().map(|d| content(dwg, &d.name(), &objects)).collect();
    let changed: Vec<bool> = contents.iter().zip(&l.original).map(|(c, o)| (c.len(), crc32(0, c)) != *o).collect();
    let section_map_id = u32_at(&l.data, 0x5C)? as i32;
    let page_map_id = u32_at(&l.data, 0x50)? as i32;

    // The pages in file order: (id, kind). Kinds: kept page (index into l.pages), new data page
    // (section index, payload, start), section map, page map.
    enum Slot {
        Kept(usize),
        New { section: usize, payload: Vec<u8>, start: u64 },
        SectionMap,
        PageMap,
    }
    let owner: HashMap<i32, usize> = l.descs.iter().enumerate().flat_map(|(i, d)| d.pages.iter().map(move |p| (p.0, i))).collect();
    let mut slots: Vec<Slot> = Vec::new();
    let mut placed_new = vec![false; l.descs.len()];
    let mut new_pages: Vec<Option<Vec<(Vec<u8>, u64)>>> = Vec::with_capacity(l.descs.len());
    for (i, d) in l.descs.iter().enumerate() {
        new_pages.push(if changed[i] { Some(split(d, &contents[i])?) } else { None });
    }
    for (k, p) in l.pages.iter().enumerate() {
        if p.id == section_map_id {
            // Sections that had no pages and now need some go before the system pages.
            for (i, n) in new_pages.iter_mut().enumerate() {
                if !placed_new[i]
                    && l.descs[i].pages.is_empty()
                    && let Some(list) = n.take()
                {
                    slots.extend(list.into_iter().map(|(payload, start)| Slot::New { section: i, payload, start }));
                    placed_new[i] = true;
                }
            }
            slots.push(Slot::SectionMap);
        } else if p.id == page_map_id {
            slots.push(Slot::PageMap);
        } else if let Some(&i) = owner.get(&p.id) {
            if !changed[i] {
                slots.push(Slot::Kept(k));
            } else if !placed_new[i] {
                placed_new[i] = true;
                if let Some(list) = new_pages[i].take() {
                    slots.extend(list.into_iter().map(|(payload, start)| Slot::New { section: i, payload, start }));
                }
            }
        } else if !changed.iter().any(|&c| c) {
            // A gap or unknown page, kept while nothing moves.
            slots.push(Slot::Kept(k));
        }
    }
    if !slots.iter().any(|s| matches!(s, Slot::SectionMap)) || !slots.iter().any(|s| matches!(s, Slot::PageMap)) {
        return Err(corrupt("the system pages are not in the page map"));
    }

    // Page numbers: kept as they were while no section changed; otherwise 1, 2, … in file order.
    let renumber = changed.iter().any(|&c| c);
    let ids: Vec<i32> = slots
        .iter()
        .enumerate()
        .map(|(n, s)| match s {
            _ if renumber => n as i32 + 1,
            Slot::Kept(k) => l.pages[*k].id,
            Slot::SectionMap => section_map_id,
            _ => page_map_id,
        })
        .collect();
    let id_of_slot = |n: usize| ids.get(n).copied().unwrap_or(0);

    // The section map: each section's pages with their new numbers.
    let mut descs = l.descs.clone();
    for (i, d) in descs.iter_mut().enumerate() {
        if !changed[i] && !renumber {
            continue;
        }
        d.size = contents[i].len() as u64;
        let mut list = Vec::new();
        for (n, s) in slots.iter().enumerate() {
            match s {
                Slot::Kept(k) if owner.get(&l.pages[*k].id) == Some(&i) => {
                    if let Some(old) = l.descs[i].pages.iter().find(|p| p.0 == l.pages[*k].id) {
                        list.push((id_of_slot(n), old.1, old.2));
                    }
                }
                Slot::New { section, payload, start } if *section == i => list.push((id_of_slot(n), payload.len() as u32, *start)),
                _ => {}
            }
        }
        d.pages = list;
    }
    let mut section_map = Vec::new();
    let mut head = l.map_head;
    head[0] = descs.len() as u32;
    for v in head {
        section_map.extend_from_slice(&v.to_le_bytes());
    }
    for d in &descs {
        section_map.extend_from_slice(&d.size.to_le_bytes());
        section_map.extend_from_slice(&(d.pages.len() as u32).to_le_bytes());
        for v in [d.max_size, d.unknown, d.compressed, d.id, d.encrypted] {
            section_map.extend_from_slice(&v.to_le_bytes());
        }
        section_map.extend_from_slice(&d.name);
        for &(id, size, start) in &d.pages {
            section_map.extend_from_slice(&(id as u32).to_le_bytes());
            section_map.extend_from_slice(&size.to_le_bytes());
            section_map.extend_from_slice(&start.to_le_bytes());
        }
    }
    let section_map_index = slots.iter().position(|s| matches!(s, Slot::SectionMap)).unwrap_or(0);
    let section_map_page = if section_map == l.section_map_content {
        l.pages.iter().find(|p| p.id == section_map_id).map(|p| p.bytes.clone()).unwrap_or_default()
    } else {
        pad(make_system_page(SECTION_MAP, &section_map)?)
    };

    // Page bytes except the page map (whose size depends on itself), and the page map.
    let page_map_index = slots.iter().position(|s| matches!(s, Slot::PageMap)).unwrap_or(0);
    let mut sizes: Vec<usize> = Vec::with_capacity(slots.len());
    for s in &slots {
        sizes.push(match s {
            Slot::Kept(k) => l.pages[*k].bytes.len(),
            Slot::New { payload, .. } => (32 + payload.len()).next_multiple_of(0x20),
            Slot::SectionMap => section_map_page.len(),
            Slot::PageMap => 0,
        });
    }
    let page_map_for = |sizes: &[usize]| -> Vec<u8> {
        let mut m = Vec::new();
        for (n, s) in slots.iter().enumerate() {
            m.extend_from_slice(&(id_of_slot(n) as u32).to_le_bytes());
            m.extend_from_slice(&(sizes[n] as u32).to_le_bytes());
            if let Slot::Kept(k) = s
                && let Some(g) = l.pages[*k].gap
            {
                g.iter().for_each(|v| m.extend_from_slice(&v.to_le_bytes()));
            }
        }
        m
    };
    let old_page_map = l.pages.iter().find(|p| p.id == page_map_id).map(|p| p.bytes.clone()).unwrap_or_default();
    sizes[page_map_index] = old_page_map.len();
    let mut page_map = page_map_for(&sizes);
    let page_map_page = if page_map == l.page_map_content {
        old_page_map
    } else {
        // Its own size is in it: settle on a size that holds the compressed map.
        let mut page = Vec::new();
        for _ in 0..8 {
            page = pad(make_system_page(PAGE_MAP, &page_map)?);
            if page.len() == sizes[page_map_index] {
                break;
            }
            sizes[page_map_index] = page.len();
            page_map = page_map_for(&sizes);
        }
        page
    };
    sizes[page_map_index] = page_map_page.len();

    // Lay the pages out.
    let old_address: Vec<u64> = l
        .pages
        .iter()
        .scan(0x100u64, |at, p| {
            let here = *at;
            *at += p.bytes.len() as u64;
            Some(here)
        })
        .collect();
    let mut out = vec![0u8; 0x100];
    let mut first_page_of: HashMap<usize, u64> = HashMap::new();
    let mut last_id = 0i32;
    for (n, s) in slots.iter().enumerate() {
        let address = out.len() as u64;
        match s {
            Slot::Kept(k) => {
                let p = &l.pages[*k];
                let mut bytes = p.bytes.clone();
                if let Some(&i) = owner.get(&p.id) {
                    first_page_of.entry(i).or_insert(address);
                    // Re-mask the header for its new address (and new number).
                    let old_at = old_address.get(*k).copied().unwrap_or(address);
                    if old_at != address
                        && let Some(h) = bytes.get_mut(..32)
                    {
                        mask(h, old_at);
                        mask(h, address);
                    }
                }
                out.extend_from_slice(&bytes);
            }
            Slot::New { section, payload, start } => {
                first_page_of.entry(*section).or_insert(address);
                out.extend(make_data_page(l.descs[*section].id, *start, payload, address));
            }
            Slot::SectionMap => out.extend_from_slice(&section_map_page),
            Slot::PageMap => out.extend_from_slice(&page_map_page),
        }
        last_id = last_id.max(id_of_slot(n));
    }
    let pages_end = out.len() as u64;
    let page_map_at = 0x100 + sizes[..page_map_index].iter().map(|&s| s as u64).sum::<u64>();
    let unchanged = !renumber && page_map == l.page_map_content;
    if unchanged {
        if l.copy_shift < 0 {
            out.truncate(out.len().saturating_sub(l.copy_shift.unsigned_abs() as usize).max(0x100));
        }
        out.extend_from_slice(&l.gap_before_copy);
    }
    let copy_at = out.len() as u64;

    // The file header data.
    let mut data = l.data.clone();
    if !unchanged {
        put_u32(&mut data, 0x28, last_id as u32);
        put_u64(&mut data, 0x2C, pages_end - 0x100);
        put_u32(&mut data, 0x3C, slots.iter().filter(|s| matches!(s, Slot::Kept(k) if l.pages[*k].id < 0)).count() as u32);
        put_u32(&mut data, 0x40, slots.len() as u32);
        put_u32(&mut data, 0x50, id_of_slot(page_map_index) as u32);
        put_u32(&mut data, 0x5C, id_of_slot(section_map_index) as u32);
        put_u32(&mut data, 0x60, slots.len() as u32);
    }
    put_u64(&mut data, 0x34, copy_at);
    put_u64(&mut data, 0x54, page_map_at - 0x100);
    put_u32(&mut data, 0x68, 0);
    let crc = crc32(0, &data);
    put_u32(&mut data, 0x68, crc);
    let enc: Vec<u8> = data.iter().zip(magic(HEADER_DATA_LEN)).map(|(a, b)| a ^ b).collect();

    // The first 0x100 bytes: the plain header (with the start addresses of the preview, summary
    // and VBA pages, + 0x20), the header data, and 0x14 bytes of the sequence.
    let mut head = l.head.clone();
    head[..6].copy_from_slice(l.version.id().as_bytes());
    for (k, (at, name)) in [(0x0D, PREVIEW), (0x20, SUMMARY), (0x24, VBA)].into_iter().enumerate() {
        if l.starts[k] == 0 {
            continue;
        }
        let first = l.descs.iter().position(|d| d.name() == name).and_then(|i| first_page_of.get(&i));
        if let Some(&a) = first {
            put_u32(&mut head, at, (a + 0x20) as u32);
        }
    }
    out[..0x80].copy_from_slice(&head);
    out[0x80..0x80 + HEADER_DATA_LEN].copy_from_slice(&enc);
    let seq = magic(0x100);
    out[0x80 + HEADER_DATA_LEN..0x100].copy_from_slice(&seq[0x100 - 0x14..]);
    out.extend_from_slice(&enc);
    if unchanged {
        out.extend_from_slice(&l.tail);
    }
    Ok(out)
}
