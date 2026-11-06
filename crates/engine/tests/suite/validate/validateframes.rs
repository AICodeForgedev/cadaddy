//! Validate on a three-level job whose upper floors' sheets are drawn far from the first floor's
//! (Q6126): each level's plan on its own architectural sheet, thousands of feet apart in the
//! drawing, each level's power plan registered onto its architectural plan by a shift, the
//! levels stacked by `model.build`. Every level's areas, elements, walls and print crops come out
//! in the one building frame the 3D model uses; `validate` and `validate.packet` take their scope
//! from `room` / `level` / `area` as `validate.prescreen` does; `rooms.list {level}` filters.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
/// Each level's architectural sheet starts at (0, y); its power plan at (E_X, y).
const LEVELS: [(&str, f64, f64); 3] = [("L1", 0.0, 0.0), ("L2", 10.0, 60_000.0), ("L3", 20.0, 91_200.0)];
const E_X: f64 = 4000.0;
/// The plan's origin on its sheet (inches).
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on the sheet starting at (sx, sy).
fn at(sx: f64, sy: f64, p: [f64; 2]) -> [f64; 2] {
    [sx + OX + p[0] * 12.0, sy + OY + p[1] * 12.0]
}

fn rect(s: &mut Session, sx: f64, sy: f64, a: [f64; 2], b: [f64; 2]) {
    let v = [at(sx, sy, a), at(sx, sy, [b[0], a[1]]), at(sx, sy, b), at(sx, sy, [a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn line(s: &mut Session, sx: f64, sy: f64, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [at(sx, sy, a), at(sx, sy, b)]})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": p, "text": t, "height": 6})).unwrap();
}

/// A floor: a 100' × 12' shell of 6" walls, partitions at 15' and 30' (an office, a storage room
/// and a corridor).
fn walls(s: &mut Session, sx: f64, sy: f64, ext: &str, part: &str) {
    layer(s, ext);
    rect(s, sx, sy, [0.0, 0.0], [100.0, 12.0]);
    rect(s, sx, sy, [0.5, 0.5], [99.5, 11.5]);
    layer(s, part);
    line(s, sx, sy, [15.0, 0.5], [15.0, 11.5]);
    line(s, sx, sy, [30.0, 0.5], [30.0, 11.5]);
}

/// Where the receptacle of a level is on the building (feet): on the office's south wall face.
const RECEP: [f64; 2] = [6.0, 0.5];

/// Where a receptacle block is drawn on each level's plan (feet): on the storage room's north
/// wall face.
const PLAN_RECEP: [f64; 2] = [22.0, 11.5];

/// A receptacle block (TYPE `receptacle`) at a sheet point (drawing inches) on E-POWR.
fn receptacle_block(s: Session, p: [f64; 2]) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key("RECEP") {
        let mut b = Block::new("RECEP");
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert("RECEP".into(), Arc::new(b));
    }
    let attribs = vec![Attrib {
        tag: "TYPE".into(),
        text: Text { value: "receptacle".into(), ..style },
        invisible: true,
        constant: false,
        prompt: String::new(),
        props: AttribProps::default(),
        multiline: None,
        modes: AttribModes::default(),
    }];
    let ins = Insert {
        block: "RECEP".into(),
        insert: Vec3::new(p[0], p[1], 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs,
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    let h = d.new_handle();
    let mut e = Entity::new(h, EntityKind::Insert(ins));
    e.common.layer = "E-POWR".into();
    d.model.push(e);
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    s2
}

/// A receptacle the readers put in the spec, read on the level's power plan.
fn device(level: &str, sheet: &str, sheet_at: [f64; 2]) -> Value {
    let fields = json!({
        "type": {"value": "A", "source": "plan"},
        "device_type": {"value": "duplex receptacle", "source": "plan"},
        "sheet": {"value": sheet, "source": "plan"},
        "symbol": {"value": "1F0", "source": "plan"},
        "circuit": {"value": "LP1-3", "source": "plan"},
    });
    json!({"id": format!("device:{level}:{:.1},{:.1}", sheet_at[0], sheet_at[1]), "kind": "device", "level": level,
           "at_ft": [sheet_at[0] / 12.0, sheet_at[1] / 12.0], "fields": fields})
}

fn spec() -> Value {
    let mut levels = Vec::new();
    let mut sheets = Vec::new();
    let mut rooms = Vec::new();
    let mut tags = Vec::new();
    for (i, (name, elev, y)) in LEVELS.iter().enumerate() {
        let n = i + 1;
        levels.push(json!({"name": name, "elevation_ft": elev, "height_ft": 10, "ceiling_ft": 9, "region": [0, y, SW, y + SH]}));
        sheets.push(json!({"number": format!("A-{n}01"), "title": format!("FLOOR PLAN - LEVEL {n}"), "discipline": "A", "level": name,
                           "source": "model", "rect": [0, y, SW, y + SH],
                           "registration": {"reference": format!("A-{n}01"), "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}}));
        sheets.push(json!({"number": format!("E-{n}01"), "title": format!("POWER PLAN - LEVEL {n}"), "discipline": "E", "level": name,
                           "source": "model", "rect": [E_X, y, E_X + SW, y + SH],
                           "registration": {"reference": format!("A-{n}01"), "method": "tags", "a": 1.0, "b": 0.0, "tx": -E_X, "ty": 0.0, "matched": 4}}));
        for (r, nm) in [(1, "Office"), (2, "Storage"), (3, "Corridor")] {
            rooms.push(json!({"name": nm, "number": format!("{n}0{r}"), "level": name}));
        }
        tags.push(device(name, &format!("E-{n}01"), at(E_X, *y, RECEP)));
    }
    json!({
        "units_per_foot": 12,
        "stack": true,
        "levels": levels,
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"},
            {"layer": "E-POWR", "role": "device"}
        ],
        "devices": {"by_type": {"receptacle": {"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]}}},
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": rooms,
        "sheets": sheets,
        "tags": tags
    })
}

fn job() -> Session {
    let mut s = Session::new();
    for (i, (_, _, y)) in LEVELS.iter().enumerate() {
        let n = i + 1;
        walls(&mut s, 0.0, *y, "A-WALL", "A-WALL-CL");
        layer(&mut s, "A-ROOM-TAG");
        text(&mut s, at(0.0, *y, [5.0, 6.0]), "OFFICE");
        text(&mut s, at(0.0, *y, [20.0, 6.0]), "STORAGE");
        text(&mut s, at(0.0, *y, [60.0, 6.0]), "CORRIDOR");
        // The power plan: the walls as background, the receptacle's symbol.
        walls(&mut s, E_X, *y, "G-BKGD", "G-BKGD");
        layer(&mut s, "E-SYM");
        let p = at(E_X, *y, RECEP);
        s.execute("circle", &json!({"center": p, "radius": 6})).unwrap();
        // Sheet frames.
        layer(&mut s, "G-FRAME");
        for x in [0.0, E_X] {
            s.execute("pline", &json!({"vertices": [[x, *y], [x + SW, *y], [x + SW, *y + SH], [x, *y + SH]], "closed": true})).unwrap();
        }
        let _ = n;
    }
    for (_, _, y) in LEVELS {
        s = receptacle_block(s, at(0.0, y, PLAN_RECEP));
    }
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    let b = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(b["stacked"], json!(2), "the upper levels are stacked onto the first: {b}");
    s
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-vframes-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

fn nums(v: &Value) -> Vec<f64> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default()
}

fn areas(s: &mut Session, p: Value) -> Vec<Value> {
    s.execute("validate.areas", &p).unwrap()["areas"].as_array().cloned().unwrap_or_default()
}

/// The built device of a level.
fn device_of(s: &Session, level: &str) -> String {
    let d = s.doc().unwrap();
    let hs: Vec<String> = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("device"))
        .filter(|e| cadaddy_engine::cmd::bspec::made_level(e).is_some_and(|l| l == level))
        .map(|e| e.handle.hex())
        .collect();
    assert_eq!(hs.len(), 1, "one built receptacle on {level}: {hs:?}");
    hs[0].clone()
}

#[test]
fn every_levels_areas_overlap_the_first_floors_footprint() {
    let mut s = job();
    let all = areas(&mut s, json!({}));
    for (level, _, _) in LEVELS {
        let office = all
            .iter()
            .find(|a| a["level"] == level && a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("office")))
            .unwrap_or_else(|| panic!("no office on {level}: {all:?}"));
        let b = nums(&office["box"]);
        assert!(
            (b[0] - 0.5).abs() < 0.6 && (b[1] - 0.5).abs() < 0.6 && (b[2] - 15.0).abs() < 0.6 && (b[3] - 11.5).abs() < 0.6,
            "{level}'s office in building feet, over the first floor's: {b:?}"
        );
    }
    // A level's areas only.
    let l2 = areas(&mut s, json!({"level": "L2"}));
    assert!(!l2.is_empty() && l2.iter().all(|a| a["level"] == "L2"), "{l2:?}");
}

#[test]
fn an_upper_floor_element_stands_in_its_room_by_its_wall() {
    let mut s = job();
    for (level, _, _) in LEVELS {
        let h = device_of(&s, level);
        let r = s.execute("bim.element", &json!({"handle": h})).unwrap();
        assert_eq!(r["level"], level, "{r:#}");
        let a = nums(&r["at"]);
        assert!((a[0] - RECEP[0]).abs() < 1.0 && (a[1] - RECEP[1]).abs() < 1.0, "{level}'s receptacle at the same building point: {r:#}");
        assert!(r["nearest_wall"].is_object(), "{level}'s receptacle has a wall by it: {r:#}");
        assert!(r["host"].is_object(), "{level}'s receptacle is on its wall: {r:#}");
    }
    // The prescreen measures the upper floors' hosts too: nothing is far from its wall.
    for (level, _, _) in LEVELS {
        let r = s.execute("validate.prescreen", &json!({"level": level, "rules": ["G-HOST"], "all": true, "limit": 50})).unwrap();
        let host: Vec<&Value> = r["findings"].as_array().unwrap().iter().filter(|f| f["rule"] == "G-HOST").collect();
        // The plan's receptacle, and the built device the job's tag names a receptacle (Q6128).
        assert_eq!(host.len(), 2, "{level}'s receptacles are measured: {r:#}");
        assert!(host.iter().all(|f| f["verdict"] == "pass" && f["level"] == level && f["kind"] == "receptacle"), "{level}: {r:#}");
        let plan: Vec<&&Value> = host.iter().filter(|f| f["built"] == false).collect();
        assert_eq!(plan.len(), 1, "{level}: {r:#}");
        let a = nums(&plan[0]["at"]);
        assert!((a[0] - PLAN_RECEP[0]).abs() < 1.0 && (a[1] - PLAN_RECEP[1]).abs() < 1.0, "{level}: {r:#}");
    }
}

#[test]
fn every_levels_packet_has_walls_and_print_crops() {
    let mut s = job();
    let first_around = {
        let mut around = None;
        for (i, (level, _, _)) in LEVELS.iter().enumerate() {
            let n = i + 1;
            let id = format!("{level}/{n}01");
            let out = dir(&format!("packet-{level}"));
            let m = s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy()})).unwrap();
            let objects = read_json(&out.join("objects.json"));
            let walls = objects["walls"].as_array().map_or(0, Vec::len);
            assert!(walls >= 3, "{level}'s office packet holds its walls: {objects:#}");
            assert!(
                objects["elements"]
                    .as_array()
                    .is_some_and(|e| e.iter().any(|x| x["kind"].as_str().is_some_and(|k| k.contains("device") || k.contains("recept")))),
                "{level}'s office packet holds its receptacle: {objects:#}"
            );
            let prints: Vec<String> = m["prints"].as_array().unwrap().iter().filter_map(|p| p["sheet"].as_str().map(str::to_string)).collect();
            assert!(prints.contains(&format!("A-{n}01")) && prints.contains(&format!("E-{n}01")), "{level}: {prints:?} {:#}", m["left_out"]);
            let not_placed: Vec<&Value> =
                m["left_out"].as_array().unwrap().iter().filter(|l| l["why"].as_str().is_some_and(|w| w.contains("not placed"))).collect();
            assert!(not_placed.is_empty(), "{level}: {not_placed:?}");
            // The 3D view looks where the build put the level: over the first floor.
            let r3 = m["model"].as_array().unwrap().iter().find(|x| x["kind"] == "3d").unwrap_or_else(|| panic!("{m:#}"));
            let a = nums(&r3["render"]["around"]);
            match &around {
                None => around = Some(a),
                Some(first) => assert!(a.iter().zip(first).all(|(x, y)| (x - y).abs() < 1.0), "{level}: {a:?} vs {first:?}"),
            }
            let _ = std::fs::remove_dir_all(&out);
        }
        around
    };
    assert!(first_around.is_some());
}

#[test]
fn validate_takes_its_scope_from_the_room_given() {
    let mut s = job();
    let out = dir("run");
    let r = s.execute("validate", &json!({"room": "301", "out_dir": out.to_string_lossy()})).unwrap();
    assert_eq!(r["areas_total"], json!(1), "{r:#}");
    assert_eq!(r["scope"]["scope"], "room", "{r:#}");
    let r = s.execute("validate", &json!({"level": "L2", "out_dir": out.to_string_lossy()})).unwrap();
    assert_eq!(r["scope"]["scope"], "level", "{r:#}");
    assert!(r["areas"].as_array().unwrap().iter().all(|a| a["id"].as_str().is_some_and(|i| i.starts_with("L2/"))), "{r:#}");
    let _ = std::fs::remove_dir_all(&out);
    // The packet too: a room's one area.
    let out = dir("packet-room");
    let m = s.execute("validate.packet", &json!({"room": "201", "out_dir": out.to_string_lossy()})).unwrap();
    assert_eq!(m["area"]["id"], "L2/201", "{m:#}");
    let _ = std::fs::remove_dir_all(&out);
    // A room number on no level names no area.
    let out = dir("packet-none");
    assert!(s.execute("validate.packet", &json!({"room": "999", "out_dir": out.to_string_lossy()})).is_err());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn rooms_list_filters_by_level_in_the_models_frame() {
    let mut s = job();
    let all = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    assert_eq!(all["count"], json!(9), "{all:#}");
    let l2 = s.execute("rooms.list", &json!({"level": "L2", "outline": false})).unwrap();
    let rooms = l2["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 3, "{l2:#}");
    assert!(rooms.iter().all(|r| r["level"] == "L2"), "{l2:#}");
    // Where the 3D model has them: the second floor's office over the first's.
    let office = |v: &Value| -> Vec<f64> {
        nums(&v["rooms"].as_array().unwrap().iter().find(|r| r["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("office"))).unwrap()["at"])
    };
    let l1 = s.execute("rooms.list", &json!({"level": "L1", "outline": false})).unwrap();
    let (a, b) = (office(&l1), office(&l2));
    assert!((a[0] - b[0]).abs() < 12.0 && (a[1] - b[1]).abs() < 12.0, "{a:?} vs {b:?}");
    assert!(s.execute("rooms.list", &json!({"level": "L9"})).is_err());
}
