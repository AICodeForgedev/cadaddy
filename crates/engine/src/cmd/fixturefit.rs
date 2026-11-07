//! Plumbing fixtures onto the fixtures the architectural plans draw (Q6240).
//!
//! A plumbing plan tags a fixture with a balloon set beside it (its tag in a hexagon), clear of
//! the fixture's own linework: a foot or several off, often over the tub next to the water closet
//! or in a door swing. The tag names the fixture; it doesn't place it. The architectural plan the
//! plumbing plan is registered onto draws the fixture itself: a water closet's bowl and tank, a
//! lavatory's basin in its vanity, a tub, a sink's basin in its counter.
//!
//! Each placed plumbing fixture whose schedule row names a fixture of a known kind (its words:
//! `WATER CLOSET`, `LAVATORY`, `TUB`, `SINK`…) goes onto the drawn fixture of that kind nearest
//! its tag, within reach, one drawn fixture per tagged one, nearest pairs first. It keeps where its
//! tag put it (`tag_at_ft`) and says what it sits on and why (`position_provenance` drawn,
//! `position_basis`). One no drawn fixture of its kind is near stays where its tag put it, and
//! says so (`position_provenance` inferred): a gap in the drawings, never hidden.
//!
//! The drawn outlines are the closed (or nearly closed) outlines, circles and ellipses of the
//! architectural plan sheets in the job's drawing, where the placed fixtures are: a sheet that
//! isn't registered there (an unregistered enlarged plan) is elsewhere in the drawing, so its
//! points are never taken. What each kind looks like drawn (its outline's sizes, a water closet's
//! tank beside its bowl, the depth of the counter a basin sits in) and how far a tag may be from
//! its fixture are rules data (`fixturefit.json`), with the drafting practice they come from.
//!
//! A fixture the rules say holds no receptacle (a tub, a shower: NEC 406.9(C) keeps receptacles
//! out of the tub and shower space) is drawn with marks of its own inside it, a drain, an
//! overflow, whose small round shape reads like a device symbol. Where a circuit label printed
//! over the fixture (a luminaire's tag set on the tub, its leader running to the light) labels
//! such a mark, the device reading takes it for a receptacle: [`receptacles_inside`] finds the
//! receptacles whose symbol lies inside a drawn fixture of such a family, so the build leaves
//! them out (a circuit label inside a fixture's outline is not a receptacle symbol).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use serde::Deserialize;

use super::readers::ReadDevice;

const RULES: &str = include_str!("fixturefit.json");

/// Outlines looked at, at most.
const MAX_OUTLINES: usize = 500_000;
/// Vertices of one polyline looked at, at most (a longer one is no fixture).
const MAX_VERTICES: usize = 2_000;
/// Fixtures fitted, at most.
const MAX_FIXTURES: usize = 50_000;
/// Fixture-to-drawn-fixture pairs weighed, at most.
const MAX_PAIRS: usize = 2_000_000;
/// Outlines one grid query returns, at most.
const MAX_NEAR: usize = 20_000;
/// A polyline of this many vertices or more is drawn with curves (its arcs flattened).
const ROUNDED_VERTICES: usize = 8;
/// A drain is round: its short side at least this share of its long one.
const DRAIN_ROUND: f64 = 0.75;

/// How fixtures are fitted: distances and the drawn kinds (`fixturefit.json`).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    /// How far from its tag a fixture may be drawn, feet.
    pub reach_ft: f64,
    /// Outlines this close are one fixture's pieces, inches.
    pub join_in: f64,
    /// An outline open by this much or less is still closed, inches.
    pub outline_gap_in: f64,
    /// Nothing larger is one fixture's outline, inches.
    pub largest_in: f64,
    /// A drain or faucet hole's width, inches: `[min, max]`.
    pub drain_in: [f64; 2],
    /// A counter's depth, inches: `[min, max]`.
    pub counter_in: [f64; 2],
    /// A symbol this far inside a fixture's outline (inches) is inside it: drawn in the fixture,
    /// not on the wall the fixture is set against.
    pub inside_in: f64,
    pub families: Vec<Family>,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            reach_ft: 8.0,
            join_in: 3.0,
            outline_gap_in: 3.0,
            largest_in: 96.0,
            drain_in: [0.5, 4.0],
            counter_in: [12.0, 36.0],
            inside_in: 2.0,
            families: Vec::new(),
        }
    }
}

/// A kind of fixture: the words of its schedule row's fixture that name it, and how it is drawn.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Family {
    pub fixture: String,
    pub words: Vec<String>,
    pub drawn: Vec<Drawn>,
    /// No receptacle is ever inside it (a tub, a shower: NEC 406.9(C)); a device symbol read
    /// inside its drawn outline is one of its own marks.
    pub holds_no_receptacle: bool,
    /// The code or practice that says so, for the basis.
    pub holds_no_receptacle_rule: String,
}

/// One way a kind of fixture is drawn. Sizes are `[short min, short max, long min, long max]`,
/// inches.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Drawn {
    /// The outline (a closed one) of this size.
    pub part: Option<[f64; 4]>,
    /// With a second outline of this size against it (a water closet's tank).
    pub beside: Option<[f64; 4]>,
    /// A basin: nothing larger than a drain drawn in it (not a cooktop with its burners), and
    /// rounded or holding its drain (not a plain box: an appliance, a shelf); it may be open
    /// along one of its own sides (a sink drawn against its backsplash).
    pub basin: bool,
    /// Drawn with curves (a one-piece water closet's bowl), not a plain box.
    pub rounded: bool,
    /// A basin's counter, when one is drawn round it, is this deep (inches, `[min, max]`).
    pub counter_depth_in: Option<[f64; 2]>,
    /// The basin is always drawn in a counter (a kitchen sink): none round it, no basin.
    pub counter_required: bool,
    /// A weaker likeness (a water closet's bowl with no tank drawn, a basin in no counter): taken
    /// only by the fixtures no stronger one is near.
    pub weak: bool,
    /// Or: a counter of this size with a drain drawn in it; the fixture is at the drain.
    pub counter: Option<[f64; 4]>,
    /// What the fixture is put on, said in its basis.
    pub what: String,
}

/// The rules data.
pub(crate) fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// The family a schedule's fixture is (`ADA WATER CLOSET` → water closet): the first whose words
/// are words of it.
pub(crate) fn family_of<'a>(rules: &'a Rules, fixture: &str) -> Option<(usize, &'a Family)> {
    let tokens: Vec<String> =
        fixture.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_string).collect();
    rules.families.iter().enumerate().find(|(_, f)| {
        f.words.iter().any(|w| {
            let want: Vec<String> =
                w.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_string).collect();
            !want.is_empty() && tokens.windows(want.len()).any(|win| win == want.as_slice())
        })
    })
}

/// A drawn outline: its box, its sides (short, long; along its own edges when it is turned) and
/// the sheet entity it is.
#[derive(Clone, Debug)]
struct Outline {
    handle: u64,
    lo: Vec2,
    hi: Vec2,
    centre: Vec2,
    short: f64,
    long: f64,
    /// Closed, or open by no more than the rules' gap.
    closed: bool,
    /// Open along one of its own sides (both ends on one edge of its box).
    open_side: bool,
    /// Drawn with curves: a circle or ellipse, an arc in it, or many short edges.
    rounded: bool,
    circle: bool,
}

impl Outline {
    fn contains(&self, o: &Outline, eps: f64) -> bool {
        self.lo.x <= o.lo.x + eps && self.lo.y <= o.lo.y + eps && self.hi.x >= o.hi.x - eps && self.hi.y >= o.hi.y - eps && self.area() > o.area()
    }
    fn area(&self) -> f64 {
        (self.hi.x - self.lo.x) * (self.hi.y - self.lo.y)
    }
    fn gap(&self, o: &Outline) -> f64 {
        let dx = (o.lo.x - self.hi.x).max(self.lo.x - o.hi.x).max(0.0);
        let dy = (o.lo.y - self.hi.y).max(self.lo.y - o.hi.y).max(0.0);
        dx.hypot(dy)
    }
    fn fits(&self, size: &[f64; 4], inch: f64) -> bool {
        let (s, l) = (self.short / inch, self.long / inch);
        s >= size[0] && s <= size[1] && l >= size[2] && l <= size[3]
    }
}

/// The outline of a polyline's points: its box, and its sides along its longest edge when that
/// box is the tighter one (a fixture drawn turned).
fn polyline_outline(handle: u64, pts: &[Vec2], closed: bool) -> Option<Outline> {
    let first = *pts.first()?;
    let (mut lo, mut hi) = (first, first);
    for p in pts {
        lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    if !(lo.x.is_finite() && lo.y.is_finite() && hi.x.is_finite() && hi.y.is_finite()) {
        return None;
    }
    let (w, h) = (hi.x - lo.x, hi.y - lo.y);
    let on_edge = |p: Vec2, q: Vec2, tol: f64| {
        ((p.x - lo.x).abs() <= tol && (q.x - lo.x).abs() <= tol)
            || ((p.x - hi.x).abs() <= tol && (q.x - hi.x).abs() <= tol)
            || ((p.y - lo.y).abs() <= tol && (q.y - lo.y).abs() <= tol)
            || ((p.y - hi.y).abs() <= tol && (q.y - hi.y).abs() <= tol)
    };
    let last = pts.last().copied().unwrap_or(first);
    let open_side = !closed && pts.len() >= 4 && on_edge(first, last, 0.02 * w.max(h));
    let mut out = Outline {
        handle,
        lo,
        hi,
        centre: (lo + hi) * 0.5,
        short: w.min(h),
        long: w.max(h),
        closed,
        open_side,
        rounded: pts.len() >= ROUNDED_VERTICES,
        circle: false,
    };
    // Along the longest edge.
    let longest = pts.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))).max_by(|a, b| a.0.dist(a.1).total_cmp(&b.0.dist(b.1)));
    if let Some((a, b)) = longest {
        let d = b - a;
        let len = d.x.hypot(d.y);
        if len > 1e-9 {
            let (c, s) = (d.x / len, d.y / len);
            let turned = |p: Vec2| Vec2::new(p.x * c + p.y * s, -p.x * s + p.y * c);
            let back = |p: Vec2| Vec2::new(p.x * c - p.y * s, p.x * s + p.y * c);
            let t0 = turned(first);
            let (mut tlo, mut thi) = (t0, t0);
            for p in pts {
                let t = turned(*p);
                tlo = Vec2::new(tlo.x.min(t.x), tlo.y.min(t.y));
                thi = Vec2::new(thi.x.max(t.x), thi.y.max(t.y));
            }
            let (tw, th) = (thi.x - tlo.x, thi.y - tlo.y);
            if tw * th < w * h * 0.95 {
                out.short = tw.min(th);
                out.long = tw.max(th);
                out.centre = back((tlo + thi) * 0.5);
            }
        }
    }
    Some(out)
}

/// The outlines an entity draws (none: it draws no outline).
fn outline_of(handle: u64, kind: &EntityKind, gap: f64) -> Option<Outline> {
    match kind {
        EntityKind::LwPolyline(pl) if pl.vertices.len() >= 3 && pl.vertices.len() <= MAX_VERTICES => {
            let pts: Vec<Vec2> = pl.vertices.iter().map(|v| v.p).collect();
            let closed = pl.closed || matches!((pts.first(), pts.last()), (Some(a), Some(b)) if a.dist(*b) <= gap);
            let arcs = pl.vertices.iter().any(|v| v.bulge.is_finite() && v.bulge.abs() > 1e-6);
            polyline_outline(handle, &pts, closed).map(|o| Outline { rounded: o.rounded || arcs, ..o })
        }
        EntityKind::Circle(c) if c.radius.is_finite() && c.radius > 0.0 && c.center.x.is_finite() && c.center.y.is_finite() => {
            let ctr = Vec2::new(c.center.x, c.center.y);
            let r = Vec2::new(c.radius, c.radius);
            Some(Outline {
                handle,
                lo: ctr - r,
                hi: ctr + r,
                centre: ctr,
                short: 2.0 * c.radius,
                long: 2.0 * c.radius,
                closed: true,
                open_side: false,
                rounded: true,
                circle: true,
            })
        }
        EntityKind::Ellipse(e) => {
            let major = e.major.x.hypot(e.major.y);
            let full = (e.end - e.start).abs() >= std::f64::consts::TAU - 1e-3 || (e.end - e.start).abs() < 1e-9;
            if !(major.is_finite() && major > 0.0 && e.ratio.is_finite() && e.ratio > 0.0 && full && e.center.x.is_finite() && e.center.y.is_finite())
            {
                return None;
            }
            let ctr = Vec2::new(e.center.x, e.center.y);
            let r = Vec2::new(major, major);
            let minor = major * e.ratio.min(1.0);
            Some(Outline {
                handle,
                lo: ctr - r,
                hi: ctr + r,
                centre: ctr,
                short: 2.0 * minor,
                long: 2.0 * major,
                closed: true,
                open_side: false,
                rounded: true,
                circle: false,
            })
        }
        _ => None,
    }
}

/// The outlines in cells, for finding the ones near a point.
struct Grid {
    cell: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
}

impl Grid {
    fn key(&self, v: f64) -> i64 {
        let k = (v / self.cell).floor();
        if k.is_finite() { k.clamp(-1e12, 1e12) as i64 } else { 0 }
    }
    fn build(outlines: &[Outline], cell: f64) -> Grid {
        let mut g = Grid { cell, cells: HashMap::new() };
        for (i, o) in outlines.iter().enumerate() {
            let (x0, x1, y0, y1) = (g.key(o.lo.x), g.key(o.hi.x), g.key(o.lo.y), g.key(o.hi.y));
            // An outline is at most the rules' largest: a few cells.
            if x1.saturating_sub(x0) > 8 || y1.saturating_sub(y0) > 8 {
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    g.cells.entry((x, y)).or_default().push(i);
                }
            }
        }
        g
    }
    /// The outlines within `r` of the box `lo..hi`.
    fn near(&self, lo: Vec2, hi: Vec2, r: f64) -> Vec<usize> {
        let (x0, x1, y0, y1) = (self.key(lo.x - r), self.key(hi.x + r), self.key(lo.y - r), self.key(hi.y + r));
        let mut out: BTreeSet<usize> = BTreeSet::new();
        if x1.saturating_sub(x0) > 64 || y1.saturating_sub(y0) > 64 {
            return Vec::new();
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.cells.get(&(x, y)) {
                    out.extend(v.iter().copied());
                }
                if out.len() >= MAX_NEAR {
                    return out.into_iter().collect();
                }
            }
        }
        out.into_iter().collect()
    }
}

/// A drawn fixture a tagged one may go on: its family, where it is, the outlines it is made of
/// (each taken once) and what it is, said.
#[derive(Clone, Debug)]
struct Symbol {
    family: usize,
    /// A strong likeness (matched first) or a weak one.
    strong: bool,
    at: Vec2,
    parts: Vec<usize>,
    what: String,
}

/// One fixture fitted (or not): its symbol's handle and where it was and is, model units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fit {
    pub handle: String,
    pub tag_at: [f64; 2],
    pub at: [f64; 2],
    /// `drawn` (on a drawn fixture) or `inferred` (at its tag: none drawn near).
    pub provenance: &'static str,
    pub basis: String,
    /// The drawn fixture's box, `[x0, y0, x1, y1]` model units (a drawn fit only).
    pub outline: Option<[f64; 4]>,
    /// Its level (as the device reading gave it) and its family's name (`tub`).
    pub level: String,
    pub family: String,
}

/// What [`fit_fixtures`] did: by family, how many went onto a drawn fixture and how many stayed at
/// their tags; how far they moved (feet, the largest and the middle one); every fixture's fit.
#[derive(Clone, Debug, Default)]
pub(crate) struct FitReport {
    pub moved: BTreeMap<String, usize>,
    pub at_tag: BTreeMap<String, usize>,
    pub largest_move_ft: f64,
    pub median_move_ft: f64,
    pub fits: Vec<Fit>,
}

/// The architectural plan sheets' outlines in the job's drawing: entities of the sheets of
/// discipline A whose region (when the job has one) is a plan.
fn plan_outlines(d: &Drawing, spans: &[(u64, u64, String)], gap: f64, largest: f64) -> (Vec<Outline>, Vec<String>) {
    let mut out = Vec::new();
    let mut sheets = Vec::new();
    for e in d.model.iter() {
        let Some((_, _, sheet)) = spans.iter().find(|(lo, hi, _)| (*lo..*hi).contains(&e.handle.0)) else { continue };
        let Some(o) = outline_of(e.handle.0, &e.kind, gap) else { continue };
        if o.long > largest || o.long <= 0.0 {
            continue;
        }
        out.push(o);
        sheets.push(sheet.clone());
        if out.len() >= MAX_OUTLINES {
            break;
        }
    }
    (out, sheets)
}

/// The drawn fixtures of the families wanted near `points` (model units), from `outlines`.
fn symbols_near(rules: &Rules, outlines: &[Outline], grid: &Grid, wanted: &BTreeSet<usize>, points: &[Vec2], inch: f64) -> Vec<Symbol> {
    let reach = rules.reach_ft * 12.0 * inch;
    let mut near: BTreeSet<usize> = BTreeSet::new();
    for p in points {
        near.extend(grid.near(*p, *p, reach));
    }
    let eps = 0.25 * inch;
    // A drain (or a faucet hole): a small round outline, a circle or a short closed polyline.
    let drain =
        |o: &Outline| o.closed && o.short >= rules.drain_in[0] * inch && o.long <= rules.drain_in[1] * inch && o.short >= DRAIN_ROUND * o.long;
    let inside = |o: &Outline| -> Vec<usize> {
        grid.near(o.lo, o.hi, 0.0).into_iter().filter(|&j| outlines.get(j).is_some_and(|x| x.handle != o.handle && o.contains(x, eps))).collect()
    };
    // The counter round an outline: the smallest outline (closed or not: a counter is often drawn
    // against its wall on three sides) holding it, of a counter's depth.
    let counter_of = |o: &Outline| -> Option<f64> {
        grid.near(o.lo, o.hi, 0.0)
            .into_iter()
            .filter_map(|j| outlines.get(j))
            .filter(|c| {
                c.handle != o.handle
                    && !c.circle
                    && c.contains(o, eps)
                    && c.short >= rules.counter_in[0] * inch
                    && c.short <= rules.counter_in[1] * inch
            })
            .min_by(|a, b| a.area().total_cmp(&b.area()))
            .map(|c| c.short / inch)
    };
    let mut out: Vec<Symbol> = Vec::new();
    // First the fixtures drawn in two pieces (a bowl and its tank): their pieces are no other
    // fixture's (a water closet's bowl is no lavatory's basin).
    let mut paired: BTreeSet<usize> = BTreeSet::new();
    for (fi, fam) in rules.families.iter().enumerate().filter(|(i, _)| wanted.contains(i)) {
        for alt in &fam.drawn {
            let (Some(part), Some(beside)) = (alt.part, alt.beside) else { continue };
            for &i in &near {
                let Some(o) = outlines.get(i) else { continue };
                if !o.closed || o.circle || !o.fits(&part, inch) {
                    continue;
                }
                let join = rules.join_in * inch;
                let mate = grid
                    .near(o.lo, o.hi, join)
                    .into_iter()
                    .filter(|&j| j != i)
                    .filter_map(|j| Some((j, outlines.get(j)?)))
                    .filter(|(_, b)| b.closed && !b.circle && b.fits(&beside, inch) && o.gap(b) <= join && !o.contains(b, eps) && !b.contains(o, eps))
                    .min_by(|a, b| o.gap(a.1).total_cmp(&o.gap(b.1)));
                let Some((j, b)) = mate else { continue };
                let lo = Vec2::new(o.lo.x.min(b.lo.x), o.lo.y.min(b.lo.y));
                let hi = Vec2::new(o.hi.x.max(b.hi.x), o.hi.y.max(b.hi.y));
                paired.insert(i);
                paired.insert(j);
                out.push(Symbol { family: fi, strong: !alt.weak, at: (lo + hi) * 0.5, parts: vec![i, j], what: alt.what.clone() });
            }
        }
    }
    for (fi, fam) in rules.families.iter().enumerate().filter(|(i, _)| wanted.contains(i)) {
        for alt in fam.drawn.iter().filter(|a| a.beside.is_none()) {
            for &i in &near {
                let Some(o) = outlines.get(i) else { continue };
                if paired.contains(&i) {
                    continue;
                }
                if let Some(part) = alt.part {
                    let shut = o.closed || (alt.basin && o.open_side);
                    if !shut || (o.circle && !alt.basin) || (alt.rounded && !o.rounded) || !o.fits(&part, inch) {
                        continue;
                    }
                    // The outer line is the fixture (a tub's inner line, a basin's rim, are its).
                    let outer = grid
                        .near(o.lo, o.hi, 0.0)
                        .into_iter()
                        .filter_map(|j| outlines.get(j))
                        .any(|x| x.closed && x.contains(o, eps) && x.fits(&part, inch));
                    if outer {
                        continue;
                    }
                    let mut parts = vec![i];
                    let mut strong = !alt.weak;
                    if alt.basin {
                        let within = inside(o);
                        if within.iter().filter_map(|&j| outlines.get(j)).any(|x| x.closed && x.short > rules.drain_in[1] * inch) {
                            continue;
                        }
                        match (counter_of(o), alt.counter_depth_in) {
                            (Some(depth), Some(want)) if !(depth >= want[0] && depth <= want[1]) => continue,
                            (None, _) if alt.counter_required => continue,
                            // In no counter (wall-hung, freestanding): a weaker likeness.
                            (None, _) => strong = false,
                            _ => {}
                        }
                        let drains: Vec<usize> = within.into_iter().filter(|&j| outlines.get(j).is_some_and(drain)).collect();
                        if !o.rounded && drains.is_empty() {
                            continue;
                        }
                        parts.extend(drains);
                    }
                    out.push(Symbol { family: fi, strong, at: o.centre, parts, what: alt.what.clone() });
                } else if let Some(counter) = alt.counter {
                    if o.circle || !o.fits(&counter, inch) {
                        continue;
                    }
                    for j in inside(o) {
                        if outlines.get(j).is_some_and(|x| drain(x) && x.short >= DRAIN_ROUND * x.long) && !paired.contains(&j) {
                            let at = outlines.get(j).map_or(o.centre, |x| x.centre);
                            out.push(Symbol { family: fi, strong: !alt.weak, at, parts: vec![j], what: alt.what.clone() });
                        }
                    }
                }
            }
        }
    }
    // One symbol per family and set of pieces.
    let mut seen: BTreeSet<(usize, Vec<usize>)> = BTreeSet::new();
    out.retain(|s| {
        let mut k = s.parts.clone();
        k.sort_unstable();
        seen.insert((s.family, k))
    });
    out
}

/// Put each plumbing fixture of `devices` (kind `plumbing fixture`, model units, on its level's
/// architectural plan) whose schedule row (`fixtures`: tag → fixture) is of a drawn family onto the
/// drawn fixture of that family nearest it within reach, one drawn fixture per fixture, nearest
/// pairs first. `spans`: the job's sheets' handle ranges, `(lo, hi, sheet)`, of the architectural
/// plan sheets. `upf`: drawing units per foot.
pub(crate) fn fit_fixtures(
    d: &Drawing,
    spans: &[(u64, u64, String)],
    fixtures: &BTreeMap<String, String>,
    devices: &mut [ReadDevice],
    upf: f64,
    rules: &Rules,
) -> FitReport {
    let mut report = FitReport::default();
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let inch = upf / 12.0;
    // The fixtures to fit and their families.
    let todo: Vec<(usize, usize)> = devices
        .iter()
        .enumerate()
        .filter(|(_, dv)| dv.kind == "plumbing fixture" && dv.at.iter().all(|v| v.is_finite()))
        .filter_map(|(i, dv)| Some((i, family_of(rules, fixtures.get(dv.tag.trim())?)?.0)))
        .take(MAX_FIXTURES)
        .collect();
    if todo.is_empty() {
        return report;
    }
    let (outlines, sheets) = plan_outlines(d, spans, rules.outline_gap_in * inch, rules.largest_in * inch);
    let grid = Grid::build(&outlines, 48.0 * inch);
    let wanted: BTreeSet<usize> = todo.iter().map(|(_, f)| *f).collect();
    let points: Vec<Vec2> = todo.iter().filter_map(|(i, _)| devices.get(*i)).map(|dv| Vec2::new(dv.at[0], dv.at[1])).collect();
    let symbols = symbols_near(rules, &outlines, &grid, &wanted, &points, inch);
    // The pairs within reach, nearest first.
    let reach = rules.reach_ft * upf;
    let mut by_family: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (si, s) in symbols.iter().enumerate() {
        by_family.entry(s.family).or_default().push(si);
    }
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    'all: for (k, (i, fam)) in todo.iter().enumerate() {
        let Some(dv) = devices.get(*i) else { continue };
        let p = Vec2::new(dv.at[0], dv.at[1]);
        for &si in by_family.get(fam).map(Vec::as_slice).unwrap_or(&[]) {
            let Some(s) = symbols.get(si) else { continue };
            let dist = p.dist(s.at);
            if dist <= reach {
                pairs.push((dist, k, si));
                if pairs.len() >= MAX_PAIRS {
                    break 'all;
                }
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    let mut done: BTreeMap<usize, usize> = BTreeMap::new();
    // The strong likenesses first; then, for the fixtures none is near, the weak ones.
    for pass_strong in [true, false] {
        for &(_, k, si) in &pairs {
            if done.contains_key(&k) {
                continue;
            }
            let Some(s) = symbols.get(si) else { continue };
            if (pass_strong && !s.strong) || s.parts.iter().any(|p| taken.contains(p)) {
                continue;
            }
            taken.extend(s.parts.iter().copied());
            done.insert(k, si);
        }
    }
    let mut moves: Vec<f64> = Vec::new();
    for (k, (i, fam)) in todo.iter().enumerate() {
        let name = rules.families.get(*fam).map_or_else(String::new, |f| f.fixture.clone());
        let Some(dv) = devices.get_mut(*i) else { continue };
        let tag_at = dv.at;
        match done.get(&k).and_then(|si| symbols.get(*si)) {
            Some(s) => {
                let moved = Vec2::new(tag_at[0], tag_at[1]).dist(s.at) / upf;
                dv.at = [s.at.x, s.at.y];
                let pieces: Vec<String> = s.parts.iter().filter_map(|p| outlines.get(*p)).map(|o| cadaddy_doc::Handle(o.handle).hex()).collect();
                let sheet = s.parts.first().and_then(|p| sheets.get(*p)).map_or("the architectural plan", String::as_str);
                let outline = s.parts.iter().filter_map(|p| outlines.get(*p)).fold(None::<[f64; 4]>, |acc, o| {
                    Some(acc.map_or([o.lo.x, o.lo.y, o.hi.x, o.hi.y], |b| [b[0].min(o.lo.x), b[1].min(o.lo.y), b[2].max(o.hi.x), b[3].max(o.hi.y)]))
                });
                report.fits.push(Fit {
                    handle: dv.handle.clone(),
                    tag_at,
                    at: dv.at,
                    outline,
                    level: dv.level.clone(),
                    family: name.clone(),
                    provenance: "drawn",
                    basis: format!(
                        "on the {name} {sheet} draws ({}; outline {}), {moved:.1} ft from its {} tag: a tag is set beside its fixture",
                        s.what,
                        pieces.join("+"),
                        dv.tag.trim()
                    ),
                });
                *report.moved.entry(name).or_default() += 1;
                moves.push(moved);
            }
            None => {
                report.fits.push(Fit {
                    handle: dv.handle.clone(),
                    tag_at,
                    at: tag_at,
                    outline: None,
                    level: dv.level.clone(),
                    family: name.clone(),
                    provenance: "inferred",
                    basis: format!(
                        "at its {} tag: the architectural plan draws no {name} within {} ft of it that a nearer tag didn't take",
                        dv.tag.trim(),
                        rules.reach_ft
                    ),
                });
                *report.at_tag.entry(name).or_default() += 1;
            }
        }
    }
    moves.sort_by(f64::total_cmp);
    report.largest_move_ft = moves.last().copied().unwrap_or(0.0);
    report.median_move_ft = moves.get(moves.len() / 2).copied().unwrap_or(0.0);
    report
}

/// A receptacle read inside a drawn fixture that holds none: which device, and why it is none.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Inside {
    /// Its index in the devices.
    pub index: usize,
    pub reason: String,
}

/// The receptacles of `devices` (model units) whose symbol lies inside the drawn outline of a
/// fitted fixture whose family holds no receptacle (`holds_no_receptacle`: a tub, a shower), at
/// least the rules' `inside_in` in from its every side, on the same level. Such a symbol is one of
/// the fixture's own marks (its drain) that a circuit label printed over the fixture labelled: no
/// receptacle symbol is drawn inside a tub. A symbol on the wall the fixture is set against, or
/// near the fixture but outside it, is not taken. `upf`: drawing units per foot.
pub(crate) fn receptacles_inside(fits: &[Fit], devices: &[ReadDevice], upf: f64, rules: &Rules) -> Vec<Inside> {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let inset = if rules.inside_in.is_finite() { rules.inside_in.max(0.0) } else { 2.0 } * upf / 12.0;
    let holds_none = |family: &str| rules.families.iter().find(|f| f.fixture == family).filter(|f| f.holds_no_receptacle);
    let boxes: Vec<(&Fit, [f64; 4], &Family)> = fits
        .iter()
        .take(MAX_FIXTURES)
        .filter_map(|f| {
            let b = f.outline.filter(|b| b.iter().all(|v| v.is_finite()))?;
            Some((f, b, holds_none(&f.family)?))
        })
        .collect();
    if boxes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (index, dv) in devices.iter().enumerate() {
        if dv.kind != "receptacle" || !dv.at.iter().all(|v| v.is_finite()) {
            continue;
        }
        let [x, y] = dv.at;
        let hit = boxes.iter().find(|(f, b, _)| {
            (f.level.is_empty() || dv.level.is_empty() || f.level == dv.level)
                && b[2] - b[0] > 2.0 * inset
                && b[3] - b[1] > 2.0 * inset
                && x >= b[0] + inset
                && x <= b[2] - inset
                && y >= b[1] + inset
                && y <= b[3] - inset
        });
        if let Some((f, b, fam)) = hit {
            let rule = if fam.holds_no_receptacle_rule.is_empty() { String::new() } else { format!(" ({})", fam.holds_no_receptacle_rule) };
            out.push(Inside {
                index,
                reason: format!(
                    "inside the {} drawn there ({:.1} by {:.1} ft): a {} holds no receptacle{rule}, so a mark inside its outline that the circuit label {} names is one of its own marks, not a receptacle symbol",
                    f.family,
                    (b[2] - b[0]) / upf,
                    (b[3] - b[1]) / upf,
                    f.family,
                    if dv.circuit.trim().is_empty() { "printed by it" } else { dv.circuit.trim() },
                ),
            });
        }
    }
    out
}

/// Each fit onto the model's plumbing fixture it is (its symbol, where it is): where its tag put
/// it (`tag_at_ft`), what it sits on (`position_provenance`: `drawn`, or `inferred` at its tag)
/// and why (`position_basis`). The fixtures written.
pub(crate) fn record(sp: &mut cadaddy_walk3d::BuildingSpec, fits: &[Fit]) -> usize {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut by: BTreeMap<&str, Vec<&Fit>> = BTreeMap::new();
    for f in fits.iter().take(MAX_FIXTURES) {
        by.entry(f.handle.as_str()).or_default().push(f);
    }
    let mut n = 0usize;
    for t in sp.tags.iter_mut().filter(|t| t.id.starts_with("device:") && t.kind == "plumbing_fixture") {
        let Some(symbol) = t.fields.get("symbol").map(|f| f.value.clone()) else { continue };
        let at = [t.at_ft[0] * upf, t.at_ft[1] * upf];
        // The fit at this place (a fixture copied into the rooms of its type shares its symbol).
        let Some(f) = by.get(symbol.as_str()).and_then(|v| v.iter().find(|f| (f.at[0] - at[0]).hypot(f.at[1] - at[1]) <= 0.01 * upf)) else {
            continue;
        };
        t.set("tag_at_ft", format!("{:.2}, {:.2}", f.tag_at[0] / upf, f.tag_at[1] / upf), "plan");
        t.set("position_provenance", f.provenance, if f.provenance == "drawn" { "plan" } else { "rule" });
        t.set("position_basis", f.basis.clone(), if f.provenance == "drawn" { "plan" } else { "rule" });
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Circle, Common, LwPolyline, Space};
    use cadaddy_geom::{PolyVertex, Vec3};

    fn add(d: &mut Drawing, kind: EntityKind) -> u64 {
        d.add(&Space::Model, Common { layer: "PDF-GEOMETRY".into(), ..Common::default() }, kind).map(|h| h.0).unwrap_or(0)
    }

    fn rect(d: &mut Drawing, x0: f64, y0: f64, x1: f64, y1: f64, closed: bool) -> u64 {
        let mut v: Vec<PolyVertex> = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].iter().map(|p| PolyVertex::new(Vec2::new(p[0], p[1]))).collect();
        if !closed {
            // Open at a faucet: the last side stops 1" short.
            v.push(PolyVertex::new(Vec2::new(x0, y0 + 1.0)));
        }
        add(d, EntityKind::LwPolyline(LwPolyline { vertices: v, closed, const_width: 0.0, elevation: 0.0, plinegen: false }))
    }

    /// A sink as drawn against its backsplash: rounded corners, open along its back (top) side.
    fn sink_u(d: &mut Drawing, x0: f64, y0: f64, x1: f64, y1: f64) -> u64 {
        let r = 1.5;
        let pts = [[x1, y1], [x1, y0 + r], [x1 - 0.4, y0 + 0.4], [x1 - r, y0], [x0 + r, y0], [x0 + 0.4, y0 + 0.4], [x0, y0 + r], [x0, y1]];
        let v: Vec<PolyVertex> = pts.iter().map(|p| PolyVertex::new(Vec2::new(p[0], p[1]))).collect();
        add(d, EntityKind::LwPolyline(LwPolyline { vertices: v, closed: false, const_width: 0.0, elevation: 0.0, plinegen: false }))
    }

    fn circle(d: &mut Drawing, x: f64, y: f64, r: f64) -> u64 {
        add(d, EntityKind::Circle(Circle { center: Vec3::new(x, y, 0.0), radius: r }))
    }

    fn dev(tag: &str, x: f64, y: f64) -> ReadDevice {
        ReadDevice { tag: tag.into(), at: [x, y], kind: "plumbing fixture".into(), handle: format!("{tag}@{x}"), ..Default::default() }
    }

    fn schedule() -> BTreeMap<String, String> {
        [("T-1", "ADA WATER CLOSET"), ("T-2", "LAVATORY"), ("T-3", "TUB SHOWER"), ("T-4", "KITCHEN SINK"), ("T-9", "FLOOR DRAIN")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    /// A bath and a kitchenette, drawn in inches: a tub across the top, a water closet (bowl and
    /// tank) and a vanity with its basin along the bottom; a counter with a cooktop and a sink.
    fn bath(d: &mut Drawing, ox: f64, oy: f64) {
        rect(d, ox + 5.0, oy + 66.0, ox + 65.0, oy + 98.0, true); // tub 60 x 32
        rect(d, ox + 9.0, oy + 69.0, ox + 61.0, oy + 95.0, true); // its inner line
        rect(d, ox + 12.0, oy, ox + 32.0, oy + 9.5, true); // tank 20 x 9.5
        rect(d, ox + 14.5, oy + 11.0, ox + 29.5, oy + 29.0, true); // bowl 15 x 18 (1.5" off the tank)
        rect(d, ox + 41.0, oy, ox + 89.0, oy + 20.0, true); // vanity 48 x 20
        // A second vanity beside it, its basin in loose lines, its drain a small square on its corner.
        let diamond: Vec<PolyVertex> = [[0.0, -0.5], [0.5, 0.0], [0.0, 0.5], [-0.5, 0.0], [0.0, -0.5]]
            .iter()
            .map(|p| PolyVertex::new(Vec2::new(ox + 120.0 + p[0], oy + 10.0 + p[1])))
            .collect();
        rect(d, ox + 96.0, oy, ox + 144.0, oy + 21.0, true);
        add(d, EntityKind::LwPolyline(LwPolyline { vertices: diamond, closed: false, const_width: 0.0, elevation: 0.0, plinegen: false }));
        rect(d, ox + 56.0, oy + 5.0, ox + 74.0, oy + 17.0, false); // its basin 18 x 12, open at the faucet
        circle(d, ox + 65.0, oy + 11.0, 0.75); // its drain
        // The kitchenette below: a counter 58 x 24 drawn against its wall (open), a cooktop with
        // two burners, and the sink.
        rect(d, ox + 35.0, oy - 48.0, ox + 93.0, oy - 24.0, false);
        rect(d, ox + 37.0, oy - 46.0, ox + 49.0, oy - 25.0, true); // cooktop 12 x 21
        circle(d, ox + 43.0, oy - 40.0, 3.45);
        circle(d, ox + 43.0, oy - 32.0, 3.45);
        sink_u(d, ox + 77.0, oy - 45.0, ox + 91.0, oy - 27.0); // sink 14 x 18
        // An under-counter refrigerator beside the counter: a plain box of a sink's size.
        rect(d, ox + 8.0, oy - 48.0, ox + 31.6, oy - 21.2, true);
    }

    fn spans_all(d: &Drawing) -> Vec<(u64, u64, String)> {
        vec![(0, d.model.iter().map(|e| e.handle.0).max().unwrap_or(0) + 1, "A-1".to_string())]
    }

    #[test]
    fn the_rules_data_reads() {
        let r: Rules = serde_json::from_str(RULES).expect("fixturefit.json");
        assert!(r.reach_ft > 0.0 && r.families.len() >= 5);
        assert!(r.families.iter().all(|f| !f.words.is_empty() && !f.drawn.is_empty()));
    }

    #[test]
    fn a_fixture_is_named_by_its_schedule_rows_words_tub_before_shower() {
        let r = rules();
        let name = |s: &str| family_of(r, s).map(|(_, f)| f.fixture.clone());
        assert_eq!(name("ADA WATER CLOSET").as_deref(), Some("water closet"));
        assert_eq!(name("TUB SHOWER").as_deref(), Some("tub"));
        assert_eq!(name("ADA ROLL IN SHOWER").as_deref(), Some("shower"));
        assert_eq!(name("PUBLIC LAVATORY").as_deref(), Some("lavatory"));
        assert_eq!(name("KITCHEN SINK").as_deref(), Some("sink"));
        assert_eq!(name("FLOOR DRAIN"), None);
        assert_eq!(name("CLEANOUT"), None, "no word inside another word (LAV in CLEANOUT? no)");
    }

    #[test]
    fn tagged_fixtures_go_onto_the_fixtures_the_plan_draws_not_their_tag_balloons() {
        let mut d = Drawing::default();
        bath(&mut d, 1000.0, 1000.0);
        // The tags: the water closet's over the tub (as a plumbing plan sets it, clear of the
        // fixture), the lavatory's in the door swing, the tub's on the tub, the sink's by the
        // vanity, a floor drain's (no drawn family) anywhere.
        let mut devs = vec![
            dev("T-1", 1025.0, 1080.0),
            dev("T-2", 1040.0, 1045.0),
            dev("T-3", 1032.0, 1076.0),
            dev("T-4", 1080.0, 1010.0),
            dev("T-9", 1100.0, 1100.0),
        ];
        let rep = fit_fixtures(&d, &spans_all(&d), &schedule(), &mut devs, 12.0, rules());
        let at = |i: usize| devs.get(i).map(|d| d.at).unwrap_or_default();
        let near = |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).hypot(p[1] - q[1]) <= 6.0;
        assert!(near(at(0), [1022.0, 1014.5]), "the water closet on its bowl and tank: {:?}", at(0));
        assert!(near(at(1), [1065.0, 1011.0]), "the lavatory on its basin: {:?}", at(1));
        assert!(near(at(2), [1035.0, 1082.0]), "the tub on its outline, not its inner line: {:?}", at(2));
        assert!(near(at(3), [1084.0, 964.0]), "the sink on the kitchen sink, not the vanity's basin nor the cooktop: {:?}", at(3));
        assert_eq!(at(4), [1100.0, 1100.0], "a floor drain stays where its plan puts it");
        // A lavatory tag by the second vanity: at its drain.
        let mut other = vec![dev("T-2", 1125.0, 1040.0)];
        let _ = fit_fixtures(&d, &spans_all(&d), &schedule(), &mut other, 12.0, rules());
        let lav = other.first().map(|d| d.at).unwrap_or_default();
        assert!((lav[0] - 1120.0).abs() < 0.5 && (lav[1] - 1010.0).abs() < 0.5, "at the drain drawn in its vanity: {lav:?}");
        assert_eq!(rep.moved.values().sum::<usize>(), 4, "{rep:?}");
        let wc = rep.fits.iter().find(|f| f.handle.starts_with("T-1")).expect("fit");
        assert_eq!((wc.provenance, wc.tag_at), ("drawn", [1025.0, 1080.0]));
        assert!(wc.basis.contains("water closet A-1 draws (its bowl and tank"), "{}", wc.basis);
    }

    #[test]
    fn one_drawn_fixture_takes_one_tag_and_a_fixture_none_is_near_stays_and_says_so() {
        let mut d = Drawing::default();
        bath(&mut d, 0.0, 0.0);
        // Two water closet tags and one drawn water closet: the nearer tag takes it.
        let mut devs = vec![dev("T-1", 22.0, 50.0), dev("T-1", 22.0, 40.0), dev("T-1", 500.0, 500.0)];
        let rep = fit_fixtures(&d, &spans_all(&d), &schedule(), &mut devs, 12.0, rules());
        let at = |i: usize| devs.get(i).map(|d| d.at).unwrap_or_default();
        assert!((at(1)[0] - 22.0).abs() < 1.0 && (at(1)[1] - 14.5).abs() < 1.0, "the nearer one: {:?}", at(1));
        assert_eq!(at(0), [22.0, 50.0], "the farther one stays");
        assert_eq!(at(2), [500.0, 500.0], "nothing drawn within reach: stays");
        assert_eq!(rep.at_tag.get("water closet"), Some(&2));
        let stay = rep.fits.iter().find(|f| f.tag_at == [500.0, 500.0]).expect("fit");
        assert_eq!(stay.provenance, "inferred");
        assert!(stay.basis.contains("draws no water closet within 8 ft"), "{}", stay.basis);
    }

    /// A rounded outline (16 points on an ellipse): a bowl, a seat.
    fn oval(d: &mut Drawing, cx: f64, cy: f64, w: f64, h: f64) -> u64 {
        let v: Vec<PolyVertex> = (0..16)
            .map(|i| {
                let a = f64::from(i) * std::f64::consts::TAU / 16.0;
                PolyVertex::new(Vec2::new(cx + w / 2.0 * a.cos(), cy + h / 2.0 * a.sin()))
            })
            .collect();
        add(d, EntityKind::LwPolyline(LwPolyline { vertices: v, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }))
    }

    #[test]
    fn a_bowl_with_no_tank_drawn_is_taken_only_by_a_tag_no_bowl_and_tank_is_near() {
        let mut d = Drawing::default();
        bath(&mut d, 0.0, 0.0);
        // A second water closet drawn as its bowl alone (its tank in loose lines), above the first.
        oval(&mut d, 22.0, 46.0, 15.0, 18.0);
        // The first tag is nearer the lone bowl, but a bowl and tank is in reach: it takes that;
        // the second tag takes the lone bowl.
        let mut devs = vec![dev("T-1", 22.0, 38.0), dev("T-1", 22.0, 60.0)];
        let rep = fit_fixtures(&d, &spans_all(&d), &schedule(), &mut devs, 12.0, rules());
        let at = |i: usize| devs.get(i).map(|d| d.at).unwrap_or_default();
        assert!((at(0)[1] - 14.5).abs() < 1.0, "the bowl and tank first: {:?}", at(0));
        assert!((at(1)[1] - 46.0).abs() < 1.0, "the lone bowl for the other: {:?} {rep:?}", at(1));
        assert!(rep.fits.iter().any(|f| f.basis.contains("its bowl, no tank drawn")), "{rep:?}");
    }

    #[test]
    fn a_rounded_seat_in_no_counter_is_no_kitchen_sink() {
        let mut d = Drawing::default();
        oval(&mut d, 0.0, 0.0, 18.0, 18.0);
        let mut devs = vec![dev("T-4", 30.0, 0.0)];
        let rep = fit_fixtures(&d, &spans_all(&d), &schedule(), &mut devs, 12.0, rules());
        assert_eq!(devs.first().map(|d| d.at), Some([30.0, 0.0]), "{rep:?}");
    }

    #[test]
    fn a_plain_box_of_a_sinks_size_is_no_sink() {
        let mut d = Drawing::default();
        bath(&mut d, 0.0, 0.0);
        // A sink tag by the refrigerator, the sink across the counter taken by its own tag.
        let mut devs = vec![dev("T-4", 20.0, -30.0), dev("T-4", 84.0, -30.0)];
        let rep = fit_fixtures(&d, &spans_all(&d), &schedule(), &mut devs, 12.0, rules());
        assert_eq!(devs.first().map(|d| d.at), Some([20.0, -30.0]), "not onto the refrigerator: {rep:?}");
        let sink = devs.get(1).map(|d| d.at).unwrap_or_default();
        assert!((sink[0] - 84.0).abs() < 1.0 && (sink[1] + 36.0).abs() < 1.0, "the sink open along its back is a sink: {sink:?}");
    }

    #[test]
    fn an_unregistered_sheets_drawing_elsewhere_in_the_drawing_is_never_taken() {
        let mut d = Drawing::default();
        // The plan the fixture is placed on draws none; another sheet (an unregistered enlarged
        // plan, beside it in the drawing) draws one far off; a sheet of another discipline draws
        // one right there.
        let first = d.model.iter().map(|e| e.handle.0).max().unwrap_or(0) + 1;
        bath(&mut d, 5000.0, 0.0);
        let mid = d.model.iter().map(|e| e.handle.0).max().unwrap_or(0) + 1;
        bath(&mut d, 0.0, 0.0);
        let spans = vec![(first, mid, "A-8".to_string())];
        let mut devs = vec![dev("T-1", 22.0, 50.0)];
        let rep = fit_fixtures(&d, &spans, &schedule(), &mut devs, 12.0, rules());
        assert_eq!(devs.first().map(|d| d.at), Some([22.0, 50.0]), "{rep:?}");
    }

    #[test]
    fn the_model_fixture_keeps_its_tags_place_and_says_what_it_sits_on() {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        let mut t = cadaddy_walk3d::tags::ElementTag {
            id: "device:Level 1:1.8,1.2".into(),
            kind: "plumbing_fixture".into(),
            at_ft: [22.0 / 12.0, 14.5 / 12.0],
            ..Default::default()
        };
        t.set("symbol", "1A", "plan");
        sp.tags.push(t);
        let fit = Fit {
            handle: "1A".into(),
            tag_at: [24.0, 60.0],
            at: [22.0, 14.5],
            provenance: "drawn",
            basis: "on the water closet A-1 draws".into(),
            outline: None,
            level: String::new(),
            family: "water closet".into(),
        };
        assert_eq!(record(&mut sp, &[fit]), 1);
        let f = |k: &str| sp.tags.first().and_then(|t| t.fields.get(k)).map(|f| (f.value.clone(), f.source.clone()));
        assert_eq!(f("tag_at_ft"), Some(("2.00, 5.00".to_string(), "plan".to_string())));
        assert_eq!(f("position_provenance"), Some(("drawn".to_string(), "plan".to_string())));
        assert_eq!(f("position_basis").map(|v| v.0), Some("on the water closet A-1 draws".to_string()));
    }

    fn receptacle(circuit: &str, x: f64, y: f64, level: &str) -> ReadDevice {
        ReadDevice {
            tag: "REC".into(),
            at: [x, y],
            kind: "receptacle".into(),
            circuit: circuit.into(),
            level: level.into(),
            handle: format!("R{x}"),
            ..Default::default()
        }
    }

    #[test]
    fn a_circuit_label_inside_a_drawn_tub_is_no_receptacle_but_one_on_its_wall_or_beside_it_stays() {
        // Two baths (the tub 60 x 32 across each top, its drain a small circle at one end), each
        // tagged by the plumbing plan; a luminaire's circuit label printed over the tub labelled
        // the drain, which the device reading took for a receptacle.
        let mut d = Drawing::default();
        bath(&mut d, 0.0, 0.0);
        bath(&mut d, 400.0, 0.0);
        circle(&mut d, 11.0, 82.0, 1.0);
        let mut devs = vec![dev("T-3", 32.0, 76.0), dev("T-3", 432.0, 76.0)];
        for dv in &mut devs {
            dv.level = "Level 1".into();
        }
        devs.extend([
            receptacle("Q-3", 11.0, 82.0, "Level 1"),  // the drain, inside the tub
            receptacle("Q-3", 435.0, 80.0, "Level 1"), // the second tub's middle
            receptacle("Q-11", 35.0, 97.0, "Level 1"), // on the wall the tub is set against (at its edge)
            receptacle("Q-12", 80.0, 80.0, "Level 1"), // beside the tub, outside it
            receptacle("Q-13", 11.0, 82.0, "Level 2"), // another level's, at the same point
        ]);
        let rep = fit_fixtures(&d, &spans_all(&d), &schedule(), &mut devs, 12.0, rules());
        let tubs: Vec<&Fit> = rep.fits.iter().filter(|f| f.family == "tub").collect();
        assert_eq!(tubs.len(), 2, "{rep:?}");
        assert!(tubs.iter().all(|f| f.outline.is_some() && f.level == "Level 1"), "{tubs:?}");
        let inside = receptacles_inside(&rep.fits, &devs, 12.0, rules());
        let taken: Vec<&str> = inside.iter().filter_map(|i| devs.get(i.index)).map(|d| d.circuit.as_str()).collect();
        assert_eq!(taken, ["Q-3", "Q-3"], "{inside:?}");
        let why = inside.first().map(|i| i.reason.as_str()).unwrap_or_default();
        assert!(why.contains("tub") && why.contains("406.9(C)") && why.contains("Q-3"), "{why}");
        // No tub fitted (no plumbing tags): nothing taken.
        assert!(receptacles_inside(&[], &devs, 12.0, rules()).is_empty());
        // A water closet holds no such rule: a receptacle read on one stays.
        let mut wc = tubs.first().map(|f| (*f).clone()).expect("a tub fit");
        wc.family = "water closet".into();
        assert!(receptacles_inside(&[wc], &devs, 12.0, rules()).is_empty());
    }

    #[test]
    fn hostile_numbers_do_not_panic() {
        let mut d = Drawing::default();
        rect(&mut d, f64::NAN, 0.0, 10.0, f64::INFINITY, true);
        circle(&mut d, 0.0, 0.0, f64::NAN);
        rect(&mut d, -1e300, -1e300, 1e300, 1e300, true);
        let mut devs = vec![dev("T-1", f64::NAN, 0.0), dev("T-2", 1e300, -1e300), dev("T-3", 0.0, 0.0)];
        let rep = fit_fixtures(&d, &spans_all(&d), &schedule(), &mut devs, f64::NAN, rules());
        devs.push(receptacle("Q-1", f64::NAN, 0.0, ""));
        let mut fits = rep.fits;
        fits.push(Fit {
            handle: "X".into(),
            tag_at: [0.0, 0.0],
            at: [0.0, 0.0],
            provenance: "drawn",
            basis: String::new(),
            outline: Some([f64::NAN, -1e300, 1e300, f64::INFINITY]),
            level: String::new(),
            family: "tub".into(),
        });
        let _ = receptacles_inside(&fits, &devs, f64::NAN, rules());
    }
}
