//! Q6620: a rebuild of a hand-corrected model keeps what was made by hand. A copy (COPY, paste,
//! ARRAY) of a generated element keeps its source's build identity; `model.build {force: true}`
//! must keep it (as an "add" override), the way it keeps moves and property edits.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line, entity_bounds};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn push(d: &mut Drawing, layer: &str, kind: EntityKind) {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, kind);
    e.common.layer = layer.into();
    d.model.push(e);
}

fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    push(d, layer, EntityKind::Line(Line { a: p(a), b: p(b) }));
}

/// Two 20' × 10' rooms of 6" walls side by side (feet; the drawing is in inches).
fn plan() -> Drawing {
    let mut d = Drawing::new_imperial();
    for x0 in [0.0, 20.0] {
        let o = [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
        let i = [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
        for ((ax, ay), (bx, by)) in o.into_iter().chain(i) {
            line(&mut d, "A-WALL", (ax + x0, ay), (bx + x0, by));
        }
    }
    d
}

fn device(x: f64, y: f64, mark: &str, kind: &str, circuit: &str) -> Value {
    json!({"id": format!("device:L1:{x:.1},{y:.1}:{mark}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("{kind} {mark}"), "source": "plan"},
        "mount_height_in": {"value": "18", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"},
        "panel": {"value": "LP", "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "symbol": {"value": format!("{mark}{x}"), "source": "plan"},
    }})
}

fn built() -> Session {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Sample Floor Plan", None);
    let sp = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": [
            device(5.0, 0.8, "REC", "receptacle", "LP-1"),
            device(12.0, 0.8, "REC", "receptacle", "LP-1"),
            device(8.0, 9.2, "S", "switch", "LP-3"),
            device(28.0, 9.2, "T", "thermostat", "LP-5"),
        ],
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

/// The generated devices: (handle, part, centre in feet, circuit).
fn devices(s: &Session) -> Vec<(Handle, String, [f64; 2], String)> {
    let d = s.doc().unwrap();
    let mut v: Vec<_> = d
        .model
        .iter()
        .filter_map(|e| {
            let part = made_as(e)?;
            if matches!(part.as_str(), "wall" | "floor" | "ceiling" | "room" | "slab" | "roof" | "ground" | "atom") {
                return None;
            }
            let c = entity_bounds(d, e, 0).center();
            let circuit = cadaddy_engine::doc::props::read(&e.common.xdata).get("circuit").map(|p| format!("{:?}", p)).unwrap_or_default();
            Some((e.handle, part, [(c.x / 12.0 * 10.0).round() / 10.0, (c.y / 12.0 * 10.0).round() / 10.0], circuit))
        })
        .collect();
    v.sort_by(|a, b| a.2[0].total_cmp(&b.2[0]).then(a.2[1].total_cmp(&b.2[1])));
    v
}

fn at(s: &Session, part: &str, x: f64, y: f64) -> Vec<(Handle, String)> {
    devices(s).into_iter().filter(|d| d.1 == part && (d.2[0] - x).abs() < 0.3 && (d.2[1] - y).abs() < 0.3).map(|d| (d.0, d.3)).collect()
}

fn circuit_of(p: &str) -> String {
    p.split("String(\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or_default().to_string()
}

fn overrides(s: &mut Session) -> Vec<Value> {
    s.execute("override.list", &json!({})).unwrap()["overrides"].as_array().cloned().unwrap_or_default()
}

fn kinds(s: &mut Session) -> Vec<String> {
    let mut v: Vec<String> = overrides(s).iter().map(|o| format!("{}:{}", o["kind"].as_str().unwrap(), o["status"].as_str().unwrap())).collect();
    v.sort();
    v
}

/// The devices as (x, y, circuit), in feet.
fn spots(s: &Session) -> Vec<(f64, f64, String)> {
    devices(s).into_iter().filter(|d| d.1 == "device").map(|d| (d.2[0], d.2[1], circuit_of(&d.3))).collect()
}

fn force(s: &mut Session) -> Value {
    s.execute("model.build", &json!({"force": true})).unwrap()
}

/// A receptacle copied to the other room and given its own circuit, the switch moved 2': a
/// rebuild with `force` keeps all three edits, and its circuit stays on the copy (it went onto
/// the thermostat, the nearest device of the key every spec device shared).
fn edited() -> Session {
    let mut s = built();
    let rec = at(&s, "device", 12.0, 0.4)[0].0;
    let sw = at(&s, "device", 8.0, 9.6)[0].0;
    s.execute("copy", &json!({"handles": [rec.hex()], "delta": [156.0, 0.0]})).unwrap();
    let copy = at(&s, "device", 25.0, 0.4)[0].0;
    s.execute("bim.props.set", &json!({"handle": copy.hex(), "key": "circuit", "value": "LP-7"})).unwrap();
    s.execute("move", &json!({"handles": [sw.hex()], "delta": [24.0, 0.0]})).unwrap();
    s
}

const EDITED: [(f64, f64, &str); 5] = [(5.0, 0.4, "LP-1"), (10.0, 9.6, "LP-3"), (12.0, 0.4, "LP-1"), (25.0, 0.4, "LP-7"), (28.0, 9.6, "LP-5")];

fn expect(s: &Session, want: &[(f64, f64, &str)]) {
    let got = spots(s);
    let want: Vec<(f64, f64, String)> = want.iter().map(|(x, y, c)| (*x, *y, c.to_string())).collect();
    assert_eq!(got, want);
}

#[test]
fn a_forced_rebuild_keeps_copies_moves_and_their_properties() {
    let mut s = edited();
    expect(&s, &EDITED);
    assert_eq!(kinds(&mut s), ["add:active", "move:active"], "the copy and the move are recorded when made");
    let r = force(&mut s);
    expect(&s, &EDITED);
    assert_eq!(r["handMade"]["kept"], json!(1), "{}", r["handMade"]);
    assert_eq!(r["handMade"]["found_at_rebuild"], json!(0));
    // Again: nothing grows, nothing moves twice.
    force(&mut s);
    expect(&s, &EDITED);
    assert_eq!(kinds(&mut s), ["add:active", "move:active"]);
    // The copy is no generated element: a property set on it is its own, no override.
    let copy = at(&s, "device", 25.0, 0.4)[0].0;
    s.execute("bim.props.set", &json!({"handle": copy.hex(), "key": "circuit", "value": "LP-9"})).unwrap();
    assert_eq!(kinds(&mut s), ["add:active", "move:active"]);
    force(&mut s);
    assert_eq!(spots(&s).iter().find(|d| d.0 == 25.0).map(|d| d.2.as_str()), Some("LP-9"));
}

#[test]
fn a_reopened_model_keeps_them_through_a_rebuild() {
    let s = edited();
    let d = s.doc().unwrap().clone();
    // Opened again: no session record of the build, only what the drawing holds.
    let mut t = Session::empty();
    t.open_drawing(d, "Sample Floor Plan", None);
    force(&mut t);
    expect(&t, &EDITED);
}

#[test]
fn copies_made_before_they_were_recorded_are_found_at_the_rebuild() {
    let mut s = built();
    let rec = at(&s, "device", 12.0, 0.4)[0].0;
    // As an older version left them: plain copies of the element, its build tag and all, one of
    // them on the very spot of its source (no copy a person would keep).
    {
        let d = s.doc_mut().unwrap();
        let src = (**d.model.get(rec).unwrap()).clone();
        for dx in [156.0, 0.0] {
            let mut c = src.clone();
            c.handle = d.new_handle();
            if let EntityKind::Mesh(m) = &mut c.kind {
                for v in &mut m.vertices {
                    v.x += dx;
                }
            }
            d.model.push(c);
        }
    }
    assert_eq!(spots(&s).len(), 6);
    let r = force(&mut s);
    assert_eq!(r["handMade"]["found_at_rebuild"], json!(1), "{}", r["handMade"]);
    expect(&s, &[(5.0, 0.4, "LP-1"), (8.0, 9.6, "LP-3"), (12.0, 0.4, "LP-1"), (25.0, 0.4, "LP-1"), (28.0, 9.6, "LP-5")]);
    assert_eq!(kinds(&mut s), ["add:active"]);
    // Recorded now: the next rebuild keeps it without finding it again.
    let r = force(&mut s);
    assert_eq!(r["handMade"]["found_at_rebuild"], json!(0));
    assert_eq!(spots(&s).len(), 5);
}

#[test]
fn an_erased_copy_is_let_go_and_undo_brings_it_back() {
    let mut s = edited();
    let copy = at(&s, "device", 25.0, 0.4)[0].0;
    s.execute("erase", &json!({"handles": [copy.hex()]})).unwrap();
    assert_eq!(kinds(&mut s), ["move:active"]);
    force(&mut s);
    expect(&s, &[(5.0, 0.4, "LP-1"), (10.0, 9.6, "LP-3"), (12.0, 0.4, "LP-1"), (28.0, 9.6, "LP-5")]);
    // Undone (the rebuild, then the erase), the copy and its record come back together.
    s.execute("undo", &json!({})).unwrap();
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(kinds(&mut s), ["add:active", "move:active"]);
    expect(&s, &EDITED);
}

#[test]
fn hand_edits_are_no_build_input_an_unchanged_model_is_kept() {
    let mut s = edited();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["kept"], json!(true), "a copy and a move are steps after the build: {}", r["notes"]);
    expect(&s, &EDITED);
}

#[test]
fn an_override_keyed_to_no_plan_object_is_found_by_place_only() {
    let mut s = built();
    let sw = at(&s, "device", 8.0, 9.6)[0].0;
    s.execute("bim.props.set", &json!({"handle": sw.hex(), "key": "note", "value": "checked"})).unwrap();
    // As a key written before keys carried the element's id: its source is the spec's `0`, and
    // it stood 20' from any device.
    {
        let d = s.doc_mut().unwrap();
        let text = d.overrides.as_deref().unwrap().to_string();
        let at_ = "\"at\":[96.0,115.0]";
        assert!(text.contains(at_), "{text}");
        let old = text.replace("0#device:L1:8.0,9.2:S", "0").replace(at_, "\"at\":[96.0,355.0]");
        d.overrides = Some(old).into();
    }
    force(&mut s);
    assert_eq!(kinds(&mut s), ["props:orphaned"], "not put on the nearest device 20' away");
    let noted = s.doc().unwrap().model.iter().filter(|e| cadaddy_engine::doc::props::read(&e.common.xdata).get("note").is_some()).count();
    assert_eq!(noted, 0);
}

#[test]
fn a_build_of_some_parts_keeps_every_other_element_as_it_stands() {
    let mut s = edited();
    // An edit no record holds (a device's elevation changed in place by a tool outside the
    // commands): a whole rebuild would lose it, a build of the ceilings doesn't.
    let rec = at(&s, "device", 5.0, 0.4)[0].0;
    s.doc_mut().unwrap().modify_entity(rec, |e| e.common.layer = "E-POWER-FIELD".into()).unwrap();
    let before: Vec<(Handle, String)> = devices(&s).into_iter().map(|d| (d.0, format!("{:?}", d.2))).collect();
    let ceilings =
        |s: &Session| s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("ceiling")).map(|e| e.handle).collect::<Vec<_>>();
    let old_ceilings = ceilings(&s);
    let r = s.execute("model.build", &json!({"parts": ["Ceiling", "nothing_called_this"]})).unwrap();
    assert_eq!(r["parts"]["unmatched"], json!(["nothing_called_this"]), "{}", r["parts"]);
    assert!(r["parts"]["elements_rebuilt"].as_u64().unwrap() >= 1, "{}", r["parts"]);
    let after: Vec<(Handle, String)> = devices(&s).into_iter().map(|d| (d.0, format!("{:?}", d.2))).collect();
    assert_eq!(before, after, "every device kept, handle and place");
    assert_eq!(s.doc().unwrap().entity(rec).unwrap().common.layer, "E-POWER-FIELD");
    let new_ceilings = ceilings(&s);
    assert_eq!(new_ceilings.len(), old_ceilings.len());
    assert!(new_ceilings.iter().all(|h| !old_ceilings.contains(h)), "the ceilings were built again");
    for bad in [json!([]), json!("ceiling"), json!([""]), json!([1])] {
        assert!(s.execute("model.build", &json!({"parts": bad})).is_err(), "{bad}");
    }
}
