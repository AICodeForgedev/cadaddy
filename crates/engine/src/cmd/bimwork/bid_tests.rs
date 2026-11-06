//! Tests of Q6196 bidpackage: the bid package from the model (`bim.bid`): the takeoff by system and floor,
//! material from a price-book workbook with its quotes, labor hours from labor-unit data with
//! their source, allowances and markups as their own lines, the value-engineering alternates,
//! inclusions, exclusions and clarifications and the Web Prices workbook (the letter, plotted by
//! the app's plotter, is tested in `tests/bidpackage_letter.rs`).

use crate::Session;
use crate::doc::Drawing;
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

/// A luminaire of type A on `circuit`.
fn fixture(n: usize, x: f64, y: f64, circuit: &str) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": "luminaire", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"}
    }})
}

/// Another device of `kind`, on `circuit` when one is given.
fn device(n: usize, kind: &str, x: f64, y: f64, circuit: &str) -> Value {
    let mut v = json!({"id": format!("device:L1:{n}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": {}});
    if !circuit.is_empty() {
        v["fields"]["circuit"] = json!({"value": circuit, "source": "plan"});
    }
    v
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
        circuit("circuit:L1:E-101:LP1-5", "LP1-5", "LP1"),
        fixture(8, 20.0, -30.0, "LP1-5"),
        fixture(9, 30.0, -30.0, "LP1-5"),
        fixture(10, 40.0, -30.0, "LP1-5"),
        device(11, "switch", 18.0, -24.0, "LP1-5"),
        device(12, "smoke_detector", 25.0, -28.0, ""),
    ];
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
                {"panel": "LP1", "circuits": [{"number": "1", "description": "RECEPTACLES", "breaker": "20", "poles": 1}, {"number": "3", "description": "RECEPTACLES", "breaker": "20", "poles": 1}, {"number": "5", "description": "LIGHTING", "breaker": "20", "poles": 1}]}
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
                let p = crate::doc::props::read(&e.common.xdata);
                p.get("circuit").map(|x| x.text()).as_deref() == Some("LP1-3") && p.get("atom").is_none()
            })
            .map(|e| e.handle)
            .collect();
        for h in hs {
            let _ = d.modify_entity(h, |e| {
                let mut p = crate::doc::props::read(&e.common.xdata);
                p.set("room", crate::doc::props::Prop::new("ELEC 105", crate::doc::props::Source::computed()));
                e.common.xdata = crate::doc::props::write(&e.common.xdata, &p).into();
            });
        }
    }
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    std::fs::write(dir.join("260519 - Conductors.txt"), SPEC).expect("spec text");
    s
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-bid-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn row(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|c| (*c).to_string()).collect()
}

/// A made-up price book in the Web Prices layout: conduit, wire, receptacles (an old quote),
/// the luminaire by its Item Map row, and the panels excluded (furnished by the owner).
fn book(dir: &std::path::Path) -> String {
    let prices = vec![
        row(&["Key", "Description", "Catalog", "Unit", "Unit Price $", "Source Type", "Vendor", "URL", "Price Date"]),
        row(&["emt34", "3/4 EMT", "EMT-075", "ft", "0.80", "WEB", "Vendor X", "", "2026-10-20"]),
        row(&["emt2", "2 EMT", "EMT-200", "ft", "3.10", "WEB", "Vendor X", "", "2026-10-20"]),
        row(&["thhn10", "#10 THHN", "T10", "ft", "0.30", "QUOTE", "Vendor Y", "", "2026-10-25"]),
        row(&["thhn8", "#8 THHN", "T8", "ft", "0.45", "QUOTE", "Vendor Y", "", "2026-10-25"]),
        row(&["rcpt", "duplex receptacle", "R-15", "ea", "3.00", "MCC", "Vendor Z", "", "2026-01-02"]),
        row(&["fxA", "type A luminaire", "LUM-A", "ea", "120.00", "QUOTE", "Vendor X", "", "2026-10-30"]),
        row(&["sw", "single pole switch", "SW-1", "ea", "2.50", "WEB", "Vendor X", "", "2026-10-30"]),
    ];
    let map = vec![
        row(&["CADaddy Item", "Size", "Action", "Price Key 1", "Price Key 2", "Note"]),
        row(&["EMT conduit", "3/4\"", "price", "emt34", "", ""]),
        row(&["EMT conduit", "2\"", "price", "emt2", "", ""]),
        row(&["THHN copper", "#10", "price", "thhn10", "", ""]),
        row(&["THHN copper", "#8", "price", "thhn8", "", ""]),
        row(&["receptacle", "", "price", "rcpt", "", ""]),
        row(&["Luminaire type A", "", "price", "fxA", "", ""]),
        row(&["switch", "", "price", "sw", "", ""]),
        row(&["panel LP1", "", "exclude", "", "", "owner-furnished panelboard"]),
    ];
    let bytes = crate::cmd::table::xlsx::write_sheets(&[("Price List".into(), prices), ("Item Map".into(), map)]).expect("book");
    let path = dir.join("Web Prices.xlsx");
    std::fs::write(&path, bytes).expect("write book");
    path.to_string_lossy().into_owned()
}

/// A made-up labor-unit table: per item and size, per 100 ft, a whole category, and a row with
/// no source (not used).
const LABOR: &str = "item,category,size,unit,hours,per,source\nEMT conduit,,3/4\",ft,5,100,Company labor table p.3\nEMT conduit,,2\",ft,9,100,Company labor table p.3\nreceptacle,,,each,0.5,1,Company labor table p.7\nLuminaire type A,,,each,1.2,1,Company labor table p.11\n,wire,,ft,0.8,100,Company labor table p.9\nswitch,,,each,0.4,1,\n";

fn bid(s: &mut Session, dir: &std::path::Path, extra: Value) -> Value {
    let labor = dir.join("labor.csv");
    std::fs::write(&labor, LABOR).expect("labor");
    let mut p = json!({
        "price_book": book(dir), "labor_units": labor.to_string_lossy(), "labor_rate": 80, "today": "2026-11-04",
        "markups": {"tax_pct": 8.25, "overhead_pct": 10, "profit_pct": 5, "bond_pct": 1},
        "allowances": [{"name": "Temporary power", "amount": 500, "note": "per the general conditions"}],
        "show_lines": true, "alternates": false,
    });
    if let (Some(m), Some(x)) = (p.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            m.insert(k.clone(), v.clone());
        }
    }
    s.execute("bim.bid", &p).expect("bid")
}

fn lines(v: &Value) -> Vec<Value> {
    v["lines"].as_array().cloned().unwrap_or_default()
}

fn find(v: &Value, item: &str, size: &str, circuit: Option<&str>) -> Value {
    lines(v)
        .into_iter()
        .find(|l| l["item"] == json!(item) && l["size"] == json!(size) && circuit.is_none_or(|c| l["circuit"] == json!(c)))
        .unwrap_or(Value::Null)
}

fn money(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

#[test]
fn the_bid_rolls_the_takeoff_up_by_system_and_floor_every_number_from_its_lines() {
    let dir = tmp("roll");
    let mut s = building(&dir);
    let v = bid(&mut s, &dir, json!({}));
    let all = lines(&v);
    assert!(!all.is_empty(), "{v}");
    // Systems: devices by their words; raceway, wire and boxes by the devices on their circuit;
    // feeder raceway and conductors on no circuit to gear.
    assert_eq!(find(&v, "Luminaire type A", "", None)["system"], json!("lighting"));
    assert_eq!(find(&v, "switch", "", None)["system"], json!("lighting"));
    let plate = find(&v, "Device plate", "1 gang", Some("LP1-5"));
    assert_eq!((plate["system"].clone(), plate["system_why"].as_str().map(|w| w.contains("LP1-5"))), (json!("lighting"), Some(true)), "{plate}");
    assert_eq!(find(&v, "THHN copper", "#10", Some("LP1-1"))["system"], json!("power"));
    assert_eq!(find(&v, "EMT conduit", "2\"", None)["system"], json!("gear"));
    // The aluminum feeder's conductors along its routes (Q6201: its schedule's material).
    assert_eq!(find(&v, "XHHW aluminum", "#2/0", None)["system"], json!("gear"));
    for l in &all {
        assert!(!l["system_why"].as_str().unwrap_or_default().is_empty(), "every line says why: {l}");
    }
    // Material: the price book's quote × the quantity bought (waste in); labor on the installed
    // quantity at the table's hours (per 100 ft), with its source.
    let emt = find(&v, "EMT conduit", "3/4\"", Some("LP1-1"));
    assert_eq!(
        (emt["price_keys"].clone(), emt["vendor"].clone(), emt["price_date"].clone()),
        (json!(["emt34"]), json!("Vendor X"), json!("2026-10-20"))
    );
    assert!((money(&emt["material"]) - 0.8 * money(&emt["qty_bought"])).abs() < 0.01, "{emt}");
    assert!((money(&emt["labor_unit_hours"]) - 0.05).abs() < 1e-9 && emt["labor_source"] == json!("labor units: Company labor table p.3"), "{emt}");
    assert!((money(&emt["labor_hours"]) - 0.05 * money(&emt["qty"])).abs() < 0.001, "{emt}");
    let wire = find(&v, "Copper ground, green", "#10", Some("LP1-1"));
    assert_eq!(wire["labor_source"], json!("labor units: Company labor table p.9"), "a whole category's row: {wire}");
    // The row with no source isn't used: the switch's labor is the starter book's, marked as a
    // placeholder, and the unsourced row is reported.
    assert!(find(&v, "switch", "", None)["labor_source"].as_str().is_some_and(|x| x.contains("placeholder")));
    assert!(v["warnings"].as_array().unwrap().iter().any(|w| w["kind"] == json!("labor_units_unsourced")), "{}", v["warnings"]);
    // Nothing is priced at zero: unpriced lines and lines with no labor unit are listed.
    let un: Vec<&str> = v["unpriced"].as_array().unwrap().iter().filter_map(|l| l["item"].as_str()).collect();
    assert!(un.contains(&"Locknut") && !un.contains(&"EMT conduit"), "{un:?}");
    for l in &all {
        if l["status"] == json!("unpriced") {
            assert!(l["unit_price"].is_null() && money(&l["material"]) == 0.0 && !l["reason"].as_str().unwrap_or_default().is_empty(), "{l}");
        }
    }
    assert!(v["no_labor"].as_array().unwrap().iter().any(|l| l["item"] == json!("Locknut")));
    // The panel the price book excludes is out of the bid, with the Item Map's reason.
    assert_eq!(find(&v, "panel LP1", "1'-8\" x 1'-0\" x 4'-0\" H", None)["bid"], json!("EXCLUDED"));
    assert!(v["exclusions"].as_array().unwrap().iter().any(|e| e["text"].as_str().unwrap_or_default().contains("owner-furnished panelboard")));
    // Totals: material, labor at the rate, the allowance, tax on material, then overhead, profit
    // and bond compounding, each its own line.
    let base: Vec<&Value> = all.iter().filter(|l| l["bid"] == json!("BASE")).collect();
    let mat: f64 = base.iter().map(|l| money(&l["material"])).sum();
    let hours: f64 = base.iter().map(|l| money(&l["labor_hours"])).sum();
    let bb = &v["base_bid"];
    assert!((money(&bb["material"]) - mat).abs() < 0.05 && (money(&bb["labor_hours"]) - hours).abs() < 0.01, "{bb}");
    assert!((money(&bb["labor"]) - money(&bb["labor_hours"]) * 80.0).abs() < 0.05, "{bb}");
    assert_eq!((money(&bb["allowances"]), money(&bb["labor_rate"])), (500.0, 80.0));
    assert!((money(&bb["tax"]) - money(&bb["material"]) * 0.0825).abs() < 0.01, "{bb}");
    let direct = money(&bb["material"]) * 1.0825 + money(&bb["labor"]) + 500.0;
    assert!((money(&bb["direct"]) - direct).abs() < 0.02, "{bb}");
    let total = direct * 1.10 * 1.05 * 1.01;
    assert!((money(&bb["total"]) - total).abs() < 0.05, "{bb} vs {total}");
    for k in ["overhead", "profit", "bond"] {
        assert!(money(&bb[k]) > 0.0, "{k}: {bb}");
    }
    // By system and by floor: each sums to the bid to the cent; a system's lines are named.
    let cents = |x: &Value| (money(x) * 100.0).round() as i64;
    let sum = |k: &str| v[k].as_array().unwrap().iter().map(|g| cents(&g["share"])).sum::<i64>();
    assert_eq!((sum("by_system"), sum("by_floor")), (cents(&bb["total"]), cents(&bb["total"])));
    let general = v["by_system"].as_array().unwrap().iter().find(|g| g["system"] == json!("general")).cloned().unwrap_or(Value::Null);
    assert_eq!(general["allowances"], json!(500.0), "the allowance in its system");
    for g in v["by_system"].as_array().unwrap() {
        let ids: Vec<u64> = g["line_ids"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
        let m: f64 = all.iter().filter(|l| ids.contains(&l["line"].as_u64().unwrap_or(0))).map(|l| money(&l["material"])).sum();
        assert!((m - money(&g["material"])).abs() < 0.05, "{g}");
    }
    assert!(v["by_floor"].as_array().unwrap().iter().any(|f| f["level"] == json!("L1")));
    // The quotes used: vendor and date; the old one flagged.
    let q = v["quotes"].as_array().unwrap();
    let r = q.iter().find(|x| x["key"] == json!("rcpt")).unwrap();
    assert_eq!((r["vendor"].clone(), r["date"].clone(), r["stale"].clone()), (json!("Vendor Z"), json!("2026-01-02"), json!(true)));
    // Scope: what is in each system, the scope split's by-others work with its source, other
    // trades, open RFIs as clarifications, assumptions with their sources.
    let texts = |k: &str| v[k].as_array().unwrap().iter().map(|x| x["text"].as_str().unwrap_or_default().to_string()).collect::<Vec<_>>();
    assert!(texts("inclusions").iter().any(|t| t.starts_with("Lighting and lighting controls:")), "{:?}", texts("inclusions"));
    assert!(texts("inclusions").iter().any(|t| t.contains("Allowance: Temporary power $500.00")));
    assert!(texts("exclusions").iter().any(|t| t.contains("smoke detector (1): by the fire alarm contractor")), "{:?}", texts("exclusions"));
    assert!(v["exclusions"].as_array().unwrap().iter().all(|x| !x["source"].as_str().unwrap_or_default().is_empty()));
    assert!(texts("clarifications").iter().any(|t| t.contains("open RFIs")), "{:?}", texts("clarifications"));
    assert!(texts("assumptions").iter().any(|t| t.contains("Company labor table p.3")), "{:?}", texts("assumptions"));
    assert!(texts("assumptions").iter().any(|t| t.contains("Web Prices.xlsx")), "{:?}", texts("assumptions"));
    assert!(v["text"].as_str().unwrap().starts_with("# Bid summary"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_workbook_keeps_live_formulas_into_its_price_list_and_rates() {
    let dir = tmp("export");
    let mut s = building(&dir);
    let file = dir.join("bid.xlsx");
    let v = bid(&mut s, &dir, json!({"file": file.to_string_lossy()}));
    assert_eq!(v["path"].as_str(), Some(file.to_string_lossy().as_ref()));
    let wb = crate::cmd::table::xlsx::open(&std::fs::read(&file).unwrap()).unwrap();
    let names: Vec<&str> = wb.sheets.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Bid Summary", "Priced Takeoff", "Alternates", "Allowances", "Scope", "Quotes", "Price List", "Item Map"]);
    // The takeoff: unit $ is an INDEX/MATCH of the line's key into the Price List beside it;
    // material = bought × unit $; labor $ = labor h × the summary's rate cell.
    let cells = wb.values(Some("Priced Takeoff"), None).unwrap();
    let head = &cells[0];
    let at = |n: &str| head.iter().position(|h| h == n).unwrap();
    let r = cells.iter().position(|c| c[at("Item")] == "Luminaire type A").unwrap();
    assert_eq!((cells[r][at("Price key 1")].as_str(), cells[r][at("Unit $")].as_str(), cells[r][at("System")].as_str()), ("fxA", "120", "lighting"));
    let f = wb.formulas(Some("Priced Takeoff")).unwrap();
    let unit = f.iter().find(|(c, _)| *c == format!("T{}", r + 1)).map(|x| x.1.clone()).unwrap_or_default();
    assert!(unit.contains("INDEX('Price List'!$E:$E,MATCH($Q") && unit.contains("'Price List'!$A:$A,0)"), "{unit}");
    let ld = f.iter().find(|(c, _)| *c == format!("Y{}", r + 1)).map(|x| x.1.clone()).unwrap_or_default();
    assert_eq!(ld, format!("$X{}*'Bid Summary'!$B$3", r + 1));
    // The summary: editable inputs, SUMIFS by system, the base bid's value as computed.
    let sm = wb.values(Some("Bid Summary"), None).unwrap();
    assert_eq!((sm[2][0].as_str(), sm[2][1].as_str()), ("Labor rate $/h", "80"));
    assert_eq!((sm[3][0].as_str(), sm[3][1].as_str()), ("Tax % (on material)", "8.25"));
    let base = sm.iter().find(|r| r[0] == "BASE BID").unwrap();
    assert!((base[3].parse::<f64>().unwrap() - money(&v["base_bid"]["total"])).abs() < 0.01, "{base:?}");
    let sf = wb.formulas(Some("Bid Summary")).unwrap();
    assert!(sf.iter().any(|(_, f)| f.starts_with("SUMIFS('Priced Takeoff'!$U:$U,'Priced Takeoff'!$C:$C,$A")), "{sf:?}");
    let scope = wb.values(Some("Scope"), None).unwrap();
    assert!(scope.iter().any(|r| r[0] == "Exclusion" && r[1].contains("smoke detector")));
    // A file that exists is refused without force.
    assert!(s.execute("bim.bid", &json!({"file": file.to_string_lossy(), "alternates": false})).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_alternates_carry_their_add_or_deduct_from_value_engineering() {
    let dir = tmp("alt");
    let mut s = building(&dir);
    let spec = dir.join("260519 - Conductors.txt").to_string_lossy().to_string();
    let ve = json!({"specs": [spec], "code_texts": code_texts("V-A"), "code_sheet": "C-001"});
    // A cost DB pricing every line of both ways (labor in dollars, as such DBs keep it).
    let study = s.execute("ve.study", &json!({"model": false, "specs": ve["specs"], "code_texts": ve["code_texts"]})).unwrap();
    let mut csv = String::from("# price_date: 2026-10-30\n# manpower_rate: 50\nitem,size,unit,material_unit,labor_unit\n");
    for sc in study["scenarios"].as_array().into_iter().flatten() {
        for side in ["as_designed", "alternative"] {
            for l in sc[side]["lines"].as_array().into_iter().flatten() {
                let q = |k: &str| l[k].as_str().unwrap_or_default().replace('"', "\"\"");
                let (m, lab) = if side == "as_designed" { (2.0, 3.0) } else { (1.5, 1.0) };
                csv.push_str(&format!("\"{}\",\"{}\",{},{m},{lab}\n", q("item"), q("size"), l["unit"].as_str().unwrap_or("ea")));
            }
        }
    }
    let db = dir.join("cost.csv");
    std::fs::write(&db, csv).unwrap();
    let mut vp = ve.clone();
    vp["cost_db"] = json!(db.to_string_lossy());
    let v = bid(&mut s, &dir, json!({"alternates": true, "ve": vp}));
    let alts = v["alternates"].as_array().unwrap();
    assert!(!alts.is_empty(), "{v}");
    let f = 1.10 * 1.05 * 1.01;
    let mut priced = 0;
    for a in alts {
        assert!(matches!(a["tier"].as_str(), Some("allowed_now" | "needs_rfi")), "{a}");
        if let Some(x) = a["add_deduct"].as_f64() {
            priced += 1;
            let want = (money(&a["material"]) * 1.0825 + money(&a["labor"])) * f;
            assert!((x - want).abs() < 0.05, "{a}");
            assert!(x < 0.0, "the cheaper method is a deduct: {a}");
        }
    }
    assert!(priced > 0, "{alts:?}");
    assert!(alts.iter().any(|a| a["tier"] == json!("needs_rfi") && a["rfi"].as_str().is_some_and(|r| !r.is_empty())), "{alts:?}");
    // In the workbook, the add/deduct is a formula of the summary's rates.
    let file = dir.join("alt.xlsx");
    let _ = bid(&mut s, &dir, json!({"alternates": true, "ve": vp, "file": file.to_string_lossy()}));
    let wb = crate::cmd::table::xlsx::open(&std::fs::read(&file).unwrap()).unwrap();
    assert!(wb.formulas(Some("Alternates")).unwrap().iter().any(|(_, f)| f.contains("'Bid Summary'!$B$4")));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn labor_units_in_another_layout_give_the_same_hours_and_the_inputs_are_kept() {
    let dir = tmp("layout");
    let mut s = building(&dir);
    let a = bid(&mut s, &dir, json!({}));
    // The same table as a workbook: other column names, another order, per 1 not per 100.
    let rows = vec![
        row(&["Source", "Hours per unit", "Unit", "Size", "Kind"]),
        row(&["Company labor table p.3", "0.05", "ft", "3/4\"", "EMT conduit"]),
        row(&["Company labor table p.3", "0.09", "ft", "2\"", "EMT conduit"]),
        row(&["Company labor table p.7", "0.5", "ea", "", "receptacle"]),
        row(&["Company labor table p.11", "1.2", "ea", "", "Luminaire type A"]),
    ];
    let wb = dir.join("labor.xlsx");
    std::fs::write(&wb, crate::cmd::table::xlsx::write_sheets(&[("Units".into(), rows)]).unwrap()).unwrap();
    let b = bid(&mut s, &dir, json!({"labor_units": wb.to_string_lossy()}));
    for (item, size) in [("EMT conduit", "3/4\""), ("EMT conduit", "2\""), ("receptacle", ""), ("Luminaire type A", "")] {
        let (x, y) = (find(&a, item, size, None), find(&b, item, size, None));
        assert!((money(&x["labor_hours"]) - money(&y["labor_hours"])).abs() < 1e-6, "{item} {size}: {x} vs {y}");
        assert_eq!(x["labor_source"], y["labor_source"]);
    }
    // The inputs (rates, allowances, names) are kept: a run with none gives the same bid.
    let c = s
        .execute(
            "bim.bid",
            &json!({"price_book": book(&dir), "labor_units": dir.join("labor.csv").to_string_lossy(), "today": "2026-11-04", "alternates": false}),
        )
        .unwrap();
    assert_eq!(c["base_bid"], a["base_bid"]);
    assert_eq!(c["allowances"], a["allowances"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn hostile_bid_params_are_refused_never_a_panic() {
    let dir = tmp("hostile");
    let mut s = building(&dir);
    let pb = book(&dir);
    for p in [
        json!({"price_book": pb, "markups": {"tax_pct": 500}}),
        json!({"price_book": pb, "markups": "lots"}),
        json!({"price_book": pb, "labor_rate": -4}),
        json!({"price_book": pb, "labor_rate": "x"}),
        json!({"price_book": pb, "allowances": [{"name": "", "amount": 3}]}),
        json!({"price_book": pb, "allowances": [{"name": "x", "amount": f64::MAX}]}),
        json!({"price_book": pb, "allowances": [{"name": "x", "amount": 3, "system": "nonsense"}]}),
        json!({"price_book": pb, "allowances": "many"}),
        json!({"price_book": pb, "trades": []}),
        json!({"price_book": pb, "today": "yesterday"}),
        json!({"price_book": pb, "stale_days": 0}),
        json!({"price_book": dir.join("260519 - Conductors.txt").to_string_lossy()}),
        json!({"price_book": pb, "labor_units": dir.join("missing.csv").to_string_lossy()}),
    ] {
        assert!(s.execute("bim.bid", &p).is_err(), "{p}");
    }
    let many: Vec<Value> = (0..500).map(|i| json!({"name": format!("a{i}"), "amount": 1})).collect();
    assert!(s.execute("bim.bid", &json!({"price_book": pb, "allowances": many})).is_err());
    // An empty drawing: no spec, a clear error.
    let mut e = Session::empty();
    e.open_drawing(Drawing::new_imperial(), "Empty", None);
    assert!(e.execute("bim.bid", &json!({})).is_err());
    std::fs::remove_dir_all(&dir).ok();
}
