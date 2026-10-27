//! Sitework design: grading contours, building pad, paving, curbs, sidewalks, striping,
//! utility structures, retaining walls, landscaping, irrigation, fencing, site furnishings,
//! and quantity takeoff. Called from the hotel design to extend the site elements from Q309.

use crate::rng::Rng;
use crate::truth::*;

/// Layer names for the civil sheets.
pub const GRADING_LAYER: &str = "C-GRAD";
pub const PAVING_LAYER: &str = "C-PAVE";
pub const LANDSCAPE_LAYER: &str = "C-LAND";
pub const FENCE_LAYER: &str = "C-FNCE";
pub const FURNISH_LAYER: &str = "C-FURN";
pub const STRUCTURE_LAYER: &str = "C-STRC";
/// Existing-ground contours on the grading plan (proposed ones stay on [`GRADING_LAYER`]).
pub const EXISTING_GRADING_LAYER: &str = "C-GRAD-EXST";

/// Design all sitework elements for a hotel.
pub fn design_sitework(t: &mut Truth, bldg_w: f64, bldg_d: f64, r: &mut Rng) {
    let setback = 30.0;
    let parking_south = -setback - 60.0;

    design_grading(t, bldg_w, bldg_d, setback, r);
    design_building_pad(t, bldg_w, bldg_d);
    design_paving(t, bldg_w, bldg_d, setback, parking_south);
    design_curbs_sidewalks(t, bldg_w, bldg_d, setback, parking_south);
    design_striping(t, bldg_w, setback, parking_south);
    design_utility_structures(t, bldg_w, bldg_d, setback);
    design_retaining_walls(t, bldg_w, bldg_d, setback, r);
    design_landscaping(t, bldg_w, bldg_d, setback, r);
    design_fencing(t, bldg_w, bldg_d, setback, parking_south);
    design_furnishings(t, bldg_w, setback, parking_south, r);
    design_utility_piping(t);
    compute_quantities(t);
}

fn next_se_id(t: &Truth) -> usize {
    t.site_elements.iter().filter_map(|se| se.id.strip_prefix("se-").and_then(|n| n.parse::<usize>().ok())).max().map_or(0, |m| m + 1)
}

fn next_ss_id(t: &Truth) -> usize {
    t.site_structures.iter().filter_map(|s| s.id.strip_prefix("ss-").and_then(|n| n.parse::<usize>().ok())).max().map_or(0, |m| m + 1)
}

fn next_gc_id(t: &Truth) -> usize {
    t.grading_contours.iter().filter_map(|c| c.id.strip_prefix("gc-").and_then(|n| n.parse::<usize>().ok())).max().map_or(0, |m| m + 1)
}

// --- Grading ---

fn design_grading(t: &mut Truth, bldg_w: f64, bldg_d: f64, setback: f64, r: &mut Rng) {
    let mut gc_id = next_gc_id(t);
    let base_elev = 100.0; // datum elevation
    let pad_elev = base_elev + 1.0; // building pad 1 ft above grade

    // Existing contours: gently sloping across the site.
    let _site_w = bldg_w + 2.0 * setback + 40.0;
    let site_lo_y = -setback - 80.0;
    let site_hi_y = bldg_d + setback + 20.0;
    let existing_slope = r.range(1, 3) as f64; // 1-3% slope north to south

    for elev_offset in -3i32..=3 {
        let elev = base_elev + elev_offset as f64;
        let y_at_elev = site_lo_y + (elev - (base_elev - existing_slope * (site_hi_y - site_lo_y) / 100.0)) * 100.0 / existing_slope;
        if y_at_elev < site_lo_y || y_at_elev > site_hi_y {
            continue;
        }
        let weight = if elev_offset % 5 == 0 { "major" } else { "minor" };
        t.grading_contours.push(GradingContour {
            id: format!("gc-{gc_id}"),
            surface: "existing".into(),
            elevation_ft: elev,
            points: vec![[-setback - 20.0, y_at_elev], [bldg_w + setback + 20.0, y_at_elev]],
            weight: weight.into(),
        });
        gc_id += 1;
    }

    // Proposed contours: flat pad around building, drainage slopes away.
    for elev_offset in -2i32..=2 {
        let elev = pad_elev + elev_offset as f64 * 0.5;
        let y_at_elev = -setback + (elev - pad_elev) * 50.0; // 2% slope
        if y_at_elev < site_lo_y || y_at_elev > site_hi_y {
            continue;
        }
        let weight = if (elev * 10.0).round() as i32 % 50 == 0 { "major" } else { "minor" };
        t.grading_contours.push(GradingContour {
            id: format!("gc-{gc_id}"),
            surface: "proposed".into(),
            elevation_ft: elev,
            points: vec![[-setback - 10.0, y_at_elev], [bldg_w + setback + 10.0, y_at_elev]],
            weight: weight.into(),
        });
        gc_id += 1;
    }

    let _ = gc_id;
}

// --- Building pad ---

fn design_building_pad(t: &mut Truth, bldg_w: f64, bldg_d: f64) {
    let mut se_id = next_se_id(t);
    let pad_margin = 5.0; // 5 ft beyond building footprint
    let min = [-pad_margin, -pad_margin];
    let max = [bldg_w + pad_margin, bldg_d + pad_margin];
    let area = (max[0] - min[0]) * (max[1] - min[1]);
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "building_pad".into(),
        tag: "BLDG PAD".into(),
        at_ft: [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0],
        min_ft: Some(min),
        max_ft: Some(max),
        material: "compacted_fill".into(),
        area_sf: Some(area),
        ..SiteElement::default()
    });
    se_id += 1;
    let _ = se_id;
}

// --- Paving ---

fn design_paving(t: &mut Truth, bldg_w: f64, _bldg_d: f64, setback: f64, parking_south: f64) {
    let mut se_id = next_se_id(t);

    // Parking lot: asphalt paving.
    let park_min = [-setback, parking_south];
    let park_max = [bldg_w + setback, -setback];
    let park_area = (park_max[0] - park_min[0]) * (park_max[1] - park_min[1]);
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "paving_asphalt".into(),
        tag: "PARKING LOT ASPHALT".into(),
        at_ft: [(park_min[0] + park_max[0]) / 2.0, (park_min[1] + park_max[1]) / 2.0],
        min_ft: Some(park_min),
        max_ft: Some(park_max),
        material: "asphalt".into(),
        area_sf: Some(park_area),
        ..SiteElement::default()
    });
    se_id += 1;

    // Sidewalk from building entrance: concrete paving.
    let sw_min = [bldg_w / 2.0 - 3.0, -setback];
    let sw_max = [bldg_w / 2.0 + 3.0, 0.0];
    let sw_area = (sw_max[0] - sw_min[0]) * (sw_max[1] - sw_min[1]);
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "paving_concrete".into(),
        tag: "ENTRY WALK".into(),
        at_ft: [(sw_min[0] + sw_max[0]) / 2.0, (sw_min[1] + sw_max[1]) / 2.0],
        min_ft: Some(sw_min),
        max_ft: Some(sw_max),
        material: "concrete".into(),
        area_sf: Some(sw_area),
        ..SiteElement::default()
    });
    se_id += 1;

    let _ = se_id;
}

// --- Curbs and sidewalks ---

fn design_curbs_sidewalks(t: &mut Truth, bldg_w: f64, bldg_d: f64, setback: f64, parking_south: f64) {
    let mut se_id = next_se_id(t);

    // Curb around parking lot perimeter.
    let curb_segments = [
        ([-setback, parking_south], [bldg_w + setback, parking_south], "CURB-S"),
        ([bldg_w + setback, parking_south], [bldg_w + setback, -setback], "CURB-E"),
        ([-setback, parking_south], [-setback, -setback], "CURB-W"),
    ];
    for (a, b, tag) in curb_segments {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "curb".into(),
            tag: tag.into(),
            at_ft: [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
            min_ft: Some([a[0].min(b[0]), a[1].min(b[1])]),
            max_ft: Some([a[0].max(b[0]), a[1].max(b[1])]),
            material: "concrete".into(),
            length_ft: Some(len),
            height_ft: Some(0.5), // 6-inch curb
            ..SiteElement::default()
        });
        se_id += 1;
    }

    // Sidewalk along building north face.
    let sw_len = bldg_w + 2.0 * setback;
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "sidewalk".into(),
        tag: "SIDEWALK-N".into(),
        at_ft: [bldg_w / 2.0, bldg_d + setback - 2.0],
        min_ft: Some([-setback, bldg_d + setback - 5.0]),
        max_ft: Some([bldg_w + setback, bldg_d + setback]),
        material: "concrete".into(),
        length_ft: Some(sw_len),
        area_sf: Some(sw_len * 5.0),
        ..SiteElement::default()
    });
    se_id += 1;

    let _ = se_id;
}

// --- Striping ---

fn design_striping(t: &mut Truth, bldg_w: f64, setback: f64, parking_south: f64) {
    let mut se_id = next_se_id(t);
    let space_width = 9.0;

    // Striping lines between parking spaces.
    let mut px = -setback + 5.0;
    let mut stripe_idx = 1;
    while px + space_width < bldg_w + setback - 5.0 {
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "striping".into(),
            tag: format!("STR-{stripe_idx}"),
            at_ft: [px + space_width, parking_south + 14.0],
            min_ft: Some([px + space_width - 0.17, parking_south + 5.0]),
            max_ft: Some([px + space_width + 0.17, parking_south + 23.0]),
            material: "paint".into(),
            length_ft: Some(18.0),
            ..SiteElement::default()
        });
        se_id += 1;
        stripe_idx += 1;
        px += space_width;
    }

    // Handicap striping at entrance.
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "striping".into(),
        tag: "HC-1".into(),
        at_ft: [bldg_w / 2.0 - 15.0, parking_south + 14.0],
        min_ft: Some([bldg_w / 2.0 - 20.0, parking_south + 5.0]),
        max_ft: Some([bldg_w / 2.0 - 10.0, parking_south + 23.0]),
        material: "paint".into(),
        ..SiteElement::default()
    });
    se_id += 1;

    let _ = se_id;
}

// --- Utility structures ---

fn design_utility_structures(t: &mut Truth, bldg_w: f64, bldg_d: f64, setback: f64) {
    let mut ss_id = next_ss_id(t);
    let base_elev = 100.0;

    // Storm manholes along the storm line.
    for (i, y) in [-setback - 45.0, -setback - 20.0, -10.0].iter().enumerate() {
        t.site_structures.push(SiteStructure {
            id: format!("ss-{ss_id}"),
            kind: "manhole".into(),
            tag: format!("STMH-{}", i + 1),
            at_ft: [bldg_w / 2.0 - 15.0, *y],
            rim_ft: Some(base_elev + 0.5),
            invert_ft: Some(base_elev - 5.5),
            system: "storm".into(),
            size_in: Some(48.0),
        });
        ss_id += 1;
    }

    // Catch basins at parking lot corners.
    for (i, (x, y)) in [
        (-setback + 2.0, -setback - 58.0),
        (bldg_w + setback - 2.0, -setback - 58.0),
        (-setback + 2.0, -setback - 2.0),
        (bldg_w + setback - 2.0, -setback - 2.0),
    ]
    .iter()
    .enumerate()
    {
        t.site_structures.push(SiteStructure {
            id: format!("ss-{ss_id}"),
            kind: "catch_basin".into(),
            tag: format!("CB-{}", i + 1),
            at_ft: [*x, *y],
            rim_ft: Some(base_elev),
            invert_ft: Some(base_elev - 4.0),
            system: "storm".into(),
            size_in: Some(24.0),
        });
        ss_id += 1;
    }

    // Sanitary cleanout near building.
    t.site_structures.push(SiteStructure {
        id: format!("ss-{ss_id}"),
        kind: "cleanout".into(),
        tag: "CO-1".into(),
        at_ft: [bldg_w / 2.0 - 10.0, -3.0],
        rim_ft: Some(base_elev + 1.0),
        invert_ft: Some(base_elev - 7.5),
        system: "sanitary".into(),
        size_in: Some(6.0),
    });
    ss_id += 1;

    // Sanitary manhole at property line.
    t.site_structures.push(SiteStructure {
        id: format!("ss-{ss_id}"),
        kind: "manhole".into(),
        tag: "SSMH-1".into(),
        at_ft: [bldg_w / 2.0 - 10.0, -setback - 55.0],
        rim_ft: Some(base_elev),
        invert_ft: Some(base_elev - 8.0),
        system: "sanitary".into(),
        size_in: Some(48.0),
    });
    ss_id += 1;

    // Water meter.
    t.site_structures.push(SiteStructure {
        id: format!("ss-{ss_id}"),
        kind: "meter".into(),
        tag: "WM-1".into(),
        at_ft: [bldg_w / 2.0 - 5.0, -setback - 55.0],
        rim_ft: None,
        invert_ft: None,
        system: "water".into(),
        size_in: Some(2.0),
    });
    ss_id += 1;

    // Backflow preventer.
    t.site_structures.push(SiteStructure {
        id: format!("ss-{ss_id}"),
        kind: "backflow_preventer".into(),
        tag: "BFP-1".into(),
        at_ft: [bldg_w / 2.0 - 5.0, -setback - 50.0],
        rim_ft: None,
        invert_ft: None,
        system: "water".into(),
        size_in: Some(2.0),
    });
    ss_id += 1;

    // Valve box on water main.
    t.site_structures.push(SiteStructure {
        id: format!("ss-{ss_id}"),
        kind: "valve_box".into(),
        tag: "VB-1".into(),
        at_ft: [bldg_w / 2.0 - 5.0, -setback - 30.0],
        rim_ft: Some(base_elev + 0.3),
        invert_ft: Some(base_elev - 3.5),
        system: "water".into(),
        size_in: Some(6.0),
    });
    ss_id += 1;

    // Electric junction box.
    t.site_structures.push(SiteStructure {
        id: format!("ss-{ss_id}"),
        kind: "junction_box".into(),
        tag: "JB-1".into(),
        at_ft: [bldg_w / 2.0 + 5.0, -setback - 40.0],
        rim_ft: Some(base_elev + 0.2),
        invert_ft: Some(base_elev - 2.5),
        system: "electric".into(),
        size_in: Some(36.0),
    });
    ss_id += 1;

    let _ = (ss_id, bldg_d);
}

// --- Retaining walls ---

fn design_retaining_walls(t: &mut Truth, bldg_w: f64, bldg_d: f64, setback: f64, r: &mut Rng) {
    let mut se_id = next_se_id(t);
    let wall_height = 2.0 + r.range(0, 3) as f64; // 2-4 ft

    // Retaining wall along east property line (grade change).
    let len = bldg_d + 2.0 * setback;
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "retaining_wall".into(),
        tag: "RW-1".into(),
        at_ft: [bldg_w + setback + 5.0, bldg_d / 2.0],
        min_ft: Some([bldg_w + setback + 5.0, -setback]),
        max_ft: Some([bldg_w + setback + 6.0, bldg_d + setback]),
        material: "concrete".into(),
        height_ft: Some(wall_height),
        length_ft: Some(len),
        ..SiteElement::default()
    });
    se_id += 1;
    let _ = se_id;
}

// --- Landscaping ---

fn design_landscaping(t: &mut Truth, bldg_w: f64, bldg_d: f64, setback: f64, r: &mut Rng) {
    let mut se_id = next_se_id(t);

    // Trees along north face.
    let n_trees = 4 + r.range(0, 3);
    for i in 0..n_trees {
        let x = -setback + 10.0 + i as f64 * (bldg_w + 2.0 * setback - 20.0) / (n_trees - 1).max(1) as f64;
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "tree".into(),
            tag: format!("T-{}", i + 1),
            at_ft: [x, bldg_d + setback + 5.0],
            ..SiteElement::default()
        });
        se_id += 1;
    }

    // Shrubs along building entrance.
    for i in 0..6 {
        let x = bldg_w / 2.0 - 8.0 + i as f64 * 3.0;
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "shrub".into(),
            tag: format!("SH-{}", i + 1),
            at_ft: [x, -2.0],
            ..SiteElement::default()
        });
        se_id += 1;
    }

    // Planters at entrance.
    for (i, x_off) in [-6.0, 6.0].iter().enumerate() {
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "planter".into(),
            tag: format!("PL-{}", i + 1),
            at_ft: [bldg_w / 2.0 + x_off, -setback + 2.0],
            min_ft: Some([bldg_w / 2.0 + x_off - 2.0, -setback]),
            max_ft: Some([bldg_w / 2.0 + x_off + 2.0, -setback + 4.0]),
            material: "concrete".into(),
            area_sf: Some(16.0),
            ..SiteElement::default()
        });
        se_id += 1;
    }

    // Irrigation heads in landscape zones.
    for i in 0..8 {
        let x = -setback + 5.0 + i as f64 * (bldg_w + 2.0 * setback - 10.0) / 7.0;
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "irrigation_head".into(),
            tag: format!("IRR-{}", i + 1),
            at_ft: [x, bldg_d + setback + 8.0],
            ..SiteElement::default()
        });
        se_id += 1;
    }

    let _ = se_id;
}

// --- Fencing ---

fn design_fencing(t: &mut Truth, bldg_w: f64, bldg_d: f64, setback: f64, parking_south: f64) {
    let mut se_id = next_se_id(t);

    // Property line fence.
    let segments = [
        ([-setback - 10.0, parking_south - 10.0], [bldg_w + setback + 10.0, parking_south - 10.0], "FENCE-S"),
        ([bldg_w + setback + 10.0, parking_south - 10.0], [bldg_w + setback + 10.0, bldg_d + setback + 10.0], "FENCE-E"),
        ([-setback - 10.0, parking_south - 10.0], [-setback - 10.0, bldg_d + setback + 10.0], "FENCE-W"),
        ([-setback - 10.0, bldg_d + setback + 10.0], [bldg_w + setback + 10.0, bldg_d + setback + 10.0], "FENCE-N"),
    ];
    for (a, b, tag) in segments {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "fence".into(),
            tag: tag.into(),
            at_ft: [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
            min_ft: Some([a[0].min(b[0]), a[1].min(b[1])]),
            max_ft: Some([a[0].max(b[0]), a[1].max(b[1])]),
            material: "chain_link".into(),
            height_ft: Some(6.0),
            length_ft: Some(len),
            ..SiteElement::default()
        });
        se_id += 1;
    }

    // Gate at entry.
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "gate".into(),
        tag: "GATE-1".into(),
        at_ft: [bldg_w / 2.0, parking_south - 10.0],
        min_ft: Some([bldg_w / 2.0 - 12.0, parking_south - 10.5]),
        max_ft: Some([bldg_w / 2.0 + 12.0, parking_south - 9.5]),
        material: "chain_link".into(),
        height_ft: Some(6.0),
        length_ft: Some(24.0),
        ..SiteElement::default()
    });
    se_id += 1;

    let _ = se_id;
}

// --- Furnishings ---

fn design_furnishings(t: &mut Truth, bldg_w: f64, setback: f64, parking_south: f64, r: &mut Rng) {
    let mut se_id = next_se_id(t);

    // Benches at entrance.
    for (i, x_off) in [-10.0, 10.0].iter().enumerate() {
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "bench".into(),
            tag: format!("BN-{}", i + 1),
            at_ft: [bldg_w / 2.0 + x_off, -setback + 5.0],
            ..SiteElement::default()
        });
        se_id += 1;
    }

    // Bike rack.
    t.site_elements.push(SiteElement {
        id: format!("se-{se_id}"),
        kind: "bike_rack".into(),
        tag: "BR-1".into(),
        at_ft: [bldg_w / 2.0 + 20.0, -setback + 3.0],
        min_ft: Some([bldg_w / 2.0 + 18.0, -setback + 1.0]),
        max_ft: Some([bldg_w / 2.0 + 22.0, -setback + 5.0]),
        material: "steel".into(),
        ..SiteElement::default()
    });
    se_id += 1;

    // Trash cans.
    let n_trash = 2 + r.range(0, 2);
    for i in 0..n_trash {
        let x = -setback + 15.0 + i as f64 * (bldg_w + 2.0 * setback - 30.0) / (n_trash - 1).max(1) as f64;
        t.site_elements.push(SiteElement {
            id: format!("se-{se_id}"),
            kind: "trash_can".into(),
            tag: format!("TC-{}", i + 1),
            at_ft: [x, parking_south + 3.0],
            ..SiteElement::default()
        });
        se_id += 1;
    }

    let _ = se_id;
}

// --- Utility piping ---

/// Size and material of the site utility runs (the underground runs from the site plan): the
/// pipe a civil utility plan writes beside each run, and its drawn length.
fn design_utility_piping(t: &mut Truth) {
    for se in t.site_elements.iter_mut() {
        let (size, material) = match se.kind.as_str() {
            "underground_storm" => (12.0, "RCP"),
            "underground_sewer" => (6.0, "PVC"),
            "underground_water" => (4.0, "DIP"),
            _ => continue,
        };
        se.size_in = Some(size);
        se.material = material.into();
        if let (Some(a), Some(b)) = (se.min_ft, se.max_ft) {
            se.length_ft = Some(((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt());
        }
    }
}

// --- Quantity takeoff ---

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// A takeoff line whose quantity is the sum of its elements' shares.
fn line(csi: &str, description: &str, unit: &str, status: &str, sources: &[&str], elements: Vec<SiteQuantityPart>) -> SiteQuantity {
    SiteQuantity {
        id: String::new(),
        csi: csi.into(),
        description: description.into(),
        // `+ 0.0`: an empty sum is -0.0.
        quantity: round2(elements.iter().map(|e| e.quantity).sum::<f64>() + 0.0),
        unit: unit.into(),
        sources: sources.iter().map(|s| s.to_string()).collect(),
        status: status.into(),
        elements,
    }
}

fn parts<'a>(it: impl Iterator<Item = &'a SiteElement>, share: impl Fn(&SiteElement) -> f64) -> Vec<SiteQuantityPart> {
    it.map(|se| SiteQuantityPart { id: se.id.clone(), quantity: round2(share(se)) }).collect()
}

/// The site takeoff from the design. Earthwork is the cut and fill between the existing and the
/// proposed surfaces built from the grading contours (the same computation the `site.takeoff`
/// command runs on the read C-102); a missing surface gives an `insufficient data` line, never a
/// number. Every line names its sheets, its status and the elements whose shares make it up.
pub fn compute_quantities(t: &mut Truth) {
    use crate::earthwork::{self as st, Contour, Surface};
    let mut qs = Vec::new();

    // Earthwork: cut and fill between the surfaces.
    let contours: Vec<Contour> = t
        .grading_contours
        .iter()
        .filter_map(|c| {
            let surface = match c.surface.as_str() {
                "existing" => Surface::Existing,
                "proposed" => Surface::Proposed,
                _ => return None,
            };
            Some(Contour { id: c.id.clone(), sheet: "C-102".into(), surface, elevation_ft: c.elevation_ft, points_ft: c.points.clone() })
        })
        .collect();
    let to_parts = |ps: Vec<st::Part>| ps.into_iter().map(|p| SiteQuantityPart { id: p.id, quantity: p.quantity }).collect::<Vec<_>>();
    match st::earthwork(&contours, &[]) {
        Ok(e) => {
            qs.push(line("31 22 00", "EARTHWORK, CUT (EXCAVATION)", "CY", "read", &["C-102"], to_parts(e.cut_parts)));
            qs.push(line("31 22 00", "EARTHWORK, FILL (COMPACTED)", "CY", "read", &["C-102"], to_parts(e.fill_parts)));
        }
        Err(why) => qs.push(SiteQuantity {
            csi: "31 22 00".into(),
            description: format!("EARTHWORK, CUT AND FILL: {why}"),
            unit: "CY".into(),
            sources: vec!["C-102".into()],
            status: "insufficient data".into(),
            ..SiteQuantity::default()
        }),
    }
    let of = |kinds: &[&str]| -> Vec<&SiteElement> { t.site_elements.iter().filter(|se| kinds.contains(&se.kind.as_str())).collect() };

    // Paving.
    let asphalt = of(&["paving_asphalt"]);
    if !asphalt.is_empty() {
        let p = parts(asphalt.iter().copied(), |se| se.area_sf.unwrap_or(0.0) / 9.0);
        qs.push(line("32 11 23", "AGGREGATE BASE COURSE (UNDER ASPHALT)", "SY", "rule", &["C-101"], p.clone()));
        qs.push(line("32 12 16", "ASPHALT PAVING", "SY", "read", &["C-101"], p));
    }
    let concrete = of(&["paving_concrete", "sidewalk"]);
    if !concrete.is_empty() {
        qs.push(line("32 13 13", "CONCRETE PAVING AND WALKS", "SF", "read", &["C-101"], parts(concrete.into_iter(), |se| se.area_sf.unwrap_or(0.0))));
    }

    // Linear and counted site work.
    for (kind, csi, desc, unit, sheets) in [
        ("curb", "32 16 13", "CONCRETE CURB AND GUTTER", "LF", &["C-101"][..]),
        ("striping", "32 17 23", "PARKING LOT STRIPING", "LF", &["C-101"][..]),
        ("fence", "32 31 13", "FENCE", "LF", &["C-101"][..]),
        ("gate", "32 31 13", "FENCE GATE", "EA", &["C-101"][..]),
        ("retaining_wall", "32 32 23", "CONCRETE RETAINING WALL", "SF", &["C-101", "C-102"][..]),
        ("tree", "32 93 00", "SHADE TREE, 3 IN. CALIPER", "EA", &["C-101"][..]),
        ("shrub", "32 93 00", "SHRUB, 5 GAL", "EA", &["C-101"][..]),
        ("irrigation_head", "32 84 00", "IRRIGATION SPRAY HEAD", "EA", &["C-101"][..]),
        ("bench", "12 93 13", "SITE BENCH", "EA", &["C-101"][..]),
        ("bike_rack", "12 93 13", "BIKE RACK", "EA", &["C-101"][..]),
        ("trash_can", "12 93 13", "TRASH RECEPTACLE", "EA", &["C-101"][..]),
    ] {
        let es = of(&[kind]);
        // Striping without a drawn length (the accessible-space hatch) isn't linear.
        let es: Vec<&SiteElement> = es.into_iter().filter(|se| unit != "LF" || se.length_ft.is_some()).collect();
        if es.is_empty() {
            continue;
        }
        let share = |se: &SiteElement| match (unit, kind) {
            ("EA", _) => 1.0,
            ("SF", _) => se.length_ft.unwrap_or(0.0) * se.height_ft.unwrap_or(0.0),
            _ => se.length_ft.unwrap_or(0.0),
        };
        qs.push(line(csi, desc, unit, "read", sheets, parts(es.into_iter(), share)));
    }

    // Utility piping by system, size and material, then its trench.
    let mut systems: Vec<(&str, &str, &str, &str)> = Vec::new();
    for (kind, system, csi, name) in [
        ("underground_storm", "storm", "33 41 00", "STORM"),
        ("underground_sewer", "sanitary", "33 31 00", "SANITARY SEWER"),
        ("underground_water", "water", "33 11 00", "WATER"),
    ] {
        if of(&[kind]).iter().any(|se| se.length_ft.is_some()) {
            systems.push((kind, system, csi, name));
        }
    }
    for &(kind, _, csi, name) in &systems {
        let mut groups: Vec<((Option<u64>, String), Vec<&SiteElement>)> = Vec::new();
        for se in of(&[kind]).into_iter().filter(|se| se.length_ft.is_some()) {
            let key = (se.size_in.map(f64::to_bits), se.material.to_ascii_uppercase());
            match groups.iter_mut().find(|g| g.0 == key) {
                Some(g) => g.1.push(se),
                None => groups.push((key, vec![se])),
            }
        }
        for ((size, material), es) in groups {
            let size = size.map(f64::from_bits);
            let desc = format!(
                "{name} PIPE, {}{}",
                size.map_or("SIZE NOT SHOWN".to_string(), |s| format!("{} IN.", trim_num(s))),
                if material.is_empty() { String::new() } else { format!(" {material}") }
            );
            qs.push(line(csi, &desc, "LF", "read", &["C-101", "C-103"], parts(es.into_iter(), |se| se.length_ft.unwrap_or(0.0))));
        }
    }
    for &(kind, _, _, name) in &systems {
        let es: Vec<&SiteElement> =
            of(&[kind]).into_iter().filter(|se| se.length_ft.is_some() && se.size_in.is_some() && se.depth_ft.is_some()).collect();
        if es.is_empty() {
            continue;
        }
        let tr = |se: &SiteElement| st::trench_cy(se.length_ft.unwrap_or(0.0), se.size_in.unwrap_or(0.0), se.depth_ft.unwrap_or(0.0));
        qs.push(line(
            "31 23 16",
            &format!("TRENCH EXCAVATION, {name} PIPE"),
            "CY",
            "rule",
            &["C-101", "C-103"],
            parts(es.iter().copied(), |se| tr(se).0),
        ));
        qs.push(line("31 23 23", &format!("TRENCH BACKFILL, {name} PIPE"), "CY", "rule", &["C-101", "C-103"], parts(es.into_iter(), |se| tr(se).1)));
    }

    // Utility structures: storm and sanitary manholes on their own lines.
    let mut by_line: Vec<((&str, String), Vec<SiteQuantityPart>)> = Vec::new();
    for s in &t.site_structures {
        let Some((csi, desc)) = st::structure_line(&s.kind, &s.system) else { continue };
        let part = SiteQuantityPart { id: s.id.clone(), quantity: 1.0 };
        match by_line.iter_mut().find(|l| l.0.0 == csi && l.0.1 == desc) {
            Some(l) => l.1.push(part),
            None => by_line.push(((csi, desc), vec![part])),
        }
    }
    for ((csi, desc), ps) in by_line {
        qs.push(line(csi, &desc, "EA", "read", &["C-103"], ps));
    }

    for (i, q) in qs.iter_mut().enumerate() {
        q.id = format!("sq-{i}");
    }
    t.site_quantities = qs;
}

fn trim_num(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
