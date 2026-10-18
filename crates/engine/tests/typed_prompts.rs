//! Commands that had only a JSON form prompt when typed (battle test 3): LTSCALE, LIMITS, BASE,
//! COLOR, LWEIGHT, GETVAR, FIND, LAYISO, LAYMCUR, LAYMCH, BREAKATPOINT.

use cadaddy_engine::doc::EntityKind;
use cadaddy_engine::{Input, Session};
use serde_json::json;

fn typed(s: &mut Session, inputs: &[&str]) {
    for i in inputs {
        s.cmdline(i).unwrap_or_else(|e| panic!("{i}: {e}"));
    }
    assert!(s.running.is_none(), "still running after {inputs:?}: {}", s.prompt_text());
}

#[test]
fn bare_commands_prompt_for_what_they_need() {
    let mut s = Session::new();
    typed(&mut s, &["ltscale", "2.5"]);
    assert_eq!(cadaddy_engine::sysvars::get(&s, "LTSCALE"), Some(json!(2.5)));
    typed(&mut s, &["limits", "0,0", "48,36"]);
    typed(&mut s, &["base", "5,5"]);
    typed(&mut s, &["color", "red"]);
    typed(&mut s, &["lweight", "0.5"]);
    typed(&mut s, &["getvar", "ltscale"]);
    assert!(s.log.iter().rev().take(3).any(|l| l.contains("2.5")), "{:?}", s.log.last());
    typed(&mut s, &["find", "nothing-here"]);

    s.execute("layer.new", &json!({"name": "A"})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let l = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Line(_))).unwrap().handle;
    s.cmdline("breakatpoint").unwrap();
    s.input(Input::Pick(vec![l])).unwrap();
    typed(&mut s, &["5,0"]);
    let lines = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_))).count();
    assert_eq!(lines, 2);
}

/// 0.7.5 re-verify (N1-N4): MEASUREGEOM typed, readable defaults, LAYMCH by object or existing
/// name only.
#[test]
fn measuregeom_defaults_and_laymch() {
    let mut s = Session::new();
    s.cmdline("measuregeom").unwrap();
    for i in ["", "0,0", "3,4"] {
        s.cmdline(i).unwrap();
    }
    assert!(s.log.iter().any(|l| l.contains("Distance = 5")), "{:?}", s.log.iter().rev().take(3).collect::<Vec<_>>());
    s.cmdline("x").unwrap();
    assert!(s.running.is_none());

    s.cmdline("limits").unwrap();
    assert!(s.prompt_text().contains("<0.0000,0.0000>"), "{}", s.prompt_text());
    s.cancel();
    s.cmdline("color").unwrap();
    assert!(s.prompt_text().contains("<BYLAYER>"), "{}", s.prompt_text());
    s.cancel();

    s.execute("layer.new", &json!({"name": "E-LITE"})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let before = s.doc().unwrap().layers.len();
    typed_until(&mut s, &["laymch", "all", "", "0,5"]);
    assert_eq!(s.doc().unwrap().layers.len(), before, "a typed point must not create a layer");
    s.cmdline("E-LITE").unwrap();
    assert!(s.running.is_none());
    assert!(s.doc().unwrap().model.iter().all(|e| e.common.layer == "E-LITE"));
}

fn typed_until(s: &mut Session, inputs: &[&str]) {
    for i in inputs {
        s.cmdline(i).unwrap_or_else(|e| panic!("{i}: {e}"));
    }
}

/// A text style with a fixed height skips TEXT's height prompt, so AutoCAD scripts
/// (`text 0,0 0 FIXED`) work (battle test R2).
#[test]
fn fixed_height_style_skips_the_height_prompt() {
    let mut s = Session::new();
    s.execute("style", &json!({"name": "FIX", "height": 3, "current": true})).unwrap();
    s.script("text 0,0 0 FIXED\n\n").unwrap();
    let t = s.doc().unwrap().model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).expect("text made");
    assert_eq!((t.value.as_str(), t.height), ("FIXED", 3.0));
}

/// A typed snap override finds the object at the point and snaps to it, however far the snap
/// point is: `cen` on a circle's edge gives its centre, `mid` anywhere on a line its midpoint
/// (battle tests D2/R4).
#[test]
fn typed_snap_overrides_pick_the_object() {
    let mut s = Session::new();
    s.execute("circle", &json!({"center": [20, 0], "radius": 3})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    s.script("line cen 20,3 mid 9,0 \n").unwrap();
    let d = s.doc().unwrap();
    let l = d.model.iter().filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some(l.clone()) } else { None }).last().unwrap();
    assert_eq!((l.a.x, l.a.y, l.b.x, l.b.y), (20.0, 0.0, 5.0, 0.0), "{:?}", s.log.iter().rev().take(4).collect::<Vec<_>>());
}

/// Running CEN: hovering a circle's edge snaps to its centre (battle test R4, real mouse).
#[test]
fn running_center_snap_from_the_edge() {
    let mut s = Session::new();
    s.execute("circle", &json!({"center": [0, 0], "radius": 20})).unwrap();
    let d = s.doc().unwrap();
    let hit = cadaddy_engine::snap::osnap(d, &cadaddy_engine::doc::Space::Model, cadaddy_engine::geom::Vec2::new(20.2, 0.3), 1.0, 4133, None, false);
    assert_eq!(hit.map(|h| (h.point.x, h.point.y, h.mode)), Some((0.0, 0.0, 4)));
    // A nearby endpoint still wins over a far centre.
    s.execute("line", &json!({"points": [[20.5, 0.5], [40, 0]]})).unwrap();
    let d = s.doc().unwrap();
    let hit = cadaddy_engine::snap::osnap(d, &cadaddy_engine::doc::Space::Model, cadaddy_engine::geom::Vec2::new(20.2, 0.3), 1.0, 4133, None, false);
    assert_eq!(hit.map(|h| h.mode), Some(1));
}

/// An annotative fixed-height style: TEXT takes its paper height and doesn't ask (batch 13).
#[test]
fn annotative_fixed_height_style_skips_the_height_prompt() {
    let mut s = Session::new();
    s.execute("style", &json!({"name": "ANNO", "height": 0.125, "annotative": true, "current": true})).unwrap();
    s.script("text 0,0 0 NOTE\n\n").unwrap();
    assert!(s.log.iter().any(|l| l.contains("Text height:  0.1250")), "{:?}", s.log);
    assert!(s.doc().unwrap().model.iter().any(|e| matches!(&e.kind, EntityKind::Text(t) if t.value == "NOTE")));
}

/// HPGAPTOL typed by name (battle test H-GAP): hatching a room whose walls don't meet works once
/// the gap tolerance covers the gaps.
#[test]
fn hpgaptol_typed_closes_gaps_for_hatch() {
    let mut s = Session::new();
    for (a, b) in [([0.0, 0.0], [9.5, 0.0]), ([10.0, 0.0], [10.0, 10.0]), ([10.0, 10.0], [0.0, 10.0]), ([0.0, 10.0], [0.0, 0.5])] {
        s.execute("line", &json!({"points": [a, b]})).unwrap();
    }
    assert!(s.execute("hatch", &json!({"points": [[5, 5]], "pattern": "SOLID"})).is_err());
    s.script("hpgaptol 1\n").unwrap();
    assert_eq!(cadaddy_engine::sysvars::get(&s, "HPGAPTOL"), Some(json!(1.0)));
    s.execute("hatch", &json!({"points": [[5, 5]], "pattern": "SOLID"})).unwrap();
    assert!(s.execute("setvar", &json!({"name": "HPGAPTOL", "value": 6000})).is_err());
}

/// SELECT {all: true} selects everything (battle test 30), and a typed ALL on a large drawing
/// stays fast (the selection set no longer grows quadratically).
#[test]
fn select_all_json_and_large_typed_all() {
    let mut s = Session::new();
    s.execute("setvar", &json!({"name": "MAXARRAY", "value": 1000000})).unwrap();
    s.script("line 0,0 0.5,0\n\n-array l  r 200 250 1 1\n").unwrap();
    assert_eq!(s.execute("select", &json!({"all": true})).unwrap()["selected"], 50_000);
    s.execute("select", &json!({"clear": true})).unwrap();
    let t = std::time::Instant::now();
    s.script("move all  0,0 10,0\n").unwrap();
    assert!(t.elapsed().as_secs_f64() < 10.0, "{:?}", t.elapsed());
    assert!(s.log.iter().any(|l| l == "50000 found, 50000 total"));
}

/// -HATCH: an unknown pattern is asked for again (the script's next word doesn't become the
/// scale), and a pattern a hatch in the drawing already defines can be used for new hatches.
#[test]
fn hatch_pattern_unknown_reasks_and_drawing_patterns_are_usable() {
    let mut s = Session::new();
    s.execute("rectang", &json!({"p1": [0, 0], "p2": [10, 10]})).unwrap();
    s.cmdline("-hatch").unwrap();
    s.cmdline("p").unwrap();
    let _ = s.cmdline("NOSUCH");
    assert!(s.prompt_text().to_ascii_lowercase().contains("pattern"), "{}", s.prompt_text());
    s.cancel();
    // A hatch that came from another program's file carries its own definition.
    let h = s.execute("hatch", &json!({"points": [[5, 5]], "pattern": "ANSI31"})).unwrap()["handle"].as_str().unwrap().to_string();
    let hh = cadaddy_engine::doc::Handle::parse_hex(&h).unwrap();
    let lines = cadaddy_engine::doc::library::pattern("ANSI31").unwrap().lines;
    s.doc_mut()
        .unwrap()
        .modify_entity(hh, |e| {
            if let EntityKind::Hatch(x) = &mut e.kind {
                x.pattern = "STEELX".into();
                x.pattern_lines = lines.clone();
            }
        })
        .unwrap();
    s.execute("rectang", &json!({"p1": [20, 0], "p2": [30, 10]})).unwrap();
    s.execute("hatch", &json!({"points": [[25, 5]], "pattern": "STEELX"})).unwrap();
}

/// -DIMSTYLE and -STYLE as AutoCAD scripts write them (battle test 19).
#[test]
fn dash_dimstyle_and_style_scripts() {
    let mut s = Session::new();
    s.execute("setvar", &json!({"name": "DIMSCALE", "value": 48})).unwrap();
    s.script("-dimstyle s ARCH48\n").unwrap();
    assert!(s.running.is_none(), "{} {:?}", s.prompt_text(), s.log.iter().rev().take(4).collect::<Vec<_>>());
    let d = s.doc().unwrap();
    assert_eq!(d.header.str("DIMSTYLE", ""), "ARCH48");
    assert!(d.dim_style("ARCH48").is_some_and(|st| (st.scale - 48.0).abs() < 1e-9));
    s.script("-dimstyle r Standard\n").unwrap();
    assert_eq!(s.doc().unwrap().header.str("DIMSTYLE", ""), "Standard");
    s.script("-style NOTES arial.ttf 0.125 1 0 n n\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let d = s.doc().unwrap();
    assert_eq!(d.header.str("TEXTSTYLE", ""), "NOTES");
    assert!(d.text_style("NOTES").is_some_and(|t| t.font == "arial.ttf" && (t.height - 0.125).abs() < 1e-9));
}
