//! Q5275 `bim.from_pdf` on a whole print set: a made-up merged set (cover sheet first) builds
//! from its plan pages, not page 1; a hard-failed import check is never a pass; nothing is
//! written next to the PDFs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
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

fn temp(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_bimpdfpages_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    dir
}

/// A prints folder holding one merged PDF of the made-up hotel's `only` sheets, in that order.
fn prints(tag: &str, only: &[&str]) -> (PathBuf, PathBuf) {
    let set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    let pages: Vec<(Vec<u8>, String, String)> = only
        .iter()
        .filter_map(|n| set.truth.sheets.iter().find(|s| s.number == *n))
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    assert_eq!(pages.len(), only.len());
    let dir = temp(tag);
    let pdf = dir.join("Sample Set.pdf");
    std::fs::write(&pdf, cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    (dir, pdf)
}

/// Every file and folder under `dir`.
fn listing(dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).expect("read dir").flatten() {
            let p = e.path();
            out.insert(p.strip_prefix(dir).expect("inside").to_string_lossy().to_string());
            if p.is_dir() {
                todo.push(p);
            }
        }
    }
    out
}

fn sheets(r: &Value) -> Vec<String> {
    r["pages"].as_array().expect("pages").iter().filter_map(|p| p["sheet"].as_str().map(str::to_string)).collect()
}

/// MUST 2: a hard fail is never reported as a pass.
fn check_is_honest(r: &Value) {
    for c in r["check_results"].as_array().expect("check results") {
        let hard = c["hard_fails"].as_array().expect("hard_fails listed");
        if !hard.is_empty() {
            assert_eq!(c["pass"], json!(false), "a hard fail reported as a pass: {c}");
        }
        let failed_hard = c["checks"].as_array().expect("checks").iter().any(|k| k["hard_fail"] == json!(true) && k["pass"] == json!(false));
        assert_eq!(failed_hard, !hard.is_empty(), "{c}");
    }
}

#[test]
fn a_merged_set_builds_from_its_plan_pages_not_the_cover() {
    io();
    let (dir, pdf) = prints("set", &["G-001", "A-101", "A-102", "M-101"]);
    let before = listing(&dir);
    let out_dir = temp("set_out");
    let out = out_dir.join("Sample Model.cdy");
    let mut s = Session::new();
    let r = s.execute("bim.from_pdf", &json!({"path": pdf.to_string_lossy(), "out": out.to_string_lossy(), "force": true})).expect("bim.from_pdf");
    // MUST 1: the plan pages, each named with its level; the cover sheet is not one of them.
    let built = sheets(&r);
    assert!(built.iter().any(|n| n == "A-101") && built.iter().any(|n| n == "A-102"), "{r}");
    assert!(!built.iter().any(|n| n == "G-001"), "the cover sheet is no plan: {built:?}");
    let note = r["pages_note"].as_str().expect("pages note");
    assert!(note.contains("A-101") && note.contains("A-102"), "says which pages it built: {note}");
    let levels: BTreeSet<&str> = r["pages"].as_array().expect("pages").iter().filter_map(|p| p["level"].as_str()).collect();
    assert!(levels.len() >= 2, "the floor plans keep their levels: {levels:?}");
    assert!(r["levels"].as_array().is_some_and(|l| l.len() >= 2), "stacked levels built: {}", r["levels"]);
    check_is_honest(&r);
    // MUST 3: saved at --out only; the prints folder is as it was.
    assert_eq!(r["cdy_paths"], json!([out.to_string_lossy()]), "{r}");
    assert!(out.is_file());
    assert_eq!(listing(&dir), before, "nothing is written next to the PDF");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn without_out_the_model_goes_to_a_temp_folder() {
    io();
    let (dir, pdf) = prints("noout", &["A-101", "A-102"]);
    let before = listing(&dir);
    let mut s = Session::new();
    let r = s.execute("bim.from_pdf", &json!({"path": pdf.to_string_lossy(), "force": true})).expect("bim.from_pdf");
    let saved = r["cdy_paths"][0].as_str().map(PathBuf::from).expect("saved");
    assert!(saved.starts_with(std::env::temp_dir()) && !saved.starts_with(&dir), "{}", saved.display());
    assert!(saved.is_file());
    assert_eq!(sheets(&r).len(), 2, "both pages: {r}");
    assert_eq!(listing(&dir), before, "nothing is written next to the PDF");
    let _ = saved.parent().map(std::fs::remove_dir_all);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A two-page PDF of loose linework (no title block, no plan, no symbols): 250 short lines of
/// different lengths and directions (page 2 has 10 more, so it is no copy of page 1).
fn loose_linework_pdf(dir: &Path) -> PathBuf {
    let mut pages = Vec::new();
    for page in 0..2u32 {
        let mut s = Session::new();
        for i in 0..250 + page * 10 {
            let (x, y) = (f64::from(i % 25) * 40.0, f64::from(i / 25) * 40.0 + f64::from(page) * 7.0);
            let len = 5.0 + f64::from((i * 7 + page) % 23);
            let dy = f64::from((i * 3) % 11);
            s.execute("line", &json!({"points": [[x, y], [x + len, y + dy]]})).expect("line");
        }
        let opts = cadaddy_io::pdf::PdfOptions { fit: Some(true), ..cadaddy_io::pdf::PdfOptions::default() };
        let pdf = cadaddy_io::pdf::pdf(s.doc().expect("drawing"), &cadaddy_engine::doc::Space::Model, &opts).expect("plot");
        pages.push((pdf, format!("Page {}", page + 1), String::new()));
    }
    let path = dir.join("Loose Lines.pdf");
    std::fs::write(&path, cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write");
    path
}

#[test]
fn a_set_without_plans_imports_every_page_and_a_hard_fail_is_no_pass() {
    io();
    let dir = temp("noplans");
    let pdf = loose_linework_pdf(&dir);
    let before = listing(&dir);
    let out_dir = temp("noplans_out");
    let out = out_dir.join("Sample.cdy");
    let mut s = Session::new();
    let r = s.execute("bim.from_pdf", &json!({"path": pdf.to_string_lossy(), "out": out.to_string_lossy()})).expect("bim.from_pdf");
    // No plans in the set: every page is imported, not just page 1.
    assert_eq!(r["pages"].as_array().map(Vec::len), Some(2), "{r}");
    // The check hard-fails (no symbols, no scale): never a pass, and the build stops.
    let c = &r["check_results"][0];
    assert!(c["hard_fails"].as_array().is_some_and(|h| !h.is_empty()), "{c}");
    assert_eq!(c["pass"], json!(false), "{c}");
    check_is_honest(&r);
    assert_eq!(r["cdy_paths"], json!([]), "{r}");
    assert!(r["warnings"].as_array().is_some_and(|w| w.iter().any(|w| w.as_str().is_some_and(|w| w.contains("import check failed")))), "{r}");
    assert!(!out.exists());
    // force: built anyway, and still reported as failed.
    let r =
        s.execute("bim.from_pdf", &json!({"path": pdf.to_string_lossy(), "out": out.to_string_lossy(), "force": true})).expect("bim.from_pdf force");
    assert_eq!(r["check_results"][0]["pass"], json!(false), "{r}");
    check_is_honest(&r);
    assert_eq!(listing(&dir), before, "nothing is written next to the PDF");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn printset_open_takes_one_pdf_and_only_its_pages() {
    io();
    let (dir, pdf) = prints("one", &["A-101", "A-102"]);
    // Another PDF in the same folder is not part of the set.
    std::fs::copy(&pdf, dir.join("Other Set.pdf")).expect("copy");
    let mut s = Session::new();
    let r = s.execute("printset.open", &json!({"path": pdf.to_string_lossy()})).expect("open one PDF");
    assert_eq!(r["sheets"], json!(2), "{r}");
    let c = s.execute("printset.classify", &json!({})).expect("classify");
    let numbers: BTreeSet<&str> = c["sheets"].as_array().expect("sheets").iter().filter_map(|x| x["number"].as_str()).collect();
    assert!(numbers.contains("A-101") && numbers.contains("A-102"), "{numbers:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
