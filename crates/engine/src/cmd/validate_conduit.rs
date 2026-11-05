//! E-CONDUIT-CONTINUITY, E-CONDUIT-FILL, E-BOX-FILL, E-DERATE, E-VDROP, E-METHOD, G-CLASH and
//! E-TAKEOFF (Q6153): Validate checks the routed branch circuits (Q6152) from the model's own
//! pieces — the conduit sticks with their two ends, the wires pulled in each raceway, the cables,
//! the boxes and the devices and gear they serve — recomputed here, never read back from the
//! router's report (`route/report.json`: its fill, derating, voltage-drop and box-fill rows are
//! not used). The router's `clash` items are carried into the run beside the recomputed ones.
//!
//! - **E-CONDUIT-CONTINUITY**: each device on a circuit is joined to its panel by that circuit's
//!   raceway: the sticks whose circuit includes it, end to end (ends within the rule's `touch_in`
//!   meet), through every box their ends touch, to the panel's box. A drop whose nearest end stops
//!   short of the device's box is a `fail` naming how far; a device no piece of its circuit
//!   reaches, or one whose pieces never reach its panel, is a `fail`. A cable run (NM, MC) is
//!   followed by its pieces' links (a cable's two ends are not stored).
//! - **E-CONDUIT-FILL**: each raceway run's conductors (its wires: count and size) against its
//!   trade size's area (NEC Chapter 9 Table 1: 53% one conductor, 31% two, 40% more; Tables 4
//!   and 5 as rules data).
//! - **E-BOX-FILL**: each box's conductors (each one entering from a raceway or cable whose end
//!   touches it, all grounds as one, a device yoke as two of the largest; NEC 314.16(B)) against
//!   its volume.
//! - **E-DERATE**: each raceway's current-carrying conductors (every one but the grounds) and the
//!   NEC 310.15(C)(1) factor for their number: every conductor's adjusted ampacity (the 90 °C
//!   column times the factor, never above the 75 °C column, 110.14(C), nor the 240.4(D) limit)
//!   carries the largest breaker of the circuits in it, read from the panel schedule.
//! - **E-VDROP**: each circuit's length along its raceway from its panel to its farthest device
//!   (the sticks' own lengths), its smallest conductor against the drawing's length table when it
//!   prints one (`0-60' = #12`), else the drop (2 K I L / cmil, I from the schedule's load, else
//!   the rule's share of the breaker) against the 3% of NEC 210.19(A) Informational Note 4.
//! - **E-METHOD**: each run's wiring method (its pieces' type) against the specification's
//!   wiring-methods table for the area its pieces are in (a guest or dwelling unit, an exposed
//!   room, else concealed), the project's wiring-method rule, or an approved value-engineering
//!   alternative.
//! - **G-CLASH**: no stick comes within the clearance (rules data, by what it passes) of a duct,
//!   pipe, terminal, structure, equipment, other conduit or a piece of gear's working space
//!   (NEC 110.26(A)); the router's `clash` items are carried in (source `build`).
//! - **E-TAKEOFF**: the stored count index (what `bim.count` and the BOM read) and the
//!   `conduit.takeoff` lines against a recount of the model's pieces (conduit by its sticks' ends,
//!   wire and cable by their lengths, every fitting and box one each), within the rule's
//!   `takeoff_pct` (1%).
//!
//! Anything the model or the documents don't say (a raceway material with no area table, a
//! conductor size outside the tables, a breaker the schedule doesn't give, no wiring-methods
//! table) is `unsure`, never a pass.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};

use cadaddy_doc::props::{self, Prop};
use cadaddy_doc::{Drawing, Entity, Handle};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

use super::validate::Frame;
use super::validate_packet::{Area, inside, point_in, pt, r2};
use super::validate_rules::Effective;

pub const CONTINUITY: &str = "E-CONDUIT-CONTINUITY";
pub const FILL: &str = "E-CONDUIT-FILL";
pub const BOX_FILL: &str = "E-BOX-FILL";
pub const DERATE: &str = "E-DERATE";
pub const VDROP: &str = "E-VDROP";
pub const METHOD: &str = "E-METHOD";
pub const CLASH: &str = "G-CLASH";
pub const TAKEOFF: &str = "E-TAKEOFF";
/// Every rule of this module.
pub const ALL: [&str; 8] = [CONTINUITY, FILL, BOX_FILL, DERATE, VDROP, METHOD, CLASH, TAKEOFF];
/// The verdicts these rules give.
pub const VERDICTS: [&str; 3] = ["pass", "fail", "unsure"];

/// Elements looked at, at most (every one).
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
/// Pieces of each kind kept, at most (hostile models).
const MAX_PIECES: usize = 1_000_000;
/// Obstacles kept, at most.
const MAX_OBSTACLES: usize = 400_000;
/// Findings of one rule kept, at most.
const MAX_FINDINGS: usize = 200_000;
/// Connections one box or cable lists, at most.
const MAX_LINKS: usize = 64;
/// A bucket of the plan grid, feet.
const CELL_FT: f64 = 8.0;
/// Default tolerances: ends within this meet, inches; a drop's end this far off its box fails.
const DEFAULT_TOUCH_IN: f64 = 1.0;
/// A drop's end farther than this from its box (inches) is no drop of it: none reaches it.
const FAR_GAP_IN: f64 = 240.0;
/// A device this close (feet) to equipment is served by the run that lands at it.
const SERVES_FT: f64 = 1.0;
/// A takeoff line off by more than this share of the recount fails (percent).
const DEFAULT_TAKEOFF_PCT: f64 = 1.0;
/// Device kinds whose box holds a yoke (NEC 314.16(B)(4)).
const YOKE_KINDS: [&str; 6] = ["receptacle", "switch", "dimmer", "occupancy sensor", "outlet", "gfci"];
/// Structure the router passes through by rule (bored holes, sleeves): not a clash here.
const PASSED: [&str; 7] = ["slab", "floor", "footing", "deck", "foundation", "roof", "framing"];

// ---------------------------------------------------------------- small helpers

fn text(ps: &props::Props, k: &str) -> String {
    ps.get(k).map(Prop::text).unwrap_or_default().trim().to_string()
}

fn num(ps: &props::Props, k: &str) -> Option<f64> {
    ps.get(k).map(Prop::text).and_then(|t| t.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0)
}

/// A trade size as printed (`3/4"`, `1"`, `1-1/4"`, `2 in`): inches.
pub(crate) fn trade_in(s: &str) -> Option<f64> {
    let t: String = s.trim().trim_end_matches(['"', ' ']).trim_end_matches("in").trim().chars().take(16).collect();
    let frac = |f: &str| -> Option<f64> {
        let (a, b) = f.split_once('/')?;
        let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
        (b > 0.0).then_some(a / b)
    };
    let v = if let Some((w, f)) = t.split_once('-') {
        w.trim().parse::<f64>().ok()? + frac(f)?
    } else if t.contains('/') {
        frac(&t)?
    } else {
        t.parse::<f64>().ok()?
    };
    (v.is_finite() && v > 0.0 && v < 12.0).then_some(v)
}

/// A conductor size as the tables key it (`#12` → `12`, `#3/0` → `3/0`, `250 kcmil` → `250`).
pub(crate) fn awg(s: &str) -> String {
    let t = s.trim().trim_start_matches('#').trim();
    t.chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect()
}

/// The rank of a conductor size among the voltage-drop sizes (larger conductor, larger rank).
fn size_rank(size: &str, sizes: &[String]) -> Option<usize> {
    sizes.iter().position(|s| s == size)
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn tol(r: &Effective, name: &str, default: f64) -> f64 {
    r.rule.tolerance.get(name).copied().filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(default)
}

fn rank(verdict: &str, severity: &str) -> u8 {
    match verdict {
        "pass" => 9,
        "unsure" => 5,
        _ => match severity {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        },
    }
}

/// The area of `areas` on `level` holding `at` (feet); `Some(None)`: on a level in scope but in
/// no area (whole levels only); `None`: not in scope.
fn area_for<'a>(areas: &'a [Area], whole_levels: bool, level: &str, at: [f64; 2]) -> Option<Option<&'a Area>> {
    let mine: Vec<&Area> = areas.iter().filter(|a| a.level.eq_ignore_ascii_case(level)).collect();
    if mine.is_empty() {
        return None;
    }
    let a = mine.iter().copied().find(|a| inside(a.bbox, at) && (a.outline.len() < 3 || point_in(&a.outline, at)));
    if a.is_some() || whole_levels { Some(a) } else { None }
}

/// The panel and circuit numbers of a circuit label (`LP1-1,3,5`).
fn circuit_of(label: &str) -> Option<(String, Vec<String>)> {
    let (panel, nums) = super::printcond::parse_circuit(label)?;
    let nums: Vec<String> = nums.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
    (!nums.is_empty()).then_some((panel, nums))
}

/// Is `tag` (a piece of gear's) the panel `panel`, or a typical room's instance of it (`G-214`)?
fn is_panel(tag: &str, panel: &str) -> bool {
    let (t, p) = (tag.trim().to_ascii_uppercase(), panel.trim().to_ascii_uppercase());
    !p.is_empty() && (t == p || t.starts_with(&format!("{p}-")))
}

/// The squared distance from `p` to the box `lo..hi`.
fn box_dist2(p: Vec3, lo: Vec3, hi: Vec3) -> f64 {
    let d = |v: f64, a: f64, b: f64| {
        if v < a {
            a - v
        } else if v > b {
            v - b
        } else {
            0.0
        }
    };
    let (x, y, z) = (d(p.x, lo.x, hi.x), d(p.y, lo.y, hi.y), d(p.z, lo.z, hi.z));
    x * x + y * y + z * z
}

/// Does the segment `a..b` pass through the box `lo..hi` (slab test)?
fn seg_hits_box(a: Vec3, b: Vec3, lo: Vec3, hi: Vec3) -> bool {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, d, l, h) in [(a.x, b.x - a.x, lo.x, hi.x), (a.y, b.y - a.y, lo.y, hi.y), (a.z, b.z - a.z, lo.z, hi.z)] {
        if d.abs() < 1e-12 {
            if p < l || p > h {
                return false;
            }
            continue;
        }
        let (u, v) = ((l - p) / d, (h - p) / d);
        let (u, v) = if u <= v { (u, v) } else { (v, u) };
        t0 = t0.max(u);
        t1 = t1.min(v);
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// The closest distance between segments `a..b` and `c..d`, and the point on the first.
fn seg_seg(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> (f64, Vec3) {
    let (u, v, w) = (b - a, d - c, a - c);
    let (aa, bb, cc, dd, ee) = (u.dot(u), u.dot(v), v.dot(v), u.dot(w), v.dot(w));
    let den = aa * cc - bb * bb;
    let (mut s, mut t);
    if aa < 1e-12 && cc < 1e-12 {
        return ((a - c).len(), a);
    }
    if aa < 1e-12 {
        s = 0.0;
        t = (ee / cc).clamp(0.0, 1.0);
    } else if cc < 1e-12 {
        t = 0.0;
        s = (-dd / aa).clamp(0.0, 1.0);
    } else {
        s = if den.abs() > 1e-12 { ((bb * ee - cc * dd) / den).clamp(0.0, 1.0) } else { 0.0 };
        t = (bb * s + ee) / cc;
        if t < 0.0 {
            t = 0.0;
            s = (-dd / aa).clamp(0.0, 1.0);
        } else if t > 1.0 {
            t = 1.0;
            s = ((bb - dd) / aa).clamp(0.0, 1.0);
        }
    }
    let p = a + u * s;
    let q = c + v * t;
    ((p - q).len(), p)
}

// ---------------------------------------------------------------- the model's pieces

/// A conduit stick: its run, circuit and panel, its material and size, its two ends (drawing
/// units, unstacked: where its level's plan has it).
#[derive(Clone, Debug)]
pub(crate) struct Stick {
    pub handle: Handle,
    pub run: String,
    pub circuit: String,
    pub panel: String,
    pub level: String,
    pub ty: String,
    pub size: String,
    pub size_in: Option<f64>,
    pub a: Vec3,
    pub b: Vec3,
    /// Its centreline: its two ends, or (ends apart both in plan and in height: a bend inside
    /// the stick) up or down at its lower end and across at its upper end's height.
    pub path: Vec<Vec3>,
    /// Its outside radius (drawing units).
    pub r: f64,
    pub room: String,
    /// Set in a wall's cavity (a vertical drop).
    pub vertical: bool,
}

/// A conductor pulled in a raceway.
#[derive(Clone, Debug)]
pub(crate) struct Wire {
    pub run: String,
    pub circuit: String,
    /// The tables' key (`12`, `3/0`).
    pub size: String,
    /// `ground`, `neutral` or `phase A`.
    pub role: String,
}

impl Wire {
    fn ground(&self) -> bool {
        self.role.eq_ignore_ascii_case("ground")
    }
}

/// A cable run (NM, MC, AC…): its conductors from its type (`NM 12/2 w/G`).
#[derive(Clone, Debug)]
pub(crate) struct Cable {
    pub handle: Handle,
    pub run: String,
    pub circuit: String,
    pub level: String,
    pub ty: String,
    pub room: String,
    /// What its ends land in (`connects`).
    pub links: Vec<Handle>,
    /// Its middle (drawing units, unstacked), when its body is drawn.
    pub at: Option<Vec3>,
}

impl Cable {
    /// Its insulated conductors (count, size) and whether it has a ground.
    fn conductors(&self) -> Option<(u32, String, bool)> {
        let t = self.ty.to_ascii_uppercase();
        let (_, rest) = t.split_once(' ')?;
        let spec: String = rest.split_whitespace().next()?.to_string();
        let (size, n) = spec.split_once('/')?;
        let n = n.trim().parse::<u32>().ok().filter(|n| (1..=12).contains(n))?;
        let size = awg(size);
        (!size.is_empty()).then(|| (n, size, t.contains("W/G") || t.contains("WITH GROUND")))
    }
}

/// A box: what it serves (a device's handle), its volume.
#[derive(Clone, Debug)]
pub(crate) struct BoxEl {
    pub handle: Handle,
    pub of: Option<Handle>,
    pub cubic_in: Option<f64>,
    pub size: String,
    pub level: String,
    pub lo: Vec3,
    pub hi: Vec3,
    pub links: Vec<Handle>,
}

/// A device on a circuit.
#[derive(Clone, Debug)]
pub(crate) struct Dev {
    pub handle: Handle,
    pub circuit: String,
    pub kind: String,
    pub level: String,
    pub lo: Vec3,
    pub hi: Vec3,
}

/// A piece of gear (an equipment element with its tag).
#[derive(Clone, Debug)]
pub(crate) struct GearEl {
    pub handle: Handle,
    pub tag: String,
    pub level: String,
    pub lo: Vec3,
    pub hi: Vec3,
    /// Electrical gear (a panel, switchboard, transformer…): its working space is kept clear.
    pub electrical: bool,
}

/// Something a stick keeps clear of.
#[derive(Clone, Debug)]
pub(crate) struct Obstacle {
    pub handle: Handle,
    pub level: String,
    pub category: String,
    pub system: String,
    pub trade: &'static str,
    pub lo: Vec3,
    pub hi: Vec3,
}

/// One piece as the takeoff counts it.
#[derive(Clone, Debug)]
struct Counted {
    kind: String,
    item: String,
    size: String,
    level: String,
    /// Feet (conduit by its ends, wire and cable by their length), else 1.
    qty: f64,
}

/// The model's raceway pieces, read once.
#[derive(Default)]
pub(crate) struct Net {
    pub upf: f64,
    pub sticks: Vec<Stick>,
    pub wires: Vec<Wire>,
    pub cables: Vec<Cable>,
    pub boxes: Vec<BoxEl>,
    pub devs: Vec<Dev>,
    pub gear: Vec<GearEl>,
    pub obstacles: Vec<Obstacle>,
    counted: Vec<Counted>,
    /// Every piece's links (`connects`), for cables and junctions.
    links: HashMap<Handle, Vec<Handle>>,
    /// What each piece is (`atom` kind, `device`, `gear`).
    what: HashMap<Handle, String>,
}

/// Coordinates past this (drawing units) are no building's: the piece is skipped.
const FAR: f64 = 1.0e9;

fn sane(p: Vec3) -> bool {
    p.is_finite() && p.x.abs() < FAR && p.y.abs() < FAR && p.z.abs() < FAR
}

/// A plan cell index (clamped: hostile coordinates never overflow).
fn cell_of(v: f64, cell: f64) -> i64 {
    let x = (v / cell).floor();
    if x.is_finite() { x.clamp(-1.0e12, 1.0e12) as i64 } else { 0 }
}

/// A stick's centreline from its ends: straight, or (apart both in plan and in height by more
/// than `tol`) the router's square turn inside it — up or down at the lower end, across at the
/// upper end's height.
pub(crate) fn stick_path(a: Vec3, b: Vec3, tol: f64) -> Vec<Vec3> {
    let plan = (b.x - a.x).hypot(b.y - a.y);
    if plan <= tol || (b.z - a.z).abs() <= tol {
        return vec![a, b];
    }
    if a.z < b.z { vec![a, Vec3::new(a.x, a.y, b.z), b] } else { vec![a, Vec3::new(b.x, b.y, a.z), b] }
}

/// A stick's centreline as its body is drawn: each tube of its mesh (two rings of the build's
/// sides) a piece from one ring's centre to the next; each piece apart both in plan and in
/// height a square turn ([`stick_path`]). None when the mesh isn't tubes.
fn body_path(e: &Entity, tol: f64) -> Option<Vec<Vec3>> {
    const SIDES: usize = 8;
    let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { return None };
    let n = m.vertices.len();
    if n < 2 * SIDES || n % (2 * SIDES) != 0 || n > 2 * SIDES * 10_000 {
        return None;
    }
    let off = super::bspec::stacked_by(e);
    let off3 = Vec3::new(off.x, off.y, 0.0);
    let centre = |vs: &[cadaddy_geom::Vec3]| -> Vec3 {
        let k = vs.len().max(1) as f64;
        vs.iter().fold(Vec3::new(0.0, 0.0, 0.0), |s, v| s + Vec3::new(v.x, v.y, v.z)) * (1.0 / k) - off3
    };
    let mut path: Vec<Vec3> = Vec::new();
    for tube in m.vertices.chunks(2 * SIDES) {
        let (r0, r1) = tube.split_at(SIDES);
        let (c0, c1) = (centre(r0), centre(r1));
        if !(sane(c0) && sane(c1)) {
            return None;
        }
        let piece = stick_path(c0, c1, tol);
        match path.last() {
            Some(last) if (*last - c0).len() <= tol => path.extend(piece.into_iter().skip(1)),
            _ => path.extend(piece),
        }
    }
    (path.len() >= 2).then_some(path)
}

fn path_len(p: &[Vec3]) -> f64 {
    p.windows(2).map(|w| if let [x, y] = w { (*y - *x).len() } else { 0.0 }).sum()
}

/// A mesh's box less its level's stacking move (where its level's plan has it).
fn plan_box(e: &Entity) -> Option<(Vec3, Vec3)> {
    let (lo, hi) = super::fieldview::mesh_box(e)?;
    let off = super::bspec::stacked_by(e);
    let (lo, hi) = (Vec3::new(lo[0] - off.x, lo[1] - off.y, lo[2]), Vec3::new(hi[0] - off.x, hi[1] - off.y, hi[2]));
    (sane(lo) && sane(hi)).then_some((lo, hi))
}

impl Net {
    pub(crate) fn of(d: &Drawing, sp: &BuildingSpec) -> Net {
        let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        let mut n = Net { upf, ..Net::default() };
        let rl = super::conduitroute::rules();
        let gear_word = |k: &str| {
            let k = k.to_ascii_lowercase();
            rl.gear.kinds.iter().any(|w| k.contains(w.as_str()))
        };
        for e in d.model.iter().take(MAX_ENTITIES) {
            if super::conduitroute::mep_piece(e) {
                n.piece(e, upf);
                continue;
            }
            let made = super::wallgen::made_as(e);
            if made.as_deref() == Some("device") && n.devs.len() < MAX_PIECES {
                let ps = props::read(&e.common.xdata);
                if ps.get("misplaced").is_some() {
                    continue;
                }
                let Some((lo, hi)) = plan_box(e) else { continue };
                n.what.insert(e.handle, "device".into());
                n.devs.push(Dev {
                    handle: e.handle,
                    circuit: text(&ps, "circuit"),
                    kind: text(&ps, "kind").replace('_', " ").to_ascii_lowercase(),
                    level: super::bspec::made_level(e).unwrap_or_default(),
                    lo,
                    hi,
                });
                continue;
            }
            if super::bimlook::made_tag(e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment"))
                && n.gear.len() < MAX_PIECES
                && let Some((lo, hi)) = plan_box(e)
            {
                let tag = super::bimlook::made_tag(e, "tag").unwrap_or_default().trim().to_string();
                let electrical = sp
                    .mep
                    .equipment
                    .iter()
                    .find(|q| q.tag.eq_ignore_ascii_case(&tag))
                    .is_some_and(|q| gear_word(&format!("{} {}", q.kind, q.title)) || super::gear::is_gear(q));
                n.what.insert(e.handle, "gear".into());
                n.gear.push(GearEl { handle: e.handle, tag, level: super::bspec::made_level(e).unwrap_or_default(), lo, hi, electrical });
            }
        }
        // What a conduit keeps clear of: the other trades' elements, the equipment and the drawn
        // conduit (as the model's elements classify them).
        for el in super::bimwork::elements(d) {
            if n.obstacles.len() >= MAX_OBSTACLES {
                break;
            }
            let cat = el.category.to_ascii_lowercase();
            let keep = match el.trade {
                _ if cat.starts_with("piece:") => false,
                "architecture" => false,
                "electrical" => cat == "equipment" || (cat == "conduit" && el.run.is_empty()),
                _ => !matches!(cat.as_str(), "room" | "device" | "space" | "zone") && !PASSED.iter().any(|w| cat.contains(w)),
            };
            if !keep {
                continue;
            }
            let off = d.model.get(el.handle).map_or(Vec2::new(0.0, 0.0), |e| super::bspec::stacked_by(e));
            let off3 = Vec3::new(off.x, off.y, 0.0);
            let (lo, hi) = (el.lo - off3, el.hi - off3);
            if !(sane(lo) && sane(hi)) {
                continue;
            }
            n.obstacles.push(Obstacle {
                handle: el.handle,
                level: el.level.clone(),
                category: el.category.clone(),
                system: el.system.clone(),
                trade: el.trade,
                lo,
                hi,
            });
        }
        n
    }

    fn piece(&mut self, e: &Entity, upf: f64) {
        let ps = props::read(&e.common.xdata);
        let kind = text(&ps, super::mepatoms::ATOM_KEY);
        if kind.is_empty() {
            return;
        }
        let level = super::bspec::made_level(e).unwrap_or_default();
        let links: Vec<Handle> = super::mepatoms::connects_of(e).into_iter().take(MAX_LINKS).collect();
        if !links.is_empty() && self.links.len() < MAX_PIECES {
            self.links.insert(e.handle, links.clone());
        }
        if self.what.len() < MAX_PIECES * 4 {
            self.what.insert(e.handle, kind.clone());
        }
        let length = num(&ps, "length_ft");
        let mut qty = 1.0;
        match kind.as_str() {
            "conduit" if self.sticks.len() < MAX_PIECES => {
                let Some((pa, pb)) = super::mepatoms::build::stick_ends(&ps) else { return };
                // The stored ends are where the build cut the stick; a level stacked after that
                // carries its body, not its ends: take the ends the body is at.
                let off = super::bspec::stacked_by(e);
                let off3 = Vec3::new(off.x, off.y, 0.0);
                let (a, b) = match super::fieldview::mesh_box(e) {
                    Some((lo, hi)) if off.len() > 1e-9 => {
                        let c = Vec3::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0);
                        let mid = (pa + pb) * 0.5;
                        if (c - (mid + off3)).len() < (c - mid).len() { (pa, pb) } else { (pa - off3, pb - off3) }
                    }
                    _ => (pa, pb),
                };
                if !(sane(a) && sane(b)) {
                    return;
                }
                // Its body's centreline when its mesh is the build's tubes (a stick cut from a
                // route with corners follows them), else its two ends.
                let path = body_path(e, upf / 12.0).unwrap_or_else(|| stick_path(a, b, upf / 12.0));
                let (a, b) = (path.first().copied().unwrap_or(a), path.last().copied().unwrap_or(b));
                let size = text(&ps, "size");
                let size_in = trade_in(&size);
                let body = super::mepatoms::build::body_of(&ps);
                let r = size_in.map_or(body, |s| s.max(body)) / 24.0 * upf;
                qty = path_len(&path) / upf;
                self.sticks.push(Stick {
                    handle: e.handle,
                    run: text(&ps, "run"),
                    circuit: text(&ps, "circuit"),
                    panel: text(&ps, "panel"),
                    level: level.clone(),
                    ty: text(&ps, "type"),
                    size_in,
                    size,
                    a,
                    b,
                    path,
                    r,
                    room: text(&ps, "room"),
                    vertical: text(&ps, "vertical") == "true",
                });
            }
            "wire" if self.wires.len() < MAX_PIECES => {
                qty = length.unwrap_or(0.0);
                self.wires.push(Wire {
                    run: text(&ps, "run"),
                    circuit: text(&ps, "circuit"),
                    size: awg(&text(&ps, "size")),
                    role: {
                        let c = text(&ps, "conductor");
                        if c.is_empty() && text(&ps, "item").to_ascii_lowercase().contains("ground") { "ground".into() } else { c }
                    },
                });
            }
            "cable" if self.cables.len() < MAX_PIECES => {
                qty = length.unwrap_or(0.0);
                self.cables.push(Cable {
                    handle: e.handle,
                    run: text(&ps, "run"),
                    circuit: text(&ps, "circuit"),
                    level: level.clone(),
                    ty: text(&ps, "type"),
                    room: text(&ps, "room"),
                    links: links.clone(),
                    at: plan_box(e).map(|(lo, hi)| (lo + hi) * 0.5),
                });
            }
            "box" if self.boxes.len() < MAX_PIECES => {
                if let Some((lo, hi)) = plan_box(e) {
                    self.boxes.push(BoxEl {
                        handle: e.handle,
                        of: Handle::parse_hex(&text(&ps, "box_of")),
                        cubic_in: num(&ps, "cubic_in").filter(|v| *v > 0.0),
                        size: text(&ps, "size"),
                        level: level.clone(),
                        lo,
                        hi,
                        links: links.clone(),
                    });
                }
            }
            "offset" => qty = 1.0,
            _ => {}
        }
        if self.counted.len() < MAX_PIECES * 4 {
            self.counted.push(Counted { kind, item: text(&ps, "item"), size: text(&ps, "size"), level, qty });
        }
    }
}

// ---------------------------------------------------------------- the end-to-end graph

/// The ends of a set of sticks joined where they meet and where they touch a box: a graph whose
/// nodes are the sticks' ends and the boxes, its edges the sticks (their length) and the joins.
struct Graph {
    /// Node → (node, feet).
    adj: Vec<Vec<(usize, f64)>>,
    /// The node of each box touched (index into `Net::boxes`).
    box_node: HashMap<usize, usize>,
    /// Each stick end's node and point: (node, point, stick).
    ends: Vec<(usize, Vec3, usize)>,
}

impl Graph {
    /// The graph of `sticks` (indices into `net.sticks`), ends within `tol` joined, ends within
    /// `tol` of a box joined to the box.
    fn of(net: &Net, sticks: &[usize], tol: f64, boxes_near: &BoxGrid) -> Graph {
        let mut g = Graph { adj: Vec::new(), box_node: HashMap::new(), ends: Vec::new() };
        let cell = (tol * 4.0).max(1e-6);
        let mut grid: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
        let key = |p: Vec3| (cell_of(p.x, cell), cell_of(p.y, cell), cell_of(p.z, cell));
        for &si in sticks {
            let Some(s) = net.sticks.get(si) else { continue };
            let (na, nb) = (g.adj.len(), g.adj.len() + 1);
            g.adj.push(Vec::new());
            g.adj.push(Vec::new());
            let len = path_len(&s.path) / net.upf;
            g.link(na, nb, len);
            for (node, p) in [(na, s.a), (nb, s.b)] {
                let (kx, ky, kz) = key(p);
                let mut near: Vec<usize> = Vec::new();
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for dz in -1..=1 {
                            if let Some(v) = grid.get(&(kx.saturating_add(dx), ky.saturating_add(dy), kz.saturating_add(dz))) {
                                near.extend(v.iter().copied());
                            }
                        }
                    }
                }
                for j in near {
                    if let Some((other, q, _)) = g.ends.get(j).copied()
                        && (q - p).len() <= tol
                    {
                        g.link(node, other, 0.0);
                    }
                }
                grid.entry((kx, ky, kz)).or_default().push(g.ends.len());
                g.ends.push((node, p, si));
                for bi in boxes_near.near(p, tol, net) {
                    let bn = match g.box_node.get(&bi) {
                        Some(x) => *x,
                        None => {
                            g.adj.push(Vec::new());
                            let x = g.adj.len() - 1;
                            g.box_node.insert(bi, x);
                            x
                        }
                    };
                    g.link(node, bn, 0.0);
                }
            }
        }
        g
    }

    /// The nodes joined to `n`.
    fn component(&self, n: usize) -> BTreeSet<usize> {
        let mut seen = BTreeSet::new();
        let mut todo = vec![n];
        while let Some(x) = todo.pop() {
            if !seen.insert(x) {
                continue;
            }
            todo.extend(self.adj.get(x).into_iter().flatten().map(|(m, _)| *m).filter(|m| !seen.contains(m)));
        }
        seen
    }

    fn link(&mut self, a: usize, b: usize, w: f64) {
        if let Some(x) = self.adj.get_mut(a) {
            x.push((b, w));
        }
        if let Some(x) = self.adj.get_mut(b) {
            x.push((a, w));
        }
    }

    /// The shortest distance (feet) from any of `sources` to every node, with the source each
    /// came from.
    fn from(&self, sources: &[(usize, usize)]) -> Vec<Option<(f64, usize)>> {
        #[derive(PartialEq)]
        struct St(f64, usize, usize);
        impl Eq for St {}
        impl PartialOrd for St {
            fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for St {
            fn cmp(&self, o: &Self) -> Ordering {
                o.0.total_cmp(&self.0).then_with(|| o.1.cmp(&self.1))
            }
        }
        let mut dist: Vec<Option<(f64, usize)>> = vec![None; self.adj.len()];
        let mut heap = BinaryHeap::new();
        for &(n, src) in sources {
            if let Some(slot) = dist.get_mut(n)
                && slot.is_none_or(|(d, _)| d > 0.0)
            {
                *slot = Some((0.0, src));
                heap.push(St(0.0, n, src));
            }
        }
        while let Some(St(d, n, src)) = heap.pop() {
            if dist.get(n).copied().flatten().is_some_and(|(x, _)| x < d - 1e-12) {
                continue;
            }
            for &(m, w) in self.adj.get(n).map_or(&[][..], Vec::as_slice) {
                let nd = d + w;
                if let Some(slot) = dist.get_mut(m)
                    && slot.is_none_or(|(x, _)| nd < x - 1e-12)
                {
                    *slot = Some((nd, src));
                    heap.push(St(nd, m, src));
                }
            }
        }
        dist
    }
}

/// The boxes on a plan grid, for "which boxes does this end touch".
struct BoxGrid {
    cell: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
}

impl BoxGrid {
    fn of(net: &Net) -> BoxGrid {
        let cell = (CELL_FT * net.upf).max(1e-6);
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, b) in net.boxes.iter().enumerate() {
            let (x0, y0, x1, y1) = (cell_of(b.lo.x, cell), cell_of(b.lo.y, cell), cell_of(b.hi.x, cell), cell_of(b.hi.y, cell));
            if x1.saturating_sub(x0) > 4 || y1.saturating_sub(y0) > 4 {
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    cells.entry((x, y)).or_default().push(i);
                }
            }
        }
        BoxGrid { cell, cells }
    }
    /// The boxes within `tol` of `p`.
    fn near(&self, p: Vec3, tol: f64, net: &Net) -> Vec<usize> {
        let k = (cell_of(p.x, self.cell), cell_of(p.y, self.cell));
        let mut out = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &i in self.cells.get(&(k.0.saturating_add(dx), k.1.saturating_add(dy))).map_or(&[][..], Vec::as_slice) {
                    if let Some(b) = net.boxes.get(i)
                        && box_dist2(p, b.lo, b.hi) <= tol * tol
                        && !out.contains(&i)
                    {
                        out.push(i);
                    }
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------- the check

pub(crate) struct Ctx<'a> {
    pub d: &'a Drawing,
    pub sp: &'a BuildingSpec,
    pub frame: &'a Frame,
    pub areas: &'a [Area],
    pub whole_levels: bool,
    /// `conduit.takeoff`'s answer (`{price: false, group_by: [level]}`), run by the caller.
    pub takeoff: Option<&'a Value>,
}

/// The rules switched on.
#[derive(Default, Clone, Copy)]
pub(crate) struct On<'a> {
    pub continuity: Option<&'a Effective>,
    pub fill: Option<&'a Effective>,
    pub box_fill: Option<&'a Effective>,
    pub derate: Option<&'a Effective>,
    pub vdrop: Option<&'a Effective>,
    pub method: Option<&'a Effective>,
    pub clash: Option<&'a Effective>,
    pub takeoff: Option<&'a Effective>,
}

impl<'a> On<'a> {
    pub(crate) fn of(on: impl Fn(&str) -> Option<&'a Effective>) -> On<'a> {
        On {
            continuity: on(CONTINUITY),
            fill: on(FILL),
            box_fill: on(BOX_FILL),
            derate: on(DERATE),
            vdrop: on(VDROP),
            method: on(METHOD),
            clash: on(CLASH),
            takeoff: on(TAKEOFF),
        }
    }
    fn any(&self) -> bool {
        [self.continuity, self.fill, self.box_fill, self.derate, self.vdrop, self.method, self.clash, self.takeoff].iter().any(Option::is_some)
    }
}

/// What the conduit rules found.
#[derive(Default)]
pub(crate) struct Checked {
    /// (rank, rule, verdict, finding).
    pub findings: Vec<(u8, &'static str, &'static str, Value)>,
    /// Per rule: why → objects not measured.
    pub unmeasured: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Checked {
    fn unmeasured(&mut self, rule: &str, why: impl Into<String>, n: u64) {
        if n > 0 {
            *self.unmeasured.entry(rule.to_string()).or_default().entry(why.into()).or_default() += n;
        }
    }
}

/// `conduit.takeoff`'s lines for E-TAKEOFF (unpriced, by level), when the rule runs.
pub(crate) fn read_takeoff(s: &mut crate::Session, rules: &[Effective], only: &[String]) -> Option<Value> {
    let on = rules.iter().any(|r| r.enabled && r.rule.id == TAKEOFF) && (only.is_empty() || only.iter().any(|o| o.eq_ignore_ascii_case(TAKEOFF)));
    if !on {
        return None;
    }
    s.execute("conduit.takeoff", &json!({"price": false, "group_by": ["level"]})).ok()
}

/// A breaker's amps and poles, and its load (VA), from its panel's schedule.
fn breaker(sp: &BuildingSpec, panel: &str, number: &str) -> Option<(f64, u32, Option<f64>, String)> {
    let s = sp
        .mep
        .panel_schedules
        .iter()
        .find(|p| p.panel.trim().eq_ignore_ascii_case(panel.trim()))
        .or_else(|| sp.mep.panel_schedules.iter().find(|p| is_panel(panel, &p.panel)))?;
    let c = s.circuits.iter().find(|c| c.number.trim() == number.trim())?;
    let digits: String = c.breaker.chars().skip_while(|ch| !ch.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    let amps = digits.parse::<f64>().ok().filter(|a| (5.0..=4000.0).contains(a))?;
    let src =
        format!("panel {} schedule{} circuit {}", s.panel, if s.source.is_empty() { String::new() } else { format!(" ({})", s.source) }, c.number);
    Some((amps, c.poles.max(1), c.load_va.filter(|v| v.is_finite() && *v > 0.0 && *v < 1.0e6), src))
}

/// A panel's voltages from its schedule's header (`120/208`): line to neutral, line to line.
fn panel_volts(sp: &BuildingSpec, panel: &str) -> Option<(f64, f64)> {
    let s = sp.mep.panel_schedules.iter().find(|p| p.panel.trim().eq_ignore_ascii_case(panel.trim()) || is_panel(panel, &p.panel))?;
    for (k, v) in &s.header {
        if !k.to_ascii_lowercase().contains("volt") {
            continue;
        }
        let nums: Vec<f64> = v
            .split(|c: char| !c.is_ascii_digit())
            .filter(|x| !x.is_empty())
            .filter_map(|x| x.parse::<f64>().ok())
            .filter(|x| (100.0..=600.0).contains(x))
            .collect();
        if let [a, b, ..] = nums.as_slice() {
            return Some((a.min(*b), a.max(*b)));
        }
    }
    None
}

/// Run the conduit rules over the scope.
pub(crate) fn check(cx: &Ctx, on: On) -> Checked {
    let mut out = Checked::default();
    if !on.any() {
        return out;
    }
    let net = Net::of(cx.d, cx.sp);
    let rl = super::conduitroute::rules();
    let upf = net.upf;
    let frame = cx.frame;
    let in_scope = |level: &str, p: Vec3| -> Option<(Option<&Area>, [f64; 2])> {
        let at = frame.to_ft_on(level, Vec2::new(p.x, p.y));
        area_for(cx.areas, cx.whole_levels, level, at).map(|a| (a, at))
    };
    let mut got: Vec<(u8, &'static str, &'static str, Value)> = Vec::new();
    let mut push = |rule: &'static str,
                    r: &Effective,
                    handle: Option<Handle>,
                    kind: &str,
                    key: &str,
                    verdict: &'static str,
                    issue: String,
                    expected: String,
                    found: String,
                    measured: Value,
                    level: &str,
                    place: (Option<&Area>, [f64; 2]),
                    source: &str| {
        if got.len() >= MAX_FINDINGS * ALL.len() {
            return;
        }
        let id = format!("PS-{}", super::validate_run::hash8(&format!("{rule}|{level}|{key}|{verdict}")));
        got.push((
            rank(verdict, &r.rule.severity),
            rule,
            verdict,
            json!({
                "id": id, "rule": rule, "handle": handle.map(|h| h.hex()), "kind": kind, "verdict": verdict,
                "issue": issue, "expected": expected, "found": found, "measured": measured, "evidence": [],
                "confidence": if verdict == "unsure" { 0.5 } else { 1.0 },
                "severity": r.rule.severity, "area": place.0.map(|a| a.id.clone()), "level": level,
                "at": pt([r2(place.1[0]), r2(place.1[1])]), "source": source,
            }),
        ));
    };

    let routed = !net.sticks.is_empty() || !net.cables.is_empty();
    // Runs: sticks, wires and cables by run.
    let mut run_sticks: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, s) in net.sticks.iter().enumerate().filter(|(_, s)| !s.run.is_empty()) {
        run_sticks.entry(s.run.as_str()).or_default().push(i);
    }
    let mut run_wires: HashMap<&str, Vec<&Wire>> = HashMap::new();
    for w in net.wires.iter().filter(|w| !w.run.is_empty()) {
        run_wires.entry(w.run.as_str()).or_default().push(w);
    }
    // A run's place: its longest stick's middle.
    let run_place = |ids: &[usize]| -> Option<(&Stick, Vec3)> {
        let s = ids.iter().filter_map(|i| net.sticks.get(*i)).max_by(|a, b| (a.b - a.a).len().total_cmp(&(b.b - b.a).len()))?;
        Some((s, (s.a + s.b) * 0.5))
    };
    let boxes_near = BoxGrid::of(&net);

    // ---- E-CONDUIT-FILL and E-DERATE: each raceway run.
    if on.fill.is_some() || on.derate.is_some() {
        if !routed {
            for rr in [on.fill.map(|_| FILL), on.derate.map(|_| DERATE)].into_iter().flatten() {
                out.unmeasured(rr, "no routed raceway in the model (model.build routes the circuits)", 1);
            }
        }
        for (run, ids) in &run_sticks {
            let Some((lead, mid)) = run_place(ids) else { continue };
            let Some(place) = in_scope(&lead.level, mid) else { continue };
            let wires: &[&Wire] = run_wires.get(run).map_or(&[], Vec::as_slice);
            let ty = lead.ty.to_ascii_uppercase();
            let base = json!({"run": run, "circuit": lead.circuit, "raceway": format!("{} {}", lead.size, lead.ty).trim(), "sticks": ids.len()});
            let with = |extra: Value| {
                let mut m = base.clone();
                if let (Some(o), Some(x)) = (m.as_object_mut(), extra.as_object()) {
                    for (k, v) in x {
                        o.insert(k.clone(), v.clone());
                    }
                }
                m
            };
            let makeup = {
                let mut by: BTreeMap<(bool, String), u32> = BTreeMap::new();
                for w in wires {
                    *by.entry((w.ground(), w.size.clone())).or_default() += 1;
                }
                by.iter().map(|((g, s), n)| format!("{n}#{s}{}", if *g { "G" } else { "" })).collect::<Vec<_>>().join(" + ")
            };
            if let Some(r) = on.fill {
                let area = lead.size_in.and_then(|si| {
                    rl.fill.area_sq_in.get(&ty).and_then(|rows| rows.iter().find(|row| row.first().is_some_and(|x| (x - si).abs() < 0.01)))
                });
                let wire_areas: Vec<Option<f64>> = wires.iter().map(|w| rl.fill.wire_sq_in.get(&w.size).copied()).collect();
                if wires.is_empty() {
                    out.unmeasured(FILL, "a raceway run with no conductors in the model", 1);
                } else if let Some(row) = area {
                    let total = row.get(1).copied().unwrap_or(0.0);
                    let pct = match wires.len() {
                        1 => rl.fill.one_pct,
                        2 => rl.fill.two_pct,
                        _ => rl.fill.over_two_pct,
                    };
                    let allowed = total * pct / 100.0;
                    if wire_areas.iter().any(Option::is_none) {
                        let sizes: BTreeSet<String> =
                            wires.iter().filter(|w| !rl.fill.wire_sq_in.contains_key(&w.size)).map(|w| format!("#{}", w.size)).collect();
                        push(
                            FILL,
                            r,
                            Some(lead.handle),
                            "conduit",
                            run,
                            "unsure",
                            format!(
                                "raceway {run}: a conductor size the area table doesn't list ({})",
                                sizes.into_iter().collect::<Vec<_>>().join(", ")
                            ),
                            format!("conductors within {pct}% of the raceway's {total} sq in ({})", rl.fill.source),
                            makeup.clone(),
                            with(json!({"check": "fill", "raceway_sq_in": total, "allowed_pct": pct})),
                            &lead.level,
                            place,
                            "prescreen",
                        );
                    } else {
                        let used: f64 = wire_areas.iter().flatten().sum();
                        let fill_pct = if total > 0.0 { used / total * 100.0 } else { f64::INFINITY };
                        let ok = used <= allowed + 1e-9;
                        push(
                            FILL,
                            r,
                            Some(lead.handle),
                            "conduit",
                            run,
                            if ok { "pass" } else { "fail" },
                            if ok {
                                format!("raceway {run} is {:.1}% full", fill_pct)
                            } else {
                                format!("raceway {run} is overfilled: {:.1}% of its area, {pct}% allowed for {} conductors", fill_pct, wires.len())
                            },
                            format!("at most {} sq in of conductors ({pct}% of {total} sq in, {})", r3(allowed), rl.fill.source),
                            format!("{makeup}: {} sq in", r3(used)),
                            with(
                                json!({"check": "fill", "conductors": wires.len(), "conductor_sq_in": r3(used), "raceway_sq_in": total, "allowed_pct": pct, "fill_pct": r1(fill_pct)}),
                            ),
                            &lead.level,
                            place,
                            "prescreen",
                        );
                    }
                } else {
                    push(
                        FILL,
                        r,
                        Some(lead.handle),
                        "conduit",
                        run,
                        "unsure",
                        format!("raceway {run}: no area table for {} {} ({})", lead.size, lead.ty, rl.fill.source),
                        "conductors within Chapter 9's fill of the raceway's area".into(),
                        makeup.clone(),
                        with(json!({"check": "fill", "conductors": wires.len()})),
                        &lead.level,
                        place,
                        "prescreen",
                    );
                }
            }
            if let Some(r) = on.derate {
                if lead.circuit.is_empty() {
                    out.unmeasured(DERATE, "a raceway with no branch circuit (a feeder's): E-GEAR-FILL checks its conductors at the gear", 1);
                    continue;
                }
                let hots: Vec<&&Wire> = wires.iter().filter(|w| !w.ground()).collect();
                if hots.is_empty() {
                    out.unmeasured(DERATE, "a raceway run with no conductors in the model", 1);
                    continue;
                }
                let ccc = u32::try_from(hots.len()).unwrap_or(u32::MAX);
                let factor = if ccc <= 3 {
                    1.0
                } else {
                    rl.derate
                        .factors
                        .iter()
                        .find(|f| f.first().is_some_and(|a| f64::from(ccc) >= *a) && f.get(1).is_some_and(|b| f64::from(ccc) <= *b))
                        .and_then(|f| f.get(2).copied())
                        .unwrap_or(0.35)
                };
                let breakers: Vec<Option<(f64, u32, Option<f64>, String)>> = match circuit_of(&lead.circuit) {
                    Some((panel, nums)) => nums.iter().map(|n| breaker(cx.sp, if lead.panel.is_empty() { &panel } else { &lead.panel }, n)).collect(),
                    None => vec![None],
                };
                let mut sizes: Vec<String> = hots.iter().map(|w| w.size.clone()).collect();
                sizes.sort();
                sizes.dedup();
                let allowed = |s: &str| -> Option<f64> {
                    let a90 = rl.derate.amp_90c.get(s).copied()?;
                    let a75 = rl.derate.amp_75c.get(s).copied()?;
                    let small = rl.derate.small_conductor_max_a.get(s).copied().unwrap_or(f64::INFINITY);
                    Some((a90 * factor).min(a75).min(small))
                };
                let least = sizes.iter().map(|s| (s.clone(), allowed(s))).min_by(|a, b| a.1.unwrap_or(-1.0).total_cmp(&b.1.unwrap_or(-1.0)));
                let m = with(json!({
                    "check": "derate", "ccc": ccc, "factor": factor, "conductors": makeup,
                    "breakers": breakers.iter().map(|b| b.as_ref().map(|(a, p, _, src)| json!({"amps": a, "poles": p, "from": src}))).collect::<Vec<_>>(),
                }));
                let expected = format!(
                    "every conductor's ampacity after the {} adjustment for {ccc} current-carrying conductors (the 90 °C column, never above the 75 °C column or NEC 240.4(D)) at least its breaker ({})",
                    if ccc > 3 { format!("{}", factor) } else { "no".into() },
                    rl.derate.source
                );
                match (breakers.iter().all(Option::is_some), least) {
                    (true, Some((size, Some(amps)))) => {
                        let trip = breakers.iter().flatten().map(|b| b.0).fold(0.0, f64::max);
                        let ok = amps + 1e-9 >= trip;
                        let mut m = m;
                        m["allowed_a"] = json!(r1(amps));
                        m["breaker_a"] = json!(trip);
                        m["smallest"] = json!(format!("#{size}"));
                        push(
                            DERATE,
                            r,
                            Some(lead.handle),
                            "conduit",
                            run,
                            if ok { "pass" } else { "fail" },
                            if ok {
                                format!("raceway {run}: {ccc} current-carrying conductors, #{size} good for {} A on a {trip} A breaker", r1(amps))
                            } else {
                                format!(
                                    "raceway {run}: {ccc} current-carrying conductors derate #{size} to {} A, under its {trip} A breaker",
                                    r1(amps)
                                )
                            },
                            expected,
                            format!("{makeup}; factor {factor}: #{size} {} A", r1(amps)),
                            m,
                            &lead.level,
                            place,
                            "prescreen",
                        );
                    }
                    (true, _) => push(
                        DERATE,
                        r,
                        Some(lead.handle),
                        "conduit",
                        run,
                        "unsure",
                        format!("raceway {run}: a conductor size the ampacity table doesn't list"),
                        expected,
                        makeup.clone(),
                        m,
                        &lead.level,
                        place,
                        "prescreen",
                    ),
                    (false, _) => push(
                        DERATE,
                        r,
                        Some(lead.handle),
                        "conduit",
                        run,
                        "unsure",
                        format!("raceway {run}: the breaker of circuit {} isn't in a panel schedule", lead.circuit),
                        expected,
                        makeup.clone(),
                        m,
                        &lead.level,
                        place,
                        "prescreen",
                    ),
                }
            }
        }
    }

    // ---- E-METHOD: each run's method against the area its pieces are in.
    if let Some(r) = on.method {
        let methods = super::conduitroute::Methods::stored(cx.d);
        let mr = &rl.methods;
        let rule_method =
            cx.d.project_rules
                .get(cadaddy_doc::projrules::RuleDomain::WiringMethod, "general")
                .map(|r| r.active.value.trim().to_string())
                .filter(|v| !v.is_empty());
        let approved = super::valueeng::approved_circuits(cx.d);
        let has = |text: &str, words: &[String]| words.iter().any(|w| text.contains(w.as_str()));
        let name_of = |key: &str| -> Option<String> {
            let v = match key {
                "raceway" => &mr.raceway,
                "mc" => &mr.mc,
                "ac" => &mr.ac,
                "nm" => &mr.nm,
                "se" => &mr.se,
                "uf" => &mr.uf,
                _ => return None,
            };
            Some(v.clone()).filter(|v| !v.is_empty())
        };
        let branch: Vec<&super::conduitroute::MethodApp> = methods
            .applications
            .iter()
            .filter(|a| {
                let up = a.application.to_ascii_uppercase();
                !has(&up, &mr.feeder_words) || has(&up, &mr.branch_words)
            })
            .collect();
        let app_for = |area: &str| -> Option<&super::conduitroute::MethodApp> {
            let pick = |pred: &dyn Fn(&str) -> bool| branch.iter().copied().find(|a| pred(&a.application.to_ascii_uppercase()));
            match area {
                "unit" => pick(&|u| has(u, &mr.unit_words))
                    .or_else(|| pick(&|u| has(u, &mr.concealed_words) && !has(u, &mr.exposed_words) && !has(u, &mr.below_words))),
                "exposed" => pick(&|u| has(u, &mr.exposed_words) && !has(u, &mr.unit_words)),
                _ => pick(&|u| has(u, &mr.concealed_words) && !has(u, &mr.exposed_words) && !has(u, &mr.unit_words) && !has(u, &mr.below_words)),
            }
        };
        let area_of = |room: &str| {
            let up = room.to_ascii_uppercase();
            if has(&up, &mr.unit_words) {
                "unit"
            } else if has(&up, &mr.exposed_rooms) {
                "exposed"
            } else {
                "concealed"
            }
        };
        // Each run: its method, the rooms its pieces are in, a place.
        struct RunM<'b> {
            method: String,
            circuit: String,
            level: String,
            handle: Handle,
            at: Vec3,
            rooms: BTreeSet<&'b str>,
        }
        let mut runs: BTreeMap<String, RunM> = BTreeMap::new();
        for s in net.sticks.iter().filter(|s| !s.run.is_empty() && !s.circuit.is_empty()) {
            let e = runs.entry(s.run.clone()).or_insert_with(|| RunM {
                method: s.ty.clone(),
                circuit: s.circuit.clone(),
                level: s.level.clone(),
                handle: s.handle,
                at: (s.a + s.b) * 0.5,
                rooms: BTreeSet::new(),
            });
            e.rooms.insert(s.room.as_str());
        }
        for c in net.cables.iter().filter(|c| !c.run.is_empty() && !c.circuit.is_empty()) {
            let at = c.at.unwrap_or(Vec3::new(f64::NAN, f64::NAN, 0.0));
            let e = runs.entry(c.run.clone()).or_insert_with(|| RunM {
                method: c.ty.split_whitespace().next().unwrap_or_default().to_string(),
                circuit: c.circuit.clone(),
                level: c.level.clone(),
                handle: c.handle,
                at,
                rooms: BTreeSet::new(),
            });
            e.rooms.insert(c.room.as_str());
        }
        if runs.is_empty() {
            out.unmeasured(METHOD, "no routed raceway or cable in the model", 1);
        }
        for (run, m) in &runs {
            if !m.at.is_finite() {
                out.unmeasured(METHOD, "a cable run whose body the model doesn't place", 1);
                continue;
            }
            let Some(place) = in_scope(&m.level, m.at) else { continue };
            let method_up = m.method.to_ascii_uppercase();
            let areas: BTreeSet<&str> = m.rooms.iter().map(|r| area_of(r)).collect();
            let measured = json!({"check": "method", "run": run, "circuit": m.circuit, "method": m.method, "areas": areas, "rooms": m.rooms.iter().filter(|r| !r.is_empty()).take(12).collect::<Vec<_>>()});
            // An approved value engineering alternative, the project's rule, the documents' table.
            let ve = approved.iter().find(|(c, _)| c.eq_ignore_ascii_case(&m.circuit) || c.ends_with(&format!(":{}", m.circuit)));
            if let Some((_, (alt, why))) =
                ve.filter(|(_, (alt, _))| alt.to_ascii_uppercase().starts_with(&method_up) || method_up.starts_with(&alt.to_ascii_uppercase()))
            {
                push(
                    METHOD,
                    r,
                    Some(m.handle),
                    "conduit",
                    run,
                    "pass",
                    format!("run {run} is {}: {why}", m.method),
                    format!("{alt} ({why})"),
                    m.method.clone(),
                    measured,
                    &m.level,
                    place,
                    "prescreen",
                );
                continue;
            }
            if let Some(v) = &rule_method {
                let ok = v.to_ascii_uppercase().contains(&method_up) && !method_up.is_empty();
                push(
                    METHOD,
                    r,
                    Some(m.handle),
                    "conduit",
                    run,
                    if ok { "pass" } else { "fail" },
                    if ok {
                        format!("run {run} is {} as the project's wiring-method rule says", m.method)
                    } else {
                        format!("run {run} is {}; the project's wiring-method rule says {v}", m.method)
                    },
                    format!("{v} (project rule wiring_method/general)"),
                    m.method.clone(),
                    measured,
                    &m.level,
                    place,
                    "prescreen",
                );
                continue;
            }
            let mut allowed_all: Vec<(String, Vec<String>, String)> = Vec::new();
            let mut missing = Vec::new();
            for a in &areas {
                match app_for(a) {
                    Some(app) => {
                        let names: Vec<String> = app.methods.iter().filter_map(|k| name_of(k)).collect();
                        allowed_all.push((a.to_string(), names, format!("spec {} {}: {}", app.section, app.item, app.application)));
                    }
                    None => missing.push(a.to_string()),
                }
            }
            if !missing.is_empty() || allowed_all.is_empty() {
                push(
                    METHOD,
                    r,
                    Some(m.handle),
                    "conduit",
                    run,
                    "unsure",
                    format!("run {run} is {}: no wiring-methods table read for its {} area", m.method, missing.join(", ")),
                    "the method the specification's wiring-methods table allows for the area".into(),
                    m.method.clone(),
                    measured,
                    &m.level,
                    place,
                    "prescreen",
                );
                continue;
            }
            let bad: Vec<&(String, Vec<String>, String)> =
                allowed_all.iter().filter(|(_, names, _)| !names.iter().any(|n| n.eq_ignore_ascii_case(&m.method))).collect();
            let expected = allowed_all.iter().map(|(a, n, src)| format!("{a}: {} ({src})", n.join(", "))).collect::<Vec<_>>().join("; ");
            push(
                METHOD,
                r,
                Some(m.handle),
                "conduit",
                run,
                if bad.is_empty() { "pass" } else { "fail" },
                if bad.is_empty() {
                    format!("run {run} is {}, allowed where it runs", m.method)
                } else {
                    format!(
                        "run {run} is {}, which the specification doesn't allow in its {} area",
                        m.method,
                        bad.iter().map(|b| b.0.as_str()).collect::<Vec<_>>().join(", ")
                    )
                },
                expected,
                m.method.clone(),
                measured,
                &m.level,
                place,
                "prescreen",
            );
        }
    }

    // ---- The circuits end to end: E-CONDUIT-CONTINUITY and E-VDROP.
    if on.continuity.is_some() || on.vdrop.is_some() {
        let touch = on.continuity.map_or(DEFAULT_TOUCH_IN, |r| tol(r, "touch_in", DEFAULT_TOUCH_IN)) / 12.0 * upf;
        if !routed {
            for rr in [on.continuity.map(|_| CONTINUITY), on.vdrop.map(|_| VDROP)].into_iter().flatten() {
                out.unmeasured(rr, "no routed raceway or cable in the model (model.build routes the circuits)", 1);
            }
        }
        // Each (panel, circuit number): its devices and the sticks and cables carrying it.
        let mut circ_sticks: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
        for (i, s) in net.sticks.iter().enumerate() {
            if let Some((panel, nums)) = circuit_of(&s.circuit) {
                for n in nums {
                    circ_sticks.entry((panel.clone(), n)).or_default().push(i);
                }
            }
        }
        let mut circ_cables: BTreeMap<(String, String), usize> = BTreeMap::new();
        for c in &net.cables {
            if let Some((panel, nums)) = circuit_of(&c.circuit) {
                for n in nums {
                    *circ_cables.entry((panel.clone(), n)).or_default() += 1;
                }
            }
        }
        let mut circ_devs: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
        let mut no_circuit = 0u64;
        for (i, dv) in net.devs.iter().enumerate() {
            match circuit_of(&dv.circuit) {
                Some((panel, nums)) => {
                    for n in nums.into_iter().take(1) {
                        circ_devs.entry((panel.clone(), n)).or_default().push(i);
                    }
                }
                None => no_circuit += 1,
            }
        }
        if let Some(_r) = on.continuity {
            out.unmeasured(CONTINUITY, "a device with no circuit (nothing to follow)", no_circuit);
        }
        let box_of: HashMap<Handle, usize> = net.boxes.iter().enumerate().filter_map(|(i, b)| Some((b.of?, i))).collect();
        let gear_by_panel =
            |panel: &str| -> Vec<usize> { net.gear.iter().enumerate().filter(|(_, g)| is_panel(&g.tag, panel)).map(|(i, _)| i).collect() };
        for ((panel, number), devs) in &circ_devs {
            let label = format!("{panel}-{number}");
            let sticks = circ_sticks.get(&(panel.clone(), number.clone())).cloned().unwrap_or_default();
            let cables = circ_cables.get(&(panel.clone(), number.clone())).copied().unwrap_or(0);
            let gears = gear_by_panel(panel);
            let g = Graph::of(&net, &sticks, touch, &boxes_near);
            // The panel's side: every end within the touch of its box.
            let mut sources: Vec<(usize, usize)> = Vec::new();
            for &(node, p, _) in &g.ends {
                for &gi in &gears {
                    if let Some(ge) = net.gear.get(gi)
                        && box_dist2(p, ge.lo, ge.hi) <= touch * touch
                    {
                        sources.push((node, gi));
                    }
                }
            }
            let dist = g.from(&sources);
            // Each device: its box's node, reached?
            let mut reach: Vec<(usize, Option<(f64, usize)>)> = Vec::new();
            for &di in devs {
                let Some(dv) = net.devs.get(di) else { continue };
                let c = (dv.lo + dv.hi) * 0.5;
                let Some(place) = in_scope(&dv.level, c) else { continue };
                let bi = box_of.get(&dv.handle).copied();
                let (blo, bhi) = bi.and_then(|i| net.boxes.get(i)).map_or((dv.lo, dv.hi), |b| (b.lo, b.hi));
                let node = bi.and_then(|i| g.box_node.get(&i).copied());
                // The nearest end of its circuit to its box when none touches it.
                let nearest = g.ends.iter().map(|(n, p, _)| (box_dist2(*p, blo, bhi).sqrt(), *n)).min_by(|a, b| a.0.total_cmp(&b.0));
                let reached = node.and_then(|n| dist.get(n).copied().flatten());
                reach.push((di, reached));
                let Some(r) = on.continuity else { continue };
                let measured_base = json!({"check": "continuity", "circuit": label, "device": dv.handle.hex(), "device_kind": dv.kind, "box": bi.and_then(|i| net.boxes.get(i)).map(|b| b.handle.hex()), "sticks": sticks.len()});
                let expected = format!(
                    "a raceway of circuit {label} from panel {panel} to its box, every drop ending on the box (within {} in)",
                    r2(touch / upf * 12.0)
                );
                let (verdict, issue, found, extra) = if sticks.is_empty() && cables > 0 {
                    // A cable run: followed by its pieces' links.
                    let ok = linked_to_panel(&net, bi.map(|i| net.boxes.get(i).map_or(dv.handle, |b| b.handle)).unwrap_or(dv.handle), panel);
                    if ok {
                        (
                            "pass",
                            format!("{} on {label}: its cable is linked to panel {panel}", dv.kind),
                            "linked by its cable's pieces".to_string(),
                            json!({"by": "links"}),
                        )
                    } else {
                        (
                            "fail",
                            format!("{} on {label}: its cable never reaches panel {panel}", dv.kind),
                            "the cable's pieces stop short of the panel".to_string(),
                            json!({"by": "links"}),
                        )
                    }
                } else if sticks.is_empty() {
                    (
                        "fail",
                        format!("{} on {label}: no raceway or cable of its circuit reaches it", dv.kind),
                        "no piece of the circuit".to_string(),
                        json!({}),
                    )
                } else if node.is_none() {
                    let gap_in = nearest.map(|(d, _)| d / upf * 12.0);
                    (
                        "fail",
                        match gap_in {
                            Some(gi) if gi <= FAR_GAP_IN => format!("{} on {label}: the raceway stops {:.1} in short of its box", dv.kind, gi),
                            _ => format!("{} on {label}: no raceway of its circuit comes within {} ft of its box", dv.kind, FAR_GAP_IN / 12.0),
                        },
                        gap_in.map_or_else(|| "no end".to_string(), |gi| format!("nearest end {:.1} in from the box", gi)),
                        json!({"gap_in": gap_in.map(r2)}),
                    )
                } else if reached.is_none() {
                    // How near its own piece of raceway comes to a panel of its tag.
                    let near = node.map(|n| g.component(n)).and_then(|comp| {
                        g.ends
                            .iter()
                            .filter(|(en, _, _)| comp.contains(en))
                            .flat_map(|(_, p, _)| {
                                gears.iter().filter_map(|gi| net.gear.get(*gi)).map(move |ge| (box_dist2(*p, ge.lo, ge.hi).sqrt(), ge.tag.clone()))
                            })
                            .min_by(|a, b| a.0.total_cmp(&b.0))
                    });
                    let near_in = near.as_ref().map(|(d, _)| r1(d / upf * 12.0));
                    (
                        "fail",
                        match &near {
                            Some((_, tag)) => format!(
                                "{} on {label}: its raceway is not joined to panel {panel}; its nearest end is {} in from panel {tag}",
                                dv.kind,
                                near_in.unwrap_or(0.0)
                            ),
                            None => {
                                format!("{} on {label}: its raceway is not joined to panel {panel}, and no panel {panel} is in the model", dv.kind)
                            }
                        },
                        format!("{} end(s) of the circuit at a panel {panel}; the device's piece is cut off", sources.len()),
                        json!({"panel_ends": sources.len(), "nearest_panel": near.as_ref().map(|x| x.1.clone()), "nearest_panel_in": near_in}),
                    )
                } else {
                    let ft = reached.map_or(0.0, |x| x.0);
                    (
                        "pass",
                        format!("{} on {label} is joined to panel {panel}", dv.kind),
                        format!("{:.1} ft of raceway from the panel", ft),
                        json!({"from_panel_ft": r1(ft)}),
                    )
                };
                let mut m = measured_base;
                if let (Some(o), Some(x)) = (m.as_object_mut(), extra.as_object()) {
                    for (k, v) in x {
                        o.insert(k.clone(), v.clone());
                    }
                }
                push(
                    CONTINUITY,
                    r,
                    Some(dv.handle),
                    &dv.kind,
                    &format!("{}|{}", label, dv.handle.hex()),
                    verdict,
                    issue,
                    expected,
                    found,
                    m,
                    &dv.level,
                    place,
                    "prescreen",
                );
            }
            // E-VDROP: each panel instance's farthest device.
            let Some(r) = on.vdrop else { continue };
            if sticks.is_empty() {
                if cables > 0 {
                    out.unmeasured(VDROP, "a cable circuit (its length is the cable's; not followed end to end)", 1);
                }
                continue;
            }
            let mut by_gear: BTreeMap<usize, (f64, usize)> = BTreeMap::new();
            for (di, rch) in &reach {
                if let Some((ft, gi)) = rch {
                    let e = by_gear.entry(*gi).or_insert((*ft, *di));
                    if *ft > e.0 {
                        *e = (*ft, *di);
                    }
                }
            }
            if by_gear.is_empty() {
                out.unmeasured(VDROP, "a circuit not joined to its panel (E-CONDUIT-CONTINUITY fails it)", 1);
            }
            for (gi, (ft, di)) in by_gear {
                let Some(dv) = net.devs.get(di) else { continue };
                let Some(place) = in_scope(&dv.level, (dv.lo + dv.hi) * 0.5) else { continue };
                let gtag = net.gear.get(gi).map_or_else(|| panel.clone(), |g| g.tag.clone());
                // Its conductor: the smallest of its own runs' hots, else the phase it is on in a
                // shared run.
                let runs: BTreeSet<&str> = sticks.iter().filter_map(|i| net.sticks.get(*i)).map(|s| s.run.as_str()).collect();
                let own: Vec<&str> = runs
                    .iter()
                    .copied()
                    .filter(|run| {
                        run_wires.get(run).is_some_and(|ws| ws.first().and_then(|w| circuit_of(&w.circuit)).is_some_and(|(_, n)| n.len() == 1))
                    })
                    .collect();
                let phase = number.parse::<u32>().ok().map(|n| format!("phase {}", super::mepatoms::phase_of(n)));
                let pick: Vec<String> = if own.is_empty() {
                    runs.iter()
                        .flat_map(|run| run_wires.get(run).into_iter().flatten())
                        .filter(|w| !w.ground() && phase.as_ref().is_none_or(|p| w.role.eq_ignore_ascii_case(p)))
                        .map(|w| w.size.clone())
                        .collect()
                } else {
                    own.iter().flat_map(|run| run_wires.get(run).into_iter().flatten()).filter(|w| !w.ground()).map(|w| w.size.clone()).collect()
                };
                let smallest = pick.iter().filter_map(|s| size_rank(s, &rl.vdrop.sizes).map(|k| (k, s.clone()))).min_by_key(|x| x.0);
                let brk = breaker(cx.sp, &gtag, number).or_else(|| breaker(cx.sp, panel, number));
                let volts = panel_volts(cx.sp, &gtag).or_else(|| panel_volts(cx.sp, panel));
                let (texts, _) = drawing_table(cx.d);
                let key = format!("{label}|{gtag}");
                let base = json!({"check": "vdrop", "circuit": label, "panel": gtag, "length_ft": r1(ft), "farthest": dv.handle.hex(), "conductor": smallest.as_ref().map(|s| format!("#{}", s.1))});
                let Some((rank_have, size)) = smallest else {
                    push(
                        VDROP,
                        r,
                        Some(dv.handle),
                        &dv.kind,
                        &key,
                        "unsure",
                        format!("circuit {label}: its conductor size isn't known"),
                        "the conductor size its length calls for".into(),
                        "no conductor size".into(),
                        base,
                        &dv.level,
                        place,
                        "prescreen",
                    );
                    continue;
                };
                // The legend's table wins.
                if !texts.0.is_empty() {
                    let need = texts.0.iter().find(|(hi, _)| ft <= *hi + 1e-9).map(|(_, s)| s.clone());
                    let mut m = base.clone();
                    m["table"] = json!(texts.1);
                    match need.as_ref().and_then(|n| size_rank(n, &rl.vdrop.sizes).map(|k| (k, n.clone()))) {
                        Some((rank_need, n)) => {
                            let ok = rank_have >= rank_need;
                            m["needed"] = json!(format!("#{n}"));
                            push(
                                VDROP,
                                r,
                                Some(dv.handle),
                                &dv.kind,
                                &key,
                                if ok { "pass" } else { "fail" },
                                if ok {
                                    format!("circuit {label}: {:.0} ft on #{size}, the drawing's table asks #{n}", ft)
                                } else {
                                    format!("circuit {label}: {:.0} ft on #{size}; the drawing's length table asks #{n}", ft)
                                },
                                format!("#{n} or larger for {:.0} ft (the drawing's table: {})", ft, texts.1),
                                format!("#{size}"),
                                m,
                                &dv.level,
                                place,
                                "prescreen",
                            );
                        }
                        None => push(
                            VDROP,
                            r,
                            Some(dv.handle),
                            &dv.kind,
                            &key,
                            "unsure",
                            format!("circuit {label}: {:.0} ft is longer than the drawing's length table reaches", ft),
                            format!("a size from the drawing's table ({})", texts.1),
                            format!("#{size}"),
                            m,
                            &dv.level,
                            place,
                            "prescreen",
                        ),
                    }
                    continue;
                }
                let Some((amps_brk, poles, load, src)) = brk else {
                    push(
                        VDROP,
                        r,
                        Some(dv.handle),
                        &dv.kind,
                        &key,
                        "unsure",
                        format!("circuit {label}: its breaker and load aren't in a panel schedule"),
                        format!("at most {}% drop ({})", rl.vdrop.max_pct, rl.vdrop.source),
                        format!("{:.0} ft on #{size}", ft),
                        base,
                        &dv.level,
                        place,
                        "prescreen",
                    );
                    continue;
                };
                let (v, mult) = match (poles, volts) {
                    (1, Some((ln, _))) => (ln, 2.0),
                    (1, None) => (rl.vdrop.voltage, 2.0),
                    (2, Some((_, ll))) => (ll, 2.0),
                    (_, Some((_, ll))) => (ll, 3f64.sqrt()),
                    (_, None) => (0.0, 0.0),
                };
                let cmil = rl.vdrop.cmil.get(&size).copied();
                let (amps, load_from) = match load {
                    Some(va) if v > 0.0 => (va / v, format!("{src}: {va:.0} VA")),
                    _ => (
                        amps_brk * rl.vdrop.load_pct / 100.0,
                        format!("{}% of its {amps_brk} A breaker (rule; the schedule gives no load)", rl.vdrop.load_pct),
                    ),
                };
                let (Some(cmil), true) = (cmil, v > 0.0) else {
                    push(
                        VDROP,
                        r,
                        Some(dv.handle),
                        &dv.kind,
                        &key,
                        "unsure",
                        format!("circuit {label}: no circular-mil area for #{size} or no voltage for a {poles}-pole circuit"),
                        format!("at most {}%", rl.vdrop.max_pct),
                        format!("{:.0} ft on #{size}", ft),
                        base,
                        &dv.level,
                        place,
                        "prescreen",
                    );
                    continue;
                };
                let vd = mult * rl.vdrop.k_copper * amps * ft / cmil;
                let pct = vd / v * 100.0;
                let ok = pct <= rl.vdrop.max_pct + 1e-9;
                let mut m = base;
                m["volts"] = json!(v);
                m["amps"] = json!(r2(amps));
                m["load_from"] = json!(load_from);
                m["drop_pct"] = json!(r2(pct));
                // A failure on the rule's share of the breaker (no load given) is not known.
                let verdict = if ok {
                    "pass"
                } else if load.is_some() {
                    "fail"
                } else {
                    "unsure"
                };
                push(
                    VDROP,
                    r,
                    Some(dv.handle),
                    &dv.kind,
                    &key,
                    verdict,
                    if ok {
                        format!("circuit {label}: {:.0} ft on #{size} drops {:.2}%", ft, pct)
                    } else {
                        format!("circuit {label}: {:.0} ft on #{size} drops {:.2}%, over {}%", ft, pct, rl.vdrop.max_pct)
                    },
                    format!("at most {}% drop at {} V ({})", rl.vdrop.max_pct, v, rl.vdrop.source),
                    format!("{:.2}% at {:.1} A ({load_from})", pct, amps),
                    m,
                    &dv.level,
                    place,
                    "prescreen",
                );
            }
        }
    }

    // ---- E-BOX-FILL: each box's conductors against its volume.
    if let Some(r) = on.box_fill {
        let touch = tol(r, "touch_in", DEFAULT_TOUCH_IN) / 12.0 * upf;
        let bf = &rl.box_fill;
        // The runs whose ends touch each box.
        let mut box_runs: HashMap<usize, BTreeSet<&str>> = HashMap::new();
        for s in net.sticks.iter().filter(|s| !s.run.is_empty()) {
            for p in [s.a, s.b] {
                for bi in boxes_near.near(p, touch, &net) {
                    box_runs.entry(bi).or_default().insert(s.run.as_str());
                }
            }
        }
        let cables: HashMap<Handle, &Cable> = net.cables.iter().map(|c| (c.handle, c)).collect();
        // A cable lands in the boxes either side's links name.
        let mut cable_in: HashMap<Handle, BTreeSet<Handle>> = HashMap::new();
        for c in &net.cables {
            for h in &c.links {
                cable_in.entry(*h).or_default().insert(c.handle);
            }
        }
        let devs: HashMap<Handle, &Dev> = net.devs.iter().map(|d| (d.handle, d)).collect();
        let vol = |s: &str| bf.cu_in.get(s).copied();
        for (bi, b) in net.boxes.iter().enumerate() {
            let c = (b.lo + b.hi) * 0.5;
            let Some(place) = in_scope(&b.level, c) else { continue };
            let runs = box_runs.get(&bi);
            // Directly, or through the connector at its end.
            let mut in_set: BTreeSet<Handle> = b.links.iter().copied().filter(|h| cables.contains_key(h)).collect();
            for h in b.links.iter().filter(|h| net.what.get(h).is_some_and(|k| k == "connector")) {
                in_set.extend(net.links.get(h).into_iter().flatten().copied().filter(|x| cables.contains_key(x)));
            }
            in_set.extend(cable_in.get(&b.handle).into_iter().flatten().copied());
            let in_cables: Vec<&Cable> = in_set.iter().filter_map(|h| cables.get(h).copied()).collect();
            // Each conductor entering: (size, ground).
            let mut conds: Vec<(String, bool)> = Vec::new();
            for run in runs.into_iter().flatten() {
                for w in run_wires.get(run).into_iter().flatten() {
                    conds.push((w.size.clone(), w.ground()));
                }
            }
            let mut unknown_cable = 0;
            for cb in &in_cables {
                match cb.conductors() {
                    Some((n, size, g)) => {
                        conds.extend(std::iter::repeat_n((size.clone(), false), n as usize));
                        if g {
                            conds.push((size, true));
                        }
                    }
                    None => unknown_cable += 1,
                }
            }
            if conds.is_empty() && unknown_cable == 0 {
                out.unmeasured(BOX_FILL, "a box no raceway or cable of the model enters", 1);
                continue;
            }
            let dev = b.of.and_then(|h| devs.get(&h).copied());
            let yoke = dev.is_some_and(|d| YOKE_KINDS.iter().any(|k| d.kind.contains(k)));
            let hots: Vec<&String> = conds.iter().filter(|c| !c.1).map(|c| &c.0).collect();
            let grounds: Vec<&String> = conds.iter().filter(|c| c.1).map(|c| &c.0).collect();
            let bigger = |a: &&String, b: &&String| vol(a).unwrap_or(0.0).total_cmp(&vol(b).unwrap_or(0.0));
            let largest_hot = hots.iter().copied().max_by(|a, b| bigger(a, b));
            let largest_g = grounds.iter().copied().max_by(|a, b| bigger(a, b));
            let known = unknown_cable == 0 && conds.iter().all(|c| vol(&c.0).is_some());
            let need: f64 = hots.iter().filter_map(|s| vol(s)).sum::<f64>()
                + largest_g.and_then(|s| vol(s)).map_or(0.0, |v| v * bf.grounds)
                + if yoke { largest_hot.and_then(|s| vol(s)).map_or(0.0, |v| v * bf.yoke) } else { 0.0 };
            let what = match dev {
                Some(d) => format!("{} box", d.kind),
                None => "junction box".to_string(),
            };
            let list = {
                let mut by: BTreeMap<(bool, &str), u32> = BTreeMap::new();
                for c in &conds {
                    *by.entry((c.1, c.0.as_str())).or_default() += 1;
                }
                by.iter().map(|((g, s), n)| format!("{n}#{s}{}", if *g { "G" } else { "" })).collect::<Vec<_>>().join(" + ")
            };
            let m = json!({
                "check": "box_fill", "box": b.handle.hex(), "size": b.size, "device": b.of.map(|h| h.hex()), "cubic_in": b.cubic_in,
                "conductors": list, "yoke": yoke, "needed_cu_in": r2(need), "runs": runs.map_or(0, BTreeSet::len), "cables": in_cables.len(),
            });
            let expected = format!("the box holds what enters it ({})", bf.source);
            match (b.cubic_in, known) {
                (Some(cu), true) => {
                    let ok = need <= cu + 1e-9;
                    push(
                        BOX_FILL,
                        r,
                        Some(b.handle),
                        "box",
                        &b.handle.hex(),
                        if ok { "pass" } else { "fail" },
                        if ok {
                            format!("{what}: {} cu in needed, {cu} cu in", r2(need))
                        } else {
                            format!("{what} is overfilled: {} cu in of conductors and yoke in a {cu} cu in box", r2(need))
                        },
                        format!("at most {cu} cu in ({})", b.size),
                        format!("{list}{}: {} cu in", if yoke { " + a yoke" } else { "" }, r2(need)),
                        m,
                        &b.level,
                        place,
                        "prescreen",
                    );
                }
                (None, _) => push(
                    BOX_FILL,
                    r,
                    Some(b.handle),
                    "box",
                    &b.handle.hex(),
                    "unsure",
                    format!("{what}: its volume isn't recorded"),
                    expected,
                    list,
                    m,
                    &b.level,
                    place,
                    "prescreen",
                ),
                (Some(_), false) => push(
                    BOX_FILL,
                    r,
                    Some(b.handle),
                    "box",
                    &b.handle.hex(),
                    "unsure",
                    format!("{what}: a conductor whose size the volume table doesn't list"),
                    expected,
                    list,
                    m,
                    &b.level,
                    place,
                    "prescreen",
                ),
            }
        }
    }

    // ---- G-CLASH: each stick against what it keeps clear of; the router's items carried in.
    if let Some(r) = on.clash {
        let cl = |cat: &str| -> f64 {
            let c = cat.to_ascii_lowercase();
            let key = match c.as_str() {
                "duct" | "pipe" | "fitting" | "terminal" | "equipment" | "conduit" => c.as_str(),
                "beam" | "column" | "joist" | "truss" | "structure" => "structure",
                "working space" => "working space",
                _ => "other",
            };
            if key == "working space" {
                return 0.0;
            }
            rl.clearance_in.get(key).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.5)
        };
        // The obstacles and the gear's working space, by level and plan cell.
        let cell = (CELL_FT * upf).max(1e-6);
        let mut obs: Vec<Obstacle> = net.obstacles.clone();
        let code = super::coderules::rules().into_iter().find(|x| x.id == "E-WORKSPACE").unwrap_or_default();
        let depth_ft = code.depth_in.first().and_then(|row| row.get(1)).copied().unwrap_or(36.0) / 12.0;
        let height_ft = code.height_in.unwrap_or(78.0) / 12.0;
        for g in net.gear.iter().filter(|g| g.electrical) {
            let floor = cx.sp.level(&g.level).map_or(g.lo.z, |l| l.elevation_ft * upf);
            let reach = depth_ft * upf;
            // Its working space before its long faces (which one is its front the model doesn't
            // say): from the floor to 6-1/2 ft or its top.
            let (wx, wy) = (g.hi.x - g.lo.x, g.hi.y - g.lo.y);
            let (lo, hi) = if wx >= wy {
                (Vec3::new(g.lo.x, g.lo.y - reach, floor), Vec3::new(g.hi.x, g.hi.y + reach, (floor + height_ft * upf).max(g.hi.z)))
            } else {
                (Vec3::new(g.lo.x - reach, g.lo.y, floor), Vec3::new(g.hi.x + reach, g.hi.y, (floor + height_ft * upf).max(g.hi.z)))
            };
            obs.push(Obstacle {
                handle: g.handle,
                level: g.level.clone(),
                category: "working space".into(),
                system: g.tag.clone(),
                trade: "electrical",
                lo,
                hi,
            });
        }
        let mut grid: HashMap<(String, i64, i64), Vec<usize>> = HashMap::new();
        for (i, o) in obs.iter().enumerate() {
            let (x0, y0, x1, y1) = (cell_of(o.lo.x, cell), cell_of(o.lo.y, cell), cell_of(o.hi.x, cell), cell_of(o.hi.y, cell));
            if x1.saturating_sub(x0).saturating_mul(y1.saturating_sub(y0)) > 40_000 {
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    grid.entry((o.level.to_ascii_lowercase(), x, y)).or_default().push(i);
                }
            }
        }
        // The devices each run serves (its ends' boxes): a run landing on equipment it serves
        // is no clash with it.
        let touch = tol(r, "touch_in", DEFAULT_TOUCH_IN) / 12.0 * upf;
        let mut run_boxes: HashMap<&str, Vec<usize>> = HashMap::new();
        for s in net.sticks.iter().filter(|s| !s.run.is_empty()) {
            for p in [s.a, s.b] {
                for bi in boxes_near.near(p, touch, &net) {
                    let v = run_boxes.entry(s.run.as_str()).or_default();
                    if !v.contains(&bi) {
                        v.push(bi);
                    }
                }
            }
        }
        let serves = |run: &str, o: &Obstacle| -> bool {
            let pad = SERVES_FT * upf;
            run_boxes.get(run).into_iter().flatten().filter_map(|bi| net.boxes.get(*bi)).any(|b| {
                b.hi.x >= o.lo.x - pad
                    && b.lo.x <= o.hi.x + pad
                    && b.hi.y >= o.lo.y - pad
                    && b.lo.y <= o.hi.y + pad
                    && b.hi.z >= o.lo.z - pad
                    && b.lo.z <= o.hi.z + pad
            })
        };
        // Per run, each obstacle it comes too near once.
        let mut hits: BTreeMap<(String, Handle), (usize, Vec3, f64, &Stick)> = BTreeMap::new();
        let mut checked_runs: BTreeSet<&str> = BTreeSet::new();
        for s in &net.sticks {
            if s.circuit.is_empty() && s.run.is_empty() {
                continue;
            }
            checked_runs.insert(s.run.as_str());
            let lv = s.level.to_ascii_lowercase();
            let (x0, x1) = (s.a.x.min(s.b.x), s.a.x.max(s.b.x));
            let (y0, y1) = (s.a.y.min(s.b.y), s.a.y.max(s.b.y));
            let mut seen: BTreeSet<usize> = BTreeSet::new();
            for x in cell_of(x0, cell)..=cell_of(x1, cell).min(cell_of(x0, cell).saturating_add(2_000)) {
                for y in cell_of(y0, cell)..=cell_of(y1, cell).min(cell_of(y0, cell).saturating_add(2_000)) {
                    for &oi in grid.get(&(lv.clone(), x, y)).map_or(&[][..], Vec::as_slice) {
                        if !seen.insert(oi) {
                            continue;
                        }
                        let Some(o) = obs.get(oi) else { continue };
                        if o.category == "working space" {
                            // Its own panel's raceways come into it; a drop in a wall's cavity
                            // is behind its face.
                            if is_panel(&o.system, &s.panel) || s.vertical {
                                continue;
                            }
                        } else if o.trade == "electrical"
                            && o.category.eq_ignore_ascii_case("equipment")
                            && net.gear.iter().any(|g| g.handle == o.handle && is_panel(&g.tag, &s.panel))
                        {
                            continue;
                        }
                        let pad = s.r + cl(&o.category) / 12.0 * upf;
                        let (lo, hi) = (o.lo - Vec3::new(pad, pad, pad), o.hi + Vec3::new(pad, pad, pad));
                        if !s.path.windows(2).any(|w| matches!(w, [p, q] if seg_hits_box(*p, *q, lo, hi))) || serves(&s.run, o) {
                            continue;
                        }
                        let mid = Vec3::new(
                            ((s.a.x + s.b.x) / 2.0).clamp(o.lo.x, o.hi.x),
                            ((s.a.y + s.b.y) / 2.0).clamp(o.lo.y, o.hi.y),
                            ((s.a.z + s.b.z) / 2.0).clamp(lo.z, hi.z),
                        );
                        hits.entry((s.run.clone(), o.handle)).or_insert((oi, mid, pad, s));
                    }
                }
            }
        }
        // Other runs' sticks: crossing within the clearance, away from where they meet.
        let cc = cl("conduit") / 12.0 * upf;
        let mut sgrid: HashMap<(String, i64, i64), Vec<usize>> = HashMap::new();
        for (i, s) in net.sticks.iter().enumerate().filter(|(_, s)| !s.run.is_empty()) {
            let c = (s.a + s.b) * 0.5;
            sgrid.entry((s.level.to_ascii_lowercase(), cell_of(c.x, cell), cell_of(c.y, cell))).or_default().push(i);
        }
        let mut pairs: BTreeSet<(String, String)> = BTreeSet::new();
        let mut cross: Vec<(usize, usize, Vec3, f64, bool)> = Vec::new();
        for (i, s) in net.sticks.iter().enumerate().filter(|(_, s)| !s.run.is_empty()) {
            if (s.b - s.a).len() > 4.0 * cell {
                continue;
            }
            let c = (s.a + s.b) * 0.5;
            let (kx, ky) = (cell_of(c.x, cell), cell_of(c.y, cell));
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for &j in sgrid.get(&(s.level.to_ascii_lowercase(), kx.saturating_add(dx), ky.saturating_add(dy))).map_or(&[][..], Vec::as_slice)
                    {
                        let Some(t) = net.sticks.get(j) else { continue };
                        if j <= i || t.run == s.run {
                            continue;
                        }
                        let reach = s.r + t.r + cc;
                        let meet = [(s.a, t.a), (s.a, t.b), (s.b, t.a), (s.b, t.b)].iter().any(|(p, q)| (*p - *q).len() <= reach + touch);
                        if meet {
                            continue;
                        }
                        let (dd, p, along) = s
                            .path
                            .windows(2)
                            .flat_map(|w| t.path.windows(2).map(move |v| (w, v)))
                            .filter_map(|(w, v)| match (w, v) {
                                ([a, b], [c, d]) => {
                                    let (dd, p) = seg_seg(*a, *b, *c, *d);
                                    let (u, v) = (*b - *a, *d - *c);
                                    let cos = (u.dot(v) / (u.len() * v.len()).max(1e-12)).abs();
                                    Some((dd, p, cos > 0.9))
                                }
                                _ => None,
                            })
                            .min_by(|x, y| x.0.total_cmp(&y.0))
                            .unwrap_or((f64::INFINITY, s.a, false));
                        let near_end = [s.a, s.b, t.a, t.b].iter().any(|e| (*e - p).len() <= reach);
                        if dd < reach - 1e-9 && !near_end {
                            let k = if s.run < t.run { (s.run.clone(), t.run.clone()) } else { (t.run.clone(), s.run.clone()) };
                            if pairs.insert(k) {
                                cross.push((i, j, p, (dd - s.r - t.r) / upf * 12.0, along));
                            }
                        }
                    }
                }
            }
        }
        let report = super::conduitroute::stored_report(cx.d);
        let carried: Vec<Value> = report.as_ref().and_then(|x| x["clashes"].as_array().cloned()).unwrap_or_default();
        let mut known: BTreeSet<(String, String)> = BTreeSet::new();
        for c in &carried {
            if let (Some(b), Some(circ)) = (c["b"]["handle"].as_str(), c["a"]["circuit"].as_str()) {
                known.insert((b.to_ascii_uppercase(), circ.to_string()));
            }
        }
        let mut clashed: BTreeSet<&str> = BTreeSet::new();
        for ((run, oh), (oi, at, pad, s)) in &hits {
            let Some(o) = obs.get(*oi) else { continue };
            let Some(place) = in_scope(&s.level, *at) else { continue };
            clashed.insert(run.as_str());
            let need_in = r2((pad - s.r) / upf * 12.0);
            let floor = cx.sp.level(&s.level).map_or(0.0, |l| l.elevation_ft * upf);
            let by_router = known.contains(&(oh.hex().to_ascii_uppercase(), s.circuit.clone()));
            push(
                CLASH,
                r,
                Some(s.handle),
                "conduit",
                &format!("{run}|{}", oh.hex()),
                "fail",
                format!("conduit {run} ({}) comes within {need_in} in of {} {} {}", s.circuit, o.trade, o.category, o.system).trim().to_string(),
                format!(
                    "{need_in} in clear of the {} ({})",
                    o.category,
                    if o.category == "working space" {
                        "NEC 110.26(A)".to_string()
                    } else {
                        super::conduitroute::rules().clearance_in.get("source").and_then(Value::as_str).unwrap_or_default().to_string()
                    }
                ),
                format!("through or within the clearance at {:.1} ft above the floor", (at.z - floor) / upf),
                json!({"check": "clash", "run": run, "circuit": s.circuit, "b": {"handle": oh.hex(), "category": o.category, "system": o.system, "trade": o.trade}, "clearance_in": need_in, "z_ft": r2((at.z - floor) / upf), "also_router_item": by_router}),
                &s.level,
                place,
                "prescreen",
            );
        }
        for (i, j, p, gap_in, along) in &cross {
            let (Some(s), Some(t)) = (net.sticks.get(*i), net.sticks.get(*j)) else { continue };
            let Some(place) = in_scope(&s.level, *p) else { continue };
            clashed.insert(s.run.as_str());
            push(
                CLASH,
                r,
                Some(s.handle),
                "conduit",
                &format!("{}|{}", s.run, t.run),
                "fail",
                format!(
                    "conduit {} ({}) {} conduit {} ({}) {:.1} in apart (surface to surface)",
                    s.run,
                    s.circuit,
                    if *along { "runs alongside" } else { "crosses" },
                    t.run,
                    t.circuit,
                    gap_in
                ),
                format!("{} in clear of other conduit", cl("conduit")),
                format!("{:.1} in clear, {}, away from any box", gap_in, if *along { "side by side" } else { "crossing" }),
                json!({"check": "clash", "run": s.run, "circuit": s.circuit, "gap_in": r2(*gap_in), "alongside": along, "z_ft": r2((p.z - cx.sp.level(&s.level).map_or(0.0, |l| l.elevation_ft * upf)) / upf), "b": {"handle": t.handle.hex(), "category": "conduit", "run": t.run, "circuit": t.circuit, "trade": "electrical"}}),
                &s.level,
                place,
                "prescreen",
            );
        }
        // Each run with no clash: a pass.
        for (run, ids) in &run_sticks {
            if clashed.contains(run) {
                continue;
            }
            let Some((lead, mid)) = run_place(ids) else { continue };
            let Some(place) = in_scope(&lead.level, mid) else { continue };
            push(
                CLASH,
                r,
                Some(lead.handle),
                "conduit",
                run,
                "pass",
                format!("conduit {run} keeps clear of the other trades, the structure, the gear's working space and other conduit"),
                "the clearances of the rules data".into(),
                format!("{} stick(s), none too near", ids.len()),
                json!({"check": "clash", "run": run, "circuit": lead.circuit, "sticks": ids.len()}),
                &lead.level,
                place,
                "prescreen",
            );
        }
        // The router's own items, carried in.
        for c in &carried {
            let level = c["level"].as_str().unwrap_or_default().to_string();
            let at = c["at"]
                .as_array()
                .map(|a| (a.first().and_then(Value::as_f64).unwrap_or(f64::NAN), a.get(1).and_then(Value::as_f64).unwrap_or(f64::NAN)));
            let Some((x, y)) = at.filter(|(x, y)| x.is_finite() && y.is_finite()) else {
                out.unmeasured(CLASH, "a router clash item with no point", 1);
                continue;
            };
            let Some(place) = in_scope(&level, Vec3::new(x, y, 0.0)) else { continue };
            let what = c["a"]["what"].as_str().unwrap_or("conduit");
            push(
                CLASH,
                r,
                c["b"]["handle"].as_str().and_then(Handle::parse_hex),
                c["b"]["category"].as_str().unwrap_or("conduit"),
                &format!(
                    "router|{}|{}|{}|{x:.1},{y:.1}",
                    what,
                    c["a"]["circuit"].as_str().unwrap_or_default(),
                    c["b"]["handle"].as_str().unwrap_or_default()
                ),
                "fail",
                format!(
                    "the router's clash: {} {} × {} {} ({})",
                    what,
                    c["a"]["circuit"].as_str().unwrap_or_default(),
                    c["b"]["category"].as_str().unwrap_or_default(),
                    c["b"]["system"].as_str().unwrap_or_default(),
                    c["pair"].as_str().unwrap_or_default()
                )
                .trim()
                .to_string(),
                format!("{} in clear", c["clearance_in"]),
                c["fix"].as_str().unwrap_or_default().to_string(),
                json!({"check": "clash", "router_item": c}),
                &level,
                place,
                "build",
            );
        }
    }

    // ---- E-TAKEOFF: the index and the takeoff against a recount.
    if let Some(r) = on.takeoff {
        let pct = tol(r, "takeoff_pct", DEFAULT_TAKEOFF_PCT);
        let levels: BTreeSet<String> = cx.areas.iter().map(|a| a.level.to_ascii_lowercase()).collect();
        let by_ft = |k: &str| matches!(k, "conduit" | "cable" | "wire");
        type K = (String, String, String);
        let mut recount: BTreeMap<K, f64> = BTreeMap::new();
        for c in net.counted.iter().filter(|c| levels.contains(&c.level.to_ascii_lowercase()) && super::mepatoms::PIECES.contains(&c.kind.as_str())) {
            *recount.entry((c.kind.clone(), c.item.clone(), c.size.clone())).or_default() += c.qty;
        }
        let mut index: BTreeMap<K, f64> = BTreeMap::new();
        let stored = super::mepatoms::stored(cx.d);
        if let Some(ix) = &stored {
            for (k, t) in ix.rows.iter().filter(|(k, _)| {
                k.trade.is_empty() && super::mepatoms::PIECES.contains(&k.kind.as_str()) && levels.contains(&k.level.to_ascii_lowercase())
            }) {
                *index.entry((k.kind.clone(), k.item.clone(), k.size.clone())).or_default() += if by_ft(&k.kind) { t.length_ft } else { t.pieces() };
            }
        }
        let mut lines: BTreeMap<K, f64> = BTreeMap::new();
        let mut has_lines = false;
        if let Some(t) = cx.takeoff {
            has_lines = t["lines"].is_array();
            for l in t["lines"].as_array().into_iter().flatten() {
                let g = |k: &str| l[k].as_str().unwrap_or_default().to_string();
                if !levels.contains(&g("level").to_ascii_lowercase()) || !super::mepatoms::PIECES.contains(&g("kind").as_str()) {
                    continue;
                }
                *lines.entry((g("kind"), g("item"), g("size"))).or_default() += l["qty"].as_f64().unwrap_or(0.0);
            }
            if t["line_count"].as_u64().is_some_and(|n| n as usize > t["lines"].as_array().map_or(0, Vec::len)) {
                out.unmeasured(TAKEOFF, "conduit.takeoff lines past the 5,000 it returns", 1);
            }
        }
        if recount.is_empty() && index.is_empty() {
            out.unmeasured(TAKEOFF, "no raceway pieces in the model", 1);
        }
        let level_name = cx.areas.first().map(|a| a.level.clone()).unwrap_or_default();
        let place = (None, [f64::NAN, f64::NAN]);
        let keys: BTreeSet<K> = recount.keys().chain(index.keys()).chain(lines.keys()).cloned().collect();
        for k in keys {
            let want = recount.get(&k).copied().unwrap_or(0.0);
            let unit = if by_ft(&k.0) { "ft" } else { "ea" };
            let close = |have: f64| (have - want).abs() <= (want.abs() * pct / 100.0).max(if unit == "ft" { 0.05 } else { 1e-9 });
            let mut sides: Vec<(&str, Option<f64>)> =
                vec![("index", if stored.is_some() { Some(index.get(&k).copied().unwrap_or(0.0)) } else { None })];
            if has_lines {
                sides.push(("conduit.takeoff", Some(lines.get(&k).copied().unwrap_or(0.0))));
            }
            let label = format!("{} {} {}", k.0, k.1, k.2).split_whitespace().collect::<Vec<_>>().join(" ");
            let bad: Vec<String> = sides.iter().filter_map(|(n, v)| v.filter(|v| !close(*v)).map(|v| format!("{n} {} {unit}", r2(v)))).collect();
            let known = sides.iter().any(|(_, v)| v.is_some());
            let verdict = if !known {
                "unsure"
            } else if bad.is_empty() {
                "pass"
            } else {
                "fail"
            };
            let m = json!({
                "check": "takeoff", "kind": k.0, "item": k.1, "size": k.2, "unit": unit, "recount": r2(want),
                "index": sides.iter().find(|s| s.0 == "index").and_then(|s| s.1).map(r2),
                "takeoff": sides.iter().find(|s| s.0 == "conduit.takeoff").and_then(|s| s.1).map(r2),
                "tolerance_pct": pct,
            });
            push(
                TAKEOFF,
                r,
                None,
                &k.0,
                &label,
                verdict,
                match verdict {
                    "pass" => format!("{label}: the takeoff matches a recount of the model ({} {unit})", r2(want)),
                    "fail" => format!("{label}: the takeoff is off a recount of the model ({} {unit}): {}", r2(want), bad.join(", ")),
                    _ => format!("{label}: no stored index or takeoff to compare"),
                },
                format!("{} {unit} (a recount of the model's pieces), within {pct}%", r2(want)),
                sides
                    .iter()
                    .map(|(n, v)| format!("{n}: {}", v.map_or_else(|| "none".to_string(), |v| format!("{} {unit}", r2(v)))))
                    .collect::<Vec<_>>()
                    .join("; "),
                m,
                &level_name,
                place,
                "prescreen",
            );
        }
    }
    out.findings = got;
    out
}

/// The drawing's length table (bands, what it read), once.
fn drawing_table(d: &Drawing) -> ((Vec<(f64, String)>, String), ()) {
    thread_local! {
        static CACHE: std::cell::RefCell<Option<(usize, (Vec<(f64, String)>, String))>> = const { std::cell::RefCell::new(None) };
    }
    let key = std::ptr::from_ref(d) as usize ^ d.model.len();
    if let Some(hit) = CACHE.with(|c| c.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, v)| v.clone())) {
        return (hit, ());
    }
    let texts: Vec<String> = d
        .model
        .iter()
        .take(400_000)
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::Text(t) => Some(t.value.clone()),
            cadaddy_doc::EntityKind::MText(t) => Some(t.contents.clone()),
            _ => None,
        })
        .collect();
    let got = super::conduitroute::length_table(&texts);
    CACHE.with(|c| *c.borrow_mut() = Some((key, got.clone())));
    (got, ())
}

/// Is a box (or device) linked to a panel through its pieces' links (a cable run)?
fn linked_to_panel(net: &Net, from: Handle, panel: &str) -> bool {
    let gear: HashMap<Handle, &str> = net.gear.iter().map(|g| (g.handle, g.tag.as_str())).collect();
    let mut seen: BTreeSet<Handle> = BTreeSet::new();
    let mut todo = vec![from];
    while let Some(h) = todo.pop() {
        if seen.len() > 20_000 || !seen.insert(h) {
            continue;
        }
        if gear.get(&h).is_some_and(|t| is_panel(t, panel)) {
            return true;
        }
        if gear.contains_key(&h) {
            continue;
        }
        for x in net.links.get(&h).into_iter().flatten() {
            if net.what.get(x).is_some_and(|k| k != "device") || gear.contains_key(x) {
                todo.push(*x);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_sizes_and_conductor_sizes_read() {
        assert_eq!(trade_in("3/4\""), Some(0.75));
        assert_eq!(trade_in("1-1/4\""), Some(1.25));
        assert_eq!(trade_in("2\""), Some(2.0));
        assert_eq!(trade_in("junk"), None);
        assert_eq!(awg("#12"), "12");
        assert_eq!(awg("#3/0"), "3/0");
        assert_eq!(awg("250 kcmil"), "250");
    }

    #[test]
    fn a_segment_through_a_box_and_two_crossing_segments() {
        let (lo, hi) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 10.0));
        assert!(seg_hits_box(Vec3::new(-5.0, 5.0, 5.0), Vec3::new(15.0, 5.0, 5.0), lo, hi));
        assert!(!seg_hits_box(Vec3::new(-5.0, 15.0, 5.0), Vec3::new(15.0, 15.0, 5.0), lo, hi));
        let (d, _) = seg_seg(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(5.0, -5.0, 1.0), Vec3::new(5.0, 5.0, 1.0));
        assert!((d - 1.0).abs() < 1e-9, "{d}");
    }

    #[test]
    fn a_cable_type_gives_its_conductors() {
        let c = Cable {
            handle: Handle(1),
            run: String::new(),
            circuit: String::new(),
            level: String::new(),
            ty: "NM 12/2 w/G".into(),
            room: String::new(),
            links: Vec::new(),
            at: None,
        };
        assert_eq!(c.conductors(), Some((2, "12".into(), true)));
        let c = Cable { ty: "MC 10/3".into(), ..c };
        assert_eq!(c.conductors(), Some((3, "10".into(), false)));
    }

    #[test]
    fn the_panel_of_a_typical_room_matches() {
        assert!(is_panel("LP1", "lp1"));
        assert!(is_panel("Q-101", "Q"));
        assert!(!is_panel("QA", "Q"));
    }
}
