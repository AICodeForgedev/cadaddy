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
    // typicalfix: each level lists its sheets as {sheet, typical} (typical = the sheet covers more than one level).
    assert_eq!(one.sheets, [serde_json::json!({"sheet": "A111", "typical": false}), serde_json::json!({"sheet": "M111", "typical": false})]);
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
    assert_eq!(
        index.levels.first().map(|l| l.sheets.clone()),
        Some(vec![serde_json::json!({"sheet": "A111", "typical": false})]),
        "only floor plans fill a level"
    );
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

#[test]
fn a_ground_floor_plan_is_the_first_floor() {
    let project = classify(vec![
        ("Set.pdf", 1, sheet("A1.1", &["GROUND FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A1.2", &["MAIN LEVEL - ENLARGED"], "")),
        ("Set.pdf", 3, sheet("A1.3", &["ROOF PLAN"], "")),
    ]);
    let index = printset::sheet_index(&project);
    let a = index.sheets.iter().find(|s| s.number == "A1.1").expect("A1.1");
    assert_eq!((a.level.as_str(), a.level_source.as_str()), ("1", "title"));
    let names: Vec<&str> = index.levels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["1", "R"]);
}

/// A one-storey building whose plans name no floor (`FLOOR PLAN`), only its roof plan a level:
/// the floor is still a level, and every plan naming none is on it (to be confirmed), whichever
/// discipline's and in whichever order the set prints them.
#[test]
fn a_one_storey_building_named_only_by_its_roof_plan_has_its_floor() {
    for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0]] {
        let all = [
            ("Bid Set.pdf", 1, sheet("E201", &["FLOOR PLAN - ELECTRICAL"], "")),
            ("Bid Set.pdf", 2, sheet("A-110", &["ROOF PLAN"], "")),
            ("Bid Set.pdf", 3, sheet("A-100", &["FLOOR PLAN"], "")),
        ];
        let project = classify(order.iter().map(|&i| all[i].clone()).collect());
        let index = printset::sheet_index(&project);
        let names: Vec<&str> = index.levels.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["1", "R"], "{order:?}");
        for n in ["E201", "A-100"] {
            let s = index.sheets.iter().find(|s| s.number == n).expect("sheet");
            assert_eq!((s.level.as_str(), s.level_source.as_str()), ("1", "default"), "{n} {order:?}");
        }
        let first = index.levels.first().map(|l| l.sheets.len());
        assert_eq!(first, Some(2), "the floor's plans: {:?}", index.levels);
    }
}

/// A building of one floor and a roof: a plan naming no level is on the floor (to be
/// confirmed), not left for someone to say which.
#[test]
fn a_floor_and_its_roof_are_one_storey() {
    let project = classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FIRST FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["ROOF PLAN"], "")),
        ("Set.pdf", 3, sheet("P-201", &["PLUMBING PLAN"], "")),
    ]);
    let index = printset::sheet_index(&project);
    let p = index.sheets.iter().find(|s| s.number == "P-201").expect("P-201");
    assert_eq!((p.level.as_str(), p.level_source.as_str()), ("1", "default"));
    let names: Vec<&str> = index.levels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["1", "R"]);
}

/// Where a set's title block puts the sheet number.
#[derive(Clone, Copy)]
enum Strip {
    /// A strip down the right edge, no labels, the number in its bottom corner.
    Right,
    /// A band along the bottom, the number at its right end.
    Bottom,
}

/// A page of a combined set whose title block has no labels and whose number doesn't stand out
/// (not twice the notes' lettering), `w` × `h` (sheet sizes vary in one set). `number` is `None`
/// when the number isn't text (plotted as strokes). `tags` are equipment and schedule marks in
/// the drawing area near the strip, lettered larger than the number: `(tag, x, y)` as shares of
/// the sheet.
fn combined_page(strip: Strip, w: f64, h: f64, number: Option<&str>, title: &str, page: u32, tags: &[(&str, f64, f64)]) -> Drawing {
    let mut d = Drawing::default();
    line(&mut d, (0.0, 0.0), (w, 0.0));
    line(&mut d, (w, 0.0), (w, h));
    line(&mut d, (w, h), (0.0, h));
    line(&mut d, (0.0, h), (0.0, 0.0));
    let note = 0.006 * h;
    // The drawing: rooms of linework, notes and lettering of this sheet's own.
    for i in 0..8 {
        let x = 0.05 * w + 0.09 * w * f64::from(i);
        line(&mut d, (x, 0.25 * h), (x + 0.07 * w, 0.25 * h));
        line(&mut d, (x, 0.25 * h), (x, 0.85 * h));
    }
    for i in 0..24 {
        let at = (0.06 * w + 0.03 * w * f64::from(i % 12), 0.3 * h + 0.25 * h * f64::from(i / 12));
        text(&mut d, &format!("ROOM {page}{i:02}"), at.0, at.1, note);
    }
    for i in 0..10 {
        text(&mut d, &format!("GENERAL NOTE {i}"), 0.1 * w, 0.9 * h - 0.012 * h * f64::from(i), note);
    }
    for (t, x, y) in tags {
        text(&mut d, t, x * w, y * h, 1.6 * 0.012 * h);
    }
    // The title block: firm and project lines, the title, the number in the corner.
    match strip {
        Strip::Right => {
            line(&mut d, (0.9 * w, 0.0), (0.9 * w, h));
            for (i, t) in ["ARCHITECTS INC", "123 SAMPLE STREET", "PROJECT A", "HOTEL BUILDING"].iter().enumerate() {
                text(&mut d, t, 0.91 * w, 0.9 * h - 0.02 * h * i as f64, note);
            }
            text(&mut d, title, 0.91 * w, 0.12 * h, note);
            if let Some(n) = number {
                text(&mut d, n, 0.93 * w, 0.04 * h, 0.012 * h);
            }
        }
        Strip::Bottom => {
            line(&mut d, (0.0, 0.1 * h), (w, 0.1 * h));
            for (i, t) in ["ARCHITECTS INC", "123 SAMPLE STREET", "PROJECT A"].iter().enumerate() {
                text(&mut d, t, 0.05 * w + 0.15 * w * i as f64, 0.05 * h, note);
            }
            text(&mut d, title, 0.6 * w, 0.05 * h, note);
            if let Some(n) = number {
                text(&mut d, n, 0.93 * w, 0.03 * h, 0.012 * h);
            }
        }
    }
    d
}

/// Q6281: a combined set whose title block numbers don't stand out, with equipment and schedule
/// tags in the drawing area along the title block. Each page's number comes from the title block's
/// number spot, never a tag; pages are never merged under a tag; electrical pages are electrical.
#[test]
fn a_combined_sets_numbers_come_from_its_title_block_never_a_tag_in_the_drawing() {
    for strip in [Strip::Right, Strip::Bottom] {
        // Equipment tags at the right of the drawing area (inside the band a reader looks in for
        // a title block), larger than the number: the same few repeat page to page.
        let (tx, ty) = match strip {
            Strip::Right => (0.84, 0.3),
            Strip::Bottom => (0.84, 0.12),
        };
        let equipment = [("EF-3", tx, ty), ("WH-1", tx, ty + 0.05), ("UH-2", tx + 0.02, ty + 0.1)];
        // A plumbing fixture mark: its letter is a discipline's, so only the place tells it apart.
        let fixture = [("P-4", tx, ty + 0.15)];
        let sheets: Vec<(&str, &str, Vec<(&str, f64, f64)>)> = vec![
            ("G-001", "COVER SHEET", vec![]),
            ("A-101", "FIRST FLOOR PLAN", vec![]),
            ("A-102", "SECOND FLOOR PLAN", vec![]),
            ("M-101", "FIRST FLOOR HVAC PLAN", equipment.to_vec()),
            ("M-102", "SECOND FLOOR HVAC PLAN", equipment.to_vec()),
            ("M-601", "MECHANICAL SCHEDULES", equipment.to_vec()),
            ("P-101", "FIRST FLOOR PLUMBING PLAN", [equipment.as_slice(), fixture.as_slice()].concat()),
            ("P-102", "SECOND FLOOR PLUMBING PLAN", [equipment.as_slice(), fixture.as_slice()].concat()),
            ("E-101", "FIRST FLOOR POWER PLAN", equipment.to_vec()),
            ("E-102", "SECOND FLOOR POWER PLAN", [equipment.as_slice(), fixture.as_slice()].concat()),
            ("E-201", "FIRST FLOOR LIGHTING PLAN", equipment.to_vec()),
            ("E-601", "PANEL SCHEDULES", equipment.to_vec()),
        ];
        let mut pages: Vec<(&str, u32, Drawing)> = Vec::new();
        for (i, (n, t, tags)) in sheets.iter().enumerate() {
            let page = u32::try_from(i + 1).expect("page");
            // Two sheet sizes in one set.
            let (w, h) = if i % 3 == 0 { (3600.0, 2400.0) } else { (3000.0, 2000.0) };
            pages.push(("Combined Set.pdf", page, combined_page(strip, w, h, Some(n), t, page, tags)));
        }
        // A page whose number is plotted as strokes (no text): unnumbered, never named by a tag.
        pages.push(("Combined Set.pdf", 13, combined_page(strip, 3000.0, 2000.0, None, "ROOF HVAC PLAN", 13, &equipment)));
        let project = classify(pages);
        let index = printset::sheet_index(&project);
        let numbers: Vec<&str> = index.sheets.iter().map(|s| s.number.as_str()).collect();
        for (n, _, _) in &sheets {
            assert!(numbers.contains(n), "{n} missing: {numbers:?}");
        }
        assert_eq!(index.pages, 13);
        assert_eq!(index.sheets.len(), 13, "every page its own sheet: {numbers:?}");
        assert!(index.sheets.iter().all(|s| s.sources.len() == 1), "no page merged into another: {:?}", index.sheets);
        for tag in ["EF-3", "WH-1", "UH-2", "P-4"] {
            assert!(!numbers.contains(&tag), "a tag named a sheet: {numbers:?}");
        }
        assert_eq!(index.unidentified, 1, "the page with no number text");
        let disc = |n: &str| index.sheets.iter().find(|s| s.number == n).map(|s| s.discipline.clone()).expect(n);
        for n in ["E-101", "E-102", "E-201", "E-601"] {
            assert_eq!(disc(n), "E", "{n}");
        }
        assert_eq!(disc("P-101"), "P");
        let codes: std::collections::BTreeSet<String> = index.sheets.iter().map(|s| s.discipline.clone()).collect();
        assert!(codes.iter().all(|c| ["G", "A", "M", "P", "E", "unknown", ""].contains(&c.as_str())), "{codes:?}");
    }
}

/// A set numbering one discipline's sheets in a series of its own (two letters the designator
/// list doesn't name, the first a discipline's), printed at another spot of another title block
/// than the trades' numbers. Each of those pages keeps its own number, never a number-like text
/// that happens to stand where the trades print theirs, and the series is its discipline.
#[test]
fn a_sets_own_series_keeps_its_numbers_at_its_own_spot() {
    let (w, h) = (3000.0, 2000.0);
    let mut pages: Vec<(&str, u32, Drawing)> = Vec::new();
    let trades = [
        ("M-101", "FIRST FLOOR HVAC PLAN"),
        ("M-102", "SECOND FLOOR HVAC PLAN"),
        ("E-101", "FIRST FLOOR POWER PLAN"),
        ("E-102", "SECOND FLOOR POWER PLAN"),
        ("P-101", "FIRST FLOOR PLUMBING PLAN"),
    ];
    for (i, (n, t)) in trades.iter().enumerate() {
        let page = u32::try_from(i + 1).expect("page");
        pages.push(("Combined Set.pdf", page, combined_page(Strip::Right, w, h, Some(n), t, page, &[])));
    }
    let series = ["AJ101", "AJ102", "AJ103", "AJ104", "AJ104.1", "AJ104.2"];
    for (i, n) in series.iter().enumerate() {
        let page = u32::try_from(trades.len() + i + 1).expect("page");
        let title = format!("UNIT {} FLOOR PLAN", i + 1);
        let mut d = combined_page(Strip::Bottom, w, h, None, &title, page, &[]);
        // its own number, in its own title block's corner
        text(&mut d, n, 0.75 * w, 0.03 * h, 0.012 * h);
        // and a number-like mark where the trades' title blocks print theirs
        text(&mut d, "L9", 0.93 * w, 0.04 * h, 0.012 * h);
        pages.push(("Combined Set.pdf", page, d));
    }
    let project = classify(pages);
    let index = printset::sheet_index(&project);
    let numbers: Vec<&str> = index.sheets.iter().map(|s| s.number.as_str()).collect();
    for n in trades.iter().map(|(n, _)| *n).chain(series) {
        assert!(numbers.contains(&n), "{n} missing: {numbers:?}");
    }
    assert!(!numbers.iter().any(|n| n.starts_with("L9")), "a stray mark named a sheet: {numbers:?}");
    let disc = |n: &str| index.sheets.iter().find(|s| s.number == n).map(|s| s.discipline.clone()).expect(n);
    assert_eq!(disc("AJ104.1"), "A");
}

/// A labelled title block (`DRAWING TITLE`, the number below) on a page with more notes.
fn labelled(number: &str, title: &[&str], notes: &[&str]) -> Drawing {
    let mut d = sheet(number, title, "10/01/2026");
    for (i, n) in notes.iter().enumerate() {
        text(&mut d, n, 200.0, 500.0 - 12.0 * i as f64, 5.0);
    }
    d
}

/// Q6281: pages are copies of one sheet only when their numbers AND titles (or contents) match.
/// Two different sheets that print one number are a conflict to report, both kept; a copy of a
/// sheet in another file is still one sheet.
#[test]
fn pages_that_share_a_number_are_copies_only_when_they_are_the_same_sheet() {
    let power: Vec<String> = (0..12).map(|i| format!("RECEPTACLE NOTE {i}")).collect();
    let light: Vec<String> = (0..12).map(|i| format!("LUMINAIRE NOTE {i}")).collect();
    let power: Vec<&str> = power.iter().map(String::as_str).collect();
    let light: Vec<&str> = light.iter().map(String::as_str).collect();
    let project = classify(vec![
        ("Combined Set.pdf", 1, labelled("E-201", &["SECOND FLOOR", "POWER PLAN"], &power)),
        ("Combined Set.pdf", 2, labelled("E-201", &["THIRD FLOOR", "LIGHTING PLAN"], &light)),
        ("Electrical Set.pdf", 1, labelled("E-201", &["SECOND FLOOR", "POWER PLAN"], &power)),
    ]);
    let index = printset::sheet_index(&project);
    assert_eq!(index.sheets.len(), 2, "{:?}", index.sheets);
    let kept = index.sheets.iter().find(|s| s.number == "E-201").expect("E-201");
    assert_eq!(kept.title, "SECOND FLOOR POWER PLAN");
    assert_eq!(kept.sources.len(), 2, "the electrical set's copy is a copy: {:?}", kept.sources);
    let other = index.sheets.iter().find(|s| s.number == "E-201~2").expect("the other sheet keeps its own entry");
    assert_eq!((other.title.as_str(), other.sources.as_slice()), ("THIRD FLOOR LIGHTING PLAN", ["Combined Set.pdf#2".to_string()].as_slice()));
    assert_eq!(index.number_conflicts.len(), 1, "{:?}", index.number_conflicts);
    let c = &index.number_conflicts[0];
    assert_eq!((c.number.as_str(), c.sheets.len()), ("E-201", 2), "{c:?}");
    let page2 = project.sheets.iter().find(|e| e.file == "Combined Set.pdf" && e.page == 2).expect("page 2");
    assert_eq!(page2.printed_as, "E-201");
    let found = page2.stage_results.get("classify").map(|r| r.findings.iter().any(|f| f.kind == "conflict")).unwrap_or(false);
    assert!(found, "the clash is reported on the page");
}

/// The levels and elevations of an index, rounded to the hundredth of a foot.
fn levels_of(index: &printset::SheetIndex) -> Vec<(String, Option<f64>, Option<f64>)> {
    let r = |v: f64| (v * 100.0).round() / 100.0;
    index.levels.iter().map(|l| (l.name.clone(), l.elevation_ft.map(r), l.height_ft.map(r))).collect()
}

fn lv(name: &str, el: Option<f64>, h: Option<f64>) -> (String, Option<f64>, Option<f64>) {
    (name.to_string(), el, h)
}

/// A section's level markers name their level (`SECOND FLOOR`, `ROOF`); a detail callout's bare
/// number (`3`, `26`), a lone letter or a fixture tag (`L1`) beside a dimension is no level marker.
#[test]
fn a_level_marker_names_its_level_and_a_bare_number_beside_a_dimension_is_none() {
    let mut section = marked("A301", &["BUILDING SECTIONS"], &[("FIRST FLOOR", "100'-0\""), ("SECOND FLOOR", "110'-6\""), ("ROOF", "121'-0\"")]);
    for (i, (bubble, dim)) in
        [("3", "1'-0\""), ("26", "0'-0\""), ("R", "1'-0\""), ("7", "4'-0\""), ("L1", "9'-7\""), ("L 2", "3'-2\"")].iter().enumerate()
    {
        let x = 300.0 + 120.0 * i as f64;
        text(&mut section, bubble, x, 300.0, 6.0);
        text(&mut section, dim, x, 292.0, 6.0);
    }
    let project = classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FIRST FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["SECOND FLOOR PLAN"], "")),
        ("Set.pdf", 3, sheet("A103", &["ROOF PLAN"], "")),
        ("Set.pdf", 4, section),
    ]);
    let s = project.sheets.iter().find(|e| e.number == "A301").expect("section");
    let mut codes: Vec<&str> = s.level_marks.iter().map(|m| m.level.as_str()).collect();
    codes.sort_unstable();
    assert_eq!(codes, ["1", "2", "R"], "{:?}", s.level_marks);
    let index = printset::sheet_index(&project);
    assert!(index.level_warnings.is_empty(), "{:?}", index.level_warnings);
    assert_eq!(levels_of(&index), [lv("1", Some(0.0), Some(10.5)), lv("2", Some(10.5), Some(10.5)), lv("R", Some(21.0), None)]);
}

/// `sheet` with large view titles (each over its scale line) side by side in the drawing area.
fn sheet_with_views(number: &str, title: &[&str], views: &[&str]) -> Drawing {
    let mut d = sheet(number, title, "");
    for (i, v) in views.iter().enumerate() {
        let x = 150.0 + 500.0 * i as f64;
        text(&mut d, v, x, 80.0, 16.0);
        text(&mut d, "SCALE: 1/4\" = 1'-0\"", x, 62.0, 5.0);
    }
    d
}

/// Details, sections and elevations that name a level (`LEVEL 3 SHEAR WALL DETAIL`) are no plan
/// views: they never split their sheet into level plans nor make a level the floor plans don't.
#[test]
fn detail_and_section_view_titles_make_no_level() {
    let project = classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FIRST FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["SECOND FLOOR PLAN"], "")),
        ("Set.pdf", 3, sheet_with_views("S501", &["SHEAR WALL", "DETAILS"], &["LEVEL 2 SHEAR WALL DETAIL", "LEVEL 3 SHEAR WALL DETAIL"])),
        ("Set.pdf", 4, sheet_with_views("S502", &["FRAMING"], &["LEVEL 3 WALL SECTION", "LEVEL 4 WALL SECTION"])),
    ]);
    let index = printset::sheet_index(&project);
    let names: Vec<&str> = index.levels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["1", "2"], "{:?}", index.sheets);
    for n in ["S501", "S502"] {
        let s = index.sheets.iter().find(|s| s.number == n).unwrap_or_else(|| panic!("{n}: {:?}", index.sheets));
        assert!(s.level.is_empty() && s.levels.is_empty(), "{n}: {s:?}");
    }
}

/// A floor that has plans is a level, even when its plans are enlarged or partial ones, or name
/// the floor only in the view title under the plan.
#[test]
fn a_floor_with_plans_is_a_level_whatever_its_plans_are_titled() {
    let project = classify(vec![
        ("Set.pdf", 1, sheet("A1.21", &["ENLARGED FIRST FLOOR", "PLAN - NORTH"], "")),
        ("Set.pdf", 2, sheet("A1.22", &["ENLARGED FIRST FLOOR", "PLAN - SOUTH"], "")),
        ("Set.pdf", 3, sheet("A1.31", &["SECOND FLOOR PLAN"], "")),
        ("Set.pdf", 4, sheet_with_view("A1.41", &["BUILDING A", "OVERALL"], "THIRD FLOOR PLAN")),
        ("Set.pdf", 5, sheet("A1.51", &["ROOF PLAN"], "")),
    ]);
    let index = printset::sheet_index(&project);
    let names: Vec<&str> = index.levels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["1", "2", "3", "R"], "{:?}", index.sheets);
    let first = index.levels.first().map(|l| l.sheets.clone()).unwrap_or_default();
    assert_eq!(
        first,
        [serde_json::json!({"sheet": "A1.21", "typical": false}), serde_json::json!({"sheet": "A1.22", "typical": false})],
        "the floor's only plans are its enlarged ones"
    );
    let a141 = index.sheets.iter().find(|s| s.number == "A1.41").expect("A1.41");
    assert_eq!((a141.sheet_type.as_str(), a141.level.as_str()), ("plan", "3"), "{a141:?}");
}

/// Elevations are above the first floor whatever datum the sections use (`100'-0"` or the true
/// elevation); a section's unnumbered finish floor is a one-storey building's floor, and the top
/// of plate is where the roof bears when no roof is marked.
#[test]
fn elevations_on_any_datum_from_finish_floor_and_top_of_plate() {
    // Three storeys on the true elevation, the roof marked only by its plate.
    let section = marked(
        "A301",
        &["WALL SECTIONS"],
        &[("FIRST FLOOR", "112'-7\""), ("SECOND FLOOR", "123'-1\""), ("THIRD FLOOR", "133'-7\""), ("T.O. PLATE", "142'-8\"")],
    );
    let index = printset::sheet_index(&classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FIRST FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["SECOND FLOOR PLAN"], "")),
        ("Set.pdf", 3, sheet("A103", &["THIRD FLOOR PLAN"], "")),
        ("Set.pdf", 4, sheet("A104", &["ROOF PLAN"], "")),
        ("Set.pdf", 5, section),
    ]));
    assert!(index.level_warnings.is_empty(), "{:?}", index.level_warnings);
    assert_eq!(
        levels_of(&index),
        [lv("1", Some(0.0), Some(10.5)), lv("2", Some(10.5), Some(10.5)), lv("3", Some(21.0), Some(9.08)), lv("R", Some(30.08), None)]
    );
    assert_eq!(index.levels.last().map(|l| l.elevation_source.as_str()), Some("top of plate"));
    // One storey: its elevations mark the finished floor and the top of plate, no level number.
    let elevations = marked("A201", &["EXTERIOR ELEVATIONS"], &[("FINISH FLOOR", "100'-0\""), ("TOP OF PLATE", "112'-0\"")]);
    let index = printset::sheet_index(&classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["ROOF PLAN"], "")),
        ("Set.pdf", 3, elevations),
    ]));
    assert_eq!(levels_of(&index), [lv("1", Some(0.0), Some(12.0)), lv("R", Some(12.0), None)]);
    assert_eq!(index.levels.first().map(|l| l.elevation_source.as_str()), Some("finish floor"));
}

/// A truss bearing marked on the sections is where the roof bears (no roof level marked), and
/// the tops of parapet marked above it are the roof's parapets, lowest first, as heights above
/// it (Q6314: the roof stood a default storey over the top floor and had no parapet).
#[test]
fn a_truss_bearing_is_the_roofs_elevation_and_parapet_tops_its_parapets() {
    let section = marked(
        "A301",
        &["BUILDING SECTIONS"],
        &[
            ("SECOND FLOOR", "110'-0\""),
            ("TRUSS BEARING", "119'-1 1/2\""),
            ("T.O. PARAPET (LOW)", "123'-7 1/2\""),
            ("T.O. PARAPET (HIGH)", "127'-7 1/2\""),
        ],
    );
    let elevation = marked("A201", &["EXTERIOR ELEVATIONS"], &[("FIRST FLOOR", "100'-0\""), ("T.O. PARAPET (LOW)", "123'-7 1/2\"")]);
    let index = printset::sheet_index(&classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FIRST FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["SECOND FLOOR PLAN"], "")),
        ("Set.pdf", 3, sheet("A103", &["ROOF PLAN"], "")),
        ("Set.pdf", 4, section),
        ("Set.pdf", 5, elevation),
    ]));
    assert!(index.level_warnings.is_empty(), "{:?}", index.level_warnings);
    assert_eq!(levels_of(&index), [lv("1", Some(0.0), Some(10.0)), lv("2", Some(10.0), Some(9.13)), lv("R", Some(19.13), None)]);
    let roof = index.levels.last().expect("roof");
    assert_eq!(roof.elevation_source, "top of plate");
    assert_eq!(roof.parapets_ft, [4.5, 8.5], "{roof:?}");
    assert!(index.levels.iter().take(2).all(|l| l.parapets_ft.is_empty()));
}

/// fix-parapet-one-height-per-roof: tops of parapet on a sheet of exterior elevations carry the
/// facade of the view title nearest them; a section's carry none. Every top is kept by facade.
#[test]
fn tops_of_parapet_keep_the_facade_their_elevation_shows() {
    let section = marked("A301", &["BUILDING SECTIONS"], &[("SECOND FLOOR", "110'-0\""), ("TRUSS BEARING", "119'-1 1/2\"")]);
    let mut elevations = sheet("A201", &["EXTERIOR ELEVATIONS"], "");
    // Two elevations one over the other, each titled under it, its marks at its right end.
    for (title, y, top) in [("NORTH ELEVATION", 380.0, "127'-7 1/2\""), ("EAST ELEVATION", 140.0, "123'-7 1/2\"")] {
        text(&mut elevations, title, 300.0, y, 10.0);
        text(&mut elevations, "T.O. PARAPET", 1100.0, y + 120.0, 6.0);
        text(&mut elevations, top, 1100.0, y + 112.0, 6.0);
        text(&mut elevations, "FIRST FLOOR", 1100.0, y + 30.0, 6.0);
        text(&mut elevations, "100'-0\"", 1100.0, y + 22.0, 6.0);
    }
    let index = printset::sheet_index(&classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FIRST FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["SECOND FLOOR PLAN"], "")),
        ("Set.pdf", 3, sheet("A103", &["ROOF PLAN"], "")),
        ("Set.pdf", 4, section),
        ("Set.pdf", 5, elevations),
    ]));
    let roof = index.levels.last().expect("roof");
    assert_eq!(roof.parapets_ft, [4.5, 8.5], "{roof:?}");
    let tops: Vec<(f64, Option<&str>)> = roof.parapet_tops.iter().map(|t| (t.height_ft, t.facade.as_deref())).collect();
    assert_eq!(tops, [(4.5, Some("east")), (8.5, Some("north"))], "{roof:?}");
}

/// A marker whose name is long prints its elevation under the name's far end, farther from the
/// name's start than a short marker's: it is still the marker's, and the next marker up keeps
/// its own.
#[test]
fn a_long_markers_elevation_under_its_far_end_is_read() {
    let mut section = sheet("A302", &["WALL SECTIONS"], "");
    // The floors and `TRUSS BEARING` short, the elevation under them; the parapets' names
    // long, their elevations 60 to the right (10 letters' worth) and a line under.
    for (name, el, y, dx) in [
        ("FIRST FLOOR", "100'-0\"", 60.0, 0.0),
        ("SECOND FLOOR", "110'-0\"", 120.0, 0.0),
        ("TRUSS BEARING", "119'-1 1/2\"", 180.0, 0.0),
        ("T.O. PARAPET (LOW)", "123'-7 1/2\"", 240.0, 60.0),
        ("T.O. PARAPET (HIGH)", "125'-7 1/2\"", 262.0, 60.0),
    ] {
        text(&mut section, name, 950.0, y, 6.0);
        text(&mut section, el, 950.0 + dx, y - 8.0, 6.0);
    }
    let index = printset::sheet_index(&classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FIRST FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["SECOND FLOOR PLAN"], "")),
        ("Set.pdf", 3, sheet("A103", &["ROOF PLAN"], "")),
        ("Set.pdf", 4, section),
    ]));
    let roof = index.levels.last().expect("roof");
    assert_eq!((roof.name.as_str(), roof.elevation_ft.map(|v| (v * 100.0).round() / 100.0)), ("R", Some(19.13)), "{:?}", index.levels);
    assert_eq!(roof.parapets_ft, [4.5, 6.5], "{roof:?}");
}

/// A sheet printing a plan beside its details (`OVERALL ROOF PLAN & DETAILS`) is that plan: it
/// goes on its level. A sheet of details only (`ROOF DETAILS`, `PLAN DETAILS`) stays a detail.
#[test]
fn a_plan_printed_with_its_details_is_a_plan_of_its_level() {
    let project = classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FIRST FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["SECOND FLOOR PLAN"], "")),
        ("Set.pdf", 3, sheet("A105", &["OVERALL ROOF PLAN &", "DETAILS"], "")),
        ("Set.pdf", 4, sheet("A501", &["ROOF DETAILS"], "")),
        ("Set.pdf", 5, sheet("A502", &["PLAN DETAILS"], "")),
        ("Set.pdf", 6, sheet("A301", &["FLOOR PLAN AND", "WALL SECTIONS"], "")),
    ]);
    let index = printset::sheet_index(&project);
    let kind = |n: &str| index.sheets.iter().find(|s| s.number == n).map(|s| (s.sheet_type.clone(), s.level.clone())).unwrap_or_default();
    assert_eq!(kind("A105"), ("plan".to_string(), "R".to_string()), "{:?}", index.sheets);
    assert_eq!(kind("A501").0, "detail");
    assert_eq!(kind("A502").0, "detail");
    assert_eq!(kind("A301").0, "plan");
    let roof = index.levels.iter().find(|l| l.name == "R").map(|l| l.sheets.clone()).unwrap_or_default();
    assert_eq!(roof, [serde_json::json!({"sheet": "A105", "typical": false})]);
}

/// Marks that give one level different elevations: the most marks win and the others are
/// reported, never used silently.
#[test]
fn marks_that_disagree_are_reported() {
    let project = classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FIRST FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["SECOND FLOOR PLAN"], "")),
        ("Set.pdf", 3, marked("A301", &["BUILDING SECTIONS"], &[("FIRST FLOOR", "100'-0\""), ("SECOND FLOOR", "110'-6\"")])),
        ("Set.pdf", 4, marked("A302", &["BUILDING SECTIONS"], &[("FIRST FLOOR", "100'-0\""), ("SECOND FLOOR", "110'-6\"")])),
        ("Set.pdf", 5, marked("A303", &["WALL SECTIONS"], &[("FIRST FLOOR", "100'-0\""), ("SECOND FLOOR", "111'-0\"")])),
    ]);
    let index = printset::sheet_index(&project);
    assert_eq!(levels_of(&index), [lv("1", Some(0.0), Some(10.5)), lv("2", Some(10.5), None)]);
    assert_eq!(index.level_warnings.len(), 1, "{:?}", index.level_warnings);
    assert!(index.level_warnings[0].starts_with("level 2: 111'-0\" not used"), "{:?}", index.level_warnings);
}

/// What the index says of each entry, in order.
fn entries_of(project: &PrintSetProject) -> Vec<(String, u32, String, String, String, String, String, Option<String>)> {
    project
        .sheets
        .iter()
        .map(|e| {
            let classify = e.stage_results.get("classify");
            (
                e.file.clone(),
                e.page,
                e.number.clone(),
                e.title.clone(),
                e.level.clone(),
                e.superseded_by.clone(),
                classify.map(|r| format!("{:?}", r.status)).unwrap_or_default(),
                classify.and_then(|r| r.error.clone()),
            )
        })
        .collect()
}

#[test]
fn classify_reads_several_pages_at_once_and_keeps_the_sets_order() {
    // A combined set and a discipline set repeating part of it with a later date, a page that
    // can't be read, and plan sheets naming their levels: classify reads the pages on several
    // threads at once (a dense page takes seconds), and the index is the one a page-by-page read
    // gives.
    let mut prints: BTreeMap<(String, u32), Drawing> = BTreeMap::new();
    let mut entries: Vec<SheetEntry> = Vec::new();
    let floors = ["FIRST", "SECOND", "THIRD", "FOURTH"];
    for page in 1..=24u32 {
        let floor = floors[(page as usize) % floors.len()];
        let d = sheet(&format!("M{}{:02}", 5 + page / 10, page), &[floor, "FLOOR HVAC PLAN"], "03/02/2026");
        prints.insert(("Combined Set.pdf".into(), page), d);
        entries.push(SheetEntry { file: "Combined Set.pdf".into(), page, dirty: true, ..SheetEntry::default() });
    }
    for page in 1..=6u32 {
        let d = sheet(&format!("M5{page:02}"), &[floors[(page as usize) % floors.len()], "FLOOR HVAC PLAN"], "04/15/2026");
        prints.insert(("Mechanical Set.pdf".into(), page), d);
        entries.push(SheetEntry { file: "Mechanical Set.pdf".into(), page, dirty: true, ..SheetEntry::default() });
    }
    entries.insert(10, SheetEntry { file: "Combined Set.pdf".into(), page: 99, dirty: true, ..SheetEntry::default() });
    let run = |load: &(dyn Fn(&SheetEntry) -> Result<Drawing, String> + Sync)| {
        let mut project = PrintSetProject { sheets: entries.clone(), ..PrintSetProject::default() };
        let mut seen = Vec::new();
        printset::run_classify_prints(&mut project, load, &AtomicBool::new(false), &mut |p| seen.push(p.sheet_index));
        (project, seen)
    };
    let print = |e: &SheetEntry| prints.get(&(e.file.clone(), e.page)).cloned().ok_or_else(|| "no such page".to_string());

    // one page at a time
    let one = std::sync::Mutex::new(());
    let (serial, _) = run(&|e| {
        let _only = one.lock();
        print(e)
    });
    // as classify reads them: how many loads run at once
    let running = std::sync::atomic::AtomicUsize::new(0);
    let most = std::sync::atomic::AtomicUsize::new(0);
    let (parallel, seen) = run(&|e| {
        let now = running.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        most.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(20));
        running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        print(e)
    });
    if std::thread::available_parallelism().map_or(1, |n| n.get()) > 1 {
        assert!(most.load(std::sync::atomic::Ordering::SeqCst) > 1, "the pages were read one at a time");
    }
    assert_eq!(entries_of(&parallel), entries_of(&serial));
    assert_eq!(seen, (0..entries.len()).collect::<Vec<_>>(), "progress in the set's order");
    assert_eq!(parallel.sheets.iter().filter(|e| e.superseded_by.is_empty() && !e.number.is_empty()).count(), 24, "{:?}", entries_of(&parallel));
    assert_eq!(parallel.sheets.iter().filter(|e| !e.superseded_by.is_empty()).count(), 6, "the later set's copies are current");
    assert_eq!(entries_of(&parallel).iter().filter(|e| e.6 == "Failed").map(|e| e.7.clone()).collect::<Vec<_>>(), [Some("no such page".to_string())]);
}

/// A building of one storey with a mezzanine (Q6322): the mezzanine's plans of every trade are on
/// its own level, ranked over the floor and under the roof; the building is still of one storey,
/// so a plan naming no level is on its floor; the floor's height runs past the mezzanine to the
/// roof, the mezzanine's from its own mark up to the roof.
#[test]
fn a_mezzanine_is_a_level_inside_its_storey() {
    let project = classify(vec![
        ("Set.pdf", 1, sheet("A101", &["FLOOR PLAN"], "")),
        ("Set.pdf", 2, sheet("A102", &["MEZZANINE PLAN"], "")),
        ("Set.pdf", 3, sheet("A103", &["ROOF PLAN"], "")),
        ("Set.pdf", 4, sheet("M102", &["MECHANICAL", "MEZZANINE PLAN"], "")),
        ("Set.pdf", 5, sheet("E101", &["POWER PLAN"], "")),
        ("Set.pdf", 6, sheet("E102", &["LIGHTING MEZZ. PLAN"], "")),
        ("Set.pdf", 7, marked("A301", &["BUILDING SECTIONS"], &[("FIRST FLOOR", "100'-0\""), ("MEZZANINE", "110'-6\""), ("ROOF", "122'-0\"")])),
    ]);
    let index = printset::sheet_index(&project);
    assert_eq!(levels_of(&index), [lv("1", Some(0.0), Some(22.0)), lv("M", Some(10.5), Some(11.5)), lv("R", Some(22.0), None)]);
    for (n, level, source) in
        [("A102", "M", "title"), ("M102", "M", "title"), ("E102", "M", "title"), ("A101", "1", "default"), ("E101", "1", "default")]
    {
        let s = index.sheets.iter().find(|s| s.number == n).expect("sheet");
        assert_eq!((s.level.as_str(), s.level_source.as_str()), (level, source), "{n}");
    }
    let m = index.levels.iter().find(|l| l.name == "M").expect("mezzanine");
    assert_eq!(m.sheets.len(), 3, "{:?}", m.sheets);
}

/// A sheet title that lists parts (Q6514: `FLOOR PLAN/ DETAILS/ SCHEDULES`) names everything on
/// the sheet: a part naming a plan makes the sheet that plan, so the set's only floor plan is the
/// floor's plan and a finish or tile plan doesn't stand in for it.
#[test]
fn a_title_listing_a_plan_and_other_drawings_is_that_plan() {
    let project = classify(vec![
        ("Set.pdf", 1, sheet("A1.1", &["FLOOR PLAN/ DETAILS/ SCHEDULES"], "")),
        ("Set.pdf", 2, sheet("A1.2", &["FLOOR TILE PLAN"], "")),
        ("Set.pdf", 3, sheet("A2.1", &["REFLECTED CEILING PLAN/ CEILING DETAILS"], "")),
        ("Set.pdf", 4, sheet("A3.1", &["ROOF PLAN/ ROOF DETAILS"], "")),
        ("Set.pdf", 5, sheet("A4.1", &["CANOPY PLAN/ ELEVATIONS"], "")),
        ("Set.pdf", 6, sheet("A5.1", &["EQUIPMENT/ ENLARGED STORAGE/ LIFE SAFETY"], "")),
        ("Set.pdf", 7, sheet("A6.1", &["DOOR/ WINDOW SCHEDULES"], "")),
    ]);
    let index = printset::sheet_index(&project);
    let of = |n: &str| index.sheets.iter().find(|s| s.number == n).unwrap_or_else(|| panic!("{n}: {:?}", index.sheets));
    let filed: Vec<(&str, &str, &str)> = index.sheets.iter().map(|s| (s.number.as_str(), s.sheet_type.as_str(), s.level.as_str())).collect();
    for (n, want) in [("A1.1", "plan"), ("A1.2", "plan"), ("A2.1", "rcp"), ("A3.1", "plan"), ("A4.1", "plan"), ("A5.1", "plan"), ("A6.1", "schedule")]
    {
        assert_eq!(of(n).sheet_type, want, "{n} {:?}: {filed:?}", of(n).title);
    }
    // The floor plan sheet is on the building's one floor, beside the tile plan.
    let floor = index.levels.iter().find(|l| l.name == "1").unwrap_or_else(|| panic!("no level 1: {:?}", index.levels));
    let on: Vec<&str> = floor.sheets.iter().filter_map(|s| s["sheet"].as_str()).collect();
    assert!(on.contains(&"A1.1"), "{on:?}");
    assert_eq!(of("A1.1").level, "1");
    // The whole-floor equipment plan is on that floor too (its life-safety part doesn't make it a
    // reference sheet of no level).
    assert_eq!(of("A5.1").level, "1");
}

/// Q6514: a sheet titled as a floor plan that is still filed as something else (a title of one
/// part, `FLOOR PLAN NOTES`) while the floor has no sheet titled as its floor plan is listed, to
/// ask; a set whose floor has its floor plan lists nothing.
#[test]
fn a_floor_plan_title_filed_otherwise_on_a_floor_without_one_is_listed() {
    let project = classify(vec![("Set.pdf", 1, sheet("A1.1", &["FLOOR PLAN NOTES"], "")), ("Set.pdf", 2, sheet("A1.2", &["FLOOR TILE PLAN"], ""))]);
    let index = printset::sheet_index(&project);
    let listed: Vec<(&str, &str, &str)> =
        index.floor_plans_filed_otherwise.iter().map(|f| (f.sheet.as_str(), f.sheet_type.as_str(), f.level.as_str())).collect();
    assert_eq!(listed, [("A1.1", "schedule", "1")], "{:?}", index.sheets);

    let project = classify(vec![
        ("Set.pdf", 1, sheet("A1.1", &["FLOOR PLAN NOTES"], "")),
        ("Set.pdf", 2, sheet("A1.2", &["FLOOR TILE PLAN"], "")),
        ("Set.pdf", 3, sheet("A1.3", &["FLOOR PLAN/ DETAILS/ SCHEDULES"], "")),
    ]);
    let index = printset::sheet_index(&project);
    assert!(index.floor_plans_filed_otherwise.is_empty(), "{:?}", index.floor_plans_filed_otherwise);
    let plan = index.sheets.iter().find(|s| s.number == "A1.3").expect("A1.3");
    assert_eq!((plan.sheet_type.as_str(), plan.views.clone()), ("plan", vec!["detail".to_string(), "schedule".to_string()]));
}
