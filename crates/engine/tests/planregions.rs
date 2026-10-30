//! Plan regions (Q6030): a sheet printing two floor plans (`FIRST FLOOR MECHANICAL PLAN` under
//! one, `SECOND FLOOR MECHANICAL PLAN` under the other) gives each level only its own plan:
//! the duct and pipe each level reads are what that level's plan shows, never the whole sheet's.

use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
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

const SEED: u64 = 42;

fn folder(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_planregions_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    dir
}

/// The made-up hotel's sheets `only`, each its own PDF.
fn write_sheets(dir: &std::path::Path, only: &[&str]) {
    let set = generate_hotel(SEED, HotelSize::Small).expect("generate hotel");
    for sh in set.truth.sheets.iter().filter(|s| only.contains(&s.number.as_str())) {
        std::fs::write(dir.join(format!("{}.pdf", sh.number)), set.file(&sh.pdf).expect("sheet pdf")).expect("write sheet");
    }
}

/// One sheet printing `lower`'s and `upper`'s plans, one above the other, as `number`.
fn write_two_view(dir: &std::path::Path, lower: &str, upper: &str, number: &str, word: &str) {
    write_two_view_as(dir, lower, upper, number, word, true);
}

/// [`write_two_view`], with the views' own titles (`FIRST FLOOR MECHANICAL PLAN`) left off when
/// `titled` is false: only the sheet's title says which levels it shows.
fn write_two_view_as(dir: &std::path::Path, lower: &str, upper: &str, number: &str, word: &str, titled: bool) {
    let (t, st) = cadaddy_mepfix::hotel_planned(SEED, HotelSize::Small);
    let mut two = cadaddy_mepfix::printed::two_view_sheet(&t, &st, lower, upper, number, word).expect("two-view sheet");
    if !titled {
        let mut kept = cadaddy_engine::doc::EntityStore::new();
        kept.push_many(
            two.drawing
                .model
                .iter()
                .filter(|e| !matches!(&e.kind, cadaddy_engine::doc::EntityKind::Text(x) if cadaddy_mepfix::sheet_reader::detect_levels(&x.value).len() == 1))
                .map(|e| (**e).clone())
                .collect::<Vec<_>>(),
        );
        two.drawing.model = kept;
    }
    // Plotted at the plans' printed scale (as a real sheet is), on paper the size of the views.
    let printed = t.sheets.iter().find(|s| s.number == lower).and_then(|s| s.scale_text.clone()).unwrap_or_default();
    let k = cadaddy_mepfix::sheet_reader::parse_scale(&printed).unwrap_or(0.125);
    let ext = two.drawing.extents(&cadaddy_engine::doc::Space::Model);
    let (w, h) = ((ext.max.x - ext.min.x) * k / 12.0 * 25.4, (ext.max.y - ext.min.y) * k / 12.0 * 25.4);
    let opts = cadaddy_io::pdf::PdfOptions {
        paper_mm: Some((w.min(h), w.max(h))),
        landscape: Some(w > h),
        fit: Some(true),
        window: Some(ext),
        margins: Some([0.0; 4]),
        ..cadaddy_io::pdf::PdfOptions::default()
    };
    let pdf = cadaddy_io::pdf::pdf(&two.drawing, &cadaddy_engine::doc::Space::Model, &opts).expect("plot");
    std::fs::write(dir.join(format!("{number}.pdf")), pdf).expect("write two-view sheet");
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str(s.doc().ok().and_then(|d| d.agent.clone()).as_deref().unwrap_or("{}")).expect("job state")
}

fn run_job(dir: &std::path::Path) -> Session {
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    s
}

fn mep_read(s: &Session) -> Value {
    let job = job_state(s);
    job["guided_job"]["stages"].as_array().and_then(|a| a.iter().find(|x| x["name"] == "mep_read")).expect("mep_read stage")["result"].clone()
}

/// Feet of `trade` each level read: `(level, sheet) -> ft`.
fn per_level(read: &Value, trade: &str) -> Vec<(String, String, f64)> {
    let mut out: Vec<(String, String, f64)> = read["sheets"]
        .as_array()
        .expect("sheets")
        .iter()
        .filter(|r| r["trade"] == trade)
        .map(|r| {
            (r["level"].as_str().unwrap_or("").to_string(), r["sheet"].as_str().unwrap_or("").to_string(), r["length_ft"].as_f64().unwrap_or(0.0))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// What the whole sheet `file` reads for `cmd` (`ducts.read`, `pipes.read`): both plans at once,
/// as a level was given before (Q6030's doubling). Ducts as the job takes them: applied, so the
/// traced centrelines are tidied (`ducts.read`'s own `length_ft` is the reader's, before that).
fn whole_sheet(file: &std::path::Path, cmd: &str) -> f64 {
    let mut s = Session::new();
    s.execute("pdfimport", &json!({"path": file.display().to_string(), "page": 1, "images": false})).expect("pdfimport");
    if cmd == "ducts.read" {
        s.execute(cmd, &json!({"apply": true})).expect(cmd);
        let q = s.execute("mep.qty", &json!({})).expect("mep.qty");
        return q["totals"]["duct_length_ft"].as_f64().unwrap_or(0.0);
    }
    let r = s.execute(cmd, &json!({})).expect(cmd);
    r["length_ft"].as_f64().unwrap_or(0.0)
}

#[test]
fn a_sheet_with_two_plans_gives_each_level_only_its_own_plan() {
    let _g = serial();
    real_pdf_reader();
    // Level 1's and level 2's mechanical and plumbing plans, two to a sheet (the made-up
    // hotel's level 1 has more duct than its level 2).
    let two = folder("two");
    write_sheets(&two, &["A-101", "A-102"]);
    write_two_view(&two, "M-101", "M-102", "M-1A", "MECHANICAL");
    write_two_view(&two, "P-101", "P-102", "P-1A", "PLUMBING");
    let mut s = run_job(&two);
    let read = mep_read(&s);
    for (trade, file, cmd) in [("duct", "M-1A.pdf", "ducts.read"), ("pipe", "P-1A.pdf", "pipes.read")] {
        let b = per_level(&read, trade);
        let whole = whole_sheet(&two.join(file), cmd);
        eprintln!("{trade}: per level {b:?}; the whole sheet {whole}");
        let levels: Vec<&str> = b.iter().map(|(l, _, _)| l.as_str()).collect();
        assert_eq!(levels, ["Level 1", "Level 2"], "{trade}: the two-plan sheet is read once per level: {read}");
        let (l1, l2) = (b[0].2, b[1].2);
        assert!(l1 > 10.0 && l2 > 10.0, "{trade}: each level reads its own plan: {b:?}");
        // Each level its own plan: together they are the sheet, read once; neither takes it all.
        assert!((l1 + l2 - whole).abs() < 0.1 * whole, "{trade}: the levels add up to the sheet: {b:?} vs {whole}");
        assert!(l1 < 0.85 * whole && l2 < 0.85 * whole, "{trade}: no level takes the full sheet: {b:?} vs {whole}");
    }
    // Different duct lengths: each level gets its own.
    let duct = per_level(&read, "duct");
    assert!(duct[0].2 > 1.3 * duct[1].2, "level 1's plan has more duct than level 2's: {duct:?}");
    // The model's totals are the plans' (not doubled), put onto the architecture: both levels'
    // views are laid on their own plans; a run that can't be is left out with a finding.
    let q = s.execute("mep.qty", &json!({})).expect("mep.qty");
    let duct: f64 = duct.iter().map(|x| x.2).sum();
    let model = q["totals"]["duct_length_ft"].as_f64().expect("duct");
    assert!(model <= duct + 1.0 && model >= 0.95 * duct, "{} read {duct}", q["totals"]);
    let state = s.doc().ok().and_then(|d| d.agent.clone()).unwrap_or_default();
    assert!(model >= duct - 1.0 || state.contains("MEP items not placed"), "runs left out without a finding: {} read {duct}", q["totals"]);
    // levels.list: each level's entry for the sheet names its own view, and no sheet is on two
    // levels untyped.
    let list = s.execute("levels.list", &json!({})).expect("levels.list");
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for l in list["levels"].as_array().expect("levels") {
        for e in l["sheets"].as_array().expect("sheets") {
            if e["typical"] == json!(false) {
                *seen.entry(e["sheet"].as_str().unwrap_or("").to_string()).or_default() += 1;
            }
        }
    }
    assert!(seen.values().all(|n| *n == 1), "a floor-specific sheet is on one level only: {seen:?}");
    for (level, code) in [("Level 1", "1"), ("Level 2", "2")] {
        let l = list["levels"].as_array().and_then(|a| a.iter().find(|x| x["name"] == level)).unwrap_or_else(|| panic!("{level}: {list}"));
        let e = l["sheets"].as_array().and_then(|a| a.iter().find(|x| x["printed"] == "M-1A")).unwrap_or_else(|| panic!("{level}: {l}"));
        assert_eq!(e["sheet"], json!(format!("M-1A/{code}")), "{e}");
        assert_eq!(e["typical"], json!(false), "{e}");
        assert!(e["view"]["title"].as_str().unwrap_or("").contains("FLOOR"), "{e}");
        assert_eq!(e["view"]["region"].as_array().map(Vec::len), Some(4), "{e}");
    }
    // The job's regions: the sheet is one region per view, each of its own level.
    let job = job_state(&s);
    let views: Vec<&Value> = job["guided_job"]["sheet_regions"].as_array().expect("regions").iter().filter(|r| r["number"] == "M-1A").collect();
    assert_eq!(views.len(), 2, "{views:?}");
    assert!(views.iter().all(|r| r["view"].is_object() && r["levels"].is_null()), "{views:?}");
    let _ = std::fs::remove_dir_all(&two);
}

#[test]
fn an_open_print_set_reads_each_view_on_its_level() {
    let _g = serial();
    real_pdf_reader();
    let dir = folder("project");
    write_two_view(&dir, "P-101", "P-102", "P-1A", "PLUMBING");
    let mut s = Session::new();
    s.execute("printset.open", &json!({"path": dir.display().to_string()})).expect("printset.open");
    s.execute("printset.classify", &json!({})).expect("classify");
    let pipes = s.execute("pipes.read", &json!({})).expect("pipes.read");
    assert_eq!(pipes["source"], "print set", "{pipes}");
    let b = per_level(&pipes, "pipe");
    eprintln!("print set: {b:?}");
    let levels: Vec<&str> = b.iter().map(|(l, _, _)| l.as_str()).collect();
    assert_eq!(levels, ["Level 1", "Level 2"], "{pipes}");
    assert!(b.iter().all(|x| x.2 > 10.0), "{b:?}");
    assert!(b[0].2 < 0.85 * (b[0].2 + b[1].2) && b[1].2 < 0.85 * (b[0].2 + b[1].2), "{b:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_two_plan_sheet_whose_views_carry_no_titles_still_gives_each_level_its_own_plan() {
    let _g = serial();
    real_pdf_reader();
    // The sheet's title names both levels (`MECHANICAL PLAN - FIRST & SECOND FLOORS`) but the
    // plans have no titles under them: its two blocks of linework split it, bottom to top.
    let dir = folder("untitled");
    write_sheets(&dir, &["A-101", "A-102"]);
    write_two_view_as(&dir, "M-101", "M-102", "M-1A", "MECHANICAL", false);
    let s = run_job(&dir);
    let b = per_level(&mep_read(&s), "duct");
    let whole = whole_sheet(&dir.join("M-1A.pdf"), "ducts.read");
    eprintln!("untitled duct: per level {b:?}; the whole sheet {whole}");
    let levels: Vec<&str> = b.iter().map(|(l, _, _)| l.as_str()).collect();
    assert_eq!(levels, ["Level 1", "Level 2"], "{b:?}");
    let (l1, l2) = (b[0].2, b[1].2);
    assert!(l1 > 10.0 && l2 > 10.0, "each level reads its own plan: {b:?}");
    assert!((l1 + l2 - whole).abs() < 0.1 * whole, "the levels add up to the sheet, read once: {b:?} vs {whole}");
    assert!(l1 > 1.3 * l2, "the bottom plan (level 1's, more duct) is level 1's: {b:?}");
    let job = job_state(&s);
    let views: Vec<&Value> = job["guided_job"]["sheet_regions"].as_array().expect("regions").iter().filter(|r| r["number"] == "M-1A").collect();
    assert_eq!(views.len(), 2, "one region per plan: {views:?}");
    assert!(views.iter().all(|r| r["view"].is_object() && r["levels"].is_null()), "{views:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
