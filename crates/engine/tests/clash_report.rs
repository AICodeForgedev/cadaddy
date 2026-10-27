//! The clash report as PDF (`model.clashes.report`, Q261 rework): grouped, every clash with its
//! id, status, trade and notes, through the plotter like the other PDF exports.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::EntityKind;
use serde_json::{Value, json};

fn plot_hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

/// Two made-up systems crossing over a 9 ft ceiling, and a pipe hanging below it.
fn session() -> Session {
    let mut s = Session::new();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 13, "ceiling_ft": 9, "region": [-1000, -1000, 1000, 1000], "base": [0, 0]}],
            "mep": {
                "systems": [
                    {"name": "M-DUCT-SUPP", "layer": "M-DUCT-SUPP", "kind": "duct", "size_in": [12, 12], "bottom_ft": 10},
                    {"name": "P-DCW", "layer": "P-DCW", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 10}
                ],
                "runs": [
                    {"system": "M-DUCT-SUPP", "level": "L1", "points": [[0, 120], [480, 120]], "size_in": [12, 12], "bottoms_ft": [10]},
                    {"system": "P-DCW", "level": "L1", "points": [[240, 0], [240, 240]], "size_in": [2, 0], "bottoms_ft": [10.3]},
                    {"system": "P-DCW", "level": "L1", "points": [[600, 0], [600, 240]], "size_in": [2, 0], "bottoms_ft": [8]}
                ]
            }
        }}),
    )
    .unwrap();
    s
}

fn texts(d: &cadaddy_engine::doc::Drawing) -> Vec<String> {
    d.model.iter().filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None }).collect()
}

/// 130 triaged clashes in two groups: more than a page holds, some with long notes.
fn many() -> Vec<Value> {
    (0..130)
        .map(|i| {
            let status = ["New", "Active", "Resolved"][i % 3];
            json!({
                "id": format!("C{:04}", i + 1), "kind": if i % 3 == 0 { "column" } else { "crossing" }, "level": "L2",
                "systems": ["M-DUCT-SUPP", "P-DCW"], "message": format!("Duct crosses pipe {i}"),
                "status": status, "trade": if i % 2 == 0 { "Plumbing" } else { "Mechanical" },
                "notes": if i == 7 { "Lower the pipe below the duct and add a sleeve where it passes the beam at the corridor".to_string() } else { format!("note {i}") },
                "group": if i < 40 { "Column C-3 (40 clashes)" } else { "" },
            })
        })
        .collect()
}

/// Every clash id, status, trade and note is on a page; the groups are headed; long notes wrap.
#[test]
fn every_clash_is_in_the_report() {
    let mut s = session();
    let clashes = many();
    let r = cadaddy_engine::cmd::clashreport::report_pdf(&mut s, &json!({"clashes": clashes})).unwrap();
    let t = texts(&r.drawing);
    let pages = r.plot["pages"].as_array().unwrap().len();
    assert!(pages >= 2, "{pages}");
    assert_eq!(t.iter().filter(|x| x.starts_with("CLASH REPORT   130 clashes: 44 new, 43 active, 43 resolved")).count(), pages);
    assert_eq!(t.iter().filter(|x| *x == "Notes").count(), pages, "the header on every page");
    for c in &clashes {
        assert!(t.iter().any(|x| x == c["id"].as_str().unwrap()), "{} missing", c["id"]);
    }
    assert!(t.iter().any(|x| x == "Column C-3 (40 clashes) (40)"), "the palette's group");
    assert!(t.iter().any(|x| x == "Level L2 — Through column (30)") && t.iter().any(|x| x == "Level L2 — Crossing (60)"), "{t:?}");
    for w in ["Active", "Resolved", "Plumbing", "Mechanical", "note 129"] {
        assert!(t.iter().any(|x| x == w), "{w}");
    }
    // The long note wraps onto the lines below, whole.
    let joined = t.join(" ");
    assert!(joined.contains("Lower the pipe below the duct") && joined.contains("at the corridor"), "{joined}");
    assert!(r.cut.is_empty(), "{:?}", r.cut);
    assert_eq!((r.clashes, r.groups), (130, 3));
}

/// Through the command: the PDF is written (pages > 0), and reading it back finds every clash id
/// from the open drawing's own detection; a build without a plotter and a wrong file type are
/// refused with a message.
#[test]
fn report_command_writes_a_pdf_with_every_clash() {
    let mut s = session();
    let dir = std::env::temp_dir().join(format!("cadaddy-clash-report-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Sample Clashes.pdf");
    let e = s.execute("model.clashes.report", &json!({"path": dir.join("x.csv").display().to_string()})).unwrap_err();
    assert!(e.to_string().contains(".pdf"), "{e}");
    plot_hooks();
    let detected = s.execute("model.clashes", &json!({})).unwrap()["total"].as_u64().unwrap();
    assert!(detected >= 2, "{detected}");
    let out = s.execute("model.clashes.report", &json!({"path": path.display().to_string()})).unwrap();
    assert_eq!(out["clashes"].as_u64(), Some(detected));
    assert!(out["pages"].as_u64().unwrap() >= 1);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"%PDF"));
    assert!(cadaddy_io::pdf_read::page_count(&bytes).unwrap() >= 1);
    let (d, _) = cadaddy_io::pdf_read::read_json(&bytes, &json!({"page": 1})).unwrap();
    let words = texts(&d).join(" ");
    for i in 1..=detected {
        assert!(words.contains(&format!("C{i:04}")), "C{i:04} not in the PDF: {words}");
    }
    assert!(words.contains("CLASH REPORT"), "{words}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_report_requests_are_refused_or_capped() {
    let mut s = session();
    for p in [json!({}), json!({"path": ""}), json!({"path": "a.pdf", "clashes": "no"})] {
        assert!(s.execute("model.clashes.report", &p).is_err(), "{p}");
    }
    let odd = json!({"clashes": [5, null, {"id": 3, "notes": "x".repeat(5000), "systems": "M"}, {"notes": "word ".repeat(400)}]});
    let r = cadaddy_engine::cmd::clashreport::report_pdf(&mut s, &odd).unwrap();
    assert_eq!(r.clashes, 4);
    assert!(!r.cut.is_empty(), "the overlong notes are listed in cut");
}
