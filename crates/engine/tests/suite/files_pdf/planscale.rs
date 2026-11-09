//! Q6396: a sheet comes in at the scale its plans print, not at whichever note its title-block
//! corner holds. The made-up sheet draws a roof plan at 1/8" = 1'-0" and, in the lower right
//! corner where a title block's scale field would be, a small elevation at 1/2" = 1'-0": read by
//! the elevation's note the roof plan came in at a quarter of its size, and no trade's roof plan
//! (drawn at 1/4") could be fitted onto it. A sheet that prints one scale keeps it.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Space;
use serde_json::{Value, json};

fn io() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": at, "text": t, "height": h})).unwrap();
}

/// A 17" × 11" sheet in sheet inches: its border, a plan titled `plan_title` printing
/// `plan_scale`, and (with `detail`) a small elevation at 1/2" = 1'-0" in the lower right corner.
fn sheet(plan_title: &str, plan_scale: &str, detail: bool) -> Session {
    let mut s = Session::new();
    rect(&mut s, [0.0, 0.0], [17.0, 11.0]);
    // The plan: an outline with a few curbs and a drain box inside, 8" × 6" on the sheet.
    rect(&mut s, [1.0, 3.0], [9.0, 9.0]);
    rect(&mut s, [2.0, 4.0], [3.5, 5.0]);
    rect(&mut s, [5.0, 6.5], [6.0, 7.5]);
    rect(&mut s, [7.0, 4.0], [7.4, 4.4]);
    text(&mut s, [1.0, 2.15], plan_title, 0.25);
    text(&mut s, [1.0, 1.9], plan_scale, 0.12);
    if detail {
        rect(&mut s, [11.0, 1.8], [14.0, 2.8]);
        text(&mut s, [11.0, 1.15], "ELEVATION - EQUIPMENT SCREEN", 0.25);
        text(&mut s, [11.0, 0.9], "1/2\" = 1'-0\"", 0.12);
    }
    text(&mut s, [15.0, 0.6], "A-103", 0.4);
    s
}

/// The sheet through PDF at paper size, imported by `pdfimport` with its scale read off it.
fn import(s: &Session, tag: &str) -> Value {
    io();
    let opts = json!({"layers": false, "fit": false, "center": true, "scale": 1.0, "paper": "ANSI B", "landscape": true});
    let bytes = cadaddy_io::pdf::plot(s.doc().unwrap(), &Space::Model, &opts).unwrap();
    let dir = std::env::temp_dir().join(format!("cadaddy-planscale-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Sample Roof Plan.pdf");
    std::fs::write(&path, bytes).unwrap();
    let mut t = Session::new();
    let r = t.execute("pdfimport", &json!({"path": path.to_string_lossy(), "layout": false, "images": false})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[test]
fn a_sheet_comes_in_at_the_scale_its_plan_prints() {
    let r = import(&sheet("ROOF PLAN", "1/8\" = 1'-0\"", true), "detail");
    // 1/8" = 1'-0": 96 drawing inches a sheet inch (the elevation's note alone gave 24).
    assert_eq!(r["scale"].as_f64(), Some(96.0), "{}", r["scaleSource"]);
    let src = r["scaleSource"].as_str().unwrap_or("");
    assert!(src.starts_with("scale note 1/8\""), "{src}");
    assert!(src.contains("ROOF PLAN") && src.contains("24"), "says which view gave it and what was read first: {src}");
    assert_eq!(r["planPrintScale"].as_f64(), Some(0.125), "{r}");
    assert_eq!(r["planScaleReread"]["from"].as_f64(), Some(24.0), "{r}");
}

#[test]
fn a_sheet_of_one_scale_keeps_its_note() {
    let r = import(&sheet("ROOF PLAN", "1/4\" = 1'-0\"", false), "one");
    assert_eq!(r["scale"].as_f64(), Some(48.0), "{}", r["scaleSource"]);
    assert!(r.get("planScaleReread").is_none(), "{r}");
}

#[test]
fn a_given_scale_is_never_read_again() {
    io();
    let s = sheet("ROOF PLAN", "1/8\" = 1'-0\"", true);
    let opts = json!({"layers": false, "fit": false, "center": true, "scale": 1.0, "paper": "ANSI B", "landscape": true});
    let bytes = cadaddy_io::pdf::plot(s.doc().unwrap(), &Space::Model, &opts).unwrap();
    let dir = std::env::temp_dir().join(format!("cadaddy-planscale-given-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Sample Roof Plan.pdf");
    std::fs::write(&path, bytes).unwrap();
    let mut t = Session::new();
    let r = t.execute("pdfimport", &json!({"path": path.to_string_lossy(), "scale": 24, "layout": false, "images": false})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(r["scale"].as_f64(), Some(24.0), "{r}");
    assert!(r.get("planScaleReread").is_none(), "{r}");
}
