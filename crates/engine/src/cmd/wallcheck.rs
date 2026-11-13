//! Walls the build reads off a plan that can't all be walls (Q6583).
//!
//! A plan draws counters, equipment, service windows and repeated linework beside its walls, and
//! read as face pairs they become walls: a small one-room-deep building came out with several
//! times the walls it can hold, and its board, tape and paint priced on them. Three rules:
//! 1. while the plan is read (`wallgen`), two readings of one wall whose bodies overlap are one
//!    wall, a pair walled in on both sides is the air between two walls, and a pair inside a
//!    closed outline deeper than any wall and no deeper than casework
//!    ([`Rules::casework_depth_max_in`]) is the thing drawn there;
//! 2. after the build, a storey whose walls total more than it can hold (its outline's
//!    perimeter, plus its floor cut into the smallest rooms the rules allow,
//!    [`overbuilt_levels`]) is a high finding and an always-asked question before the takeoff;
//! 3. a level's wall board over its walls' area times their faces ([`board_over`]) is a suspect
//!    line of the takeoff;
//! 4. a storey whose rooms cover far less than its floor ([`under_roomed_levels`]: rooms built to
//!    their casework, or rooms left out) is a finding (Q6487).
//!
//! The numbers are rule data: `wallcheck.json`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

const RULES: &str = include_str!("wallcheck.json");

/// The question for a storey with more walls than it can hold (always asked).
pub const OVERBUILT_QUESTION: &str = "More walls than the storey can hold: level";
/// The answer that keeps the walls as built.
pub const KEEP: &str = "Keep them: the walls are as drawn";
/// The answer that says they need reading again.
pub const RECHECK: &str = "Some are casework or equipment: check the plan's walls (wallgen.plan)";

/// The rule data (`wallcheck.json`).
#[derive(Clone, Debug, Deserialize)]
pub struct Rules {
    /// A closed outline deeper than any wall and at most this deep (inches) across its narrowest
    /// side is casework or equipment (a counter, a cabinet run, an appliance): what pairs up
    /// inside it is no wall.
    pub casework_depth_max_in: f64,
    /// The smallest side of a room (feet): a floor cut all into such rooms holds the most walls a
    /// storey can.
    pub min_room_side_ft: f64,
    /// A storey's walls over what it can hold by more than this share are reported (0: any).
    pub max_over_ratio: f64,
    /// Faces of a wall that take board.
    pub wall_faces: f64,
    /// A level's wall board over its walls' area times their faces by more than this ratio is
    /// suspect.
    pub board_over_ratio: f64,
    /// A storey with fewer feet of walls is not weighed (a penthouse, a canopy: too little to say).
    pub min_level_walls_ft: f64,
    /// A storey whose built rooms cover less than this share of its floor is reported (a
    /// building's rooms are most of its floor; walls, shafts and chases the rest).
    pub min_room_share: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            casework_depth_max_in: 48.0,
            min_room_side_ft: 5.0,
            max_over_ratio: 0.0,
            wall_faces: 2.0,
            board_over_ratio: 1.25,
            min_level_walls_ft: 40.0,
            min_room_share: 0.5,
        }
    }
}

pub fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// A storey with more walls than it can hold.
#[derive(Clone, Debug, PartialEq)]
pub struct Overbuilt {
    pub level: String,
    /// Its walls' length, feet.
    pub walls_ft: f64,
    /// The most walls it can hold, feet.
    pub can_hold_ft: f64,
    /// Its outline's perimeter and its floor area (feet, square feet).
    pub perimeter_ft: f64,
    pub area_sq_ft: f64,
}

/// A plan box `[x0, y0, x1, y1]` (drawing units).
pub type PlanBox = [f64; 4];

fn finite_pos(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

fn grow(b: &mut PlanBox, o: &PlanBox) {
    *b = [b[0].min(o[0].min(o[2])), b[1].min(o[1].min(o[3])), b[2].max(o[0].max(o[2])), b[3].max(o[1].max(o[3]))];
}

const EMPTY: PlanBox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];

/// The storeys whose walls total more than they can hold: their outline's perimeter (once round
/// the outside) plus their floor area cut into rooms [`Rules::min_room_side_ft`] square (each
/// such room adds two of its sides, the others its neighbours'), over by more than
/// [`Rules::max_over_ratio`]. `walls`: each built wall's level, length (drawing units) and plan
/// box; `floors`: each built floor's level and plan box (a storey without floors is measured on
/// its walls' extents). `upf` drawing units a foot.
pub fn overbuilt_levels(walls: &[(String, f64, PlanBox)], floors: &[(String, PlanBox)], upf: f64) -> Vec<Overbuilt> {
    let upf = if finite_pos(upf) { upf } else { 12.0 };
    let r = rules();
    let side = if finite_pos(r.min_room_side_ft) { r.min_room_side_ft } else { 5.0 };
    let mut by: BTreeMap<String, (f64, PlanBox)> = BTreeMap::new();
    for (level, len, b) in walls {
        if level.is_empty() || !finite_pos(*len) || !b.iter().all(|v| v.is_finite()) {
            continue;
        }
        let e = by.entry(level.clone()).or_insert((0.0, EMPTY));
        e.0 += len / upf;
        grow(&mut e.1, b);
    }
    // Each storey's floors: their area (each floor's box) and their extents.
    let mut fl: BTreeMap<String, (f64, PlanBox)> = BTreeMap::new();
    for (level, b) in floors {
        if !b.iter().all(|v| v.is_finite()) {
            continue;
        }
        let a = (b[2] - b[0]).abs() * (b[3] - b[1]).abs() / (upf * upf);
        let e = fl.entry(level.clone()).or_insert((0.0, EMPTY));
        e.0 += a;
        grow(&mut e.1, b);
    }
    let mut out = Vec::new();
    for (level, (walls_ft, wb)) in by {
        if walls_ft < r.min_level_walls_ft {
            continue;
        }
        let (area, b) = match fl.get(&level) {
            Some((a, b)) if *a > 0.0 && b[2] > b[0] && b[3] > b[1] => {
                // Floors laid over each other count once: no more than their extents.
                let whole = (b[2] - b[0]) * (b[3] - b[1]) / (upf * upf);
                (a.min(whole), *b)
            }
            _ => ((wb[2] - wb[0]).abs() * (wb[3] - wb[1]).abs() / (upf * upf), wb),
        };
        let perimeter = 2.0 * ((b[2] - b[0]).abs() + (b[3] - b[1]).abs()) / upf;
        if !(finite_pos(area) && finite_pos(perimeter)) {
            continue;
        }
        let can_hold = perimeter + 2.0 * area / side;
        if walls_ft > can_hold * (1.0 + r.max_over_ratio.max(0.0)) {
            out.push(Overbuilt { level, walls_ft, can_hold_ft: can_hold, perimeter_ft: perimeter, area_sq_ft: area });
        }
    }
    out
}

/// A storey whose rooms cover far less than its floor.
#[derive(Clone, Debug, PartialEq)]
pub struct UnderRoomed {
    pub level: String,
    /// Its rooms' floor area and its floor's, square feet.
    pub rooms_sq_ft: f64,
    pub floor_sq_ft: f64,
}

/// The storeys with rooms whose rooms cover less than [`Rules::min_room_share`] of their floor:
/// `rooms` each built room's level and floor area (square feet), `floors` each built floor's level
/// and plan box (laid over each other they count once: no more than their extents). A storey with
/// no floor, or none of its rooms built, isn't weighed here. `upf` drawing units a foot.
pub fn under_roomed_levels(rooms: &[(String, f64)], floors: &[(String, PlanBox)], upf: f64) -> Vec<UnderRoomed> {
    let upf = if finite_pos(upf) { upf } else { 12.0 };
    let share = rules().min_room_share;
    if !finite_pos(share) {
        return Vec::new();
    }
    let mut by: BTreeMap<String, f64> = BTreeMap::new();
    for (level, a) in rooms {
        if !level.is_empty() && finite_pos(*a) {
            *by.entry(level.clone()).or_default() += a;
        }
    }
    let mut fl: BTreeMap<String, (f64, PlanBox)> = BTreeMap::new();
    for (level, b) in floors {
        if !b.iter().all(|v| v.is_finite()) {
            continue;
        }
        let e = fl.entry(level.clone()).or_insert((0.0, EMPTY));
        e.0 += (b[2] - b[0]).abs() * (b[3] - b[1]).abs() / (upf * upf);
        grow(&mut e.1, b);
    }
    let mut out = Vec::new();
    for (level, rooms_sq_ft) in by {
        let Some((a, b)) = fl.get(&level) else { continue };
        let whole = (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0) / (upf * upf);
        let floor_sq_ft = a.min(whole);
        if finite_pos(floor_sq_ft) && rooms_sq_ft < share * floor_sq_ft {
            out.push(UnderRoomed { level, rooms_sq_ft, floor_sq_ft });
        }
    }
    out
}

/// Whether a level's wall board (square feet) is over its walls' area (square feet) times their
/// faces by more than the rules allow.
pub fn board_over(board_sq_ft: f64, wall_sq_ft: f64) -> bool {
    let r = rules();
    finite_pos(board_sq_ft) && finite_pos(wall_sq_ft) && board_sq_ft > wall_sq_ft * r.wall_faces.max(1.0) * r.board_over_ratio.max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_load() {
        let r = rules();
        assert!(r.casework_depth_max_in > 20.0 && r.min_room_side_ft > 0.0 && r.wall_faces >= 1.0 && r.board_over_ratio >= 1.0);
    }

    /// A rectangle of walls `w` × `h` feet, with `extra` feet of walls inside (inches, 12 a foot).
    fn storey(level: &str, w: f64, h: f64, extra: f64) -> Vec<(String, f64, PlanBox)> {
        let (w, h) = (w * 12.0, h * 12.0);
        let mut v = vec![
            (level.to_string(), w, [0.0, 0.0, w, 0.0]),
            (level.to_string(), w, [0.0, h, w, h]),
            (level.to_string(), h, [0.0, 0.0, 0.0, h]),
            (level.to_string(), h, [w, 0.0, w, h]),
        ];
        // The inside walls, each across the building.
        let mut left = extra * 12.0;
        let mut x = 6.0;
        while left > 0.0 {
            let l = left.min(h);
            v.push((level.to_string(), l, [x, 0.0, x, l]));
            left -= l;
            x = (x + 6.0).min(w - 1.0);
        }
        v
    }

    #[test]
    fn a_small_building_with_several_times_its_walls_is_overbuilt() {
        // 46' × 19': 130' round the outside, 874 sq ft; at most 130 + 2 × 874 / 5 ≈ 480' of walls
        // (a floor all in 5' rooms) — 580' is not a building's.
        let walls = storey("Level 1", 46.0, 19.0, 450.0);
        let over = overbuilt_levels(&walls, &[], 12.0);
        assert_eq!(over.len(), 1, "{over:?}");
        let o = &over[0];
        assert_eq!(o.level, "Level 1");
        assert!((o.walls_ft - 580.0).abs() < 1.0 && o.can_hold_ft < o.walls_ft, "{o:?}");
        // The same storey with a few rooms' walls is a building.
        assert!(overbuilt_levels(&storey("Level 1", 46.0, 19.0, 60.0), &[], 12.0).is_empty());
    }

    #[test]
    fn a_dense_storey_of_small_rooms_is_not() {
        // A long building of guest rooms: 230' × 66', walls every 13' both sides of a corridor,
        // demising and bathroom walls: about 3,000'.
        let walls = storey("Level 2", 230.0, 66.0, 2_400.0);
        assert!(overbuilt_levels(&walls, &[], 12.0).is_empty());
        // Its floors give its area: the same.
        let floors = vec![("Level 2".to_string(), [0.0, 0.0, 230.0 * 12.0, 66.0 * 12.0])];
        assert!(overbuilt_levels(&walls, &floors, 12.0).is_empty());
    }

    #[test]
    fn a_floor_smaller_than_the_walls_extents_holds_less() {
        // Walls strewn over 46' × 19' round a floor of 20' × 10': the floor is what holds them.
        let walls = storey("L1", 46.0, 19.0, 120.0);
        assert!(overbuilt_levels(&walls, &[], 12.0).is_empty());
        let floors = vec![("L1".to_string(), [0.0, 0.0, 240.0, 120.0])];
        assert_eq!(overbuilt_levels(&walls, &floors, 12.0).len(), 1);
    }

    #[test]
    fn a_few_walls_are_not_weighed() {
        let walls = vec![("Roof".to_string(), 30.0 * 12.0, [0.0, 0.0, 12.0, 0.0])];
        assert!(overbuilt_levels(&walls, &[], 12.0).is_empty());
    }

    #[test]
    fn hostile_numbers_are_skipped() {
        let walls = vec![
            ("L1".to_string(), f64::NAN, [0.0, 0.0, 1.0, 1.0]),
            ("L1".to_string(), 1e9, [f64::INFINITY, 0.0, 1.0, 1.0]),
            (String::new(), 100.0, [0.0, 0.0, 1.0, 1.0]),
        ];
        assert!(overbuilt_levels(&walls, &[("L1".into(), [f64::NAN; 4])], f64::NAN).is_empty());
    }

    #[test]
    fn rooms_far_short_of_their_floor_are_reported() {
        // A 45' x 19' floor (855 sq ft): rooms of 211 sq ft are rooms built to their counters;
        // 586 sq ft is the building's rooms.
        let floors = vec![("Level 1".to_string(), [0.0, 0.0, 45.0 * 12.0, 19.0 * 12.0])];
        let under = under_roomed_levels(&[("Level 1".into(), 166.0), ("Level 1".into(), 45.0)], &floors, 12.0);
        assert_eq!(under.len(), 1, "{under:?}");
        assert!((under[0].rooms_sq_ft - 211.0).abs() < 1e-9 && (under[0].floor_sq_ft - 855.0).abs() < 1e-6, "{under:?}");
        assert!(under_roomed_levels(&[("Level 1".into(), 524.0), ("Level 1".into(), 62.0)], &floors, 12.0).is_empty());
        // No floor, or no room: not weighed. Hostile numbers are skipped.
        assert!(under_roomed_levels(&[("Level 2".into(), 10.0)], &floors, 12.0).is_empty());
        assert!(under_roomed_levels(&[], &floors, 12.0).is_empty());
        assert!(under_roomed_levels(&[("Level 1".into(), f64::NAN)], &[("Level 1".into(), [f64::NAN; 4])], f64::NAN).is_empty());
    }

    #[test]
    fn board_over_its_walls_faces_is_suspect() {
        assert!(!board_over(11_000.0, 5_700.0));
        assert!(board_over(15_552.0, 5_700.0));
        assert!(!board_over(f64::NAN, 5_700.0) && !board_over(100.0, 0.0));
    }
}
