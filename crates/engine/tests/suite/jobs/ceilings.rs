//! Ceilings from a synthetic reflected ceiling plan: the office's 2×4 grid at 9'-0", the
//! corridor's gypsum ceiling at 8'-0" with a 7'-0" soffit, a 2×4 light and a diffuser centred in
//! their tiles; the RCP lined up with the floor plan; built into the model.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Mesh};
use serde_json::json;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// A block of a `w` × `h` rectangle centred on its base, inserted at `at`.
fn block(s: &mut Session, name: &str, w: f64, h: f64, at: [f64; 2]) {
    if s.doc().unwrap().block(name).is_none() {
        let before: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
        rect(s, [-w / 2.0, -h / 2.0], [w / 2.0, h / 2.0]);
        let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).filter(|h| !before.contains(h)).collect();
        s.execute("block", &json!({"name": name, "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    }
    s.execute("insert", &json!({"name": name, "at": at})).unwrap();
}

/// The walls: a 40' × 20' box, OFFICE west and CORRIDOR east of a 12" wall, at `dy`.
fn walls(s: &mut Session, dy: f64) {
    layer(s, "A-WALL");
    rect(s, [0.0, dy], [480.0, dy + 240.0]);
    rect(s, [6.0, dy + 6.0], [234.0, dy + 234.0]);
    rect(s, [246.0, dy + 6.0], [474.0, dy + 234.0]);
}

/// R = where the RCP is drawn (1500" above the plan).
const R: f64 = 1500.0;

fn drawing() -> Session {
    let mut s = Session::new();
    walls(&mut s, 0.0);
    layer(&mut s, "A-AREA-IDEN");
    text(&mut s, [100.0, 120.0], "OFFICE");
    text(&mut s, [340.0, 120.0], "CORRIDOR");
    // The RCP.
    walls(&mut s, R);
    layer(&mut s, "A-CLNG-GRID");
    for i in 1..=9 {
        let x = 24.0 * f64::from(i);
        line(&mut s, [x, R + 6.0], [x, R + 234.0]);
    }
    for i in 0..4 {
        let y = R + 54.0 + 48.0 * f64::from(i);
        line(&mut s, [6.0, y], [234.0, y]);
    }
    layer(&mut s, "A-CLNG-NOTE");
    text(&mut s, [60.0, R + 120.0], "9'-0\" A.F.F.");
    text(&mut s, [300.0, R + 120.0], "CLG 8'-0\" GYP");
    layer(&mut s, "A-CLNG-SOFT");
    rect(&mut s, [400.0, R + 6.0], [474.0, R + 234.0]);
    s.execute("hatch", &json!({"points": [[430, R + 100.0]], "pattern": "ANSI31"})).unwrap();
    text(&mut s, [410.0, R + 150.0], "SOFFIT 7'-0\"");
    layer(&mut s, "E-LITE");
    block(&mut s, "2X4 LT", 24.0, 48.0, [100.0, R + 100.0]);
    layer(&mut s, "M-HVAC");
    block(&mut s, "SD-1", 12.0, 12.0, [180.0, R + 190.0]);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}, {"layer": "A-AREA-IDEN", "role": "room_tag"}, {"layer": "E-LITE", "role": "fixture"}],
            "rooms": [{"name": "Office"}, {"name": "Corridor"}]
        }}),
    )
    .unwrap();
    s
}

const REGION: [f64; 4] = [-100.0, R - 100.0, 1000.0, R + 500.0];

#[test]
fn the_rcp_gives_heights_grids_soffits_and_fixtures() {
    let mut s = drawing();
    let r = s.execute("rcp.read", &json!({"region": REGION, "level": "L1"})).unwrap();
    assert_eq!(r["offset"], json!([0.0, -R]), "lined up on the walls");
    let ceilings = r["ceilings"].as_array().unwrap();
    assert_eq!(ceilings.len(), 2, "{r}");
    let office = ceilings.iter().find(|c| c["height_ft"] == json!(9.0)).unwrap();
    assert_eq!(office["grid"]["spacing_in"], json!([24.0, 48.0]), "2×4 tiles: {office}");
    let corridor = ceilings.iter().find(|c| c["height_ft"] == json!(8.0)).unwrap();
    assert!(corridor.get("grid").is_none(), "gypsum: {corridor}");
    // The office's area is the room's, on the floor plan.
    let xs: Vec<f64> = office["outline"].as_array().unwrap().iter().map(|p| p[0].as_f64().unwrap()).collect();
    let ys: Vec<f64> = office["outline"].as_array().unwrap().iter().map(|p| p[1].as_f64().unwrap()).collect();
    assert_eq!((xs.iter().cloned().fold(f64::INFINITY, f64::min), xs.iter().cloned().fold(0.0, f64::max)), (6.0, 234.0));
    assert_eq!((ys.iter().cloned().fold(f64::INFINITY, f64::min), ys.iter().cloned().fold(0.0, f64::max)), (6.0, 234.0));
    // The soffit at its note's height.
    assert_eq!(r["soffits"].as_array().unwrap().len(), 1);
    assert_eq!(r["soffits"][0]["height_ft"], json!(7.0));
    // The light and the diffuser, centred in their tiles (the grid crosses at x 216 and y 54 on
    // the plan; tiles 24" along x, 48" along y).
    let items = r["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{r}");
    let light = items.iter().find(|i| i["kind"] == "fixture").unwrap();
    assert_eq!((light["at"].clone(), light["size_in"].clone()), (json!([108.0, 78.0]), json!([24.0, 48.0])), "{light}");
    let diff = items.iter().find(|i| i["kind"] == "diffuser").unwrap();
    assert_eq!(diff["at"], json!([180.0, 174.0]), "{diff}");
    // Applied: the spec has them and the rooms take their heights.
    s.execute("rcp.read", &json!({"region": REGION, "level": "L1", "apply": true})).unwrap();
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    assert_eq!(sp["spec"]["rcp"]["ceilings"].as_array().unwrap().len(), 2);
    let room = |n: &str| sp["spec"]["rooms"].as_array().unwrap().iter().find(|r| r["name"] == n).unwrap()["ceiling_ft"].clone();
    assert_eq!((room("Office"), room("Corridor")), (json!(9.0), json!(8.0)));
    let parts: Vec<String> =
        sp["parts"].as_array().unwrap().iter().filter(|p| p["section"] == "rcp").map(|p| p["text"].as_str().unwrap().to_string()).collect();
    assert!(parts.iter().any(|t| t == "Ceiling 1 (Office) on L1: 24×48\" grid at 9'-0\""), "{parts:?}");
    assert!(parts.iter().any(|t| t == "In the ceilings: 1 light fixtures, 1 diffusers"), "{parts:?}");
    // Errors.
    assert!(s.execute("rcp.read", &json!({"region": REGION})).is_err());
    assert!(s.execute("rcp.read", &json!({"region": REGION, "level": "L9"})).is_err());
    assert!(s.execute("rcp.read", &json!({"region": REGION, "level": "L1", "offset": [1]})).is_err());
}

fn made(s: &Session, part: &str) -> Vec<Mesh> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some(part))
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None })
        .collect()
}

fn z(m: &Mesh) -> (f64, f64) {
    m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)))
}

#[test]
fn model_build_hangs_the_ceilings() {
    let mut s = drawing();
    s.execute("rcp.read", &json!({"region": REGION, "level": "L1", "apply": true})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!([&r["rcpCeilings"], &r["grids"], &r["soffits"], &r["lights"], &r["diffusers"]].map(|v| v.as_u64().unwrap()), [2, 1, 1, 1, 1], "{r}");
    assert_eq!(r["ceilings"], json!(2), "the RCP's ceilings replace the rooms': {r}");
    let mut heights: Vec<f64> = made(&s, "ceiling").iter().map(|m| z(m).0).collect();
    heights.sort_by(f64::total_cmp);
    assert_eq!(heights, vec![96.0, 108.0]);
    // The grid's tees hang 1½" under the 9' ceiling; 9 lines one way, 4 the other.
    let grid = &made(&s, "grid")[0];
    assert_eq!(z(grid), (106.5, 108.0));
    assert_eq!(grid.faces.len(), 6 * (9 + 4), "{}", grid.faces.len());
    // The light 4" deep under the ceiling over its tile; the soffit from 7' up to the ceiling.
    let light = &made(&s, "light")[0];
    assert_eq!(z(light), (104.0, 108.0));
    let x = light.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.x), hi.max(v.x)));
    assert_eq!(x, (96.0, 120.0));
    assert_eq!(z(&made(&s, "soffit")[0]), (84.0, 108.0));
}
