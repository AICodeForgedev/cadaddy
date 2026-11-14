//! Q6598: a service drawn as two systems (a sprinkler main and its branch lines named apart, a
//! riser and its floor branches) still gets its tee where a branch ends on its main or leaves its
//! riser, in the takeoff (`mep.qty`) and in the model (`model.build`), and the two are connected.
//! Other services (sanitary and vent) and runs that pass at another height get none.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook::made_tag;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::{Value, json};

/// Three storeys of 10 ft (sheet units are inches); a 2" sprinkler main and a 3/4" branch
/// ending on its side, the branch's system named `branch`; a 4" riser from the slab through the
/// roof with two 1" branches leaving it on level 1 (a cross) and one on level 2 (a tee); a branch
/// ending on the main's side 5 ft under it (no tee); a vent ending on a sanitary main (no tee).
fn model(branch: &str) -> Session {
    let mut s = Session::new();
    let level = |name: &str, i: u32| json!({"name": name, "elevation_ft": f64::from(i) * 10.0, "height_ft": 10.0, "ceiling_ft": 8.0});
    let pipe = |name: &str, layer: &str, d: f64| json!({"name": name, "layer": layer, "kind": "pipe", "size_in": [d, 0], "bottom_ft": 9});
    let mut systems = vec![
        pipe("FP-MAIN", "F-SPRK-MAIN", 2.0),
        pipe("FP-RISER", "F-SPRK-RISR", 4.0),
        pipe("FP", "F-SPRK", 1.0),
        pipe("SAN", "P-SANR", 3.0),
        pipe("V", "P-VENT", 2.0),
    ];
    if !systems.iter().any(|s| s["name"] == branch) {
        systems.push(pipe(branch, "F-SPRK-BRCH", 0.75));
    }
    let run = |system: &str, level: &str, pts: Value, d: f64, bottom: f64| json!({"system": system, "level": level, "points": pts, "size_in": [d, 0], "bottoms_ft": [bottom]});
    let spec = json!({
        "units_per_foot": 12,
        "stack": false,
        "levels": [level("Level 1", 0), level("Level 2", 1), level("Level 3", 2)],
        "mep": {
            "systems": systems,
            "runs": [
                run("FP-MAIN", "Level 1", json!([[0, 0], [480, 0]]), 2.0, 9.0),
                run(branch, "Level 1", json!([[240, 0], [240, 180]]), 0.75, 9.0),
                run("FP", "Level 1", json!([[360, 0], [360, -180]]), 1.0, 4.0),
                {"system": "FP-RISER", "level": "Level 1", "points": [[600, 600]], "size_in": [4, 0], "bottoms_ft": [0.0], "top_ft": 30.0},
                run("FP", "Level 1", json!([[600, 600], [360, 600]]), 1.0, 9.0),
                run("FP", "Level 1", json!([[600, 600], [840, 600]]), 1.0, 9.0),
                run("FP", "Level 2", json!([[600, 600], [840, 600]]), 1.0, 9.0),
                run("SAN", "Level 1", json!([[0, 1200], [480, 1200]]), 3.0, 9.0),
                run("V", "Level 1", json!([[240, 1200], [240, 1400]]), 2.0, 9.0)
            ]
        }
    });
    s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
    s
}

/// The takeoff's fittings: (level, system, type, size) → count.
fn fittings(s: &mut Session) -> Vec<(String, String, String, String, u64)> {
    let q = s.execute("mep.qty", &json!({})).unwrap();
    let mut out: Vec<_> = q["fittings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let t = |k: &str| f[k].as_str().unwrap_or_default().to_string();
            (t("level"), t("system"), t("type"), t("size"), f["count"].as_u64().unwrap_or(0))
        })
        .collect();
    out.sort();
    out
}

fn row(level: &str, system: &str, ty: &str, size: &str) -> (String, String, String, String, u64) {
    (level.into(), system.into(), ty.into(), size.into(), 1)
}

/// The tee and the cross, whatever the branch's system is named: named as the main (the control)
/// or apart from it. Nothing else: no tee for the branch 5 ft under the main, none for the vent
/// on the sanitary main.
#[test]
fn a_branch_of_another_system_name_gets_its_tee_in_the_takeoff() {
    for branch in ["FP-MAIN", "FP-BRANCH", "Sprinkler Branch Lines"] {
        let got = fittings(&mut model(branch));
        assert_eq!(
            got,
            vec![
                row("Level 1", "FP-MAIN", "tee", "2\" x 3/4\""),
                row("Level 1", "FP-RISER", "cross", "4\""),
                row("Level 2", "FP-RISER", "tee", "4\" x 1\""),
            ],
            "branch system {branch}"
        );
    }
}

fn mep(s: &Session, part: &str) -> Vec<(Handle, String, String)> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("mep") && made_tag(e, "part") == Some(part))
        .map(|e| (e.handle, made_tag(e, "system").unwrap_or_default().to_string(), cadaddy_engine::cmd::bspec::made_level(e).unwrap_or_default()))
        .collect()
}

fn connects(s: &Session, h: Handle) -> Vec<Handle> {
    let d = s.doc().unwrap();
    let e = d.entity(h).unwrap();
    cadaddy_engine::doc::props::read(&e.common.xdata)
        .get("connects")
        .map(|p| p.text())
        .unwrap_or_default()
        .split(',')
        .filter_map(Handle::parse_hex)
        .collect()
}

/// The model builds the same fittings, each connected to the runs it joins: the branch to the
/// main though their systems are named apart, the floor branch to the riser's piece on its level.
#[test]
fn a_branch_of_another_system_name_gets_its_tee_in_the_model_and_is_connected() {
    let mut s = model("FP-BRANCH");
    let r = s.execute("model.build", &json!({"echo": false})).unwrap();
    assert_eq!((r["tees"].clone(), r["crosses"].clone(), r["reducers"].clone()), (json!(2), json!(1), json!(0)), "{r}");
    let tees = mep(&s, "fitting:tee");
    let segs = mep(&s, "segment");
    let seg =
        |system: &str, level: &str| -> Vec<Handle> { segs.iter().filter(|(_, sy, l)| sy == system && l == level).map(|(h, _, _)| *h).collect() };
    // The main's tee: drawn with the main, joining the main and the branch.
    let main_tee = tees.iter().find(|(_, sy, _)| sy == "FP-MAIN").expect("the main's tee");
    let c = connects(&s, main_tee.0);
    for h in seg("FP-MAIN", "Level 1").into_iter().chain(seg("FP-BRANCH", "Level 1")) {
        assert!(c.contains(&h), "the tee connects {h:?}: {c:?}");
    }
    // The riser's tee on level 2: upright, around the branch's height (19' to 19'-1").
    let riser_tee = tees.iter().find(|(_, sy, l)| sy == "FP-RISER" && l == "Level 2").expect("the riser's tee");
    let d = s.doc().unwrap();
    let EntityKind::Mesh(m) = &d.entity(riser_tee.0).unwrap().kind else { panic!("a mesh") };
    let (lo, hi) = m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v.z), b.max(v.z)));
    assert!(lo > 220.0 && lo < 228.0 && hi > 229.0 && hi < 236.0, "{lo} {hi}");
    let c = connects(&s, riser_tee.0);
    let riser_l2 = seg("FP-RISER", "Level 2");
    assert!(!riser_l2.is_empty() && riser_l2.iter().all(|h| c.contains(h)), "{c:?} {riser_l2:?}");
    assert!(seg("FP", "Level 2").iter().all(|h| c.contains(h)), "{c:?}");
    // No fitting on the sanitary main, none for the low branch.
    let fits: Vec<_> = d.model.iter().filter(|e| made_tag(e, "part").is_some_and(|p| p.starts_with("fitting:"))).collect();
    assert!(fits.iter().all(|e| made_tag(e, "system") != Some("SAN") && made_tag(e, "system") != Some("V")), "{}", fits.len());
}
