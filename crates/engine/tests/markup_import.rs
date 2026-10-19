//! MARKUPIMPORT: a synthetic annotated PDF placed onto a layout's sheet and through a pair of
//! points, as revision clouds, multileaders, text and polylines on the markup layer, listed and
//! marked done.

use cadaddy_doc::markup::{Markup, MarkupKind};
use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::{Value, json};

fn m(kind: MarkupKind, points: Vec<[f64; 2]>, rect: [f64; 4], text: &str) -> Markup {
    Markup {
        kind,
        points,
        strokes: Vec::new(),
        rect,
        text: text.into(),
        author: "Reviewer".into(),
        subject: String::new(),
        color: None,
        date: String::new(),
    }
}

fn pdf(dir: &std::path::Path) -> String {
    let mut ink = m(MarkupKind::Ink, Vec::new(), [500.0, 500.0, 560.0, 540.0], "");
    ink.strokes = vec![vec![[500.0, 500.0], [520.0, 540.0], [560.0, 510.0]]];
    let markups = vec![
        m(MarkupKind::Cloud, vec![[72.0, 72.0], [144.0, 72.0], [144.0, 144.0], [72.0, 144.0]], [72.0, 72.0, 144.0, 144.0], "Move this wall"),
        m(MarkupKind::Callout, vec![[200.0, 150.0], [250.0, 300.0]], [250.0, 290.0, 400.0, 320.0], "Add outlet"),
        m(MarkupKind::Note, Vec::new(), [600.0, 600.0, 700.0, 640.0], "General note"),
        ink,
    ];
    let p = dir.join("review.pdf");
    std::fs::write(&p, cadaddy_io::pdf_markups::annotated_pdf(1224.0, 792.0, &markups).unwrap()).unwrap();
    p.to_string_lossy().into_owned()
}

fn hex(v: &Value) -> Handle {
    Handle::parse_hex(v.as_str().unwrap()).unwrap()
}

#[test]
fn markups_land_on_the_sheet_and_are_tracked() {
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    let dir = std::env::temp_dir().join(format!("cadaddy-markup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = pdf(&dir);
    let mut s = Session::new();
    // In a layout: the page is the sheet (inches here: 72 points = 1).
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    let r = s.execute("markupimport", &json!({ "path": path })).unwrap();
    let placed = r["placed"].as_array().unwrap().clone();
    assert_eq!(placed.len(), 4, "{r}");
    let d = s.doc().unwrap();
    assert!(d.layer("MARKUP").is_some_and(|l| !l.plot), "a markup layer that doesn't plot");
    let cloud = d.entity(hex(&placed[0]["handles"][0])).unwrap();
    let EntityKind::LwPolyline(pl) = &cloud.kind else { panic!("a revision cloud: {:?}", cloud.kind) };
    assert!(pl.closed && pl.vertices.iter().any(|v| v.bulge != 0.0), "bulged arcs");
    assert_eq!(cloud.common.layer, "MARKUP");
    let xs: Vec<f64> = pl.vertices.iter().map(|v| v.p.x).collect();
    let lo = xs.iter().copied().fold(f64::MAX, f64::min);
    let origin = d.layouts.iter().find(|l| l.name == "Layout1").unwrap().origin;
    assert!((lo - (origin.x + 1.0)).abs() < 1e-6, "72 points = 1 inch from the sheet's corner: {lo}");
    let leader = d.entity(hex(&placed[1]["handles"][0])).unwrap();
    let EntityKind::MLeader(ml) = &leader.kind else { panic!("a multileader") };
    assert!(ml.text.as_ref().unwrap().contents.contains("Add outlet"));
    assert!(matches!(d.entity(hex(&placed[2]["handles"][0])).unwrap().kind, EntityKind::MText(_)));
    assert!(matches!(d.entity(hex(&placed[3]["handles"][0])).unwrap().kind, EntityKind::LwPolyline(_)));

    // Listed, then marked done.
    let list = s.execute("markups", &json!({})).unwrap();
    let ids: Vec<&str> = list["markups"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["M1", "M2", "M3", "M4"]);
    assert_eq!(list["markups"][0]["text"], "Move this wall");
    assert_eq!(list["markups"][0]["author"], "Reviewer");
    s.execute("markupstatus", &json!({ "id": "M1", "status": "done" })).unwrap();
    assert_eq!(s.execute("markups", &json!({})).unwrap()["markups"][0]["status"], "done");
    assert!(s.execute("markupstatus", &json!({ "id": "M99", "status": "done" })).is_err());

    // In model space, through a pair of points: page (72,72)→(1000,0), (144,72)→(1100,0).
    s.execute("layout", &json!({ "option": "set", "name": "Model" })).unwrap();
    let r = s.execute("markupimport", &json!({ "path": path, "from": [[72, 72], [144, 72]], "to": [[1000, 0], [1100, 0]] })).unwrap();
    assert_eq!(r["placed"][0]["id"], "M5", "ids continue");
    let EntityKind::LwPolyline(pl) = &s.doc().unwrap().entity(hex(&r["placed"][0]["handles"][0])).unwrap().kind else { panic!() };
    let xs: Vec<f64> = pl.vertices.iter().map(|v| v.p.x).collect();
    assert!((xs.iter().copied().fold(f64::MAX, f64::min) - 1000.0).abs() < 1e-6);
    assert!((xs.iter().copied().fold(f64::MIN, f64::max) - 1100.0).abs() < 1e-6);
    assert!(s.execute("markupimport", &json!({ "path": dir.join("none.pdf").to_string_lossy() })).is_err());
    // One undo per import.
    let before = s.execute("markups", &json!({})).unwrap()["markups"].as_array().unwrap().len();
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.execute("markups", &json!({})).unwrap()["markups"].as_array().unwrap().len(), before - 4);
}
