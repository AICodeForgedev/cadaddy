//! Q6329: a print set with no plan view (a part drawing, a set of schedules and a cover) is told at
//! classify and declines there, saying why, before the specification search and the sheet
//! imports (it used to run every stage, 7-26 s, to build nothing); a correction that makes one of
//! its sheets a plan lets the job go on. And another print set beside the prints is not imported
//! page by page to tell whether it is the project manual: a drawing sheet is no book page.
//! Made-up sets only: the generator's own sheets and hand-drawn ones.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Common, Drawing, EntityKind, HAlign, Line, Space, Text, VAlign};
use cadaddy_engine::geom::{Bounds2, Vec2, Vec3};
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

fn folder(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_nonplanset_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    dir
}

/// The made-up generator's sheets `only`, each a one-page PDF.
fn generated(only: &[&str]) -> Vec<(Vec<u8>, String, String)> {
    let (t, st) = cadaddy_mepfix::hotel_planned(7, HotelSize::Small);
    let kinds: Vec<(String, cadaddy_mepfix::layout::Kind)> =
        cadaddy_mepfix::layout::hotel_sheet_list().into_iter().map(|(n, _, _, k)| (n, k)).collect();
    let mut pages = Vec::new();
    for n in only {
        let sh = t.sheets.iter().find(|s| s.number == *n).expect("sheet in the set");
        let (_, kind) = kinds.iter().find(|(k, _)| k == n).expect("sheet kind");
        let (d, _, _) = cadaddy_mepfix::draw::draw(&t, &st, sh, kind);
        let pdf = cadaddy_io::pdf::pdf(&d, &Space::Model, &cadaddy_mepfix::pdf_options(sh, &st)).expect("plot");
        pages.push((pdf, format!("{} {}", sh.number, sh.title), String::new()));
    }
    pages
}

fn text(d: &mut Drawing, at: [f64; 2], h: f64, s: &str) {
    let t = Text {
        insert: Vec3::new(at[0], at[1], 0.0),
        align_pt: None,
        height: h,
        value: s.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    };
    d.add(&Space::Model, Common { layer: "TEXT".into(), ..Common::default() }, EntityKind::Text(t)).expect("text");
}

fn line(d: &mut Drawing, a: [f64; 2], b: [f64; 2]) {
    let l = Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) };
    d.add(&Space::Model, Common { layer: "OBJECT".into(), ..Common::default() }, EntityKind::Line(l)).expect("line");
}

/// One page `w` × `h` inches, drawn in sheet inches.
fn page(d: &Drawing, w: f64, h: f64, title: &str) -> Vec<u8> {
    let o = cadaddy_io::pdf::PdfOptions {
        paper_mm: Some((w * 25.4, h * 25.4)),
        landscape: Some(w > h),
        fit: Some(true),
        window: Some(Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(w, h))),
        margins: Some([0.0; 4]),
        title: title.into(),
        stamp: Some(false),
        ..cadaddy_io::pdf::PdfOptions::default()
    };
    cadaddy_io::pdf::pdf(d, &Space::Model, &o).expect("plot")
}

/// A machined part's drawing on a 17" × 11" sheet: the part's outline and holes, its notes, and
/// a title block naming it (another format than the generator's sheets).
fn part_drawing() -> Vec<u8> {
    let mut d = Drawing::default();
    for (a, b) in [([1.0, 2.0], [9.0, 2.0]), ([9.0, 2.0], [9.0, 7.0]), ([9.0, 7.0], [1.0, 7.0]), ([1.0, 7.0], [1.0, 2.0])] {
        line(&mut d, a, b);
    }
    for x in [2.0, 8.0] {
        for (a, b) in
            [([x - 0.2, 4.3], [x + 0.2, 4.3]), ([x + 0.2, 4.3], [x + 0.2, 4.7]), ([x + 0.2, 4.7], [x - 0.2, 4.7]), ([x - 0.2, 4.7], [x - 0.2, 4.3])]
        {
            line(&mut d, a, b);
        }
    }
    text(&mut d, [1.0, 8.0], 0.15, "NOTES: 1. BREAK ALL SHARP EDGES. 2. FINISH: ZINC PLATE.");
    text(&mut d, [1.0, 1.4], 0.12, "4X 0.40 HOLES THRU");
    // The title block, lower right.
    for (a, b) in [([12.0, 0.5], [16.5, 0.5]), ([16.5, 0.5], [16.5, 3.0]), ([16.5, 3.0], [12.0, 3.0]), ([12.0, 3.0], [12.0, 0.5])] {
        line(&mut d, a, b);
    }
    text(&mut d, [12.2, 2.6], 0.12, "DRAWING TITLE");
    text(&mut d, [12.2, 2.3], 0.14, "SUPPORT PLATE");
    text(&mut d, [12.2, 1.6], 0.12, "SHEET NO.");
    text(&mut d, [12.2, 1.1], 0.3, "M-501");
    page(&d, 17.0, 11.0, "Support plate")
}

/// A page of text, `w` × `h` inches: specification sections' headers and PART 1 lines.
fn spec_page(w: f64, h: f64) -> Vec<u8> {
    let mut d = Drawing::default();
    let lines = [
        "SECTION 26 05 19 - LOW-VOLTAGE ELECTRICAL POWER CONDUCTORS AND CABLES",
        "PART 1 - GENERAL",
        "1.1 SUMMARY",
        "SECTION 26 27 26 - WIRING DEVICES",
        "PART 1 - GENERAL",
    ];
    for (i, l) in lines.iter().enumerate() {
        text(&mut d, [0.8, h - 1.0 - 0.4 * i as f64], 0.12, l);
    }
    page(&d, w, h, "Sections")
}

fn job_state(s: &Session) -> Value {
    let agent: Value = serde_json::from_str(s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).as_deref().unwrap_or("{}")).expect("job state");
    agent["guided_job"].clone()
}

fn stage_status(job: &Value, name: &str) -> String {
    job["stages"]
        .as_array()
        .and_then(|a| a.iter().find(|st| st["name"] == name))
        .map(|st| st["status"].as_str().unwrap_or("").to_string())
        .unwrap_or_default()
}

fn model_objects(s: &Session) -> usize {
    s.doc().map_or(0, |d| d.model.iter().count())
}

fn write(dir: &Path, name: &str, pages: &[(Vec<u8>, String, String)]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, cadaddy_io::pdf_merge::merge(pages).expect("merge")).expect("write");
    p
}

/// The generator's cover and schedules as one merged set: `bim.from_pdf` declines at classify,
/// saying the set has no plan view and naming its sheets; nothing is imported, no specification
/// is searched for, nothing is saved. A set with a plan among the same sheets is built from it.
#[test]
fn a_set_of_schedules_declines_at_classify_with_why() {
    let _g = serial();
    pdf_reader();
    let dir = folder("schedules");
    let prints = dir.join("Prints");
    std::fs::create_dir_all(&prints).expect("prints");
    write(&prints, "Sample Set.pdf", &generated(&["G-001", "M-601", "E-601"]));
    let mut s = Session::new();
    let out = dir.join("out").join("model.cdy");
    let r = s.execute("bim.from_pdf", &json!({"path": prints.to_string_lossy(), "out": out.to_string_lossy()})).expect("bim.from_pdf");
    assert_eq!(r["ok"], json!(false), "{r}");
    let why = r["declined"].as_str().expect("declined, with why");
    assert!(why.contains("has no plan views"), "{why}");
    for sheet in ["M-601", "E-601"] {
        assert!(why.contains(sheet), "{sheet} named in {why}");
    }
    assert!(r["failed"].as_array().is_some_and(|f| f.len() == 1 && f[0].as_str().is_some_and(|m| m.contains("has no plan views"))), "{r}");
    assert!(r["cdy_paths"].as_array().is_some_and(Vec::is_empty) && !out.exists(), "nothing saved: {r}");
    assert_eq!(r["built_counts"], json!({}));
    // Stopped at classify: no sheet imported, no stage after it run, no specification searched.
    let job = job_state(&s);
    assert_eq!(stage_status(&job, "classify"), "error", "{}", job["stages"]);
    assert_eq!(stage_status(&job, "levels"), "pending");
    assert_eq!(job["sheets_loaded"], json!(false));
    assert!(job["specs_found"].is_null(), "the specification isn't searched for: {}", job["specs_found"]);
    assert_eq!(model_objects(&s), 0, "no sheet imported");

    // The same sheets with a floor plan: built from the plan (nothing declined).
    let dir = folder("withplan");
    write(&dir, "Sample Set.pdf", &generated(&["G-001", "A-101", "M-601"]));
    let mut s = Session::new();
    let r = s.execute("job.start", &json!({"pdfs": dir.to_string_lossy(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    let job = job_state(&s);
    assert_ne!(stage_status(&job, "classify"), "error", "{r}");
    assert!(
        !job["findings"].as_array().is_some_and(|f| f.iter().any(|x| x["description"].as_str().is_some_and(|d| d.contains("has no plan views")))),
        "a set with a plan is not declined"
    );
    assert_eq!(job["sheets_loaded"], json!(true));
}

/// A part drawing (another title block, one PDF named on its own) declines the same way; a
/// correction that makes it a plan lets the job go on from it.
#[test]
fn a_part_drawing_declines_and_a_plan_correction_goes_on() {
    let _g = serial();
    pdf_reader();
    let dir = folder("part");
    let pdf = dir.join("Support Plate.pdf");
    std::fs::write(&pdf, part_drawing()).expect("write");
    let mut s = Session::new();
    s.execute("job.start", &json!({"pdfs": pdf.to_string_lossy(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    let job = job_state(&s);
    assert_eq!(stage_status(&job, "classify"), "error", "{}", job["stages"]);
    let finding = job["findings"]
        .as_array()
        .and_then(|f| f.iter().find(|x| x["description"].as_str().is_some_and(|d| d.contains("has no plan views"))))
        .cloned()
        .expect("the no-plan finding");
    assert_eq!(finding["blocking"], json!(true));
    assert!(finding["description"].as_str().is_some_and(|d| d.contains("SUPPORT PLATE")), "{finding}");
    assert!(finding["actions"].to_string().contains("job.sheets.set"), "says how to go on: {finding}");
    let number = job["sheets"]["sheets"][0]["number"].as_str().expect("its number").to_string();

    // It is a plan after all: the job goes on from it.
    s.execute("job.sheets.set", &json!({ number.clone(): {"type": "plan", "level": "1"} })).expect("job.sheets.set");
    let job = job_state(&s);
    assert_eq!(stage_status(&job, "classify"), "done", "{}", job["stages"]);
    assert_ne!(stage_status(&job, "levels"), "pending", "{}", job["stages"]);
    assert_eq!(job["sheets_loaded"], json!(true), "the sheet imported as a plan");
    assert!(
        !job["findings"].as_array().is_some_and(|f| f.iter().any(|x| x["description"].as_str().is_some_and(|d| d.contains("has no plan views")))),
        "the decline is gone"
    );
    assert!(!job["specs_found"].is_null(), "the specification is looked for once it goes on");
}

/// The prints beside another job's print set (drawing sheets carrying section headers) and a
/// book of sections: the book is the specification found; the other set's sheets are no book's
/// pages and aren't read.
#[test]
fn a_print_set_beside_the_prints_is_not_read_as_the_manual() {
    let _g = serial();
    pdf_reader();
    let dir = folder("beside");
    let prints = dir.join("Prints");
    let other = dir.join("Other Set");
    let docs = dir.join("Docs");
    for d in [&prints, &other, &docs] {
        std::fs::create_dir_all(d).expect("folder");
    }
    write(&prints, "Sample Set.pdf", &generated(&["A-101"]));
    std::fs::write(other.join("E-1.pdf"), spec_page(36.0, 24.0)).expect("sheet");
    std::fs::write(other.join("E-2.pdf"), spec_page(17.0, 11.0)).expect("half-size sheet");
    std::fs::write(docs.join("Book.pdf"), spec_page(8.5, 11.0)).expect("book");
    let mut s = Session::new();
    // Not auto: the job stops at classify's questions, after the search.
    s.execute("job.start", &json!({"pdfs": prints.to_string_lossy(), "goal": "bim", "checkpoint": false})).expect("job.start");
    let job = job_state(&s);
    let found: Vec<String> = job["specs_found"].as_array().expect("searched").iter().filter_map(Value::as_str).map(str::to_string).collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].ends_with("Book.pdf"), "{found:?}");
}
