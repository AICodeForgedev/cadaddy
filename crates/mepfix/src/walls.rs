//! Walls and their assemblies: the wall type legend (partition types sheet) and the architectural
//! floor plans with a tag on (or beside, with a leader) most walls.
//!
//! Each wall type is a made-up but ordinary build-up — gypsum board layers per side (regular,
//! Type X, moisture- and abuse-resistant, shaft liner), metal studs (size, gauge, spacing), cavity
//! insulation, resilient or furring channels, CMU, sheathing — with a height rule (to the ceiling,
//! 6" past it, to the underside of the deck, full height), a fire rating and an STC rating. How the
//! legend words all this varies by seed, as offices' standards do; so do the tags (`B`, `1B`,
//! `P3`, `W-4`) and their symbols.
//!
//! The truth gives every wall its type and its height: the rule applied to the ceilings of the
//! rooms either side (the higher one) and the underside of the deck above.

use crate::draw::Pen;
use crate::rng::Rng;
use crate::truth::*;
use cadaddy_doc::{HAlign, VAlign};
use cadaddy_geom::Vec2;
use std::f64::consts::{FRAC_PI_2, PI};

/// How the legend and the plans word and draw walls.
#[derive(Clone, Debug, Default)]
pub struct WallStyle {
    /// Tags: 0 letters (`A`), 1 number + letter (`1A`), 2 `P1` (exterior `X1`), 3 `W-1`.
    pub scheme: u8,
    /// Gypsum board: 0 `GYP. BD.`, 1 `GWB`, 2 `GYPSUM BOARD`.
    pub gyp: u8,
    /// Type X: 0 `TYPE X`, 1 `TYPE 'X'`.
    pub type_x: u8,
    /// Moisture-/abuse-resistant spelled out (otherwise `MR` / `AR`).
    pub spelled: bool,
    /// Side labels: 0 `SIDE 1:`/`SIDE 2:`, 1 `SIDE A:`/`SIDE B:`, 2 `ONE SIDE:`/`OTHER SIDE:`.
    pub sides: u8,
    /// Same both sides: `EACH SIDE` (otherwise `BOTH SIDES`).
    pub each: bool,
    /// Layer counts: 0 `(2) LAYERS`, 1 `2 LAYERS`, 2 `TWO LAYERS`.
    pub count: u8,
    /// Studs: 0 `3-5/8" 20 GA. METAL STUDS @ 16" O.C.`, 1 `3-5/8" MTL. STUDS, 20 GA., 16" O.C.`.
    pub studs: u8,
    /// Height lines: 0 `HEIGHT: …`, 1 `EXTEND …`.
    pub height: u8,
    /// Ratings: 0 `FIRE RATING: 1 HR`, 1 `1-HOUR RATED ASSEMBLY`, 2 `RATING: 1 HOUR`.
    pub rating: u8,
    /// `STC 45`, `STC: 45`, `MIN. STC 45`.
    pub stc: u8,
    /// Overall thickness printed.
    pub overall: bool,
    /// Fractions written `4-7/8"` (otherwise `4 7/8"`).
    pub dash: bool,
    /// Tag symbols: 0 hexagon, 1 diamond, 2 circle, 3 rectangle.
    pub symbol: u8,
    /// Sheet title: 0 `PARTITION TYPES`, 1 `WALL TYPES`, 2 `PARTITION SCHEDULE`.
    pub title: u8,
    /// Share of tags on the wall itself (the rest beside it with a leader).
    pub on_wall: f64,
    /// Share of walls tagged.
    pub tagged: f64,
    /// The partition goes 6" past the ceiling (otherwise to the ceiling).
    pub above_ceiling: bool,
    /// Insulation in the plain partitions.
    pub part_insulated: bool,
    /// The corridor walls get abuse-resistant board on the corridor side.
    pub abuse: bool,
    /// The masonry walls are furred out with gypsum board inside.
    pub furred: bool,
}

impl WallStyle {
    pub fn new(r: &mut Rng) -> WallStyle {
        WallStyle {
            scheme: r.pick(&[0, 1, 2, 3]),
            gyp: r.pick(&[0, 1, 2]),
            type_x: r.pick(&[0, 1]),
            spelled: r.chance(0.5),
            sides: r.pick(&[0, 1, 2]),
            each: r.chance(0.6),
            count: r.pick(&[0, 0, 1, 2]),
            studs: r.pick(&[0, 1]),
            height: r.pick(&[0, 1]),
            rating: r.pick(&[0, 1, 2]),
            stc: r.pick(&[0, 1, 2]),
            overall: r.chance(0.6),
            dash: r.chance(0.6),
            symbol: r.pick(&[0, 1, 2, 3]),
            title: r.pick(&[0, 1, 2]),
            on_wall: r.pick::<f64>(&[0.0, 0.3, 0.6]),
            tagged: r.pick::<f64>(&[0.75, 0.85, 0.9]),
            above_ceiling: r.chance(0.6),
            part_insulated: r.chance(0.5),
            abuse: r.chance(0.4),
            furred: r.chance(0.5),
        }
    }

    pub fn sheet_title(&self) -> &'static str {
        ["PARTITION TYPES", "WALL TYPES", "PARTITION SCHEDULE"].get(self.title as usize).copied().unwrap_or("PARTITION TYPES")
    }
}

/// What a wall type is for (which walls get it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Partition,
    Corridor,
    Exterior,
    Wet,
    Acoustic,
    Masonry,
    Shaft,
}

const ROLES: [Role; 7] = [Role::Partition, Role::Corridor, Role::Acoustic, Role::Wet, Role::Masonry, Role::Shaft, Role::Exterior];

fn layer(material: &str, thickness_in: f64, side: &str) -> WallLayer {
    WallLayer { material: material.into(), thickness_in, side: side.into(), gauge: None, spacing_in: None, in_cavity: false }
}

fn stud(thickness_in: f64, gauge: u32, spacing_in: f64) -> WallLayer {
    WallLayer { gauge: Some(gauge), spacing_in: Some(spacing_in), ..layer("stud", thickness_in, "core") }
}

fn batt(thickness_in: f64) -> WallLayer {
    WallLayer { in_cavity: true, ..layer("insulation", thickness_in, "core") }
}

/// A role's build-up.
fn build(role: Role, ws: &WallStyle) -> (String, Vec<WallLayer>, &'static str, Option<&'static str>, Option<u32>) {
    let both = |m: &str, n: usize, core: Vec<WallLayer>| {
        let mut v: Vec<WallLayer> = (0..n).map(|_| layer(m, 0.625, "1")).collect();
        v.extend(core);
        v.extend((0..n).map(|_| layer(m, 0.625, "2")));
        v
    };
    match role {
        Role::Partition => {
            let mut core = vec![stud(3.625, 25, 16.0)];
            if ws.part_insulated {
                core.push(batt(3.5));
            }
            let h = if ws.above_ceiling { "above_ceiling" } else { "ceiling" };
            ("NON-RATED PARTITION".into(), both("gyp", 1, core), h, None, ws.part_insulated.then_some(40))
        }
        Role::Corridor => {
            let mut v = both("gyp_x", 1, vec![stud(3.625, 20, 16.0), batt(3.5)]);
            if ws.abuse
                && let Some(l) = v.first_mut()
            {
                l.material = "gyp_ar".into();
            }
            ("1 HOUR RATED CORRIDOR PARTITION".into(), v, "deck", Some("1 HR"), Some(45))
        }
        Role::Acoustic => {
            let mut v = vec![layer("gyp", 0.625, "1"), layer("gyp", 0.625, "1"), stud(3.625, 20, 16.0), batt(3.5)];
            v.push(layer("resilient_channel", 0.5, "2"));
            v.push(layer("gyp", 0.625, "2"));
            ("ACOUSTIC PARTITION".into(), v, "deck", None, Some(55))
        }
        Role::Wet => ("PLUMBING CHASE WALL".into(), both("gyp_mr", 1, vec![stud(6.0, 20, 16.0), batt(3.5)]), "deck", None, Some(45)),
        Role::Masonry => {
            let mut v = vec![layer("cmu", 8.0, "core")];
            if ws.furred {
                v.push(layer("furring", 0.875, "2"));
                v.push(layer("gyp_x", 0.625, "2"));
            }
            ("2 HOUR MASONRY WALL".into(), v, "deck", Some("2 HR"), Some(50))
        }
        Role::Shaft => {
            let v = vec![layer("shaftliner", 1.0, "1"), stud(2.5, 25, 24.0), layer("gyp_x", 0.625, "2"), layer("gyp_x", 0.625, "2")];
            ("2 HOUR SHAFT WALL".into(), v, "deck", Some("2 HR"), None)
        }
        Role::Exterior => {
            let v = vec![layer("sheathing", 0.625, "1"), stud(6.0, 18, 16.0), batt(6.0), layer("gyp_x", 0.625, "2")];
            ("EXTERIOR WALL".into(), v, "full", None, None)
        }
    }
}

fn tags(scheme: u8) -> Vec<String> {
    match scheme {
        1 => ["1A", "1B", "2A", "2B", "3A", "3B", "4A"].map(String::from).to_vec(),
        2 => ["P1", "P2", "P3", "P4", "P5", "P6", "P7"].map(String::from).to_vec(),
        3 => ["W-1", "W-2", "W-3", "W-4", "W-5", "W-6", "W-7"].map(String::from).to_vec(),
        _ => ["A", "B", "C", "D", "E", "F", "G"].map(String::from).to_vec(),
    }
}

/// The legend's wall types; returns them with the role of each.
pub fn wall_types(ws: &WallStyle, r: &mut Rng) -> Vec<(Role, WallType)> {
    let mut order = ROLES.to_vec();
    // Shuffled, so a tag says nothing about the role.
    for i in (1..order.len()).rev() {
        order.swap(i, r.below(i + 1));
    }
    let names = tags(ws.scheme);
    let mut out = Vec::new();
    for (i, role) in order.into_iter().enumerate() {
        let tag = if ws.scheme == 2 && role == Role::Exterior { "X1".to_string() } else { names.get(i).cloned().unwrap_or_else(|| format!("T{i}")) };
        let (name, layers, height, fire, stc) = build(role, ws);
        let thickness_in = layers.iter().filter(|l| !l.in_cavity).map(|l| l.thickness_in).sum();
        out.push((
            role,
            WallType {
                tag,
                name,
                layers,
                thickness_in,
                height: height.into(),
                fire_rating: fire.map(str::to_string),
                stc,
                external: role == Role::Exterior,
            },
        ));
    }
    // Tags in legend order.
    out.sort_by(|a, b| a.1.tag.cmp(&b.1.tag));
    out
}

// ---- The walls ---------------------------------------------------------------------------------

fn r6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// The level code above `level`.
fn above(level: &str) -> &'static str {
    if level == "1" { "2" } else { "R" }
}

/// The underside of the deck over `level` (it bears on the top of the steel), above its floor,
/// feet.
pub fn deck_ft(t: &Truth, level: &str) -> f64 {
    let elev = |c: &str| t.level(c).map_or(0.0, |l| l.elevation_ft);
    let up = above(level);
    let tos = t.building.beams.iter().find(|b| b.level == up).map_or(elev(up) - (t.building.slab_in + 1.5) / 12.0, |b| b.tos_ft);
    tos - elev(level)
}

/// A wall's height by its type's rule: `ceiling` the higher ceiling either side (none: the deck).
pub fn rule_height(rule: &str, ceiling: Option<f64>, deck: f64) -> f64 {
    match (rule, ceiling) {
        ("ceiling", Some(c)) => c.min(deck),
        ("above_ceiling", Some(c)) => (c + 0.5).min(deck),
        _ => deck,
    }
}

/// The rooms of `level` either side of a wall `th_ft` thick from `a` to `b` (sampled along it).
pub fn rooms_beside<'a>(rooms: impl Iterator<Item = &'a Room> + Clone, a: [f64; 2], b: [f64; 2], th_ft: f64) -> Vec<&'a Room> {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let mut out: Vec<&Room> = Vec::new();
    if len <= 1e-9 {
        return out;
    }
    let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
    let n = [-u[1], u[0]];
    let off = th_ft / 2.0 + 0.75;
    let steps = ((len / 2.0).ceil() as usize).clamp(1, 1000);
    for i in 0..=steps {
        let s = (0.5 + (len - 1.0) * i as f64 / steps as f64).min(len);
        for side in [-1.0, 1.0] {
            let p = [a[0] + u[0] * s + n[0] * off * side, a[1] + u[1] * s + n[1] * off * side];
            if let Some(rm) = rooms.clone().find(|r| p[0] > r.min_ft[0] && p[0] < r.max_ft[0] && p[1] > r.min_ft[1] && p[1] < r.max_ft[1])
                && !out.iter().any(|o| o.id == rm.id)
            {
                out.push(rm);
            }
        }
    }
    out
}

/// The walls of the building: types by what they separate, doors, tags (most walls), heights.
pub fn design_walls(t: &mut Truth, ws: &WallStyle, r: &mut Rng) {
    let types = wall_types(ws, r);
    let of = |role: Role| types.iter().find(|(x, _)| *x == role).map(|(_, w)| w.clone()).unwrap_or_default();
    let th = |role: Role| of(role).thickness_in / 12.0;
    let xs: Vec<f64> = t.building.grid.x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = t.building.grid.y.iter().map(|g| g.at_ft).collect();
    let (Some(&w), Some(&y1), Some(&y2), Some(&y3)) = (xs.last(), ys.get(1), ys.get(2), ys.get(3)) else {
        t.wall_types = types.into_iter().map(|(_, w)| w).collect();
        return;
    };
    let nb = xs.len().saturating_sub(1);
    let x = |i: usize| xs.get(i).copied().unwrap_or(0.0);
    let (te, tc, tm) = (th(Role::Exterior), th(Role::Corridor), th(Role::Masonry));
    let mut walls: Vec<Wall> = Vec::new();
    let mut n = 0usize;
    for lv in ["1", "2"] {
        let rooms: Vec<Room> = t.building.rooms.iter().filter(|rm| rm.level == lv).cloned().collect();
        let named = |p: [f64; 2]| rooms.iter().find(|rm| p[0] > rm.min_ft[0] && p[0] < rm.max_ft[0] && p[1] > rm.min_ft[1] && p[1] < rm.max_ft[1]);
        let quiet = |p: [f64; 2]| named(p).is_some_and(|rm| rm.name == "CONFERENCE" || rm.name == "TRAINING");
        // (role, a, b)
        let mut list: Vec<(Role, [f64; 2], [f64; 2])> = vec![
            (Role::Exterior, [-te / 2.0, 0.0], [w + te / 2.0, 0.0]),
            (Role::Exterior, [w + te / 2.0, y3], [-te / 2.0, y3]),
            (Role::Exterior, [0.0, y3 - te / 2.0], [0.0, te / 2.0]),
            (Role::Exterior, [w, te / 2.0], [w, y3 - te / 2.0]),
            (Role::Masonry, [te / 2.0, y1], [x(1) + tm / 2.0, y1]),
            (Role::Corridor, [x(1) + tm / 2.0, y1], [w - te / 2.0, y1]),
            (Role::Corridor, [te / 2.0, y2], [w - te / 2.0, y2]),
        ];
        for i in 1..nb {
            let xi = x(i);
            let role = if i == 1 {
                Role::Masonry
            } else if quiet([xi - 2.0, y1 / 2.0]) || quiet([xi + 2.0, y1 / 2.0]) {
                Role::Acoustic
            } else {
                Role::Partition
            };
            let top = if i == 1 { tm } else { tc };
            list.push((role, [xi, te / 2.0], [xi, y1 - top / 2.0]));
            let ym = (y2 + y3) / 2.0;
            let role = if i + 1 == nb {
                Role::Wet
            } else if quiet([xi - 2.0, ym]) || quiet([xi + 2.0, ym]) {
                Role::Acoustic
            } else {
                Role::Partition
            };
            list.push((role, [xi, y2 + tc / 2.0], [xi, y3 - te / 2.0]));
        }
        // The toilet rooms' shared wall.
        let toilets: Vec<&Room> = rooms.iter().filter(|rm| rm.min_ft[1] >= y2 - 1e-6 && rm.min_ft[0] >= x(nb.saturating_sub(1)) - 1e-6).collect();
        if toilets.len() == 2
            && let Some(mid) = toilets.iter().map(|rm| rm.max_ft[0]).reduce(f64::min)
        {
            list.push((Role::Wet, [mid, y2 + tc / 2.0], [mid, y3 - te / 2.0]));
        }
        let deck = deck_ft(t, lv);
        let start = walls.len();
        for (role, a, b) in list {
            let wt = of(role);
            n += 1;
            let beside = rooms_beside(rooms.iter(), a, b, wt.thickness_in / 12.0);
            let ceiling = beside.iter().filter_map(|rm| rm.ceiling.as_ref().map(|c| c.height_ft)).reduce(f64::max);
            let height = rule_height(&wt.height, ceiling, deck);
            walls.push(Wall {
                id: format!("W{lv}-{n:03}"),
                level: lv.into(),
                a_ft: [r6(a[0]), r6(a[1])],
                b_ft: [r6(b[0]), r6(b[1])],
                wall_type: wt.tag.clone(),
                tagged: true,
                height_ft: r6(height),
                rooms: beside.iter().map(|rm| rm.number.clone()).collect(),
                openings: Vec::new(),
                tags: Vec::new(),
                layout_from: None,
            });
        }
        // Doors: one into each room from the corridor (two toilet rooms share a bay), and the
        // entrance at the corridor's west end on the ground floor.
        let door = 3.0;
        for rm in &rooms {
            if rm.name == "CORRIDOR" {
                continue;
            }
            let south = rm.max_ft[1] <= y1 + 1e-6;
            let wy = if south { y1 } else { y2 };
            let lo = rm.min_ft[0] + 1.0 + door / 2.0 + if rm.min_ft[0] < 1e-6 { te } else { 0.5 };
            let hi = rm.max_ft[0] - 1.0 - door / 2.0 - 0.5;
            if hi <= lo {
                continue;
            }
            let c = lo + (hi - lo) * (r.below(1000) as f64 / 1000.0);
            if let Some(wall) = walls.iter_mut().skip(start).find(|wl| {
                (wl.a_ft[1] - wy).abs() < 1e-6
                    && (wl.b_ft[1] - wy).abs() < 1e-6
                    && c > wl.a_ft[0].min(wl.b_ft[0]) + 2.0
                    && c < wl.a_ft[0].max(wl.b_ft[0]) - 2.0
            }) {
                let s = (c - wall.a_ft[0]).abs();
                wall.openings.push([r6(s - door / 2.0), r6(s + door / 2.0)]);
            }
        }
        if lv == "1"
            && let Some(wall) = walls.get_mut(start + 2)
        {
            // West wall, north to south: the corridor's middle.
            let s = wall.a_ft[1] - (y1 + y2) / 2.0;
            wall.openings.push([r6(s - door / 2.0), r6(s + door / 2.0)]);
        }
        for wall in walls.iter_mut().skip(start) {
            wall.openings.sort_by(|p, q| p[0].total_cmp(&q[0]));
        }
        // Where other walls meet each wall (tags keep clear of those).
        let ends: Vec<[f64; 2]> = walls.iter().skip(start).flat_map(|wl| [wl.a_ft, wl.b_ft]).collect();
        for wall in walls.iter_mut().skip(start) {
            let len = (wall.b_ft[0] - wall.a_ft[0]).hypot(wall.b_ft[1] - wall.a_ft[1]);
            if len < 1.0 {
                continue;
            }
            let u = [(wall.b_ft[0] - wall.a_ft[0]) / len, (wall.b_ft[1] - wall.a_ft[1]) / len];
            let tees: Vec<f64> = ends
                .iter()
                .filter_map(|p| {
                    let d = [p[0] - wall.a_ft[0], p[1] - wall.a_ft[1]];
                    let s = d[0] * u[0] + d[1] * u[1];
                    let off = (d[0] * u[1] - d[1] * u[0]).abs();
                    (off < 1.5 && s > 0.5 && s < len - 0.5).then_some(s)
                })
                .collect();
            let clear = |s: f64| {
                s > 3.0 && s < len - 3.0 && wall.openings.iter().all(|o| s < o[0] - 2.5 || s > o[1] + 2.5) && tees.iter().all(|x| (s - x).abs() > 3.5)
            };
            wall.tagged = r.chance(ws.tagged);
            if !wall.tagged {
                continue;
            }
            let wanted = if len > 50.0 { vec![0.25, 0.75] } else { vec![0.5] };
            for f in wanted {
                let s = [0.0, 0.08, -0.08, 0.16, -0.16, 0.24, -0.24].iter().map(|d| (f + d) * len).find(|s| clear(*s));
                let Some(s) = s else { continue };
                // Beside it: towards the rooms (inside for exterior walls; away from the
                // corridor for corridor walls), else either way.
                let n = [-u[1], u[0]];
                let mid = [wall.a_ft[0] + u[0] * s, wall.a_ft[1] + u[1] * s];
                let toward = |p: [f64; 2]| if (p[0] - mid[0]) * n[0] + (p[1] - mid[1]) * n[1] >= 0.0 { 1.0 } else { -1.0 };
                let side = if wall.rooms.len() <= 1 {
                    toward([w / 2.0, y3 / 2.0])
                } else if (wall.a_ft[1] - wall.b_ft[1]).abs() < 1e-6 && ((wall.a_ft[1] - y1).abs() < 1e-6 || (wall.a_ft[1] - y2).abs() < 1e-6) {
                    -toward([mid[0], (y1 + y2) / 2.0])
                } else if r.chance(0.5) {
                    1.0
                } else {
                    -1.0
                };
                let shift = (r.below(5) as f64 - 2.0) * 0.75;
                let shift = if clear(s + shift) { shift } else { 0.0 };
                wall.tags.push(WallTag { s_ft: r6(s), on_wall: r.chance(ws.on_wall), side, shift_ft: shift, gap_ft: 0.5 + r.below(4) as f64 * 0.4 });
            }
            if wall.tags.is_empty() {
                wall.tagged = false;
            }
        }
    }
    t.wall_types = types.into_iter().map(|(_, w)| w).collect();
    t.walls = walls;
}

// ---- How the legend words it -------------------------------------------------------------------

/// A length in inches as the legend prints it: `5/8"`, `1"`, `3-5/8"` (or `3 5/8"`).
pub fn frac(v: f64, dash: bool) -> String {
    let eighths = (v * 8.0).round() as i64;
    let (whole, rest) = (eighths / 8, eighths % 8);
    let f = match rest {
        0 => String::new(),
        2 | 6 => format!("{}/4", rest / 2),
        4 => "1/2".into(),
        _ => format!("{rest}/8"),
    };
    match (whole, f.is_empty()) {
        (0, false) => format!("{f}\""),
        (w, true) => format!("{w}\""),
        (w, false) => format!("{w}{}{f}\"", if dash { "-" } else { " " }),
    }
}

fn count_words(n: usize, ws: &WallStyle) -> String {
    let s = if n == 1 { "LAYER" } else { "LAYERS" };
    match ws.count {
        1 => format!("{n} {s}"),
        2 => format!("{} {s}", ["ZERO", "ONE", "TWO", "THREE", "FOUR"].get(n).copied().unwrap_or("SEVERAL")),
        _ => format!("({n}) {s}"),
    }
}

fn board_words(m: &str, ws: &WallStyle) -> String {
    let gyp = ["GYP. BD.", "GWB", "GYPSUM BOARD"].get(ws.gyp as usize).copied().unwrap_or("GYP. BD.");
    let x = if ws.type_x == 1 { "TYPE 'X'" } else { "TYPE X" };
    match m {
        "gyp_x" => format!("{x} {gyp}"),
        "gyp_mr" => format!("{} {gyp}", if ws.spelled { "MOISTURE-RESISTANT" } else { "MR" }),
        "gyp_ar" => format!("{} {gyp}", if ws.spelled { "ABUSE-RESISTANT" } else { "AR" }),
        "shaftliner" => "SHAFTLINER PANEL".into(),
        "sheathing" => "EXTERIOR SHEATHING".into(),
        "resilient_channel" => "RESILIENT CHANNEL".into(),
        "furring" => "FURRING CHANNEL".into(),
        _ => gyp.into(),
    }
}

fn is_board(m: &str) -> bool {
    m.starts_with("gyp")
}

/// One side's layers, outer face first, as the legend lists them.
fn side_words(layers: &[&WallLayer], ws: &WallStyle) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(l) = layers.get(i) {
        let mut n = 1;
        while layers.get(i + n).is_some_and(|o| o.material == l.material && (o.thickness_in - l.thickness_in).abs() < 1e-9) {
            n += 1;
        }
        let what = format!("{} {}", frac(l.thickness_in, ws.dash), board_words(&l.material, ws));
        if is_board(&l.material) {
            parts.push(format!("{} {what}", count_words(n, ws)));
        } else {
            for _ in 0..n {
                parts.push(what.clone());
            }
        }
        i += n;
    }
    parts.join(", ")
}

/// The legend's lines for a wall type (the tag is in its symbol).
pub fn legend_lines(wt: &WallType, ws: &WallStyle) -> Vec<String> {
    let mut out = vec![wt.name.clone()];
    let s1: Vec<&WallLayer> = wt.layers.iter().filter(|l| l.side == "1").collect();
    // Side 2 printed outer face first.
    let s2: Vec<&WallLayer> = wt.layers.iter().rev().filter(|l| l.side == "2").collect();
    let labels = match ws.sides {
        1 => ("SIDE A:", "SIDE B:"),
        2 => ("ONE SIDE:", "OTHER SIDE:"),
        _ => ("SIDE 1:", "SIDE 2:"),
    };
    let same = s1.len() == s2.len() && s1.iter().zip(&s2).all(|(a, b)| a.material == b.material && (a.thickness_in - b.thickness_in).abs() < 1e-9);
    if same && !s1.is_empty() {
        out.push(format!("{} {}", side_words(&s1, ws), if ws.each { "EACH SIDE" } else { "BOTH SIDES" }));
    } else {
        if !s1.is_empty() {
            out.push(format!("{} {}", labels.0, side_words(&s1, ws)));
        }
        if !s2.is_empty() {
            out.push(format!("{} {}", labels.1, side_words(&s2, ws)));
        }
    }
    for l in wt.layers.iter().filter(|l| l.side == "core") {
        let d = frac(l.thickness_in, ws.dash);
        match l.material.as_str() {
            "stud" => {
                let (g, o) = (l.gauge.unwrap_or(20), frac(l.spacing_in.unwrap_or(16.0), ws.dash));
                out.push(if ws.studs == 1 { format!("{d} MTL. STUDS, {g} GA., {o} O.C.") } else { format!("{d} {g} GA. METAL STUDS @ {o} O.C.") });
            }
            "insulation" if wt.external => out.push(format!("{d} THERMAL BATT INSULATION")),
            "insulation" => {
                out.push(if ws.studs == 1 { format!("{d} ACOUSTIC BATT INSUL.") } else { format!("{d} SOUND ATTENUATION BATT INSULATION") })
            }
            "cmu" => out.push(if ws.spelled { format!("{d} CONCRETE MASONRY UNITS") } else { format!("{d} CMU") }),
            m => out.push(format!("{d} {}", board_words(m, ws))),
        }
    }
    let h = match (wt.height.as_str(), ws.height) {
        ("ceiling", 0) => "HEIGHT: TO CEILING",
        ("ceiling", _) => "EXTEND TO CEILING",
        ("above_ceiling", 0) => "HEIGHT: 6\" ABOVE CEILING",
        ("above_ceiling", _) => "EXTEND TO CEILING + 6\"",
        ("deck", 0) => "HEIGHT: TO UNDERSIDE OF DECK",
        ("deck", _) => "EXTEND TO U/S OF STRUCTURE",
        (_, 0) => "HEIGHT: FULL HEIGHT",
        _ => "FULL HEIGHT, FLOOR TO STRUCTURE",
    };
    out.push(h.into());
    match &wt.fire_rating {
        Some(r) => {
            let hrs = r.split_whitespace().next().unwrap_or("1");
            out.push(match ws.rating {
                1 => format!("{hrs}-HOUR RATED ASSEMBLY"),
                2 => format!("RATING: {hrs} HOUR"),
                _ => format!("FIRE RATING: {r}"),
            });
        }
        None if ws.rating == 0 => out.push("FIRE RATING: NONE".into()),
        None => {}
    }
    if let Some(s) = wt.stc {
        out.push(match ws.stc {
            1 => format!("STC: {s}"),
            2 => format!("MIN. STC {s}"),
            _ => format!("STC {s}"),
        });
    }
    if ws.overall {
        out.push(format!("OVERALL: {}", frac(wt.thickness_in, ws.dash)));
    }
    if wt.external {
        out.push("CLADDING AND AIR BARRIER PER ELEVATIONS".into());
    }
    out
}

// ---- Drawing -----------------------------------------------------------------------------------

/// A tag symbol centred on `c`, `r` across (model units).
pub fn tag_symbol(pen: &mut Pen, layer: &str, c: Vec2, r: f64, kind: u8) {
    match kind {
        0 => {
            let pts: Vec<Vec2> = (0..6).map(|i| c + Vec2::new((i as f64 * PI / 3.0).cos(), (i as f64 * PI / 3.0).sin()) * r).collect();
            pen.poly(layer, &pts, true);
        }
        1 => {
            let pts = [c + Vec2::new(r, 0.0), c + Vec2::new(0.0, r), c + Vec2::new(-r, 0.0), c + Vec2::new(0.0, -r)];
            pen.poly(layer, &pts, true);
        }
        2 => pen.circle(layer, c, r),
        _ => {
            let (a, b) = (c - Vec2::new(r * 1.2, r * 0.75), c + Vec2::new(r * 1.2, r * 0.75));
            pen.rect(layer, a, b);
        }
    }
}

/// Text that reads upright: an angle turned into (-90°, 90°].
fn readable(a: f64) -> f64 {
    let mut a = a.rem_euclid(2.0 * PI);
    if a > FRAC_PI_2 + 1e-9 && a <= 3.0 * FRAC_PI_2 + 1e-9 {
        a -= PI;
    }
    if a > PI {
        a -= 2.0 * PI;
    }
    a
}

/// A level's floor plan: walls as face pairs with door openings and swings, room names, tags.
pub fn draw_plan(pen: &mut Pen, t: &Truth, ws: &WallStyle, level: &str, scale: f64) {
    let rot = readable(pen.rot());
    for rm in t.building.rooms.iter().filter(|r| r.level == level) {
        let c = [(rm.min_ft[0] + rm.max_ft[0]) / 2.0, (rm.min_ft[1] + rm.max_ft[1]) / 2.0];
        let p = pen.b(c);
        pen.block("ARCH-TEXT", p, 0.08, &[rm.name.clone(), rm.number.clone()], rot);
    }
    // Symbol size: 0.14" on paper.
    let r_ft = 0.14 / scale.max(1e-6);
    for wall in t.walls.iter().filter(|w| w.level == level) {
        let th = t.wall_types.iter().find(|x| x.tag == wall.wall_type).map_or(4.875, |x| x.thickness_in) / 12.0;
        let (a, b) = (wall.a_ft, wall.b_ft);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len <= 1e-9 {
            continue;
        }
        let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let n = [-u[1], u[0]];
        let at = |s: f64, o: f64| [a[0] + u[0] * s + n[0] * o, a[1] + u[1] * s + n[1] * o];
        // Faces between the openings, jambs at each opening.
        let mut s = 0.0;
        let mut pieces = Vec::new();
        for o in &wall.openings {
            pieces.push((s, o[0]));
            s = o[1];
        }
        pieces.push((s, len));
        for (s0, s1) in pieces {
            if s1 - s0 > 1e-6 {
                for side in [-1.0, 1.0] {
                    let (p, q) = (pen.b(at(s0, side * th / 2.0)), pen.b(at(s1, side * th / 2.0)));
                    pen.line("A-WALL", p, q);
                }
            }
        }
        for o in &wall.openings {
            for s in o {
                let (p, q) = (pen.b(at(*s, -th / 2.0)), pen.b(at(*s, th / 2.0)));
                pen.line("A-WALL", p, q);
            }
            // The leaf open at 90° into the room (left of a→b for the corridor's north wall,
            // where the rooms are north; right for the south one).
            let into = if wall.rooms.len() <= 1 { -1.0 } else { door_side(t, wall, level) };
            let w = o[1] - o[0];
            let hinge = at(o[0], into * th / 2.0);
            let tip = at(o[0], into * (th / 2.0 + w));
            let (ph, pt) = (pen.b(hinge), pen.b(tip));
            pen.line("A-DOOR", ph, pt);
            let a0 = (pt.y - ph.y).atan2(pt.x - ph.x);
            let close = pen.b(at(o[1], into * th / 2.0));
            let a1 = (close.y - ph.y).atan2(close.x - ph.x);
            let (s, e) = if (a1 - a0).rem_euclid(2.0 * PI) <= PI { (a0, a1) } else { (a1, a0) };
            pen.arc("A-DOOR", ph, pen.bl(w), s, e);
        }
        for tp in &wall.tags {
            let r = pen.bl(r_ft);
            if tp.on_wall {
                let c = pen.b(at(tp.s_ft, 0.0));
                tag_symbol(pen, "A-ANNO-WTAG", c, r, ws.symbol);
                pen.text("A-ANNO-WTAG", c, 0.09, &wall.wall_type, rot, HAlign::Center, VAlign::Middle);
            } else {
                let d = th / 2.0 + r_ft * 1.6 + tp.gap_ft;
                let c = at(tp.s_ft + tp.shift_ft, tp.side * d);
                let land = at(tp.s_ft, tp.side * th / 2.0);
                let (pc, pl) = (pen.b(c), pen.b(land));
                tag_symbol(pen, "A-ANNO-WTAG", pc, r, ws.symbol);
                pen.text("A-ANNO-WTAG", pc, 0.09, &wall.wall_type, rot, HAlign::Center, VAlign::Middle);
                let dir = pl - pc;
                let l = dir.len();
                if l > r * 1.3 {
                    let start = pc + dir * (r * 1.25 / l);
                    pen.line("A-ANNO-WTAG", start, pl);
                }
            }
        }
    }
}

/// Which side of a corridor wall its doors swing to: away from the corridor.
fn door_side(t: &Truth, wall: &Wall, level: &str) -> f64 {
    let corridor = t.building.rooms.iter().find(|r| r.level == level && r.name == "CORRIDOR");
    let (a, b) = (wall.a_ft, wall.b_ft);
    let len = (b[0] - a[0]).hypot(b[1] - a[1]).max(1e-9);
    let n = [-(b[1] - a[1]) / len, (b[0] - a[0]) / len];
    match corridor {
        Some(c) => {
            let mid = [(c.min_ft[0] + c.max_ft[0]) / 2.0, (c.min_ft[1] + c.max_ft[1]) / 2.0];
            if (mid[0] - a[0]) * n[0] + (mid[1] - a[1]) * n[1] >= 0.0 { -1.0 } else { 1.0 }
        }
        None => 1.0,
    }
}

/// The partition types sheet: per type its tag, a section through its layers and its lines.
pub fn draw_legend(pen: &mut Pen, t: &Truth, ws: &WallStyle, area: [[f64; 2]; 2]) {
    let [[x0, y0], [x1, y1]] = area;
    pen.ptext("TEXT", [x0 + 0.3, y1 - 0.4], 0.2, ws.sheet_title(), HAlign::Left);
    pen.pline("TEXT", [x0 + 0.3, y1 - 0.48], [x0 + 0.3 + ws.sheet_title().len() as f64 * 0.17, y1 - 0.48]);
    let mut x = x0 + 0.3;
    let mut y = y1 - 1.0;
    let col_w = 7.2;
    for wt in &t.wall_types {
        let lines = legend_lines(wt, ws);
        let h = (lines.len() as f64 * 0.22 + 0.2).max(1.5);
        if y - h < y0 + 0.3 {
            y = y1 - 1.0;
            x += col_w;
            if x + col_w > x1 + 1.0 {
                break;
            }
        }
        let c = pen.p([x + 0.3, y - 0.05]);
        let r = pen.pl(0.22);
        tag_symbol(pen, "A-ANNO-WTAG", c, r, ws.symbol);
        let tc = [x + 0.3, y - 0.05];
        let p = pen.p(tc);
        pen.text("A-ANNO-WTAG", p, 0.12, &wt.tag, 0.0, HAlign::Center, VAlign::Middle);
        // A section through the wall, 1" on paper for 6" of wall, layers left (side 1) to right.
        let k = 1.0 / 6.0;
        let (sy0, sy1) = (y - 1.35, y - 0.45);
        let mut sx = x + 0.1;
        for l in wt.layers.iter().filter(|l| !l.in_cavity) {
            let wdt = l.thickness_in * k;
            pen.prect("DETAIL", [sx, sy0], [sx + wdt, sy1]);
            if l.material == "stud" && wt.layers.iter().any(|o| o.in_cavity) {
                // Batt: a zigzag through the cavity.
                let pts: Vec<Vec2> =
                    (0..=8).map(|i| pen.p([sx + if i % 2 == 0 { 0.03 } else { wdt - 0.03 }, sy0 + (sy1 - sy0) * i as f64 / 8.0])).collect();
                pen.poly("DETAIL", &pts, false);
            }
            if l.material == "cmu" {
                pen.pline("DETAIL", [sx, sy0], [sx + wdt, sy1]);
                pen.pline("DETAIL", [sx, sy1], [sx + wdt, sy0]);
            }
            sx += wdt;
        }
        for (i, line) in lines.iter().enumerate() {
            let size = if i == 0 { 0.11 } else { 0.09 };
            pen.ptext("TEXT", [x + 2.0, y - i as f64 * 0.22], size, line, HAlign::Left);
        }
        y -= h + 0.35;
    }
}
