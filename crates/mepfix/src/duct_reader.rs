//! Ductwork reader: detects duct runs, fittings, terminals and equipment from mechanical
//! plan sheets (DXF) and produces a network graph scored against the truth.
//!
//! Duct runs on mechanical sheets are drawn as double parallel lines on the `DUCT` layer
//! (the two sides of the duct), with optional centrelines on `CENTRE`.  Fittings are arcs
//! on `DUCT` (radius elbows: two concentric arcs) or tapered pairs (transitions).  Terminals
//! live on `DEVICE`, equipment on `EQUIP`, and size/height tags on `TEXT` with optional leader
//! lines.
//!
//! Pipeline: wall pairs → chained runs → tags → legend → inference (sizes, depths, systems) over
//! the [`DuctGraph`] of runs, fittings, terminals and equipment.  Every run, fitting, terminal and
//! box says where its reading came from ([`Source`]) and how sure it is ([`Status`]): a size read
//! from a tag is `read`; one taken from the wall gap or a connected tagged duct is `inferred`;
//! the stock 12" depth is `defaulted`, never shown as read.
//!
//! The reader works in model inches only.  [`DuctNetwork::to_found_runs`] converts to building
//! feet with the alignment [`crate::sheet_reader`] read from the sheet's grid, and the result is
//! scored against the truth via [`crate::score::score_runs`].

use crate::score::{self, FoundRun};
use crate::sheet_reader::{self, Alignment, RecognisedSheet};
use crate::truth::Truth;
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use std::collections::{BTreeMap, VecDeque};

// ---- Limits ----------------------------------------------------------------

/// Maximum segments to process from a single drawing (hostile-input cap).
const MAX_SEGS: usize = 100_000;
/// Maximum duct pairs to consider.
const MAX_PAIRS: usize = 20_000;
/// Maximum runs after chaining.
const MAX_RUNS: usize = 5_000;
/// Maximum arcs considered for radius elbows.
const MAX_ARCS: usize = 20_000;
/// Duct half-width range in model inches (the distance from centreline to one wall line).
/// Smallest: 4" duct ⇒ 2" half; largest: 60" ⇒ 30".  Scaled by the sheet.
const MIN_HALF_IN: f64 = 1.5;
const MAX_HALF_IN: f64 = 35.0;
/// Alignment tolerance: how far a found centreline may differ from a truth run, in feet.
const MATCH_TOL_FT: f64 = 1.5;
/// The depth given to a rectangular duct nothing on the sheet sizes (inches).
pub const DEFAULT_DEPTH_IN: f64 = 12.0;
/// The width given to a duct drawn only as a centreline, with no tag (inches).
pub const DEFAULT_WIDTH_IN: f64 = 12.0;

// ---- Public types ----------------------------------------------------------

/// Where a reading came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// Text printed for the item: a size or height tag, a system label, an equipment tag.
    Tag,
    /// The sheet's legend or abbreviation list (a label read through it).
    Legend,
    /// The linework itself: the gap between a duct's walls, a symbol's shape, where runs meet.
    Geometry,
    /// Taken from something connected or nearby: a tagged duct of the same width, the
    /// terminals and boxes a duct leads to.
    Proximity,
    /// Nothing on the sheet said: a stock value.
    #[default]
    Default,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Tag => "tag",
            Source::Legend => "legend",
            Source::Geometry => "geometry",
            Source::Proximity => "proximity",
            Source::Default => "default",
        }
    }
}

/// How sure a reading is: printed for it, worked out, or a stock guess.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Read,
    Inferred,
    #[default]
    Defaulted,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Read => "read",
            Status::Inferred => "inferred",
            Status::Defaulted => "defaulted",
        }
    }
}

/// A reading's source and status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Provenance {
    pub source: Source,
    pub status: Status,
}

impl Provenance {
    pub const fn read(source: Source) -> Self {
        Provenance { source, status: Status::Read }
    }
    pub const fn inferred(source: Source) -> Self {
        Provenance { source, status: Status::Inferred }
    }
    pub const DEFAULTED: Provenance = Provenance { source: Source::Default, status: Status::Defaulted };
}

/// A detected duct segment (the centreline of a parallel-line pair or a centreline entity).
#[derive(Clone, Debug)]
pub struct DetectedSegment {
    /// Centreline endpoints in model inches.
    pub a: Vec2,
    pub b: Vec2,
    /// Half-width in model inches (half the gap between the two wall lines).
    pub half_w: f64,
    /// `rect` or `round`.
    pub shape: String,
    /// Confidence 0–1.
    pub confidence: f64,
}

/// A detected fitting at a node.
#[derive(Clone, Debug)]
pub struct DetectedFitting {
    /// `elbow`, `tee`, `cross`, `transition`, `reducer`, `turn`.
    pub kind: String,
    pub at: Vec2,
    pub confidence: f64,
    pub source: Source,
    pub status: Status,
}

/// A detected terminal device.
#[derive(Clone, Debug)]
pub struct DetectedTerminal {
    /// `supply_diffuser`, `return_grille`, `exhaust_grille`.
    pub kind: String,
    pub at: Vec2,
    pub confidence: f64,
    pub source: Source,
    pub status: Status,
}

/// A detected equipment box.
#[derive(Clone, Debug)]
pub struct DetectedEquipment {
    /// `ahu`, `vav`, `exhaust_fan`, `unknown`.
    pub kind: String,
    pub tag: String,
    pub min: Vec2,
    pub max: Vec2,
    pub confidence: f64,
    pub source: Source,
    pub status: Status,
}

/// A size tag found on the sheet.
#[derive(Clone, Debug)]
pub struct DetectedTag {
    pub text: String,
    /// Where the tag text sits, model inches.
    pub at: Vec2,
    /// The size parsed from the text.
    pub size_in: Option<[f64; 2]>,
    /// System abbreviation found in the tag, if any.
    pub system: Option<String>,
    /// Whether a leader line connects it to a run.
    pub has_leader: bool,
    /// The run it was attached to.
    pub run: Option<usize>,
}

/// A legend entry read from the sheet.
#[derive(Clone, Debug)]
pub struct DetectedLegend {
    pub symbol: String,
    pub text: String,
}

/// A chained duct run (multiple segments joined end to end).
#[derive(Clone, Debug)]
pub struct DuctRun {
    /// Centreline vertices in model inches.
    pub points: Vec<Vec2>,
    /// Width (across run) in inches: from its size tag, else the parallel-pair gap.
    pub width_in: f64,
    /// Depth in inches (for rectangular; 0 for round).
    pub depth_in: f64,
    /// `rect` or `round`.
    pub shape: String,
    /// The system code (`SA`, `RA`, `EA`, `OA`), if determined.
    pub system: Option<String>,
    /// Bottom height in feet above finished floor, if a height tag was found.
    pub bottom_ft: Option<f64>,
    /// Confidence 0–1.
    pub confidence: f64,
    /// The measured gap between the walls (inches); `None` for a run drawn only as a centreline.
    pub gap_in: Option<f64>,
    /// Where the width and shape came from.
    pub size: Provenance,
    /// Where the depth came from (the size's own for round ducts).
    pub depth: Provenance,
    /// Where the system came from.
    pub system_from: Provenance,
    /// The size tag attached to it (index into [`DuctNetwork::tags`]).
    pub tag: Option<usize>,
}

impl DuctRun {
    /// The run's overall status: the least sure of its width/shape and (rectangular) depth.
    pub fn status(&self) -> Status {
        if self.shape == "rect" { self.size.status.max(self.depth.status) } else { self.size.status }
    }

    /// The run's size as the truth writes it: `[width, depth]`, or `[diameter, diameter]`.
    pub fn size_in(&self) -> [f64; 2] {
        if self.shape == "rect" { [self.width_in, self.depth_in] } else { [self.width_in, self.width_in] }
    }

    fn new(points: Vec<Vec2>, gap: Option<f64>, confidence: f64) -> DuctRun {
        let (width_in, size) = match gap {
            Some(g) => (g, Provenance::inferred(Source::Geometry)),
            None => (DEFAULT_WIDTH_IN, Provenance::DEFAULTED),
        };
        DuctRun {
            points,
            width_in,
            depth_in: 0.0,
            shape: "rect".into(),
            system: None,
            bottom_ft: None,
            confidence,
            gap_in: gap,
            size,
            depth: Provenance::DEFAULTED,
            system_from: Provenance::DEFAULTED,
            tag: None,
        }
    }
}

/// A node of the duct network: a run end, a fitting, a terminal or a box.
#[derive(Clone, Debug)]
pub struct GraphNode {
    /// `end`, `joint` (straight on), `elbow`, `transition`, `reducer`, `tee`, `cross`, `terminal`,
    /// `equipment`.
    pub kind: String,
    pub at: Vec2,
    /// For `terminal`: index into [`DuctNetwork::terminals`].
    pub terminal: Option<usize>,
    /// For `equipment`: index into [`DuctNetwork::equipment`].
    pub equipment: Option<usize>,
}

/// An edge of the duct network: a piece of a run between two nodes, or the link from a run's
/// end to the terminal or box it serves (`run` is `None`).
#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub a: usize,
    pub b: usize,
    /// Index into [`DuctNetwork::runs`]; `None` for a terminal/equipment link.
    pub run: Option<usize>,
    /// Centreline, model inches (empty for a link).
    pub points: Vec<Vec2>,
    pub size_in: [f64; 2],
    pub shape: String,
    pub system: Option<String>,
    pub confidence: f64,
}

/// The connected duct network: nodes and the segments between them.
#[derive(Clone, Debug, Default)]
pub struct DuctGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl DuctGraph {
    /// The duct edges (not links) meeting at each node.
    pub fn degrees(&self) -> Vec<usize> {
        let mut deg = vec![0; self.nodes.len()];
        for e in self.edges.iter().filter(|e| e.run.is_some()) {
            for n in [e.a, e.b] {
                if let Some(d) = deg.get_mut(n) {
                    *d += 1;
                }
            }
        }
        deg
    }

    /// Branch points: tees and crosses.
    pub fn branches(&self) -> usize {
        self.nodes.iter().filter(|n| n.kind == "tee" || n.kind == "cross").count()
    }

    /// The terminals joined to the network.
    pub fn connected_terminals(&self) -> usize {
        let linked: Vec<bool> = self.linked();
        self.nodes.iter().enumerate().filter(|(i, n)| n.terminal.is_some() && linked.get(*i).copied().unwrap_or(false)).count()
    }

    /// The terminals from which the ducts lead (through any number of fittings) to a box.
    pub fn terminals_reaching_equipment(&self) -> usize {
        let comp = self.components(false);
        let with_equipment: std::collections::BTreeSet<usize> =
            self.nodes.iter().enumerate().filter(|(_, n)| n.equipment.is_some()).filter_map(|(i, _)| comp.get(i).copied()).collect();
        self.nodes
            .iter()
            .enumerate()
            .filter(|(i, n)| n.terminal.is_some() && self.linked().get(*i).copied().unwrap_or(false))
            .filter(|(i, _)| comp.get(*i).is_some_and(|c| with_equipment.contains(c)))
            .count()
    }

    fn linked(&self) -> Vec<bool> {
        let mut l = vec![false; self.nodes.len()];
        for e in &self.edges {
            for n in [e.a, e.b] {
                if let Some(x) = l.get_mut(n) {
                    *x = true;
                }
            }
        }
        l
    }

    /// Connected component of every node; `cut_at_equipment`: boxes don't join what they touch
    /// (a VAV's inlet and outlet sides stay apart, as do an AHU's supply and return).
    fn components(&self, cut_at_equipment: bool) -> Vec<usize> {
        let n = self.nodes.len();
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for e in &self.edges {
            if let (Some(x), Some(_)) = (adj.get_mut(e.a), self.nodes.get(e.b)) {
                x.push(e.b);
            }
            if let (Some(x), Some(_)) = (adj.get_mut(e.b), self.nodes.get(e.a)) {
                x.push(e.a);
            }
        }
        let mut comp = vec![usize::MAX; n];
        let mut next = 0;
        for s in 0..n {
            if comp.get(s) != Some(&usize::MAX) {
                continue;
            }
            let mut q = VecDeque::from([s]);
            if let Some(c) = comp.get_mut(s) {
                *c = next;
            }
            while let Some(u) = q.pop_front() {
                let stop = cut_at_equipment && self.nodes.get(u).is_some_and(|x| x.equipment.is_some()) && u != s;
                if stop {
                    continue;
                }
                for &v in adj.get(u).map(Vec::as_slice).unwrap_or_default() {
                    let is_eq = self.nodes.get(v).is_some_and(|x| x.equipment.is_some());
                    if comp.get(v) == Some(&usize::MAX) && !(cut_at_equipment && is_eq) {
                        if let Some(c) = comp.get_mut(v) {
                            *c = next;
                        }
                        q.push_back(v);
                    }
                }
            }
            next += 1;
        }
        comp
    }
}

/// The full duct network read from a single mechanical sheet.
#[derive(Clone, Debug, Default)]
pub struct DuctNetwork {
    pub runs: Vec<DuctRun>,
    pub fittings: Vec<DetectedFitting>,
    pub terminals: Vec<DetectedTerminal>,
    pub equipment: Vec<DetectedEquipment>,
    pub tags: Vec<DetectedTag>,
    pub legend: Vec<DetectedLegend>,
    pub abbreviations: BTreeMap<String, String>,
    pub graph: DuctGraph,
}

impl DuctNetwork {
    /// Convert runs to [`FoundRun`]s in building feet for scoring, with the sheet's alignment as
    /// [`crate::sheet_reader`] read it.
    pub fn to_found_runs(&self, level: &str, al: &Alignment) -> Vec<FoundRun> {
        let mut out = Vec::new();
        for run in &self.runs {
            let sys = run.system.clone().unwrap_or_default();
            for w in run.points.windows(2) {
                let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
                out.push(FoundRun {
                    level: level.to_string(),
                    kind: "duct".into(),
                    system: sys.clone(),
                    shape: run.shape.clone(),
                    size_in: run.size_in(),
                    from_ft: sheet_reader::building_from_model(al, [a.x, a.y]),
                    to_ft: sheet_reader::building_from_model(al, [b.x, b.y]),
                    bottom_ft: run.bottom_ft,
                });
            }
        }
        out
    }
}

// ---- Geometry helpers ------------------------------------------------------

/// A straight segment from a line or polyline.
#[derive(Clone, Debug)]
struct LineSeg {
    a: Vec2,
    b: Vec2,
    layer: String,
}

/// Extract all straight segments from the model space of a drawing.
fn line_segments(d: &Drawing) -> Vec<LineSeg> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_SEGS * 2) {
        match &e.kind {
            EntityKind::Line(l) => {
                let (a, b) = (l.a.xy(), l.b.xy());
                if a.is_finite() && b.is_finite() && a.dist(b) > 1e-9 {
                    out.push(LineSeg { a, b, layer: e.common.layer.to_string() });
                }
            }
            EntityKind::LwPolyline(p) => {
                let verts: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
                let n = verts.len();
                let m = if p.closed { n } else { n.saturating_sub(1) };
                for i in 0..m {
                    let (Some(a), Some(b)) = (verts.get(i), verts.get((i + 1) % n.max(1))) else {
                        continue;
                    };
                    if a.is_finite() && b.is_finite() && a.dist(*b) > 1e-9 {
                        out.push(LineSeg { a: *a, b: *b, layer: e.common.layer.to_string() });
                    }
                }
            }
            _ => {}
        }
        if out.len() >= MAX_SEGS {
            break;
        }
    }
    out
}

/// An arc (angles in radians, counter-clockwise) on a layer.
#[derive(Clone, Debug)]
struct ArcSeg {
    c: Vec2,
    r: f64,
    start: f64,
    end: f64,
    layer: String,
}

/// Extract arcs (not full circles) from model space.
fn arcs(d: &Drawing) -> Vec<ArcSeg> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if let EntityKind::Arc(a) = &e.kind
            && a.center.xy().is_finite()
            && a.radius.is_finite()
            && a.radius > 0.0
            && a.start.is_finite()
            && a.end.is_finite()
        {
            out.push(ArcSeg { c: a.center.xy(), r: a.radius, start: a.start, end: a.end, layer: e.common.layer.to_string() });
            if out.len() >= MAX_ARCS {
                break;
            }
        }
    }
    out
}

/// Extract text entities: (text, position, layer, height).
fn text_entities(d: &Drawing) -> Vec<(String, Vec2, String, f64)> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if let EntityKind::Text(t) = &e.kind {
            let at = t.align_pt.unwrap_or(t.insert).xy();
            if at.is_finite() && !t.value.trim().is_empty() {
                out.push((t.value.clone(), at, e.common.layer.to_string(), t.height));
            }
        }
    }
    out
}

/// Extract rectangles from closed 4-vertex polylines.
fn rectangles(d: &Drawing, layer_filter: &str) -> Vec<(Vec2, Vec2, String)> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case(layer_filter) {
            continue;
        }
        if let EntityKind::LwPolyline(p) = &e.kind
            && p.closed
            && p.vertices.len() == 4
        {
            let pts: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
            if pts.iter().all(|p| p.is_finite()) {
                let min = Vec2::new(pts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min), pts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min));
                let max =
                    Vec2::new(pts.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max), pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max));
                out.push((min, max, e.common.layer.to_string()));
            }
        }
    }
    out
}

fn minmax(a: f64, b: f64) -> (f64, f64) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Distance from point p to line segment a-b.
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + d * t)
}

// ---- Parallel pair detection -----------------------------------------------

/// A parallel pair of line segments representing the two walls of a duct.
#[derive(Clone, Debug)]
struct DuctPair {
    /// Centreline endpoints.
    c0: Vec2,
    c1: Vec2,
    /// Half-width (distance from centreline to each wall).
    half_w: f64,
}

/// Find parallel line pairs on the DUCT layer that represent duct walls.
fn duct_pairs(segs: &[LineSeg]) -> Vec<DuctPair> {
    use std::f64::consts::PI;

    let duct_segs: Vec<&LineSeg> = segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("DUCT")).collect();
    if duct_segs.is_empty() {
        return Vec::new();
    }

    // Half-width limits in model units (drawing inches).
    let min_hw = MIN_HALF_IN;
    let max_hw = MAX_HALF_IN;

    // Group segments by direction (1-degree buckets), modulo 180°.
    let mut groups: BTreeMap<i64, Vec<(f64, usize)>> = BTreeMap::new();
    for (i, s) in duct_segs.iter().enumerate() {
        let d = s.b - s.a;
        let ang = d.y.atan2(d.x).rem_euclid(PI);
        let key = (ang.to_degrees() * 2.0).round() as i64 % 360;
        let n = Vec2::new(-ang.sin(), ang.cos());
        groups.entry(key).or_default().push((n.dot(s.a), i));
    }

    let mut out = Vec::new();
    for (_key, mut g) in groups {
        g.sort_by(|a, b| a.0.total_cmp(&b.0));
        for x in 0..g.len() {
            if out.len() >= MAX_PAIRS {
                return out;
            }
            for y in x + 1..g.len() {
                let gap = (g[y].0 - g[x].0).abs();
                if gap > max_hw * 2.0 + 1.0 {
                    break;
                }
                let half = gap / 2.0;
                if half < min_hw || half > max_hw {
                    continue;
                }
                let (s1, s2) = (duct_segs[g[x].1], duct_segs[g[y].1]);
                let d1 = s1.b - s1.a;
                let d2 = s2.b - s2.a;
                let l1 = d1.len();
                let l2 = d2.len();
                if l1 < 1e-9 || l2 < 1e-9 {
                    continue;
                }
                let u1 = d1 * (1.0 / l1);
                let u2 = d2 * (1.0 / l2);
                // Check parallelism (cross product near 0).
                if u1.cross(u2).abs() > 0.03 {
                    continue;
                }
                let u = u1;
                let n = Vec2::new(-u.y, u.x);
                let (a0, a1) = minmax(u.dot(s1.a), u.dot(s1.b));
                let (b0, b1) = minmax(u.dot(s2.a), u.dot(s2.b));
                let overlap = a1.min(b1) - a0.max(b0);
                // As long as the duct is wide, or (a short piece between a tee and a
                // transition) at least 8".
                if overlap < (half * 2.0).min(8.0) {
                    continue;
                }
                let mid_n = (n.dot(s1.a) + n.dot(s2.a)) / 2.0;
                let lo = a0.max(b0);
                let hi = a1.min(b1);
                out.push(DuctPair { c0: u * lo + n * mid_n, c1: u * hi + n * mid_n, half_w: half });
                if out.len() >= MAX_PAIRS {
                    return out;
                }
            }
        }
    }
    out
}

// ---- Radius elbows ----------------------------------------------------------

/// Radius elbows: two concentric `DUCT` arcs over the same sweep (the duct's inner and outer
/// walls).  Each becomes two straight pieces through the corner the elbow turns (so it chains
/// with the runs it joins, and the network reaches the corner like the truth's centrelines),
/// and an elbow fitting at that corner.
fn radius_elbows(arcs: &[ArcSeg]) -> (Vec<DuctPair>, Vec<DetectedFitting>) {
    use std::f64::consts::TAU;
    let mut duct: Vec<&ArcSeg> = arcs.iter().filter(|a| a.layer.eq_ignore_ascii_case("DUCT")).collect();
    duct.sort_by(|a, b| a.c.x.total_cmp(&b.c.x));
    let same_angle = |x: f64, y: f64| {
        let d = (x - y).rem_euclid(TAU);
        !(0.02..=TAU - 0.02).contains(&d)
    };
    let mut used = vec![false; duct.len()];
    let (mut pairs, mut fittings) = (Vec::new(), Vec::new());
    for i in 0..duct.len() {
        let Some(&p) = duct.get(i) else { continue };
        if used.get(i).copied().unwrap_or(true) {
            continue;
        }
        for j in i + 1..duct.len() {
            let Some(&q) = duct.get(j) else { continue };
            if q.c.x - p.c.x > 0.5 {
                break;
            }
            if used.get(j).copied().unwrap_or(true) || p.c.dist(q.c) > 0.5 || !same_angle(p.start, q.start) || !same_angle(p.end, q.end) {
                continue;
            }
            let half = (p.r - q.r).abs() / 2.0;
            let sweep = (p.end - p.start).rem_euclid(TAU);
            if !(MIN_HALF_IN..=MAX_HALF_IN).contains(&half) || !(0.1..3.0).contains(&sweep) {
                continue;
            }
            let rm = (p.r + q.r) / 2.0;
            let dir = |a: f64| Vec2::new(a.cos(), a.sin());
            let p1 = p.c + dir(p.start) * rm;
            let p2 = p.c + dir(p.start + sweep) * rm;
            let corner = p.c + dir(p.start + sweep / 2.0) * (rm / (sweep / 2.0).cos());
            if !corner.is_finite() {
                continue;
            }
            pairs.push(DuctPair { c0: p1, c1: corner, half_w: half });
            pairs.push(DuctPair { c0: corner, c1: p2, half_w: half });
            fittings.push(DetectedFitting { kind: "elbow".into(), at: corner, confidence: 0.8, source: Source::Geometry, status: Status::Read });
            if let Some(u) = used.get_mut(i) {
                *u = true;
            }
            if let Some(u) = used.get_mut(j) {
                *u = true;
            }
            break;
        }
        if pairs.len() >= MAX_PAIRS {
            break;
        }
    }
    (pairs, fittings)
}

// ---- Run chaining ----------------------------------------------------------

/// How the reader chains wall pairs into runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DuctOptions {
    /// Chain every pair, skipping those already in a run (and runs along one already found).
    /// On by default.  Off, chaining stops at the first pair an earlier run took (the old
    /// behaviour, which lost every duct after it).
    pub all_pairs: bool,
}

impl Default for DuctOptions {
    fn default() -> Self {
        DuctOptions { all_pairs: true }
    }
}

/// Chain parallel pairs that share an endpoint into continuous runs.
fn chain_runs(pairs: &[DuctPair], o: &DuctOptions) -> Vec<DuctRun> {
    if pairs.is_empty() {
        return Vec::new();
    }

    let tol = 2.0; // model inches tolerance for endpoint matching
    let mut used = vec![false; pairs.len()];
    let mut runs = Vec::new();

    for start in 0..pairs.len() {
        if runs.len() >= MAX_RUNS {
            break;
        }
        if used[start] {
            // A pair already chained into an earlier run: skipped (or, with `all_pairs` off, the
            // end of the chaining).
            if o.all_pairs {
                continue;
            }
            break;
        }
        used[start] = true;
        let p = &pairs[start];
        let mut pts = vec![p.c0, p.c1];
        let mut total_hw = p.half_w;
        let mut count = 1.0;

        // Extend forward from the tail.
        while let Some(&tail) = pts.last() {
            let next = pairs
                .iter()
                .enumerate()
                .find(|(i, q)| !used[*i] && (q.c0.dist(tail) < tol || q.c1.dist(tail) < tol) && (q.half_w - p.half_w).abs() < p.half_w * 0.3 + 1.0);
            if let Some((i, q)) = next {
                used[i] = true;
                if q.c0.dist(tail) < tol {
                    pts.push(q.c1);
                } else {
                    pts.push(q.c0);
                }
                total_hw += q.half_w;
                count += 1.0;
            } else {
                break;
            }
        }

        // Extend backward from the head.
        while let Some(&head) = pts.first() {
            let next = pairs
                .iter()
                .enumerate()
                .find(|(i, q)| !used[*i] && (q.c0.dist(head) < tol || q.c1.dist(head) < tol) && (q.half_w - p.half_w).abs() < p.half_w * 0.3 + 1.0);
            if let Some((i, q)) = next {
                used[i] = true;
                if q.c1.dist(head) < tol {
                    pts.insert(0, q.c0);
                } else {
                    pts.insert(0, q.c1);
                }
                total_hw += q.half_w;
                count += 1.0;
            } else {
                break;
            }
        }

        let avg_hw = total_hw / count;
        let width = avg_hw * 2.0; // full width in model inches (= drawing inches = actual inches)

        // Pieces of a duct's walls pair up many ways; a run lying along one already found is
        // the same duct again.
        let covered = |q: Vec2| {
            runs.iter().any(|r: &DuctRun| {
                r.points.windows(2).any(|w| match (w.first(), w.get(1)) {
                    (Some(a), Some(b)) => seg_dist(q, *a, *b) < r.width_in.max(width) / 2.0,
                    _ => false,
                })
            })
        };
        let (Some(&first), Some(&last)) = (pts.first(), pts.last()) else { continue };
        if o.all_pairs && covered(first) && covered(last) && covered((first + last) * 0.5) {
            continue;
        }

        runs.push(DuctRun::new(pts, Some(width), 0.7));
    }
    runs
}

// ---- Tag parsing -----------------------------------------------------------

/// Parse a duct size from text: `24x12`, `24"x12"`, `24/12` → [24, 12]; `10"Ø`, `10" DIA`,
/// `12Ø`, `10" RD` → [10, 0].  Feet-and-inches words (`11'-10"`, a height note) are never sizes.
fn parse_size(t: &str) -> Option<[f64; 2]> {
    let up = t.to_ascii_uppercase().replace('×', "X");
    let num = |s: &str| s.trim().trim_end_matches('"').parse::<f64>().ok().filter(|v| *v > 0.0 && *v <= 200.0);
    // Rectangular: `24x12`, `24"x12"`, `24/12`.
    for w in up.split_whitespace() {
        if w.contains('\'') {
            continue;
        }
        if let Some((a, b)) = w.split_once('X')
            && let (Some(a), Some(b)) = (num(a), num(b))
        {
            return Some([a, b]);
        }
        // A slash is a size only between two whole duct dimensions (not a fraction `3/4"`).
        if !w.contains('"')
            && let Some((a, b)) = w.split_once('/')
            && let (Some(a), Some(b)) = (num(a), num(b))
            && a >= 4.0
            && b >= 4.0
            && a.fract() == 0.0
            && b.fract() == 0.0
        {
            return Some([a, b]);
        }
    }
    // Round: `10"Ø`, `10" DIA`, `12Ø`, `10" RD`.
    let words: Vec<&str> = up.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if w.contains('\'') {
            continue;
        }
        let round = w.contains('Ø')
            || w.contains("DIA")
            || words.get(i + 1).is_some_and(|n| n.starts_with("DIA") || n.starts_with('Ø') || n.starts_with("RD"));
        let inch = w.contains('"');
        if !(round || inch) {
            continue;
        }
        let digits: String = w.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if let Some(v) = digits.parse::<f64>().ok().filter(|v| *v > 0.0 && *v <= 120.0) {
            return Some([v, 0.0]);
        }
    }
    None
}

/// Parse a height tag: BOT/BOD/CL/TOP/TOD @ feet-inches.
fn parse_height(t: &str, section_ft: f64) -> Option<f64> {
    let up = t.to_ascii_uppercase();
    let after = |keys: &[&str]| {
        keys.iter()
            .filter_map(|k| up.find(k).map(|i| i + k.len()))
            .min()
            .and_then(|i| up.get(i..))
            .and_then(parse_feet_inches)
            .map(|inches| inches / 12.0)
    };
    if let Some(v) = after(&["BOT", "BOD", "B.O.", "BOP"]) {
        Some(v)
    } else if let Some(v) = after(&["CL ", "CL@", "C.L.", "CTR", "CENTER", "CENTRE"]) {
        Some(v - section_ft / 2.0)
    } else if let Some(v) = after(&["TOP", "TOD", "T.O."]) {
        Some(v - section_ft)
    } else {
        after(&["EL", "AFF", "A.F.F", "@"])
    }
}

/// Parse `10'-6"` or `10'6"` or `126` (inches) into total inches.
fn parse_feet_inches(s: &str) -> Option<f64> {
    let s = s.trim().trim_start_matches(|c: char| !c.is_ascii_digit() && c != '-');
    // Try feet-inches: `10'-6"`.
    if let Some(tick) = s.find('\'') {
        let ft: f64 = s[..tick].trim().parse().ok()?;
        let rest = s[tick + 1..].trim().trim_start_matches('-').trim();
        let inches: f64 = rest.trim_end_matches('"').trim().parse().unwrap_or(0.0);
        return Some(ft * 12.0 + inches);
    }
    // Pure number (inches).
    s.trim_end_matches('"').trim().parse::<f64>().ok().filter(|v| *v > 0.0)
}

/// Extract the system abbreviation from tag text, using the abbreviation map.
fn extract_system(text: &str, abbr_map: &BTreeMap<String, String>) -> Option<String> {
    let up = text.to_ascii_uppercase();
    // Check direct system codes first.
    for code in ["SA", "RA", "EA", "OA"] {
        if up.contains(code) {
            return Some(code.to_string());
        }
    }
    // Check abbreviation map.
    for (abbrev, code) in abbr_map {
        if up.contains(&abbrev.to_ascii_uppercase()) {
            return Some(code.clone());
        }
    }
    None
}

// ---- Fitting detection -----------------------------------------------------

/// Turn up/down markers: small, roughly square `DUCT` boxes with an X (min, max corners).
fn marker_boxes(d: &Drawing, segs: &[LineSeg]) -> Vec<(Vec2, Vec2)> {
    let mut out = Vec::new();
    let duct_lines: Vec<&LineSeg> = segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("DUCT")).collect();
    for (min, max, _) in rectangles(d, "DUCT") {
        let w = max.x - min.x;
        let h = max.y - min.y;
        // A turn marker is roughly square and small.
        if w < 1.0 || h < 1.0 || (w / h - 1.0).abs() > 0.5 {
            continue;
        }
        // Check for internal crossing lines (the X).
        let has_cross = duct_lines.iter().any(|l| {
            let mid = (l.a + l.b) * 0.5;
            mid.x > min.x && mid.x < max.x && mid.y > min.y && mid.y < max.y && l.a.dist(l.b) > w * 0.5
        });
        if has_cross {
            out.push((min, max));
        }
        if out.len() >= MAX_RUNS {
            break;
        }
    }
    out
}

/// Whether a segment is part of a marker box (a side or the X), not a duct wall.
fn in_marker(s: &LineSeg, boxes: &[(Vec2, Vec2)]) -> bool {
    let inside = |p: Vec2, (mi, ma): &(Vec2, Vec2)| p.x >= mi.x - 0.5 && p.x <= ma.x + 0.5 && p.y >= mi.y - 0.5 && p.y <= ma.y + 0.5;
    boxes.iter().any(|b| inside(s.a, b) && inside(s.b, b))
}

/// Fittings drawn as symbols: turn up/down markers.  Elbows come from [`radius_elbows`]; tees,
/// crosses, transitions and mitred elbows from where runs meet in the [`DuctGraph`].
fn detect_fittings(boxes: &[(Vec2, Vec2)]) -> Vec<DetectedFitting> {
    boxes
        .iter()
        .map(|(min, max)| DetectedFitting {
            kind: "turn".into(),
            at: (*min + *max) * 0.5,
            confidence: 0.8,
            source: Source::Geometry,
            status: Status::Read,
        })
        .collect()
}

// ---- Terminal detection ----------------------------------------------------

/// Detect terminals (diffusers, grilles) from DEVICE layer patterns.
fn detect_terminals(d: &Drawing) -> Vec<DetectedTerminal> {
    let mut terminals = Vec::new();
    let device_rects = rectangles(d, "DEVICE");
    let segs = line_segments(d);
    let device_lines: Vec<&LineSeg> = segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("DEVICE")).collect();

    for (min, max, _) in &device_rects {
        let center = (*min + *max) * 0.5;
        let w = max.x - min.x;
        let h = max.y - min.y;

        // Device symbols are small-ish rectangles.
        if w < 0.5 || h < 0.5 || w > 200.0 || h > 200.0 {
            continue;
        }
        // A diffuser's inner square is part of its symbol, not another device.
        if device_rects.iter().any(|(mi, ma, _)| mi.x < min.x - 0.25 && ma.x > max.x + 0.25 && mi.y < min.y - 0.25 && ma.y > max.y + 0.25) {
            continue;
        }

        // Count internal lines to classify.
        let internal: Vec<&&LineSeg> = device_lines
            .iter()
            .filter(|l| {
                let mid = (l.a + l.b) * 0.5;
                mid.x >= min.x - 0.1 && mid.x <= max.x + 0.1 && mid.y >= min.y - 0.1 && mid.y <= max.y + 0.1
            })
            .collect();

        // Supply diffuser: X pattern + inner rectangle.
        let has_diag = internal.iter().any(|l| {
            let d = l.b - l.a;
            d.x.abs() > w * 0.3 && d.y.abs() > h * 0.3
        });
        let has_horiz = internal.iter().any(|l| {
            let d = l.b - l.a;
            d.y.abs() < h * 0.2 && d.x.abs() > w * 0.3
        });

        let kind = if has_diag && internal.len() >= 2 {
            // Check for inner rectangle (supply diffuser has one).
            let inner_rects =
                device_rects.iter().filter(|(mi, ma, _)| mi.x > min.x + 0.5 && ma.x < max.x - 0.5 && mi.y > min.y + 0.5 && ma.y < max.y - 0.5);
            if inner_rects.count() > 0 { "supply_diffuser" } else { "exhaust_grille" }
        } else if has_horiz && internal.len() >= 2 {
            "return_grille"
        } else {
            continue;
        };

        terminals.push(DetectedTerminal { kind: kind.into(), at: center, confidence: 0.7, source: Source::Geometry, status: Status::Read });
    }
    terminals
}

// ---- Equipment detection ---------------------------------------------------

/// Detect equipment boxes (AHU, VAV) from EQUIP layer.
fn detect_equipment(d: &Drawing) -> Vec<DetectedEquipment> {
    let mut equipment = Vec::new();
    let equip_rects = rectangles(d, "EQUIP");
    let texts = text_entities(d);

    for (min, max, _) in &equip_rects {
        let w = max.x - min.x;
        let h = max.y - min.y;
        if w < 2.0 || h < 2.0 {
            continue;
        }

        // Its tag: the nearest text that reads as an equipment tag (out on a leader beside a VAV
        // box, inside an AHU), else the nearest text at all (a guess).
        let reach = w.max(h) * 3.0;
        let center = (*min + *max) * 0.5;
        let kind_of = |t: &str| {
            let up = t.to_ascii_uppercase();
            if up.contains("AHU") {
                "ahu"
            } else if up.contains("VAV") || up.starts_with("VB") {
                "vav"
            } else if up.starts_with("EF") || up.contains("FAN") {
                "exhaust_fan"
            } else {
                "unknown"
            }
        };
        let near = |(_, at, layer, _): &&(String, Vec2, String, f64), r: f64| {
            (layer.eq_ignore_ascii_case("TEXT") || layer.eq_ignore_ascii_case("EQUIP")) && at.dist(center) < r
        };
        let by_dist = |a: &&(String, Vec2, String, f64), b: &&(String, Vec2, String, f64)| a.1.dist(center).total_cmp(&b.1.dist(center));
        let tag = texts
            .iter()
            .filter(|t| near(t, reach + 120.0) && kind_of(&t.0) != "unknown")
            .min_by(by_dist)
            .or_else(|| texts.iter().filter(|t| near(t, reach)).min_by(by_dist));
        let Some((t, _, _, _)) = tag else { continue };
        let (tag_text, kind) = (t.clone(), kind_of(t).to_string());

        // The kind is read from its tag; an unrecognised tag leaves it a guess.
        let status = if kind == "unknown" { Status::Inferred } else { Status::Read };
        equipment.push(DetectedEquipment { kind, tag: tag_text, min: *min, max: *max, confidence: 0.7, source: Source::Tag, status });
    }
    equipment
}

// ---- Legend reading ---------------------------------------------------------

/// Read legend entries from the LEGEND layer.
fn read_legend(d: &Drawing) -> (Vec<DetectedLegend>, BTreeMap<String, String>) {
    let texts = text_entities(d);
    let legend_texts: Vec<&(String, Vec2, String, f64)> = texts.iter().filter(|(_, _, layer, _)| layer.eq_ignore_ascii_case("LEGEND")).collect();

    let mut legend = Vec::new();
    let mut abbr_map = BTreeMap::new();

    for (text, _, _, _) in &legend_texts {
        // Abbreviation entries: `SA = SUPPLY AIR` or `BOD = BOTTOM OF DUCT`.
        if let Some((left, right)) = text.split_once('=') {
            let left = left.trim().to_ascii_uppercase();
            let right = right.trim();
            if !left.is_empty() && !right.is_empty() {
                // Map the abbreviation back to a system code.
                let system = match right.to_ascii_uppercase() {
                    r if r.contains("SUPPLY AIR") => Some("SA"),
                    r if r.contains("RETURN AIR") => Some("RA"),
                    r if r.contains("EXHAUST AIR") => Some("EA"),
                    r if r.contains("OUTSIDE AIR") => Some("OA"),
                    _ => None,
                };
                if let Some(sys) = system {
                    abbr_map.insert(left.clone(), sys.to_string());
                }
            }
        }
        legend.push(DetectedLegend { symbol: String::new(), text: text.clone() });
    }

    // Also look at TEXT layer for abbreviation blocks.
    let text_layer: Vec<&(String, Vec2, String, f64)> = texts.iter().filter(|(_, _, layer, _)| layer.eq_ignore_ascii_case("TEXT")).collect();

    for (text, _, _, _) in &text_layer {
        if let Some((left, right)) = text.split_once('=') {
            let left = left.trim().to_ascii_uppercase();
            let right = right.trim();
            let system = match right.to_ascii_uppercase() {
                r if r.contains("SUPPLY AIR") => Some("SA"),
                r if r.contains("RETURN AIR") => Some("RA"),
                r if r.contains("EXHAUST AIR") => Some("EA"),
                r if r.contains("OUTSIDE AIR") => Some("OA"),
                _ => None,
            };
            if let Some(sys) = system {
                abbr_map.insert(left, sys.to_string());
            }
        }
    }

    (legend, abbr_map)
}

// ---- Tag attachment --------------------------------------------------------

/// The system a text names and how: a code printed outright (`SA`) is the tag's own; one read
/// through the sheet's abbreviation list is the legend's.
fn system_of(text: &str, abbr_map: &BTreeMap<String, String>) -> Option<(String, Source)> {
    let up = text.to_ascii_uppercase();
    if ["SA", "RA", "EA", "OA"].iter().any(|c| up.contains(c)) {
        return extract_system(text, &BTreeMap::new()).map(|c| (c, Source::Tag));
    }
    extract_system(text, abbr_map).map(|c| (c, Source::Legend))
}

/// Distance from `p` to a run's wall (0 inside the duct).
fn wall_dist(p: Vec2, run: &DuctRun) -> f64 {
    let d = run.points.windows(2).filter_map(|w| Some(seg_dist(p, *w.first()?, *w.get(1)?))).fold(f64::INFINITY, f64::min);
    (d - run.width_in / 2.0).max(0.0)
}

/// The far end of a leader drawn from a tag: a `TEXT` line starting at the text (within its
/// length) and running away from it.
fn leader_target(at: Vec2, text: &str, h: f64, lines: &[&LineSeg]) -> Option<Vec2> {
    let reach = text.chars().count() as f64 * h * 0.9 + h * 2.0;
    lines
        .iter()
        .filter_map(|l| {
            let (near, far) = if l.a.dist(at) <= l.b.dist(at) { (l.a, l.b) } else { (l.b, l.a) };
            (near.dist(at) < reach && far.dist(at) > near.dist(at) + h).then_some((near.dist(at), far))
        })
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .map(|(_, far)| far)
}

/// Attach size/height/system tags to their duct runs.  A size tag goes to the nearest run whose
/// wall gap agrees with it (a run drawn only as a centreline agrees with any); a tag no
/// compatible run is near stays unattached rather than resizing the wrong duct.
fn attach_tags(runs: &mut [DuctRun], d: &Drawing, segs: &[LineSeg], abbr_map: &BTreeMap<String, String>) -> Vec<DetectedTag> {
    let texts = text_entities(d);
    let text_lines: Vec<&LineSeg> = segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("TEXT")).collect();
    let mut detected_tags: Vec<DetectedTag> = Vec::new();
    let mut tag_dist = vec![f64::INFINITY; runs.len()];
    // Height notes wait for the size tags (their run is the size tag's beside them).
    let mut heights: Vec<(String, Vec2, f64, Option<Vec2>)> = Vec::new();

    for (text, at, layer, h) in texts.iter().take(MAX_SEGS) {
        if !layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        let h = if h.is_finite() && *h > 0.0 { *h } else { 6.0 };
        let size = parse_size(text);
        let system = if text.chars().count() <= 24 { system_of(text, abbr_map) } else { None };
        let up = text.to_ascii_uppercase();
        let is_height = ["BOT", "BOD", "B.O.", "CL ", "CL@", "TOP", "TOD", "T.O.", "EL", "AFF", "@"].iter().any(|k| up.contains(k));
        if size.is_none() && system.is_none() && !is_height {
            continue;
        }
        let leader = leader_target(*at, text, h, &text_lines);
        let target = leader.unwrap_or(*at);
        // A tag sits inside its duct or beside it (past the wall by about a text block); on a
        // leader, its end touches the wall.
        let reach = if leader.is_some() { h.max(12.0) } else { (h * 5.0).max(48.0) };
        let tag_index = detected_tags.len();
        detected_tags.push(DetectedTag {
            text: text.clone(),
            at: *at,
            size_in: size,
            system: system.as_ref().map(|s| s.0.clone()),
            has_leader: leader.is_some(),
            run: None,
        });

        if let Some([w, dpt]) = size {
            let best = runs
                .iter()
                .enumerate()
                .filter(|(_, r)| r.gap_in.is_none_or(|g| (g - w).abs() <= (w * 0.1).max(1.5)))
                .map(|(i, r)| (i, wall_dist(target, r)))
                .filter(|(_, dd)| *dd < reach)
                .min_by(|x, y| x.1.total_cmp(&y.1));
            if let Some((ri, dd)) = best
                && dd < tag_dist.get(ri).copied().unwrap_or(f64::INFINITY)
                && let Some(run) = runs.get_mut(ri)
            {
                if let Some(td) = tag_dist.get_mut(ri) {
                    *td = dd;
                }
                run.width_in = w;
                if dpt > 0.0 {
                    run.depth_in = dpt;
                    run.shape = "rect".into();
                } else {
                    run.depth_in = 0.0;
                    run.shape = "round".into();
                }
                run.size = Provenance::read(Source::Tag);
                run.depth = Provenance::read(Source::Tag);
                run.tag = Some(tag_index);
                run.confidence = run.confidence.max(0.85);
                if let Some(t) = detected_tags.get_mut(tag_index) {
                    t.run = Some(ri);
                }
            }
        } else if let Some((code, src)) = &system {
            let best =
                runs.iter().enumerate().map(|(i, r)| (i, wall_dist(target, r))).filter(|(_, dd)| *dd < reach).min_by(|x, y| x.1.total_cmp(&y.1));
            if let Some((ri, _)) = best
                && let Some(run) = runs.get_mut(ri)
            {
                run.system = Some(code.clone());
                run.system_from = Provenance::read(*src);
                run.confidence = run.confidence.max(0.8);
                if let Some(t) = detected_tags.get_mut(tag_index) {
                    t.run = Some(ri);
                }
            }
        }
        if size.is_some()
            && let Some((code, src)) = &system
            && let Some(ri) = detected_tags.get(tag_index).and_then(|t| t.run)
            && let Some(run) = runs.get_mut(ri)
        {
            run.system = Some(code.clone());
            run.system_from = Provenance::read(*src);
        }
        if is_height && size.is_none() {
            heights.push((text.clone(), *at, h, leader));
        }
    }

    // Heights: with the size tag printed beside them, else the nearest run.
    for (text, at, h, leader) in heights {
        let by_tag = detected_tags
            .iter()
            .filter(|t| t.size_in.is_some() && t.run.is_some() && t.at.dist(at) < h * 3.5)
            .min_by(|x, y| x.at.dist(at).total_cmp(&y.at.dist(at)))
            .and_then(|t| t.run);
        let target = leader.unwrap_or(at);
        let reach = (h * 5.0).max(48.0);
        let ri = by_tag.or_else(|| {
            runs.iter()
                .enumerate()
                .map(|(i, r)| (i, wall_dist(target, r)))
                .filter(|(_, dd)| *dd < reach)
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .map(|(i, _)| i)
        });
        if let Some(run) = ri.and_then(|i| runs.get_mut(i)) {
            let section_ft = if run.shape == "rect" && run.depth_in > 0.0 { run.depth_in } else { run.width_in } / 12.0;
            if let Some(v) = parse_height(&text, section_ft) {
                run.bottom_ft = Some(v);
            }
        }
    }

    detected_tags
}

// ---- System inference from pipe labels in breaks ---------------------------

/// Infer system codes from labels drawn in breaks along runs (pipe-style `SA` labels).
fn infer_systems_from_labels(runs: &mut [DuctRun], d: &Drawing, abbr_map: &BTreeMap<String, String>) {
    let texts = text_entities(d);
    let reach = 20.0; // model inches — labels sit right on the centreline

    for (text, at, layer, _) in &texts {
        if !layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        let up = text.trim().to_ascii_uppercase();
        // System label: short text (1–3 chars, or a known abbreviation) near a run.
        let sys = if ["SA", "RA", "EA", "OA"].contains(&up.as_str()) {
            Some((up.clone(), Source::Tag))
        } else {
            abbr_map.get(&up).cloned().map(|c| (c, Source::Legend))
        };

        if let Some((code, src)) = sys {
            // Find the nearest run.
            let mut best_run: Option<usize> = None;
            let mut best_dist = f64::INFINITY;
            for (i, run) in runs.iter().enumerate() {
                if run.system.is_some() {
                    continue; // already has a system
                }
                for w in run.points.windows(2) {
                    if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
                        let d = seg_dist(*at, *a, *b);
                        if d < best_dist {
                            best_dist = d;
                            best_run = Some(i);
                        }
                    }
                }
            }
            if let Some(ri) = best_run.filter(|_| best_dist < reach)
                && let Some(run) = runs.get_mut(ri)
            {
                run.system = Some(code);
                run.system_from = Provenance::read(src);
            }
        }
    }
}

// ---- Network graph ---------------------------------------------------------

/// One end of a run: where it is, which way it points out of the run, its half-width.
#[derive(Clone, Debug)]
struct End {
    run: usize,
    at: Vec2,
    out: Vec2,
    hw: f64,
}

fn run_ends(runs: &[DuctRun]) -> Vec<End> {
    let mut ends = Vec::new();
    for (i, r) in runs.iter().enumerate() {
        let n = r.points.len();
        let (Some(&p0), Some(&p1), Some(&q0), Some(&q1)) = (r.points.first(), r.points.get(1), r.points.last(), r.points.get(n.saturating_sub(2)))
        else {
            continue;
        };
        if p0.dist(p1) < 1e-9 || q0.dist(q1) < 1e-9 {
            continue;
        }
        let hw = (r.width_in / 2.0).clamp(0.5, MAX_HALF_IN);
        ends.push(End { run: i, at: p0, out: (p0 - p1).normalized(), hw });
        ends.push(End { run: i, at: q0, out: (q0 - q1).normalized(), hw });
    }
    ends
}

/// Whether two run ends meet at one fitting: close (within the wider duct's width, as the walls
/// of a tee, elbow or transition leave them), facing each other or square, not side by side.
fn ends_join(e: &End, f: &End) -> bool {
    e.run != f.run
        && e.at.dist(f.at) <= e.hw.max(f.hw) * 2.0 + 8.0
        && e.out.dot(f.out) < 0.5
        && (f.at - e.at).dot(e.out) > -2.0
        && (e.at - f.at).dot(f.out) > -2.0
}

/// A run's end that meets another run part way along: the branch of a tee off a main drawn
/// straight through.  Returns (run, piece, distance along the piece).
fn tee_on(e: &End, runs: &[DuctRun]) -> Option<(usize, usize, f64)> {
    let mut best: Option<(usize, usize, f64, f64)> = None;
    for (ri, r) in runs.iter().enumerate() {
        if ri == e.run {
            continue;
        }
        let hw = r.width_in / 2.0;
        for (wi, w) in r.points.windows(2).enumerate() {
            let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else { continue };
            let len = a.dist(b);
            if len < 1e-9 {
                continue;
            }
            let u = (b - a) * (1.0 / len);
            let t = (e.at - a).dot(u);
            if t < e.hw + 1.0 || t > len - e.hw - 1.0 || e.out.dot(u).abs() > 0.5 {
                continue;
            }
            let foot = a + u * t;
            let lateral = e.at.dist(foot);
            if lateral > hw + 6.0 || (foot - e.at).dot(e.out) < -2.0 {
                continue;
            }
            if best.is_none_or(|x| lateral < x.3) {
                best = Some((ri, wi, t, lateral));
            }
        }
    }
    best.map(|(r, w, t, _)| (r, w, t))
}

/// Build the network: run ends that meet become one node, a branch meeting a main part way
/// splits it at a tee, and the free ends link to the terminals and boxes they reach.
fn build_graph(runs: &[DuctRun], terms: &[DetectedTerminal], equip: &[DetectedEquipment]) -> DuctGraph {
    let ends = run_ends(runs);
    let mut order: Vec<usize> = (0..ends.len()).collect();
    order.sort_by(|&i, &j| ends.get(i).map_or(0.0, |e| e.at.x).total_cmp(&ends.get(j).map_or(0.0, |e| e.at.x)));
    let window = MAX_HALF_IN * 2.0 + 8.0;
    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        let Some(e) = ends.get(i) else { continue };
        for &j in order.iter().skip(k + 1) {
            let Some(f) = ends.get(j) else { continue };
            if f.at.x - e.at.x > window {
                break;
            }
            if ends_join(e, f) {
                candidates.push((e.at.dist(f.at), i, j));
            }
        }
    }
    // Nearest first, and a group grows only by ends that meet every end already in it: two tees
    // and the transition between them stay three nodes, not one.
    candidates.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut group: Vec<usize> = (0..ends.len()).collect();
    let mut members: BTreeMap<usize, Vec<usize>> = (0..ends.len()).map(|i| (i, vec![i])).collect();
    for (_, i, j) in candidates {
        let (Some(&gi), Some(&gj)) = (group.get(i), group.get(j)) else { continue };
        if gi == gj {
            continue;
        }
        let (Some(ma), Some(mb)) = (members.get(&gi), members.get(&gj)) else { continue };
        let all = ma.iter().all(|&x| mb.iter().all(|&y| matches!((ends.get(x), ends.get(y)), (Some(e), Some(f)) if ends_join(e, f))));
        if !all {
            continue;
        }
        let moved = members.remove(&gj).unwrap_or_default();
        for &y in &moved {
            if let Some(g) = group.get_mut(y) {
                *g = gi;
            }
        }
        if let Some(ma) = members.get_mut(&gi) {
            ma.extend(moved);
        }
    }
    let mut g = DuctGraph::default();
    let mut node_of_group: BTreeMap<usize, usize> = BTreeMap::new();
    for (root, m) in &members {
        let at = m.iter().filter_map(|i| ends.get(*i)).fold(Vec2::new(0.0, 0.0), |acc, e| acc + e.at) * (1.0 / m.len().max(1) as f64);
        node_of_group.insert(*root, g.nodes.len());
        g.nodes.push(GraphNode { kind: "end".into(), at, terminal: None, equipment: None });
    }
    let mut end_node: Vec<usize> = group.iter().map(|gr| node_of_group.get(gr).copied().unwrap_or(0)).collect();

    // Tees on mains drawn straight through.
    let mut splits: BTreeMap<usize, Vec<(usize, f64, usize)>> = BTreeMap::new();
    for m in members.values() {
        let [i] = m.as_slice() else { continue };
        let Some(e) = ends.get(*i) else { continue };
        let Some((ri, wi, t)) = tee_on(e, runs) else { continue };
        let list = splits.entry(ri).or_default();
        let node = match list.iter().find(|(w, tt, _)| *w == wi && (tt - t).abs() < e.hw * 2.0 + 4.0) {
            Some((_, _, n)) => *n,
            None => {
                let Some(r) = runs.get(ri) else { continue };
                let (Some(&a), Some(&b)) = (r.points.get(wi), r.points.get(wi + 1)) else { continue };
                let at = a + (b - a).normalized() * t;
                g.nodes.push(GraphNode { kind: "end".into(), at, terminal: None, equipment: None });
                list.push((wi, t, g.nodes.len() - 1));
                g.nodes.len() - 1
            }
        };
        // The lone end's own node folds into the tee (left without edges).
        if let Some(slot) = end_node.get_mut(*i) {
            *slot = node;
        }
    }

    // Edges: every run, cut at its tees.
    let mut run_nodes: Vec<Option<[usize; 2]>> = vec![None; runs.len()];
    for (k, e) in ends.iter().enumerate() {
        if let (Some(slot), Some(&n)) = (run_nodes.get_mut(e.run), end_node.get(k)) {
            match slot {
                None => *slot = Some([n, n]),
                Some(pair) => pair[1] = n,
            }
        }
    }
    for (ri, r) in runs.iter().enumerate() {
        let Some(Some([start, end])) = run_nodes.get(ri).copied() else { continue };
        let mut ss = splits.get(&ri).cloned().unwrap_or_default();
        ss.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)));
        let Some(&p0) = r.points.first() else { continue };
        let (mut cur, mut pts) = (start, vec![p0]);
        for (wi, w) in r.points.windows(2).enumerate() {
            let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else { continue };
            let u = (b - a).normalized();
            for (_, t, node) in ss.iter().filter(|x| x.0 == wi) {
                let q = a + u * *t;
                pts.push(q);
                g.edges.push(edge(cur, *node, Some(ri), std::mem::replace(&mut pts, vec![q]), r));
                cur = *node;
            }
            pts.push(b);
        }
        g.edges.push(edge(cur, end, Some(ri), pts, r));
    }

    // Boxes at the free ends inside or beside them, then each terminal at the nearest node (a
    // runout's end, or in line on a branch where the drop leaves).
    let deg = g.degrees();
    let mut taken = vec![false; g.nodes.len()];
    let free = |n: usize, taken: &[bool]| deg.get(n) == Some(&1) && !taken.get(n).copied().unwrap_or(true);
    for (ei, eq) in equip.iter().enumerate() {
        let m = 18.0;
        let inside = |p: Vec2| p.x > eq.min.x - m && p.x < eq.max.x + m && p.y > eq.min.y - m && p.y < eq.max.y + m;
        let hits: Vec<usize> = (0..deg.len()).filter(|&n| free(n, &taken) && g.nodes.get(n).is_some_and(|x| inside(x.at))).collect();
        if hits.is_empty() {
            continue;
        }
        g.nodes.push(GraphNode { kind: "equipment".into(), at: (eq.min + eq.max) * 0.5, terminal: None, equipment: Some(ei) });
        let en = g.nodes.len() - 1;
        for n in hits {
            if let Some(t) = taken.get_mut(n) {
                *t = true;
            }
            g.edges.push(GraphEdge {
                a: n,
                b: en,
                run: None,
                points: Vec::new(),
                size_in: [0.0, 0.0],
                shape: String::new(),
                system: None,
                confidence: eq.confidence,
            });
        }
    }
    for (ti, tm) in terms.iter().enumerate() {
        let best = (0..deg.len())
            .filter(|&n| deg.get(n).is_some_and(|d| *d > 0) && !taken.get(n).copied().unwrap_or(true))
            .filter_map(|n| Some((n, g.nodes.get(n)?.at.dist(tm.at))))
            .filter(|(_, d)| *d < 36.0)
            .min_by(|x, y| x.1.total_cmp(&y.1));
        let Some((n, _)) = best else { continue };
        if let Some(t) = taken.get_mut(n) {
            *t = true;
        }
        g.nodes.push(GraphNode { kind: "terminal".into(), at: tm.at, terminal: Some(ti), equipment: None });
        let tn = g.nodes.len() - 1;
        g.edges.push(GraphEdge {
            a: n,
            b: tn,
            run: None,
            points: Vec::new(),
            size_in: [0.0, 0.0],
            shape: String::new(),
            system: None,
            confidence: tm.confidence,
        });
    }
    classify_nodes(&mut g, runs);
    g
}

fn edge(a: usize, b: usize, run: Option<usize>, points: Vec<Vec2>, r: &DuctRun) -> GraphEdge {
    GraphEdge { a, b, run, points, size_in: r.size_in(), shape: r.shape.clone(), system: r.system.clone(), confidence: r.confidence }
}

/// The direction an edge leaves a node.
fn leaving(e: &GraphEdge, n: usize) -> Option<Vec2> {
    let k = e.points.len();
    let (p, q) = if e.a == n { (e.points.first()?, e.points.get(1)?) } else { (e.points.last()?, e.points.get(k.checked_sub(2)?)?) };
    (p.dist(*q) > 1e-9).then(|| (*q - *p).normalized())
}

/// Name each duct node by what meets there.
fn classify_nodes(g: &mut DuctGraph, runs: &[DuctRun]) {
    let deg = g.degrees();
    let mut at: Vec<Vec<usize>> = vec![Vec::new(); g.nodes.len()];
    for (k, e) in g.edges.iter().enumerate().filter(|(_, e)| e.run.is_some()) {
        for n in [e.a, e.b] {
            if let Some(v) = at.get_mut(n) {
                v.push(k);
            }
        }
    }
    for (n, node) in g.nodes.iter_mut().enumerate() {
        if node.terminal.is_some() || node.equipment.is_some() {
            continue;
        }
        node.kind = match deg.get(n).copied().unwrap_or(0) {
            0 | 1 => "end".into(),
            2 => {
                let es: Vec<&GraphEdge> = at.get(n).map(|v| v.iter().filter_map(|k| g.edges.get(*k)).collect()).unwrap_or_default();
                let (Some(e1), Some(e2)) = (es.first(), es.get(1)) else { continue };
                let w = |e: &GraphEdge| e.run.and_then(|r| runs.get(r)).map_or(0.0, |r| r.width_in);
                let round = |e: &GraphEdge| e.run.and_then(|r| runs.get(r)).is_some_and(|r| r.shape == "round");
                if (w(e1) - w(e2)).abs() > 1.0 {
                    if round(e1) && round(e2) { "reducer".into() } else { "transition".into() }
                } else {
                    match (leaving(e1, n), leaving(e2, n)) {
                        (Some(d1), Some(d2)) if d1.dot(d2) > -0.94 => "elbow".into(),
                        _ => "joint".into(),
                    }
                }
            }
            3 => "tee".into(),
            _ => "cross".into(),
        };
    }
}

/// Runs meeting at each duct node (not across terminals or boxes): neighbours for inference.
fn run_neighbours(g: &DuctGraph, n_runs: usize) -> Vec<Vec<(usize, bool)>> {
    let mut at: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for e in &g.edges {
        if let Some(r) = e.run {
            at.entry(e.a).or_default().push(r);
            at.entry(e.b).or_default().push(r);
        }
    }
    let mut nb: Vec<Vec<(usize, bool)>> = vec![Vec::new(); n_runs];
    for (n, rs) in &at {
        // A size change: the runs either side are not the same duct.
        let change = g.nodes.get(*n).is_some_and(|x| x.kind == "transition" || x.kind == "reducer");
        for &a in rs {
            for &b in rs {
                if a != b
                    && let Some(v) = nb.get_mut(a)
                    && !v.contains(&(b, change))
                {
                    v.push((b, change));
                }
            }
        }
    }
    nb
}

// ---- Size and depth inference ----------------------------------------------

/// Snap a measured wall gap (inches) to a nominal duct size: duct sizes come in whole even
/// inches, 4" and up.
fn snap_width(w: f64) -> f64 {
    ((w / 2.0).round() * 2.0).max(4.0)
}

/// Sizes for runs no tag sized.  A run joined (not across a size change) to a tagged run of the
/// same wall gap is the same duct: it takes that tag's size (`inferred` from `proximity`).  Else
/// the gap gives the width (`inferred` from `geometry`, a guess); a rectangular depth comes from
/// the nearest connected tagged duct (`inferred`), or is the stock 12" (`defaulted`).
fn infer_depths_and_snap(runs: &mut [DuctRun], g: &DuctGraph) {
    let nb = run_neighbours(g, runs.len());
    let tagged: Vec<bool> = runs.iter().map(|r| r.size.source == Source::Tag).collect();
    let width_of = |r: &DuctRun| r.gap_in.unwrap_or(r.width_in);
    let mut updates: Vec<(usize, Option<usize>, Option<usize>)> = Vec::new();
    for i in 0..runs.len() {
        if tagged.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(g0) = runs.get(i).and_then(|r| r.gap_in) else {
            updates.push((i, None, None));
            continue;
        };
        // Same duct: breadth-first through same-gap runs, no size change.
        let mut same = None;
        let mut seen = vec![false; runs.len()];
        let mut q = VecDeque::from([(i, 0usize)]);
        if let Some(s) = seen.get_mut(i) {
            *s = true;
        }
        while let Some((u, hops)) = q.pop_front() {
            if u != i && tagged.get(u).copied().unwrap_or(false) {
                same = Some(u);
                break;
            }
            if hops >= 6 {
                continue;
            }
            for &(v, change) in nb.get(u).map(Vec::as_slice).unwrap_or_default() {
                let ok = !change && runs.get(v).is_some_and(|r| (width_of(r) - g0).abs() <= 1.0);
                if ok && !seen.get(v).copied().unwrap_or(true) {
                    if let Some(s) = seen.get_mut(v) {
                        *s = true;
                    }
                    q.push_back((v, hops + 1));
                }
            }
        }
        // Depth only: the nearest connected tagged rectangular duct.
        let mut deep = None;
        if same.is_none() {
            let mut seen = vec![false; runs.len()];
            let mut q = VecDeque::from([(i, 0usize)]);
            if let Some(s) = seen.get_mut(i) {
                *s = true;
            }
            while let Some((u, hops)) = q.pop_front() {
                if u != i && tagged.get(u).copied().unwrap_or(false) && runs.get(u).is_some_and(|r| r.shape == "rect") {
                    deep = Some(u);
                    break;
                }
                if hops >= 6 {
                    continue;
                }
                for &(v, _) in nb.get(u).map(Vec::as_slice).unwrap_or_default() {
                    if !seen.get(v).copied().unwrap_or(true) {
                        if let Some(s) = seen.get_mut(v) {
                            *s = true;
                        }
                        q.push_back((v, hops + 1));
                    }
                }
            }
        }
        updates.push((i, same, deep));
    }
    for (i, same, deep) in updates {
        let from_same = same.and_then(|j| runs.get(j)).map(|r| (r.width_in, r.depth_in, r.shape.clone()));
        let from_deep = deep.and_then(|j| runs.get(j)).map(|r| r.depth_in);
        let Some(run) = runs.get_mut(i) else { continue };
        if let Some((w, dpt, shape)) = from_same {
            run.width_in = w;
            run.depth_in = dpt;
            run.shape = shape;
            run.size = Provenance::inferred(Source::Proximity);
            run.depth = Provenance::inferred(Source::Proximity);
            continue;
        }
        if run.gap_in.is_some() {
            run.width_in = snap_width(run.width_in);
        }
        if run.shape == "rect" && run.depth_in < 1.0 {
            if let Some(dpt) = from_deep {
                run.depth_in = dpt;
                run.depth = Provenance::inferred(Source::Proximity);
            } else {
                // The stock depth: square up to 12", else 12".
                run.depth_in = if run.width_in <= DEFAULT_DEPTH_IN { run.width_in } else { DEFAULT_DEPTH_IN };
                run.depth = Provenance::DEFAULTED;
            }
        }
    }
}

// ---- System inference over the network --------------------------------------

/// Systems for runs no label named: each part of the network between boxes takes the system its
/// terminals and boxes say (diffusers supply, return grilles return, exhaust grilles and fans
/// exhaust, a VAV box supply) and any run labelled in it.
fn infer_systems(runs: &mut [DuctRun], g: &DuctGraph, terms: &[DetectedTerminal], equip: &[DetectedEquipment]) {
    let comp = g.components(true);
    let mut votes: BTreeMap<usize, BTreeMap<&'static str, f64>> = BTreeMap::new();
    let code = |s: &str| -> Option<&'static str> { ["SA", "RA", "EA", "OA"].into_iter().find(|c| *c == s) };
    for e in &g.edges {
        let Some(&c) = comp.get(e.a) else { continue };
        if let Some(r) = e.run.and_then(|r| runs.get(r))
            && r.system_from.status == Status::Read
            && let Some(k) = r.system.as_deref().and_then(code)
        {
            *votes.entry(c).or_default().entry(k).or_default() += 3.0;
        }
        if e.run.is_none() {
            let (Some(na), Some(nb)) = (g.nodes.get(e.a), g.nodes.get(e.b)) else { continue };
            // The duct side of the link carries the component.
            let (duct, other) = if na.terminal.is_some() || na.equipment.is_some() { (e.b, na) } else { (e.a, nb) };
            let Some(&c) = comp.get(duct) else { continue };
            let vote = if let Some(t) = other.terminal.and_then(|t| terms.get(t)) {
                match t.kind.as_str() {
                    "supply_diffuser" => Some(("SA", 1.0)),
                    "return_grille" => Some(("RA", 1.0)),
                    "exhaust_grille" => Some(("EA", 1.0)),
                    _ => None,
                }
            } else if let Some(q) = other.equipment.and_then(|q| equip.get(q)) {
                match q.kind.as_str() {
                    "vav" => Some(("SA", 2.0)),
                    "exhaust_fan" => Some(("EA", 2.0)),
                    _ => None,
                }
            } else {
                None
            };
            if let Some((k, w)) = vote {
                *votes.entry(c).or_default().entry(k).or_default() += w;
            }
        }
    }
    let pick: BTreeMap<usize, &'static str> =
        votes.iter().filter_map(|(c, v)| v.iter().max_by(|x, y| x.1.total_cmp(y.1)).map(|(k, _)| (*c, *k))).collect();
    for e in &g.edges {
        let Some(r) = e.run.and_then(|r| runs.get_mut(r)) else { continue };
        if r.system_from.status == Status::Read {
            continue;
        }
        if let Some(k) = comp.get(e.a).and_then(|c| pick.get(c)) {
            r.system = Some((*k).to_string());
            r.system_from = Provenance::inferred(Source::Proximity);
        }
    }
}

// ---- System inference from equipment and terminals -------------------------

/// The fallback for runs the network left without a system: equipment (AHU/VAV→SA,
/// exhaust fan→EA) and terminals within reach, then along touching runs.
fn infer_systems_from_equipment(runs: &mut [DuctRun], equip: &[DetectedEquipment], terms: &[DetectedTerminal]) {
    let reach = 40.0; // model inches

    // Assign SA to runs near AHU/VAV equipment.
    for eq in equip {
        let center = (eq.min + eq.max) * 0.5;
        let sys = match eq.kind.as_str() {
            "ahu" | "vav" => "SA",
            "exhaust_fan" => "EA",
            _ => continue,
        };
        for run in runs.iter_mut() {
            if run.system.is_some() {
                continue;
            }
            let near = run.points.iter().any(|p| p.dist(center) < reach + (eq.max - eq.min).len());
            if near {
                run.system = Some(sys.to_string());
                run.system_from = Provenance::inferred(Source::Proximity);
            }
        }
    }

    // Assign from terminals: supply diffusers→SA, return grilles→RA, exhaust grilles→EA.
    for tm in terms {
        let sys = match tm.kind.as_str() {
            "supply_diffuser" => "SA",
            "return_grille" => "RA",
            "exhaust_grille" => "EA",
            _ => continue,
        };
        for run in runs.iter_mut() {
            if run.system.is_some() {
                continue;
            }
            let near = run.points.iter().any(|p| p.dist(tm.at) < reach);
            if near {
                run.system = Some(sys.to_string());
                run.system_from = Provenance::inferred(Source::Proximity);
            }
        }
    }

    // Propagate: runs touching a system-assigned run get the same system.
    let tol = 5.0;
    let mut changed = true;
    let mut iterations = 0;
    while changed && iterations < 20 {
        changed = false;
        iterations += 1;
        // Collect known system endpoints.
        let known: Vec<(Vec2, String)> = runs
            .iter()
            .filter_map(|r| {
                let sys = r.system.as_ref()?;
                let mut pts = Vec::new();
                if let Some(f) = r.points.first() {
                    pts.push((*f, sys.clone()));
                }
                if let Some(l) = r.points.last() {
                    pts.push((*l, sys.clone()));
                }
                Some(pts)
            })
            .flatten()
            .collect();
        for run in runs.iter_mut() {
            if run.system.is_some() {
                continue;
            }
            for (p, sys) in &known {
                let near = run.points.first().is_some_and(|f| f.dist(*p) < tol) || run.points.last().is_some_and(|l| l.dist(*p) < tol);
                if near {
                    run.system = Some(sys.clone());
                    run.system_from = Provenance::inferred(Source::Proximity);
                    changed = true;
                    break;
                }
            }
        }
    }
}

// ---- Centreline fallback ---------------------------------------------------

/// The gap between the `DUCT` walls either side of a centreline piece: parallel lines overlapping
/// it, at equal offsets left and right.
fn walls_beside(cs: &LineSeg, duct: &[&LineSeg]) -> Option<f64> {
    let len = cs.a.dist(cs.b);
    if len < 1e-9 {
        return None;
    }
    let u = (cs.b - cs.a) * (1.0 / len);
    let n = Vec2::new(-u.y, u.x);
    let (mut left, mut right): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());
    for w in duct.iter().take(MAX_SEGS) {
        let d = w.b - w.a;
        let l = d.len();
        if l < 1e-9 || (d * (1.0 / l)).cross(u).abs() > 0.03 {
            continue;
        }
        let (s0, s1) = minmax((w.a - cs.a).dot(u), (w.b - cs.a).dot(u));
        if s1.min(len) - s0.max(0.0) < 1.0 {
            continue;
        }
        let off = (w.a - cs.a).dot(n);
        if (MIN_HALF_IN..=MAX_HALF_IN).contains(&off) {
            left.push(off);
        } else if (MIN_HALF_IN..=MAX_HALF_IN).contains(&-off) {
            right.push(-off);
        }
    }
    // The nearest matching pair (an inner duct's walls before a wider one's).
    let mut best: Option<f64> = None;
    for l in &left {
        for r in &right {
            if (l - r).abs() < 1.0 && best.is_none_or(|b| l + r < b) {
                best = Some(l + r);
            }
        }
    }
    best
}

/// Add runs from CENTRE-layer lines not already covered by a parallel-pair run.
fn add_centreline_runs(runs: &mut Vec<DuctRun>, segs: &[LineSeg]) {
    let centre_segs: Vec<&LineSeg> =
        segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("CENTRE") || s.layer.eq_ignore_ascii_case("CENTER")).collect();
    if centre_segs.is_empty() {
        return;
    }

    let duct_segs: Vec<&LineSeg> = segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("DUCT")).collect();
    let cover_tol = 5.0; // model inches
    for cs in centre_segs.iter().take(MAX_RUNS) {
        // Skip if this centreline is already covered by an existing run.
        let mid = (cs.a + cs.b) * 0.5;
        let covered = runs.iter().any(|r| {
            r.points.windows(2).any(|w| if let (Some(a), Some(b)) = (w.first(), w.get(1)) { seg_dist(mid, *a, *b) < cover_tol } else { false })
        });
        if covered {
            continue;
        }

        // Its width: the duct walls beside it, one each side at the same offset (pieces too
        // short to pair, between tees and transitions); else the default (until a tag sizes it).
        runs.push(DuctRun::new(vec![cs.a, cs.b], walls_beside(cs, &duct_segs), 0.5));
        if runs.len() >= MAX_RUNS {
            break;
        }
    }
}

// ---- Main reader entry point -----------------------------------------------

/// Read the ductwork from a mechanical-plan DXF drawing (model inches; nothing about the
/// sheet's placement is needed or used).
pub fn read_ducts(d: &Drawing) -> DuctNetwork {
    read_ducts_with(d, &DuctOptions::default())
}

/// [`read_ducts`] with options.
pub fn read_ducts_with(d: &Drawing, o: &DuctOptions) -> DuctNetwork {
    // 1. Find parallel line pairs on DUCT layer, and radius elbows (concentric arc pairs).
    let mut segs = line_segments(d);
    // Turn markers are symbols, not walls.
    let boxes = marker_boxes(d, &segs);
    if !boxes.is_empty() {
        segs.retain(|s| !(s.layer.eq_ignore_ascii_case("DUCT") && in_marker(s, &boxes)));
    }
    let mut pairs = duct_pairs(&segs);
    let (elbow_pairs, mut fittings) = radius_elbows(&arcs(d));
    pairs.extend(elbow_pairs.into_iter().take(MAX_PAIRS.saturating_sub(pairs.len())));

    // 2. Chain pairs into runs.
    let mut runs = chain_runs(&pairs, o);

    // 2b. Add centreline-based runs that aren't covered by pairs.
    add_centreline_runs(&mut runs, &segs);

    // 3. Read legend and abbreviations.
    let (legend, abbr_map) = read_legend(d);

    // 4. Attach size/height/system tags.
    let tags = attach_tags(&mut runs, d, &segs, &abbr_map);

    // 5. Infer systems from labels in run breaks.
    infer_systems_from_labels(&mut runs, d, &abbr_map);

    // 6. Detect fittings drawn as symbols, terminals, equipment.
    fittings.extend(detect_fittings(&boxes));
    let terminals = detect_terminals(d);
    let equipment = detect_equipment(d);

    // 7. The network: run ends, tees, terminals, boxes.
    let mut graph = build_graph(&runs, &terminals, &equipment);

    // 8. Sizes and depths for the runs no tag sized; systems over the network, then nearby.
    infer_depths_and_snap(&mut runs, &graph);
    infer_systems(&mut runs, &graph, &terminals, &equipment);
    infer_systems_from_equipment(&mut runs, &equipment, &terminals);

    // 9. The edges carry the final sizes and systems; the network's fittings join the list.
    classify_nodes(&mut graph, &runs);
    for e in &mut graph.edges {
        if let Some(r) = e.run.and_then(|r| runs.get(r)) {
            e.size_in = r.size_in();
            e.shape = r.shape.clone();
            e.system = r.system.clone();
            e.confidence = r.confidence;
        }
    }
    for n in &graph.nodes {
        if !["elbow", "tee", "cross", "transition", "reducer"].contains(&n.kind.as_str()) {
            continue;
        }
        if n.kind == "elbow" && fittings.iter().any(|f| f.kind == "elbow" && f.at.dist(n.at) < 24.0) {
            continue;
        }
        fittings.push(DetectedFitting { kind: n.kind.clone(), at: n.at, confidence: 0.6, source: Source::Geometry, status: Status::Inferred });
    }

    let abbreviations = abbr_map;
    DuctNetwork { runs, fittings, terminals, equipment, tags, legend, abbreviations, graph }
}

/// Read ducts and score against truth for a single mechanical sheet, placed with the alignment
/// [`crate::sheet_reader`] read for it (a sheet it couldn't align scores nothing found).
pub fn read_and_score(d: &Drawing, t: &Truth, sh: &RecognisedSheet) -> score::RunScore {
    read_and_score_with(d, t, sh, &DuctOptions::default())
}

/// [`read_and_score`] with options.
pub fn read_and_score_with(d: &Drawing, t: &Truth, sh: &RecognisedSheet, o: &DuctOptions) -> score::RunScore {
    let level = sh.level.clone().unwrap_or_default();
    let found = match &sh.alignment {
        Some(al) => read_ducts_with(d, o).to_found_runs(&level, al),
        None => Vec::new(),
    };
    score::score_runs(t, &level, "duct", &found, MATCH_TOL_FT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_rectangular() {
        assert_eq!(parse_size("24x12"), Some([24.0, 12.0]));
        assert_eq!(parse_size("24\"x12\""), Some([24.0, 12.0]));
        assert_eq!(parse_size("36x18 BOT @ 10'-6\""), Some([36.0, 18.0]));
    }

    #[test]
    fn parse_size_round() {
        assert_eq!(parse_size("10\"Ø"), Some([10.0, 0.0]));
        assert_eq!(parse_size("12\" DIA"), Some([12.0, 0.0]));
        assert_eq!(parse_size("8\"Ø"), Some([8.0, 0.0]));
    }

    #[test]
    fn parse_height_tags() {
        assert_eq!(parse_height("BOT @ 10'-0\"", 1.0), Some(10.0));
        assert_eq!(parse_height("CL @ 10'-6\"", 1.0), Some(10.0));
        assert_eq!(parse_height("TOP @ 11'-0\"", 1.0), Some(10.0));
        assert!(parse_height("24x12", 1.0).is_none());
    }

    #[test]
    fn parse_feet_inches_values() {
        assert_eq!(parse_feet_inches("10'-6\""), Some(126.0));
        assert_eq!(parse_feet_inches("9'-0\""), Some(108.0));
        assert_eq!(parse_feet_inches("12'-0\""), Some(144.0));
    }

    #[test]
    fn chaining_skips_used_pairs_instead_of_stopping() {
        // Pair 1 is chained into pair 0's run; pair 2 is another duct and must still be found
        // (it was lost when the loop stopped at the first used pair). Pair 3 lies along the
        // first duct again (its walls paired another way) and is not a second run.
        let p = |a: (f64, f64), b: (f64, f64)| DuctPair { c0: Vec2::new(a.0, a.1), c1: Vec2::new(b.0, b.1), half_w: 6.0 };
        let pairs = [p((0.0, 0.0), (100.0, 0.0)), p((100.0, 0.0), (200.0, 0.0)), p((0.0, 300.0), (150.0, 300.0)), p((20.0, 1.0), (180.0, 1.0))];
        let runs = chain_runs(&pairs, &DuctOptions { all_pairs: true });
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert!(runs.iter().any(|r| r.points.iter().all(|q| (q.y - 300.0).abs() < 1e-9)));
        // Skipping is the default; the old stop-at-the-used-pair loses the second duct.
        assert_eq!(chain_runs(&pairs, &DuctOptions::default()).len(), 2);
        assert_eq!(chain_runs(&pairs, &DuctOptions { all_pairs: false }).len(), 1);
    }

    #[test]
    fn system_extraction() {
        let mut map = BTreeMap::new();
        map.insert("S".into(), "SA".into());
        assert_eq!(extract_system("SA 24x12", &map), Some("SA".into()));
        assert_eq!(extract_system("RA 24x12", &map), Some("RA".into()));
        assert_eq!(extract_system("S 24x12", &map), Some("SA".into()));
    }

    /// A drawing of `DUCT` lines, `DUCT` arcs (centre, radius, start, end) and `TEXT`.
    fn sheet(lines: &[((f64, f64), (f64, f64))], arcs: &[((f64, f64), f64, f64, f64)], texts: &[((f64, f64), &str)]) -> Drawing {
        use cadaddy_doc::{Arc, Common, HAlign, Line, Space, Text, VAlign};
        use cadaddy_geom::Vec3;
        let mut d = Drawing::new_imperial();
        let on = |layer: &str| Common { layer: layer.into(), ..Common::default() };
        for (a, b) in lines {
            let _ = d.add(&Space::Model, on("DUCT"), EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) }));
        }
        for (c, r, start, end) in arcs {
            let _ = d.add(&Space::Model, on("DUCT"), EntityKind::Arc(Arc { center: Vec3::new(c.0, c.1, 0.0), radius: *r, start: *start, end: *end }));
        }
        for (at, v) in texts {
            let p = Vec3::new(at.0, at.1, 0.0);
            let text = Text {
                insert: p,
                align_pt: Some(p),
                height: 6.0,
                value: (*v).into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: HAlign::Center,
                valign: VAlign::Middle,
            };
            let _ = d.add(&Space::Model, on("TEXT"), EntityKind::Text(text));
        }
        d
    }

    #[test]
    fn height_notes_and_words_are_not_sizes() {
        assert_eq!(parse_size("TOD @ 11'-10\""), None);
        assert_eq!(parse_size("B.O.D. 11'-6\" AFF"), None);
        assert_eq!(parse_size("BOT @ 10'-0\""), None);
        assert_eq!(parse_size("24/12"), Some([24.0, 12.0]));
        assert_eq!(parse_size("EXHAUST 24x12"), Some([24.0, 12.0]));
        assert_eq!(parse_size("12Ø"), Some([12.0, 0.0]));
        assert_eq!(parse_size("1/2"), None);
    }

    #[test]
    fn untagged_depth_is_defaulted_and_tagged_size_is_read() {
        // A 24" duct with no tag, and an 18" one far away tagged 18x10 inside it.
        let d = sheet(
            &[((0.0, 0.0), (240.0, 0.0)), ((0.0, 24.0), (240.0, 24.0)), ((0.0, 1000.0), (240.0, 1000.0)), ((0.0, 1018.0), (240.0, 1018.0))],
            &[],
            &[((120.0, 1009.0), "18x10")],
        );
        let net = read_ducts(&d);
        assert_eq!(net.runs.len(), 2, "{:?}", net.runs);
        let plain = net.runs.iter().find(|r| r.points.iter().all(|p| p.y < 100.0)).expect("untagged run");
        assert_eq!(plain.width_in, 24.0);
        assert_eq!(plain.size, Provenance::inferred(Source::Geometry), "the gap width is a guess");
        assert_eq!(plain.depth_in, DEFAULT_DEPTH_IN);
        assert_eq!(plain.depth, Provenance::DEFAULTED);
        assert_eq!(plain.status(), Status::Defaulted);
        let tagged = net.runs.iter().find(|r| r.points.iter().all(|p| p.y > 900.0)).expect("tagged run");
        assert_eq!(tagged.size_in(), [18.0, 10.0]);
        assert_eq!(tagged.size, Provenance::read(Source::Tag));
        assert_eq!(tagged.status(), Status::Read);
        assert!(tagged.tag.is_some());
    }

    #[test]
    fn a_tag_never_resizes_a_duct_of_another_width() {
        // A 10x8 tag beside a 30" duct: no compatible duct, so it stays unattached.
        let d = sheet(&[((0.0, 0.0), (240.0, 0.0)), ((0.0, 30.0), (240.0, 30.0))], &[], &[((120.0, 50.0), "10x8")]);
        let net = read_ducts(&d);
        assert_eq!(net.runs.len(), 1);
        assert!(net.runs.iter().all(|r| r.size.source != Source::Tag && r.width_in == 30.0));
        assert!(net.tags.iter().all(|t| t.run.is_none()));
    }

    #[test]
    fn radius_elbow_reads_as_a_turn_through_its_corner() {
        use std::f64::consts::FRAC_PI_2;
        let a = [
            ArcSeg { c: Vec2::new(0.0, 0.0), r: 24.0, start: 0.0, end: FRAC_PI_2, layer: "DUCT".into() },
            ArcSeg { c: Vec2::new(0.0, 0.0), r: 48.0, start: 0.0, end: FRAC_PI_2, layer: "DUCT".into() },
        ];
        let (pairs, fittings) = radius_elbows(&a);
        assert_eq!(pairs.len(), 2);
        let near = |p: Vec2, x: f64, y: f64| p.dist(Vec2::new(x, y)) < 1e-6;
        assert!(pairs.iter().any(|p| near(p.c0, 36.0, 0.0) && near(p.c1, 36.0, 36.0)));
        assert!(pairs.iter().any(|p| near(p.c0, 36.0, 36.0) && near(p.c1, 0.0, 36.0)));
        assert!(pairs.iter().all(|p| (p.half_w - 12.0).abs() < 1e-9));
        assert_eq!(fittings.len(), 1);
        assert!(fittings.iter().all(|f| f.kind == "elbow" && near(f.at, 36.0, 36.0) && f.status == Status::Read));
    }

    #[test]
    fn a_branch_off_a_straight_main_is_a_tee() {
        // A 24" main along x, a 24" branch north from its wall at x = 240.
        let d = sheet(
            &[((0.0, 0.0), (480.0, 0.0)), ((0.0, 24.0), (480.0, 24.0)), ((228.0, 24.0), (228.0, 200.0)), ((252.0, 24.0), (252.0, 200.0))],
            &[],
            &[],
        );
        let net = read_ducts(&d);
        assert_eq!(net.runs.len(), 2, "{:?}", net.runs);
        assert_eq!(net.graph.branches(), 1, "{:?}", net.graph.nodes);
        assert_eq!(net.graph.edges.iter().filter(|e| e.run.is_some()).count(), 3);
        assert!(net.fittings.iter().any(|f| f.kind == "tee"));
    }

    #[test]
    fn snapped_widths_are_whole_even_inches() {
        assert_eq!(snap_width(23.7), 24.0);
        assert_eq!(snap_width(68.2), 68.0);
        assert_eq!(snap_width(1.0), 4.0);
    }
}
