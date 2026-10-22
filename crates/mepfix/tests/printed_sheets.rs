//! Sheets read from their prints: a PDF brings a sheet in on one or a few pen layers, so the
//! sheet reader can't lean on `TITLE` and `GRID-BUBBLE` layers there. The fixture set's own PDF
//! prints are recognised like its DXF sheets, and a hand-made sheet on layer 0 with what print
//! sets do (sheet numbers without a hyphen, titles over two lines, scale notes without a
//! `SCALE:` label, grid bubbles drawn as many-sided polylines) reads right.

use cadaddy_doc::{Common, Drawing, EntityKind, HAlign, LwPolyline, Space, Text, VAlign};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use cadaddy_mepfix::generate;
use cadaddy_mepfix::sheet_reader::{self, parse_scale, recognise_dxf};

fn import(pdf: &[u8]) -> Drawing {
    let opts = cadaddy_io::pdf_read::PdfOptions { images: false, markups: false, layout: false, ..Default::default() };
    cadaddy_io::pdf_read::read(pdf, &opts).unwrap_or_else(|e| panic!("pdf import: {e}")).drawing
}

#[test]
fn the_fixture_sets_printed_sheets_are_recognised_like_its_dxf() {
    let set = generate(2).unwrap_or_else(|e| panic!("generate: {e}"));
    let (mut numbers, mut levels, mut level_total, mut scales, mut scale_total, mut grids, mut plans) = (0, 0, 0, 0, 0, 0, 0);
    let mut recognised = Vec::new();
    for sh in &set.truth.sheets {
        let pdf = set.files.iter().find(|(n, _)| *n == sh.pdf).map(|(_, b)| b).unwrap_or_else(|| panic!("{} has no pdf", sh.number));
        let rec = recognise_dxf(&import(pdf));
        numbers += usize::from(rec.number == sh.number);
        assert_eq!(rec.discipline, sh.discipline, "{}: discipline", sh.number);
        if let Some(level) = &sh.level {
            level_total += 1;
            levels += usize::from(rec.level.as_ref() == Some(level));
        }
        if sh.scale_text.is_some() {
            scale_total += 1;
            scales += usize::from((rec.scale - sh.scale).abs() < 1e-9);
        }
        if sh.level.is_some() && sh.title.contains("PLAN") && sh.window_ft.is_none() {
            plans += 1;
            grids += usize::from(!rec.grid_x.is_empty() && !rec.grid_y.is_empty());
        }
        recognised.push(rec);
    }
    let n = set.truth.sheets.len();
    assert_eq!(numbers, n, "sheet numbers read from the prints");
    assert_eq!(levels, level_total, "levels read from the prints");
    assert_eq!(scales, scale_total, "scales read from the prints");
    assert_eq!(grids, plans, "grids found on the printed plans");
    // The grid assembles and the plans align from the prints alone.
    let map = sheet_reader::sheet_map_from(recognised);
    assert_eq!(map.grid_x.len(), set.truth.building.grid.x.len(), "grid lines across");
    assert_eq!(map.grid_y.len(), set.truth.building.grid.y.len(), "grid lines up");
    assert!(map.sheets.iter().filter(|s| s.alignment.is_some()).count() >= plans, "printed plans aligned");
}

// ---- A hand-made sheet on layer 0 ------------------------------------------------------------

fn add(d: &mut Drawing, kind: EntityKind) {
    d.add(&Space::Model, Common { layer: "0".into(), ..Common::default() }, kind).unwrap_or_else(|e| panic!("add: {e}"));
}

fn text(d: &mut Drawing, at: [f64; 2], h: f64, s: &str) {
    let t = Text {
        insert: Vec3::new(at[0], at[1], 0.0),
        align_pt: None,
        height: h,
        value: s.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "STANDARD".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    };
    add(d, EntityKind::Text(t));
}

/// A circle as a print draws one: a closed polyline of short segments.
fn round(d: &mut Drawing, c: [f64; 2], r: f64) {
    let vertices = (0..24)
        .map(|i| {
            let a = f64::from(i) / 24.0 * std::f64::consts::TAU;
            PolyVertex { p: Vec2::new(c[0] + r * a.cos(), c[1] + r * a.sin()), bulge: 0.0, start_width: 0.0, end_width: 0.0 }
        })
        .collect();
    add(d, EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }));
}

/// A label inside a round shape, placed as a print places it: baseline, left end.
fn bubble(d: &mut Drawing, c: [f64; 2], r: f64, label: &str) {
    round(d, c, r);
    let h = r * 0.9;
    text(d, [c[0] - h * 0.35 * label.len() as f64, c[1] - h * 0.5], h, label);
}

/// A 1/8" plan, model in inches: 3 grid lines across (1, 2, 3; 24' apart), 2 up (A, B; 30'
/// apart), bubbles 3' across at both ends, a keynote (smaller, `1` twice in line), and a title
/// block down the right edge.
fn printed_plan(number: &str) -> Drawing {
    let mut d = Drawing::default();
    let r = 18.0;
    for (i, l) in ["1", "2", "3"].iter().enumerate() {
        let x = 120.0 + i as f64 * 288.0;
        bubble(&mut d, [x, -60.0], r, l);
        bubble(&mut d, [x, 480.0], r, l);
    }
    for (i, l) in ["A", "B"].iter().enumerate() {
        let y = 60.0 + i as f64 * 360.0;
        bubble(&mut d, [-60.0, y], r, l);
        bubble(&mut d, [780.0, y], r, l);
    }
    bubble(&mut d, [200.0, 200.0], 6.0, "1");
    bubble(&mut d, [200.0, 260.0], 6.0, "1");
    // The view's title and scale under it; a detail at another scale once.
    text(&mut d, [0.0, -130.0], 14.0, "PARTIAL PLAN - EAST");
    text(&mut d, [0.0, -150.0], 10.0, "1/8\" = 1'-0\"");
    text(&mut d, [400.0, -150.0], 10.0, "1/8\" = 1'-0\"");
    text(&mut d, [600.0, -150.0], 10.0, "1\" = 20'-0\"");
    // The title block: project lines, the title over two lines, the number largest at the foot.
    let tb = 900.0;
    text(&mut d, [tb, 500.0], 8.0, "SAMPLE PROJECT A");
    text(&mut d, [tb, 300.0], 14.0, "SECOND FLOOR");
    text(&mut d, [tb, 280.0], 14.0, "MECHANICAL PLAN");
    text(&mut d, [tb, 120.0], 8.0, "DRAWN BY: XX");
    text(&mut d, [tb, -140.0], 36.0, number);
    d
}

#[test]
fn a_printed_sheet_without_layers_reads_its_title_block_and_grid() {
    let rec = recognise_dxf(&printed_plan("M201"));
    assert_eq!(rec.number, "M201");
    assert_eq!(rec.discipline, "M");
    assert_eq!(rec.title, "SECOND FLOOR MECHANICAL PLAN");
    assert_eq!(rec.level.as_deref(), Some("2"));
    assert_eq!(rec.scale_text.as_deref(), Some("1/8\" = 1'-0\""));
    assert!((rec.scale - 0.125).abs() < 1e-12, "scale {}", rec.scale);
    let labels = |g: &[sheet_reader::DetectedGrid]| g.iter().map(|g| g.label.clone()).collect::<Vec<_>>();
    assert_eq!(labels(&rec.grid_x), ["1", "2", "3"], "the keynote isn't a grid line");
    assert_eq!(labels(&rec.grid_y), ["A", "B"]);
    assert!((rec.grid_x[1].at_in - rec.grid_x[0].at_in - 288.0).abs() < 1e-6);
}

#[test]
fn sheet_numbers_and_scales_as_print_sets_write_them() {
    // Hyphenless, decimal and lettered numbers name their discipline.
    for (number, disc) in [("A101", "A"), ("C2.0", "C"), ("FP-201", "FP"), ("E1.01", "E"), ("S-301A", "S")] {
        let rec = recognise_dxf(&printed_plan(number));
        assert_eq!(rec.number, number);
        assert_eq!(rec.discipline, disc, "{number}");
    }
    // An engineering scale is paper inches per however many feet.
    assert_eq!(parse_scale("1\" = 20'-0\""), Some(0.05));
    assert_eq!(parse_scale("1/8\" = 1'-0\""), Some(0.125));
    assert_eq!(parse_scale("3\" = 1'-0\""), Some(3.0));
}

fn dash(d: &mut Drawing, a: [f64; 2], b: [f64; 2]) {
    add(d, EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }));
}

#[test]
fn grid_lines_with_one_bubble_each_read_from_their_dashes() {
    // A bubble at one end of each line, the line printed as its dashes (a center linetype),
    // as on a plan split over two views: lines 1–3 run down from their bubbles, A–B right.
    let mut d = Drawing::default();
    let r = 18.0;
    for (i, l) in ["1", "2", "3"].iter().enumerate() {
        let x = 120.0 + i as f64 * 288.0;
        bubble(&mut d, [x, 480.0], r, l);
        for k in 0..20 {
            let y = 480.0 - r - f64::from(k) * 24.0;
            dash(&mut d, [x, y], [x, y - 16.0]);
        }
    }
    for (i, l) in ["A", "B"].iter().enumerate() {
        let y = 60.0 + i as f64 * 360.0;
        bubble(&mut d, [-60.0, y], r, l);
        for k in 0..30 {
            let x = -60.0 + r + f64::from(k) * 24.0;
            dash(&mut d, [x, y], [x + 16.0, y]);
        }
    }
    // A keynote bubble with no line and a door tag: no grid lines of their own.
    bubble(&mut d, [300.0, 300.0], r, "4");
    let rec = recognise_dxf(&d);
    let labels = |g: &[sheet_reader::DetectedGrid]| g.iter().map(|g| g.label.clone()).collect::<Vec<_>>();
    assert_eq!(labels(&rec.grid_x), ["1", "2", "3"]);
    assert_eq!(labels(&rec.grid_y), ["A", "B"]);
    assert!((rec.grid_y[1].at_in - rec.grid_y[0].at_in - 360.0).abs() < 1e-6);
}

// ---- Two levels on one sheet (the hotel generator's printed two-view sheet) -------------------

#[test]
fn titles_name_their_levels_lists_and_ranges() {
    use cadaddy_mepfix::sheet_reader::detect_levels;
    let cases: [(&str, &[&str]); 12] = [
        ("SECOND FLOOR PLAN", &["2"]),
        ("1ST FLOOR LIGHTING PLAN", &["1"]),
        ("POWER PLANS- FIRST AND SECOND FLOOR", &["1", "2"]),
        ("THIRD & FOURTH FLOOR DUCT PLAN", &["3", "4"]),
        ("GAS PIPING - 1ST AND 3RD FLOORS", &["1", "3"]),
        ("EGRESS - SECOND-FOURTH FLOOR PLAN", &["2", "3", "4"]),
        ("SIGNAGE PLAN - 2ND THRU 4TH FLOOR", &["2", "3", "4"]),
        ("LEVELS 2-4 POWER PLAN", &["2", "3", "4"]),
        ("LEVEL 02 LIGHTING PLAN", &["2"]),
        ("FOURTH FLOOR AND ROOF PLAN", &["4", "R"]),
        // A note, not a level.
        ("LEVEL MARKS ARE TO FINISHED FLOOR.", &[]),
        ("FLOOR AND ROOF ASSEMBLIES", &[]),
    ];
    for (title, want) in cases {
        assert_eq!(detect_levels(title), want.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{title}");
    }
}

#[test]
fn a_sheet_printing_two_levels_splits_into_views_each_aligned() {
    use cadaddy_mepfix::sheet_reader::{recognise_views, sheet_map_from};
    let (t, st) = cadaddy_mepfix::hotel_planned(7, cadaddy_mepfix::hotel::HotelSize::Small);
    let sheet = cadaddy_mepfix::printed::two_view_sheet(&t, &st, "E-101", "E-102", "E-1A", "LIGHTING").expect("two-view sheet");
    let lines = t.building.grid.x.len() + t.building.grid.y.len();

    // Whole, the sheet reads its own title (not the notes heading the title column) and both levels.
    let whole = recognise_dxf(&sheet.drawing);
    assert_eq!(whole.number, "E-1A");
    assert!(!whole.title.contains("NOTES"), "title {:?}", whole.title);
    assert_eq!(whole.levels, ["1", "2"]);

    // Split, each view is a sheet of its own level with the whole grid read from one bubble per line.
    let views = recognise_views(&sheet.drawing);
    assert_eq!(views.iter().map(|v| v.number.as_str()).collect::<Vec<_>>(), ["E-1A/1", "E-1A/2"]);
    for (v, level) in views.iter().zip(&sheet.levels) {
        assert_eq!(v.level.as_ref(), Some(level));
        assert_eq!(v.printed_number, "E-1A");
        assert_eq!(v.discipline, "E");
        assert_eq!(v.grid_x.len() + v.grid_y.len(), lines, "{}: grid {}x{}", v.number, v.grid_x.len(), v.grid_y.len());
    }

    // Both views align, the upper exactly the shift above the lower.
    let map = sheet_map_from(views);
    let a: Vec<_> = map.sheets.iter().map(|s| s.alignment.clone().unwrap_or_else(|| panic!("{} not aligned", s.number))).collect();
    let (lo, up) = (&a[0], &a[1]);
    assert!(lo.rotation_deg.abs() < 1e-6 && up.rotation_deg.abs() < 1e-6);
    assert!((up.offset_in[0] - lo.offset_in[0]).abs() < 1.0, "x offsets {:?} {:?}", lo.offset_in, up.offset_in);
    assert!((up.offset_in[1] - lo.offset_in[1] - sheet.shift).abs() < 1.0, "y offsets {:?} {:?} shift {}", lo.offset_in, up.offset_in, sheet.shift);
}

// ---- Fix classes 4 and 5b, and the floor whose views read no grid -----------------------------

#[test]
fn stray_keynote_pairs_do_not_beat_a_one_bubble_per_line_grid() {
    use cadaddy_mepfix::printed::{OneViewOptions, one_view_sheet};
    let (t, st) = cadaddy_mepfix::hotel_planned(7, cadaddy_mepfix::hotel::HotelSize::Small);
    let opts = OneViewOptions { keynote_pairs: true, ..OneViewOptions::default() };
    let sheet = one_view_sheet(&t, &st, "E-102", "LIGHTING", &opts).expect("one-view sheet");
    let rec = recognise_dxf(&sheet.drawing);
    // The two keynote pairs are read as bubbles and alone pair up as one line each way (1×1);
    // the bubbles' own lines read the grid.
    assert_eq!(rec.bubbles.len(), sheet.grid_x + sheet.grid_y + 4, "one bubble per line and the four keynote tags");
    assert_eq!(rec.level.as_deref(), Some("2"));
    assert_eq!((rec.grid_x.len(), rec.grid_y.len()), (sheet.grid_x, sheet.grid_y), "grid {}x{}", rec.grid_x.len(), rec.grid_y.len());
    let want_x: Vec<&str> = t.building.grid.x.iter().map(|g| g.label.as_str()).collect();
    let want_y: Vec<&str> = t.building.grid.y.iter().map(|g| g.label.as_str()).collect();
    assert_eq!(rec.grid_x.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), want_x);
    assert_eq!(rec.grid_y.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), want_y);
}

/// A sheet of wall sections, each section showing the grid bubbles `1` and `A` at its corner (as
/// details do): the two `1`s and the two `A`s each pair along the line from one section to the
/// next, so bubble pairs turn the sheet by that line's angle.
fn detail_sheet() -> Drawing {
    let mut d = Drawing::default();
    let r = 18.0;
    for (i, at) in [[0.0, 0.0], [420.0, 300.0]].iter().enumerate() {
        bubble(&mut d, [at[0] + 60.0, at[1] + 240.0], r, "1");
        bubble(&mut d, [at[0], at[1] + 60.0], r, "A");
        text(&mut d, [at[0], at[1] - 40.0], 14.0, &format!("WALL SECTION {}", i + 1));
    }
    text(&mut d, [900.0, 300.0], 14.0, "WALL SECTIONS");
    text(&mut d, [900.0, -140.0], 36.0, "A501");
    d
}

#[test]
fn a_detail_whose_bubble_pairs_turn_it_gets_no_alignment() {
    let plan = recognise_dxf(&printed_plan("M201"));
    let detail = recognise_dxf(&detail_sheet());
    assert_eq!(detail.bubbles.len(), 4, "the detail's bubbles are read");
    assert!(detail.grid_x.is_empty() && detail.grid_y.is_empty(), "no square lines on the detail");
    let map = sheet_reader::sheet_map_from(vec![plan, detail]);
    let get = |n: &str| map.sheets.iter().find(|s| s.number == n).unwrap_or_else(|| panic!("{n}"));
    assert!(get("M201").alignment.is_some(), "the plan aligns");
    assert_eq!(get("A501").alignment, None, "the detail's pair rotation is rejected");
}

#[test]
fn a_floor_printed_without_its_grid_lines_reads_and_aligns_its_grid() {
    use cadaddy_mepfix::printed::{OneViewOptions, one_view_sheet};
    let (t, st) = cadaddy_mepfix::hotel_planned(7, cadaddy_mepfix::hotel::HotelSize::Small);
    let mut views = Vec::new();
    // The repro: the fourth floor printed with one stray grid line and its bubbles alone (before
    // the bubble-row reading it read a 0×0 grid from its 12 bubbles; the real set's read 1×0 from 33).
    for (number, lines) in [("E-101", usize::MAX), ("E-102", usize::MAX), ("E-103", usize::MAX), ("E-104", 1)] {
        let opts = OneViewOptions { grid_lines: lines, ..OneViewOptions::default() };
        let sheet = one_view_sheet(&t, &st, number, "LIGHTING", &opts).expect("one-view sheet");
        let rec = recognise_dxf(&sheet.drawing);
        assert_eq!((rec.grid_x.len(), rec.grid_y.len()), (sheet.grid_x, sheet.grid_y), "{number}: grid {}x{}", rec.grid_x.len(), rec.grid_y.len());
        views.push(rec);
    }
    let map = sheet_reader::sheet_map_from(views);
    let a: Vec<_> = map.sheets.iter().map(|s| s.alignment.clone().unwrap_or_else(|| panic!("{} not aligned", s.number))).collect();
    // Every floor prints its plan in the same place: the fourth aligns where the first does.
    for (s, al) in map.sheets.iter().zip(&a) {
        assert!(al.rotation_deg.abs() < 1e-6, "{} rotation", s.number);
        assert!(
            (al.offset_in[0] - a[0].offset_in[0]).abs() < 1.0 && (al.offset_in[1] - a[0].offset_in[1]).abs() < 1.0,
            "{} offset {:?} vs {:?}",
            s.number,
            al.offset_in,
            a[0].offset_in
        );
    }
}
