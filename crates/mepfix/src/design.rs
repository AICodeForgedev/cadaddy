//! The made-up building and its MEP systems, as numbers (the truth), before anything is drawn.
//!
//! A two-storey office: a row of rooms either side of a corridor between grid lines, a mechanical
//! room at the west end of each floor (an air handler, and on level 1 the water heater and the
//! utility services), toilets at the east end over a plumbing chase. Each floor's air handler
//! feeds a rectangular supply main down the corridor; round branches feed a VAV box in each room
//! and round runouts its diffusers. Return is through the ceiling plenum to a short return main.
//! Toilet exhaust rises to a roof fan. Cold and hot water, hot water return, gas, sanitary and
//! vent; a fire protection riser and mains with branch lines to pendent heads.
//!
//! Heights follow coordination practice: under each floor's structure the ducts hang highest
//! (centreline-aligned in one band), sprinkler piping below them, plumbing below that, and the
//! ceilings below everything. A few clashes are planted on purpose and recorded.

use crate::rng::Rng;
use crate::style::Style;
use crate::truth::*;
use std::collections::BTreeMap;

/// Outside diameter of a nominal pipe size, inches (steel/copper close enough for clashes).
pub fn pipe_od(n: f64) -> f64 {
    match n {
        x if x <= 0.5 => 0.84,
        x if x <= 0.75 => 1.05,
        x if x <= 1.0 => 1.315,
        x if x <= 1.25 => 1.66,
        x if x <= 1.5 => 1.9,
        x if x <= 2.0 => 2.375,
        x if x <= 2.5 => 2.875,
        x if x <= 3.0 => 3.5,
        x if x <= 4.0 => 4.5,
        x => x + 0.625,
    }
}

/// The depth (vertical size) of a segment's cross-section, inches.
pub fn depth_in(shape: &str, size: [f64; 2]) -> f64 {
    match shape {
        "rect" => size[1],
        "round" => size[0],
        _ => pipe_od(size[0]),
    }
}

/// The width (across, on the plan) of a segment's cross-section, inches.
pub fn width_in(shape: &str, size: [f64; 2]) -> f64 {
    match shape {
        "rect" => size[0],
        "round" => size[0],
        _ => pipe_od(size[0]),
    }
}

/// Round duct size for an airflow (low velocity), inches.
fn round_for(cfm: f64) -> f64 {
    [(110.0, 6.0), (200.0, 8.0), (350.0, 10.0), (550.0, 12.0), (800.0, 14.0), (1100.0, 16.0), (1450.0, 18.0)]
        .iter()
        .find(|(c, _)| cfm <= *c)
        .map_or(20.0, |(_, d)| *d)
}

/// VAV box inlet size for an airflow, inches (at most 12: the rooms are office-sized).
fn vav_inlet(cfm: f64) -> f64 {
    [(225.0, 6.0), (400.0, 8.0), (650.0, 10.0)].iter().find(|(c, _)| cfm <= *c).map_or(12.0, |(_, d)| *d)
}

/// Rectangular duct width for an airflow at a depth, inches (even, at least 8, at most `max_w`).
fn rect_width(cfm: f64, h: f64, fpm: f64, max_w: f64) -> f64 {
    let area = cfm / fpm * 144.0;
    let w = ((area / h / 2.0).ceil() * 2.0).max(8.0);
    w.min(max_w.max(8.0))
}

fn floor_to(v: f64, step: f64) -> f64 {
    (v / step + 1e-9).floor() * step
}

fn round_to(v: f64, step: f64) -> f64 {
    (v / step).round() * step
}

/// One floor's layout facts, shared by the systems.
#[derive(Clone, Debug)]
struct Floor {
    code: String,
    elev: f64,
    /// Duct band centreline, above the floor.
    duct_c: f64,
    /// Sprinkler and plumbing pipe centrelines.
    fp_c: f64,
    pl_c: f64,
    /// Underside of the girders above, above the floor.
    girder_bob: f64,
    office_clg: f64,
    corr_clg: f64,
    toilet_clg: f64,
}

/// The graph under construction.
struct Net {
    nodes: Vec<Node>,
    segs: Vec<Segment>,
    /// Level code → elevation.
    elev: BTreeMap<String, f64>,
}

impl Net {
    fn node(&mut self, kind: &str, system: &str, level: &str, at: [f64; 3]) -> usize {
        let i = self.nodes.len();
        self.nodes.push(Node {
            id: format!("N{:04}", i + 1),
            kind: kind.into(),
            system: system.into(),
            level: level.into(),
            at_ft: at.map(|v| (v * 1e6).round() / 1e6),
            segments: Vec::new(),
            connects: None,
        });
        i
    }

    fn at(&self, i: usize) -> [f64; 3] {
        self.nodes.get(i).map_or([0.0; 3], |n| n.at_ft)
    }

    fn link(&mut self, a: usize, b: usize, system: &str, shape: &str, size: [f64; 2], level: &str, sheet: &str) -> usize {
        let (pa, pb) = (self.at(a), self.at(b));
        let vertical = (pa[0] - pb[0]).abs() < 1e-9 && (pa[1] - pb[1]).abs() < 1e-9;
        let elev = self.elev.get(level).copied().unwrap_or(0.0);
        let d = depth_in(shape, size) / 12.0;
        let height = (!vertical).then(|| {
            let c = (pa[2] + pb[2]) / 2.0 - elev;
            Height { bottom_ft: r6(c - d / 2.0), top_ft: r6(c + d / 2.0), centre_ft: r6(c), note: None }
        });
        let i = self.segs.len();
        let id = format!("S{:04}", i + 1);
        let below_floor = !vertical && pa[2].max(pb[2]) < elev;
        self.segs.push(Segment {
            id: id.clone(),
            system: system.into(),
            kind: if shape == "pipe" { "pipe".into() } else { "duct".into() },
            shape: shape.into(),
            size_in: size,
            from: self.nodes.get(a).map(|n| n.id.clone()).unwrap_or_default(),
            to: self.nodes.get(b).map(|n| n.id.clone()).unwrap_or_default(),
            level: level.into(),
            path_ft: [pa, pb],
            vertical,
            height,
            tag: None,
            slope: None,
            below_floor,
            sheet: sheet.into(),
        });
        for n in [a, b] {
            if let Some(n) = self.nodes.get_mut(n) {
                n.segments.push(id.clone());
            }
        }
        i
    }

    /// A run from node `start` through `pts` (new auto-kind nodes), one size; returns the nodes
    /// (start first) and the segments.
    fn run(
        &mut self,
        start: usize,
        pts: &[[f64; 3]],
        system: &str,
        shape: &str,
        size: [f64; 2],
        level: &str,
        sheet: &str,
    ) -> (Vec<usize>, Vec<usize>) {
        let mut nodes = vec![start];
        let mut segs = Vec::new();
        let mut prev = start;
        for p in pts {
            let n = self.node("", system, level, *p);
            segs.push(self.link(prev, n, system, shape, size, level, sheet));
            nodes.push(n);
            prev = n;
        }
        (nodes, segs)
    }
}

fn r6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// Everything decided before drawing: the truth (without sheets and clashes) and the elements
/// the planted clashes touch.
pub struct Design {
    pub truth: Truth,
    /// Segment and equipment ids made or moved to plant a clash.
    pub planted: Vec<String>,
}

pub fn design(seed: u64, st: &Style, r: &mut Rng) -> Design {
    let mut t = Truth { schema: SCHEMA.into(), seed, ..Truth::default() };
    let names = ["HARBOR POINT", "MAPLE CROSSING", "CEDAR HOLLOW", "BLUE HERON", "RIVERSTONE", "OAK MEADOW", "IRON BRIDGE", "WILLOW BEND"];
    let kinds = ["OFFICE BUILDING", "COMMONS", "BUSINESS CENTER", "OFFICE PAVILION"];
    t.project = format!("{} {}", r.pick(&names), r.pick(&kinds));
    t.building.name = t.project.clone();

    // ---- Grid, levels, structure ---------------------------------------------------------
    let nb = r.range(4, 7);
    let uniform = r.chance(0.6);
    let base = r.pick::<f64>(&[24.0, 25.0, 26.0, 28.0, 30.0]);
    let bays: Vec<f64> = (0..nb).map(|_| if uniform { base } else { r.pick::<f64>(&[24.0, 26.0, 28.0, 30.0]) }).collect();
    let mut xs = vec![0.0];
    for b in &bays {
        xs.push(xs.last().copied().unwrap_or(0.0) + b);
    }
    let width = xs.last().copied().unwrap_or(0.0);
    let d_s = r.pick::<f64>(&[20.0, 22.0, 24.0, 26.0]);
    let cw = r.pick::<f64>(&[6.0, 7.0, 8.0]);
    let d_n = r.pick::<f64>(&[20.0, 22.0, 24.0, 26.0]);
    let (y1, y2) = (d_s, d_s + cw);
    let y3 = y2 + d_n;
    let ys = [0.0, y1, y2, y3];
    let letters = ["A", "B", "C", "D", "E", "F", "G", "H", "J", "K"];
    let label = |i: usize, numbers: bool| {
        if numbers { format!("{}", i + 1) } else { letters.get(i).map_or_else(|| format!("{}", i + 1), |l| (*l).to_string()) }
    };
    t.building.grid.x = xs.iter().enumerate().map(|(i, x)| GridLine { label: label(i, st.numbers_on_x), at_ft: *x }).collect();
    t.building.grid.y = ys.iter().enumerate().map(|(i, y)| GridLine { label: label(i, !st.numbers_on_x), at_ft: *y }).collect();

    let f2f1 = r.pick::<f64>(&[13.0, 13.5, 14.0, 15.0]);
    let f2f2 = r.pick::<f64>(&[13.0, 13.5, 14.0, 15.0]);
    t.building.levels = vec![
        Level { name: "LEVEL 1".into(), code: "1".into(), elevation_ft: 0.0 },
        Level { name: "LEVEL 2".into(), code: "2".into(), elevation_ft: f2f1 },
        Level { name: "ROOF".into(), code: "R".into(), elevation_ft: f2f1 + f2f2 },
    ];
    let elev = |c: &str| match c {
        "1" => 0.0,
        "2" => f2f1,
        _ => f2f1 + f2f2,
    };
    t.building.slab_in = r.pick::<f64>(&[5.0, 5.5, 6.0]);
    let girders = [("W16x31", 15.9, 5.5), ("W18x35", 17.7, 6.0), ("W21x44", 20.7, 6.5), ("W24x55", 23.6, 7.0)];
    let beams = [("W12x19", 12.2, 4.0), ("W14x22", 13.7, 5.0), ("W16x26", 15.7, 5.5)];
    let columns = [("W10x33", [9.7, 8.0]), ("W8x31", [8.0, 8.0]), ("HSS6x6", [6.0, 6.0]), ("W10x49", [10.0, 10.0])];
    // Structure of the level-2 floor and of the roof: girders on the x lines (spanning the rooms
    // and the corridor), beams on the y lines.
    let mut framing = BTreeMap::new();
    for lv in ["2", "R"] {
        let g = girders[r.below(girders.len())];
        let mut b = beams[r.below(beams.len())];
        if b.1 > g.1 {
            b = beams[0];
        }
        // The roof is metal deck only.
        let tos = if lv == "R" { elev(lv) - 1.5 / 12.0 } else { elev(lv) - t.building.slab_in / 12.0 };
        framing.insert(lv.to_string(), (g, b, tos));
        let mut n = 0;
        for (i, x) in xs.iter().enumerate() {
            for j in 0..3 {
                n += 1;
                t.building.beams.push(Beam {
                    id: format!("B-{lv}-{n:03}"),
                    level: lv.into(),
                    grid: label(i, st.numbers_on_x),
                    size: g.0.into(),
                    depth_in: g.1,
                    flange_in: g.2,
                    from_ft: [*x, ys[j]],
                    to_ft: [*x, ys[j + 1]],
                    tos_ft: r6(tos),
                });
            }
        }
        for (j, y) in ys.iter().enumerate() {
            for i in 0..nb {
                n += 1;
                t.building.beams.push(Beam {
                    id: format!("B-{lv}-{n:03}"),
                    level: lv.into(),
                    grid: label(j, !st.numbers_on_x),
                    size: b.0.into(),
                    depth_in: b.1,
                    flange_in: b.2,
                    from_ft: [xs.get(i).copied().unwrap_or(0.0), *y],
                    to_ft: [xs.get(i + 1).copied().unwrap_or(0.0), *y],
                    tos_ft: r6(tos),
                });
            }
        }
    }
    let col = columns[r.below(columns.len())];
    let roof_tos = framing.get("R").map_or(f2f1 + f2f2, |f| f.2);
    for (i, x) in xs.iter().enumerate() {
        for (j, y) in ys.iter().enumerate() {
            let g = format!("{}-{}", label(i, st.numbers_on_x), label(j, !st.numbers_on_x));
            t.building.columns.push(Column {
                id: format!("C-{g}"),
                grid: g,
                at_ft: [*x, *y],
                size: col.0.into(),
                footprint_in: col.1,
                bottom_ft: 0.0,
                top_ft: r6(roof_tos),
            });
        }
    }

    // ---- Loads and duct sizes (they set the bands) ---------------------------------------
    let cfm_sf = r.pick::<f64>(&[0.9, 1.0, 1.1, 1.2]);
    let h_main = r.pick::<f64>(&[12.0, 14.0]);
    let max_main_w = (cw - 2.0) * 12.0;
    let ahu_h = r.pick::<f64>(&[6.0, 6.5, 7.0]);
    let fp_main: f64 = if nb >= 6 { 4.0 } else { 3.0 };
    let cw_main: f64 = if nb >= 6 { 2.0 } else { 1.5 };

    // The rooms of a floor that get supply air: (bay, south side, cfm).
    let mut floors = Vec::new();
    for lv in ["1", "2"] {
        let above = if lv == "1" { "2" } else { "R" };
        let (g, b, tos) = framing.get(above).copied().unwrap_or((girders[0], beams[0], 0.0));
        let e = elev(lv);
        let girder_bob = tos - g.1 / 12.0 - e;
        let bob = tos - g.1.max(b.1) / 12.0 - e;
        let duct_top = floor_to(bob - 1.0 / 12.0, 1.0 / 12.0);
        // The deepest duct item in the band: the main, or a 12" VAV box (16" casing).
        let depth = h_main.max(16.0);
        let duct_c = duct_top - depth / 24.0;
        let fp_c = duct_top - depth / 12.0 - 2.0 / 12.0 - pipe_od(fp_main) / 24.0;
        let fp_bottom = fp_c - pipe_od(fp_main) / 24.0;
        let pl_c = fp_bottom - 2.0 / 12.0 - pipe_od(cw_main.max(2.0)) / 24.0;
        let pl_bottom = pl_c - pipe_od(cw_main.max(2.0)) / 24.0;
        let office_clg = r.pick::<f64>(&[8.5, 9.0, 9.5, 10.0]).min(floor_to(fp_bottom - 0.25, 0.5)).max(7.5);
        let corr_clg = r.pick::<f64>(&[8.0, 8.5, 9.0]).min(floor_to(pl_bottom - 0.25, 0.5)).max(7.5);
        let toilet_clg = r.pick::<f64>(&[8.0, 8.5]).min(floor_to(pl_bottom - 0.25, 0.5)).max(7.5);
        floors.push(Floor { code: lv.into(), elev: e, duct_c, fp_c, pl_c, girder_bob, office_clg, corr_clg, toilet_clg });
    }

    // ---- Rooms --------------------------------------------------------------------------
    let pool = ["OFFICE", "OFFICE", "OPEN OFFICE", "CONFERENCE", "BREAK ROOM", "WORK ROOM", "TRAINING", "STORAGE", "FILES", "COPY"];
    let toilets = r.pick(&[("MEN", "WOMEN"), ("TOILET", "TOILET"), ("RESTROOM", "RESTROOM")]);
    for f in &floors {
        let n0: usize = if f.code == "1" { 100 } else { 200 };
        let act = |h: f64| Some(Ceiling { height_ft: h, kind: "ACT".into(), tag: String::new() });
        let add = |t: &mut Truth, num: usize, name: &str, min: [f64; 2], max: [f64; 2], ceiling: Option<Ceiling>| {
            let mut ceiling = ceiling;
            if let Some(c) = ceiling.as_mut() {
                c.tag = st.ceiling_text(c.height_ft, &c.kind).join(" ");
            }
            t.building.rooms.push(Room {
                id: format!("R{num}"),
                level: f.code.clone(),
                number: format!("{num}"),
                name: name.into(),
                min_ft: min,
                max_ft: max,
                ceiling,
            });
        };
        add(&mut t, n0, "CORRIDOR", [0.0, y1], [width, y2], act(f.corr_clg));
        for i in 0..nb {
            let (x0, x1) = (xs.get(i).copied().unwrap_or(0.0), xs.get(i + 1).copied().unwrap_or(0.0));
            let south = n0 + 1 + i;
            if i == 0 {
                add(&mut t, south, "MECH", [x0, 0.0], [x1, y1], None);
            } else {
                let name = r.pick(&pool);
                let c = if name == "STORAGE" || name == "FILES" {
                    Some(Ceiling { height_ft: f.office_clg, kind: "GYP".into(), tag: String::new() })
                } else {
                    act(f.office_clg)
                };
                add(&mut t, south, name, [x0, 0.0], [x1, y1], c);
            }
            let north = n0 + 1 + nb + i;
            if i + 1 == nb {
                let mid = (x0 + x1) / 2.0;
                let gyp = Some(Ceiling { height_ft: f.toilet_clg, kind: "GYP".into(), tag: String::new() });
                add(&mut t, north, toilets.0, [x0, y2], [mid, y3], gyp.clone());
                add(&mut t, north + 1, toilets.1, [mid, y2], [x1, y3], gyp);
            } else {
                add(&mut t, north, r.pick(&pool), [x0, y2], [x1, y3], act(f.office_clg));
            }
        }
    }

    // ---- Systems -------------------------------------------------------------------------
    let sysdefs = [
        ("SA", "SUPPLY AIR", "duct"),
        ("RA", "RETURN AIR", "duct"),
        ("EA", "EXHAUST AIR", "duct"),
        ("OA", "OUTSIDE AIR", "duct"),
        ("CW", "DOMESTIC COLD WATER", "pipe"),
        ("HW", "DOMESTIC HOT WATER", "pipe"),
        ("HWR", "DOMESTIC HOT WATER RETURN", "pipe"),
        ("SAN", "SANITARY", "pipe"),
        ("V", "VENT", "pipe"),
        ("G", "NATURAL GAS", "pipe"),
        ("FP", "FIRE PROTECTION", "pipe"),
    ];
    t.systems = sysdefs.iter().map(|(c, n, k)| System { code: (*c).into(), name: (*n).into(), kind: (*k).into(), printed: st.sys(c) }).collect();

    let mut net = Net { nodes: Vec::new(), segs: Vec::new(), elev: ["1", "2", "R"].iter().map(|c| ((*c).to_string(), elev(c))).collect() };
    let mut planted: Vec<String> = Vec::new();
    let plant_fp = r.chance(0.6);
    let plant_ceiling = r.chance(0.5);
    let plant_struct = r.chance(0.5);
    let plant_fp = plant_fp || !(plant_ceiling || plant_struct);
    let plant_level = r.pick(&["1", "2"]);

    let w0 = bays.first().copied().unwrap_or(24.0);
    let yc = (y1 + y2) / 2.0;
    let x_m = 0.5 * w0;
    let x_r = 0.25 * w0;
    let ahu_y0 = 0.2 * d_s;
    let ahu_w = r.pick::<f64>(&[5.0, 5.5, 6.0]);
    let ahu_yc = ahu_y0 + ahu_w / 2.0;
    let fs = 0.25;
    let fnorth = r.pick::<f64>(&[0.25, 0.35]);
    let mut equip_n = 0usize;
    let mut term_n = 0usize;
    let mut vav_n: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_cfm = BTreeMap::new();
    let mut ea_riser_segs = Vec::new();
    let mut ea_l1_top = None;
    let x_last0 = xs.get(nb - 1).copied().unwrap_or(0.0);
    let x_last1 = width;
    let tw = x_last1 - x_last0;
    let x_er = x_last1 - 3.8;
    let y_er = y2 + 1.4;
    let chase = |k: usize| (x_last1 - 0.75 - 0.4 * k as f64, y2 + 1.0);
    let mut ea_cfm_per_level = 0.0;
    // Level 1's ends of the risers, joined when level 2 is laid out.
    let mut fp_l1_tee: Option<usize> = None;
    let mut san_base: Option<usize> = None;
    let mut l1_feet: BTreeMap<String, usize> = BTreeMap::new();

    let room_at = |t: &Truth, lv: &str, x: f64, y: f64| -> Option<Room> {
        t.building
            .rooms
            .iter()
            .find(|rm| rm.level == lv && x >= rm.min_ft[0] - 1e-9 && x <= rm.max_ft[0] + 1e-9 && y >= rm.min_ft[1] - 1e-9 && y <= rm.max_ft[1] + 1e-9)
            .cloned()
    };
    let mut add_term = |t: &mut Truth,
                        kind: &str,
                        tag: &str,
                        sys: &str,
                        lv: &str,
                        at: [f64; 3],
                        face: [f64; 2],
                        neck: f64,
                        cfm: Option<f64>,
                        node: Option<String>|
     -> String {
        term_n += 1;
        let id = format!("T{term_n:03}");
        let room = room_at(t, lv, at[0], at[1]).map(|r| r.number).unwrap_or_default();
        t.terminals.push(Terminal {
            id: id.clone(),
            kind: kind.into(),
            type_tag: tag.into(),
            system: sys.into(),
            level: lv.into(),
            room,
            at_ft: at,
            face_in: face,
            neck_in: neck,
            cfm,
            node,
        });
        id
    };

    for f in &floors {
        let lv = f.code.as_str();
        let e = f.elev;
        let msheet = format!("M-10{lv}");
        let psheet = format!("P-10{lv}");
        let fsheet = format!("FP-10{lv}");
        let zc = e + f.duct_c;

        // Rooms that get supply: bay, side (true = south), branch x.
        let mut branches: Vec<(f64, bool, f64, Room)> = Vec::new();
        for i in 0..nb {
            let (x0, wi) = (xs.get(i).copied().unwrap_or(0.0), bays.get(i).copied().unwrap_or(24.0));
            if i > 0
                && let Some(rm) = room_at(&t, lv, x0 + 0.5 * wi, y1 / 2.0)
            {
                let cfm = round_to((rm.max_ft[0] - rm.min_ft[0]) * (rm.max_ft[1] - rm.min_ft[1]) * cfm_sf, 25.0);
                branches.push((x0 + fs * wi, true, cfm, rm));
            }
            if i + 1 < nb
                && let Some(rm) = room_at(&t, lv, x0 + 0.5 * wi, (y2 + y3) / 2.0)
            {
                let cfm = round_to((rm.max_ft[0] - rm.min_ft[0]) * (rm.max_ft[1] - rm.min_ft[1]) * cfm_sf, 25.0);
                let fx = if i == 0 { 0.8 } else { fnorth };
                branches.push((x0 + fx * wi, false, cfm, rm));
            }
        }
        branches.sort_by(|a, b| a.0.total_cmp(&b.0));
        let supply: f64 = branches.iter().map(|b| b.2).sum();
        total_cfm.insert(lv.to_string(), supply);

        // Air handler.
        equip_n += 1;
        let ahu_tag = format!("AHU-{lv}");
        let ahu_min = [0.15 * w0, ahu_y0, e];
        let ahu_max = [0.65 * w0, ahu_y0 + ahu_w, e + ahu_h];
        let ahu_idx = t.equipment.len();
        let oa_cfm = round_to(supply * 0.2, 25.0);
        t.equipment.push(Equipment {
            id: format!("E{equip_n:02}"),
            tag: ahu_tag.clone(),
            kind: "ahu".into(),
            level: lv.into(),
            min_ft: ahu_min,
            max_ft: ahu_max,
            cfm: Some(supply),
            schedule: [
                ("TAG", ahu_tag.clone()),
                ("SERVES", format!("LEVEL {lv}")),
                ("CFM", format!("{supply:.0}")),
                ("OA CFM", format!("{oa_cfm:.0}")),
                ("ESP (IN WG)", "1.5".into()),
                ("HEAT", if lv == "1" { "GAS".into() } else { "ELEC".into() }),
                ("SIZE (L x W x H)", format!("{} x {} x {}", feet_inches(ahu_max[0] - ahu_min[0]), feet_inches(ahu_w), feet_inches(ahu_h))),
                ("WEIGHT (LB)", format!("{:.0}", round_to(supply * 0.6 + 900.0, 50.0))),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
            nodes: Vec::new(),
        });

        // Supply main: up out of the AHU, north to the corridor, east past the branches.
        let s_conn = net.node("equipment", "SA", lv, [x_m, ahu_yc, e + ahu_h]);
        let mut stops: Vec<f64> = branches.iter().map(|b| b.0).collect();
        stops.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        let cfm_after = |x: f64| -> f64 { branches.iter().filter(|b| b.0 > x + 1e-6).map(|b| b.2).sum() };
        let size_for = |cfm: f64| [rect_width(cfm, h_main, 1200.0, max_main_w), h_main];
        let first = size_for(supply);
        let (n_up, s_up) = net.run(s_conn, &[[x_m, ahu_yc, zc], [x_m, yc, zc]], "SA", "rect", first, lv, &msheet);
        let mut main_segs = s_up.clone();
        let mut prev = n_up.last().copied().unwrap_or(s_conn);
        let mut size = first;
        let mut at_x: BTreeMap<String, usize> = BTreeMap::new();
        for (k, x) in stops.iter().enumerate() {
            let n = net.node("", "SA", lv, [*x, yc, zc]);
            main_segs.push(net.link(prev, n, "SA", "rect", size, lv, &msheet));
            at_x.insert(format!("{x:.4}"), n);
            prev = n;
            let down = cfm_after(*x);
            if down <= 0.0 {
                break;
            }
            let next = size_for(down);
            if next != size {
                // The transition just downstream of the tee.
                let gap = stops.get(k + 1).map_or(4.0, |nx| nx - x);
                let tx = x + (gap / 2.0).min(2.0);
                let n = net.node("", "SA", lv, [tx, yc, zc]);
                main_segs.push(net.link(prev, n, "SA", "rect", size, lv, &msheet));
                prev = n;
                size = next;
            }
        }
        if let Some(eq) = t.equipment.get_mut(ahu_idx) {
            eq.nodes.push(net.nodes.get(s_conn).map(|n| n.id.clone()).unwrap_or_default());
        }
        if let Some(n) = net.nodes.get_mut(s_conn) {
            n.connects = Some(t.equipment.get(ahu_idx).map(|e| e.id.clone()).unwrap_or_default());
        }

        // Branches: round to a VAV box, round runouts to the diffusers.
        let plant_room = branches.iter().filter(|b| !b.1).map(|b| b.3.number.clone()).nth(1);
        for (x, south, cfm, rm) in &branches {
            let Some(&tee) = at_x.get(&format!("{x:.4}")) else { continue };
            let dir = if *south { -1.0 } else { 1.0 };
            let wall = if *south { y1 } else { y2 };
            let depth = rm.max_ft[1] - rm.min_ft[1];
            let clg = rm.ceiling.as_ref().map_or(f.office_clg, |c| c.height_ft);
            let inlet = vav_inlet(*cfm);
            let y_in = wall + dir * 3.0;
            let len = 3.5;
            let y_out = y_in + dir * len;
            let (bn, _) = net.run(tee, &[[*x, y_in, zc]], "SA", "round", [inlet, inlet], lv, &msheet);
            let inlet_node = bn.last().copied().unwrap_or(tee);
            let n_vav = vav_n.entry(lv.to_string()).or_default();
            *n_vav += 1;
            equip_n += 1;
            let vav_tag = format!("VAV-{lv}-{:02}", *n_vav);
            let vh = if inlet <= 8.0 { 12.0 } else { inlet + 4.0 };
            let vw = inlet * 1.5 + 4.0;
            let (ya, yb) = if *south { (y_out, y_in) } else { (y_in, y_out) };
            let eq_id = format!("E{equip_n:02}");
            if let Some(n) = net.nodes.get_mut(inlet_node) {
                n.kind = "equipment".into();
                n.connects = Some(eq_id.clone());
            }
            let out_node = net.node("equipment", "SA", lv, [*x, y_out, zc]);
            if let Some(n) = net.nodes.get_mut(out_node) {
                n.connects = Some(eq_id.clone());
            }
            t.equipment.push(Equipment {
                id: eq_id,
                tag: vav_tag.clone(),
                kind: "vav".into(),
                level: lv.into(),
                min_ft: [r6(x - vw / 24.0), r6(ya), r6(zc - vh / 24.0)],
                max_ft: [r6(x + vw / 24.0), r6(yb), r6(zc + vh / 24.0)],
                cfm: Some(*cfm),
                schedule: [
                    ("TAG", vav_tag),
                    ("INLET (IN)", fmt_in(inlet)),
                    ("MAX CFM", format!("{cfm:.0}")),
                    ("MIN CFM", format!("{:.0}", round_to(cfm * 0.3, 5.0))),
                    ("SERVES", rm.number.clone()),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
                nodes: [inlet_node, out_node].iter().filter_map(|n| net.nodes.get(*n).map(|n| n.id.clone())).collect(),
            });
            let n_diff = if depth >= 24.0 { 3 } else { 2 };
            let fr: &[f64] = if n_diff == 3 { &[0.4, 0.62, 0.85] } else { &[0.45, 0.8] };
            let each = round_to(cfm / n_diff as f64, 5.0);
            let neck = round_for(each).max(6.0);
            let type_tag = format!("CD-{}", ((neck - 6.0) / 2.0) as i64 + 1);
            // The planted low runout: its bottom 2" below the ceiling.
            let low = plant_ceiling && lv == plant_level && plant_room.as_deref() == Some(rm.number.as_str());
            let mut prev = out_node;
            let mut remaining = *cfm;
            let mut run_z = zc;
            if low {
                let d = round_for(remaining);
                run_z = e + clg - 2.0 / 12.0 + d / 24.0;
                let n = net.node("", "SA", lv, [*x, y_out + dir * 0.75, zc]);
                let s1 = net.link(prev, n, "SA", "round", [d, d], lv, &msheet);
                let m = net.node("", "SA", lv, [*x, y_out + dir * 0.75, run_z]);
                let s2 = net.link(n, m, "SA", "round", [d, d], lv, &msheet);
                planted.extend([s1, s2].iter().filter_map(|i| net.segs.get(*i).map(|s| s.id.clone())));
                prev = m;
            }
            for fy in fr {
                let y = wall + dir * fy * depth;
                let d = round_for(remaining);
                let n = net.node("", "SA", lv, [*x, y, run_z]);
                let s = net.link(prev, n, "SA", "round", [d, d], lv, &msheet);
                if low {
                    planted.extend(net.segs.get(s).map(|s| s.id.clone()));
                }
                let dn = net.node("terminal", "SA", lv, [*x, y, e + clg]);
                net.link(n, dn, "SA", "round", [neck, neck], lv, &msheet);
                let nid = net.nodes.get(dn).map(|n| n.id.clone());
                let tid = add_term(&mut t, "supply_diffuser", &type_tag, "SA", lv, [*x, y, e + clg], [24.0, 24.0], neck, Some(each), nid);
                if let Some(n) = net.nodes.get_mut(dn) {
                    n.connects = Some(tid);
                }
                remaining -= each;
                prev = n;
            }
            // Plenum return grille in the room, on the side away from the supply branch.
            let rw = rm.max_ft[0] - rm.min_ft[0];
            let gx = rm.min_ft[0] + if *x - rm.min_ft[0] > 0.5 * rw { 0.25 } else { 0.75 } * rw;
            let gy = wall + dir * 0.5 * depth;
            add_term(&mut t, "return_grille", "RG-1", "RA", lv, [gx, gy, e + clg], [24.0, 24.0], 0.0, Some(round_to(cfm * 0.85, 5.0)), None);
        }

        // Return: up out of the AHU, north to the corridor, a grille there and one at the west end.
        let ret = round_to(supply * 0.85, 25.0);
        let rsize = [rect_width(ret, h_main, 1000.0, max_main_w), h_main];
        let r_conn = net.node("equipment", "RA", lv, [x_r, ahu_yc, e + ahu_h]);
        let (rn, _) = net.run(r_conn, &[[x_r, ahu_yc, zc], [x_r, yc, zc]], "RA", "rect", rsize, lv, &msheet);
        let r_tee = rn.last().copied().unwrap_or(r_conn);
        let half = round_to(ret / 2.0, 25.0);
        let hsize = [rect_width(half, h_main, 1000.0, max_main_w), h_main];
        let (rw, _) = net.run(r_tee, &[[1.5, yc, zc]], "RA", "rect", hsize, lv, &msheet);
        for (node, gx) in [(r_tee, x_r), (rw.last().copied().unwrap_or(r_tee), 1.5)] {
            let g = net.node("terminal", "RA", lv, [gx, yc, e + f.corr_clg]);
            net.link(node, g, "RA", "rect", [22.0, 22.0], lv, &msheet);
            let nid = net.nodes.get(g).map(|n| n.id.clone());
            let tid = add_term(&mut t, "return_grille", "RG-2", "RA", lv, [gx, yc, e + f.corr_clg], [24.0, 24.0], 22.0, Some(half), nid);
            if let Some(n) = net.nodes.get_mut(g) {
                n.connects = Some(tid);
            }
        }
        // Outside air: a louvre in the south wall to the AHU's intake.
        let oa_size = [rect_width(oa_cfm, 12.0, 800.0, 48.0), 12.0];
        let x_oa = 0.4 * w0;
        let louv = net.node("terminal", "OA", lv, [x_oa, 0.0, e + 4.0]);
        let oa_in = net.node("equipment", "OA", lv, [x_oa, ahu_y0, e + 4.0]);
        net.link(louv, oa_in, "OA", "rect", oa_size, lv, &msheet);
        let nid = net.nodes.get(louv).map(|n| n.id.clone());
        let tid = add_term(&mut t, "louver", "L-1", "OA", lv, [x_oa, 0.0, e + 4.0], [oa_size[0] + 12.0, 24.0], 0.0, Some(oa_cfm), nid);
        if let Some(n) = net.nodes.get_mut(louv) {
            n.connects = Some(tid);
        }
        for (n, _) in [(r_conn, 0), (oa_in, 0)] {
            if let Some(eq) = t.equipment.get_mut(ahu_idx) {
                eq.nodes.push(net.nodes.get(n).map(|n| n.id.clone()).unwrap_or_default());
            }
            if let Some(node) = net.nodes.get_mut(n) {
                node.connects = Some(t.equipment.get(ahu_idx).map(|e| e.id.clone()).unwrap_or_default());
            }
        }

        // Toilet exhaust: a grille in each toilet, to the riser in the chase.
        let y_eg = y2 + 0.65 * d_n;
        let eg_cfm = r.pick::<f64>(&[100.0, 125.0, 150.0]);
        let eg_neck = round_for(eg_cfm);
        let mut ea_nodes = Vec::new();
        for fx in [0.4, 0.6] {
            let gx = x_last0 + fx * tw;
            let g = net.node("terminal", "EA", lv, [gx, y_eg, e + f.toilet_clg]);
            let up = net.node("", "EA", lv, [gx, y_eg, zc]);
            net.link(g, up, "EA", "round", [eg_neck, eg_neck], lv, &msheet);
            let nid = net.nodes.get(g).map(|n| n.id.clone());
            let tid = add_term(&mut t, "exhaust_grille", "EG-1", "EA", lv, [gx, y_eg, e + f.toilet_clg], [12.0, 12.0], eg_neck, Some(eg_cfm), nid);
            if let Some(n) = net.nodes.get_mut(g) {
                n.connects = Some(tid);
            }
            ea_nodes.push(up);
        }
        let (Some(&a), Some(&b)) = (ea_nodes.first(), ea_nodes.get(1)) else { continue };
        net.link(a, b, "EA", "round", [eg_neck, eg_neck], lv, &msheet);
        ea_cfm_per_level = 2.0 * eg_cfm;
        let ea_d = round_for(2.0 * eg_cfm);
        let (en, _) = net.run(b, &[[x_er, y_eg, zc], [x_er, y_er, zc]], "EA", "round", [ea_d, ea_d], lv, &msheet);
        let ea_foot = en.last().copied().unwrap_or(b);
        if lv == "1" {
            ea_l1_top = Some(ea_foot);
        } else if let Some(l1) = ea_l1_top {
            let riser = [12.0, 10.0];
            ea_riser_segs.push(net.link(l1, ea_foot, "EA", "rect", riser, "1", "M-101"));
        }

        // Fire protection: the riser in the mech room, a main down the corridor, branch lines
        // to the heads.
        let x_fr = 0.1 * w0;
        let y_fr = y1 - 1.5;
        let y_fp = y1 + 0.2 * cw;
        let fz = e + f.fp_c;
        let fp_tee = net.node("", "FP", lv, [x_fr, y_fr, fz]);
        let (fm, _) = net.run(fp_tee, &[[x_fr, y_fp, fz]], "FP", "pipe", [fp_main, fp_main], lv, &fsheet);
        let mut prev = fm.last().copied().unwrap_or(fp_tee);
        let mut stops: Vec<(f64, Vec<(bool, f64)>, bool)> = Vec::new(); // x, branches (south?, x), corridor head
        for i in 0..nb {
            let (x0, wi) = (xs.get(i).copied().unwrap_or(0.0), bays.get(i).copied().unwrap_or(24.0));
            let mid = x0 + 0.5 * wi;
            let mut br = Vec::new();
            if i > 0 {
                br.push((true, mid));
            }
            if i + 1 < nb {
                br.push((false, mid));
            }
            stops.push((mid, br, false));
            stops.push((x0 + 0.75 * wi, Vec::new(), true));
            if i + 1 == nb {
                stops.push((x0 + 0.25 * wi, vec![(false, x0 + 0.25 * wi)], false));
                stops.push((x0 + 0.7 * wi, vec![(false, x0 + 0.7 * wi)], false));
            }
        }
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        // The planted sprinkler branch: up into the duct band to cross the supply main.
        let plant_fp_x = plant_fp
            .then(|| {
                stops
                    .iter()
                    .filter(|s| s.1.iter().any(|b| !b.0) && s.0 > x_m + 1.0 && s.0 < stops.last().map_or(0.0, |l| l.0) - 1.0)
                    .nth(1)
                    .map(|s| s.0)
            })
            .flatten()
            .filter(|_| lv == plant_level);
        let last_x = stops.last().map_or(0.0, |s| s.0);
        let first_x = stops.first().map_or(0.0, |s| s.0);
        // The main steps down along its run: its size, 2-1/2" past 60% of it, 2" to the last
        // branch; each step is a reducer between branches.
        let size_at = |x: f64| {
            if (x - last_x).abs() < 1e-6 {
                2.0
            } else if x > first_x + 0.6 * (last_x - first_x) {
                2.5
            } else {
                fp_main
            }
        };
        let mut size_prev = fp_main;
        let mut x_prev = x_fr;
        for (x, br, head) in &stops {
            let size = size_at(*x);
            if size != size_prev && *x - x_prev > 2.0 {
                let m = net.node("", "FP", lv, [(x_prev + *x) / 2.0, y_fp, fz]);
                net.link(prev, m, "FP", "pipe", [size_prev, size_prev], lv, &fsheet);
                prev = m;
            }
            let n = net.node("", "FP", lv, [*x, y_fp, fz]);
            net.link(prev, n, "FP", "pipe", [size, size], lv, &fsheet);
            prev = n;
            size_prev = size;
            x_prev = *x;
            if *head {
                let h = net.node("terminal", "FP", lv, [*x, y_fp, e + f.corr_clg]);
                net.link(n, h, "FP", "pipe", [1.0, 1.0], lv, &fsheet);
                let nid = net.nodes.get(h).map(|n| n.id.clone());
                let tid = add_term(&mut t, "sprinkler_head", "SH-1", "FP", lv, [*x, y_fp, e + f.corr_clg], [0.0, 0.0], 0.5, None, nid);
                if let Some(nd) = net.nodes.get_mut(h) {
                    nd.connects = Some(tid);
                }
            }
            for (south, bx) in br {
                let Some(rm) = room_at(&t, lv, *bx + 0.01, if *south { y1 / 2.0 } else { (y2 + y3) / 2.0 }) else { continue };
                let wall = if *south { y1 } else { y2 };
                let dir = if *south { -1.0 } else { 1.0 };
                let depth = rm.max_ft[1] - rm.min_ft[1];
                let clg = rm.ceiling.as_ref().map_or(f.office_clg, |c| c.height_ft);
                let toilet = rm.max_ft[0] - rm.min_ft[0] < tw * 0.75 && x_last0 <= rm.min_ft[0] + 1e-6;
                let fr: &[f64] = if toilet {
                    &[0.4]
                } else if depth >= 24.0 {
                    &[0.2, 0.5, 0.8]
                } else {
                    &[0.3, 0.75]
                };
                let bsize = if fr.len() >= 3 { 1.5 } else { 1.25 };
                let mut from = n;
                let mut bz = fz;
                if plant_fp_x.is_some_and(|px| (px - x).abs() < 1e-6) && !south {
                    // Up just off the main, across the supply main in the duct band.
                    let a = net.node("", "FP", lv, [*bx, y_fp + 0.5, fz]);
                    let s1 = net.link(from, a, "FP", "pipe", [bsize, bsize], lv, &fsheet);
                    bz = zc;
                    let b = net.node("", "FP", lv, [*bx, y_fp + 0.5, bz]);
                    let s2 = net.link(a, b, "FP", "pipe", [bsize, bsize], lv, &fsheet);
                    let c = net.node("", "FP", lv, [*bx, y2 + 1.0, bz]);
                    let s3 = net.link(b, c, "FP", "pipe", [bsize, bsize], lv, &fsheet);
                    planted.extend([s1, s2, s3].iter().filter_map(|i| net.segs.get(*i).map(|s| s.id.clone())));
                    from = c;
                }
                for (k, fy) in fr.iter().enumerate() {
                    let y = wall + dir * fy * depth;
                    let hn = net.node("", "FP", lv, [*bx, y, bz]);
                    let s = net.link(from, hn, "FP", "pipe", [bsize, bsize], lv, &fsheet);
                    if bz != fz && k == 0 {
                        planted.extend(net.segs.get(s).map(|s| s.id.clone()));
                    }
                    let h = net.node("terminal", "FP", lv, [*bx, y, e + clg]);
                    net.link(hn, h, "FP", "pipe", [1.0, 1.0], lv, &fsheet);
                    let nid = net.nodes.get(h).map(|n| n.id.clone());
                    let tid = add_term(&mut t, "sprinkler_head", "SH-1", "FP", lv, [*bx, y, e + clg], [0.0, 0.0], 0.5, None, nid);
                    if let Some(nd) = net.nodes.get_mut(h) {
                        nd.connects = Some(tid);
                    }
                    from = hn;
                }
            }
        }
        if lv == "1" {
            let svc = net.node("service", "FP", lv, [x_fr, y_fr, 0.5]);
            net.link(svc, fp_tee, "FP", "pipe", [fp_main.max(4.0), fp_main.max(4.0)], lv, &fsheet);
            fp_l1_tee = Some(fp_tee);
        } else if let Some(l1) = fp_l1_tee.take() {
            net.link(l1, fp_tee, "FP", "pipe", [fp_main.max(4.0), fp_main.max(4.0)], "1", "FP-101");
        }

        // Plumbing: risers in the chase; fixtures along the toilets' back wall.
        let fx_y = y3 - 1.0;
        let pz = e + f.pl_c;
        let fixtures_at = [(0.1, "water_closet", "WC-1"), (0.3, "lavatory", "LAV-1"), (0.7, "lavatory", "LAV-1"), (0.9, "water_closet", "WC-1")];
        let mut fixture_ids = Vec::new();
        for (fx, kind, tag) in fixtures_at {
            let x = x_last0 + fx * tw;
            let id = format!("F{:02}", t.fixtures.len() + 1);
            let room = room_at(&t, lv, x, fx_y).map(|r| r.number).unwrap_or_default();
            t.fixtures.push(Fixture {
                id: id.clone(),
                tag: tag.into(),
                kind: kind.into(),
                level: lv.into(),
                room,
                at_ft: [x, fx_y, e],
                nodes: Vec::new(),
            });
            fixture_ids.push((id, x, kind));
        }
        let fix_node = |net: &mut Net, t: &mut Truth, fid: &str, sys: &str, at: [f64; 3]| -> usize {
            let n = net.node("fixture", sys, lv, at);
            if let Some(nd) = net.nodes.get_mut(n) {
                nd.connects = Some(fid.to_string());
            }
            if let Some(fx) = t.fixtures.iter_mut().find(|f| f.id == fid) {
                fx.nodes.push(net.nodes.get(n).map(|n| n.id.clone()).unwrap_or_default());
            }
            n
        };
        // Branch lines at the toilets: per pipe k, north from its riser, west along the wall.
        let mut riser_feet: BTreeMap<&str, usize> = BTreeMap::new();
        for (k, sys, size, serves) in [(0usize, "CW", 1.0, &["water_closet", "lavatory"][..]), (1, "HW", 0.75, &["lavatory"][..])] {
            let (rx, ry) = chase(k);
            let yf = y3 - 1.5 - 0.2 * k as f64;
            let foot = net.node("", sys, lv, [rx, ry, pz]);
            riser_feet.insert(sys, foot);
            let (rn, _) = net.run(foot, &[[rx, yf, pz]], sys, "pipe", [size, size], lv, &psheet);
            let mut prev = rn.last().copied().unwrap_or(foot);
            let mut targets: Vec<&(String, f64, &str)> = fixture_ids.iter().filter(|f| serves.contains(&f.2)).collect();
            targets.sort_by(|a, b| b.1.total_cmp(&a.1));
            let last = targets.len().saturating_sub(1);
            for (j, (fid, x, kind)) in targets.iter().enumerate() {
                let n = net.node("", sys, lv, [*x, yf, pz]);
                let s = if j == last && sys == "CW" { 0.75 } else { size };
                net.link(prev, n, sys, "pipe", [s, s], lv, &psheet);
                let z = e + if *kind == "lavatory" { 2.0 } else { 1.5 };
                let fnode = fix_node(&mut net, &mut t, fid, sys, [*x, yf, z]);
                net.link(n, fnode, sys, "pipe", [0.5, 0.5], lv, &psheet);
                prev = n;
            }
            if sys == "HW" {
                // Hot water return from the far lavatory back to its riser.
                let (rx2, ry2) = chase(2);
                let yr = yf - 0.2;
                let x_far = targets.last().map_or(rx2, |t| t.1);
                let a = net.node("", "HWR", lv, [x_far, yr, pz]);
                net.link(prev, a, "HWR", "pipe", [0.5, 0.5], lv, &psheet);
                let foot = net.node("", "HWR", lv, [rx2, ry2, pz]);
                net.run(a, &[[rx2, yr, pz]], "HWR", "pipe", [0.5, 0.5], lv, &psheet);
                let corner = net.nodes.len() - 1;
                net.link(corner, foot, "HWR", "pipe", [0.5, 0.5], lv, &psheet);
                riser_feet.insert("HWR", foot);
            }
        }
        // Sanitary: drains under the floor to the stack (level 2 between the beams, level 1
        // under the slab, out to the site sewer).
        let (sx, sy) = chase(3);
        let slope = 0.25 / 12.0; // ft per ft
        let z_j = if lv == "1" { -2.0 } else { e - 1.6 };
        let mut pts: Vec<(f64, &str, String)> = fixture_ids.iter().map(|f| (f.1, f.2, f.0.clone())).collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let z_at = |x: f64| z_j + slope * (sx - x + (fx_y - sy));
        let mut prev: Option<usize> = None;
        for (x, kind, fid) in &pts {
            let n = net.node("", "SAN", lv, [*x, fx_y, z_at(*x)]);
            if let Some(p) = prev {
                let size = if kind == &"water_closet" || pts.iter().any(|q| q.0 < *x && q.1 == "water_closet") { 3.0 } else { 2.0 };
                net.link(p, n, "SAN", "pipe", [size, size], lv, &psheet);
            }
            let fnode = fix_node(&mut net, &mut t, fid, "SAN", [*x, fx_y, e]);
            let d = if *kind == "water_closet" { 3.0 } else { 1.5 };
            net.link(fnode, n, "SAN", "pipe", [d, d], lv, &psheet);
            prev = Some(n);
        }
        let Some(last_fix) = prev else { continue };
        let corner = net.node("", "SAN", lv, [sx, fx_y, z_at(sx)]);
        net.link(last_fix, corner, "SAN", "pipe", [4.0, 4.0], lv, &psheet);
        if lv == "1" {
            // Junction with the stack's line, out north to the sewer.
            let base = net.node("", "SAN", lv, [sx, sy, z_j + slope * 2.0 * (fx_y - sy)]);
            net.link(base, corner, "SAN", "pipe", [4.0, 4.0], lv, &psheet);
            let out = net.node("service", "SAN", lv, [sx, y3 + 5.0, z_j + slope * (fx_y - sy) - slope * (y3 + 5.0 - fx_y)]);
            net.link(corner, out, "SAN", "pipe", [4.0, 4.0], lv, &psheet);
            riser_feet.insert("SAN", base);
            san_base = Some(base);
        } else {
            let top = net.node("", "SAN", lv, [sx, sy, z_j]);
            net.link(corner, top, "SAN", "pipe", [4.0, 4.0], lv, &psheet);
            if let Some(base) = san_base.take() {
                net.link(base, top, "SAN", "pipe", [4.0, 4.0], "1", "P-101");
            }
        }
        if lv == "1" {
            for (sys, n) in &riser_feet {
                l1_feet.insert((*sys).to_string(), *n);
            }
        } else {
            for (sys, top) in &riser_feet {
                if *sys == "SAN" {
                    continue;
                }
                if let Some(&base) = l1_feet.get(*sys) {
                    let size = match *sys {
                        "CW" => 1.25,
                        "HW" => 1.0,
                        _ => 0.75,
                    };
                    net.link(base, *top, sys, "pipe", [size, size], "1", "P-101");
                }
            }
        }
    }

    // ---- Level 1 utilities: water heater, cold water service, hot water, gas ----------------
    if let Some(f1) = floors.first() {
        let pz = f1.elev + f1.pl_c;
        let x_wh = 0.85 * w0;
        let y_wh = 0.62 * d_s;
        let wh_h = 5.0;
        equip_n += 1;
        let wh_id = format!("E{equip_n:02}");
        let gal = r.pick::<f64>(&[50.0, 75.0, 100.0]);
        t.equipment.push(Equipment {
            id: wh_id.clone(),
            tag: "WH-1".into(),
            kind: "water_heater".into(),
            level: "1".into(),
            min_ft: [x_wh - 1.0, y_wh - 1.0, 0.0],
            max_ft: [x_wh + 1.0, y_wh + 1.0, wh_h],
            cfm: None,
            schedule: [
                ("TAG", "WH-1".to_string()),
                ("TYPE", "GAS, TANK".into()),
                ("CAPACITY (GAL)", format!("{gal:.0}")),
                ("INPUT (MBH)", format!("{:.0}", gal * 2.0)),
                ("SIZE (DIA x H)", "2'-0\" x 5'-0\"".into()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
            nodes: Vec::new(),
        });
        let conn = |net: &mut Net, t: &mut Truth, sys: &str, at: [f64; 3]| -> usize {
            let n = net.node("equipment", sys, "1", at);
            if let Some(nd) = net.nodes.get_mut(n) {
                nd.connects = Some(wh_id.clone());
            }
            if let Some(eq) = t.equipment.iter_mut().find(|e| e.id == wh_id) {
                eq.nodes.push(net.nodes.get(n).map(|n| n.id.clone()).unwrap_or_default());
            }
            n
        };
        let y_cw = y1 + 0.55 * cw;
        let y_hw = y1 + 0.67 * cw;
        let y_hwr = y1 + 0.79 * cw;
        let feet = &l1_feet;
        // Cold water: service in the south wall, a tee down to the heater, then to the chase.
        let (cx, _) = chase(0);
        let svc = net.node("service", "CW", "1", [x_wh + 0.4, 0.0, pz]);
        let tee = net.node("", "CW", "1", [x_wh + 0.4, y_wh, pz]);
        net.link(svc, tee, "CW", "pipe", [cw_main, cw_main], "1", "P-101");
        let wc = conn(&mut net, &mut t, "CW", [x_wh + 0.4, y_wh, wh_h]);
        net.link(tee, wc, "CW", "pipe", [1.0, 1.0], "1", "P-101");
        let (cwn, cws) = net.run(tee, &[[x_wh + 0.4, y_cw, pz], [cx, y_cw, pz]], "CW", "pipe", [cw_main, cw_main], "1", "P-101");
        if let (Some(&end), Some(&foot)) = (cwn.last(), feet.get("CW")) {
            net.link(end, foot, "CW", "pipe", [cw_main, cw_main], "1", "P-101");
        }
        // Hot water out of the heater to the chase, raised over one girder when planted.
        let (hx, _) = chase(1);
        let hc = conn(&mut net, &mut t, "HW", [x_wh - 0.4, y_wh, wh_h]);
        let hw_size = 1.25;
        let mut pts = vec![[x_wh - 0.4, y_wh, pz], [x_wh - 0.4, y_hw, pz]];
        let girder_x = xs.iter().copied().filter(|x| *x > x_wh + 3.0 && *x < hx - 4.0).nth(if nb > 4 { 1 } else { 0 });
        let mut raised = Vec::new();
        if plant_struct && let Some(gx) = girder_x {
            let hi = f1.elev + f1.girder_bob + 3.0 / 12.0;
            let len_before = pts.len();
            pts.extend([[gx - 2.5, y_hw, pz], [gx - 2.5, y_hw, hi], [gx + 2.5, y_hw, hi], [gx + 2.5, y_hw, pz]]);
            // The segments into the rise, along the top and down again.
            raised = (len_before + 1..len_before + 4).collect();
        }
        pts.push([hx, y_hw, pz]);
        let (hwn, hws) = net.run(hc, &pts, "HW", "pipe", [hw_size, hw_size], "1", "P-101");
        for k in &raised {
            if let Some(s) = hws.get(*k).and_then(|i| net.segs.get(*i)) {
                planted.push(s.id.clone());
            }
        }
        if let (Some(&end), Some(&foot)) = (hwn.last(), feet.get("HW")) {
            net.link(end, foot, "HW", "pipe", [hw_size, hw_size], "1", "P-101");
        }
        // Hot water return from the chase back to the heater.
        let (rx, _) = chase(2);
        if let Some(&foot) = feet.get("HWR") {
            let (rn, _) =
                net.run(foot, &[[rx, y_hwr, pz], [x_wh - 0.8, y_hwr, pz], [x_wh - 0.8, y_wh, pz]], "HWR", "pipe", [0.75, 0.75], "1", "P-101");
            let rc = conn(&mut net, &mut t, "HWR", [x_wh - 0.8, y_wh, 4.0]);
            if let Some(&end) = rn.last() {
                net.link(end, rc, "HWR", "pipe", [0.75, 0.75], "1", "P-101");
            }
        }
        // Gas: from the meter outside the south wall, to the AHU and the heater.
        let x_g = 0.72 * w0;
        equip_n += 1;
        let gm_id = format!("E{equip_n:02}");
        t.equipment.push(Equipment {
            id: gm_id.clone(),
            tag: "GM-1".into(),
            kind: "gas_meter".into(),
            level: "1".into(),
            min_ft: [x_g - 0.75, -2.5, 0.0],
            max_ft: [x_g + 0.75, -1.5, 3.0],
            cfm: None,
            schedule: [("TAG", "GM-1".to_string()), ("SERVICE", "NATURAL GAS, 7\" WC".into())].into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            nodes: Vec::new(),
        });
        let meter = net.node("equipment", "G", "1", [x_g, -2.0, 3.0]);
        if let Some(n) = net.nodes.get_mut(meter) {
            n.connects = Some(gm_id.clone());
        }
        if let Some(eq) = t.equipment.iter_mut().find(|e| e.id == gm_id) {
            eq.nodes.push(net.nodes.get(meter).map(|n| n.id.clone()).unwrap_or_default());
        }
        let ahu_mid = ahu_yc;
        let (gn, _) = net.run(meter, &[[x_g, -2.0, pz], [x_g, ahu_mid, pz]], "G", "pipe", [1.25, 1.25], "1", "P-101");
        let g_tee = gn.last().copied().unwrap_or(meter);
        let (ga, _) = net.run(g_tee, &[[0.67 * w0, ahu_mid, pz]], "G", "pipe", [0.75, 0.75], "1", "P-101");
        let ahu1 = t.equipment.iter().find(|e| e.tag == "AHU-1").map(|e| e.id.clone()).unwrap_or_default();
        let ga_c = net.node("equipment", "G", "1", [0.67 * w0, ahu_mid, ahu_h * 0.6]);
        if let Some(n) = net.nodes.get_mut(ga_c) {
            n.connects = Some(ahu1.clone());
        }
        if let Some(eq) = t.equipment.iter_mut().find(|e| e.id == ahu1) {
            eq.nodes.push(net.nodes.get(ga_c).map(|n| n.id.clone()).unwrap_or_default());
        }
        if let Some(&end) = ga.last() {
            net.link(end, ga_c, "G", "pipe", [0.75, 0.75], "1", "P-101");
        }
        let (gw, _) = net.run(g_tee, &[[x_g, y_wh - 0.6, pz], [x_wh, y_wh - 0.6, pz]], "G", "pipe", [1.0, 1.0], "1", "P-101");
        let gc = conn(&mut net, &mut t, "G", [x_wh, y_wh - 0.6, 4.0]);
        if let Some(&end) = gw.last() {
            net.link(end, gc, "G", "pipe", [0.75, 0.75], "1", "P-101");
        }
        let _ = (cws, hws);
    }

    // ---- Roof: exhaust fan on the riser, vent through the roof ----------------------------
    let roof = elev("R");
    if let Some(&l2) = net
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.system == "EA" && n.level == "2" && (n.at_ft[0] - x_er).abs() < 1e-6 && (n.at_ft[1] - y_er).abs() < 1e-6)
        .map(|(i, _)| i)
        .collect::<Vec<_>>()
        .last()
    {
        equip_n += 1;
        let ef_id = format!("E{equip_n:02}");
        let fan = 2.0 * ea_cfm_per_level;
        t.equipment.push(Equipment {
            id: ef_id.clone(),
            tag: "EF-1".into(),
            kind: "exhaust_fan".into(),
            level: "R".into(),
            min_ft: [x_er - 1.0, y_er - 1.0, roof],
            max_ft: [x_er + 1.0, y_er + 1.0, roof + 2.5],
            cfm: Some(fan),
            schedule: [("TAG", "EF-1".to_string()), ("TYPE", "ROOF UPBLAST".into()), ("CFM", format!("{fan:.0}")), ("SERVES", "TOILETS".into())]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            nodes: Vec::new(),
        });
        let c = net.node("equipment", "EA", "R", [x_er, y_er, roof]);
        if let Some(n) = net.nodes.get_mut(c) {
            n.connects = Some(ef_id.clone());
        }
        if let Some(eq) = t.equipment.iter_mut().find(|e| e.id == ef_id) {
            eq.nodes.push(net.nodes.get(c).map(|n| n.id.clone()).unwrap_or_default());
        }
        ea_riser_segs.push(net.link(l2, c, "EA", "rect", [12.0, 10.0], "2", "M-102"));
    }
    let (vx, vy) = chase(4);
    let vb = net.node("cap", "V", "1", [vx, vy, 3.0]);
    let vt = net.node("vtr", "V", "R", [vx, vy, roof + 1.0]);
    net.link(vb, vt, "V", "pipe", [3.0, 3.0], "1", "P-101");

    // ---- Node kinds, risers ----------------------------------------------------------------
    finish_kinds(&mut net);
    t.nodes = net.nodes;
    t.segments = net.segs;
    t.risers = risers(&t);
    Design { truth: t, planted }
}

/// Fittings from the geometry where the kind was left to it: ends are caps, two collinear
/// segments of different sizes a transition (rectangular) or reducer, a turn an elbow, three a
/// tee, four a cross.
fn finish_kinds(net: &mut Net) {
    let segs: BTreeMap<String, (usize, [[f64; 3]; 2], String, [f64; 2])> =
        net.segs.iter().enumerate().map(|(i, s)| (s.id.clone(), (i, s.path_ft, s.shape.clone(), s.size_in))).collect();
    for n in &mut net.nodes {
        if !n.kind.is_empty() {
            continue;
        }
        let dirs: Vec<([f64; 3], &str, [f64; 2])> = n
            .segments
            .iter()
            .filter_map(|id| segs.get(id))
            .map(|(_, p, shape, size)| {
                let other = if dist(p[0], n.at_ft) < 1e-6 { p[1] } else { p[0] };
                let v = [other[0] - n.at_ft[0], other[1] - n.at_ft[1], other[2] - n.at_ft[2]];
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
                ([v[0] / l, v[1] / l, v[2] / l], shape.as_str(), *size)
            })
            .collect();
        n.kind = match dirs.as_slice() {
            [] | [_] => "cap".into(),
            [a, b] => {
                let dot = a.0[0] * b.0[0] + a.0[1] * b.0[1] + a.0[2] * b.0[2];
                // Sloped drains bend by a hair at a size change: still collinear.
                if dot < -0.999 {
                    if a.2 == b.2 && a.1 == b.1 {
                        "coupling".into()
                    } else if a.1 == "rect" || b.1 == "rect" {
                        "transition".into()
                    } else {
                        "reducer".into()
                    }
                } else {
                    "elbow".into()
                }
            }
            [_, _, _] => "tee".into(),
            _ => "cross".into(),
        };
    }
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Vertical segments joining nodes of different levels, grouped by system and plan position:
/// the risers and stacks. Each level's plan marks it: `up` on the lowest level (it turns up from
/// that plan), `down` on the highest (it turns down from it), `through` in between.
fn risers(t: &Truth) -> Vec<Riser> {
    let order = |code: &str| t.building.levels.iter().position(|l| l.code == code).unwrap_or(0);
    let mut out: Vec<Riser> = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for s in t.segments.iter().filter(|s| s.vertical) {
        let (Some(a), Some(b)) = (t.node(&s.from), t.node(&s.to)) else { continue };
        if a.level == b.level {
            continue;
        }
        let (lo, hi) = (s.path_ft[0][2].min(s.path_ft[1][2]), s.path_ft[0][2].max(s.path_ft[1][2]));
        let (la, lb) = (order(&a.level).min(order(&b.level)), order(&a.level).max(order(&b.level)));
        let at = [s.path_ft[0][0], s.path_ft[0][1]];
        if let Some(k) = out.iter().position(|r| r.system == s.system && (r.at_ft[0] - at[0]).abs() < 1e-6 && (r.at_ft[1] - at[1]).abs() < 1e-6) {
            if let (Some(r), Some(sp)) = (out.get_mut(k), spans.get_mut(k)) {
                r.bottom_ft = r.bottom_ft.min(lo);
                r.top_ft = r.top_ft.max(hi);
                r.segments.push(s.id.clone());
                *sp = (sp.0.min(la), sp.1.max(lb));
            }
        } else {
            out.push(Riser {
                id: format!("RS-{}", out.len() + 1),
                system: s.system.clone(),
                at_ft: at,
                size_in: s.size_in,
                bottom_ft: lo,
                top_ft: hi,
                segments: vec![s.id.clone()],
                shown: Vec::new(),
            });
            spans.push((la, lb));
        }
    }
    for (r, (lo, hi)) in out.iter_mut().zip(spans) {
        r.segments.sort_by(|a, b| {
            let z = |id: &String| t.segment(id).map_or(0.0, |s| s.path_ft[0][2].min(s.path_ft[1][2]));
            z(a).total_cmp(&z(b))
        });
        let printed = t.system(&r.system).map_or(r.system.clone(), |s| s.printed.clone());
        for (k, l) in t.building.levels.iter().enumerate().take(hi + 1).skip(lo) {
            // The roof has no plan in the set.
            if l.code == "R" {
                continue;
            }
            let (symbol, note) = if k == lo {
                ("up", format!("{printed} UP"))
            } else if k == hi {
                ("down", format!("{printed} DN"))
            } else {
                ("through", format!("{printed} UP/DN"))
            };
            let to_roof = t.building.levels.get(hi).is_some_and(|l| l.code == "R");
            let note = match (r.system.as_str(), to_roof, k + 1 == hi) {
                ("EA", true, true) => "UP TO EF-1".to_string(),
                ("V", true, true) => format!("{printed} UP THRU ROOF"),
                _ => note,
            };
            r.shown.push(RiserMark { level: l.code.clone(), symbol: symbol.into(), note });
        }
    }
    out
}
