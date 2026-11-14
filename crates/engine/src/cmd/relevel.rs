//! A built element's level follows where it stands (Q6491).
//!
//! A build files each element under a level (its WALLGEN tag), and the model tree, the element's
//! properties, takeoff by level, level exports and clash scopes all read that tag. An element
//! moved to another storey (`3dmove`, `move`, a grip, a moved copy, a `move` override) is filed
//! again under the storey it reached: the storey whose floor-to-floor band holds the place its
//! own level's floor moved to (so a slab, hung below its floor, moves by storeys like everything
//! else). It keeps its place in the building: a level whose plan is drawn elsewhere on the sheet
//! gets the element at the same building point.
//!
//! Where it stands is taken as it is (Q6628): an element below the lowest floor (a pit, a sump,
//! under the slab) is on the lowest level; one in a shaft or between floors (an overrun above
//! the top floor) on the level whose floor is the highest one under it. A level a person chose
//! (`bim.level.move`, or a `level` property they set that agrees) is kept: a later move does not
//! file the element again.
//!
//! - [`after_command`]: the pass after every command that moved or copied built elements.
//! - `bim.level.move {handle | handles (default the selection), level, keep_height?}`: moves
//!   elements to a level by name, keeping their height above their floor (an element already
//!   standing in that level's band, or with `keep_height`, is only filed there), and records the
//!   choice.
//! - [`stored_conflict`]: a stored `level` property that disagrees with the element's level
//!   (shown by `bim.element` as `level_conflict`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, RawTag};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const MOVE: &str = "bim.level.move";

/// Most changed objects one pass looks at (a bigger change is a build or an import, which files
/// its own elements).
const MAX_CHANGED: usize = 200_000;
/// Most model objects scanned for a copy's original.
const MAX_SCAN: usize = 2_000_000;
/// Most mesh vertices read for an element's height.
const MAX_VERTS: usize = 1_000_000;
/// How close (feet) a point may sit under a floor and still be on it.
const ON_FLOOR_FT: f64 = 0.05;
/// Most elements one `bim.level.move` moves.
const MOVE_MAX: usize = 100_000;
/// The note on a `level` property `bim.level.move` sets.
const CHOSEN: &str = "level chosen with bim.level.move: kept when the element is moved";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(MOVE, "Move to Level", run_move).params("{handle? | handles? (default: the selection), level: name, keep_height?: bool} → {level, moved, filed, kept, elements: [{handle, from, raised_ft}]}: built elements onto another level — raised or lowered by the floors' difference (the same height above their floor), at the same place in the building, filed under that level; one already standing in that level's band (a pit under the lowest floor, an overrun above the top one), or every one with keep_height, is filed there without moving. The level is kept as the person's choice (a `level` property of source user): later moves don't file it again"),
    ]
}

// ─── Where an element stands ─────────────────────────────────────────────────

/// The lowest and highest z of an object (drawing units), for the kinds a build makes.
fn z_range(e: &Entity) -> Option<(f64, f64)> {
    let r = match &e.kind {
        EntityKind::Mesh(m) => m
            .vertices
            .iter()
            .take(MAX_VERTS)
            .filter(|p| p.z.is_finite())
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.z), b.max(p.z))),
        EntityKind::Insert(i) => (i.insert.z, i.insert.z),
        EntityKind::Line(l) => (l.a.z.min(l.b.z), l.a.z.max(l.b.z)),
        EntityKind::Circle(c) => (c.center.z, c.center.z),
        EntityKind::Point(p) => (p.p.z, p.p.z),
        _ => return None,
    };
    (r.0.is_finite() && r.1.is_finite() && r.0 <= r.1).then_some(r)
}

/// The levels of a spec, lowest first, with finite elevations.
struct Storeys {
    upf: f64,
    levels: Vec<(String, f64)>,
    bases: Vec<(String, Vec2)>,
}

impl Storeys {
    fn of(d: &Drawing) -> Option<Storeys> {
        let sp = d.building_spec.as_deref().filter(|t| !t.trim().is_empty()).and_then(|t| BuildingSpec::from_json(t).ok())?;
        Self::of_spec(d, &sp)
    }

    fn of_spec(d: &Drawing, sp: &BuildingSpec) -> Option<Storeys> {
        let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        let mut levels: Vec<(String, f64)> =
            sp.levels.iter().filter(|l| l.elevation_ft.is_finite()).map(|l| (l.name.clone(), l.elevation_ft)).collect();
        if levels.len() < 2 {
            return None;
        }
        levels.sort_by(|a, b| a.1.total_cmp(&b.1));
        Some(Storeys { upf, levels, bases: cadaddy_walk3d::level_bases(d, &sp.to_rules()) })
    }

    fn named(&self, name: &str) -> Option<&(String, f64)> {
        self.levels.iter().find(|l| l.0.eq_ignore_ascii_case(name.trim()))
    }

    fn base(&self, name: &str) -> Vec2 {
        self.bases.iter().find(|b| b.0.eq_ignore_ascii_case(name.trim())).map_or(Vec2::ZERO, |b| b.1)
    }

    /// The level whose floor-to-floor band holds `z_ft` (feet above the datum): the highest floor
    /// at or under it; under the lowest floor, the lowest level. `keep` wins over another level
    /// with the same floor.
    fn holding(&self, z_ft: f64, keep: &str) -> Option<&(String, f64)> {
        let first = self.levels.first()?;
        let floor = self.levels.iter().rev().find(|l| l.1 <= z_ft + ON_FLOOR_FT).unwrap_or(first).1;
        self.levels
            .iter()
            .filter(|l| l.1 == floor)
            .find(|l| l.0.eq_ignore_ascii_case(keep))
            .or_else(|| self.levels.iter().rev().find(|l| l.1 == floor))
    }
}

/// `xdata` with the WALLGEN level tag set to `level`.
fn with_level(xdata: &[RawTag], level: &str) -> Vec<RawTag> {
    let mut x = xdata.to_vec();
    if let Some(at) = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP))
        && let Some(t) = x.get_mut(at + 2).filter(|t| t.code == 1000)
    {
        t.value = level.chars().take(255).collect();
    }
    x
}

/// Files built element `h` under `to` (from `from`), keeping its place in the building: when the
/// two levels' plans are drawn apart on the sheet, its stacking record takes the difference.
fn file_under(d: &mut Drawing, st: &Storeys, h: Handle, from: &str, to: &str) -> bool {
    let off = st.base(from) - st.base(to);
    d.modify_entity(h, |e| {
        let mut x = with_level(&e.common.xdata, to);
        if off.is_finite() && (off.x != 0.0 || off.y != 0.0) {
            x = super::bspec::with_stacked(&x, off);
        }
        e.common.xdata = x.into();
    })
    .is_ok()
}

/// Where an element of level `from` spanning `z` (drawing units) stands, in feet above the datum,
/// for filing it: its bottom; but an element hung under its own floor and reaching it (a slab, a
/// footing under its floor, a pit ladder through it) stands on that floor, so it moves by storeys
/// with it.
fn standing(st: &Storeys, from: &str, z: (f64, f64)) -> Option<f64> {
    let (bottom, top) = (z.0 / st.upf, z.1 / st.upf);
    if !(bottom.is_finite() && top.is_finite()) {
        return None;
    }
    match st.named(from) {
        Some((_, floor)) if bottom < floor - ON_FLOOR_FT && top >= floor - ON_FLOOR_FT => Some(*floor),
        _ => Some(bottom),
    }
}

/// After an element of level `from` spanning `z` moved up by `dz` (drawing units): the level it
/// is on now, when that is another level. Where it stands is taken as it is (Q6628): under the
/// lowest floor it is on the lowest level, in a shaft or between floors on the level whose floor
/// is the highest one under it; an element hung under its own floor counts from that floor.
fn moved_to<'a>(st: &'a Storeys, from: &str, z: (f64, f64), dz: f64) -> Option<&'a str> {
    let (name, _) = st.named(from)?;
    let reached = standing(st, name, z)? + dz / st.upf;
    if !reached.is_finite() {
        return None;
    }
    let to = st.holding(reached, name)?;
    (!to.0.eq_ignore_ascii_case(name)).then_some(to.0.as_str())
}

/// Whether a person chose element `e`'s level: a `level` property of source user (set by
/// `bim.level.move` or by hand) naming the level it is filed under. Such an element is not filed
/// again when it is moved (Q6628).
pub(crate) fn chosen(e: &Entity) -> bool {
    let Some(level) = super::bspec::made_level(e).filter(|l| !l.trim().is_empty()) else { return false };
    let store = cadaddy_doc::props::read(&e.common.xdata);
    store
        .get("level")
        .filter(|p| p.has_value() && p.source.kind == cadaddy_doc::props::SourceKind::User)
        .is_some_and(|p| p.text().trim().eq_ignore_ascii_case(level.trim()))
}

/// `xdata` with a person's `level` choice set to `level` (`Some`) or taken off (`None`).
fn with_choice(xdata: &[RawTag], level: Option<&str>) -> Vec<RawTag> {
    let mut store = cadaddy_doc::props::read(xdata);
    match level {
        Some(l) => {
            let mut p = cadaddy_doc::props::Prop::new(l.chars().take(255).collect::<String>(), cadaddy_doc::props::Source::user());
            p.note = Some(CHOSEN.into());
            store.set("level", p);
        }
        None => {
            store.remove("level");
        }
    }
    cadaddy_doc::props::write(xdata, &store)
}

/// An element's lowest and highest z (drawing units), for the kinds a build makes; `None` for
/// an element whose level a person chose (it is not filed again when moved).
pub(crate) fn span(e: &Entity) -> Option<(f64, f64)> {
    if chosen(e) {
        return None;
    }
    z_range(e)
}

/// Files element `h` of level `from`, which spanned `z` and moved up by `dz` (drawing units),
/// under the level it reached. The `move` override applies through this, so a rebuild files it
/// the same way.
pub(crate) fn refile_moved(d: &mut Drawing, h: Handle, from: &str, z: (f64, f64), dz: f64) -> bool {
    let Some(st) = Storeys::of(d) else { return false };
    match moved_to(&st, from, z, dz).map(str::to_string) {
        Some(to) => file_under(d, &st, h, from, &to),
        None => false,
    }
}

// ─── After every command ─────────────────────────────────────────────────────

/// The WALLGEN tags of an element that say which one it is (part, level, source and the rest),
/// for finding a copy's original.
fn identity(e: &Entity) -> Option<Vec<&str>> {
    super::bimlook::made_tags(e).map(|t| t.iter().map(|t| t.value.as_str()).collect())
}

/// The pass after every command: each built element the command moved up or down, and each copy
/// of a built element (the original still in the drawing) at another height than its original,
/// is filed under the level it reached. A build's own elements are left as the build filed them.
pub(crate) fn after_command(s: &mut Session, before: Option<&Arc<Drawing>>) {
    let Some(before) = before else { return };
    let Ok(st) = s.state() else { return };
    if Arc::ptr_eq(before, &st.doc) || before.model.unchanged_from(&st.doc.model) {
        return;
    }
    let now = Arc::clone(&st.doc);
    let built = st.model_records.for_command(before, &now).and_then(|r| r.built.as_ref());
    if built.is_some_and(|b| Arc::ptr_eq(&b.0, &now)) {
        return;
    }
    let (added, modified, _) = before.model.changes(&now.model);
    if added.len().saturating_add(modified.len()) > MAX_CHANGED {
        return;
    }
    // (element, level it was filed under, its z span before, how far it moved up)
    let mut moves: Vec<(Handle, String, (f64, f64), f64)> = Vec::new();
    for h in modified {
        let (Some(a), Some(b)) = (before.model.get(h), now.model.get(h)) else { continue };
        let (Some(la), Some(lb)) = (super::bspec::made_level(a), super::bspec::made_level(b)) else { continue };
        // A command that set the level itself (`bim.level.move`) has filed it.
        if !la.eq_ignore_ascii_case(&lb) {
            continue;
        }
        // A level a person chose is kept (Q6628).
        if chosen(b) {
            continue;
        }
        if let (Some(za), Some(zb)) = (z_range(a), z_range(b)) {
            let dz = zb.0 - za.0;
            if dz.is_finite() && dz != 0.0 {
                moves.push((h, la, za, dz));
            }
        }
    }
    // Copies: a new built element whose WALLGEN tags are those of one that was there before and
    // still is.
    let copies: Vec<&Arc<Entity>> = added.iter().filter_map(|h| now.model.get(*h)).filter(|e| super::wallgen::made_as(e).is_some()).collect();
    // A copy of an element whose level a person chose: that choice was the original's, the copy
    // is filed by where it stands (Q6628).
    let unchoose: Vec<Handle> = copies.iter().filter(|e| chosen(e)).map(|e| e.handle).collect();
    if !copies.is_empty() {
        let sources: HashSet<&str> = copies.iter().filter_map(|e| identity(e)).filter_map(|t| t.get(2).copied()).collect();
        let mut originals: HashMap<Vec<&str>, Vec<&Arc<Entity>>> = HashMap::new();
        for e in before.model.iter().take(MAX_SCAN) {
            if let Some(t) = identity(e).filter(|t| t.get(2).is_some_and(|s| sources.contains(s)))
                && now.model.get(e.handle).is_some()
            {
                originals.entry(t).or_default().push(e);
            }
        }
        for c in copies {
            let (Some(t), Some(zc)) = (identity(c), z_range(c)) else { continue };
            let Some(level) = super::bspec::made_level(c) else { continue };
            // The original of its size (one of the same tags); the nearest in height.
            let Some(zo) = originals
                .get(&t)
                .into_iter()
                .flatten()
                .filter_map(|o| z_range(o))
                .filter(|z| ((z.1 - z.0) - (zc.1 - zc.0)).abs() <= 1e-6 * (1.0 + zc.1.abs()))
                .min_by(|a, b| (a.0 - zc.0).abs().total_cmp(&(b.0 - zc.0).abs()))
            else {
                continue;
            };
            let dz = zc.0 - zo.0;
            if dz.is_finite() && dz != 0.0 {
                moves.push((c.handle, level, zo, dz));
            }
        }
    }
    if moves.is_empty() && unchoose.is_empty() {
        return;
    }
    let storeys = Storeys::of(&now);
    let refile: Vec<(Handle, String, String)> = storeys
        .iter()
        .flat_map(|st| moves.iter().filter_map(move |(h, from, z, dz)| moved_to(st, from, *z, *dz).map(|to| (*h, from.clone(), to.to_string()))))
        .collect();
    if refile.is_empty() && unchoose.is_empty() {
        return;
    }
    let Ok(st) = s.state_mut() else { return };
    let d = Arc::make_mut(&mut st.doc);
    for h in unchoose {
        let _ = d.modify_entity(h, |e| e.common.xdata = with_choice(&e.common.xdata, None).into());
    }
    if let Some(storeys) = storeys {
        for (h, from, to) in refile {
            file_under(d, &storeys, h, &from, &to);
        }
    }
}

// ─── bim.level.move ──────────────────────────────────────────────────────────

fn run_move(s: &mut Session, p: &Value) -> Result<Value> {
    let want = p
        .get("level")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| bad(MOVE, "`level` (a level's name) is required"))?;
    let hs = targets(s, p).map_err(|e| bad(MOVE, e.to_string()))?;
    if hs.is_empty() {
        return Err(bad(MOVE, "select the elements to move, or give `handle` / `handles`"));
    }
    if hs.len() > MOVE_MAX {
        return Err(bad(MOVE, format!("{} elements: at most {MOVE_MAX} at once", hs.len())));
    }
    let (sp, _) = super::bspec::current(s, MOVE)?;
    let d = s.doc()?;
    let st = Storeys::of_spec(d, &sp).ok_or_else(|| bad(MOVE, "the building has fewer than two levels"))?;
    let (to, to_elev) = st.named(want).cloned().ok_or_else(|| {
        let names: Vec<&str> = st.levels.iter().take(50).map(|l| l.0.as_str()).collect();
        bad(MOVE, format!("there is no level named {want} — levels: {names:?}"))
    })?;
    let keep_height = p.get("keep_height").and_then(Value::as_bool).unwrap_or(false);
    // (element, level it is filed under, how far it is raised)
    let mut plan: Vec<(Handle, String, f64)> = Vec::new();
    let mut kept = 0usize;
    for h in hs {
        let e = d.entity(h).ok_or_else(|| bad(MOVE, format!("{} is not in the drawing", h.hex())))?;
        let from = super::bspec::made_level(e)
            .filter(|l| !l.trim().is_empty())
            .ok_or_else(|| bad(MOVE, format!("{} is not an element of the built model", h.hex())))?;
        if from.eq_ignore_ascii_case(&to) {
            kept += 1;
            if !chosen(e) {
                plan.push((h, from, 0.0));
            }
            continue;
        }
        let (_, from_elev) =
            st.named(&from).ok_or_else(|| bad(MOVE, format!("{} is on level {from}, which the building spec doesn't have", h.hex())))?;
        // Already standing in the level's band (filed under another one): only filed there.
        let stands_there =
            z_range(e).and_then(|z| standing(&st, &from, z)).and_then(|z| st.holding(z, &to)).is_some_and(|l| l.0.eq_ignore_ascii_case(&to));
        let dz = if keep_height || stands_there { 0.0 } else { (to_elev - from_elev) * st.upf };
        if !dz.is_finite() {
            return Err(bad(MOVE, "the levels' elevations are out of range"));
        }
        plan.push((h, from, dz));
    }
    let doc = s.doc_mut()?;
    let mut moved = Vec::new();
    let mut filed = 0usize;
    for (h, from, dz) in plan {
        let same = from.eq_ignore_ascii_case(&to);
        let mut shifted = dz == 0.0;
        let m = cadaddy_doc::translation(Vec3 { x: 0.0, y: 0.0, z: dz });
        let ok = doc
            .modify_entity(h, |e| {
                if dz != 0.0 {
                    shifted = e.kind.transform3(&m);
                }
                if shifted {
                    e.common.xdata = with_choice(&e.common.xdata, Some(&to)).into();
                }
            })
            .is_ok();
        if !(ok && shifted) || same {
            continue;
        }
        if file_under(doc, &st, h, &from, &to) {
            if dz == 0.0 {
                filed += 1;
            }
            let raised = dz / st.upf;
            moved.push(json!({"handle": h.hex(), "from": from, "raised_ft": (raised * 1000.0).round() / 1000.0}));
        }
    }
    Ok(json!({"level": to, "moved": moved.len(), "filed": filed, "kept": kept, "elements": moved}))
}

// ─── A stored level that disagrees ───────────────────────────────────────────

/// A stored `level` property of a built element that names another level than the one it is
/// filed under: `{stored, level, why, fix}`.
pub(crate) fn stored_conflict(e: &Entity) -> Option<Value> {
    let level = super::bspec::made_level(e).filter(|l| !l.trim().is_empty())?;
    let store = cadaddy_doc::props::read(&e.common.xdata);
    let stored = store.get("level").filter(|p| p.has_value())?.text();
    let stored = stored.trim();
    if stored.is_empty() || stored.eq_ignore_ascii_case(level.trim()) {
        return None;
    }
    Some(json!({
        "stored": stored,
        "level": level,
        "why": "its stored level property names another level than the one its place in the building files it under",
        "fix": format!("bim.level.move {{handle: \"{}\", level: \"{stored}\"}} moves it there; or set the property to {level}", e.handle.hex()),
    }))
}
