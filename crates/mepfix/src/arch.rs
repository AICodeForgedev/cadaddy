//! The rest of the architecture (Q307): doors and the door schedule (size, swing, frame,
//! hardware group, rating), windows and storefront with their schedule, the stair and the
//! elevator with their shafts and pit, slab edges, the roof plan with its curbs, and the room
//! finish schedule (floor, base, wall, ceiling) with its legend.
//!
//! Everything comes from a random stream of its own (like the walls), so the rest of a set stays
//! as it was. The doors are the wall openings the walls already have; one north room becomes
//! the stair and elevator core (on both levels) and keeps its walls, door and ceiling.
//!
//! The schedules' headers, column order and code spellings vary by seed, as offices' standards do.

use crate::draw::{Pen, table};
use crate::rng::Rng;
use crate::truth::*;
use crate::walls::tag_symbol;
use cadaddy_doc::{HAlign, VAlign};
use cadaddy_geom::Vec2;

/// How the architectural sheets word and draw these elements.
#[derive(Clone, Debug, Default)]
pub struct ArchStyle {
    /// Door marks: 0 the room number (`103`), 1 with a letter (`103A`), 2 `D103`.
    pub mark: u8,
    /// Door height, inches.
    pub door_h_in: f64,
    /// Office doors: half lite (`B`) or narrow lite (`C`).
    pub office_lite: bool,
    /// Door schedule header wording: 0 short, 1 long.
    pub headers: u8,
    /// Ratings printed `20 MIN` (otherwise `20`, under a `LABEL (MIN)` header).
    pub rating_min: bool,
    /// Rating column before the frame (otherwise last before hardware).
    pub rating_first: bool,
    /// Hardware groups: 0 `HW-1`, 1 `1`, 2 `HDW 1`.
    pub hw: u8,
    /// The mechanical room's door gets a card reader.
    pub mech_card: bool,
    /// Punched window size and sill, inches.
    pub window: [f64; 3],
    /// Level 1's south windows are storefront.
    pub storefront: bool,
    /// Window tag symbol (see [`tag_symbol`]; never the wall tags').
    pub window_symbol: u8,
    /// Slab edge beyond the exterior walls' outer face, inches.
    pub slab_beyond_in: f64,
    /// `EDGE OF SLAB` (otherwise `SLAB EDGE`).
    pub edge_words: bool,
    pub parapet_in: Option<f64>,
    /// Roof slope, inches per foot.
    pub slope: f64,
    pub drains: usize,
    /// Which north bay is the stair and elevator core.
    pub core_bay: usize,
    /// Flight width, inches.
    pub stair_w_in: f64,
    pub pit_ft: f64,
    pub capacity_lb: u32,
    /// Finish codes with a dash (`CPT-1`, otherwise `CPT1`).
    pub dash: bool,
    /// Finish schedule headers: 0 `ROOM NO.`…, 1 `NO.`…, 2 `RM #`….
    pub finish_headers: u8,
    /// Device plates in a column of the finish legend (otherwise in the description).
    pub plate_column: bool,
    /// Colours of the paint finishes.
    pub colours: [&'static str; 3],
}

impl ArchStyle {
    pub fn new(r: &mut Rng, wall_symbol: u8, nb: usize) -> ArchStyle {
        let window_symbol = [2u8, 1, 0, 3].iter().copied().find(|s| *s != wall_symbol).unwrap_or(2);
        let last = nb.saturating_sub(2);
        ArchStyle {
            mark: r.pick(&[0, 1, 2]),
            door_h_in: r.pick::<f64>(&[84.0, 84.0, 96.0]),
            office_lite: r.chance(0.5),
            headers: r.pick(&[0, 1]),
            rating_min: r.chance(0.6),
            rating_first: r.chance(0.5),
            hw: r.pick(&[0, 1, 2]),
            mech_card: r.chance(0.5),
            window: r.pick(&[[48.0, 60.0, 36.0], [60.0, 60.0, 30.0], [72.0, 54.0, 36.0], [48.0, 72.0, 24.0]]),
            storefront: r.chance(0.5),
            window_symbol,
            slab_beyond_in: r.pick::<f64>(&[3.0, 6.0, 12.0]),
            edge_words: r.chance(0.5),
            parapet_in: r.pick(&[None, Some(36.0), Some(42.0)]),
            slope: r.pick::<f64>(&[0.25, 0.25, 0.125]),
            drains: r.pick(&[2, 3]),
            core_bay: r.range(0, last + 1).min(last),
            stair_w_in: r.pick::<f64>(&[44.0, 48.0]),
            pit_ft: r.pick::<f64>(&[4.0, 4.5, 5.0, 5.5]),
            capacity_lb: r.pick(&[2500, 3000, 3500]),
            dash: r.chance(0.6),
            finish_headers: r.pick(&[0, 1, 2]),
            plate_column: r.chance(0.5),
            colours: r.pick(&[["LIGHT GRAY", "WHITE", "DEEP BLUE"], ["WARM BEIGE", "WHITE", "SAGE GREEN"], ["PALE GRAY", "OFF-WHITE", "TERRACOTTA"]]),
        }
    }

    /// A finish code as the schedule spells it.
    pub fn code(&self, prefix: &str, n: u32) -> String {
        if self.dash { format!("{prefix}-{n}") } else { format!("{prefix}{n}") }
    }

    pub fn hw_group(&self, n: u32) -> String {
        match self.hw {
            1 => format!("{n}"),
            2 => format!("HDW {n}"),
            _ => format!("HW-{n}"),
        }
    }
}

fn r6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

fn inside(rm: &Room, p: [f64; 2]) -> bool {
    p[0] > rm.min_ft[0] && p[0] < rm.max_ft[0] && p[1] > rm.min_ft[1] && p[1] < rm.max_ft[1]
}

/// Handing seen from `outside` (a unit normal towards the outside side of the door): standing
/// there facing the door, hinges on the right = `RH`; a door swinging towards you is reverse
/// (`RHR`). The rule every door hardware schedule uses.
pub fn hand(hinge: [f64; 2], latch: [f64; 2], into: [f64; 2], outside: [f64; 2]) -> String {
    let facing = [-outside[0], -outside[1]];
    let right = [facing[1], -facing[0]];
    let h = [hinge[0] - latch[0], hinge[1] - latch[1]];
    let rh = h[0] * right[0] + h[1] * right[1] > 0.0;
    let reverse = into[0] * outside[0] + into[1] * outside[1] > 0.0;
    format!("{}{}", if rh { "RH" } else { "LH" }, if reverse { "R" } else { "" })
}

/// Which side of a door is the outside: the exterior if one side is out of the building, else
/// the corridor, else the side it swings away from. `rooms`: the room names either side
/// (+normal side, -normal side); returns +1 or -1 along the normal.
pub fn outside_side(plus: Option<&str>, minus: Option<&str>, into: f64) -> f64 {
    match (plus, minus) {
        (None, Some(_)) => 1.0,
        (Some(_), None) => -1.0,
        (Some(p), Some(m)) if p == "CORRIDOR" && m != "CORRIDOR" => 1.0,
        (Some(p), Some(m)) if m == "CORRIDOR" && p != "CORRIDOR" => -1.0,
        _ => -into,
    }
}

fn wall_th(t: &Truth, wall: &PlanWall) -> f64 {
    t.wall_types.iter().find(|x| x.tag == wall.wall_type).map_or(4.875, |x| x.thickness_in) / 12.0
}

/// A wall's unit direction and left normal, and its length.
fn frame_of(wall: &PlanWall) -> Option<([f64; 2], [f64; 2], f64)> {
    let (a, b) = (wall.a_ft, wall.b_ft);
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    if len <= 1e-9 {
        return None;
    }
    let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
    Some((u, [-u[1], u[0]], len))
}

fn at(wall: &PlanWall, u: [f64; 2], n: [f64; 2], s: f64, o: f64) -> [f64; 2] {
    [wall.a_ft[0] + u[0] * s + n[0] * o, wall.a_ft[1] + u[1] * s + n[1] * o]
}

/// The door in `wall` whose opening is `o`, if the set has doors.
pub fn door_at<'a>(t: &'a Truth, wall: &PlanWall, o: [f64; 2]) -> Option<&'a Door> {
    let (u, n, _) = frame_of(wall)?;
    let mid = at(wall, u, n, (o[0] + o[1]) / 2.0, 0.0);
    t.arch.as_ref()?.doors.iter().find(|d| d.wall == wall.id && (d.at_ft[0] - mid[0]).hypot(d.at_ft[1] - mid[1]) < 0.1)
}

// ---- Design ------------------------------------------------------------------------------------

/// The stair and elevator core, doors, windows, slab edges, roof and finishes.
pub fn design_arch(t: &mut Truth, st: &ArchStyle, r: &mut Rng) {
    let mut arch = Arch::default();
    let xs: Vec<f64> = t.building.grid.x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = t.building.grid.y.iter().map(|g| g.at_ft).collect();
    let (Some(&w), Some(&y2), Some(&y3)) = (xs.last(), ys.get(2), ys.get(3)) else {
        t.arch = Some(arch);
        return;
    };
    let x = |i: usize| xs.get(i).copied().unwrap_or(0.0);
    let f2f = t.building.levels.iter().find(|l| l.code == "2").map_or(13.5, |l| l.elevation_ft);
    let te = t.wall_types.iter().find(|w| w.external).map_or(8.0, |w| w.thickness_in) / 12.0;

    // ---- The stair and elevator core: one north room on both levels ----
    let (cx0, cx1) = (x(st.core_bay), x(st.core_bay + 1));
    let mut core_rooms = Vec::new();
    for rm in t.building.rooms.iter_mut().filter(|rm| (rm.min_ft[0] - cx0).abs() < 1e-6 && (rm.min_ft[1] - y2).abs() < 1e-6) {
        rm.name = "STAIR".into();
        core_rooms.push(rm.number.clone());
    }
    let inset = 0.75;
    let (ix0, ix1, iy0, iy1) = (cx0 + inset, cx1 - inset, y2 + inset, y3 - inset);
    let risers = ((f2f * 12.0) / 7.0).ceil().max(2.0) as u32;
    let riser_in = f2f * 12.0 / risers as f64;
    let tread_in = 11.0;
    let n1 = risers.div_ceil(2);
    let wf = st.stair_w_in / 12.0;
    let run = (n1.saturating_sub(1)) as f64 * tread_in / 12.0;
    let depth = run + wf;
    let sy0 = (iy0 + 3.75).min(iy1 - depth).max(iy0);
    let sx1 = ix1 - 0.25;
    let sx0 = sx1 - (2.0 * wf + 0.5);
    arch.stairs.push(ArchStair {
        tag: "STAIR 1".into(),
        rooms: core_rooms.clone(),
        levels: vec!["1".into(), "2".into()],
        min_ft: [r6(sx0), r6(sy0)],
        max_ft: [r6(sx1), r6(sy0 + depth)],
        width_in: st.stair_w_in,
        risers,
        riser_in: r6(riser_in),
        tread_in,
    });
    let ex0 = ix0 + 0.5;
    let ey1 = iy1 - 0.5;
    arch.elevators.push(ArchElevator {
        tag: "ELEV. 1".into(),
        levels: vec!["1".into(), "2".into()],
        min_ft: [r6(ex0), r6(ey1 - 7.0)],
        max_ft: [r6(ex0 + 8.0), r6(ey1)],
        pit_ft: st.pit_ft,
        capacity_lb: st.capacity_lb,
    });

    // ---- Doors ----
    let mut hw_used: Vec<(u32, Vec<&str>, bool)> = Vec::new();
    let mut use_hw = |n: u32| {
        let (items, card): (Vec<&str>, bool) = match n {
            1 => (vec!["HINGES", "PASSAGE LATCH", "WALL STOP"], false),
            2 => (vec!["HINGES", "OFFICE LOCK", "WALL STOP"], false),
            3 => (vec!["HINGES", "STOREROOM LOCK", "CLOSER"], false),
            4 => (vec!["HINGES", "PRIVACY LOCK", "CLOSER", "KICK PLATE"], false),
            5 => (vec!["CONTINUOUS HINGE", "ELECTRIFIED STRIKE", "CARD READER", "CLOSER", "THRESHOLD"], true),
            6 => (vec!["HINGES", "PANIC DEVICE", "CLOSER", "SMOKE SEALS"], false),
            _ => (vec!["HINGES", "ELECTRIFIED STOREROOM LOCK", "CARD READER", "CLOSER"], true),
        };
        if !hw_used.iter().any(|h| h.0 == n) {
            hw_used.push((n, items, card));
        }
        n
    };
    let walls = t.plan_walls.clone();
    let rooms = t.building.rooms.clone();
    let mut marks: Vec<String> = Vec::new();
    for wall in &walls {
        let Some((u, n, _)) = frame_of(wall) else { continue };
        let th = wall_th(t, wall);
        let rating = t.wall_types.iter().find(|x| x.tag == wall.wall_type).and_then(|x| x.fire_rating.clone());
        for o in &wall.openings {
            let into = if wall.rooms.len() <= 1 { -1.0 } else { crate::walls::door_side(t, wall, &wall.level) };
            let mid = (o[0] + o[1]) / 2.0;
            let room_at = |side: f64| {
                let p = at(wall, u, n, mid, side * (th / 2.0 + 1.0));
                rooms.iter().filter(|rm| rm.level == wall.level).find(|rm| inside(rm, p))
            };
            let (plus, minus) = (room_at(1.0), room_at(-1.0));
            let (into_room, other) = if into > 0.0 { (plus, minus) } else { (minus, plus) };
            let (sh, sl) = if r.chance(0.5) { (o[0], o[1]) } else { (o[1], o[0]) };
            let hinge = at(wall, u, n, sh, into * th / 2.0);
            let latch = at(wall, u, n, sl, into * th / 2.0);
            let out = outside_side(plus.map(|r| r.name.as_str()), minus.map(|r| r.name.as_str()), into);
            let hd = hand(hinge, latch, [n[0] * into, n[1] * into], [n[0] * out, n[1] * out]);
            // The room the door serves: the one it swings into, else (out of the building) the
            // one it leaves.
            let serves = into_room.or(other);
            let name = serves.map_or("", |r| r.name.as_str());
            let exterior = into_room.is_none() || other.is_none();
            let num = serves.map_or_else(|| "0".to_string(), |r| r.number.clone());
            let mut mark = match st.mark {
                1 => format!("{num}A"),
                2 => format!("D{num}"),
                _ => num.clone(),
            };
            let mut k = 1u8;
            while marks.contains(&mark) {
                k += 1;
                let letter = char::from(b'A' + k.min(25) - 1);
                mark = if st.mark == 2 { format!("D{num}{letter}") } else { format!("{num}{letter}") };
            }
            marks.push(mark.clone());
            let (door_type, material, frame) = if exterior {
                ("D", "AL", "AL")
            } else {
                match name {
                    "MECH" | "STAIR" => ("A", "HM", "HM"),
                    "OFFICE" | "CONFERENCE" | "TRAINING" => (if st.office_lite { "B" } else { "C" }, "WD", "HM"),
                    _ => ("A", "WD", "HM"),
                }
            };
            let fire_rating = if exterior {
                None
            } else if name == "STAIR" {
                Some("60 MIN".to_string())
            } else {
                match rating.as_deref() {
                    Some(x) if x.starts_with('2') => Some("90 MIN".to_string()),
                    Some(_) => Some("20 MIN".to_string()),
                    None => None,
                }
            };
            let hardware = if exterior {
                use_hw(5)
            } else {
                match name {
                    "MECH" if st.mech_card => use_hw(7),
                    "MECH" | "STORAGE" => use_hw(3),
                    "STAIR" => use_hw(6),
                    "OFFICE" | "FILES" => use_hw(2),
                    "MEN" | "WOMEN" | "TOILET" | "RESTROOM" => use_hw(4),
                    _ => use_hw(1),
                }
            };
            arch.doors.push(Door {
                mark,
                level: wall.level.clone(),
                wall: wall.id.clone(),
                at_ft: {
                    let p = at(wall, u, n, mid, 0.0);
                    [r6(p[0]), r6(p[1])]
                },
                hinge_ft: [r6(hinge[0]), r6(hinge[1])],
                latch_ft: [r6(latch[0]), r6(latch[1])],
                width_in: ((o[1] - o[0]) * 12.0).round(),
                height_in: st.door_h_in,
                door_type: door_type.into(),
                material: material.into(),
                frame: frame.into(),
                fire_rating,
                hardware: st.hw_group(hardware),
                swings_into: into_room.map(|r| r.number.clone()),
                from_room: other.map(|r| r.number.clone()),
                hand: hd,
            });
        }
    }
    hw_used.sort_by_key(|h| h.0);
    arch.hardware = hw_used
        .into_iter()
        .map(|(n, items, card)| HardwareGroup { group: st.hw_group(n), items: items.into_iter().map(String::from).collect(), card_reader: card })
        .collect();

    // ---- Windows: one per bay in the long exterior walls, clear of tags and the core ----
    let [ww, wh, sill] = st.window;
    let mut wtypes = vec![WindowType {
        mark: "1".into(),
        description: "ALUMINUM FIXED WINDOW".into(),
        width_in: ww,
        height_in: wh,
        sill_in: sill,
        frame: "ALUM.".into(),
        glazing: "1\" INSUL.".into(),
        storefront: false,
    }];
    if st.storefront {
        wtypes.push(WindowType {
            mark: "2".into(),
            description: "ALUMINUM STOREFRONT".into(),
            width_in: 96.0,
            height_in: 96.0,
            sill_in: 0.0,
            frame: "ALUM.".into(),
            glazing: "1\" INSUL. TEMP.".into(),
            storefront: true,
        });
    }
    let external: Vec<String> = t.wall_types.iter().filter(|w| w.external).map(|w| w.tag.clone()).collect();
    for wall in walls.iter().filter(|wl| external.contains(&wl.wall_type)) {
        let Some((u, n, len)) = frame_of(wall) else { continue };
        if u[1].abs() > 1e-6 {
            continue; // the end walls stay solid
        }
        let south = wall.a_ft[1].abs() < 1.0;
        let wt = if south && wall.level == "1" && st.storefront { wtypes.get(1) } else { wtypes.first() };
        let Some(wt) = wt else { continue };
        let half = wt.width_in / 24.0;
        for i in 0..xs.len().saturating_sub(1) {
            let c = (x(i) + x(i + 1)) / 2.0;
            let s = (c - wall.a_ft[0]) * u[0];
            if s - half < 2.0 || s + half > len - 2.0 {
                continue;
            }
            // Not the mechanical room or the core; tags and openings keep clear.
            let p_in = at(wall, u, n, s, if south { 2.0 } else { -2.0 });
            let p_in = [p_in[0], if south { 2.0 } else { y3 - 2.0 }];
            let room = rooms.iter().find(|rm| rm.level == wall.level && inside(rm, p_in));
            if room.is_some_and(|rm| rm.name == "MECH" || rm.name == "STAIR") {
                continue;
            }
            if wall.tags.iter().any(|tg| (tg.s_ft - s).abs() < half + 1.75 || (tg.s_ft + tg.shift_ft - s).abs() < half + 1.75) {
                continue;
            }
            if wall.openings.iter().any(|o| o[1] > s - half - 1.0 && o[0] < s + half + 1.0) {
                continue;
            }
            let p = at(wall, u, n, s, 0.0);
            arch.windows.push(Window {
                mark: wt.mark.clone(),
                level: wall.level.clone(),
                wall: wall.id.clone(),
                at_ft: [r6(p[0]), r6(p[1])],
                along_ft: [r6(s - half), r6(s + half)],
            });
        }
    }
    wtypes.retain(|wt| arch.windows.iter().any(|w| w.mark == wt.mark));
    arch.window_types = wtypes;

    // ---- Slab edges and the roof ----
    let b = st.slab_beyond_in / 12.0;
    let (lo, hi) = ([-te / 2.0 - b, -te / 2.0 - b], [w + te / 2.0 + b, y3 + te / 2.0 + b]);
    for lv in ["1", "2", "R"] {
        arch.slab_edges.push(SlabEdge {
            level: lv.into(),
            min_ft: [r6(lo[0]), r6(lo[1])],
            max_ft: [r6(hi[0]), r6(hi[1])],
            beyond_face_in: st.slab_beyond_in,
        });
    }
    let drains: Vec<[f64; 2]> = (0..st.drains).map(|i| [r6(w * (i as f64 + 1.0) / (st.drains as f64 + 1.0)), r6(y3 / 2.0)]).collect();
    let mut curbs = Vec::new();
    for e in t.equipment.iter().filter(|e| e.level == "R") {
        curbs.push(Curb {
            tag: e.tag.clone(),
            kind: "equipment".into(),
            min_ft: [r6(e.min_ft[0] - 0.5), r6(e.min_ft[1] - 0.5)],
            max_ft: [r6(e.max_ft[0] + 0.5), r6(e.max_ft[1] + 0.5)],
            height_in: 14.0,
        });
    }
    if let Some(s) = arch.stairs.first() {
        curbs.push(Curb {
            tag: "ROOF HATCH".into(),
            kind: "hatch".into(),
            min_ft: [r6(s.max_ft[0] - 3.5), r6(s.max_ft[1] - 3.0)],
            max_ft: [r6(s.max_ft[0] - 0.5), r6(s.max_ft[1] - 0.5)],
            height_in: 12.0,
        });
    }
    if let Some(e) = arch.elevators.first() {
        curbs.push(Curb {
            tag: "ELEV. OVERRUN".into(),
            kind: "overrun".into(),
            min_ft: [r6(e.min_ft[0] - 0.75), r6(e.min_ft[1] - 0.75)],
            max_ft: [r6(e.max_ft[0] + 0.75), r6(e.max_ft[1] + 0.75)],
            height_in: 120.0,
        });
    }
    arch.roof = Roof {
        min_ft: arch.slab_edges.last().map_or(lo, |s| s.min_ft),
        max_ft: arch.slab_edges.last().map_or(hi, |s| s.max_ft),
        parapet_in: st.parapet_in,
        slope_in_ft: st.slope,
        drains,
        curbs,
    };

    // ---- Room finishes ----
    let mut codes: Vec<FinishCode> = Vec::new();
    let mut code = |kind: &str, prefix: &str, n: u32, desc: String, plates: Option<&str>| {
        let c = st.code(prefix, n);
        if !codes.iter().any(|x| x.code == c) {
            codes.push(FinishCode { code: c.clone(), kind: kind.into(), description: desc, plates: plates.map(String::from) });
        }
        c
    };
    let [c1, c2, c3] = st.colours;
    let mut rooms_sorted = t.building.rooms.clone();
    rooms_sorted.sort_by(|a, b| a.level.cmp(&b.level).then(a.number.cmp(&b.number)));
    for rm in &rooms_sorted {
        let wet = matches!(rm.name.as_str(), "MEN" | "WOMEN" | "TOILET" | "RESTROOM");
        let (floor, base) = match rm.name.as_str() {
            _ if wet => {
                (code("floor", "CT", 1, "PORCELAIN FLOOR TILE, 12x24".into(), None), code("base", "CTB", 1, "PORCELAIN COVE BASE".into(), None))
            }
            "MECH" => (code("floor", "SC", 1, "SEALED CONCRETE".into(), None), code("base", "RB", 1, "RUBBER BASE, 4\"".into(), None)),
            "STAIR" => (code("floor", "RT", 1, "RUBBER TREADS AND RISERS".into(), None), code("base", "RB", 1, "RUBBER BASE, 4\"".into(), None)),
            "CORRIDOR" | "BREAK ROOM" | "WORK ROOM" | "COPY" => {
                (code("floor", "LVT", 1, "LUXURY VINYL TILE".into(), None), code("base", "RB", 1, "RUBBER BASE, 4\"".into(), None))
            }
            "STORAGE" | "FILES" => {
                (code("floor", "VCT", 1, "VINYL COMPOSITION TILE".into(), None), code("base", "RB", 1, "RUBBER BASE, 4\"".into(), None))
            }
            _ => (code("floor", "CPT", 1, "CARPET TILE".into(), None), code("base", "RB", 1, "RUBBER BASE, 4\"".into(), None)),
        };
        let wall = match rm.name.as_str() {
            _ if wet => code("wall", "CT", 2, "PORCELAIN WALL TILE TO 7'-0\", PAINT ABOVE".into(), Some("STAINLESS")),
            "MECH" | "STAIR" => code("wall", "PT", 2, format!("EPOXY PAINT, SEMI-GLOSS, COLOR: {c2}"), Some("STAINLESS")),
            "CONFERENCE" | "TRAINING" => code("wall", "PT", 3, format!("PAINT, EGGSHELL, ACCENT COLOR: {c3}"), Some("GRAY")),
            _ => code("wall", "PT", 1, format!("PAINT, EGGSHELL, COLOR: {c1}"), Some("WHITE")),
        };
        let ceiling = match rm.ceiling.as_ref() {
            Some(c) if c.kind == "ACT" => {
                let g = c.grid_ft.unwrap_or([2.0, 4.0]);
                if g[1] > 3.0 {
                    code("ceiling", "ACT", 1, "ACOUSTIC CEILING TILE, 2x4, SUSPENDED GRID".into(), None)
                } else {
                    code("ceiling", "ACT", 2, "ACOUSTIC CEILING TILE, 2x2, SUSPENDED GRID".into(), None)
                }
            }
            Some(_) => code("ceiling", "GYP", 1, "PAINTED GYPSUM BOARD".into(), None),
            None => code("ceiling", "EXP", 1, "EXPOSED STRUCTURE, PAINTED".into(), None),
        };
        arch.finishes.push(RoomFinish {
            level: rm.level.clone(),
            number: rm.number.clone(),
            name: rm.name.clone(),
            floor,
            base,
            wall,
            ceiling,
            ceiling_ft: rm.ceiling.as_ref().map(|c| c.height_ft),
        });
    }
    let order = |k: &str| ["floor", "base", "wall", "ceiling"].iter().position(|x| *x == k).unwrap_or(4);
    codes.sort_by(|a, b| order(&a.kind).cmp(&order(&b.kind)).then(a.code.cmp(&b.code)));
    arch.finish_codes = codes;
    t.arch = Some(arch);
}

// ---- Drawing -----------------------------------------------------------------------------------

/// `3'-0"`.
fn ft_in(inches: f64) -> String {
    feet_inches(inches / 12.0)
}

/// What a floor plan adds to the walls: door tags, windows and their tags, the stair, the
/// elevator, the slab edge.
pub fn draw_floor(pen: &mut Pen, t: &Truth, st: &ArchStyle, level: &str, scale: f64) {
    let Some(arch) = t.arch.as_ref() else { return };
    let rot = crate::walls::readable(pen.rot());
    let r_ft = 0.13 / scale.max(1e-6);
    for d in arch.doors.iter().filter(|d| d.level == level) {
        let Some(wall) = t.plan_walls.iter().find(|w| w.id == d.wall) else { continue };
        let Some((u, n, _)) = frame_of(wall) else { continue };
        let th = wall_th(t, wall);
        // On the side it doesn't swing to, clear of the wall.
        let sw = [d.hinge_ft[0] - d.at_ft[0], d.hinge_ft[1] - d.at_ft[1]];
        let into = if sw[0] * n[0] + sw[1] * n[1] >= 0.0 { 1.0 } else { -1.0 };
        let off = th / 2.0 + r_ft + 0.6;
        let c = [d.at_ft[0] - n[0] * into * off + u[0] * 0.0, d.at_ft[1] - n[1] * into * off];
        let pc = pen.b(c);
        let r = pen.bl(r_ft);
        pen.circle("A-DOOR-IDEN", pc, r);
        pen.text("A-DOOR-IDEN", pc, 0.07, &d.mark, rot, HAlign::Center, VAlign::Middle);
    }
    for wi in arch.windows.iter().filter(|w| w.level == level) {
        let Some(wall) = t.plan_walls.iter().find(|w| w.id == wi.wall) else { continue };
        let Some((u, n, _)) = frame_of(wall) else { continue };
        let th = wall_th(t, wall);
        let [s0, s1] = wi.along_ft;
        let pts = [at(wall, u, n, s0, -th / 2.0), at(wall, u, n, s1, -th / 2.0), at(wall, u, n, s1, th / 2.0), at(wall, u, n, s0, th / 2.0)];
        let pv: Vec<Vec2> = pts.iter().map(|p| pen.b(*p)).collect();
        pen.poly("A-GLAZ", &pv, true);
        let (a, b) = (pen.b(at(wall, u, n, s0, 0.0)), pen.b(at(wall, u, n, s1, 0.0)));
        pen.line("A-GLAZ", a, b);
        // The tag outside: exterior walls have the building on their left (+n).
        let c = at(wall, u, n, (s0 + s1) / 2.0, -(th / 2.0 + r_ft * 1.6 + 1.2));
        let pc = pen.b(c);
        let r = pen.bl(r_ft * 1.1);
        tag_symbol(pen, "A-GLAZ-IDEN", pc, r, st.window_symbol);
        pen.text("A-GLAZ-IDEN", pc, 0.08, &wi.mark, rot, HAlign::Center, VAlign::Middle);
    }
    for s in arch.stairs.iter().filter(|s| s.levels.iter().any(|l| l == level)) {
        let wf = s.width_in / 12.0;
        let tr = s.tread_in / 12.0;
        let n1 = s.risers.div_ceil(2);
        let n2 = s.risers - n1;
        let land = s.max_ft[1] - wf;
        let flights = [(s.max_ft[0] - wf, s.max_ft[0], n1), (s.min_ft[0], s.min_ft[0] + wf, n2)];
        for (x0, x1, nr) in flights {
            let y0 = land - nr.saturating_sub(1) as f64 * tr;
            pen.brect("A-FLOR-STRS", [x0, y0], [x1, land]);
            for k in 1..nr.saturating_sub(1) {
                let y = y0 + k as f64 * tr;
                let (a, b) = (pen.b([x0, y]), pen.b([x1, y]));
                pen.line("A-FLOR-STRS", a, b);
            }
        }
        pen.brect("A-FLOR-STRS", [s.min_ft[0], land], [s.max_ft[0], s.max_ft[1]]);
        // Walking line up the first flight.
        let xm = s.max_ft[0] - wf / 2.0;
        let y0 = land - n1.saturating_sub(1) as f64 * tr;
        let (a, b) = (pen.b([xm, y0 + 0.5]), pen.b([xm, land - 0.5]));
        pen.line("A-FLOR-STRS", a, b);
        let first = s.levels.first().is_some_and(|l| l == level);
        let words = format!("{} {}R @ {}", if first { "UP" } else { "DN" }, s.risers, crate::walls::frac(s.riser_in, true));
        let p = pen.b([(s.min_ft[0] + s.max_ft[0]) / 2.0, y0 - 1.2]);
        pen.text("A-FLOR-STRS", p, 0.07, &words, rot, HAlign::Center, VAlign::Middle);
        let p = pen.b([(s.min_ft[0] + s.max_ft[0]) / 2.0, s.max_ft[1] - wf / 2.0]);
        pen.text("A-FLOR-STRS", p, 0.08, &s.tag, rot, HAlign::Center, VAlign::Middle);
    }
    for e in arch.elevators.iter().filter(|e| e.levels.iter().any(|l| l == level)) {
        pen.brect("A-ELEV", e.min_ft, e.max_ft);
        pen.brect("A-ELEV", [e.min_ft[0] - 0.67, e.min_ft[1] - 0.67], [e.max_ft[0] + 0.67, e.max_ft[1] + 0.67]);
        let (a, b) = (pen.b(e.min_ft), pen.b(e.max_ft));
        pen.line("A-ELEV", a, b);
        let (a, b) = (pen.b([e.min_ft[0], e.max_ft[1]]), pen.b([e.max_ft[0], e.min_ft[1]]));
        pen.line("A-ELEV", a, b);
        let c = pen.b([(e.min_ft[0] + e.max_ft[0]) / 2.0, e.min_ft[1] - 1.6]);
        let mut lines = vec![e.tag.clone(), format!("{} LB", e.capacity_lb)];
        if e.levels.first().is_some_and(|l| l == level) {
            lines.push(format!("PIT {}", feet_inches(e.pit_ft)));
        }
        pen.block("A-ELEV", c, 0.07, &lines, rot);
    }
    for se in arch.slab_edges.iter().filter(|s| s.level == level) {
        pen.brect("A-SLAB-EDGE", se.min_ft, se.max_ft);
        let words = if st.edge_words { "EDGE OF SLAB" } else { "SLAB EDGE" };
        let p = pen.b([se.min_ft[0] + 6.0, se.min_ft[1] - 1.0]);
        pen.text("A-SLAB-EDGE", p, 0.07, words, rot, HAlign::Left, VAlign::Middle);
    }
}

/// The roof plan: edge, parapet, slope, drains, curbs, hatch and overrun.
pub fn draw_roof(pen: &mut Pen, t: &Truth, st: &ArchStyle) {
    let Some(arch) = t.arch.as_ref() else { return };
    let rot = crate::walls::readable(pen.rot());
    let rf = &arch.roof;
    pen.brect("A-ROOF", rf.min_ft, rf.max_ft);
    if let Some(p) = rf.parapet_in {
        pen.brect("A-ROOF", [rf.min_ft[0] + 0.67, rf.min_ft[1] + 0.67], [rf.max_ft[0] - 0.67, rf.max_ft[1] - 0.67]);
        let at = pen.b([rf.min_ft[0] + 2.0, rf.max_ft[1] - 2.0]);
        pen.text("A-ROOF", at, 0.08, &format!("PARAPET, {}\" ABOVE ROOF", p.round()), rot, HAlign::Left, VAlign::Middle);
    }
    let slope = format!("SLOPE {}/FT", crate::walls::frac(rf.slope_in_ft, true));
    for d in &rf.drains {
        let c = pen.b(*d);
        let r = pen.bl(0.75);
        pen.circle("A-ROOF-DRAN", c, r);
        let p = pen.b([d[0] + 1.2, d[1] - 1.2]);
        pen.text("A-ROOF-DRAN", p, 0.07, "RD", rot, HAlign::Left, VAlign::Middle);
        // Slope arrows from the low edges' middle towards the drain.
        for dy in [-1.0, 1.0] {
            let tail = [d[0], d[1] + dy * (rf.max_ft[1] - rf.min_ft[1]) * 0.3];
            let (a, b) = (pen.b(tail), pen.b([d[0], d[1] + dy * 1.5]));
            pen.line("A-ROOF", a, b);
            let p = pen.b([d[0] + 0.8, tail[1] - dy * 2.5]);
            pen.text("A-ROOF", p, 0.06, &slope, rot, HAlign::Left, VAlign::Middle);
        }
    }
    for c in &rf.curbs {
        pen.brect("A-ROOF-CURB", c.min_ft, c.max_ft);
        let mid = [(c.min_ft[0] + c.max_ft[0]) / 2.0, (c.min_ft[1] + c.max_ft[1]) / 2.0];
        let lines = match c.kind.as_str() {
            "equipment" => vec![c.tag.clone(), format!("ON {}\" CURB", c.height_in.round())],
            "hatch" => vec![c.tag.clone(), format!("{}\" CURB", c.height_in.round())],
            _ => vec![c.tag.clone(), format!("+{} ABOVE ROOF", feet_inches(c.height_in / 12.0))],
        };
        let p = pen.b([mid[0], c.min_ft[1] - 1.4]);
        pen.block("A-ROOF-CURB", p, 0.065, &lines, rot);
    }
    let _ = st;
}

/// The door, hardware and window schedules.
pub fn draw_door_schedule(pen: &mut Pen, t: &Truth, st: &ArchStyle, area: [[f64; 2]; 2]) {
    let Some(arch) = t.arch.as_ref() else { return };
    let [[x0, y0], [_, y1]] = area;
    let long = st.headers == 1;
    let rating_h = if st.rating_min { if long { "FIRE RATING" } else { "RATING" } } else { "LABEL (MIN)" };
    let mut cols: Vec<&str> = vec![
        if long { "DOOR NO." } else { "MARK" },
        if long { "WIDTH" } else { "WD" },
        if long { "HEIGHT" } else { "HT" },
        if long { "DOOR TYPE" } else { "TYPE" },
        if long { "MATERIAL" } else { "MATL" },
    ];
    if st.rating_first {
        cols.push(rating_h);
        cols.push("FRAME");
    } else {
        cols.push("FRAME");
        cols.push(rating_h);
    }
    cols.push(if long { "HARDWARE" } else { "HW SET" });
    let mut rows = vec![cols.iter().map(|s| s.to_string()).collect::<Vec<_>>()];
    let mut doors: Vec<&Door> = arch.doors.iter().collect();
    doors.sort_by(|a, b| a.mark.cmp(&b.mark));
    for d in doors {
        let rating = match &d.fire_rating {
            Some(r) if st.rating_min => r.clone(),
            Some(r) => r.trim_end_matches(" MIN").to_string(),
            None => "--".into(),
        };
        let mut row = vec![d.mark.clone(), ft_in(d.width_in), ft_in(d.height_in), d.door_type.clone(), d.material.clone()];
        if st.rating_first {
            row.push(rating);
            row.push(d.frame.clone());
        } else {
            row.push(d.frame.clone());
            row.push(rating);
        }
        row.push(d.hardware.clone());
        rows.push(row);
    }
    let x = x0 + 0.3;
    let h = table(pen, [x, y1 - 0.6], "DOOR SCHEDULE", &rows);
    let mut y = y1 - 0.6 - h - 0.5;
    let mut hw_rows = vec![vec![if st.hw == 1 { "SET".to_string() } else { "GROUP".to_string() }, "HARDWARE".to_string()]];
    for g in &arch.hardware {
        hw_rows.push(vec![g.group.clone(), g.items.join(", ")]);
    }
    let x2 = x + 11.5;
    let hh = table(pen, [x2, y1 - 0.6], "HARDWARE GROUPS", &hw_rows);
    let mut win_rows = vec![["MARK", "DESCRIPTION", "WIDTH", "HEIGHT", "SILL", "FRAME", "GLAZING"].map(String::from).to_vec()];
    for w in &arch.window_types {
        win_rows.push(vec![
            w.mark.clone(),
            w.description.clone(),
            ft_in(w.width_in),
            ft_in(w.height_in),
            ft_in(w.sill_in),
            w.frame.clone(),
            w.glazing.clone(),
        ]);
    }
    let wy = y1 - 0.6 - hh - 0.6;
    if wy - (win_rows.len() as f64) * 0.24 > y0 {
        table(pen, [x2, wy], "WINDOW SCHEDULE", &win_rows);
    } else {
        y = y.max(y0 + 2.0);
        table(pen, [x, y], "WINDOW SCHEDULE", &win_rows);
    }
    let notes = [
        "DOOR TYPES: A = FLUSH, B = HALF LITE, C = NARROW LITE, D = FULL GLASS.",
        "MATERIALS: WD = WOOD, HM = HOLLOW METAL, AL = ALUMINUM.",
        "RATED DOORS: LABELED DOOR AND FRAME, CLOSER AND POSITIVE LATCHING.",
    ];
    for (i, s) in notes.iter().enumerate() {
        pen.ptext("TEXT", [x2, y0 + 0.8 + i as f64 * 0.22], 0.08, s, HAlign::Left);
    }
}

/// The room finish schedule and the finish legend.
pub fn draw_finish_schedule(pen: &mut Pen, t: &Truth, st: &ArchStyle, area: [[f64; 2]; 2]) {
    let Some(arch) = t.arch.as_ref() else { return };
    let [[x0, _], [_, y1]] = area;
    let cols: [&str; 7] = match st.finish_headers {
        1 => ["NO.", "ROOM", "FLOOR", "BASE", "WALLS", "CLG.", "CLG. HT."],
        2 => ["RM #", "ROOM NAME", "FLR", "BASE", "WALL", "CEILING", "HEIGHT"],
        _ => ["ROOM NO.", "ROOM NAME", "FLOOR", "BASE", "WALLS", "CEILING", "CLG. HT."],
    };
    let mut rows = vec![cols.map(String::from).to_vec()];
    for f in &arch.finishes {
        rows.push(vec![
            f.number.clone(),
            f.name.clone(),
            f.floor.clone(),
            f.base.clone(),
            f.wall.clone(),
            f.ceiling.clone(),
            f.ceiling_ft.map_or_else(|| "--".to_string(), feet_inches),
        ]);
    }
    let x = x0 + 0.3;
    table(pen, [x, y1 - 0.6], "ROOM FINISHES", &rows);
    let mut legend = if st.plate_column {
        vec![vec!["CODE".to_string(), "DESCRIPTION".to_string(), "DEVICE PLATES".to_string()]]
    } else {
        vec![vec!["CODE".to_string(), "DESCRIPTION".to_string()]]
    };
    for c in &arch.finish_codes {
        if st.plate_column {
            legend.push(vec![c.code.clone(), c.description.clone(), c.plates.clone().unwrap_or_default()]);
        } else {
            let d = match &c.plates {
                Some(p) => format!("{}; DEVICE PLATES: {p}", c.description),
                None => c.description.clone(),
            };
            legend.push(vec![c.code.clone(), d]);
        }
    }
    table(pen, [x + 12.5, y1 - 0.6], "FINISH LEGEND", &legend);
}
