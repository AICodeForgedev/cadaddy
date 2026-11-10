//! THE PACKAGE SET (`bim.export.ar {set: true, dir}`, Q6420): the whole building for the field
//! app as the four views' packages, never one package with everything in it.
//!
//! - **building-architectural:** rooms (as floor tiles), walls, windows, doors, floors, roof
//!   and stairs, solid, plus each level's solid exterior for the app's opening reveal. No MEP,
//!   framing, cabinets or furniture.
//! - **building-mep-mains** (the default view): the floors and one outline around the outside of
//!   the building as the only context, with mains and big gear: electrical gear, house panels and
//!   their feeders, cable tray, main ducts, large pipe and risers, fire mains and risers, pumps,
//!   large mechanical equipment, and each elevator as one hoistway volume with its machine,
//!   disconnect and feeder.
//! - **building-mep-full** (loaded on top of the mains): every other duct, air terminal and
//!   exhaust fan, plumbing mains down to the rules' limit, every panel with its feeder, the feeds
//!   to large equipment, and anything that passes through a floor (by its geometry against the
//!   level elevations), whatever its size, except the branches inside or serving an elevator.
//!   Never branch circuits, boxes, devices, fixtures, sprinkler heads or sprinkler branch lines.
//! - **levels/LEVEL:** the Floor view: the architecture as context with walls cut at the rules'
//!   height, a tile per room (its own node, number and name), room and unit panels, lights,
//!   cabinets and the level's MEP with its sprinkler heads and branch lines. Devices, boxes,
//!   branch wiring and plumbing fixtures standing in a room are in that room's package instead.
//! - **rooms/ROOM:** the Room view: everything in the room at true position from its anchor,
//!   ready for field alignment (the single-room export); furniture only here, off by default.
//! - **building.json:** the index: levels, rooms, big equipment, main systems by trade, every
//!   package with its files' sizes and SHA-256 (and a content hash of its models, so Update
//!   pulls only what changed), the four views and the default (MEP, mains), and the connection
//!   graph: each element's system, what feeds it and what it serves, known or inferred, by
//!   element id across packages. A link the documents don't give is left unknown.
//!
//! Every node carries its menu group (`mechanical`, `electrical`, `plumbing`, `cabinets`,
//! `furniture` or `base`). The building packages are generic (simplified on purpose, marked so
//! in their sidecars): the app never offers field alignment on them. What goes where is rules
//! data (`set` in arexport.json, a job's own under `ar_export.set`), never job constants.
//!
//! Read-only: the drawing is not changed. Files go through the output guard; a folder holding
//! an earlier set's `building.json` is the set's own to replace.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use cadaddy_doc::Drawing;
use cadaddy_geom::Vec2;
use cadaddy_walk3d::BuildingSpec;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::*;

/// The index's schema name and its file name in the folder.
pub const INDEX_SCHEMA: &str = "cadaddy.ar.building/1";
pub const INDEX_FILE: &str = "building.json";
/// Most rooms and levels a set writes a package for.
const MAX_SET_ROOMS: usize = 5_000;
const MAX_SET_LEVELS: usize = 200;
/// Most elements a set reads, and graph entries a package's links list per element.
const MAX_SET_ELEMENTS: usize = 2_000_000;
const MAX_LINKS: usize = 200;

// ─── Rules ───────────────────────────────────────────────────────────────────

/// What goes in which package (`set` in arexport.json).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SetRules {
    /// Caps of each building package (a room package keeps the export's own caps).
    pub max_triangles: u64,
    pub max_bytes: u64,
    /// The Architectural view's parts; those of them drawn as their box there (simplified).
    pub architectural_parts: Vec<String>,
    pub box_parts: Vec<String>,
    /// The MEP views' only context (with the generated outline).
    pub mep_context_parts: Vec<String>,
    /// A level package cuts these parts this high above its floor (a plan's cut).
    pub level_cut_ft: f64,
    pub cut_parts: Vec<String>,
    /// Parts over a floor plan (ceilings, roof): in the level package, off by default.
    pub overhead_parts: Vec<String>,
    /// The menu groups, the first rule that matches winning.
    pub groups: Vec<GroupRule>,
    pub equipment: EquipmentRules,
    pub sizes: SizeRules,
    pub elevator: ElevatorRules,
    pub crossing: CrossingRules,
    /// The MEP views' outline: its bar, inches across and high.
    pub outline_bar_in: f64,
}

/// A menu group: by words of the element (its kind, part, category, type, tag, layer) for the
/// classes listed (any when empty), else by trade.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct GroupRule {
    pub group: String,
    pub words: Vec<String>,
    pub classes: Vec<String>,
    pub trades: Vec<String>,
    /// Shown when a package opens (furniture: off).
    pub default_on: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct EquipmentRules {
    /// Electrical gear: always in the mains.
    pub gear_words: Vec<String>,
    pub panel_words: Vec<String>,
    pub disconnect_words: Vec<String>,
    /// Large equipment by what it is, or by its plan size.
    pub big_words: Vec<String>,
    pub min_extent_ft: f64,
    pub fan_words: Vec<String>,
    /// Equipment that is a device to the views (a junction box, a contactor).
    pub device_words: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SizeRules {
    /// A duct this wide or wider is a main.
    pub main_duct_in: f64,
    /// Plumbing pipe this size or larger is a main (the mains view).
    pub main_pipe_in: f64,
    /// Plumbing pipe under this size is left out of the full view (fixture connections, traps).
    pub plumbing_min_in: f64,
    /// Fire protection pipe of unknown role this size or larger is a main.
    pub fire_main_in: f64,
    /// A raceway or cable of unknown connection this size or larger is a feeder.
    pub feeder_min_in: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ElevatorRules {
    pub words: Vec<String>,
    pub machine_words: Vec<String>,
    /// A branch circuit is the elevator's when its circuit says so.
    pub branch_words: Vec<String>,
    /// A disconnect this close to a hoistway is the elevator's.
    pub reach_ft: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct CrossingRules {
    pub kinds: Vec<String>,
    pub tolerance_ft: f64,
}

impl SetRules {
    fn tri_cap(&self) -> u64 {
        if self.max_triangles == 0 { 150_000 } else { self.max_triangles }
    }
    fn cut_ft(&self) -> f64 {
        Some(self.level_cut_ft).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(4.0).min(50.0)
    }
    fn tolerance_ft(&self) -> f64 {
        Some(self.crossing.tolerance_ft).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.25).min(5.0)
    }
    fn bar_m(&self) -> f64 {
        Some(self.outline_bar_in).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(2.0).min(24.0) * 0.0254
    }
    fn reach_ft(&self) -> f64 {
        Some(self.elevator.reach_ft).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(15.0).min(200.0)
    }
}

// ─── Words ───────────────────────────────────────────────────────────────────

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).take(200).collect()
}

/// Whether one of `words` (each one or more words) appears, whole words in order, in `toks`.
fn says(toks: &[String], words: &[String]) -> bool {
    words.iter().take(500).any(|w| {
        let want = tokens(w);
        !want.is_empty() && toks.windows(want.len()).any(|win| win.iter().zip(&want).all(|(a, b)| a == b))
    })
}

/// A size as printed, in inches: `24x12` → 24 (the larger side), `3`, `3/4"`, `1-1/4"`, `0.75`.
pub(super) fn size_in(text: &str) -> Option<f64> {
    let t = text.trim().trim_start_matches(['Ø', '⌀']).trim();
    let first = |s: &str| -> Option<f64> {
        let s = s.trim().trim_end_matches(['"', '\'']).trim_end_matches("in").trim();
        let num = |x: &str| -> Option<f64> {
            match x.split_once('/') {
                Some((a, b)) => {
                    let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
                    (b > 0.0).then(|| a / b)
                }
                None => x.trim().parse::<f64>().ok(),
            }
        };
        let v = match s.split_once('-') {
            Some((w, f)) if f.contains('/') => num(w)? + num(f)?,
            _ => num(s)?,
        };
        (v.is_finite() && v > 0.0 && v < 10_000.0).then_some(v)
    };
    let head = t.split(|c: char| c.is_whitespace() || c == '"').next().unwrap_or("");
    let sides: Vec<f64> = head.split(['x', 'X', '×']).filter_map(first).collect();
    sides.into_iter().reduce(f64::max)
}

// ─── What each element is to the views ───────────────────────────────────────

/// An element's place in the four views.
#[derive(Clone, Debug, Default)]
struct Role {
    group: String,
    default_on: bool,
    arch: bool,
    as_box: bool,
    mep_context: bool,
    overhead: bool,
    /// Why it is in the mains view, else in the full view.
    mains: Option<String>,
    full: Option<String>,
    /// Shown in a room package, not its level's, when it stands in a room.
    room_tier: bool,
    furniture: bool,
    cabinets: bool,
    /// Why the building MEP views leave it out (counted in their sidecars).
    left_out: Option<&'static str>,
    /// main, branch, riser, feeder, panel, gear, hoistway…
    role: Option<String>,
    house: Option<(&'static str, String)>,
    feeder: Option<usize>,
}

/// What a piece of equipment is to the views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kit {
    Hoistway,
    ElevatorMachine,
    Disconnect,
    Panel,
    Distribution,
    Big,
    Fan,
    Device,
    Other,
}

/// What the views need to know about the building's distribution, from the building spec.
struct Cx<'a> {
    rules: &'a SetRules,
    sp: &'a BuildingSpec,
    /// Level elevations (feet).
    floors: Vec<f64>,
    /// Equipment kinds by tag (upper case).
    kinds: HashMap<String, String>,
    /// House (`house`) or room and unit (`unit`) panels, and why, by tag.
    panels: HashMap<String, (&'static str, String)>,
    /// Elevator hoistways' plan boxes (building feet).
    hoistways: Vec<([f64; 2], [f64; 2])>,
    /// Large equipment's plan boxes.
    big: Vec<([f64; 2], [f64; 2])>,
    /// Circuit descriptions by (panel, circuit), upper case.
    circuits: HashMap<(String, String), String>,
    /// Tags that are in the mains view (gear, house panels, big equipment, elevator gear).
    mains_tags: BTreeSet<String>,
}

fn up(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

impl<'a> Cx<'a> {
    fn new(rules: &'a SetRules, sp: &'a BuildingSpec) -> Cx<'a> {
        let floors = sp.levels.iter().map(|l| l.elevation_ft).filter(|e| e.is_finite()).collect();
        let kinds: HashMap<String, String> = sp.mep.equipment.iter().take(100_000).map(|e| (up(&e.tag), e.kind.clone())).collect();
        let mut circuits = HashMap::new();
        for ps in sp.mep.panel_schedules.iter().take(10_000) {
            for c in ps.circuits.iter().take(2_000) {
                circuits.insert((up(&ps.panel), up(&c.number)), up(&c.description));
            }
        }
        for e in sp.mep.equipment.iter().take(100_000) {
            for c in e.circuits.iter().take(2_000) {
                circuits.entry((up(&e.tag), up(&c.number))).or_insert_with(|| up(&c.description));
            }
        }
        let mut cx =
            Cx { rules, sp, floors, kinds, panels: HashMap::new(), hoistways: Vec::new(), big: Vec::new(), circuits, mains_tags: BTreeSet::new() };
        cx.panels = cx.panel_scopes();
        cx
    }

    fn is_gear(&self, kind_text: &str) -> bool {
        says(&tokens(kind_text), &self.rules.equipment.gear_words)
    }

    /// House versus room and unit panels, from the distribution data: a panel whose circuits
    /// serve one room is the room's (or the unit's); one fed straight from gear, or serving more
    /// than one room or area, is a house panel; else the documents don't say.
    fn panel_scopes(&self) -> HashMap<String, (&'static str, String)> {
        let mut out = HashMap::new();
        let rules = &self.rules.equipment;
        for e in self.sp.mep.equipment.iter().take(100_000) {
            let toks = tokens(&format!("{} {}", e.kind, e.tag));
            if !says(&toks, &rules.panel_words) || self.is_gear(&e.kind) {
                continue;
            }
            let tag = up(&e.tag);
            let mut rooms: BTreeSet<String> = BTreeSet::new();
            for ps in self.sp.mep.panel_schedules.iter().take(10_000).filter(|p| up(&p.panel) == tag) {
                rooms.extend(ps.circuits.iter().take(2_000).map(|c| up(&c.room)).filter(|r| !r.is_empty()));
            }
            rooms.extend(e.circuits.iter().take(2_000).map(|c| up(&c.room)).filter(|r| !r.is_empty()));
            let fed = self.sp.mep.feeders.iter().find(|f| up(&f.to) == tag);
            let from_gear = fed.and_then(|f| self.kinds.get(&up(&f.from))).is_some_and(|k| self.is_gear(k));
            let scope = if rooms.len() == 1 {
                ("unit", format!("its circuits serve one room ({})", rooms.iter().next().map(String::as_str).unwrap_or_default()))
            } else if from_gear {
                ("house", format!("fed straight from gear ({})", fed.map(|f| f.from.as_str()).unwrap_or_default()))
            } else if rooms.len() > 1 {
                ("house", format!("its circuits serve {} rooms or areas", rooms.len()))
            } else {
                ("unknown", "the distribution data don't say what it serves".to_string())
            };
            out.insert(tag, scope);
        }
        out
    }

    /// Whether a box from `lo` to `hi` (feet above the datum) passes through a floor.
    fn crosses(&self, lo: f64, hi: f64) -> bool {
        let t = self.rules.tolerance_ft();
        self.floors.iter().any(|f| lo < f - t && hi > f + t)
    }

    fn near(boxes: &[([f64; 2], [f64; 2])], lo: [f64; 3], hi: [f64; 3], reach: f64) -> bool {
        boxes.iter().any(|(a, b)| {
            let dx = (a[0] - hi[0]).max(lo[0] - b[0]).max(0.0);
            let dy = (a[1] - hi[1]).max(lo[1] - b[1]).max(0.0);
            dx.hypot(dy) <= reach
        })
    }
}

/// The words an element is described by: its build part, kind, tag, category, type and layer.
fn element_words(it: &Item, tag: &str) -> Vec<String> {
    let mpart = made_tag(it.e, "part").unwrap_or_default();
    tokens(&format!("{} {} {} {} {} {} {}", it.part, mpart, it.kind, tag, it.class.category, it.class.ty, it.e.common.layer))
}

fn group_of(rules: &SetRules, it: &Item, toks: &[String]) -> (String, bool) {
    for g in rules.groups.iter().take(100) {
        if g.group.trim().is_empty() {
            continue;
        }
        let class_ok = g.classes.is_empty() || g.classes.iter().any(|c| c.eq_ignore_ascii_case(it.class.key));
        let by_words = !g.words.is_empty() && class_ok && says(toks, &g.words);
        let by_trade = !g.trades.is_empty() && g.trades.iter().any(|t| slug(t) == it.trade);
        if by_words || by_trade {
            return (slug(&g.group), g.default_on.unwrap_or(true));
        }
    }
    ("base".into(), true)
}

fn equipment_class(cx: &Cx, it: &Item, toks: &[String]) -> Kit {
    let r = &cx.rules.equipment;
    let el = &cx.rules.elevator;
    if says(toks, &r.disconnect_words) {
        return Kit::Disconnect;
    }
    if says(toks, &el.words) {
        return if says(toks, &el.machine_words) { Kit::ElevatorMachine } else { Kit::Hoistway };
    }
    if says(toks, &r.gear_words) {
        return Kit::Distribution;
    }
    if says(toks, &r.panel_words) {
        return Kit::Panel;
    }
    let extent = (it.hi[0] - it.lo[0]).max(it.hi[1] - it.lo[1]);
    if says(toks, &r.big_words) || (r.min_extent_ft > 0.0 && extent >= r.min_extent_ft) {
        return Kit::Big;
    }
    if says(toks, &r.fan_words) {
        return Kit::Fan;
    }
    if says(toks, &r.device_words) {
        return Kit::Device;
    }
    Kit::Other
}

/// The feeder (an index into the spec's feeders) a run's piece belongs to.
fn feeder_of(cx: &Cx, it: &Item) -> Option<usize> {
    let system = made_tag(it.e, "system").map(up).unwrap_or_default();
    let mepid = made_tag(it.e, "mepid").unwrap_or_default();
    let from_id = mepid.strip_prefix("feeder:").map(|r| up(r.split(['@', '/']).next().unwrap_or_default()));
    cx.sp.mep.feeders.iter().take(100_000).position(|f| {
        let t = up(&f.tag);
        !t.is_empty() && (t == system || from_id.as_deref() == Some(t.as_str()))
    })
}

/// A circuit's number on its panel: `PU-1-5` on panel `PU-1` is `5`.
fn circuit_number(panel: &str, circuit: &str) -> String {
    let (p, c) = (panel.trim(), circuit.trim());
    match c.get(..p.len()) {
        Some(head) if !p.is_empty() && head.eq_ignore_ascii_case(p) => {
            c.get(p.len()..).unwrap_or_default().trim_start_matches(['-', '/', '.', ' ']).to_string()
        }
        _ => c.to_string(),
    }
}

fn is_equipment(it: &Item) -> bool {
    it.part == "equipment"
        || (it.part == "mep"
            && (made_tag(it.e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment:"))
                || made_tag(it.e, "kind").is_some_and(|k| k.eq_ignore_ascii_case("equipment"))))
}

fn is_vertical(it: &Item) -> bool {
    let (dx, dy, dz) = (it.hi[0] - it.lo[0], it.hi[1] - it.lo[1], it.hi[2] - it.lo[2]);
    dz > 2.0 && dz > dx && dz > dy
}

fn role_of(d: &Drawing, cx: &Cx, it: &Item) -> Role {
    let rules = cx.rules;
    let store = cadaddy_doc::props::read(&it.e.common.xdata);
    let tag = tag_of(d, it.e, &store);
    let toks = element_words(it, &tag);
    let (group, default_on) = group_of(rules, it, &toks);
    let mut r = Role { default_on, ..Role::default() };
    r.furniture = group == "furniture";
    r.cabinets = group == "cabinets";
    r.group = group;
    if r.furniture || r.cabinets {
        return r;
    }
    let part = it.part.as_str();
    r.arch = Rules::is(&rules.architectural_parts, part);
    r.as_box = Rules::is(&rules.box_parts, part);
    r.mep_context = Rules::is(&rules.mep_context_parts, part);
    r.overhead = Rules::is(&rules.overhead_parts, part);
    let kind = made_tag(it.e, "kind").unwrap_or_default().to_ascii_lowercase();
    let mpart = made_tag(it.e, "part").unwrap_or("segment").to_ascii_lowercase();
    let size = made_tag(it.e, "size").and_then(size_in);
    let crossing_kind = rules.crossing.kinds.iter().any(|k| k.eq_ignore_ascii_case(&kind));
    let crosses = crossing_kind && cx.crosses(it.lo[2], it.hi[2]);
    match part {
        _ if is_equipment(it) => equipment_role(cx, it, &toks, &tag, &mut r),
        "mep" => {
            if mpart.starts_with("terminal:") {
                match (kind.as_str(), it.class.key) {
                    ("duct", _) => r.full = Some("air terminal".into()),
                    (_, "fire") => r.left_out = Some("sprinkler head (the floor view's)"),
                    _ => {
                        r.room_tier = true;
                        r.left_out = Some("fixture or device");
                    }
                }
            } else if let Some(piece) = mpart.strip_prefix("atom:") {
                if matches!(piece, "conduit" | "cable") {
                    run_role(d, cx, it, &store, size, crosses, &mut r);
                } else if matches!(piece, "box" | "ring") {
                    r.room_tier = true;
                    r.left_out = Some("box");
                } else {
                    r.left_out = Some("piece of a run or of gear");
                }
            } else {
                match kind.as_str() {
                    "duct" => match size {
                        Some(z) if z >= rules.sizes.main_duct_in => {
                            r.mains = Some(format!("main duct ({z} in, from {} in)", rules.sizes.main_duct_in));
                            r.role = Some("main".into());
                        }
                        _ => {
                            r.full = Some("duct".into());
                            r.role = Some(if crosses { "riser" } else { "branch" }.into());
                        }
                    },
                    "pipe" => pipe_role(cx, it, size, crosses, &mut r),
                    "tray" => {
                        r.mains = Some("cable tray".into());
                        r.role = Some("main".into());
                    }
                    "conduit" | "cable" => run_role(d, cx, it, &store, size, crosses, &mut r),
                    _ => r.left_out = Some("grounding or other"),
                }
            }
        }
        "light" => r.left_out = Some("light fixture (the floor view's)"),
        "diffuser" => r.full = Some("air terminal".into()),
        "device" => {
            if it.class.key == "lights" {
                r.left_out = Some("light fixture (the floor view's)");
            } else if it.class.key == "fire" {
                r.left_out = Some("sprinkler head (the floor view's)");
            } else {
                r.room_tier = true;
                r.left_out = Some("fixture or device");
            }
        }
        _ => {}
    }
    r
}

fn equipment_role(cx: &Cx, it: &Item, toks: &[String], tag: &str, r: &mut Role) {
    let class = equipment_class(cx, it, toks);
    let tag_up = up(tag);
    match class {
        Kit::Hoistway => {
            r.mains = Some("elevator: one hoistway volume".into());
            r.as_box = true;
            r.role = Some("hoistway".into());
        }
        Kit::ElevatorMachine => {
            r.mains = Some("elevator machine or controller".into());
            r.role = Some("elevator_machine".into());
        }
        Kit::Disconnect => {
            let elevator = says(toks, &cx.rules.elevator.words) || Cx::near(&cx.hoistways, it.lo, it.hi, cx.rules.reach_ft());
            if elevator {
                r.mains = Some("elevator disconnect".into());
                r.role = Some("elevator_disconnect".into());
            } else if Cx::near(&cx.big, it.lo, it.hi, cx.rules.reach_ft()) {
                r.full = Some("disconnect of large equipment".into());
                r.role = Some("disconnect".into());
            } else {
                r.left_out = Some("small electrical equipment");
            }
        }
        Kit::Panel => {
            let (scope, why) = cx.panels.get(&tag_up).cloned().unwrap_or(("unknown", "not in the distribution data".into()));
            if scope == "house" {
                r.mains = Some(format!("house panel: {why}"));
            } else {
                r.full = Some(format!("{} panel: {why}", if scope == "unit" { "room or unit" } else { "a" }));
            }
            r.role = Some("panel".into());
            r.house = Some((scope, why));
        }
        Kit::Distribution => {
            r.mains = Some("electrical gear".into());
            r.role = Some("gear".into());
        }
        Kit::Big => {
            r.mains = Some("large equipment".into());
            r.role = Some("equipment".into());
        }
        Kit::Fan => {
            r.full = Some("fan".into());
            r.role = Some("equipment".into());
        }
        Kit::Device => r.left_out = Some("small electrical equipment"),
        Kit::Other => {
            if r.group == "electrical" {
                r.left_out = Some("small electrical equipment");
            } else {
                r.full = Some("equipment".into());
                r.role = Some("equipment".into());
            }
        }
    }
}

fn pipe_role(cx: &Cx, it: &Item, size: Option<f64>, crosses: bool, r: &mut Role) {
    let sizes = &cx.rules.sizes;
    let vertical = is_vertical(it);
    let Some(z) = size else {
        r.left_out = Some("pipe size unknown");
        return;
    };
    if it.class.key == "fire" {
        if vertical || z >= sizes.fire_main_in {
            r.mains = Some(if vertical { "fire riser".into() } else { format!("fire main ({z} in, from {} in)", sizes.fire_main_in) });
            r.role = Some(if vertical { "riser" } else { "main" }.into());
        } else if crosses {
            r.full = Some("crosses a floor".into());
            r.role = Some("branch".into());
        } else {
            r.left_out = Some("sprinkler branch line (the floor view's)");
            r.role = Some("branch".into());
        }
        return;
    }
    if z >= sizes.main_pipe_in {
        r.mains = Some(if vertical { format!("riser ({z} in)") } else { format!("main ({z} in, from {} in)", sizes.main_pipe_in) });
        r.role = Some(if vertical { "riser" } else { "main" }.into());
    } else if z >= sizes.plumbing_min_in {
        r.full = Some(format!("main line ({z} in, from {} in)", sizes.plumbing_min_in));
        r.role = Some(if vertical { "riser" } else { "main" }.into());
    } else if crosses {
        r.full = Some("crosses a floor".into());
        r.role = Some(if vertical { "riser" } else { "branch" }.into());
    } else {
        r.left_out = Some("pipe under the size limit");
        r.role = Some("branch".into());
    }
}

#[allow(clippy::too_many_arguments)]
fn run_role(d: &Drawing, cx: &Cx, it: &Item, store: &cadaddy_doc::props::Props, size: Option<f64>, crosses: bool, r: &mut Role) {
    if let Some(fi) = feeder_of(cx, it) {
        let to = cx.sp.mep.feeders.get(fi).map(|f| up(&f.to)).unwrap_or_default();
        r.feeder = Some(fi);
        r.role = Some("feeder".into());
        if cx.mains_tags.contains(&to) {
            r.mains = Some(format!("feeder to {to}"));
        } else {
            r.full = Some(format!("feeder to {to}"));
        }
        return;
    }
    let circuit = circuit_of(d, it.e, store);
    if circuit.is_empty()
        && let Some(z) = size.filter(|z| *z >= cx.rules.sizes.feeder_min_in)
    {
        r.full = Some(format!("feeder by size ({z} in, from {} in: what it connects is unknown)", cx.rules.sizes.feeder_min_in));
        r.role = Some("feeder".into());
        return;
    }
    r.role = Some("branch".into());
    let panel = store.get("panel").filter(|p| p.has_value()).map(|p| up(&p.text())).unwrap_or_default();
    let number = up(&circuit_number(&panel, &circuit));
    let desc = cx.circuits.get(&(panel, number)).map(|t| tokens(t)).unwrap_or_default();
    let mid = [(it.lo[0] + it.hi[0]) / 2.0, (it.lo[1] + it.hi[1]) / 2.0];
    let in_shaft = cx.hoistways.iter().any(|(a, b)| mid[0] >= a[0] - 0.5 && mid[0] <= b[0] + 0.5 && mid[1] >= a[1] - 0.5 && mid[1] <= b[1] + 0.5);
    if in_shaft || says(&desc, &cx.rules.elevator.branch_words) {
        r.left_out = Some("elevator branch circuit");
        return;
    }
    if crosses {
        r.full = Some("crosses a floor".into());
        return;
    }
    r.room_tier = true;
    r.left_out = Some("branch circuit");
}

// ─── Generated nodes ─────────────────────────────────────────────────────────

type Tri = ([[f64; 3]; 3], [f64; 3]);

fn material(name: &str, rgb: [f32; 3], opacity: f32) -> Material {
    Material { name: name.into(), rgb, opacity }
}

/// Wound to face up (+Y).
fn up_tri(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Tri {
    let cy = (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]);
    if cy >= 0.0 { ([a, b, c], [0.0, 1.0, 0.0]) } else { ([a, c, b], [0.0, 1.0, 0.0]) }
}

/// A room's floor tile: its outline at `y` (output metres), facing up.
fn tile(place: &Place, outline: &[[f64; 2]], z_ft: f64) -> Vec<Tri> {
    let flat: Vec<Vec2> = outline.iter().map(|p| Vec2::new(p[0], p[1])).collect();
    let pts: Vec<[f64; 3]> = outline.iter().map(|p| place.of_ft([p[0], p[1], z_ft])).collect();
    cadaddy_doc::triangulate(&[flat])
        .into_iter()
        .filter_map(|t| Some(up_tri(*pts.get(t[0])?, *pts.get(t[1])?, *pts.get(t[2])?)))
        .filter(|(t, _)| {
            let (a, b, c) = (t[0], t[1], t[2]);
            ((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])).abs() > 1e-6
        })
        .collect()
}

/// A bar `w` across and `h` high along the plan edge `a`–`b` (output metres, at height `y`).
fn bar(a: [f64; 2], b: [f64; 2], y: f64, w: f64, h: f64) -> Vec<Tri> {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dz);
    if len < 1e-6 {
        return Vec::new();
    }
    let (nx, nz) = (-dz / len * w / 2.0, dx / len * w / 2.0);
    let p = |q: [f64; 2], s: f64, yy: f64| [q[0] + nx * s, yy, q[1] + nz * s];
    let c = [p(a, -1.0, y), p(b, -1.0, y), p(b, 1.0, y), p(a, 1.0, y), p(a, -1.0, y + h), p(b, -1.0, y + h), p(b, 1.0, y + h), p(a, 1.0, y + h)];
    let quad = |i: [usize; 4]| -> Vec<Tri> {
        let q: Vec<[f64; 3]> = i.iter().filter_map(|k| c.get(*k).copied()).collect();
        let (Some(q0), Some(q1), Some(q2), Some(q3)) = (q.first(), q.get(1), q.get(2), q.get(3)) else { return Vec::new() };
        let n = super::unit(super::newell(&q)).unwrap_or([0.0, 1.0, 0.0]);
        vec![([*q0, *q1, *q2], n), ([*q0, *q2, *q3], n)]
    };
    // Each side wound so its Newell normal points out.
    [[4, 5, 6, 7], [3, 2, 1, 0], [0, 1, 5, 4], [2, 3, 7, 6], [1, 2, 6, 5], [3, 0, 4, 7]].into_iter().flat_map(quad).collect()
}

/// The plan edges (output x, z) of upward faces that no other upward face shares: a floor
/// plate's outside edge (and its holes').
fn boundary(tris: &[Tri]) -> Vec<([f64; 2], [f64; 2])> {
    let q = |p: [f64; 3]| ((p[0] * 1000.0).round() as i64, (p[2] * 1000.0).round() as i64);
    let mut count: BTreeMap<((i64, i64), (i64, i64)), (usize, [f64; 2], [f64; 2])> = BTreeMap::new();
    for (t, n) in tris {
        if n[1] < 0.9 {
            continue;
        }
        for k in 0..3 {
            let (Some(a), Some(b)) = (t.get(k), t.get((k + 1) % 3)) else { continue };
            let (ka, kb) = (q(*a), q(*b));
            if ka == kb {
                continue;
            }
            let key = if ka < kb { (ka, kb) } else { (kb, ka) };
            let e = count.entry(key).or_insert((0, [a[0], a[2]], [b[0], b[2]]));
            e.0 += 1;
        }
    }
    count.into_values().filter(|(n, _, _)| *n == 1).map(|(_, a, b)| (a, b)).collect()
}

/// Edges chained into polylines (closed where they close), at most `cap` points in all.
fn chain(edges: &[([f64; 2], [f64; 2])], cap: usize) -> Vec<Vec<[f64; 2]>> {
    let q = |p: [f64; 2]| ((p[0] * 1000.0).round() as i64, (p[1] * 1000.0).round() as i64);
    let mut at: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, (a, b)) in edges.iter().enumerate() {
        at.entry(q(*a)).or_default().push(i);
        at.entry(q(*b)).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut out = Vec::new();
    let mut total = 0usize;
    for start in 0..edges.len() {
        if used.get(start).copied().unwrap_or(true) {
            continue;
        }
        let Some((a, b)) = edges.get(start).copied() else { continue };
        if let Some(u) = used.get_mut(start) {
            *u = true;
        }
        let mut line = vec![a, b];
        let mut end = b;
        while total + line.len() < cap {
            let next = at.get(&q(end)).and_then(|v| v.iter().copied().find(|i| !used.get(*i).copied().unwrap_or(true)));
            let Some(i) = next else { break };
            if let Some(u) = used.get_mut(i) {
                *u = true;
            }
            let Some((p, r)) = edges.get(i).copied() else { break };
            end = if q(p) == q(end) { r } else { p };
            line.push(end);
        }
        total += line.len();
        out.push(line);
        if total >= cap {
            break;
        }
    }
    out
}

// ─── The set ─────────────────────────────────────────────────────────────────

/// A package to write: its name (its path under the folder, without extension), view, items,
/// generated nodes, mode, frame and caps.
struct Pkg<'a> {
    name: String,
    view: &'static str,
    items: Vec<Item<'a>>,
    gens: Vec<Generated>,
    mode: &'static str,
    generic: bool,
    left_out: BTreeMap<String, usize>,
    level: Option<String>,
    room: Option<usize>,
}

fn clean(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).take(60).collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "x".into() } else { out }
}

fn sha_hex(b: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b);
    h.finalize().iter().map(|x| format!("{x:02x}")).collect()
}

fn ft3(v: [f64; 3]) -> Value {
    json!([r4(v[0]), r4(v[1]), r4(v[2])])
}

/// A building-wide request for every object is refused: the views split it.
pub(super) fn everything() -> crate::EngineError {
    bad(
        CMD,
        "an AR export never puts every object of the building in one package: export the package set (set: true, dir: a folder) for the architectural, MEP, level and room views, or one room, a level or a box",
    )
}

pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let started = web_time::Instant::now();
    for k in ["room", "rooms", "level", "box", "anchor", "path", "trades", "kinds"] {
        if p.get(k).is_some_and(|v| !v.is_null()) {
            return Err(bad(CMD, format!("`{k}` is for one export: `set` writes the whole building's packages into `dir`")));
        }
    }
    let dir = str_param(p, "dir")
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .ok_or_else(|| bad(CMD, "`set` needs `dir`: the folder to write the packages into"))?;
    if p.get("dir").is_some_and(|v| !v.is_string()) {
        return Err(bad(CMD, "`dir` must be a folder path"));
    }
    let (want_glb, want_usdz) = formats(p)?;
    let small = bool_param(CMD, p, "small")?.unwrap_or(false);
    let force = bool_param(CMD, p, "force")?.unwrap_or(false);
    let root = PathBuf::from(dir);
    if root.exists() && !root.is_dir() {
        return Err(bad(CMD, format!("{dir} is a file, not a folder")));
    }
    let index_path = root.join(INDEX_FILE);
    // An earlier set's folder is the set's own to replace; anything else needs force: true.
    let ours = force || is_index(&index_path);
    let guard_p = if ours { json!({"force": true}) } else { json!({}) };
    super::super::exchange::guard_output(s, CMD, &guard_p, &index_path.to_string_lossy())?;

    let (sp, _) = super::super::bspec::current(s, CMD)?;
    let d = s.doc()?;
    if !super::super::bimlook::has_model(d) {
        return Err(bad(CMD, "nothing is built yet: run model.build first"));
    }
    let (rules, own_rules) = Rules::of(d);
    let set = rules.set.clone();
    let tri_cap = cap_param(p, "max_triangles", set.tri_cap())?;
    let byte_cap = cap_param(p, "max_bytes", if set.max_bytes == 0 { rules.caps.max_bytes } else { set.max_bytes })?;
    let room_caps = (cap_param(p, "max_triangles", rules.caps.max_triangles)?, cap_param(p, "max_bytes", rules.caps.max_bytes)?);
    let frame = Frame::of_spec(d, &sp);
    let all_rooms = rooms(d, &frame);
    if all_rooms.len() > MAX_SET_ROOMS {
        return Err(bad(CMD, format!("{} rooms: a set writes at most {MAX_SET_ROOMS} room packages", all_rooms.len())));
    }
    if sp.levels.len() > MAX_SET_LEVELS {
        return Err(bad(CMD, format!("{} levels: a set writes at most {MAX_SET_LEVELS} level packages", sp.levels.len())));
    }
    // Each room's anchor (its corner by its entry), as the single-room export picks it.
    let mut anchors = Vec::with_capacity(all_rooms.len());
    for r in &all_rooms {
        let sc = Scope { rooms: vec![r.clone()], ..Scope::default() };
        anchors.push(anchor_of(s, &json!({}), &sc, &sp, &frame, &rules, &all_rooms)?);
    }
    let d = s.doc()?;
    let mut read_rules = rules.clone();
    read_rules.caps.max_elements = MAX_SET_ELEMENTS as u64;
    let mut dropped = Dropped::default();
    let items = collect(d, &frame, &sp, &read_rules, &Scope::default(), "translucent", small, &mut dropped);
    if items.is_empty() && all_rooms.is_empty() {
        return Err(bad(CMD, "nothing built to export"));
    }

    // What each element is to the views.
    let mut cx = Cx::new(&set, &sp);
    // Equipment first: the hoistways and large equipment place the disconnects near them.
    let pre: Vec<(Vec<String>, Kit)> = items
        .iter()
        .map(|it| {
            if !is_equipment(it) {
                return (Vec::new(), Kit::Other);
            }
            let store = cadaddy_doc::props::read(&it.e.common.xdata);
            let toks = element_words(it, &tag_of(d, it.e, &store));
            let class = equipment_class(&cx, it, &toks);
            (toks, class)
        })
        .collect();
    for (it, (_, class)) in items.iter().zip(&pre) {
        match class {
            Kit::Hoistway => cx.hoistways.push(([it.lo[0], it.lo[1]], [it.hi[0], it.hi[1]])),
            Kit::Big => cx.big.push(([it.lo[0], it.lo[1]], [it.hi[0], it.hi[1]])),
            _ => {}
        }
    }
    // The tags in the mains: their feeders are mains too.
    for (it, (toks, class)) in items.iter().zip(&pre) {
        if toks.is_empty() {
            continue;
        }
        let tag = up(&tag_of(d, it.e, &cadaddy_doc::props::read(&it.e.common.xdata)));
        let mains = match class {
            Kit::Hoistway | Kit::ElevatorMachine | Kit::Distribution | Kit::Big => true,
            Kit::Panel => cx.panels.get(&tag).is_some_and(|(k, _)| *k == "house"),
            Kit::Disconnect => says(toks, &set.elevator.words) || Cx::near(&cx.hoistways, it.lo, it.hi, set.reach_ft()),
            _ => false,
        };
        if mains && !tag.is_empty() {
            cx.mains_tags.insert(tag);
        }
    }
    let roles: Vec<Role> = items.iter().map(|it| role_of(d, &cx, it)).collect();

    // The building's frame: plan feet from the low corner of everything, the lowest floor's
    // elevation at y = 0; the level packages share it, so the app moves between them in place.
    let mut origin = [f64::INFINITY, f64::INFINITY];
    for it in &items {
        origin = [origin[0].min(it.lo[0]), origin[1].min(it.lo[1])];
    }
    for r in &all_rooms {
        for q in &r.outline {
            origin = [origin[0].min(q[0]), origin[1].min(q[1])];
        }
    }
    let origin = if origin.iter().all(|v| v.is_finite()) { [origin[0].floor(), origin[1].floor()] } else { [0.0, 0.0] };
    let datum = sp.levels.iter().map(|l| l.elevation_ft).filter(|e| e.is_finite()).fold(f64::INFINITY, f64::min);
    let datum = if datum.is_finite() { datum } else { 0.0 };
    let place = Place { frame: &frame, anchor: origin, floor_ft: datum, clip: None };
    let lowest = sp.levels.iter().min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)).map(|l| l.name.clone()).unwrap_or_default();
    let level_of = |it: &Item| -> String {
        if !it.level.is_empty() {
            return it.level.clone();
        }
        // No level tag: the level whose floor it stands on.
        sp.levels
            .iter()
            .filter(|l| l.elevation_ft <= it.lo[2] + 0.5)
            .max_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft))
            .map_or(lowest.clone(), |l| l.name.clone())
    };

    // Which rooms each element stands in.
    let margin = rules.margin_ft();
    let mut in_rooms: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    for (ri, room) in all_rooms.iter().enumerate() {
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for q in &room.outline {
            lo = [lo[0].min(q[0]), lo[1].min(q[1])];
            hi = [hi[0].max(q[0]), hi[1].max(q[1])];
        }
        for (ii, it) in items.iter().enumerate() {
            if !level_of(it).eq_ignore_ascii_case(&room.level)
                || it.hi[0] + margin < lo[0]
                || it.lo[0] - margin > hi[0]
                || it.hi[1] + margin < lo[1]
                || it.lo[1] - margin > hi[1]
            {
                continue;
            }
            if box_meets(&room.outline, [it.lo[0] - margin, it.lo[1] - margin], [it.hi[0] + margin, it.hi[1] + margin])
                && let Some(v) = in_rooms.get_mut(ii)
            {
                v.push(ri);
            }
        }
    }

    let arch_rgb = rules.rgb("architecture", "architecture");

    // building-architectural.
    let mut arch = Pkg {
        name: "building-architectural".into(),
        view: "architectural",
        items: Vec::new(),
        gens: Vec::new(),
        mode: "solid",
        generic: true,
        left_out: BTreeMap::new(),
        level: None,
        room: None,
    };
    let mut mains = Pkg { name: "building-mep-mains".into(), view: "mep", mode: "translucent", ..pkg_like(&arch) };
    let mut full = Pkg { name: "building-mep-full".into(), view: "mep", mode: "translucent", ..pkg_like(&arch) };
    for (it, r) in items.iter().zip(&roles) {
        if r.arch {
            let mut x = with(it, r, &[]);
            x.as_box = r.as_box;
            if r.as_box {
                x.extra.insert("simplified".into(), json!("box"));
            }
            arch.items.push(x);
        }
        if r.mep_context {
            mains.items.push(with(it, r, &[("view_basis", json!("context: the floors"))]));
        }
        let mep = r.mains.is_some() || r.full.is_some() || r.left_out.is_some();
        if let Some(why) = &r.mains {
            let mut x = with(it, r, &[("view_basis", json!(why))]);
            x.as_box = r.as_box;
            if r.as_box {
                x.extra.insert("simplified".into(), json!("box"));
            }
            x.context = false;
            mains.items.push(x);
        } else if let Some(why) = &r.full {
            let mut x = with(it, r, &[("view_basis", json!(why))]);
            x.context = false;
            full.items.push(x);
        } else if mep && let Some(why) = r.left_out {
            *mains.left_out.entry(why.to_string()).or_default() += 1;
        }
    }
    full.left_out = mains.left_out.clone();
    let unknown_pipes = mains.left_out.get("pipe size unknown").copied().unwrap_or(0);
    // Room tiles: in the architectural view and in each level's.
    let tiles: Vec<Generated> = all_rooms
        .iter()
        .map(|r| {
            let floor = floor_of(&sp, &r.level);
            let mut meta = Map::new();
            meta.insert("id".into(), json!(r.handle.hex()));
            meta.insert("kind".into(), json!("room"));
            meta.insert("tag".into(), json!(r.label()));
            meta.insert("number".into(), json!(r.number));
            meta.insert("name".into(), json!(r.name));
            meta.insert("level".into(), json!(r.level));
            meta.insert("area_sf".into(), json!(r4(r.area)));
            meta.insert("group".into(), json!("base"));
            meta.insert("role".into(), json!("room_tile"));
            meta.insert("trade".into(), json!("architecture"));
            Generated {
                name: node_name("room", &r.label()),
                tris: tile(&place, &r.outline, floor + 0.03),
                material: material("room_tile", rules.rgb("room_tile", "architecture"), 1.0),
                meta,
            }
        })
        .collect();
    arch.gens.extend(tiles.iter().cloned());

    // The floors, by level: the solid exterior (each floor plate stretched to its storey) and
    // the one outline (the outside edge of the largest plate).
    let mut floors_by_level: BTreeMap<String, Vec<Tri>> = BTreeMap::new();
    for (it, r) in items.iter().zip(&roles) {
        if it.part == "floor" && !r.furniture {
            floors_by_level.entry(level_of(it)).or_default().extend(element_triangles(&place, &it.level, it.e));
        }
    }
    let mut level_outlines: BTreeMap<String, Vec<Vec<[f64; 2]>>> = BTreeMap::new();
    let mut biggest: Option<(f64, String)> = None;
    for (lv, tris) in &floors_by_level {
        let Some(l) = sp.level(lv) else { continue };
        let top_y = tris.iter().flat_map(|(t, _)| t.iter().map(|p| p[1])).fold(f64::NEG_INFINITY, f64::max);
        let to_y = (l.elevation_ft + l.height_ft.max(1.0) - datum) * M_PER_FT;
        let shell: Vec<Tri> =
            tris.iter().map(|(t, n)| (t.map(|p| if p[1] >= top_y - 1e-4 { [p[0], to_y.max(p[1]), p[2]] } else { p }), *n)).collect();
        let mut meta = Map::new();
        meta.insert("id".into(), json!(format!("exterior:{lv}")));
        meta.insert("kind".into(), json!("exterior"));
        meta.insert("level".into(), json!(lv));
        meta.insert("group".into(), json!("base"));
        meta.insert("role".into(), json!("exterior"));
        meta.insert("reveal".into(), json!(true));
        meta.insert("basis".into(), json!("the level's floor plates raised to its floor-to-floor height (generated, for the opening reveal)"));
        arch.gens.push(Generated { name: node_name("exterior", lv), tris: shell, material: material("exterior", arch_rgb, 1.0), meta });
        let edges = boundary(tris);
        let area: f64 = tris
            .iter()
            .filter(|(_, n)| n[1] > 0.9)
            .map(|(t, _)| ((t[1][0] - t[0][0]) * (t[2][2] - t[0][2]) - (t[2][0] - t[0][0]) * (t[1][2] - t[0][2])).abs() / 2.0)
            .sum();
        if biggest.as_ref().is_none_or(|b| area > b.0) {
            biggest = Some((area, lv.clone()));
        }
        let to_ft = |q: [f64; 2]| [r4(q[0] / M_PER_FT + origin[0]), r4(origin[1] - q[1] / M_PER_FT)];
        let ft_edges: Vec<([f64; 2], [f64; 2])> = edges.iter().map(|(a, b)| (to_ft(*a), to_ft(*b))).collect();
        level_outlines.insert(lv.clone(), chain(&ft_edges, 20_000));
    }
    if let Some((_, lv)) = &biggest {
        let edges = floors_by_level.get(lv).map(|t| boundary(t)).unwrap_or_default();
        let w = set.bar_m();
        let tris: Vec<Tri> = edges.iter().flat_map(|(a, b)| bar(*a, *b, 0.0, w, w)).collect();
        let mut meta = Map::new();
        meta.insert("id".into(), json!("outline"));
        meta.insert("kind".into(), json!("outline"));
        meta.insert("group".into(), json!("base"));
        meta.insert("role".into(), json!("outline"));
        meta.insert("basis".into(), json!(format!("the outside edge of the largest floor plate ({lv}), at the lowest floor (generated)")));
        mains.gens.push(Generated { name: "building_outline".into(), tris, material: material("outline", arch_rgb, 1.0), meta });
    }

    // One package per level, one per room.
    let cut_ft = set.cut_ft();
    let mut levels: Vec<Pkg> = Vec::new();
    for l in sp.levels.iter().take(MAX_SET_LEVELS) {
        let cut_y = (l.elevation_ft + cut_ft - datum) * M_PER_FT;
        let mut pk = Pkg {
            name: format!("levels/{}", clean(&l.name)),
            view: "floor",
            mode: "translucent",
            generic: false,
            level: Some(l.name.clone()),
            ..pkg_like(&arch)
        };
        for ((it, r), rs) in items.iter().zip(&roles).zip(&in_rooms) {
            if !level_of(it).eq_ignore_ascii_case(&l.name) || r.furniture || (r.room_tier && !rs.is_empty()) {
                continue;
            }
            let mut extra: Vec<(&str, Value)> = Vec::new();
            if r.overhead {
                extra.push(("overhead", json!(true)));
                extra.push(("default_on", json!(false)));
            }
            if r.room_tier {
                extra.push(("outside_rooms", json!(true)));
            }
            let mut x = with(it, r, &extra);
            if Rules::is(&set.cut_parts, &it.part) {
                x.cut_m = Some(cut_y);
            }
            pk.items.push(x);
        }
        pk.gens
            .extend(tiles.iter().filter(|t| t.meta.get("level").and_then(Value::as_str).is_some_and(|v| v.eq_ignore_ascii_case(&l.name))).cloned());
        levels.push(pk);
    }
    let mut room_pkgs: Vec<Pkg> = Vec::new();
    let mut names_used: BTreeSet<String> = BTreeSet::new();
    let furniture_outside = roles.iter().zip(&in_rooms).filter(|(r, rs)| r.furniture && rs.is_empty()).count();
    for (ri, room) in all_rooms.iter().enumerate() {
        let mut base = format!("rooms/{}-{}", clean(&room.level), clean(&room.label()));
        if !names_used.insert(base.clone()) {
            base = format!("{base}-{}", room.handle.hex());
            names_used.insert(base.clone());
        }
        let mut pk = Pkg { name: base, view: "room", mode: "", generic: false, level: Some(room.level.clone()), room: Some(ri), ..pkg_like(&arch) };
        for ((it, r), rs) in items.iter().zip(&roles).zip(&in_rooms) {
            if rs.contains(&ri) {
                let mut x = with(it, r, &[]);
                x.room = Some(room.label());
                pk.items.push(x);
            }
        }
        room_pkgs.push(pk);
    }

    // No package holds every object of the model.
    let all_ids: BTreeSet<String> = items.iter().map(|it| it.e.handle.hex()).chain(all_rooms.iter().map(|r| r.handle.hex())).collect();
    let ids_of = |pk: &Pkg| -> BTreeSet<String> {
        pk.items
            .iter()
            .map(|it| it.e.handle.hex())
            .chain(pk.gens.iter().filter_map(|g| g.meta.get("id").and_then(Value::as_str).map(str::to_string)))
            .filter(|id| all_ids.contains(id))
            .collect()
    };
    let mut packages_of: HashMap<String, Vec<String>> = HashMap::new();
    let pkgs: Vec<&Pkg> = [&arch, &mains, &full].into_iter().chain(levels.iter()).chain(room_pkgs.iter()).collect();
    for pk in &pkgs {
        let ids = ids_of(pk);
        if all_ids.len() > 1 && ids.len() >= all_ids.len() {
            return Err(everything());
        }
        for id in ids {
            packages_of.entry(id).or_default().push(pk.name.clone());
        }
    }

    // Write each package.
    std::fs::create_dir_all(root.join("levels")).map_err(|e| bad(CMD, format!("{dir}: {e}")))?;
    std::fs::create_dir_all(root.join("rooms")).map_err(|e| bad(CMD, format!("{dir}: {e}")))?;
    let mut written: Vec<Value> = Vec::new();
    let mut room_entries: Vec<Value> = Vec::new();
    let mut room_totals = (0usize, 0usize, 0u64, 0usize);
    for pk in &pkgs {
        if pk.items.is_empty() && pk.gens.is_empty() {
            continue;
        }
        let stem_path = root.join(&pk.name);
        let stem = stem_path.to_string_lossy().into_owned();
        for (want, ext) in [(want_glb, "glb"), (want_usdz, "usdz"), (true, "json")] {
            if want {
                super::super::exchange::guard_output(s, CMD, &guard_p, &format!("{stem}.{ext}"))?;
            }
        }
        let stem_name = stem_path.file_name().map_or("model".into(), |n| n.to_string_lossy().into_owned());
        let (fitted, frame_v) = match pk.room.and_then(|ri| all_rooms.get(ri).zip(anchors.get(ri))) {
            Some((room, anchor)) => {
                let sc = Scope { rooms: vec![room.clone()], ..Scope::default() };
                let clip_m = clip_box(&sc, margin).map(|(lo, hi)| {
                    let (ax, ay) = (anchor.at[0], anchor.at[1]);
                    [(lo[0] - ax) * M_PER_FT, (hi[0] - ax) * M_PER_FT, -(hi[1] - ay) * M_PER_FT, -(lo[1] - ay) * M_PER_FT]
                });
                let rplace = Place { frame: &frame, anchor: anchor.at, floor_ft: anchor.floor_ft, clip: clip_m };
                let mode = Some(rules.context.mode.to_ascii_lowercase())
                    .filter(|m| matches!(m.as_str(), "translucent" | "solid" | "off"))
                    .unwrap_or_else(|| "translucent".into());
                let mut summ = summary(anchor, &sc, &rules, own_rules, &mode, small);
                summ["package"] = json!({"name": pk.name, "view": pk.view, "level": pk.level, "room": room.label()});
                summ["generic"] = json!(false);
                summ["field_alignment"] = json!(true);
                let points = reference_points(&sc, &rplace);
                let f = fit(
                    d,
                    &pk.items,
                    &pk.gens,
                    &rplace,
                    &rules,
                    &mode,
                    &summ,
                    &points,
                    &stem_name,
                    (want_glb, want_usdz),
                    &Dropped::default(),
                    room_caps,
                    &ROOM_STEPS,
                )?;
                (f, json!({"anchor": summ["anchor"]}))
            }
            None => {
                let an = Anchor {
                    kind: "building_origin",
                    name: "building origin".into(),
                    level: lowest.clone(),
                    room: None,
                    at: origin,
                    floor_ft: datum,
                    how: "the low corner of the building's plan, the lowest floor's elevation".into(),
                    entry: None,
                };
                let sc = Scope { level: pk.level.clone(), ..Scope::default() };
                let mut summ = summary(&an, &sc, &rules, own_rules, pk.mode, small);
                summ["package"] = json!({"name": pk.name, "view": pk.view, "level": pk.level});
                summ["generic"] = json!(pk.generic);
                summ["field_alignment"] = json!(false);
                if pk.generic {
                    summ["generic_note"] = json!("simplified on purpose for looking, not for layout: never align it to the field");
                }
                let steps: &[Step] = if pk.generic { &BUILDING_STEPS } else { &LEVEL_STEPS };
                let caps = if pk.generic { (tri_cap, byte_cap) } else { room_caps };
                let f = fit(
                    d,
                    &pk.items,
                    &pk.gens,
                    &place,
                    &rules,
                    pk.mode,
                    &summ,
                    &[],
                    &stem_name,
                    (want_glb, want_usdz),
                    &Dropped::default(),
                    caps,
                    steps,
                )?;
                (f, json!({"anchor": summ["anchor"]}))
            }
        };
        let mut fitted = fitted;
        if !pk.left_out.is_empty() {
            fitted.sidecar["left_out"] = json!(pk.left_out);
        }
        let mut groups: BTreeMap<String, (usize, bool)> = BTreeMap::new();
        for n in &fitted.built.scene.nodes {
            let g = n.meta.get("group").and_then(Value::as_str).unwrap_or("base").to_string();
            let on = n.meta.get("default_on").and_then(Value::as_bool).unwrap_or(true);
            let e = groups.entry(g).or_insert((0, on));
            e.0 += 1;
            e.1 = e.1 && on;
        }
        fitted.sidecar["groups"] =
            json!(groups.iter().map(|(g, (n, on))| (g.clone(), json!({"nodes": n, "default_on": on}))).collect::<Map<String, Value>>());
        let (_, bytes) = write_files(&stem, &fitted)?;
        let side_text = serde_json::to_string_pretty(&fitted.sidecar).map_err(|e| bad(CMD, e.to_string()))?;
        let mut files = Map::new();
        let mut content = Sha256::new();
        for (role, data) in [("glb", fitted.glb.as_deref()), ("usdz", fitted.usdz.as_deref()), ("sidecar", Some(side_text.as_bytes()))] {
            let Some(data) = data else { continue };
            let ext = if role == "sidecar" { "json" } else { role };
            let sha = sha_hex(data);
            if role != "sidecar" {
                content.update(sha.as_bytes());
            }
            files.insert(role.into(), json!({"path": format!("{}.{ext}", pk.name), "bytes": data.len(), "sha256": sha}));
        }
        let content: String = content.finalize().iter().map(|x| format!("{x:02x}")).collect();
        let tris = fitted.built.scene.drawn_triangles();
        let nodes = fitted.built.scene.nodes.len();
        let model_bytes = bytes.get("glb").and_then(Value::as_u64).unwrap_or(0).max(bytes.get("usdz").and_then(Value::as_u64).unwrap_or(0));
        let entry = json!({
            "name": pk.name,
            "view": pk.view,
            "level": pk.level,
            "generic": pk.generic,
            "field_alignment": !pk.generic,
            "nodes": nodes,
            "triangles": tris,
            "simplified": fitted.sidecar["simplified"],
            "files": files,
            "content_hash": content,
            "frame": frame_v,
        });
        if pk.room.is_some() {
            room_totals = (room_totals.0 + 1, room_totals.1 + tris, room_totals.2 + model_bytes, room_totals.3.max(tris));
            room_entries.push(entry);
        } else {
            written.push(entry);
        }
    }

    // The connection graph: every element, its system, what feeds it and what it serves.
    let tag_ids: HashMap<String, String> = items
        .iter()
        .filter(|it| is_equipment(it))
        .filter_map(|it| {
            let t = up(&tag_of(d, it.e, &cadaddy_doc::props::read(&it.e.common.xdata)));
            (!t.is_empty()).then(|| (t, it.e.handle.hex()))
        })
        .collect();
    let link = |tag: &str, via: Option<&str>, extra: &[(&str, Value)], known: &str| -> Value {
        let mut m = Map::new();
        m.insert("tag".into(), json!(tag));
        if let Some(id) = tag_ids.get(&up(tag)) {
            m.insert("id".into(), json!(id));
        }
        if let Some(v) = via {
            m.insert("via".into(), json!(v));
        }
        for (k, v) in extra {
            m.insert((*k).into(), v.clone());
        }
        m.insert("link".into(), json!(known));
        Value::Object(m)
    };
    let feeders = &sp.mep.feeders;
    let mut graph: Vec<Value> = Vec::with_capacity(items.len() + all_rooms.len());
    let mut links = (0usize, 0usize, 0usize);
    for (it, r) in items.iter().zip(&roles) {
        let id = it.e.handle.hex();
        let store = cadaddy_doc::props::read(&it.e.common.xdata);
        let tag = tag_of(d, it.e, &store);
        let system = made_tag(it.e, "system").map(str::to_string).filter(|v| !v.is_empty());
        let mut fed: Vec<Value> = Vec::new();
        let mut serves: Vec<Value> = Vec::new();
        let is_mep = it.trade != "architecture" && it.trade != "structure" && it.trade != "site" && !r.furniture && !r.cabinets;
        if let Some(fi) = r.feeder.and_then(|i| feeders.get(i)) {
            fed.push(link(&fi.from, Some(&fi.tag), &[], "known"));
            serves.push(link(&fi.to, Some(&fi.tag), &[], "known"));
        } else if r
            .role
            .as_deref()
            .is_some_and(|x| matches!(x, "panel" | "gear" | "equipment" | "hoistway" | "elevator_machine" | "elevator_disconnect" | "disconnect"))
            && !tag.is_empty()
        {
            let t = up(&tag);
            for f in feeders.iter().take(100_000) {
                if up(&f.to) == t {
                    fed.push(link(&f.from, Some(&f.tag), &[], "known"));
                }
                if up(&f.from) == t && serves.len() < MAX_LINKS {
                    serves.push(link(&f.to, Some(&f.tag), &[], "known"));
                }
            }
            if let Some(eq) = sp.mep.equipment.iter().find(|e| up(&e.tag) == t) {
                for sys in eq.systems.iter().take(MAX_LINKS) {
                    serves.push(json!({"system": sys, "link": "known"}));
                }
            }
            for (k, desc) in cx.circuits.iter() {
                if k.0 == t && serves.len() < MAX_LINKS {
                    serves.push(json!({"circuit": k.1, "description": desc, "link": "known"}));
                }
            }
        } else if is_mep {
            let panel = store.get("panel").filter(|p| p.has_value());
            let circuit = circuit_of(d, it.e, &store);
            if let Some(pp) = panel {
                let how = if pp.source.kind.is_assumed() { "inferred" } else { "known" };
                let number = circuit_number(&pp.text(), &circuit);
                fed.push(link(&pp.text(), None, &[("circuit", json!(number))], how));
            } else if let Some(sys) = &system {
                let units: Vec<&cadaddy_walk3d::MepEquipment> =
                    sp.mep.equipment.iter().filter(|e| e.systems.iter().any(|x| x.eq_ignore_ascii_case(sys))).take(MAX_LINKS).collect();
                for u in units {
                    fed.push(link(&u.tag, Some(sys), &[], "known"));
                }
            }
            if fed.is_empty() && !circuit.is_empty() {
                fed.push(json!({"circuit": circuit, "link": "unknown", "why": "its panel is not given"}));
            }
        }
        let summary = if fed.iter().any(|f| f["link"] == "known") {
            links.0 += 1;
            "known"
        } else if fed.iter().any(|f| f["link"] == "inferred") {
            links.1 += 1;
            "inferred"
        } else {
            if is_mep {
                links.2 += 1;
            }
            "unknown"
        };
        let mut m = Map::new();
        m.insert("id".into(), json!(id));
        m.insert("kind".into(), json!(it.kind));
        if !tag.is_empty() {
            m.insert("tag".into(), json!(tag));
        }
        m.insert("group".into(), json!(r.group));
        m.insert("level".into(), json!(level_of(it)));
        if let Some(sys) = &system {
            m.insert("system".into(), json!(sys));
        } else if let Some(f) = r.feeder.and_then(|i| feeders.get(i)) {
            m.insert("system".into(), json!(f.tag));
        }
        if let Some(role) = &r.role {
            m.insert("role".into(), json!(role));
        }
        if let Some((scope, why)) = &r.house {
            m.insert("panel_scope".into(), json!({"scope": scope, "basis": why}));
        }
        m.insert("packages".into(), json!(packages_of.get(&id).cloned().unwrap_or_default()));
        if is_mep {
            m.insert("fed_by".into(), json!(fed));
            m.insert("serves".into(), json!(serves));
            m.insert("link".into(), json!(summary));
        }
        graph.push(Value::Object(m));
    }
    for r in &all_rooms {
        let id = r.handle.hex();
        graph.push(json!({"id": id, "kind": "room", "tag": r.label(), "group": "base", "level": r.level, "packages": packages_of.get(&id).cloned().unwrap_or_default()}));
    }

    // The index.
    let level_entries: Vec<Value> = sp
        .levels
        .iter()
        .take(MAX_SET_LEVELS)
        .map(|l| {
            json!({
                "name": l.name,
                "elevation_ft": r4(l.elevation_ft),
                "height_ft": r4(l.height_ft),
                "outline": level_outlines.get(&l.name).cloned().unwrap_or_default(),
                "package": format!("levels/{}", clean(&l.name)),
                "rooms": all_rooms.iter().filter(|r| r.level.eq_ignore_ascii_case(&l.name)).count(),
            })
        })
        .collect();
    let rooms_json: Vec<Value> = all_rooms
        .iter()
        .zip(&anchors)
        .zip(&room_pkgs)
        .map(|((r, a), pk)| {
            let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for q in &r.outline {
                lo = [lo[0].min(q[0]), lo[1].min(q[1])];
                hi = [hi[0].max(q[0]), hi[1].max(q[1])];
            }
            json!({
                "id": r.handle.hex(),
                "number": r.number,
                "name": r.name,
                "label": r.label(),
                "level": r.level,
                "bounds": {"min": [r4(lo[0]), r4(lo[1])], "max": [r4(hi[0]), r4(hi[1])]},
                "area_sf": r4(r.area),
                "anchor": {"name": a.name, "kind": a.kind, "plan_x": r4(a.at[0]), "plan_y": r4(a.at[1]), "floor_elevation_ft": r4(a.floor_ft), "how": a.how},
                "package": if pk.items.is_empty() { Value::Null } else { json!(pk.name) },
                "elements": pk.items.len(),
            })
        })
        .collect();
    let equipment: Vec<Value> = items
        .iter()
        .zip(&roles)
        .filter(|(_, r)| {
            r.role
                .as_deref()
                .is_some_and(|x| matches!(x, "panel" | "gear" | "equipment" | "hoistway" | "elevator_machine" | "elevator_disconnect" | "disconnect"))
        })
        .map(|(it, r)| {
            let tag = tag_of(d, it.e, &cadaddy_doc::props::read(&it.e.common.xdata));
            let kind = made_tag(it.e, "part").and_then(|p| p.strip_prefix("equipment:")).map(str::to_string).unwrap_or_else(|| it.kind.clone());
            json!({
                "id": it.e.handle.hex(),
                "kind": kind,
                "tag": tag,
                "level": level_of(it),
                "group": r.group,
                "position_ft": ft3([(it.lo[0] + it.hi[0]) / 2.0, (it.lo[1] + it.hi[1]) / 2.0, it.lo[2]]),
                "view": if r.mains.is_some() { "mains" } else if r.full.is_some() { "full" } else { "floor" },
                "panel_scope": r.house.as_ref().map(|(k, _)| k),
            })
        })
        .collect();
    // Main systems by trade: each system's elements in the mains and full views.
    let mut systems: BTreeMap<String, BTreeMap<String, (String, usize, usize)>> = BTreeMap::new();
    for (it, r) in items.iter().zip(&roles) {
        let (in_m, in_f) = (r.mains.is_some(), r.full.is_some());
        if !(in_m || in_f) {
            continue;
        }
        let name = made_tag(it.e, "system")
            .map(str::to_string)
            .filter(|v| !v.is_empty())
            .or_else(|| r.feeder.and_then(|i| feeders.get(i)).map(|f| f.tag.clone()));
        let Some(name) = name else { continue };
        let kind = made_tag(it.e, "kind").unwrap_or("equipment").to_string();
        let e = systems.entry(r.group.clone()).or_default().entry(name).or_insert((kind, 0, 0));
        if in_m {
            e.1 += 1;
        } else {
            e.2 += 1;
        }
    }
    let systems_json: Map<String, Value> = systems
        .into_iter()
        .map(|(g, m)| (g, json!(m.into_iter().map(|(n, (k, a, b))| json!({"name": n, "kind": k, "mains": a, "full": b})).collect::<Vec<_>>())))
        .collect();
    let level_pkgs: Map<String, Value> = levels.iter().filter_map(|pk| Some((pk.level.clone()?, json!([pk.name])))).collect();
    let room_pkg_map: Map<String, Value> =
        room_pkgs.iter().filter(|pk| !pk.items.is_empty()).filter_map(|pk| Some((all_rooms.get(pk.room?)?.handle.hex(), json!([pk.name])))).collect();
    let group_rules: Map<String, Value> = {
        let mut m = Map::new();
        for g in ["base", "mechanical", "electrical", "plumbing", "cabinets", "furniture"] {
            let on = set.groups.iter().find(|r| slug(&r.group) == g).and_then(|r| r.default_on).unwrap_or(true);
            m.insert(g.into(), json!({"default_on": on, "always_on": g == "base"}));
        }
        m
    };
    let title = s.state().map(|st| st.title.clone()).unwrap_or_default();
    let index = json!({
        "schema": INDEX_SCHEMA,
        "generator": "CADaddy bim.export.ar (set)",
        "exported": super::super::project::timestamp(),
        "title": std::path::Path::new(&title).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        "units": {"plan": "ft", "models": "m", "up_axis": "Y"},
        "frame": {
            "origin_plan_ft": [r4(origin[0]), r4(origin[1])],
            "datum_elevation_ft": r4(datum),
            "how": "building and level packages: x = plan x − origin x, y = elevation − datum, z = −(plan y − origin y), metres; a room package's origin is its anchor (rooms[].anchor) on its floor",
        },
        "views": {
            "default": {"view": "mep", "density": "mains"},
            "architectural": {"packages": ["building-architectural"]},
            "mep": {"densities": {"mains": ["building-mep-mains"], "full": ["building-mep-mains", "building-mep-full"]}, "default_density": "mains"},
            "floor": {"by_level": level_pkgs},
            "room": {"by_room": room_pkg_map, "furniture": "its own group, off by default"},
        },
        "groups": group_rules,
        "levels": level_entries,
        "rooms": rooms_json,
        "equipment": equipment,
        "systems": systems_json,
        "packages": written,
        "room_packages": room_entries,
        "graph": {
            "elements": graph,
            "links": {"known": links.0, "inferred": links.1, "unknown": links.2},
            "rule": "known: the documents give it (feeder schedule, panel schedule, the plan's circuit tags); inferred: a rule or default gave it; unknown: the documents don't say, so it is not guessed",
        },
        "left_out": {
            "pipe_size_unknown": unknown_pipes,
            "furniture_outside_rooms": furniture_outside,
            "small_parts": dropped.small,
            "not_meshes": dropped.not_mesh,
        },
        "rules": {"source": if own_rules { "job (building spec `ar_export`) over the shipped rules" } else { "shipped rules (arexport.json)" }},
    });
    let text = serde_json::to_string_pretty(&index).map_err(|e| bad(CMD, e.to_string()))?;
    crate::writefile::write_atomic(&index_path, text.as_bytes()).map_err(|e| bad(CMD, format!("{}: {e}", index_path.display())))?;
    // The folder's manifest, so a static server serves it to the app's Update button.
    let manifest = super::manifest::run(s, &json!({"dir": dir}))?;
    let seconds = (started.elapsed().as_secs_f64() * 100.0).round() / 100.0;
    s.echo(format!("AR package set: {} building and level packages, {} room packages → {}", written.len(), room_totals.0, index_path.display()));
    Ok(json!({
        "dir": dir,
        "index": index_path.to_string_lossy(),
        "packages": written.iter().map(|w| json!({"name": w["name"], "view": w["view"], "nodes": w["nodes"], "triangles": w["triangles"], "generic": w["generic"], "simplified": w["simplified"], "bytes": {"glb": w["files"]["glb"]["bytes"], "usdz": w["files"]["usdz"]["bytes"], "sidecar": w["files"]["sidecar"]["bytes"]}})).collect::<Vec<_>>(),
        "rooms": {"packages": room_totals.0, "triangles": room_totals.1, "max_triangles": room_totals.3, "model_bytes": room_totals.2},
        "elements": all_ids.len(),
        "links": {"known": links.0, "inferred": links.1, "unknown": links.2},
        "left_out": index["left_out"],
        "manifest": {"path": manifest["path"], "packages": manifest["packages"]},
        "seconds": seconds,
    }))
}

/// An element of a package, with its menu group, whether it shows when the package opens, its
/// role and what the package adds.
fn with<'a>(it: &Item<'a>, r: &Role, extra: &[(&str, Value)]) -> Item<'a> {
    let mut x = it.clone();
    x.extra.insert("group".into(), json!(r.group));
    if !r.default_on {
        x.extra.insert("default_on".into(), json!(false));
    }
    if let Some(role) = &r.role {
        x.extra.insert("role".into(), json!(role));
    }
    for (k, v) in extra {
        x.extra.insert((*k).into(), v.clone());
    }
    x
}

/// An empty package like `p` (its fields other than the name and view).
fn pkg_like<'a>(p: &Pkg) -> Pkg<'a> {
    Pkg {
        name: String::new(),
        view: p.view,
        items: Vec::new(),
        gens: Vec::new(),
        mode: p.mode,
        generic: p.generic,
        left_out: BTreeMap::new(),
        level: None,
        room: None,
    }
}

/// True when `path` is an index this command wrote.
fn is_index(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if !meta.is_file() || meta.len() > 512 * 1024 * 1024 {
        return false;
    }
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .is_some_and(|v| v.get("schema").and_then(Value::as_str) == Some(INDEX_SCHEMA))
}

/// A building package's simplifications: the context as boxes, then every element as its box,
/// then the far elements (the context is the view's point: it stays).
const BUILDING_STEPS: [Step; 4] =
    [Step::of(Ctx::Full, false, false), Step::of(Ctx::Boxes, false, false), Step::of(Ctx::Boxes, true, false), Step::of(Ctx::Boxes, true, true)];
/// A level package's: the context as boxes, then the far elements.
const LEVEL_STEPS: [Step; 3] = [Step::of(Ctx::Full, false, false), Step::of(Ctx::Boxes, false, false), Step::of(Ctx::Boxes, false, true)];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_printed() {
        assert_eq!(size_in("24x12"), Some(24.0));
        assert_eq!(size_in("10x8"), Some(10.0));
        assert_eq!(size_in("3"), Some(3.0));
        assert_eq!(size_in("3/4\""), Some(0.75));
        assert_eq!(size_in("1-1/4\""), Some(1.25));
        assert_eq!(size_in("4\" square"), Some(4.0));
        assert_eq!(size_in(""), None);
        assert_eq!(size_in("x"), None);
    }

    #[test]
    fn words_match_whole_words_in_order() {
        let t = tokens("equipment:exhaust_fan EF-1");
        assert!(says(&t, &["exhaust fan".into()]));
        assert!(says(&t, &["ef".into()]));
        assert!(!says(&t, &["fan exhaust".into()]));
        assert!(!says(&tokens("ELEVATION"), &["elev".into()]));
    }

    #[test]
    fn a_square_plate_has_four_boundary_edges_and_one_loop() {
        let a = [0.0, 0.0, 0.0];
        let b = [1.0, 0.0, 0.0];
        let c = [1.0, 0.0, -1.0];
        let dd = [0.0, 0.0, -1.0];
        let tris = vec![up_tri(a, b, c), up_tri(a, c, dd)];
        let e = boundary(&tris);
        assert_eq!(e.len(), 4);
        let loops = chain(&e, 100);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 5, "closed: back at the start {loops:?}");
    }

    #[test]
    fn a_circuit_number_drops_its_panel() {
        assert_eq!(circuit_number("PU-1", "PU-1-5"), "5");
        assert_eq!(circuit_number("LP", "lp/12"), "12");
        assert_eq!(circuit_number("A", "7"), "7");
        assert_eq!(circuit_number("", "7"), "7");
    }

    #[test]
    fn a_bar_is_a_closed_box() {
        let t = bar([0.0, 0.0], [2.0, 0.0], 0.0, 0.1, 0.1);
        assert_eq!(t.len(), 12);
    }

    #[test]
    fn the_shipped_set_rules_read() {
        let r: Rules = serde_json::from_str(RULES).unwrap_or_default();
        assert_eq!(r.set.tri_cap(), 150_000);
        assert!(r.set.groups.iter().any(|g| g.group == "furniture" && g.default_on == Some(false)));
        assert!(r.set.sizes.fire_main_in > r.set.sizes.plumbing_min_in);
        assert_eq!(r.set.sizes.feeder_min_in, 1.0);
    }
}
