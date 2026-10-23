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
//!   through a column, or cutting into an object of an attached IFC model (`ifc.attach`).

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag, Space};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, DuctType, MepKind, MepRun, MepSystem, level_at};
use serde_json::{Value, json};
use std::collections::BTreeMap;
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
            .params("{tolerance_in?: inches (0), references?: bool (clash with attached IFC models; default true), ignore_classes?: [IFC class] (default Space, Zone, SpatialZone, Annotation, Grid, Site)} → {clashes: [{kind: below_ceiling|into_structure|crossing|column|reference, message, systems, at, by_in, handles, reference?: {ref, class, guid, name, level}}], total}")
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
    } else if up.starts_with("E-") && (has("TRAY") || has("CT")) {
        (MepKind::Tray, [12.0, 4.0], 10.0)
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
pub(crate) fn read(s: &Session, sp: &BuildingSpec, only: Option<&str>) -> Result<(Vec<MepSystem>, Vec<MepRun>)> {
    let plan = super::verify::plan_session(s)?;
    let d: &Drawing = plan.doc()?;
    let ents = plan_objects(d);
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut systems = sp.mep.systems.clone();
    if systems.is_empty() {
        let mut layers: Vec<String> = ents.iter().map(|e| e.common.layer.to_string()).collect();
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
                id: String::new(),
                encased: false,
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
        MepKind::Tray => "tray",
        MepKind::Cable => "cable",
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

/// What kind of piece of a run it is: a straight segment, or a fitting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FittingKind {
    Segment,
    Elbow,
    Drop,
    /// Where a run meets another's side, or three runs meet end to end.
    Tee,
    /// Four or more runs at one point.
    Cross,
    /// A change of size between two runs meeting end to end (rectangular on either side).
    Transition,
    /// A change of size between two round runs.
    Reducer,
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
    /// For elbows: the incoming and outgoing directions (unit vectors) at the turn; drops: the
    /// direction the duct comes in.
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

/// The depth of a section, inches: a rectangle's height, a round one's diameter.
fn depth_in(size: [f64; 2]) -> f64 {
    if size[1] > 0.0 { size[1] } else { size[0] }
}

/// A unit vector from `a` to `b`, if they differ.
fn unit(a: Vec2, b: Vec2) -> Option<Vec2> {
    let l = a.dist(b);
    (l > 1e-9).then(|| (b - a) * (1.0 / l))
}

/// Turning enough to be an elbow (about 5° or more).
fn turns(u: Vec2, v: Vec2) -> bool {
    u.cross(v).abs() > 0.09 || u.dot(v) < 0.0
}

/// The run as pieces: its segments, and at each corner a fitting the size of the section: an
/// elbow where it turns in plan, a drop where its height changes by 1/16" or more (a zero-length segment is a
/// vertical one: a riser between the heights around it). No elbow at `skip` (a tee is there).
/// Returns `(pieces, counts)`.
fn pieces(run: &MepRun, base_z: f64, upf: f64, skip: &[Vec2]) -> (Vec<Piece>, PieceCounts) {
    let h = depth_in(run.size_in) / 12.0 * upf;
    let pts: Vec<Vec2> = run.points.iter().take(MAX_SEGS).map(|p| Vec2::new(p[0], p[1])).collect();
    let nseg = pts.len().saturating_sub(1);
    let bottom = |i: usize| base_z + run.bottoms_ft.get(i).copied().unwrap_or(0.0) * upf;
    let at = |i: usize| pts.get(i).copied().unwrap_or(Vec2::ZERO);
    let dir = |i: usize| unit(at(i), at(i + 1));
    let skipped = |p: Vec2| skip.iter().any(|q| q.dist(p) <= 1e-6 * upf.max(1.0));
    let mut out = Vec::new();
    let mut counts = PieceCounts::default();
    for i in 0..nseg {
        if dir(i).is_some() {
            let z = bottom(i);
            out.push(Piece::segment(at(i), at(i + 1), z, z + h, run.size_in));
        }
    }
    let mut corner = |out: &mut Vec<Piece>, p: Vec2, zs: &[f64], din: Option<Vec2>, dout: Option<Vec2>| {
        let lo = zs.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = zs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let (u, v) = match (din, dout) {
            (Some(u), Some(v)) => (u, v),
            (Some(u), None) => (u, u),
            (None, Some(v)) => (v, v),
            (None, None) => (Vec2::X, Vec2::X),
        };
        // Under a sixteenth of an inch is the same height (the numbers' rounding).
        if hi - lo > upf / 192.0 {
            counts.drops += 1;
            out.push(Piece::fitting(p, lo, hi + h, FittingKind::Drop, run.size_in, u, v));
        } else if din.is_some() && dout.is_some() && turns(u, v) && !skipped(p) {
            counts.elbows += 1;
            out.push(Piece::fitting(p, lo, lo + h, FittingKind::Elbow, run.size_in, u, v));
        }
    };
    let mut i = 0;
    while i < nseg {
        if dir(i).is_none() {
            // Vertical: the zero-length segments i..j, between the segments around them.
            let mut j = i;
            while j < nseg && dir(j).is_none() {
                j += 1;
            }
            let mut zs: Vec<f64> = (i..j).map(bottom).collect();
            let before = i.checked_sub(1);
            let after = (j < nseg).then_some(j);
            zs.extend(before.map(bottom));
            zs.extend(after.map(bottom));
            corner(&mut out, at(i), &zs, before.and_then(dir), after.and_then(dir));
            i = j;
            continue;
        }
        if i + 1 < nseg && dir(i + 1).is_some() {
            corner(&mut out, at(i + 1), &[bottom(i), bottom(i + 1)], dir(i), dir(i + 1));
        }
        i += 1;
    }
    (out, counts)
}

#[derive(Clone, Copy, Debug, Default)]
struct PieceCounts {
    elbows: usize,
    drops: usize,
}

// ---- where runs meet ----

/// One end of a run where it joins others: its plan point, height span (sheet units), size and
/// the direction into the run (none for a vertical end).
#[derive(Clone, Copy, Debug)]
struct RunEnd {
    run: usize,
    at: Vec2,
    z0: f64,
    z1: f64,
    size: [f64; 2],
    into: Option<Vec2>,
}

/// A fitting where runs meet: a tee, cross, transition or reducer, with the ends it joins.
#[derive(Clone, Debug)]
struct Joint {
    kind: FittingKind,
    at: Vec2,
    /// The run it is drawn with (its system's layer and source): the main run of a tee.
    owner: usize,
    ends: Vec<RunEnd>,
}

/// A run's geometry for finding its joints: the run (its sizes and bottoms), its plan points and
/// its level's floor (sheet units).
struct RunGeo<'a> {
    run: &'a MepRun,
    pts: Vec<Vec2>,
    base_z: f64,
}

impl RunGeo<'_> {
    fn bottom(&self, i: usize, upf: f64) -> f64 {
        self.base_z + self.run.bottoms_ft.get(i).copied().unwrap_or(0.0) * upf
    }
    fn h(&self, upf: f64) -> f64 {
        depth_in(self.run.size_in) / 12.0 * upf
    }
    /// The run's two ends (one for a single point).
    fn ends(&self, idx: usize, upf: f64) -> Vec<RunEnd> {
        let n = self.pts.len();
        if n < 2 {
            return Vec::new();
        }
        let nseg = n - 1;
        let first_dir = (0..nseg).find_map(|i| unit(self.pts.get(i).copied()?, self.pts.get(i + 1).copied()?));
        let last_dir = (0..nseg).rev().find_map(|i| unit(self.pts.get(i + 1).copied()?, self.pts.get(i).copied()?));
        let h = self.h(upf);
        let (b0, b1) = (self.bottom(0, upf), self.bottom(nseg - 1, upf));
        let mut out = Vec::new();
        // Each end at its own segment's height; a vertical end (zero-length segments) has no
        // direction.
        if let Some(p) = self.pts.first() {
            out.push(RunEnd {
                run: idx,
                at: *p,
                z0: b0,
                z1: b0 + h,
                size: self.run.size_in,
                into: first_dir.filter(|_| self.pts.get(1).is_some_and(|q| unit(*p, *q).is_some())),
            });
        }
        // A vertical run that stays within its own height: one end, not two at the same place.
        let stub = first_dir.is_none() && overlaps(b0, b0 + h, b1, b1 + h, 0.0);
        if let Some(p) = self.pts.last().filter(|_| !stub) {
            let into = last_dir.filter(|_| self.pts.get(nseg - 1).is_some_and(|q| unit(*p, *q).is_some()));
            out.push(RunEnd { run: idx, at: *p, z0: b1, z1: b1 + h, size: self.run.size_in, into });
        }
        out
    }
}

/// Height spans overlapping (an inch of slack).
fn overlaps(a0: f64, a1: f64, b0: f64, b1: f64, upf: f64) -> bool {
    a0 <= b1 + upf / 12.0 && b0 <= a1 + upf / 12.0
}

fn seg_near(p: Vec2, a: Vec2, b: Vec2) -> (f64, Vec2) {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 1e-18 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let q = a + ab * t;
    (p.dist(q), q)
}

/// The joints of one system's runs on one level: a run ending on another's side (a tee; two
/// ending at the same point there, a cross), and runs ending at the same point: two of
/// different sizes a transition (a reducer when both are round), two at an angle an elbow, three
/// a tee, four or more a cross. Ends must also meet in height. Also, per run, the points where a
/// tee sits on it (no elbow is drawn there).
fn joints(geo: &[RunGeo], upf: f64) -> (Vec<Joint>, Vec<Vec<Vec2>>) {
    let inch = upf / 12.0;
    let half = |s: [f64; 2]| s[0] / 2.0 * inch;
    let ends: Vec<RunEnd> = geo.iter().enumerate().flat_map(|(i, g)| g.ends(i, upf)).collect();
    // Segments in a grid of 10' cells, so ends look only near them.
    let cell = (10.0 * upf).max(1e-6);
    let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut grid: std::collections::HashMap<(i64, i64), Vec<(usize, usize)>> = std::collections::HashMap::new();
    for (ri, g) in geo.iter().enumerate() {
        for (si, w) in g.pts.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            if a.dist(b) <= 1e-9 {
                continue;
            }
            let pad = half(g.run.size_in) + inch;
            let (k0, k1) = (key(Vec2::new(a.x.min(b.x) - pad, a.y.min(b.y) - pad)), key(Vec2::new(a.x.max(b.x) + pad, a.y.max(b.y) + pad)));
            let cells = (k1.0 - k0.0 + 1).saturating_mul(k1.1 - k0.1 + 1);
            if !(1..=4096).contains(&cells) {
                continue;
            }
            for x in k0.0..=k1.0 {
                for y in k0.1..=k1.1 {
                    grid.entry((x, y)).or_default().push((ri, si));
                }
            }
        }
    }
    let mut used = vec![false; ends.len()];
    let mut joints: Vec<Joint> = Vec::new();
    let mut skip: Vec<Vec<Vec2>> = vec![Vec::new(); geo.len()];
    // On another run's side.
    let mut side: Vec<(usize, Vec2, Vec<usize>)> = Vec::new();
    for (ei, e) in ends.iter().enumerate() {
        let own = half(e.size) + inch;
        let mut best: Option<(f64, usize, Vec2)> = None;
        for &(ri, si) in grid.get(&key(e.at)).map(Vec::as_slice).unwrap_or_default() {
            let Some(g) = geo.get(ri).filter(|_| ri != e.run) else { continue };
            let (Some(a), Some(b)) = (g.pts.get(si), g.pts.get(si + 1)) else { continue };
            let (dist, q) = seg_near(e.at, *a, *b);
            let z = g.bottom(si, upf);
            if dist > half(g.run.size_in) + inch || !overlaps(e.z0, e.z1, z, z + g.h(upf), upf) {
                continue;
            }
            // At (or beyond) the other's end, the runs meet end to end, not on its side.
            let at_end = [g.pts.first(), g.pts.last()].into_iter().flatten().any(|p| p.dist(e.at) <= own || p.dist(q) <= own);
            if at_end {
                continue;
            }
            if best.is_none_or(|(d, _, _)| dist < d) {
                best = Some((dist, ri, q));
            }
        }
        let Some((_, main, q)) = best else { continue };
        if let Some(u) = used.get_mut(ei) {
            *u = true;
        }
        match side.iter_mut().find(|(m, p, _)| *m == main && p.dist(q) <= own) {
            Some((_, _, list)) => list.push(ei),
            None => side.push((main, q, vec![ei])),
        }
    }
    for (main, q, list) in side {
        let Some(g) = geo.get(main) else { continue };
        // The main run on both sides of the point.
        let Some(si) = g.pts.windows(2).position(|w| w[0].dist(w[1]) > 1e-9 && seg_near(q, w[0], w[1]).0 <= 1e-6 * upf.max(1.0)) else { continue };
        let (a, b) = (g.pts.get(si).copied().unwrap_or(q), g.pts.get(si + 1).copied().unwrap_or(q));
        let z = g.bottom(si, upf);
        let mut je: Vec<RunEnd> = Vec::new();
        for d in [unit(q, b).or_else(|| unit(a, b)), unit(q, a).or_else(|| unit(b, a))] {
            je.push(RunEnd { run: main, at: q, z0: z, z1: z + g.h(upf), size: g.run.size_in, into: d });
        }
        je.extend(list.iter().filter_map(|e| ends.get(*e)).map(|e| RunEnd { at: q, ..*e }));
        if let Some(s) = skip.get_mut(main) {
            s.push(q);
            // A tee on a corner of the main run: the vertex itself.
            if let Some(v) = g.pts.iter().find(|p| p.dist(q) <= half(g.run.size_in)) {
                s.push(*v);
            }
        }
        let kind = if list.len() >= 2 { FittingKind::Cross } else { FittingKind::Tee };
        joints.push(Joint { kind, at: q, owner: main, ends: je });
    }
    // End to end.
    for ei in 0..ends.len() {
        if used.get(ei).copied().unwrap_or(true) {
            continue;
        }
        let Some(e) = ends.get(ei).copied() else { continue };
        let mut at = vec![ei];
        for (ej, f) in ends.iter().enumerate().skip(ei + 1) {
            let tol = half(e.size).min(half(f.size)) + inch;
            if f.run == e.run || used.get(ej).copied().unwrap_or(true) || f.at.dist(e.at) > tol || !overlaps(e.z0, e.z1, f.z0, f.z1, upf) {
                continue;
            }
            // One end of each run: the one nearest in height (a riser's two ends are at one point).
            match at.iter_mut().find(|k| ends.get(**k).is_some_and(|g| g.run == f.run)) {
                Some(k) => {
                    if ends.get(*k).is_some_and(|g| (g.z0 - e.z0).abs() > (f.z0 - e.z0).abs()) {
                        *k = ej;
                    }
                }
                None => at.push(ej),
            }
        }
        if at.len() < 2 {
            continue;
        }
        for k in &at {
            if let Some(u) = used.get_mut(*k) {
                *u = true;
            }
        }
        let je: Vec<RunEnd> = at.iter().filter_map(|k| ends.get(*k)).copied().collect();
        let kind = match je.as_slice() {
            // A riser's end: the drop is its fitting.
            [a, b] if a.into.is_none() || b.into.is_none() => continue,
            [a, b] => {
                let turn = matches!((a.into, b.into), (Some(u), Some(v)) if turns(u, v * -1.0));
                if turn {
                    FittingKind::Elbow
                } else if (a.size[0] - b.size[0]).abs() > 1e-6 || (a.size[1] - b.size[1]).abs() > 1e-6 {
                    if a.size[1] > 0.0 || b.size[1] > 0.0 { FittingKind::Transition } else { FittingKind::Reducer }
                } else {
                    continue;
                }
            }
            [_, _, _] => FittingKind::Tee,
            _ => FittingKind::Cross,
        };
        // Drawn with the largest run.
        let owner = je.iter().max_by(|a, b| (a.size[0] * a.size[1].max(1.0)).total_cmp(&(b.size[0] * b.size[1].max(1.0)))).map_or(e.run, |x| x.run);
        joints.push(Joint { kind, at: e.at, owner, ends: je });
    }
    (joints, skip)
}

// ---- mesh generation for pieces ----

/// The number of subdivisions for round duct cross-sections.
const ROUND_SIDES: u32 = 12;

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

/// The centreline of an elbow at `corner` turning from `dir_in` to `dir_out`: an arc of radius
/// `radius` tangent to both segments, as `n + 1` points with their tangents. None when it
/// hardly turns or turns right back.
fn elbow_arc(corner: Vec2, dir_in: Vec2, dir_out: Vec2, radius: f64, n: u32) -> Option<Vec<(Vec2, Vec2)>> {
    let angle = dir_in.cross(dir_out).atan2(dir_in.dot(dir_out));
    if angle.abs() < 0.01 || angle.abs() > std::f64::consts::PI - 0.01 || radius <= 1e-9 {
        return None;
    }
    let sign = angle.signum();
    let t = radius * (angle.abs() / 2.0).tan();
    let start = corner - dir_in * t;
    let centre = start + dir_in.perp() * (sign * radius);
    Some(
        (0..=n)
            .map(|k| {
                let phi = angle * f64::from(k) / f64::from(n);
                (centre + (start - centre).rotate(phi), dir_in.rotate(phi))
            })
            .collect(),
    )
}

/// An elbow fitting: the bend at the corner, its centreline an arc of radius half the duct's
/// width joining the two segments. Rectangular: boxes as wide as the duct along the arc; round:
/// a bent tube. Turning right back: a post.
fn add_elbow(m: &mut Mesh, center: Vec2, w: f64, z0: f64, z1: f64, dir_in: Vec2, dir_out: Vec2, round: bool) {
    const N_ARC: u32 = 6;
    let half_w = w / 2.0;
    match elbow_arc(center, dir_in, dir_out, half_w, N_ARC) {
        Some(arc) if round => add_elbow_round(m, &arc, half_w, z0, z1),
        Some(arc) => {
            let last = arc.len().saturating_sub(2);
            for (k, w2) in arc.windows(2).enumerate() {
                let [(p0, t0), (p1, _)] = [w2[0], w2[1]];
                // Lengthened a little where boxes meet, so the outside of the bend has no notches.
                let Some(u) = unit(p0, p1) else { continue };
                let ext = half_w * (t0.cross(u).abs().asin()).tan();
                let (e0, e1) = (if k == 0 { 0.0 } else { ext }, if k == last { 0.0 } else { ext });
                super::wallgen::add_box(m, p0 - u * e0, p1 + u * e1, w, z0, z1);
            }
        }
        None if dir_in.dot(dir_out) < 0.0 => super::wallgen::add_box(m, center - dir_in * half_w, center + dir_in * half_w, w, z0, z1),
        None => {}
    }
}

/// A round elbow: rings across the arc's tangent, joined by quads.
fn add_elbow_round(m: &mut Mesh, arc: &[(Vec2, Vec2)], radius: f64, z0: f64, z1: f64) {
    let zc = (z0 + z1) / 2.0;
    let sides = ROUND_SIDES;
    let Ok(rings) = u32::try_from(arc.len()) else { return };
    if rings < 2 {
        return;
    }
    let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
    for (c, tangent) in arc {
        let n = tangent.perp();
        for k in 0..sides {
            let theta = f64::from(k) * std::f64::consts::TAU / f64::from(sides);
            let q = *c + n * (radius * theta.cos());
            m.vertices.push(Vec3::new(q.x, q.y, zc + radius * theta.sin()));
        }
    }
    for r in 0..rings - 1 {
        for k in 0..sides {
            let j = (k + 1) % sides;
            let (r0, r1) = (base + r * sides, base + (r + 1) * sides);
            m.faces.push(MeshFace { v: vec![r0 + k, r0 + j, r1 + j, r1 + k], hidden: 0 });
        }
    }
    let last = base + (rings - 1) * sides;
    for k in 1..sides.saturating_sub(1) {
        m.faces.push(MeshFace { v: vec![base, base + k + 1, base + k], hidden: 0 });
        m.faces.push(MeshFace { v: vec![last, last + k, last + k + 1], hidden: 0 });
    }
}

/// A drop (riser) fitting: vertical from z0 to z1. Round: an upright tube of the duct's
/// diameter; rectangular: a box as wide as the duct across the way it comes in and as deep as
/// it along it.
fn add_drop_fitting(m: &mut Mesh, center: Vec2, w: f64, depth: f64, z0: f64, z1: f64, dir_in: Vec2, round: bool) {
    if round {
        add_upright_cylinder(m, center, w / 2.0, z0, z1);
    } else {
        let u = if dir_in.len() > 1e-9 { dir_in.normalized() } else { Vec2::X };
        super::wallgen::add_box(m, center - u * (depth / 2.0), center + u * (depth / 2.0), w, z0, z1);
    }
}

/// A short straight piece of a section, from `a` to `b` (a stub of a tee).
fn add_section(m: &mut Mesh, a: Vec2, b: Vec2, size_in: [f64; 2], z0: f64, upf: f64) {
    let w = size_in[0] / 12.0 * upf;
    if size_in[1] <= 0.0 {
        add_round_segment(m, a, b, w / 2.0, z0, z0 + w);
    } else {
        super::wallgen::add_box(m, a, b, w, z0, z0 + size_in[1] / 12.0 * upf);
    }
}

/// A loft from one section at `a` to another at `b` (a transition or reducer): rings of the
/// same count, round sections as polygons, rectangles as four corners repeated.
fn add_loft(m: &mut Mesh, a: Vec2, b: Vec2, (sa, za): ([f64; 2], f64), (sb, zb): ([f64; 2], f64), upf: f64) {
    let Some(u) = unit(a, b) else { return };
    let n = u.perp();
    let ring = |c: Vec2, s: [f64; 2], z0: f64| -> Vec<Vec3> {
        let w = s[0] / 12.0 * upf;
        if s[1] <= 0.0 {
            let r = w / 2.0;
            (0..ROUND_SIDES)
                .map(|k| {
                    let t = f64::from(k) * std::f64::consts::TAU / f64::from(ROUND_SIDES);
                    let q = c + n * (r * t.cos());
                    Vec3::new(q.x, q.y, z0 + r + r * t.sin())
                })
                .collect()
        } else {
            let (hw, h) = (w / 2.0, s[1] / 12.0 * upf);
            // Corners in the same turning order as the round ring, spread over its points.
            let corners = [(hw, h / 2.0), (-hw, h / 2.0), (-hw, -h / 2.0), (hw, -h / 2.0)];
            (0..ROUND_SIDES)
                .map(|k| {
                    let (x, y) = corners.get((k * 4 / ROUND_SIDES) as usize).copied().unwrap_or((hw, h / 2.0));
                    let q = c + n * x;
                    Vec3::new(q.x, q.y, z0 + h / 2.0 + y)
                })
                .collect()
        }
    };
    let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
    m.vertices.extend(ring(a, sa, za));
    m.vertices.extend(ring(b, sb, zb));
    let s = ROUND_SIDES;
    for k in 0..s {
        let j = (k + 1) % s;
        m.faces.push(MeshFace { v: vec![base + k, base + j, base + s + j, base + s + k], hidden: 0 });
    }
    for k in 1..s.saturating_sub(1) {
        m.faces.push(MeshFace { v: vec![base, base + k + 1, base + k], hidden: 0 });
        m.faces.push(MeshFace { v: vec![base + s, base + s + k, base + s + k + 1], hidden: 0 });
    }
}

/// A joint's mesh. A tee or cross: a stub of each run from the point, each as long as the widest
/// other run's half-width and 2" more (vertical ends are their risers' own). A transition or
/// reducer: a foot-long loft between the two sections centred on the point. An end-to-end elbow:
/// the bend between the two.
fn joint_mesh(m: &mut Mesh, j: &Joint, upf: f64) {
    let inch = upf / 12.0;
    match j.kind {
        FittingKind::Transition | FittingKind::Reducer => {
            let [a, b] = [j.ends.first(), j.ends.get(1)];
            let (Some(a), Some(b)) = (a, b) else { return };
            let half = upf / 2.0;
            let da = a.into.or_else(|| b.into.map(|v| v * -1.0)).unwrap_or(Vec2::X);
            let db = b.into.unwrap_or(da * -1.0);
            add_loft(m, j.at + da * half, j.at + db * half, (a.size, a.z0), (b.size, b.z0), upf);
        }
        FittingKind::Elbow => {
            let [a, b] = [j.ends.first(), j.ends.get(1)];
            let (Some(a), Some(b)) = (a, b) else { return };
            let (Some(u), Some(v)) = (a.into, b.into) else { return };
            add_elbow(m, j.at, a.size[0] * inch, a.z0, a.z1, u * -1.0, v, a.size[1] <= 0.0);
        }
        _ => {
            for e in &j.ends {
                let Some(d) = e.into else { continue };
                let reach = j.ends.iter().filter(|o| !std::ptr::eq(*o, e)).map(|o| o.size[0] / 2.0).fold(0.0_f64, f64::max) + 2.0;
                add_section(m, j.at, j.at + d * (reach * inch), e.size, e.z0, upf);
            }
        }
    }
}

/// Add a terminal or box as a simple shape, 6" tall: a box (rectangular) or an upright squat
/// cylinder (round).
fn add_terminal_mesh(m: &mut Mesh, center: Vec2, size_in: [f64; 2], z0: f64, upf: f64) {
    let w = size_in[0] / 12.0 * upf;
    let h = 6.0 / 12.0 * upf;
    if size_in[1] <= 0.0 {
        add_upright_cylinder(m, center, w / 2.0, z0, z0 + h);
    } else {
        let d = size_in[1] / 12.0 * upf;
        let half = Vec2::new(w / 2.0, 0.0);
        super::wallgen::add_box(m, center - half, center + half, d, z0, z0 + h);
    }
}

/// A sprinkler head at its deflector height `z0`: a 1" body and a 2" deflector disc, the body
/// above the deflector for pendent and sidewall heads (they hang from their pipe), below it for
/// upright heads (they stand on theirs).
fn add_sprinkler_mesh(m: &mut Mesh, center: Vec2, kind: &str, z0: f64, upf: f64) {
    let inch = upf / 12.0;
    let body = 2.5 * inch;
    let disc = 0.25 * inch;
    if kind == "sprinkler_upright" {
        add_upright_cylinder(m, center, 0.5 * inch, z0 - body, z0);
        add_upright_cylinder(m, center, 1.0 * inch, z0, z0 + disc);
    } else {
        add_upright_cylinder(m, center, 0.5 * inch, z0, z0 + body);
        add_upright_cylinder(m, center, 1.0 * inch, z0 - disc, z0);
    }
}

/// An upright 16-sided cylinder (a tank) from z0 to z1.
pub(crate) fn add_upright_cylinder(m: &mut Mesh, c: Vec2, r: f64, z0: f64, z1: f64) {
    if !(r > 1e-9 && z1 > z0) {
        return;
    }
    const N: u32 = 16;
    let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
    for z in [z0, z1] {
        for i in 0..N {
            let a = f64::from(i) * std::f64::consts::TAU / f64::from(N);
            m.vertices.push(Vec3::new(c.x + r * a.cos(), c.y + r * a.sin(), z));
        }
    }
    let f = |v: Vec<u32>| MeshFace { v: v.iter().map(|i| base + i).collect(), hidden: 0 };
    m.faces.push(f((0..N).rev().collect()));
    m.faces.push(f((N..2 * N).collect()));
    for i in 0..N {
        let j = (i + 1) % N;
        m.faces.push(f(vec![i, j, N + j, N + i]));
    }
}

/// A run's piece as a mesh: a box (rectangular) or a tube (round) segment, an elbow or a drop.
fn piece_mesh(m: &mut Mesh, p: &Piece, upf: f64) {
    let w = p.size_a[0] / 12.0 * upf;
    match p.kind {
        FittingKind::Segment if p.is_round() => add_round_segment(m, p.a, p.b, w / 2.0, p.z0, p.z1),
        FittingKind::Segment => super::wallgen::add_box(m, p.a, p.b, w, p.z0, p.z1),
        FittingKind::Elbow => add_elbow(m, p.a, w, p.z0, p.z1, p.dir_in, p.dir_out, p.is_round()),
        FittingKind::Drop => add_drop_fitting(m, p.a, w, depth_in(p.size_a) / 12.0 * upf, p.z0, p.z1, p.dir_in, p.is_round()),
        FittingKind::Tee | FittingKind::Cross | FittingKind::Transition | FittingKind::Reducer => {}
    }
}

/// Sanitise a name for use in a layer name (alphanumeric, dash, underscore only).
pub(crate) fn sanitise(name: &str, max: usize) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(max).collect()
}

/// Ensure the layer exists; create it with the given ACI colour if not.
pub(crate) fn ensure_layer(d: &mut Drawing, layer: &str, color: i16) {
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
        MepKind::Tray => "tray",
        MepKind::Cable => "cable",
    }
}

/// The xdata tag for a fitting's type (for IFC: IfcDuctFitting, etc).
fn fitting_kind_str(fk: FittingKind) -> &'static str {
    match fk {
        FittingKind::Segment => "segment",
        FittingKind::Elbow => "elbow",
        FittingKind::Drop => "drop",
        FittingKind::Tee => "tee",
        FittingKind::Cross => "cross",
        FittingKind::Transition => "transition",
        FittingKind::Reducer => "reducer",
    }
}

/// A size in inches as tagged: `24x12` rectangular, `10` round.
fn size_text(s: [f64; 2]) -> String {
    let n = |v: f64| {
        let t = format!("{v:.3}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    if s[1] > 0.0 { format!("{}x{}", n(s[0]), n(s[1])) } else { n(s[0]) }
}

/// What a run's generated objects are known by from one build to the next: the plan object it
/// was read from (else its revision id) and where it starts.
fn run_base(r: &MepRun) -> String {
    let src = r.objects.iter().find(|o| o.as_str() != "reviewed").map_or(r.id.as_str(), String::as_str);
    let at = r.points.first().map_or(String::new(), |p| format!("{:.2},{:.2}", p[0], p[1]));
    format!("{src}@{at}")
}

/// Build the layer name for a system on a level: `3D-MEP-<system>-<level>`.
pub(crate) fn mep_layer_name(system: &str, level: &str) -> String {
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
    let (mut total_tees, mut total_crosses, mut total_transitions, mut total_reducers) = (0usize, 0usize, 0usize, 0usize);
    let t = |code: i32, v: String| RawTag { code, value: v };

    // -- Runs: per system and level, first where they meet (tees, crosses, transitions,
    // reducers), then each run's segments and every fitting as an object of its own. --
    struct Shown<'a> {
        run: &'a MepRun,
        sy: &'a MepSystem,
        level: String,
        base_z: f64,
    }
    let shown: Vec<Shown> = sp
        .mep
        .runs
        .iter()
        .take(MAX_RUNS)
        .filter_map(|r| {
            let sy = sp.mep.systems.iter().find(|s| s.name == r.system).filter(|s| s.show)?;
            let level = sp.level(&r.level)?;
            Some(Shown { run: r, sy, level: level.name.clone(), base_z: level.elevation_ft * upf })
        })
        .collect();
    let mut groups: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (i, s) in shown.iter().enumerate() {
        groups.entry((s.sy.name.as_str(), s.level.as_str())).or_default().push(i);
    }
    let mut all_joints: Vec<Joint> = Vec::new();
    let mut skip_at: Vec<Vec<Vec2>> = vec![Vec::new(); shown.len()];
    for members in groups.values() {
        let geo: Vec<RunGeo> = members
            .iter()
            .filter_map(|i| shown.get(*i))
            .map(|s| RunGeo { run: s.run, pts: s.run.points.iter().take(MAX_SEGS).map(|p| Vec2::new(p[0], p[1])).collect(), base_z: s.base_z })
            .collect();
        let (js, sk) = joints(&geo, upf);
        let global = |k: usize| members.get(k).copied().unwrap_or(usize::MAX);
        for (k, pts) in sk.into_iter().enumerate() {
            if let Some(s) = skip_at.get_mut(global(k)) {
                *s = pts;
            }
        }
        all_joints.extend(js.into_iter().map(|mut j| {
            j.owner = global(j.owner);
            for e in &mut j.ends {
                e.run = global(e.run);
            }
            j
        }));
    }
    let source_of = |objects: &[String]| objects.first().and_then(|h| u64::from_str_radix(h, 16).ok()).map_or(Handle(0), Handle);
    let duct_type_str = |sy: &MepSystem| {
        sy.duct_type.map(|dt| match dt {
            DuctType::Supply => "supply",
            DuctType::Return => "return",
            DuctType::Exhaust => "exhaust",
            DuctType::OutsideAir => "outside_air",
            DuctType::Other => "other",
        })
    };
    // The tags of a generated MEP object: its level, source, system, kind, part, sizes and an
    // id of its own (`model.revise` keeps hand edits by it).
    let tags = |level: &str, source: Handle, sy: &MepSystem, part: String, sizes: &[[f64; 2]], id: String| -> Vec<RawTag> {
        let mut x = vec![
            t(1001, super::wallgen::APP.into()),
            t(1000, "mep".into()),
            t(1000, level.to_string()),
            t(1000, source.hex()),
            t(1000, format!("system:{}", sy.name)),
            t(1000, format!("kind:{}", kind_str(sy.kind))),
            t(1000, part),
        ];
        if let Some(dt) = duct_type_str(sy) {
            x.push(t(1000, format!("duct_type:{dt}")));
        }
        for (k, s) in sizes.iter().enumerate().take(2) {
            if s[0] > 0.0 && s[0].is_finite() {
                x.push(t(1000, format!("{}:{}", if k == 0 { "size" } else { "size2" }, size_text(*s))));
            }
        }
        x.push(t(1000, format!("mepid:{}", id.chars().take(200).collect::<String>())));
        x
    };
    for (i, s) in shown.iter().enumerate() {
        let (r, sy) = (s.run, s.sy);
        let layer = mep_layer_name(&sy.name, &s.level);
        ensure_layer(d, &layer, sy.aci_color());
        let (ps, counts) = pieces(r, s.base_z, upf, skip_at.get(i).map(Vec::as_slice).unwrap_or_default());
        let source = source_of(&r.objects);
        let base = run_base(r);
        let mut seg_mesh = super::wallgen::empty_mesh();
        for p in ps.iter().filter(|p| !p.is_fitting()) {
            piece_mesh(&mut seg_mesh, p, upf);
        }
        if !seg_mesh.faces.is_empty() {
            let x = tags(&s.level, source, sy, "part:segment".into(), &[r.size_in], format!("{base}/segment"));
            if d.add(&Space::Model, Common { layer: layer.as_str().into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(seg_mesh)).is_ok()
            {
                total_runs += 1;
            }
        }
        // Each fitting its own object, tagged with its kind.
        let mut nth: BTreeMap<&str, usize> = BTreeMap::new();
        for p in ps.iter().filter(|p| p.is_fitting()) {
            let mut m = super::wallgen::empty_mesh();
            piece_mesh(&mut m, p, upf);
            if m.faces.is_empty() {
                continue;
            }
            let ks = fitting_kind_str(p.kind);
            let n = nth.entry(ks).or_default();
            *n += 1;
            let x = tags(&s.level, source, sy, format!("part:fitting:{ks}"), &[p.size_a], format!("{base}/{ks}{n}"));
            if d.add(&Space::Model, Common { layer: layer.as_str().into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
                total_fittings += 1;
            }
        }
        total_elbows += counts.elbows;
        total_drops += counts.drops;
    }
    // Where runs meet.
    for j in all_joints.iter().take(MAX_SEGS) {
        let Some(s) = shown.get(j.owner) else { continue };
        let mut m = super::wallgen::empty_mesh();
        joint_mesh(&mut m, j, upf);
        if m.faces.is_empty() {
            continue;
        }
        let layer = mep_layer_name(&s.sy.name, &s.level);
        ensure_layer(d, &layer, s.sy.aci_color());
        let ks = fitting_kind_str(j.kind);
        // The main size, then the smallest other one (a tee's branch, a transition's other end).
        let main = j.ends.iter().find(|e| e.run == j.owner).map_or(s.run.size_in, |e| e.size);
        let other = j
            .ends
            .iter()
            .filter(|e| e.size != main)
            .min_by(|a, b| (a.size[0] * a.size[1].max(1.0)).total_cmp(&(b.size[0] * b.size[1].max(1.0))))
            .map(|e| e.size);
        let sizes: Vec<[f64; 2]> = std::iter::once(main).chain(other).collect();
        let x = tags(&s.level, source_of(&s.run.objects), s.sy, format!("part:fitting:{ks}"), &sizes, format!("{ks}@{:.2},{:.2}", j.at.x, j.at.y));
        if d.add(&Space::Model, Common { layer: layer.into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            total_fittings += 1;
            match j.kind {
                FittingKind::Tee => total_tees += 1,
                FittingKind::Cross => total_crosses += 1,
                FittingKind::Transition => total_transitions += 1,
                FittingKind::Reducer => total_reducers += 1,
                FittingKind::Elbow => total_elbows += 1,
                _ => {}
            }
        }
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
        if term.terminal_kind.starts_with("sprinkler") {
            add_sprinkler_mesh(&mut m, center, &term.terminal_kind, z0, upf);
        } else {
            add_terminal_mesh(&mut m, center, term.size_in, z0, upf);
        }

        if !m.faces.is_empty() {
            let source = source_of(&term.objects);
            // A terminal read from no plan object is known by where it is.
            let id = format!("{}/terminal@{:.2},{:.2}", term.objects.first().map_or("", String::as_str), term.at[0], term.at[1]);
            let x = tags(&level.name, source, sy, format!("part:terminal:{}", term.terminal_kind), &[term.size_in], id);
            if d.add(&Space::Model, Common { layer: layer.into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
                total_terminals += 1;
            }
        }
    }

    // -- Equipment: a box (or an upright cylinder) per scheduled item, tagged with its kind,
    // tag, systems and schedule row for IFC. --
    let mut total_equipment = 0usize;
    // Equipment on a housekeeping pad stands on it.
    let pads = super::concrete::elements(sp, &d.bim_types);
    let on_pad = |e: &cadaddy_walk3d::MepEquipment| super::concrete::pad_top(&pads, &e.tag).map_or(e.bottom_ft, |t| e.bottom_ft.max(t));
    for e in sp.mep.equipment.iter().take(MAX_RUNS) {
        let Some(level) = sp.level(&e.level) else { continue };
        let ok = e.min.iter().chain(&e.max).chain([&e.bottom_ft, &e.height_ft]).all(|v| v.is_finite());
        if !ok || e.height_ft <= 0.0 || e.height_ft > 100.0 {
            continue;
        }
        let layer = format!("3D-MEP-EQUIP-{}", sanitise(&level.name, 16));
        ensure_layer(d, &layer, 30);
        let (lo, hi) = (Vec2::new(e.min[0].min(e.max[0]), e.min[1].min(e.max[1])), Vec2::new(e.min[0].max(e.max[0]), e.min[1].max(e.max[1])));
        let z0 = (level.elevation_ft + on_pad(e)) * upf;
        let z1 = z0 + e.height_ft * upf;
        let mut m = super::wallgen::empty_mesh();
        if e.round {
            add_upright_cylinder(&mut m, (lo + hi) * 0.5, (hi.x - lo.x).min(hi.y - lo.y) / 2.0, z0, z1);
        } else {
            let cy = (lo.y + hi.y) / 2.0;
            super::wallgen::add_box(&mut m, Vec2::new(lo.x, cy), Vec2::new(hi.x, cy), hi.y - lo.y, z0, z1);
        }
        if m.faces.is_empty() {
            continue;
        }
        let cap = |v: String| -> String { v.chars().take(250).collect() };
        let mut xdata = vec![
            t(1001, super::wallgen::APP.into()),
            t(1000, "mep".into()),
            t(1000, level.name.clone()),
            t(1000, Handle(0).hex()),
            t(1000, "kind:equipment".into()),
            t(1000, cap(format!("part:equipment:{}", sanitise(&e.kind, 40)))),
            t(1000, cap(format!("tag:{}", e.tag))),
        ];
        for sy in e.systems.iter().take(32) {
            xdata.push(t(1000, cap(format!("system:{sy}"))));
        }
        if !e.title.is_empty() {
            xdata.push(t(1000, cap(format!("title:{}", e.title))));
        }
        for (k, v) in e.properties.iter().take(64) {
            xdata.push(t(1000, cap(format!("prop:{k}={v}"))));
        }
        for (k, v) in e.sources.iter().take(8) {
            xdata.push(t(1000, cap(format!("prop:SOURCE {k}={v}"))));
        }
        if d.add(&Space::Model, Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            total_equipment += 1;
        }
    }

    // -- Feeders: their raceways where they come in through each gear's entry plates (rows
    // across the gear from one corner; what doesn't fit runs past its edge, so a crowded plate
    // shows), up to the ceiling or down into the slab. --
    let rules = super::gear::Rules::of(sp);
    let mut total_stubs = 0usize;
    for f in sp.mep.feeders.iter().take(MAX_RUNS) {
        if !(f.conduit_in.is_finite() && f.conduit_in > 0.0) || f.conduits == 0 {
            continue;
        }
        for (tag, side) in
            [(&f.to, f.entry.as_str()), (&f.from, if f.from_entry.trim().is_empty() { f.entry.as_str() } else { f.from_entry.as_str() })]
        {
            let Some(g) = sp.mep.equipment.iter().find(|e| e.tag.eq_ignore_ascii_case(tag.trim())) else { continue };
            let Some(level) = sp.level(&g.level) else { continue };
            let top = side.trim().eq_ignore_ascii_case("top");
            let lo = Vec2::new(g.min[0].min(g.max[0]), g.min[1].min(g.max[1]));
            let hi = Vec2::new(g.min[0].max(g.max[0]), g.min[1].max(g.max[1]));
            let z_floor = level.elevation_ft * upf;
            let z_top = (level.elevation_ft + on_pad(g) + g.height_ft) * upf;
            let (z0, z1) = if top {
                (z_top, ((level.elevation_ft + level.height_ft.max(on_pad(g) + g.height_ft + 1.0)) * upf).max(z_top + upf))
            } else {
                (z_floor - upf, (level.elevation_ft + on_pad(g)) * upf)
            };
            if !(z0.is_finite() && z1.is_finite() && z1 > z0) {
                continue;
            }
            // Where this feeder's raceways start on the plate: after the ones placed before it.
            let before: u32 = sp
                .mep
                .feeders
                .iter()
                .take_while(|x| !std::ptr::eq(*x, f))
                .filter(|x| {
                    let here_to = x.to.trim().eq_ignore_ascii_case(tag.trim()) && x.entry.trim().eq_ignore_ascii_case(side.trim());
                    let xs = if x.from_entry.trim().is_empty() { &x.entry } else { &x.from_entry };
                    let here_from = x.from.trim().eq_ignore_ascii_case(tag.trim()) && xs.trim().eq_ignore_ascii_case(side.trim());
                    here_to || here_from
                })
                .fold(0u32, |a, x| a.saturating_add(x.conduits.min(400)));
            let pitch = (rules.od(f.conduit_in) + rules.spacing_in) / 12.0 * upf;
            let per_row = (((hi.x - lo.x) / pitch).floor() as u32).max(1);
            let layer = format!("3D-ELEC-RACEWAY-{}", sanitise(&level.name, 16));
            ensure_layer(d, &layer, 1);
            for k in 0..f.conduits.min(400) {
                let n = before.saturating_add(k);
                let c = Vec2::new(lo.x + pitch * (f64::from(n % per_row) + 0.5), lo.y + pitch * (f64::from(n / per_row) + 0.5));
                let mut m = super::wallgen::empty_mesh();
                add_upright_cylinder(&mut m, c, rules.od(f.conduit_in) / 24.0 * upf, z0, z1);
                if m.faces.is_empty() {
                    continue;
                }
                let xdata = vec![
                    t(1001, super::wallgen::APP.into()),
                    t(1000, "mep".into()),
                    t(1000, level.name.clone()),
                    t(1000, Handle(0).hex()),
                    t(1000, format!("system:{}", f.tag.chars().take(60).collect::<String>())),
                    t(1000, "kind:conduit".into()),
                    t(1000, "part:segment".into()),
                ];
                if d.add(&Space::Model, Common { layer: layer.clone().into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok()
                {
                    total_stubs += 1;
                }
            }
        }
    }

    // -- Grounding: conductors along their routes (drawn an inch thick to be seen), rods, the
    // concrete-encased electrode in its footing, rings and busbars. --
    let mut total_ground = 0usize;
    if let Some(g) = &sp.mep.grounding {
        let inch = upf / 12.0;
        let mut add_ground = |d: &mut Drawing, level: &str, tag: &str, part: &str, m: Mesh| {
            if m.faces.is_empty() {
                return;
            }
            let layer = "3D-GROUNDING".to_string();
            ensure_layer(d, &layer, 3);
            let xdata = vec![
                t(1001, super::wallgen::APP.into()),
                t(1000, "mep".into()),
                t(1000, level.to_string()),
                t(1000, Handle(0).hex()),
                t(1000, "system:grounding".into()),
                t(1000, "kind:grounding".into()),
                t(1000, format!("part:{}", sanitise(part, 40))),
                t(1000, format!("tag:{}", tag.chars().take(60).collect::<String>())),
            ];
            if d.add(&Space::Model, Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
                total_ground += 1;
            }
        };
        let conductor = |path: &[[f64; 3]]| -> Mesh {
            let mut m = super::wallgen::empty_mesh();
            for w in path.windows(2).take(1000) {
                let [a, b] = w else { continue };
                if !(a.iter().chain(b.iter()).all(|v| v.is_finite())) {
                    continue;
                }
                let (pa, pb) = (Vec2::new(a[0], a[1]), Vec2::new(b[0], b[1]));
                if pa.dist(pb) > 1e-9 {
                    super::wallgen::add_box(&mut m, pa, pb, inch, a[2].min(b[2]) * upf - inch / 2.0, a[2].max(b[2]) * upf + inch / 2.0);
                } else if (a[2] - b[2]).abs() > 1e-9 {
                    add_upright_cylinder(&mut m, pa, inch / 2.0, a[2].min(b[2]) * upf, a[2].max(b[2]) * upf);
                }
            }
            m
        };
        for e in g.electrodes.iter().take(MAX_RUNS) {
            add_ground(d, &e.level, &e.tag, "gec", conductor(&e.path));
            let at = Vec2::new(e.at[0], e.at[1]);
            let z = e.z_ft * upf;
            let mut m = super::wallgen::empty_mesh();
            match e.kind.to_ascii_lowercase().as_str() {
                "ground_rod" => {
                    for k in 0..e.count.clamp(1, 50) {
                        let c = at + Vec2::new(f64::from(k) * 6.0 * upf, 0.0);
                        let len = if e.rod_length_ft.is_finite() { e.rod_length_ft.clamp(1.0, 100.0) } else { 10.0 };
                        add_upright_cylinder(&mut m, c, inch * 0.3125, z - len * upf, z);
                    }
                }
                "concrete_encased" if e.length_ft.is_finite() && e.length_ft > 0.0 => {
                    let a = e.angle_deg.to_radians();
                    let end = at + Vec2::new(a.cos(), a.sin()) * (e.length_ft.min(1000.0) * upf);
                    super::wallgen::add_box(&mut m, at, end, inch, z - inch / 2.0, z + inch / 2.0);
                }
                "ground_ring" if e.ring_length_ft.is_finite() && e.ring_length_ft > 0.0 => {
                    let side = e.ring_length_ft.min(4000.0) / 4.0 * upf;
                    let c = [at, at + Vec2::new(side, 0.0), at + Vec2::new(side, side), at + Vec2::new(0.0, side)];
                    for k in 0..4 {
                        let (Some(p), Some(q)) = (c.get(k), c.get((k + 1) % 4)) else { continue };
                        super::wallgen::add_box(&mut m, *p, *q, inch, z - inch / 2.0, z + inch / 2.0);
                    }
                }
                _ => {}
            }
            add_ground(d, &e.level, &e.tag, &e.kind, m);
        }
        for b in g.bonds.iter().take(MAX_RUNS) {
            add_ground(d, &b.level, &b.tag, &format!("bond:{}", b.kind), conductor(&b.path));
        }
        for b in g.busbars.iter().take(MAX_RUNS) {
            let Some(level) = sp.level(&b.level) else { continue };
            let z = if b.z_ft != 0.0 { b.z_ft } else { level.elevation_ft + 7.0 } * upf;
            let at = Vec2::new(b.at[0], b.at[1]);
            let mut m = super::wallgen::empty_mesh();
            super::wallgen::add_box(&mut m, at - Vec2::new(6.0 * inch, 0.0), at + Vec2::new(6.0 * inch, 0.0), inch, z - 2.0 * inch, z + 2.0 * inch);
            add_ground(d, &b.level, &b.tag, "busbar", m);
        }
    }

    json!({
        "mepRuns": total_runs, "elbows": total_elbows, "drops": total_drops, "tees": total_tees, "crosses": total_crosses,
        "transitions": total_transitions, "reducers": total_reducers, "fittings": total_fittings, "terminals": total_terminals, "mepEquipment": total_equipment, "feederRaceways": total_stubs,
        "grounding": total_ground,
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
        let (ps, _) = pieces(r, level.elevation_ft * upf, upf, &[]);
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
    // Into the objects of attached IFC models (architecture, structure from other programs), in
    // building coordinates: each level's plan base taken off.
    if p.get("references").and_then(Value::as_bool).unwrap_or(true) && !super::ifcref::ref_names(d).is_empty() {
        let bases = cadaddy_walk3d::level_bases(d, &sp.to_rules());
        let ignore: Option<Vec<String>> =
            p.get("ignore_classes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).take(64).map(str::to_string).collect());
        let mut pieces_ref = Vec::new();
        for (ri, r) in sp.mep.runs.iter().enumerate().take(MAX_RUNS) {
            let Some(level) = sp.level(&r.level) else { continue };
            let base = bases.iter().find(|(n, _)| n.eq_ignore_ascii_case(&level.name)).map_or(Vec2::ZERO, |(_, b)| *b);
            let base = if base.is_finite() { base } else { Vec2::ZERO };
            let (ps, _) = pieces(r, level.elevation_ft * upf, upf, &[]);
            let w = r.size_in[0] / 12.0 * upf;
            for pc in ps.into_iter().filter(|p| !p.is_fitting()) {
                if pieces_ref.len() >= MAX_SEGS {
                    break;
                }
                pieces_ref.push(super::ifcref::RunPiece {
                    system: &r.system,
                    run: ri,
                    a: pc.a - base,
                    b: pc.b - base,
                    z0: pc.z0,
                    z1: pc.z1,
                    w,
                    handles: handles(r),
                    base,
                });
            }
        }
        out.extend(super::ifcref::clashes(d, &pieces_ref, tol, upf, ignore.as_deref()));
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
        assert!(draft_system("E-CABL-TRAY").is_some_and(|s| s.kind == MepKind::Tray && s.size_in == [12.0, 4.0]));
        assert!(draft_system("A-WALL").is_none());
    }
}
