//! A built element's level follows where it stands (Q6491): copied and moved to another storey,
//! it is filed under the storey whose floor-to-floor band holds it (`bim.element`, `bim.tree`,
//! takeoff by level), keeping its place in the building; `bim.level.move` moves elements to a
//! level by name; a stored `level` property that disagrees with the element's level is flagged.
//! Made-up two- and three-storey buildings, their plans drawn side by side or over one another.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook;
use cadaddy_engine::doc::Handle;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// Feet → inches.
fn ft(p: [f64; 2]) -> [f64; 2] {
    [p[0] * 12.0, p[1] * 12.0]
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [ft(a), ft([b[0], a[1]]), ft(b), ft([a[0], b[1]])], "closed": true})).unwrap();
}

/// A building of `storeys` levels `floor_ft` floor to floor, 30' × 20' with 6" walls; each
/// level's plan drawn `east_ft` east of the one below (0: one over the other). On every level
/// a supply duct under the ceiling and an exhaust fan at the ceiling.
fn building(storeys: usize, floor_ft: f64, east_ft: f64) -> Session {
    let mut s = Session::new();
    let mut levels = Vec::new();
    let mut runs = Vec::new();
    let mut equipment = Vec::new();
    for i in 0..storeys {
        let name = format!("Level {}", i + 1);
        let x0 = east_ft * i as f64;
        let elev = floor_ft * i as f64;
        if i == 0 || east_ft > 0.0 {
            layer(&mut s, "A-WALL");
            rect(&mut s, [x0, 0.0], [x0 + 30.0, 20.0]);
            rect(&mut s, [x0 + 0.5, 0.5], [x0 + 29.5, 19.5]);
        }
        let mut lv = json!({"name": name, "elevation_ft": elev, "height_ft": floor_ft, "ceiling_ft": floor_ft - 2.0});
        if east_ft > 0.0 {
            lv["region"] = json!([(x0 - 5.0) * 12.0, -60.0, (x0 + 40.0) * 12.0, 300.0]);
            lv["base"] = json!([x0 * 12.0, 0.0]);
        }
        levels.push(lv);
        runs.push(json!({"system": "SUPPLY AIR", "level": name, "points": [ft([x0 + 2.0, 4.0]), ft([x0 + 28.0, 4.0])], "size_in": [12.0, 8.0], "bottoms_ft": [floor_ft - 2.5]}));
        equipment.push(json!({"tag": format!("EF-{}", i + 1), "kind": "exhaust_fan", "level": name,
            "min": ft([x0 + 10.0, 10.0]), "max": ft([x0 + 11.0, 11.0]), "bottom_ft": floor_ft - 3.0, "height_ft": 1.0}));
    }
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": levels,
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"}],
        "wall_types": [{"name": "Exterior", "thickness_in": 6, "from": "pairs"}],
        "mep": {
            "systems": [{"name": "SUPPLY AIR", "layer": "M-SUPPLY", "kind": "duct", "size_in": [12.0, 8.0], "bottom_ft": floor_ft - 2.5, "duct_type": "supply"}],
            "runs": runs,
            "equipment": equipment
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// The built elements of `category` on `level`.
fn on_level(s: &Session, category: &str, level: &str) -> Vec<Handle> {
    let d = s.doc().unwrap();
    d.model.iter().filter(|e| bimlook::classify(d, e).is_some_and(|c| c.category.contains(category) && c.level == level)).map(|e| e.handle).collect()
}

fn element(s: &mut Session, h: Handle) -> Value {
    s.execute("bim.element", &json!({"handle": h.hex()})).unwrap()
}

/// The elements of `category` the model tree lists on `level`.
fn tree_count(s: &mut Session, level: &str, category: &str) -> u64 {
    let t = s.execute("bim.tree", &json!({"limit": 1000})).unwrap();
    let Some(lv) = t["levels"].as_array().unwrap().iter().find(|l| l["name"] == json!(level)) else { return 0 };
    lv["trades"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["categories"].as_array().unwrap().iter())
        .filter(|c| c["name"].as_str().is_some_and(|n| n.contains(category)))
        .map(|c| c["count"].as_u64().unwrap())
        .sum()
}

/// Copy the one selected element, then move the copy up `dz` feet: the copy is the new element.
fn copy_up(s: &mut Session, h: Handle, dz: f64) -> Handle {
    let before: Vec<Handle> = s.doc().unwrap().model.iter().map(|e| e.handle).collect();
    s.execute("copy", &json!({"handles": [h.hex()], "delta": [0, 0]})).unwrap();
    let copy = s.doc().unwrap().model.iter().map(|e| e.handle).find(|h| !before.contains(h)).expect("the copy");
    s.execute("3dmove", &json!({"handles": [copy.hex()], "delta": [0, 0, dz * 12.0]})).unwrap();
    copy
}

fn assert_placed(r: &Value, level: &str, at: [f64; 2], aff_in: f64) {
    assert_eq!(r["level"], json!(level), "{r}");
    let got = [r["at"][0].as_f64().unwrap(), r["at"][1].as_f64().unwrap()];
    assert!((got[0] - at[0]).abs() < 0.05 && (got[1] - at[1]).abs() < 0.05, "same place in the building: {got:?} vs {at:?}: {r}");
    assert!((r["aff_in"].as_f64().unwrap() - aff_in).abs() < 0.5, "same height above its own floor: {r}");
}

#[test]
fn a_copy_moved_up_a_storey_is_on_that_storey() {
    // Plans over one another, and plans side by side (each level's plan 100' east of the one
    // below): the copy keeps its place in the building either way.
    for east in [0.0, 100.0] {
        let mut s = building(2, 10.0, east);
        let fan = *on_level(&s, "Equipment", "Level 1").first().expect("the level 1 exhaust fan");
        let orig = element(&mut s, fan);
        let at = [orig["at"][0].as_f64().unwrap(), orig["at"][1].as_f64().unwrap()];
        let aff = orig["aff_in"].as_f64().unwrap();
        let cat = orig["category"].as_str().unwrap().to_string();
        let on_2 = tree_count(&mut s, "Level 2", &cat);
        let copy = copy_up(&mut s, fan, 10.0);
        let r = element(&mut s, copy);
        assert_placed(&r, "Level 2", at, aff);
        assert_eq!(tree_count(&mut s, "Level 2", &cat), on_2 + 1, "the model tree files the copy under level 2 (east {east})");
        // The original stays where it was.
        assert_eq!(element(&mut s, fan)["level"], json!("Level 1"));
        // Undo takes the move back and the copy is on level 1 again; redo files it on level 2.
        s.execute("undo", &json!({})).unwrap();
        assert_eq!(element(&mut s, copy)["level"], json!("Level 1"));
        s.execute("redo", &json!({})).unwrap();
        assert_eq!(element(&mut s, copy)["level"], json!("Level 2"));
    }
}

/// The duct elements `bim.takeoff` by level counts on `level`.
fn takeoff_ducts(s: &mut Session, level: &str) -> u64 {
    let t = s.execute("bim.takeoff", &json!({"group_by": "level", "keep": false})).unwrap();
    t["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["level"] == json!(level) && l["category"].as_str().is_some_and(|c| c.to_ascii_lowercase().contains("duct")))
        .map(|l| l["element_count"].as_u64().unwrap_or(0))
        .sum()
}

#[test]
fn a_run_moved_two_storeys_is_on_the_storey_it_reached_and_takeoff_follows() {
    let mut s = building(3, 10.0, 0.0);
    let duct = *on_level(&s, "Duct", "Level 1").first().expect("a level 1 duct");
    let q = |s: &mut Session, level: &str| -> usize { on_level(s, "Duct", level).len() };
    let (n1, n3) = (q(&mut s, "Level 1"), q(&mut s, "Level 3"));
    let (t1, t3) = (takeoff_ducts(&mut s, "Level 1"), takeoff_ducts(&mut s, "Level 3"));
    assert!(t1 > 0 && t3 > 0, "{t1} {t3}");
    s.execute("3dmove", &json!({"handles": [duct.hex()], "delta": [0, 0, 240.0]})).unwrap();
    assert_eq!(element(&mut s, duct)["level"], json!("Level 3"));
    assert_eq!((q(&mut s, "Level 1"), q(&mut s, "Level 3")), (n1 - 1, n3 + 1));
    assert_eq!(
        (takeoff_ducts(&mut s, "Level 1"), takeoff_ducts(&mut s, "Level 3")),
        (0, t3 + t1),
        "takeoff by level: the level's one run, with its pieces, is on level 3 now"
    );
    // A small move within the storey leaves it there; one below the lowest floor stays on the
    // lowest level.
    s.execute("3dmove", &json!({"handles": [duct.hex()], "delta": [0, 0, -18.0]})).unwrap();
    assert_eq!(element(&mut s, duct)["level"], json!("Level 3"));
    s.execute("3dmove", &json!({"handles": [duct.hex()], "delta": [0, 0, -12.0 * 40.0]})).unwrap();
    assert_eq!(element(&mut s, duct)["level"], json!("Level 1"));
}

#[test]
fn a_floor_slab_copied_up_one_storey_lands_one_storey_up() {
    // A slab's bottom is under its own level's floor: it moves by storeys, not by where its
    // bottom falls.
    let mut s = building(3, 10.0, 100.0);
    let slab = *on_level(&s, "Floors", "Level 2").first().expect("a level 2 floor");
    assert!(element(&mut s, slab)["aff_in"].as_f64().unwrap() <= 0.0, "a slab's bottom is at or under its floor");
    let copy = copy_up(&mut s, slab, 10.0);
    assert_eq!(element(&mut s, copy)["level"], json!("Level 3"));
}

#[test]
fn a_command_moves_elements_to_a_level_by_name() {
    for east in [0.0, 100.0] {
        let mut s = building(2, 10.0, east);
        let duct = *on_level(&s, "Duct", "Level 1").first().unwrap();
        let orig = element(&mut s, duct);
        let at = [orig["at"][0].as_f64().unwrap(), orig["at"][1].as_f64().unwrap()];
        let r = s.execute("bim.level.move", &json!({"handles": [duct.hex()], "level": "level 2"})).unwrap();
        assert_eq!(r["moved"], json!(1), "{r}");
        let e = element(&mut s, duct);
        assert_placed(&e, "Level 2", at, orig["aff_in"].as_f64().unwrap());
        assert!((e["bottom_ft"].as_f64().unwrap() - orig["bottom_ft"].as_f64().unwrap() - 10.0).abs() < 0.01, "{e}");
        // Already there: nothing moves. No such level, or not a built element: an error.
        let r = s.execute("bim.level.move", &json!({"handles": [duct.hex()], "level": "Level 2"})).unwrap();
        assert_eq!(r["moved"], json!(0), "{r}");
        assert!(s.execute("bim.level.move", &json!({"handles": [duct.hex()], "level": "Level 9"})).is_err());
        assert!(s.execute("bim.level.move", &json!({"handles": ["FFFFFF"], "level": "Level 1"})).is_err());
        assert!(s.execute("bim.level.move", &json!({"level": "Level 1"})).is_err(), "nothing selected");
    }
}

#[test]
fn a_stored_level_that_disagrees_is_flagged() {
    let mut s = building(2, 10.0, 0.0);
    let duct = *on_level(&s, "Duct", "Level 1").first().unwrap();
    s.execute("bim.props.set", &json!({"handle": duct.hex(), "key": "level", "value": "Level 2"})).unwrap();
    let r = element(&mut s, duct);
    assert_eq!(r["level"], json!("Level 1"), "storing a value doesn't move it: {r}");
    let flag = &r["level_conflict"];
    assert_eq!((flag["stored"].clone(), flag["level"].clone()), (json!("Level 2"), json!("Level 1")), "{r}");
    assert!(flag["fix"].as_str().is_some_and(|f| f.contains("bim.level.move")), "{r}");
    // Agreeing: no flag.
    s.execute("bim.props.set", &json!({"handle": duct.hex(), "key": "level", "value": "level 1"})).unwrap();
    assert!(element(&mut s, duct).get("level_conflict").is_none());
}

#[test]
fn hostile_params_never_panic() {
    let mut s = building(2, 10.0, 0.0);
    for p in [
        json!({}),
        json!({"level": 3}),
        json!({"handles": "x", "level": "Level 1"}),
        json!({"handles": [1, null], "level": "Level 1"}),
        json!({"handle": "zz", "level": ""}),
    ] {
        let _ = s.execute("bim.level.move", &p);
    }
    // A move by a huge or non-finite amount never panics either.
    let duct = *on_level(&s, "Duct", "Level 1").first().unwrap();
    let _ = s.execute("3dmove", &json!({"handles": [duct.hex()], "delta": [0, 0, 1e300]}));
}

#[test]
fn a_move_override_up_a_storey_is_filed_there_after_every_build() {
    let mut s = building(2, 10.0, 0.0);
    let fan = *on_level(&s, "Equipment", "Level 1").first().unwrap();
    s.execute("override.add", &json!({"handle": fan.hex(), "kind": "move", "delta": [0, 0, 120.0]})).unwrap();
    let on = |s: &Session| on_level(s, "Equipment", "Level 2").len();
    assert_eq!((on_level(&s, "Equipment", "Level 1").len(), on(&s)), (0, 2), "moved up a storey by the override");
    // Rebuilt from the plan: the override is applied again and files it on level 2 again.
    s.execute("model.build", &json!({})).unwrap();
    assert_eq!((on_level(&s, "Equipment", "Level 1").len(), on(&s)), (0, 2), "after the rebuild");
}
