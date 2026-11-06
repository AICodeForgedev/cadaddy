//! Three DWG/DXF save losses (soak B5-001..003).
//!
//! An associative HATCH without source boundary objects (flag 71 = 1, no 330 handles, as imported
//! from a DXF) keeps `associative` through a DWG save and read, in every engine and version.

use cadaddy_doc::*;
use cadaddy_geom::{PolyVertex, Vec2};
use cadaddy_io::{DwgEngine, Version};

fn drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    let vs = [(0.0, 0.0), (4.0, 0.0), (4.0, 3.0), (0.0, 3.0)].into_iter().map(|(x, y)| PolyVertex::new(Vec2::new(x, y))).collect();
    let l = HatchLoop { curves: Vec::new(), vertices: vs, outer: true, sources: Vec::new() };
    let h = Hatch {
        pattern: "ANSI31".into(),
        solid: false,
        loops: vec![l],
        scale: 1.0,
        angle: 0.0,
        associative: true,
        style: 0,
        elevation: 0.0,
        gradient: None,
        origin: Vec2::ZERO,
        background: None,
        pattern_lines: Vec::new(),
    };
    d.add(&Space::Model, Common::default(), EntityKind::Hatch(Box::new(h))).expect("add");
    d
}

fn associative(d: &Drawing) -> Vec<bool> {
    d.model.iter().filter_map(|e| if let EntityKind::Hatch(h) = &e.kind { Some(h.associative) } else { None }).collect()
}

#[test]
fn sourceless_associative_hatch_survives_dwg() {
    for engine in [DwgEngine::Native, DwgEngine::Bridge] {
        for v in [Version::R2004, Version::R2018] {
            let bytes = cadaddy_io::write_dwg(&drawing(), v, engine).unwrap();
            let back = cadaddy_io::read_dwg(&bytes, engine).unwrap();
            assert_eq!(associative(&back), vec![true], "{engine:?} {v:?}");
        }
    }
}

/// A style left at the default fixed extension length (1.0) came back 0 from a pre-2007 DWG
/// through the bridge, which has no such field there.
#[test]
fn default_fixed_extension_length_survives_dwg() {
    let mut d = Drawing::new_imperial();
    let mut s = d.dim_styles[0].clone();
    s.fixed_ext = true;
    s.fixed_ext_length = 1.0;
    d.dim_styles[0] = s;
    for engine in [DwgEngine::Native, DwgEngine::Bridge] {
        for v in [Version::R2000, Version::R2004, Version::R2007, Version::R2018] {
            let bytes = cadaddy_io::write_dwg(&d, v, engine).unwrap();
            for read in [DwgEngine::Native, DwgEngine::Bridge].into_iter().filter(|r| engine == DwgEngine::Bridge || *r == engine) {
                let s = &cadaddy_io::read_dwg(&bytes, read).unwrap().dim_styles[0];
                assert!(
                    s.fixed_ext && (s.fixed_ext_length - 1.0).abs() < 1e-9,
                    "{engine:?} {v:?} read {read:?}: {} {}",
                    s.fixed_ext,
                    s.fixed_ext_length
                );
            }
        }
    }
}

/// The Defpoints layer set to plot keeps that through a DXF save and read.
#[test]
fn plotting_defpoints_layer_survives_dxf() {
    let mut d = Drawing::new_imperial();
    match d.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case("Defpoints")) {
        Some(l) => l.plot = true,
        None => {
            let mut l = d.layers[0].clone();
            l.name = "Defpoints".into();
            l.plot = true;
            d.layers.push(l);
        }
    }
    let back = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&d).as_bytes()).unwrap();
    assert!(back.layers.iter().find(|l| l.name.eq_ignore_ascii_case("Defpoints")).is_some_and(|l| l.plot));
}
