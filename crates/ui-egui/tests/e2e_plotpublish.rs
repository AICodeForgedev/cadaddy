//! End to end: plot preview and PUBLISH. The preview is the plot as an image (ink on it), the
//! Plot Preview window opens from its menu, and PUBLISH writes one PDF with a page per sheet.

mod e2e_common;

use e2e_common::{E2e, folder, inked};
use serde_json::json;

#[test]
fn preview_shows_the_plot_and_publish_writes_the_sheets() {
    let mut e = E2e::new();
    let dir = folder("plotpublish");
    e.typed("rectang 0,0 100,60");
    e.typed("circle 50,30 20");
    // The preview: the plot as an image, with the drawing's ink on it.
    let p = e.exec("preview", json!({ "layout": "Model", "width": 600 }));
    let png = cadaddy_engine::cmd::file::base64_decode(p["data"].as_str().unwrap()).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    assert!(img.width() >= 300 && inked(&img) > 200, "the preview is blank: {}x{}", img.width(), img.height());
    // Plot Preview from the menu opens its window.
    e.menu("preview");
    assert_eq!(e.dialog().as_deref(), Some("preview"));
    e.ok("engine.execute", json!({ "command": "ui.dialog.close", "params": {} }));
    // PUBLISH: Model and a layout in one PDF.
    let pdf = dir.join("Project A Set.pdf");
    let r = e.exec("publish", json!({ "path": pdf, "layouts": ["Model", "Layout1"], "single": true }));
    let bytes = std::fs::read(&pdf).unwrap_or_else(|err| panic!("{r}: {err}"));
    assert!(bytes.starts_with(b"%PDF"));
    // Page objects: `/Type /Page` (or `/Type/Page`) not followed by `s` (the page tree).
    let text = String::from_utf8_lossy(&bytes).replace("/Type/Page", "/Type /Page");
    let pages = text.match_indices("/Type /Page").filter(|(i, _)| !text[i + 11..].starts_with('s')).count();
    assert!(pages >= 2, "two sheets → two pages, found {pages}: {r}");
    let _ = std::fs::remove_dir_all(&dir);
}
