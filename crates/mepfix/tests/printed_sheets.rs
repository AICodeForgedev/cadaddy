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
