//! A made-up drawing with one of each entity and table type, shared by the converter and DWG
//! interop tests (`#[path]`-included; not a test target of its own).

use std::path::Path;

use cadaddy_engine::Session;
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Face3d, Mesh, MeshFace, Solid, Space};
use serde_json::{Value, json};

/// A made-up drawing with one of each entity type (made by its command; the few without one
/// added directly) and some of each table type.
pub fn kitchen_sink(dir: &Path) -> Drawing {
    let mut s = Session::new();
    let run = |s: &mut Session, id: &str, p: Value| {
        s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"));
    };
    let last = |s: &Session| s.doc().map(|d| d.model.iter().last().map(|e| e.handle.hex()).unwrap_or_default()).unwrap_or_default();
    run(&mut s, "layer.new", json!({ "name": "A-WALL", "color": "red", "linetype": "Continuous", "lineweight": 0.5 }));
    run(&mut s, "linetype", json!({ "load": "DASHED" }));
    run(&mut s, "layer.new", json!({ "name": "A-HIDDEN", "color": 30, "linetype": "DASHED" }));
    run(&mut s, "style", json!({ "name": "Notes", "font": "Arial", "height": 0, "current": false }));
    run(&mut s, "dimstyle", json!({ "name": "Arch", "current": false }));
    run(&mut s, "mleaderstyle", json!({ "name": "Callout", "current": false }));
    run(&mut s, "tablestyle", json!({ "name": "Schedule", "current": false }));
    run(&mut s, "mlstyle", json!({ "name": "Double", "current": false }));
    run(&mut s, "ucs", json!({ "origin": [10, 10], "name": "Site" }));
    run(&mut s, "ucs", json!({ "world": true }));
    run(&mut s, "layer.current", json!({ "name": "A-WALL" }));
    run(&mut s, "line", json!({ "points": [[0, 0], [10, 5]] }));
    run(&mut s, "point", json!({ "at": [1, 2] }));
    run(&mut s, "circle", json!({ "center": [20, 0], "radius": 4 }));
    run(&mut s, "arc", json!({ "center": [30, 0], "radius": 3, "start": 10, "end": 120 }));
    run(&mut s, "ellipse", json!({ "center": [40, 0], "major": [5, 1], "ratio": 0.4 }));
    run(
        &mut s,
        "pline",
        json!({ "vertices": [[0, 20], { "p": [10, 20], "bulge": 0.5, "startWidth": 0.2, "endWidth": 0.4 }, [20, 25]], "closed": false }),
    );
    run(&mut s, "3dpoly", json!({ "points": [[0, 30, 0], [5, 30, 2], [10, 35, 4]] }));
    run(&mut s, "spline", json!({ "fit": [[0, 40], [5, 44], [10, 40], [15, 44]] }));
    run(&mut s, "spline", json!({ "control": [[20, 40], [25, 46], [30, 40], [35, 46]], "degree": 3 }));
    run(&mut s, "helix", json!({ "center": [50, 40], "baseRadius": 3, "topRadius": 2, "height": 8, "turns": 3 }));
    run(&mut s, "ray", json!({ "base": [60, 0], "through": [61, 1] }));
    run(&mut s, "xline", json!({ "base": [70, 0], "angle": 30 }));
    run(&mut s, "text", json!({ "at": [0, 60], "text": "PLAN %%c25", "height": 2.5, "rotation": 15, "justify": "MC" }));
    run(&mut s, "mtext", json!({ "at": [20, 60], "text": "{\\fArial|b1;Bold} line\\Pnext", "width": 30, "height": 2 }));
    run(&mut s, "attdef", json!({ "tag": "ROOM", "prompt": "Room number", "default": "101", "at": [0, 0], "height": 1 }));
    let mut hs = vec![last(&s)];
    run(&mut s, "circle", json!({ "center": [0, 0], "radius": 2 }));
    hs.push(last(&s));
    run(&mut s, "block", json!({ "name": "TAG", "base": [0, 0], "handles": hs, "keep": "delete" }));
    run(&mut s, "insert", json!({ "name": "TAG", "at": [40, 60], "scale": [1.5, 1.5], "rotation": 20, "attribs": { "ROOM": "204" } }));
    run(&mut s, "dimlinear", json!({ "p1": [0, 80], "p2": [20, 80], "at": [10, 85] }));
    run(&mut s, "dimaligned", json!({ "p1": [30, 80], "p2": [40, 90], "at": [33, 88] }));
    run(&mut s, "dimangular", json!({ "vertex": [50, 80], "p1": [60, 80], "p2": [55, 90], "at": [58, 86] }));
    run(&mut s, "dimradius", json!({ "center": [20, 0], "point": [24, 0] }));
    run(&mut s, "dimdiameter", json!({ "center": [20, 0], "point": [20, 4] }));
    run(&mut s, "dimordinate", json!({ "feature": [70, 80], "leader": [75, 85] }));
    run(&mut s, "mline", json!({ "points": [[0, 100], [20, 100], [20, 110]], "scale": 2, "justification": "zero" }));
    run(&mut s, "leader", json!({ "points": [[30, 100], [35, 105], [40, 105]], "text": "NOTE" }));
    run(&mut s, "mleader", json!({ "points": [[50, 100], [55, 106]], "text": "Callout" }));
    run(&mut s, "rectang", json!({ "p1": [70, 100], "p2": [80, 110] }));
    run(&mut s, "hatch", json!({ "points": [[75, 105]], "pattern": "ANSI31", "scale": 2, "angle": 15 }));
    run(
        &mut s,
        "table",
        json!({ "at": [0, 140], "rows": 2, "cols": 2, "cells": [["a", "b"], ["c", "1.5"]], "title": "Schedule", "header": ["H1", "H2"] }),
    );
    run(&mut s, "tolerance", json!({ "at": [40, 140], "symbol": "position", "tol1": "%%c0.1(M)", "datums": ["A", "B"] }));
    run(&mut s, "wipeout", json!({ "points": [[60, 140], [70, 140], [70, 145], [60, 145]] }));
    run(&mut s, "box", json!({ "corner": [0, 160, 0], "length": 5, "width": 4, "height": 3 }));
    run(&mut s, "circle", json!({ "center": [20, 165], "radius": 3 }));
    let c = last(&s);
    run(&mut s, "region", json!({ "handles": [c] }));
    run(&mut s, "sectionplane", json!({ "from": [0, 170], "to": [10, 170] }));
    run(&mut s, "light", json!({ "kind": "point", "position": [5, 5, 10] }));
    let png = dir.join("photo.png");
    let list = cadaddy_engine::render::DisplayList::default();
    let view = cadaddy_engine::render::raster::View::of_window(
        &cadaddy_engine::geom::Bounds2::new(cadaddy_engine::geom::Vec2::ZERO, cadaddy_engine::geom::Vec2::new(1.0, 1.0)),
        Some(8),
        Some(8),
    );
    std::fs::write(&png, cadaddy_engine::render::raster::render_png(&list, &view, &Default::default()).unwrap()).unwrap();
    run(&mut s, "imageattach", json!({ "path": png, "at": [80, 140], "scale": 5 }));
    let pdf = dir.join("sheet.pdf");
    run(&mut s, "plot", json!({ "path": pdf }));
    run(&mut s, "pdfattach", json!({ "path": pdf, "at": [100, 140] }));
    run(&mut s, "mview", json!({ "layout": "Layout1", "p1": [1, 1], "p2": [8, 6] }));
    run(&mut s, "layout", json!({ "option": "new", "name": "Sheet A" }));
    run(&mut s, "view", json!({ "option": "save", "name": "Front", "center": [10, 10], "height": 40 }));
    let first = s.doc().map(|d| d.model.iter().take(2).map(|e| e.handle.hex()).collect::<Vec<_>>()).unwrap();
    run(&mut s, "group", json!({ "name": "Pair", "handles": first }));
    let mut d = s.doc().unwrap().clone();
    let q = |x: f64, y: f64| [Vec3::new(x, y, 0.0), Vec3::new(x + 4.0, y, 0.0), Vec3::new(x, y + 3.0, 0.0), Vec3::new(x + 4.0, y + 3.0, 0.0)];
    let wall = || Common { layer: "A-WALL".into(), ..Common::default() };
    d.add(&Space::Model, wall(), EntityKind::Solid(Solid { corners: q(0.0, 200.0) })).unwrap();
    d.add(&Space::Model, wall(), EntityKind::Trace(Solid { corners: q(10.0, 200.0) })).unwrap();
    d.add(&Space::Model, wall(), EntityKind::Face3d(Face3d { corners: q(20.0, 200.0).map(|v| Vec3::new(v.x, v.y, v.x * 0.1)), hidden_edges: 2 }))
        .unwrap();
    let m = Mesh {
        vertices: vec![Vec3::new(30.0, 200.0, 0.0), Vec3::new(34.0, 200.0, 0.0), Vec3::new(32.0, 203.0, 1.0), Vec3::new(32.0, 201.0, 4.0)],
        faces: vec![
            MeshFace { v: vec![0, 1, 2], ..Default::default() },
            MeshFace { v: vec![0, 1, 3], ..Default::default() },
            MeshFace { v: vec![1, 2, 3], ..Default::default() },
        ],
        ..Mesh::default()
    };
    d.add(&Space::Model, wall(), EntityKind::Mesh(m)).unwrap();
    // Paper space: a title line on Layout1 beside its viewport (R12 keeps it; not the viewport).
    let title = cadaddy_engine::doc::Line { a: Vec3::new(1.0, 0.5, 0.0), b: Vec3::new(9.0, 0.5, 0.0) };
    d.add(&Space::Paper("Layout1".into()), Common::default(), EntityKind::Line(title)).unwrap();
    d
}
