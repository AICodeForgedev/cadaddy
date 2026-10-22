//! Toolpath computation: offset, lead-in/out, tabs, depth passes, cut ordering.
//!
//! The main entry point is [`compute`], which takes closed profiles and a [`CamJob`] and produces
//! a sequence of [`Move`]s ready for G-code emission.

use cadaddy_geom::{Arc, Line, Polyline, Segment, Vec2, offset::offset_polyline_round};
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
            Move::Rapid { x, y, .. } | Move::Feed { x, y, .. } | Move::ArcCw { x, y, .. } | Move::ArcCcw { x, y, .. } => Vec2::new(x, y),
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
    let mut paths = Vec::new();
    let mut all_offsets = Vec::new();

    for idx in order {
        let Some((prof_idx, offset_pl)) = offset_profiles.get(idx) else { continue };
        let area = profiles.get(*prof_idx).map_or(0.0, signed_area);
        all_offsets.push(offset_pl.clone());
        let moves = profile_moves(offset_pl, job, waste_on_left(area, job.offset));
        paths.push(ProfileToolpath { profile_index: *prof_idx, moves });
    }

    Ok(ToolpathResult { paths, offset_profiles: all_offsets })
}

/// Whether the waste (the material the cut removes, where leads may go) lies to the left of a
/// profile's direction of travel: inside the profile for `Inside`, outside it for `Outside` and
/// `On`. A CCW profile (positive area) has its inside on the left. The tool-centre path keeps the
/// profile's direction, so the same side holds for it.
fn waste_on_left(area: f64, side: OffsetSide) -> bool {
    match side {
        OffsetSide::Inside => area > 0.0,
        OffsetSide::Outside | OffsetSide::On => area < 0.0,
    }
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
            if area > 0.0 {
                radius
            } else {
                -radius
            }
        }
        OffsetSide::Outside => {
            if area > 0.0 {
                -radius
            } else {
                radius
            }
        }
        OffsetSide::On => 0.0, // already handled
    };
    // Round joins: the tool centre swings round an outside corner on an arc of the tool radius.
    let pieces = offset_polyline_round(pl, d);
    if pieces.is_empty() {
        return Ok(Vec::new()); // profile too small for this tool
    }
    Ok(pieces.into_iter().map(|p| p.polyline).collect())
}

// ── Ordering ─────────────────────────────────────────────────────────────────

fn order_profiles(offset: &[(usize, Polyline)], originals: &[Polyline], order: CutOrder) -> Vec<usize> {
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
        let best = (0..n).filter(|i| !used[*i]).min_by(|a, b| pos.dist(centroids[*a]).total_cmp(&pos.dist(centroids[*b])));
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

/// The tool-centre path as segments, starting at the middle of its longest segment, so a lead
/// joins a straight run (or a smooth arc) rather than a corner.
pub(crate) fn path_segments(pl: &Polyline) -> Vec<Segment> {
    let segs = pl.segments();
    let longest = segs.iter().enumerate().max_by(|a, b| a.1.len().total_cmp(&b.1.len())).map(|(i, s)| (i, *s));
    let Some((k, s)) = longest else { return segs };
    if s.len() <= 1e-9 {
        return segs;
    }
    let (first, second) = split_segment(&s, 0.5);
    let mut out = Vec::with_capacity(segs.len() + 1);
    out.push(second);
    out.extend(segs.iter().skip(k + 1).copied());
    out.extend(segs.iter().take(k).copied());
    out.push(first);
    out
}

/// `s` split at parameter `t` into the part before and the part after, both in its direction.
fn split_segment(s: &Segment, t: f64) -> (Segment, Segment) {
    let p = s.at(t);
    match *s {
        Segment::Line(l) => (Segment::Line(Line::new(l.a, p)), Segment::Line(Line::new(p, l.b))),
        Segment::Arc { arc, ccw } => {
            let a = arc.center.angle_to(p);
            let before = if ccw { Arc::new(arc.center, arc.radius, arc.start, a) } else { Arc::new(arc.center, arc.radius, a, arc.end) };
            let after = if ccw { Arc::new(arc.center, arc.radius, a, arc.end) } else { Arc::new(arc.center, arc.radius, arc.start, a) };
            (Segment::Arc { arc: before, ccw }, Segment::Arc { arc: after, ccw })
        }
    }
}

fn profile_moves(offset_pl: &Polyline, job: &CamJob, waste_left: bool) -> Vec<Move> {
    let safe_z = job.tool.safe_z;
    let total_depth = job.tool.depth;
    let step = if job.tool.step_down > 0.0 { job.tool.step_down } else { total_depth };
    let feed = job.tool.feed_rate;
    let plunge = job.tool.plunge_rate;

    let segs = path_segments(offset_pl);
    let (Some(first), Some(last)) = (segs.first(), segs.last()) else { return Vec::new() };
    let entry = first.start();
    let exit = last.end();

    // Tabs: arc-length ranges from the entry, kept on every pass that goes below their top.
    let tab_ranges = tab_ranges(offset_pl, &job.tabs);
    let tab_top = -total_depth + job.tabs.height;

    let lead_in = lead_in(entry, first.tangent(0.0), waste_left, &job.lead_in);
    let lead_out = lead_out(exit, last.tangent(1.0), waste_left, &job.lead_out);
    let approach = lead_in.as_ref().map_or(entry, |l| l.from);

    let mut moves = Vec::new();
    moves.push(Move::Rapid { x: approach.x, y: approach.y, z: safe_z });

    // Depth passes.
    let mut z = 0.0_f64;
    let mut pass = 0usize;
    let max_passes = ((total_depth / step).ceil() as usize).clamp(1, 1000);
    while pass < max_passes {
        z = (z - step).max(-total_depth);

        // Back over the approach point at safe Z before plunging (the retract left the tool
        // over the lead-out end): never a diagonal feed through the stock.
        if let Some(prev) = moves.last().map(Move::end_xy)
            && !prev.near(approach, 1e-9)
        {
            moves.push(Move::Rapid { x: approach.x, y: approach.y, z: safe_z });
        }
        moves.push(Move::Feed { x: approach.x, y: approach.y, z, f: plunge });
        if let Some(l) = &lead_in {
            moves.push(l.to_move(z, feed));
        }

        let tabs: &[(f64, f64)] = if z < tab_top - 1e-9 { &tab_ranges } else { &[] };
        append_profile_cut(&mut moves, &segs, z, feed, plunge, tabs, tab_top);

        if let Some(l) = &lead_out {
            moves.push(l.to_move(z, feed));
        }

        // Retract.
        let last_xy = moves.last().map(Move::end_xy).unwrap_or(entry);
        moves.push(Move::Rapid { x: last_xy.x, y: last_xy.y, z: safe_z });

        pass += 1;
        if z <= -total_depth + 1e-12 {
            break;
        }
    }

    moves
}

// ── Lead-in / lead-out ───────────────────────────────────────────────────────

/// A lead move from `from` to `to`: a straight feed, or a tangent arc about `centre`.
struct Lead {
    from: Vec2,
    to: Vec2,
    /// `Some((centre, ccw))` for an arc lead.
    arc: Option<(Vec2, bool)>,
}

impl Lead {
    fn to_move(&self, z: f64, f: f64) -> Move {
        let (x, y) = (self.to.x, self.to.y);
        match self.arc {
            None => Move::Feed { x, y, z, f },
            Some((c, ccw)) => {
                let (i, j) = (c.x - self.from.x, c.y - self.from.y);
                if ccw { Move::ArcCcw { x, y, z, i, j, f } } else { Move::ArcCw { x, y, z, i, j, f } }
            }
        }
    }
}

/// Unit normal to direction `t` pointing into the waste.
fn waste_normal(t: Vec2, waste_left: bool) -> Vec2 {
    if waste_left { t.perp() } else { -t.perp() }
}

/// The lead into `entry` (travel direction `t` there), on the waste side. An arc lead is a
/// quarter circle of radius `length` centred `length` into the waste from the entry, ending
/// tangent to the path; a line lead comes straight in along the waste normal.
fn lead_in(entry: Vec2, t: Vec2, waste_left: bool, lead: &crate::LeadParams) -> Option<Lead> {
    if lead.kind == LeadKind::None || !lead.length.is_finite() || lead.length <= 0.0 {
        return None;
    }
    let n = waste_normal(t, waste_left);
    let l = lead.length;
    match lead.kind {
        LeadKind::Arc => Some(Lead { from: entry + (n - t) * l, to: entry, arc: Some((entry + n * l, waste_left)) }),
        LeadKind::Line => Some(Lead { from: entry + n * l, to: entry, arc: None }),
        LeadKind::None => None,
    }
}

/// The lead away from `exit` (travel direction `t` there) into the waste: the mirror of
/// [`lead_in`], leaving tangent to the path.
fn lead_out(exit: Vec2, t: Vec2, waste_left: bool, lead: &crate::LeadParams) -> Option<Lead> {
    if lead.kind == LeadKind::None || !lead.length.is_finite() || lead.length <= 0.0 {
        return None;
    }
    let n = waste_normal(t, waste_left);
    let l = lead.length;
    match lead.kind {
        LeadKind::Arc => Some(Lead { from: exit, to: exit + (n + t) * l, arc: Some((exit + n * l, waste_left)) }),
        LeadKind::Line => Some(Lead { from: exit, to: exit + n * l, arc: None }),
        LeadKind::None => None,
    }
}

// ── Profile cutting with tabs ────────────────────────────────────────────────

/// Ranges along the profile (cumulative arc-length from the entry) where tabs are placed.
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

/// Emit the cut around the profile at depth `z`. Inside a tab range the tool rides at `tab_top`:
/// each segment is split exactly at the tab ends (lines stay lines, arcs stay arcs) and the tool
/// lifts and drops straight up and down there.
fn append_profile_cut(moves: &mut Vec<Move>, segs: &[Segment], z: f64, feed: f64, plunge: f64, tabs: &[(f64, f64)], tab_top: f64) {
    let in_tab = |at: f64| tabs.iter().any(|(a, b)| at > *a && at < *b);
    let mut cum_len = 0.0_f64;
    let mut cur_z = z;
    for s in segs {
        let seg_len = s.len();
        let (seg_start, seg_end) = (cum_len, cum_len + seg_len);
        cum_len = seg_end;
        if seg_len <= 0.0 {
            continue;
        }
        let mut cuts: Vec<f64> = vec![seg_start, seg_end];
        for (a, b) in tabs {
            cuts.extend([*a, *b].into_iter().filter(|u| *u > seg_start && *u < seg_end));
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
        for w in cuts.windows(2) {
            let (Some(&u0), Some(&u1)) = (w.first(), w.get(1)) else { continue };
            let piece_z = if in_tab((u0 + u1) / 2.0) { tab_top } else { z };
            let (t0, t1) = ((u0 - seg_start) / seg_len, (u1 - seg_start) / seg_len);
            let (p0, p1) = (s.at(t0), s.at(t1));
            if (piece_z - cur_z).abs() > 1e-12 {
                let f = if piece_z < cur_z { plunge } else { feed };
                moves.push(Move::Feed { x: p0.x, y: p0.y, z: piece_z, f });
                cur_z = piece_z;
            }
            match s {
                Segment::Line(_) => moves.push(Move::Feed { x: p1.x, y: p1.y, z: piece_z, f: feed }),
                Segment::Arc { arc, ccw } => {
                    let (i, j) = (arc.center.x - p0.x, arc.center.y - p0.y);
                    moves.push(if *ccw {
                        Move::ArcCcw { x: p1.x, y: p1.y, z: piece_z, i, j, f: feed }
                    } else {
                        Move::ArcCw { x: p1.x, y: p1.y, z: piece_z, i, j, f: feed }
                    });
                }
            }
        }
    }
    // Back to the pass depth before the lead-out.
    if (cur_z - z).abs() > 1e-12
        && let Some(p) = moves.last().map(Move::end_xy)
    {
        moves.push(Move::Feed { x: p.x, y: p.y, z, f: plunge });
    }
}

// ── CNC DXF helpers ──────────────────────────────────────────────────────────

/// Join open polylines whose end points coincide (within `tol`) into single polylines, closing a
/// chain that comes back to its start. Closed polylines pass through. Bulges are kept, so arcs
/// stay arcs; a piece joined end-to-end is reversed (vertex order and bulge signs). Blocks and
/// splines are the caller's to explode and flatten first.
pub fn clean_cnc_polylines(polylines: &[Polyline], tol: f64) -> Vec<Polyline> {
    use std::collections::HashMap;
    let tol = if tol.is_finite() && tol > 0.0 { tol } else { 1e-6 };
    let mut out = Vec::new();
    let mut open: Vec<Option<Polyline>> = Vec::new();
    for pl in polylines {
        if pl.closed || pl.vertices.len() < 2 {
            if !pl.vertices.is_empty() {
                out.push(pl.clone());
            }
        } else {
            open.push(Some(pl.clone()));
        }
    }
    // End points by grid cell of size `tol`, so finding a neighbour looks at nine cells.
    let cell = |p: Vec2| ((p.x / tol).floor() as i64, (p.y / tol).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, pl) in open.iter().enumerate() {
        for v in pl.iter().flat_map(|p| [p.vertices.first(), p.vertices.last()]).flatten() {
            grid.entry(cell(v.p)).or_default().push(i);
        }
    }
    // An unused open piece with an end at `p` (`true` when it is its last vertex).
    let find = |open: &[Option<Polyline>], p: Vec2| -> Option<(usize, bool)> {
        let (cx, cy) = cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &i in grid.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    let Some(Some(pl)) = open.get(i) else { continue };
                    if pl.vertices.first().is_some_and(|v| v.p.near(p, tol)) {
                        return Some((i, false));
                    }
                    if pl.vertices.last().is_some_and(|v| v.p.near(p, tol)) {
                        return Some((i, true));
                    }
                }
            }
        }
        None
    };
    for i in 0..open.len() {
        let Some(mut chain) = open.get_mut(i).and_then(Option::take) else { continue };
        // Grow forward from the end, then (reversed) from the start.
        for _ in 0..2 {
            while let Some(end) = chain.vertices.last().map(|v| v.p) {
                if chain.vertices.first().is_some_and(|v| v.p.near(end, tol)) && chain.vertices.len() > 2 {
                    break;
                }
                let Some((j, at_end)) = find(&open, end) else { break };
                let Some(next) = open.get_mut(j).and_then(Option::take) else { break };
                let next = if at_end { reversed(&next) } else { next };
                chain.vertices.pop();
                chain.vertices.extend(next.vertices);
            }
            chain = reversed(&chain);
        }
        let (first, last) = (chain.vertices.first().map(|v| v.p), chain.vertices.last().map(|v| v.p));
        if let (Some(f), Some(l)) = (first, last)
            && chain.vertices.len() > 2
            && f.near(l, tol)
        {
            chain.vertices.pop();
            chain.closed = true;
        }
        out.push(chain);
    }
    out
}

/// An open polyline run the other way: vertices in reverse order, each bulge moved to the
/// vertex that now starts its segment and negated.
fn reversed(pl: &Polyline) -> Polyline {
    let n = pl.vertices.len();
    let vertices = (0..n)
        .rev()
        .filter_map(|k| {
            let v = pl.vertices.get(k)?;
            let bulge = k.checked_sub(1).and_then(|p| pl.vertices.get(p)).map_or(0.0, |prev| -prev.bulge);
            Some(cadaddy_geom::PolyVertex { p: v.p, bulge, start_width: 0.0, end_width: 0.0 })
        })
        .collect();
    Polyline { vertices, closed: pl.closed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CamJob, LeadParams, OffsetSide, TabParams, ToolParams};
    use cadaddy_geom::PolyVertex;

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
        // First rapid (to entry) + 4 retract rapids = 5 rapids (no leads: each retract is
        // already over the entry, so no rapid back to it).
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

    // ── Rework regression tests: real geometry against known truth ───────────

    const EPS: f64 = 1e-6;

    fn square_cw() -> Polyline {
        let mut sq = square();
        sq.vertices.reverse();
        sq
    }

    fn circle_r10() -> Polyline {
        let b = cadaddy_geom::arc_to_bulge(std::f64::consts::FRAC_PI_2);
        Polyline {
            vertices: vec![
                PolyVertex::with_bulge(Vec2::new(10.0, 0.0), b),
                PolyVertex::with_bulge(Vec2::new(0.0, 10.0), b),
                PolyVertex::with_bulge(Vec2::new(-10.0, 0.0), b),
                PolyVertex::with_bulge(Vec2::new(0.0, -10.0), b),
            ],
            closed: true,
        }
    }

    fn one_path(profile: &Polyline, job: &CamJob) -> (Vec<Move>, Polyline) {
        let r = compute(std::slice::from_ref(profile), job).expect("toolpath");
        assert_eq!(r.paths.len(), 1);
        (r.paths[0].moves.clone(), r.offset_profiles[0].clone())
    }

    /// Each move with the XY it starts from.
    fn with_starts(moves: &[Move]) -> Vec<(Vec2, &Move)> {
        let mut prev = Vec2::ZERO;
        moves
            .iter()
            .map(|m| {
                let s = prev;
                prev = m.end_xy();
                (s, m)
            })
            .collect()
    }

    /// The cutting moves of a single-pass job: everything between the plunge and the retract.
    fn cut_moves(moves: &[Move]) -> Vec<(Vec2, Move)> {
        with_starts(moves)
            .into_iter()
            .filter(|(_, m)| !matches!(m, Move::Rapid { .. }) && m.end_z() < 0.0)
            .skip(1)
            .map(|(s, m)| (s, m.clone()))
            .collect()
    }

    fn dist_to(pl: &Polyline, p: Vec2) -> f64 {
        pl.segments().iter().map(|s| s.closest(p).dist(p)).fold(f64::INFINITY, f64::min)
    }

    fn arc_centre(start: Vec2, m: &Move) -> Option<(Vec2, bool)> {
        match *m {
            Move::ArcCw { i, j, .. } => Some((start + Vec2::new(i, j), false)),
            Move::ArcCcw { i, j, .. } => Some((start + Vec2::new(i, j), true)),
            _ => None,
        }
    }

    // MUST 1: lead arcs are true tangent arcs and every lead lies on the waste side.
    #[test]
    fn leads_are_tangent_arcs_on_the_waste_side() {
        for (name, profile) in [("ccw", square()), ("cw", square_cw())] {
            for side in [OffsetSide::Outside, OffsetSide::Inside] {
                for kind in [LeadKind::Arc, LeadKind::Line] {
                    let mut job = basic_job(side);
                    job.tool.step_down = 0.0;
                    job.lead_in = LeadParams { kind, length: 2.0 };
                    job.lead_out = LeadParams { kind, length: 2.0 };
                    let (moves, _) = one_path(&profile, &job);
                    let ctx = format!("{name} {side:?} {kind:?}");
                    let waste = |p: Vec2| {
                        let inside = crate::profile::contains_point(&profile, p);
                        if side == OffsetSide::Outside { !inside } else { inside }
                    };
                    let mut arcs = 0;
                    for (start, m) in with_starts(&moves) {
                        if let Some((c, _)) = arc_centre(start, m) {
                            arcs += 1;
                            let (r0, r1) = (start.dist(c), m.end_xy().dist(c));
                            assert!((r0 - r1).abs() < EPS, "{ctx}: arc start radius {r0} != end radius {r1}");
                            assert!(r0 > EPS, "{ctx}: arc centre is an end point");
                        }
                    }
                    // The lead starts where the tool plunges, and ends where it retracts.
                    let lead_start = moves[0].end_xy();
                    let lead_end = moves[moves.len() - 2].end_xy();
                    assert!(waste(lead_start), "{ctx}: lead-in start {lead_start:?} is not in the waste");
                    assert!(waste(lead_end), "{ctx}: lead-out end {lead_end:?} is not in the waste");
                    // The lead-in reaches the path tangentially: lead arc radius = L, its end on the path.
                    let lead_in = &moves[2];
                    if kind == LeadKind::Arc {
                        let (c, _) = arc_centre(lead_start, lead_in).expect("lead-in is an arc");
                        assert!((lead_start.dist(c) - 2.0).abs() < EPS, "{ctx}: lead radius");
                        // Tangent at the end: radius ⟂ the first cut direction.
                        let entry = lead_in.end_xy();
                        let next = moves[3].end_xy();
                        assert!((entry - c).dot(next - entry).abs() < EPS, "{ctx}: lead-in arc not tangent");
                        assert_eq!(arcs, 2 + if side == OffsetSide::Outside { 4 } else { 0 }, "{ctx}: arc count");
                    }
                }
            }
        }
    }

    // MUST 2: every pass after the first goes back over the lead-in start at safe Z; every
    // downward feed is straight down.
    #[test]
    fn every_plunge_is_vertical_over_the_entry() {
        let mut job = basic_job(OffsetSide::Outside);
        job.tool.depth = 4.0;
        job.tool.step_down = 2.0;
        job.lead_in = LeadParams { kind: LeadKind::Arc, length: 2.0 };
        job.lead_out = LeadParams { kind: LeadKind::Arc, length: 2.0 };
        let (moves, _) = one_path(&square(), &job);
        let approach = moves[0].end_xy();
        let mut plunges = 0;
        let mut prev: Option<&Move> = None;
        for m in &moves {
            if let (Some(p), Move::Feed { x, y, z, f }) = (prev, m)
                && *z < p.end_z() - 1e-12
            {
                plunges += 1;
                assert!(p.end_xy().near(Vec2::new(*x, *y), 1e-12), "diagonal plunge {p:?} -> {m:?}");
                assert_eq!(*f, job.tool.plunge_rate);
                assert!(Vec2::new(*x, *y).near(approach, 1e-12), "plunge away from the lead-in start: {m:?}");
                assert!(matches!(p, Move::Rapid { z, .. } if (*z - job.tool.safe_z).abs() < 1e-12), "plunge not from safe Z: {p:?}");
            }
            prev = Some(m);
        }
        assert_eq!(plunges, 2, "one plunge per pass");
    }

    /// Arc-length position of `q` along the tool path (from the entry), if it lies on it.
    fn position_on(segs: &[Segment], q: Vec2) -> Option<f64> {
        let mut cum = 0.0;
        for s in segs {
            if s.closest(q).dist(q) < 1e-7 {
                let along = match *s {
                    Segment::Line(l) => l.a.dist(q),
                    Segment::Arc { arc, ccw } => {
                        let (a0, a1) = (arc.center.angle_to(s.start()), arc.center.angle_to(q));
                        let sweep = if ccw { a1 - a0 } else { a0 - a1 };
                        sweep.rem_euclid(std::f64::consts::TAU) * arc.radius
                    }
                };
                return Some(cum + along);
            }
            cum += s.len();
        }
        None
    }

    // MUST 3: tabs survive every pass below their top, not only the last.
    #[test]
    fn tabs_are_kept_on_every_pass_below_the_tab_top() {
        let mut job = basic_job(OffsetSide::Outside);
        job.tool.depth = 6.0;
        job.tool.step_down = 1.0;
        job.tabs = TabParams { width: 2.0, height: 2.0, count: 4 };
        let (moves, offset) = one_path(&square(), &job);
        let segs = path_segments(&offset);
        let ranges = tab_ranges(&offset, &job.tabs);
        assert_eq!(ranges.len(), 4);
        let in_tab = |u: f64| ranges.iter().any(|(a, b)| u > a + 1e-7 && u < b - 1e-7);
        let mut checked = 0;
        for (start, m) in with_starts(&moves) {
            if matches!(m, Move::Rapid { .. }) || m.end_z() >= -4.0 - 1e-9 {
                continue;
            }
            let mut probes = vec![m.end_xy()];
            if matches!(m, Move::Feed { .. }) {
                probes.push(start.lerp(m.end_xy(), 0.5));
            }
            for q in probes {
                if let Some(u) = position_on(&segs, q) {
                    checked += 1;
                    assert!(!in_tab(u), "move {m:?} cuts below Z -4 inside a tab at {u}");
                }
            }
        }
        assert!(checked > 20, "the probe found the path ({checked})");
        // The passes at -5 and -6 both lift straight up to -4 once per tab; no other pass lifts.
        for pass_z in [-5.0, -6.0] {
            let lifts = moves
                .windows(2)
                .filter(|w| (w[0].end_z() - pass_z).abs() < 1e-9 && (w[1].end_z() + 4.0).abs() < 1e-9 && w[0].end_xy().near(w[1].end_xy(), 1e-12))
                .filter(|w| matches!(w[1], Move::Feed { .. }))
                .count();
            assert_eq!(lifts, 4, "lifts on the pass at {pass_z}");
        }
        let all_lifts = moves.windows(2).filter(|w| matches!(w[1], Move::Feed { .. }) && w[1].end_z() > w[0].end_z() + 1e-12).count();
        assert_eq!(all_lifts, 8, "2 passes below the tab top x 4 tabs");
    }

    // MUST 4: offsets against known geometry.
    #[test]
    fn outside_square_is_offset_by_the_tool_radius_with_corner_arcs() {
        let mut job = basic_job(OffsetSide::Outside);
        job.tool.step_down = 0.0;
        let sq = square();
        let (moves, _) = one_path(&sq, &job);
        let cuts = cut_moves(&moves);
        let corners = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        let (mut lines, mut arcs, mut corners_hit) = (0, 0, [false; 4]);
        for (start, m) in &cuts {
            let end = m.end_xy();
            assert!((dist_to(&sq, *start) - 3.0).abs() < EPS && (dist_to(&sq, end) - 3.0).abs() < EPS, "{m:?} not 3 from the square");
            match arc_centre(*start, m) {
                None => {
                    lines += 1;
                    // Straight and parallel to a square edge, outside it.
                    assert!((start.x - end.x).abs() < EPS || (start.y - end.y).abs() < EPS, "{m:?} not parallel");
                    let mid = start.lerp(end, 0.5);
                    assert!((dist_to(&sq, mid) - 3.0).abs() < EPS);
                    assert!(!crate::profile::contains_point(&sq, mid));
                }
                Some((c, _)) => {
                    arcs += 1;
                    let k = corners.iter().position(|k| k.near(c, EPS)).unwrap_or_else(|| panic!("arc centre {c:?} is not a corner"));
                    corners_hit[k] = true;
                    assert!((start.dist(c) - 3.0).abs() < EPS && (end.dist(c) - 3.0).abs() < EPS, "corner arc radius");
                }
            }
        }
        assert_eq!((lines, arcs), (5, 4), "4 edges (one split at the entry) and 4 corner arcs");
        assert!(corners_hit.iter().all(|h| *h));
    }

    fn assert_square_path(cuts: &[(Vec2, Move)], lo: f64, hi: f64) {
        let on_edge = |p: Vec2| {
            let inside = p.x > lo - EPS && p.x < hi + EPS && p.y > lo - EPS && p.y < hi + EPS;
            let edge = (p.x - lo).abs() < EPS || (p.x - hi).abs() < EPS || (p.y - lo).abs() < EPS || (p.y - hi).abs() < EPS;
            inside && edge
        };
        let mut seen = [false; 4];
        for (start, m) in cuts {
            assert!(matches!(m, Move::Feed { .. }), "no arcs on a square path: {m:?}");
            let end = m.end_xy();
            assert!(on_edge(*start) && on_edge(end) && on_edge(start.lerp(end, 0.5)), "{start:?} -> {end:?} off the {lo}..{hi} square");
            for (k, c) in [(lo, lo), (hi, lo), (hi, hi), (lo, hi)].into_iter().enumerate() {
                seen[k] |= end.near(Vec2::new(c.0, c.1), EPS);
            }
        }
        assert!(seen.iter().all(|s| *s), "every corner of the {lo}..{hi} square is visited");
    }

    #[test]
    fn inside_square_is_the_4x4_square() {
        let mut job = basic_job(OffsetSide::Inside);
        job.tool.step_down = 0.0;
        let (moves, _) = one_path(&square(), &job);
        assert_square_path(&cut_moves(&moves), 3.0, 7.0);
    }

    #[test]
    fn on_square_is_the_original() {
        let mut job = basic_job(OffsetSide::On);
        job.tool.step_down = 0.0;
        let (moves, _) = one_path(&square(), &job);
        assert_square_path(&cut_moves(&moves), 0.0, 10.0);
    }

    #[test]
    fn outside_circle_r10_gives_arcs_r13() {
        let mut job = basic_job(OffsetSide::Outside);
        job.tool.step_down = 0.0;
        let (moves, _) = one_path(&circle_r10(), &job);
        let cuts = cut_moves(&moves);
        assert!(cuts.len() >= 4, "{moves:#?}");
        let mut sweep = 0.0;
        for (start, m) in &cuts {
            let (c, ccw) = arc_centre(*start, m).unwrap_or_else(|| panic!("not an arc: {m:?}"));
            assert!(c.near(Vec2::ZERO, EPS), "centre {c:?}");
            assert!((start.dist(c) - 13.0).abs() < EPS && (m.end_xy().dist(c) - 13.0).abs() < EPS, "radius 13");
            assert!(ccw, "the circle runs CCW");
            sweep += (c.angle_to(m.end_xy()) - c.angle_to(*start)).rem_euclid(std::f64::consts::TAU);
        }
        assert!((sweep - std::f64::consts::TAU).abs() < EPS, "a full turn, got {sweep}");
    }

    // MUST 7 (crate half): open pieces meeting end to end are joined, closed when they loop.
    #[test]
    fn clean_cnc_polylines_joins_touching_pieces() {
        let p = |x: f64, y: f64| PolyVertex::new(Vec2::new(x, y));
        let pieces = vec![
            Polyline { vertices: vec![p(0.0, 0.0), p(10.0, 0.0)], closed: false },
            Polyline { vertices: vec![p(10.0, 10.0), p(0.0, 10.0)], closed: false },
            // Runs the other way: joined reversed.
            Polyline { vertices: vec![p(10.0, 10.0), p(10.0, 0.0)], closed: false },
            Polyline { vertices: vec![p(0.0, 10.0), p(0.0, 0.0)], closed: false },
            Polyline { vertices: vec![p(50.0, 0.0), PolyVertex::with_bulge(Vec2::new(60.0, 0.0), 1.0), p(70.0, 0.0)], closed: false },
            Polyline { vertices: vec![p(80.0, 0.0), p(70.0, 0.0)], closed: false },
        ];
        let out = clean_cnc_polylines(&pieces, 1e-6);
        assert_eq!(out.len(), 2, "{out:?}");
        let rect = out.iter().find(|pl| pl.closed).expect("closed rectangle");
        assert_eq!(rect.vertices.len(), 4);
        assert!((signed_area(rect).abs() - 100.0).abs() < EPS);
        let open = out.iter().find(|pl| !pl.closed).expect("open chain");
        assert_eq!(open.vertices.len(), 4);
        // The bulge (a half circle 60 → 70) is kept on its segment whichever way the chain runs.
        let len: f64 = open.segments().iter().map(Segment::len).sum();
        assert!((len - (10.0 + 5.0 * std::f64::consts::PI + 10.0)).abs() < EPS, "chain length {len}");
    }
}
