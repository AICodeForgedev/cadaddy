//! Q6551 fix-gfci-load-side-by-distance: a GFCI-tagged receptacle is built standard on the load
//! side of another GFCI receptacle only where its routed circuit reaches it through that one (the
//! route's wiring order, line to load), never because the other is nearer the panel; otherwise it
//! keeps its drawn GFCI. Validate judges the same way.

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

const REPORT: &str = "route/report.json";

/// A receptacle tag at (x, y) feet on LP1-1, its plan type `mark`.
fn receptacle(n: usize, x: f64, y: f64, mark: &str) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": mark, "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x1F0 + n), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "circuit": {"value": "LP1-1", "source": "plan"},
        "mount_height_in": {"value": "18", "source": "note"}
    }})
}

/// A made-up storey: panel LP1 (its middle at (-1.2, 0) ft) and circuit LP1-1 on an AFCI (not
/// GFCI) breaker with a standard receptacle R by the panel and two receptacles the plan tags GFCI:
/// N, the nearer the panel in a straight line, and F, the nearer along the square route (the
/// router's homerun box is R, its next F). `wide`: N at (12, 12) and F at (21, 0) (17.8 against
/// 22.2 ft in a straight line), a chain past what #12 reaches, so the router branches at R
/// (R → F, R → N); else R at (2, 0), N at (4, 3) and F at (6, 0) (6.0 against 7.2 ft), chained R → F → N.
fn building(wide: bool) -> Session {
    let (r, n, f) = if wide { ((5.0, 0.0), (12.0, 12.0), (21.0, 0.0)) } else { ((2.0, 0.0), (4.0, 3.0), (6.0, 0.0)) };
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    // One room round it all (a 6 in shell, its tag inside): what Validate checks by.
    layer(&mut s, "A-WALL");
    // (Its walls far from the receptacles: none is set on a wall face.)
    rect(&mut s, [-246.0, -246.0], [486.0, 366.0]);
    rect(&mut s, [-240.0, -240.0], [480.0, 360.0]);
    layer(&mut s, "A-ROOM-TAG");
    s.execute("text", &json!({"at": [120.0, 300.0], "text": "101", "height": 6.0})).expect("tag");
    layer(&mut s, "0");
    let circuit = json!({"id": "circuit:L1:E-101:LP1-1", "kind": "circuit", "level": "L1", "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": "LP1-1", "source": "plan"},
        "panel": {"value": "LP1", "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "conductors": {"value": "2#12 + 1#12G", "source": "default"},
        "conduit": {"value": "3/4\" EMT", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }});
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Shell"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Shell", "thickness_in": 6}],
        "rooms": [{"name": "Open Office", "number": "101", "level": "L1"}],
        "tags": [receptacle(1, r.0, r.1, "A"), receptacle(2, n.0, n.1, "GFCI"), receptacle(3, f.0, f.1, "GFCI"), circuit],
        "mep": {
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
                 "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
            ],
            "panel_schedules": [{"panel": "LP1", "source": "E-601", "legend": {"GF": "GROUND FAULT INTERRUPTER", "AF": "ARC FAULT"},
                "circuits": [{"number": "1", "description": "RECEPTACLES", "breaker": "20", "poles": 1, "protection": ["AF"], "source": "E-601"}]}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s.execute("bim.props.fill", &json!({})).expect("fill");
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

/// The device elements by their x (feet): R, N, F.
fn devices(s: &Session) -> [String; 3] {
    let d = s.doc().expect("doc");
    let mut at: Vec<(f64, String)> = d
        .model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device"))
        .map(|e| {
            let p = cadaddy_engine::doc::props::read(&e.common.xdata);
            let x = p
                .get("location")
                .map(|l| l.text())
                .unwrap_or_default()
                .split(',')
                .next()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(f64::NAN);
            (x, e.handle.hex())
        })
        .collect();
    at.sort_by(|a, b| a.0.total_cmp(&b.0));
    let h: Vec<String> = at.into_iter().map(|x| x.1).collect();
    assert_eq!(h.len(), 3, "three receptacles built: {h:?}");
    [h[0].clone(), h[1].clone(), h[2].clone()]
}

fn props(s: &mut Session, h: &str) -> Value {
    s.execute("bim.props.get", &json!({"handle": h})).expect("get")["elements"][0]["props"].clone()
}

fn report(s: &Session) -> Value {
    let d = s.doc().expect("doc");
    serde_json::from_slice(d.embedded.get(REPORT).expect("route report")).expect("json")
}

fn set_report(s: &mut Session, r: &Value) {
    let b = serde_json::to_vec(r).expect("bytes");
    s.doc_mut().expect("doc").embedded.insert(REPORT.into(), Arc::new(b));
}

fn gfci_finding(s: &mut Session, h: &str) -> Value {
    let r = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": ["G-PRODUCT-CODE"]})).expect("prescreen");
    r["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["check"] == "gfci" && f["handle"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(h)))
        .cloned()
        .unwrap_or_else(|| panic!("no gfci finding on {h}: {r:#}"))
}

/// The joints the route stored for LP1-1 (line side, load side) and its homerun's box.
fn wiring(s: &Session) -> (String, Vec<(String, String)>) {
    let rep = report(s);
    let w = rep["wiring"]["circuits"].as_array().cloned().unwrap_or_default();
    let row = w.iter().find(|c| c["label"] == "LP1-1").cloned().unwrap_or_else(|| panic!("LP1-1's wiring stored: {:#}", rep["wiring"]));
    let up = |v: &Value| v.as_str().unwrap_or_default().to_ascii_uppercase();
    let joints = row["joints"].as_array().into_iter().flatten().map(|j| (up(&j[0]), up(&j[1]))).collect();
    (up(&row["home"]), joints)
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_ascii_uppercase(), b.to_ascii_uppercase())
}

#[test]
fn a_gfci_tagged_receptacle_drops_its_gfci_only_behind_one_upstream_on_its_route() {
    let mut s = building(false);
    let [r, n, f] = devices(&s);
    // The route stores its wiring order: the homerun lands in R, then R → F → N.
    let (home, joints) = wiring(&s);
    assert_eq!(home, r.to_ascii_uppercase(), "the homerun's box is R");
    assert_eq!(joints, vec![pair(&r, &f), pair(&f, &n)], "chained R → F → N");
    // F, first on the route, is the GFCI device; N, the nearer the panel in a straight line, is
    // fed through it (line to load), so it is the one built standard.
    let c = s.execute("bim.coderules", &json!({})).expect("coderules");
    assert_eq!(c["gfci"]["device"], json!(1), "{c:#}");
    assert_eq!(c["gfci"]["load_side"], json!(1), "{c:#}");
    let pf = props(&mut s, &f);
    assert_eq!(pf["gfci"]["value"], "device", "F: {pf:#}");
    assert_eq!(pf["receptacle_type"]["value"], "GFCI", "F: {pf:#}");
    assert!(pf["protected_by"].is_null(), "F: {pf:#}");
    assert!(pf["design_value"].is_null(), "F is built as drawn: {pf:#}");
    let pn = props(&mut s, &n);
    assert_eq!(pn["gfci"]["value"], "load side", "N: {pn:#}");
    assert_eq!(pn["receptacle_type"]["value"], "standard", "N: {pn:#}");
    assert!(pn["protected_by"]["value"].as_str().is_some_and(|p| p.contains(&f)), "N is protected by F: {pn:#}");
    assert!(pn["gfci"]["source"]["doc"].as_str().is_some_and(|t| t.contains("upstream") && t.contains("as routed")), "{pn:#}");
    assert_eq!(pn["design_value"]["value"], "GFCI", "the drawn GFCI kept beside: {pn:#}");
    assert_eq!(pn["corrected_value"]["value"], "standard");
    // Validate agrees: N is protected from F upstream of it on its routed circuit.
    let v = gfci_finding(&mut s, &n);
    assert_eq!(v["verdict"], "pass", "{v:#}");
    assert!(v["measured"]["protected_by"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&f)), "{v:#}");

    // Routed again on a branch that never passes the other (R feeds F and N each on its own
    // joint): neither is upstream of the other, so each keeps its GFCI and the earlier
    // correction goes.
    let mut branched = report(&s);
    if let Some(rows) = branched["wiring"]["circuits"].as_array_mut() {
        for row in rows.iter_mut().filter(|c| c["label"] == "LP1-1") {
            row["joints"] = json!([[r, f], [r, n]]);
        }
    }
    set_report(&mut s, &branched);
    let c = s.execute("bim.coderules", &json!({})).expect("coderules");
    assert_eq!(c["gfci"]["device"], json!(2), "{c:#}");
    assert!(c["gfci"]["load_side"].is_null(), "{c:#}");
    let pn = props(&mut s, &n);
    assert_eq!(pn["gfci"]["value"], "device", "N: {pn:#}");
    assert_eq!(pn["receptacle_type"]["value"], "GFCI", "N: {pn:#}");
    assert!(pn["protected_by"].is_null(), "no stale protector: {pn:#}");
    assert!(pn["design_value"].is_null() && pn["corrected_value"].is_null(), "built as drawn again: {pn:#}");
    // Validate fails it built standard there (still tagged GFCI): its route passes no GFCI
    // receptacle.
    s.execute("bim.props.set", &json!({"handle": n, "key": "receptacle_type", "value": "standard"})).expect("set");
    let v = gfci_finding(&mut s, &n);
    assert_eq!(v["verdict"], "fail", "{v:#}");
    assert!(v["issue"].as_str().is_some_and(|i| i.contains("routed circuit reaches it through no GFCI")), "{v:#}");
}

#[test]
fn a_gfci_on_its_own_branch_keeps_its_gfci_however_near_the_panel_the_other_is() {
    // The router branches at R (R → F, R → N): F's way never passes N, N's never F.
    let mut s = building(true);
    let [r, n, f] = devices(&s);
    let (home, mut joints) = wiring(&s);
    joints.sort();
    let mut want = vec![pair(&r, &f), pair(&r, &n)];
    want.sort();
    assert_eq!((home, joints), (r.to_ascii_uppercase(), want), "branched at R");
    let c = s.execute("bim.coderules", &json!({})).expect("coderules");
    assert_eq!(c["gfci"]["device"], json!(2), "{c:#}");
    for h in [&n, &f] {
        let p = props(&mut s, h);
        assert_eq!(p["gfci"]["value"], "device", "{h}: {p:#}");
        assert_eq!(p["receptacle_type"]["value"], "GFCI", "{h}: {p:#}");
        assert!(p["design_value"].is_null(), "built as drawn: {p:#}");
    }
    // Validate passes both as GFCI devices.
    for h in [&n, &f] {
        assert_eq!(gfci_finding(&mut s, h)["verdict"], "pass");
    }
}

#[test]
fn with_no_route_stored_each_keeps_its_drawn_gfci() {
    // Which is first isn't known, so each keeps its drawn GFCI (never by distance).
    let mut s = building(false);
    let [_, n, f] = devices(&s);
    let mut none = report(&s);
    if let Some(o) = none.as_object_mut() {
        o.remove("wiring");
    }
    set_report(&mut s, &none);
    let c = s.execute("bim.coderules", &json!({})).expect("coderules");
    assert_eq!(c["gfci"]["device_each"], json!(2), "{c:#}");
    for h in [&n, &f] {
        let p = props(&mut s, h);
        assert_eq!(p["receptacle_type"]["value"], "GFCI", "{h}: {p:#}");
    }
}
