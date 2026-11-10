//! A vertical MEP run cut at each floor it passes (Q6493).
//!
//! A riser (a run with one plan point and a `top_ft`) is given on the level it starts on, but a
//! stack or a duct shaft through four storeys stands in four storeys: the field installs it a
//! storey at a time, each storey's piece through its own floor (with that floor's penetration and
//! firestop), and a level's takeoff, model tree, export and clash scope count that storey's piece.
//! The build and the takeoffs take a riser as its pieces: one per storey, each on the level whose
//! storey it stands in (the highest floor at or under its foot; under every floor, the lowest
//! level), its plan point carried onto that level's plan, its heights over that level's floor. The
//! pieces keep the run's identity: the same source, the same id and the riser's name
//! ([`StoreyCut::riser`]) on every piece. A run in one storey is not cut.

use std::borrow::Cow;

use cadaddy_walk3d::{BuildingSpec, MepRun};

/// The most levels a riser is cut at (a bound on hostile input).
const MAX_LEVELS: usize = 500;

/// Heights closer than this (feet) are one.
const EPS_FT: f64 = 1e-6;

/// Where one storey's piece of a riser comes from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoreyCut {
    /// The whole riser's name ([`super::mep::run_base`] of the run as given): the same on every
    /// piece.
    pub riser: String,
    /// The level the riser is given on.
    pub given_level: String,
    /// This piece, from the bottom (0), and how many there are.
    pub index: usize,
    pub count: usize,
    /// The floors (level names) this piece passes: its foot through its own floor, a slab it
    /// starts under.
    pub floors: Vec<String>,
}

/// One storey's run: the run itself when it stands in one storey, else a piece of it.
#[derive(Clone, Debug)]
pub(crate) struct StoreyRun<'a> {
    pub run: Cow<'a, MepRun>,
    pub cut: Option<StoreyCut>,
}

/// A point of level `from`'s plan on level `to`'s plan (each level's plan may be drawn in its own
/// place: their base points, else their regions' corners; levels with neither share one plan).
pub(crate) fn carry(sp: &BuildingSpec, p: [f64; 2], from: &str, to: &str) -> [f64; 2] {
    if from.eq_ignore_ascii_case(to) {
        return p;
    }
    match super::schedequip::origins(sp, from, to) {
        Some((a, b)) if a.iter().chain(b.iter()).all(|v| v.is_finite()) => [p[0] - a[0] + b[0], p[1] - a[1] + b[1]],
        _ => p,
    }
}

/// `r` as the storeys' runs it stands in: itself (uncut) unless it is a riser that passes a
/// floor or stands in another level's storey than the one it is given on.
pub(crate) fn storey_runs<'a>(sp: &BuildingSpec, r: &'a MepRun) -> Vec<StoreyRun<'a>> {
    let one = || vec![StoreyRun { run: Cow::Borrowed(r), cut: None }];
    let (Some(top), [p], Some(&bot)) = (r.top_ft, r.points.as_slice(), r.bottoms_ft.first()) else { return one() };
    if !(top.is_finite() && bot.is_finite() && top > bot + EPS_FT && p.iter().all(|v| v.is_finite())) {
        return one();
    }
    let Some(own) = sp.level(&r.level) else { return one() };
    let base = own.elevation_ft;
    if !base.is_finite() {
        return one();
    }
    // The floors, over the riser's own level's floor, lowest first. A level on a sheet of its
    // own drawing (an enlarged plan) is no storey of this building.
    let mut floors: Vec<(&str, f64)> =
        sp.levels.iter().filter(|l| l.elevation_ft.is_finite()).take(MAX_LEVELS).map(|l| (l.name.as_str(), l.elevation_ft - base)).collect();
    floors.sort_by(|a, b| a.1.total_cmp(&b.1));
    let Some(&lowest) = floors.first() else { return one() };
    // The storey a height stands in: the highest floor at or under it, else the lowest level.
    let owner = |z: f64| floors.iter().rev().find(|f| f.1 <= z + EPS_FT).copied().unwrap_or(lowest);
    let mut cuts: Vec<f64> = vec![bot];
    cuts.extend(floors.iter().map(|f| f.1).filter(|z| *z > bot + EPS_FT && *z < top - EPS_FT));
    cuts.push(top);
    cuts.dedup_by(|a, b| (*a - *b).abs() <= EPS_FT);
    // Each storey's span; next spans of one storey (a floor of no other level) are one.
    let mut spans: Vec<(&str, f64, f64, f64)> = Vec::new();
    for w in cuts.windows(2) {
        let [a, b] = w else { continue };
        let (name, floor) = owner(*a);
        match spans.last_mut() {
            Some(last) if last.0.eq_ignore_ascii_case(name) => last.3 = *b,
            _ => spans.push((name, floor, *a, *b)),
        }
    }
    if let [(name, _, _, _)] = spans.as_slice()
        && name.eq_ignore_ascii_case(&own.name)
    {
        return one();
    }
    let riser = super::mep::run_base(r);
    let count = spans.len();
    spans
        .iter()
        .enumerate()
        .map(|(index, (name, floor, a, b))| {
            // The floors it passes: each over the riser's foot (it stands on a floor it starts at)
            // from this piece's foot up to under its head.
            let passes: Vec<String> =
                floors.iter().filter(|f| f.1 > bot + EPS_FT && f.1 >= a - EPS_FT && f.1 < b - EPS_FT).map(|f| f.0.to_string()).collect();
            let mut piece = r.clone();
            piece.level = (*name).to_string();
            piece.points = vec![carry(sp, *p, &own.name, name)];
            piece.bottoms_ft = vec![a - floor];
            piece.top_ft = Some(b - floor);
            StoreyRun {
                run: Cow::Owned(piece),
                cut: Some(StoreyCut { riser: riser.clone(), given_level: own.name.clone(), index, count, floors: passes }),
            }
        })
        .collect()
}

/// Every run of `runs` as its storeys' runs ([`storey_runs`]), each with the index of the run it
/// comes from, at most `max` in all.
pub(crate) fn all_storey_runs<'a>(sp: &BuildingSpec, runs: &'a [MepRun], max: usize) -> Vec<(usize, StoreyRun<'a>)> {
    let mut out: Vec<(usize, StoreyRun<'a>)> = Vec::new();
    for (i, r) in runs.iter().enumerate() {
        if out.len() >= max {
            break;
        }
        out.extend(storey_runs(sp, r).into_iter().map(|s| (i, s)));
    }
    out.truncate(max);
    out
}

/// The levels (lower case) the risers standing in any of `levels` (lower case) also stand in:
/// a change to one of those levels moves the riser's cut at its floor, so its pieces on the
/// levels round it change too.
pub(crate) fn levels_sharing_risers(sp: &BuildingSpec, levels: &std::collections::BTreeSet<String>) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for r in sp.mep.runs.iter().filter(|r| r.top_ft.is_some()).take(super::mep::MAX_RUNS) {
        let on: Vec<String> = storey_runs(sp, r).iter().map(|s| s.run.level.to_ascii_lowercase()).collect();
        if on.len() > 1 && on.iter().any(|l| levels.contains(l)) {
            out.extend(on);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(levels: &[(&str, f64)]) -> BuildingSpec {
        let lv: Vec<serde_json::Value> =
            levels.iter().map(|(n, e)| serde_json::json!({"name": n, "elevation_ft": e, "height_ft": 10.0, "ceiling_ft": 8.0})).collect();
        BuildingSpec { levels: serde_json::from_value(serde_json::Value::Array(lv)).unwrap(), ..Default::default() }
    }

    fn riser(level: &str, bot: f64, top: f64) -> MepRun {
        serde_json::from_value(serde_json::json!({
            "system": "SAN", "level": level, "points": [[5.0, 5.0]], "size_in": [3.0, 0.0], "bottoms_ft": [bot], "top_ft": top, "objects": ["1A"]
        }))
        .unwrap()
    }

    /// Four storeys of 10 ft, a stack from under the slab to over the roof: one piece per storey,
    /// each on its own level from its floor, the slab and each floor passed once, one riser name.
    #[test]
    fn a_stack_through_four_storeys_is_four_pieces() {
        let sp = spec(&[("Level 1", 0.0), ("Level 2", 10.0), ("Level 3", 20.0), ("Level 4", 30.0)]);
        let r = riser("Level 1", -1.5, 41.0);
        let ps = storey_runs(&sp, &r);
        let got: Vec<(String, f64, Option<f64>)> = ps.iter().map(|s| (s.run.level.clone(), s.run.bottoms_ft[0], s.run.top_ft)).collect();
        assert_eq!(
            got,
            vec![
                ("Level 1".into(), -1.5, Some(10.0)),
                ("Level 2".into(), 0.0, Some(10.0)),
                ("Level 3".into(), 0.0, Some(10.0)),
                ("Level 4".into(), 0.0, Some(11.0)),
            ]
        );
        let floors: Vec<Vec<String>> = ps.iter().filter_map(|s| s.cut.as_ref()).map(|c| c.floors.clone()).collect();
        assert_eq!(floors, vec![vec!["Level 1".to_string()], vec!["Level 2".into()], vec!["Level 3".into()], vec!["Level 4".into()]]);
        let names: Vec<&str> = ps.iter().filter_map(|s| s.cut.as_ref()).map(|c| c.riser.as_str()).collect();
        assert!(names.iter().all(|n| *n == "1A@5.00,5.00"), "{names:?}");
        assert!(ps.iter().all(|s| s.run.objects == r.objects && s.run.system == "SAN"));
        // The length is kept: 42.5 ft in all.
        let len: f64 = ps.iter().map(|s| s.run.top_ft.unwrap_or(0.0) - s.run.bottoms_ft[0]).sum();
        assert!((len - 42.5).abs() < 1e-9, "{len}");
    }

    /// A riser in one storey, and a run that is not a riser, stay as they are; a riser given on
    /// a level but standing wholly in the storey over it is filed there.
    #[test]
    fn a_riser_in_one_storey_is_not_cut() {
        let sp = spec(&[("L1", 0.0), ("L2", 10.0)]);
        let r = riser("L1", 0.0, 9.0);
        let ps = storey_runs(&sp, &r);
        assert!(matches!(ps.as_slice(), [s] if s.cut.is_none() && matches!(s.run, Cow::Borrowed(_))));
        // Ends at the floor over it: not through it.
        assert!(storey_runs(&sp, &riser("L1", 0.0, 10.0)).iter().all(|s| s.cut.is_none()));
        let r = riser("L1", 12.0, 18.0);
        let up = storey_runs(&sp, &r);
        assert_eq!(up.len(), 1);
        assert_eq!((up[0].run.level.as_str(), up[0].run.bottoms_ft[0], up[0].run.top_ft), ("L2", 2.0, Some(8.0)));
        // A riser given on the upper level reaching down: filed on the lower.
        let r = riser("L2", -8.0, 4.0);
        let down = storey_runs(&sp, &r);
        let got: Vec<(&str, f64)> = down.iter().map(|s| (s.run.level.as_str(), s.run.bottoms_ft[0])).collect();
        assert_eq!(got, vec![("L1", 2.0), ("L2", 0.0)]);
    }

    /// Plans drawn side by side: each piece's plan point is on its own level's plan.
    #[test]
    fn a_piece_is_on_its_own_levels_plan() {
        let mut sp = spec(&[("L1", 0.0), ("L2", 10.0)]);
        sp.levels[0].region = Some([0.0, 0.0, 100.0, 100.0]);
        sp.levels[1].region = Some([200.0, 0.0, 300.0, 100.0]);
        let r = riser("L1", 0.0, 15.0);
        let ps = storey_runs(&sp, &r);
        let at: Vec<[f64; 2]> = ps.iter().map(|s| s.run.points[0]).collect();
        assert_eq!(at, vec![[5.0, 5.0], [205.0, 5.0]]);
    }

    /// Hostile heights and points are not cut (and do not panic).
    #[test]
    fn hostile_risers_are_left_alone() {
        let sp = spec(&[("L1", 0.0), ("L2", 10.0)]);
        for (b, t) in [(f64::NAN, 20.0), (0.0, f64::INFINITY), (20.0, 0.0)] {
            let mut r = riser("L1", 0.0, 20.0);
            r.bottoms_ft = vec![b];
            r.top_ft = Some(t);
            assert!(storey_runs(&sp, &r).iter().all(|s| s.cut.is_none()));
        }
        let mut r = riser("Nowhere", 0.0, 20.0);
        assert!(storey_runs(&sp, &r).iter().all(|s| s.cut.is_none()));
        r.level = "L1".into();
        r.points.clear();
        assert!(storey_runs(&sp, &r).iter().all(|s| s.cut.is_none()));
    }
}
