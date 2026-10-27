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

/// Lettering added to one of the set's drawings (plan text, 0.1 high).
fn letter(d: &mut cadaddy_doc::Drawing, v: &str, at: [f64; 2]) {
    let t = cadaddy_doc::Text {
        insert: cadaddy_geom::Vec3::new(at[0], at[1], 0.0),
        align_pt: None,
        height: 0.1,
        value: v.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: cadaddy_doc::HAlign::default(),
        valign: cadaddy_doc::VAlign::default(),
    };
    let common = cadaddy_doc::Common { layer: "PEN-1".into(), ..cadaddy_doc::Common::default() };
    d.add(&cadaddy_doc::Space::Model, common, cadaddy_doc::EntityKind::Text(t)).expect("text");
}

/// The set with the core's room and door numbers lettered on both E-1 and the enlarged E-3, and
/// neither grid lines on E-3's view nor a boundary box on E-1 to place it by.
fn tagged_set() -> (Vec<SetSheet>, Vec<LinkPage>) {
    let sheets = linkset::sheets();
    // E-3 (twice the scale) onto E-1: x' = x/2 + 6, y' = y/2 + 2 (their shared grid lines).
    let numbers = [("101", [7.0, 7.0]), ("102", [15.0, 7.0]), ("103", [7.0, 15.0]), ("104", [15.0, 15.0]), ("101A", [11.5, 9.0])];
    let pages = linkset::drawings()
        .into_iter()
        .map(|(n, mut d)| {
            if n == "E-3" {
                for (t, p) in numbers {
                    letter(&mut d, t, p);
                }
            }
            if n == "E-1" {
                for (t, p) in numbers.iter().take(4) {
                    letter(&mut d, t, [p[0] / 2.0 + 6.0, p[1] / 2.0 + 2.0]);
                }
                // A number E-3 doesn't print, and a sheet number in a bubble's place.
                letter(&mut d, "110", [3.0, 3.0]);
            }
            let mut read = printlinks::read_page(&d, n);
            if n == "E-3" {
                for v in read.views.iter_mut().chain(read.whole.as_mut()) {
                    v.grid_x.clear();
                    v.grid_y.clear();
                }
            }
            if n == "E-1" {
                read.boxes.clear();
                read.dash_boxes.clear();
                for c in &mut read.callouts {
                    c.boundary = None;
                }
            }
            LinkPage { sheet: sheets.iter().find(|s| s.number == n).cloned().unwrap_or_default(), read }
        })
        .collect();
    (sheets, pages)
}

#[test]
fn an_enlarged_plan_with_no_grid_or_box_registers_by_the_tags_it_shares() {
    let (sheets, pages) = tagged_set();
    let e3 = pages.iter().find(|p| p.sheet.number == "E-3").expect("E-3");
    assert!(e3.read.tags.iter().any(|t| t.0 == "101A"), "{:?}", e3.read.tags);
    let l = printlinks::link_set(&sheets, &pages);
    let e = &l.enlarged[0];
    assert!(e.registered, "{e:#?}");
    assert_eq!((e.method.as_str(), e.overall_sheet.as_str(), e.matched_tags), ("tags", "E-1", 4), "{e:#?}");
    // Paper units here: the error left is the lettering's own offset (the same size of letters
    // at two scales), well under a tag's size.
    assert!(e.residual.is_some_and(|r| r < 0.1), "{e:#?}");
    assert_eq!(e.scale, Some(2.0));
    // Its devices count once: the overall plan's 3 copies in its outline are left out.
    assert_eq!((e.devices, e.overall_ignored), (5, 3), "{e:#?}");
    assert_eq!(l.double_counted, 0);
    let at = e.place([6.0, 6.0]).expect("placed");
    assert!((at[0] - 9.0).abs() < 0.1 && (at[1] - 5.0).abs() < 0.1, "{at:?}");
}

#[test]
fn a_turned_enlarged_plan_registers_by_its_tags_with_its_rotation() {
    let (sheets, mut pages) = tagged_set();
    // Turn everything E-3 shows by 90° about the origin and move it (a plan drawn turned on its
    // sheet): (x, y) -> (-y + 40, x + 3).
    let turn = |p: [f64; 2]| [-p[1] + 40.0, p[0] + 3.0];
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-3") {
        for t in &mut p.read.tags {
            t.1 = turn(t.1);
        }
        for dv in &mut p.read.devices {
            dv.at = turn(dv.at);
        }
        for v in p.read.views.iter_mut().chain(p.read.whole.as_mut()) {
            if let Some(r) = v.region {
                let (a, b) = (turn([r[0], r[1]]), turn([r[2], r[3]]));
                v.region = Some([a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]);
            }
        }
    }
    let l = printlinks::link_set(&sheets, &pages);
    let e = &l.enlarged[0];
    assert!(e.registered && e.method == "tags", "{e:#?}");
    assert_eq!(e.rotation_deg, Some(-90.0), "{e:#?}");
    assert!(e.residual.is_some_and(|r| r < 0.1), "{e:#?}");
    let at = e.place(turn([6.0, 6.0])).expect("placed");
    assert!((at[0] - 9.0).abs() < 0.1 && (at[1] - 5.0).abs() < 0.1, "{at:?}");
    assert_eq!(e.devices, 5);
}

/// A wall line added to one of the set's drawings.
fn wall(d: &mut cadaddy_doc::Drawing, a: [f64; 2], b: [f64; 2]) {
    let l = cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(a[0], a[1], 0.0), b: cadaddy_geom::Vec3::new(b[0], b[1], 0.0) };
    let common = cadaddy_doc::Common { layer: "PEN-2".into(), ..cadaddy_doc::Common::default() };
    d.add(&cadaddy_doc::Space::Model, common, cadaddy_doc::EntityKind::Line(l)).expect("line");
}

/// The set with the floor's walls drawn on E-1 and the core's on E-3 (at twice the scale), and
/// nothing else to place E-3 by: no grid lines read on its view, no tags, no boundary box.
fn walled_set() -> (Vec<SetSheet>, Vec<LinkPage>) {
    let sheets = linkset::sheets();
    // E-1's walls (E-1 units): the outline, a corridor, rooms of unequal widths, the core's
    // rooms (x 8..16, y 4..12, split at 12 and 8) with a closet.
    let mut floor: Vec<[[f64; 2]; 2]> = vec![
        [[1.0, 1.0], [18.0, 1.0]],
        [[18.0, 1.0], [18.0, 14.0]],
        [[18.0, 14.0], [1.0, 14.0]],
        [[1.0, 14.0], [1.0, 1.0]],
        [[1.0, 2.5], [7.0, 2.5]],
        [[3.3, 1.0], [3.3, 2.5]],
        [[5.1, 1.0], [5.1, 2.5]],
        [[1.0, 13.0], [8.0, 13.0]],
        [[2.6, 13.0], [2.6, 14.0]],
        [[16.7, 1.0], [16.7, 4.0]],
    ];
    let core = [
        [[8.0, 4.0], [16.0, 4.0]],
        [[16.0, 4.0], [16.0, 12.0]],
        [[16.0, 12.0], [8.0, 12.0]],
        [[8.0, 12.0], [8.0, 4.0]],
        [[12.0, 4.0], [12.0, 12.0]],
        [[8.0, 8.0], [16.0, 8.0]],
        [[13.1, 9.5], [14.6, 9.5]],
        [[14.6, 9.5], [14.6, 12.0]],
        [[9.0, 4.0], [9.0, 5.3]],
    ];
    floor.extend(core);
    // E-1 → E-3: x' = 2(x − 6), y' = 2(y − 2) (the inverse of the shared grid lines' fit).
    let to_e3 = |p: [f64; 2]| [2.0 * (p[0] - 6.0), 2.0 * (p[1] - 2.0)];
    let pages = linkset::drawings()
        .into_iter()
        .map(|(n, mut d)| {
            if n == "E-1" {
                for s in &floor {
                    wall(&mut d, s[0], s[1]);
                }
            }
            if n == "E-3" {
                for s in &core {
                    wall(&mut d, to_e3(s[0]), to_e3(s[1]));
                }
            }
            let mut read = printlinks::read_page(&d, n);
            if n == "E-3" {
                for v in read.views.iter_mut().chain(read.whole.as_mut()) {
                    v.grid_x.clear();
                    v.grid_y.clear();
                }
                read.tags.clear();
            }
            if n == "E-1" {
                read.boxes.clear();
                read.dash_boxes.clear();
                read.tags.clear();
                for c in &mut read.callouts {
                    c.boundary = None;
                }
            }
            LinkPage { sheet: sheets.iter().find(|s| s.number == n).cloned().unwrap_or_default(), read }
        })
        .collect();
    (sheets, pages)
}

#[test]
fn an_enlarged_plan_with_no_grid_tag_or_box_registers_by_the_walls_both_draw() {
    let (sheets, pages) = walled_set();
    let l = printlinks::link_set(&sheets, &pages);
    let e = &l.enlarged[0];
    assert!(e.registered, "{e:#?}");
    assert_eq!((e.method.as_str(), e.overall_sheet.as_str()), ("lines", "E-1"), "{e:#?}");
    assert!(e.line_coverage.is_some_and(|c| c > 0.6), "{e:#?}");
    assert_eq!(e.scale, Some(2.0));
    // Where E-3's points land: the grid lines' fit.
    for p in [[6.0, 6.0], [20.0, 20.0], [4.0, 12.0]] {
        let at = e.place(p).expect("placed");
        assert!((at[0] - (p[0] / 2.0 + 6.0)).abs() < 0.05 && (at[1] - (p[1] / 2.0 + 2.0)).abs() < 0.05, "{p:?} -> {at:?}");
    }
    // Its devices count once: the overall plan's 3 copies in its outline are left out.
    assert_eq!((e.devices, e.overall_ignored), (5, 3), "{e:#?}");
    assert_eq!(l.double_counted, 0);
}

#[test]
fn an_enlarged_plan_whose_walls_lie_nowhere_on_its_plan_stays_unregistered() {
    let (sheets, mut pages) = walled_set();
    // E-1 without the core: its other walls only.
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-1") {
        p.read.lines.retain(|s| {
            let m = s.mid();
            !(m[0] > 7.9 && m[0] < 16.1 && m[1] > 3.9 && m[1] < 12.1)
        });
    }
    let l = printlinks::link_set(&sheets, &pages);
    let e = &l.enlarged[0];
    assert!(!e.registered, "{e:#?}");
    assert_eq!(l.enlarged_registered, 0);
}

/// A core plan whose linework comes apart into two blocks a little apart: both pieces are view
/// 1's, all five devices counted, whether its title is read (over a scale line), read without
/// a scale line, or not printed at all (the sheet's biggest block).
#[test]
fn a_core_plan_in_pieces_counts_every_piece() {
    for title in [Some(("ENLARGED FIRST FLOOR CORE PLAN", true)), Some(("ENLARGED FIRST FLOOR CORE PLAN", false)), None] {
        let (sheets, pages) = with_e3(linkset::enlarged_sheet_in_pieces(0.4, title));
        let l = printlinks::link_set(&sheets, &pages);
        assert_eq!(l.enlarged.len(), 1, "{title:?}: {:#?}", l.enlarged);
        assert_eq!(devices_of(&l, "E-3", "1"), 5, "{title:?}: {:#?}", l.enlarged);
        let core = &l.enlarged[0];
        assert!(core.registered, "{title:?}: {core:#?}");
        assert_eq!(core.overall_ignored, 3, "{title:?}: {core:#?}");
        assert_eq!(l.double_counted, 0);
    }
}

/// Pieces too far apart to read as one plan: once the view registers on the overall plan, the
/// box drawn round the enlarged area there takes in every device of the sheet that lands in it.
#[test]
fn the_overall_plans_box_counts_the_pieces_its_view_missed() {
    let (sheets, pages) = with_e3(linkset::enlarged_sheet_in_pieces(2.5, Some(("ENLARGED FIRST FLOOR CORE PLAN", true))));
    let l = printlinks::link_set(&sheets, &pages);
    let core = l.enlarged.iter().find(|e| e.detail == "1").expect("view 1");
    assert!(core.registered, "{core:#?}");
    assert_eq!((core.devices, core.devices_from.as_str()), (5, "boundary"), "{core:#?}");
    assert_eq!(core.boundary, Some([7.5, 3.5, 16.5, 12.5]), "the box drawn on E-1: {core:#?}");
    assert_eq!(core.overall_ignored, 3);
    assert_eq!(devices_of(&l, "E-3", "1"), 5);
    assert_eq!(l.double_counted, 0);
}

/// An enlarged plan says what it left out: marks that are no device type, symbols with no mark.
#[test]
fn an_enlarged_plan_reports_the_symbols_it_left_out() {
    let mut e3 = linkset::enlarged_sheet(Some(("ENLARGED FIRST FLOOR CORE PLAN", true)));
    let (sheets, _) = the_set();
    // A keynote number beside one symbol (lettering, not a type).
    let mut ins = None;
    for e in e3.model.iter() {
        if let cadaddy_doc::EntityKind::Insert(i) = &e.kind {
            ins = Some(i.clone());
            break;
        }
    }
    let mut ins = ins.expect("a device");
    ins.insert = cadaddy_geom::Vec3::new(8.0, 8.0, 0.0);
    if let Some(a) = ins.attribs.first_mut() {
        a.text.value = "12".into();
    }
    let mut bare = ins.clone();
    bare.insert = cadaddy_geom::Vec3::new(16.0, 16.0, 0.0);
    bare.attribs.clear();
    let common = cadaddy_doc::Common::default();
    e3.add(&cadaddy_doc::Space::Model, common.clone(), cadaddy_doc::EntityKind::Insert(ins)).expect("add");
    e3.add(&cadaddy_doc::Space::Model, common, cadaddy_doc::EntityKind::Insert(bare)).expect("add");
    let (_, pages) = with_e3(e3);
    let l = printlinks::link_set(&sheets, &pages);
    let core = l.enlarged.iter().find(|e| e.detail == "1").expect("view 1");
    assert_eq!(core.devices, 5, "{core:#?}");
    assert_eq!(core.uncounted_tags.get("12"), Some(&1), "{core:#?}");
    assert_eq!(core.untagged_symbols, 1, "{core:#?}");
    let v = serde_json::to_value(core).expect("json");
    assert_eq!(v["untagged_symbols"], 1);
    assert_eq!(v["uncounted_tags"]["12"], 1);
}

/// A symbol the import tagged with its circuit label (the label came first) counts by the type
/// mark printed beside it; a mark another symbol already carries (the import keeps one-letter
/// marks on the page) is not taken twice, and a circuit-tagged symbol with no mark stays out.
#[test]
fn a_symbol_tagged_with_its_circuit_counts_by_the_mark_beside_it() {
    let mut e3 = linkset::enlarged_sheet(Some(("ENLARGED FIRST FLOOR CORE PLAN", true)));
    let (sheets, _) = the_set();
    let mut ins = None;
    for e in e3.model.iter() {
        if let cadaddy_doc::EntityKind::Insert(i) = &e.kind {
            ins = Some(i.clone());
            break;
        }
    }
    let ins = ins.expect("a device");
    let text = |v: &str, x: f64, y: f64| {
        cadaddy_doc::EntityKind::Text(cadaddy_doc::Text {
            insert: cadaddy_geom::Vec3::new(x, y, 0.0),
            align_pt: None,
            height: 0.12,
            value: v.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        })
    };
    let tagged = |tag: &str, x: f64, y: f64| {
        let mut i = ins.clone();
        i.insert = cadaddy_geom::Vec3::new(x, y, 0.0);
        if let Some(a) = i.attribs.first_mut() {
            a.text.value = tag.into();
        }
        cadaddy_doc::EntityKind::Insert(i)
    };
    let common = cadaddy_doc::Common::default();
    let mut add = |k| {
        e3.add(&cadaddy_doc::Space::Model, common.clone(), k).expect("add");
    };
    // Circuit-tagged, its type mark beside it.
    add(tagged("D-12", 8.0, 8.0));
    add(text("A", 8.25, 8.05));
    // A device tagged A, the page still printing its A: not a second device.
    add(tagged("A", 16.0, 16.0));
    add(text("A", 16.25, 16.05));
    // Circuit-tagged with no mark beside it.
    add(tagged("D-14", 16.0, 8.0));
    let (_, pages) = with_e3(e3);
    let e3p = pages.iter().find(|p| p.sheet.number == "E-3").expect("E-3");
    let rescued: Vec<_> = e3p.read.devices.iter().filter(|dv| dv.alt.is_some()).map(|dv| (dv.tag.as_str(), dv.alt.as_deref())).collect();
    assert_eq!(rescued, [("D-12", Some("A"))]);
    let l = printlinks::link_set(&sheets, &pages);
    let core = l.enlarged.iter().find(|e| e.detail == "1").expect("view 1");
    assert_eq!(core.devices, 7, "5 + the circuit-tagged one + the tagged one: {core:#?}");
    assert_eq!(core.devices_by_type.get("A"), Some(&7), "{core:#?}");
    assert_eq!(l.double_counted, 0);
}

/// The core plan printed with each device's circuit label read before its type mark: the
/// import tags every symbol with its circuit, and the marks beside them still count all five.
#[test]
fn a_printed_core_plan_with_circuit_labels_counts_by_the_type_marks() {
    let (sheets, mut pages) = printed_set();
    let pdf = linkset::print(&linkset::enlarged_sheet_circuits_first(Some(("ENLARGED FIRST FLOOR CORE PLAN", true))))
        .unwrap_or_else(|e| panic!("E-3: {e}"));
    let (page, _) = cadaddy_io::pdf_read::read_json(&pdf, &serde_json::json!({"page": 1, "images": false, "fills": "merge"}))
        .unwrap_or_else(|e| panic!("E-3: {e}"));
    let read = printlinks::read_page(&page, "E-3");
    // The import's side of it: the symbols carry the circuits.
    assert!(read.devices.iter().all(|dv| dv.tag.starts_with("D-")), "{:?}", read.devices);
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-3") {
        p.read = read.clone();
    }
    let l = printlinks::link_set(&sheets, &pages);
    assert_eq!(devices_of(&l, "E-3", "1"), 5, "{:#?}", l.enlarged);
    let core = l.enlarged.iter().find(|e| e.detail == "1").expect("view 1");
    assert_eq!(core.devices_by_type.get("A"), Some(&5), "{core:#?}");
    assert!(core.registered, "{core:#?}");
    assert_eq!(l.double_counted, 0);
}

/// The core plan in pieces printed and imported: still one view 1 with all five devices.
#[test]
fn a_printed_core_plan_in_pieces_counts_every_piece() {
    let (sheets, mut pages) = printed_set();
    let pdf = linkset::print(&linkset::enlarged_sheet_in_pieces(0.4, Some(("ENLARGED FIRST FLOOR CORE PLAN", true))))
        .unwrap_or_else(|e| panic!("E-3: {e}"));
    let (page, _) = cadaddy_io::pdf_read::read_json(&pdf, &serde_json::json!({"page": 1, "images": false, "fills": "merge"}))
        .unwrap_or_else(|e| panic!("E-3: {e}"));
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-3") {
        p.read = printlinks::read_page(&page, "E-3");
    }
    let l = printlinks::link_set(&sheets, &pages);
    assert_eq!(devices_of(&l, "E-3", "1"), 5, "{:#?}", l.enlarged);
    let core = l.enlarged.iter().find(|e| e.detail == "1").expect("view 1");
    assert!(core.registered, "{core:#?}");
    assert_eq!(l.double_counted, 0);
}
