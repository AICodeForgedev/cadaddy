//! The `entities` query (control channel `engine.execute`, MCP `query_entities`) rejects malformed
//! filters instead of ignoring them and returning everything.

use cadaddy_engine::Session;
use serde_json::json;

#[test]
fn malformed_filters_are_errors() {
    let mut s = Session::new();
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    s.execute("circle", &json!({"center": [100, 100], "radius": 1})).unwrap();
    for p in [
        json!({"window": [-5, -5, 5, 5]}),
        json!({"window": [[-5, -5]]}),
        json!({"window": [[-5, -5], [5, 5], [9, 9]]}),
        json!({"window": {"min": [-5, -5], "max": [5, 5]}}),
        json!({"window": "x"}),
        json!({"type": 7}),
        json!({"layer": ["0"]}),
        json!({"limit": -1}),
        json!({"offset": "1"}),
        json!({"crossing": "yes"}),
        json!({"selected": "true"}),
        json!({"selected": 1}),
        json!({"selected": []}),
        json!({"selected": {}}),
    ] {
        assert!(s.execute("entities", &p).is_err(), "{p} must be rejected");
    }
    let r = s.execute("entities", &json!({"window": [[-5, -5], [5, 5]]})).unwrap();
    assert_eq!(r["count"], 1, "{r}");
}

#[test]
fn selection_filter_combines_before_paging_without_changing_state() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "측정", "current": true})).unwrap();
    for x in [0, 10, 20] {
        s.execute("circle", &json!({"center": [x, 0], "radius": 2})).unwrap();
    }
    s.execute("line", &json!({"points": [[0, 0], [3, 4]]})).unwrap();
    let all = s.execute("entities", &json!({})).unwrap();
    let hs: Vec<_> = all["entities"].as_array().unwrap().iter().map(|e| e["handle"].clone()).collect();
    // Leave a redo entry to prove the query doesn't clear redo or create an undo entry.
    s.execute("circle", &json!({"center": [99, 99], "radius": 1})).unwrap();
    s.undo().unwrap();
    s.execute("select", &json!({"handles": [hs[0], hs[2], hs[3]]})).unwrap();
    let before = s.state().unwrap().clone();
    let report = s.execute("drawing.inspect", &json!({})).unwrap();
    let selected = s.execute("entities", &json!({"selected": true})).unwrap();
    assert_eq!(selected["count"], 3);
    assert_eq!(selected["entities"][0]["handle"], hs[0]);
    assert_eq!(selected["entities"][1]["handle"], hs[2]);
    assert_eq!(s.execute("entities", &json!({"selected": false})).unwrap(), all);
    assert_eq!(s.execute("entities", &json!({"selected": null})).unwrap(), all);
    assert_eq!(s.execute("entities", &json!({})).unwrap(), all);
    let page = s.execute("entities", &json!({"selected": true, "type": "circle", "layer": "측정", "limit": 1, "offset": 1})).unwrap();
    assert_eq!(page, json!({"count": 2, "returned": 1, "offset": 1, "entities": [all["entities"][2]]}));
    let window = json!({"selected": true, "type": "circle", "layer": "측정", "window": [[-3, -3], [3, 3]], "crossing": false});
    assert_eq!(s.execute("entities", &window).unwrap()["count"], 1);
    assert_eq!(s.execute("entities", &json!({"selected": true, "layer": "other"})).unwrap()["count"], 0);
    assert_eq!(s.execute("entities", &json!({"selected": true, "limit": 0})).unwrap()["returned"], 0);
    assert_eq!(s.execute("entities", &json!({"selected": true, "offset": 99})).unwrap()["returned"], 0);
    assert!(s.execute("entities", &json!({"selected": "true"})).is_err());
    assert_eq!(s.execute("drawing.inspect", &json!({})).unwrap(), report);
    let after = s.state().unwrap();
    assert!(std::sync::Arc::ptr_eq(&before.doc, &after.doc));
    assert!(std::sync::Arc::ptr_eq(&before.saved, &after.saved));
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.selection, after.selection);
    assert_eq!(before.previous_selection, after.previous_selection);
    assert_eq!(before.is_dirty(), after.is_dirty());
    for (a, b) in [(&before.undo, &after.undo), (&before.redo, &after.redo)] {
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b) {
            assert_eq!(a.label, b.label);
            assert_eq!(a.selection, b.selection);
            assert!(std::sync::Arc::ptr_eq(&a.doc, &b.doc));
        }
    }
}

#[test]
fn selection_is_empty_by_default_and_limited_to_active_space() {
    use cadaddy_doc::{Handle, Space};
    let mut s = Session::new();
    s.execute("circle", &json!({"center": [0, 0], "radius": 2})).unwrap();
    let model = s.doc().unwrap().model.iter().next().unwrap().handle;
    assert_eq!(s.execute("entities", &json!({"selected": true})).unwrap()["count"], 0);
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    s.execute("circle", &json!({"center": [10, 10], "radius": 2})).unwrap();
    let paper = s.doc().unwrap().space(&Space::Paper("Layout1".into())).unwrap().iter().last().unwrap().handle;
    // Even stale, duplicate and foreign-space selection handles cannot leak entities.
    s.state_mut().unwrap().selection = vec![model, paper, paper, Handle(u64::MAX)];
    let page = s.execute("entities", &json!({"selected": true})).unwrap();
    assert_eq!(page["count"], 1);
    assert_eq!(page["entities"][0]["handle"], paper.hex());
    s.state_mut().unwrap().space = Space::Model;
    let page = s.execute("entities", &json!({"selected": true})).unwrap();
    assert_eq!(page["count"], 1);
    assert_eq!(page["entities"][0]["handle"], model.hex());
    s.state_mut().unwrap().selection.clear();
    assert_eq!(s.execute("entities", &json!({"selected": true})).unwrap()["entities"], json!([]));
}

mod filters {
    use cadaddy_engine::Session;
    use cadaddy_engine::doc::{Common, EntityKind, Line, Mesh, MeshFace, MeshKind, Space};
    use cadaddy_engine::geom::Vec3;
    use serde_json::{Value, json};

    fn on(layer: &str) -> Common {
        Common { layer: layer.into(), ..Common::default() }
    }

    fn line(s: &mut Session, layer: &str, a: [f64; 3], b: [f64; 3]) {
        let l = Line { a: Vec3::new(a[0], a[1], a[2]), b: Vec3::new(b[0], b[1], b[2]) };
        s.doc_mut().unwrap().add(&Space::Model, on(layer), EntityKind::Line(l)).unwrap();
    }

    /// A closed box of 8 vertices and 6 faces from `lo` to `hi`, as a polyface mesh.
    fn polyface(s: &mut Session, layer: &str, lo: [f64; 3], hi: [f64; 3]) {
        let vertices = (0..8u8)
            .map(|i| {
                let pick = |bit: u8, k: usize| if i & bit == 0 { lo[k] } else { hi[k] };
                Vec3::new(pick(1, 0), pick(2, 1), pick(4, 2))
            })
            .collect();
        let faces = [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]]
            .iter()
            .map(|f| MeshFace { v: f.to_vec(), hidden: 0 })
            .collect();
        let m = Mesh { vertices, faces, kind: MeshKind::Polyface };
        s.doc_mut().unwrap().add(&Space::Model, on(layer), EntityKind::Mesh(m)).unwrap();
    }

    /// A drawing with objects on made-up layers whose names hold wildcard characters.
    fn sample() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).ok();
        line(&mut s, "3D-MEP-EQUIP-Level1", [0.0, 0.0, 1.0], [3.0, 4.0, 5.0]);
        line(&mut s, "3D-MEP-EQUIP-Level2", [10.0, 0.0, 0.0], [12.0, 0.0, 0.0]);
        line(&mut s, "A-WALL", [0.0, 10.0, 0.0], [5.0, 10.0, 0.0]);
        line(&mut s, "E-1#2", [0.0, 20.0, 0.0], [5.0, 20.0, 0.0]);
        polyface(&mut s, "3D-MEP-EQUIP-Level1", [100.0, 200.0, 8.0], [104.0, 203.0, 10.5]);
        s
    }

    fn count(s: &mut Session, p: Value) -> u64 {
        let r = s.execute("entities", &p).unwrap();
        r["count"].as_u64().unwrap_or_else(|| panic!("{p}: {r}"))
    }

    #[test]
    fn layer_takes_wildcards_and_comma_lists_in_any_case() {
        let mut s = sample();
        for (pattern, want) in [
            ("3D-MEP-EQUIP-Level*", 3),
            ("3d-mep-equip-level*", 3),
            ("*-Level#", 3),
            ("*-Level2", 1),
            ("3D.MEP*", 3),
            ("@-WALL", 1),
            ("A-WALL,E-1#2", 2),
            (" a-wall ,  e-1#2 ", 2),
            ("E-1#2", 1),
            ("e-1`#2", 1),
            ("[AE]-*", 2),
            ("[~AE]*", 3),
            ("~3D*", 2),
            ("?-WALL", 1),
            ("A-WALL", 1),
        ] {
            assert_eq!(count(&mut s, json!({"layer": pattern})), want, "layer {pattern}");
        }
        let r = s.execute("entities", &json!({"layer": "3D-MEP-EQUIP-Level*"})).unwrap();
        assert!(r.get("warning").is_none(), "{r}");
    }

    #[test]
    fn a_layer_filter_no_layer_matches_says_so() {
        let mut s = sample();
        for pattern in ["3D-MEP-EQUIP-Level", "Nope*", "A-WALL;E-1#2", ""] {
            let r = s.execute("entities", &json!({"layer": pattern})).unwrap();
            assert_eq!(r["count"], 0, "{pattern}: {r}");
            let w = r["warning"].as_str().unwrap_or_else(|| panic!("{pattern}: no warning in {r}"));
            assert!(w.contains("No layer in the drawing matches"), "{w}");
        }
        // A layer that exists but holds nothing in the window is a plain 0: the filter matched.
        let r = s.execute("entities", &json!({"layer": "A-WALL", "window": [[500, 500], [600, 600]]})).unwrap();
        assert_eq!(r["count"], 0);
        assert!(r.get("warning").is_none(), "{r}");
        // An empty layer in the layer table matches too.
        s.execute("layer.new", &json!({"name": "Spare"})).unwrap();
        let r = s.execute("entities", &json!({"layer": "spare"})).unwrap();
        assert!(r.get("warning").is_none(), "{r}");
    }

    #[test]
    fn type_takes_wildcards_class_names_and_comma_lists() {
        let mut s = sample();
        s.execute("circle", &json!({"center": [50, 50], "radius": 2})).unwrap();
        for (pattern, want) in
            [("line", 4), ("LINE", 4), ("AcDbLine", 4), ("line,circle", 5), ("Circ*", 1), ("AcDb*Mesh", 1), ("~Line", 2), ("[lc]i*", 5)]
        {
            assert_eq!(count(&mut s, json!({"type": pattern})), want, "type {pattern}");
        }
        let r = s.execute("entities", &json!({"type": "Lien"})).unwrap();
        assert_eq!(r["count"], 0);
        let w = r["warning"].as_str().unwrap_or_else(|| panic!("no warning in {r}"));
        assert!(w.contains("of a type matching \"Lien\"") && w.contains("Line") && w.contains("Circle"), "{w}");
        // Both filters can warn at once.
        let r = s.execute("entities", &json!({"type": "Lien", "layer": "Nope"})).unwrap();
        let w = r["warning"].as_str().unwrap_or_default();
        assert!(w.contains("No layer") && w.contains("Lien"), "{w}");
    }

    #[test]
    fn hostile_patterns_are_refused_or_finish() {
        let mut s = sample();
        let long = "*".repeat(2000);
        assert!(s.execute("entities", &json!({"layer": long})).is_err());
        // Many stars against a long name: a quick answer or a refusal, never a hang.
        let name = "a".repeat(200);
        line(&mut s, &name, [0.0, 0.0, 0.0], [1.0, 1.0, 0.0]);
        let started = std::time::Instant::now();
        let r = s.execute("entities", &json!({"layer": format!("{}b", "*a".repeat(40))}));
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
        if let Ok(r) = r {
            assert_eq!(r["count"], 0, "{r}");
        }
    }

    #[test]
    fn every_entity_carries_its_3d_extents() {
        let mut s = sample();
        s.execute("circle", &json!({"center": [50, 60], "radius": 2})).unwrap();
        let r = s.execute("entities", &json!({"layer": "3D-MEP-EQUIP-Level1"})).unwrap();
        let ents = r["entities"].as_array().unwrap();
        let ext = |ty: &str| ents.iter().find(|e| e["type"] == ty).map(|e| e["extents"].clone()).unwrap_or_else(|| panic!("{ty} in {r}"));
        assert_eq!(ext("Line"), json!({"min": [0.0, 0.0, 1.0], "max": [3.0, 4.0, 5.0]}));
        let mesh = ents.iter().find(|e| e["type"] != "Line").unwrap();
        assert_eq!(mesh["extents"], json!({"min": [100.0, 200.0, 8.0], "max": [104.0, 203.0, 10.5]}), "{mesh}");
        let r = s.execute("entities", &json!({"type": "circle"})).unwrap();
        assert_eq!(r["entities"][0]["extents"], json!({"min": [48.0, 58.0, 0.0], "max": [52.0, 62.0, 0.0]}));
        s.execute("box", &json!({"corner": [5, 6, 7], "length": 2, "width": 3, "height": 4})).unwrap();
        let r = s.execute("entities", &json!({"type": "AcDb3dSolid"})).unwrap();
        let e = &r["entities"][0]["extents"];
        let close = |v: &Value, want: [f64; 3]| (0..3).all(|i| (v[i].as_f64().unwrap() - want[i]).abs() < 1e-6);
        assert!(close(&e["min"], [5.0, 6.0, 7.0]) && close(&e["max"], [7.0, 9.0, 11.0]), "{e}");
    }

    #[test]
    fn list_prints_where_a_3d_object_is() {
        let mut s = sample();
        let r = s.execute("entities", &json!({"type": "AcDb*Mesh"})).unwrap();
        let h = r["entities"][0]["handle"].clone();
        let text = s.execute("list", &json!({"handles": [h]})).unwrap()["message"].as_str().unwrap().to_string();
        assert!(text.contains("Polyface mesh"), "{text}");
        assert!(text.contains("Vertex count = 8") && text.contains("Face count = 6"), "{text}");
        assert!(text.contains("Lower bound X=100.0000  Y=200.0000  Z=8.0000"), "{text}");
        assert!(text.contains("Upper bound X=104.0000  Y=203.0000  Z=10.5000"), "{text}");
        assert!(!text.contains("items)"), "{text}");
        s.execute("box", &json!({"corner": [5, 6, 7], "length": 2, "width": 3, "height": 4})).unwrap();
        let r = s.execute("entities", &json!({"type": "3d solid"})).unwrap();
        let h = r["entities"][0]["handle"].clone();
        let text = s.execute("list", &json!({"handles": [h]})).unwrap()["message"].as_str().unwrap().to_string();
        assert!(text.contains("Lower bound X=5.0000  Y=6.0000  Z=7.0000"), "{text}");
        assert!(text.contains("Upper bound X=7.0000  Y=9.0000  Z=11.0000"), "{text}");
    }

    #[test]
    fn select_class_takes_the_same_wildcards() {
        let mut s = sample();
        for (pattern, want) in [("@cDbLine", 4), ("AcDb*Mesh,circle", 1), ("[l]ine", 4), ("~AcDbLine", 1), ("line, AcDbPolyFaceMesh", 5)] {
            let r = s.execute("select", &json!({"class": pattern})).unwrap();
            assert_eq!(r["selected"], want, "class {pattern}: {r}");
        }
    }
}
