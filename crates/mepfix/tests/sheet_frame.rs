//! A sheet's frame (Q5273): the border and the title-block strip are found from their rules, so
//! what is drawn there (firm names, addresses, review stamps, revisions) is never read as the
//! building; a plan's walls are never taken for a title-block rule. Civil, landscape and site
//! plans are on the site in the sheet index, never on the lowest storey.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use cadaddy_doc::{Common, Drawing, EntityKind, HAlign, Line, Space, Text, VAlign};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_mepfix::printset::{self, PrintSetProject, SheetEntry, StageStatus};
use cadaddy_mepfix::sheet_facts::{is_site_sheet, sheet_frame};

fn v(x: f64, y: f64) -> Vec2 {
    Vec2::new(x, y)
}

fn rect(segs: &mut Vec<(Vec2, Vec2)>, x0: f64, y0: f64, x1: f64, y1: f64) {
    segs.extend([(v(x0, y0), v(x1, y0)), (v(x1, y0), v(x1, y1)), (v(x1, y1), v(x0, y1)), (v(x0, y1), v(x0, y0))]);
}

/// A 42" × 30" sheet: a double border, a floor plan whose long walls run most of the sheet's
/// height and width (never from border to border), and the title block given by `strip`.
fn sheet(strip: &str) -> Vec<(Vec2, Vec2)> {
    let mut s = Vec::new();
    rect(&mut s, 0.5, 0.5, 41.5, 29.5);
    rect(&mut s, 0.75, 0.75, 41.25, 29.25);
    // The plan: a long building, its walls 80% of the sheet's height and 75% of its width.
    rect(&mut s, 3.0, 4.0, 33.0, 26.0);
    rect(&mut s, 3.1, 4.1, 32.9, 25.9);
    s.push((v(18.0, 4.1), v(18.0, 25.9)));
    match strip {
        // Two full-height rules near the right, rows between them and the border.
        "right" => {
            s.push((v(37.0, 0.75), v(37.0, 29.25)));
            s.push((v(37.2, 0.75), v(37.2, 29.25)));
            for y in [5.0, 10.0, 15.0, 20.0] {
                s.push((v(37.2, y), v(41.25, y)));
            }
        }
        // A full-width rule along the bottom, drawn in pieces (as plotted), boxes above it.
        "bottom" => {
            s.push((v(0.75, 3.0), v(20.0, 3.0)));
            s.push((v(20.0, 3.0), v(41.25, 3.0)));
            for x in [10.0, 20.0, 30.0] {
                s.push((v(x, 0.75), v(x, 3.0)));
            }
        }
        _ => {}
    }
    s
}

#[test]
fn the_title_block_strip_on_the_right_is_off_the_drawing_area() {
    let f = sheet_frame(&sheet("right"), [0.0, 0.0, 42.0, 30.0]);
    assert!(f.border, "{f:?}");
    let tb = f.title_block.expect("title block");
    assert!((tb[0] - 37.0).abs() < 1e-6, "the strip starts at its leftmost full-height rule: {f:?}");
    let a = f.area;
    assert!(a[0] > 0.75 && a[1] > 0.75 && a[3] < 29.25 && a[2] < 37.0, "inside the border, left of the strip: {f:?}");
    assert!(a[0] < 3.0 && a[1] < 4.0 && a[2] > 33.0 && a[3] > 26.0, "the whole plan is in it: {f:?}");
}

#[test]
fn a_title_block_along_the_bottom_is_found_from_a_rule_drawn_in_pieces() {
    let f = sheet_frame(&sheet("bottom"), [0.0, 0.0, 42.0, 30.0]);
    let tb = f.title_block.expect("title block");
    assert!((tb[3] - 3.0).abs() < 1e-6, "{f:?}");
    assert!(f.area[1] > 3.0 && f.area[1] < 4.0, "above the strip, below the plan: {f:?}");
    assert!(f.area[2] > 33.0, "{f:?}");
}

#[test]
fn a_plan_s_long_walls_are_never_a_title_block_rule() {
    // No title block: the walls (80% of the height, 75% of the width) are not taken for one.
    let f = sheet_frame(&sheet("none"), [0.0, 0.0, 42.0, 30.0]);
    assert!(f.border);
    assert_eq!(f.title_block, None, "{f:?}");
    assert!(f.area[2] > 41.0 && f.area[3] > 29.0, "{f:?}");
    // No border either (a cropped plan): the whole sheet.
    let mut plan = Vec::new();
    rect(&mut plan, 3.0, 4.0, 33.0, 26.0);
    let f = sheet_frame(&plan, [0.0, 0.0, 42.0, 30.0]);
    assert_eq!((f.area, f.title_block, f.border), ([0.0, 0.0, 42.0, 30.0], None, false));
}

#[test]
fn hostile_frames_never_panic() {
    let nan = f64::NAN;
    let inf = f64::INFINITY;
    let junk = vec![(v(nan, 0.0), v(1.0, nan)), (v(inf, 0.0), v(-inf, 0.0)), (v(0.0, 0.0), v(0.0, 0.0)), (v(1e300, -1e300), v(-1e300, 1e300))];
    for r in [[0.0, 0.0, 42.0, 30.0], [nan, 0.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0], [5.0, 5.0, -5.0, -5.0], [-inf, -inf, inf, inf]] {
        let f = sheet_frame(&junk, r);
        assert!(f.area.iter().all(|x| !x.is_nan()) || r.iter().any(|x| x.is_nan()), "{r:?} {f:?}");
    }
    // Many rules: capped, no panic.
    let many: Vec<(Vec2, Vec2)> = (0..200_000).map(|i| (v(f64::from(i % 997) * 0.04, 0.0), v(f64::from(i % 997) * 0.04, 30.0))).collect();
    let _ = sheet_frame(&many, [0.0, 0.0, 42.0, 30.0]);
}

#[test]
fn site_sheets_by_discipline_and_title() {
    assert!(is_site_sheet("C", "DIMENSION CONTROL PLAN"));
    assert!(is_site_sheet("L", "LAYOUT PLAN"));
    assert!(is_site_sheet("A", "ARCHITECTURAL SITE PLAN"));
    assert!(is_site_sheet("E", "SITE LIGHTING PLAN"));
    assert!(is_site_sheet("", "LANDSCAPE & IRRIGATION PLAN"));
    assert!(!is_site_sheet("A", "OVERALL FIRST FLOOR PLAN"));
    assert!(!is_site_sheet("A", "OPPOSITE HAND UNIT PLAN"), "a word, not a substring");
    assert!(!is_site_sheet("E", "SECOND FLOOR POWER PLAN"));
}

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

/// A plotted sheet: some linework and a title block naming it.
fn plotted(number: &str, title: &[&str]) -> Drawing {
    let mut d = Drawing::default();
    for i in 0..6 {
        let x = 100.0 + 150.0 * f64::from(i);
        add(&mut d, EntityKind::Line(Line { a: Vec3::new(x, 100.0, 0.0), b: Vec3::new(x + 120.0, 100.0, 0.0) }));
    }
    text(&mut d, "DRAWING TITLE:", 1500.0, 300.0, 8.0);
    for (i, t) in title.iter().enumerate() {
        text(&mut d, t, 1500.0, 285.0 - 13.0 * i as f64, 8.0);
    }
    text(&mut d, number, 1500.0, 200.0, 30.0);
    d
}

#[test]
fn civil_landscape_and_site_plans_are_on_the_site_never_the_lowest_storey() {
    let pages = vec![
        ("Full Set.pdf", 1, plotted("A111", &["OVERALL FIRST", "FLOOR PLAN"])),
        ("Full Set.pdf", 2, plotted("A112", &["OVERALL SECOND", "FLOOR PLAN"])),
        ("Civil Set.pdf", 1, plotted("C3.1", &["DIMENSION CONTROL PLAN"])),
        ("Civil Set.pdf", 2, plotted("L1.0", &["LAYOUT PLAN"])),
        ("Full Set.pdf", 3, plotted("A010", &["ARCHITECTURAL", "SITE PLAN"])),
        ("Plumb Set.pdf", 1, plotted("P-201", &["PLUMBING PLAN"])),
    ];
    let mut prints: BTreeMap<(String, u32), Drawing> = BTreeMap::new();
    let mut project = PrintSetProject::default();
    for (file, page, d) in pages {
        project.sheets.push(SheetEntry { file: file.into(), page, dirty: true, ..SheetEntry::default() });
        prints.insert((file.into(), page), d);
    }
    let cancel = AtomicBool::new(false);
    let load = |e: &SheetEntry| prints.get(&(e.file.clone(), e.page)).cloned().ok_or_else(|| "no print".to_string());
    assert_eq!(printset::run_classify_prints(&mut project, &load, &cancel, &mut |_| {}).status, StageStatus::Completed);
    let index = printset::sheet_index(&project);
    let level = |n: &str| index.sheets.iter().find(|s| s.number == n).map(|s| s.level.clone()).unwrap_or_else(|| panic!("{n}: {:?}", index.sheets));
    assert_eq!(level("A111"), "1");
    assert_eq!(level("C3.1"), "SITE");
    assert_eq!(level("L1.0"), "SITE");
    assert_eq!(level("A010"), "SITE");
    // A building plan naming no level still takes the lowest, to be confirmed.
    assert_eq!(level("P-201"), "1");
    assert_eq!(index.plans_without_level, ["P-201"]);
    let names: Vec<&str> = index.levels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["1", "2"], "the site is no storey");
}
