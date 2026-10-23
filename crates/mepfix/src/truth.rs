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
    /// Feeders between the electrical gear (the feeder schedule).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elec_feeders: Vec<cadaddy_walk3d::Feeder>,
    /// The problems planted in the gear and its feeders: what the gear review must find
    /// (gear tag, check: `lug count`, `lug range`, `lug material`, `neutral bar`, `ground bar`,
    /// `entry`, `gutter`, `bending`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gear_problems: Vec<(String, String)>,
    /// Device subscripts, circuits beside devices, breaker types, homerun conductors and planted
    /// tagging problems (Q299).
    #[serde(default, skip_serializing_if = "crate::elecspec::ElecSpec::is_empty")]
    pub elec_spec: crate::elecspec::ElecSpec,
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
    pub grading_contours: Vec<GradingContour>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub site_structures: Vec<SiteStructure>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub site_quantities: Vec<SiteQuantity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fire_alarm_devices: Vec<FireAlarmDevice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub telecom_devices: Vec<TelecomDevice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equip_connections: Vec<EquipConnection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sleeves: Vec<Sleeve>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cable_pathways: Vec<CablePathway>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lv_bom: Vec<LvBomLine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elevations: Vec<ElevationView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<SectionView>,
    /// Callout symbols drawn on the sheets, each linking to a target sheet/detail.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub callouts: Vec<CalloutEntry>,
    /// Planted gaps and conflicts for the RFI helper (Q296).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<Gap>,
    /// Fire protection head schedule: sprig length (inches) for upright heads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fp_sprig_in: Option<f64>,
    /// Fire protection head schedule: sidewall offset below ceiling (inches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fp_sidewall_below_in: Option<f64>,
    // --- Unit types (Q305) ---
    /// Typical unit types: room templates that repeat across floors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unit_types: Vec<UnitType>,
    /// Placed instances of unit types on each floor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unit_instances: Vec<UnitInstance>,
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
    /// The ceiling type legend: every ceiling build-up (boards on their framing, tile in a grid,
    /// open to structure) the rooms' ceilings are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ceiling_types: Vec<CeilingType>,
    /// The soffit details: how each kind of soffit is built and what it hangs from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub soffit_types: Vec<SoffitType>,
    /// Each room's ceiling type (open rooms too) and whether the finish schedule names it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub room_ceilings: Vec<RoomCeiling>,
    /// The head of wall details: how each wall type meets what is above it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub head_details: Vec<HeadDetail>,
    /// Sleeve and pathway notes on the floor plans (sets with planted gaps only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sleeve_notes: Vec<SleeveNote>,
    /// Gaps and conflicts put into the prints on purpose (sets with planted gaps only): what an
    /// RFI list should say besides the gaps every set has (untagged walls, unscheduled rooms…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub planted: Vec<PlantedGap>,
    /// Doors, windows, the stair and elevator, slab edges, the roof and room finishes (Q307).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<Arch>,
    /// The roof and wall envelope: assemblies, drainage and flashing, details, spec, takeoff and
    /// coordination flags (Q337).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Envelope>,
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
    /// The RCP tag and the finish schedule leave its height out (a planted gap).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub height_hidden: bool,
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
    /// How it is built ([`SoffitType::tag`]); none on soffits from before the soffit details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soffit_type: Option<String>,
    /// From the ceiling round it down to its bottom, feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_ft: Option<f64>,
    /// Length of its sides that show (not against a wall), feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sides_ft: Option<f64>,
    /// Whether the RCP calls out its detail (otherwise the reader has to work it out).
    #[serde(default)]
    pub called_out: bool,
    /// The detail number its callout points to when that isn't its type's (a planted gap: a
    /// callout to a detail the details sheet doesn't have).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callout_detail: Option<String>,
}

/// A ceiling type as the ceiling legend gives it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CeilingType {
    /// As the legend and the finish schedule print it (`C2`, `CLG-2`, `GYP-1`).
    pub tag: String,
    pub name: String,
    /// `ACT`, `GYP` or `OPEN` (exposed structure).
    pub kind: String,
    /// From the room's face up: the boards (or tiles), then their framing. Material names as
    /// [`WallTypeLayer::material`] plus `act_tile`, `act_grid`, `hat_channel`, `furring`,
    /// `main_runner` (suspended framing's carrying channels), `wood_furring`; side `finish` or
    /// `framing`.
    pub layers: Vec<WallTypeLayer>,
    /// ACT tile size, feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_ft: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_rating: Option<String>,
}

/// A room's ceiling type.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoomCeiling {
    pub level: String,
    /// The room's number.
    pub room: String,
    /// [`CeilingType::tag`].
    pub ceiling_type: String,
    /// The finish schedule names the type (otherwise it says `SEE RCP`).
    pub scheduled: bool,
}

/// A soffit build-up as its detail gives it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SoffitType {
    /// The detail's number on the details sheet (`3`); the RCP calls it out as `3/A-502`.
    pub tag: String,
    pub name: String,
    /// The bottom's boards, room face first.
    pub bottom: Vec<WallTypeLayer>,
    /// The vertical faces' boards, room face first.
    pub sides: Vec<WallTypeLayer>,
    /// Its framing (studs for the sides, joists or channels for the bottom).
    pub framing: Vec<WallTypeLayer>,
    /// `ceiling`: built under the ceiling, which runs on above it; `structure`: hung from the
    /// structure, the ceiling stops at its sides.
    pub hangs_from: String,
    /// How far its side boards go past the ceiling, inches (hung soffits).
    #[serde(default)]
    pub above_ceiling_in: f64,
}

/// A head of wall detail and the wall types it is for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HeadDetail {
    /// The detail's number on the details sheet.
    pub tag: String,
    pub name: String,
    /// `deflection` (slotted deflection track), `double` (nested double track), `single` (top
    /// track, braced to the structure), `angle` (steel angles each side, masonry).
    pub track: String,
    /// `deck` (the underside of the structure) or `ceiling`.
    pub to: String,
    /// The deflection the head takes, inches (deflection and double tracks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deflection_in: Option<f64>,
    /// The wall types it is for (their tags).
    pub wall_types: Vec<String>,
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
    /// How its head meets what is above ([`HeadDetail::tag`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Bottom track (stud walls), none for masonry.
    #[serde(default)]
    pub bottom_track: bool,
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
    /// The end its studs were laid out from (`a` or `b`; walls stacked storey over storey share
    /// one): what a print reader can't know until a shop drawing or a field point says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_from: Option<String>,
    /// The tag the floor plan prints when it isn't `wall_type` (a planted gap: a tag the legend
    /// doesn't have).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_tag: Option<String>,
    /// The tag an enlarged plan prints for it when that differs from the floor plan's (a planted
    /// conflict between sheets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enlarged_tag: Option<String>,
}

/// A sleeve or pathway note on a floor plan: `count` × `size_in` sleeves at `at_ft`, and the
/// route (where to) and method (through what, how) the note gives, if any.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SleeveNote {
    pub id: String,
    pub level: String,
    pub at_ft: [f64; 2],
    /// The note as printed.
    pub text: String,
    pub count: usize,
    pub size_in: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Cored through the slab onto a beam below it: it won't fit where it is drawn.
    #[serde(default)]
    pub on_beam: bool,
}

/// A gap or conflict planted in a set: its kind (an [`crate::rfi::RfiKind`] name) and what it
/// is about (a wall id, a wall type tag, a room `level/number`, a soffit id, a sleeve id…).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlantedGap {
    pub kind: String,
    pub about: String,
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
    /// Sprinkler heads: `pendent` (below a ceiling), `upright` (on a sprig, open to structure),
    /// `sidewall` (on a wall, under the ceiling).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
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

/// An electrical panel, transformer, switchboard or disconnect.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElecPanel {
    pub id: String,
    pub tag: String,
    /// `panel`, `transformer`, `switchboard`, `disconnect`.
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
    /// The equipment schedule: width, depth and height (inches), the bottom above the floor
    /// (inches), and its columns as printed (`TYPE`, `VOLTAGE`, `RATING`, `MOUNTING`…).
    #[serde(default)]
    pub size_in: [f64; 3],
    #[serde(default)]
    pub bottom_in: f64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
    /// The lug schedule's rows for it, and its entry plates and gutter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terminations: Vec<cadaddy_walk3d::Termination>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enclosure: Option<cadaddy_walk3d::Enclosure>,
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
    /// `bollard`, `sign`, `curb`, `sidewalk`, `driveway`, `striping`, `building_pad`,
    /// `paving_asphalt`, `paving_concrete`, `underground_electric`, `underground_water`,
    /// `underground_sewer`, `underground_storm`, `underground_gas`, `underground_telecom`,
    /// `manhole`, `catch_basin`, `cleanout`, `valve_box`, `bench`, `bike_rack`, `trash_can`,
    /// `planter`, `tree`, `shrub`, `irrigation_head`, `fence`, `gate`, `retaining_wall`.
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
    /// Height for retaining walls, fencing, feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_ft: Option<f64>,
    /// Material: `asphalt`, `concrete`, `chain_link`, `wood`, `aluminum`, etc.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub material: String,
    /// Length for linear elements (curbs, fences, retaining walls), feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_ft: Option<f64>,
    /// Area for surface elements (paving, building pad), square feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_sf: Option<f64>,
    /// Slope in percent for graded surfaces and utility runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slope_pct: Option<f64>,
    /// Pipe size for utility runs, nominal inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_in: Option<f64>,
}

/// A grading contour on the civil grading plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GradingContour {
    pub id: String,
    /// `existing` or `proposed`.
    pub surface: String,
    /// Elevation, feet above datum.
    pub elevation_ft: f64,
    /// Polyline vertices in plan, building feet.
    pub points: Vec<[f64; 2]>,
    /// `major` (every 5 ft) or `minor` (every 1 ft).
    pub weight: String,
}

/// A site utility structure (manhole, catch basin, cleanout, valve box).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteStructure {
    pub id: String,
    /// `manhole`, `catch_basin`, `cleanout`, `valve_box`, `junction_box`,
    /// `meter`, `backflow_preventer`.
    pub kind: String,
    pub tag: String,
    /// Position, building feet.
    pub at_ft: [f64; 2],
    /// Rim elevation, feet above datum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rim_ft: Option<f64>,
    /// Invert elevation (pipe bottom), feet above datum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert_ft: Option<f64>,
    /// Which utility system: `storm`, `sanitary`, `water`, `electric`, `telecom`, `gas`.
    pub system: String,
    /// Diameter or size, inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_in: Option<f64>,
}

/// A site quantity for the civil takeoff.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteQuantity {
    pub id: String,
    /// CSI section: `31 22 00` (grading), `32 11 23` (aggregate base), `32 12 16` (asphalt),
    /// `32 13 13` (concrete paving), `32 16 13` (curbs), `32 31 13` (fencing), etc.
    pub csi: String,
    pub description: String,
    /// 0 when the status is `insufficient data`.
    pub quantity: f64,
    /// `SF`, `LF`, `CY`, `EA`, `SY`, `TON`, `GAL`.
    pub unit: String,
    /// The sheets the quantity is measured on (`C-101`, `C-102`, `C-103`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// `read` (measured off the drawing), `rule` (derived from read values by a stated rule),
    /// `assumed`, or `insufficient data` (no number).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// The elements (site elements, structures, contours) that make up the quantity, each with
    /// its share; the shares add up to the quantity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<SiteQuantityPart>,
}

/// One element's share of a [`SiteQuantity`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteQuantityPart {
    pub id: String,
    pub quantity: f64,
}

// --- Fire alarm (FA sheets) ---

/// A fire alarm device.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FireAlarmDevice {
    pub id: String,
    /// `smoke_detector`, `heat_detector`, `pull_station`, `horn_strobe`, `strobe`,
    /// `speaker`, `speaker_strobe`, `duct_detector`, `co_detector`,
    /// `sprinkler_flow_switch`, `tamper_switch`, `annunciator`, `facp`,
    /// `nac_panel`, `monitor_module`, `relay_module`,
    /// `elevator_recall`, `door_holder`, `mag_door_holder`.
    pub kind: String,
    pub tag: String,
    pub level: String,
    pub room: String,
    pub at_ft: [f64; 3],
    /// Mounting: `CLG` (ceiling), `WALL` (wall).
    pub mounting: String,
    /// Mounting height above floor, inches. `0` = ceiling.
    #[serde(default)]
    pub mounting_in: f64,
    /// Candela rating for notification appliances (horn/strobe, strobe, speaker/strobe):
    /// 15, 30, 75, 110, 177 cd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candela: Option<u32>,
    /// Address or zone tag on the device (addressable systems).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// SLC (signalling line circuit) tag: `SLC-1`, `SLC-2`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slc: String,
    /// NAC (notification appliance circuit) tag: `NAC-1`, `NAC-2`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub nac: String,
    /// FA circuit (legacy field kept for backward compat; prefer slc/nac).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub circuit: String,
    /// NAC panel or power supply feeding this device, when separate from the FACP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nac_panel: Option<String>,
    /// Interface notes: `elevator_recall`, `door_holder`, `sprinkler_flow_tamper`, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    pub sheet: String,
}

// --- Telecom (T sheets) ---

/// A telecom device or outlet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TelecomDevice {
    pub id: String,
    /// `data_outlet`, `wifi_ap`, `camera`, `door_access`, `card_reader`, `rex`,
    /// `mag_lock`, `door_contact`, `door_ajar`, `tv_outlet`, `telephone`,
    /// `nurse_call_station`, `nurse_call_display`, `ada_pull_cord`,
    /// `speaker`, `pool_alarm`, `door_alarm`, `idf`, `mdf`, `patch_panel`,
    /// `backboard`, `rack`.
    pub kind: String,
    pub tag: String,
    pub level: String,
    pub room: String,
    pub at_ft: [f64; 3],
    /// Mounting height, inches above floor. `0` = ceiling.
    pub mounting_in: f64,
    /// LV system: `data`, `tv`, `telephone`, `security`, `access_control`,
    /// `nurse_call`, `sound`, `fire_alarm`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lv_system: String,
    /// Cable count and type per outlet (e.g. `"2 × Cat6"`, `"1 × RG6"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cable_spec: Option<String>,
    /// Box or bracket type: `"single_gang"`, `"double_gang"`, `"lv_bracket"`, `"4S"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_type: Option<String>,
    /// Faceplate type: `"single"`, `"double"`, `"quad"`, `"blank"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faceplate: Option<String>,
    /// The IDF/MDF/head-end this device homes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_panel: Option<String>,
    pub sheet: String,
}

// --- Equipment connections (Q308) ---

/// An equipment connection: a piece of equipment with its electrical feed (disconnect,
/// receptacle, junction box) as shown on schedules and enlarged plans.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EquipConnection {
    pub id: String,
    /// `ptac`, `vtac`, `rtu`, `water_heater`, `kitchen_equip`, `washer`, `dryer`,
    /// `pool_pump`, `pool_heater`, `elevator`, `fire_pump`, `signage`, `exhaust_fan`,
    /// `garbage_disposal`, `dishwasher`, `ice_machine`, `vending`.
    pub kind: String,
    pub tag: String,
    pub level: String,
    pub room: String,
    pub at_ft: [f64; 3],
    /// The circuit feeding this connection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub circuit: String,
    /// The panel feeding this connection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub panel: String,
    /// Connection hardware: `disconnect`, `receptacle`, `junction_box`, `whip`.
    pub connection_type: String,
    /// The disconnect or receptacle tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disconnect_tag: Option<String>,
    /// Notes from the schedule (voltage, phase, MCA, MOCP, etc.).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub schedule: BTreeMap<String, String>,
    pub sheet: String,
}

// --- Sleeves (Q308) ---

/// A sleeve (penetration) where a cable pathway crosses a wall, floor, rated assembly,
/// or framing member.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sleeve {
    pub id: String,
    /// LV system(s) passing through: `data`, `fire_alarm`, `security`, etc.
    pub systems: Vec<String>,
    pub level: String,
    /// Centre of the sleeve, building feet.
    pub at_ft: [f64; 3],
    /// What it penetrates: `wall`, `floor`, `rated_wall`, `rated_floor`, `beam`.
    pub penetrates: String,
    /// ID of the wall or floor element it crosses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    /// Material: `EMT`, `ENT`, `firestop_device`.
    pub material: String,
    /// Trade size, inches.
    pub size_in: f64,
    /// Length through the assembly, inches.
    pub length_in: f64,
    /// Fire rating of the assembly crossed: `""`, `1-HR`, `2-HR`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fire_rating: String,
    /// Firestop system for rated assemblies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firestop_system: Option<String>,
    /// Cables through this sleeve (count × type).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cables: Vec<String>,
    pub sheet: String,
}

// --- Cable pathways (Q308) ---

/// A low-voltage cable pathway segment (J-hook, cable tray, conduit stub, ENT).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CablePathway {
    pub id: String,
    /// `j_hook`, `cable_tray`, `conduit`, `ent`, `d_ring`.
    pub kind: String,
    /// The LV system(s) this pathway carries.
    pub systems: Vec<String>,
    pub level: String,
    pub from_ft: [f64; 2],
    pub to_ft: [f64; 2],
    /// Elevation of the pathway, feet above level floor.
    pub elevation_ft: f64,
    /// For conduit/ENT: trade size, inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_in: Option<f64>,
    /// For J-hooks: spacing along the run, inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spacing_in: Option<f64>,
    /// For cable tray: width × depth, inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tray_size_in: Option<[f64; 2]>,
    pub sheet: String,
}

// --- Low-voltage BOM line (Q308) ---

/// A bill of materials line item for low-voltage systems.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LvBomLine {
    pub id: String,
    /// LV system: `data`, `fire_alarm`, `security`, `access_control`, `nurse_call`,
    /// `tv`, `sound`, `telephone`.
    pub system: String,
    /// Item category: `device`, `cable`, `pathway`, `sleeve`, `firestop`, `support`,
    /// `faceplate`, `box`, `panel`, `rack`, `misc`.
    pub category: String,
    /// Description.
    pub description: String,
    /// Quantity (count for devices, linear feet for cable/conduit).
    pub quantity: f64,
    /// Unit: `EA`, `LF`, `LOT`.
    pub unit: String,
    /// Whether this is from actual documents or assumed from rules.
    #[serde(default)]
    pub assumed: bool,
    /// Scope flag: `ours`, `lv_vendor`, `gc`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    /// Spec item reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<String>,
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

// --- Callouts (Q306) ---

/// A callout symbol drawn on a sheet, pointing at a detail/view on another sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CalloutEntry {
    pub id: String,
    /// `section_mark`, `elevation_tag`, `detail_bubble`, `enlarged_plan`, `match_line`, `key_plan`.
    pub kind: String,
    /// The sheet the callout is drawn on (its number, e.g. `A-101`).
    pub on_sheet: String,
    /// The detail/view number shown in the bubble (top half), e.g. `1`, `A`.
    pub detail: String,
    /// The target sheet number shown in the bubble (bottom half), e.g. `A-501`.
    pub target_sheet: String,
    /// Position on the plan, building feet.
    pub at_ft: [f64; 2],
    /// For section marks: the cut line endpoints, building feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_from_ft: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_to_ft: Option<[f64; 2]>,
    /// For section/elevation marks: the looking direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub looking: Option<String>,
    /// Whether this callout is a planted broken reference (target doesn't exist).
    #[serde(default)]
    pub broken: bool,
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

// --- Unit types (Q305) ---

/// A typical unit type: a room template (geometry, devices, finishes) that repeats across floors.
/// Hotels are mostly repeats: a King room, a Double Queen, an ADA King, a Suite. Each type
/// defines the canonical layout; instances on the floor plans mirror and/or override it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UnitType {
    pub id: String,
    /// Human-readable name: `"KING ROOM"`, `"SUITE"`.
    pub name: String,
    /// Width along the corridor (x), feet.
    pub width_ft: f64,
    /// Depth into the building (y), feet.
    pub depth_ft: f64,
    /// Ceiling height, feet.
    pub ceiling_height_ft: f64,
    /// Ceiling kind: `ACT` or `GYP`.
    pub ceiling_kind: String,
    /// Wall assembly tag for the room's perimeter: `WT-3`.
    pub wall_assembly: String,
    /// Whether the type is ADA-accessible.
    #[serde(default)]
    pub ada: bool,
    /// Expected electrical devices per room, by kind and count. Kind matches `ElecDevice::kind`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub devices: BTreeMap<String, u32>,
    /// Expected terminals per room, by kind and count. Kind matches `Terminal::kind`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub terminals: BTreeMap<String, u32>,
    /// Expected plumbing fixtures per room, by kind and count.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fixtures: BTreeMap<String, u32>,
    /// Expected casework per room, by kind and count.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub casework: BTreeMap<String, u32>,
    /// The PTAC/VTAC sleeve side in the canonical (un-mirrored) orientation: `left` or `right`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hvac_sleeve_side: String,
    /// The wet wall side in the canonical orientation: `left` or `right`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub wet_wall_side: String,
}

/// A placed instance of a unit type on a floor.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UnitInstance {
    pub id: String,
    /// References `UnitType::id`.
    pub unit_type: String,
    /// The room id this instance occupies.
    pub room: String,
    /// Level code.
    pub level: String,
    /// Room number as printed on the plans.
    pub number: String,
    /// Handing: `normal` (same as the type's canonical layout) or `mirrored` (flipped across
    /// the room's long axis — north-side rooms in a double-loaded corridor).
    pub hand: String,
    /// Position of the room's south-west corner (or the canonical corner), building feet.
    pub at_ft: [f64; 2],
    /// Override fields: keys are field names (`ceiling_height_ft`, `ada`, `wall_assembly`…),
    /// values are the overridden value as a string. An empty map means the instance matches the
    /// type exactly.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, String>,
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
                        s("CW", "supply", 6.0, -8.0, 0.5), // left of centre, 6" AFF
                        s("SAN", "waste", 0.0, 0.0, 3.0),  // centreline, at floor
                        s("V", "vent", 20.0, -6.0, 2.0),   // left of waste, 20" AFF
                    ],
                },
                RoughInRule {
                    fixture_kind: "lavatory".into(),
                    stubs: vec![
                        s("CW", "supply", 20.0, -4.0, 0.5), // left, 20" AFF
                        s("HW", "supply", 20.0, 4.0, 0.5),  // right, 20" AFF
                        s("SAN", "waste", 18.0, 0.0, 1.5),  // centreline, 18" AFF
                        s("V", "vent", 24.0, 4.0, 1.5),     // right of waste, 24" AFF
                    ],
                },
                RoughInRule {
                    fixture_kind: "urinal".into(),
                    stubs: vec![
                        s("CW", "supply", 42.0, 0.0, 0.75), // centreline, 42" AFF
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
                RoughInRule { fixture_kind: "floor_drain".into(), stubs: vec![s("SAN", "waste", 0.0, 0.0, 2.0)] },
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

// --- The rest of the architecture (Q307) ---

/// What the architectural sheets show besides walls: doors and their schedule, windows,
/// the stair and elevator with their shafts, slab edges, the roof and the room finish schedule.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Arch {
    pub doors: Vec<Door>,
    /// The hardware groups the door schedule refers to.
    pub hardware: Vec<HardwareGroup>,
    pub windows: Vec<Window>,
    /// Window types as the window schedule lists them.
    pub window_types: Vec<WindowType>,
    pub stairs: Vec<ArchStair>,
    pub elevators: Vec<ArchElevator>,
    pub slab_edges: Vec<SlabEdge>,
    pub roof: Roof,
    pub finishes: Vec<RoomFinish>,
    /// The finish legend: every code the finish schedule uses.
    pub finish_codes: Vec<FinishCode>,
}

/// A door: its schedule row and where it is on the plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Door {
    /// The mark on the plan and in the schedule (`103`, `103A`, `D103`).
    pub mark: String,
    pub level: String,
    /// The wall it is in.
    pub wall: String,
    /// Middle of the opening on the wall's centreline, building feet.
    pub at_ft: [f64; 2],
    /// Hinge and latch jambs, on the face it swings from, building feet.
    pub hinge_ft: [f64; 2],
    pub latch_ft: [f64; 2],
    pub width_in: f64,
    pub height_in: f64,
    /// Door type letter in the schedule (`A` flush, `B` half lite, `C` narrow lite, `D` full glass).
    pub door_type: String,
    /// `WD`, `HM`, `AL`.
    pub material: String,
    /// Frame: `HM`, `AL`, `WD`.
    pub frame: String,
    /// `20 MIN`, `60 MIN`, `90 MIN`; none = not rated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_rating: Option<String>,
    /// Hardware group (`HW-3`).
    pub hardware: String,
    /// The room it swings into (none: out of the building).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swings_into: Option<String>,
    /// The room on its other side (none: outside).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_room: Option<String>,
    /// Handing seen from the outside (the exterior, else the corridor): `LH`, `RH`, `LHR`, `RHR`.
    pub hand: String,
}

/// A hardware group: what a door with it gets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HardwareGroup {
    pub group: String,
    /// The schedule's words, e.g. `STOREROOM LOCK, CLOSER`.
    pub items: Vec<String>,
    /// An electrified lock and card reader (the electrician's box and conduit).
    pub card_reader: bool,
}

/// A window type in the window schedule.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowType {
    pub mark: String,
    pub description: String,
    pub width_in: f64,
    pub height_in: f64,
    /// Above the floor, inches (a storefront's is 0).
    pub sill_in: f64,
    pub frame: String,
    pub glazing: String,
    pub storefront: bool,
}

/// A window on a plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub mark: String,
    pub level: String,
    pub wall: String,
    /// Middle of it on the wall's centreline, building feet.
    pub at_ft: [f64; 2],
    /// From and to along the wall from its `a_ft`, feet.
    pub along_ft: [f64; 2],
}

/// A stair with its shaft: the flights as drawn on each level it serves.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ArchStair {
    pub tag: String,
    /// The room (shaft) it is in, by level: the room number on each level.
    pub rooms: Vec<String>,
    /// Levels from the bottom one up.
    pub levels: Vec<String>,
    /// The flights' and landing's outline, building feet.
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Clear width of a flight, inches.
    pub width_in: f64,
    /// Risers per storey, riser height and tread depth, inches.
    pub risers: u32,
    pub riser_in: f64,
    pub tread_in: f64,
}

/// An elevator: its hoistway (shaft), pit and overrun.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ArchElevator {
    pub tag: String,
    pub levels: Vec<String>,
    /// Hoistway inside, building feet.
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Pit below the lowest floor, feet.
    pub pit_ft: f64,
    /// Capacity, pounds.
    pub capacity_lb: u32,
}

/// The edge of a level's slab (the roof's for `R`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SlabEdge {
    pub level: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Beyond the exterior walls' outer face, inches.
    pub beyond_face_in: f64,
}

/// The roof plan: its edge, slope, drains and what stands on it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Roof {
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Parapet height above the roof, inches (none: no parapet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parapet_in: Option<f64>,
    /// Slope, inches per foot.
    pub slope_in_ft: f64,
    pub drains: Vec<[f64; 2]>,
    pub curbs: Vec<Curb>,
}

/// An equipment curb, roof hatch or elevator overrun on the roof.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Curb {
    /// The equipment's tag (`EF-1`), `ROOF HATCH` or `ELEV. OVERRUN`.
    pub tag: String,
    /// `equipment`, `hatch` or `overrun`.
    pub kind: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Above the roof, inches.
    pub height_in: f64,
}

/// A room's row in the room finish schedule.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoomFinish {
    pub level: String,
    pub number: String,
    pub name: String,
    pub floor: String,
    pub base: String,
    pub wall: String,
    pub ceiling: String,
    /// As the schedule prints it, feet (none: open to structure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling_ft: Option<f64>,
}

/// A finish code in the finish legend.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FinishCode {
    pub code: String,
    /// `floor`, `base`, `wall`, `ceiling`.
    pub kind: String,
    pub description: String,
    /// Device plate colour on walls with this finish (`WHITE`, `GRAY`, `STAINLESS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plates: Option<String>,
}

// --- Roofing and building envelope (Q337) ---

/// The roof and wall envelope as the roof drainage and flashing plan, the assemblies sheet and
/// the spec give it, with the takeoff and the coordination flags they lead to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub roof: RoofAssembly,
    /// Exterior wall assemblies outboard of the backup wall, by zone.
    pub claddings: Vec<Cladding>,
    /// The roof edge: `coping` (on the parapet) or `edge_metal`.
    pub edge: String,
    pub overflows: Vec<Overflow>,
    /// Pipes through the roof (vents).
    pub pipes: Vec<RoofPipe>,
    /// Crickets behind the curbs and saddles between the drains.
    pub ridges: Vec<RoofRidge>,
    pub walkways: Vec<Walkway>,
    /// The details on the assemblies sheet.
    pub details: Vec<Detail>,
    /// Detail callouts on the roof drainage plan.
    pub callouts: Vec<Callout>,
    /// The wall section's datums: `1`, `2`, `R`, `P` (top of parapet or roof edge), feet.
    pub datums: Vec<Datum>,
    pub spec: EnvSpec,
    pub takeoff: Vec<QtyItem>,
    pub flags: Vec<EnvFlag>,
    /// Gaps put there on purpose: `no_curb_detail`, `missing_detail`, `no_overflow`, `spec_mil`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub planted: Vec<String>,
}

/// One layer of a roof or exterior wall assembly.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EnvLayer {
    /// Roof: `membrane`, `cover_board`, `tapered`, `insulation`, `vapor_retarder`, `deck`.
    /// Wall: `cladding`, `air_space`, `furring`, `ci`, `barrier`, `sheathing`.
    pub component: String,
    /// As the legend prints it.
    pub description: String,
    /// Inches (a tapered layer: its least; a membrane or barrier: 0).
    pub thickness_in: f64,
    /// Identical boards stacked (`(2) LAYERS 2"`).
    #[serde(default = "one_layer")]
    pub count: u32,
}

fn one_layer() -> u32 {
    1
}

/// The roof assembly, top to bottom.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoofAssembly {
    pub tag: String,
    /// `tpo`, `pvc`, `epdm`, `sbs`.
    pub membrane: String,
    /// Membrane thickness, mils (none for a built-up sheet system).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mil: Option<f64>,
    /// `fully adhered`, `mechanically fastened`, `induction welded`, `torch applied`.
    pub attachment: String,
    pub layers: Vec<EnvLayer>,
}

/// An exterior wall assembly over the backup wall: the cladding and what is behind it, outside
/// to inside, on the levels it covers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cladding {
    pub tag: String,
    /// `brick`, `siding`, `eifs`, `panel`.
    pub kind: String,
    /// Level codes, bottom up.
    pub levels: Vec<String>,
    /// Goes on up the parapet (or the roof edge fascia).
    pub to_top: bool,
    pub layers: Vec<EnvLayer>,
}

/// A secondary (overflow) outlet: a `scupper` through the parapet or an overflow `drain`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Overflow {
    pub kind: String,
    pub at_ft: [f64; 2],
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoofPipe {
    pub tag: String,
    pub at_ft: [f64; 2],
    pub size_in: f64,
}

/// A cricket (behind a curb, `at` = the curb's tag) or a saddle (between two drains).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoofRidge {
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub at: String,
    pub outline: Vec<[f64; 2]>,
    pub height_in: f64,
}

/// A strip of walkway pads, building feet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Walkway {
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
}

/// A detail on the assemblies sheet: its number, title and what it details (a curb's tag,
/// `edge`, `drain`, `pipe`, `cricket`, `window`, `storefront`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Detail {
    pub number: u32,
    pub title: String,
    pub subject: String,
}

/// A detail callout on the roof drainage plan: detail `number` on `sheet`, pointing at `at_ft`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Callout {
    pub number: u32,
    pub sheet: String,
    pub at_ft: [f64; 2],
    pub subject: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Datum {
    pub code: String,
    pub elevation_ft: f64,
}

/// What the spec (Division 07) requires.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EnvSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membrane_mil: Option<f64>,
    /// Least average thermal resistance of the roof insulation.
    pub min_r: f64,
    /// Insulation's thermal resistance per inch.
    pub r_per_in: f64,
    /// Sheet metal coping or edge gauge.
    pub metal_ga: u32,
}

/// A takeoff line: net quantity from the model, waste, and what to order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QtyItem {
    /// Stable key (`roof.membrane`, `wall.EW-1.cladding`, `pen.pipe_boot`).
    pub code: String,
    pub description: String,
    /// `SQ`, `SF`, `LF`, `EA`, `BF`.
    pub unit: String,
    pub net: f64,
    pub waste_pct: f64,
    /// Net plus waste, rounded up.
    pub order: f64,
}

/// A coordination flag: `no_curb_detail`, `drain_structure`, `no_overflow`, `spec_mil`,
/// `r_value`, `no_cricket`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EnvFlag {
    pub kind: String,
    /// What it is about (a curb's tag, a drain's place `RD@x,y`).
    pub subject: String,
    pub message: String,
}
