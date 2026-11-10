//! Q6420 arexport set: a whole building as the field app's package set (`bim.export.ar {set}`):
//! a generic building model, one package per level and one per room, and `building.json`, the
//! index with the levels, rooms, big equipment, main systems, package hashes and the connection
//! graph, following the owner's four views (architectural, MEP mains and full, floor, room),
//! never one package with everything. A made-up two-storey building only.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Arc, Drawing, Entity, EntityKind, Handle, Line, Mesh, MeshFace, RawTag};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// Feet between the two storeys' plans on the sheet.
const L2_DX: f64 = 100.0;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-arset-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn push(d: &mut Drawing, layer: &str, kind: EntityKind) -> Handle {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, kind);
    e.common.layer = layer.into();
    d.model.push(e);
    h
}

/// A line in feet.
fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    push(d, layer, EntityKind::Line(Line { a: p(a), b: p(b) }));
}

/// A room's inner faces from (x0, y0) to (x1, y1) feet, with door gaps `(x from, x to)` in its
/// top (`top: true`) or bottom side.
fn room(d: &mut Drawing, dx: f64, x0: f64, y0: f64, x1: f64, y1: f64, top: &[(f64, f64)], bottom: &[(f64, f64)]) {
    let side = |d: &mut Drawing, y: f64, gaps: &[(f64, f64)]| {
        let mut at = x0;
        for (a, b) in gaps {
            line(d, "A-WALL", (dx + at, y), (dx + a, y));
            at = *b;
        }
        line(d, "A-WALL", (dx + at, y), (dx + x1, y));
    };
    side(d, y0, bottom);
    side(d, y1, top);
    line(d, "A-WALL", (dx + x0, y0), (dx + x0, y1));
    line(d, "A-WALL", (dx + x1, y0), (dx + x1, y1));
}

/// A door in a horizontal wall (faces at `y0` and `y1`) from `x0` to `x1` feet: jambs, the leaf
/// and its swing.
fn door(d: &mut Drawing, dx: f64, x0: f64, x1: f64, y0: f64, y1: f64) {
    line(d, "A-WALL", (dx + x0, y0), (dx + x0, y1));
    line(d, "A-WALL", (dx + x1, y0), (dx + x1, y1));
    line(d, "A-DOOR", (dx + x0, y0), (dx + x0, y0 - (x1 - x0)));
    push(
        d,
        "A-DOOR",
        EntityKind::Arc(Arc {
            center: Vec3 { x: (dx + x0) * 12.0, y: y0 * 12.0, z: 0.0 },
            radius: (x1 - x0) * 12.0,
            start: 1.5 * std::f64::consts::PI,
            end: 0.0,
        }),
    );
}

/// One storey 60 x 30 ft: two guest rooms and an electrical room off a corridor, 6" walls, a
/// window in each guest room's outside wall.
fn storey(d: &mut Drawing, dx: f64) {
    // The outside faces, with the windows' gaps in the south wall.
    for (a, b) in [((0., 30.), (60., 30.)), ((60., 0.), (60., 30.)), ((0., 0.), (0., 30.))] {
        line(d, "A-WALL", (dx + a.0, a.1), (dx + b.0, b.1));
    }
    for (a, b) in [(0.0, 8.0), (12.0, 28.0), (32.0, 60.0)] {
        line(d, "A-WALL", (dx + a, 0.0), (dx + b, 0.0));
    }
    room(d, dx, 0.5, 0.5, 19.75, 19.5, &[(8.0, 11.0)], &[(8.0, 12.0)]);
    room(d, dx, 20.25, 0.5, 39.75, 19.5, &[(28.0, 31.0)], &[(28.0, 32.0)]);
    room(d, dx, 40.25, 0.5, 59.5, 19.5, &[(48.0, 51.0)], &[]);
    room(d, dx, 0.5, 20.0, 59.5, 29.5, &[], &[(8.0, 11.0), (28.0, 31.0), (48.0, 51.0)]);
    for (a, b) in [(8.0, 11.0), (28.0, 31.0), (48.0, 51.0)] {
        door(d, dx, a, b, 19.5, 20.0);
    }
}

fn plan() -> Drawing {
    let mut d = Drawing::new_imperial();
    storey(&mut d, 0.0);
    storey(&mut d, L2_DX);
    d
}

fn text_on(s: &mut Session, layer: &str, at: [f64; 2], t: &str) {
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
}

fn device(level: &str, x: f64, y: f64, mark: &str, kind: &str, extra: Value) -> Value {
    let mut fields = json!({
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("{kind} {mark}"), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "symbol": {"value": format!("S{x:.0}{y:.0}"), "source": "plan"},
    });
    if let (Some(f), Some(x)) = (fields.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            f.insert(k.clone(), json!({"value": v, "source": "plan"}));
        }
    }
    json!({"id": format!("device:{level}:{x:.1},{y:.1}:{mark}"), "kind": kind, "level": level, "at_ft": [x, y], "fields": fields})
}

fn spec() -> Value {
    let (l2x, l2) = (L2_DX * 12.0, L2_DX);
    let eq = |tag: &str, kind: &str, level: &str, min: [f64; 2], max: [f64; 2], bottom: f64, height: f64| json!({"tag": tag, "kind": kind, "level": level, "min": min, "max": max, "bottom_ft": bottom, "height_ft": height});
    let mut ahu = eq("AHU-1", "ahu", "L2", [l2x + 600.0, 60.0], [l2x + 660.0, 108.0], 0.0, 5.0);
    ahu["systems"] = json!(["SA"]);
    json!({
        "version": 1, "units_per_foot": 12, "stack": true,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0], "region": [-60, -60, 780, 420]},
            {"name": "L2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "base": [l2x, 0], "region": [l2x - 60.0, -60, l2x + 780.0, 420]}
        ],
        "layers": [
            {"layer": "A-WALL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-DOOR", "role": "door"},
            {"layer": "A-GLAZ", "role": "window"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "openings": {"enabled": true, "min_ft": 1.5, "max_ft": 16.0, "door_head_ft": 7.0, "window_head_ft": 7.0, "window_sill_ft": 3.0},
        "tags": [
            device("L1", 4.0, 0.8, "REC", "receptacle", json!({"mount_height_in": "18", "circuit": "PU-1-1"})),
            device("L1", 16.0, 0.8, "REC", "receptacle", json!({"mount_height_in": "18", "circuit": "PU-1-3"})),
            device("L1", 10.0, 10.0, "A", "light_fixture", json!({"circuit": "PU-1-5"})),
            device("L1", 30.0, 10.0, "A", "light_fixture", json!({"circuit": "PU-2-5"})),
            device("L1", 24.0, 0.8, "REC", "receptacle", json!({"mount_height_in": "18", "circuit": "7"})),
            device("L2", l2 + 10.0, 10.0, "A", "light_fixture", json!({"circuit": "PH-2"}))
        ],
        "mep": {
            "systems": [
                {"name": "SA", "layer": "M-SUPPLY", "kind": "duct", "size_in": [24, 12], "bottom_ft": 8, "duct_type": "supply"},
                {"name": "CW", "layer": "P-DOMW-CW", "kind": "pipe", "size_in": [3, 0], "bottom_ft": 9},
                {"name": "EA", "layer": "M-EXHAUST", "kind": "duct", "size_in": [6, 0], "bottom_ft": 8.5, "duct_type": "exhaust"},
                {"name": "FP", "layer": "F-SPRK", "kind": "pipe", "size_in": [4, 0], "bottom_ft": 9.2},
                {"name": "PWR", "layer": "E-POWER", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9.4},
                {"name": "E-FEEDER", "layer": "E-FDR", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9.6},
                {"name": "DATA", "layer": "T-DATA", "kind": "conduit", "size_in": [1.25, 0], "bottom_ft": 9.6}
            ],
            "runs": [
                {"system": "SA", "level": "L1", "points": [[24, 288], [696, 288]], "size_in": [24, 12], "bottoms_ft": [8]},
                {"system": "SA", "level": "L1", "points": [[120, 288], [120, 120]], "size_in": [10, 8], "bottoms_ft": [8]},
                {"system": "CW", "level": "L1", "points": [[24, 300], [696, 300]], "size_in": [3, 0], "bottoms_ft": [9]},
                {"system": "CW", "level": "L1", "points": [[360, 300], [360, 120]], "size_in": [0.75, 0], "bottoms_ft": [9]},
                {"system": "CW", "level": "L1", "points": [[372, 300]], "size_in": [0.75, 0], "bottoms_ft": [9], "top_ft": 19},
                {"system": "EA", "level": "L1", "points": [[60, 60], [60, 220]], "size_in": [6, 0], "bottoms_ft": [8.5]},
                {"system": "FP", "level": "L1", "points": [[24, 312], [696, 312]], "size_in": [4, 0], "bottoms_ft": [9.2]},
                {"system": "FP", "level": "L1", "points": [[240, 312], [240, 120]], "size_in": [1, 0], "bottoms_ft": [9.2]},
                {"system": "FP", "level": "L1", "points": [[30, 312]], "size_in": [4, 0], "bottoms_ft": [9.2], "top_ft": 19.2},
                {"system": "PWR", "level": "L1", "points": [[600, 200], [600, 120]], "size_in": [0.75, 0], "bottoms_ft": [9.4]},
                {"system": "PWR", "level": "L1", "points": [[690, 200]], "size_in": [0.75, 0], "bottoms_ft": [9.4], "top_ft": 19.4},
                {"system": "PWR", "level": "L1", "points": [[500, 200]], "size_in": [0.75, 0], "bottoms_ft": [9.4], "top_ft": 19.4},
                {"system": "PWR", "level": "L1", "points": [[400, 200], [400, 150]], "size_in": [1.25, 0], "bottoms_ft": [9.4]},
                {"system": "E-FEEDER", "level": "L1", "points": [[420, 200], [420, 150]], "size_in": [0.75, 0], "bottoms_ft": [9.6]},
                {"system": "DATA", "level": "L1", "points": [[440, 200], [440, 150]], "size_in": [1.25, 0], "bottoms_ft": [9.6]}
            ],
            "terminals": [
                {"system": "SA", "level": "L1", "at": [120, 108], "size_in": [12, 12], "bottom_ft": 9, "terminal_kind": "diffuser"},
                {"system": "FP", "level": "L1", "at": [240, 120], "size_in": [1, 0], "bottom_ft": 9, "terminal_kind": "sprinkler_pendent"}
            ],
            "equipment": [
                eq("MDP", "switchboard", "L1", [600.0, 12.0], [672.0, 36.0], 0.0, 7.5),
                eq("PH", "panel", "L1", [540.0, 6.0], [560.0, 12.0], 1.0, 5.0),
                eq("PU-1", "panel", "L1", [200.0, 6.0], [214.0, 10.0], 4.0, 2.5),
                eq("PU-2", "panel", "L1", [440.0, 6.0], [454.0, 10.0], 4.0, 2.5),
                ahu,
                eq("EF-1", "exhaust_fan", "L1", [54.0, 54.0], [66.0, 66.0], 8.6, 0.8),
                eq("ELEV-1", "elevator", "L1", [640.0, 180.0], [700.0, 240.0], 0.0, 20.0),
                eq("DS-ELEV", "disconnect", "L1", [620.0, 236.0], [628.0, 240.0], 4.0, 1.5),
                eq("FPMP-1", "fire_pump", "L1", [24.0, 300.0], [48.0, 324.0], 0.0, 3.0)
            ],
            "feeders": [
                {"tag": "F-PH", "from": "MDP", "to": "PH", "size": "#2", "conduits": 1, "conduit_in": 1.25},
                {"tag": "F-PU1", "from": "PH", "to": "PU-1", "size": "#6", "conduits": 1, "conduit_in": 1},
                {"tag": "F-PU2", "from": "PH", "to": "PU-2", "size": "#6", "conduits": 1, "conduit_in": 1},
                {"tag": "F-ELEV", "from": "MDP", "to": "DS-ELEV", "size": "#4", "conduits": 1, "conduit_in": 1.25}
            ],
            "panel_schedules": [
                {"panel": "PU-1", "source": "E-601", "circuits": [{"number": "1", "description": "RECEPTACLES", "room": "101"}, {"number": "5", "description": "LIGHTS", "room": "101"}]},
                {"panel": "PH", "source": "E-601", "circuits": [{"number": "2", "description": "CORRIDOR LIGHTS", "room": "203"}, {"number": "4", "description": "STAIR", "room": "103"}]}
            ]
        }
    })
}

/// The two-storey building, built.
fn built() -> Session {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Sample Building", None);
    // Windows: a 4' block across each gap in the outside wall.
    s.execute("layer.new", &json!({"name": "A-GLAZ", "current": false})).expect("layer");
    let h = s.execute("pline", &json!({"vertices": [[-24, -3], [24, -3], [24, 3], [-24, 3]], "closed": true})).unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_string();
    s.execute("block", &json!({"name": "WIN4", "base": [0, 0], "handles": [h], "keep": "delete"})).expect("block");
    s.execute("layer.current", &json!({"name": "A-GLAZ"})).expect("layer");
    for dx in [0.0, L2_DX] {
        for x in [10.0, 30.0] {
            s.execute("insert", &json!({"name": "WIN4", "at": [(dx + x) * 12.0, 3.0]})).expect("window");
        }
    }
    for dx in [0.0, L2_DX] {
        let lv = if dx == 0.0 { 1 } else { 2 };
        for (x, n, name) in [(10.0, 1, "GUEST"), (30.0, 2, "GUEST"), (50.0, 4, "ELEC"), (30.0, 3, "CORRIDOR")] {
            let y = if n == 3 { 25.0 } else { 10.0 };
            text_on(&mut s, "A-ROOM-TAG", [(dx + x) * 12.0, (y + 1.0) * 12.0], name);
            text_on(&mut s, "A-ROOM-TAG", [(dx + x) * 12.0, (y - 1.0) * 12.0], &format!("{lv}0{n}"));
        }
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec()})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    // What other builds make, by hand: a bed and a vanity cabinet in guest room 101, and a
    // sanitary pipe whose size no document gives.
    built_box(&mut s, "furniture", &["type:Bed"], [60.0, 36.0, 0.0], [132.0, 120.0, 24.0]);
    built_box(&mut s, "casework", &["type:Vanity"], [24.0, 200.0, 0.0], [72.0, 222.0, 34.0]);
    built_box(&mut s, "mep", &["system:SAN", "kind:pipe", "part:segment"], [300.0, 60.0, 96.0], [304.0, 200.0, 100.0]);
    s
}

/// A box (inches) built on L1 as a build tags its elements: part, level, source, then `tags`.
fn built_box(s: &mut Session, part: &str, tags: &[&str], lo: [f64; 3], hi: [f64; 3]) {
    let d = s.doc_mut().unwrap();
    let v = |x: usize, y: usize, z: usize| Vec3 { x: [lo[0], hi[0]][x], y: [lo[1], hi[1]][y], z: [lo[2], hi[2]][z] };
    let vertices = vec![v(0, 0, 0), v(1, 0, 0), v(1, 1, 0), v(0, 1, 0), v(0, 0, 1), v(1, 0, 1), v(1, 1, 1), v(0, 1, 1)];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([2, 3, 7, 6]), f([1, 2, 6, 5]), f([3, 0, 4, 7])];
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, ..Default::default() }));
    e.common.layer = "3D-Other".into();
    let mut x = vec![RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }];
    for t in [part, "L1", "0"].into_iter().chain(tags.iter().copied()) {
        x.push(RawTag { code: 1000, value: t.into() });
    }
    e.common.xdata = x.into();
    d.model.push(e);
}

// ─── Reading the set back ────────────────────────────────────────────────────

fn read_json(p: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

/// The build's tags of an element (after the build's marker).
fn tags(e: &Entity) -> Vec<String> {
    let x = &e.common.xdata;
    let Some(at) = x.iter().position(|t| t.code == 1001 && t.value == "CADADDY_WALLGEN") else { return Vec::new() };
    x[at + 1..].iter().take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.clone()).collect()
}

fn tag<'a>(t: &'a [String], key: &str) -> Option<&'a str> {
    t.iter().find_map(|v| v.strip_prefix(&format!("{key}:")))
}

/// The model's elements by what they are: name → handles.
fn census(s: &Session) -> BTreeMap<&'static str, BTreeSet<String>> {
    let d = s.doc().unwrap();
    let mut out: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
    for e in d.model.iter() {
        let t = tags(e);
        let Some(part) = t.first() else { continue };
        let h = e.handle.hex();
        let system = tag(&t, "system").unwrap_or_default();
        let kind = tag(&t, "kind").unwrap_or_default();
        let mpart = tag(&t, "part").unwrap_or_default();
        let mepid = tag(&t, "mepid").unwrap_or_default();
        let etag = tag(&t, "tag").unwrap_or_default();
        let mut put = |k: &'static str| {
            out.entry(k).or_default().insert(h.clone());
        };
        match part.as_str() {
            "wall" | "door" | "window" | "stair" => put("shell"),
            "floor" => put("floor"),
            "device" => put("device"),
            "furniture" => put("furniture"),
            "casework" => put("cabinet"),
            _ => {}
        }
        if part != "mep" {
            continue;
        }
        if mpart == "atom:box" || mpart == "atom:ring" {
            put("box");
        }
        if mpart.starts_with("equipment:") {
            match etag {
                "MDP" => put("MDP"),
                "PH" => put("PH"),
                "PU-1" | "PU-2" => put("unit_panel"),
                "ELEV-1" => put("hoistway"),
                "DS-ELEV" => put("elevator_disconnect"),
                "EF-1" => put("bath_fan"),
                "AHU-1" => put("ahu"),
                "FPMP-1" => put("fire_pump"),
                _ => {}
            }
        }
        if mpart == "atom:conduit" {
            match system {
                "F-ELEV" => put("elevator_feeder"),
                "F-PH" => put("house_feeder"),
                "F-PU1" | "F-PU2" => put("unit_feeder"),
                "PWR" if mepid.starts_with("@690.00") => put("elevator_branch"),
                "PWR" if mepid.starts_with("@500.00") => put("branch_through_floor"),
                "PWR" if mepid.starts_with("@400.00") => put("unknown_1_25"),
                "PWR" => put("branch"),
                "E-FEEDER" => put("feeder_by_name"),
                "DATA" => put("low_voltage"),
                _ => {}
            }
        }
        match (kind, system, mpart) {
            ("duct", "SA", m) if !m.starts_with("terminal") && tag(&t, "size") == Some("24x12") => put("main_duct"),
            ("duct", "SA", "segment") => put("branch_duct"),
            ("duct", "SA", m) if m.starts_with("terminal") => put("diffuser"),
            ("duct", "EA", _) => put("bath_fan_duct"),
            ("pipe", "FP", m) if m.starts_with("terminal") => put("sprinkler_head"),
            ("pipe", "FP", _) if mepid.starts_with("@24.00") => put("fire_main"),
            ("pipe", "FP", _) if mepid.starts_with("@30.00") => put("fire_riser"),
            ("pipe", "FP", _) if mepid.starts_with("@240.00") => put("sprinkler_branch"),
            ("pipe", "CW", _) if mepid.starts_with("@24.00") => put("cw_main"),
            ("pipe", "CW", _) if mepid.starts_with("@360.00") => put("cw_branch"),
            ("pipe", "CW", _) if mepid.starts_with("@372.00") => put("cw_riser"),
            ("pipe", "SAN", _) => put("unsized_pipe"),
            _ => {}
        }
    }
    out
}

/// A package's element ids (from its sidecar) and its nodes' metadata.
fn package(dir: &Path, name: &str) -> (BTreeSet<String>, Vec<Value>, Value) {
    let side = read_json(&dir.join(format!("{name}.json")));
    let els = side["elements"].as_array().unwrap().clone();
    let ids = els.iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect();
    (ids, els, side)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn the_set_follows_the_four_views_and_never_holds_everything() {
    let _g = serial();
    let mut s = built();
    let c = census(&s);
    for k in [
        "shell",
        "floor",
        "device",
        "furniture",
        "cabinet",
        "box",
        "MDP",
        "PH",
        "unit_panel",
        "hoistway",
        "elevator_disconnect",
        "bath_fan",
        "ahu",
        "fire_pump",
        "elevator_feeder",
        "house_feeder",
        "unit_feeder",
        "elevator_branch",
        "branch_through_floor",
        "unknown_1_25",
        "branch",
        "main_duct",
        "branch_duct",
        "diffuser",
        "bath_fan_duct",
        "sprinkler_head",
        "fire_main",
        "fire_riser",
        "sprinkler_branch",
        "cw_main",
        "cw_branch",
        "cw_riser",
        "unsized_pipe",
        "feeder_by_name",
        "low_voltage",
    ] {
        assert!(c.get(k).is_some_and(|v| !v.is_empty()), "the synthetic building has {k}: {:?}", c.keys().collect::<Vec<_>>());
    }
    let d = dir("views");
    let r = s.execute("bim.export.ar", &json!({"set": true, "dir": d.to_string_lossy()})).unwrap_or_else(|e| panic!("set: {e}"));
    let index = read_json(&d.join("building.json"));
    assert_eq!(index["schema"], json!("cadaddy.ar.building/1"));

    // Triangles and bytes per package (the DONE report's numbers).
    for p in r["packages"].as_array().unwrap() {
        println!(
            "PACKAGE {} view={} nodes={} triangles={} glb={} usdz={}",
            p["name"], p["view"], p["nodes"], p["triangles"], p["bytes"]["glb"], p["bytes"]["usdz"]
        );
    }
    println!("ROOMS {}", r["rooms"]);
    for p in index["packages"].as_array().unwrap() {
        if p["generic"] == json!(true) {
            assert!(p["triangles"].as_u64().unwrap() <= 150_000, "{p}");
        }
    }

    let (arch, arch_els, arch_side) = package(&d, "building-architectural");
    let (mains, mains_els, mains_side) = package(&d, "building-mep-mains");
    let (full, full_els, _) = package(&d, "building-mep-full");
    let (l1, l1_els, _) = package(&d, "levels/L1");
    let (l2, _, _) = package(&d, "levels/L2");
    let all = |k: &str| c.get(k).cloned().unwrap_or_default();
    let has_all = |pkg: &BTreeSet<String>, k: &str, name: &str| {
        let want = all(k);
        let miss: Vec<&String> = want.iter().filter(|h| !pkg.contains(*h)).collect();
        assert!(miss.is_empty(), "{name} holds every {k}: missing {miss:?}");
    };
    let has_none = |pkg: &BTreeSet<String>, k: &str, name: &str| {
        let want = all(k);
        let there: Vec<&String> = want.iter().filter(|h| pkg.contains(*h)).collect();
        assert!(there.is_empty(), "{name} holds no {k}: {there:?}");
    };

    // Mains: gear, house panels and their feeders, main duct, large pipe, fire mains, risers and
    // pumps, large equipment, and each elevator's hoistway, disconnect and feeder.
    for k in [
        "MDP",
        "PH",
        "house_feeder",
        "hoistway",
        "elevator_disconnect",
        "elevator_feeder",
        "ahu",
        "fire_pump",
        "main_duct",
        "cw_main",
        "fire_main",
        "fire_riser",
    ] {
        has_all(&mains, k, "building-mep-mains");
    }
    // Full on top: every other duct, terminal and the bathroom fan with its duct, every panel
    // with its feeder, anything through a floor, a run of unknown connection by its size.
    for k in [
        "unit_panel",
        "unit_feeder",
        "branch_duct",
        "diffuser",
        "bath_fan",
        "bath_fan_duct",
        "cw_riser",
        "branch_through_floor",
        "unknown_1_25",
        "feeder_by_name",
        "unsized_pipe",
    ] {
        has_all(&full, k, "building-mep-full");
    }
    let mep: BTreeSet<String> = mains.union(&full).cloned().collect();
    // Never in a building MEP package: the shell, wires' boxes, devices, fixtures, furniture,
    // branch circuits (the elevator's even through a floor), small pipe, sprinkler heads and
    // branch lines that stay on their floor.
    for k in [
        "shell",
        "device",
        "box",
        "furniture",
        "cabinet",
        "branch",
        "elevator_branch",
        "cw_branch",
        "sprinkler_head",
        "sprinkler_branch",
        "low_voltage",
    ] {
        has_none(&mep, k, "the building MEP packages");
    }
    // Their only context: the floors and one outline.
    let context: Vec<&Value> = mains_els.iter().filter(|e| e["view_basis"].as_str().is_none_or(|b| b.starts_with("context"))).collect();
    for e in &context {
        let id = e["id"].as_str().unwrap_or_default();
        assert!(all("floor").contains(id) || e["kind"] == json!("outline"), "mains context is floors and the outline: {e}");
    }
    assert_eq!(mains_els.iter().filter(|e| e["kind"] == json!("outline")).count(), 1, "one outline");
    has_all(&mains, "floor", "building-mep-mains");
    assert!(full_els.iter().all(|e| e["view_basis"].is_string()), "every full element says why it is there");
    // A pipe with neither a role nor a size is not dropped (Q6431): role decides first, size only
    // when the role is unknown, and with neither it shows in Full MEP marked inferred, counted.
    assert!(mains_side["left_out"]["pipe size unknown"].is_null(), "{}", mains_side["left_out"]);
    assert_eq!(index["left_out"]["pipe_size_unknown"], json!(0));
    assert_eq!(index["shown_inferred"]["no_role_no_size"], json!(1), "{}", index["shown_inferred"]);
    let no_size: Vec<&Value> = full_els.iter().filter(|e| all("unsized_pipe").contains(e["id"].as_str().unwrap_or_default())).collect();
    assert_eq!(no_size.len(), 1, "the unsized pipe is in Full MEP");
    assert!(no_size[0]["inferred"].as_str().is_some_and(|w| w.contains("no role and no size")), "{}", no_size[0]);
    // The hoistway is one simple volume.
    let hoist = mains_els.iter().find(|e| all("hoistway").contains(e["id"].as_str().unwrap())).unwrap();
    assert_eq!(hoist["simplified"], json!("box"), "{hoist}");

    // Architectural: the shell and floors, no MEP, framing, cabinets or furniture.
    has_all(&arch, "shell", "building-architectural");
    has_all(&arch, "floor", "building-architectural");
    for e in &arch_els {
        let part = e["part"].as_str().unwrap_or_default();
        assert!(!matches!(part, "mep" | "device" | "atom" | "furniture" | "casework" | "column" | "beam"), "architectural holds {e}");
    }
    assert!(arch_els.iter().any(|e| e["role"] == json!("exterior")), "the solid exterior for the reveal");
    assert_eq!(arch_els.iter().filter(|e| e["role"] == json!("room_tile")).count(), 8, "a tile per room");

    // Floor: L1 has its sprinkler heads and branch lines, its panels and lights, a tile per room
    // with number and name, walls cut low; no furniture; nothing of L2.
    for k in ["sprinkler_head", "sprinkler_branch", "unit_panel", "PH", "cabinet"] {
        has_all(&l1, k, "levels/L1");
    }
    has_none(&l1, "furniture", "levels/L1");
    has_none(&l2, "furniture", "levels/L2");
    let tiles: Vec<&Value> = l1_els.iter().filter(|e| e["role"] == json!("room_tile")).collect();
    assert_eq!(tiles.len(), 4, "{tiles:?}");
    assert!(tiles.iter().any(|t| t["number"] == json!("101") && t["name"] == json!("GUEST")), "{tiles:?}");
    assert!(l1_els.iter().all(|e| e["level"] == json!("L1") || e["role"] == json!("room_tile")), "only L1");
    let wall = l1_els.iter().find(|e| e["part"] == json!("wall")).unwrap();
    assert!(wall["top_aff_in"].as_f64().unwrap() > 90.0, "the element keeps its own height: {wall}");

    // Furniture only in room packages, marked off; cabinets in level and room packages only.
    let graph = index["graph"]["elements"].as_array().unwrap();
    let pkgs_of = |h: &str| -> Vec<String> {
        graph
            .iter()
            .find(|g| g["id"] == json!(h))
            .map(|g| g["packages"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    };
    for h in all("furniture") {
        let p = pkgs_of(&h);
        assert!(!p.is_empty() && p.iter().all(|x| x.starts_with("rooms/")), "furniture only in rooms: {p:?}");
        let (_, els, _) = package(&d, &p[0]);
        let n = els.iter().find(|e| e["id"] == json!(h)).unwrap();
        assert_eq!(n["group"], json!("furniture"));
        assert_eq!(n["default_on"], json!(false), "{n}");
    }
    for h in all("cabinet") {
        let p = pkgs_of(&h);
        assert!(p.iter().any(|x| x.starts_with("levels/")) && p.iter().any(|x| x.starts_with("rooms/")), "{p:?}");
        assert!(p.iter().all(|x| !x.starts_with("building-")), "{p:?}");
    }

    // No package holds every object; every element is in some level or room package; the graph
    // covers every element the export keeps.
    let every: BTreeSet<String> = graph.iter().map(|g| g["id"].as_str().unwrap().to_string()).collect();
    let mut names: Vec<String> = index["packages"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect();
    names.extend(index["room_packages"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()));
    assert!(names.len() >= 3 + 2 + 8, "{names:?}");
    for n in &names {
        let (ids, els, side) = package(&d, n);
        assert!(ids.len() < every.len(), "{n} holds {} of {} objects", ids.len(), every.len());
        for e in &els {
            assert!(
                ["base", "mechanical", "electrical", "plumbing", "cabinets", "furniture"].contains(&e["group"].as_str().unwrap_or_default()),
                "{n}: {e}"
            );
        }
        if n.starts_with("building-") {
            assert_eq!(side["generic"], json!(true), "{n}");
            assert_eq!(side["field_alignment"], json!(false), "{n}");
        } else if n.starts_with("rooms/") {
            assert_eq!(side["field_alignment"], json!(true), "{n}");
            assert_eq!(side["anchor"]["kind"], json!("room_corner"), "{n}");
        }
    }
    for g in graph {
        let p = g["packages"].as_array().unwrap();
        assert!(p.iter().any(|x| x.as_str().is_some_and(|x| x.starts_with("levels/") || x.starts_with("rooms/"))), "{g}");
    }
    let model = s.doc().unwrap();
    let meshes = model.model.iter().filter(|e| matches!(e.kind, EntityKind::Mesh(_)) && !tags(e).is_empty()).count();
    let small: u64 = index["left_out"]["small_parts"].as_object().unwrap().values().map(|v| v.as_u64().unwrap()).sum();
    assert_eq!(every.len() as u64 + small, meshes as u64, "every built element is in the graph or counted as a loose piece");

    // The connection graph by element id, known links only from the documents.
    let by_tag = |t: &str| graph.iter().find(|g| g["tag"] == json!(t) && g["role"] == json!("panel")).unwrap_or_else(|| panic!("{t}"));
    let pu1 = by_tag("PU-1");
    assert_eq!(pu1["fed_by"][0]["tag"], json!("PH"), "{pu1}");
    assert_eq!(pu1["fed_by"][0]["via"], json!("F-PU1"));
    assert_eq!(pu1["fed_by"][0]["link"], json!("known"));
    assert_eq!(pu1["fed_by"][0]["id"], json!(all("PH").into_iter().next().unwrap()));
    assert_eq!(pu1["panel_scope"]["scope"], json!("unit"), "{pu1}");
    assert_eq!(by_tag("PH")["panel_scope"]["scope"], json!("house"));
    let feeder = graph.iter().find(|g| all("elevator_feeder").contains(g["id"].as_str().unwrap())).unwrap();
    assert_eq!((feeder["fed_by"][0]["tag"].clone(), feeder["serves"][0]["tag"].clone()), (json!("MDP"), json!("DS-ELEV")), "{feeder}");
    let devices: Vec<&Value> = graph.iter().filter(|g| all("device").contains(g["id"].as_str().unwrap())).collect();
    assert!(
        devices.iter().any(|g| g["fed_by"][0]["tag"] == json!("PU-1") && g["fed_by"][0]["circuit"] == json!("5") && g["link"] == json!("known")),
        "{devices:?}"
    );
    assert!(
        devices.iter().any(|g| g["link"] == json!("unknown") && g["fed_by"][0]["circuit"] == json!("7")),
        "a circuit with no panel stays unknown: {devices:?}"
    );
    let diffuser = graph.iter().find(|g| all("diffuser").contains(g["id"].as_str().unwrap())).unwrap();
    assert_eq!(diffuser["fed_by"][0]["tag"], json!("AHU-1"), "{diffuser}");

    // The index: levels, rooms, equipment, systems, the four views and the default.
    assert_eq!(index["views"]["default"], json!({"view": "mep", "density": "mains"}));
    assert_eq!(index["views"]["mep"]["densities"]["full"], json!(["building-mep-mains", "building-mep-full"]));
    assert_eq!(index["views"]["floor"]["by_level"]["L2"], json!(["levels/L2"]));
    assert_eq!(index["groups"]["furniture"]["default_on"], json!(false));
    let levels = index["levels"].as_array().unwrap();
    assert_eq!(levels.len(), 2);
    assert_eq!(levels[1]["elevation_ft"], json!(10.0));
    assert!(!levels[0]["outline"].as_array().unwrap().is_empty(), "{}", levels[0]);
    let rooms = index["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 8);
    let r101 = rooms.iter().find(|r| r["number"] == json!("101")).unwrap();
    assert!(r101["package"].as_str().unwrap().starts_with("rooms/"), "{r101}");
    assert!(r101["anchor"]["plan_x"].is_number() && r101["bounds"]["max"].is_array(), "{r101}");
    assert!(index["equipment"].as_array().unwrap().iter().any(|e| e["tag"] == json!("ELEV-1") && e["view"] == json!("mains")));
    assert!(index["systems"]["mechanical"].as_array().unwrap().iter().any(|s| s["name"] == json!("SA")), "{}", index["systems"]);
    for p in index["packages"].as_array().unwrap() {
        assert_eq!(p["files"]["glb"]["sha256"].as_str().unwrap().len(), 64, "{p}");
        assert_eq!(p["content_hash"].as_str().unwrap().len(), 64);
    }
    assert!(d.join("manifest.json").exists(), "the folder's manifest for the app's Update");
    let _ = arch_side;

    // Again into the same folder: its own set, replaced; unchanged packages keep their content hash.
    s.execute("bim.export.ar", &json!({"set": true, "dir": d.to_string_lossy()})).unwrap_or_else(|e| panic!("again: {e}"));
    let again = read_json(&d.join("building.json"));
    let hashes = |v: &Value| -> Vec<(Value, Value)> {
        v["packages"].as_array().unwrap().iter().map(|p| (p["name"].clone(), p["content_hash"].clone())).collect()
    };
    assert_eq!(hashes(&index), hashes(&again), "unchanged packages hash the same");
}

#[test]
fn a_building_wide_request_for_everything_is_refused() {
    let _g = serial();
    let mut s = built();
    let d = dir("refuse");
    let e = s.execute("bim.export.ar", &json!({"level": "all", "path": d.join("x").to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("never puts every object") && e.contains("set: true"), "{e}");
    // Every room of both levels: every object at once, so refused too.
    let every: Vec<String> = ["101", "102", "103", "104", "201", "202", "203", "204"].iter().map(|x| x.to_string()).collect();
    let e = s.execute("bim.export.ar", &json!({"rooms": every, "path": d.join("y").to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("never puts every object"), "{e}");
    // One level is the Floor view: allowed.
    s.execute("bim.export.ar", &json!({"level": "L1", "path": d.join("z").to_string_lossy()})).expect("one level");
    for (p, want) in [
        (json!({"set": true}), "needs `dir`"),
        (json!({"dir": d.to_string_lossy()}), "add set: true"),
        (json!({"set": true, "dir": d.to_string_lossy(), "room": "101"}), "`room` is for one export"),
    ] {
        let e = s.execute("bim.export.ar", &p).unwrap_err().to_string();
        assert!(e.contains(want), "{p}: {e}");
    }
    // A folder with someone else's building.json is not the set's to replace.
    let f = dir("foreign");
    std::fs::write(f.join("building.json"), "{\"mine\": true}").unwrap();
    let e = s.execute("bim.export.ar", &json!({"set": true, "dir": f.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("force"), "{e}");
    s.execute("bim.export.ar", &json!({"set": true, "dir": f.to_string_lossy(), "force": true})).expect("with force");
}

/// The field app's test fixture (apps/ios/CADaddyField/CADaddyFieldTests/Fixtures/SyntheticSet)
/// comes from this exporter, never by hand: this building's set, with one receptacle whose panel
/// a rule gave (so the graph has known, inferred and unknown links). With
/// `CADADDY_FIELD_FIXTURE=<dir>` it writes the set's models, sidecars and index there
/// (`apps/ios/CADaddyField/CADaddyFieldTests/Fixtures/regenerate.sh`).
#[test]
fn the_field_apps_fixture_comes_from_this_exporter() {
    use cadaddy_doc::props::{self, Prop, Source, SourceKind};
    let _g = serial();
    let mut s = built();
    {
        let d = s.doc_mut().unwrap();
        let mut hit = Vec::new();
        for e in d.model.iter() {
            if tags(e).first().map(String::as_str) != Some("device") {
                continue;
            }
            let store = props::read(&e.common.xdata);
            if store.get("circuit").map(Prop::text).as_deref() == Some("7") && store.get("panel").is_none_or(|p| !p.has_value()) {
                hit.push(e.handle);
            }
        }
        assert_eq!(hit.len(), 1, "one receptacle on circuit 7 with no panel");
        for h in hit {
            d.model.modify(h, |e| {
                let mut store = props::read(&e.common.xdata);
                store.set("panel", Prop::new("PU-2", Source::of(SourceKind::Rule)));
                e.common.xdata = props::write(&e.common.xdata, &store).into();
            });
        }
    }
    let d = dir("fixture");
    s.execute("bim.export.ar", &json!({"set": true, "dir": d.to_string_lossy()})).unwrap_or_else(|e| panic!("set: {e}"));
    let index = read_json(&d.join("building.json"));
    let links = &index["graph"]["links"];
    for k in ["known", "inferred", "unknown"] {
        assert!(links[k].as_u64().is_some_and(|n| n > 0), "the fixture has {k} links: {links}");
    }
    // The next export after one change in one room (the bed of 101 taken out): only that room's
    // package changes its content hash, so Update pulls that package and nothing else.
    {
        let doc = s.doc_mut().unwrap();
        let bed: Vec<Handle> =
            doc.model.iter().filter(|e| tags(e).first().map(String::as_str) == Some("furniture")).map(|e| e.handle).collect();
        assert_eq!(bed.len(), 1, "one bed");
        for h in bed {
            doc.model.remove(h);
        }
    }
    let d2 = dir("fixture-v2");
    s.execute("bim.export.ar", &json!({"set": true, "dir": d2.to_string_lossy()})).unwrap_or_else(|e| panic!("set v2: {e}"));
    let index2 = read_json(&d2.join("building.json"));
    let hashes = |v: &Value| -> BTreeMap<String, Value> {
        v["packages"]
            .as_array()
            .into_iter()
            .chain(v["room_packages"].as_array())
            .flatten()
            .map(|p| (p["name"].as_str().unwrap_or_default().to_string(), p["content_hash"].clone()))
            .collect()
    };
    let (h1, h2) = (hashes(&index), hashes(&index2));
    let changed: Vec<&String> = h2.iter().filter(|(k, v)| h1.get(*k) != Some(*v)).map(|(k, _)| k).collect();
    assert_eq!(changed, vec!["rooms/L1-101-GUEST"], "only the changed room's package changes");
    let Ok(out) = std::env::var("CADADDY_FIELD_FIXTURE") else { return };
    let out = PathBuf::from(out);
    // The whole first export, then the second one's index with just its changed packages.
    let copy = |from: &Path, to: &Path, idx: &Value, only: Option<&[&String]>| {
        let _ = std::fs::remove_dir_all(to);
        let mut files = vec![PathBuf::from("building.json"), PathBuf::from("manifest.json")];
        for p in idx["packages"].as_array().into_iter().chain(idx["room_packages"].as_array()).flatten() {
            if only.is_some_and(|o| !o.iter().any(|n| p["name"].as_str() == Some(n.as_str()))) {
                continue;
            }
            for k in ["glb", "sidecar"] {
                if let Some(f) = p["files"][k]["path"].as_str() {
                    files.push(PathBuf::from(f));
                }
            }
        }
        for f in files {
            let t = to.join(&f);
            std::fs::create_dir_all(t.parent().unwrap()).unwrap();
            std::fs::copy(from.join(&f), &t).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        }
    };
    copy(&d, &out, &index, None);
    copy(&d2, &out.with_file_name(format!("{}V2", out.file_name().unwrap().to_string_lossy())), &index2, Some(&changed));
}
