//! Validate's wall list holds every built wall whatever its footprint (Q6139): an L-shaped
//! piece, a notched one and a six-cornered one are read as the rectangles they are made of, a
//! wall's piece over a door is a face too (an element below it is in the opening, and says so),
//! a wall whose footprint is a sliver is listed with why, `bim.element` on a wall finds the wall
//! itself, and G-HOST measures to an element's stored host wall first — on a made-up one-level
//! plan whose built walls are swapped for those shapes.

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::props::{self, Prop, Source};
use cadaddy_engine::doc::{
    Attrib, AttribModes, AttribProps, Block, Drawing, Entity, EntityKind, HAlign, Handle, Insert, Line, Mesh, MeshFace, Text, VAlign,
};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;

/// A plan point (feet) on the sheet.
fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

/// Where the receptacles are (feet), each on one of the walls [`shapes`] builds.
const ON_L_LONG: [f64; 2] = [22.0, 3.55];
const ON_L_SHORT: [f64; 2] = [17.55, 5.0];
const IN_NOTCH: [f64; 2] = [22.0, 7.8];
const ON_HEX: [f64; 2] = [34.0, 3.55];
const UNDER_HEADER: [f64; 2] = [33.5, 6.55];
const BESIDE_DOOR: [f64; 2] = [31.0, 6.55];

fn text(p: [f64; 2], value: &str) -> Text {
    Text {
        insert: Vec3::new(p[0], p[1], 0.0),
        align_pt: None,
        height: 6.0,
        value: value.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::default(),
        valign: VAlign::default(),
    }
}

/// A one-level plan (a 40' × 12' shell of 6" walls, a partition at 15') with receptacles at
/// the points above, built.
fn built() -> Session {
    let mut d = Drawing::new_imperial();
    let add = |d: &mut Drawing, layer: &str, k: EntityKind| {
        let h = d.new_handle();
        let mut e = Entity::new(h, k);
        e.common.layer = layer.into();
        d.model.push(e);
        h
    };
    let l = |a: [f64; 2], b: [f64; 2]| {
        let (a, b) = (at(a), at(b));
        EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) })
    };
    for (a, b) in [([0.0, 0.0], [40.0, 0.0]), ([40.0, 0.0], [40.0, 12.0]), ([40.0, 12.0], [0.0, 12.0]), ([0.0, 12.0], [0.0, 0.0])] {
        let _ = add(&mut d, "A-WALL", l(a, b));
    }
    for (a, b) in [([0.5, 0.5], [39.5, 0.5]), ([39.5, 0.5], [39.5, 11.5]), ([39.5, 11.5], [0.5, 11.5]), ([0.5, 11.5], [0.5, 0.5])] {
        let _ = add(&mut d, "A-WALL", l(a, b));
    }
    let _ = add(&mut d, "A-WALL", l([15.0, 0.5], [15.0, 11.5]));
    let _ = add(&mut d, "A-WALL", l([15.5, 0.5], [15.5, 11.5]));
    for (t, q) in [("OFFICE", [5.0, 6.0]), ("STORAGE", [25.0, 10.0])] {
        let _ = add(&mut d, "A-ROOM-TAG", EntityKind::Text(text(at(q), t)));
    }
    let mut b = Block::new("RECEP");
    b.entities.push(Entity::new(d.new_handle(), EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
    d.blocks.insert("RECEP".into(), Arc::new(b));
    let points = [ON_L_LONG, ON_L_SHORT, IN_NOTCH, ON_HEX, UNDER_HEADER, BESIDE_DOOR];
    let mut symbols = Vec::new();
    for q in points {
        let p = at(q);
        let attribs = vec![Attrib {
            tag: "TYPE".into(),
            text: text(p, "receptacle"),
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
        symbols.push(add(&mut d, "E-POWR", EntityKind::Insert(ins)).hex());
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let tags: Vec<Value> = points
        .iter()
        .zip(&symbols)
        .enumerate()
        .map(|(i, (q, sym))| {
            let p = at(*q);
            json!({"id": format!("device:L1:r{i}"), "kind": "device", "level": "L1", "at_ft": [p[0] / 12.0, p[1] / 12.0], "fields": {
                "type": {"value": "A", "source": "plan"}, "device_type": {"value": "duplex receptacle", "source": "plan"},
                "sheet": {"value": "A-101", "source": "plan"}, "symbol": {"value": sym, "source": "plan"}}})
        })
        .collect();
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [0, 0, SW, SH]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}, {"layer": "E-POWR", "role": "device"}],
        "devices": {"by_type": {"receptacle": {"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]}}},
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "rooms": [{"name": "Office", "number": "101", "level": "L1"}, {"name": "Storage", "number": "102", "level": "L1"}],
        "tags": tags,
        "sheets": [{"number": "A-101", "title": "FLOOR PLAN", "discipline": "A", "level": "L1", "source": "model", "rect": [0, 0, SW, SH]}]
    });
    s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// A prism over `outline` (feet) from `z0` to `z1` (inches): its bottom and top faces and its
/// sides, as a built wall box is drawn.
fn prism(m: &mut Mesh, outline: &[[f64; 2]], z0: f64, z1: f64) {
    let base = m.vertices.len() as u32;
    let n = outline.len() as u32;
    for z in [z0, z1] {
        for q in outline {
            let p = at(*q);
            m.vertices.push(Vec3::new(p[0], p[1], z));
        }
    }
    m.faces.push(MeshFace { v: (0..n).rev().map(|i| base + i).collect(), hidden: 0 });
    m.faces.push(MeshFace { v: (0..n).map(|i| base + n + i).collect(), hidden: 0 });
    for i in 0..n {
        let j = (i + 1) % n;
        m.faces.push(MeshFace { v: vec![base + i, base + j, base + n + j, base + n + i], hidden: 0 });
    }
}

/// The walls [`built`] made in the storage room's place, swapped for walls of the shapes under
/// test (each a built wall element: the build's tags, a new mesh); their handles by name.
fn shapes(s: Session) -> (Session, Vec<(&'static str, String)>) {
    let mut d = (*s.doc().unwrap()).clone();
    let template = d.model.iter().find(|e| made_as(e).as_deref() == Some("wall")).cloned().expect("a built wall");
    let EntityKind::Mesh(tm) = &template.kind else { panic!("a built wall is a mesh") };
    let empty = || Mesh { vertices: Vec::new(), faces: Vec::new(), kind: tm.kind };
    let mut out = Vec::new();
    let mut put = |d: &mut Drawing, name: &'static str, m: Mesh| {
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Mesh(m));
        e.common = template.common.clone();
        d.model.push(e);
        out.push((name, h.hex()));
    };
    // An L: a 10' leg along x (6" thick), a 3' leg up from its west end.
    let mut m = empty();
    prism(&mut m, &[[17.0, 3.0], [27.0, 3.0], [27.0, 3.5], [17.5, 3.5], [17.5, 6.0], [17.0, 6.0]], 0.0, 120.0);
    put(&mut d, "L", m);
    // A 12" wall with a 9" deep alcove notched out of its north face (3" left behind it).
    let mut m = empty();
    prism(&mut m, &[[17.0, 7.5], [27.0, 7.5], [27.0, 8.5], [23.0, 8.5], [23.0, 7.75], [21.0, 7.75], [21.0, 8.5], [17.0, 8.5]], 0.0, 120.0);
    put(&mut d, "notched", m);
    // Six corners: a 6" wall with pointed (mitred both ways) ends.
    let mut m = empty();
    prism(&mut m, &[[30.0, 3.0], [38.0, 3.0], [38.25, 3.25], [38.0, 3.5], [30.0, 3.5], [29.75, 3.25]], 0.0, 120.0);
    put(&mut d, "hexagon", m);
    // A wall with a door: a box on the floor each side, the header from 7' up over the door.
    let mut m = empty();
    prism(&mut m, &[[29.0, 6.0], [32.5, 6.0], [32.5, 6.5], [29.0, 6.5]], 0.0, 120.0);
    prism(&mut m, &[[32.5, 6.0], [35.5, 6.0], [35.5, 6.5], [32.5, 6.5]], 84.0, 120.0);
    prism(&mut m, &[[35.5, 6.0], [38.0, 6.0], [38.0, 6.5], [35.5, 6.5]], 0.0, 120.0);
    put(&mut d, "door", m);
    // A box drawn flat: a hundredth of an inch thick.
    let mut m = empty();
    prism(&mut m, &[[36.0, 10.0], [36.001, 10.0], [36.001, 10.5], [36.0, 10.5]], 0.0, 120.0);
    put(&mut d, "sliver", m);
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Floor Plan", None);
    (s2, out)
}

fn handle(names: &[(&str, String)], name: &str) -> String {
    names.iter().find(|(n, _)| *n == name).map(|(_, h)| h.clone()).unwrap_or_else(|| panic!("no wall {name}"))
}

/// The plan receptacle at `p` (feet) and its built twin.
fn receptacle_at(s: &Session, p: [f64; 2]) -> (Handle, Option<Handle>) {
    let d = s.doc().unwrap();
    let q = at(p);
    let plan = d
        .model
        .iter()
        .find(|e| matches!(&e.kind, EntityKind::Insert(i) if (i.insert.x - q[0]).abs() < 0.01 && (i.insert.y - q[1]).abs() < 0.01))
        .map(|e| e.handle)
        .unwrap_or_else(|| panic!("no receptacle at {p:?}"));
    let twin = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("device"))
        .find(|e| {
            let c = cadaddy_engine::doc::entity_bounds(d, e, 0).center();
            (c.x - q[0]).hypot(c.y - q[1]) < 6.0
        })
        .map(|e| e.handle);
    (plan, twin)
}

/// G-HOST's finding on the receptacle at `p`.
fn host_finding(pre: &Value, s: &Session, p: [f64; 2]) -> Value {
    let (plan, twin) = receptacle_at(s, p);
    let ids: Vec<String> = [Some(plan), twin].into_iter().flatten().map(|h| h.hex()).collect();
    pre["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule"] == "G-HOST" && f["handle"].as_str().is_some_and(|h| ids.iter().any(|i| i.eq_ignore_ascii_case(h))))
        .cloned()
        .unwrap_or_else(|| panic!("no G-HOST finding for the receptacle at {p:?}: {pre:#}"))
}

fn prescreen(s: &mut Session) -> Value {
    s.execute("validate.prescreen", &json!({"rules": ["G-HOST"], "all": true, "limit": 50})).unwrap()
}

#[test]
fn every_wall_is_listed_whatever_its_footprint() {
    let (mut s, names) = shapes(built());
    let pre = prescreen(&mut s);
    let walls = &pre["walls"];
    // The sliver is listed, with why; every other shape gives pieces.
    let first = walls["unmeasured"]["first"].as_array().unwrap();
    let sliver = first.iter().find(|u| u["handle"].as_str() == Some(handle(&names, "sliver").as_str())).unwrap_or_else(|| panic!("{walls:#}"));
    assert!(sliver["why"].as_str().unwrap().contains("sliver"), "{sliver}");
    assert_eq!(walls["unmeasured"]["walls"], json!(1), "{walls:#}");
    assert!(walls["over_openings"].as_u64().unwrap() >= 1, "the door's header: {walls:#}");
    // Every built wall element is in the list (as pieces, a span or listed with why).
    let built = s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).count();
    assert_eq!(walls["elements"].as_u64(), Some(built as u64), "{walls:#}");

    // A device on each: on the L's long leg and on its short leg, in the notch, on the
    // six-cornered wall, beside the door, each measured to its own piece's face.
    for (p, name, face) in [
        (ON_L_LONG, "L", "north"),
        (ON_L_SHORT, "L", "east"),
        (IN_NOTCH, "notched", "north"),
        (ON_HEX, "hexagon", "north"),
        (BESIDE_DOOR, "door", "north"),
    ] {
        let f = host_finding(&pre, &s, p);
        assert_eq!(f["verdict"], "pass", "{p:?}: {f:#}");
        assert_eq!(f["measured"]["wall"].as_str(), Some(handle(&names, name).as_str()), "{p:?}: {f:#}");
        assert_eq!(f["measured"]["face"], face, "{p:?}: {f:#}");
        assert!(f["measured"]["off_face_in"].as_f64().unwrap().abs() < 1.0, "{p:?}: {f:#}");
    }
    // The L's legs are 6" thick (the least rectangle round the L would be 3' thick), the
    // alcove's back 3" (the notched wall's thinner piece).
    assert!((host_finding(&pre, &s, ON_L_LONG)["measured"]["thickness_in"].as_f64().unwrap() - 6.0).abs() < 0.01);
    assert!((host_finding(&pre, &s, ON_L_SHORT)["measured"]["thickness_in"].as_f64().unwrap() - 6.0).abs() < 0.01);
    assert!((host_finding(&pre, &s, IN_NOTCH)["measured"]["thickness_in"].as_f64().unwrap() - 3.0).abs() < 0.01);
    assert!((host_finding(&pre, &s, ON_HEX)["measured"]["thickness_in"].as_f64().unwrap() - 6.0).abs() < 0.01);

    // Under the door's header, at 18": on the wall's face line but in the opening, said so.
    let f = host_finding(&pre, &s, UNDER_HEADER);
    assert_eq!(f["verdict"], "fail", "{f:#}");
    assert!(f["issue"].as_str().unwrap().contains("opening"), "{f:#}");
    assert_eq!(f["measured"]["wall"].as_str(), Some(handle(&names, "door").as_str()), "{f:#}");
    assert!((f["measured"]["opening"]["wall_from_aff_in"].as_f64().unwrap() - 84.0).abs() < 0.01, "{f:#}");
    assert!(f["measured"]["off_face_in"].as_f64().unwrap().abs() < 1.0, "{f:#}");

    // The storage room's packet lists the door's header as a wall piece over an opening, and
    // the sliver with why.
    let areas = s.execute("validate.areas", &json!({})).unwrap()["areas"].as_array().cloned().unwrap();
    let storage = areas.iter().find(|a| a["number"] == "102").unwrap_or_else(|| panic!("{areas:?}"));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let out = std::env::temp_dir().join(format!("cadaddy-vwalllist-{}-{nanos}", std::process::id()));
    s.execute("validate.packet", &json!({"area": storage["id"], "out_dir": out.to_string_lossy()})).unwrap();
    let o: Value = serde_json::from_str(&std::fs::read_to_string(out.join("objects.json")).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&out);
    let door = handle(&names, "door");
    let span = o["wall_spans"].as_array().unwrap().iter().find(|w| w["handle"].as_str() == Some(door.as_str())).unwrap_or_else(|| panic!("{o:#}"));
    assert_eq!((span["from_aff_in"].as_f64(), span["to_aff_in"].as_f64()), (Some(84.0), Some(120.0)), "{span}");
    let gone = o["walls_unmeasured"].as_array().unwrap();
    assert!(
        gone.iter()
            .any(|w| w["handle"].as_str() == Some(handle(&names, "sliver").as_str()) && w["why"].as_str().is_some_and(|y| y.contains("sliver"))),
        "{o:#}"
    );
}

#[test]
fn bim_element_on_a_wall_finds_the_wall_itself() {
    let (mut s, names) = shapes(built());
    for name in ["L", "notched", "hexagon", "door"] {
        let h = handle(&names, name);
        let r = s.execute("bim.element", &json!({"handle": h})).unwrap();
        assert_eq!(r["nearest_wall"]["wall"].as_str(), Some(h.as_str()), "{name}: {r:#}");
        assert!(r["nearest_wall"]["off_face_in"].as_f64().unwrap() < 0.0, "inside itself: {name}: {r:#}");
    }
    // The door's wall is measured from its longest piece on the floor, not its box's centre
    // (under the header).
    let r = s.execute("bim.element", &json!({"handle": handle(&names, "door")})).unwrap();
    let a = r["at"].as_array().unwrap().iter().filter_map(Value::as_f64).collect::<Vec<_>>();
    assert!((a[0] - 30.75).abs() < 0.01 && (a[1] - 6.25).abs() < 0.01, "{r:#}");
    let sliver = s.execute("bim.element", &json!({"handle": handle(&names, "sliver")})).unwrap();
    assert!(sliver["wall_unmeasured"].as_str().unwrap().contains("sliver"), "{sliver:#}");
    // A device under the header: in the opening, not hosted.
    let (plan, twin) = receptacle_at(&s, UNDER_HEADER);
    let r = s.execute("bim.element", &json!({"handle": twin.unwrap_or(plan).hex()})).unwrap();
    assert!(r["host"].is_null(), "{r:#}");
    assert_eq!(r["in_opening"]["wall"].as_str(), Some(handle(&names, "door").as_str()), "{r:#}");
}

#[test]
fn g_host_measures_to_the_stored_host_wall_first() {
    let (s, names) = shapes(built());
    let mut d = (*s.doc().unwrap()).clone();
    // The receptacle on the L's long leg records the L as its host; the one on the hexagon a
    // wall the model no longer has; the one beside the door the notched wall (its record is
    // wrong: it is on the door's wall).
    let stale = Handle::parse_hex("FFFFFF").unwrap().hex();
    for (p, host) in [(ON_L_LONG, handle(&names, "L")), (ON_HEX, stale.clone()), (BESIDE_DOOR, handle(&names, "notched"))] {
        let (plan, twin) = receptacle_at(&s, p);
        let target = twin.unwrap_or(plan);
        d.modify_entity(target, |e| {
            let mut store = props::read(&e.common.xdata);
            store.set("host_wall", Prop::new(host.clone(), Source::computed()));
            e.common.xdata = props::write(&e.common.xdata, &store).into();
        })
        .unwrap();
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let pre = prescreen(&mut s);
    let f = host_finding(&pre, &s, ON_L_LONG);
    assert_eq!((f["verdict"].as_str(), f["measured"]["host_from"].as_str()), (Some("pass"), Some("stored")), "{f:#}");
    let f = host_finding(&pre, &s, ON_HEX);
    assert_eq!(f["verdict"], "pass", "{f:#}");
    assert_eq!(f["measured"]["host_from"], "nearest", "{f:#}");
    assert_eq!(f["measured"]["stored_host"]["wall"].as_str(), Some(stale.as_str()), "{f:#}");
    let f = host_finding(&pre, &s, BESIDE_DOOR);
    assert_eq!(f["verdict"], "pass", "on the door's wall: {f:#}");
    assert_eq!(f["measured"]["wall"].as_str(), Some(handle(&names, "door").as_str()), "{f:#}");
    assert_eq!(f["measured"]["stored_host"]["wall"].as_str(), Some(handle(&names, "notched").as_str()), "{f:#}");
    assert!(f["issue"].as_str().unwrap().contains("its record names wall"), "{f:#}");
}
