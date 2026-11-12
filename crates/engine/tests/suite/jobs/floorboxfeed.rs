//! fix-floor-box-fed-from-above: a floor box on a slab on grade is fed from below the slab
//! (buried raceway from the box to the nearest wall of its room, stubbed up in that wall), never
//! by raceway rising out of the box through the open room; a keynote's under-slab conduit from
//! the box is that raceway and carries the box's circuit.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// A device tag at (x, y) feet on `circuit`: a floor box (`mount` floor) or a wall receptacle.
fn device(n: usize, x: f64, y: f64, circuit: &str, floor: bool) -> Value {
    let mut t = json!({"id": format!("device:L1:{n}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x3A0 + n), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"},
        "mount_height_in": {"value": "18", "source": "note"}
    }});
    if floor {
        t["fields"]["mount"] = json!({"value": "floor", "source": "note"});
        t["fields"]["device_type"] = json!({"value": "floor box", "source": "note"});
        t["fields"]["mount_height_in"] = json!({"value": "0", "source": "note"});
    }
    t
}

fn circuit(label: &str) -> Value {
    json!({"id": format!("circuit:L1:E-101:{label}"), "kind": "circuit", "level": "L1", "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": label, "source": "plan"},
        "panel": {"value": "LP1", "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "conductors": {"value": "2#12 + 1#12G", "source": "default"},
        "conduit": {"value": "EMT", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }})
}

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).expect("layer");
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).expect("pline");
}

/// A made-up one-storey building on a slab on grade (12' floor to floor, 9' ceiling): one open
/// room 40' x 30' (inside 0..480 x 0..360 in, a 6 in shell), panel LP1 by its west wall. Circuit
/// LP1-7: a floor box at (20', 22') (its room's nearest wall the north one, 8' off) and a wall
/// receptacle at (35', 2'). `keynote`: the keynotes built an under-slab conduit from the floor
/// box north to that wall (1", its two stub-ups), as the keynote step does.
fn building(keynote: bool) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    layer(&mut s, "A-WALL");
    rect(&mut s, [-6.0, -6.0], [486.0, 366.0]);
    rect(&mut s, [0.0, 0.0], [480.0, 360.0]);
    layer(&mut s, "A-ROOM-TAG");
    s.execute("text", &json!({"at": [120.0, 120.0], "text": "101", "height": 6.0})).expect("tag");
    layer(&mut s, "0");
    let tags = vec![device(1, 20.0, 22.0, "LP1-7", true), device(2, 35.0, 2.0, "LP1-7", false), circuit("LP1-7")];
    let mut systems = vec![];
    let mut runs = vec![];
    if keynote {
        systems.push(json!({"name": "E-UG", "layer": "E-UG", "kind": "conduit", "size_in": [1.0, 0], "bottom_ft": -1.5}));
        let called = "note: E-101 keynote 7 (under the slab)";
        let base = json!({"system": "E-UG", "level": "L1", "size_in": [1.0, 0], "size_source": "note: E-101 keynote 7",
            "height_source": called, "source_sheet": "E-101"});
        let mut r = base.clone();
        r["id"] = json!("keynote:E-101:7:0:0");
        r["points"] = json!([[240.0, 264.0], [240.0, 360.0]]);
        r["bottoms_ft"] = json!([-1.5]);
        runs.push(r);
        for (i, (at, top)) in [([240.0, 264.0], 0.0), ([240.0, 360.0], 1.5)].into_iter().enumerate() {
            let mut u = base.clone();
            u["id"] = json!(format!("keynote:E-101:7:0:0:up{i}"));
            u["points"] = json!([at]);
            u["bottoms_ft"] = json!([-1.5]);
            u["top_ft"] = json!(top);
            runs.push(u);
        }
    }
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Shell"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Shell", "thickness_in": 6}],
        "rooms": [{"name": "Lounge", "number": "101", "level": "L1"}],
        "tags": tags,
        "mep": {
            "systems": systems,
            "runs": runs,
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [2, 100], "max": [6, 120], "height_ft": 4, "bottom_ft": 2.5,
                 "enclosure": {"top_entry_in": [3.0, 10.0], "bottom_entry_in": [3.0, 10.0], "gutter_in": [0.0, 0.0]}}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

/// Every conduit stick: its circuit, raceway, ends (feet).
fn sticks(s: &Session) -> Vec<(String, String, [f64; 3], [f64; 3])> {
    let d = s.doc().expect("doc");
    let xyz = |t: String| {
        let v: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
        [v.first().copied().unwrap_or(0.0) / 12.0, v.get(1).copied().unwrap_or(0.0) / 12.0, v.get(2).copied().unwrap_or(0.0) / 12.0]
    };
    d.model
        .iter()
        .map(|e| cadaddy_engine::doc::props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(|x| x.text()).as_deref() == Some("conduit"))
        .map(|p| {
            (
                p.get("circuit").map(|x| x.text()).unwrap_or_default(),
                p.get("type").map(|x| x.text()).unwrap_or_default(),
                xyz(p.get("start").map(|x| x.text()).unwrap_or_default()),
                xyz(p.get("end").map(|x| x.text()).unwrap_or_default()),
            )
        })
        .collect()
}

/// The floor box as built: its plan point (feet).
fn floor_box(s: &Session) -> [f64; 2] {
    let d = s.doc().expect("doc");
    let mut found = None;
    for e in d.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device")) {
        let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind else { continue };
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for v in &m.vertices {
            for (k, x) in [v.x, v.y, v.z].into_iter().enumerate() {
                lo[k] = lo[k].min(x);
                hi[k] = hi[k].max(x);
            }
        }
        if hi[2] < 12.0 {
            found = Some([(lo[0] + hi[0]) / 24.0, (lo[1] + hi[1]) / 24.0]);
        }
    }
    found.expect("the floor box is built low")
}

/// The sticks with an end within 0.6' of `at` in plan.
fn at_box(st: &[(String, String, [f64; 3], [f64; 3])], at: [f64; 2]) -> Vec<&(String, String, [f64; 3], [f64; 3])> {
    let near = |p: &[f64; 3]| (p[0] - at[0]).hypot(p[1] - at[1]) < 0.6;
    st.iter().filter(|(_, _, a, b)| near(a) || near(b)).collect()
}

#[test]
fn a_floor_box_on_grade_is_fed_under_the_slab_from_the_nearest_wall_never_rising_through_the_room() {
    let mut s = building(false);
    let fb = floor_box(&s);
    assert!((fb[0] - 20.0).abs() < 0.6 && (fb[1] - 22.0).abs() < 0.6, "{fb:?}");
    let st = sticks(&s);
    let mine: Vec<_> = st.iter().filter(|x| x.0.contains("LP1-7")).collect();
    assert!(!mine.is_empty(), "{st:?}");
    // Nothing rises out of the box into the room: every stick at the box stays at or under the
    // floor (the drop out of its bottom, the run under the slab).
    let here = at_box(&st, fb);
    assert!(!here.is_empty(), "a raceway enters the floor box: {st:?}");
    assert!(here.iter().all(|(_, _, a, b)| a[2].max(b[2]) < 0.5), "raceway rising out of the floor box through the room: {here:?}");
    // Under the slab: the underground raceway (PVC), carrying the circuit, to the north wall
    // (y 30') and up in it; no exposed raceway in the open room over the box.
    let buried: Vec<_> = mine.iter().filter(|(_, _, a, b)| a[2].min(b[2]) < -0.5).collect();
    assert!(!buried.is_empty(), "{mine:?}");
    assert!(buried.iter().all(|(_, ty, ..)| ty.eq_ignore_ascii_case("PVC")), "{buried:?}");
    assert!(buried.iter().any(|(_, _, a, b)| [a, b].iter().any(|p| (p[1] - 30.0).abs() < 0.6 && (p[0] - 20.0).abs() < 0.6)), "{buried:?}");
    // One raceway takes the stub on above the floor (the floor box is the end of one leg: no
    // box at the stub for a second).
    let stub = |p: &[f64; 3]| (p[0] - 20.0).abs() < 0.6 && (p[1] - 30.0).abs() < 0.6 && (p[2] - 1.5).abs() < 0.1;
    let above: Vec<_> = mine.iter().filter(|(_, ty, a, b)| !ty.eq_ignore_ascii_case("PVC") && (stub(a) || stub(b))).collect();
    assert_eq!(above.len(), 1, "{above:?}");
    let over_box = |p: &[f64; 3]| (p[0] - fb[0]).abs() < 3.0 && (p[1] - fb[1]).abs() < 3.0 && p[2] > 0.5 && p[2] < 8.5;
    assert!(!st.iter().any(|(_, _, a, b)| over_box(a) && over_box(b)), "exposed raceway over the floor box: {st:?}");
    let r = s.execute("conduit.route", &json!({})).expect("report");
    let fed = &r["floor_boxes_under_slab"];
    assert_eq!((fed["boxes"].clone(), fed["fed"].clone(), fed["keynote_runs_taken"].clone()), (json!(1), json!(1), json!(0)), "{fed}");
    assert!(fed["items"][0]["basis"].as_str().is_some_and(|b| b.contains("slab on grade")), "{fed}");
    // The circuit's conductors run through it: the wire takeoff counts its length.
    let w = s.execute("bim.count", &json!({"kind": "wire", "circuit": "LP1-7"})).expect("count");
    let len = fed["items"][0]["length_ft"].as_f64().unwrap_or(0.0);
    assert!(len > 8.0 && w["length_ft"].as_f64().is_some_and(|x| x > 3.0 * len), "{len} {w}");
}

#[test]
fn the_keynotes_under_slab_conduit_carries_the_floor_boxs_circuit_and_is_not_doubled() {
    let mut s = building(true);
    let fb = floor_box(&s);
    let st = sticks(&s);
    let buried: Vec<_> = st.iter().filter(|(_, _, a, b)| a[2].min(b[2]) < -0.5).collect();
    assert!(!buried.is_empty(), "{st:?}");
    // One raceway under the slab, the keynote's, and it is the circuit's: no empty copy.
    assert!(buried.iter().all(|(c, ..)| c.contains("LP1-7")), "an empty keynote conduit under the slab: {buried:?}");
    let flat: f64 = buried.iter().filter(|(_, _, a, b)| (a[2] - b[2]).abs() < 0.1).map(|(_, _, a, b)| (a[0] - b[0]).hypot(a[1] - b[1])).sum();
    assert!((flat - 8.0).abs() < 1.0, "once along the keynote's 8' path: {flat} {buried:?}");
    assert!(at_box(&st, fb).iter().all(|(_, _, a, b)| a[2].max(b[2]) < 0.5), "{st:?}");
    let r = s.execute("conduit.route", &json!({})).expect("report");
    let fed = &r["floor_boxes_under_slab"];
    assert_eq!((fed["fed"].clone(), fed["keynote_runs_taken"].clone()), (json!(1), json!(1)), "{fed}");
    assert_eq!(fed["items"][0]["size_in"], json!(1.0), "the keynote's size: {fed}");
    assert!(fed["items"][0]["basis"].as_str().is_some_and(|b| b.contains("keynote 7")), "{fed}");
    // A reroute starts from the keynote's run again and takes it on the same way.
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let again = sticks(&s);
    let buried2: Vec<_> = again.iter().filter(|(_, _, a, b)| a[2].min(b[2]) < -0.5).collect();
    assert!(!buried2.is_empty() && buried2.iter().all(|(c, ..)| c.contains("LP1-7")), "{buried2:?}");
    let r2 = s.execute("conduit.route", &json!({})).expect("report");
    assert_eq!(r2["floor_boxes_under_slab"]["keynote_runs_taken"], json!(1), "{}", r2["floor_boxes_under_slab"]);
}

#[test]
fn a_circuit_with_no_floor_box_is_fed_from_above_as_before() {
    // No floor box: nothing is fed from below the slab.
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": [device(1, 20.0, 10.0, "LP1-7", false), device(2, 30.0, 10.0, "LP1-7", false), circuit("LP1-7")],
        "mep": {"equipment": [
            {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
             "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
        ]}
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let r = s.execute("conduit.route", &json!({})).expect("report");
    assert_eq!(r["floor_boxes_under_slab"]["boxes"], json!(0), "{r}");
    assert!(sticks(&s).iter().all(|(_, _, a, b)| a[2].min(b[2]) > -0.1), "nothing under the slab");
}
