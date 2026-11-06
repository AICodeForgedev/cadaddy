//! Soak S5-001: the prints are read in inches, so a guided job in a metric drawing built a model
//! 25.4 times too small (no rooms, a takeoff of a few square feet). An empty drawing takes the
//! prints' units and builds the imperial job's model; one with work in it is refused.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

fn real_pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

/// The made-up office's floor plan and mechanical plan (seed 1) in a folder of their own.
fn prints(tag: &str) -> std::path::PathBuf {
    let set = cadaddy_mepfix::generate(1).expect("generate");
    let dir = std::env::temp_dir().join(format!("cadaddy-jobunits-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    for name in ["A-101.pdf", "M-101.pdf"] {
        std::fs::write(dir.join(name), set.file(name).expect("sheet")).expect("write");
    }
    dir
}

/// A BIM job run unattended in `d`: its status, and what the model holds.
fn job(d: Drawing, tag: &str) -> (Value, Value) {
    let dir = prints(tag);
    let mut s = Session::empty();
    s.open_drawing(d, "Drawing1", None);
    let mut st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    for _ in 0..50 {
        if st["done"] == json!(true) {
            break;
        }
        st = s.execute("job.next", &json!({})).expect("job.next");
    }
    assert_eq!(st["done"], json!(true), "{st}");
    let rooms = s.execute("bim.props.find", &json!({"kind": "room", "limit": 1})).expect("rooms")["count"].clone();
    let tree = s.execute("bim.tree", &json!({"limit": 1})).expect("tree")["total"].clone();
    let count = s.execute("bim.count", &json!({})).expect("count");
    let takeoff = s.execute("bim.takeoff", &json!({"keep": false})).expect("takeoff")["totals"].clone();
    let _ = std::fs::remove_dir_all(&dir);
    (st, json!({"rooms": rooms, "elements": tree, "pieces": count["count"], "area_sf": count["area_sf"], "takeoff": takeoff}))
}

#[test]
fn a_job_in_an_empty_metric_drawing_builds_the_imperial_jobs_model() {
    let _g = serial();
    real_pdf_reader();
    let (_, imperial) = job(Drawing::new_imperial(), "imperial");
    assert!(imperial["rooms"].as_u64().unwrap_or(0) >= 5, "{imperial}");
    let (st, metric) = job(Drawing::new_metric(), "metric");
    assert_eq!(metric, imperial, "the same prints, the same model");
    assert!(st["units_note"].as_str().is_some_and(|n| n.contains("millimetres") && n.contains("inches")), "{st}");
}

#[test]
fn a_job_in_a_metric_drawing_with_work_in_it_is_refused() {
    let _g = serial();
    real_pdf_reader();
    let dir = prints("refused");
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_metric(), "Drawing1", None);
    s.execute("line", &json!({"points": [[0, 0], [1000, 0]]})).expect("line");
    let e = s
        .execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("millimetres") && e.contains("new imperial drawing"), "{e}");
    // Nothing imported, the drawing's units as they were.
    let d = s.doc().expect("doc");
    assert_eq!(d.model.len(), 1);
    assert_eq!(d.header.i64("INSUNITS", 0), 4);
    let _ = std::fs::remove_dir_all(&dir);
}
