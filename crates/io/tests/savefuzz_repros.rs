//! Minimal repros of what the save/reopen differential fuzzing (Q239, `engine/tests/savefuzz.rs`)
//! found in the readers and writers.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use cadaddy_color::{Color, Rgb};
use cadaddy_doc::*;
use cadaddy_io::{DwgEngine, Version};

const DWG: [Version; 6] = [Version::R2000, Version::R2004, Version::R2007, Version::R2010, Version::R2013, Version::R2018];

fn native(d: &Drawing, v: Version) -> Drawing {
    let bytes = cadaddy_io::write_dwg(d, v, DwgEngine::Native).unwrap();
    cadaddy_io::read_dwg(&bytes, DwgEngine::Native).unwrap()
}

/// Names and text outside Latin-1 in a DWG older than 2007 (code-page strings): written as
/// `\U+XXXX` escapes and read back, not as `?` ("Δ fläche" came back "? fläche").
#[test]
fn native_dwg_keeps_characters_outside_the_code_page() {
    let mut d = Drawing::new_metric();
    for name in ["Δ fläche", "層-1", "Слой 2", "€ 5", "emoji 🙂"] {
        d.layers.push(Layer { name: name.into(), ..Layer::default() });
    }
    d.text_styles.push(TextStyle { name: "日本語 style".into(), ..TextStyle::default() });
    d.blocks.insert("Ébauche ط".into(), Arc::new(Block::new("Ébauche ط")));
    for v in DWG {
        let bytes = cadaddy_io::write_dwg(&d, v, DwgEngine::Native).unwrap();
        let back = cadaddy_io::read_dwg(&bytes, DwgEngine::Native).unwrap();
        for name in ["Δ fläche", "層-1", "Слой 2", "€ 5", "emoji 🙂"] {
            assert!(back.layers.iter().any(|l| l.name == name), "{v:?}: layer {name}: {:?}", back.layers.iter().map(|l| &l.name).collect::<Vec<_>>());
        }
        assert!(back.text_styles.iter().any(|t| t.name == "日本語 style"), "{v:?}");
        assert!(back.blocks.contains_key("Ébauche ط"), "{v:?}");
        // Through the bridge's reader too (another reading of the same bytes).
        let bridge = cadaddy_io::read_dwg(&bytes, DwgEngine::Bridge).unwrap();
        assert!(bridge.layers.iter().any(|l| l.name == "Δ fläche"), "{v:?} bridge: {:?}", bridge.layers.iter().map(|l| &l.name).collect::<Vec<_>>());
    }
}

/// A block definition's description in CADaddy's DWG writer (it wrote an empty one).
#[test]
fn native_dwg_keeps_block_descriptions() {
    let mut d = Drawing::new_metric();
    let mut b = Block::new("B1");
    b.description = "emoji 🙂 and Ébauche".into();
    d.blocks.insert("B1".into(), Arc::new(b));
    for v in DWG {
        let back = native(&d, v);
        assert_eq!(back.blocks.get("B1").map(|b| b.description.as_str()), Some("emoji 🙂 and Ébauche"), "{v:?}");
    }
}

/// A layer that is off and has a true colour (DWG 2004+ stores colour index 0 for it, which
/// can't carry the "off" sign of DXF group 62).
#[test]
fn native_dwg_keeps_true_colour_layers_off() {
    let mut d = Drawing::new_metric();
    d.layers.push(Layer { name: "Off".into(), color: Color::True(Rgb(1, 2, 3)), on: false, ..Layer::default() });
    d.layers.push(Layer { name: "On".into(), color: Color::True(Rgb(4, 5, 6)), ..Layer::default() });
    for v in DWG {
        let back = native(&d, v);
        let l = |n: &str| back.layers.iter().find(|l| l.name == n).cloned().unwrap();
        assert!(!l("Off").on, "{v:?}");
        assert!(l("On").on, "{v:?}");
        if v >= Version::R2004 {
            assert_eq!(l("Off").color, Color::True(Rgb(1, 2, 3)), "{v:?}");
        }
    }
}

/// Defpoints never plots: a Release 12 DXF has no plot flag, and reading one gave it the flag.
#[test]
fn defpoints_does_not_plot_after_release_12() {
    let d = Drawing::new_metric();
    let bytes = cadaddy_io::write_with(&d, "x.dxf", &cadaddy_io::SaveOptions { version: Some(Version::R12), binary: false }).unwrap();
    let back = cadaddy_io::read(&bytes, "x.dxf").unwrap();
    let def = back.layers.iter().find(|l| l.name.eq_ignore_ascii_case("Defpoints")).unwrap();
    assert!(!def.plot);
    assert!(back.layers.iter().find(|l| l.name == "0").unwrap().plot);
}

/// A dense hatch's pattern in SVG: its lines as a few paths, not an element per dash (a DOTS
/// hatch was a million `<circle>`s, 92 MB, more elements than an SVG reader takes).
#[test]
fn svg_writes_a_dense_hatch_as_few_paths() {
    use cadaddy_geom::{LoopEdge, Vec2};
    for pattern in ["ANSI31", "DOTS"] {
        let mut d = Drawing::new_metric();
        let (a, b, c, e) = (Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(100.0, 100.0), Vec2::new(0.0, 100.0));
        let edges = vec![LoopEdge::Line { a, b }, LoopEdge::Line { a: b, b: c }, LoopEdge::Line { a: c, b: e }, LoopEdge::Line { a: e, b: a }];
        let hatch = Hatch {
            pattern: pattern.into(),
            solid: false,
            loops: vec![HatchLoop::from_curves(edges, true, Vec::new())],
            scale: if pattern == "DOTS" { 0.5 } else { 0.01 },
            angle: 0.0,
            associative: false,
            style: 0,
            elevation: 0.0,
            gradient: None,
            origin: Vec2::ZERO,
            background: None,
            pattern_lines: Vec::new(),
        };
        d.add(&Space::Model, Common::default(), EntityKind::Hatch(hatch)).unwrap();
        let svg = String::from_utf8(cadaddy_io::write(&d, "x.svg").unwrap()).unwrap();
        let elements = svg.matches("<path").count() + svg.matches("<polyline").count() + svg.matches("<circle").count();
        let segments = svg.matches('M').count();
        assert!(segments > 2_000, "{pattern}: a dense pattern ({segments} subpaths)");
        assert!(elements < 10, "{pattern}: {elements} elements");
        let back = cadaddy_io::read(svg.as_bytes(), "x.svg").unwrap();
        assert!(back.model.len() > 1_000, "{pattern}: read back ({} objects)", back.model.len());
    }
}
