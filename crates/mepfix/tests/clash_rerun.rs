//! Re-running clash detection after an edit keeps what people decided about each clash (Q261
//! rework): the engine's `model.clashes {previous}` matches clashes by kind, level, systems and
//! plan position, never by handles, so a rebuilt model with new handles keeps its triage.

use cadaddy_engine::Session;
use cadaddy_mepfix::truth::Truth;
use serde_json::{Value, json};

/// The fixture's horizontal ducts and pipes as building-spec runs (one run per segment, plan
/// inches), each tagged with a made-up object handle `A<i>` so the test can tell runs apart.
fn session(t: &Truth) -> Session {
    let mut systems: Vec<Value> = Vec::new();
    let mut runs: Vec<Value> = Vec::new();
    for s in t.segments.iter().filter(|s| !s.vertical && s.height.is_some()) {
        let Some(h) = &s.height else { continue };
        if !systems.iter().any(|x| x["name"] == s.system.as_str()) {
            let kind = if s.kind == "duct" { "duct" } else { "pipe" };
            systems.push(json!({"name": s.system, "layer": format!("M-{}", s.system), "kind": kind, "size_in": s.size_in, "bottom_ft": h.bottom_ft}));
        }
        let [a, b] = s.path_ft;
        let i = runs.len();
        runs.push(json!({
            "system": s.system, "level": s.level, "points": [[a[0] * 12.0, a[1] * 12.0], [b[0] * 12.0, b[1] * 12.0]],
            "size_in": s.size_in, "bottoms_ft": [h.bottom_ft], "objects": [format!("A{i:X}")],
        }));
    }
    let levels: Vec<Value> = t
        .building
        .levels
        .iter()
        .map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 13, "ceiling_ft": 9, "region": [-100000, -100000, 100000, 100000], "base": [0, 0]}))
        .collect();
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"levels": levels, "mep": {"systems": systems, "runs": runs}}})).unwrap();
    s
}

/// The run index a clash belongs to (its first handle `A<i>` / `B<i>`).
fn runs_of(c: &Value) -> Vec<usize> {
    c["handles"].as_array().unwrap().iter().filter_map(|h| usize::from_str_radix(h.as_str()?.get(1..)?, 16).ok()).collect()
}

/// Every run's objects renamed `A<i>` → `B<i>`: what rebuilding the model or re-reading the
/// prints does to the handles.
fn new_handles(s: &mut Session) {
    let mut spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    for r in spec["mep"]["runs"].as_array_mut().unwrap() {
        let renamed: Vec<String> = r["objects"].as_array().unwrap().iter().map(|o| o.as_str().unwrap().replacen('A', "B", 1)).collect();
        r["objects"] = json!(renamed);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
}

#[test]
fn rerun_after_an_edit_keeps_statuses_on_a_fixture() {
    for seed in [1u64, 2] {
        let set = cadaddy_mepfix::generate(seed).unwrap();
        let t = &set.truth;
        let mut s = session(t);
        let first = s.execute("model.clashes", &json!({})).unwrap()["clashes"].as_array().unwrap().clone();
        assert!(first.len() >= 3, "seed {seed}: {} clashes", first.len());
        // Every clash triaged: an id, a status, a trade and a note of its own.
        let triaged: Vec<Value> = first
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut c = c.clone();
                c["id"] = json!(format!("C{:04}", i + 1));
                c["status"] = json!(if i % 2 == 0 { "Active" } else { "Resolved" });
                c["trade"] = json!("Mechanical");
                c["notes"] = json!(format!("note {}", i + 1));
                c["author"] = json!("Reviewer A");
                c
            })
            .collect();
        // Fix: the first run hanging below the ceiling goes up to 9'-6" (above the 9 ft ceiling).
        let fixed = triaged.iter().find(|c| c["kind"] == "below_ceiling").unwrap_or_else(|| panic!("seed {seed}: no below-ceiling clash"));
        let fixed_run = runs_of(fixed)[0];
        let fixed_ids: Vec<&str> =
            triaged.iter().filter(|c| c["kind"] == "below_ceiling" && runs_of(c).contains(&fixed_run)).map(|c| c["id"].as_str().unwrap()).collect();
        // New: a run with no clash drops to 6 ft.
        let n_runs = s.execute("mep.list", &json!({})).unwrap()["items"].as_array().unwrap().len();
        let quiet_run = (0..n_runs).find(|r| !first.iter().any(|c| runs_of(c).contains(r))).expect("a run with no clash");
        s.execute("mep.edit", &json!({"id": fixed_run, "bottom_ft": 9.5})).unwrap();
        s.execute("mep.edit", &json!({"id": quiet_run, "bottom_ft": 6.0, "rebuild": true})).unwrap();
        new_handles(&mut s);
        let again = s.execute("model.clashes", &json!({"previous": triaged})).unwrap();
        let now = again["clashes"].as_array().unwrap();
        let gone = again["gone"].as_array().unwrap();
        // The fixed run's below-ceiling clashes are gone, and reported as such.
        assert!(!now.iter().any(|c| c["kind"] == "below_ceiling" && runs_of(c).contains(&fixed_run)), "seed {seed}");
        for id in &fixed_ids {
            assert!(gone.iter().any(|g| g["id"] == *id), "seed {seed}: {id} not reported gone");
            assert!(!now.iter().any(|c| c["id"] == *id), "seed {seed}: {id} still listed");
        }
        // The new one is new: no id, no status carried.
        let new =
            now.iter().find(|c| c["kind"] == "below_ceiling" && runs_of(c).contains(&quiet_run)).expect("the dropped run hangs below the ceiling");
        assert!(new.get("id").is_none() && new.get("status").is_none(), "seed {seed}: {new}");
        // Every clash of the untouched runs keeps its id, status, trade, notes and author, from the
        // clash at the same place (the handles all changed).
        let untouched: Vec<&Value> = now.iter().filter(|c| !runs_of(c).iter().any(|r| *r == fixed_run || *r == quiet_run)).collect();
        let before = triaged.iter().filter(|c| !runs_of(c).iter().any(|r| *r == fixed_run || *r == quiet_run)).count();
        assert_eq!(untouched.len(), before, "seed {seed}");
        for c in untouched {
            let id = c["id"].as_str().unwrap_or_else(|| panic!("seed {seed}: untouched clash lost its id: {c}"));
            let was = triaged.iter().find(|p| p["id"] == id).unwrap();
            assert_eq!(was["kind"], c["kind"]);
            assert_eq!(was["at"], c["at"], "seed {seed}: {id} matched a clash elsewhere");
            assert_eq!(runs_of(was), runs_of(c), "seed {seed}: {id}");
            for f in ["status", "trade", "notes", "author"] {
                assert_eq!(was[f], c[f], "seed {seed}: {id} {f}");
            }
        }
        assert!(c_ids_unique(now), "seed {seed}");
    }
}

fn c_ids_unique(now: &[Value]) -> bool {
    let ids: Vec<&str> = now.iter().filter_map(|c| c["id"].as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.len() == ids.len()
}

/// A clash that moved further than the match distance is a different clash; one within it is
/// the same (a run nudged on the plan keeps its triage).
#[test]
fn match_distance_decides_whether_a_moved_clash_is_the_same() {
    let set = cadaddy_mepfix::generate(1).unwrap();
    let mut s = session(&set.truth);
    let first = s.execute("model.clashes", &json!({})).unwrap()["clashes"].as_array().unwrap().clone();
    let c = first.iter().find(|c| c["kind"] == "below_ceiling").unwrap().clone();
    let shifted = |dx_ft: f64| {
        let mut p = c.clone();
        p["at"][0] = json!(p["at"][0].as_f64().unwrap() + dx_ft * 12.0);
        p["id"] = json!("C0099");
        p["status"] = json!("Active");
        p
    };
    let near = s.execute("model.clashes", &json!({"previous": [shifted(1.5)]})).unwrap();
    assert!(near["clashes"].as_array().unwrap().iter().any(|x| x["id"] == "C0099" && x["status"] == "Active"));
    assert!(near["gone"].as_array().unwrap().is_empty());
    let far = s.execute("model.clashes", &json!({"previous": [shifted(5.0)]})).unwrap();
    assert!(!far["clashes"].as_array().unwrap().iter().any(|x| x["id"] == "C0099"));
    assert_eq!(far["gone"][0]["id"], "C0099");
    // A wider match distance takes it again; hostile values are ignored.
    let wide = s.execute("model.clashes", &json!({"previous": [shifted(5.0)], "match_ft": 6})).unwrap();
    assert!(wide["clashes"].as_array().unwrap().iter().any(|x| x["id"] == "C0099"));
    for bad in [json!({"previous": [{"at": "x"}, 5, null]}), json!({"previous": [shifted(0.0)], "match_ft": -1}), json!({"previous": "no"})] {
        assert!(s.execute("model.clashes", &bad).is_ok(), "{bad}");
    }
}
