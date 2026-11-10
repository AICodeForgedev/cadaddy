//! The BIM view's four views (Q6419): Architectural, MEP (mains and big gear, or full), Floor
//! and Room, one at a time, none drawing every object; what each draws comes from the model's
//! own data (feeders, panel schedules, systems, the levels' floors), then sizes; every switch
//! and branch turned on stays inside the view; the takeoff, queries, clashes and exports are the
//! same in every view. A made-up building (`sample::bim_building`).

use std::collections::BTreeSet;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, Handle};
use serde_json::{Value, json};

fn building(levels: usize, rooms: usize) -> Session {
    let mut s = Session::new();
    s.open_drawing(cadaddy_engine::sample::bim_building(levels, rooms), "Sample Building", None);
    s
}

fn view(s: &mut Session, p: Value) -> Value {
    s.execute("bim.view", &p).unwrap_or_else(|e| panic!("bim.view {p}: {e}"))
}

fn shown(s: &Session) -> Drawing {
    bimlook::display(s.doc().unwrap()).expect("the BIM view of a built model")
}

/// What a drawn object is, as the tests name it: `part`, `mep kind part system tag size`.
fn label(e: &Entity) -> String {
    let tag = |k: &str| bimlook::made_tag(e, k).unwrap_or("").to_string();
    match made_as(e).as_deref() {
        Some("mep") => format!("mep {} {} {} {} {}", tag("kind"), tag("part"), tag("system"), tag("tag"), tag("size")),
        Some(p) => p.to_string(),
        None => format!("context {}", e.common.layer),
    }
}

fn labels(d: &Drawing) -> Vec<String> {
    d.model.iter().map(|e| label(e)).collect()
}

fn has(ls: &[String], f: impl Fn(&str) -> bool) -> bool {
    ls.iter().any(|l| f(l))
}

fn count(ls: &[String], f: impl Fn(&str) -> bool) -> usize {
    ls.iter().filter(|l| f(l)).count()
}

/// Every built element of the drawing (handles).
fn built(d: &Drawing) -> BTreeSet<Handle> {
    d.model.iter().filter(|e| bimlook::classify(d, e).is_some()).map(|e| e.handle).collect()
}

/// The built elements the view draws (handles of the drawing's, context the view adds left out).
fn drawn(s: &Session) -> BTreeSet<Handle> {
    let all = built(s.doc().unwrap());
    shown(s).model.iter().map(|e| e.handle).filter(|h| all.contains(h)).collect()
}

/// Every switch and every tree branch on, Full MEP on, every fidelity tier: the most a user can
/// turn on.
fn everything_on(s: &mut Session) {
    view(s, json!({"switches": {"m": true, "e": true, "p": true, "cabinets": true, "furniture": true}}));
    s.execute("bim.tree", &json!({"path": ["*"], "visible": true})).unwrap();
    s.execute("view.fidelity", &json!({"level": "detail"})).unwrap();
}

#[test]
fn a_model_opens_in_mep_mains_and_big_gear() {
    let mut s = building(3, 6);
    let r = view(&mut s, json!({}));
    assert_eq!(r["view"], "mep");
    assert_eq!(r["full"], false);
    assert_eq!(r["breadcrumb"], json!(["Building"]));
    assert!(r["drawn"].as_u64().unwrap() < r["total"].as_u64().unwrap() / 4, "{r}");
    let ls = labels(&shown(&s));
    // Context: the floors, one outline, the hoistway; no other architecture.
    assert!(has(&ls, |l| l == "floor"));
    assert!(has(&ls, |l| l == "outline"), "the building's outline");
    assert!(!has(&ls, |l| l.starts_with("context")), "no sheet linework");
    assert!(has(&ls, |l| l == "hoistway"), "the elevator's hoistway");
    for p in ["wall", "door", "window", "stair", "ceiling", "room", "column", "casework", "furniture", "atom", "device", "light"] {
        assert!(!has(&ls, |l| l == p), "{p} in the MEP view");
    }
    // Mains and big gear.
    for want in [
        "mep equipment equipment:switchboard  SWB-1 ",
        "mep equipment equipment:panel  H2 ",
        "mep equipment equipment:elevator  ELEV-1 ",
        "mep equipment equipment:disconnect  ELEV-1 DISC ",
        "mep equipment equipment:doas  DOAS-1 ",
        "mep equipment equipment:fire_pump  FP-1 ",
        "mep conduit segment F-ELEV  1.5",
        "mep duct segment SA  24x12",
        "mep pipe segment SAN  4",
        "mep pipe segment FIRE MAIN  4",
        "mep pipe segment FIRE STANDPIPE  4",
        "mep pipe segment CW  3",
        "mep tray segment TRAY  12x4",
    ] {
        assert!(has(&ls, |l| l == want), "{want} not drawn: {:?}", ls.iter().filter(|l| l.starts_with("mep")).collect::<BTreeSet<_>>());
    }
    // House feeders up their riser; an unknown run 1" and up is a feeder.
    assert!(has(&ls, |l| l == "mep conduit segment E-FEEDER  2"));
    assert!(has(&ls, |l| l == "mep conduit segment C-1  1.25"));
    // Not at this density: unit panels and their feeders, branch ducts, fans, small pipe.
    for not in [
        "equipment:panel  U ",
        "equipment:exhaust_fan",
        "SA  8x6",
        "EA  6",
        "terminal:",
        "CW  2",
        "CW  0.75",
        "E-FEEDER  0.75",
        "SPRINKLER",
        "E-BRANCH",
        "C-2",
        "atom:",
    ] {
        assert!(!has(&ls, |l| l.contains(not)), "{not} drawn at mains density");
    }
}

#[test]
fn full_mep_adds_distribution_and_what_crosses_floors_never_branches() {
    let mut s = building(3, 6);
    view(&mut s, json!({"full": true}));
    let ls = labels(&shown(&s));
    // Every duct, terminal and exhaust fan with its riser; every panel and its feeder.
    for want in ["SA  8x6", "EA  6", "terminal:diffuser", "equipment:exhaust_fan", "equipment:panel  U ", "E-FEEDER  0.75", "CW  2"] {
        assert!(has(&ls, |l| l.contains(want)), "{want} not in Full MEP");
    }
    // A branch circuit through a floor shows; the elevator's traveling cable doesn't, though it
    // crosses every floor; a branch circuit's pieces, wire, boxes and devices never.
    assert_eq!(count(&ls, |l| l == "mep conduit segment E-BRANCH  0.75"), 2, "the branch stubs through the two floors");
    assert!(!has(&ls, |l| l.starts_with("mep cable segment E-BRANCH")), "the elevator's traveling cable");
    for not in ["atom:", "terminal:head", "SPRINKLER BRANCH", "CW  0.75", "C-2  0.5"] {
        assert!(!has(&ls, |l| l.contains(not)), "{not} in Full MEP");
    }
    for p in ["device", "light", "wall", "door", "window", "furniture", "casework"] {
        assert!(!has(&ls, |l| l == p), "{p} in Full MEP");
    }
    // The CW line with no size is left out, and said.
    assert!(!has(&ls, |l| l == "mep pipe segment CW  "));
    let r = view(&mut s, json!({}));
    assert!(r["left_out"]["pipe_unknown_size"].as_u64().unwrap() >= 3, "{r}");
    // Mains is inside Full.
    let full = drawn(&s);
    view(&mut s, json!({"full": false}));
    let mains = drawn(&s);
    assert!(mains.is_subset(&full) && mains.len() < full.len());
}

#[test]
fn architectural_is_the_building_alone() {
    let mut s = building(2, 4);
    view(&mut s, json!({"view": "architectural"}));
    let d = shown(&s);
    let ls = labels(&d);
    for p in ["wall", "door", "window", "floor", "room", "stair", "ceiling"] {
        assert!(has(&ls, |l| l == p), "{p} not drawn");
    }
    assert!(!has(&ls, |l| l.starts_with("mep")
        || l == "atom"
        || l == "casework"
        || l == "furniture"
        || l == "column"
        || l == "device"
        || l == "light"));
    // Solid: nothing see-through.
    assert!(!d.model.iter().any(|e| e.common.material.as_deref() == Some(bimlook::GHOST_MATERIAL)));
}

#[test]
fn floor_and_room_views_one_level_one_room() {
    let mut s = building(3, 6);
    let r = view(&mut s, json!({"level": "L2"}));
    assert_eq!((r["view"].as_str(), r["level"].as_str()), (Some("floor"), Some("L2")));
    assert_eq!(r["rooms"].as_array().unwrap().len(), 6);
    let d = shown(&s);
    assert!(d.model.iter().filter(|e| made_as(e).is_some()).all(|e| cadaddy_engine::cmd::bspec::made_level(e).as_deref() == Some("L2")), "only L2");
    let ls = labels(&d);
    for want in ["wall", "door", "window", "room", "light", "casework"] {
        assert!(has(&ls, |l| l == want), "{want} not on the floor");
    }
    for want in ["equipment:panel  U ", "equipment:panel  H2 ", "SPRINKLER BRANCH", "terminal:head", "CW  0.75", "SA  8x6"] {
        assert!(has(&ls, |l| l.contains(want)), "{want} not on the floor");
    }
    for not in ["device", "furniture", "atom"] {
        assert!(!has(&ls, |l| l == not), "{not} on the floor");
    }
    assert!(!has(&ls, |l| l.contains("atom:")), "branch pieces belong to the Room view");
    // Cabinets switch off.
    view(&mut s, json!({"switches": {"cabinets": false}}));
    assert!(!has(&labels(&shown(&s)), |l| l == "casework"));
    view(&mut s, json!({"switches": {"cabinets": true}}));
    // The Room view: everything in the room, the rest of the floor's architecture ghosted.
    let r = view(&mut s, json!({"room": "203"}));
    assert_eq!(r["view"], "room");
    assert_eq!(r["breadcrumb"], json!(["Building", "L2", "203"]));
    assert_eq!(r["offered"], json!(["m", "e", "p", "cabinets", "furniture"]));
    let d = shown(&s);
    let ls = labels(&d);
    assert_eq!(count(&ls, |l| l == "device"), 5, "its four receptacles and its lavatory: {ls:?}");
    for want in ["atom:conduit", "atom:coupling", "atom:strap", "atom:wire", "atom:box", "terminal:head", "equipment:panel  U "] {
        assert!(has(&ls, |l| l.contains(want)), "{want} not in the room");
    }
    assert_eq!(count(&ls, |l| l.contains("equipment:panel  U ")), 1, "its own unit panel only");
    assert!(!has(&ls, |l| l == "furniture"), "furniture is off by default");
    let ghosts =
        d.model.iter().filter(|e| e.common.material.as_deref() == Some(bimlook::GHOST_MATERIAL) && made_as(e).as_deref() == Some("wall")).count();
    assert!(ghosts > 4, "the surroundings ghosted: {ghosts}");
    view(&mut s, json!({"switches": {"furniture": true}}));
    assert_eq!(count(&labels(&shown(&s)), |l| l == "furniture"), 2);
    // Up: the floor, then the whole building's view.
    assert_eq!(view(&mut s, json!({"up": true}))["view"], "floor");
    let r = view(&mut s, json!({"up": true}));
    assert_eq!((r["view"].as_str(), r["level"].as_str()), (Some("mep"), None));
    // Picking enters: an element's level, then its room.
    let d = s.doc().unwrap();
    let rec = d
        .model
        .iter()
        .find(|e| made_as(e).as_deref() == Some("device") && cadaddy_engine::cmd::bspec::made_level(e).as_deref() == Some("L3"))
        .map(|e| e.handle.hex())
        .unwrap();
    assert_eq!(view(&mut s, json!({"enter": rec}))["level"], "L3");
    let r = view(&mut s, json!({"enter": rec}));
    assert_eq!((r["view"].as_str(), r["room"].as_str()), (Some("room"), Some("301")));
    // bim.look's one level is the Floor view of it.
    s.execute("bim.look", &json!({"level": "L1"})).unwrap();
    assert_eq!(view(&mut s, json!({}))["view"], "floor");
    s.execute("bim.look", &json!({"level": null})).unwrap();
    assert_eq!(view(&mut s, json!({}))["view"], "mep");
    assert!(s.execute("bim.view", &json!({"view": "street"})).is_err());
    assert!(s.execute("bim.view", &json!({"level": "L9"})).is_err());
    assert!(s.execute("bim.view", &json!({"switches": {"x": true}})).is_err());
}

#[test]
fn nothing_gets_past_the_active_view() {
    let mut s = building(3, 6);
    let all = built(s.doc().unwrap());
    let per = |s: &mut Session, p: Value| {
        view(s, p);
        let before = drawn(s);
        everything_on(s);
        let after = drawn(s);
        s.execute("view.fidelity", &json!({"level": "auto"})).unwrap();
        (before, after)
    };
    for p in [
        json!({"view": "architectural"}),
        json!({"view": "mep", "full": false}),
        json!({"view": "mep", "full": true}),
        json!({"level": "L2"}),
        json!({"room": "202"}),
    ] {
        let (before, after) = per(&mut s, p.clone());
        assert!(!after.is_empty(), "{p}");
        assert!(after.len() < all.len(), "{p}: every object drawn");
        // The switches add only what the view allows (cabinets, furniture in a room).
        let r = view(&mut s, json!({}));
        let room = r["view"] == "room";
        let extra: Vec<String> = after.difference(&before).filter_map(|h| s.doc().unwrap().model.get(*h)).map(|e| label(e)).collect();
        assert!(extra.iter().all(|l| room && (l == "furniture" || l == "casework")), "{p}: {extra:?}");
    }
    // A file with no saved view (an older version's, with one level shown) opens in MEP.
    let mut s = building(2, 2);
    let mut spec: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    spec["bim_view"] = json!({"level": "L2", "hidden_paths": []});
    s.doc_mut().unwrap().building_spec = Some(spec.to_string()).into();
    let r = view(&mut s, json!({}));
    assert_eq!(r["view"], "mep");
    assert!(!has(&labels(&shown(&s)), |l| l == "wall"));
}

#[test]
fn panels_are_house_or_unit_by_the_distribution() {
    let s = building(2, 4);
    // Rename the unit panels' tag: the data, not the name, says they serve one room each.
    let d = s.doc().unwrap();
    let text = d.building_spec.as_deref().unwrap().replace("\"tag\":\"U\"", "\"tag\":\"PNL-Q\"").replace("\"to\":\"U\"", "\"to\":\"PNL-Q\"");
    let mut d = d.clone();
    d.building_spec = Some(text).into();
    let list: Vec<Entity> = d
        .model
        .iter()
        .filter(|e| bimlook::made_tag(e, "tag") == Some("U"))
        .map(|e| {
            let mut c = (**e).clone();
            for t in c.common.xdata.iter_mut() {
                if t.value == "tag:U" {
                    t.value = "tag:PNL-Q".into();
                }
            }
            c
        })
        .collect();
    for c in list {
        d.model.push(c);
    }
    let mut s2 = Session::new();
    s2.open_drawing(d, "Sample Building", None);
    let ls = labels(&shown(&s2));
    assert!(!has(&ls, |l| l.contains("PNL-Q")), "unit panels are not mains");
    assert!(has(&ls, |l| l.contains("equipment:panel  H1 ")), "house panels are");
    view(&mut s2, json!({"full": true}));
    assert!(has(&labels(&shown(&s2)), |l| l.contains("PNL-Q")));
}

#[test]
fn follow_the_system_across_levels() {
    let mut s = building(3, 4);
    let d = s.doc().unwrap();
    let h2 = d.model.iter().find(|e| bimlook::made_tag(e, "tag") == Some("H2")).map(|e| e.handle.hex()).unwrap();
    let r = view(&mut s, json!({"follow": h2}));
    assert!(r["follow"]["elements"].as_u64().unwrap() > 4, "{r}");
    let d = shown(&s);
    let solid: Vec<String> = d
        .model
        .iter()
        .filter(|e| e.common.material.as_deref() != Some(bimlook::GHOST_MATERIAL))
        .map(|e| format!("{} @{}", label(e), cadaddy_engine::cmd::bspec::made_level(e).unwrap_or_default()))
        .collect();
    // What feeds it (the switchboard on L1) and what it feeds (L2's unit panels and circuits).
    assert!(has(&solid, |l| l.contains("SWB-1")), "{solid:?}");
    assert!(has(&solid, |l| l.contains("equipment:panel  U ") && l.ends_with("@L2")));
    assert!(has(&solid, |l| l.contains("atom:conduit") && l.ends_with("@L2")), "its circuits, drawn though the view is MEP");
    assert!(!has(&solid, |l| l.contains("equipment:panel  U ") && l.ends_with("@L3")), "another level's unit panels are not on it");
    assert!(!has(&solid, |l| l.contains("duct")), "the rest faded");
    // The followed set is never every object.
    let all = built(s.doc().unwrap());
    assert!(drawn(&s).len() < all.len());
    assert!(view(&mut s, json!({"follow": null})).get("follow").is_none());
}

/// The takeoff, queries, clashes, piece counts and the AR export read the model, never a view.
#[test]
fn counts_are_the_same_in_every_view() {
    let mut s = building(2, 4);
    let dir = crate::helper::scratch("bimviews-ar");
    let strip = |mut v: Value| -> Value {
        fn walk(v: &mut Value) {
            match v {
                Value::Object(o) => {
                    o.retain(|k, _| !(k.ends_with("_ms") || k == "ms" || k.contains("path") || k.contains("file") || k == "written"));
                    o.values_mut().for_each(walk);
                }
                Value::Array(a) => a.iter_mut().for_each(walk),
                _ => {}
            }
        }
        walk(&mut v);
        v
    };
    let run = std::cell::Cell::new(0);
    let counts = |s: &mut Session| {
        // Each export in a folder of its own (an export never overwrites).
        run.set(run.get() + 1);
        let out = dir.join(format!("v{}", run.get()));
        std::fs::create_dir_all(&out).unwrap();
        let pipes = s.execute("bim.props.find", &json!({"kind": "pipe", "limit": 1})).unwrap()["count"].clone();
        let devices = s.execute("bim.props.find", &json!({"kind": "device", "limit": 1})).unwrap()["count"].clone();
        let take = s.execute("bim.takeoff", &json!({})).unwrap();
        let clash = s.execute("bim.clash", &json!({})).map(|c| c["total"].clone()).unwrap_or(Value::Null);
        let pieces = s.execute("bim.count", &json!({"kind": "coupling"})).map(|c| c["count"].clone()).unwrap_or(Value::Null);
        let ar = s
            .execute("bim.export.ar", &json!({"room": "201", "level": "L2", "path": out.join("room").to_string_lossy()}))
            .map(strip)
            .unwrap_or(Value::Null);
        json!({"pipes": pipes, "devices": devices, "csv": take["csv"], "totals": take["totals"], "clash": clash, "pieces": pieces, "ar": ar})
    };
    let before = counts(&mut s);
    assert!(before["pipes"].as_u64().unwrap() > 10, "{before}");
    assert!(before["ar"].is_object() && before["clash"].is_number(), "{before}");
    for p in [
        json!({"view": "architectural"}),
        json!({"view": "mep", "full": true}),
        json!({"level": "L2"}),
        json!({"room": "202"}),
        json!({"view": "mep", "full": false}),
    ] {
        view(&mut s, p.clone());
        assert_eq!(counts(&mut s), before, "{p}");
    }
}

#[test]
fn rules_are_project_data() {
    let mut s = building(2, 2);
    view(&mut s, json!({}));
    assert!(!has(&labels(&shown(&s)), |l| l == "mep pipe segment CW  2"));
    // This project calls 2" plumbing large pipe: it draws at mains density.
    let r = s.execute("bim.view.rules", &json!({"rules": {"large_pipe_min_in": 2}})).unwrap();
    assert_eq!(r["source"], "project");
    assert!(has(&labels(&shown(&s)), |l| l == "mep pipe segment CW  2"));
    assert!(s.execute("bim.view.rules", &json!({"rules": {"no_such_rule": 1}})).is_err());
    assert!(s.execute("bim.view.rules", &json!({"rules": {"large_pipe_min_in": "big"}})).is_err());
    assert_eq!(s.execute("bim.view.rules", &json!({"reset": true})).unwrap()["source"], "default");
}

#[test]
fn report_measures_each_view() {
    let mut s = building(3, 6);
    let r = s.execute("bim.view.report", &json!({"width": 320, "height": 200})).unwrap();
    let total = r["total"].as_u64().unwrap();
    for v in ["architectural", "mep", "mep_full", "floor", "room"] {
        let x = &r["views"][v];
        let n = x["drawn"].as_u64().unwrap();
        assert!(n > 0 && n < total, "{v}: {x}");
        assert!(x["all_switches_drawn"].as_u64().unwrap() < total, "{v}: {x}");
        assert!(x["tris"].as_u64().unwrap() > 0 && x["gpu_mb_est"].as_f64().is_some(), "{v}: {x}");
    }
    let n = |v: &str| r["views"][v]["drawn"].as_u64().unwrap();
    assert!(n("mep") < n("mep_full") && n("room") < n("floor"), "{r}");
}
