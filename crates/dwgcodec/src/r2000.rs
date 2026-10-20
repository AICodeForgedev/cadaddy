//! The R2000 (AC1015) file layout (spec §3): a file header with section-locator records, then the
//! sections at the addresses those give, the objects, the object map (spec §23.1) and the
//! preview image (spec §14.2), whose address is the "image seeker".
//!
//! Section-locator records (spec §3.2.6): 0 header variables, 1 classes, 2 object map, 3 the
//! object free space data (spec §21, §25), 4 the template (spec §22), 5 the auxiliary header
//! (spec §27). The file header CRC is the 16-bit CRC from 0, XORed with a constant that depends
//! on the number of records.
//!
//! Writing keeps the order of the regions and the bytes between them, so a file written by a
//! careful writer comes back byte for byte; addresses are recomputed, and the values that hold
//! addresses (the locators, the image seeker, the objects' start in the free-space data, the
//! preview's own pointers) are updated to match. A second file header (spec §26) is kept only
//! while the addresses it repeats are unchanged; otherwise it is left out (it is a recovery copy).

use crate::crc::crc8;
use crate::dwg::{Dwg, Layout as AnyLayout, Section};
use crate::error::corrupt;
use crate::frame;
use crate::objects::{self, RawObject};
use crate::{Error, Limits, Result, Version, objmap};

/// The sentinel after the file header's CRC (spec §3.2.6).
const FILE_HEADER_END: [u8; 16] = [0x95, 0xA0, 0x4E, 0x28, 0x99, 0x82, 0x1A, 0xE5, 0x5E, 0x41, 0xE0, 0x5F, 0x9D, 0x3A, 0x4D, 0x00];
/// The preview's sentinels (spec §14.2).
const PREVIEW_START: [u8; 16] = [0x1F, 0x25, 0x6D, 0x07, 0xD4, 0x36, 0x28, 0x28, 0x9D, 0x57, 0xCA, 0x3F, 0x9D, 0x44, 0x10, 0x2B];
const PREVIEW_END: [u8; 16] = [0xE0, 0xDA, 0x92, 0xF8, 0x2B, 0xC9, 0xD7, 0xD7, 0x62, 0xA8, 0x35, 0xC0, 0x62, 0xBB, 0xEF, 0xD4];
/// The second file header's sentinels (spec §26).
const SECOND_START: [u8; 16] = [0xD4, 0x7B, 0x21, 0xCE, 0x28, 0x93, 0x9F, 0xBF, 0x53, 0x24, 0x40, 0x09, 0x12, 0x3C, 0xAA, 0x01];
const SECOND_END: [u8; 16] = [0x2B, 0x84, 0xDE, 0x31, 0xD7, 0x6C, 0x60, 0x40, 0xAC, 0xDB, 0xBF, 0xF6, 0xED, 0xC3, 0x55, 0xFE];
/// Most section-locator records read (spec §3.2.6 has seen up to 6).
const MAX_RECORDS: usize = 16;
/// Largest second file header searched for its end sentinel.
const MAX_SECOND_HEADER: usize = 4096;

pub const PREVIEW: &str = "AcDb:Preview";
pub const SECOND_HEADER: &str = "R2000:SecondHeader";

/// The XOR applied to the file header CRC for `n` locator records (spec §3.2.6).
fn header_crc_xor(n: usize) -> Option<u16> {
    match n {
        3 => Some(0xA598),
        4 => Some(0x8101),
        5 => Some(0x3CC4),
        6 => Some(0x8461),
        _ => None,
    }
}

/// The section a locator record holds, by the name the same data has from R2004 on.
fn record_name(rec: u8) -> String {
    match rec {
        3 => "AcDb:ObjFreeSpace".into(),
        4 => "AcDb:Template".into(),
        5 => "AcDb:AuxHeader".into(),
        n => format!("R2000:Record{n}"),
    }
}

/// A stretch of the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Region {
    Record(u8),
    Objects,
    Preview,
    SecondHeader,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    /// Bytes 0x06..0x15 of the file (maintenance version, image seeker, code page, …).
    head: [u8; 15],
    /// The locator records, in stored order, with the addresses and sizes read.
    records: Vec<(u8, u32, u32)>,
    /// The regions in file order, each with the bytes stored before it.
    regions: Vec<(Region, Vec<u8>)>,
    /// Bytes after the last region.
    tail: Vec<u8>,
    /// Where the objects started and the preview was, as read.
    objects_start: u32,
    preview_at: u32,
}

fn rl(bytes: &[u8], at: usize) -> Result<u32> {
    let b = bytes.get(at..at + 4).ok_or(Error::Truncated("the file header"))?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn span<'a>(bytes: &'a [u8], at: u32, len: usize, what: &'static str) -> Result<&'a [u8]> {
    let at = at as usize;
    bytes.get(at..at.checked_add(len).ok_or(Error::Truncated(what))?).ok_or(Error::Truncated(what))
}

/// The length of the preview at `at` (sentinel, RL size, the size's bytes, sentinel).
fn preview_len(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at..at + 16)? != PREVIEW_START {
        return None;
    }
    let size = usize::try_from(rl(bytes, at + 16).ok()?).ok()?;
    let len = 36usize.checked_add(size)?;
    (bytes.get(at + len - 16..at + len)? == PREVIEW_END).then_some(len)
}

pub(crate) fn read(bytes: &[u8], limits: &Limits) -> Result<Dwg> {
    let mut head = [0u8; 15];
    head.copy_from_slice(bytes.get(6..0x15).ok_or(Error::Truncated("the file header"))?);
    let n = rl(bytes, 0x15)? as usize;
    if n > MAX_RECORDS {
        return Err(corrupt(format!("{n} section locators")));
    }
    let mut records = Vec::with_capacity(n);
    for i in 0..n {
        let at = 0x19 + 9 * i;
        let rec = *bytes.get(at).ok_or(Error::Truncated("the file header"))?;
        records.push((rec, rl(bytes, at + 1)?, rl(bytes, at + 5)?));
    }
    let crc_at = 0x19 + 9 * n;
    let stored = u16::from_le_bytes([
        *bytes.get(crc_at).ok_or(Error::Truncated("the file header"))?,
        *bytes.get(crc_at + 1).ok_or(Error::Truncated("the file header"))?,
    ]);
    if let Some(xor) = header_crc_xor(n) {
        let computed = crc8(0, bytes.get(..crc_at).unwrap_or_default()) ^ xor;
        if stored != computed {
            return Err(Error::BadCrc { what: "file header", stored: u32::from(stored), computed: u32::from(computed) });
        }
    }
    if bytes.get(crc_at + 2..crc_at + 18) != Some(FILE_HEADER_END.as_slice()) {
        return Err(corrupt("the file header's end sentinel is missing"));
    }
    let header_end = crc_at + 18;

    let find = |rec: u8| records.iter().find(|r| r.0 == rec).copied();
    let (_, at, size) = find(0).ok_or_else(|| corrupt("no header variables section"))?;
    let (header, _) = frame::read(
        span(bytes, at, size as usize, "the header variables")?,
        &frame::HEADER_START,
        &frame::HEADER_END,
        false,
        false,
        "header variables",
    )?;
    let (_, at, size) = find(1).ok_or_else(|| corrupt("no classes section"))?;
    let (classes, _) =
        frame::read(span(bytes, at, size as usize, "the classes")?, &frame::CLASSES_START, &frame::CLASSES_END, false, false, "classes")?;
    let (_, map_at, _) = find(2).ok_or_else(|| corrupt("no object map"))?;
    let (entries, map_len) = objmap::read(bytes.get(map_at as usize..).ok_or(Error::Truncated("the object map"))?, limits.max_objects)?;

    // The objects, in the order they are stored.
    let mut placed: Vec<(usize, RawObject, usize)> = Vec::with_capacity(entries.len());
    for e in &entries {
        let at = usize::try_from(e.offset).map_err(|_| corrupt("an object address"))?;
        let (o, len) = objects::read(bytes, at, false, e.handle)?;
        placed.push((at, o, len));
    }
    placed.sort_by_key(|p| p.0);
    let objects_start = placed.first().map_or(map_at as usize, |p| p.0);
    let objects_end = placed.iter().map(|p| p.0 + p.2).max().unwrap_or(objects_start);
    // Objects are written one after another; a file whose records overlap or leave gaps comes
    // back packed.
    let objects: Vec<RawObject> = placed.into_iter().map(|p| p.1).collect();

    // The regions in file order: sections, objects, map, preview, second header.
    let mut spans: Vec<(usize, usize, Region)> = Vec::new();
    let mut sections = Vec::new();
    for &(rec, at, size) in &records {
        match rec {
            0 | 1 => spans.push((at as usize, size as usize, Region::Record(rec))),
            2 => spans.push((map_at as usize, map_len, Region::Record(2))),
            _ if size == 0 => {}
            _ => {
                sections.push(Section { name: record_name(rec), data: span(bytes, at, size as usize, "a section")?.to_vec() });
                spans.push((at as usize, size as usize, Region::Record(rec)));
            }
        }
    }
    if !objects.is_empty() {
        spans.push((objects_start, objects_end - objects_start, Region::Objects));
    }
    let preview_at = u32::from_le_bytes([head[7], head[8], head[9], head[10]]);
    if let Some(len) = preview_len(bytes, preview_at as usize) {
        sections.push(Section { name: PREVIEW.into(), data: span(bytes, preview_at, len, "the preview")?.to_vec() });
        spans.push((preview_at as usize, len, Region::Preview));
    }
    let after_map = map_at as usize + map_len;
    if bytes.get(after_map..after_map + 16) == Some(SECOND_START.as_slice()) {
        let window = bytes.get(after_map..(after_map + MAX_SECOND_HEADER).min(bytes.len())).unwrap_or_default();
        if let Some(end) = window.windows(16).position(|w| w == SECOND_END) {
            sections.push(Section { name: SECOND_HEADER.into(), data: window.get(..end + 16).unwrap_or_default().to_vec() });
            spans.push((after_map, end + 16, Region::SecondHeader));
        }
    }
    spans.sort_by_key(|s| s.0);
    // The object map always follows the objects when written.
    if let (Some(m), Some(o)) = (spans.iter().position(|s| s.2 == Region::Record(2)), spans.iter().position(|s| s.2 == Region::Objects))
        && m < o
    {
        let map = spans.remove(m);
        let o = spans.iter().position(|s| s.2 == Region::Objects).unwrap_or(spans.len());
        spans.insert(o + 1, map);
    }
    let mut regions = Vec::with_capacity(spans.len());
    let mut pos = header_end;
    for (at, len, region) in spans {
        let gap = if at >= pos { bytes.get(pos..at).unwrap_or_default().to_vec() } else { Vec::new() };
        regions.push((region, gap));
        pos = pos.max(at.saturating_add(len));
    }
    let tail = bytes.get(pos..).unwrap_or_default().to_vec();
    let layout = Layout { head, records, regions, tail, objects_start: objects_start as u32, preview_at };
    Ok(Dwg { version: Version::R2000, header, classes, objects, sections, layout: AnyLayout::R2000(layout) })
}

/// The preview's own addresses moved by `delta` (spec §14.2: each image's start is absolute).
fn move_preview(mut data: Vec<u8>, delta: i64) -> Vec<u8> {
    let Some(&count) = data.get(20) else { return data };
    for i in 0..usize::from(count) {
        let at = 21 + 9 * i + 1;
        let Some(b) = data.get(at..at + 4) else { break };
        let start = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if start == 0 {
            continue;
        }
        let moved = u32::try_from(i64::from(start) + delta).unwrap_or(start);
        if let Some(slot) = data.get_mut(at..at + 4) {
            slot.copy_from_slice(&moved.to_le_bytes());
        }
    }
    data
}

pub(crate) fn write(dwg: &Dwg, l: &Layout) -> Result<Vec<u8>> {
    let n = l.records.len();
    let xor = header_crc_xor(n).ok_or_else(|| Error::Unsupported(format!("writing {n} section locators")))?;
    let header_end = 0x19 + 9 * n + 18;
    let mut out = vec![0u8; header_end];
    let section = |name: &str| dwg.sections.iter().find(|s| s.name == name).map(|s| s.data.clone());
    let mut placed: Vec<(u8, u32, u32)> = Vec::new();
    let mut entries: Vec<objmap::Entry> = Vec::with_capacity(dwg.objects.len());
    let mut objects_start: Option<u32> = None;
    let mut preview_at = 0u32;
    let mut freespace_at: Option<usize> = None;
    let addr = |out: &Vec<u8>| u32::try_from(out.len()).map_err(|_| Error::TooLarge("a DWG file over 4 GB"));
    for (region, gap) in &l.regions {
        out.extend_from_slice(gap);
        let at = addr(&out)?;
        match *region {
            Region::Record(0) => out.extend(frame::write(&dwg.header, &frame::HEADER_START, &frame::HEADER_END)),
            Region::Record(1) => out.extend(frame::write(&dwg.classes, &frame::CLASSES_START, &frame::CLASSES_END)),
            Region::Record(2) => {
                entries.sort_by_key(|e| e.handle);
                out.extend(objmap::write(&entries));
            }
            Region::Record(rec) => {
                let Some(data) = section(&record_name(rec)) else { continue };
                if rec == 3 {
                    freespace_at = Some(out.len());
                }
                out.extend(data);
            }
            Region::Objects => {
                objects_start = Some(at);
                for o in &dwg.objects {
                    entries.push(objmap::Entry { handle: o.handle, offset: u64::from(addr(&out)?) });
                    objects::write(&mut out, o, false);
                }
            }
            Region::Preview => {
                let Some(data) = section(PREVIEW) else { continue };
                preview_at = at;
                out.extend(move_preview(data, i64::from(at) - i64::from(l.preview_at)));
            }
            Region::SecondHeader => {
                // Kept only while every address it repeats is where it was.
                let same = placed.iter().all(|p| l.records.contains(p)) && objects_start.is_none_or(|s| s == l.objects_start);
                if same && let Some(data) = section(SECOND_HEADER) {
                    out.extend(data);
                }
                continue;
            }
        }
        if let Region::Record(rec) = *region {
            placed.push((rec, at, addr(&out)? - at));
        }
    }
    out.extend_from_slice(&l.tail);

    // The objects' start in the free-space data (spec §21.1: offset 16), when it held the old one.
    if let (Some(at), Some(start)) = (freespace_at, objects_start)
        && l.objects_start != 0
        && let Some(slot) = out.get_mut(at + 16..at + 20)
        && slot == l.objects_start.to_le_bytes()
    {
        slot.copy_from_slice(&start.to_le_bytes());
    }

    // The file header.
    out[..6].copy_from_slice(Version::R2000.id().as_bytes());
    out[6..0x15].copy_from_slice(&l.head);
    let seeker = if dwg.section(PREVIEW).is_some() { preview_at } else { 0 };
    out[0x0D..0x11].copy_from_slice(&seeker.to_le_bytes());
    out[0x15..0x19].copy_from_slice(&(n as u32).to_le_bytes());
    for (i, &(rec, old_at, old_size)) in l.records.iter().enumerate() {
        let (at, size) = placed.iter().find(|p| p.0 == rec).map_or((old_at, old_size), |p| (p.1, p.2));
        let (at, size) = if placed.iter().any(|p| p.0 == rec) || (old_at == 0 && old_size == 0) { (at, size) } else { (0, 0) };
        let o = 0x19 + 9 * i;
        out[o] = rec;
        out[o + 1..o + 5].copy_from_slice(&at.to_le_bytes());
        out[o + 5..o + 9].copy_from_slice(&size.to_le_bytes());
    }
    let crc_at = 0x19 + 9 * n;
    let crc = crc8(0, &out[..crc_at]) ^ xor;
    out[crc_at..crc_at + 2].copy_from_slice(&crc.to_le_bytes());
    out[crc_at + 2..crc_at + 18].copy_from_slice(&FILE_HEADER_END);
    Ok(out)
}
