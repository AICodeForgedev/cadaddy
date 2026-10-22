//! Site parts: trees and shrubs, parking stalls, vehicles (generic outlines at typical design
//! sizes), site utilities, amenities and gates. Real size, inches; centred on their insertion
//! point unless they sit against a curb or fence line (Y 0).

use std::f64::consts::TAU;

use super::{Args, Catalog, Discipline, Kit, REAL, args};

const D: Discipline = Discipline::Site;

pub(crate) fn add(c: &mut Catalog) {
    const PL: &str = "Planting";
    for ft in [10.0, 15.0, 20.0, 25.0, 30.0, 40.0] {
        c.add(
            D,
            PL,
            &format!("TREE_DECIDUOUS_{ft}FT"),
            format!("Tree, deciduous {ft}' canopy"),
            "shade plant landscape",
            tree,
            args(&[ft * 12.0, 0.0]),
            REAL,
        );
        c.add(
            D,
            PL,
            &format!("TREE_CONIFER_{ft}FT"),
            format!("Tree, conifer {ft}' spread"),
            "evergreen pine plant landscape",
            tree,
            args(&[ft * 12.0, 1.0]),
            REAL,
        );
    }
    for ft in [10.0, 15.0, 20.0] {
        c.add(D, PL, &format!("TREE_PALM_{ft}FT"), format!("Tree, palm {ft}' fronds"), "plant landscape", tree, args(&[ft * 12.0, 2.0]), REAL);
    }
    for ft in [2.0, 3.0, 4.0, 6.0] {
        c.add(D, PL, &format!("SHRUB_{ft}FT"), format!("Shrub, {ft}' spread"), "bush plant landscape", shrub, args(&[ft * 12.0, 0.0]), REAL);
        c.add(
            D,
            PL,
            &format!("SHRUB_EVERGREEN_{ft}FT"),
            format!("Shrub, evergreen {ft}' spread"),
            "bush plant landscape",
            shrub,
            args(&[ft * 12.0, 1.0]),
            REAL,
        );
    }
    const PK: &str = "Parking";
    for (word, label, w, l, aisle, angle) in [
        ("STD_90", "standard 9'x18', 90°", 108.0, 216.0, 0.0, 90.0),
        ("COMPACT_90", "compact 8'x16', 90°", 96.0, 192.0, 0.0, 90.0),
        ("ADA_90", "accessible 8'x18' with 5' aisle", 96.0, 216.0, 60.0, 90.0),
        ("VAN_90", "van accessible 11'x18' with 5' aisle", 132.0, 216.0, 60.0, 90.0),
        ("STD_60", "standard 9' wide, 60°", 108.0, 216.0, 0.0, 60.0),
        ("STD_45", "standard 9' wide, 45°", 108.0, 216.0, 0.0, 45.0),
    ] {
        c.add(
            D,
            PK,
            &format!("PARKING_{word}"),
            format!("Parking stall, {label}"),
            "space car stall striping",
            stall,
            args(&[w, l, aisle, angle]),
            REAL,
        );
    }
    const VE: &str = "Vehicles";
    for (word, label, l, w) in [
        ("COMPACT", "Car, compact", 168.0, 70.0),
        ("SEDAN", "Car, sedan", 192.0, 74.0),
        ("SUV", "SUV", 198.0, 79.0),
        ("PICKUP", "Pickup truck", 228.0, 80.0),
        ("VAN", "Passenger van", 204.0, 79.0),
        ("BOX_TRUCK", "Box truck", 288.0, 96.0),
        ("BUS", "Bus, 40'", 480.0, 102.0),
        ("FIRE_TRUCK", "Fire truck", 420.0, 100.0),
    ] {
        c.add(D, VE, &format!("VEHICLE_{word}"), format!("{label} (plan, generic)"), "vehicle car auto plan", vehicle, args(&[l, w]), REAL);
    }
    const UT: &str = "Site utilities";
    for (word, label, kind, s) in [
        ("MANHOLE_48", "Manhole, 48\" structure", 0.0, 48.0),
        ("MANHOLE_60", "Manhole, 60\" structure", 0.0, 60.0),
        ("CATCH_BASIN_24X24", "Catch basin, 24\"x24\"", 1.0, 24.0),
        ("CATCH_BASIN_24X36", "Catch basin, 24\"x36\"", 1.0, 36.0),
        ("CURB_INLET", "Curb inlet", 2.0, 48.0),
        ("FIRE_HYDRANT", "Fire hydrant", 3.0, 12.0),
        ("LIGHT_POLE_1", "Light pole, single head", 4.0, 1.0),
        ("LIGHT_POLE_2", "Light pole, double head", 4.0, 2.0),
        ("BOLLARD", "Bollard", 5.0, 6.0),
        ("UTILITY_POLE", "Utility pole", 6.0, 12.0),
        ("VALVE_BOX", "Valve box", 7.0, 8.0),
        ("WATER_METER", "Water meter box", 8.0, 18.0),
        ("CLEANOUT_SITE", "Cleanout, site", 9.0, 6.0),
    ] {
        c.add(D, UT, word, label.into(), "civil storm sewer water", utility, args(&[kind, s]), REAL);
    }
    const AM: &str = "Site amenities";
    for (word, label, kind, l) in [
        ("BENCH_4", "Bench, 4'", 0.0, 48.0),
        ("BENCH_6", "Bench, 6'", 0.0, 72.0),
        ("PICNIC_TABLE_6", "Picnic table, 6'", 1.0, 72.0),
        ("PICNIC_TABLE_8", "Picnic table, 8'", 1.0, 96.0),
        ("BIKE_RACK_5", "Bike rack, 5 bikes", 2.0, 5.0),
        ("BIKE_RACK_10", "Bike rack, 10 bikes", 2.0, 10.0),
        ("TRASH_RECEPTACLE", "Trash receptacle", 3.0, 24.0),
        ("FLAGPOLE", "Flagpole", 4.0, 30.0),
        ("MAILBOX_CLUSTER_8", "Mailbox cluster, 8 doors", 5.0, 8.0),
        ("MAILBOX_CLUSTER_16", "Mailbox cluster, 16 doors", 5.0, 16.0),
    ] {
        c.add(D, AM, word, label.into(), "furniture outdoor", amenity, args(&[kind, l]), REAL);
    }
    for (word, label, w, double) in [
        ("GATE_WALK_36", "Gate, walk 3'", 36.0, 0.0),
        ("GATE_WALK_48", "Gate, walk 4'", 48.0, 0.0),
        ("GATE_DRIVE_144", "Gate, double drive 12'", 144.0, 1.0),
        ("GATE_DRIVE_192", "Gate, double drive 16'", 192.0, 1.0),
    ] {
        c.add(D, AM, word, label.into(), "fence swing", gate, args(&[w, double]), REAL);
    }
}

/// A tree: deciduous (scalloped canopy), conifer (spokes) or palm (fronds); a trunk and a
/// simple body (trunk and canopy) to its height.
fn tree(k: &mut Kit, a: &Args) {
    let (d, kind) = (a.get(0), a.get(1) as i64);
    let r = d / 2.0;
    match kind {
        0 => {
            let lobes = 12;
            let pts: Vec<(f64, f64, f64)> = (0..lobes)
                .map(|i| {
                    let t = TAU * f64::from(i) / f64::from(lobes);
                    (r * 0.92 * t.cos(), r * 0.92 * t.sin(), -0.35)
                })
                .collect();
            k.pline(&pts, true);
            k.circle((0.0, 0.0), r * 0.06);
        }
        1 => {
            k.circle((0.0, 0.0), r);
            for i in 0..16 {
                let t = TAU * f64::from(i) / 16.0;
                let l = if i % 2 == 0 { r } else { r * 0.7 };
                k.line((0.0, 0.0), (l * t.cos(), l * t.sin()));
            }
        }
        _ => {
            for i in 0..8 {
                let t = TAU * f64::from(i) / 8.0;
                let tip = (r * t.cos(), r * t.sin());
                let mid = (r * 0.55 * (t + 0.18).cos(), r * 0.55 * (t + 0.18).sin());
                k.path(&[(0.0, 0.0), mid, tip]);
            }
            k.circle((0.0, 0.0), r * 0.08);
        }
    }
    let trunk = (r * 0.05).max(3.0);
    k.cylinder((0.0, 0.0), trunk, 0.0, 96.0);
    k.cylinder((0.0, 0.0), r * 0.8, 96.0, 96.0 + d * 0.8);
}

fn shrub(k: &mut Kit, a: &Args) {
    let (d, evergreen) = (a.get(0), a.get(1) > 0.5);
    let r = d / 2.0;
    if evergreen {
        k.circle((0.0, 0.0), r);
        for i in 0..8 {
            let t = TAU * f64::from(i) / 8.0;
            k.line((r * 0.3 * t.cos(), r * 0.3 * t.sin()), (r * t.cos(), r * t.sin()));
        }
    } else {
        let pts: Vec<(f64, f64, f64)> = (0..8)
            .map(|i| {
                let t = TAU * f64::from(i) / 8.0;
                (r * 0.85 * t.cos(), r * 0.85 * t.sin(), -0.4)
            })
            .collect();
        k.pline(&pts, true);
    }
    k.dot((0.0, 0.0), (r * 0.08).max(1.0));
}

/// A stall from the curb (Y 0) out to −Y, `w` wide (measured square to its stripes), at `angle`.
fn stall(k: &mut Kit, a: &Args) {
    let (w, l, aisle, angle) = (a.get(0), a.get(1), a.get(2), a.get(3));
    let t = angle.to_radians();
    if (angle - 90.0).abs() < 1e-9 {
        k.line((0.0, 0.0), (0.0, -l));
        k.line((w, 0.0), (w, -l));
        if aisle > 0.0 {
            // The access aisle beside it, hatched with diagonals.
            k.line((w + aisle, 0.0), (w + aisle, -l));
            let n = (l / 36.0).floor() as usize;
            for i in 0..n {
                let y = -(i as f64) * 36.0;
                k.line((w, y), (w + aisle, (y - aisle).max(-l)));
            }
            // The accessible symbol's place: a wheelchair box.
            k.rect((w / 2.0 - 18.0, -l + 12.0), (w / 2.0 + 18.0, -l + 48.0));
        }
        k.stretch("Width", (0.0, -l), (w, -l), (w * 0.6, 6.0), (w + aisle + 6.0, -l - 6.0), 84.0, 156.0, 6.0);
    } else {
        // Angled: stripes at the angle, the stall width along the curb w / sin(angle).
        let along = w / t.sin();
        let (dx, dy) = (l * t.cos(), -l * t.sin());
        k.line((0.0, 0.0), (dx, dy));
        k.line((along, 0.0), (along + dx, dy));
        k.line((dx, dy), (along + dx, dy));
    }
    k.attdef_hidden("STALL_NO", "Stall number", "", (w / 2.0, -l / 2.0), 6.0);
}

fn vehicle(k: &mut Kit, a: &Args) {
    let (l, w) = (a.get(0), a.get(1));
    let (x, y) = (l / 2.0, w / 2.0);
    k.round_rect((-x, -y), (x, y), w * 0.15);
    let cab = if l > 260.0 { l * 0.2 } else { l * 0.45 };
    // Windshield and the roof or box.
    k.line((x - l * 0.28, -y * 0.8), (x - l * 0.28, y * 0.8));
    k.round_rect((x - l * 0.28 - cab, -y * 0.8), (x - l * 0.28, y * 0.8), w * 0.08);
    // Wheels.
    for wx in [-x + l * 0.18, x - l * 0.2] {
        k.rect((wx - 13.0, y - 2.0), (wx + 13.0, y + 3.0));
        k.rect((wx - 13.0, -y - 3.0), (wx + 13.0, -y + 2.0));
    }
}

/// Site utilities, by `kind` (see the table in [`add`]).
fn utility(k: &mut Kit, a: &Args) {
    let (kind, s) = (a.get(0) as i64, a.get(1));
    match kind {
        0 => {
            k.circle((0.0, 0.0), s / 2.0);
            k.circle((0.0, 0.0), 12.0);
            k.text((0.0, 0.0), 6.0, "MH");
        }
        1 => {
            k.rect((-12.0, -s / 2.0), (12.0, s / 2.0));
            for f in [-0.3, 0.0, 0.3] {
                k.line((-10.0, s * f), (10.0, s * f));
            }
        }
        2 => {
            k.rect((-s / 2.0, 0.0), (s / 2.0, -24.0));
            k.line((-s / 2.0 - 24.0, 0.0), (s / 2.0 + 24.0, 0.0));
            k.line((-s / 2.0, 0.0), (-s / 2.0 - 12.0, 6.0));
            k.line((s / 2.0, 0.0), (s / 2.0 + 12.0, 6.0));
        }
        3 => {
            k.circle((0.0, 0.0), 6.0);
            k.rect((-10.0, -2.0), (-6.0, 2.0));
            k.rect((6.0, -2.0), (10.0, 2.0));
            k.rect((-2.0, 6.0), (2.0, 10.0));
            k.cylinder((0.0, 0.0), 6.0, 0.0, 30.0);
        }
        4 => {
            k.circle((0.0, 0.0), 4.0);
            let heads = s as usize;
            for i in 0..heads {
                let dir = if i == 0 { 1.0 } else { -1.0 };
                k.line((dir * 4.0, 0.0), (dir * 30.0, 0.0));
                k.rect((dir * 30.0, -6.0), (dir * 48.0, 6.0));
            }
            k.cylinder((0.0, 0.0), 4.0, 0.0, 300.0);
        }
        5 => {
            k.circle((0.0, 0.0), s / 2.0);
            k.dot((0.0, 0.0), s / 4.0);
            k.cylinder((0.0, 0.0), s / 2.0, 0.0, 42.0);
        }
        6 => {
            k.circle((0.0, 0.0), s / 2.0);
            k.line((-s / 2.0, -s / 2.0), (s / 2.0, s / 2.0));
            k.line((-s / 2.0, s / 2.0), (s / 2.0, -s / 2.0));
        }
        7 => {
            k.circle((0.0, 0.0), s / 2.0);
            k.text((0.0, 0.0), 3.0, "WV");
        }
        8 => {
            k.rect((-s / 2.0, -s * 0.35), (s / 2.0, s * 0.35));
            k.text((0.0, 0.0), 3.0, "WM");
        }
        _ => {
            k.circle((0.0, 0.0), s / 2.0);
            k.circle((0.0, 0.0), s / 4.0);
            k.text((0.0, -s), 3.0, "CO");
        }
    }
}

fn amenity(k: &mut Kit, a: &Args) {
    let (kind, n) = (a.get(0) as i64, a.get(1));
    match kind {
        0 => {
            k.rect((-n / 2.0, 0.0), (n / 2.0, -20.0));
            for f in [-0.25, 0.0, 0.25] {
                k.line((n * f, 0.0), (n * f, -20.0));
            }
            k.cube((-n / 2.0, 0.0), (n / 2.0, -20.0), 16.0, 18.0);
        }
        1 => {
            k.rect((-n / 2.0, -15.0), (n / 2.0, 15.0));
            k.rect((-n / 2.0, 22.0), (n / 2.0, 32.0));
            k.rect((-n / 2.0, -32.0), (n / 2.0, -22.0));
            k.cube((-n / 2.0, -15.0), (n / 2.0, 15.0), 28.0, 30.0);
        }
        2 => {
            let count = n as usize;
            for i in 0..count {
                let x = i as f64 * 30.0;
                k.pline(&[(x, 0.0, 0.0), (x, 18.0, -1.0), (x + 18.0, 18.0, 0.0), (x + 18.0, 0.0, 0.0)], false);
            }
        }
        3 => {
            k.circle((0.0, 0.0), n / 2.0);
            k.circle((0.0, 0.0), n / 2.0 - 2.0);
            k.cylinder((0.0, 0.0), n / 2.0, 0.0, 36.0);
        }
        4 => {
            k.circle((0.0, 0.0), 3.0);
            k.circle((0.0, 0.0), 18.0);
            k.cylinder((0.0, 0.0), 3.0, 0.0, n * 12.0);
        }
        _ => {
            let w = 18.0 + n * 1.5;
            k.rect((-w / 2.0, 0.0), (w / 2.0, -18.0));
            let cols = (n / 4.0).max(1.0) as usize;
            for i in 1..cols {
                let x = -w / 2.0 + w * i as f64 / cols as f64;
                k.line((x, 0.0), (x, -18.0));
            }
            k.cube((-w / 2.0, 0.0), (w / 2.0, -18.0), 0.0, 60.0);
        }
    }
}

fn gate(k: &mut Kit, a: &Args) {
    let (w, double) = (a.get(0), a.get(1) > 0.5);
    k.line((-12.0, 0.0), (0.0, 0.0));
    k.line((w, 0.0), (w + 12.0, 0.0));
    k.rect((-2.0, -2.0), (2.0, 2.0));
    k.rect((w - 2.0, -2.0), (w + 2.0, 2.0));
    if double {
        let h = w / 2.0;
        k.line((0.0, 0.0), (0.0, -h));
        k.arc((0.0, 0.0), h, 270.0, 360.0);
        k.line((w, 0.0), (w, -h));
        k.arc((w, 0.0), h, 180.0, 270.0);
    } else {
        k.line((0.0, 0.0), (0.0, -w));
        k.arc((0.0, 0.0), w, 270.0, 360.0);
    }
}
