//! Classic 2D commands: TRACE, LOAD / SHAPE / COMPILE / MKSHAPE, MKLTYPE, ATTEXT and COPYM —
//! JSON forms, typed forms, saving and reading back, and bad input refused.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use cadaddy_engine::geom::Vec2;
use serde_json::{Value, json};

/// A fresh folder for one test's files.
fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-classic2d-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn path(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

fn handles(v: &Value) -> Vec<Handle> {
    v["handles"].as_array().unwrap().iter().map(|h| Handle::parse_hex(h.as_str().unwrap()).unwrap()).collect()
}

/// The newest object's handle (hex).
fn last(s: &Session) -> String {
    s.doc().unwrap().model.iter().last().unwrap().handle.hex()
}

/// File formats for SAVEAS / OPEN (io is a lower layer than the engine).
fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

fn near(a: Vec2, b: Vec2) -> bool {
    a.dist(b) < 1e-6
}

/// A made-up shape source: a unit box and a diagonal tick.
const SHP: &str = "; made-up shapes for tests\n*1,5,BOX\n014,010,01C,018,0\n*2,9,TICK\n2,8,(-2,3),1,8,(2,-3),0\n";

// ---------- TRACE ----------

#[test]
fn trace_makes_mitred_segments() {
    let mut s = Session::new();
    let r = s.execute("trace", &json!({ "points": [[0, 0], [10, 0], [10, 10]], "width": 2 })).unwrap();
    let hs = handles(&r);
    assert_eq!(hs.len(), 2);
    let d = s.doc().unwrap();
    let corners = |h: Handle| match &d.entity(h).unwrap().kind {
        EntityKind::Trace(t) => t.corners.map(|c| c.xy()),
        k => panic!("{k:?}"),
    };
    // First segment: square start, its end mitred to the corner (11,-1) outside, (9,1) inside.
    let a = corners(hs[0]);
    assert!(near(a[0], Vec2::new(0.0, 1.0)) && near(a[1], Vec2::new(0.0, -1.0)), "{a:?}");
    assert!(near(a[2], Vec2::new(9.0, 1.0)) && near(a[3], Vec2::new(11.0, -1.0)), "{a:?}");
    // The second starts where the first ends and ends square.
    let b = corners(hs[1]);
    assert!(near(b[0], a[2]) && near(b[1], a[3]), "{b:?}");
    assert!(near(b[2], Vec2::new(9.0, 10.0)) && near(b[3], Vec2::new(11.0, 10.0)), "{b:?}");
    // The width became TRACEWID; the next call uses it.
    assert_eq!(s.doc().unwrap().header.f64("TRACEWID", 0.0), 2.0);
    let r = s.execute("trace", &json!({ "points": [[0, 20], [5, 20]] })).unwrap();
    let c = match &s.doc().unwrap().entity(handles(&r)[0]).unwrap().kind {
        EntityKind::Trace(t) => t.corners,
        _ => unreachable!(),
    };
    assert!((c[0].y - c[1].y - 2.0).abs() < 1e-9);
    for bad in [json!({}), json!({ "points": [[0, 0]] }), json!({ "points": [[1, 1], [1, 1]] }), json!({ "points": [[0, 0], [1, 0]], "width": -1 })] {
        assert!(s.execute("trace", &bad).is_err(), "{bad}");
    }
}

#[test]
fn trace_typed_draws_each_segment_as_the_next_point_comes() {
    let mut s = Session::new();
    for t in ["trace", "0.5", "0,0", "10,0", "10,5", "0,5", ""] {
        s.cmdline(t).unwrap();
    }
    assert!(s.running.is_none());
    let d = s.doc().unwrap();
    let traces: Vec<_> = d.model.iter().filter_map(|e| if let EntityKind::Trace(t) = &e.kind { Some(t.corners) } else { None }).collect();
    assert_eq!(traces.len(), 3);
    // Each joins the next: the end of one is the start of the other.
    for w in traces.windows(2) {
        assert!(near(w[0][2].xy(), w[1][0].xy()) && near(w[0][3].xy(), w[1][1].xy()));
    }
    assert_eq!(d.header.f64("TRACEWID", 0.0), 0.5);
    // The typed default comes from TRACEWID.
    s.cmdline("trace").unwrap();
    assert!(s.prompt_text().contains("<0.5000>"), "{}", s.prompt_text());
    s.cmdline("").unwrap();
}

// ---------- shapes ----------

#[test]
fn compile_load_and_shape() {
    let dir = scratch("shapes");
    let src = dir.join("madeup.shp");
    std::fs::write(&src, SHP).unwrap();
    let mut s = Session::new();
    // COMPILE writes madeup.shx next to the source.
    let r = s.execute("compile", &json!({ "file": path(&src) })).unwrap();
    assert_eq!(r["shapes"], 2);
    let shx = dir.join("madeup.shx");
    assert!(shx.exists());
    // SHAPE needs the file loaded.
    assert!(s.execute("shape", &json!({ "name": "BOX", "at": [0, 0] })).is_err());
    let r = s.execute("load", &json!({ "file": path(&dir.join("madeup")) })).unwrap();
    assert_eq!(r["shapes"], json!(["BOX", "TICK"]));
    let r = s.execute("shape", &json!({ "name": "box", "at": [5, 5], "size": 2, "rotation": 90 })).unwrap();
    let h = Handle::parse_hex(r["handle"].as_str().unwrap()).unwrap();
    {
        let d = s.doc().unwrap();
        let EntityKind::Shape(sh) = &d.entity(h).unwrap().kind else { panic!() };
        assert_eq!((sh.name.as_str(), sh.size), ("BOX", 2.0));
        // Drawn: a box 2 across, turned a quarter, left of the insertion point.
        let st = cadaddy_engine::render::shapes::strokes(d, sh);
        let b = cadaddy_engine::geom::Bounds2::from_points(st.iter().flatten().copied());
        assert!(near(b.min, Vec2::new(3.0, 5.0)) && near(b.max, Vec2::new(5.0, 7.0)), "{b:?}");
        // Picked on its strokes.
        let picked = cadaddy_engine::select::pick(d, &cadaddy_engine::doc::Space::Model, Vec2::new(3.0, 6.0), 0.05);
        assert_eq!(picked, Some(h));
    }
    let r = s.execute("shape", &json!({ "list": true })).unwrap();
    assert_eq!(r["shapes"].as_array().unwrap().len(), 2);
    // MOVE / MIRROR keep it drawn in the right place.
    s.execute("move", &json!({ "handles": [h.hex()], "delta": [10, 0] })).unwrap();
    s.execute("mirror", &json!({ "handles": [h.hex()], "p1": [20, 0], "p2": [20, 10], "erase": true })).unwrap();
    {
        let d = s.doc().unwrap();
        let sh = d.model.iter().find_map(|e| if let EntityKind::Shape(sh) = &e.kind { Some(sh.clone()) } else { None }).unwrap();
        let st = cadaddy_engine::render::shapes::strokes(d, &sh);
        let b = cadaddy_engine::geom::Bounds2::from_points(st.iter().flatten().copied());
        // Moved to x 13..15, mirrored about x = 20: 25..27.
        assert!(near(b.min, Vec2::new(25.0, 5.0)) && near(b.max, Vec2::new(27.0, 7.0)), "{b:?}");
    }
    // Saved as DXF, the shape and the loaded file come back.
    hooks();
    let dxf = dir.join("shapes.dxf");
    s.execute("saveas", &json!({ "path": path(&dxf) })).unwrap();
    let mut t = Session::new();
    t.execute("open", &json!({ "path": path(&dxf) })).unwrap();
    let d = t.doc().unwrap();
    assert_eq!(d.shape_files.len(), 1);
    assert!(d.shape_files[0].to_ascii_lowercase().ends_with("madeup.shx"));
    let sh = d.model.iter().find_map(|e| if let EntityKind::Shape(sh) = &e.kind { Some(sh.clone()) } else { None }).unwrap();
    assert_eq!(sh.name, "BOX");
    assert!(!cadaddy_engine::render::shapes::strokes(d, &sh).is_empty());
    // And as Release 12 DXF.
    let r12 = dir.join("shapes-r12.dxf");
    s.execute("saveas", &json!({ "path": path(&r12), "version": 12 })).unwrap();
    let mut t12 = Session::new();
    t12.execute("open", &json!({ "path": path(&r12) })).unwrap();
    let d = t12.doc().unwrap();
    assert_eq!(d.shape_files.len(), 1);
    let sh12 = d.model.iter().find_map(|e| if let EntityKind::Shape(sh) = &e.kind { Some(sh.clone()) } else { None }).unwrap();
    assert_eq!(sh12, sh);
    // And as .cdy.
    let cdy = dir.join("shapes.cdy");
    s.execute("saveas", &json!({ "path": path(&cdy) })).unwrap();
    let mut u = Session::new();
    u.execute("open", &json!({ "path": path(&cdy) })).unwrap();
    assert_eq!(u.doc().unwrap().shape_files, s.doc().unwrap().shape_files);
    assert_eq!(u.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Shape(_))).count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shapes_typed() {
    let dir = scratch("shapes-typed");
    let src = dir.join("typed.shp");
    std::fs::write(&src, SHP).unwrap();
    let mut s = Session::new();
    s.cmdline("compile").unwrap();
    s.cmdline(&path(&src)).unwrap();
    assert!(s.log.iter().any(|l| l.contains("Compilation successful")), "{:?}", s.log);
    s.cmdline("load").unwrap();
    s.cmdline(&path(&dir.join("typed.shx"))).unwrap();
    for t in ["shape", "?"] {
        s.cmdline(t).unwrap();
    }
    assert!(s.log.iter().any(|l| l.starts_with("TICK")), "{:?}", s.log);
    for t in ["nothing", "tick", "1,1", "3", "0"] {
        s.cmdline(t).unwrap();
    }
    assert!(s.running.is_none());
    assert!(s.log.iter().any(|l| l.contains("Shape NOTHING is not in a loaded shape file")));
    let d = s.doc().unwrap();
    let sh = d.model.iter().find_map(|e| if let EntityKind::Shape(sh) = &e.kind { Some(sh.clone()) } else { None }).unwrap();
    assert_eq!((sh.name.as_str(), sh.size, sh.insert.xy()), ("TICK", 3.0, Vec2::new(1.0, 1.0)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compile_and_load_refuse_bad_files() {
    let dir = scratch("shapes-bad");
    let mut s = Session::new();
    let bad = dir.join("bad.shp");
    std::fs::write(&bad, "*1,2,A\n0x9,0\n").unwrap();
    let e = s.execute("compile", &json!({ "file": path(&bad) })).unwrap_err().to_string();
    assert!(e.contains("line 2"), "{e}");
    assert!(s.execute("compile", &json!({ "text": SHP })).is_err(), "text needs an output");
    let out = dir.join("from-text.shx");
    s.execute("compile", &json!({ "text": SHP, "output": path(&out) })).unwrap();
    assert!(out.exists());
    std::fs::write(dir.join("junk.shx"), b"not a shape file").unwrap();
    assert!(s.execute("load", &json!({ "file": path(&dir.join("junk.shx")) })).is_err());
    assert!(s.execute("load", &json!({ "file": path(&dir.join("absent.shx")) })).is_err());
    assert!(s.execute("load", &json!({})).is_err());
    assert!(s.doc().unwrap().shape_files.is_empty());
    for v in [
        json!({ "name": "" }),
        json!({ "name": "BOX" }),
        json!({ "name": "BOX", "at": [0, 0], "size": -1 }),
        json!({ "name": "BOX", "at": [0, 0], "oblique": 89 }),
    ] {
        assert!(s.execute("shape", &v).is_err(), "{v}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mkshape_makes_a_shape_that_matches_the_objects() {
    let dir = scratch("mkshape");
    let mut s = Session::new();
    // A 10 × 5 rectangle and a line across it.
    s.execute("rectang", &json!({ "p1": [0, 0], "p2": [10, 5] })).unwrap();
    let rect = last(&s);
    s.execute("line", &json!({ "points": [[0, 0], [10, 5]] })).unwrap();
    let line = last(&s);
    let file = dir.join("mine.shp");
    let r =
        s.execute("mkshape", &json!({ "name": "PANEL", "file": path(&file), "handles": [rect, line], "base": [0, 0], "resolution": 100 })).unwrap();
    assert_eq!(r["number"], 1);
    let size = r["size"].as_f64().unwrap();
    assert!((size - 0.1).abs() < 1e-12, "{size}");
    assert!(file.exists() && dir.join("mine.shx").exists());
    // Loaded: SHAPE at that size covers the objects again.
    let r = s.execute("shape", &json!({ "name": "panel", "at": [100, 0], "size": size })).unwrap();
    let h = Handle::parse_hex(r["handle"].as_str().unwrap()).unwrap();
    let d = s.doc().unwrap();
    let EntityKind::Shape(sh) = &d.entity(h).unwrap().kind else { panic!() };
    let st = cadaddy_engine::render::shapes::strokes(d, sh);
    let b = cadaddy_engine::geom::Bounds2::from_points(st.iter().flatten().copied());
    assert!(near(b.min, Vec2::new(100.0, 0.0)) && near(b.max, Vec2::new(110.0, 5.0)), "{b:?}");
    // A second shape goes into the same file; making PANEL again replaces it.
    s.execute("circle", &json!({ "center": [0, 0], "radius": 3 })).unwrap();
    let c = last(&s);
    let r = s.execute("mkshape", &json!({ "name": "RING", "file": path(&file), "handles": [c.clone()], "base": [0, 0] })).unwrap();
    assert_eq!(r["number"], 2);
    let r = s.execute("mkshape", &json!({ "name": "panel", "file": path(&file), "handles": [c], "base": [0, 0], "resolution": 20 })).unwrap();
    assert_eq!(r["number"], 1);
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.matches("*1,").count() + text.matches("*2,").count(), 2, "{text}");
    assert_eq!(s.doc().unwrap().shape_files.len(), 1);
    for v in [
        json!({ "name": "A B", "file": path(&file), "base": [0, 0] }),
        json!({ "name": "X", "file": path(&file), "base": [0, 0], "resolution": 0, "handles": [] }),
    ] {
        assert!(s.execute("mkshape", &v).is_err(), "{v}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- MKLTYPE ----------

#[test]
fn mkltype_writes_and_loads_a_linetype() {
    let dir = scratch("mkltype");
    let file = dir.join("mine.lin");
    std::fs::write(&file, "*OTHER,kept\nA,1,-1\n*FENCE,old one\nA,9,-9\n").unwrap();
    let mut s = Session::new();
    // Along 0..10: a dash 0..4, a dot at 6, text "X" at 8 (0.5 above the line).
    let a = {
        s.execute("line", &json!({ "points": [[0, 0], [4, 0]] })).unwrap();
        last(&s)
    };
    s.execute("point", &json!({ "at": [6, 0] })).unwrap();
    let p = last(&s);
    s.execute("text", &json!({ "text": "X", "at": [8, 0.5], "height": 1 })).unwrap();
    let t = s.doc().unwrap().model.iter().last().unwrap().handle.hex();
    let r = s
        .execute(
            "mkltype",
            &json!({ "file": path(&file), "name": "FENCE", "description": "Made-up fence", "start": [0, 0], "end": [10, 0], "handles": [a, p, t] }),
        )
        .unwrap();
    let pat: Vec<f64> = r["pattern"].as_array().unwrap().iter().map(|e| e["length"].as_f64().unwrap()).collect();
    assert_eq!(pat, [4.0, -2.0, 0.0, -2.0, -2.0], "{r}");
    let text_el = &r["pattern"][3];
    assert_eq!(text_el["text"], "X");
    assert!((text_el["offset"][1].as_f64().unwrap() - 0.5).abs() < 1e-9);
    // Loaded into the drawing.
    let lt = s.doc().unwrap().linetype("FENCE").unwrap().clone();
    assert_eq!(lt.description, "Made-up fence");
    assert_eq!(lt.pattern.len(), 5);
    // The file: the old FENCE replaced, OTHER kept, and LINETYPE reads it back the same.
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("*OTHER,kept") && !text.contains("old one"), "{text}");
    let mut t2 = Session::new();
    t2.execute("linetype", &json!({ "file": path(&file), "load": "FENCE" })).unwrap();
    let back = t2.doc().unwrap().linetype("FENCE").unwrap().clone();
    let lens: Vec<f64> = back.pattern.iter().map(|e| e.length).collect();
    assert_eq!(lens, [4.0, -2.0, 0.0, -2.0, -2.0]);
    assert_eq!(back.pattern[3].text.as_deref(), Some("X"));
    for v in [
        json!({ "file": path(&file), "name": "Continuous", "start": [0, 0], "end": [1, 0] }),
        json!({ "file": path(&file), "name": "A", "start": [0, 0], "end": [0, 0] }),
        json!({ "name": "A", "start": [0, 0], "end": [1, 0] }),
    ] {
        assert!(s.execute("mkltype", &v).is_err(), "{v}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mkltype_carries_shapes() {
    let dir = scratch("mkltype-shape");
    std::fs::write(dir.join("ticks.shp"), SHP).unwrap();
    let mut s = Session::new();
    s.execute("compile", &json!({ "file": path(&dir.join("ticks.shp")) })).unwrap();
    s.execute("load", &json!({ "file": path(&dir.join("ticks.shx")) })).unwrap();
    let a = {
        s.execute("line", &json!({ "points": [[0, 0], [2, 0]] })).unwrap();
        last(&s)
    };
    let b = s.execute("shape", &json!({ "name": "BOX", "at": [3, 0], "size": 0.5 })).unwrap()["handle"].as_str().unwrap().to_string();
    let file = dir.join("ticks.lin");
    let r = s.execute("mkltype", &json!({ "file": path(&file), "name": "BOXED", "start": [0, 0], "end": [4, 0], "handles": [a, b] })).unwrap();
    assert_eq!(r["pattern"][1]["shape"], 1, "{r}");
    assert!(std::fs::read_to_string(&file).unwrap().contains("[BOX,ticks.shx,S=0.5"));
    // Drawn along a line with that linetype: the boxes appear.
    s.execute("line", &json!({ "points": [[0, 10], [8, 10]] })).unwrap();
    let h = s.doc().unwrap().model.iter().last().unwrap().handle;
    s.doc_mut().unwrap().modify_entity(h, |e| e.common.linetype = "BOXED".into()).unwrap();
    let d = s.doc().unwrap();
    let list = cadaddy_engine::render::build(d, &cadaddy_engine::doc::Space::Model, &Default::default());
    let tall = list.prims.iter().filter(|p| p.handle == h).flat_map(|p| list.points(p)).any(|q| q.y > 10.4);
    assert!(tall, "the box shapes stand above the line");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- ATTEXT ----------

/// Two references of a made-up block with ROOM and NOTE attributes, one without attributes.
fn tagged() -> Session {
    let mut s = Session::new();
    s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    let c = s.doc().unwrap().model.iter().last().unwrap().handle;
    s.execute("attdef", &json!({ "tag": "ROOM", "default": "0", "at": [0, -2], "height": 1 })).unwrap();
    let a1 = s.doc().unwrap().model.iter().last().unwrap().handle;
    s.execute("attdef", &json!({ "tag": "NOTE", "default": "", "at": [0, -4], "height": 1 })).unwrap();
    let a2 = s.doc().unwrap().model.iter().last().unwrap().handle;
    s.execute("block", &json!({ "name": "DOOR", "base": [0, 0], "handles": [c.hex(), a1.hex(), a2.hex()], "keep": "delete" })).unwrap();
    s.execute("insert", &json!({ "name": "DOOR", "at": [10, 20], "attribs": { "ROOM": "101", "NOTE": "Sample Floor Plan" } })).unwrap();
    s.execute("insert", &json!({ "name": "DOOR", "at": [30.5, 40], "rotation": 90, "attribs": { "ROOM": "102", "NOTE": "Project A" } })).unwrap();
    s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    s.execute(
        "block",
        &json!({ "name": "PLAIN", "base": [0, 0], "handles": [s.doc().unwrap().model.iter().last().unwrap().handle.hex()], "keep": "delete" }),
    )
    .unwrap();
    s.execute("insert", &json!({ "name": "PLAIN", "at": [0, 0] })).unwrap();
    s
}

const TEMPLATE: &str = "BL:NAME C008000\nBL:X N008002\nBL:ORIENT N005000\nROOM N005000\nNOTE C020000\n";

#[test]
fn attext_cdf_sdf_and_dxf() {
    let dir = scratch("attext");
    std::fs::write(dir.join("tmpl.txt"), TEMPLATE).unwrap();
    let mut s = tagged();
    let r = s.execute("attext", &json!({ "template": path(&dir.join("tmpl.txt")) })).unwrap();
    assert_eq!(r["records"], 2);
    assert_eq!(r["text"], "'DOOR',10.00,0,101,'Sample Floor Plan'\n'DOOR',30.50,90,102,'Project A'\n");
    // SDF: fixed columns.
    let r = s.execute("attext", &json!({ "format": "sdf", "templateText": TEMPLATE })).unwrap();
    let first = r["text"].as_str().unwrap().lines().next().unwrap().to_string();
    assert_eq!(first, "DOOR       10.00    0  101Sample Floor Plan");
    // Quote and delimiter from the template; written to a file.
    let out = dir.join("out.txt");
    s.execute("attext", &json!({ "templateText": format!("C:QUOTE \"\nC:DELIM ;\n{TEMPLATE}"), "file": path(&out) })).unwrap();
    assert!(std::fs::read_to_string(&out).unwrap().starts_with("\"DOOR\";10.00;0;101;\"Sample Floor Plan\"\n"));
    // DXF: the references with attributes, readable as a DXF file.
    let dxx = dir.join("out.dxx");
    let r = s.execute("attext", &json!({ "format": "dxf", "file": path(&dxx) })).unwrap();
    assert_eq!(r["records"], 2);
    let text = std::fs::read_to_string(&dxx).unwrap();
    assert_eq!(text.matches("\nATTRIB\n").count(), 4);
    assert!(text.trim_end().ends_with("EOF"));
    // A numeric field whose value isn't a number is written as 0, with a note.
    let doors: Vec<String> =
        s.doc().unwrap().model.iter().filter(|e| matches!(&e.kind, EntityKind::Insert(i) if i.block == "DOOR")).map(|e| e.handle.hex()).collect();
    let r = s.execute("attext", &json!({ "templateText": "NOTE N004000\n", "handles": [doors[0]] })).unwrap();
    assert_eq!(r["text"], "0\n");
    assert_eq!(r["notes"].as_array().unwrap().len(), 1);
    for bad in [json!({}), json!({ "templateText": "BL:NAME N008000" }), json!({ "format": "xml", "templateText": TEMPLATE })] {
        assert!(s.execute("attext", &bad).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn attext_typed_and_nested() {
    let dir = scratch("attext-typed");
    std::fs::write(dir.join("t.txt"), "BL:LEVEL N002000\nBL:NAME C008000\nROOM C005000\n").unwrap();
    let mut s = tagged();
    // A block holding a DOOR: its door is extracted at level 2, where the outer reference puts it.
    let door = s.doc().unwrap().model.iter().find(|e| matches!(&e.kind, EntityKind::Insert(i) if i.block == "DOOR")).unwrap().handle;
    s.execute("block", &json!({ "name": "SUITE", "base": [0, 0], "handles": [door.hex()], "keep": "delete" })).unwrap();
    s.execute("insert", &json!({ "name": "SUITE", "at": [100, 0] })).unwrap();
    for t in ["attext", "c", &path(&dir.join("t.txt")), &path(&dir.join("x"))] {
        s.cmdline(t).unwrap();
    }
    assert!(s.running.is_none());
    let text = std::fs::read_to_string(dir.join("x.txt")).unwrap();
    assert!(text.contains(" 2,'DOOR','101'") || text.contains("2,'DOOR','101'"), "{text}");
    assert!(text.contains("1,'DOOR','102'"), "{text}");
    assert!(s.log.iter().any(|l| l.contains("2 record(s) in extract file.")), "{:?}", s.log);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- COPYM ----------

fn circle_centres(s: &Session) -> Vec<Vec2> {
    let mut v: Vec<Vec2> =
        s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Circle(c) = &e.kind { Some(c.center.xy()) } else { None }).collect();
    v.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    v
}

#[test]
fn copym_modes() {
    let mut s = Session::new();
    let c = {
        s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
        last(&s)
    };
    let r = s.execute("copym", &json!({ "handles": [c], "base": [0, 0], "to": [[5, 0], [0, 5]] })).unwrap();
    assert_eq!(r["copies"], 2);
    let r = s.execute("copym", &json!({ "handles": [c], "base": [0, 0], "mode": "repeat", "to": [0, -2], "count": 3 })).unwrap();
    assert_eq!(r["copies"], 3);
    let r = s.execute("copym", &json!({ "handles": [c], "base": [0, 0], "mode": "divide", "to": [-12, 0], "count": 4 })).unwrap();
    assert_eq!(r["copies"], 4);
    let r = s.execute("copym", &json!({ "handles": [c], "base": [0, 0], "mode": "measure", "to": [0, 25], "spacing": 10 })).unwrap();
    assert_eq!(r["copies"], 2);
    // Spacing 100 × 100 up to (300, 150): 4 columns, 2 rows, less the original.
    let r = s.execute("copym", &json!({ "handles": [c], "base": [0, 0], "mode": "array", "to": [100, 100], "corner": [300, 150] })).unwrap();
    assert_eq!(r["copies"], 7);
    let r = s.execute("copym", &json!({ "handles": [c], "base": [0, 0], "mode": "array", "to": [-50, -50], "rows": 2, "columns": 2 })).unwrap();
    assert_eq!(r["copies"], 3);
    let got = circle_centres(&s);
    for want in [
        (5.0, 0.0),
        (0.0, 5.0),
        (0.0, -2.0),
        (0.0, -4.0),
        (0.0, -6.0),
        (-3.0, 0.0),
        (-12.0, 0.0),
        (0.0, 10.0),
        (0.0, 20.0),
        (300.0, 100.0),
        (200.0, 0.0),
        (-50.0, -50.0),
    ] {
        assert!(got.iter().any(|g| near(*g, Vec2::new(want.0, want.1))), "{want:?} in {got:?}");
    }
    assert_eq!(got.len(), 1 + 2 + 3 + 4 + 2 + 7 + 3);
    // One undo step takes a whole COPYM back.
    s.execute("u", &json!({})).unwrap();
    assert_eq!(circle_centres(&s).len(), 1 + 2 + 3 + 4 + 2 + 7);
    for bad in [
        json!({ "handles": [c], "base": [0, 0] }),
        json!({ "handles": [c], "base": [0, 0], "mode": "divide", "to": [1, 0], "count": 0 }),
        json!({ "handles": [c], "base": [0, 0], "mode": "measure", "to": [1e9, 0], "spacing": 1e-3 }),
        json!({ "handles": [c], "base": [0, 0], "mode": "array", "to": [1, 1], "rows": 1000, "columns": 1000 }),
        json!({ "handles": [c], "base": [0, 0], "mode": "sideways", "to": [1, 0] }),
        json!({ "handles": [], "base": [0, 0], "to": [1, 0] }),
    ] {
        assert!(s.execute("copym", &bad).is_err(), "{bad}");
    }
}

#[test]
fn copym_typed() {
    let mut s = Session::new();
    s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    for t in ["copym", "0,1", "", "0,0", "10,0", "r", "r", "d", "0,10", "2", "m", "-10,0", "4", "a", "0,-5", "10,-10", ""] {
        s.cmdline(t).unwrap();
    }
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let got = circle_centres(&s);
    for want in [(10.0, 0.0), (20.0, 0.0), (30.0, 0.0), (0.0, 5.0), (0.0, 10.0), (-4.0, 0.0), (-8.0, 0.0), (0.0, -5.0), (0.0, -10.0)] {
        assert!(got.iter().any(|g| near(*g, Vec2::new(want.0, want.1))), "{want:?} in {got:?}");
    }
    // Array: spacing (0,-5) has no X step, so one column of 3 rows (0, -5, -10): 2 copies.
    assert_eq!(got.len(), 1 + 1 + 2 + 2 + 2 + 2, "{got:?}");
}
