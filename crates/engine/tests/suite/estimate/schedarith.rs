//! The panel and load-calculation tables' own arithmetic, cross-checked in `rfi.read` (Q6649) on
//! made-up buildings: a distribution schedule whose rows for two panels carry each other's
//! totals, and guest-suite demand calculations that count an item only some units have for more
//! or fewer units than the plans give it to (and a line whose count times its VA isn't its
//! total). Each mismatch is an RFI with the field's correction kept beside the printed value.

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Props, Source};
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// A device as the build makes them: a small box with the build's tag and its property store.
fn dev(d: &mut Drawing, x: f64, ps: &[(&str, &str)]) -> Handle {
    let v = |x: f64, y: f64, z: f64| Vec3 { x: x * 12.0, y: y * 12.0, z: z * 12.0 };
    let (lo, hi) = ([x, 0.0, 1.0], [x + 0.3, 0.2, 1.3]);
    let vertices = vec![
        v(lo[0], lo[1], lo[2]),
        v(hi[0], lo[1], lo[2]),
        v(hi[0], hi[1], lo[2]),
        v(lo[0], hi[1], lo[2]),
        v(lo[0], lo[1], hi[2]),
        v(hi[0], lo[1], hi[2]),
        v(hi[0], hi[1], hi[2]),
        v(lo[0], hi[1], hi[2]),
    ];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, ..Default::default() }));
    e.common.layer = "E-POWER".into();
    let x = vec![
        RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
        RawTag { code: 1000, value: "device".into() },
        RawTag { code: 1000, value: "L1".into() },
        RawTag { code: 1000, value: "0".into() },
    ];
    let mut store = Props::default();
    for (k, val) in ps {
        store.set(k, Prop::new(json!(val), Source::print("Sample Sheet E-101", None)));
    }
    e.common.xdata = props::write(&x, &store).into();
    d.ensure_layer("E-POWER");
    d.model.push(e);
    h
}

fn ckt(n: &str, desc: &str, amps: &str, poles: u32, va: Option<f64>) -> Value {
    json!({"number": n, "description": desc, "breaker": amps, "poles": poles, "load_va": va, "source": "X-2"})
}

fn session(spec: Value, d: Drawing) -> Session {
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Building", None);
    s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
    s
}

fn check<'a>(r: &'a Value, id: &str) -> &'a Value {
    r["requirements"]["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("no check {id}: {}", r["requirements"]))
}

fn items<'a>(r: &'a Value, id: &str) -> Vec<&'a Value> {
    r["items"].as_array().unwrap().iter().filter(|i| i["topic"].as_str().unwrap_or("").starts_with(&format!("requirement: {id} "))).collect()
}

/// A distribution schedule feeding three panels: two rows swapped (one panel's own schedule prints
/// its total, the other's rows add up to it), the third right. Either schedule layout: the fed
/// panel named after a feeder word, or by a feeder record.
fn distribution(swapped: bool) -> Session {
    let (r1, r2) = if swapped { (12_700.0, 27_200.0) } else { (27_100.0, 12_800.0) };
    let three = "208Y/120V, 3PH, 4W";
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}],
        "mep": {
            "feeders": [{"tag": "F3", "from": "MSB", "to": "LP3", "sets": 1, "phases": 3, "size": "#3", "material": "cu"}],
            "panel_schedules": [
                {"panel": "MSB", "source": "X-1", "header": {"volts/phase": three, "bus amps": "400A"},
                 "circuits": [
                    ckt("1", "PANEL LP1", "100", 3, Some(r1)),
                    ckt("2", "PANELBOARD LP2", "100", 3, Some(r2)),
                    ckt("3", "LP3 KITCHEN", "60", 3, Some(5_000.0)),
                    ckt("4", "SPACE", "", 3, None)
                 ]},
                {"panel": "LP1", "source": "X-2", "header": {"volts/phase": three, "bus amps": "100A"},
                 "circuits": [ckt("1", "LIGHTING", "20", 1, Some(1_500.0)), ckt("2", "WATER HEATER", "30", 3, Some(9_000.0))],
                 "totals": {"connected_va": 27_000.0}},
                {"panel": "LP2", "source": "X-3", "header": {"volts/phase": three, "bus amps": "100A"},
                 "circuits": [ckt("1", "RECEPTACLES", "20", 1, Some(4_900.0)), ckt("2", "FAN COIL", "20", 2, Some(8_000.0))]},
                {"panel": "LP3", "source": "X-3", "header": {"volts/phase": three, "bus amps": "100A"},
                 "circuits": [ckt("1", "OVEN", "40", 3, Some(5_000.0))]}
            ]
        }
    });
    session(spec, Drawing::new_imperial())
}

#[test]
fn feeder_rows_that_carry_another_panel_s_total_are_raised_and_corrected() {
    let mut s = distribution(true);
    let r = s.execute("rfi.read", &json!({})).unwrap();
    let c = check(&r, "E-FEEDER-ROW-LOAD");
    // Three rows feed a panel with a total (LP3 by its feeder record); two are wrong.
    assert_eq!((c["objects"].as_u64(), c["open_objects"].as_u64(), c["findings"].as_u64()), (Some(3), Some(2), Some(2)), "{c}");
    let found = items(&r, "E-FEEDER-ROW-LOAD");
    let row1 = found.iter().find(|i| i["about"] == "panel MSB circuit 1").unwrap_or_else(|| panic!("{found:?}"));
    let issue = row1["issue"].as_str().unwrap();
    assert!(issue.contains("12,700 VA") && issue.contains("27,000 VA") && issue.contains("look swapped"), "{issue}");
    assert!(row1["assumed"].as_str().unwrap().starts_with("Corrected for the field"), "{row1}");
    assert_eq!(row1["sheets"], json!(["X-1", "X-2"]));
    // LP2's rows add to its total (no printed total): 12,900 VA, which row 1 carries.
    let row2 = found.iter().find(|i| i["about"] == "panel MSB circuit 2").unwrap();
    assert!(row2["issue"].as_str().unwrap().contains("12,900 VA"), "{row2}");

    // Apply: the RFIs go in the spec and each row takes its panel's total, the printed load kept
    // beside it with the reason and the rule.
    s.execute("rfi.read", &json!({"apply": true})).unwrap();
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    let row = &sp["mep"]["panel_schedules"][0]["circuits"][0];
    assert_eq!((row["load_va"].as_f64(), row["design_load_va"].as_f64()), (Some(27_000.0), Some(12_700.0)), "{row}");
    assert_eq!(row["corrections"][0]["key"], "load_va");
    assert!(row["corrections"][0]["rule"].as_str().unwrap().contains("NEC 215.2"), "{row}");
    assert!(sp["mep"]["panel_schedules"][0]["circuits"][2]["corrections"].is_null(), "the right row is untouched");
    let list = s.execute("rfi.list", &json!({"kind": "requirement"})).unwrap();
    assert_eq!(list["total"], 2, "{}", list["csv"]);
    // Read again: still the design team's question (the printed value is the design's), same ids,
    // nothing twice, the correction not stacked.
    let again = s.execute("rfi.read", &json!({"apply": true})).unwrap();
    assert_eq!(check(&again, "E-FEEDER-ROW-LOAD")["findings"], 2);
    assert_eq!(s.execute("rfi.list", &json!({"kind": "requirement"})).unwrap()["total"], 2);
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    assert_eq!(sp["mep"]["panel_schedules"][0]["circuits"][0]["corrections"].as_array().unwrap().len(), 1);
    // The load calculation reports the same rows by the same rule.
    let v = s.execute("bim.loadcalc", &json!({"occupancy": "commercial"})).unwrap();
    let swaps = v["findings"].as_array().unwrap().iter().filter(|f| f["message"].as_str().unwrap().contains("look swapped")).count();
    assert_eq!(swaps, 2, "{}", v["findings"]);
}

#[test]
fn feeder_rows_within_rounding_raise_nothing() {
    let mut s = distribution(false);
    let r = s.execute("rfi.read", &json!({"apply": true})).unwrap();
    let c = check(&r, "E-FEEDER-ROW-LOAD");
    assert_eq!((c["objects"].as_u64(), c["open_objects"].as_u64()), (Some(3), Some(0)), "{c}");
    assert_eq!(c["ran"], true);
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    assert!(sp["mep"]["panel_schedules"][0]["circuits"][0]["design_load_va"].is_null());
}

/// Six guest suites on a typical unit panel, three on each distribution panel; a dishwasher only
/// in some suites (two under DPA, one under DPB), a range in every one. Each panel's demand
/// calculation counts the dishwasher once; DPB's microwave line prints a total its count doesn't
/// give. `mapped`: the feeder records say which suites each panel feeds.
fn suites(mapped: bool) -> Session {
    let mut d = Drawing::new_imperial();
    let rooms = ["101", "102", "103", "201", "202", "203"];
    for (i, room) in rooms.iter().enumerate() {
        let x = i as f64 * 10.0;
        dev(&mut d, x, &[("kind", "receptacle"), ("panel", "U"), ("circuit", "U-1"), ("room", room)]);
        dev(&mut d, x + 1.0, &[("kind", "equipment_connection"), ("panel", "U"), ("circuit", "U-3"), ("room", room)]);
        dev(&mut d, x + 2.0, &[("kind", "receptacle"), ("panel", "U"), ("circuit", "U-4"), ("room", room)]);
    }
    for (i, room) in ["101", "102", "201"].iter().enumerate() {
        dev(&mut d, 80.0 + i as f64, &[("kind", "receptacle"), ("panel", "U"), ("circuit", "U-2"), ("room", room)]);
    }
    let unit_total = 1_000.0 + 1_200.0 + 2_400.0 + 1_200.0;
    let feeders: Vec<Value> = if mapped {
        rooms
            .iter()
            .map(|r| {
                let from = if r.starts_with('1') { "DPA" } else { "DPB" };
                json!({"tag": format!("F-{r}"), "from": from, "to": format!("U-{r}"), "sets": 1, "phases": 1})
            })
            .collect()
    } else {
        Vec::new()
    };
    let calc = |panel: &str, micro: &str| {
        json!({"title": format!("PANELBOARD {panel} DEMAND CALCULATION"), "panel": panel, "basis": "220.84", "source": "X-4", "lines": [
            {"label": "3 UNITS WITH A TOTAL SF OF:", "values": [3.0, 900.0], "text": "3 UNITS WITH A TOTAL SF OF: 900.00"},
            {"label": "RANGE CONNECTION (2,400VA/SUITE):", "values": [7.2], "text": "RANGE CONNECTION (2,400VA/SUITE): 7.20 KVA"},
            {"label": "DISHWASHER RECEPTACLE (LOWER LEVEL ONLY X1) (1,200VA/SUITE):", "values": [1.2], "text": "DISHWASHER RECEPTACLE (LOWER LEVEL ONLY X1) (1,200VA/SUITE): 1.20 KVA"},
            {"label": "MICROWAVE RECEPTACLE (X3) (1,200VA/SUITE):", "values": [3.6], "text": micro},
            {"label": "TOTAL AMPS AT 120/208V, 3PH:", "values": [40.0]}
        ]})
    };
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}],
        "mep": {
            "feeders": feeders,
            "panel_schedules": [
                {"panel": "DPA", "source": "X-4", "header": {"volts/phase": "208Y/120V, 3PH, 4W", "bus amps": "225A"},
                 "circuits": [ckt("1", "PANELBOARD U (3 UNITS)", "125", 2, Some(unit_total * 3.0))]},
                {"panel": "DPB", "source": "X-4", "header": {"volts/phase": "208Y/120V, 3PH, 4W", "bus amps": "225A"},
                 "circuits": [ckt("1", "PANELBOARD U (3 UNITS)", "125", 2, Some(unit_total * 3.0))]},
                {"panel": "U", "typical_of": 6, "source": "X-4", "header": {"volts/phase": "208Y/120V, 1PH, 3W", "bus amps": "100A"},
                 "circuits": [
                    ckt("1", "ROOM RCPT AND LIGHTING", "20", 1, Some(1_000.0)),
                    ckt("2", "DISHWASHER (LOWER LEVEL ONLY)", "20", 1, Some(1_200.0)),
                    ckt("3", "RANGE", "30", 2, Some(2_400.0)),
                    ckt("4", "MICROWAVE", "20", 1, Some(1_200.0))
                 ],
                 "totals": {"connected_va": unit_total}}
            ],
            "load_summaries": [
                calc("DPA", "MICROWAVE RECEPTACLE (X3) (1,200VA/SUITE): 3.60 KVA"),
                calc("DPB", "MICROWAVE RECEPTACLE (X3) (1,200VA/SUITE): 2.40 KVA")
            ]
        }
    });
    session(spec, d)
}

#[test]
fn a_per_unit_count_the_plans_dont_give_on_its_feeder_is_raised() {
    let mut s = suites(true);
    let r = s.execute("rfi.read", &json!({})).unwrap();
    let c = check(&r, "E-CALC-UNIT-COUNT");
    // Three per-unit lines in each calculation; DPA's dishwasher count (1, the plans 2) and DPB's
    // microwave total (3 x 1,200 VA is not 2,400 VA) are wrong; DPB's dishwasher (1) is right and
    // the every-suite range is counted for every unit.
    assert_eq!((c["objects"].as_u64(), c["open_objects"].as_u64()), (Some(6), Some(2)), "{c}");
    let found = items(&r, "E-CALC-UNIT-COUNT");
    let dw = found.iter().find(|i| i["about"].as_str().unwrap().contains("DPA") && i["issue"].as_str().unwrap().contains("DISHWASHER")).unwrap();
    let issue = dw["issue"].as_str().unwrap();
    assert!(issue.contains("counts 1 unit") && issue.contains("in 2 of the 3 unit(s) DPA feeds") && issue.contains("U-2"), "{issue}");
    let mw = found.iter().find(|i| i["about"].as_str().unwrap().contains("DPB")).unwrap();
    assert!(mw["issue"].as_str().unwrap().contains("3,600 VA") && mw["issue"].as_str().unwrap().contains("2,400 VA"), "{mw}");
    assert!(!found.iter().any(|i| i["about"].as_str().unwrap().contains("DPB") && i["issue"].as_str().unwrap().contains("DISHWASHER")), "{found:?}");

    // Apply: the line keeps its printed values, the count's correction beside them.
    s.execute("rfi.read", &json!({"apply": true})).unwrap();
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    let line = &sp["mep"]["load_summaries"][0]["lines"][2];
    assert_eq!(line["values"], json!([1.2]));
    assert_eq!((line["corrections"][0]["key"].as_str(), line["corrections"][0]["design"].as_str()), (Some("count"), Some("1")), "{line}");
    assert_eq!(line["corrections"][0]["corrected"], "2");
    assert!(line["corrections"][0]["rule"].as_str().unwrap().contains("220.84"), "{line}");
}

#[test]
fn with_no_feeder_map_the_calculations_are_counted_together() {
    let mut s = suites(false);
    let r = s.execute("rfi.read", &json!({"apply": true})).unwrap();
    let found = items(&r, "E-CALC-UNIT-COUNT");
    // 1 + 1 printed against the 3 suites the plans give a dishwasher: both lines asked about,
    // neither corrected (which suites each panel feeds is not known).
    let dw: Vec<&&Value> = found.iter().filter(|i| i["issue"].as_str().unwrap().contains("DISHWASHER")).collect();
    assert_eq!(dw.len(), 2, "{found:?}");
    assert!(
        dw.iter().all(|i| i["issue"].as_str().unwrap().contains("together count 2") && i["issue"].as_str().unwrap().contains("in 3 unit(s)")),
        "{dw:?}"
    );
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    assert!(sp["mep"]["load_summaries"][0]["lines"][2]["corrections"].is_null(), "{}", sp["mep"]["load_summaries"][0]);
}
