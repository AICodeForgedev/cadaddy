//! Q6207 arexport: a room of the built model as AR files (`bim.export.ar`) — a .glb read back
//! (nodes, positions within 1 mm, metadata, the anchor at the origin), the .usdz's package
//! structure (stored, 64-byte aligned, root layer first, Y up in metres), the caps (an oversized
//! scope simplified and reported) and hostile input. Made-up plans only.

use std::path::{Path, PathBuf};

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Arc, Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-arexport-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn push(d: &mut Drawing, layer: &str, kind: EntityKind) {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, kind);
    e.common.layer = layer.into();
    d.model.push(e);
}

/// A line in feet.
fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    push(d, layer, EntityKind::Line(Line { a: p(a), b: p(b) }));
}

/// A door in a vertical partition (faces at `x0` and `x1`) from `y0` to `y1` feet: jambs, the
/// open leaf and its swing.
fn door(d: &mut Drawing, x0: f64, x1: f64, y0: f64, y1: f64) {
    line(d, "A-WALL", (x0, y0), (x1, y0));
    line(d, "A-WALL", (x0, y1), (x1, y1));
    line(d, "A-DOOR", (x1, y1), (x1 + (y1 - y0), y1));
    push(
        d,
        "A-DOOR",
        EntityKind::Arc(Arc {
            center: Vec3 { x: x1 * 12.0, y: y1 * 12.0, z: 0.0 },
            radius: (y1 - y0) * 12.0,
            start: 1.5 * std::f64::consts::PI,
            end: 0.0,
        }),
    );
}

/// A closet (0.5–4.75'), a guest room (5.25–19.75') and a corridor (20.25–60'), 0.5–9.5' deep,
/// 6" walls; a 2.5' closet door and a 3' corridor door in the guest room's side walls.
fn plan() -> Drawing {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (60.5, 0.)), ((60.5, 0.), (60.5, 10.)), ((60.5, 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        line(&mut d, "A-WALL", a, b);
    }
    // Closet.
    for (a, b) in
        [((0.5, 0.5), (4.75, 0.5)), ((4.75, 0.5), (4.75, 6.0)), ((4.75, 8.5), (4.75, 9.5)), ((4.75, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))]
    {
        line(&mut d, "A-WALL", a, b);
    }
    // Guest room.
    for (a, b) in [
        ((5.25, 0.5), (19.75, 0.5)),
        ((19.75, 0.5), (19.75, 2.0)),
        ((19.75, 5.0), (19.75, 9.5)),
        ((19.75, 9.5), (5.25, 9.5)),
        ((5.25, 9.5), (5.25, 8.5)),
        ((5.25, 6.0), (5.25, 0.5)),
    ] {
        line(&mut d, "A-WALL", a, b);
    }
    // Corridor.
    for (a, b) in [
        ((20.25, 0.5), (60.0, 0.5)),
        ((60.0, 0.5), (60.0, 9.5)),
        ((60.0, 9.5), (20.25, 9.5)),
        ((20.25, 9.5), (20.25, 5.0)),
        ((20.25, 2.0), (20.25, 0.5)),
    ] {
        line(&mut d, "A-WALL", a, b);
    }
    door(&mut d, 4.75, 5.25, 6.0, 8.5);
    door(&mut d, 19.75, 20.25, 2.0, 5.0);
    d
}

fn text_on(s: &mut Session, layer: &str, at: [f64; 2], t: &str) {
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
}

fn device(x: f64, y: f64, mark: &str, kind: &str, symbol: &str, extra: Value) -> Value {
    let mut fields = json!({
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("{kind} {mark}"), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "symbol": {"value": symbol, "source": "plan"},
    });
    if let (Some(f), Some(x)) = (fields.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            f.insert(k.clone(), json!({"value": v, "source": "plan"}));
        }
    }
    json!({"id": format!("device:L1:{x:.1},{y:.1}:{mark}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": fields})
}

/// The three rooms built with three devices at known heights in the guest room.
fn built(extra: Value) -> Session {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Sample Floor Plan", None);
    for (x, n, name) in [(18.0, "103", "CLOSET"), (110.0, "101", "GUEST"), (400.0, "102", "CORRIDOR")] {
        text_on(&mut s, "A-ROOM-TAG", [x, 64.0], name);
        text_on(&mut s, "A-ROOM-TAG", [x + 6.0, 52.0], n);
    }
    let tags = vec![
        device(8.0, 0.8, "REC", "receptacle", "1F0", json!({"mount_height_in": "18", "circuit": "LP-1"})),
        device(12.0, 0.8, "S", "switch", "1F1", json!({"mount_height_in": "48"})),
        device(16.0, 9.2, "T", "thermostat", "1F2", json!({"mount_height_in": "60"})),
        device(10.0, 9.2, "P-1", "plumbing_fixture", "1F3", json!({"fixture": "LAVATORY"})),
    ];
    let mut sp = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-DOOR", "role": "door"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "openings": {"enabled": true, "min_ft": 1.5, "max_ft": 16.0, "door_head_ft": 7.0, "window_head_ft": 7.0, "window_sill_ft": 2.0},
        "tags": tags,
    });
    if let (Some(o), Some(x)) = (sp.as_object_mut(), extra.as_object()) {
        o.extend(x.clone());
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

// ─── Reading the files back ──────────────────────────────────────────────────

struct Glb {
    doc: Value,
    bin: Vec<u8>,
}

fn u32_at(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
}

fn read_glb(path: &Path) -> Glb {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[0..4], b"glTF");
    assert_eq!(u32_at(&b, 4), 2, "glTF version 2");
    assert_eq!(u32_at(&b, 8), b.len(), "the header's length is the file's");
    let jlen = u32_at(&b, 12);
    assert_eq!(&b[16..20], b"JSON");
    assert_eq!(jlen % 4, 0);
    let doc: Value = serde_json::from_slice(&b[20..20 + jlen]).unwrap();
    let at = 20 + jlen;
    let blen = u32_at(&b, at);
    assert_eq!(&b[at + 4..at + 8], b"BIN\0");
    let bin = b[at + 8..at + 8 + blen].to_vec();
    assert_eq!(doc["buffers"][0]["byteLength"].as_u64().unwrap() as usize, bin.len());
    Glb { doc, bin }
}

impl Glb {
    fn floats(&self, accessor: usize) -> Vec<[f32; 3]> {
        let a = &self.doc["accessors"][accessor];
        let v = &self.doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let off = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let n = a["count"].as_u64().unwrap() as usize;
        assert_eq!(a["componentType"], json!(5126));
        (0..n)
            .map(|i| {
                let f = |k: usize| f32::from_le_bytes(self.bin[off + (i * 3 + k) * 4..off + (i * 3 + k) * 4 + 4].try_into().unwrap());
                [f(0), f(1), f(2)]
            })
            .collect()
    }

    /// A node's box (metres): its mesh's POSITION bounds moved by its translation.
    fn node_box(&self, node: &Value) -> ([f64; 3], [f64; 3]) {
        let mesh = &self.doc["meshes"][node["mesh"].as_u64().unwrap() as usize];
        let acc = mesh["primitives"][0]["attributes"]["POSITION"].as_u64().unwrap() as usize;
        let t: Vec<f64> = node["translation"].as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap()).collect()).unwrap_or(vec![0.0; 3]);
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in self.floats(acc) {
            for k in 0..3 {
                lo[k] = lo[k].min(f64::from(p[k]) + t[k]);
                hi[k] = hi[k].max(f64::from(p[k]) + t[k]);
            }
        }
        // The accessor's declared bounds are the data's.
        let a = &self.doc["accessors"][acc];
        let pts = self.floats(acc);
        for k in 0..3 {
            let dmin = pts.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min);
            assert_eq!(a["min"][k].as_f64().unwrap(), f64::from(dmin), "accessor min");
        }
        (lo, hi)
    }

    fn element_nodes(&self) -> Vec<Value> {
        self.doc["nodes"].as_array().unwrap().iter().filter(|n| n.get("mesh").is_some()).cloned().collect()
    }
}

/// The zip's entries: (name, method, data offset, data).
fn read_zip(path: &Path) -> Vec<(String, u16, usize, Vec<u8>)> {
    let z = std::fs::read(path).unwrap();
    let mut out = Vec::new();
    let mut at = 0usize;
    while z.len() >= at + 4 && z[at..at + 4] == [0x50, 0x4B, 0x03, 0x04] {
        let method = u16::from_le_bytes([z[at + 8], z[at + 9]]);
        let crc = u32_at(&z, at + 14) as u32;
        let size = u32_at(&z, at + 18);
        let n = u16::from_le_bytes([z[at + 26], z[at + 27]]) as usize;
        let x = u16::from_le_bytes([z[at + 28], z[at + 29]]) as usize;
        let name = String::from_utf8(z[at + 30..at + 30 + n].to_vec()).unwrap();
        let data_at = at + 30 + n + x;
        let data = z[data_at..data_at + size].to_vec();
        assert_eq!(crc32(&data), crc, "{name}: crc");
        out.push((name, method, data_at, data));
        at = data_at + size;
    }
    // The central directory and its end record follow.
    assert_eq!(z[at..at + 4], [0x50, 0x4B, 0x01, 0x02]);
    assert_eq!(z[z.len() - 22..z.len() - 18], [0x50, 0x4B, 0x05, 0x06]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for b in data {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn mesh_box_in(s: &Session, h: &str) -> ([f64; 3], [f64; 3]) {
    let d = s.doc().unwrap();
    let e = d.model.get(Handle::parse_hex(h).unwrap()).unwrap();
    let EntityKind::Mesh(m) = &e.kind else { panic!("{h} is a mesh") };
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in &m.vertices {
        for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
            lo[k] = lo[k].min(c);
            hi[k] = hi[k].max(c);
        }
    }
    (lo, hi)
}

fn export(s: &mut Session, d: &Path, name: &str, mut p: Value) -> Value {
    p["path"] = json!(d.join(name).to_string_lossy());
    s.execute("bim.export.ar", &p).unwrap_or_else(|e| panic!("export: {e}"))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn a_room_round_trips_with_its_devices_where_the_model_has_them() {
    let _g = serial();
    let mut s = built(json!({}));
    let d = dir("room");
    let r = export(&mut s, &d, "guest", json!({"room": "101"}));
    assert_eq!(r["files"]["glb"], json!(d.join("guest.glb").to_string_lossy()), "{r:#}");
    assert!(d.join("guest.usdz").exists() && d.join("guest.json").exists());

    // The anchor: the guest room's corner nearest its entry (the corridor door, not the
    // closet's), on the floor.
    let side: Value = serde_json::from_slice(&std::fs::read(d.join("guest.json")).unwrap()).unwrap();
    let a = &side["anchor"];
    assert_eq!(a["kind"], json!("room_corner"), "{a:#}");
    assert_eq!(a["room"], json!("101 GUEST"), "{a:#}");
    assert_eq!(a["entry"]["to"], json!("102 CORRIDOR"), "{a:#}");
    let (ax, ay) = (a["plan_x"].as_f64().unwrap(), a["plan_y"].as_f64().unwrap());
    assert!((ax - 19.75).abs() < 0.3 && (ay - 0.5).abs() < 0.3, "the corner by the corridor door: {a:#}");
    assert_eq!(side["up_axis"], json!("Y"));
    assert_eq!(side["meters_per_unit"], json!(1));
    let named = side["points"].as_array().unwrap().iter().find(|p| p["name"] == a["name"]).expect("the anchor is a listed point");
    for c in named["position"].as_array().unwrap() {
        assert!(c.as_f64().unwrap().abs() < 1e-6, "the anchor is the origin: {named}");
    }

    // The .glb: one node per element, the devices where the drawing has them (1 mm).
    let g = read_glb(&d.join("guest.glb"));
    let nodes = g.element_nodes();
    assert_eq!(nodes.len(), r["nodes"].as_u64().unwrap() as usize);
    assert_eq!(nodes.len(), side["elements"].as_array().unwrap().len());
    assert_eq!(g.doc["nodes"][0]["children"].as_array().unwrap().len(), nodes.len(), "the root holds every element");
    let devices: Vec<&Value> = nodes.iter().filter(|n| n["extras"]["devid"].is_string()).collect();
    assert_eq!(devices.len(), 4, "{:#}", r);
    // A plumbing fixture is plumbing (the rules' kind trades), whatever its layer.
    let lav = devices.iter().find(|n| n["extras"]["tag"] == json!("P-1")).expect("the lavatory");
    assert_eq!(lav["extras"]["trade"], json!("plumbing"), "{:#}", lav["extras"]);
    // Everything is cut to the room and its margin: the walls running on past it too.
    // (The room's outline as the build traced it, its door swings included.)
    let xs: Vec<f64> = side["points"].as_array().unwrap().iter().map(|p| p["plan"][0].as_f64().unwrap()).collect();
    let (xmin, xmax) = (xs.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    assert!(xmax < 30.0, "the corridor is not the guest room: {xs:?}");
    let (x0, x1) = ((xmin - 1.0 - ax) * 0.3048 - 1e-3, (xmax + 1.0 - ax) * 0.3048 + 1e-3);
    for n in &nodes {
        let (lo, hi) = g.node_box(n);
        assert!(lo[0] >= x0 && hi[0] <= x1, "{} within the room {x0}..{x1}: {lo:?}..{hi:?} {n}", n["name"]);
    }
    assert!(r["cut_to_scope"].as_u64().unwrap() > 0, "{r:#}");
    let m = 0.3048 / 12.0;
    for (tag, kind, height) in [("REC", "receptacle", 18.0), ("S", "switch", 48.0), ("T", "thermostat", 60.0)] {
        let n = devices.iter().find(|n| n["extras"]["tag"] == json!(tag)).unwrap_or_else(|| panic!("{tag}: {devices:#?}"));
        let x = &n["extras"];
        assert_eq!(x["kind"], json!(kind), "{x:#}");
        assert_eq!(x["mounting_height_in"].as_f64().unwrap(), height, "{x:#}");
        assert_eq!(x["mounting_height_from"], json!("documented"));
        assert!((x["mounting_height_m"].as_f64().unwrap() - height * 0.0254).abs() < 1e-4);
        assert_eq!(x["trade"], json!("electrical"), "{x:#}");
        assert_eq!(x["level"], json!("L1"));
        assert_eq!(x["room"], json!("101 GUEST"));
        assert!(x["source"].as_str().unwrap().starts_with("model.build"), "{x:#}");
        assert!(n["name"].as_str().unwrap().starts_with(&format!("{kind}_{tag}")), "named by kind and tag: {}", n["name"]);
        // Where it is: drawing inches → metres from the anchor, Y up, plan y along −Z.
        let (lo, hi) = mesh_box_in(&s, x["id"].as_str().unwrap());
        let want_lo = [(lo[0] - ax * 12.0) * m, lo[2] * m, -(hi[1] - ay * 12.0) * m];
        let want_hi = [(hi[0] - ax * 12.0) * m, hi[2] * m, -(lo[1] - ay * 12.0) * m];
        let (glo, ghi) = g.node_box(n);
        for k in 0..3 {
            assert!(
                (glo[k] - want_lo[k]).abs() < 0.001 && (ghi[k] - want_hi[k]).abs() < 0.001,
                "{tag} axis {k}: {glo:?}..{ghi:?} vs {want_lo:?}..{want_hi:?}"
            );
        }
        // Its bottom above the floor is the model's.
        assert!((glo[1] - x["bottom_aff_in"].as_f64().unwrap() * 0.0254).abs() < 0.001);
    }
    let rec = devices.iter().find(|n| n["extras"]["tag"] == json!("REC")).unwrap();
    assert_eq!(rec["extras"]["circuit"], json!("LP-1"));
    // The shell is there as see-through context.
    assert!(nodes.iter().any(|n| n["extras"]["kind"] == json!("wall") && n["extras"]["context"] == json!(true)), "walls as context");
    let mats = g.doc["materials"].as_array().unwrap();
    assert!(mats.iter().any(|m| m["alphaMode"] == json!("BLEND")), "the context is translucent");
    // Equal parts share a mesh: the receptacle and the switch boxes along one wall are one shape.
    let rec_mesh = |tag: &str| devices.iter().find(|n| n["extras"]["tag"] == json!(tag)).unwrap()["mesh"].clone();
    assert_eq!(rec_mesh("REC"), rec_mesh("S"), "one mesh for the two equal device boxes");
    assert!(r["meshes"].as_u64().unwrap() < r["nodes"].as_u64().unwrap(), "{r:#}");
    // Uncut, the walls run the plan's length.
    let whole = export(&mut s, &d, "whole", json!({"room": "101", "clip": false, "formats": ["glb"]}));
    assert_eq!(whole["cut_to_scope"], json!(0));
    let gw = read_glb(&d.join("whole.glb"));
    let widest = gw
        .element_nodes()
        .iter()
        .map(|n| {
            let (lo, hi) = gw.node_box(n);
            hi[0] - lo[0]
        })
        .fold(0.0, f64::max);
    assert!(widest > 50.0 * 0.3048, "an outer wall's 60 ft: {widest}");
}

#[test]
fn the_usdz_is_a_stored_aligned_package_with_the_root_layer_first() {
    let _g = serial();
    let mut s = built(json!({}));
    let d = dir("usdz");
    let r = export(&mut s, &d, "room.usdz", json!({"room": "GUEST"}));
    let z = read_zip(&d.join("room.usdz"));
    assert!(!z.is_empty());
    assert_eq!(z[0].0, "model.usda", "the root layer first");
    for (name, method, at, _) in &z {
        assert_eq!(*method, 0, "{name} is stored, not compressed");
        assert_eq!(at % 64, 0, "{name}'s data is 64-byte aligned");
    }
    let text = String::from_utf8(z[0].3.clone()).unwrap();
    assert!(text.starts_with("#usda 1.0"));
    for want in
        ["upAxis = \"Y\"", "metersPerUnit = 1", "defaultPrim = \"CADaddy_AR\"", "info:id = \"UsdPreviewSurface\"", "string schema = \"cadaddy.ar/1\""]
    {
        assert!(text.contains(want), "{want}");
    }
    // One instanceable element per node, each with its metadata.
    assert_eq!(text.matches("instanceable = true").count() as u64, r["nodes"].as_u64().unwrap());
    assert!(text.contains("string kind = \"receptacle\"") && text.contains("string tag = \"REC\""));
    assert!(text.contains("prepend references = </CADaddy_AR/Prototypes/part"));
}

#[test]
fn an_oversized_scope_is_simplified_and_says_what_went() {
    let _g = serial();
    let mut s = built(json!({}));
    let d = dir("caps");
    let full = export(&mut s, &d, "level", json!({"level": "L1", "formats": ["glb"]}));
    assert_eq!(full["simplified"], json!("nothing"), "{full:#}");
    let n = full["triangles"].as_u64().unwrap();
    // A cap under the shell's triangles: the context is simplified or dropped first.
    let cap = n / 3;
    let small = export(&mut s, &d, "level-capped", json!({"level": "L1", "formats": ["glb"], "max_triangles": cap}));
    assert!(small["triangles"].as_u64().unwrap() <= cap, "{small:#}");
    assert_ne!(small["simplified"], json!("nothing"), "{small:#}");
    // The devices are kept over the shell.
    assert!(small["kinds"]["receptacle"].as_u64().unwrap_or(0) == 1, "{small:#}");
    // A cap so low only a few elements fit: the far ones go, and are counted.
    let tiny = export(&mut s, &d, "level-tiny", json!({"level": "L1", "formats": ["glb"], "max_triangles": 30}));
    assert!(tiny["triangles"].as_u64().unwrap() <= 30, "{tiny:#}");
    let over = &tiny["dropped"]["over_caps"];
    assert!(over.as_object().is_some_and(|o| !o.is_empty()), "what went is reported: {tiny:#}");
    // A byte cap too.
    let bytes = export(&mut s, &d, "level-bytes", json!({"level": "L1", "formats": ["glb"], "max_bytes": 20_000}));
    assert!(bytes["bytes"]["glb"].as_u64().unwrap() <= 20_000, "{bytes:#}");
}

#[test]
fn filters_anchors_and_job_rules() {
    let _g = serial();
    let mut s = built(json!({}));
    let d = dir("filters");
    let set =
        s.execute("bim.export.ar.rules", &json!({"rules": {"colors": {"by_system": {"devices": [255, 0, 0]}}, "context": {"mode": "off"}}})).unwrap();
    assert_eq!(set["own"], json!(true));
    assert_eq!(set["rules"]["caps"]["max_triangles"], json!(300_000), "the rest stays shipped: {set:#}");
    s.execute("bim.export.ar.rules", &json!({"rules": "red"})).unwrap_err();
    // The job's rules: devices red, no context.
    let r = export(&mut s, &d, "red", json!({"room": "101", "formats": ["glb"]}));
    let g = read_glb(&d.join("red.glb"));
    assert!(g.element_nodes().iter().all(|n| n["extras"]["context"].is_null()), "context off by the job's rules");
    let mats = g.doc["materials"].as_array().unwrap();
    assert!(mats.iter().any(|m| m["pbrMetallicRoughness"]["baseColorFactor"] == json!([1.0, 0.0, 0.0, 1.0])), "{mats:#?}");
    assert!(r["dropped"]["context_off"].as_u64().unwrap() > 0);
    // A kind filter, the context back on by the call.
    let k = export(&mut s, &d, "switch", json!({"room": "101", "kinds": ["switch"], "context": "translucent", "formats": ["glb"]}));
    assert_eq!(k["kinds"]["switch"], json!(1), "{k:#}");
    assert!(k["kinds"].get("receptacle").is_none(), "{k:#}");
    // A corner by number, and a named point at given coordinates.
    let c = export(&mut s, &d, "corner", json!({"room": "101", "anchor": {"corner": 1}, "formats": ["glb"]}));
    assert_eq!(c["anchor"]["kind"], json!("room_corner"));
    assert!(c["anchor"]["name"].as_str().unwrap().ends_with("corner 1"));
    let p = export(&mut s, &d, "point", json!({"room": "101", "anchor": {"name": "survey mark", "at": [10, 5]}, "formats": ["glb"]}));
    assert_eq!(p["anchor"]["kind"], json!("named_point"));
    assert_eq!((p["anchor"]["plan_x"].as_f64(), p["anchor"]["plan_y"].as_f64()), (Some(10.0), Some(5.0)));
    // The default scope: one room.
    let def = export(&mut s, &d, "default", json!({"formats": ["glb"]}));
    assert_eq!(def["scope"]["default_room"], json!(true));
    assert_eq!(def["scope"]["rooms"].as_array().unwrap().len(), 1);
    // Back to the shipped rules: the context returns.
    let back = s.execute("bim.export.ar.rules", &json!({"reset": true})).unwrap();
    assert_eq!(back["own"], json!(false));
    let r = export(&mut s, &d, "reset", json!({"room": "101", "formats": ["glb"]}));
    assert_eq!(r["dropped"]["context_off"], json!(0), "{r:#}");
}

#[test]
fn hostile_calls_are_refused_not_panicked() {
    let _g = serial();
    let d = dir("hostile");
    // Nothing built.
    let mut empty = Session::empty();
    empty.open_drawing(Drawing::new_imperial(), "Empty", None);
    let e = empty.execute("bim.export.ar", &json!({"path": d.join("none").to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("model.build"), "{e}");
    let mut s = built(json!({}));
    let path = d.join("x").to_string_lossy().into_owned();
    for (p, says) in [
        (json!({"room": "999"}), "no room"),
        (json!({"level": "L9"}), "no level"),
        (json!({"room": "101", "kinds": ["no-such-kind"], "context": "off"}), "nothing built"),
        (json!({"box": {"min": [0, 0]}}), "box"),
        (json!({"box": {"min": [0, 0], "max": [1e9, 1]}}), "100,000"),
        (json!({"room": "101", "anchor": {"corner": 99}}), "corners"),
        (json!({"room": "101", "anchor": {"corner": -1}}), "whole number"),
        (json!({"room": "101", "anchor": "x"}), "object"),
        (json!({"room": "101", "anchor": {"name": "nowhere"}}), "anchor.at"),
        (json!({"room": "101", "formats": ["obj"]}), "glb or usdz"),
        (json!({"room": "101", "context": "maybe"}), "translucent"),
        (json!({"room": "101", "max_triangles": -5}), "max_triangles"),
        (json!({"room": "101", "rooms": 7}), "rooms"),
        (json!({"room": "101", "path": d.join("x.dwg").to_string_lossy()}), "drawing"),
    ] {
        let mut p = p;
        if p.get("path").is_none() {
            p["path"] = json!(path);
        }
        let e = s.execute("bim.export.ar", &p).expect_err(&p.to_string()).to_string();
        assert!(e.contains(says), "{p}: {e}");
    }
    assert!(!d.join("x.glb").exists(), "nothing written for a refused call");
    // NaN geometry in a built element: left out, the rest exported.
    let h = {
        let r = s.execute("bim.props.find", &json!({"kind": "device", "limit": 1})).unwrap();
        Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap()
    };
    let _ = s.doc_mut().unwrap().modify_entity(h, |e| {
        if let EntityKind::Mesh(m) = &mut e.kind {
            for v in &mut m.vertices {
                v.x = f64::NAN;
            }
        }
    });
    let r = export(&mut s, &d, "nan", json!({"room": "101"}));
    assert!(r["nodes"].as_u64().unwrap() > 0, "{r:#}");
    // An existing file is not replaced without force.
    std::fs::write(d.join("keep.glb"), b"precious").unwrap();
    let e = s.execute("bim.export.ar", &json!({"room": "101", "path": d.join("keep").to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("force:true"), "{e}");
    assert_eq!(std::fs::read(d.join("keep.glb")).unwrap(), b"precious");
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Q6218 arupdate: `bim.export.ar.manifest {dir}` lists each export in a folder with every file's
/// relative name, size and SHA-256; re-running replaces its own manifest; only the re-exported
/// room's hashes change; nothing but manifest.json is written; hostile folders are refused or
/// skipped, never panicked on.
#[test]
fn the_manifest_lists_each_export_with_sizes_and_hashes() {
    let _g = serial();
    let mut s = built(json!({}));
    let d = dir("manifest");
    export(&mut s, &d, "guest", json!({"room": "101"}));
    std::fs::create_dir_all(d.join("sub")).unwrap();
    export(&mut s, &d.join("sub"), "corridor", json!({"room": "102", "formats": ["glb"]}));
    // Not a package: a stray .json, a hidden folder, and a sidecar naming a path outside.
    std::fs::write(d.join("notes.json"), b"{\"hello\": 1}").unwrap();
    std::fs::create_dir_all(d.join(".cache")).unwrap();
    std::fs::write(d.join(".cache/x.json"), b"{}").unwrap();
    std::fs::write(d.join("evil.json"), br#"{"schema": "cadaddy.ar/1", "glb": "../../etc/passwd"}"#).unwrap();
    std::fs::write(d.join("empty.json"), br#"{"schema": "cadaddy.ar/1"}"#).unwrap();
    // A hand-made sidecar naming only `model` (the field app's own sample layout).
    std::fs::write(d.join("hand.glb"), b"glTF-ish").unwrap();
    std::fs::write(d.join("hand.json"), br#"{"format": "cadaddy-ar", "model": "hand.glb", "title": "Sample Room"}"#).unwrap();
    #[cfg(unix)]
    {
        std::fs::write(d.join("linked.json"), br#"{"schema": "cadaddy.ar/1", "glb": "linked.glb"}"#).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", d.join("linked.glb")).unwrap();
    }
    let before: Vec<(String, Vec<u8>)> = ["guest.glb", "guest.usdz", "guest.json", "sub/corridor.glb", "sub/corridor.json"]
        .iter()
        .map(|n| (n.to_string(), std::fs::read(d.join(n)).unwrap()))
        .collect();

    let r = s.execute("bim.export.ar.manifest", &json!({"dir": d.to_string_lossy()})).unwrap();
    assert_eq!(r["packages"], json!(3), "{r:#}");
    assert_eq!(r["files"], json!(7), "{r:#}");
    let skipped = r["skipped"].as_array().unwrap();
    assert!(skipped.iter().any(|x| x["name"] == "evil.json" && x["why"].as_str().unwrap().contains("plain file name")), "{r:#}");
    assert!(skipped.iter().any(|x| x["name"] == "empty.json"), "{r:#}");
    #[cfg(unix)]
    assert!(skipped.iter().any(|x| x["name"] == "linked.json" && x["why"].as_str().unwrap().contains("link")), "{r:#}");
    let m: Value = serde_json::from_slice(&std::fs::read(d.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["format"], json!("cadaddy-ar-manifest"));
    assert_eq!(m["version"], json!(1));
    let pk = m["packages"].as_array().unwrap();
    let names: Vec<&str> = pk.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["guest", "hand", "sub/corridor"]);
    assert_eq!(pk[1]["label"], json!("Sample Room"));
    let guest = &pk[0];
    assert_eq!(guest["label"], json!("101 GUEST"), "{guest:#}");
    assert_eq!(guest["level"], json!("L1"), "{guest:#}");
    assert!(guest["exported"].as_str().unwrap().ends_with('Z'), "{guest:#}");
    for p in pk {
        for f in p["files"].as_array().unwrap() {
            let n = f["name"].as_str().unwrap();
            let bytes = std::fs::read(d.join(n)).unwrap();
            assert_eq!(f["bytes"].as_u64().unwrap() as usize, bytes.len(), "{n}");
            assert_eq!(f["sha256"].as_str().unwrap(), sha256_hex(&bytes), "{n}");
        }
    }
    let roles: Vec<&str> = pk[2]["files"].as_array().unwrap().iter().map(|f| f["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["glb", "sidecar"]);
    // Read-only apart from manifest.json.
    for (n, b) in &before {
        assert_eq!(&std::fs::read(d.join(n)).unwrap(), b, "{n} unchanged");
    }

    // Re-export one room (a changed model) and re-run: its own manifest is replaced without
    // force, the guest room's hashes change and the corridor's don't.
    let hashes = |m: &Value, i: usize| -> Vec<String> {
        m["packages"][i]["files"].as_array().unwrap().iter().map(|f| f["sha256"].as_str().unwrap().to_string()).collect()
    };
    export(&mut s, &d, "guest", json!({"room": "101", "context": "off", "force": true}));
    s.execute("bim.export.ar.manifest", &json!({"dir": d.to_string_lossy()})).unwrap();
    let m2: Value = serde_json::from_slice(&std::fs::read(d.join("manifest.json")).unwrap()).unwrap();
    assert_ne!(hashes(&m, 0)[0], hashes(&m2, 0)[0], "the re-exported model's hash changed");
    assert_eq!(hashes(&m, 2), hashes(&m2, 2), "the untouched export keeps its hashes");

    // A manifest.json that isn't ours is kept unless force:true.
    let other = dir("manifest-other");
    std::fs::write(other.join("manifest.json"), b"{\"mine\": true}").unwrap();
    let e = s.execute("bim.export.ar.manifest", &json!({"dir": other.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("force:true"), "{e}");
    assert_eq!(std::fs::read(other.join("manifest.json")).unwrap(), b"{\"mine\": true}");
    let r = s.execute("bim.export.ar.manifest", &json!({"dir": other.to_string_lossy(), "force": true})).unwrap();
    assert_eq!(r["packages"], json!(0));

    // Hostile calls.
    for (p, says) in [
        (json!({}), "required"),
        (json!({"dir": ""}), "required"),
        (json!({"dir": 7}), "required"),
        (json!({"dir": d.join("nowhere").to_string_lossy()}), "not a folder"),
        (json!({"dir": d.join("guest.glb").to_string_lossy()}), "not a folder"),
        (json!({"dir": d.to_string_lossy(), "force": "yes"}), "force"),
    ] {
        let e = s.execute("bim.export.ar.manifest", &p).expect_err(&p.to_string()).to_string();
        assert!(e.contains(says), "{p}: {e}");
    }
}
