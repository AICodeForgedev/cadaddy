//! Redline: SEND TO REDLINE plots to one PDF (opened only when Redline is installed), IMPORT
//! REDLINE MARKUPS brings each reviewer's markups back on their own layer.

use cadaddy_doc::markup::{Markup, MarkupKind};
use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io, set_pdf_merge};
use cadaddy_engine::doc::Handle;
use serde_json::json;

fn note(author: &str, rect: [f64; 4], text: &str) -> Markup {
    Markup {
        kind: MarkupKind::Note,
        points: Vec::new(),
        strokes: Vec::new(),
        rect,
        text: text.into(),
        author: author.into(),
        subject: String::new(),
        color: None,
        date: String::new(),
    }
}

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-redline-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn each_reviewer_gets_a_layer() {
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    let d = dir("import");
    let markups = vec![
        note("Ann Lee", [72.0, 72.0, 144.0, 100.0], "Move this"),
        note("Bob/QA", [200.0, 200.0, 300.0, 230.0], "Check that"),
        note("", [400.0, 400.0, 500.0, 430.0], "Anonymous"),
    ];
    let p = d.join("review.pdf");
    std::fs::write(&p, cadaddy_io::pdf_markups::annotated_pdf(1224.0, 792.0, &markups).unwrap()).unwrap();
    let mut s = Session::new();
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    let r = s.execute("redline.import", &json!({ "path": p.to_string_lossy() })).unwrap();
    let placed = r["placed"].as_array().unwrap();
    let layers: Vec<&str> = placed.iter().map(|x| x["layer"].as_str().unwrap()).collect();
    assert_eq!(layers, ["MARKUP-Ann Lee", "MARKUP-Bob_QA", "MARKUP"], "{r}");
    let doc = s.doc().unwrap();
    for (x, name) in placed.iter().zip(&layers) {
        assert!(doc.layer(name).is_some_and(|l| !l.plot), "{name}: a layer that doesn't plot");
        let h = Handle::parse_hex(x["handles"][0].as_str().unwrap()).unwrap();
        assert_eq!(doc.entity(h).unwrap().common.layer, *name);
    }
    // Plain MARKUPIMPORT keeps the one layer.
    let r = s.execute("markupimport", &json!({ "path": p.to_string_lossy() })).unwrap();
    assert!(r["placed"].as_array().unwrap().iter().all(|x| x["layer"] == "MARKUP"), "{r}");
    std::fs::remove_dir_all(&d).ok();
}

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
    });
    set_pdf_merge(|parts| cadaddy_io::pdf_merge::merge(parts).map_err(|e| e.to_string()));
}

#[test]
fn send_saves_the_pdf_when_redline_is_not_installed() {
    hooks();
    let d = dir("send");
    let mut s = Session::new();
    s.execute("line", &json!({ "points": [[0, 0], [100, 50]] })).unwrap();
    let out = d.join("sheet.pdf");
    let r = s.execute("redline", &json!({ "path": out.to_string_lossy() })).unwrap();
    assert_eq!(r["opened"], false, "no opener installed in tests: {r}");
    assert_eq!(r["path"], out.to_string_lossy().as_ref());
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"%PDF"), "a PDF");
    // Hostile params don't panic.
    for p in [json!({ "path": 5 }), json!({ "layouts": "x" }), json!(null)] {
        let _ = s.execute("redline", &p);
        let _ = s.execute("redline.import", &p);
    }
    std::fs::remove_dir_all(&d).ok();
}
