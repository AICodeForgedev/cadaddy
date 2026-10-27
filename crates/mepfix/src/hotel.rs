//! A made-up 4-storey hotel: guest rooms along double-loaded corridors on floors 2–4, public
//! spaces on the ground floor, stacked wet walls, elevator + stairs, mechanical/electrical/IDF
//! rooms, roof with RTUs, and a site with parking, light poles, a transformer pad and
//! underground utilities. Extends the MEP fixture truth with wall assemblies, ceiling assemblies,
//! casework, a spec book, fire alarm, telecom, and site elements.
//!
//! Coordinates: building feet, origin at grid A/1 (south-west corner of the building).

use crate::design;
use crate::rng::Rng;
use crate::style::Style;
use crate::truth::*;
// BTreeMap used by some equipment schedules.
#[allow(unused_imports)]
use std::collections::BTreeMap;

/// Size of the generated hotel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotelSize {
    /// 4 rooms per side per floor (quick CI).
    Small,
    /// 10 rooms per side per floor (realistic).
    Full,
    /// 20 rooms per side per floor: twice the full hotel, for the end-to-end performance check.
    Double,
}

/// The design result before sheets are laid out.
pub struct HotelDesign {
    pub truth: Truth,
    /// Segment ids that were intentionally placed to cause clashes.
    pub planted: Vec<String>,
}

/// Build the hotel's truth (before sheets).
pub fn hotel_design(seed: u64, st: &Style, size: HotelSize, r: &mut Rng) -> HotelDesign {
    let rooms_per_side: usize = match size {
        HotelSize::Small => 4,
        HotelSize::Full => 10,
        HotelSize::Double => 20,
    };

    let mut t = Truth { schema: SCHEMA.to_string(), seed, project: project_name(r), kind: "hotel".into(), ..Truth::default() };

    // --- Grid ---
    let room_width_ft = 13.0;
    let corridor_width_ft = 6.0;
    let room_depth_ft = 24.0;
    let building_depth_ft = room_depth_ft * 2.0 + corridor_width_ft; // 54 ft N-S
    let stair_width_ft = 12.0;
    let elev_width_ft = 10.0;

    // X grid: stair + rooms + elevator area + rooms + stair
    // We'll simplify: rooms span from grid A to last grid line
    let n_x = rooms_per_side + 3; // +1 for start, +1 for elevator bay, +1 for end stair
    let mut x_labels = Vec::new();
    let mut x_vals = Vec::new();
    let mut x = 0.0;
    for i in 0..=n_x {
        let label = if st.numbers_on_x { format!("{}", i + 1) } else { ((b'A' + i as u8) as char).to_string() };
        x_labels.push(label);
        x_vals.push(x);
        if i == 0 {
            x += stair_width_ft; // west stair
        } else if i == rooms_per_side / 2 + 1 {
            x += elev_width_ft; // elevator bay
        } else if i == n_x {
            // last
        } else {
            x += room_width_ft;
        }
    }

    for (label, at) in x_labels.iter().zip(x_vals.iter()) {
        t.building.grid.x.push(GridLine { label: label.clone(), at_ft: *at });
    }

    // Y grid: 4 lines
    let y_labels: Vec<String> = if st.numbers_on_x {
        ["A", "B", "C", "D"].iter().map(|s| s.to_string()).collect()
    } else {
        ["1", "2", "3", "4"].iter().map(|s| s.to_string()).collect()
    };
    let y_vals = [0.0, room_depth_ft, room_depth_ft + corridor_width_ft, building_depth_ft];
    for (label, at) in y_labels.iter().zip(y_vals.iter()) {
        t.building.grid.y.push(GridLine { label: label.clone(), at_ft: *at });
    }

    // --- Levels ---
    let floor_to_floor_ft = 11.0; // typical hotel
    let ground_height_ft = 14.0; // lobby is taller
    t.building.levels = vec![
        Level { name: "LEVEL 1".into(), code: "1".into(), elevation_ft: 0.0 },
        Level { name: "LEVEL 2".into(), code: "2".into(), elevation_ft: ground_height_ft },
        Level { name: "LEVEL 3".into(), code: "3".into(), elevation_ft: ground_height_ft + floor_to_floor_ft },
        Level { name: "LEVEL 4".into(), code: "4".into(), elevation_ft: ground_height_ft + 2.0 * floor_to_floor_ft },
        Level { name: "ROOF".into(), code: "R".into(), elevation_ft: ground_height_ft + 3.0 * floor_to_floor_ft },
    ];
    t.building.slab_in = 6.0;

    let building_width_ft = *x_vals.last().unwrap_or(&100.0);
    let elev_bay_start = x_vals.get(rooms_per_side / 2 + 1).copied().unwrap_or(50.0);
    let elev_bay_end = elev_bay_start + elev_width_ft;

    // --- Stairs & Elevator ---
    let west_stair_max_x = x_vals.get(1).copied().unwrap_or(stair_width_ft);
    let east_stair_min_x = x_vals.get(n_x.saturating_sub(1)).copied().unwrap_or(building_width_ft - stair_width_ft);
    let all_levels: Vec<String> = ["1", "2", "3", "4", "R"].iter().map(|s| s.to_string()).collect();

    t.building.stairs.push(Stair {
        id: "stair-w".into(),
        tag: "STAIR A".into(),
        min_ft: [0.0, room_depth_ft],
        max_ft: [west_stair_max_x, room_depth_ft + corridor_width_ft],
        levels: all_levels.clone(),
    });
    t.building.stairs.push(Stair {
        id: "stair-e".into(),
        tag: "STAIR B".into(),
        min_ft: [east_stair_min_x, room_depth_ft],
        max_ft: [building_width_ft, room_depth_ft + corridor_width_ft],
        levels: all_levels.clone(),
    });

    t.building.elevators.push(Elevator {
        id: "elev-1".into(),
        tag: "ELEV 1".into(),
        min_ft: [elev_bay_start + 1.0, room_depth_ft + 0.5],
        max_ft: [elev_bay_start + 8.0, room_depth_ft + corridor_width_ft - 0.5],
        levels: all_levels.clone(),
    });

    // Shafts: plumbing chases at wet wall stacks
    let shaft_every = 2; // one plumbing shaft per 2 rooms
    let mut shaft_idx = 0;
    {
        let mut sx = west_stair_max_x;
        let mut room_i = 0;
        while sx < east_stair_min_x {
            if (sx - elev_bay_start).abs() < 1.0 {
                sx += elev_width_ft;
                continue;
            }
            if room_i % shaft_every == 1 {
                t.building.shafts.push(Shaft {
                    id: format!("shaft-p{shaft_idx}"),
                    tag: format!("PC-{}", shaft_idx + 1),
                    kind: "plumbing".into(),
                    min_ft: [sx - 0.5, room_depth_ft - 0.5],
                    max_ft: [sx + 0.5, room_depth_ft + 0.5],
                    levels: all_levels.clone(),
                });
                shaft_idx += 1;
            }
            sx += room_width_ft;
            room_i += 1;
        }
    }

    // --- Columns ---
    let mut col_id = 0;
    for xi in 0..t.building.grid.x.len() {
        for yi in 0..t.building.grid.y.len() {
            let gx = t.building.grid.x.get(xi).map(|g| g.label.clone()).unwrap_or_default();
            let gy = t.building.grid.y.get(yi).map(|g| g.label.clone()).unwrap_or_default();
            let cx = t.building.grid.x.get(xi).map(|g| g.at_ft).unwrap_or(0.0);
            let cy = t.building.grid.y.get(yi).map(|g| g.at_ft).unwrap_or(0.0);
            t.building.columns.push(Column {
                id: format!("col-{col_id}"),
                grid: format!("{gx}-{gy}"),
                at_ft: [cx, cy],
                size: "W10x49".into(),
                footprint_in: [10.0, 10.0],
                bottom_ft: 0.0,
                top_ft: ground_height_ft + 3.0 * floor_to_floor_ft,
            });
            col_id += 1;
        }
    }

    // --- Beams (on each level above ground) ---
    let mut beam_id = 0;
    for lv in ["2", "3", "4", "R"] {
        let elev = t.building.levels.iter().find(|l| l.code == lv).map(|l| l.elevation_ft).unwrap_or(0.0);
        // Beams along Y grid lines (east-west, between x grids)
        for yi in 0..t.building.grid.y.len() {
            let gy = t.building.grid.y.get(yi).map(|g| g.label.clone()).unwrap_or_default();
            let cy = t.building.grid.y.get(yi).map(|g| g.at_ft).unwrap_or(0.0);
            for xi in 0..t.building.grid.x.len().saturating_sub(1) {
                let x0 = t.building.grid.x.get(xi).map(|g| g.at_ft).unwrap_or(0.0);
                let x1 = t.building.grid.x.get(xi + 1).map(|g| g.at_ft).unwrap_or(0.0);
                let size = if (x1 - x0) > 11.0 { "W14x22" } else { "W12x19" };
                let depth = if size == "W14x22" { 13.74 } else { 12.16 };
                let tos = elev - t.building.slab_in / 12.0;
                t.building.beams.push(Beam {
                    id: format!("beam-{beam_id}"),
                    level: lv.into(),
                    grid: gy.clone(),
                    size: size.into(),
                    depth_in: depth,
                    flange_in: if size == "W14x22" { 5.0 } else { 4.005 },
                    from_ft: [x0, cy],
                    to_ft: [x1, cy],
                    tos_ft: tos,
                });
                beam_id += 1;
            }
        }
    }

    // --- Rooms ---
    // Ground floor: public spaces
    let ground_rooms = ground_floor_rooms(
        rooms_per_side,
        west_stair_max_x,
        east_stair_min_x,
        elev_bay_start,
        elev_bay_end,
        room_depth_ft,
        corridor_width_ft,
        building_depth_ft,
        building_width_ft,
    );
    for rm in &ground_rooms {
        t.building.rooms.push(rm.clone());
    }

    // Floors 2-4: guest rooms
    let room_types = guest_room_sequence(rooms_per_side, r);
    for lv in ["2", "3", "4"] {
        let elev = t.building.levels.iter().find(|l| l.code == lv).map(|l| l.elevation_ft).unwrap_or(0.0);
        let _ = elev; // used indirectly through level code
        // Corridor
        t.building.rooms.push(Room {
            id: format!("rm-{lv}-CORR"),
            level: lv.into(),
            number: format!("{lv}00"),
            name: "CORRIDOR".into(),
            min_ft: [0.0, room_depth_ft],
            max_ft: [building_width_ft, room_depth_ft + corridor_width_ft],
            ceiling: Some(Ceiling { height_ft: 8.5, kind: "GYP".into(), tag: "GYP @ 8'-6\"".into(), grid_ft: None, height_hidden: false }),
            plenum_ft: None,
        });

        // South side rooms
        let mut x = west_stair_max_x;
        let mut room_num = 1;
        let floor_num: usize = lv.parse().unwrap_or(2);
        for rt in &room_types {
            if x >= east_stair_min_x - 1.0 {
                break;
            }
            if (x - elev_bay_start).abs() < 1.0 {
                x = elev_bay_end;
            }
            let w = rt.width_ft();
            if x + w > east_stair_min_x + 0.1 {
                break;
            }
            let num = format!("{}{:02}", floor_num, room_num);
            let ceil_h = if rt.is_ada() { 9.0 } else { 8.5 };
            t.building.rooms.push(Room {
                id: format!("rm-{lv}-S{room_num}"),
                level: lv.into(),
                number: num,
                name: rt.name().into(),
                min_ft: [x, 0.0],
                max_ft: [x + w, room_depth_ft],
                ceiling: Some(Ceiling {
                    height_ft: ceil_h,
                    kind: if rt.is_suite() { "GYP".into() } else { "ACT".into() },
                    tag: format!("{} @ {}", if rt.is_suite() { "GYP" } else { "ACT" }, feet_inches(ceil_h)),
                    grid_ft: None,
                    height_hidden: false,
                }),
                plenum_ft: None,
            });
            x += w;
            room_num += 1;
        }
        // North side rooms (mirrored)
        x = west_stair_max_x;
        for rt in &room_types {
            if x >= east_stair_min_x - 1.0 {
                break;
            }
            if (x - elev_bay_start).abs() < 1.0 {
                x = elev_bay_end;
            }
            let w = rt.width_ft();
            if x + w > east_stair_min_x + 0.1 {
                break;
            }
            let num = format!("{}{:02}", floor_num, room_num);
            let ceil_h = if rt.is_ada() { 9.0 } else { 8.5 };
            t.building.rooms.push(Room {
                id: format!("rm-{lv}-N{}", room_num - rooms_per_side),
                level: lv.into(),
                number: num,
                name: rt.name().into(),
                min_ft: [x, room_depth_ft + corridor_width_ft],
                max_ft: [x + w, building_depth_ft],
                ceiling: Some(Ceiling {
                    height_ft: ceil_h,
                    kind: if rt.is_suite() { "GYP" } else { "ACT" }.into(),
                    tag: format!("{} @ {}", if rt.is_suite() { "GYP" } else { "ACT" }, feet_inches(ceil_h)),
                    grid_ft: None,
                    height_hidden: false,
                }),
                plenum_ft: None,
            });
            x += w;
            room_num += 1;
        }
    }

    // --- Unit types and their instances (Q305) ---
    // Before anything goes into the rooms: a mirrored instance gets its contents flipped and an
    // instance's overrides change what is placed in it.
    t.unit_types = unit_type_defs(room_depth_ft);
    t.unit_instances = unit_instances(&t);

    // --- Wall assemblies ---
    t.wall_assemblies = wall_assemblies();

    // --- Walls for every room boundary ---
    let mut wall_id = 0;
    for rm in &t.building.rooms {
        let [x0, y0] = rm.min_ft;
        let [x1, y1] = rm.max_ft;
        // Four walls per room (simplified)
        for (from, to) in [
            ([x0, y0], [x1, y0]), // south
            ([x1, y0], [x1, y1]), // east
            ([x1, y1], [x0, y1]), // north
            ([x0, y1], [x0, y0]), // west
        ] {
            let asm = wall_for_room(&rm.name, &rm.level);
            let h = rm.ceiling.as_ref().map(|c| c.height_ft + 2.5).unwrap_or(11.0);
            t.walls.push(WallInstance {
                id: format!("wall-{wall_id}"),
                assembly: asm.into(),
                level: rm.level.clone(),
                from_ft: from,
                to_ft: to,
                height_ft: h,
                sheet: None,
            });
            wall_id += 1;
        }
    }

    // --- Wall types (for floor plan drawing) ---
    t.wall_types = wall_type_defs();

    // --- Plan walls (for floor plan drawing): one PlanWall per WallInstance ---
    hotel_plan_walls(&mut t);

    // --- Ceiling assemblies ---
    t.ceiling_assemblies = ceiling_assemblies();

    // --- Soffits (bulkheads at bathroom vanities in guest rooms) ---
    let mut soffit_id = 0;
    for rm in &t.building.rooms {
        if rm.name.contains("KING") || rm.name.contains("QUEEN") || rm.name.contains("SUITE") {
            let vanity_depth = 2.5;
            let (x0, x1) = hand_span(rm, rm.min_ft[0] + 0.5, rm.min_ft[0] + 5.0);
            t.soffits.push(SoffitInstance {
                id: format!("soffit-{soffit_id}"),
                assembly: "CA-2".into(),
                level: rm.level.clone(),
                room: rm.id.clone(),
                min_ft: [x0, rm.min_ft[1] + 0.5],
                max_ft: [x1, rm.min_ft[1] + vanity_depth + 0.5],
                bottom_ft: 7.5,
                sheet: None,
            });
            soffit_id += 1;
        }
    }

    // --- Casework ---
    let mut cw_id = 0;
    for rm in &t.building.rooms {
        match rm.name.as_str() {
            "KING ROOM" | "DOUBLE QUEEN ROOM" | "ADA KING ROOM" => {
                // Vanity in bathroom area
                let (x0, x1) = hand_span(rm, rm.min_ft[0] + 0.5, rm.min_ft[0] + 4.0);
                t.casework.push(CaseworkItem {
                    id: format!("cw-{cw_id}"),
                    kind: "vanity".into(),
                    tag: "V-1".into(),
                    level: rm.level.clone(),
                    room: rm.id.clone(),
                    min_ft: [x0, rm.min_ft[1] + 0.5],
                    max_ft: [x1, rm.min_ft[1] + 2.0],
                    height_in: 34.0,
                    depth_in: 22.0,
                    spec_section: Some("06 41 16".into()),
                });
                cw_id += 1;
                // Wardrobe (left out where a column's furring takes its place)
                if unit_override(&t, &rm.id, "casework.wardrobe") != Some("0") {
                    let (x0, x1) = hand_span(rm, rm.max_ft[0] - 4.0, rm.max_ft[0] - 0.5);
                    t.casework.push(CaseworkItem {
                        id: format!("cw-{cw_id}"),
                        kind: "wardrobe".into(),
                        tag: "WR-1".into(),
                        level: rm.level.clone(),
                        room: rm.id.clone(),
                        min_ft: [x0, rm.min_ft[1] + 0.5],
                        max_ft: [x1, rm.min_ft[1] + 2.5],
                        height_in: 84.0,
                        depth_in: 24.0,
                        spec_section: Some("06 41 16".into()),
                    });
                    cw_id += 1;
                }
            }
            "SUITE" => {
                let (x0, x1) = hand_span(rm, rm.min_ft[0] + 0.5, rm.min_ft[0] + 5.0);
                t.casework.push(CaseworkItem {
                    id: format!("cw-{cw_id}"),
                    kind: "vanity".into(),
                    tag: "V-2".into(),
                    level: rm.level.clone(),
                    room: rm.id.clone(),
                    min_ft: [x0, rm.min_ft[1] + 0.5],
                    max_ft: [x1, rm.min_ft[1] + 2.5],
                    height_in: 34.0,
                    depth_in: 22.0,
                    spec_section: Some("06 41 16".into()),
                });
                cw_id += 1;
                // Kitchenette counter
                let (x0, x1) = hand_span(rm, rm.min_ft[0] + 8.0, rm.min_ft[0] + 14.0);
                t.casework.push(CaseworkItem {
                    id: format!("cw-{cw_id}"),
                    kind: "counter".into(),
                    tag: "KC-1".into(),
                    level: rm.level.clone(),
                    room: rm.id.clone(),
                    min_ft: [x0, rm.min_ft[1] + 0.5],
                    max_ft: [x1, rm.min_ft[1] + 2.5],
                    height_in: 36.0,
                    depth_in: 25.0,
                    spec_section: Some("06 41 16".into()),
                });
                cw_id += 1;
            }
            "KITCHEN" | "BREAKFAST AREA" => {
                t.casework.push(CaseworkItem {
                    id: format!("cw-{cw_id}"),
                    kind: "base_cabinet".into(),
                    tag: "BC-1".into(),
                    level: rm.level.clone(),
                    room: rm.id.clone(),
                    min_ft: [rm.min_ft[0] + 1.0, rm.min_ft[1] + 1.0],
                    max_ft: [rm.max_ft[0] - 1.0, rm.min_ft[1] + 3.0],
                    height_in: 34.5,
                    depth_in: 24.0,
                    spec_section: Some("11 40 00".into()),
                });
                cw_id += 1;
                t.casework.push(CaseworkItem {
                    id: format!("cw-{cw_id}"),
                    kind: "upper_cabinet".into(),
                    tag: "UC-1".into(),
                    level: rm.level.clone(),
                    room: rm.id.clone(),
                    min_ft: [rm.min_ft[0] + 1.0, rm.min_ft[1] + 1.0],
                    max_ft: [rm.max_ft[0] - 1.0, rm.min_ft[1] + 3.0],
                    height_in: 30.0,
                    depth_in: 12.0,
                    spec_section: Some("11 40 00".into()),
                });
                cw_id += 1;
            }
            "FRONT DESK" => {
                t.casework.push(CaseworkItem {
                    id: format!("cw-{cw_id}"),
                    kind: "counter".into(),
                    tag: "FD-1".into(),
                    level: rm.level.clone(),
                    room: rm.id.clone(),
                    min_ft: [rm.min_ft[0] + 2.0, rm.max_ft[1] - 4.0],
                    max_ft: [rm.max_ft[0] - 2.0, rm.max_ft[1] - 2.0],
                    height_in: 42.0,
                    depth_in: 30.0,
                    spec_section: Some("06 41 16".into()),
                });
                cw_id += 1;
            }
            _ => {}
        }
    }

    // --- MEP systems (reuse the graph approach from the office, scaled up) ---
    let systems = mep_systems(st);
    t.systems = systems;

    // Generate MEP for each floor
    let mut planted = Vec::new();
    let mut seg_id = 0;
    let mut node_id = 0;
    let mut term_id = 0;
    let mut equip_id = 0;
    let mut fix_id = 0;
    let mut riser_id = 0;

    // Equipment: RTUs on roof, AHUs in mechanical room, boiler/water heater on L1
    let mech_room = t.building.rooms.iter().find(|r| r.name == "MECHANICAL" && r.level == "1");
    let mech_min = mech_room.map(|r| r.min_ft).unwrap_or([0.0, 0.0]);
    let _mech_max = mech_room.map(|r| r.max_ft).unwrap_or([20.0, 20.0]);

    // AHU on level 1
    t.equipment.push(Equipment {
        id: format!("equip-{equip_id}"),
        tag: "AHU-1".into(),
        kind: "ahu".into(),
        level: "1".into(),
        min_ft: [mech_min[0] + 1.0, mech_min[1] + 1.0, 0.0],
        max_ft: [mech_min[0] + 8.0, mech_min[1] + 4.0, 5.0],
        cfm: Some(12000.0),
        schedule: [("CFM".into(), "12000".into()), ("MOTOR".into(), "10 HP".into()), ("VOLTS".into(), "460/3/60".into())].into(),
        nodes: vec![],
    });
    equip_id += 1;

    // RTUs on roof
    for i in 0..2 {
        let rx = building_width_ft * 0.25 + i as f64 * building_width_ft * 0.5;
        let ry = building_depth_ft / 2.0;
        let roof_elev = t.building.levels.iter().find(|l| l.code == "R").map(|l| l.elevation_ft).unwrap_or(47.0);
        t.equipment.push(Equipment {
            id: format!("equip-{equip_id}"),
            tag: format!("RTU-{}", i + 1),
            kind: "rtu".into(),
            level: "R".into(),
            min_ft: [rx - 4.0, ry - 3.0, roof_elev],
            max_ft: [rx + 4.0, ry + 3.0, roof_elev + 5.0],
            cfm: Some(8000.0),
            schedule: [("CFM".into(), "8000".into()), ("TONS".into(), "20".into()), ("VOLTS".into(), "460/3/60".into())].into(),
            nodes: vec![],
        });
        equip_id += 1;
    }

    // Water heater
    t.equipment.push(Equipment {
        id: format!("equip-{equip_id}"),
        tag: "WH-1".into(),
        kind: "water_heater".into(),
        level: "1".into(),
        min_ft: [mech_min[0] + 10.0, mech_min[1] + 1.0, 0.0],
        max_ft: [mech_min[0] + 13.0, mech_min[1] + 3.0, 5.0],
        cfm: None,
        schedule: [("INPUT".into(), "199 MBH".into()), ("TYPE".into(), "GAS".into())].into(),
        nodes: vec![],
    });
    equip_id += 1;

    // Boiler
    t.equipment.push(Equipment {
        id: format!("equip-{equip_id}"),
        tag: "B-1".into(),
        kind: "boiler".into(),
        level: "1".into(),
        min_ft: [mech_min[0] + 10.0, mech_min[1] + 4.0, 0.0],
        max_ft: [mech_min[0] + 14.0, mech_min[1] + 7.0, 4.0],
        cfm: None,
        schedule: [("INPUT".into(), "500 MBH".into()), ("TYPE".into(), "GAS".into())].into(),
        nodes: vec![],
    });
    equip_id += 1;

    // Generate ductwork on each level
    for lv in ["1", "2", "3", "4"] {
        let elev = t.building.levels.iter().find(|l| l.code == lv).map(|l| l.elevation_ft).unwrap_or(0.0);
        let struct_depth_in = t.building.slab_in + 14.0; // slab + beam
        let duct_band_ft = elev + (if lv == "1" { ground_height_ft } else { floor_to_floor_ft }) - struct_depth_in / 12.0 - 0.5;

        // Supply main along the corridor
        let main_size = [24.0, 14.0_f64];
        let main_y = room_depth_ft + corridor_width_ft / 2.0;
        let main_z = duct_band_ft;

        // Supply main: west to east through the corridor
        let main_start_x = west_stair_max_x;
        let main_end_x = east_stair_min_x;

        let from_node = format!("n-{node_id}");
        node_id += 1;
        let to_node = format!("n-{node_id}");
        node_id += 1;

        let seg = format!("seg-{seg_id}");
        seg_id += 1;

        t.nodes.push(Node {
            id: from_node.clone(),
            kind: "open_end".into(),
            system: "SA".into(),
            level: lv.into(),
            at_ft: [main_start_x, main_y, main_z],
            segments: vec![seg.clone()],
            connects: None,
        });
        t.nodes.push(Node {
            id: to_node.clone(),
            kind: "open_end".into(),
            system: "SA".into(),
            level: lv.into(),
            at_ft: [main_end_x, main_y, main_z],
            segments: vec![seg.clone()],
            connects: None,
        });
        t.segments.push(Segment {
            id: seg,
            system: "SA".into(),
            kind: "duct".into(),
            shape: "rect".into(),
            size_in: main_size,
            from: from_node,
            to: to_node,
            level: lv.into(),
            path_ft: [[main_start_x, main_y, main_z], [main_end_x, main_y, main_z]],
            vertical: false,
            height: Some(Height {
                bottom_ft: main_z - main_size[1] / 24.0,
                top_ft: main_z + main_size[1] / 24.0,
                centre_ft: main_z,
                note: None,
                source: HeightSource::Rule,
            }),
            tag: None,
            slope: None,
            below_floor: false,
            sheet: String::new(),
        });

        // Supply diffusers in each room
        let level_rooms: Vec<&Room> = t.building.rooms.iter().filter(|r| r.level == lv && r.name != "CORRIDOR").collect();
        for rm in &level_rooms {
            let cx = (rm.min_ft[0] + rm.max_ft[0]) / 2.0;
            let cy = (rm.min_ft[1] + rm.max_ft[1]) / 2.0;
            let ceil_h = rm.ceiling.as_ref().map(|c| c.height_ft).unwrap_or(9.0);
            t.terminals.push(Terminal {
                id: format!("term-{term_id}"),
                kind: "supply_diffuser".into(),
                type_tag: "CD-1".into(),
                system: "SA".into(),
                level: lv.into(),
                room: rm.id.clone(),
                at_ft: [cx, cy, elev + ceil_h],
                face_in: [24.0, 24.0],
                neck_in: 10.0,
                cfm: Some(200.0),
                node: None,
                mount: None,
            });
            term_id += 1;
        }

        // Return grilles
        for rm in &level_rooms {
            let cx = hand_x(rm, (rm.min_ft[0] + rm.max_ft[0]) / 2.0 + 3.0);
            let cy = (rm.min_ft[1] + rm.max_ft[1]) / 2.0;
            let ceil_h = rm.ceiling.as_ref().map(|c| c.height_ft).unwrap_or(9.0);
            t.terminals.push(Terminal {
                id: format!("term-{term_id}"),
                kind: "return_grille".into(),
                type_tag: "RG-1".into(),
                system: "RA".into(),
                level: lv.into(),
                room: rm.id.clone(),
                at_ft: [cx, cy, elev + ceil_h],
                face_in: [24.0, 12.0],
                neck_in: 10.0,
                cfm: Some(150.0),
                node: None,
                mount: None,
            });
            term_id += 1;
        }

        // Sprinkler main and heads
        let fp_z = main_z - main_size[1] / 12.0 - 0.5; // below supply duct
        let fp_from = format!("n-{node_id}");
        node_id += 1;
        let fp_to = format!("n-{node_id}");
        node_id += 1;
        let fp_seg = format!("seg-{seg_id}");
        seg_id += 1;
        t.nodes.push(Node {
            id: fp_from.clone(),
            kind: "open_end".into(),
            system: "FP".into(),
            level: lv.into(),
            at_ft: [main_start_x, main_y - 1.5, fp_z],
            segments: vec![fp_seg.clone()],
            connects: None,
        });
        t.nodes.push(Node {
            id: fp_to.clone(),
            kind: "open_end".into(),
            system: "FP".into(),
            level: lv.into(),
            at_ft: [main_end_x, main_y - 1.5, fp_z],
            segments: vec![fp_seg.clone()],
            connects: None,
        });
        t.segments.push(Segment {
            id: fp_seg,
            system: "FP".into(),
            kind: "pipe".into(),
            shape: "pipe".into(),
            size_in: [2.0, 2.0],
            from: fp_from,
            to: fp_to,
            level: lv.into(),
            path_ft: [[main_start_x, main_y - 1.5, fp_z], [main_end_x, main_y - 1.5, fp_z]],
            vertical: false,
            height: Some(Height { bottom_ft: fp_z - 1.0 / 12.0, top_ft: fp_z + 1.0 / 12.0, centre_ft: fp_z, note: None, source: HeightSource::Rule }),
            tag: None,
            slope: None,
            below_floor: false,
            sheet: String::new(),
        });

        // Sprinkler heads in corridor
        let head_spacing = 12.0;
        let mut hx = main_start_x + 6.0;
        while hx < main_end_x - 3.0 {
            let ceil_h = 8.5; // corridor ceiling
            t.terminals.push(Terminal {
                id: format!("term-{term_id}"),
                kind: "sprinkler_head".into(),
                type_tag: "SH-1".into(),
                system: "FP".into(),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [hx, main_y, elev + ceil_h - 0.25],
                face_in: [0.0, 0.0],
                neck_in: 0.5,
                cfm: None,
                node: None,
                mount: Some("pendent".into()),
            });
            term_id += 1;
            hx += head_spacing;
        }

        // Plumbing fixtures in guest rooms (water closets + lavatories).
        // North-side rooms ("N" in the id) are mirrored: wet wall and fixtures at the
        // opposite (high-x) side of the room.
        for rm in &level_rooms {
            if rm.name.contains("KING") || rm.name.contains("QUEEN") || rm.name.contains("SUITE") || rm.name.contains("ADA") {
                // WC near wet wall
                t.fixtures.push(Fixture {
                    id: format!("fix-{fix_id}"),
                    tag: "WC".into(),
                    kind: "water_closet".into(),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [hand_x(rm, rm.min_ft[0] + 2.5), rm.min_ft[1] + 1.0, elev],
                    nodes: vec![],
                    wall: None,
                });
                fix_id += 1;
                // Lavatory
                t.fixtures.push(Fixture {
                    id: format!("fix-{fix_id}"),
                    tag: "LAV".into(),
                    kind: "lavatory".into(),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [hand_x(rm, rm.min_ft[0] + 1.5), rm.min_ft[1] + 1.0, elev + 2.83],
                    nodes: vec![],
                    wall: None,
                });
                fix_id += 1;
                // Tub (a roll-in shower where the unit is made accessible)
                let shower = unit_override(&t, &rm.id, "fixtures.shower") == Some("1");
                t.fixtures.push(Fixture {
                    id: format!("fix-{fix_id}"),
                    tag: if shower { "SH" } else { "TUB" }.into(),
                    kind: if shower { "shower" } else { "bathtub" }.into(),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [hand_x(rm, rm.min_ft[0] + 4.5), rm.min_ft[1] + 1.0, elev],
                    nodes: vec![],
                    wall: None,
                });
                fix_id += 1;
            }
        }

        // CW/HW mains along corridor below duct
        let pipe_z = fp_z - 0.5;
        for (sys, off) in [("CW", 0.0), ("HW", 0.3), ("HWR", 0.6)] {
            let ps = format!("seg-{seg_id}");
            seg_id += 1;
            let pf = format!("n-{node_id}");
            node_id += 1;
            let pt = format!("n-{node_id}");
            node_id += 1;
            let pipe_y = main_y + 1.5 + off;
            t.nodes.push(Node {
                id: pf.clone(),
                kind: "open_end".into(),
                system: sys.into(),
                level: lv.into(),
                at_ft: [main_start_x, pipe_y, pipe_z],
                segments: vec![ps.clone()],
                connects: None,
            });
            t.nodes.push(Node {
                id: pt.clone(),
                kind: "open_end".into(),
                system: sys.into(),
                level: lv.into(),
                at_ft: [main_end_x, pipe_y, pipe_z],
                segments: vec![ps.clone()],
                connects: None,
            });
            let nominal = match sys {
                "CW" => 2.0,
                "HW" => 1.5,
                _ => 1.0,
            };
            t.segments.push(Segment {
                id: ps,
                system: sys.into(),
                kind: "pipe".into(),
                shape: "pipe".into(),
                size_in: [nominal, nominal],
                from: pf,
                to: pt,
                level: lv.into(),
                path_ft: [[main_start_x, pipe_y, pipe_z], [main_end_x, pipe_y, pipe_z]],
                vertical: false,
                height: Some(Height {
                    bottom_ft: pipe_z - design::pipe_od(nominal) / 24.0,
                    top_ft: pipe_z + design::pipe_od(nominal) / 24.0,
                    centre_ft: pipe_z,
                    note: None,
                    source: HeightSource::Rule,
                }),
                tag: None,
                slope: None,
                below_floor: false,
                sheet: String::new(),
            });
        }

        // Sanitary drain (below floor)
        let san_seg = format!("seg-{seg_id}");
        seg_id += 1;
        let san_f = format!("n-{node_id}");
        node_id += 1;
        let san_t = format!("n-{node_id}");
        node_id += 1;
        let san_z = elev - 0.5;
        t.nodes.push(Node {
            id: san_f.clone(),
            kind: "open_end".into(),
            system: "SAN".into(),
            level: lv.into(),
            at_ft: [main_start_x, main_y + 2.5, san_z],
            segments: vec![san_seg.clone()],
            connects: None,
        });
        t.nodes.push(Node {
            id: san_t.clone(),
            kind: "open_end".into(),
            system: "SAN".into(),
            level: lv.into(),
            at_ft: [main_end_x, main_y + 2.5, san_z - (main_end_x - main_start_x) / 48.0],
            segments: vec![san_seg.clone()],
            connects: None,
        });
        t.segments.push(Segment {
            id: san_seg,
            system: "SAN".into(),
            kind: "pipe".into(),
            shape: "pipe".into(),
            size_in: [4.0, 4.0],
            from: san_f,
            to: san_t,
            level: lv.into(),
            path_ft: [[main_start_x, main_y + 2.5, san_z], [main_end_x, main_y + 2.5, san_z - (main_end_x - main_start_x) / 48.0]],
            vertical: false,
            height: Some(Height {
                bottom_ft: san_z - 2.0 / 12.0,
                top_ft: san_z + 2.0 / 12.0,
                centre_ft: san_z,
                note: None,
                source: HeightSource::Rule,
            }),
            tag: None,
            slope: Some("1/4\"/FT".into()),
            below_floor: true,
            sheet: String::new(),
        });

        // Plant a clash: on level 2, push one supply branch through the FP main
        if lv == "2" {
            let clash_x = main_start_x + 20.0;
            let clash_seg = format!("seg-{seg_id}");
            seg_id += 1;
            let cf = format!("n-{node_id}");
            node_id += 1;
            let ct = format!("n-{node_id}");
            node_id += 1;
            t.nodes.push(Node {
                id: cf.clone(),
                kind: "tee".into(),
                system: "SA".into(),
                level: "2".into(),
                at_ft: [clash_x, main_y, main_z],
                segments: vec![clash_seg.clone()],
                connects: None,
            });
            t.nodes.push(Node {
                id: ct.clone(),
                kind: "open_end".into(),
                system: "SA".into(),
                level: "2".into(),
                at_ft: [clash_x, main_y - 2.5, fp_z],
                segments: vec![clash_seg.clone()],
                connects: None,
            });
            t.segments.push(Segment {
                id: clash_seg.clone(),
                system: "SA".into(),
                kind: "duct".into(),
                shape: "round".into(),
                size_in: [10.0, 10.0],
                from: cf,
                to: ct,
                level: "2".into(),
                path_ft: [[clash_x, main_y, fp_z], [clash_x, main_y - 2.5, fp_z]],
                vertical: false,
                height: Some(Height {
                    bottom_ft: fp_z - 5.0 / 12.0,
                    top_ft: fp_z + 5.0 / 12.0,
                    centre_ft: fp_z,
                    note: None,
                    source: HeightSource::Rule,
                }),
                tag: None,
                slope: None,
                below_floor: false,
                sheet: String::new(),
            });
            planted.push(clash_seg);
        }
    }

    // --- Risers ---
    // CW/HW/SAN risers
    for (sys, x_off) in [("CW", 0.0), ("HW", 0.3), ("SAN", 0.6)] {
        let rx = west_stair_max_x + room_width_ft + 0.5;
        let ry = room_depth_ft + 0.2 + x_off;
        let nominal = match sys {
            "CW" => 2.0,
            "SAN" => 4.0,
            _ => 1.5,
        };
        t.risers.push(Riser {
            id: format!("riser-{riser_id}"),
            system: sys.into(),
            at_ft: [rx, ry],
            size_in: [nominal, nominal],
            bottom_ft: 0.0,
            top_ft: ground_height_ft + 3.0 * floor_to_floor_ft,
            segments: vec![],
            shown: ["1", "2", "3", "4"]
                .iter()
                .map(|l| RiserMark {
                    level: l.to_string(),
                    symbol: if *l == "1" { "up" } else { "through" }.into(),
                    note: format!("{}\" {}", pipe_size(nominal), st.sys(sys)),
                })
                .collect(),
        });
        riser_id += 1;
    }

    // FP riser
    t.risers.push(Riser {
        id: format!("riser-{riser_id}"),
        system: "FP".into(),
        at_ft: [west_stair_max_x + 1.0, room_depth_ft + 1.0],
        size_in: [4.0, 4.0],
        bottom_ft: 0.0,
        top_ft: ground_height_ft + 3.0 * floor_to_floor_ft,
        segments: vec![],
        shown: ["1", "2", "3", "4"]
            .iter()
            .map(|l| RiserMark {
                level: l.to_string(),
                symbol: if *l == "1" { "up" } else { "through" }.into(),
                note: format!("4\" {}", st.sys("FP")),
            })
            .collect(),
    });
    riser_id += 1;

    // --- Electrical (reuse Q287 approach, extended for the hotel) ---
    hotel_electrical(
        &mut t,
        st,
        rooms_per_side,
        building_width_ft,
        building_depth_ft,
        room_depth_ft,
        corridor_width_ft,
        west_stair_max_x,
        east_stair_min_x,
        elev_bay_start,
        elev_bay_end,
        r,
    );

    // --- Fire alarm ---
    hotel_fire_alarm(&mut t, building_width_ft, building_depth_ft, room_depth_ft, corridor_width_ft);

    // --- Telecom ---
    hotel_telecom(&mut t, r);

    // --- Equipment connections ---
    hotel_equip_connections(&mut t);

    // --- Sleeves and cable pathways ---
    hotel_sleeves_and_pathways(&mut t);

    // --- Site elements ---
    hotel_site(&mut t, building_width_ft, building_depth_ft, r);
    crate::sitework::design_sitework(&mut t, building_width_ft, building_depth_ft, &mut r.fork(20));

    // --- Spec book ---
    t.spec_items = spec_book();

    // --- LV BOM (after spec book so spec IDs are available) ---
    hotel_lv_bom(&mut t);

    // --- Elevations and sections ---
    t.elevations = vec![
        ElevationView { id: "elev-north".into(), name: "NORTH ELEVATION".into(), direction: "north".into(), sheet: String::new(), room: None },
        ElevationView { id: "elev-south".into(), name: "SOUTH ELEVATION".into(), direction: "south".into(), sheet: String::new(), room: None },
        ElevationView { id: "elev-east".into(), name: "EAST ELEVATION".into(), direction: "east".into(), sheet: String::new(), room: None },
        ElevationView { id: "elev-west".into(), name: "WEST ELEVATION".into(), direction: "west".into(), sheet: String::new(), room: None },
    ];
    // Interior elevations for a typical guest room
    t.elevations.push(ElevationView {
        id: "elev-int-bath".into(),
        name: "TYPICAL BATH INTERIOR ELEVATIONS".into(),
        direction: "interior".into(),
        sheet: "A-511".into(),
        room: Some("rm-2-S1".into()),
    });

    t.sections = vec![
        SectionView {
            id: "sec-ew".into(),
            name: "BUILDING SECTION A".into(),
            cut_from_ft: [building_width_ft / 3.0, 0.0],
            cut_to_ft: [building_width_ft / 3.0, building_depth_ft],
            looking: "west".into(),
            sheet: "A-521".into(),
        },
        SectionView {
            id: "sec-ns".into(),
            name: "BUILDING SECTION B".into(),
            cut_from_ft: [0.0, building_depth_ft / 2.0],
            cut_to_ft: [building_width_ft, building_depth_ft / 2.0],
            looking: "north".into(),
            sheet: "A-522".into(),
        },
    ];

    // --- Gaps for Q296 ---
    t.gaps = plant_gaps(&mut t, r);

    let _ = (seg_id, node_id, term_id, equip_id, fix_id, riser_id); // suppress unused

    HotelDesign { truth: t, planted }
}

// --- Room types ---

#[derive(Clone, Copy, Debug)]
enum RoomType {
    King,
    DoubleQueen,
    AdaKing,
    Suite,
}

impl RoomType {
    fn width_ft(self) -> f64 {
        match self {
            Self::King => 13.0,
            Self::DoubleQueen => 14.0,
            Self::AdaKing => 15.0,
            Self::Suite => 26.0,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::King => "KING ROOM",
            Self::DoubleQueen => "DOUBLE QUEEN ROOM",
            Self::AdaKing => "ADA KING ROOM",
            Self::Suite => "SUITE",
        }
    }
    fn is_ada(self) -> bool {
        matches!(self, Self::AdaKing)
    }
    fn is_suite(self) -> bool {
        matches!(self, Self::Suite)
    }
}

fn guest_room_sequence(count: usize, r: &mut Rng) -> Vec<RoomType> {
    let mut types = Vec::with_capacity(count);
    for i in 0..count {
        let rt = if i == 0 {
            RoomType::AdaKing // first room is always ADA
        } else if i == 1 && count >= 3 {
            RoomType::Suite // suite goes second so it fits before running out of space
        } else if r.chance(0.4) {
            RoomType::DoubleQueen
        } else {
            RoomType::King
        };
        types.push(rt);
    }
    types
}

// --- Ground floor rooms ---

fn ground_floor_rooms(
    _rooms_per_side: usize,
    stair_w: f64,
    stair_e: f64,
    elev_start: f64,
    elev_end: f64,
    room_depth: f64,
    corr_width: f64,
    bldg_depth: f64,
    bldg_width: f64,
) -> Vec<Room> {
    let mut rooms = Vec::new();
    let south_y = 0.0;
    let corr_y0 = room_depth;
    let corr_y1 = room_depth + corr_width;
    let north_y = bldg_depth;

    // Corridor on level 1
    rooms.push(Room {
        id: "rm-1-CORR".into(),
        level: "1".into(),
        number: "100".into(),
        name: "CORRIDOR".into(),
        min_ft: [0.0, corr_y0],
        max_ft: [bldg_width, corr_y1],
        ceiling: Some(Ceiling { height_ft: 10.0, kind: "GYP".into(), tag: "GYP @ 10'-0\"".into(), grid_ft: None, height_hidden: false }),
        plenum_ft: None,
    });

    // South side: Lobby, Front desk, Breakfast, Kitchen
    let mut x = stair_w;
    let lobby_w = (stair_e - stair_w - elev_end + elev_start) * 0.35;
    let lobby_w = lobby_w.max(20.0);
    rooms.push(Room {
        id: "rm-1-LOBBY".into(),
        level: "1".into(),
        number: "101".into(),
        name: "LOBBY".into(),
        min_ft: [x, south_y],
        max_ft: [x + lobby_w, corr_y0],
        ceiling: Some(Ceiling { height_ft: 12.0, kind: "GYP".into(), tag: "GYP @ 12'-0\"".into(), grid_ft: None, height_hidden: false }),
        plenum_ft: None,
    });
    x += lobby_w;

    let fd_w = 15.0;
    rooms.push(Room {
        id: "rm-1-FD".into(),
        level: "1".into(),
        number: "102".into(),
        name: "FRONT DESK".into(),
        min_ft: [x, south_y],
        max_ft: [x + fd_w, corr_y0],
        ceiling: Some(Ceiling { height_ft: 10.0, kind: "GYP".into(), tag: "GYP @ 10'-0\"".into(), grid_ft: None, height_hidden: false }),
        plenum_ft: None,
    });
    x += fd_w;

    // Skip elevator bay
    if x >= elev_start - 0.5 && x < elev_end + 0.5 {
        x = elev_end;
    }

    let bk_w = 20.0;
    rooms.push(Room {
        id: "rm-1-BRKFST".into(),
        level: "1".into(),
        number: "103".into(),
        name: "BREAKFAST AREA".into(),
        min_ft: [x, south_y],
        max_ft: [(x + bk_w).min(stair_e), corr_y0],
        ceiling: Some(Ceiling { height_ft: 10.0, kind: "ACT".into(), tag: "ACT @ 10'-0\"".into(), grid_ft: None, height_hidden: false }),
        plenum_ft: None,
    });
    x = (x + bk_w).min(stair_e);

    if x < stair_e - 5.0 {
        rooms.push(Room {
            id: "rm-1-KITCHEN".into(),
            level: "1".into(),
            number: "104".into(),
            name: "KITCHEN".into(),
            min_ft: [x, south_y],
            max_ft: [stair_e, corr_y0],
            ceiling: Some(Ceiling { height_ft: 10.0, kind: "ACT".into(), tag: "ACT @ 10'-0\"".into(), grid_ft: None, height_hidden: false }),
            plenum_ft: None,
        });
    }

    // North side: Fitness, Laundry, Pool, Pool Equip, Mech, Elec, IDF
    x = stair_w;
    let fitness_w = 20.0;
    rooms.push(Room {
        id: "rm-1-FIT".into(),
        level: "1".into(),
        number: "110".into(),
        name: "FITNESS CENTER".into(),
        min_ft: [x, corr_y1],
        max_ft: [x + fitness_w, north_y],
        ceiling: Some(Ceiling { height_ft: 10.0, kind: "GYP".into(), tag: "GYP @ 10'-0\"".into(), grid_ft: None, height_hidden: false }),
        plenum_ft: None,
    });
    x += fitness_w;

    let laundry_w = 15.0;
    rooms.push(Room {
        id: "rm-1-LAUNDRY".into(),
        level: "1".into(),
        number: "111".into(),
        name: "LAUNDRY".into(),
        min_ft: [x, corr_y1],
        max_ft: [x + laundry_w, north_y],
        ceiling: Some(Ceiling { height_ft: 10.0, kind: "GYP".into(), tag: "GYP @ 10'-0\"".into(), grid_ft: None, height_hidden: false }),
        plenum_ft: None,
    });
    x += laundry_w;

    // Skip elevator bay
    if x >= elev_start - 0.5 && x < elev_end + 0.5 {
        x = elev_end;
    }

    let pool_w = 30.0;
    rooms.push(Room {
        id: "rm-1-POOL".into(),
        level: "1".into(),
        number: "112".into(),
        name: "POOL".into(),
        min_ft: [x, corr_y1],
        max_ft: [(x + pool_w).min(stair_e - 30.0), north_y],
        ceiling: Some(Ceiling { height_ft: 12.0, kind: "GYP".into(), tag: "GYP @ 12'-0\"".into(), grid_ft: None, height_hidden: false }),
        plenum_ft: None,
    });
    x = (x + pool_w).min(stair_e - 30.0);

    let pe_w = 10.0;
    rooms.push(Room {
        id: "rm-1-PE".into(),
        level: "1".into(),
        number: "113".into(),
        name: "POOL EQUIPMENT".into(),
        min_ft: [x, corr_y1],
        max_ft: [x + pe_w, north_y],
        ceiling: None, // open to structure
        plenum_ft: None,
    });
    x += pe_w;

    let mech_w = (stair_e - x) * 0.5;
    let mech_w = mech_w.max(12.0);
    rooms.push(Room {
        id: "rm-1-MECH".into(),
        level: "1".into(),
        number: "114".into(),
        name: "MECHANICAL".into(),
        min_ft: [x, corr_y1],
        max_ft: [x + mech_w, north_y],
        ceiling: None,
        plenum_ft: None,
    });
    x += mech_w;

    let elec_w = (stair_e - x) * 0.5;
    let elec_w = elec_w.max(8.0);
    rooms.push(Room {
        id: "rm-1-ELEC".into(),
        level: "1".into(),
        number: "115".into(),
        name: "ELECTRICAL".into(),
        min_ft: [x, corr_y1],
        max_ft: [x + elec_w, north_y],
        ceiling: None,
        plenum_ft: None,
    });
    x += elec_w;

    if stair_e - x > 5.0 {
        rooms.push(Room {
            id: "rm-1-IDF".into(),
            level: "1".into(),
            number: "116".into(),
            name: "IDF".into(),
            min_ft: [x, corr_y1],
            max_ft: [stair_e, north_y],
            ceiling: None,
            plenum_ft: None,
        });
    }

    rooms
}

// --- Wall assemblies ---

fn wall_assemblies() -> Vec<WallAssembly> {
    vec![
        WallAssembly {
            id: "wa-1".into(),
            type_tag: "WT-1".into(),
            name: "GUEST ROOM DEMISING".into(),
            layers: vec![
                WallLayer { material: "5/8\" GYP BD".into(), thickness_in: 0.625 },
                WallLayer { material: "3-5/8\" MTL STUD".into(), thickness_in: 3.625 },
                WallLayer { material: "BATT INSULATION".into(), thickness_in: 3.625 },
                WallLayer { material: "5/8\" GYP BD".into(), thickness_in: 0.625 },
            ],
            thickness_in: 4.875,
            fire_rating: "1-HR".into(),
            height_rule: "SLAB TO SLAB".into(),
            stc: Some(50),
        },
        WallAssembly {
            id: "wa-2".into(),
            type_tag: "WT-2".into(),
            name: "CORRIDOR WALL".into(),
            layers: vec![
                WallLayer { material: "5/8\" TYPE X GYP BD (2 LAYERS)".into(), thickness_in: 1.25 },
                WallLayer { material: "3-5/8\" MTL STUD".into(), thickness_in: 3.625 },
                WallLayer { material: "BATT INSULATION".into(), thickness_in: 3.625 },
                WallLayer { material: "5/8\" TYPE X GYP BD (2 LAYERS)".into(), thickness_in: 1.25 },
            ],
            thickness_in: 6.125,
            fire_rating: "1-HR".into(),
            height_rule: "SLAB TO SLAB".into(),
            stc: Some(55),
        },
        WallAssembly {
            id: "wa-3".into(),
            type_tag: "WT-3".into(),
            name: "WET WALL (STACKED)".into(),
            layers: vec![
                WallLayer { material: "5/8\" GYP BD".into(), thickness_in: 0.625 },
                WallLayer { material: "6\" MTL STUD".into(), thickness_in: 6.0 },
                WallLayer { material: "5/8\" GYP BD".into(), thickness_in: 0.625 },
            ],
            thickness_in: 7.25,
            fire_rating: "".into(),
            height_rule: "SLAB TO SLAB".into(),
            stc: Some(45),
        },
        WallAssembly {
            id: "wa-4".into(),
            type_tag: "WT-4".into(),
            name: "SHAFT WALL".into(),
            layers: vec![
                WallLayer { material: "5/8\" TYPE X GYP BD (2 LAYERS)".into(), thickness_in: 1.25 },
                WallLayer { material: "2-1/2\" C-H STUD".into(), thickness_in: 2.5 },
                WallLayer { material: "1\" GYP LINER".into(), thickness_in: 1.0 },
            ],
            thickness_in: 4.75,
            fire_rating: "2-HR".into(),
            height_rule: "SLAB TO SLAB".into(),
            stc: Some(50),
        },
        WallAssembly {
            id: "wa-5".into(),
            type_tag: "WT-5".into(),
            name: "EXTERIOR WALL".into(),
            layers: vec![
                WallLayer { material: "FACE BRICK".into(), thickness_in: 3.625 },
                WallLayer { material: "AIR SPACE".into(), thickness_in: 1.0 },
                WallLayer { material: "RIGID INSULATION".into(), thickness_in: 2.0 },
                WallLayer { material: "5/8\" GYP SHEATHING".into(), thickness_in: 0.625 },
                WallLayer { material: "6\" MTL STUD".into(), thickness_in: 6.0 },
                WallLayer { material: "BATT INSULATION".into(), thickness_in: 6.0 },
                WallLayer { material: "5/8\" GYP BD".into(), thickness_in: 0.625 },
            ],
            thickness_in: 13.875,
            fire_rating: "".into(),
            height_rule: "FLOOR TO DECK".into(),
            stc: Some(55),
        },
        WallAssembly {
            id: "wa-6".into(),
            type_tag: "WT-6".into(),
            name: "BATHROOM PARTITION".into(),
            layers: vec![
                WallLayer { material: "1/2\" CEMENT BD".into(), thickness_in: 0.5 },
                WallLayer { material: "3-5/8\" MTL STUD".into(), thickness_in: 3.625 },
                WallLayer { material: "1/2\" CEMENT BD".into(), thickness_in: 0.5 },
            ],
            thickness_in: 4.625,
            fire_rating: "".into(),
            height_rule: "FLOOR TO CLG".into(),
            stc: None,
        },
    ]
}

/// Wall types for the floor plan (mapping assembly IDs to plan tags and thicknesses).
fn wall_type_defs() -> Vec<WallType> {
    let l = |mat: &str, thick: f64, side: &str| WallTypeLayer {
        material: mat.into(),
        thickness_in: thick,
        side: side.into(),
        gauge: None,
        spacing_in: None,
        in_cavity: false,
    };
    vec![
        WallType {
            tag: "WT-1".into(),
            name: "GUEST ROOM DEMISING (1-HR)".into(),
            layers: vec![l("gyp", 0.625, "1"), l("stud", 3.625, "core"), l("gyp", 0.625, "2")],
            thickness_in: 4.875,
            height: "deck".into(),
            fire_rating: Some("1 HR".into()),
            stc: Some(50),
            ..WallType::default()
        },
        WallType {
            tag: "WT-2".into(),
            name: "CORRIDOR WALL (1-HR)".into(),
            layers: vec![l("gyp_x", 1.25, "1"), l("stud", 3.625, "core"), l("gyp_x", 1.25, "2")],
            thickness_in: 6.125,
            height: "deck".into(),
            fire_rating: Some("1 HR".into()),
            stc: Some(55),
            ..WallType::default()
        },
        WallType {
            tag: "WT-3".into(),
            name: "WET WALL".into(),
            layers: vec![l("gyp_mr", 0.625, "1"), l("stud", 3.625, "core"), l("gyp_mr", 0.625, "2")],
            thickness_in: 4.875,
            height: "ceiling".into(),
            ..WallType::default()
        },
        WallType {
            tag: "WT-4".into(),
            name: "SHAFT WALL (2-HR)".into(),
            layers: vec![l("gyp_x", 1.25, "1"), l("shaftliner", 2.5, "core"), l("gyp_x", 0.625, "2")],
            thickness_in: 4.375,
            height: "full".into(),
            fire_rating: Some("2 HR".into()),
            stc: Some(60),
            ..WallType::default()
        },
        WallType {
            tag: "WT-5".into(),
            name: "EXTERIOR WALL".into(),
            layers: vec![l("cmu", 3.625, "1"), l("insulation", 2.0, "core"), l("stud", 6.0, "core"), l("gyp", 0.625, "2")],
            thickness_in: 12.25,
            height: "full".into(),
            external: true,
            ..WallType::default()
        },
        WallType {
            tag: "WT-6".into(),
            name: "BATHROOM PARTITION".into(),
            layers: vec![l("gyp_mr", 0.625, "1"), l("stud", 3.625, "core"), l("gyp_mr", 0.625, "2")],
            thickness_in: 4.875,
            height: "ceiling".into(),
            ..WallType::default()
        },
    ]
}

/// Populate `plan_walls` from the hotel's wall instances for floor plan drawing.
fn hotel_plan_walls(t: &mut Truth) {
    // Map assembly IDs to their type tags.
    let asm_to_tag: BTreeMap<String, String> = t.wall_assemblies.iter().map(|a| (a.id.clone(), a.type_tag.clone())).collect();

    for (pw_id, wi) in t.walls.iter().enumerate() {
        let tag = asm_to_tag.get(&wi.assembly).cloned().unwrap_or_else(|| wi.assembly.clone());
        let rooms: Vec<String> = t
            .building
            .rooms
            .iter()
            .filter(|rm| {
                rm.level == wi.level
                    && rm.min_ft[0] <= wi.from_ft[0].max(wi.to_ft[0]) + 0.5
                    && rm.max_ft[0] >= wi.from_ft[0].min(wi.to_ft[0]) - 0.5
                    && rm.min_ft[1] <= wi.from_ft[1].max(wi.to_ft[1]) + 0.5
                    && rm.max_ft[1] >= wi.from_ft[1].min(wi.to_ft[1]) - 0.5
            })
            .map(|rm| rm.number.clone())
            .collect();

        // Add a door opening for guest room walls that face the corridor.
        let mut openings = Vec::new();
        let is_guest = tag == "WT-1" || tag == "WT-2";
        let wall_len = ((wi.to_ft[0] - wi.from_ft[0]).powi(2) + (wi.to_ft[1] - wi.from_ft[1]).powi(2)).sqrt();
        if is_guest && wall_len > 4.0 {
            // Horizontal walls at corridor boundary get a 3-ft door opening.
            let is_corridor_edge = (wi.from_ft[1] - wi.to_ft[1]).abs() < 0.1
                && !rooms.is_empty();
            if is_corridor_edge {
                let door_start = 2.0_f64.min(wall_len - 3.5);
                if door_start >= 0.0 && door_start + 3.0 <= wall_len + 0.1 {
                    openings.push([door_start, door_start + 3.0]);
                }
            }
        }

        // Wall tag position: midpoint of the wall.
        let mid_s = wall_len / 2.0;
        let tags = vec![WallTag { s_ft: mid_s, on_wall: true, side: 1.0, shift_ft: 0.0, gap_ft: 0.0 }];

        t.plan_walls.push(PlanWall {
            id: format!("pw-{pw_id}"),
            level: wi.level.clone(),
            a_ft: wi.from_ft,
            b_ft: wi.to_ft,
            wall_type: tag,
            tagged: true,
            height_ft: wi.height_ft,
            rooms,
            openings,
            tags,
            layout_from: Some("a".into()),
            plan_tag: None,
            enlarged_tag: None,
        });
    }
}

fn wall_for_room(name: &str, level: &str) -> &'static str {
    match name {
        "CORRIDOR" => "wa-2",
        "KING ROOM" | "DOUBLE QUEEN ROOM" | "ADA KING ROOM" | "SUITE" => {
            if level != "1" {
                "wa-1"
            } else {
                "wa-2"
            }
        }
        "MECHANICAL" | "ELECTRICAL" | "IDF" => "wa-4",
        "LOBBY" | "FRONT DESK" | "BREAKFAST AREA" | "KITCHEN" | "FITNESS CENTER" | "POOL" => "wa-2",
        _ => "wa-1",
    }
}

// --- Ceiling assemblies ---

fn ceiling_assemblies() -> Vec<CeilingAssembly> {
    vec![
        CeilingAssembly {
            id: "CA-1".into(),
            type_tag: "CT-1".into(),
            name: "ACT CEILING".into(),
            layers: vec![CeilingLayer { material: "2x2 ACOUSTICAL TILE".into(), thickness_in: 0.75 }],
            total_depth_in: 0.75,
            fire_rating: String::new(),
        },
        CeilingAssembly {
            id: "CA-2".into(),
            type_tag: "CT-2".into(),
            name: "GYP BD CEILING".into(),
            layers: vec![
                CeilingLayer { material: "5/8\" GYP BD".into(), thickness_in: 0.625 },
                CeilingLayer { material: "HAT CHANNEL".into(), thickness_in: 1.5 },
            ],
            total_depth_in: 2.125,
            fire_rating: String::new(),
        },
        CeilingAssembly {
            id: "CA-3".into(),
            type_tag: "CT-3".into(),
            name: "1-HR RATED CEILING".into(),
            layers: vec![
                CeilingLayer { material: "5/8\" TYPE X GYP BD (2 LAYERS)".into(), thickness_in: 1.25 },
                CeilingLayer { material: "HAT CHANNEL".into(), thickness_in: 1.5 },
            ],
            total_depth_in: 2.75,
            fire_rating: "1-HR".into(),
        },
    ]
}

// --- MEP systems ---

fn mep_systems(st: &Style) -> Vec<System> {
    vec![
        System { code: "SA".into(), name: "SUPPLY AIR".into(), kind: "duct".into(), printed: st.sys("SA") },
        System { code: "RA".into(), name: "RETURN AIR".into(), kind: "duct".into(), printed: st.sys("RA") },
        System { code: "EA".into(), name: "EXHAUST AIR".into(), kind: "duct".into(), printed: st.sys("EA") },
        System { code: "OA".into(), name: "OUTSIDE AIR".into(), kind: "duct".into(), printed: st.sys("OA") },
        System { code: "CW".into(), name: "COLD WATER".into(), kind: "pipe".into(), printed: "CW".into() },
        System { code: "HW".into(), name: "HOT WATER".into(), kind: "pipe".into(), printed: "HW".into() },
        System { code: "HWR".into(), name: "HOT WATER RETURN".into(), kind: "pipe".into(), printed: st.sys("HWR") },
        System { code: "SAN".into(), name: "SANITARY".into(), kind: "pipe".into(), printed: st.sys("SAN") },
        System { code: "V".into(), name: "VENT".into(), kind: "pipe".into(), printed: "V".into() },
        System { code: "G".into(), name: "GAS".into(), kind: "pipe".into(), printed: st.sys("G") },
        System { code: "FP".into(), name: "FIRE PROTECTION".into(), kind: "pipe".into(), printed: st.sys("FP") },
    ]
}

// --- Electrical (hotel scale) ---

fn hotel_electrical(
    t: &mut Truth,
    _st: &Style,
    _rooms_per_side: usize,
    _bldg_w: f64,
    _bldg_d: f64,
    room_depth: f64,
    corr_width: f64,
    stair_w: f64,
    stair_e: f64,
    _elev_start: f64,
    _elev_end: f64,
    r: &mut Rng,
) {
    let mut dev_id = 0;
    let mut panel_id = 0;
    let mut circuit_id = 0;
    // Panels: one per floor + main switchboard on L1
    for (floor_idx, &lv) in ["1", "2", "3", "4"].iter().enumerate() {
        let elec_room = t.building.rooms.iter().find(|rm| rm.level == lv && (rm.name == "ELECTRICAL" || rm.name.contains("CORRIDOR")));
        let px = elec_room.map(|r| r.min_ft[0] + 2.0).unwrap_or(stair_w + 2.0);
        let py = elec_room.map(|r| r.min_ft[1] + 1.0).unwrap_or(room_depth + 1.0);
        let elev = t.building.levels.iter().find(|l| l.code == lv).map(|l| l.elevation_ft).unwrap_or(0.0);

        // Lighting panel
        t.elec_panels.push(ElecPanel {
            id: format!("ep-{panel_id}"),
            tag: format!("L{lv}"),
            kind: "panel".into(),
            level: lv.into(),
            room: elec_room.map(|r| r.id.clone()).unwrap_or_default(),
            at_ft: [px, py, elev + 5.0],
            min_ft: [px - 0.5, py - 0.25],
            max_ft: [px + 0.5, py + 0.25],
            schedule: vec![],
            sheet: String::new(),
            ..ElecPanel::default()
        });
        let lp_tag = format!("L{lv}");
        panel_id += 1;

        // Power panel
        t.elec_panels.push(ElecPanel {
            id: format!("ep-{panel_id}"),
            tag: format!("P{lv}"),
            kind: "panel".into(),
            level: lv.into(),
            room: elec_room.map(|r| r.id.clone()).unwrap_or_default(),
            at_ft: [px + 2.0, py, elev + 5.0],
            min_ft: [px + 1.5, py - 0.25],
            max_ft: [px + 2.5, py + 0.25],
            schedule: vec![],
            sheet: String::new(),
            ..ElecPanel::default()
        });
        let pp_tag = format!("P{lv}");
        panel_id += 1;

        // Devices in each room on this level
        let level_rooms: Vec<Room> = t.building.rooms.iter().filter(|rm| rm.level == lv).cloned().collect();
        let mut ckt_num = 1;
        for rm in &level_rooms {
            let cx = (rm.min_ft[0] + rm.max_ft[0]) / 2.0;
            let cy = (rm.min_ft[1] + rm.max_ft[1]) / 2.0;
            let w = rm.max_ft[0] - rm.min_ft[0];

            // Light fixture(s)
            let n_lights = ((w / 10.0).ceil() as usize).max(1);
            let mut ckt_devices = Vec::new();
            for i in 0..n_lights {
                let lx = rm.min_ft[0] + (i as f64 + 0.5) * w / n_lights as f64;
                let jx = lx + (r.below(3) as f64 - 1.0) * 0.5;
                let jy = cy + (r.below(3) as f64 - 1.0) * 0.5;
                let did = format!("ed-{dev_id}");
                t.elec_devices.push(ElecDevice {
                    id: did.clone(),
                    tag: format!("LT-{}", dev_id + 1),
                    kind: "light_fixture".into(),
                    type_tag: if rm.name.contains("CORRIDOR") { "B".into() } else { "A".into() },
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [lx, cy, elev + rm.ceiling.as_ref().map(|c| c.height_ft).unwrap_or(9.0)],
                    drawn_ft: [jx, jy],
                    mounting_in: 0.0,
                    mounting_note: Some("CLG".into()),
                    arch_positioned: false,
                    circuit: format!("{lp_tag}/{ckt_num}"),
                    panel: lp_tag.clone(),
                    sheet: String::new(),
                });
                ckt_devices.push(did);
                dev_id += 1;
            }

            // Switch at door
            let did = format!("ed-{dev_id}");
            t.elec_devices.push(ElecDevice {
                id: did.clone(),
                tag: format!("SW-{}", dev_id + 1),
                kind: "switch".into(),
                type_tag: "S".into(),
                level: lv.into(),
                room: rm.id.clone(),
                at_ft: [hand_x(rm, rm.min_ft[0] + 0.5), rm.max_ft[1] - 0.5, elev + 4.0],
                drawn_ft: [hand_x(rm, rm.min_ft[0] + 0.5 + r.below(2) as f64 * 0.3), rm.max_ft[1] - 0.5],
                mounting_in: 48.0,
                mounting_note: Some("+48\" AFF".into()),
                arch_positioned: true,
                circuit: format!("{lp_tag}/{ckt_num}"),
                panel: lp_tag.clone(),
                sheet: String::new(),
            });
            ckt_devices.push(did);
            dev_id += 1;

            // Lighting circuit
            t.elec_circuits.push(ElecCircuit {
                id: format!("ec-{circuit_id}"),
                label: format!("{lp_tag}/{ckt_num}"),
                panel: lp_tag.clone(),
                number: format!("{ckt_num}"),
                level: lv.into(),
                devices: ckt_devices,
                homerun_ft: [cx, cy],
                sheet: String::new(),
            });
            circuit_id += 1;
            ckt_num += 1;

            // Receptacles
            let typical = ((w / 12.0).ceil() as usize).max(1);
            // An end unit's override adds receptacles on its end wall (drawn where they go: no
            // jitter, so the other rooms' draws stay as they were).
            let n_recep = unit_override(t, &rm.id, "devices.receptacle").and_then(|v| v.parse::<usize>().ok()).unwrap_or(typical).clamp(1, 16);
            let mut pwr_devices = Vec::new();
            for i in 0..n_recep {
                let rx = rm.min_ft[0] + (i as f64 + 0.5) * w / n_recep as f64;
                let did = format!("ed-{dev_id}");
                t.elec_devices.push(ElecDevice {
                    id: did.clone(),
                    tag: format!("R-{}", dev_id + 1),
                    kind: "receptacle".into(),
                    type_tag: "R".into(),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [rx, rm.min_ft[1] + 0.5, elev + 1.5],
                    drawn_ft: [rx + if i < typical { r.below(2) as f64 * 0.3 } else { 0.0 }, rm.min_ft[1] + 0.5],
                    mounting_in: 18.0,
                    mounting_note: Some("+18\" AFF".into()),
                    arch_positioned: true,
                    circuit: format!("{pp_tag}/{ckt_num}"),
                    panel: pp_tag.clone(),
                    sheet: String::new(),
                });
                pwr_devices.push(did);
                dev_id += 1;
            }

            // Power circuit
            t.elec_circuits.push(ElecCircuit {
                id: format!("ec-{circuit_id}"),
                label: format!("{pp_tag}/{ckt_num}"),
                panel: pp_tag.clone(),
                number: format!("{ckt_num}"),
                level: lv.into(),
                devices: pwr_devices,
                homerun_ft: [cx, cy],
                sheet: String::new(),
            });
            circuit_id += 1;
            ckt_num += 1;
        }

        // Main conduit run along corridor
        let corr_y = room_depth + corr_width / 2.0;
        t.elec_conduits.push(ElecConduit {
            id: format!("econd-{floor_idx}"),
            conduit_type: if r.chance(0.5) { "EMT" } else { "MC" }.into(),
            size: "1".into(),
            size_in: 1.0,
            level: lv.into(),
            from_ft: [stair_w, corr_y],
            to_ft: [stair_e, corr_y],
            tag: None,
            sheet: String::new(),
        });
    }
}

// --- Fire alarm ---

fn hotel_fire_alarm(t: &mut Truth, bldg_w: f64, _bldg_d: f64, room_depth: f64, corr_width: f64) {
    let mut fa_id = 0;
    let mut addr = 1u32;

    // FACP in electrical room on L1
    let elec_room = t.building.rooms.iter().find(|rm| rm.level == "1" && rm.name == "ELECTRICAL");
    let ex = elec_room.map(|r| r.min_ft[0] + 3.0).unwrap_or(10.0);
    let ey = elec_room.map(|r| r.min_ft[1] + 1.0).unwrap_or(10.0);
    t.fire_alarm_devices.push(FireAlarmDevice {
        id: format!("fa-{fa_id}"),
        kind: "facp".into(),
        tag: "FACP".into(),
        level: "1".into(),
        room: elec_room.map(|r| r.id.clone()).unwrap_or_default(),
        at_ft: [ex, ey, 5.0],
        mounting: "WALL".into(),
        mounting_in: 60.0,
        slc: "SLC-1".into(),
        circuit: "SLC-1".into(),
        sheet: String::new(),
        ..FireAlarmDevice::default()
    });
    fa_id += 1;

    // NAC power supply next to FACP
    t.fire_alarm_devices.push(FireAlarmDevice {
        id: format!("fa-{fa_id}"),
        kind: "nac_panel".into(),
        tag: "NAC-PS-1".into(),
        level: "1".into(),
        room: elec_room.map(|r| r.id.clone()).unwrap_or_default(),
        at_ft: [ex + 2.0, ey, 5.0],
        mounting: "WALL".into(),
        mounting_in: 60.0,
        nac: "NAC-1".into(),
        circuit: "NAC-1".into(),
        sheet: String::new(),
        ..FireAlarmDevice::default()
    });
    fa_id += 1;

    // Annunciator at front desk
    t.fire_alarm_devices.push(FireAlarmDevice {
        id: format!("fa-{fa_id}"),
        kind: "annunciator".into(),
        tag: "ANN".into(),
        level: "1".into(),
        room: "rm-1-FD".into(),
        at_ft: [20.0, 2.0, 5.0],
        mounting: "WALL".into(),
        mounting_in: 60.0,
        slc: "SLC-1".into(),
        circuit: "SLC-1".into(),
        sheet: String::new(),
        ..FireAlarmDevice::default()
    });
    fa_id += 1;

    // Elevator recall module on L1
    for elev in &t.building.elevators.clone() {
        t.fire_alarm_devices.push(FireAlarmDevice {
            id: format!("fa-{fa_id}"),
            kind: "elevator_recall".into(),
            tag: format!("ER-{}", fa_id + 1),
            level: "1".into(),
            room: "rm-1-CORR".to_string(),
            at_ft: [elev.min_ft[0] + 1.0, elev.min_ft[1] - 0.5, 7.0],
            mounting: "WALL".into(),
            mounting_in: 84.0,
            slc: "SLC-1".into(),
            address: Some(format!("SLC1-A{addr}")),
            interface: Some("elevator_recall".into()),
            circuit: "SLC-1".into(),
            sheet: String::new(),
            ..FireAlarmDevice::default()
        });
        fa_id += 1;
        addr += 1;
    }

    // Door holders at stair doors (mag-release)
    for stair in &t.building.stairs.clone() {
        for lv in &stair.levels {
            let elev = t.building.levels.iter().find(|l| l.code.as_str() == lv.as_str()).map(|l| l.elevation_ft).unwrap_or(0.0);
            t.fire_alarm_devices.push(FireAlarmDevice {
                id: format!("fa-{fa_id}"),
                kind: "mag_door_holder".into(),
                tag: format!("DH-{}", fa_id + 1),
                level: lv.clone(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [stair.min_ft[0] + 0.5, stair.min_ft[1] - 0.5, elev + 7.0],
                mounting: "WALL".into(),
                mounting_in: 84.0,
                slc: "SLC-1".into(),
                address: Some(format!("SLC1-A{addr}")),
                interface: Some("door_holder".into()),
                circuit: "SLC-1".into(),
                sheet: String::new(),
                ..FireAlarmDevice::default()
            });
            fa_id += 1;
            addr += 1;
        }
    }

    for lv in ["1", "2", "3", "4"] {
        let elev = t.building.levels.iter().find(|l| l.code == lv).map(|l| l.elevation_ft).unwrap_or(0.0);
        let ceil_h = if lv == "1" { 10.0 } else { 8.5 };

        // Smoke detectors in corridor every 30 ft
        let corr_y = room_depth + corr_width / 2.0;
        let mut sx = 10.0;
        while sx < bldg_w - 5.0 {
            t.fire_alarm_devices.push(FireAlarmDevice {
                id: format!("fa-{fa_id}"),
                kind: "smoke_detector".into(),
                tag: format!("SD-{}", fa_id + 1),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [sx, corr_y, elev + ceil_h],
                mounting: "CLG".into(),
                mounting_in: 0.0,
                slc: "SLC-1".into(),
                address: Some(format!("SLC1-A{addr}")),
                circuit: "SLC-1".into(),
                sheet: String::new(),
                ..FireAlarmDevice::default()
            });
            fa_id += 1;
            addr += 1;
            sx += 30.0;
        }

        // Duct detector at RTU on roof
        if lv == "1" {
            t.fire_alarm_devices.push(FireAlarmDevice {
                id: format!("fa-{fa_id}"),
                kind: "duct_detector".into(),
                tag: format!("DD-{}", fa_id + 1),
                level: "R".into(),
                room: String::new(),
                at_ft: [bldg_w / 2.0, room_depth + corr_width / 2.0, 40.0],
                mounting: "CLG".into(),
                mounting_in: 0.0,
                slc: "SLC-1".into(),
                address: Some(format!("SLC1-A{addr}")),
                circuit: "SLC-1".into(),
                sheet: String::new(),
                ..FireAlarmDevice::default()
            });
            fa_id += 1;
            addr += 1;
        }

        // Smoke detectors in guest rooms
        for rm in &t.building.rooms.clone() {
            if rm.level != lv {
                continue;
            }
            if rm.name.contains("KING") || rm.name.contains("QUEEN") || rm.name.contains("SUITE") || rm.name.contains("ADA") {
                let cx = (rm.min_ft[0] + rm.max_ft[0]) / 2.0;
                let cy = (rm.min_ft[1] + rm.max_ft[1]) / 2.0;
                let ch = rm.ceiling.as_ref().map(|c| c.height_ft).unwrap_or(8.5);
                t.fire_alarm_devices.push(FireAlarmDevice {
                    id: format!("fa-{fa_id}"),
                    kind: "smoke_detector".into(),
                    tag: format!("SD-{}", fa_id + 1),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [cx, cy, elev + ch],
                    mounting: "CLG".into(),
                    mounting_in: 0.0,
                    slc: "SLC-1".into(),
                    address: Some(format!("SLC1-A{addr}")),
                    circuit: "SLC-1".into(),
                    sheet: String::new(),
                    ..FireAlarmDevice::default()
                });
                fa_id += 1;
                addr += 1;
            }
        }

        // Horn/strobe in corridor with candela
        let mut hx = 15.0;
        while hx < bldg_w - 10.0 {
            t.fire_alarm_devices.push(FireAlarmDevice {
                id: format!("fa-{fa_id}"),
                kind: "horn_strobe".into(),
                tag: format!("HS-{}", fa_id + 1),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [hx, room_depth + 0.5, elev + 7.0],
                mounting: "WALL".into(),
                mounting_in: 84.0,
                candela: Some(75),
                nac: "NAC-1".into(),
                circuit: "NAC-1".into(),
                sheet: String::new(),
                ..FireAlarmDevice::default()
            });
            fa_id += 1;
            hx += 75.0;
        }

        // Strobes in guest rooms (15 cd wall mount)
        for rm in &t.building.rooms.clone() {
            if rm.level != lv {
                continue;
            }
            if rm.name.contains("KING") || rm.name.contains("QUEEN") || rm.name.contains("SUITE") || rm.name.contains("ADA") {
                t.fire_alarm_devices.push(FireAlarmDevice {
                    id: format!("fa-{fa_id}"),
                    kind: "strobe".into(),
                    tag: format!("ST-{}", fa_id + 1),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [rm.min_ft[0] + 1.0, rm.min_ft[1] + 0.5, elev + 7.0],
                    mounting: "WALL".into(),
                    mounting_in: 84.0,
                    candela: Some(15),
                    nac: "NAC-1".into(),
                    circuit: "NAC-1".into(),
                    sheet: String::new(),
                    ..FireAlarmDevice::default()
                });
                fa_id += 1;
            }
        }

        // Pull stations at stairs
        for stair in &t.building.stairs.clone() {
            if !stair.levels.contains(&lv.to_string()) {
                continue;
            }
            t.fire_alarm_devices.push(FireAlarmDevice {
                id: format!("fa-{fa_id}"),
                kind: "pull_station".into(),
                tag: format!("PS-{}", fa_id + 1),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [stair.min_ft[0] + 1.0, stair.min_ft[1] + 0.5, elev + 4.0],
                mounting: "WALL".into(),
                mounting_in: 48.0,
                slc: "SLC-1".into(),
                address: Some(format!("SLC1-A{addr}")),
                circuit: "SLC-1".into(),
                sheet: String::new(),
                ..FireAlarmDevice::default()
            });
            fa_id += 1;
            addr += 1;
        }

        // Monitor module for sprinkler flow/tamper on each floor
        t.fire_alarm_devices.push(FireAlarmDevice {
            id: format!("fa-{fa_id}"),
            kind: "monitor_module".into(),
            tag: format!("MM-{}", fa_id + 1),
            level: lv.into(),
            room: String::new(),
            at_ft: [5.0, room_depth + corr_width + 1.0, elev + 8.0],
            mounting: "WALL".into(),
            mounting_in: 96.0,
            slc: "SLC-1".into(),
            address: Some(format!("SLC1-A{addr}")),
            interface: Some("sprinkler_flow_tamper".into()),
            circuit: "SLC-1".into(),
            sheet: String::new(),
            ..FireAlarmDevice::default()
        });
        fa_id += 1;
        addr += 1;
    }
}

// --- Telecom ---

fn hotel_telecom(t: &mut Truth, r: &mut Rng) {
    let mut tc_id = 0;

    // MDF on L1 in telecom room
    let telecom_room = t.building.rooms.iter().find(|rm| rm.level == "1" && (rm.name == "TELECOM" || rm.name == "ELECTRICAL"));
    let tr_x = telecom_room.map(|r| r.min_ft[0] + 1.0).unwrap_or(8.0);
    let tr_y = telecom_room.map(|r| r.min_ft[1] + 1.0).unwrap_or(8.0);
    let mdf_room_id = telecom_room.map(|r| r.id.clone()).unwrap_or_default();

    t.telecom_devices.push(TelecomDevice {
        id: format!("tc-{tc_id}"),
        kind: "mdf".into(),
        tag: "MDF".into(),
        level: "1".into(),
        room: mdf_room_id.clone(),
        at_ft: [tr_x, tr_y, 0.0],
        mounting_in: 0.0,
        lv_system: "data".into(),
        sheet: String::new(),
        ..TelecomDevice::default()
    });
    let mdf_tag = format!("tc-{tc_id}");
    tc_id += 1;

    // Backboard in MDF
    t.telecom_devices.push(TelecomDevice {
        id: format!("tc-{tc_id}"),
        kind: "backboard".into(),
        tag: "BB-MDF".into(),
        level: "1".into(),
        room: mdf_room_id.clone(),
        at_ft: [tr_x + 1.0, tr_y, 4.0],
        mounting_in: 48.0,
        lv_system: "data".into(),
        sheet: String::new(),
        ..TelecomDevice::default()
    });
    tc_id += 1;

    // Rack in MDF
    t.telecom_devices.push(TelecomDevice {
        id: format!("tc-{tc_id}"),
        kind: "rack".into(),
        tag: "RACK-MDF".into(),
        level: "1".into(),
        room: mdf_room_id,
        at_ft: [tr_x + 3.0, tr_y, 0.0],
        mounting_in: 0.0,
        lv_system: "data".into(),
        sheet: String::new(),
        ..TelecomDevice::default()
    });
    tc_id += 1;

    // IDF per upper floor
    for lv in ["2", "3", "4"] {
        let elev = t.building.levels.iter().find(|l| l.code == lv).map(|l| l.elevation_ft).unwrap_or(0.0);
        let idf_room = t.building.rooms.iter().find(|rm| rm.level == lv && (rm.name == "TELECOM" || rm.name == "ELECTRICAL"));
        let ix = idf_room.map(|r| r.min_ft[0] + 1.0).unwrap_or(8.0);
        let iy = idf_room.map(|r| r.min_ft[1] + 1.0).unwrap_or(8.0);
        let idf_room_id = idf_room.map(|r| r.id.clone()).unwrap_or_default();

        t.telecom_devices.push(TelecomDevice {
            id: format!("tc-{tc_id}"),
            kind: "idf".into(),
            tag: format!("IDF-{lv}"),
            level: lv.into(),
            room: idf_room_id.clone(),
            at_ft: [ix, iy, elev],
            mounting_in: 0.0,
            lv_system: "data".into(),
            home_panel: Some(mdf_tag.clone()),
            sheet: String::new(),
            ..TelecomDevice::default()
        });
        tc_id += 1;

        // Patch panel in IDF
        t.telecom_devices.push(TelecomDevice {
            id: format!("tc-{tc_id}"),
            kind: "patch_panel".into(),
            tag: format!("PP-{lv}"),
            level: lv.into(),
            room: idf_room_id,
            at_ft: [ix + 1.0, iy, elev + 4.0],
            mounting_in: 48.0,
            lv_system: "data".into(),
            home_panel: Some(mdf_tag.clone()),
            sheet: String::new(),
            ..TelecomDevice::default()
        });
        tc_id += 1;
    }

    for lv in ["1", "2", "3", "4"] {
        let elev = t.building.levels.iter().find(|l| l.code == lv).map(|l| l.elevation_ft).unwrap_or(0.0);
        let idf_tag = if lv == "1" { "MDF".to_string() } else { format!("IDF-{lv}") };

        for rm in &t.building.rooms.clone() {
            if rm.level != lv {
                continue;
            }

            // Data outlet in every room (2 × Cat6)
            t.telecom_devices.push(TelecomDevice {
                id: format!("tc-{tc_id}"),
                kind: "data_outlet".into(),
                tag: format!("D-{}", tc_id + 1),
                level: lv.into(),
                room: rm.id.clone(),
                at_ft: [rm.min_ft[0] + 2.0, rm.min_ft[1] + 0.5, elev + 1.5],
                mounting_in: 18.0,
                lv_system: "data".into(),
                cable_spec: Some("2 x Cat6".into()),
                box_type: Some("lv_bracket".into()),
                faceplate: Some("double".into()),
                home_panel: Some(idf_tag.clone()),
                sheet: String::new(),
            });
            tc_id += 1;

            // TV outlet in guest rooms (1 × RG6)
            if rm.name.contains("KING") || rm.name.contains("QUEEN") || rm.name.contains("SUITE") {
                t.telecom_devices.push(TelecomDevice {
                    id: format!("tc-{tc_id}"),
                    kind: "tv_outlet".into(),
                    tag: format!("TV-{}", tc_id + 1),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [(rm.min_ft[0] + rm.max_ft[0]) / 2.0, rm.max_ft[1] - 0.5, elev + 4.0],
                    mounting_in: 48.0,
                    lv_system: "tv".into(),
                    cable_spec: Some("1 x RG6".into()),
                    box_type: Some("lv_bracket".into()),
                    faceplate: Some("single".into()),
                    home_panel: Some(idf_tag.clone()),
                    sheet: String::new(),
                });
                tc_id += 1;

                // Telephone
                t.telecom_devices.push(TelecomDevice {
                    id: format!("tc-{tc_id}"),
                    kind: "telephone".into(),
                    tag: format!("TEL-{}", tc_id + 1),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [rm.max_ft[0] - 2.0, rm.min_ft[1] + 1.0, elev + 2.5],
                    mounting_in: 18.0,
                    lv_system: "telephone".into(),
                    cable_spec: Some("1 x Cat6".into()),
                    box_type: Some("lv_bracket".into()),
                    faceplate: Some("single".into()),
                    home_panel: Some(idf_tag.clone()),
                    sheet: String::new(),
                });
                tc_id += 1;
            }
        }

        // WiFi APs in corridor
        let corr_y =
            t.building.rooms.iter().find(|rm| rm.level == lv && rm.name == "CORRIDOR").map(|r| (r.min_ft[1] + r.max_ft[1]) / 2.0).unwrap_or(27.0);
        let ceil_h = if lv == "1" { 10.0 } else { 8.5 };
        let bldg_w = t.building.grid.x.last().map(|g| g.at_ft).unwrap_or(100.0);
        let mut ax = 20.0;
        while ax < bldg_w - 10.0 {
            t.telecom_devices.push(TelecomDevice {
                id: format!("tc-{tc_id}"),
                kind: "wifi_ap".into(),
                tag: format!("WAP-{}", tc_id + 1),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [ax, corr_y, elev + ceil_h],
                mounting_in: 0.0,
                lv_system: "data".into(),
                cable_spec: Some("2 x Cat6".into()),
                home_panel: Some(idf_tag.clone()),
                sheet: String::new(),
                ..TelecomDevice::default()
            });
            tc_id += 1;
            ax += 50.0;
        }

        // Cameras at corridor ends and lobby
        if lv == "1" || r.chance(0.5) {
            t.telecom_devices.push(TelecomDevice {
                id: format!("tc-{tc_id}"),
                kind: "camera".into(),
                tag: format!("CAM-{}", tc_id + 1),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [10.0, corr_y, elev + ceil_h],
                mounting_in: 0.0,
                lv_system: "security".into(),
                cable_spec: Some("1 x Cat6".into()),
                home_panel: Some(idf_tag.clone()),
                sheet: String::new(),
                ..TelecomDevice::default()
            });
            tc_id += 1;
        }

        // Access control at stairs and elevator
        for stair in &t.building.stairs.clone() {
            if !stair.levels.contains(&lv.to_string()) {
                continue;
            }
            // Card reader
            t.telecom_devices.push(TelecomDevice {
                id: format!("tc-{tc_id}"),
                kind: "card_reader".into(),
                tag: format!("CR-{}", tc_id + 1),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [stair.min_ft[0] + 0.5, stair.min_ft[1] - 0.5, elev + 4.0],
                mounting_in: 48.0,
                lv_system: "access_control".into(),
                cable_spec: Some("1 x Cat6".into()),
                home_panel: Some(idf_tag.clone()),
                sheet: String::new(),
                ..TelecomDevice::default()
            });
            tc_id += 1;

            // Door contact
            t.telecom_devices.push(TelecomDevice {
                id: format!("tc-{tc_id}"),
                kind: "door_contact".into(),
                tag: format!("DC-{}", tc_id + 1),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [stair.min_ft[0] + 0.5, stair.min_ft[1] - 0.2, elev + 7.0],
                mounting_in: 84.0,
                lv_system: "access_control".into(),
                home_panel: Some(idf_tag.clone()),
                sheet: String::new(),
                ..TelecomDevice::default()
            });
            tc_id += 1;

            // REX (request to exit)
            t.telecom_devices.push(TelecomDevice {
                id: format!("tc-{tc_id}"),
                kind: "rex".into(),
                tag: format!("REX-{}", tc_id + 1),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [stair.min_ft[0] + 2.0, stair.min_ft[1] - 0.5, elev + 4.0],
                mounting_in: 48.0,
                lv_system: "access_control".into(),
                home_panel: Some(idf_tag.clone()),
                sheet: String::new(),
                ..TelecomDevice::default()
            });
            tc_id += 1;
        }

        // Nurse call / ADA pull cords in ADA rooms
        for rm in &t.building.rooms.clone() {
            if rm.level != lv || !rm.name.contains("ADA") {
                continue;
            }
            t.telecom_devices.push(TelecomDevice {
                id: format!("tc-{tc_id}"),
                kind: "ada_pull_cord".into(),
                tag: format!("ADA-{}", tc_id + 1),
                level: lv.into(),
                room: rm.id.clone(),
                at_ft: [rm.min_ft[0] + 3.0, rm.min_ft[1] + 1.0, elev + 1.0],
                mounting_in: 12.0,
                lv_system: "nurse_call".into(),
                home_panel: Some(idf_tag.clone()),
                sheet: String::new(),
                ..TelecomDevice::default()
            });
            tc_id += 1;
        }

        // Speakers in corridor (sound system)
        let mut spx = 25.0;
        while spx < bldg_w - 10.0 {
            t.telecom_devices.push(TelecomDevice {
                id: format!("tc-{tc_id}"),
                kind: "speaker".into(),
                tag: format!("SPK-{}", tc_id + 1),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [spx, corr_y, elev + ceil_h],
                mounting_in: 0.0,
                lv_system: "sound".into(),
                cable_spec: Some("1 x 18/2".into()),
                home_panel: Some(idf_tag.clone()),
                sheet: String::new(),
                ..TelecomDevice::default()
            });
            tc_id += 1;
            spx += 40.0;
        }
    }
}

// --- Equipment connections (Q308) ---

fn hotel_equip_connections(t: &mut Truth) {
    let mut ec_id = 0;

    for lv in ["1", "2", "3", "4"] {
        let elev = t.building.levels.iter().find(|l| l.code == lv).map(|l| l.elevation_ft).unwrap_or(0.0);

        // PTAC/VTAC in every guest room
        for rm in &t.building.rooms.clone() {
            if rm.level != lv {
                continue;
            }
            if rm.name.contains("KING") || rm.name.contains("QUEEN") || rm.name.contains("SUITE") || rm.name.contains("ADA") {
                let mut sched = BTreeMap::new();
                sched.insert("voltage".into(), "208V".into());
                sched.insert("phase".into(), "1".into());
                sched.insert("MCA".into(), "12.5".into());
                sched.insert("MOCP".into(), "20".into());
                t.equip_connections.push(EquipConnection {
                    id: format!("ec-{ec_id}"),
                    kind: "ptac".into(),
                    tag: format!("PTAC-{}", ec_id + 1),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [hand_x(rm, rm.max_ft[0] - 1.0), rm.min_ft[1] + 0.5, elev + 1.0],
                    circuit: format!("P{lv}/{}", (ec_id % 42) + 1),
                    panel: format!("P{lv}"),
                    connection_type: "receptacle".into(),
                    disconnect_tag: None,
                    schedule: sched,
                    sheet: String::new(),
                });
                ec_id += 1;
            }
        }

        // Water heater on L1
        if lv == "1" {
            let mech = t.building.rooms.iter().find(|rm| rm.level == "1" && rm.name == "MECHANICAL");
            let mx = mech.map(|r| r.min_ft[0] + 2.0).unwrap_or(15.0);
            let my = mech.map(|r| r.min_ft[1] + 2.0).unwrap_or(15.0);
            let mut sched = BTreeMap::new();
            sched.insert("voltage".into(), "208V".into());
            sched.insert("phase".into(), "3".into());
            sched.insert("MCA".into(), "36".into());
            sched.insert("MOCP".into(), "45".into());
            t.equip_connections.push(EquipConnection {
                id: format!("ec-{ec_id}"),
                kind: "water_heater".into(),
                tag: "WH-1".into(),
                level: "1".into(),
                room: mech.map(|r| r.id.clone()).unwrap_or_default(),
                at_ft: [mx, my, 0.0],
                circuit: "P1/25".into(),
                panel: "P1".into(),
                connection_type: "disconnect".into(),
                disconnect_tag: Some("DS-WH1".into()),
                schedule: sched,
                sheet: String::new(),
            });
            ec_id += 1;
        }
    }

    // Elevator on L1
    for elev_shaft in &t.building.elevators.clone() {
        let mut sched = BTreeMap::new();
        sched.insert("voltage".into(), "208V".into());
        sched.insert("phase".into(), "3".into());
        sched.insert("MCA".into(), "50".into());
        sched.insert("MOCP".into(), "60".into());
        t.equip_connections.push(EquipConnection {
            id: format!("ec-{ec_id}"),
            kind: "elevator".into(),
            tag: format!("ELEV-{}", elev_shaft.tag),
            level: "1".into(),
            room: String::new(),
            at_ft: [(elev_shaft.min_ft[0] + elev_shaft.max_ft[0]) / 2.0, (elev_shaft.min_ft[1] + elev_shaft.max_ft[1]) / 2.0, 0.0],
            circuit: "MDP/1".into(),
            panel: "MDP".into(),
            connection_type: "disconnect".into(),
            disconnect_tag: Some(format!("DS-ELEV-{}", elev_shaft.tag)),
            schedule: sched,
            sheet: String::new(),
        });
        ec_id += 1;
    }

    // RTU on roof
    let bldg_w = t.building.grid.x.last().map(|g| g.at_ft).unwrap_or(100.0);
    let room_depth = 24.0;
    let corr_w = 6.0;
    let mut sched = BTreeMap::new();
    sched.insert("voltage".into(), "208V".into());
    sched.insert("phase".into(), "3".into());
    sched.insert("MCA".into(), "72".into());
    sched.insert("MOCP".into(), "90".into());
    t.equip_connections.push(EquipConnection {
        id: format!("ec-{ec_id}"),
        kind: "rtu".into(),
        tag: "RTU-1".into(),
        level: "R".into(),
        room: String::new(),
        at_ft: [bldg_w / 2.0, room_depth + corr_w / 2.0, 40.0],
        circuit: "MDP/3".into(),
        panel: "MDP".into(),
        connection_type: "disconnect".into(),
        disconnect_tag: Some("DS-RTU1".into()),
        schedule: sched,
        sheet: String::new(),
    });
    ec_id += 1;

    // Signage at entrance
    let mut sched = BTreeMap::new();
    sched.insert("voltage".into(), "120V".into());
    sched.insert("phase".into(), "1".into());
    t.equip_connections.push(EquipConnection {
        id: format!("ec-{ec_id}"),
        kind: "signage".into(),
        tag: "SIGN-1".into(),
        level: "1".into(),
        room: String::new(),
        at_ft: [bldg_w / 2.0, -2.0, 10.0],
        circuit: "L1/15".into(),
        panel: "L1".into(),
        connection_type: "junction_box".into(),
        disconnect_tag: None,
        schedule: sched,
        sheet: String::new(),
    });
    let _ = ec_id;
}

// --- Sleeves and cable pathways (Q308) ---

fn hotel_sleeves_and_pathways(t: &mut Truth) {
    let mut sl_id = 0;
    let mut cp_id = 0;
    let bldg_w = t.building.grid.x.last().map(|g| g.at_ft).unwrap_or(100.0);
    let room_depth = 24.0;
    let corr_w = 6.0;
    let corr_y = room_depth + corr_w / 2.0;

    // J-hook pathway along corridor on each floor (data + FA + security)
    for lv in ["1", "2", "3", "4"] {
        let elev = t.building.levels.iter().find(|l| l.code == lv).map(|l| l.elevation_ft).unwrap_or(0.0);
        let ceil_h = if lv == "1" { 10.0 } else { 8.5 };
        let pathway_elev = ceil_h - 0.5;

        t.cable_pathways.push(CablePathway {
            id: format!("cp-{cp_id}"),
            kind: "j_hook".into(),
            systems: vec!["data".into(), "fire_alarm".into(), "security".into(), "access_control".into()],
            level: lv.into(),
            from_ft: [5.0, corr_y],
            to_ft: [bldg_w - 5.0, corr_y],
            elevation_ft: pathway_elev,
            spacing_in: Some(48.0),
            sheet: String::new(),
            ..CablePathway::default()
        });
        cp_id += 1;

        // ENT stubs from corridor to rooms (4 × 2in ENT per BOH room)
        for rm in &t.building.rooms.clone() {
            if rm.level != lv {
                continue;
            }
            // One ENT stub per room for LV
            let rx = (rm.min_ft[0] + rm.max_ft[0]) / 2.0;
            let ry = if rm.min_ft[1] < room_depth { rm.max_ft[1] } else { rm.min_ft[1] };
            t.cable_pathways.push(CablePathway {
                id: format!("cp-{cp_id}"),
                kind: "ent".into(),
                systems: vec!["data".into(), "tv".into()],
                level: lv.into(),
                from_ft: [rx, corr_y],
                to_ft: [rx, ry],
                elevation_ft: pathway_elev,
                size_in: Some(2.0),
                sheet: String::new(),
                ..CablePathway::default()
            });
            cp_id += 1;
        }

        // Sleeves: corridor crosses rated walls at stair doors
        for wall in &t.walls.clone() {
            if wall.level != lv {
                continue;
            }
            // Check if this wall's assembly is rated
            let assembly = t.wall_assemblies.iter().find(|a| a.id == wall.assembly);
            let fire_rating = assembly.map(|a| a.fire_rating.clone()).unwrap_or_default();
            if fire_rating.is_empty() {
                continue;
            }
            let thickness = assembly.map(|a| a.thickness_in).unwrap_or(6.0);

            // If the wall crosses the corridor path roughly, add a sleeve
            let wall_cx = (wall.from_ft[0] + wall.to_ft[0]) / 2.0;
            let wall_cy = (wall.from_ft[1] + wall.to_ft[1]) / 2.0;
            let in_corridor_x = wall_cx > 5.0 && wall_cx < bldg_w - 5.0;
            let near_corridor_y = (wall_cy - corr_y).abs() < corr_w;
            if !in_corridor_x || !near_corridor_y {
                continue;
            }

            t.sleeves.push(Sleeve {
                id: format!("sl-{sl_id}"),
                systems: vec!["data".into(), "fire_alarm".into()],
                level: lv.into(),
                at_ft: [wall_cx, wall_cy, elev + pathway_elev],
                penetrates: "rated_wall".into(),
                element_id: Some(wall.id.clone()),
                material: "EMT".into(),
                size_in: 2.0,
                length_in: thickness,
                fire_rating: fire_rating.clone(),
                firestop_system: Some("3M Fire Barrier CP 25WB+".into()),
                cables: vec!["4 x Cat6".into(), "2 x 18/2 FPLR".into()],
                sheet: String::new(),
            });
            sl_id += 1;
        }

        // Floor sleeve for riser between levels (data + FA)
        if lv != "1" {
            let riser_x =
                t.building.shafts.iter().find(|s| s.kind == "electrical").map(|s| (s.min_ft[0] + s.max_ft[0]) / 2.0).unwrap_or(bldg_w / 2.0);
            let riser_y = t.building.shafts.iter().find(|s| s.kind == "electrical").map(|s| (s.min_ft[1] + s.max_ft[1]) / 2.0).unwrap_or(corr_y);
            t.sleeves.push(Sleeve {
                id: format!("sl-{sl_id}"),
                systems: vec!["data".into(), "fire_alarm".into(), "security".into()],
                level: lv.into(),
                at_ft: [riser_x, riser_y, elev],
                penetrates: "floor".into(),
                element_id: None,
                material: "EMT".into(),
                size_in: 4.0,
                length_in: 6.0,
                fire_rating: "2-HR".into(),
                firestop_system: Some("3M Fire Barrier CP 25WB+".into()),
                cables: vec!["24 x Cat6".into(), "4 x 18/2 FPLR".into(), "2 x RG6".into()],
                sheet: String::new(),
            });
            sl_id += 1;
        }
    }
}

// --- Low-voltage BOM (Q308) ---

fn hotel_lv_bom(t: &mut Truth) {
    let mut bom_id = 0;

    // Count FA devices by kind
    let mut fa_counts: BTreeMap<String, u32> = BTreeMap::new();
    for d in &t.fire_alarm_devices {
        *fa_counts.entry(d.kind.clone()).or_default() += 1;
    }
    for (kind, count) in &fa_counts {
        let desc = match kind.as_str() {
            "facp" => "Fire Alarm Control Panel",
            "nac_panel" => "NAC Power Supply",
            "annunciator" => "Remote Annunciator",
            "smoke_detector" => "Photoelectric Smoke Detector",
            "heat_detector" => "Rate-of-Rise Heat Detector",
            "horn_strobe" => "Horn/Strobe, Wall Mount",
            "strobe" => "Strobe, Wall Mount",
            "pull_station" => "Manual Pull Station",
            "duct_detector" => "Duct Smoke Detector",
            "monitor_module" => "Monitor Module",
            "relay_module" => "Relay Module",
            "elevator_recall" => "Elevator Recall Interface",
            "mag_door_holder" => "Magnetic Door Holder",
            _ => "Fire Alarm Device",
        };
        t.lv_bom.push(LvBomLine {
            id: format!("bom-{bom_id}"),
            system: "fire_alarm".into(),
            category: "device".into(),
            description: desc.into(),
            quantity: f64::from(*count),
            unit: "EA".into(),
            scope: "ours".into(),
            spec_id: Some("spec-12".into()),
            ..LvBomLine::default()
        });
        bom_id += 1;
    }

    // Count telecom devices by kind
    let mut tc_counts: BTreeMap<String, u32> = BTreeMap::new();
    for d in &t.telecom_devices {
        *tc_counts.entry(d.kind.clone()).or_default() += 1;
    }
    for (kind, count) in &tc_counts {
        let (sys, desc) = match kind.as_str() {
            "data_outlet" => ("data", "Data Outlet, Cat6"),
            "tv_outlet" => ("tv", "TV Outlet, RG6"),
            "telephone" => ("telephone", "Telephone Outlet, Cat6"),
            "wifi_ap" => ("data", "Wireless Access Point"),
            "camera" => ("security", "IP Camera, PoE"),
            "card_reader" => ("access_control", "Card Reader"),
            "door_contact" => ("access_control", "Door Contact Sensor"),
            "rex" => ("access_control", "Request-to-Exit Sensor"),
            "ada_pull_cord" => ("nurse_call", "ADA Pull Cord Station"),
            "speaker" => ("sound", "Ceiling Speaker"),
            "mdf" => ("data", "Main Distribution Frame"),
            "idf" => ("data", "Intermediate Distribution Frame"),
            "patch_panel" => ("data", "48-Port Patch Panel"),
            "backboard" => ("data", "Plywood Backboard"),
            "rack" => ("data", "19in Equipment Rack"),
            _ => ("data", "Telecom Device"),
        };
        t.lv_bom.push(LvBomLine {
            id: format!("bom-{bom_id}"),
            system: sys.into(),
            category: "device".into(),
            description: desc.into(),
            quantity: f64::from(*count),
            unit: "EA".into(),
            scope: if sys == "security" || sys == "access_control" { "lv_vendor" } else { "ours" }.into(),
            spec_id: if sys == "data" || sys == "telephone" { Some("spec-13".into()) } else { None },
            ..LvBomLine::default()
        });
        bom_id += 1;
    }

    // LV brackets/faceplates from telecom devices
    let lv_brackets = t.telecom_devices.iter().filter(|d| d.box_type.is_some()).count();
    if lv_brackets > 0 {
        t.lv_bom.push(LvBomLine {
            id: format!("bom-{bom_id}"),
            system: "data".into(),
            category: "box".into(),
            description: "Low-Voltage Mounting Bracket".into(),
            quantity: lv_brackets as f64,
            unit: "EA".into(),
            scope: "ours".into(),
            ..LvBomLine::default()
        });
        bom_id += 1;
    }

    let faceplates = t.telecom_devices.iter().filter(|d| d.faceplate.is_some()).count();
    if faceplates > 0 {
        t.lv_bom.push(LvBomLine {
            id: format!("bom-{bom_id}"),
            system: "data".into(),
            category: "faceplate".into(),
            description: "Faceplate".into(),
            quantity: faceplates as f64,
            unit: "EA".into(),
            scope: "ours".into(),
            ..LvBomLine::default()
        });
        bom_id += 1;
    }

    // Estimate cable footage: each device gets ~75 ft average run to IDF
    let data_devices = t.telecom_devices.iter().filter(|d| d.cable_spec.is_some()).count();
    if data_devices > 0 {
        t.lv_bom.push(LvBomLine {
            id: format!("bom-{bom_id}"),
            system: "data".into(),
            category: "cable".into(),
            description: "Cat6 Cable".into(),
            quantity: (data_devices as f64) * 75.0,
            unit: "LF".into(),
            scope: "ours".into(),
            spec_id: Some("spec-13".into()),
            ..LvBomLine::default()
        });
        bom_id += 1;
    }

    // FA cable estimate: SLC loop + NAC runs
    let fa_device_count = t.fire_alarm_devices.len();
    if fa_device_count > 0 {
        // SLC loop: avg 50 ft per device
        t.lv_bom.push(LvBomLine {
            id: format!("bom-{bom_id}"),
            system: "fire_alarm".into(),
            category: "cable".into(),
            description: "18/2 FPLR SLC Cable".into(),
            quantity: (fa_device_count as f64) * 50.0,
            unit: "LF".into(),
            scope: "ours".into(),
            ..LvBomLine::default()
        });
        bom_id += 1;

        // NAC cable
        let nac_devices = t.fire_alarm_devices.iter().filter(|d| !d.nac.is_empty()).count();
        t.lv_bom.push(LvBomLine {
            id: format!("bom-{bom_id}"),
            system: "fire_alarm".into(),
            category: "cable".into(),
            description: "14/2 FPLR NAC Cable".into(),
            quantity: (nac_devices as f64) * 60.0,
            unit: "LF".into(),
            scope: "ours".into(),
            ..LvBomLine::default()
        });
        bom_id += 1;
    }

    // Sleeves
    let sleeve_count = t.sleeves.len();
    if sleeve_count > 0 {
        t.lv_bom.push(LvBomLine {
            id: format!("bom-{bom_id}"),
            system: "data".into(),
            category: "sleeve".into(),
            description: "EMT Sleeve".into(),
            quantity: sleeve_count as f64,
            unit: "EA".into(),
            scope: "ours".into(),
            ..LvBomLine::default()
        });
        bom_id += 1;
    }

    // Firestop
    let firestop_count = t.sleeves.iter().filter(|s| s.firestop_system.is_some()).count();
    if firestop_count > 0 {
        t.lv_bom.push(LvBomLine {
            id: format!("bom-{bom_id}"),
            system: "data".into(),
            category: "firestop".into(),
            description: "Firestop Sealant at Rated Penetration".into(),
            quantity: firestop_count as f64,
            unit: "EA".into(),
            scope: "ours".into(),
            ..LvBomLine::default()
        });
        bom_id += 1;
    }

    // J-hooks from pathways
    for pw in &t.cable_pathways.clone() {
        if pw.kind != "j_hook" {
            continue;
        }
        let run_len_ft = ((pw.to_ft[0] - pw.from_ft[0]).powi(2) + (pw.to_ft[1] - pw.from_ft[1]).powi(2)).sqrt();
        let spacing_ft = pw.spacing_in.unwrap_or(48.0) / 12.0;
        let hook_count = if spacing_ft > 0.0 { (run_len_ft / spacing_ft).ceil() } else { 1.0 };
        t.lv_bom.push(LvBomLine {
            id: format!("bom-{bom_id}"),
            system: pw.systems.first().cloned().unwrap_or_default(),
            category: "support".into(),
            description: "J-Hook Cable Support".into(),
            quantity: hook_count,
            unit: "EA".into(),
            scope: "ours".into(),
            ..LvBomLine::default()
        });
        bom_id += 1;
    }

    let _ = bom_id;
}

// --- Site ---

fn hotel_site(t: &mut Truth, bldg_w: f64, bldg_d: f64, _r: &mut Rng) {
    let mut se_id = 0;

    // Building is set back from property line
    let setback = 30.0;

    // Parking lot to the south
    let parking_min = [-setback, -setback - 60.0];
    let parking_max = [bldg_w + setback, -setback];
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "driveway".into(),
        tag: "MAIN ENTRY".into(),
        at_ft: [bldg_w / 2.0, parking_min[1]],
        min_ft: Some([bldg_w / 2.0 - 12.0, parking_min[1]]),
        max_ft: Some([bldg_w / 2.0 + 12.0, -setback]),
        ..SiteElement::default()
    });
    se_id += 1;

    // Parking spaces
    let space_width = 9.0;
    let space_depth = 18.0;
    let mut px = parking_min[0] + 5.0;
    while px + space_width < parking_max[0] - 5.0 {
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "parking_space".into(),
            tag: format!("P{}", se_id),
            at_ft: [px + space_width / 2.0, parking_min[1] + space_depth / 2.0 + 5.0],
            min_ft: Some([px, parking_min[1] + 5.0]),
            max_ft: Some([px + space_width, parking_min[1] + 5.0 + space_depth]),
            ..SiteElement::default()
        });
        se_id += 1;
        px += space_width;
    }

    // Light poles
    for i in 0..4 {
        let lx = parking_min[0] + 20.0 + i as f64 * (parking_max[0] - parking_min[0] - 40.0) / 3.0;
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "light_pole".into(),
            tag: format!("LP-{}", i + 1),
            at_ft: [lx, parking_min[1] + 30.0],
            min_ft: None,
            max_ft: None,
            ..SiteElement::default()
        });
        se_id += 1;
    }
    // Poles along building perimeter
    for i in 0..6 {
        let lx = -setback + 10.0 + i as f64 * (bldg_w + 2.0 * setback - 20.0) / 5.0;
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "light_pole".into(),
            tag: format!("LP-{}", 5 + i),
            at_ft: [lx, bldg_d + setback - 5.0],
            min_ft: None,
            max_ft: None,
            ..SiteElement::default()
        });
        se_id += 1;
    }

    // Transformer pad
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "transformer_pad".into(),
        tag: "XFMR-PAD".into(),
        at_ft: [bldg_w + 10.0, bldg_d / 2.0],
        min_ft: Some([bldg_w + 5.0, bldg_d / 2.0 - 5.0]),
        max_ft: Some([bldg_w + 15.0, bldg_d / 2.0 + 5.0]),
        ..SiteElement::default()
    });
    se_id += 1;

    // Dumpster enclosure
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "dumpster_enclosure".into(),
        tag: "DUMPSTER".into(),
        at_ft: [bldg_w + 10.0, 10.0],
        min_ft: Some([bldg_w + 5.0, 5.0]),
        max_ft: Some([bldg_w + 17.0, 15.0]),
        ..SiteElement::default()
    });
    se_id += 1;

    // Fire hydrant
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "fire_hydrant".into(),
        tag: "FH".into(),
        at_ft: [-setback + 5.0, -setback + 5.0],
        min_ft: None,
        max_ft: None,
        ..SiteElement::default()
    });
    se_id += 1;

    // Underground utilities — with conduit type, size, and count for electrical/telecom.
    for (kind, depth, x_off, mat, size, count) in [
        // Water/sewer/storm get size+material overwritten by design_utility_piping; set
        // initial values that won't conflict (size is replaced, material is replaced).
        ("underground_water", 4.0, -5.0, "", None, None),
        ("underground_sewer", 8.0, -10.0, "", None, None),
        ("underground_electric", 3.0, 5.0, "PVC SCH 40", Some(4.0), Some(2u32)),
        ("underground_gas", 3.0, 10.0, "", None, None),
        ("underground_storm", 6.0, -15.0, "", None, None),
        ("underground_telecom", 3.0, 15.0, "PVC SCH 40", Some(4.0), Some(4u32)),
    ] {
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: kind.into(),
            tag: kind.replace("underground_", "").to_uppercase(),
            at_ft: [bldg_w / 2.0 + x_off, -setback - 30.0],
            min_ft: Some([bldg_w / 2.0 + x_off - 0.5, -setback - 60.0]),
            max_ft: Some([bldg_w / 2.0 + x_off + 0.5, -5.0]),
            depth_ft: Some(depth),
            material: mat.into(),
            size_in: size,
            conduit_count: count,
            ..SiteElement::default()
        });
        se_id += 1;
    }

    // Pull box / handhole — small square near an underground electric run.
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "pull_box".into(),
        tag: "PB-1".into(),
        at_ft: [bldg_w / 2.0 + 5.0, -setback - 15.0],
        min_ft: Some([bldg_w / 2.0 + 3.0, -setback - 17.0]),
        max_ft: Some([bldg_w / 2.0 + 7.0, -setback - 13.0]),
        ..SiteElement::default()
    });
    se_id += 1;

    // Pole base — at the base of the first parking lot light pole.
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "pole_base".into(),
        tag: "PLB-1".into(),
        at_ft: [parking_min[0] + 20.0, parking_min[1] + 30.0],
        min_ft: Some([parking_min[0] + 18.0, parking_min[1] + 28.0]),
        max_ft: Some([parking_min[0] + 22.0, parking_min[1] + 32.0]),
        ..SiteElement::default()
    });
    se_id += 1;

    // EV charger station.
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "ev_charger".into(),
        tag: "EV-1".into(),
        at_ft: [parking_min[0] + 10.0, parking_min[1] + 10.0],
        min_ft: Some([parking_min[0] + 8.0, parking_min[1] + 8.0]),
        max_ft: Some([parking_min[0] + 12.0, parking_min[1] + 12.0]),
        ..SiteElement::default()
    });
    se_id += 1;

    // Monument sign at the property entrance.
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "monument_sign".into(),
        tag: "MONUMENT SIGN".into(),
        at_ft: [bldg_w / 2.0, parking_min[1] - 10.0],
        min_ft: Some([bldg_w / 2.0 - 5.0, parking_min[1] - 14.0]),
        max_ft: Some([bldg_w / 2.0 + 5.0, parking_min[1] - 6.0]),
        ..SiteElement::default()
    });
    se_id += 1;

    let _ = se_id;
}

// --- Spec book ---

fn spec_book() -> Vec<SpecItem> {
    vec![
        SpecItem {
            id: "spec-1".into(),
            division: "06".into(),
            section: "06 41 16".into(),
            title: "PLASTIC-LAMINATE-CLAD ARCHITECTURAL CABINETS".into(),
            manufacturer: "Clearwater Millwork".into(),
            model: "CWM-400 Series".into(),
            or_equal: true,
            referenced_by: vec!["V-1".into(), "V-2".into(), "WR-1".into()],
        },
        SpecItem {
            id: "spec-2".into(),
            division: "09".into(),
            section: "09 29 00".into(),
            title: "GYPSUM BOARD".into(),
            manufacturer: "Ridgewall Building Products".into(),
            model: "RW-500 Moisture Guard".into(),
            or_equal: true,
            referenced_by: vec!["wa-1".into(), "wa-2".into()],
        },
        SpecItem {
            id: "spec-3".into(),
            division: "09".into(),
            section: "09 51 13".into(),
            title: "ACOUSTICAL PANEL CEILINGS".into(),
            manufacturer: "Skyline Acoustics".into(),
            model: "SA-770 Quiet Tile".into(),
            or_equal: true,
            referenced_by: vec!["CA-1".into()],
        },
        SpecItem {
            id: "spec-4".into(),
            division: "22".into(),
            section: "22 11 16".into(),
            title: "DOMESTIC WATER PIPING".into(),
            manufacturer: "Streamline Pipe Works".into(),
            model: "SPW PressJoint CTS".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-5".into(),
            division: "22".into(),
            section: "22 42 00".into(),
            title: "COMMERCIAL PLUMBING FIXTURES".into(),
            manufacturer: "Pinnacle Plumbing Co".into(),
            model: "Summit SX-224".into(),
            or_equal: true,
            referenced_by: vec!["WC".into(), "LAV".into()],
        },
        SpecItem {
            id: "spec-6".into(),
            division: "23".into(),
            section: "23 05 93".into(),
            title: "TESTING, ADJUSTING AND BALANCING FOR HVAC".into(),
            manufacturer: "Airpoint Balancing Services".into(),
            model: "APB Certified Method".into(),
            or_equal: false,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-7".into(),
            division: "23".into(),
            section: "23 31 13".into(),
            title: "METAL DUCTS".into(),
            manufacturer: "DuctPro Industries".into(),
            model: "DP SnapLock Series".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-8".into(),
            division: "23".into(),
            section: "23 74 13".into(),
            title: "PACKAGED ROOFTOP UNITS".into(),
            manufacturer: "NorthStar Climate Systems".into(),
            model: "Explorer RTU-120".into(),
            or_equal: true,
            referenced_by: vec!["RTU-1".into(), "RTU-2".into()],
        },
        SpecItem {
            id: "spec-9".into(),
            division: "26".into(),
            section: "26 24 16".into(),
            title: "PANELBOARDS".into(),
            manufacturer: "Volterra Electric".into(),
            model: "VE-PB100A".into(),
            or_equal: true,
            referenced_by: vec!["L1".into(), "L2".into(), "P1".into(), "P2".into()],
        },
        SpecItem {
            id: "spec-10".into(),
            division: "26".into(),
            section: "26 51 13".into(),
            title: "INTERIOR LIGHTING".into(),
            manufacturer: "Brightvale Lighting".into(),
            model: "BV-T4 Recessed Series".into(),
            or_equal: true,
            referenced_by: vec!["A".into(), "B".into()],
        },
        SpecItem {
            id: "spec-11".into(),
            division: "21".into(),
            section: "21 13 13".into(),
            title: "WET-PIPE SPRINKLER SYSTEMS".into(),
            manufacturer: "GuardFlow Fire Products".into(),
            model: "GF-302 Pendent".into(),
            or_equal: true,
            referenced_by: vec!["SH-1".into()],
        },
        SpecItem {
            id: "spec-12".into(),
            division: "28".into(),
            section: "28 31 11".into(),
            title: "DIGITAL ADDRESSABLE FIRE ALARM SYSTEM".into(),
            manufacturer: "AlertWorks Safety".into(),
            model: "AW-3030 DAFCP".into(),
            or_equal: true,
            referenced_by: vec!["FACP".into()],
        },
        SpecItem {
            id: "spec-13".into(),
            division: "27".into(),
            section: "27 15 13".into(),
            title: "COMMUNICATIONS COPPER HORIZONTAL CABLING".into(),
            manufacturer: "CableCraft Telecom".into(),
            model: "CC-6A UltraLink".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-14".into(),
            division: "11".into(),
            section: "11 40 00".into(),
            title: "FOODSERVICE EQUIPMENT".into(),
            manufacturer: "ChefLine Commercial".into(),
            model: "CL-16 Warewash Series".into(),
            or_equal: true,
            referenced_by: vec!["BC-1".into()],
        },
        SpecItem {
            id: "spec-15".into(),
            division: "10".into(),
            section: "10 28 13".into(),
            title: "TOILET ACCESSORIES".into(),
            manufacturer: "CleanEdge Products".into(),
            model: "CE-800 Series".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        // --- Division 27: Communications (Q308) ---
        SpecItem {
            id: "spec-16".into(),
            division: "27".into(),
            section: "27 11 00".into(),
            title: "COMMUNICATIONS EQUIPMENT ROOM FITTINGS".into(),
            manufacturer: "RackPro Infrastructure".into(),
            model: "RP-1E Cable Manager".into(),
            or_equal: true,
            referenced_by: vec!["MDF".into(), "IDF-2".into(), "IDF-3".into(), "IDF-4".into()],
        },
        SpecItem {
            id: "spec-17".into(),
            division: "27".into(),
            section: "27 13 23".into(),
            title: "COMMUNICATIONS OPTICAL FIBER BACKBONE CABLING".into(),
            manufacturer: "FiberPath Optics".into(),
            model: "FP GelFree SM-12".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-18".into(),
            division: "27".into(),
            section: "27 41 16".into(),
            title: "INTEGRATED AUDIO-VIDEO SYSTEMS".into(),
            manufacturer: "SoundVault AV".into(),
            model: "SV-42T Ceiling Speaker".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        // --- Division 28: Electronic Safety and Security (Q308) ---
        SpecItem {
            id: "spec-19".into(),
            division: "28".into(),
            section: "28 31 43".into(),
            title: "FIRE DETECTION AND ALARM NOTIFICATION APPLIANCES".into(),
            manufacturer: "AlertWorks Safety".into(),
            model: "AW-2RH Horn/Strobe, AW-2R Strobe".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-20".into(),
            division: "28".into(),
            section: "28 31 46".into(),
            title: "SMOKE DETECTION SENSORS".into(),
            manufacturer: "AlertWorks Safety".into(),
            model: "AW-SD2 Photoelectric Smoke Detector".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-21".into(),
            division: "28".into(),
            section: "28 13 00".into(),
            title: "ACCESS CONTROL".into(),
            manufacturer: "SecureGate Systems".into(),
            model: "SG-SE R10 Reader".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-22".into(),
            division: "28".into(),
            section: "28 23 00".into(),
            title: "VIDEO SURVEILLANCE".into(),
            manufacturer: "VisionPoint Security".into(),
            model: "VP-3075 Dome".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-23".into(),
            division: "27".into(),
            section: "27 52 23".into(),
            title: "NURSE CALL AND CODE BLUE SYSTEMS".into(),
            manufacturer: "MedLink Communications".into(),
            model: "ML Responder 5".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        // --- Division 26: Equipment connections (Q308) ---
        SpecItem {
            id: "spec-24".into(),
            division: "26".into(),
            section: "26 28 16".into(),
            title: "ENCLOSED SWITCHES AND CIRCUIT BREAKERS".into(),
            manufacturer: "Volterra Electric".into(),
            model: "VE-DH Safety Switch".into(),
            or_equal: true,
            referenced_by: vec!["DS-WH1".into(), "DS-RTU1".into()],
        },
        SpecItem {
            id: "spec-25".into(),
            division: "26".into(),
            section: "26 27 26".into(),
            title: "WIRING DEVICES".into(),
            manufacturer: "WireStar Devices".into(),
            model: "WS-5362 Duplex Receptacle".into(),
            or_equal: true,
            referenced_by: vec![],
        },
    ]
}

// --- Planted gaps for Q296 ---

fn plant_gaps(t: &mut Truth, _r: &mut Rng) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let mut gid = 0;

    // 1. wall_no_tag: a wall on level 2 drawn without a type tag.
    // Find the corresponding plan_wall and mark it untagged.
    if let Some(w) = t.walls.iter().find(|w| w.level == "2" && w.assembly == "wa-1") {
        let wid = w.id.clone();
        let at = w.from_ft;
        // Mark the corresponding plan_wall as untagged.
        if let Some(pw) = t.plan_walls.iter_mut().find(|pw| pw.level == "2" && pw.a_ft == at) {
            pw.tagged = false;
            pw.tags.clear();
        }
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "wall_no_tag".into(),
            description: "Guest room demising wall has no wall type tag on the plan".into(),
            elements: vec![wid],
            level: "2".into(),
            at_ft: at,
        });
        gid += 1;
    }

    // 2. wall_tag_not_in_legend: a wall on level 3 printed with tag "WT-7" which
    //    doesn't exist in the wall type legend.
    if let Some(w) = t.walls.iter().find(|w| w.level == "3" && w.assembly == "wa-1") {
        let wid = w.id.clone();
        let at = w.from_ft;
        // Override the plan_wall's printed tag to WT-7 (the legend has WT-1..WT-6).
        if let Some(pw) = t.plan_walls.iter_mut().find(|pw| pw.level == "3" && pw.a_ft == at) {
            pw.plan_tag = Some("WT-7".into());
        }
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "wall_tag_not_in_legend".into(),
            description: "Wall references type WT-7 which is not in the wall type legend".into(),
            elements: vec![wid],
            level: "3".into(),
            at_ft: at,
        });
        gid += 1;
    }

    // 3. wall_type_conflict: a corridor wall shown as WT-2 on A-102 and WT-1 on A-202
    //    (floor plan vs RCP for the same level).
    if let Some(w) = t.walls.iter().find(|w| w.level == "2" && w.assembly == "wa-2") {
        let wid = w.id.clone();
        let at = w.from_ft;
        // The floor plan (A-102) shows WT-2 (correct). Set the enlarged_tag to WT-1 so
        // the enlarged plan prints a different tag, creating a conflict.
        if let Some(pw) = t.plan_walls.iter_mut().find(|pw| pw.level == "2" && pw.wall_type == "WT-2" && pw.a_ft == at) {
            pw.enlarged_tag = Some("WT-1".into());
        }
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "wall_type_conflict".into(),
            description: "Corridor wall shown as WT-2 on A-102 and WT-1 on A-202".into(),
            elements: vec![wid],
            level: "2".into(),
            at_ft: at,
        });
        gid += 1;
    }

    // 4. ceiling_no_height: the level 3 corridor ceiling has height_hidden = true,
    //    so the RCP draws the type symbol but no height note.
    if let Some(rm) = t.building.rooms.iter_mut().find(|r| r.level == "3" && r.name == "CORRIDOR") {
        let rm_id = rm.id.clone();
        let at = rm.min_ft;
        if let Some(c) = rm.ceiling.as_mut() {
            c.height_hidden = true;
        }
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "ceiling_no_height".into(),
            description: "Level 3 corridor RCP shows ceiling type but no height note".into(),
            elements: vec![rm_id],
            level: "3".into(),
            at_ft: at,
        });
        gid += 1;
    }

    // 5. rated_wall_stops_at_ceiling
    if let Some(w) = t.walls.iter().find(|w| w.level == "2" && w.assembly == "wa-2") {
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "rated_wall_stops_at_ceiling".into(),
            description: "1-HR rated corridor wall (WT-2) terminates at ceiling instead of continuing to structure above".into(),
            elements: vec![w.id.clone()],
            level: "2".into(),
            at_ft: w.from_ft,
        });
        gid += 1;
    }

    // 6. soffit_missing_from_section
    if let Some(s) = t.soffits.first() {
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "soffit_missing_from_section".into(),
            description: "Bathroom soffit shown on RCP but not visible in building section".into(),
            elements: vec![s.id.clone()],
            level: s.level.clone(),
            at_ft: s.min_ft,
        });
        gid += 1;
    }

    // 7. head_not_specified
    if let Some(w) = t.walls.iter().find(|w| w.assembly == "wa-4") {
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "head_not_specified".into(),
            description: "Shaft wall (WT-4, 2-HR) meets ACT ceiling with no head-of-wall detail specified".into(),
            elements: vec![w.id.clone()],
            level: w.level.clone(),
            at_ft: w.from_ft,
        });
        gid += 1;
    }

    // 8. height_assembly_conflict
    if let Some(w) = t.walls.iter().find(|w| w.level == "4" && w.assembly == "wa-1") {
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "height_assembly_conflict".into(),
            description: "Guest room wall WT-1 (height rule: SLAB TO SLAB) is drawn only to ceiling height on level 4".into(),
            elements: vec![w.id.clone()],
            level: "4".into(),
            at_ft: w.from_ft,
        });
        gid += 1;
    }

    let _ = gid;
    gaps
}

// --- Unit types (Q305) ---

impl RoomType {
    /// The unit type id of the room type.
    fn unit_type_id(self) -> &'static str {
        match self {
            Self::King => "ut-king",
            Self::DoubleQueen => "ut-dblqueen",
            Self::AdaKing => "ut-adaking",
            Self::Suite => "ut-suite",
        }
    }
}

/// The guest room type a room's name says, if it is a guest room.
fn guest_type(name: &str) -> Option<RoomType> {
    [RoomType::King, RoomType::DoubleQueen, RoomType::AdaKing, RoomType::Suite].into_iter().find(|rt| rt.name() == name)
}

/// Whether a room is a mirrored unit: a guest room on the north side of the corridor. Its
/// contents are flipped left to right (wet wall, PTAC sleeve and its receptacle, switch,
/// casework), so neighbours share wet walls.
fn mirrored(rm: &Room) -> bool {
    guest_type(&rm.name).is_some() && rm.id.contains("-N")
}

/// `x` inside `rm`, flipped across the room's middle when the room is a mirrored unit.
fn hand_x(rm: &Room, x: f64) -> f64 {
    if mirrored(rm) { rm.min_ft[0] + rm.max_ft[0] - x } else { x }
}

/// The span `a..b` along x inside `rm`, flipped (low end first) when the room is mirrored.
fn hand_span(rm: &Room, a: f64, b: f64) -> (f64, f64) {
    let (p, q) = (hand_x(rm, a), hand_x(rm, b));
    (p.min(q), p.max(q))
}

/// The value the room's unit instance overrides `key` with, if it does.
fn unit_override<'a>(t: &'a Truth, room: &str, key: &str) -> Option<&'a str> {
    t.unit_instances.iter().find(|i| i.room == room)?.overrides.get(key).map(String::as_str)
}

/// Build the unit type definitions from the room types: what `hotel_design` places in a room of
/// each type, in its normal hand (wet wall on the left, PTAC sleeve on the right).
fn unit_type_defs(room_depth_ft: f64) -> Vec<UnitType> {
    let mut types = Vec::new();
    for rt in [RoomType::King, RoomType::DoubleQueen, RoomType::AdaKing, RoomType::Suite] {
        let w = rt.width_ft();
        let ceil_h = if rt.is_ada() { 9.0 } else { 8.5 };
        let ceil_kind = if rt.is_suite() { "GYP" } else { "ACT" };
        let n_lights = ((w / 10.0).ceil() as u32).max(1);
        let n_recep = ((w / 12.0).ceil() as u32).max(1);
        let devices = BTreeMap::from([("light_fixture".to_string(), n_lights), ("switch".into(), 1), ("receptacle".into(), n_recep)]);
        let terminals = BTreeMap::from([("supply_diffuser".to_string(), 1), ("return_grille".into(), 1)]);
        let fixtures = BTreeMap::from([("water_closet".to_string(), 1), ("lavatory".into(), 1), ("bathtub".into(), 1)]);
        // A suite has a kitchenette counter where the other rooms have a wardrobe.
        let casework = BTreeMap::from([("vanity".to_string(), 1), (if rt.is_suite() { "counter" } else { "wardrobe" }.into(), 1)]);
        types.push(UnitType {
            id: rt.unit_type_id().into(),
            name: rt.name().into(),
            width_ft: w,
            depth_ft: room_depth_ft,
            ceiling_height_ft: ceil_h,
            ceiling_kind: ceil_kind.into(),
            wall_assembly: "wa-1".into(),
            ada: rt.is_ada(),
            devices,
            terminals,
            fixtures,
            casework,
            hvac_sleeve_side: "right".into(),
            wet_wall_side: "left".into(),
        });
    }
    types
}

/// A unit instance for every guest room on floors 2–4, in room order. South-side rooms keep the
/// canonical hand; north-side rooms are mirrored. Then one override of each kind.
fn unit_instances(t: &Truth) -> Vec<UnitInstance> {
    let mut units: Vec<UnitInstance> = t
        .building
        .rooms
        .iter()
        .filter(|rm| ["2", "3", "4"].contains(&rm.level.as_str()))
        .filter_map(|rm| Some((rm, guest_type(&rm.name)?)))
        .enumerate()
        .map(|(i, (rm, rt))| UnitInstance {
            id: format!("ui-{i}"),
            unit_type: rt.unit_type_id().into(),
            room: rm.id.clone(),
            level: rm.level.clone(),
            number: rm.number.clone(),
            hand: if mirrored(rm) { "mirrored" } else { "normal" }.into(),
            at_ft: rm.min_ft,
            overrides: BTreeMap::new(),
        })
        .collect();
    plant_overrides(t, &mut units);
    units
}

/// One instance of each override kind a unit schedule carries, each changing what is placed in
/// that room only:
/// - **ADA**: a king (else double queen) room on level 3 made accessible: a roll-in shower in
///   place of the tub;
/// - **end unit**: the last room on the south side of level 4, by the east stair: a receptacle
///   more on its end wall;
/// - **column**: a level 2 room with a building column on its wall (a grid line crossing it): the
///   column's furring takes the wardrobe's place.
fn plant_overrides(t: &Truth, units: &mut [UnitInstance]) {
    let room = |u: &UnitInstance| t.building.rooms.iter().find(|r| r.id == u.room);
    let free = |u: &UnitInstance| u.overrides.is_empty();
    let ada = units
        .iter()
        .position(|u| u.level == "3" && u.unit_type == "ut-king")
        .or_else(|| units.iter().position(|u| u.level == "3" && u.unit_type == "ut-dblqueen"))
        .or_else(|| units.iter().position(|u| matches!(u.unit_type.as_str(), "ut-king" | "ut-dblqueen")));
    if let Some(u) = ada.and_then(|i| units.get_mut(i)) {
        u.overrides = BTreeMap::from([
            ("ada".to_string(), "true".to_string()),
            ("fixtures.bathtub".into(), "0".into()),
            ("fixtures.shower".into(), "1".into()),
        ]);
    }
    let end = units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.level == "4" && u.room.contains("-S") && free(u))
        .max_by(|a, b| a.1.at_ft[0].total_cmp(&b.1.at_ft[0]))
        .map(|(i, _)| i);
    if let Some(i) = end {
        let n = units.get(i).and_then(room).map_or(1, |r| (((r.max_ft[0] - r.min_ft[0]) / 12.0).ceil() as u32).max(1));
        if let Some(u) = units.get_mut(i) {
            u.overrides = BTreeMap::from([("end_unit".to_string(), "true".to_string()), ("devices.receptacle".into(), (n + 1).to_string())]);
        }
    }
    let columned =
        |u: &UnitInstance| room(u).is_some_and(|r| t.building.grid.x.iter().any(|g| g.at_ft > r.min_ft[0] + 1.0 && g.at_ft < r.max_ft[0] - 1.0));
    let walled = |u: &UnitInstance| u.level == "2" && u.unit_type != "ut-suite" && free(u);
    let column = units.iter().position(|u| walled(u) && columned(u)).or_else(|| units.iter().position(walled));
    if let Some(u) = column.and_then(|i| units.get_mut(i)) {
        u.overrides = BTreeMap::from([("column".to_string(), "true".to_string()), ("casework.wardrobe".into(), "0".into())]);
    }
}

// --- Project names ---

fn project_name(r: &mut Rng) -> String {
    let names = [
        "SUMMIT PLACE HOTEL",
        "LAKEWOOD LODGE",
        "CRESTVIEW INN",
        "RIVERSIDE SUITES",
        "BLUE RIDGE HOTEL",
        "PARKSIDE HOSPITALITY",
        "CEDAR CREEK INN",
        "WINFIELD HOTEL",
    ];
    r.pick(&names).to_string()
}

// ==========================================================================
// Tests — Q305: unit types match the synthetic hotel's truth
// ==========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotel_truth_for;

    /// Four unit types are generated.
    #[test]
    fn hotel_unit_types_exist() {
        let t = hotel_truth_for(42, HotelSize::Small);
        assert_eq!(t.unit_types.len(), 4, "expected 4 unit types");
        let ids: Vec<&str> = t.unit_types.iter().map(|u| u.id.as_str()).collect();
        assert!(ids.contains(&"ut-king"));
        assert!(ids.contains(&"ut-dblqueen"));
        assert!(ids.contains(&"ut-adaking"));
        assert!(ids.contains(&"ut-suite"));
    }

    /// Every instance references a valid type and room.
    #[test]
    fn instances_reference_valid_types_and_rooms() {
        let t = hotel_truth_for(42, HotelSize::Small);
        let type_ids: Vec<&str> = t.unit_types.iter().map(|u| u.id.as_str()).collect();
        let room_ids: Vec<&str> = t.building.rooms.iter().map(|r| r.id.as_str()).collect();
        for inst in &t.unit_instances {
            assert!(type_ids.contains(&inst.unit_type.as_str()), "instance {} → unknown type {}", inst.id, inst.unit_type);
            assert!(room_ids.contains(&inst.room.as_str()), "instance {} → unknown room {}", inst.id, inst.room);
        }
    }

    /// South-side rooms are `normal`, north-side rooms are `mirrored`.
    #[test]
    fn handing_correct() {
        let t = hotel_truth_for(42, HotelSize::Small);
        for inst in &t.unit_instances {
            if inst.room.contains("-S") {
                assert_eq!(inst.hand, "normal", "south room {} should be normal", inst.room);
            } else if inst.room.contains("-N") {
                assert_eq!(inst.hand, "mirrored", "north room {} should be mirrored", inst.room);
            }
        }
    }

    /// Each guest room on floors 2–4 has exactly one unit instance.
    #[test]
    fn every_guest_room_has_instance() {
        let t = hotel_truth_for(42, HotelSize::Small);
        let guest_rooms: Vec<&str> = t
            .building
            .rooms
            .iter()
            .filter(|r| ["2", "3", "4"].contains(&r.level.as_str()))
            .filter(|r| r.name != "CORRIDOR")
            .filter(|r| {
                !r.id.contains("MECH") && !r.id.contains("ELEC") && !r.id.contains("IDF") && !r.id.contains("STAIR") && !r.id.contains("ELEV")
            })
            .map(|r| r.id.as_str())
            .collect();

        let instance_rooms: Vec<&str> = t.unit_instances.iter().map(|i| i.room.as_str()).collect();
        for rm in &guest_rooms {
            assert!(instance_rooms.contains(rm), "guest room {rm} has no unit instance");
        }
    }

    /// What a room holds of a category's kind: the type's count, or the instance's override.
    fn expected(ut: &UnitType, inst: &UnitInstance, category: &str, kind: &str) -> u32 {
        let typed = match category {
            "devices" => &ut.devices,
            "terminals" => &ut.terminals,
            "fixtures" => &ut.fixtures,
            _ => &ut.casework,
        };
        inst.overrides.get(&format!("{category}.{kind}")).and_then(|v| v.parse().ok()).unwrap_or_else(|| typed.get(kind).copied().unwrap_or(0))
    }

    /// The type's counts, with the instance's overrides, match what hotel_design placed in each
    /// room (counted from the placed objects, not the type).
    #[test]
    fn device_counts_match_truth() {
        for t in [hotel_truth_for(42, HotelSize::Small), hotel_truth_for(99, HotelSize::Full)] {
            for inst in &t.unit_instances {
                let ut = t.unit_types.iter().find(|u| u.id == inst.unit_type).expect("type");
                let placed = |category: &str, kind: &str| -> u32 {
                    let n = match category {
                        "devices" => t.elec_devices.iter().filter(|d| d.room == inst.room && d.kind == kind).count(),
                        "terminals" => t.terminals.iter().filter(|d| d.room == inst.room && d.kind == kind).count(),
                        "fixtures" => t.fixtures.iter().filter(|f| f.room == inst.room && f.kind == kind).count(),
                        _ => t.casework.iter().filter(|c| c.room == inst.room && c.kind == kind).count(),
                    };
                    n as u32
                };
                let mut keys: Vec<(String, String)> = Vec::new();
                for (category, map) in
                    [("devices", &ut.devices), ("terminals", &ut.terminals), ("fixtures", &ut.fixtures), ("casework", &ut.casework)]
                {
                    keys.extend(map.keys().map(|k| (category.to_string(), k.clone())));
                }
                keys.extend(inst.overrides.keys().filter_map(|k| k.split_once('.')).map(|(c, k)| (c.to_string(), k.to_string())));
                for (category, kind) in &keys {
                    assert_eq!(
                        placed(category, kind),
                        expected(ut, inst, category, kind),
                        "room {} type {}: {category}.{kind}",
                        inst.room,
                        inst.unit_type
                    );
                }
                // Nothing else of those categories is in the room.
                let total: u32 = keys.iter().map(|(c, k)| expected(ut, inst, c, k)).sum();
                let fixtures_cw =
                    t.fixtures.iter().filter(|f| f.room == inst.room).count() + t.casework.iter().filter(|c| c.room == inst.room).count();
                let typed_fx_cw: u32 = keys.iter().filter(|(c, _)| c == "fixtures" || c == "casework").map(|(c, k)| expected(ut, inst, c, k)).sum();
                assert_eq!(fixtures_cw as u32, typed_fx_cw, "room {}: fixtures and casework beyond its type", inst.room);
                assert!(total > 0);
            }
        }
    }

    /// One override of each kind (ADA, end unit, column in the room), on distinct instances, each
    /// changing only its own room.
    #[test]
    fn overrides_of_every_kind() {
        for t in [hotel_truth_for(42, HotelSize::Small), hotel_truth_for(7, HotelSize::Small), hotel_truth_for(99, HotelSize::Full)] {
            let with = |flag: &str| t.unit_instances.iter().filter(|i| i.overrides.get(flag).map(String::as_str) == Some("true")).collect::<Vec<_>>();
            let (ada, end, column) = (with("ada"), with("end_unit"), with("column"));
            assert_eq!((ada.len(), end.len(), column.len()), (1, 1, 1), "seed {}", t.seed);
            let (ada, end, column) = (ada[0], end[0], column[0]);
            assert!(ada.room != end.room && end.room != column.room && ada.room != column.room);
            // ADA: a shower, no tub.
            assert_eq!(t.fixtures.iter().filter(|f| f.room == ada.room && f.kind == "shower").count(), 1);
            assert_eq!(t.fixtures.iter().filter(|f| f.room == ada.room && f.kind == "bathtub").count(), 0);
            assert_eq!(t.fixtures.iter().filter(|f| f.kind == "shower").count(), 1, "only the ADA room has one");
            // End unit: one receptacle more than its type.
            let ut = t.unit_types.iter().find(|u| u.id == end.unit_type).expect("type");
            let recep = t.elec_devices.iter().filter(|d| d.room == end.room && d.kind == "receptacle").count() as u32;
            assert_eq!(recep, ut.devices.get("receptacle").copied().unwrap_or(0) + 1);
            assert!(end.level == "4" && end.room.contains("-S"));
            // Column: no wardrobe there, every other non-suite room keeps its own.
            assert_eq!(t.casework.iter().filter(|c| c.room == column.room && c.kind == "wardrobe").count(), 0);
            for i in t.unit_instances.iter().filter(|i| i.room != column.room && i.unit_type != "ut-suite") {
                assert_eq!(t.casework.iter().filter(|c| c.room == i.room && c.kind == "wardrobe").count(), 1, "{}", i.room);
            }
        }
    }

    /// Mirrored rooms really are mirrored: the wet wall (WC, lavatory, tub, vanity), the PTAC
    /// sleeve and its receptacle, the switch and the wardrobe sit on the other side of the room.
    #[test]
    fn mirrored_rooms_flip_their_contents() {
        let t = hotel_truth_for(42, HotelSize::Small);
        let side = |room: &Room, x: f64| if x < (room.min_ft[0] + room.max_ft[0]) / 2.0 { "left" } else { "right" };
        let mut seen = 0;
        for inst in &t.unit_instances {
            let room = t.building.rooms.iter().find(|r| r.id == inst.room).expect("room");
            let ut = t.unit_types.iter().find(|u| u.id == inst.unit_type).expect("type");
            let flip = |s: &str| {
                if inst.hand == "mirrored" {
                    if s == "left" { "right" } else { "left" }
                } else if s == "left" {
                    "left"
                } else {
                    "right"
                }
            };
            let wc = t.fixtures.iter().find(|f| f.room == inst.room && f.kind == "water_closet").expect("wc");
            assert_eq!(side(room, wc.at_ft[0]), flip(&ut.wet_wall_side), "{} wet wall", inst.room);
            let ptac = t.equip_connections.iter().find(|e| e.room == inst.room && e.kind == "ptac").expect("ptac");
            assert_eq!(side(room, ptac.at_ft[0]), flip(&ut.hvac_sleeve_side), "{} PTAC sleeve", inst.room);
            assert_eq!(ptac.connection_type, "receptacle");
            let sw = t.elec_devices.iter().find(|d| d.room == inst.room && d.kind == "switch").expect("switch");
            assert_eq!(side(room, sw.at_ft[0]), flip("left"), "{} switch", inst.room);
            let vanity = t.casework.iter().find(|c| c.room == inst.room && c.kind == "vanity").expect("vanity");
            assert_eq!(side(room, (vanity.min_ft[0] + vanity.max_ft[0]) / 2.0), flip(&ut.wet_wall_side), "{} vanity", inst.room);
            seen += usize::from(inst.hand == "mirrored");
        }
        assert!(seen > 0, "some rooms are mirrored");
    }

    /// Types have the correct dimensions per variant.
    #[test]
    fn type_dimensions_correct() {
        let t = hotel_truth_for(42, HotelSize::Small);

        let king = t.unit_types.iter().find(|u| u.id == "ut-king").expect("king");
        assert!((king.width_ft - 13.0).abs() < 0.01);
        assert!((king.ceiling_height_ft - 8.5).abs() < 0.01);
        assert_eq!(king.ceiling_kind, "ACT");
        assert!(!king.ada);

        let dq = t.unit_types.iter().find(|u| u.id == "ut-dblqueen").expect("dblqueen");
        assert!((dq.width_ft - 14.0).abs() < 0.01);

        let ada = t.unit_types.iter().find(|u| u.id == "ut-adaking").expect("adaking");
        assert!((ada.width_ft - 15.0).abs() < 0.01);
        assert!((ada.ceiling_height_ft - 9.0).abs() < 0.01);
        assert!(ada.ada);

        let suite = t.unit_types.iter().find(|u| u.id == "ut-suite").expect("suite");
        assert!((suite.width_ft - 26.0).abs() < 0.01);
        assert_eq!(suite.ceiling_kind, "GYP");
    }

    /// Instance position matches the room's corner.
    #[test]
    fn instance_position_matches_room() {
        let t = hotel_truth_for(42, HotelSize::Small);
        for inst in &t.unit_instances {
            let rm = match t.building.rooms.iter().find(|r| r.id == inst.room) {
                Some(r) => r,
                None => continue,
            };
            assert!((inst.at_ft[0] - rm.min_ft[0]).abs() < 0.01, "instance {} x={} vs room min_x={}", inst.id, inst.at_ft[0], rm.min_ft[0]);
        }
    }

    /// Full-size hotel: many instances, all valid.
    #[test]
    fn full_hotel_unit_coverage() {
        let t = hotel_truth_for(99, HotelSize::Full);
        assert_eq!(t.unit_types.len(), 4);
        assert!(t.unit_instances.len() > 20, "full hotel should have many instances, got {}", t.unit_instances.len());
        let room_ids: Vec<&str> = t.building.rooms.iter().map(|r| r.id.as_str()).collect();
        for inst in &t.unit_instances {
            assert!(room_ids.contains(&inst.room.as_str()), "instance {} room {} not found", inst.id, inst.room);
        }
    }

    /// Deterministic: same seed → same types and instances.
    #[test]
    fn deterministic() {
        let t1 = hotel_truth_for(42, HotelSize::Small);
        let t2 = hotel_truth_for(42, HotelSize::Small);
        assert_eq!(t1.unit_types.len(), t2.unit_types.len());
        assert_eq!(t1.unit_instances.len(), t2.unit_instances.len());
        for (a, b) in t1.unit_instances.iter().zip(t2.unit_instances.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.room, b.room);
            assert_eq!(a.hand, b.hand);
        }
    }

    /// Takeoff by type × count: totals are correct.
    #[test]
    fn takeoff_by_type() {
        let t = hotel_truth_for(42, HotelSize::Small);
        for ut in &t.unit_types {
            let count = t.unit_instances.iter().filter(|i| i.unit_type == ut.id).count() as u32;
            // Total lights = per-type × count
            let expected_lights = ut.devices.get("light_fixture").copied().unwrap_or(0) * count;
            let actual_lights: u32 = t
                .unit_instances
                .iter()
                .filter(|i| i.unit_type == ut.id)
                .map(|i| t.elec_devices.iter().filter(|d| d.room == i.room && d.kind == "light_fixture").count() as u32)
                .sum();
            assert_eq!(actual_lights, expected_lights, "type {}: expected {} total lights, got {}", ut.id, expected_lights, actual_lights);
        }
    }

    /// Unit schedule summary matches instance counts.
    #[test]
    fn schedule_summary() {
        let t = hotel_truth_for(42, HotelSize::Small);
        for ut in &t.unit_types {
            let total = t.unit_instances.iter().filter(|i| i.unit_type == ut.id).count();
            let normal = t.unit_instances.iter().filter(|i| i.unit_type == ut.id && i.hand == "normal").count();
            let mirrored = t.unit_instances.iter().filter(|i| i.unit_type == ut.id && i.hand == "mirrored").count();
            assert_eq!(total, normal + mirrored, "type {}: total {} != normal {} + mirrored {}", ut.id, total, normal, mirrored);
        }
    }
}
