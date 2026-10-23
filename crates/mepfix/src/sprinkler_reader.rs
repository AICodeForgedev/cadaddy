//! Sprinkler reader: heads and branch lines from the fire protection plans, checked against the
//! reflected ceiling plans, with each head's height from its ceiling.
//!
//! - **Heads** are found by their symbols, by geometry alone: a small circle (pendent: with a cross
//!   or a filled dot in it; upright: a circle in a circle) or a half circle on a flat line
//!   (sidewall). Symbol sizes are paper sizes, so the sheet's recognised scale sets the radius
//!   looked for. Legend symbols, outside the building, are left out.
//! - **Lines**: the plan's pipe lines, joined end to end where they continue straight, split
//!   where another line or a head joins them. Each head joins the nearest line: on it (a drop),
//!   or at the end of a short arm (an armover).
//! - **Sizes**: a size tag beside the piece (`1-1/4"`), else the sheet's pipe schedule (largest
//!   heads count each size may carry) applied to the heads downstream of the piece.
//! - **Heights**: pendent heads sit at their room's ceiling (from the ceiling reader); sidewall
//!   heads below it by the head schedule's `N" BELOW CEILING`; upright heads above their line's
//!   centreline (from its `CL @` / `BOP @` note) by the schedule's `N" SPRIG`.
//!
//! Everything comes from the sheets and the sheet map (Q255); the truth is only used to score.

use crate::ceiling_reader::CeilingResult;
use crate::sheet_reader::{Alignment, RecognisedSheet, SheetMap, building_from_model};
use crate::truth::{Truth, fmt_in};
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;

/// Most heads, lines and texts read from one sheet (hostile drawings).
const MAX_ITEMS: usize = 50_000;
/// Head symbol radius on paper, inches, and how far off it may be.
const HEAD_R_IN: f64 = 0.045;
const HEAD_R_TOL: f64 = 0.15;

/// A head found on a fire protection plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundHead {
    pub level: String,
    pub at_ft: [f64; 2],
    /// `pendent`, `upright`, `sidewall`.
    pub mount: String,
    /// Height above the level's floor, feet, when it could be worked out.
    pub height_ft: Option<f64>,
    /// Where the height came from: `ceiling`, `ceiling-offset`, `line+sprig`.
    pub height_source: String,
    /// The room it is in (from the ceiling reader's rooms).
    pub room: Option<String>,
    /// The line it joins (index into [`SprinklerResult::lines`]).
    pub line: Option<usize>,
    /// Armover length, feet (0 for a head right on its line).
    pub arm_ft: f64,
    /// The reflected ceiling plan of the level shows a head here too.
    pub on_rcp: bool,
}

/// A straight piece of sprinkler pipe between joints.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundPiece {
    pub level: String,
    pub from_ft: [f64; 2],
    pub to_ft: [f64; 2],
    pub size_in: Option<f64>,
    /// `tag` or `schedule` (or empty when neither gave one).
    pub size_source: String,
    /// Heads downstream of the piece (away from the mains).
    pub heads_downstream: usize,
    /// Centreline height above the floor from a note on its line, feet.
    pub centre_ft: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SprinklerResult {
    pub heads: Vec<FoundHead>,
    pub lines: Vec<FoundPiece>,
    /// The pipe schedule read from the sheets: size (inches), most heads.
    pub pipe_schedule: Vec<(f64, usize)>,
    /// Sprig length and the sidewall offset below the ceiling, inches, from the head schedule.
    pub sprig_in: Option<f64>,
    pub sidewall_below_in: Option<f64>,
}

// ---- Parsing helpers --------------------------------------------------------------------------

/// A nominal pipe size as printed: `1"`, `1-1/4"`, `3/4"`, `2-1/2"` (the inch mark optional).
pub fn parse_pipe_size(s: &str) -> Option<f64> {
    let t = s.trim().trim_end_matches('"').trim();
    if t.is_empty() || t.len() > 8 {
        return None;
    }
    let frac = |f: &str| -> Option<f64> {
        let (a, b) = f.split_once('/')?;
        let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
        (b > 0.0).then(|| a / b)
    };
    let v = if let Some((w, f)) = t.split_once('-') {
        w.parse::<f64>().ok()? + frac(f)?
    } else if t.contains('/') {
        frac(t)?
    } else {
        t.parse::<f64>().ok()?
    };
    (v > 0.0 && v <= 12.0).then_some(v)
}

/// Feet and inches (`9'-3"`, `10'-7 1/2"`) → feet.
pub fn parse_feet_inches(s: &str) -> Option<f64> {
    let s = s.trim();
    let (f, rest) = s.split_once('\'')?;
    let feet = f.trim().parse::<f64>().ok()?;
    let inch_part = rest.trim().trim_start_matches('-').trim().trim_end_matches('"').trim();
    let mut inches = 0.0;
    for part in inch_part.split_whitespace() {
        inches += if let Some((a, b)) = part.split_once('/') {
            let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
            if b == 0.0 {
                return None;
            }
            a / b
        } else {
            part.parse::<f64>().ok()?
        };
    }
    Some(feet + inches / 12.0)
}

/// A pipe height note → (centreline above the floor, feet) given the pipe's outside diameter.
fn parse_pipe_note(s: &str, od_in: f64) -> Option<f64> {
    let u = s.to_uppercase();
    let (key, rest) = ["B.O.P.", "BOP @", "CL @", "C.L.", "CTR"].iter().find_map(|k| u.find(k).map(|i| (*k, u[i + k.len()..].to_string())))?;
    let v = parse_feet_inches(rest.trim_start_matches(['@', ' ']).trim_end_matches("AFF").trim())?;
    Some(if key.starts_with('B') { v + od_in / 24.0 } else { v })
}

/// The number before `" SPRIG` or `" BELOW CEILING` in a schedule row.
fn inches_before(s: &str, what: &str) -> Option<f64> {
    let u = s.to_uppercase();
    let i = u.find(what)?;
    let head = u.get(..i)?.trim_end().trim_end_matches('"');
    let num: String = head.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
    num.parse::<f64>().ok()
}

// ---- Drawing contents ---------------------------------------------------------------------------

struct Contents {
    circles: Vec<(Vec2, f64)>,
    arcs: Vec<(Vec2, f64, f64, f64)>,
    solids: Vec<Vec2>,
    lines: Vec<(Vec2, Vec2, String)>,
    texts: Vec<(String, Vec2)>,
}

fn contents(d: &Drawing) -> Contents {
    let mut c = Contents { circles: Vec::new(), arcs: Vec::new(), solids: Vec::new(), lines: Vec::new(), texts: Vec::new() };
    for e in d.model.iter().take(MAX_ITEMS * 4) {
        match &e.kind {
            EntityKind::Circle(x) => c.circles.push((x.center.xy(), x.radius)),
            EntityKind::Arc(x) => c.arcs.push((x.center.xy(), x.radius, x.start, x.end)),
            EntityKind::Solid(x) => {
                let s = x.corners.iter().fold(Vec2::new(0.0, 0.0), |a, p| a + p.xy());
                c.solids.push(s * 0.25);
            }
            EntityKind::Line(l) => c.lines.push((l.a.xy(), l.b.xy(), e.common.layer.to_uppercase())),
            EntityKind::LwPolyline(p) => {
                for w in p.vertices.windows(2) {
                    if let [a, b] = w {
                        c.lines.push((a.p, b.p, e.common.layer.to_uppercase()));
                    }
                }
            }
            EntityKind::Text(t) => {
                let at = t.align_pt.map_or(t.insert.xy(), |p| p.xy());
                c.texts.push((t.value.clone(), at));
            }
            EntityKind::MText(t) => c.texts.push((t.contents.clone(), t.insert.xy())),
            _ => {}
        }
    }
    c
}

/// Model inches per paper inch on a recognised sheet.
fn model_per_paper(sh: &RecognisedSheet) -> f64 {
    if sh.scale > 0.0 { 12.0 / sh.scale } else { 96.0 }
}

/// Head symbols on a sheet: (model centre, mount).
fn head_symbols(c: &Contents, k: f64) -> Vec<(Vec2, &'static str)> {
    let r0 = HEAD_R_IN * k;
    let near = |r: f64| (r - r0).abs() <= r0 * HEAD_R_TOL;
    let mut out = Vec::new();
    for (p, r) in c.circles.iter().take(MAX_ITEMS) {
        if !near(*r) {
            continue;
        }
        let inner = c.circles.iter().any(|(q, s)| (*q - *p).len() < r0 * 0.05 && (*s - r0 * 0.45).abs() < r0 * 0.1);
        if inner {
            out.push((*p, "upright"));
            continue;
        }
        // Pendent: a filled dot, or a cross of two lines through the centre the circle's size.
        let dot = c.solids.iter().any(|q| (*q - *p).len() < r0 * 0.1);
        let cross =
            c.lines.iter().filter(|(a, b, _)| ((*a + *b) * 0.5 - *p).len() < r0 * 0.05 && ((*a - *b).len() - 2.0 * r0).abs() < r0 * 0.2).count() >= 2;
        if dot || cross {
            out.push((*p, "pendent"));
        }
    }
    for (p, r, s, e) in c.arcs.iter().take(MAX_ITEMS) {
        let sweep = (e - s).rem_euclid(std::f64::consts::TAU);
        if near(*r) && (sweep - std::f64::consts::PI).abs() < 0.05 {
            let flat = c.lines.iter().any(|(a, b, _)| ((*a + *b) * 0.5 - *p).len() < r0 * 0.05 && ((*a - *b).len() - 2.0 * r0).abs() < r0 * 0.2);
            if flat {
                out.push((*p, "sidewall"));
            }
        }
    }
    out
}

// ---- Lines --------------------------------------------------------------------------------------

/// Straight lines from the pipe segments: collinear touching segments joined.
fn straight_lines(segs: &[([f64; 2], [f64; 2])]) -> Vec<([f64; 2], [f64; 2])> {
    let mut lines: Vec<([f64; 2], [f64; 2])> = segs.iter().filter(|(a, b)| dist(*a, *b) > 1e-3).copied().collect();
    let tol = 0.05;
    let mut merged = true;
    let mut guard = 0;
    while merged && guard < 10_000 {
        merged = false;
        guard += 1;
        'outer: for i in 0..lines.len() {
            for j in i + 1..lines.len() {
                let (Some(&(a0, a1)), Some(&(b0, b1))) = (lines.get(i), lines.get(j)) else { continue };
                if !collinear(a0, a1, b0, tol) || !collinear(a0, a1, b1, tol) {
                    continue;
                }
                // Touching or overlapping along the shared direction.
                let u = unit(a0, a1);
                let t = |p: [f64; 2]| (p[0] - a0[0]) * u[0] + (p[1] - a0[1]) * u[1];
                let (amin, amax) = (0.0f64, t(a1));
                let (bmin, bmax) = (t(b0).min(t(b1)), t(b0).max(t(b1)));
                if bmin > amax + tol || bmax < amin - tol {
                    continue;
                }
                let lo = amin.min(bmin);
                let hi = amax.max(bmax);
                let p = |s: f64| [a0[0] + u[0] * s, a0[1] + u[1] * s];
                if let Some(slot) = lines.get_mut(i) {
                    *slot = (p(lo), p(hi));
                }
                lines.swap_remove(j);
                merged = true;
                break 'outer;
            }
        }
    }
    lines
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn unit(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let l = dist(a, b).max(1e-12);
    [(b[0] - a[0]) / l, (b[1] - a[1]) / l]
}

fn collinear(a: [f64; 2], b: [f64; 2], p: [f64; 2], tol: f64) -> bool {
    let u = unit(a, b);
    ((p[0] - a[0]) * -u[1] + (p[1] - a[1]) * u[0]).abs() < tol
}

/// Where two segments cross inside both (not at their ends).
fn crossing(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> Option<[f64; 2]> {
    let r = [b[0] - a[0], b[1] - a[1]];
    let s = [d[0] - c[0], d[1] - c[1]];
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() < 1e-12 {
        return None;
    }
    let t = ((c[0] - a[0]) * s[1] - (c[1] - a[1]) * s[0]) / den;
    let u = ((c[0] - a[0]) * r[1] - (c[1] - a[1]) * r[0]) / den;
    (t > 1e-6 && t < 1.0 - 1e-6 && u > 1e-6 && u < 1.0 - 1e-6).then(|| [a[0] + r[0] * t, a[1] + r[1] * t])
}

/// Distance from p to segment a–b and the position along it (0..len).
fn project(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> (f64, f64) {
    let u = unit(a, b);
    let len = dist(a, b);
    let s = ((p[0] - a[0]) * u[0] + (p[1] - a[1]) * u[1]).clamp(0.0, len);
    let q = [a[0] + u[0] * s, a[1] + u[1] * s];
    (dist(p, q), s)
}

// ---- Reading ------------------------------------------------------------------------------------

/// Read the sprinkler heads and lines of every fire protection plan in a set.
pub fn read_sprinklers(dxf_data: &[(String, Vec<u8>)], map: &SheetMap, ceilings: &CeilingResult) -> SprinklerResult {
    let mut res = SprinklerResult::default();
    let drawings: Vec<(&RecognisedSheet, Drawing)> = dxf_data
        .iter()
        .filter_map(|(name, bytes)| {
            let stem = name.trim_end_matches(".dxf");
            let sh = map.sheets.iter().find(|s| s.number == stem)?;
            Some((sh, cadaddy_io::read_dxf(bytes).ok()?))
        })
        .collect();
    let (gx, gy) = (map.grid_x.iter().map(|g| g.at_ft), map.grid_y.iter().map(|g| g.at_ft));
    let (x0, x1) = gx.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v), b.max(v)));
    let ys: Vec<f64> = gy.collect();
    let (y0, y1) = ys.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(*v), b.max(*v)));
    let inside = |p: [f64; 2]| !x0.is_finite() || (p[0] > x0 - 1.0 && p[0] < x1 + 1.0 && p[1] > y0 - 1.0 && p[1] < y1 + 1.0);

    // Schedules first (any FP sheet): the pipe schedule, sprig and sidewall offsets.
    for (sh, d) in &drawings {
        if sh.discipline != "FP" {
            continue;
        }
        let c = contents(d);
        if res.pipe_schedule.is_empty() {
            res.pipe_schedule = pipe_schedule(&c);
        }
        for (t, _) in &c.texts {
            res.sprig_in = res.sprig_in.or_else(|| inches_before(t, "\" SPRIG"));
            res.sidewall_below_in = res.sidewall_below_in.or_else(|| inches_before(t, "\" BELOW CEILING"));
        }
    }
    // Heads on the reflected ceiling plans, to cross-check.
    let mut rcp_heads: Vec<(String, [f64; 2])> = Vec::new();
    for (sh, d) in &drawings {
        if sh.discipline != "A" || !sh.title.to_uppercase().contains("CEILING") {
            continue;
        }
        let (Some(level), Some(al)) = (&sh.level, &sh.alignment) else { continue };
        let c = contents(d);
        for (p, _) in head_symbols(&c, model_per_paper(sh)) {
            let b = building_from_model(al, [p.x, p.y]);
            if inside(b) {
                rcp_heads.push((level.clone(), b));
            }
        }
    }
    for (sh, d) in &drawings {
        // The level fire protection plans (not schedules).
        if sh.discipline != "FP" || !sh.title.to_uppercase().contains("PLAN") {
            continue;
        }
        let (Some(level), Some(al)) = (&sh.level, &sh.alignment) else { continue };
        read_sheet(&mut res, sh, d, level, al, &inside, &rcp_heads, ceilings);
    }
    res
}

fn pipe_schedule(c: &Contents) -> Vec<(f64, usize)> {
    let Some((_, title)) = c.texts.iter().find(|(t, _)| t.trim().eq_ignore_ascii_case("PIPE SCHEDULE")) else { return Vec::new() };
    // Rows under the title: a size cell and a count cell at the same height, close to the title.
    let mut rows: Vec<(f64, f64, usize)> = Vec::new();
    for (t, p) in &c.texts {
        let Some(size) = parse_pipe_size(t).filter(|_| t.contains('"')) else { continue };
        if p.y > title.y || (p.x - title.x).abs() > (title.y - p.y).max(1.0) * 4.0 {
            continue;
        }
        let count = c.texts.iter().filter(|(_, q)| (q.y - p.y).abs() < 1e-6 && q.x > p.x).filter_map(|(u, _)| u.trim().parse::<usize>().ok()).next();
        if let Some(n) = count {
            rows.push((p.y, size, n));
        }
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out: Vec<(f64, usize)> = rows.into_iter().map(|(_, s, n)| (s, n)).collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);
    out
}

#[allow(clippy::too_many_arguments)]
fn read_sheet(
    res: &mut SprinklerResult,
    sh: &RecognisedSheet,
    d: &Drawing,
    level: &str,
    al: &Alignment,
    inside: &dyn Fn([f64; 2]) -> bool,
    rcp: &[(String, [f64; 2])],
    ceilings: &CeilingResult,
) {
    let c = contents(d);
    let k = model_per_paper(sh);
    let to_b = |p: Vec2| building_from_model(al, [p.x, p.y]);
    // Pipe lines, in building feet (riser and rise/drop circles aren't lines).
    let segs: Vec<([f64; 2], [f64; 2])> = c
        .lines
        .iter()
        .filter(|(_, _, l)| matches!(l.as_str(), "PIPE" | "HEAVY"))
        .map(|(a, b, _)| (to_b(*a), to_b(*b)))
        .filter(|(a, b)| inside(*a) && inside(*b))
        .take(MAX_ITEMS)
        .collect();
    // Symbol strokes (head crosses, sidewall flats) are on their own layer; only pipe lines here.
    let lines = straight_lines(&segs);
    // Heads.
    let mut heads: Vec<(usize, [f64; 2], &str)> = Vec::new();
    for (p, mount) in head_symbols(&c, k) {
        let b = to_b(p);
        if inside(b) {
            heads.push((heads.len(), b, mount));
        }
    }
    // Texts in building feet.
    let texts: Vec<(String, [f64; 2])> = c.texts.iter().map(|(t, p)| (t.clone(), to_b(*p))).filter(|(_, p)| inside(*p)).collect();
    let text_ft = sh.scale.max(1e-6).recip() * 0.12; // a text height or so, feet

    // The riser (a circle of the riser symbol's size) is where the water comes from.
    let riser_r = 0.07 * k;
    let risers: Vec<[f64; 2]> =
        c.circles.iter().filter(|(_, r)| (*r - riser_r).abs() <= riser_r * HEAD_R_TOL).map(|(p, _)| to_b(*p)).filter(|p| inside(*p)).collect();

    // Split the lines into pieces at every joint: where another line meets or crosses them, at
    // the riser and at each head's foot (a head right on a line).
    let mut cut_points: Vec<[f64; 2]> = risers.clone();
    for (i, (a, b)) in lines.iter().enumerate() {
        for (j, (c0, c1)) in lines.iter().enumerate() {
            if i == j {
                continue;
            }
            for p in [c0, c1] {
                if project(*a, *b, *p).0 < 0.1 {
                    cut_points.push(*p);
                }
            }
            if let Some(x) = crossing(*a, *b, *c0, *c1) {
                cut_points.push(x);
            }
        }
    }
    for (_, p, _) in &heads {
        if let Some((a, b)) = lines.iter().min_by(|x, y| project(x.0, x.1, *p).0.total_cmp(&project(y.0, y.1, *p).0))
            && project(*a, *b, *p).0 < 0.2
        {
            let (_, s) = project(*a, *b, *p);
            let u = unit(*a, *b);
            cut_points.push([a[0] + u[0] * s, a[1] + u[1] * s]);
        }
    }
    let mut pieces: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for (a, b) in &lines {
        let len = dist(*a, *b);
        let mut cuts: Vec<f64> = vec![0.0, len];
        cuts.extend(cut_points.iter().filter(|p| project(*a, *b, **p).0 < 0.1).map(|p| project(*a, *b, *p).1));
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|x, y| (*x - *y).abs() < 0.05);
        let u = unit(*a, *b);
        let at = |s: f64| [a[0] + u[0] * s, a[1] + u[1] * s];
        for w in cuts.windows(2) {
            if let [s0, s1] = w
                && s1 - s0 >= 0.05
            {
                pieces.push((at(*s0), at(*s1)));
            }
        }
    }
    pieces.truncate(MAX_ITEMS);
    // Nodes: piece ends merged.
    let mut nodes: Vec<[f64; 2]> = Vec::new();
    let node_of = |p: [f64; 2], nodes: &mut Vec<[f64; 2]>| -> usize {
        if let Some(i) = nodes.iter().position(|q| dist(*q, p) < 0.05) {
            return i;
        }
        nodes.push(p);
        nodes.len() - 1
    };
    let edges: Vec<(usize, usize)> = pieces.iter().map(|(a, b)| (node_of(*a, &mut nodes), node_of(*b, &mut nodes))).collect();
    // Heads at nodes: a head on a line sits at its foot's node; an armover's at the arm's end.
    let head_node: Vec<Option<usize>> = heads
        .iter()
        .map(|(_, p, _)| {
            nodes.iter().enumerate().map(|(i, q)| (i, dist(*q, *p))).filter(|(_, d)| *d < 0.2).min_by(|x, y| x.1.total_cmp(&y.1)).map(|x| x.0)
        })
        .collect();
    let mut heads_at = vec![0usize; nodes.len()];
    for n in head_node.iter().flatten() {
        if let Some(h) = heads_at.get_mut(*n) {
            *h += 1;
        }
    }
    // Walk the tree from the riser (else from the end of the longest piece): each piece's far
    // node, and the heads beyond it.
    let root = risers
        .first()
        .and_then(|r| nodes.iter().enumerate().map(|(i, q)| (i, dist(*q, *r))).min_by(|x, y| x.1.total_cmp(&y.1)).map(|x| x.0))
        .or_else(|| edges.iter().zip(&pieces).max_by(|x, y| dist(x.1.0, x.1.1).total_cmp(&dist(y.1.0, y.1.1))).map(|(e, _)| e.0))
        .unwrap_or(0);
    let mut parent_edge: Vec<Option<usize>> = vec![None; nodes.len()];
    let mut order = vec![root];
    let mut seen = vec![false; nodes.len()];
    if let Some(s) = seen.get_mut(root) {
        *s = true;
    }
    let mut k_i = 0;
    while let Some(&n) = order.get(k_i) {
        k_i += 1;
        for (e, (a, b)) in edges.iter().enumerate() {
            let other = if *a == n {
                *b
            } else if *b == n {
                *a
            } else {
                continue;
            };
            if seen.get(other).copied().unwrap_or(true) {
                continue;
            }
            if let Some(s) = seen.get_mut(other) {
                *s = true;
            }
            if let Some(pe) = parent_edge.get_mut(other) {
                *pe = Some(e);
            }
            order.push(other);
        }
    }
    // Heads beyond each node, leaves first.
    let mut beyond = heads_at.clone();
    for &n in order.iter().rev() {
        let Some(Some(e)) = parent_edge.get(n) else { continue };
        let Some((a, b)) = edges.get(*e) else { continue };
        let up = if *a == n { *b } else { *a };
        let here = beyond.get(n).copied().unwrap_or(0);
        if let Some(u) = beyond.get_mut(up) {
            *u += here;
        }
    }
    // Size tags and height notes: each goes to the piece it stands nearest.
    let near_piece = |p: [f64; 2]| {
        pieces
            .iter()
            .enumerate()
            .map(|(i, (a, b))| (i, project(*a, *b, p)))
            .filter(|(_, (_, s))| *s > 0.02)
            .min_by(|x, y| x.1.0.total_cmp(&y.1.0))
            .filter(|(_, (d, _))| *d < text_ft * 12.0)
            .map(|x| x.0)
    };
    let mut tag_of: Vec<Option<f64>> = vec![None; pieces.len()];
    let mut note_of: Vec<Option<String>> = vec![None; pieces.len()];
    for (t, p) in &texts {
        let Some(i) = near_piece(*p) else { continue };
        if let Some(v) = parse_pipe_size(t).filter(|_| t.trim().ends_with('"') && !t.contains('@')) {
            if let Some(slot) = tag_of.get_mut(i) {
                slot.get_or_insert(v);
            }
        } else if parse_pipe_note(t, 1.0).is_some()
            && let Some(slot) = note_of.get_mut(i)
        {
            slot.get_or_insert(t.clone());
        }
    }
    let base = res.lines.len();
    for (i, ((a, b), (na, nb))) in pieces.iter().zip(&edges).enumerate() {
        // Downstream of a piece: everything beyond its far node (away from the riser).
        let far = if parent_edge.get(*nb).copied().flatten() == Some(i) { *nb } else { *na };
        let downstream = beyond.get(far).copied().unwrap_or(0);
        let tag = tag_of.get(i).copied().flatten();
        let (size, source) = match tag {
            Some(v) => (Some(v), "tag"),
            None if downstream > 0 => (res.pipe_schedule.iter().find(|(_, n)| downstream <= *n).map(|(d, _)| *d), "schedule"),
            None => (None, ""),
        };
        let centre = note_of.get(i).cloned().flatten().and_then(|t| parse_pipe_note(&t, crate::design::pipe_od(size.unwrap_or(1.0))));
        res.lines.push(FoundPiece {
            level: level.into(),
            from_ft: *a,
            to_ft: *b,
            size_in: size,
            size_source: if size.is_some() { source.into() } else { String::new() },
            heads_downstream: downstream,
            centre_ft: centre,
        });
    }
    // A height note on one piece holds for the straight line it is part of.
    for i in base..res.lines.len() {
        if res.lines.get(i).is_some_and(|p| p.centre_ft.is_some()) {
            continue;
        }
        let Some(p) = res.lines.get(i).cloned() else { continue };
        let c = res
            .lines
            .iter()
            .skip(base)
            .filter(|q| q.centre_ft.is_some() && collinear(p.from_ft, p.to_ft, q.from_ft, 0.05) && collinear(p.from_ft, p.to_ft, q.to_ft, 0.05))
            .find_map(|q| q.centre_ft);
        if let Some(slot) = res.lines.get_mut(i) {
            slot.centre_ft = c;
        }
    }
    // Each head: the piece it hangs from (ends at its node), and the arm when that piece is a
    // short spur of its own.
    let joins: Vec<(usize, f64, f64)> = heads
        .iter()
        .zip(&head_node)
        .map(|((_, p, _), n)| {
            let piece = n
                .and_then(|n| parent_edge.get(n).copied().flatten())
                .or_else(|| pieces.iter().enumerate().map(|(i, (a, b))| (i, project(*a, *b, *p).0)).min_by(|x, y| x.1.total_cmp(&y.1)).map(|x| x.0));
            let Some(pi) = piece else { return (usize::MAX, 0.0, f64::INFINITY) };
            let (a, b) = pieces.get(pi).copied().unwrap_or(([0.0; 2], [0.0; 2]));
            // An armover: a short piece sideways off a longer line, with the head at its end.
            let len = dist(a, b);
            let up = edges.get(pi).map(|(na, nb)| if parent_edge.get(*nb).copied().flatten() == Some(pi) { *na } else { *nb });
            // Short, square to the piece feeding it, and nothing beyond the head.
            let feeder = up.and_then(|u| parent_edge.get(u).copied().flatten()).and_then(|f| pieces.get(f));
            let square = feeder.is_some_and(|(c, d)| {
                let (u, v) = (unit(a, b), unit(*c, *d));
                (u[0] * v[0] + u[1] * v[1]).abs() < 0.05
            });
            let leaf = n.is_some_and(|n| edges.iter().filter(|(x, y)| *x == n || *y == n).count() == 1);
            let spur = len < 3.0 && square && leaf;
            (pi, 0.0, if spur { len } else { 0.0 })
        })
        .collect();
    // Heads with their room, height, line and arm.
    let level_rooms: Vec<&crate::ceiling_reader::FoundRoom> = ceilings.rooms.iter().filter(|r| r.level == level).collect();
    for ((_, p, mount), (li, _, arm)) in heads.iter().zip(&joins) {
        // The smallest room round the head (the ceiling reader can also report the building's
        // outline as a room), and its ceiling with a height.
        let room = level_rooms
            .iter()
            .filter(|r| p[0] >= r.min_ft[0] && p[0] <= r.max_ft[0] && p[1] >= r.min_ft[1] && p[1] <= r.max_ft[1])
            .min_by(|a, b| {
                ((a.max_ft[0] - a.min_ft[0]) * (a.max_ft[1] - a.min_ft[1])).total_cmp(&((b.max_ft[0] - b.min_ft[0]) * (b.max_ft[1] - b.min_ft[1])))
            })
            .map(|r| r.number.clone());
        let clg = room.as_ref().and_then(|n| ceilings.ceilings.iter().filter(|c| c.level == level && &c.room_number == n).find_map(|c| c.height_ft));
        let arm_len = *arm;
        let line_idx = (*li != usize::MAX).then_some(*li);
        // The centreline note of the piece the head hangs from, else the nearest noted piece.
        let piece_centre = line_idx.and_then(|i| res.lines.get(base + i)).and_then(|pc| pc.centre_ft).or_else(|| {
            res.lines
                .iter()
                .skip(base)
                .filter(|pc| pc.centre_ft.is_some())
                .min_by(|x, y| project(x.from_ft, x.to_ft, *p).0.total_cmp(&project(y.from_ft, y.to_ft, *p).0))
                .and_then(|pc| pc.centre_ft)
        });
        let (height, source) = match *mount {
            "upright" => (piece_centre.map(|c| c + res.sprig_in.unwrap_or(4.0) / 12.0), "line+sprig"),
            "sidewall" => (clg.map(|c| c - res.sidewall_below_in.unwrap_or(4.0) / 12.0), "ceiling-offset"),
            _ => (clg, "ceiling"),
        };
        let on_rcp = rcp.iter().any(|(l, q)| l == level && dist(*q, *p) < 0.5);
        res.heads.push(FoundHead {
            level: level.into(),
            at_ft: *p,
            mount: (*mount).into(),
            height_ft: height,
            height_source: if height.is_some() { source.into() } else { String::new() },
            room,
            line: line_idx.map(|i| i + base),
            arm_ft: arm_len,
            on_rcp,
        });
    }
}

/// Generate a seed's set and read it: sheets recognised, ceilings read, sprinklers read.
pub fn read_fixture_sprinklers(seed: u64) -> Result<(Truth, SprinklerResult), String> {
    let set = crate::generate(seed)?;
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = crate::sheet_reader::recognise_fixture_set(&dxf);
    let ceilings = crate::ceiling_reader::read_ceilings(&dxf, &map, None);
    let res = read_sprinklers(&dxf, &map, &ceilings);
    Ok((set.truth, res))
}

/// The heads as building spec terminals (`mep.terminals` for `bspec.set`): plan position in
/// drawing inches (building feet × 12), deflector height above the floor, kind
/// `sprinkler_<mount>`. `level_names` maps level codes to the spec's level names; heads whose
/// height wasn't found are left out.
pub fn spec_terminals(res: &SprinklerResult, system: &str, level_names: &[(&str, &str)]) -> Vec<serde_json::Value> {
    res.heads
        .iter()
        .filter_map(|h| {
            let level = level_names.iter().find(|(c, _)| *c == h.level)?.1;
            let z = h.height_ft?;
            Some(serde_json::json!({
                "system": system,
                "level": level,
                "at": [h.at_ft[0] * 12.0, h.at_ft[1] * 12.0],
                "size_in": [2.0, 0.0],
                "bottom_ft": z,
                "terminal_kind": format!("sprinkler_{}", h.mount),
            }))
        })
        .collect()
}

// ---- Scoring ------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SprinklerScore {
    pub heads_total: usize,
    pub heads_found: usize,
    /// Found heads with no true head within half a foot.
    pub heads_extra: usize,
    pub mount_correct: usize,
    /// Of the found heads: those with a height, and how many are within an inch.
    pub heights: usize,
    pub heights_within_1in: usize,
    pub height_mae_ft: Option<f64>,
    pub arms_total: usize,
    pub arms_found: usize,
    pub on_rcp: usize,
    /// Branch pieces (horizontal sprinkler pipe under 2"): matched, sized right, and of those
    /// sized from the schedule (no tag) how many were right.
    pub pieces_total: usize,
    pub pieces_matched: usize,
    pub pieces_size_correct: usize,
    pub schedule_sized: usize,
    pub schedule_correct: usize,
}

/// Score a reading against the truth.
pub fn score_sprinklers(t: &Truth, r: &SprinklerResult) -> SprinklerScore {
    let mut sc = SprinklerScore::default();
    let elev = |l: &str| t.level(l).map_or(0.0, |x| x.elevation_ft);
    let heads: Vec<&crate::truth::Terminal> =
        t.terminals.iter().filter(|x| x.kind == "sprinkler_head" && ["1", "2"].contains(&x.level.as_str())).collect();
    sc.heads_total = heads.len();
    let mut errs = Vec::new();
    for h in &heads {
        let p = [h.at_ft[0], h.at_ft[1]];
        let Some(f) = r
            .heads
            .iter()
            .filter(|f| f.level == h.level)
            .min_by(|a, b| dist(a.at_ft, p).total_cmp(&dist(b.at_ft, p)))
            .filter(|f| dist(f.at_ft, p) < 0.5)
        else {
            continue;
        };
        sc.heads_found += 1;
        if Some(f.mount.as_str()) == h.mount.as_deref() {
            sc.mount_correct += 1;
        }
        if f.on_rcp {
            sc.on_rcp += 1;
        }
        if let Some(z) = f.height_ft {
            let e = (z - (h.at_ft[2] - elev(&h.level))).abs();
            sc.heights += 1;
            if e <= 1.0 / 12.0 + 1e-6 {
                sc.heights_within_1in += 1;
            }
            errs.push(e);
        }
        // A true armover: the head's node joins a short horizontal arm.
        let arm = t.segments.iter().any(|s| {
            s.system == "FP"
                && !s.vertical
                && s.length_ft() < 3.0
                && s.size_in[0] < 2.0
                && h.node.as_ref().is_some_and(|n| {
                    t.node(n).is_some_and(|hn| {
                        hn.segments.iter().any(|d| {
                            t.segment(d).is_some_and(|d| d.vertical && (d.from == s.to || d.to == s.to || d.from == s.from || d.to == s.from))
                        })
                    })
                })
        });
        if arm {
            sc.arms_total += 1;
            if f.arm_ft > 0.5 {
                sc.arms_found += 1;
            }
        }
    }
    sc.heads_extra = r.heads.iter().filter(|f| !heads.iter().any(|h| h.level == f.level && dist([h.at_ft[0], h.at_ft[1]], f.at_ft) < 0.5)).count();
    sc.height_mae_ft = (!errs.is_empty()).then(|| errs.iter().sum::<f64>() / errs.len() as f64);
    // Branch pieces: the truth's horizontal sprinkler pipe under 2" in the plans' bands.
    for s in t.segments.iter().filter(|s| s.system == "FP" && !s.vertical && s.size_in[0] < 2.0 && ["1", "2"].contains(&s.level.as_str())) {
        sc.pieces_total += 1;
        let (a, b) = ([s.path_ft[0][0], s.path_ft[0][1]], [s.path_ft[1][0], s.path_ft[1][1]]);
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let found = r.lines.iter().filter(|p| p.level == s.level).find(|p| {
            project(p.from_ft, p.to_ft, mid).0 < 0.2 && dist(p.from_ft, p.to_ft) > 0.05 && {
                let u = unit(p.from_ft, p.to_ft);
                let v = unit(a, b);
                (u[0] * v[0] + u[1] * v[1]).abs() > 0.99
            }
        });
        let Some(p) = found else { continue };
        sc.pieces_matched += 1;
        let ok = p.size_in.is_some_and(|z| (z - s.size_in[0]).abs() < 1e-6);
        if ok {
            sc.pieces_size_correct += 1;
        }
        if p.size_source == "schedule" {
            sc.schedule_sized += 1;
            if ok {
                sc.schedule_correct += 1;
            }
        }
    }
    sc
}

impl SprinklerScore {
    pub fn summary(&self) -> String {
        format!(
            "heads {}/{} (+{} extra), mount {}, heights {} ({} within 1\", MAE {}), arms {}/{}, on RCP {}, pieces {}/{} matched, {} sized right ({} of {} from the schedule)",
            self.heads_found,
            self.heads_total,
            self.heads_extra,
            self.mount_correct,
            self.heights,
            self.heights_within_1in,
            self.height_mae_ft.map_or("-".into(), |m| format!("{}\"", fmt_in((m * 12.0 * 100.0).round() / 100.0))),
            self.arms_found,
            self.arms_total,
            self.on_rcp,
            self.pieces_matched,
            self.pieces_total,
            self.pieces_size_correct,
            self.schedule_correct,
            self.schedule_sized
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_heights_parse() {
        assert_eq!(parse_pipe_size("1-1/4\""), Some(1.25));
        assert_eq!(parse_pipe_size("1\""), Some(1.0));
        assert_eq!(parse_pipe_size("3/4\""), Some(0.75));
        assert_eq!(parse_pipe_size("CW"), None);
        assert_eq!(parse_feet_inches("9'-3\""), Some(9.25));
        assert_eq!(parse_feet_inches("10'-7 1/2\""), Some(10.0 + 7.5 / 12.0));
        assert!((parse_pipe_note("CL @ 9'-3\"", 1.315).unwrap_or(0.0) - 9.25).abs() < 1e-9);
        let bop = parse_pipe_note("BOP @ 9'-0\"", 1.2).unwrap_or(0.0);
        assert!((bop - (9.0 + 0.05)).abs() < 1e-9, "{bop}");
        assert!(parse_pipe_note("B.O.P. 9'-0\" AFF", 1.2).is_some());
        assert_eq!(inches_before("OPEN TO STRUCTURE, 4\" SPRIG", "\" SPRIG"), Some(4.0));
        assert_eq!(inches_before("4\" BELOW CEILING", "\" BELOW CEILING"), Some(4.0));
    }

    #[test]
    fn collinear_pieces_join() {
        let segs = [([0.0, 0.0], [5.0, 0.0]), ([5.0, 0.0], [9.0, 0.0]), ([9.0, 0.0], [9.0, 4.0])];
        let l = straight_lines(&segs);
        assert_eq!(l.len(), 2);
        assert!(l.iter().any(|(a, b)| (dist(*a, *b) - 9.0).abs() < 1e-9));
    }
}
