//! BIM and estimate inputs take only values that can be (Q6384): a number field of an element's
//! form a finite number in range, an enum field an allowed value, a level one of the model's,
//! prices 0 or more on a real date, a crew rate above 0; a kind of count, a trade of an AR
//! export or an object of a BCF topic that names nothing is refused, never read as zero or as
//! everything; a dry run of an edit that changes no object says what data it would change.

use std::path::PathBuf;

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Arc, Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-inputs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn push(d: &mut Drawing, layer: &str, kind: EntityKind) {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, kind);
    e.common.layer = layer.into();
    d.model.push(e);
}

/// A line in feet.
fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    push(d, layer, EntityKind::Line(Line { a: p(a), b: p(b) }));
}

/// A room's inner faces from (x0, y0) to (x1, y1) feet, door gaps `(x from, x to)` in its top
/// or bottom side.
fn room(d: &mut Drawing, x0: f64, y0: f64, x1: f64, y1: f64, top: &[(f64, f64)], bottom: &[(f64, f64)]) {
    let side = |d: &mut Drawing, y: f64, gaps: &[(f64, f64)]| {
        let mut at = x0;
        for (a, b) in gaps {
            line(d, "A-WALL", (at, y), (*a, y));
            at = *b;
        }
        line(d, "A-WALL", (at, y), (x1, y));
    };
    side(d, y0, bottom);
    side(d, y1, top);
    line(d, "A-WALL", (x0, y0), (x0, y1));
    line(d, "A-WALL", (x1, y0), (x1, y1));
}

/// A door in a horizontal wall (faces at `y0`, `y1`) from `x0` to `x1` feet.
fn door(d: &mut Drawing, x0: f64, x1: f64, y0: f64, y1: f64) {
    line(d, "A-WALL", (x0, y0), (x0, y1));
    line(d, "A-WALL", (x1, y0), (x1, y1));
    line(d, "A-DOOR", (x0, y0), (x0, y0 - (x1 - x0)));
    push(
        d,
        "A-DOOR",
        EntityKind::Arc(Arc {
            center: Vec3 { x: x0 * 12.0, y: y0 * 12.0, z: 0.0 },
            radius: (x1 - x0) * 12.0,
            start: 1.5 * std::f64::consts::PI,
            end: 0.0,
        }),
    );
}

/// One storey 40 x 30 ft: two rooms off a corridor, 6" walls.
fn plan() -> Drawing {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 30.), (40., 30.)), ((40., 0.), (40., 30.)), ((0., 0.), (0., 30.)), ((0., 0.), (40., 0.))] {
        line(&mut d, "A-WALL", a, b);
    }
    room(&mut d, 0.5, 0.5, 19.75, 19.5, &[(8.0, 11.0)], &[]);
    room(&mut d, 20.25, 0.5, 39.5, 19.5, &[(28.0, 31.0)], &[]);
    room(&mut d, 0.5, 20.0, 39.5, 29.5, &[], &[(8.0, 11.0), (28.0, 31.0)]);
    for (a, b) in [(8.0, 11.0), (28.0, 31.0)] {
        door(&mut d, a, b, 19.5, 20.0);
    }
    d
}

fn device(x: f64, y: f64, mark: &str, kind: &str, extra: Value) -> Value {
    let mut fields = json!({
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("{kind} {mark}"), "source": "plan"},
        "sheet": {"value": "E-1", "source": "plan"},
        "symbol": {"value": format!("S{x:.0}{y:.0}"), "source": "plan"},
    });
    if let (Some(f), Some(x)) = (fields.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            f.insert(k.clone(), json!({"value": v, "source": "plan"}));
        }
    }
    json!({"id": format!("device:L1:{x:.1},{y:.1}:{mark}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": fields})
}

fn spec() -> Value {
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0], "region": [-60, -60, 540, 420]}],
        "layers": [
            {"layer": "A-WALL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-DOOR", "role": "door"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "openings": {"enabled": true, "min_ft": 1.5, "max_ft": 16.0, "door_head_ft": 7.0, "window_head_ft": 7.0, "window_sill_ft": 3.0},
        "tags": [
            device(4.0, 0.8, "REC", "receptacle", json!({"mount_height_in": "18", "circuit": "P1-1"})),
            device(16.0, 0.8, "REC", "receptacle", json!({"mount_height_in": "18", "circuit": "P1-3"})),
            device(24.0, 0.8, "REC", "receptacle", json!({"mount_height_in": "18", "circuit": "P1-5"})),
            device(10.0, 10.0, "A", "light_fixture", json!({"circuit": "P1-7"}))
        ],
        "mep": {
            "systems": [
                {"name": "SA", "layer": "M-SUPPLY", "kind": "duct", "size_in": [12, 8], "bottom_ft": 8, "duct_type": "supply"},
                {"name": "CW", "layer": "P-DOMW-CW", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 9}
            ],
            "runs": [
                {"system": "SA", "level": "L1", "points": [[24, 288], [456, 288]], "size_in": [12, 8], "bottoms_ft": [8]},
                {"system": "CW", "level": "L1", "points": [[24, 300], [384, 300]], "size_in": [1, 0], "bottoms_ft": [9]}
            ]
        }
    })
}

fn text_on(s: &mut Session, layer: &str, at: [f64; 2], t: &str) {
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
}

/// The storey, built: walls, doors, rooms, devices, a duct and a pipe.
fn built() -> Session {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Sample Building", None);
    for (x, y, n, name) in [(10.0, 10.0, "101", "OFFICE"), (30.0, 10.0, "102", "OFFICE"), (20.0, 25.0, "103", "CORRIDOR")] {
        text_on(&mut s, "A-ROOM-TAG", [x * 12.0, (y + 1.0) * 12.0], name);
        text_on(&mut s, "A-ROOM-TAG", [x * 12.0, (y - 1.0) * 12.0], n);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec()})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s.set_selection(Vec::new());
    s
}

fn find(s: &mut Session, kind: &str) -> Vec<String> {
    let r = s.execute("bim.props.find", &json!({"kind": kind})).unwrap_or_else(|e| panic!("find {kind}: {e}"));
    r["handles"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::to_string).collect()
}

/// A receptacle of the model.
fn receptacle(s: &mut Session) -> String {
    let all = find(s, "device");
    all.into_iter()
        .find(|h| {
            let g = s.execute("bim.props.get", &json!({"handle": h})).unwrap();
            g["elements"][0]["props"]
                .as_object()
                .is_some_and(|p| p.values().any(|v| v["value"].as_str().is_some_and(|t| t.to_ascii_lowercase().contains("receptacle"))))
        })
        .expect("a receptacle")
}

fn err(s: &mut Session, cmd: &str, p: Value) -> String {
    match s.execute(cmd, &p) {
        Ok(v) => format!("ACCEPTED {v}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn element_properties_take_only_values_the_element_can_hold() {
    let mut s = built();
    let h = receptacle(&mut s);
    let set = |k: &str, v: Value| json!({"handle": h, "key": k, "value": v});
    // A number field of its form: finite, 0 or more, in range.
    for v in [json!("nan"), json!("inf"), json!("-1e300"), json!(-1e300), json!(-1), json!("lots")] {
        let e = err(&mut s, "bim.props.set", set("mount_height", v.clone()));
        assert!(!e.starts_with("ACCEPTED") && e.contains("mount_height"), "mount_height {v}: {e}");
    }
    for v in [json!(18), json!("18"), json!("48 in AFF"), Value::Null] {
        s.execute("bim.props.set", &set("mount_height", v.clone())).unwrap_or_else(|e| panic!("mount_height {v}: {e}"));
    }
    // An enum field: one of its allowed values.
    let e = err(&mut s, "bim.props.set", set("gfci", json!("sometimes")));
    assert!(!e.starts_with("ACCEPTED") && e.contains("gfci"), "{e}");
    s.execute("bim.props.set", &set("gfci", json!("breaker"))).expect("an allowed gfci");
    // A level: one of the model's.
    let e = err(&mut s, "bim.props.set", set("level", json!("Level 99")));
    assert!(!e.starts_with("ACCEPTED") && e.contains("Level 99") && e.contains("L1"), "{e}");
    s.execute("bim.props.set", &set("level", json!("L1"))).expect("its own level");
    // Text a store would have to cut or change is refused, not changed silently.
    let e = err(&mut s, "bim.props.set", set("remark", json!("x".repeat(1_000_000))));
    assert!(!e.starts_with("ACCEPTED") && e.contains("1000"), "long: {}", e.chars().take(200).collect::<String>());
    let e = err(&mut s, "bim.props.set", set("remark", json!("a\u{0}b")));
    assert!(!e.starts_with("ACCEPTED") && e.contains("control"), "{e}");
    s.execute("bim.props.set", &set("remark", json!("two\nlines"))).expect("line breaks are text");
    // Nothing of a refused call was set.
    let g = s.execute("bim.props.get", &json!({"handle": h})).unwrap();
    assert_ne!(g["elements"][0]["props"]["mount_height"]["value"], json!("nan"), "{g}");
    // An enum field through bim.form.record too.
    let e = err(&mut s, "bim.form.record", json!({"handle": h, "key": "gfci", "value": "deadbolt", "source": {"kind": "spec", "doc": "Section 1"}}));
    assert!(!e.starts_with("ACCEPTED") && e.contains("gfci"), "{e}");
}

#[test]
fn estimate_inputs_are_prices_dates_and_rates_that_can_be() {
    let mut s = built();
    let price = |m: Value, date: &str| json!({"item": "EMT conduit", "unit": "ft", "material": m, "date": date, "source": "test"});
    let e = err(&mut s, "estimate.set_price", price(json!(-3), "2026-11-01"));
    assert!(!e.starts_with("ACCEPTED") && e.contains("material"), "{e}");
    let e = err(&mut s, "estimate.set_price", price(json!(1), "not-a-date"));
    assert!(!e.starts_with("ACCEPTED") && e.contains("date"), "{e}");
    s.execute("estimate.set_price", &price(json!(1.25), "2026-11-01")).expect("a real price");
    let e = err(&mut s, "estimate.settings", json!({"crew_rate": 0}));
    assert!(!e.starts_with("ACCEPTED") && e.contains("crew_rate"), "{e}");
    s.execute("estimate.settings", &json!({"crew_rate": 95})).expect("a crew rate");
    // A folder with no baseline file is refused, and the prices there are stay.
    let empty = dir("empty");
    let e = err(&mut s, "estimate.import", json!({"folder": empty.to_string_lossy()}));
    assert!(!e.starts_with("ACCEPTED") && e.contains("none of the baseline"), "{e}");
    let _ = std::fs::remove_dir_all(&empty);
}

#[test]
fn counts_refuse_kinds_they_do_not_count() {
    let mut s = built();
    let walls = find(&mut s, "wall").len();
    assert!(walls >= 4, "walls {walls}");
    for k in ["wall", "walls", "room", "duct", "pipe"] {
        let e = err(&mut s, "bim.count", json!({"kind": k}));
        assert!(!e.starts_with("ACCEPTED") && e.contains("bim.takeoff"), "{k}: {e}");
    }
    let r = s.execute("bim.count", &json!({"kind": "receptacle"})).expect("receptacles");
    assert!(r["count"].as_u64().unwrap_or(0) >= 3, "{r}");
    let r = s.execute("bim.count", &json!({"kind": "nonsense"})).expect("a kind with none");
    assert_eq!(r["count"], json!(0));
    assert!(r["note"].as_str().is_some_and(|n| n.contains("nonsense")), "{r}");
}

#[test]
fn an_ar_export_trade_that_matches_nothing_is_refused() {
    let _g = serial();
    let mut s = built();
    let d = dir("ar");
    let at = |n: &str| d.join(n).to_string_lossy().into_owned();
    s.execute("bim.export.ar", &json!({"level": "L1", "trades": ["electrical"], "path": at("elec"), "formats": ["glb"]})).expect("electrical");
    let e = err(&mut s, "bim.export.ar", json!({"level": "L1", "trades": ["nonsense"], "path": at("none"), "formats": ["glb"]}));
    assert!(!e.starts_with("ACCEPTED") && e.contains("nonsense"), "{e}");
    let e = err(&mut s, "bim.export.ar", json!({"level": "L1", "trades": ["electrical", "nonsense"], "path": at("both"), "formats": ["glb"]}));
    assert!(!e.starts_with("ACCEPTED") && e.contains("nonsense"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_bcf_topic_carries_the_model_elements_it_names() {
    let mut s = built();
    let h = receptacle(&mut s);
    let r = s.execute("bcf.create", &json!({"title": "Check this outlet", "selection": [h], "snapshot": false})).expect("topic");
    assert_eq!(r["selected"], json!(1), "{r}");
    s.set_selection(Vec::new());
    let shown = s.execute("bcf.show", &json!({"topic": r["guid"]})).expect("show");
    assert_eq!(shown["selected"], json!([h]), "{shown}");
    let e = err(&mut s, "bcf.create", json!({"title": "Nothing", "selection": ["FFFFFFF"], "snapshot": false}));
    assert!(!e.starts_with("ACCEPTED") && e.contains("FFFFFFF"), "{e}");
}

#[test]
fn a_dry_run_says_what_data_an_edit_would_change() {
    let mut s = built();
    s.execute("agent.dryrun", &json!({"on": true})).unwrap();
    let r = s
        .execute("estimate.set_price", &json!({"item": "EMT conduit", "unit": "ft", "material": 2, "date": "2026-11-01", "source": "test"}))
        .expect("dry price");
    let data = r["wouldChange"]["data"].as_array().cloned().unwrap_or_default();
    assert!(data.iter().any(|x| x.as_str().is_some_and(|t| t.contains("estimate"))), "{r}");
    let r = s.execute("bcf.create", &json!({"title": "Dry", "snapshot": false})).expect("dry topic");
    let data = r["wouldChange"]["data"].as_array().cloned().unwrap_or_default();
    assert!(data.iter().any(|x| x == "bcf"), "{r}");
    s.execute("agent.dryrun", &json!({"on": false})).unwrap();
    // Nothing was kept.
    let list = s.execute("bcf.list", &json!({})).expect("list");
    assert!(list["topics"].as_array().is_none_or(Vec::is_empty), "{list}");
}
