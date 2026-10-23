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
        "rect" | "tray" => size[1],
        "round" => size[0],
        _ => pipe_od(size[0]),
    }
}

/// The width (across, on the plan) of a segment's cross-section, inches.
pub fn width_in(shape: &str, size: [f64; 2]) -> f64 {
    match shape {
        "rect" | "tray" => size[0],
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

/// The pipe schedule (light hazard, steel) the fire protection sheets print: the smallest size
/// that may carry `heads` sprinkler heads.
pub const PIPE_SCHEDULE: &[(f64, usize)] = &[(1.0, 2), (1.25, 3), (1.5, 5), (2.0, 10), (2.5, 30), (3.0, 60)];

/// Branch pipe size for a number of heads downstream, inches.
pub fn head_pipe(heads: usize) -> f64 {
    PIPE_SCHEDULE.iter().find(|(_, n)| heads <= *n).map_or(4.0, |(d, _)| *d)
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
            Height { bottom_ft: r6(c - d / 2.0), top_ft: r6(c + d / 2.0), centre_ft: r6(c), note: None, source: HeightSource::Rule }
        });
        let i = self.segs.len();
        let id = format!("S{:04}", i + 1);
        let below_floor = !vertical && pa[2].max(pb[2]) < elev;
        self.segs.push(Segment {
            id: id.clone(),
            system: system.into(),
            kind: match shape {
                "pipe" | "tray" | "conduit" => shape.into(),
                _ => "duct".into(),
            },
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
    design_scheduled(seed, st, r, 4.0, 4.0)
}

/// Like [`design`] but with custom sprinkler head schedule values (inches).
pub fn design_scheduled(seed: u64, st: &Style, r: &mut Rng, fp_sprig_in: f64, fp_sidewall_below_in: f64) -> Design {
    let mut t =
        Truth { schema: SCHEMA.into(), seed, fp_sprig_in: Some(fp_sprig_in), fp_sidewall_below_in: Some(fp_sidewall_below_in), ..Truth::default() };
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

    // ---- Slabs and openings ----------------------------------------------------------------
    let deck_in = 1.5;
    for lv in ["2", "R"] {
        let is_roof = lv == "R";
        let (kind, conc) = if is_roof { ("roof", 0.0) } else { ("composite", t.building.slab_in) };
        let top = elev(lv);
        t.building.slabs.push(Slab { id: format!("SL-{lv}"), level: lv.into(), kind: kind.into(), concrete_in: conc, deck_in, top_ft: r6(top) });
    }
    // A plumbing chase / shaft at the east end of the building.
    let shaft_x0 = xs.get(nb.saturating_sub(1)).copied().unwrap_or(0.0) + 1.0;
    let shaft_x1 = xs.get(nb).copied().unwrap_or(shaft_x0 + 3.0);
    let shaft_y0 = ys.get(1).copied().unwrap_or(0.0);
    let shaft_y1 = ys.get(2).copied().unwrap_or(shaft_y0 + 4.0);
    for lv in ["2", "R"] {
        t.building.openings.push(Opening {
            id: format!("OP-{lv}-SHAFT"),
            level: lv.into(),
            kind: "shaft".into(),
            min_ft: [r6(shaft_x0), r6(shaft_y0)],
            max_ft: [r6(shaft_x1), r6(shaft_y1)],
            label: Some("PLUMBING CHASE".into()),
        });
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
    let two_by_two = seed.is_multiple_of(3);
    let act_grid: [f64; 2] = if two_by_two { [2.0, 2.0] } else { [2.0, 4.0] };
    // Soffits: a dropped GYP section along the corridor where the main duct runs, on some seeds.
    // From a random stream of their own, so the rest of a set stays as it was before soffits.
    let mut sr = Rng::new(seed ^ 0x534F_4646_4954_5300).fork(6);
    let has_soffits = sr.chance(0.4);
    let soffit_drop: f64 = sr.pick(&[0.5, 1.0, 1.5]); // how far below the corridor ceiling
    let mut soffit_n = 0usize;
    for f in &floors {
        let n0: usize = if f.code == "1" { 100 } else { 200 };
        let act = |h: f64| Some(Ceiling { height_ft: h, kind: "ACT".into(), tag: String::new(), grid_ft: Some(act_grid), height_hidden: false });
        let plenum = |h: f64| -> Option<f64> {
            let p = f.girder_bob - h;
            if p > 0.01 { Some(r6(p)) } else { None }
        };
        let add = |t: &mut Truth, num: usize, name: &str, min: [f64; 2], max: [f64; 2], ceiling: Option<Ceiling>| {
            let mut ceiling = ceiling;
            if let Some(c) = ceiling.as_mut() {
                c.tag = st.ceiling_text(c.height_ft, &c.kind).join(" ");
            }
            let plenum_ft = ceiling.as_ref().and_then(|c| plenum(c.height_ft));
            t.building.rooms.push(Room {
                id: format!("R{num}"),
                level: f.code.clone(),
                number: format!("{num}"),
                name: name.into(),
                min_ft: min,
                max_ft: max,
                ceiling,
                plenum_ft,
            });
        };
        add(&mut t, n0, "CORRIDOR", [0.0, y1], [width, y2], act(f.corr_clg));
        // Soffits in the corridor (one per floor) where the main duct runs.
        if has_soffits {
            soffit_n += 1;
            let sh = f.corr_clg - soffit_drop;
            let sw = cw * 0.6; // a bit narrower than the corridor
            let sy0 = y1 + (y2 - y1 - sw) / 2.0;
            let sx0 = xs.get(1).copied().unwrap_or(0.0); // starts past the mech room
            let sx1 = xs.last().copied().unwrap_or(width); // to the far end
            t.rcp_soffits.push(Soffit {
                id: format!("SF{soffit_n:03}"),
                level: f.code.clone(),
                room: format!("{n0}"),
                height_ft: r6(sh),
                width_ft: r6(sw),
                length_ft: r6(sx1 - sx0),
                min_ft: [r6(sx0), r6(sy0)],
                max_ft: [r6(sx1), r6(sy0 + sw)],
                tag: format!("SOFFIT @ {}", feet_inches(sh)),
                ..Soffit::default()
            });
        }
        for i in 0..nb {
            let (x0, x1) = (xs.get(i).copied().unwrap_or(0.0), xs.get(i + 1).copied().unwrap_or(0.0));
            let south = n0 + 1 + i;
            if i == 0 {
                add(&mut t, south, "MECH", [x0, 0.0], [x1, y1], None);
            } else {
                let name = r.pick(&pool);
                let c = if name == "STORAGE" || name == "FILES" {
                    Some(Ceiling { height_ft: f.office_clg, kind: "GYP".into(), tag: String::new(), grid_ft: None, height_hidden: false })
                } else {
                    act(f.office_clg)
                };
                add(&mut t, south, name, [x0, 0.0], [x1, y1], c);
            }
            let north = n0 + 1 + nb + i;
            if i + 1 == nb {
                let mid = (x0 + x1) / 2.0;
                let gyp = Some(Ceiling { height_ft: f.toilet_clg, kind: "GYP".into(), tag: String::new(), grid_ft: None, height_hidden: false });
                add(&mut t, north, toilets.0, [x0, y2], [mid, y3], gyp.clone());
                add(&mut t, north + 1, toilets.1, [mid, y2], [x1, y3], gyp);
            } else {
                add(&mut t, north, r.pick(&pool), [x0, y2], [x1, y3], act(f.office_clg));
            }
        }
    }

    // ---- Casework (break rooms, toilets, conference rooms) --------------------------------
    {
        let mut cw_id = 0usize;
        let mut cw = |t: &mut Truth,
                      kind: &str,
                      tag: &str,
                      level: &str,
                      room: &str,
                      min: [f64; 2],
                      max: [f64; 2],
                      height_in: f64,
                      depth_in: f64,
                      spec: Option<&str>| {
            cw_id += 1;
            t.casework.push(CaseworkItem {
                id: format!("cw-{cw_id}"),
                kind: kind.into(),
                tag: tag.into(),
                level: level.into(),
                room: room.into(),
                min_ft: min,
                max_ft: max,
                height_in,
                depth_in,
                spec_section: spec.map(|s| s.into()),
            });
        };
        for rm in &t.building.rooms.clone() {
            let is_break = rm.name == "BREAK ROOM";
            let is_toilet = rm.name.contains("TOILET") || rm.name.contains("REST") || rm.name.contains("MEN") || rm.name.contains("WOMEN");
            let is_conf = rm.name == "CONFERENCE";
            let rw = rm.max_ft[0] - rm.min_ft[0];
            let rh = rm.max_ft[1] - rm.min_ft[1];
            if is_break {
                // Base cabinets along the south wall, counter on top, uppers above.
                let cab_len = (rw - 2.0).clamp(4.0, 10.0);
                let x0 = rm.min_ft[0] + 1.0;
                let x1 = x0 + cab_len;
                let depth = 2.0; // 24" = 2 ft
                let y0 = rm.min_ft[1] + 0.5;
                let y1 = y0 + depth;
                cw(&mut t, "base_cabinet", "BC-1", &rm.level, &rm.number, [x0, y0], [x1, y1], 34.0, 24.0, Some("11 40 00"));
                cw(&mut t, "counter", "CT-1", &rm.level, &rm.number, [x0, y0], [x1, y1], 36.0, 25.0, Some("06 41 16"));
                cw(&mut t, "upper_cabinet", "UC-1", &rm.level, &rm.number, [x0, y0], [x1, y1], 42.0, 14.0, Some("11 40 00"));
                // Backsplash between counter and upper
                cw(&mut t, "backsplash", "BS-1", &rm.level, &rm.number, [x0, y0], [x1, y0 + 0.33], 18.0, 4.0, Some("09 30 00"));
                // Appliances: fridge at one end, microwave above counter
                cw(&mut t, "appliance", "RF-1", &rm.level, &rm.number, [x1 + 0.2, y0], [x1 + 3.0, y1], 70.0, 28.0, Some("11 40 00"));
                // Sink (plumbing fixture) in the counter run
                let sink_x = (x0 + x1) / 2.0;
                cw(&mut t, "counter", "SK-1", &rm.level, &rm.number, [sink_x - 1.0, y0], [sink_x + 1.0, y1], 36.0, 22.0, Some("22 40 00"));
            }
            if is_toilet {
                // Vanity along the north wall with backsplash
                let van_len = (rw - 2.0).clamp(3.0, 8.0);
                let x0 = rm.min_ft[0] + 1.0;
                let x1 = x0 + van_len;
                let depth = 22.0 / 12.0; // 22" in feet
                let y1 = rm.max_ft[1] - 0.5;
                let y0 = y1 - depth;
                cw(&mut t, "vanity", "V-1", &rm.level, &rm.number, [x0, y0], [x1, y1], 34.0, 22.0, Some("06 41 16"));
                cw(&mut t, "backsplash", "BS-2", &rm.level, &rm.number, [x0, y1 - 0.33], [x1, y1], 6.0, 4.0, Some("09 30 00"));
            }
            if is_conf && rw > 15.0 && rh > 12.0 {
                // Credenza / counter along south wall
                let cab_len = (rw * 0.4).min(8.0);
                let x0 = rm.min_ft[0] + (rw - cab_len) / 2.0;
                let x1 = x0 + cab_len;
                let y0 = rm.min_ft[1] + 0.5;
                let y1 = y0 + 20.0 / 12.0;
                cw(&mut t, "base_cabinet", "CR-1", &rm.level, &rm.number, [x0, y0], [x1, y1], 30.0, 20.0, Some("06 41 16"));
                cw(&mut t, "counter", "CT-2", &rm.level, &rm.number, [x0, y0], [x1, y1], 30.0, 20.0, Some("06 41 16"));
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
            mount: None,
        });
        id
    };
    // Sprinklers draw from a stream of their own, so the rest of each set doesn't move.
    let mut sprk = Rng::new(seed ^ 0x5350_524B_4C52);
    // Heads offset from their branch line on armovers; sidewall heads along the corridor.
    let armovers = sprk.chance(0.5);
    let sidewall = sprk.chance(0.4);
    let sprig = t.fp_sprig_in.unwrap_or(4.0) / 12.0;
    let sw_offset = t.fp_sidewall_below_in.unwrap_or(4.0) / 12.0;
    let set_mount = |t: &mut Truth, id: &str, mount: &str, tag: &str| {
        if let Some(tm) = t.terminals.iter_mut().find(|x| x.id == id) {
            tm.mount = Some(mount.into());
            tm.type_tag = tag.into();
        }
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
                // Pendent in the corridor ceiling, or sidewall on its north wall (an arm out to
                // the wall, the head 4" under the ceiling).
                let (from, at, mount, tag) = if sidewall {
                    let wy = y2 - 0.33;
                    let a = net.node("", "FP", lv, [*x, wy, fz]);
                    net.link(n, a, "FP", "pipe", [1.0, 1.0], lv, &fsheet);
                    (a, [*x, wy, e + f.corr_clg - sw_offset], "sidewall", "SH-3")
                } else {
                    (n, [*x, y_fp, e + f.corr_clg], "pendent", "SH-1")
                };
                let h = net.node("terminal", "FP", lv, at);
                net.link(from, h, "FP", "pipe", [1.0, 1.0], lv, &fsheet);
                let nid = net.nodes.get(h).map(|n| n.id.clone());
                let tid = add_term(&mut t, "sprinkler_head", tag, "FP", lv, at, [0.0, 0.0], 0.5, None, nid);
                set_mount(&mut t, &tid, mount, tag);
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
                // Pipe schedule sizing (light hazard): each piece by the heads downstream of it.
                let bsize = head_pipe(fr.len());
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
                // Armovers go toward the nearer side wall of the room, alternating in big rooms.
                let toward_west = *bx - rm.min_ft[0] <= rm.max_ft[0] - *bx;
                for (k, fy) in fr.iter().enumerate() {
                    let y = wall + dir * fy * depth;
                    let hn = net.node("", "FP", lv, [*bx, y, bz]);
                    let piece = head_pipe(fr.len() - k);
                    let s = net.link(from, hn, "FP", "pipe", [piece, piece], lv, &fsheet);
                    if bz != fz && k == 0 {
                        planted.extend(net.segs.get(s).map(|s| s.id.clone()));
                    }
                    let mut drop_from = hn;
                    let mut hx = *bx;
                    if armovers {
                        let west = if fr.len() > 1 { (k % 2 == 0) == toward_west } else { toward_west };
                        hx = *bx + if west { -1.5 } else { 1.5 };
                        let a = net.node("", "FP", lv, [hx, y, bz]);
                        let s = net.link(hn, a, "FP", "pipe", [1.0, 1.0], lv, &fsheet);
                        if bz != fz {
                            planted.extend(net.segs.get(s).map(|s| s.id.clone()));
                        }
                        drop_from = a;
                    }
                    let h = net.node("terminal", "FP", lv, [hx, y, e + clg]);
                    net.link(drop_from, h, "FP", "pipe", [1.0, 1.0], lv, &fsheet);
                    let nid = net.nodes.get(h).map(|n| n.id.clone());
                    let tid = add_term(&mut t, "sprinkler_head", "SH-1", "FP", lv, [hx, y, e + clg], [0.0, 0.0], 0.5, None, nid);
                    set_mount(&mut t, &tid, "pendent", "SH-1");
                    if let Some(nd) = net.nodes.get_mut(h) {
                        nd.connects = Some(tid);
                    }
                    from = hn;
                }
            }
        }
        // The mechanical room is open to structure: a branch off the riser along its south side
        // with upright heads on 4" sprigs.
        let y_mb = 0.12 * d_s;
        let (bn, _) = net.run(fp_tee, &[[x_fr, y_mb, fz]], "FP", "pipe", [1.25, 1.25], lv, &fsheet);
        let mut from = bn.last().copied().unwrap_or(fp_tee);
        for (k, hxf) in [0.3, 0.55, 0.8].iter().enumerate() {
            let hx = hxf * w0;
            let n = net.node("", "FP", lv, [hx, y_mb, fz]);
            let size = head_pipe(3 - k);
            net.link(from, n, "FP", "pipe", [size, size], lv, &fsheet);
            let at = [hx, y_mb, fz + sprig];
            let h = net.node("terminal", "FP", lv, at);
            net.link(n, h, "FP", "pipe", [1.0, 1.0], lv, &fsheet);
            let nid = net.nodes.get(h).map(|n| n.id.clone());
            let tid = add_term(&mut t, "sprinkler_head", "SH-2", "FP", lv, at, [0.0, 0.0], 0.5, None, nid);
            set_mount(&mut t, &tid, "upright", "SH-2");
            if let Some(nd) = net.nodes.get_mut(h) {
                nd.connects = Some(tid);
            }
            from = n;
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
                wall: None,
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
            // An in-line recirculation pump on the return, a few feet before the corner to the
            // heater (when the run along the corridor is long enough to hold it).
            let corner = x_wh - 0.8;
            let dir = if rx >= corner { 1.0 } else { -1.0 };
            let (rn, _) = if (rx - corner).abs() > 6.0 {
                let (x_in, x_out) = (corner + dir * 3.5, corner + dir * 2.5);
                let (pn, _) = net.run(foot, &[[rx, y_hwr, pz], [x_in, y_hwr, pz]], "HWR", "pipe", [0.75, 0.75], "1", "P-101");
                let p_in = pn.last().copied().unwrap_or(foot);
                equip_n += 1;
                let cp_id = format!("E{equip_n:02}");
                if let Some(n) = net.nodes.get_mut(p_in) {
                    n.kind = "equipment".into();
                    n.connects = Some(cp_id.clone());
                }
                let p_out = net.node("equipment", "HWR", "1", [x_out, y_hwr, pz]);
                if let Some(n) = net.nodes.get_mut(p_out) {
                    n.connects = Some(cp_id.clone());
                }
                // A small body round the pipe (the band between the sprinklers and the ceilings
                // is tight).
                let (zl, zh) = (pz - 2.5 / 12.0, pz + 2.5 / 12.0);
                t.equipment.push(Equipment {
                    id: cp_id,
                    tag: "CP-1".into(),
                    kind: "pump".into(),
                    level: "1".into(),
                    min_ft: [r6(x_in.min(x_out)), r6(y_hwr - 0.25), r6(zl)],
                    max_ft: [r6(x_in.max(x_out)), r6(y_hwr + 0.25), r6(zh)],
                    cfm: None,
                    schedule: [
                        ("TAG", "CP-1".to_string()),
                        ("TYPE", "IN-LINE CIRCULATOR".into()),
                        ("SERVICE", "HOT WATER RECIRC".into()),
                        ("GPM", "4".into()),
                        ("HEAD (FT)", "10".into()),
                        ("SIZE (L x W x H)", "1'-0\" x 0'-6\" x 0'-5\"".into()),
                    ]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
                    nodes: [p_in, p_out].iter().filter_map(|n| net.nodes.get(*n).map(|n| n.id.clone())).collect(),
                });
                net.run(p_out, &[[corner, y_hwr, pz], [corner, y_wh, pz]], "HWR", "pipe", [0.75, 0.75], "1", "P-101")
            } else {
                net.run(foot, &[[rx, y_hwr, pz], [corner, y_hwr, pz], [corner, y_wh, pz]], "HWR", "pipe", [0.75, 0.75], "1", "P-101")
            };
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

        // The condensing unit for level 2's air handler (DX cooling), on a pad outside the south
        // wall, west of the intake louver (clear of its tag). Its refrigerant lines aren't drawn.
        equip_n += 1;
        let cu_x = 0.1 * w0;
        t.equipment.push(Equipment {
            id: format!("E{equip_n:02}"),
            tag: "CU-1".into(),
            kind: "condensing_unit".into(),
            level: "1".into(),
            min_ft: [r6(cu_x - 1.5), -3.75, 0.0],
            max_ft: [r6(cu_x + 1.5), -1.25, 2.75],
            cfm: None,
            schedule: [
                ("TAG", "CU-1".to_string()),
                ("SERVES", "AHU-2".into()),
                ("TONS", format!("{:.1}", total_cfm.get("2").copied().unwrap_or(4000.0) / 400.0)),
                ("REFRIGERANT", "R-454B".into()),
                ("SIZE (L x W x H)", "3'-0\" x 2'-6\" x 2'-9\"".into()),
                ("WEIGHT (LB)", "310".into()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
            nodes: Vec::new(),
        });
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

    // ---- Electrical -----------------------------------------------------------------------
    elec_design(&mut t, &floors, &xs, y1, y2, y3, cw, r);
    // Its own random stream: the rest of each set is drawn as before.
    gear_design(&mut t, &floors, &xs, &mut Rng::new(seed ^ 0x6EA2_0290));

    // ---- Cable tray and feeder conduit (coordination trades) ----------------------------------
    tray_design(&mut net, &mut t, &floors, &xs, y1, cw, w0, x_last0, cw_main, seed);

    // ---- Node kinds, risers ----------------------------------------------------------------
    finish_kinds(&mut net);
    t.nodes = net.nodes;
    t.segments = net.segs;
    t.risers = risers(&t);

    // ---- Walls, devices, rough-ins, in-wall coordination ----------------------------------
    generate_walls(&mut t);
    assign_fixture_walls(&mut t);
    let ri_rules = RoughInRules::default_rules();
    generate_rough_ins(&mut t, &ri_rules);
    let plant_inwall = r.chance(0.7);
    let plant_inwall_level = r.pick(&["1", "2"]);
    generate_devices(&mut t, &mut planted, plant_inwall, plant_inwall_level, &ri_rules, r);
    // ---- Architectural sections and elevations (for callouts, Q306) -------------------------
    let mid_bay = width / 2.0;
    let mid_depth = y3 / 2.0;
    t.sections = vec![
        SectionView {
            id: "sect-1".into(),
            name: "BUILDING SECTION A".into(),
            cut_from_ft: [mid_bay, 0.0],
            cut_to_ft: [mid_bay, y3],
            looking: "west".into(),
            sheet: String::new(),
        },
        SectionView {
            id: "sect-2".into(),
            name: "BUILDING SECTION B".into(),
            cut_from_ft: [0.0, mid_depth],
            cut_to_ft: [width, mid_depth],
            looking: "north".into(),
            sheet: String::new(),
        },
    ];
    t.elevations = vec![
        ElevationView { id: "elev-north".into(), name: "NORTH ELEVATION".into(), direction: "north".into(), sheet: String::new(), room: None },
        ElevationView { id: "elev-south".into(), name: "SOUTH ELEVATION".into(), direction: "south".into(), sheet: String::new(), room: None },
        ElevationView { id: "elev-east".into(), name: "EAST ELEVATION".into(), direction: "east".into(), sheet: String::new(), room: None },
        ElevationView { id: "elev-west".into(), name: "WEST ELEVATION".into(), direction: "west".into(), sheet: String::new(), room: None },
    ];

    Design { truth: t, planted }
}

// ========================================================================================
// In-wall coordination: walls, devices, rough-ins, placement zones
// ========================================================================================

/// Derive architectural walls from room boundaries. Every room edge that is shared with another
/// room or with the exterior becomes a wall segment. Walls are deduplicated: if two rooms share
/// an edge, one wall with both room ids on its sides is created.
fn generate_walls(t: &mut Truth) {
    let mut walls = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let thickness = 4.5; // 3-5/8" stud + 2 layers drywall ≈ 4.5"
    let stud_spacing = 16.0;

    for lv_code in ["1", "2"] {
        let rooms: Vec<Room> = t.building.rooms.iter().filter(|r| r.level == lv_code).cloned().collect();
        // Each room has 4 edges. For each edge, find the adjacent room or mark exterior.
        for rm in &rooms {
            let edges: [([f64; 2], [f64; 2]); 4] = [
                ([rm.min_ft[0], rm.min_ft[1]], [rm.max_ft[0], rm.min_ft[1]]), // south
                ([rm.max_ft[0], rm.min_ft[1]], [rm.max_ft[0], rm.max_ft[1]]), // east
                ([rm.min_ft[0], rm.max_ft[1]], [rm.max_ft[0], rm.max_ft[1]]), // north
                ([rm.min_ft[0], rm.min_ft[1]], [rm.min_ft[0], rm.max_ft[1]]), // west
            ];
            for (from, to) in &edges {
                // Canonical key: sorted endpoints to avoid duplicates.
                let key =
                    format!("{lv_code}:{:.4},{:.4}-{:.4},{:.4}", from[0].min(to[0]), from[1].min(to[1]), from[0].max(to[0]), from[1].max(to[1]),);
                if !seen.insert(key) {
                    // Already created this wall from the other room's perspective: update its
                    // second side.
                    if let Some(w) =
                        walls.iter_mut().filter(|w: &&mut Wall| w.level == lv_code && edge_matches(w.from_ft, w.to_ft, *from, *to)).last()
                        && w.sides[1] == "EXT"
                    {
                        w.sides[1] = rm.id.clone();
                    }
                    continue;
                }
                // Find adjacent room on the other side.
                let other = rooms
                    .iter()
                    .filter(|o| o.id != rm.id)
                    .find(|o| shares_edge(o, *from, *to))
                    .map(|o| o.id.clone())
                    .unwrap_or_else(|| "EXT".to_string());
                let id = format!("W-{lv_code}-{:03}", walls.len() + 1);
                walls.push(Wall {
                    id,
                    level: lv_code.into(),
                    from_ft: *from,
                    to_ft: *to,
                    thickness_in: thickness,
                    stud_spacing_in: stud_spacing,
                    sides: [rm.id.clone(), other],
                });
            }
        }
    }
    t.building.walls = walls;
}

/// Whether a room shares the given edge (or a segment of it).
fn shares_edge(rm: &Room, from: [f64; 2], to: [f64; 2]) -> bool {
    let eps = 1e-6;
    // Horizontal edge: same y, overlapping x range.
    if (from[1] - to[1]).abs() < eps {
        let y = from[1];
        let (x0, x1) = (from[0].min(to[0]), from[0].max(to[0]));
        if ((y - rm.min_ft[1]).abs() < eps || (y - rm.max_ft[1]).abs() < eps) && x0 < rm.max_ft[0] - eps && x1 > rm.min_ft[0] + eps {
            return true;
        }
    }
    // Vertical edge: same x, overlapping y range.
    if (from[0] - to[0]).abs() < eps {
        let x = from[0];
        let (y0, y1) = (from[1].min(to[1]), from[1].max(to[1]));
        if ((x - rm.min_ft[0]).abs() < eps || (x - rm.max_ft[0]).abs() < eps) && y0 < rm.max_ft[1] - eps && y1 > rm.min_ft[1] + eps {
            return true;
        }
    }
    false
}

/// Whether two wall definitions match the same edge (possibly reversed).
fn edge_matches(a0: [f64; 2], a1: [f64; 2], b0: [f64; 2], b1: [f64; 2]) -> bool {
    let eps = 1e-6;
    let eq = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps;
    (eq(a0, b0) && eq(a1, b1)) || (eq(a0, b1) && eq(a1, b0))
}

/// The length of a wall in inches.
fn wall_length_in(w: &Wall) -> f64 {
    let dx = w.to_ft[0] - w.from_ft[0];
    let dy = w.to_ft[1] - w.from_ft[1];
    (dx * dx + dy * dy).sqrt() * 12.0
}

/// Which stud cavity an offset (inches from wall start) falls into.
fn stud_cavity_at(offset_in: f64, spacing: f64) -> usize {
    if offset_in <= 0.0 {
        return 0;
    }
    (offset_in / spacing) as usize
}

/// Project a building-feet point onto a wall, returning inches from the wall start.
fn project_onto_wall(w: &Wall, pt: [f64; 2]) -> f64 {
    let dx = w.to_ft[0] - w.from_ft[0];
    let dy = w.to_ft[1] - w.from_ft[1];
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-12 {
        return 0.0;
    }
    let t = ((pt[0] - w.from_ft[0]) * dx + (pt[1] - w.from_ft[1]) * dy) / (len * len);
    t * len * 12.0 // feet to inches
}

/// Assign each plumbing fixture to the wall it is against (back wall).
fn assign_fixture_walls(t: &mut Truth) {
    let walls = t.building.walls.clone();
    for fx in &mut t.fixtures {
        let pt = [fx.at_ft[0], fx.at_ft[1]];
        // Find the closest wall on the same level within 2 ft.
        let best = walls
            .iter()
            .filter(|w| w.level == fx.level)
            .filter_map(|w| {
                let d = point_to_segment_dist(pt, w.from_ft, w.to_ft);
                if d < 2.0 { Some((d, w.id.clone())) } else { None }
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, wid)) = best {
            fx.wall = Some(wid);
        }
    }
}

/// Distance from a point to a line segment, in feet.
fn point_to_segment_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-18 {
        return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len_sq).clamp(0.0, 1.0);
    let proj = [a[0] + t * dx, a[1] + t * dy];
    ((p[0] - proj[0]).powi(2) + (p[1] - proj[1]).powi(2)).sqrt()
}

/// Generate in-wall rough-in stubs behind each plumbing fixture according to the given rules.
/// Vents and waste stacks are modelled as vertical runs through the cavity — vents from the
/// stub height up to the ceiling, waste stacks from the floor through the ceiling — so any
/// device in that cavity at any height will clash.
fn generate_rough_ins(t: &mut Truth, rules: &RoughInRules) {
    let fixtures = t.fixtures.clone();
    let walls = t.building.walls.clone();
    let rooms = t.building.rooms.clone();
    let mut rough_ins = Vec::new();
    let mut stack_n = 0usize;

    for fx in &fixtures {
        let Some(wall_id) = fx.wall.as_ref() else { continue };
        let Some(wall) = walls.iter().find(|w| &w.id == wall_id) else { continue };
        let Some(rule) = rules.for_kind(&fx.kind) else { continue };

        let elev = t.level(&fx.level).map_or(0.0, |l| l.elevation_ft);
        let along_in = project_onto_wall(wall, [fx.at_ft[0], fx.at_ft[1]]);

        // Ceiling height for this fixture's room (default 96" = 8').
        let ceiling_in = rooms
            .iter()
            .find(|r| r.level == fx.level && (r.number == fx.room || r.id == fx.room))
            .and_then(|r| r.ceiling.as_ref())
            .map_or(96.0, |c| c.height_ft * 12.0);

        // One stack per fixture (groups the vent + waste verticals).
        stack_n += 1;
        let stack_id = format!("STK{stack_n:03}");

        for stub in &rule.stubs {
            let offset_along = along_in + stub.offset_in;
            let cavity = stud_cavity_at(offset_along, wall.stud_spacing_in);
            let z = elev + stub.aff_in / 12.0;
            let dx = wall.to_ft[0] - wall.from_ft[0];
            let dy = wall.to_ft[1] - wall.from_ft[1];
            let len = (dx * dx + dy * dy).sqrt();
            let frac = if len > 1e-12 { offset_along / (len * 12.0) } else { 0.0 };
            let at = [wall.from_ft[0] + frac * dx, wall.from_ft[1] + frac * dy, z];

            let od = pipe_od(stub.size_in);
            // Vertical run extent depends on the stub kind:
            // - vent: runs from the stub height up to the ceiling (or above)
            // - waste at floor: runs from floor (0) to a few inches above the connection
            // - supply: just the stub point ± half the pipe OD
            let (run_bottom, run_top, stack) = match stub.kind.as_str() {
                "vent" => (stub.aff_in, ceiling_in, stack_id.clone()),
                "waste" if stub.aff_in < 1.0 => (0.0, ceiling_in, stack_id.clone()),
                _ => (stub.aff_in - od / 2.0, stub.aff_in + od / 2.0, String::new()),
            };

            let id = format!("RI{:03}", rough_ins.len() + 1);
            rough_ins.push(RoughIn {
                id,
                fixture: fx.id.clone(),
                wall: wall_id.clone(),
                system: stub.system.clone(),
                kind: stub.kind.clone(),
                at_ft: at.map(r6),
                size_in: stub.size_in,
                aff_in: stub.aff_in,
                offset_in: stub.offset_in,
                stud_cavity: cavity,
                assumed: true,
                run_bottom_in: run_bottom,
                run_top_in: run_top,
                stack,
                source: stub.id.clone(),
            });
        }
    }
    t.rough_ins = rough_ins;
}

/// Generate electrical devices (receptacles, switches) on the walls of each room.
/// NEC requires a duplex outlet within 6 ft of any point along a wall (≈ every 12 ft of wall)
/// and a switch near each door (we place one per room).
/// When `plant_inwall` is true, several devices are planted in conflict with rough-in stubs
/// (receptacle in waste cavity, switch in vent cavity).  Near-misses (close but not clashing)
/// are also planted to verify the detector does not over-flag.
fn generate_devices(
    t: &mut Truth,
    planted: &mut Vec<String>,
    plant_inwall: bool,
    plant_level: &str,
    rules: &RoughInRules,
    r: &mut Rng,
) {
    let walls = t.building.walls.clone();
    let clearance = rules.clearance_in;
    let mut devices = Vec::new();
    let mut dev_n = 0usize;
    let mut planted_receptacle = false;
    let mut planted_switch_vent = false;
    // Track near-miss device ids so tests can verify they are NOT flagged.
    let mut near_miss_ids: Vec<String> = Vec::new();

    for lv_code in ["1", "2"] {
        let rooms: Vec<Room> = t.building.rooms.iter().filter(|r| r.level == lv_code).cloned().collect();
        let esheet = format!("E-30{lv_code}");

        for rm in &rooms {
            if rm.name == "MECH" {
                continue;
            }
            let room_walls: Vec<&Wall> = walls.iter().filter(|w| w.level == lv_code && (w.sides[0] == rm.id || w.sides[1] == rm.id)).collect();

            // ---- Switch ----
            // Place on the first interior wall.  When planting, put it on a wall with a vent
            // cavity (the fixture's back wall) so a switch at +48" clashes with the vent riser.
            {
                // Default switch wall: first interior wall.
                let default_sw = room_walls.iter().find(|w| w.sides[0] != "EXT" && w.sides[1] != "EXT");
                // For planting: find any room wall that has a vent rough-in.
                let want_plant_sw = plant_inwall
                    && !planted_switch_vent
                    && lv_code == plant_level
                    && is_toilet_room(&rm.name);
                let vent_wall = if want_plant_sw {
                    room_walls.iter().find(|w| vent_cavity_on_wall(t, &w.id).is_some())
                } else {
                    None
                };
                let sw_wall = vent_wall.or(default_sw);
                if let Some(sw_wall) = sw_wall {
                    let sw_len = wall_length_in(sw_wall);
                    let mut along = 6.0;
                    let mut cavity = stud_cavity_at(along, sw_wall.stud_spacing_in);
                    let actually_plant = want_plant_sw && vent_wall.is_some();
                    if actually_plant {
                        if let Some(vc) = vent_cavity_on_wall(t, &sw_wall.id) {
                            cavity = vc;
                            along = (vc as f64 + 0.5) * sw_wall.stud_spacing_in;
                        }
                    }
                    let frac = along / sw_len.max(1.0);
                    let dx = sw_wall.to_ft[0] - sw_wall.from_ft[0];
                    let dy = sw_wall.to_ft[1] - sw_wall.from_ft[1];
                    let at = [sw_wall.from_ft[0] + frac * dx, sw_wall.from_ft[1] + frac * dy];
                    let diag = diag_offset(at, sw_wall, 0.4);
                    dev_n += 1;
                    let id = format!("D{dev_n:03}");
                    if actually_plant {
                        planted.push(id.clone());
                        planted_switch_vent = true;
                    }
                    devices.push(Device {
                        id,
                        tag: format!("S-{lv_code}-{dev_n:02}"),
                        kind: "switch".into(),
                        level: lv_code.into(),
                        room: rm.number.clone(),
                        wall: sw_wall.id.clone(),
                        at_ft: at.map(r6),
                        diag_ft: diag.map(r6),
                        aff_in: 48.0,
                        box_in: [2.75, 4.5, 3.5],
                        stud_cavity: cavity,
                        fixed: true,
                        sheet: esheet.clone(),
                    });
                }
            }

            // ---- Receptacles ----
            for wall in &room_walls {
                let len = wall_length_in(wall);
                if len < 24.0 {
                    continue;
                }
                let count = ((len / (12.0 * 12.0)).ceil() as usize).max(1);
                let spacing = len / (count as f64 + 1.0);
                for k in 0..count {
                    let along = spacing * (k as f64 + 1.0);
                    let cavity = stud_cavity_at(along, wall.stud_spacing_in);

                    // Plant: receptacle in a waste cavity.
                    let want_plant = plant_inwall
                        && !planted_receptacle
                        && lv_code == plant_level
                        && is_toilet_room(&rm.name)
                        && has_roughin_in_cavity(t, &wall.id, cavity);

                    let in_roughin_zone = is_near_roughin_inches(t, &wall.id, along, clearance, wall.stud_spacing_in, len);
                    if in_roughin_zone && !want_plant {
                        // Try to place a near-miss: in adjacent cavity but > clearance away.
                        let near_miss = find_near_miss_cavity(t, &wall.id, cavity, wall.stud_spacing_in, len, clearance);
                        if let Some((nm_cav, nm_along)) = near_miss {
                            if !near_miss_ids.is_empty() || !r.chance(0.3) {
                                // Already planted a near-miss or random skip: shift to a clear spot.
                                let shifted = find_clear_cavity(t, &wall.id, cavity, wall.stud_spacing_in, len);
                                if shifted == cavity { continue; }
                                let new_along = (shifted as f64 + 0.5) * wall.stud_spacing_in;
                                if new_along > len || new_along < 0.0 { continue; }
                                let frac = new_along / len;
                                let dx = wall.to_ft[0] - wall.from_ft[0];
                                let dy = wall.to_ft[1] - wall.from_ft[1];
                                let at = [wall.from_ft[0] + frac * dx, wall.from_ft[1] + frac * dy];
                                let diag = diag_offset(at, wall, 0.3);
                                dev_n += 1;
                                devices.push(Device {
                                    id: format!("D{dev_n:03}"),
                                    tag: format!("R-{lv_code}-{dev_n:02}"),
                                    kind: "receptacle".into(),
                                    level: lv_code.into(),
                                    room: rm.number.clone(),
                                    wall: wall.id.clone(),
                                    at_ft: at.map(r6),
                                    diag_ft: diag.map(r6),
                                    aff_in: 18.0,
                                    box_in: [2.75, 4.5, 3.5],
                                    stud_cavity: shifted,
                                    fixed: r.chance(0.7),
                                    sheet: esheet.clone(),
                                });
                                continue;
                            }
                            // Plant a near-miss.
                            let frac = nm_along / len;
                            let dx = wall.to_ft[0] - wall.from_ft[0];
                            let dy = wall.to_ft[1] - wall.from_ft[1];
                            let at = [wall.from_ft[0] + frac * dx, wall.from_ft[1] + frac * dy];
                            let diag = diag_offset(at, wall, 0.3);
                            dev_n += 1;
                            let id = format!("D{dev_n:03}");
                            near_miss_ids.push(id.clone());
                            devices.push(Device {
                                id,
                                tag: format!("R-{lv_code}-{dev_n:02}"),
                                kind: "receptacle".into(),
                                level: lv_code.into(),
                                room: rm.number.clone(),
                                wall: wall.id.clone(),
                                at_ft: at.map(r6),
                                diag_ft: diag.map(r6),
                                aff_in: 18.0,
                                box_in: [2.75, 4.5, 3.5],
                                stud_cavity: nm_cav,
                                fixed: true,
                                sheet: esheet.clone(),
                            });
                            continue;
                        }
                        // No near-miss opportunity: shift to clear.
                        let shifted = find_clear_cavity(t, &wall.id, cavity, wall.stud_spacing_in, len);
                        if shifted == cavity { continue; }
                        let new_along = (shifted as f64 + 0.5) * wall.stud_spacing_in;
                        if new_along > len || new_along < 0.0 { continue; }
                        let frac = new_along / len;
                        let dx = wall.to_ft[0] - wall.from_ft[0];
                        let dy = wall.to_ft[1] - wall.from_ft[1];
                        let at = [wall.from_ft[0] + frac * dx, wall.from_ft[1] + frac * dy];
                        let diag = diag_offset(at, wall, 0.3);
                        dev_n += 1;
                        devices.push(Device {
                            id: format!("D{dev_n:03}"),
                            tag: format!("R-{lv_code}-{dev_n:02}"),
                            kind: "receptacle".into(),
                            level: lv_code.into(),
                            room: rm.number.clone(),
                            wall: wall.id.clone(),
                            at_ft: at.map(r6),
                            diag_ft: diag.map(r6),
                            aff_in: 18.0,
                            box_in: [2.75, 4.5, 3.5],
                            stud_cavity: shifted,
                            fixed: r.chance(0.7),
                            sheet: esheet.clone(),
                        });
                        continue;
                    }

                    let frac = along / len;
                    let dx = wall.to_ft[0] - wall.from_ft[0];
                    let dy = wall.to_ft[1] - wall.from_ft[1];
                    let at = [wall.from_ft[0] + frac * dx, wall.from_ft[1] + frac * dy];
                    let diag = diag_offset(at, wall, 0.3);

                    dev_n += 1;
                    let id = format!("D{dev_n:03}");
                    if want_plant {
                        planted.push(id.clone());
                        planted_receptacle = true;
                    }
                    let fixed = !want_plant && r.chance(0.7);

                    devices.push(Device {
                        id,
                        tag: format!("R-{lv_code}-{dev_n:02}"),
                        kind: "receptacle".into(),
                        level: lv_code.into(),
                        room: rm.number.clone(),
                        wall: wall.id.clone(),
                        at_ft: at.map(r6),
                        diag_ft: diag.map(r6),
                        aff_in: 18.0,
                        box_in: [2.75, 4.5, 3.5],
                        stud_cavity: cavity,
                        fixed,
                        sheet: esheet.clone(),
                    });
                }
            }
        }
    }
    t.devices = devices;
    t.near_miss_ids = near_miss_ids;
}

fn is_toilet_room(name: &str) -> bool {
    matches!(name, "MEN" | "WOMEN" | "TOILET" | "RESTROOM")
}

/// Check whether the given wall+cavity already has a rough-in stub.
fn has_roughin_in_cavity(t: &Truth, wall_id: &str, cavity: usize) -> bool {
    t.rough_ins.iter().any(|ri| ri.wall == wall_id && ri.stud_cavity == cavity)
}

/// Check whether a cavity (or within `margin` cavities) has a rough-in stub.
fn is_near_roughin(t: &Truth, wall_id: &str, cavity: usize, margin: usize) -> bool {
    t.rough_ins.iter().any(|ri| ri.wall == wall_id && { ri.stud_cavity.abs_diff(cavity) <= margin })
}

/// Check whether a position (inches along the wall) is within `clearance_in` of any rough-in
/// pipe edge on this wall.  Same cavity = always too close (the device would share a stud bay).
fn is_near_roughin_inches(t: &Truth, wall_id: &str, along_in: f64, clearance_in: f64, stud_spacing: f64, _wall_len: f64) -> bool {
    let box_half = 2.75 / 2.0;
    let dev_cav = stud_cavity_at(along_in, stud_spacing);
    for ri in t.rough_ins.iter().filter(|ri| ri.wall == wall_id) {
        // Same cavity = always a clash, regardless of inch distance.
        if ri.stud_cavity == dev_cav {
            return true;
        }
        let ri_along = (ri.stud_cavity as f64 + 0.5) * stud_spacing;
        let od = pipe_od(ri.size_in);
        let gap = (along_in - ri_along).abs() - box_half - od / 2.0;
        if gap < clearance_in {
            return true;
        }
    }
    false
}

/// Find the nearest clear cavity (no rough-in within 1 cavity) starting from `start`.
fn find_clear_cavity(t: &Truth, wall_id: &str, start: usize, spacing: f64, wall_len: f64) -> usize {
    let max_cavity = (wall_len / spacing) as usize;
    for offset in 1..=max_cavity {
        if start + offset <= max_cavity && !is_near_roughin(t, wall_id, start + offset, 1) {
            return start + offset;
        }
        if start >= offset && !is_near_roughin(t, wall_id, start - offset, 1) {
            return start - offset;
        }
    }
    start // fallback
}

/// Find a cavity on `wall_id` that contains a vent rough-in (for planting a switch clash).
fn vent_cavity_on_wall(t: &Truth, wall_id: &str) -> Option<usize> {
    t.rough_ins
        .iter()
        .find(|ri| ri.wall == wall_id && ri.kind == "vent")
        .map(|ri| ri.stud_cavity)
}

/// Find a near-miss position: a cavity adjacent to a rough-in stub but with a gap > clearance.
/// Returns `(cavity, along_in)` or `None`.
fn find_near_miss_cavity(
    t: &Truth,
    wall_id: &str,
    _start: usize,
    spacing: f64,
    wall_len: f64,
    clearance_in: f64,
) -> Option<(usize, f64)> {
    let max_cavity = (wall_len / spacing) as usize;
    let box_half = 2.75 / 2.0;
    // Look for a cavity that is adjacent to a rough-in but where the gap in inches exceeds
    // the clearance.  Specifically: 2 cavities away from any stub → gap ≈ 2×16 − box/2 − od/2
    // which is well over 3".
    for ri in t.rough_ins.iter().filter(|ri| ri.wall == wall_id) {
        let od = pipe_od(ri.size_in);
        // Try 2 cavities away (gap ≈ 32" - margins, well above 3")
        for delta in [2isize, -2] {
            let cav = ri.stud_cavity as isize + delta;
            if cav < 0 || cav as usize > max_cavity { continue; }
            let cav = cav as usize;
            if has_roughin_in_cavity(t, wall_id, cav) { continue; }
            let along = (cav as f64 + 0.5) * spacing;
            if along < 0.0 || along > wall_len { continue; }
            let ri_along = (ri.stud_cavity as f64 + 0.5) * spacing;
            let gap = (along - ri_along).abs() - box_half - od / 2.0;
            if gap > clearance_in + 1.0 {
                return Some((cav, along));
            }
        }
    }
    None
}

/// Compute a diagrammatic offset position for an E-sheet symbol.  The symbol is shifted
/// perpendicular to the wall by `offset_ft` (building feet) so it does not sit on the true
/// architectural position.
fn diag_offset(at: [f64; 2], wall: &Wall, offset_ft: f64) -> [f64; 2] {
    let dx = wall.to_ft[0] - wall.from_ft[0];
    let dy = wall.to_ft[1] - wall.from_ft[1];
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-12 {
        return at;
    }
    // Normal to the wall (perpendicular, into the room).
    let nx = -dy / len;
    let ny = dx / len;
    [at[0] + nx * offset_ft, at[1] + ny * offset_ft]
}

// ========================================================================================

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

/// Cable tray and a feeder conduit bank down each floor's corridor, and a conduit riser bank
/// from the level 1 panels up to level 2's (coordination only: no circuiting).
///
/// They hang in the plumbing band, laterally between the corridor sprinkler drops (south) and
/// the domestic water mains (north): the bank first, 3" clear, then the tray. Corridor widths
/// set the sizes so both fit: 6' corridors a 6" tray and (2) 2" conduits, 7' a 12" tray and
/// (2) 2", 8' a 12" tray and (2) 3" or (3) 2". The tray is 4" deep; both sit on the band's
/// bottom (above the corridor ceiling). They run from just past the mech room (its return
/// grilles and duct risers) to the toilet bay (the plumbing turns there).
#[allow(clippy::too_many_arguments)]
fn tray_design(net: &mut Net, t: &mut Truth, floors: &[Floor], xs: &[f64], y1: f64, cw: f64, w0: f64, x_last0: f64, cw_main: f64, seed: u64) {
    // A stream of its own: the rest of each set doesn't move.
    let mut r = Rng::new(seed ^ 0x5452_4159_434F);
    let tray_w = if cw < 6.5 { 6.0 } else { 12.0 };
    let (n_bank, bank_d) = if cw < 7.5 { (2, 2.0) } else { r.pick(&[(2usize, 3.0f64), (3, 2.0)]) };
    let gap = 1.0 / 12.0; // between conduits
    let od = pipe_od(bank_d) / 12.0;
    let lo = y1 + 0.2 * cw + 0.3;
    let bank_ys: Vec<f64> = (0..n_bank).map(|k| lo + od / 2.0 + k as f64 * (od + gap)).collect();
    let tray_y = lo + n_bank as f64 * od + (n_bank as f64 - 1.0) * gap + 0.25 + tray_w / 24.0;
    let (xa, xb) = (w0 + 2.0, x_last0 - 1.0);
    for c in [("CT", "CABLE TRAY", "tray"), ("EC", "FEEDER CONDUIT", "conduit")] {
        if !t.systems.iter().any(|s| s.code == c.0) {
            t.systems.push(System {
                code: c.0.into(),
                name: c.1.into(),
                kind: c.2.into(),
                printed: if c.0 == "CT" { "CT".into() } else { "C".into() },
            });
        }
    }
    let _ = xs;
    let mut l1_bank: Vec<usize> = Vec::new();
    for f in floors {
        let lv = f.code.as_str();
        let e = f.elev;
        let sheet = format!("E-20{lv}");
        // The band's bottom: the plumbing band's.
        let bottom = e + f.pl_c - pipe_od(cw_main.max(2.0)) / 24.0;
        let tz = bottom + 2.0 / 12.0;
        let a = net.node("cap", "CT", lv, [xa, tray_y, tz]);
        let b = net.node("cap", "CT", lv, [xb, tray_y, tz]);
        net.link(a, b, "CT", "tray", [tray_w, 4.0], lv, &sheet);
        let cz = bottom + od / 2.0;
        for y in &bank_ys {
            let a = net.node("cap", "EC", lv, [xa, *y, cz]);
            let b = net.node("cap", "EC", lv, [xb, *y, cz]);
            net.link(a, b, "EC", "conduit", [bank_d, bank_d], lv, &sheet);
        }
        // The riser bank between the panels in the mech room: (4) 2" from level 1 to level 2.
        let x0 = xs.first().copied().unwrap_or(0.0);
        let rz = e + 7.0;
        let ends: Vec<usize> = (0..4).map(|k| net.node("cap", "EC", lv, [x0 + 3.0 + 0.3 * k as f64, 2.0, rz])).collect();
        if lv == "1" {
            l1_bank = ends;
        } else {
            for (lo_n, hi_n) in l1_bank.iter().zip(&ends) {
                net.link(*lo_n, *hi_n, "EC", "conduit", [2.0, 2.0], "1", "E-201");
            }
        }
    }
}

/// The electrical systems: lighting fixtures, switches, receptacles, data outlets, panels,
/// circuits, conduit runs and homerun arrows. Positions are diagrammatic on the E-sheets
/// (symbols near, not at, the true position) while A-sheets carry the real positions.
#[allow(clippy::too_many_arguments)]
fn elec_design(t: &mut Truth, floors: &[Floor], xs: &[f64], y1: f64, y2: f64, _y3: f64, _cw: f64, r: &mut Rng) {
    let mut er = r.fork(99);
    let width = xs.last().copied().unwrap_or(0.0);

    // Fixture type definitions (tag, description, mounting, lamp/wattage as text).
    let light_types = [
        ("A", "2X4 RECESSED TROFFER", "RECESSED", "LED 40W"),
        ("B", "2X2 RECESSED TROFFER", "RECESSED", "LED 24W"),
        ("C", "RECESSED DOWNLIGHT 6\"", "RECESSED", "LED 12W"),
        ("D", "WALL SCONCE", "WALL MOUNT", "LED 9W"),
        ("E", "PENDANT LINEAR 4'", "PENDANT", "LED 36W"),
    ];
    let exit_types = [("ES-1", "EXIT SIGN, LED", "WALL/CEILING", "LED 2W")];
    let emerg_types = [("EM-1", "EMERGENCY LIGHT, DUAL HEAD", "WALL MOUNT", "LED 2X3W")];

    // Panel sizes per floor: one lighting panel, one power panel.
    let panel_tags_l = |lv: &str| format!("L{lv}");
    let panel_tags_p = |lv: &str| format!("P{lv}");

    // Diagrammatic offset: the E-sheets show symbols randomly offset from the true position.
    let jitter = |r: &mut Rng, x: f64, y: f64| -> [f64; 2] {
        let dx = (r.range(0, 30) as f64 - 15.0) / 12.0; // up to 1.25 ft
        let dy = (r.range(0, 30) as f64 - 15.0) / 12.0;
        [x + dx, y + dy]
    };

    let mut dev_n = 0usize;
    let mut _ckt_n = 0usize;
    let mut conduit_n = 0usize;

    for f in floors {
        let lv = f.code.as_str();
        let e = f.elev;
        let lsheet = format!("E-10{lv}");
        let psheet = format!("E-20{lv}");

        let lp_tag = panel_tags_l(lv);
        let pp_tag = panel_tags_p(lv);
        // Panels in the electrical room (first bay, south side)
        let panel_x = xs.first().copied().unwrap_or(0.0) + 2.0;
        let panel_y = 2.0;
        let lp_id = format!("EP{:02}", t.elec_panels.len() + 1);
        t.elec_panels.push(ElecPanel {
            id: lp_id.clone(),
            tag: lp_tag.clone(),
            kind: "panel".into(),
            level: lv.into(),
            room: format!("ELEC-{lv}"),
            at_ft: [panel_x, panel_y, e],
            min_ft: [panel_x - 0.75, panel_y - 0.25],
            max_ft: [panel_x + 0.75, panel_y + 0.25],
            schedule: Vec::new(),
            sheet: lsheet.clone(),
            ..ElecPanel::default()
        });
        let pp_id = format!("EP{:02}", t.elec_panels.len() + 1);
        t.elec_panels.push(ElecPanel {
            id: pp_id.clone(),
            tag: pp_tag.clone(),
            kind: "panel".into(),
            level: lv.into(),
            room: format!("ELEC-{lv}"),
            at_ft: [panel_x + 3.0, panel_y, e],
            min_ft: [panel_x + 2.25, panel_y - 0.25],
            max_ft: [panel_x + 3.75, panel_y + 0.25],
            schedule: Vec::new(),
            sheet: psheet.clone(),
            ..ElecPanel::default()
        });

        // A transformer on level 1 only.
        if lv == "1" {
            let tx_id = format!("EP{:02}", t.elec_panels.len() + 1);
            t.elec_panels.push(ElecPanel {
                id: tx_id,
                tag: "TX-1".into(),
                kind: "transformer".into(),
                level: "1".into(),
                room: format!("ELEC-{lv}"),
                at_ft: [panel_x + 6.0, panel_y, e],
                min_ft: [panel_x + 5.25, panel_y - 0.5],
                max_ft: [panel_x + 6.75, panel_y + 0.5],
                schedule: Vec::new(),
                sheet: psheet.clone(),
                ..ElecPanel::default()
            });
        }

        let mut light_ckt = 1u32;
        let mut power_ckt = 1u32;
        let mut light_schedule_rows: Vec<PanelCircuitRow> = Vec::new();
        let mut power_schedule_rows: Vec<PanelCircuitRow> = Vec::new();

        // ---- Lighting: office rooms get 2x4 troffers, corridor gets 2x2, toilets get downlights.
        // Switches at the door of each room.
        let rooms: Vec<Room> = t.building.rooms.iter().filter(|rm| rm.level == lv).cloned().collect();
        for rm in &rooms {
            let (rx0, ry0) = (rm.min_ft[0], rm.min_ft[1]);
            let (rx1, ry1) = (rm.max_ft[0], rm.max_ft[1]);
            let rw = rx1 - rx0;
            let rh = ry1 - ry0;
            let clg = rm.ceiling.as_ref().map(|c| c.height_ft).unwrap_or(9.0);
            let clg_in = clg * 12.0;

            // Pick fixture type based on room.
            let is_corridor = rm.name.contains("CORRIDOR") || rm.name.contains("HALL");
            let is_toilet = rm.name.contains("TOILET") || rm.name.contains("REST");
            let is_mech = rm.name.contains("MECH") || rm.name.contains("ELEC");
            let (fix_tag, _fix_desc, _fix_mount, _fix_watt) = if is_corridor {
                light_types[1] // 2x2
            } else if is_toilet {
                light_types[2] // downlight
            } else if is_mech {
                light_types[1] // 2x2
            } else {
                light_types[0] // 2x4
            };

            // How many fixtures: roughly one per 80 sq ft for offices, 60 for corridors.
            let area = rw * rh;
            let per_fix = if is_corridor { 60.0 } else { 80.0 };
            let count = ((area / per_fix).round() as usize).clamp(1, 12);
            let cols = if rw > rh { ((rw / 8.0).ceil() as usize).clamp(1, 4) } else { 1 };
            let rows_n = ((count as f64 / cols as f64).ceil() as usize).max(1);

            let ckt_label = format!("{lp_tag}/{light_ckt}");
            let ckt_id = format!("CK{:03}", t.elec_circuits.len() + 1);
            _ckt_n += 1;
            let mut ckt_devices: Vec<String> = Vec::new();

            for row in 0..rows_n {
                for col in 0..cols {
                    if row * cols + col >= count {
                        break;
                    }
                    dev_n += 1;
                    let fx = rx0 + (col as f64 + 0.5) * rw / cols as f64;
                    let fy = ry0 + (row as f64 + 0.5) * rh / rows_n as f64;
                    let drawn = jitter(&mut er, fx, fy);
                    let arch_pos = !is_corridor && !is_mech; // offices get exact architectural positioning
                    let dev_id = format!("ED{dev_n:04}");
                    t.elec_devices.push(ElecDevice {
                        id: dev_id.clone(),
                        tag: fix_tag.into(),
                        kind: "light_fixture".into(),
                        type_tag: fix_tag.into(),
                        level: lv.into(),
                        room: rm.number.clone(),
                        at_ft: [fx, fy, e + clg],
                        drawn_ft: drawn,
                        mounting_in: clg_in,
                        mounting_note: Some("CLG".into()),
                        arch_positioned: arch_pos,
                        circuit: ckt_id.clone(),
                        panel: lp_id.clone(),
                        sheet: lsheet.clone(),
                    });
                    ckt_devices.push(dev_id);
                }
            }

            // Switch at the door (south-west corner of the room for offices, corridor side).
            dev_n += 1;
            let sw_x = rx0 + 0.5;
            let sw_y = if ry0 <= y1 { ry1 - 0.5 } else { ry0 + 0.5 };
            let sw_drawn = jitter(&mut er, sw_x, sw_y);
            let sw_id = format!("ED{dev_n:04}");
            t.elec_devices.push(ElecDevice {
                id: sw_id.clone(),
                tag: "S".into(),
                kind: "switch".into(),
                type_tag: "S".into(),
                level: lv.into(),
                room: rm.number.clone(),
                at_ft: [sw_x, sw_y, e + 46.0 / 12.0],
                drawn_ft: sw_drawn,
                mounting_in: 46.0,
                mounting_note: Some("+46\"".into()),
                arch_positioned: false,
                circuit: ckt_id.clone(),
                panel: lp_id.clone(),
                sheet: lsheet.clone(),
            });
            ckt_devices.push(sw_id);

            // Homerun arrow from the last fixture in the room.
            let hr_x = rx0 + rw * 0.5;
            let hr_y = ry0 + rh * 0.8;
            t.elec_circuits.push(ElecCircuit {
                id: ckt_id.clone(),
                label: ckt_label.clone(),
                panel: lp_id.clone(),
                number: format!("{light_ckt}"),
                level: lv.into(),
                devices: ckt_devices,
                homerun_ft: [hr_x, hr_y],
                sheet: lsheet.clone(),
            });
            light_schedule_rows.push(PanelCircuitRow {
                circuit: format!("{light_ckt}"),
                description: format!("LIGHTING - {}", rm.name),
                breaker_amps: "20".into(),
                poles: "1".into(),
            });
            light_ckt += 1;
        }

        // Exit signs at corridor ends and emergency lights every ~75 ft of corridor.
        let corr_rooms: Vec<&Room> = rooms.iter().filter(|rm| rm.name.contains("CORRIDOR") || rm.name.contains("HALL")).collect();
        for crm in &corr_rooms {
            let (cx0, cy0) = (crm.min_ft[0], crm.min_ft[1]);
            let (cx1, cy1) = (crm.max_ft[0], crm.max_ft[1]);
            let corr_len = cx1 - cx0;
            let clg = crm.ceiling.as_ref().map(|c| c.height_ft).unwrap_or(9.0);

            // Exit signs at each end.
            for ex in [cx0 + 1.0, cx1 - 1.0] {
                dev_n += 1;
                let ey = (cy0 + cy1) / 2.0;
                let dev_id = format!("ED{dev_n:04}");
                t.elec_devices.push(ElecDevice {
                    id: dev_id.clone(),
                    tag: "ES-1".into(),
                    kind: "exit_sign".into(),
                    type_tag: exit_types[0].0.into(),
                    level: lv.into(),
                    room: crm.number.clone(),
                    at_ft: [ex, ey, e + clg],
                    drawn_ft: jitter(&mut er, ex, ey),
                    mounting_in: clg * 12.0,
                    mounting_note: Some("CLG".into()),
                    arch_positioned: false,
                    circuit: String::new(), // emergency circuit, filled below
                    panel: lp_id.clone(),
                    sheet: lsheet.clone(),
                });
            }

            // Emergency lights every 75 ft.
            let n_em = ((corr_len / 75.0).ceil() as usize).max(1);
            for k in 0..n_em {
                dev_n += 1;
                let emx = cx0 + (k as f64 + 0.5) * corr_len / n_em as f64;
                let emy = (cy0 + cy1) / 2.0;
                let dev_id = format!("ED{dev_n:04}");
                t.elec_devices.push(ElecDevice {
                    id: dev_id.clone(),
                    tag: "EM-1".into(),
                    kind: "emergency_light".into(),
                    type_tag: emerg_types[0].0.into(),
                    level: lv.into(),
                    room: crm.number.clone(),
                    at_ft: [emx, emy, e + 7.0],
                    drawn_ft: jitter(&mut er, emx, emy),
                    mounting_in: 84.0, // 7' AFF
                    mounting_note: Some("+84\"".into()),
                    arch_positioned: false,
                    circuit: String::new(),
                    panel: lp_id.clone(),
                    sheet: lsheet.clone(),
                });
            }
        }

        // Emergency circuit: gather all exit signs and emergency lights.
        let em_devs: Vec<String> = t
            .elec_devices
            .iter()
            .filter(|d| d.level == lv && (d.kind == "exit_sign" || d.kind == "emergency_light") && d.circuit.is_empty())
            .map(|d| d.id.clone())
            .collect();
        if !em_devs.is_empty() {
            let em_ckt_id = format!("CK{:03}", t.elec_circuits.len() + 1);
            let em_label = format!("{lp_tag}/{light_ckt}");
            for d in t.elec_devices.iter_mut().filter(|d| em_devs.contains(&d.id)) {
                d.circuit = em_ckt_id.clone();
            }
            let hr = t.elec_devices.iter().rfind(|d| em_devs.contains(&d.id)).map(|d| d.drawn_ft).unwrap_or([panel_x, panel_y]);
            t.elec_circuits.push(ElecCircuit {
                id: em_ckt_id,
                label: em_label,
                panel: lp_id.clone(),
                number: format!("{light_ckt}"),
                level: lv.into(),
                devices: em_devs,
                homerun_ft: hr,
                sheet: lsheet.clone(),
            });
            light_schedule_rows.push(PanelCircuitRow {
                circuit: format!("{light_ckt}"),
                description: "EMERGENCY/EXIT LIGHTING".into(),
                breaker_amps: "20".into(),
                poles: "1".into(),
            });
            light_ckt += 1;
        }

        // ---- Power plan: receptacles, data outlets, floor boxes -----------------------
        for rm in &rooms {
            let (rx0, ry0) = (rm.min_ft[0], rm.min_ft[1]);
            let (rx1, ry1) = (rm.max_ft[0], rm.max_ft[1]);
            let rw = rx1 - rx0;
            let rh = ry1 - ry0;
            let is_corridor = rm.name.contains("CORRIDOR") || rm.name.contains("HALL");
            let is_toilet = rm.name.contains("TOILET") || rm.name.contains("REST");
            let is_mech = rm.name.contains("MECH") || rm.name.contains("ELEC");

            let ckt_label = format!("{pp_tag}/{power_ckt}");
            let ckt_id = format!("CK{:03}", t.elec_circuits.len() + 1);
            let mut ckt_devices: Vec<String> = Vec::new();

            // Receptacles along walls: one every 12 ft (code), at 18" AFF.
            let perimeter = 2.0 * (rw + rh);
            let n_recep = if is_corridor { 2 } else { ((perimeter / 12.0).ceil() as usize).clamp(2, 8) };
            // Place receptacles around the perimeter.
            for k in 0..n_recep {
                dev_n += 1;
                let frac = k as f64 / n_recep as f64;
                let perim_at = frac * perimeter;
                let (rx, ry) = if perim_at < rw {
                    (rx0 + perim_at, ry0)
                } else if perim_at < rw + rh {
                    (rx1, ry0 + (perim_at - rw))
                } else if perim_at < 2.0 * rw + rh {
                    (rx1 - (perim_at - rw - rh), ry1)
                } else {
                    (rx0, ry1 - (perim_at - 2.0 * rw - rh))
                };
                let drawn = jitter(&mut er, rx, ry);
                let dev_id = format!("ED{dev_n:04}");
                t.elec_devices.push(ElecDevice {
                    id: dev_id.clone(),
                    tag: "R".into(),
                    kind: "receptacle".into(),
                    type_tag: "R".into(),
                    level: lv.into(),
                    room: rm.number.clone(),
                    at_ft: [rx, ry, e + 18.0 / 12.0],
                    drawn_ft: drawn,
                    mounting_in: 18.0,
                    mounting_note: Some("+18\" AFF".into()),
                    arch_positioned: false,
                    circuit: ckt_id.clone(),
                    panel: pp_id.clone(),
                    sheet: psheet.clone(),
                });
                ckt_devices.push(dev_id);
            }

            // Data/telecom outlets: 1-2 per office room.
            if !is_corridor && !is_toilet && !is_mech {
                let n_data = er.range(1, 2);
                for k in 0..n_data {
                    dev_n += 1;
                    let dx = rx0 + (k as f64 + 0.5) * rw / n_data as f64;
                    let dy = ry0 + 0.3;
                    let drawn = jitter(&mut er, dx, dy);
                    let dev_id = format!("ED{dev_n:04}");
                    t.elec_devices.push(ElecDevice {
                        id: dev_id.clone(),
                        tag: "D".into(),
                        kind: "data_outlet".into(),
                        type_tag: "D".into(),
                        level: lv.into(),
                        room: rm.number.clone(),
                        at_ft: [dx, dy, e + 18.0 / 12.0],
                        drawn_ft: drawn,
                        mounting_in: 18.0,
                        mounting_note: Some("+18\" AFF".into()),
                        arch_positioned: false,
                        circuit: ckt_id.clone(),
                        panel: pp_id.clone(),
                        sheet: psheet.clone(),
                    });
                    ckt_devices.push(dev_id);
                }
            }

            // Floor box in large rooms (> 400 sq ft).
            if !is_corridor && !is_toilet && rw * rh > 400.0 {
                dev_n += 1;
                let fbx = (rx0 + rx1) / 2.0;
                let fby = (ry0 + ry1) / 2.0;
                let drawn = jitter(&mut er, fbx, fby);
                let dev_id = format!("ED{dev_n:04}");
                t.elec_devices.push(ElecDevice {
                    id: dev_id.clone(),
                    tag: "FB".into(),
                    kind: "floor_box".into(),
                    type_tag: "FB".into(),
                    level: lv.into(),
                    room: rm.number.clone(),
                    at_ft: [fbx, fby, e],
                    drawn_ft: drawn,
                    mounting_in: 0.0,
                    mounting_note: Some("FLOOR".into()),
                    arch_positioned: true,
                    circuit: ckt_id.clone(),
                    panel: pp_id.clone(),
                    sheet: psheet.clone(),
                });
                ckt_devices.push(dev_id);
            }

            // Homerun arrow.
            let hr_x = (rx0 + rx1) / 2.0;
            let hr_y = ry0 + rh * 0.3;
            t.elec_circuits.push(ElecCircuit {
                id: ckt_id.clone(),
                label: ckt_label,
                panel: pp_id.clone(),
                number: format!("{power_ckt}"),
                level: lv.into(),
                devices: ckt_devices,
                homerun_ft: [hr_x, hr_y],
                sheet: psheet.clone(),
            });
            power_schedule_rows.push(PanelCircuitRow {
                circuit: format!("{power_ckt}"),
                description: format!("RECEP/DATA - {}", rm.name),
                breaker_amps: "20".into(),
                poles: "1".into(),
            });
            power_ckt += 1;
        }

        // Disconnects near mechanical equipment (one per floor, near the AHU).
        {
            dev_n += 1;
            let disc_x = xs.first().copied().unwrap_or(0.0) + 2.0;
            let disc_y = 4.0;
            let disc_id = format!("ED{dev_n:04}");
            let ckt_id = format!("CK{:03}", t.elec_circuits.len() + 1);
            t.elec_devices.push(ElecDevice {
                id: disc_id.clone(),
                tag: format!("DS-{lv}"),
                kind: "disconnect".into(),
                type_tag: "DS".into(),
                level: lv.into(),
                room: format!("MECH-{lv}"),
                at_ft: [disc_x, disc_y, e + 60.0 / 12.0],
                drawn_ft: jitter(&mut er, disc_x, disc_y),
                mounting_in: 60.0,
                mounting_note: Some("+60\"".into()),
                arch_positioned: true,
                circuit: ckt_id.clone(),
                panel: pp_id.clone(),
                sheet: psheet.clone(),
            });
            t.elec_circuits.push(ElecCircuit {
                id: ckt_id,
                label: format!("{pp_tag}/{power_ckt}"),
                panel: pp_id.clone(),
                number: format!("{power_ckt}"),
                level: lv.into(),
                devices: vec![disc_id],
                homerun_ft: [disc_x, disc_y],
                sheet: psheet.clone(),
            });
            power_schedule_rows.push(PanelCircuitRow {
                circuit: format!("{power_ckt}"),
                description: format!("AHU-{lv} DISCONNECT"),
                breaker_amps: "30".into(),
                poles: "2".into(),
            });
            power_ckt += 1;
        }

        // Junction boxes near the panels (one per floor).
        {
            dev_n += 1;
            let jb_x = panel_x + 1.5;
            let jb_y = panel_y + 2.0;
            let jb_id = format!("ED{dev_n:04}");
            t.elec_devices.push(ElecDevice {
                id: jb_id,
                tag: format!("JB-{lv}"),
                kind: "junction_box".into(),
                type_tag: "JB".into(),
                level: lv.into(),
                room: format!("ELEC-{lv}"),
                at_ft: [jb_x, jb_y, e + 10.0],
                drawn_ft: jitter(&mut er, jb_x, jb_y),
                mounting_in: 120.0,
                mounting_note: Some("CLG".into()),
                arch_positioned: true,
                circuit: String::new(),
                panel: lp_id.clone(),
                sheet: lsheet.clone(),
            });
        }

        // Conduit runs: from panels to the corridor, then branching to rooms.
        // Only draw a few representative conduit runs (not every one).
        let corr_y = (y1 + y2) / 2.0;
        // Main conduit from lighting panel to corridor.
        {
            conduit_n += 1;
            let cid = format!("EC{conduit_n:03}");
            let size_in = 1.0;
            t.elec_conduits.push(ElecConduit {
                id: cid,
                conduit_type: er.pick(&["EMT", "EMT", "MC"]).into(),
                size: "1".into(),
                size_in,
                level: lv.into(),
                from_ft: [panel_x, panel_y],
                to_ft: [panel_x, corr_y],
                tag: Some(Tag { text: vec!["1\"C".into()], at_ft: [panel_x + 0.5, (panel_y + corr_y) / 2.0], leader: false }),
                sheet: lsheet.clone(),
            });
        }
        // Branch conduits along corridor on the lighting sheet.
        let corr_x0 = xs.first().copied().unwrap_or(0.0);
        let corr_x1 = width;
        {
            conduit_n += 1;
            let cid = format!("EC{conduit_n:03}");
            t.elec_conduits.push(ElecConduit {
                id: cid,
                conduit_type: "EMT".into(),
                size: "3/4".into(),
                size_in: 0.75,
                level: lv.into(),
                from_ft: [corr_x0, corr_y],
                to_ft: [corr_x1, corr_y],
                tag: Some(Tag { text: vec!["3/4\"C".into()], at_ft: [(corr_x0 + corr_x1) / 2.0, corr_y + 0.5], leader: false }),
                sheet: lsheet.clone(),
            });
        }
        // Power panel to corridor conduit.
        {
            conduit_n += 1;
            let cid = format!("EC{conduit_n:03}");
            t.elec_conduits.push(ElecConduit {
                id: cid,
                conduit_type: "EMT".into(),
                size: "1-1/4".into(),
                size_in: 1.25,
                level: lv.into(),
                from_ft: [panel_x + 3.0, panel_y],
                to_ft: [panel_x + 3.0, corr_y],
                tag: Some(Tag { text: vec!["1-1/4\"C".into()], at_ft: [panel_x + 3.5, (panel_y + corr_y) / 2.0], leader: false }),
                sheet: psheet.clone(),
            });
        }

        let _ = (light_ckt, power_ckt); // last values unused after the loop
        // Fill panel schedules.
        if let Some(lp) = t.elec_panels.iter_mut().find(|p| p.id == lp_id) {
            lp.schedule = light_schedule_rows;
        }
        if let Some(pp) = t.elec_panels.iter_mut().find(|p| p.id == pp_id) {
            pp.schedule = power_schedule_rows;
        }
    }

    // Light fixture schedule as a general note on the drawing (truth only, drawn separately).
    // The schedule exists in the truth for the E-601 sheet.
}

/// The electrical gear's schedule data, lug provisions and feeders: the switchboard MDP (the
/// service), the transformer TX-1, every floor's panels, the disconnect DS-1, with four problems
/// planted where the field finds them (`Truth::gear_problems`): the MDP's neutral bar short for
/// the parallel service neutrals, its top entry plate too small for the feeders leaving it, one
/// power panel's CU-only main lugs on an aluminium feeder, one lighting panel's main lugs too
/// small for its feeder; and the transformer's secondary lugs ordered separately (a lug kit).
fn gear_design(t: &mut Truth, floors: &[Floor], xs: &[f64], r: &mut Rng) {
    use cadaddy_walk3d::{Enclosure, Feeder, Termination};
    let Some(first) = floors.first() else { return };
    let lv1 = first.code.clone();
    let panel_x = xs.first().copied().unwrap_or(0.0) + 2.0;
    let panel_y = 2.0;
    let lug = |point: &str, lugs: u32, range: &str, material: &str| Termination {
        point: point.into(),
        lugs,
        range: range.into(),
        material: material.into(),
        ..Termination::default()
    };
    let size_text = |s: [f64; 3]| format!("{}\"W x {}\"D x {}\"H", s[0], s[1], s[2]);
    let entry_text = |e: [f64; 2]| if e[0] > 0.0 { format!("{}\" x {}\"", e[0], e[1]) } else { "NONE".to_string() };
    // The switchboard and the disconnect, beside the floor's panels.
    for (id_n, tag, kind, dx, w, d) in [(0, "MDP", "switchboard", 10.0, 72.0, 36.0), (1, "DS-1", "disconnect", 15.0, 16.0, 8.0)] {
        let (cx, cy) = (panel_x + dx, panel_y + d / 24.0);
        t.elec_panels.push(ElecPanel {
            id: format!("EG{:02}", id_n + 1),
            tag: tag.into(),
            kind: kind.into(),
            level: lv1.clone(),
            room: format!("ELEC-{lv1}"),
            at_ft: [cx, cy, first.elev],
            min_ft: [cx - w / 24.0, cy - d / 24.0],
            max_ft: [cx + w / 24.0, cy + d / 24.0],
            sheet: format!("E-20{lv1}"),
            ..ElecPanel::default()
        });
    }
    let n_floors = floors.len();
    let bad_p = floors.get(r.range(0, n_floors.max(1))).map(|f| format!("P{}", f.code)).unwrap_or_default();
    let bad_l = floors.get(r.range(0, n_floors.max(1))).map(|f| format!("L{}", f.code)).unwrap_or_default();
    // Feeders: the service, the transformer, every floor's power and lighting panels, the disconnect.
    let mut feeders = vec![Feeder {
        tag: "F-SVC".into(),
        sets: 4,
        size: "600 kcmil".into(),
        material: "al".into(),
        grounds: 0,
        extra_grounds: vec!["3/0".into(), "#2".into()],
        conduits: 4,
        conduit_in: 4.0,
        entry: "bottom".into(),
        to: "MDP".into(),
        ..Feeder::default()
    }];
    feeders.push(Feeder {
        tag: "F-TX1".into(),
        from: "MDP".into(),
        to: "TX-1".into(),
        size: "#8".into(),
        neutrals: 0,
        ground_size: "#10".into(),
        conduit_in: 0.75,
        ..Feeder::default()
    });
    for f in floors {
        feeders.push(Feeder {
            tag: format!("F-P{}", f.code),
            from: "MDP".into(),
            to: format!("P{}", f.code),
            size: "3/0".into(),
            material: "al".into(),
            ground_size: "#6".into(),
            conduit_in: 2.0,
            ..Feeder::default()
        });
    }
    for f in floors {
        feeders.push(Feeder {
            tag: format!("F-L{}", f.code),
            from: "TX-1".into(),
            to: format!("L{}", f.code),
            size: "1/0".into(),
            ground_size: "#6".into(),
            conduit_in: 1.5,
            ..Feeder::default()
        });
    }
    feeders.push(Feeder {
        tag: "F-DS1".into(),
        from: "MDP".into(),
        to: "DS-1".into(),
        size: "#6".into(),
        neutrals: 0,
        ground_size: "#10".into(),
        conduit_in: 1.0,
        ..Feeder::default()
    });
    // The MDP's neutral bar: one position short of the service's four parallel neutrals and the
    // power panels' neutrals. Its ground bar: room to spare.
    let mdp_neutrals = u32::try_from(4 + n_floors).unwrap_or(8);
    let mdp_grounds = u32::try_from(2 + 1 + n_floors + 1).unwrap_or(8);
    let mut problems = vec![("MDP".to_string(), "neutral bar".to_string()), ("MDP".to_string(), "entry".to_string())];
    for p in &mut t.elec_panels {
        let mut props = BTreeMap::new();
        let (size, bottom, mounting, voltage, rating, typ) = match p.kind.as_str() {
            "switchboard" => {
                p.terminations = vec![
                    lug("main", 4, "#2-600 kcmil", "cu/al"),
                    lug("breaker", 1, "#14-350 kcmil", "cu/al"),
                    lug("neutral_bar", mdp_neutrals - 1, "#6-600 kcmil", "cu/al"),
                    lug("ground_bar", mdp_grounds + 4, "#14-4/0", "cu/al"),
                ];
                // The top plate: too small for the feeders leaving it.
                p.enclosure = Some(Enclosure { top_entry_in: [6.0, 3.0], bottom_entry_in: [60.0, 20.0], gutter_in: [12.0, 20.0] });
                ([72.0, 36.0, 90.0], 0.0, "FLOOR".to_string(), "480Y/277V, 3PH, 4W", "2000A", "SWITCHBOARD")
            }
            "transformer" => {
                let secondary = Termination {
                    point: "secondary".into(),
                    lugs: 1,
                    range: "#6-250 kcmil".into(),
                    material: "cu/al".into(),
                    lug_type: "compression".into(),
                    supplied: false,
                    bolt_in: 0.375,
                    holes: 1,
                    stack_in: 0.5,
                    ..Termination::default()
                };
                p.terminations = vec![
                    lug("primary", 1, "#14-1/0", "cu/al"),
                    secondary,
                    lug("neutral_bar", 6, "#6-250 kcmil", "cu/al"),
                    lug("ground_bar", 6, "#14-1/0", "cu/al"),
                ];
                ([18.0, 12.0, 30.0], 0.0, "FLOOR".to_string(), "480V-208Y/120V", "45 KVA", "TRANSFORMER")
            }
            "disconnect" => {
                p.terminations = vec![lug("line", 1, "#14-#2", "cu/al"), lug("load", 1, "#14-#2", "cu/al"), lug("ground_bar", 3, "#14-#4", "cu/al")];
                ([16.0, 8.0, 24.0], 48.0, "SURFACE, TOP @ 6'-0\"".to_string(), "480V, 3PH", "60A NF", "DISCONNECT SWITCH")
            }
            _ => {
                let power = p.tag.starts_with('P');
                let mut main =
                    lug("main", 1, if p.tag == bad_l { "#14-#2" } else { "#6-250 kcmil" }, if power && p.tag == bad_p { "cu" } else { "cu/al" });
                main.bending_in = 8.0;
                p.terminations = vec![
                    main,
                    lug("breaker", 1, "#14-#2", "cu/al"),
                    lug("neutral_bar", 42, "#14-4/0", "cu/al"),
                    lug("ground_bar", 30, "#14-#4", "cu/al"),
                ];
                if p.tag == bad_p {
                    problems.push((p.tag.clone(), "lug material".into()));
                }
                if p.tag == bad_l {
                    problems.push((p.tag.clone(), "lug range".into()));
                }
                let v = if power { "480Y/277V, 3PH, 4W" } else { "208Y/120V, 3PH, 4W" };
                ([18.0, 6.0, 60.0], 18.0, "SURFACE, TOP @ 6'-6\"".to_string(), v, "225A MLO", "PANELBOARD")
            }
        };
        p.size_in = size;
        p.bottom_in = bottom;
        if let Some(e) = &p.enclosure {
            props.insert("TOP ENTRY".to_string(), entry_text(e.top_entry_in));
            props.insert("BOTTOM ENTRY".to_string(), entry_text(e.bottom_entry_in));
            props.insert("GUTTER".to_string(), entry_text(e.gutter_in));
        }
        props.insert("TYPE".to_string(), typ.to_string());
        props.insert("VOLTAGE".to_string(), voltage.to_string());
        props.insert("RATING".to_string(), rating.to_string());
        props.insert("SIZE".to_string(), size_text(size));
        props.insert("MOUNTING".to_string(), mounting);
        p.properties = props;
    }
    problems.sort();
    t.elec_feeders = feeders;
    t.gear_problems = problems;
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
