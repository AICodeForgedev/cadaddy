//! The fixture sets' duct networks (their ground truth) built in 3D: every elbow, tee,
//! transition and drop of the network becomes a fitting object of its own kind, counted exactly
//! against the truth's nodes.
//!
//! The truth's graph goes into the spec as runs the way a print reader hands them over: a run
//! follows a duct through its elbows while the size stays; it ends at a size change, at a branch
//! (unless the duct carries straight on through the tee at the same size: then the branch ends on
//! its side) and at terminals and equipment. A vertical segment is a zero-length one at its
//! own height (a drop between the heights around it).

use cadaddy_engine::Session;
use cadaddy_mepfix::truth::{Node, Segment, Truth};
use cadaddy_mepfix::truth_for;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// What the truth says one level's system has: (elbows in plan, tees, crosses, transitions and
/// reducers, vertical segments).
#[derive(Debug, Default, PartialEq)]
struct Counts {
    elbows: u64,
    tees: u64,
    crosses: u64,
    transitions: u64,
    drops: u64,
}

fn size(s: &Segment) -> [f64; 2] {
    if s.shape == "round" { [s.size_in[0], 0.0] } else { s.size_in }
}

/// The level's runs of `system` as spec runs (sheet units: inches), and the truth's counts.
fn runs_from_truth(t: &Truth, level: &str, system: &str) -> (Vec<Value>, Counts) {
    let elevation = t.building.levels.iter().find(|l| l.code == level).map_or(0.0, |l| l.elevation_ft);
    let segs: Vec<&Segment> = t.segments.iter().filter(|s| s.kind == "duct" && s.level == level && s.system == system).collect();
    let index: HashMap<&str, usize> = segs.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();
    let nodes: HashMap<&str, &Node> = t.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut want = Counts::default();
    // Which two segments a node joins into one run.
    let mut joined: HashMap<(usize, &str), usize> = HashMap::new();
    for n in t.nodes.iter().filter(|n| n.level == level && n.system == system) {
        let here: Vec<usize> = n.segments.iter().filter_map(|id| index.get(id.as_str()).copied()).collect();
        // A node joining another level's or system's ducts (a riser through the floor) is that
        // network's as much as this one's: only the nodes wholly in this one are counted.
        if here.is_empty() || here.len() != n.segments.len() {
            continue;
        }
        let seg = |i: usize| segs[i];
        let away = |i: usize| {
            let s = seg(i);
            let [a, b] = s.path_ft;
            let (from, to) = if s.from == n.id { (a, b) } else { (b, a) };
            let d = [to[0] - from[0], to[1] - from[1]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
            if l > 1e-9 { Some([d[0] / l, d[1] / l]) } else { None }
        };
        let pair = match n.kind.as_str() {
            "elbow" => {
                if here.iter().all(|i| !seg(*i).vertical) {
                    want.elbows += 1;
                }
                match here.as_slice() {
                    [a, b] if size(seg(*a)) == size(seg(*b)) => Some((*a, *b)),
                    _ => None,
                }
            }
            "tee" | "cross" => {
                if n.kind == "tee" {
                    want.tees += 1;
                } else {
                    want.crosses += 1;
                }
                // Straight on through at the same size: one run, the branch on its side.
                let mut through = None;
                for (k, a) in here.iter().enumerate() {
                    for b in here.iter().skip(k + 1) {
                        if let (Some(u), Some(v)) = (away(*a), away(*b))
                            && u[0] * v[0] + u[1] * v[1] < -0.999
                            && size(seg(*a)) == size(seg(*b))
                        {
                            through = Some((*a, *b));
                        }
                    }
                }
                through
            }
            "transition" | "reducer" => {
                want.transitions += 1;
                None
            }
            _ => None,
        };
        if let Some((a, b)) = pair {
            joined.insert((a, n.id.as_str()), b);
            joined.insert((b, n.id.as_str()), a);
        }
    }
    let other_end = |i: usize, node: &str| if segs[i].from == node { segs[i].to.as_str() } else { segs[i].from.as_str() };
    let pos = |id: &str| nodes.get(id).map_or([0.0; 3], |n| n.at_ft);
    // A vertical segment's height at one end: at a terminal or equipment the end itself, else
    // the bottom of the duct it meets.
    let v_bottom = |s: &Segment, node: &str| {
        let z = pos(node)[2] - elevation;
        let end = nodes.get(node).is_some_and(|n| matches!(n.kind.as_str(), "terminal" | "equipment" | "cap" | "open_end"));
        if end { z } else { z - size(s)[0].max(size(s)[1]) / 24.0 }
    };
    // Vertical segments that leave the duct they hang from (one inside it, to a diffuser right
    // under the duct, is no drop).
    let h_bottom = |s: &Segment| s.height.as_ref().map(|h| h.bottom_ft);
    want.drops = segs
        .iter()
        .filter(|s| s.vertical)
        .filter(|s| {
            let (low, high) = if pos(&s.from)[2] <= pos(&s.to)[2] { (&s.from, &s.to) } else { (&s.to, &s.from) };
            let above = nodes
                .get(high.as_str())
                .map(|n| n.segments.iter().filter_map(|id| index.get(id.as_str())).filter_map(|i| h_bottom(segs[*i])).fold(f64::INFINITY, f64::min));
            above.is_none_or(|b| !b.is_finite() || v_bottom(s, low) < b - 1e-6)
        })
        .count() as u64;
    let mut done = vec![false; segs.len()];
    let mut runs = Vec::new();
    // Start at ends that join nothing (then any loop left).
    let starts: Vec<(usize, String)> = (0..segs.len())
        .flat_map(|i| [(i, segs[i].from.clone()), (i, segs[i].to.clone())])
        .filter(|(i, n)| !joined.contains_key(&(*i, n.as_str())))
        .collect();
    for (first, start) in starts {
        if done[first] {
            continue;
        }
        let mut points: Vec<[f64; 2]> = Vec::new();
        let mut bottoms: Vec<f64> = Vec::new();
        let p0 = pos(&start);
        points.push([p0[0] * 12.0, p0[1] * 12.0]);
        let (mut i, mut at) = (first, start.clone());
        loop {
            done[i] = true;
            let s = segs[i];
            let next = other_end(i, &at).to_string();
            let p = pos(&next);
            if s.vertical {
                if bottoms.is_empty() {
                    points.push([p[0] * 12.0, p[1] * 12.0]);
                    bottoms.push(v_bottom(s, &at));
                }
                bottoms.push(v_bottom(s, &next));
            } else {
                let b = s.height.as_ref().map_or(p[2] - elevation - size(s)[0].max(size(s)[1]) / 24.0, |h| h.bottom_ft);
                bottoms.push(b);
            }
            points.push([p[0] * 12.0, p[1] * 12.0]);
            match joined.get(&(i, next.as_str())) {
                Some(j) if !done[*j] => {
                    i = *j;
                    at = next;
                }
                _ => break,
            }
        }
        runs.push(json!({"system": system, "level": level, "points": points, "size_in": size(segs[first]), "bottoms_ft": bottoms}));
    }
    (runs, want)
}

/// Built from the truth: (what model.build counted, the fitting objects by kind).
fn build(t: &Truth, level: &str, system: &str) -> (Value, BTreeMap<String, u64>, Counts) {
    let (runs, want) = runs_from_truth(t, level, system);
    let levels: Vec<Value> = t
        .building
        .levels
        .iter()
        .map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 13, "ceiling_ft": 9, "region": [-100000, -100000, 100000, 100000], "base": [0, 0]}))
        .collect();
    let mut s = Session::new();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {"levels": levels, "mep": {
            "systems": [{"name": system, "layer": format!("M-DUCT-{system}"), "kind": "duct", "size_in": [12, 12], "bottom_ft": 10}],
            "runs": runs
        }}}),
    )
    .unwrap();
    let built = s.execute("model.build", &json!({})).unwrap();
    let mut objects: BTreeMap<String, u64> = BTreeMap::new();
    for e in s.doc().unwrap().model.iter() {
        for v in e.common.xdata.iter().filter_map(|x| x.value.strip_prefix("part:fitting:")) {
            *objects.entry(v.to_string()).or_default() += 1;
        }
    }
    (built, objects, want)
}

fn got(built: &Value, objects: &BTreeMap<String, u64>) -> Counts {
    let n = |k: &str| built[k].as_u64().unwrap_or(0);
    let o = |k: &str| objects.get(k).copied().unwrap_or(0);
    // What model.build reports and the objects it made agree.
    assert_eq!(
        (n("elbows"), n("tees"), n("crosses"), n("transitions") + n("reducers"), n("drops")),
        (o("elbow"), o("tee"), o("cross"), o("transition") + o("reducer"), o("drop")),
        "{built} {objects:?}"
    );
    Counts { elbows: n("elbows"), tees: n("tees"), crosses: n("crosses"), transitions: n("transitions") + n("reducers"), drops: n("drops") }
}

#[test]
fn a_small_supply_network_builds_every_fitting_of_the_truth() {
    // Seed 5, level 1's supply air: a rectangular main with transitions, round branches teed off
    // it, drops to the diffusers.
    let t = truth_for(5);
    let (built, objects, want) = build(&t, "1", "SA");
    eprintln!("truth {want:?}\nbuilt {built}\nobjects {objects:?}");
    assert!(want.elbows > 0 && want.tees > 0 && want.transitions > 0 && want.drops > 0, "the fixture has every kind: {want:?}");
    assert_eq!(got(&built, &objects), want);
}

#[test]
fn every_duct_system_of_several_sets_builds_its_truths_fittings() {
    let mut checked = 0;
    for seed in 1..=6u64 {
        let t = truth_for(seed);
        let mut pairs: Vec<(String, String)> = t.segments.iter().filter(|s| s.kind == "duct").map(|s| (s.level.clone(), s.system.clone())).collect();
        pairs.sort();
        pairs.dedup();
        for (level, system) in pairs {
            let (built, objects, want) = build(&t, &level, &system);
            assert_eq!(got(&built, &objects), want, "seed {seed} level {level} {system}");
            checked += 1;
        }
    }
    assert!(checked >= 20, "{checked} networks");
}
