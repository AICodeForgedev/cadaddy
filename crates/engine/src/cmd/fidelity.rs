//! Fidelity levels of the BIM view ([`cadaddy_walk3d::fidelity`]): which built elements the 3D
//! view holds at which zoom.
//!
//! - [`Tiered::of`] gives every element of the BIM view its level (rules data: the starter's and
//!   the project's), indexes each level's elements in an R-tree over the plan, and merges the
//!   runs whose rule says so into one set of centre lines per system, level and fidelity (what
//!   they draw as from a level further out).
//! - [`Tiered::coarse`] is what the view holds for the whole building at a level (the building's
//!   and floor's elements, the force-shown ones as markers, the next level's runs as lines);
//!   [`Tiered::fine`] the room and detail elements in a region (what is in view).
//! - [`pick_target`]: a click at a coarse level picks what is drawn there: the run, or the room
//!   an element is in.
//! - `bim.fidelity {rules?, zoom?, default_tier?, show?, reset?, bench?}`: the rules in force,
//!   the project's own, the force-shown selections, the counts per level; `bench` measures what
//!   each level costs to build and hold.
//!
//! Fidelity changes only what the view draws: takeoffs, counts, validation and `bim.element`
//! read the drawing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use cadaddy_doc::color::Color;
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, EntityStore, Handle, entity_bounds};
use cadaddy_geom::{Bounds2, Vec3};
use cadaddy_walk3d::fidelity::{Element, Far, Fidelity, FidelityRules, KEY, MAX_RULES, MAX_SHOW, ProjectFidelity, Rule, Show, ZoomPatch};
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{AABB, RTree, RTreeObject};
use serde_json::{Value, json};

use super::bimlook::{self, Class};
use super::*;
use crate::{Result, Session};

/// Most objects looked at.
const MAX_SCAN: usize = 2_000_000;
/// Most centre lines one merged run holds, and pieces of one mesh looked at.
const MAX_RUN_LINES: usize = 200_000;
const MAX_PIECES: usize = 10_000;
/// Most room and detail elements one region holds (a region over the whole building at room
/// level would otherwise hold everything).
pub const MAX_FINE: usize = 250_000;
/// How tall a force-shown element's marker stands above it, feet.
const MARKER_FT: f64 = 10.0;

type Item = GeomWithData<Rectangle<[f64; 2]>, u32>;

/// Runs merged into centre lines: one system's on one level at one fidelity.
#[derive(Clone, Debug)]
pub struct Aggregate {
    pub tier: Fidelity,
    pub trade: &'static str,
    pub system: String,
    pub level: String,
    pub layer: String,
    pub rgb: [u8; 3],
    pub lines: Vec<(Vec3, Vec3)>,
    pub members: Vec<Handle>,
    pub bounds: Bounds2,
}

/// The BIM view by fidelity level.
pub struct Tiered {
    /// What the view draws at full fidelity ([`bimlook::display`]).
    pub display: Arc<Drawing>,
    pub rules: FidelityRules,
    /// Drawing units a foot.
    pub upf: f64,
    tier: HashMap<Handle, Fidelity>,
    forced: HashSet<Handle>,
    /// The view's objects, and each level's R-tree over their plan bounds.
    entities: Vec<Arc<Entity>>,
    index: [RTree<Item>; 4],
    pub aggregates: Vec<Aggregate>,
    /// Elements at each level.
    pub counts: [usize; 4],
}

/// What a rule sees of a built element (its class, build tags and size).
fn element<'a>(c: &'a Class, e: &'a Entity, part: &'a str, extent_ft: Option<f64>) -> Element<'a> {
    let tag = |k: &str| bimlook::made_tag(e, k).unwrap_or("");
    let mep = part.eq_ignore_ascii_case("mep");
    Element {
        trade: c.key,
        group: c.group,
        category: &c.category,
        ty: &c.ty,
        part,
        kind: tag("kind"),
        piece: if mep { bimlook::made_tag(e, "part").unwrap_or("segment") } else { "" },
        system: tag("system"),
        name: bimlook::made_name(e).or_else(|| bimlook::made_tag(e, "tag")).unwrap_or(""),
        level: &c.level,
        size_in: bimlook::made_tag(e, "size").and_then(cadaddy_walk3d::fidelity::size_inches),
        extent_ft,
    }
}

/// An object's largest extent, drawing units (a mesh's in 3D).
fn extent(d: &Drawing, e: &Entity) -> Option<f64> {
    let v = match &e.kind {
        EntityKind::Mesh(m) => {
            let (lo, hi) = corners(&m.vertices)?;
            let s = hi - lo;
            s.x.max(s.y).max(s.z)
        }
        _ => {
            let b = entity_bounds(d, e, 0);
            if b.is_empty() {
                return None;
            }
            b.width().max(b.height())
        }
    };
    v.is_finite().then_some(v)
}

fn corners(ps: &[Vec3]) -> Option<(Vec3, Vec3)> {
    let first = *ps.first()?;
    let (mut lo, mut hi) = (first, first);
    for p in ps {
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    Some((lo, hi))
}

/// A built element's level, the rule that set it, whether a force-shown selection takes it,
/// and its class.
pub fn level_of(d: &Drawing, e: &Entity, rules: &FidelityRules, upf: f64) -> Option<(Fidelity, Option<usize>, bool, Class)> {
    let c = bimlook::classify(d, e)?;
    let part = super::wallgen::made_as(e).unwrap_or_default();
    let ext = extent(d, e).map(|x| x / upf.max(1e-12));
    let el = element(&c, e, &part, ext);
    let (t, rule) = rules.assign(&el);
    let shown = rules.shown(&el);
    Some((t, rule, shown, c))
}

/// The centre line of each piece of a mesh (its corners joined by faces), along the piece's
/// longest direction; a line is itself.
pub fn centre_lines(e: &Entity) -> Vec<(Vec3, Vec3)> {
    match &e.kind {
        EntityKind::Line(l) => vec![(l.a, l.b)],
        EntityKind::Mesh(m) => {
            // Pieces: corners joined by a face.
            let n = m.vertices.len();
            let mut parent: Vec<usize> = (0..n).collect();
            fn root(p: &mut [usize], mut i: usize) -> usize {
                let mut guard = 0;
                while let Some(&up) = p.get(i) {
                    if up == i || guard > 64 {
                        break;
                    }
                    let grand = p.get(up).copied().unwrap_or(up);
                    if let Some(slot) = p.get_mut(i) {
                        *slot = grand;
                    }
                    i = up;
                    guard += 1;
                }
                i
            }
            for f in &m.faces {
                let mut it = f.v.iter().map(|i| *i as usize).filter(|i| *i < n);
                let Some(first) = it.next() else { continue };
                for v in it {
                    let (a, b) = (root(&mut parent, first), root(&mut parent, v));
                    if a != b
                        && let Some(slot) = parent.get_mut(a)
                    {
                        *slot = b;
                    }
                }
            }
            let mut pieces: BTreeMap<usize, Vec<Vec3>> = BTreeMap::new();
            for (i, v) in m.vertices.iter().enumerate() {
                let r = root(&mut parent, i);
                if pieces.len() >= MAX_PIECES && !pieces.contains_key(&r) {
                    continue;
                }
                pieces.entry(r).or_default().push(*v);
            }
            pieces.values().filter_map(|ps| axis(ps)).collect()
        }
        _ => Vec::new(),
    }
}

/// The segment through points `ps` along their main direction (power iteration on their
/// spread), from the first to the last of them along it.
fn axis(ps: &[Vec3]) -> Option<(Vec3, Vec3)> {
    if ps.len() < 2 {
        return None;
    }
    let n = ps.len() as f64;
    let c = ps.iter().fold(Vec3::ZERO, |a, p| a + *p) * (1.0 / n);
    let mut cov = [[0.0f64; 3]; 3];
    for p in ps {
        let d = *p - c;
        let v = [d.x, d.y, d.z];
        for (i, row) in cov.iter_mut().enumerate() {
            for (j, x) in row.iter_mut().enumerate() {
                *x += v.get(i).copied().unwrap_or(0.0) * v.get(j).copied().unwrap_or(0.0);
            }
        }
    }
    // Start along the box's longest side.
    let (lo, hi) = corners(ps)?;
    let s = hi - lo;
    let mut a = if s.x >= s.y && s.x >= s.z {
        Vec3::new(1.0, 0.0, 0.0)
    } else if s.y >= s.z {
        Vec3::new(0.0, 1.0, 0.0)
    } else {
        Vec3::new(0.0, 0.0, 1.0)
    };
    for _ in 0..24 {
        let v = [a.x, a.y, a.z];
        let m = |r: &[f64; 3]| r[0] * v[0] + r[1] * v[1] + r[2] * v[2];
        let next = Vec3::new(m(&cov[0]), m(&cov[1]), m(&cov[2]));
        let len = next.len();
        if !len.is_finite() || len < 1e-300 {
            break;
        }
        a = next * (1.0 / len);
    }
    let (mut t0, mut t1) = (f64::MAX, f64::MIN);
    for p in ps {
        let t = (*p - c).dot(a);
        t0 = t0.min(t);
        t1 = t1.max(t);
    }
    (t1 - t0 > 1e-9 && t0.is_finite() && t1.is_finite()).then(|| (c + a * t0, c + a * t1))
}

fn rect(b: &Bounds2) -> Option<Rectangle<[f64; 2]>> {
    (!b.is_empty() && b.min.is_finite() && b.max.is_finite()).then(|| Rectangle::from_corners([b.min.x, b.min.y], [b.max.x, b.max.y]))
}

impl Tiered {
    /// The BIM view of `d` by level; `None` when the view is off or nothing is built.
    pub fn of(d: &Drawing) -> Option<Tiered> {
        let display = Arc::new(bimlook::display(d)?);
        let rules = FidelityRules::of_spec(d.building_spec.as_deref());
        let upf = super::elemprops::units_per_foot(d);
        let look = bimlook::Look::of(d);
        let mut tier: HashMap<Handle, Fidelity> = HashMap::new();
        let mut forced: HashSet<Handle> = HashSet::new();
        let mut counts = [0usize; 4];
        // Runs drawn as lines from further out: (level, trade, system, storey) → members.
        let mut runs: BTreeMap<(Fidelity, &'static str, String, String), Vec<Handle>> = BTreeMap::new();
        for e in d.model.iter().take(MAX_SCAN) {
            let Some((t, rule, shown, c)) = level_of(d, e, &rules, upf) else { continue };
            tier.insert(e.handle, t);
            if let Some(n) = counts.get_mut(t.index() as usize) {
                *n += 1;
            }
            if shown {
                forced.insert(e.handle);
            }
            if rules.far(rule) == Far::Line {
                let system = bimlook::made_tag(e, "system").unwrap_or(c.key).to_string();
                runs.entry((t, c.key, system, c.level.clone())).or_default().push(e.handle);
            }
        }
        // Each level's objects in its own tree (the sheet's own lines, when shown, with the
        // building).
        let entities: Vec<Arc<Entity>> = display.model.iter().take(MAX_SCAN).cloned().collect();
        let mut items: [Vec<Item>; 4] = Default::default();
        let mut at: HashMap<Handle, u32> = HashMap::new();
        for (i, e) in entities.iter().enumerate() {
            at.insert(e.handle, i as u32);
            let t = tier.get(&e.handle).copied().unwrap_or(Fidelity::Building);
            if let (Some(r), Some(list)) = (rect(&entity_bounds(&display, e, 0)), items.get_mut(t.index() as usize)) {
                list.push(GeomWithData::new(r, i as u32));
            }
        }
        let index = items.map(RTree::bulk_load);
        let mut aggregates = Vec::new();
        for ((t, trade, system, level), members) in runs {
            let mut lines = Vec::new();
            let mut bounds = Bounds2::EMPTY;
            let mut layer = String::new();
            for h in &members {
                let Some(e) = at.get(h).and_then(|i| entities.get(*i as usize)) else { continue };
                if layer.is_empty() {
                    layer = e.common.layer.to_string();
                }
                for (a, b) in centre_lines(e) {
                    if lines.len() >= MAX_RUN_LINES {
                        break;
                    }
                    bounds = bounds.union(&Bounds2::new(a.xy(), a.xy())).union(&Bounds2::new(b.xy(), b.xy()));
                    lines.push((a, b));
                }
            }
            if !lines.is_empty() {
                aggregates.push(Aggregate { tier: t, trade, system, level, layer, rgb: look.rgb(trade), lines, members, bounds });
            }
        }
        Some(Tiered { display, rules, upf, tier, forced, entities, index, aggregates, counts })
    }

    /// An object's level (the sheet's own lines: the building's).
    pub fn tier_of(&self, h: Handle) -> Fidelity {
        self.tier.get(&h).copied().unwrap_or(Fidelity::Building)
    }

    pub fn forced(&self) -> usize {
        self.forced.len()
    }

    fn empty(&self) -> Drawing {
        let mut out = (*self.display).clone();
        out.model = EntityStore::new();
        out
    }

    fn push_lines(&self, out: &mut Drawing, a: &Aggregate) {
        let common =
            Common { layer: a.layer.as_str().into(), color: Color::True(cadaddy_color::Rgb(a.rgb[0], a.rgb[1], a.rgb[2])), ..Common::default() };
        for (p, q) in &a.lines {
            let _ = out.add(&cadaddy_doc::Space::Model, common.clone(), EntityKind::Line(cadaddy_doc::Line { a: *p, b: *q }));
        }
    }

    /// A force-shown element in the marker colour, and its marker: a line standing up from its
    /// top.
    fn push_marked(&self, out: &mut Drawing, e: &Arc<Entity>) {
        let m = self.rules.marker_rgb;
        let ink = Color::True(cadaddy_color::Rgb(m[0], m[1], m[2]));
        let mut c = (**e).clone();
        c.common.color = ink;
        c.common.material = None.into();
        let top = match &e.kind {
            EntityKind::Mesh(mesh) => corners(&mesh.vertices).map(|(lo, hi)| Vec3::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0, hi.z)),
            _ => {
                let b = entity_bounds(&self.display, e, 0);
                (!b.is_empty()).then(|| {
                    let p = b.center();
                    Vec3::new(p.x, p.y, 0.0)
                })
            }
        };
        let layer = c.common.layer.clone();
        out.model.push(c);
        if let Some(t) = top.filter(|t| t.is_finite()) {
            let up = Vec3::new(t.x, t.y, t.z + MARKER_FT * self.upf);
            let _ = out.add(
                &cadaddy_doc::Space::Model,
                Common { layer, color: ink, ..Common::default() },
                EntityKind::Line(cadaddy_doc::Line { a: t, b: up }),
            );
        }
    }

    /// What the view holds for the whole building at `level`: the elements of the building and
    /// floor levels up to it, the force-shown ones as markers, and the merged runs of the level
    /// after the last of those as lines (room and detail hold the floor's: the same model, its
    /// lines hidden inside the runs drawn close in and seen beyond them).
    pub fn coarse(&self, level: Fidelity) -> Drawing {
        let top = level.min(Fidelity::Floor);
        let mut out = self.empty();
        for e in &self.entities {
            if self.forced.contains(&e.handle) {
                self.push_marked(&mut out, e);
            } else if self.tier_of(e.handle) <= top {
                out.model.push_shared(Arc::clone(e));
            }
        }
        for a in self.aggregates.iter().filter(|a| a.tier.coarser() == Some(top)) {
            self.push_lines(&mut out, a);
        }
        out
    }

    /// The room and detail elements up to `level` within `region` (plan, drawing units), at most
    /// `cap` of them, and the next level's merged runs there as lines; how many elements, and
    /// whether the cap cut it. `None` at the building and floor levels.
    pub fn fine(&self, level: Fidelity, region: &Bounds2, cap: usize) -> Option<(Drawing, usize, bool)> {
        if level < Fidelity::Room {
            return None;
        }
        let mut out = self.empty();
        let mut n = 0usize;
        let mut capped = false;
        let env = (!region.is_empty() && region.min.is_finite() && region.max.is_finite())
            .then(|| AABB::from_corners([region.min.x, region.min.y], [region.max.x, region.max.y]));
        let mut picked: Vec<u32> = Vec::new();
        for t in [Fidelity::Room, Fidelity::Detail].into_iter().filter(|t| *t <= level) {
            let Some(tree) = self.index.get(t.index() as usize) else { continue };
            match &env {
                Some(env) => picked.extend(tree.locate_in_envelope_intersecting(env).map(|it| it.data)),
                None => picked.extend(tree.iter().map(|it| it.data)),
            }
        }
        // Drawn in the drawing's order.
        picked.sort_unstable();
        for i in picked {
            let Some(e) = self.entities.get(i as usize) else { continue };
            if self.forced.contains(&e.handle) {
                continue;
            }
            if n >= cap {
                capped = true;
                break;
            }
            out.model.push_shared(Arc::clone(e));
            n += 1;
        }
        for a in self.aggregates.iter().filter(|a| a.tier.coarser() == Some(level) && (env.is_none() || a.bounds.intersects(region))) {
            self.push_lines(&mut out, a);
        }
        Some((out, n, capped))
    }

    /// How many elements of each level fall in `region`.
    pub fn count_in(&self, region: &Bounds2) -> [usize; 4] {
        let mut out = [0; 4];
        let Some(r) = rect(region) else { return out };
        let env = r.envelope();
        for (k, tree) in self.index.iter().enumerate() {
            if let Some(n) = out.get_mut(k) {
                *n = tree.locate_in_envelope_intersecting(&env).count();
            }
        }
        out
    }
}

/// What a click on `h` picks when the view shows `level`: `h` itself when it is drawn there;
/// else what stands for it — its run (the build's pieces of one run: segments and fittings), or
/// the room it is in; else `h`.
pub fn pick_target(d: &Drawing, h: Handle, level: Fidelity) -> Vec<Handle> {
    let Some(e) = d.model.get(h) else { return vec![h] };
    let rules = FidelityRules::of_spec(d.building_spec.as_deref());
    let upf = super::elemprops::units_per_foot(d);
    let Some((t, _, shown, c)) = level_of(d, e, &rules, upf) else { return vec![h] };
    if t <= level || shown {
        return vec![h];
    }
    // Its run: the pieces sharing its run's id.
    if let Some(run) = bimlook::made_tag(e, "mepid").and_then(|id| id.rsplit_once('/')).map(|(base, _)| base.to_string())
        && !run.is_empty()
    {
        let system = bimlook::made_tag(e, "system").unwrap_or("");
        let members: Vec<Handle> = d
            .model
            .iter()
            .take(MAX_SCAN)
            .filter(|x| {
                bimlook::made_tag(x, "mepid").and_then(|id| id.rsplit_once('/')).is_some_and(|(b, _)| b == run)
                    && bimlook::made_tag(x, "system").unwrap_or("") == system
                    && super::bspec::made_level(x).unwrap_or_default() == c.level
            })
            .map(|x| x.handle)
            .collect();
        if !members.is_empty() {
            return members;
        }
    }
    // The room it is in: the smallest on its level holding its middle.
    let b = entity_bounds(d, e, 0);
    if !b.is_empty() {
        let mid = b.center();
        let room = d
            .model
            .iter()
            .take(MAX_SCAN)
            .filter(|x| super::wallgen::made_as(x).is_some_and(|p| p.eq_ignore_ascii_case("room")))
            .filter(|x| super::bspec::made_level(x).unwrap_or_default() == c.level)
            .filter_map(|x| {
                let rb = entity_bounds(d, x, 0);
                rb.contains(mid).then(|| (rb.width() * rb.height(), x.handle))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, r)) = room {
            return vec![r];
        }
    }
    vec![h]
}

// ─── bim.fidelity ────────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("bim.fidelity", "Fidelity Levels", run).menu(&["View", "Fidelity Levels"]).params(
        "{rules?: [{tier: building|floor|room|detail, trade?, category?, part?, kind?, piece?, words?, level_words?: [text], min_size_in?, max_size_in?, min_extent_ft?, max_extent_ft?: number, far?: none|line, why?: text}] (the project's own, before the starter's), zoom?: {floor_px_per_ft?, room_px_per_ft?, detail_px_per_ft?, hysteresis?}, default_tier?: level, show?: {trade?, category?, words?, level?} | [them] | null (force-show at every zoom as markers; null clears), reset?: bool (the starter rules only), bench?: bool (build each level and report its cost)} → {source, zoom, default_tier, rules, project_rules, show, counts: {building, floor, room, detail}, by_trade, forced, merged_runs, bench?}: which built elements the 3D view draws at which zoom",
    )]
}

fn parse<T: serde::de::DeserializeOwned>(what: &str, v: &Value) -> Result<T> {
    serde_json::from_value(v.clone()).map_err(|e| bad("bim.fidelity", format!("`{what}`: {e}")))
}

fn save(s: &mut Session, p: &ProjectFidelity) -> Result<()> {
    let d = s.doc()?;
    let mut spec: Value = d.building_spec.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(json!({}));
    let val = if p.is_empty() { Value::Null } else { serde_json::to_value(p).unwrap_or(Value::Null) };
    if spec.get(KEY).cloned().unwrap_or(Value::Null) == val {
        return Ok(());
    }
    if let Some(o) = spec.as_object_mut() {
        if val.is_null() {
            o.remove(KEY);
        } else {
            o.insert(KEY.into(), val);
        }
    }
    s.doc_mut()?.building_spec = Some(spec.to_string());
    Ok(())
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.fidelity";
    let mut own = cadaddy_walk3d::fidelity::project_of(s.doc()?.building_spec.as_deref()).unwrap_or_default();
    if p.get("reset").and_then(Value::as_bool) == Some(true) {
        own = ProjectFidelity { show: own.show, ..ProjectFidelity::default() };
    }
    if let Some(v) = p.get("rules").filter(|v| !v.is_null()) {
        let rules: Vec<Rule> = parse("rules", v)?;
        if rules.len() > MAX_RULES {
            return Err(bad(CMD, format!("at most {MAX_RULES} rules")));
        }
        own.rules = rules;
    }
    if let Some(v) = p.get("zoom").filter(|v| !v.is_null()) {
        let z: ZoomPatch = parse("zoom", v)?;
        for x in [z.floor_px_per_ft, z.room_px_per_ft, z.detail_px_per_ft].into_iter().flatten() {
            if !(x.is_finite() && x > 0.0) {
                return Err(bad(CMD, "zoom thresholds are pixels a foot, above 0"));
            }
        }
        own.zoom = Some(z);
    }
    if let Some(v) = p.get("default_tier").filter(|v| !v.is_null()) {
        let t = v.as_str().and_then(Fidelity::parse).ok_or_else(|| bad(CMD, "`default_tier` is building, floor, room or detail"))?;
        own.default_tier = Some(t);
    }
    match p.get("show") {
        Some(Value::Null) => own.show.clear(),
        Some(v @ Value::Object(_)) => {
            let one: Show = parse("show", v)?;
            if one.is_empty() {
                return Err(bad(CMD, "`show` needs trade, category, words or level"));
            }
            own.show = vec![one];
        }
        Some(v @ Value::Array(_)) => {
            let list: Vec<Show> = parse("show", v)?;
            own.show = list.into_iter().filter(|x| !x.is_empty()).take(MAX_SHOW).collect();
        }
        Some(v) => return Err(bad(CMD, format!("`show` must be an object, a list or null, got {v}"))),
        None => {}
    }
    save(s, &own)?;
    let d = s.doc()?;
    let rules = FidelityRules::with_project(Some(&own));
    let started = web_time::Instant::now();
    let tiered = Tiered::of(d);
    let tier_ms = started.elapsed().as_secs_f64() * 1000.0;
    let upf = super::elemprops::units_per_foot(d);
    // Elements of each trade at each level.
    let mut by_trade: BTreeMap<&'static str, [usize; 4]> = BTreeMap::new();
    for e in d.model.iter().take(MAX_SCAN) {
        if let Some((t, _, _, c)) = level_of(d, e, &rules, upf)
            && let Some(n) = by_trade.entry(c.key).or_default().get_mut(t.index() as usize)
        {
            *n += 1;
        }
    }
    let levels = |a: &[usize; 4]| json!({"building": a[0], "floor": a[1], "room": a[2], "detail": a[3]});
    let counts = tiered.as_ref().map_or([0; 4], |t| t.counts);
    let mut out = json!({
        "source": rules.source,
        "zoom": rules.zoom,
        "default_tier": rules.default_tier,
        "project_rules": rules.project_rules,
        "rules": rules.rules,
        "show": rules.show,
        "counts": levels(&counts),
        "by_trade": by_trade.iter().map(|(k, a)| (k.to_string(), levels(a))).collect::<serde_json::Map<String, Value>>(),
        "forced": tiered.as_ref().map_or(0, Tiered::forced),
        "merged_runs": tiered.as_ref().map_or(0, |t| t.aggregates.len()),
        "built": tiered.is_some(),
    });
    if p.get("bench").and_then(Value::as_bool) == Some(true)
        && let Some(t) = &tiered
    {
        out["bench"] = bench(t, tier_ms);
    }
    Ok(out)
}

/// What one model costs: the time to build it, its memory, triangles and instances, and the
/// vertex bytes a GPU holds for it.
fn cost(d: &Drawing, elements: usize, select_ms: f64) -> Value {
    use std::sync::atomic::AtomicUsize;
    let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let started = web_time::Instant::now();
    let m = cadaddy_render::model3::build(d, &cadaddy_doc::Space::Model, 1.0, cadaddy_color::Rgb(0, 0, 0), &p, &t);
    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
    // A vertex is 68 bytes on the GPU and a placement 48 (the shaded view's buffers).
    let held: usize = m.base.tris.len() * 3
        + m.base.lines.len() * 2
        + m.protos.iter().flat_map(|p| p.lods.iter()).map(|s| s.tris.len() * 3 + s.lines.len() * 2).sum::<usize>();
    json!({
        "elements": elements,
        "objects": d.model.len(),
        "select_ms": (select_ms * 10.0).round() / 10.0,
        "build_ms": (build_ms * 10.0).round() / 10.0,
        "model_mb": (m.bytes() as f64 / 1e6 * 100.0).round() / 100.0,
        "gpu_vertex_mb": ((held * 68 + m.instances.len() * 48) as f64 / 1e6 * 100.0).round() / 100.0,
        "tris": m.total_tris(),
        "prototypes": m.protos.len(),
        "instances": m.instances.len(),
    })
}

/// Each level's cost: the whole building's model at each level, and the room and detail
/// elements of the room holding the most of them; and everything at once (no fidelity).
fn bench(t: &Tiered, tier_ms: f64) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("tiers_ms".into(), json!((tier_ms * 10.0).round() / 10.0));
    for level in [Fidelity::Building, Fidelity::Floor] {
        let started = web_time::Instant::now();
        let d = t.coarse(level);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let n = t.counts.iter().take(level.index() as usize + 1).sum::<usize>();
        out.insert(level.name().into(), cost(&d, n, ms));
    }
    // The room with the most room-level elements in it.
    let rooms: Vec<Bounds2> = t
        .display
        .model
        .iter()
        .take(MAX_SCAN)
        .filter(|e| super::wallgen::made_as(e).is_some_and(|p| p.eq_ignore_ascii_case("room")))
        .map(|e| entity_bounds(&t.display, e, 0))
        .filter(|b| !b.is_empty())
        .take(5_000)
        .collect();
    let best = rooms.iter().max_by_key(|b| t.count_in(b)[Fidelity::Room.index() as usize]).copied();
    if let Some(room) = best {
        let region = room.expand(room.width().max(room.height()) * 0.25);
        for level in [Fidelity::Room, Fidelity::Detail] {
            let started = web_time::Instant::now();
            if let Some((d, n, capped)) = t.fine(level, &region, MAX_FINE) {
                let ms = started.elapsed().as_secs_f64() * 1000.0;
                let mut c = cost(&d, n, ms);
                c["capped"] = json!(capped);
                c["region_ft"] = json!([((region.width() / t.upf) * 10.0).round() / 10.0, ((region.height() / t.upf) * 10.0).round() / 10.0]);
                out.insert(format!("{}_region", level.name()), c);
            }
        }
    }
    let all: usize = t.counts.iter().sum();
    out.insert("everything".into(), cost(&t.display, all, 0.0));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Mesh, MeshFace, MeshKind, RawTag, Space};
    use cadaddy_geom::Vec2;

    fn tags(part: &str, level: &str, more: &[&str]) -> Vec<RawTag> {
        let mut x = vec![
            RawTag { code: 1001, value: super::super::wallgen::APP.into() },
            RawTag { code: 1000, value: part.into() },
            RawTag { code: 1000, value: level.into() },
            RawTag { code: 1000, value: "0".into() },
        ];
        x.extend(more.iter().map(|m| RawTag { code: 1000, value: (*m).into() }));
        x
    }

    fn boxmesh(lo: Vec3, hi: Vec3) -> Mesh {
        let v = |x: bool, y: bool, z: bool| Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z });
        let vertices = vec![
            v(false, false, false),
            v(true, false, false),
            v(true, true, false),
            v(false, true, false),
            v(false, false, true),
            v(true, false, true),
            v(true, true, true),
            v(false, true, true),
        ];
        let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
        Mesh {
            vertices,
            faces: vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])],
            kind: MeshKind::Subd,
        }
    }

    fn add(d: &mut Drawing, layer: &str, x: Vec<RawTag>, m: Mesh) -> Handle {
        d.add(&Space::Model, Common { layer: layer.into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).unwrap()
    }

    /// A made-up two-room floor (inches): a slab, a roof, walls, two rooms, a main duct, a small
    /// pipe run (two pieces and a coupling), a conduit run, a panel, a rooftop fan, and an
    /// outlet in each room.
    pub(crate) fn building() -> (Drawing, BTreeMap<&'static str, Handle>) {
        let mut d = Drawing { building_spec: Some(json!({"units_per_foot": 12.0}).to_string()), ..Drawing::default() };
        let mut h = BTreeMap::new();
        let b = |x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64| boxmesh(Vec3::new(x0, y0, z0), Vec3::new(x1, y1, z1));
        h.insert("slab", add(&mut d, "S-SLAB", tags("floor", "Level 1", &[]), b(0.0, 0.0, -6.0, 480.0, 240.0, 0.0)));
        h.insert("roof", add(&mut d, "A-ROOF", tags("roof", "Roof", &[]), b(0.0, 0.0, 120.0, 480.0, 240.0, 126.0)));
        h.insert("wall", add(&mut d, "A-WALL", tags("wall", "Level 1", &[]), b(236.0, 0.0, 0.0, 244.0, 240.0, 108.0)));
        h.insert("room_a", add(&mut d, "A-ROOM", tags("room", "Level 1", &[]), b(0.0, 0.0, 0.0, 236.0, 240.0, 108.0)));
        h.insert("room_b", add(&mut d, "A-ROOM", tags("room", "Level 1", &[]), b(244.0, 0.0, 0.0, 480.0, 240.0, 108.0)));
        let duct = ["system:SA-1", "kind:duct", "part:segment", "duct_type:supply", "size:24x12", "mepid:d1/segment"];
        h.insert("duct", add(&mut d, "3D-MEP-SA", tags("mep", "Level 1", &duct), b(0.0, 100.0, 96.0, 480.0, 124.0, 108.0)));
        let pipe = |id: &'static str| ["system:CW", "kind:pipe", "part:segment", "size:1/2", id];
        h.insert(
            "pipe_1",
            add(&mut d, "3D-MEP-CW", tags("mep", "Level 1", &pipe("mepid:p7/segment")), {
                // Two straight pieces in one run mesh.
                let mut m = b(10.0, 10.0, 90.0, 110.0, 10.5, 90.5);
                let more = b(110.0, 10.0, 90.0, 110.5, 200.0, 90.5);
                let n = m.vertices.len() as u32;
                m.vertices.extend(more.vertices);
                m.faces.extend(more.faces.into_iter().map(|f| MeshFace { v: f.v.iter().map(|i| i + n).collect(), hidden: 0 }));
                m
            }),
        );
        let coupling = ["system:CW", "kind:pipe", "part:fitting:coupling", "size:1/2", "mepid:p7/coupling1"];
        h.insert("coupling", add(&mut d, "3D-MEP-CW", tags("mep", "Level 1", &coupling), b(60.0, 9.8, 89.8, 61.0, 10.7, 90.7)));
        let conduit = ["system:Power", "kind:conduit", "part:segment", "size:3/4", "mepid:c3/segment"];
        h.insert("conduit", add(&mut d, "3D-MEP-E", tags("mep", "Level 1", &conduit), b(300.0, 20.0, 100.0, 460.0, 20.75, 100.75)));
        let panel = ["system:Power", "kind:equipment", "part:equipment:panel", "tag:Panelboard"];
        h.insert("panel", add(&mut d, "E-EQUIP", tags("mep", "Level 1", &panel), b(300.0, 0.0, 40.0, 320.0, 4.0, 80.0)));
        h.insert("fan", add(&mut d, "M-EQUIP", tags("equipment", "Roof", &["type:Exhaust Fan"]), b(200.0, 100.0, 126.0, 230.0, 130.0, 150.0)));
        h.insert("outlet_a", add(&mut d, "E-DEVICE", tags("device", "Level 1", &["type:Duplex"]), b(100.0, 0.0, 18.0, 104.0, 2.0, 22.0)));
        h.insert("outlet_b", add(&mut d, "E-DEVICE", tags("device", "Level 1", &["type:Duplex"]), b(400.0, 0.0, 18.0, 404.0, 2.0, 22.0)));
        (d, h)
    }

    fn handles(d: &Drawing) -> HashSet<Handle> {
        d.model.iter().map(|e| e.handle).collect()
    }

    #[test]
    fn each_level_holds_its_elements_and_the_coarser_ones() {
        let (d, h) = building();
        let t = Tiered::of(&d).expect("a built model");
        let tier = |k: &str| t.tier_of(h[k]);
        assert_eq!(tier("slab"), Fidelity::Building);
        assert_eq!(tier("roof"), Fidelity::Building);
        assert_eq!(tier("duct"), Fidelity::Building, "a 24 in main");
        assert_eq!(tier("fan"), Fidelity::Building, "an exhaust fan on the roof");
        assert_eq!(tier("wall"), Fidelity::Floor);
        assert_eq!(tier("conduit"), Fidelity::Floor);
        assert_eq!(tier("panel"), Fidelity::Floor);
        assert_eq!(tier("pipe_1"), Fidelity::Room, "a half-inch pipe");
        assert_eq!(tier("outlet_a"), Fidelity::Room);
        assert_eq!(tier("coupling"), Fidelity::Detail);
        assert_eq!(t.counts, [4, 5, 3, 1]);
        // Far out: the building only; the small pipe isn't held (not even as a line).
        let far = handles(&t.coarse(Fidelity::Building));
        assert!(far.contains(&h["duct"]) && far.contains(&h["fan"]) && far.contains(&h["roof"]));
        assert!(!far.contains(&h["wall"]) && !far.contains(&h["outlet_a"]) && !far.contains(&h["pipe_1"]));
        // A floor: walls and runs too; the small pipe as its centre lines (one per piece), the
        // outlets not at all.
        let floor = t.coarse(Fidelity::Floor);
        let fh = handles(&floor);
        assert!(fh.contains(&h["wall"]) && fh.contains(&h["conduit"]) && fh.contains(&h["panel"]));
        assert!(!fh.contains(&h["outlet_a"]) && !fh.contains(&h["pipe_1"]));
        let lines: Vec<_> = floor.model.iter().filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some(l.clone()) } else { None }).collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().any(|l| (l.a.x - l.b.x).abs() > 90.0 && (l.a.y - 10.25).abs() < 0.5));
        // Close up in room A: its outlet and the pipe; not room B's outlet.
        let room_a = Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(236.0, 240.0));
        let (fine, n, capped) = t.fine(Fidelity::Room, &room_a, MAX_FINE).unwrap();
        let fine = handles(&fine);
        assert!(fine.contains(&h["outlet_a"]) && fine.contains(&h["pipe_1"]));
        assert!(!fine.contains(&h["outlet_b"]) && !fine.contains(&h["coupling"]));
        assert_eq!((n, capped), (2, false));
        // Detail adds the coupling; a cap of one cuts it.
        let (det, _, _) = t.fine(Fidelity::Detail, &room_a, MAX_FINE).unwrap();
        assert!(handles(&det).contains(&h["coupling"]));
        assert!(t.fine(Fidelity::Detail, &room_a, 1).unwrap().2);
        assert!(t.fine(Fidelity::Floor, &room_a, MAX_FINE).is_none());
        // Room and detail hold the floor's whole-building layer (its lines too).
        assert_eq!(t.coarse(Fidelity::Room).model.len(), floor.model.len());
        assert_eq!(t.coarse(Fidelity::Detail).model.len(), floor.model.len());
        // Every element is held at the finest level over the whole building, once.
        let whole = Bounds2::new(Vec2::new(-1e6, -1e6), Vec2::new(1e6, 1e6));
        let built = handles(&d);
        let mut all: HashSet<Handle> = handles(&t.coarse(Fidelity::Detail)).intersection(&built).copied().collect();
        let fine_all = handles(&t.fine(Fidelity::Detail, &whole, MAX_FINE).unwrap().0);
        assert!(all.is_disjoint(&fine_all));
        all.extend(fine_all);
        assert_eq!(all, handles(&d));
    }

    #[test]
    fn a_coarse_click_picks_the_run_or_the_room() {
        let (mut d, h) = building();
        let more = ["system:CW", "kind:pipe", "part:fitting:elbow", "size:1/2", "mepid:p7/elbow1"];
        let elbow = add(&mut d, "3D-MEP-CW", tags("mep", "Level 1", &more), boxmesh(Vec3::new(109.0, 9.0, 89.0), Vec3::new(111.0, 11.0, 91.0)));
        // At the floor level the small pipe stands for its whole run (segment and fittings).
        let mut run = pick_target(&d, h["pipe_1"], Fidelity::Floor);
        run.sort();
        let mut want = vec![h["pipe_1"], h["coupling"], elbow];
        want.sort();
        assert_eq!(run, want);
        // An outlet stands for its room; room B's for room B.
        assert_eq!(pick_target(&d, h["outlet_a"], Fidelity::Building), vec![h["room_a"]]);
        assert_eq!(pick_target(&d, h["outlet_b"], Fidelity::Floor), vec![h["room_b"]]);
        // Drawn at the level: itself.
        assert_eq!(pick_target(&d, h["outlet_a"], Fidelity::Room), vec![h["outlet_a"]]);
        assert_eq!(pick_target(&d, h["wall"], Fidelity::Floor), vec![h["wall"]]);
        assert_eq!(pick_target(&d, Handle(0xFFFF_FFFF), Fidelity::Building), vec![Handle(0xFFFF_FFFF)]);
    }

    fn session() -> (Session, BTreeMap<&'static str, Handle>) {
        let (d, h) = building();
        let mut s = Session::new();
        *s.doc_mut().unwrap() = d;
        (s, h)
    }

    #[test]
    fn the_command_keeps_the_projects_rules_and_force_shown_elements() {
        let (mut s, h) = session();
        let r = s.execute("bim.fidelity", &json!({})).unwrap();
        assert_eq!(r["counts"], json!({"building": 4, "floor": 5, "room": 3, "detail": 1}));
        assert_eq!(r["project_rules"], 0);
        assert_eq!(r["by_trade"]["devices"]["room"], 2);
        assert_eq!(r["merged_runs"], 1);
        assert!(r["rules"].as_array().is_some_and(|a| !a.is_empty()));
        // The project's own rule goes first: the outlets on the floor level.
        let r = s
            .execute(
                "bim.fidelity",
                &json!({"rules": [{"tier": "floor", "trade": ["devices"], "why": "seen per floor on this job"}], "zoom": {"room_px_per_ft": 40}}),
            )
            .unwrap();
        assert_eq!(r["counts"], json!({"building": 4, "floor": 7, "room": 1, "detail": 1}));
        assert_eq!(r["project_rules"], 1);
        assert_eq!(r["zoom"]["room_px_per_ft"], 40.0);
        assert!(r["source"].as_str().unwrap().contains("project"));
        // Kept in the drawing's building spec, with the spec's other settings.
        let spec: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
        assert_eq!(spec["units_per_foot"], 12.0);
        assert_eq!(spec[KEY]["rules"][0]["tier"], "floor");
        // Show the devices at every zoom: drawn far out, in the marker colour, with markers.
        let r = s.execute("bim.fidelity", &json!({"show": {"trade": "devices"}})).unwrap();
        assert_eq!(r["forced"], 2);
        let t = Tiered::of(s.doc().unwrap()).unwrap();
        let far = t.coarse(Fidelity::Building);
        let m = t.rules.marker_rgb;
        let ink = Color::True(cadaddy_color::Rgb(m[0], m[1], m[2]));
        let outlet = far.model.get(h["outlet_a"]).unwrap();
        assert_eq!(outlet.common.color, ink);
        let markers = far.model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_)) && e.common.color == ink).count();
        assert_eq!(markers, 2);
        // Never twice: not in a region's fine elements.
        let all = Bounds2::new(cadaddy_geom::Vec2::new(-1e6, -1e6), cadaddy_geom::Vec2::new(1e6, 1e6));
        assert!(!handles(&t.fine(Fidelity::Detail, &all, MAX_FINE).unwrap().0).contains(&h["outlet_a"]));
        // A reset drops the rules and keeps the selection; `null` clears it.
        let r = s.execute("bim.fidelity", &json!({"reset": true})).unwrap();
        assert_eq!((r["project_rules"].clone(), r["forced"].clone()), (json!(0), json!(2)));
        let r = s.execute("bim.fidelity", &json!({"show": null})).unwrap();
        assert_eq!(r["forced"], 0);
        let spec: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
        assert!(spec.get(KEY).is_none(), "nothing of the project's left: {spec}");
        // Bad values are refused, the drawing unchanged.
        for bad in [
            json!({"default_tier": "street"}),
            json!({"zoom": {"floor_px_per_ft": -1}}),
            json!({"show": 5}),
            json!({"show": {}}),
            json!({"rules": [{"tier": "attic"}]}),
        ] {
            assert!(s.execute("bim.fidelity", &bad).is_err(), "{bad}");
        }
        assert!(Value::from(s.doc().unwrap().building_spec.as_deref().unwrap()).as_str().is_some_and(|t| !t.contains(KEY)));
    }

    #[test]
    fn the_level_shown_changes_nothing_counted() {
        let (mut s, h) = session();
        let before = (s.execute("bim.trades", &json!({})).unwrap(), s.execute("bim.element", &json!({"handle": h["outlet_a"].hex()})).unwrap());
        for level in Fidelity::ALL {
            s.fidelity_shown = Some(level);
            let now = (s.execute("bim.trades", &json!({})).unwrap(), s.execute("bim.element", &json!({"handle": h["outlet_a"].hex()})).unwrap());
            assert_eq!(now, before, "at {level:?}");
        }
        // The counts per level add up to every built element, whatever the rules.
        let r = s.execute("bim.fidelity", &json!({"rules": [{"tier": "detail", "part": ["wall"]}]})).unwrap();
        let sum: u64 = ["building", "floor", "room", "detail"].iter().map(|k| r["counts"][k].as_u64().unwrap()).sum();
        assert_eq!(sum, 13);
        assert_eq!(s.execute("bim.trades", &json!({})).unwrap(), before.0);
    }

    #[test]
    fn a_click_far_out_selects_the_room_and_close_in_the_outlet() {
        let (mut s, h) = session();
        let mid = s.doc_mut().unwrap();
        let outlet =
            add(mid, "E-DEVICE", tags("device", "Level 1", &["type:Duplex"]), boxmesh(Vec3::new(50.0, 120.0, 18.0), Vec3::new(54.0, 122.0, 22.0)));
        let click = |s: &mut Session, level: Option<Fidelity>| {
            s.fidelity_shown = level;
            s.set_selection(Vec::new());
            s.idle_click(cadaddy_geom::Vec2::new(50.0, 121.0), false).unwrap();
            s.selection()
        };
        assert_eq!(click(&mut s, Some(Fidelity::Building)), vec![h["room_a"]]);
        assert_eq!(click(&mut s, Some(Fidelity::Floor)), vec![h["room_a"]]);
        assert_eq!(click(&mut s, Some(Fidelity::Room)), vec![outlet]);
        assert_eq!(click(&mut s, None), vec![outlet], "the 3D view isn't showing levels");
    }

    #[test]
    fn centre_lines_follow_each_piece() {
        let m = boxmesh(Vec3::new(0.0, 0.0, 0.0), Vec3::new(100.0, 2.0, 2.0));
        let e = Entity::new(Handle(1), EntityKind::Mesh(m));
        let l = centre_lines(&e);
        assert_eq!(l.len(), 1);
        let (a, b) = l[0];
        assert!((a.x.min(b.x)).abs() < 1e-6 && (a.x.max(b.x) - 100.0).abs() < 1e-6);
        assert!((a.y - 1.0).abs() < 1e-6 && (a.z - 1.0).abs() < 1e-6);
        // A diagonal piece.
        let turned = Mesh {
            vertices: boxmesh(Vec3::ZERO, Vec3::new(100.0, 1.0, 1.0))
                .vertices
                .iter()
                .map(|p| Vec3::new(p.x * 0.6 - p.y * 0.8, p.x * 0.8 + p.y * 0.6, p.z))
                .collect(),
            ..boxmesh(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0))
        };
        let (a, b) = centre_lines(&Entity::new(Handle(2), EntityKind::Mesh(turned)))[0];
        let d = b - a;
        assert!((d.len() - 100.0).abs() < 0.5, "{}", d.len());
        assert!((d.y / d.x - 0.8 / 0.6).abs() < 1e-3);
        // Nothing for a point, a degenerate mesh, a hostile face index.
        let bad = Mesh { vertices: vec![Vec3::ZERO], faces: vec![MeshFace { v: vec![0, 99, u32::MAX], hidden: 0 }], kind: MeshKind::Subd };
        assert!(centre_lines(&Entity::new(Handle(3), EntityKind::Mesh(bad))).is_empty());
    }
}
