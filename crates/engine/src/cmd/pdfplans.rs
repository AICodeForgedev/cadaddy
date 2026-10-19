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
            "raster_only": true,
            "message": "This page is a picture (a scan or a rendering), not linework: plans can only be read from vector PDFs.",
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
    let near_pair = |sg: &super::perceive::geometry::Seg| {
        found.iter().any(|pr| {
            let reach = pr.thickness / 2.0 + 0.25 * k;
            dist((sg.a + sg.b) * 0.5, pr.c0, pr.c1) <= reach || dist((pr.c0 + pr.c1) * 0.5, sg.a, sg.b) <= reach
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
        if score >= 2 {
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
            ((16.0 * k..=54.0 * k).contains(&r) && pts.iter().all(|p| (p.dist(c) - r).abs() <= 0.03 * r) && (60.0..=100.0).contains(&sweep))
                .then_some((a, b, c, r))
        });
        let Some((a, b, c, r)) = fit else { continue };
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
    // Room names: names inside a closed area of a room's size.
    let ext = d.extents(&cadaddy_doc::Space::Model);
    let sheet_area = (ext.width() * ext.height()).max(1e-9);
    let mut rooms: Vec<(Handle, String)> = Vec::new();
    let mut numbers: Vec<Handle> = Vec::new();
    for (h, t, at, _) in texts(&ents) {
        let name = t.chars().filter(|c| c.is_alphabetic()).count() >= 2
            && t.chars().count() <= 40
            && !room_number(&t)
            && !t.contains('\'')
            && !t.contains(':');
        if !name && !room_number(&t) {
            continue;
        }
        let Ok(loops) = super::hatch::loops_at(s, at) else { continue };
        let Some(outer) = loops.iter().find(|l| l.outer).or(loops.first()) else { continue };
        let q = cadaddy_geom::Polyline { vertices: outer.vertices.clone(), closed: true }.tessellate(1e-3);
        let area = cadaddy_geom::shoelace(&q).abs();
        if area >= 16.0 * 144.0 * k * k && area <= 0.5 * sheet_area {
            if name {
                rooms.push((h, t.clone()));
            } else {
                numbers.push(h);
            }
        }
    }
    // The sheet: number and title from the title block.
    let sheets = super::perceive::run_sheets(s, &json!({}))?;
    let items = sheets["items"].as_array().cloned().unwrap_or_default();
    let sheet = items.iter().find(|i| i["number"].is_string()).or(items.first()).cloned().unwrap_or(Value::Null);
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
            let r = super::autospec::draft(s)?;
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
