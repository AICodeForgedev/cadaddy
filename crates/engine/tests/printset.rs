//! Engine-level tests for the print-set project: two-session isolation, full-hotel through
//! the command path, and verification of the unified pipeline (MUST 1 and MUST 6 from readerwire).

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::json;

// ─── Verify MUST 1: printset.run command exists and is reachable ─────────────

#[test]
fn printset_run_command_is_registered() {
    assert!(cadaddy_engine::cmd::find_by_id_or_alias("printset.run").is_some(), "printset.run command should be registered");
    assert!(cadaddy_engine::cmd::find_by_id_or_alias("printset.open").is_some(), "printset.open command should be registered");
    assert!(cadaddy_engine::cmd::find_by_id_or_alias("printset.classify").is_some(), "printset.classify command should be registered");
    assert!(cadaddy_engine::cmd::find_by_id_or_alias("printset.dashboard").is_some(), "printset.dashboard command should be registered");
}

#[test]
fn printset_run_without_project_gives_error() {
    let mut s = Session::new();
    let result = s.execute("printset.run", &json!({}));
    assert!(result.is_err(), "printset.run without a project should error");
    let err = format!("{}", result.unwrap_err());
    assert!(err.contains("printset.open"), "error should mention printset.open: {err}");
}

// ─── Verify MUST 6: two sessions, independent projects ──────────────────────

#[test]
fn two_sessions_have_independent_projects() {
    let set_a = generate_hotel(42, HotelSize::Small).expect("generate hotel A");
    let set_b = generate_hotel(99, HotelSize::Small).expect("generate hotel B");

    let dir_a = std::env::temp_dir().join(format!("cadaddy_ps_sess_a_{}", std::process::id()));
    let dir_b = std::env::temp_dir().join(format!("cadaddy_ps_sess_b_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
    set_a.write_to(&dir_a).expect("write A");
    set_b.write_to(&dir_b).expect("write B");

    let mut s1 = Session::new();
    let mut s2 = Session::new();

    // Open different folders in each session.
    let r1 = s1.execute("printset.open", &json!({"path": dir_a.display().to_string(), "name": "Hotel A"})).expect("open A");
    let r2 = s2.execute("printset.open", &json!({"path": dir_b.display().to_string(), "name": "Hotel B"})).expect("open B");

    assert_eq!(r1["name"], "Hotel A");
    assert_eq!(r2["name"], "Hotel B");

    // Each session's dashboard should reflect its own project.
    let d1 = s1.execute("printset.dashboard", &json!({})).expect("dashboard A");
    let d2 = s2.execute("printset.dashboard", &json!({})).expect("dashboard B");

    assert_eq!(d1["name"], "Hotel A");
    assert_eq!(d2["name"], "Hotel B");
    // They should both have sheets and not share state.
    assert!(d1["sheets"].as_u64().unwrap_or(0) > 0, "session 1 should have sheets");
    assert!(d2["sheets"].as_u64().unwrap_or(0) > 0, "session 2 should have sheets");

    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

// ─── Full hotel through command path (MUST 7) ───────────────────────────────

#[test]
fn full_hotel_through_command_path() {
    let set = generate_hotel(100, HotelSize::Full).expect("generate full hotel");

    let dir = std::env::temp_dir().join(format!("cadaddy_ps_cmd_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    set.write_to(&dir).expect("write fixture set");

    let mut s = Session::new();

    // printset.open
    let open_result = s.execute("printset.open", &json!({"path": dir.display().to_string(), "name": "Full Hotel"})).expect("open");
    assert_eq!(open_result["name"], "Full Hotel");
    let sheet_count = open_result["sheets"].as_u64().unwrap_or(0);
    assert!(sheet_count > 20, "full hotel should have >20 sheets, got {sheet_count}");

    // printset.classify
    let classify_result = s.execute("printset.classify", &json!({})).expect("classify");
    assert_eq!(classify_result["classified"].as_u64().unwrap_or(0), sheet_count, "all sheets should be classified");

    // printset.run (runs all stages; readers are stubs so items=0, but the command should work)
    let run_result = s.execute("printset.run", &json!({})).expect("run");
    assert!(run_result["stages_run"].as_u64().unwrap_or(0) > 0, "should have run stages");

    // printset.dashboard
    let dash = s.execute("printset.dashboard", &json!({})).expect("dashboard");
    assert_eq!(dash["name"], "Full Hotel");
    assert!(dash["sheets"].as_u64().unwrap_or(0) > 0);

    // printset.sheets — verify the listing command works (no discipline filter since
    // the engine classify runs without a DXF loader, so file-only projects don't get disciplines).
    let all_sheets = s.execute("printset.sheets", &json!({})).expect("all sheets");
    assert!(all_sheets["total"].as_u64().unwrap_or(0) > 0, "should list all sheets");

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── Sheet index (Q5262): one entry per real sheet, levels from the plan titles ─────────────

/// A folder like a real job's: the full set as one merged PDF, plus a discipline set that
/// prints the mechanical sheets again.
/// `only`: the sheets the full set holds (all when empty).
fn job_folder(tag: &str, only: &[&str]) -> (cadaddy_mepfix::truth::Truth, std::path::PathBuf) {
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_sheetindex_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(dir.join("Mechanical")).expect("folders");
    if !only.is_empty() {
        set.truth.sheets.retain(|s| only.contains(&s.number.as_str()));
    }
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    for sh in set.truth.sheets.iter().filter(|s| s.discipline == "M") {
        std::fs::write(dir.join("Mechanical").join(&sh.pdf), set.file(&sh.pdf).expect("sheet pdf")).expect("write sheet");
    }
    (set.truth, dir)
}

#[test]
fn classify_gives_one_entry_per_real_sheet_with_its_level() {
    let (truth, dir) = job_folder("classify", &[]);
    let mut s = Session::new();
    s.execute("printset.open", &json!({"path": dir.display().to_string()})).expect("open");
    let r = s.execute("printset.classify", &json!({})).expect("classify");
    let mech = truth.sheets.iter().filter(|s| s.discipline == "M").count();
    assert!(mech > 0);
    assert_eq!(r["pages"].as_u64(), Some((truth.sheets.len() + mech) as u64), "every page read");
    assert_eq!(r["total"].as_u64(), Some(truth.sheets.len() as u64), "the mechanical sheets' second copies are merged");
    let sheets = r["sheets"].as_array().expect("index");
    assert_eq!(sheets.len(), truth.sheets.len());
    for sh in &truth.sheets {
        let e = sheets.iter().find(|e| e["number"] == sh.number.as_str()).unwrap_or_else(|| panic!("{} in the index", sh.number));
        let sources = e["sources"].as_array().map_or(0, Vec::len);
        assert_eq!(sources, if sh.discipline == "M" { 2 } else { 1 }, "{}: {}", sh.number, e["sources"]);
        if sh.title.contains("PLAN")
            && let Some(l) = &sh.level
        {
            assert_eq!(e["level"].as_str(), Some(l.as_str()), "{} level", sh.number);
        }
    }
    let levels = r["levels"].as_array().map_or(0, Vec::len);
    let truth_levels: std::collections::BTreeSet<&String> =
        truth.sheets.iter().filter(|s| s.title.contains("PLAN")).filter_map(|s| s.level.as_ref()).collect();
    assert!(levels >= truth_levels.len(), "levels {} vs {truth_levels:?}", r["levels"]);
    // The stages see each sheet once: the current copies only.
    let current = s.execute("printset.sheets", &json!({"current": true})).expect("sheets");
    assert_eq!(current["total"].as_u64(), Some(truth.sheets.len() as u64));
    assert!(current["sheets"].as_array().is_some_and(|a| a.iter().all(|x| x["page"].as_u64().is_some_and(|p| p >= 1))), "pages are listed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_guided_job_reports_the_index_and_rooms_carry_a_level() {
    // The job imports the plan sheets' pages: the real PDF reader.
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
    let (truth, dir) = job_folder("job", &["G-001", "A-101", "A-102", "A-103", "M-101", "M-102"]);
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    assert_eq!(st["sheets"].as_u64(), Some(truth.sheets.len() as u64), "{st}");
    assert!(st["levels"].as_u64().is_some_and(|n| n >= 2), "{st}");
    let classify = s.execute("job.explain", &json!({"stage": "classify"})).expect("explain classify");
    let index = classify["result"]["sheets"].as_array().expect("job.explain classify lists the sheets");
    assert_eq!(index.len(), truth.sheets.len());
    assert!(index.iter().all(|e| e["sources"].as_array().is_some_and(|a| !a.is_empty())));
    let levels = s.execute("job.explain", &json!({"stage": "levels"})).expect("explain levels");
    assert!(levels["result"]["levels"].as_array().is_some_and(|a| a.len() >= 2), "{}", levels["result"]);
    let rooms = s.execute("job.explain", &json!({"stage": "rooms"})).expect("explain rooms");
    let list = rooms["result"]["rooms"].as_array().cloned().unwrap_or_default();
    assert!(!list.is_empty(), "rooms read");
    let with_level = list.iter().filter(|r| r["level"].as_str().is_some_and(|l| !l.is_empty())).count();
    assert!(with_level > 0, "rooms carry a level: {:?}", list.iter().take(3).collect::<Vec<_>>());
    // Each plan sheet sits in its own rectangle: rooms on different levels' sheets differ.
    let distinct: std::collections::BTreeSet<&str> = list.iter().filter_map(|r| r["level"].as_str()).collect();
    assert!(distinct.len() >= 2, "rooms on several levels: {distinct:?}");
    for stage in ["import", "classify", "levels", "rooms"] {
        let e = s.execute("job.explain", &json!({"stage": stage})).expect("explain");
        assert!(e["error"].is_null(), "{stage}: {}", e["error"]);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_two_view_sheet_is_one_entry_and_its_page_is_imported_once() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
    let (t, st) = cadaddy_mepfix::hotel_planned(7, HotelSize::Small);
    let two = cadaddy_mepfix::printed::two_view_sheet(&t, &st, "E-101", "E-102", "E-1A", "LIGHTING").expect("two-view sheet");
    let opts = cadaddy_io::pdf::PdfOptions { fit: Some(true), ..cadaddy_io::pdf::PdfOptions::default() };
    let pdf = cadaddy_io::pdf::pdf(&two.drawing, &cadaddy_engine::doc::Space::Model, &opts).expect("plot");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_sheetindex_twoview_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    std::fs::write(dir.join("Lighting.pdf"), pdf).expect("write");

    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true})).expect("job.start");
    assert_eq!(st["sheets"].as_u64(), Some(1), "the two views are one sheet: {st}");
    let classify = s.execute("job.explain", &json!({"stage": "classify"})).expect("explain");
    let e = &classify["result"]["sheets"][0];
    assert_eq!(e["number"], "E-1A", "{e}");
    assert_eq!(e["levels"], json!(["1", "2"]), "{e}");
    let agent: serde_json::Value = serde_json::from_str(s.doc().ok().and_then(|d| d.agent.clone()).as_deref().unwrap_or("{}")).expect("job state");
    let regions = agent["guided_job"]["sheet_regions"].as_array().cloned().unwrap_or_default();
    // The page is imported once: one rectangle, split into its two views (Q6030), each level
    // taking only its own plan.
    assert_eq!(regions.len(), 2, "one region per view: {regions:?}");
    assert!(regions.iter().all(|r| r["number"] == "E-1A" && r["rect"] == regions[0]["rect"]), "the page is imported once: {regions:?}");
    assert_eq!(regions.iter().map(|r| r["level"].clone()).collect::<Vec<_>>(), [json!("1"), json!("2")], "{regions:?}");
    assert!(regions.iter().all(|r| r["view"]["rect"].is_array()), "{regions:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Q5271: the devices the guided job's devices stage counts on the E plans are the model's
/// devices (on their sheets' levels) and the BOM's device units.
#[test]
fn the_guided_job_puts_its_devices_in_the_model_and_the_bom() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
    let (_truth, dir) = job_folder("devices", &[]);
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    let devices = s.execute("job.explain", &json!({"stage": "devices"})).expect("explain devices");
    let counted = devices["result"]["devices"]["count"].as_u64().unwrap_or(0);
    eprintln!("devices stage: {:#}", devices["result"]["devices"]["by_type"]);
    assert!(counted > 0, "the generated E plans have tagged devices: {:#}", devices["result"]);
    let built = s.execute("job.explain", &json!({"stage": "bim_build"})).expect("explain bim_build");
    // Every device read is in the model, or left out (it can't be put onto the architecture) with
    // a finding; a typical-room plan's devices are added to each room of its type.
    let dv = &built["result"]["devices"];
    let (inserted, not_placed, stamped) =
        (dv["inserted"].as_u64().unwrap_or(0), dv["not_placed"].as_u64().unwrap_or(0), dv["stamped"].as_u64().unwrap_or(0));
    assert_eq!(inserted + not_placed, counted + stamped, "{dv:#}");
    assert!(not_placed * 20 <= counted, "most devices land on the architecture: {dv:#}");
    if not_placed > 0 {
        let state = s.doc().ok().and_then(|d| d.agent.clone()).unwrap_or_default();
        assert!(state.contains("Devices not placed"), "left out with a finding");
    }
    let next = s.execute("job.next", &json!({})).expect("job.next");
    assert_eq!(next["summary"]["devices"]["count"].as_u64(), Some(inserted), "the model's devices: {:#}", next["summary"]);
    let by_level = next["summary"]["devices"]["by_level"].as_object().cloned().unwrap_or_default();
    eprintln!("devices by level: {by_level:?}");
    assert!(by_level.keys().all(|l| !l.is_empty()), "every device has a level: {by_level:?}");
    let bom = s.execute("bim.bom", &json!({"group_by": "level"})).expect("bom");
    // The electrical devices, and the plumbing plans' fixtures their schedule types (Q6165: the
    // plumbing trade's, not electrical "device type" lines).
    let lines: Vec<&serde_json::Value> =
        bom["lines"].as_array().into_iter().flatten().filter(|l| l["category"] == "devices" || l["category"] == "plumbing fixtures").collect();
    let units: f64 = lines.iter().filter_map(|l| l["net"].as_f64()).sum();
    // Another trade's devices (thermostats, fire alarm, low voltage: bim.scope) leave the device
    // lines for `by_others`, one entry each (Q6167).
    let others = bom["by_others"].as_array().into_iter().flatten().filter(|x| x["trade"] != "concrete").count();
    assert_eq!(units + others as f64, inserted as f64, "the BOM counts the model's devices: {lines:#?} by others {others}");
    assert!(lines.iter().all(|l| l["level"].as_str().is_some_and(|v| !v.is_empty())));
    assert!(lines.iter().all(|l| (l["category"] == "devices") == (l["trade"] == "electrical")), "{lines:#?}");
    assert!(!lines.iter().any(|l| l["item"].as_str().is_some_and(|i| i.starts_with("device type"))), "{lines:#?}");
    let _ = std::fs::remove_dir_all(&dir);
}
