//! End to end: a reviewed PDF's markups imported (MARKUPIMPORT typed with its JSON form), the
//! Markups palette opened from MARKUPS showing them, one ticked done (MARKUPSTATUS), and the
//! markup objects drawn.

mod e2e_common;

use cadaddy_engine::doc::markup::{Markup, MarkupKind};
use e2e_common::{E2e, folder};
use serde_json::json;

fn markup(kind: MarkupKind, rect: [f64; 4], text: &str) -> Markup {
    Markup {
        kind,
        points: vec![[rect[0], rect[1]], [rect[2], rect[1]], [rect[2], rect[3]], [rect[0], rect[3]]],
        strokes: Vec::new(),
        rect,
        text: text.into(),
        author: "Reviewer A".into(),
        subject: String::new(),
        color: Some([255, 0, 0]),
        date: String::new(),
    }
}

#[test]
fn markups_are_imported_listed_and_ticked_done() {
    let mut e = E2e::new();
    let dir = folder("markups");
    let pdf = dir.join("Sample Plan reviewed.pdf");
    let marks = [
        markup(MarkupKind::Rectangle, [100.0, 100.0, 200.0, 160.0], "Check this wall"),
        markup(MarkupKind::Note, [300.0, 300.0, 320.0, 320.0], "Add a door here"),
    ];
    std::fs::write(&pdf, cadaddy_io::pdf_markups::annotated_pdf(612.0, 792.0, &marks).unwrap()).unwrap();
    let before = e.entity_count();
    let r = e.exec("markupimport", json!({ "path": pdf, "scale": 1.0, "at": [0, 0] }));
    let placed = r["placed"].as_array().cloned().unwrap_or_default();
    assert_eq!(placed.len(), 2, "{r}");
    assert!(e.entity_count() > before, "markup objects drawn");
    assert!(e.inked(400, 400) > 0, "the markups render");

    // MARKUPS typed alone opens the palette, listing both.
    e.typed("markups");
    assert_eq!(e.dialog().as_deref(), Some("markups"));
    let list = e.exec("markups", json!({}));
    let ms = list["markups"].as_array().cloned().unwrap_or_default();
    assert_eq!(ms.len(), 2);
    assert!(ms.iter().any(|m| m["text"] == "Check this wall") && ms.iter().all(|m| m["status"] == "open"), "{list}");
    let labels: Vec<String> = e.elements().iter().filter_map(|x| x["label"].as_str().map(str::to_string)).collect();
    assert!(labels.iter().any(|l| l.contains("Check this wall")), "the palette shows the comment: {labels:?}");

    // Ticked done.
    let id = ms[0]["id"].as_str().unwrap().to_string();
    e.exec("markupstatus", json!({ "id": id, "status": "done" }));
    let list = e.exec("markups", json!({}));
    assert!(list["markups"].as_array().unwrap().iter().any(|m| m["id"] == id.as_str() && m["status"] == "done"));
    let _ = std::fs::remove_dir_all(&dir);
}
