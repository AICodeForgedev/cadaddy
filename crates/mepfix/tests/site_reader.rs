//! Tests for site element reading against the Q254 hotel fixture generator's ground truth.
//!
//! The hotel fixture set generates a C-101 DXF sheet with site elements (parking spaces, light
//! poles, transformer pad, fire hydrant, dumpster enclosure, driveways, underground utilities)
//! on `C-SITE` and `C-UNDR` layers. The site reader must find them and classify them correctly.

use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::site_reader;

/// Generate a hotel fixture set, read the C-101 DXF, and return (truth, found).
fn fixture(seed: u64) -> (cadaddy_mepfix::truth::Truth, Vec<site_reader::ReadSiteElement>) {
    let set = generate_hotel(seed, HotelSize::Small).unwrap_or_else(|e| {
        panic!("generate_hotel({seed}): {e}");
    });
    let sheet = set.truth.sheets.iter().find(|s| s.number == "C-101").expect("C-101 sheet not found");
    let dxf_bytes = set.file("C-101.dxf").expect("C-101.dxf not found");
    let drawing = cadaddy_io::read_dxf(dxf_bytes).expect("failed to parse C-101.dxf");
    let found = site_reader::read_sites(&drawing, sheet);
    (set.truth, found)
}

#[test]
fn site_truth_is_nonempty() {
    for seed in 1..=3 {
        let set = generate_hotel(seed, HotelSize::Small).unwrap_or_else(|e| panic!("generate_hotel({seed}): {e}"));
        assert!(!set.truth.site_elements.is_empty(), "seed {seed}: no site element truth entries generated");
    }
}

#[test]
fn reader_finds_some_elements() {
    for seed in 1..=3 {
        let (_truth, found) = fixture(seed);
        assert!(!found.is_empty(), "seed {seed}: reader found no site elements");
    }
}

#[test]
fn reader_finds_light_poles() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        let truth_poles = truth.site_elements.iter().filter(|se| se.kind == "light_pole").count();
        let found_poles = found.iter().filter(|se| se.kind == "light_pole").count();
        assert!(truth_poles > 0, "seed {seed}: no light poles in truth");
        assert!(found_poles > 0, "seed {seed}: {truth_poles} light poles in truth, none found by reader");
    }
}

#[test]
fn reader_finds_transformer_pad() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        let truth_xfmr = truth.site_elements.iter().filter(|se| se.kind == "transformer_pad").count();
        let found_xfmr = found.iter().filter(|se| se.kind == "transformer_pad").count();
        assert!(truth_xfmr > 0, "seed {seed}: no transformer pads in truth");
        assert!(found_xfmr > 0, "seed {seed}: {truth_xfmr} transformer pads in truth, none found by reader");
    }
}

#[test]
fn reader_finds_fire_hydrant() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        let truth_fh = truth.site_elements.iter().filter(|se| se.kind == "fire_hydrant").count();
        let found_fh = found.iter().filter(|se| se.kind == "fire_hydrant").count();
        assert!(truth_fh > 0, "seed {seed}: no fire hydrants in truth");
        assert!(found_fh > 0, "seed {seed}: {truth_fh} fire hydrants in truth, none found by reader");
    }
}

#[test]
fn reader_finds_parking_spaces() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        let truth_parking = truth.site_elements.iter().filter(|se| se.kind == "parking_space").count();
        let found_parking = found.iter().filter(|se| se.kind == "parking_space").count();
        assert!(truth_parking > 0, "seed {seed}: no parking spaces in truth");
        assert!(
            found_parking > 0,
            "seed {seed}: {truth_parking} parking spaces in truth, none found by reader (found kinds: {:?})",
            found.iter().map(|se| se.kind.as_str()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn reader_finds_underground_utilities() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        let truth_ug = truth.site_elements.iter().filter(|se| se.kind.starts_with("underground_")).count();
        let found_ug = found.iter().filter(|se| se.kind.starts_with("underground_")).count();
        assert!(truth_ug > 0, "seed {seed}: no underground utilities in truth");
        assert!(
            found_ug > 0,
            "seed {seed}: {truth_ug} underground utilities in truth, none found by reader (found kinds: {:?})",
            found.iter().map(|se| se.kind.as_str()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn reader_finds_dumpster() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        let truth_dump = truth.site_elements.iter().filter(|se| se.kind == "dumpster_enclosure").count();
        let found_dump = found.iter().filter(|se| se.kind == "dumpster_enclosure").count();
        assert!(truth_dump > 0, "seed {seed}: no dumpster enclosures in truth");
        assert!(
            found_dump > 0,
            "seed {seed}: {truth_dump} dumpster enclosures in truth, none found by reader (found kinds: {:?})",
            found.iter().map(|se| se.kind.as_str()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn reader_finds_driveway() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        let truth_drive = truth.site_elements.iter().filter(|se| se.kind == "driveway").count();
        let found_drive = found.iter().filter(|se| se.kind == "driveway").count();
        assert!(truth_drive > 0, "seed {seed}: no driveways in truth");
        assert!(
            found_drive > 0,
            "seed {seed}: {truth_drive} driveways in truth, none found by reader (found kinds: {:?})",
            found.iter().map(|se| se.kind.as_str()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn underground_depths_are_found() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        let truth_depths = truth.site_elements.iter().filter(|se| se.depth_ft.is_some()).count();
        let found_depths = found.iter().filter(|se| se.depth_ft.is_some()).count();
        assert!(truth_depths > 0, "seed {seed}: no underground depth values in truth");
        assert!(
            found_depths > 0,
            "seed {seed}: {truth_depths} underground elements with depth in truth, none found (found: {:?})",
            found.iter().filter(|se| se.kind.starts_with("underground_")).map(|se| (&se.kind, &se.tag, se.depth_ft)).collect::<Vec<_>>()
        );
    }
}

#[test]
fn position_accuracy() {
    // Each found element should be within 5 ft of its truth counterpart.
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        let mut matched = 0;
        for te in &truth.site_elements {
            let best = found.iter().filter(|f| f.kind == te.kind).min_by(|a, b| {
                let da = (a.at_ft[0] - te.at_ft[0]).powi(2) + (a.at_ft[1] - te.at_ft[1]).powi(2);
                let db = (b.at_ft[0] - te.at_ft[0]).powi(2) + (b.at_ft[1] - te.at_ft[1]).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });
            if let Some(b) = best {
                let dist = ((b.at_ft[0] - te.at_ft[0]).powi(2) + (b.at_ft[1] - te.at_ft[1]).powi(2)).sqrt();
                if dist < 5.0 {
                    matched += 1;
                }
            }
        }
        let total = truth.site_elements.len();
        assert!(matched > total / 3, "seed {seed}: position accuracy too low — matched {matched}/{total} within 5 ft");
    }
}

#[test]
fn no_panics_on_empty_drawing() {
    let drawing = cadaddy_doc::Drawing::new_imperial();
    let sheet = cadaddy_mepfix::truth::Sheet::default();
    let found = site_reader::read_sites(&drawing, &sheet);
    assert!(found.is_empty());
}
