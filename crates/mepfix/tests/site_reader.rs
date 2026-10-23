//! Tests for site element reading against the Q254 hotel fixture generator's ground truth.
//!
//! Per-kind precision and recall tests (MUST 2) replace the old `> 0` existence checks.
//! Position tolerance: 3 ft for point elements; bbox overlap for underground runs.

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

fn fixture_full(seed: u64) -> (cadaddy_mepfix::truth::Truth, Vec<site_reader::ReadSiteElement>) {
    let set = generate_hotel(seed, HotelSize::Full).unwrap_or_else(|e| {
        panic!("generate_hotel({seed}, Full): {e}");
    });
    let sheet = set.truth.sheets.iter().find(|s| s.number == "C-101").expect("C-101 sheet not found");
    let dxf_bytes = set.file("C-101.dxf").expect("C-101.dxf not found");
    let drawing = cadaddy_io::read_dxf(dxf_bytes).expect("failed to parse C-101.dxf");
    let found = site_reader::read_sites(&drawing, sheet);
    (set.truth, found)
}

/// Check that a truth element at `at_ft` has a matching found element within `tol` feet.
fn matched_point(truth_at: [f64; 2], found: &[site_reader::ReadSiteElement], kind: &str, tol: f64) -> bool {
    found.iter().any(|f| {
        f.kind == kind && (f.at_ft[0] - truth_at[0]).hypot(f.at_ft[1] - truth_at[1]) < tol
    })
}

/// For underground runs: check bbox overlap between truth and found.
fn bbox_overlap(t_mn: [f64; 2], t_mx: [f64; 2], f_mn: [f64; 2], f_mx: [f64; 2]) -> bool {
    let x_over = t_mn[0].max(f_mn[0]) < t_mx[0].min(f_mx[0]);
    let y_over = t_mn[1].max(f_mn[1]) < t_mx[1].min(f_mx[1]);
    x_over && y_over
}

/// Compute per-kind precision and recall. Returns (recall, precision).
fn precision_recall(
    truth: &cadaddy_mepfix::truth::Truth,
    found: &[site_reader::ReadSiteElement],
    kind: &str,
    tol: f64,
) -> (f64, f64) {
    let truth_elems: Vec<_> = truth.site_elements.iter().filter(|se| se.kind == kind).collect();
    let found_elems: Vec<_> = found.iter().filter(|se| se.kind == kind).collect();

    if truth_elems.is_empty() && found_elems.is_empty() {
        return (1.0, 1.0);
    }
    if truth_elems.is_empty() {
        return (1.0, 0.0);
    }
    if found_elems.is_empty() {
        return (0.0, 1.0);
    }

    let is_underground = kind.starts_with("underground_");

    // Recall: how many truth elements have a matching found element.
    let mut true_positives = 0;
    let mut used_found = vec![false; found_elems.len()];
    for te in &truth_elems {
        let best = found_elems.iter().enumerate().filter(|(i, _)| !used_found[*i]).min_by(|(_, a), (_, b)| {
            let da = (a.at_ft[0] - te.at_ft[0]).hypot(a.at_ft[1] - te.at_ft[1]);
            let db = (b.at_ft[0] - te.at_ft[0]).hypot(b.at_ft[1] - te.at_ft[1]);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some((i, fe)) = best {
            let matches = if is_underground {
                // For runs: bbox overlap.
                if let (Some(t_mn), Some(t_mx), Some(f_mn), Some(f_mx)) = (te.min_ft, te.max_ft, fe.min_ft, fe.max_ft) {
                    bbox_overlap(t_mn, t_mx, f_mn, f_mx)
                } else {
                    false
                }
            } else {
                (fe.at_ft[0] - te.at_ft[0]).hypot(fe.at_ft[1] - te.at_ft[1]) < tol
            };
            if matches {
                true_positives += 1;
                used_found[i] = true;
            }
        }
    }

    let recall = true_positives as f64 / truth_elems.len() as f64;
    let precision = true_positives as f64 / found_elems.len() as f64;
    (recall, precision)
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

// --- MUST 2: Per-kind precision/recall >= 0.95 on seeds 1-3 Small + seed 1 Full ---

const TESTED_KINDS: &[&str] = &[
    "light_pole",
    "parking_space",
    "transformer_pad",
    "fire_hydrant",
    "dumpster_enclosure",
    "driveway",
    "underground_water",
    "underground_sewer",
    "underground_electric",
    "underground_gas",
    "underground_storm",
    "underground_telecom",
];

#[test]
fn per_kind_precision_recall_seeds_1_3_small() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        for kind in TESTED_KINDS {
            let truth_count = truth.site_elements.iter().filter(|se| se.kind == *kind).count();
            if truth_count == 0 {
                continue;
            }
            let (recall, precision) = precision_recall(&truth, &found, kind, 3.0);
            assert!(
                recall >= 0.95,
                "seed {seed} kind {kind}: recall {recall:.3} < 0.95 (truth {truth_count}, found {})",
                found.iter().filter(|se| se.kind == *kind).count()
            );
            assert!(
                precision >= 0.95,
                "seed {seed} kind {kind}: precision {precision:.3} < 0.95 (truth {truth_count}, found {})",
                found.iter().filter(|se| se.kind == *kind).count()
            );
        }
    }
}

#[test]
fn per_kind_precision_recall_seed_1_full() {
    let (truth, found) = fixture_full(1);
    for kind in TESTED_KINDS {
        let truth_count = truth.site_elements.iter().filter(|se| se.kind == *kind).count();
        if truth_count == 0 {
            continue;
        }
        let (recall, precision) = precision_recall(&truth, &found, kind, 3.0);
        assert!(
            recall >= 0.95,
            "Full seed 1 kind {kind}: recall {recall:.3} < 0.95 (truth {truth_count}, found {})",
            found.iter().filter(|se| se.kind == *kind).count()
        );
        assert!(
            precision >= 0.95,
            "Full seed 1 kind {kind}: precision {precision:.3} < 0.95 (truth {truth_count}, found {})",
            found.iter().filter(|se| se.kind == *kind).count()
        );
    }
}

// --- MUST 1: Depth correctness ---

#[test]
fn underground_depths_match_truth() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        for te in truth.site_elements.iter().filter(|se| se.depth_ft.is_some()) {
            let truth_depth = te.depth_ft.unwrap();
            // Find the matched found element (same kind, bbox overlap).
            let matched = found.iter().find(|f| {
                f.kind == te.kind
                    && if let (Some(t_mn), Some(t_mx), Some(f_mn), Some(f_mx)) = (te.min_ft, te.max_ft, f.min_ft, f.max_ft) {
                        bbox_overlap(t_mn, t_mx, f_mn, f_mx)
                    } else {
                        (f.at_ft[0] - te.at_ft[0]).hypot(f.at_ft[1] - te.at_ft[1]) < 5.0
                    }
            });
            let found_depth = matched.and_then(|f| f.depth_ft);
            assert!(
                found_depth.is_some(),
                "seed {seed} kind {}: truth depth {truth_depth:.1} but no depth found",
                te.kind
            );
            let fd = found_depth.unwrap();
            assert!(
                (fd - truth_depth).abs() < 0.1,
                "seed {seed} kind {}: truth depth {truth_depth:.1}, found {fd:.1} (diff {:.2})",
                te.kind,
                (fd - truth_depth).abs()
            );
        }
    }
}

// --- MUST 3: Conduit type, size, count, and route length ---

#[test]
fn underground_conduit_type_size_count() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        for te in truth.site_elements.iter().filter(|se| se.kind.starts_with("underground_") && !se.material.is_empty()) {
            let matched = found.iter().find(|f| {
                f.kind == te.kind
                    && if let (Some(t_mn), Some(t_mx), Some(f_mn), Some(f_mx)) = (te.min_ft, te.max_ft, f.min_ft, f.max_ft) {
                        bbox_overlap(t_mn, t_mx, f_mn, f_mx)
                    } else {
                        false
                    }
            });
            assert!(
                matched.is_some(),
                "seed {seed} kind {}: truth has conduit type {} but no matching found element",
                te.kind, te.material
            );
            let m = matched.unwrap();
            // Check conduit type.
            assert!(
                !m.conduit_type.is_empty(),
                "seed {seed} kind {}: truth conduit type {}, found empty",
                te.kind, te.material
            );
            assert_eq!(
                m.conduit_type, te.material,
                "seed {seed} kind {}: conduit type mismatch",
                te.kind
            );
            // Check size.
            assert_eq!(
                m.size_in, te.size_in,
                "seed {seed} kind {}: size mismatch (truth {:?}, found {:?})",
                te.kind, te.size_in, m.size_in
            );
            // Check conduit count.
            assert_eq!(
                m.conduit_count,
                te.conduit_count,
                "seed {seed} kind {}: conduit count mismatch (truth {:?}, found {:?})",
                te.kind, te.conduit_count, m.conduit_count
            );
        }
    }
}

#[test]
fn underground_route_length() {
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        for te in truth.site_elements.iter().filter(|se| se.kind.starts_with("underground_")) {
            let truth_len = if let (Some(mn), Some(mx)) = (te.min_ft, te.max_ft) {
                ((mx[0] - mn[0]).powi(2) + (mx[1] - mn[1]).powi(2)).sqrt()
            } else {
                continue;
            };
            if truth_len < 1.0 {
                continue;
            }
            let matched = found.iter().find(|f| {
                f.kind == te.kind
                    && if let (Some(t_mn), Some(t_mx), Some(f_mn), Some(f_mx)) = (te.min_ft, te.max_ft, f.min_ft, f.max_ft) {
                        bbox_overlap(t_mn, t_mx, f_mn, f_mx)
                    } else {
                        false
                    }
            });
            if let Some(m) = matched {
                assert!(
                    !m.route_ft.is_empty(),
                    "seed {seed} kind {}: matched but route_ft is empty",
                    te.kind
                );
                let route_len: f64 = m
                    .route_ft
                    .windows(2)
                    .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
                    .sum();
                let ratio = route_len / truth_len;
                assert!(
                    (ratio - 1.0).abs() < 0.02,
                    "seed {seed} kind {}: route length {route_len:.1} vs truth {truth_len:.1} (ratio {ratio:.4})",
                    te.kind
                );
            }
        }
    }
}

// --- MUST 4: New kinds (pull_box, pole_base, ev_charger, monument_sign) ---

#[test]
fn new_kinds_count_and_position() {
    let new_kinds = ["pull_box", "pole_base", "ev_charger", "monument_sign"];
    for seed in 1..=3 {
        let (truth, found) = fixture(seed);
        for kind in &new_kinds {
            let truth_count = truth.site_elements.iter().filter(|se| se.kind == *kind).count();
            let found_count = found.iter().filter(|se| se.kind == *kind).count();
            assert!(truth_count > 0, "seed {seed}: no {kind} in truth");
            if found_count != truth_count {
                // Diagnostic: show what we found near the truth position.
                for te in truth.site_elements.iter().filter(|se| se.kind == *kind) {
                    let nearby: Vec<_> = found
                        .iter()
                        .filter(|f| (f.at_ft[0] - te.at_ft[0]).hypot(f.at_ft[1] - te.at_ft[1]) < 10.0)
                        .map(|f| format!("{}:{} at({:.1},{:.1})", f.kind, f.tag, f.at_ft[0], f.at_ft[1]))
                        .collect();
                    eprintln!(
                        "seed {seed} {kind}: truth at ({:.1},{:.1}), nearby found: {:?}",
                        te.at_ft[0], te.at_ft[1], nearby
                    );
                }
            }
            assert_eq!(
                found_count, truth_count,
                "seed {seed} kind {kind}: truth {truth_count}, found {found_count}"
            );
            for te in truth.site_elements.iter().filter(|se| se.kind == *kind) {
                assert!(
                    matched_point(te.at_ft, &found, kind, 3.0),
                    "seed {seed} kind {kind}: truth at ({:.1}, {:.1}) not found within 3 ft",
                    te.at_ft[0],
                    te.at_ft[1]
                );
            }
        }
    }
}

// --- MUST 6: Sheet, confidence, and unknown ---

#[test]
fn every_result_has_sheet_and_confidence() {
    for seed in 1..=3 {
        let (_truth, found) = fixture(seed);
        for f in &found {
            assert!(!f.sheet.is_empty(), "seed {seed} kind {}: empty sheet", f.kind);
            assert!(
                f.confidence > 0.0 && f.confidence <= 1.0,
                "seed {seed} kind {}: confidence {} out of range",
                f.kind,
                f.confidence
            );
        }
    }
}

#[test]
fn unrecognised_label_gives_unknown() {
    use cadaddy_doc::geom::Vec3;
    use cadaddy_doc::{Entity, EntityKind, Handle, Line, Text};

    let mut d = cadaddy_doc::Drawing::new_imperial();
    // Add a line on C-UNDR.
    let mut e1 = Entity::new(Handle(9000), EntityKind::Line(Line {
        a: Vec3::new(0.0, 0.0, 0.0),
        b: Vec3::new(100.0, 0.0, 0.0),
    }));
    e1.common.layer = "C-UNDR".into();
    d.model.push(e1);
    // Add an unrecognised label near it.
    let mut e2 = Entity::new(Handle(9001), EntityKind::Text(Text {
        insert: Vec3::new(50.0, 5.0, 0.0),
        align_pt: None,
        height: 1.0,
        value: "SOMETHING STRANGE".into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: cadaddy_doc::HAlign::Left,
        valign: cadaddy_doc::VAlign::Baseline,
    }));
    e2.common.layer = "C-UNDR".into();
    d.model.push(e2);
    let sheet = cadaddy_mepfix::truth::Sheet::default();
    let found = site_reader::read_sites(&d, &sheet);
    let ug = found.iter().find(|f| f.kind.starts_with("underground_") || f.kind == "unknown");
    assert!(ug.is_some(), "expected an underground/unknown element");
    assert_eq!(ug.unwrap().kind, "unknown", "unrecognised label should give kind 'unknown', got '{}'", ug.unwrap().kind);
}

#[test]
fn no_panics_on_empty_drawing() {
    let drawing = cadaddy_doc::Drawing::new_imperial();
    let sheet = cadaddy_mepfix::truth::Sheet::default();
    let found = site_reader::read_sites(&drawing, &sheet);
    assert!(found.is_empty());
}
