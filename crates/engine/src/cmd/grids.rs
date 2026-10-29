//! Structural grids: found on each level's plan, used to line the levels (and their sheets) up,
//! and to place the columns drawn at their intersections.
//!
//! - `grids.detect`: grid bubbles (circles 4"–24" in radius around a short label: `A`, `3`,
//!   `C.1`; or block references holding such a circle or a hexagon, labelled by an attribute or
//!   a text, as the parts library's `G_GRID_BUBBLE_050/075/HEX` are) and the grid line touching
//!   each (the longest line, or straight run of a polyline, with an end at the bubble), per
//!   level; bubbles at both ends of one line are one grid. And their intersections, labelled
//!   `A-1` (letters first). A level on its own sheet (`sheet`) is read from that drawing.
//! - `levels.align {reference?, apply?}`: each level's intersections matched by label with the
//!   reference level's give its scale and offset (least squares: `q ≈ s·p + t`). Applied, every
//!   level's base point becomes its point of the same grid intersection, so the levels stack
//!   exactly (a level on its own sheet gets its base in that sheet's coordinates); a level drawn
//!   at another scale is reported and left (levels share one drawing scale), and so is one whose
//!   fit can't be trusted (fewer than 2 intersections in common, or one missing by over 1").
//! - `grids.columns {level, apply?}`: the column outlines (closed outlines and circles up to 4'
//!   across, on structural or column layers) centred on the level's grid intersections, into
//!   the spec's `columns` with the outline they came from and status `rule`; `model.build`
//!   stands them up through their storeys.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, RawTag, Space};
use cadaddy_geom::{Polyline, Vec2};
use cadaddy_walk3d::{BuildingSpec, GridColumn, Role, level_at};
use serde_json::{Value, json};
use std::sync::Arc;

use super::bspec::{current, plan_objects};
use super::perceive::{texts, units_per_inch};
use super::*;
use crate::{Result, Session};

/// Bubbles, lines and columns looked at, at most (hostile drawings).
const MAX_ITEMS: usize = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("grids.detect", "Detect Grids", run_detect)
            .params("{level?: name} → {levels: [{level, sheet?: the level's own drawing, note?, grids: [{label, from, to, bubble}], intersections: [{label, at}]}]}")
            .noundo(),
        CommandSpec::new("levels.align", "Align Levels by Grids", run_align).params(
            "{reference?: level (default the first with grids), apply?: bool (set each level's base point to its point of a common grid intersection; levels on their own sheet are read from it and get their base in its coordinates; a level with fewer than 2 intersections in common or one missing by over 1\" is left, with a note)} → {reference, levels: [{level, sheet?, matched, scale, offset, residual, base?, note?}]}",
        ),
        CommandSpec::new("grids.columns", "Columns at Grids", run_columns).params(
            "{level: name (drawn in this drawing), to_level?: name, apply?: bool (into the spec's columns, replacing the level's)} → {columns: [{grid, at, size_in, round, source: the outline's handle, status: rule}]}",
        ),
    ]
}

/// A grid line and its label.
#[derive(Clone, Debug)]
struct Grid {
    label: String,
    a: Vec2,
    b: Vec2,
    bubble: Vec2,
}

/// A grid label: 1–4 letters, digits and dots (`A`, `12`, `C.1`, `AA`).
fn grid_label(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty()
        && t.len() <= 4
        && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
        && t.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

/// A grid bubble: its centre, radius and label.
#[derive(Clone)]
struct Bubble {
    at: Vec2,
    r: f64,
    label: String,
}

/// The grid bubbles of a set of objects: circles 4"–24" in radius around a grid label, and block
/// references whose block holds a circle or a closed polygon (a hexagon) of that size, labelled
/// by an attribute, a text in the block or a text over it.
fn bubbles(d: &Drawing, ents: &[&Arc<Entity>], k: f64) -> Vec<Bubble> {
    let notes = texts(ents);
    // A bubble is about 3/8"-5/8" on paper: up to 8' radius at 1/16" (a whole floor on a small sheet).
    let size = |r: f64| r >= 4.0 * k && r <= 96.0 * k;
    let over = |o: Vec2, r: f64| notes.iter().find(|(_, t, at, _)| at.dist(o) <= r && grid_label(t)).map(|(_, t, _, _)| t.trim().to_string());
    let mut out = Vec::new();
    for e in ents {
        if out.len() >= MAX_ITEMS {
            break;
        }
        match &e.kind {
            EntityKind::Circle(c) => {
                let (o, r) = (c.center.xy(), c.radius);
                if let Some(label) = over(o, r).filter(|_| size(r)) {
                    out.push(Bubble { at: o, r, label });
                }
            }
            // A circle a PDF drew as a path: a polyline whose points are all on one circle and go
            // most of the way round (an arc of a door swing does not).
            EntityKind::LwPolyline(pl) if pl.vertices.len() >= 12 => {
                let pts: Vec<Vec2> = pl.vertices.iter().map(|v| v.p).collect();
                let Some((o, r)) = round_path(&pts) else { continue };
                if let Some(label) = over(o, r).filter(|_| size(r)) {
                    out.push(Bubble { at: o, r, label });
                }
            }
            EntityKind::Insert(i) => {
                let Some(blk) = d.block(&i.block) else { continue };
                let m = i.transform(blk.base.xy());
                let sc = m.det().abs().sqrt();
                if !sc.is_finite() || sc <= 0.0 {
                    continue;
                }
                // Every round shape of the block: a symbol the PDF import made of a repeated group
                // holds several bubbles (two grids' bubbles and their leaders).
                let shapes: Vec<(Vec2, f64)> = blk
                    .entities
                    .iter()
                    .take(64)
                    .filter_map(|be| match &be.kind {
                        EntityKind::Circle(c) => Some((c.center.xy(), c.radius)),
                        EntityKind::LwPolyline(pl) if pl.closed && (5..=12).contains(&pl.vertices.len()) => {
                            let c = pl.vertices.iter().fold(Vec2::new(0.0, 0.0), |a, v| a + v.p) * (1.0 / pl.vertices.len() as f64);
                            Some((c, pl.vertices.iter().map(|v| v.p.dist(c)).fold(0.0, f64::max)))
                        }
                        // A circle drawn as a path, made a symbol block by the PDF import.
                        EntityKind::LwPolyline(pl) if pl.vertices.len() > 12 => round_path(&pl.vertices.iter().map(|v| v.p).collect::<Vec<_>>()),
                        _ => None,
                    })
                    .map(|(c, r)| (m.apply(c), r * sc))
                    .filter(|&(_, r)| size(r))
                    .collect();
                let one = shapes.len() == 1;
                for (o, r) in shapes {
                    // One shape: the block's own label (an attribute, a text in it) or a text over it;
                    // several: each by the text over it.
                    let own = || {
                        i.attribs.iter().map(|a| a.text.value.trim()).find(|t| grid_label(t)).map(str::to_string).or_else(|| {
                            blk.entities.iter().take(64).find_map(|be| match &be.kind {
                                EntityKind::Text(t) if grid_label(&t.value) => Some(t.value.trim().to_string()),
                                _ => None,
                            })
                        })
                    };
                    let label = if one { own().or_else(|| over(o, r)) } else { over(o, r) };
                    if let Some(label) = label {
                        out.push(Bubble { at: o, r, label });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The centre and radius of a path drawn as a circle: every point within 6% of one radius from
/// the centre and the points going at least 300° round it.
fn round_path(pts: &[Vec2]) -> Option<(Vec2, f64)> {
    let o = pts.iter().fold(Vec2::new(0.0, 0.0), |a, p| a + *p) * (1.0 / pts.len() as f64);
    let rs: Vec<f64> = pts.iter().map(|p| p.dist(o)).collect();
    let r = rs.iter().sum::<f64>() / rs.len() as f64;
    if !(r.is_finite() && r > 0.0) || rs.iter().any(|x| (x - r).abs() > 0.06 * r) {
        return None;
    }
    let mut angles: Vec<f64> = pts.iter().map(|p| (p.y - o.y).atan2(p.x - o.x)).collect();
    angles.sort_by(f64::total_cmp);
    let gap = angles.windows(2).map(|w| w[1] - w[0]).chain([angles[0] + std::f64::consts::TAU - angles[angles.len() - 1]]).fold(0.0, f64::max);
    (gap <= std::f64::consts::TAU / 6.0).then_some((o, r))
}

/// Lines, and the straight runs of open polylines (collinear segments joined), 10' long or more.
fn grid_lines(ents: &[&Arc<Entity>], k: f64) -> Vec<(Vec2, Vec2)> {
    let mut out: Vec<(Vec2, Vec2)> = Vec::new();
    let mut push = |a: Vec2, b: Vec2| {
        if a.dist(b) >= 120.0 * k && out.len() < MAX_ITEMS * 10 {
            out.push((a, b));
        }
    };
    for e in ents {
        match &e.kind {
            EntityKind::Line(l) => push(l.a.xy(), l.b.xy()),
            EntityKind::LwPolyline(pl) if !pl.closed => {
                let mut run: Option<(Vec2, Vec2)> = None;
                for w in pl.vertices.windows(2).take(MAX_ITEMS) {
                    let [v, u] = w else { continue };
                    if v.bulge.abs() > 1e-9 || v.p.dist(u.p) < 1e-9 {
                        if let Some((a, b)) = run.take() {
                            push(a, b);
                        }
                        continue;
                    }
                    run = match run {
                        Some((a, b)) if (b - a).cross(u.p - b).abs() <= 1e-6 * a.dist(b) * b.dist(u.p) && (b - a).dot(u.p - b) > 0.0 => {
                            Some((a, u.p))
                        }
                        Some((a, b)) => {
                            push(a, b);
                            Some((v.p, u.p))
                        }
                        None => Some((v.p, u.p)),
                    };
                }
                if let Some((a, b)) = run {
                    push(a, b);
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether the line `a`–`b` lies along grid `g`: parallel, and within 1" of its line.
fn along(g: &Grid, a: Vec2, b: Vec2, k: f64) -> bool {
    let r = g.b - g.a;
    let (lr, ls) = (r.len(), a.dist(b));
    if lr < 1e-12 || ls < 1e-12 {
        return false;
    }
    let off = |p: Vec2| (p - g.a).cross(r).abs() / lr;
    r.cross(b - a).abs() <= 1e-3 * lr * ls && off(a) <= k && off(b) <= k
}

/// The grids of a set of objects.
fn grids(d: &Drawing, ents: &[&Arc<Entity>], k: f64) -> Vec<Grid> {
    grids_with(d, ents, k, false, false)
}

/// [`grids`]; `placing`: for putting views onto a plan, each grid's direction from its row of
/// bubbles first and its line across the whole plan (not the line as drawn). `one_floor`: a
/// whole floor's plan, where a grid has a bubble at each end at most (a label on more is a tag:
/// window tags, section marks).
fn grids_with(d: &Drawing, ents: &[&Arc<Entity>], k: f64, placing: bool, one_floor: bool) -> Vec<Grid> {
    let lines = grid_lines(ents, k);
    let mut out: Vec<Grid> = Vec::new();
    let mut all = bubbles(d, ents, k);
    if one_floor {
        let mut n: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for b in &all {
            *n.entry(b.label.to_ascii_uppercase()).or_default() += 1;
        }
        all.retain(|b| n.get(&b.label.to_ascii_uppercase()).is_some_and(|c| *c <= 2));
    }
    // How far the bubbles spread: a grid drawn from its bubble alone spans that much each way.
    let spread = cadaddy_geom::Bounds2::from_points(all.iter().map(|x| x.at));
    let reach = if spread.is_empty() { 120.0 * k } else { (spread.max - spread.min).len().max(120.0 * k) };
    for Bubble { at: o, r, label } in all.iter().cloned() {
        // The longest line with an end on (or in) the bubble.
        let touching =
            lines.iter().filter(|(a, b)| a.dist(o) <= 1.3 * r || b.dist(o) <= 1.3 * r).max_by(|x, y| x.0.dist(x.1).total_cmp(&y.0.dist(y.1)));
        // A dashed grid line is pieces too short to be a line: its direction is across its row of
        // bubbles (a row across the top marks vertical grids, a column down the side horizontal).
        // The nearest bubble of the same family (numbers, or letters) lies along the row: a row
        // across marks vertical grids, a column horizontal ones. Two views side by side repeat
        // the labels far off; the nearest is the view's own.
        let from_row = || {
            let family = |l: &str| l.chars().next().is_some_and(|c| c.is_ascii_digit());
            let next = all
                .iter()
                .filter(|x| x.at.dist(o) > r && family(&x.label) == family(&label) && !x.label.eq_ignore_ascii_case(&label))
                .min_by(|a, b| a.at.dist(o).total_cmp(&b.at.dist(o)))?;
            let d = next.at - o;
            if d.x.abs() >= d.y.abs() {
                Some((Vec2::new(o.x, o.y + reach), Vec2::new(o.x, o.y - reach)))
            } else {
                Some((Vec2::new(o.x + reach, o.y), Vec2::new(o.x - reach, o.y)))
            }
        };
        // The row of bubbles first (a wall or leader passing a bubble is no grid), its own line
        // for a bubble alone.
        let line = if placing { from_row().or(touching.copied()) } else { touching.copied().or_else(from_row) };
        let Some((a, b)) = line else { continue };
        // From the bubble end to the far end.
        let (a, b) = if a.dist(o) <= b.dist(o) { (a, b) } else { (b, a) };
        // Placing, a grid runs across the whole plan: a dashed one's first piece is only its direction.
        let len = a.dist(b);
        let (a, b) = if placing && len > 1e-9 && len < reach { (a, a + (b - a) * (reach / len)) } else { (a, b) };
        // Bubbles at both ends of a grid line (or on pieces of one line) are one grid: its
        // line spans them all.
        if let Some(g) = out.iter_mut().find(|g| g.label.eq_ignore_ascii_case(&label) && along(g, a, b, k)) {
            let (a0, len) = (g.a, g.a.dist(g.b));
            let u = (g.b - g.a) * (1.0 / len);
            let (lo, hi) = [(a - a0).dot(u), (b - a0).dot(u)].iter().fold((0.0_f64, len), |(lo, hi), t| (lo.min(*t), hi.max(*t)));
            g.a = a0 + u * lo;
            g.b = a0 + u * hi;
            continue;
        }
        out.push(Grid { label, a, b, bubble: o });
        if out.len() >= MAX_ITEMS {
            break;
        }
    }
    out
}

/// Where two grid lines cross (within a few feet of both), and its label (letters first).
fn intersections(gs: &[Grid], k: f64) -> Vec<(String, Vec2)> {
    let mut out: Vec<(String, Vec2)> = Vec::new();
    let slack = 36.0 * k;
    for (i, g) in gs.iter().enumerate() {
        for h in gs.iter().skip(i + 1) {
            let (r, s) = (g.b - g.a, h.b - h.a);
            let den = r.cross(s);
            if den.abs() < 0.1 * r.len() * s.len() {
                continue;
            }
            let t = (h.a - g.a).cross(s) / den;
            let u = (h.a - g.a).cross(r) / den;
            let (lr, ls) = (r.len().max(1e-12), s.len().max(1e-12));
            if t * lr < -slack || t * lr > lr + slack || u * ls < -slack || u * ls > ls + slack {
                continue;
            }
            let p = g.a + r * t;
            let letters = |l: &str| l.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
            let label = if letters(&g.label) || !letters(&h.label) { format!("{}-{}", g.label, h.label) } else { format!("{}-{}", h.label, g.label) };
            out.push((label, p));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    // One crossing per label and place (bubbles at both ends of a grid make it twice).
    let mut one: Vec<(String, Vec2)> = Vec::new();
    for (l, p) in out {
        if !one.iter().any(|(m, q)| *m == l && q.dist(p) <= 24.0 * k) {
            one.push((l, p));
        }
    }
    one
}

/// A level's grids and their intersections, from this drawing or the level's own sheet.
struct LevelGrids {
    level: String,
    /// The level's own drawing (`sheet`), when it is on one.
    sheet: Option<String>,
    /// Its drawing's units per inch.
    k: f64,
    grids: Vec<Grid>,
    xs: Vec<(String, Vec2)>,
    /// Why its sheet gave nothing.
    note: Option<String>,
}

/// Most bytes read of a level's own sheet.
#[cfg(not(target_arch = "wasm32"))]
const MAX_SHEET_BYTES: u64 = 256 << 20;

/// The drawing of a level on its own sheet: its stored path, beside the drawing at `host`.
fn read_sheet(path: &str, host: Option<&str>) -> std::result::Result<Drawing, String> {
    let hooks = super::file::io().ok_or("file formats are not available in this build")?;
    let file = crate::doc_path::resolve(path, host.map(std::path::Path::new)).map_err(|r| format!("not opened, {r}"))?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Read;
        let f = crate::readfile::open(&file).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        f.take(MAX_SHEET_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_SHEET_BYTES {
            return Err("too large".into());
        }
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        (hooks.read)(&bytes, &name)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (hooks, file);
        Err("other drawings are not read on the web".into())
    }
}

/// Each level's grids and intersections: levels drawn here from their part of this drawing,
/// levels on their own sheet from that drawing (within their `region`, when set).
fn by_level(s: &Session, sp: &BuildingSpec) -> Result<Vec<LevelGrids>> {
    let d = s.doc()?;
    let host = s.state().ok().and_then(|st| st.path.clone());
    let k = units_per_inch(d);
    let all = plan_objects(d);
    let centre = |d: &Drawing, e: &Entity| {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        (!b.is_empty()).then(|| b.center())
    };
    let mut out = Vec::new();
    for l in &sp.levels {
        let mut row = LevelGrids { level: l.name.clone(), sheet: l.sheet.clone(), k, grids: Vec::new(), xs: Vec::new(), note: None };
        match &l.sheet {
            None => {
                let mine: Vec<&Arc<Entity>> = all
                    .iter()
                    .copied()
                    .filter(|e| centre(d, e).and_then(|c| level_at(&sp.levels, c.x, c.y)).is_some_and(|x| x.name == l.name))
                    .collect();
                row.grids = grids(d, &mine, k);
            }
            Some(f) => match read_sheet(f, host.as_deref()) {
                Ok(sd) => {
                    row.k = units_per_inch(&sd);
                    let inside = |c: Vec2| {
                        l.region.is_none_or(|[x0, y0, x1, y1]| c.x >= x0.min(x1) && c.x <= x0.max(x1) && c.y >= y0.min(y1) && c.y <= y0.max(y1))
                    };
                    let mine: Vec<&Arc<Entity>> = plan_objects(&sd).into_iter().filter(|e| centre(&sd, e).is_some_and(inside)).collect();
                    row.grids = grids(&sd, &mine, row.k);
                }
                Err(e) => row.note = Some(format!("its sheet {f} could not be read ({e}): check the level's sheet path, or set its base by hand")),
            },
        }
        row.xs = intersections(&row.grids, row.k);
        out.push(row);
    }
    Ok(out)
}

fn pt(p: Vec2) -> Value {
    json!([(p.x * 1e6).round() / 1e6, (p.y * 1e6).round() / 1e6])
}

fn run_detect(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "grids.detect")?;
    let only = str_param(p, "level");
    if let Some(l) = only.filter(|l| sp.level(l).is_none()) {
        return Err(bad("grids.detect", format!("there is no level named {l}")));
    }
    let levels: Vec<Value> = by_level(s, &sp)?
        .into_iter()
        .filter(|l| only.is_none_or(|o| o.eq_ignore_ascii_case(&l.level)))
        .map(|l| {
            let mut row = json!({
                "level": l.level,
                "grids": l.grids.iter().map(|g| json!({"label": g.label, "from": pt(g.a), "to": pt(g.b), "bubble": pt(g.bubble)})).collect::<Vec<_>>(),
                "intersections": l.xs.iter().map(|(n, p)| json!({"label": n, "at": pt(*p)})).collect::<Vec<_>>(),
            });
            if let Some(f) = l.sheet {
                row["sheet"] = json!(f);
            }
            if let Some(n) = l.note {
                row["note"] = json!(n);
            }
            row
        })
        .collect();
    Ok(json!({ "levels": levels }))
}

/// The least-squares scale and offset taking `p` onto `q` (`q ≈ s·p + t`), and the largest miss.
fn fit(pairs: &[(Vec2, Vec2)]) -> Option<(f64, Vec2, f64)> {
    let n = pairs.len() as f64;
    if pairs.is_empty() {
        return None;
    }
    let pm = pairs.iter().fold(Vec2::new(0.0, 0.0), |a, (p, _)| a + *p) * (1.0 / n);
    let qm = pairs.iter().fold(Vec2::new(0.0, 0.0), |a, (_, q)| a + *q) * (1.0 / n);
    let num: f64 = pairs.iter().map(|(p, q)| (*p - pm).dot(*q - qm)).sum();
    let den: f64 = pairs.iter().map(|(p, _)| (*p - pm).dot(*p - pm)).sum();
    let s = if den > 1e-12 { num / den } else { 1.0 };
    let t = qm - pm * s;
    let miss = pairs.iter().map(|(p, q)| (*p * s + t).dist(*q)).fold(0.0, f64::max);
    (s.is_finite() && t.is_finite()).then_some((s, t, miss))
}

fn run_align(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "levels.align")?;
    let found = by_level(s, &sp)?;
    let reference = match str_param(p, "reference") {
        Some(r) => sp.level(r).map(|l| l.name.clone()).ok_or_else(|| bad("levels.align", format!("there is no level named {r}")))?,
        None => {
            found.iter().find(|l| !l.xs.is_empty()).map(|l| l.level.clone()).ok_or_else(|| bad("levels.align", "no level has grid intersections"))?
        }
    };
    let Some(LevelGrids { xs: ref_xs, k: ref_k, .. }) = found.iter().find(|l| l.level == reference) else {
        return Err(bad("levels.align", "the reference level has no grids"));
    };
    if ref_xs.is_empty() {
        return Err(bad("levels.align", format!("{reference} has no grid intersections")));
    }
    let apply = bool_or(p, "apply", false);
    // The reference's base: its own, else its first intersection.
    let ref_level = sp.level(&reference).cloned();
    let ref_base =
        ref_level.as_ref().and_then(|l| l.base).map(|b| Vec2::new(b[0], b[1])).or_else(|| ref_xs.first().map(|x| x.1)).unwrap_or(Vec2::new(0.0, 0.0));
    let mut rows = Vec::new();
    let mut bases: Vec<(String, [f64; 2])> = Vec::new();
    for LevelGrids { level: l, sheet, xs, note, .. } in &found {
        let pairs: Vec<(Vec2, Vec2)> = xs.iter().filter_map(|(n, pp)| ref_xs.iter().find(|(m, _)| m == n).map(|(_, q)| (*pp, *q))).collect();
        let mut row = json!({ "level": l, "matched": pairs.len() });
        if let Some(f) = sheet {
            row["sheet"] = json!(f);
        }
        let Some((scale, t, miss)) = fit(&pairs).filter(|_| !pairs.is_empty()) else {
            row["note"] = json!(note.clone().unwrap_or_else(|| "no grid intersections in common with the reference".into()));
            rows.push(row);
            continue;
        };
        row["scale"] = json!((scale * 1e9).round() / 1e9);
        row["offset"] = pt(t);
        row["residual"] = json!((miss * 1e6).round() / 1e6);
        if pairs.len() < 2 {
            row["note"] = json!(format!(
                "only grid intersection {} is in common with {reference}: not aligned (two or more are needed to check the fit); check its grid labels with grids.detect, or set its base by hand",
                xs.iter().find(|(n, _)| ref_xs.iter().any(|(m, _)| m == n)).map_or("", |(n, _)| n.as_str())
            ));
        } else if miss > *ref_k {
            row["note"] = json!(format!(
                "its grids don't line up with {reference}'s: an intersection misses by {:.2}\" after the fit (over 1\"), so a grid label may not match; not aligned: check its grid labels with grids.detect, or set its base by hand",
                miss / ref_k
            ));
        } else if (scale - 1.0).abs() > 1e-3 {
            row["note"] = json!(format!("drawn at {:.4}× the reference's scale: not aligned (levels share one drawing scale)", scale));
        } else {
            // The point of this level that lands on the reference's base: base = (B_ref − t) / s.
            let base = (ref_base - t) * (1.0 / scale);
            let b = [(base.x * 1e6).round() / 1e6, (base.y * 1e6).round() / 1e6];
            row["base"] = json!(b);
            bases.push((l.clone(), b));
        }
        rows.push(row);
    }
    if apply {
        for (l, b) in &bases {
            if let Some(x) = sp.levels.iter_mut().find(|x| &x.name == l) {
                x.base = Some(*b);
            }
        }
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({ "reference": reference, "levels": rows, "applied": apply }))
}

fn run_columns(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "grids.columns")?;
    let level = str_param(p, "level").ok_or_else(|| bad("grids.columns", "`level` is required"))?;
    let level = sp.level(level).map(|l| l.name.clone()).ok_or_else(|| bad("grids.columns", format!("there is no level named {level}")))?;
    let to_level = match str_param(p, "to_level") {
        Some(t) => Some(sp.level(t).map(|l| l.name.clone()).ok_or_else(|| bad("grids.columns", format!("there is no level named {t}")))?),
        None => None,
    };
    if let Some(f) = sp.level(&level).and_then(|l| l.sheet.as_ref()) {
        return Err(bad("grids.columns", format!("{level} is drawn on its own sheet ({f}): open that drawing and run grids.columns there")));
    }
    let found = by_level(s, &sp)?;
    let d = s.doc()?;
    let k = units_per_inch(d);
    let Some(LevelGrids { xs, .. }) = found.iter().find(|l| l.level == level) else { return Err(bad("grids.columns", "no grids on that level")) };
    let all = plan_objects(d);
    let structural = |e: &Entity| {
        let up = e.common.layer.to_ascii_uppercase();
        up.starts_with("S-") || up.contains("COL") || sp.role(&e.common.layer).is_some_and(|r| r.role == Role::Column)
    };
    let mut columns: Vec<GridColumn> = Vec::new();
    for (label, at) in xs.iter().take(MAX_ITEMS) {
        let shape = all.iter().filter(|e| structural(e)).find_map(|e| {
            let (outline, round): (Vec<Vec2>, bool) = match &e.kind {
                EntityKind::Circle(c) => (vec![c.center.xy() - Vec2::new(c.radius, c.radius), c.center.xy() + Vec2::new(c.radius, c.radius)], true),
                EntityKind::LwPolyline(pl) if pl.closed && pl.vertices.len() >= 3 => {
                    (Polyline { vertices: pl.vertices.clone(), closed: true }.tessellate(1e-3), false)
                }
                _ => return None,
            };
            let b = cadaddy_geom::Bounds2::from_points(outline.iter().copied());
            (!b.is_empty() && b.width() <= 48.0 * k && b.height() <= 48.0 * k && b.center().dist(*at) <= 6.0 * k).then_some((b, round, e.handle))
        });
        if let Some((b, round, source)) = shape {
            let r8 = |v: f64| (v / k * 8.0).round() / 8.0;
            columns.push(GridColumn {
                level: level.clone(),
                grid: label.clone(),
                at: [at.x, at.y],
                size_in: [r8(b.width()), r8(b.height())],
                round,
                to_level: to_level.clone(),
                source: source.hex(),
                status: "rule".into(),
            });
        }
    }
    let apply = bool_or(p, "apply", false);
    let out = json!({ "columns": serde_json::to_value(&columns).unwrap_or(Value::Null), "applied": apply });
    if apply {
        sp.columns.retain(|c| c.level != level);
        sp.columns.extend(columns);
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(out)
}

/// The spec's grid columns into model space: prisms from their level's floor to the top of their
/// storey (or of `to_level`'s). The count.
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec) -> usize {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let k = upf / 12.0;
    if !sp.columns.is_empty() && d.layer("3D-Columns").is_none() {
        d.layers.push(cadaddy_doc::Layer { name: "3D-Columns".into(), color: cadaddy_doc::color::Color::Index(8), ..cadaddy_doc::Layer::default() });
    }
    let mut n = 0;
    for c in sp.columns.iter().take(MAX_ITEMS * 10) {
        let Some(level) = sp.level(&c.level) else { continue };
        let top_level = c.to_level.as_ref().and_then(|t| sp.level(t)).unwrap_or(level);
        let (z0, z1) = (level.elevation_ft * upf, (top_level.elevation_ft + top_level.height_ft) * upf);
        let at = Vec2::new(c.at[0], c.at[1]);
        let (w, h) = (c.size_in[0] * k / 2.0, c.size_in[1] * k / 2.0);
        let outline: Vec<Vec2> = if c.round {
            (0..24).map(|i| at + Vec2::from_angle(f64::from(i) * std::f64::consts::TAU / 24.0) * w).collect()
        } else {
            vec![at + Vec2::new(-w, -h), at + Vec2::new(w, -h), at + Vec2::new(w, h), at + Vec2::new(-w, h)]
        };
        let Some(m) = super::bspec::prism(&outline, z0, z1) else { continue };
        let t = |code: i32, v: String| RawTag { code, value: v };
        // The outline it was read from (none for a column typed into the spec).
        let source = Handle::parse_hex(&c.source).filter(|h| h.0 != 0).map(|h| h.hex()).unwrap_or_default();
        let mut xdata = vec![
            t(1001, super::wallgen::APP.into()),
            t(1000, "column".into()),
            t(1000, level.name.clone()),
            t(1000, source),
            t(1000, format!("grid:{}", c.grid)),
        ];
        if !c.status.is_empty() {
            xdata.push(t(1000, format!("status:{}", c.status)));
        }
        if d.add(&Space::Model, Common { layer: "3D-Columns".into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_fits() {
        assert!(grid_label("A") && grid_label("12") && grid_label("C.1") && !grid_label("ROOM 1") && !grid_label(""));
        let pairs =
            [(Vec2::new(0.0, 0.0), Vec2::new(10.0, 5.0)), (Vec2::new(4.0, 0.0), Vec2::new(14.0, 5.0)), (Vec2::new(0.0, 2.0), Vec2::new(10.0, 7.0))];
        let (s, t, miss) = fit(&pairs).unwrap_or((0.0, Vec2::new(0.0, 0.0), 1.0));
        assert!((s - 1.0).abs() < 1e-12 && t.dist(Vec2::new(10.0, 5.0)) < 1e-12 && miss < 1e-12);
        let half: Vec<(Vec2, Vec2)> = pairs.iter().map(|(p, q)| (*p * 0.5, *q)).collect();
        assert!((fit(&half).map_or(0.0, |f| f.0) - 2.0).abs() < 1e-12);
    }

    /// Grids `xs` (vertical, bubbled below) and `ys` (horizontal, bubbled left), 300 apart,
    /// drawn at `o` scaled `k`.
    fn grid_view(s: &mut crate::Session, o: [f64; 2], k: f64, xs: &[(f64, &str)], ys: &[(f64, &str)]) {
        let p = |x: f64, y: f64| [o[0] + x * k, o[1] + y * k];
        for &(x, label) in xs {
            s.execute("line", &json!({"points": [p(x, -78.0), p(x, 700.0)]})).unwrap();
            s.execute("circle", &json!({"center": p(x, -96.0), "radius": 18.0 * k})).unwrap();
            s.execute("text", &json!({"at": p(x - 4.0, -100.0), "text": label, "height": 8.0 * k})).unwrap();
        }
        for &(y, label) in ys {
            s.execute("line", &json!({"points": [p(-78.0, y), p(1000.0, y)]})).unwrap();
            s.execute("circle", &json!({"center": p(-96.0, y), "radius": 18.0 * k})).unwrap();
            s.execute("text", &json!({"at": p(-100.0, y - 4.0), "text": label, "height": 8.0 * k})).unwrap();
        }
    }

    /// [`grid_view`] as a PDF draws it: each bubble a 24-point path, each grid line dashes.
    fn pdf_grid_view(s: &mut crate::Session, o: [f64; 2], k: f64, xs: &[(f64, &str)], ys: &[(f64, &str)]) {
        let p = |x: f64, y: f64| [o[0] + x * k, o[1] + y * k];
        let ring = |c: [f64; 2], r: f64| -> Vec<[f64; 2]> {
            (0..24).map(|i| f64::from(i) * std::f64::consts::TAU / 24.0).map(|a| [c[0] + r * a.cos(), c[1] + r * a.sin()]).collect()
        };
        let dashes = |s: &mut crate::Session, a: [f64; 2], b: [f64; 2]| {
            let n = 30;
            for i in (0..n).step_by(2) {
                let t = |j: i32| [a[0] + (b[0] - a[0]) * f64::from(j) / f64::from(n), a[1] + (b[1] - a[1]) * f64::from(j) / f64::from(n)];
                s.execute("line", &json!({"points": [t(i), t(i + 1)]})).unwrap();
            }
        };
        for &(x, label) in xs {
            dashes(s, p(x, -70.0), p(x, 700.0));
            s.execute("pline", &json!({"points": ring(p(x, -96.0), 26.0 * k)})).unwrap();
            s.execute("text", &json!({"at": p(x - 4.0, -100.0), "text": label, "height": 18.0 * k})).unwrap();
        }
        for &(y, label) in ys {
            dashes(s, p(-70.0, y), p(1000.0, y));
            s.execute("pline", &json!({"points": ring(p(-96.0, y), 26.0 * k)})).unwrap();
            s.execute("text", &json!({"at": p(-100.0, y - 4.0), "text": label, "height": 18.0 * k})).unwrap();
        }
    }

    #[test]
    fn path_bubbles_and_dashed_grids_still_place_a_view() {
        let mut s = crate::Session::new();
        let (gx, gy) = ([(0.0, "A"), (300.0, "B"), (600.0, "C")], [(0.0, "1"), (300.0, "2")]);
        pdf_grid_view(&mut s, [0.0, 0.0], 1.0, &gx, &gy);
        pdf_grid_view(&mut s, [5000.0, 0.0], 0.5, &gx, &gy);
        let plan = [("1false".to_string(), [-200.0, -200.0, 1100.0, 800.0])];
        let g = GridPlacer::new(s.doc().unwrap(), 12.0, &plan, &[[4700.0, -300.0, 6000.0, 1000.0]]);
        let at = [5000.0 + 450.0 * 0.5, 150.0 * 0.5];
        let (k, t) = g.place(at, "1false").expect("placed by path bubbles and dashed grids");
        let q = [at[0] * k + t[0], at[1] * k + t[1]];
        assert!((k - 2.0).abs() < 1e-6 && (q[0] - 450.0).abs() < 1e-3 && (q[1] - 150.0).abs() < 1e-3, "scale {k}, landed at {q:?}");
        // A door swing (a quarter arc of points) is no bubble.
        let arc: Vec<Vec2> = (0..12).map(|i| f64::from(i) * std::f64::consts::FRAC_PI_2 / 11.0).map(|a| Vec2::new(a.cos(), a.sin()) * 30.0).collect();
        assert!(round_path(&arc).is_none());
    }

    /// Points every 12 along the walls of a floor `n` bays long: a corridor and the bay walls,
    /// with an odd end (a stair) so one bay over is no fit.
    fn floor_lines(n: i32, o: Vec2, s: f64) -> Vec<Vec2> {
        let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
        let w = f64::from(n) * 180.0;
        for y in [0.0, 300.0, 360.0, 660.0] {
            segs.push((Vec2::new(0.0, y), Vec2::new(w, y)));
        }
        for i in 0..=n {
            let x = f64::from(i) * 180.0;
            segs.push((Vec2::new(x, 0.0), Vec2::new(x, 300.0)));
            segs.push((Vec2::new(x, 360.0), Vec2::new(x, 660.0)));
        }
        segs.push((Vec2::new(w, 0.0), Vec2::new(w + 240.0, -120.0)));
        segs.push((Vec2::new(w + 240.0, -120.0), Vec2::new(w + 240.0, 660.0)));
        let mut out = Vec::new();
        for (a, b) in segs {
            let m = ((a.dist(b) / 12.0).ceil() as usize).max(1);
            out.extend((0..=m).map(|i| o + (a + (b - a) * (i as f64 / m as f64)) * s));
        }
        out
    }

    #[test]
    fn a_repeating_plan_fits_the_same_way_every_time() {
        // Bays repeating every 30' tie many offsets: the pick must not follow a hash map's order.
        let k = 1.0;
        let mut tgt = Vec::new();
        for bay in 0..8 {
            let x0 = f64::from(bay) * 360.0;
            for i in 0..=60 {
                tgt.push(Vec2::new(x0, f64::from(i) * 12.0));
                tgt.push(Vec2::new(x0 + f64::from(i % 31) * 12.0, 0.0));
            }
        }
        let src: Vec<Vec2> = tgt.iter().filter(|p| p.x < 1100.0).map(|p| *p + Vec2::new(5.0, 3.0)).collect();
        let first = linework_fit(&src, &tgt, k).map(|f| (f.0.to_bits(), f.1.x.to_bits(), f.1.y.to_bits()));
        for _ in 0..12 {
            assert_eq!(linework_fit(&src, &tgt, k).map(|f| (f.0.to_bits(), f.1.x.to_bits(), f.1.y.to_bits())), first);
        }
    }

    #[test]
    fn a_background_view_lands_on_its_floor_plan() {
        // The plan at full size with clutter (furniture: a dot field), the view at half size elsewhere.
        let mut plan = floor_lines(12, Vec2::new(1000.0, 2000.0), 1.0);
        plan.extend((0..120).map(|i| Vec2::new(1030.0 + f64::from(i % 20) * 107.0, 2040.0 + f64::from(i / 20) * 97.0)));
        let view = floor_lines(12, Vec2::new(50000.0, 300.0), 0.5);
        let (s, t, score, margin) = linework_fit(&view, &plan, 1.0).expect("a fit");
        let corner = Vec2::new(50000.0, 300.0) * s + t;
        assert!((s - 2.0).abs() < 0.02 && corner.dist(Vec2::new(1000.0, 2000.0)) < 18.0, "scale {s}, corner at {corner:?}");
        assert!(score >= 0.9 && margin >= 0.04, "score {score}, margin {margin}");
        // A view of another building (a square block) is refused.
        let other: Vec<Vec2> = (0..400).flat_map(|i| [Vec2::new(f64::from(i) * 3.0, 0.0), Vec2::new(0.0, f64::from(i) * 3.0)]).collect();
        assert!(linework_fit(&other, &plan, 1.0).is_none_or(|f| f.2 < 0.9 || f.3 < 0.04));
    }

    #[test]
    fn an_enlarged_view_goes_onto_the_plan_by_its_own_grids() {
        let mut s = crate::Session::new();
        // The floor plan: grids A B C (x 0, 300, 600) and 1 2 (y 0, 300).
        let (gx, gy) = ([(0.0, "A"), (300.0, "B"), (600.0, "C")], [(0.0, "1"), (300.0, "2")]);
        grid_view(&mut s, [0.0, 0.0], 1.0, &gx, &gy);
        // An enlarged sheet with two views at 1.2x: A-B, then B-C (B, 1 and 2 shown twice).
        grid_view(&mut s, [5000.0, 0.0], 1.2, &gx[..2], &gy);
        grid_view(&mut s, [7000.0, 0.0], 1.2, &[(300.0, "B"), (600.0, "C")], &gy);
        let plan = [("1false".to_string(), [-200.0, -200.0, 1100.0, 800.0])];
        let g = GridPlacer::new(s.doc().unwrap(), 12.0, &plan, &[[4700.0, -300.0, 9000.0, 1000.0]]);
        // A decoy near the second view: a tag in a circle labelled like a grid on the plan.
        s.execute("circle", &json!({"center": [7000.0 + 200.0, 500.0], "radius": 20.0})).unwrap();
        s.execute("text", &json!({"at": [7000.0 + 196.0, 496.0], "text": "A", "height": 9.6})).unwrap();
        // A point between B and C in the second view: (450, 150) on the plan.
        let at = [7000.0 + 450.0 * 1.2, 150.0 * 1.2];
        let (k, t) = g.place(at, "1false").expect("placed by its view's grids");
        let q = [at[0] * k + t[0], at[1] * k + t[1]];
        assert!((k - 1.0 / 1.2).abs() < 1e-6, "scale {k}");
        assert!((q[0] - 450.0).abs() < 1e-3 && (q[1] - 150.0).abs() < 1e-3, "landed at {q:?}");
        // Nothing for a level without a plan, or a point on no sheet.
        assert!(g.place(at, "2false").is_none() && g.place([20000.0, 0.0], "1false").is_none());
    }
}

/// Grid intersections per level (on its architectural plan) and per sheet, to put a point of an
/// enlarged or partial plan onto its level's plan by the grid lines its view shows (a view of an
/// enlarged sheet shares no room number with the floor plan, but shows the same grid bubbles).
pub(crate) struct GridPlacer {
    k: f64,
    upf: f64,
    levels: Vec<(String, Vec<(String, Vec2)>)>,
    sheets: Vec<([f64; 4], Vec<(String, Vec2)>)>,
    /// Per level then per sheet: objects, bubbles and grids found (for the progress lines).
    counts: Vec<(usize, usize, usize)>,
}

/// Fewest shared grid intersections for a fit.
const MIN_GRID_PAIRS: usize = 3;

impl GridPlacer {
    /// `levels`: each level's key and its architectural plan's region; `sheets`: the rectangles
    /// of the sheets whose points may need placing.
    pub(crate) fn new(d: &Drawing, upf: f64, levels: &[(String, [f64; 4])], sheets: &[[f64; 4]]) -> Self {
        let k = units_per_inch(d);
        let objs: Vec<(Vec2, &Arc<Entity>)> = plan_objects(d)
            .into_iter()
            .filter_map(|e| {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                (!b.is_empty()).then(|| (b.center(), e))
            })
            .collect();
        let inside = |r: &[f64; 4], c: Vec2| c.x >= r[0].min(r[2]) && c.x <= r[0].max(r[2]) && c.y >= r[1].min(r[3]) && c.y <= r[1].max(r[3]);
        let mut counts = Vec::new();
        let mut xs_in = |r: &[f64; 4], one_floor: bool| {
            let mine: Vec<&Arc<Entity>> = objs.iter().filter(|(c, _)| inside(r, *c)).map(|(_, e)| *e).collect();
            let gs = grids_with(d, &mine, k, true, one_floor);
            counts.push((mine.len(), bubbles(d, &mine, k).len(), gs.len()));
            intersections(&gs, k)
        };
        let levels: Vec<(String, Vec<(String, Vec2)>)> = levels.iter().map(|(key, r)| (key.clone(), xs_in(r, true))).collect();
        let sheets: Vec<([f64; 4], Vec<(String, Vec2)>)> = sheets.iter().map(|r| (*r, xs_in(r, false))).collect();
        GridPlacer { k, upf, levels, sheets, counts }
    }

    /// How many grid intersections each level's plan and each sheet gave (for the progress lines).
    pub(crate) fn summary(&self) -> String {
        let lv: Vec<String> = self.levels.iter().map(|(k, xs)| format!("{k}:{}", xs.len())).collect();
        let sh: Vec<usize> = self.sheets.iter().map(|(_, xs)| xs.len()).collect();
        let n = self.levels.len();
        format!("plans {lv:?} (objects, bubbles, grids {:?}); sheets {sh:?}", &self.counts[..n.min(self.counts.len())])
    }

    /// The scale and offset that put `p` (on one of the sheets) onto the plan of the level keyed
    /// `level`, from the grid intersections of `p`'s own view (the nearest of each label).
    pub(crate) fn place(&self, p: [f64; 2], level: &str) -> Option<(f64, [f64; 2])> {
        let at = Vec2::new(p[0], p[1]);
        let inside = |r: &[f64; 4]| p[0] >= r[0].min(r[2]) && p[0] <= r[0].max(r[2]) && p[1] >= r[1].min(r[3]) && p[1] <= r[1].max(r[3]);
        let (_, xs) = self.sheets.iter().find(|(r, _)| inside(r))?;
        let (_, plan) = self.levels.iter().find(|(key, _)| key == level)?;
        // The nearest instance of each label: the view `p` is in repeats its labels elsewhere.
        let mut near: Vec<(&str, Vec2, f64)> = Vec::new();
        for (label, q) in xs {
            let d = q.dist(at);
            match near.iter_mut().find(|(l, _, _)| *l == label.as_str()) {
                Some(n) if d < n.2 => *n = (label.as_str(), *q, d),
                Some(_) => {}
                None => near.push((label.as_str(), *q, d)),
            }
        }
        near.sort_by(|a, b| a.2.total_cmp(&b.2));
        let on_plan = |l: &str| {
            let hits: Vec<Vec2> = plan.iter().filter(|(x, _)| x == l).map(|(_, q)| *q).collect();
            (hits.len() == 1).then(|| hits[0])
        };
        // Consensus: each two of the nearest matches give a scale and offset; the one most of
        // the others agree with wins (a neighbouring view's label, or a tag, agrees with none).
        let cands: Vec<(Vec2, Vec2)> = near.iter().take(12).filter_map(|(l, q, _)| Some((*q, on_plan(l)?))).collect();
        let tol = 1.5 * self.upf.max(12.0 * self.k);
        let mut best: Option<(usize, f64, Vec2)> = None;
        for i in 0..cands.len() {
            for j in i + 1..cands.len() {
                let Some((s, t, _)) = fit(&[cands[i], cands[j]]) else { continue };
                if !(0.1..=10.0).contains(&s) || cands[i].0.dist(cands[j].0) < 1e-6 {
                    continue;
                }
                let agree: Vec<(Vec2, Vec2)> = cands.iter().copied().filter(|(a, b)| (*a * s + t).dist(*b) <= tol).collect();
                if agree.len() >= MIN_GRID_PAIRS && best.as_ref().is_none_or(|b| agree.len() > b.0) {
                    // The fit of all that agree.
                    if let Some((s2, t2, miss)) = fit(&agree).filter(|f| f.2 <= tol && (0.1..=10.0).contains(&f.0)) {
                        let _ = miss;
                        best = Some((agree.len(), s2, t2));
                    }
                }
            }
        }
        if let Some((_, s, t)) = best {
            return Some((s, [t.x, t.y]));
        }
        None
    }
}

/// The screened (grey) background drawings of sheets: each view of an MEP or low-voltage sheet is
/// drawn over the architectural plan in grey, and a view with no grid bubble or room number is
/// put onto the plan by its background's outline.
pub(crate) struct Backgrounds {
    /// Per sheet rectangle, the outlines of its background drawings, largest first.
    sheets: Vec<([f64; 4], Vec<cadaddy_geom::Bounds2>)>,
    /// Per sheet, points along its background lines (every foot).
    lines: Vec<Vec<Vec2>>,
}

/// Grid cell for joining background lines into drawings, inches of the drawing.
const BACKGROUND_CELL_IN: f64 = 48.0;

impl Backgrounds {
    pub(crate) fn new(d: &Drawing, sheets: &[[f64; 4]]) -> Self {
        let k = units_per_inch(d);
        let grey = |c: &cadaddy_doc::color::Color| match c {
            cadaddy_doc::color::Color::True(rgb) => {
                let (r, g, b) = (rgb.0, rgb.1, rgb.2);
                r.min(g).min(b) >= 110 && r.max(g).max(b) - r.min(g).min(b) <= 24 && r.max(g).max(b) < 250
            }
            _ => false,
        };
        let cell = BACKGROUND_CELL_IN * k;
        let mut by_sheet: Vec<Vec<Vec2>> = vec![Vec::new(); sheets.len()];
        let mut lines: Vec<Vec<Vec2>> = vec![Vec::new(); sheets.len()];
        for e in plan_objects(d) {
            if !grey(&e.common.color) || !matches!(e.kind, EntityKind::Line(_) | EntityKind::LwPolyline(_)) {
                continue;
            }
            let b = cadaddy_doc::entity_bounds(d, e, 0);
            if b.is_empty() {
                continue;
            }
            let c = b.center();
            if let Some(i) = sheets.iter().position(|r| c.x >= r[0] && c.x <= r[2] && c.y >= r[1] && c.y <= r[3]) {
                by_sheet[i].extend([b.min, b.max, c]);
                lines[i].extend(line_samples(&[e], 12.0 * k));
            }
        }
        let clusters = |pts: &[Vec2]| -> Vec<cadaddy_geom::Bounds2> {
            use std::collections::{HashMap, HashSet};
            let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
            let mut cells: HashMap<(i64, i64), Vec<Vec2>> = HashMap::new();
            for p in pts {
                cells.entry(key(*p)).or_default().push(*p);
            }
            let mut seen: HashSet<(i64, i64)> = HashSet::new();
            let mut out = Vec::new();
            for start in cells.keys().copied().collect::<Vec<_>>() {
                if !seen.insert(start) {
                    continue;
                }
                let mut stack = vec![start];
                let mut b = cadaddy_geom::Bounds2::from_points(cells[&start].iter().copied());
                while let Some((x, y)) = stack.pop() {
                    for dx in -1..=1 {
                        for dy in -1..=1 {
                            let n = (x + dx, y + dy);
                            if let Some(ps) = cells.get(&n)
                                && seen.insert(n)
                            {
                                for p in ps {
                                    b.add(*p);
                                }
                                stack.push(n);
                            }
                        }
                    }
                }
                out.push(b);
            }
            out.sort_by(|a, b| ((b.max.x - b.min.x) * (b.max.y - b.min.y)).total_cmp(&((a.max.x - a.min.x) * (a.max.y - a.min.y))));
            out
        };
        Backgrounds { sheets: sheets.iter().zip(&by_sheet).map(|(r, pts)| (*r, clusters(pts))).collect(), lines }
    }

    /// The background line points inside `b` (one view's).
    pub(crate) fn points_in(&self, b: &cadaddy_geom::Bounds2) -> Vec<Vec2> {
        let c = b.center();
        let Some(i) = self.sheets.iter().position(|(r, _)| c.x >= r[0] && c.x <= r[2] && c.y >= r[1] && c.y <= r[3]) else { return Vec::new() };
        self.lines[i].iter().copied().filter(|p| b.contains(*p)).collect()
    }

    /// The background outline holding `p` (the smallest that does: a view, not the sheet).
    pub(crate) fn around(&self, p: [f64; 2]) -> Option<cadaddy_geom::Bounds2> {
        let at = Vec2::new(p[0], p[1]);
        let (_, cs) = self.sheets.iter().find(|(r, _)| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3])?;
        cs.iter().filter(|b| b.contains(at)).min_by(|a, b| ((a.max.x - a.min.x) * (a.max.y - a.min.y)).total_cmp(&((b.max.x - b.min.x) * (b.max.y - b.min.y)))).copied()
    }
}

/// Points every `step` along the lines and polylines of `ents`.
pub(crate) fn line_samples(ents: &[&Arc<Entity>], step: f64) -> Vec<Vec2> {
    let mut out = Vec::new();
    let mut along = |a: Vec2, b: Vec2| {
        let n = ((a.dist(b) / step).ceil() as usize).clamp(1, 10_000);
        for i in 0..=n {
            out.push(a + (b - a) * (i as f64 / n as f64));
        }
    };
    for e in ents {
        match &e.kind {
            EntityKind::Line(l) => along(l.a.xy(), l.b.xy()),
            EntityKind::LwPolyline(pl) => {
                for w in pl.vertices.windows(2) {
                    along(w[0].p, w[1].p);
                }
                if pl.closed && pl.vertices.len() > 2 {
                    along(pl.vertices[pl.vertices.len() - 1].p, pl.vertices[0].p);
                }
            }
            _ => {}
        }
    }
    out
}

/// Cells holding linework, for scoring an overlay.
struct Occupancy {
    cell: f64,
    cells: std::collections::HashSet<(i64, i64)>,
}

impl Occupancy {
    /// `pts` in cells of `cell`, each grown by `grow` cells (a tolerance).
    fn new(pts: &[Vec2], cell: f64, grow: i64) -> Self {
        let mut cells = std::collections::HashSet::new();
        for p in pts {
            let (i, j) = ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
            for di in -grow..=grow {
                for dj in -grow..=grow {
                    cells.insert((i + di, j + dj));
                }
            }
        }
        Occupancy { cell, cells }
    }
    fn hit(&self, p: Vec2) -> bool {
        self.cells.contains(&((p.x / self.cell).floor() as i64, (p.y / self.cell).floor() as i64))
    }
    /// The share of `pts` under `p ↦ p·s + t` that land on linework.
    fn score(&self, pts: &[Vec2], s: f64, t: Vec2) -> f64 {
        if pts.is_empty() {
            return 0.0;
        }
        pts.iter().filter(|p| self.hit(**p * s + t)).count() as f64 / pts.len() as f64
    }
}

/// The scale and offset that lay linework `src` (a view's background) over `tgt` (a floor plan),
/// with its score (F1 of the view's lines on the plan and the plan's lines under the view that the
/// view has) and its margin over the best fit 10' or more away; `None` without one.
/// Votes on a 4' grid over scales 0.4-2.5, then refines on a 1' grid.
pub(crate) fn linework_fit(src: &[Vec2], tgt: &[Vec2], k: f64) -> Option<(f64, Vec2, f64, f64)> {
    if src.len() < 50 || tgt.len() < 50 {
        return None;
    }
    let coarse = 48.0 * k;
    let tcells: std::collections::HashSet<(i64, i64)> =
        tgt.iter().map(|p| ((p.x / coarse).floor() as i64, (p.y / coarse).floor() as i64)).collect();
    // A few thousand source points are enough to score with.
    let stride = (src.len() / 2000).max(1);
    let few: Vec<Vec2> = src.iter().step_by(stride).copied().collect();
    let mut cands: Vec<(f64, f64, (i64, i64))> = Vec::new();
    // A view with neither grids nor tags is the whole floor: placed, it spans half the plan's
    // linework or more (not a shrunk copy in a dense corner).
    let (sb, tb) = (cadaddy_geom::Bounds2::from_points(few.iter().copied()), cadaddy_geom::Bounds2::from_points(tgt.iter().copied()));
    let (sw, tw) = (sb.width().max(sb.height()), tb.width().max(tb.height()));
    let mut s = 0.4;
    while s <= 2.5 {
        if sw * s < 0.5 * tw || sw * s > 1.2 * tw {
            s *= 1.01;
            continue;
        }
        let scells: std::collections::HashSet<(i64, i64)> =
            few.iter().map(|p| ((p.x * s / coarse).floor() as i64, (p.y * s / coarse).floor() as i64)).collect();
        let mut votes: std::collections::HashMap<(i64, i64), u32> = std::collections::HashMap::new();
        for u in &scells {
            for v in &tcells {
                *votes.entry((v.0 - u.0, v.1 - u.1)).or_default() += 1;
            }
        }
        // The coarse grid only proposes an offset per scale: a dense plan has lines in nearly
        // every coarse cell, so placements are told apart on the fine grid.
        // Ties go to the smallest offset: a hash map's order differs run to run, and with it the
        // fit a tie picked (the same job placed 71 fewer devices one run than the next).
        if let Some((o, n)) = votes.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0))) {
            cands.push((f64::from(n) / scells.len() as f64, s, o));
        }
        s *= 1.01;
    }
    let fine = Occupancy::new(tgt, 12.0 * k, 1);
    // Refine every scale's proposal on the 1' grid: scale ±0.5%, offset within two coarse cells.
    let mut refined: Vec<(f64, f64, Vec2)> = Vec::new();
    for &(_, s0, o) in &cands {
        let t0 = Vec2::new(o.0 as f64 * coarse, o.1 as f64 * coarse);
        let mut best = (0.0, s0, t0);
        for si in -1..=1 {
            let s = s0 * (1.0 + 0.005 * f64::from(si));
            for dx in -8..=8 {
                for dy in -8..=8 {
                    let t = t0 + Vec2::new(f64::from(dx), f64::from(dy)) * (coarse / 4.0);
                    let sc = fine.score(&few, s, t);
                    if sc > best.0 {
                        best = (sc, s, t);
                    }
                }
            }
        }
        // F1 of the refined placement: the plan's lines under the view that the view has too.
        let (_, s, t) = best;
        let mine = Occupancy::new(&few.iter().map(|p| *p * s + t).collect::<Vec<_>>(), 12.0 * k, 1);
        let placed = cadaddy_geom::Bounds2::from_points(few.iter().map(|p| *p * s + t));
        let under: Vec<Vec2> = tgt.iter().copied().filter(|p| placed.contains(*p)).step_by((tgt.len() / 20_000).max(1)).collect();
        let precision = if under.is_empty() { 0.0 } else { under.iter().filter(|p| mine.hit(**p)).count() as f64 / under.len() as f64 };
        let f = if best.0 + precision > 0.0 { 2.0 * best.0 * precision / (best.0 + precision) } else { 0.0 };
        refined.push((f, s, t));
    }
    refined.sort_by(|a, b| b.0.total_cmp(&a.0));
    // Polish the best few on 3" steps and 0.1% of scale: a right fit a few inches off loses
    // points to a wrong one that happens to sit on lines.
    // Coverage is measured against the view's lines in full (a thinned sample leaves gaps that
    // read as plan lines the view doesn't have).
    let dense: Vec<Vec2> = src.iter().step_by((src.len() / 12_000).max(1)).copied().collect();
    // `mine`: the view's lines on a grid in its own terms, made for scale near `s` (a plan point is
    // mapped back into it rather than the view mapped out for each trial placement).
    let view_grid = |s: f64| Occupancy::new(&dense, 12.0 * k / s, 1);
    let f1 = |s: f64, t: Vec2, mine: &Occupancy| {
        let r = fine.score(&few, s, t);
        let placed = cadaddy_geom::Bounds2::from_points(few.iter().map(|p| *p * s + t));
        let under: Vec<Vec2> = tgt.iter().copied().filter(|p| placed.contains(*p)).step_by((tgt.len() / 20_000).max(1)).collect();
        let p = if under.is_empty() { 0.0 } else { under.iter().filter(|q| mine.hit((**q - t) * (1.0 / s))).count() as f64 / under.len() as f64 };
        if r + p > 0.0 { 2.0 * r * p / (r + p) } else { 0.0 }
    };
    for cand in refined.iter_mut().take(4) {
        let (_, s0, t0) = *cand;
        let mine = view_grid(s0);
        let mut best = (f1(s0, t0, &mine), s0, t0);
        for si in -2..=2 {
            let s = s0 * (1.0 + 0.001 * f64::from(si));
            for dx in -4..=4 {
                for dy in -4..=4 {
                    let t = t0 + Vec2::new(f64::from(dx), f64::from(dy)) * (3.0 * k);
                    let sc = f1(s, t, &mine);
                    if sc > best.0 {
                        best = (sc, s, t);
                    }
                }
            }
        }
        *cand = best;
    }
    // The rest scored the same way, so the margin compares like with like.
    for cand in refined.iter_mut().skip(4) {
        cand.0 = f1(cand.1, cand.2, &view_grid(cand.1));
    }
    refined.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (score, s, t) = *refined.first()?;
    // The best placement well away from it (one bay over, another view).
    let probe = few.iter().fold(Vec2::new(0.0, 0.0), |a, p| a + *p) * (1.0 / few.len() as f64);
    let at = probe * s + t;
    let second = refined.iter().skip(1).filter(|(_, s2, t2)| (probe * *s2 + *t2).dist(at) >= 120.0 * k).map(|r| r.0).fold(0.0, f64::max);
    Some((s, t, score, score - second))
}
