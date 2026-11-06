//! E-GEAR-SPACE, E-GEAR-FILL, E-GEAR-SIZE (Q6151) on a made-up one-storey job: an electrical
//! room (11) west of a storage room (12), a partition between them with a 3 ft door that swings
//! into 11. In 11: distribution panel DPA on the south wall (a supply duct crosses 7 ft over it),
//! and panel LPB on the partition beside the door. Panel LPC stands in 12 with 30 in in front of it.
//! DPA's feeder comes in as four parallel sets of 500 kcmil on main lugs that take two per phase,
//! through a gutter too small for them; six 2" conduits leave through its top, whose drawn
//! conduit area is 16 x 4 in. LPB's record says the submittal's box is 24 in wide; it is built 20.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": p, "text": t, "height": h})).unwrap();
}

/// A door block: its leaf line and its swing arc from the hinge.
fn door(s: &mut Session, at: [f64; 2], rotation: f64) {
    if s.doc().unwrap().block("DOOR-36").is_none() {
        let before: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
        s.execute("line", &json!({"points": [[0.0, 0.0], [36.0, 0.0]]})).unwrap();
        s.execute("arc", &json!({"center": [0, 0], "radius": 36, "start": 0, "end": 90})).unwrap();
        let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).filter(|h| !before.contains(h)).collect();
        s.execute("block", &json!({"name": "DOOR-36", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    }
    s.execute("insert", &json!({"name": "DOOR-36", "at": at, "rotation": rotation})).unwrap();
}

fn panel(tag: &str, min: [f64; 2], max: [f64; 2], bottom_ft: f64, box_prop: &str, extra: Value) -> Value {
    let mut p = json!({
        "tag": tag, "kind": "panel", "level": "L1", "min": min, "max": max, "bottom_ft": bottom_ft, "height_ft": 50.0 / 12.0,
        "properties": {"BOX": box_prop}, "sources": {"size": "Gear submittal p.4"}, "confidence": 0.9,
    });
    if let (Some(o), Some(x)) = (p.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            o.insert(k.clone(), v.clone());
        }
    }
    p
}

fn feeder(tag: &str, from: &str, to: &str, sets: u32, size: &str, conduits: u32, conduit_in: f64, entry: &str) -> Value {
    json!({
        "tag": tag, "from": from, "to": to, "sets": sets, "phases": 3, "size": size, "material": "cu", "neutrals": 1, "grounds": 1,
        "ground_size": "#4", "conduits": conduits, "conduit_in": conduit_in, "entry": entry, "from_entry": entry,
        "source": format!("FEEDER SCHEDULE on X-601 row {tag}"),
    })
}

fn spec() -> Value {
    let sheet = |n: &str, title: &str, rect: [f64; 4]| json!({"number": n, "title": title, "discipline": "E", "source": "model", "rect": rect});
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0], "region": [-100, -100, 1300, 700]}],
        "layers": [
            {"layer": "A-WALL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-DOOR", "role": "door"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "rooms": [
            {"name": "Electrical", "number": "11", "level": "L1"},
            {"name": "Storage", "number": "12", "level": "L1"}
        ],
        "schedule": {"tags": [{"mark": "11", "at": [140.0, 50.0], "level": "L1", "swings_into": "11", "source": "X-11"}]},
        "mep": {
            "systems": [{"name": "SA", "layer": "M-DUCT", "kind": "duct", "size_in": [12, 8], "bottom_ft": 7}],
            "runs": [{"system": "SA", "level": "L1", "points": [[30.0, -20.0], [30.0, 100.0]], "size_in": [12, 8], "bottoms_ft": [7.0], "id": "D1"}],
            "equipment": [
                panel("DPA", [20.0, 6.0], [40.0, 12.0], 2.0, "50 H x 20 W x 6 D in", json!({
                    "properties": {"BOX": "50 H x 20 W x 6 D in", "TOP CONDUIT AREA": "16 W x 4 D in, 64 sq in; drawn: Gear submittal p.4"},
                    "terminations": [{"point": "main", "lugs": 2, "wires_per_lug": 1, "range": "3/0-500 kcmil", "material": "cu", "supplied": true, "bending_in": 16.0, "source": "Gear submittal p.4"}],
                    "enclosure": {"top_entry_in": [16.0, 4.0], "bottom_entry_in": [0.0, 0.0], "gutter_in": [4.0, 4.0]}
                })),
                panel("LPB", [144.0, 70.0], [150.0, 90.0], 2.0, "50 H x 24 W x 6 D in", json!({})),
                panel("LPC", [250.0, 6.0], [270.0, 12.0], 2.0, "50 H x 20 W x 6 D in", json!({}))
            ],
            "feeders": [
                feeder("F4", "SWB", "DPA", 4, "500 kcmil", 4, 3.0, "bottom"),
                feeder("F2", "DPA", "LPB", 1, "#2", 6, 2.0, "top")
            ],
            "panel_schedules": [
                {"panel": "DPA", "source": "X-601", "header": {"volts/phase": "208Y/120V, 3PH, 4W", "location": "ELEC 11"},
                 "circuits": [{"number": "1", "description": "PANEL LPB"}, {"number": "3", "description": "PANEL LPC"}]},
                {"panel": "LPB", "source": "X-601", "header": {"volts/phase": "208Y/120V, 3PH, 4W"}, "circuits": []},
                {"panel": "LPC", "source": "X-601", "header": {"volts/phase": "208Y/120V, 3PH, 4W"}, "circuits": []}
            ]
        },
        "rfis": [{"id": "RFI-C1", "kind": "conflict", "level": "L1", "at": [30.0, 9.0], "sheets": ["X-501", "X-601"],
                  "about": "DPA feeder", "issue": "the one-line gives DPA a 400 A feeder; its schedule says 225 A", "assumed": "the schedule", "status": "open"}],
        "sheets": [
            sheet("X-501", "ONE-LINE DIAGRAM", [400.0, 300.0, 700.0, 500.0]),
            sheet("X-601", "PANEL SCHEDULES", [800.0, 300.0, 1100.0, 500.0])
        ]
    })
}

fn job() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    // The shell (inside 6..294 x 6..180) and the partition at x 150..156.
    rect(&mut s, [0.0, 0.0], [300.0, 186.0]);
    rect(&mut s, [6.0, 6.0], [294.0, 180.0]);
    s.execute("line", &json!({"points": [[150.0, 6.0], [150.0, 180.0]]})).unwrap();
    s.execute("line", &json!({"points": [[156.0, 6.0], [156.0, 180.0]]})).unwrap();
    // LPC's room is only 30 in deep in front of it: a wall at y 42.
    s.execute("line", &json!({"points": [[156.0, 42.0], [294.0, 42.0]]})).unwrap();
    s.execute("line", &json!({"points": [[156.0, 48.0], [294.0, 48.0]]})).unwrap();
    layer(&mut s, "A-DOOR");
    door(&mut s, [153.0, 32.0], 90.0);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, [78.0, 150.0], "11", 6.0);
    text(&mut s, [225.0, 150.0], "12", 6.0);
    // The schedule and one-line sheets' words.
    layer(&mut s, "E-ANNO-TEXT");
    text(&mut s, [420.0, 480.0], "ONE-LINE DIAGRAM", 8.0);
    text(&mut s, [820.0, 480.0], "PANEL SCHEDULE DPA", 8.0);
    let r = s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    s.execute("model.build", &json!({})).unwrap();
    s
}

fn screen(s: &mut Session, rules: &[&str]) -> (Value, Vec<Value>) {
    let r = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": rules})).unwrap();
    let mut fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    let mut cursor = r["next"].clone();
    while let Some(c) = cursor.as_u64() {
        let more = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "cursor": c, "rules": rules})).unwrap();
        fs.extend(more["findings"].as_array().cloned().unwrap_or_default());
        cursor = more["next"].clone();
    }
    (r, fs)
}

/// The finding of `rule` about gear `tag` whose check starts with `check`.
fn finding<'a>(fs: &'a [Value], rule: &str, tag: &str, check: &str) -> &'a Value {
    fs.iter()
        .find(|f| f["rule"] == rule && f["tag"] == tag && f["measured"]["check"].as_str().is_some_and(|c| c.starts_with(check)))
        .unwrap_or_else(|| panic!("no {rule} {check} finding for {tag}: {}", Value::Array(fs.to_vec())))
}

#[test]
fn the_gear_rules_are_built_in_and_run_by_the_prescreen() {
    let mut s = Session::new();
    for id in ["E-GEAR-SPACE", "E-GEAR-FILL", "E-GEAR-SIZE"] {
        let r = s.execute("validate.rules", &json!({"id": id})).unwrap();
        let rule = &r["rules"][0];
        assert_eq!(rule["id"], id, "{r}");
        assert_eq!(rule["trade"], "electrical", "{r}");
        assert!(rule["how"].as_array().is_some_and(|h| !h.is_empty()), "{r}");
    }
    let mut s = job();
    let (r, _) = screen(&mut s, &["E-GEAR-SPACE", "E-GEAR-FILL", "E-GEAR-SIZE"]);
    for id in ["E-GEAR-SPACE", "E-GEAR-FILL", "E-GEAR-SIZE"] {
        assert!(r["checked"][id].is_object(), "{id}: {r}");
    }
}

#[test]
fn working_space_clear_short_and_crossed_by_a_door() {
    let mut s = job();
    let (_, fs) = screen(&mut s, &["E-GEAR-SPACE"]);
    // DPA faces 14 ft of room: clear, its ceiling 9 ft; the duct 7 ft over it is in its dedicated space.
    let dp1 = finding(&fs, "E-GEAR-SPACE", "DPA", "working space");
    assert_eq!(dp1["verdict"], "pass", "{dp1}");
    assert_eq!(dp1["measured"]["volts_to_ground"], json!(120.0), "{dp1}");
    assert_eq!(finding(&fs, "E-GEAR-SPACE", "DPA", "headroom")["verdict"], "pass");
    let ded = finding(&fs, "E-GEAR-SPACE", "DPA", "dedicated space");
    assert_eq!(ded["verdict"], "fail", "{ded}");
    assert_eq!(ded["measured"]["through"][0]["run"], "D1", "{ded}");
    // LPB: the door beside it swings into 11 across its working space.
    let p2 = finding(&fs, "E-GEAR-SPACE", "LPB", "working space");
    assert_eq!(p2["verdict"], "fail", "{p2}");
    let doors = p2["measured"]["doors"].as_array().unwrap();
    assert!(doors.iter().any(|d| d["verdict"] == "fail" && d["swings_into"] == "11"), "{p2}");
    // LPC: a wall 30 in in front of it (36 in needed at 120 V).
    let p3 = finding(&fs, "E-GEAR-SPACE", "LPC", "working space");
    assert_eq!(p3["verdict"], "fail", "{p3}");
    let clear = p3["measured"]["clear_in"].as_f64().unwrap();
    assert!((clear - 30.0).abs() < 1.0, "{p3}");
}

#[test]
fn too_many_conduits_an_overfull_gutter_and_a_lug_too_small_fail() {
    let mut s = job();
    let (_, fs) = screen(&mut s, &["E-GEAR-FILL"]);
    // Six 2" conduits on a 16 x 4 in top: they don't pack.
    let top = finding(&fs, "E-GEAR-FILL", "DPA", "raceways top");
    assert_eq!(top["verdict"], "fail", "{top}");
    assert_eq!(top["measured"]["raceways"], json!(6), "{top}");
    assert!(top["measured"]["plate_from"].as_str().unwrap().contains("drawn"), "{top}");
    // Twelve 500 kcmil phase conductors (and their neutrals and grounds) in a 4 x 4 gutter.
    let gutter = finding(&fs, "E-GEAR-FILL", "DPA", "gutter");
    assert_eq!(gutter["verdict"], "fail", "{gutter}");
    assert!(gutter["measured"]["conductor_sq_in"].as_f64().unwrap() > 6.4, "{gutter}");
    // Four parallel sets on main lugs that take two per phase.
    let lugs = finding(&fs, "E-GEAR-FILL", "DPA", "lugs F4");
    assert_eq!(lugs["verdict"], "fail", "{lugs}");
    assert!(lugs["issue"].as_str().unwrap().contains("parallel"), "{lugs}");
    // 16 in at the lugs bends one 500 kcmil per terminal entering the wall opposite (14 in needed).
    assert_eq!(finding(&fs, "E-GEAR-FILL", "DPA", "bending F4")["verdict"], "pass");
    // DPA's schedule names LPC, which no feeder record gives: its count is unsure, never a pass.
    let named = finding(&fs, "E-GEAR-FILL", "DPA", "feeders");
    assert_eq!(named["verdict"], "unsure", "{named}");
    // LPB gives no gutter or bending space: missing, unsure.
    assert_eq!(finding(&fs, "E-GEAR-FILL", "LPB", "gutter")["verdict"], "unsure");
    assert_eq!(finding(&fs, "E-GEAR-FILL", "LPB", "bending F2")["verdict"], "unsure");
    // The build's own conflict about DPA's feeder comes in as a conflict.
    assert!(fs.iter().any(|f| f["rule"] == "E-GEAR-FILL" && f["verdict"] == "conflict" && f["source"] == "build"), "{}", Value::Array(fs.clone()));
}

#[test]
fn an_enclosure_built_another_size_than_the_submittal_fails() {
    let mut s = job();
    let (_, fs) = screen(&mut s, &["E-GEAR-SIZE"]);
    let dp1 = finding(&fs, "E-GEAR-SIZE", "DPA", "enclosure size");
    assert_eq!(dp1["verdict"], "pass", "{dp1}");
    assert_eq!(dp1["measured"]["model_from"], "built", "{dp1}");
    let p2 = finding(&fs, "E-GEAR-SIZE", "LPB", "enclosure size");
    assert_eq!(p2["verdict"], "fail", "{p2}");
    assert!(p2["issue"].as_str().unwrap().contains("width"), "{p2}");
}

#[test]
fn gear_with_no_record_of_its_documents_is_never_a_pass() {
    let mut s = job();
    // Take away LPC's box record and every schedule: no size, no voltage.
    let mut sp: Value = serde_json::from_str(&s.execute("bspec.get", &json!({})).unwrap()["spec"].to_string()).unwrap();
    if let Some(eq) = sp["mep"]["equipment"].as_array_mut() {
        for e in eq.iter_mut().filter(|e| e["tag"] == "LPC") {
            e["properties"] = json!({});
            e["sources"] = json!({});
        }
    }
    sp["mep"]["panel_schedules"] = json!([]);
    s.execute("bspec.set", &json!({"spec": sp, "replace": true})).unwrap();
    let (_, fs) = screen(&mut s, &["E-GEAR-SIZE", "E-GEAR-SPACE", "E-GEAR-FILL"]);
    assert_eq!(finding(&fs, "E-GEAR-SIZE", "LPC", "enclosure size")["verdict"], "unsure");
    // No voltage: the depth it needs isn't known (and 2 ft is short of any).
    let ws = finding(&fs, "E-GEAR-SPACE", "LPC", "working space");
    assert_ne!(ws["verdict"], "pass", "{ws}");
    assert_eq!(finding(&fs, "E-GEAR-FILL", "LPC", "raceways")["verdict"], "unsure");
    assert!(fs.iter().all(|f| f["tag"] != "LPC" || f["verdict"] != "pass"), "{}", Value::Array(fs.clone()));
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-vgear-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

#[test]
fn an_electrical_room_packet_carries_the_gear_the_riser_and_the_schedules() {
    let mut s = job();
    let out = dir("elec");
    s.execute("validate.packet", &json!({"area": "L1/11", "out_dir": out.to_string_lossy()})).unwrap();
    let o = read_json(&out.join("objects.json"));
    let gear = o["gear"]["gear"].as_array().unwrap_or_else(|| panic!("{}", o["gear"]));
    let tags: Vec<&str> = gear.iter().filter_map(|g| g["tag"].as_str()).collect();
    assert!(tags.contains(&"DPA") && tags.contains(&"LPB") && !tags.contains(&"LPC"), "{tags:?}");
    let dp1 = gear.iter().find(|g| g["tag"] == "DPA").unwrap();
    assert_eq!(dp1["feeders"].as_array().unwrap().len(), 2, "{dp1}");
    let m = read_json(&out.join("manifest.json"));
    let prints = m["prints"].as_array().unwrap();
    assert!(prints.iter().any(|c| c["kind"] == "gear_riser" && c["sheet"] == "X-501"), "{m}");
    assert!(prints.iter().any(|c| c["kind"] == "panel_schedule" && c["sheet"] == "X-601"), "{m}");
    let pre = o["prescreen"]["findings"].as_array().unwrap();
    assert!(pre.iter().any(|f| f["rule"] == "E-GEAR-SPACE"), "{}", o["prescreen"]);
    let rules = read_json(&out.join("rules.json"));
    let ids: Vec<&str> = rules["rules"].as_array().unwrap().iter().filter_map(|r| r["id"].as_str()).collect();
    for id in ["E-GEAR-SPACE", "E-GEAR-FILL", "E-GEAR-SIZE"] {
        assert!(ids.contains(&id), "{id}: {ids:?}");
    }
    let prompt = std::fs::read_to_string(out.join("prompt.md")).unwrap();
    assert!(prompt.contains("`gear`"), "{prompt}");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn hostile_gear_records_do_not_panic() {
    let mut s = job();
    let mut sp: Value = serde_json::from_str(&s.execute("bspec.get", &json!({})).unwrap()["spec"].to_string()).unwrap();
    sp["mep"]["equipment"] = json!([
        {"tag": "X", "kind": "panel", "level": "L1", "min": [1e308, -1e308], "max": [-1e308, 1e308], "bottom_ft": 1e308, "height_ft": 1e308,
         "properties": {"BOX": "1e400 H x NaN W x -3 D in"}, "enclosure": {"top_entry_in": [1e308, 1e308], "bottom_entry_in": [-1, -1], "gutter_in": [1e-308, 1e308]},
         "terminations": [{"point": "main", "lugs": 4294967295u64, "wires_per_lug": 4294967295u64, "range": "999999 kcmil-1", "bending_in": 1e308}]},
        {"tag": "", "kind": "switchboard", "level": "nowhere", "min": [0, 0], "max": [0, 0], "bottom_ft": 0, "height_ft": 0}
    ]);
    sp["mep"]["feeders"] = json!([
        {"tag": "Z", "from": "X", "to": "X", "sets": 4294967295u64, "phases": 4294967295u64, "size": "1/0/0", "conduits": 4294967295u64, "conduit_in": 1e308, "entry": "sideways"}
    ]);
    s.execute("bspec.set", &json!({"spec": sp, "replace": true})).unwrap();
    let r = s.execute("validate.prescreen", &json!({"rules": ["E-GEAR-SPACE", "E-GEAR-FILL", "E-GEAR-SIZE"], "all": true}));
    assert!(r.is_ok(), "{r:?}");
}
