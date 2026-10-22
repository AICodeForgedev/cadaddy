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
            ceiling: Some(Ceiling { height_ft: 8.5, kind: "GYP".into(), tag: "GYP @ 8'-6\"".into(), grid_ft: None }),
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
                }),
                plenum_ft: None,
            });
            x += w;
            room_num += 1;
        }
    }

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

    // --- Ceiling assemblies ---
    t.ceiling_assemblies = ceiling_assemblies();

    // --- Soffits (bulkheads at bathroom vanities in guest rooms) ---
    let mut soffit_id = 0;
    for rm in &t.building.rooms {
        if rm.name.contains("KING") || rm.name.contains("QUEEN") || rm.name.contains("SUITE") {
            let vanity_depth = 2.5;
            t.soffits.push(SoffitInstance {
                id: format!("soffit-{soffit_id}"),
                assembly: "CA-2".into(),
                level: rm.level.clone(),
                room: rm.id.clone(),
                min_ft: [rm.min_ft[0] + 0.5, rm.min_ft[1] + 0.5],
                max_ft: [rm.min_ft[0] + 5.0, rm.min_ft[1] + vanity_depth + 0.5],
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
                t.casework.push(CaseworkItem {
                    id: format!("cw-{cw_id}"),
                    kind: "vanity".into(),
                    tag: "V-1".into(),
                    level: rm.level.clone(),
                    room: rm.id.clone(),
                    min_ft: [rm.min_ft[0] + 0.5, rm.min_ft[1] + 0.5],
                    max_ft: [rm.min_ft[0] + 4.0, rm.min_ft[1] + 2.0],
                    height_in: 34.0,
                    depth_in: 22.0,
                    spec_section: Some("06 41 16".into()),
                });
                cw_id += 1;
                // Wardrobe
                t.casework.push(CaseworkItem {
                    id: format!("cw-{cw_id}"),
                    kind: "wardrobe".into(),
                    tag: "WR-1".into(),
                    level: rm.level.clone(),
                    room: rm.id.clone(),
                    min_ft: [rm.max_ft[0] - 4.0, rm.min_ft[1] + 0.5],
                    max_ft: [rm.max_ft[0] - 0.5, rm.min_ft[1] + 2.5],
                    height_in: 84.0,
                    depth_in: 24.0,
                    spec_section: Some("06 41 16".into()),
                });
                cw_id += 1;
            }
            "SUITE" => {
                t.casework.push(CaseworkItem {
                    id: format!("cw-{cw_id}"),
                    kind: "vanity".into(),
                    tag: "V-2".into(),
                    level: rm.level.clone(),
                    room: rm.id.clone(),
                    min_ft: [rm.min_ft[0] + 0.5, rm.min_ft[1] + 0.5],
                    max_ft: [rm.min_ft[0] + 5.0, rm.min_ft[1] + 2.5],
                    height_in: 34.0,
                    depth_in: 22.0,
                    spec_section: Some("06 41 16".into()),
                });
                cw_id += 1;
                // Kitchenette counter
                t.casework.push(CaseworkItem {
                    id: format!("cw-{cw_id}"),
                    kind: "counter".into(),
                    tag: "KC-1".into(),
                    level: rm.level.clone(),
                    room: rm.id.clone(),
                    min_ft: [rm.min_ft[0] + 8.0, rm.min_ft[1] + 0.5],
                    max_ft: [rm.min_ft[0] + 14.0, rm.min_ft[1] + 2.5],
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
            let cx = (rm.min_ft[0] + rm.max_ft[0]) / 2.0 + 3.0;
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

        // Plumbing fixtures in guest rooms (water closets + lavatories)
        for rm in &level_rooms {
            if rm.name.contains("KING") || rm.name.contains("QUEEN") || rm.name.contains("SUITE") || rm.name.contains("ADA") {
                // WC near wet wall
                t.fixtures.push(Fixture {
                    id: format!("fix-{fix_id}"),
                    tag: "WC".into(),
                    kind: "water_closet".into(),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [rm.min_ft[0] + 2.5, rm.min_ft[1] + 1.0, elev],
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
                    at_ft: [rm.min_ft[0] + 1.5, rm.min_ft[1] + 1.0, elev + 2.83],
                    nodes: vec![],
                    wall: None,
                });
                fix_id += 1;
                // Tub/shower
                t.fixtures.push(Fixture {
                    id: format!("fix-{fix_id}"),
                    tag: "TUB".into(),
                    kind: "bathtub".into(),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [rm.min_ft[0] + 4.5, rm.min_ft[1] + 1.0, elev],
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

    // --- Site elements ---
    hotel_site(&mut t, building_width_ft, building_depth_ft, r);

    // --- Spec book ---
    t.spec_items = spec_book();

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
        sheet: String::new(),
        room: Some("rm-2-S1".into()),
    });

    t.sections = vec![
        SectionView {
            id: "sect-1".into(),
            name: "BUILDING SECTION A".into(),
            cut_from_ft: [building_width_ft / 3.0, 0.0],
            cut_to_ft: [building_width_ft / 3.0, building_depth_ft],
            looking: "west".into(),
            sheet: String::new(),
        },
        SectionView {
            id: "sect-2".into(),
            name: "BUILDING SECTION B".into(),
            cut_from_ft: [0.0, building_depth_ft / 2.0],
            cut_to_ft: [building_width_ft, building_depth_ft / 2.0],
            looking: "north".into(),
            sheet: String::new(),
        },
    ];

    // --- Gaps for Q296 ---
    t.gaps = plant_gaps(&t, r);

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
        } else if i == count - 1 {
            RoomType::Suite // last room is a suite
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
        ceiling: Some(Ceiling { height_ft: 10.0, kind: "GYP".into(), tag: "GYP @ 10'-0\"".into(), grid_ft: None }),
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
        ceiling: Some(Ceiling { height_ft: 12.0, kind: "GYP".into(), tag: "GYP @ 12'-0\"".into(), grid_ft: None }),
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
        ceiling: Some(Ceiling { height_ft: 10.0, kind: "GYP".into(), tag: "GYP @ 10'-0\"".into(), grid_ft: None }),
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
        ceiling: Some(Ceiling { height_ft: 10.0, kind: "ACT".into(), tag: "ACT @ 10'-0\"".into(), grid_ft: None }),
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
            ceiling: Some(Ceiling { height_ft: 10.0, kind: "ACT".into(), tag: "ACT @ 10'-0\"".into(), grid_ft: None }),
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
        ceiling: Some(Ceiling { height_ft: 10.0, kind: "GYP".into(), tag: "GYP @ 10'-0\"".into(), grid_ft: None }),
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
        ceiling: Some(Ceiling { height_ft: 10.0, kind: "GYP".into(), tag: "GYP @ 10'-0\"".into(), grid_ft: None }),
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
        ceiling: Some(Ceiling { height_ft: 12.0, kind: "GYP".into(), tag: "GYP @ 12'-0\"".into(), grid_ft: None }),
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
                at_ft: [rm.min_ft[0] + 0.5, rm.max_ft[1] - 0.5, elev + 4.0],
                drawn_ft: [rm.min_ft[0] + 0.5 + r.below(2) as f64 * 0.3, rm.max_ft[1] - 0.5],
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
            let n_recep = ((w / 12.0).ceil() as usize).max(1);
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
                    drawn_ft: [rx + r.below(2) as f64 * 0.3, rm.min_ft[1] + 0.5],
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
        circuit: "SLC-1".into(),
        sheet: String::new(),
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
        circuit: "SLC-1".into(),
        sheet: String::new(),
    });
    fa_id += 1;

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
                circuit: "SLC-1".into(),
                sheet: String::new(),
            });
            fa_id += 1;
            sx += 30.0;
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
                    circuit: "SLC-1".into(),
                    sheet: String::new(),
                });
                fa_id += 1;
            }
        }

        // Horn/strobe in corridor
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
                circuit: "NAC-1".into(),
                sheet: String::new(),
            });
            fa_id += 1;
            hx += 75.0;
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
                circuit: "SLC-1".into(),
                sheet: String::new(),
            });
            fa_id += 1;
        }
    }
}

// --- Telecom ---

fn hotel_telecom(t: &mut Truth, r: &mut Rng) {
    let mut tc_id = 0;

    for lv in ["1", "2", "3", "4"] {
        let elev = t.building.levels.iter().find(|l| l.code == lv).map(|l| l.elevation_ft).unwrap_or(0.0);

        for rm in &t.building.rooms.clone() {
            if rm.level != lv {
                continue;
            }

            // Data outlet in every room
            t.telecom_devices.push(TelecomDevice {
                id: format!("tc-{tc_id}"),
                kind: "data_outlet".into(),
                tag: format!("D-{}", tc_id + 1),
                level: lv.into(),
                room: rm.id.clone(),
                at_ft: [rm.min_ft[0] + 2.0, rm.min_ft[1] + 0.5, elev + 1.5],
                mounting_in: 18.0,
                sheet: String::new(),
            });
            tc_id += 1;

            // TV outlet in guest rooms
            if rm.name.contains("KING") || rm.name.contains("QUEEN") || rm.name.contains("SUITE") {
                t.telecom_devices.push(TelecomDevice {
                    id: format!("tc-{tc_id}"),
                    kind: "tv_outlet".into(),
                    tag: format!("TV-{}", tc_id + 1),
                    level: lv.into(),
                    room: rm.id.clone(),
                    at_ft: [(rm.min_ft[0] + rm.max_ft[0]) / 2.0, rm.max_ft[1] - 0.5, elev + 4.0],
                    mounting_in: 48.0,
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
                sheet: String::new(),
            });
            tc_id += 1;
            ax += 50.0;
        }

        // Cameras in lobby and corridors
        if lv == "1" || r.chance(0.5) {
            t.telecom_devices.push(TelecomDevice {
                id: format!("tc-{tc_id}"),
                kind: "camera".into(),
                tag: format!("CAM-{}", tc_id + 1),
                level: lv.into(),
                room: format!("rm-{lv}-CORR"),
                at_ft: [10.0, corr_y, elev + ceil_h],
                mounting_in: 0.0,
                sheet: String::new(),
            });
            tc_id += 1;
        }
    }
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
        depth_ft: None,
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
            depth_ft: None,
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
            depth_ft: None,
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
            depth_ft: None,
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
        depth_ft: None,
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
        depth_ft: None,
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
        depth_ft: None,
    });
    se_id += 1;

    // Underground utilities
    for (kind, depth, x_off) in [
        ("underground_water", 4.0, -5.0),
        ("underground_sewer", 8.0, -10.0),
        ("underground_electric", 3.0, 5.0),
        ("underground_gas", 3.0, 10.0),
        ("underground_storm", 6.0, -15.0),
        ("underground_telecom", 3.0, 15.0),
    ] {
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: kind.into(),
            tag: kind.replace("underground_", "").to_uppercase(),
            at_ft: [bldg_w / 2.0 + x_off, -setback - 30.0],
            min_ft: Some([bldg_w / 2.0 + x_off - 0.5, -setback - 60.0]),
            max_ft: Some([bldg_w / 2.0 + x_off + 0.5, -5.0]),
            depth_ft: Some(depth),
        });
        se_id += 1;
    }

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
            manufacturer: "National Gypsum".into(),
            model: "Gold Bond XP".into(),
            or_equal: true,
            referenced_by: vec!["wa-1".into(), "wa-2".into()],
        },
        SpecItem {
            id: "spec-3".into(),
            division: "09".into(),
            section: "09 51 13".into(),
            title: "ACOUSTICAL PANEL CEILINGS".into(),
            manufacturer: "Armstrong Ceiling Solutions".into(),
            model: "Cortega 770".into(),
            or_equal: true,
            referenced_by: vec!["CA-1".into()],
        },
        SpecItem {
            id: "spec-4".into(),
            division: "22".into(),
            section: "22 11 16".into(),
            title: "DOMESTIC WATER PIPING".into(),
            manufacturer: "Viega".into(),
            model: "ProPress CTS".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-5".into(),
            division: "22".into(),
            section: "22 42 00".into(),
            title: "COMMERCIAL PLUMBING FIXTURES".into(),
            manufacturer: "Kohler".into(),
            model: "Highline K-25224".into(),
            or_equal: true,
            referenced_by: vec!["WC".into(), "LAV".into()],
        },
        SpecItem {
            id: "spec-6".into(),
            division: "23".into(),
            section: "23 05 93".into(),
            title: "TESTING, ADJUSTING AND BALANCING FOR HVAC".into(),
            manufacturer: "N/A".into(),
            model: "N/A".into(),
            or_equal: false,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-7".into(),
            division: "23".into(),
            section: "23 31 13".into(),
            title: "METAL DUCTS".into(),
            manufacturer: "Lindab".into(),
            model: "Safe Click".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-8".into(),
            division: "23".into(),
            section: "23 74 13".into(),
            title: "PACKAGED ROOFTOP UNITS".into(),
            manufacturer: "Trane".into(),
            model: "Voyager YCD120".into(),
            or_equal: true,
            referenced_by: vec!["RTU-1".into(), "RTU-2".into()],
        },
        SpecItem {
            id: "spec-9".into(),
            division: "26".into(),
            section: "26 24 16".into(),
            title: "PANELBOARDS".into(),
            manufacturer: "Eaton".into(),
            model: "PRL1A".into(),
            or_equal: true,
            referenced_by: vec!["L1".into(), "L2".into(), "P1".into(), "P2".into()],
        },
        SpecItem {
            id: "spec-10".into(),
            division: "26".into(),
            section: "26 51 13".into(),
            title: "INTERIOR LIGHTING".into(),
            manufacturer: "Lithonia Lighting".into(),
            model: "BLT4 Series".into(),
            or_equal: true,
            referenced_by: vec!["A".into(), "B".into()],
        },
        SpecItem {
            id: "spec-11".into(),
            division: "21".into(),
            section: "21 13 13".into(),
            title: "WET-PIPE SPRINKLER SYSTEMS".into(),
            manufacturer: "Viking".into(),
            model: "VK302 Pendent".into(),
            or_equal: true,
            referenced_by: vec!["SH-1".into()],
        },
        SpecItem {
            id: "spec-12".into(),
            division: "28".into(),
            section: "28 31 11".into(),
            title: "DIGITAL ADDRESSABLE FIRE ALARM SYSTEM".into(),
            manufacturer: "Notifier by Honeywell".into(),
            model: "NFS2-3030".into(),
            or_equal: true,
            referenced_by: vec!["FACP".into()],
        },
        SpecItem {
            id: "spec-13".into(),
            division: "27".into(),
            section: "27 15 13".into(),
            title: "COMMUNICATIONS COPPER HORIZONTAL CABLING".into(),
            manufacturer: "Belden".into(),
            model: "CAT6A 10GXS13".into(),
            or_equal: true,
            referenced_by: vec![],
        },
        SpecItem {
            id: "spec-14".into(),
            division: "11".into(),
            section: "11 40 00".into(),
            title: "FOODSERVICE EQUIPMENT".into(),
            manufacturer: "Hobart".into(),
            model: "AM16 Series".into(),
            or_equal: true,
            referenced_by: vec!["BC-1".into()],
        },
        SpecItem {
            id: "spec-15".into(),
            division: "10".into(),
            section: "10 28 13".into(),
            title: "TOILET ACCESSORIES".into(),
            manufacturer: "Bobrick".into(),
            model: "B-Series".into(),
            or_equal: true,
            referenced_by: vec![],
        },
    ]
}

// --- Planted gaps for Q296 ---

fn plant_gaps(t: &Truth, _r: &mut Rng) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let mut gid = 0;

    // 1. A wall with no type tag: pick a wall on level 2 and leave its assembly blank
    if let Some(w) = t.walls.iter().find(|w| w.level == "2" && w.assembly == "wa-1") {
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "wall_no_tag".into(),
            description: "Guest room demising wall has no wall type tag on the plan".into(),
            elements: vec![w.id.clone()],
            level: "2".into(),
            at_ft: w.from_ft,
        });
        gid += 1;
    }

    // 2. A wall tag not in the legend: reference a non-existent assembly
    if let Some(w) = t.walls.iter().find(|w| w.level == "3" && w.assembly == "wa-1") {
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "wall_tag_not_in_legend".into(),
            description: "Wall references type WT-7 which is not in the wall type legend".into(),
            elements: vec![w.id.clone()],
            level: "3".into(),
            at_ft: w.from_ft,
        });
        gid += 1;
    }

    // 3. Same wall shown with different types on two sheets
    if t.walls.len() > 20 {
        let w = &t.walls[10];
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "wall_type_conflict".into(),
            description: "Corridor wall shown as WT-2 on A-201 and WT-1 on A-301".into(),
            elements: vec![w.id.clone()],
            level: w.level.clone(),
            at_ft: w.from_ft,
        });
        gid += 1;
    }

    // 4. Ceiling with no height
    if let Some(rm) = t.building.rooms.iter().find(|r| r.level == "3" && r.name == "CORRIDOR") {
        gaps.push(Gap {
            id: format!("gap-{gid}"),
            kind: "ceiling_no_height".into(),
            description: "Level 3 corridor RCP shows ceiling type but no height note".into(),
            elements: vec![rm.id.clone()],
            level: "3".into(),
            at_ft: rm.min_ft,
        });
        gid += 1;
    }

    // 5. Rated wall stops at ceiling (should go slab to slab)
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

    // 6. Soffit shown on RCP but missing from section
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

    // 7. Head condition not specified at rated wall / ceiling intersection
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

    // 8. Height/assembly conflict: wall height rule says SLAB TO SLAB but drawn to ceiling
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
