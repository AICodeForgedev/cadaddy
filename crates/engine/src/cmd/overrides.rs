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
//! nothing.
//!
//! A copy a person makes of a generated element (COPY, paste, ARRAY, MIRROR, a grip copy) carries
//! its source's build tag, so a rebuild took it for the build's own and dropped it (Q6620: 347
//! devices of a corrected model). It is hand-made: recorded as an `add` override when made, with
//! the build tag `hand:copy` ([`hand_made`]), so no build removes it, no override of a generated
//! element lands on it, and every rebuild keeps it as it stands (its later moves and properties
//! included). A model whose copies were made before this was so has them found at its next
//! rebuild ([`keep_copies`]): more elements of one device id than the build makes. `override.show` hides them (the model as the rules generate it) and shows them again;
//! `override.revert` takes one back; `override.list` lists them by author, status or kind.

use std::collections::HashMap;

use cadaddy_doc::overrides::{self, Author, Change, ElementKey, Override, Overrides, Status};
use cadaddy_doc::{Drawing, Entity, Handle, bimspec, entity_bounds};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

const ADD: &str = "override.add";

/// The build tag a copy made by hand of a generated element carries (`hand:copy`).
pub(crate) const HAND_TAG: &str = "hand";

/// Most hand-made copies recorded by one command (a paste of a whole floor's devices is a few
/// thousand); more are left as they are, and the next rebuild finds them ([`keep_copies`]).
const MAX_ADOPT: usize = 50_000;

/// The commands that make copies of what they act on: a generated element they add is a copy a
/// person made.
const COPYING: [&str; 17] = [
    "copy",
    "copym",
    "ncopy",
    "copytolayer",
    "mirror",
    "3dmirror",
    "array",
    "arrayrect",
    "arraypolar",
    "arraypath",
    "arrayclassic",
    "3darray",
    "pasteclip",
    "pasteorig",
    "pastespec",
    "grip",
    "grip.move",
];

/// The commands that move what they act on (a person's corrections; `bim.props.batch` moves the
/// elements a step finds): a generated element they only displaced is recorded as moved, so a
/// rebuild moves it again.
const MOVING: [&str; 7] = ["move", "3dmove", "stretch", "align", "grip.move", "grip.stretch", "bim.props.batch"];

/// The notes of the overrides recorded from a person's edits as they made them (a copy, a move):
/// steps after the build, not what it reads ([`build_input`]).
const COPIED_BY_HAND: &str = "copied by hand";
const MOVED_BY_HAND: &str = "moved by hand";
const FOUND_AT_REBUILD: &str = "a copy made by hand before copies were recorded: kept at the rebuild";
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
    d.overrides =
        (if o.is_empty() && !o.hidden { None } else { Some(o.to_json().map_err(|e| crate::EngineError::Other(format!("overrides: {e}")))?) }).into();
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

/// One chunk's plan objects (anything not generated and not added by an override): how many
/// it holds, and those with bounds, with their place among them, bounds and a signature of
/// their geometry and layer, independent of handles.
struct SourceChunk {
    /// Per object of the chunk: `None` not a plan object, else its bounds and signature (`None`:
    /// no bounds).
    each: Vec<Option<Option<(Bounds2, u64)>>>,
    count: usize,
    items: Vec<(usize, Bounds2, u64)>,
    /// The union of the items' bounds.
    bounds: Bounds2,
}

impl SourceChunk {
    /// `before`: the chunk before an edit; the objects it did not touch keep what they had.
    fn build(d: &Drawing, items: &[std::sync::Arc<Entity>], before: Option<(&[std::sync::Arc<Entity>], &SourceChunk)>) -> SourceChunk {
        let old: HashMap<usize, usize> =
            before.map(|(items, _)| items.iter().enumerate().map(|(i, e)| (std::sync::Arc::as_ptr(e) as usize, i)).collect()).unwrap_or_default();
        let each: Vec<Option<Option<(Bounds2, u64)>>> = items
            .iter()
            .map(|e| {
                if let Some(kept) = before.zip(old.get(&(std::sync::Arc::as_ptr(e) as usize))).and_then(|((_, b), &i)| b.each.get(i)) {
                    return *kept;
                }
                if wallgen::made_as(e).is_some() || !overrides::applied_ids(&e.common.xdata).is_empty() {
                    return None;
                }
                let b = entity_bounds(d, e, 0);
                Some((!b.is_empty()).then(|| (b, fnv(format!("{:?}|{}", e.kind, e.common.layer.to_ascii_uppercase()).as_bytes(), FNV0))))
            })
            .collect();
        let mut out = SourceChunk { each: Vec::new(), count: 0, items: Vec::new(), bounds: Bounds2::EMPTY };
        for x in each.iter().flatten() {
            if let Some((b, sig)) = x {
                out.bounds = out.bounds.union(b);
                out.items.push((out.count, *b, *sig));
            }
            out.count += 1;
        }
        out.each = each;
        out
    }
}

/// What else of the drawing an object's bounds depend on (block definitions, dimension styles,
/// the annotation scale).
#[derive(PartialEq)]
struct SourceContext {
    blocks: Vec<(String, usize)>,
    dim_styles: Vec<cadaddy_doc::DimStyle>,
    dimscale: u64,
    anno: (u64, bool),
}

impl SourceContext {
    fn of(d: &Drawing) -> SourceContext {
        SourceContext {
            blocks: d.blocks.iter().map(|(k, b)| (k.clone(), std::sync::Arc::as_ptr(b) as usize)).collect(),
            dim_styles: d.dim_styles.clone(),
            dimscale: d.header.f64("DIMSCALE", 1.0).to_bits(),
            anno: (d.current_anno_scale().factor().to_bits(), d.anno_all_visible()),
        }
    }
}

static SOURCES: crate::chunkpool::ChunkPool<SourceChunk, SourceContext> = crate::chunkpool::ChunkPool::new();

/// Let go of the kept plan objects (a drawing closed).
pub(crate) fn clear_sources() {
    SOURCES.clear();
}

/// Let go of the kept plan objects of chunks no open drawing holds ([`crate::Session::release_unheld`]).
pub(crate) fn retain_held(held: &std::collections::HashSet<usize>) {
    SOURCES.retain_held(held);
}

/// The model's plan objects by chunk (the first [`MAX_SOURCES`] of them), kept per chunk while it
/// is unchanged: a first edit of an element (Q6438) looked at every object of the model.
struct Sources {
    parts: Vec<crate::chunkpool::Part<SourceChunk>>,
}

fn sources(d: &Drawing) -> Sources {
    let (parts, _r) =
        SOURCES.parts(&d.model, SourceContext::of(d), |c, before| SourceChunk::build(d, c.items(), before.map(|b| (b.chunk.items(), &*b.data))));
    #[cfg(test)]
    tests::LAST.with(|l| l.set(_r));
    Sources { parts }
}

/// A signature of what the plan holds in `area`.
fn area_sig(sources: &Sources, area: &Bounds2) -> u64 {
    let mut sigs: Vec<u64> = Vec::new();
    let mut first = 0usize;
    for p in &sources.parts {
        let c: &SourceChunk = &p.data;
        let left = MAX_SOURCES.saturating_sub(first);
        first = first.saturating_add(c.count);
        if left == 0 || !c.bounds.intersects(area) {
            continue;
        }
        sigs.extend(c.items.iter().filter(|(at, b, _)| *at < left && b.intersects(area)).map(|(_, _, s)| *s));
    }
    sigs.sort_unstable();
    sigs.iter().fold(FNV0, |h, s| fnv(&s.to_le_bytes(), h))
}

/// A generated element as the index sees it.
struct Made {
    handle: Handle,
    part: String,
    level: String,
    source: String,
    /// Its own identity: the source with the element's id when it has one (`<source>#<devid>`).
    twin: String,
    at: Vec2,
}

/// The generated elements in model space (a copy made by hand is none: [`hand_made`]).
fn index(d: &Drawing) -> Vec<Made> {
    d.model
        .iter()
        .filter_map(|e| {
            let (part, level, source) = super::revise3d::tag_key(e)?;
            let twin = super::revise3d::twin_key(e).map_or_else(|| source.clone(), |k| k.2);
            Some(Made { handle: e.handle, part, level, source, twin, at: entity_bounds(d, e, 0).center() })
        })
        .collect()
}

/// Whether a key's source names no plan object (`0`: devices and equipment made from the spec's
/// tags, which all share it): it says nothing about which element it was.
fn sourceless(source: &str) -> bool {
    Handle::parse_hex(source.split('#').next().unwrap_or_default()).is_none_or(|h| h.0 == 0)
}

/// The element `key` names: the one of its own identity (its plan object and id), else the
/// plan object's (the one it made, however far it is now), else — the plan object redrawn, or a
/// new revision from another file — the nearest of its part on its level within `reach`. A key
/// whose plan object made many elements of its part (a sheet's devices; no plan object at all,
/// `0`, for the spec's tags) names none of them by it, only by place: keyed before keys carried
/// the element's id, it named the nearest of them however far, and a copy's circuit went onto a
/// thermostat across the floor (Q6620).
fn find(index: &[Made], key: &ElementKey, reach: f64) -> Option<Handle> {
    let at = Vec2::new(key.at[0], key.at[1]);
    let nearest =
        |it: &mut dyn Iterator<Item = &Made>| it.map(|m| (m.handle, m.at.dist(at))).filter(|(_, d)| d.is_finite()).min_by(|a, b| a.1.total_cmp(&b.1));
    let same = |m: &&Made| m.part == key.part && m.level == key.level;
    if let Some((h, _)) = nearest(&mut index.iter().filter(same).filter(|m| m.twin == key.source)) {
        return Some(h);
    }
    if !sourceless(&key.source) {
        let mut of_source = index.iter().filter(same).filter(|m| m.source == key.source);
        if let (Some(one), None) = (of_source.next(), of_source.next()) {
            return Some(one.handle);
        }
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
/// Whether a properties override sets what an element's circuit links are derived from (its
/// circuit or panel): applied or taken off, the links follow (Q6501).
fn moves_circuit(o: &Override) -> bool {
    matches!(&o.change, Change::Props { props, .. } if props.iter().any(|en| super::circuitlinks::is_input(&en.key)))
}

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
            let from = d.entity(h).and_then(|e| Some((super::bspec::made_level(e)?, super::relevel::bottom(e)?)));
            d.modify_entity(h, |e| {
                translated(e, dl);
                e.common.xdata = overrides::mark(&e.common.xdata, id).into();
            })
            .ok()?;
            // Moved to another storey: filed under it (Q6491), after every build too.
            if let Some((from, bottom)) = from {
                super::relevel::refile_moved(d, h, &from, bottom, dl[2]);
            }
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
            let from = d.entity(h).and_then(|e| Some((super::bspec::made_level(e)?, super::relevel::bottom(e)?)));
            d.modify_entity(h, |e| {
                translated(e, back);
                e.common.xdata = overrides::unmark(&e.common.xdata, id).into();
            })
            .ok()?;
            if let Some((from, bottom)) = from {
                super::relevel::refile_moved(d, h, &from, bottom, back[2]);
            }
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
    /// The added items this pass put in.
    pub added: Vec<Handle>,
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
    let mut relink: Vec<Handle> = Vec::new();
    for e in d.model.iter() {
        for id in overrides::applied_ids(&e.common.xdata) {
            by_id.entry(id).or_insert(e.handle);
        }
    }
    // The added items first (Q6620): a copy made by hand comes back with the marks of the
    // overrides set on it since (its properties), which then find it in place, not the nearest
    // generated element of its kind.
    let is_add = |o: &Override| matches!(o.change, Change::Add { .. });
    let (adds, rest): (Vec<usize>, Vec<usize>) = (0..ov.items.len()).partition(|&i| ov.items.get(i).is_some_and(is_add));
    let n_adds = adds.len();
    for (k, i) in adds.into_iter().chain(rest).enumerate() {
        if k == n_adds {
            for h in &out.added {
                if let Some(e) = d.entity(*h) {
                    for id in overrides::applied_ids(&e.common.xdata) {
                        by_id.entry(id).or_insert(*h);
                    }
                }
            }
        }
        let Some(o) = ov.items.get_mut(i) else { continue };
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
                if let Some(h) = o.applied {
                    out.applied.push(o.id);
                    if matches!(o.change, Change::Add { .. }) {
                        out.added.push(h);
                    }
                    if moves_circuit(o) {
                        relink.push(h);
                    }
                }
            }
        }
        match o.status {
            Status::Active => out.active += 1,
            Status::Review => out.review.push(o.id),
            Status::Orphaned => out.orphaned.push(o.id),
        }
    }
    super::circuitlinks::relink(d, &relink);
    d.overrides = Some(ov.to_json().map_err(|e| format!("overrides: {e}"))?).into();
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
    let mut src: Option<Sources> = None;
    // Per element: its new properties, and the override they go in (an earlier one's id, or a
    // new one).
    let mut plan: Vec<(Handle, &[cadaddy_doc::props::Entry], std::result::Result<u32, Override>)> = Vec::new();
    for (h, props) in edits {
        let Some(e) = d.model.get(*h) else { continue };
        let Some((part, level, source)) = super::revise3d::twin_key(e) else { continue };
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
    let relink: Vec<Handle> =
        plan.iter().filter(|(_, props, _)| props.iter().any(|en| super::circuitlinks::is_input(&en.key))).map(|(h, ..)| *h).collect();
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
    super::circuitlinks::relink(d, &relink);
    save(d, &ov)?;
    super::revise3d::resync(s, &touched);
    Ok(out)
}

// ---------- copies made by hand (Q6620) ----------

/// Whether `e` is a copy a person made of a generated element (recorded as an `add` override).
pub(crate) fn hand_made(e: &Entity) -> bool {
    super::bimlook::made_tag(e, HAND_TAG).is_some()
}

/// `xdata` with the build tag [`HAND_TAG`] added to its build block (once).
fn with_hand_tag(xdata: &[cadaddy_doc::RawTag]) -> Vec<cadaddy_doc::RawTag> {
    let mut out = xdata.to_vec();
    let Some(at) = out.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(wallgen::APP)) else { return out };
    let end = out.iter().skip(at + 1).position(|t| t.code == 1001).map_or(out.len(), |n| at + 1 + n);
    let has = out
        .get(at + 1..end)
        .is_some_and(|b| b.iter().any(|t| t.code == 1000 && t.value.split_once(':').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(HAND_TAG))));
    if !has {
        out.insert(end, cadaddy_doc::RawTag { code: 1000, value: format!("{HAND_TAG}:copy") });
    }
    out
}

/// Record the generated-looking objects `handles` (copies a person made) as hand-made: each
/// gets the [`HAND_TAG`] build tag and an `add` override of its own (the marks of its source's
/// overrides come off: they are the source's). Returns the overrides' ids.
pub(crate) fn adopt(d: &mut Drawing, handles: &[Handle], author: Author, note: &str) -> std::result::Result<Vec<u32>, String> {
    let mut ov = load(d)?;
    if ov.hidden {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for h in handles.iter().take(MAX_ADOPT) {
        let Some(e) = d.model.get(*h) else { continue };
        if wallgen::made_as(e).is_none() {
            continue;
        }
        let b = entity_bounds(d, e, 0);
        if b.is_empty() || ov.items.len() >= overrides::MAX_OVERRIDES {
            continue;
        }
        let Some(id) = ov.take_id() else { break };
        let xdata = overrides::with_ids(&with_hand_tag(&e.common.xdata), &[id]);
        if d.modify_entity(*h, |x| x.common.xdata = xdata.clone().into()).is_err() {
            continue;
        }
        let Some(now) = d.model.get(*h).map(|e| (**e).clone()) else { continue };
        ov.items.push(Override {
            id,
            author,
            note: Some(note.chars().take(overrides::MAX_NOTE).collect()),
            change: Change::Add { entity: Box::new(now) },
            target: None,
            area: rect(&area_around(&b)),
            area_sig: 0,
            status: Status::Active,
            applied: Some(*h),
        });
        ids.push(id);
    }
    if ids.is_empty() {
        return Ok(ids);
    }
    let src = sources(d);
    for o in ov.items.iter_mut().filter(|o| ids.contains(&o.id)) {
        o.area_sig = area_sig(&src, &bounds_of(&o.area));
    }
    d.overrides = Some(ov.to_json().map_err(|e| format!("overrides: {e}"))?).into();
    Ok(ids)
}

/// After a command called from outside: the generated elements a copying command ([`COPYING`])
/// added are copies a person made, recorded as such; a hand-made copy changed since is kept as
/// it is now (its `add` override's object), and one erased is let go (its override taken away).
pub(crate) fn after_command(s: &mut Session, before: Option<&std::sync::Arc<Drawing>>) {
    if !s.journal.at_top() {
        return;
    }
    let Some(before) = before else { return };
    let cmd = s.outer_command.to_ascii_lowercase();
    let Ok(d) = s.doc() else { return };
    // Nothing to record (no copying or moving command), and no hand-made copy to follow (no
    // overrides at all): no comparing the model with the one before.
    let acts = COPYING.contains(&cmd.as_str()) || MOVING.contains(&cmd.as_str()) || cmd.starts_with("grip");
    if (!acts && d.overrides.is_none()) || std::ptr::eq(before.as_ref(), d) || d.model.unchanged_from(&before.model) {
        return;
    }
    let (added, modified, erased) = before.model.changes(&d.model);
    let copying = COPYING.contains(&cmd.as_str()) || (cmd.starts_with("grip.") && !added.is_empty());
    let copies: Vec<Handle> =
        if copying { added.iter().copied().filter(|h| d.model.get(*h).is_some_and(|e| wallgen::made_as(e).is_some())).collect() } else { Vec::new() };
    let hand = |dr: &Drawing, h: &Handle| dr.model.get(*h).is_some_and(|e| hand_made(e));
    let changed: Vec<Handle> = modified.iter().chain(&added).copied().filter(|h| !copies.contains(h) && hand(d, h)).collect();
    let gone: Vec<Handle> = erased.iter().copied().filter(|h| hand(before, h)).collect();
    let built = |dr: &Drawing, h: &Handle| dr.model.get(*h).is_some_and(|e| super::revise3d::built_object(e));
    // Generated elements a person moved (Q6620): moved again after every rebuild.
    let moves: Vec<(Handle, [f64; 3])> = if MOVING.contains(&cmd.as_str()) {
        modified
            .iter()
            .filter(|h| built(before, h) && built(d, h))
            .filter_map(|h| Some((*h, displacement(before, before.model.get(*h)?, d, d.model.get(*h)?)?)))
            .collect()
    } else {
        Vec::new()
    };
    if copies.is_empty() && changed.is_empty() && gone.is_empty() && moves.is_empty() {
        return;
    }
    let gone_ids: Vec<u32> = gone.iter().filter_map(|h| before.model.get(*h)).flat_map(|e| overrides::applied_ids(&e.common.xdata)).collect();
    let Ok(d) = s.doc_mut() else { return };
    if !copies.is_empty() {
        let note = format!("{COPIED_BY_HAND} ({cmd})");
        if let Err(e) = adopt(d, &copies, Author::User, &note) {
            s.echo(e);
            return;
        }
    }
    if !moves.is_empty() {
        let mut touched = Vec::new();
        if let Err(e) = record_moves(d, &moves, &cmd, &mut touched) {
            s.echo(e);
            return;
        }
        super::revise3d::resync(s, &touched);
    }
    if changed.is_empty() && gone_ids.is_empty() {
        return;
    }
    let Ok(d) = s.doc_mut() else { return };
    let Ok(mut ov) = load(d) else { return };
    let mut dirty = false;
    // Still in the model under the mark (a rebuild put it back elsewhere): not erased.
    let present: std::collections::HashSet<u32> =
        if gone_ids.is_empty() { Default::default() } else { d.model.iter().flat_map(|e| overrides::applied_ids(&e.common.xdata)).collect() };
    for id in gone_ids {
        if !present.contains(&id) && matches!(ov.get(id).map(|o| &o.change), Some(Change::Add { .. })) {
            ov.remove(id);
            dirty = true;
        }
    }
    for h in changed {
        let Some(e) = d.model.get(h) else { continue };
        for id in overrides::applied_ids(&e.common.xdata) {
            if let Some(o) = ov.get_mut(id)
                && let Change::Add { entity } = &mut o.change
            {
                **entity = (**e).clone();
                o.applied = Some(h);
                dirty = true;
            }
        }
    }
    if dirty && let Ok(t) = ov.to_json() {
        d.overrides = Some(t).into();
    }
}

/// How far `b` (in `db`) stands from `a` (in `da`) when it is `a` displaced and nothing else: a
/// mesh's every vertex by one vector, any other object its extents. `None`: not displaced (still,
/// turned, reshaped, or put on another layer).
fn displacement(da: &Drawing, a: &Entity, db: &Drawing, b: &Entity) -> Option<[f64; 3]> {
    if a.common.layer != b.common.layer || std::mem::discriminant(&a.kind) != std::mem::discriminant(&b.kind) {
        return None;
    }
    let (ba, bb) = (entity_bounds(da, a, 0), entity_bounds(db, b, 0));
    if ba.is_empty() || bb.is_empty() {
        return None;
    }
    let size = ba.min.dist(ba.max).max(1.0);
    let tol = size * 1e-9 + 1e-9;
    let d = match (&a.kind, &b.kind) {
        (cadaddy_doc::EntityKind::Mesh(ma), cadaddy_doc::EntityKind::Mesh(mb)) => {
            if ma.vertices.len() != mb.vertices.len() || ma.faces != mb.faces {
                return None;
            }
            let (p, q) = (ma.vertices.first()?, mb.vertices.first()?);
            let d = Vec3 { x: q.x - p.x, y: q.y - p.y, z: q.z - p.z };
            let same = ma
                .vertices
                .iter()
                .zip(&mb.vertices)
                .all(|(p, q)| (q.x - p.x - d.x).abs() <= tol && (q.y - p.y - d.y).abs() <= tol && (q.z - p.z - d.z).abs() <= tol);
            if !same {
                return None;
            }
            [d.x, d.y, d.z]
        }
        _ => {
            let (ca, cb) = (ba.center(), bb.center());
            let d = [cb.x - ca.x, cb.y - ca.y, 0.0];
            let mut moved = a.clone();
            translated(&mut moved, d);
            let bm = entity_bounds(da, &moved, 0);
            let near = |p: Vec2, q: Vec2| (p.x - q.x).abs() <= tol && (p.y - q.y).abs() <= tol;
            if !near(bm.min, bb.min) || !near(bm.max, bb.max) {
                return None;
            }
            d
        }
    };
    (d.iter().all(|v| v.is_finite()) && d.iter().any(|v| v.abs() > tol)).then_some(d)
}

/// Record generated elements a person displaced (`moves`: each by how far) as `move` overrides,
/// added to the one already on the element. The elements stay as the command left them.
fn record_moves(d: &mut Drawing, moves: &[(Handle, [f64; 3])], cmd: &str, touched: &mut Vec<Handle>) -> std::result::Result<(), String> {
    let mut ov = load(d)?;
    if ov.hidden {
        return Ok(());
    }
    let mut fresh: Vec<u32> = Vec::new();
    for (h, delta) in moves.iter().take(MAX_ADOPT) {
        let Some(e) = d.model.get(*h) else { continue };
        let ids = overrides::applied_ids(&e.common.xdata);
        // Moved before: the one move override grows.
        if let Some(o) =
            ids.iter().find_map(|id| ov.get(*id).filter(|o| matches!(o.change, Change::Move { .. })).map(|o| o.id)).and_then(|id| ov.get_mut(id))
            && let Change::Move { delta: was } = &mut o.change
        {
            for (w, x) in was.iter_mut().zip(delta) {
                *w += x;
            }
            continue;
        }
        let Some((part, level, source)) = super::revise3d::twin_key(e) else { continue };
        let now = entity_bounds(d, e, 0);
        if now.is_empty() || ov.items.len() >= overrides::MAX_OVERRIDES {
            continue;
        }
        // Where it was generated: before this move and the ones applied to it.
        let mut back = [delta[0], delta[1]];
        for id in &ids {
            if let Some(Change::Move { delta }) = ov.get(*id).map(|o| &o.change) {
                back[0] += delta[0];
                back[1] += delta[1];
            }
        }
        let bounds = Bounds2 { min: Vec2::new(now.min.x - back[0], now.min.y - back[1]), max: Vec2::new(now.max.x - back[0], now.max.y - back[1]) };
        let Some(id) = ov.take_id() else { break };
        let at = bounds.center();
        ov.items.push(Override {
            id,
            author: Author::User,
            note: Some(format!("{MOVED_BY_HAND} ({cmd})")),
            change: Change::Move { delta: *delta },
            target: Some(ElementKey { part, level, source, at: [at.x, at.y] }),
            area: rect(&area_around(&bounds)),
            area_sig: 0,
            status: Status::Active,
            applied: Some(*h),
        });
        fresh.push(id);
        let x = overrides::mark(&e.common.xdata, id);
        if d.modify_entity(*h, |e| e.common.xdata = x.clone().into()).is_ok() {
            touched.push(*h);
        }
    }
    if !fresh.is_empty() {
        let src = sources(d);
        for o in ov.items.iter_mut().filter(|o| fresh.contains(&o.id)) {
            o.area_sig = area_sig(&src, &bounds_of(&o.area));
        }
    }
    save(d, &ov).map_err(|e| e.to_string())
}

/// After a rebuild: the elements of the model before it (`old`: its generated ones) a person
/// copied before copies were recorded. One element is built per device id; the rebuilt model
/// has as many of an id as the build makes, so the old elements of an id beyond that — the ones
/// after the first by handle (a copy takes a newer one), at least `apart` from every rebuilt
/// element of their id — are copies a person made: put back as they stood and recorded as
/// hand-made. Returns how many.
pub(crate) fn keep_copies(d: &mut Drawing, old: &[std::sync::Arc<Entity>], apart: f64) -> usize {
    let id_of = |e: &Entity| -> Option<super::revise3d::TagKey> {
        super::bimlook::made_tag(e, "devid").filter(|i| !i.is_empty())?;
        super::revise3d::twin_key(e)
    };
    let mut was: HashMap<super::revise3d::TagKey, Vec<&std::sync::Arc<Entity>>> = HashMap::new();
    for e in old {
        if let Some(k) = id_of(e) {
            was.entry(k).or_default().push(e);
        }
    }
    was.retain(|_, v| v.len() > 1);
    if was.is_empty() {
        return 0;
    }
    let mut now: HashMap<super::revise3d::TagKey, Vec<Vec2>> = HashMap::new();
    for e in d.model.iter() {
        if let Some(k) = id_of(e).filter(|k| was.contains_key(k)) {
            now.entry(k).or_default().push(entity_bounds(d, e, 0).center());
        }
    }
    let mut back = Vec::new();
    for (k, mut v) in was {
        let Some(rebuilt) = now.get(&k) else { continue };
        if v.len() <= rebuilt.len() {
            continue;
        }
        v.sort_by_key(|e| e.handle.0);
        for e in v.into_iter().skip(rebuilt.len()) {
            let at = entity_bounds(d, e, 0).center();
            if !at.x.is_finite() || !at.y.is_finite() || rebuilt.iter().any(|p| p.dist(at) < apart) {
                continue;
            }
            let mut x = (**e).clone();
            if d.entity(x.handle).is_some() {
                x.handle = d.new_handle();
            } else {
                d.bump_handseed(x.handle);
            }
            x.common.xdata = overrides::with_ids(&x.common.xdata, &[]).into();
            back.push(x.handle);
            d.model.push(x);
        }
    }
    back.sort_by_key(|h| h.0);
    match adopt(d, &back, Author::User, FOUND_AT_REBUILD) {
        Ok(ids) => {
            // Not recorded (too many, or overrides hidden): not kept either.
            if ids.len() < back.len() {
                let kept: std::collections::HashSet<Handle> =
                    d.model.iter().filter(|e| hand_made(e) && back.contains(&e.handle)).map(|e| e.handle).collect();
                for h in back.iter().filter(|h| !kept.contains(h)) {
                    d.model.remove(*h);
                }
            }
            ids.len()
        }
        Err(_) => {
            for h in &back {
                d.model.remove(*h);
            }
            0
        }
    }
}

/// The drawing's overrides as a build's record weighs them (Q6403: what the build reads): the
/// ones recorded from a person's copies and moves as they made them are left out, as the edits
/// themselves are (steps after the build, in place already), and where the others are applied
/// now and how they stand is no input either. `None`: no overrides that count.
pub(crate) fn build_input(d: &Drawing) -> Option<String> {
    let text = d.overrides.as_deref()?;
    let Ok(ov) = Overrides::from_json(text) else { return Some(text.to_string()) };
    let by_hand =
        |o: &Override| o.note.as_deref().is_some_and(|n| n.starts_with(COPIED_BY_HAND) || n.starts_with(MOVED_BY_HAND) || n == FOUND_AT_REBUILD);
    let rest: Vec<Override> = ov.items.into_iter().filter(|o| !by_hand(o)).map(|o| Override { applied: None, status: Status::Active, ..o }).collect();
    (!rest.is_empty() || ov.hidden).then(|| format!("{}|{rest:?}", ov.hidden))
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
    let generated = super::revise3d::twin_key(&e);
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
    if let Some(h) = element.filter(|_| moves_circuit(&o)) {
        super::circuitlinks::relink(d, &[h]);
    }
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
        let mut relink = Vec::new();
        for i in (0..ov.items.len()).rev() {
            let Some(o) = ov.items.get(i) else { continue };
            if let Some(h) = unapply(d, o, &mut touched) {
                changed += 1;
                if moves_circuit(o) {
                    relink.push(h);
                }
            }
            if let Some(o) = ov.items.get_mut(i) {
                o.applied = None;
            }
        }
        super::circuitlinks::relink(d, &relink);
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    thread_local! {
        /// How many chunks the last [`sources`] on this thread worked out.
        pub(super) static LAST: std::cell::Cell<crate::chunkpool::Refresh> = std::cell::Cell::new(Default::default());
    }

    /// The signature as it was worked out before the per-chunk cache (Q6438): every plan object
    /// of the model on every call. The cache must give the same.
    fn sig_by_scan(d: &Drawing, area: &Bounds2) -> u64 {
        let src: Vec<(Bounds2, u64)> = d
            .model
            .iter()
            .filter(|e| wallgen::made_as(e).is_none() && overrides::applied_ids(&e.common.xdata).is_empty())
            .take(MAX_SOURCES)
            .filter_map(|e| {
                let b = entity_bounds(d, e, 0);
                (!b.is_empty()).then(|| (b, fnv(format!("{:?}|{}", e.kind, e.common.layer.to_ascii_uppercase()).as_bytes(), FNV0)))
            })
            .collect();
        let mut sigs: Vec<u64> = src.iter().filter(|(b, _)| b.intersects(area)).map(|(_, s)| *s).collect();
        sigs.sort_unstable();
        sigs.iter().fold(FNV0, |h, s| fnv(&s.to_le_bytes(), h))
    }

    fn made(h: u64, source: &str, twin: &str, x: f64) -> Made {
        Made { handle: Handle(h), part: "device".into(), level: "L1".into(), source: source.into(), twin: twin.into(), at: Vec2::new(x, 0.0) }
    }

    fn key(source: &str, x: f64) -> ElementKey {
        ElementKey { part: "device".into(), level: "L1".into(), source: source.into(), at: [x, 0.0] }
    }

    /// Q6620: an element is found by its own id; a plan object that made one element finds it
    /// however far it went; one that made many (or none, `0`) finds only what stands within reach.
    #[test]
    fn find_takes_the_elements_own_id_and_a_shared_source_only_within_reach() {
        let idx = [made(1, "A", "A#d1", 0.0), made(2, "A", "A#d2", 500.0), made(3, "B", "B", 900.0), made(4, "0", "0#d4", 40.0)];
        // Its own id, wherever it is.
        assert_eq!(find(&idx, &key("A#d2", 0.0), 1.0), Some(Handle(2)));
        // The one element of plan object B, moved far.
        assert_eq!(find(&idx, &key("B", 0.0), 1.0), Some(Handle(3)));
        // A key of the shared source A (written before keys carried ids): by place only.
        assert_eq!(find(&idx, &key("A", 300.0), 10.0), None);
        assert_eq!(find(&idx, &key("A", 495.0), 10.0), Some(Handle(2)));
        // No plan object: never the nearest of all of them however far.
        assert_eq!(find(&idx, &key("0", 200.0), 10.0), None);
        assert_eq!(find(&idx, &key("0", 45.0), 10.0), Some(Handle(4)));
    }

    #[test]
    fn the_cached_source_signature_is_the_full_scans() {
        let mut s = super::super::elemprops::tests::model(30_000, 500, 2, 4);
        let areas = [
            Bounds2 { min: Vec2::new(0.0, 0.0), max: Vec2::new(100.0, 100.0) },
            Bounds2 { min: Vec2::new(-5.0, -5.0), max: Vec2::new(4000.0, 300.0) },
            Bounds2 { min: Vec2::new(1e6, 1e6), max: Vec2::new(1e6 + 1.0, 1e6 + 1.0) },
            Bounds2 { min: Vec2::new(20_000.0, 0.0), max: Vec2::new(60_000.0, 2_000.0) },
            Bounds2::EMPTY,
        ];
        for a in &areas {
            let d = s.doc().unwrap();
            assert_eq!(area_sig(&sources(d), a), sig_by_scan(d, a), "{a:?}");
        }
        // An object moved: its chunk again, the same answer as a scan.
        let h = s.doc().unwrap().model.iter().nth(70).unwrap().handle;
        s.execute("move", &json!({"handles": [h.hex()], "delta": [3, 0]})).unwrap();
        for a in &areas {
            let d = s.doc().unwrap();
            assert_eq!(area_sig(&sources(d), a), sig_by_scan(d, a), "{a:?}");
        }
    }

    #[test]
    fn a_first_property_edit_of_an_element_does_not_look_at_every_plan_object_again() {
        let mut s = super::super::elemprops::tests::model(50_000, 2_000, 4, 20);
        let chunks = s.doc().unwrap().model.chunk_refs().len();
        let one = |s: &mut Session, loc: &str| {
            s.execute("bim.props.find", &json!({"kind": "device", "where": {"location": loc}, "select": true})).unwrap();
            s.execute("bim.props.set", &json!({"props": {"mounting": "18 in AFF"}})).unwrap();
            LAST.with(|l| l.get())
        };
        let first = one(&mut s, "Space 3");
        assert_eq!(first.built, chunks, "the first edit works the plan out once");
        let next = one(&mut s, "Space 40");
        assert!(next.built <= 2 && next.built + next.reused == chunks, "{next:?} of {chunks}");
    }
}
