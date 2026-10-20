//! End to end: PLOTSTAMP. Turned on for model space with a field of our own, a plot to PDF carries
//! the stamp (its text in the page), the drawing's name and layout name in it; turned off, the next
//! plot has none.

mod e2e_common;

use e2e_common::{E2e, folder};
use serde_json::json;

/// The PDF of a plot of model space, uncompressed so its text can be read.
fn plot(e: &mut E2e, path: &std::path::Path) -> String {
    e.exec("plot", json!({ "path": path, "layout": "Model", "compress": false }));
    let bytes = std::fs::read(path).expect("the PDF");
    assert!(bytes.starts_with(b"%PDF"));
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn the_stamp_is_plotted_while_it_is_on() {
    let mut e = E2e::new();
    let dir = folder("plotstamp");
    e.typed("rectang 0,0 100,60");
    let dwg = dir.join("Sample Floor Plan.dwg");
    e.exec("saveas", json!({ "path": dwg }));
    let pdf = dir.join("plot.pdf");
    assert!(!plot(&mut e, &pdf).contains("Issued for review"), "off by default");

    let r = e.exec("plotstamp", json!({ "layout": "Model", "on": true, "userFields": ["Issued for review"], "dateTime": false }));
    assert_eq!(r["on"], true, "{r}");
    let text = plot(&mut e, &pdf);
    let stamp = text.lines().find(|l| l.contains("Issued for review") && l.contains(") Tj")).unwrap_or_else(|| panic!("no stamp in the PDF"));
    assert!(stamp.contains("(Sample Floor Plan.dwg, Model, ") && stamp.contains("/FS"), "{stamp}");

    e.exec("plotstamp", json!({ "layout": "Model", "on": false }));
    assert!(!plot(&mut e, &pdf).contains("Issued for review"), "off again");
    let _ = std::fs::remove_dir_all(&dir);
}
