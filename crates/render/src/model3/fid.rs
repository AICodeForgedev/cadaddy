//! Fidelity tiers for big models (Q6163): a building model holds every stud, stick and coupling,
//! and a view of the whole building can neither draw nor read them. Like a map's zoom levels,
//! each element has a tier and a view draws the tiers its scale calls for:
//!
//! - [`Tier::Building`]: the shell, slabs and roof, big equipment, mains and risers.
//! - [`Tier::Floor`]: walls, doors and windows, branch ducts and pipes, conduit runs as runs,
//!   panels, terminals.
//! - [`Tier::Room`]: devices, fixtures, luminaires, boxes, sticks and cable, framing and sheets.
//! - [`Tier::Detail`]: couplings, straps, rings, fasteners, seams.
//!
//! What an element's tier is comes from the caller's classifier (rules data, see the engine's
//! `view.fidelity`); this module keeps the index: every classified element with its tier, kind,
//! level, run and bounds, a grid over the plan per tier (the spatial index the view culls by),
//! and the elements held back from the model until a view needs them ([`Stream`]).
//!
//! Counts never come from here: the model is what is drawn; quantities read the drawing.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, EntityStore, Handle, Space};
use cadaddy_geom::Vec3;

use super::{Aabb, in_view};
use crate::shade::{Scene, ShadeView, scene_clipped};

/// How much of the model a view shows, coarse to fine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    #[default]
    Building,
    Floor,
    Room,
    Detail,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Building, Tier::Floor, Tier::Room, Tier::Detail];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The tier at `i` (past the last: [`Tier::Detail`]).
    pub fn from_index(i: usize) -> Tier {
        Tier::ALL.get(i).copied().unwrap_or(Tier::Detail)
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::Building => "building",
            Tier::Floor => "floor",
            Tier::Room => "room",
            Tier::Detail => "detail",
        }
    }

    /// `building`, `floor`, `room` or `detail` (any case; `b`, `f`, `r`, `d`).
    pub fn parse(s: &str) -> Option<Tier> {
        match s.trim().to_ascii_lowercase().as_str() {
            "building" | "b" => Some(Tier::Building),
            "floor" | "f" | "level" => Some(Tier::Floor),
            "room" | "r" => Some(Tier::Room),
            "detail" | "d" => Some(Tier::Detail),
            _ => None,
        }
    }

    /// The tier below this one (coarser), if any.
    pub fn coarser(self) -> Option<Tier> {
        self.index().checked_sub(1).map(Tier::from_index)
    }
}

/// The tiers a part of the model draws at, `from` to `to` inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Show {
    pub from: Tier,
    pub to: Tier,
}

impl Show {
    pub const ALWAYS: Show = Show { from: Tier::Building, to: Tier::Detail };

    /// An element of tier `t`: from its tier on.
    pub fn from(t: Tier) -> Show {
        Show { from: t, to: Tier::Detail }
    }

    pub fn at(self, t: Tier) -> bool {
        self.from <= t && t <= self.to
    }
}

impl Default for Show {
    fn default() -> Self {
        Show::ALWAYS
    }
}

/// What the classifier says about one element of the model.
#[derive(Clone, Debug, PartialEq)]
pub struct ElemClass {
    pub tier: Tier,
    /// Its kind (`receptacle`, `conduit`, `wall`): what a search forces into view by.
    pub kind: String,
    /// Its level's name (empty: none).
    pub level: String,
    /// The run it is a piece of: below its tier the run draws as one line.
    pub group: Option<String>,
    /// The tier from which, below its own, it draws as its run's line (`None`: nothing).
    pub line_from: Option<Tier>,
}

/// How a classified element is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// Its triangles are in the model's base chunks.
    Base,
    /// A placement of a prototype (repeated pieces drawn once, placed many times).
    Instance,
    /// Not in the model until a view at its tier needs its cell ([`Stream`]).
    Streamed,
    /// Outside the section box: not drawn.
    Outside,
}

/// No run.
pub const NO_GROUP: u32 = u32::MAX;

/// One classified element.
#[derive(Clone, Copy, Debug)]
pub struct Elem {
    pub handle: Handle,
    pub tier: Tier,
    /// Index in [`Fid::kinds`], [`Fid::levels`], [`Fid::groups`] ([`NO_GROUP`]: none).
    pub kind: u32,
    pub level: u32,
    pub group: u32,
    pub bounds: Aabb,
    pub held: Held,
    /// Its triangles (a mesh's faces as triangles).
    pub tris: u32,
}

/// A cell of a [`Grid`]: the elements whose centre is in it, the bounds they reach, and how many
/// of them are streamed.
#[derive(Clone, Debug, Default)]
pub struct GridCell {
    pub elems: Vec<u32>,
    pub bounds: Aabb,
    pub streamed: u32,
}

/// A grid over the plan of one tier's elements: the spatial index views cull by. Each element is
/// in the cell holding its centre; a cell's bounds are its elements'.
#[derive(Clone, Debug, Default)]
pub struct Grid {
    lo: [f64; 2],
    size: [f64; 2],
    n: [usize; 2],
    pub cells: Vec<GridCell>,
}

/// About this many elements per grid cell.
const PER_CELL: usize = 48;
/// Most cells along a side of a grid.
const MAX_SIDE: usize = 128;

impl Grid {
    fn build(elems: &[Elem], members: &[u32], over: &Aabb) -> Grid {
        if members.is_empty() || over.is_empty() {
            return Grid::default();
        }
        let (w, h) = ((over.hi.x - over.lo.x).max(1e-9), (over.hi.y - over.lo.y).max(1e-9));
        let want = (members.len() / PER_CELL).clamp(1, MAX_SIDE * MAX_SIDE) as f64;
        let nx = ((want * w / h).sqrt().ceil() as usize).clamp(1, MAX_SIDE);
        let ny = ((want / nx as f64).ceil() as usize).clamp(1, MAX_SIDE);
        let mut g = Grid { lo: [over.lo.x, over.lo.y], size: [w / nx as f64, h / ny as f64], n: [nx, ny], cells: vec![GridCell::default(); nx * ny] };
        for &i in members {
            let Some(e) = elems.get(i as usize) else { continue };
            let c = g.cell_of(e.bounds.center());
            if let Some(cell) = g.cells.get_mut(c) {
                cell.elems.push(i);
                cell.bounds.grow(e.bounds.lo);
                cell.bounds.grow(e.bounds.hi);
                if e.held == Held::Streamed {
                    cell.streamed += 1;
                }
            }
        }
        g
    }

    /// The cell holding plan point `p` (clamped to the grid).
    pub fn cell_of(&self, p: Vec3) -> usize {
        let [nx, ny] = self.n;
        if nx == 0 || ny == 0 {
            return 0;
        }
        let ix = ((p.x - self.lo[0]) / self.size[0]).floor();
        let iy = ((p.y - self.lo[1]) / self.size[1]).floor();
        let ix = if ix.is_finite() { (ix.max(0.0) as usize).min(nx - 1) } else { 0 };
        let iy = if iy.is_finite() { (iy.max(0.0) as usize).min(ny - 1) } else { 0 };
        iy * nx + ix
    }

    /// The cells with elements that `v` sees.
    pub fn in_view<'a>(&'a self, v: &'a ShadeView) -> impl Iterator<Item = usize> + 'a {
        self.cells.iter().enumerate().filter(move |(_, c)| !c.elems.is_empty() && in_view(&c.bounds, v)).map(|(i, _)| i)
    }
}

/// A model's elements by fidelity tier.
#[derive(Clone, Debug, Default)]
pub struct Fid {
    pub elems: Vec<Elem>,
    pub kinds: Vec<String>,
    pub levels: Vec<String>,
    pub groups: Vec<String>,
    /// Per tier, its elements' grid.
    pub grids: [Grid; 4],
    by_handle: HashMap<Handle, u32>,
    /// The streamed elements (their objects only), how they are cut into triangles and clipped.
    pub(crate) source: Option<Arc<Drawing>>,
    pub(crate) tol: f64,
    pub(crate) bg: Rgb,
    pub(crate) planes: Vec<cadaddy_doc::SectionPlane>,
}

/// Elements a search wants seen at any zoom: of this kind (any case), on this level if given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Force {
    pub kind: String,
    pub level: Option<String>,
}

/// Most markers one view shows.
pub const MAX_MARKERS: usize = 20_000;

/// Interns names into a list.
#[derive(Default)]
pub(crate) struct Names {
    pub list: Vec<String>,
    at: HashMap<String, u32>,
}

impl Names {
    pub fn id(&mut self, s: &str) -> u32 {
        if let Some(i) = self.at.get(s) {
            return *i;
        }
        let i = self.list.len() as u32;
        self.list.push(s.to_string());
        self.at.insert(s.to_string(), i);
        i
    }
}

impl Fid {
    pub(crate) fn new(elems: Vec<Elem>, kinds: Vec<String>, levels: Vec<String>, groups: Vec<String>, bounds: &Aabb) -> Fid {
        let mut members: [Vec<u32>; 4] = Default::default();
        for (i, e) in elems.iter().enumerate() {
            if e.held != Held::Outside
                && let Some(m) = members.get_mut(e.tier.index())
            {
                m.push(i as u32);
            }
        }
        let grids = [0, 1, 2, 3].map(|t| Grid::build(&elems, members.get(t).map_or(&[][..], |m| &m[..]), bounds));
        let by_handle = elems.iter().enumerate().map(|(i, e)| (e.handle, i as u32)).collect();
        Fid { elems, kinds, levels, groups, grids, by_handle, ..Fid::default() }
    }

    /// The element of handle `h`.
    pub fn elem(&self, h: Handle) -> Option<&Elem> {
        self.by_handle.get(&h).and_then(|i| self.elems.get(*i as usize))
    }

    pub fn kind_of(&self, e: &Elem) -> &str {
        self.kinds.get(e.kind as usize).map_or("", String::as_str)
    }

    pub fn level_of(&self, e: &Elem) -> &str {
        self.levels.get(e.level as usize).map_or("", String::as_str)
    }

    pub fn group_of(&self, e: &Elem) -> Option<&str> {
        self.groups.get(e.group as usize).map(String::as_str)
    }

    /// The elements of run `group` (indices), in model order.
    pub fn members(&self, group: u32) -> Vec<u32> {
        if group == NO_GROUP {
            return Vec::new();
        }
        self.elems.iter().enumerate().filter(|(_, e)| e.group == group).map(|(i, _)| i as u32).collect()
    }

    /// What a click on `h` at `tier` picks: its whole run when the tier draws the run as a line
    /// (the piece itself isn't drawn), else `h` alone.
    pub fn pick(&self, h: Handle, tier: Tier) -> Vec<Handle> {
        match self.elem(h) {
            Some(e) if e.tier > tier && e.group != NO_GROUP => {
                self.members(e.group).iter().filter_map(|i| self.elems.get(*i as usize)).map(|m| m.handle).collect()
            }
            _ => vec![h],
        }
    }

    /// Elements per tier, and of them how many each tier holds back to stream.
    pub fn per_tier(&self) -> [TierCount; 4] {
        let mut out = [TierCount::default(); 4];
        for e in &self.elems {
            let Some(c) = out.get_mut(e.tier.index()) else { continue };
            c.elements += 1;
            c.tris += e.tris as usize;
            match e.held {
                Held::Base => c.base += 1,
                Held::Instance => c.instanced += 1,
                Held::Streamed => {
                    c.streamed += 1;
                    c.streamed_tris += e.tris as usize;
                }
                Held::Outside => c.outside += 1,
            }
        }
        out
    }

    /// The elements `force` names that `v` sees and `tier` doesn't draw (nearest first is not
    /// needed: markers are flat).
    pub fn forced(&self, v: &ShadeView, tier: Tier, force: &Force) -> Vec<u32> {
        let kind = force.kind.trim();
        if kind.is_empty() {
            return Vec::new();
        }
        let kinds: Vec<u32> = self.kinds.iter().enumerate().filter(|(_, k)| k.eq_ignore_ascii_case(kind)).map(|(i, _)| i as u32).collect();
        let level: Option<Vec<u32>> = force
            .level
            .as_deref()
            .map(|l| self.levels.iter().enumerate().filter(|(_, n)| n.eq_ignore_ascii_case(l.trim())).map(|(i, _)| i as u32).collect());
        let mut out = Vec::new();
        for (i, e) in self.elems.iter().enumerate() {
            if out.len() >= MAX_MARKERS {
                break;
            }
            if e.tier > tier
                && e.held != Held::Outside
                && kinds.contains(&e.kind)
                && level.as_ref().is_none_or(|l| l.contains(&e.level))
                && in_view(&e.bounds, v)
            {
                out.push(i as u32);
            }
        }
        out
    }

    /// The triangles of the streamed elements of tier `t`'s cell `cell`.
    pub fn cell_scene(&self, t: Tier, cell: usize) -> Scene {
        let (Some(src), Some(c)) = (self.source.as_ref(), self.grids.get(t.index()).and_then(|g| g.cells.get(cell))) else {
            return Scene::default();
        };
        let mut d = (**src).clone();
        let mut store = EntityStore::new();
        for i in &c.elems {
            let Some(e) = self.elems.get(*i as usize).filter(|e| e.held == Held::Streamed) else { continue };
            if let Some(ent) = src.model.get(e.handle) {
                store.push_shared(Arc::clone(ent));
            }
        }
        d.model = store;
        let mut s = scene_clipped(&d, &Space::Model, self.tol, self.bg, &self.planes);
        s.lighting = None;
        s
    }

    /// Bytes the index holds.
    pub fn bytes(&self) -> usize {
        self.elems.len() * (std::mem::size_of::<Elem>() + std::mem::size_of::<(Handle, u32)>() + 8)
            + self
                .grids
                .iter()
                .map(|g| g.cells.len() * std::mem::size_of::<GridCell>() + g.cells.iter().map(|c| c.elems.len() * 4).sum::<usize>())
                .sum::<usize>()
            + self.kinds.iter().chain(&self.levels).chain(&self.groups).map(|s| s.len() + 24).sum::<usize>()
    }
}

/// Elements of one tier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TierCount {
    pub elements: usize,
    /// Their triangles, each once.
    pub tris: usize,
    pub base: usize,
    pub instanced: usize,
    pub streamed: usize,
    pub streamed_tris: usize,
    pub outside: usize,
}

/// When Auto fidelity changes tier: a view this many feet tall or more shows the building, the
/// floor, the room tier; below `room_ft`, detail. `margin`: how far past a threshold (a fraction
/// of it) the view must go before the tier changes, so a zoom that wavers about one doesn't
/// flicker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoTiers {
    pub building_ft: f64,
    pub floor_ft: f64,
    pub room_ft: f64,
    pub margin: f64,
}

impl Default for AutoTiers {
    fn default() -> Self {
        AutoTiers { building_ft: 240.0, floor_ft: 90.0, room_ft: 12.0, margin: 0.15 }
    }
}

impl AutoTiers {
    /// The thresholds, coarsest first (the view height at and above which each of building,
    /// floor, room holds).
    fn bounds(&self) -> [f64; 3] {
        [self.building_ft, self.floor_ft, self.room_ft]
    }

    /// The tier for a view `height_ft` tall with no history.
    pub fn raw(&self, height_ft: f64) -> Tier {
        let b = self.bounds();
        match b.iter().position(|t| height_ft >= *t) {
            Some(i) => Tier::from_index(i),
            None => Tier::Detail,
        }
    }

    /// The tier for a view `height_ft` tall that showed `prev` last: it changes only once the
    /// height is `margin` past the threshold between them.
    pub fn tier(&self, height_ft: f64, prev: Option<Tier>) -> Tier {
        if !height_ft.is_finite() {
            return prev.unwrap_or(Tier::Building);
        }
        let raw = self.raw(height_ft);
        let Some(p) = prev else { return raw };
        let m = self.margin.clamp(0.0, 0.9);
        let b = self.bounds();
        if raw > p {
            // Finer: below the threshold under `p` by the margin.
            let edge = b.get(p.index()).copied().unwrap_or(0.0);
            if height_ft < edge * (1.0 - m) { raw } else { p }
        } else if raw < p {
            // Coarser: above the threshold of `p` (the one above it) by the margin.
            let edge = p.index().checked_sub(1).and_then(|i| b.get(i)).copied().unwrap_or(f64::INFINITY);
            if height_ft > edge * (1.0 + m) { raw } else { p }
        } else {
            p
        }
    }
}

/// The streamed part of a model ([`Held::Streamed`]): the grid cells of held-back elements a
/// view needs at its tier, cut into triangles when first seen, dropped when the view zooms out
/// past their tier, and, past the byte budget, the ones longest out of view.
#[derive(Clone, Debug)]
pub struct Stream {
    pub cells: BTreeMap<(u8, u32), StreamCell>,
    /// Bytes the cells hold, and the most kept.
    pub bytes: usize,
    pub budget: usize,
    /// Changes with every cell added or dropped (the GPU's copy follows it).
    pub generation: u64,
    /// The model the cells came from (its address).
    model: usize,
    clock: u64,
}

/// One streamed cell.
#[derive(Clone, Debug)]
pub struct StreamCell {
    pub scene: Scene,
    pub bounds: Aabb,
    pub tier: Tier,
    used: u64,
}

/// What [`Stream::update`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StreamStep {
    pub added: usize,
    pub dropped: usize,
    /// Cells the view still needs (the next update brings them in).
    pub pending: usize,
    pub ms: f64,
}

/// Default bytes a stream keeps.
pub const STREAM_BUDGET: usize = 384 << 20;

impl Default for Stream {
    fn default() -> Self {
        Stream { cells: BTreeMap::new(), bytes: 0, budget: STREAM_BUDGET, generation: 0, model: 0, clock: 0 }
    }
}

impl Stream {
    /// Bring in the cells `v` sees at `tier` of `m` (at most `max_ms` of work; the rest is
    /// `pending`), nearest the view's centre first; drop the cells above `tier`, then, past the
    /// budget, the ones longest out of view. Another model starts the stream again.
    pub fn update(&mut self, m: &super::Model3, v: &ShadeView, tier: Tier, max_ms: f64) -> StreamStep {
        let t0 = web_time::Instant::now();
        let mut step = StreamStep::default();
        let addr = m as *const super::Model3 as usize;
        if self.model != addr {
            if !self.cells.is_empty() {
                step.dropped += self.cells.len();
                self.cells.clear();
                self.bytes = 0;
                self.generation += 1;
            }
            self.model = addr;
        }
        let Some(f) = m.fid.as_ref() else { return step };
        self.clock += 1;
        let before = self.cells.len();
        self.cells.retain(|k, _| Tier::from_index(usize::from(k.0)) <= tier);
        step.dropped += before - self.cells.len();
        // The cells wanted, nearest the view's centre first.
        let centre = v.center;
        let mut want: Vec<(f64, (u8, u32))> = Vec::new();
        for t in Tier::ALL.into_iter().filter(|t| *t <= tier) {
            let Some(g) = f.grids.get(t.index()) else { continue };
            for ci in g.in_view(v) {
                let Some(c) = g.cells.get(ci).filter(|c| c.streamed > 0) else { continue };
                let key = (t.index() as u8, ci as u32);
                if let Some(have) = self.cells.get_mut(&key) {
                    have.used = self.clock;
                    continue;
                }
                let p = c.bounds.center();
                let d = (p.dot(v.right) - centre.x).hypot(p.dot(v.up) - centre.y);
                want.push((d, key));
            }
        }
        want.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, key) in &want {
            if step.added > 0 && t0.elapsed().as_secs_f64() * 1000.0 >= max_ms {
                step.pending += 1;
                continue;
            }
            let t = Tier::from_index(usize::from(key.0));
            let scene = f.cell_scene(t, key.1 as usize);
            let bounds = f.grids.get(t.index()).and_then(|g| g.cells.get(key.1 as usize)).map_or(Aabb::EMPTY, |c| c.bounds);
            self.bytes += super::scene_bytes(&scene);
            self.cells.insert(*key, StreamCell { scene, bounds, tier: t, used: self.clock });
            step.added += 1;
        }
        // Over the budget: the cells longest out of view go.
        if self.bytes > self.budget {
            let mut old: Vec<(u64, (u8, u32))> = self.cells.iter().filter(|(_, c)| c.used < self.clock).map(|(k, c)| (c.used, *k)).collect();
            old.sort();
            for (_, k) in old {
                if self.bytes <= self.budget {
                    break;
                }
                if self.cells.remove(&k).is_some() {
                    step.dropped += 1;
                }
                self.bytes = self.cells.values().map(|c| super::scene_bytes(&c.scene)).sum();
            }
        }
        if step.added > 0 || step.dropped > 0 {
            self.bytes = self.cells.values().map(|c| super::scene_bytes(&c.scene)).sum();
            self.generation += 1;
        }
        step.ms = t0.elapsed().as_secs_f64() * 1000.0;
        step
    }

    /// The cells `v` sees, nearest first by their bounds' depth.
    pub fn visible(&self, v: &ShadeView) -> Vec<(u8, u32)> {
        let mut out: Vec<(f64, (u8, u32))> =
            self.cells.iter().filter(|(_, c)| in_view(&c.bounds, v)).map(|(k, c)| (-c.bounds.center().dot(v.dir), *k)).collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().map(|x| x.1).collect()
    }

    /// Triangles the cells hold.
    pub fn tris(&self) -> usize {
        self.cells.values().map(|c| c.scene.tris.len()).sum()
    }
}
