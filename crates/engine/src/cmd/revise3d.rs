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

/// A revision report's lists, paged alike.
const LISTS: [&str; 6] = ["added", "removed", "moved", "kept", "conflicts", "changed"];
/// Items per list in one page of a revision report, by default and at most.
const PAGE: usize = 100;
const PAGE_MAX: usize = 500;
/// Most `plan.rooms` pages read (its largest page is 200 rooms).
const MAX_ROOM_PAGES: usize = 100;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("model.revise", "Revise Model from Plans", run)
        .menu(&["Draw", "Modeling", "Revise Model from Plans"])
        .params("{against?: path (an earlier or later version of the plan to compare with, rooms included; default the plan the model was last built from), rebuild?: bool (default true), tolerance? (drawing units: how far an object may move and still count as moved rather than removed and added; default 5 % of the larger side of the box around both plans' objects), last?: bool (return the last revision that found changes, e.g. the one an associative model made when the plan was edited, instead of comparing again), offset?, limit? (default 100, at most 500: each list is paged alike)} → {added, removed, moved: [{kind, handle, layer, name?, at, from?}], unchanged, rebuilt, auto, kept: [handles], conflicts: [{handle?, part, source, change}], changed: [handles of rebuilt objects of changed plan objects], totals: {added, removed, moved, kept, conflicts, changed}, offset, limit, next?: offset of the next page, last?: the last revision's totals when this one found nothing}")]
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
    /// The drawing as the build left it (shared, not copied): a plan unchanged since then needs
    /// no rebuild, even when the build was one step of a longer command.
    pub built: Option<BuiltDrawing>,
}

/// The drawing a build left; two are equal when they are the same drawing.
#[derive(Clone)]
pub struct BuiltDrawing(pub std::sync::Arc<cadaddy_doc::Drawing>);

impl PartialEq for BuiltDrawing {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for BuiltDrawing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BuiltDrawing({} objects)", self.0.model.len())
    }
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
            out.push(PlanItem { kind, handle: e.handle, layer: e.common.layer.to_string(), name, signature, at });
        }
    }
    out
}

/// Above this many model objects the plan's rooms are not traced for the record.
const MAX_TRACED_OBJECTS: usize = 200_000;

/// The rooms of the current plan as plan items (name labels in closed outlines), read once. A
/// room's signature is its name, area and centre, so a room moved without changing size is a
/// change too. (Paging through `plan.rooms` traced every room again for each page of 200.)
fn room_items(s: &mut Session) -> Vec<PlanItem> {
    // A whole print set in one drawing (a guided job's sheets): tracing it took 11 min on
    // Windows and found no rooms. Skipped there; both record and revise read it the same way.
    if s.doc().map_or(true, |d| d.model.len() > MAX_TRACED_OBJECTS) {
        return Vec::new();
    }
    let Ok(rooms) = super::perceive::geometry::rooms_found(s) else { return Vec::new() };
    rooms
        .into_iter()
        .take(MAX_ROOM_PAGES * 200)
        .map(|r| PlanItem {
            kind: "room",
            handle: r.label,
            layer: String::new(),
            signature: format!("{}|{:.3}|{:.3}|{:.3}", r.name.to_ascii_uppercase(), r.area, r.at.x, r.at.y),
            name: Some(r.name),
            at: r.at,
        })
        .collect()
}

/// The rooms of another drawing (another version of the plan), read as [`room_items`] reads them.
fn rooms_of(other: Drawing) -> Vec<PlanItem> {
    let mut t = Session::empty();
    t.open_drawing(other, "Plan", None);
    room_items(&mut t)
}

/// Record what the build just made (and the plan it made it from).
pub(crate) fn record(s: &mut Session, g: &WallGen) {
    record_with(s, g, None);
}

/// [`record`], with the plan's rooms when they were just read (`None`: read them).
fn record_with(s: &mut Session, g: &WallGen, rooms: Option<Vec<PlanItem>>) {
    // The overrides go back on the regenerated model first: what they change is no hand edit.
    super::overrides::after_build(s);
    let Ok(d) = s.doc() else { return };
    let mut plan = plan_items(d, g);
    let made: HashMap<Handle, (u64, Option<TagKey>)> =
        d.model.iter().filter(|e| wallgen::made_as(e).is_some()).map(|e| (e.handle, (fingerprint(e), tag_key(e)))).collect();
    plan.extend(rooms.unwrap_or_else(|| room_items(s)));
    if let Ok(st) = s.state_mut() {
        let built = Some(BuiltDrawing(std::sync::Arc::clone(&st.doc)));
        st.model_record = Some(ModelRecord { plan, made, built });
    }
}

/// After an override changed generated objects outside a build: record them as they are now, so
/// a rebuild doesn't take the override's change for a hand edit (or a deletion by hand).
pub(crate) fn resync(s: &mut Session, touched: &[Handle]) {
    if touched.is_empty() {
        return;
    }
    let Ok(d) = s.doc() else { return };
    let now: Vec<(Handle, Option<(u64, Option<TagKey>)>)> =
        touched.iter().map(|h| (*h, d.model.get(*h).filter(|e| wallgen::made_as(e).is_some()).map(|e| (fingerprint(e), tag_key(e))))).collect();
    let Ok(st) = s.state_mut() else { return };
    let Some(rec) = st.model_record.as_mut() else { return };
    for (h, v) in now {
        match v {
            Some(v) => {
                rec.made.insert(h, v);
            }
            None => {
                rec.made.remove(&h);
            }
        }
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

/// A generated object's (part, level, source) tag: what the rebuilt counterpart shares. One plan
/// object makes many MEP objects (a run's segments, each fitting, terminals read from nothing),
/// so theirs adds the object's own id (`mepid:`) to the source: `<source>#<id>`.
pub(crate) fn tag_key(e: &Entity) -> Option<TagKey> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(wallgen::APP))?;
    let v = |k: usize| x.get(at + k).filter(|t| t.code == 1000).map(|t| t.value.clone());
    let part = v(1)?;
    let mut source = v(3).unwrap_or_default();
    if part == "mep"
        && let Some(id) = x.iter().skip(at + 4).take_while(|t| t.code != 1001).find_map(|t| t.value.strip_prefix("mepid:"))
    {
        source = format!("{source}#{id}");
    }
    Some((part, v(2).unwrap_or_default(), source))
}

/// The plan object a tag key's source names (before an MEP object's `#<id>`).
fn key_source(source: &str) -> Option<Handle> {
    Handle::parse_hex(source.split('#').next().unwrap_or_default())
}

/// What a rebuild kept of the hand edits: (kept handles, conflicts).
pub struct Kept {
    pub kept: Vec<Handle>,
    pub conflicts: Vec<Value>,
}

/// Build again from `g`, putting back the hand edits since the last build whose plan objects
/// didn't change (`changed`: the plan objects that did). The rebuilt objects of unchanged plan
/// objects keep the handles they had. `rooms`: the plan's rooms, when just read.
pub(crate) fn rebuild_keeping_edits(s: &mut Session, g: &WallGen, changed: &HashSet<Handle>, rooms: Option<Vec<PlanItem>>) -> Result<(Value, Kept)> {
    let before = s.state()?.model_record.clone();
    // The edits: generated objects changed since the build, and ones deleted.
    let mut edited: Vec<Entity> = Vec::new();
    let mut deleted: Vec<TagKey> = Vec::new();
    // The untouched generated objects of unchanged plan objects, by tag, in drawing order: their
    // rebuilt counterparts take these handles back.
    let mut handles_of: HashMap<TagKey, std::collections::VecDeque<Handle>> = HashMap::new();
    if let Some(rec) = &before {
        let d = s.doc()?;
        for e in d.model.iter().filter(|e| wallgen::made_as(e).is_some()) {
            match rec.made.get(&e.handle) {
                Some((f, _)) if *f != fingerprint(e) => edited.push((**e).clone()),
                Some((_, Some(key))) if !key_source(&key.2).is_some_and(|h| changed.contains(&h)) => {
                    handles_of.entry(key.clone()).or_default().push_back(e.handle);
                }
                _ => {}
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
        let src = key_source(&key.2);
        if src.is_some_and(|h| changed.contains(&h)) {
            kept.conflicts.push(json!({ "part": key.0, "source": key.2.split('#').next().unwrap_or_default(), "change": "deleted" }));
            continue;
        }
        let d = s.doc_mut()?;
        let twins: Vec<Handle> = d.model.iter().filter(|x| tag_key(x).as_ref() == Some(&key)).map(|x| x.handle).collect();
        for h in twins {
            d.remove_entity(h);
        }
    }
    keep_handles(s.doc_mut()?, handles_of);
    record_with(s, g, rooms);
    Ok((built, kept))
}

/// Give rebuilt objects back the handles their counterparts had before the build (`handles_of`,
/// by tag), when free: anything that refers to an unchanged part by handle still finds it.
fn keep_handles(d: &mut Drawing, mut handles_of: HashMap<TagKey, std::collections::VecDeque<Handle>>) {
    if handles_of.is_empty() {
        return;
    }
    let mut moves: Vec<(Handle, Handle)> = Vec::new();
    for e in d.model.iter().filter(|e| wallgen::made_as(e).is_some()) {
        let Some(old) = tag_key(e).and_then(|k| handles_of.get_mut(&k)).and_then(std::collections::VecDeque::pop_front) else { continue };
        if old != e.handle {
            moves.push((e.handle, old));
        }
    }
    for (now, old) in moves {
        if d.entity(old).is_some() {
            continue;
        }
        if let Some(e) = d.remove_entity(now) {
            d.model.push(Entity { handle: old, ..(*e).clone() });
        }
    }
}

fn item_json(i: &PlanItem) -> Value {
    json!({ "kind": i.kind, "handle": i.handle.hex(), "layer": i.layer, "name": i.name, "at": [i.at.x, i.at.y] })
}

/// How to revise: match plan objects by handle (the plan the model was built from) or not
/// (another file), how far a moved object may go (`None`: 5 % of the plans' size), whether to
/// build again, and whether an associative model's rebuild asked.
pub(crate) struct Revise {
    pub by_handle: bool,
    pub tolerance: Option<f64>,
    pub rebuild: bool,
    pub auto: bool,
}

/// Compare `old` (its plan objects) and `old_rooms` with the plan now and, when they differ and
/// `how.rebuild`, build again keeping the hand edits; select the rebuilt objects of changed plan
/// objects and echo a summary with the conflicts. The whole report (every list), also kept as
/// the drawing's last revision when it found changes.
pub(crate) fn revise(s: &mut Session, g: &WallGen, old: Vec<PlanItem>, old_rooms: Vec<PlanItem>, how: &Revise) -> Result<Value> {
    let now = plan_items(s.doc()?, g);
    let rooms_now = room_items(s);
    // The default reach: from the plan objects only (not the model, which moves with them).
    let reach = how.tolerance.unwrap_or_else(|| {
        let b = Bounds2::from_points(old.iter().chain(&now).chain(&old_rooms).chain(&rooms_now).map(|i| i.at));
        (b.width().max(b.height()) * 0.05).max(1e-6)
    });
    let (added, removed, moved, unchanged) = diff(&old, &now, how.by_handle, reach);
    // Rooms: by name, size and centre (their labels' handles don't follow a room across files).
    let (r_added, r_removed, r_moved, r_unchanged) = diff(&old_rooms, &rooms_now, false, reach);
    let changed_sources: HashSet<Handle> =
        removed.iter().map(|i| i.handle).chain(moved.iter().flat_map(|(o, n)| [o.handle, n.handle])).chain(added.iter().map(|i| i.handle)).collect();
    let any = !(added.is_empty() && removed.is_empty() && moved.is_empty() && r_added.is_empty() && r_removed.is_empty() && r_moved.is_empty());
    let rebuild = how.rebuild && any;
    let mut out = json!({
        "added": added.iter().chain(&r_added).map(item_json).collect::<Vec<_>>(),
        "removed": removed.iter().chain(&r_removed).map(item_json).collect::<Vec<_>>(),
        "moved": moved.iter().chain(&r_moved).map(|(o, n)| { let mut v = item_json(n); v["from"] = json!([o.at.x, o.at.y]); v }).collect::<Vec<_>>(),
        "unchanged": unchanged + r_unchanged,
        "rebuilt": rebuild,
        "auto": how.auto,
        "kept": [],
        "conflicts": [],
        "changed": [],
    });
    let (n_added, n_removed, n_moved) = (added.len() + r_added.len(), removed.len() + r_removed.len(), moved.len() + r_moved.len());
    if rebuild {
        let (built, kept) = rebuild_keeping_edits(s, g, &changed_sources, Some(rooms_now))?;
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
        let conflicts = kept.conflicts.len();
        s.echo(format!(
            "Plan changes: {n_added} added, {n_removed} removed, {n_moved} moved. Rebuilt; {} edit(s) kept, {conflicts} conflict(s).{}",
            kept.kept.len(),
            if conflicts > 0 { " The rebuilt parts replaced the conflicting edits: model.revise {last: true} lists them." } else { "" }
        ));
    }
    if any {
        s.state_mut()?.last_revision = Some(out.clone());
    }
    Ok(out)
}

/// One page of a revision report: each list from `offset`, at most `limit` items, with every
/// list's total, and the next page's offset while any list has more.
fn paged(report: &Value, p: &Value) -> Value {
    let offset = p.get("offset").and_then(Value::as_u64).map_or(0, |o| usize::try_from(o).unwrap_or(usize::MAX));
    let limit = p.get("limit").and_then(Value::as_u64).map_or(PAGE, |l| usize::try_from(l).unwrap_or(PAGE_MAX)).clamp(1, PAGE_MAX);
    let mut out = report.clone();
    let mut totals = serde_json::Map::new();
    let mut more = false;
    for key in LISTS {
        let list = report.get(key).and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        totals.insert(key.into(), json!(list.len()));
        more |= list.len() > offset.saturating_add(limit);
        out[key] = json!(list.iter().skip(offset).take(limit).collect::<Vec<_>>());
    }
    out["totals"] = Value::Object(totals);
    out["offset"] = json!(offset);
    out["limit"] = json!(limit);
    if more {
        out["next"] = json!(offset.saturating_add(limit));
    }
    out
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "model.revise";
    if p.get("last").and_then(Value::as_bool).unwrap_or(false) {
        // The last revision includes the one the plan's latest edits are waiting for.
        wallgen::update_model(s);
        return match &s.state()?.last_revision {
            Some(r) => Ok(paged(r, p)),
            None => Err(bad(cmd, "no revision has found plan changes yet")),
        };
    }
    let g = s
        .state()?
        .wallgen
        .clone()
        .ok_or_else(|| crate::EngineError::Other("The drawing has no model yet: run model.build or WALLGEN first.".into()))?;
    let tolerance = match p.get("tolerance") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().filter(|t| t.is_finite() && *t >= 0.0).ok_or_else(|| bad(cmd, "`tolerance` must be a distance"))?),
    };
    // The plan to compare with: another file, or what the model was last built from.
    let (old, old_rooms, by_handle) = match str_param(p, "against") {
        Some(path) => {
            let hooks = super::file::io().ok_or_else(|| bad(cmd, "file formats are not available in this build"))?;
            let bytes = read_file(cmd, path)?;
            let other = (hooks.read)(&bytes, path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
            (plan_items(&other, &g), rooms_of(other), false)
        }
        None => match &s.state()?.model_record {
            Some(r) => {
                let (old, rooms) = r.plan.iter().cloned().partition(|i| i.kind != "room");
                (old, rooms, true)
            }
            None => return Err(bad(cmd, "no record of the plan the model was built from: build it once, or pass `against`")),
        },
    };
    let rebuild = p.get("rebuild").and_then(Value::as_bool).unwrap_or(true);
    let report = revise(s, &g, old, old_rooms, &Revise { by_handle, tolerance, rebuild, auto: false })?;
    let mut out = paged(&report, p);
    // Nothing changed since the last build: the last revision that found changes (an associative
    // model's rebuild, say), in short.
    let found = LISTS.iter().take(3).any(|k| report[*k].as_array().is_some_and(|l| !l.is_empty()));
    if !found && let Some(last) = &s.state()?.last_revision {
        let short = paged(last, &json!({"limit": 1}));
        out["last"] = json!({ "totals": short["totals"], "auto": last["auto"], "rebuilt": last["rebuilt"] });
    }
    Ok(out)
}
