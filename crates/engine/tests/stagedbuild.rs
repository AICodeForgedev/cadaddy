//! Staged build (Q5258): `bim.from_pdf` builds a print set in eight stages — shell, walls,
//! openings, framing, rooms, mep, drywall, finishes — lines every trade plan up on its level's
//! architectural plan (its background removed before the trade readers run), and keeps each
//! stage's checkpoint in the .cdy.

use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

const STAGES: [&str; 8] = ["shell", "walls", "openings", "framing", "rooms", "mep", "drywall", "finishes"];

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

/// A made-up print set as one merged PDF holding `only`'s sheets (every sheet when empty).
fn job_folder(tag: &str, only: &[&str]) -> std::path::PathBuf {
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| only.is_empty() || only.contains(&s.number.as_str()));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_staged_{tag}_{}_{nanos}", std::process::id()));
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

fn bim(s: &mut Session, dir: &std::path::Path) -> Value {
    let out = dir.join("out").join("Model.cdy");
    s.execute("bim.from_pdf", &json!({"path": dir.display().to_string(), "out": out.display().to_string(), "force": true})).expect("bim.from_pdf")
}

fn stage<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["stages"].as_array().and_then(|a| a.iter().find(|s| s["name"] == name)).unwrap_or_else(|| panic!("stage {name}: {}", r["stages"]))
}

fn count(r: &Value, name: &str, key: &str) -> f64 {
    stage(r, name)["counts"][key].as_f64().unwrap_or_else(|| panic!("{name}.{key}: {}", stage(r, name)))
}

/// Two levels of architecture with the electrical, mechanical and plumbing plans over them.
const SET: &[&str] = &["A-101", "A-102", "E-101", "M-101", "M-102", "P-101", "P-102"];

#[test]
fn bim_from_pdf_builds_in_eight_stages_in_order() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("eight", SET);
    let mut s = Session::new();
    let r = bim(&mut s, &dir);
    let names: Vec<&str> = r["stages"].as_array().expect("stages").iter().filter_map(|s| s["name"].as_str()).collect();
    assert_eq!(names, STAGES, "{}", r["stages"]);
    for st in r["stages"].as_array().expect("stages") {
        assert_eq!(st["status"], "done", "{st}");
    }
    let staged = &r["staged"];
    assert_eq!(staged["ok"], json!(true), "{staged}");
    assert_eq!(staged["stages_8ok"], json!(true));
    assert_eq!(staged["stages_done"], json!(8));
    assert!(staged["levels"].as_u64().unwrap_or(0) >= 2, "{staged}");
    // Each stage holds its own elements: shell and walls split the walls, framing frames them.
    let walls = count(&r, "walls", "walls");
    assert!(walls > 0.0);
    assert_eq!(count(&r, "shell", "exterior_walls") + count(&r, "walls", "interior_walls"), walls);
    assert_eq!(count(&r, "framing", "walls_framed"), walls, "every wall framed");
    assert!(count(&r, "framing", "studs") > walls * 2.0, "end studs and field studs at least");
    assert!(count(&r, "rooms", "rooms") > 0.0);
    assert!(count(&r, "drywall", "drywall_sf") > 0.0);
    assert_eq!(staged["stage_studs"].as_f64(), Some(count(&r, "framing", "studs")));
    assert_eq!(staged["stage_drywall_sf"].as_f64(), Some(count(&r, "drywall", "drywall_sf")));
    assert_eq!(staged["stage_rooms"].as_f64(), Some(count(&r, "rooms", "rooms")));
    // Every trade plan lined up on its level's architectural plan.
    let sheets = r["sheets"].as_array().expect("sheets");
    let mut seen: Vec<&str> = sheets.iter().filter_map(|x| x["sheet"].as_str()).collect();
    seen.sort_unstable();
    assert_eq!(seen, ["E-101", "M-101", "M-102", "P-101", "P-102"], "{}", r["sheets"]);
    for x in sheets {
        assert_eq!(x["registered"], json!(true), "{x}");
        assert!(x["matched_pct"].as_f64().unwrap_or(0.0) >= 0.6, "{x}");
        assert!(x["background_lines"].as_u64().unwrap_or(0) > 0, "its background was removed: {x}");
        let arch = x["arch_sheet"].as_str().unwrap_or("");
        let level = x["level"].as_str().unwrap_or("");
        assert_eq!(arch.ends_with("01"), level == "Level 1", "{x}");
    }
    assert_eq!(staged["mep_sheets"], json!(5));
    assert_eq!(staged["registered"], json!(5));
    assert!(count(&r, "mep", "duct_ft") + count(&r, "mep", "pipe_ft") > 0.0, "{}", stage(&r, "mep"));
    assert!(r["questions"].as_array().is_some_and(|q| q.iter().all(|x| x["stage"] != "mep")), "{}", r["questions"]);
}

#[test]
fn the_trade_plans_add_no_walls() {
    let _g = serial();
    real_pdf_reader();
    let mut s = Session::new();
    let whole = bim(&mut s, &job_folder("whole", SET));
    let mut a = Session::new();
    let arch = bim(&mut a, &job_folder("arch", &["A-101", "A-102"]));
    assert_eq!(count(&whole, "walls", "walls"), count(&arch, "walls", "walls"), "walls unchanged by adding the E/M/P sheets");
    assert_eq!(count(&whole, "walls", "walls_from_other_sheets"), 0.0);
    assert_eq!(whole["staged"]["wall_ratio"].as_f64(), Some(1.0));
    assert_eq!(whole["staged"]["arch_walls"], arch["staged"]["arch_walls"]);
}

#[test]
fn each_stage_is_a_checkpoint_to_view_and_roll_back() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("checkpoints", &["A-101", "M-101", "P-101"]);
    let mut s = Session::new();
    let r = bim(&mut s, &dir);
    let elements = |name: &str| stage(&r, name)["elements"].as_u64().unwrap_or(0);
    // Kept in the .cdy.
    let path = r["cdy_paths"][0].as_str().expect("saved");
    let d = cadaddy_io::read(&std::fs::read(path).expect("read cdy"), "Model.cdy").expect("open cdy");
    let agent: Value = serde_json::from_str(d.agent.as_deref().expect("agent")).expect("agent json");
    let kept = agent["staged_build"]["stages"].as_array().expect("staged_build");
    assert_eq!(kept.iter().filter_map(|x| x["name"].as_str()).collect::<Vec<_>>(), STAGES);
    for x in kept {
        assert_eq!(x["handles"].as_array().map(Vec::len).unwrap_or(0) as u64, elements(x["name"].as_str().unwrap_or("")), "{}", x["name"]);
    }
    // The model as it stood after the walls: the later stages hidden.
    let v = s.execute("job.stage_view", &json!({"stage": "walls"})).expect("stage_view");
    let later: u64 = STAGES[2..].iter().map(|n| elements(n)).sum();
    assert_eq!(v["shown"].as_u64(), Some(elements("shell") + elements("walls")), "{v}");
    assert_eq!(v["hidden"].as_u64(), Some(later), "{v}");
    let all = s.execute("job.stage_view", &json!({"stage": "all"})).expect("all");
    assert_eq!(all["hidden"], json!(0));
    assert!(s.execute("job.stage_view", &json!({"stage": "basement"})).is_err());
    // Back to the rooms: mep, drywall and finishes are erased, the rest stays.
    let before = s.doc().expect("doc").model.len() as u64;
    let rb = s.execute("job.rollback", &json!({"stage": "rooms"})).expect("rollback");
    let gone = elements("mep") + elements("drywall") + elements("finishes");
    assert_eq!(rb["removed"].as_u64(), Some(gone), "{rb}");
    assert_eq!(s.doc().expect("doc").model.len() as u64, before - gone);
    let st = rb["stages"].as_array().expect("stages");
    for (i, x) in st.iter().enumerate() {
        assert_eq!(x["status"], if i > 4 { "rolled_back" } else { "done" }, "{x}");
    }
    let parts: Vec<String> = s.doc().expect("doc").model.iter().filter_map(|e| cadaddy_engine::cmd::wallgen::made_as(e)).collect();
    assert!(parts.iter().any(|p| p == "framing_member") && parts.iter().any(|p| p == "wall"));
    assert!(!parts.iter().any(|p| p == "ceiling" || p == "mep"), "{parts:?}");
    assert!(s.execute("job.rollback", &json!({})).is_err(), "a stage is required");
}

#[test]
fn a_trade_plan_with_no_architectural_plan_to_line_up_on_is_a_question() {
    let _g = serial();
    real_pdf_reader();
    // Level 2's mechanical plan without level 2's architectural plan.
    let dir = job_folder("question", &["A-101", "M-101", "M-102"]);
    let mut s = Session::new();
    let r = bim(&mut s, &dir);
    let m2 = r["sheets"].as_array().and_then(|a| a.iter().find(|x| x["sheet"] == "M-102")).cloned().expect("M-102");
    assert_eq!(m2["registered"], json!(false), "{m2}");
    let q = r["questions"].as_array().expect("questions");
    assert!(q.iter().any(|x| x["stage"] == "mep" && x["sheet"] == "M-102"), "{}", r["questions"]);
    assert!(
        r["warnings"].as_array().is_some_and(|w| w.iter().any(|x| x.as_str().is_some_and(|t| t.contains("M-102") && t.contains("not lined up")))),
        "{}",
        r["warnings"]
    );
    assert_eq!(r["staged"]["registered"], json!(1));
}

#[test]
fn the_guided_job_reports_the_eight_build_stages() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("guided", &["A-101", "M-101", "P-101"]);
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    let job: Value = serde_json::from_str(s.doc().ok().and_then(|d| d.agent.clone()).as_deref().unwrap_or("{}")).expect("job state");
    let build =
        job["guided_job"]["stages"].as_array().and_then(|a| a.iter().find(|x| x["name"] == "bim_build")).expect("bim_build")["result"].clone();
    let names: Vec<&str> = build["build_stages"].as_array().expect("build_stages").iter().filter_map(|x| x["name"].as_str()).collect();
    assert_eq!(names, STAGES, "{build}");
    assert_eq!(build["staged"]["stages_8ok"], json!(true));
    assert!(job["staged_build"]["stages"].as_array().is_some_and(|a| a.len() == 8), "the checkpoints are kept");
}

#[test]
fn hostile_stage_calls_refuse_clearly() {
    let mut s = Session::new();
    for (cmd, p) in [
        ("job.stage_view", json!({})),
        ("job.stage_view", json!({"stage": "walls"})),
        ("job.rollback", json!({"stage": "walls"})),
        ("job.rollback", json!({"stage": 5})),
        ("job.rollback", json!({"stage": "\u{0}".repeat(10_000)})),
    ] {
        let e = s.execute(cmd, &p);
        assert!(e.is_err(), "{cmd} {p}: {e:?}");
    }
}

/// A set whose only architectural floor plan is level 1, the trades' plans on four levels: the
/// floor titles on the trade sheets are never banded across the whole drawing into levels (a
/// band takes in every sheet side by side, so walls came off the trades' backgrounds). The walls
/// are level 1's, each on the A sheet, and the walls stage says which sheet they came from.
#[test]
fn one_architectural_plan_never_takes_walls_from_the_trade_sheets() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("onearch", &["A-101", "M-101", "M-102", "M-103", "M-104", "P-101", "P-102", "P-103", "P-104"]);
    let mut s = Session::new();
    let r = bim(&mut s, &dir);
    let staged = &r["staged"];
    assert_eq!(staged["ok"], json!(true), "{staged} {}", r["warnings"]);
    assert!(count(&r, "walls", "walls") > 0.0, "{}", stage(&r, "walls"));
    assert_eq!(count(&r, "walls", "walls_from_other_sheets"), 0.0, "{}", stage(&r, "walls"));
    assert_eq!(staged["wall_ratio"], json!(1.0), "{staged}");
    let notes = stage(&r, "walls")["notes"].to_string();
    assert!(notes.contains("A-101") && !notes.contains("M-10") && !notes.contains("P-10"), "{notes}");
    // The shell names the sheet each level's plan is read off: A-101, an architectural sheet.
    let shell = stage(&r, "shell")["notes"].to_string();
    assert!(shell.contains("level plans: ") && shell.contains("on A-101 (A, "), "{shell}");
    assert!(!shell.contains("(not A, "), "{shell}");
    // Each level's region is its own A plan's (none for the levels without one), never a band
    // across the sheets.
    let spec: Value = serde_json::from_str(s.doc().expect("doc").building_spec.as_deref().unwrap_or("{}")).expect("spec");
    let regions: Vec<[f64; 4]> = spec["levels"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|l| serde_json::from_value(l["region"].clone()).ok())
        .filter(|r: &[f64; 4]| r[0] > -1.0e8)
        .collect();
    assert_eq!(regions.len(), 1, "{}", spec["levels"]);
}

/// Measured by hand: the staged build of the whole made-up hotel (`cargo test --release --test
/// stagedbuild -- --ignored --nocapture`).
#[test]
#[ignore]
fn measure_a_whole_set() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("measure", &[]);
    let mut s = Session::new();
    let t = std::time::Instant::now();
    let r = bim(&mut s, &dir);
    eprintln!("elapsed {:?}", t.elapsed());
    eprintln!("staged {}", serde_json::to_string_pretty(&r["staged"]).unwrap_or_default());
    for st in r["stages"].as_array().map(Vec::as_slice).unwrap_or_default() {
        eprintln!("{} {} {}", st["name"], st["status"], st["counts"]);
    }
}
