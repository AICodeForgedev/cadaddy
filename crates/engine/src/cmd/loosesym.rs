//! Symbols drawn as loose strokes (Q6167): a device with no mark beside it (a fire alarm plan's
//! strobe, horn/strobe) imports as its strokes, not as a symbol block, so it is no device until
//! something says what it is. Its legend says so: the row's drawn sample has the same shape as
//! each copy on the plans. The plans' short strokes are grouped where they touch, and each group
//! whose shape (turned a quarter at a time, or mirrored) is a legend row's is that row's symbol.

use std::collections::BTreeMap;

use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_geom::{Mat3, Vec2};

use super::guided::{Shape, shape_of};

/// Strokes looked at on one sheet, symbols found on it, at most.
const MAX_STROKES: usize = 200_000;
const MAX_FOUND: usize = 5_000;
/// Strokes in one symbol, at most (a legend's sample is a few lines and circles).
const MAX_PARTS: usize = 64;

/// A legend row's symbol found on a plan.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    /// Its middle (drawing units).
    pub at: Vec2,
    /// The row's words and the sheet of the legend.
    pub row: String,
    pub legend_sheet: String,
    pub handles: Vec<Handle>,
}

/// The extent of a stroke.
fn extent(k: &EntityKind) -> Option<[f64; 4]> {
    let (lo, hi) = match k {
        EntityKind::Line(l) => ([l.a.x.min(l.b.x), l.a.y.min(l.b.y)], [l.a.x.max(l.b.x), l.a.y.max(l.b.y)]),
        EntityKind::Circle(c) => ([c.center.x - c.radius, c.center.y - c.radius], [c.center.x + c.radius, c.center.y + c.radius]),
        EntityKind::Arc(a) => ([a.center.x - a.radius, a.center.y - a.radius], [a.center.x + a.radius, a.center.y + a.radius]),
        EntityKind::LwPolyline(p) if !p.vertices.is_empty() => {
            p.vertices.iter().take(MAX_PARTS).fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), v| {
                ([lo[0].min(v.p.x), lo[1].min(v.p.y)], [hi[0].max(v.p.x), hi[1].max(v.p.y)])
            })
        }
        _ => return None,
    };
    let b = [lo[0], lo[1], hi[0], hi[1]];
    b.iter().all(|v| v.is_finite()).then_some(b)
}

fn find_root(parent: &mut [usize], mut i: usize) -> usize {
    while let Some(&p) = parent.get(i) {
        if p == i {
            break;
        }
        let gp = parent.get(p).copied().unwrap_or(p);
        if let Some(x) = parent.get_mut(i) {
            *x = gp;
        }
        i = p;
    }
    i
}

/// The legend rows' symbols among the loose strokes of the objects with handles in `lo..hi` (one
/// sheet), each no bigger than `max_size` across, outside `exclude` (the legend tables
/// themselves), for the rows `wanted` takes (by their words and their legend's sheet).
pub(crate) fn find(
    d: &Drawing,
    lo: u64,
    hi: u64,
    legend: &BTreeMap<Shape, (String, String)>,
    wanted: &dyn Fn(&str, &str) -> bool,
    max_size: f64,
    exclude: &[[f64; 4]],
) -> Vec<Found> {
    let mut out = Vec::new();
    if legend.is_empty() || !(max_size.is_finite() && max_size > 0.0) {
        return out;
    }
    // The sheet's short strokes.
    let strokes: Vec<(Handle, &EntityKind, [f64; 4])> = d
        .model
        .iter()
        .filter(|e| e.handle.0 >= lo && e.handle.0 < hi)
        .filter_map(|e| extent(&e.kind).map(|b| (e.handle, &e.kind, b)))
        .filter(|(_, _, b)| b[2] - b[0] <= max_size && b[3] - b[1] <= max_size)
        .take(MAX_STROKES)
        .collect();
    // Those that touch, grouped (a grid a symbol wide).
    let tol = 0.02 * max_size;
    let cell = |v: f64| (v / max_size).floor() as i64;
    let mut grid: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, (_, _, b)) in strokes.iter().enumerate() {
        grid.entry((cell(b[0]), cell(b[1]))).or_default().push(i);
    }
    let mut parent: Vec<usize> = (0..strokes.len()).collect();
    for (i, (_, _, b)) in strokes.iter().enumerate() {
        let (cx, cy) = (cell(b[0]), cell(b[1]));
        for gx in cx - 1..=cx + 1 {
            for gy in cy - 1..=cy + 1 {
                for &j in grid.get(&(gx, gy)).into_iter().flatten().filter(|&&j| j > i) {
                    let Some((_, _, c)) = strokes.get(j) else { continue };
                    let touch = b[0] <= c[2] + tol && c[0] <= b[2] + tol && b[1] <= c[3] + tol && c[1] <= b[3] + tol;
                    if touch {
                        let (ri, rj) = (find_root(&mut parent, i), find_root(&mut parent, j));
                        if ri != rj
                            && let Some(p) = parent.get_mut(ri)
                        {
                            *p = rj;
                        }
                    }
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..strokes.len() {
        let r = find_root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    for members in groups.values() {
        if out.len() >= MAX_FOUND {
            break;
        }
        if members.len() < 2 || members.len() > MAX_PARTS {
            continue;
        }
        let b = members
            .iter()
            .filter_map(|&i| strokes.get(i).map(|s| s.2))
            .fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |a, b| {
                [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
            });
        if b[2] - b[0] > max_size || b[3] - b[1] > max_size {
            continue;
        }
        let mid = Vec2::new((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        if exclude.iter().any(|x| mid.x >= x[0] && mid.x <= x[2] && mid.y >= x[1] && mid.y <= x[3]) {
            continue;
        }
        let kinds: Vec<&EntityKind> = members.iter().filter_map(|&i| strokes.get(i).map(|s| s.1)).collect();
        // As drawn, a quarter turn at a time, and mirrored.
        let mut hit: Option<&(String, String)> = None;
        'turns: for mirrored in [false, true] {
            for q in 0..4 {
                let m = Mat3::rotate_about(mid, f64::from(q) * std::f64::consts::FRAC_PI_2);
                let m = if mirrored { Mat3::mirror(mid, mid + Vec2::new(0.0, 1.0)).then(m) } else { m };
                let turned: Vec<EntityKind> = kinds
                    .iter()
                    .map(|k| {
                        let mut k = (*k).clone();
                        k.transform(&m);
                        k
                    })
                    .collect();
                if let Some(row) = shape_of(turned.iter()).and_then(|s| legend.get(&s)).filter(|(w, sheet)| wanted(w, sheet)) {
                    hit = Some(row);
                    break 'turns;
                }
            }
        }
        if let Some((row, sheet)) = hit {
            out.push(Found {
                at: mid,
                row: row.clone(),
                legend_sheet: sheet.clone(),
                handles: members.iter().filter_map(|&i| strokes.get(i).map(|s| s.0)).collect(),
            });
        }
    }
    out
}

/// The extent of some drawn objects (lines, circles, arcs, polylines).
pub(crate) fn bounds_of(parts: &[&EntityKind]) -> Option<[f64; 4]> {
    parts.iter().filter_map(|k| extent(k)).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
}

/// A straight piece of a drawn symbol (a curve as its chords): its middle, length and direction
/// (radians, a half turn round).
#[derive(Clone, Copy, Debug)]
struct Seg {
    mid: Vec2,
    len: f64,
    dir: f64,
}

/// Chords a circle (or a full arc) is read as.
const CIRCLE_CHORDS: usize = 16;
/// Pieces in one legend sample, at most; pieces looked at on one sheet, at most.
const MAX_TEMPLATE_SEGS: usize = 96;
const MAX_SEGS: usize = 400_000;
/// Comparisons made on one sheet, at most (hostile sheets stop early).
const MAX_WORK: usize = 60_000_000;
/// The share of a sample's drawn length a copy must show: the plans may draw a part otherwise
/// (a circle of another size in the box of a horn/strobe) or not at all.
const MATCH_SHARE: f64 = 0.75;

fn seg(a: Vec2, b: Vec2) -> Option<Seg> {
    let v = b - a;
    let len = v.x.hypot(v.y);
    (len.is_finite() && len > 1e-9).then(|| Seg { mid: (a + b) * 0.5, len, dir: v.y.atan2(v.x).rem_euclid(std::f64::consts::PI) })
}

/// An object's straight pieces.
fn segs_of(k: &EntityKind, out: &mut Vec<Seg>) {
    let ring = |c: Vec2, r: f64, a0: f64, sweep: f64, n: usize, out: &mut Vec<Seg>| {
        let at = |i: usize| c + Vec2::new((a0 + sweep * i as f64 / n as f64).cos(), (a0 + sweep * i as f64 / n as f64).sin()) * r;
        out.extend((0..n).filter_map(|i| seg(at(i), at(i + 1))));
    };
    match k {
        EntityKind::Line(l) => out.extend(seg(l.a.xy(), l.b.xy())),
        EntityKind::Circle(c) if c.radius.is_finite() && c.radius > 0.0 => {
            ring(c.center.xy(), c.radius, 0.0, std::f64::consts::TAU, CIRCLE_CHORDS, out);
        }
        EntityKind::Arc(a) if a.radius.is_finite() && a.radius > 0.0 && a.start.is_finite() && a.end.is_finite() => {
            let sweep = (a.end - a.start).rem_euclid(std::f64::consts::TAU);
            let n = ((sweep / std::f64::consts::TAU * CIRCLE_CHORDS as f64).ceil() as usize).clamp(1, CIRCLE_CHORDS);
            ring(a.center.xy(), a.radius, a.start, sweep, n, out);
        }
        EntityKind::LwPolyline(p) => {
            let v: Vec<Vec2> = p.vertices.iter().take(MAX_TEMPLATE_SEGS).map(|v| v.p).collect();
            out.extend(v.windows(2).filter_map(|w| match w {
                [a, b] => seg(*a, *b),
                _ => None,
            }));
            if p.closed
                && let (Some(a), Some(b)) = (v.last(), v.first())
            {
                out.extend(seg(*a, *b));
            }
        }
        _ => {}
    }
}

/// The smaller angle between two directions a half turn round.
fn turn_between(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(std::f64::consts::PI);
    d.min(std::f64::consts::PI - d)
}

/// A legend row's drawn sample, matched piece by piece ([`find_by_parts`]).
#[derive(Clone, Debug)]
pub(crate) struct Template {
    pub row: String,
    pub legend_sheet: String,
    /// Its pieces about the middle of its extent.
    segs: Vec<Seg>,
    size: f64,
    total: f64,
}

impl Template {
    /// The row's sample from its drawn objects: three objects or more (fewer, a plain box or ring
    /// that shows itself everywhere), of some size.
    pub(crate) fn new(parts: &[&EntityKind], row: &str, legend_sheet: &str) -> Option<Template> {
        let mut segs = Vec::new();
        for k in parts.iter().take(MAX_PARTS) {
            segs_of(k, &mut segs);
        }
        segs.truncate(MAX_TEMPLATE_SEGS);
        let (lo, hi) = segs.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), s| {
            let h = Vec2::new(s.dir.cos(), s.dir.sin()) * (s.len / 2.0);
            let (a, b) = (s.mid - h, s.mid + h);
            ([lo[0].min(a.x).min(b.x), lo[1].min(a.y).min(b.y)], [hi[0].max(a.x).max(b.x), hi[1].max(a.y).max(b.y)])
        });
        let size = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        if parts.len() < 3 || segs.len() < 4 || !(size.is_finite() && size > 0.0) {
            return None;
        }
        let c = Vec2::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
        for s in &mut segs {
            s.mid -= c;
        }
        let total = segs.iter().map(|s| s.len).sum();
        Some(Template { row: row.to_string(), legend_sheet: legend_sheet.to_string(), segs, size, total })
    }

    /// Its pieces turned a quarter `q` times (after mirroring).
    fn turned(&self, q: u8, mirrored: bool) -> Vec<Seg> {
        self.segs
            .iter()
            .map(|s| {
                let (mut p, mut dir) = (s.mid, s.dir);
                if mirrored {
                    p = Vec2::new(-p.x, p.y);
                    dir = std::f64::consts::PI - dir;
                }
                for _ in 0..q {
                    p = Vec2::new(-p.y, p.x);
                }
                Seg { mid: p, len: s.len, dir: (dir + f64::from(q) * std::f64::consts::FRAC_PI_2).rem_euclid(std::f64::consts::PI) }
            })
            .collect()
    }
}

/// The copies of legend rows' samples drawn loose on a sheet (objects `lo..hi`), each found by
/// its pieces: as drawn, turned a quarter at a time or mirrored, touching other linework or not,
/// with [`MATCH_SHARE`] of the sample's drawn length where the sample has it. A place two rows
/// fit is the row it shows more of (a horn/strobe's box, X and flag over a box and flag). Not in
/// `exclude` (the legend tables).
pub(crate) fn find_by_parts(d: &Drawing, lo: u64, hi: u64, templates: &[Template], exclude: &[[f64; 4]]) -> Vec<Found> {
    let Some(size) = templates.iter().map(|t| t.size).max_by(f64::total_cmp) else { return Vec::new() };
    // The sheet's pieces of objects no bigger than a few symbols, by where their middles are.
    let mut segs: Vec<(Seg, Handle)> = Vec::new();
    for e in d.model.iter().filter(|e| e.handle.0 >= lo && e.handle.0 < hi) {
        let Some(b) = extent(&e.kind) else { continue };
        if b[2] - b[0] > 3.0 * size || b[3] - b[1] > 3.0 * size {
            continue;
        }
        let mut s = Vec::new();
        segs_of(&e.kind, &mut s);
        segs.extend(s.into_iter().map(|s| (s, e.handle)));
        if segs.len() >= MAX_SEGS {
            break;
        }
    }
    let cell = |v: f64| (v / size).floor() as i64;
    let mut grid: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, (s, _)) in segs.iter().enumerate() {
        grid.entry((cell(s.mid.x), cell(s.mid.y))).or_default().push(i);
    }
    let near = |p: Vec2| {
        let (cx, cy) = (cell(p.x), cell(p.y));
        (cx - 1..=cx + 1).flat_map(move |x| (cy - 1..=cy + 1).map(move |y| (x, y)))
    };
    let mut work = 0usize;
    // (matched length, share, middle, template, handles)
    let mut hits: Vec<(f64, f64, Vec2, usize, Vec<Handle>)> = Vec::new();
    'all: for (ti, t) in templates.iter().enumerate() {
        let tol = (0.06 * t.size).max(1e-6);
        for mirrored in [false, true] {
            for q in 0..4u8 {
                let parts = t.turned(q, mirrored);
                let Some(anchor) = parts.iter().copied().max_by(|a, b| a.len.total_cmp(&b.len)) else { continue };
                for (s, _) in segs.iter().filter(|(s, _)| (s.len - anchor.len).abs() <= 0.1 * anchor.len && turn_between(s.dir, anchor.dir) <= 0.1) {
                    let shift = s.mid - anchor.mid;
                    if exclude.iter().any(|x| shift.x >= x[0] && shift.x <= x[2] && shift.y >= x[1] && shift.y <= x[3]) {
                        continue;
                    }
                    let (mut matched, mut left) = (0.0, t.total);
                    let mut handles = Vec::new();
                    for part in &parts {
                        if matched + left < MATCH_SHARE * t.total {
                            break;
                        }
                        left -= part.len;
                        let at = part.mid + shift;
                        let hit = near(at).filter_map(|k| grid.get(&k)).flatten().find(|&&j| {
                            work += 1;
                            segs.get(j).is_some_and(|(o, _)| {
                                (o.mid - at).len() <= tol
                                    && (o.len - part.len).abs() <= (0.25 * part.len).max(tol)
                                    && turn_between(o.dir, part.dir) <= 0.18
                            })
                        });
                        if let Some((_, h)) = hit.and_then(|&j| segs.get(j)) {
                            matched += part.len;
                            if !handles.contains(h) {
                                handles.push(*h);
                            }
                        }
                    }
                    if matched >= MATCH_SHARE * t.total {
                        hits.push((matched, matched / t.total, shift, ti, handles));
                    }
                    if work > MAX_WORK {
                        break 'all;
                    }
                }
            }
        }
    }
    // The best fit at each place.
    hits.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.total_cmp(&a.1)));
    let mut out: Vec<Found> = Vec::new();
    let mut kept: Vec<(Vec2, f64)> = Vec::new();
    for (_, _, at, ti, handles) in hits {
        let Some(t) = templates.get(ti) else { continue };
        if kept.iter().any(|(p, s)| (*p - at).len() <= 0.5 * s.max(t.size)) {
            continue;
        }
        kept.push((at, t.size));
        out.push(Found { at, row: t.row.clone(), legend_sheet: t.legend_sheet.clone(), handles });
        if out.len() >= MAX_FOUND {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Circle, Entity, Line};
    use cadaddy_geom::Vec3;

    fn push(d: &mut Drawing, k: EntityKind) -> Handle {
        let h = Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        d.model.push(Entity::new(h, k));
        h
    }

    /// A made-up strobe: a circle with an X in it and a stem to its left, at `c`, size `s`, turned.
    fn strobe(d: &mut Drawing, c: Vec2, s: f64, turn: f64) {
        let p = |x: f64, y: f64| {
            let v = Vec2::new(x, y).rotate(turn) + c;
            Vec3::new(v.x, v.y, 0.0)
        };
        push(d, EntityKind::Circle(Circle { center: Vec3::new(c.x, c.y, 0.0), radius: 0.3 * s }));
        push(d, EntityKind::Line(Line { a: p(-0.2 * s, -0.2 * s), b: p(0.2 * s, 0.2 * s) }));
        push(d, EntityKind::Line(Line { a: p(-0.2 * s, 0.2 * s), b: p(0.2 * s, -0.2 * s) }));
        push(d, EntityKind::Line(Line { a: p(-0.5 * s, -0.25 * s), b: p(-0.5 * s, 0.25 * s) }));
        push(d, EntityKind::Line(Line { a: p(-0.5 * s, 0.0), b: p(-0.3 * s, 0.0) }));
    }

    #[test]
    fn a_legend_rows_drawn_sample_finds_its_loose_copies_turned_or_not() {
        let mut d = Drawing::new_imperial();
        // The legend's sample (its shape is the row's), then the plan's copies: as drawn, a
        // quarter turn (on a wall the other way), and one by a long wall line it touches.
        let lo = d.handseed.max(0x100);
        strobe(&mut d, Vec2::new(500.0, 500.0), 6.0, 0.0);
        let sample: Vec<EntityKind> = d.model.iter().map(|e| e.kind.clone()).collect();
        let mut legend = BTreeMap::new();
        legend.insert(shape_of(sample.iter()).unwrap(), ("WALL STROBE".to_string(), "E-313".to_string()));
        strobe(&mut d, Vec2::new(10.0, 10.0), 6.0, 0.0);
        strobe(&mut d, Vec2::new(60.0, 10.0), 6.0, std::f64::consts::FRAC_PI_2);
        strobe(&mut d, Vec2::new(110.0, 10.0), 6.0, 0.0);
        push(&mut d, EntityKind::Line(Line { a: Vec3::new(107.0, -40.0, 0.0), b: Vec3::new(107.0, 60.0, 0.0) }));
        // Something else of the same size: no row's.
        push(&mut d, EntityKind::Circle(Circle { center: Vec3::new(200.0, 10.0, 0.0), radius: 2.0 }));
        push(&mut d, EntityKind::Line(Line { a: Vec3::new(198.0, 10.0, 0.0), b: Vec3::new(202.0, 10.0, 0.0) }));
        let hi = d.handseed;
        let found = find(&d, lo, hi, &legend, &|w, _| w.contains("STROBE"), 12.0, &[[490.0, 490.0, 510.0, 510.0]]);
        let at: Vec<(i64, i64)> = found.iter().map(|f| (f.at.x.round() as i64, f.at.y.round() as i64)).collect();
        assert_eq!(at.len(), 3, "the three copies, not the sample in the legend: {found:?}");
        assert!(at.contains(&(9, 10)) && at.iter().any(|a| a.0 == 60) && at.iter().any(|a| a.0 == 109), "{at:?}");
        assert!(found.iter().all(|f| f.row == "WALL STROBE" && f.legend_sheet == "E-313" && f.handles.len() == 5), "{found:?}");
        // Rows not wanted: none.
        assert!(find(&d, lo, hi, &legend, &|w, _| w.contains("SMOKE"), 12.0, &[]).is_empty());
        // Hostile sizes and empty legends never panic.
        for s in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let _ = find(&d, lo, hi, &legend, &|_, _| true, s, &[]);
        }
        assert!(find(&d, lo, hi, &BTreeMap::new(), &|_, _| true, 12.0, &[]).is_empty());
    }

    /// A made-up horn/strobe: a box with an X and a circle in it and a flag at its side, at
    /// `c`, its box `s` across, its circle `r`, turned a quarter `q` times.
    fn horn_strobe(d: &mut Drawing, c: Vec2, s: f64, r: f64, q: u8) {
        let p = |x: f64, y: f64| {
            let mut v = Vec2::new(x, y);
            for _ in 0..q {
                v = Vec2::new(-v.y, v.x);
            }
            v + c
        };
        let poly = |pts: &[Vec2], closed: bool| {
            EntityKind::LwPolyline(cadaddy_doc::LwPolyline {
                vertices: pts.iter().map(|v| cadaddy_geom::PolyVertex { p: *v, bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect(),
                closed,
                const_width: 0.0,
                elevation: 0.0,
                plinegen: false,
            })
        };
        let h = s / 2.0;
        push(d, poly(&[p(-h, -h), p(h, -h), p(h, h), p(-h, h)], true));
        for (a, b) in
            [((-h, -h), (-0.25 * s, -0.25 * s)), ((h, h), (0.25 * s, 0.25 * s)), ((-h, h), (-0.25 * s, 0.25 * s)), ((h, -h), (0.25 * s, -0.25 * s))]
        {
            let (a, b) = (p(a.0, a.1), p(b.0, b.1));
            push(d, EntityKind::Line(Line { a: Vec3::new(a.x, a.y, 0.0), b: Vec3::new(b.x, b.y, 0.0) }));
        }
        let ring: Vec<Vec2> = (0..10).map(|i| p(r * (i as f64 * 0.628).cos(), r * (i as f64 * 0.628).sin())).collect();
        push(d, poly(&ring, true));
        push(d, poly(&[p(h, 0.0), p(h + 0.87 * s, h), p(h + 0.87 * s, -h)], true));
    }

    #[test]
    fn a_legend_sample_finds_its_copies_by_their_parts_turned_touching_or_drawn_otherwise() {
        let mut d = Drawing::new_imperial();
        let lo = d.handseed.max(0x100);
        horn_strobe(&mut d, Vec2::new(500.0, 500.0), 24.0, 6.0, 0);
        let sample: Vec<EntityKind> = d.model.iter().map(|e| e.kind.clone()).collect();
        let parts: Vec<&EntityKind> = sample.iter().collect();
        let combo = Template::new(&parts, "COMBINATION HORN & STROBE", "E-313").unwrap();
        // A plain flagged box (a hearing impaired one, its letter is text): only the box and flag.
        let mut d2 = Drawing::new_imperial();
        horn_strobe(&mut d2, Vec2::new(0.0, 0.0), 24.0, 6.0, 0);
        let flagged: Vec<EntityKind> = d2.model.iter().enumerate().filter(|(i, _)| *i == 0 || *i == 6).map(|(_, e)| e.kind.clone()).collect();
        assert!(Template::new(&flagged.iter().collect::<Vec<_>>(), "FLAGGED BOX", "E-313").is_none(), "two objects: no sample");
        // The plan: one as drawn; one a quarter turn with a bigger circle, against the box of a
        // pull station (sharing its side); a box alone and a box with a flag but no X.
        horn_strobe(&mut d, Vec2::new(100.0, 100.0), 24.0, 6.0, 0);
        horn_strobe(&mut d, Vec2::new(300.0, 100.0), 24.0, 7.5, 1);
        let poly = |pts: &[(f64, f64)]| {
            EntityKind::LwPolyline(cadaddy_doc::LwPolyline {
                vertices: pts
                    .iter()
                    .map(|v| cadaddy_geom::PolyVertex { p: Vec2::new(v.0, v.1), bulge: 0.0, start_width: 0.0, end_width: 0.0 })
                    .collect(),
                closed: true,
                const_width: 0.0,
                elevation: 0.0,
                plinegen: false,
            })
        };
        push(&mut d, poly(&[(312.0, 88.0), (336.0, 88.0), (336.0, 112.0), (312.0, 112.0)]));
        push(&mut d, poly(&[(588.0, 88.0), (612.0, 88.0), (612.0, 112.0), (588.0, 112.0)]));
        push(&mut d, poly(&[(688.0, 88.0), (712.0, 88.0), (712.0, 112.0), (688.0, 112.0)]));
        push(&mut d, poly(&[(712.0, 100.0), (732.9, 112.0), (732.9, 88.0)]));
        let hi = d.handseed;
        let found = find_by_parts(&d, lo, hi, std::slice::from_ref(&combo), &[[480.0, 480.0, 520.0, 520.0]]);
        let mut at: Vec<(i64, i64)> = found.iter().map(|f| (f.at.x.round() as i64, f.at.y.round() as i64)).collect();
        at.sort_unstable();
        assert_eq!(at.len(), 2, "{found:?}");
        assert!((at[0].0 - 110).abs() <= 2 && (at[1].0 - 300).abs() <= 2, "{at:?}");
        assert!(found.iter().all(|f| f.row == "COMBINATION HORN & STROBE" && f.legend_sheet == "E-313" && f.handles.len() >= 3), "{found:?}");
        // Hostile input never panics; nothing to look for finds nothing.
        assert!(find_by_parts(&d, lo, hi, &[], &[]).is_empty());
        let mut bad = Drawing::new_imperial();
        push(&mut bad, EntityKind::Line(Line { a: Vec3::new(f64::NAN, 0.0, 0.0), b: Vec3::new(1.0, f64::INFINITY, 0.0) }));
        push(&mut bad, EntityKind::Circle(Circle { center: Vec3::new(0.0, 0.0, 0.0), radius: -1.0 }));
        let _ = find_by_parts(&bad, 0, u64::MAX, std::slice::from_ref(&combo), &[]);
    }
}
