//! The geometric perceptions: wall face pairs, door swings and opening blocks, and room outlines.

use cadaddy_doc::{Drawing, Entity, EntityKind};
use cadaddy_geom::{Polyline, Segment, Vec2};
use serde_json::{Value, json};

use super::{objects, page, pt, round, texts, units_per_inch};
use crate::cmd::{bool_or, str_param};
use crate::{Result, Session};

/// Straight segments looked at, at most (hostile drawings).
const MAX_SEGS: usize = 200_000;
/// Pairs reported at most.
const MAX_PAIRS: usize = 50_000;
/// Rooms traced at most.
const MAX_ROOMS: usize = 1_000;

/// A straight segment of a line or polyline.
#[derive(Clone, Debug)]
pub(crate) struct Seg {
    pub a: Vec2,
    pub b: Vec2,
    pub layer: String,
}

/// The straight segments of the space's lines and polylines.
pub(crate) fn segments(ents: &[&std::sync::Arc<Entity>]) -> Vec<Seg> {
    let mut out = Vec::new();
    for e in ents {
        if out.len() >= MAX_SEGS {
            break;
        }
        match &e.kind {
            EntityKind::Line(l) => out.push(Seg { a: l.a.xy(), b: l.b.xy(), layer: e.common.layer.clone() }),
            EntityKind::LwPolyline(p) => {
                for sg in (Polyline { vertices: p.vertices.clone(), closed: p.closed }).segments() {
                    if let Segment::Line(l) = sg {
                        out.push(Seg { a: l.a, b: l.b, layer: e.common.layer.clone() });
                    }
                }
            }
            _ => {}
        }
    }
    out.retain(|s| s.a.is_finite() && s.b.is_finite() && s.a.dist(s.b) > 1e-9);
    out
}

/// Wall size limits in drawing units.
pub(crate) struct Limits {
    pub min_thick: f64,
    pub max_thick: f64,
    pub min_len: f64,
}

impl Limits {
    pub fn default_for(d: &Drawing) -> Limits {
        let k = units_per_inch(d);
        Limits { min_thick: 2.5 * k, max_thick: 20.0 * k, min_len: 12.0 * k }
    }
}

/// A wall candidate: two parallel faces a wall's thickness apart.
pub(crate) struct Pair {
    pub layer: String,
    pub c0: Vec2,
    pub c1: Vec2,
    pub thickness: f64,
    pub length: f64,
    pub confidence: f64,
}

/// Parallel face pairs: segments in the same direction (within half a degree), their lines
/// `min_thick..max_thick` apart, overlapping along the direction by at least `min_len`.
pub(crate) fn pairs(_d: &Drawing, segs: &[Seg], lim: &Limits) -> Vec<Pair> {
    // Group by direction (half-degree buckets), then sort each group by offset across.
    let mut groups: std::collections::HashMap<i64, Vec<(f64, usize)>> = std::collections::HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        let d = s.b - s.a;
        let ang = d.y.atan2(d.x).rem_euclid(std::f64::consts::PI);
        let key = (ang.to_degrees() * 2.0).round() as i64 % 360;
        let n = Vec2::new(-ang.sin(), ang.cos());
        groups.entry(key).or_default().push((n.dot(s.a), i));
    }
    let mut out = Vec::new();
    for (key, mut g) in groups {
        g.sort_by(|a, b| a.0.total_cmp(&b.0));
        let ang = (key as f64 / 2.0).to_radians();
        let (u, n) = (Vec2::new(ang.cos(), ang.sin()), Vec2::new(-ang.sin(), ang.cos()));
        for x in 0..g.len() {
            for y in x + 1..g.len() {
                let gap = g[y].0 - g[x].0;
                if gap > lim.max_thick {
                    break;
                }
                if gap < lim.min_thick {
                    continue;
                }
                let (s1, s2) = (&segs[g[x].1], &segs[g[y].1]);
                let (a0, a1) = minmax(u.dot(s1.a), u.dot(s1.b));
                let (b0, b1) = minmax(u.dot(s2.a), u.dot(s2.b));
                let (lo, hi) = (a0.max(b0), a1.min(b1));
                let overlap = hi - lo;
                if overlap < lim.min_len.max(gap * 1.5) {
                    continue;
                }
                let mid = (g[x].0 + g[y].0) / 2.0;
                let (c0, c1) = (u * lo + n * mid, u * hi + n * mid);
                let shorter = (a1 - a0).min(b1 - b0).max(1e-9);
                let wall_layer = s1.layer.to_ascii_uppercase().contains("WALL");
                let confidence = (0.35
                    + 0.25 * f64::from(u8::from(wall_layer))
                    + 0.25 * (overlap / shorter).min(1.0)
                    + 0.15 * f64::from(u8::from(s1.layer == s2.layer)))
                .min(1.0);
                out.push(Pair { layer: s1.layer.clone(), c0, c1, thickness: gap, length: overlap, confidence });
                if out.len() >= MAX_PAIRS {
                    return out;
                }
            }
        }
    }
    out.sort_by(|a, b| b.confidence.total_cmp(&a.confidence).then(b.length.total_cmp(&a.length)));
    out
}

fn minmax(a: f64, b: f64) -> (f64, f64) {
    if a <= b { (a, b) } else { (b, a) }
}

pub(super) fn run_walls(s: &mut Session, p: &Value) -> Result<Value> {
    let (d, ents) = objects(s)?;
    let mut lim = Limits::default_for(d);
    let num = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0);
    if let Some(v) = num("minThickness") {
        lim.min_thick = v;
    }
    if let Some(v) = num("maxThickness") {
        lim.max_thick = v;
    }
    if let Some(v) = num("minLength") {
        lim.min_len = v;
    }
    let mut segs = segments(&ents);
    if let Some(pat) = str_param(p, "layers").filter(|x| !x.trim().is_empty()) {
        segs.retain(|g| pat.split(',').any(|q| crate::cmd::qselect::wildcard(q.trim(), &g.layer)));
    }
    let items: Vec<Value> = pairs(d, &segs, &lim)
        .into_iter()
        .map(|w| json!({"layer": w.layer, "centerline": [pt(w.c0), pt(w.c1)], "thickness": round(w.thickness), "length": round(w.length), "confidence": round(w.confidence)}))
        .collect();
    Ok(page(items, p))
}

// ---------------- plan.openings ----------------

pub(super) fn run_openings(s: &mut Session, p: &Value) -> Result<Value> {
    let (d, ents) = objects(s)?;
    let k = units_per_inch(d);
    let (rmin, rmax) = (16.0 * k, 54.0 * k);
    let segs = segments(&ents);
    let mut items = Vec::new();
    for e in &ents {
        match &e.kind {
            // A door swing: a quarter-ish arc of a door's width, ideally with its leaf drawn from
            // the hinge (the arc's centre) to one end.
            EntityKind::Arc(a) => {
                let mut sweep = (a.end - a.start).rem_euclid(std::f64::consts::TAU);
                if sweep < 1e-9 {
                    sweep = std::f64::consts::TAU;
                }
                let deg = sweep.to_degrees();
                if !(60.0..=100.0).contains(&deg) || !(rmin..=rmax).contains(&a.radius) {
                    continue;
                }
                let c = a.center.xy();
                let ends = [c + Vec2::from_angle(a.start) * a.radius, c + Vec2::from_angle(a.end) * a.radius];
                let tol = a.radius * 0.05;
                let leaf = segs.iter().any(|g| {
                    let (p, q) = (g.a, g.b);
                    ends.iter().any(|e| (p.dist(c) < tol && q.dist(*e) < tol) || (q.dist(c) < tol && p.dist(*e) < tol))
                });
                let on_door_layer = e.common.layer.to_ascii_uppercase().contains("DOOR");
                let confidence = (0.5 + 0.3 * f64::from(u8::from(leaf)) + 0.2 * f64::from(u8::from(on_door_layer))).min(1.0);
                items.push(json!({
                    "kind": "door", "at": pt(c), "width": round(a.radius), "layer": e.common.layer, "handle": e.handle.hex(),
                    "swing": round(deg), "confidence": round(confidence),
                }));
            }
            // Door and window blocks, by name.
            EntityKind::Insert(i) => {
                let name = i.block.to_ascii_uppercase();
                let kind = if name.contains("DOOR") || name.starts_with("DR") {
                    "door"
                } else if name.contains("WIN") || name.contains("WDW") || name.contains("GLAZ") {
                    "window"
                } else {
                    continue;
                };
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                let width = if b.is_empty() { 0.0 } else { b.width().max(b.height()) };
                items.push(json!({
                    "kind": kind, "at": pt(i.insert.xy()), "width": round(width), "layer": e.common.layer, "handle": e.handle.hex(),
                    "block": i.block, "confidence": 0.75,
                }));
            }
            _ => {}
        }
    }
    Ok(page(items, p))
}

// ---------------- plan.rooms ----------------

/// A room number: optional letter, 2–4 digits, optional letter (`101`, `B12`, `204A`).
fn room_number(t: &str) -> bool {
    let t = t.trim();
    let digits = t.chars().filter(char::is_ascii_digit).count();
    let letters = t.chars().filter(char::is_ascii_alphabetic).count();
    (2..=4).contains(&digits) && letters <= 2 && t.len() <= 6 && t.chars().all(|c| c.is_ascii_alphanumeric())
}

fn polygon(l: &cadaddy_doc::HatchLoop) -> Vec<Vec2> {
    Polyline { vertices: l.vertices.clone(), closed: true }.tessellate(1e-3)
}

pub(super) fn run_rooms(s: &mut Session, p: &Value) -> Result<Value> {
    let (d, ents) = objects(s)?;
    let k = units_per_inch(d);
    let min_area = 16.0 * 144.0 * k * k;
    let ext = d.extents(&s.space());
    let sheet = (ext.width() * ext.height()).max(1e-9);
    let all = texts(&ents);
    let names: Vec<_> = all
        .iter()
        .filter(|(_, t, _, _)| t.chars().filter(|c| c.is_alphabetic()).count() >= 2 && t.chars().count() <= 40 && !room_number(t))
        .collect();
    let numbers: Vec<_> = all.iter().filter(|(_, t, _, _)| room_number(t)).collect();
    let mut items: Vec<Value> = Vec::new();
    let mut seen: Vec<Vec<Vec2>> = Vec::new();
    let with_outline = bool_or(p, "outline", true);
    for (h, name, at, height) in names.into_iter().take(MAX_ROOMS) {
        let Ok(loops) = crate::cmd::hatch::loops_at(s, *at) else { continue };
        let Some(outer) = loops.iter().find(|l| l.outer).or(loops.first()) else { continue };
        let poly = polygon(outer);
        if poly.len() < 3 {
            continue;
        }
        let area = cadaddy_geom::shoelace(&poly).abs();
        if area < min_area {
            continue;
        }
        // One room per outline: a second label in the same room is its number or a note.
        if seen.iter().any(|q| q.len() == poly.len() && q.iter().zip(&poly).all(|(a, b)| a.dist(*b) < 1e-6)) {
            continue;
        }
        let perimeter: f64 = poly.iter().zip(poly.iter().cycle().skip(1)).map(|(a, b)| a.dist(*b)).sum();
        // The room number: a number inside the outline, the nearest to the name.
        let number = numbers
            .iter()
            .filter(|(_, _, q, _)| cadaddy_geom::point_in_polygon(&poly, *q))
            .min_by(|a, b| a.2.dist(*at).total_cmp(&b.2.dist(*at)))
            .map(|(_, t, _, _)| t.clone());
        let share = area / sheet;
        let confidence = (0.5 + 0.2 * f64::from(u8::from(number.is_some())) + if share < 0.5 { 0.3 } else { 0.0 }
            - if poly.len() > 200 { 0.1 } else { 0.0 })
        .clamp(0.0, 1.0);
        let mut v = json!({
            "name": name, "number": number, "at": pt(*at), "area": round(area), "perimeter": round(perimeter),
            "label": h.hex(), "textHeight": round(*height), "confidence": round(confidence),
        });
        if with_outline {
            let step = (poly.len() / 64).max(1);
            v["outline"] = json!(poly.iter().step_by(step).map(|q| pt(*q)).collect::<Vec<_>>());
        }
        seen.push(poly);
        items.push(v);
    }
    Ok(page(items, p))
}
