//! Q6592: every piece a run is built as (a conduit stick, bend, fitting, box, strap, wire; a duct
//! or pipe segment and its fittings) carries its elevation, measured off its own built body:
//! the bottom above its level's floor, and for a vertical piece its top too. It is written when
//! the piece is cut, again on every rebuild, and `bim.props.fill` keeps it current after an edit.
//! A routed conduit piece's is computed (derived); a duct's or pipe's says its run's height
//! source, so an assumed height stays a default.

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, SourceKind};
use cadaddy_engine::doc::{Drawing, EntityKind, Handle};
use serde_json::{Value, json};

use super::mepatoms::building;

/// Each MEP piece of the model of kind `kind` (its `kind:` tag): handle, props, its body's z range.
fn pieces(s: &Session, kind: &str) -> Vec<(Handle, props::Props, f64, f64)> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::bimlook::made_tag(e, "kind") == Some(kind))
        .filter_map(|e| {
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let lo = m.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
            let hi = m.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
            Some((e.handle, props::read(&e.common.xdata), lo, hi))
        })
        .collect()
}

fn elevation(ps: &props::Props, key: &str) -> Option<f64> {
    ps.get(key).and_then(|p| p.text().parse::<f64>().ok())
}

fn text(ps: &props::Props, key: &str) -> String {
    ps.get(key).map(Prop::text).unwrap_or_default()
}

/// The conduit pieces have no elevation unless each matches its own body, above `floor_ft`.
fn assert_each_piece_at_its_body(s: &Session, kind: &str, floor_ft: f64, source: SourceKind) -> usize {
    let all = pieces(s, kind);
    assert!(!all.is_empty(), "no {kind} pieces");
    for (h, ps, lo, hi) in &all {
        let what = format!("{kind} piece {} ({}) {:?}", h.hex(), text(ps, "atom"), ps);
        let el = elevation(ps, "elevation").unwrap_or_else(|| panic!("no elevation on {what}"));
        let p = ps.get("elevation").expect("elevation");
        assert_eq!(p.source.kind, source, "{what}");
        assert_eq!(p.unit.as_deref(), Some("ft"), "{what}");
        assert!((el - (lo / 12.0 - floor_ft)).abs() < 0.011, "its body's bottom above the level: {el} vs {lo} on {what}");
        if text(ps, "vertical") == "true" {
            let top = elevation(ps, "elevation_top").unwrap_or_else(|| panic!("a vertical piece's top: {what}"));
            assert!((top - (hi / 12.0 - floor_ft)).abs() < 0.011, "{top} vs {hi} on {what}");
            assert!(top > el, "{what}");
        }
    }
    all.len()
}

#[test]
fn every_conduit_piece_carries_its_elevation_from_its_own_body() {
    let mut s = building(None);
    let n = assert_each_piece_at_its_body(&s, "conduit", 0.0, SourceKind::Computed);
    // Every kind of piece: sticks, couplings, connectors, straps, the box, the wire.
    let all = pieces(&s, "conduit");
    let kinds: std::collections::BTreeSet<String> = all.iter().map(|(_, ps, _, _)| text(ps, "atom")).collect();
    for k in ["conduit", "coupling", "connector", "box", "wire"] {
        assert!(kinds.contains(k), "{k} in {kinds:?}");
    }
    // The drawn 1" run at its tagged 10' bottom; the drop to the receptacle box is vertical,
    // its top at the route's 9' and its bottom at the box.
    let stick = all.iter().find(|(_, ps, _, _)| text(ps, "atom") == "conduit" && text(ps, "size") == "1\"").expect("a 1\" stick");
    let el = elevation(&stick.1, "elevation").expect("elevation");
    assert!((9.9..=10.1).contains(&el), "the drawn run's bottom: {el}");
    let verticals: Vec<(f64, f64, String)> = all
        .iter()
        .filter(|(_, ps, _, _)| text(ps, "atom") == "conduit" && text(ps, "vertical") == "true")
        .map(|(_, ps, _, _)| (elevation(ps, "elevation").unwrap_or(-1.0), elevation(ps, "elevation_top").unwrap_or(-1.0), text(ps, "circuit")))
        .collect();
    // The feeder into the panel rises from its top (2' up, 4' tall) to the top of the storey.
    assert!(verticals.iter().any(|(lo, hi, _)| (lo - 6.0).abs() < 0.05 && (hi - 12.0).abs() < 0.05), "the panel's feeder: {verticals:?}");
    // The receptacle's box at its 54-1/8" mounting height: its bottom a little below.
    let bx = all.iter().find(|(_, ps, _, _)| text(ps, "atom") == "box").map(|(_, ps, _, _)| elevation(ps, "elevation").unwrap_or(-1.0));
    assert!(bx.is_some_and(|b| (3.5..=4.6).contains(&b)), "the box: {bx:?}");
    // The object form counts it as derived, on every piece.
    let c = s.execute("bim.completeness", &json!({"kind": "conduit"})).expect("completeness");
    let f = &c["kinds"]["conduit"]["fields"]["elevation"];
    assert_eq!(f["pct"].as_f64(), Some(100.0), "{f}");
    assert_eq!(f["derived"].as_u64(), Some(n as u64), "{f}");
    // And a stick's object shows it.
    let g = s.execute("bim.props.get", &json!({"handle": stick.0.hex()})).expect("get");
    assert!(g.to_string().contains("\"elevation\""), "{g}");
}

/// A made-up two-storey building: a drawn conduit, a supply duct and a water pipe on the upper
/// floor (12' above the lower), and a riser through both.
fn two_storeys(conduit_bottom_ft: f64) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}
        ],
        "mep": {
            "systems": [
                {"name": "E-COND", "layer": "E-COND", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 9},
                {"name": "SA", "layer": "M-DUCT-SUPPLY", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9},
                {"name": "DCW", "layer": "P-DOMW", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 9}
            ],
            "runs": [
                {"system": "E-COND", "level": "L2", "points": [[0, 600], [444, 600]], "size_in": [1, 0], "bottoms_ft": [conduit_bottom_ft], "size_source": "tag", "height_source": "tag"},
                {"system": "SA", "level": "L2", "points": [[0, 300], [360, 300]], "size_in": [12, 8], "bottoms_ft": [9.5], "height_source": "tag"},
                {"system": "DCW", "level": "L2", "points": [[0, 100], [240, 100], [240, 340]], "size_in": [1, 0], "bottoms_ft": [8, 8], "height_source": "tag"},
                {"system": "DCW", "level": "L1", "points": [[480, 100]], "size_in": [1, 0], "bottoms_ft": [1], "top_ft": 10, "height_source": "tag"}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
    s
}

#[test]
fn pieces_on_an_upper_floor_are_measured_from_their_own_level() {
    let s = two_storeys(10.0);
    assert_each_piece_at_its_body(&s, "conduit", 12.0, SourceKind::Computed);
    let stick = pieces(&s, "conduit").into_iter().find(|(_, ps, _, _)| text(ps, "atom") == "conduit").expect("a stick");
    let el = elevation(&stick.1, "elevation").unwrap_or(-1.0);
    assert!((9.9..=10.1).contains(&el), "above its level, not above the ground floor: {el}");
}

#[test]
fn duct_and_pipe_pieces_carry_their_elevation_from_the_build() {
    let mut s = two_storeys(10.0);
    // Straight from the build, before any bim.props.fill; its source the run's height's (read
    // from a tag here: a print's).
    assert_each_piece_at_its_body(&s, "duct", 12.0, SourceKind::Print);
    let ducts = pieces(&s, "duct");
    assert!(ducts.iter().any(|(_, ps, _, _)| elevation(ps, "elevation").is_some_and(|e| (e - 9.5).abs() < 0.05)), "{ducts:?}");
    let pipes = pieces(&s, "pipe");
    assert!(!pipes.is_empty());
    for (h, ps, lo, hi) in &pipes {
        let el = elevation(ps, "elevation").unwrap_or_else(|| panic!("no elevation on pipe piece {} {ps:?}", h.hex()));
        assert_eq!(ps.get("elevation").map(|p| p.source.kind), Some(SourceKind::Print), "{ps:?}");
        // The upper floor's pipe is measured from 12'; the riser from the ground floor, its top too.
        let floor = if *lo >= 12.0 * 12.0 - 1.0 { 12.0 } else { 0.0 };
        assert!((el - (lo / 12.0 - floor)).abs() < 0.011, "{el} {lo} {ps:?}");
        if *hi - *lo > 5.0 * 12.0 {
            let top = elevation(ps, "elevation_top").unwrap_or(-1.0);
            assert!((top - 10.0).abs() < 0.05, "the riser's top: {top} {ps:?}");
        }
    }
    // The form counts them filled; the fill afterwards agrees with the build.
    let c = s.execute("bim.completeness", &json!({"kind": "duct"})).expect("completeness");
    assert_eq!(c["kinds"]["duct"]["fields"]["elevation"]["pct"].as_f64(), Some(100.0), "{c}");
    let before = pieces(&s, "duct").into_iter().map(|(_, ps, ..)| ps.get("elevation").map(|p| (p.text(), p.source.kind))).collect::<Vec<_>>();
    s.execute("bim.props.fill", &json!({})).expect("fill");
    let after = pieces(&s, "duct").into_iter().map(|(_, ps, ..)| ps.get("elevation").map(|p| (p.text(), p.source.kind))).collect::<Vec<_>>();
    assert_eq!(before, after);
}

#[test]
fn a_rebuild_at_another_height_writes_each_piece_again() {
    let mut s = two_storeys(10.0);
    // The run's height changes (a re-read, a corrected tag): the pieces are cut again.
    let mut spec: Value = serde_json::from_str(s.doc().expect("doc").building_spec.as_deref().expect("spec")).expect("json");
    spec["mep"]["runs"][0]["bottoms_ft"] = json!([11.0]);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("rebuild");
    assert_each_piece_at_its_body(&s, "conduit", 12.0, SourceKind::Computed);
    let stick = pieces(&s, "conduit").into_iter().find(|(_, ps, _, _)| text(ps, "atom") == "conduit").expect("a stick");
    let el = elevation(&stick.1, "elevation").unwrap_or(-1.0);
    assert!((10.9..=11.1).contains(&el), "the new height: {el}");
}

#[test]
fn a_reroute_writes_the_new_pieces_elevation() {
    let mut s = building(None);
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    assert_each_piece_at_its_body(&s, "conduit", 0.0, SourceKind::Computed);
}

#[test]
fn props_fill_keeps_a_moved_piece_current() {
    let mut s = building(None);
    let (h, ps, _, _) =
        pieces(&s, "conduit").into_iter().find(|(_, ps, _, _)| text(ps, "atom") == "conduit" && text(ps, "size") == "1\"").expect("a stick");
    let el = elevation(&ps, "elevation").expect("elevation");
    // A person lifts the stick 1' and drops another piece's value; the fill reads both again.
    let (other, _, _, _) = pieces(&s, "conduit").into_iter().find(|(o, ps, _, _)| *o != h && text(ps, "atom") == "coupling").expect("a coupling");
    {
        let d = s.doc_mut().expect("doc");
        d.modify_entity(h, |e| {
            if let EntityKind::Mesh(m) = &mut e.kind {
                for v in m.vertices.iter_mut() {
                    v.z += 12.0;
                }
            }
        })
        .expect("lift");
        d.modify_entity(other, |e| {
            let mut ps = props::read(&e.common.xdata);
            ps.remove("elevation");
            e.common.xdata = props::write(&e.common.xdata, &ps).into();
        })
        .expect("clear");
    }
    s.execute("bim.props.fill", &json!({})).expect("fill");
    assert_each_piece_at_its_body(&s, "conduit", 0.0, SourceKind::Computed);
    let now = pieces(&s, "conduit").into_iter().find(|(o, ..)| *o == h).map(|(_, ps, ..)| elevation(&ps, "elevation").unwrap_or(0.0));
    assert!(now.is_some_and(|n| (n - el - 1.0).abs() < 0.011), "{el} -> {now:?}");
    // A person's own value is theirs: the fill leaves it.
    s.execute("bim.props.set", &json!({"handle": h.hex(), "props": {"elevation": "12.5"}})).expect("set");
    s.execute("bim.props.fill", &json!({})).expect("fill");
    let d = s.doc().expect("doc");
    let kept = props::read(&d.model.get(h).expect("stick").common.xdata);
    assert_eq!(kept.get("elevation").map(|p| p.source.kind), Some(SourceKind::User), "{kept:?}");
}
