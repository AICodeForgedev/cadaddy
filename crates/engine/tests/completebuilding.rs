//! Q6216 completebuilding: every built object says what it is by the documents (drawn, derived
//! from a matching source, or inferred by a rule) with its basis; filled-in work adds to the model
//! and its counts without changing what the documents draw; the BIM view shows it apart. Made-up
//! plans only.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook::{self, INFERRED_RGB};
use cadaddy_engine::doc::color::Color;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// One made-up room, 6" walls: 0.5–39.5' by 0.5–19.5'.
fn room() -> Drawing {
    let mut d = Drawing::new_imperial();
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let sides = [(0.0, 0.0), (40.0, 0.0), (40.0, 20.0), (0.0, 20.0)];
    let inner = [(0.5, 0.5), (39.5, 0.5), (39.5, 19.5), (0.5, 19.5)];
    for ring in [sides, inner] {
        for i in 0..4 {
            let h = Handle(d.handseed.max(0x100));
            d.handseed = h.0 + 1;
            let mut e = Entity::new(h, EntityKind::Line(Line { a: p(ring[i]), b: p(ring[(i + 1) % 4]) }));
            e.common.layer = "A-WALL".into();
            d.model.push(e);
        }
    }
    d
}

fn system(name: &str, kind: &str) -> Value {
    json!({"name": name, "layer": name, "kind": kind, "size_in": [2, 0], "bottom_ft": 9})
}

fn run(system: &str, points: Value, made: Option<(&str, &str)>) -> Value {
    let mut r = json!({"system": system, "level": "L1", "points": points, "size_in": [1.5, 0], "bottoms_ft": [9.0], "height_source": "tag", "size_source": "tag"});
    if let Some((m, basis)) = made {
        r["made"] = json!(m);
        r["basis"] = json!(basis);
    }
    r
}

/// The room built with a water closet, its drawn waste, vent and cold water; `filled`: also a
/// hot water branch a rule inferred and a supply duct copied (derived) from another floor.
fn built(filled: bool) -> Session {
    let mut s = Session::empty();
    s.open_drawing(room(), "Made-up Room", None);
    let fixture = json!({
        "id": "device:L1:10.0,5.0:WC-9", "kind": "plumbing_fixture", "level": "L1", "at_ft": [10.0, 5.0],
        "fields": {
            "type": {"value": "WC-9", "source": "plan"},
            "fixture": {"value": "WATER CLOSET", "source": "schedule"},
            "device_type": {"value": "WATER CLOSET", "source": "schedule"},
            "sheet": {"value": "P-91", "source": "plan"},
            "symbol": {"value": "1F8", "source": "plan"},
        },
    });
    let mut runs = vec![
        run("SAN", json!([[96.0, 48.0], [96.0, 240.0]]), None),
        run("V", json!([[132.0, 48.0], [132.0, 240.0]]), None),
        run("CW", json!([[60.0, 84.0], [480.0, 84.0]]), None),
    ];
    if filled {
        runs.push(run("HW", json!([[60.0, 96.0], [300.0, 96.0]]), Some(("inferred", "P-BRANCH-WATER: trade practice: a branch teed off the main (made-up)"))));
        runs.push(run("SA", json!([[24.0, 200.0], [400.0, 200.0]]), Some(("derived", "derived from L2's drawn duct where the rooms stack (made-up)"))));
    }
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": [fixture],
        "mep": {"systems": [system("SAN", "pipe"), system("V", "pipe"), system("CW", "pipe"), system("HW", "pipe"), system("SA", "duct")], "runs": runs},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

/// The build tags of an element (its `CADADDY_WALLGEN` group), `None` when it is not built.
fn build_tags(e: &Entity) -> Option<Vec<String>> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case("CADADDY_WALLGEN"))?;
    Some(x.iter().skip(at + 1).take_while(|t| t.code != 1001).map(|t| t.value.clone()).collect())
}

fn tag<'a>(tags: &'a [String], key: &str) -> Option<&'a str> {
    tags.iter().find_map(|t| t.strip_prefix(&format!("{key}:")))
}

#[test]
fn every_built_element_carries_its_provenance_and_nothing_filled_in_reads_drawn() {
    let mut s = built(true);
    let d = s.doc().expect("doc");
    let (mut n, mut inferred, mut derived) = (0, 0, 0);
    for e in d.model.iter() {
        let Some(tags) = build_tags(e) else { continue };
        n += 1;
        let status = tag(&tags, "status").unwrap_or_else(|| panic!("every built element carries a status: {tags:?}"));
        assert!(["drawn", "derived", "inferred"].contains(&status), "{status}");
        if status != "drawn" {
            assert!(tag(&tags, "basis").is_some_and(|b| b.len() > 10), "a filled-in element says why: {tags:?}");
        }
        // Filled-in work is on its own layer, and that layer holds no drawn work.
        let layer = e.common.layer.to_ascii_uppercase();
        if layer.contains("INFERRED") {
            assert_eq!(status, "inferred", "{layer} {tags:?}");
            inferred += 1;
        }
        if layer.contains("DERIVED") {
            assert_eq!(status, "derived", "{layer} {tags:?}");
            derived += 1;
        }
    }
    assert!(n > 10 && inferred > 0 && derived > 0, "{n} {inferred} {derived}");
    let p = s.execute("bim.provenance", &json!({"list": 5, "connections": true})).expect("provenance");
    assert_eq!(p["elements"], p["marked"], "100% carry the mark: {p}");
    assert_eq!(p["inferred_without_basis"], json!(0), "{p}");
    assert!(p["inferred"].as_u64().unwrap_or(0) >= inferred as u64, "{p}");
    assert!(p["derived_with_basis"].as_u64().unwrap_or(0) >= derived as u64, "{p}");
    // Asked for inferred work only, nothing drawn comes back.
    let only = s.execute("bim.provenance", &json!({"made": "inferred", "list": 50})).expect("inferred");
    assert!(only["listed"].as_array().is_some_and(|l| !l.is_empty() && l.iter().all(|x| x["made"] == "inferred")), "{only}");
    assert_eq!(only["drawn"], json!(0));
    // The fixture is connected to its lines.
    assert_eq!(p["connections"]["plumbing"]["fixtures"], json!(1), "{p}");
    assert_eq!(p["connections"]["plumbing"]["connected"], json!(1), "{p}");
}

#[test]
fn filling_in_adds_work_and_leaves_what_the_documents_draw_alone() {
    let mut a = built(false);
    let mut b = built(true);
    let pa = a.execute("bim.provenance", &json!({})).expect("a");
    let pb = b.execute("bim.provenance", &json!({})).expect("b");
    // The drawn runs' feet are the same; the filled-in ones are added beside them.
    for sys in ["pipe SAN", "pipe V", "pipe CW"] {
        assert_eq!(pa["runs"]["by_system"][sys]["drawn"], pb["runs"]["by_system"][sys]["drawn"], "{sys}: {pa} {pb}");
    }
    assert!(pa["runs"]["by_system"].get("pipe HW").is_none());
    assert_eq!(pb["runs"]["by_system"]["pipe HW"]["inferred"]["ft"], json!(20.0), "{pb}");
    assert_eq!(pb["runs"]["by_system"]["duct SA"]["derived"]["ft"], json!(31.3), "{pb}");
    // The drawn elements are the same in number; the filled-in ones come on top.
    assert_eq!(pa["drawn"], pb["drawn"], "{pa} {pb}");
    assert_eq!(pa["inferred"], json!(0));
    assert!(pb["inferred"].as_u64().unwrap_or(0) > 0 && pb["derived"].as_u64().unwrap_or(0) > 0, "{pb}");
    // The takeoff counts filled-in work, each row apart from the drawn.
    let q = b.execute("mep.qty", &json!({})).expect("qty");
    let rows: Vec<String> = q["pipes"].as_array().expect("pipes").iter().filter_map(|r| r["system"].as_str().map(str::to_string)).collect();
    assert!(rows.contains(&"CW".to_string()) && rows.contains(&"HW (inferred)".to_string()), "{rows:?}");
    let ducts: Vec<String> = q["ducts"].as_array().expect("ducts").iter().filter_map(|r| r["system"].as_str().map(str::to_string)).collect();
    assert!(ducts.contains(&"SA (derived)".to_string()), "{ducts:?}");
}

#[test]
fn the_bim_view_shows_filled_in_work_apart_from_drawn() {
    let mut s = built(true);
    let v = s.execute("bim.look", &json!({"apart": true})).expect("look");
    assert_eq!(v["apart"], json!(true), "{v}");
    assert!(v["made"]["inferred"].as_u64().unwrap_or(0) > 0, "{v}");
    let d = s.doc().expect("doc").clone();
    let shown = bimlook::display(&d).expect("the BIM view");
    let rgb = |e: &Entity| match e.common.color {
        Color::True(c) => Some([c.0, c.1, c.2]),
        _ => None,
    };
    let (mut apart, mut drawn_apart) = (0, 0);
    for e in shown.model.iter() {
        let Some(tags) = build_tags(e) else { continue };
        let inferred = tag(&tags, "status") == Some("inferred");
        if inferred {
            assert_eq!(rgb(e), Some(INFERRED_RGB), "inferred work in its own colour");
            apart += 1;
        } else if rgb(e) == Some(INFERRED_RGB) {
            drawn_apart += 1;
        }
    }
    assert!(apart > 0);
    assert_eq!(drawn_apart, 0, "nothing else looks inferred");
    // Off again: the trades' colours.
    s.execute("bim.look", &json!({"apart": false})).expect("off");
    let shown = bimlook::display(s.doc().expect("doc")).expect("view");
    assert!(shown.model.iter().all(|e| rgb(e) != Some(INFERRED_RGB)));
}
