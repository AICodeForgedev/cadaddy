//! The deterministic prescreen (Q6124) on a made-up job: two guest rooms and a corridor, a
//! receptacle on its wall and one 2 ft off it, a switch at the wrong height, a PTAC in one guest
//! room only; an enlarged plan drawn turned 180° against the plan, the king unit's mechanical
//! enlarged view, a riser diagram and an interior elevation titled by the unit type.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Mesh, MeshFace, RawTag, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// The plan's origin on the drawing (inches): the level's base.
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
const A401: f64 = 16000.0;
const M401: f64 = 20000.0;
const P501: f64 = 24000.0;
const A501: f64 = 28000.0;
/// The enlarged view's fit onto the plan, turned 180° at half scale: project = −0.5 · sheet + T
/// (the view's centre lands on the middle of guest room 101).
const ENL_TX: f64 = 1293.0 + 0.5 * (A401 + 1300.0);
const ENL_TY: f64 = 1072.0 + 0.5 * 800.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on the plan.
fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let v = [at(a), at([b[0], a[1]]), at(b), at([a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [at(a), at(b)]})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": p, "text": t, "height": 6})).unwrap();
}

/// Block references at building points (feet), on `layer` with a TYPE attribute.
fn place(s: Session, block: &str, layer: &str, pts: &[[f64; 2]], ty: &str) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key(block) {
        let mut b = Block::new(block);
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert(block.into(), Arc::new(b));
    }
    let mut hs = Vec::new();
    for p in pts {
        let attribs = vec![Attrib {
            tag: "TYPE".into(),
            text: Text { value: ty.into(), ..style.clone() },
            invisible: true,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        }];
        let q = at(*p);
        let ins = Insert {
            block: block.into(),
            insert: Vec3::new(q[0], q[1], 0.0),
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
        e.common.layer = layer.into();
        d.model.push(e);
        hs.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, hs)
}

fn spec() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "E-POWR", "role": "device"},
            {"layer": "M-EQPM", "role": "device"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [{"name": "Guest Room", "number": "101"}, {"name": "Guest Room", "number": "102"}, {"name": "Corridor", "number": "103"}],
        "devices": {"by_type": {
            "DUPLEX": {"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]},
            "SW": {"ceiling": false, "aff_ft": 3.0},
            "PTAC": {"ceiling": false, "aff_ft": 1.0}
        }},
        "sheets": [
            {"number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH],
             "registration": {"reference": "A-101", "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}},
            {"number": "A-401", "title": "ENLARGED PLANS", "discipline": "A", "level": "L1", "source": "model", "rect": [A401, 0.0, A401 + SW, SH],
             "view": {"title": "ENLARGED PLAN - GUEST ROOM 101", "rect": [A401 + 200.0, 200.0, A401 + 2400.0, 1400.0]},
             "registration": {"reference": "A-101", "method": "view_fit", "a": -0.5, "b": 0.0, "tx": ENL_TX, "ty": ENL_TY, "matched": 0}},
            {"number": "M-401", "title": "MECHANICAL ENLARGED PLANS", "discipline": "M", "source": "model", "rect": [M401, 0.0, M401 + SW, SH],
             "view": {"title": "ENLARGED PLAN - KING", "rect": [M401 + 200.0, 200.0, M401 + 1800.0, 1400.0]}},
            {"number": "P-501", "title": "PLUMBING RISER DIAGRAMS", "discipline": "P", "source": "model", "rect": [P501, 0.0, P501 + SW, SH]},
            {"number": "A-501", "title": "INTERIOR ELEVATIONS", "discipline": "A", "source": "model", "rect": [A501, 0.0, A501 + SW, SH]}
        ]
    })
}

struct Job {
    s: Session,
    on_wall: String,
    off_wall: String,
    switch: String,
    ptac: String,
}

fn job() -> Job {
    let mut s = Session::new();
    // A 100' × 12' shell of 6" walls; partitions at 15' and 30'.
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [100.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [99.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [15.0, 0.5], [15.0, 11.5]);
    line(&mut s, [30.0, 0.5], [30.0, 11.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([5.0, 6.0]), "101");
    text(&mut s, at([20.0, 6.0]), "102");
    text(&mut s, at([60.0, 6.0]), "103");
    // The other sheets: frames, the mechanical view, the riser diagram and the elevation titles.
    layer(&mut s, "G-FRAME");
    for x in [0.0, A401, M401, P501, A501] {
        s.execute("pline", &json!({"vertices": [[x, 0.0], [x + SW, 0.0], [x + SW, SH], [x, SH]], "closed": true})).unwrap();
    }
    layer(&mut s, "M-ENLG");
    s.execute(
        "pline",
        &json!({"vertices": [[M401 + 400.0, 400.0], [M401 + 1600.0, 400.0], [M401 + 1600.0, 1200.0], [M401 + 400.0, 1200.0]], "closed": true}),
    )
    .unwrap();
    layer(&mut s, "P-RISR");
    s.execute("line", &json!({"points": [[P501 + 1000.0, 600.0], [P501 + 1000.0, 1600.0]]})).unwrap();
    text(&mut s, [P501 + 1000.0, 500.0], "KING SANITARY RISER");
    layer(&mut s, "A-ELEV");
    text(&mut s, [A501 + 1000.0, 600.0], "KING - NORTH");
    // The model: in room 101 a receptacle on the south wall's face, one 2 ft off the north
    // wall, a switch; in room 102 the king layout turned 180° (receptacle by the north wall near
    // the east end, PTAC by the south wall near the west end).
    let (s, recs) = place(s, "RECEP", "E-POWR", &[[10.0, 0.5], [6.0, 9.5], [27.0, 11.5]], "DUPLEX");
    let (s, sws) = place(s, "SWITCH", "E-POWR", &[[14.5, 6.0]], "SW");
    let (mut s, ptacs) = place(s, "PTACU", "M-EQPM", &[[18.0, 0.5]], "PTAC");
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    for (h, kind) in [(&recs[0], "receptacle"), (&recs[1], "receptacle"), (&recs[2], "receptacle"), (&sws[0], "switch"), (&ptacs[0], "ptac")] {
        s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": "E-101", "kind": kind})).unwrap();
    }
    // The king unit type (its layout as read from its enlarged plan) and room 102 its instance.
    let mut sp: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    sp["unit_types"] = json!([{"id": "ut-king", "tag": "KING", "name": "KING", "width_ft": 14.5, "depth_ft": 11.0,
        "items": [{"category": "devices", "kind": "receptacle", "u": 0.2, "v": 0.0}, {"category": "equipment", "kind": "ptac", "u": 0.8, "v": 1.0}],
        "source": {"sheet": "M-401"}}]);
    sp["unit_instances"] = json!([{"type": "ut-king", "room": "102", "number": "102", "level": "L1", "hand": "normal", "at_ft": [15.0, 0.5]}]);
    s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    Job { s, on_wall: recs[0].clone(), off_wall: recs[1].clone(), switch: sws[0].clone(), ptac: ptacs[0].clone() }
}

fn rule(s: &mut Session, domain: &str, key: &str, value: &str) -> Result<Value, String> {
    s.execute("project.rules.set", &json!({"domain": domain, "key": key, "value": value, "reason": "made-up test rule"})).map_err(|e| e.to_string())
}

fn findings(r: &Value) -> Vec<Value> {
    r["findings"].as_array().cloned().unwrap_or_default()
}

fn of<'a>(fs: &'a [Value], rule: &str, handle: &str) -> Option<&'a Value> {
    fs.iter().find(|f| f["rule"] == rule && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(handle)))
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-prescreen-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

fn area_of(s: &mut Session, number: &str) -> String {
    let r = s.execute("validate.areas", &json!({})).unwrap();
    let all = r["areas"].as_array().cloned().unwrap_or_default();
    let a = all.iter().find(|a| a["number"] == number).unwrap_or_else(|| panic!("no room {number} in {all:?}"));
    a["id"].as_str().unwrap().to_string()
}

#[test]
fn a_device_two_feet_off_its_wall_fails_g_host_with_the_distance() {
    let mut j = job();
    let r = j.s.execute("validate.prescreen", &json!({"room": "101", "all": true, "limit": 50})).unwrap();
    let fs = findings(&r);
    let off = of(&fs, "G-HOST", &j.off_wall).unwrap_or_else(|| panic!("no G-HOST reading for the floating receptacle: {r}"));
    assert_eq!(off["verdict"], "fail", "{off}");
    let d = off["measured"]["off_face_in"].as_f64().unwrap();
    assert!((d - 24.0).abs() < 1.5, "2 ft off the north wall's face: {off}");
    assert_eq!(off["measured"]["face"], "south", "{off}");
    assert_eq!(off["source"], "prescreen");
    assert_eq!(off["confidence"], 1.0);
    assert_eq!(off["severity"], "high");
    assert!(off["area"].as_str().unwrap().contains("101"), "{off}");
    let on = of(&fs, "G-HOST", &j.on_wall).unwrap_or_else(|| panic!("no G-HOST reading for the wall receptacle: {r}"));
    assert_eq!(on["verdict"], "pass", "{on}");
    assert!(on["measured"]["off_face_in"].as_f64().unwrap().abs() < 1.0, "{on}");
    assert_eq!(on["measured"]["face"], "north", "{on}");
    // Without `all` only the problems; the counts say what was checked.
    let r = j.s.execute("validate.prescreen", &json!({"room": "101"})).unwrap();
    assert!(findings(&r).iter().all(|f| f["verdict"] != "pass"), "{r}");
    assert!(r["checked"]["G-HOST"]["pass"].as_u64().unwrap() >= 1, "{r}");
    assert!(r["checked"]["G-HOST"]["fail"].as_u64().unwrap() >= 1, "{r}");
    // The project's tolerance decides: 30 in lets the floating receptacle pass.
    j.s.execute("validate.rules.set", &json!({"id": "G-HOST", "tolerance": {"plan_in": 30}, "reason": "made-up test"})).unwrap();
    let r = j.s.execute("validate.prescreen", &json!({"room": "101", "all": true, "limit": 50})).unwrap();
    assert_eq!(of(&findings(&r), "G-HOST", &j.off_wall).unwrap()["verdict"], "pass", "{r}");
    // Disabled: not checked.
    j.s.execute("validate.rules.set", &json!({"id": "G-HOST", "enabled": false, "reason": "made-up test"})).unwrap();
    let r = j.s.execute("validate.prescreen", &json!({"room": "101", "all": true, "limit": 50})).unwrap();
    assert!(findings(&r).iter().all(|f| f["rule"] != "G-HOST"), "{r}");
    assert!(r["disabled"].as_array().unwrap().iter().any(|x| x == "G-HOST"), "{r}");
}

/// A built device: a box the build made from plan object `src`, 4" wide, 2" deep and 4" tall,
/// its back at building point `p` (feet), facing +y or −y, its bottom `z` inches above the floor.
fn built_device(s: Session, src: &str, p: [f64; 2], facing_north: bool, z: f64) -> (Session, String) {
    let mut d = (*s.doc().unwrap()).clone();
    let q = at(p);
    let (y0, y1) = if facing_north { (q[1], q[1] + 2.0) } else { (q[1] - 2.0, q[1]) };
    let vertices: Vec<Vec3> = [(q[0] - 2.0, y0), (q[0] + 2.0, y0), (q[0] + 2.0, y1), (q[0] - 2.0, y1)]
        .iter()
        .flat_map(|(x, y)| [Vec3::new(*x, *y, z), Vec3::new(*x, *y, z + 4.0)])
        .collect();
    let faces = vec![
        MeshFace { v: vec![0, 2, 4, 6], hidden: 0 },
        MeshFace { v: vec![1, 3, 5, 7], hidden: 0 },
        MeshFace { v: vec![0, 1, 3, 2], hidden: 0 },
        MeshFace { v: vec![4, 5, 7, 6], hidden: 0 },
    ];
    let h = d.new_handle();
    let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, kind: Default::default() }));
    let tag = |v: &str| RawTag { code: 1000, value: v.into() };
    e.common.xdata = vec![RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }, tag("device"), tag("L1"), tag(src)].into();
    e.common.layer = "3D-Devices".into();
    d.model.push(e);
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, h.hex())
}

#[test]
fn bim_element_says_where_an_element_is_and_its_host_wall() {
    let Job { s, on_wall, off_wall, .. } = job();
    // The built elements of the two receptacles: one at the south wall's face, one 2 ft off the
    // north wall.
    let (s, on) = built_device(s, &on_wall, [10.0, 0.5], true, 16.0);
    let (mut s, off) = built_device(s, &off_wall, [6.0, 9.5], true, 16.0);
    let r = s.execute("bim.element", &json!({"handle": on})).unwrap();
    let at: Vec<f64> = r["at"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!((at[0] - 10.0).abs() < 0.6 && (at[1] - 0.5).abs() < 0.6, "the receptacle on the south wall: {r}");
    let (bot, top, aff) = (r["bottom_ft"].as_f64().unwrap(), r["top_ft"].as_f64().unwrap(), r["aff_in"].as_f64().unwrap());
    assert!(top > bot, "{r}");
    assert!((aff - 16.0).abs() < 0.05 && (aff - bot * 12.0).abs() < 0.05, "level L1's floor is at 0: {r}");
    assert!(r["center_aff_in"].as_f64().unwrap() > aff && r["top_aff_in"].as_f64().unwrap() > aff, "{r}");
    assert!(r["host"].is_object(), "on its wall: {r}");
    assert!(r["host"]["wall"].as_str().is_some_and(|w| !w.is_empty()), "{r}");
    assert_eq!(r["host"]["face"], "north", "{r}");
    let r = s.execute("bim.element", &json!({"handle": off})).unwrap();
    assert!(r["host"].is_null(), "2 ft off its wall it is not hosted: {r}");
    let near = r["nearest_wall"]["off_face_in"].as_f64().unwrap();
    assert!((near - 24.0).abs() < 2.0, "{r}");
}

#[test]
fn a_heights_rule_from_the_project_fails_a_switch_at_the_wrong_height() {
    let mut j = job();
    rule(&mut j.s, "mounting_height", "switch/general", "48\"").unwrap();
    rule(&mut j.s, "mounting_height", "receptacle/guest room", "18").unwrap();
    let r = j.s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": ["G-HEIGHT"]})).unwrap();
    let fs = findings(&r);
    let sw = of(&fs, "G-HEIGHT", &j.switch).unwrap_or_else(|| panic!("no G-HEIGHT reading for the switch: {r}"));
    assert_eq!(sw["verdict"], "fail", "{sw}");
    assert!((sw["measured"]["aff_in"].as_f64().unwrap() - 36.0).abs() < 0.5, "{sw}");
    assert!((sw["measured"]["expected_in"].as_f64().unwrap() - 48.0).abs() < 1e-6, "{sw}");
    assert!(sw["measured"]["expected_from"].as_str().unwrap().contains("switch/general"), "{sw}");
    let rec = of(&fs, "G-HEIGHT", &j.on_wall).unwrap();
    assert_eq!(rec["verdict"], "pass", "{rec}");
    assert!(rec["measured"]["expected_from"].as_str().unwrap().contains("receptacle/guest room"), "{rec}");
    assert!(fs.iter().all(|f| f["rule"] == "G-HEIGHT"), "only the rule asked for");
}

#[test]
fn a_room_types_expected_set_finds_the_missing_ptac() {
    let mut j = job();
    // Refused: a set that doesn't read, or in the wrong domain.
    for (dom, val) in [("custom", "ptac"), ("custom", "ptac=lots"), ("custom", "ptac@roof=1"), ("tolerance", "ptac=1")] {
        assert!(rule(&mut j.s, dom, "prescreen.expect.guest room", val).is_err(), "{dom} {val}");
    }
    rule(&mut j.s, "custom", "prescreen.expect.guest room", "ptac=1; receptacle=1+; panel@wall=1").unwrap();
    let r = j.s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": ["G-PRESENT"]})).unwrap();
    let fs = findings(&r);
    let present = |room: &str, item: &str| {
        fs.iter()
            .find(|f| f["rule"] == "G-PRESENT" && f["area"].as_str().is_some_and(|a| a.ends_with(room)) && f["measured"]["item"] == item)
            .unwrap_or_else(|| panic!("no {item} reading in {room}: {r}"))
            .clone()
    };
    let missing = present("101", "ptac=1");
    assert_eq!(missing["verdict"], "missing", "{missing}");
    assert_eq!(missing["measured"]["count"], 0);
    assert_eq!(missing["expected"], "1 ptac");
    assert!(missing["issue"].as_str().unwrap().contains("guest room"), "{missing}");
    let there = present("102", "ptac=1");
    assert_eq!(there["verdict"], "pass", "{there}");
    assert!(there["measured"]["handles"].as_array().unwrap().iter().any(|h| h.as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.ptac))), "{there}");
    assert_eq!(there["measured"]["unit_type"], "ut-king");
    assert_eq!(present("101", "receptacle=1+")["verdict"], "pass");
    assert_eq!(present("101", "panel@wall=1")["verdict"], "missing");
    // The corridor has no set: said, not failed.
    assert!(r["rooms_without_set"].as_array().unwrap().iter().any(|n| n.as_str().is_some_and(|n| n.eq_ignore_ascii_case("corridor"))), "{r}");
    // The unit type's own set wins over the room name's when its key is longer.
    rule(&mut j.s, "custom", "prescreen.expect.king", "ptac=2").unwrap();
    rule(&mut j.s, "custom", "prescreen.expect.guest room king", "ptac=2").unwrap();
    let r = j.s.execute("validate.prescreen", &json!({"room": "101", "rules": ["G-PRESENT"]})).unwrap();
    assert!(findings(&r).iter().any(|f| f["measured"]["item"] == "ptac=1"), "room 101 keeps the guest room set: {r}");
}

#[test]
fn a_packet_carries_the_prescreen_and_the_enlarged_views_turn() {
    let mut j = job();
    rule(&mut j.s, "custom", "prescreen.expect.guest room", "ptac=1").unwrap();
    let a101 = area_of(&mut j.s, "101");
    let out = dir("p101");
    j.s.execute("validate.packet", &json!({"area": a101, "out_dir": out.to_string_lossy()})).unwrap();
    let m = read_json(&out.join("manifest.json"));
    // The enlarged view drawn turned 180°: the crop says so.
    let enl = m["prints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["sheet"] == "A-401" && c["kind"] == "enlarged_plan")
        .unwrap_or_else(|| panic!("no enlarged plan crop: {m}"));
    assert!((enl["orientation"]["rotation_deg"].as_f64().unwrap() - 180.0).abs() < 0.05, "{enl}");
    assert_eq!(enl["orientation"]["mirrored"], false);
    assert!((enl["orientation"]["scale"].as_f64().unwrap() - 0.5).abs() < 1e-6, "{enl}");
    let prompt = std::fs::read_to_string(out.join("prompt.md")).unwrap();
    assert!(prompt.contains("turned 180°"), "{prompt}");
    assert!(prompt.contains("`prescreen`"), "{prompt}");
    // objects.json: the measured checks of the area.
    let o = read_json(&out.join("objects.json"));
    let pre = o["prescreen"]["findings"].as_array().unwrap();
    let off = pre.iter().find(|f| f["rule"] == "G-HOST" && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.off_wall))).unwrap();
    assert_eq!(off["verdict"], "fail");
    assert!(pre.iter().any(|f| f["rule"] == "G-PRESENT" && f["verdict"] == "missing"), "{}", o["prescreen"]);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn a_typical_rooms_packet_has_its_types_views_turned_as_the_room_is() {
    let mut j = job();
    let a102 = area_of(&mut j.s, "102");
    let out = dir("p102");
    j.s.execute("validate.packet", &json!({"area": a102, "out_dir": out.to_string_lossy()})).unwrap();
    let m = read_json(&out.join("manifest.json"));
    let prints = m["prints"].as_array().unwrap();
    let mech = prints.iter().find(|c| c["sheet"] == "M-401").unwrap_or_else(|| panic!("no mechanical enlarged view of the type: {m}"));
    assert_eq!(mech["kind"], "enlarged_plan");
    assert_eq!(mech["placed_by"], "unit_type");
    assert_eq!(mech["trade"], "mechanical");
    // Room 102 holds the king layout turned 180°.
    assert_eq!(mech["orientation"]["by"], "layout_fit", "{mech}");
    assert!((mech["orientation"]["rotation_deg"].as_f64().unwrap() - 180.0).abs() < 1e-6, "{mech}");
    assert_eq!(mech["orientation"]["mirrored"], false);
    assert_eq!(mech["orientation"]["unit_type"], "ut-king");
    assert!(mech["orientation"]["fit_ft"].as_f64().unwrap() < 1.0, "{mech}");
    // The riser diagram naming the type, and the interior elevation titled by it.
    let riser = prints.iter().find(|c| c["sheet"] == "P-501").unwrap_or_else(|| panic!("no riser diagram: {m}"));
    assert_eq!(riser["kind"], "riser_diagram");
    assert!(riser["shows"].as_str().unwrap().contains("KING"), "{riser}");
    assert!(prints.iter().any(|c| c["sheet"] == "A-501" && c["kind"] == "interior_elevation"), "{m}");
    let o = read_json(&out.join("objects.json"));
    assert_eq!(o["unit"]["type"], "ut-king");
    assert_eq!(o["unit"]["layout_fit"]["rotation_deg"], 180.0);
    // Mirrored instead: the hand says so where the fit can't (no layout).
    let mut sp: Value = serde_json::from_str(j.s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    sp["unit_instances"][0]["hand"] = json!("mirrored");
    sp["unit_types"][0]["items"] = json!([]);
    j.s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    let out2 = dir("p102m");
    j.s.execute("validate.packet", &json!({"area": a102, "out_dir": out2.to_string_lossy()})).unwrap();
    let m = read_json(&out2.join("manifest.json"));
    let mech = m["prints"].as_array().unwrap().iter().find(|c| c["sheet"] == "M-401").unwrap().clone();
    assert_eq!(mech["orientation"]["mirrored"], true, "{mech}");
    assert_eq!(mech["orientation"]["by"], "unit hand", "{mech}");
    let prompt = std::fs::read_to_string(out2.join("prompt.md")).unwrap();
    assert!(prompt.contains("mirrored"), "{prompt}");
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&out2);
}

#[test]
fn the_prescreen_is_deterministic_paged_written_and_in_the_validate_run() {
    let mut j = job();
    rule(&mut j.s, "custom", "prescreen.expect.guest room", "ptac=1; receptacle=1+").unwrap();
    rule(&mut j.s, "mounting_height", "switch", "48").unwrap();
    let a = j.s.execute("validate.prescreen", &json!({"all": true, "limit": 50})).unwrap();
    let b = j.s.execute("validate.prescreen", &json!({"all": true, "limit": 50})).unwrap();
    assert_eq!(a["findings"], b["findings"]);
    assert_eq!(a["scope"]["scope"], "building");
    assert!(a["ms"].as_u64().is_some());
    // Paged: the pages put together are the whole list.
    let total = a["total"].as_u64().unwrap() as usize;
    assert!(total >= 6, "{a}");
    let mut seen = Vec::new();
    let mut cursor = 0;
    loop {
        let r = j.s.execute("validate.prescreen", &json!({"all": true, "limit": 2, "cursor": cursor})).unwrap();
        assert!(r.to_string().len() <= 16_000);
        seen.extend(findings(&r).into_iter().map(|f| f["id"].clone()));
        match r["next"]["cursor"].as_u64() {
            Some(c) => cursor = c as usize,
            None => break,
        }
    }
    assert_eq!(seen, findings(&a).iter().map(|f| f["id"].clone()).collect::<Vec<_>>());
    // Every finding into a file, through the output guard.
    let d = dir("file");
    std::fs::create_dir_all(&d).unwrap();
    let path = d.join("prescreen.json");
    j.s.execute("validate.prescreen", &json!({"path": path.to_string_lossy()})).unwrap();
    let written = read_json(&path);
    assert_eq!(
        written["findings"].as_array().unwrap().len(),
        j.s.execute("validate.prescreen", &json!({})).unwrap()["total"].as_u64().unwrap() as usize
    );
    assert!(j.s.execute("validate.prescreen", &json!({"path": path.to_string_lossy()})).is_err(), "an existing file needs force");
    j.s.execute("validate.prescreen", &json!({"path": path.to_string_lossy(), "force": true})).unwrap();
    // The Validate run's pre-screen counts them.
    let run = j.s.execute("validate", &json!({"scope": "level", "level": "L1", "out_dir": d.join("run").to_string_lossy()})).unwrap();
    let measured = &run["prescreen"]["measured"];
    assert_eq!(measured["call"], "validate.prescreen", "{run}");
    assert!(measured["problems"].as_u64().unwrap() >= 2, "{run}");
    assert!(measured["checked"]["G-PRESENT"]["missing"].as_u64().unwrap() >= 1, "{run}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn bad_prescreen_calls_are_refused_and_never_panic() {
    let mut j = job();
    for p in [
        json!({"rules": ["G-NOPE"]}),
        json!({"rules": "G-HOST"}),
        json!({"limit": -1}),
        json!({"room": "999"}),
        json!({"level": "L9"}),
        json!({"scope": "galaxy"}),
        json!({"area": "nowhere"}),
    ] {
        assert!(j.s.execute("validate.prescreen", &p).is_err(), "{p}");
    }
    for p in [
        json!({"limit": 1_000_000_000u64}),
        json!({"cursor": 1e300}),
        json!({"all": "yes"}),
        json!({"area": {"area": [0, 0, 1e308, 1e308], "level": "L1"}}),
    ] {
        let _ = j.s.execute("validate.prescreen", &p);
    }
    // No model at all.
    let mut s = Session::new();
    assert!(s.execute("validate.prescreen", &json!({})).is_err());
}
