//! Job MEP (Q5272): the guided job reads duct on the M plans and pipe on the P plans of the
//! print set, per level, and builds the runs on their levels.

use crate::helper::serial;
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

/// A made-up print set as one merged PDF holding `only`'s sheets.
fn job_folder(tag: &str, only: &[&str]) -> std::path::PathBuf {
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| only.contains(&s.number.as_str()));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_jobmep_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    dir
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str(s.doc().ok().and_then(|d| d.agent.clone()).as_deref().unwrap_or("{}")).expect("job state")
}

/// The set the tests run: two levels of architecture, mechanical and plumbing plans, and an
/// electrical plan (which must add neither ducts nor pipes).
const SET: &[&str] = &["A-101", "A-102", "E-101", "M-101", "M-102", "P-101", "P-102"];

fn run_job(tag: &str, only: &[&str]) -> Session {
    let dir = job_folder(tag, only);
    let mut s = Session::new();
    let mut st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).expect("job.start");
    // A set whose electrical plan prints no device marks (its only letters by symbols are the
    // grids' bubbles, no devices: Q6165) is asked about it even in auto mode: none, it says.
    if st["done"] != json!(true) {
        let next = s.execute("job.next", &json!({})).expect("job.next");
        let q = &next["question"];
        assert!(q["question"].as_str().is_some_and(|t| t.contains("No device symbols counted on the electrical plans")), "{next}");
        st = s.execute("job.answer", &json!({"id": q["id"], "answer": "No devices on these sheets"})).expect("job.answer");
        st = if st["done"] == json!(true) { st } else { s.execute("job.status", &json!({})).expect("status") };
    }
    assert_eq!(st["done"], json!(true), "{st}");
    s
}

fn stage<'a>(job: &'a Value, name: &str) -> &'a Value {
    job["guided_job"]["stages"].as_array().and_then(|a| a.iter().find(|s| s["name"] == name)).unwrap_or_else(|| panic!("stage {name}"))
}

fn spec(s: &mut Session) -> Value {
    s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone()
}

fn sheets_of(read: &Value, trade: &str) -> Vec<String> {
    read["sheets"].as_array().expect("sheets").iter().filter(|r| r["trade"] == trade).map(|r| r["sheet"].as_str().unwrap_or("").to_string()).collect()
}

#[test]
fn the_job_reads_ducts_on_mechanical_plans_and_pipes_on_plumbing_plans() {
    let _g = serial();
    real_pdf_reader();
    let mut s = run_job("trades", SET);
    let job = job_state(&s);
    let read = stage(&job, "mep_read")["result"].clone();
    // Ducts on the M plans only, pipes on the P plans only: never on the A or E sheets.
    assert_eq!(sheets_of(&read, "duct"), ["M-101", "M-102"], "{read}");
    assert_eq!(sheets_of(&read, "pipe"), ["P-101", "P-102"], "{read}");
    for r in read["sheets"].as_array().expect("sheets") {
        assert!(r["runs"].as_u64().unwrap_or(0) > 0 && r["length_ft"].as_f64().unwrap_or(0.0) > 10.0, "every plan reads its trade: {r}");
    }
    let (duct_ft, pipe_ft) = (read["duct_ft"].as_f64().expect("duct_ft"), read["pipe_ft"].as_f64().expect("pipe_ft"));
    assert!(duct_ft > 50.0 && pipe_ft > 100.0, "{read}");
    // The model's quantities are what the plans read, put onto the architecture: a run that can't
    // be placed isn't drawn, and says so.
    let q = s.execute("mep.qty", &json!({})).expect("mep.qty");
    let not_placed = job["guided_job"]["findings"].to_string().contains("MEP items not placed");
    for (model, read) in
        [(q["totals"]["duct_length_ft"].as_f64().expect("duct"), duct_ft), (q["totals"]["pipe_length_ft"].as_f64().expect("pipe"), pipe_ft)]
    {
        assert!(model <= read + 1.0 && model >= 0.95 * read, "{} read {duct_ft} {pipe_ft}", q["totals"]);
        assert!(model >= read - 1.0 || not_placed, "runs left out without a finding: {} read {read}", q["totals"]);
    }
    // Each run from its own sheet: duct runs from M plans, pipe runs from P plans.
    let sp = spec(&mut s);
    let kind_of = |system: &str| {
        sp["mep"]["systems"].as_array().and_then(|a| a.iter().find(|x| x["name"] == system)).map(|x| x["kind"].as_str().unwrap_or("").to_string())
    };
    let runs = sp["mep"]["runs"].as_array().expect("runs");
    assert!(!runs.is_empty());
    for r in runs {
        let sheet = r["source_sheet"].as_str().unwrap_or("");
        let kind = kind_of(r["system"].as_str().unwrap_or("")).unwrap_or_default();
        match kind.as_str() {
            "duct" => assert!(sheet.starts_with("M-"), "{r}"),
            "pipe" => assert!(sheet.starts_with("P-"), "{r}"),
            k => panic!("run of kind {k}: {r}"),
        }
    }
}

#[test]
fn the_runs_are_built_on_their_sheets_levels() {
    let _g = serial();
    real_pdf_reader();
    let mut s = run_job("levels", SET);
    let job = job_state(&s);
    let build = stage(&job, "bim_build")["result"].clone();
    let sp = spec(&mut s);
    let runs = sp["mep"]["runs"].as_array().expect("runs").clone();
    assert_eq!(build["mepRuns"].as_u64(), Some(runs.len() as u64), "every run read is built: {build}");
    // Each run on its sheet's level (101: level 1, 102: level 2), and that level exists.
    let levels: Vec<String> = sp["levels"].as_array().expect("levels").iter().map(|l| l["name"].as_str().unwrap_or("").to_string()).collect();
    for r in &runs {
        let sheet = r["source_sheet"].as_str().unwrap_or("");
        let want = if sheet.ends_with("01") { "Level 1" } else { "Level 2" };
        assert_eq!(r["level"], json!(want), "{r}");
        assert!(levels.iter().any(|l| l == want), "{levels:?}");
    }
    assert!(runs.iter().any(|r| r["level"] == "Level 2"));
    // Registered onto the level's architectural floor plan: every point inside its rectangle.
    let regions = job["guided_job"]["sheet_regions"].as_array().expect("regions").clone();
    let arch_rect = |level: &str| {
        let code = level.trim_start_matches("Level ");
        let r = regions.iter().find(|g| g["level"] == code && g["number"].as_str().unwrap_or("").starts_with("A-")).expect("arch plan");
        [0, 1, 2, 3].map(|i| r["rect"][i].as_f64().expect("rect"))
    };
    for r in &runs {
        let [x0, y0, x1, y1] = arch_rect(r["level"].as_str().unwrap_or(""));
        for p in r["points"].as_array().expect("points") {
            let (x, y) = (p[0].as_f64().expect("x"), p[1].as_f64().expect("y"));
            assert!(x >= x0 && x <= x1 && y >= y0 && y <= y1, "{} point {p} outside its level's plan", r["source_sheet"]);
        }
    }
    // The MEP plans add no walls (and no levels that build the plans again): the walls are
    // the architectural plans' alone.
    let plain = run_job("levels_arch", &["A-101", "A-102", "E-101"]);
    let plain_build = stage(&job_state(&plain), "bim_build")["result"].clone();
    assert_eq!(build["walls"], plain_build["walls"], "MEP sheets never add walls");
    assert_eq!(build["levels"], plain_build["levels"], "{build}");
    // B4-022: the stage's counts are what the model holds, as bim.tree counts them.
    let mut plain = plain;
    for (key, category) in [("walls", "Walls"), ("doors", "Doors"), ("windows", "Windows")] {
        let tree = plain.execute("bim.tree", &json!({"category": category, "limit": 1})).expect("tree");
        assert_eq!(plain_build[key], tree["total"], "{key}: {plain_build}");
    }
}

#[test]
fn the_readers_on_a_job_drawing_read_only_their_trade() {
    let _g = serial();
    real_pdf_reader();
    let mut s = run_job("readers", SET);
    let ducts = s.execute("ducts.read", &json!({})).expect("ducts.read");
    assert_eq!(ducts["source"], "job drawing", "{ducts}");
    assert_eq!(sheets_of(&ducts, "duct"), ["M-101", "M-102"], "{ducts}");
    assert!(ducts["runs"].as_u64().unwrap_or(0) > 0);
    let pipes = s.execute("pipes.read", &json!({})).expect("pipes.read");
    assert_eq!(sheets_of(&pipes, "pipe"), ["P-101", "P-102"], "{pipes}");
    assert_eq!(pipes["duct_runs"], json!(0));
    // One level.
    let two = s.execute("pipes.read", &json!({"level": "Level 2"})).expect("pipes.read level");
    assert_eq!(sheets_of(&two, "pipe"), ["P-102"], "{two}");
    // Applying again replaces what those sheets read before: nothing counts twice.
    s.execute("pipes.read", &json!({"apply": true})).expect("apply pipes");
    s.execute("ducts.read", &json!({"apply": true})).expect("apply ducts");
    let before = s.execute("mep.qty", &json!({})).expect("qty")["totals"].clone();
    s.execute("pipes.read", &json!({"apply": true})).expect("apply pipes");
    s.execute("ducts.read", &json!({"apply": true})).expect("apply ducts");
    let after = s.execute("mep.qty", &json!({})).expect("qty")["totals"].clone();
    assert_eq!(before["pipe_length_ft"], after["pipe_length_ft"], "{before} {after}");
    assert_eq!(before["duct_length_ft"], after["duct_length_ft"], "{before} {after}");
}

#[test]
fn another_trades_sheet_reads_no_ducts_or_pipes() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("etrade", &["E-101", "M-101", "P-101"]);
    let pdf = dir.join("Full Set.pdf").display().to_string();
    let page = |s: &mut Session, n: u64| {
        s.execute("pdfimport", &json!({"path": pdf, "page": n, "images": false})).expect("pdfimport");
    };
    // The electrical plan (page 3 of M, P, E): neither ducts nor pipes, and it says why.
    let mut s = Session::new();
    page(&mut s, 3);
    for cmd in ["ducts.read", "pipes.read"] {
        let r = s.execute(cmd, &json!({"apply": true})).expect(cmd);
        assert_eq!(r["runs"], json!(0), "{cmd}: {r}");
        assert_eq!(r["applied"], json!(false));
        assert!(r["warnings"][0].as_str().unwrap_or("").contains("E-101"), "{cmd}: {r}");
    }
    // Each trade's own sheet still reads.
    let mut m = Session::new();
    page(&mut m, 1);
    assert!(m.execute("ducts.read", &json!({})).expect("ducts")["runs"].as_u64().unwrap_or(0) > 0);
    assert_eq!(m.execute("pipes.read", &json!({})).expect("pipes")["runs"], json!(0), "no pipes off the mechanical plan");
    let mut p = Session::new();
    page(&mut p, 2);
    let pipes = p.execute("pipes.read", &json!({"apply": true})).expect("pipes");
    assert!(pipes["runs"].as_u64().unwrap_or(0) > 0 && pipes["length_ft"].as_f64().unwrap_or(0.0) > 10.0, "{pipes}");
    assert_eq!(pipes["applied"], json!(true));
    let q = p.execute("mep.qty", &json!({})).expect("qty");
    assert!(q["totals"]["pipe_length_ft"].as_f64().unwrap_or(0.0) > 10.0, "{q}");
}

#[test]
fn an_open_print_set_reads_its_plumbing_plans() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("project", &["A-101", "E-101", "P-101", "P-102"]);
    let mut s = Session::new();
    s.execute("printset.open", &json!({"path": dir.display().to_string()})).expect("printset.open");
    s.execute("printset.classify", &json!({})).expect("classify");
    let pipes = s.execute("pipes.read", &json!({})).expect("pipes.read");
    assert_eq!(pipes["source"], "print set", "{pipes}");
    assert_eq!(sheets_of(&pipes, "pipe"), ["P-101", "P-102"], "{pipes}");
    assert!(pipes["pipe_ft"].as_f64().unwrap_or(0.0) > 100.0, "{pipes}");
    // No mechanical plans in this set: a warning, not an error.
    let ducts = s.execute("ducts.read", &json!({})).expect("ducts.read");
    assert_eq!(ducts["runs"], json!(0));
    assert!(ducts["warnings"].as_array().is_some_and(|w| !w.is_empty()), "{ducts}");
}

#[test]
fn a_reader_with_nothing_to_read_warns_and_never_errors() {
    let mut s = Session::new();
    for cmd in ["conduit.read", "pipes.read", "ducts.read", "gear.read", "mep.equipment.read"] {
        let r = s.execute(cmd, &json!({"apply": true})).unwrap_or_else(|e| panic!("{cmd} errored on an empty drawing: {e}"));
        assert!(r["warnings"].as_array().is_some_and(|w| !w.is_empty()), "{cmd}: {r}");
    }
}

/// The whole made-up hotel's duct and pipe (run by hand: `--ignored --nocapture`, release).
#[test]
#[ignore]
fn measure_a_full_set() {
    let _g = serial();
    real_pdf_reader();
    let set = generate_hotel(42, HotelSize::Full).expect("hotel");
    let only: Vec<&str> = set.truth.sheets.iter().map(|s| s.number.as_str()).collect();
    let mut s = run_job("full", &only);
    let job = job_state(&s);
    let read = stage(&job, "mep_read")["result"].clone();
    for r in read["sheets"].as_array().expect("sheets") {
        eprintln!("{} {} {} runs {} ft {}", r["sheet"], r["level"], r["trade"], r["runs"], r["length_ft"]);
    }
    let q = s.execute("mep.qty", &json!({})).expect("qty");
    eprintln!(
        "duct_ft {} pipe_ft {} built {}",
        q["totals"]["duct_length_ft"],
        q["totals"]["pipe_length_ft"],
        stage(&job, "bim_build")["result"]["mepRuns"]
    );
}
