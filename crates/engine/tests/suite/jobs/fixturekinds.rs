//! Q6315 fixturekinds: a built luminaire is a `light_fixture` element and a built plumbing
//! fixture a `plumbing_fixture` element (its kind set when it is made, from what it was read as:
//! its own kind, or its mark in the light fixture schedule), with a 3D body sized from what is
//! drawn, else its schedule's size, else its kind's usual size (rules data), each body saying
//! which and why. The fixture schedules count what was placed. Made-up plans only.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// One made-up room, 6" walls: 0.5–39.5' by 0.5–19.5'.
fn room() -> Drawing {
    let mut d = Drawing::new_imperial();
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let sides = [(0.0, 0.0), (40.0, 0.0), (40.0, 20.0), (0.0, 20.0)];
    let inner = [(0.5, 0.5), (39.5, 0.5), (39.5, 19.5), (0.5, 19.5)];
    for ring in [sides, inner] {
        for i in 0..4 {
            let h = Handle(d.handseed.max(0x100));
            d.handseed = h.0 + 1;
            let mut e = Entity::new(h, EntityKind::Line(Line { a: p(ring[i]), b: p(ring[(i + 1) % 4]) }));
            e.common.layer = "A-WALL".into();
            d.model.push(e);
        }
    }
    d
}

/// A read device: its kind as its reader gave it, its mark, where, and other fields.
fn tag(n: usize, kind: &str, mark: &str, at: [f64; 2], fields: &[(&str, &str, &str)]) -> Value {
    let mut f = json!({
        "type": {"value": mark, "source": "plan"},
        "sheet": {"value": "X-1", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x900 + n), "source": "plan"},
    });
    for (k, v, src) in fields {
        f[*k] = json!({"value": v, "source": src});
    }
    json!({"id": format!("device:L1:{:.1},{:.1}:{mark}-{n}", at[0], at[1]), "kind": kind, "level": "L1", "at_ft": at, "fields": f})
}

/// The room built with two troffers (their own kind `luminaire`), a wall sconce (`wall luminaire`),
/// two downlights read only as devices whose mark the light fixture schedule lists, three
/// plumbing fixtures (a water closet, a lavatory, a tub its schedule sizes, one whose outline the
/// architectural plan draws) and two receptacles.
fn built() -> Session {
    let mut s = Session::empty();
    s.open_drawing(room(), "Made-up Room", None);
    let tags = vec![
        tag(1, "luminaire", "LA", [10.0, 10.0], &[]),
        tag(2, "luminaire", "LA", [20.0, 10.0], &[]),
        tag(3, "wall luminaire", "LW", [30.0, 0.8], &[]),
        tag(4, "", "LB", [12.0, 15.0], &[]),
        tag(5, "device", "LB", [22.0, 15.0], &[]),
        tag(6, "plumbing_fixture", "PX-1", [3.0, 1.2], &[("fixture", "WATER CLOSET, FLOOR MOUNTED", "schedule")]),
        tag(7, "plumbing_fixture", "PX-2", [6.0, 1.2], &[("fixture", "LAVATORY, WALL HUNG", "schedule")]),
        tag(8, "plumbing_fixture", "PX-3", [9.0, 2.0], &[("fixture", "BATHTUB 60X32", "schedule")]),
        tag(9, "plumbing_fixture", "PX-1", [34.0, 3.0], &[("fixture", "WATER CLOSET", "schedule"), ("drawn_size_in", "21, 30", "plan")]),
        tag(10, "receptacle", "", [15.0, 0.8], &[]),
        tag(11, "", "", [25.0, 0.8], &[]),
    ];
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": tags,
        "mep": {"fixture_schedule": [
            {"tag": "LA", "manufacturer": "Maker One", "model": "TRF-24", "description": "2X4 LED TROFFER, LAY-IN", "installation": "RECESSED", "source": "X-0"},
            {"tag": "LB", "manufacturer": "Maker Two", "model": "DL-6", "description": "6\" ROUND LED DOWNLIGHT", "installation": "RECESSED", "source": "X-0"},
            {"tag": "LW", "manufacturer": "Maker Three", "model": "SC-1", "description": "WALL SCONCE", "installation": "WALL", "source": "X-0"},
        ]},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

fn count(r: &Value) -> u64 {
    r["rows"].as_array().unwrap().iter().map(|x| x["count"].as_str().unwrap().parse::<u64>().unwrap()).sum()
}

/// The built elements of `kind` by their type mark: (handle, mark).
fn found(s: &mut Session, kind: &str) -> Vec<(String, String)> {
    let r = s.execute("bim.props.find", &json!({"kind": kind, "limit": 1000})).unwrap();
    let hs: Vec<String> = r["handles"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()).collect();
    hs.into_iter()
        .map(|h| {
            let g = s.execute("bim.props.get", &json!({"handle": h})).unwrap();
            let mark = g["elements"][0]["props"]["type_mark"]["value"].as_str().unwrap_or("").to_string();
            (h, mark)
        })
        .collect()
}

fn props(s: &mut Session, h: &str) -> Value {
    s.execute("bim.props.get", &json!({"handle": h})).unwrap()["elements"][0]["props"].clone()
}

/// Plan size (inches, sorted short side first) and height of an element's body.
fn body(s: &Session, h: &str) -> ([f64; 2], f64) {
    let d = s.doc().unwrap();
    let e = d.entity(Handle::parse_hex(h).unwrap()).unwrap();
    let EntityKind::Mesh(m) = &e.kind else { panic!("{h} is no mesh") };
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for v in &m.vertices {
        for (i, c) in [v.x, v.y, v.z].into_iter().enumerate() {
            lo[i] = lo[i].min(c);
            hi[i] = hi[i].max(c);
        }
    }
    let (a, b) = (hi[0] - lo[0], hi[1] - lo[1]);
    ([a.min(b), a.max(b)], hi[2] - lo[2])
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.6
}

#[test]
fn luminaires_and_plumbing_fixtures_are_their_own_kinds_and_their_schedules_count_them() {
    let mut s = built();
    let lights = found(&mut s, "light_fixture");
    let plumbing = found(&mut s, "plumbing_fixture");
    let devices = found(&mut s, "device");
    let counts = (lights.len(), plumbing.len(), devices.len());
    assert_eq!(counts, (5, 4, 2), "(light_fixture, plumbing_fixture, device) elements");
    assert_eq!(lights.len(), 5, "two troffers, a sconce, two downlights typed by the light fixture schedule: {lights:?}");
    assert_eq!(plumbing.len(), 4, "{plumbing:?}");
    assert_eq!(devices.len(), 2, "only the receptacles stay devices: {devices:?}");
    // The schedules count what was placed, by type.
    let r = s.execute("bim.schedule", &json!({"kind": "light_fixture"})).unwrap();
    assert_eq!(count(&r), 5, "{r}");
    let by_mark = |m: &str| r["rows"].as_array().unwrap().iter().find(|x| x["type_mark"] == m).map(|x| x["count"].clone());
    assert_eq!(by_mark("LA"), Some(json!("2")), "{r}");
    assert_eq!(by_mark("LB"), Some(json!("2")), "{r}");
    assert_eq!(by_mark("LW"), Some(json!("1")), "{r}");
    let r = s.execute("bim.schedule", &json!({"kind": "plumbing_fixture"})).unwrap();
    assert_eq!(count(&r), 4, "{r}");
    let r = s.execute("bim.schedule", &json!({"kind": "device"})).unwrap();
    assert_eq!(count(&r), 7, "the device schedule: the receptacles and the luminaires: {r}");
    // A device the schedule typed says what made it a luminaire.
    let lb = lights.iter().find(|(_, m)| m == "LB").unwrap().0.clone();
    let p = props(&mut s, &lb);
    assert_eq!(p["kind"]["value"], "luminaire", "{p}");
    assert_eq!(p["kind"]["source"]["kind"], "schedule", "{p}");
}

#[test]
fn each_fixture_has_a_body_sized_from_the_documents_or_its_kind_and_says_which() {
    let mut s = built();
    let lights = found(&mut s, "light_fixture");
    let plumbing = found(&mut s, "plumbing_fixture");
    let devices = found(&mut s, "device");
    let of = |v: &[(String, String)], m: &str| v.iter().filter(|(_, x)| x == m).map(|(h, _)| h.clone()).collect::<Vec<_>>();
    // A troffer the schedule says is 2X4: 24" by 48", derived from its schedule row.
    let la = of(&lights, "LA");
    let (plan, h) = body(&s, &la[0]);
    assert!(near(plan[0], 24.0) && near(plan[1], 48.0) && h > 2.0 && h < 12.0, "{plan:?} {h}");
    let p = props(&mut s, &la[0]);
    assert_eq!(p["body_provenance"]["value"], "derived", "{p}");
    assert!(p["body_basis"]["value"].as_str().unwrap().contains("2X4"), "{p}");
    // A downlight: its kind's usual size (no size printed), inferred, its basis said.
    let lb = of(&lights, "LB");
    let (plan, _) = body(&s, &lb[0]);
    assert!(plan[1] <= 10.0 && plan[0] >= 4.0, "{plan:?}");
    let p = props(&mut s, &lb[0]);
    assert_eq!(p["body_provenance"]["value"], "inferred", "{p}");
    assert!(!p["body_basis"]["value"].as_str().unwrap_or("").is_empty(), "{p}");
    // A water closet against its wall: its kind's usual body, on the floor, ~30" high.
    let wc = of(&plumbing, "PX-1");
    assert_eq!(wc.len(), 2);
    let inferred = wc.iter().find(|h| props(&mut s, h)["body_provenance"]["value"] == "inferred").unwrap().clone();
    let (plan, h) = body(&s, &inferred);
    assert!(near(plan[0], 20.0) && near(plan[1], 28.0) && near(h, 30.0), "{plan:?} {h}");
    // The one the architectural plan draws: the drawn outline's size.
    let drawn = wc.iter().find(|h| **h != inferred).unwrap().clone();
    let (plan, _) = body(&s, &drawn);
    assert!(near(plan[0], 21.0) && near(plan[1], 30.0), "{plan:?}");
    assert_eq!(props(&mut s, &drawn)["body_provenance"]["value"], "drawn");
    // A tub its schedule sizes (60X32): derived.
    let tub = of(&plumbing, "PX-3");
    let (plan, _) = body(&s, &tub[0]);
    assert!(near(plan[0], 32.0) && near(plan[1], 60.0), "{plan:?}");
    assert_eq!(props(&mut s, &tub[0])["body_provenance"]["value"], "derived");
    // A lavatory: a body, not a 2x4x4" box.
    let lav = of(&plumbing, "PX-2");
    let (plan, _) = body(&s, &lav[0]);
    assert!(plan[1] >= 16.0, "{plan:?}");
    // A receptacle keeps its box.
    let (plan, _) = body(&s, &devices[0].0);
    assert!(plan[1] <= 4.5, "{plan:?}");
    assert!(props(&mut s, &devices[0].0)["body_provenance"].is_null());
}
