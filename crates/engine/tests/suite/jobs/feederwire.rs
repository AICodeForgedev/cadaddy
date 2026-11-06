//! Q6201 feederwire: a feeder's conductors ride its routed raceway in the takeoff (its
//! schedule's sets, size, material and insulation, makeup at each gear), and a luminaire gets an
//! outlet box only when its type needs one. Made-up buildings only.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use cadaddy_engine::doc::props;
use serde_json::{Value, json};

/// A routed feeder run as the router stores it: `pts` (inches) at `z` feet over its level's
/// floor, or a riser at one point from `z` up to `top`.
fn run(level: &str, src: &str, pts: &[[f64; 2]], z: f64, top: Option<f64>, size: f64) -> Value {
    let mut r = json!({"system": "E-FEEDER", "level": level, "points": pts, "size_in": [size, 0], "bottoms_ft": vec![z; pts.len().saturating_sub(1).max(1)],
        "size_source": "feeder schedule", "height_source": "proposed", "source_sheet": src});
    if let Some(t) = top {
        r["top_ft"] = json!(t);
    }
    r
}

/// Two made-up storeys (12' floor to floor, 9' ceilings): switchboard SB1 and panel PA on the
/// first, panel PB on the second, a typical unit panel U (copies U-101 under U-201).
/// - F2: SB1 to PB, two sets of 4/0 aluminum (2 conduits, 2"), from the feeder schedule only;
/// - F3: SB1 to PA, one set of 250 kcmil copper with a 4/0 neutral (2-1/2");
/// - F4: PA to the typical U (its copies stacked), #6 copper, fed through U-101 to U-201.
fn two_storeys() -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Two Storeys", None);
    let mut runs = Vec::new();
    // F2, each conduit: up out of SB1 (top 7') to 10', across to beside PB, up a floor, over
    // into PB's top (7' over its floor): 3 + 21.5 + 12 on the first, 1.5 + 3 on the second.
    for y in [12.0, 15.0] {
        let src = "feeder F2 SB1→PB";
        runs.push(run("L1", src, &[[24.0, y]], 7.0, Some(10.0), 2.0));
        runs.push(run("L1", src, &[[24.0, y], [282.0, y]], 10.0, None, 2.0));
        runs.push(run("L1", src, &[[282.0, y]], 10.0, Some(22.0), 2.0));
        runs.push(run("L2", src, &[[282.0, y], [264.0, y]], 10.0, None, 2.0));
        runs.push(run("L2", src, &[[264.0, y]], 7.0, Some(10.0), 2.0));
    }
    // F3: up out of SB1, 38' across, down into PA: 44'.
    let src = "feeder F3 SB1→PA";
    runs.push(run("L1", src, &[[36.0, 6.0]], 7.0, Some(10.0), 2.5));
    runs.push(run("L1", src, &[[36.0, 6.0], [492.0, 6.0]], 10.0, None, 2.5));
    runs.push(run("L1", src, &[[492.0, 6.0]], 7.0, Some(10.0), 2.5));
    // F4: PA up, 9' across, down into U-101 (3 + 9 + 3); then through U-101's lugs straight up
    // into U-201's bottom (7' to 12' + 4' = 16': 9').
    let src = "feeder F4 PA→U-101→U-201";
    runs.push(run("L1", src, &[[498.0, 18.0]], 7.0, Some(10.0), 1.0));
    runs.push(run("L1", src, &[[498.0, 18.0], [606.0, 18.0]], 10.0, None, 1.0));
    runs.push(run("L1", src, &[[606.0, 18.0]], 7.0, Some(10.0), 1.0));
    runs.push(run("L1", src, &[[612.0, 12.0]], 7.0, Some(16.0), 1.0));
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}
        ],
        "mep": {
            "systems": [{"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "conduit", "size_in": [2, 0], "bottom_ft": 10}],
            "runs": runs,
            "equipment": [
                {"tag": "SB1", "kind": "switchboard", "level": "L1", "min": [0, 0], "max": [48, 24], "height_ft": 7, "bottom_ft": 0},
                {"tag": "PA", "kind": "panel", "level": "L1", "min": [480, 0], "max": [504, 24], "height_ft": 3, "bottom_ft": 4},
                {"tag": "PB", "kind": "panel", "level": "L2", "min": [252, 0], "max": [276, 24], "height_ft": 3, "bottom_ft": 4},
                {"tag": "U-101", "kind": "panel", "level": "L1", "min": [600, 0], "max": [624, 24], "height_ft": 3, "bottom_ft": 4},
                {"tag": "U-201", "kind": "panel", "level": "L2", "min": [600, 0], "max": [624, 24], "height_ft": 3, "bottom_ft": 4}
            ],
            "feeders": [
                {"tag": "F2", "from": "SB1", "to": "PB", "sets": 2, "phases": 3, "size": "4/0", "material": "al", "neutrals": 1, "grounds": 1, "ground_size": "4",
                 "conduits": 2, "conduit_in": 2, "entry": "top", "source": "FEEDER SCHEDULE E-601"},
                {"tag": "F3", "from": "SB1", "to": "PA", "sets": 1, "phases": 3, "size": "250 kcmil", "material": "cu", "neutrals": 1, "neutral_size": "4/0",
                 "grounds": 1, "ground_size": "#4", "conduits": 1, "conduit_in": 2.5, "entry": "top", "source": "riser diagram E-602"},
                {"tag": "F4", "from": "PA", "to": "U", "sets": 1, "phases": 2, "size": "#6", "material": "cu", "neutrals": 1, "grounds": 1, "ground_size": "#10",
                 "conduits": 1, "conduit_in": 1, "entry": "top", "source": "panel PA schedule circuit 2"}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({"route": false})).expect("build");
    assert!(built["atoms"].as_u64().unwrap_or(0) > 0, "{built}");
    s
}

/// The wire pieces of a system: (item, size) → (pieces, feet).
fn wire_of(s: &Session, system: &str) -> Vec<(String, String, usize, f64)> {
    let d = s.doc().expect("doc");
    let mut out: Vec<(String, String, usize, f64)> = Vec::new();
    for e in d.model.iter() {
        let ps = props::read(&e.common.xdata);
        let g = |k: &str| ps.get(k).map(|p| p.text()).unwrap_or_default();
        if g("atom") != "wire" || g("system") != system {
            continue;
        }
        let ft: f64 = g("length_ft").parse().unwrap_or(0.0);
        match out.iter_mut().find(|x| x.0 == g("item") && x.1 == g("size")) {
            Some(x) => {
                x.2 += 1;
                x.3 += ft;
            }
            None => out.push((g("item"), g("size"), 1, ft)),
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    out
}

fn feet(w: &[(String, String, usize, f64)], item: &str, size: &str) -> f64 {
    w.iter().filter(|x| x.0 == item && x.1 == size).map(|x| x.3).sum()
}

/// The wire pieces a feeder's routed raceways carry (their conductors say the feeder).
fn feeder_wire(s: &Session, tag: &str) -> Vec<(String, String, usize, f64)> {
    let d = s.doc().expect("doc");
    let mut out: Vec<(String, String, usize, f64)> = Vec::new();
    for e in d.model.iter() {
        let ps = props::read(&e.common.xdata);
        let g = |k: &str| ps.get(k).map(|p| p.text()).unwrap_or_default();
        if g("atom") != "wire" || g("conductors_of_feeder").is_empty() {
            continue;
        }
        let doc = ps.get("conductors_of_feeder").and_then(|p| p.source.doc.clone()).unwrap_or_default();
        if !doc.starts_with(&format!("feeder {tag}:")) {
            continue;
        }
        let ft: f64 = g("length_ft").parse().unwrap_or(0.0);
        match out.iter_mut().find(|x| x.0 == g("item") && x.1 == g("size")) {
            Some(x) => {
                x.2 += 1;
                x.3 += ft;
            }
            None => out.push((g("item"), g("size"), 1, ft)),
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    out
}

#[test]
fn a_feeder_carries_its_schedules_conductors_along_its_route_with_makeup_at_each_gear() {
    let mut s = two_storeys();
    // F2: per conduit 5 conductors (3 phases, the neutral, the ground; 2 sets in 2 conduits: one
    // each), each its route plus 3' at SB1 and 3' at PB: (3 + 21.5 + 12 + 3) + (1.5 + 3 + 3) = 47'.
    let f2 = feeder_wire(&s, "F2");
    assert!((feet(&f2, "XHHW aluminum", "#4/0") - 4.0 * 2.0 * 47.0).abs() < 0.05, "{f2:?}");
    assert!((feet(&f2, "XHHW aluminum", "#4") - 2.0 * 47.0).abs() < 0.05, "an aluminum feeder's ground is aluminum: {f2:?}");
    assert_eq!(f2.iter().map(|x| x.2).sum::<usize>(), 20, "two chains (one a storey) of 5 in each of 2 conduits: {f2:?}");
    // F3: one set, the phases 250 kcmil, a reduced 4/0 neutral, a #4 ground: 44' + 3' + 3'.
    let f3 = feeder_wire(&s, "F3");
    assert!((feet(&f3, "THHN copper", "250 kcmil") - 3.0 * 50.0).abs() < 0.05, "{f3:?}");
    assert!((feet(&f3, "THHN copper", "#4/0") - 50.0).abs() < 0.05, "{f3:?}");
    assert!((feet(&f3, "Copper ground, green", "#4") - 50.0).abs() < 0.05, "{f3:?}");
    // F4 (2 phases, the neutral, the ground): PA to U-101 (15' + 3' + 3'), then on through
    // U-101's feed-through lugs to U-201 (9' + 3' + 3'): the typical copies each fed.
    let f4 = feeder_wire(&s, "F4");
    assert!((feet(&f4, "THHN copper", "#6") - 3.0 * (21.0 + 15.0)).abs() < 0.05, "{f4:?}");
    assert!((feet(&f4, "Copper ground, green", "#10") - (21.0 + 15.0)).abs() < 0.05, "{f4:?}");
    // The routed feeders have no entry stubs of their own (their routes enter the gear).
    for tag in ["F2", "F3", "F4"] {
        assert!(wire_of(&s, tag).is_empty(), "no stub for {tag}: {:?}", wire_of(&s, tag));
    }
    // The takeoff counts them (bim.count reads the index).
    let c = s.execute("bim.count", &json!({"kind": "wire", "system": "E-FEEDER", "group_by": "size"})).expect("count");
    let rows = c["rows"].as_array().cloned().unwrap_or_default();
    let ft = |size: &str| rows.iter().filter(|r| r["size"] == json!(size)).filter_map(|r| r["length_ft"].as_f64()).sum::<f64>();
    assert!((ft("#4/0") - (4.0 * 2.0 * 47.0 + 50.0)).abs() < 0.1, "{c}");
    assert!((ft("250 kcmil") - 150.0).abs() < 0.1, "{c}");
}

#[test]
fn a_feeder_with_no_route_keeps_its_entry_stubs() {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Stubs", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "mep": {
            "equipment": [
                {"tag": "SB1", "kind": "switchboard", "level": "L1", "min": [0, 0], "max": [48, 24], "height_ft": 7},
                {"tag": "PA", "kind": "panel", "level": "L1", "min": [480, 0], "max": [504, 24], "height_ft": 3, "bottom_ft": 4}
            ],
            "feeders": [{"tag": "F9", "from": "SB1", "to": "PA", "sets": 1, "phases": 3, "size": "#1", "material": "al", "neutrals": 1, "grounds": 1,
                         "ground_size": "#6", "conduits": 1, "conduit_in": 1.5, "entry": "top"}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
    let w = wire_of(&s, "F9");
    assert_eq!(w.iter().map(|x| x.2).sum::<usize>(), 10, "a stub of 5 conductors at each end: {w:?}");
    assert!(w.iter().all(|x| x.0 == "XHHW aluminum"), "the schedule's material: {w:?}");
}

/// Luminaires of made-up types on a one-storey plan: each type's schedule row (or none).
fn lights(rows: Value, marks: &[&str]) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Lights", None);
    let tags: Vec<Value> = marks
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let x = 5.0 + 4.0 * i as f64;
            json!({"id": format!("device:L1:{x:.1},5.0:{m}"), "kind": "luminaire", "level": "L1", "at_ft": [x, 5.0], "fields": {
                "type": {"value": m, "source": "plan"},
                "device_type": {"value": format!("luminaire {m}"), "source": "plan"},
                "sheet": {"value": "E-101", "source": "plan"},
                "symbol": {"value": format!("1F{i}"), "source": "plan"}
            }})
        })
        .collect();
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": tags,
        "mep": {"fixture_schedule": rows}
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
    s
}

/// The type marks of the luminaires that have a box.
fn boxed(s: &Session) -> Vec<String> {
    let d = s.doc().expect("doc");
    let mut out: Vec<String> = d
        .model
        .iter()
        .filter_map(|e| {
            let ps = props::read(&e.common.xdata);
            if ps.get("atom").map(|p| p.text()).as_deref() != Some("box") {
                return None;
            }
            let dev = ps.get("box_of").and_then(|p| cadaddy_engine::doc::Handle::parse_hex(p.text().trim()))?;
            let dps = props::read(&d.model.get(dev)?.common.xdata);
            dps.get("type_mark").map(|p| p.text())
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_luminaire_takes_an_outlet_box_only_when_its_schedule_row_says_it_mounts_on_one() {
    // Rows in the schedule's own words and column orders: a recessed downlight, a lay-in
    // troffer, a surface wrap, a pendant, a wall light at a height, and a type with no row.
    let rows = json!([
        {"tag": "R1", "installation": "RECESSED", "description": "DOWNLIGHT", "source": "E-9 LIGHT FIXTURE SCHEDULE"},
        {"tag": "R2", "installation": "", "description": "2X4 LAY-IN TROFFER", "source": "E-9 LIGHT FIXTURE SCHEDULE"},
        {"tag": "S1", "installation": "Surface mounted", "description": "LINEAR WRAP", "source": "E-9 LIGHT FIXTURE SCHEDULE"},
        {"tag": "P1", "installation": "PENDANT", "description": "DECORATIVE", "source": "E-9 LIGHT FIXTURE SCHEDULE"},
        {"tag": "W1", "installation": "7'-6\" AFF", "description": "EMERGENCY UNIT", "source": "E-9 LIGHT FIXTURE SCHEDULE"}
    ]);
    let s = lights(rows, &["R1", "R2", "S1", "P1", "W1", "N1"]);
    assert_eq!(boxed(&s), vec!["N1", "P1", "S1", "W1"], "recessed housings carry their own junction box; no row: the usual box");
    // Each box says why it is there.
    let d = s.doc().expect("doc");
    let why: Vec<String> = d
        .model
        .iter()
        .map(|e| props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(|x| x.text()).as_deref() == Some("box"))
        .filter_map(|p| p.get("box_needed").map(|x| x.text()))
        .collect();
    assert!(why.iter().any(|w| w.contains("installation \"pendant\"") && w.contains("E-9 LIGHT FIXTURE SCHEDULE type P1")), "{why:?}");
    assert!(why.iter().any(|w| w.starts_with("box: the trade's usual")), "the type with no row: {why:?}");
}

#[test]
fn a_cut_sheet_that_gives_a_type_its_own_junction_box_takes_the_box_away() {
    // Two made-up surface-scheduled types: one's cut sheet is a canless wafer with a remote
    // driver box, the other's mounts on a standard junction box.
    let rows = json!([
        {"tag": "Q7", "installation": "SURFACE/CEILING", "description": "DOWNLIGHT", "source": "E-9 LIGHT FIXTURE SCHEDULE"},
        {"tag": "Q8", "installation": "SURFACE/CEILING", "description": "DOWNLIGHT", "source": "E-9 LIGHT FIXTURE SCHEDULE"}
    ]);
    let mut s = lights(rows, &["Q7", "Q7", "Q8"]);
    assert_eq!(boxed(&s), vec!["Q7", "Q7", "Q8"], "the schedule alone: surface, a box each");
    let file = std::env::temp_dir().join(format!("cadaddy-feederwire-cutsheet-{}.txt", std::process::id()));
    let text = "--- page 1 of 2 ---\nType : Q7\nManufacturer : ACME LIGHTING\nModel Number : WF4 900LM 90CRI 3000K\n4in Canless LED wafer, 900 lumens, 90 CRI, remote driver / junction box\n--- page 2 of 2 ---\nType : Q8\nManufacturer : OTHER CO\nModel Number : SM6 LED 1000LM\nSurface LED downlight, 1000 lumens, 90 CRI; mounts to standard 3-1/2 in to 4 in octagonal junction boxes\n";
    std::fs::write(&file, text).expect("write");
    let r = s.execute("bim.submittals", &json!({"paths": [file.to_string_lossy()]})).expect("submittals");
    std::fs::remove_file(&file).ok();
    assert_eq!(r["luminaire_boxes_removed"], json!(2), "{r}");
    assert_eq!(boxed(&s), vec!["Q8"], "{r}");
    let c = s.execute("bim.count", &json!({"kind": "box"})).expect("count");
    assert_eq!(c["count"], json!(1), "the count index follows: {c}");
    // The canless type's elements say why: its cut sheet.
    let d = s.doc().expect("doc");
    let jb: Vec<String> = d
        .model
        .iter()
        .map(|e| props::read(&e.common.xdata))
        .filter(|p| p.get("type_mark").map(|x| x.text()).as_deref() == Some("Q7") && p.get("atom").is_none())
        .filter_map(|p| p.get("junction_box").map(|x| x.text()))
        .collect();
    assert_eq!(jb, vec!["canless", "canless"]);
    // They lost the box they were set in.
    assert!(
        d.model
            .iter()
            .map(|e| props::read(&e.common.xdata))
            .filter(|p| p.get("type_mark").map(|x| x.text()).as_deref() == Some("Q7"))
            .all(|p| p.get("box").is_none())
    );
}
