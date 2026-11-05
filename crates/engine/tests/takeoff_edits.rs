//! Soak S5-002: after a plain ERASE, COPY, EXPLODE or MOVE of model pieces (and their undo and
//! redo), the takeoff, the BOM and an estimate are what they are after the model's count index
//! is rebuilt from the elements: the electrical lines the BOM counts from that index follow the
//! edit (they kept the model as built).

use std::sync::{Mutex, MutexGuard};

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

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// The made-up office's floor plan and electrical plan (seed 1), built unattended.
fn built() -> Session {
    let set = cadaddy_mepfix::generate(1).expect("generate");
    let dir = std::env::temp_dir().join(format!("cadaddy-takeoff-edits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    for name in ["A-101.pdf", "E-101.pdf"] {
        std::fs::write(dir.join(name), set.file(name).expect("sheet")).expect("write");
    }
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Drawing1", None);
    let mut st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    for _ in 0..50 {
        if st["done"] == json!(true) {
            break;
        }
        st = s.execute("job.next", &json!({})).expect("job.next");
    }
    assert_eq!(st["done"], json!(true), "{st}");
    let _ = std::fs::remove_dir_all(&dir);
    s
}

/// The takeoff's lines, the BOM's lines and an estimate's total.
fn quantities(s: &mut Session, rows: &Value) -> (Value, Value, Value) {
    let t = s.execute("bim.takeoff", &json!({"keep": false})).expect("takeoff");
    let b = s.execute("bim.bom", &json!({})).expect("bom");
    let e = s.execute("bim.estimate", &json!({"rows": rows})).expect("estimate");
    (t["lines"].clone(), b["lines"].clone(), e["total"].clone())
}

/// The quantities now equal those after `bim.count {refresh: true}`; the takeoff's count total.
fn agrees_with_a_recount(s: &mut Session, rows: &Value, step: &str) -> f64 {
    let c = s.execute("bim.count", &json!({"verify": true})).expect("count");
    assert_eq!(c["verify"]["matches"], json!(true), "{step}: {c}");
    let now = quantities(s, rows);
    s.execute("bim.count", &json!({"refresh": true})).expect("refresh");
    assert!(now == quantities(s, rows), "{step}: the takeoff, BOM or estimate read a stale count index");
    s.execute("bim.takeoff", &json!({"keep": false})).expect("takeoff")["totals"]["count"].as_f64().unwrap_or(0.0)
}

fn sticks(s: &mut Session, n: usize) -> Vec<String> {
    let f = s.execute("bim.props.find", &json!({"kind": "conduit", "limit": n})).expect("find");
    f["handles"].as_array().into_iter().flatten().filter_map(|h| h.as_str().map(str::to_string)).collect()
}

#[test]
fn the_takeoff_bom_and_estimate_follow_plain_edits_of_model_pieces() {
    let _g = serial();
    real_pdf_reader();
    let mut s = built();
    let lines = s.execute("bim.takeoff", &json!({"keep": false})).expect("takeoff")["lines"].clone();
    let rows: Value =
        lines.as_array().into_iter().flatten().map(|l| json!({"item": l["item"], "unit": l["unit"], "material": 1.0, "labor_hrs": 0.1})).collect();
    let built = agrees_with_a_recount(&mut s, &rows, "built");
    let two = sticks(&mut s, 2);
    assert_eq!(two.len(), 2, "the electrical plan's conduit is built as sticks");
    s.execute("erase", &json!({"handles": two})).expect("erase");
    let erased = agrees_with_a_recount(&mut s, &rows, "erase");
    assert!(erased < built, "the erased sticks leave the takeoff: {built} -> {erased}");
    let h = sticks(&mut s, 1);
    s.execute("copy", &json!({"handles": h, "delta": [0, 48]})).expect("copy");
    assert!(agrees_with_a_recount(&mut s, &rows, "copy") > erased);
    let h = sticks(&mut s, 1);
    s.execute("explode", &json!({"handles": h})).expect("explode");
    agrees_with_a_recount(&mut s, &rows, "explode");
    let h = sticks(&mut s, 3);
    s.execute("move", &json!({"handles": h, "delta": [12, 0]})).expect("move");
    agrees_with_a_recount(&mut s, &rows, "move");
    for (cmd, k) in [("undo", 2), ("redo", 1), ("undo", 3)] {
        s.execute(cmd, &json!({"count": k})).expect(cmd);
        agrees_with_a_recount(&mut s, &rows, &format!("{cmd} {k}"));
    }
    assert_eq!(agrees_with_a_recount(&mut s, &rows, "all undone"), built);
}
