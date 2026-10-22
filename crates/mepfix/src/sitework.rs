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
    let existing_slope = r.range(1, 4) as f64; // 1-3% slope north to south

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

// --- Quantity takeoff ---

fn compute_quantities(t: &mut Truth) {
    let mut sq_id = 0;
    let mut qs = Vec::new();

    // Earthwork: cut/fill from grading contours.
    let existing: Vec<&GradingContour> = t.grading_contours.iter().filter(|c| c.surface == "existing").collect();
    let proposed: Vec<&GradingContour> = t.grading_contours.iter().filter(|c| c.surface == "proposed").collect();
    if !existing.is_empty() && !proposed.is_empty() {
        // Simplified cut/fill estimate from elevation differences.
        let avg_existing: f64 = existing.iter().map(|c| c.elevation_ft).sum::<f64>() / existing.len() as f64;
        let avg_proposed: f64 = proposed.iter().map(|c| c.elevation_ft).sum::<f64>() / proposed.len() as f64;
        let site_area: f64 = t.site_elements.iter().filter(|se| se.kind == "building_pad").filter_map(|se| se.area_sf).sum::<f64>().max(5000.0);
        let diff = avg_proposed - avg_existing;
        let cy = (diff.abs() * site_area / 27.0).max(100.0);
        if diff > 0.0 {
            qs.push(SiteQuantity {
                id: format!("sq-{sq_id}"),
                csi: "31 22 00".into(),
                description: "FILL (COMPACTED)".into(),
                quantity: cy.round(),
                unit: "CY".into(),
            });
        } else {
            qs.push(SiteQuantity {
                id: format!("sq-{sq_id}"),
                csi: "31 22 00".into(),
                description: "CUT (EXCAVATION)".into(),
                quantity: cy.round(),
                unit: "CY".into(),
            });
        }
        sq_id += 1;
    }

    // Paving.
    let asphalt_sf: f64 = t.site_elements.iter().filter(|se| se.kind == "paving_asphalt").filter_map(|se| se.area_sf).sum();
    if asphalt_sf > 0.0 {
        // Aggregate base course: 6 inches.
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 11 23".into(),
            description: "AGGREGATE BASE COURSE, 6 IN.".into(),
            quantity: (asphalt_sf / 9.0).round(), // SY
            unit: "SY".into(),
        });
        sq_id += 1;
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 12 16".into(),
            description: "ASPHALT PAVING, 3 IN. HOT MIX".into(),
            quantity: (asphalt_sf / 9.0).round(),
            unit: "SY".into(),
        });
        sq_id += 1;
    }

    let concrete_sf: f64 =
        t.site_elements.iter().filter(|se| se.kind == "paving_concrete" || se.kind == "sidewalk").filter_map(|se| se.area_sf).sum();
    if concrete_sf > 0.0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 13 13".into(),
            description: "CONCRETE PAVING, 4 IN.".into(),
            quantity: concrete_sf.round(),
            unit: "SF".into(),
        });
        sq_id += 1;
    }

    // Curbs.
    let curb_lf: f64 = t.site_elements.iter().filter(|se| se.kind == "curb").filter_map(|se| se.length_ft).sum();
    if curb_lf > 0.0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 16 13".into(),
            description: "CONCRETE CURB AND GUTTER".into(),
            quantity: curb_lf.round(),
            unit: "LF".into(),
        });
        sq_id += 1;
    }

    // Striping.
    let stripe_count = t.site_elements.iter().filter(|se| se.kind == "striping").count();
    if stripe_count > 0 {
        let stripe_lf: f64 =
            t.site_elements.iter().filter(|se| se.kind == "striping").filter_map(|se| se.length_ft).sum::<f64>().max(stripe_count as f64 * 18.0);
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 17 23".into(),
            description: "PARKING LOT STRIPING".into(),
            quantity: stripe_lf.round(),
            unit: "LF".into(),
        });
        sq_id += 1;
    }

    // Fencing.
    let fence_lf: f64 = t.site_elements.iter().filter(|se| se.kind == "fence").filter_map(|se| se.length_ft).sum();
    if fence_lf > 0.0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 31 13".into(),
            description: "CHAIN LINK FENCE, 6 FT".into(),
            quantity: fence_lf.round(),
            unit: "LF".into(),
        });
        sq_id += 1;
    }

    let gate_count = t.site_elements.iter().filter(|se| se.kind == "gate").count();
    if gate_count > 0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 31 13".into(),
            description: "CHAIN LINK GATE, 24 FT DOUBLE SWING".into(),
            quantity: gate_count as f64,
            unit: "EA".into(),
        });
        sq_id += 1;
    }

    // Retaining walls.
    let rw_sf: f64 =
        t.site_elements.iter().filter(|se| se.kind == "retaining_wall").map(|se| se.length_ft.unwrap_or(0.0) * se.height_ft.unwrap_or(0.0)).sum();
    if rw_sf > 0.0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 32 23".into(),
            description: "CONCRETE RETAINING WALL".into(),
            quantity: rw_sf.round(),
            unit: "SF".into(),
        });
        sq_id += 1;
    }

    // Landscaping.
    let tree_count = t.site_elements.iter().filter(|se| se.kind == "tree").count();
    if tree_count > 0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 93 00".into(),
            description: "SHADE TREE, 3 IN. CALIPER".into(),
            quantity: tree_count as f64,
            unit: "EA".into(),
        });
        sq_id += 1;
    }
    let shrub_count = t.site_elements.iter().filter(|se| se.kind == "shrub").count();
    if shrub_count > 0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 93 00".into(),
            description: "SHRUB, 5 GAL".into(),
            quantity: shrub_count as f64,
            unit: "EA".into(),
        });
        sq_id += 1;
    }

    // Irrigation.
    let irr_count = t.site_elements.iter().filter(|se| se.kind == "irrigation_head").count();
    if irr_count > 0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "32 84 00".into(),
            description: "IRRIGATION SPRAY HEAD".into(),
            quantity: irr_count as f64,
            unit: "EA".into(),
        });
        sq_id += 1;
    }

    // Site furnishings.
    let bench_count = t.site_elements.iter().filter(|se| se.kind == "bench").count();
    if bench_count > 0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "12 93 13".into(),
            description: "SITE BENCH".into(),
            quantity: bench_count as f64,
            unit: "EA".into(),
        });
        sq_id += 1;
    }
    let rack_count = t.site_elements.iter().filter(|se| se.kind == "bike_rack").count();
    if rack_count > 0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "12 93 13".into(),
            description: "BIKE RACK".into(),
            quantity: rack_count as f64,
            unit: "EA".into(),
        });
        sq_id += 1;
    }
    let tc_count = t.site_elements.iter().filter(|se| se.kind == "trash_can").count();
    if tc_count > 0 {
        qs.push(SiteQuantity {
            id: format!("sq-{sq_id}"),
            csi: "12 93 13".into(),
            description: "TRASH RECEPTACLE".into(),
            quantity: tc_count as f64,
            unit: "EA".into(),
        });
        sq_id += 1;
    }

    // Utility structures.
    for (kind, desc, csi) in [
        ("manhole", "SANITARY/STORM MANHOLE", "33 05 13"),
        ("catch_basin", "CATCH BASIN", "33 41 13"),
        ("cleanout", "CLEANOUT", "33 31 13"),
        ("valve_box", "VALVE BOX", "33 11 13"),
        ("junction_box", "ELECTRIC JUNCTION BOX", "33 71 00"),
        ("meter", "WATER METER", "33 11 13"),
        ("backflow_preventer", "BACKFLOW PREVENTER", "33 11 13"),
    ] {
        let count = t.site_structures.iter().filter(|s| s.kind == kind).count();
        if count > 0 {
            qs.push(SiteQuantity { id: format!("sq-{sq_id}"), csi: csi.into(), description: desc.into(), quantity: count as f64, unit: "EA".into() });
            sq_id += 1;
        }
    }

    let _ = sq_id;
    t.site_quantities = qs;
}
