//! The set's sheet index: one entry per real sheet from each page's title block (never the file
//! name), copies in several files merged (the newer revision date current), levels from the plan
//! titles and plan numbers, heights from the sections' level marks.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use cadaddy_doc::{Common, Drawing, EntityKind, HAlign, Line, Space, Text, VAlign};
use cadaddy_geom::Vec3;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::printset::{self, PrintSetProject, SheetEntry, StageStatus};

fn add(d: &mut Drawing, kind: EntityKind) {
    let _ = d.add(&Space::Model, Common { layer: "0".into(), ..Common::default() }, kind);
}

fn text(d: &mut Drawing, s: &str, x: f64, y: f64, h: f64) {
    add(
        d,
        EntityKind::Text(Text {
            insert: Vec3::new(x, y, 0.0),
            align_pt: None,
            height: h,
            value: s.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        }),
    );
}

fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    add(d, EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) }));
}

/// A plotted sheet: some linework, and a title block naming it (title lines under "DRAWING
/// TITLE:", the number in large lettering below, the issue date).
fn sheet(number: &str, title: &[&str], date: &str) -> Drawing {
    let mut d = Drawing::default();
    for i in 0..6 {
        let x = 100.0 + 150.0 * f64::from(i);
        line(&mut d, (x, 100.0), (x + 120.0, 100.0));
        line(&mut d, (x, 100.0), (x, 600.0));
    }
    text(&mut d, "DRAWING TITLE:", 1500.0, 300.0, 8.0);
    for (i, t) in title.iter().enumerate() {
        text(&mut d, t, 1500.0, 285.0 - 13.0 * i as f64, 8.0);
    }
    text(&mut d, number, 1500.0, 200.0, 30.0);
    if !date.is_empty() {
        text(&mut d, &format!("ISSUED {date}"), 1500.0, 150.0, 5.0);
    }
    d
}

/// A project of `pages` (file, page, drawing), classified from the drawings.
fn classify(pages: Vec<(&str, u32, Drawing)>) -> PrintSetProject {
    let mut prints: BTreeMap<(String, u32), Drawing> = BTreeMap::new();
    let mut project = PrintSetProject::default();
    for (file, page, d) in pages {
        project.sheets.push(SheetEntry { file: file.into(), page, dirty: true, ..SheetEntry::default() });
        prints.insert((file.into(), page), d);
    }
    let cancel = AtomicBool::new(false);
    let load = |e: &SheetEntry| prints.get(&(e.file.clone(), e.page)).cloned().ok_or_else(|| "no print".to_string());
    let rec = printset::run_classify_prints(&mut project, &load, &cancel, &mut |_| {});
    assert_eq!(rec.status, StageStatus::Completed);
    project
}

#[test]
fn the_title_block_names_the_sheet_never_the_file_name() {
    // An "electrical" set carrying an architectural sheet.
    let project = classify(vec![("Electrical Set.pdf", 3, sheet("A111", &["OVERALL FIRST", "FLOOR PLAN"], "10/01/2026"))]);
    let index = printset::sheet_index(&project);
    assert_eq!(index.sheets.len(), 1);
    let s = &index.sheets[0];
    assert_eq!((s.number.as_str(), s.discipline.as_str(), s.title.as_str()), ("A111", "A", "OVERALL FIRST FLOOR PLAN"));
    assert_eq!((s.sheet_type.as_str(), s.level.as_str(), s.level_source.as_str()), ("plan", "1", "title"));
    assert_eq!(s.sources, ["Electrical Set.pdf#3"]);
    assert_eq!(s.issued, "2026-10-01");
    assert_eq!(index.unidentified, 0);
}

#[test]
fn the_same_sheet_in_two_files_is_one_entry_and_the_newer_revision_is_current() {
    let title = ["OVERALL SECOND", "FLOOR PLAN"];
    // The discipline set sorts first but carries the newer revision date: the date decides.
    let project = classify(vec![
        ("Arch Set.pdf", 1, sheet("A112", &title, "10/20/2026")),
        ("Full Set.pdf", 9, sheet("A112", &title, "10/01/2026")),
        ("Full Set.pdf", 10, sheet("E-2", &["SECOND FLOOR POWER PLAN"], "10/01/2026")),
    ]);
    let index = printset::sheet_index(&project);
    assert_eq!(index.sheets.len(), 2, "{:?}", index.sheets);
    let a = index.sheets.iter().find(|s| s.number == "A112").expect("A112");
    assert_eq!(a.sources, ["Arch Set.pdf#1", "Full Set.pdf#9"], "current copy first");
    assert_eq!(a.superseded, ["Full Set.pdf#9"]);
    assert_eq!(a.issued, "2026-10-20");
    let old = project.sheets.iter().find(|e| e.file == "Full Set.pdf" && e.page == 9).expect("old copy");
    assert_eq!(old.superseded_by, "Arch Set.pdf#1");
    assert!(old.superseded_reason.starts_with("revision date: Arch Set.pdf is dated 2026-10-20"), "{}", old.superseded_reason);

    // Same date: one entry still; the later file is current (path order).
    let project = classify(vec![("Arch Set.pdf", 1, sheet("A112", &title, "10/01/2026")), ("Full Set.pdf", 9, sheet("A112", &title, "10/01/2026"))]);
    let index = printset::sheet_index(&project);
    assert_eq!(index.sheets.len(), 1);
    assert_eq!(index.sheets[0].sources, ["Full Set.pdf#9", "Arch Set.pdf#1"]);
    let old = project.sheets.iter().find(|e| e.file == "Arch Set.pdf").expect("copy");
    assert!(old.superseded_reason.starts_with("path order:"), "{}", old.superseded_reason);
}

#[test]
fn levels_come_from_plan_titles_and_numbers_and_every_plan_gets_one() {
    let project = classify(vec![
        ("Full Set.pdf", 1, sheet("A111", &["OVERALL FIRST", "FLOOR PLAN"], "")),
        ("Full Set.pdf", 2, sheet("A112", &["OVERALL SECOND", "FLOOR PLAN"], "")),
        ("Full Set.pdf", 3, sheet("A113", &["OVERALL THIRD", "FLOOR PLAN"], "")),
        ("Full Set.pdf", 4, sheet("A114", &["OVERALL FOURTH", "FLOOR PLAN"], "")),
        ("Full Set.pdf", 5, sheet("A115", &["ROOF PLAN"], "")),
        // A numbered series whose titles name no level: the last digit is the level.
        ("Mech Set.pdf", 1, sheet("M111", &["MECHANICAL PLAN"], "")),
        ("Mech Set.pdf", 2, sheet("M112", &["MECHANICAL PLAN"], "")),
        // A plan of no level and no series in a building of several levels: no level until
        // someone says which (never the lowest).
        ("Plumb Set.pdf", 1, sheet("P-201", &["PLUMBING PLAN"], "")),
        // Not a plan: no level.
        ("Full Set.pdf", 6, sheet("G-001", &["COVER SHEET"], "")),
    ]);
    let index = printset::sheet_index(&project);
    let level = |n: &str| index.sheets.iter().find(|s| s.number == n).map(|s| (s.level.clone(), s.level_source.clone())).expect(n);
    for (n, l) in [("A111", "1"), ("A112", "2"), ("A113", "3"), ("A114", "4"), ("A115", "R")] {
        assert_eq!(level(n), (l.to_string(), "title".to_string()), "{n}");
    }
    assert_eq!(level("M111"), ("1".into(), "number".into()));
    assert_eq!(level("M112"), ("2".into(), "number".into()));
    assert_eq!(level("P-201"), (String::new(), "ask".into()));
    assert_eq!(level("G-001"), (String::new(), String::new()));
    assert_eq!(index.plans_without_level, ["P-201"]);
    let names: Vec<&str> = index.levels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["1", "2", "3", "4", "R"]);
    let one = index.levels.first().expect("level 1");
    assert_eq!(one.sheets, ["A111", "M111"]);
    assert!(index.levels.iter().all(|l| l.elevation_ft.is_none()), "no sections: no heights");
}

#[test]
fn heights_come_from_the_sections_level_marks() {
    let mut section = sheet("A301", &["BUILDING SECTIONS"], "");
    for (i, (name, el)) in
        [("FIRST FLOOR", "100'-0\""), ("SECOND FLOOR", "110'-8\""), ("THIRD FLOOR", "121'-4\""), ("FOURTH FLOOR", "132'-0\""), ("ROOF", "142'-8\"")]
            .iter()
            .enumerate()
    {
        let y = 120.0 + 40.0 * i as f64;
        text(&mut section, name, 950.0, y, 6.0);
        text(&mut section, el, 950.0, y - 8.0, 6.0);
    }
    let project = classify(vec![
        ("Full Set.pdf", 1, sheet("A111", &["OVERALL FIRST", "FLOOR PLAN"], "")),
        ("Full Set.pdf", 2, sheet("A112", &["OVERALL SECOND", "FLOOR PLAN"], "")),
        ("Full Set.pdf", 3, sheet("A113", &["OVERALL THIRD", "FLOOR PLAN"], "")),
        ("Full Set.pdf", 4, sheet("A114", &["OVERALL FOURTH", "FLOOR PLAN"], "")),
        ("Full Set.pdf", 5, sheet("A115", &["ROOF PLAN"], "")),
        ("Full Set.pdf", 6, section),
    ]);
    let s = project.sheets.iter().find(|e| e.number == "A301").expect("section");
    assert_eq!(s.sheet_type, "section");
    assert_eq!(s.level_marks.len(), 5, "{:?}", s.level_marks);
    let index = printset::sheet_index(&project);
    let got: Vec<(String, Option<f64>, Option<f64>)> = index
        .levels
        .iter()
        .map(|l| (l.name.clone(), l.elevation_ft.map(|e| (e * 100.0).round() / 100.0), l.height_ft.map(|h| (h * 100.0).round() / 100.0)))
        .collect();
    assert_eq!(
        got,
        [
            ("1".to_string(), Some(0.0), Some(10.67)),
            ("2".to_string(), Some(10.67), Some(10.67)),
            ("3".to_string(), Some(21.33), Some(10.67)),
            ("4".to_string(), Some(32.0), Some(10.67)),
            ("R".to_string(), Some(42.67), None),
        ],
        "elevations above the first floor at 100'-0\""
    );
}

#[test]
fn dates_read_in_the_ways_sheets_print_them() {
    for (t, want) in [
        ("ISSUED 10/01/2026", Some("2026-10-01")),
        ("REV 2 10-24-26 ADDENDUM", Some("2026-10-24")),
        ("2026-10-20", Some("2026-10-20")),
        ("10.5.2026 / 9/30/2026", Some("2026-10-05")),
        ("SCALE: 1/8\" = 1'-0\"", None),
        ("3/4\" CONDUIT", None),
        ("13/40/2026", None),
        ("A-1,3", None),
    ] {
        assert_eq!(printset::latest_date(t).as_deref(), want, "{t}");
    }
}

#[test]
fn hostile_prints_never_panic() {
    let mut d = Drawing::default();
    text(&mut d, "DRAWING TITLE:", f64::MAX, f64::MIN, 8.0);
    text(&mut d, "A1", f64::NAN, 0.0, f64::INFINITY);
    text(&mut d, &"9/".repeat(5000), 0.0, 0.0, 1.0);
    text(&mut d, "LEVEL 99999999999 EL. 1e308'", 0.0, 0.0, 1.0);
    let project = classify(vec![("x.pdf", 1, d), ("x.pdf", 2, Drawing::default())]);
    let index = printset::sheet_index(&project);
    assert_eq!(index.pages, 2);
    let _ = serde_json::to_string(&index).expect("serializes");
}

#[test]
fn a_generated_set_is_one_entry_per_sheet_though_three_files_hold_each() {
    // The fixture folder holds every sheet three times: a page of set.pdf, its own PDF, its DXF.
    let set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    let truth = &set.truth;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_sheet_index_{}_{nanos}", std::process::id()));
    set.write_to(&dir).expect("write set");
    let mut project = printset::create_project("Project A", &dir).expect("project");
    assert!(project.sheets.len() >= 3 * truth.sheets.len(), "{} entries", project.sheets.len());
    let loader = printset::PrintLoader::new(&dir);
    let cancel = AtomicBool::new(false);
    printset::run_classify_prints(&mut project, &|e| loader.load(e), &cancel, &mut |_| {});
    let index = printset::sheet_index(&project);
    for sh in &truth.sheets {
        let got = index.sheets.iter().filter(|s| s.number == sh.number).collect::<Vec<_>>();
        assert_eq!(got.len(), 1, "{} once in the index", sh.number);
        assert_eq!(got[0].sources.len(), 3, "{}: {:?}", sh.number, got[0].sources);
        if let Some(l) = sh.level.as_ref().filter(|_| sh.title.contains("PLAN")) {
            assert_eq!(&got[0].level, l, "{} level", sh.number);
        }
    }
    assert_eq!(index.sheets.len(), truth.sheets.len(), "{:?}", index.sheets.iter().map(|s| &s.number).collect::<Vec<_>>());
    let truth_levels: std::collections::BTreeSet<&String> = truth.sheets.iter().filter_map(|s| s.level.as_ref()).collect();
    assert!(index.levels.len() >= truth_levels.len().min(2), "levels {:?}", index.levels);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A sheet with level markers drawn as a name over its elevation.
fn marked(number: &str, title: &[&str], marks: &[(&str, &str)]) -> Drawing {
    let mut d = sheet(number, title, "");
    for (i, (name, el)) in marks.iter().enumerate() {
        let y = 120.0 + 40.0 * i as f64;
        text(&mut d, name, 950.0, y, 6.0);
        text(&mut d, el, 950.0, y - 8.0, 6.0);
    }
    d
}

#[test]
fn a_site_sheets_spot_elevation_is_no_level_and_levels_rise_in_order() {
    let plans = || {
        vec![
            ("Full Set.pdf", 1, sheet("A111", &["OVERALL FIRST", "FLOOR PLAN"], "")),
            ("Full Set.pdf", 2, sheet("A112", &["OVERALL SECOND", "FLOOR PLAN"], "")),
            ("Full Set.pdf", 3, sheet("A113", &["OVERALL THIRD", "FLOOR PLAN"], "")),
            ("Full Set.pdf", 4, sheet("A114", &["OVERALL FOURTH", "FLOOR PLAN"], "")),
        ]
    };
    let elevations = |index: &printset::SheetIndex| -> Vec<(String, Option<f64>)> {
        index.levels.iter().map(|l| (l.name.clone(), l.elevation_ft.map(|e| (e * 100.0).round() / 100.0))).collect()
    };
    let section = || marked("A301", &["BUILDING SECTIONS"], &[("SECOND FLOOR", "12'-0\""), ("THIRD FLOOR", "24'-0\""), ("FOURTH FLOOR", "36'-0\"")]);
    // The civil sheet marks the finished first floor on the site datum (147'-0").
    let mut pages = plans();
    pages.push(("Civil Set.pdf", 1, marked("C-101", &["GRADING AND", "DRAINAGE PLAN"], &[("FIRST FLOOR", "147'-0\"")])));
    pages.push(("Full Set.pdf", 5, section()));
    let index = printset::sheet_index(&classify(pages));
    assert_eq!(
        elevations(&index),
        [("1".to_string(), None), ("2".to_string(), Some(12.0)), ("3".to_string(), Some(24.0)), ("4".to_string(), Some(36.0))]
    );
    assert!(index.level_warnings.is_empty(), "{:?}", index.level_warnings);
    // The same mark on a sheet that is no site sheet: it breaks the order, so it is dropped.
    let mut pages = plans();
    pages.push(("Full Set.pdf", 6, marked("A201", &["EXTERIOR ELEVATIONS"], &[("FIRST FLOOR", "147'-0\"")])));
    pages.push(("Full Set.pdf", 5, section()));
    let index = printset::sheet_index(&classify(pages));
    assert_eq!(elevations(&index).first(), Some(&("1".to_string(), None)));
    assert_eq!(index.level_warnings.len(), 1, "{:?}", index.level_warnings);
    assert!(index.level_warnings[0].starts_with("level 1: 147'-0\" not used"), "{:?}", index.level_warnings);
}

/// `sheet` with one view titled `view` over its scale line, in the drawing area.
fn sheet_with_view(number: &str, title: &[&str], view: &str) -> Drawing {
    let mut d = sheet(number, title, "");
    text(&mut d, view, 300.0, 80.0, 10.0);
    text(&mut d, "SCALE: 1/32\" = 1'-0\"", 300.0, 62.0, 5.0);
    d
}

#[test]
fn only_floor_plans_define_and_fill_a_level() {
    let project = classify(vec![
        ("Full Set.pdf", 1, sheet("A111", &["FIRST FLOOR PLAN"], "")),
        ("Full Set.pdf", 2, sheet("A112", &["SECOND FLOOR PLAN"], "")),
        // An enlarged plan whose only level word is its key plan's: part of a floor, no level.
        ("Full Set.pdf", 3, sheet_with_view("A501", &["ENLARGED UNIT PLANS"], "KEY PLAN - FIRST FLOOR")),
        // Details naming a level: a reference sheet.
        ("Full Set.pdf", 4, sheet("A601", &["FIRST FLOOR DETAILS"], "")),
        // A landscape plan naming a floor (a courtyard on it): on the site.
        ("Full Set.pdf", 5, sheet("L101", &["LANDSCAPE PLAN - FIRST FLOOR COURTYARD"], "")),
        // A cover sheet naming a level: no level.
        ("Fire Alarm Set.pdf", 1, sheet("FA001", &["FIRE ALARM COVER SHEET - LEVEL 1"], "")),
        // A plan naming no level in a building of two: no level until someone says which.
        ("Plumb Set.pdf", 1, sheet("P-201", &["PLUMBING PLAN"], "")),
    ]);
    let index = printset::sheet_index(&project);
    let level = |n: &str| {
        index
            .sheets
            .iter()
            .find(|s| s.number == n)
            .map(|s| (s.level.clone(), s.level_source.clone()))
            .unwrap_or_else(|| panic!("{n}: {:?}", index.sheets))
    };
    assert_eq!(level("A111"), ("1".into(), "title".into()));
    assert_eq!(level("A501"), (String::new(), String::new()), "a key plan names its floor, not the sheet's");
    assert_eq!(level("A601"), (String::new(), String::new()), "details are on no level");
    assert_eq!(level("L101"), ("SITE".into(), "site".into()), "a landscape plan is on the site");
    assert_eq!(level("FA001"), (String::new(), String::new()), "a cover is on no level");
    assert_eq!(level("P-201"), (String::new(), "ask".into()));
    assert_eq!(index.plans_without_level, ["P-201"]);
    let names: Vec<&str> = index.levels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["1", "2"], "the site and reference sheets make no level");
    assert_eq!(index.levels.first().map(|l| l.sheets.clone()), Some(vec!["A111".to_string()]), "only floor plans fill a level");
}

#[test]
fn in_a_building_of_one_level_a_plan_naming_none_is_on_it() {
    let project =
        classify(vec![("Full Set.pdf", 1, sheet("A111", &["FIRST FLOOR PLAN"], "")), ("Plumb Set.pdf", 1, sheet("P-201", &["PLUMBING PLAN"], ""))]);
    let index = printset::sheet_index(&project);
    let p = index.sheets.iter().find(|s| s.number == "P-201").expect("P-201");
    assert_eq!((p.level.as_str(), p.level_source.as_str()), ("1", "default"));
    assert_eq!(index.plans_without_level, ["P-201"]);
}
