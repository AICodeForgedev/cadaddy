//! Overrides of generated elements (`override.*`): edits that survive re-runs.
//!
//! What a build generated (WALLGEN, `model.build`, the rebuild after a plan change or a new
//! revision) can be overridden by a person, an agent or an import: a device moved, its type
//! changed, a stud verified in the field, a false detection deleted, a missing item added. Each
//! override is kept in the drawing (`cadaddy_doc::overrides`), keyed to the element's stable
//! identity (its WALLGEN tag — part, level, plan object — and where it stood), not written into
//! the generated geometry.
//!
//! After every build the overrides are applied again to the regenerated elements:
//! - kept (`active`) where the plan in their source area is as it was;
//! - flagged `review` ("the drawing changed here") where the new revision touches that area;
//! - `orphaned` when their element is gone: listed, never dropped.
//!
//! Applied overrides mark their element (XDATA `CADADDY_OVERRIDE`), so applying again changes
//! nothing. `override.show` hides them (the model as the rules generate it) and shows them again;
//! `override.revert` takes one back; `override.list` lists them by author, status or kind.

use std::collections::HashMap;

use cadaddy_doc::overrides::{self, Author, Change, ElementKey, Override, Overrides, Status};
use cadaddy_doc::{Drawing, Entity, Handle, bimspec, entity_bounds};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

const ADD: &str = "override.add";
const LIST: &str = "override.list";
const REVERT: &str = "override.revert";
const ACCEPT: &str = "override.accept";
const SHOW: &str = "override.show";
const REAPPLY: &str = "override.reapply";

/// Most plan objects an area signature looks at.
const MAX_SOURCES: usize = 400_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(ADD, "Override Generated Element", run_add)
            .alias(&["override"])
            .menu(&["Draw", "Modeling", "Overrides", "Override Generated Element"])
            .params("{handle, kind: move|type|verify|delete|add, delta?: [dx, dy, dz?] (move), mark?: type mark (type), note?, author?: user|agent|import (default user)} → {override}; kind add registers a hand-drawn object as a missing item")
            .interactive(|_| Ok(Box::new(AddM::default()))),
        CommandSpec::new(LIST, "List Overrides", run_list)
            .alias(&["overrides"])
            .menu(&["Draw", "Modeling", "Overrides", "List Overrides"])
            .params("{author?: user|agent|import, status?: active|review|orphaned, kind?, select?: bool (select their elements)} → {overrides: [...], hidden, counts: {active, review, orphaned}, byAuthor: {user, agent, import}}")
            .noundo(),
        CommandSpec::new(REVERT, "Revert Override", run_revert)
            .alias(&["overriderevert"])
            .menu(&["Draw", "Modeling", "Overrides", "Revert Override"])
            .params("{id: \"OV-0001\" | number} → {reverted, element?}: the element goes back to what the rules generate")
            .interactive(|_| Ok(Box::new(RevertM))),
        CommandSpec::new(ACCEPT, "Accept Override After Review", run_accept)
            .params("{id?, all?: bool} → {accepted: [ids]}: the drawing's change in the source area is taken as seen"),
        CommandSpec::new(SHOW, "Show or Hide Overrides", run_show)
            .alias(&["overrideshow"])
            .menu(&["Draw", "Modeling", "Overrides", "Show or Hide Overrides"])
            .params("{on?: bool (default: toggle)} → {hidden, changed}: hidden, the model shows what the rules generate"),
        CommandSpec::new(REAPPLY, "Re-apply Overrides", run_reapply)
            .params("{} → {active, review, orphaned, applied}: applies the overrides not applied yet (builds do this by themselves)"),
    ]
}

// ---------- storage ----------

/// The drawing's overrides (none stored: an empty set).
pub fn load(d: &Drawing) -> std::result::Result<Overrides, String> {
    match d.overrides.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => Overrides::from_json(t),
        None => Ok(Overrides::default()),
    }
}

fn save(d: &mut Drawing, o: &Overrides) -> Result<()> {
    d.overrides = if o.is_empty() && !o.hidden { None } else { Some(o.to_json().map_err(|e| crate::EngineError::Other(format!("overrides: {e}")))?) };
    Ok(())
}

fn load_for(cmd: &str, s: &Session) -> Result<Overrides> {
    load(s.doc()?).map_err(|e| bad(cmd, e))
}

// ---------- identities and source areas ----------

fn fnv(bytes: &[u8], seed: u64) -> u64 {
    bytes.iter().fold(seed, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}
const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

fn rect(b: &Bounds2) -> [f64; 4] {
    [b.min.x, b.min.y, b.max.x, b.max.y]
}

fn bounds_of(a: &[f64; 4]) -> Bounds2 {
    Bounds2 { min: Vec2::new(a[0], a[1]), max: Vec2::new(a[2], a[3]) }
}

/// The source area of an element with these bounds: the bounds and a tenth of their size around.
fn area_around(b: &Bounds2) -> Bounds2 {
    let diag = b.min.dist(b.max);
    b.expand(if diag.is_finite() { diag * 0.1 + 1e-6 } else { 1e-6 })
}

/// The plan objects (anything not generated and not added by an override) with their bounds and
/// a signature of their geometry and layer, independent of handles.
fn sources(d: &Drawing) -> Vec<(Bounds2, u64)> {
    d.model
        .iter()
        .filter(|e| wallgen::made_as(e).is_none() && overrides::applied_ids(&e.common.xdata).is_empty())
        .take(MAX_SOURCES)
        .filter_map(|e| {
            let b = entity_bounds(d, e, 0);
            (!b.is_empty()).then(|| (b, fnv(format!("{:?}|{}", e.kind, e.common.layer.to_ascii_uppercase()).as_bytes(), FNV0)))
        })
        .collect()
}

/// A signature of what the plan holds in `area`.
fn area_sig(sources: &[(Bounds2, u64)], area: &Bounds2) -> u64 {
    let mut sigs: Vec<u64> = sources.iter().filter(|(b, _)| b.intersects(area)).map(|(_, s)| *s).collect();
    sigs.sort_unstable();
    sigs.iter().fold(FNV0, |h, s| fnv(&s.to_le_bytes(), h))
}

/// A generated element as the index sees it.
struct Made {
    handle: Handle,
    part: String,
    level: String,
    source: String,
    at: Vec2,
}

/// The generated elements in model space.
fn index(d: &Drawing) -> Vec<Made> {
    d.model
        .iter()
        .filter_map(|e| {
            let (part, level, source) = super::revise3d::tag_key(e)?;
            Some(Made { handle: e.handle, part, level, source, at: entity_bounds(d, e, 0).center() })
        })
        .collect()
}

/// The element `key` names: the same plan object's (nearest to where it stood), else — the plan
/// object redrawn, or a new revision from another file — the nearest of its part on its level
/// within `reach`.
fn find(index: &[Made], key: &ElementKey, reach: f64) -> Option<Handle> {
    let at = Vec2::new(key.at[0], key.at[1]);
    let nearest =
        |it: &mut dyn Iterator<Item = &Made>| it.map(|m| (m.handle, m.at.dist(at))).filter(|(_, d)| d.is_finite()).min_by(|a, b| a.1.total_cmp(&b.1));
    let same = |m: &&Made| m.part == key.part && m.level == key.level;
    if let Some((h, _)) = nearest(&mut index.iter().filter(same).filter(|m| m.source == key.source)) {
        return Some(h);
    }
    nearest(&mut index.iter().filter(same)).filter(|(_, d)| *d <= reach).map(|(h, _)| h)
}

/// How far a new revision may have moved an element and it still be the same one.
fn reach(o: &Override) -> f64 {
    let a = bounds_of(&o.area);
    (a.min.dist(a.max) * 0.5).max(1e-6)
}

// ---------- applying ----------

fn translated(e: &mut Entity, d: [f64; 3]) {
    let m = cadaddy_doc::translation(Vec3 { x: d[0], y: d[1], z: d[2] });
    e.kind.transform3(&m);
}

fn set_mark(xdata: &[cadaddy_doc::RawTag], mark: Option<&str>) -> Vec<cadaddy_doc::RawTag> {
    let mut x = bimspec::strip_bim_xdata(xdata);
    if let Some(m) = mark {
        x.extend(bimspec::type_mark_xdata(m));
    }
    x
}

/// Apply `o` to element `h` (or, for an added item, put it in). Returns the element it now
/// applies to. `touched` collects every handle changed.
fn apply(d: &mut Drawing, o: &mut Override, h: Option<Handle>, touched: &mut Vec<Handle>) -> Option<Handle> {
    let id = o.id;
    match &mut o.change {
        Change::Add { entity } => {
            let mut e = (**entity).clone();
            e.handle = if d.entity(e.handle).is_none() && e.handle.0 != 0 { e.handle } else { d.new_handle() };
            e.common.xdata = overrides::mark(&e.common.xdata, id).into();
            d.ensure_layer(&e.common.layer);
            let h = e.handle;
            d.model.push(e);
            touched.push(h);
            Some(h)
        }
        Change::Delete { removed } => {
            let h = h?;
            let e = d.remove_entity(h)?;
            *removed = Some(Box::new((*e).clone()));
            touched.push(h);
            Some(h)
        }
        Change::Move { delta } => {
            let (h, dl) = (h?, *delta);
            d.modify_entity(h, |e| {
                translated(e, dl);
                e.common.xdata = overrides::mark(&e.common.xdata, id).into();
            })
            .ok()?;
            touched.push(h);
            Some(h)
        }
        Change::Type { mark, before } => {
            let h = h?;
            *before = d.entity(h).and_then(|e| bimspec::entity_type_mark(&e.common.xdata).map(str::to_string));
            let m = mark.clone();
            d.modify_entity(h, |e| e.common.xdata = overrides::mark(&set_mark(&e.common.xdata, Some(&m)), id).into()).ok()?;
            touched.push(h);
            Some(h)
        }
        Change::Verify => {
            let h = h?;
            d.modify_entity(h, |e| e.common.xdata = overrides::mark(&e.common.xdata, id).into()).ok()?;
            touched.push(h);
            Some(h)
        }
        Change::Props { props, before } => {
            let h = h?;
            let e = d.entity(h)?;
            let mut store = cadaddy_doc::props::read(&e.common.xdata);
            // On an element just generated, what it held before is what the build gave it.
            if !overrides::applied_ids(&e.common.xdata).contains(&id) {
                before.clear();
            }
            for en in props.iter() {
                if !before.iter().any(|(k, _)| k.eq_ignore_ascii_case(&en.key)) {
                    before.push((en.key.clone(), store.get(&en.key).cloned()));
                }
                store.set(&en.key, en.prop.clone());
            }
            d.modify_entity(h, |e| e.common.xdata = overrides::mark(&cadaddy_doc::props::write(&e.common.xdata, &store), id).into()).ok()?;
            touched.push(h);
            Some(h)
        }
    }
}

/// The element override `id` is applied to now (marked with it).
fn marked(d: &Drawing, o: &Override) -> Option<Handle> {
    if let Some(h) = o.applied
        && d.entity(h).is_some_and(|e| overrides::applied_ids(&e.common.xdata).contains(&o.id))
    {
        return Some(h);
    }
    d.model.iter().find(|e| overrides::applied_ids(&e.common.xdata).contains(&o.id)).map(|e| e.handle)
}

/// Take `o` off its element: what the rules generated comes back. Returns the element.
fn unapply(d: &mut Drawing, o: &Override, touched: &mut Vec<Handle>) -> Option<Handle> {
    let id = o.id;
    match &o.change {
        Change::Delete { removed } => {
            // Still deleted: put the element back as it was deleted.
            let e = removed.as_ref()?;
            if o.status == Status::Orphaned || o.applied.is_none() {
                return None;
            }
            let mut e = (**e).clone();
            if d.entity(e.handle).is_some() {
                e.handle = d.new_handle();
            }
            let h = e.handle;
            d.model.push(e);
            touched.push(h);
            Some(h)
        }
        Change::Add { .. } => {
            let h = marked(d, o)?;
            d.remove_entity(h);
            touched.push(h);
            Some(h)
        }
        Change::Move { delta } => {
            let h = marked(d, o)?;
            let back = [-delta[0], -delta[1], -delta[2]];
            d.modify_entity(h, |e| {
                translated(e, back);
                e.common.xdata = overrides::unmark(&e.common.xdata, id).into();
            })
            .ok()?;
            touched.push(h);
            Some(h)
        }
        Change::Type { before, .. } => {
            let h = marked(d, o)?;
            let b = before.clone();
            d.modify_entity(h, |e| e.common.xdata = overrides::unmark(&set_mark(&e.common.xdata, b.as_deref()), id).into()).ok()?;
            touched.push(h);
            Some(h)
        }
        Change::Verify => {
            let h = marked(d, o)?;
            d.modify_entity(h, |e| e.common.xdata = overrides::unmark(&e.common.xdata, id).into()).ok()?;
            touched.push(h);
            Some(h)
        }
        Change::Props { before, .. } => {
            let h = marked(d, o)?;
            let mut store = d.entity(h).map(|e| cadaddy_doc::props::read(&e.common.xdata)).unwrap_or_default();
            for (k, was) in before {
                match was {
                    Some(p) => {
                        store.set(k, p.clone());
                    }
                    None => {
                        store.remove(k);
                    }
                }
            }
            d.modify_entity(h, |e| e.common.xdata = overrides::unmark(&cadaddy_doc::props::write(&e.common.xdata, &store), id).into()).ok()?;
            touched.push(h);
            Some(h)
        }
    }
}

/// What a re-apply did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub active: usize,
    pub review: Vec<u32>,
    pub orphaned: Vec<u32>,
    /// Overrides applied by this pass (not already in place).
    pub applied: Vec<u32>,
}

/// Apply the overrides not in place on `d`. `fresh`: the model was just generated again (a
/// deleted element not found is then gone from the revision, not still deleted).
pub fn reapply(d: &mut Drawing, fresh: bool, touched: &mut Vec<Handle>) -> std::result::Result<Outcome, String> {
    let mut ov = load(d)?;
    let mut out = Outcome::default();
    if ov.is_empty() || ov.hidden {
        return Ok(out);
    }
    let idx = index(d);
    let src = sources(d);
    let mut by_id: HashMap<u32, Handle> = HashMap::new();
    for e in d.model.iter() {
        for id in overrides::applied_ids(&e.common.xdata) {
            by_id.entry(id).or_insert(e.handle);
        }
    }
    for o in &mut ov.items {
        let sig_ok = area_sig(&src, &bounds_of(&o.area)) == o.area_sig;
        let settled = if sig_ok { Status::Active } else { Status::Review };
        // In place already: an earlier pass, or a hand edit a rebuild kept.
        if let Some(&h) = by_id.get(&o.id) {
            o.applied = Some(h);
            o.status = settled;
        } else if matches!(o.change, Change::Delete { .. }) && !fresh && o.applied.is_some() && o.status != Status::Orphaned {
            o.status = settled;
        } else {
            let target = match (&o.change, &o.target) {
                (Change::Add { .. }, _) => None,
                (_, Some(k)) => find(&idx, k, reach(o)),
                (_, None) => None,
            };
            let needs_target = !matches!(o.change, Change::Add { .. });
            if needs_target && target.is_none() {
                o.applied = None;
                o.status = Status::Orphaned;
            } else {
                o.applied = apply(d, o, target, touched);
                o.status = if o.applied.is_some() { settled } else { Status::Orphaned };
                if o.applied.is_some() {
                    out.applied.push(o.id);
                }
            }
        }
        match o.status {
            Status::Active => out.active += 1,
            Status::Review => out.review.push(o.id),
            Status::Orphaned => out.orphaned.push(o.id),
        }
    }
    d.overrides = Some(ov.to_json().map_err(|e| format!("overrides: {e}"))?);
    Ok(out)
}

/// After a build or rebuild: apply the overrides to the regenerated model and report what
/// needs a look.
pub(crate) fn after_build(s: &mut Session) {
    let Ok(d) = s.doc_mut() else { return };
    if d.overrides.is_none() {
        return;
    }
    let mut touched = Vec::new();
    match reapply(d, true, &mut touched) {
        Ok(o) if !o.review.is_empty() || !o.orphaned.is_empty() => s.echo(format!(
            "Overrides: {} kept, {} to review (the drawing changed there), {} orphaned (their element is gone).",
            o.active,
            o.review.len(),
            o.orphaned.len()
        )),
        Ok(_) => {}
        Err(e) => s.echo(e),
    }
}

/// Set properties a person edited on generated elements, as overrides (one per element, merged
/// into the element's earlier properties override), so a rebuild puts them back. `edits`: the
/// element and its new properties. Returns, per element, the override's label; an element no
/// build generated is left out (the caller writes its store directly).
pub(crate) fn set_props(
    s: &mut Session,
    cmd: &str,
    edits: &[(Handle, Vec<cadaddy_doc::props::Entry>)],
    author: Author,
) -> Result<Vec<(Handle, String)>> {
    let mut ov = load_for(cmd, s)?;
    if ov.hidden {
        return Err(bad(cmd, "overrides are hidden: show them (override.show) before editing a generated element's properties"));
    }
    let d = s.doc()?;
    let mut src: Option<Vec<(Bounds2, u64)>> = None;
    // Per element: its new properties, and the override they go in (an earlier one's id, or a
    // new one).
    let mut plan: Vec<(Handle, &[cadaddy_doc::props::Entry], std::result::Result<u32, Override>)> = Vec::new();
    for (h, props) in edits {
        let Some(e) = d.model.get(*h) else { continue };
        let Some((part, level, source)) = super::revise3d::tag_key(e) else { continue };
        let ids = overrides::applied_ids(&e.common.xdata);
        if let Some(id) = ids.iter().copied().find(|id| matches!(ov.get(*id).map(|o| &o.change), Some(Change::Props { .. }))) {
            plan.push((*h, props, Ok(id)));
            continue;
        }
        // Where it was generated: undo the moves already applied to it.
        let mut bounds = entity_bounds(d, e, 0);
        for id in &ids {
            if let Some(Change::Move { delta }) = ov.get(*id).map(|o| &o.change) {
                bounds = Bounds2 {
                    min: Vec2::new(bounds.min.x - delta[0], bounds.min.y - delta[1]),
                    max: Vec2::new(bounds.max.x - delta[0], bounds.max.y - delta[1]),
                };
            }
        }
        if bounds.is_empty() {
            continue;
        }
        if ov.items.len() + plan.len() >= overrides::MAX_OVERRIDES {
            return Err(bad(cmd, "the drawing holds too many overrides"));
        }
        let at = bounds.center();
        let area = area_around(&bounds);
        let sig = area_sig(src.get_or_insert_with(|| sources(d)), &area);
        let id = ov.take_id().ok_or_else(|| bad(cmd, "the drawing holds too many overrides"))?;
        plan.push((
            *h,
            props,
            Err(Override {
                id,
                author,
                note: None,
                change: Change::Props { props: Vec::new(), before: Vec::new() },
                target: Some(ElementKey { part, level, source, at: [at.x, at.y] }),
                area: rect(&area),
                area_sig: sig,
                status: Status::Active,
                applied: None,
            }),
        ));
    }
    let mut touched = Vec::new();
    let mut out = Vec::new();
    let d = s.doc_mut()?;
    for (h, props, slot) in plan {
        let o = match slot {
            Ok(id) => match ov.get_mut(id) {
                Some(o) => o,
                None => continue,
            },
            Err(o) => {
                ov.items.push(o);
                match ov.items.last_mut() {
                    Some(o) => o,
                    None => continue,
                }
            }
        };
        // The new values go on the element (the earlier ones are in place already), then join
        // the override's kept ones.
        let before = match &o.change {
            Change::Props { before, .. } => before.clone(),
            _ => Vec::new(),
        };
        let mut once = Override { change: Change::Props { props: props.to_vec(), before }, ..o.clone() };
        let applied = apply(d, &mut once, Some(h), &mut touched);
        if let (Change::Props { props: kept, before }, Change::Props { before: now, .. }) = (&mut o.change, once.change) {
            *before = now;
            for en in props {
                match kept.iter_mut().find(|k| k.key.eq_ignore_ascii_case(&en.key)) {
                    Some(k) => k.prop = en.prop.clone(),
                    None => kept.push(en.clone()),
                }
            }
        }
        o.applied = applied;
        out.push((h, o.label()));
    }
    save(d, &ov)?;
    super::revise3d::resync(s, &touched);
    Ok(out)
}

// ---------- commands ----------

fn id_param(cmd: &str, p: &Value) -> Result<u32> {
    match p.get("id") {
        Some(Value::String(t)) => overrides::parse_label(t),
        Some(v) => v.as_u64().and_then(|n| u32::try_from(n).ok()),
        None => None,
    }
    .ok_or_else(|| bad(cmd, "`id` (an override id such as \"OV-0001\") is required"))
}

fn override_json(o: &Override) -> Value {
    let mut v = json!({
        "id": o.label(),
        "kind": o.change.kind(),
        "author": o.author.as_str(),
        "status": o.status.as_str(),
        "area": o.area,
    });
    if let Some(n) = &o.note {
        v["note"] = json!(n);
    }
    if let Some(t) = &o.target {
        v["target"] = json!({ "part": t.part, "level": t.level, "source": t.source, "at": t.at });
    }
    if let Some(h) = o.applied {
        v["element"] = json!(h.hex());
    }
    match &o.change {
        Change::Move { delta } => v["delta"] = json!(delta),
        Change::Type { mark, before } => {
            v["mark"] = json!(mark);
            v["before"] = json!(before);
        }
        Change::Props { props, .. } => {
            v["props"] = Value::Object(props.iter().map(|e| (e.key.clone(), e.prop.value.clone())).collect());
        }
        _ => {}
    }
    if o.status == Status::Review {
        v["message"] = json!("review: the drawing changed here");
    } else if o.status == Status::Orphaned {
        v["message"] = json!("orphaned: its element is no longer generated");
    }
    v
}

fn delta_param(p: &Value) -> Option<[f64; 3]> {
    let a = p.get("delta")?.as_array()?;
    let n = |i: usize| a.get(i).and_then(Value::as_f64);
    let d = [n(0)?, n(1)?, n(2).unwrap_or(0.0)];
    (d.iter().all(|v| v.is_finite()) && a.len() <= 3).then_some(d)
}

fn run_add(s: &mut Session, p: &Value) -> Result<Value> {
    let h = str_param(p, "handle").and_then(Handle::parse_hex).ok_or_else(|| bad(ADD, "`handle` (hex) is required"))?;
    let kind = str_param(p, "kind").unwrap_or("").trim().to_ascii_lowercase();
    let author = match str_param(p, "author") {
        None => Author::User,
        Some(a) => Author::parse(a).ok_or_else(|| bad(ADD, "`author` must be user, agent or import"))?,
    };
    let note = str_param(p, "note").map(|n| n.chars().take(overrides::MAX_NOTE).collect::<String>()).filter(|n| !n.trim().is_empty());
    let mut ov = load_for(ADD, s)?;
    if ov.hidden {
        return Err(bad(ADD, "overrides are hidden: show them (override.show) before adding one"));
    }
    let d = s.doc()?;
    let e = d.model.get(h).ok_or_else(|| bad(ADD, format!("no object {} in model space", h.hex())))?.clone();
    let generated = super::revise3d::tag_key(&e);
    let change = match kind.as_str() {
        "move" => Change::Move { delta: delta_param(p).ok_or_else(|| bad(ADD, "`delta` ([dx, dy] or [dx, dy, dz], finite) is required to move"))? },
        "type" => {
            let mark = str_param(p, "mark").map(str::trim).filter(|m| !m.is_empty() && m.len() <= 64);
            Change::Type { mark: mark.ok_or_else(|| bad(ADD, "`mark` (a type mark) is required"))?.to_string(), before: None }
        }
        "verify" => Change::Verify,
        "delete" => Change::Delete { removed: None },
        "add" => {
            if generated.is_some() {
                return Err(bad(ADD, "that object was generated: add takes an object drawn by hand (a missing item)"));
            }
            if !overrides::applied_ids(&e.common.xdata).is_empty() {
                return Err(bad(ADD, "that object is already an override's"));
            }
            Change::Add { entity: Box::new((*e).clone()) }
        }
        _ => return Err(bad(ADD, "`kind` must be move, type, verify, delete or add")),
    };
    let target = match (&change, &generated) {
        (Change::Add { .. }, _) => None,
        (_, None) => return Err(bad(ADD, "that object wasn't generated by a build: edit it directly, or add it as a missing item (kind add)")),
        (_, Some((part, level, source))) => {
            // Where it was generated: undo the moves already applied to it.
            let mut at = entity_bounds(d, &e, 0).center();
            let mut bounds = entity_bounds(d, &e, 0);
            for id in overrides::applied_ids(&e.common.xdata) {
                if let Some(Change::Move { delta }) = ov.get(id).map(|o| &o.change) {
                    at = Vec2::new(at.x - delta[0], at.y - delta[1]);
                    bounds = Bounds2 {
                        min: Vec2::new(bounds.min.x - delta[0], bounds.min.y - delta[1]),
                        max: Vec2::new(bounds.max.x - delta[0], bounds.max.y - delta[1]),
                    };
                }
                if let (Change::Verify, Some(Change::Verify)) = (&change, ov.get(id).map(|o| &o.change)) {
                    return Err(bad(ADD, format!("already verified ({})", overrides::label(id))));
                }
            }
            Some((ElementKey { part: part.clone(), level: level.clone(), source: source.clone(), at: [at.x, at.y] }, bounds))
        }
    };
    let bounds = match &target {
        Some((_, b)) => *b,
        None => entity_bounds(d, &e, 0),
    };
    if bounds.is_empty() {
        return Err(bad(ADD, "that object has no extent to key an override to"));
    }
    let area = area_around(&bounds);
    let id = ov.take_id().ok_or_else(|| bad(ADD, "the drawing holds too many overrides"))?;
    let mut o =
        Override { id, author, note, change, target: target.map(|(k, _)| k), area: rect(&area), area_sig: 0, status: Status::Active, applied: None };
    let mut touched = Vec::new();
    let d = s.doc_mut()?;
    o.applied = match &o.change {
        // The hand-drawn object becomes the added item (marked in place, not copied).
        Change::Add { .. } => {
            d.modify_entity(h, |x| x.common.xdata = overrides::mark(&x.common.xdata, id).into()).map_err(|e| bad(ADD, e.to_string()))?;
            touched.push(h);
            Some(h)
        }
        _ => apply(d, &mut o, Some(h), &mut touched),
    };
    // Signed once applied: an added item is no plan content of its own area.
    o.area_sig = area_sig(&sources(d), &area);
    let out = override_json(&o);
    ov.items.push(o);
    save(d, &ov)?;
    super::revise3d::resync(s, &touched);
    s.echo(format!("{} recorded ({}).", overrides::label(id), out["kind"].as_str().unwrap_or("")));
    Ok(json!({ "override": out }))
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let ov = load_for(LIST, s)?;
    let author = match str_param(p, "author") {
        None => None,
        Some(a) => Some(Author::parse(a).ok_or_else(|| bad(LIST, "`author` must be user, agent or import"))?),
    };
    let status = match str_param(p, "status") {
        None => None,
        Some(a) => Some(Status::parse(a).ok_or_else(|| bad(LIST, "`status` must be active, review or orphaned"))?),
    };
    let kind = str_param(p, "kind").map(|k| k.trim().to_ascii_lowercase());
    let shown: Vec<&Override> = ov
        .items
        .iter()
        .filter(|o| {
            author.is_none_or(|a| o.author == a) && status.is_none_or(|st| o.status == st) && kind.as_deref().is_none_or(|k| o.change.kind() == k)
        })
        .collect();
    let count = |st: Status| ov.items.iter().filter(|o| o.status == st).count();
    let by = |a: Author| ov.items.iter().filter(|o| o.author == a).count();
    if bool_or(p, "select", false) {
        let d = s.doc()?;
        let sel: Vec<Handle> = shown.iter().filter_map(|o| o.applied).filter(|h| d.entity(*h).is_some()).collect();
        s.set_selection(sel);
    }
    Ok(json!({
        "overrides": shown.iter().map(|o| override_json(o)).collect::<Vec<_>>(),
        "hidden": ov.hidden,
        "counts": { "active": count(Status::Active), "review": count(Status::Review), "orphaned": count(Status::Orphaned) },
        "byAuthor": { "user": by(Author::User), "agent": by(Author::Agent), "import": by(Author::Import) },
    }))
}

fn run_revert(s: &mut Session, p: &Value) -> Result<Value> {
    let id = id_param(REVERT, p)?;
    let mut ov = load_for(REVERT, s)?;
    let o = ov.remove(id).ok_or_else(|| bad(REVERT, format!("no override {}", overrides::label(id))))?;
    let mut touched = Vec::new();
    let d = s.doc_mut()?;
    let element = if ov.hidden { None } else { unapply(d, &o, &mut touched) };
    save(d, &ov)?;
    super::revise3d::resync(s, &touched);
    s.echo(format!("{} reverted.", o.label()));
    Ok(json!({ "reverted": o.label(), "element": element.map(|h| h.hex()) }))
}

fn run_accept(s: &mut Session, p: &Value) -> Result<Value> {
    let mut ov = load_for(ACCEPT, s)?;
    let ids: Vec<u32> = if bool_or(p, "all", false) {
        ov.items.iter().filter(|o| o.status == Status::Review).map(|o| o.id).collect()
    } else {
        vec![id_param(ACCEPT, p)?]
    };
    let src = sources(s.doc()?);
    let mut accepted = Vec::new();
    for id in ids {
        let o = ov.get_mut(id).ok_or_else(|| bad(ACCEPT, format!("no override {}", overrides::label(id))))?;
        if o.status == Status::Orphaned {
            return Err(bad(ACCEPT, format!("{} is orphaned: revert it, or build a model that has its element", o.label())));
        }
        o.area_sig = area_sig(&src, &bounds_of(&o.area));
        o.status = Status::Active;
        accepted.push(o.label());
    }
    save(s.doc_mut()?, &ov)?;
    Ok(json!({ "accepted": accepted }))
}

fn run_show(s: &mut Session, p: &Value) -> Result<Value> {
    let mut ov = load_for(SHOW, s)?;
    let hide = match p.get("on").and_then(Value::as_bool) {
        Some(on) => !on,
        None => !ov.hidden,
    };
    let mut touched = Vec::new();
    let d = s.doc_mut()?;
    let mut changed = 0;
    if hide && !ov.hidden {
        // Newest first: a later override on the same element comes off before an earlier one.
        for i in (0..ov.items.len()).rev() {
            let Some(o) = ov.items.get(i) else { continue };
            if unapply(d, o, &mut touched).is_some() {
                changed += 1;
            }
            if let Some(o) = ov.items.get_mut(i) {
                o.applied = None;
            }
        }
        ov.hidden = true;
        save(d, &ov)?;
    } else if !hide && ov.hidden {
        ov.hidden = false;
        save(d, &ov)?;
        changed = reapply(d, false, &mut touched).map_err(|e| bad(SHOW, e))?.applied.len();
    }
    super::revise3d::resync(s, &touched);
    Ok(json!({ "hidden": hide, "changed": changed }))
}

fn run_reapply(s: &mut Session, _p: &Value) -> Result<Value> {
    let mut touched = Vec::new();
    let o = reapply(s.doc_mut()?, false, &mut touched).map_err(|e| bad(REAPPLY, e))?;
    super::revise3d::resync(s, &touched);
    let l = |v: &[u32]| v.iter().map(|&i| overrides::label(i)).collect::<Vec<_>>();
    Ok(json!({ "active": o.active, "review": l(&o.review), "orphaned": l(&o.orphaned), "applied": l(&o.applied) }))
}

// ---------- at the command line ----------

/// OVERRIDE at the command line: pick the element, choose what to do, then the move's two points
/// or the type mark.
#[derive(Default)]
struct AddM {
    picked: Option<Handle>,
    kind: Option<&'static str>,
    base: Option<Vec2>,
}

impl AddM {
    fn finish(s: &mut Session, p: Value) -> Result<Step> {
        run_add(s, &p)?;
        Ok(Step::Done)
    }
}

impl Interactive for AddM {
    fn name(&self) -> &'static str {
        "OVERRIDE"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        // One object picked first is the element.
        if let [h] = s.selection().as_slice() {
            self.picked = Some(*h);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (self.picked, self.kind, self.base) {
            (None, ..) => Prompt::new("Select generated element (or an object drawn by hand to add)", Accept::POINT),
            (Some(_), None, _) => Prompt::new("Override", Accept::TEXT).kw(&["Move", "Type", "Verify", "Delete", "Add"]),
            (Some(_), Some("type"), _) => Prompt::new("Enter type mark", Accept::TEXT),
            (Some(_), Some(_), None) => Prompt::new("Specify base point", Accept::POINT),
            (Some(_), Some(_), Some(b)) => Prompt::new("Specify second point", Accept::POINT).base(b),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let Some(h) = self.picked else {
            return match i {
                Input::Point(p) => {
                    self.picked = Some(super::curves::pick_at(s, p).ok_or_else(|| EngineError::Other("Nothing there.".into()))?);
                    Ok(Step::Continue)
                }
                Input::Pick(v) if v.len() == 1 => {
                    self.picked = v.first().copied();
                    Ok(Step::Continue)
                }
                Input::Enter | Input::Cancel => Ok(Step::Done),
                _ => Err(EngineError::Other("Select one object.".into())),
            };
        };
        match (self.kind, i) {
            (_, Input::Cancel) => Ok(Step::Done),
            (None, Input::Keyword(k) | Input::Text(k)) => {
                let kind = match k.trim().to_ascii_lowercase().as_str() {
                    "move" | "m" => "move",
                    "type" | "t" => "type",
                    "verify" | "v" => "verify",
                    "delete" | "d" => "delete",
                    "add" | "a" => "add",
                    _ => return Err(EngineError::Other("Choose Move, Type, Verify, Delete or Add.".into())),
                };
                if matches!(kind, "verify" | "delete" | "add") {
                    return Self::finish(s, json!({ "handle": h.hex(), "kind": kind }));
                }
                self.kind = Some(kind);
                Ok(Step::Continue)
            }
            (None, Input::Enter) => Ok(Step::Done),
            (Some("type"), Input::Text(m) | Input::Keyword(m)) => Self::finish(s, json!({ "handle": h.hex(), "kind": "type", "mark": m.trim() })),
            (Some("move"), Input::Point(p)) => match self.base {
                None => {
                    self.base = Some(p);
                    Ok(Step::Continue)
                }
                Some(b) => Self::finish(s, json!({ "handle": h.hex(), "kind": "move", "delta": [p.x - b.x, p.y - b.y, 0.0] })),
            },
            (Some(_), Input::Enter) => Ok(Step::Done),
            _ => Err(EngineError::Other("Invalid input.".into())),
        }
    }
}

/// OVERRIDEREVERT at the command line: the override's id (default the newest).
struct RevertM;

impl Interactive for RevertM {
    fn name(&self) -> &'static str {
        "OVERRIDEREVERT"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        let newest = s.doc().ok().and_then(|d| load(d).ok()).and_then(|o| o.items.last().map(Override::label));
        let p = Prompt::new("Enter override to revert", Accept::TEXT);
        match newest {
            Some(l) => p.default(l),
            None => p,
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let id = match i {
            Input::Text(t) | Input::Keyword(t) => t,
            Input::Enter => match s.doc().ok().and_then(|d| load(d).ok()).and_then(|o| o.items.last().map(Override::label)) {
                Some(l) => l,
                None => {
                    s.echo("No overrides.");
                    return Ok(Step::Done);
                }
            },
            Input::Cancel => return Ok(Step::Done),
            _ => return Err(EngineError::Other("Enter an override id such as OV-0001.".into())),
        };
        run_revert(s, &json!({ "id": id.trim() }))?;
        Ok(Step::Done)
    }
}
