//! Door hardware devices on the door's latch side (Q6605): a card reader or a door holder drawn
//! at the hinge jamb is built on the latch side (a holder where the open leaf's edge rests), its
//! drawn spot and the rule kept on it.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Arc, Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn push(d: &mut Drawing, layer: &str, kind: EntityKind) {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, kind);
    e.common.layer = layer.into();
    d.model.push(e);
}

fn line_in(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    push(d, layer, EntityKind::Line(Line { a: Vec3 { x: a.0, y: a.1, z: 0.0 }, b: Vec3 { x: b.0, y: b.1, z: 0.0 } }));
}

fn wall(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    line_in(d, "A-WALL", (a.0 * 12.0, a.1 * 12.0), (b.0 * 12.0, b.1 * 12.0));
}

/// One room 20' x 10' (made up), a 3' door in its north wall between x = 8' and 11': hinged at the east jamb (11'), shut against the
/// west (latch) jamb, drawn open into the room.
fn room() -> Drawing {
    let mut d = Drawing::new_imperial();
    let outer = [((0., 0.), (20.5, 0.)), ((20.5, 0.), (20.5, 10.)), ((20.5, 10.), (11., 10.)), ((8., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let inner =
        [((0.5, 0.5), (20.0, 0.5)), ((20.0, 0.5), (20.0, 9.5)), ((20.0, 9.5), (11.0, 9.5)), ((8.0, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    for (a, b) in outer.into_iter().chain(inner) {
        wall(&mut d, a, b);
    }
    wall(&mut d, (8.0, 9.5), (8.0, 10.0));
    wall(&mut d, (11.0, 9.5), (11.0, 10.0));
    // The leaf open from the hinge (11', 9.5') down into the room; the swing from the shut end at
    // the latch jamb (8', 9.5') round to it.
    line_in(&mut d, "A-DOOR", (132.0, 114.0), (132.0, 78.0));
    push(
        &mut d,
        "A-DOOR",
        EntityKind::Arc(Arc {
            center: Vec3 { x: 132.0, y: 114.0, z: 0.0 },
            radius: 36.0,
            start: std::f64::consts::PI,
            end: 1.5 * std::f64::consts::PI,
        }),
    );
    d
}

fn device(x: f64, y: f64, mark: &str, kind: &str) -> Value {
    json!({
        "id": format!("device:L1:{x:.2},{y:.2}:{mark}"), "kind": kind, "level": "L1", "at_ft": [x, y],
        "fields": {
            "type": {"value": mark, "source": "plan"},
            "device_type": {"value": format!("{kind} {mark}"), "source": "plan"},
            "sheet": {"value": "E-901", "source": "plan"},
            "symbol": {"value": format!("1F{mark}"), "source": "plan"},
        },
    })
}

fn job() -> Session {
    let mut s = Session::new();
    s.open_drawing(room(), "Door Hardware Room", None);
    s.execute("layer.new", &json!({"name": "A-ROOM-TAG", "current": true})).expect("layer");
    s.execute("text", &json!({"at": [100.0, 50.0], "text": "OFFICE", "height": 6})).expect("text");
    s.execute("text", &json!({"at": [100.0, 40.0], "text": "1001", "height": 6})).expect("text");
    let tags = vec![
        // A card reader drawn 1' past the hinge jamb, on the door's wall.
        device(12.0, 9.3, "CR", "low_voltage_outlet"),
        // One already beside the latch jamb.
        device(7.0, 9.3, "CR", "low_voltage_outlet"),
        // A door holder drawn at the hinge jamb.
        device(11.3, 9.2, "DH", "door_holder"),
        // A receptacle on the door holders' circuit is no door hardware.
        device(12.5, 9.3, "REC", "receptacle"),
    ];
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-DOOR", "role": "door"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "openings": {"enabled": true, "min_ft": 1.5, "max_ft": 16.0, "door_head_ft": 7.0, "window_head_ft": 7.0, "window_sill_ft": 2.0},
        "sheets": [{"number": "E-901", "level": "L1", "discipline": "E", "rect": [-24, -24, 600, 200],
                    "registration": {"reference": "E-901", "method": "reference", "a": 1, "b": 0, "tx": 0, "ty": 0}}],
        "tags": tags,
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

fn at_ft(s: &mut Session, mark: &str) -> Vec<(Value, [f64; 2])> {
    let r = s.execute("bim.props.find", &json!({"kind": "device", "where": {"type_mark": mark}, "limit": 10})).expect("find");
    let hs: Vec<String> = r["handles"].as_array().expect("handles").iter().filter_map(|h| h.as_str().map(str::to_string)).collect();
    let mut out = Vec::new();
    for h in hs {
        let g = s.execute("bim.props.get", &json!({"handle": h})).expect("get");
        let p = g["elements"][0]["props"].clone();
        let c = s.execute("bim.element", &json!({"handle": h})).ok().and_then(|v| Some([v["at"][0].as_f64()?, v["at"][1].as_f64()?]));
        out.push((p, c.unwrap_or([f64::NAN, f64::NAN])));
    }
    out
}

#[test]
fn a_card_reader_and_a_door_holder_drawn_at_the_hinge_go_to_the_latch_side_their_drawn_spot_kept() {
    let _g = serial();
    let mut s = job();
    // Measured first: two of the three door hardware devices are on the hinge side.
    let m = s.execute("bim.doorside", &json!({"apply": false})).expect("measure");
    assert_eq!((m["checked"].as_u64(), m["moved"].as_u64(), m["right_side"].as_u64()), (Some(3), Some(2), Some(1)), "{m:#}");
    let before = at_ft(&mut s, "CR");
    let r = s.execute("bim.doorside", &json!({})).expect("side");
    assert_eq!(r["moved_by_kind"], json!({"card_reader": 1, "door_holder": 1}), "{r:#}");
    let after = at_ft(&mut s, "CR");
    let moved: Vec<&(Value, [f64; 2])> = after.iter().filter(|(p, _)| p.get("door_side").is_some_and(|v| !v.is_null())).collect();
    assert_eq!(moved.len(), 1, "{after:?}");
    let (p, c) = moved[0];
    // Mirrored across the opening's middle (9.5'): 1' past the latch jamb, on the same wall.
    assert!((c[0] - 7.0).abs() < 0.3 && (c[1] - 9.3).abs() < 0.4, "{c:?} (before {before:?})");
    let why = p["door_side"]["value"].as_str().unwrap_or_default();
    assert!(why.contains("latch side") && why.contains("drawn at") && why.contains("card reader"), "{p:#}");
    assert_eq!(p["door_side"]["source"]["kind"], json!("rule"), "{p:#}");
    // The holder: where the open leaf's latch edge rests (the leaf's end at (11', 6.5')), set back 3".
    let dh = at_ft(&mut s, "DH");
    let (hp, hc) = &dh[0];
    assert!(hp["door_side"]["value"].as_str().is_some_and(|w| w.contains("open leaf")), "{hp:#}");
    assert!((hc[0] - 11.0).abs() < 0.4 && (hc[1] - 6.75).abs() < 0.4, "{hc:?}");
    // The receptacle stays; a second pass moves nothing.
    assert!(at_ft(&mut s, "REC").iter().all(|(p, _)| p.get("door_side").is_none_or(Value::is_null)));
    let again = s.execute("bim.doorside", &json!({})).expect("again");
    assert_eq!((again["moved"].as_u64(), again["right_side"].as_u64()), (Some(0), Some(3)), "{again:#}");
}
