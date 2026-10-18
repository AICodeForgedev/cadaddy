//! PDF underlays: PDFATTACH (and ATTACH of a .pdf) places a page drawn from the PDF, not converted;
//! PDFLAYERS, PDFCLIP and PDFADJUST change how it shows; PDFOSNAP snaps to its geometry.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::geom::{Vec2, Vec3};
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Handle, Layer, Line, Space};
use cadaddy_engine::snap::{self, mode};
use cadaddy_render::{Kind, Options, build};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
    });
}

/// A PDF of two lines on two layers (WALLS red, NOTES green) and a circle, written by CADaddy.
fn pdf(dir: &std::path::Path) -> String {
    let mut d = Drawing::new_imperial();
    let mut walls = Layer::new("WALLS");
    walls.color = cadaddy_engine::doc::color::Color::Index(1);
    let mut notes = Layer::new("NOTES");
    notes.color = cadaddy_engine::doc::color::Color::Index(3);
    d.layers.extend([walls, notes]);
    let line = |a: (f64, f64), b: (f64, f64)| EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) });
    let on = |l: &str| Common { layer: l.into(), ..Common::default() };
    d.add(&Space::Model, on("WALLS"), line((0.0, 0.0), (10.0, 0.0))).unwrap();
    d.add(&Space::Model, on("NOTES"), line((2.0, 3.0), (7.0, 6.0))).unwrap();
    d.add(&Space::Model, on("WALLS"), EntityKind::Circle(cadaddy_engine::doc::Circle { center: Vec3::new(5.0, 3.0, 0.0), radius: 1.0 })).unwrap();
    let path = dir.join("plan.pdf");
    std::fs::write(&path, cadaddy_io::write(&d, "plan.pdf").unwrap()).unwrap();
    path.to_string_lossy().to_string()
}

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-underlay-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn h(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

fn underlay(s: &Session, hd: Handle) -> cadaddy_engine::doc::Underlay {
    match &s.doc().unwrap().entity(hd).unwrap().kind {
        EntityKind::Underlay(u) => u.clone(),
        k => panic!("not an underlay: {k:?}"),
    }
}

fn prims(s: &Session) -> Vec<cadaddy_render::DPrim> {
    build(s.doc().unwrap(), &Space::Model, &Options { transparency: true, ..Options::default() }).prims
}

#[test]
fn attach_shows_the_page_without_converting_it() {
    hooks();
    let dir = dir("attach");
    let path = pdf(&dir);
    let mut s = Session::new();
    let r = s.execute("pdfattach", &json!({ "path": path, "at": [100, 50], "scale": 2 })).unwrap();
    let u = underlay(&s, h(&r));
    assert_eq!((u.path.as_str(), u.page), (path.as_str(), 1));
    assert!(r["size"][0].as_f64().unwrap() > 0.0 && r["size"][1].as_f64().unwrap() > 0.0, "{r}");
    let layers: Vec<String> = serde_json::from_value(r["layers"].clone()).unwrap();
    assert!(layers.iter().any(|l| l == "WALLS") && layers.iter().any(|l| l == "NOTES"), "{layers:?}");
    // One object in the drawing; its extents are the placed page.
    assert_eq!(s.doc().unwrap().model.iter().count(), 1);
    let ext = s.doc().unwrap().extents(&Space::Model);
    assert!((ext.min.x - 100.0).abs() < 1e-9 && (ext.width() - 2.0 * u.size.x).abs() < 1e-6, "{ext:?}");
    // Drawn from the PDF: more than the frame, with the default fade.
    let p = prims(&s);
    assert!(p.len() > 2, "{}", p.len());
    assert!(p.iter().all(|q| q.transparency >= 25));

    // Errors: no file, a page past the end, a bad scale.
    assert!(s.execute("pdfattach", &json!({ "path": dir.join("none.pdf").to_string_lossy() })).is_err());
    assert!(s.execute("pdfattach", &json!({ "path": path, "page": 9 })).is_err());
    assert!(s.execute("pdfattach", &json!({ "path": path, "scale": -1 })).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn layers_clip_and_adjust_change_what_shows() {
    hooks();
    let dir = dir("adjust");
    let path = pdf(&dir);
    let mut s = Session::new();
    let u = h(&s.execute("pdfattach", &json!({ "path": path })).unwrap());
    let all = prims(&s).len();

    // A layer turned off leaves its objects out; turned on again they're back.
    let r = s.execute("pdflayers", &json!({ "handle": u.hex(), "off": ["NOTES"] })).unwrap();
    assert!(r["layers"].as_array().unwrap().iter().any(|l| l["name"] == "NOTES" && l["on"] == false), "{r}");
    assert!(prims(&s).len() < all);
    s.execute("pdflayers", &json!({ "handle": u.hex(), "on": "NOTES" })).unwrap();
    assert_eq!(prims(&s).len(), all);

    // Monochrome draws everything in colour 7; fade sets the transparency.
    let r = s.execute("pdfadjust", &json!({ "handles": [u.hex()], "monochrome": true, "fade": 60, "contrast": 50 })).unwrap();
    assert_eq!(r, json!({ "fade": 60, "contrast": 50, "monochrome": true }));
    assert!(prims(&s).iter().all(|q| q.aci7 && q.transparency >= 60));
    assert!(s.execute("pdfadjust", &json!({ "handles": [u.hex()], "fade": 90 })).is_err());
    assert!(s.execute("pdfadjust", &json!({ "handles": [u.hex()], "contrast": 10 })).is_err());

    // A clip boundary keeps only what is inside it; off and delete bring the page back.
    let size = underlay(&s, u).size;
    let r = s.execute("pdfclip", &json!({ "handle": u.hex(), "rect": [[0, 0], [size.x / 4.0, size.y / 4.0]] })).unwrap();
    assert_eq!(r["clipping"], true);
    let clipped: f64 = prims(&s).iter().filter(|q| q.kind == Kind::Polyline).map(|q| q.len as f64).sum();
    s.execute("pdfclip", &json!({ "handle": u.hex(), "off": true })).unwrap();
    let whole: f64 = prims(&s).iter().filter(|q| q.kind == Kind::Polyline).map(|q| q.len as f64).sum();
    assert!(clipped < whole, "{clipped} < {whole}");
    s.execute("pdfclip", &json!({ "handle": u.hex(), "delete": true })).unwrap();
    assert!(underlay(&s, u).clip.is_empty());
    assert!(s.execute("pdfclip", &json!({ "handle": u.hex(), "points": [[0, 0], [1, 1]] })).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn attach_by_file_type_typed_and_transformed() {
    hooks();
    let dir = dir("typed");
    let path = pdf(&dir);
    let mut s = Session::new();
    let r = s.execute("attach", &json!({ "path": path, "at": [5, 5] })).unwrap();
    assert!(matches!(s.doc().unwrap().entity(h(&r)).unwrap().kind, EntityKind::Underlay(_)));

    let mut s = Session::new();
    s.cmdline("PDFATTACH").unwrap();
    s.cmdline(&path).unwrap();
    assert_eq!(s.current_prompt().unwrap().display(), "Enter page number <1>:");
    s.script("\n10,20\n3\n90\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let hd = s.doc().unwrap().model.iter().next().unwrap().handle;
    let u = underlay(&s, hd);
    assert!((u.insert.x - 10.0).abs() < 1e-9 && (u.u.y - 3.0).abs() < 1e-9 && u.u.x.abs() < 1e-9, "{u:?}");

    // MOVE and SCALE place it anew; the definition stays.
    s.execute("move", &json!({ "handles": [hd.hex()], "from": [0, 0], "to": [1, 1] })).unwrap();
    s.execute("scale", &json!({ "handles": [hd.hex()], "base": [11, 21], "factor": 2 })).unwrap();
    let u2 = underlay(&s, hd);
    assert!((u2.insert.x - 11.0).abs() < 1e-9 && (u2.u.y - 6.0).abs() < 1e-9 && u2.path == path, "{u2:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn snapping_to_the_page_follows_pdfosnap() {
    hooks();
    let dir = dir("snap");
    let path = pdf(&dir);
    // Where the page's NOTES line starts, in sheet inches, read as the underlay reads the page.
    let (page, _) = cadaddy_io::pdf_read::read_json(&std::fs::read(&path).unwrap(), &json!({ "page": 1, "scale": 1.0, "join": false })).unwrap();
    let notes = page
        .model
        .iter()
        .filter(|e| e.common.layer == "NOTES")
        .find_map(|e| match &e.kind {
            EntityKind::Line(l) => Some(l.a.xy()),
            EntityKind::LwPolyline(p) => p.vertices.first().map(|v| v.p),
            _ => None,
        })
        .expect("the NOTES line");
    let mut s = Session::new();
    s.execute("pdfattach", &json!({ "path": path, "at": [100, 0], "scale": 2 })).unwrap();
    let want = Vec2::new(100.0, 0.0) + notes * 2.0;
    let d = s.doc().unwrap().clone();
    let hit = snap::osnap(&d, &Space::Model, want + Vec2::new(0.01, 0.01), 0.1, mode::END, None, false).expect("a snap on the page");
    assert!(hit.point.dist(want) < 1e-6, "{:?} vs {want:?}", hit.point);

    s.execute("setvar", &json!({ "name": "PDFOSNAP", "value": 0 })).unwrap_or_else(|_| {
        s.doc_mut().unwrap().header.set_i64("PDFOSNAP", 0);
        Value::Null
    });
    let d = s.doc().unwrap().clone();
    assert!(snap::osnap(&d, &Space::Model, want + Vec2::new(0.01, 0.01), 0.1, mode::END, None, false).is_none_or(|h| h.point.dist(want) > 1e-6));
    std::fs::remove_dir_all(&dir).ok();
}
