//! Placement on a face (Q6174): faceplates, tolerance bands and the solve inside the framing.
//!
//! Everything here is measured on one face, in inches: `along` from the face's start, `z` above
//! the floor. A device's physical extent is its faceplate's outline; checks use its edges.
//! Height is hard (a documented AFF never moves). The along-wall position is soft: the documented
//! or plan position is the target, the box is solved to the side of a stud inside the allowed
//! band, never in a bay a pipe or other excluding element occupies.

use serde_json::Value;

/// The layout rules data: plate sizes, bands, tolerances, concealed-behind appliances.
pub(crate) const RULES: &str = include_str!("../layout_rules.json");

/// The rules data, read once per call (small).
pub(crate) fn rules() -> Value {
    serde_json::from_str(RULES).unwrap_or_default()
}

fn num(v: &Value, key: &str, or: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(or)
}

/// A faceplate: style, gangs, outer width and height (inches) and where its size came from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Plate {
    pub style: String,
    pub gangs: u32,
    pub w: f64,
    pub h: f64,
    /// `spec`/`submittal`/`type` (a document said the style) or `rules data` (derived).
    pub from: String,
}

/// The plate style a device's own words name (`JUMBO`, `MIDWAY`/`MID-SIZE`, `OVERSIZE`), else
/// none.
pub(crate) fn style_said(words: &str) -> Option<&'static str> {
    let u = words.to_ascii_uppercase();
    if u.contains("JUMBO") || u.contains("OVERSIZE") || u.contains("OVER-SIZE") {
        Some("jumbo")
    } else if u.contains("MIDWAY") || u.contains("MID-SIZE") || u.contains("MIDSIZE") || u.contains("MID SIZE") {
        Some("midsize")
    } else if u.contains("STANDARD SIZE") || u.contains("STANDARD PLATE") {
        Some("standard")
    } else {
        None
    }
}

/// The plate of `style` with `gangs`, from the rules data's sizes (unknown style: the default).
pub(crate) fn plate(rules: &Value, style: Option<&str>, gangs: u32, from: &str) -> Plate {
    let plates = &rules["faceplates"];
    let def = rules["default_plate_style"].as_str().unwrap_or("standard");
    let st = style.filter(|s| plates.get(*s).is_some()).unwrap_or(def);
    let row = &plates[st];
    let g = gangs.clamp(1, 12);
    let w = num(row, "w_one_gang_in", 2.75) + num(row, "per_gang_in", 1.81) * f64::from(g - 1);
    Plate { style: st.to_string(), gangs: g, w, h: num(row, "h_in", 4.5), from: from.to_string() }
}

/// A rectangle on a face: along from..to, z from..to (inches).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub a0: f64,
    pub a1: f64,
    pub z0: f64,
    pub z1: f64,
}

impl Rect {
    pub fn centred(along: f64, z: f64, w: f64, h: f64) -> Rect {
        Rect { a0: along - w / 2.0, a1: along + w / 2.0, z0: z - h / 2.0, z1: z + h / 2.0 }
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.a0 < o.a1 && o.a0 < self.a1 && self.z0 < o.z1 && o.z0 < self.z1
    }
    /// The plate edges outside `zone` (shrunk by `clear`): which edge and by how much (inches).
    pub fn outside(&self, zone: &Rect, clear: f64) -> Vec<(&'static str, f64)> {
        let mut out = Vec::new();
        let checks = [
            ("left", (zone.a0 + clear) - self.a0),
            ("right", self.a1 - (zone.a1 - clear)),
            ("bottom", (zone.z0 + clear) - self.z0),
            ("top", self.z1 - (zone.z1 - clear)),
        ];
        for (edge, by) in checks {
            if by > 1e-6 {
                out.push((edge, by));
            }
        }
        out
    }
}

/// Devices on one face that share a plate: each device's along and centre height (inches); the
/// groups (indices) of devices side by side at one height within the gang spacing.
pub(crate) fn gangs(devs: &[(f64, f64)], spacing: f64, dz: f64) -> Vec<Vec<usize>> {
    let mut idx: Vec<usize> = (0..devs.len()).collect();
    idx.sort_by(|&x, &y| {
        let (a, b) = (devs.get(x).copied().unwrap_or_default(), devs.get(y).copied().unwrap_or_default());
        a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0))
    });
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in idx {
        let Some(&(t, z)) = devs.get(i) else { continue };
        let joined = groups
            .iter_mut()
            .find(|g| g.last().and_then(|&j| devs.get(j)).is_some_and(|&(tj, zj)| (t - tj).abs() <= spacing && (z - zj).abs() <= dz));
        match joined {
            Some(g) => g.push(i),
            None => groups.push(vec![i]),
        }
    }
    groups
}

/// What excludes a box from a stud bay: its along extent on the face and what it is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Obstacle {
    pub a0: f64,
    pub a1: f64,
    pub what: String,
}

/// How a box's along position came out of the framing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Solved {
    /// No studs in the model yet: the band is kept, nothing is solved.
    PendingFraming,
    /// Solved to `at` (the box's centre), `drift` from the target, and why.
    At { at: f64, drift: f64, why: String },
    /// No legal spot inside the band: what to do.
    Conflict { why: String, suggest: Vec<String> },
}

/// Solve a box `box_w` wide whose design centre is `target` with a drift band `±band` on a face
/// whose studs (`studs`: their centres, `stud_w` wide) are known, away from `obstacles`. The box
/// mounts to the side of a stud (either side), wholly inside one bay; it never sits in a bay an
/// obstacle occupies. The legal spot nearest the target wins; none within the band: a conflict.
pub(crate) fn solve(target: f64, band: f64, box_w: f64, studs: &[f64], stud_w: f64, obstacles: &[Obstacle]) -> Solved {
    if studs.is_empty() {
        return Solved::PendingFraming;
    }
    let mut ss: Vec<f64> = studs.iter().copied().filter(|s| s.is_finite()).take(10_000).collect();
    ss.sort_by(f64::total_cmp);
    let half_s = stud_w / 2.0;
    let mut best: Option<(f64, f64, String)> = None;
    let mut blocked: Vec<String> = Vec::new();
    for (k, s) in ss.iter().enumerate() {
        for side in [-1.0f64, 1.0] {
            // The box against this side of the stud, in the bay that way.
            let at = s + side * (half_s + box_w / 2.0);
            let bay = if side > 0.0 {
                (s + half_s, ss.get(k + 1).map_or(f64::INFINITY, |n| n - half_s))
            } else {
                (k.checked_sub(1).and_then(|j| ss.get(j)).map_or(f64::NEG_INFINITY, |p| p + half_s), s - half_s)
            };
            if bay.1 - bay.0 < box_w - 1e-9 {
                continue;
            }
            if let Some(o) = obstacles.iter().find(|o| o.a0 < bay.1 && o.a1 > bay.0) {
                if bay.0 < target + band && bay.1 > target - band && !blocked.contains(&o.what) {
                    blocked.push(o.what.clone());
                }
                continue;
            }
            let drift = at - target;
            if best.as_ref().is_none_or(|b| drift.abs() < b.1.abs() - 1e-9) {
                let why = format!("{} of the stud at {:.1} in", if side > 0.0 { "right side" } else { "left side" }, s);
                best = Some((at, drift, why));
            }
        }
    }
    match best {
        Some((at, drift, mut why)) if drift.abs() <= band + 1e-9 => {
            if let Some(b) = blocked.first() {
                why = format!("{why}; the nearer bay has {b}");
            }
            Solved::At { at, drift, why }
        }
        other => {
            let mut suggest = Vec::new();
            if let Some((at, drift, _)) = other {
                suggest.push(format!("move to the next legal bay: {at:.1} in ({drift:+.1} in, outside the ±{band:.1} in band)"));
            }
            suggest.push("add blocking for a box between studs".to_string());
            if !blocked.is_empty() {
                suggest.push(format!("ask the plumber to offset {}", blocked.join(", ")));
            }
            let why = if blocked.is_empty() {
                format!("no stud side inside the ±{band:.1} in band round {target:.1} in")
            } else {
                format!("every bay inside the ±{band:.1} in band round {target:.1} in is taken ({})", blocked.join(", "))
            };
            Solved::Conflict { why, suggest }
        }
    }
}

/// Studs from a wall's start every `spacing` along a face `len` long (plus one at the end): the
/// layout the framing (tradeatoms) will build; tests use it.
#[cfg(test)]
pub(crate) fn stud_layout(start: f64, len: f64, spacing: f64) -> Vec<f64> {
    if !(spacing.is_finite() && spacing > 1.0 && len.is_finite() && len > 0.0) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut s = start;
    while s <= start + len + 1e-9 && out.len() < 10_000 {
        out.push(s);
        s += spacing;
    }
    if out.last().is_some_and(|l| (start + len - l).abs() > 1e-6) {
        out.push(start + len);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plates_from_the_rules_data_and_the_documents_words() {
        let r = rules();
        let one = plate(&r, None, 1, "rules data");
        let jumbo = plate(&r, Some("jumbo"), 1, "spec");
        let two = plate(&r, None, 2, "rules data");
        assert!(jumbo.w > one.w && jumbo.h > one.h, "{one:?} {jumbo:?}");
        assert!(two.w > one.w && (two.h - one.h).abs() < 1e-9 && two.gangs == 2);
        assert_eq!(style_said("DUPLEX RECEPTACLE, JUMBO NYLON PLATE"), Some("jumbo"));
        assert_eq!(style_said("DUPLEX RECEPTACLE"), None);
    }

    #[test]
    fn two_devices_side_by_side_share_one_plate() {
        let g = gangs(&[(10.0, 48.0), (11.81, 48.0), (40.0, 48.0), (10.0, 18.0)], 2.5, 1.0);
        assert_eq!(g.len(), 3, "{g:?}");
        assert!(g.iter().any(|x| x.len() == 2 && x.contains(&0) && x.contains(&1)));
    }

    #[test]
    fn a_plate_edge_outside_its_zone_is_named() {
        let cab = Rect { a0: 0.0, a1: 30.0, z0: 54.0, z1: 84.0 };
        let p = Rect::centred(29.0, 66.0, 2.75, 4.5);
        let out = p.outside(&cab, 0.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "right");
        assert!((out[0].1 - 0.375).abs() < 1e-9, "{out:?}");
    }

    /// Studs 16" on centre from 0.
    fn studs() -> Vec<f64> {
        stud_layout(0.0, 96.0, 16.0)
    }

    #[test]
    fn a_target_two_inches_from_a_stud_solves_to_the_stud_side() {
        // Target 18.5": the stud at 16 (to 16.75) — the box (2.25 wide) at its right side, centre 17.875.
        match solve(18.5, 6.0, 2.25, &studs(), 1.5, &[]) {
            Solved::At { at, drift, why } => {
                assert!((at - 17.875).abs() < 1e-9 && (drift + 0.625).abs() < 1e-9, "{at} {drift}");
                assert!(why.contains("stud at 16.0"), "{why}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_target_in_a_bay_with_a_stack_moves_to_the_next_bay_inside_the_band() {
        // The 16–32 bay holds a 3" stack: the box goes to the nearest side of another bay.
        let stack = Obstacle { a0: 22.0, a1: 25.5, what: "a 3 in SAN stack".into() };
        match solve(20.0, 15.0, 2.25, &studs(), 1.5, std::slice::from_ref(&stack)) {
            Solved::At { at, why, .. } => {
                assert!(!(16.0..32.0).contains(&at), "not in the stack's bay: {at}");
                assert!(why.contains("SAN stack"), "{why}");
            }
            other => panic!("{other:?}"),
        }
        // A tight band: no legal bay → a conflict naming the stack, with fixes.
        match solve(24.0, 3.0, 2.25, &studs(), 1.5, &[stack]) {
            Solved::Conflict { why, suggest } => {
                assert!(why.contains("SAN stack"), "{why}");
                assert!(suggest.iter().any(|s| s.contains("blocking")) && suggest.iter().any(|s| s.contains("plumber")), "{suggest:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_studs_means_pending_framing() {
        assert_eq!(solve(20.0, 6.0, 2.25, &[], 1.5, &[]), Solved::PendingFraming);
    }
}
