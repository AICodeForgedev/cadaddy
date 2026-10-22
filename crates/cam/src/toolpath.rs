//! Toolpath computation: offset, lead-in/out, tabs, depth passes, cut ordering.
//!
//! The main entry point is [`compute`], which takes closed profiles and a [`CamJob`] and produces
//! a sequence of [`Move`]s ready for G-code emission.

use cadaddy_geom::{offset::offset_polyline, Polyline, Segment, Vec2};
use serde::{Deserialize, Serialize};

use crate::profile::{centroid, contains, perimeter, signed_area};
use crate::{CamError, CamJob, CutOrder, LeadKind, OffsetSide, Result, TabParams};

// ── Toolpath primitives ──────────────────────────────────────────────────────

/// A single CNC move.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Move {
    /// Rapid positioning (G00).
    Rapid { x: f64, y: f64, z: f64 },
    /// Linear feed (G01).
    Feed { x: f64, y: f64, z: f64, f: f64 },
    /// Clockwise arc (G02) — I, J are centre offsets from the start.
    ArcCw { x: f64, y: f64, z: f64, i: f64, j: f64, f: f64 },
    /// Counter-clockwise arc (G03).
    ArcCcw { x: f64, y: f64, z: f64, i: f64, j: f64, f: f64 },
}

impl Move {
    pub fn end_xy(&self) -> Vec2 {
        match *self {
            Move::Rapid { x, y, .. }
            | Move::Feed { x, y, .. }
            | Move::ArcCw { x, y, .. }
            | Move::ArcCcw { x, y, .. } => Vec2::new(x, y),
        }
    }
    pub fn end_z(&self) -> f64 {
        match *self {
            Move::Rapid { z, .. } | Move::Feed { z, .. } | Move::ArcCw { z, .. } | Move::ArcCcw { z, .. } => z,
        }
    }
}

/// A complete toolpath: header comment, moves, and the source profile index.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileToolpath {
    /// Index of the source profile (in the order they were passed to [`compute`]).
    pub profile_index: usize,
    pub moves: Vec<Move>,
}

/// A complete CAM job result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolpathResult {
    pub paths: Vec<ProfileToolpath>,
    /// The offset polylines (for preview).
    pub offset_profiles: Vec<Polyline>,
}

// ── Public entry point ───────────────────────────────────────────────────────

/// Compute toolpaths for the given closed profiles.
///
/// Each profile must be a closed [`Polyline`].  The function:
/// 1. Applies tool-radius offset (inside / outside / on).
/// 2. Orders cuts (nearest-neighbour or inside-out).
/// 3. For each profile, generates depth passes with lead-in/out and tabs.
pub fn compute(profiles: &[Polyline], job: &CamJob) -> Result<ToolpathResult> {
    if profiles.is_empty() {
        return Err(CamError::NoProfiles);
    }
    if job.offset != OffsetSide::On && job.tool.diameter <= 0.0 {
        return Err(CamError::BadToolDiameter(job.tool.diameter));
    }

    // 1. Offset each profile.
    let mut offset_profiles: Vec<(usize, Polyline)> = Vec::new();
    for (i, pl) in profiles.iter().enumerate() {
        if !pl.closed || pl.vertices.len() < 3 {
            continue;
        }
        let offset_poly = offset_profile(pl, job)?;
        for op in offset_poly {
            offset_profiles.push((i, op));
        }
    }
    if offset_profiles.is_empty() {
        return Err(CamError::NoProfiles);
    }

    // 2. Order cuts.
    let order = order_profiles(&offset_profiles, profiles, job.order);

    // 3. Generate moves.
    let _safe_z = job.tool.safe_z;
    let mut paths = Vec::new();
    let mut all_offsets = Vec::new();

    for idx in order {
        let (prof_idx, ref offset_pl) = offset_profiles[idx];
        all_offsets.push(offset_pl.clone());
        let moves = profile_moves(offset_pl, job, prof_idx)?;
        paths.push(ProfileToolpath { profile_index: prof_idx, moves });
    }

    Ok(ToolpathResult { paths, offset_profiles: all_offsets })
}

// ── Offset ───────────────────────────────────────────────────────────────────

fn offset_profile(pl: &Polyline, job: &CamJob) -> Result<Vec<Polyline>> {
    if job.offset == OffsetSide::On {
        return Ok(vec![pl.clone()]);
    }
    let radius = job.tool.diameter / 2.0;
    // Positive offset goes to the LEFT of the polyline's direction.
    // CCW polyline (positive area): left = inside.
    let area = signed_area(pl);
    let d = match job.offset {
        OffsetSide::Inside => {
            if area > 0.0 { radius } else { -radius }
        }
        OffsetSide::Outside => {
            if area > 0.0 { -radius } else { radius }
        }
        OffsetSide::On => 0.0, // already handled
    };
    let pieces = offset_polyline(pl, d);
    if pieces.is_empty() {
        return Ok(Vec::new()); // profile too small for this tool
    }
    Ok(pieces.into_iter().map(|p| p.polyline).collect())
}

// ── Ordering ─────────────────────────────────────────────────────────────────

fn order_profiles(
    offset: &[(usize, Polyline)],
    originals: &[Polyline],
    order: CutOrder,
) -> Vec<usize> {
    let n = offset.len();
    if n == 0 {
        return Vec::new();
    }
    match order {
        CutOrder::Nearest => nearest_neighbour(offset),
        CutOrder::InsideOut => inside_out(offset, originals),
    }
}

fn nearest_neighbour(offset: &[(usize, Polyline)]) -> Vec<usize> {
    let n = offset.len();
    let centroids: Vec<Vec2> = offset.iter().map(|(_, pl)| centroid(pl)).collect();
    let mut used = vec![false; n];
    let mut result = Vec::with_capacity(n);
    let mut pos = Vec2::ZERO;
    for _ in 0..n {
        let best = (0..n)
            .filter(|i| !used[*i])
            .min_by(|a, b| pos.dist(centroids[*a]).total_cmp(&pos.dist(centroids[*b])));
        let Some(idx) = best else { break };
        used[idx] = true;
        pos = centroids[idx];
        result.push(idx);
    }
    result
}

fn inside_out(offset: &[(usize, Polyline)], originals: &[Polyline]) -> Vec<usize> {
    let n = offset.len();
    // Compute nesting depth: how many other original profiles contain this one.
    let mut depth: Vec<usize> = vec![0; n];
    for i in 0..n {
        for j in 0..n {
            if i != j {
                let (oi, _) = &offset[i];
                let (oj, _) = &offset[j];
                // Use the original profiles for containment (offset might be smaller).
                if let (Some(a), Some(b)) = (originals.get(*oj), originals.get(*oi))
                    && contains(a, b)
                {
                    depth[i] += 1;
                }
            }
        }
    }
    let mut indices: Vec<usize> = (0..n).collect();
    // Deepest first (innermost = most nested).
    indices.sort_by(|a, b| depth[*b].cmp(&depth[*a]));
    indices
}

// ── Profile moves (one profile, all depth passes) ────────────────────────────

fn profile_moves(offset_pl: &Polyline, job: &CamJob, _prof_idx: usize) -> Result<Vec<Move>> {
    let safe_z = job.tool.safe_z;
    let total_depth = job.tool.depth;
    let step = if job.tool.step_down > 0.0 { job.tool.step_down } else { total_depth };
    let feed = job.tool.feed_rate;
    let plunge = job.tool.plunge_rate;

    let segs = offset_pl.segments();
    if segs.is_empty() {
        return Ok(Vec::new());
    }

    // Entry point: first vertex of the offset polyline.
    let entry = segs.first().map(|s| s.start()).unwrap_or(Vec2::ZERO);

    // Compute tab positions along the profile.
    let tab_ranges = tab_ranges(offset_pl, &job.tabs);

    let mut moves = Vec::new();

    // Rapid to above the entry (or lead-in start).
    let lead_in_start = lead_in_point(entry, &segs, &job.lead_in);
    let approach = lead_in_start.unwrap_or(entry);
    moves.push(Move::Rapid { x: approach.x, y: approach.y, z: safe_z });

    // Depth passes.
    let mut z = 0.0_f64;
    let mut pass = 0usize;
    let max_passes = ((total_depth / step).ceil() as usize).clamp(1, 1000);
    while pass < max_passes {
        z = (z - step).max(-total_depth);
        let is_last_pass = (z + total_depth).abs() < 1e-12;

        // Plunge.
        if let Some(li) = lead_in_start {
            moves.push(Move::Feed { x: li.x, y: li.y, z, f: plunge });
            // Lead-in arc or line to the profile entry.
            append_lead_in(&mut moves, li, entry, &segs, &job.lead_in, z, feed);
        } else {
            moves.push(Move::Feed { x: entry.x, y: entry.y, z, f: plunge });
        }

        // Cut around the profile.
        append_profile_cut(&mut moves, &segs, z, feed, if is_last_pass { &tab_ranges } else { &[] }, &job.tabs);

        // Lead-out.
        let exit = segs.last().map(|s| s.end()).unwrap_or(entry);
        if let Some(lo) = lead_out_point(exit, &segs, &job.lead_out) {
            append_lead_out(&mut moves, exit, lo, &segs, &job.lead_out, z, feed);
        }

        // Retract.
        let last_xy = moves.last().map(|m| m.end_xy()).unwrap_or(entry);
        moves.push(Move::Rapid { x: last_xy.x, y: last_xy.y, z: safe_z });

        pass += 1;
        if z <= -total_depth + 1e-12 {
            break;
        }
    }

    Ok(moves)
}

// ── Lead-in / lead-out ───────────────────────────────────────────────────────

/// Compute a lead-in start point: offset perpendicular to the first segment direction.
fn lead_in_point(entry: Vec2, segs: &[Segment], lead: &crate::LeadParams) -> Option<Vec2> {
    if lead.kind == LeadKind::None || lead.length <= 0.0 || segs.is_empty() {
        return None;
    }
    let dir = segs.first().map(|s| {
        let d = s.end() - s.start();
        if d.len() > 1e-12 { d.normalized() } else { Vec2::new(1.0, 0.0) }
    })?;
    let perp = Vec2::new(-dir.y, dir.x); // left of travel direction
    match lead.kind {
        LeadKind::Arc => Some(entry + perp * lead.length),
        LeadKind::Line => Some(entry + perp * lead.length),
        LeadKind::None => None,
    }
}

fn lead_out_point(exit: Vec2, segs: &[Segment], lead: &crate::LeadParams) -> Option<Vec2> {
    if lead.kind == LeadKind::None || lead.length <= 0.0 || segs.is_empty() {
        return None;
    }
    let dir = segs.last().map(|s| {
        let d = s.end() - s.start();
        if d.len() > 1e-12 { d.normalized() } else { Vec2::new(1.0, 0.0) }
    })?;
    let perp = Vec2::new(-dir.y, dir.x);
    match lead.kind {
        LeadKind::Arc => Some(exit + perp * lead.length),
        LeadKind::Line => Some(exit + perp * lead.length),
        LeadKind::None => None,
    }
}

fn append_lead_in(moves: &mut Vec<Move>, from: Vec2, to: Vec2, _segs: &[Segment], lead: &crate::LeadParams, z: f64, feed: f64) {
    match lead.kind {
        LeadKind::Arc => {
            // Quarter-circle arc from `from` to `to`, centre at the midpoint projected onto the
            // perpendicular bisector — simplified as the entry point itself (the arc centre is
            // at the profile entry, radius = lead.length).
            let i = to.x - from.x;
            let j = to.y - from.y;
            // We go CW into the profile (from the left side).
            moves.push(Move::ArcCw { x: to.x, y: to.y, z, i, j, f: feed });
        }
        LeadKind::Line => {
            moves.push(Move::Feed { x: to.x, y: to.y, z, f: feed });
        }
        LeadKind::None => {}
    }
}

fn append_lead_out(moves: &mut Vec<Move>, from: Vec2, to: Vec2, _segs: &[Segment], lead: &crate::LeadParams, z: f64, feed: f64) {
    match lead.kind {
        LeadKind::Arc => {
            let i = to.x - from.x;
            let j = to.y - from.y;
            moves.push(Move::ArcCcw { x: to.x, y: to.y, z, i, j, f: feed });
        }
        LeadKind::Line => {
            moves.push(Move::Feed { x: to.x, y: to.y, z, f: feed });
        }
        LeadKind::None => {}
    }
}

// ── Profile cutting with tabs ────────────────────────────────────────────────

/// Ranges along the profile (cumulative arc-length) where tabs are placed.
fn tab_ranges(pl: &Polyline, tabs: &TabParams) -> Vec<(f64, f64)> {
    if tabs.count == 0 || tabs.width <= 0.0 {
        return Vec::new();
    }
    let total = perimeter(pl);
    if total <= 0.0 {
        return Vec::new();
    }
    let spacing = total / tabs.count as f64;
    let half_w = tabs.width / 2.0;
    (0..tabs.count)
        .map(|i| {
            let centre = spacing * (i as f64 + 0.5);
            ((centre - half_w).max(0.0), (centre + half_w).min(total))
        })
        .collect()
}

/// Emit feed moves around the profile, lifting for tabs on the last pass.
fn append_profile_cut(moves: &mut Vec<Move>, segs: &[Segment], z: f64, feed: f64, tabs: &[(f64, f64)], tab_params: &TabParams) {
    let mut cum_len = 0.0_f64;
    for s in segs {
        let seg_len = s.len();
        let seg_start = cum_len;
        let seg_end = cum_len + seg_len;

        // Check if any tab overlaps this segment.
        let in_tab = |at: f64| -> bool { tabs.iter().any(|(a, b)| at >= *a && at <= *b) };

        match s {
            Segment::Line(line) => {
                // Sample the segment to handle tabs.
                if tabs.is_empty() || !tabs.iter().any(|(a, b)| *b > seg_start && *a < seg_end) {
                    moves.push(Move::Feed { x: line.b.x, y: line.b.y, z, f: feed });
                } else {
                    // Split into small steps where tabs change.
                    let steps = 20usize.max((seg_len / 0.5).ceil() as usize).min(200);
                    for step in 1..=steps {
                        let t = step as f64 / steps as f64;
                        let p = line.a.lerp(line.b, t);
                        let at = seg_start + seg_len * t;
                        let cut_z = if in_tab(at) { z + tab_params.height } else { z };
                        moves.push(Move::Feed { x: p.x, y: p.y, z: cut_z, f: feed });
                    }
                }
            }
            Segment::Arc { arc, ccw } => {
                let end = s.end();
                if tabs.is_empty() || !tabs.iter().any(|(a, b)| *b > seg_start && *a < seg_end) {
                    // Emit as a G02/G03.
                    let start_pt = s.start();
                    let i = arc.center.x - start_pt.x;
                    let j = arc.center.y - start_pt.y;
                    if *ccw {
                        moves.push(Move::ArcCcw { x: end.x, y: end.y, z, i, j, f: feed });
                    } else {
                        moves.push(Move::ArcCw { x: end.x, y: end.y, z, i, j, f: feed });
                    }
                } else {
                    // Linearize through tabs.
                    let steps = 20usize.max((seg_len / 0.5).ceil() as usize).min(200);
                    for step in 1..=steps {
                        let t = step as f64 / steps as f64;
                        let p = s.at(t);
                        let at = seg_start + seg_len * t;
                        let cut_z = if in_tab(at) { z + tab_params.height } else { z };
                        moves.push(Move::Feed { x: p.x, y: p.y, z: cut_z, f: feed });
                    }
                }
            }
        }

        cum_len = seg_end;
    }
}

// ── CNC DXF helpers ──────────────────────────────────────────────────────────

/// Flatten entities into joined polylines suitable for CNC DXF export.
///
/// Takes a set of closed polylines and returns them with:
/// - blocks exploded (the caller should have already exploded inserts)
/// - splines linearized (the caller approximates them)
/// - arcs preserved as bulge vertices
/// - open chains joined where endpoints coincide
pub fn clean_cnc_polylines(polylines: &[Polyline]) -> Vec<Polyline> {
    // For now we just pass them through; the caller extracts and joins.
    polylines.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_geom::PolyVertex;
    use crate::{CamJob, LeadParams, OffsetSide, TabParams, ToolParams};

    fn square() -> Polyline {
        Polyline {
            vertices: vec![
                PolyVertex::new(Vec2::new(0.0, 0.0)),
                PolyVertex::new(Vec2::new(10.0, 0.0)),
                PolyVertex::new(Vec2::new(10.0, 10.0)),
                PolyVertex::new(Vec2::new(0.0, 10.0)),
            ],
            closed: true,
        }
    }

    fn basic_job(offset: OffsetSide) -> CamJob {
        CamJob {
            tool: ToolParams {
                diameter: 6.0,
                feed_rate: 500.0,
                plunge_rate: 100.0,
                spindle_speed: 12000.0,
                depth: 5.0,
                safe_z: 10.0,
                step_down: 2.5,
            },
            offset,
            lead_in: LeadParams { kind: LeadKind::None, length: 0.0 },
            lead_out: LeadParams { kind: LeadKind::None, length: 0.0 },
            tabs: TabParams::default(),
            order: crate::CutOrder::Nearest,
        }
    }

    #[test]
    fn outside_offset_produces_toolpath() {
        let result = compute(&[square()], &basic_job(OffsetSide::Outside)).ok();
        assert!(result.is_some());
        let r = result.as_ref().map(|r| &r.paths);
        let paths = r.as_ref();
        assert!(paths.is_some_and(|p| !p.is_empty()));
        // Should have rapid + plunge + cut moves.
        let moves = &paths.map(|p| &p[0].moves);
        assert!(moves.is_some_and(|m| m.len() >= 3));
    }

    #[test]
    fn inside_offset_produces_toolpath() {
        let result = compute(&[square()], &basic_job(OffsetSide::Inside));
        assert!(result.is_ok());
    }

    #[test]
    fn on_offset_no_compensation() {
        let result = compute(&[square()], &basic_job(OffsetSide::On));
        assert!(result.is_ok());
    }

    #[test]
    fn depth_passes_respect_step_down() {
        let mut job = basic_job(OffsetSide::On);
        job.tool.depth = 10.0;
        job.tool.step_down = 2.5;
        let result = compute(&[square()], &job);
        assert!(result.is_ok());
        let r = result.as_ref().ok().map(|r| &r.paths[0]);
        // 10 / 2.5 = 4 passes, each ends with a retract rapid.
        let rapids: usize = r.map_or(0, |p| p.moves.iter().filter(|m| matches!(m, Move::Rapid { .. })).count());
        // First rapid (to entry) + 4 retract rapids = 5 rapids.
        assert_eq!(rapids, 5, "expected 5 rapids for 4 depth passes");
    }

    #[test]
    fn empty_profiles_error() {
        let result = compute(&[], &basic_job(OffsetSide::On));
        assert!(matches!(result, Err(CamError::NoProfiles)));
    }

    #[test]
    fn bad_diameter_error() {
        let mut job = basic_job(OffsetSide::Outside);
        job.tool.diameter = -1.0;
        let result = compute(&[square()], &job);
        assert!(matches!(result, Err(CamError::BadToolDiameter(_))));
    }

    #[test]
    fn tabs_lift_z() {
        let mut job = basic_job(OffsetSide::On);
        job.tabs = TabParams { width: 2.0, height: 1.0, count: 4 };
        job.tool.step_down = 5.0; // single pass so tabs apply
        let result = compute(&[square()], &job);
        assert!(result.is_ok());
        let r = result.ok();
        let all_z: Vec<f64> = r.iter().flat_map(|r| r.paths.iter().flat_map(|p| p.moves.iter().map(|m| m.end_z()))).collect();
        // Should have some moves at tab height (-5 + 1 = -4) and some at full depth (-5).
        let has_tab_z = all_z.iter().any(|z| (*z - (-4.0)).abs() < 0.1);
        let has_full_z = all_z.iter().any(|z| (*z - (-5.0)).abs() < 0.1);
        assert!(has_tab_z, "should have tab-height moves, got z values: {all_z:?}");
        assert!(has_full_z, "should have full-depth moves, got z values: {all_z:?}");
    }

    #[test]
    fn lead_in_arc_adds_approach() {
        let mut job = basic_job(OffsetSide::On);
        job.lead_in = LeadParams { kind: LeadKind::Arc, length: 3.0 };
        job.tool.step_down = 5.0;
        let result = compute(&[square()], &job);
        assert!(result.is_ok());
        let moves = &result.as_ref().ok().map(|r| &r.paths[0].moves);
        // First move is a rapid, second is plunge (feed), third is lead-in arc (ArcCw).
        assert!(moves.is_some_and(|m| m.len() >= 3 && matches!(m[2], Move::ArcCw { .. })));
    }

    #[test]
    fn multiple_profiles_ordered() {
        let sq1 = square();
        let mut sq2 = square();
        for v in &mut sq2.vertices {
            v.p.x += 50.0;
        }
        let result = compute(&[sq1, sq2], &basic_job(OffsetSide::On));
        assert!(result.is_ok());
        let r = result.ok();
        assert_eq!(r.as_ref().map(|r| r.paths.len()), Some(2));
    }

    #[test]
    fn inside_out_cuts_holes_first() {
        // Outer square + inner square (hole).
        let outer = Polyline {
            vertices: vec![
                PolyVertex::new(Vec2::new(0.0, 0.0)),
                PolyVertex::new(Vec2::new(20.0, 0.0)),
                PolyVertex::new(Vec2::new(20.0, 20.0)),
                PolyVertex::new(Vec2::new(0.0, 20.0)),
            ],
            closed: true,
        };
        let inner = Polyline {
            vertices: vec![
                PolyVertex::new(Vec2::new(5.0, 5.0)),
                PolyVertex::new(Vec2::new(15.0, 5.0)),
                PolyVertex::new(Vec2::new(15.0, 15.0)),
                PolyVertex::new(Vec2::new(5.0, 15.0)),
            ],
            closed: true,
        };
        let mut job = basic_job(OffsetSide::On);
        job.order = crate::CutOrder::InsideOut;
        let result = compute(&[outer.clone(), inner.clone()], &job);
        assert!(result.is_ok());
        let r = result.ok();
        // Inner profile (index 1) should be cut first.
        assert_eq!(r.as_ref().map(|r| r.paths[0].profile_index), Some(1));
        assert_eq!(r.as_ref().map(|r| r.paths[1].profile_index), Some(0));
    }

    #[test]
    fn toolpath_starts_and_ends_at_safe_z() {
        let result = compute(&[square()], &basic_job(OffsetSide::On));
        assert!(result.is_ok());
        let moves = &result.as_ref().ok().map(|r| &r.paths[0].moves);
        if let Some(m) = moves {
            // First move should be a rapid at safe_z.
            assert!(matches!(m.first(), Some(Move::Rapid { z, .. }) if (*z - 10.0).abs() < 1e-9));
            // Last move should be a rapid at safe_z.
            assert!(matches!(m.last(), Some(Move::Rapid { z, .. }) if (*z - 10.0).abs() < 1e-9));
        }
    }
}
