//! Q6154 valueengineering: the wiring-method alternatives the code and the specification allow,
//! tiered (as designed / allowed now / needs RFI), weighed against the design's intent, priced
//! with their manpower from the routed takeoff, modelled as scenarios and decided by their RFI.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

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

/// The specification's wiring-methods article (made up, in the MasterFormat layout).
const SPEC: &str = "PART 3 - EXECUTION\n3.2\nWIRING METHODS\nA.\nExposed Feeders: Type THHN/THWN-2, single conductors in raceway.\nB.\nFeeders Concealed in Ceilings, Walls, and Partitions: Type THHN/THWN-2, single conductors in raceway.\nC.\nExposed Branch Circuits: Type THHN/THWN-2, single conductors in raceway.\nD.\nBranch Circuits Concealed in Ceilings, Walls, and Partitions: Type THHN/THWN-2, single conductors in raceway, Metal-clad cable, Type MC, Nonmetallic-sheathed cable, Type NM.\nE.\nBranch Circuits below Slabs-on-Grade: single conductors in raceway.\n3.3\nINSTALLATION\nA.\nConceal cables.";

/// The code-analysis sheet's texts (made up).
fn code_texts(construction: &str) -> Value {
    json!([
        "USE AND OCCUPANCY CLASSIFICATION",
        "RESIDENTIAL GROUP R-1 OCCUPANCY",
        "ACTUAL:",
        "38 FEET / 3 STORIES",
        format!("CONSTRUCTION TYPE   {construction}"),
        "FULLY SPRINKLERED (NFPA 13)",
        "FLOOR/CEILING CONSTRUCTION",
        "1 HR",
        "2020 NATIONAL ELECTRICAL CODE"
    ])
}

/// A made-up one-storey building: two units (rooms 101 and 102) with a panel each of the
/// typical schedule U, fed from DPA by feeder 1 (aluminum 2/0, 2" EMT); circuit U-1 in each
/// unit; a common-area circuit LP1-1 and a service-room circuit LP1-3 on panel LP1.
fn building(dir: &std::path::Path) -> Session {
    building_with(dir, Vec::new())
}

/// [`building`] with more tags (devices) in it.
fn building_with(dir: &std::path::Path, more: Vec<Value>) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let mut tags = vec![
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
    tags.extend(more);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": tags,
        // A lay-in ceiling over the common area's receptacles (the RCP's grid).
        "rcp": {"ceilings": [{"level": "L1", "outline": [[100, -300], [400, -300], [400, -180], [100, -180]], "height_ft": 9, "grid": {"spacing_in": [24, 24], "origin": [0, 0]}}]},
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
    // The service room: LP1-3's receptacle stands in an electrical room (the rooms' names are the
    // documents'; here set on the device as the build's room fill would).
    {
        let d = s.doc_mut().expect("doc");
        let hs: Vec<_> = d
            .model
            .iter()
            .filter(|e| {
                let p = cadaddy_engine::doc::props::read(&e.common.xdata);
                p.get("circuit").map(|x| x.text()).as_deref() == Some("LP1-3") && p.get("atom").is_none()
            })
            .map(|e| e.handle)
            .collect();
        for h in hs {
            let _ = d.modify_entity(h, |e| {
                let mut p = cadaddy_engine::doc::props::read(&e.common.xdata);
                p.set("room", cadaddy_engine::doc::props::Prop::new("ELEC 105", cadaddy_engine::doc::props::Source::computed()));
                e.common.xdata = cadaddy_engine::doc::props::write(&e.common.xdata, &p).into();
            });
        }
    }
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    std::fs::write(dir.join("260519 - Conductors.txt"), SPEC).expect("spec text");
    s
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-ve-{name}-{}", std::process::id()));
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

fn scenario<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["scenarios"].as_array().and_then(|a| a.iter().find(|x| x["id"] == json!(id))).unwrap_or(&Value::Null)
}

#[test]
fn the_alternatives_are_tiered_from_the_code_sheet_the_spec_and_the_rooms() {
    let dir = tmp("tiers");
    let mut s = building(&dir);
    let v = study(&mut s, &dir, json!({}));
    // The code sheet's facts, each with its text.
    assert_eq!(v["facts"]["construction_type"]["value"], json!("V-A"), "{}", v["facts"]);
    assert_eq!(v["facts"]["sprinklered"]["value"], json!(true));
    assert_eq!(v["facts"]["occupancies"][0]["value"], json!("R-1"));
    assert_eq!(v["facts"]["stories"]["value"], json!(3));
    assert_eq!(v["facts"]["electrical_code"]["value"], json!("NEC 2020"));
    // SER for the units' feeders: the code allows it in Type V, the spec lists raceway only.
    let ser = scenario(&v, "se_unit_feeders");
    assert_eq!(ser["tier"], json!("needs_rfi"), "{ser}");
    assert_eq!(ser["quantity"]["items"], json!(2), "one feeder to each unit's panel: {}", ser["quantity"]);
    assert_eq!(ser["application"]["item"], json!("B"));
    // NM in the units and MC in the concealed areas: the spec lists both: allowed now.
    let nm = scenario(&v, "nm_unit_branch");
    assert_eq!(nm["tier"], json!("allowed_now"), "{nm}");
    assert!(nm["quantity"]["items"].as_u64().unwrap_or(0) >= 1, "{}", nm["quantity"]);
    assert!(nm.get("rfi").is_none_or(Value::is_null));
    let mc = scenario(&v, "mc_concealed_branch");
    assert_eq!(mc["tier"], json!("allowed_now"), "{mc}");
    // MC exposed in the service room: the design's raceway protects it there; not proposed.
    let mx = scenario(&v, "mc_exposed_branch");
    assert_eq!(mx["tier"], json!("not_proposed"), "{mx}");
    assert!(mx["why"].as_str().is_some_and(|w| w.contains("physical protection")), "{mx}");
    // Each tier per kind of run and area, never mixed: as designed always listed.
    let groups = v["groups"].as_array().cloned().unwrap_or_default();
    let unit = groups.iter().find(|g| g["kind"] == json!("branch") && g["area"] == json!("unit")).expect("unit branch group");
    assert!(unit["as_designed"]["items"].as_u64().unwrap_or(0) >= 1, "{unit}");
    assert!(unit["allowed_now"].as_array().is_some_and(|a| a.contains(&json!("nm_unit_branch"))), "{unit}");
    assert!(unit["needs_rfi"].as_array().is_some_and(|a| !a.contains(&json!("nm_unit_branch"))));
    let exposed = groups.iter().find(|g| g["kind"] == json!("branch") && g["area"] == json!("exposed")).expect("exposed group");
    assert!(exposed["not_proposed"].as_array().is_some_and(|a| a.contains(&json!("mc_exposed_branch"))), "{exposed}");
    // The common area's circuit under the lay-in ceiling stays as designed for NM (334.12(A)(2)),
    // not for MC; the units' ceilings the documents don't type are said, to confirm.
    let named = |sc: &Value| {
        sc["quantity"]["named"]
            .as_array()
            .map(|a| a.iter().filter_map(|n| n["label"].as_str().map(str::to_string)).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let nmc = scenario(&v, "nm_common_branch");
    assert!(!named(nmc).contains(&"LP1-1".to_string()), "{nmc}");
    assert!(
        nmc["excluded"]
            .as_array()
            .is_some_and(|e| e.iter().any(|x| x["area"] == json!("suspended") && x["kept_as_designed"].as_u64().unwrap_or(0) >= 1)),
        "{nmc}"
    );
    assert!(named(mc).contains(&"LP1-1".to_string()), "{mc}");
    assert!(
        nm["to_confirm"].as_array().is_some_and(|c| c.iter().any(|x| x["area"] == json!("ceiling_unknown") && x["items"].as_u64().unwrap_or(0) >= 1)),
        "{nm}"
    );
    // Each option says where it is never used and why.
    let excl = nm["excluded"].as_array().cloned().unwrap_or_default();
    assert!(excl.iter().any(|e| e["area"] == json!("suspended") && e["why"].as_str().is_some_and(|w| w.contains("334.12(A)(2)"))), "{excl:?}");
    assert!(excl.iter().any(|e| e["area"] == json!("plenum")));
    // Type I-A: NM and SER aren't allowed by the code; MC still is.
    let v = s
        .execute(
            "ve.study",
            &json!({"specs": [dir.join("260519 - Conductors.txt").to_string_lossy()], "code_texts": code_texts("I-A"), "model": false}),
        )
        .expect("study");
    assert_eq!(scenario(&v, "se_unit_feeders")["tier"], json!("not_allowed"));
    assert_eq!(scenario(&v, "nm_unit_branch")["tier"], json!("not_allowed"));
    assert_eq!(scenario(&v, "mc_concealed_branch")["tier"], json!("allowed_now"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn both_ways_are_priced_from_the_takeoff_with_labor_dollars_turned_into_man_hours() {
    let dir = tmp("price");
    let mut s = building(&dir);
    let v = study(&mut s, &dir, json!({"model": false}));
    let ser = scenario(&v, "se_unit_feeders");
    // Without a cost DB, the estimate's prices; what nothing prices is listed, never guessed.
    let designed: Vec<Value> = ser["as_designed"]["lines"].as_array().cloned().unwrap_or_default();
    assert!(designed.iter().any(|l| l["kind"] == json!("conduit") && l["size"] == json!("2\"")), "{designed:?}");
    assert!(designed.iter().any(|l| l["item"] == json!("XHHW aluminum") && l["size"] == json!("#2/0")), "{designed:?}");
    let alt: Vec<Value> = ser["alternative"]["lines"].as_array().cloned().unwrap_or_default();
    assert!(alt.iter().any(|l| l["item"] == json!("SER cable aluminum") && l["size"] == json!("2/0-2/0-2/0-4")), "{alt:?}");
    assert!(alt.iter().any(|l| l["kind"] == json!("strap")), "the cable's supports: {alt:?}");
    // A cost DB that prices every line both ways (labor in dollars, $50/h).
    let mut csv = String::from(
        "# price_date: 2026-09-30\n# escalation: 4% a year\n# manpower_rate: 50\n# manpower_rate_source: the job's labor workbook\nitem,size,unit,material_unit,labor_unit\n",
    );
    for sc in v["scenarios"].as_array().into_iter().flatten() {
        for side in ["as_designed", "alternative"] {
            for l in sc[side]["lines"].as_array().into_iter().flatten() {
                let item = l["item"].as_str().unwrap_or_default().replace('"', "\"\"");
                let size = l["size"].as_str().unwrap_or_default().replace('"', "\"\"");
                let (m, lab) = if side == "as_designed" { (2.0, 3.0) } else { (1.5, 1.0) };
                csv.push_str(&format!("\"{item}\",\"{size}\",{},{m},{lab}\n", l["unit"].as_str().unwrap_or("ea")));
            }
        }
    }
    let db = dir.join("costdb.csv");
    std::fs::write(&db, &csv).expect("cost db");
    let book = dir.join("Sample Floor Plan-ve-prices.csv");
    let v = study(&mut s, &dir, json!({"model": false, "cost_db": db.to_string_lossy(), "crew_size": 2, "price_book": book.to_string_lossy()}));
    let ser = scenario(&v, "se_unit_feeders");
    let d = &ser["as_designed"];
    assert!(d["unpriced"].as_array().is_some_and(Vec::is_empty), "{d}");
    let (labor, mh) = (d["labor_dollars"].as_f64().unwrap_or(0.0), d["man_hours"].as_f64().unwrap_or(0.0));
    assert!(labor > 0.0 && (mh - labor / 50.0).abs() < 0.05, "man-hours = labor $ ÷ $50/h: {labor} {mh}");
    let sv = &ser["saves"];
    assert!(sv.is_object(), "both ways priced in full: {ser}");
    let dmh = sv["man_hours"].as_f64().unwrap_or(0.0);
    assert!(dmh > 0.0 && (sv["crew_days"].as_f64().unwrap_or(0.0) - dmh / 16.0).abs() < 0.02, "crew-days at 2 × 8 h: {sv}");
    for k in ["material", "labor_dollars", "cost"] {
        assert!(sv[k].as_f64().is_some(), "{k}: {sv}");
    }
    assert_eq!(v["pricing"]["price_date"], json!("2026-09-30"));
    assert_eq!(v["pricing"]["escalation"], json!("4% a year"));
    assert_eq!(v["pricing"]["manpower"]["manpower_rate"], json!(50.0));
    assert_eq!(v["pricing"]["manpower"]["manpower_rate_source"], json!("the job's labor workbook"));
    // The private price book beside the drawing: the rows the job needs, with the DB's basis.
    let b = std::fs::read_to_string(&book).expect("price book");
    assert!(b.contains("# manpower_rate: 50") && b.contains("SER cable aluminum"), "{b}");
    assert!(v["price_book"]["rows"].as_u64().unwrap_or(0) > 3);
    // A DB missing a line: no saving claimed, the line named.
    let short: String = csv.lines().filter(|l| !l.starts_with("\"SER cable")).map(|l| format!("{l}\n")).collect();
    std::fs::write(&db, short).expect("cost db");
    let v = study(&mut s, &dir, json!({"model": false, "cost_db": db.to_string_lossy()}));
    let ser = scenario(&v, "se_unit_feeders");
    assert!(ser["saves"].is_null(), "{ser}");
    assert!(
        ser["unpriced"].as_array().is_some_and(|u| u.iter().any(|x| x.as_str().is_some_and(|x| x.starts_with("SER cable")))),
        "{}",
        ser["unpriced"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_starter_book_prices_the_aluminum_ser_alternative_both_ways() {
    // No cost DB: the starter book carries aluminum XHHW-2 and aluminum SER, so neither side of
    // the SER alternative leaves an aluminum line unpriced.
    let dir = tmp("starter");
    let mut s = building(&dir);
    let v = study(&mut s, &dir, json!({}));
    let ser = scenario(&v, "se_unit_feeders");
    let unpriced: Vec<&str> = ["unpriced", "as_designed", "alternative"]
        .iter()
        .flat_map(|k| {
            let u = if *k == "unpriced" { &ser["unpriced"] } else { &ser[*k]["unpriced"] };
            u.as_array().into_iter().flatten().filter_map(Value::as_str)
        })
        .collect();
    assert!(!unpriced.iter().any(|u| u.starts_with("SER cable") || u.starts_with("XHHW aluminum")), "{ser}");
    // The units' NM branch alternative's cable as the study names it (`NM-B cable`, `12/2 w/G`):
    // the starter book prices it.
    let nm = s
        .execute(
            "estimate.change",
            &json!({"lines": [
                {"item": "NM-B cable", "size": "12/2 w/G", "unit": "ft", "qty": 100.0},
                {"item": "NM-B cable", "size": "10/2 w/G", "unit": "ft", "qty": 100.0}
            ]}),
        )
        .expect("estimate.change");
    let lines = nm["lines"].as_array().expect("lines");
    assert!(lines.len() == 2 && lines.iter().all(|l| l["priced"] == json!(true)), "{nm}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_needs_rfi_option_drafts_its_rfi_with_citations_intent_savings_and_a_sketch() {
    let dir = tmp("rfi");
    let mut s = building(&dir);
    let sk = dir.join("sketches");
    let v = study(&mut s, &dir, json!({"sketches": sk.to_string_lossy()}));
    let ser = scenario(&v, "se_unit_feeders");
    let rfi = &ser["rfi"];
    let id = rfi["id"].as_str().expect("an RFI id").to_string();
    assert!(rfi["question"].as_str().is_some_and(|q| q.contains("service-entrance cable (SER)") && q.contains("2 feeders")), "{rfi}");
    assert!(rfi["conflict"].as_str().is_some_and(|c| c.contains("26 05 19 §B") && c.contains("single conductors in raceway")), "{rfi}");
    assert!(rfi["citations"].as_array().is_some_and(|c| c.iter().any(|x| x["cite"] == json!("NEC 338.10(B)(4)(a)"))), "{rfi}");
    assert!(rfi["building"].as_array().is_some_and(|b| b.iter().any(|x| x.as_str().is_some_and(|x| x.contains("Type V-A")))), "{rfi}");
    // The design's likely reason: the 2" raceway is larger than its conductors need (future
    // capacity): said with its risk.
    assert!(
        rfi["intent"].as_array().is_some_and(|i| i.iter().any(|x| x.as_str().is_some_and(|x| x.contains("future capacity") && x.contains("risk")))),
        "{rfi}"
    );
    assert!(rfi["savings"].as_str().is_some_and(|t| t.contains("savings not priced") || t.contains("saves")), "{rfi}");
    let svg = std::fs::read_to_string(rfi["sketch"].as_str().expect("sketch path")).expect("sketch written");
    assert!(svg.starts_with("<svg") && svg.contains("#d01010") && svg.contains(&id), "a marked-up crop");
    // On the RFI list (kind method_choice, open) and in the review queue as an `rfi` item.
    let list = s.execute("rfi.list", &json!({"kind": "method_choice"})).expect("rfi.list");
    let item = list["items"].as_array().and_then(|a| a.iter().find(|x| x["id"] == json!(id)).cloned()).expect("on the list");
    assert_eq!(item["status"], json!("open"));
    assert!(item["issue"].as_str().is_some_and(|t| t.contains("Conflict:") && t.contains("Intent:") && t.contains("Savings:")), "{item}");
    assert!(item["extent"].is_array(), "its place for the export's picture: {item}");
    let review = s.execute("review.list", &json!({"kind": "rfi", "limit": 50})).expect("review");
    assert!(
        review["items"].as_array().is_some_and(|a| a.iter().any(|x| x["title"].as_str().is_some_and(|t| t.contains("wiring method")))),
        "{review}"
    );
    // Studying again keeps one RFI with its status.
    let _ = study(&mut s, &dir, json!({}));
    let list = s.execute("rfi.list", &json!({"kind": "method_choice"})).expect("rfi.list");
    assert_eq!(list["items"].as_array().map(|a| a.iter().filter(|x| x["id"] == json!(id)).count()), Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}

fn count(s: &mut Session, q: Value) -> Value {
    s.execute("bim.count", &q).expect("count")
}

#[test]
fn a_scenario_toggles_in_the_model_and_the_takeoff_and_its_approved_rfi_makes_it_the_design() {
    let dir = tmp("scenario");
    let mut s = building(&dir);
    let before_conduit = count(&mut s, json!({"kind": "conduit"}))["length_ft"].as_f64().unwrap_or(0.0);
    let v = study(&mut s, &dir, json!({}));
    let m = &scenario(&v, "nm_unit_branch")["model"];
    assert!(m["pieces"].as_u64().unwrap_or(0) > 0 && m["replaces"].as_u64().unwrap_or(0) > 0, "{m}");
    // Hidden at first; the model's counts and its takeoff are the design's.
    assert_eq!(count(&mut s, json!({"kind": "conduit"}))["length_ft"].as_f64().unwrap_or(0.0), before_conduit);
    let design = s.execute("conduit.takeoff", &json!({"group_by": ["circuit"], "price": false})).expect("takeoff");
    let has = |t: &Value, kind: &str, circuit: &str| {
        t["lines"].as_array().is_some_and(|a| a.iter().any(|l| l["kind"] == json!(kind) && l["circuit"] == json!(circuit)))
    };
    assert!(has(&design, "conduit", "U-1") && !has(&design, "cable", "U-1"), "{design}");
    // The takeoff of the scenario: NM cable for the units' circuit, the rest as designed.
    let alt = s.execute("conduit.takeoff", &json!({"group_by": ["circuit"], "price": false, "scenario": "nm_unit_branch"})).expect("takeoff");
    assert!(has(&alt, "cable", "U-1") && !has(&alt, "conduit", "U-1"), "{alt}");
    assert!(has(&alt, "conduit", "LP1-1"), "the common area stays as designed: {alt}");
    // Shown in the model: its layer on, the design's pieces it replaces hidden; hidden again.
    let shown = s.execute("ve.scenario", &json!({"id": "nm_unit_branch", "show": true})).expect("show");
    assert_eq!(shown["shown"], json!(true));
    let layer = shown["layer"].as_str().unwrap_or_default().to_string();
    let hidden = |s: &Session| s.doc().map(|d| d.model.iter().filter(|e| !e.common.visible).count()).unwrap_or(0);
    assert_eq!(hidden(&s) as u64, shown["replaces"].as_u64().unwrap_or(0));
    assert!(s.doc().map(|d| d.layer(&layer).is_some_and(|l| l.on)).unwrap_or(false));
    s.execute("ve.scenario", &json!({"id": "nm_unit_branch", "show": false})).expect("hide");
    assert_eq!(hidden(&s), 0);
    // An option that needs its RFI can't just be chosen.
    assert!(s.execute("ve.decide", &json!({"id": "se_unit_feeders", "decision": "chosen"})).is_err());
    // The RFI answered approved: the feeders are SER in the model, on every rebuild.
    let rfi = scenario(&v, "se_unit_feeders")["rfi"]["id"].as_str().unwrap_or_default().to_string();
    let r = s.execute("rfi.set", &json!({"id": rfi, "status": "approved", "answer": "Engineer: SER acceptable as proposed"})).expect("approve");
    assert_eq!(r["item"]["status"], json!("approved"), "{r}");
    assert!(r["value_engineering"]["adopted"]["pieces"].as_u64().unwrap_or(0) > 0, "{r}");
    let ser_cable = |s: &mut Session| count(s, json!({"kind": "cable", "system": "E-FEEDER"}))["count"].as_u64().unwrap_or(0);
    assert!(ser_cable(&mut s) > 0, "SER in the model");
    s.execute("model.build", &json!({})).expect("rebuild");
    assert!(ser_cable(&mut s) > 0, "a rebuild keeps the approved method");
    // The allowed-now NM chosen: the units' circuit is NM cable, also after a reroute.
    let c = s.execute("ve.decide", &json!({"id": "nm_unit_branch", "decision": "chosen", "by": "project manager"})).expect("choose");
    assert!(c["adopted"]["pieces"].as_u64().unwrap_or(0) > 0, "{c}");
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let t = s.execute("conduit.takeoff", &json!({"group_by": ["circuit"], "price": false})).expect("takeoff");
    assert!(has(&t, "cable", "U-1") && !has(&t, "conduit", "U-1"), "{t}");
    assert!(has(&t, "conduit", "LP1-1"), "{t}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_value_engineering_params_never_panic() {
    let dir = tmp("hostile");
    let mut s = building(&dir);
    for p in [
        json!({}),
        json!({"specs": 5}),
        json!({"specs": ["/nonexistent/26 05 19.pdf"], "code_texts": [1, null, "CONSTRUCTION TYPE", "Ω".repeat(5000)]}),
        json!({"cost_db": "/nonexistent.csv"}),
        json!({"manpower_rate": -1, "crew_size": f64::MAX, "hours_per_day": 0}),
        json!({"construction_type": "XYZ", "model": "yes"}),
    ] {
        let _ = s.execute("ve.study", &p);
    }
    for p in [json!({}), json!({"id": 3}), json!({"id": "nope", "show": true}), json!({"id": "nm_unit_branch", "show": "x"})] {
        let _ = s.execute("ve.scenario", &p);
    }
    for p in [json!({}), json!({"id": "nope", "decision": "approved"}), json!({"id": "nm_unit_branch", "decision": "maybe"})] {
        assert!(s.execute("ve.decide", &p).is_err());
    }
    assert!(s.execute("conduit.takeoff", &json!({"scenario": "nope"})).is_err());
    let mut empty = Session::empty();
    empty.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let _ = empty.execute("ve.study", &json!({}));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A takeoff's lines summed by kind, item and size.
fn lines_of(t: &Value) -> std::collections::BTreeMap<(String, String, String), f64> {
    let mut out = std::collections::BTreeMap::new();
    for l in t["lines"].as_array().into_iter().flatten() {
        let k = (
            l["kind"].as_str().unwrap_or_default().to_string(),
            l["item"].as_str().unwrap_or_default().to_string(),
            l["size"].as_str().unwrap_or_default().to_string(),
        );
        *out.entry(k).or_default() += l["qty"].as_f64().unwrap_or(0.0);
    }
    out
}

/// Sets `key` on the model's elements `pick` chooses (as the build's fills would).
fn set_prop(s: &mut Session, key: &str, value: &str, pick: impl Fn(&cadaddy_engine::doc::props::Props) -> bool) -> usize {
    use cadaddy_engine::doc::props::{self, Prop, Source};
    let d = s.doc_mut().expect("doc");
    let hs: Vec<_> = d.model.iter().filter(|e| pick(&props::read(&e.common.xdata))).map(|e| e.handle).collect();
    for h in &hs {
        let _ = d.modify_entity(*h, |e| {
            let mut p = props::read(&e.common.xdata);
            p.set(key, Prop::new(value, Source::computed()));
            e.common.xdata = props::write(&e.common.xdata, &p).into();
        });
    }
    hs.len()
}

#[test]
fn the_nm_scenarios_takeoff_has_what_nm_work_uses_and_drops_the_raceway_it_replaces() {
    // The units' circuit U-1 (four receptacles; the last two share one plate) and a data outlet
    // in the same room, 101.
    let dir = tmp("nmwork");
    let data = json!({"id": "device:L1:20", "kind": "data_outlet", "level": "L1", "at_ft": [8.0, 5.0], "fields": {
        "type": {"value": "D", "source": "plan"}, "symbol": {"value": "3A0", "source": "plan"}, "sheet": {"value": "T-101", "source": "plan"},
        "mount_height_in": {"value": "18", "source": "note"}
    }});
    let mut s = building_with(&dir, vec![data]);
    let txt = |p: &cadaddy_engine::doc::props::Props, k: &str| p.get(k).map(|x| x.text()).unwrap_or_default();
    // The room the build's room fill would give them; the plate group the device reader would.
    assert!(set_prop(&mut s, "room", "101", |p| txt(p, "circuit") == "U-1" || txt(p, "kind") == "data_outlet") > 4);
    let data_box = set_prop(&mut s, "room", "101", |p| txt(p, "atom") == "box" && txt(p, "circuit").is_empty());
    assert_eq!(data_box, 1, "the data outlet's box");
    // The two receptacles farthest along (45 and 52 ft) share one plate.
    let far: Vec<String> = {
        let d = s.doc().expect("doc");
        let mut devs: Vec<(f64, String)> = d
            .model
            .iter()
            .filter(|e| {
                let p = cadaddy_engine::doc::props::read(&e.common.xdata);
                txt(&p, "atom").is_empty() && txt(&p, "circuit") == "U-1"
            })
            .map(|e| {
                let b = cadaddy_engine::doc::entity_bounds(d, e, 0);
                (b.min.x, e.handle.hex())
            })
            .collect();
        devs.sort_by(|a, b| a.0.total_cmp(&b.0));
        devs.iter().rev().take(2).map(|x| x.1.clone()).collect()
    };
    assert_eq!(far.len(), 2);
    for h in &far {
        s.execute("bim.props.set", &json!({"handle": h, "key": "plate_group", "value": "101/W1#2"})).expect("plate group");
    }
    let v = study(&mut s, &dir, json!({}));
    let nm = scenario(&v, "nm_unit_branch");
    let cw = &nm["cable_work"];
    // Each cable entry: the units' panels (an NM connector each, NEC 312.5(C)); the boxes it is
    // set in now (their own clamps, 314.17(C)).
    assert_eq!(cw["cable_entries"], json!({"gear": 2, "nonmetallic": 6}), "{cw}");
    // The four 4" square boxes: three nonmetallic new-work boxes (two 1 gang; the shared plate's
    // two devices in one 2 gang box).
    assert_eq!(cw["boxes_replaced"], json!(4), "{cw}");
    assert_eq!(cw["new_boxes"], json!({"1 gang": 2, "2 gang": 1}), "{cw}");
    assert_eq!(cw["lv_brackets"], json!(1), "{cw}");
    // A nail plate where each drop passes the top plate: at each receptacle and over each panel.
    assert_eq!(cw["nail_plates"]["top_bottom_plates"], json!(6), "{cw}");
    assert_eq!(cw["nail_plates"]["studs"], json!(0), "bored in the studs' middle: 1-5/16 in to each edge: {cw}");
    let staples = cw["supports"]["count"].as_u64().unwrap_or(0);
    assert!(staples > 0 && cw["supports"]["source"].as_str().is_some_and(|t| t.contains("334.30")), "{cw}");
    let cites = cw["sources"].to_string();
    for c in ["300.4(A)(1)", "312.5(C)", "314.17(C)", "314.16(A)(2)"] {
        assert!(cites.contains(c), "{c}: {cites}");
    }
    // Both takeoffs exact: with the scenario on, the design's lines it replaces are gone and its
    // own are there, every other line as designed.
    let off = lines_of(&s.execute("conduit.takeoff", &json!({"group_by": [], "price": false})).expect("takeoff"));
    let on = lines_of(&s.execute("conduit.takeoff", &json!({"group_by": [], "price": false, "scenario": "nm_unit_branch"})).expect("takeoff"));
    let side = |k: &str| {
        let mut m: std::collections::BTreeMap<(String, String, String), f64> = std::collections::BTreeMap::new();
        for l in nm[k]["lines"].as_array().into_iter().flatten() {
            let key = (
                l["kind"].as_str().unwrap_or_default().to_string(),
                l["item"].as_str().unwrap_or_default().to_string(),
                l["size"].as_str().unwrap_or_default().to_string(),
            );
            *m.entry(key).or_default() += l["qty"].as_f64().unwrap_or(0.0);
        }
        m
    };
    let (design, alt) = (side("as_designed"), side("alternative"));
    // The takeoff names a cable by its conductors' size (`#8`), the study by its makeup (`8/2 w/G`).
    let cable_ft =
        |m: &std::collections::BTreeMap<(String, String, String), f64>| m.iter().filter(|(k, _)| k.0 == "cable").map(|(_, q)| q).sum::<f64>();
    assert!((cable_ft(&on) - cable_ft(&alt)).abs() < 0.05 && cable_ft(&alt) > 0.0, "{on:?} {alt:?}");
    let keys: std::collections::BTreeSet<_> =
        off.keys().chain(on.keys()).chain(design.keys()).chain(alt.keys()).filter(|k| k.0 != "cable" && k.0 != "firestop").cloned().collect();
    for k in keys {
        let want = off.get(&k).copied().unwrap_or(0.0) - design.get(&k).copied().unwrap_or(0.0) + alt.get(&k).copied().unwrap_or(0.0);
        let got = on.get(&k).copied().unwrap_or(0.0);
        assert!((want - got).abs() < 0.05, "{k:?}: off {:?} design {:?} alt {:?} on {got}", off.get(&k), design.get(&k), alt.get(&k));
    }
    let q = |m: &std::collections::BTreeMap<(String, String, String), f64>, kind: &str, item: &str, size: &str| {
        m.get(&(kind.to_string(), item.to_string(), size.to_string())).copied().unwrap_or(0.0)
    };
    // Off: the design's boxes; on: its new boxes, the bracket, the connectors and plates, no
    // raceway left on the units' circuit.
    assert_eq!(
        q(&off, "box", "Box, 4\" square", "4\" square") - q(&on, "box", "Box, 4\" square", "4\" square"),
        5.0,
        "four device boxes and the data outlet's"
    );
    assert_eq!(q(&on, "box", "Device box, 1 gang, nonmetallic", "1 gang"), 2.0);
    assert_eq!(q(&on, "box", "Device box, 2 gang, nonmetallic", "2 gang"), 1.0);
    assert_eq!(q(&on, "bracket", "Low-voltage mounting bracket", "1 gang"), 1.0);
    assert_eq!(q(&on, "connector", "NM cable connector", "3/8\""), 2.0);
    assert_eq!(q(&on, "nail_plate", "Nail plate (stud guard), 1/16 in steel", "1-1/2 x 3"), 6.0);
    // The staples by the cable through the room: its legs' #12 (its voltage drop's #8 is on the
    // homerun only, Q6235).
    assert_eq!(q(&on, "strap", "Cable staple", "#12"), staples as f64);
    assert_eq!(
        q(&off, "connector", "NM cable connector", "3/8\"") + q(&off, "nail_plate", "Nail plate (stud guard), 1/16 in steel", "1-1/2 x 3"),
        0.0
    );
    let feeder_conduit = q(&off, "conduit", "EMT conduit", "2\"");
    assert!(feeder_conduit > 0.0 && q(&on, "conduit", "EMT conduit", "2\"") == feeder_conduit, "the feeders stay as designed");
    // Its $ and labor are the new lines': each priced or named unpriced, never dropped.
    let priced = nm["alternative"]["lines"].as_array().cloned().unwrap_or_default();
    let unpriced = nm["alternative"]["unpriced"].to_string();
    for item in ["NM cable connector", "Nail plate (stud guard)", "Device box, 2 gang, nonmetallic", "Low-voltage mounting bracket"] {
        assert!(
            priced.iter().any(|l| l["item"].as_str().is_some_and(|i| i.starts_with(item)) && (l["material"].is_number() || unpriced.contains(item))),
            "{item}: {priced:?} {unpriced}"
        );
    }
    // The SER scenario: its connectors sized to the cable, a locknut at each.
    let ser = scenario(&v, "se_unit_feeders");
    let lines = ser["alternative"]["lines"].as_array().cloned().unwrap_or_default();
    let has = |kind: &str, size: &str, n: f64| lines.iter().any(|l| l["kind"] == json!(kind) && l["size"] == json!(size) && l["qty"] == json!(n));
    assert!(has("connector", "1-1/4\"", 4.0) && has("locknut", "1-1/4\"", 4.0), "two feeders, a connector and its locknut at each end: {lines:?}");
    // Shown, the design's boxes it replaces are hidden with its circuits' pieces.
    let shown = s.execute("ve.scenario", &json!({"id": "nm_unit_branch", "show": true})).expect("show");
    let hidden = s.doc().map(|d| d.model.iter().filter(|e| !e.common.visible).count()).unwrap_or(0);
    assert_eq!(hidden as u64, shown["replaces"].as_u64().unwrap_or(0));
    let _ = std::fs::remove_dir_all(&dir);
}
