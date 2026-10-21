//! Clashes computed from the truth's geometry: every pair of modelled elements that overlap (hard
//! clashes; runs or equipment reaching into a beam or column), runs or equipment hanging below
//! the ceiling of the room they are in, and in-wall clashes where electrical devices share a
//! stud cavity (or are within clearance) with plumbing rough-in stubs or risers.

use crate::design::{depth_in, pipe_od, width_in};
use crate::truth::{Clash, ClearRange, PlacementZone, Truth};
use std::collections::BTreeSet;

/// Overlaps smaller than this (feet) are touching, not clashing.
const TOUCH: f64 = 1.0 / 96.0;

#[derive(Clone, Debug)]
struct Item {
    id: String,
    min: [f64; 3],
    max: [f64; 3],
    /// Structure (beams, columns) clashes are `into_structure`.
    structure: bool,
    /// Nodes it is connected through: items sharing one don't clash.
    nodes: Vec<String>,
}

fn items(t: &Truth) -> Vec<Item> {
    let mut out = Vec::new();
    for s in &t.segments {
        let (w, d) = (width_in(&s.shape, s.size_in) / 24.0, depth_in(&s.shape, s.size_in) / 24.0);
        let [a, b] = s.path_ft;
        let along_x = (a[0] - b[0]).abs() > (a[1] - b[1]).abs();
        let (hx, hy, hz) = if s.vertical {
            // A vertical rectangular duct's size is [along x, along y].
            if s.shape == "rect" { (s.size_in[0] / 24.0, s.size_in[1] / 24.0, 0.0) } else { (w, w, 0.0) }
        } else if along_x {
            (0.0, w, d)
        } else {
            (w, 0.0, d)
        };
        out.push(Item {
            id: s.id.clone(),
            min: [a[0].min(b[0]) - hx, a[1].min(b[1]) - hy, a[2].min(b[2]) - hz],
            max: [a[0].max(b[0]) + hx, a[1].max(b[1]) + hy, a[2].max(b[2]) + hz],
            structure: false,
            nodes: vec![s.from.clone(), s.to.clone()],
        });
    }
    for e in &t.equipment {
        out.push(Item { id: e.id.clone(), min: e.min_ft, max: e.max_ft, structure: false, nodes: e.nodes.clone() });
    }
    for b in &t.building.beams {
        let hf = b.flange_in / 24.0;
        let along_x = (b.from_ft[0] - b.to_ft[0]).abs() > (b.from_ft[1] - b.to_ft[1]).abs();
        let (hx, hy) = if along_x { (0.0, hf) } else { (hf, 0.0) };
        out.push(Item {
            id: b.id.clone(),
            min: [b.from_ft[0].min(b.to_ft[0]) - hx, b.from_ft[1].min(b.to_ft[1]) - hy, b.tos_ft - b.depth_in / 12.0],
            max: [b.from_ft[0].max(b.to_ft[0]) + hx, b.from_ft[1].max(b.to_ft[1]) + hy, b.tos_ft],
            structure: true,
            nodes: Vec::new(),
        });
    }
    for c in &t.building.columns {
        let (hx, hy) = (c.footprint_in[0] / 24.0, c.footprint_in[1] / 24.0);
        out.push(Item {
            id: c.id.clone(),
            min: [c.at_ft[0] - hx, c.at_ft[1] - hy, c.bottom_ft],
            max: [c.at_ft[0] + hx, c.at_ft[1] + hy, c.top_ft],
            structure: true,
            nodes: Vec::new(),
        });
    }
    out
}

/// The overlap of two boxes, if every side of it is more than touching.
fn overlap(a: &Item, b: &Item) -> Option<([f64; 3], [f64; 3])> {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for k in 0..3 {
        lo[k] = a.min[k].max(b.min[k]);
        hi[k] = a.max[k].min(b.max[k]);
        if hi[k] - lo[k] <= TOUCH {
            return None;
        }
    }
    Some((lo, hi))
}

/// Every clash, `planted` set for those involving an element in `planted`.
pub fn clashes(t: &Truth, planted: &[String]) -> Vec<Clash> {
    let all = items(t);
    let planted: BTreeSet<&str> = planted.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    let push = |out: &mut Vec<Clash>, kind: &str, a: &str, b: &str, lo: [f64; 3], hi: [f64; 3]| {
        let depth = (0..3).map(|k| hi[k] - lo[k]).fold(f64::INFINITY, f64::min);
        out.push(Clash {
            id: String::new(),
            kind: kind.into(),
            a: a.into(),
            b: b.into(),
            at_ft: [0, 1, 2].map(|k| ((lo[k] + hi[k]) / 2.0 * 1e4).round() / 1e4),
            depth_in: (depth * 12.0 * 100.0).round() / 100.0,
            planted: planted.contains(a) || planted.contains(b),
            description: None,
            wall: None,
        });
    };
    for (i, a) in all.iter().enumerate() {
        for b in all.iter().skip(i + 1) {
            if a.structure && b.structure {
                continue;
            }
            if a.nodes.iter().any(|n| b.nodes.contains(n)) {
                continue;
            }
            if let Some((lo, hi)) = overlap(a, b) {
                let kind = if a.structure || b.structure { "into_structure" } else { "hard" };
                push(&mut out, kind, &a.id, &b.id, lo, hi);
            }
        }
    }
    // Below the ceiling: horizontal runs and hung equipment over a room with a ceiling.
    let elev = |code: &str| t.level(code).map_or(0.0, |l| l.elevation_ft);
    for room in &t.building.rooms {
        let Some(c) = &room.ceiling else { continue };
        let floor = elev(&room.level);
        let clg = floor + c.height_ft;
        let inside = |min: [f64; 3], max: [f64; 3]| {
            min[0] < room.max_ft[0] - TOUCH && max[0] > room.min_ft[0] + TOUCH && min[1] < room.max_ft[1] - TOUCH && max[1] > room.min_ft[1] + TOUCH
        };
        for s in t.segments.iter().filter(|s| !s.vertical && !s.below_floor && s.level == room.level) {
            let Some(it) = all.iter().find(|i| i.id == s.id) else { continue };
            if inside(it.min, it.max) && it.min[2] < clg - TOUCH && it.max[2] > floor {
                let lo = [it.min[0].max(room.min_ft[0]), it.min[1].max(room.min_ft[1]), it.min[2]];
                let hi = [it.max[0].min(room.max_ft[0]), it.max[1].min(room.max_ft[1]), clg];
                push(&mut out, "below_ceiling", &s.id, &room.id, lo, hi);
            }
        }
        for e in t.equipment.iter().filter(|e| e.level == room.level && e.min_ft[2] > floor + 0.5) {
            if inside(e.min_ft, e.max_ft) && e.min_ft[2] < clg - TOUCH {
                let lo = [e.min_ft[0].max(room.min_ft[0]), e.min_ft[1].max(room.min_ft[1]), e.min_ft[2]];
                let hi = [e.max_ft[0].min(room.max_ft[0]), e.max_ft[1].min(room.max_ft[1]), clg];
                push(&mut out, "below_ceiling", &e.id, &room.id, lo, hi);
            }
        }
    }
    // In-wall clashes: devices sharing a stud cavity with rough-in stubs or within clearance.
    let clearance_in = 3.0;
    let clearance_cavities = (clearance_in / 16.0_f64).ceil() as usize;
    for dev in &t.devices {
        let Some(wall) = t.wall(&dev.wall) else { continue };
        for ri in t.rough_ins.iter().filter(|ri| ri.wall == dev.wall) {
            // Same stud cavity or within clearance_cavities.
            let dev_cav = dev.stud_cavity;
            let ri_cav = ri.stud_cavity;
            let cav_dist = dev_cav.abs_diff(ri_cav);
            if cav_dist > clearance_cavities {
                continue;
            }
            // They must overlap vertically (device box vs stub height).
            let dev_bot = dev.aff_in - dev.box_in[1] / 2.0;
            let dev_top = dev.aff_in + dev.box_in[1] / 2.0;
            let stub_od = pipe_od(ri.size_in);
            let ri_bot = ri.aff_in - stub_od / 2.0;
            let ri_top = ri.aff_in + stub_od / 2.0;
            // Overlap with clearance buffer.
            if dev_top + clearance_in < ri_bot || ri_top + clearance_in < dev_bot {
                continue;
            }
            let elev = t.level(&dev.level).map_or(0.0, |l| l.elevation_ft);
            let z_mid = elev + (dev.aff_in + ri.aff_in) / 2.0 / 12.0;
            let x_mid = (dev.at_ft[0] + ri.at_ft[0]) / 2.0;
            let y_mid = (dev.at_ft[1] + ri.at_ft[1]) / 2.0;

            // Distance in inches along the wall between their centres.
            let dev_along = project_on_wall(wall, dev.at_ft);
            let ri_along = project_on_wall(wall, [ri.at_ft[0], ri.at_ft[1]]);
            let horiz_dist = (dev_along - ri_along).abs();

            let fixture = t.fixtures.iter().find(|f| f.id == ri.fixture);
            let room = fixture.map_or("", |f| f.room.as_str());
            let fix_kind = fixture.map_or("fixture", |f| f.kind.as_str());
            let desc = format!(
                "{} {} +{}\" AFF in the wall behind {} ({}): {:.0}\" from the {} stub",
                if dev.kind == "receptacle" { "Receptacle" } else { "Switch" },
                dev.tag,
                dev.aff_in as i64,
                fix_kind,
                room,
                horiz_dist,
                ri.kind,
            );

            out.push(Clash {
                id: String::new(),
                kind: "in_wall".into(),
                a: dev.id.clone(),
                b: ri.id.clone(),
                at_ft: [r4(x_mid), r4(y_mid), r4(z_mid)],
                depth_in: r2((clearance_in - horiz_dist).max(stub_od).min(clearance_in + stub_od)),
                planted: planted.contains(dev.id.as_str()) || planted.contains(ri.id.as_str()),
                description: Some(desc),
                wall: Some(dev.wall.clone()),
            });
        }
    }

    for (k, c) in out.iter_mut().enumerate() {
        c.id = format!("X{:03}", k + 1);
    }
    out
}

fn r4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Project a point onto a wall, returning inches from wall start.
fn project_on_wall(w: &crate::truth::Wall, pt: [f64; 2]) -> f64 {
    let dx = w.to_ft[0] - w.from_ft[0];
    let dy = w.to_ft[1] - w.from_ft[1];
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-18 {
        return 0.0;
    }
    let t = ((pt[0] - w.from_ft[0]) * dx + (pt[1] - w.from_ft[1]) * dy) / len_sq;
    t * len_sq.sqrt() * 12.0
}

/// Compute placement zones for devices that are not fixed, analyzing which positions along
/// their wall are clear of in-wall conflicts.
pub fn placement_zones(t: &Truth) -> Vec<PlacementZone> {
    let clearance_in = 3.0;
    let mut zones = Vec::new();

    for dev in t.devices.iter().filter(|d| !d.fixed) {
        let Some(wall) = t.wall(&dev.wall) else { continue };
        let wall_len = wall_length_in(wall);
        if wall_len < 1.0 {
            continue;
        }

        // The zone is the wall segment within the room (we use the full wall for simplicity
        // and clip to a reasonable range around the device's nominal position).
        let dev_along = project_on_wall(wall, dev.at_ft);
        // Zone: ±3 ft (36") from the device's nominal position, clamped to the wall.
        let zone_start = (dev_along - 36.0).max(0.0);
        let zone_end = (dev_along + 36.0).min(wall_len);

        // Collect all rough-in stubs in this wall with their along-wall positions and height ranges.
        let stubs: Vec<(f64, f64, f64, String)> = t
            .rough_ins
            .iter()
            .filter(|ri| ri.wall == dev.wall)
            .map(|ri| {
                let along = project_on_wall(wall, [ri.at_ft[0], ri.at_ft[1]]);
                let od = pipe_od(ri.size_in);
                (along, ri.aff_in - od / 2.0, ri.aff_in + od / 2.0, ri.id.clone())
            })
            .collect();

        let dev_bot = dev.aff_in - dev.box_in[1] / 2.0;
        let dev_top = dev.aff_in + dev.box_in[1] / 2.0;
        let box_half_w = dev.box_in[0] / 2.0;

        // For each position in the zone, check if the device box (centred there) would conflict.
        // We check at 1" resolution.
        let mut blocked = Vec::new(); // (start_in, end_in) blocked ranges
        for &(stub_along, stub_bot, stub_top, ref _rid) in &stubs {
            // Vertical overlap check.
            if dev_top + clearance_in < stub_bot || stub_top + clearance_in < dev_bot {
                continue;
            }
            // The device box at position p conflicts if |p - stub_along| < box_half_w + clearance.
            let excl_half = box_half_w + clearance_in + pipe_od(0.5); // conservative
            blocked.push((stub_along - excl_half, stub_along + excl_half));
        }

        // Merge blocked ranges.
        blocked.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (s, e) in &blocked {
            if let Some(last) = merged.last_mut()
                && *s <= last.1
            {
                last.1 = last.1.max(*e);
                continue;
            }
            merged.push((*s, *e));
        }

        // Find clear ranges within the zone.
        let mut clear_ranges = Vec::new();
        let mut cursor = zone_start;
        for (bs, be) in &merged {
            if *bs > cursor && *bs > zone_start {
                let cs = cursor.max(zone_start);
                let ce = bs.min(zone_end);
                if ce - cs > box_half_w * 2.0 {
                    clear_ranges.push(ClearRange {
                        from_in: r2(cs),
                        to_in: r2(ce),
                        description: format!("keep {} between {:.0}\" and {:.0}\" from the wall start", dev.tag, cs, ce,),
                    });
                }
            }
            cursor = *be;
        }
        if cursor < zone_end {
            let cs = cursor.max(zone_start);
            if zone_end - cs > box_half_w * 2.0 {
                clear_ranges.push(ClearRange {
                    from_in: r2(cs),
                    to_in: r2(zone_end),
                    description: format!("keep {} between {:.0}\" and {:.0}\" from the wall start", dev.tag, cs, zone_end,),
                });
            }
        }

        let conflicts: Vec<String> = stubs
            .iter()
            .filter(|(along, bot, top, _)| {
                (dev_top + clearance_in >= *bot && *top + clearance_in >= dev_bot)
                    && *along >= zone_start - box_half_w - clearance_in
                    && *along <= zone_end + box_half_w + clearance_in
            })
            .map(|(_, _, _, id)| id.clone())
            .collect();

        let status = if clear_ranges.is_empty() && !conflicts.is_empty() {
            "will_clash"
        } else if !conflicts.is_empty() {
            "may_clash"
        } else {
            "clear"
        };

        zones.push(PlacementZone { device: dev.id.clone(), zone_in: [r2(zone_start), r2(zone_end)], status: status.into(), clear_ranges, conflicts });
    }
    zones
}

fn wall_length_in(w: &crate::truth::Wall) -> f64 {
    let dx = w.to_ft[0] - w.from_ft[0];
    let dy = w.to_ft[1] - w.from_ft[1];
    (dx * dx + dy * dy).sqrt() * 12.0
}
