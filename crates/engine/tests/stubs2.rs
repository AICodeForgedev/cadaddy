//! Options that used to report "not available yet": DIST/AREA Arc, -RENAME of plot styles and
//! viewport configurations (materials refused with the reason), colour books in colour prompts,
//! and HATCHEDIT recreating boundaries as regions.

use cadaddy_engine::Session;
use cadaddy_engine::doc::color::{Color, Rgb};
use cadaddy_engine::doc::{EntityKind, Handle, PlotStyle, PlotStyleKind, PlotStyleTable, VportConfig};
use serde_json::json;

fn run(lines: &[&str]) -> Session {
    let mut s = Session::new();
    for l in lines {
        s.cmdline(l).unwrap();
    }
    s
}

fn said(s: &Session, text: &str) -> bool {
    s.log.iter().any(|l| l.contains(text))
}

#[test]
fn dist_multiple_points_measures_arcs() {
    // A line of 10, then a half circle of radius 5 tangent to it: 10 + 5π.
    let s = run(&["dist", "0,0", "m", "10,0", "a", "10,10", "t"]);
    assert!(said(&s, "Distance = 25.7080"), "{:?}", s.log);
    // Undo takes the arc back; Line goes straight again.
    let s = run(&["dist", "0,0", "m", "10,0", "a", "10,10", "u", "l", "10,10", "t"]);
    assert!(said(&s, "Distance = 20.0000"), "{:?}", s.log);
}

#[test]
fn area_arc_after_undo_is_straight_again() {
    let s = run(&["area", "0,0", "10,0", "a", "10,10", "u", "l", "10,10", "0,10", ""]);
    assert!(said(&s, "Area = 100.0000"), "{:?}", s.log);
    // Two points joined by a line and a half circle enclose an area too.
    let s = run(&["area", "0,0", "10,0", "a", "0,0", ""]);
    assert!(said(&s, "Area = "), "{:?}", s.log);
}

fn styled_session() -> (Session, String) {
    let mut s = Session::new();
    let h = s.execute("line", &json!({ "points": [[0, 0], [1, 0]] })).unwrap()["handles"][0].as_str().unwrap().to_string();
    let d = s.doc_mut().unwrap();
    d.plot_style_tables.push(PlotStyleTable {
        name: "site.stb".into(),
        description: String::new(),
        kind: PlotStyleKind::Named,
        styles: vec![PlotStyle { name: "Normal".into(), ..Default::default() }, PlotStyle { name: "Thin".into(), ..Default::default() }],
    });
    d.layers.iter_mut().for_each(|l| l.plot_style = "Thin".into());
    d.header.set_str("CPLOTSTYLE", "Thin");
    let hh = Handle::parse_hex(&h).unwrap();
    d.modify_entity(hh, |e| e.common.plot_style = "Thin".into()).unwrap();
    d.vports.push(VportConfig { name: "Two".into(), tiles: Vec::new() });
    (s, h)
}

#[test]
fn rename_plot_styles_and_viewport_configurations() {
    let (mut s, h) = styled_session();
    s.execute("rename", &json!({ "table": "plotstyle", "from": "thin", "to": "Hairline" })).unwrap();
    let d = s.doc().unwrap();
    assert!(d.plot_style_tables[0].styles.iter().any(|st| st.name == "Hairline"));
    assert!(d.layers.iter().all(|l| l.plot_style == "Hairline"));
    assert_eq!(d.entity(Handle::parse_hex(&h).unwrap()).unwrap().common.plot_style, "Hairline");
    assert_eq!(d.header.str("CPLOTSTYLE", ""), "Hairline");
    for p in [
        json!({ "table": "plotstyle", "from": "Normal", "to": "X" }),
        json!({ "table": "plotstyle", "from": "Missing", "to": "X" }),
        json!({ "table": "plotstyle", "from": "Hairline", "to": "Normal" }),
        json!({ "table": "plotstyle", "from": "Hairline", "to": "bad|name" }),
    ] {
        assert!(s.execute("rename", &p).is_err(), "{p}");
    }
    // Viewport configurations: typed, as -RENAME asks.
    s.script("-RENAME VP Two Pair\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert!(s.doc().unwrap().vports.iter().any(|v| v.name == "Pair"), "{:?}", s.log);
    assert!(s.execute("rename", &json!({ "table": "vport", "from": "*Active", "to": "X" })).is_err());
    assert!(s.execute("rename", &json!({ "table": "vport", "from": "Nope", "to": "X" })).is_err());
    // Materials: the reason, not a stub.
    let e = s.execute("rename", &json!({ "table": "material", "from": "a", "to": "b" })).unwrap_err();
    assert!(e.to_string().contains("material definitions"), "{e}");
}

#[test]
fn colour_books_list_find_and_load() {
    let mut s = Session::new();
    let r = s.execute("colorbook", &json!({})).unwrap();
    assert!(r["books"].as_array().unwrap().iter().any(|b| b["name"] == "CADaddy Basics"));
    let r = s.execute("colorbook", &json!({ "action": "find", "book": "cadaddy basics", "color": "GRAPHITE" })).unwrap();
    assert_eq!(r["rgb"], "#34393e");
    // A TSV book and a JSON one.
    let tsv = "# test book\nbook\tStudio\nWarm\tAmber\t#ffbf00\nWarm\tRust\t#b7410e\nCool\tTeal\t#008080\n";
    let r = s.execute("colorbook", &json!({ "action": "load", "text": tsv })).unwrap();
    assert_eq!((r["book"].clone(), r["colors"].clone()), (json!("Studio"), json!(3)));
    let js = r##"{"name": "Plan", "pages": [{"name": "A", "colors": [{"name": "Wall", "rgb": [10, 20, 30]}]}]}"##;
    s.execute("colorbook", &json!({ "action": "load", "text": js })).unwrap();
    let r = s.execute("colorbook", &json!({ "action": "show", "book": "studio" })).unwrap();
    assert_eq!(r["pages"].as_array().unwrap().len(), 2);
    for p in [
        json!({ "action": "load", "text": "book\tCADaddy Basics\nA\tB\t#000000\n" }),
        json!({ "action": "load", "text": "book\tEmpty\n" }),
        json!({ "action": "load", "text": "nonsense" }),
        json!({ "action": "load", "text": "book\tX\nA\tB\tred\n" }),
        json!({ "action": "load", "text": "{\"name\": \"\", \"pages\": []}" }),
        json!({ "action": "find", "book": "Studio", "color": "Nope" }),
        json!({ "action": "find", "book": "Nope", "color": "Amber" }),
        json!({ "action": "wobble" }),
    ] {
        assert!(s.execute("colorbook", &p).is_err(), "{p}");
    }

    // -LAYER Color COlorbook: book name (with its space), then colour name.
    s.execute("layer.new", &json!({ "name": "L1" })).unwrap();
    for l in ["-LAYER", "C", "CO", "Studio", "teal", "L1", ""] {
        s.cmdline(l).unwrap();
    }
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!(s.doc().unwrap().layer("L1").unwrap().color, Color::True(Rgb(0, 128, 128)), "{:?}", s.log);
    // A book that isn't there asks again.
    for l in ["-LAYER", "C", "CO", "Nope"] {
        let _ = s.cmdline(l);
    }
    assert!(s.running.is_some() && s.prompt_text().contains("Color Book"), "{}", s.prompt_text());
    s.cancel();
}

#[test]
fn hatchedit_recreates_region_boundaries() {
    let mut s = Session::new();
    s.cmdline("rectang 0,0 10,10").unwrap();
    s.cmdline("rectang 3,3 6,6").unwrap();
    let r = s.execute("hatch", &json!({ "points": [[1, 1]], "pattern": "SOLID" })).unwrap();
    let hatch = r.get("handle").cloned().unwrap_or_else(|| r["handles"][0].clone());
    let r = s.execute("hatchedit", &json!({ "handles": [hatch], "recreate": "region", "associate": true })).unwrap();
    let made = r["handles"].as_array().unwrap().clone();
    assert_eq!(made.len(), 1, "one region for the outer loop and its island: {r}");
    let d = s.doc().unwrap();
    let region = Handle::parse_hex(made[0].as_str().unwrap()).unwrap();
    match &d.entity(region).unwrap().kind {
        EntityKind::Region(g) => {
            let mut outer: Vec<bool> = g.loops.iter().map(|l| l.outer).collect();
            outer.sort();
            assert_eq!(outer, vec![false, true]);
            assert!((g.props().area - 91.0).abs() < 1e-6, "{}", g.props().area);
        }
        k => panic!("{k:?}"),
    }
    match &d.entity(Handle::parse_hex(hatch.as_str().unwrap()).unwrap()).unwrap().kind {
        EntityKind::Hatch(h) => assert!(h.associative && h.loops.iter().all(|l| l.sources == vec![region])),
        k => panic!("{k:?}"),
    }
    assert!(s.execute("hatchedit", &json!({ "handles": [hatch], "recreate": "spline" })).is_err());
}
