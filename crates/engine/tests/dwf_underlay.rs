//! DWF and DWFx through the commands: DWFXOUT/DWFOUT write sheets at true scale that DWFATTACH
//! puts back on the source's coordinates (snaps land on the source's endpoints); the sheet's
//! layers come back as underlay layers; an attached DWF survives DXF, DWG and .cdy; PUBLISH and
//! the plot devices write DWFx/DWF with the layouts' names.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::geom::PolyVertex;
use cadaddy_engine::doc::geom::{Vec2, Vec3};
use cadaddy_engine::doc::{Arc, Common, Drawing, EntityKind, Handle, Layer, Layout, Line, LwPolyline, Space};
use cadaddy_engine::snap::{self, mode};
use cadaddy_render::{Kind, Options, build};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: Some(cadaddy_io::read_dwf_page),
        write_dwfx: Some(cadaddy_io::write_dwfx_sheets),
    });
}

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-dwf-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn p(path: &std::path::Path) -> String {
    path.to_string_lossy().to_string()
}

fn line(a: (f64, f64), b: (f64, f64)) -> EntityKind {
    EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) })
}

fn on(layer: &str) -> Common {
    Common { layer: layer.into(), ..Common::default() }
}

/// Lines, an arc and a closed polyline in `space`; their end points.
fn sheet_geometry(s: &mut Session, space: &Space) -> Vec<Vec2> {
    let d = s.doc_mut().unwrap();
    d.add(space, on("0"), line((20.0, 20.0), (180.0, 20.0))).unwrap();
    d.add(space, on("0"), line((20.0, 20.0), (60.0, 110.0))).unwrap();
    d.add(space, on("0"), EntityKind::Arc(Arc { center: Vec3::new(120.0, 80.0, 0.0), radius: 30.0, start: 0.0, end: std::f64::consts::FRAC_PI_2 }))
        .unwrap();
    let pts = [(140.0, 30.0), (170.0, 30.0), (170.0, 55.0), (140.0, 55.0)];
    let vertices = pts.iter().map(|&(x, y)| PolyVertex { p: Vec2::new(x, y), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
    d.add(space, on("0"), EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false })).unwrap();
    let mut ends = vec![Vec2::new(20.0, 20.0), Vec2::new(180.0, 20.0), Vec2::new(60.0, 110.0), Vec2::new(150.0, 80.0), Vec2::new(120.0, 110.0)];
    ends.extend(pts.iter().map(|&(x, y)| Vec2::new(x, y)));
    ends
}

/// Every point snaps (END) to itself within `tol`, in model space of `s`.
fn snaps_to(s: &Session, points: &[Vec2], tol: f64) {
    let d = s.doc().unwrap().clone();
    for want in points {
        let hit = snap::osnap(&d, &Space::Model, *want + Vec2::new(tol / 4.0, tol / 4.0), tol * 10.0, mode::END, None, false)
            .unwrap_or_else(|| panic!("no snap near {want:?}"));
        assert!(hit.point.dist(*want) < tol, "{:?} vs {want:?}", hit.point);
    }
}

fn h(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

#[test]
fn a_layout_exported_as_dwfx_attaches_back_on_its_own_coordinates() {
    hooks();
    let dir = dir("layout");
    let mut src = Session::new();
    *src.doc_mut().unwrap() = Drawing::new_metric();
    let layout = src.doc().unwrap().layouts[0].name.clone();
    let ends = sheet_geometry(&mut src, &Space::Paper(layout.clone()));
    let file = dir.join("Sample Sheet.dwfx");
    let r = src.execute("dwfxout", &json!({ "path": p(&file), "layouts": [layout] })).unwrap();
    assert_eq!((r["format"].as_str(), r["pages"].as_u64()), (Some("dwfx"), Some(1)));

    // Into another drawing, through the command, where the sheet was drawn from.
    let mut s = Session::new();
    let r = s.execute("dwfattach", &json!({ "path": p(&file), "trueScale": true })).unwrap();
    assert_eq!(r["sheet"].as_str(), Some(layout.as_str()), "{r}");
    let unit = cadaddy_render::paper::page_unit_mm(src.doc().unwrap(), &src.doc().unwrap().layout(&layout).unwrap().page);
    assert_eq!(r["scale"].as_f64(), Some(25.4 / unit), "{r}");
    snaps_to(&s, &ends, 1e-3);

    // Not at true scale: at the origin, a sheet inch per drawing unit, the same points scaled.
    let mut s = Session::new();
    let r = s.execute("dwfattach", &json!({ "path": p(&file) })).unwrap();
    let origin = Vec2::new(r["origin"][0].as_f64().unwrap(), r["origin"][1].as_f64().unwrap());
    let k = r["scale"].as_f64().unwrap();
    let inch: Vec<Vec2> = ends.iter().map(|q| (*q - origin) / k).collect();
    snaps_to(&s, &inch, 1e-3 / k);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn model_space_and_dwf6_attach_back_on_their_own_coordinates() {
    hooks();
    let dir = dir("model");
    let mut src = Session::new();
    let ends = sheet_geometry(&mut src, &Space::Model);
    for (cmd, ext) in [("dwfxout", "dwfx"), ("dwfout", "dwf")] {
        let file = dir.join(format!("Sample Plan.{ext}"));
        let r = src.execute(cmd, &json!({ "path": p(&file), "scale": 4, "layouts": ["Model"] })).unwrap();
        assert_eq!((r["format"].as_str(), r["pages"].as_u64()), (Some(ext), Some(1)), "{r}");
        let mut s = Session::new();
        let r = s.execute("dwfattach", &json!({ "path": p(&file), "trueScale": true })).unwrap();
        assert_eq!((r["scale"].as_f64(), r["sheet"].as_str()), (Some(4.0), Some("Model")), "{cmd}: {r}");
        snaps_to(&s, &ends, 1e-3);
    }
    // DWFOUT writes the sheets `layouts` names, one section each.
    let layout = src.doc().unwrap().layouts[0].name.clone();
    let r = src.execute("dwfout", &json!({ "layouts": ["Model", layout] })).unwrap();
    assert_eq!(r["pages"], 2);
    assert!(src.execute("dwfout", &json!({ "layouts": ["No Such Sheet"] })).is_err());
    assert!(src.execute("dwfxout", &json!({ "scale": -1 })).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_dwf_using_binary_opcodes_is_refused_with_a_reason() {
    hooks();
    let dir = dir("binary");
    let file = dir.join("Other.dwf");
    std::fs::write(&file, b"(DWF V06.00)\x10\x03\x00\x00\x00\x00").unwrap();
    let mut s = Session::new();
    let e = s.execute("dwfattach", &json!({ "path": p(&file) })).unwrap_err().to_string();
    assert!(e.contains("unsupported W2D opcode 0x10") && e.contains("save the sheet set as DWFx and attach that"), "{e}");
    assert_eq!(s.doc().unwrap().model.iter().count(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

/// Model space lines on WALLS and E-POWER, written as DWFx; the WALLS and E-POWER end points.
fn two_layer_file(dir: &std::path::Path) -> (String, Vec<Vec2>, Vec<Vec2>) {
    let mut src = Session::new();
    let d = src.doc_mut().unwrap();
    d.layers.extend([Layer::new("WALLS"), Layer::new("E-POWER")]);
    d.add(&Space::Model, on("WALLS"), line((0.0, 0.0), (10.0, 0.0))).unwrap();
    d.add(&Space::Model, on("E-POWER"), line((2.0, 3.0), (7.0, 6.0))).unwrap();
    let file = p(&dir.join("Two Layers.dwfx"));
    src.execute("dwfxout", &json!({ "path": file, "layouts": ["Model"] })).unwrap();
    (file, vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)], vec![Vec2::new(2.0, 3.0), Vec2::new(7.0, 6.0)])
}

/// The polylines drawn in model space: their points.
fn drawn(s: &Session) -> Vec<Vec<Vec2>> {
    let list = build(s.doc().unwrap(), &Space::Model, &Options::default());
    list.prims.iter().filter(|q| q.kind == Kind::Polyline).map(|q| list.points(q).to_vec()).collect()
}

fn near_all(lines: &[Vec<Vec2>], pts: &[Vec2]) -> bool {
    pts.iter().all(|q| lines.iter().any(|l| l.iter().any(|v| v.dist(*q) < 1e-6)))
}

#[test]
fn the_sheets_layers_are_the_underlays_layers() {
    hooks();
    let dir = dir("layers");
    let (file, walls, power) = two_layer_file(&dir);
    let mut s = Session::new();
    let r = s.execute("dwfattach", &json!({ "path": file, "trueScale": true })).unwrap();
    assert_eq!(r["layers"], json!(["WALLS", "E-POWER"]), "{r}");
    let u = h(&r);
    let all = drawn(&s);
    assert_eq!(all.len(), 2);
    assert!(near_all(&all, &walls) && near_all(&all, &power));
    // E-POWER off: only the WALLS line is drawn; on again, both.
    let r = s.execute("pdflayers", &json!({ "handle": u.hex(), "off": ["E-POWER"] })).unwrap();
    assert_eq!(r["layers"], json!([{ "name": "WALLS", "on": true }, { "name": "E-POWER", "on": false }]));
    let some = drawn(&s);
    assert_eq!(some.len(), 1);
    assert!(near_all(&some, &walls) && !near_all(&some, &power));
    s.execute("pdflayers", &json!({ "handle": u.hex(), "on": ["E-POWER"] })).unwrap();
    assert_eq!(drawn(&s).len(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_attached_dwf_survives_dxf_dwg_and_cdy() {
    hooks();
    let dir = dir("save");
    let (file, walls, power) = two_layer_file(&dir);
    let mut s = Session::new();
    s.execute("dwfattach", &json!({ "path": file, "trueScale": true })).unwrap();
    for ext in ["dxf", "dwg", "cdy"] {
        let host = dir.join(format!("host.{ext}"));
        s.execute("saveas", &json!({ "path": p(&host) })).unwrap();
        // Nothing read so far is kept: the page is read again from the DWFx.
        cadaddy_engine::cmd::forget_underlay_pages();
        let mut t = Session::empty();
        t.execute("open", &json!({ "path": p(&host) })).unwrap();
        let u = t
            .doc()
            .unwrap()
            .model
            .iter()
            .find_map(|e| if let EntityKind::Underlay(u) = &e.kind { Some(u.as_ref().clone()) } else { None })
            .unwrap_or_else(|| panic!("{ext}: no underlay"));
        assert!(u.is_dwf() && u.path == file && u.page == 1, "{ext}: {u:?}");
        let d = t.doc().unwrap();
        assert_eq!(d.model.iter().next().map(|e| e.kind.dxf_name()), Some("DWFUNDERLAY"), "{ext}");
        let lines = drawn(&t);
        assert_eq!(lines.len(), 2, "{ext}: the page's two lines");
        assert!(near_all(&lines, &walls) && near_all(&lines, &power), "{ext}");
        snaps_to(&t, &[walls.clone(), power.clone()].concat(), 1e-3);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn publish_to_dwfx_gives_one_named_page_per_layout() {
    hooks();
    let dir = dir("publish");
    let mut s = Session::new();
    {
        let d = s.doc_mut().unwrap();
        d.layouts.clear();
        d.layouts.push(Layout::new("Ground Floor", 1));
        d.layouts.push(Layout::new("Roof Plan", 2));
        d.add(&Space::Paper("Ground Floor".into()), on("0"), line((1.0, 1.0), (5.0, 1.0))).unwrap();
        d.add(&Space::Paper("Roof Plan".into()), on("0"), line((2.0, 2.0), (2.0, 6.0))).unwrap();
    }
    let file = dir.join("Sample Set.dwfx");
    let r = s.execute("publish", &json!({ "path": p(&file), "layouts": ["Ground Floor", "Roof Plan"] })).unwrap();
    assert_eq!((r["format"].as_str(), r["pages"].as_u64()), (Some("dwfx"), Some(2)), "{r}");
    let bytes = std::fs::read(&file).unwrap();
    let names: Vec<String> = (1..=2)
        .map(|k| {
            let page = cadaddy_io::dwfx::read_page(&bytes, k).unwrap();
            assert_eq!(page.pages, 2);
            page.name.unwrap()
        })
        .collect();
    assert_eq!(names, ["Ground Floor", "Roof Plan"]);
    assert!(cadaddy_io::dwfx::read_page(&bytes, 3).is_err());
    // Each page through DWFATTACH names its sheet; `format: dwf` writes DWF 6 sections.
    let mut t = Session::new();
    assert_eq!(t.execute("dwfattach", &json!({ "path": p(&file), "page": 2 })).unwrap()["sheet"], "Roof Plan");
    let r = s.execute("publish", &json!({ "format": "dwf", "layouts": ["Ground Floor", "Roof Plan"] })).unwrap();
    assert_eq!((r["format"].as_str(), r["pages"].as_u64()), (Some("dwf"), Some(2)));
    let dwf = cadaddy_engine::cmd::file::base64_decode(r["data"].as_str().unwrap()).unwrap();
    assert_eq!(cadaddy_io::dwfx::read_page(&dwf, 2).unwrap().name.as_deref(), Some("Roof Plan"));
    assert!(s.execute("publish", &json!({ "format": "svg" })).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_plot_devices_write_dwfx_and_dwf() {
    hooks();
    let dir = dir("plot");
    let names = cadaddy_engine::cmd::plotters::device_names();
    assert!(names.iter().any(|n| n == "CADaddy DWFx") && names.iter().any(|n| n == "CADaddy DWF"), "{names:?}");
    let mut s = Session::new();
    s.doc_mut().unwrap().add(&Space::Model, on("0"), line((0.0, 0.0), (10.0, 5.0))).unwrap();
    for (dev, ext) in [("CADaddy DWFx", "dwfx"), ("CADaddy DWF", "dwf")] {
        let file = dir.join(format!("plot.{ext}"));
        s.execute("plot", &json!({ "device": dev, "layout": "Model", "path": p(&file) })).unwrap_or_else(|e| panic!("{dev}: {e}"));
        let bytes = std::fs::read(&file).unwrap();
        assert_eq!(cadaddy_io::dwfx::is_dwf6(&bytes), ext == "dwf", "{dev}");
        let page = cadaddy_io::dwfx::read_page(&bytes, 1).unwrap();
        assert_eq!((page.pages, page.name.as_deref()), (1, Some("Model")), "{dev}");
        assert_eq!(page.drawing.model.iter().count(), 1, "{dev}");
    }
    std::fs::remove_dir_all(&dir).ok();
}
