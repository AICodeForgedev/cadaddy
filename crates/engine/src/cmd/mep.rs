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
//!   turns and the drops, on a layer per system and level (`3D-MEP-<system>-<level>`); a system with `show: false`
//!   is left out.
//! - `model.clashes {tolerance_in?}`: runs hanging below their room's ceiling, reaching into the
//!   floor structure above (the next level's slab), crossing another system's run, or running
//!   through a column.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag, Space};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, DuctType, MepKind, MepRun, MepSystem, level_at};
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
        CommandSpec::new("mep.list", "List MEP Items", run_list)
            .params("{level?: name, system?: name, guessed_only?: bool} → {items: [{id, system, level, kind, segments, size_in, bottoms_ft, objects, confidence, size_source, height_source, reviewed}], systems: [...]}")
            .noundo(),
        CommandSpec::new("mep.edit", "Edit MEP Item", run_edit)
            .params("{id: index, system?: name, size_in?: [w,h], bottom_ft?: feet, reviewed?: bool, rebuild?: bool} → {item, rebuilt?}"),
    ]
}

/// Systems from standard layer names, for drawings whose spec names none.
fn draft_system(layer: &str) -> Option<MepSystem> {
    let up = layer.to_ascii_uppercase();
    let parts: Vec<&str> = up.split(['-', '_', ' ']).collect();
    let has = |w: &str| parts.contains(&w);
    let (kind, size, bottom) = if up.starts_with("M-") && (has("DUCT") || has("SUPP") || has("RETN") || has("EXHS") || has("HVAC") || has("OA")) {
        (MepKind::Duct, [12.0, 12.0], 10.0)
    } else if up.starts_with("P-") || (up.starts_with("M-") && (has("PIPE") || has("HWS") || has("CHWS") || has("CHWR") || has("STEM"))) {
        (MepKind::Pipe, [2.0, 0.0], 10.0)
    } else if up.starts_with("E-") && (has("COND") || has("CNDT")) {
        (MepKind::Conduit, [1.0, 0.0], 10.0)
    } else {
        return None;
    };
    // Infer duct type from layer name tokens.
    let duct_type = if kind == MepKind::Duct { Some(infer_duct_type(&parts)) } else { None };
    Some(MepSystem {
        name: layer.to_string(),
        layer: layer.to_string(),
        kind,
        size_in: size,
        bottom_ft: bottom,
        show: true,
        duct_type,
        color_index: 0,
    })
}

/// Infer the duct type from layer-name tokens (supply, return, exhaust, outside air).
fn infer_duct_type(parts: &[&str]) -> DuctType {
    if parts.iter().any(|p| *p == "SUPP" || *p == "SUPPLY" || *p == "SA") {
        DuctType::Supply
    } else if parts.iter().any(|p| *p == "RETN" || *p == "RETURN" || *p == "RA" || *p == "RTN") {
        DuctType::Return
    } else if parts.iter().any(|p| *p == "EXHS" || *p == "EXHAUST" || *p == "EA" || *p == "EXH") {
        DuctType::Exhaust
    } else if parts.iter().any(|p| *p == "OA" || *p == "OUTSIDE" || *p == "FRESH" || *p == "OAIR") {
        DuctType::OutsideAir
    } else {
        DuctType::Other
    }
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

// ---------------- list and edit for the review palette ----------------

/// An item in the MEP review list: one run, with the confidence and source of its size and
/// height, and whether it has been reviewed. `id` is its index in `sp.mep.runs`.
fn item_json(i: usize, r: &MepRun, sy: Option<&MepSystem>, sp: &BuildingSpec) -> Value {
    let segments = r.points.len().saturating_sub(1);
    // Confidence: "tag" (size or height came from a tag in the drawing) or "default" (the
    // system's defaults or the auto-drafted ones). A run whose size differs from the system's
    // had a tag; a run whose bottoms differ from the system's had a height tag.
    let size_from_tag = sy.is_some_and(|s| r.size_in != s.size_in);
    let height_from_tag = sy.is_some_and(|s| r.bottoms_ft.iter().any(|b| (*b - s.bottom_ft).abs() > 1e-6));
    let confidence = match (size_from_tag, height_from_tag) {
        (true, true) => "high",
        (true, false) | (false, true) => "medium",
        (false, false) => "low",
    };
    let size_source = if size_from_tag { "tag" } else { "default" };
    let height_source = if height_from_tag { "tag" } else { "default" };
    let kind = sy.map_or("unknown", |s| match s.kind {
        MepKind::Duct => "duct",
        MepKind::Pipe => "pipe",
        MepKind::Conduit => "conduit",
    });
    // `reviewed` is stored in xdata on the run's source objects if present, else in
    // a simple check against a list we keep in memory.  For now we use the building spec's
    // state: a run is reviewed if it has an `"reviewed": true` marker in its `objects` list
    // (the last entry being "reviewed" is the convention).
    let reviewed = r.objects.last().is_some_and(|s| s == "reviewed");
    let level_elev = sp.level(&r.level).map_or(0.0, |l| l.elevation_ft);
    json!({
        "id": i,
        "system": r.system,
        "level": r.level,
        "kind": kind,
        "segments": segments,
        "size_in": r.size_in,
        "bottoms_ft": r.bottoms_ft,
        "objects": r.objects.iter().filter(|s| s.as_str() != "reviewed").collect::<Vec<_>>(),
        "confidence": confidence,
        "size_source": size_source,
        "height_source": height_source,
        "reviewed": reviewed,
        "level_elevation_ft": level_elev,
    })
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "mep.list")?;
    let only_level = str_param(p, "level").map(str::to_string);
    let only_system = str_param(p, "system").map(str::to_string);
    let guessed_only = bool_or(p, "guessed_only", false);
    let mut items = Vec::new();
    for (i, r) in sp.mep.runs.iter().enumerate() {
        if only_level.as_deref().is_some_and(|l| !l.eq_ignore_ascii_case(&r.level)) {
            continue;
        }
        if only_system.as_deref().is_some_and(|s| !s.eq_ignore_ascii_case(&r.system)) {
            continue;
        }
        let sy = sp.mep.systems.iter().find(|s| s.name == r.system);
        if guessed_only {
            let size_from_tag = sy.is_some_and(|s| r.size_in != s.size_in);
            let height_from_tag = sy.is_some_and(|s| r.bottoms_ft.iter().any(|b| (*b - s.bottom_ft).abs() > 1e-6));
            if size_from_tag || height_from_tag {
                continue;
            }
        }
        items.push(item_json(i, r, sy, &sp));
    }
    let systems = serde_json::to_value(&sp.mep.systems).unwrap_or(Value::Null);
    Ok(json!({ "items": items, "systems": systems, "total": items.len() }))
}

fn run_edit(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "mep.edit")?;
    let id = p.get("id").and_then(Value::as_u64).ok_or_else(|| bad("mep.edit", "id is required"))? as usize;

    // Validate system name before mutating.
    let new_system = str_param(p, "system").map(str::to_string);
    if let Some(sys) = &new_system
        && !sp.mep.systems.iter().any(|s| s.name.eq_ignore_ascii_case(sys))
    {
        return Err(bad("mep.edit", format!("unknown system {sys}")));
    }

    // Apply mutations via get_mut (no panics).
    {
        let run = sp.mep.runs.get_mut(id).ok_or_else(|| bad("mep.edit", format!("no run at index {id}")))?;
        if let Some(sys) = &new_system {
            run.system = sys.clone();
        }
        if let Some(sz) = p.get("size_in").and_then(Value::as_array) {
            let w = sz.first().and_then(Value::as_f64).unwrap_or(0.0);
            let h = sz.get(1).and_then(Value::as_f64).unwrap_or(0.0);
            if w > 0.0 {
                run.size_in = [w, h];
            }
        }
        if let Some(b) = p.get("bottom_ft").and_then(Value::as_f64).filter(|v| v.is_finite()) {
            run.bottoms_ft.fill(b);
        }
        if let Some(reviewed) = p.get("reviewed").and_then(Value::as_bool) {
            run.objects.retain(|s| s.as_str() != "reviewed");
            if reviewed {
                run.objects.push("reviewed".to_string());
            }
        }
    }

    // Build the item JSON from the (now immutable) spec.
    let run = sp.mep.runs.get(id).ok_or_else(|| bad("mep.edit", format!("no run at index {id}")))?;
    let sy = sp.mep.systems.iter().find(|sy| sy.name == run.system);
    let item = item_json(id, run, sy, &sp);

    let do_rebuild = bool_or(p, "rebuild", false);
    s.doc_mut()?.building_spec = Some(sp.to_json());

    if do_rebuild {
        let _ = s.execute("model.build", &json!({}));
        Ok(json!({ "item": item, "rebuilt": true }))
    } else {
        Ok(json!({ "item": item }))
    }
}

// ---------------- in 3D ----------------

/// What kind of piece a segment of a run is (straight duct or a fitting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FittingKind {
    Segment,
    Elbow,
    Drop,
}

/// A run's pieces, sheet units: straight segments and fittings, each `(a, b, z0)` along the
/// plan with its bottom (risers: a == b, with `z1`).
#[derive(Clone, Copy, Debug)]
struct Piece {
    a: Vec2,
    b: Vec2,
    z0: f64,
    z1: f64,
    kind: FittingKind,
    /// The size, inches `[w, h]`.
    size_a: [f64; 2],
    /// For elbows: the incoming and outgoing directions (unit vectors) at the turn.
    dir_in: Vec2,
    dir_out: Vec2,
}

impl Piece {
    fn segment(a: Vec2, b: Vec2, z0: f64, z1: f64, size: [f64; 2]) -> Self {
        let d = if a.dist(b) > 1e-9 { (b - a) * (1.0 / a.dist(b)) } else { Vec2::new(1.0, 0.0) };
        Piece { a, b, z0, z1, kind: FittingKind::Segment, size_a: size, dir_in: d, dir_out: d }
    }
    fn fitting(center: Vec2, z0: f64, z1: f64, kind: FittingKind, size: [f64; 2], dir_in: Vec2, dir_out: Vec2) -> Self {
        Piece { a: center, b: center, z0, z1, kind, size_a: size, dir_in, dir_out }
    }
    fn is_fitting(&self) -> bool {
        self.kind != FittingKind::Segment
    }
    fn is_round(&self) -> bool {
        self.size_a[1] <= 0.0
    }
}

/// The run as pieces: its segments (and, at a drop, the riser between their heights; at each
/// turn or drop a fitting the size of the section). Returns `(pieces, counts)`.
fn pieces(run: &MepRun, base_z: f64, upf: f64) -> (Vec<Piece>, PieceCounts) {
    let h = if run.size_in[1] > 0.0 { run.size_in[1] } else { run.size_in[0] } / 12.0 * upf;
    let pts: Vec<Vec2> = run.points.iter().map(|p| Vec2::new(p[0], p[1])).collect();
    let mut out = Vec::new();
    let mut counts = PieceCounts::default();
    for (i, w2) in pts.windows(2).enumerate() {
        let [a, b] = [w2[0], w2[1]];
        let z = base_z + run.bottoms_ft.get(i).copied().unwrap_or(0.0) * upf;
        out.push(Piece::segment(a, b, z, z + h, run.size_in));
        // The corner to the next segment.
        if let Some(c) = pts.get(i + 2) {
            let (u, v) = (b - a, *c - b);
            let turn = u.len() > 1e-9 && v.len() > 1e-9 && (u.cross(v) / (u.len() * v.len())).abs() > 0.09;
            let z2 = base_z + run.bottoms_ft.get(i + 1).copied().unwrap_or(0.0) * upf;
            let dir_in = if u.len() > 1e-9 { u * (1.0 / u.len()) } else { Vec2::new(1.0, 0.0) };
            let dir_out = if v.len() > 1e-9 { v * (1.0 / v.len()) } else { Vec2::new(1.0, 0.0) };
            if (z2 - z).abs() > 1e-9 {
                counts.drops += 1;
                out.push(Piece::fitting(b, z.min(z2), z.max(z2) + h, FittingKind::Drop, run.size_in, dir_in, dir_out));
            } else if turn {
                counts.elbows += 1;
                out.push(Piece::fitting(b, z, z + h, FittingKind::Elbow, run.size_in, dir_in, dir_out));
            }
        }
    }
    (out, counts)
}

#[derive(Clone, Copy, Debug, Default)]
struct PieceCounts {
    elbows: usize,
    drops: usize,
}

// ---- mesh generation for pieces ----

/// The number of subdivisions for round duct cross-sections.
const ROUND_SIDES: u32 = 12;

/// Add a straight rectangular duct segment to the mesh. `w` is the section width in sheet
/// units (the perpendicular spread of the box); `z0→z1` span the section height.
fn add_rect_segment(m: &mut Mesh, a: Vec2, b: Vec2, w: f64, _h_in: f64, z0: f64, z1: f64, _upf: f64) {
    if a.dist(b) <= 1e-9 {
        return;
    }
    super::wallgen::add_box(m, a, b, w, z0, z1);
}

/// Add a straight round duct/pipe segment to the mesh (n-sided tube).
fn add_round_segment(m: &mut Mesh, a: Vec2, b: Vec2, radius: f64, z0: f64, z1: f64) {
    let len = a.dist(b);
    if len <= 1e-9 || radius <= 1e-9 {
        return;
    }
    let u = (b - a) * (1.0 / len);
    let n = Vec2::new(-u.y, u.x);
    let zc = (z0 + z1) / 2.0;
    let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
    let sides = ROUND_SIDES;
    for end in [a, b] {
        for k in 0..sides {
            let t = f64::from(k) * std::f64::consts::TAU / f64::from(sides);
            let q = end + n * (radius * t.cos());
            m.vertices.push(Vec3::new(q.x, q.y, zc + radius * t.sin()));
        }
    }
    for k in 0..sides {
        let j = (k + 1) % sides;
        m.faces.push(MeshFace { v: vec![base + k, base + j, base + sides + j, base + sides + k], hidden: 0 });
    }
    // End caps.
    for k in 1..sides.saturating_sub(1) {
        m.faces.push(MeshFace { v: vec![base, base + k + 1, base + k], hidden: 0 });
        m.faces.push(MeshFace { v: vec![base + sides, base + sides + k, base + sides + k + 1], hidden: 0 });
    }
}

/// Add an elbow fitting: a curved bend at the corner with `n_arc` subdivisions along the
/// turn radius (the duct's width, clamped to a reasonable range).
fn add_elbow(m: &mut Mesh, center: Vec2, w: f64, h_in: f64, z0: f64, z1: f64, upf: f64, dir_in: Vec2, dir_out: Vec2, round: bool) {
    let n_arc: u32 = 6;
    let half_w = w / 2.0;
    let half_t = if h_in > 0.0 { h_in / 12.0 * upf / 2.0 } else { half_w };
    // The turn radius: half-width for tight elbows, more natural geometry.
    let _radius = half_w;
    // Find the angle of the turn.
    let cross = dir_in.cross(dir_out);
    let dot = dir_in.dot(dir_out);
    let angle = cross.atan2(dot);
    if angle.abs() < 0.01 {
        return; // nearly straight, no elbow needed
    }
    // The centre of the arc is offset from the corner towards the inside.
    let bisector = dir_in + dir_out;
    let blen = bisector.len();
    if blen < 1e-9 {
        // 180 degree turn: just a post
        let half = Vec2::new(half_w, 0.0);
        super::wallgen::add_box(m, center - half, center + half, half_w * 2.0, z0, z1);
        return;
    }
    if round {
        add_elbow_round(m, center, half_w, z0, z1, dir_in, dir_out, angle, n_arc);
    } else {
        add_elbow_rect(m, center, half_w, half_t, z0, z1, dir_in, dir_out, angle, n_arc);
    }
}

/// A rectangular elbow: a sequence of short boxes along the arc.
fn add_elbow_rect(m: &mut Mesh, center: Vec2, half_w: f64, half_t: f64, z0: f64, z1: f64, dir_in: Vec2, _dir_out: Vec2, angle: f64, n_arc: u32) {
    let sign = if angle > 0.0 { 1.0 } else { -1.0 };
    let perp_in = Vec2::new(-dir_in.y, dir_in.x) * sign;
    let arc_center = center + perp_in * half_w;
    let start_angle = dir_in.y.atan2(dir_in.x);
    for k in 0..n_arc {
        let t0 = f64::from(k) / f64::from(n_arc);
        let t1 = f64::from(k + 1) / f64::from(n_arc);
        let a0 = start_angle + angle * t0;
        let a1 = start_angle + angle * t1;
        let d0 = Vec2::new(a0.cos(), a0.sin());
        let d1 = Vec2::new(a1.cos(), a1.sin());
        let p0 = arc_center - Vec2::new(-d0.y, d0.x) * sign * half_w;
        let p1 = arc_center - Vec2::new(-d1.y, d1.x) * sign * half_w;
        super::wallgen::add_box(m, p0, p1, half_t * 2.0, z0, z1);
    }
}

/// A round elbow: a torus section using ring cross-sections.
fn add_elbow_round(m: &mut Mesh, center: Vec2, radius: f64, z0: f64, z1: f64, dir_in: Vec2, _dir_out: Vec2, angle: f64, n_arc: u32) {
    let sign = if angle > 0.0 { 1.0 } else { -1.0 };
    let perp_in = Vec2::new(-dir_in.y, dir_in.x) * sign;
    let arc_center = center + perp_in * radius;
    let start_angle = dir_in.y.atan2(dir_in.x);
    let zc = (z0 + z1) / 2.0;
    let sides = ROUND_SIDES;
    let rings = n_arc + 1;
    let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);

    // Generate ring vertices along the arc.
    for r in 0..rings {
        let t = f64::from(r) / f64::from(n_arc);
        let a = start_angle + angle * t;
        let d = Vec2::new(a.cos(), a.sin());
        let ring_center = arc_center - Vec2::new(-d.y, d.x) * sign * radius;
        let n = Vec2::new(-d.y, d.x);
        for k in 0..sides {
            let theta = f64::from(k) * std::f64::consts::TAU / f64::from(sides);
            let q = ring_center + n * (radius * theta.cos());
            m.vertices.push(Vec3::new(q.x, q.y, zc + radius * theta.sin()));
        }
    }

    // Connect rings with quads.
    for r in 0..n_arc {
        for k in 0..sides {
            let j = (k + 1) % sides;
            let r0 = base + r * sides;
            let r1 = base + (r + 1) * sides;
            m.faces.push(MeshFace { v: vec![r0 + k, r0 + j, r1 + j, r1 + k], hidden: 0 });
        }
    }

    // End caps.
    for k in 1..sides.saturating_sub(1) {
        m.faces.push(MeshFace { v: vec![base, base + k + 1, base + k], hidden: 0 });
        let last_ring = base + n_arc * sides;
        m.faces.push(MeshFace { v: vec![last_ring, last_ring + k, last_ring + k + 1], hidden: 0 });
    }
}

/// Add a drop/riser fitting: a vertical transition between two heights.
fn add_drop_fitting(m: &mut Mesh, center: Vec2, w: f64, h_in: f64, z0: f64, z1: f64, upf: f64, round: bool) {
    if round {
        let r = w / 2.0;
        let half = Vec2::new(r, 0.0);
        add_round_segment(m, center - half * 0.01, center + half * 0.01, r, z0, z1);
    } else {
        let half = Vec2::new(w / 2.0, 0.0);
        let t = if h_in > 0.0 { h_in / 12.0 * upf } else { w };
        super::wallgen::add_box(m, center - half, center + half, t, z0, z1);
    }
}

/// Add a terminal or box as a simple shape: rectangular box or squat cylinder.
fn add_terminal_mesh(m: &mut Mesh, center: Vec2, size_in: [f64; 2], z0: f64, upf: f64) {
    let w = size_in[0] / 12.0 * upf;
    let round = size_in[1] <= 0.0;
    // Terminal height: 6 inches (a small box or disk).
    let h = 6.0 / 12.0 * upf;
    if round {
        add_round_segment(m, center - Vec2::new(0.01, 0.0), center + Vec2::new(0.01, 0.0), w / 2.0, z0, z0 + h);
    } else {
        let d = if size_in[1] > 0.0 { size_in[1] / 12.0 * upf } else { w };
        let half = Vec2::new(w / 2.0, 0.0);
        super::wallgen::add_box(m, center - half, center + half, d, z0, z0 + h);
    }
}

/// A piece as a mesh: a box (rectangular), an octagonal tube (round), or a fitting.
fn piece_mesh(m: &mut Mesh, p: &Piece, upf: f64) {
    let w = p.size_a[0] / 12.0 * upf;
    match p.kind {
        FittingKind::Segment => {
            if p.is_round() {
                add_round_segment(m, p.a, p.b, w / 2.0, p.z0, p.z1);
            } else {
                add_rect_segment(m, p.a, p.b, w, p.size_a[1], p.z0, p.z1, upf);
            }
        }
        FittingKind::Elbow => {
            add_elbow(m, p.a, w, p.size_a[1], p.z0, p.z1, upf, p.dir_in, p.dir_out, p.is_round());
        }
        FittingKind::Drop => {
            add_drop_fitting(m, p.a, w, p.size_a[1], p.z0, p.z1, upf, p.is_round());
        }
    }
}

/// Sanitise a name for use in a layer name (alphanumeric, dash, underscore only).
fn sanitise(name: &str, max: usize) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(max).collect()
}

/// Ensure the layer exists; create it with the given ACI colour if not.
fn ensure_layer(d: &mut Drawing, layer: &str, color: i16) {
    if d.layer(layer).is_none() {
        let ci = u8::try_from(color.clamp(1, 255)).unwrap_or(7);
        d.layers.push(cadaddy_doc::Layer { name: layer.to_string(), color: cadaddy_doc::color::Color::Index(ci), ..cadaddy_doc::Layer::default() });
    }
}

/// The xdata kind string for IFC export.
fn kind_str(kind: MepKind) -> &'static str {
    match kind {
        MepKind::Duct => "duct",
        MepKind::Pipe => "pipe",
        MepKind::Conduit => "conduit",
    }
}

/// The xdata tag for a fitting's type (for IFC: IfcDuctFitting, etc).
fn fitting_kind_str(fk: FittingKind) -> &'static str {
    match fk {
        FittingKind::Segment => "segment",
        FittingKind::Elbow => "elbow",
        FittingKind::Drop => "drop",
    }
}

/// Build the layer name for a system on a level: `3D-MEP-<system>-<level>`.
fn mep_layer_name(system: &str, level: &str) -> String {
    let sys = sanitise(system, 40);
    let lvl = sanitise(level, 16);
    format!("3D-MEP-{sys}-{lvl}")
}

/// The shown systems' runs into model space (sheet units), one mesh per run on its system's
/// layer per level. The counts.
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec) -> Value {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut total_runs = 0usize;
    let mut total_elbows = 0usize;
    let mut total_drops = 0usize;
    let mut total_fittings = 0usize;
    let mut total_terminals = 0usize;
    let t = |code: i32, v: String| RawTag { code, value: v };

    // -- Runs --
    for r in sp.mep.runs.iter().take(MAX_RUNS) {
        let Some(sy) = sp.mep.systems.iter().find(|s| s.name == r.system).filter(|s| s.show) else { continue };
        let Some(level) = sp.level(&r.level) else { continue };
        let layer = mep_layer_name(&sy.name, &level.name);
        ensure_layer(d, &layer, sy.aci_color());
        let (ps, counts) = pieces(r, level.elevation_ft * upf, upf);

        // Separate segment meshes and fitting meshes for distinct IFC mapping.
        let mut seg_mesh = super::wallgen::empty_mesh();
        let mut fit_mesh = super::wallgen::empty_mesh();
        let mut fit_kinds: Vec<FittingKind> = Vec::new();
        for p in &ps {
            if p.is_fitting() {
                piece_mesh(&mut fit_mesh, p, upf);
                fit_kinds.push(p.kind);
            } else {
                piece_mesh(&mut seg_mesh, p, upf);
            }
        }

        let source = r.objects.first().and_then(|h| u64::from_str_radix(h, 16).ok()).map_or(Handle(0), Handle);
        let duct_type_str = sy.duct_type.map(|dt| match dt {
            DuctType::Supply => "supply",
            DuctType::Return => "return",
            DuctType::Exhaust => "exhaust",
            DuctType::OutsideAir => "outside_air",
            DuctType::Other => "other",
        });

        // Add segment mesh.
        if !seg_mesh.faces.is_empty() {
            let mut xdata = vec![
                t(1001, super::wallgen::APP.into()),
                t(1000, "mep".into()),
                t(1000, level.name.clone()),
                t(1000, source.hex()),
                t(1000, format!("system:{}", sy.name)),
                t(1000, format!("kind:{}", kind_str(sy.kind))),
                t(1000, "part:segment".into()),
            ];
            if let Some(dt) = duct_type_str {
                xdata.push(t(1000, format!("duct_type:{dt}")));
            }
            if d.add(&Space::Model, Common { layer: layer.clone(), xdata, ..Common::default() }, EntityKind::Mesh(seg_mesh)).is_ok() {
                total_runs += 1;
            }
        }

        // Add fitting mesh (one combined mesh per run's fittings, tagged distinctly).
        if !fit_mesh.faces.is_empty() {
            // Use the most common fitting kind for the tag; elbows dominate.
            let dominant_fitting = if counts.elbows > 0 {
                FittingKind::Elbow
            } else if counts.drops > 0 {
                FittingKind::Drop
            } else {
                fit_kinds.first().copied().unwrap_or(FittingKind::Elbow)
            };
            let mut xdata = vec![
                t(1001, super::wallgen::APP.into()),
                t(1000, "mep".into()),
                t(1000, level.name.clone()),
                t(1000, source.hex()),
                t(1000, format!("system:{}", sy.name)),
                t(1000, format!("kind:{}", kind_str(sy.kind))),
                t(1000, format!("part:fitting:{}", fitting_kind_str(dominant_fitting))),
            ];
            if let Some(dt) = duct_type_str {
                xdata.push(t(1000, format!("duct_type:{dt}")));
            }
            if d.add(&Space::Model, Common { layer: layer.clone(), xdata, ..Common::default() }, EntityKind::Mesh(fit_mesh)).is_ok() {
                total_fittings += 1;
            }
        }

        total_elbows += counts.elbows;
        total_drops += counts.drops;
    }

    // -- Terminals --
    for term in sp.mep.terminals.iter().take(MAX_RUNS) {
        let Some(sy) = sp.mep.systems.iter().find(|s| s.name == term.system).filter(|s| s.show) else { continue };
        let Some(level) = sp.level(&term.level) else { continue };
        let layer = mep_layer_name(&sy.name, &level.name);
        ensure_layer(d, &layer, sy.aci_color());

        let center = Vec2::new(term.at[0], term.at[1]);
        let z0 = (level.elevation_ft + term.bottom_ft) * upf;
        let mut m = super::wallgen::empty_mesh();
        add_terminal_mesh(&mut m, center, term.size_in, z0, upf);

        if !m.faces.is_empty() {
            let source = term.objects.first().and_then(|h| u64::from_str_radix(h, 16).ok()).map_or(Handle(0), Handle);
            let xdata = vec![
                t(1001, super::wallgen::APP.into()),
                t(1000, "mep".into()),
                t(1000, level.name.clone()),
                t(1000, source.hex()),
                t(1000, format!("system:{}", sy.name)),
                t(1000, format!("kind:{}", kind_str(sy.kind))),
                t(1000, format!("part:terminal:{}", term.terminal_kind)),
            ];
            if d.add(&Space::Model, Common { layer, xdata, ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
                total_terminals += 1;
            }
        }
    }

    json!({
        "mepRuns": total_runs, "elbows": total_elbows, "drops": total_drops,
        "fittings": total_fittings, "terminals": total_terminals,
    })
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
        let (ps, _) = pieces(r, level.elevation_ft * upf, upf);
        let w = r.size_in[0] / 12.0 * upf;
        items.extend(ps.into_iter().filter(|p| !p.is_fitting()).map(|piece| Item { run: r, piece, w }));
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
