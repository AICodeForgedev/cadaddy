//! Plans imported from PDF to a building spec. A vector PDF keeps no layer names (everything is
//! on PDF_Geometry or one layer per pen), so `plan.classify` reads the drawing by what it is:
//!
//! - **scale**: a scale note (`SCALE: 1/4" = 1'-0"`), else a dimension (a length text such as
//!   `40'-0"` along a line: its value over the line's length), when the sheet came in at paper
//!   size;
//! - **walls**: parallel faces a wall's thickness apart, the heaviest line weight of the sheet,
//!   and wall poché (hatches and fills a wall's thickness wide);
//! - **doors**: swing arcs (60°–100°, 16"–54") and their leaves;
//! - **room names**: names (not numbers) inside a closed area of a room's size;
//! - **sheet**: the title block's sheet number and title (`plan.sheets`).
//!
//! A page that is only pictures (scans) is reported as unsupported. `apply: true` scales the
//! drawing to real inches and moves what it found onto NCS layers (A-WALL, A-DOOR,
//! A-AREA-IDEN), where `bspec.draft` reads them; `draft: true` then drafts and keeps the spec.

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Lineweight};
use cadaddy_geom::{Mat3, Vec2};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;

use super::bspec::{room_tags, same_outline};
use super::perceive::geometry::{Limits, openings, pairs, room_number, segments};
use super::perceive::{feet_inches, texts, units_per_inch};
use super::*;
use crate::{Result, Session};

/// Objects looked at, at most (hostile drawings).
const MAX_OBJECTS: usize = 200_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("plan.classify", "Classify Plan", run).params(
        "{apply?: bool (scale to real inches and move walls, doors and room names onto A-WALL, A-DOOR, A-AREA-IDEN), draft?: bool (then bspec.draft and keep the spec)} → {raster_only, scale: {factor, source}, walls: {objects, pairs, lineweight_mm?}, doors, rooms: [name], sheet: {number?, title?}, applied, spec?}",
    )]
}

/// The lineweight an object draws with (its own, else its layer's), hundredths of a millimetre.
fn weight(d: &Drawing, e: &Entity) -> u16 {
    match e.common.lineweight {
        Lineweight::Mm100(w) => w,
        _ => match d.layer(&e.common.layer).map(|l| l.lineweight) {
            Some(Lineweight::Mm100(w)) => w,
            _ => 0,
        },
    }
}

/// The drawing's scale (drawing units per real inch… as a factor to multiply it by), and where
/// it came from: the scale note, a dimension, or none (already real size).
fn scale(d: &Drawing, ents: &[&Arc<Entity>]) -> (f64, String) {
    let ext = d.extents(&cadaddy_doc::Space::Model);
    // A sheet at paper size is under about 5 feet across; a plan at real size is far larger.
    let paper = !ext.is_empty() && ext.width().max(ext.height()) <= 60.0 * units_per_inch(d);
    if !paper {
        return (1.0, "real size".into());
    }
    let notes = texts(ents);
    if let Some((t, f)) = notes.iter().find_map(|(_, t, _, _)| {
        let up = t.to_ascii_uppercase();
        (up.contains("SCALE") || up.contains('=')).then(|| scale_note(t).map(|f| (t.clone(), f))).flatten()
    }) {
        return (f, format!("scale note {t}"));
    }
    // A dimension: a length text beside a line, its value over the line's length.
    let segs = segments(ents);
    let mut ratios: Vec<f64> = Vec::new();
    for (_, t, at, h) in &notes {
        let clean: String = t.chars().filter(|c| !c.is_whitespace()).collect();
        if clean.chars().any(|c| c.is_ascii_alphabetic()) || !clean.contains('\'') {
            continue;
        }
        let Some(value) = feet_inches(&clean) else { continue };
        let near = segs
            .iter()
            .filter(|sg| {
                let mid = (sg.a + sg.b) * 0.5;
                mid.dist(*at) <= 3.0 * h.max(1e-9) + sg.a.dist(sg.b) * 0.5 && dist(*at, sg.a, sg.b) <= 3.0 * h.max(1e-9)
            })
            .max_by(|a, b| a.a.dist(a.b).total_cmp(&b.a.dist(b.b)));
        if let Some(sg) = near {
            let len = sg.a.dist(sg.b);
            if len > 1e-9 {
                ratios.push(value / len);
            }
        }
    }
    ratios.sort_by(f64::total_cmp);
    if let Some(m) = ratios.get(ratios.len() / 2).copied() {
        let agree = ratios.iter().filter(|r| (*r / m - 1.0).abs() < 0.02).count();
        if agree * 2 > ratios.len() {
            return (m, format!("{agree} dimension(s)"));
        }
    }
    (1.0, "none found (the sheet stays at paper size)".into())
}

/// Real inches per paper inch from a scale note: `1/4" = 1'-0"`, `1" = 20'`, `1:100`.
pub(crate) fn scale_note(text: &str) -> Option<f64> {
    let t = text.replace(['\u{2019}', '\u{2032}'], "'").replace(['\u{201D}', '\u{2033}'], "\"").to_ascii_uppercase();
    let t = t.split_once("SCALE").map_or(t.as_str(), |(_, r)| r).trim_start_matches([':', ' ']).trim().to_string();
    let num = |s: &str| -> Option<f64> {
        let s = s.trim().trim_end_matches('"').trim();
        match s.split_once('/') {
            Some((a, b)) => {
                let (whole, a) = match a.trim().rsplit_once(' ') {
                    Some((w, a)) => (w.trim().parse::<f64>().ok()?, a),
                    None => (0.0, a),
                };
                Some(whole + a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok().filter(|b| *b != 0.0)?)
            }
            None => s.parse::<f64>().ok(),
        }
    };
    if let Some((lhs, rhs)) = t.split_once('=') {
        let paper = num(lhs)?;
        let (ft, inch) = match rhs.trim().split_once('\'') {
            Some((f, rest)) => {
                let i = rest.trim().trim_start_matches('-').trim();
                (f.trim().parse::<f64>().ok()?, if i.trim_end_matches('"').trim().is_empty() { 0.0 } else { num(i)? })
            }
            None => (0.0, num(rhs)?),
        };
        let real = ft * 12.0 + inch;
        return (paper > 0.0 && real > 0.0).then(|| real / paper).filter(|k| k.is_finite() && (0.001..=1e6).contains(k));
    }
    let (a, b) = t.split_once(':')?;
    let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
    (a > 0.0 && b > 0.0).then(|| b / a)
}

/// The circle through three points: centre and radius.
fn circle3(a: Vec2, b: Vec2, c: Vec2) -> Option<(Vec2, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let sq = |p: Vec2| p.x * p.x + p.y * p.y;
    let ux = (sq(a) * (b.y - c.y) + sq(b) * (c.y - a.y) + sq(c) * (a.y - b.y)) / d;
    let uy = (sq(a) * (c.x - b.x) + sq(b) * (a.x - c.x) + sq(c) * (b.x - a.x)) / d;
    let o = Vec2::new(ux, uy);
    let r = o.dist(a);
    (r.is_finite() && r > 0.0).then_some((o, r))
}

fn dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + d * t)
}

/// Wall pairs bucketed by their box grown by their reach (half the thickness plus `slack`):
/// the pairs a face could be near.
struct PairGrid {
    min: Vec2,
    cell: f64,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<usize>>,
}

impl PairGrid {
    /// Cells per side, at most (a hostile plan's pairs can't make a huge grid).
    const MAX_SIDE: usize = 256;

    fn new(pairs: &[super::perceive::geometry::Pair], slack: f64) -> PairGrid {
        let grown = |p: &super::perceive::geometry::Pair| {
            let r = p.thickness / 2.0 + slack;
            (Vec2::new(p.c0.x.min(p.c1.x) - r, p.c0.y.min(p.c1.y) - r), Vec2::new(p.c0.x.max(p.c1.x) + r, p.c0.y.max(p.c1.y) + r))
        };
        let boxes: Vec<(Vec2, Vec2)> = pairs.iter().map(grown).collect();
        let lo = boxes.iter().fold(Vec2::new(f64::INFINITY, f64::INFINITY), |m, b| Vec2::new(m.x.min(b.0.x), m.y.min(b.0.y)));
        let hi = boxes.iter().fold(Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY), |m, b| Vec2::new(m.x.max(b.1.x), m.y.max(b.1.y)));
        let span = (hi.x - lo.x).max(hi.y - lo.y);
        let side = ((pairs.len() as f64).sqrt().ceil() as usize).clamp(1, Self::MAX_SIDE);
        if !(span.is_finite() && span > 0.0) {
            return PairGrid { min: Vec2::new(0.0, 0.0), cell: 1.0, cols: 1, rows: 1, cells: vec![(0..pairs.len()).collect()] };
        }
        let cell = span / side as f64;
        let mut g = PairGrid { min: lo, cell, cols: side, rows: side, cells: vec![Vec::new(); side * side] };
        for (i, (a, b)) in boxes.iter().enumerate() {
            let (c0, r0, c1, r1) = g.range(*a, *b);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    if let Some(v) = g.cells.get_mut(r * g.cols + c) {
                        v.push(i);
                    }
                }
            }
        }
        g
    }

    /// The cells (first and last column and row) a box covers, clamped to the grid.
    fn range(&self, a: Vec2, b: Vec2) -> (usize, usize, usize, usize) {
        let at = |v: f64, lo: f64, n: usize| {
            let i = ((v - lo) / self.cell).floor();
            if i.is_finite() { (i.max(0.0) as usize).min(n - 1) } else { 0 }
        };
        (
            at(a.x.min(b.x), self.min.x, self.cols),
            at(a.y.min(b.y), self.min.y, self.rows),
            at(a.x.max(b.x), self.min.x, self.cols),
            at(a.y.max(b.y), self.min.y, self.rows),
        )
    }

    /// The pairs that could be near the face `a`–`b` (each once).
    fn near(&self, a: Vec2, b: Vec2) -> impl Iterator<Item = usize> + '_ {
        let (c0, r0, c1, r1) = self.range(a, b);
        let mut seen: Vec<usize> =
            (r0..=r1).flat_map(|r| (c0..=c1).map(move |c| r * self.cols + c)).filter_map(|i| self.cells.get(i)).flatten().copied().collect();
        seen.sort_unstable();
        seen.dedup();
        seen.into_iter()
    }
}

/// The layer of the searchable (hidden) copy of each text in a PDF from our exporter
/// (`cadaddy_io::pdf_read::HIDDEN_TEXT_LAYER`).
const HIDDEN_TEXT_LAYER: &str = "PDF-HIDDEN-TEXT";

/// The hidden copy of the text `text` (its tallest instance): a different text on the hidden
/// text layer starting within half its height of it.
fn hidden_copy(ents: &[&Arc<Entity>], text: &str) -> Option<String> {
    let (at, h) = texts(ents).into_iter().filter(|(_, t, _, _)| t == text).map(|(_, _, at, h)| (at, h)).max_by(|a, b| a.1.total_cmp(&b.1))?;
    let hidden: Vec<&Arc<Entity>> = ents.iter().filter(|e| e.common.layer == HIDDEN_TEXT_LAYER).copied().collect();
    texts(&hidden)
        .into_iter()
        .filter(|(_, t, p, _)| !t.is_empty() && t != text && p.dist(at) <= 0.5 * h)
        .min_by(|a, b| a.2.dist(at).total_cmp(&b.2.dist(at)))
        .map(|(_, t, _, _)| t)
}

/// Words a title block prints as field labels, not as the sheet's title.
const TITLE_BLOCK_LABELS: &[&str] =
    &["SHEET", "NO", "NO.", "NUMBER", "TITLE", "DATE", "SCALE", "DRAWN", "CHECKED", "BY", "PROJECT", "REV", "REVISION", "JOB", "DWG"];

/// The sheet title printed beside the sheet number `number` (plain texts, as a PDF title block
/// comes in): the nearest text within eight number heights that reads as words, is not a field
/// label, a scale or a dimension.
fn title_near(ents: &[&Arc<Entity>], number: &str) -> Option<String> {
    let all = texts(ents);
    let (at, h) = all.iter().filter(|(_, t, _, _)| t == number).map(|(_, _, at, h)| (*at, *h)).max_by(|a, b| a.1.total_cmp(&b.1))?;
    let reach = 8.0 * h.max(1e-9);
    all.iter()
        .filter(|(_, t, p, _)| {
            t != number
                && p.dist(at) <= reach
                && t.chars().count() <= 60
                && t.chars().filter(|c| c.is_alphabetic()).count() >= 3
                && !t.contains([':', '=', '\'', '"'])
                && !t.split_whitespace().all(|w| TITLE_BLOCK_LABELS.contains(&w.to_ascii_uppercase().as_str()))
        })
        .min_by(|a, b| a.2.dist(at).total_cmp(&b.2.dist(at)).then(b.3.total_cmp(&a.3)))
        .map(|(_, t, _, _)| t.clone())
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    // The drawing as it stands (a shared copy: the session is used below).
    let doc = Arc::clone(&s.state()?.doc);
    let d: &Drawing = &doc;
    let ents: Vec<&Arc<Entity>> = d.model.iter().filter(|e| super::wallgen::made_as(e).is_none()).take(MAX_OBJECTS).collect();
    // Pictures only: a scan, not linework.
    let images = ents.iter().filter(|e| matches!(e.kind, EntityKind::Image(_))).count();
    let vectors = segments(&ents).len();
    if images > 0 && vectors < 50 {
        return Ok(json!({
            "status": "skipped",
            "raster_only": true,
            "message": "This page is a picture (a scan or a rendering), not linework: plans can only be read from vector PDFs.",
            "applied": false,
        }));
    }
    if ents.is_empty() || vectors == 0 {
        return Ok(json!({
            "status": "skipped",
            "message": "0 vector inputs to classify — the drawing is empty or has no linework. Try: open or convert a PDF first.",
            "applied": false,
        }));
    }
    let (factor, source) = scale(d, &ents);
    // Sizes in the drawing's units as it stands (before any scaling).
    let k = units_per_inch(d) / factor;
    let lim = Limits { min_thick: 2.5 * k, max_thick: 20.0 * k, min_len: 12.0 * k };
    // Walls: faces in pairs, the heaviest pen, poché.
    let segs_by: Vec<(Handle, Vec<super::perceive::geometry::Seg>)> = ents.iter().map(|e| (e.handle, segments(&[*e]))).collect();
    let all_segs: Vec<super::perceive::geometry::Seg> = segs_by.iter().flat_map(|(_, s)| s.iter().cloned()).collect();
    let found = pairs(d, &all_segs, &lim);
    // A face of a pair: its middle on the pair's centreline (within half the thickness), or the
    // pair's middle on it (a long outer face beside several rooms).
    // Only the pairs whose box (grown by their reach) meets the face's box can pass: a grid of
    // them instead of every pair for every face (a guest floor has thousands of each).
    let grid = PairGrid::new(&found, 0.25 * k);
    let near_pair = |sg: &super::perceive::geometry::Seg| {
        grid.near(sg.a, sg.b).any(|i| {
            found.get(i).is_some_and(|pr| {
                let reach = pr.thickness / 2.0 + 0.25 * k;
                dist((sg.a + sg.b) * 0.5, pr.c0, pr.c1) <= reach || dist((pr.c0 + pr.c1) * 0.5, sg.a, sg.b) <= reach
            })
        })
    };
    let weights: Vec<u16> = ents.iter().map(|e| weight(d, e)).filter(|w| *w > 0).collect();
    let heaviest = weights.iter().copied().max();
    let lightest = weights.iter().copied().min();
    let heavy = |e: &Entity| heaviest.is_some_and(|h| Some(h) != lightest && weight(d, e) == h);
    let mut walls: HashSet<Handle> = HashSet::new();
    for (e, (_, segs)) in ents.iter().zip(&segs_by) {
        let paired = !segs.is_empty() && segs.iter().any(&near_pair);
        let poche = match &e.kind {
            EntityKind::Hatch(_) | EntityKind::Solid(_) => {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                !b.is_empty() && (lim.min_thick..=lim.max_thick).contains(&b.width().min(b.height()))
            }
            _ => false,
        };
        let score = 2 * u8::from(paired) + u8::from(heavy(e) && !segs.is_empty()) + 2 * u8::from(poche);
        // Where pens differ, line pairs in the lightest pen are a duct's sides, a stair's treads,
        // casework (Q251): walls are drawn heavier, or filled.
        let light = heaviest != lightest && lightest.is_some_and(|l| weight(d, e) == l);
        if score >= 2 && (poche || !light) {
            walls.insert(e.handle);
        }
    }
    // Doors: swing arcs and their leaves.
    let ops = openings(d, &ents, &all_segs, k);
    let mut doors: HashSet<Handle> = HashSet::new();
    for o in ops.iter().filter(|o| o.kind == "door" && o.swing.is_some()) {
        doors.insert(o.handle);
        if let Some((_, ends)) = o.swing {
            let tol = o.width * 0.05;
            for (e, (_, segs)) in ents.iter().zip(&segs_by) {
                if segs
                    .iter()
                    .any(|g| ends.iter().any(|end| (g.a.dist(o.at) < tol && g.b.dist(*end) < tol) || (g.b.dist(o.at) < tol && g.a.dist(*end) < tol)))
                {
                    doors.insert(e.handle);
                    walls.remove(&e.handle);
                }
            }
        }
    }
    // Swings that came through as chords (a PDF draws arcs as curves, often read back as short
    // straight pieces): points on one circle sweeping 60°–100° with a door's radius, the leaf
    // from its centre.
    // Chord swings to rebuild as arcs (WALLGEN cuts doors at arcs): handle, centre, radius,
    // ends, and the leaf joined to an end (hinge → that end).
    let mut swings: Vec<(Handle, Vec2, f64, Vec2, Vec2, Option<Vec2>)> = Vec::new();
    for (e, (_, segs)) in ents.iter().zip(&segs_by) {
        let EntityKind::LwPolyline(pl) = &e.kind else { continue };
        if pl.closed || pl.vertices.len() < 3 || doors.contains(&e.handle) {
            continue;
        }
        let all: Vec<Vec2> = pl.vertices.iter().map(|v| v.p).collect();
        // The chords alone, or with the leaf joined at either end.
        let fit = [(0, 0), (0, 1), (1, 0)].into_iter().find_map(|(lo, hi)| {
            let pts = all.get(lo..all.len().saturating_sub(hi))?;
            let (a, m, b) = (*pts.first()?, *pts.get(pts.len() / 2)?, *pts.last()?);
            if pts.len() < 3 {
                return None;
            }
            let (c, r) = circle3(a, m, b)?;
            let sweep = (a - c).cross(b - c).atan2((a - c).dot(b - c)).abs().to_degrees();
            let leaf = match (lo, hi) {
                (1, _) => Some(a),
                (_, 1) => Some(b),
                _ => None,
            };
            ((16.0 * k..=54.0 * k).contains(&r) && pts.iter().all(|p| (p.dist(c) - r).abs() <= 0.03 * r) && (60.0..=100.0).contains(&sweep))
                .then_some((a, b, c, r, leaf))
        });
        let Some((a, b, c, r, leaf)) = fit else { continue };
        swings.push((e.handle, c, r, a, b, leaf));
        let (a, b) = (&a, &b);
        doors.insert(e.handle);
        walls.remove(&e.handle);
        let tol = r * 0.05;
        for (f, (_, fs)) in ents.iter().zip(&segs_by) {
            if fs
                .iter()
                .any(|g| [*a, *b].iter().any(|end| (g.a.dist(c) < tol && g.b.dist(*end) < tol) || (g.b.dist(c) < tol && g.a.dist(*end) < tol)))
            {
                doors.insert(f.handle);
                walls.remove(&f.handle);
            }
        }
        let _ = segs;
    }
    // Swings joined to their leaf in one polyline: an arc piece (a bulge) of a door's size.
    for e in &ents {
        let EntityKind::LwPolyline(pl) = &e.kind else { continue };
        if doors.contains(&e.handle) {
            continue;
        }
        let swing = cadaddy_geom::Polyline { vertices: pl.vertices.clone(), closed: pl.closed }.segments().into_iter().any(|sg| match sg {
            cadaddy_geom::Segment::Arc { arc, .. } => {
                let sweep = (arc.end - arc.start).rem_euclid(std::f64::consts::TAU).to_degrees();
                (16.0 * k..=54.0 * k).contains(&arc.radius) && (60.0..=100.0).contains(&sweep)
            }
            _ => false,
        });
        if swing {
            doors.insert(e.handle);
            walls.remove(&e.handle);
        }
    }
    // Room names: traced on a plan of what bounds rooms (the walls, doors and windows found
    // above), so MEP lines, stair treads, furniture and tags never split or close a room; each
    // area named once, by its tag (see `room_tags`).
    let ext = d.extents(&cadaddy_doc::Space::Model);
    let sheet_area = (ext.width() * ext.height()).max(1e-9);
    let bounding: HashSet<Handle> =
        walls.iter().chain(doors.iter()).copied().chain(ops.iter().filter(|o| o.kind == "window").map(|o| o.handle)).collect();
    let plan = super::bspec::room_plan_of(s, &bounding)?;
    let trace_on = plan.as_ref().unwrap_or(s);
    let mut areas: Vec<Vec<Vec2>> = Vec::new();
    let mut names: Vec<(usize, Handle, String, Vec2, f64)> = Vec::new();
    let mut numbers: Vec<Handle> = Vec::new();
    let mut outline_cache = None;
    for (h, t, at, height) in texts(&ents) {
        let name = t.chars().filter(|c| c.is_alphabetic()).count() >= 2
            && t.chars().count() <= 40
            && !room_number(&t)
            && !t.contains('\'')
            && !t.contains(':');
        if !name && !room_number(&t) {
            continue;
        }
        let Ok(loops) = super::hatch::loops_at_lazy(trace_on, at, &mut outline_cache) else { continue };
        let Some(outer) = loops.iter().find(|l| l.outer).or(loops.first()) else { continue };
        let q = cadaddy_geom::Polyline { vertices: outer.vertices.clone(), closed: true }.tessellate(1e-3);
        let area = cadaddy_geom::shoelace(&q).abs();
        if q.len() < 3 || area < 16.0 * 144.0 * k * k || area > 0.5 * sheet_area {
            continue;
        }
        if !name {
            numbers.push(h);
            continue;
        }
        let i = match areas.iter().position(|a| same_outline(a, &q)) {
            Some(i) => i,
            None => {
                areas.push(q);
                areas.len() - 1
            }
        };
        names.push((i, h, t.clone(), at, height));
    }
    let rooms: Vec<(Handle, String)> = room_tags(&areas, &names).into_iter().map(|(_, h, t)| (h, t)).collect();
    // The sheet: number and title from the title block.
    let sheets = super::perceive::run_sheets(s, &json!({}))?;
    let items = sheets["items"].as_array().cloned().unwrap_or_default();
    let mut sheet = items.iter().find(|i| i["number"].is_string()).or(items.first()).cloned().unwrap_or(Value::Null);
    // Our own PDFs carry each text twice: the exact hidden copy and the stroked lettering read
    // back by OCR, which can come out taller and misread ("A10.2." for "A102"). The sheet takes
    // the hidden copy at the same place.
    if let Some(exact) = sheet["number"].as_str().and_then(|n| hidden_copy(&ents, n)) {
        sheet["number"] = json!(exact);
    }
    // A title block of plain texts (a PDF keeps no attributes): the title is the text beside the
    // sheet number.
    if sheet["title"].is_null()
        && let Some(title) = sheet["number"].as_str().and_then(|n| title_near(&ents, n))
    {
        sheet["title"] = json!(hidden_copy(&ents, &title).unwrap_or(title));
    }
    let wall_weight = heaviest.filter(|_| walls.iter().filter_map(|h| d.entity(*h)).any(|e| heavy(e))).map(|w| f64::from(w) / 100.0);
    let mut out = json!({
        "raster_only": false,
        "scale": { "factor": factor, "source": source },
        "walls": { "objects": walls.len(), "pairs": found.len(), "lineweight_mm": wall_weight },
        "doors": doors.len(),
        "rooms": rooms.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>(),
        "sheet": { "number": sheet.get("number").cloned().unwrap_or(Value::Null), "title": sheet.get("title").cloned().unwrap_or(Value::Null) },
    });
    let apply = bool_or(p, "apply", false) || bool_or(p, "draft", false);
    if apply {
        let dm = s.doc_mut()?;
        for (name, color) in [("A-WALL", 7), ("A-DOOR", 2), ("A-AREA-IDEN", 3)] {
            if dm.layer(name).is_none() {
                dm.layers.push(cadaddy_doc::Layer {
                    name: name.into(),
                    color: cadaddy_doc::color::Color::Index(color),
                    ..cadaddy_doc::Layer::default()
                });
            }
        }
        let room_set: HashSet<Handle> = rooms.iter().map(|(h, _)| *h).chain(numbers.iter().copied()).collect();
        let m = Mat3::scale(factor, factor);
        // Chord swings become arcs (and their joined leaves lines) on A-DOOR.
        for (h, c, r, a, b, leaf) in &swings {
            let (mut start, mut end) = ((*a - *c).angle(), (*b - *c).angle());
            if (*a - *c).cross(*b - *c) < 0.0 {
                std::mem::swap(&mut start, &mut end);
            }
            let arc = cadaddy_doc::Arc { center: cadaddy_geom::Vec3::new(c.x, c.y, 0.0), radius: *r, start, end };
            let _ = dm.modify_entity(*h, |e| e.kind = EntityKind::Arc(arc));
            if let Some(end) = leaf {
                let common = cadaddy_doc::Common { layer: "A-DOOR".into(), ..cadaddy_doc::Common::default() };
                let line =
                    EntityKind::Line(cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(c.x, c.y, 0.0), b: cadaddy_geom::Vec3::new(end.x, end.y, 0.0) });
                if let Ok(nh) = dm.add(&cadaddy_doc::Space::Model, common, line) {
                    doors.insert(nh);
                }
            }
        }
        let hs: Vec<Handle> = dm.model.iter().filter(|e| super::wallgen::made_as(e).is_none()).map(|e| e.handle).collect();
        for h in hs {
            let layer = if walls.contains(&h) {
                Some("A-WALL")
            } else if doors.contains(&h) {
                Some("A-DOOR")
            } else if room_set.contains(&h) {
                Some("A-AREA-IDEN")
            } else {
                None
            };
            let _ = dm.modify_entity(h, |e| {
                if let Some(l) = layer {
                    e.common.layer = l.into();
                }
                if (factor - 1.0).abs() > 1e-12 {
                    e.kind.transform(&m);
                }
            });
        }
        if (factor - 1.0).abs() > 1e-12 {
            dm.header.set_i64("INSUNITS", 1);
        }
        out["applied"] = json!(true);
        if bool_or(p, "draft", false) {
            let r = super::autospec::draft_with(s, false)?;
            s.doc_mut()?.building_spec = Some(r.to_json());
            out["spec"] = serde_json::to_value(&r).unwrap_or(Value::Null);
        }
    } else {
        out["applied"] = json!(false);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {

    /// The grid never misses a pair a face is near: every pair the full scan finds is among
    /// the grid's candidates (random pairs and faces, and the degenerate cases).
    #[test]
    fn the_pair_grid_misses_nothing() {
        use super::super::perceive::geometry::{Pair, Seg};
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f64 / 10.0
        };
        let pair =
            |a: Vec2, b: Vec2, t: f64| Pair { layer: String::new(), c0: a, c1: b, thickness: t, length: a.dist(b), confidence: 1.0, faces: [0, 0] };
        let pairs: Vec<Pair> = (0..400).map(|_| pair(Vec2::new(rnd(), rnd()), Vec2::new(rnd(), rnd()), 2.0 + rnd() / 100.0)).collect();
        let slack = 0.25;
        let grid = PairGrid::new(&pairs, slack);
        let near = |sg: &Seg, pr: &Pair| {
            let reach = pr.thickness / 2.0 + slack;
            dist((sg.a + sg.b) * 0.5, pr.c0, pr.c1) <= reach || dist((pr.c0 + pr.c1) * 0.5, sg.a, sg.b) <= reach
        };
        let mut hits = 0;
        for _ in 0..2_000 {
            let a = Vec2::new(rnd(), rnd());
            let b = if rnd() < 500.0 { a + Vec2::new(rnd() / 50.0, rnd() / 50.0) } else { Vec2::new(rnd(), rnd()) };
            let sg = Seg { a, b, layer: String::new() };
            let cand: Vec<usize> = grid.near(a, b).collect();
            for (i, pr) in pairs.iter().enumerate() {
                if near(&sg, pr) {
                    hits += 1;
                    assert!(cand.contains(&i), "pair {i} missed for {a:?}-{b:?}");
                }
            }
        }
        assert!(hits > 100, "{hits}");
        // No pairs, one point-sized pair, and a face far outside: no panic.
        assert_eq!(PairGrid::new(&[], slack).near(Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0)).count(), 0);
        let one = [pair(Vec2::new(5.0, 5.0), Vec2::new(5.0, 5.0), 0.0)];
        assert_eq!(PairGrid::new(&one, 0.0).near(Vec2::new(5.0, 5.0), Vec2::new(5.0, 5.0)).count(), 1);
        let _ = grid.near(Vec2::new(-1e300, f64::NAN), Vec2::new(1e300, 1e300)).count();
    }
    use super::*;

    #[test]
    fn scale_notes() {
        assert_eq!(scale_note("SCALE: 1/4\" = 1'-0\""), Some(48.0));
        assert_eq!(scale_note("1/8\"=1'-0\""), Some(96.0));
        assert_eq!(scale_note("1\" = 20'"), Some(240.0));
        assert_eq!(scale_note("SCALE 1:100"), Some(100.0));
        assert_eq!(scale_note("FLOOR PLAN"), None);
    }
}
