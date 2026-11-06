//! Electrical gear review (Q290): a made-up service — switchboard MDP, transformer TX-1,
//! distribution panel DPA with twenty-four feeders out, panel P-1 — with problems planted where the
//! field finds them: a neutral bar short for the parallel neutrals, a breaker lug too small for
//! its feeder, aluminium on CU-only lugs, compression lugs on the transformer's secondary that
//! aren't supplied with it, and a distribution panel too small for what comes into it.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn lug(point: &str, lugs: u32, range: &str, material: &str) -> Value {
    json!({"point": point, "lugs": lugs, "range": range, "material": material})
}

fn spec() -> Value {
    let mut feeders = vec![
        // The service: four sets of 600 kcmil AL from below, with the grounding electrode
        // conductor and the main bonding jumper on the ground bar.
        json!({"tag": "F-SVC", "from": "", "to": "MDP", "sets": 4, "size": "600 kcmil", "material": "al", "neutrals": 1, "grounds": 0, "ground_size": "", "extra_grounds": ["3/0", "#2"], "conduits": 4, "conduit_in": 4, "entry": "bottom"}),
        json!({"tag": "F-TX1", "from": "MDP", "to": "TX-1", "sets": 1, "size": "250 kcmil", "material": "cu", "neutrals": 0, "grounds": 1, "ground_size": "#4", "conduits": 1, "conduit_in": 2.5, "entry": "top"}),
        json!({"tag": "F-DPA", "from": "TX-1", "to": "DPA", "sets": 2, "size": "350 kcmil", "material": "al", "neutrals": 1, "grounds": 1, "ground_size": "1/0", "conduits": 2, "conduit_in": 4, "entry": "top"}),
        json!({"tag": "F-P1", "from": "DPA", "to": "P-1", "sets": 1, "size": "#2", "material": "al", "neutrals": 1, "grounds": 1, "ground_size": "#8", "conduits": 1, "conduit_in": 1.25, "entry": "top"}),
    ];
    for k in 1..=24 {
        feeders.push(json!({"tag": format!("F-B{k:02}"), "from": "DPA", "to": "", "sets": 1, "size": "#6", "material": "cu", "neutrals": 1, "grounds": 1, "ground_size": "#10", "conduits": 1, "conduit_in": 1, "entry": "top"}));
    }
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14}],
        "mep": {
            "equipment": [
                {"tag": "MDP", "kind": "switchboard", "level": "L1", "min": [0, 0], "max": [72, 36], "bottom_ft": 0, "height_ft": 7.5,
                 "properties": {"VOLTAGE": "480Y/277V", "AMPS": "2000A"},
                 "terminations": [lug("main", 4, "#2-600 kcmil", "cu/al"), lug("breaker", 1, "#6-4/0", "cu/al"), lug("neutral_bar", 2, "#6-600 kcmil", "cu/al"), lug("ground_bar", 8, "#14-4/0", "cu/al")],
                 "enclosure": {"top_entry_in": [60, 20], "bottom_entry_in": [60, 20], "gutter_in": [12, 20]}},
                {"tag": "TX-1", "kind": "transformer", "level": "L1", "min": [120, 0], "max": [156, 30], "bottom_ft": 0, "height_ft": 5,
                 "terminations": [lug("primary", 1, "#6-250 kcmil", "cu/al"),
                                  {"point": "secondary", "lugs": 2, "range": "1/0-750", "material": "cu/al", "lug_type": "compression", "supplied": false, "bolt_in": 0.5, "holes": 2, "stack_in": 0.75},
                                  lug("ground_bar", 4, "#6-250 kcmil", "cu/al")]},
                {"tag": "DPA", "kind": "panel", "level": "L1", "min": [200, 0], "max": [220, 6], "bottom_ft": 1, "height_ft": 6,
                 "terminations": [{"point": "main", "lugs": 2, "range": "#2-600 kcmil", "material": "cu/al", "bending_in": 8, "opposite": true},
                                  lug("breaker", 1, "#14-1/0", "cu/al"), lug("neutral_bar", 24, "#14-600 kcmil", "cu/al"), lug("ground_bar", 40, "#14-1/0", "cu/al")],
                 "enclosure": {"top_entry_in": [18, 5], "bottom_entry_in": [0, 0], "gutter_in": [4, 4]}},
                {"tag": "P-1", "kind": "panel", "level": "L1", "min": [240, 0], "max": [260, 6], "bottom_ft": 1, "height_ft": 4,
                 "terminations": [lug("main", 1, "#6-2/0", "cu")]}
            ],
            "feeders": feeders
        }
    })
}

fn session() -> Session {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec()})).unwrap();
    s
}

/// The findings of `check` on `gear`.
fn found<'a>(r: &'a Value, gear: &str, check: &str) -> Vec<&'a Value> {
    r["findings"].as_array().unwrap().iter().filter(|f| f["gear"] == gear && f["check"] == check).collect()
}

#[test]
fn the_review_finds_what_the_field_would() {
    let mut s = session();
    let list = s.execute("gear.list", &json!({})).unwrap();
    assert_eq!(list["gear"].as_array().unwrap().len(), 4);
    assert_eq!(list["gear"][2]["feeders_out"].as_array().unwrap().len(), 25, "{list}");
    let r = s.execute("gear.check", &json!({})).unwrap();
    let all = |sev: &str| {
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["severity"] == sev)
            .map(|f| format!("{} {} {}", f["gear"], f["check"], f["feeder"]))
            .collect::<Vec<_>>()
    };
    println!("errors: {:#?}\nwarnings: {:#?}", all("error"), all("warning"));
    // The service's neutrals: four sets on a two-position bar.
    let n = found(&r, "MDP", "neutral bar");
    assert_eq!(n.len(), 1, "{r}");
    assert_eq!((n[0]["needed"].as_str(), n[0]["available"].as_str()), (Some("4 positions"), Some("2 positions")));
    // The ground bar takes the EGCs, the GEC and the bonding jumper: 1 + 2 = 3 of 8. Fine.
    assert!(found(&r, "MDP", "ground bar").is_empty());
    // A breaker lug too small for the transformer's feeder.
    let range = found(&r, "MDP", "lug range");
    assert_eq!(range.len(), 1);
    assert_eq!(range[0]["feeder"], "F-TX1");
    // Aluminium on P-1's CU-only main lugs.
    let al = found(&r, "P-1", "lug material");
    assert_eq!((al.len(), al[0]["feeder"].as_str()), (1, Some("F-P1")));
    // DPA: neutral bar short (2 parallel neutrals + 25 feeders out = 27 of 24), the top entry
    // plate (2 × 4" + 24 × 1" + 1 × 1-1/4"), the gutter, and the bending space at the main lugs.
    let nb = found(&r, "DPA", "neutral bar");
    assert_eq!(nb.len(), 1);
    assert_eq!(nb[0]["needed"], "27 positions");
    assert_eq!(found(&r, "DPA", "entry").len(), 1);
    assert!(found(&r, "DPA", "entry")[0]["message"].as_str().unwrap().contains("27 raceways"), "{r}");
    assert_eq!(found(&r, "DPA", "gutter").len(), 1);
    let bend = found(&r, "DPA", "bending");
    assert_eq!((bend.len(), bend[0]["needed"].as_str(), bend[0]["available"].as_str()), (1, Some("12\""), Some("8\"")));
    for f in r["findings"].as_array().unwrap().iter().filter(|f| f["severity"] == "error") {
        assert!(!f["fix"].as_str().unwrap().is_empty(), "every failure says what would fix it: {f}");
    }
    // What's fine stays quiet: the MDP's bottom entry (4 × 4"), the transformer's lugs, DPA's main lug count.
    for (g, c) in [("MDP", "entry"), ("TX-1", "lug count"), ("TX-1", "lug range"), ("DPA", "lug count"), ("MDP", "lug count")] {
        assert!(found(&r, g, c).is_empty(), "{g} {c}: {r}");
    }
    assert_eq!(r["errors"], 7, "{:#?}", all("error"));
    // The transformer's secondary lugs come separately: the kit, with its bolts.
    let kits = r["lug_kits"].as_array().unwrap();
    assert_eq!(kits.len(), 1, "{r}");
    assert_eq!(kits[0]["gear"], "TX-1");
    assert_eq!(kits[0]["lugs"], 6);
    assert_eq!(kits[0]["lug"], "compression 350 kcmil, 2-hole, AL");
    assert_eq!(kits[0]["bolts"], 12);
    assert_eq!(kits[0]["bolt"], "1/2\" x 1-3/4\" SAE grade 5");
    assert_eq!((kits[0]["nuts"].as_u64(), kits[0]["flat_washers"].as_u64(), kits[0]["belleville_washers"].as_u64()), (Some(12), Some(24), Some(12)));
    // One gear only.
    let one = s.execute("gear.check", &json!({"tag": "p-1"})).unwrap();
    assert_eq!(one["errors"], 1);
    assert!(s.execute("gear.check", &json!({"tag": "NOPE"})).is_err());
}

#[test]
fn failures_become_rfis_lug_kits_go_on_the_takeoff_and_accepted_gear_costs_more_to_install() {
    let mut s = session();
    let r = s.execute("gear.review", &json!({})).unwrap();
    let ids: Vec<String> = r["rfis"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(ids.len(), 7, "{r}");
    let rfis = s.execute("rfi.list", &json!({})).unwrap();
    // (The list numbers them its own way: RFI-001…)
    let ours: Vec<&Value> = rfis["items"].as_array().unwrap().iter().filter(|i| i["kind"].as_str().is_some_and(|k| k.starts_with("gear_"))).collect();
    assert_eq!(ours.len(), 7, "{rfis}");
    assert!(ours.iter().all(|i| i["severity"] == "high"));
    assert_eq!(ours.iter().filter(|i| i["kind"] == "gear_install").count(), 3, "entry, gutter, bending");
    assert!(ours.iter().all(|i| i["issue"].as_str().unwrap().starts_with("Submittal review: ") && i["proposed"].is_string()));
    // Again: nothing new.
    assert_eq!(s.execute("gear.review", &json!({})).unwrap()["rfis"].as_array().unwrap().len(), 0);
    // The takeoff carries the lug kit as required items.
    let q = s.execute("mep.qty", &json!({})).unwrap();
    assert_eq!(q["lug_kits"].as_array().unwrap().len(), 1);
    let rows: Vec<Vec<&str>> = q["csv"].as_str().unwrap().lines().filter(|l| l.starts_with("Lug kit,")).map(|l| l.split(',').collect()).collect();
    assert_eq!(rows.len(), 5, "lugs, bolts, nuts, flat and Belleville washers");
    assert_eq!((rows[0][1], rows[0][2]), ("L1", "TX-1 secondary"));
    assert_eq!((rows[2][3], rows[2][6], rows[3][3], rows[3][6]), ("nut", "12", "flat washer", "24"), "{rows:?}");
    assert!(q["csv"].as_str().unwrap().contains("\"lug, compression 350 kcmil, 2-hole, AL\",,,6,ea"), "{}", q["csv"]);
    assert!(q["csv"].as_str().unwrap().contains("\"bolt, 1/2\"\" x 1-3/4\"\" SAE grade 5\",,,12,ea"), "{}", q["csv"]);
    // DPA accepted as submitted for its entry and gutter: a labor condition.
    s.execute("gear.accept", &json!({"tag": "DPA", "check": "entry"})).unwrap();
    let a = s.execute("gear.accept", &json!({"tag": "DPA", "check": "gutter"})).unwrap();
    assert_eq!(a["labor"], json!([{"gear": "DPA", "conditions": ["entry", "gutter"], "factor": 1.32}]));
    let r = s.execute("gear.check", &json!({"tag": "DPA"})).unwrap();
    assert_eq!(found(&r, "DPA", "entry")[0]["severity"], "accepted");
    let q = s.execute("mep.qty", &json!({})).unwrap();
    assert_eq!(q["labor_conditions"][0]["factor"], 1.32);
    // Undo the acceptance; undo the command itself.
    s.execute("gear.accept", &json!({"tag": "DPA", "check": "gutter", "undo": true})).unwrap();
    assert_eq!(s.execute("gear.check", &json!({"tag": "DPA"})).unwrap()["labor"][0]["factor"], 1.15);
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.execute("gear.check", &json!({"tag": "DPA"})).unwrap()["labor"][0]["factor"], 1.32);
    for p in [json!({"tag": "DPA", "check": "looks"}), json!({"tag": "NOPE", "check": "entry"}), json!({"check": "entry"})] {
        assert!(s.execute("gear.accept", &p).is_err(), "{p}");
    }
}

#[test]
fn the_gear_and_its_incoming_raceways_build_and_export() {
    let mut s = session();
    let b = s.execute("model.build", &json!({})).unwrap();
    let mep = b.get("mep").cloned().unwrap_or_else(|| b.clone());
    println!("{b}");
    assert_eq!(mep["mepEquipment"], 4, "{b}");
    // F-SVC 4 at MDP; F-TX1 1 + 1; F-DPA 2 + 2; F-P1 1 + 1; twenty-four branch feeders at DPA.
    assert_eq!(mep["feederRaceways"], 36, "{b}");
    if let Ok(dir) = std::env::var("GEAR_SHOTS") {
        let png = std::path::Path::new(&dir).join("gear-3d.png");
        s.execute(
            "model.render",
            &json!({"path": png.display().to_string(), "style": "shaded", "width": 900, "height": 600, "around": [-24, -24, 300, 60]}),
        )
        .unwrap();
    }
}

#[test]
fn gear_rules_from_the_spec_and_hostile_input() {
    let mut s = session();
    // A project whose gutters may fill to 90% and whose bending table is shallower: DPA passes both.
    let mut sp = spec();
    sp["mep"]["gear_rules"] = json!({"gutter_fill": 0.9, "spacing_in": 0.25, "bending_opposite": [["600 kcmil", [8, 8, 8, 8, 8]]]});
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).unwrap();
    let r = s.execute("gear.check", &json!({"tag": "DPA"})).unwrap();
    assert!(found(&r, "DPA", "gutter").is_empty(), "{r}");
    assert!(found(&r, "DPA", "bending").is_empty(), "{r}");
    // Hostile: huge counts, NaN sizes, junk ranges and rules don't panic.
    let mut sp = spec();
    sp["mep"]["feeders"] = json!([
        {"tag": "X", "from": "DPA", "to": "P-1", "sets": 4_000_000_000u64, "size": "∞", "conduits": 4_000_000_000u64, "conduit_in": 1e308, "entry": "sideways"},
        {"tag": "Y", "from": "P-1", "to": "DPA", "sets": 0, "size": "#2", "neutral_size": "-1", "extra_grounds": vec!["x"; 100]}
    ]);
    sp["mep"]["equipment"][2]["terminations"][0]["range"] = json!("750-1/0");
    sp["mep"]["gear_rules"] = json!({"conduit_od": "big", "areas": [[1, 2]], "bending_adjacent": [["#2", [-5, "x"]]], "gutter_fill": -1});
    let _ = s.execute("bspec.set", &json!({"replace": true, "spec": sp}));
    for c in ["gear.list", "gear.check", "gear.review", "mep.qty", "model.build"] {
        let _ = s.execute(c, &json!({}));
    }
    let mut empty = Session::new();
    assert!(empty.execute("gear.check", &json!({})).is_err(), "no building spec");
}
