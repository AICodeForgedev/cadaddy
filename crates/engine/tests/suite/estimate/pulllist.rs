//! Q6195 pulllist: the field paperwork from the model's pieces — the wire pull schedule (pull
//! points, slack, reels), the conduit cut and bend list (equal to the takeoff), the release lists
//! with their price book lines, and the per-floor CSV files and sheets.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// A made-up one-storey building: panel LP1 fed from MDP (feeder F1), circuit LP1-3's estimated
/// route from LP1 zig-zagging through four corners to a receptacle and down to its box (five
/// quarter bends: past the 360° between pull points), and LP1-1's straight route to another.
/// `wiring`: the project's wiring method; `feeder_route`: F1 routed gear to gear; `volts`: LP1's.
fn building(wiring: Option<&str>, feeder_route: bool, volts: Option<&str>) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    if let Some(w) = wiring {
        s.execute("project.rules.set", &json!({"domain": "wiring_method", "key": "general", "value": w, "tier": "spec", "source": "Spec 26 05 19", "reason": "the specification's wiring method"})).expect("rule");
    }
    let c3 = "circuit:L1:E-101:LP1-3";
    let c1 = "circuit:L1:E-101:LP1-1";
    let circuit = |id: &str, num: &str| {
        json!({"id": id, "kind": "circuit", "level": "L1", "at_ft": [0.0, 10.0], "fields": {
            "circuit": {"value": format!("LP1-{num}"), "source": "plan"},
            "panel": {"value": "LP1", "source": "plan"},
            "conductors": {"value": "2#12 + 1#12G", "source": "default"},
            "conduit": {"value": "3/4\" EMT", "source": "default"},
            "route": {"value": "estimated route", "source": "rule"}
        }})
    };
    let receptacle = |x_in: f64, y_in: f64, num: &str| {
        json!({"id": format!("device:L1:{},{}", x_in / 12.0, y_in / 12.0), "kind": "receptacle", "level": "L1", "at_ft": [x_in / 12.0, y_in / 12.0], "fields": {
            "type": {"value": "A", "source": "plan"},
            "symbol": {"value": "1F0", "source": "plan"},
            "sheet": {"value": "E-101", "source": "plan"},
            "circuit": {"value": format!("LP1-{num}"), "source": "plan"},
            "mount_height_in": {"value": "18", "source": "note"}
        }})
    };
    let mut runs = vec![
        json!({"system": "E-CIRCUIT-EST", "level": "L1", "points": [[0, 120], [120, 120], [120, 240], [240, 240], [240, 360], [360, 360]], "size_in": [0.75, 0], "bottoms_ft": [9, 9, 9, 9, 9],
               "id": format!("{c3}#home"), "height_source": "estimated route"}),
        json!({"system": "E-CIRCUIT-EST", "level": "L1", "points": [[360, 360]], "size_in": [0.75, 0], "bottoms_ft": [1.5], "top_ft": 9,
               "id": format!("{c3}#d0"), "height_source": "estimated route"}),
        json!({"system": "E-CIRCUIT-EST", "level": "L1", "points": [[0, 112], [200, 112]], "size_in": [0.75, 0], "bottoms_ft": [9],
               "id": format!("{c1}#home"), "height_source": "estimated route"}),
        json!({"system": "E-CIRCUIT-EST", "level": "L1", "points": [[200, 112]], "size_in": [0.75, 0], "bottoms_ft": [1.5], "top_ft": 9,
               "id": format!("{c1}#d0"), "height_source": "estimated route"}),
        // A drawn raceway on no circuit: nothing to pull.
        json!({"system": "E-COND", "level": "L1", "points": [[0, 600], [444, 600]], "size_in": [1, 0], "bottoms_ft": [10], "size_source": "tag", "height_source": "tag"}),
    ];
    let mut systems = vec![
        json!({"name": "E-COND", "layer": "E-COND", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 9}),
        json!({"name": "E-CIRCUIT-EST", "layer": "E-CIRCUIT-EST", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9}),
    ];
    if feeder_route {
        systems.push(json!({"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "conduit", "size_in": [1.5, 0], "bottom_ft": 10, "show": true}));
        runs.push(json!({"system": "E-FEEDER", "level": "L1", "source_sheet": "feeder F1 MDP\u{2192}LP1", "points": [[-175, 120], [-110, 120], [-110, 40], [-14, 40], [-14, 116]], "size_in": [1.5, 0], "bottoms_ft": [10, 10, 10, 10]}));
    }
    let mut lp1 = json!({"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, 106], "max": [-4, 126], "height_ft": 4, "bottom_ft": 2});
    if let Some(v) = volts {
        lp1["properties"] = json!({"VOLTS": v});
    }
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": [receptacle(360.0, 360.0, "3"), circuit(c3, "3"), receptacle(200.0, 112.0, "1"), circuit(c1, "1")],
        "mep": {
            "systems": systems,
            "runs": runs,
            "equipment": [lp1, {"tag": "MDP", "kind": "switchboard", "level": "L1", "min": [-200, 100], "max": [-150, 140], "height_ft": 7}],
            "feeders": [{"tag": "F1", "from": "MDP", "to": "LP1", "sets": 1, "phases": 3, "size": "#1", "material": "cu", "neutrals": 1, "grounds": 1, "ground_size": "#6", "conduits": 1, "conduit_in": 1.5, "entry": "top"}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({"route": false})).expect("build");
    assert!(built["atoms"].as_u64().unwrap_or(0) > 0, "{built}");
    s
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-pulllist-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("tmp");
    d
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

fn pull<'a>(r: &'a Value, circuit: &str) -> &'a Value {
    r["pulls"].as_array().and_then(|a| a.iter().find(|p| p["circuit"] == json!(circuit) || p["feeder"] == json!(circuit))).unwrap_or(&Value::Null)
}

#[test]
fn a_route_past_360_degrees_of_bends_gets_a_pull_box_and_its_slack() {
    let mut s = building(None, false, None);
    let r = s.execute("pull.schedule", &json!({})).expect("schedule");
    let p = pull(&r, "LP1-3");
    assert!(p.is_object(), "{r}");
    // Four corners on the ceiling and the bend into the drop: 450° between the panel and the box.
    assert!((f(&p["bends_deg"]) - 450.0).abs() < 1.0, "{p}");
    assert_eq!(p["pull_boxes"].as_array().map(Vec::len), Some(1), "{p}");
    assert_eq!(p["pulls"].as_array().map(Vec::len), Some(2), "{p}");
    for x in p["pulls"].as_array().into_iter().flatten() {
        assert!(f(&x["bends_deg"]) <= 361.0, "no pull over the limit: {p}");
    }
    let legs: f64 = p["pulls"].as_array().into_iter().flatten().map(|x| f(&x["ft"])).sum();
    assert!((legs - f(&p["raceway_ft"])).abs() < 0.05, "the pulls add up to the path: {p}");
    // Small conductors: the trade's box for a 3/4" raceway, with the rule.
    assert!(p["pull_boxes"][0]["size"].as_str().is_some_and(|b| b.contains("4\" square")), "{p}");
    assert!(p["pull_boxes"][0]["source"].as_str().is_some_and(|b| b.contains("314")), "{p}");
    // The route: 50 ft across the ceiling and the drop to the box; it runs from the panel to it.
    let drop = f(&p["vertical_ft"]);
    assert!(drop > 6.0 && drop < 8.0, "{p}");
    assert!((f(&p["raceway_ft"]) - (50.0 + drop)).abs() < 0.05, "{p}");
    assert_eq!(p["from"], json!("LP1"), "{p}");
    assert_eq!(p["to"], json!("receptacle box"), "{p}");
    // 2#12 + 1#12G on circuit 3 (phase B): red, white, green; each cut the path plus the
    // takeoff's makeup, 1.5 ft at the box (300.14's 6" and a 12" allowance) and 3.5 ft at the
    // panel (half its 4 ft can and a box's makeup), and a 2 ft loop in the pull box.
    let mut colors: Vec<&str> = p["conductors"].as_array().into_iter().flatten().filter_map(|c| c["color"].as_str()).collect();
    colors.sort_unstable();
    assert_eq!(colors, vec!["green", "red", "white"], "{p}");
    for c in p["conductors"].as_array().into_iter().flatten() {
        assert!((f(&c["cut_ft"]) - (f(&p["raceway_ft"]) + 1.5 + 3.5 + 2.0)).abs() < 0.05, "{c} {p}");
    }
    // The straight circuit: no pull box, one pull.
    let q = pull(&r, "LP1-1");
    assert_eq!(q["pull_boxes"].as_array().map(Vec::len), Some(0), "{q}");
    assert!(f(&q["bends_deg"]) < 100.0, "{q}");
    // The drawn raceway on no circuit is set apart.
    assert_eq!(r["empty_raceways"]["runs"], json!(1), "{r}");
}

#[test]
fn the_wire_pulled_reconciles_with_the_takeoff() {
    let mut s = building(None, false, None);
    let r = s.execute("pull.schedule", &json!({})).expect("schedule");
    let w = &r["wire"];
    assert_eq!(w["matches"], json!(true), "{w}");
    let t = &w["terms"];
    let expect =
        f(&w["takeoff_ft"]) - f(&t["not_in_pulls_ft"]) + f(&t["gear_end_slack_ft"]) + f(&t["pull_box_loops_ft"]) + f(&t["feeder_conductors_ft"]);
    assert!((expect - f(&w["pulled_ft"])).abs() < 0.05, "{w}");
    // The takeoff's wire is the model's: bim.count's.
    let counted = s.execute("bim.count", &json!({"kind": "wire"})).expect("count");
    assert!((f(&counted["length_ft"]) - f(&w["takeoff_ft"])).abs() < 0.05, "{counted} {w}");
    // F1 is not routed: its stubs' wire is the takeoff's, in no pull.
    assert_eq!(r["unrouted_feeders"][0]["feeder"], json!("F1"), "{r}");
    assert!(f(&t["not_in_pulls_ft"]) > 0.0, "{w}");
    assert_eq!(f(&t["feeder_conductors_ft"]), 0.0, "{w}");
    // Two circuits' three conductors each, one pull box on LP1-3.
    assert!((f(&t["pull_box_loops_ft"]) - 3.0 * 2.0).abs() < 1e-6, "{w}");
    // The takeoff's panel makeup (3 ft) already gives each conductor its slack at the panel.
    assert!(f(&t["gear_end_slack_ft"]).abs() < 1e-6, "{w}");
}

#[test]
fn a_routed_feeder_is_pulled_gear_to_gear_with_its_schedules_conductors() {
    let mut s = building(None, true, Some("480Y/277"));
    let r = s.execute("pull.schedule", &json!({"circuit": "F1"})).expect("schedule");
    let p = pull(&r, "F1 to LP1");
    assert!(p.is_object(), "{r}");
    assert_eq!(p["kind"], json!("feeder"));
    assert_eq!((p["from"].as_str(), p["to"].as_str()), (Some("MDP"), Some("LP1")), "{p}");
    // 3#1 + 1#1 N + 1#6G per the feeder schedule; LP1 is 277/480: brown, orange, yellow, gray.
    let n: u64 = p["conductors"].as_array().into_iter().flatten().filter_map(|c| c["count"].as_u64()).sum();
    assert_eq!(n, 5, "{p}");
    let mut colors: Vec<&str> = p["conductors"].as_array().into_iter().flatten().filter_map(|c| c["color"].as_str()).collect();
    colors.sort_unstable();
    assert_eq!(colors, vec!["brown", "gray", "green", "orange", "yellow"], "{p}");
    // Gear at both ends: 3 ft each.
    for c in p["conductors"].as_array().into_iter().flatten() {
        assert!((f(&c["cut_ft"]) - (f(&p["raceway_ft"]) + 6.0)).abs() < 0.05, "{c} {p}");
    }
    assert!(f(&p["raceway_ft"]) > 13.0, "{p}");
    // The feeder's conductors aren't the takeoff's (only its stubs are): the reconciliation says so.
    let all = s.execute("pull.schedule", &json!({"max": 0})).expect("all");
    let w = &all["wire"];
    assert_eq!(w["matches"], json!(true), "{w}");
    assert!((f(&w["terms"]["feeder_conductors_ft"]) - f(&p["conductor_ft"])).abs() < 0.05, "{w} {p}");
    assert!(all["unrouted_feeders"].as_array().is_some_and(Vec::is_empty), "{all}");
    assert_eq!(all["pulls"].as_array().map(Vec::len), Some(0), "max 0 lists none");
    assert!(all["totals"]["pulls"].as_u64().unwrap_or(0) >= 3, "{all}");
    // #1 conductors in 1-1/2": a pull box would be sized 8 x the trade size (NEC 314.28).
    assert!(f(&p["max_bends_deg"]) > 0.0, "{p}");
}

#[test]
fn the_cut_list_totals_equal_the_takeoff() {
    let mut s = building(None, true, None);
    let r = s.execute("pull.cutlist", &json!({})).expect("cutlist");
    assert_eq!(r["takeoff_check"]["conduit_takeoff"]["matches"], json!(true), "{r}");
    assert_eq!(r["takeoff_check"]["model_index"]["matches"], json!(true), "{r}");
    assert!(r["takeoff_check"]["conduit_takeoff"]["compared"].as_u64().unwrap_or(0) >= 4, "{r}");
    // The 37' drawn run of 1" EMT: three full sticks and a 7' cut piece, three couplings.
    let drawn = r["runs"].as_array().and_then(|a| a.iter().find(|x| x["raceway"] == json!("1\" EMT"))).cloned().unwrap_or_default();
    assert_eq!(drawn["full_sticks"], json!(3), "{drawn}");
    assert_eq!(drawn["cut_pieces"], json!([7.0]), "{drawn}");
    assert_eq!(drawn["couplings"], json!(3), "{drawn}");
    assert_eq!(drawn["connectors"], json!(2), "{drawn}");
    // LP1-3's 3/4" is bent by hand: its five bends are field bends, no elbow.
    let c3 = r["runs"].as_array().and_then(|a| a.iter().find(|x| x["circuit"] == json!("LP1-3"))).cloned().unwrap_or_default();
    let bends: u64 = c3["field_bends"].as_object().map(|o| o.values().filter_map(Value::as_u64).sum()).unwrap_or(0);
    assert_eq!(bends, 5, "{c3}");
    assert!(c3["elbows"].as_object().is_some_and(serde_json::Map::is_empty), "{c3}");
    // The feeder's 1-1/2" is past hand bending: elbows by their angle.
    let fd = r["runs"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["raceway"] == json!("1-1/2\" EMT") && x["system"] == json!("E-FEEDER")))
        .cloned()
        .unwrap_or_default();
    assert!(fd["elbows"].as_object().is_some_and(|o| !o.is_empty()), "{fd}");
    // By size and floor: the sticks' feet are bim.count's.
    let counted = s.execute("bim.count", &json!({"kind": "conduit", "size": "1\""})).expect("count");
    let line = r["by_size_floor"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["kind"] == json!("conduit") && x["size"] == json!("1\"")))
        .cloned()
        .unwrap_or_default();
    assert!((f(&line["qty"]) - f(&counted["length_ft"])).abs() < 0.01, "{line} {counted}");
    // An edit keeps them equal.
    let h = {
        let d = s.doc().expect("doc");
        d.model
            .iter()
            .find(|e| {
                let ps = cadaddy_engine::doc::props::read(&e.common.xdata);
                ps.get("atom").map(|p| p.text()).as_deref() == Some("conduit") && ps.get("size").map(|p| p.text()).as_deref() == Some("1\"")
            })
            .map(|e| e.handle.hex())
            .expect("a stick")
    };
    s.execute("bim.atoms.edit", &json!({"handle": h, "length_ft": 4.5})).expect("edit");
    let again = s.execute("pull.cutlist", &json!({})).expect("cutlist");
    assert_eq!(again["takeoff_check"]["conduit_takeoff"]["matches"], json!(true), "{again}");
    assert_eq!(again["takeoff_check"]["model_index"]["matches"], json!(true), "{again}");
}

#[test]
fn nm_cable_runs_reel_from_coils_and_wire_from_reels() {
    let mut s = building(Some("NM cable"), false, None);
    let r = s.execute("pull.schedule", &json!({})).expect("schedule");
    let p = pull(&r, "LP1-3");
    assert!(p["raceway"].as_str().is_some_and(|x| x.starts_with("NM")), "{p}");
    // A cable is laid, not pulled: no pull box however it turns.
    assert_eq!(p["pull_boxes"].as_array().map(Vec::len), Some(0), "{p}");
    let reels = r["reels"].as_array().cloned().unwrap_or_default();
    let nm = reels.iter().find(|g| g["kind"] == json!("cable")).cloned().unwrap_or_default();
    assert!(nm["source"].as_str().is_some_and(|x| x.contains("NM-B")), "{nm}");
    // Two cables, both on one 250 ft coil.
    assert_eq!(nm["cuts"], json!(2), "{nm}");
    assert_eq!(nm["reels"].as_array().map(Vec::len), Some(1), "{nm}");
    assert_eq!(nm["reels"][0]["length_ft"], json!(250.0), "{nm}");
    assert_eq!(r["wire"]["matches"], json!(true), "{}", r["wire"]);
    // EMT: every conductor on a standard reel of its size and colour, none lost.
    let e = building(None, false, None).execute("pull.schedule", &json!({})).expect("emt");
    let mut cuts = 0;
    for g in e["reels"].as_array().into_iter().flatten() {
        assert_eq!(g["kind"], json!("wire"), "{g}");
        let on: usize = g["reels"].as_array().into_iter().flatten().map(|x| x["cuts"].as_array().map_or(0, Vec::len)).sum();
        assert_eq!(on as u64, g["cuts"].as_u64().unwrap_or(0), "{g}");
        for x in g["reels"].as_array().into_iter().flatten() {
            assert!([500.0, 2500.0].contains(&f(&x["length_ft"])), "{x}");
        }
        cuts += on;
    }
    // Two circuits of three conductors.
    assert_eq!(cuts, 6, "{e}");
}

#[test]
fn release_lists_go_per_floor_and_phase_with_their_price_book_lines() {
    let mut s = building(None, false, None);
    let book = tmp("book");
    let r = s.execute("pull.release", &json!({"price_folder": book.to_string_lossy()})).expect("release");
    let lines = r["lines"].as_array().cloned().unwrap_or_default();
    let emt = lines.iter().find(|l| l["item"] == json!("EMT conduit") && l["size"] == json!("3/4\"")).cloned().unwrap_or_default();
    assert_eq!(emt["phase"], json!("rough-in"), "{emt}");
    assert_eq!(emt["price_keys"], json!(["emt34"]), "{emt}");
    assert!((f(&emt["extended"]) - f(&emt["unit_price"]) * f(&emt["qty"])).abs() < 0.01, "{emt}");
    // The feet released are the model's.
    let counted = s.execute("bim.count", &json!({"kind": "conduit", "size": "3/4\""})).expect("count");
    assert!((f(&emt["qty"]) - f(&counted["length_ft"])).abs() < 0.01, "{emt} {counted}");
    // The receptacles and their plates go out at trim; a device with no map row says why.
    let rec = lines.iter().find(|l| l["category"] == json!("devices")).cloned().unwrap_or_default();
    assert_eq!(rec["phase"], json!("trim"), "{rec}");
    assert_eq!(rec["qty"], json!(2.0), "{rec}");
    assert!(rec["unpriced"].as_str().is_some_and(|w| w.contains("Item Map")), "{rec}");
    let plate = lines.iter().find(|l| l["category"] == json!("plates")).cloned().unwrap_or_default();
    assert_eq!((plate["phase"].as_str(), plate["qty"].as_f64()), (Some("trim"), Some(2.0)), "{plate}");
    // Field bends are the crew's: nothing released for them.
    assert!(!lines.iter().any(|l| l["item"].as_str().is_some_and(|i| i.contains("field bend"))), "{r}");
    // One phase; per room.
    let trim = s.execute("pull.release", &json!({"phase": "trim", "price_folder": book.to_string_lossy()})).expect("trim");
    assert!(trim["lines"].as_array().into_iter().flatten().all(|l| l["phase"] == json!("trim")), "{trim}");
    let rooms = s.execute("pull.release", &json!({"by": "room", "price_folder": book.to_string_lossy()})).expect("rooms");
    assert!(rooms["totals"].as_array().into_iter().flatten().all(|t| t.get("area").is_some()), "{rooms}");
    assert!(s.execute("pull.release", &json!({"by": "sideways"})).is_err());
    assert!(s.execute("pull.release", &json!({"phase": "demolition"})).is_err());
    let _ = std::fs::remove_dir_all(&book);
}

#[test]
fn each_floor_gets_its_csv_files_and_printable_sheets() {
    let mut s = building(None, true, None);
    let dir = tmp("sheets");
    let r = s.execute("pull.sheets", &json!({"folder": dir.to_string_lossy(), "price_folder": dir.to_string_lossy()})).expect("sheets");
    let fl = &r["floors"][0];
    assert_eq!(fl["level"], json!("L1"), "{r}");
    for name in ["pulls", "reels", "cuts", "release"] {
        let path = dir.join(format!("{name}-L1.csv"));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(text.lines().count() >= 2, "{name}: {text}");
    }
    let pulls = std::fs::read_to_string(dir.join("pulls-L1.csv")).unwrap_or_default();
    assert_eq!(pulls.lines().count() as u64 - 1, fl["rows"]["pulls"].as_u64().unwrap_or(0), "{pulls}");
    assert!(pulls.contains("LP1-3") && pulls.contains("F1 to LP1"), "{pulls}");
    // The sheet: a paper space layout holding the schedules' text.
    let layout = fl["layouts"][0].as_str().unwrap_or_default().to_string();
    let d = s.doc().expect("doc");
    let l = d.layout(&layout).expect("layout");
    let texts: Vec<String> = l
        .entities
        .iter()
        .filter_map(|e| match &e.kind {
            cadaddy_engine::doc::EntityKind::Text(t) => Some(t.value.clone()),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t.starts_with("WIRE PULL SCHEDULE")), "{texts:?}");
    assert!(texts.iter().any(|t| t.starts_with("REEL PLAN")), "{texts:?}");
    assert!(texts.iter().any(|t| t.starts_with("CONDUIT CUT AND BEND LIST")), "{texts:?}");
    assert!(texts.iter().any(|t| t.starts_with("MATERIAL RELEASE")), "{texts:?}");
    assert!(texts.iter().any(|t| t == "LP1-3"), "{texts:?}");
    // Again: the files are there, so it asks for `force`; with it, the sheets are drawn again.
    let e = s.execute("pull.sheets", &json!({"folder": dir.to_string_lossy()})).expect_err("existing").to_string();
    assert!(e.contains("force"), "{e}");
    let again = s.execute("pull.sheets", &json!({"folder": dir.to_string_lossy(), "force": true})).expect("again");
    assert_eq!(again["floors"][0]["layouts"], fl["layouts"], "the same sheets, drawn again");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn files_and_filters_and_hostile_params() {
    let mut s = building(None, false, None);
    let dir = tmp("files");
    let pulls = dir.join("pulls.csv");
    let reels = dir.join("reels.csv");
    let r = s.execute("pull.schedule", &json!({"file": pulls.to_string_lossy(), "reels_file": reels.to_string_lossy()})).expect("files");
    assert!(r["file"].is_string() && pulls.exists() && reels.exists(), "{r}");
    assert!(s.execute("pull.schedule", &json!({"file": pulls.to_string_lossy()})).is_err(), "an existing file needs force");
    let cuts = dir.join("cuts.xlsx");
    s.execute("pull.cutlist", &json!({"file": cuts.to_string_lossy()})).expect("xlsx");
    assert!(cuts.exists());
    // A level with nothing, a circuit filter, hostile numbers.
    let none = s.execute("pull.schedule", &json!({"level": "L9"})).expect("none");
    assert_eq!(none["totals"]["pulls"], json!(0), "{none}");
    let one = s.execute("pull.schedule", &json!({"circuit": "lp1-1"})).expect("one");
    assert_eq!(one["totals"]["pulls"], json!(1), "{one}");
    for bad in [json!({"max": -1}), json!({"max": 1e30}), json!({"max": "x"})] {
        let _ = s.execute("pull.schedule", &bad);
        let _ = s.execute("pull.cutlist", &bad);
        let _ = s.execute("pull.release", &bad);
    }
    // No model: an error that says to build it.
    let mut empty = Session::empty();
    empty.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let e = empty.execute("pull.schedule", &json!({})).expect_err("no model").to_string();
    assert!(e.contains("model.build"), "{e}");
    assert!(empty.execute("pull.sheets", &json!({})).is_err());
    assert!(empty.execute("pull.sheets", &json!({"pdf": true})).is_err(), "pdf needs a folder");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A feeder the routing added after the build (as the job does): routed runs in the spec, no
/// pieces in the model. F2 from MDP up a riser to LP2 on the floor above, whose plan sits at its
/// own corner of the sheet.
fn with_spec_feeder(s: &mut Session) {
    let got = s.execute("bspec.get", &json!({})).expect("spec");
    let mut spec = got["spec"].clone();
    spec["levels"]
        .as_array_mut()
        .expect("levels")
        .push(json!({"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "region": [-300, 2900, 700, 3900]}));
    let mep = spec["mep"].as_object_mut().expect("mep");
    mep["equipment"].as_array_mut().expect("equipment").push(json!({"tag": "LP2", "kind": "panel", "level": "L2", "min": [76, 3190], "max": [96, 3210], "height_ft": 4, "bottom_ft": 2, "properties": {"VOLTS": "208Y/120"}}));
    mep["feeders"].as_array_mut().expect("feeders").push(json!({"tag": "F2", "from": "MDP", "to": "LP2", "sets": 1, "phases": 3, "size": "#3", "material": "cu", "neutrals": 1, "grounds": 1, "ground_size": "#8", "conduits": 1, "conduit_in": 1, "entry": "top"}));
    if !mep["systems"].as_array().expect("systems").iter().any(|y| y["name"] == json!("E-FEEDER")) {
        mep["systems"]
            .as_array_mut()
            .expect("systems")
            .push(json!({"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 10, "show": true}));
    }
    let leg = "feeder F2 MDP\u{2192}LP2";
    let runs = mep["runs"].as_array_mut().expect("runs");
    for r in [
        json!({"points": [[-160, 130]], "bottoms_ft": [7], "top_ft": 10, "level": "L1"}),
        json!({"points": [[-160, 130], [404, 130], [404, 300]], "bottoms_ft": [10, 10], "level": "L1"}),
        json!({"points": [[404, 300]], "bottoms_ft": [10], "top_ft": 22, "level": "L1"}),
        json!({"points": [[104, 3200], [86, 3200]], "bottoms_ft": [10], "level": "L2"}),
        json!({"points": [[86, 3200]], "bottoms_ft": [6], "top_ft": 10, "level": "L2"}),
    ] {
        let mut r = r;
        r["system"] = json!("E-FEEDER");
        r["source_sheet"] = json!(leg);
        r["size_in"] = json!([1, 0]);
        r["size_source"] = json!("feeder schedule");
        r["height_source"] = json!("proposed");
        runs.push(r);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("set");
}

#[test]
fn a_feeder_held_only_as_routed_runs_is_pulled_from_them_and_said_to_be_in_no_takeoff() {
    let mut s = building(None, false, None);
    with_spec_feeder(&mut s);
    let r = s.execute("pull.schedule", &json!({})).expect("schedule");
    let p = pull(&r, "F2 to LP2");
    assert!(p.is_object(), "{r}");
    assert_eq!((p["from"].as_str(), p["to"].as_str()), (Some("MDP"), Some("LP2")), "{p}");
    assert_eq!(p["in_takeoff"], json!(false), "{p}");
    assert_eq!(p["raceway"], json!("1\" EMT"), "{p}");
    // Out of MDP's top (3 ft), across (47 + 14.17 ft), up a floor (12 ft), across (1.5 ft) and
    // down into LP2 (4 ft): the floors joined at the riser.
    assert!((f(&p["raceway_ft"]) - 81.67).abs() < 0.05, "{p}");
    assert!((f(&p["vertical_ft"]) - 19.0).abs() < 0.05, "{p}");
    assert!((f(&p["bends_deg"]) - 450.0).abs() < 1.0, "{p}");
    // #3 conductors: a pull box 8 x 1" long (NEC 314.28) in the straight before the fifth bend.
    assert_eq!(p["pull_boxes"].as_array().map(Vec::len), Some(1), "{p}");
    assert!(p["pull_boxes"][0]["size"].as_str().is_some_and(|b| b.starts_with("8\" long")), "{p}");
    let mut colors: Vec<&str> = p["conductors"].as_array().into_iter().flatten().filter_map(|c| c["color"].as_str()).collect();
    colors.sort_unstable();
    assert_eq!(colors, vec!["black", "blue", "green", "red", "white"], "{p}");
    for c in p["conductors"].as_array().into_iter().flatten() {
        // Both ends at gear (3 ft each), one pull box loop (2 ft).
        assert!((f(&c["cut_ft"]) - (f(&p["raceway_ft"]) + 6.0 + 2.0)).abs() < 0.05, "{c} {p}");
    }
    assert!((f(&r["feeders_not_in_takeoff"]["raceway_ft"]) - f(&p["raceway_ft"])).abs() < 0.05, "{r}");
    assert_eq!(r["wire"]["matches"], json!(true), "{}", r["wire"]);
    assert!(!r["unrouted_feeders"].as_array().into_iter().flatten().any(|u| u["feeder"] == json!("F2")), "{r}");
    // The cut list says it has no pieces for it, and still equals the takeoff.
    let c = s.execute("pull.cutlist", &json!({})).expect("cutlist");
    assert!((f(&c["feeders_not_cut"]["raceway_ft"]) - f(&p["raceway_ft"])).abs() < 0.05, "{c}");
    assert_eq!(c["takeoff_check"]["conduit_takeoff"]["matches"], json!(true), "{c}");
}
