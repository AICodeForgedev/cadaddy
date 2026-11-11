//! Where the connection of a piece of ceiling equipment is built (Q6521): at the housing of the
//! equipment it serves, above the ceiling over it (`E-CONN-CEILING-EQUIP`, trade practice rules
//! data). It is worked out again on every build from the equipment and the ceiling as they stand
//! then, so it moves when either moves. A height the job fixed earlier (a fan's bottom when its
//! typical room was laid in) is the design value, kept beside the built one.

use cadaddy_geom::Vec2;
use cadaddy_walk3d::MepEquipment;
use cadaddy_walk3d::tags::ElementTag;
use serde_json::Value;

/// The trade practice rule the connection's place comes from.
pub(crate) const RULE: &str = "E-CONN-CEILING-EQUIP";
/// The rule's numbers when its data gives none: reach (feet), tolerance and clearance (inches).
const REACH_FT: f64 = 15.0;
const TOLERANCE_IN: f64 = 12.0;
const CLEAR_IN: f64 = 1.0;
/// Pieces of equipment looked at for one connection, at most.
const MAX_EQUIPMENT: usize = 20_000;
/// Names one connection is matched by, at most.
const MAX_NAMES: usize = 4;

/// The rule's numbers, from its data.
#[derive(Clone, Debug)]
pub(crate) struct Numbers {
    /// How far (feet) from where its plan puts the connection its equipment may be.
    pub reach_ft: f64,
    /// How far (inches) from its housing a connection may be and still be at it.
    pub tolerance_in: f64,
    /// How far (inches) above the ceiling's underside the box's bottom is at least.
    pub clear_in: f64,
    /// The rule's id and reason.
    pub basis: String,
}

impl Numbers {
    pub(crate) fn read() -> Self {
        let d = super::provenance::rule_data(RULE);
        let num = |k: &str, or: f64| d.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(or);
        Numbers {
            reach_ft: num("reach_ft", REACH_FT).min(200.0),
            tolerance_in: num("tolerance_in", TOLERANCE_IN).min(120.0),
            clear_in: num("clear_in", CLEAR_IN).min(24.0),
            basis: super::provenance::rule(RULE).basis_for(""),
        }
    }
}

/// A connection built at its equipment.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AtEquipment {
    /// The equipment's tag.
    pub tag: String,
    /// The box's middle in plan (drawing units) and the direction along its housing's side.
    pub centre: Vec2,
    pub along: Vec2,
    /// The box's bottom and top, inches over its level's floor.
    pub bottom_in: f64,
    pub top_in: f64,
    /// The underside of the ceiling over the equipment, inches over the floor, when one is known.
    pub ceiling_in: Option<f64>,
    /// How far the box is from the housing (inches; 0 when it touches it).
    pub gap_in: f64,
    /// Why it is there: the rule, the housing and the ceiling.
    pub basis: String,
}

impl AtEquipment {
    /// The box's middle, inches over the floor.
    pub(crate) fn middle_in(&self) -> f64 {
        (self.bottom_in + self.top_in) / 2.0
    }
}

/// What a connection tag names as its equipment: its `serves` (the tag before a `:`), its height
/// note's `connection to <TAG>`, else its type mark (`EF`).
pub(crate) fn named(t: &ElementTag) -> Vec<String> {
    let field = |k: &str| t.fields.get(k).map(|f| f.value.trim().to_string()).filter(|v| !v.is_empty());
    let mut out: Vec<String> = Vec::new();
    let mut push = |v: &str| {
        let v = v.trim().to_ascii_uppercase();
        if !v.is_empty() && v.len() <= 40 && !out.contains(&v) && out.len() < MAX_NAMES {
            out.push(v);
        }
    };
    if let Some(s) = field("serves") {
        push(s.split(':').next().unwrap_or_default());
    }
    if let Some(note) = field("mount_height_note")
        && let Some(rest) = note.strip_prefix("connection to ")
    {
        push(rest.split(|c: char| c.is_whitespace() || c == '(' || c == ';' || c == ',').next().unwrap_or_default());
    }
    if let Some(m) = field("type") {
        push(&m);
    }
    out
}

/// Whether `name` names the equipment tagged `tag`: the tag itself, or (a name with no number,
/// a fan's `EF`) any of its kind's numbered tags (`EF-2`, `EF2`), never another mark (`EFX`).
fn names(tag: &str, name: &str) -> bool {
    let (a, b) = (tag.trim().to_ascii_uppercase(), name.trim().to_ascii_uppercase());
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b
        || (!b.chars().any(|c| c.is_ascii_digit())
            && a.strip_prefix(&b).is_some_and(|r| r.starts_with('-') || r.starts_with(|c: char| c.is_ascii_digit())))
}

/// Whether two level names are one level (`3` is `Level 3`: a device read off its sheet may still
/// carry the sheet's level code).
fn same_level(a: &str, b: &str) -> bool {
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    a.trim().eq_ignore_ascii_case(b.trim()) || (!digits(a).is_empty() && digits(a) == digits(b))
}

/// An equipment's plan box (low and high corners), when it is a real one.
fn plan_box(e: &MepEquipment) -> Option<(Vec2, Vec2)> {
    let ok = e.min.iter().chain(&e.max).all(|v| v.is_finite());
    ok.then(|| (Vec2::new(e.min[0].min(e.max[0]), e.min[1].min(e.max[1])), Vec2::new(e.min[0].max(e.max[0]), e.min[1].max(e.max[1]))))
}

/// How far `p` is from a box (0 inside it).
fn gap(lo: Vec2, hi: Vec2, p: Vec2) -> f64 {
    Vec2::new(p.x.clamp(lo.x, hi.x), p.y.clamp(lo.y, hi.y)).dist(p)
}

/// The words a unit's mounting row is matched with: its title and its schedule's type and
/// mounting words.
fn mount_words(e: &MepEquipment) -> String {
    let said: Vec<&str> = e
        .properties
        .iter()
        .filter(|(k, _)| matches!(k.to_ascii_uppercase().as_str(), "TYPE" | "DESCRIPTION" | "MOUNTING" | "MOUNT" | "ARRANGEMENT" | "LOCATION"))
        .map(|(_, v)| v.as_str())
        .take(8)
        .collect();
    format!("{} {}", e.title, said.join(" "))
}

/// Where a connection (`at`: where it was read, `also`: where the build registers it, drawing
/// units) is built when it names a piece of ceiling equipment on its level within the rule's
/// reach: the nearest such one, the box on its housing's side nearest where the connection was
/// read (outside it, touching it), at the housing's middle height but never lower than
/// `clear_in` over the ceiling over the equipment (`ceiling_at`: the built ceiling's underside
/// over a plan point, inches over the floor; else the level's ceiling), and under the floor
/// above. `box_in`: the box's thickness, length and height. None when it names no such
/// equipment: it is built as before.
#[allow(clippy::too_many_arguments)]
pub(crate) fn place(
    t: &ElementTag,
    equipment: &[MepEquipment],
    level: &cadaddy_walk3d::Level,
    at: Vec2,
    also: Vec2,
    upf: f64,
    box_in: [f64; 3],
    ceiling_at: &dyn Fn(Vec2) -> Option<f64>,
    n: &Numbers,
) -> Option<AtEquipment> {
    if super::schedequip::is_roof(&level.name) {
        return None;
    }
    let wanted = named(t);
    if wanted.is_empty() || !(upf.is_finite() && upf > 0.0) {
        return None;
    }
    let reach = n.reach_ft * upf;
    let (e, lo, hi) = equipment
        .iter()
        .take(MAX_EQUIPMENT)
        .filter(|e| same_level(&e.level, &level.name) && wanted.iter().any(|w| names(&e.tag, w)))
        .filter(|e| e.bottom_ft.is_finite() && e.height_ft.is_finite() && e.height_ft > 0.0 && e.height_ft < 100.0)
        .filter_map(|e| plan_box(e).map(|(lo, hi)| (e, lo, hi)))
        .map(|(e, lo, hi)| (gap(lo, hi, at).min(gap(lo, hi, also)), e, lo, hi))
        .filter(|(g, ..)| g.is_finite() && *g <= reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, e, lo, hi)| (e, lo, hi))?;
    let mid = (lo + hi) * 0.5;
    let level_ceiling = (level.ceiling_ft.is_finite() && level.ceiling_ft > 6.0).then_some(level.ceiling_ft * 12.0);
    let ceiling = ceiling_at(mid).filter(|c| c.is_finite()).or(level_ceiling);
    let (bottom, top) = (e.bottom_ft * 12.0, (e.bottom_ft + e.height_ft) * 12.0);
    // Ceiling equipment: its mounting row hangs it above the ceiling, or its housing is there.
    let ceiling_kind = super::schedequip::hangs_above_ceiling(&e.kind, &mount_words(e)) || ceiling.is_some_and(|c| bottom >= c - 0.5);
    if !ceiling_kind {
        return None;
    }
    // On the housing's side nearest where it was read (drawn over it: the nearest side).
    let p = Vec2::new(at.x.clamp(lo.x, hi.x), at.y.clamp(lo.y, hi.y));
    let sides = [
        (p.x - lo.x, Vec2::new(lo.x, p.y), Vec2::new(-1.0, 0.0)),
        (hi.x - p.x, Vec2::new(hi.x, p.y), Vec2::new(1.0, 0.0)),
        (p.y - lo.y, Vec2::new(p.x, lo.y), Vec2::new(0.0, -1.0)),
        (hi.y - p.y, Vec2::new(p.x, hi.y), Vec2::new(0.0, 1.0)),
    ];
    let (_, on, out) = sides.iter().copied().min_by(|a, b| a.0.total_cmp(&b.0))?;
    let [thick, _, tall] = box_in;
    let centre = on + out * (thick.max(0.0) / 12.0 * upf / 2.0);
    let along = Vec2::new(-out.y, out.x);
    // Half way up the housing, never under the ceiling (its box above the board), never through
    // the floor over it.
    let floor_over = level.height_ft * 12.0;
    let lowest = ceiling.map(|c| c + n.clear_in);
    let mut z0 = (bottom + top) / 2.0 - tall / 2.0;
    if let Some(l) = lowest {
        z0 = z0.max(l);
    }
    if floor_over.is_finite() && floor_over > tall && z0 + tall > floor_over && lowest.is_none_or(|l| floor_over - tall >= l) {
        z0 = floor_over - tall;
    }
    let z1 = z0 + tall;
    let gap_in = if z0 > top {
        z0 - top
    } else if z1 < bottom {
        bottom - z1
    } else {
        0.0
    };
    let said_ceiling = ceiling.map_or_else(|| "no ceiling known over it".to_string(), |c| format!("the ceiling over it at {c:.1} in"));
    let mut basis = format!(
        "{}; at {}'s housing ({bottom:.1} to {top:.1} in over the floor), {said_ceiling}: the box on its side, above the ceiling",
        n.basis, e.tag
    );
    if gap_in > n.tolerance_in {
        basis.push_str(&format!(
            "; its housing is {gap_in:.1} in from the box, more than the {:.0} in a connection is at its equipment by: the equipment's height against the ceiling is to check",
            n.tolerance_in
        ));
    }
    Some(AtEquipment { tag: e.tag.clone(), centre, along, bottom_in: z0, top_in: z1, ceiling_in: ceiling, gap_in, basis })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(ceiling_ft: f64) -> cadaddy_walk3d::Level {
        cadaddy_walk3d::Level {
            name: "Level 2".into(),
            elevation_ft: 10.0,
            height_ft: 12.0,
            ceiling_ft,
            region: None,
            base: None,
            attr: None,
            sheet: None,
        }
    }

    fn fan(tag: &str, at: [f64; 2], bottom_ft: f64) -> MepEquipment {
        MepEquipment {
            tag: tag.into(),
            kind: "exhaust_fan".into(),
            level: "Level 2".into(),
            min: at,
            max: [at[0] + 12.0, at[1] + 12.0],
            bottom_ft,
            height_ft: 10.0 / 12.0,
            ..Default::default()
        }
    }

    fn conn(mark: &str, at: [f64; 2], note: &str) -> ElementTag {
        let mut t = ElementTag {
            id: "device:Level 2:1,1".into(),
            kind: "equipment_connection".into(),
            level: "Level 2".into(),
            at_ft: at,
            ..Default::default()
        };
        t.set("type", mark, "plan");
        if !note.is_empty() {
            t.set("mount_height_note", note, "note");
        }
        t
    }

    const BOX: [f64; 3] = [2.0, 4.0, 4.0];

    #[test]
    fn a_marks_name_reaches_its_numbered_tags_and_no_other_mark() {
        assert!(names("EF-2", "EF") && names("EF2", "ef") && names("EF", "EF"));
        assert!(!names("EFX", "EF") && !names("EF-2", "EF-1") && !names("", "EF"));
        assert!(same_level("3", "Level 3") && same_level("Level 2", "level 2") && !same_level("Level 2", "Level 3"));
        let t = conn("J", [0.0, 0.0], "connection to EF-4 in the room's ceiling (M-5)");
        assert_eq!(named(&t), vec!["EF-4".to_string(), "J".to_string()]);
    }

    #[test]
    fn a_fan_connection_is_above_the_ceiling_at_the_housing_whatever_height_it_was_given() {
        let n = Numbers::read();
        // Ceilings of 7'-11" and 9'-0": each fan at its ceiling, the connection read at its middle.
        for ceiling in [95.0, 108.0] {
            let f = fan("EF", [100.0, 50.0], ceiling / 12.0);
            let t = conn("EF", [106.0 / 12.0, 56.0 / 12.0], "connection to EF in the room's ceiling (M-5)");
            let at = Vec2::new(106.0, 56.0);
            let got = place(&t, std::slice::from_ref(&f), &level(8.0), at, at, 12.0, BOX, &|_| Some(ceiling), &n)
                .unwrap_or_else(|| panic!("no place at {ceiling}"));
            assert!(got.bottom_in >= ceiling + n.clear_in - 1e-9, "above the {ceiling} in ceiling: {got:?}");
            assert!(got.gap_in <= n.tolerance_in && got.gap_in == 0.0, "at the housing: {got:?}");
            assert!((got.middle_in() - (ceiling + 5.0)).abs() < 1e-9, "half way up the housing: {got:?}");
            // On the housing's side, touching it.
            assert!((got.centre.x - 99.0).abs() < 1e-9 && got.centre.y == 56.0, "{got:?}");
            assert!(got.basis.starts_with(RULE), "{}", got.basis);
        }
    }

    #[test]
    fn it_follows_the_fan_and_the_ceiling() {
        let n = Numbers::read();
        let t = conn("EF", [0.0, 0.0], "");
        let at = Vec2::new(90.0, 56.0);
        let a =
            place(&t, &[fan("EF", [100.0, 50.0], 95.0 / 12.0)], &level(8.0), at, at, 12.0, BOX, &|_| Some(95.0), &n).unwrap_or_else(|| panic!("a"));
        let b =
            place(&t, &[fan("EF", [130.0, 50.0], 120.0 / 12.0)], &level(8.0), at, at, 12.0, BOX, &|_| Some(120.0), &n).unwrap_or_else(|| panic!("b"));
        assert!(b.bottom_in > a.bottom_in + 20.0 && b.centre.x > a.centre.x + 20.0, "{a:?} {b:?}");
        // A fan whose housing was left under a ceiling raised since: the box still above the
        // ceiling, the gap said.
        let c =
            place(&t, &[fan("EF", [100.0, 50.0], 95.0 / 12.0)], &level(8.0), at, at, 12.0, BOX, &|_| Some(117.0), &n).unwrap_or_else(|| panic!("c"));
        assert!(c.bottom_in >= 118.0 && c.gap_in > n.tolerance_in && c.basis.contains("to check"), "{c:?}");
    }

    #[test]
    fn other_equipment_and_far_or_other_level_fans_are_left_alone() {
        let n = Numbers::read();
        let t = conn("EF", [0.0, 0.0], "");
        let at = Vec2::new(106.0, 56.0);
        let mut wall = fan("EF", [100.0, 50.0], 7.0);
        wall.title = "SIDEWALL EXHAUST FAN".into();
        assert!(place(&t, std::slice::from_ref(&wall), &level(9.0), at, at, 12.0, BOX, &|_| Some(108.0), &n).is_none(), "a wall fan");
        let far = fan("EF", [100.0 + 12.0 * 40.0, 50.0], 9.0);
        assert!(place(&t, &[far], &level(9.0), at, at, 12.0, BOX, &|_| Some(108.0), &n).is_none(), "40 ft off");
        let mut other = fan("EF", [100.0, 50.0], 9.0);
        other.level = "Level 3".into();
        assert!(place(&t, &[other], &level(9.0), at, at, 12.0, BOX, &|_| Some(108.0), &n).is_none(), "another level");
        let mut roof = level(9.0);
        roof.name = "Roof".into();
        let mut up = fan("EF", [100.0, 50.0], 1.0);
        up.level = "Roof".into();
        assert!(place(&t, &[up], &roof, at, at, 12.0, BOX, &|_| None, &n).is_none(), "a roof fan");
        // Hostile numbers build nothing and never panic.
        let mut bad = fan("EF", [f64::NAN, 50.0], 9.0);
        bad.height_ft = f64::INFINITY;
        assert!(place(&t, &[bad], &level(9.0), at, at, 12.0, BOX, &|_| Some(108.0), &n).is_none());
        assert!(place(&t, &[fan("EF", [100.0, 50.0], 9.0)], &level(9.0), at, at, f64::NAN, BOX, &|_| Some(108.0), &n).is_none());
    }
}
