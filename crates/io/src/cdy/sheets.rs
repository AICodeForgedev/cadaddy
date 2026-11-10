//! A print set's sheets as sections of their own (Q6418): the `SHTI` sheet index and the model
//! space chunks it files under each sheet.
//!
//! A drawing that holds a print set (its [`SheetStore::sheets`] lists the sheets) is written
//! with an `SHTI` chunk: for each sheet, its number and place, and which of the file's `ENTS`
//! chunks hold only that sheet's objects (by their place among the index's `ENTS` entries), with
//! each chunk's object count and largest handle. The chunks themselves are ordinary `ENTS`
//! chunks, so a reader that doesn't know `SHTI` skips it and reads every object as before.
//!
//! An object belongs to a sheet when it lies inside the sheet's rectangle and carries no
//! extended data: a model element (a wall, a device, a room…) always carries its properties
//! there, so the building model is never filed under a sheet. A chunk is filed under a sheet only
//! when all its objects are; any other chunk is read on every open.
//!
//! [`read_parked`](super::read_parked) leaves the sheets' chunks as stored ([`StoredChunk`]) in
//! the drawing's [`SheetStore::parked`]; [`decode`] turns one into objects when it is wanted and
//! [`encode`] stores objects again. Saving writes a parked chunk, and a decoded one not changed
//! since, as it was stored, byte for byte.

use std::collections::HashMap;
use std::sync::Arc;

use cadaddy_doc::sheetstore::{SheetRect, StoredChunk};
use cadaddy_doc::{Drawing, Entity, entity_bounds};
use cadaddy_geom::{Bounds2, Vec2};
use serde::{Deserialize, Serialize};

use super::container::{self, CHUNK_HEADER_LEN, FLAG_DEFLATE, IndexEntry, MAX_RAW};
use super::model;

/// The sheet index's chunk type.
pub const TAG: [u8; 4] = *b"SHTI";
/// The sheet index's schema.
pub const VERSION: u16 = 1;
/// Most sheets an index lists (hostile files).
const MAX_SHEETS: usize = 10_000;
/// Most chunks it files (as many as a file may hold).
const MAX_FILED: usize = container::MAX_CHUNKS;

/// One sheet in the index.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(super) struct IndexSheet {
    pub number: String,
    pub rect: [f64; 4],
    /// `[place among the ENTS entries, objects, largest handle]` of each of its chunks.
    #[serde(default)]
    pub chunks: Vec<[u64; 3]>,
}

/// The `SHTI` chunk's JSON.
#[derive(Serialize, Deserialize, Debug, Default)]
pub(super) struct SheetIndex {
    /// The `ENTS` entries the file's index had when this was written: an index that doesn't
    /// match the file is not used.
    pub ents: u64,
    pub sheets: Vec<IndexSheet>,
}

pub(super) fn rect_of(r: &[f64; 4]) -> Option<Bounds2> {
    let b = Bounds2::new(Vec2::new(r[0], r[1]), Vec2::new(r[2], r[3]));
    (b.min.is_finite() && b.max.is_finite() && b.width() > 0.0 && b.height() > 0.0).then_some(b)
}

fn rect_array(b: &Bounds2) -> [f64; 4] {
    [b.min.x, b.min.y, b.max.x, b.max.y]
}

/// The sheet of `sheets` an object lies on, if it is a sheet's (see the module).
fn sheet_of(d: &Drawing, sheets: &[(Bounds2, f64)], e: &Entity) -> Option<u32> {
    if !e.common.xdata.is_empty() {
        return None;
    }
    let b = entity_bounds(d, e, 0);
    if b.is_empty() || !b.min.is_finite() || !b.max.is_finite() {
        return None;
    }
    sheets.iter().position(|(r, tol)| r.expand(*tol).contains_box(&b)).and_then(|i| u32::try_from(i).ok())
}

/// The sheets' rectangles with the slack an object may stick out by (a thousandth of the sheet).
pub fn slack(sheets: &[SheetRect]) -> Vec<(Bounds2, f64)> {
    sheets.iter().take(MAX_SHEETS).map(|s| (s.rect, 1e-3 * (s.rect.width() + s.rect.height()))).collect()
}

/// Most pieces the writer splits one object chunk into at sheet boundaries; a chunk mixing more
/// is written whole (and read on every open).
pub const MAX_PIECES: usize = 8;

/// `items` as runs of consecutive objects on the same sheet (or on none): `(sheet, from, to)`.
pub fn runs(d: &Drawing, sheets: &[(Bounds2, f64)], items: &[Arc<Entity>]) -> Vec<(Option<u32>, usize, usize)> {
    let mut out: Vec<(Option<u32>, usize, usize)> = Vec::new();
    for (i, e) in items.iter().enumerate() {
        let k = if sheets.is_empty() { None } else { sheet_of(d, sheets, e) };
        match out.last_mut() {
            Some((last, _, to)) if *last == k => *to = i + 1,
            _ => out.push((k, i, i + 1)),
        }
    }
    out
}

/// How the writer stores one object chunk: whole, or (a chunk holding the end of one sheet and
/// the start of the next) as its runs, each a chunk of its own filed under its sheet, in order.
pub fn pieces(d: &Drawing, sheets: &[(Bounds2, f64)], items: &[Arc<Entity>]) -> Vec<(Option<u32>, usize, usize)> {
    let r = runs(d, sheets, items);
    if r.len() > MAX_PIECES || (r.len() > 1 && r.iter().all(|(k, _, _)| k.is_none())) {
        return vec![(None, 0, items.len())];
    }
    r
}

/// The sheet all of `items` lie on, if they all lie on one.
pub fn chunk_sheet(d: &Drawing, sheets: &[(Bounds2, f64)], items: &[Arc<Entity>]) -> Option<u32> {
    if sheets.is_empty() {
        return None;
    }
    let first = sheet_of(d, sheets, items.first()?)?;
    items.iter().skip(1).all(|e| sheet_of(d, sheets, e) == Some(first)).then_some(first)
}

/// The sheet index's JSON for the chunks written: `filed` is, per `ENTS` chunk in file order,
/// the sheet (by its place in `sheets`) it holds only objects of, its object count and largest
/// handle. Sheets without chunks are listed too.
pub(super) fn index_json(sheets: &[SheetRect], filed: &[(Option<u32>, usize, u64)]) -> Vec<u8> {
    let mut out: Vec<IndexSheet> =
        sheets.iter().take(MAX_SHEETS).map(|s| IndexSheet { number: s.number.clone(), rect: rect_array(&s.rect), chunks: Vec::new() }).collect();
    for (i, (sheet, objects, max)) in filed.iter().enumerate() {
        if let Some(s) = sheet.and_then(|k| out.get_mut(k as usize)) {
            s.chunks.push([i as u64, *objects as u64, *max]);
        }
    }
    let index = SheetIndex { ents: filed.len() as u64, sheets: out };
    serde_json::to_vec(&index).unwrap_or_default()
}

/// Where a file's sheets' chunks are: per `ENTS` entry place, the sheet (its place in
/// [`SheetIndex::sheets`]), objects and largest handle. `None` when the file has no usable index
/// (none, damaged, or not matching the file's chunks).
pub(super) struct Filing {
    pub sheets: Vec<(String, Bounds2)>,
    pub by_place: HashMap<usize, (usize, usize, u64)>,
}

pub(super) fn filing(bytes: &[u8], entries: &[IndexEntry]) -> Option<Filing> {
    let e = entries.iter().rev().find(|e| e.tag == TAG && e.version <= VERSION)?;
    let container::Read::Ok(c) = container::read_entry(bytes, e) else { return None };
    let index: SheetIndex = serde_json::from_slice(&c.data).ok()?;
    let ents = entries.iter().filter(|e| &e.tag == b"ENTS").count();
    if index.ents != ents as u64 || index.sheets.len() > MAX_SHEETS {
        return None;
    }
    let mut sheets = Vec::with_capacity(index.sheets.len());
    let mut by_place = HashMap::new();
    for (k, s) in index.sheets.iter().enumerate() {
        let rect = rect_of(&s.rect)?;
        sheets.push((s.number.chars().take(200).collect::<String>(), rect));
        for [place, objects, max] in s.chunks.iter().take(MAX_FILED) {
            let place = usize::try_from(*place).ok().filter(|p| *p < ents)?;
            let objects = usize::try_from(*objects).ok().filter(|n| *n <= model::ENTITY_CHUNK)?;
            // A chunk filed twice: the index isn't this file's.
            if by_place.insert(place, (k, objects, *max)).is_some() {
                return None;
            }
        }
    }
    Some(Filing { sheets, by_place })
}

/// The `ENTS` chunk at `e` as stored, when it is sound (its checksum holds); `None` when it
/// isn't (it is read the ordinary way then, which salvages what it can).
pub(super) fn stored_at(bytes: &[u8], e: &IndexEntry, objects: usize, max_handle: u64) -> Option<StoredChunk> {
    let at = usize::try_from(e.offset).ok()?;
    let head = bytes.get(at..at.checked_add(CHUNK_HEADER_LEN)?)?;
    let u16_at = |i: usize| Some(u16::from_le_bytes(head.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(head.get(i..i + 4)?.try_into().ok()?));
    let u64_at = |i: usize| Some(u64::from_le_bytes(head.get(i..i + 8)?.try_into().ok()?));
    if head.get(..4)? != b"ENTS" {
        return None;
    }
    let (version, flags, len, crc, raw_len) = (u16_at(4)?, u16_at(6)?, u64_at(8)?, u32_at(16)?, u64_at(20)?);
    if version > model::max_version(b"ENTS") || version != e.version || len != e.len || raw_len > MAX_RAW {
        return None;
    }
    let start = at.checked_add(CHUNK_HEADER_LEN)?;
    let stored = bytes.get(start..start.checked_add(usize::try_from(len).ok()?)?)?;
    if crc32fast::hash(stored) != crc {
        return None;
    }
    let deflated = flags & FLAG_DEFLATE != 0;
    if !deflated && len != raw_len {
        return None;
    }
    Some(StoredChunk { version, raw_len, deflated, stored: stored.to_vec(), objects, max_handle })
}

/// The objects of a stored chunk, with notes about the ones that couldn't be read. An error
/// when the chunk doesn't inflate or decode at all.
pub fn decode(c: &StoredChunk) -> Result<(Vec<Entity>, Vec<String>), String> {
    let raw_len = usize::try_from(c.raw_len.min(MAX_RAW)).map_err(|_| "a sheet's part is too large".to_string())?;
    let data = if c.deflated {
        match miniz_oxide::inflate::decompress_to_vec_with_limit(&c.stored, raw_len) {
            Ok(d) if d.len() == raw_len => d,
            _ => return Err("a sheet's part doesn't decompress".into()),
        }
    } else {
        c.stored.clone()
    };
    model::entity_list(&data, c.version, &model::Meter::default()).ok_or_else(|| "a sheet's part couldn't be decoded".to_string())
}

/// `items` stored as an `ENTS` chunk stores them.
pub fn encode(items: &[Arc<Entity>]) -> Result<StoredChunk, String> {
    let (version, raw) = model::entities(items)?;
    let p = container::pack(&raw, true);
    let max_handle = items.iter().map(|e| e.handle.0).max().unwrap_or(0);
    Ok(StoredChunk { version, raw_len: raw.len() as u64, deflated: p.deflated, stored: p.stored, objects: items.len(), max_handle })
}
