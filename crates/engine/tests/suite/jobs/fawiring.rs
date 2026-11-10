//! Q6503 fawiring: a fire alarm system drawn as devices only is wired as the field wires it:
//! every notification appliance and initiating device traces through its circuit's cable to the
//! control panel, and the cable is taken off. A made-up building only.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

fn fa(x: f64, y: f64, mark: &str, fa_kind: &str, extra: Value) -> Value {
    let mut fields = json!({
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("fire alarm device {mark}"), "source": "plan"},
        "fa_kind": {"value": fa_kind, "source": "legend"},
        "sheet": {"value": "FA-902", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x3B0 + (x * 10.0 + y * 1000.0) as u64), "source": "plan"},
    });
    if let (Some(f), Some(x)) = (fields.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            f.insert(k.clone(), json!({"value": v, "source": "plan"}));
        }
    }
    json!({"id": format!("device:L1:{x:.1},{y:.1}"), "kind": "fire_alarm_device", "level": "L1", "at_ft": [x, y], "fields": fields})
}

/// A made-up building: an electrical room with the control panel and an office; ten 177 cd
/// horn/strobes (two NACs on the panel's outputs), six smoke detectors and two pull stations
/// (one SLC).
fn building() -> Session {
    let mut d = Drawing::new_imperial();
    for (a, b) in [
        ((0., 0.), (60., 0.)),
        ((60., 0.), (60., 30.)),
        ((60., 30.), (0., 30.)),
        ((0., 30.), (0., 0.)),
        ((0.5, 0.5), (9.75, 0.5)),
        ((9.75, 0.5), (9.75, 29.5)),
        ((9.75, 29.5), (0.5, 29.5)),
        ((0.5, 29.5), (0.5, 0.5)),
        ((10.25, 0.5), (59.5, 0.5)),
        ((59.5, 0.5), (59.5, 29.5)),
        ((59.5, 29.5), (10.25, 29.5)),
        ((10.25, 29.5), (10.25, 0.5)),
    ] {
        line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Building", None);
    s.execute("layer.new", &json!({"name": "A-ROOM-TAG", "current": true})).expect("layer");
    for (at, t) in [([40.0, 190.0], "ELECTRICAL"), ([46.0, 176.0], "101"), ([300.0, 190.0], "OFFICE"), ([306.0, 176.0], "102")] {
        s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
    }
    let mut tags: Vec<Value> =
        (0..10).map(|i| fa(15.0 + 4.0 * f64::from(i), 15.0, "HS", "horn/strobe", json!({"mount": "ceiling", "candela": "177cd"}))).collect();
    tags.extend((0..6).map(|i| fa(15.0 + 8.0 * f64::from(i), 25.0, "S", "smoke detector", json!({"mount": "ceiling"}))));
    tags.push(fa(10.8, 2.0, "P", "pull station", json!({"mount_height_in": "48"})));
    tags.push(fa(59.2, 15.0, "P", "pull station", json!({"mount_height_in": "48"})));
    let sp = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": tags,
        "mep": {"equipment": [{"tag": "FACP", "kind": "fire alarm panel", "level": "L1", "min": [6.0, 120.0], "max": [30.0, 126.0],
                               "bottom_ft": 3.5, "height_ft": 2.5, "title": "CONTROL PANEL", "properties": {"FA_KIND": "control panel"}}]},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

#[test]
fn every_fire_alarm_device_reaches_its_panel_and_the_cable_is_in_the_takeoff() {
    let mut s = building();
    let found = s.execute("bim.props.find", &json!({"kind": "device", "where": {"kind": "fire_alarm_device"}, "limit": 100})).expect("find");
    let devices: Vec<String> = found["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    assert_eq!(devices.len(), 18, "{found}");
    let mut open = Vec::new();
    for h in &devices {
        let c = s.execute("bim.chain", &json!({"handle": h})).expect("chain");
        if !c["note"].is_null() || c["source"]["tag"] != json!("FACP") {
            open.push(c);
        }
    }
    assert!(open.is_empty(), "{} of {} devices reach no panel: {:?}", open.len(), devices.len(), open.first());
    let tk = s.execute("conduit.takeoff", &json!({"price": false})).expect("takeoff");
    let ft: f64 = tk["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["kind"] == json!("cable") && l["item"].as_str().is_some_and(|i| i.starts_with("Fire alarm cable")))
        .filter_map(|l| l["qty"].as_f64())
        .sum();
    assert!(ft > 100.0, "the fire alarm cable is taken off: {ft} ft");
}
