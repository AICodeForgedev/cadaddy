//! The print set's links on the made-up set of [`cadaddy_mepfix::linkset`]: read straight off its
//! drawings, and printed to PDF and imported page by page as a job reads them.

use cadaddy_mepfix::linkset;
use cadaddy_mepfix::printlinks::{self, LinkPage, SetSheet, SheetLinks};

fn the_set() -> (Vec<SetSheet>, Vec<LinkPage>) {
    let sheets = linkset::sheets();
    let pages = linkset::drawings()
        .iter()
        .map(|(n, d)| LinkPage { sheet: sheets.iter().find(|s| s.number == *n).cloned().unwrap_or_default(), read: printlinks::read_page(d, n) })
        .collect();
    (sheets, pages)
}

fn links() -> SheetLinks {
    let (sheets, pages) = the_set();
    printlinks::link_set(&sheets, &pages)
}

#[test]
fn every_callout_resolves_but_the_broken_one() {
    let l = links();
    let got: Vec<(String, String, String, String, bool)> =
        l.callouts.iter().map(|c| (c.on_sheet.clone(), c.kind.clone(), c.detail.clone(), c.target_sheet.clone(), c.resolved)).collect();
    let want: Vec<(String, String, String, String, bool)> = [
        ("E-1", "enlarged_plan", "1", "E-3", true),
        ("E-1", "detail_bubble", "2", "E-4", true),
        ("E-1", "detail_bubble", "5", "E-9", false),
        ("E-1", "discipline_ref", "", "A", true),
        ("E-1", "sheet_ref", "", "E-4", true),
        ("E-1", "match_line", "", "E-2", true),
        ("E-2", "match_line", "", "E-1", true),
    ]
    .iter()
    .map(|(a, b, c, d, e)| (a.to_string(), b.to_string(), c.to_string(), d.to_string(), *e))
    .collect();
    let mut got_sorted = got.clone();
    got_sorted.sort();
    let mut want_sorted = want.clone();
    want_sorted.sort();
    assert_eq!(got_sorted, want_sorted, "{got:#?}");
    assert_eq!(l.resolved, 6);
    assert_eq!(l.resolved_pct, 0.857);
    assert_eq!(l.unresolved.len(), 1);
    assert_eq!((l.unresolved[0].on_sheet.as_str(), l.unresolved[0].target_sheet.as_str(), l.unresolved[0].detail.as_str()), ("E-1", "E-9", "5"));
    // The details the bubbles name are on their sheets.
    let detail = l.callouts.iter().find(|c| c.detail == "2").map(|c| c.detail_found);
    assert_eq!(detail, Some(Some(true)));
    // View-number bubbles (each view title's own) are not links.
    assert!(l.callouts.iter().all(|c| c.on_sheet != "E-3" && c.on_sheet != "E-4"), "{:#?}", l.callouts);
    assert_eq!(l.sheets.len(), 6);
}

#[test]
fn the_enlarged_plan_registers_into_its_boundary_and_its_devices_count_once() {
    let l = links();
    assert_eq!(l.enlarged.len(), 1, "{:#?}", l.enlarged);
    let e = &l.enlarged[0];
    assert_eq!((e.overall_sheet.as_str(), e.sheet.as_str(), e.detail.as_str(), e.level.as_str()), ("E-1", "E-3", "1", "1"));
    assert!(e.registered);
    assert_eq!(e.method, "grids");
    assert_eq!(e.scale, Some(2.0));
    assert_eq!(e.boundary, Some([7.5, 3.5, 16.5, 12.5]));
    assert_eq!(e.devices, 5);
    assert_eq!(e.overall_ignored, 3);
    assert_eq!(l.enlarged_registered, 1);
    assert_eq!(l.double_counted, 0);
    // Level 1: E-1's 7 outside the core, E-2's 4, the enlarged plan's 5.
    assert_eq!(l.levels.len(), 1, "{:#?}", l.levels);
    assert_eq!(l.levels[0].devices, 16);
    assert_eq!(l.levels[0].sheets, vec!["E-1".to_string(), "E-2".to_string()]);
}

#[test]
fn the_match_line_stitches_one_level() {
    let l = links();
    assert_eq!(l.match_lines.len(), 1, "{:#?}", l.match_lines);
    let m = &l.match_lines[0];
    assert_eq!(m.sheets, ["E-1".to_string(), "E-2".to_string()]);
    assert_eq!(m.level, "1");
    assert!(m.stitched);
    assert_eq!(m.offset, Some([14.0, 0.0]));
    assert!(l.levels[0].stitched);
}

#[test]
fn the_detail_height_lands_on_the_device_its_bubble_points_from() {
    let l = links();
    assert_eq!(l.facts.len(), 1, "{:#?}", l.facts);
    let f = &l.facts[0];
    assert_eq!((f.on_sheet.as_str(), f.target_sheet.as_str(), f.detail.as_str(), f.kind.as_str()), ("E-1", "E-4", "2", "mounting_height"));
    assert_eq!(f.value_ft, 1.5);
    assert_eq!(f.applied_to.len(), 1);
    assert_eq!(f.applied_to[0]["tag"], "F");
}

/// Without the enlarged sheet's registration the overall plan's copy would count: a set whose
/// enlarged plan names no grid and draws no boundary is reported unregistered, never silently
/// added twice.
#[test]
fn an_enlarged_plan_that_cannot_be_placed_is_reported_unregistered() {
    let (sheets, mut pages) = the_set();
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-3") {
        for v in &mut p.read.views {
            v.grid_x.clear();
            v.grid_y.clear();
        }
    }
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-1") {
        p.read.boxes.clear();
        for c in &mut p.read.callouts {
            c.boundary = None;
        }
    }
    let l = printlinks::link_set(&sheets, &pages);
    let e = &l.enlarged[0];
    assert!(!e.registered, "{e:#?}");
    assert_eq!(e.overall_sheet, "E-1");
    assert_eq!(l.enlarged_registered, 0);
}

/// The same set printed to PDF and imported page by page, as `printset.links` reads a job.
fn printed_set() -> (Vec<SetSheet>, Vec<LinkPage>) {
    let sheets = linkset::sheets();
    let pages = linkset::drawings()
        .iter()
        .map(|(n, d)| {
            let pdf = linkset::print(d).unwrap_or_else(|e| panic!("{n}: {e}"));
            let (page, _) = cadaddy_io::pdf_read::read_json(&pdf, &serde_json::json!({"page": 1, "images": false, "fills": "merge"}))
                .unwrap_or_else(|e| panic!("{n}: {e}"));
            LinkPage { sheet: sheets.iter().find(|s| s.number == *n).cloned().unwrap_or_default(), read: printlinks::read_page(&page, n) }
        })
        .collect();
    (sheets, pages)
}

/// Printed and imported, the pages come back at their own scale notes (E-1 and E-2 at 1/8",
/// E-3 at 1/4": building inches on every sheet), the bubbles' circles taken for lettering by the
/// import and the text doubled (hidden text and strokes read back): the links come out the same.
#[test]
fn a_printed_set_links_the_same_way() {
    let (sheets, pages) = printed_set();
    let l = printlinks::link_set(&sheets, &pages);
    let mut got: Vec<(String, String, String, String, bool)> =
        l.callouts.iter().map(|c| (c.on_sheet.clone(), c.kind.clone(), c.detail.clone(), c.target_sheet.clone(), c.resolved)).collect();
    got.sort();
    let mut want: Vec<(String, String, String, String, bool)> = [
        ("E-1", "detail_bubble", "2", "E-4", true),
        ("E-1", "detail_bubble", "5", "E-9", false),
        ("E-1", "discipline_ref", "", "A", true),
        ("E-1", "enlarged_plan", "1", "E-3", true),
        ("E-1", "match_line", "", "E-2", true),
        ("E-1", "sheet_ref", "", "E-4", true),
        ("E-2", "match_line", "", "E-1", true),
    ]
    .iter()
    .map(|(a, b, c, d, e)| (a.to_string(), b.to_string(), c.to_string(), d.to_string(), *e))
    .collect();
    want.sort();
    assert_eq!(got, want);
    assert_eq!(l.unresolved.len(), 1);
    let e = &l.enlarged[..];
    assert_eq!(e.len(), 1, "{e:#?}");
    let e = &e[0];
    assert_eq!((e.overall_sheet.as_str(), e.sheet.as_str(), e.detail.as_str(), e.level.as_str()), ("E-1", "E-3", "1", "1"));
    assert!(e.registered);
    assert_eq!(e.method, "grids");
    // Both sheets in building inches: one to one.
    assert_eq!(e.scale, Some(1.0));
    assert_eq!(e.boundary, Some([7.5 * 96.0, 3.5 * 96.0, 16.5 * 96.0, 12.5 * 96.0]));
    assert_eq!((e.devices, e.overall_ignored), (5, 3));
    assert_eq!(l.double_counted, 0);
    assert_eq!(l.levels.len(), 1);
    assert_eq!(l.levels[0].devices, 16);
    assert!(l.levels[0].stitched);
    assert_eq!(l.match_lines.len(), 1);
    assert_eq!(l.match_lines[0].offset, Some([14.0 * 96.0, 0.0]));
    assert_eq!(l.facts.len(), 1);
    assert_eq!(l.facts[0].value_ft, 1.5);
    assert_eq!(l.facts[0].applied_to.first().map(|a| a["tag"].clone()), Some(serde_json::json!("F")));
}

/// The set with E-3 drawn by `e3` in place of its own.
fn with_e3(e3: cadaddy_doc::Drawing) -> (Vec<SetSheet>, Vec<LinkPage>) {
    let (sheets, mut pages) = the_set();
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-3") {
        p.read = printlinks::read_page(&e3, "E-3");
    }
    (sheets, pages)
}

/// A view titled with its level but no scale line under it is still a view, and still the
/// enlarged plan.
#[test]
fn an_enlarged_view_without_a_scale_line_is_found_by_its_title() {
    let (sheets, pages) = with_e3(linkset::enlarged_sheet(Some(("ENLARGED FIRST FLOOR CORE PLAN", false))));
    let l = printlinks::link_set(&sheets, &pages);
    assert_eq!(l.enlarged.len(), 1, "{:#?}", l.enlarged);
    let e = &l.enlarged[0];
    assert_eq!((e.detail.as_str(), e.level.as_str(), e.overall_sheet.as_str()), ("1", "1", "E-1"));
    assert!(e.registered);
    assert_eq!((e.devices, e.overall_ignored), (5, 3));
}

/// An enlarged-plan sheet whose view has no title: its plan is the sheet's biggest block, and
/// with no callout pointing at it the overall plan is the one sharing its grid lines.
#[test]
fn an_untitled_enlarged_plan_finds_its_overall_plan_by_the_grid() {
    let (sheets, mut pages) = with_e3(linkset::enlarged_sheet(None));
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-1") {
        p.read.callouts.retain(|c| c.target_sheet != "E-3");
    }
    let l = printlinks::link_set(&sheets, &pages);
    assert_eq!(l.enlarged.len(), 1, "{:#?}", l.enlarged);
    let e = &l.enlarged[0];
    assert_eq!((e.sheet.as_str(), e.overall_sheet.as_str(), e.level.as_str(), e.method.as_str()), ("E-3", "E-1", "1", "grids"));
    // The only view of a sheet that numbers none is its view 1.
    assert_eq!(e.detail, "1");
    assert!(e.registered);
    assert_eq!(e.scale, Some(2.0));
    // No box to go by: the boundary is the enlarged plan's own outline, placed by the grid.
    let b = e.boundary.unwrap_or_default();
    for (got, want) in b.iter().zip([8.0, 4.0, 16.0, 12.0]) {
        assert!((got - want).abs() < 0.1, "{b:?}");
    }
    assert_eq!((e.devices, e.overall_ignored), (5, 3));
    assert_eq!(l.double_counted, 0);
}

/// The devices of one enlarged plan, as a reader of the JSON answer finds them: the entries
/// whose `on_sheet` is the enlarged sheet and whose `detail` is its view number.
fn devices_of(l: &SheetLinks, sheet: &str, detail: &str) -> u64 {
    let v = serde_json::to_value(l).expect("json");
    v["enlarged"]
        .as_array()
        .expect("enlarged")
        .iter()
        .filter(|e| e["on_sheet"] == sheet && e["detail"] == detail)
        .filter_map(|e| e["devices"].as_u64())
        .sum()
}

/// In the answer, an enlarged plan is named by its own sheet and view (`on_sheet` E-3, detail 1)
/// and `target_sheet` is the overall plan it registers on.
#[test]
fn an_enlarged_plan_is_named_by_its_own_sheet_and_view() {
    let l = links();
    let v = serde_json::to_value(&l).expect("json");
    let e = &v["enlarged"][0];
    assert_eq!((e["on_sheet"].as_str(), e["target_sheet"].as_str(), e["detail"].as_str()), (Some("E-3"), Some("E-1"), Some("1")), "{e:#}");
    assert_eq!(devices_of(&l, "E-3", "1"), 5);
    assert_eq!(devices_of(&l, "E-1", "1"), 0);
}

/// A sheet that comes twice in the set (two PDFs of the same sheet) is one enlarged plan, and its
/// devices count once.
#[test]
fn an_enlarged_sheet_printed_twice_counts_once() {
    let (sheets, mut pages) = the_set();
    let again: Vec<LinkPage> = linkset::drawings()
        .iter()
        .filter(|(n, _)| *n == "E-3")
        .map(|(n, d)| LinkPage { sheet: sheets.iter().find(|s| s.number == *n).cloned().unwrap_or_default(), read: printlinks::read_page(d, n) })
        .collect();
    assert_eq!(again.len(), 1);
    pages.extend(again);
    let l = printlinks::link_set(&sheets, &pages);
    assert_eq!(l.enlarged.len(), 1, "{:#?}", l.enlarged);
    assert_eq!(devices_of(&l, "E-3", "1"), 5);
    assert_eq!(l.double_counted, 0);
}

/// An enlarged sheet whose main plan is titled by its area alone beside small titled enlarged
/// views: the main plan is still view 1, with its own devices only, registered on the overall
/// plan; the small views keep theirs. Whether the reader finds its title (a scale line under it)
/// or not (title and scale on one line, no level named).
#[test]
fn the_main_plan_beside_titled_details_is_view_1() {
    for scale_line in [false, true] {
        main_plan_is_view_1(scale_line);
    }
}

fn main_plan_is_view_1(scale_line: bool) {
    let (sheets, pages) = with_e3(linkset::enlarged_sheet_with_details(scale_line));
    let l = printlinks::link_set(&sheets, &pages);
    let mut got: Vec<(String, usize)> = l.enlarged.iter().map(|e| (e.detail.clone(), e.devices)).collect();
    got.sort();
    assert_eq!(got, vec![("1".to_string(), 5), ("4".to_string(), 2), ("5".to_string(), 2)], "{:#?}", l.enlarged);
    assert_eq!(devices_of(&l, "E-3", "1"), 5);
    let core = l.enlarged.iter().find(|e| e.detail == "1").expect("view 1");
    assert_eq!((core.overall_sheet.as_str(), core.level.as_str()), ("E-1", "1"));
    assert!(core.registered, "{core:#?}");
    assert_eq!(core.overall_ignored, 3);
    assert!(core.title.contains("COMMON AREA CORE"), "{core:#?}");
    // Its number bubble names the view; it is not a link to the sheet itself.
    assert!(!l.callouts.iter().any(|c| c.on_sheet == "E-3" && c.target_sheet == "E-3" && c.detail == "1"), "{:#?}", l.callouts);
    assert_eq!(l.double_counted, 0);
}

/// The same sheet printed and imported (its circles taken for lettering, its text doubled): view
/// 1 is still the main plan with its own five devices.
#[test]
fn the_printed_main_plan_beside_titled_details_is_view_1() {
    let (sheets, mut pages) = printed_set();
    let pdf = linkset::print(&linkset::enlarged_sheet_with_details(false)).unwrap_or_else(|e| panic!("E-3: {e}"));
    let (page, _) = cadaddy_io::pdf_read::read_json(&pdf, &serde_json::json!({"page": 1, "images": false, "fills": "merge"}))
        .unwrap_or_else(|e| panic!("E-3: {e}"));
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-3") {
        p.read = printlinks::read_page(&page, "E-3");
    }
    let l = printlinks::link_set(&sheets, &pages);
    let mut got: Vec<(String, usize)> = l.enlarged.iter().map(|e| (e.detail.clone(), e.devices)).collect();
    got.sort();
    assert_eq!(got, vec![("1".to_string(), 5), ("4".to_string(), 2), ("5".to_string(), 2)], "{:#?}", l.enlarged);
    let core = l.enlarged.iter().find(|e| e.detail == "1").expect("view 1");
    assert_eq!(core.overall_sheet, "E-1");
    assert!(core.registered, "{core:#?}");
    assert_eq!(core.overall_ignored, 3);
    assert_eq!(l.double_counted, 0);
}

/// A schedule on another sheet lists that sheet's types only: the power marks on the enlarged
/// plan and the overall plans still count, as each sheet's own legends say.
#[test]
fn another_sheets_schedule_does_not_drop_the_enlarged_plans_devices() {
    let before = links();
    let (mut sheets, mut pages) = the_set();
    let e5 = SetSheet {
        number: "E-5".into(),
        title: "FIRST FLOOR LIGHTING PLAN".into(),
        discipline: "E".into(),
        sheet_type: "plan".into(),
        level: "1".into(),
    };
    sheets.push(e5.clone());
    let read = printlinks::read_page(&linkset::lighting_sheet(), "E-5");
    assert!(read.legend_types.contains("L1") && !read.legend_types.contains("A"), "{:?}", read.legend_types);
    pages.push(LinkPage { sheet: e5, read });
    let l = printlinks::link_set(&sheets, &pages);
    assert_eq!(devices_of(&l, "E-3", "1"), 5, "{:#?}", l.enlarged);
    assert_eq!(l.enlarged[0].overall_ignored, 3);
    assert_eq!(l.double_counted, 0);
    // The level: what it was, plus the lighting plan's two fixtures.
    let level = |l: &SheetLinks| l.levels.iter().find(|v| v.level == "1").map(|v| v.devices);
    assert_eq!(level(&l), level(&before).map(|n| n + 2));
}
