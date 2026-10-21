//! The ground truth of a fixture set: everything the sheets show, as numbers. A print reader is
//! scored by comparing what it found with this.
//!
//! Coordinates are **building coordinates in feet**: x east, y north, from the intersection of the
//! first grid lines; z is the elevation above the level 1 finished floor. Sizes are **inches**.
//! Each sheet records how building coordinates map onto its drawing (model) space and its paper.
//!
//! The MEP systems are a graph: [`Node`]s (fittings, connections to terminals, equipment and
//! fixtures, open ends) joined by straight [`Segment`]s. A segment is either horizontal (it may
//! slope, for drains) or vertical (risers, drops to diffusers and heads).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The truth file's format; bumped when a field changes meaning.
pub const SCHEMA: &str = "cadaddy-mepfix/2";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Truth {
    pub schema: String,
    pub seed: u64,
    /// The made-up project's name, as the title blocks print it.
    pub project: String,
    /// `"office"` or `"hotel"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    pub building: Building,
    /// The systems on the sheets, with the abbreviation the prints use for each.
    pub systems: Vec<System>,
    pub nodes: Vec<Node>,
    pub segments: Vec<Segment>,
    pub terminals: Vec<Terminal>,
    pub equipment: Vec<Equipment>,
    pub fixtures: Vec<Fixture>,
    pub elec_devices: Vec<ElecDevice>,
    pub elec_panels: Vec<ElecPanel>,
    pub elec_circuits: Vec<ElecCircuit>,
    pub elec_conduits: Vec<ElecConduit>,
    pub risers: Vec<Riser>,
    // --- Hotel extensions (Q304) ---
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wall_assemblies: Vec<WallAssembly>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub walls: Vec<WallInstance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ceiling_assemblies: Vec<CeilingAssembly>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub soffits: Vec<SoffitInstance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub casework: Vec<CaseworkItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spec_items: Vec<SpecItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub site_elements: Vec<SiteElement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fire_alarm_devices: Vec<FireAlarmDevice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub telecom_devices: Vec<TelecomDevice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elevations: Vec<ElevationView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<SectionView>,
    /// Planted gaps and conflicts for the RFI helper (Q296).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<Gap>,
    pub sheets: Vec<Sheet>,
    /// In-wall rough-in stubs behind plumbing fixtures (supply, waste, vent).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rough_ins: Vec<RoughIn>,
    /// Electrical devices placed on walls (receptacles, switches, junction boxes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<Device>,
    /// Placement zones for devices with unfixed positions (where in-wall conflicts may/will/won't
    /// occur).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placement_zones: Vec<PlacementZone>,
    /// Soffits read from the reflected ceiling plans (office set; the hotel uses `soffits`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rcp_soffits: Vec<Soffit>,
    /// The wall type legend (partition types sheet), every type in it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wall_types: Vec<WallType>,
    /// The walls on the architectural floor plans, with their type and height.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_walls: Vec<PlanWall>,
    /// Every clash between the modelled elements (hard clashes, runs below a ceiling, runs into
    /// structure, in-wall device-vs-pipe), computed from the geometry; `planted` ones were put
    /// there on purpose.
    pub clashes: Vec<Clash>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Building {
    pub name: String,
    pub levels: Vec<Level>,
    pub grid: Grid,
    pub rooms: Vec<Room>,
    /// Architectural walls derived from the room geometry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub walls: Vec<Wall>,
    pub columns: Vec<Column>,
    pub beams: Vec<Beam>,
    /// Concrete slab on deck, inches (top of slab = the level's elevation).
    pub slab_in: f64,
    // --- Hotel extensions ---
    /// Stairs (bounding box per stair per level).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stairs: Vec<Stair>,
    /// Elevator shafts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elevators: Vec<Elevator>,
    /// Vertical shafts (plumbing, electrical, duct).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shafts: Vec<Shaft>,
    /// Per-level slab descriptions (composite deck, roof deck, slab-on-grade).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slabs: Vec<Slab>,
    /// Openings and shafts through the floor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub openings: Vec<Opening>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Level {
    /// As the title blocks print it: `LEVEL 1`, `LEVEL 2`, `ROOF`.
    pub name: String,
    /// Short code used in tags and ids: `1`, `2`, `R`.
    pub code: String,
    pub elevation_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    /// Lines of constant x (they run north–south on the plans), west to east.
    pub x: Vec<GridLine>,
    /// Lines of constant y, south to north.
    pub y: Vec<GridLine>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GridLine {
    pub label: String,
    pub at_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub level: String,
    pub number: String,
    pub name: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// `None`: open to structure.
    pub ceiling: Option<Ceiling>,
    /// Depth of the plenum (space between the ceiling and the structure above), feet.
    /// Only meaningful when `ceiling` is `Some`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plenum_ft: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ceiling {
    /// Above the room's floor.
    pub height_ft: f64,
    /// `ACT` (acoustic tile in a grid) or `GYP` (gypsum board).
    pub kind: String,
    /// The tag as the reflected ceiling plan prints it.
    pub tag: String,
    /// ACT grid tile size: `[across, along]` in feet (e.g. `[2.0, 4.0]` for 2×4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_ft: Option<[f64; 2]>,
}

/// A soffit or bulkhead: a dropped section of ceiling (lower than the room's main ceiling),
/// typically concealing ductwork or structure.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Soffit {
    pub id: String,
    pub level: String,
    pub room: String,
    /// Bottom of the soffit above the floor, feet.
    pub height_ft: f64,
    /// Width (across the run) and length (along it), feet.
    pub width_ft: f64,
    pub length_ft: f64,
    /// Min/max corners in building feet.
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// The tag on the RCP.
    pub tag: String,
}

/// A wall type as the wall type legend (partition schedule) gives it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WallType {
    /// The tag printed on the plans (`B`, `1B`, `P3`, `W-4`).
    pub tag: String,
    /// The legend's title (`1 HOUR RATED CORRIDOR PARTITION`).
    pub name: String,
    /// From side 1's face to side 2's; cavity insulation right after its studs.
    pub layers: Vec<WallTypeLayer>,
    /// Overall, inches (cavity insulation takes none).
    pub thickness_in: f64,
    /// `ceiling`, `above_ceiling` (6" past it), `deck` (underside of the structure) or `full`.
    pub height: String,
    /// `1 HR`, `2 HR`; none = not rated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_rating: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stc: Option<u32>,
    #[serde(default)]
    pub external: bool,
}

/// One layer of a wall type.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WallTypeLayer {
    /// `gyp`, `gyp_x` (Type X), `gyp_mr` (moisture-resistant), `gyp_ar` (abuse-resistant),
    /// `shaftliner`, `sheathing`, `stud`, `insulation`, `resilient_channel`, `furring`, `cmu`.
    pub material: String,
    pub thickness_in: f64,
    /// `1`, `core` or `2`.
    pub side: String,
    /// Studs: gauge and spacing, inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gauge: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spacing_in: Option<f64>,
    /// Insulation in the stud cavity.
    #[serde(default)]
    pub in_cavity: bool,
}

/// A wall on an architectural floor plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanWall {
    pub id: String,
    pub level: String,
    /// Centreline ends as drawn, building feet.
    pub a_ft: [f64; 2],
    pub b_ft: [f64; 2],
    /// Its type's tag.
    pub wall_type: String,
    /// Whether the plan tags it (an untagged wall's type has to be guessed).
    pub tagged: bool,
    /// Above its floor, from its type's height rule and the ceilings and structure there.
    pub height_ft: f64,
    /// The rooms either side (numbers); an exterior wall has rooms on one side.
    pub rooms: Vec<String>,
    /// Door openings: from and to, feet along the wall from `a_ft`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub openings: Vec<[f64; 2]>,
    /// Where its tags are (none when untagged).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<WallTag>,
}

/// A wall type tag on a plan: `s_ft` along the wall from `a_ft` where it is (or where its leader
/// lands), on the wall or beside it (`side` +1 left of a→b, -1 right) `gap_ft` clear, shifted
/// `shift_ft` along.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WallTag {
    pub s_ft: f64,
    pub on_wall: bool,
    pub side: f64,
    pub shift_ft: f64,
    pub gap_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub id: String,
    /// Grid intersection, `B-2` (x line label, y line label).
    pub grid: String,
    pub at_ft: [f64; 2],
    pub size: String,
    /// Along x and along y, inches.
    pub footprint_in: [f64; 2],
    pub bottom_ft: f64,
    pub top_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Beam {
    pub id: String,
    /// The level whose floor (or the roof) it carries.
    pub level: String,
    /// The grid line it runs on.
    pub grid: String,
    pub size: String,
    pub depth_in: f64,
    pub flange_in: f64,
    pub from_ft: [f64; 2],
    pub to_ft: [f64; 2],
    /// Top of steel, elevation.
    pub tos_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Slab {
    pub id: String,
    /// The level whose floor it forms.
    pub level: String,
    /// `composite` (concrete on metal deck) or `roof` (metal roof deck only).
    pub kind: String,
    /// Concrete thickness, inches (0 for bare roof deck).
    pub concrete_in: f64,
    /// Metal deck depth, inches.
    pub deck_in: f64,
    /// Top of slab elevation, feet.
    pub top_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Opening {
    pub id: String,
    /// The level the opening is on.
    pub level: String,
    /// `shaft` (extends through multiple levels) or `opening` (single floor).
    pub kind: String,
    /// Minimum and maximum corners, building feet (plan).
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Size label as printed on the plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct System {
    /// Our code: `SA` supply air, `RA` return, `EA` exhaust, `OA` outside air, `CW` cold water,
    /// `HW` hot water, `HWR` hot water return, `SAN` sanitary, `V` vent, `G` gas, `FP` fire
    /// protection.
    pub code: String,
    pub name: String,
    /// `duct` or `pipe`.
    pub kind: String,
    /// The abbreviation the prints of this set use (varied by seed: `SA`, `S`…).
    pub printed: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    /// `elbow`, `tee`, `cross`, `transition` (rectangular size change), `reducer` (round or pipe
    /// size change), `offset` (an elbow in a vertical plane), `terminal`, `equipment`, `fixture`,
    /// `cap`, `open_end`, `service` (where a utility enters the building), `vtr` (vent through
    /// roof).
    pub kind: String,
    pub system: String,
    pub level: String,
    pub at_ft: [f64; 3],
    pub segments: Vec<String>,
    /// For `terminal`, `equipment`, `fixture`: the id of the thing it connects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connects: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub system: String,
    /// `duct` or `pipe`.
    pub kind: String,
    /// `rect`, `round` (ducts) or `pipe`.
    pub shape: String,
    /// Rectangular: width (across, on the plan) × depth; round duct and pipe: [diameter, diameter]
    /// (pipe: nominal size).
    pub size_in: [f64; 2],
    pub from: String,
    pub to: String,
    pub level: String,
    /// Centreline, from → to.
    pub path_ft: [[f64; 3]; 2],
    pub vertical: bool,
    /// Horizontal segments: how high it hangs and where that came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<Height>,
    /// The size tag, if the sheet tags this segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<Tag>,
    /// Drainage slope as printed (`1/4"/FT`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slope: Option<String>,
    /// Drawn on the plan as hidden (below the floor).
    #[serde(default)]
    pub below_floor: bool,
    /// The sheet it is drawn on.
    pub sheet: String,
}

/// Where a segment's height came from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeightSource {
    /// Parsed from a height note on the run (BOD, TOD, CL, AFF, BOP, etc.).
    Note,
    /// Derived from the room's reflected ceiling height plus clearance.
    Ceiling,
    /// Assigned by trade stacking rules (ducts highest, then FP, then plumbing).
    #[default]
    Rule,
    /// Tight to structure (flush against the underside of a beam or slab).
    Structure,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Height {
    /// Bottom of the duct or pipe above the level's floor, feet.
    pub bottom_ft: f64,
    pub top_ft: f64,
    pub centre_ft: f64,
    /// The note on the sheet that gives it (`BOD @ 10'-6"`), when there is one; a reader has to
    /// work out the others (ceilings, structure, rules).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Where this height came from: a note on the run, the room's ceiling, trade rules, or
    /// structure clearance.
    #[serde(default)]
    pub source: HeightSource,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    /// The lines of text as printed.
    pub text: Vec<String>,
    /// Where the tag's text sits, building feet (plan).
    pub at_ft: [f64; 2],
    /// Joined to the run by a leader line.
    pub leader: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Terminal {
    pub id: String,
    /// `supply_diffuser`, `return_grille`, `exhaust_grille`, `louver`, `sprinkler_head`.
    pub kind: String,
    /// Type tag from the schedule (`CD-2`, `RG-1`, `EG-1`, `L-1`, `SH-1`).
    pub type_tag: String,
    pub system: String,
    pub level: String,
    pub room: String,
    pub at_ft: [f64; 3],
    pub face_in: [f64; 2],
    pub neck_in: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfm: Option<f64>,
    /// The node it connects to (none for a plenum return grille).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Equipment {
    pub id: String,
    pub tag: String,
    /// `ahu`, `vav`, `exhaust_fan`, `condensing_unit`, `water_heater`, `pump`, `gas_meter`.
    pub kind: String,
    pub level: String,
    /// Box: minimum and maximum corners, building feet.
    pub min_ft: [f64; 3],
    pub max_ft: [f64; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfm: Option<f64>,
    /// The schedule's columns for it, as printed.
    pub schedule: BTreeMap<String, String>,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fixture {
    pub id: String,
    pub tag: String,
    /// `water_closet`, `lavatory`, `urinal`, `shower`, `mop_sink`, `floor_drain`, `sink`.
    pub kind: String,
    pub level: String,
    pub room: String,
    pub at_ft: [f64; 3],
    pub nodes: Vec<String>,
    /// The wall this fixture is against (its back wall).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall: Option<String>,
}

/// An electrical device on a plan: light fixture, receptacle, switch, data outlet, etc.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElecDevice {
    pub id: String,
    pub tag: String,
    /// `light_fixture`, `receptacle`, `switch`, `data_outlet`, `floor_box`, `junction_box`,
    /// `disconnect`, `exit_sign`, `emergency_light`.
    pub kind: String,
    /// The fixture type from the schedule (`A`, `B`, `C`, `ES-1`, `EM-1`, `R`, `S`…).
    pub type_tag: String,
    pub level: String,
    pub room: String,
    /// True building position (from the architectural plan).
    pub at_ft: [f64; 3],
    /// Where the symbol is drawn on the E-sheet (diagrammatic: near, not at, the true position).
    pub drawn_ft: [f64; 2],
    /// Mounting height above the floor, inches. `0` means on the ceiling.
    pub mounting_in: f64,
    /// Mounting label as printed: `+18" AFF`, `+46"`, `CLG`, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounting_note: Option<String>,
    /// `true`: the A-sheets (architectural plans with dimensions) fix the exact position.
    /// `false`: the position is a zone / field decision.
    pub arch_positioned: bool,
    /// The circuit this device is on.
    pub circuit: String,
    /// The panel this device is fed from.
    pub panel: String,
    /// The sheet it is drawn on.
    pub sheet: String,
}

/// An electrical panel or transformer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElecPanel {
    pub id: String,
    pub tag: String,
    /// `panel`, `transformer`.
    pub kind: String,
    pub level: String,
    pub room: String,
    pub at_ft: [f64; 3],
    /// Box on the plan.
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// The panel schedule's columns as printed (circuit → description, load, breaker).
    pub schedule: Vec<PanelCircuitRow>,
    /// The sheet it is drawn on.
    pub sheet: String,
}

/// One row in a panel schedule.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelCircuitRow {
    pub circuit: String,
    pub description: String,
    pub breaker_amps: String,
    pub poles: String,
}

/// An electrical circuit from a panel to devices, shown as a homerun arrow.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElecCircuit {
    pub id: String,
    /// `L1/1` — panel tag, circuit number.
    pub label: String,
    pub panel: String,
    pub number: String,
    pub level: String,
    /// Device ids on this circuit.
    pub devices: Vec<String>,
    /// Homerun arrow origin (near the last device), building feet.
    pub homerun_ft: [f64; 2],
    /// The sheet the homerun is drawn on.
    pub sheet: String,
}

/// A stair core.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stair {
    pub id: String,
    pub tag: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Levels this stair serves (level codes).
    pub levels: Vec<String>,
}

/// An elevator shaft.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Elevator {
    pub id: String,
    pub tag: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub levels: Vec<String>,
}

/// A vertical shaft (plumbing chase, duct shaft, electrical shaft).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Shaft {
    pub id: String,
    pub tag: String,
    /// `plumbing`, `duct`, `electrical`.
    pub kind: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub levels: Vec<String>,
}

// --- Wall assemblies (Q293) ---

/// A wall assembly type from the wall type legend.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WallAssembly {
    pub id: String,
    /// Type tag printed on the plans: `WT-1`, `WT-2`, etc.
    pub type_tag: String,
    pub name: String,
    /// Layers from finish face to finish face.
    pub layers: Vec<WallLayer>,
    /// Overall thickness, inches.
    pub thickness_in: f64,
    /// Fire rating: `""`, `1-HR`, `2-HR`.
    pub fire_rating: String,
    /// `SLAB TO SLAB`, `FLOOR TO CLG`, `FLOOR TO DECK`.
    pub height_rule: String,
    /// STC rating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stc: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WallLayer {
    pub material: String,
    pub thickness_in: f64,
}

/// A wall placed in the building.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WallInstance {
    pub id: String,
    pub assembly: String,
    pub level: String,
    /// Centreline endpoints, building feet.
    pub from_ft: [f64; 2],
    pub to_ft: [f64; 2],
    pub height_ft: f64,
    /// Which sheet shows the wall type tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
}

// --- Ceiling / soffit assemblies (Q295) ---

/// A ceiling assembly type.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CeilingAssembly {
    pub id: String,
    pub type_tag: String,
    pub name: String,
    pub layers: Vec<CeilingLayer>,
    pub total_depth_in: f64,
    /// Fire rating of the assembly.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fire_rating: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CeilingLayer {
    pub material: String,
    pub thickness_in: f64,
}

/// A soffit (dropped ceiling area, bulkhead) placed in the building.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SoffitInstance {
    pub id: String,
    pub assembly: String,
    pub level: String,
    pub room: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Bottom of soffit elevation above the level floor, feet.
    pub bottom_ft: f64,
    /// Shown on this RCP sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
}

// --- Casework (Q301) ---

/// A casework item: cabinet, counter, appliance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CaseworkItem {
    pub id: String,
    /// `base_cabinet`, `upper_cabinet`, `counter`, `vanity`, `appliance`, `wardrobe`.
    pub kind: String,
    pub tag: String,
    pub level: String,
    pub room: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub height_in: f64,
    pub depth_in: f64,
    /// The spec section it comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_section: Option<String>,
}

// --- Spec book (Q298) ---

/// A specification item from the spec book.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpecItem {
    pub id: String,
    /// CSI division: `06`, `09`, `22`, `23`, `26`.
    pub division: String,
    /// Section number: `06 41 16`.
    pub section: String,
    pub title: String,
    /// Invented manufacturer name.
    pub manufacturer: String,
    /// Invented model number.
    pub model: String,
    /// `"or equal"` flag.
    #[serde(default)]
    pub or_equal: bool,
    /// Schedule or equipment tags that reference this.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub referenced_by: Vec<String>,
}

// --- Site / civil (C sheets) ---

/// A site element for the civil plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteElement {
    pub id: String,
    /// `parking_space`, `light_pole`, `transformer_pad`, `dumpster_enclosure`, `fire_hydrant`,
    /// `bollard`, `sign`, `curb`, `sidewalk`, `driveway`, `underground_electric`,
    /// `underground_water`, `underground_sewer`, `underground_storm`, `underground_gas`,
    /// `underground_telecom`.
    pub kind: String,
    pub tag: String,
    /// Position, building feet.
    pub at_ft: [f64; 2],
    /// For area elements: bounding box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_ft: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ft: Option<[f64; 2]>,
    /// For underground: depth below grade, feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_ft: Option<f64>,
}

// --- Fire alarm (FA sheets) ---

/// A fire alarm device.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FireAlarmDevice {
    pub id: String,
    /// `smoke_detector`, `heat_detector`, `pull_station`, `horn_strobe`, `duct_detector`,
    /// `sprinkler_flow_switch`, `tamper_switch`, `annunciator`, `facp`.
    pub kind: String,
    pub tag: String,
    pub level: String,
    pub room: String,
    pub at_ft: [f64; 3],
    /// Mounting: `CLG` (ceiling), `WALL` (wall).
    pub mounting: String,
    /// FA circuit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub circuit: String,
    pub sheet: String,
}

// --- Telecom (T sheets) ---

/// A telecom device or outlet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TelecomDevice {
    pub id: String,
    /// `data_outlet`, `wifi_ap`, `camera`, `door_access`, `tv_outlet`, `telephone`.
    pub kind: String,
    pub tag: String,
    pub level: String,
    pub room: String,
    pub at_ft: [f64; 3],
    /// Mounting height, inches above floor.
    pub mounting_in: f64,
    pub sheet: String,
}

// --- Architectural views (elevations, sections) ---

/// An elevation view drawn on an A-sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElevationView {
    pub id: String,
    pub name: String,
    /// `north`, `south`, `east`, `west`, or `interior`.
    pub direction: String,
    pub sheet: String,
    /// For interior elevations: which room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
}

/// A building section drawn on an A-sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SectionView {
    pub id: String,
    pub name: String,
    /// Section cut line endpoints, building feet.
    pub cut_from_ft: [f64; 2],
    pub cut_to_ft: [f64; 2],
    /// Looking direction: `north`, `south`, `east`, `west`.
    pub looking: String,
    pub sheet: String,
}

// --- Gaps / conflicts for Q296 ---

/// A planted gap or conflict the RFI helper should find.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Gap {
    pub id: String,
    /// `wall_no_tag`, `wall_tag_not_in_legend`, `wall_type_conflict`, `ceiling_no_type`,
    /// `ceiling_no_height`, `soffit_missing_from_section`, `rated_wall_stops_at_ceiling`,
    /// `head_not_specified`, `height_assembly_conflict`.
    pub kind: String,
    pub description: String,
    /// The element ids involved.
    pub elements: Vec<String>,
    pub level: String,
    /// Where on the plan, building feet.
    pub at_ft: [f64; 2],
}

/// A conduit run drawn on the electrical plan (where shown).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElecConduit {
    pub id: String,
    /// `EMT`, `RGS`, `PVC`, `MC` (metal clad cable, drawn as a run but not rigid conduit).
    pub conduit_type: String,
    /// Trade size: `3/4`, `1`, `1-1/4`, etc.
    pub size: String,
    /// Size in inches (for geometric purposes).
    pub size_in: f64,
    pub level: String,
    pub from_ft: [f64; 2],
    pub to_ft: [f64; 2],
    /// The tag as printed (`3/4"C`, `1"C`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<Tag>,
    pub sheet: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Riser {
    pub id: String,
    pub system: String,
    pub at_ft: [f64; 2],
    pub size_in: [f64; 2],
    pub bottom_ft: f64,
    pub top_ft: f64,
    /// The vertical segments it is made of, bottom to top.
    pub segments: Vec<String>,
    /// Per level it shows on: the symbol there (`up`: it rises from this level; `down`: it comes
    /// from below and ends or turns here; `through`: it passes) and the note printed beside it.
    pub shown: Vec<RiserMark>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RiserMark {
    pub level: String,
    pub symbol: String,
    pub note: String,
}

/// An architectural wall segment between or around rooms.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wall {
    pub id: String,
    pub level: String,
    /// Wall start point, building feet (plan).
    pub from_ft: [f64; 2],
    /// Wall end point, building feet (plan).
    pub to_ft: [f64; 2],
    /// Wall thickness, inches (stud + finish).
    pub thickness_in: f64,
    /// Stud spacing, inches on centre.
    pub stud_spacing_in: f64,
    /// Room ids on either side (exterior side is `"EXT"`).
    pub sides: [String; 2],
}

/// An in-wall rough-in stub behind a plumbing fixture.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoughIn {
    pub id: String,
    /// The fixture this rough-in serves.
    pub fixture: String,
    /// The wall the stub is inside.
    pub wall: String,
    /// `CW`, `HW`, `SAN`, `V`.
    pub system: String,
    /// `supply`, `waste`, `vent`.
    pub kind: String,
    /// Centre of the stub, building feet.
    pub at_ft: [f64; 3],
    /// Pipe size, nominal inches.
    pub size_in: f64,
    /// Height above finished floor, inches.
    pub aff_in: f64,
    /// Offset from the fixture centreline along the wall, inches.
    pub offset_in: f64,
    /// The stud cavity this stub occupies (0-based from the wall start).
    pub stud_cavity: usize,
    /// Whether this rough-in is assumed from rules (true) or explicitly shown on drawings (false).
    pub assumed: bool,
}

/// An electrical device placed on a wall.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub tag: String,
    /// `receptacle`, `switch`, `junction_box`.
    pub kind: String,
    pub level: String,
    pub room: String,
    /// The wall the device is mounted on.
    pub wall: String,
    /// Position on the plan, building feet.
    pub at_ft: [f64; 2],
    /// Mounting height above finished floor, inches.
    pub aff_in: f64,
    /// Box width × height × depth, inches.
    pub box_in: [f64; 3],
    /// The stud cavity this box occupies (0-based from the wall start).
    pub stud_cavity: usize,
    /// Whether the device position is fixed by the drawings or has a placement zone.
    pub fixed: bool,
    /// The sheet this device appears on.
    pub sheet: String,
}

/// Rough-in rules: per fixture type, the expected stubs at standard positions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoughInRules {
    pub rules: Vec<RoughInRule>,
}

/// One fixture type's rough-in stub positions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoughInRule {
    /// `water_closet`, `lavatory`, `urinal`, `shower`, `bathtub`, `mop_sink`,
    /// `floor_drain`, `floor_sink`, `drinking_fountain`, `sink`.
    pub fixture_kind: String,
    /// The stubs this fixture type generates.
    pub stubs: Vec<StubRule>,
}

impl RoughInRules {
    /// Standard rough-in positions from common plumbing practice (IPC / UPC).
    pub fn default_rules() -> Self {
        let s = |sys: &str, kind: &str, aff: f64, off: f64, size: f64| StubRule {
            system: sys.into(),
            kind: kind.into(),
            aff_in: aff,
            offset_in: off,
            size_in: size,
        };
        Self {
            rules: vec![
                RoughInRule {
                    fixture_kind: "water_closet".into(),
                    stubs: vec![
                        s("CW", "supply", 6.0, -8.0, 0.5),  // left of centre, 6" AFF
                        s("SAN", "waste", 0.0, 0.0, 3.0),    // centreline, at floor
                        s("V", "vent", 20.0, -6.0, 2.0),     // left of waste, 20" AFF
                    ],
                },
                RoughInRule {
                    fixture_kind: "lavatory".into(),
                    stubs: vec![
                        s("CW", "supply", 20.0, -4.0, 0.5),  // left, 20" AFF
                        s("HW", "supply", 20.0, 4.0, 0.5),   // right, 20" AFF
                        s("SAN", "waste", 18.0, 0.0, 1.5),    // centreline, 18" AFF
                        s("V", "vent", 24.0, 4.0, 1.5),       // right of waste, 24" AFF
                    ],
                },
                RoughInRule {
                    fixture_kind: "urinal".into(),
                    stubs: vec![
                        s("CW", "supply", 42.0, 0.0, 0.75),   // centreline, 42" AFF
                        s("SAN", "waste", 0.0, 0.0, 2.0),
                        s("V", "vent", 24.0, 4.0, 1.5),
                    ],
                },
                RoughInRule {
                    fixture_kind: "shower".into(),
                    stubs: vec![
                        s("CW", "supply", 48.0, -3.0, 0.5),
                        s("HW", "supply", 48.0, 3.0, 0.5),
                        s("SAN", "waste", 0.0, 0.0, 2.0),
                        s("V", "vent", 24.0, 6.0, 1.5),
                    ],
                },
                RoughInRule {
                    fixture_kind: "mop_sink".into(),
                    stubs: vec![
                        s("CW", "supply", 24.0, -4.0, 0.75),
                        s("HW", "supply", 24.0, 4.0, 0.75),
                        s("SAN", "waste", 0.0, 0.0, 3.0),
                        s("V", "vent", 30.0, 6.0, 2.0),
                    ],
                },
                RoughInRule {
                    fixture_kind: "floor_drain".into(),
                    stubs: vec![
                        s("SAN", "waste", 0.0, 0.0, 2.0),
                    ],
                },
                RoughInRule {
                    fixture_kind: "sink".into(),
                    stubs: vec![
                        s("CW", "supply", 20.0, -4.0, 0.5),
                        s("HW", "supply", 20.0, 4.0, 0.5),
                        s("SAN", "waste", 18.0, 0.0, 1.5),
                        s("V", "vent", 24.0, 4.0, 1.5),
                    ],
                },
            ],
        }
    }

    /// Get the rule for a fixture kind.
    pub fn for_kind(&self, kind: &str) -> Option<&RoughInRule> {
        self.rules.iter().find(|r| r.fixture_kind == kind)
    }
}

/// Where one rough-in stub sits relative to the fixture.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StubRule {
    /// `CW`, `HW`, `SAN`, `V`.
    pub system: String,
    /// `supply`, `waste`, `vent`.
    pub kind: String,
    /// Height above finished floor, inches.
    pub aff_in: f64,
    /// Offset from fixture centreline along the wall, inches (positive = right facing the wall).
    pub offset_in: f64,
    /// Nominal pipe size, inches.
    pub size_in: f64,
}

/// A placement zone for a device whose position is not fixed on the drawings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlacementZone {
    /// The device id.
    pub device: String,
    /// Zone limits along the wall from its start, inches.
    pub zone_in: [f64; 2],
    /// `will_clash` (every position conflicts), `may_clash` (some positions conflict),
    /// `clear` (no conflicts).
    pub status: String,
    /// Ranges within the zone that are free of in-wall conflicts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clear_ranges: Vec<ClearRange>,
    /// Description of the conflicts for reporting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
}

/// A range along a wall where a device can be placed without in-wall conflicts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClearRange {
    /// Start of the clear range, inches from wall start.
    pub from_in: f64,
    /// End of the clear range, inches from wall start.
    pub to_in: f64,
    /// Human-readable description.
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    /// `M-101`.
    pub number: String,
    pub title: String,
    /// `G`, `A`, `S`, `M`, `P`, `FP`.
    pub discipline: String,
    /// The level the plan shows (none for the cover and schedule sheets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    pub pdf: String,
    pub dxf: String,
    /// Width × height, inches (landscape).
    pub paper_in: [f64; 2],
    /// The plan's scale as printed (`1/8" = 1'-0"`), for plan sheets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_text: Option<String>,
    /// Enlarged plans: the part of the level they show (building feet, min and max corners).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ft: Option<[[f64; 2]; 2]>,
    /// Paper inches per building foot (1/8" = 1'-0" → 0.125).
    pub scale: f64,
    /// Building feet → drawing (model space, inches): rotate by `rotation_deg` about the origin
    /// after scaling to inches, then add `offset_in`.
    pub model_from_building: Placement,
    /// Drawing (model inches) → paper inches: `(model - origin_in) * scale`, paper origin at the
    /// sheet's lower-left corner.
    pub paper_from_model: PaperMap,
    /// The legend's entries as printed (symbol name and its text).
    pub legend: Vec<LegendItem>,
    /// Abbreviations listed on the sheet.
    pub abbreviations: Vec<LegendItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub rotation_deg: f64,
    pub offset_in: [f64; 2],
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaperMap {
    pub origin_in: [f64; 2],
    pub scale: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LegendItem {
    pub symbol: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Clash {
    pub id: String,
    /// `hard` (two elements overlap), `below_ceiling` (a run hangs below the room's ceiling),
    /// `into_structure` (a run reaches into a beam), `in_wall` (a device shares a stud cavity
    /// or is within clearance of a pipe/stub/stack inside the same wall).
    pub kind: String,
    /// Element ids (segments, equipment, beams, rooms, devices, rough-ins).
    pub a: String,
    pub b: String,
    /// The middle of the overlap.
    pub at_ft: [f64; 3],
    /// How far they overlap, inches (the smallest of the overlap's sizes).
    pub depth_in: f64,
    pub planted: bool,
    /// For `in_wall` clashes: human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// For `in_wall` clashes: the wall the clash is inside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall: Option<String>,
}

impl Truth {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn segment(&self, id: &str) -> Option<&Segment> {
        self.segments.iter().find(|s| s.id == id)
    }
    pub fn level(&self, code: &str) -> Option<&Level> {
        self.building.levels.iter().find(|l| l.code == code)
    }
    pub fn system(&self, code: &str) -> Option<&System> {
        self.systems.iter().find(|s| s.code == code)
    }
    /// The segments of one system kind (`duct` / `pipe`).
    pub fn segments_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Segment> + 'a {
        self.segments.iter().filter(move |s| s.kind == kind)
    }
    pub fn wall(&self, id: &str) -> Option<&Wall> {
        self.building.walls.iter().find(|w| w.id == id)
    }
    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }
    pub fn rough_in(&self, id: &str) -> Option<&RoughIn> {
        self.rough_ins.iter().find(|r| r.id == id)
    }
}

impl Segment {
    /// Length, feet.
    pub fn length_ft(&self) -> f64 {
        let [a, b] = self.path_ft;
        ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
    }
    /// The size as tags print it: `24x14`, `12"Ø`, `2"`.
    pub fn size_label(&self) -> String {
        match self.shape.as_str() {
            "rect" => format!("{}x{}", fmt_in(self.size_in[0]), fmt_in(self.size_in[1])),
            "round" => format!("{}\"Ø", fmt_in(self.size_in[0])),
            _ => format!("{}\"", pipe_size(self.size_in[0])),
        }
    }
}

/// Whole inches without a decimal point.
pub fn fmt_in(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{v}") }
}

/// A nominal pipe size as printed: `3/4`, `1`, `1-1/4`, `1-1/2`, `2`, `2-1/2`.
pub fn pipe_size(v: f64) -> String {
    let whole = v.trunc();
    let frac = v - whole;
    let f = [(0.25, "1/4"), (0.5, "1/2"), (0.75, "3/4")].iter().find(|(x, _)| (frac - x).abs() < 1e-6).map(|(_, t)| *t);
    match (whole as i64, f) {
        (0, Some(t)) => t.to_string(),
        (w, Some(t)) => format!("{w}-{t}"),
        (w, None) => format!("{w}"),
    }
}

/// Feet as printed on prints: `10'-6"`, `9'-0"`, `8'-7 1/2"`.
pub fn feet_inches(ft: f64) -> String {
    let neg = ft < 0.0;
    let total = (ft.abs() * 24.0).round() / 2.0; // half inches
    let feet = (total / 12.0).floor();
    let inches = total - feet * 12.0;
    let i = if (inches - inches.round()).abs() < 1e-9 { format!("{}", inches.round() as i64) } else { format!("{} 1/2", inches.floor() as i64) };
    format!("{}{}'-{}\"", if neg { "-" } else { "" }, feet as i64, i)
}
