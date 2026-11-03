//! Field and agent work on the built model (Q5242): coordination, takeoff and estimating, each a
//! command (and so an MCP tool through `execute`) with a JSON result.
//!
//! - `bim.clash {a, b, tolerance?, clearance?, group_by?}`: the elements of one trade or category
//!   against another's, mesh against mesh ([`clash`]). Each clash has a viewpoint and a status
//!   (`new`, `active`, `resolved`); a re-run with the same `a`/`b` compares with the last run.
//!   `bim.clash.status {id, status}` sets one by hand; `bim.clash.fix {id, strategy}` proposes a
//!   move of the lower-priority element (conduit yields to duct, duct to gravity piping), applies it
//!   with `approve: true` and checks again.
//! - `bim.takeoff {group_by, filter}`: counts, lengths, areas and volumes from the `bim.bom` lines,
//!   each with its element handles, and what changed since the last takeoff ([`takeoff`]).
//!   `bim.takeoff.show {line}` selects a line's elements and frames them in 3D.
//! - `bim.estimate {cost_table}`: the takeoff priced from a cost table (CSV or XLSX: item, unit,
//!   material, labor_hrs, optional waste), with a change order against an earlier estimate.
//! - `bim.issue.*`: issues, RFIs and punch items pinned to elements, with a photo, an assignee, a
//!   status and a viewpoint, kept as BCF topics ([`super::bcf`]) and so exported and imported as
//!   BCF 2.1 or 3.0; `bim.issue.marks` puts them on the plan (the sheets) as numbered marks.
//! - `bim.field.link {at, from}`: a sheet point to a 3D view of that spot, and a 3D point back to
//!   its level and sheet point.
//!
//! What a run keeps (the last clash run per `a`/`b`, the last takeoff, saved estimates) is the
//! drawing's embedded file `bimwork.json`, so it travels in `.cdy` files.

use std::collections::BTreeMap;
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::Vec3;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

mod clash;
mod issues;
mod takeoff;

/// The embedded file the work state is kept in.
pub const STATE_FILE: &str = "bimwork.json";
/// Most model objects looked at (hostile drawings).
const MAX_ELEMENTS: usize = 500_000;

pub fn specs() -> Vec<CommandSpec> {
    let mut v = clash::specs();
    v.extend(takeoff::specs());
    v.extend(issues::specs());
    v
}

// ---------------- the state kept in the drawing ----------------

pub(crate) fn state(d: &Drawing) -> Value {
    d.embedded.get(STATE_FILE).and_then(|b| serde_json::from_slice::<Value>(b).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
}

/// Keep `val` as the state's `key` (nothing when it already holds it).
pub(crate) fn set_state(s: &mut Session, key: &str, val: Value) -> Result<()> {
    let mut st = state(s.doc()?);
    if st.get(key) == Some(&val) {
        return Ok(());
    }
    if let Some(o) = st.as_object_mut() {
        o.insert(key.into(), val);
    }
    let bytes = serde_json::to_vec(&st).map_err(|e| crate::EngineError::Other(e.to_string()))?;
    s.doc_mut()?.embedded.insert(STATE_FILE.into(), Arc::new(bytes));
    Ok(())
}

// ---------------- the model's elements ----------------

/// One element of the built model: what it is and where.
#[derive(Clone, Debug)]
pub(crate) struct El {
    pub handle: Handle,
    /// `architecture`, `structure`, `mechanical`, `plumbing`, `electrical`, `fire`, `lowvoltage`.
    pub trade: &'static str,
    /// What it is: `wall`, `door`, `ceiling`, `duct`, `pipe`, `conduit`, `device`, `fitting`…
    pub category: String,
    /// Its MEP system (or layer).
    pub system: String,
    pub level: String,
    pub lo: Vec3,
    pub hi: Vec3,
    /// A piece of a run (Q6157): the run it is part of (one clash per run, not per stick).
    pub run: String,
    /// Set in a wall as its trade sets it there: a box, its ring, a drop in the wall's cavity.
    pub in_wall: bool,
}

impl El {
    pub fn centre(&self) -> Vec3 {
        (self.lo + self.hi) * 0.5
    }
    pub fn json(&self) -> Value {
        json!({"handle": self.handle.hex(), "trade": self.trade, "category": self.category, "system": self.system, "level": self.level})
    }
}

/// The trade names `trade_of` gives, as this module names them.
fn long_trade(short: &str) -> &'static str {
    match short {
        "arch" => "architecture",
        "struct" => "structure",
        "mech" => "mechanical",
        "plumb" => "plumbing",
        "elec" => "electrical",
        "fire" => "fire",
        "lowvolt" => "lowvoltage",
        _ => "other",
    }
}

/// An IFC class's trade and category.
fn ifc_class(class: &str) -> Option<(&'static str, String)> {
    let c = class.strip_prefix("Ifc").unwrap_or(class);
    let cat = c.trim_end_matches("StandardCase").to_ascii_lowercase();
    let t = match c {
        _ if c.starts_with("Wall") || c.starts_with("Door") || c.starts_with("Window") || c.starts_with("Covering") || c.starts_with("Stair") => {
            "architecture"
        }
        _ if c.starts_with("Roof") || c.starts_with("Railing") || c.starts_with("CurtainWall") || c.starts_with("Furnishing") => "architecture",
        _ if c.starts_with("Slab") || c.starts_with("Column") || c.starts_with("Beam") || c.starts_with("Footing") || c.starts_with("Member") => {
            "structure"
        }
        _ if c.starts_with("Pile") || c.starts_with("Plate") => "structure",
        _ if c.starts_with("Duct") || c.starts_with("AirTerminal") || c.starts_with("Damper") || c.starts_with("Fan") => "mechanical",
        _ if c.starts_with("Pipe") || c.starts_with("SanitaryTerminal") || c.starts_with("Valve") || c.starts_with("Pump") => "plumbing",
        _ if c.starts_with("FireSuppression") || c.starts_with("Alarm") => "fire",
        _ if c.starts_with("Cable") || c.starts_with("LightFixture") || c.starts_with("Outlet") || c.starts_with("Switching") => "electrical",
        _ if c.starts_with("ElectricDistribution") || c.starts_with("Junction") || c.starts_with("Lamp") => "electrical",
        _ => return None,
    };
    Some((t, cat))
}

/// What an object of the model is, if it is one of the building's elements.
pub(crate) fn classify(e: &Entity) -> Option<(&'static str, String, String, String)> {
    let tags = super::fieldview::wallgen_tags(e);
    let layer = e.common.layer.to_string();
    if let (Some(part), Some(level)) = (tags.first().copied(), tags.get(1).copied()) {
        let level = level.to_string();
        return Some(match part {
            "wall" | "door" | "window" | "floor" | "ceiling" | "stair" | "roof" | "room" | "railing" | "casework" | "furniture" => {
                ("architecture", part.to_string(), layer, level)
            }
            "mep" => {
                let kind = super::fieldview::tag_value(&tags, "kind").unwrap_or("mep");
                let system = super::fieldview::tag_value(&tags, "system").unwrap_or_default().to_string();
                let p = super::fieldview::tag_value(&tags, "part").unwrap_or_default();
                // A piece of a run (Q6157): sticks are its conduit, the rest its fittings, boxes,
                // supports, cable and wire.
                let piece = p.strip_prefix("atom:").map(|k| match k {
                    "conduit" => "conduit",
                    "box" | "ring" => "box",
                    "strap" => "support",
                    "cable" => "cable",
                    "wire" => "wire",
                    _ => "fitting",
                });
                let cat = if let Some(piece) = piece {
                    piece
                } else if p.starts_with("fitting") {
                    "fitting"
                } else if p.starts_with("terminal") {
                    "terminal"
                } else {
                    kind
                };
                // Electrical gear scheduled as equipment (a room's load center, a panelboard) is
                // the electrician's.
                let gear = p.strip_prefix("equipment:").is_some_and(|k| {
                    matches!(
                        k.to_ascii_lowercase().as_str(),
                        "panel" | "panelboard" | "load_center" | "switchboard" | "switchgear" | "transformer" | "disconnect"
                    )
                });
                let t = match kind {
                    "tray" | "cable" | "grounding" => "electrical",
                    _ if piece.is_some() => "electrical",
                    "equipment" if gear => "electrical",
                    _ => long_trade(cadaddy_walk3d::field::trade_of(kind, &system)),
                };
                (t, cat.to_string(), system, level)
            }
            "device" => (long_trade(cadaddy_walk3d::field::trade_of("device", &layer)), "device".into(), layer, level),
            other => {
                let kind = super::fieldview::tag_value(&tags, "kind").unwrap_or(other);
                (long_trade(cadaddy_walk3d::field::trade_of(kind, &layer)), other.to_string(), layer, level)
            }
        });
    }
    if let Some(t) = super::ifcref::tag(e)
        && let Some((trade, cat)) = ifc_class(&t.class)
    {
        return Some((trade, cat, t.systems.first().cloned().unwrap_or(layer), t.level.unwrap_or_default()));
    }
    // Untagged 3D objects on a discipline layer (`M-…`, `E-…`).
    let up = layer.to_ascii_uppercase();
    let disc = up.split(['-', '_']).next().unwrap_or_default();
    let trade = match disc {
        "A" | "I" => "architecture",
        "S" => "structure",
        "M" | "H" => "mechanical",
        "P" => "plumbing",
        "E" => "electrical",
        "F" | "FP" | "FA" => "fire",
        "T" | "LV" => "lowvoltage",
        _ => return None,
    };
    matches!(e.kind, EntityKind::Mesh(_)).then(|| (trade, "object".into(), layer, String::new()))
}

/// The model's elements (3D meshes it knows the trade of), with their boxes.
///
/// Built elements (tagged by WALLGEN) are appended after the imported 2D plan entities
/// (potentially 800K+), so the cap applies to classified elements, not raw entities.
pub(crate) fn elements(d: &Drawing) -> Vec<El> {
    d.model
        .iter()
        .filter_map(|e| {
            let (lo, hi) = super::fieldview::mesh_box(e)?;
            let (trade, category, system, level) = classify(e)?;
            let (run, in_wall) = if super::mepatoms::is_atom(e) {
                let ps = cadaddy_doc::props::read(&e.common.xdata);
                let t = |k: &str| ps.get(k).map(cadaddy_doc::props::Prop::text).unwrap_or_default();
                (t("run"), matches!(t("atom").as_str(), "box" | "ring") || t("vertical") == "true")
            } else {
                (String::new(), false)
            };
            Some(El {
                handle: e.handle,
                trade,
                category,
                system,
                level,
                lo: Vec3::new(lo[0], lo[1], lo[2]),
                hi: Vec3::new(hi[0], hi[1], hi[2]),
                run,
                in_wall,
            })
        })
        .take(MAX_ELEMENTS)
        .collect()
}

/// Does `sel` (a trade, a category or a system, `*` for any) pick `e`?
pub(crate) fn picks(sel: &[String], e: &El) -> bool {
    sel.iter().any(|s| {
        let s = s.trim().to_ascii_lowercase();
        let trade = match s.as_str() {
            "*" | "all" | "any" => return true,
            "a" | "arch" | "architecture" | "architectural" => Some("architecture"),
            "s" | "struct" | "structure" | "structural" => Some("structure"),
            "m" | "mech" | "mechanical" | "hvac" => Some("mechanical"),
            "p" | "plumb" | "plumbing" | "piping" => Some("plumbing"),
            "e" | "elec" | "electrical" | "power" | "lighting" => Some("electrical"),
            "f" | "fp" | "fire" | "fire protection" | "sprinkler" | "sprinklers" => Some("fire"),
            "t" | "lv" | "lowvolt" | "low voltage" | "lowvoltage" | "telecom" => Some("lowvoltage"),
            _ => None,
        };
        match trade {
            Some(t) => e.trade == t,
            None => {
                let sing = s.strip_suffix('s').filter(|x| x.len() > 2).unwrap_or(&s);
                e.category.eq_ignore_ascii_case(&s)
                    || e.category.eq_ignore_ascii_case(sing)
                    || e.system.eq_ignore_ascii_case(&s)
                    || s.strip_suffix('*').is_some_and(|pre| e.system.to_ascii_lowercase().starts_with(pre))
            }
        }
    })
}

/// A selector param: a name, a comma-separated list, or a list of names.
pub(crate) fn selector(p: &Value, key: &str) -> Option<Vec<String>> {
    match p.get(key)? {
        Value::String(s) => Some(s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()),
        Value::Array(a) => Some(a.iter().filter_map(Value::as_str).take(64).map(str::to_string).collect()),
        _ => None,
    }
    .filter(|v: &Vec<String>| !v.is_empty())
}

/// Drawing units per foot (the building spec's, else inches).
pub(crate) fn units_per_foot(s: &Session) -> f64 {
    super::bspec::current(s, "bim").map(|(sp, _)| sp.units_per_foot).ok().filter(|u| u.is_finite() && *u > 0.0).unwrap_or(12.0)
}

/// A camera looking at `target` from the south-west, above, `dist` away.
pub(crate) fn viewpoint(target: Vec3, dist: f64) -> Value {
    let back = Vec3::new(-1.0, -1.0, 0.8);
    let l = back.len().max(1e-9);
    let eye = target + back * (dist / l);
    json!({"eye": [r3(eye.x), r3(eye.y), r3(eye.z)], "target": [r3(target.x), r3(target.y), r3(target.z)]})
}

pub(crate) fn r3(v: f64) -> f64 {
    let r = (v * 1000.0).round() / 1000.0;
    if r == 0.0 { 0.0 } else { r }
}

/// Look at `handles` in 3D: selected (the renderers mark the selection) and framed by the camera.
pub(crate) fn frame(s: &mut Session, handles: &[Handle]) -> Result<Value> {
    let d = s.doc()?;
    let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut found = Vec::new();
    for h in handles.iter().take(100_000) {
        let Some(e) = d.entity(*h) else { continue };
        found.push(*h);
        if let Some((a, b)) = super::fieldview::mesh_box(e) {
            lo = Vec3::new(lo.x.min(a[0]), lo.y.min(a[1]), lo.z.min(a[2]));
            hi = Vec3::new(hi.x.max(b[0]), hi.y.max(b[1]), hi.z.max(b[2]));
        }
    }
    let mut out = json!({"selected": found.len(), "handles": found.iter().map(|h| h.hex()).collect::<Vec<_>>()});
    if lo.x.is_finite() && hi.x.is_finite() {
        let upf = units_per_foot(s);
        let target = (lo + hi) * 0.5;
        let vp = viewpoint(target, ((hi - lo).len() * 1.5).max(10.0 * upf));
        if let (Some(e), Some(t)) = (vec3_of(&vp["eye"]), vec3_of(&vp["target"])) {
            s.state_mut()?.ifc_view.camera = Some((e, t));
        }
        out["min"] = json!([r3(lo.x), r3(lo.y), r3(lo.z)]);
        out["max"] = json!([r3(hi.x), r3(hi.y), r3(hi.z)]);
        out["viewpoint"] = vp;
    }
    s.set_selection(found);
    Ok(out)
}

pub(crate) fn vec3_of(v: &Value) -> Option<Vec3> {
    let a = v.as_array()?;
    let n = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite());
    Some(Vec3::new(n(0)?, n(1)?, n(2).unwrap_or(0.0)))
}

/// Handles from a list of hex strings.
pub(crate) fn handles_of(v: &Value) -> Vec<Handle> {
    v.as_array().map(|a| a.iter().filter_map(|h| h.as_str().and_then(Handle::parse_hex)).take(100_000).collect()).unwrap_or_default()
}

/// Lines of the same shape keyed by `key` → (added, removed, changed) against `before`.
pub(crate) fn diff(before: &BTreeMap<String, f64>, after: &BTreeMap<String, f64>) -> Value {
    let added: Vec<Value> = after.iter().filter(|(k, _)| !before.contains_key(*k)).map(|(k, q)| json!({"key": k, "qty": r3(*q)})).collect();
    let removed: Vec<Value> = before.iter().filter(|(k, _)| !after.contains_key(*k)).map(|(k, q)| json!({"key": k, "qty": r3(*q)})).collect();
    let changed: Vec<Value> = after
        .iter()
        .filter_map(|(k, q)| {
            let b = before.get(k)?;
            ((q - b).abs() > 1e-6 * q.abs().max(1.0)).then(|| json!({"key": k, "before": r3(*b), "after": r3(*q), "delta": r3(q - b)}))
        })
        .collect();
    json!({"added": added, "removed": removed, "changed": changed})
}
