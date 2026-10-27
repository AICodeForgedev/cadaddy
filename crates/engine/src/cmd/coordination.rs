//! BIM coordination viewer commands: model tree, trade visibility, section box, viewpoints,
//! coordination views, colour schemes, isolate, 3D measure and BCF export.
//!
//! The coordination palette (UI) drives these commands to give the user a Navisworks-style
//! experience for reviewing MEP and structural coordination in 3D.
//!
//! Coordination-specific data (viewpoints, section box, colour schemes, coordination views) is
//! stored as extra JSON keys inside the drawing's `building_spec` text. The `BuildingSpec`
//! deserializer ignores unknown keys, so they survive round-trips through the spec parser.

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec3;
use cadaddy_walk3d::MepKind;
use serde_json::{Value, json};

use super::bspec::current;
use super::*;
use crate::{EngineError, Result, Session};

fn kind_str(k: MepKind) -> &'static str {
    match k {
        MepKind::Duct => "duct",
        MepKind::Pipe => "pipe",
        MepKind::Conduit => "conduit",
        MepKind::Tray => "tray",
        MepKind::Cable => "cable",
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("coordination.tree", "Coordination Tree", run_tree)
            .params("{level?, search?, detail?: bool} → {levels: [{name, elevation_ft, rooms, systems, elements}]}")
            .noundo(),
        CommandSpec::new("coordination.trades", "Trade Visibility", run_trades)
            .params("{set?: [{system, visible?}], level?} → {trades: [{system, kind, layer, visible, count}]}"),
        CommandSpec::new("coordination.sectionbox", "Section Box", run_sectionbox).params("{on?, min?, max?, fit?} → {on, min, max}"),
        CommandSpec::new("coordination.viewpoints", "Coordination Viewpoints", run_viewpoints)
            .params("{save?: {name}, restore?, delete?} → {viewpoints}")
            .noundo(),
        CommandSpec::new("coordination.measure3d", "3D Measure", run_measure3d)
            .params("{from: [x,y,z], to: [x,y,z]} → {distance, dx, dy, dz}")
            .noundo(),
        CommandSpec::new("coordination.clashlist", "Clash List", run_clashlist).params("{tolerance_in?, group_by?} → {groups, total}").noundo(),
        CommandSpec::new("coordination.clashstatus", "Clash Status", run_clashstatus).params("{id, status, note?}").noundo(),
        CommandSpec::new("coordination.clashgo", "Clash Navigate", run_clashgo)
            .params("{id} → {id, camera, handles}: frames the clash in 3D, isolates the two elements"),
        CommandSpec::new("coordination.inspect", "Inspect Element", run_inspect)
            .params("{handle} → {handle, type, category, system, level, trade, source, status, confidence, sheet, spec, validators}")
            .noundo(),
        CommandSpec::new("coordination.bcf", "BCF Export", run_bcf).params("{path, tolerance_in?} → {topics, path}").noundo(),
        // Mode, isolate, colour load and view restore change layers and visibility: each is one
        // undo step (a call that changes nothing leaves none).
        CommandSpec::new("coordination.mode", "Coordination Mode", run_mode)
            .params("{on?, ghost_opacity?} → {on, ghost_opacity}: off puts back each layer's own transparency"),
        CommandSpec::new("coordination.isolate", "Isolate Trade", run_isolate)
            .params("{system?, level?, reset?} → {isolated, hidden, shown}: hides the MEP objects of other systems / levels; reset shows them again"),
        CommandSpec::new("coordination.colors", "Colour Scheme", run_colors)
            .params("{save?: {name, colors}, load?, delete?, list?} → {schemes | active}"),
        CommandSpec::new("coordination.colorby", "Colour By Status/Level", run_colorby)
            .params("{by: status|level, off?} → {applied, scheme}: colours elements by source/status or level"),
        CommandSpec::new("coordination.views", "Coordination Views", run_views)
            .params("{save?: {name}, restore?, delete?, list?} → {views | restored, warnings}"),
    ]
}

// ─── Coordination data in building spec ───────────────────────────────────────

/// Read a coordination-specific key from the building spec JSON.
fn coord_get(d: &Drawing, key: &str) -> Value {
    d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|v| v.get(key).cloned()).unwrap_or(Value::Null)
}

/// Write a coordination-specific key into the building spec JSON (nothing when it already holds
/// `val`, so a call that changes nothing leaves no undo step).
fn coord_set(s: &mut Session, key: &str, val: &Value) -> Result<()> {
    let d = s.doc()?;
    let mut spec: Value = d.building_spec.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(json!({}));
    if spec.get(key) == Some(val) || (val.is_null() && spec.get(key).is_none()) {
        return Ok(());
    }
    if let Some(obj) = spec.as_object_mut() {
        obj.insert(key.into(), val.clone());
    }
    s.doc_mut()?.building_spec = Some(spec.to_string());
    Ok(())
}

fn r6(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

// ─── Model tree ───────────────────────────────────────────────────────────────

fn run_tree(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "coordination.tree")?;
    let d = s.doc()?;
    let filter_level = p.get("level").and_then(Value::as_str);
    let search = p.get("search").and_then(Value::as_str).map(str::to_ascii_lowercase);
    let detail = p.get("detail").and_then(Value::as_bool).unwrap_or(false);

    // Classify all model elements once.
    let els: Vec<_> = d
        .model
        .iter()
        .take(200_000)
        .filter(|e| d.is_visible(e))
        .filter_map(|e| {
            let (trade, category, system, level) = super::bimwork::classify(e)?;
            let tags: Vec<&str> = e.common.xdata.iter().filter(|t| t.code == 1000).map(|t| t.value.as_str()).collect();
            let room = tags.iter().find_map(|t| t.strip_prefix("room:")).unwrap_or_default().to_string();
            let type_mark = tags.iter().find_map(|t| t.strip_prefix("type:")).unwrap_or_default().to_string();
            let tag = tags.iter().find_map(|t| t.strip_prefix("tag:")).unwrap_or_default().to_string();
            Some((e.handle, trade, category, system, level, room, type_mark, tag))
        })
        .collect();

    let mut levels = Vec::new();
    for level in sp.levels.iter().take(500) {
        if let Some(f) = filter_level
            && !level.name.eq_ignore_ascii_case(f)
        {
            continue;
        }
        let level_els: Vec<_> = els.iter().filter(|e| e.4.eq_ignore_ascii_case(&level.name)).collect();

        // Rooms on this level.
        let rooms: Vec<Value> = sp
            .rooms
            .iter()
            .filter(|r| r.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(&level.name)))
            .take(1000)
            .map(|r| {
                let room_els: Vec<_> = level_els.iter().filter(|e| e.5.eq_ignore_ascii_case(&r.name)).collect();
                let mut room_val = json!({ "name": r.name, "ceiling_ft": r.ceiling_ft.unwrap_or(level.ceiling_ft), "elements": room_els.len() });
                if detail {
                    let handles: Vec<String> = room_els.iter().map(|e| e.0.hex()).collect();
                    room_val["handles"] = json!(handles);
                }
                room_val
            })
            .collect();

        // Systems on this level, with element handles.
        let mut sys_map: std::collections::BTreeMap<String, Vec<cadaddy_doc::Handle>> = std::collections::BTreeMap::new();
        for e in &level_els {
            sys_map.entry(e.3.clone()).or_default().push(e.0);
        }
        let systems: Vec<Value> = sys_map
            .iter()
            .map(|(name, handles)| {
                let kind = sp.mep.systems.iter().find(|sy| sy.name == *name).map_or("unknown", |sy| kind_str(sy.kind));
                let mut v = json!({ "name": name, "kind": kind, "count": handles.len() });
                if detail {
                    let hex: Vec<String> = handles.iter().take(5000).map(|h| h.hex()).collect();
                    v["handles"] = json!(hex);
                }
                v
            })
            .collect();

        let element_count = level_els.len();

        // Search filtering.
        if let Some(ref q) = search {
            let matches_level = level.name.to_ascii_lowercase().contains(q);
            let matching_rooms: Vec<_> =
                rooms.iter().filter(|r| r["name"].as_str().is_some_and(|n| n.to_ascii_lowercase().contains(q))).cloned().collect();
            let matching_sys = sys_map.keys().any(|name| name.to_ascii_lowercase().contains(q));
            let matching_els: Vec<_> = level_els
                .iter()
                .filter(|e| {
                    e.6.to_ascii_lowercase().contains(q)
                        || e.7.to_ascii_lowercase().contains(q)
                        || e.2.to_ascii_lowercase().contains(q)
                        || e.3.to_ascii_lowercase().contains(q)
                })
                .collect();
            if !matches_level && matching_rooms.is_empty() && matching_els.is_empty() && !matching_sys {
                continue;
            }
            let hits: Vec<Value> = matching_els
                .iter()
                .take(200)
                .map(|e| json!({ "handle": e.0.hex(), "category": e.2, "system": e.3, "type": e.6, "tag": e.7 }))
                .collect();
            levels.push(json!({
                "name": level.name, "elevation_ft": level.elevation_ft,
                "rooms": if matching_rooms.is_empty() { rooms.clone() } else { matching_rooms },
                "systems": systems, "elements": element_count,
                "search_hits": hits,
            }));
            continue;
        }

        let mut lv = json!({
            "name": level.name, "elevation_ft": level.elevation_ft,
            "height_ft": level.height_ft, "ceiling_ft": level.ceiling_ft,
            "rooms": rooms, "systems": systems, "elements": element_count,
        });
        if detail {
            let all_handles: Vec<String> = level_els.iter().map(|e| e.0.hex()).collect();
            lv["handles"] = json!(all_handles);
        }
        levels.push(lv);
    }

    let total_elements: usize = levels.iter().filter_map(|l| l["elements"].as_u64()).map(|n| n as usize).sum();
    Ok(json!({ "levels": levels, "total_elements": total_elements }))
}

// ─── Trade visibility ─────────────────────────────────────────────────────────

fn run_trades(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "coordination.trades")?;
    let level_filter = p.get("level").and_then(Value::as_str);

    if let Some(sets) = p.get("set").and_then(Value::as_array) {
        let d = s.doc_mut()?;
        for item in sets.iter().take(100) {
            let Some(system) = item.get("system").and_then(Value::as_str) else {
                continue;
            };
            if let Some(vis) = item.get("visible").and_then(Value::as_bool) {
                let names = trade_layers(&sp, system);
                for layer in d.layers.iter_mut().filter(|l| names.iter().any(|n| l.name.eq_ignore_ascii_case(n))) {
                    if layer.on != vis {
                        layer.on = vis;
                    }
                }
            }
        }
    }

    let d = s.doc()?;
    let mut trades = Vec::new();
    for sys in &sp.mep.systems {
        let layer_name = format!("3D-MEP-{}", sys.name);
        let names = trade_layers(&sp, &sys.name);
        let mut layers = d.layers.iter().filter(|l| names.iter().any(|n| l.name.eq_ignore_ascii_case(n))).peekable();
        let visible = layers.peek().is_none() || layers.any(|l| l.on);
        let count = sp.mep.runs.iter().filter(|r| r.system == sys.name && level_filter.is_none_or(|lf| r.level.eq_ignore_ascii_case(lf))).count();
        // Collect levels that have runs for this system.
        let mut levels: Vec<String> = Vec::new();
        for run in &sp.mep.runs {
            if run.system == sys.name && !levels.iter().any(|l| l.eq_ignore_ascii_case(&run.level)) {
                levels.push(run.level.clone());
                if levels.len() >= 100 {
                    break;
                }
            }
        }
        trades.push(json!({
            "system": sys.name,
            "kind": kind_str(sys.kind),
            "layer": layer_name,
            "visible": visible,
            "count": count,
            "levels": levels,
        }));
    }
    Ok(json!({ "trades": trades }))
}

/// The layers a trade (MEP system) draws on: one per level (`3D-MEP-<system>-<level>`, see
/// `mep::mep_layer_name`), and the plain `3D-MEP-<system>`.
fn trade_layers(sp: &cadaddy_walk3d::BuildingSpec, system: &str) -> Vec<String> {
    let mut v = vec![format!("3D-MEP-{system}")];
    v.extend(sp.levels.iter().take(500).map(|l| super::mep::mep_layer_name(system, &l.name)));
    v
}

// ─── Section box ──────────────────────────────────────────────────────────────

fn run_sectionbox(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let sbox = coord_get(d, "coord_sectionbox");
    let mut on = sbox.get("on").and_then(Value::as_bool).unwrap_or(false);
    let mut lo = arr3(&sbox, "min").unwrap_or([0.0; 3]);
    let mut hi = arr3(&sbox, "max").unwrap_or([0.0; 3]);

    if let Some(v) = p.get("on").and_then(Value::as_bool) {
        on = v;
    }
    if let Some(a) = arr3(p, "min") {
        lo = a;
    }
    if let Some(a) = arr3(p, "max") {
        hi = a;
    }
    if p.get("fit").and_then(Value::as_bool).unwrap_or(false) {
        let (blo, bhi) = model_bounds(d);
        let pad = 10.0;
        lo = [blo.x - pad, blo.y - pad, blo.z - pad];
        hi = [bhi.x + pad, bhi.y + pad, bhi.z + pad];
        on = true;
    }

    let state = json!({ "on": on, "min": lo, "max": hi });
    coord_set(s, "coord_sectionbox", &state)?;
    Ok(state)
}

fn arr3(v: &Value, key: &str) -> Option<[f64; 3]> {
    let a = v.get(key).and_then(Value::as_array).filter(|a| a.len() == 3)?;
    Some([
        a.first().and_then(Value::as_f64).filter(|f| f.is_finite())?,
        a.get(1).and_then(Value::as_f64).filter(|f| f.is_finite())?,
        a.get(2).and_then(Value::as_f64).filter(|f| f.is_finite())?,
    ])
}

fn model_bounds(d: &Drawing) -> (Vec3, Vec3) {
    let mut lo = Vec3::new(f64::MAX, f64::MAX, f64::MAX);
    let mut hi = Vec3::new(f64::MIN, f64::MIN, f64::MIN);
    for e in d.model.iter().take(500_000) {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        lo.x = lo.x.min(b.min.x);
        lo.y = lo.y.min(b.min.y);
        hi.x = hi.x.max(b.max.x);
        hi.y = hi.y.max(b.max.y);
        if let EntityKind::Mesh(m) = &e.kind {
            for v in &m.vertices {
                lo.z = lo.z.min(v.z);
                hi.z = hi.z.max(v.z);
            }
        }
    }
    if !lo.x.is_finite() {
        lo = Vec3::ZERO;
        hi = Vec3::new(100.0, 100.0, 20.0);
    }
    (lo, hi)
}

// ─── Viewpoints ───────────────────────────────────────────────────────────────

fn run_viewpoints(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let stored = coord_get(d, "coord_viewpoints");
    let mut vps: Vec<Value> = stored.as_array().cloned().unwrap_or_default();

    if let Some(save) = p.get("save") {
        let name = save.get("name").and_then(Value::as_str).unwrap_or("Unnamed");
        let st = s.state()?;
        let vf = st.zoom_frame();
        let vdir = st.view_dir;
        let entry = json!({
            "name": name,
            "center": [vf.center.x, vf.center.y],
            "height": vf.height,
            "view_dir": [vdir.x, vdir.y, vdir.z],
            "visual_style": st.visual_style.name(),
        });
        if let Some(pos) = vps.iter().position(|v| v.get("name").and_then(Value::as_str) == Some(name)) {
            if let Some(slot) = vps.get_mut(pos) {
                *slot = entry;
            }
        } else if vps.len() < 200 {
            vps.push(entry);
        }
    }

    if let Some(del) = p.get("delete").and_then(Value::as_str) {
        vps.retain(|v| v.get("name").and_then(Value::as_str) != Some(del));
    }

    if let Some(restore) = p.get("restore").and_then(Value::as_str)
        && let Some(vp) = vps.iter().find(|v| v.get("name").and_then(Value::as_str) == Some(restore))
    {
        let center = vp
            .get("center")
            .and_then(Value::as_array)
            .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)]);
        let height = vp.get("height").and_then(Value::as_f64);
        if let (Some(c), Some(h)) = (center, height) {
            s.execute("zoom", &json!({ "mode": "center", "center": c, "height": h }))?;
        }
        // Restore view direction if saved.
        if let Some(vd) = arr3(vp, "view_dir") {
            let _ = s.execute("vpoint", &json!({ "dir": vd }));
        }
        // Restore visual style if saved.
        if let Some(vs) = vp.get("visual_style").and_then(Value::as_str) {
            let _ = s.execute("vscurrent", &json!({ "style": vs }));
        }
    }

    coord_set(s, "coord_viewpoints", &json!(vps))?;
    Ok(json!({ "viewpoints": vps }))
}

// ─── 3D measure ───────────────────────────────────────────────────────────────

fn run_measure3d(_s: &mut Session, p: &Value) -> Result<Value> {
    let from = arr3(p, "from").ok_or_else(|| EngineError::Other("from must be [x,y,z]".into()))?;
    let to = arr3(p, "to").ok_or_else(|| EngineError::Other("to must be [x,y,z]".into()))?;
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let dz = to[2] - from[2];
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    let angle_xy = dy.atan2(dx).to_degrees();
    let horiz = (dx * dx + dy * dy).sqrt();
    let angle_z = dz.atan2(horiz).to_degrees();
    Ok(json!({
        "distance": r6(dist), "dx": r6(dx), "dy": r6(dy), "dz": r6(dz),
        "angle_xy": r6(angle_xy), "angle_z": r6(angle_z),
    }))
}

// ─── Clash list ───────────────────────────────────────────────────────────────

/// Stable ID for a clearance clash from `model.clashes`: deterministic from its content so
/// the same physical clash keeps the same ID across runs.  Uses the `C-` prefix so
/// `coordination.clashstatus` can persist its status in the same store as mesh clashes.
fn clearance_clash_id(c: &Value) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let kind = c.get("kind").and_then(Value::as_str).unwrap_or("");
    let level = c.get("level").and_then(Value::as_str).unwrap_or("");
    let mut systems: Vec<&str> = c.get("systems").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    systems.sort();
    let at = c
        .get("at")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).map(|v| ((v * 100.0).round() as i64).to_string()).collect::<Vec<_>>().join(","))
        .unwrap_or_default();
    let mut h = DefaultHasher::new();
    kind.hash(&mut h);
    level.hash(&mut h);
    systems.hash(&mut h);
    at.hash(&mut h);
    let hash = h.finish();
    format!("C-CL{:08x}{:08x}", hash as u32, (hash >> 32) as u32)
}

fn run_clashlist(s: &mut Session, p: &Value) -> Result<Value> {
    // Delegate to bim.clash which has stable IDs, status persistence and real grouping.
    let group_by = p.get("group_by").and_then(Value::as_str).unwrap_or("pair");
    let group_by_clash = match group_by {
        "trade" => "pair",
        "level" => "level",
        _ => group_by,
    };
    let raw = s.execute("bim.clash", &json!({ "tolerance": p.get("tolerance_in"), "group_by": group_by_clash }))?;
    // bim.clash returns stable IDs (C-{hex-a}-{hex-b}) and persisted status — pass them through.
    let mut clashes = raw.get("clashes").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut groups = raw.get("groups").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut total = raw.get("total").and_then(Value::as_u64).unwrap_or(0);

    // Merge clearance clashes from model.clashes (below_ceiling, into_structure, crossing,
    // column) that mesh-based bim.clash doesn't detect.  bim.clash is the superset: mesh +
    // clearance together in one list.
    let mep_p = json!({ "tolerance_in": p.get("tolerance_in"), "references": false, "same_trade": false });
    if let Ok(mep_raw) = super::mep::run_clashes(s, &mep_p)
        && let Some(mep_clashes) = mep_raw.get("clashes").and_then(Value::as_array)
    {
        let mut mep_ids = Vec::new();
        for c in mep_clashes {
            // Build a stable ID from the clash's content (kind + level + systems + position).
            let id = clearance_clash_id(c);
            let mut entry = c.clone();
            entry["id"] = json!(&id);
            entry["status"] = json!("new");
            clashes.push(entry);
            mep_ids.push(json!(&id));
        }
        let mep_count = mep_ids.len() as u64;
        total += mep_count;
        if mep_count > 0 {
            groups.push(json!({"key": "MEP clearance", "count": mep_count, "ids": mep_ids}));
        }
    }

    Ok(json!({ "clashes": clashes, "groups": groups, "total": total,
               "new": raw.get("new"), "active": raw.get("active"), "resolved": raw.get("resolved") }))
}

fn run_clashstatus(s: &mut Session, p: &Value) -> Result<Value> {
    let id = p.get("id").and_then(Value::as_str).ok_or_else(|| EngineError::Other("id required".into()))?;
    let status = p.get("status").and_then(Value::as_str).ok_or_else(|| EngineError::Other("status required".into()))?;
    if !["new", "active", "resolved", "approved"].contains(&status) {
        return Err(EngineError::Other("status must be new|active|resolved|approved".into()));
    }
    // Delegate to bim.clash.status which persists the status in session state.
    let mut clash_p = json!({ "id": id, "status": status });
    if let Some(note) = p.get("note") {
        clash_p["note"] = note.clone();
    }
    s.execute("bim.clash.status", &clash_p)?;
    // Also persist in building_spec so status survives save/open.
    let d = s.doc()?;
    let mut statuses: serde_json::Map<String, Value> = coord_get(d, "coord_clash_statuses").as_object().cloned().unwrap_or_default();
    statuses.insert(id.to_string(), json!({ "status": status, "note": p.get("note") }));
    coord_set(s, "coord_clash_statuses", &Value::Object(statuses))?;
    Ok(json!({ "id": id, "status": status }))
}

// ─── Clash navigation ────────────────────────────────────────────────────────

/// Frames the clash in 3D: sets the camera to the clash bbox, isolates/highlights the two
/// elements and ghosts the rest. MUST 3.
fn run_clashgo(s: &mut Session, p: &Value) -> Result<Value> {
    let id = p.get("id").and_then(Value::as_str).ok_or_else(|| EngineError::Other("coordination.clashgo: `id` required".into()))?;
    // Get the clash from bim.clash state.
    let d = s.doc()?;
    let st = super::bimwork::state(d);
    let runs = st["clash"]["runs"].as_object();
    let item = runs
        .and_then(|r| r.values().find_map(|run| run["items"].get(id).cloned()))
        .ok_or_else(|| EngineError::Other(format!("coordination.clashgo: no clash {id}: run coordination.clashlist first")))?;
    let ha = item["a"].as_str().and_then(cadaddy_doc::Handle::parse_hex);
    let hb = item["b"].as_str().and_then(cadaddy_doc::Handle::parse_hex);
    let (Some(ha), Some(hb)) = (ha, hb) else {
        return Err(EngineError::Other(format!("coordination.clashgo: elements of {id} are gone")));
    };
    // Compute bbox of the two elements.
    let els = super::bimwork::elements(d);
    let ea = els.iter().find(|e| e.handle == ha);
    let eb = els.iter().find(|e| e.handle == hb);
    let (lo, hi) = match (ea, eb) {
        (Some(a), Some(b)) => (
            Vec3::new(a.lo.x.min(b.lo.x), a.lo.y.min(b.lo.y), a.lo.z.min(b.lo.z)),
            Vec3::new(a.hi.x.max(b.hi.x), a.hi.y.max(b.hi.y), a.hi.z.max(b.hi.z)),
        ),
        _ => return Err(EngineError::Other(format!("coordination.clashgo: elements of {id} not found in model"))),
    };
    let center = (lo + hi) * 0.5;
    let size = (hi - lo).len().max(1.0) * 2.0;

    // Set 3D camera to frame the clash (southwest view looking at the center).
    let _ = s.execute("vpoint", &json!({ "dir": [-1, -1, 1] }));
    let _ = s.execute("zoom", &json!({ "mode": "center", "center": [center.x, center.y], "height": size }));

    // Ghost all other elements, keep only the two clash elements visible.
    let d = s.doc()?;
    let mut to_ghost = Vec::new();
    for e in d.model.iter().take(200_000) {
        if e.handle != ha && e.handle != hb && !d.is_hidden_object(e.handle) {
            to_ghost.push(e.handle);
        }
    }
    let record: Vec<Value> = to_ghost.iter().map(|h| json!(h.hex())).collect();
    super::selecttools::hide(s, to_ghost)?;
    coord_set(s, "coord_clashgo_hidden", &Value::Array(record))?;

    // Select the two elements.
    let _ = s.execute("select", &json!({ "handles": [ha.hex(), hb.hex()] }));

    Ok(json!({
        "id": id,
        "camera": { "target": [r6(center.x), r6(center.y), r6(center.z)], "size": r6(size) },
        "handles": [ha.hex(), hb.hex()],
    }))
}

// ─── Inspect element ─────────────────────────────────────────────────────────

/// Returns detailed info about an element: type, spec, model number, source, status, confidence,
/// source sheet, and validator/review items. MUST 4.
fn run_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    let handle_str = p.get("handle").and_then(Value::as_str).ok_or_else(|| EngineError::Other("coordination.inspect: `handle` required".into()))?;
    let handle =
        cadaddy_doc::Handle::parse_hex(handle_str).ok_or_else(|| EngineError::Other(format!("coordination.inspect: invalid handle {handle_str}")))?;
    let d = s.doc()?;
    let entity = d.entity(handle).ok_or_else(|| EngineError::Other(format!("coordination.inspect: no entity {handle_str}")))?;

    // Classify the element.
    let (trade, category, system, level) = super::bimwork::classify(entity).unwrap_or(("other", String::new(), String::new(), String::new()));

    // Extract tags from xdata.
    let tags: Vec<&str> = entity.common.xdata.iter().filter(|t| t.code == 1000).map(|t| t.value.as_str()).collect();

    // Source: where this element came from (PDF import, BIM build, IFC, etc.).
    let source = super::wallgen::made_as(entity).unwrap_or_default();

    // Source sheet: the sheet this element was read from (from xdata tag "sheet:...").
    let sheet = tags.iter().find_map(|t| t.strip_prefix("sheet:")).unwrap_or_default();

    // Status: from the element's validation/review xdata.
    let status = tags.iter().find_map(|t| t.strip_prefix("status:")).unwrap_or("unknown");

    // Confidence: from xdata.
    let confidence = tags.iter().find_map(|t| t.strip_prefix("confidence:")).unwrap_or("unknown");

    // Spec: type mark / model number from xdata.
    let type_mark = tags.iter().find_map(|t| t.strip_prefix("type:")).unwrap_or_default();
    let model_no = tags.iter().find_map(|t| t.strip_prefix("model:")).unwrap_or_default();

    // Validator/review items: any validation findings on this element.
    let validators: Vec<Value> = tags.iter().filter_map(|t| t.strip_prefix("validate:")).map(|v| json!({ "finding": v })).collect();

    Ok(json!({
        "handle": handle_str,
        "type": type_mark,
        "category": category,
        "system": system,
        "level": level,
        "trade": trade,
        "source": source,
        "status": status,
        "confidence": confidence,
        "sheet": sheet,
        "model_number": model_no,
        "spec": entity.common.layer.to_string(),
        "validators": validators,
    }))
}

// ─── BCF export ───────────────────────────────────────────────────────────────

fn run_bcf(s: &mut Session, p: &Value) -> Result<Value> {
    let path = p.get("path").and_then(Value::as_str).ok_or_else(|| EngineError::Other("path required".into()))?;
    let raw = s.execute("bim.clash", &json!({ "tolerance": p.get("tolerance_in") }))?;
    let clashes = raw.get("clashes").and_then(Value::as_array).cloned().unwrap_or_default();

    // Build IFC GUID lookup from the drawing's element index.
    let d = s.doc()?;
    let ifc_els = super::ifcmodel::elements(d);
    let ifc_guid = |h_hex: &str| -> String {
        cadaddy_doc::Handle::parse_hex(h_hex).and_then(|h| ifc_els.iter().find(|e| e.handle == h)).map(|e| e.tag.guid.clone()).unwrap_or_default()
    };

    // Read persisted statuses for each clash.
    let stored_statuses = coord_get(d, "coord_clash_statuses");

    let io = |e: std::io::Error| EngineError::Other(format!("BCF write: {e}"));
    let mut buf: Vec<u8> = Vec::new();
    {
        use std::io::Write;
        let mut zip = zip_mini::ZipWriter::new(&mut buf);

        zip.start_file("bcf.version").map_err(io)?;
        write!(
            zip,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <Version VersionId=\"2.1\"><DetailedVersion>2.1</DetailedVersion></Version>"
        )
        .map_err(io)?;

        let project_id = stable_uuid("cadaddy-coordination-project");
        zip.start_file("project.bcfp").map_err(io)?;
        write!(
            zip,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <ProjectExtension><Project ProjectId=\"{project_id}\">\
            <Name>CADaddy Coordination</Name></Project></ProjectExtension>"
        )
        .map_err(io)?;

        for clash in clashes.iter().take(1000) {
            let clash_id = clash.get("id").and_then(Value::as_str).unwrap_or_default();
            // Stable GUID: hash the clash id deterministically.
            let guid = stable_uuid(clash_id);
            let kind = clash.get("kind").and_then(Value::as_str).unwrap_or("unknown");
            let msg = escape_xml(clash.get("message").and_then(Value::as_str).unwrap_or(""));
            let title = escape_xml(&format!("{clash_id}: {kind}"));
            let date = iso_date();
            // Real status from persisted data or clash response.
            let bcf_status = stored_statuses
                .get(clash_id)
                .and_then(|s| s["status"].as_str())
                .or_else(|| clash.get("status").and_then(Value::as_str))
                .unwrap_or("New");
            let bcf_status = match bcf_status {
                "new" => "New",
                "active" => "Active",
                "resolved" => "Resolved",
                "approved" => "Closed",
                other => other,
            };

            // Markup with topic.
            zip.start_file(&format!("{guid}/markup.bcf")).map_err(io)?;
            write!(
                zip,
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                <Markup><Topic Guid=\"{guid}\" TopicType=\"Clash\" TopicStatus=\"{bcf_status}\">\
                <Title>{title}</Title><Description>{msg}</Description>\
                <CreationDate>{date}</CreationDate><CreationAuthor>CADaddy</CreationAuthor>\
                </Topic></Markup>"
            )
            .map_err(io)?;

            // Viewpoint with camera and components (IfcGuid).
            let vp = clash.get("viewpoint");
            let at = clash.get("at").and_then(Value::as_array);
            let ha_hex = clash.get("a").and_then(|a| a.get("handle")).and_then(Value::as_str).unwrap_or_default();
            let hb_hex = clash.get("b").and_then(|b| b.get("handle")).and_then(Value::as_str).unwrap_or_default();
            let guid_a = ifc_guid(ha_hex);
            let guid_b = ifc_guid(hb_hex);
            let vp_guid = stable_uuid(&format!("{clash_id}-vp"));

            zip.start_file(&format!("{guid}/viewpoint.bcfv")).map_err(io)?;
            write!(zip, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<VisualizationInfo Guid=\"{vp_guid}\">").map_err(io)?;
            // Camera.
            if let Some(at) = at {
                let x = at.first().and_then(Value::as_f64).unwrap_or(0.0);
                let y = at.get(1).and_then(Value::as_f64).unwrap_or(0.0);
                let z = at.get(2).and_then(Value::as_f64).unwrap_or(0.0);
                let eye = vp.and_then(|v| v.get("eye")).and_then(Value::as_array);
                let (ex, ey, ez) = eye.map_or((x - 10.0, y - 10.0, z + 10.0), |e| {
                    (
                        e.first().and_then(Value::as_f64).unwrap_or(x - 10.0),
                        e.get(1).and_then(Value::as_f64).unwrap_or(y - 10.0),
                        e.get(2).and_then(Value::as_f64).unwrap_or(z + 10.0),
                    )
                });
                write!(
                    zip,
                    "<PerspectiveCamera><CameraViewPoint>\
                    <X>{ex}</X><Y>{ey}</Y><Z>{ez}</Z></CameraViewPoint>\
                    <CameraDirection><X>{}</X><Y>{}</Y><Z>{}</Z></CameraDirection>\
                    <CameraUpVector><X>0</X><Y>0</Y><Z>1</Z></CameraUpVector>\
                    <FieldOfView>60</FieldOfView></PerspectiveCamera>",
                    x - ex,
                    y - ey,
                    z - ez
                )
                .map_err(io)?;
            }
            // Components.
            if !guid_a.is_empty() || !guid_b.is_empty() {
                write!(zip, "<Components><Selection>").map_err(io)?;
                if !guid_a.is_empty() {
                    write!(zip, "<Component IfcGuid=\"{guid_a}\"/>").map_err(io)?;
                }
                if !guid_b.is_empty() {
                    write!(zip, "<Component IfcGuid=\"{guid_b}\"/>").map_err(io)?;
                }
                write!(zip, "</Selection></Components>").map_err(io)?;
            }
            write!(zip, "</VisualizationInfo>").map_err(io)?;
        }
        zip.finish().map_err(io)?;
    }

    std::fs::write(path, &buf).map_err(|e| EngineError::Other(format!("BCF write: {e}")))?;
    Ok(json!({ "topics": clashes.len().min(1000), "path": path }))
}

/// Deterministic UUID from a string (for stable BCF topic GUIDs).
fn stable_uuid(input: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    input.hash(&mut h);
    let a = h.finish();
    h.write_u64(a);
    let b = h.finish();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
        a as u32,
        (a >> 32) as u16,
        (a >> 48) & 0xfff,
        8 | ((b >> 60) & 3),
        (b >> 48) & 0xfff,
        b & 0xffffffffffff
    )
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn iso_date() -> String {
    let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = now / 86400;
    let secs = now % 86400;
    let (mut y, mut m, mut day) = (1970u64, 1u64, 1u64);
    let mut rem = days;
    loop {
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let yd = if leap { 366 } else { 365 };
        if rem < yd {
            break;
        }
        rem -= yd;
        y += 1;
    }
    let md = [31, if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for d in md {
        if rem < d {
            day = rem + 1;
            break;
        }
        rem -= d;
        m += 1;
    }
    format!("{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}Z", secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// Minimal ZIP writer (store method, no compression).
mod zip_mini {
    use std::io::{self, Write};

    pub struct ZipWriter<'a> {
        buf: &'a mut Vec<u8>,
        entries: Vec<Entry>,
        data_start: usize,
    }

    struct Entry {
        name: String,
        offset: u32,
        size: u32,
        crc: u32,
    }

    impl<'a> ZipWriter<'a> {
        pub fn new(buf: &'a mut Vec<u8>) -> Self {
            ZipWriter { buf, entries: Vec::new(), data_start: 0 }
        }

        pub fn start_file(&mut self, name: &str) -> io::Result<()> {
            self.flush_pending();
            let offset = self.buf.len() as u32;
            let nb = name.as_bytes();
            self.buf.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            self.buf.extend_from_slice(&20u16.to_le_bytes());
            self.buf.extend_from_slice(&[0; 2]);
            self.buf.extend_from_slice(&[0; 2]);
            self.buf.extend_from_slice(&[0; 4]);
            self.buf.extend_from_slice(&[0; 4]); // crc placeholder
            self.buf.extend_from_slice(&[0; 4]); // size placeholder
            self.buf.extend_from_slice(&[0; 4]); // size placeholder
            self.buf.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            self.buf.extend_from_slice(&[0; 2]);
            self.buf.extend_from_slice(nb);
            self.data_start = self.buf.len();
            self.entries.push(Entry { name: name.to_string(), offset, size: 0, crc: 0 });
            Ok(())
        }

        fn flush_pending(&mut self) {
            if let Some(entry) = self.entries.last_mut()
                && entry.size == 0
                && self.data_start > 0
            {
                let data = &self.buf[self.data_start..];
                let size = data.len() as u32;
                let crc = crc32(data);
                entry.size = size;
                entry.crc = crc;
                let off = entry.offset as usize;
                self.buf[off + 14..off + 18].copy_from_slice(&crc.to_le_bytes());
                self.buf[off + 18..off + 22].copy_from_slice(&size.to_le_bytes());
                self.buf[off + 22..off + 26].copy_from_slice(&size.to_le_bytes());
            }
        }

        pub fn finish(&mut self) -> io::Result<()> {
            self.flush_pending();
            let cd_offset = self.buf.len() as u32;
            let mut cd_size = 0u32;
            for e in &self.entries {
                let nb = e.name.as_bytes();
                let start = self.buf.len();
                self.buf.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
                self.buf.extend_from_slice(&20u16.to_le_bytes());
                self.buf.extend_from_slice(&20u16.to_le_bytes());
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 4]);
                self.buf.extend_from_slice(&e.crc.to_le_bytes());
                self.buf.extend_from_slice(&e.size.to_le_bytes());
                self.buf.extend_from_slice(&e.size.to_le_bytes());
                self.buf.extend_from_slice(&(nb.len() as u16).to_le_bytes());
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 4]);
                self.buf.extend_from_slice(&e.offset.to_le_bytes());
                self.buf.extend_from_slice(nb);
                cd_size += (self.buf.len() - start) as u32;
            }
            self.buf.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
            self.buf.extend_from_slice(&[0; 4]);
            self.buf.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
            self.buf.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
            self.buf.extend_from_slice(&cd_size.to_le_bytes());
            self.buf.extend_from_slice(&cd_offset.to_le_bytes());
            self.buf.extend_from_slice(&[0; 2]);
            Ok(())
        }
    }

    impl Write for ZipWriter<'_> {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB88320 } else { crc >> 1 };
            }
        }
        !crc
    }
}

// ─── Coordination mode ───────────────────────────────────────────────────────

/// Enable or disable coordination visual mode. When on, architecture/structure layers are shown
/// ghosted (transparent) and MEP trade layers are solid in their system colours. Turning it on
/// keeps each layer's own transparency (`saved`); turning it off puts that back.
fn run_mode(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let stored = coord_get(d, "coord_mode");
    let was_on = stored.get("on").and_then(Value::as_bool).unwrap_or(false);
    let mut on = was_on;
    let mut ghost_opacity = stored.get("ghost_opacity").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(0.2);
    // The layers' own transparency, kept while the mode is on.
    let mut saved: serde_json::Map<String, Value> =
        if was_on { stored.get("saved").and_then(Value::as_object).cloned().unwrap_or_default() } else { serde_json::Map::new() };

    if let Some(v) = p.get("on").and_then(Value::as_bool) {
        on = v;
    }
    if let Some(v) = p.get("ghost_opacity").and_then(Value::as_f64).filter(|v| v.is_finite()) {
        ghost_opacity = v.clamp(0.0, 1.0);
    }

    if on {
        let opacity_pct = ((1.0 - ghost_opacity) * 90.0).round().clamp(0.0, 90.0) as u8;
        for layer in d.layers.iter().take(MAX_MODE_LAYERS) {
            if !saved.contains_key(&layer.name) {
                saved.insert(layer.name.clone(), json!(layer.transparency));
            }
        }
        let want = |name: &str| if name.starts_with("3D-MEP-") { 0 } else { opacity_pct };
        if d.layers.iter().take(MAX_MODE_LAYERS).any(|l| l.transparency != want(&l.name)) {
            let d = s.doc_mut()?;
            for layer in d.layers.iter_mut().take(MAX_MODE_LAYERS) {
                layer.transparency = want(&layer.name);
            }
        }
    } else if was_on {
        let own = |name: &str| saved.get(name).and_then(Value::as_u64).map(|t| t.min(90) as u8);
        if d.layers.iter().take(MAX_MODE_LAYERS).any(|l| own(&l.name).is_some_and(|t| t != l.transparency)) {
            let d = s.doc_mut()?;
            for layer in d.layers.iter_mut().take(MAX_MODE_LAYERS) {
                if let Some(t) = own(&layer.name) {
                    layer.transparency = t;
                }
            }
        }
        saved.clear();
    }

    let mut state = json!({ "on": on, "ghost_opacity": ghost_opacity });
    if on {
        state["saved"] = Value::Object(saved);
    }
    coord_set(s, "coord_mode", &state)?;
    Ok(json!({ "on": on, "ghost_opacity": ghost_opacity }))
}

/// Most layers coordination mode ghosts and puts back.
const MAX_MODE_LAYERS: usize = 10_000;

// ─── Isolate trade / level ───────────────────────────────────────────────────

/// Most MEP objects one isolate hides (and remembers, to show again).
const MAX_ISOLATED: usize = 200_000;

/// A generated MEP object's level and system (from its tags), `None` for anything else.
fn mep_level_system(e: &cadaddy_doc::Entity) -> Option<(String, Option<String>)> {
    if super::wallgen::made_as(e).as_deref() != Some("mep") {
        return None;
    }
    let level = super::bspec::made_level(e)?;
    let system = e.common.xdata.iter().filter(|t| t.code == 1000).find_map(|t| t.value.strip_prefix("system:").map(str::to_string));
    Some((level, system))
}

/// Show again the MEP objects the last isolate hid; how many.
fn unisolate(s: &mut Session) -> Result<usize> {
    let d = s.doc()?;
    let prev: Vec<cadaddy_doc::Handle> = coord_get(d, "coord_isolated")
        .as_array()
        .map(|a| {
            a.iter().take(MAX_ISOLATED).filter_map(Value::as_str).filter_map(|h| u64::from_str_radix(h, 16).ok()).map(cadaddy_doc::Handle).collect()
        })
        .unwrap_or_default();
    let back: Vec<cadaddy_doc::Handle> = prev.into_iter().filter(|h| d.is_hidden_object(*h)).collect();
    if !back.is_empty() {
        let d = s.doc_mut()?;
        for h in &back {
            d.hidden_objects.remove(h);
        }
    }
    coord_set(s, "coord_isolated", &Value::Null)?;
    Ok(back.len())
}

/// Show only the MEP objects of one system and/or one level (the others are hidden, as
/// HIDEOBJECTS does), or `reset` to show them again. A system or level the building doesn't
/// have is an error naming the ones it has.
fn run_isolate(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "coordination.isolate")?;

    if p.get("reset").and_then(Value::as_bool).unwrap_or(false) {
        let shown = unisolate(s)?;
        return Ok(json!({ "isolated": null, "hidden": 0, "shown": shown }));
    }

    let system = match p.get("system").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
        Some(want) => match sp.mep.systems.iter().find(|sy| sy.name.eq_ignore_ascii_case(want)) {
            Some(sy) => Some(sy.name.clone()),
            None => {
                let names: Vec<&str> = sp.mep.systems.iter().take(50).map(|sy| sy.name.as_str()).collect();
                return Err(EngineError::Other(format!("coordination.isolate: there is no system named {want} — systems: {names:?}")));
            }
        },
        None => None,
    };
    let level = match p.get("level").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
        Some(want) => match sp.level_matching(want).and_then(|i| sp.levels.get(i)) {
            Some(l) => Some(l.name.clone()),
            None => {
                let names: Vec<&str> = sp.levels.iter().take(50).map(|l| l.name.as_str()).collect();
                return Err(EngineError::Other(format!("coordination.isolate: there is no level named {want} — levels: {names:?}")));
            }
        },
        None => None,
    };
    if system.is_none() && level.is_none() {
        return Err(EngineError::Other("coordination.isolate: give a system and/or a level to isolate, or reset: true".into()));
    }

    // A new isolate replaces the last one.
    unisolate(s)?;
    let d = s.doc()?;
    let mut hide = Vec::new();
    let mut shown = 0usize;
    for e in d.model.iter().take(MAX_ISOLATED * 5) {
        let Some((lv, sy)) = mep_level_system(e) else { continue };
        let keep = level.as_deref().is_none_or(|l| lv.eq_ignore_ascii_case(l))
            && system.as_deref().is_none_or(|want| sy.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(want)));
        if keep {
            shown += 1;
        } else if hide.len() < MAX_ISOLATED && !d.is_hidden_object(e.handle) {
            hide.push(e.handle);
        }
    }
    let record: Vec<Value> = hide.iter().map(|h| json!(h.hex())).collect();
    let hidden = super::selecttools::hide(s, hide)?;
    coord_set(s, "coord_isolated", &Value::Array(record))?;
    Ok(json!({ "isolated": { "system": system, "level": level }, "hidden": hidden, "shown": shown }))
}

// ─── Colour schemes ──────────────────────────────────────────────────────────

/// Manage named colour schemes for coordination views. Each scheme maps system names to RGB
/// colours. Stored as `coord_color_schemes` in the building spec.
fn run_colors(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let stored = coord_get(d, "coord_color_schemes");
    let mut schemes: Vec<Value> = stored.as_array().cloned().unwrap_or_default();
    let names_of =
        |schemes: &[Value]| -> Vec<String> { schemes.iter().filter_map(|s| s.get("name").and_then(Value::as_str)).map(str::to_string).collect() };

    // List all schemes.
    if p.get("list").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(json!({ "schemes": names_of(&schemes) }));
    }

    // Save a scheme.
    if let Some(save) = p.get("save") {
        let name = save.get("name").and_then(Value::as_str).unwrap_or("Default");
        let colors = save.get("colors").cloned().unwrap_or(json!({}));
        let entry = json!({ "name": name, "colors": colors });
        if let Some(pos) = schemes.iter().position(|s| s.get("name").and_then(Value::as_str) == Some(name)) {
            if let Some(slot) = schemes.get_mut(pos) {
                *slot = entry;
            }
        } else if schemes.len() < 50 {
            schemes.push(entry);
        }
        coord_set(s, "coord_color_schemes", &json!(schemes))?;
        return Ok(json!({ "saved": name }));
    }

    // Load (apply) a scheme: set system colours on their layers (every level's).
    if let Some(load_name) = p.get("load").and_then(Value::as_str) {
        let scheme = schemes.iter().find(|s| s.get("name").and_then(Value::as_str) == Some(load_name));
        let Some(scheme) = scheme else {
            return Err(EngineError::Other(format!("colour scheme '{load_name}' not found — schemes: {:?}", names_of(&schemes))));
        };
        let colors = scheme.get("colors").cloned().unwrap_or(json!({}));
        let sp = current(s, "coordination.colors").ok().map(|(sp, _)| sp);
        let comp = |v: Option<&Value>| v.and_then(Value::as_f64).filter(|f| f.is_finite()).map_or(255, |f| f.round().clamp(0.0, 255.0) as u8);
        let d = s.doc_mut()?;
        if let Some(obj) = colors.as_object() {
            for (system, color_val) in obj.iter().take(100) {
                let names = sp.as_ref().map_or_else(|| vec![format!("3D-MEP-{system}")], |sp| trade_layers(sp, system));
                if let Some(arr) = color_val.as_array()
                    && arr.len() == 3
                {
                    let c = cadaddy_color::Color::True(cadaddy_color::Rgb(comp(arr.first()), comp(arr.get(1)), comp(arr.get(2))));
                    for layer in d.layers.iter_mut().filter(|l| names.iter().any(|n| l.name.eq_ignore_ascii_case(n))) {
                        layer.color = c;
                    }
                }
            }
        }
        coord_set(s, "coord_active_scheme", &json!(load_name))?;
        return Ok(json!({ "active": load_name, "colors": colors }));
    }

    // Delete a scheme.
    if let Some(del) = p.get("delete").and_then(Value::as_str) {
        schemes.retain(|s| s.get("name").and_then(Value::as_str) != Some(del));
        coord_set(s, "coord_color_schemes", &json!(schemes))?;
        return Ok(json!({ "deleted": del }));
    }

    // Default: return current state.
    let active = coord_get(s.doc()?, "coord_active_scheme");
    Ok(json!({ "schemes": schemes, "active": active }))
}

// ─── Colour by status / level ────────────────────────────────────────────────

/// Status colour scheme: maps element source/status to RGB.
const STATUS_COLORS: &[(&str, [u8; 3])] = &[
    ("fixed", [0, 128, 0]),            // green — fixed/verified placement
    ("rule", [0, 0, 200]),             // blue — placed by a rule
    ("zone", [128, 0, 255]),           // purple — zone-placed
    ("field-verified", [0, 200, 200]), // cyan — field-verified
    ("override", [255, 165, 0]),       // orange — manually overridden
    ("agent", [255, 255, 0]),          // yellow — placed by agent
    ("failing", [255, 0, 0]),          // red — failing validation
    ("unknown", [160, 160, 160]),      // grey — no status
];

/// Level colour scheme: deterministic colour per level index.
const LEVEL_COLORS: &[[u8; 3]] = &[
    [65, 105, 225], // royal blue
    [220, 20, 60],  // crimson
    [34, 139, 34],  // forest green
    [255, 140, 0],  // dark orange
    [148, 0, 211],  // dark violet
    [0, 206, 209],  // dark turquoise
    [199, 21, 133], // medium violet red
    [255, 215, 0],  // gold
];

/// Colour all model elements by their source/status or by level, as render-time colour overrides.
/// MUST 5.
fn run_colorby(s: &mut Session, p: &Value) -> Result<Value> {
    let by = p.get("by").and_then(Value::as_str).unwrap_or("status");
    if p.get("off").and_then(Value::as_bool).unwrap_or(false) {
        // Restore original colours from saved state.
        let d = s.doc()?;
        let saved = coord_get(d, "coord_colorby_saved");
        if let Some(entries) = saved.as_array() {
            let d = s.doc_mut()?;
            for entry in entries.iter().take(200_000) {
                if let (Some(h_hex), Some(r), Some(g), Some(b)) = (
                    entry.get("h").and_then(Value::as_str),
                    entry.get("r").and_then(Value::as_u64),
                    entry.get("g").and_then(Value::as_u64),
                    entry.get("b").and_then(Value::as_u64),
                ) && let Some(h) = cadaddy_doc::Handle::parse_hex(h_hex)
                {
                    let _ = d.modify_entity(h, |e| {
                        e.common.color = cadaddy_color::Color::True(cadaddy_color::Rgb(r as u8, g as u8, b as u8));
                    });
                }
            }
        }
        coord_set(s, "coord_colorby_saved", &Value::Null)?;
        coord_set(s, "coord_colorby_active", &Value::Null)?;
        return Ok(json!({ "applied": false, "by": by }));
    }

    let (sp, _) = current(s, "coordination.colorby")?;
    let d = s.doc()?;
    // Save original colours for later restore.
    let mut saved = Vec::new();
    let mut applied = std::collections::BTreeMap::new();

    for e in d.model.iter().take(200_000) {
        let (_trade, _category, _system, level) = super::bimwork::classify(e).unwrap_or(("other", String::new(), String::new(), String::new()));
        let tags: Vec<&str> = e.common.xdata.iter().filter(|t| t.code == 1000).map(|t| t.value.as_str()).collect();

        let color = match by {
            "status" => {
                let status = tags.iter().find_map(|t| t.strip_prefix("status:")).unwrap_or("unknown");
                STATUS_COLORS.iter().find(|(s, _)| *s == status).map(|(_, c)| *c).unwrap_or([160, 160, 160])
            }
            "level" => {
                let idx = sp.levels.iter().position(|l| l.name.eq_ignore_ascii_case(&level)).unwrap_or(0);
                LEVEL_COLORS[idx % LEVEL_COLORS.len()]
            }
            _ => return Err(EngineError::Other("coordination.colorby: `by` must be status or level".into())),
        };

        let orig = match e.common.color {
            cadaddy_color::Color::True(rgb) => (rgb.0, rgb.1, rgb.2),
            _ => (255, 255, 255),
        };
        saved.push(json!({ "h": e.handle.hex(), "r": orig.0, "g": orig.1, "b": orig.2 }));
        applied.insert(e.handle, color);
    }

    // Apply colours.
    let d = s.doc_mut()?;
    for (h, [r, g, b]) in &applied {
        let _ = d.modify_entity(*h, |e| {
            e.common.color = cadaddy_color::Color::True(cadaddy_color::Rgb(*r, *g, *b));
        });
    }
    coord_set(s, "coord_colorby_saved", &json!(saved))?;
    coord_set(s, "coord_colorby_active", &json!(by))?;
    Ok(json!({ "applied": true, "by": by, "elements": applied.len() }))
}

// ─── Coordination views (full state) ─────────────────────────────────────────

/// A coordination view saves the full viewing state: camera, section box, trade visibility,
/// colour scheme and visual style. Stored as `coord_views` in the building spec.
fn run_views(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let stored = coord_get(d, "coord_views");
    let mut views: Vec<Value> = stored.as_array().cloned().unwrap_or_default();

    // Save: capture full state.
    if let Some(save) = p.get("save") {
        let name = save.get("name").and_then(Value::as_str).unwrap_or("Unnamed");

        // Camera state.
        let st = s.state()?;
        let vf = st.zoom_frame();
        let vdir = st.view_dir;
        let vstyle = st.visual_style.name();

        // Section box state.
        let d = s.doc()?;
        let sbox = coord_get(d, "coord_sectionbox");
        // The mode without the layers' kept transparency (restoring puts back what is current).
        let mode = coord_get(d, "coord_mode");
        let mode = json!({
            "on": mode.get("on").and_then(Value::as_bool).unwrap_or(false),
            "ghost_opacity": mode.get("ghost_opacity").and_then(Value::as_f64).unwrap_or(0.2),
        });
        let active_scheme = coord_get(d, "coord_active_scheme");

        // Trade visibility snapshot.
        let trade_vis: Vec<Value> =
            d.layers.iter().filter(|l| l.name.starts_with("3D-MEP-")).take(200).map(|l| json!({ "layer": l.name, "on": l.on })).collect();

        let entry = json!({
            "name": name,
            "center": [vf.center.x, vf.center.y],
            "height": vf.height,
            "view_dir": [vdir.x, vdir.y, vdir.z],
            "visual_style": vstyle,
            "sectionbox": sbox,
            "coord_mode": mode,
            "color_scheme": active_scheme,
            "trades": trade_vis,
        });

        if let Some(pos) = views.iter().position(|v| v.get("name").and_then(Value::as_str) == Some(name)) {
            if let Some(slot) = views.get_mut(pos) {
                *slot = entry;
            }
        } else if views.len() < 200 {
            views.push(entry);
        }
        coord_set(s, "coord_views", &json!(views))?;
        return Ok(json!({ "saved": name, "views": views }));
    }

    // Delete.
    if let Some(del) = p.get("delete").and_then(Value::as_str) {
        views.retain(|v| v.get("name").and_then(Value::as_str) != Some(del));
        coord_set(s, "coord_views", &json!(views))?;
        return Ok(json!({ "views": views }));
    }

    // Restore: apply the whole saved state (one undo step: the parts run here, not as commands
    // of their own). What could not be put back is reported in `warnings`.
    if let Some(restore_name) = p.get("restore").and_then(Value::as_str) {
        let view = views.iter().find(|v| v.get("name").and_then(Value::as_str) == Some(restore_name)).cloned();
        let Some(view) = view else {
            let names: Vec<&str> = views.iter().filter_map(|v| v.get("name").and_then(Value::as_str)).take(50).collect();
            return Err(EngineError::Other(format!("coordination view '{restore_name}' not found — saved views: {names:?}")));
        };
        let mut warnings: Vec<String> = Vec::new();

        // Camera.
        let center = view
            .get("center")
            .and_then(Value::as_array)
            .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)]);
        let height = view.get("height").and_then(Value::as_f64);
        if let Some(vd) = arr3(&view, "view_dir")
            && let Err(e) = s.execute("vpoint", &json!({ "dir": vd }))
        {
            warnings.push(format!("view direction: {e}"));
        }
        if let (Some(c), Some(h)) = (center, height)
            && let Err(e) = s.execute("zoom", &json!({ "mode": "center", "center": c, "height": h }))
        {
            warnings.push(format!("camera: {e}"));
        }
        if let Some(vs) = view.get("visual_style").and_then(Value::as_str)
            && let Err(e) = s.execute("vscurrent", &json!({ "style": vs }))
        {
            warnings.push(format!("visual style: {e}"));
        }

        // Section box: a view saved before any box was set has it off.
        let sbox = match view.get("sectionbox").filter(|v| v.is_object()) {
            Some(b) => b.clone(),
            None => {
                let mut cur = coord_get(s.doc()?, "coord_sectionbox");
                match cur.as_object_mut() {
                    Some(o) => {
                        o.insert("on".into(), json!(false));
                        cur
                    }
                    None => Value::Null,
                }
            }
        };
        coord_set(s, "coord_sectionbox", &sbox)?;

        // Coordination mode, on or off (off puts back the layers' own transparency).
        let mode = view.get("coord_mode").cloned().unwrap_or(Value::Null);
        let on = mode.get("on").and_then(Value::as_bool).unwrap_or(false);
        let mut mp = json!({ "on": on });
        if let Some(g) = mode.get("ghost_opacity").and_then(Value::as_f64) {
            mp["ghost_opacity"] = json!(g);
        }
        run_mode(s, &mp)?;

        // Colour scheme.
        if let Some(scheme_name) = view.get("color_scheme").and_then(Value::as_str)
            && let Err(e) = run_colors(s, &json!({ "load": scheme_name }))
        {
            warnings.push(format!("colour scheme: {e}"));
        }

        // Trade visibility.
        if let Some(trades) = view.get("trades").and_then(Value::as_array) {
            let want: Vec<(&str, bool)> = trades
                .iter()
                .take(200)
                .filter_map(|tv| Some((tv.get("layer").and_then(Value::as_str)?, tv.get("on").and_then(Value::as_bool)?)))
                .collect();
            let d = s.doc()?;
            let differs = want.iter().any(|(n, on)| d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(n) && l.on != *on));
            if differs {
                let d = s.doc_mut()?;
                for (lname, on) in &want {
                    if let Some(layer) = d.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(lname)) {
                        layer.on = *on;
                    }
                }
            }
        }

        return Ok(json!({ "restored": restore_name, "warnings": warnings }));
    }

    // List.
    if p.get("list").and_then(Value::as_bool).unwrap_or(false) {
        let names: Vec<&str> = views.iter().filter_map(|v| v.get("name").and_then(Value::as_str)).collect();
        return Ok(json!({ "views": names }));
    }

    Ok(json!({ "views": views }))
}

/// Read coordination state as a public helper for the renderer: section box, coordination mode.
/// Returns `(sbox_on, sbox_min, sbox_max, coord_mode_on, ghost_opacity)`.
pub fn coord_render_state(d: &Drawing) -> CoordRenderState {
    let sbox = coord_get(d, "coord_sectionbox");
    let sbox_on = sbox.get("on").and_then(Value::as_bool).unwrap_or(false);
    let sbox_min = arr3(&sbox, "min").unwrap_or([f64::NEG_INFINITY; 3]);
    let sbox_max = arr3(&sbox, "max").unwrap_or([f64::INFINITY; 3]);

    let mode = coord_get(d, "coord_mode");
    let mode_on = mode.get("on").and_then(Value::as_bool).unwrap_or(false);
    let ghost_opacity = mode.get("ghost_opacity").and_then(Value::as_f64).unwrap_or(0.2);

    CoordRenderState { sbox_on, sbox_min, sbox_max, mode_on, ghost_opacity }
}

/// Coordination rendering state read from the drawing.
#[derive(Clone, Debug, Default)]
pub struct CoordRenderState {
    pub sbox_on: bool,
    pub sbox_min: [f64; 3],
    pub sbox_max: [f64; 3],
    pub mode_on: bool,
    pub ghost_opacity: f64,
}

impl CoordRenderState {
    /// Whether a point is inside the section box (or the box is off).
    pub fn inside_sbox(&self, p: Vec3) -> bool {
        if !self.sbox_on {
            return true;
        }
        p.x >= self.sbox_min[0]
            && p.x <= self.sbox_max[0]
            && p.y >= self.sbox_min[1]
            && p.y <= self.sbox_max[1]
            && p.z >= self.sbox_min[2]
            && p.z <= self.sbox_max[2]
    }

    /// The 6 section planes of the section box (for clipping), empty if the box is off.
    pub fn sbox_planes(&self) -> Vec<cadaddy_doc::SectionPlane> {
        if !self.sbox_on {
            return Vec::new();
        }
        let lo = self.sbox_min;
        let hi = self.sbox_max;
        let big = 1e6;
        vec![
            // +X face: keep what is below max X (normal = -X, origin at max X).
            cadaddy_doc::SectionPlane {
                origin: Vec3::new(hi[0], (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0),
                normal: Vec3::new(-1.0, 0.0, 0.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
            // -X face.
            cadaddy_doc::SectionPlane {
                origin: Vec3::new(lo[0], (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0),
                normal: Vec3::new(1.0, 0.0, 0.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
            // +Y face.
            cadaddy_doc::SectionPlane {
                origin: Vec3::new((lo[0] + hi[0]) / 2.0, hi[1], (lo[2] + hi[2]) / 2.0),
                normal: Vec3::new(0.0, -1.0, 0.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
            // -Y face.
            cadaddy_doc::SectionPlane {
                origin: Vec3::new((lo[0] + hi[0]) / 2.0, lo[1], (lo[2] + hi[2]) / 2.0),
                normal: Vec3::new(0.0, 1.0, 0.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
            // +Z face.
            cadaddy_doc::SectionPlane {
                origin: Vec3::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, hi[2]),
                normal: Vec3::new(0.0, 0.0, -1.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
            // -Z face.
            cadaddy_doc::SectionPlane {
                origin: Vec3::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]),
                normal: Vec3::new(0.0, 0.0, 1.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
        ]
    }
}

// ─── Section box face drag ───────────────────────────────────────────────────

/// A face of the section box, which the 3D view's handles drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxFace {
    MinX,
    MaxX,
    MinY,
    MaxY,
    MinZ,
    MaxZ,
}

impl BoxFace {
    pub const ALL: [BoxFace; 6] = [BoxFace::MinX, BoxFace::MaxX, BoxFace::MinY, BoxFace::MaxY, BoxFace::MinZ, BoxFace::MaxZ];

    /// The axis the face is across: 0 = X, 1 = Y, 2 = Z.
    pub fn axis(self) -> usize {
        match self {
            BoxFace::MinX | BoxFace::MaxX => 0,
            BoxFace::MinY | BoxFace::MaxY => 1,
            BoxFace::MinZ | BoxFace::MaxZ => 2,
        }
    }

    /// Whether it is the face at the box's max on its axis.
    pub fn is_max(self) -> bool {
        matches!(self, BoxFace::MaxX | BoxFace::MaxY | BoxFace::MaxZ)
    }
}

/// The centre of a face of the box `lo`..`hi` (where its drag handle sits).
pub fn sbox_face_centre(lo: [f64; 3], hi: [f64; 3], face: BoxFace) -> [f64; 3] {
    let mut c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let a = face.axis();
    if let (Some(slot), Some(v)) = (c.get_mut(a), if face.is_max() { hi.get(a) } else { lo.get(a) }) {
        *slot = *v;
    }
    c
}

/// The box after dragging `face` by `delta` drawing units along its axis (outwards for a max
/// face when positive). The face stops at the opposite one: the box never turns inside out. A
/// non-finite `delta` leaves the box as it is.
pub fn sbox_drag_face(lo: [f64; 3], hi: [f64; 3], face: BoxFace, delta: f64) -> ([f64; 3], [f64; 3]) {
    let (mut lo, mut hi) = (lo, hi);
    if !delta.is_finite() {
        return (lo, hi);
    }
    let a = face.axis();
    if let (Some(l), Some(h)) = (lo.get_mut(a), hi.get_mut(a)) {
        if face.is_max() {
            *h = (*h + delta).max(*l);
        } else {
            *l = (*l + delta).min(*h);
        }
    }
    (lo, hi)
}

/// How far (drawing units along the face's axis) a pointer drag of `drag_px` (screen pixels, y
/// down) moves a face, when `step` units along that axis show as `axis_px` pixels on screen.
/// `None` when the axis points at the viewer (it shows shorter than 2 pixels): the face can't be
/// dragged in that view.
pub fn sbox_drag_delta(axis_px: [f64; 2], step: f64, drag_px: [f64; 2]) -> Option<f64> {
    let len2 = axis_px[0] * axis_px[0] + axis_px[1] * axis_px[1];
    if len2.is_nan() || len2 < 4.0 || !step.is_finite() || step <= 0.0 {
        return None;
    }
    let along = (drag_px[0] * axis_px[0] + drag_px[1] * axis_px[1]) / len2;
    Some(along * step).filter(|d| d.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure3d_basic() {
        let mut s = Session::new();
        let r = run_measure3d(&mut s, &json!({ "from": [0, 0, 0], "to": [3, 4, 0] }));
        assert!(r.is_ok());
        let v = r.as_ref().ok().and_then(|v| v.get("distance")).and_then(Value::as_f64).unwrap_or(0.0);
        assert!((v - 5.0).abs() < 0.01, "distance = {v}");
    }

    #[test]
    fn measure3d_vertical() {
        let mut s = Session::new();
        let r = run_measure3d(&mut s, &json!({ "from": [0, 0, 0], "to": [0, 0, 10] }));
        let dz = r.as_ref().ok().and_then(|v| v.get("dz")).and_then(Value::as_f64).unwrap_or(0.0);
        assert!((dz - 10.0).abs() < 0.01);
    }

    #[test]
    fn uuid_format() {
        let u = stable_uuid("test");
        assert_eq!(u.len(), 36);
        assert_eq!(u.chars().nth(14), Some('4'));
        // Deterministic: same input produces the same UUID.
        assert_eq!(u, stable_uuid("test"));
        // Different input produces a different UUID.
        assert_ne!(u, stable_uuid("other"));
    }

    #[test]
    fn zip_mini_valid() {
        let mut buf = Vec::new();
        {
            let mut z = zip_mini::ZipWriter::new(&mut buf);
            z.start_file("test.txt").ok();
            std::io::Write::write_all(&mut z, b"hello world").ok();
            z.finish().ok();
        }
        assert_eq!(&buf[..4], &[0x50, 0x4b, 0x03, 0x04]);
        assert!(buf.windows(4).any(|w| w == [0x50, 0x4b, 0x05, 0x06]));
    }

    #[test]
    fn iso_date_format() {
        let d = iso_date();
        assert!(d.ends_with('Z'));
        assert_eq!(d.len(), 20);
    }

    #[test]
    fn xml_escape() {
        assert_eq!(escape_xml("a<b>c&d"), "a&lt;b&gt;c&amp;d");
    }

    #[test]
    fn arr3_parses() {
        let v = json!({ "pt": [1.0, 2.0, 3.0] });
        assert_eq!(arr3(&v, "pt"), Some([1.0, 2.0, 3.0]));
        assert_eq!(arr3(&v, "nope"), None);
    }

    #[test]
    fn coordination_mode_toggles() {
        let mut s = Session::new();
        let r = run_mode(&mut s, &json!({ "on": true, "ghost_opacity": 0.3 }));
        assert!(r.is_ok());
        let v = r.as_ref().ok().and_then(|v| v.get("on")).and_then(Value::as_bool).unwrap_or(false);
        assert!(v, "mode should be on");
        let opacity = r.as_ref().ok().and_then(|v| v.get("ghost_opacity")).and_then(Value::as_f64).unwrap_or(0.0);
        assert!((opacity - 0.3).abs() < 0.01);

        // Turn off.
        let r = run_mode(&mut s, &json!({ "on": false }));
        assert!(r.is_ok());
        let v = r.as_ref().ok().and_then(|v| v.get("on")).and_then(Value::as_bool).unwrap_or(true);
        assert!(!v, "mode should be off");
    }

    #[test]
    fn color_scheme_crud() {
        let mut s = Session::new();
        // Save.
        let r = run_colors(
            &mut s,
            &json!({
                "save": { "name": "Test Scheme", "colors": { "Supply": [255, 0, 0], "Return": [0, 0, 255] } }
            }),
        );
        assert!(r.is_ok());
        let saved = r.as_ref().ok().and_then(|v| v.get("saved")).and_then(Value::as_str);
        assert_eq!(saved, Some("Test Scheme"));

        // List.
        let r = run_colors(&mut s, &json!({ "list": true }));
        assert!(r.is_ok());
        let schemes = r.as_ref().ok().and_then(|v| v.get("schemes")).and_then(Value::as_array);
        assert_eq!(schemes.map(|a| a.len()), Some(1));

        // Delete.
        let r = run_colors(&mut s, &json!({ "delete": "Test Scheme" }));
        assert!(r.is_ok());
        let r = run_colors(&mut s, &json!({ "list": true }));
        let schemes = r.as_ref().ok().and_then(|v| v.get("schemes")).and_then(Value::as_array);
        assert_eq!(schemes.map(|a| a.len()), Some(0));
    }

    #[test]
    fn coord_views_save_and_delete() {
        let mut s = Session::new();
        let r = run_views(&mut s, &json!({ "save": { "name": "View A" } }));
        assert!(r.is_ok());
        let views = r.as_ref().ok().and_then(|v| v.get("views")).and_then(Value::as_array);
        assert_eq!(views.map(|a| a.len()), Some(1));

        // Delete.
        let r = run_views(&mut s, &json!({ "delete": "View A" }));
        assert!(r.is_ok());
        let views = r.as_ref().ok().and_then(|v| v.get("views")).and_then(Value::as_array);
        assert_eq!(views.map(|a| a.len()), Some(0));
    }

    #[test]
    fn coord_render_state_defaults() {
        let d = cadaddy_doc::Drawing::new_imperial();
        let crs = coord_render_state(&d);
        assert!(!crs.sbox_on);
        assert!(!crs.mode_on);
        assert!((crs.ghost_opacity - 0.2).abs() < 0.01);
    }

    #[test]
    fn sbox_planes_generates_six_when_on() {
        let crs = CoordRenderState { sbox_on: true, sbox_min: [-10.0, -10.0, 0.0], sbox_max: [10.0, 10.0, 20.0], mode_on: false, ghost_opacity: 0.2 };
        let planes = crs.sbox_planes();
        assert_eq!(planes.len(), 6);
        // A point inside should pass all planes.
        let inside = Vec3::new(0.0, 0.0, 10.0);
        for p in &planes {
            assert!(p.side(inside) >= 0.0, "inside point should be on kept side");
        }
        // A point outside should fail at least one.
        let outside = Vec3::new(20.0, 0.0, 10.0);
        assert!(planes.iter().any(|p| p.side(outside) < 0.0), "outside point should be clipped");
    }

    #[test]
    fn sbox_planes_empty_when_off() {
        let crs = CoordRenderState::default();
        assert!(crs.sbox_planes().is_empty());
    }

    #[test]
    fn inside_sbox_check() {
        let crs = CoordRenderState { sbox_on: true, sbox_min: [0.0, 0.0, 0.0], sbox_max: [10.0, 10.0, 10.0], ..CoordRenderState::default() };
        assert!(crs.inside_sbox(Vec3::new(5.0, 5.0, 5.0)));
        assert!(!crs.inside_sbox(Vec3::new(15.0, 5.0, 5.0)));
        // When off, everything is inside.
        let off = CoordRenderState::default();
        assert!(off.inside_sbox(Vec3::new(100.0, 100.0, 100.0)));
    }
}
