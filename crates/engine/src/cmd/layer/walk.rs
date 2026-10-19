//! LAYWALK (show only some layers for a while, then put them back) and LAYVPI (isolate layers to
//! the current layout viewport).

use std::collections::HashSet;

use cadaddy_doc::{EntityKind, Handle, Layer, Space};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Header key of the layers' On/Freeze state before LAYWALK started (JSON).
const WALK_KEY: &str = "CADADDY_LAYWALK";

/// The layer names matching `pats` (names or `*`/`?` wildcards, case-insensitive), in table order.
fn matching(d: &cadaddy_doc::Drawing, pats: &[String]) -> Vec<String> {
    d.layers
        .iter()
        .filter(|l| pats.iter().any(|p| crate::cmd::qselect::wildcard(&p.to_lowercase(), &l.name.to_lowercase())))
        .map(|l| l.name.clone())
        .collect()
}

fn name_list(p: &Value, key: &str) -> Vec<String> {
    match p.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).take(10_000).map(str::to_string).collect(),
        Some(Value::String(t)) => t.split(',').map(str::trim).filter(|n| !n.is_empty()).take(10_000).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// `laywalk`: `{show}` turns every other layer off (the first call remembers how they were);
/// `{end: true}` stops, putting On/Freeze back unless `restore` is false.
pub fn run_laywalk(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "laywalk";
    let d = s.doc_mut()?;
    let walking = !d.header.str(WALK_KEY, "").is_empty();
    if crate::cmd::bool_or(p, "end", false) {
        if !walking {
            return Ok(json!({ "walking": false, "restored": false }));
        }
        let restore = crate::cmd::bool_or(p, "restore", true);
        if restore {
            let snap: Vec<(String, bool, bool)> = serde_json::from_str(&d.header.str(WALK_KEY, "")).unwrap_or_default();
            for (name, on, frozen) in snap {
                if let Some(l) = d.layer_mut(&name) {
                    l.on = on;
                    l.frozen = frozen;
                }
            }
        }
        d.header.set_str(WALK_KEY, "");
        return Ok(json!({ "walking": false, "restored": restore }));
    }
    let pats = name_list(p, "show");
    if pats.is_empty() {
        let shown: Vec<&str> = d.layers.iter().filter(|l| l.on && !l.frozen).map(|l| l.name.as_str()).collect();
        return Ok(json!({ "walking": walking, "shown": shown }));
    }
    let show = matching(d, &pats);
    if show.is_empty() {
        return Err(bad(cmd, format!("no layer matches \"{}\"", pats.join(","))));
    }
    if !walking {
        let snap: Vec<(String, bool, bool)> = d.layers.iter().map(|l| (l.name.clone(), l.on, l.frozen)).collect();
        d.header.set_str(WALK_KEY, &serde_json::to_string(&snap).unwrap_or_default());
    }
    let keep: HashSet<String> = show.iter().map(|n| n.to_ascii_lowercase()).collect();
    for l in &mut d.layers {
        let on = keep.contains(&l.name.to_ascii_lowercase());
        l.on = on;
        if on {
            l.frozen = false;
        }
    }
    let n = show.len();
    Ok(json!({ "walking": true, "shown": show, "message": format!("{n} layer{} shown.", if n == 1 { "" } else { "s" }) }))
}

/// `layvpi`: freeze the layers of the selected objects (or `layers`) in every other viewport of
/// this layout (`allLayouts`: of every layout), and thaw them in the current one.
pub fn run_layvpi(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "layvpi";
    let mut layers = name_list(p, "layers");
    if layers.is_empty() {
        layers = obj_layers(s, p)?;
    }
    if layers.is_empty() {
        return Err(bad(cmd, "select objects on the layers to isolate (or pass `layers`)"));
    }
    let target: Handle = match p.get("viewport").and_then(Value::as_str) {
        Some(t) => Handle::parse_hex(t).ok_or_else(|| bad(cmd, "`viewport` is a handle"))?,
        None => s
            .state()?
            .active_viewport()
            .map(|(h, _)| h)
            .ok_or_else(|| bad(cmd, "LAYVPI works inside a layout viewport (MSPACE) or with `viewport`"))?,
    };
    let all = crate::cmd::bool_or(p, "allLayouts", false);
    let here = match s.space() {
        Space::Paper(n) => Some(n),
        Space::Model => None,
    };
    let d = s.doc_mut()?;
    for n in &layers {
        if d.layer(n).is_none() {
            return Err(bad(cmd, format!("no layer named \"{n}\"")));
        }
    }
    let names: Vec<String> = layers.iter().filter_map(|n| d.layer(n).map(|l: &Layer| l.name.clone())).collect();
    let in_target =
        d.layouts.iter().any(|l| l.entities.iter().any(|e| e.handle == target && matches!(&e.kind, EntityKind::Viewport(v) if v.id != 1)));
    if !in_target {
        return Err(bad(cmd, format!("{} is not a layout viewport", target.hex())));
    }
    let mut vps: Vec<Handle> = Vec::new();
    for l in &d.layouts {
        if !all && here.as_deref().is_some_and(|h| !h.eq_ignore_ascii_case(&l.name)) {
            continue;
        }
        if !all && here.is_none() && !l.entities.iter().any(|e| e.handle == target) {
            continue;
        }
        vps.extend(l.entities.iter().filter(|e| matches!(&e.kind, EntityKind::Viewport(v) if v.id != 1)).map(|e| e.handle));
    }
    let mut frozen_in = 0;
    for h in vps {
        let isolate = h == target;
        d.modify_entity(h, |e| {
            if let EntityKind::Viewport(v) = &mut e.kind {
                if isolate {
                    v.frozen_layers.retain(|f| !names.iter().any(|n| n.eq_ignore_ascii_case(f)));
                } else {
                    for n in &names {
                        if !v.frozen_layers.iter().any(|f| f.eq_ignore_ascii_case(n)) {
                            v.frozen_layers.push(n.clone());
                        }
                    }
                }
            }
        })
        .map_err(|e| bad(cmd, e.to_string()))?;
        if !isolate {
            frozen_in += 1;
        }
    }
    let msg = format!("Layer(s) {} isolated to viewport {} ({frozen_in} other viewport(s)).", names.join(", "), target.hex());
    Ok(json!({ "layers": names, "viewport": target.hex(), "frozenIn": frozen_in, "message": msg }))
}
