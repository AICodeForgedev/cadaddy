//! Procurement from the model (Q333): releases from the bill of materials sum to it; gear whose
//! lead time runs past its 4D need date is flagged late; a transformer released without its
//! separately supplied lug kit is refused (or flagged when allowed); spares by rule; submittals,
//! order status and exports.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn spec() -> Value {
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}, {"name": "L2", "elevation_ft": 12, "height_ft": 12}],
        "mep": {
            "systems": [{"name": "SA", "layer": "M-DUCT-SA", "kind": "duct", "size_in": [12, 12], "bottom_ft": 9}],
            "runs": [
                {"system": "SA", "level": "L1", "points": [[0, 0], [480, 0]], "size_in": [12, 12], "bottoms_ft": [9]},
                {"system": "SA", "level": "L2", "points": [[0, 0], [360, 0]], "size_in": [12, 12], "bottoms_ft": [9]}
            ],
            "equipment": [
                {"tag": "MDP", "kind": "switchboard", "level": "L1", "min": [600, 0], "max": [672, 36], "bottom_ft": 0, "height_ft": 7.5},
                {"tag": "TX-1", "kind": "transformer", "level": "L1", "min": [700, 0], "max": [718, 12], "bottom_ft": 0, "height_ft": 2.5,
                 "terminations": [{"point": "secondary", "lugs": 1, "range": "#6-250 kcmil", "lug_type": "compression", "supplied": false, "bolt_in": 0.375, "holes": 1, "stack_in": 0.5}]},
                {"tag": "P1", "kind": "panel", "level": "L1", "min": [740, 0], "max": [758, 6], "bottom_ft": 1.5, "height_ft": 5},
                {"tag": "P2", "kind": "panel", "level": "L2", "min": [740, 0], "max": [758, 6], "bottom_ft": 1.5, "height_ft": 5}
            ],
            "feeders": [
                {"tag": "F-TX1", "from": "MDP", "to": "TX-1", "size": "#6", "conduit_in": 1},
                {"tag": "F-P1", "from": "TX-1", "to": "P1", "size": "3/0", "conduit_in": 2}
            ]
        }
    })
}

fn session() -> Session {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec()})).unwrap();
    s
}

fn bom(s: &mut Session) -> Vec<Value> {
    s.execute("bim.bom", &json!({"group_by": "level"})).unwrap()["lines"].as_array().unwrap().clone()
}

#[test]
fn releases_sum_to_the_bom_with_spares_on_top() {
    let mut s = session();
    let lines = bom(&mut s);
    assert!(lines.iter().any(|l| l["item"].as_str().unwrap().starts_with("Lug kit TX-1 secondary")), "the BOM carries the lug kit: {lines:#?}");
    let duct = lines
        .iter()
        .find(|l| l["level"] == "L1" && l["category"].as_str().unwrap().contains("duct"))
        .cloned()
        .unwrap_or_else(|| panic!("{lines:#?}"));
    let duct_cat = duct["category"].as_str().unwrap().to_string();
    s.execute("project.rules.set", &json!({"domain": "custom", "key": format!("spares.{duct_cat}"), "value": "10"})).unwrap();
    // Floor by floor, then the rest of the job.
    let r1 = s.execute("procure.release", &json!({"name": "Level 1", "level": "L1"})).unwrap();
    assert!(r1["unlinked"].as_array().unwrap().is_empty(), "{r1}");
    s.execute("procure.release", &json!({"name": "Level 2", "level": "L2"})).unwrap();
    let rest = s.execute("procure.release", &json!({"name": "Job-wide"}));
    let l = s.execute("procure.list", &json!({"today": "2026-10-21"})).unwrap();
    assert_eq!(l["sums_to_bom"], true, "{l}");
    assert!(l["unreleased"].as_array().unwrap().is_empty());
    // Exactly: every BOM line's need is in one release.
    let mut released: std::collections::BTreeMap<String, f64> = Default::default();
    for r in l["releases"].as_array().unwrap() {
        for x in r["lines"].as_array().unwrap() {
            *released.entry(x["key"].as_str().unwrap().to_string()).or_default() += x["need"].as_f64().unwrap();
        }
    }
    for b in &lines {
        let k = format!(
            "{}|{}|{}|{}|{}",
            b["trade"].as_str().unwrap(),
            b["category"].as_str().unwrap(),
            b["item"].as_str().unwrap(),
            b["size"].as_str().unwrap(),
            b["level"].as_str().unwrap_or("")
        );
        assert!((released.get(&k).copied().unwrap_or(0.0) - b["net"].as_f64().unwrap()).abs() < 1e-6, "{k}");
    }
    // Spares by rule on top of the exact need (10% of the L1 duct).
    let rel1 = &l["releases"][0];
    let d1 = rel1["lines"].as_array().unwrap().iter().find(|x| x["item"] == duct["item"] && x["level"] == "L1").unwrap();
    assert!((d1["spares"].as_f64().unwrap() - (duct["net"].as_f64().unwrap() * 0.1 * 10.0).round() / 10.0).abs() < 1e-6, "{d1}");
    assert_eq!(d1["qty"].as_f64().unwrap(), d1["need"].as_f64().unwrap() + d1["spares"].as_f64().unwrap());
    // Nothing left: another release is refused; a name can't repeat.
    assert!(rest.is_err() || l["releases"].as_array().unwrap().len() == 3);
    assert!(s.execute("procure.release", &json!({"name": "Again"})).is_err());
    assert!(s.execute("procure.release", &json!({"name": "Level 1", "level": "L2"})).is_err());
}

#[test]
fn gear_released_without_its_lug_kit_is_refused_or_flagged() {
    let mut s = session();
    // The transformer and the panels without the lug kit category: refused.
    let e = s.execute("procure.release", &json!({"name": "Gear", "level": "L1", "categories": ["equipment"]})).unwrap_err().to_string();
    assert!(e.contains("transformer TX-1 is released without Lug kit TX-1 secondary"), "{e}");
    // Allowed by hand: made, and flagged.
    let r = s.execute("procure.release", &json!({"name": "Gear", "level": "L1", "categories": ["equipment"], "allow_unlinked": true})).unwrap();
    assert_eq!(r["unlinked"].as_array().unwrap().len(), 1, "{r}");
    let l = s.execute("procure.list", &json!({})).unwrap();
    assert!(l["releases"][0]["unlinked"][0].as_str().unwrap().contains("(they arrive together: TX-1)"));
    // The lug kit alone later: still apart from its (already released) gear, but nothing of the
    // group is left to hold back, so it goes.
    let k = s.execute("procure.release", &json!({"name": "Lug kits", "categories": ["lug kits"]})).unwrap();
    assert!(k["unlinked"].as_array().unwrap().is_empty(), "{k}");
    // Released together from the start: no flag.
    let mut s = session();
    let r = s.execute("procure.release", &json!({"name": "Gear and kits", "level": "L1", "categories": ["equipment", "lug kits"]})).unwrap();
    assert!(r["unlinked"].as_array().unwrap().is_empty());
}

#[test]
fn a_late_lead_time_item_is_flagged_against_the_4d_schedule() {
    let mut s = session();
    s.execute(
        "fourd.import",
        &json!({"rows": [["Task", "Start", "Finish", "Level"], ["L1 electrical rough-in", "2026-12-01", "2027-01-15", "L1"], ["L2 ductwork", "2028-03-01", "2028-04-01", "L2"]]}),
    )
    .unwrap();
    s.execute("procure.release", &json!({"name": "Level 1", "level": "L1"})).unwrap();
    s.execute("procure.release", &json!({"name": "Level 2", "level": "L2"})).unwrap();
    let l = s.execute("procure.list", &json!({"today": "2026-10-21"})).unwrap();
    let line = |r: usize, item: &str| {
        l["releases"][r]["lines"].as_array().unwrap().iter().find(|x| x["item"].as_str().unwrap().contains(item)).cloned().unwrap()
    };
    // The switchboard: 60 weeks back from 12-01 is long past.
    let mdp = line(0, "switchboard MDP");
    assert_eq!((mdp["class"].as_str(), mdp["need_date"].as_str()), (Some("gear"), Some("2026-12-01")));
    assert_eq!(mdp["order_by"], "2025-09-23", "60 weeks and 14 days of submittal review before 2026-12-01");
    assert!(mdp["flags"][0].as_str().unwrap().starts_with("late: order by 2025-09-23"), "{mdp}");
    assert!(l["releases"][0]["late"].as_u64().unwrap() >= 1);
    // L2's duct is needed in 2028: in time.
    let duct = l["releases"][1]["lines"].as_array().unwrap().iter().find(|x| x["category"].as_str().unwrap().contains("duct")).cloned().unwrap();
    assert!(!duct["flags"].as_array().unwrap().iter().any(|f| f.as_str().unwrap().starts_with("late")), "{duct}");
    // Company lead times and an approved submittal move the date.
    s.execute("procure.leadtimes", &json!({"set": [{"class": "gear", "weeks_min": 4, "weeks_max": 6}]})).unwrap();
    s.execute("procure.submittal", &json!({"item": "gear", "spec_section": "26 24 13", "status": "approved", "submitted": "2026-09-01", "returned": "2026-09-20", "reviewer": "Engineer", "comment": "Approved as submitted."})).unwrap();
    let l = s.execute("procure.list", &json!({"today": "2026-10-21"})).unwrap();
    let mdp = l["releases"][0]["lines"].as_array().unwrap().iter().find(|x| x["item"] == "switchboard MDP").cloned().unwrap();
    assert_eq!((mdp["order_by"].as_str(), mdp["submittal"].as_str()), (Some("2026-10-20"), Some("approved")));
    assert!(mdp["flags"][0].as_str().unwrap().starts_with("late"), "a day late: {mdp}");
    let log = s.execute("procure.submittals", &json!({})).unwrap();
    assert_eq!(log["submittals"][0]["comments"], json!(["Approved as submitted."]));
    // Ordered, then short on arrival.
    s.execute("procure.status", &json!({"release": "Level 1", "item": "switchboard MDP", "status": "ordered"})).unwrap();
    let l = s.execute("procure.list", &json!({"today": "2026-10-21"})).unwrap();
    let mdp = l["releases"][0]["lines"].as_array().unwrap().iter().find(|x| x["item"] == "switchboard MDP").cloned().unwrap();
    assert!(mdp["flags"].as_array().unwrap().is_empty(), "ordered: no longer late to order: {mdp}");
    s.execute("procure.status", &json!({"release": "R002", "item": "duct", "status": "short", "received": 10})).unwrap();
    let l = s.execute("procure.list", &json!({})).unwrap();
    assert!(
        l["releases"][1]["lines"].as_array().unwrap().iter().any(|x| x["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap().starts_with("short: 10 of")))
    );
    // Exports.
    let dir = std::env::temp_dir().join(format!("cadaddy-procure-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("r1.csv");
    s.execute("procure.export", &json!({"release": "R001", "path": csv.display().to_string(), "today": "2026-10-21"})).unwrap();
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with("Item,Size,Level,Unit,Need,Spares,Qty,Class,Lead (wk),Need date,Order by,Submittal,Status,Flags"), "{text}");
    assert!(text.contains("switchboard MDP"));
    let xlsx = dir.join("r1.xlsx");
    s.execute("procure.export", &json!({"release": "Level 1", "path": xlsx.display().to_string()})).unwrap();
    assert!(std::fs::read(&xlsx).unwrap().starts_with(b"PK"));
    assert!(
        s.execute("procure.export", &json!({"release": "Level 1", "path": dir.join("r.pdf").display().to_string()}))
            .unwrap_err()
            .to_string()
            .contains("PDF export isn't available")
    );
}

#[test]
fn bad_requests_are_refused() {
    let mut s = session();
    for (c, p) in [
        ("procure.release", json!({})),
        ("procure.release", json!({"name": "x", "level": "L9"})),
        ("procure.status", json!({"release": "R001", "item": "x", "status": "lost"})),
        ("procure.submittal", json!({"item": "gear", "status": "maybe"})),
        ("procure.submittal", json!({})),
        ("procure.leadtimes", json!({"set": [{"class": "gear", "weeks_min": 9, "weeks_max": 2}]})),
        ("procure.leadtimes", json!({"set": [{"class": "gear"}]})),
        ("procure.export", json!({"release": "nope", "path": "x.xlsx"})),
    ] {
        assert!(s.execute(c, &p).is_err(), "{c} {p}");
    }
    assert!(Session::new().execute("procure.list", &json!({})).is_err());
}
