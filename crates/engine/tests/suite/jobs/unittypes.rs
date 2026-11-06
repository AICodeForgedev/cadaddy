//! Q305 unit types end to end: `unittypes.read` fills the building spec from the synthetic hotel's
//! prints (enlarged unit plans, floor plan unit tags, the overrides table), and the schedule,
//! takeoff, edit, override, place and detect commands work on what it read. Quantities are checked
//! against what the hotel actually placed in each room, not against the type definitions.

use cadaddy_engine::Session;
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::truth::{Room, Truth};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The hotel's architectural floor plans and unit plans sheet, written to a fresh folder.
fn sheets(name: &str, seed: u64, size: HotelSize) -> (Truth, PathBuf) {
    let (t, files) = cadaddy_mepfix::unit_reader::fixture_unit_sheets(seed, size);
    let dir = std::env::temp_dir().join(format!("cadaddy-unittypes-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (n, b) in &files {
        std::fs::write(dir.join(n), b).unwrap();
    }
    (t, dir)
}

/// A session whose building spec was read from the hotel's prints.
fn read(name: &str, seed: u64, size: HotelSize) -> (Session, Truth, PathBuf) {
    let (t, dir) = sheets(name, seed, size);
    let mut s = Session::new();
    let r = s.execute("unittypes.read", &json!({"dir": dir.display().to_string()})).unwrap();
    assert_eq!(r["instances"].as_u64(), Some(t.unit_instances.len() as u64), "{r}");
    assert_eq!(r["notes"], json!([]), "{r}");
    (s, t, dir)
}

fn truth_room<'a>(t: &'a Truth, level: &str, number: &str) -> &'a Room {
    t.building.rooms.iter().find(|r| r.level == level && r.number == number).unwrap()
}

/// What the hotel placed in a room, by category and kind (counted from the placed objects).
fn placed(t: &Truth, room: &str) -> BTreeMap<(String, String), u64> {
    let mut out = BTreeMap::new();
    for it in cadaddy_mepfix::units::room_items(t, room) {
        *out.entry((it.category.to_string(), it.kind)).or_default() += 1;
    }
    out
}

fn type_name_of(spec_types: &Value, id: &str) -> String {
    spec_types.as_array().unwrap().iter().find(|t| t["id"] == id).unwrap()["name"].as_str().unwrap().to_string()
}

/// MUST 1 + 5: the schedule read from the prints has the truth's exact count per type and hand,
/// and every instance and override carries its source (and an override its author).
#[test]
fn schedule_from_the_prints_counts_every_type_and_hand() {
    let (mut s, t, dir) = read("schedule", 99, HotelSize::Full);
    let list = s.execute("unittypes.list", &json!({})).unwrap();
    let types = list["types"].clone();
    let sched = s.execute("unittypes.schedule", &json!({})).unwrap();
    let mut got: BTreeMap<(String, String), u64> = BTreeMap::new();
    for row in sched["summary"].as_array().unwrap() {
        let name = type_name_of(&types, row["type"].as_str().unwrap());
        assert_eq!(row["type_name"], name.as_str());
        got.insert((name.clone(), "normal".into()), row["normal"].as_u64().unwrap());
        got.insert((name, "mirrored".into()), row["mirrored"].as_u64().unwrap());
    }
    let mut want: BTreeMap<(String, String), u64> = BTreeMap::new();
    for ut in &t.unit_types {
        for hand in ["normal", "mirrored"] {
            let n = t.unit_instances.iter().filter(|i| i.unit_type == ut.id && i.hand == hand).count() as u64;
            if t.unit_instances.iter().any(|i| i.unit_type == ut.id) {
                want.insert((ut.name.clone(), hand.into()), n);
            }
        }
    }
    assert_eq!(got, want);
    let rows = sched["instances"].as_array().unwrap();
    assert_eq!(rows.len(), t.unit_instances.len());
    for row in rows {
        assert!(row["source"]["sheet"].as_str().unwrap().starts_with("A-10"), "{row}");
        assert_eq!(row["status"], "read");
        let ti = t.unit_instances.iter().find(|i| i.level == row["level"] && i.number == row["number"]).unwrap();
        assert_eq!(row["hand"], ti.hand.as_str());
        assert_eq!(row["overrides"].as_array().unwrap().len(), ti.overrides.len(), "{row}");
        for o in row["overrides"].as_array().unwrap() {
            assert_eq!(o["source"]["sheet"], "A-401", "{o}");
            assert_eq!(o["author"], "unittypes.read");
            assert_eq!(ti.overrides.get(o["key"].as_str().unwrap()).map(String::as_str), o["value"].as_str(), "{o}");
        }
    }
    // Filters.
    let l3 = s.execute("unittypes.schedule", &json!({"level": "3"})).unwrap();
    assert_eq!(l3["instances"].as_array().unwrap().len(), t.unit_instances.iter().filter(|i| i.level == "3").count());
    std::fs::remove_dir_all(&dir).ok();
}

/// MUST 5: the takeoff per type equals what the hotel placed in that type's rooms, terminals and
/// the PTACs included, every instance's overrides applied.
#[test]
fn takeoff_equals_what_the_rooms_hold() {
    let (mut s, t, dir) = read("takeoff", 99, HotelSize::Full);
    let r = s.execute("unittypes.takeoff", &json!({})).unwrap();
    assert_eq!(r["source"], "spec");
    let mut terminals = 0;
    for row in r["types"].as_array().unwrap() {
        let mut want: BTreeMap<(String, String), u64> = BTreeMap::new();
        let rooms = row["rooms"].as_array().unwrap();
        assert_eq!(rooms.len() as u64, row["count"].as_u64().unwrap());
        let instances: Vec<_> = t.unit_instances.iter().filter(|i| rooms.iter().any(|n| n == i.number.as_str())).collect();
        assert_eq!(instances.len(), rooms.len());
        for i in instances {
            for (k, n) in placed(&t, &truth_room(&t, &i.level, &i.number).id) {
                *want.entry(k).or_default() += n;
            }
        }
        let mut got: BTreeMap<(String, String), u64> = BTreeMap::new();
        for cat in ["devices", "terminals", "fixtures", "casework", "equipment"] {
            for (k, v) in row[cat].as_object().unwrap() {
                if v.as_u64().unwrap() > 0 {
                    got.insert((cat.to_string(), k.clone()), v.as_u64().unwrap());
                }
            }
            let total: u64 = row[cat].as_object().unwrap().values().map(|v| v.as_u64().unwrap()).sum();
            assert_eq!(row["totals"][cat].as_u64(), Some(total));
        }
        assert_eq!(got, want, "type {}", row["type_name"]);
        terminals += row["totals"]["terminals"].as_u64().unwrap();
        assert!(row["totals"]["terminals"].as_u64().unwrap() >= 2 * row["count"].as_u64().unwrap(), "a diffuser and a grille per unit");
    }
    assert!(terminals > 0);
    let all: u64 = t.unit_instances.iter().map(|i| placed(&t, &truth_room(&t, &i.level, &i.number).id).values().sum::<u64>()).sum();
    let grand: u64 = ["devices", "terminals", "fixtures", "casework", "equipment"]
        .iter()
        .map(|c| r["grand_total"][c].as_object().unwrap().values().map(|v| v.as_u64().unwrap()).sum::<u64>())
        .sum();
    assert_eq!(grand, all);
    std::fs::remove_dir_all(&dir).ok();
}

/// MUST 5: an instance keyed `type` (the field view's) and one keyed `unit_type` (the
/// generator's) are both counted; `bspec.set` keeps the unit keys.
#[test]
fn both_instance_keys_are_accepted() {
    let mut s = Session::new();
    s.execute(
        "bspec.set",
        &json!({"spec": {
            "unit_types": [{"id": "K", "name": "King", "devices": {"receptacle": 2}, "terminals": {"supply_diffuser": 1}}],
            "unit_instances": [
                {"type": "K", "number": "201", "level": "2", "hand": "mirrored"},
                {"unit_type": "K", "number": "202", "level": "2", "hand": "normal"}
            ]
        }}),
    )
    .unwrap();
    let sched = s.execute("unittypes.schedule", &json!({})).unwrap();
    assert_eq!(sched["summary"], json!([{"type": "K", "type_name": "King", "count": 2, "normal": 1, "mirrored": 1}]));
    let r = s.execute("unittypes.takeoff", &json!({})).unwrap();
    assert_eq!(r["types"][0]["devices"]["receptacle"], 4);
    assert_eq!(r["types"][0]["terminals"]["supply_diffuser"], 2);
    assert_eq!(r["grand_total"]["terminals"]["supply_diffuser"], 2);
    // A later bspec.set keeps them.
    s.execute("bspec.set", &json!({"spec": {"units_per_foot": 12}})).unwrap();
    assert_eq!(s.execute("unittypes.schedule", &json!({})).unwrap()["instances"].as_array().unwrap().len(), 2);
}

/// The type a test edits: the king if the hotel has kings, else the double queen.
fn edited_type(s: &mut Session) -> (String, u64) {
    let list = s.execute("unittypes.list", &json!({})).unwrap();
    let sched = s.execute("unittypes.schedule", &json!({})).unwrap();
    let count = |id: &str| sched["instances"].as_array().unwrap().iter().filter(|i| i["type"] == id).count() as u64;
    for name in ["KING ROOM", "DOUBLE QUEEN ROOM"] {
        if let Some(t) = list["types"].as_array().unwrap().iter().find(|t| t["name"] == name) {
            let id = t["id"].as_str().unwrap().to_string();
            if count(&id) > 1 {
                return (id.clone(), count(&id));
            }
        }
    }
    panic!("no type with several instances");
}

fn type_total(s: &mut Session, id: &str, cat: &str, kind: &str) -> u64 {
    let r = s.execute("unittypes.takeoff", &json!({})).unwrap();
    r["types"].as_array().unwrap().iter().find(|t| t["type"] == id).unwrap()[cat][kind].as_u64().unwrap()
}

fn instance_value(s: &mut Session, number: &str, cat: &str, kind: &str) -> u64 {
    let r = s.execute("unittypes.takeoff", &json!({"instances": true})).unwrap();
    r["instances"].as_array().unwrap().iter().find(|i| i["number"] == number).unwrap()[cat][kind].as_u64().unwrap()
}

/// MUST 4: editing a type's receptacle count changes every instance of it but the ones that
/// override the receptacles; it can be previewed and undone.
#[test]
fn a_type_edit_reaches_every_instance_but_the_overridden() {
    let (mut s, _t, dir) = read("edit", 99, HotelSize::Full);
    let (id, count) = edited_type(&mut s);
    let sched = s.execute("unittypes.schedule", &json!({"type": id})).unwrap();
    let rows = sched["instances"].as_array().unwrap().clone();
    // A person overrides one plain instance's receptacles.
    let plain = rows.iter().find(|r| r["overrides"].as_array().unwrap().is_empty()).unwrap();
    let number = plain["number"].as_str().unwrap().to_string();
    s.execute("unittypes.override", &json!({"number": number, "set": {"devices.receptacle": 7}, "author": "estimator"})).unwrap();
    let overridden = 1 + rows.iter().filter(|r| r["overrides"].as_array().unwrap().iter().any(|o| o["key"] == "devices.receptacle")).count() as u64;
    let before = type_total(&mut s, &id, "devices", "receptacle");
    let per_unit = s.execute("unittypes.list", &json!({})).unwrap()["types"].as_array().unwrap().iter().find(|t| t["id"] == id.as_str()).unwrap()["devices"]
        ["receptacle"]
        .as_u64()
        .unwrap();
    // A preview changes nothing.
    let pv = s.execute("unittypes.edit", &json!({"type": id, "set": {"devices.receptacle": per_unit + 2}, "preview": true})).unwrap();
    assert_eq!(pv["instances"].as_u64(), Some(count));
    assert!(pv["kept"].as_array().unwrap().iter().any(|k| k["number"] == number.as_str() && k["value"] == "7"), "{pv}");
    assert_eq!(type_total(&mut s, &id, "devices", "receptacle"), before);
    let delta = 2;
    let r = s.execute("unittypes.edit", &json!({"type": id, "set": {"devices.receptacle": per_unit + delta}, "author": "estimator"})).unwrap();
    assert_eq!(r["changed"][0]["from"].as_u64(), Some(per_unit), "{r}");
    let after = type_total(&mut s, &id, "devices", "receptacle");
    assert_eq!(after - before, (count - overridden) * delta, "count {count}, overridden {overridden}");
    assert_eq!(instance_value(&mut s, &number, "devices", "receptacle"), 7, "the override is kept");
    let other = rows.iter().find(|r| r["number"] != number.as_str() && r["overrides"].as_array().unwrap().is_empty()).unwrap()["number"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(instance_value(&mut s, &other, "devices", "receptacle"), per_unit + delta);
    // Undo puts the type back.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(type_total(&mut s, &id, "devices", "receptacle"), before);
    // Bad edits are refused.
    for bad in [
        json!({"type": id, "set": {"devices.receptacle": -1}}),
        json!({"type": id, "set": {"id": "X"}}),
        json!({"type": "nope", "set": {"name": "X"}}),
        json!({"type": id}),
    ] {
        assert!(s.execute("unittypes.edit", &bad).is_err(), "{bad}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// MUST 3: an override (`flag`) changes its own instance's quantities only: taking it off brings
/// that instance back to its type, and no other instance moves.
fn override_changes_only_its_instance(flag: &str) {
    let (mut s, t, dir) = read(flag, 42, HotelSize::Small);
    let ti = t.unit_instances.iter().find(|i| i.overrides.get(flag).map(String::as_str) == Some("true")).unwrap();
    let each = |s: &mut Session| -> BTreeMap<String, Value> {
        let r = s.execute("unittypes.takeoff", &json!({"instances": true})).unwrap();
        r["instances"].as_array().unwrap().iter().map(|i| (i["number"].as_str().unwrap().to_string(), i.clone())).collect()
    };
    let with = each(&mut s);
    // In the schedule, with its source and author.
    let sched = s.execute("unittypes.schedule", &json!({"level": ti.level})).unwrap();
    let row = sched["instances"].as_array().unwrap().iter().find(|r| r["number"] == ti.number.as_str()).unwrap().clone();
    assert!(
        row["overrides"].as_array().unwrap().iter().any(|o| o["key"] == flag && o["author"] == "unittypes.read" && o["source"]["sheet"] == "A-401"),
        "{row}"
    );
    // Its quantities are what the room holds.
    let mine = &with[&ti.number];
    for ((cat, kind), n) in placed(&t, &ti.room) {
        assert_eq!(mine[cat.as_str()][kind.as_str()].as_u64(), Some(n), "{flag}: {cat}.{kind}");
    }
    let keys: Vec<String> = ti.overrides.keys().cloned().collect();
    s.execute("unittypes.override", &json!({"number": ti.number, "level": ti.level, "remove": keys})).unwrap();
    let without = each(&mut s);
    assert_ne!(with[&ti.number], without[&ti.number], "{flag}: the override changed the instance's takeoff");
    for (n, v) in &with {
        if n != &ti.number {
            assert_eq!(&without[n], v, "{flag}: room {n} moved");
        }
    }
    // Without its override it is a plain instance of its type.
    let plain = t.unit_instances.iter().find(|i| i.unit_type == ti.unit_type && i.overrides.is_empty()).unwrap();
    let strip = |v: &Value| {
        let mut v = v.clone();
        for k in ["number", "level", "hand", "overridden"] {
            v.as_object_mut().unwrap().remove(k);
        }
        v
    };
    assert_eq!(strip(&without[&ti.number]), strip(&with[&plain.number]), "{flag}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ada_override_changes_only_its_room() {
    override_changes_only_its_instance("ada");
}

#[test]
fn end_unit_override_changes_only_its_room() {
    override_changes_only_its_instance("end_unit");
}

#[test]
fn column_override_changes_only_its_room() {
    override_changes_only_its_instance("column");
}

/// MUST 3: a re-read keeps the overrides and type edits a person made, and doesn't double the
/// read ones.
#[test]
fn overrides_and_edits_survive_a_re_read() {
    let (mut s, t, dir) = read("reread", 42, HotelSize::Small);
    let plain = t.unit_instances.iter().find(|i| i.overrides.is_empty()).unwrap();
    s.execute(
        "unittypes.override",
        &json!({"number": plain.number, "set": {"casework.wardrobe": 0, "column": "true"}, "author": "field crew", "note": "column found on site"}),
    )
    .unwrap();
    let list = s.execute("unittypes.list", &json!({})).unwrap();
    let id = list["types"][0]["id"].as_str().unwrap().to_string();
    s.execute("unittypes.edit", &json!({"type": id, "set": {"name": "RENAMED"}})).unwrap();
    let before = s.execute("unittypes.schedule", &json!({})).unwrap();
    s.execute("unittypes.read", &json!({"dir": dir.display().to_string()})).unwrap();
    let after = s.execute("unittypes.schedule", &json!({})).unwrap();
    assert_eq!(before["instances"], after["instances"]);
    let row = after["instances"].as_array().unwrap().iter().find(|r| r["number"] == plain.number.as_str()).unwrap().clone();
    let ov = row["overrides"].as_array().unwrap();
    assert_eq!(ov.len(), 2, "{row}");
    assert!(ov.iter().all(|o| o["author"] == "field crew" && o["note"] == "column found on site"), "{row}");
    assert_eq!(s.execute("unittypes.list", &json!({})).unwrap()["types"][0]["name"], "RENAMED");
    std::fs::remove_dir_all(&dir).ok();
}

/// MUST 2: placing a mirrored instance puts its wet wall, PTAC sleeve and its receptacle,
/// switch, casework and grille on the mirrored side, where the hotel put them.
#[test]
fn mirrored_instances_place_on_the_other_side() {
    let (mut s, t, dir) = read("place", 42, HotelSize::Small);
    let r = s.execute("unittypes.place", &json!({})).unwrap();
    let insts = r["instances"].as_array().unwrap();
    assert_eq!(insts.len(), t.unit_instances.len());
    let mut mirrored = 0;
    for inst in insts {
        let room = truth_room(&t, inst["level"].as_str().unwrap(), inst["number"].as_str().unwrap());
        let mid = (room.min_ft[0] + room.max_ft[0]) / 2.0;
        let side = |x: f64| if x < mid { "left" } else { "right" };
        let wc = t.fixtures.iter().find(|f| f.room == room.id && f.kind == "water_closet").unwrap();
        let ptac = t.equip_connections.iter().find(|e| e.room == room.id && e.kind == "ptac").unwrap();
        assert_eq!(inst["wet_wall_side"], side(wc.at_ft[0]), "{}", inst["number"]);
        assert_eq!(inst["hvac_sleeve_side"], side(ptac.at_ft[0]), "{}", inst["number"]);
        let items = inst["items"].as_array().unwrap();
        let truth_items = cadaddy_mepfix::units::room_items(&t, &room.id);
        for it in items {
            let at = [it["at_ft"][0].as_f64().unwrap(), it["at_ft"][1].as_f64().unwrap()];
            let kind = match it["kind"].as_str().unwrap() {
                "ptac_sleeve" | "ptac_receptacle" => "ptac",
                k => k,
            };
            let near = truth_items.iter().filter(|x| x.kind == kind).map(|x| (x.at_ft[0] - at[0]).hypot(x.at_ft[1] - at[1])).fold(f64::MAX, f64::min);
            assert!(near < 0.05, "room {} ({}): {} at {at:?} is {near} ft from the hotel's", inst["number"], inst["hand"], it["kind"]);
        }
        let kinds = |k: &str| items.iter().filter(|i| i["kind"] == k).count();
        assert_eq!((kinds("ptac"), kinds("ptac_sleeve"), kinds("ptac_receptacle")), (1, 1, 1));
        assert_eq!(kinds("water_closet"), 1);
        mirrored += usize::from(inst["hand"] == "mirrored");
    }
    assert!(mirrored > 3);
    // The column room places no wardrobe; the end unit's receptacles (one more, respaced) and the
    // ADA room's shower are counted, not placed.
    let col = t.unit_instances.iter().find(|i| i.overrides.contains_key("column")).unwrap();
    let r = s.execute("unittypes.place", &json!({"number": col.number})).unwrap();
    assert!(r["instances"][0]["items"].as_array().unwrap().iter().all(|i| i["kind"] != "wardrobe"));
    assert_eq!(r["instances"][0]["unplaced"], json!([]));
    let end = t.unit_instances.iter().find(|i| i.overrides.contains_key("end_unit")).unwrap();
    let r = s.execute("unittypes.place", &json!({"number": end.number})).unwrap();
    let n = end.overrides["devices.receptacle"].parse::<u64>().unwrap();
    assert_eq!(r["instances"][0]["unplaced"], json!([{"category": "devices", "kind": "receptacle", "count": n}]));
    assert!(r["instances"][0]["items"].as_array().unwrap().iter().all(|i| i["kind"] != "receptacle"));
    let ada = t.unit_instances.iter().find(|i| i.overrides.contains_key("ada")).unwrap();
    let r = s.execute("unittypes.place", &json!({"number": ada.number})).unwrap();
    assert_eq!(r["instances"][0]["unplaced"], json!([{"category": "fixtures", "kind": "shower", "count": 1}]));
    std::fs::remove_dir_all(&dir).ok();
}

/// MUST 5: detect on an open floor plan finds every unit by its tag, with its type and hand,
/// exactly as the truth has them on that level.
#[test]
fn detect_finds_the_units_on_the_open_plan() {
    let (t, dir) = sheets("detect", 42, HotelSize::Small);
    let mut s = Session::new();
    let plan = cadaddy_io::read_dxf(&std::fs::read(dir.join("A-103.dxf")).unwrap()).unwrap();
    s.open_drawing(plan, "A-103", None);
    s.execute("unittypes.read", &json!({"dir": dir.display().to_string()})).unwrap();
    let r = s.execute("unittypes.detect", &json!({})).unwrap();
    let detected = r["detected"].as_array().unwrap();
    let level: Vec<_> = t.unit_instances.iter().filter(|i| i.level == "3").collect();
    assert_eq!(detected.len(), level.len(), "{r}");
    let list = s.execute("unittypes.list", &json!({})).unwrap();
    for d in detected {
        let ti = level.iter().find(|i| i.number == d["number"].as_str().unwrap()).unwrap();
        let truth_name = &t.unit_types.iter().find(|u| u.id == ti.unit_type).unwrap().name;
        assert_eq!(&type_name_of(&list["types"], d["suggested_type"].as_str().unwrap()), truth_name, "{d}");
        assert_eq!(d["hand"], ti.hand.as_str());
        assert_eq!(d["confidence"], 1.0, "{d}");
        assert_eq!(d["by"], "tag");
    }
    let total: u64 = r["types"].as_array().unwrap().iter().map(|t| t["count"].as_u64().unwrap()).sum();
    assert_eq!(total as usize, level.len());
    std::fs::remove_dir_all(&dir).ok();
}

/// MUST 6: hostile quantities saturate instead of wrapping, and the commands refuse bad input
/// without a panic.
#[test]
fn hostile_quantities_saturate_and_never_panic() {
    let mut s = Session::new();
    let big = u64::MAX;
    let instances: Vec<Value> = (0..60_100)
        .map(|i| json!({"type": "H", "number": format!("{i}"), "level": "2", "overrides": {"devices.x": "18446744073709551615"}}))
        .collect();
    s.execute(
        "bspec.set",
        &json!({"spec": {
            "unit_types": [
                {"id": "H", "name": "Huge", "devices": {"receptacle": big, "light": 1e300, "neg": -5, "nan": "NaN", "text": "99999999999999999999999", "frac": 2.7},
                 "items": [{"category": "devices", "kind": "receptacle", "u": 1e308, "v": -1e308}], "width_ft": 1e308, "depth_ft": "x"},
                {"id": 5}, "junk"
            ],
            "unit_instances": instances
        }}),
    )
    .unwrap();
    let r = s.execute("unittypes.takeoff", &json!({"instances": false})).unwrap();
    let row = &r["types"][0];
    assert_eq!(row["count"].as_u64(), Some(50_000), "instances are capped");
    assert_eq!(row["devices"]["receptacle"].as_u64(), Some(big), "saturated, not wrapped");
    assert_eq!(row["devices"]["light"].as_u64(), Some(big));
    assert_eq!(row["devices"]["x"].as_u64(), Some(big));
    assert_eq!(row["devices"]["neg"].as_u64(), Some(0));
    assert_eq!(row["devices"]["nan"].as_u64(), Some(0));
    assert_eq!(row["devices"]["text"].as_u64(), Some(big));
    assert_eq!(row["devices"]["frac"].as_u64(), Some(100_000));
    assert_eq!(row["totals"]["devices"].as_u64(), Some(big));
    assert_eq!(r["grand_total"]["devices"]["receptacle"].as_u64(), Some(big));
    for (cmd, p) in [
        ("unittypes.schedule", json!({"level": 5, "type": null})),
        ("unittypes.place", json!({"number": {"a": 1}})),
        ("unittypes.detect", json!({"tolerance": "x"})),
        ("unittypes.list", json!(null)),
    ] {
        let _ = s.execute(cmd, &p);
    }
    for (cmd, p) in [
        ("unittypes.edit", json!({"type": "H", "set": {"devices.receptacle": 1e300}})),
        ("unittypes.edit", json!({"type": "H", "set": {"width_ft": f64::MAX}})),
        ("unittypes.edit", json!({"type": "", "set": {"name": "X"}})),
        ("unittypes.edit", json!({"type": "junk", "set": {"name": "X"}})),
        ("unittypes.override", json!({"number": "1", "set": {"devices.receptacle": -3}})),
        ("unittypes.override", json!({"number": "nope", "set": {"ada": true}})),
        ("unittypes.override", json!({"number": "1"})),
        ("unittypes.read", json!({})),
        ("unittypes.read", json!({"files": [5]})),
        ("unittypes.read", json!({"dir": "/no/such/folder/anywhere"})),
    ] {
        assert!(s.execute(cmd, &p).is_err(), "{cmd} {p}");
    }
    // The schedule's `table` stub is gone from the command.
    let spec = cadaddy_engine::find_command("unittypes.schedule").unwrap();
    assert!(!spec.params.contains("table"));
}
