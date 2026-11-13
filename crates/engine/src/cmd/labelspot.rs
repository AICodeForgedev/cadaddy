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

use std::collections::{BTreeSet, HashMap};

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

    #[test]
    fn hostile_input_gives_the_label_point() {
        let mut l = text("X-7", 0.0, 0.0);
        l.height = f64::NAN;
        let leaders = Leaders::new(vec![(Vec2::new(f64::NAN, 0.0), Vec2::new(1.0, 1.0))], f64::NAN);
        assert_eq!(locate(&l, &[], Some(&leaders), &[Vec2::new(f64::INFINITY, 0.0)], 6.0).1, Spot::Label);
    }
}
