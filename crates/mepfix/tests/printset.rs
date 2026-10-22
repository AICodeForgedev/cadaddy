//! Tests for the print-set project module: creating projects from truth data, running the full
//! pipeline, checking the dashboard, and testing re-import with delta detection.

use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::printset::{self, Stage, StageStatus};
use std::sync::atomic::AtomicBool;

/// Generate a hotel fixture set and return its truth (with sheets populated).
fn hotel_truth_with_sheets(seed: u64) -> cadaddy_mepfix::truth::Truth {
    generate_hotel(seed, HotelSize::Small).expect("generate hotel").truth
}

// ─── Project creation from truth ─────────────────────────────────────────────

#[test]
fn create_project_from_hotel_truth() {
    let truth = hotel_truth_with_sheets(42);
    let project = printset::create_from_truth(&truth);
    assert_eq!(project.schema, "cadaddy-printset/1");
    assert!(!project.sheets.is_empty(), "project should have sheets");
    assert_eq!(project.dashboard.total_sheets, project.sheets.len());
    assert!(project.sheets.len() >= 20, "hotel should have at least 20 sheets, got {}", project.sheets.len());
}

#[test]
fn sheets_are_classified_from_truth() {
    let truth = hotel_truth_with_sheets(42);
    let project = printset::create_from_truth(&truth);

    // Check that disciplines are correctly assigned.
    let disciplines: Vec<&str> = project.sheets.iter().map(|s| s.discipline.as_str()).collect();
    assert!(disciplines.contains(&"M"), "should have mechanical sheets");
    assert!(disciplines.contains(&"P"), "should have plumbing sheets");
    assert!(disciplines.contains(&"E"), "should have electrical sheets");
    assert!(disciplines.contains(&"A"), "should have architectural sheets");
    assert!(disciplines.contains(&"S"), "should have structural sheets");

    // Check that levels are assigned to plan sheets.
    let plan_sheets: Vec<&printset::SheetEntry> = project.sheets.iter().filter(|s| s.sheet_type == "plan" || s.sheet_type == "rcp").collect();
    assert!(!plan_sheets.is_empty(), "should have plan sheets");
    for sh in &plan_sheets {
        assert!(!sh.level.is_empty(), "plan sheet {} should have a level", sh.number);
    }

    // Check that sheet types are assigned.
    let types: Vec<&str> = project.sheets.iter().map(|s| s.sheet_type.as_str()).collect();
    assert!(types.contains(&"plan") || types.contains(&"rcp"), "should have plan or rcp sheets");
    assert!(types.contains(&"schedule"), "should have schedule sheets");
    assert!(types.contains(&"cover"), "should have a cover sheet");
}

// ─── Classification stage ────────────────────────────────────────────────────

#[test]
fn classify_stage_completes() {
    let truth = hotel_truth_with_sheets(42);
    let mut project = printset::create_from_truth(&truth);
    let cancel = AtomicBool::new(false);
    let mut progress_count = 0usize;
    let rec = printset::run_classify(&mut project, None, &cancel, &mut |_| {
        progress_count += 1;
    });

    assert_eq!(rec.status, StageStatus::Completed);
    assert_eq!(rec.sheets_processed, rec.sheets_total);
    assert!(progress_count > 0, "progress should have been called");
    assert_eq!(project.dashboard.classified_sheets, project.sheets.len());
}

#[test]
fn classify_stage_cancels() {
    let truth = hotel_truth_with_sheets(42);
    let mut project = printset::create_from_truth(&truth);
    let cancel = AtomicBool::new(true); // Cancel immediately.
    let rec = printset::run_classify(&mut project, None, &cancel, &mut |_| {});
    // Should have processed zero or very few sheets.
    assert!(rec.sheets_processed < rec.sheets_total, "should have cancelled early");
}

// ─── Full pipeline on truth ──────────────────────────────────────────────────

#[test]
fn full_pipeline_on_hotel_truth() {
    let truth = hotel_truth_with_sheets(42);
    let mut project = printset::create_from_truth(&truth);
    let cancel = AtomicBool::new(false);
    let mut last_stage = String::new();
    let records = printset::run_all_from_truth(&mut project, &truth, &cancel, &mut |p| {
        last_stage = p.stage.clone();
    });

    // All stages should have run.
    assert_eq!(records.len(), Stage::all().len(), "should have run all {} stages", Stage::all().len());

    // The classify stage should be completed.
    assert_eq!(records[0].stage, "classify");
    assert_eq!(records[0].status, StageStatus::Completed);

    // Dashboard should reflect the pipeline results.
    let d = &project.dashboard;
    assert!(d.total_items > 0, "should have found items");
    assert!(d.mean_confidence > 0.0, "mean confidence should be positive");
    assert!(d.stages_completed > 0, "should have completed stages");
    assert!(!d.levels.is_empty(), "should have level summaries");
    assert!(!d.disciplines.is_empty(), "should have discipline summaries");

    // Check that we found items in the duct, piping, and electrical stages.
    let duct_rec = records.iter().find(|r| r.stage == "ducts").expect("should have ducts stage");
    assert!(duct_rec.items_found > 0, "should have found duct items");

    let pipe_rec = records.iter().find(|r| r.stage == "piping").expect("should have piping stage");
    assert!(pipe_rec.items_found > 0, "should have found piping items");

    let elec_rec = records.iter().find(|r| r.stage == "electrical").expect("should have electrical stage");
    assert!(elec_rec.items_found > 0, "should have found electrical items");
}

// ─── Dashboard content ───────────────────────────────────────────────────────

#[test]
fn dashboard_reports_gaps_and_rfis() {
    let truth = hotel_truth_with_sheets(42);
    let mut project = printset::create_from_truth(&truth);
    let cancel = AtomicBool::new(false);
    let _ = printset::run_all_from_truth(&mut project, &truth, &cancel, &mut |_| {});

    let d = &project.dashboard;
    // The truth has clashes, so the dashboard should report them.
    if !truth.clashes.is_empty() {
        assert!(d.total_clashes > 0, "dashboard should report clashes");
    }
    // The pipeline generates findings (guessed heights, conflicts), so gaps/rfis should exist.
    assert!(d.open_gaps > 0 || d.open_rfis > 0, "dashboard should have gaps ({}) or rfis ({})", d.open_gaps, d.open_rfis,);
}

#[test]
fn dashboard_level_summaries_cover_all_levels() {
    let truth = hotel_truth_with_sheets(42);
    let mut project = printset::create_from_truth(&truth);
    let cancel = AtomicBool::new(false);
    let _ = printset::run_all_from_truth(&mut project, &truth, &cancel, &mut |_| {});

    let level_names: Vec<&str> = project.dashboard.levels.iter().map(|l| l.level.as_str()).collect();
    // The hotel has levels 1, 2, 3, 4.
    for l in &["1", "2", "3", "4"] {
        assert!(level_names.contains(l), "dashboard should have level {l}");
    }
}

// ─── Re-import with delta detection ──────────────────────────────────────────

#[test]
fn reimport_detects_unchanged_files() {
    // Create a project from truth, write files to a temp dir, reimport the same files.
    let set = generate_hotel(42, HotelSize::Small).expect("generate hotel");

    let dir = std::env::temp_dir().join("cadaddy_printset_test_reimport");
    let _ = std::fs::remove_dir_all(&dir);
    set.write_to(&dir).expect("write fixture set");

    let project = printset::create_project("Test Hotel", &dir).expect("create project");
    let files = printset::discover_sheets(&dir).expect("discover sheets");
    let delta = printset::compute_delta(&project, &files).expect("compute delta");

    // All files should be unchanged (same hashes).
    assert!(delta.added.is_empty(), "no files should be added on reimport");
    assert!(delta.changed.is_empty(), "no files should have changed");
    assert!(delta.removed.is_empty(), "no files should be removed");
    assert!(!delta.unchanged.is_empty(), "all files should be unchanged");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reimport_detects_added_and_removed_files() {
    let set = generate_hotel(42, HotelSize::Small).expect("generate hotel");

    let dir = std::env::temp_dir().join("cadaddy_printset_test_reimport_delta");
    let _ = std::fs::remove_dir_all(&dir);
    set.write_to(&dir).expect("write fixture set");

    let mut project = printset::create_project("Test Hotel", &dir).expect("create project");

    // Simulate a revised set: add a new file, remove one.
    let new_file = dir.join("NEW-001.dxf");
    std::fs::write(&new_file, b"new sheet data").expect("write new file");

    // Remove the first DXF file.
    let first_dxf = project.sheets.iter().find(|s| s.file.ends_with(".dxf")).map(|s| s.file.clone());
    if let Some(ref f) = first_dxf {
        let _ = std::fs::remove_file(dir.join(f));
    }

    let files = printset::discover_sheets(&dir).expect("discover sheets");
    let delta = printset::compute_delta(&project, &files).expect("compute delta");

    assert!(delta.added.contains(&"NEW-001.dxf".to_string()), "should detect added file");
    if first_dxf.is_some() {
        assert!(!delta.removed.is_empty(), "should detect removed file");
    }

    // Apply the reimport.
    let delta2 = printset::apply_reimport(&mut project, &files, "addendum-1").expect("apply reimport");
    assert_eq!(project.revision, "addendum-1");
    assert_eq!(project.revisions.len(), 1);
    assert_eq!(project.revisions[0].sheets_added, delta2.added.len());

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── Serialization round-trip ────────────────────────────────────────────────

#[test]
fn project_serialization_roundtrip() {
    let truth = hotel_truth_with_sheets(42);
    let mut project = printset::create_from_truth(&truth);
    let cancel = AtomicBool::new(false);
    let _ = printset::run_all_from_truth(&mut project, &truth, &cancel, &mut |_| {});

    let path = std::env::temp_dir().join("cadaddy_printset_test_roundtrip.json");
    printset::save_project(&project, &path).expect("save project");
    let loaded = printset::load_project(&path).expect("load project");

    assert_eq!(loaded.schema, project.schema);
    assert_eq!(loaded.name, project.name);
    assert_eq!(loaded.sheets.len(), project.sheets.len());
    assert_eq!(loaded.stages.len(), project.stages.len());
    assert_eq!(loaded.dashboard.total_items, project.dashboard.total_items);

    let _ = std::fs::remove_file(&path);
}

// ─── Memory budgets ──────────────────────────────────────────────────────────

#[test]
fn project_json_size_stays_reasonable() {
    let truth = hotel_truth_with_sheets(42);
    let mut project = printset::create_from_truth(&truth);
    let cancel = AtomicBool::new(false);
    let _ = printset::run_all_from_truth(&mut project, &truth, &cancel, &mut |_| {});

    let json = serde_json::to_vec(&project).expect("serialize");
    // The project JSON for a small hotel should be under 1 MB.
    assert!(json.len() < 1_000_000, "project JSON should be under 1 MB, got {} bytes", json.len());
}

#[test]
fn summary_json_is_valid() {
    let truth = hotel_truth_with_sheets(42);
    let mut project = printset::create_from_truth(&truth);
    let cancel = AtomicBool::new(false);
    let _ = printset::run_all_from_truth(&mut project, &truth, &cancel, &mut |_| {});

    let summary = project.summary_json();
    assert!(summary.is_object());
    assert!(summary.get("name").is_some());
    assert!(summary.get("sheets").is_some());
    assert!(summary.get("dashboard").is_some());
    assert!(summary.get("confidence").is_some());

    let sheets = project.sheets_json();
    assert!(sheets.is_array());
}

// ─── Stage pipeline ordering ─────────────────────────────────────────────────

#[test]
fn stages_run_in_order() {
    let truth = hotel_truth_with_sheets(42);
    let mut project = printset::create_from_truth(&truth);
    let cancel = AtomicBool::new(false);
    let _ = printset::run_all_from_truth(&mut project, &truth, &cancel, &mut |_| {});

    // Verify stages ran in order by checking the stage records.
    let stage_names: Vec<&str> = project.stages.iter().map(|r| r.stage.as_str()).collect();
    let expected: Vec<&str> = Stage::all().iter().map(|s| s.name()).collect();
    assert_eq!(stage_names.len(), expected.len(), "should have run all stages");
    for (i, name) in stage_names.iter().enumerate() {
        assert_eq!(*name, expected[i], "stage {i} should be {}, got {name}", expected[i]);
    }
}

// ─── Classification correctness ──────────────────────────────────────────────

#[test]
fn all_hotel_sheets_are_classified() {
    let truth = hotel_truth_with_sheets(42);
    let mut project = printset::create_from_truth(&truth);
    let cancel = AtomicBool::new(false);
    let _ = printset::run_classify(&mut project, None, &cancel, &mut |_| {});

    for sheet in &project.sheets {
        assert!(!sheet.number.is_empty(), "sheet should have a number: {:?}", sheet.file);
        assert!(!sheet.discipline.is_empty(), "sheet {} should have a discipline", sheet.number);
        assert!(sheet.discipline != "unknown" || sheet.sheet_type == "cover", "sheet {} has unknown discipline", sheet.number,);
    }
}

#[test]
fn discipline_counts_match_hotel() {
    let truth = hotel_truth_with_sheets(42);
    let project = printset::create_from_truth(&truth);

    let mech_count = project.sheets.iter().filter(|s| s.discipline == "M").count();
    let plumb_count = project.sheets.iter().filter(|s| s.discipline == "P").count();
    let elec_count = project.sheets.iter().filter(|s| s.discipline == "E").count();
    let arch_count = project.sheets.iter().filter(|s| s.discipline == "A").count();

    // The hotel has 4 levels, so at least 4 sheets per MEP discipline.
    assert!(mech_count >= 4, "expected at least 4 mechanical sheets, got {mech_count}");
    assert!(plumb_count >= 4, "expected at least 4 plumbing sheets, got {plumb_count}");
    assert!(elec_count >= 4, "expected at least 4 electrical sheets, got {elec_count}");
    assert!(arch_count >= 4, "expected at least 4 architectural sheets, got {arch_count}");
}
