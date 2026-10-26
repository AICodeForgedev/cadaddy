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
        (e["sheet"].as_str(), e["on_sheet"].as_str(), e["target_sheet"].as_str(), e["overall_sheet"].as_str(), e["detail"].as_str()),
        (Some("E-3"), Some("E-3"), Some("E-1"), Some("E-1"), Some("1"))
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
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "checkpoint": false, "auto": true})).expect("job.start");
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

/// The fixture crate's whole made-up set (cadaddy_mepfix::generate): its enlarged plumbing
/// plans are boxed with dashed rectangles on the overall plans (one box's sides lie on dash-dot
/// grid lines) and named by `SEE ENLARGED PLAN P-401` notes, not bubbles; its equipment tags are
/// circled like bubbles (`RG-2`, `SH-1`); one bubble points at a sheet the set doesn't have.
#[test]
fn the_fixture_sets_enlarged_plans_register_on_their_overall_plans() {
    let _g = serial();
    real_pdf_reader();
    let set = cadaddy_mepfix::generate(42).expect("fixture set");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_printlinks_fixture_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    for sh in &set.truth.sheets {
        std::fs::write(dir.join(&sh.pdf), set.file(&sh.pdf).expect("sheet pdf")).expect("write");
    }
    let mut s = Session::new();
    let v = s.execute("printset.links", &json!({"pdfs": dir.display().to_string()})).expect("printset.links");
    let enlarged: Vec<(String, String, bool)> = v["enlarged"]
        .as_array()
        .expect("enlarged")
        .iter()
        .map(|e| (e["on_sheet"].as_str().unwrap_or("").to_string(), e["target_sheet"].as_str().unwrap_or("").to_string(), e["registered"] == true))
        .collect();
    let want: Vec<(String, String, bool)> = [("P-401", "P-101", true), ("P-402", "P-102", true), ("P-403", "P-101", true)]
        .iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), *c))
        .collect();
    assert_eq!(enlarged, want, "{:#}", v["enlarged"]);
    assert_eq!(v["double_counted"], json!(0));
    // Each boundary lies on its overall plan's paper, within the plan's own block.
    for e in v["enlarged"].as_array().into_iter().flatten() {
        let b: Vec<f64> = e["boundary"].as_array().expect("boundary").iter().filter_map(Value::as_f64).collect();
        assert!(b.len() == 4 && b[2] > b[0] && b[3] > b[1], "{e}");
        // The view is its plan, not its title's underline bar: it holds the plan's devices.
        assert!(e["devices"].as_u64().is_some_and(|n| n >= 1), "{e}");
    }
    // The circled equipment tags are not callouts; the bubble to A-999 is reported.
    let unresolved: Vec<&str> = v["unresolved"].as_array().expect("unresolved").iter().filter_map(|u| u["target_sheet"].as_str()).collect();
    assert_eq!(unresolved, vec!["A-999"], "{:#}", v["unresolved"]);
    assert!(v["callouts"].as_array().map_or(0, Vec::len) >= 30, "{:#}", v["callouts"]);
    assert!(v["resolved_pct"].as_f64().is_some_and(|p| p >= 0.95), "{}", v["resolved_pct"]);
    std::fs::remove_dir_all(&dir).ok();
}

/// MUST 3 in the guided job: inside a registered enlarged plan's boundary the enlarged sheet is
/// the authority. Its five devices go into the model where they sit on the overall plan (inside
/// the boundary, on the first floor); E-1's three copies there are left out; every other device
/// stays where it was.
#[test]
fn the_jobs_enlarged_plan_devices_land_in_its_boundary_once() {
    let _g = serial();
    real_pdf_reader();
    let dir = prints("authority");
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "checkpoint": false, "auto": true})).expect("job.start");
    let devices = s.execute("job.explain", &json!({"stage": "devices"})).expect("explain devices");
    let r = &devices["result"];
    assert_eq!(r["enlarged_devices"], json!({"moved_onto_overall_plans": 5, "overall_copies_left_out": 3}), "{r:#}\n{st:#}");
    // Counted once: the level's 16, as the sheet map counts them.
    assert_eq!(r["devices"]["count"], json!(16), "{:#}", r["devices"]);
    let b: Vec<f64> = r["links"]["enlarged"][0]["boundary"].as_array().expect("boundary").iter().filter_map(Value::as_f64).collect();
    assert_eq!(b.len(), 4);
    let spec = s.execute("bspec.get", &json!({})).expect("bspec.get");
    let upf = spec["spec"]["units_per_foot"].as_f64().filter(|v| *v > 0.0).unwrap_or(12.0);
    let tags: Vec<&Value> = spec["spec"]["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["id"].as_str().is_some_and(|i| i.starts_with("device:")) && t["fields"].get("symbol").is_some())
        .collect();
    assert_eq!(tags.len(), 16, "{tags:#?}");
    let sheet = |t: &Value| t["fields"]["sheet"]["value"].as_str().unwrap_or("").to_string();
    let inside: Vec<&&Value> = tags
        .iter()
        .filter(|t| {
            let (x, y) = (t["at_ft"][0].as_f64().unwrap_or(f64::NAN) * upf, t["at_ft"][1].as_f64().unwrap_or(f64::NAN) * upf);
            x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]
        })
        .collect();
    assert_eq!(inside.len(), 5, "{inside:#?}");
    assert!(inside.iter().all(|t| sheet(t) == "E-3"), "{inside:#?}");
    let e1_level = tags.iter().find(|t| sheet(t) == "E-1").map(|t| t["level"].clone()).expect("an E-1 device");
    assert!(inside.iter().all(|t| t["level"] == e1_level), "{inside:#?}");
    std::fs::remove_dir_all(&dir).ok();
}
