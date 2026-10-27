//! Sheets read from their prints: a PDF brings a sheet in on one or a few pen layers, so the
//! sheet reader can't lean on `TITLE` and `GRID-BUBBLE` layers there. The fixture set's own PDF
//! prints are recognised like its DXF sheets, and a hand-made sheet on layer 0 with what print
//! sets do (sheet numbers without a hyphen, titles over two lines, scale notes without a
//! `SCALE:` label, grid bubbles drawn as many-sided polylines) reads right. A merged set PDF (a
//! real set's shape: pen layers only) is read page by page, and past the sheet stage: its
//! conduit runs with their height notes, and its walls.

use cadaddy_doc::{Common, Drawing, EntityKind, HAlign, LwPolyline, Space, Text, VAlign};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use cadaddy_mepfix::generate;
use cadaddy_mepfix::sheet_reader::{self, parse_scale, recognise_dxf};
use cadaddy_mepfix::{conduit_reader, printed};

fn import(pdf: &[u8]) -> Drawing {
    // recover_text off: SHX recovery and symbol detection consume grid-bubble strokes and
    // labels that the sheet recogniser needs for grid alignment.
    let opts = cadaddy_io::pdf_read::PdfOptions { images: false, markups: false, layout: false, recover_text: false, ..Default::default() };
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
    // ... and align right: every printed plan aligned, measured against the truth.
    let aligned = assert_printed_alignment(&map, &set.truth, &|s| s.title.contains("PLAN") && s.window_ft.is_none());
    assert_eq!(aligned, plans, "printed plans aligned within {PRINTED_GRID_TOLERANCE_FT} ft");
}

/// Largest alignment error allowed at a grid intersection of a printed page, feet.
const PRINTED_GRID_TOLERANCE_FT: f64 = 0.1;

/// The truth sheet as its print imports: a page comes in drawn full size at the sheet's scale,
/// its lower-left corner at the origin, so the drawing is the sheet's shifted by the paper origin.
fn as_printed(ts: &cadaddy_mepfix::truth::Sheet) -> cadaddy_mepfix::truth::Sheet {
    let mut p = ts.clone();
    for (o, origin) in p.model_from_building.offset_in.iter_mut().zip(ts.paper_from_model.origin_in) {
        *o -= origin;
    }
    p
}

/// Score `map` against the truth ([`sheet_reader::score_sheet_map`], the truth sheets as printed)
/// and assert every aligned page among the plan pages (`plan`, of the truth sheets with a level)
/// puts every grid intersection within [`PRINTED_GRID_TOLERANCE_FT`]; returns how many aligned.
fn assert_printed_alignment(
    map: &sheet_reader::SheetMap,
    t: &cadaddy_mepfix::truth::Truth,
    plan: &dyn Fn(&cadaddy_mepfix::truth::Sheet) -> bool,
) -> usize {
    let printed: Vec<_> = t.sheets.iter().map(as_printed).collect();
    let scores = sheet_reader::score_sheet_map(map, &printed, &t.building.grid.x, &t.building.grid.y);
    let mut aligned = 0;
    let mut worst = 0.0f64;
    for (ts, sc) in t.sheets.iter().zip(&scores) {
        if ts.level.is_none() || !plan(ts) {
            continue;
        }
        let Some(err) = sc.grid_max_error_ft else { continue };
        assert!(err < PRINTED_GRID_TOLERANCE_FT, "{}: printed alignment worst grid error {err:.4} ft", ts.number);
        aligned += 1;
        worst = worst.max(err);
    }
    println!("{aligned} printed plan pages aligned, worst grid error {worst:.4} ft");
    aligned
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

// ---- Merged print sets, read past the sheet stage --------------------------------------------

/// A generated set's merged `set.pdf`, read page by page.
fn merged(set: &cadaddy_mepfix::FixtureSet) -> printed::PrintedSet {
    let pdf = set.files.iter().find(|(n, _)| n == "set.pdf").map(|(_, b)| b).unwrap_or_else(|| panic!("no set.pdf"));
    let p = printed::read_merged(pdf).unwrap_or_else(|e| panic!("read set.pdf: {e}"));
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    // Nothing on the fixture's own synthetic layers: content-class layers (TEXT, A-WALL) are
    // expected since the default import now uses content classification for non-OCG PDFs.
    for (n, b) in &p.dxf {
        let d = cadaddy_io::read_dxf(b).unwrap_or_else(|e| panic!("{n}: {e}"));
        for l in ["DEVICE", "TEXT", "CALLOUT", "ARCH", "A-WALL", "GRID-BUBBLE", "TITLE"] {
            assert!(!d.layers.iter().any(|x| x.name.eq_ignore_ascii_case(l)), "{n} has layer {l}");
        }
    }
    p
}

/// The hotel's merged print set (50 sheets in one PDF, as a real set comes) recognised page by
/// page and assembled: the numbers Q274 reported for it, held.
#[test]
fn the_hotels_merged_set_pdf_is_recognised_page_by_page() {
    let set = cadaddy_mepfix::generate_hotel(1, cadaddy_mepfix::hotel::HotelSize::Small).unwrap_or_else(|e| panic!("hotel: {e}"));
    let p = merged(&set);
    let t = &set.truth;
    // 54: 50 base + 4 SL-10x sleeve layout sheets (lowvolt-rw1)
    assert_eq!((p.pages.len(), t.sheets.len()), (54, 54), "a page per sheet");
    let truth = |n: &str| t.sheets.iter().find(|s| s.number == n);
    let numbers = p.pages.iter().filter(|r| truth(&r.number).is_some()).count();
    let distinct: std::collections::BTreeSet<&str> = p.pages.iter().map(|r| r.number.as_str()).collect();
    assert_eq!((numbers, distinct.len()), (54, 54), "sheet numbers read");
    // A level on every plan sheet (every sheet the truth gives a level), and the right one.
    for s in t.sheets.iter().filter(|s| s.level.is_some()) {
        let r = p.pages.iter().find(|r| r.number == s.number).unwrap_or_else(|| panic!("{} not read", s.number));
        assert_eq!(r.level, s.level, "{}: level", s.number);
    }
    let scales = p.pages.iter().filter(|r| truth(&r.number).is_some_and(|s| s.scale_text.is_some() && (s.scale - r.scale).abs() < 1e-9)).count();
    assert!(scales >= 43, "scales read right: {scales}");
    let grids = p.pages.iter().filter(|r| !r.grid_x.is_empty() && !r.grid_y.is_empty()).count();
    assert!(grids >= 40, "grids found: {grids}");
    // Aligned to the common grid, and measured against the truth: every aligned plan page within
    // a tenth of a foot at every grid intersection.
    let aligned = assert_printed_alignment(&p.map, t, &|s| s.window_ft.is_none());
    assert!(aligned >= 40, "plan pages aligned to the common grid within {PRINTED_GRID_TOLERANCE_FT} ft: {aligned}");
    assert_eq!(p.map.grid_x.len(), t.building.grid.x.len(), "grid lines across");
    assert_eq!(p.map.grid_y.len(), t.building.grid.y.len(), "grid lines up");
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The conduit runs the electrical plans draw (truth): every branch run, and every feeder
/// bank's conduits, with whether a printed note gives its height (a bank's tag, `BOC @ …`, is
/// the height of each conduit in the bank; branch runs carry none).
fn truth_runs(t: &cadaddy_mepfix::truth::Truth) -> Vec<(String, [f64; 2], [f64; 2], bool)> {
    let mut out: Vec<(String, [f64; 2], [f64; 2], bool)> =
        t.elec_conduits.iter().filter(|c| !c.sheet.is_empty()).map(|c| (c.level.clone(), c.from_ft, c.to_ft, false)).collect();
    let feeders: Vec<&cadaddy_mepfix::truth::Segment> =
        t.segments.iter().filter(|s| s.kind == "conduit" && !s.vertical && !s.sheet.is_empty()).collect();
    for s in &feeders {
        let [a, b] = s.path_ft;
        // Its bank: the conduits of its level over the same span.
        let bank_noted = feeders.iter().any(|o| {
            o.level == s.level
                && (o.path_ft[0][0] - a[0]).abs() < 1e-6
                && (o.path_ft[1][0] - b[0]).abs() < 1e-6
                && o.height.as_ref().is_some_and(|h| h.note.is_some())
        });
        out.push((s.level.clone(), [a[0], a[1]], [b[0], b[1]], bank_noted));
    }
    out
}

/// Conduit runs and their height notes read from the office sets' merged prints, with no layer
/// names: at least 80% of the drawn runs found (both ends within 1', same level), and the share
/// of runs whose height comes from a printed note within 10 points of the truth's share.
#[test]
fn conduit_runs_and_height_notes_read_from_merged_prints() {
    let (mut truth_total, mut truth_found, mut truth_noted, mut read, mut read_noted) = (0, 0, 0, 0, 0);
    for seed in 1..=3 {
        let set = cadaddy_mepfix::generate(seed).unwrap_or_else(|e| panic!("generate: {e}"));
        let p = merged(&set);
        let mut branch = conduit_reader::read_conduits(&p.dxf, &p.map);
        let ceilings = cadaddy_mepfix::ceiling_reader::read_ceilings(&p.dxf, &p.map, None);
        conduit_reader::assign_heights(&mut branch, &|level| Some(conduit_reader::context_from_ceilings(&ceilings, level, 0.0, 12.0)));
        let trays = cadaddy_mepfix::tray_reader::read_trays(&p.dxf, &p.map);
        let runs = printed::conduit_runs(&branch, &trays);
        let truth = truth_runs(&set.truth);
        let same = |r: &printed::PrintRun, t: &(String, [f64; 2], [f64; 2], bool)| {
            r.level == t.0
                && ((dist(r.from_ft, t.1) <= 1.0 && dist(r.to_ft, t.2) <= 1.0) || (dist(r.from_ft, t.2) <= 1.0 && dist(r.to_ft, t.1) <= 1.0))
        };
        let found = truth.iter().filter(|t| runs.iter().any(|r| same(r, t))).count();
        println!(
            "seed {seed}: {} truth runs ({} noted), {found} found; {} read ({} from a note)",
            truth.len(),
            truth.iter().filter(|t| t.3).count(),
            runs.len(),
            runs.iter().filter(|r| r.from_note).count()
        );
        assert!(runs.iter().all(|r| r.bottom_ft.is_some()), "every run has a height");
        truth_total += truth.len();
        truth_found += found;
        truth_noted += truth.iter().filter(|t| t.3).count();
        read += runs.len();
        read_noted += runs.iter().filter(|r| r.from_note).count();
    }
    assert!(truth_total >= 20, "{truth_total} truth runs");
    let recall = truth_found as f64 / truth_total as f64;
    assert!(recall >= 0.8, "runs found: {truth_found}/{truth_total}");
    let (want, got) = (truth_noted as f64 / truth_total as f64, read_noted as f64 / read.max(1) as f64);
    assert!(want > 0.0, "the truth has noted runs");
    assert!((got - want).abs() <= 0.10, "heights from notes: read {got:.2}, truth {want:.2}");
}

/// Walls read from the office sets' merged prints, with no layer names: at least 80% of the
/// truth's plan walls found. A truth wall is found when the walls read on its centreline cover at
/// least half its length; "on its centreline": both ends of a wall read within 3" (0.25') of the
/// truth wall's centreline, on the same level.
#[test]
fn walls_read_from_merged_prints() {
    const TOL_FT: f64 = 0.25;
    let (mut total, mut found, mut read, mut read_on) = (0, 0, 0, 0);
    for seed in 1..=3 {
        let set = cadaddy_mepfix::generate(seed).unwrap_or_else(|e| panic!("generate: {e}"));
        let p = merged(&set);
        let ceilings = cadaddy_mepfix::ceiling_reader::read_ceilings(&p.dxf, &p.map, None);
        let walls = cadaddy_mepfix::wall_reader::read_walls(&p.dxf, &p.map, &ceilings, &Default::default());
        // Where along the truth wall (feet from a) a wall read on its centreline runs, if it does.
        let on = |w: &cadaddy_mepfix::wall_reader::FoundWall, t: &cadaddy_mepfix::truth::PlanWall| -> Option<(f64, f64)> {
            let len = dist(t.a_ft, t.b_ft);
            if w.level != t.level || len < 1e-9 {
                return None;
            }
            let u = [(t.b_ft[0] - t.a_ft[0]) / len, (t.b_ft[1] - t.a_ft[1]) / len];
            let off = |q: [f64; 2]| ((q[0] - t.a_ft[0]) * -u[1] + (q[1] - t.a_ft[1]) * u[0]).abs();
            let along = |q: [f64; 2]| (q[0] - t.a_ft[0]) * u[0] + (q[1] - t.a_ft[1]) * u[1];
            let (s0, s1) = (along(w.a_ft).min(along(w.b_ft)).max(0.0), along(w.a_ft).max(along(w.b_ft)).min(len));
            (off(w.a_ft) <= TOL_FT && off(w.b_ft) <= TOL_FT && s1 > s0).then_some((s0, s1))
        };
        let mut seed_found = 0;
        for t in &set.truth.plan_walls {
            let mut spans: Vec<(f64, f64)> = walls.walls.iter().filter_map(|w| on(w, t)).collect();
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (mut covered, mut reach) = (0.0, 0.0_f64);
            for (s0, s1) in spans {
                covered += (s1 - s0.max(reach)).max(0.0);
                reach = reach.max(s1);
            }
            seed_found += usize::from(covered >= 0.5 * dist(t.a_ft, t.b_ft));
        }
        let on_truth = walls.walls.iter().filter(|w| set.truth.plan_walls.iter().any(|t| on(w, t).is_some())).count();
        println!(
            "seed {seed}: {seed_found}/{} truth walls found; {} walls read, {on_truth} on a truth wall",
            set.truth.plan_walls.len(),
            walls.walls.len()
        );
        total += set.truth.plan_walls.len();
        found += seed_found;
        read += walls.walls.len();
        read_on += on_truth;
    }
    assert!(total >= 30, "{total} truth walls");
    assert!(found as f64 >= 0.8 * total as f64, "walls found: {found}/{total}");
    // Stairs, the elevator and casework outlines read as walls too; most of what is read is wall.
    assert!(read_on as f64 >= 0.45 * read as f64, "walls read on a truth wall: {read_on}/{read}");
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

// ---- Q6030 planregions: each level its own view ------------------------------------------------

#[test]
fn a_two_level_sheet_has_a_view_per_level_and_a_one_view_sheet_none() {
    let (t, st) = cadaddy_mepfix::hotel_planned(7, cadaddy_mepfix::hotel::HotelSize::Small);
    let two = printed::two_view_sheet(&t, &st, "E-101", "E-102", "E-1A", "LIGHTING").expect("two-view sheet");
    let views = sheet_reader::level_views(&two.drawing);
    assert_eq!(views.iter().map(|(v, _)| v.levels.clone()).collect::<Vec<_>>(), [vec!["1".to_string()], vec!["2".to_string()]]);
    let (lower, upper) = (views[0].1, views[1].1);
    assert!(lower[3] <= upper[1], "the lower view ends where the upper begins: {lower:?} {upper:?}");
    let one = printed::one_view_sheet(&t, &st, "E-102", "LIGHTING", &printed::OneViewOptions::default()).expect("one-view sheet");
    assert!(sheet_reader::level_views(&one.drawing).is_empty(), "one plan: no views to split");
}

#[test]
fn views_titled_over_a_scale_line_split_a_sheet_when_their_lettering_is_the_sheets_size() {
    // Two plans side by side, each a block of short lines, titled over a SCALE line in the
    // sheet's usual lettering (too small for the large-title reading).
    let mut d = Drawing::new_imperial();
    let seg = |d: &mut Drawing, a: [f64; 2], b: [f64; 2]| {
        let vertices = vec![PolyVertex::new(Vec2::new(a[0], a[1])), PolyVertex::new(Vec2::new(b[0], b[1]))];
        add(d, EntityKind::LwPolyline(LwPolyline { vertices, closed: false, const_width: 0.0, elevation: 0.0, plinegen: false }));
    };
    for (k, title) in ["FIRST FLOOR PLAN", "SECOND FLOOR PLAN"].iter().enumerate() {
        let x0 = k as f64 * 140.0;
        for i in 0..10 {
            for j in 0..9 {
                let (x, y) = (x0 + i as f64 * 10.0, 20.0 + j as f64 * 10.0);
                seg(&mut d, [x, y], [x + 10.0, y]);
                seg(&mut d, [x, y], [x, y + 10.0]);
            }
        }
        for i in 0..8 {
            text(&mut d, [x0 + 5.0 + i as f64 * 11.0, 60.0], 2.0, "OFFICE");
        }
        text(&mut d, [x0 + 10.0, 12.0], 2.0, title);
        text(&mut d, [x0 + 10.0, 9.0], 1.5, "SCALE: 1/8\" = 1'-0\"");
    }
    let views = sheet_reader::level_views(&d);
    assert_eq!(views.iter().map(|(v, _)| v.levels.clone()).collect::<Vec<_>>(), [vec!["1".to_string()], vec!["2".to_string()]], "{views:?}");
    assert!(views[0].1[2] < views[1].1[0], "the views stand apart: {views:?}");
}

/// A block of short lines (a plan's linework) `w` × `h` squares of 10 from `at`.
fn plan_block(d: &mut Drawing, at: [f64; 2], w: usize, h: usize) {
    for i in 0..w {
        for j in 0..h {
            let (x, y) = (at[0] + i as f64 * 10.0, at[1] + j as f64 * 10.0);
            for b in [[x + 10.0, y], [x, y + 10.0]] {
                let vertices = vec![PolyVertex::new(Vec2::new(x, y)), PolyVertex::new(Vec2::new(b[0], b[1]))];
                add(d, EntityKind::LwPolyline(LwPolyline { vertices, closed: false, const_width: 0.0, elevation: 0.0, plinegen: false }));
            }
        }
    }
}

#[test]
fn a_two_level_sheet_with_untitled_views_splits_by_its_blocks_of_linework() {
    let one = |v: &[(sheet_reader::PlanView, [f64; 4])]| v.iter().map(|(v, _)| v.levels.clone()).collect::<Vec<_>>();
    let levels = ["1".to_string(), "2".to_string()];
    // Two plans side by side, no view titles: the titled readings find nothing.
    let mut d = Drawing::new_imperial();
    plan_block(&mut d, [0.0, 20.0], 10, 9);
    plan_block(&mut d, [140.0, 20.0], 8, 9);
    text(&mut d, [5.0, 60.0], 2.0, "OFFICE");
    assert!(sheet_reader::level_views(&d).is_empty(), "no titles to read");
    let views = sheet_reader::level_blocks(&d, &levels);
    assert_eq!(one(&views), [vec!["1".to_string()], vec!["2".to_string()]], "left to right, lowest first: {views:?}");
    let (left, right) = (views[0].1, views[1].1);
    assert!((left[2] - 120.0).abs() < 10.0 && left[2] <= right[0], "they meet halfway between the plans: {views:?}");
    assert!(left[0].is_infinite() && right[2].is_infinite(), "open at the sheet's edges: {views:?}");
    // A typical plan (the set's "T"), a level more than the blocks, or one level: no split.
    assert!(sheet_reader::level_blocks(&d, &["T".to_string()]).is_empty());
    assert!(sheet_reader::level_blocks(&d, &["1".to_string(), "2".to_string(), "3".to_string()]).is_empty());
    assert!(sheet_reader::level_blocks(&d, &levels[..1]).is_empty());
    // One plan and a small legend block: one big block, so it is one view.
    let mut single = Drawing::new_imperial();
    plan_block(&mut single, [0.0, 20.0], 10, 9);
    plan_block(&mut single, [140.0, 20.0], 3, 2);
    assert!(sheet_reader::level_blocks(&single, &levels).is_empty(), "one plan is never split");

    // Stacked plans whose small titles name their levels: the titles decide, not the order.
    let mut stacked = Drawing::new_imperial();
    plan_block(&mut stacked, [0.0, 0.0], 10, 6);
    plan_block(&mut stacked, [0.0, 100.0], 10, 6);
    text(&mut stacked, [10.0, 92.0], 1.0, "LEVEL 2 MECHANICAL PLAN");
    text(&mut stacked, [10.0, -8.0], 1.0, "LEVEL 1 MECHANICAL PLAN");
    text(&mut stacked, [10.0, 162.0], 1.0, "MECHANICAL PLANS - LEVELS 1 AND 2");
    let views = sheet_reader::level_blocks(&stacked, &levels);
    assert_eq!(one(&views), [vec!["1".to_string()], vec!["2".to_string()]], "{views:?}");
    let (lower, upper) = (views[0].1, views[1].1);
    assert!(lower[3] > 60.0 && lower[3] < 100.0 && lower[3] == upper[1], "they meet between the plans: {views:?}");
    assert!(lower[1] < 0.0 && upper[1] < 92.0 && upper[1] > 60.0, "the bottom plan's part holds the title under the top plan: {views:?}");
    assert_eq!(views[0].0.title, "LEVEL 1 MECHANICAL PLAN", "the title naming one level, not the sheet's: {views:?}");
    // Here the title under the top plan says it is level 1's.
    let mut swapped = Drawing::new_imperial();
    plan_block(&mut swapped, [0.0, 0.0], 10, 6);
    plan_block(&mut swapped, [0.0, 100.0], 10, 6);
    text(&mut swapped, [10.0, 92.0], 1.0, "LEVEL 1 MECHANICAL PLAN");
    text(&mut swapped, [10.0, -8.0], 1.0, "LEVEL 2 MECHANICAL PLAN");
    let views = sheet_reader::level_blocks(&swapped, &levels);
    assert_eq!(one(&views), [vec!["2".to_string()], vec!["1".to_string()]], "the titles decide, not the order: {views:?}");
    assert!(views[1].1[1] > views[0].1[1], "level 1 is the top plan: {views:?}");
    // Untitled, they go bottom to top.
    let mut bare = Drawing::new_imperial();
    plan_block(&mut bare, [0.0, 0.0], 10, 6);
    plan_block(&mut bare, [0.0, 100.0], 10, 6);
    let views = sheet_reader::level_blocks(&bare, &levels);
    assert_eq!(one(&views), [vec!["1".to_string()], vec!["2".to_string()]]);
    assert!(views[0].1[3] < views[1].1[3], "the bottom plan first: {views:?}");
}

#[test]
fn a_typical_title_says_so() {
    for t in ["TYPICAL 2ND-4TH FLOOR PLAN", "LEVELS 3-4 MECHANICAL PLAN (TYP.)", "Typical floor plan"] {
        assert!(sheet_reader::says_typical(t), "{t}");
    }
    for t in ["FIRST AND SECOND FLOOR PLANS", "LEVEL 3 & 4 POWER PLANS", "TYPEWRITER ROOM"] {
        assert!(!sheet_reader::says_typical(t), "{t}");
    }
}
