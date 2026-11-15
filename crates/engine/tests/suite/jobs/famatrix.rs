//! Q6647: the fire alarm sequence's outputs need a device or relay and its power on the drawings.
//! A made-up model whose spec holds what the job read off a sequence matrix (a shunt trip of the
//! elevator's power, marked for one input) and an exhaust fan schedule note tying a fan to the fire
//! alarm; one set leaves them with no device (an elevator disconnect with no shunt trip, no relay),
//! the other gives each its device (the panel schedule's shunt-trip breaker and its control power,
//! a relay module drawn for the fan). `rfi.read` raises one RFI per output with no device, and one
//! for a circuit feeding damper actuators whose schedule row carries no load.

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Props, Source};
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag};
use cadaddy_engine::geom::Vec3;
use cadaddy_engine::walk3d::BuildingSpec;
use serde_json::{Value, json};

/// An equipment element as the build makes them (a box, feet; the drawing is in inches).
fn element(d: &mut Drawing, tag: &str, at: [f64; 2]) -> Handle {
    let v = |x: f64, y: f64, z: f64| Vec3 { x: x * 12.0, y: y * 12.0, z: z * 12.0 };
    let (x0, y0, x1, y1) = (at[0], at[1], at[0] + 2.0, at[1] + 2.0);
    let vertices =
        vec![v(x0, y0, 0.0), v(x1, y0, 0.0), v(x1, y1, 0.0), v(x0, y1, 0.0), v(x0, y0, 2.0), v(x1, y0, 2.0), v(x1, y1, 2.0), v(x0, y1, 2.0)];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, ..Default::default() }));
    e.common.layer = "E-EQPM".into();
    let x = vec![
        RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
        RawTag { code: 1000, value: "equipment".into() },
        RawTag { code: 1000, value: "L1".into() },
        RawTag { code: 1000, value: "0".into() },
    ];
    let mut store = Props::default();
    store.set("tag", Prop::new(json!(tag), Source::print("Sample Sheet E-101", None)));
    e.common.xdata = props::write(&x, &store).into();
    d.ensure_layer("E-EQPM");
    d.model.push(e);
    h
}

fn equipment(tag: &str, kind: &str, title: &str, at: [f64; 2], props: Value) -> Value {
    json!({"tag": tag, "kind": kind, "title": title, "level": "L1", "min": [at[0] * 12.0, at[1] * 12.0],
        "max": [at[0] * 12.0 + 24.0, at[1] * 12.0 + 24.0], "bottom_ft": 0.0, "height_ft": 2.0, "properties": props})
}

struct Built {
    disconnect: Handle,
    fan: Handle,
}

/// The made-up building; `devices` gives each output its device.
fn model(devices: bool) -> (Session, Built) {
    let mut d = Drawing::new_imperial();
    let disconnect = element(&mut d, "DS-E9", [10.0, 10.0]);
    let fan = element(&mut d, "EF-9", [40.0, 10.0]);
    element(&mut d, "FSD-9", [60.0, 10.0]);
    let mut eq = vec![
        // The elevator's disconnect: its schedule gives no shunt trip.
        equipment("DS-E9", "disconnect", "ELEVATOR DISCONNECT", [10.0, 10.0], json!({"RATING": "60A 3P"})),
        equipment("EF-9", "exhaust_fan", "EXHAUST FAN", [40.0, 10.0], json!({"REMARKS": "INTERLOCK WITH THE FIRE ALARM CONTROL PANEL"})),
        // A damper and its power box on a circuit whose schedule row carries no load.
        equipment("FSD-9", "fire_smoke_damper", "combination fire/smoke damper", [60.0, 10.0], json!({})),
        equipment("FSD-9-PWR", "damper_power_box", "junction box", [61.0, 10.0], json!({"FEEDS": "FSD-9", "PANEL": "PQ", "CIRCUIT": "12"})),
    ];
    let mut circuits = vec![
        json!({"number": "3", "description": "ELEVATOR", "breaker": "60", "poles": 3, "load_va": 18000.0}),
        json!({"number": "10", "description": "LIGHTS", "breaker": "20", "poles": 1, "load_va": 900.0}),
        json!({"number": "12", "description": "DAMPERS", "breaker": "20", "poles": 1}),
    ];
    if devices {
        // The schedule marks the elevator's breaker shunt trip and gives its control power; a
        // relay module is drawn for the fan; the damper circuit carries its load.
        circuits[0]["protection"] = json!(["ST"]);
        circuits[2]["load_va"] = json!(200.0);
        circuits.push(json!({"number": "14", "description": "SHUNT TRIP POWER", "breaker": "20", "poles": 1, "load_va": 180.0}));
        eq.push(equipment("CR-9", "fa_control_module", "CONTROL MODULE", [43.0, 10.0], json!({"FOR": "EF-9"})));
    }
    let sp = json!({
        "levels": [{"name": "L1", "elevation_ft": 0.0, "height_ft": 10.0}],
        "mep": {
            "equipment": eq,
            "panel_schedules": [{"panel": "PQ", "source": "E-601", "legend": {"ST": "SHUNT TRIP"}, "circuits": circuits}],
            // What the job read off the sequence matrix: the output an input marks.
            "fa_outputs": [{"class": "shunt_trip", "text": "SHUNT TRIP ELEVATOR POWER", "sheet": "FA-001", "inputs": 1}],
        },
    });
    let sp: BuildingSpec = serde_json::from_value(sp).unwrap();
    d.building_spec = Some(sp.to_json()).into();
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Building", None);
    (s, Built { disconnect, fan })
}

fn check<'a>(r: &'a Value, id: &str) -> &'a Value {
    r["requirements"]["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("no check {id}: {}", r["requirements"]))
}

fn items(s: &mut Session, id: &str) -> Vec<Value> {
    let list = s.execute("rfi.list", &json!({"kind": "requirement"})).unwrap();
    list["items"].as_array().unwrap().iter().filter(|i| i["topic"].as_str().is_some_and(|t| t.contains(id))).cloned().collect()
}

#[test]
fn a_matrix_output_with_no_device_on_the_drawings_is_an_rfi() {
    let (mut s, b) = model(false);
    let r = s.execute("rfi.read", &json!({"apply": true})).unwrap();
    let c = check(&r, "FA-OUTPUT-DEVICE");
    assert_eq!(c["ran"], true, "{c}");
    // The shunt trip and the fan's tie: two outputs, both open.
    assert_eq!((c["objects"].as_u64(), c["findings"].as_u64(), c["missing"].as_u64()), (Some(2), Some(2), Some(2)), "{c}");
    let fa = items(&mut s, "FA-OUTPUT-DEVICE");
    assert_eq!(fa.len(), 2, "{fa:?}");
    let shunt = fa.iter().find(|i| i["issue"].as_str().is_some_and(|t| t.contains("SHUNT TRIP ELEVATOR POWER"))).expect("shunt trip item");
    assert_eq!(shunt["severity"], "high", "{shunt}");
    assert!(shunt["issue"].as_str().unwrap().contains("DS-E9") && shunt["issue"].as_str().unwrap().contains("FA-001"), "{shunt}");
    assert!(shunt["proposed"].as_str().unwrap().contains("shunt-trip breaker"), "{shunt}");
    assert!(shunt["sides"].to_string().contains("620.51"), "{shunt}");
    assert_eq!(shunt["elements"], json!([b.disconnect.hex()]), "{shunt}");
    let fan = fa.iter().find(|i| i["issue"].as_str().is_some_and(|t| t.contains("INTERLOCK WITH THE FIRE ALARM"))).expect("fan item");
    assert_eq!(fan["elements"], json!([b.fan.hex()]), "{fan}");
    // The damper circuit's row carries no load where the schedule prints loads.
    let load = check(&r, "FA-ACTUATOR-LOAD");
    assert_eq!((load["objects"].as_u64(), load["findings"].as_u64()), (Some(1), Some(1)), "{load}");
    let li = items(&mut s, "FA-ACTUATOR-LOAD");
    assert!(li.first().is_some_and(|i| i["issue"].as_str().is_some_and(|t| t.contains("circuit 12") && t.contains("no load"))), "{li:?}");
    // Read again: the same items, nothing doubled.
    s.execute("rfi.read", &json!({"apply": true})).unwrap();
    assert_eq!(items(&mut s, "FA-OUTPUT-DEVICE").len(), 2);
}

#[test]
fn outputs_whose_devices_the_drawings_show_raise_nothing() {
    let (mut s, _) = model(true);
    let r = s.execute("rfi.read", &json!({"apply": true})).unwrap();
    let c = check(&r, "FA-OUTPUT-DEVICE");
    assert_eq!((c["objects"].as_u64(), c["met_by_documents"].as_u64(), c["findings"].as_u64()), (Some(2), Some(2), Some(0)), "{c}");
    assert_eq!(check(&r, "FA-ACTUATOR-LOAD")["findings"], 0);
    assert!(items(&mut s, "FA-OUTPUT-DEVICE").is_empty() && items(&mut s, "FA-ACTUATOR-LOAD").is_empty());
}
