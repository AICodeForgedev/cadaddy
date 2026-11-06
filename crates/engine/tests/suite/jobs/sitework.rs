//! Tests for Q339 sitework: grading, paving, curbs, sidewalks, striping, utility structures,
//! retaining walls, landscaping, fencing, site furnishings, and quantity takeoff.
//!
//! The takeoff is scored against the generator's truth: earthwork against planes with a known
//! cut and fill, the civil reader (`site.takeoff`'s) against the drawn C-101/C-102/C-103 over
//! three seeds, pipe footage by system, size and material, and every quantity's elements adding
//! back up to it.

use cadaddy_engine::cmd::sitetakeoff::{self as st, Contour, Surface};
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::site_reader;
use cadaddy_mepfix::truth::{SiteQuantity, Truth};

fn hotel_truth(seed: u64) -> cadaddy_mepfix::truth::Truth {
    let set = generate_hotel(seed, HotelSize::Small).unwrap_or_else(|e| panic!("generate_hotel({seed}): {e}"));
    set.truth
}

fn hotel_set(seed: u64) -> cadaddy_mepfix::FixtureSet {
    generate_hotel(seed, HotelSize::Small).unwrap_or_else(|e| panic!("generate_hotel({seed}): {e}"))
}

// --- Grading contours ---

#[test]
fn grading_contours_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        assert!(!t.grading_contours.is_empty(), "seed {seed}: no grading contours generated");
        let existing = t.grading_contours.iter().filter(|c| c.surface == "existing").count();
        let proposed = t.grading_contours.iter().filter(|c| c.surface == "proposed").count();
        assert!(existing > 0, "seed {seed}: no existing contours");
        assert!(proposed > 0, "seed {seed}: no proposed contours");
    }
}

#[test]
fn grading_contours_have_points() {
    let t = hotel_truth(1);
    for gc in &t.grading_contours {
        assert!(gc.points.len() >= 2, "contour {} has fewer than 2 points", gc.id);
        assert!(gc.elevation_ft > 0.0, "contour {} has non-positive elevation", gc.id);
    }
}

// --- Building pad ---

#[test]
fn building_pad_is_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let pads: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "building_pad").collect();
        assert_eq!(pads.len(), 1, "seed {seed}: expected exactly 1 building pad, found {}", pads.len());
        let pad = &pads[0];
        assert!(pad.area_sf.unwrap_or(0.0) > 0.0, "seed {seed}: building pad has no area");
        assert!(pad.min_ft.is_some() && pad.max_ft.is_some(), "seed {seed}: building pad has no bounds");
    }
}

// --- Paving ---

#[test]
fn paving_areas_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let asphalt: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "paving_asphalt").collect();
        let concrete: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "paving_concrete").collect();
        assert!(!asphalt.is_empty(), "seed {seed}: no asphalt paving");
        assert!(!concrete.is_empty(), "seed {seed}: no concrete paving");
        for p in &asphalt {
            assert!(p.area_sf.unwrap_or(0.0) > 0.0, "seed {seed}: asphalt paving has no area");
        }
    }
}

// --- Curbs and sidewalks ---

#[test]
fn curbs_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let curbs: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "curb").collect();
        assert!(curbs.len() >= 3, "seed {seed}: expected at least 3 curb segments, found {}", curbs.len());
        for c in &curbs {
            assert!(c.length_ft.unwrap_or(0.0) > 0.0, "seed {seed}: curb {} has no length", c.tag);
            assert!(c.material == "concrete", "seed {seed}: curb {} material should be concrete", c.tag);
        }
    }
}

#[test]
fn sidewalks_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let sws: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "sidewalk").collect();
        assert!(!sws.is_empty(), "seed {seed}: no sidewalks");
        for sw in &sws {
            assert!(sw.area_sf.unwrap_or(0.0) > 0.0, "seed {seed}: sidewalk has no area");
        }
    }
}

// --- Striping ---

#[test]
fn striping_is_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let stripes: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "striping").collect();
        assert!(!stripes.is_empty(), "seed {seed}: no striping");
    }
}

// --- Utility structures ---

#[test]
fn utility_structures_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        assert!(!t.site_structures.is_empty(), "seed {seed}: no site structures");
        let manholes = t.site_structures.iter().filter(|s| s.kind == "manhole").count();
        let catch_basins = t.site_structures.iter().filter(|s| s.kind == "catch_basin").count();
        let cleanouts = t.site_structures.iter().filter(|s| s.kind == "cleanout").count();
        assert!(manholes > 0, "seed {seed}: no manholes");
        assert!(catch_basins > 0, "seed {seed}: no catch basins");
        assert!(cleanouts > 0, "seed {seed}: no cleanouts");
    }
}

#[test]
fn utility_structures_have_elevations() {
    let t = hotel_truth(1);
    let with_rim = t.site_structures.iter().filter(|s| s.rim_ft.is_some()).count();
    let with_inv = t.site_structures.iter().filter(|s| s.invert_ft.is_some()).count();
    assert!(with_rim > 0, "no structures with rim elevation");
    assert!(with_inv > 0, "no structures with invert elevation");
}

#[test]
fn utility_structures_have_systems() {
    let t = hotel_truth(1);
    let systems: Vec<&str> = t.site_structures.iter().map(|s| s.system.as_str()).collect();
    assert!(systems.contains(&"storm"), "no storm system structures");
    assert!(systems.contains(&"sanitary"), "no sanitary system structures");
    assert!(systems.contains(&"water"), "no water system structures");
}

// --- Retaining walls ---

#[test]
fn retaining_walls_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let rws: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "retaining_wall").collect();
        assert!(!rws.is_empty(), "seed {seed}: no retaining walls");
        for rw in &rws {
            assert!(rw.height_ft.unwrap_or(0.0) > 0.0, "seed {seed}: retaining wall has no height");
            assert!(rw.length_ft.unwrap_or(0.0) > 0.0, "seed {seed}: retaining wall has no length");
        }
    }
}

// --- Landscaping ---

#[test]
fn trees_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let trees: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "tree").collect();
        assert!(trees.len() >= 4, "seed {seed}: expected at least 4 trees, found {}", trees.len());
    }
}

#[test]
fn shrubs_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let shrubs: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "shrub").collect();
        assert!(!shrubs.is_empty(), "seed {seed}: no shrubs");
    }
}

#[test]
fn irrigation_heads_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let irr: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "irrigation_head").collect();
        assert!(!irr.is_empty(), "seed {seed}: no irrigation heads");
    }
}

// --- Fencing ---

#[test]
fn fencing_is_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let fences: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "fence").collect();
        let gates: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "gate").collect();
        assert!(fences.len() >= 4, "seed {seed}: expected at least 4 fence segments, found {}", fences.len());
        assert!(!gates.is_empty(), "seed {seed}: no gates");
        for f in &fences {
            assert!(f.height_ft.unwrap_or(0.0) > 0.0, "seed {seed}: fence has no height");
            assert!(f.length_ft.unwrap_or(0.0) > 0.0, "seed {seed}: fence has no length");
        }
    }
}

// --- Furnishings ---

#[test]
fn site_furnishings_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let benches: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "bench").collect();
        let bike_racks: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "bike_rack").collect();
        let trash_cans: Vec<_> = t.site_elements.iter().filter(|se| se.kind == "trash_can").collect();
        assert!(!benches.is_empty(), "seed {seed}: no benches");
        assert!(!bike_racks.is_empty(), "seed {seed}: no bike racks");
        assert!(!trash_cans.is_empty(), "seed {seed}: no trash cans");
    }
}

// --- Quantities ---

#[test]
fn site_quantities_are_computed() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        assert!(!t.site_quantities.is_empty(), "seed {seed}: no site quantities");
        // Should have at least earthwork, paving, curbs, fencing.
        let has_grading = t.site_quantities.iter().any(|q| q.csi.starts_with("31"));
        let has_paving = t.site_quantities.iter().any(|q| q.csi.starts_with("32 1"));
        let has_curb = t.site_quantities.iter().any(|q| q.description.contains("CURB"));
        let has_fence = t.site_quantities.iter().any(|q| q.description.contains("FENCE"));
        assert!(has_grading, "seed {seed}: no grading quantities");
        assert!(has_paving, "seed {seed}: no paving quantities");
        assert!(has_curb, "seed {seed}: no curb quantities");
        assert!(has_fence, "seed {seed}: no fence quantities");
    }
}

#[test]
fn quantities_have_positive_values() {
    let t = hotel_truth(1);
    for sq in &t.site_quantities {
        // One side of the earthwork may be zero (this site is all fill): it is measured, not
        // floored at a made-up minimum.
        let earthwork = sq.csi == "31 22 00" && sq.status == "read";
        assert!(
            sq.quantity.is_finite() && (sq.quantity > 0.0 || earthwork && sq.quantity == 0.0),
            "quantity '{}' has value {}",
            sq.description,
            sq.quantity
        );
        assert!(!sq.unit.is_empty(), "quantity '{}' has no unit", sq.description);
        assert!(!sq.csi.is_empty(), "quantity '{}' has no CSI code", sq.description);
    }
}

// --- Sheet generation ---

#[test]
fn grading_sheet_is_generated() {
    let set = hotel_set(1);
    let sheet = set.truth.sheets.iter().find(|s| s.number == "C-102");
    assert!(sheet.is_some(), "C-102 grading sheet not generated");
    assert!(set.file("C-102.pdf").is_some(), "C-102.pdf not generated");
    assert!(set.file("C-102.dxf").is_some(), "C-102.dxf not generated");
}

#[test]
fn site_utility_sheet_is_generated() {
    let set = hotel_set(1);
    let sheet = set.truth.sheets.iter().find(|s| s.number == "C-103");
    assert!(sheet.is_some(), "C-103 site utility sheet not generated");
    assert!(set.file("C-103.pdf").is_some(), "C-103.pdf not generated");
    assert!(set.file("C-103.dxf").is_some(), "C-103.dxf not generated");
}

#[test]
fn site_schedules_sheet_is_generated() {
    let set = hotel_set(1);
    let sheet = set.truth.sheets.iter().find(|s| s.number == "C-601");
    assert!(sheet.is_some(), "C-601 schedules sheet not generated");
    assert!(set.file("C-601.pdf").is_some(), "C-601.pdf not generated");
}

// --- Reader detects new element types ---

#[test]
fn reader_finds_paving_or_similar() {
    // Paving is a hatched outline or one labelled with its material; its area is read.
    for seed in 1..=3 {
        let (t, c) = read_civil(seed);
        for (material, kinds) in [("asphalt", &["paving_asphalt"][..]), ("concrete", &["paving_concrete", "sidewalk"][..])] {
            let truth: Vec<f64> = t.site_elements.iter().filter(|se| kinds.contains(&se.kind.as_str())).filter_map(|se| se.area_sf).collect();
            let read: Vec<f64> = c.features.iter().filter(|f| f.kind == "paving" && f.material == material).map(|f| f.area_sf).collect();
            assert_eq!(read.len(), truth.len(), "seed {seed}: {material} paving areas {read:?} vs truth {truth:?}");
            within(read.iter().sum(), truth.iter().sum(), 0.03, &format!("seed {seed}: {material} paving SF"));
        }
        assert!(!c.features.iter().any(|f| f.kind == "paving" && f.material == "unknown"), "seed {seed}: paving read without a material");
    }
}

#[test]
fn reader_finds_fence_layer_elements() {
    // Fences are lines with X marks along them; gates are circles on a fence line.
    for seed in 1..=3 {
        let (t, c) = read_civil(seed);
        for kind in ["fence", "curb"] {
            let truth: Vec<f64> = t.site_elements.iter().filter(|se| se.kind == kind).filter_map(|se| se.length_ft).collect();
            let read: Vec<f64> = c.features.iter().filter(|f| f.kind == kind).map(|f| f.length_ft).collect();
            assert_eq!(read.len(), truth.len(), "seed {seed}: {kind} runs {read:?} vs truth {truth:?}");
            within(read.iter().sum(), truth.iter().sum(), 0.03, &format!("seed {seed}: {kind} LF"));
        }
        let gates = t.site_elements.iter().filter(|se| se.kind == "gate").count();
        assert_eq!(c.features.iter().filter(|f| f.kind == "gate").count(), gates, "seed {seed}: gates");
    }
}

// --- Coordination with site electrical (Q309) ---

#[test]
fn site_elements_include_electrical_from_q309() {
    let t = hotel_truth(1);
    let ug_elec = t.site_elements.iter().filter(|se| se.kind == "underground_electric").count();
    assert!(ug_elec > 0, "no underground electric elements (Q309 coordination)");
}

// --- No panics ---

#[test]
fn sitework_no_panics_across_seeds() {
    for seed in 1..=5 {
        let _ = hotel_truth(seed);
    }
}

// --- Truth JSON round-trip ---

#[test]
fn truth_json_round_trip() {
    let t = hotel_truth(1);
    let json = serde_json::to_string_pretty(&t).expect("serialize");
    let t2: cadaddy_mepfix::truth::Truth = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(t.grading_contours.len(), t2.grading_contours.len());
    assert_eq!(t.site_structures.len(), t2.site_structures.len());
    assert_eq!(t.site_quantities.len(), t2.site_quantities.len());
    assert_eq!(t.site_elements.len(), t2.site_elements.len());
}

// --- Reading the civil sheets (Q339 rework) ---

fn within(got: f64, want: f64, rel: f64, what: &str) {
    assert!((got - want).abs() <= rel * want.abs().max(1e-9) + 1e-6, "{what}: read {got}, truth {want} (more than {}% apart)", rel * 100.0);
}

fn read_sheets(set: &cadaddy_mepfix::FixtureSet, edit: impl Fn(&str) -> String) -> st::CivilRead {
    let mut reads = Vec::new();
    for n in ["C-101", "C-102", "C-103"] {
        let bytes = set.file(&format!("{n}.dxf")).unwrap_or_else(|| panic!("{n}.dxf"));
        let text = edit(&String::from_utf8_lossy(bytes));
        let d = cadaddy_io::read_dxf(text.as_bytes()).unwrap_or_else(|e| panic!("{n}.dxf: {e}"));
        reads.push(st::read_sheet(&d, n, st::units_per_foot(&d)));
    }
    st::merge(reads)
}

/// The hotel's truth and what the civil reader reads off its C-101, C-102 and C-103.
fn read_civil(seed: u64) -> (Truth, st::CivilRead) {
    let set = hotel_set(seed);
    let c = read_sheets(&set, |s| s.to_string());
    (set.truth, c)
}

fn contour(id: &str, surface: Surface, z: f64, a: [f64; 2], b: [f64; 2]) -> Contour {
    Contour { id: id.into(), sheet: "C-102".into(), surface, elevation_ft: z, points_ft: vec![a, b] }
}

#[test]
fn earthwork_from_planar_surfaces_matches_the_analytic_cut_and_fill() {
    // Existing z = 100 + 0.04 x over a 100 ft square: contours every foot, x = 0, 25, 50, 75, 100.
    let existing: Vec<Contour> =
        (0..=4).map(|i| contour(&format!("ex{i}"), Surface::Existing, 100.0 + i as f64, [25.0 * i as f64, 0.0], [25.0 * i as f64, 100.0])).collect();
    // 1. Proposed flat at 101: cut where x > 25, fill where x < 25.
    //    cut = 100 * ∫25..100 (0.04x - 1) dx = 11 250 CF; fill = 100 * ∫0..25 (1 - 0.04x) dx = 1 250 CF.
    let mut cs = existing.clone();
    cs.push(contour("p0", Surface::Proposed, 101.0, [0.0, 0.0], [100.0, 0.0]));
    cs.push(contour("p1", Surface::Proposed, 101.0, [0.0, 100.0], [100.0, 100.0]));
    let e = st::earthwork(&cs, &[]).expect("both surfaces");
    within(e.cut_cy, 11_250.0 / 27.0, 0.02, "flat pad: cut CY");
    within(e.fill_cy, 1_250.0 / 27.0, 0.02, "flat pad: fill CY");
    within(e.graded_area_sf, 10_000.0, 0.01, "flat pad: graded SF");
    // 2. Proposed z = 100 + 0.02 y (contours y = 0, 50, 100): dz = 0.04x - 0.02y.
    //    cut = ∫0..100 (200 - 2y + 0.005y²) dy = 11 666.67 CF; fill = ∫0..100 0.005y² dy = 1 666.67 CF.
    let mut cs = existing;
    for (i, z) in [100.0, 101.0, 102.0].iter().enumerate() {
        let y = 50.0 * i as f64;
        cs.push(contour(&format!("q{i}"), Surface::Proposed, *z, [0.0, y], [100.0, y]));
    }
    let e = st::earthwork(&cs, &[]).expect("both surfaces");
    within(e.cut_cy, 11_666.667 / 27.0, 0.02, "sloped pad: cut CY");
    within(e.fill_cy, 1_666.667 / 27.0, 0.02, "sloped pad: fill CY");
    // The parts (each proposed contour's share) add back up to the totals.
    assert!((e.cut_parts.iter().map(|p| p.quantity).sum::<f64>() - e.cut_cy).abs() < 0.011, "{e:?}");
    assert!((e.fill_parts.iter().map(|p| p.quantity).sum::<f64>() - e.fill_cy).abs() < 0.011, "{e:?}");
    // Identical surfaces: nothing to move (no 100 CY floor).
    let same: Vec<Contour> =
        cs.iter().filter(|c| c.surface == Surface::Proposed).flat_map(|c| [c.clone(), Contour { surface: Surface::Existing, ..c.clone() }]).collect();
    let e = st::earthwork(&same, &[]).expect("both surfaces");
    assert!(e.cut_cy < 0.5 && e.fill_cy < 0.5, "identical surfaces move nothing: {e:?}");
}

#[test]
fn earthwork_without_a_surface_is_insufficient_data() {
    let ex = vec![
        contour("ex0", Surface::Existing, 100.0, [0.0, 0.0], [0.0, 100.0]),
        contour("ex1", Surface::Existing, 101.0, [100.0, 0.0], [100.0, 100.0]),
    ];
    let why = st::earthwork(&ex, &[]).expect_err("no proposed surface");
    assert!(why.contains("proposed"), "{why}");
    let q = st::takeoff(&st::CivilRead { contours: ex, ..st::CivilRead::default() });
    let line = q.iter().find(|q| q.csi == "31 22 00").expect("an earthwork line");
    assert_eq!((line.quantity, line.status.as_str()), (None, "insufficient data"), "{line:?}");
    assert_eq!(q.iter().filter(|q| q.csi == "31 22 00").count(), 1);
    // The truth's takeoff says the same when the grading plan has no proposed contours.
    let mut t = hotel_truth(1);
    t.grading_contours.retain(|c| c.surface == "existing");
    cadaddy_mepfix::sitework::compute_quantities(&mut t);
    let lines: Vec<&SiteQuantity> = t.site_quantities.iter().filter(|q| q.csi == "31 22 00").collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0].status, "insufficient data");
    assert_eq!(lines[0].quantity, 0.0);
    assert!(lines[0].elements.is_empty());
}

#[test]
fn truth_earthwork_is_the_volume_between_the_planes() {
    // The generator's surfaces are planes sloping in y: the fill (and cut) is the integral of
    // the difference over the area both cover, computed here in closed form.
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let plane = |surface: &str| {
            let cs: Vec<_> = t.grading_contours.iter().filter(|c| c.surface == surface).collect();
            let (a, b) = (cs.first().expect("two contours"), cs.last().expect("two contours"));
            let (ya, yb, za) = (a.points[0][1], b.points[0][1], a.elevation_ft);
            let slope = (b.elevation_ft - za) / (yb - ya);
            let xs = cs.iter().flat_map(|c| c.points.iter().map(|p| p[0]));
            let (x0, x1) = xs.fold((f64::MAX, f64::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
            (move |y: f64| za + slope * (y - ya), ya.min(yb), ya.max(yb), x0, x1)
        };
        let (e, ey0, ey1, ex0, ex1) = plane("existing");
        let (p, py0, py1, px0, px1) = plane("proposed");
        let (y0, y1, w) = (ey0.max(py0), ey1.min(py1), px1.min(ex1) - px0.max(ex0));
        // dz is linear in y: integrate its positive and negative parts exactly.
        let (d0, d1) = (p(y0) - e(y0), p(y1) - e(y1));
        let part = |a: f64, b: f64| -> f64 {
            if a >= 0.0 && b >= 0.0 {
                (a + b) / 2.0 * (y1 - y0)
            } else if a <= 0.0 && b <= 0.0 {
                0.0
            } else {
                let pos = a.max(b);
                pos * pos / (a - b).abs() / 2.0 * (y1 - y0)
            }
        };
        let fill = part(d0, d1) * w / 27.0;
        let cut = part(-d0, -d1) * w / 27.0;
        let q = |d: &str| t.site_quantities.iter().find(|q| q.description == d).unwrap_or_else(|| panic!("seed {seed}: {d}"));
        let (qc, qf) = (q("EARTHWORK, CUT (EXCAVATION)"), q("EARTHWORK, FILL (COMPACTED)"));
        assert_eq!((qc.status.as_str(), qf.status.as_str()), ("read", "read"));
        assert!(fill > 0.0, "seed {seed}: the pad needs fill");
        within(qf.quantity, fill, 0.02, &format!("seed {seed}: fill CY"));
        assert!((qc.quantity - cut).abs() <= 0.02 * cut + 1.0, "seed {seed}: cut {} vs {cut}", qc.quantity);
    }
}

#[test]
fn civil_reader_scores_against_truth() {
    for seed in 1..=3 {
        let (t, c) = read_civil(seed);
        // Contours by elevation, per surface.
        for (name, surface) in [("existing", Surface::Existing), ("proposed", Surface::Proposed)] {
            let mut want: Vec<f64> = t.grading_contours.iter().filter(|g| g.surface == name).map(|g| g.elevation_ft).collect();
            let mut got: Vec<f64> = c.contours.iter().filter(|g| g.surface == surface).map(|g| g.elevation_ft).collect();
            want.sort_by(f64::total_cmp);
            got.sort_by(f64::total_cmp);
            assert_eq!(got, want, "seed {seed}: {name} contours");
        }
        assert!(c.spots.is_empty(), "seed {seed}: no spot grades on these sheets, read {:?}", c.spots);
        // Structures: counts by kind exact, RIM/INV to 0.1 ft, manhole systems.
        assert!(c.unknown.is_empty(), "seed {seed}: unknown {:?}", c.unknown);
        assert_eq!(c.structures.len(), t.site_structures.len(), "seed {seed}: structures");
        for ts in &t.site_structures {
            let rs = c.structures.iter().find(|r| r.tag == ts.tag).unwrap_or_else(|| panic!("seed {seed}: {} not read", ts.tag));
            assert_eq!(rs.kind, ts.kind, "seed {seed}: {}", ts.tag);
            if ts.kind == "manhole" || ts.kind == "catch_basin" {
                assert_eq!(rs.system, ts.system, "seed {seed}: {}", ts.tag);
            }
            for (what, r, w) in [("RIM", rs.rim_ft, ts.rim_ft), ("INV", rs.invert_ft, ts.invert_ft)] {
                match (r, w) {
                    (Some(r), Some(w)) => assert!((r - w).abs() <= 0.1, "seed {seed}: {} {what} {r} vs {w}", ts.tag),
                    (None, None) => {}
                    _ => panic!("seed {seed}: {} {what} read {r:?}, truth {w:?}", ts.tag),
                }
            }
        }
        // Utility pipe: one run per system, size, material and depth as drawn, LF within 2%.
        for (kind, system) in [("underground_storm", "storm"), ("underground_sewer", "sanitary"), ("underground_water", "water")] {
            let truth: Vec<_> = t.site_elements.iter().filter(|se| se.kind == kind).collect();
            let read: Vec<_> = c.pipes.iter().filter(|p| p.system == system).collect();
            assert_eq!(read.len(), truth.len(), "seed {seed}: {system} runs");
            for (r, w) in read.iter().zip(&truth) {
                assert_eq!(r.size_in, w.size_in, "seed {seed}: {system} size");
                assert_eq!(r.material, w.material, "seed {seed}: {system} material");
                assert_eq!(r.depth_ft, w.depth_ft, "seed {seed}: {system} depth");
                within(r.length_ft, w.length_ft.expect("drawn length"), 0.02, &format!("seed {seed}: {system} LF"));
                assert_eq!(r.sheets, ["C-101", "C-103"], "seed {seed}: {system} drawn on the site and utility plans, counted once");
            }
        }
        // Earthwork from the read C-102 matches the truth's.
        let e = st::earthwork(&c.contours, &c.spots).expect("both surfaces read");
        let q = |d: &str| t.site_quantities.iter().find(|q| q.description == d).map_or(0.0, |q| q.quantity);
        within(e.fill_cy, q("EARTHWORK, FILL (COMPACTED)"), 0.02, &format!("seed {seed}: read fill CY"));
        assert!((e.cut_cy - q("EARTHWORK, CUT (EXCAVATION)")).abs() <= 0.02 * e.cut_cy + 1.0, "seed {seed}: read cut CY {}", e.cut_cy);
    }
}

#[test]
fn read_takeoff_matches_the_truth_takeoff() {
    for seed in 1..=3 {
        let (t, c) = read_civil(seed);
        let read = st::takeoff(&c);
        // What the sheets show is read: everything but the counted planting, furnishings,
        // striping and walls (drawn on C-101, not taken off by the civil reader).
        let skip = ["32 17 23", "32 32 23", "32 93 00", "32 84 00", "12 93 13"];
        for w in t.site_quantities.iter().filter(|q| !skip.contains(&q.csi.as_str())) {
            let r = read
                .iter()
                .find(|r| r.csi == w.csi && r.description == w.description)
                .unwrap_or_else(|| panic!("seed {seed}: no read line for {w:?}"));
            let rel = if w.unit == "LF" && w.csi.starts_with("33") { 0.02 } else { 0.03 };
            let got = r.quantity.unwrap_or_else(|| panic!("seed {seed}: {} has no number", r.description));
            if w.quantity == 0.0 {
                assert!(got <= 1.0, "seed {seed}: {} read {got}, truth 0", w.description);
            } else {
                within(got, w.quantity, rel, &format!("seed {seed}: {} ({})", w.description, w.unit));
            }
            assert_eq!(r.status, w.status, "seed {seed}: {}", w.description);
        }
        for r in &read {
            assert!(
                t.site_quantities.iter().any(|w| w.csi == r.csi && w.description == r.description),
                "seed {seed}: read line {} not in the truth",
                r.description
            );
        }
    }
}

#[test]
fn utility_piping_takeoff_by_system_size_and_material() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        let lf = |d: &str| t.site_quantities.iter().find(|q| q.description == d && q.unit == "LF").unwrap_or_else(|| panic!("seed {seed}: {d}"));
        for (kind, desc) in [
            ("underground_storm", "STORM PIPE, 12 IN. RCP"),
            ("underground_sewer", "SANITARY SEWER PIPE, 6 IN. PVC"),
            ("underground_water", "WATER PIPE, 4 IN. DIP"),
        ] {
            let truth: f64 = t.site_elements.iter().filter(|se| se.kind == kind).filter_map(|se| se.length_ft).sum();
            assert!(truth > 0.0);
            within(lf(desc).quantity, truth, 0.02, &format!("seed {seed}: {desc}"));
            assert_eq!(lf(desc).sources, ["C-101", "C-103"]);
        }
        // Trench excavation and backfill per system, by the stated rule.
        for name in ["STORM", "SANITARY SEWER", "WATER"] {
            let exc = t.site_quantities.iter().find(|q| q.description == format!("TRENCH EXCAVATION, {name} PIPE")).expect("trench");
            let back = t.site_quantities.iter().find(|q| q.description == format!("TRENCH BACKFILL, {name} PIPE")).expect("backfill");
            assert_eq!((exc.status.as_str(), exc.unit.as_str()), ("rule", "CY"));
            assert!(back.quantity > 0.0 && back.quantity < exc.quantity, "seed {seed}: {name} backfill {} vs trench {}", back.quantity, exc.quantity);
        }
        // Storm: 85 LF × (1 + 2 ft) wide × 6 ft deep = 56.67 CY.
        let storm = t.site_elements.iter().find(|se| se.kind == "underground_storm").expect("storm run");
        let (exc, _) = st::trench_cy(storm.length_ft.unwrap_or(0.0), 12.0, 6.0);
        within(exc, storm.length_ft.unwrap_or(0.0) * 3.0 * 6.0 / 27.0, 1e-9, "trench rule");
        // Storm and sanitary manholes on separate lines.
        let ea = |d: &str| t.site_quantities.iter().find(|q| q.description == d).map(|q| q.quantity);
        let count = |sys: &str| t.site_structures.iter().filter(|s| s.kind == "manhole" && s.system == sys).count() as f64;
        assert_eq!(ea("STORM MANHOLE"), Some(count("storm")));
        assert_eq!(ea("SANITARY MANHOLE"), Some(count("sanitary")));
        assert!(t.site_quantities.iter().all(|q| !q.description.contains("SANITARY/STORM")));
    }
}

#[test]
fn every_quantity_explains_back_to_its_elements() {
    for seed in 1..=3 {
        let set = hotel_set(seed);
        let t = &set.truth;
        let ids: Vec<&str> = t
            .site_elements
            .iter()
            .map(|e| e.id.as_str())
            .chain(t.site_structures.iter().map(|s| s.id.as_str()))
            .chain(t.grading_contours.iter().map(|c| c.id.as_str()))
            .collect();
        assert!(!t.site_quantities.is_empty());
        for q in &t.site_quantities {
            assert!(["read", "rule", "assumed"].contains(&q.status.as_str()), "seed {seed}: {q:?}");
            assert!(!q.sources.is_empty(), "seed {seed}: {} has no source sheet", q.description);
            let sum: f64 = q.elements.iter().map(|e| e.quantity).sum();
            assert!((sum - q.quantity).abs() < 0.005, "seed {seed}: {} = {} but its elements add to {sum}", q.description, q.quantity);
            assert!(q.quantity == 0.0 || !q.elements.is_empty(), "seed {seed}: {} has no elements", q.description);
            for e in &q.elements {
                assert!(ids.contains(&e.id.as_str()), "seed {seed}: {} names {} which isn't in the design", q.description, e.id);
            }
        }
        // The read takeoff's elements are objects of the sheets it names.
        let c = read_sheets(&set, |s| s.to_string());
        let drawings: Vec<(String, cadaddy_doc::Drawing)> = ["C-101", "C-102", "C-103"]
            .iter()
            .map(|n| (n.to_string(), cadaddy_io::read_dxf(set.file(&format!("{n}.dxf")).expect("dxf")).expect("parse")))
            .collect();
        for q in st::takeoff(&c) {
            assert!(["read", "rule", "assumed"].contains(&q.status.as_str()), "seed {seed}: {q:?}");
            let total = q.quantity.expect("a number");
            let sum: f64 = q.elements.iter().map(|e| e.quantity).sum();
            assert!((sum - total).abs() < 0.005, "seed {seed}: {} = {total} but its elements add to {sum}", q.description);
            assert!(!q.sources.is_empty() && (total == 0.0 || !q.elements.is_empty()), "seed {seed}: {q:?}");
            for e in &q.elements {
                let (sheet, handle) = e.id.split_once('#').unwrap_or_else(|| panic!("{} is sheet#handle", e.id));
                assert!(q.sources.iter().any(|s| s == sheet), "seed {seed}: {} element {} not on its sources {:?}", q.description, e.id, q.sources);
                let d = &drawings.iter().find(|(n, _)| n == sheet).expect("a read sheet").1;
                assert!(
                    d.model.iter().any(|x| x.handle.hex() == handle),
                    "seed {seed}: {} element {} isn't an object of {sheet}",
                    q.description,
                    e.id
                );
            }
        }
    }
}

#[test]
fn unknown_symbols_stay_unknown() {
    // A plain circle or triangle is named only by its tag: retagged, they are unknown.
    let set = hotel_set(1);
    let retag = |s: &str| s.replace("CO-1", "X-9").replace("WM-1", "Q-7");
    let c = read_sheets(&set, retag);
    let tags: Vec<&str> = c.unknown.iter().map(|u| u.tag.as_str()).collect();
    assert_eq!(tags.len(), 2, "{:?}", c.unknown);
    assert!(tags.contains(&"X-9") && tags.contains(&"Q-7"), "{tags:?}");
    assert!(!c.structures.iter().any(|s| s.tag == "X-9" || s.tag == "Q-7"));
    // The plan reader in this crate does the same.
    let sheet = set.truth.sheets.iter().find(|s| s.number == "C-103").expect("C-103");
    let text = retag(&String::from_utf8_lossy(set.file("C-103.dxf").expect("C-103.dxf")));
    let d = cadaddy_io::read_dxf(text.as_bytes()).expect("parse");
    let found = site_reader::read_site_structures(&d, sheet);
    for tag in ["X-9", "Q-7"] {
        let s = found.iter().find(|s| s.tag == tag).unwrap_or_else(|| panic!("{tag} not listed"));
        assert_eq!((s.kind.as_str(), s.system.as_str()), ("unknown", ""), "{tag}");
    }
}

#[test]
fn site_structure_reader_scores_against_truth() {
    for seed in 1..=3 {
        let set = hotel_set(seed);
        let sheet = set.truth.sheets.iter().find(|s| s.number == "C-103").expect("C-103");
        let d = cadaddy_io::read_dxf(set.file("C-103.dxf").expect("C-103.dxf")).expect("parse");
        let found = site_reader::read_site_structures(&d, sheet);
        assert_eq!(found.len(), set.truth.site_structures.len(), "seed {seed}: {found:?}");
        for ts in &set.truth.site_structures {
            let r = found.iter().find(|r| r.tag == ts.tag).unwrap_or_else(|| panic!("seed {seed}: {} not read", ts.tag));
            assert_eq!((r.kind.as_str(), r.system.as_str()), (ts.kind.as_str(), ts.system.as_str()), "seed {seed}: {}", ts.tag);
            assert!(
                (r.at_ft[0] - ts.at_ft[0]).abs() < 0.5 && (r.at_ft[1] - ts.at_ft[1]).abs() < 0.5,
                "seed {seed}: {} at {:?} vs {:?}",
                ts.tag,
                r.at_ft,
                ts.at_ft
            );
            for (what, r, w) in [("RIM", r.rim_ft, ts.rim_ft), ("INV", r.invert_ft, ts.invert_ft)] {
                match (r, w) {
                    (Some(r), Some(w)) => assert!((r - w).abs() <= 0.1, "seed {seed}: {} {what}", ts.tag),
                    (None, None) => {}
                    _ => panic!("seed {seed}: {} {what} read {r:?}, truth {w:?}", ts.tag),
                }
            }
        }
    }
}
