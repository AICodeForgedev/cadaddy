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

// ─── A real folder: a merged set, a revision folder, a two-level sheet ────────

/// Loads an entry as the project would: a page of a PDF, or a DXF.
fn load_print(dir: &std::path::Path, e: &printset::SheetEntry) -> Result<cadaddy_doc::Drawing, String> {
    let bytes = std::fs::read(dir.join(&e.file)).map_err(|err| err.to_string())?;
    if e.file.ends_with(".dxf") {
        return cadaddy_io::read_dxf(&bytes).map_err(|err| err.to_string());
    }
    let opts = cadaddy_io::pdf_read::PdfOptions { page: e.page.max(1), images: false, markups: false, layout: false, ..Default::default() };
    cadaddy_io::pdf_read::read(&bytes, &opts).map(|i| i.drawing).map_err(|err| err.to_string())
}

#[test]
fn a_folder_with_a_merged_set_and_a_revision_folder_classifies_page_by_page() {
    let set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    let truth = &set.truth;
    let dir = std::env::temp_dir().join("cadaddy_printset_test_folder");
    let _ = std::fs::remove_dir_all(&dir);
    let rev = dir.join("Revision 1");
    std::fs::create_dir_all(&rev).expect("folders");
    // The set as one merged PDF; the revision folder reissues one sheet and adds a sheet that
    // prints two levels' plans.
    std::fs::write(dir.join("Sample Set.pdf"), set.file("set.pdf").expect("set.pdf")).expect("write set");
    let reissued = truth.sheets.iter().find(|s| s.number == "M-102").expect("M-102");
    std::fs::write(rev.join("M-102.pdf"), set.file(&reissued.pdf).expect("sheet pdf")).expect("write revision");
    let (t, st) = cadaddy_mepfix::hotel_planned(7, HotelSize::Small);
    let two = cadaddy_mepfix::printed::two_view_sheet(&t, &st, "E-101", "E-102", "E-1A", "LIGHTING").expect("two-view sheet");
    std::fs::write(rev.join("E-1A.dxf"), cadaddy_io::write_dxf(&two.drawing)).expect("write two-view sheet");

    let mut project = printset::create_project("Project A", &dir).expect("create project");
    let pages = truth.sheets.len();
    assert_eq!(project.sheets.len(), pages + 2, "one entry per page of the merged set, then the revision folder's files");
    assert!(project.sheets.iter().take(pages).all(|s| s.page >= 1), "the merged set's entries are its pages");
    assert!(project.sheets.last().is_some_and(|s| s.file.starts_with("Revision 1/")), "revision folders come after the set");

    let cancel = AtomicBool::new(false);
    let rec = printset::run_classify_prints(&mut project, &|e| load_print(&dir, e), &cancel, &mut |_| {});
    assert_eq!(rec.status, StageStatus::Completed);

    let current: Vec<&printset::SheetEntry> = project.sheets.iter().filter(|s| s.superseded_by.is_empty()).collect();
    for sh in &truth.sheets {
        assert!(current.iter().any(|s| s.number == sh.number), "{} read from its page", sh.number);
    }
    // The reissued sheet replaces the set's page.
    let replaced: Vec<&printset::SheetEntry> = project.sheets.iter().filter(|s| !s.superseded_by.is_empty()).collect();
    assert_eq!(replaced.len(), 1);
    assert_eq!(replaced[0].number, "M-102");
    assert_eq!(replaced[0].superseded_by, "Revision 1/M-102.pdf#1");
    assert_eq!(project.dashboard.superseded_sheets, 1);
    // The two-level sheet is two plans, one per level.
    for (number, level) in [("E-1A/1", "1"), ("E-1A/2", "2")] {
        let v = current.iter().find(|s| s.number == number).unwrap_or_else(|| panic!("{number}"));
        assert_eq!((v.level.as_str(), v.sheet_type.as_str(), v.discipline.as_str()), (level, "plan", "E"));
    }
    // Plans carry their level.
    for sh in truth.sheets.iter().filter(|s| s.level.is_some() && s.title.contains("PLAN")) {
        let got = current.iter().find(|s| s.number == sh.number).map(|s| s.level.clone());
        assert_eq!(got, sh.level.clone(), "{} level", sh.number);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_floor_plan_title_without_a_floor_has_no_level() {
    // "FLOOR PLAN" isn't level "PLAN"; lists and ranges give their first level.
    for (title, want) in [
        ("ENLARGED FLOOR PLAN - KITCHEN", ""),
        ("SHEET NOTES - FLOOR PLAN", ""),
        ("LEVEL 2 MECHANICAL PLAN", "2"),
        ("THIRD & FOURTH FLOOR DUCT PLAN", "3"),
        ("ROOF FRAMING PLAN", "R"),
        ("GRADING AND EARTHWORK PLAN", "SITE"),
    ] {
        assert_eq!(printset::level_of_title(title), want, "{title}");
    }
}

#[test]
fn names_sort_with_their_numbers_by_value() {
    use std::cmp::Ordering;
    let mut names = vec!["Revision 10", "Revision 2", "revision 1", "ASI 03", "ASI 12", "Addendum", "Revision 2a"];
    names.sort_by(|a, b| printset::natural_cmp(a, b));
    assert_eq!(names, ["Addendum", "ASI 03", "ASI 12", "revision 1", "Revision 2", "Revision 2a", "Revision 10"]);
    assert_eq!(printset::natural_cmp("ASI 02", "ASI 2"), "ASI 02".cmp("ASI 2"), "equal numbers: exact text breaks the tie");
    // A folder's own files come before its subfolders; folders compare by number.
    assert_eq!(printset::path_order("Sample Set.pdf", "Revision 1/M-102.pdf"), Ordering::Less);
    assert_eq!(printset::path_order("Revision 2/M-102.pdf", "Revision 10/M-102.pdf"), Ordering::Less);
    assert_eq!(printset::path_order("Revision 2/Z.pdf", "Revision 2/Old/A.pdf"), Ordering::Less);
}

#[test]
fn revision_ten_supersedes_revision_two_and_says_why() {
    let set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    let truth = &set.truth;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_printset_rev_order_{}_{nanos}", std::process::id()));
    let reissued = truth.sheets.iter().find(|s| s.number == "M-102").expect("M-102");
    let pdf = set.file(&reissued.pdf).expect("sheet pdf");
    std::fs::create_dir_all(&dir).expect("folder");
    std::fs::write(dir.join("Sample Set.pdf"), set.file("set.pdf").expect("set.pdf")).expect("write set");
    // Made in the order a string sort would put them, so the order written proves nothing.
    for rev in ["Revision 10", "Revision 2"] {
        std::fs::create_dir_all(dir.join(rev)).expect("revision folder");
        std::fs::write(dir.join(rev).join("M-102.pdf"), pdf).expect("write revision");
    }

    let mut project = printset::create_project("Project A", &dir).expect("create project");
    let files: Vec<&str> = project.sheets.iter().skip(truth.sheets.len()).map(|s| s.file.as_str()).collect();
    assert_eq!(files, ["Revision 2/M-102.pdf", "Revision 10/M-102.pdf"], "revision folders in number order");

    let cancel = AtomicBool::new(false);
    let rec = printset::run_classify_prints(&mut project, &|e| load_print(&dir, e), &cancel, &mut |_| {});
    assert_eq!(rec.status, StageStatus::Completed);
    let m102: Vec<&printset::SheetEntry> = project.sheets.iter().filter(|s| s.number == "M-102").collect();
    assert_eq!(m102.len(), 3, "the set's page and both revisions");
    let current: Vec<&&printset::SheetEntry> = m102.iter().filter(|s| s.superseded_by.is_empty()).collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].file, "Revision 10/M-102.pdf", "Revision 10 is current");
    assert!(current[0].superseded_reason.is_empty());
    for old in m102.iter().filter(|s| !s.superseded_by.is_empty()) {
        assert_eq!(old.superseded_by, "Revision 10/M-102.pdf#1", "{} replaced by Revision 10", old.file);
        assert!(old.superseded_reason.starts_with("path order: Revision 10/M-102.pdf comes after "), "{}", old.superseded_reason);
        assert!(old.superseded_reason.contains(&old.file), "{}", old.superseded_reason);
    }
    assert_eq!(project.dashboard.superseded_sheets, 2);
    // The reason travels with the sheet list.
    let listed = project.sheets_json();
    let rows = listed.as_array().expect("sheet rows");
    assert_eq!(rows.iter().filter(|r| r["superseded_reason"].as_str().is_some_and(|s| s.starts_with("path order:"))).count(), 2);

    // A re-import that appends Revision 2's page after Revision 10's still leaves Revision 10 current.
    project.sheets.retain(|s| s.file != "Revision 2/M-102.pdf");
    let mut all = printset::discover_sheets(&dir).expect("discover");
    all.retain(|(n, _)| n != "Revision 2/M-102.pdf");
    all.push(("Revision 2/M-102.pdf".into(), dir.join("Revision 2").join("M-102.pdf")));
    printset::apply_reimport(&mut project, &all, "rev 2 again").expect("reimport");
    assert_eq!(project.sheets.last().map(|s| s.file.as_str()), Some("Revision 2/M-102.pdf"), "the re-import appends");
    printset::run_classify_prints(&mut project, &|e| load_print(&dir, e), &cancel, &mut |_| {});
    let now: Vec<&str> = project.sheets.iter().filter(|s| s.number == "M-102" && s.superseded_by.is_empty()).map(|s| s.file.as_str()).collect();
    assert_eq!(now, ["Revision 10/M-102.pdf"]);
    let _ = std::fs::remove_dir_all(&dir);
}
