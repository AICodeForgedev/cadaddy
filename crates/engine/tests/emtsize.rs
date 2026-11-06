//! Q6235 fix-small-emt-over: a circuit's raceway is sized for the conductors it really carries
//! (its voltage drop's larger conductor on the homerun, the legs at the circuit's own size), and
//! each circuit has one route (an estimate for another tag of a routed circuit goes).

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// A receptacle at (x, y) feet on `circuit`, read from `sheet`.
fn receptacle(n: usize, x: f64, y: f64, circuit: &str) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x2A0 + n), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"},
        "mount_height_in": {"value": "18", "source": "note"}
    }})
}

fn circuit(sheet: &str, label: &str) -> Value {
    json!({"id": format!("circuit:L1:{sheet}:{label}"), "kind": "circuit", "level": "L1", "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": label, "source": "plan"},
        "panel": {"value": "LP1", "source": "plan"},
        "sheet": {"value": sheet, "source": "plan"},
        "conductors": {"value": "2#12 + 1#12G", "source": "default"},
        "conduit": {"value": "EMT", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }})
}

/// A made-up one-storey building (12' floor to floor, 9' ceiling) with panel LP1 at the origin and
/// circuit LP1-7's receptacles in a row `far` feet east of it, `step` feet apart.
fn building(far: f64, step: f64, n: usize, extra_tags: Vec<Value>, extra_runs: Vec<Value>) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    // One room round it all: a 6 in shell, its tag in the middle.
    layer(&mut s, "A-WALL");
    rect(&mut s, [-66.0, -306.0], [2886.0, 486.0]);
    rect(&mut s, [-60.0, -300.0], [2880.0, 480.0]);
    layer(&mut s, "A-ROOM-TAG");
    s.execute("text", &json!({"at": [1400.0, 300.0], "text": "101", "height": 6.0})).expect("tag");
    layer(&mut s, "0");
    let mut tags: Vec<Value> = (0..n).map(|i| receptacle(i + 1, far + step * i as f64, 10.0, "LP1-7")).collect();
    tags.push(circuit("E-101", "LP1-7"));
    tags.extend(extra_tags);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Shell"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Shell", "thickness_in": 6}],
        "rooms": [{"name": "Open Office", "number": "101", "level": "L1"}],
        "tags": tags,
        "mep": {
            "systems": if extra_runs.is_empty() {
                json!([])
            } else {
                json!([{"name": "E-CIRCUIT-EST", "layer": "E-CIRCUIT-EST", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9.25}])
            },
            "runs": extra_runs,
            // The schedule's load: 1,920 VA (16 A, 80% of the 20 A breaker).
            "panel_schedules": [{"panel": "LP1", "source": "E-101", "circuits": [
                {"number": "7", "description": "RECEPTACLES", "breaker": "20", "poles": 1, "load_va": 1920.0, "source": "E-101"}
            ]}],
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
                 "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).expect("layer");
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).expect("pline");
}

fn count(s: &mut Session, q: Value) -> Value {
    s.execute("bim.count", &q).expect("count")
}

#[test]
fn a_long_circuits_drop_is_laid_on_its_homerun_and_its_legs_raceway_stays_small() {
    // LP1-7's four receptacles 150' from the panel: at 80% of 20 A the whole circuit would need
    // #4 for 3%, and #4 in its legs puts them in 1" EMT. The field runs the #4 homerun and #10
    // legs (a device's terminal takes #10 at most): the legs stay in the 3/4" minimum.
    let mut s = building(150.0, 10.0, 4, Vec::new(), Vec::new());
    let r = s.execute("conduit.route", &json!({"section": "conductors_upsized"})).expect("report");
    let up = r["conductors_upsized"].as_array().and_then(|a| a.iter().find(|u| u["circuit"] == json!("LP1-7")).cloned()).unwrap_or_default();
    assert_eq!((up["to"].clone(), up["legs"].clone()), (json!("2#4 + 1#4G"), json!("2#10 + 1#10G")), "{r}");
    assert!(up["why"].as_str().is_some_and(|w| w.contains("laid on the homerun") && w.contains("110.14(A)")), "{up}");
    // The wire: #4 on the homerun, #10 in the legs.
    let w = count(&mut s, json!({"kind": "wire", "circuit": "LP1-7", "group_by": "size"}));
    let ft = |v: &Value, size: &str| {
        v["rows"].as_array().and_then(|a| a.iter().find(|x| x["size"] == json!(size)).and_then(|x| x["length_ft"].as_f64())).unwrap_or(0.0)
    };
    assert!(ft(&w, "#10") > 150.0 && ft(&w, "#4") > 300.0, "{w}");
    // The raceway: the homerun's 1" (fill of 2#4 + 1#4G); every leg the 3/4" minimum.
    let c = count(&mut s, json!({"kind": "conduit", "circuit": "LP1-7", "group_by": "size"}));
    assert!(ft(&c, "3/4\"") > 60.0, "the legs in 3/4\": {c}");
    assert!(ft(&c, "1\"") < 180.0, "only the homerun in 1\": {c}");
    // Validate agrees: every raceway holds what it carries, and the drop is taken piece by piece
    // (the #10 legs and the #4 homerun), not as the whole length on #10.
    let fs = screen(&mut s, &["E-CONDUIT-FILL", "E-VDROP"]);
    let fill: Vec<&Value> = fs.iter().filter(|f| f["rule"] == json!("E-CONDUIT-FILL")).collect();
    assert!(!fill.is_empty() && fill.iter().all(|f| f["verdict"] == json!("pass")), "{fill:?}");
    let vd = fs.iter().find(|f| f["rule"] == json!("E-VDROP") && f["measured"]["circuit"] == json!("LP1-7")).cloned().unwrap_or_default();
    assert_eq!(vd["verdict"], json!("pass"), "{vd}");
    let on_small = vd["measured"]["feet_on_smallest"].as_f64().unwrap_or(f64::NAN);
    let length = vd["measured"]["length_ft"].as_f64().unwrap_or(f64::NAN);
    assert!(on_small < length, "the #4 homerun counts for less than its feet on #10: {vd}");
    assert_eq!(vd["measured"]["sizes"], json!(["#10", "#4"]), "{vd}");
}

#[test]
fn legs_that_make_the_drop_on_the_circuits_own_size_keep_it() {
    // Three receptacles 30' out: the routed 92' needs #8 as one size, but the legs' share of
    // the drop is small, so they stay #12 and only the homerun goes up.
    let mut s = building(30.0, 10.0, 3, Vec::new(), Vec::new());
    let r = s.execute("conduit.route", &json!({"section": "conductors_upsized"})).expect("report");
    let up = r["conductors_upsized"].as_array().and_then(|a| a.iter().find(|u| u["circuit"] == json!("LP1-7")).cloned()).unwrap_or_default();
    assert_eq!(up["legs"], json!("2#12 + 1#12G"), "{r}");
    assert!(up["to"].as_str().is_some_and(|t| t != "2#12 + 1#12G"), "{r}");
    let w = count(&mut s, json!({"kind": "wire", "circuit": "LP1-7", "group_by": "size"}));
    assert!(w["rows"].as_array().is_some_and(|a| a.iter().any(|x| x["size"] == json!("#12"))), "the legs' wire is #12: {w}");
    let fs = screen(&mut s, &["E-VDROP"]);
    let vd = fs.iter().find(|f| f["measured"]["circuit"] == json!("LP1-7")).cloned().unwrap_or_default();
    assert_eq!(vd["verdict"], json!("pass"), "{vd}");
}

/// Validate's prescreen findings of `rules` on L1, every page.
fn screen(s: &mut Session, rules: &[&str]) -> Vec<Value> {
    let r = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": rules})).expect("prescreen");
    let mut fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    let mut cursor = r["next"]["cursor"].clone();
    while let Some(c) = cursor.as_u64() {
        let more = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "cursor": c, "rules": rules})).expect("page");
        fs.extend(more["findings"].as_array().cloned().unwrap_or_default());
        cursor = more["next"]["cursor"].clone();
    }
    fs
}

#[test]
fn a_circuit_tagged_on_two_sheets_has_one_route() {
    // LP1-7 is tagged on its plan and again on an enlarged plan, whose read left an estimated
    // route for its tag. The routing routes the circuit (the plan's tag): the enlargement's
    // estimate is the same conduit and goes; another circuit's estimate stays.
    let est = |id: &str, y: f64| {
        json!({"system": "E-CIRCUIT-EST", "level": "L1", "id": format!("{id}#1"), "points": [[1800.0, y], [2100.0, y]], "size_in": [0.75, 0], "bottoms_ft": [9.25],
               "size_source": "default", "height_source": "estimated route", "source_sheet": "E-601"})
    };
    let mut s = building(
        150.0,
        10.0,
        4,
        vec![circuit("E-601", "LP1-7"), circuit("E-601", "LP1-9")],
        vec![est("circuit:L1:E-601:LP1-7", 60.0), est("circuit:L1:E-601:LP1-9", 200.0)],
    );
    let left = count(&mut s, json!({"kind": "conduit", "system": "E-CIRCUIT-EST", "group_by": "circuit"}));
    let circuits: Vec<String> =
        left["rows"].as_array().map(|a| a.iter().filter_map(|x| x["circuit"].as_str().map(String::from)).collect()).unwrap_or_default();
    assert_eq!(circuits, ["LP1-9"], "{left}");
    // Rerouting keeps it so.
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let again = count(&mut s, json!({"kind": "conduit", "system": "E-CIRCUIT-EST", "group_by": "circuit"}));
    assert_eq!(again["rows"], left["rows"], "{again}");
}
