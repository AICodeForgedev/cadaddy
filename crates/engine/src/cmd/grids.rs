//! Structural grids: found on each level's plan, used to line the levels (and their sheets) up,
//! and to place the columns drawn at their intersections.
//!
//! - `grids.detect`: grid bubbles (circles 4"–24" in radius around a short label: `A`, `3`,
//!   `C.1`) and the grid line touching each (the longest line with an end at the bubble), per
//!   level; and their intersections, labelled `A-1` (letters first).
//! - `levels.align {reference?, apply?}`: each level's intersections matched by label with the
//!   reference level's give its scale and offset (least squares: `q ≈ s·p + t`). Applied, every
//!   level's base point becomes its point of the same grid intersection, so the levels stack
//!   exactly; a level drawn at another scale is reported and left (levels share one drawing
//!   scale).
//! - `grids.columns {level, apply?}`: the column outlines (closed outlines and circles up to 4'
//!   across, on structural or column layers) centred on the level's grid intersections, into
//!   the spec's `columns`; `model.build` stands them up through their storeys.

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
            .params("{level?: name} → {levels: [{level, grids: [{label, from, to, bubble}], intersections: [{label, at}]}]}")
            .noundo(),
        CommandSpec::new("levels.align", "Align Levels by Grids", run_align).params(
            "{reference?: level (default the first with grids), apply?: bool (set each level's base point to its point of a common grid intersection)} → {reference, levels: [{level, matched, scale, offset, residual, base?, note?}]}",
        ),
        CommandSpec::new("grids.columns", "Columns at Grids", run_columns).params(
            "{level: name, to_level?: name, apply?: bool (into the spec's columns, replacing the level's)} → {columns: [{grid, at, size_in, round}]}",
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

/// The grids of a set of objects.
fn grids(ents: &[&Arc<Entity>], k: f64) -> Vec<Grid> {
    let notes = texts(ents);
    let lines: Vec<(Vec2, Vec2)> = ents
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) => Some((l.a.xy(), l.b.xy())),
            _ => None,
        })
        .filter(|(a, b)| a.dist(*b) >= 120.0 * k)
        .take(MAX_ITEMS * 10)
        .collect();
    let mut out: Vec<Grid> = Vec::new();
    for e in ents {
        let EntityKind::Circle(c) = &e.kind else { continue };
        let (o, r) = (c.center.xy(), c.radius);
        if !(r >= 4.0 * k && r <= 24.0 * k) {
            continue;
        }
        let Some((_, label, _, _)) = notes.iter().find(|(_, t, at, _)| at.dist(o) <= r && grid_label(t)) else { continue };
        // The longest line with an end on (or in) the bubble.
        let touching =
            lines.iter().filter(|(a, b)| a.dist(o) <= 1.3 * r || b.dist(o) <= 1.3 * r).max_by(|x, y| x.0.dist(x.1).total_cmp(&y.0.dist(y.1)));
        let Some(&(a, b)) = touching else { continue };
        // From the bubble end to the far end.
        let (a, b) = if a.dist(o) <= b.dist(o) { (a, b) } else { (b, a) };
        if out.iter().any(|g| g.label.eq_ignore_ascii_case(label) && g.a.dist(a) < 1e-6) {
            continue;
        }
        out.push(Grid { label: label.trim().to_string(), a, b, bubble: o });
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
    out
}

/// Each level's grids and intersections.
fn by_level(d: &Drawing, sp: &BuildingSpec) -> Vec<(String, Vec<Grid>, Vec<(String, Vec2)>)> {
    let k = units_per_inch(d);
    let all = plan_objects(d);
    let centre = |e: &Entity| {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        (!b.is_empty()).then(|| b.center())
    };
    let mut out = Vec::new();
    for l in sp.levels.iter().filter(|l| l.sheet.is_none()) {
        let mine: Vec<&Arc<Entity>> =
            all.iter().copied().filter(|e| centre(e).and_then(|c| level_at(&sp.levels, c.x, c.y)).is_some_and(|x| x.name == l.name)).collect();
        let gs = grids(&mine, k);
        let xs = intersections(&gs, k);
        out.push((l.name.clone(), gs, xs));
    }
    out
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
    let levels: Vec<Value> = by_level(s.doc()?, &sp)
        .into_iter()
        .filter(|(l, _, _)| only.is_none_or(|o| o.eq_ignore_ascii_case(l)))
        .map(|(l, gs, xs)| {
            json!({
                "level": l,
                "grids": gs.iter().map(|g| json!({"label": g.label, "from": pt(g.a), "to": pt(g.b), "bubble": pt(g.bubble)})).collect::<Vec<_>>(),
                "intersections": xs.iter().map(|(n, p)| json!({"label": n, "at": pt(*p)})).collect::<Vec<_>>(),
            })
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
    let found = by_level(s.doc()?, &sp);
    let reference = match str_param(p, "reference") {
        Some(r) => sp.level(r).map(|l| l.name.clone()).ok_or_else(|| bad("levels.align", format!("there is no level named {r}")))?,
        None => found
            .iter()
            .find(|(_, _, xs)| !xs.is_empty())
            .map(|(l, _, _)| l.clone())
            .ok_or_else(|| bad("levels.align", "no level has grid intersections"))?,
    };
    let Some((_, _, ref_xs)) = found.iter().find(|(l, _, _)| *l == reference) else {
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
    for (l, _, xs) in &found {
        let pairs: Vec<(Vec2, Vec2)> = xs.iter().filter_map(|(n, pp)| ref_xs.iter().find(|(m, _)| m == n).map(|(_, q)| (*pp, *q))).collect();
        let mut row = json!({ "level": l, "matched": pairs.len() });
        let Some((scale, t, miss)) = fit(&pairs).filter(|_| !pairs.is_empty()) else {
            row["note"] = json!("no grid intersections in common with the reference");
            rows.push(row);
            continue;
        };
        row["scale"] = json!((scale * 1e9).round() / 1e9);
        row["offset"] = pt(t);
        row["residual"] = json!((miss * 1e6).round() / 1e6);
        if (scale - 1.0).abs() > 1e-3 {
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
    let d = s.doc()?;
    let k = units_per_inch(d);
    let found = by_level(d, &sp);
    let Some((_, _, xs)) = found.iter().find(|(l, _, _)| *l == level) else { return Err(bad("grids.columns", "no grids on that level")) };
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
            (!b.is_empty() && b.width() <= 48.0 * k && b.height() <= 48.0 * k && b.center().dist(*at) <= 6.0 * k).then_some((b, round))
        });
        if let Some((b, round)) = shape {
            let r8 = |v: f64| (v / k * 8.0).round() / 8.0;
            columns.push(GridColumn {
                level: level.clone(),
                grid: label.clone(),
                at: [at.x, at.y],
                size_in: [r8(b.width()), r8(b.height())],
                round,
                to_level: to_level.clone(),
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
        let xdata = vec![
            t(1001, super::wallgen::APP.into()),
            t(1000, "column".into()),
            t(1000, level.name.clone()),
            t(1000, Handle(0).hex()),
            t(1000, format!("grid:{}", c.grid)),
        ];
        if d.add(&Space::Model, Common { layer: "3D-Columns".into(), xdata, ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
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
}
