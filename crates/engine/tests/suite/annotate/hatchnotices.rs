//! Saving a hatch must not report a loss that is not one: a polyline closed by a repeated first
//! point (its loop is read back without the repeat) and float residue in re-saved pattern lines.
//! A loop that really loses a vertex and a pattern line that really changes are still reported.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::kept::losses_between;
use cadaddy_engine::doc::{Drawing, EntityKind};
use cadaddy_io::{DwgEngine, SaveOptions, Version};
use serde_json::json;

type Trip = Box<dyn Fn(&Drawing) -> Drawing>;

fn trips() -> Vec<(&'static str, Trip)> {
    let dxf = |d: &Drawing| {
        let b = cadaddy_io::write_with(d, "x.dxf", &SaveOptions { version: Some(Version::R2018), binary: false }).expect("dxf");
        cadaddy_io::read(&b, "x.dxf").expect("read dxf")
    };
    let dwg = |d: &Drawing| {
        let b = cadaddy_io::write_dwg(d, Version::R2018, DwgEngine::Native).expect("dwg");
        cadaddy_io::read(&b, "x.dwg").expect("read dwg")
    };
    vec![("dxf", Box::new(dxf)), ("dwg", Box::new(dwg))]
}

fn hatched(vertices: serde_json::Value, pattern: &str, scale: f64, angle: f64) -> Drawing {
    let mut s = Session::new();
    let closed = vertices.as_array().is_some_and(|v| v.first() != v.last());
    s.execute("pline", &json!({ "vertices": vertices, "closed": closed })).expect("pline");
    let h = s.doc().expect("doc").model.iter().next().expect("pline").handle.hex();
    s.execute("hatch", &json!({ "handles": [h], "pattern": pattern, "scale": scale, "angle": angle })).expect("hatch");
    (*s.doc().expect("doc")).clone()
}

fn hatch_notes(d: &Drawing, back: &Drawing) -> Vec<String> {
    losses_between(d, back).into_iter().filter(|l| l.feature.contains("hatch")).map(|l| format!("{} {}", l.feature, l.what)).collect()
}

#[test]
fn a_polyline_closed_by_a_repeated_point_is_kept() {
    let d = hatched(json!([[0, 0], [0, 8], [8, 8], [8, 0], [0, 0]]), "SOLID", 1.0, 0.0);
    assert!(d.model.iter().any(|e| matches!(e.kind, EntityKind::Hatch(_))));
    for (name, trip) in trips() {
        let back = trip(&d);
        assert!(hatch_notes(&d, &back).is_empty(), "{name}: {:?}", hatch_notes(&d, &back));
        // Saved again, still quiet.
        assert!(hatch_notes(&back, &trip(&back)).is_empty(), "{name} again");
    }
}

#[test]
fn re_saved_pattern_lines_are_kept_across_patterns_angles_and_scales() {
    let mut checked = 0;
    let mut noisy = Vec::new();
    for pattern in ["BRICK", "ANSI31", "NET", "DOTS", "EARTH"] {
        for angle in [0.0_f64, 15.0, 30.0, 45.0, 60.0, 90.0] {
            for scale in [0.5, 1.0, 1.488, 2.0, 3.3, 0.7, 1.9] {
                let d = hatched(json!([[0, 0], [40, 0], [40, 40], [0, 40]]), pattern, scale, angle);
                for (name, trip) in trips() {
                    let (first, second) = (trip(&d), trip(&trip(&d)));
                    checked += 1;
                    for n in hatch_notes(&d, &first).into_iter().chain(hatch_notes(&first, &second)) {
                        noisy.push(format!("{pattern} {angle} {scale} {name}: {n}"));
                    }
                }
            }
        }
    }
    println!("{checked} pattern × angle × scale × format saves checked");
    assert!(noisy.is_empty(), "{} false notices: {noisy:?}", noisy.len());
}

#[test]
fn a_loop_that_really_loses_a_vertex_and_a_pattern_line_that_changes_are_reported() {
    let d = hatched(json!([[0, 0], [8, 0], [8, 8], [0, 8]]), "ANSI31", 1.0, 0.0);
    let handle = d.model.iter().find(|e| matches!(e.kind, EntityKind::Hatch(_))).expect("hatch").handle;
    // Read back, the hatch holds its pattern lines (before, they are derived data).
    let d = trips().remove(0).1(&d);
    let mut back = d.clone();
    back.model.modify(handle, |e| {
        if let EntityKind::Hatch(h) = &mut e.kind
            && let Some(l) = h.loops.first_mut()
        {
            l.vertices.pop();
        }
    });
    assert!(!hatch_notes(&d, &back).is_empty(), "a lost vertex");
    let mut back = d.clone();
    back.model.modify(handle, |e| {
        if let EntityKind::Hatch(h) = &mut e.kind {
            for l in &mut h.pattern_lines {
                l.delta.1 += 1.0;
            }
        }
    });
    assert!(!hatch_notes(&d, &back).is_empty(), "a changed pattern line");
}

/// The soak's metric cell far from the origin: the readers' un-rotating of the file's pattern
/// lines leaves 1e-15 residue in components that should be 0 for some angle and scale pairs.
#[test]
fn a_metric_cell_hatched_with_awkward_angles_is_kept_when_saved_again() {
    let mut noisy = Vec::new();
    let mut checked = 0;
    for (pattern, angle, scale) in
        [("BRICK", 30.0, 1.488), ("ANSI37", 123.4, 1.0), ("ANSI37", 123.4, 2.5), ("BRICK", 30.0, 0.7), ("NET", 17.3, 1.488)]
    {
        let mut s = Session::new();
        s.new_drawing(true);
        s.execute("rectang", &json!({ "p1": [-99960, -100000], "p2": [-99950, -99992] })).expect("rectang");
        let h = s.doc().expect("doc").model.iter().next().expect("rectangle").handle.hex();
        s.execute("hatch", &json!({ "handles": [h], "pattern": pattern, "scale": scale, "angle": angle })).expect("hatch");
        let d = (*s.doc().expect("doc")).clone();
        for (name, trip) in trips() {
            let (first, second) = (trip(&d), trip(&trip(&d)));
            checked += 1;
            for n in hatch_notes(&d, &first).into_iter().chain(hatch_notes(&first, &second)) {
                noisy.push(format!("{pattern} {angle} {scale} {name}: {n}"));
            }
        }
    }
    println!("{checked} metric cell saves checked");
    assert!(noisy.is_empty(), "{noisy:?}");
}
