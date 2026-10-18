//! Dimension variables DIMTFILL / DIMTFILLCLR, DIMTXTDIRECTION, DIMUPT, DIMALTTD / DIMALTTZ, and
//! the jog height and break size style settings: set through DIMSTYLE and SETVAR, checked, and
//! drawn.

use cadaddy_doc::{EntityKind, Handle, Space};
use cadaddy_engine::Session;
use cadaddy_engine::doc::color::Color;
use cadaddy_render::{Kind, Options, build};
use serde_json::{Value, json};

fn h(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

fn set(s: &mut Session, name: &str, value: Value) {
    s.execute("setvar", &json!({ "name": name, "value": value })).unwrap();
}

/// The dimension's style with its overrides, and its geometry.
fn geometry(s: &Session, hd: Handle) -> cadaddy_render::DimGeometry {
    let d = s.doc().unwrap();
    let EntityKind::Dimension(dm) = &d.entity(hd).unwrap().kind else { panic!("not a dimension") };
    let st = d.dim_style(&dm.style).cloned().unwrap_or_default().with_overrides(&dm.overrides);
    cadaddy_render::dimension_geometry(dm, &st, 1.0)
}

fn linear(s: &mut Session, at: [f64; 2]) -> Handle {
    h(&s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [10, 0], "at": at })).unwrap())
}

#[test]
fn new_variables_are_style_settings_with_their_ranges() {
    let mut s = Session::new();
    s.execute(
        "dimstyle",
        &json!({ "name": "Fill", "DIMTFILL": 2, "DIMTFILLCLR": "red", "DIMTXTDIRECTION": 1, "DIMUPT": 1, "DIMALTTD": 3, "DIMALTTZ": 8, "jogHeight": 2.0, "breakSize": 0.25 }),
    )
    .unwrap();
    let st = s.doc().unwrap().dim_style("Fill").unwrap().clone();
    assert_eq!((st.text_fill, st.text_fill_color, st.text_right_to_left, st.user_text_places_line), (2, Color::Index(1), true, true));
    assert_eq!((st.alt_tol_decimals, st.alt_tol_zero_suppression, st.jog_height, st.break_size), (3, 8, 2.0, 0.25));
    // Out of range values are refused, the style keeps its value.
    for (var, v) in [("DIMTFILL", json!(3)), ("DIMALTTD", json!(9)), ("jogHeight", json!(0)), ("breakSize", json!(-1))] {
        let mut m = serde_json::Map::new();
        m.insert(var.into(), v);
        let mut st2 = st.clone();
        assert_eq!(st2.apply_fields(&m), vec![var.to_string()], "{var}");
        assert_eq!(st2, st, "{var}");
    }
    // Defaults: no fill, left to right, cursor places the line only, 2 alternate tolerance decimals.
    let std = cadaddy_doc::DimStyle::default();
    assert_eq!((std.text_fill, std.text_right_to_left, std.user_text_places_line, std.alt_tol_decimals, std.jog_height), (0, false, false, 2, 1.5));
    assert_eq!(cadaddy_doc::DimStyle::iso25().break_size, 3.75);
}

#[test]
fn setvar_values_become_overrides_of_new_dimensions() {
    let mut s = Session::new();
    set(&mut s, "DIMTFILL", json!(1));
    set(&mut s, "DIMTXTDIRECTION", json!(1));
    let d = linear(&mut s, [5.0, 3.0]);
    let doc = s.doc().unwrap();
    let EntityKind::Dimension(dm) = &doc.entity(d).unwrap().kind else { panic!() };
    assert_eq!(dm.overrides.get("textFill"), Some(&json!(1)));
    assert_eq!(dm.overrides.get("textRightToLeft"), Some(&json!(true)));
}

#[test]
fn text_fill_draws_a_mask_or_a_colour_box_behind_the_text() {
    let mut s = Session::new();
    let fills = |s: &Session| {
        let l = build(s.doc().unwrap(), &Space::Model, &Options::default());
        (l.prims.iter().filter(|p| p.kind == Kind::Mask).count(), l.prims.iter().filter(|p| p.kind.is_fill()).count())
    };
    linear(&mut s, [5.0, 3.0]);
    let (masks0, fills0) = fills(&s);
    assert_eq!(masks0, 0);
    // Background: a mask (the drawing's background colour).
    s.execute("dimstyle", &json!({ "name": "Standard", "DIMTFILL": 1 })).unwrap();
    let (masks1, _) = fills(&s);
    assert_eq!(masks1, 1);
    // A colour: a filled box that isn't a mask.
    s.execute("dimstyle", &json!({ "name": "Standard", "DIMTFILL": 2, "DIMTFILLCLR": 3 })).unwrap();
    let (masks2, fills2) = fills(&s);
    assert_eq!(masks2, 0);
    assert_eq!(fills2, fills0 + 1);
}

#[test]
fn text_direction_reverses_the_runs_not_the_numbers() {
    assert_eq!(cadaddy_render::right_to_left("12.50 [0.49]"), "[0.49] 12.50");
    // Formatting groups stay whole, spaces inside them included.
    assert_eq!(cadaddy_render::right_to_left("R2.5 {\\H0.7x;\\S+0.1^ -0.1;}"), "{\\H0.7x;\\S+0.1^ -0.1;} R2.5");
    assert_eq!(cadaddy_render::right_to_left("10"), "10");

    let mut s = Session::new();
    s.execute("dimstyle", &json!({ "name": "Standard", "DIMALT": 1, "DIMDEC": 2, "DIMTXTDIRECTION": 1 })).unwrap();
    let d = linear(&mut s, [5.0, 3.0]);
    let g = geometry(&s, d);
    assert!(g.mtext.starts_with('[') && g.mtext.ends_with("10.00"), "{}", g.mtext);
}

#[test]
fn alternate_tolerances_use_their_own_precision_and_zero_suppression() {
    let mut s = Session::new();
    s.execute(
        "dimstyle",
        &json!({ "name": "Standard", "DIMDEC": 2, "DIMTOL": 1, "DIMTP": 0.1, "DIMTM": 0.1, "DIMTDEC": 2, "DIMALT": 1, "DIMALTF": 25.4, "DIMALTD": 1, "DIMALTTD": 3 }),
    )
    .unwrap();
    let d = linear(&mut s, [5.0, 3.0]);
    // 10 ±0.10, alternate 254.0 ±2.540.
    assert_eq!(geometry(&s, d).mtext, "10.00%%p0.10 [254.0%%p2.540]");

    // Unequal deviations stack; DIMALTTZ 8 (trailing zeros) drops the zeros.
    s.execute("dimstyle", &json!({ "name": "Standard", "DIMTP": 0.1, "DIMTM": 0.05, "DIMALTTZ": 8 })).unwrap();
    let d2 = linear(&mut s, [5.0, 6.0]);
    let m = geometry(&s, d2).mtext;
    assert!(m.contains("[254.0{") && m.contains("\\S+2.54^-1.27;}]"), "{m}");
}

#[test]
fn dimupt_places_the_text_with_the_dimension_line() {
    let mut s = Session::new();
    let plain = linear(&mut s, [3.0, 2.0]);
    let g = geometry(&s, plain);
    assert!((g.text_pos.x - 5.0).abs() < 1e-9, "centred without DIMUPT: {:?}", g.text_pos);

    set(&mut s, "DIMUPT", json!(1));
    let placed = linear(&mut s, [3.0, 2.0]);
    let doc = s.doc().unwrap();
    let EntityKind::Dimension(dm) = &doc.entity(placed).unwrap().kind else { panic!() };
    assert!(dm.user_text_pos);
    let g = geometry(&s, placed);
    assert!((g.text_pos.x - 3.0).abs() < 1e-9, "at the pick: {:?}", g.text_pos);

    // Typed: the dimension line pick places the text too.
    s.cmdline("DIMLINEAR 0,10 10,10 7,13").unwrap();
    let last = s.doc().unwrap().model.iter().last().unwrap().handle;
    assert!((geometry(&s, last).text_pos.x - 7.0).abs() < 1e-9);
}

#[test]
fn jog_height_and_break_size_come_from_the_style() {
    let mut s = Session::new();
    let d = linear(&mut s, [5.0, 3.0]);
    let jog_height = |s: &Session| {
        let g = geometry(s, d);
        // The jog is the dim line piece with four points; its height across the line.
        let z = g.lines.iter().find(|l| l.len() == 4).expect("a jog");
        let ys: Vec<f64> = z.iter().map(|p| p.y).collect();
        ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min)
    };
    s.execute("dimjogline", &json!({ "handle": d.hex(), "at": [5.0, 3.0] })).unwrap();
    let th = cadaddy_doc::DimStyle::default().text_height;
    assert!((jog_height(&s) - 1.5 * th).abs() < 1e-9);
    s.execute("dimstyle", &json!({ "name": "Standard", "jogHeight": 3.0 })).unwrap();
    assert!((jog_height(&s) - 3.0 * th).abs() < 1e-9);

    // DIMBREAK's automatic size is the style's break size.
    s.execute("dimstyle", &json!({ "name": "Standard", "breakSize": 0.5 })).unwrap();
    s.execute("line", &json!({ "points": [[2, -5], [2, 5]] })).unwrap();
    s.execute("dimbreak", &json!({ "handles": [d.hex()] })).unwrap();
    let doc = s.doc().unwrap();
    let EntityKind::Dimension(dm) = &doc.entity(d).unwrap().kind else { panic!() };
    assert!(!dm.marks.breaks.is_empty() && dm.marks.breaks.iter().all(|b| (b.size - 0.5).abs() < 1e-9), "{:?}", dm.marks.breaks);
}
