//! Q6382: a job on a folder of DWG or DXF sheets (`pdfs: path (folder of PDFs or DWGs)`) read
//! every sheet as a PDF ("could not import: PDF: couldn't parse input") and ended
//! `done_with_warnings` with an empty model, the import failure nowhere in the reply. Each sheet
//! is now read by its own format's reader, and a sheet that can't be read is in the job's reply:
//! the job stops when none can be read, and says which when some can't.
//! Made-up sets only: the office generator's sheets, saved as PDF, DXF and DWG.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};
use std::path::PathBuf;

fn reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

const SHEETS: [&str; 2] = ["A-101", "M-101"];

/// The made-up office's floor plan and mechanical plan (seed 1) in a folder of their own, as
/// `ext` files (`pdf`, `dxf`; `dwg`: the DXF saved as DWG).
fn prints(tag: &str, ext: &str) -> PathBuf {
    let set = cadaddy_mepfix::generate(1).expect("generate");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy-drawingsheets-{}-{tag}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    for n in SHEETS {
        let (from, to) = match ext {
            "dwg" => (format!("{n}.dxf"), format!("{n}.dwg")),
            e => (format!("{n}.{e}"), format!("{n}.{e}")),
        };
        let bytes = set.file(&from).expect("sheet").to_vec();
        let bytes = if ext == "dwg" {
            let d = cadaddy_io::read(&bytes, &from).expect("read the DXF");
            cadaddy_io::write(&d, &to).expect("save as DWG")
        } else {
            bytes
        };
        std::fs::write(dir.join(to), bytes).expect("write");
    }
    dir
}

/// A BIM job run unattended on `dir`: its last reply, and what the model holds.
fn job(dir: &std::path::Path) -> (Value, Value) {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Drawing1", None);
    let mut st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    for _ in 0..50 {
        if st["done"] == json!(true) || st["status"] == json!("error") {
            break;
        }
        match s.execute("job.next", &json!({})) {
            Ok(v) => st = v,
            Err(e) => {
                st = json!({"error": e.to_string()});
                break;
            }
        }
    }
    let rooms = s.execute("bim.props.find", &json!({"kind": "room", "limit": 1})).map(|r| r["count"].clone()).unwrap_or(Value::Null);
    let count = s.execute("bim.count", &json!({})).map(|c| c["count"].clone()).unwrap_or(Value::Null);
    (st, json!({"rooms": rooms, "pieces": count}))
}

fn rooms(m: &Value) -> u64 {
    m["rooms"].as_u64().unwrap_or(0)
}

#[test]
fn a_folder_of_dxf_or_dwg_sheets_builds_the_model_its_pdfs_do() {
    let _g = serial();
    reader();
    let pdf = prints("pdf", "pdf");
    let (_, from_pdf) = job(&pdf);
    let _ = std::fs::remove_dir_all(&pdf);
    assert!(rooms(&from_pdf) >= 5, "the PDF set builds the office: {from_pdf}");
    for ext in ["dxf", "dwg"] {
        let dir = prints(ext, ext);
        let (st, model) = job(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let text = st.to_string();
        assert!(!text.contains("couldn't parse input"), "{ext}: read as a PDF: {text}");
        assert_eq!(st["done"], json!(true), "{ext}: {st}");
        // The same office: each room the PDFs build (a sheet drawn at full size reads as it prints).
        assert_eq!(rooms(&model), rooms(&from_pdf), "{ext}: {model} against the PDFs' {from_pdf}");
    }
}

/// A job answered as a user who takes each default, on the DXF set whose `damaged` sheets are
/// spoilt after classify read them (moved, half-copied, overwritten before the build imports
/// them: the job stops at its sheet-index checkpoint after classify, which allows it). Its last
/// reply, every reply on the way, and `job.status` at the end.
fn damaged_job(tag: &str, damaged: &[&str]) -> (Value, String, Value) {
    let dir = prints(tag, "dxf");
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Drawing1", None);
    let mut st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": false, "checkpoint": false})).expect("job.start");
    for n in damaged {
        std::fs::write(dir.join(format!("{n}.dxf")), b"0\nSECTION\n2\nENTITIES\n0\nLINE\n").expect("damage");
    }
    let mut seen = vec![st.to_string()];
    for _ in 0..60 {
        if st["done"] == json!(true) || st.get("error").is_some() {
            break;
        }
        let q = st.get("question").or_else(|| st.get("next")).cloned().unwrap_or(Value::Null);
        let r = match q.get("id").and_then(Value::as_str) {
            Some(id) => {
                let a = q.get("default").cloned().filter(|d| !d.is_null()).or_else(|| q["choices"].get(0).cloned()).unwrap_or(json!("yes"));
                s.execute("job.answer", &json!({"id": id, "answer": a})).and_then(|_| s.execute("job.next", &json!({})))
            }
            None => s.execute("job.next", &json!({})),
        };
        st = r.unwrap_or_else(|e| json!({"error": e.to_string()}));
        seen.push(st.to_string());
    }
    let status = s.execute("job.status", &json!({})).expect("job.status");
    let _ = std::fs::remove_dir_all(&dir);
    (st, seen.join(" "), status)
}

#[test]
fn a_set_none_of_whose_sheets_can_be_read_stops_the_job_saying_so() {
    let _g = serial();
    reader();
    let (st, all, status) = damaged_job("none", &SHEETS);
    // Never an empty model reported as done: the job stops, its reply naming the sheets.
    assert_ne!(st["done"], json!(true), "{all}");
    let error = st["error"].as_str().unwrap_or_default();
    assert!(error.contains("could not be read") && error.contains("A-101") && error.contains("M-101"), "{st}");
    assert_eq!(status["status"], json!("error"), "{status}");
}

#[test]
fn a_sheet_that_cannot_be_read_is_asked_about_and_in_the_reply() {
    let _g = serial();
    reader();
    let (st, all, status) = damaged_job("some", &["M-101"]);
    // The question names it (the default goes on without it)...
    assert!(all.contains("could not be read and are not built: M-101"), "{all}");
    // ...and the finished job's reply warns of it; the floor plan read is built.
    assert_eq!(st["done"], json!(true), "{all}");
    assert_eq!(status["status"], json!("done_with_warnings"), "{status}");
    let report = status["warnings"].to_string();
    assert!(report.contains("sheets could not be read") && report.contains("M-101"), "{report}");
    assert!(!report.contains("could not be read and are not built: A-101"), "{report}");
}
