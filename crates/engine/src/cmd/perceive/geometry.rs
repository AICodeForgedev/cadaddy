//! The geometric perceptions: wall face pairs, door swings and opening blocks, and room outlines.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind};
use cadaddy_geom::{Polyline, Segment, Vec2};
use serde_json::{Value, json};

use super::{objects, page, pt, round, texts, units_per_inch};
use crate::cmd::{bool_or, str_param};
use crate::{Result, Session};

/// Straight segments looked at, at most (hostile drawings).
const MAX_SEGS: usize = 4_000_000;
/// Pairs reported at most.
const MAX_PAIRS: usize = 50_000;
/// Rooms traced at most.
const MAX_ROOMS: usize = 1_000;
/// A room's least width, inches (a schedule cell, a tag box and a sliver between two lines are
/// narrower).
const MIN_ROOM_WIDTH_IN: f64 = 36.0;
/// Rooms traced at most on one sheet region, when the labels are read sheet by sheet.
const MAX_ROOMS_PER_REGION: usize = 250;
/// Rooms traced at most over all the sheet regions (a whole print set).
const MAX_ROOMS_ALL_REGIONS: usize = 8_000;
/// A habitable room's least floor area, square feet (IBC 1208.3, IRC R304.1: 70 sf): a smaller
/// loop round a bedroom's or a unit's name is the furniture the name is printed on.
const HABITABLE_MIN_SF: f64 = 70.0;
/// How far from its name a room's number tag may be when it isn't inside the traced outline,
/// in the name's text heights (a tag's two or three lines, the number printed on furniture).
const NUMBER_REACH_HEIGHTS: f64 = 6.0;
/// A room tag's number text height to its name's: at least, at most.
const TAG_SIZE_BAND: (f64, f64) = (0.75, 1.33);
/// Two traced outlines are one room when their areas and bounds agree within this share.
const SAME_OUTLINE_SHARE: f64 = 0.02;

/// A straight segment of a line or polyline.
#[derive(Clone, Debug)]
pub(crate) struct Seg {
    pub a: Vec2,
    pub b: Vec2,
    pub layer: String,
    /// The mean width of the closed outline the segment is a side of (see [`outline_width`]),
    /// infinite for an open line.
    pub outline_width: f64,
    /// A wall's face drawn unit by unit, joined into one ([`join_unit_runs`]): masonry.
    pub unit_run: bool,
}

/// The mean width of a closed outline: twice its area over its perimeter (a `w × l` rectangle
/// gives `w·l / (w + l)`, about `w` when it is long and thin). Infinite when it is no outline.
pub(crate) fn outline_width(pts: &[Vec2]) -> f64 {
    if pts.len() < 3 {
        return f64::INFINITY;
    }
    let perimeter: f64 = pts.iter().zip(pts.iter().cycle().skip(1)).map(|(a, b)| a.dist(*b)).sum();
    let area = cadaddy_geom::shoelace(pts).abs();
    if perimeter.is_finite() && perimeter > 1e-12 && area.is_finite() { 2.0 * area / perimeter } else { f64::INFINITY }
}

/// The straight segments of the space's lines and polylines.
pub(crate) fn segments(ents: &[&std::sync::Arc<Entity>]) -> Vec<Seg> {
    let mut out = Vec::new();
    for e in ents {
        if out.len() >= MAX_SEGS {
            break;
        }
        match &e.kind {
            EntityKind::Line(l) => {
                out.push(Seg { a: l.a.xy(), b: l.b.xy(), layer: e.common.layer.to_string(), outline_width: f64::INFINITY, unit_run: false })
            }
            EntityKind::LwPolyline(p) => {
                // A closed outline (flagged closed, or ending where it starts): how wide it is.
                let pts: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
                let straight = p.vertices.iter().all(|v| v.bulge == 0.0);
                let ends_at_start = pts.len() >= 4 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-9);
                let width = match (straight, p.closed, ends_at_start) {
                    (true, true, _) => outline_width(&pts),
                    (true, false, true) => outline_width(pts.get(..pts.len() - 1).unwrap_or_default()),
                    _ => f64::INFINITY,
                };
                for sg in (Polyline { vertices: p.vertices.clone(), closed: p.closed }).segments() {
                    if let Segment::Line(l) = sg {
                        out.push(Seg { a: l.a, b: l.b, layer: e.common.layer.to_string(), outline_width: width, unit_run: false });
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
    /// The two faces (indices into the segments).
    pub faces: [usize; 2],
}

/// Direction buckets: half a degree each over 180°.
const BUCKETS: i64 = 360;

/// Parallel face pairs: segments in the same direction (within half a degree), their lines
/// `min_thick..max_thick` apart, overlapping along the direction by at least `min_len`.
pub(crate) fn pairs(_d: &Drawing, segs: &[Seg], lim: &Limits) -> Vec<Pair> {
    use std::f64::consts::PI;
    // Group by direction (half-degree buckets over [0°, 180°): a face drawn right to left is the
    // same direction). Faces within half a degree are in one bucket or two neighbouring ones.
    let mut groups: std::collections::HashMap<i64, Vec<usize>> = std::collections::HashMap::new();
    let mut angs = Vec::with_capacity(segs.len());
    for (i, s) in segs.iter().enumerate() {
        let d = s.b - s.a;
        let ang = d.y.atan2(d.x).rem_euclid(PI);
        angs.push(ang);
        let key = ((ang.to_degrees() * 2.0).floor() as i64).rem_euclid(BUCKETS);
        groups.entry(key).or_default().push(i);
    }
    let half_degree = 0.5f64.to_radians();
    let mut keys: Vec<i64> = groups.keys().copied().collect();
    keys.sort_unstable();
    let mut out = Vec::new();
    for key in keys {
        // This bucket with the next one (across 180° → 0° too), sorted by offset along one normal
        // for all of them: the shared edge's. Pairs inside the next bucket are its own turn.
        let next = (key + 1).rem_euclid(BUCKETS);
        let edge = ((key + 1) as f64 / 2.0).to_radians();
        let n = Vec2::new(-edge.sin(), edge.cos());
        let mut g: Vec<(f64, usize, bool)> = Vec::new();
        for (k, home) in [(key, true), (next, false)] {
            for &i in groups.get(&k).map(Vec::as_slice).unwrap_or_default() {
                let s = &segs[i];
                g.push((n.dot(s.a.mid(s.b)), i, home));
            }
        }
        g.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Midpoint offsets along the shared normal are off by up to the faces' length times the
        // sine of their half-degree turn from it; the window allows for that.
        let longest = g.iter().map(|&(_, i, _)| segs[i].a.dist(segs[i].b)).fold(0.0, f64::max);
        let window = lim.max_thick + longest * (2.0 * half_degree).sin();
        for x in 0..g.len() {
            for y in x + 1..g.len() {
                if g[y].0 - g[x].0 > window {
                    break;
                }
                if !g[x].2 && !g[y].2 {
                    continue;
                }
                let (i, j) = (g[x].1, g[y].1);
                let turn = (angs[i] - angs[j]).abs();
                if turn.min(PI - turn) > half_degree {
                    continue;
                }
                let Some(p) = face_pair(&segs[i], &segs[j], lim) else { continue };
                let (s1, s2) = (&segs[i], &segs[j]);
                let shorter = s1.a.dist(s1.b).min(s2.a.dist(s2.b)).max(1e-9);
                let wall_layer = s1.layer.to_ascii_uppercase().contains("WALL");
                let confidence = (0.35
                    + 0.25 * f64::from(u8::from(wall_layer))
                    + 0.25 * (p.length / shorter).min(1.0)
                    + 0.15 * f64::from(u8::from(s1.layer == s2.layer)))
                .min(1.0);
                out.push(Pair { layer: s1.layer.clone(), c0: p.c0, c1: p.c1, thickness: p.gap, length: p.length, confidence, faces: [i, j] });
                if out.len() >= MAX_PAIRS {
                    return out;
                }
            }
        }
    }
    out.sort_by(|a, b| b.confidence.total_cmp(&a.confidence).then(b.length.total_cmp(&a.length)));
    out
}

/// A dash of a dashed line is at most this long, inches (a plan's hidden or overhead line drawn
/// as separate pieces: about ⅛" of paper at the coarsest plan scale).
const DASH_MAX_IN: f64 = 30.0;
/// The space between two dashes is at most this, inches (narrower than any door or window).
const DASH_GAP_MAX_IN: f64 = 12.0;
/// Dashes in a row, at least, before pieces of a line are read as a dashed line.
const DASH_RUN: usize = 3;
/// Dashed lines looked at, at most (hostile drawings).
const MAX_DASH_LINES: usize = 200_000;
/// The longest pattern of pieces a dashed line repeats: a dash (1), dash-dot (2), dash-dot-dot (3).
const DASH_PERIOD_MAX: usize = 3;
/// A gap under this share of the piece beside it is a joint, not a dashed line's space: masonry
/// units drawn one by one are about 3% joint, while a dashed line's spaces are a good part of its
/// dashes.
const JOINT_SHARE: f64 = 0.1;
/// A joint is at most this wide, inches (a mortar joint drawn at any plan's scale).
pub(crate) const JOINT_MAX_IN: f64 = 1.5;

/// Takes out the dashes of dashed lines (hidden or overhead lines a print flattens into separate
/// pieces: a soffit, a header or a wall above). Three or more pieces on one line, each at most
/// [`DASH_MAX_IN`] long and about as long as the next, with about equal gaps of at most
/// [`DASH_GAP_MAX_IN`] between them, are a dashed line: no wall's face (paired with another line
/// it made a wall across a door's opening). So are pieces repeating a pattern of two or three
/// (a dash-dot or dash-dot-dot line: a callout's or a centre line's dashes and dots, each piece
/// about as long as the one a pattern before it) in at least three repeats. A wall's face broken
/// by other walls (pieces much longer) or by doors and windows (gaps wider) is kept. `k` is drawing units per inch. Returns
/// how many pieces were taken out.
pub(crate) fn drop_dashes(segs: &mut Vec<Seg>, k: f64) -> usize {
    use std::f64::consts::PI;
    if !(k.is_finite() && k > 0.0) {
        return 0;
    }
    let (max_len, max_gap, line_tol) = (DASH_MAX_IN * k, DASH_GAP_MAX_IN * k, 0.5 * k);
    // Short pieces grouped by line: direction (half-degree buckets) and offset across it.
    let mut lines: std::collections::HashMap<(i64, i64), Vec<(f64, f64, usize)>> = std::collections::HashMap::new();
    for (i, sg) in segs.iter().enumerate() {
        let d = sg.b - sg.a;
        let len = d.len();
        if !(len > 1e-9 && len <= max_len) {
            continue;
        }
        let ang = d.y.atan2(d.x).rem_euclid(PI);
        let bucket = ((ang.to_degrees() * 2.0).round() as i64).rem_euclid(360);
        let u = Vec2::new((bucket as f64 / 2.0).to_radians().cos(), (bucket as f64 / 2.0).to_radians().sin());
        let n = u.perp();
        let off = (n.dot(sg.a.mid(sg.b)) / line_tol).round() as i64;
        let (s0, s1) = minmax(u.dot(sg.a), u.dot(sg.b));
        if lines.len() < MAX_DASH_LINES || lines.contains_key(&(bucket, off)) {
            lines.entry((bucket, off)).or_default().push((s0, s1, i));
        }
    }
    let mut dash = vec![false; segs.len()];
    let close = |a: f64, b: f64, share: f64, slack: f64| (a - b).abs() <= share * a.max(b) + slack;
    for run in lines.values_mut() {
        if run.len() < DASH_RUN {
            continue;
        }
        run.sort_by(|a, b| a.0.total_cmp(&b.0));
        let rows: Vec<(f64, f64, usize)> = run.clone();
        let len = |j: usize| rows.get(j).map_or(0.0, |r| r.1 - r.0);
        let gap = |j: usize| match (j.checked_sub(1).and_then(|i| rows.get(i)), rows.get(j)) {
            (Some(p), Some(c)) => c.0 - p.1,
            _ => f64::NAN,
        };
        // Grow rows of dashes: each piece about as long as the one a pattern before it (the last
        // for a dashed line, two back for a dash-dot one, three for dash-dot-dot), each gap about
        // as wide.
        for period in 1..=DASH_PERIOD_MAX {
            let mark = |from: usize, to: usize, dash: &mut Vec<bool>| {
                if to + 1 - from > (DASH_RUN - 1) * period {
                    for r in rows.get(from..=to).into_iter().flatten() {
                        if let Some(d) = dash.get_mut(r.2) {
                            *d = true;
                        }
                    }
                }
            };
            let mut start = 0usize;
            for j in 1..rows.len() {
                let g = gap(j);
                let spaced = g > 0.05 * k && g > JOINT_SHARE * len(j - 1).max(len(j)) && g <= max_gap;
                let back = j.checked_sub(period).filter(|&b| b >= start);
                let steady = spaced
                    && back.is_none_or(|b| close(len(b), len(j), 0.15, 0.25 * k))
                    && back.filter(|&b| b > start).is_none_or(|b| close(gap(b), g, 0.25, 0.5 * k));
                if !steady {
                    mark(start, j - 1, &mut dash);
                    start = j;
                }
            }
            mark(start, rows.len() - 1, &mut dash);
        }
    }
    let before = segs.len();
    let mut it = dash.iter();
    segs.retain(|_| !it.next().copied().unwrap_or(false));
    before - segs.len()
}

/// A faceted arc (a print's flattened curve) turns at least this many vertices in a row.
const ARC_TURNS: usize = 3;
/// Each of a faceted arc's turns, degrees: at least (less is a straight line drawn in pieces), at
/// most (more is a corner).
const FACET_TURN_DEG: (f64, f64) = (2.0, 40.0);
/// A rounded corner turns at least this far in all, degrees.
const ROUNDED_TURN_DEG: f64 = 45.0;
/// A faceted arc's chords are at least (shorter ones are a scanned line's jitter) and at most
/// this long, inches.
const FACET_MIN_IN: f64 = 0.25;
const FACET_MAX_IN: f64 = 24.0;
/// The share of a rounded outline's length a wall's other face must run beside for it to be a
/// wall's face ("its whole length", less a little for where its walls meet others).
const PARTNER_SHARE: f64 = 0.95;
/// The share of its arcs' length a rounded wall face's other face runs beside, curved too (concentric).
const ARC_PARTNER_SHARE: f64 = 0.5;
/// How far a partner face may turn from the outline, degrees (a concentric arc's chords meet the
/// outline's at up to half a facet's turn).
const PARTNER_TURN_DEG: f64 = 10.0;
/// A chord is an arc's when both its ends turn at least this much, degrees (a flattened arc's
/// chords turn less the bigger it is).
const ARC_PIECE_TURN_DEG: f64 = 1.0;
/// The outline is probed every this many inches along its length.
const PARTNER_STEP_IN: f64 = 2.0;
/// Rounded outlines looked at, and probes made, at most (hostile drawings).
const MAX_ROUNDED: usize = 20_000;
const MAX_PROBES: usize = 2_000_000;
/// Grid cells a partner line is entered in, at most over all of them.
const MAX_PARTNER_CELLS: usize = 8_000_000;

/// The points of a polyline's path, arcs flattened (within a quarter inch).
fn path_points(p: &cadaddy_doc::LwPolyline, k: f64) -> Vec<Vec2> {
    let mut pts = Polyline { vertices: p.vertices.clone(), closed: p.closed }.tessellate((0.25 * k).max(1e-9));
    if p.closed && pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) > 1e-9) {
        pts.extend(pts.first().copied());
    }
    pts.retain(|v| v.is_finite());
    pts
}

/// Whether each chord of a path is an arc's: both its ends turn a little
/// ([`ARC_PIECE_TURN_DEG`]..[`FACET_TURN_DEG`], a flattened curve) and it is short
/// ([`FACET_MAX_IN`]), not a side between corners. A loop's first and last points are one.
fn arc_chords(pts: &[Vec2], k: f64) -> Vec<bool> {
    let m = pts.len();
    let closed = m > 3 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-9);
    let turns = |i: usize| -> bool {
        let ends = closed && (i == 0 || i + 1 == m);
        let prev = if ends { m.checked_sub(2) } else { i.checked_sub(1) }.and_then(|j| pts.get(j));
        let next = if ends { pts.get(1) } else { pts.get(i + 1) };
        let (Some(&a), Some(&b), Some(&c)) = (prev, pts.get(i), next) else { return false };
        let (u, v) = (b - a, c - b);
        let t = u.cross(v).atan2(u.dot(v)).to_degrees().abs();
        (ARC_PIECE_TURN_DEG..=FACET_TURN_DEG.1).contains(&t)
    };
    pts.windows(2)
        .enumerate()
        .map(|(i, w)| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.dist(*b) <= FACET_MAX_IN * k) && turns(i) && turns(i + 1))
        .collect()
}

/// Whether a polyline has a rounded corner: an arc (a bulge), or a faceted one — at least
/// [`ARC_TURNS`] vertices in a row each turning the same way by [`FACET_TURN_DEG`] between short
/// chords ([`FACET_MIN_IN`]..[`FACET_MAX_IN`]) about as long as each other, [`ROUNDED_TURN_DEG`] or more in all.
fn has_rounded_corner(p: &cadaddy_doc::LwPolyline, k: f64) -> bool {
    let n = p.vertices.len();
    let arcs = if p.closed { n } else { n.saturating_sub(1) };
    if p.vertices.iter().take(arcs).any(|v| v.bulge.is_finite() && v.bulge.abs() > 1e-6) {
        return true;
    }
    let mut pts: Vec<Vec2> = Vec::with_capacity(n);
    for v in &p.vertices {
        if v.p.is_finite() && pts.last().is_none_or(|l: &Vec2| l.dist(v.p) > 1e-6 * k.max(1e-9)) {
            pts.push(v.p);
        }
    }
    let closed = p.closed || (pts.len() > 3 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-6 * k.max(1e-9)));
    if closed && pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-6 * k.max(1e-9)) {
        pts.pop();
    }
    let m = pts.len();
    if m < 3 {
        return false;
    }
    // Each vertex's turn (signed, degrees) and the chord after it; the ends of an open one don't turn.
    let at = |i: usize| pts.get(i % m).copied().unwrap_or_default();
    let turn = |i: usize| -> Option<f64> {
        if !closed && (i == 0 || i + 1 >= m) {
            return None;
        }
        let (a, b, c) = (at(i + m - 1), at(i), at(i + 1));
        let (u, v) = (b - a, c - b);
        let t = u.cross(v).atan2(u.dot(v)).to_degrees();
        t.is_finite().then_some(t)
    };
    let facet = |t: Option<f64>| t.filter(|t| (FACET_TURN_DEG.0..=FACET_TURN_DEG.1).contains(&t.abs()));
    // A closed outline is read from a vertex that is no facet's (all facets: a circle).
    let start = if closed { (0..m).find(|&i| facet(turn(i)).is_none()) } else { Some(0) };
    let Some(start) = start else { return true };
    let (mut run, mut total, mut sign, mut chords) = (0usize, 0.0f64, 0.0f64, (f64::INFINITY, 0.0f64));
    let (min_chord, max_chord) = (FACET_MIN_IN * k, FACET_MAX_IN * k);
    for step in 0..=m {
        let i = start + step;
        let t = if step == m { None } else { facet(turn(i)) };
        // The chord into this vertex, when the last one turned too: inside the arc.
        let chord = at(i).dist(at(i + m - 1));
        let goes_on = t.is_some_and(|t| run == 0 || t.signum() == sign) && (run == 0 || (min_chord..=max_chord).contains(&chord));
        if goes_on {
            if run > 0 {
                chords = (chords.0.min(chord), chords.1.max(chord));
            }
            run += 1;
            total += t.map_or(0.0, f64::abs);
            sign = t.map_or(0.0, f64::signum);
            continue;
        }
        let alike = run < 2 || chords.1 <= 2.0 * chords.0 + 0.1 * k;
        if run >= ARC_TURNS && total >= ROUNDED_TURN_DEG && alike {
            return true;
        }
        // This vertex may begin the next run.
        (run, total, chords) = (0, 0.0, (f64::INFINITY, 0.0));
        if let Some(t) = t {
            run = 1;
            total = t.abs();
            sign = t.signum();
        }
    }
    false
}

/// The polylines with rounded corners that are no wall's face: a rounded outline (an arc or a
/// faceted arc at a corner, see [`has_rounded_corner`]) is a wall's face only where a wall's other
/// face runs beside it, parallel, a wall's thickness away and on one side, for its whole length
/// (the share [`PARTNER_SHARE`]), round its arcs too ([`ARC_PARTNER_SHARE`]): a wall with rounded
/// corners draws both faces round them. One
/// that isn't — an enlarged-plan callout's boundary round a stair, a core or a unit, drawn closed or
/// in pieces (each corner piece a dash running round the arc), a revision cloud, a door swing — is
/// an annotation: paired with a wall's face beside it, it made walls across corridors and in
/// stairs, and a second gap and window where it ran along an outside wall. `ents` are the objects
/// the faces come from (any other one may be the partner), `k` drawing units per inch. Returns the
/// handles of the polylines to leave out.
pub(crate) fn rounded_outlines_alone(ents: &[&std::sync::Arc<Entity>], k: f64, lim: &Limits) -> std::collections::HashSet<cadaddy_doc::Handle> {
    use std::collections::{HashMap, HashSet};
    let mut out = HashSet::new();
    if !(k.is_finite() && k > 0.0 && lim.max_thick.is_finite() && lim.max_thick > 0.0) {
        return out;
    }
    let rounded: Vec<(usize, Vec<Vec2>)> = ents
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match &e.kind {
            EntityKind::LwPolyline(p) if p.vertices.len() >= 3 && has_rounded_corner(p, k) => Some((i, path_points(p, k))),
            _ => None,
        })
        .take(MAX_ROUNDED)
        .collect();
    if rounded.is_empty() {
        return out;
    }
    // The probes along each outline: (point, direction, length it stands for, on an arc).
    let step = (PARTNER_STEP_IN * k).max(1e-9);
    let mut budget = MAX_PROBES;
    let probes: Vec<Vec<(Vec2, Vec2, f64, bool)>> = rounded
        .iter()
        .map(|(_, pts)| {
            let mut v = Vec::new();
            for (w, arc) in pts.windows(2).zip(arc_chords(pts, k)) {
                let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else { continue };
                let len = a.dist(b);
                if len <= 1e-9 || budget == 0 {
                    continue;
                }
                let n = ((len / step).ceil() as usize).clamp(1, budget);
                budget -= n;
                let u = (b - a) * (1.0 / len);
                v.extend((0..n).map(|j| (a + (b - a) * ((j as f64 + 0.5) / n as f64), u, len / n as f64, arc)));
            }
            v
        })
        .collect();
    // The other objects' straight pieces near the probes (and whether each is an arc's chord), by
    // grid cell.
    let cell = lim.max_thick.max(1e-9);
    let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<(Vec2, Vec2, bool, usize)>> = HashMap::new();
    for p in probes.iter().flatten() {
        let (cx, cy) = key(p.0);
        for dx in -1..=1 {
            for dy in -1..=1 {
                grid.entry((cx + dx, cy + dy)).or_default();
            }
        }
    }
    let mut cells_left = MAX_PARTNER_CELLS;
    'ents: for (i, e) in ents.iter().enumerate() {
        let pieces: Vec<(Vec2, Vec2, bool)> = match &e.kind {
            EntityKind::Line(l) => vec![(l.a.xy(), l.b.xy(), false)],
            EntityKind::LwPolyline(p) => {
                let pts = path_points(p, k);
                pts.windows(2).zip(arc_chords(&pts, k)).filter_map(|(w, arc)| Some((*w.first()?, *w.get(1)?, arc))).collect()
            }
            _ => continue,
        };
        for (a, b, arc) in pieces {
            if !(a.is_finite() && b.is_finite()) || a.dist(b) <= 1e-9 {
                continue;
            }
            // The cells along it (half a cell apart), entered where a probe looks.
            let n = ((a.dist(b) / (0.5 * cell)).ceil() as usize).saturating_add(1);
            if n > cells_left {
                break 'ents;
            }
            cells_left -= n;
            let mut last = None;
            for j in 0..=n {
                let c = key(a + (b - a) * (j as f64 / n as f64));
                if last == Some(c) {
                    continue;
                }
                last = Some(c);
                if let Some(list) = grid.get_mut(&c) {
                    list.push((a, b, arc, i));
                }
            }
        }
    }
    let max_turn = PARTNER_TURN_DEG.to_radians().sin();
    for ((own, _), probes) in rounded.iter().zip(&probes) {
        // The length (all, and on its arcs) with a partner on each side: a wall's other face is on
        // one side all along (a leader or a wall's face across the corner is no partner), and
        // round an arc it is an arc too (a square corner of a wall the outline runs round is none).
        let (mut all, mut arcs) = (0.0f64, 0.0f64);
        let mut beside = [[0.0f64; 2]; 2];
        for &(p, u, w, arc) in probes {
            all += w;
            if arc {
                arcs += w;
            }
            let (cx, cy) = key(p);
            let mut sides = [(false, false); 2];
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for &(a, b, curved, i) in grid.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                        if i == *own {
                            continue;
                        }
                        let d = b - a;
                        let len = d.len();
                        if len <= 1e-9 || (d * (1.0 / len)).cross(u).abs() > max_turn {
                            continue;
                        }
                        // The nearest point on it lies across the outline, a wall's thickness off.
                        let t = ((p - a).dot(d) / (len * len)).clamp(0.0, 1.0);
                        let off = a + d * t - p;
                        let gap = off.len();
                        if (lim.min_thick..=lim.max_thick).contains(&gap)
                            && off.dot(u).abs() <= 0.5 * gap
                            && let Some(s) = sides.get_mut(usize::from(u.cross(off) < 0.0))
                        {
                            *s = (true, s.1 || curved);
                        }
                    }
                }
            }
            for (side, &(found, curved)) in sides.iter().enumerate() {
                if found && let Some(b) = beside.get_mut(side) {
                    b[0] += w;
                    if arc && curved {
                        b[1] += w;
                    }
                }
            }
        }
        let faced = all > 1e-9 && beside.iter().any(|b| b[0] >= PARTNER_SHARE * all && (arcs <= 1e-9 || b[1] >= ARC_PARTNER_SHARE * arcs));
        if !faced && let Some(e) = ents.get(*own) {
            out.insert(e.handle);
        }
    }
    out
}

/// Joins the faces of a wall drawn unit by unit (masonry blocks, each face its own piece): three or
/// more pieces on one line, about the same length, with joints of at most [`JOINT_MAX_IN`] and
/// under [`JOINT_SHARE`] of a piece between them, become one line, so the wall's one wall and not
/// a row of block-long ones. Each run keeps its first piece's layer. `k` is drawing units per
/// inch. Returns how many pieces were merged away.
pub(crate) fn join_unit_runs(segs: &mut Vec<Seg>, k: f64) -> usize {
    use std::f64::consts::PI;
    if !(k.is_finite() && k > 0.0) {
        return 0;
    }
    let (max_len, max_joint, line_tol) = (DASH_MAX_IN * k, JOINT_MAX_IN * k, 0.5 * k);
    let mut lines: std::collections::HashMap<(i64, i64), Vec<(f64, f64, usize)>> = std::collections::HashMap::new();
    for (i, sg) in segs.iter().enumerate() {
        let d = sg.b - sg.a;
        let len = d.len();
        if !(len > 1e-9 && len <= max_len) {
            continue;
        }
        let ang = d.y.atan2(d.x).rem_euclid(PI);
        let bucket = ((ang.to_degrees() * 2.0).round() as i64).rem_euclid(360);
        let u = Vec2::new((bucket as f64 / 2.0).to_radians().cos(), (bucket as f64 / 2.0).to_radians().sin());
        let off = (u.perp().dot(sg.a.mid(sg.b)) / line_tol).round() as i64;
        let (s0, s1) = minmax(u.dot(sg.a), u.dot(sg.b));
        if lines.len() < MAX_DASH_LINES || lines.contains_key(&(bucket, off)) {
            lines.entry((bucket, off)).or_default().push((s0, s1, i));
        }
    }
    // Each run: its units in order along the line, then the short pieces lying on them.
    let mut runs: Vec<(Vec<usize>, Vec<usize>)> = Vec::new();
    for run in lines.values_mut() {
        runs.extend(unit_runs_on_line(run, max_joint, k));
    }
    let mut gone = vec![false; segs.len()];
    let mut merged = 0usize;
    // In a fixed order: which line a run's kept piece is doesn't depend on the map's.
    runs.sort_by_key(|r| r.0.first().copied());
    for (units, on) in &runs {
        let (Some(&first), Some(&last)) = (units.first(), units.last()) else { continue };
        let Some(end) = segs.get(last).cloned() else { continue };
        let Some(head) = segs.get(first).cloned() else { continue };
        // The far ends of the first and last pieces, as they run along the line.
        let u = (end.a.mid(end.b)) - (head.a.mid(head.b));
        let along = |p: Vec2| u.dot(p);
        let start = if along(head.a) <= along(head.b) { head.a } else { head.b };
        let stop = if along(end.a) >= along(end.b) { end.a } else { end.b };
        if let Some(h) = segs.get_mut(first) {
            h.a = start;
            h.b = stop;
            // No more one outline's side: the whole face of a wall.
            h.outline_width = f64::INFINITY;
            h.unit_run = true;
        }
        for &i in units.iter().skip(1).chain(on) {
            if let Some(g) = gone.get_mut(i).filter(|g| !**g) {
                *g = true;
                merged += 1;
            }
        }
    }
    let mut it = gone.iter();
    segs.retain(|_| !it.next().copied().unwrap_or(false));
    merged
}

/// The runs of units on one line (its pieces as `(start, end, index)` along it): each run's units
/// in order, then the pieces lying on them. Units follow each other a joint apart and about the same
/// length; a unit cut to fit (at a corner, an end or an opening) may start or end a run. A piece
/// lying on a unit (a speck, a bit of a hatch or another line on the face, the face drawn twice)
/// or a short one in the joint after it neither ends the run nor counts as a unit.
fn unit_runs_on_line(run: &mut [(f64, f64, usize)], max_joint: f64, k: f64) -> Vec<(Vec<usize>, Vec<usize>)> {
    type Piece = (f64, f64, usize);
    // Along the line; of two starting together, the longer first (a unit before a speck on it).
    run.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.total_cmp(&a.1)));
    let joint = |p: Piece, s: Piece| {
        let gap = s.0 - p.1;
        gap >= 0.0 && gap <= max_joint && gap < JOINT_SHARE * (p.1 - p.0).max(s.1 - s.0)
    };
    let alike = |a: f64, b: f64| (a - b).abs() <= 0.15 * a.max(b) + 0.25 * k;
    // A unit cut to fit: longer than a joint, no longer than the run's units.
    let cut = |len: f64, unit: f64| len >= max_joint && len <= unit + 0.15 * unit + 0.25 * k;
    // A run of at least DASH_RUN units, with the cut units a joint before and after it; and
    // whether it took the piece after it.
    let finish = |cur: &[Piece], on: &[usize], before: Option<Piece>, after: Option<Piece>| -> Option<(Vec<usize>, Vec<usize>, bool)> {
        let (Some(&f), Some(&l)) = (cur.first(), cur.last()) else { return None };
        if cur.len() < DASH_RUN {
            return None;
        }
        let mut units: Vec<usize> = before.filter(|b| joint(*b, f) && cut(b.1 - b.0, f.1 - f.0)).map(|b| b.2).into_iter().collect();
        units.extend(cur.iter().map(|c| c.2));
        let took = after.filter(|a| joint(l, *a) && cut(a.1 - a.0, l.1 - l.0)).map(|a| units.push(a.2)).is_some();
        Some((units, on.to_vec(), took))
    };
    let mut out = Vec::new();
    let mut cur: Vec<Piece> = Vec::new();
    let mut on: Vec<usize> = Vec::new();
    // The piece just before `cur` began, when it stood alone (a cut unit at the run's start, maybe).
    let mut before: Option<Piece> = None;
    for &p in run.iter() {
        if let Some(&last) = cur.last() {
            let (len, unit) = (p.1 - p.0, last.1 - last.0);
            if p.1 <= last.1 + 0.25 * k || (len < 0.5 * unit && p.1 <= last.1 + max_joint) {
                on.push(p.2);
                continue;
            }
            if joint(last, p) && alike(unit, len) {
                cur.push(p);
                continue;
            }
            // The run ends here (taking this piece when it's a cut unit).
            let lone = cur.first().copied().filter(|_| cur.len() == 1);
            let done = finish(&cur, &on, before, Some(p));
            cur.clear();
            on.clear();
            before = None;
            match done {
                Some((units, specks, took)) => {
                    out.push((units, specks));
                    if took {
                        continue;
                    }
                }
                None => before = lone,
            }
        }
        cur.push(p);
    }
    if let Some((units, specks, _)) = finish(&cur, &on, before, None) {
        out.push((units, specks));
    }
    out
}

/// The lines of the runs [`join_unit_runs`] joins: each wall face drawn unit by unit, end to end.
pub(crate) fn unit_run_lines(segs: &[Seg], k: f64) -> Vec<(Vec2, Vec2)> {
    let mut joined = segs.to_vec();
    if join_unit_runs(&mut joined, k) == 0 {
        return Vec::new();
    }
    // A run is longer than any piece of one. (The pieces as they were, looked up by their ends:
    // a plan of a whole storey has hundreds of thousands, each run checked against them all
    // would take hours.)
    let bits = |p: Vec2| ((p.x + 0.0).to_bits(), (p.y + 0.0).to_bits());
    let drawn: std::collections::HashSet<_> = segs.iter().map(|o| (bits(o.a), bits(o.b))).collect();
    joined.iter().filter(|g| g.a.dist(g.b) > DASH_MAX_IN * k && !drawn.contains(&(bits(g.a), bits(g.b)))).map(|g| (g.a, g.b)).collect()
}

/// Drops the pairs a thin outline's side makes with another wall's face. A side of a closed
/// outline thinner than any wall (a trim piece, a shelf or a jamb drawn beside a wall) paired
/// with a line that is already the face of a wall on its far side is no wall: one line can't be
/// a wall's face on both of its sides, so the space between them is air the plan leaves open.
/// A thin outline's side paired with a line that bounds no other wall stays (real faces are drawn
/// as thin closed outlines too), and so does one whose far wall has a thin outline's side for a
/// face itself (two walls drawn with thin finish layers, side by side). Returns how many were
/// dropped.
pub(crate) fn drop_sliver_pairs(segs: &[Seg], found: &mut Vec<Pair>, lim: &Limits) -> usize {
    let thin = |i: usize| segs.get(i).is_some_and(|s| s.outline_width < lim.min_thick);
    if !found.iter().any(|p| thin(p.faces[0]) || thin(p.faces[1])) {
        return 0;
    }
    // Which pairs each line is a face of.
    let mut by_face: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for (k, p) in found.iter().enumerate() {
        for f in p.faces {
            by_face.entry(f).or_default().push(k);
        }
    }
    let mut drop = vec![false; found.len()];
    for (k, p) in found.iter().enumerate() {
        for (t, f) in [(p.faces[0], p.faces[1]), (p.faces[1], p.faces[0])] {
            if !thin(t) {
                continue;
            }
            let (Some(ts), Some(fs)) = (segs.get(t), segs.get(f)) else { continue };
            let here = side_of(fs, ts);
            let far_wall = by_face.get(&f).into_iter().flatten().any(|&q| {
                q != k
                    && found.get(q).is_some_and(|qp| {
                        let other = if qp.faces[0] == f { qp.faces[1] } else { qp.faces[0] };
                        !thin(other) && segs.get(other).is_some_and(|os| side_of(fs, os) * here < 0.0) && run_overlap(p, qp) > 0.5 * p.length
                    })
            });
            if far_wall {
                drop[k] = true;
                break;
            }
        }
    }
    let before = found.len();
    let mut it = drop.iter();
    found.retain(|_| !it.next().copied().unwrap_or(false));
    before - found.len()
}

/// The side of the line `f` a segment's middle is on (+1 / −1, 0 on it).
fn side_of(f: &Seg, s: &Seg) -> f64 {
    let d = f.b - f.a;
    let len = d.len();
    if len <= 1e-12 {
        return 0.0;
    }
    let v = d.cross(s.a.mid(s.b) - f.a) / len;
    if v.abs() <= 1e-9 { 0.0 } else { v.signum() }
}

/// How far two pairs run side by side along the first one's centreline.
fn run_overlap(p: &Pair, q: &Pair) -> f64 {
    let len = p.c0.dist(p.c1);
    if len <= 1e-12 {
        return 0.0;
    }
    let u = (p.c1 - p.c0) * (1.0 / len);
    let (a, b) = minmax(u.dot(q.c0 - p.c0), u.dot(q.c1 - p.c0));
    (b.min(len) - a.max(0.0)).max(0.0)
}

/// A symbol's outline is at most this deep across the pair it holds, inches (a tub, a counter, a
/// through-wall unit with its sleeve): a pair closed off by lines running further is closed off
/// by other walls' faces.
const SYMBOL_DEPTH_MAX_IN: f64 = 96.0;
/// How far past a pair's end the outline holding it may close it off, inches (a tub's rounded
/// corner and rim beyond the straight of its basin).
const CAP_REACH_IN: f64 = 12.0;
/// How far a wall may stop short of the next one on its line, inches: an opening between them.
const OPENING_REACH_IN: f64 = 72.0;
/// Marks looked at, at most (hostile drawings).
const MAX_MARKS: usize = 1_000_000;
/// Cells one item of a [`CellIndex`] is filed under, at most; a bigger one is looked at always.
const MAX_ITEM_CELLS: i64 = 400;

/// Something drawn that a symbol's outline holds and a wall's doesn't (Q6423): lettering (its
/// middle), a fill, a curve (a drain, a basin's rim) or a closed inner outline (a basin, the
/// openings of a grille). Its extents on the plan.
pub(crate) struct Mark {
    pub lo: Vec2,
    pub hi: Vec2,
    /// A fill (hatch or solid): it may fill the outline right to its sides.
    pub fill: bool,
}

/// The [`Mark`]s among a plan's objects. `k` is drawing units per inch: a closed outline or a
/// curve under ¼" (a speck of a stipple) is no mark.
pub(crate) fn symbol_marks(d: &Drawing, ents: &[&std::sync::Arc<Entity>], k: f64) -> Vec<Mark> {
    let least = 0.25 * k;
    let mut out = Vec::new();
    for e in ents {
        if out.len() >= MAX_MARKS {
            break;
        }
        // (kind: 0 lettering, 1 fill, 2 curve or outline)
        let kind = match &e.kind {
            EntityKind::Text(_) | EntityKind::MText(_) | EntityKind::AttDef(_) => 0,
            EntityKind::Hatch(_) | EntityKind::Solid(_) | EntityKind::Trace(_) => 1,
            EntityKind::Circle(_) | EntityKind::Arc(_) | EntityKind::Ellipse(_) => 2,
            EntityKind::LwPolyline(p) => {
                let ends_at_start = p.vertices.len() >= 4 && p.vertices.first().zip(p.vertices.last()).is_some_and(|(a, b)| a.p.dist(b.p) <= 1e-9);
                if !(p.closed || ends_at_start) || p.vertices.len() < 3 {
                    continue;
                }
                2
            }
            _ => continue,
        };
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() || !b.min.is_finite() || !b.max.is_finite() {
            continue;
        }
        let size = b.max - b.min;
        match kind {
            0 => {
                let c = b.center();
                out.push(Mark { lo: c, hi: c, fill: false });
            }
            1 => out.push(Mark { lo: b.min, hi: b.max, fill: true }),
            _ if size.x.min(size.y) >= least && size.x.max(size.y) >= 4.0 * least => out.push(Mark { lo: b.min, hi: b.max, fill: false }),
            _ => {}
        }
    }
    out
}

/// Items (by their extents) filed by grid cell, to find the ones near a place.
struct CellIndex {
    cell: f64,
    cells: std::collections::HashMap<(i64, i64), Vec<usize>>,
    /// Items over [`MAX_ITEM_CELLS`] cells: near everything.
    wide: Vec<usize>,
}

impl CellIndex {
    fn new<I: IntoIterator<Item = (Vec2, Vec2)>>(cell: f64, items: I) -> CellIndex {
        let mut ix = CellIndex { cell, cells: std::collections::HashMap::new(), wide: Vec::new() };
        for (i, (lo, hi)) in items.into_iter().enumerate() {
            let ((x0, y0), (x1, y1)) = (ix.at(lo), ix.at(hi));
            if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > MAX_ITEM_CELLS {
                ix.wide.push(i);
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    ix.cells.entry((x, y)).or_default().push(i);
                }
            }
        }
        ix
    }

    fn at(&self, p: Vec2) -> (i64, i64) {
        let c = |v: f64| (v / self.cell).floor().clamp(-1e15, 1e15) as i64;
        (c(p.x), c(p.y))
    }

    /// The items whose cells meet the box `lo`–`hi`, each once, in order.
    fn near(&self, lo: Vec2, hi: Vec2) -> Vec<usize> {
        let ((x0, y0), (x1, y1)) = (self.at(lo), self.at(hi));
        let mut out = self.wide.clone();
        if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) <= MAX_ITEM_CELLS * 4 {
            for x in x0..=x1 {
                for y in y0..=y1 {
                    out.extend(self.cells.get(&(x, y)).into_iter().flatten());
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// A pair's frame: along its centreline from `c0` (`s`) and across it (`v`).
#[derive(Clone, Copy)]
struct Frame {
    c0: Vec2,
    u: Vec2,
    n: Vec2,
    len: f64,
}

impl Frame {
    fn of(p: &Pair) -> Option<Frame> {
        let len = p.c0.dist(p.c1);
        if !(len > 1e-9 && len.is_finite() && p.thickness.is_finite()) {
            return None;
        }
        let u = (p.c1 - p.c0) * (1.0 / len);
        Some(Frame { c0: p.c0, u, n: u.perp(), len })
    }

    fn sv(&self, q: Vec2) -> (f64, f64) {
        (self.u.dot(q - self.c0), self.n.dot(q - self.c0))
    }
}

/// The outline a pair lies in: from `s0` to `s1` along it, `v0` to `v1` across it.
#[derive(Clone, Copy)]
struct SymbolBox {
    f: Frame,
    s0: f64,
    s1: f64,
    v0: f64,
    v1: f64,
}

impl SymbolBox {
    /// Whether `q` is in the box grown by `m` (shrunk, when negative).
    fn holds(&self, q: Vec2, m: f64) -> bool {
        let (s, v) = self.f.sv(q);
        s >= self.s0 - m && s <= self.s1 + m && v >= self.v0 - m && v <= self.v1 + m
    }

    /// Its extents on the plan.
    fn bounds(&self) -> (Vec2, Vec2) {
        let f = &self.f;
        let at = |s: f64, v: f64| f.c0 + f.u * s + f.n * v;
        let pts = [at(self.s0, self.v0), at(self.s0, self.v1), at(self.s1, self.v0), at(self.s1, self.v1)];
        let lo = pts.iter().fold(Vec2::new(f64::INFINITY, f64::INFINITY), |a, p| a.min(*p));
        let hi = pts.iter().fold(Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY), |a, p| a.max(*p));
        (lo, hi)
    }
}

/// The line closing a pair off at one end: a line across both its faces (three quarters of the
/// way at least) at most [`CAP_REACH_IN`] past the end and an inch short of it, drawn in one or
/// more pieces; the nearest. Or, with no such line, both faces stopping there (a print may leave
/// an outline's side out). Where along the pair it stands, and how far across it runs either way
/// (`v` from, to).
fn pair_cap(segs: &[Seg], near: &CellIndex, p: &Pair, f: &Frame, at_end: bool, k: f64) -> Option<(f64, f64, f64)> {
    let h = p.thickness / 2.0;
    let (inward, outward) = (k, CAP_REACH_IN * k);
    let s_end = if at_end { f.len } else { 0.0 };
    let (lo, hi) = if at_end { (s_end - inward, s_end + outward) } else { (s_end - outward, s_end + inward) };
    let e = f.c0 + f.u * s_end;
    let r = outward + h + k;
    // The pieces across the pair there: where each stands along it, and its span across.
    let mut across: Vec<(f64, f64, f64)> = Vec::new();
    for i in near.near(e - Vec2::new(r, r), e + Vec2::new(r, r)) {
        if p.faces.contains(&i) {
            continue;
        }
        let Some(sg) = segs.get(i) else { continue };
        let d = sg.b - sg.a;
        let l = d.len();
        if l <= 1e-9 || (f.u.dot(d) / l).abs() > 0.5 {
            continue;
        }
        let ((sa, va), (sb, vb)) = (f.sv(sg.a), f.sv(sg.b));
        if sa.min(sb) < lo || sa.max(sb) > hi {
            continue;
        }
        across.push(((sa + sb) / 2.0, va.min(vb), va.max(vb)));
    }
    across.sort_by(|a, b| a.0.total_cmp(&b.0));
    let line_tol = 0.5 * k;
    let mut best: Option<(f64, f64, f64)> = None;
    let mut i = 0;
    while i < across.len() {
        // The pieces on one line across.
        let mut j = i + 1;
        while across.get(j).zip(across.get(j - 1)).is_some_and(|(b, a)| b.0 - a.0 <= line_tol) {
            j += 1;
        }
        let Some(line) = across.get(i..j) else { break };
        let s = line.iter().map(|x| x.0).sum::<f64>() / line.len() as f64;
        let mut spans: Vec<(f64, f64)> = line.iter().map(|x| (x.1, x.2)).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut run: Option<(f64, f64)> = None;
        for (a, b) in spans.into_iter().chain(std::iter::once((f64::INFINITY, f64::INFINITY))) {
            match run {
                Some((r0, r1)) if a <= r1 + line_tol => run = Some((r0, r1.max(b))),
                Some((r0, r1)) => {
                    let covers = r1.min(h) - r0.max(-h);
                    if covers >= 0.75 * p.thickness && best.is_none_or(|bst| (s - s_end).abs() < (bst.0 - s_end).abs()) {
                        best = Some((s, r0, r1));
                    }
                    run = Some((a, b));
                }
                None => run = Some((a, b)),
            }
        }
        i = j;
    }
    // Both faces stopping there close it all the same.
    let stops = |i: usize| {
        segs.get(i).is_some_and(|sg| {
            let (sa, sb) = (f.u.dot(sg.a - f.c0), f.u.dot(sg.b - f.c0));
            let s = if at_end { sa.max(sb) } else { sa.min(sb) };
            (s - s_end).abs() <= inward
        })
    };
    best.or_else(|| (stops(p.faces[0]) && stops(p.faces[1])).then_some((s_end, -h, h)))
}

/// The outline holding a pair when it is a symbol's (Q6423): closed off at both ends
/// ([`pair_cap`]), no deeper than [`SYMBOL_DEPTH_MAX_IN`], with a [`Mark`] inside (a fill may fill
/// it to its sides). A wall's lines hold none of these; a masonry wall's faces drawn unit by unit
/// hold the units' cores, and are no symbol's.
fn symbol_box(segs: &[Seg], near: &CellIndex, marks: &[Mark], marks_near: &CellIndex, p: &Pair, k: f64) -> Option<SymbolBox> {
    // A face drawn unit by unit is masonry's, its cores no symbol's marks.
    if p.faces.iter().any(|&i| segs.get(i).is_some_and(|s| s.unit_run)) {
        return None;
    }
    let f = Frame::of(p)?;
    let h = p.thickness / 2.0;
    let (s0, a0, b0) = pair_cap(segs, near, p, &f, false, k)?;
    let (s1, a1, b1) = pair_cap(segs, near, p, &f, true, k)?;
    let (v0, v1) = ((-h).min(a0.max(a1)), h.max(b0.min(b1)));
    if v1 - v0 > SYMBOL_DEPTH_MAX_IN * k || s1 <= s0 {
        return None;
    }
    let bx = SymbolBox { f, s0, s1, v0, v1 };
    let m = 0.5 * k;
    let (lo, hi) = bx.bounds();
    let corners = |mk: &Mark| [mk.lo, Vec2::new(mk.lo.x, mk.hi.y), Vec2::new(mk.hi.x, mk.lo.y), mk.hi];
    let held = marks_near.near(lo, hi).into_iter().filter_map(|i| marks.get(i)).any(|mk| {
        let grow = if mk.fill { m } else { -m };
        corners(mk).iter().all(|&q| bx.holds(q, grow))
    });
    held.then_some(bx)
}

/// Whether `q` reads the same wall as `p` another way: parallel, on its line within half the
/// thicker one, over half the shorter.
fn same_reading(p: &Pair, q: &Pair) -> bool {
    let (Some(f), Some(g)) = (Frame::of(p), Frame::of(q)) else { return false };
    if f.u.cross(g.u).abs() > 0.035 || f.n.dot(q.c0.mid(q.c1) - p.c0).abs() > p.thickness.max(q.thickness) / 2.0 {
        return false;
    }
    let (a, b) = (f.u.dot(q.c0 - p.c0), f.u.dot(q.c1 - p.c0));
    b.max(a).min(f.len) - a.min(b).max(0.0) > 0.5 * f.len.min(g.len)
}

/// Whether the pair `p` is part of the walls through `q`: an end of it in `q` (a corner or a
/// tee), or `q` on its line beyond an end, an opening's width at most away, much as thick.
fn joins(p: &Pair, f: &Frame, q: &Pair, k: f64) -> bool {
    let Some(g) = Frame::of(q) else { return false };
    let cross = f.u.cross(g.u).abs();
    if cross >= 0.34 {
        let reach = q.thickness / 2.0 + (2.0 * k).max(0.5 * p.thickness);
        return [p.c0, p.c1].iter().any(|&e| {
            let t = g.u.dot(e - q.c0).clamp(0.0, g.len);
            e.dist(q.c0 + g.u * t) <= reach
        });
    }
    if cross > 0.035 || f.n.dot(q.c0.mid(q.c1) - p.c0).abs() > p.thickness.max(q.thickness) / 2.0 {
        return false;
    }
    let (thin, thick) = (p.thickness.min(q.thickness), p.thickness.max(q.thickness));
    if thick > 1.5 * thin {
        return false;
    }
    let (a, b) = (f.u.dot(q.c0 - p.c0), f.u.dot(q.c1 - p.c0));
    let gap = (a.min(b) - f.len).max(-a.max(b));
    (-2.0 * k..=OPENING_REACH_IN * k).contains(&gap)
}

/// Drops the pairs read off symbols (Q6423): equipment and fixtures drawn on the plan (a
/// through-wall unit's outline and grille, a tub's front and basin, a counter round its sink, a
/// tag's frame round its number) pair up like walls' faces. A pair closed off at both ends by an
/// outline that holds lettering, a fill, a curve or an inner outline ([`symbol_box`]) is a
/// symbol's when neither it nor any pair inside that outline joins a wall outside it (an end in
/// another wall, a wall on its line beyond an end): then none of those is a wall, nor the other
/// readings of them with a face inside that join no wall. Then a symbol's line paired with a
/// line that is a wall's face on its far side, or with one that joins no wall, is no wall either.
/// Walls join other walls or the shell; the symbols are left to the equipment and plumbing
/// readers. `marks` are the plan's
/// [`symbol_marks`]; `k` is drawing units per inch. Returns how many pairs were dropped.
pub(crate) fn drop_symbol_pairs(segs: &[Seg], found: &mut Vec<Pair>, marks: &[Mark], k: f64) -> usize {
    if found.is_empty() || marks.is_empty() || !(k.is_finite() && k > 0.0) {
        return 0;
    }
    let cell = 24.0 * k;
    let seg_ix = CellIndex::new(cell, segs.iter().map(|s| (s.a.min(s.b), s.a.max(s.b))));
    let mark_ix = CellIndex::new(cell, marks.iter().map(|m| (m.lo, m.hi)));
    let pair_ix = CellIndex::new(
        cell,
        found.iter().map(|p| {
            let h = Vec2::new(p.thickness / 2.0, p.thickness / 2.0);
            (p.c0.min(p.c1) - h, p.c0.max(p.c1) + h)
        }),
    );
    let m = 0.5 * k;
    let face_in = |bx: &SymbolBox, i: usize| segs.get(i).is_some_and(|s| bx.holds(s.a, m) && bx.holds(s.b, m));
    let inside = |bx: &SymbolBox, q: &Pair| face_in(bx, q.faces[0]) && face_in(bx, q.faces[1]);
    let mut drop = vec![false; found.len()];
    let mut boxes: Vec<SymbolBox> = Vec::new();
    for (i, p) in found.iter().enumerate() {
        if drop.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(bx) = symbol_box(segs, &seg_ix, marks, &mark_ix, p, k) else { continue };
        // The symbol: the pairs inside its outline, and the other readings of those with a face
        // inside it.
        let (lo, hi) = bx.bounds();
        let near = pair_ix.near(lo - Vec2::new(cell, cell), hi + Vec2::new(cell, cell));
        let mut members: Vec<usize> = near.iter().copied().filter(|&j| j == i || found.get(j).is_some_and(|q| inside(&bx, q))).collect();
        if !members.contains(&i) {
            members.push(i);
        }
        let readings: Vec<usize> = near
            .iter()
            .copied()
            .filter(|j| !members.contains(j))
            .filter(|&j| {
                found.get(j).is_some_and(|q| {
                    (face_in(&bx, q.faces[0]) || face_in(&bx, q.faces[1])) && members.iter().filter_map(|&x| found.get(x)).any(|r| same_reading(r, q))
                })
            })
            .collect();
        // A wall outside the outline that one of them joins: the outline is a wall's. A reading
        // that joins one is that wall's (its face runs along the symbol), and stays.
        let joins_out = |a: usize| {
            let (Some(pa), Some(fa)) = (found.get(a), found.get(a).and_then(Frame::of)) else { return false };
            let reach = Vec2::new(OPENING_REACH_IN * k + pa.thickness, OPENING_REACH_IN * k + pa.thickness);
            pair_ix
                .near(pa.c0.min(pa.c1) - reach, pa.c0.max(pa.c1) + reach)
                .into_iter()
                .filter(|j| !members.contains(j) && !readings.contains(j))
                .filter_map(|j| found.get(j))
                .any(|q| !inside(&bx, q) && joins(pa, &fa, q, k))
        };
        if members.iter().any(|&a| joins_out(a)) {
            continue;
        }
        let kept: Vec<usize> = readings.iter().copied().filter(|&a| joins_out(a)).collect();
        for j in members.into_iter().chain(readings.into_iter().filter(|j| !kept.contains(j))) {
            if let Some(d) = drop.get_mut(j) {
                *d = true;
            }
        }
        boxes.push(bx);
    }
    // A symbol's line paired with a line that is a wall's face on its far side (the unit's
    // outline and the glazing line of the window it stands under): one line can't be a wall's
    // face on both sides, so the space between is the room's.
    if !boxes.is_empty() {
        let box_ix = CellIndex::new(cell, boxes.iter().map(SymbolBox::bounds));
        let mut by_face: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
        for (j, q) in found.iter().enumerate().filter(|(j, _)| !drop.get(*j).copied().unwrap_or(true)) {
            for f in q.faces {
                by_face.entry(f).or_default().push(j);
            }
        }
        let mut beside = Vec::new();
        for (j, q) in found.iter().enumerate().filter(|(j, _)| !drop.get(*j).copied().unwrap_or(true)) {
            let h = Vec2::new(q.thickness, q.thickness);
            let near = box_ix.near(q.c0.min(q.c1) - h, q.c0.max(q.c1) + h);
            for (sym, wall) in [(q.faces[0], q.faces[1]), (q.faces[1], q.faces[0])] {
                let symbol_line = near.iter().filter_map(|&b| boxes.get(b)).any(|bx| face_in(bx, sym) && !face_in(bx, wall));
                let (Some(ss), Some(ws)) = (segs.get(sym), segs.get(wall)) else { continue };
                if !symbol_line {
                    continue;
                }
                let here = side_of(ws, ss);
                // The walls on the far side, along this pair (the wall may be found in pieces).
                let Some(f) = Frame::of(q) else { continue };
                let mut along: Vec<(f64, f64)> = by_face
                    .get(&wall)
                    .into_iter()
                    .flatten()
                    .filter(|&&r| r != j)
                    .filter_map(|&r| found.get(r))
                    .filter(|rp| {
                        let other = if rp.faces[0] == wall { rp.faces[1] } else { rp.faces[0] };
                        segs.get(other).is_some_and(|os| side_of(ws, os) * here < 0.0)
                    })
                    .map(|rp| {
                        let (a, b) = minmax(f.u.dot(rp.c0 - q.c0), f.u.dot(rp.c1 - q.c0));
                        (a.max(0.0), b.min(f.len))
                    })
                    .filter(|(a, b)| b > a)
                    .collect();
                along.sort_by(|a, b| a.0.total_cmp(&b.0));
                let (mut covered, mut reach) = (0.0, f64::NEG_INFINITY);
                for (a, b) in along {
                    covered += (b - a.max(reach)).max(0.0);
                    reach = reach.max(b);
                }
                // Or it joins no wall: a symbol's line and a line by it (glazing across the
                // opening the unit stands in), no wall's piece.
                let walled = || {
                    let reach = Vec2::new(OPENING_REACH_IN * k + q.thickness, OPENING_REACH_IN * k + q.thickness);
                    pair_ix
                        .near(q.c0.min(q.c1) - reach, q.c0.max(q.c1) + reach)
                        .into_iter()
                        .filter(|&r| r != j && !drop.get(r).copied().unwrap_or(true))
                        .filter_map(|r| found.get(r))
                        .any(|r| joins(q, &f, r, k))
                };
                if covered > 0.5 * f.len || !walled() {
                    beside.push(j);
                    break;
                }
            }
        }
        for j in beside {
            if let Some(d) = drop.get_mut(j) {
                *d = true;
            }
        }
    }
    let before = found.len();
    let mut it = drop.iter();
    found.retain(|_| !it.next().copied().unwrap_or(false));
    before - found.len()
}

/// Two faces measured as a wall: the gap between their lines, their overlap and the centreline.
struct FacePair {
    gap: f64,
    length: f64,
    c0: Vec2,
    c1: Vec2,
}

/// Two nearly parallel faces as a wall when their lines are `min_thick..max_thick` apart and
/// they overlap by at least `min_len` (and 1½ times the gap), measured along their mean direction.
fn face_pair(s1: &Seg, s2: &Seg, lim: &Limits) -> Option<FacePair> {
    let d1 = (s1.b - s1.a).normalized();
    let mut d2 = (s2.b - s2.a).normalized();
    if d1.dot(d2) < 0.0 {
        d2 = d2 * -1.0;
    }
    let u = (d1 + d2).normalized();
    if !u.is_finite() || u.len() < 0.5 {
        return None;
    }
    let n = Vec2::new(-u.y, u.x);
    let (a0, a1) = minmax(u.dot(s1.a), u.dot(s1.b));
    let (b0, b1) = minmax(u.dot(s2.a), u.dot(s2.b));
    let (lo, hi) = (a0.max(b0), a1.min(b1));
    // Each face's line across `n` at a point `t` along `u`.
    let across = |s: &Seg, d: Vec2, t: f64| {
        let along = u.dot(d);
        n.dot(s.a) + (t - u.dot(s.a)) * n.dot(d) / if along.abs() > 1e-12 { along } else { 1.0 }
    };
    let tm = (lo + hi) / 2.0;
    let gap = (across(s2, d2, tm) - across(s1, d1, tm)).abs();
    let length = hi - lo;
    if !(lim.min_thick..=lim.max_thick).contains(&gap) || length < lim.min_len.max(gap * 1.5) {
        return None;
    }
    let mid = |t: f64| (across(s1, d1, t) + across(s2, d2, t)) / 2.0;
    Some(FacePair { gap, length, c0: u * lo + n * mid(lo), c1: u * hi + n * mid(hi) })
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

/// A door or window found in the plan.
pub(crate) struct Opening {
    pub kind: &'static str,
    /// A door swing's hinge (the arc's centre) or a block's insertion point.
    pub at: Vec2,
    pub width: f64,
    pub layer: String,
    pub handle: cadaddy_doc::Handle,
    /// A door swing's degrees and the arc's two ends.
    pub swing: Option<(f64, [Vec2; 2])>,
    pub block: Option<String>,
    pub confidence: f64,
    /// A swing whose leaf is drawn too (a line from the hinge to an end of the arc).
    pub leaf: bool,
    /// The arc's end the drawn leaf reaches: the door's open end (the other one is where it
    /// shuts).
    pub open_end: Option<Vec2>,
}

/// The end of a swing (hinged at `c`, its arc ending at `ends`) its leaf is drawn to: a line
/// from the hinge to that end, within `tol`. A leaf drawn as a thin outline has a side along it.
/// Whether a leaf is drawn at all, and the end when it is drawn to one only.
fn leaf_end(segs: &[Seg], c: Vec2, ends: [Vec2; 2], tol: f64) -> (bool, Option<Vec2>) {
    let to = |e: Vec2| segs.iter().any(|g| (g.a.dist(c) < tol && g.b.dist(e) < tol) || (g.b.dist(c) < tol && g.a.dist(e) < tol));
    match (to(ends[0]), to(ends[1])) {
        (true, false) => (true, Some(ends[0])),
        (false, true) => (true, Some(ends[1])),
        (a, b) => (a || b, None),
    }
}

/// Door swings (arcs of 60–100° with a door's radius, 16"–54", ideally with a leaf line from the
/// hinge to an end) and door and window blocks (by name).
pub(crate) fn openings(d: &Drawing, ents: &[&std::sync::Arc<Entity>], segs: &[Seg], k: f64) -> Vec<Opening> {
    let (rmin, rmax) = (16.0 * k, 54.0 * k);
    let mut items = Vec::new();
    for e in ents {
        match &e.kind {
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
                let (leaf, open_end) = leaf_end(segs, c, ends, tol);
                let on_door_layer = e.common.layer.to_ascii_uppercase().contains("DOOR");
                let confidence = (0.5 + 0.3 * f64::from(u8::from(leaf)) + 0.2 * f64::from(u8::from(on_door_layer))).min(1.0);
                items.push(Opening {
                    kind: "door",
                    at: c,
                    width: a.radius,
                    layer: e.common.layer.to_string(),
                    handle: e.handle,
                    swing: Some((deg, ends)),
                    block: None,
                    confidence,
                    leaf,
                    open_end,
                });
            }
            // A swing flattened to a polyline (how PDF prints arrive): a quarter-ish circle run,
            // alone or joined to its leaf and the wall's lines.
            EntityKind::LwPolyline(p) if !p.closed && (3..=200).contains(&p.vertices.len()) => {
                let pts = Polyline { vertices: p.vertices.clone(), closed: false }.tessellate(1e-3);
                for (c, r, deg, ends) in flat_swings(&pts) {
                    if !(rmin..=rmax).contains(&r) {
                        continue;
                    }
                    let tol = r * 0.05;
                    let (leaf, open_end) = leaf_end(segs, c, ends, tol);
                    let on_door_layer = e.common.layer.to_ascii_uppercase().contains("DOOR");
                    let confidence = (0.45 + 0.3 * f64::from(u8::from(leaf)) + 0.2 * f64::from(u8::from(on_door_layer))).min(1.0);
                    items.push(Opening {
                        kind: "door",
                        at: c,
                        width: r,
                        layer: e.common.layer.to_string(),
                        handle: e.handle,
                        swing: Some((deg, ends)),
                        block: None,
                        confidence,
                        leaf,
                        open_end,
                    });
                }
            }
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
                items.push(Opening {
                    kind,
                    at: i.insert.xy(),
                    width,
                    layer: e.common.layer.to_string(),
                    handle: e.handle,
                    swing: None,
                    block: Some(i.block.clone()),
                    confidence: 0.75,
                    leaf: false,
                    open_end: None,
                });
            }
            _ => {}
        }
    }
    items
}

/// Most door leaves added to a plan traced for rooms.
const MAX_LEAVES: usize = 200_000;
/// Most plan objects looked at for the jambs door swings end on.
const MAX_LEAF_SCAN: usize = 5_000_000;

/// A door's swing is a quarter-ish circle up to here.
const SWING_MAX_DEG: f64 = 100.0;
/// A door that opens back flat against its wall swings about this far. Only with something drawn
/// at the arc's centre (the leaf from its hinge): a half circle alone is no door.
const WIDE_SWING_MAX_DEG: f64 = 185.0;

/// The closed leaf of every door swing in `d` (an arc of 60–100° with a door's radius, or a
/// flattened one, or a door opening back against its wall): a line from the hinge to the end of
/// the arc where the wall's jamb is (an end of other linework), so a room traced on the plan ends
/// at its door instead of running on through the gap, and the swing stays in the room it opens
/// into. When neither end (or both) meets a jamb, both leaves are drawn; a door opening back
/// against its wall always gets both (the end its leaf is drawn to is the open one).
pub(crate) fn door_leaves(d: &Drawing, k: f64) -> Vec<(Common, cadaddy_doc::Line)> {
    let (rmin, rmax) = (16.0 * k, 54.0 * k);
    // Ends of the plan's straight linework, filed by cell.
    let cell = (2.0 * k).max(1e-9);
    let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut ends: std::collections::HashMap<(i64, i64), Vec<Vec2>> = std::collections::HashMap::new();
    let mut file = |p: Vec2| {
        if p.is_finite() {
            ends.entry(key(p)).or_default().push(p);
        }
    };
    for e in d.model.iter().take(MAX_LEAF_SCAN) {
        match &e.kind {
            EntityKind::Line(l) => {
                file(l.a.xy());
                file(l.b.xy());
            }
            EntityKind::LwPolyline(p) => p.vertices.iter().take(10_000).for_each(|v| file(v.p)),
            _ => {}
        }
    }
    let jamb = |p: Vec2, tol: f64| {
        let (cx, cy) = key(p);
        let k = (tol / cell).ceil() as i64;
        (-k..=k).any(|dx| (-k..=k).any(|dy| ends.get(&(cx + dx, cy + dy)).is_some_and(|v| v.iter().any(|q| q.dist(p) <= tol))))
    };
    let mut out = Vec::new();
    for e in d.model.iter() {
        if out.len() >= MAX_LEAVES {
            break;
        }
        let swings: Vec<(Vec2, f64, [Vec2; 2])> = match &e.kind {
            EntityKind::Arc(a) => {
                let mut sweep = (a.end - a.start).rem_euclid(std::f64::consts::TAU);
                if sweep < 1e-9 {
                    sweep = std::f64::consts::TAU;
                }
                let c = a.center.xy();
                let deg = sweep.to_degrees();
                ((60.0..=WIDE_SWING_MAX_DEG).contains(&deg) && (rmin..=rmax).contains(&a.radius))
                    .then(|| (c, deg, [c + Vec2::from_angle(a.start) * a.radius, c + Vec2::from_angle(a.end) * a.radius]))
                    .into_iter()
                    .collect()
            }
            // Alone, or joined to its leaf and the wall's lines (as PDF prints arrive).
            EntityKind::LwPolyline(p) if !p.closed && (3..=200).contains(&p.vertices.len()) => {
                let pts = Polyline { vertices: p.vertices.clone(), closed: false }.tessellate(1e-3);
                arc_runs(&pts)
                    .into_iter()
                    .filter(|(_, r, deg, _)| (rmin..=rmax).contains(r) && (60.0..=WIDE_SWING_MAX_DEG).contains(deg))
                    .map(|(c, _, deg, ends)| (c, deg, ends))
                    .collect()
            }
            _ => Vec::new(),
        };
        for (hinge, deg, arc_ends) in swings {
            let tol = 0.05 * hinge.dist(arc_ends[0]);
            let wide = deg > SWING_MAX_DEG;
            if wide && !jamb(hinge, 2.0 * tol) {
                continue;
            }
            let at_jamb = arc_ends.map(|p| jamb(p, tol));
            let leaves: Vec<Vec2> = match at_jamb {
                _ if wide => arc_ends.to_vec(),
                [true, false] => vec![arc_ends[0]],
                [false, true] => vec![arc_ends[1]],
                _ => arc_ends.to_vec(),
            };
            for end in leaves {
                if hinge.is_finite() && end.is_finite() {
                    let line = cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(hinge.x, hinge.y, 0.0), b: cadaddy_geom::Vec3::new(end.x, end.y, 0.0) };
                    out.push((e.common.clone(), line));
                }
            }
        }
    }
    out
}

/// The door swings of a run of points (on one circle within 5% of its radius, through 60–100°):
/// the whole run when it is one, else each swing drawn joined to its leaf or the wall's lines
/// ([`arc_runs`]). Each one's centre (the hinge), radius, sweep in degrees and two ends.
pub(crate) fn flat_swings(pts: &[Vec2]) -> Vec<(Vec2, f64, f64, [Vec2; 2])> {
    arc_runs(pts).into_iter().filter(|(_, _, deg, _)| (60.0..=SWING_MAX_DEG).contains(deg)).collect()
}

/// Names traced between two progress lines of the room trace.
const TRACE_PROGRESS_EVERY: usize = 100;

/// How far round a room's name its trace looks (feet, each way): a room up to twice as wide
/// round its name. A name in an area that doesn't close (a gap in its walls, an open plan)
/// stops there instead of searching ever larger windows: a whole sheet's border (a 36" sheet
/// at 1/8" = 1'-0" is 288 ft wide) and every sheet of a set side by side.
const ROOM_REACH_FT: f64 = 150.0;

/// A turn sharper than this (degrees) between two pieces of a polyline is a corner: a flattened
/// arc turns a few degrees a piece (a quarter circle drawn in 8 chords, 11¼°), a swing meets its
/// leaf and the wall's jamb square.
const ARC_CORNER_DEG: f64 = 25.0;
/// A first or last piece this many times longer than the stretch's typical chord is a straight
/// line running on from the arc (a leaf or a wall line meeting it at a tangent), not part of it.
const TANGENT_LINE_RATIO: f64 = 2.5;
/// Arcs read from one polyline, at most (hostile drawings).
const MAX_ARC_RUNS: usize = 64;

/// The arcs of a run of points: the whole run when it lies on one circle ([`flat_arc`]), else
/// each stretch between its corners that does. A PDF print joins touching strokes of one pen
/// into one polyline, so a door's swing often arrives as one piece of a polyline that runs on
/// along its leaf and the wall's jamb: the arc is still there, between two square corners.
/// A stretch needs four points (three chords) to be taken for an arc.
pub(crate) fn arc_runs(pts: &[Vec2]) -> Vec<(Vec2, f64, f64, [Vec2; 2])> {
    if let Some(a) = flat_arc(pts) {
        return vec![a];
    }
    // The corners: where the direction turns sharply between two (non-empty) pieces.
    let cos_corner = ARC_CORNER_DEG.to_radians().cos();
    let mut stretches: Vec<&[Vec2]> = Vec::new();
    let mut from = 0usize;
    let mut prev: Option<Vec2> = None;
    for i in 1..pts.len() {
        let (Some(a), Some(b)) = (pts.get(i - 1), pts.get(i)) else { continue };
        let d = *b - *a;
        let len = d.len();
        if !(len.is_finite() && len > 1e-12) {
            continue;
        }
        let u = d * (1.0 / len);
        if let Some(p) = prev
            && p.dot(u) < cos_corner
        {
            if let Some(s) = pts.get(from..i) {
                stretches.push(s);
            }
            from = i - 1;
        }
        prev = Some(u);
    }
    if let Some(s) = pts.get(from..) {
        stretches.push(s);
    }
    let mut out = Vec::new();
    for s in stretches {
        if out.len() >= MAX_ARC_RUNS {
            break;
        }
        if s.len() < 4 {
            continue;
        }
        if let Some(a) = flat_arc(s).or_else(|| flat_arc(trim_tangent_lines(s))) {
            out.push(a);
        }
    }
    out
}

/// A stretch without a first and a last piece much longer than its middle pieces (a straight line
/// running on from an arc at a tangent).
fn trim_tangent_lines(s: &[Vec2]) -> &[Vec2] {
    let len = |i: usize| match (s.get(i), s.get(i + 1)) {
        (Some(a), Some(b)) => a.dist(*b),
        _ => 0.0,
    };
    let n = s.len();
    if n < 6 {
        return s;
    }
    let mut mid: Vec<f64> = (1..n - 2).map(len).collect();
    mid.sort_by(f64::total_cmp);
    let typical = mid.get(mid.len() / 2).copied().unwrap_or(0.0);
    if !(typical.is_finite() && typical > 0.0) {
        return s;
    }
    let lo = usize::from(len(0) > TANGENT_LINE_RATIO * typical);
    let hi = n - usize::from(len(n - 2) > TANGENT_LINE_RATIO * typical);
    s.get(lo..hi).filter(|t| t.len() >= 4).unwrap_or(s)
}

/// A run of points on one circle (within 5% of its radius): its centre, radius, sweep in degrees
/// and two ends.
fn flat_arc(pts: &[Vec2]) -> Option<(Vec2, f64, f64, [Vec2; 2])> {
    let (first, last) = (*pts.first()?, *pts.last()?);
    let mid = *pts.get(pts.len() / 2)?;
    let d = 2.0 * (first.x * (mid.y - last.y) + mid.x * (last.y - first.y) + last.x * (first.y - mid.y));
    if !d.is_finite() || d.abs() < 1e-12 {
        return None;
    }
    let sq = |p: Vec2| p.x * p.x + p.y * p.y;
    let (a2, b2, c2) = (sq(first), sq(mid), sq(last));
    let c = Vec2::new(
        (a2 * (mid.y - last.y) + b2 * (last.y - first.y) + c2 * (first.y - mid.y)) / d,
        (a2 * (last.x - mid.x) + b2 * (first.x - last.x) + c2 * (mid.x - first.x)) / d,
    );
    let r = first.dist(c);
    if !c.is_finite() || !r.is_finite() || r <= 0.0 || pts.iter().any(|p| (p.dist(c) - r).abs() > 0.05 * r) {
        return None;
    }
    // The sweep: the turn from the first to the last point, through the middle one.
    let ang = |p: Vec2| (p.y - c.y).atan2(p.x - c.x);
    let sweep = |from: f64, to: f64| (to - from).rem_euclid(std::f64::consts::TAU);
    let (s_mid, s_end) = (sweep(ang(first), ang(mid)), sweep(ang(first), ang(last)));
    let deg = if s_mid <= s_end { s_end } else { std::f64::consts::TAU - s_end }.to_degrees();
    Some((c, r, deg, [first, last]))
}

pub(super) fn run_openings(s: &mut Session, p: &Value) -> Result<Value> {
    let (d, ents) = objects(s)?;
    let segs = segments(&ents);
    let items = openings(d, &ents, &segs, units_per_inch(d))
        .into_iter()
        .map(|o| {
            let mut v = json!({"kind": o.kind, "at": pt(o.at), "width": round(o.width), "layer": o.layer, "handle": o.handle.hex(), "confidence": round(o.confidence)});
            if let Some(obj) = v.as_object_mut() {
                if let Some((deg, _)) = o.swing {
                    obj.insert("swing".into(), json!(round(deg)));
                }
                if let Some(b) = o.block {
                    obj.insert("block".into(), json!(b));
                }
            }
            v
        })
        .collect();
    Ok(page(items, p))
}

// ---------------- plan.rooms ----------------

/// A room number: optional letter, 2–4 digits, optional letter (`101`, `B12`, `204A`).
pub(crate) fn room_number(t: &str) -> bool {
    let t = t.trim();
    let digits = t.chars().filter(char::is_ascii_digit).count();
    let letters = t.chars().filter(char::is_ascii_alphabetic).count();
    (2..=4).contains(&digits) && letters <= 2 && t.len() <= 6 && t.chars().all(|c| c.is_ascii_alphanumeric())
}

fn tag_words(t: &str) -> Vec<String> {
    t.to_ascii_uppercase().split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// Whether a text is an equipment or device tag, a note or a dimension rather than a room's
/// name: a panelboard or transformer label, a quoted panel letter (`'D'`), a device mark (`OS`,
/// `DH`, `J`), a circuit (`A-12`), a sheet reference or a length.
pub(crate) fn equipment_tag(t: &str) -> bool {
    let t = t.trim();
    let words = tag_words(t);
    let quoted = t.contains('\'') && t.chars().filter(|c| *c == '\'').count() >= 2;
    let length = t.chars().any(|c| c.is_ascii_digit()) && (t.contains('"') || t.contains("'-") || t.contains("' -"));
    let device_mark = words.len() == 1
        && cadaddy_mepfix::sheet_facts::is_type_tag(t)
        && !cadaddy_mepfix::wordrules::words("rooms.names").contains(&t.to_ascii_uppercase().as_str());
    let circuit =
        words.len() == 1 && t.rsplit_once('-').is_some_and(|(h, n)| !h.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    quoted || length || device_mark || circuit || words.iter().any(|w| cadaddy_mepfix::wordrules::words("equipment.names").contains(&w.as_str()))
}

/// Whether a text is a note rather than a room's name: a finish code (`CPT-1`), a ceiling, level
/// or scale note (`CLG 9'-0"`, `LEVEL 2`, `EL. 100'`, `SCALE: 1/4" = 1'-0"`, `NTS`) or a sheet
/// reference (`SEE A-201`, `A-5.1`).
pub(crate) fn note_like(t: &str) -> bool {
    let up = t.trim().to_ascii_uppercase();
    let words = tag_words(&up);
    let has = |w: &str| words.iter().any(|x| x == w);
    let ceiling = has("CLG") || has("CEILING") || has("AFF") || has("ACT");
    let level = up.starts_with("LEVEL") || has("EL") || up.contains("EL.") || up.contains("ELEV.") || up.contains("T.O.") || has("TOS");
    let scale = has("SCALE") || has("NTS") || up.contains("NOT TO SCALE");
    // A sheet reference: a sheet number with a dash or a dot (`A-201`, `E1.1`), or after SEE/REFER.
    let sheet_ref = up.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '/')).enumerate().any(|(i, w)| {
        super::sheet_number(w)
            && (w.contains('-') || w.contains('.') || (i > 0 && (has("SEE") || has("REFER") || has("REF") || has("SHEET") || has("DWG"))))
    });
    // A design-load or rating string (`ROOF LIVE LOAD 20 PSF`, `WIND 115 MPH`) and a roof or
    // canopy area's label (`LOW ROOF`, `CANOPY ABOVE`): an area of the plan, never a room (rule
    // data: `rooms.load_note_words`, `rooms.area_label_words`).
    let plain: Vec<&str> = up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let load = plain.iter().any(|w| cadaddy_mepfix::wordrules::words("rooms.load_note_words").contains(w));
    let area = plain.iter().any(|w| cadaddy_mepfix::wordrules::words("rooms.area_label_words").contains(w));
    // A storey's name alone (`FIRST FLOOR`, `2ND FLR`, `MEZZANINE`): a view's title or a level
    // marker, no room's name (one with a room's word, `GROUND FLOOR LOBBY`, is a room's).
    let storey = plain.len() <= 3
        && cadaddy_walk3d::levels::canonical_level(&up).is_some_and(|(_, k)| k != cadaddy_walk3d::levels::DatumKind::Penthouse)
        && !plain.iter().any(|w| cadaddy_mepfix::wordrules::words("rooms.names").contains(w));
    !super::finish_codes(&up).is_empty() || ceiling || level || scale || sheet_ref || load || area || storey
}

/// How near a side of the area it is traced in an outline's side may stop and still be the
/// area's border, as a share of the area's width or height.
const FRAME_SHARE: f64 = 0.01;

/// Whether a traced outline (its bounds `b`) is the border of the area `r` (`[x0, y0, x1, y1]`)
/// it is traced in, or a frame round it: it reaches every side of the area, within
/// [`FRAME_SHARE`]. A sheet's border, the box round its drawing area or a view's frame, never a
/// room: a building's rooms stand inside the view that draws them, with the view's title, notes
/// and schedules round them.
fn frames_area(b: &cadaddy_geom::Bounds2, r: &[f64; 4]) -> bool {
    let (x0, x1) = minmax(r[0], r[2]);
    let (y0, y1) = minmax(r[1], r[3]);
    let (w, h) = (x1 - x0, y1 - y0);
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && b.min.x.is_finite() && b.max.y.is_finite()) {
        return false;
    }
    let (tx, ty) = (FRAME_SHARE * w, FRAME_SHARE * h);
    b.min.x <= x0 + tx && b.min.y <= y0 + ty && b.max.x >= x1 - tx && b.max.y >= y1 - ty
}

/// Whether a traced outline holds its label: the label's point or a corner of its box inside it,
/// or its point within half the text height `th` of the outline (a tag's point on a storefront's
/// line).
fn holds_label(poly: &[Vec2], at: Vec2, corners: &[Vec2], th: f64) -> bool {
    cadaddy_geom::point_in_polygon(poly, at)
        || corners.iter().any(|c| cadaddy_geom::point_in_polygon(poly, *c))
        || crate::cmd::guided::edge_distance(poly, at) <= 0.5 * th
}

/// Most lines of a room's name stacked one over the other (`DOUBLE Q ADA` over `(TUB)`): a
/// stack of more is a note block's lines.
const MAX_ROOM_NAME_LINES: usize = 2;
/// The most a note block's next line is below the last, in its text height (line spacing).
const NOTE_LINE_SPACING: f64 = 2.5;
/// How far a note block's lines' left ends may differ, in their text height.
const NOTE_LINE_ALIGN: f64 = 0.75;

/// The texts (indices into `items`: handle, text, insertion point, height) that are lines of a
/// note block: more than [`MAX_ROOM_NAME_LINES`] lines of about one height, their left ends
/// lined up, each within [`NOTE_LINE_SPACING`] heights under the one before (a general note's
/// lines, a legend's, a schedule column's). A room's tag is a name of a line or two, its number,
/// and nothing lined up under it; room numbers and parenthesised qualifiers don't count as lines.
pub(crate) fn note_block_lines(items: &[(cadaddy_doc::Handle, String, Vec2, f64)]) -> std::collections::HashSet<usize> {
    let line = |i: usize| {
        items.get(i).is_some_and(|(_, t, at, h)| {
            let t = t.trim();
            at.is_finite()
                && h.is_finite()
                && *h > 0.0
                && t.chars().filter(|c| c.is_alphabetic()).count() >= 2
                && !room_number(t)
                && !(t.starts_with('(') && t.ends_with(')'))
        })
    };
    let mut by_y: Vec<usize> = (0..items.len()).filter(|&i| line(i)).collect();
    by_y.sort_by(|&a, &b| items[a].2.y.total_cmp(&items[b].2.y));
    // Each line's next line under it: the nearest below, of about its height, left ends lined up.
    let mut below: Vec<Option<usize>> = vec![None; items.len()];
    let mut has_above = vec![false; items.len()];
    for &i in &by_y {
        let Some((_, _, at, h)) = items.get(i) else { continue };
        let from = by_y.partition_point(|&j| items[j].2.y < at.y - NOTE_LINE_SPACING * h);
        let to = by_y.partition_point(|&j| items[j].2.y < at.y - 0.5 * h);
        let next = by_y.get(from..to).into_iter().flatten().copied().filter(|&j| {
            let (_, _, q, qh) = &items[j];
            *qh >= 0.8 * h && *qh <= 1.25 * h && (q.x - at.x).abs() <= NOTE_LINE_ALIGN * h
        });
        // The nearest one under it (the largest y below it).
        if let Some(j) = next.max_by(|&a, &b| items[a].2.y.total_cmp(&items[b].2.y)) {
            if let Some(b) = below.get_mut(i) {
                *b = Some(j);
            }
            if let Some(a) = has_above.get_mut(j) {
                *a = true;
            }
        }
    }
    // Each stack from its top line down: more lines than a room's name has, a note block.
    let mut out = std::collections::HashSet::new();
    for &top in by_y.iter().filter(|&&i| !has_above.get(i).copied().unwrap_or(true)) {
        let mut stack = vec![top];
        let mut at = top;
        while let Some(Some(next)) = below.get(at).copied() {
            if stack.contains(&next) || stack.len() > items.len() {
                break;
            }
            stack.push(next);
            at = next;
        }
        if stack.len() > MAX_ROOM_NAME_LINES {
            out.extend(stack);
        }
    }
    out
}

/// A room label's line wholly in parentheses (`(TUB)` under `DOUBLE Q ADA`) is the name above it
/// qualified, never a name of its own: it takes the text of the line right above it (about its
/// height, over it, within three lines). Its place stays: a room traced from it is that room.
pub(crate) fn lift_qualifiers(items: &mut [(cadaddy_doc::Handle, String, Vec2, f64)]) {
    let qualifier = |t: &str| t.trim().starts_with('(') && t.trim().ends_with(')');
    // By height on the page: only the band right above a qualifier is looked through (a print
    // set's sheets side by side hold hundreds of thousands of texts).
    let mut by_y: Vec<usize> = (0..items.len()).filter(|&i| items[i].2.y.is_finite()).collect();
    by_y.sort_by(|&a, &b| items[a].2.y.total_cmp(&items[b].2.y));
    let lifts: Vec<(usize, String)> = items
        .iter()
        .enumerate()
        .filter(|(_, (_, t, q, h))| qualifier(t) && h.is_finite() && *h > 0.0 && q.is_finite())
        .filter_map(|(i, (_, _, q, h))| {
            let from = by_y.partition_point(|&j| items[j].2.y <= q.y + 0.5 * h);
            let to = by_y.partition_point(|&j| items[j].2.y <= q.y + 3.0 * h);
            by_y.get(from..to)?
                .iter()
                .map(|&j| &items[j])
                .filter(|(_, t, a, ah)| {
                    let width = t.trim().chars().count() as f64 * ah;
                    !qualifier(t)
                        && !room_number(t)
                        && *ah > 0.7 * h
                        && *ah < 1.4 * h
                        && a.y > q.y + 0.5 * h
                        && a.y <= q.y + 3.0 * h
                        && q.x >= a.x - 2.0 * h
                        && q.x <= a.x + width
                })
                .min_by(|a, b| (a.2.y - q.y).total_cmp(&(b.2.y - q.y)))
                .map(|(_, t, _, _)| (i, t.clone()))
        })
        .collect();
    for (i, t) in lifts {
        if let Some(item) = items.get_mut(i) {
            item.1 = t;
        }
    }
}

/// How well a text names a room: 3 with a room or unit-type word (`DOUBLE Q`, `CORRIDOR`), 1 for
/// another name, 0 for an equipment or device tag, a note or a number.
/// A room's printed area (`105 SF`, `1,960 S.F.`, `12 SQ FT`, `11 m2`): never its name.
pub(crate) fn area_figure(t: &str) -> bool {
    let up = t.trim().to_ascii_uppercase();
    let digits: String = up.chars().take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect();
    if !digits.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let unit: String = up.get(digits.len()..).unwrap_or_default().chars().filter(|c| !c.is_whitespace() && *c != '.').collect();
    cadaddy_mepfix::wordrules::words("rooms.area_units").contains(&unit.as_str())
}

pub(crate) fn room_name_score(t: &str) -> u8 {
    // Letters one by one (`L J`): a fragment of vertical or spaced text, no name.
    let letters_only = tag_words(t).iter().all(|w| w.chars().count() <= 1);
    if equipment_tag(t) || note_like(t) || room_number(t) || area_figure(t) || t.chars().filter(|c| c.is_alphabetic()).count() < 2 || letters_only {
        0
    } else if tag_words(t).iter().any(|w| cadaddy_mepfix::wordrules::words("rooms.names").contains(&w.as_str())) {
        3
    } else {
        1
    }
}

fn polygon(l: &cadaddy_doc::HatchLoop) -> Vec<Vec2> {
    Polyline { vertices: l.vertices.clone(), closed: true }.tessellate(1e-3)
}

/// A room found from its name label: the closed outline around the label (the hatch boundary
/// search), its number (a number text inside), area and perimeter.
#[derive(Clone, Debug)]
pub(crate) struct FoundRoom {
    pub name: String,
    pub number: Option<String>,
    pub at: Vec2,
    pub label: cadaddy_doc::Handle,
    pub text_height: f64,
    pub outline: Vec<Vec2>,
    pub area: f64,
    pub perimeter: f64,
    pub confidence: f64,
}

/// A room tag block's attribute: `Some(true)` for the room's name (`NAME`, `ROOM_NAME`, `RM NAME`),
/// `Some(false)` for its number (`NUMBER`, `ROOM_NO`, `RM#`), else `None`.
pub(crate) fn tag_attribute(tag: &str) -> Option<bool> {
    let t: String = tag.to_ascii_uppercase().chars().filter(char::is_ascii_alphanumeric).collect();
    let t = ["ROOM", "SPACE", "RM"].iter().find_map(|p| t.strip_prefix(p)).unwrap_or(&t);
    match t {
        "NAME" | "NAM" => Some(true),
        "NUMBER" | "NUM" | "NO" | "NBR" | "" => Some(false),
        _ => None,
    }
}

/// A name or number candidate: (handle, text, where, height, from a room tag block's attribute).
type Label = (cadaddy_doc::Handle, String, Vec2, f64, bool);

/// The rooms of the current space, from its name labels: texts, and the name and number
/// attributes of room tag blocks. With a building spec, the labels on its room tag layers (else
/// the texts naming its rooms), traced on what bounds rooms ([`crate::cmd::bspec::room_plan`]).
/// Each outline takes its best name whatever order the labels were drawn in.
pub(crate) fn rooms_found(s: &mut Session) -> Result<Vec<FoundRoom>> {
    rooms_found_in(s, None)
}

/// The labels to trace, best room names first. With `regions` (sheet rectangles, `[x0, y0, x1,
/// y1]`): only the labels inside one, each region's best [`MAX_ROOMS_PER_REGION`] in turn, so a
/// print set's later sheets are read as fully as its first ones; else the drawing's best
/// [`MAX_ROOMS`].
fn labels_to_trace<T>(mut names: Vec<T>, at: impl Fn(&T) -> Vec2, score: impl Fn(&T) -> u8, regions: Option<&[[f64; 4]]>) -> Vec<T> {
    // A room or unit-type word claims its outline before another label in the same room.
    names.sort_by_key(|n| std::cmp::Reverse(score(n)));
    let Some(regions) = regions else {
        names.truncate(MAX_ROOMS);
        return names;
    };
    let inside = |r: &[f64; 4], p: Vec2| p.x >= r[0].min(r[2]) && p.x <= r[0].max(r[2]) && p.y >= r[1].min(r[3]) && p.y <= r[1].max(r[3]);
    let mut per: Vec<Vec<T>> = regions.iter().map(|_| Vec::new()).collect();
    for n in names {
        let p = at(&n);
        if let Some(i) = regions.iter().position(|r| inside(r, p))
            && let Some(list) = per.get_mut(i)
            && list.len() < MAX_ROOMS_PER_REGION
        {
            list.push(n);
        }
    }
    per.into_iter().flatten().take(MAX_ROOMS_ALL_REGIONS).collect()
}

/// Two outlines are one loop: the same corners, whichever each starts at and whichever way it
/// goes round.
fn same_loop(a: &[Vec2], b: &[Vec2]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let area = |p: &[Vec2]| cadaddy_geom::shoelace(p).abs();
    (area(a) - area(b)).abs() <= 1e-6 * area(a).max(1.0) && a.iter().all(|p| b.iter().any(|q| p.dist(*q) < 1e-6))
}

/// An object smaller than this, inches, is a speck of a plan's stipple (a wall's, a slab's).
const SPECK_MAX_IN: f64 = 1.0;
/// A plan with at least this share of specks is stippled, and traced without them.
const STIPPLED_SHARE: f64 = 0.25;

/// A label's width per character, in its text height (a room tag's capitals).
const LABEL_CHAR_WIDTH: f64 = 0.6;
/// A tag's number is within this of its name, in the name's text height (under its last line:
/// a two-line name, `DOUBLE Q ADA` over `(TUB)`, puts it about 5 heights from the first; the
/// next room's tag is 10 or more away).
const TAG_NUMBER_REACH: f64 = 5.5;
/// The least a room its label doesn't fit inside may be, sq ft (a smaller loop under a label is a
/// piece of furniture's linework).
const OVERHUNG_ROOM_MIN_SQFT: f64 = 40.0;

/// [`rooms_found`], the labels read only inside `regions` (sheet rectangles), sheet by sheet.
/// Room traces remembered per drawing.
const TRACES_KEPT: usize = 4;

/// The rooms traced in a drawing state (the drawing as a command left it, shared with undo), so
/// asking again in the same state reads them instead of tracing again: a model with no room
/// elements traced its plan (up to 103 s) on every `rooms.list`, `rooms.build` and
/// `bim_props fill`, and a job asked about 6 times. "No rooms" is remembered too. Any edit makes
/// a new drawing state, so an edited wall or label is traced again.
#[derive(Clone, Debug, Default)]
pub struct TraceCache {
    traces: Vec<Trace>,
    /// The latest traces by what they were traced from, whichever drawing state that was: an
    /// edit that changes nothing a trace reads (a stair, device or run line, a line off the plan,
    /// the model a rebuild regenerated) reads the rooms again instead of tracing them. Every
    /// update of an associative model traced its plan's rooms again (8 s on a 13-room plan),
    /// also when the edit bounded no room and nothing was rebuilt.
    by_inputs: Vec<(cadaddy_doc::Space, Option<Vec<[f64; 4]>>, std::sync::Arc<TraceInputs>, std::sync::Arc<Vec<FoundRoom>>)>,
    /// How many times the labels' outlines were searched (the trace's costly part) in this drawing.
    traced: usize,
}

/// Traces remembered by their inputs: one a sheet region of a print set (each traced on its own).
const INPUT_TRACES_KEPT: usize = 64;

/// What the outline search of a room trace reads, past the labels: the labels to trace and the
/// numbers they may take, the plan it traces on (its objects, the door leaves and wall-run
/// joints drawn across, and the layers, blocks and header values they are drawn with), the
/// drawing unit and the sheet's area. The same inputs trace the same rooms.
#[derive(Debug)]
pub(crate) struct TraceInputs {
    k: f64,
    sheet: f64,
    names: Vec<Label>,
    numbers: Vec<Label>,
    model: Vec<std::sync::Arc<Entity>>,
    leaves: Vec<(cadaddy_doc::Common, cadaddy_doc::Line)>,
    header: cadaddy_doc::Header,
    layers: Vec<cadaddy_doc::Layer>,
    blocks: cadaddy_doc::shared::Shared<std::collections::BTreeMap<String, std::sync::Arc<cadaddy_doc::Block>>>,
}

impl TraceInputs {
    fn same_as(&self, o: &TraceInputs) -> bool {
        self.k.to_bits() == o.k.to_bits()
            && self.sheet.to_bits() == o.sheet.to_bits()
            && self.names == o.names
            && self.numbers == o.numbers
            && self.leaves == o.leaves
            && self.layers == o.layers
            && self.header == o.header
            && self.blocks == o.blocks
            && self.model.len() == o.model.len()
            // An object no edit touched is the very same one (shared by both states).
            && self.model.iter().zip(&o.model).all(|(a, b)| std::sync::Arc::ptr_eq(a, b) || a == b)
    }
}

#[derive(Clone, Debug)]
struct Trace {
    state: std::sync::Weak<Drawing>,
    space: cadaddy_doc::Space,
    regions: Option<Vec<[f64; 4]>>,
    rooms: std::sync::Arc<Vec<FoundRoom>>,
}

/// Whether a trace of `old` holds for `now`: the same model objects (their chunks still shared:
/// nothing in model space changed) and the same of everything else the trace reads (units and
/// the other header values, layers, blocks, the building spec, the job's sheets). A command that
/// took the drawing for writing and changed nothing there (`bim.props.fill` reseating no device)
/// leaves a new state the old trace still holds for.
fn trace_holds(old: &Drawing, now: &Drawing) -> bool {
    old.model.unchanged_from(&now.model)
        && old.blocks.same(&now.blocks)
        && old.header == now.header
        && old.building_spec == now.building_spec
        && old.agent == now.agent
        && old.layers == now.layers
}

impl TraceCache {
    fn get(&self, d: &std::sync::Arc<Drawing>, space: &cadaddy_doc::Space, regions: Option<&[[f64; 4]]>) -> Option<std::sync::Arc<Vec<FoundRoom>>> {
        let same_ask = |t: &&Trace| t.space == *space && t.regions.as_deref() == regions;
        let exact = self.traces.iter().filter(same_ask).find(|t| std::ptr::eq(t.state.as_ptr(), std::sync::Arc::as_ptr(d)));
        // In paper space only the very same state: its objects are a layout's, not compared here.
        let holds =
            || self.traces.iter().filter(same_ask).filter(|_| !space.is_paper()).find(|t| t.state.upgrade().is_some_and(|old| trace_holds(&old, d)));
        exact.or_else(holds).map(|t| std::sync::Arc::clone(&t.rooms))
    }
    fn put(&mut self, d: &std::sync::Arc<Drawing>, space: cadaddy_doc::Space, regions: Option<&[[f64; 4]]>, rooms: std::sync::Arc<Vec<FoundRoom>>) {
        // States nothing holds any more can't be asked about again.
        self.traces.retain(|t| t.state.strong_count() > 0);
        if self.traces.len() >= TRACES_KEPT {
            self.traces.remove(0);
        }
        self.traces.push(Trace { state: std::sync::Arc::downgrade(d), space, regions: regions.map(<[_]>::to_vec), rooms });
    }
    /// The rooms last traced from these inputs (in this space and regions).
    fn by_inputs(&self, space: &cadaddy_doc::Space, regions: Option<&[[f64; 4]]>, inputs: &TraceInputs) -> Option<std::sync::Arc<Vec<FoundRoom>>> {
        self.by_inputs
            .iter()
            .rev()
            .find(|(sp, rs, i, _)| sp == space && rs.as_deref() == regions && i.same_as(inputs))
            .map(|(.., rooms)| std::sync::Arc::clone(rooms))
    }
    fn put_inputs(&mut self, space: cadaddy_doc::Space, regions: Option<&[[f64; 4]]>, inputs: TraceInputs, rooms: std::sync::Arc<Vec<FoundRoom>>) {
        self.traced += 1;
        // An edit and its undo (a wall moved and moved back) each find their plan's trace.
        if self.by_inputs.len() >= INPUT_TRACES_KEPT {
            self.by_inputs.remove(0);
        }
        self.by_inputs.push((space, regions.map(<[_]>::to_vec), std::sync::Arc::new(inputs), rooms));
    }
    /// How many traces are remembered.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.traces.len()
    }
    /// How many times the labels' outlines were searched.
    #[cfg(test)]
    pub(crate) fn traced(&self) -> usize {
        self.traced
    }
}

/// [`trace_rooms_in`], read from the drawing state's remembered trace when there is one
/// ([`TraceCache`]).
pub(crate) fn rooms_found_in(s: &mut Session, regions: Option<&[[f64; 4]]>) -> Result<Vec<FoundRoom>> {
    let space = s.space();
    let state = std::sync::Arc::clone(&s.state()?.doc);
    if let Some(rooms) = s.state()?.room_trace.get(&state, &space, regions) {
        return Ok(rooms.as_ref().clone());
    }
    let rooms = trace_rooms_in(s, regions)?;
    // Traced in the state it was asked in (the trace changes nothing): remembered for it.
    if let Ok(st) = s.state_mut()
        && std::sync::Arc::ptr_eq(&st.doc, &state)
    {
        st.room_trace.put(&state, space, regions, std::sync::Arc::new(rooms.clone()));
    }
    Ok(rooms)
}

/// The rooms around the plan's name labels, traced.
fn trace_rooms_in(s: &mut Session, regions: Option<&[[f64; 4]]>) -> Result<Vec<FoundRoom>> {
    // Several sheets: each traced on its own objects. A room left open on one (a doorway the
    // print draws no leaf across) widens its search window to the plan's extents: those of the
    // one sheet, not of every sheet side by side (where one open room searched them all).
    if let Some(rs) = regions.filter(|r| r.len() > 1) {
        let inside = |r: &[f64; 4], p: Vec2| p.x >= r[0].min(r[2]) && p.x <= r[0].max(r[2]) && p.y >= r[1].min(r[3]) && p.y <= r[1].max(r[3]);
        let mut out: Vec<FoundRoom> = Vec::new();
        for (i, r) in rs.iter().enumerate() {
            if out.len() >= MAX_ROOMS_ALL_REGIONS {
                break;
            }
            // A label is its first region's (as one trace of them all took it).
            let earlier = rs.get(..i).unwrap_or_default();
            if earlier.contains(r) {
                continue;
            }
            let found = trace_rooms_in(s, Some(std::slice::from_ref(r)))?;
            out.extend(found.into_iter().filter(|f| !earlier.iter().any(|e| inside(e, f.at))));
        }
        out.truncate(MAX_ROOMS_ALL_REGIONS);
        return Ok(out);
    }
    // A slow phase (over 2 s) goes to the job's progress lines: which one held a long trace up.
    let started = web_time::Instant::now();
    let mut last = 0.0f64;
    let mut phase = |what: &str| {
        let now = started.elapsed().as_secs_f64();
        if now - last > 2.0 {
            crate::cmd::guided::progress_line(&format!("  room trace: {what} took {:.1} s", now - last));
        }
        last = now;
    };
    let (d, ents) = objects(s)?;
    let k = units_per_inch(d);
    let min_area = 16.0 * 144.0 * k * k;
    let ext = d.extents(&s.space());
    let sheet = (ext.width() * ext.height()).max(1e-9);
    let spec = d.building_spec.as_deref().and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok());
    let tagged = spec.as_ref().is_some_and(|sp| crate::cmd::bspec::tags_carry_labels(sp, &ents));
    let on_tags: Vec<_> =
        ents.iter().copied().filter(|e| !tagged || spec.as_ref().is_some_and(|sp| crate::cmd::bspec::room_tag_layer(sp, &e.common.layer))).collect();
    let mut read = texts(&on_tags);
    lift_qualifiers(&mut read);
    // A note block's lines name no room (a general note's lines inside a roof area or the
    // sheet's border traced as rooms); their numbers stay numbers.
    let block = note_block_lines(&read);
    let mut all: Vec<Label> = read.into_iter().map(|(h, t, at, ht)| (h, t, at, ht, false)).collect();
    let in_block: std::collections::HashSet<cadaddy_doc::Handle> = block.iter().filter_map(|&i| all.get(i).map(|l| l.0)).collect();
    // Room tag blocks: their name and number attributes. The blocks themselves (a box or a bubble
    // around the tag) bound no room.
    let mut tag_blocks = std::collections::HashSet::new();
    let mut tag_numbers: Vec<Label> = Vec::new();
    // A room's name the import took for a repeated symbol's mark (three equipment closets' `AHU`
    // boxes alike: a symbol, the word its TAG): still the room's name where it was printed. (Names
    // only: a number on a symbol is a door's or a device's.)
    let mut symbol_names: Vec<Label> = Vec::new();
    for e in &on_tags {
        let EntityKind::Insert(i) = &e.kind else { continue };
        if !i.block.starts_with("PDF-SYM-") {
            continue;
        }
        for a in i.attribs.iter().filter(|a| a.tag.eq_ignore_ascii_case("TAG")) {
            let v = a.text.value.trim();
            let (at, h) = (a.text.justify_point(), a.text.height);
            let word = cadaddy_mepfix::wordrules::words("equipment.names").contains(&v.to_ascii_uppercase().as_str()) || room_name_score(v) >= 3;
            let printed = all.iter().any(|(_, t, q, _, _)| t.trim().eq_ignore_ascii_case(v) && q.dist(at) <= 0.5 * h.max(1e-9));
            if !v.is_empty() && v.chars().count() <= 40 && h.is_finite() && h > 0.0 && word && !room_number(v) && !printed {
                symbol_names.push((e.handle, v.to_string(), at, h, false));
            }
        }
    }
    all.extend(symbol_names);
    for e in &on_tags {
        let EntityKind::Insert(i) = &e.kind else { continue };
        // A title block (its sheet number, title, project, date) is not a room tag.
        let title_block = i.attribs.iter().any(|a| {
            let t = a.tag.to_ascii_uppercase();
            ["SHEET", "DWG", "TITLE", "PROJECT", "DATE", "SCALE", "DRAWN"].iter().any(|w| t.contains(w))
        });
        if title_block {
            continue;
        }
        for a in &i.attribs {
            let v = a.text.value.trim();
            if v.is_empty() || v.chars().count() > 40 {
                continue;
            }
            match tag_attribute(&a.tag) {
                Some(true) => all.push((e.handle, v.to_string(), a.text.justify_point(), a.text.height, true)),
                Some(false) => tag_numbers.push((e.handle, v.to_string(), a.text.justify_point(), a.text.height, true)),
                None => continue,
            }
            tag_blocks.insert(e.handle);
        }
    }
    // Untagged, a spec with rooms names them.
    let spec_rooms = spec.as_ref().filter(|sp| !tagged && !sp.rooms.is_empty()).map(|sp| sp.rooms.as_slice());
    // An equipment room tagged by the unit it houses: the word alone (`AHU`) over a room number,
    // as a room's tag prints its name over its number. (A plan's equipment tag has its own number:
    // `AHU-1`.)
    let numbered_below = |t: &str, at: Vec2, h: f64| {
        let w = t.trim().to_ascii_uppercase();
        cadaddy_mepfix::wordrules::words("equipment.names").contains(&w.as_str())
            && all.iter().any(|(_, n, q, nh, _)| {
                room_number(n) && (*nh - h).abs() <= 0.3 * h && at.y - q.y >= 0.5 * h && at.y - q.y <= 3.0 * h && (q.x - at.x).abs() <= 3.0 * h
            })
    };
    let score = |(_, t, at, h, tag): &Label| {
        if *tag {
            4
        } else if numbered_below(t, *at, *h) {
            1
        } else {
            room_name_score(t)
        }
    };
    let mut names: Vec<Label> = all
        .iter()
        // A plain text names a room in a room tag's words: not a note's line or sentence, a
        // title block's or a stamp's words ([`crate::cmd::autospec::room_like_name`]).
        .filter(|(h, t, _, _, tag)| {
            t.chars().count() <= 40 && (*tag || (!note_like(t) && !in_block.contains(h) && crate::cmd::autospec::room_like_name(t)))
        })
        .filter(|l| match spec_rooms {
            Some(rs) => {
                let t = &l.1;
                l.4 || (t.chars().filter(|c| c.is_alphabetic()).count() >= 2
                    && !room_number(t)
                    && rs.iter().any(|r| crate::cmd::bspec::names_room(r, t)))
            }
            // Equipment and device tags (a panelboard label beside a room) never name it.
            None => score(l) > 0,
        })
        .cloned()
        .collect();
    // The best name claims its outline first, whatever the drawing order: a tag block's name, then
    // a room or unit-type word, then the tallest text, then by text and place. Read sheet by sheet,
    // the score alone orders them (then drawing order): there a taller label on the plan (a unit
    // type, a note) would claim a room's outline before the room's own name.
    if regions.is_none() {
        names.sort_by(|a, b| {
            score(b).cmp(&score(a)).then(b.3.total_cmp(&a.3)).then_with(|| a.1.cmp(&b.1)).then(b.2.y.total_cmp(&a.2.y)).then(a.2.x.total_cmp(&b.2.x))
        });
    }
    // The drawing's best names, or each sheet's best in turn (the stable sort keeps the order above).
    let names = labels_to_trace(names, |n| n.2, score, regions);
    phase("labels");
    // A detail or section callout's sheet (`A131`, `A411`) reads like a lettered room number: one
    // of the set's sheet numbers is no room's.
    let mut sheet_numbers: std::collections::BTreeSet<String> =
        spec.as_ref().map(|sp| sp.sheets.iter().map(|x| x.number.trim().to_ascii_uppercase()).collect()).unwrap_or_default();
    if let Ok(Some(job)) = crate::cmd::guided::load_job(s) {
        sheet_numbers.extend(job.sheet_regions.iter().map(|r| r.number.split('/').next().unwrap_or(&r.number).trim().to_ascii_uppercase()));
    }
    let mut numbers: Vec<Label> =
        all.iter().filter(|(_, t, _, _, tag)| !tag && room_number(t) && !sheet_numbers.contains(&t.trim().to_ascii_uppercase())).cloned().collect();
    numbers.extend(tag_numbers);
    // Read sheet by sheet: trace on the plans' own objects only (a whole print set side by side
    // is so large its trace tolerance - a fraction of the extents - opens every room).
    let plan = match regions.filter(|r| !r.is_empty()) {
        Some(rs) => {
            let inside =
                |c: Vec2| rs.iter().any(|r| c.x >= r[0].min(r[2]) && c.x <= r[0].max(r[2]) && c.y >= r[1].min(r[3]) && c.y <= r[1].max(r[3]));
            let objects: Vec<&std::sync::Arc<Entity>> =
                crate::cmd::bspec::plan_objects(d).into_iter().filter(|e| crate::cmd::bspec::centre(d, e).is_some_and(inside)).collect();
            // A stippled plan (grout, concrete, earth drawn as thousands of dots) is too dense to
            // search for a room's outline, and no speck bounds a room.
            let speck = |e: &Entity| {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                b.width().max(b.height()) < SPECK_MAX_IN * k
            };
            let specks = objects.iter().filter(|e| speck(e)).count();
            let stippled = specks as f64 >= STIPPLED_SHARE * objects.len() as f64;
            let keep: std::collections::HashSet<cadaddy_doc::Handle> = objects.iter().filter(|e| !(stippled && speck(e))).map(|e| e.handle).collect();
            crate::cmd::bspec::room_plan_of(s, &keep)?
        }
        None => crate::cmd::bspec::room_plan(s)?,
    };
    phase("plan");
    // Without the tag blocks: a tag's box is not its room.
    let plan = if tag_blocks.is_empty() || s.space().is_paper() {
        plan
    } else {
        let mut wd = plan.as_ref().unwrap_or(s).doc()?.clone();
        for h in &tag_blocks {
            wd.model.remove(*h);
        }
        let mut t = Session::empty();
        t.open_drawing(wd, "Room boundaries", None);
        Some(t)
    };
    // Doors close their rooms: each door swing's leaf, closed and open, drawn across its opening
    // (a print often draws the swing's arc alone, which leaves the room open at the door).
    let mut leaves = door_leaves(plan.as_ref().unwrap_or(s).doc()?, k);
    // A wall drawn block by block closes its rooms too: each run of faces drawn across its joints.
    {
        let pd = plan.as_ref().unwrap_or(s).doc()?;
        let objects = crate::cmd::bspec::plan_objects(pd);
        for (a, b) in unit_run_lines(&segments(&objects), k) {
            let line = cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(a.x, a.y, 0.0), b: cadaddy_geom::Vec3::new(b.x, b.y, 0.0) };
            leaves.push((cadaddy_doc::Common::default(), line));
        }
    }
    phase("tag blocks and door leaves");
    // Traced from these very inputs before (in another drawing state): the same rooms.
    let inputs = {
        let pd = plan.as_ref().unwrap_or(s).doc()?;
        TraceInputs {
            k,
            sheet,
            names: names.clone(),
            numbers: numbers.clone(),
            model: pd.model.iter().cloned().collect(),
            leaves: leaves.clone(),
            header: pd.header.clone(),
            layers: pd.layers.clone(),
            blocks: pd.blocks.clone(),
        }
    };
    if let Some(rooms) = s.state()?.room_trace.by_inputs(&s.space(), regions, &inputs) {
        return Ok(rooms.as_ref().clone());
    }
    let plan = if leaves.is_empty() || s.space().is_paper() {
        plan
    } else {
        let mut wd = plan.as_ref().unwrap_or(s).doc()?.clone();
        for (common, line) in leaves {
            wd.add(&cadaddy_doc::Space::Model, common, EntityKind::Line(line))?;
        }
        let mut t = Session::empty();
        t.open_drawing(wd, "Room boundaries", None);
        Some(t)
    };
    let plan = plan.as_ref().unwrap_or(s);
    let mut out: Vec<FoundRoom> = Vec::new();
    // Rooms whose only number inside is another size than their name (taken when no tag is near).
    let mut any_size: Vec<(usize, String)> = Vec::new();
    // Whether each room in `out` was traced from round what its name is printed on.
    let mut out_probed: Vec<bool> = Vec::new();
    // Outlines that were a sheet's border or a view's frame ([`frames_area`]).
    let mut frames = 0usize;
    let mut outline_cache = None;
    let habitable_area = HABITABLE_MIN_SF * 144.0 * k * k;
    // A storey of a full set has thousands of names: say how far the trace is.
    let (total, trace_started) = (names.len(), web_time::Instant::now());
    let mut searching = std::time::Duration::ZERO;
    for (traced, (h, name, at, height, _)) in names.into_iter().enumerate() {
        if traced > 0 && traced % TRACE_PROGRESS_EVERY == 0 {
            crate::cmd::guided::progress_line(&format!(
                "  room trace: {traced} of {total} names, {} rooms, {:.0} s ({:.0} s finding boundaries)",
                out.len(),
                trace_started.elapsed().as_secs_f64(),
                searching.as_secs_f64()
            ));
        }
        // The room around the label: the first outline, traced from the label's point, its middle,
        // just above or below it or just outside its box, that holds the whole label (a piece of furniture drawn under
        // the label - a mirrored unit's desk - traces a loop the label overhangs); else, as
        // before, the one its point traces.
        let th = if height.is_finite() && height > 0.0 { height } else { 6.0 * k };
        let wide = name.chars().count() as f64 * th * LABEL_CHAR_WIDTH;
        let seeds = [
            at,
            at + Vec2::new(wide / 2.0, th / 2.0),
            at + Vec2::new(wide / 2.0, 1.8 * th),
            at + Vec2::new(wide / 2.0, -1.2 * th),
            // Just outside the label's box, each side (a desk wider than the label's middle).
            at + Vec2::new(-1.5 * th, th / 2.0),
            at + Vec2::new(wide + 1.5 * th, th / 2.0),
            at + Vec2::new(wide / 2.0, 4.0 * th),
            at + Vec2::new(wide / 2.0, -3.0 * th),
        ];
        let corners = [
            at + Vec2::new(0.05 * wide, 0.1 * th),
            at + Vec2::new(0.95 * wide, 0.1 * th),
            at + Vec2::new(0.05 * wide, 0.9 * th),
            at + Vec2::new(0.95 * wide, 0.9 * th),
        ];
        let mut first: Option<(Vec<Vec2>, f64)> = None;
        let mut holds: Option<(Vec<Vec2>, f64)> = None;
        // Whether a loop holding its whole box took it for a note (below): no room is traced
        // round it then either.
        let mut note = false;
        for (i, seed) in seeds.into_iter().enumerate() {
            let search = web_time::Instant::now();
            let found = crate::cmd::hatch::loops_at_lazy_within(plan, seed, &mut outline_cache, Some(ROOM_REACH_FT * 12.0 * k));
            searching += search.elapsed();
            let Ok(loops) = found else { continue };
            let Some(outer) = loops.iter().find(|l| l.outer).or(loops.first()) else { continue };
            let poly = polygon(outer);
            if poly.len() < 3 {
                continue;
            }
            let area = cadaddy_geom::shoelace(&poly).abs();
            // In the room: inside its outline or on one of its islands (furniture standing in it).
            let islands: Vec<Vec<Vec2>> = loops.iter().filter(|l| !l.outer).map(polygon).filter(|p| p.len() >= 3).collect();
            let in_room = |c: Vec2| cadaddy_geom::point_in_polygon(&poly, c) || islands.iter().any(|p| cadaddy_geom::point_in_polygon(p, c));
            // (The label in it: its point, or its whole box where its point sits on a line - a
            // storefront's - just outside: within half its height of the outline, the room's
            // number in it too. A note's point farther out, or a note on a notes box's edge, is no
            // room's label.)
            let whole = corners.iter().all(|c| in_room(*c));
            let on_edge = in_room(at)
                || (crate::cmd::guided::edge_distance(&poly, at) <= 0.5 * th
                    && numbers.iter().any(|(_, _, q, _, _)| cadaddy_geom::point_in_polygon(&poly, *q)));
            if whole && !on_edge && area >= min_area {
                note = true;
            }
            if area < min_area || !(i == 0 || in_room(at) || (whole && on_edge)) {
                continue;
            }
            if whole && !on_edge {
                continue;
            }
            if whole {
                holds = Some((poly, area));
                break;
            }
            // A loop the label overhangs is its room only when it is room-sized (not a desk).
            if i == 0 && area >= OVERHUNG_ROOM_MIN_SQFT * 144.0 * k * k {
                first = Some((poly, area));
            }
        }
        // A room to live in is at least a habitable room's size; any room at least the least
        // room's. A smaller loop round the name is what it is printed on (a bed, a tag's box):
        // the room is the loop round that.
        let least = if tag_words(&name).iter().any(|w| cadaddy_mepfix::wordrules::words("rooms.habitable").contains(&w.as_str())) {
            habitable_area
        } else {
            min_area
        };
        let (poly, area, probed) = match holds.or(first) {
            Some((poly, area)) if area >= least => (poly, area, false),
            _ if note => continue,
            _ => {
                let search = web_time::Instant::now();
                let found = crate::cmd::bspec::outline_round_label_within(plan, at, &mut outline_cache, least, Some(ROOM_REACH_FT * 12.0 * k));
                searching += search.elapsed();
                let Some(p) = found else { continue };
                if p.len() < 3 {
                    continue;
                }
                let area = cadaddy_geom::shoelace(&p).abs();
                if area < min_area {
                    continue;
                }
                (p, area, true)
            }
        };
        // A room's trace holds its own tag: its point, a corner of its box, or its point on the
        // outline. A loop beside the tag (cut along a line through the tags, under the tag's
        // box) is another space's.
        if !holds_label(&poly, at, &corners, th) {
            continue;
        }
        // One room per outline: a second label in the same room is its number or a note.
        // (The same loop traced from another seed, or again from round the furniture its name is
        // on, may start at another corner.)
        let b = cadaddy_geom::Bounds2::from_points(poly.iter().copied());
        // A sheet's border or a view's frame is no room, whichever label sits in it (a
        // schedule's header, a note's line): every room is drawn inside the area it is read in.
        if regions.into_iter().flatten().any(|r| frames_area(&b, r)) {
            frames += 1;
            continue;
        }
        let tol = SAME_OUTLINE_SHARE * (b.width() + b.height());
        if out.iter().zip(&out_probed).any(|(q, q_probed)| {
            let qb = cadaddy_geom::Bounds2::from_points(q.outline.iter().copied());
            same_loop(&q.outline, &poly)
                || ((probed || *q_probed)
                    && (q.area - area).abs() <= SAME_OUTLINE_SHARE * area
                    && qb.min.dist(b.min) <= tol
                    && qb.max.dist(b.max) <= tol)
        }) {
            continue;
        }
        // Read sheet by sheet, a schedule's or legend's cells trace too: no room is narrower.
        if regions.is_some() && b.width().min(b.height()) < MIN_ROOM_WIDTH_IN * k {
            continue;
        }
        let perimeter: f64 = poly.iter().zip(poly.iter().cycle().skip(1)).map(|(a, b)| a.dist(*b)).sum();
        // The room number: the one in the name's tag block, else a number inside the outline of
        // the name's size, nearest the name (an OCR copy's or a note's digits inside the room are
        // another size); else the tag's own number just under or beside the name (on a bed's
        // linework that cuts into the traced outline); else the tag nearest the name
        // ([`number_from_nearest_tag`]); else one of any size inside.
        let inside_numbers = |sized: bool| {
            numbers
                .iter()
                .filter(|(nh, _, q, qh, _)| {
                    cadaddy_geom::point_in_polygon(&poly, *q)
                        && (!sized || *nh == h || (*qh >= TAG_SIZE_BAND.0 * height && *qh <= TAG_SIZE_BAND.1 * height))
                })
                .min_by(|a, b| u8::from(a.0 != h).cmp(&u8::from(b.0 != h)).then(a.2.dist(at).total_cmp(&b.2.dist(at))).then_with(|| a.1.cmp(&b.1)))
                .map(|(_, t, _, _, _)| t.clone())
        };
        let beside = || {
            numbers
                .iter()
                .filter(|(_, _, q, _, _)| q.dist(at) <= TAG_NUMBER_REACH * th)
                .min_by(|a, b| u8::from(a.0 != h).cmp(&u8::from(b.0 != h)).then(a.2.dist(at).total_cmp(&b.2.dist(at))).then_with(|| a.1.cmp(&b.1)))
                .map(|(_, t, _, _, _)| t.clone())
        };
        let number = inside_numbers(true).or_else(beside);
        if number.is_none()
            && let Some(any) = inside_numbers(false)
        {
            any_size.push((out.len(), any));
        }
        let share = area / sheet;
        let confidence = (0.5 + 0.2 * f64::from(u8::from(number.is_some())) + if share < 0.5 { 0.3 } else { 0.0 }
            - if poly.len() > 200 { 0.1 } else { 0.0 })
        .clamp(0.0, 1.0);
        out.push(FoundRoom { name, number, at, label: h, text_height: height, outline: poly, area, perimeter, confidence });
        out_probed.push(probed);
    }
    let taken = number_from_nearest_tag(&mut out, &numbers, regions);
    for (i, t) in any_size {
        if let Some(r) = out.get_mut(i).filter(|r| r.number.is_none()) {
            r.number = Some(t);
        }
    }
    phase(&format!("tracing {} rooms ({taken} numbered from the tag nearest the name, {frames} sheet borders or view frames left out)", out.len()));
    let space = s.space();
    if let Ok(st) = s.state_mut() {
        st.room_trace.put_inputs(space, regions, inputs, std::sync::Arc::new(out.clone()));
    }
    Ok(out)
}

/// Each room traced without a number takes the number tag nearest its name that no room has (in
/// its sheet region) and that lies in no other room, within [`NUMBER_REACH_HEIGHTS`] of the
/// name's height: a tag printed on the room's furniture or in a notch of its outline. Nearest
/// pairs first, so two rooms never share a tag. Returns how many took one.
fn number_from_nearest_tag(rooms: &mut [FoundRoom], numbers: &[Label], regions: Option<&[[f64; 4]]>) -> usize {
    let region_of = |p: Vec2| {
        regions
            .and_then(|rs| rs.iter().position(|r| p.x >= r[0].min(r[2]) && p.x <= r[0].max(r[2]) && p.y >= r[1].min(r[3]) && p.y <= r[1].max(r[3])))
    };
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, r) in rooms.iter().enumerate().filter(|(_, r)| r.number.is_none()) {
        let reach = NUMBER_REACH_HEIGHTS * r.text_height;
        if !(reach.is_finite() && reach > 0.0) {
            continue;
        }
        let region = region_of(r.at);
        for (j, (_, t, q, qh, _)) in numbers.iter().enumerate() {
            // A tag prints its number at its name's size (a note's, a dimension's or an OCR
            // copy's digits are another size).
            let same_size = *qh >= TAG_SIZE_BAND.0 * r.text_height && *qh <= TAG_SIZE_BAND.1 * r.text_height;
            if !same_size || q.dist(r.at) > reach || region_of(*q) != region {
                continue;
            }
            // Another room's: its number already, or inside its outline.
            let held = rooms.iter().enumerate().any(|(o, x)| {
                o != i
                    && ((x.number.as_deref().map(str::trim) == Some(t.trim()) && region_of(x.at) == region)
                        || cadaddy_geom::point_in_polygon(&x.outline, *q))
            });
            if !held {
                pairs.push((q.dist(r.at), i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut taken = 0;
    for (_, i, j) in pairs {
        let Some(t) = numbers.get(j).map(|n| n.1.trim().to_string()) else { continue };
        let Some(region) = rooms.get(i).filter(|r| r.number.is_none()).map(|r| region_of(r.at)) else { continue };
        // A number one room took (the same tag, or its copy on the sheet) no other takes.
        if rooms.iter().any(|x| x.number.as_deref() == Some(t.as_str()) && region_of(x.at) == region) {
            continue;
        }
        if let Some(r) = rooms.get_mut(i) {
            r.number = Some(t);
            r.confidence = (r.confidence + 0.1).min(1.0);
            taken += 1;
        }
    }
    taken
}

pub(super) fn run_rooms(s: &mut Session, p: &Value) -> Result<Value> {
    let with_outline = bool_or(p, "outline", true);
    let items: Vec<Value> = rooms_found(s)?
        .into_iter()
        .map(|r| {
            let mut v = json!({
                "name": r.name, "number": r.number, "at": pt(r.at), "area": round(r.area), "perimeter": round(r.perimeter),
                "label": r.label.hex(), "textHeight": round(r.text_height), "confidence": round(r.confidence),
            });
            if with_outline {
                let step = (r.outline.len() / 64).max(1);
                v["outline"] = json!(r.outline.iter().step_by(step).map(|q| pt(*q)).collect::<Vec<_>>());
            }
            v
        })
        .collect();
    Ok(page(items, p))
}

#[cfg(test)]
mod tests {
    /// A quarter circle about `c` of radius `r` from `a0` degrees, flattened to 8 chords.
    fn quarter(c: (f64, f64), r: f64, a0: f64) -> Vec<Vec2> {
        (0..=8).map(|i| (a0 + f64::from(i) * 90.0 / 8.0).to_radians()).map(|a| Vec2::new(c.0 + r * a.cos(), c.1 + r * a.sin())).collect()
    }

    #[test]
    fn a_swing_joined_to_its_leaf_and_jamb_is_still_read() {
        // Alone: the whole run.
        let alone = quarter((0.0, 0.0), 36.0, 0.0);
        let runs = flat_swings(&alone);
        assert_eq!(runs.len(), 1, "{runs:?}");
        // As a print joins it: the leaf from the hinge, the arc, then on down the jamb's face.
        let mut joined = vec![Vec2::new(0.0, 0.0)];
        joined.extend(quarter((0.0, 0.0), 36.0, 0.0));
        joined.push(Vec2::new(0.0, 60.0));
        assert!(flat_arc(&joined).is_none(), "the whole run is no arc");
        let runs = flat_swings(&joined);
        assert_eq!(runs.len(), 1, "{runs:?}");
        let (c, r, deg, _) = runs[0];
        assert!(c.dist(Vec2::new(0.0, 0.0)) < 0.5 && (r - 36.0).abs() < 0.5 && (deg - 90.0).abs() < 1.0, "{runs:?}");
        // A double door's two swings meeting in the middle, leaf to leaf, in one polyline.
        let mut pair = vec![Vec2::new(0.0, 0.0)];
        pair.extend(quarter((0.0, 0.0), 36.0, 0.0));
        pair.extend(quarter((0.0, 72.0), 36.0, 270.0).into_iter().skip(1));
        pair.push(Vec2::new(0.0, 72.0));
        let runs = flat_swings(&pair);
        assert_eq!(runs.len(), 2, "{runs:?}");
        // A straight line running on from the arc at a tangent is trimmed off.
        let mut tangent = quarter((0.0, 0.0), 36.0, 0.0);
        tangent.push(Vec2::new(-30.0, 36.0));
        tangent.insert(0, Vec2::new(36.0, -40.0));
        let runs = flat_swings(&tangent);
        assert_eq!(runs.len(), 1, "{runs:?}");
    }

    #[test]
    fn corners_and_hostile_points_read_no_swing() {
        // A room outline, a zigzag (furniture hatching), points with no length, NaNs.
        let outline = [(0.0, 0.0), (100.0, 0.0), (100.0, 80.0), (0.0, 80.0), (0.0, 10.0)];
        let zig: Vec<(f64, f64)> = (0..40).map(|i| (f64::from(i) * 3.0, if i % 2 == 0 { 0.0 } else { 3.0 })).collect();
        let same = [(5.0, 5.0); 12];
        let nan = [(f64::NAN, 0.0), (1.0, f64::INFINITY), (2.0, 2.0), (3.0, 1.0), (4.0, 0.0)];
        for pts in [outline.to_vec(), zig, same.to_vec(), nan.to_vec(), Vec::new(), vec![(1.0, 1.0)]] {
            let pts: Vec<Vec2> = pts.into_iter().map(|(x, y)| Vec2::new(x, y)).collect();
            assert!(flat_swings(&pts).is_empty(), "{pts:?}");
        }
    }

    #[test]
    fn a_rooms_printed_area_is_never_its_name() {
        for t in ["105 SF", "1,960 S.F.", "12 SQ FT", "11 m2", "58 sf"] {
            assert!(area_figure(t), "{t}");
            assert_eq!(room_name_score(t), 0, "{t}");
        }
        for t in ["BAKING", "SF OFFICE", "105", "ROOM 105"] {
            assert!(!area_figure(t), "{t}");
        }
    }

    use super::*;

    fn seg(a: (f64, f64), b: (f64, f64), w: f64) -> Seg {
        Seg { a: Vec2::new(a.0, a.1), b: Vec2::new(b.0, b.1), layer: "A-WALL".into(), outline_width: w, unit_run: false }
    }

    fn rect_sides(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Seg> {
        let r = [Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)];
        let w = outline_width(&r);
        (0..4).map(|i| (r[i], r[(i + 1) % 4])).map(|(a, b)| seg((a.x, a.y), (b.x, b.y), w)).collect()
    }

    #[test]
    fn a_dashed_line_is_no_wall_face_but_a_wall_broken_by_walls_or_windows_is() {
        // Inches (k = 1). An overhead line across a 40" opening drawn as 18" dashes with 9" gaps.
        let mut segs: Vec<Seg> = (0..5).map(|i| f64::from(i) * 27.0).map(|x| seg((x, 3.3), (x + 18.0, 3.3), f64::INFINITY)).collect();
        // A corridor face broken every 25' by 7" party walls; a facade with 36" windows between
        // 24" piers; a wall face drawn in two pieces.
        segs.extend((0..4).map(|i| f64::from(i) * 307.0).map(|x| seg((x, 100.0), (x + 300.0, 100.0), f64::INFINITY)));
        segs.extend((0..4).map(|i| f64::from(i) * 60.0).map(|x| seg((x, 200.0), (x + 24.0, 200.0), f64::INFINITY)));
        segs.extend([seg((0.0, 300.0), (20.0, 300.0), f64::INFINITY), seg((26.0, 300.0), (46.0, 300.0), f64::INFINITY)]);
        // The dashes drawn right to left, and a vertical dashed line.
        segs.extend((0..4).map(|i| f64::from(i) * 15.0).map(|y| seg((500.0, y + 10.0), (500.0, y), f64::INFINITY)));
        let n = segs.len();
        assert_eq!(drop_dashes(&mut segs, 1.0), 9);
        assert_eq!(segs.len(), n - 9);
        assert!(segs.iter().all(|s| (s.a.y - 3.3).abs() > 1e-9 && s.a.x != 500.0));
        assert_eq!(segs.iter().filter(|s| s.a.y == 100.0).count(), 4);
        assert_eq!(segs.iter().filter(|s| s.a.y == 200.0).count(), 4);
        assert_eq!(segs.iter().filter(|s| s.a.y == 300.0).count(), 2);
        // No units: nothing taken.
        assert_eq!(drop_dashes(&mut segs, 0.0), 0);
    }

    #[test]
    fn a_dash_dot_line_in_pieces_is_no_wall_face() {
        // Inches (k = 1). A callout's dash-dot side: 18" dashes, 2" dots, 4" spaces (4 dashes, 3
        // dots); a dash-dot-dot centre line (24", 2", 2", 3" spaces, 3 repeats and a dash).
        let mut segs = Vec::new();
        let mut x = 0.0;
        for i in 0..7 {
            let len = if i % 2 == 0 { 18.0 } else { 2.0 };
            segs.push(seg((x, 0.0), (x + len, 0.0), f64::INFINITY));
            x += len + 4.0;
        }
        let mut x = 0.0;
        for i in 0..10 {
            let len = if i % 3 == 0 { 24.0 } else { 2.0 };
            segs.push(seg((x, 60.0), (x + len, 60.0), f64::INFINITY));
            x += len + 3.0;
        }
        // Two dashes and a dot are too few to tell; a wall face in two pieces with a post between.
        segs.extend([seg((0.0, 120.0), (18.0, 120.0), f64::INFINITY), seg((22.0, 120.0), (24.0, 120.0), f64::INFINITY)]);
        segs.push(seg((28.0, 120.0), (46.0, 120.0), f64::INFINITY));
        segs.extend([seg((0.0, 180.0), (28.0, 180.0), f64::INFINITY), seg((34.0, 180.0), (62.0, 180.0), f64::INFINITY)]);
        assert_eq!(drop_dashes(&mut segs, 1.0), 17);
        assert!(segs.iter().all(|s| s.a.y >= 120.0), "{segs:?}");
        assert_eq!(segs.len(), 5);
    }

    fn lwpoly(pts: &[(f64, f64, f64)], closed: bool) -> cadaddy_doc::LwPolyline {
        let vertices: Vec<Value> =
            pts.iter().map(|&(x, y, b)| json!({"p": {"x": x, "y": y}, "bulge": b, "start_width": 0.0, "end_width": 0.0})).collect();
        serde_json::from_value(json!({"vertices": vertices, "closed": closed})).unwrap()
    }

    #[test]
    fn rounded_corners_are_arcs_and_faceted_arcs_not_corners_or_jitter() {
        // Inches (k = 1). A callout's corner piece: a dash, a quarter circle in six chords, a dash.
        let mut pts = vec![(-10.0, -12.0, 0.0)];
        pts.extend((0..=6).map(|i| (f64::from(i) * 15.0 + 270.0).to_radians()).map(|a| (12.0 * a.cos(), 12.0 * a.sin(), 0.0)));
        pts.push((12.0, 8.0, 0.0));
        assert!(has_rounded_corner(&lwpoly(&pts, false), 1.0));
        // An arc (a bulge) between two sides.
        assert!(has_rounded_corner(&lwpoly(&[(0.0, 0.0, 0.0), (20.0, 0.0, 0.414), (30.0, 10.0, 0.0), (30.0, 40.0, 0.0)], false), 1.0));
        // A rectangle, an octagon (45° corners), a wall face with a jog, a line drawn with jitter.
        assert!(!has_rounded_corner(&lwpoly(&[(0.0, 0.0, 0.0), (40.0, 0.0, 0.0), (40.0, 6.0, 0.0), (0.0, 6.0, 0.0)], true), 1.0));
        let oct: Vec<(f64, f64, f64)> = (0..8).map(|i| (f64::from(i) * 45.0).to_radians()).map(|a| (10.0 * a.cos(), 10.0 * a.sin(), 0.0)).collect();
        assert!(!has_rounded_corner(&lwpoly(&oct, true), 1.0));
        assert!(!has_rounded_corner(&lwpoly(&[(0.0, 0.0, 0.0), (50.0, 0.0, 0.0), (50.0, 2.0, 0.0), (90.0, 2.0, 0.0)], false), 1.0));
        let jitter: Vec<(f64, f64, f64)> = (0..40).map(|i| (f64::from(i) * 0.1, 0.01 * f64::from(i % 3), 0.0)).collect();
        assert!(!has_rounded_corner(&lwpoly(&jitter, false), 1.0));
        // A circle in chords is all arc.
        let circle: Vec<(f64, f64, f64)> =
            (0..24).map(|i| (f64::from(i) * 15.0).to_radians()).map(|a| (10.0 * a.cos(), 10.0 * a.sin(), 0.0)).collect();
        assert!(has_rounded_corner(&lwpoly(&circle, true), 1.0));
        // No units: nothing to probe.
        let e = std::sync::Arc::new(Entity::new(cadaddy_doc::Handle(1), EntityKind::LwPolyline(lwpoly(&circle, true))));
        assert!(rounded_outlines_alone(&[&e], 0.0, &Limits { min_thick: 2.5, max_thick: 20.0, min_len: 12.0 }).is_empty());
        assert_eq!(rounded_outlines_alone(&[&e], 1.0, &Limits { min_thick: 2.5, max_thick: 20.0, min_len: 12.0 }).len(), 1);
    }

    #[test]
    fn a_wall_drawn_block_by_block_is_one_face_and_no_dashed_line() {
        // Inches (k = 1). Six 15.6" units with 0.4" joints: a masonry wall's face.
        let mut segs: Vec<Seg> = (0..6).map(|i| f64::from(i) * 16.0).map(|x| seg((x, 0.0), (x + 15.6, 0.0), f64::INFINITY)).collect();
        // A real dashed line beside it stays dashes: 18" dashes, 9" spaces.
        segs.extend((0..5).map(|i| f64::from(i) * 27.0).map(|x| seg((x, 50.0), (x + 18.0, 50.0), f64::INFINITY)));
        assert_eq!(drop_dashes(&mut segs, 1.0), 5, "only the dashed line's pieces go");
        assert_eq!(join_unit_runs(&mut segs, 1.0), 5);
        let [one] = segs.as_slice() else { panic!("{}", segs.len()) };
        assert!((one.a.x).abs() < 1e-9 && (one.b.x - 95.6).abs() < 1e-9 && one.a.y == 0.0, "{:?} {:?}", one.a, one.b);
        // Two pieces with a joint are not a run; a door's gap ends a run.
        let mut segs = vec![seg((0.0, 0.0), (15.6, 0.0), f64::INFINITY), seg((16.0, 0.0), (31.6, 0.0), f64::INFINITY)];
        assert_eq!(join_unit_runs(&mut segs, 1.0), 0);
        let mut segs: Vec<Seg> = [0.0, 16.0, 32.0, 84.0, 100.0, 116.0].iter().map(|x| seg((*x, 0.0), (*x + 15.6, 0.0), f64::INFINITY)).collect();
        assert_eq!(join_unit_runs(&mut segs, 1.0), 4);
        assert_eq!(segs.len(), 2, "the opening stays open");
    }

    #[test]
    fn a_storey_of_pieces_lists_only_its_runs_lines() {
        // A run of units, long lines drawn whole (one starting at -0.0, one at 0.0), and a
        // storey's worth of other pieces (each line is looked up among the pieces as drawn, not
        // checked against every one of them).
        let mut segs: Vec<Seg> = (0..6).map(|i| f64::from(i) * 16.0).map(|x| seg((x, 0.0), (x + 15.6, 0.0), f64::INFINITY)).collect();
        segs.push(seg((-0.0, 500.0), (400.0, 500.0), f64::INFINITY));
        for i in 0..200_000 {
            let (x, y) = (f64::from(i % 500) * 7.0 + 1000.0, f64::from(i / 500) * 7.0 + 1000.0);
            segs.push(seg((x, y), (x + 40.0, y + 3.0 + f64::from(i % 7)), f64::INFINITY));
        }
        let mut probe = segs.clone();
        probe.push(seg((0.0, 500.0), (400.0, 500.0), f64::INFINITY));
        let lines = unit_run_lines(&probe, 1.0);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let (a, b) = lines[0];
        assert!(a.x.abs() < 1e-9 && (b.x - 95.6).abs() < 1e-9, "{a:?} {b:?}");
        assert!(unit_run_lines(&segs[6..], 1.0).is_empty());
    }

    #[test]
    fn specks_on_a_block_walls_face_and_cut_blocks_at_its_ends_dont_break_it_into_blocks() {
        // Inches (k = 1). Eight 15.6" units with 0.4" joints from x 8, a cut unit 7.6" before them
        // (x 0–7.6) and one 9.6" after (x 136–145.6): one face from 0 to 145.6.
        let face = |y: f64| -> Vec<Seg> {
            let mut s = vec![seg((0.0, y), (7.6, y), f64::INFINITY), seg((136.0, y), (145.6, y), f64::INFINITY)];
            s.extend((0..8).map(|i| 8.0 + f64::from(i) * 16.0).map(|x| seg((x, y), (x + 15.6, y), f64::INFINITY)));
            s
        };
        // A stippled plan: specks of the block's fill on the face, in each unit and in a joint,
        // drawn either way; and one unit's face drawn twice (an outline and a line over it).
        let mut segs = face(0.0);
        segs.extend((0..8).map(|i| 16.0 + f64::from(i) * 16.0).map(|x| seg((x + 0.08, 0.0), (x, 0.0), f64::INFINITY)));
        segs.push(seg((55.7, 0.0), (55.9, 0.0), f64::INFINITY));
        segs.push(seg((40.0, 0.0), (55.6, 0.0), f64::INFINITY));
        let n = segs.len();
        assert_eq!(join_unit_runs(&mut segs, 1.0), n - 1);
        let [one] = segs.as_slice() else { panic!("{} pieces: {segs:?}", segs.len()) };
        assert!(one.a.x.abs() < 1e-9 && (one.b.x - 145.6).abs() < 1e-9, "{:?} {:?}", one.a, one.b);
        // The same face drawn right to left, in drawing units of ½" (k = 2).
        let mut segs: Vec<Seg> = face(0.0)
            .into_iter()
            .map(|s| Seg { a: Vec2::new(-2.0 * s.b.x, 2.0 * s.b.y), b: Vec2::new(-2.0 * s.a.x, 2.0 * s.a.y), ..s })
            .chain((0..8).map(|i| -2.0 * (12.0 + f64::from(i) * 16.0)).map(|x| seg((x, 0.0), (x + 0.16, 0.0), f64::INFINITY)))
            .collect();
        let n = segs.len();
        assert_eq!(join_unit_runs(&mut segs, 2.0), n - 1);
        // A cut piece shorter than a joint, a speck alone or a piece longer than a unit a joint
        // past a run is no cut unit: the run ends at its last full unit.
        let mut segs = face(0.0);
        segs.push(seg((145.8, 0.0), (146.0, 0.0), f64::INFINITY));
        segs.push(seg((146.4, 0.0), (176.4, 0.0), f64::INFINITY));
        join_unit_runs(&mut segs, 1.0);
        assert!(segs.iter().any(|s| (s.b.x - 145.6).abs() < 1e-9 && s.a.x.abs() < 1e-9), "{segs:?}");
        assert!(segs.iter().any(|s| s.a.x == 146.4), "the long piece stays its own");
    }

    #[test]
    fn a_thin_outline_beside_another_walls_face_is_no_wall_but_a_thin_outline_face_is() {
        // Inches. A 7" party wall (faces y 0 and y 7, 10' long) with a closed 1.8" × 3' rectangle
        // 4" off each face (trim drawn beside it at a unit's entry).
        let rect = [Vec2::new(10.0, 11.0), Vec2::new(46.0, 11.0), Vec2::new(46.0, 12.8), Vec2::new(10.0, 12.8)];
        let w = outline_width(&rect);
        assert!((1.6..1.8).contains(&w), "{w}");
        assert!(outline_width(&rect[..2]).is_infinite());
        let mut segs = vec![seg((0.0, 0.0), (120.0, 0.0), f64::INFINITY), seg((0.0, 7.0), (120.0, 7.0), f64::INFINITY)];
        segs.extend(rect_sides(10.0, 11.0, 46.0, 12.8));
        segs.extend(rect_sides(10.0, -5.8, 46.0, -4.0));
        let lim = Limits { min_thick: 2.5, max_thick: 20.0, min_len: 12.0 };
        let d = Drawing::default();
        let mut found = pairs(&d, &segs, &lim);
        let between = |p: &Pair| {
            let y = (p.c0.y + p.c1.y) / 2.0;
            y - p.thickness / 2.0 >= 7.0 - 1e-6 || y + p.thickness / 2.0 <= 1e-6
        };
        // Before: the wall's faces pair with the rectangles' sides into 4"/5.8" "walls" in the air.
        assert!(found.iter().filter(|p| between(p)).count() >= 4, "{:?}", found.iter().map(|p| p.thickness).collect::<Vec<_>>());
        let dropped = drop_sliver_pairs(&segs, &mut found, &lim);
        assert!(dropped >= 4, "{dropped}");
        assert!(!found.iter().any(between), "{:?}", found.iter().map(|p| (p.c0, p.thickness)).collect::<Vec<_>>());
        // The party wall itself stays.
        assert!(found.iter().any(|p| (p.thickness - 7.0).abs() < 1e-6));

        // A wall whose one face is a thin outline (a jamb or a finish layer drawn closed) and
        // whose other face bounds nothing else: it stays.
        let mut segs = rect_sides(0.0, 0.0, 48.0, 1.5);
        segs.push(seg((0.0, 6.0), (48.0, 6.0), f64::INFINITY));
        let mut found = pairs(&d, &segs, &lim);
        assert!(!found.is_empty());
        assert_eq!(drop_sliver_pairs(&segs, &mut found, &lim), 0);
        assert!(found.iter().any(|p| (p.thickness - 6.0).abs() < 1e-6));

        // Two walls drawn with thin finish layers, side by side 5" apart: neither is dropped.
        let mut segs = rect_sides(0.0, 0.0, 48.0, 0.6);
        segs.extend(rect_sides(0.0, 4.1, 48.0, 4.7));
        segs.extend(rect_sides(0.0, 9.7, 48.0, 10.3));
        segs.extend(rect_sides(0.0, 13.8, 48.0, 14.4));
        let mut found = pairs(&d, &segs, &lim);
        let n = found.len();
        assert!(n > 4);
        assert_eq!(drop_sliver_pairs(&segs, &mut found, &lim), 0);
    }

    #[test]
    fn one_loop_from_another_corner_or_the_other_way_round_is_the_same_room() {
        let v = |x: f64, y: f64| Vec2::new(x, y);
        let a = [v(0.0, 0.0), v(10.0, 0.0), v(10.0, 5.0), v(0.0, 5.0)];
        let b = [v(10.0, 5.0), v(0.0, 5.0), v(0.0, 0.0), v(10.0, 0.0)];
        let c = [v(0.0, 5.0), v(10.0, 5.0), v(10.0, 0.0), v(0.0, 0.0)];
        assert!(same_loop(&a, &b) && same_loop(&a, &c));
        assert!(!same_loop(&a, &[v(0.0, 0.0), v(11.0, 0.0), v(11.0, 5.0), v(0.0, 5.0)]));
        assert!(!same_loop(&a, &a[..3]));
    }

    /// A swing wider than a quarter circle is a door only with something drawn from its centre.
    #[test]
    fn a_half_circle_alone_is_no_door_but_a_wide_swing_with_its_leaf_is() {
        let arc = |sweep: f64, n: u32| -> Vec<[f64; 2]> {
            (0..=n)
                .map(|i| {
                    let a = (180.0 + sweep * f64::from(i) / f64::from(n)).to_radians();
                    [36.0 * a.cos(), 36.0 * a.sin()]
                })
                .collect()
        };
        let leaves = |lines: &[Vec<[f64; 2]>]| {
            let mut s = Session::new();
            for l in lines {
                s.execute("pline", &serde_json::json!({"vertices": l, "closed": false})).unwrap();
            }
            door_leaves(s.doc().unwrap(), 1.0).len()
        };
        let wide = arc(170.0, 16);
        assert_eq!(leaves(std::slice::from_ref(&wide)), 0);
        let leaf = vec![[0.0, 0.0], wide[16]];
        assert_eq!(leaves(&[wide, leaf]), 2);
        // A quarter swing needs no leaf drawn.
        assert_eq!(leaves(&[arc(90.0, 16)]), 2);
        // Past a half circle: no door's.
        assert_eq!(leaves(&[arc(220.0, 16), vec![[0.0, 0.0], [36.0, 0.0]]]), 0);
    }

    #[test]
    fn a_short_room_name_is_no_device_mark() {
        // Back of house and the trades' equipment room: rooms, though shaped like a device mark.
        for t in ["BOH", "MEP", "RR", "IDF"] {
            assert!(!equipment_tag(t) && room_name_score(t) > 0, "{t}");
        }
        for t in ["XA", "EF", "J"] {
            assert!(equipment_tag(t), "{t}");
        }
    }

    #[test]
    fn a_parenthesised_line_is_the_name_above_it_qualified() {
        let h = cadaddy_doc::Handle(1);
        let mut labels = vec![
            (h, "SINGLE Q ADA".to_string(), Vec2::new(1722.0, 1112.0), 11.9),
            (h, "(RIS)".to_string(), Vec2::new(1756.0, 1092.0), 11.9),
            (h, "118".to_string(), Vec2::new(1761.0, 1071.0), 11.9),
            // A note's bracket far from any name stays as it is.
            (h, "(N.I.C.)".to_string(), Vec2::new(5000.0, 0.0), 11.9),
        ];
        lift_qualifiers(&mut labels);
        let names: Vec<&str> = labels.iter().map(|l| l.1.as_str()).collect();
        assert_eq!(names, ["SINGLE Q ADA", "SINGLE Q ADA", "118", "(N.I.C.)"]);
    }

    #[test]
    fn a_print_sets_last_sheets_are_read_as_fully_as_its_first() {
        // Four sheets side by side, each with more labels than one reading traces in all; the
        // labels in drawing order (sheet 1's first), all as good names.
        let per_sheet = MAX_ROOMS / 2 + 10;
        let regions: Vec<[f64; 4]> = (0..4).map(|i| [f64::from(i) * 100.0, 0.0, f64::from(i) * 100.0 + 90.0, 90.0]).collect();
        let names: Vec<(usize, Vec2)> =
            (0..4).flat_map(|i| (0..per_sheet).map(move |j| (i, Vec2::new(f64::from(i as u32) * 100.0 + 5.0, 5.0 + (j % 80) as f64)))).collect();
        let by_sheet = |picked: &[(usize, Vec2)]| (0..4).map(|i| picked.iter().filter(|(s, _)| *s == i).count()).collect::<Vec<_>>();
        // Read over the whole drawing: the last sheets get nothing.
        let whole = labels_to_trace(names.clone(), |n| n.1, |_| 3, None);
        assert_eq!(by_sheet(&whole), [per_sheet, MAX_ROOMS - per_sheet, 0, 0]);
        // Sheet by sheet: each its own share, labels outside every sheet left out.
        let mut with_stray = names;
        with_stray.push((9, Vec2::new(-50.0, 5.0)));
        let picked = labels_to_trace(with_stray, |n| n.1, |_| 3, Some(&regions));
        let each = per_sheet.min(MAX_ROOMS_PER_REGION);
        assert_eq!(by_sheet(&picked), [each, each, each, each]);
        assert!(picked.iter().all(|(s, _)| *s != 9));
        // The best names first within a sheet.
        let scored = labels_to_trace(vec![(0, Vec2::new(5.0, 5.0)), (1, Vec2::new(6.0, 5.0))], |n| n.1, |n| u8::from(n.0 == 1), Some(&regions));
        assert_eq!(scored.first().map(|n| n.0), Some(1));
    }

    /// A 10' × 10' room outline (inches) with a name label inside, in a new imperial drawing.
    fn one_room(label: bool) -> Session {
        let mut s = Session::empty();
        s.new_drawing(false);
        s.execute("pline", &json!({"vertices": [[0, 0], [120, 0], [120, 120], [0, 120]], "closed": true})).unwrap();
        if label {
            s.execute("text", &json!({"at": [40, 60], "text": "OFFICE", "height": 6})).unwrap();
        }
        s
    }

    #[test]
    fn a_room_trace_is_read_again_in_the_same_drawing_state_and_traced_again_after_an_edit() {
        let mut s = one_room(true);
        let first = rooms_found(&mut s).unwrap();
        assert_eq!(first.len(), 1, "the labelled outline is a room");
        assert_eq!(s.state().unwrap().room_trace.len(), 1);
        // Asked again in the same state: read, not traced again (no second entry).
        let again = rooms_found(&mut s).unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again.first().map(|r| r.label), first.first().map(|r| r.label));
        assert_eq!(s.state().unwrap().room_trace.len(), 1);
        // Taken for writing and nothing changed (a new state, the old one kept by undo): still read.
        let kept = std::sync::Arc::clone(&s.state().unwrap().doc);
        let _ = s.doc_mut().unwrap();
        assert!(!std::sync::Arc::ptr_eq(&kept, &s.state().unwrap().doc), "a new drawing state");
        assert_eq!(rooms_found(&mut s).unwrap().len(), 1);
        assert_eq!(s.state().unwrap().room_trace.len(), 1, "the trace of the unchanged state holds");
        // An edit to the plan: traced again, and the edit is seen (the outline erased: no room).
        let outline = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::LwPolyline(_))).map(|e| e.handle.hex()).unwrap();
        s.execute("erase", &json!({"handles": [outline]})).unwrap();
        assert!(rooms_found(&mut s).unwrap().is_empty(), "the erased outline bounds no room");
        assert_eq!(s.state().unwrap().room_trace.len(), 2);
        // Undo brings back the state the room was traced in: read again, not traced.
        s.execute("undo", &json!({})).unwrap();
        assert_eq!(rooms_found(&mut s).unwrap().len(), 1);
        assert_eq!(s.state().unwrap().room_trace.len(), 2);
    }

    /// A plan's closed areas (inches): `(corners, labels at (x, y))`, each label 6" high.
    fn plan_of(areas: &[([f64; 4], &[(&str, f64, f64)])]) -> Session {
        let mut s = Session::empty();
        s.new_drawing(false);
        for ([x0, y0, x1, y1], labels) in areas {
            s.execute("pline", &json!({"vertices": [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], "closed": true})).unwrap();
            for (t, x, y) in *labels {
                s.execute("text", &json!({"at": [x, y], "text": t, "height": 6})).unwrap();
            }
        }
        s
    }

    #[test]
    fn note_lines_load_strings_and_roof_labels_are_no_rooms() {
        // Q6285: a plan's general-note lines, a design-load string and a roof area's label were
        // kept as rooms. Rooms: a name over its number, a name alone, a name of two lines.
        let mut s = plan_of(&[
            ([0.0, 0.0, 240.0, 180.0], &[("OFFICE", 80.0, 90.0), ("101", 90.0, 80.0)]),
            ([300.0, 0.0, 540.0, 180.0], &[("STORAGE", 380.0, 90.0)]),
            ([600.0, 0.0, 840.0, 180.0], &[("GUEST", 680.0, 95.0), ("LAUNDRY", 680.0, 85.0)]),
            // A general note's lines, each a few words a room's name could be.
            (
                [0.0, 300.0, 400.0, 600.0],
                &[
                    ("ALL WORK SHALL", 40.0, 500.0),
                    ("COMPLY WITH THE", 40.0, 490.0),
                    ("GOVERNING CODES", 40.0, 480.0),
                    ("AND ORDINANCES", 40.0, 470.0),
                ],
            ),
            ([500.0, 300.0, 800.0, 600.0], &[("DESIGN WIND 115 MPH", 560.0, 450.0)]),
            ([900.0, 300.0, 1200.0, 600.0], &[("LOW ROOF", 1000.0, 450.0)]),
        ]);
        let rooms = rooms_found(&mut s).unwrap();
        let mut names: Vec<(String, Option<String>)> = rooms.iter().map(|r| (r.name.clone(), r.number.clone())).collect();
        names.sort();
        assert_eq!(
            names,
            vec![("GUEST".to_string(), None), ("OFFICE".to_string(), Some("101".to_string())), ("STORAGE".to_string(), None)],
            "only the rooms' tags name rooms"
        );
    }

    #[test]
    fn a_note_block_is_more_lines_than_a_rooms_name() {
        let item = |t: &str, x: f64, y: f64| (cadaddy_doc::Handle(0), t.to_string(), Vec2::new(x, y), 6.0);
        // Three lined-up lines: a block. A name over its number and a qualifier, a name of two
        // lines, a line far under another, and lines not lined up: none.
        let items = vec![
            item("ALL WORK SHALL", 0.0, 100.0),
            item("COMPLY WITH", 2.0, 90.0),
            item("THE CODES", 0.0, 80.0),
            item("DOUBLE Q ADA", 200.0, 100.0),
            item("(TUB)", 200.0, 90.0),
            item("128", 200.0, 80.0),
            item("GUEST", 400.0, 100.0),
            item("LAUNDRY", 400.0, 90.0),
            item("LOBBY", 400.0, 20.0),
            item("MECH", 600.0, 100.0),
            item("ELEC", 640.0, 90.0),
            item("STOR", 600.0, 80.0),
        ];
        let block = note_block_lines(&items);
        let mut got: Vec<usize> = block.into_iter().collect();
        got.sort_unstable();
        assert_eq!(got, vec![0, 1, 2]);
        assert!(note_block_lines(&[]).is_empty());
    }

    #[test]
    fn load_strings_and_roof_or_canopy_labels_are_notes() {
        for t in [
            "ROOF LIVE LOAD 20 PSF",
            "WIND 115 MPH",
            "3000 PSI",
            "LOW ROOF",
            "CANOPY ABOVE",
            "ROOF BELOW",
            "OVERHANG",
            "ABOVE",
            "LINE OF SOFFIT",
            "FIRST FLOOR",
            "2ND FLR",
            "MEZZANINE",
        ] {
            assert!(note_like(t) && room_name_score(t) == 0, "{t}");
        }
        // Letters one by one, and a rated wall's label (a title block's or a legend's word).
        assert_eq!(room_name_score("L J"), 0);
        assert!(!crate::cmd::autospec::room_like_name("1 HOUR FIRE RATED BARRIER"));
        for t in ["OFFICE", "LOADING DOCK", "GUEST LAUNDRY", "DOUBLE Q", "ROOFTOP LOUNGE", "GROUND FLOOR LOBBY", "BOH"] {
            assert!(!note_like(t) && room_name_score(t) > 0, "{t}");
        }
    }

    #[test]
    fn a_trace_holds_its_own_tag() {
        let sq = [Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(100.0, 100.0), Vec2::new(0.0, 100.0)];
        let corners = |at: Vec2| [at + Vec2::new(1.0, 1.0), at + Vec2::new(20.0, 1.0), at + Vec2::new(1.0, 5.0), at + Vec2::new(20.0, 5.0)];
        assert!(holds_label(&sq, Vec2::new(40.0, 50.0), &corners(Vec2::new(40.0, 50.0)), 6.0));
        // Its point on the outline (a storefront's line), or its box reaching in.
        assert!(holds_label(&sq, Vec2::new(40.0, 102.0), &corners(Vec2::new(40.0, 102.0)), 6.0));
        assert!(holds_label(&sq, Vec2::new(-10.0, 50.0), &corners(Vec2::new(-10.0, 50.0)), 6.0));
        // A loop under the tag's box: another space's.
        assert!(!holds_label(&sq, Vec2::new(40.0, 120.0), &corners(Vec2::new(40.0, 120.0)), 6.0));
    }

    #[test]
    fn a_sheet_border_is_no_room_whatever_label_sits_in_it() {
        // Q6432: once a note line no longer claimed the sheet's border, the next label inside it
        // (a schedule's header, its cell's number beside it) took the whole border for its room.
        // A 36" x 24" sheet's border at 1/2" = 1'-0" (inches: 72 x 48 ft, inside a room trace's
        // reach): a room drawn inside it, a schedule's header and a cell's number in the
        // border's corner, the schedule's rows ruled across with no cell closed.
        let mut s = plan_of(&[
            ([0.0, 0.0, 864.0, 576.0], &[("DESCRIPTION", 40.0, 530.0), ("204", 40.0, 512.0)]),
            ([400.0, 150.0, 700.0, 450.0], &[("OFFICE", 500.0, 300.0), ("101", 510.0, 282.0)]),
        ]);
        for y in [525.0, 507.0, 489.0] {
            s.execute("line", &json!({"points": [[20.0, y], [300.0, y]]})).unwrap();
        }
        // Traced in the sheet's drawing area (inside its border), in its whole rect, and in one
        // view of it: the border is never a room, the room in it is.
        for region in [[10.0, 10.0, 854.0, 566.0], [0.0, 0.0, 864.0, 576.0], [0.0, 0.0, 720.0, 576.0]] {
            let rooms = rooms_found_in(&mut s, Some(&[region])).unwrap();
            let names: Vec<(String, Option<String>)> = rooms.iter().map(|r| (r.name.clone(), r.number.clone())).collect();
            assert_eq!(names, vec![("OFFICE".to_string(), Some("101".to_string()))], "{region:?}");
        }
    }

    #[test]
    fn an_outline_reaching_every_side_of_its_area_is_its_frame() {
        let b = |x0: f64, y0: f64, x1: f64, y1: f64| cadaddy_geom::Bounds2::new(Vec2::new(x0, y0), Vec2::new(x1, y1));
        let area = [10.0, 10.0, 1010.0, 710.0];
        // The border outside the area, the area's own box, a frame stopping just short of it.
        assert!(frames_area(&b(0.0, 0.0, 1020.0, 720.0), &area));
        assert!(frames_area(&b(10.0, 10.0, 1010.0, 710.0), &area));
        assert!(frames_area(&b(15.0, 14.0, 1005.0, 706.0), &[1010.0, 710.0, 10.0, 10.0]));
        // A building or a room: short of a side, however big.
        assert!(!frames_area(&b(10.0, 10.0, 1010.0, 600.0), &area));
        assert!(!frames_area(&b(100.0, 100.0, 900.0, 600.0), &area));
        // No area to frame.
        assert!(!frames_area(&b(0.0, 0.0, 1.0, 1.0), &[0.0, 0.0, 0.0, 0.0]));
        assert!(!frames_area(&b(0.0, 0.0, 1.0, 1.0), &[0.0, 0.0, f64::NAN, 1.0]));
    }

    #[test]
    fn no_rooms_is_remembered_too() {
        // An outline with no label: the trace finds nothing, and that is kept for the state.
        let mut s = one_room(false);
        assert!(rooms_found(&mut s).unwrap().is_empty());
        assert_eq!(s.state().unwrap().room_trace.len(), 1);
        assert!(rooms_found(&mut s).unwrap().is_empty());
        assert!(rooms_found(&mut s).unwrap().is_empty());
        assert_eq!(s.state().unwrap().room_trace.len(), 1, "\"no rooms\" read, not traced again");
        // Different regions are a different question.
        assert!(rooms_found_in(&mut s, Some(&[[0.0, 0.0, 200.0, 200.0]])).unwrap().is_empty());
        assert_eq!(s.state().unwrap().room_trace.len(), 2);
    }

    /// Two rooms of 6" walls on A-WALL (OFFICE west, STORAGE east of a partition at 15'), their
    /// names on A-ROOM-TAG, a stair line in the office and a note far off the plan, under a spec
    /// naming the layers' roles; built by an associative `model.build`.
    fn spec_plan() -> Session {
        let mut s = Session::new();
        let layer = |s: &mut Session, name: &str| s.execute("layer.new", &json!({"name": name, "current": true})).unwrap();
        let ft = |x: f64, y: f64| json!([x * 12.0, y * 12.0]);
        let rect = |s: &mut Session, (x0, y0): (f64, f64), (x1, y1): (f64, f64)| {
            s.execute("pline", &json!({"vertices": [ft(x0, y0), ft(x1, y0), ft(x1, y1), ft(x0, y1)], "closed": true})).unwrap();
        };
        let line = |s: &mut Session, (x0, y0): (f64, f64), (x1, y1): (f64, f64)| {
            s.execute("line", &json!({"points": [ft(x0, y0), ft(x1, y1)]})).unwrap();
        };
        layer(&mut s, "A-WALL");
        rect(&mut s, (0.0, 0.0), (30.0, 12.0));
        rect(&mut s, (0.5, 0.5), (29.5, 11.5));
        line(&mut s, (15.0, 0.5), (15.0, 11.5));
        line(&mut s, (15.5, 0.5), (15.5, 11.5));
        layer(&mut s, "A-FLOR-STRS");
        line(&mut s, (2.0, 2.0), (8.0, 2.0));
        layer(&mut s, "A-ROOM-TAG");
        for (at, t) in [((5.0, 6.0), "OFFICE"), ((20.0, 6.0), "STORAGE"), ((80.0, 40.0), "STORAGE NOTE")] {
            s.execute("text", &json!({"at": ft(at.0, at.1), "text": t, "height": 6})).unwrap();
        }
        let spec = json!({
            "version": 1,
            "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 8, "base": [0, 0]}],
            "layers": [
                {"layer": "A-WALL", "role": "wall", "wall_type": "Wall"},
                {"layer": "A-FLOR-STRS", "role": "stair"},
                {"layer": "A-ROOM-TAG", "role": "room_tag"}
            ],
            "wall_types": [{"name": "Wall", "thickness_in": 6, "from": "pairs"}]
        });
        s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
        s.execute("model.build", &json!({})).unwrap();
        s
    }

    fn handles_on(s: &Session, layer: &str, keep: impl Fn(&Entity) -> bool) -> Vec<String> {
        s.doc()
            .unwrap()
            .model
            .iter()
            .filter(|e| crate::cmd::wallgen::made_as(e).is_none() && e.common.layer == layer && keep(e))
            .map(|e| e.handle.hex())
            .collect()
    }

    /// The rooms as (name, area to a hundredth of a square inch).
    fn found(s: &mut Session) -> Vec<(String, i64)> {
        rooms_found(s).unwrap().into_iter().map(|r| (r.name, (r.area * 100.0).round() as i64)).collect()
    }

    fn traced(s: &Session) -> usize {
        s.state().unwrap().room_trace.traced()
    }

    #[test]
    fn a_live_update_traces_rooms_only_when_what_bounds_them_changed() {
        // Soak S6-004: every update of an associative model traced every room again, also when
        // the edit touched nothing a room is bounded by (the update then rebuilt nothing).
        let mut s = spec_plan();
        let rooms = found(&mut s);
        assert_eq!(rooms.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["OFFICE", "STORAGE"], "{rooms:?}");
        let n = traced(&s);
        assert!(n >= 1);
        // The stair line moved: the model is behind its plan (a stair is part of it), and the update
        // traces no room (no wall, door or window moved, no label).
        let stair = handles_on(&s, "A-FLOR-STRS", |_| true);
        s.execute("move", &json!({"handles": stair, "from": [0, 0], "to": [12, 12]})).unwrap();
        assert!(crate::cmd::wallgen::model_behind(&s), "a stair edit leaves the model behind");
        s.execute("model.update", &json!({})).unwrap();
        assert!(!crate::cmd::wallgen::model_behind(&s));
        assert_eq!(traced(&s), n, "no room traced again for a stair line");
        assert_eq!(found(&mut s), rooms);
        assert_eq!(traced(&s), n, "nor in the rebuilt model's state");
        // The partition moved: traced again, and the bigger office is seen; the rebuilt model
        // reads that trace.
        let partition: Vec<String> = handles_on(&s, "A-WALL", |e| matches!(&e.kind, EntityKind::Line(_)));
        assert_eq!(partition.len(), 2, "its two faces");
        s.execute("move", &json!({"handles": partition, "from": [0, 0], "to": [24, 0]})).unwrap();
        s.execute("model.update", &json!({})).unwrap();
        assert_eq!(traced(&s), n + 1, "the moved partition: traced once");
        let moved = found(&mut s);
        assert_eq!(traced(&s), n + 1, "the rebuilt model's state reads that trace");
        let area = |rs: &[(String, i64)], name: &str| rs.iter().find(|r| r.0 == name).map(|r| r.1);
        assert!(area(&moved, "OFFICE") > area(&rooms, "OFFICE"), "{moved:?} vs {rooms:?}");
        // Moved back: the plan traced before, read again.
        let partition: Vec<String> = handles_on(&s, "A-WALL", |e| matches!(&e.kind, EntityKind::Line(_)));
        s.execute("move", &json!({"handles": partition, "from": [24, 0], "to": [0, 0]})).unwrap();
        s.execute("model.update", &json!({})).unwrap();
        assert_eq!(found(&mut s), rooms);
        assert_eq!(traced(&s), n + 1, "the plan as it was is read, not traced");
        // A label edited: traced again.
        let office = handles_on(&s, "A-ROOM-TAG", |e| matches!(&e.kind, EntityKind::Text(t) if t.value == "OFFICE"));
        s.execute("move", &json!({"handles": office, "from": [0, 0], "to": [6, 0]})).unwrap();
        let _ = found(&mut s);
        assert_eq!(traced(&s), n + 2, "a moved label is traced again");
    }

    #[test]
    fn a_trace_read_again_by_its_inputs_is_the_trace_done_afresh() {
        // Edits that change nothing the trace reads, then the same trace in a fresh session.
        let mut s = spec_plan();
        let first = rooms_found(&mut s).unwrap();
        let stair = handles_on(&s, "A-FLOR-STRS", |_| true);
        s.execute("erase", &json!({"handles": stair})).unwrap();
        let n = traced(&s);
        let again = rooms_found(&mut s).unwrap();
        assert_eq!(traced(&s), n, "read by its inputs");
        let mut fresh = Session::empty();
        fresh.open_drawing((*s.doc().unwrap()).clone(), "Plan", None);
        let traced_afresh = rooms_found(&mut fresh).unwrap();
        assert_eq!(traced(&fresh), 1);
        let key = |rs: &[FoundRoom]| {
            rs.iter()
                .map(|r| format!("{}|{:?}|{}|{:?}|{:.6}|{:.6}|{:.6}", r.name, r.number, r.label.hex(), r.outline, r.area, r.perimeter, r.confidence))
                .collect::<Vec<_>>()
        };
        assert_eq!(key(&again), key(&traced_afresh));
        assert_eq!(key(&again), key(&first));
    }
}
