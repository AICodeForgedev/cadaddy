//! The BIM view's four views (Q6419): one is active at a time, and none of them draws every
//! object of the model. Everything stays in the model; this is only what the 3D view draws.
//!
//! - **Architectural**, the whole building: rooms, walls, windows, doors, floors, roof and
//!   stairs, solid. No MEP, framing, cabinets or furniture.
//! - **MEP**, the whole building, the view a model opens in. Its only context is the floors,
//!   one outline round the outside of the building and each elevator's hoistway. At the
//!   default density, mains and big gear (gear, house panels, feeders, cable tray, main ducts,
//!   large pipe and risers, fire mains, big mechanical equipment, elevators); Full MEP adds every
//!   duct, air terminal and exhaust fan, plumbing mains (2" and up), fire mains and cross mains,
//!   every panel with its feeder, and whatever passes through a floor. Branch circuits, boxes,
//!   devices, fixtures and sprinkler heads never draw here.
//! - **Floor**, one level: its walls, doors, windows and rooms as pale context, its panels,
//!   light fixtures and MEP, sprinkler branch lines and heads included.
//! - **Room**, one room of a level: everything in it (devices, boxes, conduit and wire,
//!   fixtures), its surroundings ghosted; furniture only with its switch on.
//!
//! What an element is for the views (main or branch, feeder or branch circuit, house or unit
//! panel) comes from the model's own data first: the feeders and panel schedules in the
//! building spec, a run's system and circuit, a pipe's role in its system; only where they
//! don't say, from its size, by the rules data (`bimviews.json`, edited per project with
//! `bim.view.rules`). The active view's allowed set is applied after every switch and tree
//! branch the user turned on ([`super::bimlook::display`]), so no setting gets past it, and
//! the 3D view never builds or uploads what the view leaves out.
//!
//! - `bim.view {view?, full?, level?, room?, up?, enter?, switches?, follow?}`: the active view.
//! - `bim.view.rules {rules?, reset?}`: the project's rules.
//! - `bim.view.report {width?, height?}`: what each view draws on this model, measured.
//!
//! Counts, the takeoff, queries, clashes, exports and plots read the drawing, never a view.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Space};
use cadaddy_geom::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::bimlook::{self, Class, Look};
use super::*;
use crate::{Result, Session};

/// The built-in rules.
const RULES: &str = include_str!("bimviews.json");
/// The key of the project's rules in the building spec.
const KEY: &str = "bim_views";
/// Most objects looked at.
const MAX_SCAN: usize = 2_000_000;
/// Most levels and rooms listed, and hoistways read.
const MAX_LIST: usize = 2_000;

const CMD: &str = "bim.view";
const RULES_CMD: &str = "bim.view.rules";
const REPORT: &str = "bim.view.report";

/// The four views.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum View {
    Architectural,
    #[default]
    Mep,
    Floor,
    Room,
}

impl View {
    pub const ALL: [View; 4] = [View::Architectural, View::Mep, View::Floor, View::Room];

    pub fn name(self) -> &'static str {
        match self {
            View::Architectural => "architectural",
            View::Mep => "mep",
            View::Floor => "floor",
            View::Room => "room",
        }
    }

    /// Its name on the view's control.
    pub fn label(self) -> &'static str {
        match self {
            View::Architectural => "Architectural",
            View::Mep => "MEP",
            View::Floor => "Floor",
            View::Room => "Room",
        }
    }

    /// `architectural`, `mep`, `floor` (`level`) or `room` (any case; `a`, `m`, `f`, `r`).
    pub fn parse(s: &str) -> Option<View> {
        match s.trim().to_ascii_lowercase().as_str() {
            "architectural" | "architecture" | "arch" | "a" => Some(View::Architectural),
            "mep" | "m" => Some(View::Mep),
            "floor" | "level" | "f" => Some(View::Floor),
            "room" | "r" => Some(View::Room),
            _ => None,
        }
    }

    /// Whether it shows one level (Floor, Room), not the whole building.
    pub fn per_level(self) -> bool {
        matches!(self, View::Floor | View::Room)
    }
}

/// The switches the views offer: the trades (M, E, P), cabinets (Floor and Room) and
/// furniture (Room only, off by default).
pub const SWITCHES: [&str; 5] = ["m", "e", "p", "cabinets", "furniture"];

/// The active view as `look` keeps it (none saved: MEP).
pub fn active(look: &Look) -> View {
    look.view.as_deref().and_then(View::parse).unwrap_or_default()
}

/// Whether switch `k` is on in `look`.
pub fn switch_on(look: &Look, k: &str) -> bool {
    if k == "furniture" { look.furniture } else { !look.switched_off.contains(k) }
}

// ─── Rules ───────────────────────────────────────────────────────────────────

/// The rules data: what each view draws.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub version: u32,
    pub source: String,
    pub default_view: String,
    pub architectural_parts: Vec<String>,
    pub context_parts: Vec<String>,
    pub level_context_parts: Vec<String>,
    pub cabinet_parts: Vec<String>,
    pub cabinet_words: Vec<String>,
    pub furniture_parts: Vec<String>,
    pub furniture_words: Vec<String>,
    pub gear_words: Vec<String>,
    pub panel_words: Vec<String>,
    pub big_equipment_words: Vec<String>,
    pub big_equipment_min_extent_ft: f64,
    pub exhaust_fan_words: Vec<String>,
    pub elevator_words: Vec<String>,
    pub disconnect_words: Vec<String>,
    /// A panel placed this many times or more is a typical unit panel (one per unit).
    pub unit_panel_copies: usize,
    pub main_duct_min_in: f64,
    pub large_pipe_min_in: f64,
    pub plumbing_main_min_in: f64,
    pub fire_main_min_in: f64,
    pub feeder_min_in: f64,
    pub fire_main_words: Vec<String>,
    pub fire_branch_words: Vec<String>,
    pub raceway_atoms: Vec<String>,
    pub floor_tolerance_ft: f64,
    pub hoistway_pad_ft: f64,
    pub max_follow: usize,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            version: 1,
            source: String::new(),
            default_view: "mep".into(),
            architectural_parts: Vec::new(),
            context_parts: vec!["floor".into()],
            level_context_parts: Vec::new(),
            cabinet_parts: Vec::new(),
            cabinet_words: Vec::new(),
            furniture_parts: Vec::new(),
            furniture_words: Vec::new(),
            gear_words: Vec::new(),
            panel_words: Vec::new(),
            big_equipment_words: Vec::new(),
            big_equipment_min_extent_ft: 4.0,
            exhaust_fan_words: Vec::new(),
            elevator_words: Vec::new(),
            disconnect_words: Vec::new(),
            unit_panel_copies: 3,
            main_duct_min_in: 18.0,
            large_pipe_min_in: 3.0,
            plumbing_main_min_in: 2.0,
            fire_main_min_in: 2.5,
            feeder_min_in: 1.0,
            fire_main_words: Vec::new(),
            fire_branch_words: Vec::new(),
            raceway_atoms: Vec::new(),
            floor_tolerance_ft: 0.25,
            hoistway_pad_ft: 1.0,
            max_follow: 20_000,
        }
    }
}

impl Rules {
    /// The built-in rules.
    pub fn defaults() -> Rules {
        serde_json::from_str::<Rules>(RULES).unwrap_or_default()
    }

    /// The rules for `d`: the project's (in its building spec, over the built-in ones key by
    /// key), and whether it has its own.
    pub fn of(d: &Drawing) -> (Rules, bool) {
        let Some(own) = project(d) else { return (Rules::defaults(), false) };
        let mut v = serde_json::to_value(Rules::defaults()).unwrap_or(Value::Null);
        if let (Some(o), Some(own)) = (v.as_object_mut(), own.as_object()) {
            for (k, x) in own {
                if o.contains_key(k) {
                    o.insert(k.clone(), x.clone());
                }
            }
            o.insert("source".into(), json!("project"));
        }
        (serde_json::from_value(v).unwrap_or_else(|_| Rules::defaults()), true)
    }

    fn feet(&self, v: f64, d: f64) -> f64 {
        if v.is_finite() && v >= 0.0 { v } else { d }
    }
}

/// The project's rules object in the building spec.
fn project(d: &Drawing) -> Option<Value> {
    d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|v| v.get(KEY).cloned()).filter(Value::is_object)
}

fn has_any(text: &str, words: &[String]) -> bool {
    words.iter().any(|w| super::fidelity::has_word(text, w))
}

fn part_in(part: &str, list: &[String]) -> bool {
    list.iter().any(|p| p.trim().eq_ignore_ascii_case(part))
}

// ─── What an element is for the views ────────────────────────────────────────

/// A panel's place in the distribution: a house panel (fed from gear, or serving common areas
/// or several rooms) or a unit panel (serving one dwelling or guest room).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelRole {
    House,
    Unit,
}

/// What a built element is to the views.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum What {
    /// Walls, doors, windows, rooms, ceilings, stairs, roof (not floors).
    Arch,
    Floor,
    Structure,
    Site,
    /// A trade's piece inside walls, floors and the roof (studs, sheets, pours, finishes).
    Piece,
    Cabinet,
    Furniture,
    Gear,
    Panel(PanelRole),
    BigEquipment,
    Equipment,
    ExhaustFan,
    ElevatorEquipment,
    ElevatorDisconnect,
    /// Raceway or cable of a feeder; `unit`: one that reaches only unit panels.
    Feeder {
        unit: bool,
    },
    Tray,
    /// Raceway or cable of a branch circuit (or low-voltage pathway, grounding).
    Branch,
    Wire,
    /// Fittings, supports, fasteners, breakers.
    Fitting,
    Box,
    Duct {
        main: bool,
    },
    Terminal,
    Pipe {
        size: Option<f64>,
    },
    /// Fire protection pipe: `main` from its role in the system (none: unknown).
    FirePipe {
        main: Option<bool>,
        size: Option<f64>,
    },
    Head,
    Device,
    Light,
    Fixture,
    Other,
}

impl What {
    /// The trade switch that hides it (`m`, `e`, `p`), if any. Fire protection goes with M.
    pub fn trade(self) -> Option<&'static str> {
        match self {
            What::Duct { .. }
            | What::Terminal
            | What::ExhaustFan
            | What::BigEquipment
            | What::Equipment
            | What::FirePipe { .. }
            | What::Head
            | What::ElevatorEquipment => Some("m"),
            What::Gear
            | What::Panel(_)
            | What::Feeder { .. }
            | What::Tray
            | What::Branch
            | What::Wire
            | What::Fitting
            | What::Box
            | What::Device
            | What::Light
            | What::ElevatorDisconnect => Some("e"),
            What::Pipe { .. } | What::Fixture => Some("p"),
            _ => None,
        }
    }

    /// A duct, pipe, conduit or cable: a run that may pass through a floor.
    fn is_run(self) -> bool {
        matches!(self, What::Duct { .. } | What::Pipe { .. } | What::FirePipe { .. } | What::Feeder { .. } | What::Branch | What::Tray)
    }
}

/// A room of the Room view: its outlines on its level (drawing coordinates).
#[derive(Clone, Debug, Default)]
struct RoomArea {
    level: String,
    name: String,
    outlines: Vec<Vec<Vec2>>,
}

impl RoomArea {
    fn holds(&self, p: Vec2) -> bool {
        self.outlines.iter().any(|o| cadaddy_geom::point_in_polygon(o, p))
    }
}

/// An elevator hoistway: its plan box on each level's plan, its height, where it stands in the
/// stacked view.
#[derive(Clone, Debug)]
struct Hoist {
    name: String,
    /// Level (upper case) → plan box on that level's plan.
    rects: Vec<(String, Vec2, Vec2)>,
    /// The box in the stacked view (the first level's), bottom and top (drawing units).
    shown: (Vec2, Vec2),
    z0: f64,
    z1: f64,
}

/// What does not draw that the view would: what the view says it left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LeftOut {
    /// Pipes with no size: not assumed.
    pub pipe_unknown_size: usize,
    /// Fire protection pipe of unknown role and size.
    pub fire_pipe_unknown: usize,
    /// Panels with no distribution data saying house or unit: drawn as house panels.
    pub panel_role_unknown: usize,
}

/// One view of one drawing, worked out once: the rules, the active view and its switches, the
/// levels' floors, the hoistways, the feeders' ends and the panels' roles.
pub struct Scope {
    pub view: View,
    pub full: bool,
    pub level: Option<String>,
    pub room: Option<String>,
    pub rules: Rules,
    switches: BTreeMap<&'static str, bool>,
    upf: f64,
    /// Level (upper case) → its floor's elevation (drawing units).
    floors: HashMap<String, f64>,
    floor_z: Vec<f64>,
    hoists: Vec<Hoist>,
    room_area: Option<RoomArea>,
    /// A feeder run's base (`mepid` before the piece) → the gear it reaches.
    feeder_ends: HashMap<String, Vec<String>>,
    /// Feeder tag (upper case) → the gear it reaches.
    feeder_to: HashMap<String, Vec<String>>,
    /// Panel tag (upper case) → its role, and whether the data said it.
    panels: HashMap<String, (PanelRole, bool)>,
    /// The followed system's elements (none: not following).
    pub follow: Option<HashSet<Handle>>,
    /// What the view left out, counted as it was asked.
    left: Cell<LeftOut>,
}

/// A run's base id (its `mepid` before the piece).
fn run_base_of(e: &Entity) -> Option<&str> {
    bimlook::made_tag(e, "mepid").and_then(|id| id.rsplit_once('/')).map(|(b, _)| b)
}

/// A mesh's corners' box.
fn mesh_box(e: &Entity) -> Option<(Vec3, Vec3)> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for v in m.vertices.iter().filter(|v| v.is_finite()) {
        lo = Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
        hi = Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
    }
    (lo.x <= hi.x).then_some((lo, hi))
}

/// Where an element stands on its level's plan: its box's middle (a mesh), its insertion point.
fn plan_point(d: &Drawing, e: &Entity) -> Option<Vec2> {
    if let Some((lo, hi)) = mesh_box(e) {
        return Some(Vec2::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0));
    }
    let b = cadaddy_doc::entity_bounds(d, e, 0);
    (!b.is_empty()).then(|| b.center())
}

/// The circuit an element is on (`LP1-3`) and its panel, from its properties.
fn circuit_of(e: &Entity) -> (Option<String>, Option<String>) {
    let ps = cadaddy_doc::props::read(&e.common.xdata);
    let get = |k: &str| ps.get(k).map(|p| p.text()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let circuit = get("circuit").or_else(|| bimlook::made_tag(e, "circuit").map(str::to_string).filter(|t| !t.is_empty()));
    let panel = get("panel").or_else(|| circuit.as_deref().and_then(|c| c.rsplit_once('-')).map(|(p, _)| p.trim().to_string()));
    (circuit, panel)
}

/// The words of an element the rules read: kind, category, type, name, tag, system.
fn text_of(e: &Entity, c: &Class) -> String {
    let mut t = [
        Some(c.category.as_str()),
        Some(c.ty.as_str()),
        bimlook::made_name(e),
        bimlook::made_tag(e, "tag"),
        bimlook::made_tag(e, "kind"),
        bimlook::made_tag(e, "part"),
        bimlook::made_tag(e, "system"),
        bimlook::made_tag(e, "title"),
        bimlook::made_tag(e, "type"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<&str>>()
    .join(" ")
    .replace([':', '_'], " ");
    t.make_ascii_lowercase();
    t
}

impl Scope {
    /// The view `look` has for `d`.
    pub fn of(d: &Drawing, look: &Look) -> Scope {
        let rules = Rules::of(d).0;
        let view = active(look);
        let upf = super::elemprops::units_per_foot(d);
        let sp = d.building_spec.as_deref().filter(|t| !t.trim().is_empty()).and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok());
        let mut floors = HashMap::new();
        let mut floor_z = Vec::new();
        let mut hoists = Vec::new();
        let mut feeder_ends: HashMap<String, Vec<String>> = HashMap::new();
        let mut feeder_to: HashMap<String, Vec<String>> = HashMap::new();
        let mut panels = HashMap::new();
        if let Some(sp) = &sp {
            for l in sp.levels.iter().take(MAX_LIST).filter(|l| l.elevation_ft.is_finite()) {
                floors.insert(l.name.to_ascii_uppercase(), l.elevation_ft * upf);
                floor_z.push(l.elevation_ft * upf);
            }
            hoists = hoistways(d, sp, upf);
            for f in sp.mep.feeders.iter().take(MAX_SCAN) {
                let to: Vec<String> = f.to.split([',', ';', '&']).map(|t| t.trim().to_ascii_uppercase()).filter(|t| !t.is_empty()).collect();
                feeder_to.entry(f.tag.trim().to_ascii_uppercase()).or_default().extend(to);
            }
            for r in sp.mep.runs.iter().filter(|r| super::ugplan::is_feeder_run(r)).take(MAX_SCAN) {
                // `feeder TAG FROM→TO, TO`: the gear it reaches.
                let to: Vec<String> = r
                    .source_sheet
                    .split_once('→')
                    .map(|(_, t)| t.split([',', ';', '&']).map(|x| x.trim().to_ascii_uppercase()).filter(|x| !x.is_empty()).collect())
                    .unwrap_or_default();
                feeder_ends.entry(super::mep::run_base(r)).or_default().extend(to);
            }
            panels = panel_roles(sp, &rules);
        }
        floor_z.sort_by(f64::total_cmp);
        floor_z.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let per = view.per_level();
        let level = if per { look.level.clone().or_else(|| first_level(d, sp.as_ref())) } else { None };
        let room = if view == View::Room { look.room.clone() } else { None };
        let room_area = match (&level, &room) {
            (Some(l), Some(r)) => Some(room_area(d, l, r)),
            _ => None,
        };
        let mut switches = BTreeMap::new();
        for k in SWITCHES {
            switches.insert(k, switch_on(look, k));
        }
        let mut s = Scope {
            view,
            full: look.full_mep,
            level,
            room,
            rules,
            switches,
            upf,
            floors,
            floor_z,
            hoists,
            room_area,
            feeder_ends,
            feeder_to,
            panels,
            follow: None,
            left: Cell::new(LeftOut::default()),
        };
        if let Some(h) = look.follow.as_deref().and_then(Handle::parse_hex)
            && d.model.get(h).is_some()
        {
            let f = follow(d, sp.as_ref(), &s, h);
            s.follow = Some(f);
        }
        s
    }

    /// What the view left out that it would have drawn, counted since it was made.
    pub fn left_out(&self) -> LeftOut {
        self.left.take()
    }

    fn count(&self, f: impl FnOnce(&mut LeftOut)) {
        let mut l = self.left.take();
        f(&mut l);
        self.left.set(l);
    }

    fn on(&self, k: &str) -> bool {
        self.switches.get(k).copied().unwrap_or(true)
    }

    fn panel_role(&self, e: &Entity) -> PanelRole {
        let tag = bimlook::made_tag(e, "tag").map(|t| t.trim().to_ascii_uppercase()).unwrap_or_default();
        match self.panels.get(&tag) {
            Some((r, true)) => *r,
            Some((r, false)) => {
                self.count(|l| l.panel_role_unknown += 1);
                *r
            }
            None => {
                self.count(|l| l.panel_role_unknown += 1);
                PanelRole::House
            }
        }
    }

    /// Whether the gear `tags` names are all unit panels.
    fn all_units(&self, tags: &[String]) -> bool {
        !tags.is_empty() && tags.iter().all(|t| matches!(self.panels.get(t), Some((PanelRole::Unit, _))))
    }

    /// What `e` (of class `c`) is to the views.
    pub fn what(&self, d: &Drawing, e: &Entity, c: &Class) -> What {
        let r = &self.rules;
        let part = super::wallgen::made_as(e).unwrap_or_default().to_ascii_lowercase();
        let text = text_of(e, c);
        let size = bimlook::made_tag(e, "size").and_then(super::fidelity::size_inches);
        if part == "mep" {
            let kind = bimlook::made_tag(e, "kind").unwrap_or("").to_ascii_lowercase();
            let mpart = bimlook::made_tag(e, "part").unwrap_or("").to_ascii_lowercase();
            if let Some(piece) = mpart.strip_prefix("atom:") {
                return match piece {
                    "wire" => What::Wire,
                    "box" | "ring" => What::Box,
                    p if part_in(p, &r.raceway_atoms) => self.raceway(e, size),
                    _ => What::Fitting,
                };
            }
            if let Some(_term) = mpart.strip_prefix("terminal:") {
                return match c.key {
                    "terminals" => What::Terminal,
                    "fire" => What::Head,
                    "fixtures" => What::Fixture,
                    _ => What::Device,
                };
            }
            if mpart.starts_with("equipment") || kind == "equipment" {
                return self.equipment(d, e, &text);
            }
            return match kind.as_str() {
                "duct" => What::Duct { main: size.is_some_and(|z| z >= r.main_duct_min_in) },
                "pipe" if c.key == "fire" => What::FirePipe { main: fire_role(&text, r), size },
                "pipe" => What::Pipe { size },
                "tray" => What::Tray,
                "conduit" | "cable" => self.raceway(e, size),
                "grounding" => What::Branch,
                "breaker" => What::Fitting,
                _ => self.equipment(d, e, &text),
            };
        }
        if part == "atom" {
            return What::Piece;
        }
        if part_in(&part, &r.furniture_parts) {
            return What::Furniture;
        }
        if part_in(&part, &r.cabinet_parts) {
            return What::Cabinet;
        }
        if part_in(&part, &r.context_parts) {
            return What::Floor;
        }
        if part_in(&part, &r.architectural_parts) {
            return What::Arch;
        }
        match part.as_str() {
            "column" | "beam" | "footing" | "brace" => return What::Structure,
            "ground" | "site" => return What::Site,
            "light" => return What::Light,
            "diffuser" => return What::Terminal,
            "equipment" => return self.equipment(d, e, &text),
            _ => {}
        }
        match c.key {
            "lights" => What::Light,
            "fixtures" => What::Fixture,
            "fire" => What::Head,
            "terminals" => What::Terminal,
            "devices" => {
                if has_any(&text, &r.disconnect_words) && (has_any(&text, &r.elevator_words) || self.in_hoistway(d, e, c, 10.0)) {
                    What::ElevatorDisconnect
                } else {
                    What::Device
                }
            }
            "equipment" => self.equipment(d, e, &text),
            "structure" => What::Structure,
            "site" => What::Site,
            _ if has_any(&text, &r.furniture_words) => What::Furniture,
            _ if has_any(&text, &r.cabinet_words) => What::Cabinet,
            _ => What::Other,
        }
    }

    /// A piece of equipment: elevator gear, a panel, switchgear, a fan, big or small.
    fn equipment(&self, d: &Drawing, e: &Entity, text: &str) -> What {
        let r = &self.rules;
        let elevator = has_any(text, &r.elevator_words);
        if has_any(text, &r.disconnect_words) {
            return if elevator { What::ElevatorDisconnect } else { What::Device };
        }
        if elevator {
            return What::ElevatorEquipment;
        }
        if has_any(text, &r.panel_words) {
            return What::Panel(self.panel_role(e));
        }
        if has_any(text, &r.gear_words) {
            return What::Gear;
        }
        if has_any(text, &r.exhaust_fan_words) {
            return What::ExhaustFan;
        }
        let extent = mesh_box(e).map(|(lo, hi)| {
            let s = hi - lo;
            s.x.max(s.y).max(s.z) / self.upf
        });
        let _ = d;
        if has_any(text, &r.big_equipment_words) || extent.is_some_and(|x| x >= self.rules.feet(r.big_equipment_min_extent_ft, 4.0)) {
            What::BigEquipment
        } else {
            What::Equipment
        }
    }

    /// Raceway or cable: a feeder's (by the distribution data: what it reaches) or a branch
    /// circuit's; unknown, by its size.
    fn raceway(&self, e: &Entity, size: Option<f64>) -> What {
        let system = bimlook::made_tag(e, "system").unwrap_or("").trim().to_string();
        let ps = cadaddy_doc::props::read(&e.common.xdata);
        let prop_system = ps.get("system").map(|p| p.text()).unwrap_or_default();
        let feeder_sys = |s: &str| s.eq_ignore_ascii_case(super::gearbuild::FEEDER_SYSTEM);
        let base = run_base_of(e).or_else(|| ps.get("run").map(|_| "")).unwrap_or("");
        let ends = self.feeder_ends.get(base);
        if feeder_sys(&system) || feeder_sys(&prop_system) || ends.is_some() {
            return What::Feeder { unit: ends.is_some_and(|t| self.all_units(t)) };
        }
        if let Some(to) = self.feeder_to.get(&system.to_ascii_uppercase()) {
            return What::Feeder { unit: self.all_units(to) };
        }
        let branch_sys = |s: &str| {
            s.eq_ignore_ascii_case(super::conduitroute::BRANCH_SYSTEM)
                || s.eq_ignore_ascii_case(super::conduitroute::lv::LV_SYSTEM)
                || s.to_ascii_lowercase().starts_with(super::printcond::CIRCUIT_TAG)
                || s.eq_ignore_ascii_case(super::ugplan::E_UG)
        };
        if branch_sys(&system) || branch_sys(&prop_system) || ps.get("circuit").is_some_and(|p| !p.text().trim().is_empty()) {
            return What::Branch;
        }
        if size.is_some_and(|z| z >= self.rules.feeder_min_in) { What::Feeder { unit: false } } else { What::Branch }
    }

    /// Whether `e` passes through a floor: its box spans a level's floor, or, standing upright,
    /// it reaches a floor other than its own's (a riser cut at the floors). From its geometry
    /// against the levels' elevations.
    fn crosses(&self, e: &Entity, c: &Class) -> bool {
        let Some((lo, hi)) = mesh_box(e) else { return false };
        let tol = self.rules.feet(self.rules.floor_tolerance_ft, 0.25) * self.upf;
        let own = self.floors.get(&c.level.to_ascii_uppercase()).copied();
        let s = hi - lo;
        let upright = s.z > 2.0 * s.x.max(s.y);
        self.floor_z.iter().any(|&z| {
            (lo.z < z - tol && hi.z > z + tol)
                || (upright && own.is_none_or(|o| (z - o).abs() > tol) && ((hi.z - z).abs() <= tol || (lo.z - z).abs() <= tol))
        })
    }

    /// Whether `e` stands in an elevator's hoistway on its level's plan (`pad_ft` round it).
    fn in_hoistway(&self, d: &Drawing, e: &Entity, c: &Class, pad_ft: f64) -> bool {
        if self.hoists.is_empty() {
            return false;
        }
        let Some(p) = plan_point(d, e) else { return false };
        let p = p - super::bspec::stacked_by(e);
        let pad = pad_ft.max(0.0) * self.upf;
        let lv = c.level.to_ascii_uppercase();
        self.hoists
            .iter()
            .flat_map(|h| h.rects.iter())
            .any(|(l, a, b)| (l == &lv || l.is_empty()) && p.x >= a.x - pad && p.x <= b.x + pad && p.y >= a.y - pad && p.y <= b.y + pad)
    }

    fn on_level(&self, c: &Class) -> bool {
        self.level.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(&c.level))
    }

    fn in_room(&self, d: &Drawing, e: &Entity, c: &Class) -> bool {
        let Some(r) = &self.room_area else { return false };
        if !r.level.eq_ignore_ascii_case(&c.level) {
            return false;
        }
        if super::wallgen::made_as(e).as_deref() == Some("room") {
            let name = super::bspec::room_tag(e, "number:").filter(|n| !n.is_empty()).or_else(|| super::bspec::room_tag(e, "name:"));
            return name.is_some_and(|n| n.eq_ignore_ascii_case(&r.name));
        }
        let by_prop = cadaddy_doc::props::read(&e.common.xdata).get("room").map(|p| p.text()).filter(|t| !t.trim().is_empty());
        if let Some(t) = by_prop
            && t.trim().eq_ignore_ascii_case(&r.name)
        {
            return true;
        }
        plan_point(d, e).is_some_and(|p| r.holds(p))
    }

    /// How the view draws `e` (of class `c`): solid, ghosted, or not at all.
    pub fn shows(&self, d: &Drawing, e: &Entity, c: &Class) -> Shown {
        if let Some(f) = &self.follow
            && f.contains(&e.handle)
        {
            return Shown::Solid;
        }
        let w = self.what(d, e, c);
        let s = self.view_shows(d, e, c, w);
        if s == Shown::No {
            return s;
        }
        if let Some(t) = w.trade()
            && !self.on(t)
        {
            return Shown::No;
        }
        if self.follow.is_some() {
            return Shown::Ghost;
        }
        s
    }

    fn view_shows(&self, d: &Drawing, e: &Entity, c: &Class, w: What) -> Shown {
        let r = &self.rules;
        let yes = |b: bool| if b { Shown::Solid } else { Shown::No };
        match self.view {
            View::Architectural => yes(matches!(w, What::Arch | What::Floor)),
            View::Mep => {
                let full = self.full;
                let crossing = || w.is_run() && self.crosses(e, c);
                // The branches inside or serving an elevator never draw, even through floors.
                let elevator_branch = || {
                    matches!(w, What::Branch | What::Wire | What::Device | What::Light | What::Box) && self.in_hoistway(d, e, c, r.hoistway_pad_ft)
                };
                let s = match w {
                    What::Floor | What::Gear | What::BigEquipment | What::ElevatorEquipment | What::ElevatorDisconnect | What::Tray => true,
                    What::Panel(PanelRole::House) => true,
                    What::Panel(PanelRole::Unit) | What::Equipment | What::Terminal | What::ExhaustFan => full,
                    What::Feeder { unit } => !unit || full,
                    What::Duct { main } => main || full,
                    What::Pipe { size: Some(z) } => {
                        z >= r.large_pipe_min_in
                            || (z >= r.plumbing_main_min_in && crossing())
                            || (full && (z >= r.plumbing_main_min_in || crossing()))
                    }
                    What::Pipe { size: None } => {
                        let x = full && crossing();
                        if !x {
                            self.count(|l| l.pipe_unknown_size += 1);
                        }
                        x
                    }
                    What::FirePipe { main, size } => {
                        let main = main.or_else(|| size.map(|z| z >= r.fire_main_min_in));
                        match main {
                            Some(true) => true,
                            _ if full && crossing() => true,
                            Some(false) => false,
                            None => {
                                self.count(|l| l.fire_pipe_unknown += 1);
                                false
                            }
                        }
                    }
                    What::Branch => full && crossing() && !elevator_branch(),
                    _ => false,
                };
                yes(s)
            }
            View::Floor => {
                if !self.on_level(c) {
                    return Shown::No;
                }
                let s = match w {
                    What::Arch | What::Floor | What::Structure => true,
                    What::Cabinet => self.on("cabinets"),
                    What::Gear
                    | What::Panel(_)
                    | What::BigEquipment
                    | What::Equipment
                    | What::ExhaustFan
                    | What::ElevatorEquipment
                    | What::ElevatorDisconnect
                    | What::Feeder { .. }
                    | What::Tray
                    | What::Duct { .. }
                    | What::Terminal
                    | What::Pipe { .. }
                    | What::FirePipe { .. }
                    | What::Head
                    | What::Light => true,
                    _ => false,
                };
                yes(s)
            }
            View::Room => {
                if !self.on_level(c) {
                    return Shown::No;
                }
                if self.in_room(d, e, c) {
                    return match w {
                        What::Furniture => yes(self.on("furniture")),
                        What::Cabinet => yes(self.on("cabinets")),
                        What::Site => Shown::No,
                        _ => Shown::Solid,
                    };
                }
                let part = super::wallgen::made_as(e).unwrap_or_default();
                if matches!(w, What::Arch | What::Floor | What::Structure) && part_in(&part, &r.level_context_parts) {
                    Shown::Ghost
                } else {
                    Shown::No
                }
            }
        }
    }
}

/// How a view draws an element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    No,
    Solid,
    /// See-through: a Room view's surroundings, what a followed system doesn't hold.
    Ghost,
}

/// A pipe's role in its fire protection system from its system's name and type: a main
/// (`MAIN`, `CROSS MAIN`, `RISER`, `STANDPIPE`) or a branch line; none when they don't say.
fn fire_role(text: &str, r: &Rules) -> Option<bool> {
    if has_any(text, &r.fire_branch_words) {
        Some(false)
    } else if has_any(text, &r.fire_main_words) {
        Some(true)
    } else {
        None
    }
}

/// The panels' roles from the distribution data: the rooms their circuits serve (one: a unit
/// panel; several: a house panel), what feeds them (gear: a house panel), and how often a
/// panel is placed (a typical panel in every unit). Panel tag (upper case) → (role, known).
pub fn panel_roles(sp: &cadaddy_walk3d::BuildingSpec, r: &Rules) -> HashMap<String, (PanelRole, bool)> {
    let is_panel = |e: &cadaddy_walk3d::MepEquipment| {
        let t = format!("{} {}", e.kind, e.title).replace('_', " ").to_ascii_lowercase();
        has_any(&t, &r.panel_words)
    };
    // Each list read once: the gear (not panels) by tag, what feeds each tag, how often each
    // panel is placed.
    let gear: HashSet<String> = sp
        .mep
        .equipment
        .iter()
        .filter(|e| {
            let t = format!("{} {} {}", e.kind, e.title, e.tag).replace('_', " ").to_ascii_lowercase();
            !has_any(&t, &r.panel_words) && has_any(&t, &r.gear_words)
        })
        .map(|e| e.tag.trim().to_ascii_uppercase())
        .collect();
    let mut fed_by: HashMap<String, Vec<String>> = HashMap::new();
    for f in &sp.mep.feeders {
        for to in f.to.split([',', ';', '&']).map(|t| t.trim().to_ascii_uppercase()).filter(|t| !t.is_empty()) {
            fed_by.entry(to).or_default().push(f.from.trim().to_ascii_uppercase());
        }
    }
    let mut copies: HashMap<String, usize> = HashMap::new();
    for e in sp.mep.equipment.iter().filter(|e| is_panel(e)) {
        *copies.entry(e.tag.trim().to_ascii_uppercase()).or_default() += 1;
    }
    let mut out = HashMap::new();
    for e in sp.mep.equipment.iter().filter(|e| is_panel(e)) {
        let tag = e.tag.trim().to_ascii_uppercase();
        let rooms: BTreeSet<String> = e.circuits.iter().map(|c| c.room.trim().to_ascii_uppercase()).filter(|r| !r.is_empty()).collect();
        let fed_from_gear = fed_by.get(&tag).is_some_and(|from| from.iter().any(|f| gear.contains(f)));
        let role = if rooms.len() == 1 {
            (PanelRole::Unit, true)
        } else if rooms.len() > 1 || fed_from_gear {
            (PanelRole::House, true)
        } else if copies.get(&tag).copied().unwrap_or(0) >= r.unit_panel_copies.max(2) {
            (PanelRole::Unit, true)
        } else {
            (PanelRole::House, false)
        };
        // A typical tag placed in several units keeps the first answer that knew.
        match out.get(&tag) {
            Some((_, true)) => {}
            _ => {
                out.insert(tag, role);
            }
        }
    }
    out
}

/// The levels' plan corners (each level's plan has its own place on its sheet).
fn corner(sp: &cadaddy_walk3d::BuildingSpec, level: &str) -> Vec2 {
    sp.levels.iter().find(|l| l.name.eq_ignore_ascii_case(level)).and_then(|l| l.region).map_or(Vec2::ZERO, |r| Vec2::new(r[0], r[1]))
}

/// The spec's elevator hoistways: each level's plan box, and its box in the stacked view from
/// its pit to the top of its highest level.
fn hoistways(d: &Drawing, sp: &cadaddy_walk3d::BuildingSpec, upf: f64) -> Vec<Hoist> {
    let offsets = bimlook::stack_offsets(d);
    let offset = |l: &str| offsets.iter().find(|(n, _)| n.eq_ignore_ascii_case(l)).map_or(Vec2::ZERO, |o| o.1);
    let mut out = Vec::new();
    for h in sp.elevators.iter().take(MAX_LIST) {
        if !h.min.iter().chain(&h.max).all(|v| v.is_finite()) {
            continue;
        }
        let names: Vec<&cadaddy_walk3d::Level> = if h.levels.is_empty() {
            sp.levels.iter().take(MAX_LIST).collect()
        } else {
            sp.levels.iter().filter(|l| h.levels.iter().any(|n| n.eq_ignore_ascii_case(&l.name))).take(MAX_LIST).collect()
        };
        let Some(first) = h.levels.first().map(String::as_str).or_else(|| names.first().map(|l| l.name.as_str())) else { continue };
        let (a, b) = (Vec2::new(h.min[0].min(h.max[0]), h.min[1].min(h.max[1])), Vec2::new(h.min[0].max(h.max[0]), h.min[1].max(h.max[1])));
        let base = corner(sp, first);
        let rects = names
            .iter()
            .map(|l| {
                let o = corner(sp, &l.name) - base;
                (l.name.to_ascii_uppercase(), a + o, b + o)
            })
            .collect();
        let z0 = names.iter().map(|l| l.elevation_ft).filter(|z| z.is_finite()).fold(f64::INFINITY, f64::min) - h.pit_ft.unwrap_or(0.0).max(0.0);
        let z1 = names
            .iter()
            .map(|l| l.elevation_ft + if l.height_ft > 0.0 { l.height_ft } else { 10.0 })
            .filter(|z| z.is_finite())
            .fold(f64::NEG_INFINITY, f64::max);
        if !(z0.is_finite() && z1.is_finite() && z1 > z0) {
            continue;
        }
        let o = offset(first);
        out.push(Hoist { name: h.name.clone(), rects, shown: (a + o, b + o), z0: z0 * upf, z1: z1 * upf });
    }
    out
}

/// The level a per-level view shows when none was picked: the lowest.
fn first_level(d: &Drawing, sp: Option<&cadaddy_walk3d::BuildingSpec>) -> Option<String> {
    if let Some(l) =
        sp.and_then(|sp| sp.levels.iter().filter(|l| l.elevation_ft.is_finite()).min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)))
    {
        return Some(l.name.clone());
    }
    d.model.iter().take(MAX_SCAN).find_map(|e| super::wallgen::made_as(e).and(super::bspec::made_level(e))).filter(|l| !l.is_empty())
}

/// The room named `room` (its number or name) on `level`: its outlines.
fn room_area(d: &Drawing, level: &str, room: &str) -> RoomArea {
    let mut out = RoomArea { level: level.to_string(), name: room.trim().to_string(), outlines: Vec::new() };
    for r in super::bspec::room_elements(d) {
        if !r.level.eq_ignore_ascii_case(level) {
            continue;
        }
        let named =
            r.number.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(room.trim())) || r.name.trim().eq_ignore_ascii_case(room.trim());
        if named && r.outline.len() >= 3 {
            out.outlines.push(r.outline);
        }
    }
    out
}

/// The rooms of `level`: number (else name) and name.
pub fn rooms_on(d: &Drawing, level: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = super::bspec::room_elements(d)
        .into_iter()
        .filter(|r| r.level.eq_ignore_ascii_case(level))
        .map(|r| (r.number.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| r.name.clone()), r.name))
        .collect();
    out.sort();
    out.dedup();
    out.truncate(MAX_LIST);
    out
}

/// The model's levels, lowest first.
pub fn levels_of(d: &Drawing) -> Vec<String> {
    let sp = d.building_spec.as_deref().filter(|t| !t.trim().is_empty()).and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok());
    let mut named: Vec<(f64, String)> = sp
        .map(|sp| sp.levels.iter().take(MAX_LIST).map(|l| (if l.elevation_ft.is_finite() { l.elevation_ft } else { 0.0 }, l.name.clone())).collect())
        .unwrap_or_default();
    let mut seen: BTreeSet<String> = named.iter().map(|(_, n)| n.to_ascii_uppercase()).collect();
    for e in d.model.iter().take(MAX_SCAN) {
        if super::wallgen::made_as(e).is_none() {
            continue;
        }
        if let Some(l) = super::bspec::made_level(e).filter(|l| !l.is_empty())
            && seen.insert(l.to_ascii_uppercase())
        {
            named.push((f64::INFINITY, l));
            if named.len() >= MAX_LIST {
                break;
            }
        }
    }
    named.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    named.into_iter().map(|x| x.1).collect()
}

// ─── Follow the system ───────────────────────────────────────────────────────

/// Levels an element of the followed system may stand on (none: any). A typical piece of gear
/// (one tag placed on several levels, a unit panel in every unit) is followed only on the
/// levels the gear feeding it, or fed by it, stands on.
type Where = Option<BTreeSet<String>>;

fn merge(map: &mut BTreeMap<String, Where>, tag: String, w: Where) -> bool {
    match map.get_mut(&tag) {
        None => {
            map.insert(tag, w);
            true
        }
        Some(None) => false,
        Some(Some(have)) => match w {
            None => {
                map.insert(tag, None);
                true
            }
            Some(w) => {
                let before = have.len();
                have.extend(w);
                have.len() > before
            }
        },
    }
}

fn allowed(w: Option<&Where>, level: &str) -> bool {
    match w {
        None => false,
        Some(None) => true,
        Some(Some(set)) => set.contains(&level.to_ascii_uppercase()),
    }
}

/// The system of `start`: what it feeds and what feeds it. A panel or gear: the gear above it
/// by the feeders, the gear and panels it feeds, the circuits of every panel it feeds and their
/// pieces; a device or a branch circuit's piece: its circuit, its panel and the gear above;
/// a duct, pipe or unit: its systems' runs, terminals and units. At most the rules' cap.
fn follow(d: &Drawing, sp: Option<&cadaddy_walk3d::BuildingSpec>, s: &Scope, start: Handle) -> HashSet<Handle> {
    let cap = s.rules.max_follow.clamp(1, MAX_SCAN);
    let mut out: HashSet<Handle> = HashSet::from([start]);
    let Some(e) = d.model.get(start) else { return out };
    let up = |t: &str| t.trim().to_ascii_uppercase();
    let splits = |t: &str| t.split([',', ';', '&']).map(up).filter(|x| !x.is_empty()).collect::<Vec<String>>();
    let feeders = sp.map(|sp| sp.mep.feeders.as_slice()).unwrap_or(&[]);
    // Each tag's levels, and the typical ones (placed more than once).
    let mut tag_levels: HashMap<String, (BTreeSet<String>, usize)> = HashMap::new();
    for q in sp.map(|sp| sp.mep.equipment.as_slice()).unwrap_or(&[]).iter().take(MAX_SCAN) {
        let slot = tag_levels.entry(up(&q.tag)).or_default();
        slot.0.insert(up(&q.level));
        slot.1 += 1;
    }
    let typical = |t: &str| tag_levels.get(t).is_some_and(|x| x.1 > 1);
    let levels_of = |t: &str| tag_levels.get(t).map(|x| x.0.clone()).unwrap_or_default();
    let level = super::bspec::made_level(e).map(|l| up(&l)).unwrap_or_default();
    let (circuit, panel) = circuit_of(e);
    let tag = bimlook::made_tag(e, "tag").map(up).filter(|t| !t.is_empty());
    let mut gear: BTreeMap<String, Where> = BTreeMap::new();
    let mut circuits: Vec<(String, bool, Where)> = Vec::new();
    let mut systems: BTreeSet<String> = BTreeSet::new();
    // Up the feeders: the gear above, a typical piece fed only from gear on its own level.
    let climb = |from: (String, Where), gear: &mut BTreeMap<String, Where>| {
        let mut q = VecDeque::from([from]);
        while let Some((x, w)) = q.pop_front() {
            if gear.len() >= MAX_LIST {
                break;
            }
            for f in feeders.iter().filter(|f| splits(&f.to).contains(&x)) {
                for src in splits(&f.from) {
                    let on = levels_of(&src);
                    if let Some(w) = &w
                        && !on.is_empty()
                        && on.is_disjoint(w)
                    {
                        continue;
                    }
                    let ww = if typical(&src) { w.clone() } else { None };
                    if merge(gear, src.clone(), ww.clone()) {
                        q.push_back((src, ww));
                    }
                }
            }
        }
    };
    let here = |t: &str| if typical(t) { Some(BTreeSet::from([level.clone()])) } else { None };
    match (&tag, &circuit) {
        (Some(t), _) => {
            let w = here(t);
            merge(&mut gear, t.clone(), w.clone());
            climb((t.clone(), w.clone()), &mut gear);
            // Down: the gear and panels it feeds (a typical one on the feeding gear's levels),
            // and every circuit on them.
            let mut below: BTreeMap<String, Where> = BTreeMap::from([(t.clone(), w.clone())]);
            let mut q = VecDeque::from([(t.clone(), w)]);
            while let Some((x, w)) = q.pop_front() {
                if below.len() >= MAX_LIST {
                    break;
                }
                for f in feeders.iter().filter(|f| splits(&f.from).contains(&x)) {
                    for to in splits(&f.to) {
                        let ww = if typical(&to) { w.clone().or_else(|| Some(levels_of(&x))) } else { None };
                        if merge(&mut below, to.clone(), ww.clone()) {
                            q.push_back((to, ww));
                        }
                    }
                }
            }
            for (b, w) in below {
                circuits.push((format!("{b}-"), true, w.clone()));
                merge(&mut gear, b, w);
            }
            if let Some(sp) = sp
                && let Some(eq) = sp.mep.equipment.iter().find(|q| up(&q.tag) == *t)
            {
                systems.extend(eq.systems.iter().map(|x| up(x)));
            }
        }
        (None, Some(c)) => {
            let p = panel.as_deref().map(up).unwrap_or_default();
            let w = here(&p);
            circuits.push((up(c), false, w.clone()));
            if !p.is_empty() {
                merge(&mut gear, p.clone(), w.clone());
                climb((p, w), &mut gear);
            }
        }
        (None, None) => {
            if let Some(sy) = bimlook::made_tag(e, "system").map(up).filter(|x| !x.is_empty()) {
                systems.insert(sy);
            }
        }
    }
    // The feeders between the gear followed: by tag (a raceway on its feeder's system), by run.
    let feeder_tags: BTreeSet<String> = feeders
        .iter()
        .filter(|f| splits(&f.from).iter().any(|x| gear.contains_key(x)) && splits(&f.to).iter().any(|x| gear.contains_key(x)))
        .map(|f| up(&f.tag))
        .collect();
    for x in d.model.iter().take(MAX_SCAN) {
        if out.len() >= cap {
            break;
        }
        if super::wallgen::made_as(x).is_none() {
            continue;
        }
        let lv = super::bspec::made_level(x).unwrap_or_default();
        let xt = bimlook::made_tag(x, "tag").map(up);
        let sy = bimlook::made_tag(x, "system").map(up).unwrap_or_default();
        let feeder_run = run_base_of(x).and_then(|b| s.feeder_ends.get(b)).is_some_and(|to| to.iter().any(|t| allowed(gear.get(t), &lv)));
        let hit = xt.as_ref().is_some_and(|t| allowed(gear.get(t), &lv))
            || (!sy.is_empty() && (systems.contains(&sy) || feeder_tags.contains(&sy)))
            || feeder_run
            || (!circuits.is_empty() && {
                let (c, _) = circuit_of(x);
                c.map(|c| up(&c)).is_some_and(|c| {
                    circuits.iter().any(|(w, prefix, at)| (if *prefix { c.starts_with(w.as_str()) } else { c == *w }) && allowed(Some(at), &lv))
                })
            });
        if hit {
            out.insert(x.handle);
        }
    }
    out
}

// ─── The drawing the view draws ──────────────────────────────────────────────

/// The view's own context in the stacked drawing `out` (MEP only): one outline round the outside
/// of the building at its lowest floor (the largest floor's edge) and each elevator's hoistway
/// as one simple volume.
pub fn add_context(out: &mut Drawing, s: &Scope) {
    if s.view != View::Mep {
        return;
    }
    let grey = cadaddy_doc::color::Color::True(cadaddy_color::Rgb(150, 150, 150));
    let layer = "BIM-VIEW-CONTEXT";
    if !out.layers.iter().any(|l| l.name.eq_ignore_ascii_case(layer)) {
        let mut l = cadaddy_doc::Layer::new(layer);
        l.color = grey;
        out.layers.push(l);
    }
    // The outline: the outer edge of the floor that covers the most plan, at the lowest floor.
    let mut best: Option<(f64, Vec<Vec2>)> = None;
    let mut low = f64::INFINITY;
    for e in out.model.iter().take(MAX_SCAN) {
        if super::wallgen::made_as(e).as_deref() != Some("floor") {
            continue;
        }
        let EntityKind::Mesh(m) = &e.kind else { continue };
        if let Some((lo, _)) = mesh_box(e) {
            low = low.min(lo.z);
        }
        if let Some(ring) = outer_ring(m) {
            let a = cadaddy_geom::shoelace(&ring).abs();
            if best.as_ref().is_none_or(|b| a > b.0) {
                best = Some((a, ring));
            }
        }
    }
    if let Some((_, ring)) = best
        && low.is_finite()
    {
        let pts: Vec<Vec3> = ring.iter().map(|p| Vec3::new(p.x, p.y, low)).collect();
        // The view's own context, tagged as such (no sheet linework is drawn in 3D).
        let tags = vec![
            cadaddy_doc::RawTag { code: 1001, value: super::wallgen::APP.into() },
            cadaddy_doc::RawTag { code: 1000, value: "outline".into() },
            cadaddy_doc::RawTag { code: 1000, value: String::new() },
            cadaddy_doc::RawTag { code: 1000, value: Handle(0).hex() },
        ];
        for (a, b) in pts.iter().zip(pts.iter().cycle().skip(1)) {
            let _ = out.add(
                &Space::Model,
                Common { layer: layer.into(), color: grey, xdata: tags.clone().into(), ..Common::default() },
                EntityKind::Line(cadaddy_doc::Line { a: *a, b: *b }),
            );
        }
    }
    for h in &s.hoists {
        let mut m = super::wallgen::empty_mesh();
        let (a, b) = h.shown;
        let mid = Vec2::new((a.x + b.x) / 2.0, a.y);
        let top = Vec2::new((a.x + b.x) / 2.0, b.y);
        super::wallgen::add_box(&mut m, mid, top, (b.x - a.x).abs(), h.z0, h.z1);
        if m.faces.is_empty() {
            continue;
        }
        let xdata = vec![
            cadaddy_doc::RawTag { code: 1001, value: super::wallgen::APP.into() },
            cadaddy_doc::RawTag { code: 1000, value: "hoistway".into() },
            cadaddy_doc::RawTag { code: 1000, value: String::new() },
            cadaddy_doc::RawTag { code: 1000, value: Handle(0).hex() },
            cadaddy_doc::RawTag { code: 1000, value: h.name.chars().take(80).collect() },
        ];
        let _ = out.add(
            &Space::Model,
            Common {
                layer: layer.into(),
                color: cadaddy_doc::color::Color::True(cadaddy_color::Rgb(176, 178, 182)),
                xdata: xdata.into(),
                material: Some(bimlook::GHOST_MATERIAL.to_string()).into(),
                ..Common::default()
            },
            EntityKind::Mesh(m),
        );
    }
}

/// A floor mesh's outer edge on plan: its upward faces' edges that no other upward face shares,
/// joined into rings; the ring with the most area.
fn outer_ring(m: &cadaddy_doc::Mesh) -> Option<Vec<Vec2>> {
    let key = |p: Vec3| ((p.x * 1e3).round() as i64, (p.y * 1e3).round() as i64);
    let mut edges: HashMap<((i64, i64), (i64, i64)), (Vec2, Vec2, i32)> = HashMap::new();
    for f in m.faces.iter().take(MAX_SCAN) {
        let pts: Vec<Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).copied()).collect();
        if pts.len() < 3 {
            continue;
        }
        // Upward (the top of the slab): the face's normal points up.
        let mut n = Vec3::new(0.0, 0.0, 0.0);
        for (a, b) in pts.iter().zip(pts.iter().cycle().skip(1)) {
            n = n + Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
        }
        if !(n.z > 0.0 && n.z.abs() > 0.9 * n.len()) {
            continue;
        }
        for (a, b) in pts.iter().zip(pts.iter().cycle().skip(1)) {
            let (ka, kb) = (key(*a), key(*b));
            if ka == kb {
                continue;
            }
            let k = if ka < kb { (ka, kb) } else { (kb, ka) };
            let slot = edges.entry(k).or_insert((a.xy(), b.xy(), 0));
            slot.2 += 1;
        }
    }
    let single: Vec<(Vec2, Vec2)> = edges.values().filter(|e| e.2 == 1).map(|e| (e.0, e.1)).collect();
    if single.len() < 3 {
        return None;
    }
    // Join them into rings, end to start.
    let k2 = |p: Vec2| ((p.x * 1e3).round() as i64, (p.y * 1e3).round() as i64);
    let mut from: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, (a, b)) in single.iter().enumerate() {
        from.entry(k2(*a)).or_default().push(i);
        from.entry(k2(*b)).or_default().push(i);
    }
    let mut used = vec![false; single.len()];
    let mut best: Option<(f64, Vec<Vec2>)> = None;
    for start in 0..single.len() {
        if used.get(start).copied().unwrap_or(true) {
            continue;
        }
        let Some(&(a, b)) = single.get(start) else { continue };
        if let Some(u) = used.get_mut(start) {
            *u = true;
        }
        let mut ring = vec![a, b];
        let mut at = b;
        for _ in 0..single.len() {
            let next = from.get(&k2(at)).and_then(|l| l.iter().copied().find(|i| !used.get(*i).copied().unwrap_or(true)));
            let Some(i) = next else { break };
            if let Some(u) = used.get_mut(i) {
                *u = true;
            }
            let Some(&(p, q)) = single.get(i) else { break };
            at = if k2(p) == k2(at) { q } else { p };
            if k2(at) == k2(a) {
                break;
            }
            ring.push(at);
        }
        if ring.len() >= 3 {
            let area = cadaddy_geom::shoelace(&ring).abs();
            if best.as_ref().is_none_or(|b| area > b.0) {
                best = Some((area, ring));
            }
        }
    }
    best.map(|b| b.1)
}

/// `out`'s objects in `ghost` (the view's see-through part) drawn see-through.
pub fn ghost_these(out: &mut Drawing, ghost: &HashSet<Handle>) {
    if ghost.is_empty() {
        return;
    }
    if !out.materials.iter().any(|m| m.name.eq_ignore_ascii_case(bimlook::GHOST_MATERIAL)) {
        out.materials.push(cadaddy_doc::material::Material {
            name: bimlook::GHOST_MATERIAL.into(),
            color: cadaddy_color::Rgb(226, 226, 222),
            roughness: 1.0,
            metallic: 0.0,
            opacity: bimlook::GHOST_OPACITY,
            ..cadaddy_doc::material::Material::default()
        });
    }
    let list: Vec<Entity> = ghost
        .iter()
        .filter_map(|h| out.model.get(*h))
        .map(|e| {
            let mut c = (**e).clone();
            c.common.material = Some(bimlook::GHOST_MATERIAL.to_string()).into();
            c
        })
        .collect();
    for c in list {
        out.model.push(c);
    }
}

/// The built elements of `d` that the view of `look` draws (handles), and how many built
/// elements there are.
pub fn drawn_set(d: &Drawing, look: &Look) -> (HashSet<Handle>, usize, LeftOut) {
    let scope = Scope::of(d, look);
    let mut shown = HashSet::new();
    let mut total = 0usize;
    for e in d.model.iter().take(MAX_SCAN) {
        let Some(c) = bimlook::classify(d, e) else { continue };
        total += 1;
        if look.shows(&c) && scope.shows(d, e, &c) != Shown::No {
            shown.insert(e.handle);
        }
    }
    (shown, total, scope.left_out())
}

/// The drawing the 3D BIM view draws ([`super::bimlook::display`]) for one state of a drawing,
/// kept until the drawing changes: 3D object snaps ask for it on every move of the cursor.
#[derive(Clone, Default)]
pub struct ShownCache(Arc<std::sync::Mutex<Option<((usize, u64), Option<Arc<Drawing>>)>>>);

impl std::fmt::Debug for ShownCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShownCache")
    }
}

impl ShownCache {
    /// What the BIM view draws of `st`'s drawing (`None`: the view is off or nothing is built).
    pub fn of(&self, st: &crate::DocState) -> Option<Arc<Drawing>> {
        let key = (Arc::as_ptr(&st.doc) as usize, st.revision);
        let Ok(mut c) = self.0.lock() else { return bimlook::display(&st.doc).map(Arc::new) };
        if let Some((k, d)) = c.as_ref()
            && *k == key
        {
            return d.clone();
        }
        let d = bimlook::display(&st.doc).map(Arc::new);
        *c = Some((key, d.clone()));
        d
    }
}

// ─── bim.view ────────────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "BIM Views", run_view).menu(&["View", "BIM Views"]).params(
            "{view?: architectural|mep|floor|room (one at a time; never every object), full?: bool (MEP: every duct, plumbing mains 2\" and up, fire mains, every panel and its feeder, what passes through floors; default mains and big gear), level?: name | null (the Floor and Room views' level; a level picks the Floor view), room?: number or name | null (the Room view's room on the level; a room picks the Room view), up?: bool (Room to Floor to the whole building, as Esc), enter?: handle (the next view down at an element: its level, then its room), switches?: {m?, e?, p?, cabinets?, furniture?: bool}, follow?: handle | null (highlight what it feeds and what feeds it across levels, the rest faded)} → {view, full, level, room, breadcrumb: [name], switches: {m, e, p, cabinets, furniture}, offered: [switch] (this view's), levels: [name], rooms: [{number, name}] (the level's), drawn (built elements this view draws), total (built elements), left_out: {pipe_unknown_size, fire_pipe_unknown, panel_role_unknown}, follow?: {handle, elements}}: what the 3D BIM view draws; counts, takeoff, query, clash, export and plot never depend on it",
        ),
        CommandSpec::new(RULES_CMD, "BIM View Rules", run_rules).params(
            "{rules?: {key: value} (any of the rules data's keys: architectural_parts, gear_words, panel_words, big_equipment_words, main_duct_min_in, large_pipe_min_in, plumbing_main_min_in (2), fire_main_min_in (2.5), feeder_min_in (1), fire_main_words, fire_branch_words, unit_panel_copies, …; kept in the drawing), reset?: bool (back to the built-in rules)} → {source: project|default, rules}: what each BIM view draws, per project",
        ),
        CommandSpec::new(REPORT, "BIM View Report", run_report)
            .params("{width?: px (1600), height?: px (1000), level?: (the Floor view's; default the lowest), room?: (the Room view's; default the level's first)} → {total, views: {architectural, mep, mep_full, floor, room: {drawn, all_switches_drawn, tris, build_ms, model_mb, gpu_mb_est, visible_ms, raster_ms}}}: what each view draws and costs on this model, measured (raster_ms: the CPU rasterizer drawing the whole view, a stand-in for a frame)")
            .noundo(),
    ]
}

fn handle_of(v: &Value) -> Option<Handle> {
    v.as_str().and_then(Handle::parse_hex).filter(|h| h.0 != 0)
}

/// The level of `want` among the model's (any case).
fn level_named(d: &Drawing, cmd: &str, want: &str) -> Result<String> {
    let levels = levels_of(d);
    levels.iter().find(|l| l.eq_ignore_ascii_case(want.trim())).cloned().ok_or_else(|| {
        bad(cmd, format!("there is no level named {want} in the built model — levels: {:?}", levels.iter().take(50).collect::<Vec<_>>()))
    })
}

/// The view's state as `bim.view` reports it.
pub fn view_json(d: &Drawing, look: &Look) -> Value {
    let scope = Scope::of(d, look);
    let view = scope.view;
    let levels = levels_of(d);
    let rooms: Vec<Value> =
        scope.level.as_deref().map(|l| rooms_on(d, l)).unwrap_or_default().into_iter().map(|(n, name)| json!({"number": n, "name": name})).collect();
    let mut crumb = vec!["Building".to_string()];
    if let Some(l) = &scope.level {
        crumb.push(l.clone());
    }
    if let Some(r) = &scope.room {
        crumb.push(r.clone());
    }
    let offered: Vec<&str> = match view {
        View::Architectural => vec![],
        View::Mep => vec!["m", "e", "p"],
        View::Floor => vec!["m", "e", "p", "cabinets"],
        View::Room => vec!["m", "e", "p", "cabinets", "furniture"],
    };
    let (drawn, total, left) = drawn_set(d, look);
    let mut out = json!({
        "view": view.name(),
        "full": look.full_mep,
        "level": scope.level,
        "room": scope.room,
        "breadcrumb": crumb,
        "switches": SWITCHES.iter().map(|k| (k.to_string(), json!(switch_on(look, k)))).collect::<serde_json::Map<String, Value>>(),
        "offered": offered,
        "levels": levels,
        "rooms": rooms,
        "drawn": drawn.len(),
        "total": total,
        "left_out": left,
    });
    if let (Some(h), Some(f)) = (look.follow.as_deref(), &scope.follow) {
        out["follow"] = json!({"handle": h, "elements": f.len()});
    }
    out
}

fn run_view(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let mut look = Look::of(d);
    let was = active(&look);
    if p.get("up").and_then(Value::as_bool) == Some(true) {
        match was {
            View::Room => look.room = None,
            View::Floor => {
                look.level = None;
                look.view = Some(look.whole.clone().unwrap_or_else(|| View::Mep.name().into()));
            }
            _ => {}
        }
        if was == View::Room {
            look.view = Some(View::Floor.name().into());
        }
    }
    if let Some(v) = p.get("view").filter(|v| !v.is_null()) {
        let t = v.as_str().unwrap_or_default();
        let view = View::parse(t).ok_or_else(|| bad(CMD, format!("`view` must be architectural, mep, floor or room, got {v}")))?;
        set_view(&mut look, view);
    }
    if let Some(b) = p.get("full").and_then(Value::as_bool) {
        look.full_mep = b;
    }
    match p.get("level") {
        None => {}
        Some(Value::Null) => {
            look.level = None;
            look.room = None;
            if active(&look).per_level() {
                look.view = Some(look.whole.clone().unwrap_or_else(|| View::Mep.name().into()));
            }
        }
        Some(Value::String(l)) => {
            let l = level_named(d, CMD, l)?;
            // Another level: its Floor view (a room is of one level).
            if look.level.as_deref() != Some(l.as_str()) || !active(&look).per_level() {
                look.level = Some(l);
                set_view(&mut look, View::Floor);
            }
        }
        Some(v) => return Err(bad(CMD, format!("`level` must be a level name or null, got {v}"))),
    }
    match p.get("room") {
        None => {}
        Some(Value::Null) => {
            look.room = None;
            if active(&look) == View::Room {
                look.view = Some(View::Floor.name().into());
            }
        }
        Some(Value::String(r)) if !r.trim().is_empty() => {
            let level = look.level.clone().or_else(|| first_level(d, None)).ok_or_else(|| bad(CMD, "the Room view needs a level: give `level`"))?;
            let rooms = rooms_on(d, &level);
            let hit =
                rooms.iter().find(|(n, name)| n.eq_ignore_ascii_case(r.trim()) || name.eq_ignore_ascii_case(r.trim())).cloned().ok_or_else(|| {
                    bad(CMD, format!("there is no room {r} on {level} — rooms: {:?}", rooms.iter().take(50).map(|x| &x.0).collect::<Vec<_>>()))
                })?;
            look.level = Some(level);
            look.room = Some(hit.0);
            set_view(&mut look, View::Room);
        }
        Some(v) => return Err(bad(CMD, format!("`room` must be a room number or name, or null, got {v}"))),
    }
    if let Some(v) = p.get("enter").filter(|v| !v.is_null()) {
        let h = handle_of(v).ok_or_else(|| bad(CMD, "`enter` must be an element's handle"))?;
        let e = d.model.get(h).ok_or_else(|| bad(CMD, format!("there is no object {} in the model", h.hex())))?;
        let level = super::bspec::made_level(e).filter(|l| !l.is_empty()).ok_or_else(|| bad(CMD, "that element is on no level"))?;
        match active(&look) {
            View::Architectural | View::Mep => {
                look.level = Some(level);
                look.room = None;
                set_view(&mut look, View::Floor);
            }
            View::Floor | View::Room => {
                let p0 = plan_point(d, e);
                let room = super::bspec::room_elements(d).into_iter().find(|r| {
                    r.level.eq_ignore_ascii_case(&level) && (r.handle == h || p0.is_some_and(|p| cadaddy_geom::point_in_polygon(&r.outline, p)))
                });
                if let Some(r) = room {
                    look.level = Some(level);
                    look.room = Some(r.number.clone().filter(|n| !n.trim().is_empty()).unwrap_or(r.name));
                    set_view(&mut look, View::Room);
                }
            }
        }
    }
    if let Some(sw) = p.get("switches").filter(|v| !v.is_null()) {
        let o = sw.as_object().ok_or_else(|| bad(CMD, "`switches` must be {m?, e?, p?, cabinets?, furniture?: bool}"))?;
        for (k, v) in o {
            let k = k.trim().to_ascii_lowercase();
            let on = v.as_bool().ok_or_else(|| bad(CMD, format!("switch `{k}` must be true or false")))?;
            match k.as_str() {
                "furniture" => look.furniture = on,
                "m" | "e" | "p" | "cabinets" => {
                    if on {
                        look.switched_off.remove(&k);
                    } else {
                        look.switched_off.insert(k);
                    }
                }
                _ => return Err(bad(CMD, format!("no switch `{k}`: m, e, p, cabinets or furniture"))),
            }
        }
    }
    match p.get("follow") {
        None => {}
        Some(Value::Null) | Some(Value::Bool(false)) => look.follow = None,
        Some(v) => {
            let h = handle_of(v).ok_or_else(|| bad(CMD, "`follow` must be an element's handle or null"))?;
            if d.model.get(h).is_none() {
                return Err(bad(CMD, format!("there is no object {} in the model", h.hex())));
            }
            look.follow = Some(h.hex());
        }
    }
    let deselected = bimlook::save_look(s, &look)?;
    let mut out = view_json(s.doc()?, &look);
    if deselected > 0 {
        out["deselected"] = json!(deselected);
    }
    Ok(out)
}

/// Switches `look` to `view`, remembering the whole-building one to come back to.
pub fn set_view(look: &mut Look, view: View) {
    if !view.per_level() {
        look.whole = Some(view.name().into());
        look.level = None;
        look.room = None;
    }
    if view != View::Room {
        look.room = None;
    }
    look.view = Some(view.name().into());
}

fn run_rules(s: &mut Session, p: &Value) -> Result<Value> {
    let reset = p.get("reset").and_then(Value::as_bool) == Some(true);
    let mut own = if reset { None } else { project(s.doc()?) };
    let mut changed = reset;
    if let Some(v) = p.get("rules").filter(|v| !v.is_null()) {
        let o = v.as_object().ok_or_else(|| bad(RULES_CMD, "`rules` must be {key: value}"))?;
        let base = serde_json::to_value(Rules::defaults()).unwrap_or(Value::Null);
        let mut merged = own.clone().unwrap_or(json!({}));
        for (k, x) in o {
            if k == "source" || k == "version" || base.get(k).is_none() {
                return Err(bad(RULES_CMD, format!("no rule `{k}`")));
            }
            merged[k.as_str()] = x.clone();
        }
        // The whole set must still read.
        let mut test = base.clone();
        if let (Some(t), Some(m)) = (test.as_object_mut(), merged.as_object()) {
            for (k, x) in m {
                t.insert(k.clone(), x.clone());
            }
        }
        serde_json::from_value::<Rules>(test).map_err(|e| bad(RULES_CMD, format!("`rules`: {e}")))?;
        own = Some(merged);
        changed = true;
    }
    if changed {
        let d = s.doc()?;
        let mut spec: Value = d.building_spec.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(json!({}));
        if let Some(o) = spec.as_object_mut() {
            match own {
                Some(v) => {
                    o.insert(KEY.into(), v);
                }
                None => {
                    o.remove(KEY);
                }
            }
        }
        s.doc_mut()?.building_spec = Some(spec.to_string()).into();
    }
    let (r, mine) = Rules::of(s.doc()?);
    Ok(json!({"source": if mine { "project" } else { "default" }, "rules": r}))
}

// ─── The report ──────────────────────────────────────────────────────────────

fn ms(t: web_time::Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

fn mb(bytes: usize) -> f64 {
    (bytes as f64 / 1048576.0 * 10.0).round() / 10.0
}

/// Bytes a vertex takes in the 3D view's GPU buffers (for the estimate).
const GPU_VERTEX: usize = 68;

/// What the view in `look` draws of `d`, measured: built, culled, rasterized on the CPU.
pub fn measure(d: &Drawing, look: &Look, width: u32, height: u32) -> Value {
    let (drawn, _, left) = drawn_set(d, look);
    // Every switch on: still inside the view.
    let mut all_on = look.clone();
    all_on.switched_off.clear();
    all_on.furniture = true;
    all_on.hidden.clear();
    all_on.hidden_paths.clear();
    let (every, _, _) = drawn_set(d, &all_on);
    let Some(md) = bimlook::display_of(d, look) else { return json!({"drawn": drawn.len(), "all_switches_drawn": every.len()}) };
    let md = Arc::new(md);
    let ext = md.extents(&Space::Model);
    let size = ext.width().max(ext.height());
    let tol = if size.is_finite() && size > 0.0 { size / 1500.0 } else { 1.0 };
    let bg = cadaddy_color::Rgb(255, 255, 255);
    let classify = super::fidelity::classifier(&md);
    let (progress, total) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let t0 = web_time::Instant::now();
    let m = cadaddy_render::model3::build_fid(
        Arc::clone(&md),
        &Space::Model,
        tol,
        bg,
        &progress,
        &total,
        &[],
        &classify,
        cadaddy_render::model3::FidOptions::default(),
    );
    let build_ms = ms(t0);
    let proj = cadaddy_geom::view3::Projection::new(Vec3::new(-1.0, -1.0, 1.0));
    let tier = cadaddy_render::model3::Tier::Detail;
    let Some(v) = cadaddy_render::shade::fit_bounds(&m.bounds, proj, width, height) else {
        return json!({"drawn": drawn.len(), "all_switches_drawn": every.len(), "build_ms": build_ms, "left_out": left});
    };
    let mut st = cadaddy_render::model3::Stream::default();
    st.update(&m, &v, tier, f64::INFINITY);
    let t1 = web_time::Instant::now();
    let vis = m.visible_at(&v, tier, None);
    let visible_ms = ms(t1);
    let t2 = web_time::Instant::now();
    let mut flat = m.flatten_visible(&vis);
    for k in st.visible(&v) {
        if let Some(c) = st.cells.get(&k) {
            flat.tris.extend_from_slice(&c.scene.tris);
            flat.lines.extend_from_slice(&c.scene.lines);
        }
    }
    let small = cadaddy_render::shade::ShadeView { width_px: 640, height_px: 400, ..v };
    let _ = cadaddy_render::shade::rasterize(&flat, &small, cadaddy_render::shade::VisualStyle::ShadedEdges, bg);
    let raster_ms = ms(t2);
    let (lt, ll) = m.loaded_at(tier);
    let gpu = (lt + st.tris()) * 3 * GPU_VERTEX + (ll + st.cells.values().map(|c| c.scene.lines.len()).sum::<usize>()) * 2 * GPU_VERTEX;
    json!({
        "drawn": drawn.len(),
        "all_switches_drawn": every.len(),
        "tris": vis.tris + st.tris(),
        "build_ms": build_ms,
        "model_mb": mb(m.bytes() + st.bytes),
        "gpu_mb_est": mb(gpu),
        "visible_ms": visible_ms,
        "raster_ms": raster_ms,
        "left_out": left,
    })
}

fn run_report(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let width = p.get("width").and_then(Value::as_u64).unwrap_or(1600).clamp(64, 8192) as u32;
    let height = p.get("height").and_then(Value::as_u64).unwrap_or(1000).clamp(64, 8192) as u32;
    let base = Look::of(d);
    let level = match str_param(p, "level") {
        Some(l) => Some(level_named(d, REPORT, l)?),
        None => first_level(d, None),
    };
    let room = str_param(p, "room").map(str::to_string).or_else(|| level.as_deref().and_then(|l| rooms_on(d, l).into_iter().next().map(|r| r.0)));
    let total = d.model.iter().take(MAX_SCAN).filter(|e| bimlook::classify(d, e).is_some()).count();
    let mut views = serde_json::Map::new();
    let mut at = |name: &str, view: View, full: bool| {
        let mut look = base.clone();
        look.follow = None;
        set_view(&mut look, view);
        look.full_mep = full;
        if view.per_level() {
            look.level = level.clone();
        }
        if view == View::Room {
            look.room = room.clone();
        }
        views.insert(name.into(), measure(d, &look, width, height));
    };
    at("architectural", View::Architectural, false);
    at("mep", View::Mep, false);
    at("mep_full", View::Mep, true);
    at("floor", View::Floor, false);
    at("room", View::Room, false);
    Ok(json!({"total": total, "level": level, "room": room, "views": views}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_parse_and_rules_read() {
        assert_eq!(View::parse("MEP"), Some(View::Mep));
        assert_eq!(View::parse("level"), Some(View::Floor));
        assert_eq!(View::parse("street"), None);
        assert_eq!(View::default(), View::Mep);
        let r = Rules::defaults();
        assert!(!r.source.is_empty() && r.architectural_parts.len() > 5);
        assert_eq!((r.plumbing_main_min_in, r.fire_main_min_in, r.feeder_min_in), (2.0, 2.5, 1.0));
        assert_eq!(fire_role("fire protection cross main", &r), Some(true));
        assert_eq!(fire_role("sprinkler branch line", &r), Some(false));
        assert_eq!(fire_role("sprinkler", &r), None);
    }

    #[test]
    fn a_floor_slabs_outer_ring() {
        let mut m = super::super::wallgen::empty_mesh();
        super::super::wallgen::add_box(&mut m, Vec2::new(0.0, 5.0), Vec2::new(20.0, 5.0), 10.0, -1.0, 0.0);
        let ring = outer_ring(&m).expect("a ring");
        assert_eq!(ring.len(), 4);
        assert!((cadaddy_geom::shoelace(&ring).abs() - 200.0).abs() < 1e-6);
    }
}
