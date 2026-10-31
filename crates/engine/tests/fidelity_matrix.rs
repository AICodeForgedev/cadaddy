//! File fidelity matrix: a drawing with one of every entity type and the table types (layers,
//! linetypes, text/dimension/multileader/table/multiline styles, blocks, groups, layouts, views,
//! UCSs) is written as DWG 2000, 2004, 2007, 2010, 2013 and 2018 (through the bridge and with
//! CADaddy's own codec), as ASCII and binary DXF and as .cdy, read back, and compared field by field (floats to 1e-6). What changed, per format and
//! type, is the matrix (`target/tmp/fidelity.md`, printed with `CADADDY_FIDELITY_REPORT=1`).
//!
//! The test fails on any difference not in [`KNOWN`] — the losses the matrix found when it was
//! made, each a fix to make (remove its line when it is fixed) — and on a known one that is gone
//! (so the list stays true).

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_engine::doc::geom::{Vec2, Vec3};
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Face3d, Mesh, MeshFace, MeshKind, RawTag, Solid, Space, Underlay, Unknown};
use cadaddy_engine::{
    Session,
    cmd::file::{IoHooks, set_io},
};
use cadaddy_io::{SaveOptions, Version};
use serde_json::{Value, json};

/// Known differences: `format|type|field`. Each is a loss to fix, or a version limit.
const KNOWN: &[&str] = &[
    // A justified TEXT is written as AutoCAD writes it: group 10 is where its baseline starts, the
    // alignment point is group 11 (it drew centred and right-aligned text from the wrong end
    // otherwise). Read back, `insert` holds the baseline start and `align_pt` the point it was
    // made at: the same text, stored the way AutoCAD stores it.
    "DWG 2000|Text|kind.insert.x",
    "DWG 2000|Text|kind.insert.y",
    "DWG 2004|Text|kind.insert.x",
    "DWG 2004|Text|kind.insert.y",
    "DWG 2007|Text|kind.insert.x",
    "DWG 2007|Text|kind.insert.y",
    "DWG 2010|Text|kind.insert.x",
    "DWG 2010|Text|kind.insert.y",
    "DWG 2013|Text|kind.insert.x",
    "DWG 2013|Text|kind.insert.y",
    "DWG 2018|Text|kind.insert.x",
    "DWG 2018|Text|kind.insert.y",
    "DWG native 2000|Text|kind.insert.x",
    "DWG native 2000|Text|kind.insert.y",
    "DWG native 2004|Text|kind.insert.x",
    "DWG native 2004|Text|kind.insert.y",
    "DWG native 2007|Text|kind.insert.x",
    "DWG native 2007|Text|kind.insert.y",
    "DWG native 2010|Text|kind.insert.x",
    "DWG native 2010|Text|kind.insert.y",
    "DWG native 2013|Text|kind.insert.x",
    "DWG native 2013|Text|kind.insert.y",
    "DWG native 2018|Text|kind.insert.x",
    "DWG native 2018|Text|kind.insert.y",
    "DXF|Text|kind.insert.x",
    "DXF|Text|kind.insert.y",
    "DXF binary|Text|kind.insert.x",
    "DXF binary|Text|kind.insert.y",
    // Versions older than the feature: AutoCAD 2000/2004 have no multileaders (written as a
    // leader with its text), multileader or table styles (2000); MESH
    // came with 2010 (older versions get a polyface mesh).
    "DWG 2000|Leader|count 1→2",
    "DWG 2000|MLeaderStyle|CALLOUT missing",
    "DWG 2000|MText|count 2→3",
    "DWG 2000|Mesh|count 1→0",
    "DWG 2000|Multileader|count 1→0",
    "DWG 2000|TableStyle|SCHEDULE missing",
    "DWG 2004|Leader|count 1→2",
    "DWG 2004|MText|count 2→3",
    "DWG 2004|Mesh|count 1→0",
    "DWG 2004|Multileader|count 1→0",
    "DWG 2007|Mesh|count 1→0",
    // The DWG bridge (not the native codec) keeps an object of a type CADaddy doesn't model as a
    // kept record and reads it back with its groups re-encoded: a real group (40) is lost. A fix
    // to make in the bridge's keep/stash path; the native codec reads it back as written.
    "DWG 2000|Proxy|kind.tags[].code",
    "DWG 2000|Proxy|kind.tags[].value",
    "DWG 2000|Proxy|kind.tags[len]",
    "DWG 2004|Proxy|kind.tags[].code",
    "DWG 2004|Proxy|kind.tags[].value",
    "DWG 2004|Proxy|kind.tags[len]",
    "DWG 2007|Proxy|kind.tags[].code",
    "DWG 2007|Proxy|kind.tags[].value",
    "DWG 2007|Proxy|kind.tags[len]",
    "DWG 2010|Proxy|kind.tags[].code",
    "DWG 2010|Proxy|kind.tags[].value",
    "DWG 2010|Proxy|kind.tags[len]",
    "DWG 2013|Proxy|kind.tags[].code",
    "DWG 2013|Proxy|kind.tags[].value",
    "DWG 2013|Proxy|kind.tags[len]",
    "DWG 2018|Proxy|kind.tags[].code",
    "DWG 2018|Proxy|kind.tags[].value",
    "DWG 2018|Proxy|kind.tags[len]",
    // The DWG bridge stores a SHAPE by its number in the shape file and gives it back without a
    // name (a fix to make; the native codec keeps the name).
    "DWG 2000|Shape|kind.name",
    "DWG 2004|Shape|kind.name",
    "DWG 2007|Shape|kind.name",
    "DWG 2010|Shape|kind.name",
    "DWG 2013|Shape|kind.name",
    "DWG 2018|Shape|kind.name",
    // Versions before 2010 have no MESH: it is written as a polyface mesh, so a second polyface
    // mesh appears next to the one made as a polyface (the same limit as "Mesh count 1→0").
    "DWG 2000|Polyface Mesh|count 1→2",
    "DWG 2004|Polyface Mesh|count 1→2",
    "DWG 2007|Polyface Mesh|count 1→2",
    "DWG native 2000|Polyface Mesh|count 1→2",
    "DWG native 2004|Polyface Mesh|count 1→2",
    "DWG native 2007|Polyface Mesh|count 1→2",
    // The DWG bridge numbers paper-space viewports again (by order in the layout).
    "DWG 2000|Viewport|kind.id",
    "DWG 2004|Viewport|kind.id",
    "DWG 2007|Viewport|kind.id",
    "DWG 2010|Viewport|kind.id",
    "DWG 2013|Viewport|kind.id",
    "DWG 2018|Viewport|kind.id",
    // CADaddy's own DWG reader and writer (Q203): the same version limits (without the bridge's
    // DIMFXL and 2000 style losses), and viewports numbered by their order in the layout too
    // (DWG doesn't store the number).
    "DWG native 2000|Leader|count 1→2",
    "DWG native 2000|MText|count 2→3",
    "DWG native 2000|Mesh|count 1→0",
    "DWG native 2000|Multileader|count 1→0",
    "DWG native 2004|Leader|count 1→2",
    "DWG native 2004|MText|count 2→3",
    "DWG native 2004|Mesh|count 1→0",
    "DWG native 2004|Multileader|count 1→0",
    "DWG native 2007|Mesh|count 1→0",
    "DWG native 2000|Viewport|kind.id",
    "DWG native 2004|Viewport|kind.id",
    "DWG native 2007|Viewport|kind.id",
    "DWG native 2010|Viewport|kind.id",
    "DWG native 2013|Viewport|kind.id",
    "DWG native 2018|Viewport|kind.id",
];

/// Every `EntityKind` variant by its `type_name` (a MESH, polyface mesh and polygon mesh are one
/// variant with three names; PDF and DWF underlays likewise). The kitchen sink must make exactly
/// these. **A new `EntityKind` variant (or `type_name`) must be added here, and made in
/// `kitchen_sink`** — the test fails until both are done.
const ALL_TYPES: &[&str] = &[
    "2D Solid",
    "3D Face",
    "3D Polyline",
    "3D Solid",
    "Arc",
    "Attribute Definition",
    "Block Reference",
    "Circle",
    "DWF Underlay",
    "Dimension",
    "Ellipse",
    "Hatch",
    "Helix",
    "Leader",
    "Light",
    "Line",
    "MLine",
    "MText",
    "Mesh",
    "Multileader",
    "NURBS Surface",
    "PDF Underlay",
    "Point",
    "Point Cloud",
    "Polygon Mesh",
    "Polyface Mesh",
    "Polyline",
    "Proxy",
    "Raster Image",
    "Ray",
    "Region",
    "Section Plane",
    "Shape",
    "Spline",
    "Table",
    "Text",
    "Tolerance",
    "Trace",
    "Viewport",
    "Wipeout",
    "Xline",
];

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
    cadaddy_engine::cmd::pointcloud::set_reader(cadaddy_io::pointcloud_io::read);
}

/// The drawing: one of each entity type, made by its command (the few without one added
/// directly), and some of each table type.
fn kitchen_sink(dir: &std::path::Path) -> (Drawing, Vec<String>) {
    let mut s = Session::new();
    let mut skipped = Vec::new();
    let mut run = |s: &mut Session, id: &str, p: Value| {
        if let Err(e) = s.execute(id, &p) {
            skipped.push(format!("{id}: {e}"));
        }
    };
    // Tables.
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
    // Entities.
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
    let defs: Vec<String> =
        s.doc().map(|d| d.model.iter().filter(|e| matches!(e.kind, EntityKind::AttDef(_))).map(|e| e.handle.hex()).collect()).unwrap_or_default();
    run(&mut s, "circle", json!({ "center": [0, 0], "radius": 2 }));
    let last = s.doc().map(|d| d.model.iter().last().map(|e| e.handle.hex()).unwrap_or_default()).unwrap_or_default();
    let mut hs = defs.clone();
    hs.push(last);
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
    let c = s.doc().map(|d| d.model.iter().last().map(|e| e.handle.hex()).unwrap_or_default()).unwrap_or_default();
    run(&mut s, "region", json!({ "handles": [c] }));
    run(&mut s, "sectionplane", json!({ "from": [0, 170], "to": [10, 170] }));
    run(&mut s, "light", json!({ "kind": "point", "position": [5, 5, 10] }));
    // Files referenced by IMAGE, PDFUNDERLAY and POINTCLOUD.
    let png = dir.join("photo.png");
    let list = cadaddy_engine::render::DisplayList::default();
    let view = cadaddy_engine::render::raster::View::of_window(
        &cadaddy_engine::geom::Bounds2::new(cadaddy_engine::geom::Vec2::ZERO, cadaddy_engine::geom::Vec2::new(1.0, 1.0)),
        Some(8),
        Some(8),
    );
    if let Some(bytes) = cadaddy_engine::render::raster::render_png(&list, &view, &Default::default()) {
        let _ = std::fs::write(&png, bytes);
    }
    run(&mut s, "imageattach", json!({ "path": png, "at": [80, 140], "scale": 5 }));
    let xyz = dir.join("scan.xyz");
    let _ = std::fs::write(&xyz, "0 0 0\n1 0 0\n0 1 0\n1 1 1\n");
    run(&mut s, "pointcloudattach", json!({ "path": xyz, "at": [90, 140, 0] }));
    let pdf = dir.join("sheet.pdf");
    run(&mut s, "plot", json!({ "path": pdf }));
    run(&mut s, "pdfattach", json!({ "path": pdf, "at": [100, 140] }));
    // Layouts, viewports, views, groups.
    run(&mut s, "mview", json!({ "layout": "Layout1", "p1": [1, 1], "p2": [8, 6] }));
    run(&mut s, "layout", json!({ "option": "new", "name": "Sheet A" }));
    run(&mut s, "view", json!({ "option": "save", "name": "Front", "center": [10, 10], "height": 40 }));
    let first = s.doc().map(|d| d.model.iter().take(2).map(|e| e.handle.hex()).collect::<Vec<_>>()).unwrap_or_default();
    run(&mut s, "group", json!({ "name": "Pair", "handles": first }));
    // A layout's page setup (paper, scale, plot area and plot style), not the defaults.
    run(
        &mut s,
        "pagesetup",
        json!({ "layout": "Sheet A", "paper": "A3", "landscape": true, "scale": 0.02, "scaleToFit": false,
                "plotStyleTable": "monochrome.ctb", "center": false, "lineweights": false, "margins": [5, 6, 7, 8] }),
    );
    // Model-space viewport configurations, a layer state, a material, an annotation scale, a plot
    // style table: each non-default, made by its command.
    run(&mut s, "-vports", json!({ "option": "3", "arrangement": "right" }));
    run(&mut s, "-vports", json!({ "option": "save", "name": "Three" }));
    run(&mut s, "-vports", json!({ "option": "single" }));
    run(&mut s, "layerstate.save", json!({ "name": "Walls only", "description": "made-up state" }));
    run(&mut s, "material.new", json!({ "name": "Brick", "color": "#aa4422", "roughness": 0.7, "metallic": 0.1, "opacity": 0.9 }));
    run(&mut s, "scalelistedit", json!({ "add": { "name": "1:37", "ratio": "1:37" } }));
    run(&mut s, "stylesmanager", json!({ "action": "new", "name": "site.ctb", "description": "made-up table" }));
    run(&mut s, "stylesmanager", json!({ "action": "new", "name": "named.stb", "kind": "named" }));
    run(&mut s, "stylesmanager", json!({ "action": "set", "name": "named.stb", "style": { "name": "Heavy", "lineweight": 0.7 } }));
    // Surfaces and shapes (a shape file made on the spot from made-up source).
    run(&mut s, "planesurf", json!({ "corners": [[0, 180, 0], [6, 186, 0]] }));
    let shp = dir.join("madeup.shp");
    let _ = std::fs::write(&shp, "*1,5,BOX\n014,010,01C,018,0\n");
    run(&mut s, "compile", json!({ "file": shp }));
    run(&mut s, "load", json!({ "file": dir.join("madeup") }));
    run(&mut s, "shape", json!({ "name": "BOX", "at": [10, 180], "size": 2, "rotation": 30, "widthFactor": 1.5, "oblique": 10 }));
    // Entity types no command makes.
    let mut d = s.doc().map_or_else(|_| Drawing::new_imperial(), |d| d.clone());
    let q = |x: f64, y: f64| [Vec3::new(x, y, 0.0), Vec3::new(x + 4.0, y, 0.0), Vec3::new(x, y + 3.0, 0.0), Vec3::new(x + 4.0, y + 3.0, 0.0)];
    let wall = || Common { layer: "A-WALL".into(), ..Common::default() };
    let _ = d.add(&Space::Model, wall(), EntityKind::Solid(Solid { corners: q(0.0, 200.0) }));
    let _ = d.add(&Space::Model, wall(), EntityKind::Trace(Solid { corners: q(10.0, 200.0) }));
    let _ =
        d.add(&Space::Model, wall(), EntityKind::Face3d(Face3d { corners: q(20.0, 200.0).map(|v| Vec3::new(v.x, v.y, v.x * 0.1)), hidden_edges: 2 }));
    let m = Mesh {
        vertices: vec![Vec3::new(30.0, 200.0, 0.0), Vec3::new(34.0, 200.0, 0.0), Vec3::new(32.0, 203.0, 1.0), Vec3::new(32.0, 201.0, 4.0)],
        faces: vec![
            MeshFace { v: vec![0, 1, 2], ..Default::default() },
            MeshFace { v: vec![0, 1, 3], ..Default::default() },
            MeshFace { v: vec![1, 2, 3], ..Default::default() },
        ],
        ..Mesh::default()
    };
    let _ = d.add(&Space::Model, wall(), EntityKind::Mesh(m.clone()));
    // A polyface mesh and a 2×2 polygon mesh as source objects, and an object of a type CADaddy
    // doesn't model, kept as read (type name "Proxy"; a real ACAD_PROXY_ENTITY is dropped on
    // writing when no class in the file names it, so this is an ARC ALIGNED TEXT).
    let polyface = Mesh { kind: MeshKind::Polyface, ..m.clone() };
    let _ = d.add(&Space::Model, wall(), EntityKind::Mesh(polyface.clone()));
    let grid = Mesh {
        vertices: vec![Vec3::new(40.0, 200.0, 0.0), Vec3::new(44.0, 200.0, 1.0), Vec3::new(40.0, 204.0, 2.0), Vec3::new(44.0, 204.0, 3.0)],
        // The quad between the four vertices: a file's reader makes the same face.
        faces: vec![MeshFace { v: vec![0, 1, 3, 2], hidden: 0 }],
        kind: MeshKind::Polygon { m: 2, n: 2, closed_m: false, closed_n: false },
    };
    let _ = d.add(&Space::Model, wall(), EntityKind::Mesh(grid));
    let proxy = Unknown {
        dxf_type: "ARCALIGNEDTEXT".into(),
        tags: vec![
            RawTag { code: 100, value: "AcDbEntity".into() },
            RawTag { code: 8, value: "A-WALL".into() },
            RawTag { code: 100, value: "AcDbArcAlignedText".into() },
            RawTag { code: 1, value: "MADE UP".into() },
            RawTag { code: 40, value: "2.5".into() },
        ],
    };
    let _ = d.add(&Space::Model, wall(), EntityKind::Unknown(proxy));
    // A DWF underlay (placed directly: attaching one needs a DWF reader and a DWF file).
    let dwf = Underlay {
        insert: Vec3::new(50.0, 200.0, 0.0),
        u: Vec3::new(1.0, 0.0, 0.0),
        v: Vec3::new(0.0, 1.0, 0.0),
        size: Vec2::new(8.5, 11.0),
        path: "made-up-plan.dwf".into(),
        page: 2,
        name: "Sheet 2".into(),
        clip: Vec::new(),
        clipping: false,
        fade: 30,
        contrast: 40,
        monochrome: false,
        layers_off: Vec::new(),
    };
    let _ = d.add(&Space::Model, wall(), EntityKind::Underlay(Box::new(dwf)));
    // PAGESETUP can't set a layout's plot window; set one here.
    if let Some(l) = d.layouts.iter_mut().find(|l| l.name == "Sheet A") {
        l.page.plot_area = "window".into();
        l.page.plot_window = [1.0, 2.0, 30.0, 40.0];
    }
    // Extended data of two applications (their names are the drawing's APPID table).
    let tag = |code: i32, v: &str| RawTag { code, value: v.into() };
    let first_line = d.model.iter().find(|e| matches!(e.kind, EntityKind::Line(_))).map(|e| e.handle);
    if let Some(h) = first_line {
        let _ = d.modify_entity(h, |e| {
            e.common.xdata = vec![tag(1001, "MADEUP_APP_A"), tag(1000, "note"), tag(1001, "MADEUP_APP_B"), tag(1070, "7")].into();
        });
    }
    (d, skipped)
}

/// The formats of the matrix: (column name, file name, options).
fn formats() -> Vec<(String, &'static str, SaveOptions)> {
    let mut v = Vec::new();
    let dwg = [
        ("2000", Version::R2000),
        ("2004", Version::R2004),
        ("2007", Version::R2007),
        ("2010", Version::R2010),
        ("2013", Version::R2013),
        ("2018", Version::R2018),
    ];
    for (n, ver) in dwg {
        v.push((format!("DWG {n}"), "x.dwg", SaveOptions { version: Some(ver), binary: false }));
    }
    // CADaddy's own DWG reader and writer (Q203), side by side with the bridge's columns.
    for (n, ver) in dwg {
        v.push((format!("DWG native {n}"), "x.dwg", SaveOptions { version: Some(ver), binary: false }));
    }
    v.push(("DXF".into(), "x.dxf", SaveOptions { version: Some(Version::R2018), binary: false }));
    v.push(("DXF binary".into(), "x.dxf", SaveOptions { version: Some(Version::R2018), binary: true }));
    v.push(("CDY".into(), "x.cdy", SaveOptions::default()));
    v
}

thread_local! {
    /// The first values seen for each differing path (for the report).
    static EXAMPLES: std::cell::RefCell<BTreeMap<String, String>> = const { std::cell::RefCell::new(BTreeMap::new()) };
}

thread_local! {
    /// Fields unset in the drawing that a format filled in (not losses).
    static FILLED: std::cell::RefCell<BTreeSet<String>> = const { std::cell::RefCell::new(BTreeSet::new()) };
}

fn example(path: &str, a: &Value, b: &Value) {
    let short = |v: &Value| v.to_string().chars().take(80).collect::<String>();
    EXAMPLES.with(|e| {
        e.borrow_mut().entry(path.to_string()).or_insert_with(|| format!("{} → {}", short(a), short(b)));
    });
}

/// Field paths where two values differ (numbers to 1e-6; array indices kept out of the path so
/// the report groups them).
fn differ(a: &Value, b: &Value, path: &str, out: &mut BTreeSet<String>) {
    // Unset before and filled in by writing (a dimension's block name and text point, a hatch's
    // pattern lines, an underlay's name, an image's clip): nothing was lost.
    if a.is_null() || (path.ends_with("textMid.x") || path.ends_with("textMid.y")) && a.as_f64() == Some(0.0) {
        if a != b {
            FILLED.with(|f| f.borrow_mut().insert(path.to_string()));
        }
        return;
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            if (x - y).abs() > 1e-6 * (1.0 + x.abs().max(y.abs())) {
                example(path, a, b);
                out.insert(path.to_string());
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                differ(x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), &p, out);
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                example(&format!("{path}[len]"), a, b);
                out.insert(format!("{path}[len]"));
            }
            for (i, j) in x.iter().zip(y) {
                differ(i, j, &format!("{path}[]"), out);
            }
        }
        _ if a != b => {
            example(path, a, b);
            out.insert(path.to_string());
        }
        _ => {}
    }
}

/// An entity's fields to compare: its common properties (no handle) and its geometry.
fn fields(e: &cadaddy_engine::doc::Entity) -> Value {
    let mut c = serde_json::to_value(&e.common).unwrap_or(Value::Null);
    if let Some(o) = c.as_object_mut() {
        o.remove("handle");
        o.remove("owner");
    }
    let mut kind = serde_json::to_value(&e.kind).unwrap_or(Value::Null);
    // A kept foreign object holds its own handle (5) and owner (330) groups once read: like the
    // handle and owner of every object, not compared.
    if let Some(tags) = kind.get_mut("tags").and_then(Value::as_array_mut) {
        tags.retain(|t| !matches!(t["code"].as_i64(), Some(5 | 330)));
    }
    json!({ "common": c, "kind": kind })
}

/// Entities by type, in drawing order, across model space and the layouts.
fn by_type(d: &Drawing) -> BTreeMap<String, Vec<Value>> {
    let mut m: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for e in d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())) {
        m.entry(e.kind.type_name().to_string()).or_default().push(fields(e));
    }
    m
}

/// The contents of each block (not the `*` ones), by block and type, in order: rows such as
/// "Block TAG · Attribute Definition", compared with the same field differ as the drawing's objects.
fn block_contents(d: &Drawing) -> BTreeMap<String, Vec<Value>> {
    let mut m: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for b in d.blocks.values().filter(|b| !b.name.starts_with('*')) {
        for e in b.entities.iter() {
            m.entry(format!("Block {} · {}", b.name.to_ascii_uppercase(), e.kind.type_name())).or_default().push(fields(e));
        }
    }
    m
}

/// Every entity type name made: in the drawing's spaces and inside its blocks.
fn made_types(d: &Drawing) -> BTreeSet<String> {
    let mut t: BTreeSet<String> = by_type(d).into_keys().collect();
    for b in d.blocks.values().filter(|b| !b.name.starts_with('*')) {
        t.extend(b.entities.iter().map(|e| e.kind.type_name().to_string()));
    }
    t
}

/// The table types by entry name.
fn tables(d: &Drawing) -> BTreeMap<String, BTreeMap<String, Value>> {
    let named = |v: Value| -> BTreeMap<String, Value> {
        v.as_array().into_iter().flatten().filter_map(|x| Some((x["name"].as_str()?.to_ascii_uppercase(), x.clone()))).collect()
    };
    let mut t = BTreeMap::new();
    t.insert("Layer".into(), named(serde_json::to_value(&d.layers).unwrap_or(Value::Null)));
    t.insert("Linetype".into(), named(serde_json::to_value(&d.linetypes).unwrap_or(Value::Null)));
    t.insert("TextStyle".into(), named(serde_json::to_value(&d.text_styles).unwrap_or(Value::Null)));
    t.insert("DimStyle".into(), named(serde_json::to_value(&d.dim_styles).unwrap_or(Value::Null)));
    t.insert("MLeaderStyle".into(), named(serde_json::to_value(&d.mleader_styles).unwrap_or(Value::Null)));
    t.insert("TableStyle".into(), named(serde_json::to_value(&d.table_styles).unwrap_or(Value::Null)));
    t.insert("MLineStyle".into(), named(serde_json::to_value(&d.mline_styles).unwrap_or(Value::Null)));
    t.insert(
        "Group".into(),
        d.groups
            .iter()
            .map(|g| {
                // Who is in it, by type and in order (handles are not kept by every format).
                let members: Vec<&str> = g.members.iter().map(|h| d.entity(*h).map_or("(missing)", |e| e.kind.type_name())).collect();
                (g.name.to_ascii_uppercase(), json!({ "members": members, "selectable": g.selectable, "description": g.description }))
            })
            .collect(),
    );
    t.insert("View".into(), named(serde_json::to_value(&d.views).unwrap_or(Value::Null)));
    t.insert("UCS".into(), named(serde_json::to_value(&d.ucss).unwrap_or(Value::Null)));
    t.insert(
        "Block".into(),
        d.blocks
            .values()
            .filter(|b| !b.name.starts_with('*'))
            .map(|b| {
                (
                    b.name.to_ascii_uppercase(),
                    json!({ "objects": b.entities.len(), "base": [b.base.x, b.base.y, b.base.z], "description": b.description }),
                )
            })
            .collect(),
    );
    t.insert(
        "Layout".into(),
        d.layouts
            .iter()
            .map(|l| {
                let page = serde_json::to_value(&l.page).unwrap_or(Value::Null);
                (l.name.to_ascii_uppercase(), json!({ "objects": l.entities.len(), "tab": l.tab_order, "page": page }))
            })
            .collect(),
    );
    t.insert("VPORT".into(), named(serde_json::to_value(&d.vports).unwrap_or(Value::Null)));
    // Registered applications: ACAD and every application the objects' extended data names (what
    // the APPID table is made from).
    let ents = d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())).chain(d.blocks.values().flat_map(|b| b.entities.iter()));
    let mut apps: BTreeMap<String, Value> = BTreeMap::new();
    apps.insert("ACAD".into(), json!({ "name": "ACAD" }));
    for e in ents {
        for x in e.common.xdata.iter().filter(|x| x.code == 1001) {
            apps.insert(x.value.to_ascii_uppercase(), json!({ "name": x.value }));
        }
    }
    t.insert("APPID".into(), apps);
    t.insert("LayerState".into(), named(serde_json::to_value(&d.layer_states).unwrap_or(Value::Null)));
    t.insert("Material".into(), named(serde_json::to_value(&d.materials).unwrap_or(Value::Null)));
    t.insert("AnnoScale".into(), named(serde_json::to_value(&d.scales).unwrap_or(Value::Null)));
    t.insert("PlotStyleTable".into(), named(serde_json::to_value(&d.plot_style_tables).unwrap_or(Value::Null)));
    t
}

/// What `after` lost or changed from `before`: (type, field) pairs.
fn compare(before: &Drawing, after: &Drawing) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let (mut a, mut b) = (by_type(before), by_type(after));
    a.extend(block_contents(before));
    b.extend(block_contents(after));
    for (ty, xs) in &a {
        let ys = b.get(ty).cloned().unwrap_or_default();
        if xs.len() != ys.len() {
            out.insert((ty.clone(), format!("count {}→{}", xs.len(), ys.len())));
            continue;
        }
        let mut paths = BTreeSet::new();
        for (x, y) in xs.iter().zip(&ys) {
            differ(x, y, "", &mut paths);
        }
        out.extend(paths.into_iter().map(|p| (ty.clone(), p)));
    }
    for ty in b.keys().filter(|t| !a.contains_key(*t)) {
        out.insert((ty.clone(), "appeared".into()));
    }
    let (ta, tb) = (tables(before), tables(after));
    for (kind, xs) in &ta {
        let ys = tb.get(kind).cloned().unwrap_or_default();
        for (name, x) in xs {
            match ys.get(name) {
                None => {
                    out.insert((kind.clone(), format!("{name} missing")));
                }
                Some(y) => {
                    let mut paths = BTreeSet::new();
                    differ(x, y, "", &mut paths);
                    out.extend(paths.into_iter().map(|p| (kind.clone(), p)));
                }
            }
        }
    }
    out
}

#[test]
fn fidelity_matrix() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-fidelity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a folder");
    let (d, skipped) = kitchen_sink(&dir);
    let types = made_types(&d);
    let rows_made: BTreeSet<String> = by_type(&d).into_keys().chain(block_contents(&d).into_keys()).collect();
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut cells: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut details = String::new();
    let cols = formats();
    // CADADDY_FIDELITY_KEEP=<folder>: the DXF files and what they hold, for the ezdxf cross-check
    // (tools/fidelity/ezdxf_check.py).
    let keep = std::env::var_os("CADADDY_FIDELITY_KEEP").map(std::path::PathBuf::from);
    if let Some(k) = &keep {
        let _ = std::fs::create_dir_all(k);
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for e in d.model.iter() {
            *counts.entry(e.kind.dxf_name().to_string()).or_default() += 1;
        }
        let _ = std::fs::write(k.join("manifest.json"), json!({ "model": counts }).to_string());
    }
    for (col, name, opts) in &cols {
        let native = col.starts_with("DWG native");
        let bytes = match (native, opts.version) {
            (true, Some(v)) => cadaddy_io::write_dwg(&d, v, cadaddy_io::DwgEngine::Native).map_err(|e| e.to_string()),
            _ => cadaddy_io::write_with(&d, name, opts).map_err(|e| e.to_string()),
        };
        if let (Some(k), Ok(b)) = (&keep, &bytes)
            && name.ends_with(".dxf")
        {
            let _ = std::fs::write(k.join(format!("{}.dxf", col.replace(' ', "_"))), b);
        }
        let back = bytes.and_then(|b| {
            if native { cadaddy_io::read_dwg(&b, cadaddy_io::DwgEngine::Native) } else { cadaddy_io::read(&b, name) }.map_err(|e| e.to_string())
        });
        let back = match back {
            Ok(b) => b,
            Err(e) => {
                found.insert(format!("{col}|*|{e}"));
                continue;
            }
        };
        for (ty, field) in compare(&d, &back) {
            *cells.entry((ty.clone(), col.clone())).or_default() += 1;
            let ex = EXAMPLES.with(|e| e.borrow().get(&field).cloned()).map(|v| format!(" ({v})")).unwrap_or_default();
            details.push_str(&format!("- {col} · {ty}: {field}{ex}\n"));
            found.insert(format!("{col}|{ty}|{field}"));
        }
    }
    // The matrix.
    let mut rows: BTreeSet<String> = rows_made.clone();
    rows.extend(tables(&d).keys().cloned());
    let mut md = format!(
        "# File fidelity matrix\n\n{} entity types, {} block contents rows, {} table types; ✓ = read back the same, N = fields that changed.\n\n| type |",
        types.len(),
        rows_made.len() - by_type(&d).len(),
        rows.len() - rows_made.len()
    );
    for (col, ..) in &cols {
        md.push_str(&format!(" {col} |"));
    }
    md.push_str("\n|---|");
    md.push_str(&"---|".repeat(cols.len()));
    md.push('\n');
    for r in &rows {
        md.push_str(&format!("| {r} |"));
        for (col, ..) in &cols {
            match cells.get(&(r.clone(), col.clone())) {
                Some(n) => md.push_str(&format!(" {n} |")),
                None => md.push_str(" ✓ |"),
            }
        }
        md.push('\n');
    }
    md.push_str(&format!("\n## Differences\n\n{details}"));
    let filled = FILLED.with(|f| f.borrow().iter().cloned().collect::<Vec<_>>().join(", "));
    md.push_str(&format!("\n## Filled in on writing (unset before; not losses)\n\n{filled}\n"));
    if !skipped.is_empty() {
        md.push_str(&format!("\n## Not in the drawing (the command refused)\n\n- {}\n", skipped.join("\n- ")));
    }
    let out = option_env!("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from).join("fidelity.md");
    let _ = std::fs::write(&out, &md);
    if std::env::var("CADADDY_FIDELITY_REPORT").is_ok() {
        println!("{md}\n(report: {})", out.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(skipped.is_empty(), "commands that made the drawing refused: {skipped:?}");
    let all: BTreeSet<String> = ALL_TYPES.iter().map(|t| (*t).to_string()).collect();
    assert_eq!(types, all, "entity types made (left) vs every EntityKind type name (right)");
    // The tables and containers are really in the drawing (a vacuous comparison proves nothing).
    let t = tables(&d);
    for (kind, name) in [
        ("VPORT", "THREE"),
        ("APPID", "MADEUP_APP_A"),
        ("APPID", "MADEUP_APP_B"),
        ("LayerState", "WALLS ONLY"),
        ("Material", "BRICK"),
        ("AnnoScale", "1:37"),
        ("PlotStyleTable", "SITE.CTB"),
        ("PlotStyleTable", "NAMED.STB"),
        ("Layout", "SHEET A"),
        ("Group", "PAIR"),
        ("Block", "TAG"),
    ] {
        assert!(
            t.get(kind).is_some_and(|m| m.contains_key(name)),
            "{kind} {name} is not in the drawing: {:?}",
            t.get(kind).map(|m| m.keys().collect::<Vec<_>>())
        );
    }
    assert_eq!(t["Group"]["PAIR"]["members"].as_array().map(Vec::len), Some(2));
    assert!(t["Layout"]["SHEET A"]["page"]["paper"].as_str().is_some_and(|p| p.contains("A3")));
    assert_eq!(t["Layout"]["SHEET A"]["page"]["plotArea"], "window");
    assert!(rows_made.contains("Block TAG · Attribute Definition") && rows_made.contains("Block TAG · Circle"), "{rows_made:?}");
    let known: BTreeSet<String> = KNOWN.iter().map(|s| (*s).to_string()).collect();
    let new: Vec<&String> = found.difference(&known).collect();
    let fixed: Vec<&String> = known.difference(&found).collect();
    assert!(
        new.is_empty(),
        "{} new difference(s) (see {}):\n{}",
        new.len(),
        out.display(),
        new.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
    );
    assert!(fixed.is_empty(), "fixed (remove from KNOWN): {fixed:?}");
}

/// The comparison itself sees what it is for: a change inside a block, a group's members, a
/// layout's page setup, a viewport configuration or an application name is a difference.
#[test]
fn matrix_sees_changes_in_blocks_groups_layouts_and_tables() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-fidelity-mut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a folder");
    let (d, _) = kitchen_sink(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(compare(&d, &d).is_empty(), "a drawing differs from itself");
    let changed = |f: &dyn Fn(&mut Drawing)| {
        let mut e = d.clone();
        f(&mut e);
        compare(&d, &e)
    };
    let has = |set: &BTreeSet<(String, String)>, ty: &str, field: &str| set.iter().any(|(t, f)| t == ty && f.contains(field));
    // Inside a block: an attribute definition's tag and the nested circle's radius.
    let diff = changed(&|e| {
        if let Some(b) = e.blocks.get_mut("TAG").map(std::sync::Arc::make_mut) {
            let hs: Vec<_> = b.entities.iter().map(|x| x.handle).collect();
            for h in hs {
                b.entities.modify(h, |x| match &mut x.kind {
                    EntityKind::AttDef(a) => a.tag = "OTHER".into(),
                    EntityKind::Circle(c) => c.radius += 1.0,
                    _ => {}
                });
            }
        }
    });
    assert!(has(&diff, "Block TAG · Attribute Definition", "tag"), "{diff:?}");
    assert!(has(&diff, "Block TAG · Circle", "radius"), "{diff:?}");
    // A group: the members' order.
    let diff = changed(&|e| {
        if let Some(g) = e.groups.iter_mut().find(|g| g.name == "Pair") {
            g.members.reverse();
        }
    });
    assert!(has(&diff, "Group", "members"), "{diff:?}");
    // A layout's page setup: paper, scale, plot area and plot style table.
    for (field, f) in [
        ("paper", (|p: &mut cadaddy_engine::doc::PageSetup| p.paper = "A4".into()) as fn(&mut cadaddy_engine::doc::PageSetup)),
        ("scale", |p| p.scale = 0.5),
        ("plotArea", |p| p.plot_area = "layout".into()),
        ("plotStyleTable", |p| p.plot_style_table = "grayscale.ctb".into()),
    ] {
        let diff = changed(&|e| {
            if let Some(l) = e.layouts.iter_mut().find(|l| l.name == "Sheet A") {
                f(&mut l.page);
            }
        });
        assert!(has(&diff, "Layout", field), "{field}: {diff:?}");
    }
    // The tables that had no row: a viewport configuration, a layer state, a material, a scale, a
    // plot style table, and an application name (the object carrying it loses its extended data).
    let diff = changed(&|e| {
        if let Some(v) = e.vports.iter_mut().find(|v| v.name == "Three") {
            v.tiles.truncate(1);
        }
    });
    assert!(has(&diff, "VPORT", "tiles"), "{diff:?}");
    let diff = changed(&|e| e.layer_states.iter_mut().for_each(|s| s.description = "x".into()));
    assert!(has(&diff, "LayerState", "description"), "{diff:?}");
    let diff = changed(&|e| e.materials.iter_mut().for_each(|m| m.roughness += 0.1));
    assert!(has(&diff, "Material", "roughness"), "{diff:?}");
    let diff = changed(&|e| e.scales.retain(|s| s.name != "1:37"));
    assert!(has(&diff, "AnnoScale", "1:37 missing"), "{diff:?}");
    let diff = changed(&|e| e.plot_style_tables.iter_mut().for_each(|t| t.description = "x".into()));
    assert!(has(&diff, "PlotStyleTable", "description"), "{diff:?}");
    let diff = changed(&|e| {
        let hs: Vec<_> = e.model.iter().filter(|x| !x.common.xdata.is_empty()).map(|x| x.handle).collect();
        for h in hs {
            let _ = e.modify_entity(h, |x| x.common.xdata = Vec::new().into());
        }
    });
    assert!(has(&diff, "APPID", "MADEUP_APP_A missing"), "{diff:?}");
}
