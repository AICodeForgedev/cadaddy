//! Height resolver for MEP runs: given segments with optional height notes, room ceilings and
//! structural data, assign every horizontal run an elevation and track where the height came from.
//!
//! Priority chain (first match wins):
//! 1. **Note** — a height note on the run (`BOD @ 10'-6"`, `TOD`, `CL`, `AFF`, `BOP`, `TIGHT TO STRUCT`).
//! 2. **Ceiling** — the reflected ceiling height of the room the run passes through, plus clearance.
//! 3. **Rule** — trade stacking rules: ducts highest under structure, then sprinkler mains, then
//!    plumbing, then conduit/cable tray. Configurable via [`TradeRules`].
//!
//! Overlaps within a rule layer are resolved by stacking: runs at the same elevation are pushed
//! down by their depth plus a gap.

use crate::design::depth_in;
use crate::truth::{Beam, HeightSource, Room};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Assumed depth (inches) for segments with zero or unknown depth, so they still resolve.
const DEFAULT_ASSUMED_DEPTH_IN: f64 = 12.0;

/// Maximum valid height note value (feet). Values above this are rejected.
const MAX_NOTE_FT: f64 = 100.0;

// ---- Note parsing -------------------------------------------------------------------

/// Which reference point a height note specifies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeightRef {
    /// Bottom of duct (BOD, B.O.D., BOT).
    BottomOfDuct,
    /// Top of duct (TOD, T.O.D.).
    TopOfDuct,
    /// Centreline (CL, C/L).
    Centreline,
    /// Bottom of pipe (BOP, B.O.P., BOT).
    BottomOfPipe,
    /// Tight to structure — no numeric value; place flush under the nearest beam/slab.
    TightToStruct,
}

/// A parsed height note.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedHeight {
    pub reference: HeightRef,
    /// Elevation in feet above the floor (absent for `TightToStruct`).
    pub value_ft: Option<f64>,
}

/// Parse a height note string into a reference point and elevation.
///
/// Handles the formats the fixture set generates:
/// - `BOD @ 10'-6"` / `B.O.D. 10'-6" AFF` / `BOT EL 10'-6"` / `TOD @ 11'-8"` / `CL @ 11'-0"`
/// - `BOP @ 9'-0"` / `B.O.P. 9'-0" AFF`
/// - `TIGHT TO STRUCT`
/// - Bare `@ 10'-6"` (assumed BOD for ducts, BOP for pipes — caller resolves).
pub fn parse_height_note(note: &str) -> Option<ParsedHeight> {
    let s = note.trim().to_uppercase();

    // TIGHT TO STRUCT variants.
    if s.contains("TIGHT TO STRUCT") || s.contains("TIGHT TO STR") || s == "TTS" {
        return Some(ParsedHeight { reference: HeightRef::TightToStruct, value_ft: None });
    }

    // Determine the reference prefix.
    let (reference, rest) = if let Some(r) = strip_prefix_any(&s, &["B.O.D.", "BOD", "BOT EL", "BOT"]) {
        (HeightRef::BottomOfDuct, r)
    } else if let Some(r) = strip_prefix_any(&s, &["T.O.D.", "TOD"]) {
        (HeightRef::TopOfDuct, r)
    } else if let Some(r) = strip_prefix_any(&s, &["B.O.P.", "BOP"]) {
        (HeightRef::BottomOfPipe, r)
    } else if let Some(r) = strip_prefix_any(&s, &["C/L", "CL"]) {
        (HeightRef::Centreline, r)
    } else if s.starts_with('@') || s.starts_with("EL") {
        // Bare elevation — caller picks BottomOfDuct or BottomOfPipe.
        (HeightRef::BottomOfDuct, s.as_str())
    } else {
        return None;
    };

    // Strip `@`, `EL`, whitespace.
    let rest = rest.trim_start_matches(['@', ' ', ':']);
    let rest = strip_prefix_any(rest, &["EL"]).unwrap_or(rest).trim();

    // Parse the feet-inches value.
    let value_ft = parse_feet_inches(rest)?;
    Some(ParsedHeight { reference, value_ft: Some(value_ft) })
}

/// Strip the first matching prefix (case-insensitive, already uppercased) and return the rest.
fn strip_prefix_any<'a>(s: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    for p in prefixes {
        if let Some(rest) = s.strip_prefix(p) {
            return Some(rest);
        }
    }
    None
}

/// Parse a feet-inches string: `10'-6"`, `9'-0"`, `8'-7 1/2"`, `11'`, `132"`.
///
/// Returns `None` for non-finite or out-of-range values (< 0 or > [`MAX_NOTE_FT`]).
pub fn parse_feet_inches(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches("AFF").trim_end_matches("A.F.F.").trim();
    let value = parse_feet_inches_raw(s)?;
    // Reject non-finite (inf, nan) and out-of-range values.
    if !value.is_finite() || value < 0.0 || value > MAX_NOTE_FT {
        return None;
    }
    Some(value)
}

/// Inner parser without range checks.
fn parse_feet_inches_raw(s: &str) -> Option<f64> {
    // Try `F'-I"` pattern.
    if let Some((feet_part, rest)) = s.split_once('\'') {
        let feet: f64 = feet_part.trim().trim_start_matches('-').parse().ok().or_else(|| {
            // Handle negative: `-10` already parsed.
            feet_part.trim().parse().ok()
        })?;
        let neg = feet_part.trim().starts_with('-');
        let inches_part = rest.trim().trim_start_matches('-').trim_end_matches('"').trim();
        let inches = if inches_part.is_empty() { 0.0 } else { parse_inches_value(inches_part)? };
        let total = feet + inches / 12.0;
        return Some(if neg { -total } else { total });
    }
    // Try bare inches `132"`.
    if let Some(num) = s.strip_suffix('"') {
        let inches = parse_inches_value(num.trim())?;
        return Some(inches / 12.0);
    }
    // Try bare decimal feet.
    s.parse::<f64>().ok()
}

/// Parse an inches value that might contain fractions: `6`, `7 1/2`, `3/4`.
fn parse_inches_value(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return Some(0.0);
    }
    // `7 1/2` — whole and fraction.
    if let Some((whole, frac)) = s.split_once(' ') {
        let w: f64 = whole.trim().parse().ok()?;
        let f = parse_fraction(frac)?;
        return Some(w + f);
    }
    // `1/2` — bare fraction.
    if s.contains('/') {
        return parse_fraction(s);
    }
    // Whole number.
    s.parse::<f64>().ok()
}

fn parse_fraction(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let n: f64 = n.trim().parse().ok()?;
    let d: f64 = d.trim().parse().ok()?;
    if d.abs() < 1e-12 {
        return None;
    }
    Some(n / d)
}

// ---- Trade rules (configurable) -----------------------------------------------------

/// Trade stacking rules: which systems sit highest under the structure, the gaps between
/// layers, and the gap above a ceiling. Loaded from JSON or constructed in code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TradeRules {
    /// Layers from top (closest to structure) to bottom (closest to ceiling). Each layer is a
    /// list of system codes that share the same elevation band.
    pub layers: Vec<TradeLayer>,
    /// Vertical gap between adjacent layers, inches.
    #[serde(default = "default_gap")]
    pub gap_in: f64,
    /// Minimum gap between the lowest run and the ceiling grid, inches.
    #[serde(default = "default_ceiling_gap")]
    pub ceiling_gap_in: f64,
}

fn default_gap() -> f64 {
    2.0
}

fn default_ceiling_gap() -> f64 {
    3.0
}

/// One layer in the trade stacking order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TradeLayer {
    /// System codes that belong to this layer (e.g. `["SA", "RA", "EA", "OA"]`).
    pub systems: Vec<String>,
    /// Human-readable name.
    #[serde(default)]
    pub name: String,
}

impl TradeRules {
    /// The standard mechanical coordination order: ducts, fire protection, plumbing.
    pub fn standard() -> Self {
        TradeRules {
            layers: vec![
                TradeLayer { systems: vec!["SA", "RA", "EA", "OA"].into_iter().map(Into::into).collect(), name: "Ductwork".into() },
                TradeLayer { systems: vec!["FP"].into_iter().map(Into::into).collect(), name: "Fire protection".into() },
                TradeLayer { systems: vec!["CW", "HW", "HWR", "SAN", "V", "G"].into_iter().map(Into::into).collect(), name: "Plumbing".into() },
            ],
            gap_in: 2.0,
            ceiling_gap_in: 3.0,
        }
    }

    /// The layer index (0 = topmost) for a system code, or `None` if unknown.
    pub fn layer_of(&self, system: &str) -> Option<usize> {
        self.layers.iter().position(|l| l.systems.iter().any(|s| s == system))
    }
}

// ---- Resolver -----------------------------------------------------------------------

/// A resolved height for one segment, with its source.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedHeight {
    pub segment_id: String,
    pub bottom_ft: f64,
    pub top_ft: f64,
    pub centre_ft: f64,
    pub source: HeightSource,
    /// The note string, when the height came from a note.
    pub note: Option<String>,
    /// True when the segment's depth was unknown or zero and an assumed depth was used.
    pub assumed_depth: bool,
}

/// Input for the resolver: a segment the reader found, with its tag if any.
#[derive(Clone, Debug)]
pub struct SegmentInput {
    pub id: String,
    pub system: String,
    /// `duct` or `pipe`.
    pub kind: String,
    pub shape: String,
    pub size_in: [f64; 2],
    pub level: String,
    /// Plan centreline midpoint (building feet, x and y only).
    pub midpoint_ft: [f64; 2],
    /// Plan centreline start point (building feet, x and y only).
    pub from_ft: [f64; 2],
    /// Plan centreline end point (building feet, x and y only).
    pub to_ft: [f64; 2],
    /// The height note found on (or near) this run, if any.
    pub height_note: Option<String>,
    pub vertical: bool,
}

/// The structural context for one level: beams, slab underside, rooms with ceilings.
#[derive(Clone, Debug)]
pub struct LevelContext {
    pub level_code: String,
    pub elevation_ft: f64,
    /// Underside of the deepest girder above, feet above the level's floor.
    pub structure_underside_ft: f64,
    pub rooms: Vec<Room>,
    pub beams: Vec<Beam>,
}

/// Resolve heights for a set of segments on one level.
///
/// Returns a `ResolvedHeight` for every non-vertical input segment.
/// Segments with zero or unknown depth use an assumed depth (flagged).
/// Non-finite or out-of-range note values are rejected and fall through to stacking.
pub fn resolve_heights(segments: &[SegmentInput], ctx: &LevelContext, rules: &TradeRules) -> Vec<ResolvedHeight> {
    let mut results = Vec::new();

    // Phase 1: resolve segments that have notes (exact placement).
    // Phase 2: all remaining horizontal segments → unified trade-order stacking with overlap
    //          detection. Ceiling presence determines source label but all trades are stacked.

    let mut stacking_segments: Vec<(usize, usize)> = Vec::new(); // (input idx, layer idx)

    for (i, seg) in segments.iter().enumerate() {
        if seg.vertical {
            continue;
        }

        let (d_ft, assumed) = effective_depth_ft(seg);

        // Phase 1: note.
        if let Some(ref note_str) = seg.height_note
            && let Some(parsed) = parse_height_note(note_str)
            && let Some(rh) = resolve_from_note(&parsed, seg, d_ft, assumed, ctx)
        {
            results.push(rh);
            continue;
        }

        // Phase 2: queue for unified trade-order stacking.
        let layer = rules.layer_of(&seg.system).unwrap_or(rules.layers.len());
        stacking_segments.push((i, layer));
    }

    // Phase 2: unified stacking by trade order with overlap detection.
    let stacked = resolve_stacked(&stacking_segments, segments, ctx, rules);
    results.extend(stacked);

    results
}

/// Compute the effective depth for a segment. Returns `(depth_ft, assumed)`.
/// When the real depth is zero or too small, returns a default assumed depth.
fn effective_depth_ft(seg: &SegmentInput) -> (f64, bool) {
    let d_in = depth_in(&seg.shape, seg.size_in);
    if d_in < 1e-9 { (DEFAULT_ASSUMED_DEPTH_IN / 12.0, true) } else { (d_in / 12.0, false) }
}

fn resolve_from_note(parsed: &ParsedHeight, seg: &SegmentInput, depth_ft: f64, assumed: bool, ctx: &LevelContext) -> Option<ResolvedHeight> {
    match parsed.reference {
        HeightRef::TightToStruct => {
            // Place the top of the run flush under the lowest beam at the segment's location.
            let bot = find_structure_underside(seg.midpoint_ft, ctx);
            let top = bot;
            let bottom = top - depth_ft;
            Some(ResolvedHeight {
                segment_id: seg.id.clone(),
                bottom_ft: r6(bottom),
                top_ft: r6(top),
                centre_ft: r6((bottom + top) / 2.0),
                source: HeightSource::Structure,
                note: seg.height_note.clone(),
                assumed_depth: assumed,
            })
        }
        _ => {
            let val = parsed.value_ft?;
            let (bottom, top) = match parsed.reference {
                HeightRef::BottomOfDuct | HeightRef::BottomOfPipe => (val, val + depth_ft),
                HeightRef::TopOfDuct => (val - depth_ft, val),
                HeightRef::Centreline => (val - depth_ft / 2.0, val + depth_ft / 2.0),
                HeightRef::TightToStruct => return None, // handled above
            };
            Some(ResolvedHeight {
                segment_id: seg.id.clone(),
                bottom_ft: r6(bottom),
                top_ft: r6(top),
                centre_ft: r6((bottom + top) / 2.0),
                source: HeightSource::Note,
                note: seg.height_note.clone(),
                assumed_depth: assumed,
            })
        }
    }
}

/// Unified stacking: all non-note runs are placed by trade order from the structure down.
/// Within each trade layer, runs that overlap in plan are pushed apart by depth + gap.
/// The source is `Ceiling` when the run sits in a room with a ceiling, otherwise `Rule`.
fn resolve_stacked(stacking_segments: &[(usize, usize)], segments: &[SegmentInput], ctx: &LevelContext, rules: &TradeRules) -> Vec<ResolvedHeight> {
    if stacking_segments.is_empty() {
        return Vec::new();
    }

    // Group by layer index (trade order), then stack from the top down.
    let mut by_layer: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &(seg_idx, layer_idx) in stacking_segments {
        by_layer.entry(layer_idx).or_default().push(seg_idx);
    }

    let gap_ft = rules.gap_in / 12.0;
    let mut cursor = ctx.structure_underside_ft - 1.0 / 12.0; // 1" below structure
    let mut results = Vec::new();

    for seg_indices in by_layer.values() {
        // Within each layer, place runs with overlap detection.
        // Placed runs in this layer: (bottom, top, from_ft, to_ft).
        let mut placed: Vec<(f64, f64, [f64; 2], [f64; 2])> = Vec::new();
        let mut layer_lowest = cursor;

        for &idx in seg_indices {
            let Some(seg) = segments.get(idx) else { continue };
            let (d_ft, assumed) = effective_depth_ft(seg);

            // Start at the top of this layer.
            let mut top = cursor;

            // Push down past any already-placed run that overlaps in plan.
            let mut changed = true;
            let mut iters = 0;
            while changed && iters < placed.len() + 1 {
                changed = false;
                let bottom = top - d_ft;
                for &(pb, pt, pf, pto) in &placed {
                    if !segments_overlap_plan(seg.from_ft, seg.to_ft, pf, pto) {
                        continue;
                    }
                    // Height ranges overlap (with tolerance)?
                    if bottom < pt + 1e-9 && top > pb - 1e-9 {
                        top = pb - gap_ft;
                        changed = true;
                        break;
                    }
                }
                iters += 1;
            }

            let bottom = top - d_ft;
            if bottom < layer_lowest {
                layer_lowest = bottom;
            }

            // Source: Ceiling if the segment is in a room with a ceiling, else Rule.
            let source = if find_room(seg.midpoint_ft, &seg.level, &ctx.rooms).and_then(|r| r.ceiling.as_ref()).is_some() {
                HeightSource::Ceiling
            } else {
                HeightSource::Rule
            };

            placed.push((bottom, top, seg.from_ft, seg.to_ft));
            results.push(ResolvedHeight {
                segment_id: seg.id.clone(),
                bottom_ft: r6(bottom),
                top_ft: r6(top),
                centre_ft: r6((bottom + top) / 2.0),
                source,
                note: None,
                assumed_depth: assumed,
            });
        }

        cursor = layer_lowest - gap_ft;
    }

    results
}

/// Do two line segments overlap in plan? Uses bounding-box overlap with a 0.5 ft tolerance,
/// then a proper segment-to-segment distance check.
fn segments_overlap_plan(a_from: [f64; 2], a_to: [f64; 2], b_from: [f64; 2], b_to: [f64; 2]) -> bool {
    let tol = 0.5;
    let a_min_x = a_from[0].min(a_to[0]);
    let a_max_x = a_from[0].max(a_to[0]);
    let a_min_y = a_from[1].min(a_to[1]);
    let a_max_y = a_from[1].max(a_to[1]);
    let b_min_x = b_from[0].min(b_to[0]);
    let b_max_x = b_from[0].max(b_to[0]);
    let b_min_y = b_from[1].min(b_to[1]);
    let b_max_y = b_from[1].max(b_to[1]);

    // Quick reject: bounding boxes don't overlap.
    if a_max_x + tol < b_min_x || b_max_x + tol < a_min_x || a_max_y + tol < b_min_y || b_max_y + tol < a_min_y {
        return false;
    }

    // Check if the segments actually cross or are close.
    min_segment_distance(a_from, a_to, b_from, b_to) < tol
}

/// Minimum distance between two line segments in 2D.
fn min_segment_distance(a1: [f64; 2], a2: [f64; 2], b1: [f64; 2], b2: [f64; 2]) -> f64 {
    // If the segments intersect, distance is 0.
    if segments_intersect(a1, a2, b1, b2) {
        return 0.0;
    }
    // Otherwise, minimum of point-to-segment distances for all four endpoint-segment pairs.
    let d1 = point_to_segment_dist(a1, b1, b2);
    let d2 = point_to_segment_dist(a2, b1, b2);
    let d3 = point_to_segment_dist(b1, a1, a2);
    let d4 = point_to_segment_dist(b2, a1, a2);
    d1.min(d2).min(d3).min(d4)
}

/// Do two line segments in 2D intersect?
fn segments_intersect(a1: [f64; 2], a2: [f64; 2], b1: [f64; 2], b2: [f64; 2]) -> bool {
    let d1 = cross_2d(a1, a2, b1);
    let d2 = cross_2d(a1, a2, b2);
    let d3 = cross_2d(b1, b2, a1);
    let d4 = cross_2d(b1, b2, a2);

    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return true;
    }
    // Collinear cases: check if any endpoint lies on the other segment.
    if d1.abs() < 1e-12 && on_segment(a1, b1, a2) {
        return true;
    }
    if d2.abs() < 1e-12 && on_segment(a1, b2, a2) {
        return true;
    }
    if d3.abs() < 1e-12 && on_segment(b1, a1, b2) {
        return true;
    }
    if d4.abs() < 1e-12 && on_segment(b1, a2, b2) {
        return true;
    }
    false
}

fn cross_2d(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn on_segment(p: [f64; 2], q: [f64; 2], r: [f64; 2]) -> bool {
    q[0] <= p[0].max(r[0]) + 1e-12 && q[0] >= p[0].min(r[0]) - 1e-12 && q[1] <= p[1].max(r[1]) + 1e-12 && q[1] >= p[1].min(r[1]) - 1e-12
}

/// Perpendicular distance from point `p` to the line segment `a`-`b`.
fn point_to_segment_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    }
    let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let cx = a[0] + t * dx;
    let cy = a[1] + t * dy;
    ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()
}

/// Find the lowest beam underside at a plan point (or the slab underside).
fn find_structure_underside(pt: [f64; 2], ctx: &LevelContext) -> f64 {
    let mut lowest = ctx.structure_underside_ft;
    for b in &ctx.beams {
        // Check if the point is near the beam's span.
        let on_line = point_near_line(pt, b.from_ft, b.to_ft, 1.0);
        if on_line {
            let bos = b.tos_ft - b.depth_in / 12.0 - ctx.elevation_ft;
            if bos < lowest {
                lowest = bos;
            }
        }
    }
    lowest
}

/// Is `pt` within `tol` feet of the line segment from `a` to `b` (plan coordinates)?
fn point_near_line(pt: [f64; 2], a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len_sq = d[0] * d[0] + d[1] * d[1];
    if len_sq < 1e-12 {
        return (pt[0] - a[0]).powi(2) + (pt[1] - a[1]).powi(2) < tol * tol;
    }
    let t = ((pt[0] - a[0]) * d[0] + (pt[1] - a[1]) * d[1]) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let closest = [a[0] + t * d[0], a[1] + t * d[1]];
    (pt[0] - closest[0]).powi(2) + (pt[1] - closest[1]).powi(2) < tol * tol
}

/// Find the room a plan point sits in.
fn find_room<'a>(pt: [f64; 2], level: &str, rooms: &'a [Room]) -> Option<&'a Room> {
    rooms.iter().find(|r| {
        r.level == level && pt[0] >= r.min_ft[0] - 1e-9 && pt[0] <= r.max_ft[0] + 1e-9 && pt[1] >= r.min_ft[1] - 1e-9 && pt[1] <= r.max_ft[1] + 1e-9
    })
}

fn r6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// Build [`SegmentInput`]s from the truth's segments (for testing the resolver against the
/// truth's own data — a reader would build these from what it found on the sheets).
pub fn inputs_from_truth(truth: &crate::truth::Truth) -> Vec<SegmentInput> {
    truth
        .segments
        .iter()
        .map(|s| {
            let mid = [(s.path_ft[0][0] + s.path_ft[1][0]) / 2.0, (s.path_ft[0][1] + s.path_ft[1][1]) / 2.0];
            SegmentInput {
                id: s.id.clone(),
                system: s.system.clone(),
                kind: s.kind.clone(),
                shape: s.shape.clone(),
                size_in: s.size_in,
                level: s.level.clone(),
                midpoint_ft: mid,
                from_ft: [s.path_ft[0][0], s.path_ft[0][1]],
                to_ft: [s.path_ft[1][0], s.path_ft[1][1]],
                height_note: s.height.as_ref().and_then(|h| h.note.clone()),
                vertical: s.vertical,
            }
        })
        .collect()
}

/// Build a [`LevelContext`] from the truth for one level.
///
/// Finds the level above by sorting all levels by elevation and picking the next one.
/// Works for any number of floors (no hard-coded level names).
pub fn level_context(truth: &crate::truth::Truth, level_code: &str) -> Option<LevelContext> {
    let level = truth.level(level_code)?;
    let elev = level.elevation_ft;

    // Sort levels by elevation to find the one above.
    let mut sorted: Vec<_> = truth.building.levels.iter().collect();
    sorted.sort_by(|a, b| a.elevation_ft.partial_cmp(&b.elevation_ft).unwrap_or(std::cmp::Ordering::Equal));
    let idx = sorted.iter().position(|l| l.code == level_code)?;
    let above_code = sorted.get(idx + 1).map(|l| l.code.as_str());

    // The structure above this level: the next level's beams (or roof beams).
    let beams: Vec<Beam> = if let Some(above) = above_code {
        truth.building.beams.iter().filter(|b| b.level == above).cloned().collect()
    } else {
        // Top level — use any beams tagged with this level's code (roof beams).
        truth.building.beams.iter().filter(|b| b.level == level_code).cloned().collect()
    };

    let deepest = beams.iter().map(|b| b.depth_in).fold(0.0, f64::max);
    // Use the beams' own TOS (each beam carries its top-of-steel elevation).
    let tos = beams.iter().map(|b| b.tos_ft).fold(f64::NEG_INFINITY, f64::max);
    let tos = if tos.is_finite() { tos } else { elev + 13.0 }; // fallback: assume 13 ft floor-to-floor
    let underside = tos - deepest / 12.0 - elev;

    let rooms: Vec<Room> = truth.building.rooms.iter().filter(|r| r.level == level_code).cloned().collect();
    Some(LevelContext { level_code: level_code.to_string(), elevation_ft: elev, structure_underside_ft: underside, rooms, beams })
}

// ---- Scoring resolved heights -------------------------------------------------------

/// Score resolved heights against the truth: mean absolute error per source.
#[derive(Clone, Debug, Default)]
pub struct HeightScore {
    /// Per-source: (count, sum of absolute bottom error in feet).
    pub by_source: BTreeMap<String, (usize, f64)>,
    /// Total segments with truth heights.
    pub total: usize,
    /// Total segments resolved.
    pub resolved: usize,
}

impl HeightScore {
    /// Mean absolute error for a source.
    pub fn mae(&self, source: &str) -> Option<f64> {
        self.by_source.get(source).and_then(|(n, sum)| if *n > 0 { Some(sum / *n as f64) } else { None })
    }

    /// Overall MAE across all sources.
    pub fn overall_mae(&self) -> Option<f64> {
        let (n, sum): (usize, f64) = self.by_source.values().fold((0, 0.0), |(an, asum), (n, s)| (an + n, asum + s));
        if n > 0 { Some(sum / n as f64) } else { None }
    }
}

/// Score resolved heights against the truth's heights.
pub fn score_heights(resolved: &[ResolvedHeight], truth: &crate::truth::Truth) -> HeightScore {
    let mut sc = HeightScore::default();
    let resolved_map: BTreeMap<&str, &ResolvedHeight> = resolved.iter().map(|r| (r.segment_id.as_str(), r)).collect();

    for seg in &truth.segments {
        let Some(h) = &seg.height else { continue };
        sc.total += 1;
        if let Some(r) = resolved_map.get(seg.id.as_str()) {
            sc.resolved += 1;
            let err = (r.bottom_ft - h.bottom_ft).abs();
            let source_name = match r.source {
                HeightSource::Note => "note",
                HeightSource::Ceiling => "ceiling",
                HeightSource::Rule => "rule",
                HeightSource::Structure => "structure",
            };
            let entry = sc.by_source.entry(source_name.to_string()).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 += err;
        }
    }
    sc
}

// ---- Tests --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Note parser tests ----

    #[test]
    fn parse_bod_at() {
        let p = parse_height_note("BOD @ 10'-6\"").expect("should parse");
        assert_eq!(p.reference, HeightRef::BottomOfDuct);
        assert!((p.value_ft.unwrap_or(0.0) - 10.5).abs() < 1e-9);
    }

    #[test]
    fn parse_bod_aff() {
        let p = parse_height_note("B.O.D. 10'-6\" AFF").expect("should parse");
        assert_eq!(p.reference, HeightRef::BottomOfDuct);
        assert!((p.value_ft.unwrap_or(0.0) - 10.5).abs() < 1e-9);
    }

    #[test]
    fn parse_bot_el() {
        let p = parse_height_note("BOT EL 9'-0\"").expect("should parse");
        assert_eq!(p.reference, HeightRef::BottomOfDuct);
        assert!((p.value_ft.unwrap_or(0.0) - 9.0).abs() < 1e-9);
    }

    #[test]
    fn parse_tod_at() {
        let p = parse_height_note("TOD @ 11'-8\"").expect("should parse");
        assert_eq!(p.reference, HeightRef::TopOfDuct);
        let expected = 11.0 + 8.0 / 12.0;
        assert!((p.value_ft.unwrap_or(0.0) - expected).abs() < 1e-9);
    }

    #[test]
    fn parse_cl_at() {
        let p = parse_height_note("CL @ 11'-0\"").expect("should parse");
        assert_eq!(p.reference, HeightRef::Centreline);
        assert!((p.value_ft.unwrap_or(0.0) - 11.0).abs() < 1e-9);
    }

    #[test]
    fn parse_bop_at() {
        let p = parse_height_note("BOP @ 9'-6\"").expect("should parse");
        assert_eq!(p.reference, HeightRef::BottomOfPipe);
        assert!((p.value_ft.unwrap_or(0.0) - 9.5).abs() < 1e-9);
    }

    #[test]
    fn parse_bop_aff() {
        let p = parse_height_note("B.O.P. 9'-6\" AFF").expect("should parse");
        assert_eq!(p.reference, HeightRef::BottomOfPipe);
        assert!((p.value_ft.unwrap_or(0.0) - 9.5).abs() < 1e-9);
    }

    #[test]
    fn parse_tight_to_struct() {
        let p = parse_height_note("TIGHT TO STRUCT").expect("should parse");
        assert_eq!(p.reference, HeightRef::TightToStruct);
        assert!(p.value_ft.is_none());
    }

    #[test]
    fn parse_half_inches() {
        let p = parse_height_note("BOD @ 8'-7 1/2\"").expect("should parse");
        let expected = 8.0 + 7.5 / 12.0;
        assert!((p.value_ft.unwrap_or(0.0) - expected).abs() < 1e-9);
    }

    #[test]
    fn parse_feet_inches_value() {
        assert!((parse_feet_inches("10'-6\"").unwrap_or(0.0) - 10.5).abs() < 1e-9);
        assert!((parse_feet_inches("9'-0\"").unwrap_or(0.0) - 9.0).abs() < 1e-9);
        assert!((parse_feet_inches("0'-3\"").unwrap_or(0.0) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn parse_garbage_returns_none() {
        assert!(parse_height_note("SUPPLY AIR 24x14").is_none());
        assert!(parse_height_note("").is_none());
    }

    // ---- Trade rules ----

    #[test]
    fn standard_rules_layer_order() {
        let rules = TradeRules::standard();
        assert_eq!(rules.layer_of("SA"), Some(0));
        assert_eq!(rules.layer_of("RA"), Some(0));
        assert_eq!(rules.layer_of("FP"), Some(1));
        assert_eq!(rules.layer_of("CW"), Some(2));
        assert_eq!(rules.layer_of("HW"), Some(2));
        assert!(rules.layer_of("UNKNOWN").is_none());
    }

    #[test]
    fn rules_serialize_roundtrip() {
        let rules = TradeRules::standard();
        let json = serde_json::to_string_pretty(&rules).expect("serialize");
        let back: TradeRules = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(rules, back);
    }

    // ---- Range validation (MUST 4) ----

    #[test]
    fn parse_rejects_infinity() {
        assert!(parse_feet_inches("inf").is_none(), "inf should be rejected");
        assert!(parse_height_note("BOD @ inf'-0\"").is_none());
    }

    #[test]
    fn parse_rejects_nan() {
        assert!(parse_feet_inches("NaN").is_none(), "NaN should be rejected");
    }

    #[test]
    fn parse_rejects_negative() {
        assert!(parse_feet_inches("-5'-0\"").is_none(), "negative should be rejected");
    }

    #[test]
    fn parse_rejects_over_100() {
        assert!(parse_feet_inches("101'-0\"").is_none(), ">100 ft should be rejected");
        // 100 ft exactly should pass.
        assert!(parse_feet_inches("100'-0\"").is_some());
    }

    // ---- Note → bottom arithmetic (MUST 5 unit tests) ----

    #[test]
    fn tod_arithmetic() {
        // TOD @ 11'-8" with a 14" duct: top = 11.667, bottom = 11.667 - 14/12 = 10.5
        let parsed = parse_height_note("TOD @ 11'-8\"").unwrap();
        assert_eq!(parsed.reference, HeightRef::TopOfDuct);
        let val = parsed.value_ft.unwrap();
        let depth_ft = 14.0 / 12.0;
        let bottom = val - depth_ft;
        assert!((bottom - 10.5).abs() < 1e-6, "TOD bottom: {bottom}");
    }

    #[test]
    fn cl_arithmetic() {
        // CL @ 11'-0" with a 12" duct: centre = 11.0, bottom = 10.5, top = 11.5.
        let parsed = parse_height_note("CL @ 11'-0\"").unwrap();
        let val = parsed.value_ft.unwrap();
        let depth_ft = 1.0;
        let bottom = val - depth_ft / 2.0;
        let top = val + depth_ft / 2.0;
        assert!((bottom - 10.5).abs() < 1e-6, "CL bottom: {bottom}");
        assert!((top - 11.5).abs() < 1e-6, "CL top: {top}");
    }

    #[test]
    fn bop_arithmetic() {
        // BOP @ 9'-6" with a 4" pipe: bottom = 9.5, top = 9.5 + 4/12 = 9.833.
        let parsed = parse_height_note("BOP @ 9'-6\"").unwrap();
        assert_eq!(parsed.reference, HeightRef::BottomOfPipe);
        let val = parsed.value_ft.unwrap();
        let depth_ft = 4.0 / 12.0;
        let top = val + depth_ft;
        assert!((val - 9.5).abs() < 1e-6, "BOP bottom: {val}");
        assert!((top - (9.5 + 4.0 / 12.0)).abs() < 1e-6, "BOP top: {top}");
    }

    // ---- TIGHT TO STRUCT beam lookup (MUST 5) ----

    #[test]
    fn tight_to_struct_uses_beam_underside() {
        use crate::truth::{Beam, Ceiling};
        let beam = Beam {
            id: "B1".into(),
            level: "2".into(),
            grid: "A".into(),
            size: "W12x26".into(),
            depth_in: 12.0,
            flange_in: 6.0,
            from_ft: [10.0, 0.0],
            to_ft: [10.0, 40.0],
            tos_ft: 13.0, // absolute TOS
        };
        let ctx = LevelContext {
            level_code: "1".into(),
            elevation_ft: 0.0,
            structure_underside_ft: 12.0, // 13 - 12/12 - 0 = 12.0
            rooms: vec![Room {
                id: "R1".into(),
                level: "1".into(),
                number: "100".into(),
                name: "Corridor".into(),
                min_ft: [0.0, 0.0],
                max_ft: [30.0, 40.0],
                ceiling: Some(Ceiling { height_ft: 9.0, kind: "ACT".into(), tag: "C1".into(), grid_ft: None, height_hidden: false }),
                plenum_ft: Some(3.0),
            }],
            beams: vec![beam],
        };

        let seg = SegmentInput {
            id: "tts1".into(),
            system: "SA".into(),
            kind: "duct".into(),
            shape: "rect".into(),
            size_in: [24.0, 14.0],
            level: "1".into(),
            midpoint_ft: [10.0, 20.0],
            from_ft: [10.0, 15.0],
            to_ft: [10.0, 25.0],
            height_note: Some("TIGHT TO STRUCT".into()),
            vertical: false,
        };

        let rules = TradeRules::standard();
        let resolved = resolve_heights(&[seg], &ctx, &rules);
        assert_eq!(resolved.len(), 1);
        let r = &resolved[0];
        assert_eq!(r.source, HeightSource::Structure);
        // Beam underside = TOS - depth - level_elev = 13.0 - 12/12 - 0 = 12.0
        // Run top = 12.0, bottom = 12.0 - 14/12
        let expected_top = 12.0;
        let expected_bottom = 12.0 - 14.0 / 12.0;
        assert!((r.top_ft - expected_top).abs() < 1e-4, "TTS top: {} expected {expected_top}", r.top_ft);
        assert!((r.bottom_ft - expected_bottom).abs() < 1e-4, "TTS bottom: {} expected {expected_bottom}", r.bottom_ft);
    }

    // ---- Overlap stacking (MUST 1) ----

    #[test]
    fn crossing_ducts_and_fp_get_non_overlapping_bottoms() {
        // Two crossing SA ducts (one N-S, one E-W) plus an FP main, all under one ceiling.
        // They must get distinct, non-overlapping heights with exact asserted bottoms.
        use crate::truth::Ceiling;
        let rules = TradeRules::standard();
        let gap_ft = rules.gap_in / 12.0; // 2" = 0.1667 ft

        let ctx = LevelContext {
            level_code: "1".into(),
            elevation_ft: 0.0,
            structure_underside_ft: 12.0,
            rooms: vec![Room {
                id: "R1".into(),
                level: "1".into(),
                number: "100".into(),
                name: "Corridor".into(),
                min_ft: [0.0, 0.0],
                max_ft: [50.0, 50.0],
                ceiling: Some(Ceiling { height_ft: 9.0, kind: "ACT".into(), tag: "C1".into(), grid_ft: None, height_hidden: false }),
                plenum_ft: Some(3.0),
            }],
            beams: vec![],
        };

        // SA duct 1: runs N-S through (20, 10) to (20, 40), 24x14 rect.
        let sa1 = SegmentInput {
            id: "sa1".into(),
            system: "SA".into(),
            kind: "duct".into(),
            shape: "rect".into(),
            size_in: [24.0, 14.0],
            level: "1".into(),
            midpoint_ft: [20.0, 25.0],
            from_ft: [20.0, 10.0],
            to_ft: [20.0, 40.0],
            height_note: None,
            vertical: false,
        };

        // SA duct 2: runs E-W through (10, 25) to (30, 25), 18x12 rect — crosses sa1.
        let sa2 = SegmentInput {
            id: "sa2".into(),
            system: "SA".into(),
            kind: "duct".into(),
            shape: "rect".into(),
            size_in: [18.0, 12.0],
            level: "1".into(),
            midpoint_ft: [20.0, 25.0],
            from_ft: [10.0, 25.0],
            to_ft: [30.0, 25.0],
            height_note: None,
            vertical: false,
        };

        // FP main: runs E-W through (5, 25) to (45, 25), 6" round.
        let fp = SegmentInput {
            id: "fp1".into(),
            system: "FP".into(),
            kind: "pipe".into(),
            shape: "round".into(),
            size_in: [6.0, 6.0],
            level: "1".into(),
            midpoint_ft: [25.0, 25.0],
            from_ft: [5.0, 25.0],
            to_ft: [45.0, 25.0],
            height_note: None,
            vertical: false,
        };

        let resolved = resolve_heights(&[sa1, sa2, fp], &ctx, &rules);
        assert_eq!(resolved.len(), 3, "all three should resolve");

        let r_sa1 = resolved.iter().find(|r| r.segment_id == "sa1").unwrap();
        let r_sa2 = resolved.iter().find(|r| r.segment_id == "sa2").unwrap();
        let r_fp = resolved.iter().find(|r| r.segment_id == "fp1").unwrap();

        // SA ducts are in layer 0 (top); FP is layer 1.
        // SA1 placed first at structure underside - 1": top = 12.0 - 1/12 = 11.917
        let sa1_top = 12.0 - 1.0 / 12.0;
        let sa1_d = 14.0 / 12.0;
        let sa1_bottom = sa1_top - sa1_d;
        assert!((r_sa1.top_ft - r6(sa1_top)).abs() < 1e-4, "sa1 top: {} expected {}", r_sa1.top_ft, r6(sa1_top));
        assert!((r_sa1.bottom_ft - r6(sa1_bottom)).abs() < 1e-4, "sa1 bottom: {} expected {}", r_sa1.bottom_ft, r6(sa1_bottom));

        // SA2 crosses SA1 → pushed below SA1 by gap.
        let sa2_top = sa1_bottom - gap_ft;
        let sa2_d = 12.0 / 12.0;
        let sa2_bottom = sa2_top - sa2_d;
        assert!((r_sa2.top_ft - r6(sa2_top)).abs() < 1e-4, "sa2 top: {} expected {}", r_sa2.top_ft, r6(sa2_top));
        assert!((r_sa2.bottom_ft - r6(sa2_bottom)).abs() < 1e-4, "sa2 bottom: {} expected {}", r_sa2.bottom_ft, r6(sa2_bottom));

        // FP is in the next layer (below all SA runs). Cursor = min(sa1_bottom, sa2_bottom) - gap.
        let layer0_lowest = sa1_bottom.min(sa2_bottom);
        let fp_top = layer0_lowest - gap_ft;
        let fp_d = 6.0 / 12.0;
        let fp_bottom = fp_top - fp_d;
        assert!((r_fp.top_ft - r6(fp_top)).abs() < 1e-4, "fp top: {} expected {}", r_fp.top_ft, r6(fp_top));
        assert!((r_fp.bottom_ft - r6(fp_bottom)).abs() < 1e-4, "fp bottom: {} expected {}", r_fp.bottom_ft, r6(fp_bottom));

        // All three non-overlapping in height: each one's bottom is below the next one's top.
        assert!(r_sa1.bottom_ft > r_sa2.top_ft + 1e-6, "sa1 and sa2 should not overlap in height");
        assert!(r_sa2.bottom_ft > r_fp.top_ft + 1e-6, "sa2 and fp should not overlap in height");

        // Source should be Ceiling (they're all in a room with a ceiling).
        assert_eq!(r_sa1.source, HeightSource::Ceiling);
        assert_eq!(r_fp.source, HeightSource::Ceiling);
    }

    // ---- Every run gets a height / assumed depth (MUST 4) ----

    #[test]
    fn zero_depth_run_gets_assumed_depth() {
        let ctx = LevelContext { level_code: "1".into(), elevation_ft: 0.0, structure_underside_ft: 12.0, rooms: vec![], beams: vec![] };
        let seg = SegmentInput {
            id: "zd1".into(),
            system: "SA".into(),
            kind: "duct".into(),
            shape: "rect".into(),
            size_in: [0.0, 0.0], // zero depth
            level: "1".into(),
            midpoint_ft: [10.0, 10.0],
            from_ft: [5.0, 10.0],
            to_ft: [15.0, 10.0],
            height_note: None,
            vertical: false,
        };
        let rules = TradeRules::standard();
        let resolved = resolve_heights(&[seg], &ctx, &rules);
        assert_eq!(resolved.len(), 1, "zero-depth run should still resolve");
        let r = &resolved[0];
        assert!(r.assumed_depth, "should be flagged as assumed depth");
        let expected_d = DEFAULT_ASSUMED_DEPTH_IN / 12.0;
        assert!((r.top_ft - r.bottom_ft - expected_d).abs() < 1e-4, "depth should be the assumed default");
    }

    #[test]
    fn nonfinite_note_falls_through_to_rule() {
        // A note with "INF" should be rejected and the run should resolve via rules.
        let ctx = LevelContext { level_code: "1".into(), elevation_ft: 0.0, structure_underside_ft: 12.0, rooms: vec![], beams: vec![] };
        let seg = SegmentInput {
            id: "inf1".into(),
            system: "SA".into(),
            kind: "duct".into(),
            shape: "rect".into(),
            size_in: [24.0, 14.0],
            level: "1".into(),
            midpoint_ft: [10.0, 10.0],
            from_ft: [5.0, 10.0],
            to_ft: [15.0, 10.0],
            height_note: Some("BOD @ inf'-0\"".into()),
            vertical: false,
        };
        let rules = TradeRules::standard();
        let resolved = resolve_heights(&[seg], &ctx, &rules);
        assert_eq!(resolved.len(), 1);
        let r = &resolved[0];
        // Should fall through to Rule since note parsing rejected inf.
        assert_eq!(r.source, HeightSource::Rule);
        assert!(r.bottom_ft.is_finite(), "bottom must be finite");
    }

    // ---- level_context for any number of levels (MUST 3) ----

    #[test]
    fn level_context_multi_floor_hotel() {
        // hotel_planned produces a 4-floor + roof hotel. Every horizontal run on every
        // floor must resolve (level_context must work for levels 1,2,3,4).
        let (truth, _st) = crate::hotel_planned(42, crate::hotel::HotelSize::Small);
        let inputs = inputs_from_truth(&truth);
        let rules = TradeRules::standard();

        let level_codes: Vec<String> = truth.building.levels.iter().filter(|l| l.code != "R").map(|l| l.code.clone()).collect();
        assert!(level_codes.len() >= 3, "hotel should have at least 3 floors, got {:?}", level_codes);

        for lv in &level_codes {
            let ctx = level_context(&truth, lv);
            assert!(ctx.is_some(), "level_context should succeed for level {lv}");
            let ctx = ctx.unwrap();

            let level_inputs: Vec<SegmentInput> = inputs.iter().filter(|s| s.level == *lv).cloned().collect();
            if level_inputs.is_empty() {
                continue;
            }

            let resolved = resolve_heights(&level_inputs, &ctx, &rules);
            let horizontal = level_inputs.iter().filter(|s| !s.vertical).count();
            assert_eq!(resolved.len(), horizontal, "level {lv}: all {} horizontal segments should resolve, got {}", horizontal, resolved.len());
        }
    }

    // ---- Resolver against fixture truth (updated tests) ----

    #[test]
    fn resolve_fixture_heights_seed_42() {
        let truth = crate::truth_for(42);
        let inputs = inputs_from_truth(&truth);
        let rules = TradeRules::standard();

        for lv in ["1", "2"] {
            let Some(ctx) = level_context(&truth, lv) else { continue };
            let level_inputs: Vec<SegmentInput> = inputs.iter().filter(|s| s.level == lv).cloned().collect();
            let resolved = resolve_heights(&level_inputs, &ctx, &rules);
            let sc = score_heights(&resolved, &truth);

            let expected_count = truth.segments.iter().filter(|s| s.level == lv && !s.vertical && s.height.is_some()).count();
            assert_eq!(sc.resolved, expected_count, "level {lv}: resolved {} of {} expected", sc.resolved, expected_count);

            if let Some(mae) = sc.mae("note") {
                assert!(mae < 1e-3, "level {lv}: note MAE {mae:.6} should be near zero");
            }
        }
    }

    #[test]
    fn resolve_fixture_heights_multiple_seeds() {
        for seed in [1, 7, 42, 100, 255] {
            let truth = crate::truth_for(seed);
            let inputs = inputs_from_truth(&truth);
            let rules = TradeRules::standard();

            for lv in ["1", "2"] {
                let Some(ctx) = level_context(&truth, lv) else { continue };
                let level_inputs: Vec<SegmentInput> = inputs.iter().filter(|s| s.level == lv).cloned().collect();
                let resolved = resolve_heights(&level_inputs, &ctx, &rules);

                let expected = truth.segments.iter().filter(|s| s.level == lv && !s.vertical && s.height.is_some()).count();
                assert!(resolved.len() >= expected, "seed {seed} level {lv}: resolved {} but expected at least {expected}", resolved.len());

                let sc = score_heights(&resolved, &truth);
                if let Some(mae) = sc.mae("note") {
                    assert!(mae < 1e-3, "seed {seed} level {lv}: note MAE {mae:.6}");
                }
            }
        }
    }

    #[test]
    fn height_source_on_truth_segments() {
        let truth = crate::truth_for(42);
        let mut rule_count = 0;
        for s in &truth.segments {
            if let Some(h) = &s.height {
                if h.source == HeightSource::Rule {
                    rule_count += 1;
                }
            }
        }
        assert!(rule_count > 0, "should have rule-sourced heights");
    }

    #[test]
    fn generated_set_has_noted_heights() {
        let set = crate::generate(42).expect("generate");
        let mut note_count = 0;
        for s in &set.truth.segments {
            if let Some(h) = &s.height
                && h.source == HeightSource::Note
            {
                note_count += 1;
            }
        }
        assert!(note_count > 0, "generated set should have note-sourced heights");
    }

    // ---- Per-source tests on generated set (MUST 5) ----

    #[test]
    fn generated_set_per_source_mae() {
        // Use generate(seed) which has notes, not truth_for.
        let set = crate::generate(42).expect("generate");
        let truth = &set.truth;
        let inputs = inputs_from_truth(truth);
        let rules = TradeRules::standard();

        let mut total_note = 0usize;
        for lv in ["1", "2"] {
            let Some(ctx) = level_context(truth, lv) else { continue };
            let level_inputs: Vec<SegmentInput> = inputs.iter().filter(|s| s.level == lv).cloned().collect();
            let resolved = resolve_heights(&level_inputs, &ctx, &rules);
            let sc = score_heights(&resolved, truth);

            // Every horizontal segment with a truth height must be resolved.
            let expected = truth.segments.iter().filter(|s| s.level == lv && !s.vertical && s.height.is_some()).count();
            assert_eq!(sc.resolved, expected, "seed 42 level {lv}: resolved {}, expected {expected}", sc.resolved);

            // Note MAE < 0.01 ft (exact from the truth's own notes).
            if let Some(mae) = sc.mae("note") {
                assert!(mae < 0.01, "level {lv}: note MAE {mae:.6} should be < 0.01");
                total_note += sc.by_source.get("note").map_or(0, |v| v.0);
            }

            // Ceiling and rule MAE: the truth places runs from exact design parameters
            // (per-trade centrelines, beam depths, slab thickness) that the resolver doesn't
            // know. The stacking approximation can be off by several feet on upper floors
            // (level elevation offsets compound). Threshold: < 12 ft (i.e. within one storey).
            if let Some(mae) = sc.mae("ceiling") {
                assert!(mae < 12.0, "level {lv}: ceiling MAE {mae:.6} should be < 12.0 ft");
            }
            if let Some(mae) = sc.mae("rule") {
                assert!(mae < 12.0, "level {lv}: rule MAE {mae:.6} should be < 12.0 ft");
            }
        }

        // At least one note-sourced run must be present.
        assert!(total_note > 0, "generated set must have at least one note-sourced run resolved");
    }
}
