//! Revisions of the 3D model when the plans change (`model.revise`).
//!
//! Every build of the model (WALLGEN, `model.build`, and the rebuild an associative model does
//! when the plan changes) records two things in the drawing's state: the plan it was built from —
//! its walls (lines and polylines on wall layers), doors and windows (door and window blocks,
//! door swings) and rooms (name labels in closed outlines), each with a signature of its geometry
//! — and a fingerprint of every object the build made.
//!
//! `model.revise` compares the plan now (or another version of it, `against`: a file) with the one
//! last built: what was added, removed and moved. Then it builds again, and puts back the edits a
//! person made to the model since: a generated object changed or deleted by hand stays as they left
//! it when the plan object it came from didn't change; when that plan object changed too, the edit
//! conflicts — the rebuilt object wins and the edit is listed. The rebuilt objects of changed plan
//! objects are selected, so the 3D view shows what changed, and the result reports it all.

use std::collections::{HashMap, HashSet};

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::{Bounds2, Vec2};
use serde_json::{Value, json};

use super::wallgen::{self, WallGen};
use super::*;
use crate::{Result, Session};

/// Most plan objects compared.
const MAX_ITEMS: usize = 200_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("model.revise", "Revise Model from Plans", run)
        .menu(&["Draw", "Modeling", "Revise Model from Plans"])
        .params("{against?: path (an earlier or later version of the plan to compare with; default the plan the model was last built from), rebuild?: bool (default true), tolerance? (drawing units; default a millionth of the plan's size)} → {added, removed, moved: [{kind, handle, layer, name?, at, from?}], unchanged, rebuilt, kept: [handles], conflicts: [{handle, part, source, change}], changed: [handles of rebuilt objects of changed plan objects]}")]
}

fn read_file(cmd: &str, path: &str) -> Result<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::readfile::read(std::path::Path::new(path)).map_err(|e| bad(cmd, format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err(bad(cmd, format!("{path}: files can't be read in the web build")))
    }
}

/// One plan object as a revision sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanItem {
    pub kind: &'static str,
    pub handle: Handle,
    pub layer: String,
    pub name: Option<String>,
    /// Its geometry, rounded (equal geometry, equal signature).
    pub signature: String,
    pub at: Vec2,
}

/// A generated object's (part, level, source) tag.
type TagKey = (String, String, String);

/// What a build left: the plan it was built from and each generated object's fingerprint and tag.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelRecord {
    pub plan: Vec<PlanItem>,
    pub made: HashMap<Handle, (u64, Option<TagKey>)>,
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// A generated object's fingerprint: its geometry, layer and colour.
fn fingerprint(e: &Entity) -> u64 {
    fnv(&format!("{:?}|{}|{:?}", e.kind, e.common.layer, e.common.color))
}

/// Plan signatures round coordinates to this many drawing units.
const SIGNATURE_STEP: f64 = 1e-6;

fn round_to(v: f64, q: f64) -> i64 {
    (v / q).round() as i64
}

/// The plan objects of `d` a model is built from, by `g`'s rules.
pub fn plan_items(d: &Drawing, g: &WallGen) -> Vec<PlanItem> {
    // A fixed step, not one from the drawing's extents: those change with the model and with any
    // plan object moved past them, and would change every signature with them.
    let q = SIGNATURE_STEP;
    let wall_layer = |layer: &str| match &g.layers {
        Some(p) => p.split(',').any(|x| super::qselect::wildcard(x.trim(), layer)),
        None => matches!(g.rules.layer(layer), Some(cadaddy_walk3d::LayerKind::Wall { .. })),
    };
    let mut out = Vec::new();
    for e in d.model.iter().filter(|e| wallgen::made_as(e).is_none()).take(MAX_ITEMS) {
        let pts = |v: &[Vec2]| -> String {
            let mut keys: Vec<(i64, i64)> = v.iter().map(|p| (round_to(p.x, q), round_to(p.y, q))).collect();
            // A line either way round is the same line.
            if keys.len() == 2 {
                keys.sort_unstable();
            }
            format!("{keys:?}")
        };
        let centre = |v: &[Vec2]| Bounds2::from_points(v.iter().copied()).center();
        let item = match &e.kind {
            EntityKind::Line(l) if wall_layer(&e.common.layer) => {
                let v = [l.a.xy(), l.b.xy()];
                Some(("wall", None, pts(&v), centre(&v)))
            }
            EntityKind::LwPolyline(p) if wall_layer(&e.common.layer) => {
                let v: Vec<Vec2> = p.vertices.iter().map(|x| x.p).collect();
                Some(("wall", None, format!("{}{}", pts(&v), p.closed), centre(&v)))
            }
            EntityKind::Insert(i) => {
                let n = i.block.to_ascii_uppercase();
                let kind = if n.contains("DOOR") || n.starts_with("DR") {
                    Some("door")
                } else if n.contains("WIN") || n.contains("WDW") || n.contains("GLAZ") {
                    Some("window")
                } else {
                    None
                };
                kind.map(|k| {
                    let p = i.insert.xy();
                    (
                        k,
                        Some(i.block.clone()),
                        format!("{}|{:?}|{}|{:.6}", n, (round_to(p.x, q), round_to(p.y, q)), round_to(i.scale.x, 1e-6), i.rotation),
                        p,
                    )
                })
            }
            EntityKind::Arc(a) if e.common.layer.to_ascii_uppercase().contains("DOOR") => {
                let c = a.center.xy();
                Some(("door", None, format!("{:?}|{}|{:.6}|{:.6}", (round_to(c.x, q), round_to(c.y, q)), round_to(a.radius, q), a.start, a.end), c))
            }
            _ => None,
        };
        if let Some((kind, name, signature, at)) = item {
            out.push(PlanItem { kind, handle: e.handle, layer: e.common.layer.clone(), name, signature, at });
        }
    }
    out
}

/// The rooms of the current plan as plan items (name labels in closed outlines).
fn room_items(s: &mut Session) -> Vec<PlanItem> {
    let Some(spec) = find_command("plan.rooms") else { return Vec::new() };
    let Ok(r) = (spec.run)(s, &json!({ "outline": false, "limit": 200 })) else { return Vec::new() };
    r["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| {
            let name = x["name"].as_str()?.to_string();
            let at = Vec2::new(x["at"][0].as_f64()?, x["at"][1].as_f64()?);
            let area = x["area"].as_f64().unwrap_or(0.0);
            Some(PlanItem {
                kind: "room",
                handle: x["label"].as_str().and_then(Handle::parse_hex).unwrap_or_default(),
                layer: String::new(),
                signature: format!("{}|{:.3}", name.to_ascii_uppercase(), area),
                name: Some(name),
                at,
            })
        })
        .collect()
}

/// Record what the build just made (and the plan it made it from).
pub(crate) fn record(s: &mut Session, g: &WallGen) {
    let Ok(d) = s.doc() else { return };
    let mut plan = plan_items(d, g);
    let made: HashMap<Handle, (u64, Option<TagKey>)> =
        d.model.iter().filter(|e| wallgen::made_as(e).is_some()).map(|e| (e.handle, (fingerprint(e), tag_key(e)))).collect();
    plan.extend(room_items(s));
    if let Ok(st) = s.state_mut() {
        st.model_record = Some(ModelRecord { plan, made });
    }
}

/// A plan comparison: (added, removed, moved (old, new), unchanged count).
type Diff = (Vec<PlanItem>, Vec<PlanItem>, Vec<(PlanItem, PlanItem)>, usize);

/// Compare two plans: the same handle (or, across files, the same geometry) is the same object;
/// left over, objects of a kind within reach of each other moved; the rest were added or removed.
pub fn diff(old: &[PlanItem], new: &[PlanItem], by_handle: bool, reach: f64) -> Diff {
    let mut unchanged = 0;
    let mut moved = Vec::new();
    let mut old_left: Vec<&PlanItem> = Vec::new();
    let mut new_used: HashSet<usize> = HashSet::new();
    let by_h: HashMap<(Handle, &str), usize> = new.iter().enumerate().map(|(i, n)| ((n.handle, n.kind), i)).collect();
    let by_sig: HashMap<(&str, &str), Vec<usize>> = new.iter().enumerate().fold(HashMap::new(), |mut m, (i, n)| {
        m.entry((n.kind, n.signature.as_str())).or_default().push(i);
        m
    });
    for o in old {
        if by_handle
            && let Some(&i) = by_h.get(&(o.handle, o.kind))
            && new_used.insert(i)
        {
            match new.get(i) {
                Some(n) if n.signature == o.signature => unchanged += 1,
                Some(n) => moved.push((o.clone(), n.clone())),
                None => old_left.push(o),
            }
            continue;
        }
        // Same geometry under another handle (another file, or redrawn in place).
        if let Some(i) = by_sig.get(&(o.kind, o.signature.as_str())).and_then(|v| v.iter().copied().find(|i| !new_used.contains(i))) {
            new_used.insert(i);
            unchanged += 1;
            continue;
        }
        old_left.push(o);
    }
    let mut removed = Vec::new();
    for o in old_left {
        let near = new
            .iter()
            .enumerate()
            .filter(|(i, n)| !new_used.contains(i) && n.kind == o.kind && (o.kind != "room" || n.name == o.name))
            .map(|(i, n)| (i, n.at.dist(o.at)))
            .filter(|(_, d)| *d <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match near {
            Some((i, _)) => {
                new_used.insert(i);
                if let Some(n) = new.get(i) {
                    moved.push((o.clone(), n.clone()));
                }
            }
            None => removed.push(o.clone()),
        }
    }
    let added = new.iter().enumerate().filter(|(i, _)| !new_used.contains(i)).map(|(_, n)| n.clone()).collect();
    (added, removed, moved, unchanged)
}

/// The plan object a generated object came from (its WALLGEN tag's source).
fn source_of(e: &Entity) -> Option<Handle> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(wallgen::APP))?;
    x.get(at + 3).filter(|t| t.code == 1000).and_then(|t| Handle::parse_hex(&t.value))
}

/// A generated object's (part, level, source) tag: what the rebuilt counterpart shares.
fn tag_key(e: &Entity) -> Option<TagKey> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(wallgen::APP))?;
    let v = |k: usize| x.get(at + k).filter(|t| t.code == 1000).map(|t| t.value.clone());
    Some((v(1)?, v(2).unwrap_or_default(), v(3).unwrap_or_default()))
}

/// What a rebuild kept of the hand edits: (kept handles, conflicts).
pub struct Kept {
    pub kept: Vec<Handle>,
    pub conflicts: Vec<Value>,
}

/// Build again from `g`, putting back the hand edits since the last build whose plan objects
/// didn't change (`changed`: the plan objects that did).
pub(crate) fn rebuild_keeping_edits(s: &mut Session, g: &WallGen, changed: &HashSet<Handle>) -> Result<(Value, Kept)> {
    let before = s.state()?.model_record.clone();
    // The edits: generated objects changed since the build, and ones deleted.
    let mut edited: Vec<Entity> = Vec::new();
    let mut deleted: Vec<TagKey> = Vec::new();
    if let Some(rec) = &before {
        let d = s.doc()?;
        for e in d.model.iter().filter(|e| wallgen::made_as(e).is_some()) {
            if rec.made.get(&e.handle).is_some_and(|(f, _)| *f != fingerprint(e)) {
                edited.push((**e).clone());
            }
        }
        deleted = rec.made.iter().filter(|(h, _)| d.entity(**h).is_none()).filter_map(|(_, (_, k))| k.clone()).collect();
    }
    let built = if g.spec.is_some() { super::bspec::build_spec(s, g)? } else { wallgen::build(s, g)? };
    let mut kept = Kept { kept: Vec::new(), conflicts: Vec::new() };
    for e in edited {
        let src = source_of(&e);
        let part = wallgen::made_as(&e).unwrap_or_default();
        if src.is_some_and(|h| changed.contains(&h)) {
            kept.conflicts.push(json!({ "handle": e.handle.hex(), "part": part, "source": src.map(|h| h.hex()), "change": "edited" }));
            continue;
        }
        // The rebuilt counterparts give way to the edited object.
        let key = tag_key(&e);
        let d = s.doc_mut()?;
        let twins: Vec<Handle> = d.model.iter().filter(|x| key.is_some() && tag_key(x) == key).map(|x| x.handle).collect();
        for h in twins {
            d.remove_entity(h);
        }
        let h = if d.entity(e.handle).is_none() { e.handle } else { d.new_handle() };
        d.model.push(Entity { handle: h, ..e });
        kept.kept.push(h);
    }
    // Deleted by hand: stays deleted unless its plan object changed (then it conflicts).
    for key in deleted {
        let src = Handle::parse_hex(&key.2);
        if src.is_some_and(|h| changed.contains(&h)) {
            kept.conflicts.push(json!({ "part": key.0, "source": key.2, "change": "deleted" }));
            continue;
        }
        let d = s.doc_mut()?;
        let twins: Vec<Handle> = d.model.iter().filter(|x| tag_key(x).as_ref() == Some(&key)).map(|x| x.handle).collect();
        for h in twins {
            d.remove_entity(h);
        }
    }
    record(s, g);
    Ok((built, kept))
}

fn item_json(i: &PlanItem) -> Value {
    json!({ "kind": i.kind, "handle": i.handle.hex(), "layer": i.layer, "name": i.name, "at": [i.at.x, i.at.y] })
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "model.revise";
    let g = s
        .state()?
        .wallgen
        .clone()
        .ok_or_else(|| crate::EngineError::Other("The drawing has no model yet: run model.build or WALLGEN first.".into()))?;
    let ext = s.doc()?.extents(&cadaddy_doc::Space::Model);
    let reach = match p.get("tolerance") {
        None | Some(Value::Null) => (ext.width().max(ext.height()) * 0.05).max(1e-6),
        Some(v) => v.as_f64().filter(|t| t.is_finite() && *t >= 0.0).ok_or_else(|| bad(cmd, "`tolerance` must be a distance"))?,
    };
    // The plan to compare with: another file, or what the model was last built from.
    let (old, by_handle) = match str_param(p, "against") {
        Some(path) => {
            let hooks = super::file::io().ok_or_else(|| bad(cmd, "file formats are not available in this build"))?;
            let bytes = read_file(cmd, path)?;
            let other = (hooks.read)(&bytes, path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
            (plan_items(&other, &g), false)
        }
        None => match &s.state()?.model_record {
            Some(r) => (r.plan.iter().filter(|i| i.kind != "room").cloned().collect::<Vec<_>>(), true),
            None => return Err(bad(cmd, "no record of the plan the model was built from: build it once, or pass `against`")),
        },
    };
    let now = plan_items(s.doc()?, &g);
    let (added, removed, moved, unchanged) = diff(&old, &now, by_handle, reach);
    // Rooms: compared by name and outline.
    let rooms_now = room_items(s);
    let rooms_then: Vec<PlanItem> = if by_handle {
        s.state()?.model_record.as_ref().map(|r| r.plan.iter().filter(|i| i.kind == "room").cloned().collect()).unwrap_or_default()
    } else {
        Vec::new()
    };
    let (r_added, r_removed, r_moved, _) =
        if by_handle { diff(&rooms_then, &rooms_now, false, reach) } else { (Vec::new(), Vec::new(), Vec::new(), 0) };
    let changed_sources: HashSet<Handle> =
        removed.iter().map(|i| i.handle).chain(moved.iter().flat_map(|(o, n)| [o.handle, n.handle])).chain(added.iter().map(|i| i.handle)).collect();
    let any = !(added.is_empty() && removed.is_empty() && moved.is_empty() && r_added.is_empty() && r_removed.is_empty() && r_moved.is_empty());
    let rebuild = p.get("rebuild").and_then(Value::as_bool).unwrap_or(true) && any;
    let mut out = json!({
        "added": added.iter().chain(&r_added).map(item_json).collect::<Vec<_>>(),
        "removed": removed.iter().chain(&r_removed).map(item_json).collect::<Vec<_>>(),
        "moved": moved.iter().chain(&r_moved).map(|(o, n)| { let mut v = item_json(n); v["from"] = json!([o.at.x, o.at.y]); v }).collect::<Vec<_>>(),
        "unchanged": unchanged,
        "rebuilt": rebuild,
    });
    if rebuild {
        let (built, kept) = rebuild_keeping_edits(s, &g, &changed_sources)?;
        // The rebuilt objects of changed plan objects, selected to show them.
        let changed: Vec<Handle> =
            s.doc()?.model.iter().filter(|e| source_of(e).is_some_and(|h| changed_sources.contains(&h))).map(|e| e.handle).collect();
        s.set_selection(changed.clone());
        if let Some(o) = out.as_object_mut() {
            o.insert("kept".into(), json!(kept.kept.iter().map(|h| h.hex()).collect::<Vec<_>>()));
            o.insert("conflicts".into(), json!(kept.conflicts));
            o.insert("changed".into(), json!(changed.iter().map(|h| h.hex()).collect::<Vec<_>>()));
            o.insert("model".into(), built);
        }
        s.echo(format!(
            "Plan changes: {} added, {} removed, {} moved. Rebuilt; {} edit(s) kept, {} conflict(s).",
            added.len() + r_added.len(),
            removed.len() + r_removed.len(),
            moved.len() + r_moved.len(),
            kept.kept.len(),
            kept.conflicts.len()
        ));
    }
    Ok(out)
}
