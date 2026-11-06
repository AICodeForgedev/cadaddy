//! Validate finds the job's walls (Q6127): host and nearest-wall lookups and the packets read the
//! built WALL ELEMENTS (type, thickness, centreline, both faces), never the plan's raw faces —
//! on a model the PDF→BIM job built from a made-up print set, on a drawing whose print set
//! holds more objects than a scan cap once let through (the build comes after them), and on
//! walls Walk mode built (faces standing on the floor, a cap over each pair, no bottoms).

use std::path::PathBuf;
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, HAlign, Insert, Line, Mesh, MeshFace, Point, Text, VAlign};
use cadaddy_engine::geom::Vec3;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

fn real_pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

fn dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("cadaddy-vwalls-{}-{name}-{nanos}", std::process::id()))
}

fn nums(v: &Value) -> Vec<f64> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default()
}

fn read_json(p: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

/// The packet of an area: its manifest and objects.
fn packet(s: &mut Session, area: &str) -> (Value, Value) {
    let out = dir("packet");
    let m = s.execute("validate.packet", &json!({"area": area, "out_dir": out.to_string_lossy()})).unwrap();
    let o = read_json(&out.join("objects.json"));
    let _ = std::fs::remove_dir_all(&out);
    (m, o)
}

/// A wall the packet lists is a built wall: its handle a wall element, thick, typed when the
/// element is, with its two face lines `thickness` apart, each listed once.
fn assert_walls_are_elements(s: &Session, objects: &Value) {
    let d = s.doc().unwrap();
    let walls = objects["walls"].as_array().unwrap();
    assert!(!walls.is_empty(), "{objects:#}");
    let mut seen = std::collections::BTreeSet::new();
    for w in walls {
        let h = w["handle"].as_str().unwrap();
        assert!(!h.starts_with("face-"), "a plan face listed as a wall: {w}");
        let e = cadaddy_engine::doc::Handle::parse_hex(h).and_then(|h| d.model.get(h)).unwrap_or_else(|| panic!("no object {h}: {w}"));
        assert_eq!(made_as(e).as_deref(), Some("wall"), "{w}");
        let t = w["thickness_in"].as_f64().unwrap();
        assert!(t > 1.0, "a wall with its thickness: {w}");
        let faces = w["faces"].as_array().unwrap();
        assert_eq!(faces.len(), 2, "both faces: {w}");
        let (f0, f1) = (nums(&faces[0]["a_ft"]), nums(&faces[1]["a_ft"]));
        assert!(((f0[0] - f1[0]).hypot(f0[1] - f1[1]) * 12.0 - t).abs() < 0.25, "the faces are the thickness apart: {w}");
        assert_ne!(faces[0]["face"], faces[1]["face"], "{w}");
        let key = (h.to_string(), w["a_ft"].to_string(), w["b_ft"].to_string());
        assert!(seen.insert(key), "a wall listed twice: {w}");
    }
}

/// A packet's walls all come within `margin` feet of the area's box.
fn assert_walls_within(objects: &Value, b: &[f64], margin: f64) {
    for w in objects["walls"].as_array().unwrap() {
        let (a, z) = (nums(&w["a_ft"]), nums(&w["b_ft"]));
        let near = (0..=100).any(|i| {
            let t = f64::from(i) / 100.0;
            let (x, y) = (a[0] + (z[0] - a[0]) * t, a[1] + (z[1] - a[1]) * t);
            x >= b[0] - margin && x <= b[2] + margin && y >= b[1] - margin && y <= b[3] + margin
        });
        assert!(near, "a wall far from the area {b:?}: {w}");
    }
}

// ---------------------------------------------------------------- a job-built model

/// The PDF→BIM job on a made-up print set: a floor plan and its power plan.
fn job() -> Session {
    real_pdf_reader();
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| ["A-101", "E-201"].contains(&s.number.as_str()));
    let folder = dir("job");
    std::fs::create_dir_all(&folder).unwrap();
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(folder.join("Sample Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).unwrap();
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": folder.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    let _ = std::fs::remove_dir_all(&folder);
    s
}

/// Drawing units per foot of the building spec.
fn upf(s: &mut Session) -> f64 {
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    sp["spec"]["units_per_foot"].as_f64().or_else(|| sp["units_per_foot"].as_f64()).filter(|u| *u > 0.0).unwrap_or(12.0)
}

/// The built walls' long faces in plan (sheet units), measured off the meshes themselves: the
/// long sides of each piece's bottom on the floor.
fn wall_faces(s: &Session) -> Vec<([f64; 2], [f64; 2])> {
    let d = s.doc().unwrap();
    let mut out = Vec::new();
    for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some("wall")) {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let z0 = m.vertices.iter().fold(f64::MAX, |a, v| a.min(v.z));
        for f in &m.faces {
            let vs: Vec<Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).copied()).collect();
            if vs.len() != 4 || vs.iter().any(|v| (v.z - z0).abs() > 1e-6) {
                continue;
            }
            let p = |i: usize| [vs[i % 4].x, vs[i % 4].y];
            let len = |i: usize| (p(i)[0] - p(i + 1)[0]).hypot(p(i)[1] - p(i + 1)[1]);
            let long = if len(0) >= len(1) { [0, 2] } else { [1, 3] };
            out.extend(long.map(|i| (p(i), p(i + 1))));
        }
    }
    out
}

/// How far `p` stands in front of (or behind) the face `a`–`b`, when it is in front of it.
fn off_face(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2;
    (l2 > 1e-12 && (0.0..=1.0).contains(&t)).then(|| ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / l2.sqrt())
}

/// The built devices: handle and plan centre (sheet units).
fn devices(s: &Session) -> Vec<(String, [f64; 2])> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("device"))
        .map(|e| {
            let c = cadaddy_engine::doc::entity_bounds(d, e, 0).center();
            (e.handle.hex(), [c.x, c.y])
        })
        .collect()
}

#[test]
fn a_job_built_models_devices_are_hosted_by_its_wall_elements() {
    let mut s = job();
    let k = upf(&mut s);
    let faces = wall_faces(&s);
    assert!(!faces.is_empty(), "the job built walls");
    let mut snapped = 0;
    let mut moved_one = false;
    for (h, c) in devices(&s) {
        let r = s.execute("bim.element", &json!({"handle": h})).unwrap();
        if r["at_ceiling"] == json!(true) {
            continue;
        }
        // Its distance from the nearest wall face, measured off the meshes (inches).
        let Some((off, face)) = faces.iter().filter_map(|(a, b)| off_face(c, *a, *b).map(|o| (o, (*a, *b)))).min_by(|x, y| x.0.total_cmp(&y.0))
        else {
            continue;
        };
        let off_in = off / k * 12.0;
        if off_in > 3.0 {
            continue;
        }
        let square = (face.0[0] - face.1[0]).abs() < 1e-6 || (face.0[1] - face.1[1]).abs() < 1e-6;
        snapped += 1;
        let host = &r["host"];
        assert!(host.is_object(), "a device {off_in:.2}\" off a wall face is hosted: {r:#}");
        assert!(!host["wall"].as_str().unwrap_or("face-").starts_with("face-"), "{r:#}");
        assert!(host["thickness_in"].as_f64().unwrap_or(0.0) > 1.0, "hosted by a wall with its thickness: {r:#}");
        if host["from"] == "stored" {
            // Its stored host wall comes first (Q6139), within G-HOST's tolerance; at a corner
            // that may be the other wall.
            assert!(host["off_face_in"].as_f64().unwrap().abs() <= 6.0, "on its stored host wall: {r:#}");
        } else {
            assert!(host["off_face_in"].as_f64().unwrap().abs() <= off_in + 0.5, "hosted by the face the mesh has it {off_in:.2}\" off: {r:#}");
            assert_eq!(r["nearest_wall"]["wall"], host["wall"], "{r:#}");
        }
        // Two feet out from its face: no host, and the wall 24" away.
        if !moved_one && square {
            let dir = match host["face"].as_str().unwrap() {
                "north" => [0.0, 1.0],
                "south" => [0.0, -1.0],
                "east" => [1.0, 0.0],
                _ => [-1.0, 0.0],
            };
            let was = host["off_face_in"].as_f64().unwrap();
            let by = (24.0 - was) / 12.0 * k;
            s.execute("move", &json!({"handles": [h], "delta": [dir[0] * by, dir[1] * by]})).unwrap();
            let r2 = s.execute("bim.element", &json!({"handle": h})).unwrap();
            if r2["nearest_wall"]["wall"] == host["wall"] {
                assert!(r2["host"].is_null(), "2 ft off its wall a device has no host: {r2:#}");
                let off = r2["nearest_wall"]["off_face_in"].as_f64().unwrap();
                assert!((off - 24.0).abs() < 0.5, "2 ft off its wall's face: {r2:#}");
                moved_one = true;
            }
            s.execute("undo", &json!({})).unwrap();
        }
    }
    assert!(snapped >= 5, "the job snapped devices to walls: {snapped}");
    assert!(moved_one, "a device moved 2 ft off its wall was measured");
}

#[test]
fn a_job_built_rooms_packet_lists_its_own_walls_once_each() {
    let mut s = job();
    let areas = s.execute("validate.areas", &json!({"limit": 100})).unwrap()["areas"].as_array().cloned().unwrap();
    let rooms: Vec<&Value> = areas.iter().filter(|a| a["kind"] == "room").collect();
    assert!(rooms.len() >= 3, "{areas:?}");
    let mut listed: Vec<std::collections::BTreeSet<String>> = Vec::new();
    for a in rooms.iter().take(4) {
        let (_, o) = packet(&mut s, a["id"].as_str().unwrap());
        assert_walls_are_elements(&s, &o);
        assert_walls_within(&o, &nums(&a["box"]), 1.5);
        listed.push(o["walls"].as_array().unwrap().iter().filter_map(|w| w["handle"].as_str().map(str::to_string)).collect());
    }
    // Rooms apart hold different walls (a room's packet is not the level's walls).
    let all: std::collections::BTreeSet<&String> = listed.iter().flatten().collect();
    assert!(listed.iter().all(|l| l.len() < all.len()), "{listed:?}");
}

// ---------------------------------------------------------------- built after a big print set

const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;

fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

/// Where the receptacle is (feet): on the office's south wall's inside face.
const RECEP: [f64; 2] = [6.0, 0.5];

/// A plan of one level (a 40' × 12' shell of 6" walls, a partition at 15') with a receptacle
/// on the office's south wall, `junk` objects of a print set drawn before it, built.
fn built(junk: usize) -> Session {
    let mut d = cadaddy_engine::doc::Drawing::new_imperial();
    let add = |d: &mut cadaddy_engine::doc::Drawing, layer: &str, k: EntityKind| {
        let h = d.new_handle();
        let mut e = Entity::new(h, k);
        e.common.layer = layer.into();
        d.model.push(e);
    };
    // The print set: points of another sheet, far off the plan.
    for i in 0..junk {
        let p = Vec3::new(-50_000.0 - (i % 1000) as f64, (i / 1000) as f64, 0.0);
        add(&mut d, "PDF-SHEET", EntityKind::Point(Point { p, angle: 0.0 }));
    }
    let l = |a: [f64; 2], b: [f64; 2]| {
        let (a, b) = (at(a), at(b));
        EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) })
    };
    for (a, b) in [([0.0, 0.0], [40.0, 0.0]), ([40.0, 0.0], [40.0, 12.0]), ([40.0, 12.0], [0.0, 12.0]), ([0.0, 12.0], [0.0, 0.0])] {
        add(&mut d, "A-WALL", l(a, b));
    }
    for (a, b) in [([0.5, 0.5], [39.5, 0.5]), ([39.5, 0.5], [39.5, 11.5]), ([39.5, 11.5], [0.5, 11.5]), ([0.5, 11.5], [0.5, 0.5])] {
        add(&mut d, "A-WALL", l(a, b));
    }
    add(&mut d, "A-WALL", l([15.0, 0.5], [15.0, 11.5]));
    add(&mut d, "A-WALL", l([15.5, 0.5], [15.5, 11.5]));
    for (t, q) in [("OFFICE", [5.0, 6.0]), ("STORAGE", [25.0, 6.0])] {
        let q = at(q);
        let text = Text {
            insert: Vec3::new(q[0], q[1], 0.0),
            align_pt: None,
            height: 6.0,
            value: t.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::default(),
            valign: VAlign::default(),
        };
        add(&mut d, "A-ROOM-TAG", EntityKind::Text(text));
    }
    let mut b = Block::new("RECEP");
    b.entities.push(Entity::new(d.new_handle(), EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
    d.blocks.insert("RECEP".into(), Arc::new(b));
    let p = at(RECEP);
    let style = Text {
        insert: Vec3::new(p[0], p[1], 0.0),
        align_pt: None,
        height: 6.0,
        value: "receptacle".into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::default(),
        valign: VAlign::default(),
    };
    let attribs = vec![Attrib {
        tag: "TYPE".into(),
        text: style,
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
    add(&mut d, "E-POWR", EntityKind::Insert(ins));
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [0, 0, SW, SH]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}, {"layer": "E-POWR", "role": "device"}],
        "devices": {"by_type": {"receptacle": {"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]}}},
        "wall_types": [{"name": "Exterior", "thickness_in": 6}],
        "rooms": [{"name": "Office", "number": "101", "level": "L1"}, {"name": "Storage", "number": "102", "level": "L1"}],
        "tags": [{"id": "device:L1:recep", "kind": "device", "level": "L1", "at_ft": [p[0] / 12.0, p[1] / 12.0], "fields": {
            "type": {"value": "A", "source": "plan"}, "device_type": {"value": "duplex receptacle", "source": "plan"},
            "sheet": {"value": "A-101", "source": "plan"}, "symbol": {"value": "1F0", "source": "plan"}}}],
        "sheets": [{"number": "A-101", "title": "FLOOR PLAN", "discipline": "A", "level": "L1", "source": "model", "rect": [0, 0, SW, SH]}]
    });
    s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// The built receptacle.
fn receptacle(s: &Session) -> String {
    let d = s.doc().unwrap();
    let hs: Vec<String> = d.model.iter().filter(|e| made_as(e).as_deref() == Some("device")).map(|e| e.handle.hex()).collect();
    assert_eq!(hs.len(), 1, "{hs:?}");
    hs[0].clone()
}

/// The receptacle is on the office's south wall, and the office's packet lists its walls.
fn assert_hosted_and_listed(s: &mut Session) {
    let r = s.execute("bim.element", &json!({"handle": receptacle(s)})).unwrap();
    let host = &r["host"];
    assert!(host.is_object(), "{r:#}");
    assert_eq!(host["face"], "north", "{r:#}");
    assert!((host["thickness_in"].as_f64().unwrap() - 6.0).abs() < 0.1, "{r:#}");
    assert!(host["off_face_in"].as_f64().unwrap().abs() < 3.0, "{r:#}");
    let areas = s.execute("validate.areas", &json!({})).unwrap()["areas"].as_array().cloned().unwrap();
    let office = areas.iter().find(|a| a["number"] == "101").unwrap_or_else(|| panic!("{areas:?}"));
    let (_, o) = packet(s, office["id"].as_str().unwrap());
    assert_walls_are_elements(s, &o);
    assert_walls_within(&o, &nums(&office["box"]), 1.5);
    // Its four sides: the south and north shell walls, the west one and the partition.
    let n = o["walls"].as_array().unwrap().len();
    assert!((4..=6).contains(&n), "{:#}", o["walls"]);
    let pre = s.execute("validate.prescreen", &json!({"rules": ["G-HOST"], "all": true})).unwrap();
    let host: Vec<&Value> = pre["findings"].as_array().unwrap().iter().filter(|f| f["rule"] == "G-HOST").collect();
    assert!(!host.is_empty() && host.iter().all(|f| f["verdict"] == "pass"), "{pre:#}");
}

#[test]
fn walls_built_after_a_big_print_set_are_found() {
    // More objects before the build than the model reading once looked at (500,000).
    let mut s = built(520_000);
    assert!(s.doc().unwrap().model.len() > 520_000);
    assert_hosted_and_listed(&mut s);
}

/// Each built wall box as Walk mode draws a wall: its two long faces standing on the floor, a
/// cap over them (drawn twice), its ends; no bottom, no wall type.
fn walk_mode_walls(s: Session) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    let walls: Vec<Arc<Entity>> = d.model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).cloned().collect();
    for w in walls {
        let EntityKind::Mesh(m) = &w.kind else { continue };
        let mut out = Mesh { vertices: m.vertices.clone(), faces: Vec::new(), kind: m.kind };
        let z0 = m.vertices.iter().fold(f64::MAX, |a, v| a.min(v.z));
        for f in &m.faces {
            let vs: Vec<f64> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).map(|v| v.z)).collect();
            if vs.iter().all(|z| (z - z0).abs() < 1e-9) {
                continue;
            }
            out.faces.push(f.clone());
            if vs.iter().all(|z| (z - vs[0]).abs() < 1e-9) {
                let mut again = f.v.clone();
                again.rotate_left(2);
                out.faces.push(MeshFace { v: again, hidden: 0 });
            }
        }
        // Walk mode's walls have no wall type.
        let _ = d.modify_entity(w.handle, |e| {
            e.kind = EntityKind::Mesh(out);
            let x: Vec<_> = e.common.xdata.iter().filter(|t| !t.value.starts_with("type:")).cloned().collect();
            e.common.xdata = x.into();
        });
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Floor Plan", None);
    s2
}

#[test]
fn walls_walk_mode_built_are_found_by_their_caps() {
    let mut s = walk_mode_walls(built(0));
    assert_hosted_and_listed(&mut s);
    let r = s.execute("bim.element", &json!({"handle": receptacle(&s)})).unwrap();
    assert!(r["host"]["type"].is_null(), "an untyped wall: {r:#}");
}
