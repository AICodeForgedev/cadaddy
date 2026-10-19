//! Duct, pipe and conduit runs in 3D, from single-line plans, and the clashes they make.
//!
//! - `mep.read {level?, apply?}`: each system's layers (the spec's `mep.systems`, else systems
//!   drafted from standard layer names: `M-DUCT*`, `M-HVAC-SUPP`… ducts; `P-*`, `M-PIPE*`,
//!   `M-HWS`… pipes; `E-COND*` conduit) are chained into runs (polylines, and lines joined end
//!   to end). Tags beside a segment give its size (`24x12` duct, `10"Ø` / `10" DIA` round,
//!   `2" CW` pipe, `3/4" C` conduit) and height (`BOT @ 10'-0"` bottom, `CL @ 9'-6"` centreline,
//!   `TOP @ 11'-0"` top, `EL 10'-0"`); a segment without one takes its neighbour's (else the
//!   system's). A change of height between two segments is a drop at their corner.
//! - `model.build` makes each run's segments (rectangular boxes or round tubes), fittings at the
//!   turns and the drops, on a layer per system (`3D-MEP-<system>`); a system with `show: false`
//!   is left out.
//! - `model.clashes {tolerance_in?}`: runs hanging below their room's ceiling, reaching into the
//!   floor structure above (the next level's slab), crossing another system's run, or running
//!   through a column.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag, Space};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, MepKind, MepRun, MepSystem, level_at};
use serde_json::{Value, json};
use std::sync::Arc;

use super::bspec::{current, plan_objects};
use super::perceive::{feet_inches, texts};
use super::*;
use crate::{Result, Session};

/// Runs and segments handled, at most (hostile drawings).
const MAX_RUNS: usize = 20_000;
const MAX_SEGS: usize = 200_000;
/// The next floor's slab depth under its level, feet (as WALLGEN builds it).
const SLAB_FT: f64 = 0.5;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mep.read", "Read MEP Runs", run_read).params(
            "{level?: name (default every level), apply?: bool (systems and runs into the spec, replacing those levels' runs)} → {systems: [{name, layer, kind, size_in, bottom_ft}], runs: [{system, level, points, size_in, bottoms_ft, objects}]}",
        ),
        CommandSpec::new("model.clashes", "Model Clashes", run_clashes)
            .params("{tolerance_in?: inches (0)} → {clashes: [{kind: below_ceiling|into_structure|crossing|column, message, systems, at, by_in, handles}], total}")
            .noundo(),
    ]
}

/// Systems from standard layer names, for drawings whose spec names none.
fn draft_system(layer: &str) -> Option<MepSystem> {
    let up = layer.to_ascii_uppercase();
    let parts: Vec<&str> = up.split(['-', '_', ' ']).collect();
    let has = |w: &str| parts.contains(&w);
    let (kind, size, bottom) = if up.starts_with("M-") && (has("DUCT") || has("SUPP") || has("RETN") || has("EXHS") || has("HVAC")) {
        (MepKind::Duct, [12.0, 12.0], 10.0)
    } else if up.starts_with("P-") || (up.starts_with("M-") && (has("PIPE") || has("HWS") || has("CHWS") || has("CHWR") || has("STEM"))) {
        (MepKind::Pipe, [2.0, 0.0], 10.0)
    } else if up.starts_with("E-") && (has("COND") || has("CNDT")) {
        (MepKind::Conduit, [1.0, 0.0], 10.0)
    } else {
        return None;
    };
    Some(MepSystem { name: layer.to_string(), layer: layer.to_string(), kind, size_in: size, bottom_ft: bottom, show: true })
}

/// A size tag, inches: `24x12` → [24, 12]; `10"Ø`, `10" DIA`, `2" CW`, `3/4" C` → [d, 0].
pub(crate) fn size_tag(t: &str) -> Option<[f64; 2]> {
    let up = t.to_ascii_uppercase().replace('×', "X");
    let num = |s: &str| -> Option<f64> {
        let s = s.trim().trim_end_matches('"');
        match s.split_once('/') {
            Some((a, b)) => {
                let (whole, a) = match a.trim().rsplit_once(' ') {
                    Some((w, a)) => (w.trim().parse::<f64>().ok()?, a),
                    None => (0.0, a),
                };
                Some(whole + a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok().filter(|b| *b != 0.0)?)
            }
            None => s.parse::<f64>().ok(),
        }
    };
    // `24x12` (or `24"x12"`)
    for w in up.split_whitespace() {
        if let Some((a, b)) = w.split_once('X')
            && let (Some(a), Some(b)) = (num(a), num(b))
            && a > 0.0
            && b > 0.0
        {
            return Some([a, b]);
        }
    }
    // `10"Ø`, `10" DIA`, `2" CW`, `3/4" C`: a number in inches.
    let words: Vec<&str> = up.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let round = w.contains('Ø') || w.contains("DIA") || words.get(i + 1).is_some_and(|n| n.starts_with("DIA") || n.starts_with('Ø'));
        let inch = w.contains('"');
        if !(round || inch) || w.contains('\'') {
            continue;
        }
        let digits: String = w.chars().take_while(|c| c.is_ascii_digit() || *c == '/' || *c == '.').collect();
        // `1 1/2"`
        let value = match (words.get(i.wrapping_sub(1)).and_then(|p| p.parse::<f64>().ok()).filter(|_| i > 0 && digits.contains('/')), num(&digits)) {
            (Some(whole), Some(f)) => Some(whole + f),
            (_, v) => v,
        };
        if let Some(v) = value.filter(|v| *v > 0.0 && *v <= 240.0) {
            return Some([v, 0.0]);
        }
    }
    None
}

/// A height tag, feet, as the run's bottom given its section height (feet).
pub(crate) fn height_tag(t: &str, section_ft: f64) -> Option<f64> {
    let up = t.to_ascii_uppercase();
    // The value after the keyword: a size before it (`2" CW CL @ 10'-6"`) is not a height.
    let after = |keys: &[&str]| {
        keys.iter().filter_map(|k| up.find(k).map(|i| i + k.len())).min().and_then(|i| up.get(i..)).and_then(feet_inches).map(|i| i / 12.0)
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

/// Chains of segments joined end to end (polylines stay whole), with their source objects.
fn chains(ents: &[&Arc<Entity>]) -> Vec<(Vec<Vec2>, Vec<Handle>)> {
    let mut pieces: Vec<(Vec<Vec2>, Vec<Handle>)> = Vec::new();
    for e in ents {
        let pts: Vec<Vec2> = match &e.kind {
            EntityKind::Line(l) => vec![l.a.xy(), l.b.xy()],
            EntityKind::LwPolyline(p) => {
                let mut v: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
                if p.closed
                    && let Some(f) = v.first().copied()
                {
                    v.push(f);
                }
                v
            }
            _ => continue,
        };
        if pts.len() >= 2 && pts.iter().all(|p| p.is_finite()) {
            pieces.push((pts, vec![e.handle]));
        }
        if pieces.len() >= MAX_SEGS {
            break;
        }
    }
    // Join pieces sharing an end.
    let tol = 1e-6;
    let mut out: Vec<(Vec<Vec2>, Vec<Handle>)> = Vec::new();
    while let Some(mut cur) = (!pieces.is_empty()).then(|| pieces.remove(0)) {
        while let (Some(&head), Some(&tail)) = (cur.0.first(), cur.0.last()) {
            let Some(i) = pieces.iter().position(|(p, _)| {
                p.first().is_some_and(|q| q.dist(tail) < tol || q.dist(head) < tol)
                    || p.last().is_some_and(|q| q.dist(tail) < tol || q.dist(head) < tol)
            }) else {
                break;
            };
            let (mut p, hs) = pieces.remove(i);
            if p.first().is_some_and(|q| q.dist(tail) < tol) {
                cur.0.extend(p.into_iter().skip(1));
            } else if p.last().is_some_and(|q| q.dist(tail) < tol) {
                p.reverse();
                cur.0.extend(p.into_iter().skip(1));
            } else if p.last().is_some_and(|q| q.dist(head) < tol) {
                p.pop();
                p.extend(cur.0);
                cur.0 = p;
            } else {
                p.reverse();
                p.pop();
                p.extend(cur.0);
                cur.0 = p;
            }
            cur.1.extend(hs);
        }
        out.push(cur);
        if out.len() >= MAX_RUNS {
            break;
        }
    }
    out
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + d * t)
}

/// The runs of the plan: systems (the spec's, else drafted from layer names) and their runs.
fn read(s: &Session, sp: &BuildingSpec, only: Option<&str>) -> Result<(Vec<MepSystem>, Vec<MepRun>)> {
    let plan = super::verify::plan_session(s)?;
    let d: &Drawing = plan.doc()?;
    let ents = plan_objects(d);
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut systems = sp.mep.systems.clone();
    if systems.is_empty() {
        let mut layers: Vec<String> = ents.iter().map(|e| e.common.layer.clone()).collect();
        layers.sort();
        layers.dedup();
        systems = layers.iter().filter_map(|l| draft_system(l)).collect();
    }
    let notes = texts(&ents);
    let mut runs = Vec::new();
    for sy in &systems {
        let mine: Vec<&Arc<Entity>> = ents.iter().copied().filter(|e| super::qselect::wildcard(&sy.layer, &e.common.layer)).collect();
        // Tags: texts on the system's layers, else on any layer near its runs.
        for (pts, handles) in chains(&mine) {
            let Some(first) = pts.first() else { continue };
            let Some(level) = level_at(&sp.levels, first.x, first.y) else { continue };
            if only.is_some_and(|o| !o.eq_ignore_ascii_case(&level.name)) {
                continue;
            }
            let segs: Vec<(Vec2, Vec2)> = pts
                .windows(2)
                .filter_map(|w| match w {
                    [a, b] => Some((*a, *b)),
                    _ => None,
                })
                .collect();
            let reach = 4.0 * upf;
            // Each tag goes to its nearest segment of this run (within reach).
            let mut size: Option<[f64; 2]> = None;
            let mut heights: Vec<Option<(f64, String)>> = vec![None; segs.len()];
            for (_, t, at, _) in &notes {
                let near = segs.iter().enumerate().map(|(i, (a, b))| (i, seg_dist(*at, *a, *b))).min_by(|x, y| x.1.total_cmp(&y.1));
                let Some((i, dist)) = near.filter(|(_, dd)| *dd <= reach) else { continue };
                // Not nearer to another system's run.
                let other_nearer = ents
                    .iter()
                    .filter(|e| !mine.iter().any(|m| m.handle == e.handle))
                    .filter(|e| systems.iter().any(|o| o.name != sy.name && super::qselect::wildcard(&o.layer, &e.common.layer)))
                    .any(|e| super::perceive::geometry::segments(&[e]).iter().any(|g| seg_dist(*at, g.a, g.b) < dist - 1e-9));
                if other_nearer {
                    continue;
                }
                if let Some(sz) = size_tag(t) {
                    size.get_or_insert(sz);
                }
                if let Some(slot) = heights.get_mut(i)
                    && slot.is_none()
                {
                    *slot = Some((0.0, t.clone()));
                }
            }
            let size = size.unwrap_or(sy.size_in);
            let section_ft = if size[1] > 0.0 { size[1] } else { size[0] } / 12.0;
            let mut bottoms: Vec<Option<f64>> = heights.iter().map(|h| h.as_ref().and_then(|(_, t)| height_tag(t, section_ft))).collect();
            // Untagged segments take their neighbour's height (forward, then back), else the system's.
            let mut last = None;
            for b in bottoms.iter_mut() {
                match b {
                    Some(v) => last = Some(*v),
                    None => *b = last,
                }
            }
            let mut next = None;
            for b in bottoms.iter_mut().rev() {
                match b {
                    Some(v) => next = Some(*v),
                    None => *b = next,
                }
            }
            let round = |v: f64| (v * 9600.0).round() / 9600.0;
            runs.push(MepRun {
                system: sy.name.clone(),
                level: level.name.clone(),
                points: pts.iter().map(|p| [p.x, p.y]).collect(),
                size_in: size,
                bottoms_ft: bottoms.into_iter().map(|b| round(b.unwrap_or(sy.bottom_ft))).collect(),
                objects: handles.iter().map(|h| h.hex()).collect(),
            });
            if runs.len() >= MAX_RUNS {
                break;
            }
        }
    }
    Ok((systems, runs))
}

fn run_read(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "mep.read")?;
    let only = str_param(p, "level").map(str::to_string);
    if let Some(l) = only.as_deref().filter(|l| sp.level(l).is_none()) {
        return Err(bad("mep.read", format!("there is no level named {l}")));
    }
    let (systems, runs) = read(s, &sp, only.as_deref())?;
    let apply = bool_or(p, "apply", false);
    let out = json!({
        "systems": serde_json::to_value(&systems).unwrap_or(Value::Null),
        "runs": serde_json::to_value(&runs).unwrap_or(Value::Null),
        "applied": apply,
    });
    if apply {
        if sp.mep.systems.is_empty() {
            sp.mep.systems = systems;
        }
        let levels: Vec<String> = match &only {
            Some(l) => vec![l.clone()],
            None => sp.levels.iter().map(|l| l.name.clone()).collect(),
        };
        sp.mep.runs.retain(|r| !levels.iter().any(|l| l.eq_ignore_ascii_case(&r.level)));
        sp.mep.runs.extend(runs);
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(out)
}

// ---------------- in 3D ----------------

/// A run's pieces, sheet units: straight segments and fittings, each `(a, b, z0)` along the
/// plan with its bottom (risers: a == b, with `z1`).
#[derive(Clone, Copy, Debug)]
struct Piece {
    a: Vec2,
    b: Vec2,
    z0: f64,
    z1: f64,
    fitting: bool,
}

/// The run as pieces: its segments (and, at a drop, the riser between their heights; at each
/// turn or drop a fitting the size of the section).
fn pieces(run: &MepRun, base_z: f64, upf: f64) -> (Vec<Piece>, usize, usize) {
    let h = if run.size_in[1] > 0.0 { run.size_in[1] } else { run.size_in[0] } / 12.0 * upf;
    let pts: Vec<Vec2> = run.points.iter().map(|p| Vec2::new(p[0], p[1])).collect();
    let mut out = Vec::new();
    let (mut elbows, mut drops) = (0, 0);
    for (i, w2) in pts.windows(2).enumerate() {
        let [a, b] = [w2[0], w2[1]];
        let z = base_z + run.bottoms_ft.get(i).copied().unwrap_or(0.0) * upf;
        out.push(Piece { a, b, z0: z, z1: z + h, fitting: false });
        // The corner to the next segment.
        if let Some(c) = pts.get(i + 2) {
            let (u, v) = (b - a, *c - b);
            let turn = u.len() > 1e-9 && v.len() > 1e-9 && (u.cross(v) / (u.len() * v.len())).abs() > 0.09;
            let z2 = base_z + run.bottoms_ft.get(i + 1).copied().unwrap_or(0.0) * upf;
            if (z2 - z).abs() > 1e-9 {
                drops += 1;
                out.push(Piece { a: b, b, z0: z.min(z2), z1: z.max(z2) + h, fitting: true });
            } else if turn {
                elbows += 1;
                out.push(Piece { a: b, b, z0: z, z1: z + h, fitting: true });
            }
        }
    }
    (out, elbows, drops)
}

/// A piece as a mesh: a box (rectangular, or a corner), or an octagonal tube (round).
fn piece_mesh(m: &mut Mesh, p: &Piece, w: f64, round: bool) {
    if p.a.dist(p.b) <= 1e-9 {
        // A fitting or riser: a square post the section's width.
        let half = Vec2::new(w / 2.0, 0.0);
        super::wallgen::add_box(m, p.a - half, p.a + half, w, p.z0, p.z1);
        return;
    }
    if !round {
        super::wallgen::add_box(m, p.a, p.b, w, p.z0, p.z1);
        return;
    }
    let len = p.a.dist(p.b);
    let u = (p.b - p.a) * (1.0 / len);
    let n = Vec2::new(-u.y, u.x);
    let r = w / 2.0;
    let zc = (p.z0 + p.z1) / 2.0;
    let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
    for end in [p.a, p.b] {
        for k in 0..8 {
            let t = f64::from(k) * std::f64::consts::TAU / 8.0;
            let q = end + n * (r * t.cos());
            m.vertices.push(Vec3::new(q.x, q.y, zc + r * t.sin()));
        }
    }
    for k in 0..8u32 {
        let j = (k + 1) % 8;
        m.faces.push(MeshFace { v: vec![base + k, base + j, base + 8 + j, base + 8 + k], hidden: 0 });
    }
    // The ends: fans of triangles, facing out.
    for k in 1..7u32 {
        m.faces.push(MeshFace { v: vec![base, base + k + 1, base + k], hidden: 0 });
        m.faces.push(MeshFace { v: vec![base + 8, base + 8 + k, base + 8 + k + 1], hidden: 0 });
    }
}

/// The shown systems' runs into model space (sheet units), one mesh per run on its system's
/// layer. The counts.
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec) -> Value {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let (mut runs, mut elbows, mut drops) = (0, 0, 0);
    for r in sp.mep.runs.iter().take(MAX_RUNS) {
        let Some(sy) = sp.mep.systems.iter().find(|s| s.name == r.system).filter(|s| s.show) else { continue };
        let Some(level) = sp.level(&r.level) else { continue };
        let layer =
            format!("3D-MEP-{}", sy.name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(60).collect::<String>());
        if d.layer(&layer).is_none() {
            let color = match sy.kind {
                MepKind::Duct => 4,
                MepKind::Pipe => 3,
                MepKind::Conduit => 6,
            };
            d.layers.push(cadaddy_doc::Layer {
                name: layer.clone(),
                color: cadaddy_doc::color::Color::Index(color),
                ..cadaddy_doc::Layer::default()
            });
        }
        let (ps, e, dr) = pieces(r, level.elevation_ft * upf, upf);
        let w = r.size_in[0] / 12.0 * upf;
        let mut m = super::wallgen::empty_mesh();
        for p in &ps {
            piece_mesh(&mut m, p, w, r.size_in[1] <= 0.0 && !p.fitting);
        }
        let t = |code: i32, v: String| RawTag { code, value: v };
        let source = r.objects.first().and_then(|h| u64::from_str_radix(h, 16).ok()).map_or(Handle(0), Handle);
        let xdata = vec![
            t(1001, super::wallgen::APP.into()),
            t(1000, "mep".into()),
            t(1000, level.name.clone()),
            t(1000, source.hex()),
            t(1000, format!("system:{}", sy.name)),
        ];
        if !m.faces.is_empty() && d.add(&Space::Model, Common { layer, xdata, ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            runs += 1;
            elbows += e;
            drops += dr;
        }
    }
    json!({ "mepRuns": runs, "elbows": elbows, "drops": drops })
}

// ---------------- clashes ----------------

fn run_clashes(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "model.clashes")?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let tol = p.get("tolerance_in").and_then(Value::as_f64).filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(0.0) / 12.0 * upf;
    let d = s.doc()?;
    let mut out: Vec<Value> = Vec::new();
    let r6 = |v: f64| (v * 1000.0).round() / 1000.0;
    // Every run's pieces, with its system and room.
    struct Item<'a> {
        run: &'a MepRun,
        piece: Piece,
        w: f64,
    }
    let mut items: Vec<Item> = Vec::new();
    for r in sp.mep.runs.iter().take(MAX_RUNS) {
        let Some(level) = sp.level(&r.level) else { continue };
        let (ps, _, _) = pieces(r, level.elevation_ft * upf, upf);
        let w = r.size_in[0] / 12.0 * upf;
        items.extend(ps.into_iter().filter(|p| !p.fitting).map(|piece| Item { run: r, piece, w }));
    }
    let handles = |r: &MepRun| r.objects.iter().take(5).cloned().collect::<Vec<_>>();
    // Below the ceiling of the room it passes through (else the level's), into the slab above.
    let rooms: Vec<(String, f64, Vec<Vec2>)> = d
        .model
        .iter()
        .filter(|e| super::wallgen::made_as(e).as_deref() == Some("ceiling"))
        .filter_map(|e| {
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let z = m.vertices.first()?.z;
            let poly: Vec<Vec2> = m.vertices.iter().map(|v| v.xy()).collect();
            Some((super::bspec::made_level(e).unwrap_or_default(), z, hull(&poly)))
        })
        .collect();
    for it in &items {
        let Some(level) = sp.level(&it.run.level) else { continue };
        let mid = (it.piece.a + it.piece.b) * 0.5;
        let ceiling = rooms
            .iter()
            .filter(|(l, _, poly)| l.eq_ignore_ascii_case(&level.name) && cadaddy_geom::point_in_polygon(poly, mid))
            .map(|(_, z, _)| *z)
            .fold(f64::NAN, f64::min);
        let ceiling = if ceiling.is_finite() { ceiling } else { (level.elevation_ft + level.ceiling_ft) * upf };
        if it.piece.z0 < ceiling - tol {
            out.push(json!({
                "kind": "below_ceiling", "systems": [it.run.system],
                "message": format!("{} hangs {} below the ceiling", it.run.system, cadaddy_walk3d::inches((ceiling - it.piece.z0) / upf * 12.0)),
                "at": [r6(mid.x), r6(mid.y)], "by_in": r6((ceiling - it.piece.z0) / upf * 12.0), "handles": handles(it.run),
            }));
        }
        let above = sp.levels.iter().map(|l| l.elevation_ft).filter(|e| *e > level.elevation_ft + 1e-6).fold(f64::INFINITY, f64::min);
        if above.is_finite() {
            let structure = (above - SLAB_FT) * upf;
            if it.piece.z1 > structure + tol {
                out.push(json!({
                    "kind": "into_structure", "systems": [it.run.system],
                    "message": format!("{} reaches {} into the floor structure above", it.run.system, cadaddy_walk3d::inches((it.piece.z1 - structure) / upf * 12.0)),
                    "at": [r6(mid.x), r6(mid.y)], "by_in": r6((it.piece.z1 - structure) / upf * 12.0), "handles": handles(it.run),
                }));
            }
        }
    }
    // Runs of two systems crossing: plan distance under their half-widths, heights overlapping.
    let n = items.len().min(20_000);
    for i in 0..n {
        for j in i + 1..n {
            let (Some(a), Some(b)) = (items.get(i), items.get(j)) else { continue };
            if a.run.system == b.run.system || a.run.level != b.run.level {
                continue;
            }
            let zo = a.piece.z1.min(b.piece.z1) - a.piece.z0.max(b.piece.z0);
            if zo <= tol {
                continue;
            }
            let dist = seg_seg(a.piece.a, a.piece.b, b.piece.a, b.piece.b);
            let clear = (a.w + b.w) / 2.0;
            if dist + tol < clear {
                let at = closest_mid(a.piece.a, a.piece.b, b.piece.a, b.piece.b);
                out.push(json!({
                    "kind": "crossing", "systems": [a.run.system, b.run.system],
                    "message": format!("{} crosses {} ({} overlap in height)", a.run.system, b.run.system, cadaddy_walk3d::inches(zo / upf * 12.0)),
                    "at": [r6(at.x), r6(at.y)], "by_in": r6(zo / upf * 12.0),
                    "handles": handles(a.run).into_iter().chain(handles(b.run)).collect::<Vec<_>>(),
                }));
            }
        }
    }
    // Through a column.
    let columns: Vec<(Handle, cadaddy_geom::Bounds2, f64, f64)> = d
        .model
        .iter()
        .filter(|e| super::wallgen::made_as(e).as_deref() == Some("column"))
        .filter_map(|e| {
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let (z0, z1) = m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)));
            Some((e.handle, cadaddy_doc::entity_bounds(d, e, 0), z0, z1))
        })
        .collect();
    for it in &items {
        for (h, b, z0, z1) in &columns {
            let inside = b.expand(it.w / 2.0 - tol);
            let hits = (0..=20).any(|k| inside.contains(it.piece.a + (it.piece.b - it.piece.a) * (f64::from(k) / 20.0)));
            if hits && it.piece.z0 < *z1 && it.piece.z1 > *z0 {
                out.push(json!({
                    "kind": "column", "systems": [it.run.system],
                    "message": format!("{} runs through a column", it.run.system),
                    "at": [r6(b.center().x), r6(b.center().y)], "by_in": 0.0,
                    "handles": handles(it.run).into_iter().chain([h.hex()]).collect::<Vec<_>>(),
                }));
            }
        }
    }
    out.truncate(5_000);
    Ok(json!({ "total": out.len(), "clashes": out }))
}

/// The convex hull of points (a ceiling's outline from its mesh), counterclockwise.
fn hull(pts: &[Vec2]) -> Vec<Vec2> {
    let mut p: Vec<Vec2> = pts.iter().copied().filter(|q| q.is_finite()).collect();
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup_by(|a, b| a.dist(*b) < 1e-9);
    if p.len() < 3 {
        return p;
    }
    let mut h: Vec<Vec2> = Vec::new();
    for pass in 0..2 {
        let start = h.len();
        let it: Box<dyn Iterator<Item = &Vec2>> = if pass == 0 { Box::new(p.iter()) } else { Box::new(p.iter().rev()) };
        for q in it {
            while h.len() >= start + 2 {
                let (a, b) = (h[h.len() - 2], h[h.len() - 1]);
                if (b - a).cross(*q - a) <= 0.0 {
                    h.pop();
                } else {
                    break;
                }
            }
            h.push(*q);
        }
        h.pop();
    }
    h
}

/// The shortest distance between two plan segments.
fn seg_seg(a: Vec2, b: Vec2, c: Vec2, e: Vec2) -> f64 {
    let (r, s) = (b - a, e - c);
    let den = r.cross(s);
    if den.abs() > 1e-12 {
        let t = (c - a).cross(s) / den;
        let u = (c - a).cross(r) / den;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            return 0.0;
        }
    }
    seg_dist(a, c, e).min(seg_dist(b, c, e)).min(seg_dist(c, a, b)).min(seg_dist(e, a, b))
}

fn closest_mid(a: Vec2, b: Vec2, c: Vec2, e: Vec2) -> Vec2 {
    let (r, s) = (b - a, e - c);
    let den = r.cross(s);
    if den.abs() > 1e-12 {
        let t = ((c - a).cross(s) / den).clamp(0.0, 1.0);
        return a + r * t;
    }
    (a + b + c + e) * 0.25
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        assert_eq!(size_tag("24x12 BOT @ 10'-0\""), Some([24.0, 12.0]));
        assert_eq!(size_tag("10\"Ø"), Some([10.0, 0.0]));
        assert_eq!(size_tag("12\" DIA"), Some([12.0, 0.0]));
        assert_eq!(size_tag("2\" CW"), Some([2.0, 0.0]));
        assert_eq!(size_tag("3/4\" C"), Some([0.75, 0.0]));
        assert_eq!(size_tag("1 1/2\" C"), Some([1.5, 0.0]));
        assert_eq!(size_tag("BOT @ 10'-0\""), None);
        assert_eq!(height_tag("BOT @ 10'-0\"", 1.0), Some(10.0));
        assert_eq!(height_tag("CL @ 10'-6\"", 1.0), Some(10.0));
        assert_eq!(height_tag("TOP @ 11'-0\"", 1.0), Some(10.0));
        assert_eq!(height_tag("24x12", 1.0), None);
        assert!(draft_system("M-DUCT-SUPP").is_some_and(|s| s.kind == MepKind::Duct));
        assert!(draft_system("P-DOMW-CW").is_some_and(|s| s.kind == MepKind::Pipe));
        assert!(draft_system("E-COND").is_some_and(|s| s.kind == MepKind::Conduit));
        assert!(draft_system("A-WALL").is_none());
    }
}
