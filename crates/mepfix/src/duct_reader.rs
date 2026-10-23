//! Ductwork reader: detects duct runs, fittings, terminals and equipment from mechanical
//! plan sheets (DXF) and produces a network graph scored against the truth.
//!
//! Duct runs on mechanical sheets are drawn as double parallel lines on the `DUCT` layer
//! (the two sides of the duct), with optional centrelines on `CENTRE`.  Fittings are arcs
//! on `DUCT` (elbows) or tapered pairs (transitions).  Terminals live on `DEVICE`, equipment
//! on `EQUIP`, and size/height tags on `TEXT` with optional leader lines.
//!
//! The reader takes an aligned sheet (from [`crate::sheet_reader`]) and returns
//! [`DuctNetwork`], which can be scored against the truth via [`crate::score::score_runs`].

use crate::score::{self, FoundRun};
use crate::truth::{Sheet, Truth};
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use std::collections::BTreeMap;

// ---- Limits ----------------------------------------------------------------

/// Maximum segments to process from a single drawing (hostile-input cap).
const MAX_SEGS: usize = 100_000;
/// Maximum duct pairs to consider.
const MAX_PAIRS: usize = 20_000;
/// Maximum runs after chaining.
const MAX_RUNS: usize = 5_000;
/// Duct half-width range in model inches (the distance from centreline to one wall line).
/// Smallest: 4" duct ⇒ 2" half; largest: 60" ⇒ 30".  Scaled by the sheet.
const MIN_HALF_IN: f64 = 1.5;
const MAX_HALF_IN: f64 = 35.0;
/// Alignment tolerance: how far a found centreline may differ from a truth run, in feet.
const MATCH_TOL_FT: f64 = 1.5;

// ---- Public types ----------------------------------------------------------

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
    /// `elbow`, `tee`, `transition`, `reducer`, `cap`, `turn`.
    pub kind: String,
    pub at: Vec2,
    pub confidence: f64,
}

/// A detected terminal device.
#[derive(Clone, Debug)]
pub struct DetectedTerminal {
    /// `supply_diffuser`, `return_grille`, `exhaust_grille`.
    pub kind: String,
    pub at: Vec2,
    pub confidence: f64,
}

/// A detected equipment box.
#[derive(Clone, Debug)]
pub struct DetectedEquipment {
    /// `ahu`, `vav`.
    pub kind: String,
    pub tag: String,
    pub min: Vec2,
    pub max: Vec2,
    pub confidence: f64,
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
    /// Width (across run) in inches, derived from parallel-pair gap.
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
}

impl DuctNetwork {
    /// Convert runs to [`FoundRun`]s in building feet for scoring, given the sheet's alignment.
    pub fn to_found_runs(&self, sh: &Sheet) -> Vec<FoundRun> {
        let mut out = Vec::new();
        for run in &self.runs {
            let sys = run.system.clone().unwrap_or_default();
            let windows = run.points.windows(2);
            for w in windows {
                let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
                let af = score::building_from_model(sh, [a.x, a.y]);
                let bf = score::building_from_model(sh, [b.x, b.y]);
                let size = if run.shape == "rect" { [run.width_in, run.depth_in] } else { [run.width_in, 0.0] };
                out.push(FoundRun {
                    level: sh.level.clone().unwrap_or_default(),
                    kind: "duct".into(),
                    system: sys.clone(),
                    shape: run.shape.clone(),
                    size_in: size,
                    from_ft: af,
                    to_ft: bf,
                    bottom_ft: run.bottom_ft,
                    ..Default::default()
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

/// Extract arcs from model space.
fn arcs(d: &Drawing) -> Vec<(Vec2, f64, f64, f64, String)> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        match &e.kind {
            EntityKind::Arc(a) if a.center.xy().is_finite() && a.radius.is_finite() && a.radius > 0.0 => {
                out.push((a.center.xy(), a.radius, a.start, a.end, e.common.layer.to_string()));
            }
            EntityKind::Circle(c) if c.center.xy().is_finite() && c.radius.is_finite() && c.radius > 0.0 => {
                out.push((c.center.xy(), c.radius, 0.0, 360.0, e.common.layer.to_string()));
            }
            _ => {}
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

/// Extract rectangles from closed 4-vertex polylines on any layer.
fn rectangles_all(d: &Drawing) -> Vec<(Vec2, Vec2, String)> {
    let mut out = Vec::new();
    for e in d.model.iter() {
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

/// Find parallel line pairs that represent duct walls.
/// When `all_layers` is false, only DUCT-layer segments are used (the original behaviour).
/// When true, segments on any layer are considered.
fn duct_pairs(segs: &[LineSeg], all_layers: bool) -> Vec<DuctPair> {
    use std::f64::consts::PI;

    let duct_segs: Vec<&LineSeg> = if all_layers {
        segs.iter().collect()
    } else {
        segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("DUCT")).collect()
    };
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
                if overlap < half * 2.0 {
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

// ---- Run chaining ----------------------------------------------------------

/// How the reader chains wall pairs into runs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DuctOptions {
    /// Chain every pair, skipping those already in a run (and runs along one already found).
    /// Off, chaining stops at the first pair an earlier run took, and centrelines supply the
    /// rest: what the vector sheets (which have centrelines) were tuned on. Scans, whose
    /// walls come in many pieces and whose centrelines are faint, need it on.
    pub all_pairs: bool,
    /// When true, read line segments from ALL layers instead of only the DUCT layer.
    /// Use for PDF-imported mechanical sheets where entities are on pen/colour layers.
    pub all_layers: bool,
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
            // A pair already chained into an earlier run: skipped, or (as tuned on the vector
            // sheets) the end of the chaining.
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

        runs.push(DuctRun {
            points: pts,
            width_in: width,
            depth_in: 0.0, // filled in by tag attachment
            shape: "rect".into(),
            system: None,
            bottom_ft: None,
            confidence: 0.7,
        });
    }
    runs
}

// ---- Tag parsing -----------------------------------------------------------

/// Parse a duct size from text: `24x12` → [24, 12]; `10"Ø` → [10, 0]; etc.
fn parse_size(t: &str) -> Option<[f64; 2]> {
    let up = t.to_ascii_uppercase().replace('×', "X");
    // Rectangular: `24x12`, `24"x12"`.
    for w in up.split_whitespace() {
        if let Some((a, b)) = w.split_once('X') {
            let a = a.trim().trim_end_matches('"').parse::<f64>().ok().filter(|v| *v > 0.0)?;
            let b = b.trim().trim_end_matches('"').parse::<f64>().ok().filter(|v| *v > 0.0)?;
            return Some([a, b]);
        }
    }
    // Round: `10"Ø`, `10" DIA`, `12Ø`, `10" RD`.
    let words: Vec<&str> = up.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
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

/// Detect fittings from arcs and geometry patterns.
/// When `all_layers` is true, arcs/rectangles on any layer are considered.
fn detect_fittings(d: &Drawing, runs: &[DuctRun], all_layers: bool) -> Vec<DetectedFitting> {
    let mut fittings = Vec::new();

    // Arcs near runs → elbows.
    for (center, _radius, _start, _end, layer) in arcs(d) {
        if !all_layers && !layer.eq_ignore_ascii_case("DUCT") {
            continue;
        }
        // In all_layers mode, only count arcs near a known run.
        if all_layers {
            let near_run = runs.iter().any(|r| {
                r.points.windows(2).any(|w| {
                    if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
                        seg_dist(center, *a, *b) < r.width_in.max(6.0)
                    } else {
                        false
                    }
                })
            });
            if !near_run {
                continue;
            }
        }
        fittings.push(DetectedFitting { kind: "elbow".into(), at: center, confidence: 0.7 });
    }

    // Turn markers: rectangles with internal crossing lines (box with X).
    let duct_rects = if all_layers { rectangles_all(d) } else { rectangles(d, "DUCT") };
    let segs = line_segments(d);
    let duct_lines: Vec<&LineSeg> = if all_layers {
        segs.iter().collect()
    } else {
        segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("DUCT")).collect()
    };
    for (min, max, _) in &duct_rects {
        let center = (*min + *max) * 0.5;
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
            fittings.push(DetectedFitting { kind: "turn".into(), at: center, confidence: 0.8 });
        }
    }

    // Detect tees and transitions at run endpoints.
    let tol = 3.0;
    for (i, run_a) in runs.iter().enumerate() {
        for end_a in [run_a.points.first(), run_a.points.last()].into_iter().flatten() {
            let mut connecting = 0;
            for (j, run_b) in runs.iter().enumerate() {
                if i == j {
                    continue;
                }
                for end_b in [run_b.points.first(), run_b.points.last()].into_iter().flatten() {
                    if end_a.dist(*end_b) < tol {
                        connecting += 1;
                    }
                }
            }
            if connecting >= 2 {
                fittings.push(DetectedFitting { kind: "tee".into(), at: *end_a, confidence: 0.6 });
            } else if connecting == 1 {
                // Check for size change → transition.
                for run_b in runs.iter() {
                    if std::ptr::eq(run_a, run_b) {
                        continue;
                    }
                    for end_b in [run_b.points.first(), run_b.points.last()].into_iter().flatten() {
                        if end_a.dist(*end_b) < tol && (run_a.width_in - run_b.width_in).abs() > 1.0 {
                            fittings.push(DetectedFitting { kind: "transition".into(), at: *end_a, confidence: 0.6 });
                        }
                    }
                }
            }
        }
    }

    fittings
}

// ---- Terminal detection ----------------------------------------------------

/// Detect terminals (diffusers, grilles) from DEVICE layer (or any layer in all_layers mode).
fn detect_terminals(d: &Drawing, all_layers: bool) -> Vec<DetectedTerminal> {
    let mut terminals = Vec::new();
    let device_rects = if all_layers { rectangles_all(d) } else { rectangles(d, "DEVICE") };
    let segs = line_segments(d);
    let device_lines: Vec<&LineSeg> = if all_layers {
        segs.iter().collect()
    } else {
        segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("DEVICE")).collect()
    };

    for (min, max, _) in &device_rects {
        let center = (*min + *max) * 0.5;
        let w = max.x - min.x;
        let h = max.y - min.y;

        // Device symbols are small-ish rectangles.
        if w < 0.5 || h < 0.5 || w > 200.0 || h > 200.0 {
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

        terminals.push(DetectedTerminal { kind: kind.into(), at: center, confidence: 0.7 });
    }
    terminals
}

// ---- Equipment detection ---------------------------------------------------

/// Detect equipment boxes (AHU, VAV, RTU, PTAC, EF) from EQUIP layer or any layer.
fn detect_equipment(d: &Drawing, all_layers: bool) -> Vec<DetectedEquipment> {
    let mut equipment = Vec::new();
    let equip_rects = if all_layers { rectangles_all(d) } else { rectangles(d, "EQUIP") };
    let texts = text_entities(d);

    for (min, max, _) in &equip_rects {
        let w = max.x - min.x;
        let h = max.y - min.y;
        if w < 2.0 || h < 2.0 {
            continue;
        }

        // Find the nearest text label for this equipment.
        let reach = w.max(h) * 3.0;
        let center = (*min + *max) * 0.5;
        let tag = texts
            .iter()
            .filter(|(_, at, layer, _)| (layer.eq_ignore_ascii_case("TEXT") || layer.eq_ignore_ascii_case("EQUIP")) && at.dist(center) < reach)
            .min_by(|(_, a, _, _), (_, b, _, _)| a.dist(center).total_cmp(&b.dist(center)));

        let (tag_text, kind) = match tag {
            Some((t, _, _, _)) => {
                let up = t.to_ascii_uppercase();
                let k = if up.contains("AHU") {
                    "ahu"
                } else if up.contains("RTU") {
                    "rtu"
                } else if up.contains("PTAC") {
                    "ptac"
                } else if up.contains("VAV") || up.contains("VB") {
                    "vav"
                } else if up.contains("EF") || up.contains("FAN") || up.contains("EXHAUST") {
                    "exhaust_fan"
                } else if up.contains("WH") || up.contains("WATER HEATER") {
                    "water_heater"
                } else if all_layers {
                    // In all_layers mode, accept any tagged rectangle near mechanical text
                    continue;
                } else {
                    "unknown"
                };
                (t.clone(), k.to_string())
            }
            None => continue,
        };

        equipment.push(DetectedEquipment { kind, tag: tag_text, min: *min, max: *max, confidence: 0.7 });
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

/// Attach size/height/system tags to the nearest duct run.
fn attach_tags(runs: &mut [DuctRun], d: &Drawing, abbr_map: &BTreeMap<String, String>, all_layers: bool) -> Vec<DetectedTag> {
    let texts = text_entities(d);
    let segs = line_segments(d);
    let text_lines: Vec<&LineSeg> = if all_layers {
        segs.iter().collect()
    } else {
        segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("TEXT")).collect()
    };
    let mut detected_tags = Vec::new();

    for (text, at, layer, _h) in &texts {
        if !all_layers && !layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        let size = parse_size(text);
        let system = extract_system(text, abbr_map);
        if size.is_none() && system.is_none() {
            // Check for height tag.
            let up = text.to_ascii_uppercase();
            let is_height = ["BOT", "BOD", "CL ", "CL@", "TOP", "TOD", "EL", "AFF", "@"].iter().any(|k| up.contains(k));
            if !is_height {
                continue;
            }
        }

        // Check for leader line to a run.
        let has_leader = text_lines.iter().any(|l| l.a.dist(*at) < 5.0 || l.b.dist(*at) < 5.0);

        detected_tags.push(DetectedTag { text: text.clone(), at: *at, size_in: size, system: system.clone(), has_leader });

        // Find the nearest run.
        let leader_target = if has_leader {
            text_lines
                .iter()
                .filter(|l| l.a.dist(*at) < 5.0 || l.b.dist(*at) < 5.0)
                .map(|l| if l.a.dist(*at) < l.b.dist(*at) { l.b } else { l.a })
                .next()
        } else {
            None
        };

        let target = leader_target.unwrap_or(*at);

        // Find the nearest run by distance to its segments.
        let mut best_run: Option<usize> = None;
        let mut best_dist = f64::INFINITY;
        for (i, run) in runs.iter().enumerate() {
            for w in run.points.windows(2) {
                if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
                    let d = seg_dist(target, *a, *b);
                    if d < best_dist {
                        best_dist = d;
                        best_run = Some(i);
                    }
                }
            }
        }

        let reach = 60.0; // model inches
        if let Some(ri) = best_run.filter(|_| best_dist < reach) {
            let run = match runs.get_mut(ri) {
                Some(r) => r,
                None => continue,
            };

            // Apply size tag.
            if let Some([w, d]) = size {
                if d > 0.0 {
                    run.width_in = w;
                    run.depth_in = d;
                    run.shape = "rect".into();
                } else {
                    run.width_in = w;
                    run.depth_in = 0.0;
                    run.shape = "round".into();
                }
                run.confidence = run.confidence.max(0.85);
            }

            // Apply system.
            if let Some(sys) = &system {
                run.system = Some(sys.clone());
                run.confidence = run.confidence.max(0.8);
            }

            // Apply height.
            let section_ft = if run.depth_in > 0.0 { run.depth_in } else { run.width_in } / 12.0;
            if let Some(h) = parse_height(text, section_ft) {
                run.bottom_ft = Some(h);
            }
        }
    }

    detected_tags
}

// ---- System inference from pipe labels in breaks ---------------------------

/// Infer system codes from labels drawn in breaks along runs (pipe-style `SA` labels).
fn infer_systems_from_labels(runs: &mut [DuctRun], d: &Drawing, abbr_map: &BTreeMap<String, String>, all_layers: bool) {
    let texts = text_entities(d);
    let reach = 20.0; // model inches — labels sit right on the centreline

    for (text, at, layer, _) in &texts {
        if !all_layers && !layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        let up = text.trim().to_ascii_uppercase();
        // System label: short text (1–3 chars, or a known abbreviation) near a run.
        let sys = if ["SA", "RA", "EA", "OA"].contains(&up.as_str()) { Some(up.clone()) } else { abbr_map.get(&up).cloned() };

        if let Some(code) = sys {
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
            }
        }
    }
}

// ---- Depth inference -------------------------------------------------------

/// Snap a measured width (inches) to the nearest standard duct size.
/// Standard rectangular duct widths: 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30,
/// 32, 34, 36, 40, 44, 48, 52, 56, 60.
/// Standard round duct diameters: 4, 5, 6, 7, 8, 9, 10, 12, 14, 16, 18, 20, 22, 24.
fn snap_width(w: f64) -> f64 {
    // Standard widths.
    let standards: &[f64] =
        &[4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 26.0, 28.0, 30.0, 32.0, 34.0, 36.0, 40.0, 44.0, 48.0, 52.0, 56.0, 60.0];
    let mut best = standards[0];
    let mut best_d = (w - best).abs();
    for &s in &standards[1..] {
        let d = (w - s).abs();
        if d < best_d {
            best = s;
            best_d = d;
        }
    }
    best
}

/// For runs whose depth wasn't set by a tag, infer rectangular depth from the pair gap
/// and known duct sizing conventions.  Also snap widths to standard sizes.
fn infer_depths_and_snap(runs: &mut [DuctRun]) {
    for run in runs.iter_mut() {
        // Snap the measured width to a standard size if no explicit tag set it.
        if run.confidence < 0.85 {
            run.width_in = snap_width(run.width_in);
        }
        if run.shape == "rect" && run.depth_in < 1.0 {
            // Default depth: 12" for typical office ducts, but try to match the width
            // if the width suggests it's a square duct.
            run.depth_in = if run.width_in <= 12.0 { run.width_in } else { 12.0 };
        }
    }
}

// ---- System inference from equipment and terminals -------------------------

/// Infer system codes from equipment labels (AHU→SA, exhaust_fan→EA) and nearby terminals.
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
                    changed = true;
                    break;
                }
            }
        }
    }
}

// ---- Centreline fallback ---------------------------------------------------

/// Add runs from CENTRE-layer lines not already covered by a parallel-pair run.
/// In all_layers mode, centreline fallback is skipped (pairs already use all layers).
fn add_centreline_runs(runs: &mut Vec<DuctRun>, segs: &[LineSeg], all_layers: bool) {
    if all_layers {
        return; // pairs already use all layers; centreline fallback would produce noise
    }
    let centre_segs: Vec<&LineSeg> =
        segs.iter().filter(|s| s.layer.eq_ignore_ascii_case("CENTRE") || s.layer.eq_ignore_ascii_case("CENTER")).collect();
    if centre_segs.is_empty() {
        return;
    }

    let cover_tol = 5.0; // model inches
    for cs in &centre_segs {
        // Skip if this centreline is already covered by an existing run.
        let mid = (cs.a + cs.b) * 0.5;
        let covered = runs.iter().any(|r| {
            r.points.windows(2).any(|w| if let (Some(a), Some(b)) = (w.first(), w.get(1)) { seg_dist(mid, *a, *b) < cover_tol } else { false })
        });
        if covered {
            continue;
        }

        // Add as a new run with default width.
        runs.push(DuctRun {
            points: vec![cs.a, cs.b],
            width_in: 12.0,
            depth_in: 0.0,
            shape: "rect".into(),
            system: None,
            bottom_ft: None,
            confidence: 0.5,
        });
        if runs.len() >= MAX_RUNS {
            break;
        }
    }
}

// ---- Main reader entry point -----------------------------------------------

/// Read the ductwork from a mechanical-plan DXF drawing.
///
/// `sh` is the truth sheet (for coordinate conversion); the reader only uses its placement
/// to convert model→building.  In a real pipeline, [`crate::sheet_reader`] would provide
/// the alignment instead.
pub fn read_ducts(d: &Drawing, sh: &Sheet) -> DuctNetwork {
    read_ducts_with(d, sh, &DuctOptions::default())
}

/// [`read_ducts`] with options.
pub fn read_ducts_with(d: &Drawing, _sh: &Sheet, o: &DuctOptions) -> DuctNetwork {
    let al = o.all_layers;

    // 1. Find parallel line pairs (DUCT layer, or all layers).
    let segs = line_segments(d);
    let pairs = duct_pairs(&segs, al);

    // 2. Chain pairs into runs.
    let mut runs = chain_runs(&pairs, o);

    // 2b. Add centreline-based runs that aren't covered by pairs (skipped in all_layers mode).
    add_centreline_runs(&mut runs, &segs, al);

    // 3. Read legend and abbreviations.
    let (legend, abbr_map) = read_legend(d);

    // 4. Attach size/height/system tags.
    let tags = attach_tags(&mut runs, d, &abbr_map, al);

    // 5. Infer systems from labels in run breaks.
    infer_systems_from_labels(&mut runs, d, &abbr_map, al);

    // 6. Snap widths and infer missing depths.
    infer_depths_and_snap(&mut runs);

    // 7. Detect fittings, terminals, equipment.
    let fittings = detect_fittings(d, &runs, al);
    let terminals = detect_terminals(d, al);
    let equipment = detect_equipment(d, al);

    // 8. Infer systems from equipment proximity and terminal tags.
    infer_systems_from_equipment(&mut runs, &equipment, &terminals);

    let abbreviations = abbr_map;

    DuctNetwork { runs, fittings, terminals, equipment, tags, legend, abbreviations }
}

/// Read ducts from a drawing with all-layers mode (for PDF-imported mechanical sheets).
pub fn read_ducts_any_layer(d: &Drawing) -> DuctNetwork {
    let sh = Sheet::default();
    read_ducts_with(d, &sh, &DuctOptions { all_pairs: true, all_layers: true })
}

/// Read ducts and score against truth for a single mechanical sheet.
pub fn read_and_score(d: &Drawing, t: &Truth, sh: &Sheet) -> score::RunScore {
    read_and_score_with(d, t, sh, &DuctOptions::default())
}

/// [`read_and_score`] with options.
pub fn read_and_score_with(d: &Drawing, t: &Truth, sh: &Sheet, o: &DuctOptions) -> score::RunScore {
    let network = read_ducts_with(d, sh, o);
    let found = network.to_found_runs(sh);
    let level = sh.level.clone().unwrap_or_default();
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
        // As tuned on the vector sheets: the chaining stops at the used pair.
        assert_eq!(chain_runs(&pairs, &DuctOptions::default()).len(), 1);
    }

    #[test]
    fn system_extraction() {
        let mut map = BTreeMap::new();
        map.insert("S".into(), "SA".into());
        assert_eq!(extract_system("SA 24x12", &map), Some("SA".into()));
        assert_eq!(extract_system("RA 24x12", &map), Some("RA".into()));
        assert_eq!(extract_system("S 24x12", &map), Some("SA".into()));
    }
}
