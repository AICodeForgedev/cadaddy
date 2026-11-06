//! Values commands used to map silently: lineweights outside AutoCAD's list (nan, -5, inf, 999,
//! 1e300), DIMADEC -1 stored as 0, and LINETYPE reading linetypes out of any text file.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Dimension, EntityKind, Handle, Lineweight};
use serde_json::{Value, json};

fn circle(s: &mut Session) -> Handle {
    let r = s.execute("circle", &json!({"center": [0, 0], "radius": 5})).unwrap();
    Handle::parse_hex(r["handle"].as_str().unwrap()).unwrap()
}

fn lineweight(s: &Session, h: Handle) -> Lineweight {
    s.doc().unwrap().entity(h).unwrap().common.lineweight
}

/// Values no command may take as a lineweight.
fn refused() -> Vec<Value> {
    vec![
        json!("nan"),
        json!("NaN"),
        json!(-5),
        json!("-5"),
        json!("inf"),
        json!("-inf"),
        json!(999),
        json!(1e300),
        json!("1e300"),
        json!(212),
        json!(0.33),
        json!(2.12),
        json!(-0.25),
        json!(26),
        json!("25 mm"),
        json!("abc"),
        json!(""),
        json!(true),
        json!({"mm": 0.25}),
        json!([0.25]),
    ]
}

/// Values every command takes, and what they mean.
fn accepted() -> Vec<(Value, Lineweight)> {
    vec![
        (json!(0.25), Lineweight::Mm100(25)),
        (json!(25), Lineweight::Mm100(25)),
        (json!("0.25"), Lineweight::Mm100(25)),
        (json!("0.25 mm"), Lineweight::Mm100(25)),
        (json!("0.25mm"), Lineweight::Mm100(25)),
        (json!("25"), Lineweight::Mm100(25)),
        (json!(0), Lineweight::Mm100(0)),
        (json!(0.05), Lineweight::Mm100(5)),
        (json!(5), Lineweight::Mm100(5)),
        (json!(0.7), Lineweight::Mm100(70)),
        (json!(1), Lineweight::Mm100(100)),
        (json!(1.06), Lineweight::Mm100(106)),
        (json!(2.11), Lineweight::Mm100(211)),
        (json!(211), Lineweight::Mm100(211)),
        (json!("ByLayer"), Lineweight::ByLayer),
        (json!("byblock"), Lineweight::ByBlock),
        (json!("Default"), Lineweight::Default),
        (json!(-1), Lineweight::ByLayer),
        (json!(-2), Lineweight::ByBlock),
        (json!(-3), Lineweight::Default),
    ]
}

fn err_text(r: cadaddy_engine::Result<Value>) -> String {
    match r {
        Ok(v) => panic!("accepted: {v}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn properties_set_takes_only_autocad_lineweights() {
    let mut s = Session::new();
    let h = circle(&mut s);
    s.execute("properties.set", &json!({"handles": [h.hex()], "lineweight": 0.5})).unwrap();
    for v in refused() {
        let m = err_text(s.execute("properties.set", &json!({"handles": [h.hex()], "lineweight": v.clone()})));
        assert!(m.contains("2.11") && m.contains("ByLayer"), "{v}: the message names the allowed list: {m}");
        assert_eq!(lineweight(&s, h), Lineweight::Mm100(50), "{v} left the object alone");
    }
    for (v, want) in accepted() {
        s.execute("properties.set", &json!({"handles": [h.hex()], "lineweight": v.clone()})).unwrap();
        assert_eq!(lineweight(&s, h), want, "{v}");
    }
}

#[test]
fn layer_new_and_layer_take_only_autocad_lineweights() {
    let mut s = Session::new();
    for (i, v) in refused().into_iter().enumerate() {
        let name = format!("LW{i}");
        let m = err_text(s.execute("layer.new", &json!({"name": name, "lineweight": v.clone()})));
        assert!(m.contains("2.11"), "{v}: {m}");
        assert!(s.doc().unwrap().layer(&name).is_none(), "{v}: no layer made");
    }
    s.execute("layer.new", &json!({"name": "Walls", "lineweight": 0.35})).unwrap();
    for v in refused() {
        assert!(s.execute("layer.set", &json!({"name": "Walls", "lineweight": v.clone()})).is_err(), "{v}");
        assert_eq!(s.doc().unwrap().layer("Walls").unwrap().lineweight, Lineweight::Mm100(35), "{v}");
    }
    for (v, want) in accepted().into_iter().filter(|(_, w)| matches!(w, Lineweight::Mm100(_) | Lineweight::Default)) {
        s.execute("layer.set", &json!({"name": "Walls", "lineweight": v.clone()})).unwrap();
        assert_eq!(s.doc().unwrap().layer("Walls").unwrap().lineweight, want, "{v}");
    }
}

#[test]
fn lweight_sets_celweight_only_to_a_standard_weight() {
    let mut s = Session::new();
    s.execute("lweight", &json!({"lineweight": 0.3})).unwrap();
    assert_eq!(s.doc().unwrap().header.i64("CELWEIGHT", -9), 30);
    for v in refused() {
        assert!(s.execute("lweight", &json!({"lineweight": v.clone()})).is_err(), "{v}");
        assert_eq!(s.doc().unwrap().header.i64("CELWEIGHT", -9), 30, "{v}");
    }
    for (v, want) in accepted() {
        s.execute("lweight", &json!({"lineweight": v.clone()})).unwrap();
        assert_eq!(s.doc().unwrap().header.i64("CELWEIGHT", -9), i64::from(want.to_dxf()), "{v}");
    }
}

#[test]
fn typed_chprop_takes_hundredths_and_refuses_the_rest() {
    let mut s = Session::new();
    s.cmdline("LINE 0,0 10,0").unwrap();
    s.cmdline("").unwrap();
    let h = s.doc().unwrap().model.iter().next().unwrap().handle;
    // A whole number is in 1/100 mm, as AutoCAD's CHPROP takes it: 25 is 0.25 mm, not 2.11 mm.
    s.script("CHPROP 5,0\n\nLW 25\n\n").unwrap();
    assert_eq!(lineweight(&s, h), Lineweight::Mm100(25));
    s.cancel();
    s.set_selection(Vec::new());
    for bad in ["999", "nan", "-5", "1e300"] {
        let _ = s.script(&format!("CHPROP 5,0\n\nLW {bad}\n\n"));
        s.cancel();
        s.set_selection(Vec::new());
        assert_eq!(lineweight(&s, h), Lineweight::Mm100(25), "CHPROP LW {bad}");
    }
}

#[test]
fn typed_layer_lweight_refuses_instead_of_snapping() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "Walls"})).unwrap();
    let lw = |s: &Session| s.doc().unwrap().layer("Walls").unwrap().lineweight;
    s.cmdline("-LAYER").unwrap();
    s.script("LW 0.35mm Walls\n\n").unwrap();
    assert_eq!(lw(&s), Lineweight::Mm100(35));
    s.cancel();
    // 0.31 used to snap to 0.30; it is not one of AutoCAD's weights.
    for bad in ["0.31", "nan", "-5", "999"] {
        s.cmdline("-LAYER").unwrap();
        let _ = s.script(&format!("LW {bad}\n"));
        let said = format!("{bad} is not a lineweight; enter a standard lineweight in mm (0.00, 0.05");
        assert!(s.log.iter().any(|l| l.to_ascii_lowercase().starts_with(&said)), "LW {bad}: {:?}", s.log);
        s.log.clear();
        s.cancel();
        assert_eq!(lw(&s), Lineweight::Mm100(35), "-LAYER LW {bad}");
    }
}

#[test]
fn xplode_table_borders_and_vplayer_refuse_odd_lineweights() {
    let mut s = Session::new();
    s.execute("rectang", &json!({"p1": [0, 0], "p2": [10, 10]})).unwrap();
    let pl = s.doc().unwrap().model.last().unwrap().handle.hex();
    for v in [json!("nan"), json!(999), json!(-5)] {
        assert!(s.execute("xplode", &json!({"handles": [pl], "lineweight": v.clone()})).is_err(), "xplode {v}");
    }
    assert!(matches!(s.doc().unwrap().model.last().unwrap().kind, EntityKind::LwPolyline(_)), "still one polyline");

    s.execute("tablestyle", &json!({"name": "Grid"})).unwrap();
    let border = |w: Value| json!({"style": "Grid", "name": "Totals", "borders": {"all": {"lineweight": w}}});
    s.execute("tablestyle.cellstyle", &border(json!(0.5))).unwrap();
    for v in [json!(0.33), json!("inf"), json!(999), json!(-5)] {
        let r = s.execute("tablestyle.cellstyle", &border(v.clone()));
        assert!(r.is_err(), "table border {v}: {r:?}");
    }
}

fn dims(s: &Session) -> Vec<Dimension> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Dimension(dm) => Some((**dm).clone()),
            _ => None,
        })
        .collect()
}

fn angular_22_25(s: &mut Session) -> String {
    let a = 22.25f64.to_radians();
    s.execute("dimangular", &json!({"vertex": [0, 0], "p1": [10, 0], "p2": [10.0 * a.cos(), 10.0 * a.sin()], "at": [6, 1]})).unwrap();
    let d = s.doc().unwrap();
    let dm = dims(s).pop().unwrap();
    cadaddy_engine::render::dimension_in(d, &dm).value
}

#[test]
fn dimadec_minus_one_uses_dimdec_and_round_trips() {
    let mut s = Session::new();
    s.execute("setvar", &json!({"name": "DIMDEC", "value": 3})).unwrap();
    s.execute("setvar", &json!({"name": "DIMADEC", "value": -1})).unwrap();
    assert_eq!(s.execute("getvar", &json!({"name": "DIMADEC"})).unwrap().get("value"), Some(&json!(-1)));
    assert_eq!(angular_22_25(&mut s), "22.250°", "DIMADEC -1 takes DIMDEC's 3 places");
    // A set DIMADEC still wins over DIMDEC.
    s.execute("setvar", &json!({"name": "DIMADEC", "value": 1})).unwrap();
    assert_eq!(angular_22_25(&mut s), "22.3°");
    // The dimstyle command keeps -1 too, and still refuses values outside -1..8.
    let r = s.execute("dimstyle", &json!({"name": "Standard", "DIMADEC": -1})).unwrap();
    assert!(r.get("ignored").is_none(), "{r}");
    assert_eq!(s.doc().unwrap().dim_style("Standard").unwrap().angular_decimals, -1);
    for v in [json!(-2), json!(9), json!("nan"), json!(1e300)] {
        let r = s.execute("dimstyle", &json!({"name": "Standard", "DIMADEC": v.clone()})).unwrap();
        assert!(r.get("ignored").is_some(), "DIMADEC {v}: {r}");
        assert_eq!(s.doc().unwrap().dim_style("Standard").unwrap().angular_decimals, -1, "{v}");
    }

    // -1 is written back as -1 in .cdy, DXF and DWG (the style and the drawing's variable).
    s.execute("setvar", &json!({"name": "DIMADEC", "value": -1})).unwrap();
    let d = s.doc().unwrap();
    let cdy = cadaddy_io::cdy::read_cdy(&cadaddy_io::cdy::write_cdy(d).unwrap()).unwrap();
    let dxf_text = cadaddy_io::write_dxf(d);
    let dxf = cadaddy_io::read_dxf(dxf_text.as_bytes()).unwrap();
    let mut back = vec![("cdy", cdy), ("dxf", dxf)];
    for engine in [cadaddy_io::DwgEngine::Native, cadaddy_io::DwgEngine::Bridge] {
        let bytes = cadaddy_io::write_dwg(d, cadaddy_io::Version::R2018, engine).unwrap();
        back.push(("dwg", cadaddy_io::read_dwg(&bytes, engine).unwrap()));
    }
    for (what, d2) in back {
        let st = d2.dim_style("Standard").unwrap_or_else(|| panic!("{what}: Standard"));
        assert_eq!(st.angular_decimals, -1, "{what}");
        assert_eq!(st.angular_places(), st.decimals, "{what}: -1 reads as the style's DIMDEC");
        assert_eq!(d2.header.i64("DIMADEC", 9), -1, "{what}: the DIMADEC variable");
    }
}

/// Enough of a DXF that its `*` lines (VPORT, block records, a dimension block) look like
/// linetype names to a reader that doesn't check the pattern line.
const DXF_LIKE: &str = "  0\nSECTION\n  2\nTABLES\n  0\nTABLE\n  2\nVPORT\n  0\nVPORT\n  2\n*Active\n 70\n     0\n  0\nENDTAB\n  0\nTABLE\n  2\nBLOCK_RECORD\n  0\nBLOCK_RECORD\n  2\n*Model_Space\n  0\nBLOCK_RECORD\n  2\n*Paper_Space\n  0\nBLOCK_RECORD\n  2\n*D1\n  0\nENDTAB\n  0\nENDSEC\n  0\nEOF\n";

#[test]
fn linetype_refuses_a_file_that_is_not_a_linetype_file() {
    let mut s = Session::new();
    let before: Vec<String> = s.doc().unwrap().linetypes.iter().map(|l| l.name.clone()).collect();
    let not_lin = [
        DXF_LIKE.to_string(),
        // A `*NAME` followed by a line of plain numbers (a .pat definition, a DXF group value).
        "*Active\n70\n*D1\n0\n".to_string(),
        "*HATCHY,A pattern\n0, 0,0, 0,.125\n".to_string(),
        // A good definition next to a name with no pattern line at all.
        "*GOOD,Good\nA,.5,-.25\n*ORPHAN,No pattern\n".to_string(),
        // Text before the first definition that is not a comment.
        "hello\n*GOOD,Good\nA,.5,-.25\n".to_string(),
        "; only a comment\n".to_string(),
    ];
    for text in &not_lin {
        let m = err_text(s.execute("linetype", &json!({"text": text})));
        assert!(m.contains("not a linetype file"), "{text:?}: {m}");
    }
    let after: Vec<String> = s.doc().unwrap().linetypes.iter().map(|l| l.name.clone()).collect();
    assert_eq!(after, before, "nothing was added");

    // From a file on disk too.
    let dir = std::env::temp_dir().join(format!("cadaddy-cmdvalues-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Sample Plan.dxf");
    std::fs::write(&path, DXF_LIKE).unwrap();
    let m = err_text(s.execute("linetype", &json!({"file": path.display().to_string()})));
    assert!(m.contains("not a linetype file"), "{m}");
    // -LINETYPE Load from that file says so and loads nothing.
    let _ = s.script(&format!("-LINETYPE\nL\n*\n{}\n\n", path.display()));
    s.cancel();
    let after: Vec<String> = s.doc().unwrap().linetypes.iter().map(|l| l.name.clone()).collect();
    assert_eq!(after, before, "-LINETYPE added nothing");
    let _ = std::fs::remove_dir_all(&dir);

    // A real .lin (comments, blank lines, a definition with a bad element) still loads.
    let lin = ";; Office linetypes\n\n*DASHY,Dashed __ __\nA,.5,-.25\n*BROKEN,Bad element\nA,.5,[BAD\n\n*DOTTY,Dotted\na,0,-.25\n";
    let r = s.execute("linetype", &json!({"text": lin})).unwrap();
    assert_eq!(r["added"], json!(["DASHY", "DOTTY"]), "{r}");
    assert!(r["warnings"].to_string().contains("BROKEN"), "{r}");
}
