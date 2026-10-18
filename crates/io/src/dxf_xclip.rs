//! XCLIP boundaries: the `SPATIAL_FILTER` objects of block references, read into
//! [`Insert::clip`](cadaddy_doc::Insert) and written back.
//!
//! A clipped reference owns an extension dictionary holding `ACAD_FILTER`, which holds `SPATIAL`,
//! the `SPATIAL_FILTER`: its boundary (two points for a rectangle, else a polygon), the boundary's
//! origin, whether it is shown (71) and the inverse of the reference's transform when it was
//! clipped. Files differ in whether the boundary points are already in block coordinates or
//! still need that inverse transform; the reading that overlaps the block's contents is used.
//! An inverted clip is flagged in an `ACAD_XREC_ROUNDTRIP` record (`ACAD_INVERTEDCLIP_ROUNDTRIP`).

use std::collections::HashMap;

use cadaddy_doc::{Drawing, EntityKind, Handle, XClip};
use cadaddy_dxf::{Section, Tag, records};
use cadaddy_geom::{Bounds2, Vec2};

/// Most boundary points read for one clip (hostile files).
const MAX_POINTS: usize = 10_000;

struct Filter {
    owner: String,
    points: Vec<Vec2>,
    origin: Vec2,
    enabled: bool,
    /// The inverse block transform: x' = m0·x + m1·y + m3, y' = m4·x + m5·y + m7.
    inverse: Option<[f64; 12]>,
}

fn key(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

/// A record's owner: its first 330 outside a `{ACAD_REACTORS ... }` group.
fn owner(tags: &[Tag]) -> Option<String> {
    let mut in_group = false;
    for t in tags {
        match t.code {
            102 => in_group = t.str().starts_with('{'),
            330 if !in_group => return Some(key(&t.str())),
            100 => break,
            _ => {}
        }
    }
    tags.iter().find(|t| t.code == 330).map(|t| key(&t.str()))
}

fn parse_filter(tags: &[Tag]) -> Option<Filter> {
    let start = tags.iter().position(|t| t.code == 100 && t.str() == "AcDbSpatialFilter")?;
    let body = tags.get(start..)?;
    let mut points = Vec::new();
    let mut x: Option<f64> = None;
    let (mut ox, mut oy) = (0.0, 0.0);
    let mut enabled = true;
    let mut after_flags = false;
    let mut m: Vec<f64> = Vec::new();
    for t in body {
        match t.code {
            10 => x = Some(t.f64()),
            20 => {
                if let Some(px) = x.take()
                    && points.len() < MAX_POINTS
                {
                    points.push(Vec2::new(px, t.f64()));
                }
            }
            11 => ox = t.f64(),
            21 => oy = t.f64(),
            71 => enabled = t.i64() != 0,
            // The front (40) and back (41) clip distances come before 73's flag is passed; the
            // matrices' 40s after it.
            73 => after_flags = true,
            40 if after_flags => m.push(t.f64()),
            _ => {}
        }
    }
    let inverse = <[f64; 12]>::try_from(m.get(..12)?).ok().filter(|v| v.iter().all(|f| f.is_finite()));
    Some(Filter { owner: owner(tags)?, points, origin: Vec2::new(ox, oy), enabled, inverse })
}

fn apply(m: &[f64; 12], p: Vec2) -> Vec2 {
    Vec2::new(m[0] * p.x + m[1] * p.y + m[3], m[4] * p.x + m[5] * p.y + m[7])
}

/// The boundary as a polygon in the boundary's own coordinates (a rectangle from its corners).
fn polygon(f: &Filter) -> Option<Vec<Vec2>> {
    let pts: Vec<Vec2> = f.points.iter().map(|p| *p + f.origin).filter(|p| p.is_finite()).collect();
    match pts.as_slice() {
        [a, b] => Some(vec![*a, Vec2::new(b.x, a.y), *b, Vec2::new(a.x, b.y)]),
        p if p.len() >= 3 => Some(pts),
        _ => None,
    }
}

/// How much of the block's contents a boundary's box covers (0 when none).
fn overlap(poly: &[Vec2], content: Bounds2) -> f64 {
    let b = Bounds2::from_points(poly.iter().copied());
    if b.is_empty() || content.is_empty() {
        return 0.0;
    }
    let w = (b.max.x.min(content.max.x) - b.min.x.max(content.min.x)).max(0.0);
    let h = (b.max.y.min(content.max.y) - b.min.y.max(content.min.y)).max(0.0);
    w * h
}

/// Read the clips of the file's block references into their [`Insert::clip`].
pub(crate) fn resolve(secs: &[Section], d: &mut Drawing) {
    let Some(objects) = secs.iter().find(|s| s.name == "OBJECTS") else { return };
    let recs = records(&objects.tags);
    if !recs.iter().any(|(k, _)| k == "SPATIAL_FILTER") {
        return;
    }
    let mut owners: HashMap<String, String> = HashMap::new();
    let mut filters: Vec<(String, Filter)> = Vec::new();
    let mut inverted_records: Vec<String> = Vec::new();
    for (kind, tags) in &recs {
        let Some(h) = tags.iter().find(|t| t.code == 5).map(|t| key(&t.str())) else { continue };
        if let Some(o) = owner(tags) {
            owners.insert(h.clone(), o);
        }
        match kind.as_str() {
            "SPATIAL_FILTER" => {
                if let Some(f) = parse_filter(tags) {
                    filters.push((h, f));
                }
            }
            "XRECORD" => {
                let flagged = tags.windows(2).any(|w| {
                    w.first().is_some_and(|a| a.str() == "ACAD_INVERTEDCLIP_ROUNDTRIP") && w.get(1).is_some_and(|b| b.code == 290 && b.i64() != 0)
                });
                if flagged {
                    inverted_records.push(h);
                }
            }
            _ => {}
        }
    }
    // The filter an inverted-clip record belongs to: up its owners (record → dictionary → filter).
    let up = |mut h: String, levels: usize| {
        let mut chain = Vec::new();
        for _ in 0..levels {
            match owners.get(&h) {
                Some(o) => {
                    chain.push(o.clone());
                    h = o.clone();
                }
                None => break,
            }
        }
        chain
    };
    let inverted: Vec<String> = inverted_records.into_iter().flat_map(|r| up(r, 3)).collect();
    for (fh, f) in filters {
        let Some(poly) = polygon(&f) else { continue };
        // The reference: filter → SPATIAL dictionary → ACAD_FILTER dictionary → its extension
        // dictionary → the INSERT.
        let insert = std::iter::once(f.owner.clone())
            .chain(up(f.owner.clone(), 4))
            .filter_map(|h| Handle::parse_hex(&h))
            .find(|h| d.entity(*h).is_some_and(|e| matches!(e.kind, EntityKind::Insert(_))) || block_entity(d, *h).is_some());
        let Some(ih) = insert else { continue };
        let ins = d.entity(ih).map(|e| (**e).clone()).or_else(|| block_entity(d, ih));
        let Some(EntityKind::Insert(ins)) = ins.map(|e| e.kind.clone()) else { continue };
        // Block coordinates: the points as given, or through the stored inverse transform.
        let content = d
            .block(&ins.block)
            .map_or(Bounds2::EMPTY, |b| b.entities.iter().map(|e| cadaddy_doc::entity_bounds(d, e, 1)).fold(Bounds2::EMPTY, |a, b| a.union(&b)));
        let mapped: Option<Vec<Vec2>> = f.inverse.map(|m| poly.iter().map(|p| apply(&m, *p)).collect());
        let boundary = match mapped {
            Some(m) if overlap(&m, content) > overlap(&poly, content) => m,
            _ => poly,
        };
        let clip = XClip { boundary, enabled: f.enabled, inverted: inverted.contains(&fh) };
        set_clip(d, ih, clip);
    }
}

/// An entity inside a block definition, by handle.
fn block_entity(d: &Drawing, h: Handle) -> Option<cadaddy_doc::Entity> {
    d.blocks.values().find_map(|b| b.entities.iter().find(|e| e.handle == h).map(|e| (**e).clone()))
}

fn set_clip(d: &mut Drawing, h: Handle, clip: XClip) {
    let put = |e: &mut cadaddy_doc::Entity| {
        if let EntityKind::Insert(i) = &mut e.kind {
            i.clip = Some(clip.clone());
        }
    };
    if d.entity(h).is_some() {
        let _ = d.modify_entity(h, put);
        return;
    }
    for b in d.blocks.values_mut() {
        if b.entities.iter().any(|e| e.handle == h) {
            std::sync::Arc::make_mut(b).entities.modify(h, put);
            return;
        }
    }
}
