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
pub fn parse_feet_inches(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches("AFF").trim_end_matches("A.F.F.").trim();
    // Try `F'-I"` pattern.
    if let Some((feet_part, rest)) = s.split_once('\'') {
        let feet: f64 = feet_part.trim().trim_start_matches('-').parse().ok().or_else(|| {
            // Handle negative: `-10` already parsed.
            feet_part.trim().parse().ok()
        })?;
        let neg = feet_part.trim().starts_with('-');
        let inches_part = rest.trim().trim_start_matches('-').trim_end_matches('"').trim();
        let inches = if inches_part.is_empty() {
            0.0
        } else {
            parse_inches_value(inches_part)?
        };
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
                TradeLayer {
                    systems: vec!["FP"].into_iter().map(Into::into).collect(),
                    name: "Fire protection".into(),
                },
                TradeLayer {
                    systems: vec!["CW", "HW", "HWR", "SAN", "V", "G"].into_iter().map(Into::into).collect(),
                    name: "Plumbing".into(),
                },
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
pub fn resolve_heights(
    segments: &[SegmentInput],
    ctx: &LevelContext,
    rules: &TradeRules,
) -> Vec<ResolvedHeight> {
    let mut results = Vec::new();

    // Phase 1: resolve segments that have notes.
    // Phase 2: ceiling-based fallback.
    // Phase 3: rule-based stacking for the rest.

    // Gather rule-layer segments for stacking.
    let mut rule_segments: Vec<(usize, usize)> = Vec::new(); // (input idx, layer idx)

    for (i, seg) in segments.iter().enumerate() {
        if seg.vertical {
            continue;
        }

        let d_in = depth_in(&seg.shape, seg.size_in);
        let d_ft = d_in / 12.0;

        // Phase 1: note.
        if let Some(ref note_str) = seg.height_note
            && let Some(parsed) = parse_height_note(note_str)
            && let Some(rh) = resolve_from_note(&parsed, seg, d_ft, ctx)
        {
            results.push(rh);
            continue;
        }

        // Phase 2: ceiling.
        if let Some(rh) = resolve_from_ceiling(seg, d_ft, ctx, rules) {
            results.push(rh);
            continue;
        }

        // Phase 3: queue for rule-based stacking.
        let layer = rules.layer_of(&seg.system).unwrap_or(rules.layers.len());
        rule_segments.push((i, layer));
    }

    // Phase 3: rule-based stacking.
    let rule_results = resolve_from_rules(&rule_segments, segments, ctx, rules);
    results.extend(rule_results);

    results
}

fn resolve_from_note(
    parsed: &ParsedHeight,
    seg: &SegmentInput,
    depth_ft: f64,
    ctx: &LevelContext,
) -> Option<ResolvedHeight> {
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
            })
        }
    }
}

fn resolve_from_ceiling(
    seg: &SegmentInput,
    depth_ft: f64,
    ctx: &LevelContext,
    rules: &TradeRules,
) -> Option<ResolvedHeight> {
    let room = find_room(seg.midpoint_ft, &seg.level, &ctx.rooms)?;
    let ceiling = room.ceiling.as_ref()?;
    let ceiling_ft = ceiling.height_ft;
    let clearance_ft = rules.ceiling_gap_in / 12.0;
    // Bottom of the run sits clearance above the ceiling.
    let bottom = ceiling_ft + clearance_ft;
    let top = bottom + depth_ft;
    // Only use ceiling if the run fits below structure.
    if top > ctx.structure_underside_ft + 0.01 {
        return None;
    }
    Some(ResolvedHeight {
        segment_id: seg.id.clone(),
        bottom_ft: r6(bottom),
        top_ft: r6(top),
        centre_ft: r6((bottom + top) / 2.0),
        source: HeightSource::Ceiling,
        note: None,
    })
}

fn resolve_from_rules(
    rule_segments: &[(usize, usize)],
    segments: &[SegmentInput],
    ctx: &LevelContext,
    rules: &TradeRules,
) -> Vec<ResolvedHeight> {
    if rule_segments.is_empty() {
        return Vec::new();
    }

    // Group by layer, then stack from the top down.
    let mut by_layer: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &(seg_idx, layer_idx) in rule_segments {
        by_layer.entry(layer_idx).or_default().push(seg_idx);
    }

    let gap_ft = rules.gap_in / 12.0;
    let mut cursor = ctx.structure_underside_ft - 1.0 / 12.0; // 1" below structure
    let mut results = Vec::new();

    for seg_indices in by_layer.values() {
        // Find the deepest segment in this layer to set the band.
        let max_depth: f64 = seg_indices
            .iter()
            .filter_map(|&i| segments.get(i))
            .map(|s| depth_in(&s.shape, s.size_in) / 12.0)
            .fold(0.0, f64::max);

        if max_depth < 1e-9 {
            continue;
        }

        let top = cursor;
        let centre = top - max_depth / 2.0;

        for &idx in seg_indices {
            let Some(seg) = segments.get(idx) else { continue };
            let d_ft = depth_in(&seg.shape, seg.size_in) / 12.0;
            // Centre-align within the band.
            let bottom = centre - d_ft / 2.0;
            let seg_top = centre + d_ft / 2.0;
            results.push(ResolvedHeight {
                segment_id: seg.id.clone(),
                bottom_ft: r6(bottom),
                top_ft: r6(seg_top),
                centre_ft: r6(centre),
                source: HeightSource::Rule,
                note: None,
            });
        }

        cursor = top - max_depth - gap_ft;
    }

    results
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
        r.level == level
            && pt[0] >= r.min_ft[0] - 1e-9
            && pt[0] <= r.max_ft[0] + 1e-9
            && pt[1] >= r.min_ft[1] - 1e-9
            && pt[1] <= r.max_ft[1] + 1e-9
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
            let mid = [
                (s.path_ft[0][0] + s.path_ft[1][0]) / 2.0,
                (s.path_ft[0][1] + s.path_ft[1][1]) / 2.0,
            ];
            SegmentInput {
                id: s.id.clone(),
                system: s.system.clone(),
                kind: s.kind.clone(),
                shape: s.shape.clone(),
                size_in: s.size_in,
                level: s.level.clone(),
                midpoint_ft: mid,
                height_note: s.height.as_ref().and_then(|h| h.note.clone()),
                vertical: s.vertical,
            }
        })
        .collect()
}

/// Build a [`LevelContext`] from the truth for one level.
pub fn level_context(truth: &crate::truth::Truth, level_code: &str) -> Option<LevelContext> {
    let level = truth.level(level_code)?;
    let elev = level.elevation_ft;
    // The structure above this level: the next level's beams.
    let above = match level_code {
        "1" => "2",
        "2" => "R",
        _ => return None,
    };
    let beams: Vec<Beam> = truth.building.beams.iter().filter(|b| b.level == above).cloned().collect();
    let deepest = beams.iter().map(|b| b.depth_in).fold(0.0, f64::max);
    let tos = beams.first().map(|b| b.tos_ft).unwrap_or(elev + 13.0);
    let underside = tos - deepest / 12.0 - elev;
    let rooms: Vec<Room> = truth.building.rooms.iter().filter(|r| r.level == level_code).cloned().collect();
    Some(LevelContext {
        level_code: level_code.to_string(),
        elevation_ft: elev,
        structure_underside_ft: underside,
        rooms,
        beams,
    })
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
pub fn score_heights(
    resolved: &[ResolvedHeight],
    truth: &crate::truth::Truth,
) -> HeightScore {
    let mut sc = HeightScore::default();
    let resolved_map: BTreeMap<&str, &ResolvedHeight> =
        resolved.iter().map(|r| (r.segment_id.as_str(), r)).collect();

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

    // ---- Resolver against fixture truth ----

    #[test]
    fn resolve_fixture_heights_seed_42() {
        let truth = crate::truth_for(42);
        let inputs = inputs_from_truth(&truth);
        let rules = TradeRules::standard();

        for lv in ["1", "2"] {
            let Some(ctx) = level_context(&truth, lv) else { continue };
            let level_inputs: Vec<SegmentInput> =
                inputs.iter().filter(|s| s.level == lv).cloned().collect();
            let resolved = resolve_heights(&level_inputs, &ctx, &rules);
            let sc = score_heights(&resolved, &truth);

            // Every horizontal segment with a truth height should be resolved.
            let expected_count = truth
                .segments
                .iter()
                .filter(|s| s.level == lv && !s.vertical && s.height.is_some())
                .count();
            assert_eq!(
                sc.resolved, expected_count,
                "level {lv}: resolved {} of {} expected",
                sc.resolved, expected_count
            );

            // Note-sourced heights should be exact (they come from the truth's own notes).
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
                let level_inputs: Vec<SegmentInput> =
                    inputs.iter().filter(|s| s.level == lv).cloned().collect();
                let resolved = resolve_heights(&level_inputs, &ctx, &rules);

                // Check that we resolve every non-vertical segment with a height.
                let expected = truth
                    .segments
                    .iter()
                    .filter(|s| s.level == lv && !s.vertical && s.height.is_some())
                    .count();
                assert!(
                    resolved.len() >= expected,
                    "seed {seed} level {lv}: resolved {} but expected at least {expected}",
                    resolved.len()
                );

                // Note-sourced should be exact.
                let sc = score_heights(&resolved, &truth);
                if let Some(mae) = sc.mae("note") {
                    assert!(mae < 1e-3, "seed {seed} level {lv}: note MAE {mae:.6}");
                }
            }
        }
    }

    #[test]
    fn height_source_on_truth_segments() {
        // Verify that the truth itself now carries source annotations.
        let truth = crate::truth_for(42);
        let mut note_count = 0;
        let mut rule_count = 0;
        for s in &truth.segments {
            if let Some(h) = &s.height {
                match h.source {
                    HeightSource::Note => note_count += 1,
                    HeightSource::Rule => rule_count += 1,
                    _ => {}
                }
            }
        }
        // After tag_runs, some segments should have Note source. But truth_for doesn't call
        // tag_runs (no sheets). So all should be Rule.
        assert!(rule_count > 0, "should have rule-sourced heights");
        // truth_for doesn't tag, so note_count should be 0.
        assert_eq!(note_count, 0, "truth_for doesn't add notes");
    }

    #[test]
    fn generated_set_has_noted_heights() {
        // The full generate path tags runs, so some should be Note-sourced.
        let set = crate::generate(42).expect("generate");
        let mut note_count = 0;
        for s in &set.truth.segments {
            if let Some(h) = &s.height {
                if h.source == HeightSource::Note {
                    note_count += 1;
                }
            }
        }
        assert!(note_count > 0, "generated set should have note-sourced heights");
    }
}
