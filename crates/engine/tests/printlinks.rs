//! `printset.links` (Q5259) on a folder of printed sheets: the made-up electrical set of
//! [`cadaddy_mepfix::linkset`], one PDF per sheet, read the way a job reads its prints (title
//! blocks classified, plan pages imported, then the detail sheets the callouts name).

use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_mepfix::linkset;
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

/// A job folder holding the set's sheets, one PDF each.
fn prints(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_printlinks_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    for (n, d) in linkset::drawings() {
        std::fs::write(dir.join(format!("{n}.pdf")), linkset::print(&d).expect("print")).expect("write");
    }
    dir
}

fn callouts(v: &Value) -> Vec<(String, String, String, String, bool)> {
    let mut got: Vec<(String, String, String, String, bool)> = v["callouts"]
        .as_array()
        .expect("callouts")
        .iter()
        .map(|c| {
            let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
            (s("on_sheet"), s("kind"), s("detail"), s("target_sheet"), c["resolved"] == true)
        })
        .collect();
    got.sort();
    got
}

#[test]
fn a_folder_of_prints_links_into_one_sheet_map() {
    let _g = serial();
    real_pdf_reader();
    let dir = prints("links");
    let mut s = Session::new();
    let v = s.execute("printset.links", &json!({"pdfs": dir.display().to_string()})).expect("printset.links");
    // The title blocks name the sheets; the plans, the enlarged plan and the detail sheet its
    // callout names are read.
    let numbers: Vec<&str> = v["sheets"].as_array().expect("sheets").iter().filter_map(|x| x["number"].as_str()).collect();
    assert_eq!(numbers, vec!["E-1", "E-2", "E-3", "E-4"], "{v:#}");
    assert_eq!(v["pages_read"], json!(4));
    // This folder holds no architectural sheets: `REFER TO ARCHITECTURAL` points nowhere, as
    // the bubble to E-9 does; both are reported, never dropped.
    let want: Vec<(String, String, String, String, bool)> = [
        ("E-1", "detail_bubble", "2", "E-4", true),
        ("E-1", "detail_bubble", "5", "E-9", false),
        ("E-1", "discipline_ref", "", "A", false),
        ("E-1", "enlarged_plan", "1", "E-3", true),
        ("E-1", "match_line", "", "E-2", true),
        ("E-1", "sheet_ref", "", "E-4", true),
        ("E-2", "match_line", "", "E-1", true),
    ]
    .iter()
    .map(|(a, b, c, d, e)| (a.to_string(), b.to_string(), c.to_string(), d.to_string(), *e))
    .collect();
    assert_eq!(callouts(&v), want, "{v:#}");
    assert_eq!(v["resolved"], json!(5));
    assert_eq!(v["unresolved"].as_array().map(Vec::len), Some(2));
    // The enlarged core plan: found, registered on E-1, its devices counted once.
    let e = &v["enlarged"][0];
    assert_eq!(v["enlarged"].as_array().map(Vec::len), Some(1), "{v:#}");
    assert_eq!(
        (e["sheet"].as_str(), e["target_sheet"].as_str(), e["on_sheet"].as_str(), e["detail"].as_str()),
        (Some("E-3"), Some("E-3"), Some("E-1"), Some("1"))
    );
    assert_eq!(e["registered"], json!(true));
    assert_eq!(e["level"], json!("1"));
    assert_eq!((e["devices"].as_u64(), e["overall_ignored"].as_u64()), (Some(5), Some(3)), "{e:#}");
    assert_eq!(v["enlarged_registered"], json!(1));
    assert_eq!(v["double_counted"], json!(0));
    // One first floor, stitched across the match line, its devices counted once each.
    assert_eq!(v["levels"], json!([{"level": "1", "sheets": ["E-1", "E-2"], "stitched": true, "devices": 16}]), "{v:#}");
    assert_eq!(v["match_lines"][0]["stitched"], json!(true));
    // The detail's mounting height on the receptacle its bubble points from.
    assert_eq!(v["facts"].as_array().map(Vec::len), Some(1), "{v:#}");
    assert_eq!(v["facts"][0]["value_ft"], json!(1.5));
    assert_eq!(v["facts"][0]["applied_to"][0]["tag"], json!("F"));
    // Again on the open print set: the same map.
    let again = s.execute("printset.links", &json!({})).expect("printset.links on the open set");
    assert_eq!(callouts(&again), want);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_job_and_bim_from_pdf_report_the_sheet_map() {
    let _g = serial();
    real_pdf_reader();
    let dir = prints("job");
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "checkpoint": false})).expect("job.start");
    let links = &st["links"];
    assert_eq!(links["enlarged_registered"], json!(1), "{st:#}");
    assert_eq!(links["double_counted"], json!(0));
    assert!(links["callouts"].as_u64().is_some_and(|n| n >= 6), "{links:#}");
    let mut b = Session::new();
    let r = b.execute("bim.from_pdf", &json!({"path": dir.display().to_string(), "force": true})).expect("bim.from_pdf");
    let links = &r["links"];
    assert_eq!(links["sheets"], json!(4), "{links:#}");
    assert_eq!(links["enlarged_registered"], json!(1), "{links:#}");
    assert_eq!(links["enlarged"][0]["devices"], json!(5));
    std::fs::remove_dir_all(&dir).ok();
}
