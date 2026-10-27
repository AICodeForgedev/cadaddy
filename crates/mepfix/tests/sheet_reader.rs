//! Tests for sheet recognition and alignment against the Q254 fixture generator's ground truth.
//!
//! The fixture set generates DXF files with known title blocks, scales, grid positions and
//! coordinate transforms. The sheet reader must recover those from the DXF text and geometry
//! alone, and its alignment must map model coordinates back to building coordinates accurately.

use cadaddy_mepfix::sheet_reader;
use cadaddy_mepfix::{generate, layout};

/// Generate a fixture set, read back the DXFs, and return (truth, sheet map).
fn fixture(seed: u64) -> (cadaddy_mepfix::truth::Truth, sheet_reader::SheetMap) {
    let set = generate(seed).unwrap_or_else(|e| {
        // Don't panic in non-test code; this IS test code.
        panic!("generate({seed}): {e}");
    });
    let dxf_files: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = sheet_reader::recognise_fixture_set(&dxf_files);
    (set.truth, map)
}

#[test]
fn every_sheet_is_recognised() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        // Every truth sheet should appear in the map by number.
        for ts in &truth.sheets {
            assert!(
                map.sheets.iter().any(|s| s.number == ts.number),
                "{ctx}: sheet {} not found in map (found: {:?})",
                ts.number,
                map.sheets.iter().map(|s| &s.number).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn sheet_numbers_are_correct() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        for ts in &truth.sheets {
            let found = map.sheets.iter().find(|s| s.number == ts.number);
            assert!(found.is_some(), "{ctx}: {} not found", ts.number);
        }
    }
}

#[test]
fn disciplines_are_correct() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        for ts in &truth.sheets {
            if let Some(f) = map.sheets.iter().find(|s| s.number == ts.number) {
                assert_eq!(f.discipline, ts.discipline, "{ctx}: {} discipline", ts.number);
            }
        }
    }
}

#[test]
fn levels_are_correct() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        for ts in &truth.sheets {
            if let Some(f) = map.sheets.iter().find(|s| s.number == ts.number) {
                assert_eq!(f.level, ts.level, "{ctx}: {} level", ts.number);
            }
        }
    }
}

#[test]
fn scales_are_detected() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        for ts in &truth.sheets {
            if let (Some(truth_scale), Some(f)) = (&ts.scale_text, map.sheets.iter().find(|s| s.number == ts.number)) {
                assert!(f.scale_text.is_some(), "{ctx}: {} has no detected scale (truth: {truth_scale})", ts.number,);
                assert_eq!(f.scale_text.as_deref(), Some(truth_scale.as_str()), "{ctx}: {} scale text", ts.number,);
                assert!((f.scale - ts.scale).abs() < 0.01, "{ctx}: {} scale value {:.4} vs truth {:.4}", ts.number, f.scale, ts.scale,);
            }
        }
    }
}

#[test]
fn grid_is_detected_on_plan_sheets() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        let n_x = truth.building.grid.x.len();
        let n_y = truth.building.grid.y.len();
        for ts in &truth.sheets {
            if ts.level.is_none() {
                continue; // Covers and schedules have no grid.
            }
            // Enlarged plans clip the drawing, so their grid bubbles may be partially or
            // fully outside the window; skip them.
            if ts.window_ft.is_some() {
                continue;
            }
            if let Some(f) = map.sheets.iter().find(|s| s.number == ts.number) {
                assert!(!f.grid_x.is_empty() || !f.grid_y.is_empty(), "{ctx}: {} has no grid detected (truth has {n_x} x, {n_y} y)", ts.number,);
            }
        }
    }
}

/// Largest alignment error allowed at a grid intersection, feet.
const GRID_TOLERANCE_FT: f64 = 0.1;

/// The fixture sets of seeds 1..=30, generated once for the tests that walk them all.
fn fixtures_1_to_30() -> &'static [(cadaddy_mepfix::truth::Truth, sheet_reader::SheetMap)] {
    static SETS: std::sync::OnceLock<Vec<(cadaddy_mepfix::truth::Truth, sheet_reader::SheetMap)>> = std::sync::OnceLock::new();
    SETS.get_or_init(|| (1..=30).map(fixture).collect())
}

/// A plan sheet the grid must align: one with a level that isn't an enlarged plan (an enlarged
/// plan's window may cut its grid bubbles off).
fn is_aligned_plan(ts: &cadaddy_mepfix::truth::Sheet) -> bool {
    ts.level.is_some() && ts.window_ft.is_none()
}

/// The worst error, feet, over every grid intersection of the truth grid: building point → the
/// sheet's drawing (`to_model`) → back through the read alignment.
fn worst_grid_error(
    truth: &cadaddy_mepfix::truth::Truth,
    alignment: &sheet_reader::Alignment,
    to_model: &dyn Fn([f64; 2]) -> [f64; 2],
) -> (f64, String) {
    let mut worst = (0.0, String::new());
    for gx in &truth.building.grid.x {
        for gy in &truth.building.grid.y {
            let p = [gx.at_ft, gy.at_ft];
            let back = sheet_reader::building_from_model(alignment, to_model(p));
            let err = (back[0] - p[0]).hypot(back[1] - p[1]);
            if err > worst.0 || err.is_nan() {
                worst = (err, format!("{}-{}", gx.label, gy.label));
            }
        }
    }
    worst
}

/// The read sheet `ts`'s alignment, panicking (with context) when it has none.
fn aligned<'a>(map: &'a sheet_reader::SheetMap, ts: &cadaddy_mepfix::truth::Sheet, ctx: &str) -> &'a sheet_reader::Alignment {
    let f = map.sheets.iter().find(|s| s.number == ts.number).unwrap_or_else(|| panic!("{ctx}: {} not read", ts.number));
    f.alignment.as_ref().unwrap_or_else(|| panic!("{ctx}: plan sheet {} not aligned", ts.number))
}

#[test]
fn alignment_recovers_building_coordinates() {
    let mut checked = 0;
    for (i, (truth, map)) in fixtures_1_to_30().iter().enumerate() {
        let ctx = format!("seed {}", i + 1);
        for ts in truth.sheets.iter().filter(|s| is_aligned_plan(s)) {
            let a = aligned(map, ts, &ctx);
            let (err, at) = worst_grid_error(truth, a, &|p| layout::to_model(ts, p));
            assert!(err < GRID_TOLERANCE_FT, "{ctx}: {} alignment error {err:.4} ft at grid {at}", ts.number);
            checked += 1;
        }
    }
    assert!(checked > 0, "no plan sheets checked");
}

#[test]
fn rotated_sheets_are_aligned() {
    let mut checked = 0;
    for (i, (truth, map)) in fixtures_1_to_30().iter().enumerate() {
        let ctx = format!("seed {}", i + 1);
        for ts in truth.sheets.iter().filter(|s| is_aligned_plan(s) && s.model_from_building.rotation_deg.abs() > 1.0) {
            let a = aligned(map, ts, &ctx);
            let (err, at) = worst_grid_error(truth, a, &|p| layout::to_model(ts, p));
            assert!(err < GRID_TOLERANCE_FT, "{ctx}: {} rotated alignment error {err:.4} ft at grid {at}", ts.number);
            checked += 1;
        }
    }
    assert!(checked > 0, "no rotated sheets checked in seeds 1..=30");
}

#[test]
fn offset_sheets_are_aligned() {
    let mut checked = 0;
    for (i, (truth, map)) in fixtures_1_to_30().iter().enumerate() {
        let ctx = format!("seed {}", i + 1);
        for ts in truth.sheets.iter().filter(|s| is_aligned_plan(s) && s.paper_from_model.origin_in != [0.0, 0.0]) {
            let a = aligned(map, ts, &ctx);
            let (err, at) = worst_grid_error(truth, a, &|p| layout::to_model(ts, p));
            assert!(err < GRID_TOLERANCE_FT, "{ctx}: {} offset alignment error {err:.4} ft at grid {at}", ts.number);
            checked += 1;
        }
    }
    assert!(checked > 0, "no offset sheets checked in seeds 1..=30");
}

/// A small deterministic generator for the jitter (xorshift), in [-1, 1].
struct Jitter(u64);

impl Jitter {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

/// Turn a sheet's grid (its bubbles and grid lines) by `deg` about the drawing origin, and move
/// every bubble (circle and label together) by up to `jitter_in` model inches each way, as a
/// print set's bubbles sit a little off. Returns how many bubbles it moved.
fn turn_and_jitter(d: &mut cadaddy_doc::Drawing, deg: f64, jitter_in: f64, rng: &mut Jitter) -> usize {
    use cadaddy_doc::EntityKind;
    let turn = |p: [f64; 2], j: [f64; 2]| {
        let r = layout::rotate(p, deg);
        [r[0] + j[0], r[1] + j[1]]
    };
    let on = |e: &cadaddy_doc::Entity, layer: &str| e.common.layer.eq_ignore_ascii_case(layer);
    // Each bubble's own jitter, by its circle (centre, radius).
    let mut circles: Vec<([f64; 2], f64, [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if let EntityKind::Circle(c) = &e.kind
            && on(e, "GRID-BUBBLE")
        {
            circles.push(([c.center.x, c.center.y], c.radius, [rng.next() * jitter_in, rng.next() * jitter_in]));
        }
    }
    let jitter_at =
        |p: [f64; 2]| circles.iter().filter(|(c, r, _)| (p[0] - c[0]).hypot(p[1] - c[1]) < r * 1.5).map(|(_, _, j)| *j).next().unwrap_or([0.0, 0.0]);
    let handles: Vec<_> = d.model.iter().filter(|e| on(e, "GRID-BUBBLE") || on(e, "GRID")).map(|e| e.handle).collect();
    for h in handles {
        d.model.modify(h, |e| match &mut e.kind {
            EntityKind::Circle(c) => {
                let p = turn([c.center.x, c.center.y], jitter_at([c.center.x, c.center.y]));
                (c.center.x, c.center.y) = (p[0], p[1]);
            }
            EntityKind::Text(t) => {
                let at = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                let j = jitter_at(at);
                let p = turn([t.insert.x, t.insert.y], j);
                (t.insert.x, t.insert.y) = (p[0], p[1]);
                if let Some(a) = &mut t.align_pt {
                    let p = turn([a.x, a.y], j);
                    (a.x, a.y) = (p[0], p[1]);
                }
            }
            EntityKind::Line(l) => {
                let (a, b) = (turn([l.a.x, l.a.y], [0.0, 0.0]), turn([l.b.x, l.b.y], [0.0, 0.0]));
                (l.a.x, l.a.y, l.b.x, l.b.y) = (a[0], a[1], b[0], b[1]);
            }
            _ => {}
        });
    }
    circles.len()
}

/// Every bubble 0.01" (on the paper) off where it belongs, and every third plan sheet of the set
/// plotted turned 0°, 90°, 180°, 270° or 2.5° (on top of the turn the fixture already gives
/// some): every plan sheet still aligns, each grid intersection back within 0.1 ft. Before the
/// axial mean, 0.01" on a square sheet read as 90°, and a sheet turned 180° read as 0°.
#[test]
fn jittered_and_turned_sheets_align_within_a_tenth_of_a_foot() {
    let mut checked = [0usize; 5];
    let mut worst = [0.0f64; 5];
    let mut failures: Vec<String> = Vec::new();
    for seed in 1..=3 {
        let set = cadaddy_mepfix::generate(seed).unwrap_or_else(|e| panic!("generate({seed}): {e}"));
        let truth = &set.truth;
        for (k, deg) in [0.0, 90.0, 180.0, 270.0, 2.5].into_iter().enumerate() {
            let mut rng = Jitter(0x9e37_79b9_7f4a_7c15 ^ (seed * 31 + k as u64));
            let mut turned: Vec<(String, f64)> = Vec::new();
            let mut sheets = Vec::new();
            let mut plan_index = 0;
            for ts in &truth.sheets {
                let Some((_, bytes)) = set.files.iter().find(|(n, _)| *n == ts.dxf) else { continue };
                let mut d = cadaddy_io::read_dxf(bytes).unwrap_or_else(|e| panic!("{}: {e}", ts.dxf));
                let turn = if is_aligned_plan(ts) {
                    plan_index += 1;
                    if plan_index % 3 == 2 { deg } else { 0.0 }
                } else {
                    0.0
                };
                let jitter_in = 0.01 * 12.0 / ts.scale.max(1e-3);
                let moved = turn_and_jitter(&mut d, turn, jitter_in, &mut rng);
                let mut rec = sheet_reader::recognise_dxf(&d);
                if is_aligned_plan(ts) {
                    assert!(moved > 0, "seed {seed}: {} has no bubbles to move", ts.number);
                    assert_eq!(rec.bubbles.len(), moved, "seed {seed} {deg}°: {} bubbles read after the move", ts.number);
                }
                if rec.number.is_empty() {
                    rec.number = ts.number.clone();
                }
                turned.push((ts.number.clone(), turn));
                sheets.push(rec);
            }
            let map = sheet_reader::sheet_map_from(sheets);
            for ts in truth.sheets.iter().filter(|s| is_aligned_plan(s)) {
                let ctx = format!("seed {seed}, every third plan turned {deg}°");
                let turn = turned.iter().find(|(n, _)| *n == ts.number).map_or(0.0, |(_, t)| *t);
                let Some(a) = map.sheets.iter().find(|s| s.number == ts.number).and_then(|s| s.alignment.as_ref()) else {
                    failures.push(format!("{ctx}: {} (turned {turn}°) not aligned", ts.number));
                    continue;
                };
                let (err, at) = worst_grid_error(truth, a, &|p| layout::rotate(layout::to_model(ts, p), turn));
                if err >= GRID_TOLERANCE_FT {
                    failures.push(format!("{ctx}: {} (turned {turn}°, read {:.3}°) error {err:.4} ft at grid {at}", ts.number, a.rotation_deg));
                }
                if turn != 0.0 || deg == 0.0 {
                    checked[k] += 1;
                    worst[k] = worst[k].max(err);
                }
            }
        }
    }
    println!("turned 0/90/180/270/2.5°: {checked:?} sheets checked, worst grid error {worst:.4?} ft");
    assert!(failures.is_empty(), "{} sheets misaligned:\n{}", failures.len(), failures.join("\n"));
    assert!(checked.iter().all(|&n| n > 0), "sheets checked per turn (0, 90, 180, 270, 2.5): {checked:?}");
}

#[test]
fn enlarged_plan_scales_are_detected() {
    // Enlarged plans (P-401, P-402, P-403) have different scales from the rest.
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        for ts in &truth.sheets {
            if ts.window_ft.is_some()
                && let Some(f) = map.sheets.iter().find(|s| s.number == ts.number)
            {
                assert!(f.scale_text.is_some(), "seed {seed}: enlarged plan {} has no scale detected", ts.number,);
                assert!((f.scale - ts.scale).abs() < 0.01, "seed {seed}: enlarged plan {} scale {:.4} vs truth {:.4}", ts.number, f.scale, ts.scale,);
            }
        }
    }
}

#[test]
fn cover_and_schedule_sheets_have_no_level() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        for ts in &truth.sheets {
            if ts.level.is_none()
                && let Some(f) = map.sheets.iter().find(|s| s.number == ts.number)
            {
                assert_eq!(f.level, None, "seed {seed}: non-plan sheet {} should have no level but got {:?}", ts.number, f.level,);
            }
        }
    }
}

#[test]
fn score_sheet_map_reports_correct_fields() {
    let (truth, map) = fixture(3);
    let scores = sheet_reader::score_sheet_map(&map, &truth.sheets, &truth.building.grid.x, &truth.building.grid.y);
    assert_eq!(scores.len(), truth.sheets.len());
    // Every plan sheet scored aligned, its worst grid intersection within tolerance.
    for (sc, ts) in scores.iter().zip(&truth.sheets).filter(|(_, ts)| is_aligned_plan(ts)) {
        let worst = sc.grid_max_error_ft.unwrap_or_else(|| panic!("{} not aligned", ts.number));
        assert!(worst < GRID_TOLERANCE_FT, "{}: worst grid error {worst:.4} ft", ts.number);
        assert!(sc.grid_error_ft.is_some_and(|mean| mean <= worst), "{}: mean {:?} vs worst {worst}", ts.number, sc.grid_error_ft);
    }
}
