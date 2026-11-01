//! `cadaddy-cli bim SET.pdf --out X.cdy` (Q5275): every page of a several-page PDF is read, the
//! model is saved at --out only, and nothing is written next to the PDF.

use std::process::Command;

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// A two-page PDF of loose linework (page 2 has more lines, so it is no copy of page 1).
fn two_page_pdf(path: &std::path::Path) {
    let mut pages = Vec::new();
    for page in 0..2u32 {
        let mut s = Session::new();
        for i in 0..250 + page * 10 {
            let (x, y) = (f64::from(i % 25) * 40.0, f64::from(i / 25) * 40.0);
            let len = 5.0 + f64::from((i * 7 + page) % 23);
            s.execute("line", &json!({"points": [[x, y], [x + len, y + f64::from((i * 3) % 11)]]})).unwrap();
        }
        let opts = cadaddy_io::pdf::PdfOptions { fit: Some(true), ..cadaddy_io::pdf::PdfOptions::default() };
        pages.push((
            cadaddy_io::pdf::pdf(s.doc().unwrap(), &cadaddy_engine::doc::Space::Model, &opts).unwrap(),
            format!("Page {}", page + 1),
            String::new(),
        ));
    }
    std::fs::write(path, cadaddy_io::pdf_merge::merge(&pages).unwrap()).unwrap();
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    v.sort();
    v
}

#[test]
fn bim_reads_every_page_and_writes_only_to_out() {
    let root = std::env::temp_dir().join(format!("cc-cli-bimpdf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (prints, work) = (root.join("prints"), root.join("work"));
    std::fs::create_dir_all(&prints).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let pdf = prints.join("Sample Set.pdf");
    two_page_pdf(&pdf);
    let out = work.join("Sample Model.cdy");
    let run = Command::new(env!("CARGO_BIN_EXE_cadaddy-cli"))
        .args(["bim", &pdf.to_string_lossy(), "--out", &out.to_string_lossy(), "--force"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    let r: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(r["pages"].as_array().map(Vec::len), Some(2), "both pages, not page 1 only: {r}");
    // A hard-failed check (loose lines: no symbols) is reported as a fail even when forced, and a
    // failed run exits non-zero even with --force (Q6077); what it built, --force saves at --out.
    assert_eq!(r["check_results"][0]["pass"], json!(false), "{r}");
    assert!(r["failed"].as_array().is_some_and(|f| !f.is_empty()), "{r}");
    assert!(!run.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&run.stderr));
    if r["built"].as_object().is_some_and(|b| !b.is_empty()) {
        assert_eq!(r["cdy_paths"], json!([out.to_string_lossy()]));
        assert!(out.is_file());
    }
    assert_eq!(names(&prints), ["Sample Set.pdf"], "nothing next to the PDF");
    let _ = std::fs::remove_dir_all(&root);
}
