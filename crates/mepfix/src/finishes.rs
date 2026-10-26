//! Q338: Finish takeoff — quantities by room and trade from the architecture read off the prints.
//!
//! From the finish schedule, room finish plans and specs: flooring (carpet, LVT, tile, sealed
//! concrete) with transitions and base, wall finishes (paint by coats/sheen, wallcovering,
//! tile/FRP), ceilings (ACT grid/tile, gypsum), doors/frames/hardware sets, toilet partitions
//! and accessories, signage, window treatments, fire extinguishers/cabinets, corner guards —
//! quantities by room/unit type, with waste and pattern rules; per-trade takeoff ready for
//! pricing.
//!
//! Q5121: the takeoff runs on a [`TakeoffInput`]: rooms (box and finish codes), doors, windows
//! and walls. [`read_input`] builds it from a print set's DXF sheets (the architecture reader's
//! rooms, schedules and doors, the wall reader's walls); [`input_from_truth`] builds it from a
//! fixture's answer key for tests only. Every line says where it came from (`source`) and how
//! (`status`: `drawing` measured on the plan, `schedule` a schedule row, `rule` a company
//! standard with its `rule` text, `guess` with the reason). Room-type rules (partitions,
//! accessories, corner guards, cabinets) come from a data table, [`default_room_rules`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::arch_reader::ArchResult;
use crate::ceiling_reader::CeilingResult;
use crate::sheet_reader::SheetMap;
use crate::truth::{FinishCode, HardwareGroup, Room, Truth, WindowType};
use crate::wall_reader::WallResult;

// ---- Waste factors (trade standards) --------------------------------------------------------

/// Standard waste factors by finish material category.
pub fn waste_factor(category: &str) -> f64 {
    match category {
        "carpet_tile" => 0.05,      // 5% — modular, low waste
        "carpet_broadloom" => 0.10, // 10% — seams and pattern match
        "lvt" => 0.08,              // 8% — plank cuts
        "vct" => 0.05,              // 5% — tile grid
        "ceramic_tile" => 0.10,     // 10% — cuts and breakage
        "porcelain_tile" => 0.10,
        "rubber_treads" => 0.05,
        "sealed_concrete" => 0.0, // sealer applied per SF, negligible
        "rubber_base" => 0.05,
        "cove_base" => 0.05,
        "paint" => 0.05,        // 5% overage
        "wallcovering" => 0.15, // 15% — pattern match
        "wall_tile" => 0.10,
        "act_tile" => 0.03,     // 3% — grid tile
        "gypsum_board" => 0.10, // 10% — sheet cuts
        "door" => 0.0,          // counted, no waste
        "frame" => 0.0,
        "hardware" => 0.0,
        "window" => 0.0,
        "glass" => 0.05,
        "partition" => 0.0,
        "accessory" => 0.0,
        "signage" => 0.0,
        "corner_guard" => 0.05,
        "fire_extinguisher" => 0.0,
        _ => 0.05,
    }
}

/// Extra waste for a patterned layout (herringbone, diagonal, chevron, patterned), on top of
/// the material's own.
pub const PATTERN_WASTE: f64 = 0.05;

/// Map a finish code description to a material category (for waste factor lookup).
fn code_to_category(desc: &str) -> &'static str {
    let d = desc.to_ascii_uppercase();
    // "PAINT, EGGSHELL" and "EPOXY PAINT" are paint; "PAINTED GYPSUM BOARD" is gypsum board.
    let paint_product = d.starts_with("PAINT ") || d.starts_with("PAINT,") || d.contains("EPOXY");
    if d.contains("CARPET TILE") {
        "carpet_tile"
    } else if d.contains("CARPET") || d.contains("BROADLOOM") {
        "carpet_broadloom"
    } else if d.contains("LVT") || d.contains("LUXURY VINYL") {
        "lvt"
    } else if d.contains("VCT") || d.contains("VINYL COMPOSITION") {
        "vct"
    } else if d.contains("PORCELAIN") && d.contains("FLOOR") {
        "porcelain_tile"
    } else if d.contains("CERAMIC") || d.contains("PORCELAIN") {
        "ceramic_tile"
    } else if d.contains("RUBBER TREAD") {
        "rubber_treads"
    } else if d.contains("SEALED CONCRETE") {
        "sealed_concrete"
    } else if d.contains("COVE BASE") {
        "cove_base"
    } else if d.contains("RUBBER BASE") {
        "rubber_base"
    } else if !paint_product && (d.contains("GYPSUM") || d.contains("GYP")) {
        "gypsum_board"
    } else if d.contains("PAINT") || d.contains("EPOXY") {
        "paint"
    } else if d.contains("WALLCOVERING") || d.contains("FRP") {
        "wallcovering"
    } else if d.contains("WALL TILE") || (d.contains("TILE") && d.contains("WALL")) {
        "wall_tile"
    } else if d.contains("ACOUSTIC") || d.contains("ACT") {
        "act_tile"
    } else {
        // Anything else defaults to paint.
        "paint"
    }
}

// ---- Room geometry --------------------------------------------------------------------------

/// Room floor area in square feet from its bounds.
pub fn room_area_sf(room: &Room) -> f64 {
    box_area(room.min_ft, room.max_ft)
}

/// Room perimeter in linear feet from its bounds.
pub fn room_perimeter_lf(room: &Room) -> f64 {
    box_perimeter(room.min_ft, room.max_ft)
}

fn box_area(lo: [f64; 2], hi: [f64; 2]) -> f64 {
    (hi[0] - lo[0]).max(0.0) * (hi[1] - lo[1]).max(0.0)
}

fn box_perimeter(lo: [f64; 2], hi: [f64; 2]) -> f64 {
    2.0 * ((hi[0] - lo[0]).max(0.0) + (hi[1] - lo[1]).max(0.0))
}

/// A wall centreline within this of a room box's side lies along that side, feet.
const SIDE_TOL: f64 = 0.75;
/// A wall whose ends differ by less than this across is axis-aligned, feet.
const AXIS_TOL: f64 = 0.05;

/// Each side of a room's box (bottom, top, left, right): its length and the height of the walls
/// along it, weighted by how much of the side each covers (none: no wall with a height there).
fn room_sides(room: &TakeoffRoom, walls: &[TakeoffWall]) -> [(f64, Option<f64>); 4] {
    let (lo, hi) = (room.min_ft, room.max_ft);
    // (fixed axis, fixed value, span on the other axis)
    let sides = [(1usize, lo[1], lo[0], hi[0]), (1, hi[1], lo[0], hi[0]), (0, lo[0], lo[1], hi[1]), (0, hi[0], lo[1], hi[1])];
    let mut out = [(0.0, None); 4];
    for (i, (axis, at, s0, s1)) in sides.into_iter().enumerate() {
        let other = 1 - axis;
        let (mut covered, mut weighted) = (0.0, 0.0);
        for w in walls.iter().filter(|w| w.level == room.level) {
            let (Some(a_fixed), Some(b_fixed), Some(a_run), Some(b_run)) = (w.a_ft.get(axis), w.b_ft.get(axis), w.a_ft.get(other), w.b_ft.get(other))
            else {
                continue;
            };
            if (a_fixed - b_fixed).abs() > AXIS_TOL || ((a_fixed + b_fixed) / 2.0 - at).abs() > SIDE_TOL {
                continue;
            }
            let Some(h) = w.height_ft.filter(|h| h.is_finite() && *h > 0.0) else { continue };
            let overlap = a_run.max(*b_run).min(s1) - a_run.min(*b_run).max(s0);
            if overlap > 0.01 {
                covered += overlap;
                weighted += overlap * h;
            }
        }
        out[i] = ((s1 - s0).max(0.0), (covered > 0.0).then(|| weighted / covered));
    }
    out
}

/// The room's box from its label: the nearest wall centreline each way (up, down, left, right).
fn box_from_walls(level: &str, at: [f64; 2], walls: &[TakeoffWall]) -> Option<([f64; 2], [f64; 2])> {
    let mut lo = [f64::NEG_INFINITY; 2];
    let mut hi = [f64::INFINITY; 2];
    for w in walls.iter().filter(|w| w.level == level) {
        for axis in 0..2 {
            let other = 1 - axis;
            let (Some(a_fixed), Some(b_fixed), Some(a_run), Some(b_run), Some(p_fixed), Some(p_run)) =
                (w.a_ft.get(axis), w.b_ft.get(axis), w.a_ft.get(other), w.b_ft.get(other), at.get(axis), at.get(other))
            else {
                continue;
            };
            if (a_fixed - b_fixed).abs() > AXIS_TOL || *p_run < a_run.min(*b_run) || *p_run > a_run.max(*b_run) {
                continue;
            }
            let v = (a_fixed + b_fixed) / 2.0;
            if v < *p_fixed {
                lo[axis] = lo[axis].max(v);
            } else if v > *p_fixed {
                hi[axis] = hi[axis].min(v);
            }
        }
    }
    let ok = (0..2).all(|i| lo[i].is_finite() && hi[i].is_finite() && hi[i] - lo[i] >= 1.0 && hi[i] - lo[i] <= 5_000.0);
    ok.then_some((lo, hi))
}

// ---- Finish takeoff line items --------------------------------------------------------------

/// One line item in the finish takeoff.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TakeoffItem {
    /// The trade this belongs to: `flooring`, `painting`, `ceilings`, `doors_frames_hardware`,
    /// `specialties`, `glazing`.
    pub trade: String,
    /// Material or item description.
    pub description: String,
    /// Finish code from the schedule (if applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_code: Option<String>,
    /// Room number (or `"BUILDING"` for building-wide items).
    pub room: String,
    /// Room name.
    pub room_name: String,
    /// Level code.
    pub level: String,
    /// Quantity before waste.
    pub quantity: f64,
    /// Unit: `SF`, `LF`, `EA`, `GAL`, `SY`.
    pub unit: String,
    /// Waste factor applied (0.0–1.0).
    pub waste_factor: f64,
    /// Quantity after waste: `quantity * (1 + waste_factor)`.
    pub with_waste: f64,
    /// Material category (for grouping).
    pub category: String,
    /// Where it came from: the plan sheet, and the schedule sheet and row (or the rule).
    #[serde(default)]
    pub source: String,
    /// `drawing` (measured on the plan), `schedule` (a schedule row), `rule` (a company
    /// standard, see `rule`) or `guess` (see `rule` for why).
    #[serde(default)]
    pub status: String,
    /// The rule or assumption behind the line (rule lines, default coats, pattern waste, guesses).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// The schedule row it came from (room number, door mark, window type).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_row: Option<String>,
    /// Paint: finish coats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coats: Option<u32>,
    /// Paint: sheen as the legend names it (`EGGSHELL`, `SEMI-GLOSS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheen: Option<String>,
    /// The room's unit type (hotel guest units), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_type: Option<String>,
}

/// A per-trade summary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TradeSummary {
    pub trade: String,
    pub items: Vec<TakeoffItem>,
    /// Total lines.
    pub line_count: usize,
}

/// One line of a unit type's roll-up: a description's total over the rooms of that type.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UnitTypeLine {
    pub trade: String,
    pub description: String,
    pub unit: String,
    pub quantity: f64,
    pub with_waste: f64,
    /// Rooms of the type with this line.
    pub rooms: usize,
}

/// The complete finish takeoff for a building.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FinishTakeoff {
    pub items: Vec<TakeoffItem>,
    /// Per-trade summaries.
    pub trades: Vec<TradeSummary>,
    /// Room-level roll-ups: room number → items.
    pub by_room: BTreeMap<String, Vec<TakeoffItem>>,
    /// Unit-type roll-ups: unit type → its lines summed over its rooms.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_unit_type: BTreeMap<String, Vec<UnitTypeLine>>,
    /// What could not be taken off and why (a room with no finish row, a door in no room).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

// ---- The takeoff's input --------------------------------------------------------------------

/// A room: its box (wall centrelines, building feet) and its finish schedule row.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TakeoffRoom {
    pub level: String,
    pub number: String,
    pub name: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Finish codes from the room finish schedule (none: the room has no row).
    #[serde(default)]
    pub floor: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub wall: Option<String>,
    #[serde(default)]
    pub ceiling: Option<String>,
    /// ACT tile size `[across, along]`, feet, from the reflected ceiling plan.
    #[serde(default)]
    pub grid_ft: Option<[f64; 2]>,
    /// The unit type it belongs to (hotel guest units).
    #[serde(default)]
    pub unit_type: Option<String>,
    /// The floor plan sheet it was read from.
    #[serde(default)]
    pub plan_sheet: String,
}

/// A door on a plan, with its schedule row's data when it has one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TakeoffDoor {
    pub level: String,
    pub mark: Option<String>,
    pub at_ft: [f64; 2],
    pub width_in: f64,
    /// From the schedule (none: no row).
    pub height_in: Option<f64>,
    #[serde(default)]
    pub door_type: String,
    #[serde(default)]
    pub material: String,
    #[serde(default)]
    pub frame: String,
    #[serde(default)]
    pub fire_rating: Option<String>,
    #[serde(default)]
    pub hardware: String,
    /// It has a door schedule row.
    #[serde(default)]
    pub scheduled: bool,
    pub swings_into: Option<String>,
    pub from_room: Option<String>,
    #[serde(default)]
    pub plan_sheet: String,
}

/// A window on a plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TakeoffWindow {
    pub level: String,
    pub mark: String,
    pub at_ft: [f64; 2],
    #[serde(default)]
    pub plan_sheet: String,
}

/// A wall's centreline and height.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TakeoffWall {
    pub level: String,
    pub a_ft: [f64; 2],
    pub b_ft: [f64; 2],
    pub height_ft: Option<f64>,
    #[serde(default)]
    pub sheet: String,
}

/// Everything the takeoff works from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TakeoffInput {
    pub rooms: Vec<TakeoffRoom>,
    pub doors: Vec<TakeoffDoor>,
    pub windows: Vec<TakeoffWindow>,
    pub walls: Vec<TakeoffWall>,
    /// The finish legend.
    pub codes: Vec<FinishCode>,
    pub hardware: Vec<HardwareGroup>,
    pub window_types: Vec<WindowType>,
    /// The sheets holding the room finish, door and window schedules.
    #[serde(default)]
    pub finish_schedule: String,
    #[serde(default)]
    pub door_schedule: String,
    #[serde(default)]
    pub window_schedule: String,
    /// What the readers could not use (reported in the takeoff's `missing`).
    #[serde(default)]
    pub notes: Vec<String>,
}

// ---- Room-type rules (company standards) ----------------------------------------------------

/// A counted item a room-type rule adds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleItem {
    pub description: String,
    pub quantity: f64,
    /// `partition`, `accessory`, `corner_guard`, `fire_extinguisher`.
    pub category: String,
}

/// What a room type gets until its enlarged plan or the Division 10 spec is read: one row of the
/// company standards table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoomRule {
    /// Room names it covers: a room matches when one of these is in its name as whole words
    /// (`MEN` matches `MEN'S TOILET`, not `WOMEN`). A room takes the first rule that matches.
    pub names: Vec<String>,
    /// The standard, as every line it makes says.
    pub rule: String,
    pub items: Vec<RuleItem>,
}

fn item(description: &str, quantity: f64, category: &str) -> RuleItem {
    RuleItem { description: description.into(), quantity, category: category.into() }
}

fn rule(names: &[&str], text: &str, items: Vec<RuleItem>) -> RoomRule {
    RoomRule { names: names.iter().map(|n| n.to_string()).collect(), rule: text.into(), items }
}

/// The company standards table: room types (office and hotel) and what each gets.
pub fn default_room_rules() -> Vec<RoomRule> {
    let partition = || item("TOILET PARTITION (FLOOR-MOUNTED, OVERHEAD-BRACED)", 2.0, "partition");
    let shared = || {
        vec![
            partition(),
            item("TOILET PAPER HOLDER, RECESSED", 2.0, "accessory"),
            item("GRAB BAR SET (ADA), 36\"+42\"", 1.0, "accessory"),
            item("SOAP DISPENSER, SURFACE-MOUNTED", 2.0, "accessory"),
            item("PAPER TOWEL DISPENSER / WASTE", 1.0, "accessory"),
            item("MIRROR, FULL-WIDTH ABOVE LAVATORY", 1.0, "accessory"),
            item("SEAT COVER DISPENSER", 2.0, "accessory"),
            item("COAT HOOK", 2.0, "accessory"),
        ]
    };
    let with_hygiene = || {
        let mut v = shared();
        v.push(item("FEMININE HYGIENE DISPENSER", 1.0, "accessory"));
        v.push(item("SANITARY NAPKIN DISPOSAL", 2.0, "accessory"));
        v
    };
    let guards = || item("CORNER GUARD, STAINLESS STEEL, 48\"", 2.0, "corner_guard");
    let cabinet = || item("FIRE EXTINGUISHER CABINET, SEMI-RECESSED", 1.0, "fire_extinguisher");
    vec![
        rule(
            &["GUEST BATH", "GUEST BATHROOM", "UNIT BATH"],
            "hotel guest bath: one set of bath accessories, no partitions (company standard until the enlarged unit plan or Division 10 spec is read)",
            vec![
                item("TOILET PAPER HOLDER, RECESSED", 1.0, "accessory"),
                item("GRAB BAR SET (ADA), 36\"+42\"", 1.0, "accessory"),
                item("TOWEL BAR, 24\"", 1.0, "accessory"),
                item("ROBE HOOK", 1.0, "accessory"),
                item("SHOWER CURTAIN ROD, CURVED", 1.0, "accessory"),
                item("MIRROR, FULL-WIDTH ABOVE LAVATORY", 1.0, "accessory"),
            ],
        ),
        rule(
            &["GUEST ROOM", "GUESTROOM", "KING", "QUEEN", "DOUBLE QUEEN"],
            "hotel guest room: wardrobe rod and shelf (company standard until the enlarged unit plan is read)",
            vec![item("WARDROBE ROD AND SHELF", 1.0, "accessory")],
        ),
        rule(
            &["WOMEN", "WOMENS", "LADIES"],
            "multi-user women's restroom: 2 stalls with the standard accessory set (company standard until the enlarged plan or Division 10 spec is read)",
            with_hygiene(),
        ),
        rule(
            &["MEN", "MENS"],
            "multi-user men's restroom: 2 stalls with the standard accessory set (company standard until the enlarged plan or Division 10 spec is read)",
            shared(),
        ),
        rule(
            &["RESTROOM", "TOILET", "UNISEX", "ALL GENDER"],
            "multi-user restroom: 2 stalls with the standard accessory set (company standard until the enlarged plan or Division 10 spec is read)",
            with_hygiene(),
        ),
        rule(
            &["CORRIDOR", "HALL", "HALLWAY"],
            "corridor: 2 corner guards at its outside corners and one extinguisher cabinet (company standard until the plans show them)",
            vec![guards(), cabinet()],
        ),
        rule(&["MECH", "MECHANICAL"], "mechanical room: one extinguisher cabinet (company standard until the plans show it)", vec![cabinet()]),
        rule(
            &["BREAK ROOM", "WORK ROOM", "COPY"],
            "high-traffic room: 2 corner guards at its outside corners (company standard until the plans show them)",
            vec![guards()],
        ),
    ]
}

/// A name's words, upper case (`MEN'S TOILET` → `MEN`, `S`, `TOILET`).
fn words(s: &str) -> Vec<String> {
    s.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// The first rule whose names are in the room's name as whole words.
pub fn rule_for<'a>(room_name: &str, rules: &'a [RoomRule]) -> Option<&'a RoomRule> {
    let w = words(room_name);
    rules.iter().find(|r| {
        r.names.iter().any(|n| {
            let p = words(n);
            !p.is_empty() && w.windows(p.len()).any(|win| win == p.as_slice())
        })
    })
}

// ---- The takeoff engine ---------------------------------------------------------------------

/// Compute the full finish takeoff from a truth set (tests: the fixture's answer key).
///
/// Requires `truth.arch` (Q307) and `truth.building.rooms` to be populated.
pub fn compute_takeoff(truth: &Truth) -> FinishTakeoff {
    compute(&input_from_truth(truth), &default_room_rules())
}

/// The takeoff's input from a fixture's answer key (tests only: a real job has none).
pub fn input_from_truth(truth: &Truth) -> TakeoffInput {
    let Some(arch) = truth.arch.as_ref() else { return TakeoffInput::default() };
    let key = "answer key".to_string();
    let rooms = arch
        .finishes
        .iter()
        .filter_map(|rf| {
            let room = truth.building.rooms.iter().find(|r| r.level == rf.level && r.number == rf.number)?;
            Some(TakeoffRoom {
                level: rf.level.clone(),
                number: rf.number.clone(),
                name: rf.name.clone(),
                min_ft: room.min_ft,
                max_ft: room.max_ft,
                floor: Some(rf.floor.clone()),
                base: Some(rf.base.clone()),
                wall: Some(rf.wall.clone()),
                ceiling: Some(rf.ceiling.clone()),
                grid_ft: room.ceiling.as_ref().and_then(|c| c.grid_ft),
                unit_type: None,
                plan_sheet: key.clone(),
            })
        })
        .collect();
    let doors = arch
        .doors
        .iter()
        .map(|d| TakeoffDoor {
            level: d.level.clone(),
            mark: Some(d.mark.clone()),
            at_ft: d.at_ft,
            width_in: d.width_in,
            height_in: Some(d.height_in),
            door_type: d.door_type.clone(),
            material: d.material.clone(),
            frame: d.frame.clone(),
            fire_rating: d.fire_rating.clone(),
            hardware: d.hardware.clone(),
            scheduled: true,
            swings_into: d.swings_into.clone(),
            from_room: d.from_room.clone(),
            plan_sheet: key.clone(),
        })
        .collect();
    let windows = arch
        .windows
        .iter()
        .map(|w| TakeoffWindow { level: w.level.clone(), mark: w.mark.clone(), at_ft: w.at_ft, plan_sheet: key.clone() })
        .collect();
    let walls = truth
        .plan_walls
        .iter()
        .map(|w| TakeoffWall { level: w.level.clone(), a_ft: w.a_ft, b_ft: w.b_ft, height_ft: Some(w.height_ft), sheet: key.clone() })
        .collect();
    TakeoffInput {
        rooms,
        doors,
        windows,
        walls,
        codes: arch.finish_codes.clone(),
        hardware: arch.hardware.clone(),
        window_types: arch.window_types.clone(),
        finish_schedule: key.clone(),
        door_schedule: key.clone(),
        window_schedule: key,
        notes: Vec::new(),
    }
}

/// The sheets of a set that hold the room finish, door and window schedules.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScheduleSheets {
    pub finishes: String,
    pub doors: String,
    pub windows: String,
}

/// Find which architectural schedule sheet holds each schedule (by the tables' titles).
pub fn schedule_sheets(dxf: &[(String, Vec<u8>)], map: &SheetMap) -> ScheduleSheets {
    let mut out = ScheduleSheets::default();
    for (name, bytes) in dxf {
        let stem = name.trim_end_matches(".dxf");
        let Some(sh) = map.sheets.iter().find(|s| s.number == stem) else { continue };
        if sh.discipline != "A" || !sh.title.to_ascii_uppercase().contains("SCHEDULE") {
            continue;
        }
        let Ok(d) = cadaddy_io::read_dxf(bytes) else { continue };
        let texts: Vec<String> = crate::sheet_facts::texts(&d).into_iter().map(|t| t.value.to_ascii_uppercase()).collect();
        let has = |w: &str| texts.iter().any(|t| t.contains(w));
        if out.finishes.is_empty() && has("ROOM FINISHES") {
            out.finishes = sh.number.clone();
        }
        if out.doors.is_empty() && has("DOOR SCHEDULE") {
            out.doors = sh.number.clone();
        }
        if out.windows.is_empty() && has("WINDOW SCHEDULE") {
            out.windows = sh.number.clone();
        }
    }
    out
}

/// The takeoff's input from what was read off the prints: the architecture reader's rooms,
/// schedules, doors and windows, the wall reader's walls and the ceiling reader's grids. Room
/// boxes run between the read wall centrelines either side of each room's label (the ceiling
/// reader's room outline when the walls don't close round it).
pub fn input_from_read(arch: &ArchResult, walls: &WallResult, ceilings: &CeilingResult, map: &SheetMap, sheets: &ScheduleSheets) -> TakeoffInput {
    let plan_of = |level: &str| {
        map.sheets
            .iter()
            .find(|s| s.discipline == "A" && s.level.as_deref() == Some(level) && s.title.to_ascii_uppercase().contains("FLOOR PLAN"))
            .map(|s| s.number.clone())
            .unwrap_or_default()
    };
    // What the reader left out (unmarked swings off the slab: legend samples) is reported, not counted.
    let mut notes: Vec<String> = arch.ignored.iter().map(|w| format!("{w}: not counted")).collect();
    let twalls: Vec<TakeoffWall> = walls
        .walls
        .iter()
        .map(|w| TakeoffWall { level: w.level.clone(), a_ft: w.a_ft, b_ft: w.b_ft, height_ft: w.height_ft, sheet: w.sheet.clone() })
        .collect();
    let mut rooms = Vec::new();
    for r in &arch.rooms {
        let bounds = box_from_walls(&r.level, r.at_ft, &twalls)
            .or_else(|| ceilings.rooms.iter().find(|c| c.level == r.level && c.number == r.number).map(|c| (c.min_ft, c.max_ft)));
        let Some((min_ft, max_ft)) = bounds else {
            notes.push(format!(
                "room {} {} on level {}: no walls close round its label and no ceiling outline, so no areas",
                r.number, r.name, r.level
            ));
            continue;
        };
        let grid_ft = ceilings.ceilings.iter().find(|c| c.level == r.level && c.room_number == r.number).and_then(|c| c.grid_ft);
        rooms.push(TakeoffRoom {
            level: r.level.clone(),
            number: r.number.clone(),
            name: r.name.clone(),
            min_ft,
            max_ft,
            floor: r.floor.clone(),
            base: r.base.clone(),
            wall: r.wall.clone(),
            ceiling: r.ceiling.clone(),
            grid_ft,
            unit_type: None,
            plan_sheet: plan_of(&r.level),
        });
    }
    let mut doors = Vec::new();
    for d in &arch.doors {
        if d.mark.is_none() && d.swings_into.is_none() && d.from_room.is_none() {
            notes.push(format!(
                "unmarked door symbol at ({:.1}, {:.1}) on level {} is in no room (a legend or key plan symbol?): not counted",
                d.at_ft[0], d.at_ft[1], d.level
            ));
            continue;
        }
        let row = d.row.as_ref();
        doors.push(TakeoffDoor {
            level: d.level.clone(),
            mark: d.mark.clone(),
            at_ft: d.at_ft,
            width_in: row.and_then(|r| r.width_in).unwrap_or(d.width_in),
            height_in: row.and_then(|r| r.height_in),
            door_type: row.map(|r| r.door_type.clone()).unwrap_or_default(),
            material: row.map(|r| r.material.clone()).unwrap_or_default(),
            frame: row.map(|r| r.frame.clone()).unwrap_or_default(),
            fire_rating: row.and_then(|r| r.fire_rating.clone()),
            hardware: row.map(|r| r.hardware.clone()).unwrap_or_default(),
            scheduled: row.is_some(),
            swings_into: d.swings_into.clone(),
            from_room: d.from_room.clone(),
            plan_sheet: plan_of(&d.level),
        });
    }
    let mut windows = Vec::new();
    for w in &arch.windows {
        match &w.mark {
            Some(m) => windows.push(TakeoffWindow { level: w.level.clone(), mark: m.clone(), at_ft: w.at_ft, plan_sheet: plan_of(&w.level) }),
            None => notes
                .push(format!("unmarked window at ({:.1}, {:.1}) on level {}: its type is unknown, not counted", w.at_ft[0], w.at_ft[1], w.level)),
        }
    }
    TakeoffInput {
        rooms,
        doors,
        windows,
        walls: twalls,
        codes: arch.finish_codes.clone(),
        hardware: arch.hardware.clone(),
        window_types: arch.window_types.clone(),
        finish_schedule: sheets.finishes.clone(),
        door_schedule: sheets.doors.clone(),
        window_schedule: sheets.windows.clone(),
        notes,
    }
}

/// Read a print set's DXF sheets (`(file name, bytes)`, the sheet number as the name) into the
/// takeoff's input: sheets recognised, ceilings, structure, architecture and walls read.
pub fn read_input(dxf: &[(String, Vec<u8>)]) -> TakeoffInput {
    let map = crate::sheet_reader::recognise_fixture_set(dxf);
    let ceilings = crate::ceiling_reader::read_ceilings(dxf, &map, None);
    let arch = crate::arch_reader::read_arch(dxf, &map, &ceilings);
    // The structure (deck heights for the walls) comes from the structural sheets only.
    let drawings: Vec<(String, cadaddy_doc::Drawing)> = dxf
        .iter()
        .filter(|(n, _)| map.sheets.iter().any(|s| s.number == n.trim_end_matches(".dxf") && s.discipline == "S"))
        .filter_map(|(n, b)| cadaddy_io::read_dxf(b).ok().map(|d| (n.trim_end_matches(".dxf").to_string(), d)))
        .collect();
    let refs: Vec<(String, &cadaddy_doc::Drawing)> = drawings.iter().map(|(n, d)| (n.clone(), d)).collect();
    let structure = crate::struct_reader::read_structure(&map, &refs);
    let walls = crate::wall_reader::read_walls(dxf, &map, &ceilings, &structure);
    input_from_read(&arch, &walls, &ceilings, &map, &schedule_sheets(dxf, &map))
}

/// One room's measured lines' source: its plan, and its finish schedule row.
fn room_source(input: &TakeoffInput, room: &TakeoffRoom) -> String {
    let plan = if room.plan_sheet.is_empty() { "floor plan".to_string() } else { format!("{} plan", room.plan_sheet) };
    let sched =
        if input.finish_schedule.is_empty() { "room finish schedule".to_string() } else { format!("{} ROOM FINISHES", input.finish_schedule) };
    format!("{plan}; {sched} row {}", room.number)
}

/// A line of a room, measured on its plan unless set otherwise.
#[allow(clippy::too_many_arguments)]
fn line(
    trade: &str,
    description: String,
    code: Option<&String>,
    room: &TakeoffRoom,
    qty: f64,
    unit: &str,
    wf: f64,
    category: &str,
    source: &str,
) -> TakeoffItem {
    TakeoffItem {
        trade: trade.into(),
        description,
        finish_code: code.cloned(),
        room: room.number.clone(),
        room_name: room.name.clone(),
        level: room.level.clone(),
        quantity: round2(qty),
        unit: unit.into(),
        waste_factor: wf,
        with_waste: round2(qty * (1.0 + wf)),
        category: category.into(),
        source: source.into(),
        status: "drawing".into(),
        rule: None,
        schedule_row: Some(room.number.clone()),
        coats: None,
        sheen: None,
        unit_type: room.unit_type.clone(),
    }
}

/// A company-standard line of a room.
fn rule_line(trade: &str, description: &str, room: &TakeoffRoom, qty: f64, category: &str, rule: &str) -> TakeoffItem {
    let plan = if room.plan_sheet.is_empty() { String::new() } else { format!(" on {}", room.plan_sheet) };
    let mut i = line(trade, description.into(), None, room, qty, "EA", 0.0, category, "");
    i.source = format!("company standard (room type rule); room {} {}{plan}", room.number, room.name);
    i.status = "rule".into();
    i.rule = Some(rule.into());
    i.schedule_row = None;
    i
}

/// The default when the finish legend gives no coat count.
const DEFAULT_COATS: u32 = 2;

/// Paint coats from the legend text (`2 COATS`, `(3) COATS`, `TWO COATS`).
pub fn parse_coats(desc: &str) -> Option<u32> {
    let w = words(desc);
    w.iter().enumerate().find_map(|(i, t)| {
        if !t.starts_with("COAT") || i == 0 {
            return None;
        }
        let n = w.get(i - 1)?;
        let v = match n.as_str() {
            "ONE" => 1,
            "TWO" => 2,
            "THREE" => 3,
            "FOUR" => 4,
            _ => n.parse::<u32>().ok()?,
        };
        (1..=6).contains(&v).then_some(v)
    })
}

/// The paint sheen the legend names.
pub fn parse_sheen(desc: &str) -> Option<String> {
    let d = desc.to_ascii_uppercase();
    ["SEMI-GLOSS", "SEMIGLOSS", "HIGH GLOSS", "GLOSS", "EGGSHELL", "SATIN", "LOW SHEEN", "FLAT", "MATTE"]
        .iter()
        .find(|s| d.contains(*s))
        .map(|s| if *s == "SEMIGLOSS" { "SEMI-GLOSS".to_string() } else { s.to_string() })
}

/// The patterned layout a floor code names, if any.
pub fn pattern_of(desc: &str) -> Option<&'static str> {
    let d = desc.to_ascii_uppercase();
    ["HERRINGBONE", "CHEVRON", "DIAGONAL", "BASKETWEAVE", "PATTERN"].into_iter().find(|p| d.contains(p))
}

/// Set a paint line's coats and sheen from its legend text (default coats as a stated rule).
fn paint_coats(mut i: TakeoffItem, desc: &str) -> TakeoffItem {
    i.sheen = parse_sheen(desc);
    match parse_coats(desc) {
        Some(c) => i.coats = Some(c),
        None => {
            i.coats = Some(DEFAULT_COATS);
            i.rule = Some(format!("{DEFAULT_COATS} finish coats: company standard (the finish legend gives no coat count)"));
        }
    }
    i
}

/// The primer line under a painted wall area.
fn primer(room: &TakeoffRoom, area: f64, code: Option<&String>) -> TakeoffItem {
    let mut i = line("painting", "PRIMER, 1 COAT".into(), code, room, area, "SF", waste_factor("paint"), "paint", "");
    i.source = format!("company standard (primer rule); room {} {}", room.number, room.name);
    i.status = "rule".into();
    i.rule = Some("1 primer coat under every painted wall: company standard".into());
    i.coats = Some(1);
    i
}

/// ACT tile size `[across, along]`, feet, from a code's text (`2x4`, `24x48`).
fn tile_size(desc: &str) -> Option<[f64; 2]> {
    let d = desc.to_ascii_uppercase();
    let b = d.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c != b'X' {
            continue;
        }
        let before: String = d.get(..i)?.chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
        let after: String = d.get(i + 1..)?.chars().take_while(char::is_ascii_digit).collect();
        let (Ok(a), Ok(b)) = (before.parse::<f64>(), after.parse::<f64>()) else { continue };
        let (a, b) = if a >= 12.0 && b >= 12.0 { (a / 12.0, b / 12.0) } else { (a, b) };
        if a > 0.0 && b > 0.0 && a <= 6.0 && b <= 6.0 {
            return Some([a.min(b), a.max(b)]);
        }
    }
    None
}

/// Transition type for a door between two floor materials.
fn transition_type(a: &str, b: &str) -> &'static str {
    let carpet = |c: &str| c.starts_with("carpet");
    let tile = |c: &str| c.ends_with("_tile");
    if carpet(a) || carpet(b) {
        "CARPET EDGE REDUCER"
    } else if a == "sealed_concrete" || b == "sealed_concrete" {
        "RESILIENT REDUCER AT CONCRETE"
    } else if tile(a) || tile(b) {
        "TILE TO RESILIENT TRANSITION"
    } else {
        "RESILIENT TRANSITION STRIP"
    }
}

/// Compute the full finish takeoff from its input, with the room-type rules given.
pub fn compute(input: &TakeoffInput, rules: &[RoomRule]) -> FinishTakeoff {
    let codes = &input.codes;
    let mut items = Vec::new();
    let mut missing = input.notes.clone();
    let doors_of = |room: &TakeoffRoom| -> Vec<&TakeoffDoor> {
        input
            .doors
            .iter()
            .filter(|d| d.level == room.level && (d.swings_into.as_deref() == Some(&room.number) || d.from_room.as_deref() == Some(&room.number)))
            .collect()
    };

    for room in &input.rooms {
        let area = box_area(room.min_ft, room.max_ft);
        let perim = box_perimeter(room.min_ft, room.max_ft);
        let doors = doors_of(room);
        let src = room_source(input, room);
        let absent: Vec<&str> = [("floor", &room.floor), ("base", &room.base), ("wall", &room.wall), ("ceiling", &room.ceiling)]
            .iter()
            .filter(|(_, c)| c.is_none())
            .map(|(k, _)| *k)
            .collect();
        if !absent.is_empty() {
            missing.push(format!(
                "room {} {} on level {}: no {} finish in the room finish schedule, so no line for it",
                room.number,
                room.name,
                room.level,
                absent.join("/")
            ));
        }

        // ---- 1. Flooring (by room) ----
        if let Some(floor) = &room.floor {
            let desc = code_description(codes, floor);
            let cat = code_to_category(&desc);
            let mut it = line("flooring", desc.clone(), Some(floor), room, area, "SF", waste_factor(cat), cat, &src);
            if let Some(p) = pattern_of(&desc) {
                let wf = waste_factor(cat) + PATTERN_WASTE;
                it.waste_factor = wf;
                it.with_waste = round2(area * (1.0 + wf));
                it.rule =
                    Some(format!("pattern rule: a {} layout adds {:.0}% waste (company standard)", p.to_ascii_lowercase(), PATTERN_WASTE * 100.0));
            }
            items.push(it);
        }

        // Base: perimeter less the door openings.
        if let Some(base) = &room.base {
            let base_desc = code_description(codes, base);
            let base_cat = code_to_category(&base_desc);
            let door_width_lf: f64 = doors.iter().map(|d| d.width_in / 12.0).sum();
            let base_lf = (perim - door_width_lf).max(0.0);
            items.push(line("flooring", base_desc, Some(base), room, base_lf, "LF", waste_factor(base_cat), base_cat, &src));
        }

        // ---- 2. Wall finishes ----
        let sides = room_sides(room, &input.walls);
        let gross: f64 = sides.iter().filter_map(|(len, h)| h.map(|h| len * h)).sum();
        let open_sides = sides.iter().filter(|(len, h)| *len > 0.0 && h.is_none()).count();
        if open_sides > 0 {
            missing.push(format!(
                "room {} {} on level {}: {open_sides} side(s) with no wall height read, left out of its wall area",
                room.number, room.name, room.level
            ));
        }
        let door_h = |d: &TakeoffDoor| d.height_in.unwrap_or(84.0) / 12.0;
        let openings: f64 = doors.iter().map(|d| d.width_in / 12.0 * door_h(d)).sum();
        let net_wall = (gross - openings).max(0.0);
        if let Some(wall) = &room.wall {
            let wall_desc = code_description(codes, wall);
            let wall_cat = code_to_category(&wall_desc);
            // If wall code is tile to a height (e.g. "PORCELAIN WALL TILE TO 7'-0\", PAINT ABOVE"),
            // split the wall area into tile zone + paint zone.
            if let Some(th) = parse_tile_height(&wall_desc) {
                let tile_gross: f64 = sides.iter().filter_map(|(len, h)| h.map(|h| len * th.min(h))).sum();
                let tile_open: f64 = doors.iter().map(|d| d.width_in / 12.0 * door_h(d).min(th)).sum();
                let tile_area = (tile_gross - tile_open).max(0.0);
                let paint_area = (net_wall - tile_area).max(0.0);
                items.push(line(
                    "painting",
                    format!("WALL TILE ({})", wall_desc),
                    Some(wall),
                    room,
                    tile_area,
                    "SF",
                    waste_factor("wall_tile"),
                    "wall_tile",
                    &src,
                ));
                if paint_area > 1.0 {
                    let it = line("painting", "PAINT ABOVE TILE".into(), Some(wall), room, paint_area, "SF", waste_factor("paint"), "paint", &src);
                    items.push(paint_coats(it, &wall_desc));
                    items.push(primer(room, paint_area, Some(wall)));
                }
            } else {
                let it = line("painting", wall_desc.clone(), Some(wall), room, net_wall, "SF", waste_factor(wall_cat), wall_cat, &src);
                if wall_cat == "paint" {
                    items.push(paint_coats(it, &wall_desc));
                    items.push(primer(room, net_wall, Some(wall)));
                } else {
                    items.push(it);
                }
            }
        }

        // ---- 3. Ceilings ----
        if let Some(ceiling) = &room.ceiling {
            let ceiling_desc = code_description(codes, ceiling);
            let up = ceiling_desc.to_ascii_uppercase();
            let exposed = up.contains("EXPOSED") || up.contains("OPEN TO STRUCTURE");
            let clg_cat = code_to_category(&ceiling_desc);
            // Paint the ceiling for "PAINTED GYPSUM BOARD" or "EXPOSED STRUCTURE, PAINTED".
            if up.contains("PAINT") || up.contains("GYPSUM") || exposed {
                let it = line(
                    "painting",
                    format!("CEILING PAINT ({})", ceiling_desc),
                    Some(ceiling),
                    room,
                    area,
                    "SF",
                    waste_factor("paint"),
                    "paint",
                    &src,
                );
                items.push(paint_coats(it, &ceiling_desc));
            }
            // Exposed structure has no ceiling to install: only its paint.
            if !exposed {
                items.push(line("ceilings", ceiling_desc.clone(), Some(ceiling), room, area, "SF", waste_factor(clg_cat), clg_cat, &src));
            }

            // ACT grid: linear feet of main tee and cross tee.
            if clg_cat == "act_tile" {
                let [across, along] = tile_size(&ceiling_desc).or(room.grid_ft).unwrap_or([2.0, 4.0]);
                let dx = (room.max_ft[0] - room.min_ft[0]).max(0.0);
                let dy = (room.max_ft[1] - room.min_ft[1]).max(0.0);
                // Main tees run along the longer dimension, cross tees across.
                let (long, short) = if dx >= dy { (dx, dy) } else { (dy, dx) };
                let main_rows = (short / along).ceil().max(1.0);
                let cross_rows = (long / across).ceil().max(1.0);
                let grid_lf = main_rows * long + cross_rows * short;
                let mut g = line(
                    "ceilings",
                    format!("CEILING GRID ({}x{} MAIN+CROSS TEE)", across, along),
                    None,
                    room,
                    grid_lf,
                    "LF",
                    0.03,
                    "act_tile",
                    &src,
                );
                g.finish_code = None;
                items.push(g);
                // Wall angle (perimeter trim).
                items.push(line("ceilings", "WALL ANGLE (PERIMETER TRIM)".into(), None, room, perim, "LF", 0.05, "act_tile", &src));
                // Suspension wire: one every 4 ft × 4 ft.
                let wire_count = (area / 16.0).ceil().max(1.0);
                let mut w = rule_line(
                    "ceilings",
                    "SUSPENSION WIRE",
                    room,
                    wire_count,
                    "act_tile",
                    "one hanger wire per 16 SF (4'-0\" x 4'-0\" spacing): company standard",
                );
                w.waste_factor = 0.05;
                w.with_waste = (wire_count * 1.05).ceil();
                items.push(w);
            }
        }

        // ---- 4. Specialties by room type (the company standards table) ----
        if let Some(r) = rule_for(&room.name, rules) {
            for it in &r.items {
                if it.quantity > 0.0 && it.quantity.is_finite() {
                    items.push(rule_line("specialties", &it.description, room, it.quantity, &it.category, &r.rule));
                }
            }
        }

        // Signage: room sign at every door.
        if !doors.is_empty() {
            items.push(rule_line(
                "specialties",
                "ROOM IDENTIFICATION SIGN, ADA",
                room,
                1.0,
                "signage",
                "one room identification sign at each room with a door: company standard",
            ));
        }

        // Window treatments: one per window in this room.
        let window_count = input
            .windows
            .iter()
            .filter(|w| {
                if w.level != room.level {
                    return false;
                }
                let wt = input.window_types.iter().find(|t| t.mark == w.mark);
                let storefront = wt.is_some_and(|t| t.storefront);
                if storefront {
                    return false; // storefront gets no treatments
                }
                w.at_ft[0] > room.min_ft[0] - 1.0
                    && w.at_ft[0] < room.max_ft[0] + 1.0
                    && w.at_ft[1] > room.min_ft[1] - 1.0
                    && w.at_ft[1] < room.max_ft[1] + 1.0
            })
            .count();
        if window_count > 0 {
            items.push(rule_line(
                "specialties",
                "WINDOW TREATMENT, ROLLER SHADE",
                room,
                window_count as f64,
                "accessory",
                "one roller shade per window that is not storefront: company standard",
            ));
        }
    }

    // ---- 5. Transitions: one per door between two floor materials ----
    let room_of = |level: &str, n: Option<&str>| n.and_then(|n| input.rooms.iter().find(|r| r.level == level && r.number == n));
    for door in &input.doors {
        let (Some(a), Some(b)) = (room_of(&door.level, door.swings_into.as_deref()), room_of(&door.level, door.from_room.as_deref())) else {
            continue;
        };
        let (Some(fa), Some(fb)) = (&a.floor, &b.floor) else { continue };
        if fa == fb {
            continue;
        }
        let (ca, cb) = (code_to_category(&code_description(codes, fa)), code_to_category(&code_description(codes, fb)));
        let (lo, hi) = if fa <= fb { (fa, fb) } else { (fb, fa) };
        let mark = door.mark.as_deref().unwrap_or("(UNMARKED)");
        let mut it =
            line("flooring", format!("FLOOR TRANSITION, {}: {lo} TO {hi}", transition_type(ca, cb)), None, a, 1.0, "EA", 0.0, "accessory", "");
        it.source = format!(
            "{} plan door {mark}; {} ROOM FINISHES rows {} and {}",
            if door.plan_sheet.is_empty() { "floor" } else { &door.plan_sheet },
            if input.finish_schedule.is_empty() { "room finish schedule" } else { &input.finish_schedule },
            a.number,
            b.number
        );
        it.schedule_row = None;
        items.push(it);
    }

    // ---- 6. Doors, frames, hardware sets ----
    for door in &input.doors {
        let room_no = door.swings_into.clone().or_else(|| door.from_room.clone()).unwrap_or_default();
        let rm = room_of(&door.level, Some(&room_no));
        let rm_name = rm.map_or("", |r| r.name.as_str());
        let mark = door.mark.clone().unwrap_or_else(|| "(UNMARKED)".into());
        let height = door.height_in.unwrap_or(84.0);
        let plan = if door.plan_sheet.is_empty() { "floor".to_string() } else { door.plan_sheet.clone() };
        let (source, status, why) = if door.scheduled {
            let ds = if input.door_schedule.is_empty() { "door schedule".to_string() } else { format!("{} DOOR SCHEDULE", input.door_schedule) };
            (format!("{plan} plan door {mark}; {ds} row {mark}"), "schedule", None)
        } else {
            (
                format!("{plan} plan door {mark}"),
                "guess",
                Some("no door schedule row: width as drawn, 7'-0\" height assumed; material, frame and hardware unknown".to_string()),
            )
        };
        let mk = |description: String, category: &str| TakeoffItem {
            trade: "doors_frames_hardware".into(),
            description,
            finish_code: None,
            room: room_no.clone(),
            room_name: rm_name.into(),
            level: door.level.clone(),
            quantity: 1.0,
            unit: "EA".into(),
            waste_factor: 0.0,
            with_waste: 1.0,
            category: category.into(),
            source: source.clone(),
            status: status.into(),
            rule: why.clone(),
            schedule_row: door.scheduled.then(|| mark.clone()),
            coats: None,
            sheen: None,
            unit_type: rm.and_then(|r| r.unit_type.clone()),
        };

        // Door leaf
        items.push(mk(
            format!(
                "DOOR {mark} — {w}x{h} {mat} TYPE {dt}",
                w = inches_to_ft_in(door.width_in),
                h = inches_to_ft_in(height),
                mat = door.material,
                dt = door.door_type,
            ),
            "door",
        ));

        // Frame
        items.push(mk(
            format!(
                "FRAME {w}x{h} {fr}{rating}",
                w = inches_to_ft_in(door.width_in),
                h = inches_to_ft_in(height),
                fr = door.frame,
                rating = door.fire_rating.as_ref().map_or(String::new(), |r| format!(" {r} RATED")),
            ),
            "frame",
        ));

        // Hardware set
        let hw_items = input.hardware.iter().find(|g| g.group == door.hardware).map_or_else(|| door.hardware.clone(), |g| g.items.join(", "));
        items.push(mk(format!("HARDWARE SET {}: {}", door.hardware, hw_items), "hardware"));
    }

    // ---- 7. Windows and glazing ----
    // Group by type.
    let mut win_counts: BTreeMap<String, usize> = BTreeMap::new();
    for w in &input.windows {
        *win_counts.entry(w.mark.clone()).or_default() += 1;
    }
    for (mark, n) in &win_counts {
        if !input.window_types.iter().any(|t| &t.mark == mark) {
            missing.push(format!("{n} window(s) marked {mark}: no row in the window schedule, so no glazing line"));
        }
    }
    let mut plans: Vec<&str> = input.windows.iter().map(|w| w.plan_sheet.as_str()).filter(|s| !s.is_empty()).collect();
    plans.sort_unstable();
    plans.dedup();
    let ws = if input.window_schedule.is_empty() { "window schedule".to_string() } else { format!("{} WINDOW SCHEDULE", input.window_schedule) };
    for wt in &input.window_types {
        let count = win_counts.get(&wt.mark).copied().unwrap_or(0);
        if count > 0 {
            let source = format!("{} plans; {ws} row {}", if plans.is_empty() { "floor".to_string() } else { plans.join(", ") }, wt.mark);
            let mk = |description: String, quantity: f64, wf: f64, category: &str| TakeoffItem {
                trade: "glazing".into(),
                description,
                finish_code: None,
                room: "BUILDING".into(),
                room_name: "".into(),
                level: "ALL".into(),
                quantity: round2(quantity),
                unit: if category == "glass" { "SF".into() } else { "EA".into() },
                waste_factor: wf,
                with_waste: round2(quantity * (1.0 + wf)),
                category: category.into(),
                source: source.clone(),
                status: "schedule".into(),
                rule: None,
                schedule_row: Some(wt.mark.clone()),
                coats: None,
                sheen: None,
                unit_type: None,
            };
            items.push(mk(
                format!(
                    "WINDOW TYPE {mark}: {desc} {w}x{h}, {frame}",
                    mark = wt.mark,
                    desc = wt.description,
                    w = inches_to_ft_in(wt.width_in),
                    h = inches_to_ft_in(wt.height_in),
                    frame = wt.frame,
                ),
                count as f64,
                0.0,
                "window",
            ));
            // Glazing area.
            let glass_sf = (wt.width_in / 12.0) * (wt.height_in / 12.0) * count as f64;
            items.push(mk(format!("GLAZING — {}", wt.glazing), glass_sf, waste_factor("glass"), "glass"));
        }
    }

    finish(items, missing)
}

/// Build the room, trade and unit-type roll-ups.
fn finish(items: Vec<TakeoffItem>, missing: Vec<String>) -> FinishTakeoff {
    let mut by_room: BTreeMap<String, Vec<TakeoffItem>> = BTreeMap::new();
    for item in &items {
        by_room.entry(format!("{}-{}", item.level, item.room)).or_default().push(item.clone());
    }

    let trade_names = ["flooring", "painting", "ceilings", "doors_frames_hardware", "specialties", "glazing"];
    let trades: Vec<TradeSummary> = trade_names
        .iter()
        .filter_map(|tn| {
            let trade_items: Vec<_> = items.iter().filter(|i| i.trade == *tn).cloned().collect();
            if trade_items.is_empty() {
                return None;
            }
            let count = trade_items.len();
            Some(TradeSummary { trade: tn.to_string(), items: trade_items, line_count: count })
        })
        .collect();

    let mut by_unit_type: BTreeMap<String, Vec<UnitTypeLine>> = BTreeMap::new();
    let mut rooms_seen: BTreeMap<(String, String, String, String), Vec<String>> = BTreeMap::new();
    for i in &items {
        let Some(ut) = &i.unit_type else { continue };
        let lines = by_unit_type.entry(ut.clone()).or_default();
        let key = (ut.clone(), i.trade.clone(), i.description.clone(), i.unit.clone());
        let rooms = rooms_seen.entry(key).or_default();
        let room = format!("{}-{}", i.level, i.room);
        let new_room = !rooms.contains(&room);
        if new_room {
            rooms.push(room);
        }
        match lines.iter_mut().find(|l| l.trade == i.trade && l.description == i.description && l.unit == i.unit) {
            Some(l) => {
                l.quantity = round2(l.quantity + i.quantity);
                l.with_waste = round2(l.with_waste + i.with_waste);
                l.rooms += usize::from(new_room);
            }
            None => lines.push(UnitTypeLine {
                trade: i.trade.clone(),
                description: i.description.clone(),
                unit: i.unit.clone(),
                quantity: i.quantity,
                with_waste: i.with_waste,
                rooms: 1,
            }),
        }
    }

    FinishTakeoff { items, trades, by_room, by_unit_type, missing }
}

// ---- Helpers --------------------------------------------------------------------------------

fn code_description(codes: &[FinishCode], code: &str) -> String {
    codes.iter().find(|c| c.code == code).map_or_else(|| code.to_string(), |c| format!("{}: {}", c.code, c.description))
}

/// Parse "...TO 7'-0\"..." → Some(7.0).
fn parse_tile_height(desc: &str) -> Option<f64> {
    let upper = desc.to_ascii_uppercase();
    let idx = upper.find("TO ")?;
    let rest = &upper[idx + 3..];
    // Look for a feet-inches pattern.
    let ft_idx = rest.find('\'')?;
    let ft_str = rest[..ft_idx].trim();
    let ft: f64 = ft_str.parse().ok()?;
    // Try to get inches after the tick.
    let after = &rest[ft_idx + 1..];
    let inches: f64 = if let Some(in_end) = after.find('"') {
        let in_str = after[..in_end].trim_start_matches('-').trim();
        in_str.parse().unwrap_or(0.0)
    } else {
        0.0
    };
    Some(ft + inches / 12.0)
}

fn inches_to_ft_in(inches: f64) -> String {
    let ft = (inches / 12.0).floor() as i64;
    let rem = (inches - ft as f64 * 12.0).round() as i64;
    if rem == 0 { format!("{ft}'-0\"") } else { format!("{ft}'-{rem}\"") }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ---- Aggregation helpers --------------------------------------------------------------------

/// Aggregate items by description, summing quantities: returns (description, total_quantity,
/// total_with_waste, unit).
pub fn aggregate_by_description(items: &[TakeoffItem]) -> Vec<(String, f64, f64, String)> {
    let mut map: BTreeMap<String, (f64, f64, String)> = BTreeMap::new();
    for item in items {
        let entry = map.entry(item.description.clone()).or_insert((0.0, 0.0, item.unit.clone()));
        entry.0 += item.quantity;
        entry.1 += item.with_waste;
    }
    map.into_iter().map(|(desc, (q, w, u))| (desc, round2(q), round2(w), u)).collect()
}

/// Aggregate items by trade: returns (trade, total_items, descriptions).
pub fn aggregate_by_trade(takeoff: &FinishTakeoff) -> Vec<(String, usize)> {
    takeoff.trades.iter().map(|t| (t.trade.clone(), t.line_count)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waste_factors_are_nonnegative() {
        for cat in &[
            "carpet_tile",
            "carpet_broadloom",
            "lvt",
            "vct",
            "ceramic_tile",
            "porcelain_tile",
            "rubber_treads",
            "sealed_concrete",
            "rubber_base",
            "cove_base",
            "paint",
            "wallcovering",
            "wall_tile",
            "act_tile",
            "gypsum_board",
            "door",
            "frame",
            "hardware",
            "window",
            "glass",
            "partition",
            "accessory",
            "signage",
            "corner_guard",
            "fire_extinguisher",
        ] {
            assert!(waste_factor(cat) >= 0.0, "{cat}");
            assert!(waste_factor(cat) <= 1.0, "{cat}");
        }
    }

    #[test]
    fn inches_to_ft_in_format() {
        assert_eq!(inches_to_ft_in(36.0), "3'-0\"");
        assert_eq!(inches_to_ft_in(84.0), "7'-0\"");
        assert_eq!(inches_to_ft_in(90.0), "7'-6\"");
    }

    #[test]
    fn parse_tile_height_works() {
        assert_eq!(parse_tile_height("PORCELAIN WALL TILE TO 7'-0\", PAINT ABOVE"), Some(7.0));
        assert_eq!(parse_tile_height("TILE TO 4'-6\""), Some(4.5));
        assert_eq!(parse_tile_height("PAINT, EGGSHELL"), None);
    }

    #[test]
    fn round2_precision() {
        assert_eq!(round2(12.345), 12.35);
        assert_eq!(round2(100.005), 100.01);
    }

    #[test]
    fn categories_put_gypsum_before_paint() {
        assert_eq!(code_to_category("GYP-1: PAINTED GYPSUM BOARD"), "gypsum_board");
        assert_eq!(code_to_category("PT-1: PAINT, EGGSHELL, COLOR: PALE GRAY"), "paint");
        assert_eq!(code_to_category("PT-2: EPOXY PAINT, SEMI-GLOSS, COLOR: OFF-WHITE"), "paint");
        assert_eq!(code_to_category("FRP-1: FIBERGLASS REINFORCED PANELS (FRP)"), "wallcovering");
        assert_eq!(code_to_category("ACT-1: ACOUSTIC CEILING TILE, 2x4"), "act_tile");
    }

    #[test]
    fn coats_sheen_tiles_and_patterns_read_from_the_code_text() {
        assert_eq!(parse_coats("PAINT, EGGSHELL, 3 COATS"), Some(3));
        assert_eq!(parse_coats("PAINT, (2) COATS OVER PRIMER"), Some(2));
        assert_eq!(parse_coats("EPOXY PAINT, TWO COATS"), Some(2));
        assert_eq!(parse_coats("PAINT, EGGSHELL"), None);
        assert_eq!(parse_sheen("EPOXY PAINT, SEMI-GLOSS"), Some("SEMI-GLOSS".into()));
        assert_eq!(parse_sheen("PAINT, EGGSHELL, ACCENT"), Some("EGGSHELL".into()));
        assert_eq!(tile_size("ACOUSTIC CEILING TILE, 2x4, SUSPENDED GRID"), Some([2.0, 4.0]));
        assert_eq!(tile_size("ACT 24X24"), Some([2.0, 2.0]));
        assert_eq!(tile_size("PAINTED GYPSUM BOARD"), None);
        assert_eq!(pattern_of("CARPET TILE, HERRINGBONE INSTALL"), Some("HERRINGBONE"));
        assert_eq!(pattern_of("CARPET TILE"), None);
    }

    #[test]
    fn room_rules_match_whole_words() {
        let rules = default_room_rules();
        let names = |n: &str| rule_for(n, &rules).map(|r| r.names.first().cloned().unwrap_or_default());
        assert_eq!(names("WOMEN"), Some("WOMEN".into()));
        assert_eq!(names("MEN'S TOILET"), Some("MEN".into()));
        assert_eq!(names("GUEST BATH"), Some("GUEST BATH".into()));
        assert_eq!(names("CORRIDOR"), Some("CORRIDOR".into()));
        assert_eq!(names("OFFICE"), None);
        assert_eq!(names("MENTORING"), None);
    }
}
