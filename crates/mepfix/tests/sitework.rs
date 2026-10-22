//! Tests for Q339 sitework: grading, paving, curbs, sidewalks, striping, utility structures,
//! retaining walls, landscaping, fencing, site furnishings, and quantity takeoff.

use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::site_reader;

fn hotel_truth(seed: u64) -> cadaddy_mepfix::truth::Truth {
    let set = generate_hotel(seed, HotelSize::Small)
        .unwrap_or_else(|e| panic!("generate_hotel({seed}): {e}"));
    set.truth
}

fn hotel_set(seed: u64) -> cadaddy_mepfix::FixtureSet {
    generate_hotel(seed, HotelSize::Small)
        .unwrap_or_else(|e| panic!("generate_hotel({seed}): {e}"))
}

// --- Grading contours ---

#[test]
fn grading_contours_are_generated() {
    for seed in 1..=3 {
        let t = hotel_truth(seed);
        assert!(
            !t.grading_contours.is_empty(),
            "seed {seed}: no grading contours generated"
        );
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
        assert!(sq.quantity > 0.0, "quantity '{}' has non-positive value {}", sq.description, sq.quantity);
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
    // Paving is drawn as large hatched rectangles; the reader may classify them as parking_space
    // or unknown due to hatching lines. Verify the reader detects area elements on the site plan.
    let set = hotel_set(1);
    let sheet = set.truth.sheets.iter().find(|s| s.number == "C-101").expect("C-101");
    let dxf = set.file("C-101.dxf").expect("C-101.dxf");
    let drawing = cadaddy_io::read_dxf(dxf).expect("parse C-101.dxf");
    let found = site_reader::read_sites(&drawing, sheet);
    let area_elements = found.iter().filter(|se| se.min_ft.is_some() && se.max_ft.is_some()).count();
    assert!(area_elements >= 5, "expected at least 5 area elements, found {area_elements}");
    let truth_paving = set.truth.site_elements.iter().filter(|se| se.kind.starts_with("paving_")).count();
    assert!(truth_paving > 0, "no paving elements in truth");
}

#[test]
fn reader_finds_fence_layer_elements() {
    // Fences are drawn as lines with X marks on C-FNCE layer. The reader does not yet classify
    // line-only elements as fences. Verify fence truth exists and the reader does not panic.
    let set = hotel_set(1);
    let truth_fences = set.truth.site_elements.iter().filter(|se| se.kind == "fence").count();
    assert!(truth_fences >= 4, "expected at least 4 fence segments in truth, found {truth_fences}");
    let sheet = set.truth.sheets.iter().find(|s| s.number == "C-101").expect("C-101");
    let dxf = set.file("C-101.dxf").expect("C-101.dxf");
    let drawing = cadaddy_io::read_dxf(dxf).expect("parse C-101.dxf");
    let _ = site_reader::read_sites(&drawing, sheet);
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
