//! Where the device a label names is drawn (Q6337): a label never locates itself.
//!
//! A circuit label, a switch designation or a fixture mark is printed beside its symbol, or set
//! clear of it with a leader to it (over a tub, outside a crowded wall, beside a keynote's
//! marker). A device the job makes from such a label goes where the label points:
//!
//! 1. the far end of a leader from its lettering (a straight piece, or a short open polyline end
//!    to end, one end by the lettering and the other off it; a wall's line, drawn with its other
//!    face beside it, is no leader), onto a symbol drawn there when one is;
//! 2. else the nearest symbol of the device's kind within reach that no other device stands on;
//! 3. else the label's own point, said so on the device (`spot`), so a review sees it was not
//!    found at a symbol.
//!
//! The rule is the drafting convention (a tag or callout is placed off its object, a leader
//! where it is far), not one set's: no sheet, mark or job value is in it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;

/// A leader's near end is within this many label heights of its lettering.
const LEADER_START_H: f64 = 1.2;
/// A leader is at least this long (shorter is a tick or an arrowhead), and at most this long, in
/// label heights (longer is a wall or a grid line).
const LEADER_MIN_H: f64 = 1.5;
const LEADER_MAX_H: f64 = 20.0;
/// A leader's pieces meet within this share of a label height; at most this many follow on.
const LEADER_JOIN_H: f64 = 0.3;
const LEADER_MAX_PIECES: usize = 4;
/// A leader is drawn at an angle to its lettering somewhere along it (a slanted piece, often after
/// a level shoulder): a piece more than this off the label's directions (radians) is one. A
/// single level or plumb line by a label is a wall, a cabinet's or a door's edge, no leader.
const LEADER_SLANT: f64 = 0.17;
/// A wall's two faces: a piece with another parallel to it this far off (in label heights,
/// overlapping it by at least half) is a wall's line, no leader.
const WALL_FACE_MIN_H: f64 = 0.15;
const WALL_FACE_MAX_H: f64 = 3.0;
/// A symbol within this many label heights of a leader's tip is the one it points at.
pub(crate) const LEADER_SNAP_H: f64 = 2.0;
/// A label's letters are about this share of its height wide.
const CHAR_W_H: f64 = 0.8;
/// Pieces of one sheet looked at for leaders, at most.
pub(crate) const MAX_LEADER_SEGS: usize = 400_000;
/// Corners of one polyline looked at as a leader, at most (more is a drawing, not a leader).
const MAX_LEADER_CORNERS: usize = 8;

/// How a device made from a label found its spot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Spot {
    /// At the end of a leader from its label (on the symbol drawn there, when one is).
    Leader,
    /// At the nearest symbol of its kind by its label (no leader from it).
    Symbol,
    /// At the nearest switch symbol by its label drawn as loose linework (no block made of it, no
    /// letter read from it; no leader from the label).
    Linework,
    /// At its label: no leader from it and no free symbol of its kind by it.
    Label,
}

impl Spot {
    /// What it says on the device (its basis), the label quoted.
    pub(crate) fn basis(self, label: &str, sheet: &str) -> String {
        let label = label.trim();
        match self {
            Spot::Leader => format!("at the end of the leader from its label {label} on {sheet}"),
            Spot::Symbol => format!("at the symbol nearest its label {label} on {sheet} (no leader from the label)"),
            Spot::Linework => {
                format!(
                    "at the switch symbol drawn as linework nearest its label {label} on {sheet} (no block or letter made of it, no leader from the label)"
                )
            }
            Spot::Label => {
                format!("at its label {label} on {sheet}: no leader from the label and no free symbol of its kind by it (location to check)")
            }
        }
    }

    /// Its key in a report's count, from a device's basis ([`Spot::basis`]).
    pub(crate) fn key_of_basis(basis: &str) -> &'static str {
        if basis.starts_with("at the end of the leader") {
            "leader"
        } else if basis.starts_with("at the symbol nearest") {
            "symbol"
        } else if basis.starts_with("at the switch symbol drawn as linework") {
            "linework"
        } else if basis.starts_with("at its label") {
            "label"
        } else {
            "symbol block"
        }
    }
}

/// A sheet's straight pieces (each line, and each short open polyline's sides and its chord
/// end to end), filed by their ends, to follow leaders on.
pub(crate) struct Leaders {
    segs: Vec<(Vec2, Vec2)>,
    cell: f64,
    ends: HashMap<(i64, i64), Vec<usize>>,
}

impl Leaders {
    /// The pieces, filed by cells of `cell` (a few label heights).
    pub(crate) fn new(segs: Vec<(Vec2, Vec2)>, cell: f64) -> Leaders {
        let cell = if cell.is_finite() && cell > 0.0 { cell } else { 1.0 };
        let segs: Vec<(Vec2, Vec2)> =
            segs.into_iter().filter(|(a, b)| a.is_finite() && b.is_finite() && a.dist(*b) > 1e-9).take(MAX_LEADER_SEGS).collect();
        let mut ends: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, (a, b)) in segs.iter().enumerate() {
            let ka = Self::key_of(*a, cell);
            let kb = Self::key_of(*b, cell);
            ends.entry(ka).or_default().push(i);
            if kb != ka {
                ends.entry(kb).or_default().push(i);
            }
        }
        Leaders { segs, cell, ends }
    }

    /// The straight pieces of a sheet's objects (lines; open polylines of a few corners, their
    /// sides and end to end: a leader drawn with a bend is one).
    pub(crate) fn of<'a>(objects: impl IntoIterator<Item = &'a cadaddy_doc::EntityKind>, cell: f64) -> Leaders {
        let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
        for k in objects {
            if segs.len() >= MAX_LEADER_SEGS {
                break;
            }
            match k {
                cadaddy_doc::EntityKind::Line(l) => segs.push((Vec2::new(l.a.x, l.a.y), Vec2::new(l.b.x, l.b.y))),
                cadaddy_doc::EntityKind::LwPolyline(pl) if !pl.closed && pl.vertices.len() <= MAX_LEADER_CORNERS => {
                    let pts: Vec<Vec2> = pl.vertices.iter().map(|v| Vec2::new(v.p.x, v.p.y)).collect();
                    segs.extend(pts.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))));
                    if pts.len() > 2
                        && let (Some(a), Some(b)) = (pts.first(), pts.last())
                    {
                        segs.push((*a, *b));
                    }
                }
                _ => {}
            }
        }
        Leaders::new(segs, cell)
    }

    fn key_of(p: Vec2, cell: f64) -> (i64, i64) {
        ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64)
    }

    /// The pieces with an end within `r` of `p` (each once).
    fn ending_near(&self, p: Vec2, r: f64) -> Vec<usize> {
        let (x0, y0) = Self::key_of(p - Vec2::new(r, r), self.cell);
        let (x1, y1) = Self::key_of(p + Vec2::new(r, r), self.cell);
        if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > 400 {
            return Vec::new();
        }
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.ends.get(&(x, y)) {
                    seen.extend(v.iter().copied());
                }
            }
        }
        seen.into_iter().filter(|i| self.segs.get(*i).is_some_and(|(a, b)| a.dist(p) <= r || b.dist(p) <= r || seg_dist(p, *a, *b) <= r)).collect()
    }

    /// Whether a piece is a wall's face: another piece parallel to it a wall's thickness off,
    /// overlapping it.
    fn walled(&self, i: usize, h: f64) -> bool {
        let Some(&(a, b)) = self.segs.get(i) else { return false };
        let len = a.dist(b);
        if len <= 0.0 {
            return false;
        }
        let u = (b - a) * (1.0 / len);
        let reach = WALL_FACE_MAX_H * h + len;
        let mut near = self.ending_near(a, reach.min(WALL_FACE_MAX_H * h + 0.5 * len));
        near.extend(self.ending_near(b, reach.min(WALL_FACE_MAX_H * h + 0.5 * len)));
        near.into_iter().filter(|j| *j != i).any(|j| {
            let Some(&(c, d)) = self.segs.get(j) else { return false };
            let l2 = c.dist(d);
            if l2 <= 0.0 {
                return false;
            }
            let v = (d - c) * (1.0 / l2);
            if u.cross(v).abs() > 0.05 {
                return false;
            }
            let off = (c - a).cross(u).abs();
            if off < WALL_FACE_MIN_H * h || off > WALL_FACE_MAX_H * h {
                return false;
            }
            let (t0, t1) = ((c - a).dot(u), (d - a).dot(u));
            let (lo, hi) = (t0.min(t1).max(0.0), t0.max(t1).min(len));
            hi - lo >= 0.5 * len.min(l2)
        })
    }

    /// The far end of a leader from a label's lettering (or from one of `with`: the lines printed
    /// with it, its keynote's marker), followed through its bends; the farthest such tip.
    pub(crate) fn leader_end(&self, label: &TextItem, with: &[&TextItem]) -> Option<Vec2> {
        let h = label.height;
        if !h.is_finite() || h <= 0.0 {
            return None;
        }
        let boxes: Vec<(Vec2, Vec2, f64)> = std::iter::once(label).chain(with.iter().copied()).map(baseline).collect();
        let by_lettering = |p: Vec2, r: f64| boxes.iter().any(|(s, e, th)| seg_dist(p, *s, *e) <= r + 0.5 * th);
        let middle = |t: &TextItem| {
            let (s, e, th) = baseline(t);
            s.mid(e) + Vec2::from_angle(t.rotation + std::f64::consts::FRAC_PI_2) * (0.5 * th)
        };
        let mut starts: BTreeSet<usize> = BTreeSet::new();
        for t in std::iter::once(label).chain(with.iter().copied()) {
            let (s, e, th) = baseline(t);
            let r = 0.5 * s.dist(e) + LEADER_START_H * h + th;
            starts.extend(self.ending_near(middle(t), r));
        }
        let mut best: Option<(f64, Vec2)> = None;
        for i in starts {
            let Some(&(a, b)) = self.segs.get(i) else { continue };
            let len = a.dist(b);
            if len < LEADER_MIN_H * h || len > LEADER_MAX_H * h {
                continue;
            }
            let (near, far) = match (by_lettering(a, LEADER_START_H * h), by_lettering(b, LEADER_START_H * h)) {
                (true, false) => (a, b),
                (false, true) => (b, a),
                _ => continue,
            };
            if self.walled(i, h) {
                continue;
            }
            // Followed on through its bends: a piece starting where it ends, going on (not back).
            let (mut tip, mut dir, mut total, mut used) = (far, (far - near) * (1.0 / len), len, vec![i]);
            let mut slanted = slant(dir, label.rotation);
            for _ in 0..LEADER_MAX_PIECES {
                let next = self.ending_near(tip, LEADER_JOIN_H * h).into_iter().filter(|j| !used.contains(j)).find_map(|j| {
                    let &(c, d) = self.segs.get(j)?;
                    let (from, to) = if c.dist(tip) <= d.dist(tip) { (c, d) } else { (d, c) };
                    let l = from.dist(to);
                    (from.dist(tip) <= LEADER_JOIN_H * h
                        && l >= 0.5 * h
                        && total + l <= LEADER_MAX_H * h
                        && ((to - from) * (1.0 / l)).dot(dir) > -0.2
                        && !by_lettering(to, LEADER_START_H * h)
                        && !self.walled(j, h))
                    .then_some((j, to, l, (to - from) * (1.0 / l)))
                });
                let Some((j, to, l, d)) = next else { break };
                used.push(j);
                total += l;
                tip = to;
                dir = d;
                slanted |= slant(d, label.rotation);
            }
            if !slanted {
                continue;
            }
            let gap = tip.dist(label.at);
            if best.is_none_or(|(g, _)| gap > g) {
                best = Some((gap, tip));
            }
        }
        best.map(|(_, p)| p)
    }
}

/// Whether a direction is slanted to a label's lettering (neither along it nor across it).
fn slant(d: Vec2, rotation: f64) -> bool {
    let r = if rotation.is_finite() { rotation } else { 0.0 };
    let a = (d.y.atan2(d.x) - r).rem_euclid(std::f64::consts::FRAC_PI_2);
    a.is_finite() && a > LEADER_SLANT && a < std::f64::consts::FRAC_PI_2 - LEADER_SLANT
}

/// A label's lettering as a line along its baseline middle (start, end) and its height.
fn baseline(t: &TextItem) -> (Vec2, Vec2, f64) {
    let h = if t.height.is_finite() && t.height > 0.0 { t.height } else { 0.0 };
    let r = if t.rotation.is_finite() { t.rotation } else { 0.0 };
    let n = t.value.trim().chars().count().min(64) as f64;
    let s = t.at + Vec2::from_angle(r + std::f64::consts::FRAC_PI_2) * (0.5 * h);
    (s, s + Vec2::from_angle(r) * (CHAR_W_H * h * n.max(1.0)), h)
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.len2();
    let u = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + ab * u)
}

/// Where the device a label names is drawn, and how that was found: the leader's end from the
/// label (onto the nearest of `symbols` within [`LEADER_SNAP_H`] of it), else the nearest of
/// `symbols` within `reach_h` label heights of the label, else the label's own point. `symbols`
/// are the free symbols of the device's kind on its sheet (none stands on them yet).
pub(crate) fn locate(label: &TextItem, with: &[&TextItem], leaders: Option<&Leaders>, symbols: &[Vec2], reach_h: f64) -> (Vec2, Spot) {
    let h = if label.height.is_finite() && label.height > 0.0 { label.height } else { 0.0 };
    let (s, e, _) = baseline(label);
    let mid = s.mid(e);
    let nearest =
        |p: Vec2, r: f64| symbols.iter().copied().filter(|q| q.is_finite() && q.dist(p) <= r).min_by(|a, b| a.dist(p).total_cmp(&b.dist(p)));
    if let Some(tip) = leaders.and_then(|l| l.leader_end(label, with)) {
        return (nearest(tip, LEADER_SNAP_H * h).unwrap_or(tip), Spot::Leader);
    }
    if h > 0.0
        && let Some(q) = nearest(mid, reach_h * h)
    {
        return (q, Spot::Symbol);
    }
    (label.at, Spot::Label)
}

/// A symbol whose strokes the lettering reader took for a letter (a switch's `S` read as `1`,
/// a box read as `O`): one or two characters lettered this many times as high as the label
/// beside it, at least and at most.
const GLYPH_MIN_H: f64 = 2.5;
const GLYPH_MAX_H: f64 = 6.0;
/// A switch designation is printed this near its symbol, in its heights.
pub(crate) const SWITCH_SYMBOL_REACH_H: f64 = 6.0;

/// The symbols drawn by a label that the import read as oversized letters (see [`GLYPH_MIN_H`]):
/// each one's middle. A switch symbol drawn as an `S` is the common case; a switch's
/// designation is printed beside it.
pub(crate) fn glyph_symbols(label: &TextItem, texts: &[&TextItem]) -> Vec<Vec2> {
    let h = label.height;
    if !h.is_finite() || h <= 0.0 {
        return Vec::new();
    }
    let (s, e, _) = baseline(label);
    let mid = s.mid(e);
    texts
        .iter()
        .filter(|t| {
            let n = t.value.trim().chars().count();
            (1..=2).contains(&n) && t.height.is_finite() && t.height >= GLYPH_MIN_H * h && t.height <= GLYPH_MAX_H * h
        })
        .map(|t| {
            let (a, b, _) = baseline(t);
            a.mid(b)
        })
        .filter(|p| p.is_finite() && p.dist(mid) <= (SWITCH_SYMBOL_REACH_H + GLYPH_MAX_H) * h)
        .collect()
}

/// A switch symbol drawn as plain linework (Q6602): the `S` the import neither made a block of
/// nor read as a letter, its strokes left loose by the designation printed beside it. Its strokes
/// together are this many designation heights across, at least and at most (smaller is lettering
/// of the designation's own size, larger a fixture's or a wall's drawing).
const SHAPE_MIN_H: f64 = 1.5;
const SHAPE_MAX_H: f64 = GLYPH_MAX_H;
/// Its narrower side is at least this share of its wider one (thinner is a line, a tick).
const SHAPE_ASPECT: f64 = 0.25;
/// Its strokes meet within this share of a designation height.
const SHAPE_JOIN_H: f64 = 0.2;
/// It is drawn curving both ways, as a letter `S` is (each way at least this far, radians,
/// along one of its strokes followed end to end). A box, a triangle, a circle and a receptacle's
/// circle and bars turn one way or none: those are other symbols.
const SHAPE_TURN: f64 = 1.2;
/// Strokes of one shape, corners of one stroke, strokes looked at by one label, and strokes of
/// one sheet filed, at most.
const MAX_SHAPE_STROKES: usize = 48;
const MAX_STROKE_POINTS: usize = 128;
const MAX_NEAR_STROKES: usize = 600;
pub(crate) const MAX_STROKES: usize = 400_000;

/// A sheet's short strokes (lines, polylines with their arcs, arcs, filled outlines' loops), each
/// no larger than a switch symbol drawn by lettering `h` high, filed by their middles: to find the
/// symbols drawn as loose linework by a designation (Q6602).
pub(crate) struct Strokes {
    strokes: Vec<(Vec<Vec2>, [f64; 4])>,
    cell: f64,
    largest: f64,
    grid: HashMap<(i64, i64), Vec<usize>>,
}

impl Strokes {
    /// The strokes of a sheet's objects lettered about `h` high.
    pub(crate) fn of<'a>(objects: impl IntoIterator<Item = &'a cadaddy_doc::EntityKind>, h: f64) -> Strokes {
        let h = if h.is_finite() && h > 0.0 { h } else { 1.0 };
        let largest = SHAPE_MAX_H * h;
        let mut s = Strokes { strokes: Vec::new(), cell: largest, largest, grid: HashMap::new() };
        let mut add = |pts: Vec<Vec2>| {
            if s.strokes.len() >= MAX_STROKES || pts.len() < 2 || pts.len() > MAX_STROKE_POINTS || !pts.iter().all(|p| p.is_finite()) {
                return;
            }
            let b = pts.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
                [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)]
            });
            let size = (b[2] - b[0]).max(b[3] - b[1]);
            if !(size > 0.0 && size <= largest) {
                return;
            }
            let key = Self::key_of(Vec2::new((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0), s.cell);
            s.grid.entry(key).or_default().push(s.strokes.len());
            s.strokes.push((pts, b));
        };
        for k in objects {
            match k {
                cadaddy_doc::EntityKind::Line(l) => add(vec![Vec2::new(l.a.x, l.a.y), Vec2::new(l.b.x, l.b.y)]),
                cadaddy_doc::EntityKind::LwPolyline(pl) if pl.vertices.len() < MAX_STROKE_POINTS / 2 => {
                    let n = pl.vertices.len();
                    let mut pts: Vec<Vec2> = Vec::new();
                    for (i, v) in pl.vertices.iter().enumerate() {
                        let a = Vec2::new(v.p.x, v.p.y);
                        pts.push(a);
                        // An arc side's middle (its bulge): the stroke curves there.
                        let next = if i + 1 < n {
                            pl.vertices.get(i + 1)
                        } else if pl.closed {
                            pl.vertices.first()
                        } else {
                            None
                        };
                        if let Some(w) = next {
                            let b = Vec2::new(w.p.x, w.p.y);
                            let d = b - a;
                            let l = d.len();
                            if v.bulge.is_finite() && v.bulge.abs() > 1e-6 && l > 0.0 {
                                pts.push(a.mid(b) + Vec2::new(d.y, -d.x) * (v.bulge * 0.5));
                            }
                            if i + 1 == n {
                                pts.push(b);
                            }
                        }
                    }
                    add(pts);
                }
                cadaddy_doc::EntityKind::Arc(a) if a.radius.is_finite() && a.radius > 0.0 && a.start.is_finite() && a.end.is_finite() => {
                    let c = Vec2::new(a.center.x, a.center.y);
                    let mut sweep = (a.end - a.start).rem_euclid(std::f64::consts::TAU);
                    if sweep <= 1e-9 {
                        sweep = std::f64::consts::TAU;
                    }
                    add((0..=12).map(|i| c + Vec2::from_angle(a.start + sweep * f64::from(i) / 12.0) * a.radius).collect());
                }
                cadaddy_doc::EntityKind::Hatch(hh) => {
                    for lp in hh.loops.iter().take(8) {
                        let mut pts: Vec<Vec2> = lp.vertices.iter().take(MAX_STROKE_POINTS).map(|v| Vec2::new(v.p.x, v.p.y)).collect();
                        if let Some(f) = pts.first().copied() {
                            pts.push(f);
                        }
                        add(pts);
                    }
                }
                _ => {}
            }
        }
        s
    }

    fn key_of(p: Vec2, cell: f64) -> (i64, i64) {
        ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64)
    }

    /// The strokes with their middles within `r` of `p`, nearest first, at most
    /// [`MAX_NEAR_STROKES`].
    fn near(&self, p: Vec2, r: f64) -> Vec<usize> {
        let (x0, y0) = Self::key_of(p - Vec2::new(r, r), self.cell);
        let (x1, y1) = Self::key_of(p + Vec2::new(r, r), self.cell);
        if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > 400 {
            return Vec::new();
        }
        let mid = |b: &[f64; 4]| Vec2::new((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        let mut got: Vec<(f64, usize)> = Vec::new();
        for x in x0..=x1 {
            for y in y0..=y1 {
                for &i in self.grid.get(&(x, y)).into_iter().flatten() {
                    if let Some((_, b)) = self.strokes.get(i) {
                        let d = mid(b).dist(p);
                        if d <= r {
                            got.push((d, i));
                        }
                    }
                }
            }
        }
        got.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        got.into_iter().take(MAX_NEAR_STROKES).map(|(_, i)| i).collect()
    }

    /// The middles of the switch symbols drawn as loose linework by a designation, nearest first:
    /// strokes meeting each other into one shape at a switch symbol's size
    /// ([`SHAPE_MIN_H`]–[`SHAPE_MAX_H`] designation heights across), within
    /// [`SWITCH_SYMBOL_REACH_H`] of it, curving both ways as an `S` does ([`SHAPE_TURN`]), on no
    /// lettering of `texts` (a word drawn by strokes is lettering) and with no device of `taken`
    /// on it.
    pub(crate) fn switch_shapes(&self, label: &TextItem, texts: &[&TextItem], taken: &[Vec2]) -> Vec<Vec2> {
        let h = label.height;
        if !h.is_finite() || h <= 0.0 {
            return Vec::new();
        }
        let (s, e, _) = baseline(label);
        let mid = s.mid(e);
        let reach = SWITCH_SYMBOL_REACH_H * h;
        let ids = self.near(mid, reach + SHAPE_MAX_H * h + 2.0 * self.largest);
        let join = SHAPE_JOIN_H * h;
        // The strokes meeting each other: one shape.
        let mut root: Vec<usize> = (0..ids.len()).collect();
        fn find(root: &mut [usize], mut i: usize) -> usize {
            for _ in 0..MAX_NEAR_STROKES {
                let Some(&r) = root.get(i) else { return i };
                if r == i {
                    return i;
                }
                i = r;
            }
            i
        }
        let strokes: Vec<&(Vec<Vec2>, [f64; 4])> = ids.iter().filter_map(|i| self.strokes.get(*i)).collect();
        for (i, a) in strokes.iter().enumerate() {
            for (j, b) in strokes.iter().enumerate().skip(i + 1) {
                let (ba, bb) = (a.1, b.1);
                if ba[0] - join > bb[2] || bb[0] - join > ba[2] || ba[1] - join > bb[3] || bb[1] - join > ba[3] {
                    continue;
                }
                if meet(&a.0, &b.0, join) {
                    let (ri, rj) = (find(&mut root, i), find(&mut root, j));
                    if let Some(r) = root.get_mut(ri) {
                        *r = rj;
                    }
                }
            }
        }
        let mut shapes: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..strokes.len() {
            let r = find(&mut root, i);
            shapes.entry(r).or_default().push(i);
        }
        let on_lettering = |p: Vec2| {
            std::iter::once(label).chain(texts.iter().copied()).any(|t| {
                let (a, b, th) = baseline(t);
                th > 0.0 && seg_dist(p, a, b) <= 0.5 * th
            })
        };
        let mut out: Vec<(f64, Vec2)> = Vec::new();
        for members in shapes.values() {
            if members.len() > MAX_SHAPE_STROKES {
                continue;
            }
            let b = members
                .iter()
                .filter_map(|i| strokes.get(*i))
                .fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, (_, q)| {
                    [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[2]), b[3].max(q[3])]
                });
            let (w, ht) = (b[2] - b[0], b[3] - b[1]);
            let size = w.max(ht);
            let c = Vec2::new((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
            if !(size >= SHAPE_MIN_H * h && size <= SHAPE_MAX_H * h && w.min(ht) >= SHAPE_ASPECT * size && c.is_finite() && c.dist(mid) <= reach) {
                continue;
            }
            if on_lettering(c) || taken.iter().any(|p| p.dist(c) <= 0.5 * size) {
                continue;
            }
            let paths = joined(members.iter().filter_map(|i| strokes.get(*i)).map(|(p, _)| p.clone()).collect(), join);
            if !paths.iter().any(|p| turns_both_ways(p, 0.02 * size)) {
                continue;
            }
            out.push((c.dist(mid), c));
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().map(|(_, c)| c).collect()
    }
}

/// Whether two strokes meet: a corner of one within `tol` of the other.
fn meet(a: &[Vec2], b: &[Vec2], tol: f64) -> bool {
    let near = |p: &[Vec2], q: &[Vec2]| p.iter().any(|x| q.windows(2).any(|w| matches!(w, [m, n] if seg_dist(*x, *m, *n) <= tol)));
    near(a, b) || near(b, a)
}

/// Strokes joined end to end where their ends meet (within `tol`), each turned to follow on. A
/// closed outline stays as it is.
fn joined(mut paths: Vec<Vec<Vec2>>, tol: f64) -> Vec<Vec<Vec2>> {
    let open = |p: &[Vec2]| p.first().zip(p.last()).is_some_and(|(a, b)| a.dist(*b) > tol);
    for _ in 0..MAX_SHAPE_STROKES {
        // The first two open paths with ends meeting: (which, which, flip the first, flip the second).
        let mut pair: Option<(usize, usize, bool, bool)> = None;
        'find: for (i, a) in paths.iter().enumerate() {
            for (j, b) in paths.iter().enumerate().skip(i + 1) {
                if !open(a) || !open(b) {
                    continue;
                }
                let (Some(&a0), Some(&a1), Some(&b0), Some(&b1)) = (a.first(), a.last(), b.first(), b.last()) else { continue };
                let how = if a1.dist(b0) <= tol {
                    Some((false, false))
                } else if a1.dist(b1) <= tol {
                    Some((false, true))
                } else if a0.dist(b0) <= tol {
                    Some((true, false))
                } else if a0.dist(b1) <= tol {
                    Some((true, true))
                } else {
                    None
                };
                if let Some((fa, fb)) = how {
                    pair = Some((i, j, fa, fb));
                    break 'find;
                }
            }
        }
        let Some((i, j, fa, fb)) = pair else { break };
        let mut b = paths.remove(j);
        if fb {
            b.reverse();
        }
        let Some(a) = paths.get_mut(i) else { break };
        if fa {
            a.reverse();
        }
        a.extend(b.into_iter().skip(1));
    }
    paths
}

/// Whether a stroke, followed along, turns left and right each at least [`SHAPE_TURN`] (corners
/// closer than `eps` taken as one).
fn turns_both_ways(pts: &[Vec2], eps: f64) -> bool {
    let mut p: Vec<Vec2> = Vec::new();
    for q in pts {
        if p.last().is_none_or(|l| l.dist(*q) > eps) {
            p.push(*q);
        }
    }
    let closed = p.len() > 3 && p.first().zip(p.last()).is_some_and(|(a, b)| a.dist(*b) <= eps.max(1e-9));
    if closed {
        p.pop();
    }
    let n = p.len();
    if n < 3 {
        return false;
    }
    let (mut left, mut right) = (0.0f64, 0.0f64);
    let corners = if closed { n } else { n - 2 };
    for i in 0..corners {
        let (Some(a), Some(b), Some(c)) = (p.get(i), p.get((i + 1) % n), p.get((i + 2) % n)) else { continue };
        let (u, v) = (*b - *a, *c - *b);
        let t = u.cross(v).atan2(u.dot(v));
        if t > 0.0 {
            left += t;
        } else {
            right -= t;
        }
    }
    left >= SHAPE_TURN && right >= SHAPE_TURN
}

/// Where the switch a designation names is drawn, and how that was found (Q6337, Q6602): the
/// leader's end from it; else the nearest switch symbol by it, one the import read as a big
/// letter ([`glyph_symbols`]) or one drawn as loose linework ([`Strokes::switch_shapes`], none
/// of `taken` on it); else its own point (said so).
pub(crate) fn locate_switch(
    label: &TextItem,
    texts: &[&TextItem],
    leaders: Option<&Leaders>,
    strokes: Option<&Strokes>,
    taken: &[Vec2],
) -> (Vec2, Spot) {
    let glyphs = glyph_symbols(label, texts);
    let shapes = strokes.map(|s| s.switch_shapes(label, texts, taken)).unwrap_or_default();
    let all: Vec<Vec2> = glyphs.iter().chain(shapes.iter()).copied().collect();
    let (at, spot) = locate(label, &[], leaders, &all, SWITCH_SYMBOL_REACH_H);
    let drawn = shapes.iter().any(|q| q.dist(at) <= 1e-9) && !glyphs.iter().any(|q| q.dist(at) <= 1e-9);
    (at, if spot == Spot::Symbol && drawn { Spot::Linework } else { spot })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: 4.0, rotation: 0.0 }
    }

    #[test]
    fn a_label_on_a_leader_locates_the_far_end_never_itself() {
        // `X-7` printed at (0,0), its leader from the lettering's end down and across to (40,-30).
        let l = text("X-7", 0.0, 0.0);
        let leaders = Leaders::new(vec![(Vec2::new(10.5, 2.0), Vec2::new(30.0, -20.0)), (Vec2::new(30.0, -20.0), Vec2::new(40.0, -30.0))], 8.0);
        let (at, spot) = locate(&l, &[], Some(&leaders), &[], 6.0);
        assert_eq!(spot, Spot::Leader);
        assert!(at.dist(Vec2::new(40.0, -30.0)) < 1e-9, "{at:?}");
        // A symbol drawn by the tip is the one it points at.
        let (at, _) = locate(&l, &[], Some(&leaders), &[Vec2::new(42.0, -31.0), Vec2::new(2.0, 1.0)], 6.0);
        assert!(at.dist(Vec2::new(42.0, -31.0)) < 1e-9, "{at:?}");
    }

    #[test]
    fn a_wall_face_by_a_label_is_no_leader() {
        // A wall's two faces 2 heights apart pass by the label: no leader, the label's point
        // (said so) when no symbol is by it either.
        let l = text("X-7", 0.0, 0.0);
        let leaders = Leaders::new(vec![(Vec2::new(14.0, 2.0), Vec2::new(14.0, 100.0)), (Vec2::new(22.0, 2.0), Vec2::new(22.0, 100.0))], 8.0);
        let (at, spot) = locate(&l, &[], Some(&leaders), &[], 6.0);
        assert_eq!((spot, at), (Spot::Label, l.at));
        assert!(Spot::Label.basis("X-7", "Z-1").contains("location to check"));
        // A single level line from the lettering's end (a cabinet's edge) is no leader either.
        let edge = Leaders::new(vec![(Vec2::new(11.0, 2.0), Vec2::new(60.0, 2.0))], 8.0);
        assert_eq!(locate(&l, &[], Some(&edge), &[], 6.0).1, Spot::Label);
        // Too long to be a leader (a grid line).
        let grid = Leaders::new(vec![(Vec2::new(11.0, 2.0), Vec2::new(400.0, -300.0))], 8.0);
        assert_eq!(locate(&l, &[], Some(&grid), &[], 6.0).1, Spot::Label);
        assert_eq!(Spot::key_of_basis(&Spot::Label.basis("X-7", "Z-1")), "label");
        assert_eq!(Spot::key_of_basis(&Spot::Leader.basis("X-7", "Z-1")), "leader");
    }

    #[test]
    fn with_no_leader_the_nearest_free_symbol_of_its_kind_within_reach() {
        let l = text("D,b", 0.0, 0.0);
        let (at, spot) = locate(&l, &[], None, &[Vec2::new(-12.0, 3.0), Vec2::new(60.0, 0.0)], 6.0);
        assert_eq!(spot, Spot::Symbol);
        assert!(at.dist(Vec2::new(-12.0, 3.0)) < 1e-9);
        // Beyond reach: none.
        let (_, spot) = locate(&l, &[], None, &[Vec2::new(60.0, 0.0)], 6.0);
        assert_eq!(spot, Spot::Label);
    }

    #[test]
    fn a_leader_from_a_keynote_marker_printed_with_the_label_counts() {
        // `X-7` beside a keynote's marker `31`: the leader leaves the marker.
        let l = text("X-7", 20.0, 0.0);
        let marker = text("31", 0.0, 0.0);
        let leaders = Leaders::new(vec![(Vec2::new(-1.0, 2.0), Vec2::new(-30.0, 25.0))], 8.0);
        let (at, spot) = locate(&l, &[&marker], Some(&leaders), &[], 6.0);
        assert_eq!(spot, Spot::Leader);
        assert!(at.dist(Vec2::new(-30.0, 25.0)) < 1e-9, "{at:?}");
    }

    #[test]
    fn a_switch_symbol_read_as_a_big_letter_is_the_symbol_its_designation_names() {
        // The switch's `S` strokes read as a `1` four times the designation's height, beside `D,b`.
        let l = text("D,b", 20.0, 10.0);
        let glyph = TextItem { value: "1".into(), at: Vec2::new(0.0, 0.0), height: 16.0, rotation: 0.0 };
        let small = text("c", 4.0, 30.0);
        let far = TextItem { value: "1".into(), at: Vec2::new(400.0, 0.0), height: 16.0, rotation: 0.0 };
        let syms = glyph_symbols(&l, &[&glyph, &small, &far]);
        assert_eq!(syms.len(), 1, "{syms:?}");
        let (at, spot) = locate(&l, &[], None, &syms, SWITCH_SYMBOL_REACH_H);
        assert_eq!(spot, Spot::Symbol);
        assert!(at.dist(Vec2::new(6.4, 8.0)) < 1e-9, "{at:?}");
    }

    /// An `S` drawn as one stroke `2r` wide and `4r` high about `c`: round its upper bowl the
    /// clock's other way, then round its lower bowl clockwise.
    fn s_stroke(c: Vec2, r: f64) -> Vec<Vec2> {
        let top = c + Vec2::new(0.0, r);
        let bottom = c - Vec2::new(0.0, r);
        let deg = |d: f64| d.to_radians();
        let mut pts: Vec<Vec2> = (0..=10).map(|i| top + Vec2::from_angle(deg(20.0 + 250.0 * f64::from(i) / 10.0)) * r).collect();
        pts.extend((1..=10).map(|i| bottom + Vec2::from_angle(deg(90.0 - 250.0 * f64::from(i) / 10.0)) * r));
        pts
    }

    fn pline(pts: &[Vec2], closed: bool) -> cadaddy_doc::EntityKind {
        cadaddy_doc::EntityKind::LwPolyline(cadaddy_doc::LwPolyline {
            vertices: pts.iter().map(|p| cadaddy_geom::PolyVertex { p: *p, bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect(),
            closed,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        })
    }

    #[test]
    fn a_switch_symbol_drawn_as_loose_linework_is_the_one_its_designation_names() {
        // Q6602: `D,b` 4 high at (20, 10); beside it a switch's `S` drawn as one loose stroke
        // (3.5 heights high) about (8, 12); farther off a box (a junction box's) and a circle.
        let l = text("D,b", 20.0, 10.0);
        let objects = [
            pline(&s_stroke(Vec2::new(8.0, 12.0), 3.5), false),
            pline(&[Vec2::new(40.0, 0.0), Vec2::new(52.0, 0.0), Vec2::new(52.0, 12.0), Vec2::new(40.0, 12.0)], true),
            cadaddy_doc::EntityKind::Circle(cadaddy_doc::Circle { center: cadaddy_geom::Vec3::new(20.0, 30.0, 0.0), radius: 5.0 }),
        ];
        let strokes = Strokes::of(objects.iter(), 4.0);
        let (at, spot) = locate_switch(&l, &[], None, Some(&strokes), &[]);
        assert_eq!(spot, Spot::Linework);
        assert!(at.dist(Vec2::new(8.0, 12.0)) < 1.0, "{at:?}");
        assert_eq!(Spot::key_of_basis(&Spot::Linework.basis("D,b", "Z-1")), "linework");
        // A device standing on it already: it is that one's, the designation stays (said so).
        let (at, spot) = locate_switch(&l, &[], None, Some(&strokes), &[Vec2::new(8.5, 11.0)]);
        assert_eq!((spot, at), (Spot::Label, l.at));
        // Out of reach (another room's switch): none.
        let far = text("D,b", 80.0, 10.0);
        assert_eq!(locate_switch(&far, &[], None, Some(&strokes), &[]).1, Spot::Label);
        // No strokes looked at: as before, its label.
        assert_eq!(locate_switch(&l, &[], None, None, &[]).1, Spot::Label);
    }

    #[test]
    fn an_s_in_two_strokes_arcs_or_a_filled_outline_is_found_a_box_or_a_bowl_is_not() {
        let l = text("3,a", 20.0, 10.0);
        let c = Vec2::new(8.0, 12.0);
        let s = s_stroke(c, 3.5);
        let (upper, lower) = s.split_at(11);
        // Its two bowls drawn apart, the lower one backwards: joined end to end, it curves both ways.
        let mut back: Vec<Vec2> = lower.to_vec();
        back.insert(0, *upper.last().unwrap());
        back.reverse();
        let two = [pline(upper, false), pline(&back, false)];
        let (at, spot) = locate_switch(&l, &[], None, Some(&Strokes::of(two.iter(), 4.0)), &[]);
        assert_eq!(spot, Spot::Linework, "two strokes");
        assert!(at.dist(c) < 1.0, "{at:?}");
        // Drawn by two arcs.
        let arcs = [
            cadaddy_doc::EntityKind::Arc(cadaddy_doc::Arc {
                center: cadaddy_geom::Vec3::new(8.0, 15.5, 0.0),
                radius: 3.5,
                start: 20f64.to_radians(),
                end: 270f64.to_radians(),
            }),
            cadaddy_doc::EntityKind::Arc(cadaddy_doc::Arc {
                center: cadaddy_geom::Vec3::new(8.0, 8.5, 0.0),
                radius: 3.5,
                start: (-160f64).to_radians(),
                end: 90f64.to_radians(),
            }),
        ];
        assert_eq!(locate_switch(&l, &[], None, Some(&Strokes::of(arcs.iter(), 4.0)), &[]).1, Spot::Linework, "arcs");
        // A filled outline (a letter's glyph the import kept as a fill): the stroke and back,
        // a little apart.
        let mut outline: Vec<Vec2> = s.iter().map(|p| *p + Vec2::new(0.3, 0.0)).collect();
        outline.extend(s.iter().rev().map(|p| *p - Vec2::new(0.3, 0.0)));
        let fill = cadaddy_doc::EntityKind::Hatch(Box::new(cadaddy_doc::Hatch {
            pattern: "SOLID".into(),
            solid: true,
            loops: vec![cadaddy_doc::HatchLoop {
                vertices: outline.iter().map(|p| cadaddy_geom::PolyVertex { p: *p, bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect(),
                outer: true,
                sources: Vec::new(),
                curves: Vec::new(),
            }],
            scale: 1.0,
            angle: 0.0,
            associative: false,
            style: 0,
            elevation: 0.0,
            gradient: None,
            origin: Vec2::ZERO,
            background: None,
            pattern_lines: Vec::new(),
        }));
        assert_eq!(locate_switch(&l, &[], None, Some(&Strokes::of([&fill], 4.0)), &[]).1, Spot::Linework, "filled outline");
        // A box, a triangle, a bowl (an arc one way), a receptacle's circle and bars: none.
        let others = [
            pline(&[Vec2::new(2.0, 6.0), Vec2::new(14.0, 6.0), Vec2::new(14.0, 18.0), Vec2::new(2.0, 18.0)], true),
            pline(&[Vec2::new(2.0, -10.0), Vec2::new(14.0, -10.0), Vec2::new(8.0, 0.0)], true),
            pline(upper, false),
            cadaddy_doc::EntityKind::Line(cadaddy_doc::Line {
                a: cadaddy_geom::Vec3::new(30.0, 20.0, 0.0),
                b: cadaddy_geom::Vec3::new(30.0, 30.0, 0.0),
            }),
        ];
        for k in &others {
            assert_eq!(locate_switch(&l, &[], None, Some(&Strokes::of([k], 4.0)), &[]).1, Spot::Label, "{k:?}");
        }
        // Its own lettering drawn by strokes (each letter its height or less), and a word printed
        // over a stroke drawn like an `S` (the word's own letter): lettering, no symbol.
        let small = [pline(&s_stroke(Vec2::new(22.0, 12.0), 0.9), false)];
        assert_eq!(locate_switch(&l, &[], None, Some(&Strokes::of(small.iter(), 4.0)), &[]).1, Spot::Label);
        let word = TextItem { value: "BATH".into(), at: Vec2::new(0.0, 8.0), height: 8.0, rotation: 0.0 };
        let on_word = [pline(&s_stroke(Vec2::new(8.0, 12.0), 2.0), false)];
        assert_eq!(locate_switch(&l, &[&word], None, Some(&Strokes::of(on_word.iter(), 4.0)), &[]).1, Spot::Label);
    }

    #[test]
    fn hostile_input_gives_the_label_point() {
        let mut l = text("X-7", 0.0, 0.0);
        l.height = f64::NAN;
        let leaders = Leaders::new(vec![(Vec2::new(f64::NAN, 0.0), Vec2::new(1.0, 1.0))], f64::NAN);
        assert_eq!(locate(&l, &[], Some(&leaders), &[Vec2::new(f64::INFINITY, 0.0)], 6.0).1, Spot::Label);
        let junk = [pline(&[Vec2::new(f64::NAN, 0.0), Vec2::new(1.0, f64::INFINITY), Vec2::new(2.0, 2.0)], false), pline(&[], true)];
        let strokes = Strokes::of(junk.iter(), f64::NAN);
        assert_eq!(locate_switch(&l, &[], Some(&leaders), Some(&strokes), &[]).1, Spot::Label);
        let ok = text("D,b", 0.0, 0.0);
        assert_eq!(locate_switch(&ok, &[], None, Some(&strokes), &[Vec2::new(f64::NAN, 0.0)]).1, Spot::Label);
    }
}
