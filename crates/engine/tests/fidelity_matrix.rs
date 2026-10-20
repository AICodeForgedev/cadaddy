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

use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Face3d, Mesh, MeshFace, Solid, Space};
use cadaddy_engine::{
    Session,
    cmd::file::{IoHooks, set_io},
};
use cadaddy_io::{SaveOptions, Version};
use serde_json::{Value, json};

/// Known differences: `format|type|field`. Each is a loss to fix, or a version limit.
const KNOWN: &[&str] = &[
    // Versions older than the feature: AutoCAD 2000/2004 have no multileaders (written as a
    // leader with its text), multileader or table styles (2000), or DIMFXL (before 2007); MESH
    // came with 2010 (older versions get a polyface mesh).
    "DWG 2000|DimStyle|fixedExtLength",
    "DWG 2000|Leader|count 1→2",
    "DWG 2000|MLeaderStyle|CALLOUT missing",
    "DWG 2000|MText|count 2→3",
    "DWG 2000|Mesh|count 1→0",
    "DWG 2000|Multileader|count 1→0",
    "DWG 2000|Polyface Mesh|appeared",
    "DWG 2000|TableStyle|SCHEDULE missing",
    "DWG 2004|DimStyle|fixedExtLength",
    "DWG 2004|Leader|count 1→2",
    "DWG 2004|MText|count 2→3",
    "DWG 2004|Mesh|count 1→0",
    "DWG 2004|Multileader|count 1→0",
    "DWG 2004|Polyface Mesh|appeared",
    "DWG 2007|Mesh|count 1→0",
    "DWG 2007|Polyface Mesh|appeared",
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
    "DWG native 2000|Polyface Mesh|appeared",
    "DWG native 2004|Leader|count 1→2",
    "DWG native 2004|MText|count 2→3",
    "DWG native 2004|Mesh|count 1→0",
    "DWG native 2004|Multileader|count 1→0",
    "DWG native 2004|Polyface Mesh|appeared",
    "DWG native 2007|Mesh|count 1→0",
    "DWG native 2007|Polyface Mesh|appeared",
    "DWG native 2000|Viewport|kind.id",
    "DWG native 2004|Viewport|kind.id",
    "DWG native 2007|Viewport|kind.id",
    "DWG native 2010|Viewport|kind.id",
    "DWG native 2013|Viewport|kind.id",
    "DWG native 2018|Viewport|kind.id",
];

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
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
    let _ = d.add(&Space::Model, wall(), EntityKind::Mesh(m));
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
    json!({ "common": c, "kind": serde_json::to_value(&e.kind).unwrap_or(Value::Null) })
}

/// Entities by type, in drawing order, across model space and the layouts.
fn by_type(d: &Drawing) -> BTreeMap<String, Vec<Value>> {
    let mut m: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for e in d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())) {
        m.entry(e.kind.type_name().to_string()).or_default().push(fields(e));
    }
    m
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
            .map(|g| (g.name.to_ascii_uppercase(), json!({ "members": g.members.len(), "selectable": g.selectable, "description": g.description })))
            .collect(),
    );
    t.insert("View".into(), named(serde_json::to_value(&d.views).unwrap_or(Value::Null)));
    t.insert("UCS".into(), named(serde_json::to_value(&d.ucss).unwrap_or(Value::Null)));
    t.insert(
        "Block".into(),
        d.blocks
            .values()
            .filter(|b| !b.name.starts_with('*'))
            .map(|b| (b.name.to_ascii_uppercase(), json!({ "objects": b.entities.len(), "base": [b.base.x, b.base.y] })))
            .collect(),
    );
    t.insert(
        "Layout".into(),
        d.layouts.iter().map(|l| (l.name.to_ascii_uppercase(), json!({ "objects": l.entities.len(), "tab": l.tab_order }))).collect(),
    );
    t
}

/// What `after` lost or changed from `before`: (type, field) pairs.
fn compare(before: &Drawing, after: &Drawing) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let (a, b) = (by_type(before), by_type(after));
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
    let types: BTreeSet<String> = by_type(&d).keys().cloned().collect();
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
    let mut rows: BTreeSet<String> = types.clone();
    rows.extend(tables(&d).keys().cloned());
    let mut md = format!(
        "# File fidelity matrix\n\n{} entity types, {} table types; ✓ = read back the same, N = fields that changed.\n\n| type |",
        types.len(),
        rows.len() - types.len()
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
    assert!(types.len() >= 30, "only {} entity types made: {skipped:?}", types.len());
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
