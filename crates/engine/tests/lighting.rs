//! Lights and the sun of rendered views: LIGHT and its kinds, light.set, LIGHTLIST,
//! SUNPROPERTIES, RENDEREXPOSURE (and SUNSTATUS), and what the drawing file keeps.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::lights::lights_of;
use cadaddy_engine::doc::lighting::LightKind;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::json;

fn handle(v: &serde_json::Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

#[test]
fn lights_by_json_and_at_the_command_line() {
    let mut s = Session::new();
    let p = handle(&s.execute("light", &json!({"position": [10, 20, 96], "intensity": 800, "color": 2})).unwrap());
    let l = lights_of(&s, &[p]).pop().unwrap();
    assert_eq!((l.kind, l.intensity, l.position.z), (LightKind::Point, 800.0, 96.0));
    assert_eq!(l.name, "Pointlight1");
    let spot = handle(&s.execute("spotlight", &json!({"position": [0, 0, 100], "target": [0, 0, 0], "hotspot": 30, "falloff": 40})).unwrap());
    let sl = lights_of(&s, &[spot]).pop().unwrap();
    assert_eq!((sl.kind, sl.hotspot, sl.falloff), (LightKind::Spot, 30.0, 40.0));
    // Typed: the kind, then where it is and where it points.
    s.script("light\nspot\n5,5\n5,0\n").unwrap();
    let last = s.doc().unwrap().model.iter().last().unwrap().clone();
    let EntityKind::Light(t) = &last.kind else { panic!("not a light") };
    assert_eq!((t.kind, t.position.x, t.target.y), (LightKind::Spot, 5.0, 0.0));
    // Changed through light.set; listed.
    s.execute("light.set", &json!({"handles": [p.hex()], "on": false, "intensity": 50})).unwrap();
    let l = lights_of(&s, &[p]).pop().unwrap();
    assert_eq!((l.on, l.intensity), (false, 50.0));
    let list = s.execute("lightlist", &json!({})).unwrap();
    assert_eq!(list["lights"].as_array().map(Vec::len), Some(3));
    // Bad input.
    assert!(s.execute("light", &json!({})).is_err(), "no position");
    assert!(s.execute("light", &json!({"position": [0, 0], "kind": "laser"})).is_err());
    assert!(s.execute("light", &json!({"position": [0, "x"]})).is_err());
    assert!(s.execute("light", &json!({"position": [0, 0], "color": 999})).is_err());
    assert!(s.execute("pointlight", &json!({"position": [0, 0], "kind": "spot"})).is_err());
    // Hostile numbers are made safe.
    let odd = handle(&s.execute("light", &json!({"position": [0, 0], "intensity": 1e300, "hotspot": -5})).unwrap());
    let o = lights_of(&s, &[odd]).pop().unwrap();
    assert!(o.intensity <= 1e9 && o.hotspot >= 0.0);
}

#[test]
fn the_sun_and_exposure() {
    let mut s = Session::new();
    let r = s.execute("sunproperties", &json!({"date": "2026-09-22", "time": "12:00", "latitude": 40, "longitude": 0, "timezone": 0})).unwrap();
    let el = r["elevation"].as_f64().unwrap();
    assert!((el - 50.0).abs() < 1.5, "{r}");
    assert_eq!((r["date"].clone(), r["time"].clone()), (json!("2026-09-22"), json!("12:00")));
    assert!(s.execute("sunproperties", &json!({"date": "2026-13-01"})).is_err());
    assert!(s.execute("sunproperties", &json!({"time": "25:00"})).is_err());
    assert!(s.execute("sunproperties", &json!({"latitude": 91})).is_err());
    // SUNSTATUS and RENDEREXPOSURE as variables and commands.
    s.execute("setvar", &json!({"name": "SUNSTATUS", "value": 0})).unwrap();
    assert!(!s.doc().unwrap().sun.on);
    assert_eq!(s.execute("renderexposure", &json!({"exposure": 1.5})).unwrap()["exposure"], json!(1.5));
    assert_eq!(s.execute("getvar", &json!({"name": "RENDEREXPOSURE"})).unwrap()["value"], json!(1.5));
    assert!(s.execute("renderexposure", &json!({"exposure": 99})).is_err());
    // Kept with the drawing, as are the lights.
    s.execute("light", &json!({"position": [1, 2, 3], "kind": "spot", "target": [1, 2, 0], "name": "Desk"})).unwrap();
    let d = s.doc().unwrap().clone();
    for name in ["x.dxf", "x.dwg"] {
        let back = cadaddy_io::read(&cadaddy_io::write(&d, name).unwrap(), name).unwrap();
        if name == "x.dxf" {
            assert_eq!(back.sun, d.sun, "{name}");
        }
        let lights: Vec<_> = back.model.iter().filter_map(|e| if let EntityKind::Light(l) = &e.kind { Some(l.clone()) } else { None }).collect();
        assert_eq!(lights.len(), 1, "{name}");
        assert_eq!((lights[0].name.as_str(), lights[0].kind), ("Desk", LightKind::Spot), "{name}");
    }
}
