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
fn a_long_circuits_legs_raceway_is_sized_for_the_legs_own_conductors() {
    // LP1-7's four receptacles 150' from the panel: its voltage drop puts a larger conductor on
    // the homerun; the legs keep a device's size, and their raceway is what those fill (the
    // 3/4" minimum here), not the homerun's.
    let mut s = building(150.0, 10.0, 4, Vec::new(), Vec::new());
    let r = s.execute("conduit.route", &json!({"section": "conductors_upsized"})).expect("report");
    let up = r["conductors_upsized"].as_array().and_then(|a| a.iter().find(|u| u["circuit"] == json!("LP1-7")).cloned()).unwrap_or_default();
    assert!(up.is_object(), "the long circuit is upsized: {r}");
    let ft = |v: &Value, size: &str| {
        v["rows"].as_array().and_then(|a| a.iter().find(|x| x["size"] == json!(size)).and_then(|x| x["length_ft"].as_f64())).unwrap_or(0.0)
    };
    let c = count(&mut s, json!({"kind": "conduit", "circuit": "LP1-7", "group_by": "size"}));
    let total = c["length_ft"].as_f64().unwrap_or(0.0);
    assert!(ft(&c, "3/4\"") > 60.0, "the legs in the 3/4\" minimum: {c}");
    assert!(ft(&c, "3/4\"") + ft(&c, "1\"") >= total - 0.01, "nothing over 1\": {c}");
    // Validate's fill check agrees with what each raceway carries.
    let fs = screen(&mut s, &["E-CONDUIT-FILL"]);
    assert!(!fs.is_empty() && fs.iter().all(|f| f["verdict"] == json!("pass")), "{fs:?}");
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
    // LP1-7 is tagged on its plan and again on an enlarged plan, each read with an estimated
    // route. The routing routes the circuit by the plan's tag (its route is by the devices): the
    // enlargement's estimate is the same conduit and goes; another circuit's estimate stays.
    let est = |id: &str, y: f64| {
        json!({"system": "E-CIRCUIT-EST", "level": "L1", "id": format!("{id}#1"), "points": [[1800.0, y], [2100.0, y]], "size_in": [0.75, 0], "bottoms_ft": [9.25],
               "size_source": "default", "height_source": "estimated route", "source_sheet": "E-601"})
    };
    let mut s = building(
        150.0,
        10.0,
        4,
        vec![circuit("E-601", "LP1-7"), circuit("E-601", "LP1-9")],
        // The plan's estimate along the devices (they join its tag), the enlargement's 25' off.
        vec![est("circuit:L1:E-101:LP1-7", 120.0), est("circuit:L1:E-601:LP1-7", 420.0), est("circuit:L1:E-601:LP1-9", 200.0)],
    );
    let left = count(&mut s, json!({"kind": "conduit", "system": "E-CIRCUIT-EST", "group_by": "circuit"}));
    let circuits: Vec<String> =
        left["rows"].as_array().map(|a| a.iter().filter_map(|x| x["circuit"].as_str().map(String::from)).collect()).unwrap_or_default();
    assert_eq!(circuits, ["LP1-9"], "{left}");
    // The one left is the raceway its conductors fill at the routing's minimum (the rules'
    // 3/4" here), not the reader's default.
    let size = count(&mut s, json!({"kind": "conduit", "system": "E-CIRCUIT-EST", "group_by": "size"}));
    assert_eq!(size["rows"].as_array().map(|a| a.iter().map(|x| x["size"].clone()).collect::<Vec<_>>()), Some(vec![json!("3/4\"")]), "{size}");
    // A specification's 1/2" minimum, rerouted: the circuit still has one route, and the estimate
    // left takes the 1/2" its 2#12 + 1#12G fill.
    let methods = json!({"min_size_in": 0.5, "min_size_source": "spec 26 05 33: minimum raceway size 1/2 in"});
    if let Ok(d) = s.doc_mut() {
        d.embedded.insert("route/methods.json".into(), std::sync::Arc::new(serde_json::to_vec(&methods).unwrap_or_default()));
    }
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let again = count(&mut s, json!({"kind": "conduit", "system": "E-CIRCUIT-EST", "group_by": "circuit"}));
    assert_eq!(again["rows"].as_array().map(|a| a.iter().map(|x| x["circuit"].clone()).collect::<Vec<_>>()), Some(vec![json!("LP1-9")]), "{again}");
    let size = count(&mut s, json!({"kind": "conduit", "system": "E-CIRCUIT-EST", "group_by": "size"}));
    assert_eq!(size["rows"].as_array().map(|a| a.iter().map(|x| x["size"].clone()).collect::<Vec<_>>()), Some(vec![json!("1/2\"")]), "{size}");
}
