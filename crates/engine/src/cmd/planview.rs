//! Plan views: 2D plan representations generated from the building model's objects.
//!
//! Each building element (wall, door, window, room, device, duct, pipe) is ONE object that holds
//! properties, a 3D model and a 2D plan representation. The plan entities are tagged with XDATA
//! linking them to their source element; selecting a plan entity resolves to the object so the
//! Properties panel shows/edits all its parameters.
//!
//! ## Commands
//!
//! - `plan.views` — Generate (or refresh) plan representations from the built model.
//! - `plan.resolve` — Given a plan entity handle, return the building element it belongs to.
//! - `plan.wall` — Create a wall object by specifying endpoints and parameters.
//! - `plan.door` — Place a door in an existing wall.
//! - `plan.window` — Place a window in an existing wall.
//!
//! ## Layers (NCS/AIA)
//!
//! Plan entities go on standard NCS layers; the original PDF linework moves to a locked
//! `PDF-UNDERLAY <sheet>` layer.

use std::collections::HashMap;
use std::f64::consts::FRAC_PI_2;

use cadaddy_doc::color::Color;
use cadaddy_doc::props::{self, Prop, Props, Source};
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Layer, RawTag, Space};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// XDATA application name tagging plan-view entities with their source element.
pub const APP: &str = "CADADDY_PLANVIEW";

/// The layers plan entities go on, with their default colour indices.
const PLAN_LAYERS: &[(&str, u8)] = &[
    ("A-WALL", 8),
    ("A-WALL-PATT", 253),
    ("A-DOOR", 1),
    ("A-GLAZ", 4),
    ("A-AREA-IDEN", 3),
    ("E-POWR", 6),
    ("E-LITE", 2),
    ("M-DUCT", 174),
    ("P-PIPE", 30),
    ("E-COND", 14),
];

// ─── Commands ────────────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("plan.views", "Generate Plan Views", run_views)
            .menu(&["Draw", "BIM", "Plan Views"])
            .params("{clear?: bool, underlay?: bool (default true: move source linework to PDF-UNDERLAY layer)} → {walls, doors, windows, rooms, devices, ducts, pipes, levels, removed, notes: [string]}"),
        CommandSpec::new("plan.resolve", "Resolve Plan Entity", run_resolve)
            .params("{handle: string} → {kind, level, source_handle, element_kind?, props?}")
            .noundo(),
        CommandSpec::new("plan.wall", "Draw Wall (Object)", run_wall)
            .menu(&["Draw", "BIM", "Wall"])
            .params("{a: [x,y], b: [x,y], thickness?: inches (default 6), height?: feet (default 9), type?: string, level?: string} → {handle, plan_handles: [string], model_handle?}")
            .interactive(|_| Ok(Box::new(WallM::default()))),
        CommandSpec::new("plan.door", "Place Door in Wall", run_door)
            .menu(&["Draw", "BIM", "Door"])
            .params("{wall: handle, position?: station (0-1 along wall, default 0.5), width?: inches (default 36), height?: feet (default 6.67), swing?: \"left\" | \"right\" (default \"left\")} → {handle, plan_handles: [string]}")
            .interactive(|_| Ok(Box::new(DoorWindowM::new("plan.door")))),
        CommandSpec::new("plan.window", "Place Window in Wall", run_window)
            .menu(&["Draw", "BIM", "Window"])
            .params("{wall: handle, position?: station (0-1 along wall, default 0.5), width?: inches (default 36), sill?: feet (default 3), height?: feet (default 4)} → {handle, plan_handles: [string]}")
            .interactive(|_| Ok(Box::new(DoorWindowM::new("plan.window")))),
    ]
}

// ─── Plan-view XDATA ─────────────────────────────────────────────────────────

/// Tag a plan entity: what kind, which level, and the source 3D element's handle.
fn plan_tag(kind: &str, level: &str, source: Handle) -> Vec<RawTag> {
    let t = |code: i32, v: String| RawTag { code, value: v };
    vec![t(1001, APP.into()), t(1000, kind.chars().take(64).collect()), t(1000, level.chars().take(255).collect()), t(1000, source.hex())]
}

/// Read the plan-view tag from an entity: (kind, level, source handle).
pub fn plan_of(e: &Entity) -> Option<(String, String, Handle)> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(APP))?;
    let kind = x.get(at + 1).filter(|t| t.code == 1000)?.value.clone();
    let level = x.get(at + 2).filter(|t| t.code == 1000)?.value.clone();
    let handle_hex = x.get(at + 3).filter(|t| t.code == 1000)?;
    let h = Handle(u64::from_str_radix(handle_hex.value.trim(), 16).ok()?);
    Some((kind, level, h))
}

/// Remove all plan-view entities from model space; how many were removed.
pub fn remove_plan_views(d: &mut Drawing) -> usize {
    let hs: Vec<Handle> = d.model.iter().filter(|e| plan_of(e).is_some()).map(|e| e.handle).collect();
    for h in &hs {
        d.model.remove(*h);
    }
    hs.len()
}

// ─── plan.views: generate plan representations ──────────────────────────────

fn run_views(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("clear").and_then(Value::as_bool) == Some(true) {
        let n = remove_plan_views(s.doc_mut()?);
        return Ok(json!({ "removed": n }));
    }
    let underlay = p.get("underlay").and_then(Value::as_bool).unwrap_or(true);
    generate_plan_views(s, underlay)
}

/// Generate plan views from the model elements.
fn generate_plan_views(s: &mut Session, underlay: bool) -> Result<Value> {
    let d = s.doc()?;
    // Collect model elements by level.
    let mut elements: Vec<ModelElement> = Vec::new();
    for e in d.model.iter() {
        if let Some(part) = super::wallgen::made_as(e) {
            let level = wallgen_level(e).unwrap_or_default();
            let source = wallgen_source(e).unwrap_or(e.handle);
            elements.push(ModelElement { handle: e.handle, part: part.to_ascii_lowercase(), level, _source: source });
        }
    }
    if elements.is_empty() {
        s.echo("No model elements yet: build the model first (Plans to Model / model.build).".to_string());
        return Ok(json!({
            "walls": 0, "doors": 0, "windows": 0, "rooms": 0, "devices": 0,
            "ducts": 0, "pipes": 0, "levels": 0, "removed": 0,
            "notes": ["no model elements found — run model.build first"],
        }));
    }
    let mut by_level: HashMap<String, Vec<&ModelElement>> = HashMap::new();
    for el in &elements {
        by_level.entry(el.level.clone()).or_default().push(el);
    }
    let (sp, _) = super::bspec::current(s, "plan.views")?;
    let d = s.doc_mut()?;
    ensure_plan_layers(d);
    let removed = remove_plan_views(d);
    if underlay {
        move_source_to_underlay(d, &sp);
    }
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut counts = PlanCounts::default();
    let mut notes: Vec<String> = Vec::new();
    for level_spec in &sp.levels {
        let Some(elems) = by_level.get(&level_spec.name) else { continue };
        let base = match level_spec.base {
            Some(b) => Vec2::new(b[0], b[1]),
            None => {
                notes.push(format!("{}: no plan base point, skipped.", level_spec.name));
                continue;
            }
        };
        for el in elems {
            match el.part.as_str() {
                "wall" => generate_wall_plan(d, el, &sp, &level_spec.name, &mut counts),
                "door" => generate_door_plan(d, el, &level_spec.name, &mut counts),
                "window" | "glass" => generate_window_plan(d, el, &level_spec.name, &mut counts),
                "device" => generate_device_plan(d, el, &level_spec.name, &mut counts),
                _ => {}
            }
        }
        generate_rooms_for_level(d, &level_spec.name, base, upf, &mut counts);
    }
    s.echo(format!(
        "Plan views: {} walls, {} doors, {} windows, {} rooms, {} devices on {} level(s).",
        counts.walls,
        counts.doors,
        counts.windows,
        counts.rooms,
        counts.devices,
        by_level.len()
    ));
    Ok(json!({
        "walls": counts.walls, "doors": counts.doors, "windows": counts.windows,
        "rooms": counts.rooms, "devices": counts.devices, "ducts": counts.ducts,
        "pipes": counts.pipes, "levels": by_level.len(), "removed": removed, "notes": notes,
    }))
}

struct ModelElement {
    handle: Handle,
    part: String,
    level: String,
    _source: Handle,
}

#[derive(Default)]
struct PlanCounts {
    walls: usize,
    doors: usize,
    windows: usize,
    rooms: usize,
    devices: usize,
    ducts: usize,
    pipes: usize,
}

fn wallgen_level(e: &Entity) -> Option<String> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP))?;
    x.get(at + 2).filter(|t| t.code == 1000).map(|t| t.value.clone())
}

fn wallgen_source(e: &Entity) -> Option<Handle> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP))?;
    let hex = x.get(at + 3).filter(|t| t.code == 1000)?;
    Some(Handle(u64::from_str_radix(hex.value.trim(), 16).ok()?))
}

fn ensure_plan_layers(d: &mut Drawing) {
    for &(name, ci) in PLAN_LAYERS {
        if d.layer(name).is_none() {
            d.layers.push(Layer { name: name.into(), color: Color::Index(ci), ..Layer::default() });
        }
    }
}

fn move_source_to_underlay(d: &mut Drawing, sp: &cadaddy_walk3d::BuildingSpec) {
    let source_layers: Vec<String> = sp.layers.iter().filter(|r| r.role != cadaddy_walk3d::Role::Ignore).map(|r| r.layer.clone()).collect();
    if source_layers.is_empty() {
        return;
    }
    let underlay_layer = "PDF-UNDERLAY";
    if d.layer(underlay_layer).is_none() {
        d.layers.push(Layer { name: underlay_layer.into(), color: Color::Index(251), locked: true, ..Layer::default() });
    }
    let hs: Vec<Handle> = d
        .model
        .iter()
        .filter(|e| {
            super::wallgen::made_as(e).is_none() && plan_of(e).is_none() && source_layers.iter().any(|l| super::qselect::wildcard(l, &e.common.layer))
        })
        .map(|e| e.handle)
        .collect();
    for h in hs {
        d.model.modify(h, |e| {
            e.common.layer = underlay_layer.into();
        });
    }
    if let Some(l) = d.layers.iter_mut().find(|l| l.name == underlay_layer) {
        l.locked = true;
    }
}

// ─── Wall plan ───────────────────────────────────────────────────────────────

fn generate_wall_plan(d: &mut Drawing, el: &ModelElement, sp: &cadaddy_walk3d::BuildingSpec, level: &str, counts: &mut PlanCounts) {
    let Some(model_ent) = d.model.get(el.handle) else { return };
    let EntityKind::Mesh(ref mesh) = model_ent.kind else { return };
    let pts: Vec<Vec2> = mesh.vertices.iter().map(|v| Vec2::new(v.x, v.y)).collect();
    if pts.len() < 4 {
        return;
    }
    let (min_x, max_x) = minmax_x(&pts);
    let (min_y, max_y) = minmax_y(&pts);
    let dx = max_x - min_x;
    let dy = max_y - min_y;
    let (a, b, thickness) = if dx >= dy {
        let cy = (min_y + max_y) / 2.0;
        (Vec2::new(min_x, cy), Vec2::new(max_x, cy), dy)
    } else {
        let cx = (min_x + max_x) / 2.0;
        (Vec2::new(cx, min_y), Vec2::new(cx, max_y), dx)
    };
    if thickness < 0.1 || a.dist(b) < 0.1 {
        return;
    }
    let dir = (b - a).normalized();
    let perp = Vec2::new(-dir.y, dir.x);
    let half_t = thickness / 2.0;
    let f1a = a + perp * half_t;
    let f1b = b + perp * half_t;
    let f2a = a - perp * half_t;
    let f2b = b - perp * half_t;
    let xtags = plan_tag("wall", level, el.handle);
    let common = Common { layer: "A-WALL".into(), xdata: xtags.clone().into(), ..Common::default() };
    let _ = d.add(&Space::Model, common.clone(), EntityKind::Line(cadaddy_doc::Line { a: v3(f1a), b: v3(f1b) }));
    let _ = d.add(&Space::Model, common, EntityKind::Line(cadaddy_doc::Line { a: v3(f2a), b: v3(f2b) }));
    // Wall type tag at midpoint.
    let wall_type_name = wallgen_wall_type(d.model.get(el.handle).map(|e| &**e)).or_else(|| sp.wall_types.first().map(|wt| wt.name.clone()));
    if let Some(type_name) = wall_type_name {
        let mid = (a + b) * 0.5;
        let tag_common = Common { layer: "A-WALL".into(), xdata: plan_tag("wall_tag", level, el.handle).into(), ..Common::default() };
        let _ = d.add(
            &Space::Model,
            tag_common,
            EntityKind::Text(cadaddy_doc::Text {
                insert: v3(mid),
                align_pt: None,
                height: thickness.max(3.0),
                value: type_name,
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: cadaddy_doc::HAlign::Center,
                valign: cadaddy_doc::VAlign::Middle,
            }),
        );
    }
    counts.walls += 1;
}

fn wallgen_wall_type(e: Option<&Entity>) -> Option<String> {
    let e = e?;
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP))?;
    x.get(at + 4).filter(|t| t.code == 1000 && t.value.starts_with("type:")).map(|t| t.value[5..].to_string())
}

// ─── Door plan ───────────────────────────────────────────────────────────────

fn generate_door_plan(d: &mut Drawing, el: &ModelElement, level: &str, counts: &mut PlanCounts) {
    let Some(model_ent) = d.model.get(el.handle) else { return };
    let EntityKind::Mesh(ref mesh) = model_ent.kind else { return };
    let pts: Vec<Vec2> = mesh.vertices.iter().map(|v| Vec2::new(v.x, v.y)).collect();
    if pts.len() < 3 {
        return;
    }
    let (min_x, max_x) = minmax_x(&pts);
    let (min_y, max_y) = minmax_y(&pts);
    let dx = max_x - min_x;
    let dy = max_y - min_y;
    let (hinge, open_end, swing_radius) = if dx > dy {
        (Vec2::new(min_x, (min_y + max_y) / 2.0), Vec2::new(max_x, (min_y + max_y) / 2.0), dx)
    } else {
        (Vec2::new((min_x + max_x) / 2.0, min_y), Vec2::new((min_x + max_x) / 2.0, max_y), dy)
    };
    let xtags = plan_tag("door", level, el.handle);
    let common = Common { layer: "A-DOOR".into(), xdata: xtags.into(), ..Common::default() };
    let _ = d.add(&Space::Model, common.clone(), EntityKind::Line(cadaddy_doc::Line { a: v3(hinge), b: v3(open_end) }));
    let start_angle = (open_end.y - hinge.y).atan2(open_end.x - hinge.x);
    let _ = d.add(
        &Space::Model,
        common,
        EntityKind::Arc(cadaddy_doc::Arc { center: v3(hinge), radius: swing_radius, start: start_angle, end: start_angle + FRAC_PI_2 }),
    );
    counts.doors += 1;
}

// ─── Window plan ─────────────────────────────────────────────────────────────

fn generate_window_plan(d: &mut Drawing, el: &ModelElement, level: &str, counts: &mut PlanCounts) {
    let Some(model_ent) = d.model.get(el.handle) else { return };
    let EntityKind::Mesh(ref mesh) = model_ent.kind else { return };
    let pts: Vec<Vec2> = mesh.vertices.iter().map(|v| Vec2::new(v.x, v.y)).collect();
    if pts.len() < 4 {
        return;
    }
    let (min_x, max_x) = minmax_x(&pts);
    let (min_y, max_y) = minmax_y(&pts);
    let dx = max_x - min_x;
    let dy = max_y - min_y;
    let glass_offset = 0.5;
    let (a, b, perp) = if dx > dy {
        let cy = (min_y + max_y) / 2.0;
        (Vec2::new(min_x, cy), Vec2::new(max_x, cy), Vec2::new(0.0, 1.0))
    } else {
        let cx = (min_x + max_x) / 2.0;
        (Vec2::new(cx, min_y), Vec2::new(cx, max_y), Vec2::new(1.0, 0.0))
    };
    let xtags = plan_tag("window", level, el.handle);
    let common = Common { layer: "A-GLAZ".into(), xdata: xtags.into(), ..Common::default() };
    let _ =
        d.add(&Space::Model, common.clone(), EntityKind::Line(cadaddy_doc::Line { a: v3(a + perp * glass_offset), b: v3(b + perp * glass_offset) }));
    let _ = d.add(&Space::Model, common, EntityKind::Line(cadaddy_doc::Line { a: v3(a - perp * glass_offset), b: v3(b - perp * glass_offset) }));
    counts.windows += 1;
}

// ─── Device plan ─────────────────────────────────────────────────────────────

fn generate_device_plan(d: &mut Drawing, el: &ModelElement, level: &str, counts: &mut PlanCounts) {
    let Some(model_ent) = d.model.get(el.handle) else { return };
    let EntityKind::Mesh(ref mesh) = model_ent.kind else { return };
    if mesh.vertices.is_empty() {
        return;
    }
    let cx = mesh.vertices.iter().map(|v| v.x).sum::<f64>() / mesh.vertices.len() as f64;
    let cy = mesh.vertices.iter().map(|v| v.y).sum::<f64>() / mesh.vertices.len() as f64;
    let pos = Vec2::new(cx, cy);
    let layer = device_plan_layer(model_ent);
    let xtags = plan_tag("device", level, el.handle);
    let block_name = super::bimlook::made_tag(model_ent, "block").map(str::to_string);
    if let Some(block) = block_name.filter(|b| d.blocks.contains_key(b.as_str())) {
        let _ = d.add(
            &Space::Model,
            Common { layer: layer.into(), xdata: xtags.into(), ..Common::default() },
            EntityKind::Insert(cadaddy_doc::Insert {
                block,
                insert: v3(pos),
                scale: Vec3::new(1.0, 1.0, 1.0),
                rotation: 0.0,
                attribs: Vec::new(),
                cols: 1,
                rows: 1,
                col_spacing: 0.0,
                row_spacing: 0.0,
                clip: None,
            }),
        );
    } else {
        let _ = d.add(
            &Space::Model,
            Common { layer: layer.into(), xdata: xtags.into(), ..Common::default() },
            EntityKind::Circle(cadaddy_doc::Circle { center: v3(pos), radius: 3.0 }),
        );
    }
    counts.devices += 1;
}

fn device_plan_layer(e: &Entity) -> &'static str {
    let layer_upper = e.common.layer.to_ascii_uppercase();
    if layer_upper.contains("LIGHT") || layer_upper.contains("LTG") || layer_upper.contains("LITE") {
        "E-LITE"
    } else if layer_upper.contains("DUCT") {
        "M-DUCT"
    } else if layer_upper.contains("PIPE") || layer_upper.contains("PLUMB") {
        "P-PIPE"
    } else if layer_upper.contains("COND") {
        "E-COND"
    } else {
        "E-POWR"
    }
}

// ─── Room plan ───────────────────────────────────────────────────────────────

fn generate_rooms_for_level(d: &mut Drawing, level: &str, _base: Vec2, _upf: f64, counts: &mut PlanCounts) {
    let room_handles: Vec<(Handle, String)> = d
        .model
        .iter()
        .filter_map(|e| {
            let _room_name = super::rooms3d::room_of(&e.common)?;
            let room_level = wallgen_level(e).unwrap_or_default();
            if room_level != level {
                return None;
            }
            let room_name = super::rooms3d::room_of(&e.common).unwrap_or_default();
            Some((e.handle, room_name))
        })
        .collect();
    // We can't easily get the center_pt inside the closure above since entity_bounds borrows d.
    // Instead, collect handles and names, then compute bounds in a second pass.
    for (handle, name) in &room_handles {
        let center_pt = d.model.get(*handle).and_then(|e| {
            let b = cadaddy_doc::entity_bounds(d, e, 0);
            (!b.is_empty()).then(|| b.center())
        });
        let Some(center) = center_pt else { continue };
        let xtags = plan_tag("room", level, *handle);
        let _ = d.add(
            &Space::Model,
            Common { layer: "A-AREA-IDEN".into(), xdata: xtags.into(), ..Common::default() },
            EntityKind::MText(cadaddy_doc::MText {
                insert: v3(center),
                height: 4.0,
                width: 48.0,
                attach: 5,
                rotation: 0.0,
                style: "Standard".into(),
                contents: name.clone(),
                line_spacing: 1.0,
                line_spacing_exact: false,
                columns: None,
                background: None,
            }),
        );
        counts.rooms += 1;
    }
}

// ─── plan.resolve ────────────────────────────────────────────────────────────

fn run_resolve(s: &mut Session, p: &Value) -> Result<Value> {
    let hex = str_param(p, "handle").ok_or_else(|| bad("plan.resolve", "`handle` is required"))?;
    let h = Handle(u64::from_str_radix(hex.trim_start_matches("0x"), 16).map_err(|_| bad("plan.resolve", "invalid handle"))?);
    let d = s.doc()?;
    let e = d.model.get(h).ok_or_else(|| bad("plan.resolve", "entity not found"))?;
    if let Some((kind, level, source)) = plan_of(e) {
        let mut result = json!({ "kind": kind, "level": level, "source_handle": source.hex() });
        if let Some(source_ent) = d.model.get(source) {
            if let Some(ek) = super::elemprops::element_kind(source_ent) {
                result["element_kind"] = json!(ek);
            }
            let p = props::read(&source_ent.common.xdata);
            if !p.is_empty() {
                let pj: serde_json::Map<String, Value> = p
                    .iter()
                    .map(|(k, pr)| (k.to_string(), json!({ "value": pr.value, "unit": pr.unit, "source": pr.source.kind.as_str() })))
                    .collect();
                result["props"] = Value::Object(pj);
            }
        }
        return Ok(result);
    }
    if let Some(part) = super::wallgen::made_as(e) {
        let level = wallgen_level(e).unwrap_or_default();
        let source = wallgen_source(e).unwrap_or(e.handle);
        let mut result = json!({ "kind": part, "level": level, "source_handle": source.hex() });
        if let Some(ek) = super::elemprops::element_kind(e) {
            result["element_kind"] = json!(ek);
        }
        let p = props::read(&e.common.xdata);
        if !p.is_empty() {
            let pj: serde_json::Map<String, Value> =
                p.iter().map(|(k, pr)| (k.to_string(), json!({ "value": pr.value, "unit": pr.unit, "source": pr.source.kind.as_str() }))).collect();
            result["props"] = Value::Object(pj);
        }
        return Ok(result);
    }
    Err(bad("plan.resolve", "entity is not a plan view or model element"))
}

// ─── plan.wall ───────────────────────────────────────────────────────────────

fn run_wall(s: &mut Session, p: &Value) -> Result<Value> {
    let a = point_param(p, "a").ok_or_else(|| bad("plan.wall", "`a` (start point) is required"))?;
    let b = point_param(p, "b").ok_or_else(|| bad("plan.wall", "`b` (end point) is required"))?;
    let thickness_in = p.get("thickness").and_then(Value::as_f64).unwrap_or(6.0);
    let height_ft = p.get("height").and_then(Value::as_f64).unwrap_or(9.0);
    let wall_type = str_param(p, "type").unwrap_or("Standard").to_string();
    let level = str_param(p, "level").unwrap_or("Level 1").to_string();
    let (sp, _) = super::bspec::current(s, "plan.wall")?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let len = a.dist(b);
    if len < 1.0 {
        return Err(bad("plan.wall", "wall too short (< 1 drawing unit)"));
    }
    let thickness = thickness_in / 12.0 * upf;
    let d = s.doc_mut()?;
    ensure_plan_layers(d);
    let dir = (b - a).normalized();
    let perp = Vec2::new(-dir.y, dir.x);
    let half_t = thickness / 2.0;
    let height = height_ft * upf;
    // 3D mesh: a box along the wall axis.
    let corners = [a + perp * half_t, b + perp * half_t, b - perp * half_t, a - perp * half_t];
    let mut vertices = Vec::new();
    for c in &corners {
        vertices.push(Vec3::new(c.x, c.y, 0.0));
    }
    for c in &corners {
        vertices.push(Vec3::new(c.x, c.y, height));
    }
    let faces = vec![
        cadaddy_doc::MeshFace { v: vec![0, 1, 2, 3], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![7, 6, 5, 4], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![0, 4, 5, 1], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![2, 6, 7, 3], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![0, 3, 7, 4], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![1, 5, 6, 2], hidden: 0 },
    ];
    let mesh = cadaddy_doc::Mesh { vertices, faces, kind: cadaddy_doc::MeshKind::Polyface };
    // WALLGEN + type tag + properties.
    let mut xdata_tags = super::wallgen::tag(cadaddy_walk3d::Part::Wall, &level, Handle(0));
    xdata_tags.push(RawTag { code: 1000, value: format!("type:{wall_type}") });
    let mut store = Props::default();
    store.set("type", Prop::new(Value::String(wall_type.clone()), Source::user()));
    store.set("height", Prop::new(json!(height_ft), Source::user()).with_unit("ft"));
    store.set("assembly", Prop::new(Value::String(format!("{thickness_in}\" wall")), Source::user()));
    store.set("level", Prop::new(Value::String(level.clone()), Source::user()));
    let xdata_with_props = props::write(&xdata_tags, &store);
    let common_3d =
        Common { layer: super::wallgen::layer_of(cadaddy_walk3d::Part::Wall).into(), xdata: xdata_with_props.into(), ..Common::default() };
    let model_handle = d.add(&Space::Model, common_3d, EntityKind::Mesh(mesh)).map_err(|e| bad("plan.wall", e.to_string()))?;
    // Plan representation.
    let mut plan_handles = Vec::new();
    let plan_common = Common { layer: "A-WALL".into(), xdata: plan_tag("wall", &level, model_handle).into(), ..Common::default() };
    if let Ok(h) =
        d.add(&Space::Model, plan_common.clone(), EntityKind::Line(cadaddy_doc::Line { a: v3(a + perp * half_t), b: v3(b + perp * half_t) }))
    {
        plan_handles.push(h.hex());
    }
    if let Ok(h) = d.add(&Space::Model, plan_common, EntityKind::Line(cadaddy_doc::Line { a: v3(a - perp * half_t), b: v3(b - perp * half_t) })) {
        plan_handles.push(h.hex());
    }
    s.echo(format!("Wall created: {:.1}' long, {thickness_in}\" thick, {height_ft}' high.", len / upf));
    Ok(json!({ "handle": model_handle.hex(), "plan_handles": plan_handles, "model_handle": model_handle.hex() }))
}

// ─── plan.door ───────────────────────────────────────────────────────────────

fn run_door(s: &mut Session, p: &Value) -> Result<Value> {
    let wall_hex = str_param(p, "wall").ok_or_else(|| bad("plan.door", "`wall` handle is required"))?;
    let wall_h = Handle(u64::from_str_radix(wall_hex.trim_start_matches("0x"), 16).map_err(|_| bad("plan.door", "invalid wall handle"))?);
    let width_in = p.get("width").and_then(Value::as_f64).unwrap_or(36.0);
    let height_ft = p.get("height").and_then(Value::as_f64).unwrap_or(6.67);
    let station = p.get("position").and_then(Value::as_f64).unwrap_or(0.5);
    let swing = str_param(p, "swing").unwrap_or("left");
    let (sp, _) = super::bspec::current(s, "plan.door")?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let d = s.doc_mut()?;
    let wall_ent = d.model.get(wall_h).ok_or_else(|| bad("plan.door", "wall entity not found"))?;
    let level = wallgen_level(wall_ent).unwrap_or_else(|| "Level 1".into());
    let EntityKind::Mesh(ref mesh) = wall_ent.kind else {
        return Err(bad("plan.door", "wall is not a mesh"));
    };
    let pts: Vec<Vec2> = mesh.vertices.iter().map(|v| Vec2::new(v.x, v.y)).collect();
    let (min_x, max_x) = minmax_x(&pts);
    let (min_y, max_y) = minmax_y(&pts);
    let dx = max_x - min_x;
    let dy = max_y - min_y;
    let (wall_a, wall_b, wall_perp) = if dx >= dy {
        let cy = (min_y + max_y) / 2.0;
        (Vec2::new(min_x, cy), Vec2::new(max_x, cy), Vec2::new(0.0, 1.0))
    } else {
        let cx = (min_x + max_x) / 2.0;
        (Vec2::new(cx, min_y), Vec2::new(cx, max_y), Vec2::new(1.0, 0.0))
    };
    let width = width_in / 12.0 * upf;
    let station_clamped = station.clamp(0.0, 1.0);
    let door_center = wall_a + (wall_b - wall_a) * station_clamped;
    let wall_dir = (wall_b - wall_a).normalized();
    let hinge = door_center - wall_dir * (width / 2.0);
    let open_end = door_center + wall_dir * (width / 2.0);
    // 3D door leaf mesh.
    let leaf_t = 1.75 / 12.0 * upf;
    let door_height = height_ft * upf;
    let half_leaf = leaf_t / 2.0;
    let corners = [hinge + wall_perp * half_leaf, open_end + wall_perp * half_leaf, open_end - wall_perp * half_leaf, hinge - wall_perp * half_leaf];
    let mut vertices = Vec::new();
    for c in &corners {
        vertices.push(Vec3::new(c.x, c.y, 0.0));
    }
    for c in &corners {
        vertices.push(Vec3::new(c.x, c.y, door_height));
    }
    let faces = vec![
        cadaddy_doc::MeshFace { v: vec![0, 1, 2, 3], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![7, 6, 5, 4], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![0, 4, 5, 1], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![2, 6, 7, 3], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![0, 3, 7, 4], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![1, 5, 6, 2], hidden: 0 },
    ];
    let mesh = cadaddy_doc::Mesh { vertices, faces, kind: cadaddy_doc::MeshKind::Polyface };
    let wg_tags = super::wallgen::tag(cadaddy_walk3d::Part::Door, &level, wall_h);
    let mut store = Props::default();
    store.set("width", Prop::new(json!(width_in), Source::user()).with_unit("in"));
    store.set("height", Prop::new(json!(height_ft), Source::user()).with_unit("ft"));
    store.set("swing", Prop::new(Value::String(swing.to_string()), Source::user()));
    store.set("level", Prop::new(Value::String(level.clone()), Source::user()));
    let xdata_with_props = props::write(&wg_tags, &store);
    let common_3d =
        Common { layer: super::wallgen::layer_of(cadaddy_walk3d::Part::Door).into(), xdata: xdata_with_props.into(), ..Common::default() };
    let door_handle = d.add(&Space::Model, common_3d, EntityKind::Mesh(mesh)).map_err(|e| bad("plan.door", e.to_string()))?;
    ensure_plan_layers(d);
    let plan_common = Common { layer: "A-DOOR".into(), xdata: plan_tag("door", &level, door_handle).into(), ..Common::default() };
    let mut plan_handles = Vec::new();
    if let Ok(h) = d.add(&Space::Model, plan_common.clone(), EntityKind::Line(cadaddy_doc::Line { a: v3(hinge), b: v3(open_end) })) {
        plan_handles.push(h.hex());
    }
    let swing_side = if swing == "right" { -1.0 } else { 1.0 };
    let start_angle = (open_end.y - hinge.y).atan2(open_end.x - hinge.x);
    let arc_end = start_angle + swing_side * FRAC_PI_2;
    let (sa, ea) = if arc_end < start_angle { (arc_end, start_angle) } else { (start_angle, arc_end) };
    if let Ok(h) = d.add(&Space::Model, plan_common, EntityKind::Arc(cadaddy_doc::Arc { center: v3(hinge), radius: width, start: sa, end: ea })) {
        plan_handles.push(h.hex());
    }
    s.echo(format!("Door placed: {width_in}\" wide, {height_ft}' high, {swing} swing."));
    Ok(json!({ "handle": door_handle.hex(), "plan_handles": plan_handles }))
}

// ─── plan.window ─────────────────────────────────────────────────────────────

fn run_window(s: &mut Session, p: &Value) -> Result<Value> {
    let wall_hex = str_param(p, "wall").ok_or_else(|| bad("plan.window", "`wall` handle is required"))?;
    let wall_h = Handle(u64::from_str_radix(wall_hex.trim_start_matches("0x"), 16).map_err(|_| bad("plan.window", "invalid wall handle"))?);
    let width_in = p.get("width").and_then(Value::as_f64).unwrap_or(36.0);
    let sill_ft = p.get("sill").and_then(Value::as_f64).unwrap_or(3.0);
    let height_ft = p.get("height").and_then(Value::as_f64).unwrap_or(4.0);
    let station = p.get("position").and_then(Value::as_f64).unwrap_or(0.5);
    let (sp, _) = super::bspec::current(s, "plan.window")?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let d = s.doc_mut()?;
    let wall_ent = d.model.get(wall_h).ok_or_else(|| bad("plan.window", "wall entity not found"))?;
    let level = wallgen_level(wall_ent).unwrap_or_else(|| "Level 1".into());
    let EntityKind::Mesh(ref mesh) = wall_ent.kind else {
        return Err(bad("plan.window", "wall is not a mesh"));
    };
    let pts: Vec<Vec2> = mesh.vertices.iter().map(|v| Vec2::new(v.x, v.y)).collect();
    let (min_x, max_x) = minmax_x(&pts);
    let (min_y, max_y) = minmax_y(&pts);
    let dx = max_x - min_x;
    let dy = max_y - min_y;
    let (wall_a, wall_b, wall_perp) = if dx >= dy {
        let cy = (min_y + max_y) / 2.0;
        (Vec2::new(min_x, cy), Vec2::new(max_x, cy), Vec2::new(0.0, 1.0))
    } else {
        let cx = (min_x + max_x) / 2.0;
        (Vec2::new(cx, min_y), Vec2::new(cx, max_y), Vec2::new(1.0, 0.0))
    };
    let width = width_in / 12.0 * upf;
    let station_clamped = station.clamp(0.0, 1.0);
    let wall_dir = (wall_b - wall_a).normalized();
    let win_center = wall_a + (wall_b - wall_a) * station_clamped;
    let win_a = win_center - wall_dir * (width / 2.0);
    let win_b = win_center + wall_dir * (width / 2.0);
    // 3D glass pane.
    let glass_t = 0.5 / 12.0 * upf;
    let half_glass = glass_t / 2.0;
    let sill_z = sill_ft * upf;
    let head_z = (sill_ft + height_ft) * upf;
    let corners = [win_a + wall_perp * half_glass, win_b + wall_perp * half_glass, win_b - wall_perp * half_glass, win_a - wall_perp * half_glass];
    let mut vertices = Vec::new();
    for c in &corners {
        vertices.push(Vec3::new(c.x, c.y, sill_z));
    }
    for c in &corners {
        vertices.push(Vec3::new(c.x, c.y, head_z));
    }
    let faces = vec![
        cadaddy_doc::MeshFace { v: vec![0, 1, 2, 3], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![7, 6, 5, 4], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![0, 4, 5, 1], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![2, 6, 7, 3], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![0, 3, 7, 4], hidden: 0 },
        cadaddy_doc::MeshFace { v: vec![1, 5, 6, 2], hidden: 0 },
    ];
    let mesh = cadaddy_doc::Mesh { vertices, faces, kind: cadaddy_doc::MeshKind::Polyface };
    let wg_tags = super::wallgen::tag(cadaddy_walk3d::Part::Glass, &level, wall_h);
    let mut store = Props::default();
    store.set("width", Prop::new(json!(width_in), Source::user()).with_unit("in"));
    store.set("sill_height", Prop::new(json!(sill_ft), Source::user()).with_unit("ft"));
    store.set("height", Prop::new(json!(height_ft), Source::user()).with_unit("ft"));
    store.set("level", Prop::new(Value::String(level.clone()), Source::user()));
    let xdata_with_props = props::write(&wg_tags, &store);
    let common_3d =
        Common { layer: super::wallgen::layer_of(cadaddy_walk3d::Part::Glass).into(), xdata: xdata_with_props.into(), ..Common::default() };
    let win_handle = d.add(&Space::Model, common_3d, EntityKind::Mesh(mesh)).map_err(|e| bad("plan.window", e.to_string()))?;
    ensure_plan_layers(d);
    let glass_offset = 0.5;
    let plan_common = Common { layer: "A-GLAZ".into(), xdata: plan_tag("window", &level, win_handle).into(), ..Common::default() };
    let mut plan_handles = Vec::new();
    if let Ok(h) = d.add(
        &Space::Model,
        plan_common.clone(),
        EntityKind::Line(cadaddy_doc::Line { a: v3(win_a + wall_perp * glass_offset), b: v3(win_b + wall_perp * glass_offset) }),
    ) {
        plan_handles.push(h.hex());
    }
    if let Ok(h) = d.add(
        &Space::Model,
        plan_common,
        EntityKind::Line(cadaddy_doc::Line { a: v3(win_a - wall_perp * glass_offset), b: v3(win_b - wall_perp * glass_offset) }),
    ) {
        plan_handles.push(h.hex());
    }
    s.echo(format!("Window placed: {width_in}\" wide, sill at {sill_ft}', {height_ft}' high."));
    Ok(json!({ "handle": win_handle.hex(), "plan_handles": plan_handles }))
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn v3(p: Vec2) -> Vec3 {
    Vec3::new(p.x, p.y, 0.0)
}

fn point_param(p: &Value, key: &str) -> Option<Vec2> {
    let arr = p.get(key)?.as_array()?;
    Some(Vec2::new(arr.first()?.as_f64()?, arr.get(1)?.as_f64()?))
}

fn minmax_x(pts: &[Vec2]) -> (f64, f64) {
    pts.iter().map(|p| p.x).fold((f64::MAX, f64::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)))
}

fn minmax_y(pts: &[Vec2]) -> (f64, f64) {
    pts.iter().map(|p| p.y).fold((f64::MAX, f64::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)))
}

// ─── Interactive machines ────────────────────────────────────────────────────

/// Interactive `plan.wall`: polyline-style wall chain.
///
/// Prompts: "Wall type <current>:" (Enter keeps), "Specify start point:",
/// "Specify next point or [Undo/Close]:" repeating. Esc ends.
/// Also accepts full JSON params from CLI/MCP through the `run_wall` path.
#[derive(Default)]
struct WallM {
    wall_type: String,
    height_ft: f64,
    pts: Vec<Vec2>,
    handles: Vec<Handle>,
    asked_type: bool,
}

impl Interactive for WallM {
    fn name(&self) -> &'static str {
        "PLAN.WALL"
    }

    fn begin(&mut self, _s: &mut Session) -> Result<Step> {
        self.wall_type = "Standard".into();
        self.height_ft = 9.0;
        self.asked_type = false;
        Ok(Step::Continue)
    }

    fn prompt(&self, _s: &Session) -> Prompt {
        if !self.asked_type {
            return Prompt::new(format!("Wall type <{}>", self.wall_type), Accept::TEXT).default(&self.wall_type);
        }
        match self.pts.len() {
            0 => Prompt::new("Specify start point", Accept::POINT),
            1 => Prompt::new("Specify next point", Accept::POINT).kw(&["Undo", "Height", "Justification"]).base_opt(self.pts.last().copied()),
            _ => {
                Prompt::new("Specify next point", Accept::POINT).kw(&["Undo", "Close", "Height", "Justification"]).base_opt(self.pts.last().copied())
            }
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if !self.asked_type {
            match i {
                Input::Enter => {
                    self.asked_type = true;
                    return Ok(Step::Continue);
                }
                Input::Text(t) => {
                    let t = t.trim().to_string();
                    if !t.is_empty() {
                        self.wall_type = t;
                    }
                    self.asked_type = true;
                    return Ok(Step::Continue);
                }
                _ => {
                    self.asked_type = true;
                    return Ok(Step::Continue);
                }
            }
        }
        match i {
            Input::Point(p) => {
                if let Some(&prev) = self.pts.last() {
                    if prev.dist(p) < 1.0 {
                        return Ok(Step::Continue);
                    }
                    let params = json!({
                        "a": [prev.x, prev.y],
                        "b": [p.x, p.y],
                        "type": self.wall_type,
                        "height": self.height_ft,
                    });
                    let result = run_wall(s, &params)?;
                    if let Some(hex) = result.get("handle").and_then(Value::as_str) {
                        if let Ok(v) = u64::from_str_radix(hex.trim_start_matches("0x"), 16) {
                            self.handles.push(Handle(v));
                        }
                    }
                }
                self.pts.push(p);
                Ok(Step::Continue)
            }
            Input::Keyword(ref k) if k == "Undo" => {
                if let Some(h) = self.handles.pop() {
                    let d = s.doc_mut()?;
                    let linked: Vec<Handle> = d.model.iter().filter(|e| plan_of(e).map_or(false, |(_, _, src)| src == h)).map(|e| e.handle).collect();
                    for lh in linked {
                        d.model.remove(lh);
                    }
                    d.model.remove(h);
                    self.pts.pop();
                }
                Ok(Step::Continue)
            }
            Input::Keyword(ref k) if k == "Close" && self.pts.len() >= 3 => {
                let first = self.pts[0];
                let last = *self.pts.last().unwrap_or(&first);
                if first.dist(last) >= 1.0 {
                    let params = json!({
                        "a": [last.x, last.y],
                        "b": [first.x, first.y],
                        "type": self.wall_type,
                        "height": self.height_ft,
                    });
                    let _ = run_wall(s, &params);
                }
                Ok(Step::Done)
            }
            Input::Keyword(ref k) if k == "Height" => {
                s.echo(format!("Current height: {}'. Enter new height:", self.height_ft));
                Ok(Step::Continue)
            }
            Input::Text(ref t) => {
                if let Some(v) = super::machines::number(t) {
                    if v > 0.0 {
                        self.height_ft = v;
                        s.echo(format!("Wall height set to {v}'."));
                    }
                }
                Ok(Step::Continue)
            }
            Input::Enter if !self.pts.is_empty() => Ok(Step::Done),
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }

    fn preview(&self, _s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        let Some(&last) = self.pts.last() else {
            return Vec::new();
        };
        if !self.asked_type {
            return Vec::new();
        }
        vec![EntityKind::Line(cadaddy_doc::Line { a: v3(last), b: v3(cursor) })]
    }
}

/// Interactive `plan.door` / `plan.window`: pick a wall, then pick position along it.
struct DoorWindowM {
    cmd: &'static str,
    wall: Option<Handle>,
}

impl DoorWindowM {
    fn new(cmd: &'static str) -> Self {
        Self { cmd, wall: None }
    }
}

impl Interactive for DoorWindowM {
    fn name(&self) -> &'static str {
        match self.cmd {
            "plan.door" => "PLAN.DOOR",
            _ => "PLAN.WINDOW",
        }
    }

    fn prompt(&self, _s: &Session) -> Prompt {
        if self.wall.is_none() {
            Prompt::new("Select host wall", Accept::POINT)
        } else {
            Prompt::new("Specify location along wall", Accept::POINT).kw(if self.cmd == "plan.door" { &["Flip swing"] } else { &[] })
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (&self.wall, i) {
            (None, Input::Pick(hs)) => {
                let h = hs.into_iter().next();
                if let Some(h) = h {
                    let d = s.doc()?;
                    if let Some(e) = d.model.get(h) {
                        if matches!(e.kind, EntityKind::Mesh(_)) && super::wallgen::made_as(e).map_or(false, |p| p.eq_ignore_ascii_case("wall")) {
                            self.wall = Some(h);
                            return Ok(Step::Continue);
                        }
                    }
                    s.echo("Not a wall. Select a wall object.".to_string());
                }
                Ok(Step::Continue)
            }
            (None, Input::Point(_)) => {
                // Point click with no pick: echo feedback so the prompt is not silent.
                s.echo("No wall at that point. Pick a wall object.".to_string());
                Ok(Step::Continue)
            }
            (None, Input::Enter) => {
                // Enter with no wall selected ends the command.
                Ok(Step::Done)
            }
            (Some(wall_h), Input::Point(p)) => {
                let d = s.doc()?;
                let wall_ent = d.model.get(*wall_h).ok_or_else(|| bad(self.cmd, "wall entity not found"))?;
                let EntityKind::Mesh(ref mesh) = wall_ent.kind else {
                    return Err(bad(self.cmd, "wall is not a mesh"));
                };
                let pts: Vec<Vec2> = mesh.vertices.iter().map(|v| Vec2::new(v.x, v.y)).collect();
                let (min_x, max_x) = minmax_x(&pts);
                let (min_y, max_y) = minmax_y(&pts);
                let dx = max_x - min_x;
                let dy = max_y - min_y;
                let (wall_a, wall_b) = if dx >= dy {
                    let cy = (min_y + max_y) / 2.0;
                    (Vec2::new(min_x, cy), Vec2::new(max_x, cy))
                } else {
                    let cx = (min_x + max_x) / 2.0;
                    (Vec2::new(cx, min_y), Vec2::new(cx, max_y))
                };
                let wall_len = wall_a.dist(wall_b);
                let station = if wall_len > 0.0 {
                    let wall_dir = (wall_b - wall_a).normalized();
                    let t = (p - wall_a).dot(wall_dir) / wall_len;
                    t.clamp(0.05, 0.95)
                } else {
                    0.5
                };
                let params = json!({
                    "wall": wall_h.hex(),
                    "position": station,
                });
                match self.cmd {
                    "plan.door" => run_door(s, &params)?,
                    _ => run_window(s, &params)?,
                };
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
}
