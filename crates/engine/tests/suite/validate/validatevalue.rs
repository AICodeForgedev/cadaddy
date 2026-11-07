//! Q6155 validatevalue: Validate checks the value-engineering recommendations: each option's
//! tier against the specification and the code (G-VE-TIER), where its cable runs (G-VE-AREA), its
//! savings recomputed from the model and the price book (G-VE-COST), its RFI (G-VE-RFI) and the
//! as-designed model unchanged by any scenario not approved (G-VE-DESIGN). Made-up building only.

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Source};
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Mesh, MeshFace, RawTag, Space};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

const RULES: [&str; 5] = ["G-VE-TIER", "G-VE-AREA", "G-VE-COST", "G-VE-RFI", "G-VE-DESIGN"];

fn receptacle(n: usize, x: f64, y: f64, circuit: &str) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x2F0 + n), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"},
        "mount_height_in": {"value": "18", "source": "note"}
    }})
}

fn circuit(id: &str, label: &str, panel: &str) -> Value {
    json!({"id": id, "kind": "circuit", "level": "L1", "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": label, "source": "plan"},
        "panel": {"value": panel, "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "conductors": {"value": "2#12 + 1#12G", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }})
}

/// A made-up specification article that prefers NM (and lists MC) for concealed branch
/// circuits, and asks for feeders in raceway.
const SPEC: &str = "PART 3 - EXECUTION\n3.2\nWIRING METHODS\nA.\nExposed Feeders: Type THHN/THWN-2, single conductors in raceway.\nB.\nFeeders Concealed in Ceilings, Walls, and Partitions: Type THHN/THWN-2, single conductors in raceway.\nC.\nExposed Branch Circuits: Type THHN/THWN-2, single conductors in raceway.\nD.\nBranch Circuits Concealed in Ceilings, Walls, and Partitions: Nonmetallic-sheathed cable, Type NM (preferred), Metal-clad cable, Type MC, or single conductors in raceway.\nE.\nBranch Circuits below Slabs-on-Grade: single conductors in raceway.\n3.3\nINSTALLATION\nA.\nConceal cables.";

fn code_texts(construction: &str) -> Value {
    json!([
        "USE AND OCCUPANCY CLASSIFICATION",
        "RESIDENTIAL GROUP R-1 OCCUPANCY",
        "ACTUAL:",
        "38 FEET / 3 STORIES",
        format!("CONSTRUCTION TYPE   {construction}"),
        "FULLY SPRINKLERED (NFPA 13)",
        "2020 NATIONAL ELECTRICAL CODE"
    ])
}

/// A room element of L1 (its floor outline, inches), tagged as the build tags a traced room.
fn room(s: &mut Session, name: &str, number: &str, [x0, y0, x1, y1]: [f64; 4]) {
    let d = s.doc_mut().expect("doc");
    let tg = |code: i32, v: &str| RawTag { code, value: v.to_string() };
    let xdata = vec![
        tg(1001, "CADADDY_WALLGEN"),
        tg(1000, "room"),
        tg(1000, "L1"),
        tg(1000, "0"),
        tg(1000, &format!("type:{name}")),
        tg(1000, &format!("name:{name}")),
        tg(1000, &format!("number:{number}")),
    ];
    let m = Mesh {
        vertices: vec![Vec3::new(x0, y0, 0.0), Vec3::new(x1, y0, 0.0), Vec3::new(x1, y1, 0.0), Vec3::new(x0, y1, 0.0)],
        faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }],
        ..Mesh::default()
    };
    d.add(&Space::Model, Common { layer: "A-AREA-ROOM".into(), visible: false, xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m))
        .expect("room");
}

/// Two guest units (101, 102) with a panel each of the typical schedule U, fed from DPA along a
/// corridor; a common-area circuit LP1-1 under a lay-in ceiling, an electrical room's LP1-3.
/// Gypsum ceilings everywhere else. `corridor_across`: a corridor drawn across the units' runs.
fn building(dir: &std::path::Path, corridor_across: bool) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let tags = vec![
        receptacle(1, 5.0, 5.0, "U-1"),
        receptacle(2, 12.0, 5.0, "U-1"),
        receptacle(3, 45.0, 5.0, "U-1"),
        receptacle(4, 52.0, 5.0, "U-1"),
        receptacle(5, 20.0, -20.0, "LP1-1"),
        receptacle(6, 30.0, -20.0, "LP1-1"),
        receptacle(7, 30.0, -40.0, "LP1-3"),
        circuit("circuit:L1:room-101:U-1", "U-1", "U"),
        circuit("circuit:L1:E-101:LP1-1", "LP1-1", "LP1"),
        circuit("circuit:L1:E-101:LP1-3", "LP1-3", "LP1"),
    ];
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": tags,
        "ceiling_types": [{"name": "GYP", "description": "gypsum board"}],
        "rcp": {"ceilings": [
            {"level": "L1", "outline": [[100, -300], [400, -300], [400, -180], [100, -180]], "height_ft": 9, "grid": {"spacing_in": [24, 24], "origin": [0, 0]}},
            {"level": "L1", "outline": [[-400, -900], [1400, -900], [1400, 1500], [-400, 1500]], "height_ft": 9, "ceiling_type": "GYP"}
        ]},
        "mep": {
            "systems": [{"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "conduit", "size_in": [2, 2], "bottom_ft": 10, "show": true}],
            "panel_schedules": [
                {"panel": "U", "typical_of": 2, "circuits": [{"number": "1", "description": "RECEPTACLES", "breaker": "20", "poles": 1}]},
                {"panel": "DPA", "circuits": [{"number": "1", "description": "PANEL U (2 UNITS)", "breaker": "125", "poles": 2}]},
                {"panel": "LP1", "circuits": [{"number": "1", "description": "RECEPTACLES", "breaker": "20", "poles": 1}, {"number": "3", "description": "RECEPTACLES", "breaker": "20", "poles": 1}]}
            ],
            "feeders": [
                {"tag": "1", "from": "DPA", "to": "U", "phases": 2, "neutrals": 1, "grounds": 1, "size": "2/0", "ground_size": "4", "material": "al", "conduits": 1, "conduit_in": 2, "sets": 1, "source": "FEEDER SCHEDULE E-601"}
            ],
            "runs": [
                {"system": "E-FEEDER", "level": "L1", "source_sheet": "feeder 1 DPA→U-1A", "points": [[-14, -600], [84, -600], [84, 1200]], "size_in": [2, 2], "bottoms_ft": [10, 10]},
                {"system": "E-FEEDER", "level": "L1", "source_sheet": "feeder 1 DPA→U-1B", "points": [[-14, -600], [580, -600], [580, 1200]], "size_in": [2, 2], "bottoms_ft": [10, 10]}
            ],
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5},
                {"tag": "DPA", "kind": "panel", "level": "L1", "min": [-24, -606], "max": [-4, -594], "height_ft": 4, "bottom_ft": 2.5},
                {"tag": "U-1A", "kind": "panel", "level": "L1", "min": [0, 1200], "max": [14, 1204], "height_ft": 2, "bottom_ft": 4.5},
                {"tag": "U-1B", "kind": "panel", "level": "L1", "min": [480, 1200], "max": [494, 1204], "height_ft": 2, "bottom_ft": 4.5}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    // The rooms (first match wins): the electrical room, a corridor across the units' runs when
    // asked, the units, the corridor along the feeders.
    room(&mut s, "Electrical", "105", [300.0, -560.0, 420.0, -400.0]);
    if corridor_across {
        room(&mut s, "Corridor", "120", [-100.0, 300.0, 800.0, 500.0]);
    }
    room(&mut s, "Guest Room", "101", [-60.0, -100.0, 300.0, 1260.0]);
    room(&mut s, "Guest Room", "102", [400.0, -100.0, 760.0, 1260.0]);
    room(&mut s, "Corridor", "110", [-300.0, -800.0, 1300.0, -100.0]);
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    std::fs::write(dir.join("260519 - Conductors.txt"), SPEC).expect("spec text");
    s
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-vv-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn study(s: &mut Session, dir: &std::path::Path, extra: Value) -> Value {
    let mut p = json!({"specs": [dir.join("260519 - Conductors.txt").to_string_lossy()], "code_texts": code_texts("V-A"), "code_sheet": "C-001"});
    if let (Some(m), Some(x)) = (p.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            m.insert(k.clone(), v.clone());
        }
    }
    s.execute("ve.study", &p).expect("study")
}

/// Every G-VE finding (all pages) of a prescreen given the study (and the extra params).
fn screen(s: &mut Session, extra: Value) -> Vec<Value> {
    let mut p = json!({"level": "L1", "all": true, "limit": 50, "rules": RULES});
    if let (Some(m), Some(x)) = (p.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            m.insert(k.clone(), v.clone());
        }
    }
    let r = s.execute("validate.prescreen", &p).expect("prescreen");
    let mut fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    let mut cursor = r["next"]["cursor"].clone();
    while let Some(c) = cursor.as_u64() {
        p["cursor"] = json!(c);
        let more = s.execute("validate.prescreen", &p).expect("page");
        fs.extend(more["findings"].as_array().cloned().unwrap_or_default());
        cursor = more["next"]["cursor"].clone();
    }
    fs
}

fn finding<'a>(fs: &'a [Value], rule: &str, scenario: &str) -> &'a Value {
    fs.iter().find(|f| f["rule"] == json!(rule) && f["scenario"] == json!(scenario)).unwrap_or(&Value::Null)
}

fn scenario_mut<'a>(v: &'a mut Value, id: &str) -> &'a mut Value {
    let i = v["scenarios"].as_array().and_then(|a| a.iter().position(|x| x["id"] == json!(id))).expect("scenario");
    &mut v["scenarios"][i]
}

/// A cost DB pricing every line of the study both ways (labor in dollars, $50/h).
fn cost_db(v: &Value, design_labor: f64) -> String {
    let mut csv = String::from(
        "# price_date: 2026-09-30\n# manpower_rate: 50\n# manpower_rate_source: the job's labor workbook\nitem,size,unit,material_unit,labor_unit\n",
    );
    let mut seen = std::collections::BTreeSet::new();
    for sc in v["scenarios"].as_array().into_iter().flatten() {
        for side in ["as_designed", "alternative"] {
            for l in sc[side]["lines"].as_array().into_iter().flatten() {
                let item = l["item"].as_str().unwrap_or_default().replace('"', "\"\"");
                let size = l["size"].as_str().unwrap_or_default().replace('"', "\"\"");
                if !seen.insert((item.clone(), size.clone())) {
                    continue;
                }
                let (m, lab) = if side == "as_designed" { (2.0, design_labor) } else { (1.5, 1.0) };
                csv.push_str(&format!("\"{item}\",\"{size}\",{},{m},{lab}\n", l["unit"].as_str().unwrap_or("ea")));
            }
        }
    }
    csv
}

#[test]
fn an_nm_option_the_spec_prefers_in_concealed_branch_circuits_is_allowed_now_and_an_se_feeder_option_needs_its_rfi() {
    let dir = tmp("tiers");
    let mut s = building(&dir, false);
    let v = study(&mut s, &dir, json!({}));
    let tier = |id: &str| v["scenarios"].as_array().and_then(|a| a.iter().find(|x| x["id"] == json!(id))).map(|x| x["tier"].clone());
    assert_eq!(tier("nm_unit_branch"), Some(json!("allowed_now")), "{v}");
    assert_eq!(tier("se_unit_feeders"), Some(json!("needs_rfi")));
    let fs = screen(&mut s, json!({"value_engineering": v.clone()}));
    // NM in the units: 26 05 19 §D lists NM, NEC 334.10(3) allows it in Type V, gypsum over it.
    let nm = finding(&fs, "G-VE-TIER", "nm_unit_branch");
    assert_eq!(nm["verdict"], json!("pass"), "{nm}");
    assert!(nm["measured"]["spec"].as_array().is_some_and(|r| r.iter().all(|x| x["item"] == json!("D") && x["lists"] == json!(true))), "{nm}");
    assert_eq!(nm["measured"]["code"]["cite"], json!("NEC 334.10(3)"));
    let area = finding(&fs, "G-VE-AREA", "nm_unit_branch");
    assert_eq!(area["verdict"], json!("pass"), "{area}");
    assert!(area["measured"]["by_area"]["unit"].as_u64().unwrap_or(0) > 0, "{area}");
    // SER for the feeders: §B asks for raceway (so the RFI), NEC 338.10(B)(4)(a) allows it.
    let se = finding(&fs, "G-VE-TIER", "se_unit_feeders");
    assert_eq!(se["verdict"], json!("pass"), "{se}");
    assert!(se["measured"]["spec"].as_array().is_some_and(|r| r.iter().any(|x| x["item"] == json!("B") && x["lists"] == json!(false))), "{se}");
    assert_eq!(finding(&fs, "G-VE-AREA", "se_unit_feeders")["verdict"], json!("pass"), "{}", finding(&fs, "G-VE-AREA", "se_unit_feeders"));
    // Its RFI cites the option's code and states the intent.
    let rfi = finding(&fs, "G-VE-RFI", "se_unit_feeders");
    assert_eq!(rfi["verdict"], json!("pass"), "{rfi}");
    assert!(rfi["measured"]["cites"].as_array().is_some_and(|c| c.contains(&json!("NEC 338.10(B)(4)(a)"))));
    // Nothing is the design yet: the model is as designed.
    for id in ["nm_unit_branch", "se_unit_feeders", "mc_concealed_branch"] {
        let d = finding(&fs, "G-VE-DESIGN", id);
        assert_eq!(d["verdict"], json!("pass"), "{id}: {d}");
    }

    // The same SER option tiered allowed now: the spec contradicts it.
    let mut wrong = v.clone();
    scenario_mut(&mut wrong, "se_unit_feeders")["tier"] = json!("allowed_now");
    // NM in a Type I-A building tiered allowed now: the code doesn't allow it.
    wrong["facts"]["construction_type"]["value"] = json!("I-A");
    let fs = screen(&mut s, json!({"value_engineering": wrong}));
    let se = finding(&fs, "G-VE-TIER", "se_unit_feeders");
    assert_eq!(se["verdict"], json!("fail"), "{se}");
    assert!(se["issue"].as_str().is_some_and(|i| i.contains("must be NEEDS RFI") && i.contains("§B")), "{se}");
    let nm = finding(&fs, "G-VE-TIER", "nm_unit_branch");
    assert_eq!(nm["verdict"], json!("fail"), "{nm}");
    assert!(nm["issue"].as_str().is_some_and(|i| i.contains("Type I-A")), "{nm}");

    // A study citing nothing that permits the method fails; one whose RFI lacks a citation too.
    let mut uncited = v.clone();
    scenario_mut(&mut uncited, "se_unit_feeders")["code"] =
        json!([{"cite": "NEC 300.21", "says": "firestop"}, {"cite": "NEC 999.9", "says": "made up"}]);
    let fs = screen(&mut s, json!({"value_engineering": uncited}));
    assert_eq!(finding(&fs, "G-VE-TIER", "se_unit_feeders")["verdict"], json!("fail"));
    let rfi = finding(&fs, "G-VE-RFI", "se_unit_feeders");
    assert_eq!(rfi["verdict"], json!("fail"), "{rfi}");
    assert!(rfi["issue"].as_str().is_some_and(|i| i.contains("NEC 999.9")), "{rfi}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nm_proposed_through_a_corridor_fails_its_area() {
    let dir = tmp("corridor");
    let mut s = building(&dir, true);
    let v = study(&mut s, &dir, json!({}));
    let fs = screen(&mut s, json!({"value_engineering": v}));
    let a = finding(&fs, "G-VE-AREA", "nm_unit_branch");
    assert_eq!(a["verdict"], json!("fail"), "{a}");
    assert!(a["measured"]["in_never_areas"]["common"]["pieces"].as_u64().unwrap_or(0) > 0, "{a}");
    assert!(a["measured"]["in_never_areas"]["common"]["rooms"].as_array().is_some_and(|r| r.contains(&json!("Corridor 120"))), "{a}");
    // MC may pass through the corridor.
    assert_eq!(finding(&fs, "G-VE-AREA", "mc_concealed_branch")["verdict"], json!("pass"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn savings_are_recomputed_from_the_model_and_the_price_book_and_one_off_by_a_labor_rate_fails() {
    let dir = tmp("cost");
    let mut s = building(&dir, false);
    let v = study(&mut s, &dir, json!({"model": false}));
    let db = dir.join("costdb.csv");
    std::fs::write(&db, cost_db(&v, 3.0)).expect("cost db");
    let v = study(&mut s, &dir, json!({"cost_db": db.to_string_lossy()}));
    let nm = v["scenarios"].as_array().and_then(|a| a.iter().find(|x| x["id"] == json!("nm_unit_branch"))).cloned().unwrap_or_default();
    assert!(nm["saves"].is_object(), "both ways priced: {nm}");
    let fs = screen(&mut s, json!({"value_engineering": v.clone(), "price_book": db.to_string_lossy()}));
    let c = finding(&fs, "G-VE-COST", "nm_unit_branch");
    assert_eq!(c["verdict"], json!("pass"), "{c}");
    assert!(c["measured"]["quantities"].as_array().is_some_and(|q| !q.is_empty() && q.iter().all(|x| x["ok"] == json!(true))), "{c}");
    let (cl, re) = (&c["measured"]["claimed"], &c["measured"]["recomputed"]);
    assert!((cl["man_hours"].as_f64().unwrap_or(0.0) - re["labor_dollars"].as_f64().unwrap_or(0.0) / 50.0).abs() < 0.05, "{c}");
    // The man-hours taken at another rate than the one stated: fail.
    let mut wrong = v.clone();
    wrong["pricing"]["manpower"]["manpower_rate"] = json!(65.0);
    let fs = screen(&mut s, json!({"value_engineering": wrong, "price_book": db.to_string_lossy()}));
    let c = finding(&fs, "G-VE-COST", "nm_unit_branch");
    assert_eq!(c["verdict"], json!("fail"), "{c}");
    assert!(c["issue"].as_str().is_some_and(|i| i.contains("man-hours") && i.contains("$65/h")), "{c}");
    // A price book whose design labor per unit isn't the study's: the saving's labor fails.
    let other = dir.join("other.csv");
    std::fs::write(&other, cost_db(&v, 4.0)).expect("other db");
    let fs = screen(&mut s, json!({"value_engineering": v.clone(), "price_book": other.to_string_lossy()}));
    let c = finding(&fs, "G-VE-COST", "nm_unit_branch");
    assert_eq!(c["verdict"], json!("fail"), "{c}");
    assert!(c["issue"].as_str().is_some_and(|i| i.contains("labor dollars")), "{c}");
    // A price book named that isn't there is an error; none at all is unsure, never a pass.
    let p = json!({"level": "L1", "rules": RULES, "value_engineering": v.clone(), "price_book": dir.join("none.csv").to_string_lossy()});
    assert!(s.execute("validate.prescreen", &p).is_err());
    let mut bare = v;
    bare["price_book"] = Value::Null;
    let fs = screen(&mut s, json!({"value_engineering": bare}));
    let c = finding(&fs, "G-VE-COST", "nm_unit_branch");
    assert_eq!(c["verdict"], json!("unsure"), "{c}");
    assert!(c["issue"].as_str().is_some_and(|i| i.contains("no price book")), "{c}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unapproved_scenario_leaves_the_design_and_an_approved_one_becomes_it() {
    let dir = tmp("design");
    let mut s = building(&dir, false);
    let v = study(&mut s, &dir, json!({}));
    // Shown is a view, not a change.
    s.execute("ve.scenario", &json!({"id": "mc_concealed_branch", "show": true})).expect("show");
    let fs = screen(&mut s, json!({"value_engineering": v.clone()}));
    let d = finding(&fs, "G-VE-DESIGN", "mc_concealed_branch");
    assert_eq!(
        d["verdict"],
        json!("pass"),
        "{d} {:?}",
        fs.iter().map(|f| (f["rule"].clone(), f["scenario"].clone(), f["verdict"].clone())).collect::<Vec<_>>()
    );
    s.execute("ve.scenario", &json!({"id": "mc_concealed_branch", "show": false})).expect("hide");
    // A design piece the scenario replaces erased: the design changed without approval.
    let gone = {
        let st = s.execute("ve.scenario", &json!({})).expect("list");
        assert!(st["scenarios"].as_array().is_some_and(|a| a.iter().any(|x| x["id"] == json!("mc_concealed_branch"))));
        let d = s.doc().expect("doc");
        d.model
            .iter()
            .find(|e| {
                let p = props::read(&e.common.xdata);
                p.get("atom").map(|x| x.text()).as_deref() == Some("conduit") && p.get("circuit").map(|x| x.text()).as_deref() == Some("LP1-1")
            })
            .map(|e| e.handle)
            .expect("a design stick of LP1-1")
    };
    let kept = s.doc_mut().expect("doc").remove_entity(gone).expect("erased");
    let fs = screen(&mut s, json!({"value_engineering": v.clone()}));
    let d = finding(&fs, "G-VE-DESIGN", "mc_concealed_branch");
    assert_eq!(d["verdict"], json!("fail"), "{d}");
    assert!(d["issue"].as_str().is_some_and(|i| i.contains("gone")), "{d}");
    // A scenario piece counted as the design: fail too.
    let piece = {
        let d = s.doc().expect("doc");
        d.model
            .iter()
            .find(|e| props::read(&e.common.xdata).get("scenario").map(|x| x.text()).as_deref() == Some("nm_unit_branch"))
            .map(|e| e.handle)
            .expect("a scenario piece")
    };
    let _ = s.doc_mut().expect("doc").modify_entity(piece, |e| {
        let mut p = props::read(&e.common.xdata);
        p.set("atom", Prop::new("cable", Source::computed()));
        e.common.xdata = props::write(&e.common.xdata, &p).into();
    });
    let fs = screen(&mut s, json!({"value_engineering": v.clone()}));
    let d = finding(&fs, "G-VE-DESIGN", "nm_unit_branch");
    assert_eq!(d["verdict"], json!("fail"), "{d}");
    assert!(d["issue"].as_str().is_some_and(|i| i.contains("takeoff pieces")), "{d}");
    drop(kept);
    // The SER option approved by its RFI becomes the design: a pass.
    s.execute("ve.decide", &json!({"id": "se_unit_feeders", "decision": "approved", "by": "the engineer"})).expect("approve");
    let fs = screen(&mut s, json!({"value_engineering": v}));
    let d = finding(&fs, "G-VE-DESIGN", "se_unit_feeders");
    assert_eq!(d["verdict"], json!("pass"), "{d}");
    assert_eq!(d["measured"]["adopted"], json!(true), "{d}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_a_study_the_rules_say_so_and_hostile_inputs_never_panic() {
    let dir = tmp("none");
    let mut s = building(&dir, false);
    let r = s.execute("validate.prescreen", &json!({"level": "L1", "rules": RULES})).expect("prescreen");
    assert!(
        r["unmeasured"]["G-VE-TIER"].as_object().is_some_and(|m| m.keys().any(|k| k.contains("no value engineering study"))),
        "{}",
        r["unmeasured"]
    );
    for bad in [json!(5), json!([1]), json!({"scenarios": 3}), json!(dir.join("missing.json").to_string_lossy())] {
        assert!(s.execute("validate.prescreen", &json!({"level": "L1", "rules": RULES, "value_engineering": bad})).is_err());
    }
    let junk = json!({"scenarios": [{"id": "x", "tier": "allowed_now", "code": "no", "saves": {"cost": "x"}, "as_designed": {"lines": [{"qty": "NaN"}]}}, {"tier": 7}, 3],
                      "facts": {"construction_type": {"value": 9}}, "pricing": {"manpower": {"manpower_rate": -1}}});
    let fs = screen(&mut s, json!({"value_engineering": junk}));
    assert!(fs.iter().all(|f| f["verdict"] != json!("pass") || f["rule"] == json!("G-VE-DESIGN")), "{fs:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
