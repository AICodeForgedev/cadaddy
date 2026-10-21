//! Q338: Finish takeoff — quantities by room and trade from the architecture truth.
//!
//! From the finish schedule, room finish plans and specs: flooring (carpet, LVT, tile, sealed
//! concrete) with transitions and base, wall finishes (paint by coats/sheen, wallcovering,
//! tile/FRP), ceilings (ACT grid/tile, gypsum), doors/frames/hardware sets, toilet partitions
//! and accessories, signage, window treatments, fire extinguishers/cabinets, corner guards —
//! quantities by room/unit type, with waste and pattern rules; per-trade takeoff ready for
//! pricing.
//!
//! The input is a [`Truth`] whose `arch` and `building.rooms` are populated (Q307).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::truth::{FinishCode, PlanWall, Room, Truth};

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

/// Map a finish code description to a material category (for waste factor lookup).
fn code_to_category(desc: &str) -> &'static str {
    let d = desc.to_ascii_uppercase();
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
    } else if d.contains("PAINT") || d.contains("EPOXY") {
        "paint"
    } else if d.contains("WALLCOVERING") || d.contains("FRP") {
        "wallcovering"
    } else if d.contains("WALL TILE") || (d.contains("TILE") && d.contains("WALL")) {
        "wall_tile"
    } else if d.contains("ACOUSTIC") || d.contains("ACT") {
        "act_tile"
    } else if d.contains("GYPSUM") || d.contains("GYP") {
        "gypsum_board"
    } else {
        // Exposed structure (painted) and anything else defaults to paint.
        "paint"
    }
}

// ---- Room geometry --------------------------------------------------------------------------

/// Room floor area in square feet from its bounds.
pub fn room_area_sf(room: &Room) -> f64 {
    let dx = (room.max_ft[0] - room.min_ft[0]).max(0.0);
    let dy = (room.max_ft[1] - room.min_ft[1]).max(0.0);
    dx * dy
}

/// Room perimeter in linear feet from its bounds.
pub fn room_perimeter_lf(room: &Room) -> f64 {
    let dx = (room.max_ft[0] - room.min_ft[0]).max(0.0);
    let dy = (room.max_ft[1] - room.min_ft[1]).max(0.0);
    2.0 * (dx + dy)
}

/// Walls bounding a room, with their length (feet) and net height (feet) there. Openings are
/// subtracted from the wall area. Returns `(wall_id, gross_length_ft, net_area_sf)`.
fn room_walls<'a>(room: &Room, walls: &'a [PlanWall]) -> Vec<(&'a PlanWall, f64, f64)> {
    let mut out = Vec::new();
    for wall in walls {
        if wall.level != room.level {
            continue;
        }
        if !wall.rooms.contains(&room.number) {
            continue;
        }
        let len = wall_length(wall);
        // Net area: full height minus openings.
        let opening_area: f64 = wall
            .openings
            .iter()
            .map(|o| {
                let w = (o[1] - o[0]).abs();
                // Approximate: full-height opening (door height ≈ 7 ft).
                w * 7.0_f64.min(wall.height_ft)
            })
            .sum();
        let gross = len * wall.height_ft;
        let net = (gross - opening_area).max(0.0);
        out.push((wall, len, net));
    }
    out
}

fn wall_length(wall: &PlanWall) -> f64 {
    let dx = wall.b_ft[0] - wall.a_ft[0];
    let dy = wall.b_ft[1] - wall.a_ft[1];
    dx.hypot(dy)
}

/// Net wall area for a room (openings subtracted), square feet.
fn room_net_wall_sf(room: &Room, walls: &[PlanWall]) -> f64 {
    room_walls(room, walls).iter().map(|(_, _, net)| *net).sum()
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
}

/// A per-trade summary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TradeSummary {
    pub trade: String,
    pub items: Vec<TakeoffItem>,
    /// Total lines.
    pub line_count: usize,
}

/// The complete finish takeoff for a building.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FinishTakeoff {
    pub items: Vec<TakeoffItem>,
    /// Per-trade summaries.
    pub trades: Vec<TradeSummary>,
    /// Room-level roll-ups: room number → items.
    pub by_room: BTreeMap<String, Vec<TakeoffItem>>,
}

// ---- The takeoff engine ---------------------------------------------------------------------

/// Compute the full finish takeoff from a truth set.
///
/// Requires `truth.arch` (Q307) and `truth.building.rooms` to be populated.
pub fn compute_takeoff(truth: &Truth) -> FinishTakeoff {
    let arch = match truth.arch.as_ref() {
        Some(a) => a,
        None => return FinishTakeoff::default(),
    };
    let rooms = &truth.building.rooms;
    let walls = &truth.plan_walls;
    let finishes = &arch.finishes;
    let codes = &arch.finish_codes;
    let mut items = Vec::new();

    // ---- 1. Flooring (by room) ----
    for rf in finishes {
        let Some(room) = rooms.iter().find(|r| r.level == rf.level && r.number == rf.number) else {
            continue;
        };
        let area = room_area_sf(room);
        let perim = room_perimeter_lf(room);
        let desc = code_description(codes, &rf.floor);
        let cat = code_to_category(&desc);
        let wf = waste_factor(cat);

        // Floor finish
        items.push(TakeoffItem {
            trade: "flooring".into(),
            description: desc.clone(),
            finish_code: Some(rf.floor.clone()),
            room: rf.number.clone(),
            room_name: rf.name.clone(),
            level: rf.level.clone(),
            quantity: round2(area),
            unit: "SF".into(),
            waste_factor: wf,
            with_waste: round2(area * (1.0 + wf)),
            category: cat.into(),
        });

        // Transitions: one per door opening in the room (where flooring meets the adjacent room's).
        let door_count = arch
            .doors
            .iter()
            .filter(|d| d.level == rf.level && (d.swings_into.as_deref() == Some(&rf.number) || d.from_room.as_deref() == Some(&rf.number)))
            .count();
        if door_count > 0 {
            items.push(TakeoffItem {
                trade: "flooring".into(),
                description: "FLOOR TRANSITION STRIP".into(),
                finish_code: None,
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: door_count as f64,
                unit: "EA".into(),
                waste_factor: 0.0,
                with_waste: door_count as f64,
                category: "accessory".into(),
            });
        }

        // Base
        let base_desc = code_description(codes, &rf.base);
        let base_cat = code_to_category(&base_desc);
        let base_wf = waste_factor(base_cat);
        // Subtract door openings from base perimeter.
        let door_width_lf: f64 = arch
            .doors
            .iter()
            .filter(|d| d.level == rf.level && (d.swings_into.as_deref() == Some(&rf.number) || d.from_room.as_deref() == Some(&rf.number)))
            .map(|d| d.width_in / 12.0)
            .sum();
        let base_lf = (perim - door_width_lf).max(0.0);
        items.push(TakeoffItem {
            trade: "flooring".into(),
            description: base_desc,
            finish_code: Some(rf.base.clone()),
            room: rf.number.clone(),
            room_name: rf.name.clone(),
            level: rf.level.clone(),
            quantity: round2(base_lf),
            unit: "LF".into(),
            waste_factor: base_wf,
            with_waste: round2(base_lf * (1.0 + base_wf)),
            category: base_cat.into(),
        });

        // ---- 2. PlanWall finishes ----
        let wall_desc = code_description(codes, &rf.wall);
        let wall_cat = code_to_category(&wall_desc);
        let wall_wf = waste_factor(wall_cat);
        let net_wall = room_net_wall_sf(room, walls);
        // If wall code is tile to a height (e.g. "PORCELAIN WALL TILE TO 7'-0\", PAINT ABOVE"),
        // split the wall area into tile zone + paint zone.
        let tile_height = parse_tile_height(&wall_desc);
        if let Some(th) = tile_height {
            // Tile area: perimeter × tile height minus openings below tile height
            let tile_area = wall_tile_area(room, walls, th);
            let paint_area = (net_wall - tile_area).max(0.0);
            items.push(TakeoffItem {
                trade: "painting".into(),
                description: format!("WALL TILE ({})", wall_desc),
                finish_code: Some(rf.wall.clone()),
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: round2(tile_area),
                unit: "SF".into(),
                waste_factor: waste_factor("wall_tile"),
                with_waste: round2(tile_area * (1.0 + waste_factor("wall_tile"))),
                category: "wall_tile".into(),
            });
            if paint_area > 1.0 {
                items.push(TakeoffItem {
                    trade: "painting".into(),
                    description: "PAINT ABOVE TILE".into(),
                    finish_code: Some(rf.wall.clone()),
                    room: rf.number.clone(),
                    room_name: rf.name.clone(),
                    level: rf.level.clone(),
                    quantity: round2(paint_area),
                    unit: "SF".into(),
                    waste_factor: waste_factor("paint"),
                    with_waste: round2(paint_area * (1.0 + waste_factor("paint"))),
                    category: "paint".into(),
                });
            }
        } else {
            items.push(TakeoffItem {
                trade: "painting".into(),
                description: wall_desc,
                finish_code: Some(rf.wall.clone()),
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: round2(net_wall),
                unit: "SF".into(),
                waste_factor: wall_wf,
                with_waste: round2(net_wall * (1.0 + wall_wf)),
                category: wall_cat.into(),
            });
        }

        // Paint the ceiling for "PAINTED GYPSUM BOARD" or "EXPOSED STRUCTURE, PAINTED".
        let ceiling_desc = code_description(codes, &rf.ceiling);
        let clg_cat = code_to_category(&ceiling_desc);
        let clg_wf = waste_factor(clg_cat);
        if ceiling_desc.to_ascii_uppercase().contains("PAINT")
            || ceiling_desc.to_ascii_uppercase().contains("GYPSUM")
            || ceiling_desc.to_ascii_uppercase().contains("EXPOSED")
        {
            // Paint for ceiling too
            items.push(TakeoffItem {
                trade: "painting".into(),
                description: format!("CEILING PAINT ({})", ceiling_desc),
                finish_code: Some(rf.ceiling.clone()),
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: round2(area),
                unit: "SF".into(),
                waste_factor: waste_factor("paint"),
                with_waste: round2(area * (1.0 + waste_factor("paint"))),
                category: "paint".into(),
            });
        }

        // ---- 3. Ceilings ----
        items.push(TakeoffItem {
            trade: "ceilings".into(),
            description: ceiling_desc,
            finish_code: Some(rf.ceiling.clone()),
            room: rf.number.clone(),
            room_name: rf.name.clone(),
            level: rf.level.clone(),
            quantity: round2(area),
            unit: "SF".into(),
            waste_factor: clg_wf,
            with_waste: round2(area * (1.0 + clg_wf)),
            category: clg_cat.into(),
        });

        // ACT grid: linear feet of main tee and cross tee.
        if clg_cat == "act_tile" {
            let grid = room.ceiling.as_ref().and_then(|c| c.grid_ft);
            let (across, along) = grid.map_or((2.0, 4.0), |g| (g[0], g[1]));
            let dx = (room.max_ft[0] - room.min_ft[0]).max(0.0);
            let dy = (room.max_ft[1] - room.min_ft[1]).max(0.0);
            // Main tees run along the longer dimension, cross tees across.
            let main_rows = (dy / along).ceil().max(1.0);
            let cross_rows = (dx / across).ceil().max(1.0);
            let main_lf = main_rows * dx;
            let cross_lf = cross_rows * dy;
            let grid_lf = main_lf + cross_lf;
            items.push(TakeoffItem {
                trade: "ceilings".into(),
                description: format!("CEILING GRID ({}x{} MAIN+CROSS TEE)", across, along),
                finish_code: None,
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: round2(grid_lf),
                unit: "LF".into(),
                waste_factor: 0.03,
                with_waste: round2(grid_lf * 1.03),
                category: "act_tile".into(),
            });
            // PlanWall angle (perimeter trim).
            items.push(TakeoffItem {
                trade: "ceilings".into(),
                description: "WALL ANGLE (PERIMETER TRIM)".into(),
                finish_code: None,
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: round2(perim),
                unit: "LF".into(),
                waste_factor: 0.05,
                with_waste: round2(perim * 1.05),
                category: "act_tile".into(),
            });
            // Suspension wire: one every 4 ft × 4 ft.
            let wire_count = ((area / 16.0).ceil() as usize).max(1);
            items.push(TakeoffItem {
                trade: "ceilings".into(),
                description: "SUSPENSION WIRE".into(),
                finish_code: None,
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: wire_count as f64,
                unit: "EA".into(),
                waste_factor: 0.05,
                with_waste: (wire_count as f64 * 1.05).ceil(),
                category: "act_tile".into(),
            });
        }

        // ---- 4. Specialties by room type ----

        // Toilet partitions and accessories
        if is_wet_room(&rf.name) {
            // Toilet partitions: assume 2 per restroom (standard 2-stall restroom).
            items.push(TakeoffItem {
                trade: "specialties".into(),
                description: "TOILET PARTITION (FLOOR-MOUNTED, OVERHEAD-BRACED)".into(),
                finish_code: None,
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: 2.0,
                unit: "EA".into(),
                waste_factor: 0.0,
                with_waste: 2.0,
                category: "partition".into(),
            });
            // Toilet accessories.
            for (desc, qty) in toilet_accessories() {
                items.push(TakeoffItem {
                    trade: "specialties".into(),
                    description: desc.into(),
                    finish_code: None,
                    room: rf.number.clone(),
                    room_name: rf.name.clone(),
                    level: rf.level.clone(),
                    quantity: qty as f64,
                    unit: "EA".into(),
                    waste_factor: 0.0,
                    with_waste: qty as f64,
                    category: "accessory".into(),
                });
            }
        }

        // Corner guards: at exterior corners of corridors and high-traffic rooms.
        if matches!(rf.name.as_str(), "CORRIDOR" | "BREAK ROOM" | "WORK ROOM" | "COPY") {
            // 2 corner guards per room (typical outside corners).
            items.push(TakeoffItem {
                trade: "specialties".into(),
                description: "CORNER GUARD, STAINLESS STEEL, 48\"".into(),
                finish_code: None,
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: 2.0,
                unit: "EA".into(),
                waste_factor: 0.0,
                with_waste: 2.0,
                category: "corner_guard".into(),
            });
        }

        // Fire extinguisher cabinet: one per corridor (per level), one in mechanical.
        if rf.name == "CORRIDOR" || rf.name == "MECH" {
            items.push(TakeoffItem {
                trade: "specialties".into(),
                description: "FIRE EXTINGUISHER CABINET, SEMI-RECESSED".into(),
                finish_code: None,
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: 1.0,
                unit: "EA".into(),
                waste_factor: 0.0,
                with_waste: 1.0,
                category: "fire_extinguisher".into(),
            });
        }

        // Signage: room sign at every door.
        if door_count > 0 {
            items.push(TakeoffItem {
                trade: "specialties".into(),
                description: "ROOM IDENTIFICATION SIGN, ADA".into(),
                finish_code: None,
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: 1.0,
                unit: "EA".into(),
                waste_factor: 0.0,
                with_waste: 1.0,
                category: "signage".into(),
            });
        }

        // Window treatments: one per window in this room.
        let window_count = arch
            .windows
            .iter()
            .filter(|w| {
                if w.level != rf.level {
                    return false;
                }
                // Check if window is in this room's bounds.
                let wt = arch.window_types.iter().find(|t| t.mark == w.mark);
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
            items.push(TakeoffItem {
                trade: "specialties".into(),
                description: "WINDOW TREATMENT, ROLLER SHADE".into(),
                finish_code: None,
                room: rf.number.clone(),
                room_name: rf.name.clone(),
                level: rf.level.clone(),
                quantity: window_count as f64,
                unit: "EA".into(),
                waste_factor: 0.0,
                with_waste: window_count as f64,
                category: "accessory".into(),
            });
        }
    }

    // ---- 5. Doors, frames, hardware sets ----
    for door in &arch.doors {
        let rm_name = door
            .swings_into
            .as_deref()
            .or(door.from_room.as_deref())
            .and_then(|n| finishes.iter().find(|f| f.number == n && f.level == door.level))
            .map_or("", |f| f.name.as_str());

        // Door leaf
        items.push(TakeoffItem {
            trade: "doors_frames_hardware".into(),
            description: format!(
                "DOOR {mark} — {w}x{h} {mat} TYPE {dt}",
                mark = door.mark,
                w = inches_to_ft_in(door.width_in),
                h = inches_to_ft_in(door.height_in),
                mat = door.material,
                dt = door.door_type,
            ),
            finish_code: None,
            room: door.swings_into.clone().or_else(|| door.from_room.clone()).unwrap_or_default(),
            room_name: rm_name.into(),
            level: door.level.clone(),
            quantity: 1.0,
            unit: "EA".into(),
            waste_factor: 0.0,
            with_waste: 1.0,
            category: "door".into(),
        });

        // Frame
        items.push(TakeoffItem {
            trade: "doors_frames_hardware".into(),
            description: format!(
                "FRAME {w}x{h} {fr}{rating}",
                w = inches_to_ft_in(door.width_in),
                h = inches_to_ft_in(door.height_in),
                fr = door.frame,
                rating = door.fire_rating.as_ref().map_or(String::new(), |r| format!(" {r} RATED")),
            ),
            finish_code: None,
            room: door.swings_into.clone().or_else(|| door.from_room.clone()).unwrap_or_default(),
            room_name: rm_name.into(),
            level: door.level.clone(),
            quantity: 1.0,
            unit: "EA".into(),
            waste_factor: 0.0,
            with_waste: 1.0,
            category: "frame".into(),
        });

        // Hardware set
        let hw_items = arch.hardware.iter().find(|g| g.group == door.hardware).map_or_else(|| door.hardware.clone(), |g| g.items.join(", "));
        items.push(TakeoffItem {
            trade: "doors_frames_hardware".into(),
            description: format!("HARDWARE SET {}: {}", door.hardware, hw_items),
            finish_code: None,
            room: door.swings_into.clone().or_else(|| door.from_room.clone()).unwrap_or_default(),
            room_name: rm_name.into(),
            level: door.level.clone(),
            quantity: 1.0,
            unit: "EA".into(),
            waste_factor: 0.0,
            with_waste: 1.0,
            category: "hardware".into(),
        });
    }

    // ---- 6. Windows and glazing ----
    // Group by type.
    let mut win_counts: BTreeMap<String, usize> = BTreeMap::new();
    for w in &arch.windows {
        *win_counts.entry(w.mark.clone()).or_default() += 1;
    }
    for wt in &arch.window_types {
        let count = win_counts.get(&wt.mark).copied().unwrap_or(0);
        if count > 0 {
            items.push(TakeoffItem {
                trade: "glazing".into(),
                description: format!(
                    "WINDOW TYPE {mark}: {desc} {w}x{h}, {frame}",
                    mark = wt.mark,
                    desc = wt.description,
                    w = inches_to_ft_in(wt.width_in),
                    h = inches_to_ft_in(wt.height_in),
                    frame = wt.frame,
                ),
                finish_code: None,
                room: "BUILDING".into(),
                room_name: "".into(),
                level: "ALL".into(),
                quantity: count as f64,
                unit: "EA".into(),
                waste_factor: 0.0,
                with_waste: count as f64,
                category: "window".into(),
            });
            // Glazing area.
            let glass_sf = (wt.width_in / 12.0) * (wt.height_in / 12.0) * count as f64;
            items.push(TakeoffItem {
                trade: "glazing".into(),
                description: format!("GLAZING — {}", wt.glazing),
                finish_code: None,
                room: "BUILDING".into(),
                room_name: "".into(),
                level: "ALL".into(),
                quantity: round2(glass_sf),
                unit: "SF".into(),
                waste_factor: waste_factor("glass"),
                with_waste: round2(glass_sf * (1.0 + waste_factor("glass"))),
                category: "glass".into(),
            });
        }
    }

    // ---- 7. Build room and trade roll-ups ----
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

    FinishTakeoff { items, trades, by_room }
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

/// PlanWall tile area in a room where tile goes to `tile_height_ft` above the floor.
fn wall_tile_area(room: &Room, walls: &[PlanWall], tile_height_ft: f64) -> f64 {
    let rw = room_walls(room, walls);
    let mut area = 0.0;
    for (wall, len, _) in &rw {
        // Tile height capped at wall height.
        let th = tile_height_ft.min(wall.height_ft);
        let gross = len * th;
        // Subtract openings that are below tile height.
        let opening_area: f64 = wall
            .openings
            .iter()
            .map(|o| {
                let w = (o[1] - o[0]).abs();
                w * th.min(7.0)
            })
            .sum();
        area += (gross - opening_area).max(0.0);
    }
    area
}

/// Whether a room is a wet room (restroom).
fn is_wet_room(name: &str) -> bool {
    matches!(name, "MEN" | "WOMEN" | "TOILET" | "RESTROOM")
}

/// Standard toilet accessories per restroom.
fn toilet_accessories() -> Vec<(&'static str, usize)> {
    vec![
        ("TOILET PAPER HOLDER, RECESSED", 2),
        ("GRAB BAR SET (ADA), 36\"+42\"", 1),
        ("SOAP DISPENSER, SURFACE-MOUNTED", 2),
        ("PAPER TOWEL DISPENSER / WASTE", 1),
        ("MIRROR, FULL-WIDTH ABOVE LAVATORY", 1),
        ("FEMININE HYGIENE DISPENSER", 1),
        ("SEAT COVER DISPENSER", 2),
        ("COAT HOOK", 2),
        ("SANITARY NAPKIN DISPOSAL", 2),
    ]
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
}
