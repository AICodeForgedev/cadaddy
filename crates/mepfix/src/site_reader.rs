//! Site element reader: recognises site elements (parking spaces, light poles, transformer pads,
//! fire hydrants, dumpster enclosures, underground utilities, driveways, pull boxes, pole bases,
//! EV chargers, monument signs, bollards, signs) drawn on civil/site sheets.
//!
//! Detection is layer- and geometry-driven: elements on `C-SITE` are above-grade, elements on
//! `C-UNDR` are underground utilities. Shape classification uses the same conventions as
//! [`crate::draw`]: circles with crosses = light poles, circles with triangles = fire hydrants,
//! rectangles with diagonal crosses = transformer pads, and so on.
//!
//! Underground parsing reuses the shared helpers from [`crate::site_services`] (conduit spec
//! parsing, route joining, depth/size extraction) so there is one C-UNDR reader.

use crate::layout::{SITE_LAYER, UNDERGROUND_LAYER};
use crate::score::building_from_model;
use crate::site_services::{
    conduits_of, depth_of, join, kind_of, length, size_of, MAX_PIECES, MAX_ROUTE, TICK_SHARE,
};
use crate::sitework::{FENCE_LAYER, FURNISH_LAYER, LANDSCAPE_LAYER};
use crate::truth::Sheet;
use cadaddy_doc::{Drawing, EntityKind};

/// A site element found by the reader.
#[derive(Clone, Debug, Default)]
pub struct ReadSiteElement {
    /// Kind: `parking_space`, `light_pole`, `transformer_pad`, `fire_hydrant`,
    /// `dumpster_enclosure`, `driveway`, `underground_*`, `pull_box`, `pole_base`,
    /// `ev_charger`, `monument_sign`, `bollard`, `sign`, or `unknown`.
    pub kind: String,
    /// Tag text found near the element.
    pub tag: String,
    /// Position in building feet.
    pub at_ft: [f64; 2],
    /// For area elements: bounding box in building feet.
    pub min_ft: Option<[f64; 2]>,
    pub max_ft: Option<[f64; 2]>,
    /// For underground: depth below grade, feet.
    pub depth_ft: Option<f64>,
    /// Sheet number where detected.
    pub sheet: String,
    /// Confidence score 0.0–1.0.
    pub confidence: f64,
    /// Pipe/conduit size in inches (underground runs).
    pub size_in: Option<f64>,
    /// Conduit type string, e.g. `"PVC SCH 40"` (underground runs).
    pub conduit_type: String,
    /// Number of conduits in a ductbank (underground runs).
    pub conduit_count: Option<u32>,
    /// Joined polyline route in building feet (underground runs).
    pub route_ft: Vec<[f64; 2]>,
}

/// Read site elements from a single DXF drawing (a C-sheet).
pub fn read_sites(d: &Drawing, sh: &Sheet) -> Vec<ReadSiteElement> {
    let mut result = Vec::new();

    // Collect geometry by layer.
    let mut site_circles: Vec<([f64; 2], f64)> = Vec::new();
    let mut site_rects: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut site_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut site_texts: Vec<(String, [f64; 2])> = Vec::new();
    let mut undr_pieces: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut undr_texts: Vec<(String, [f64; 2])> = Vec::new();

    for e in d.model.iter().take(MAX_PIECES) {
        let layer = e.common.layer.as_str();
        let is_site = layer.eq_ignore_ascii_case(SITE_LAYER)
            || layer.eq_ignore_ascii_case(FENCE_LAYER)
            || layer.eq_ignore_ascii_case(LANDSCAPE_LAYER)
            || layer.eq_ignore_ascii_case(FURNISH_LAYER);
        let is_undr = layer.eq_ignore_ascii_case(UNDERGROUND_LAYER);
        if !is_site && !is_undr {
            continue;
        }

        match &e.kind {
            EntityKind::Circle(c) => {
                if is_site {
                    site_circles.push(([c.center.x, c.center.y], c.radius));
                }
            }
            EntityKind::Line(l) => {
                let a = [l.a.x, l.a.y];
                let b = [l.b.x, l.b.y];
                if is_site {
                    site_lines.push((a, b));
                } else {
                    undr_pieces.push(vec![a, b]);
                }
            }
            EntityKind::LwPolyline(p) => {
                if is_undr && !p.closed {
                    // Open polyline on C-UNDR: part of an underground route.
                    undr_pieces.push(p.vertices.iter().take(MAX_ROUTE).map(|v| [v.p.x, v.p.y]).collect());
                } else if p.closed && p.vertices.len() == 4 && is_site {
                    // Closed 4-vertex polyline on C-SITE = rectangle.
                    // DEBUG: check what layer pole_base poly is on
                    let xs: Vec<f64> = p.vertices.iter().map(|v| v.p.x).collect();
                    let ys: Vec<f64> = p.vertices.iter().map(|v| v.p.y).collect();
                    let x_min = xs.iter().copied().fold(f64::INFINITY, f64::min);
                    let y_min = ys.iter().copied().fold(f64::INFINITY, f64::min);
                    let x_max = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    let y_max = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    site_rects.push(([x_min, y_min], [x_max, y_max]));
                } else if !p.closed && is_site {
                    // Open polyline on site: treat as connected lines.
                    for i in 0..p.vertices.len().saturating_sub(1) {
                        if let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get(i + 1)) {
                            site_lines.push(([a.p.x, a.p.y], [b.p.x, b.p.y]));
                        }
                    }
                } else if p.closed && is_site {
                    // Closed polyline with != 4 vertices: record as lines.
                    let n = p.vertices.len();
                    for i in 0..n {
                        if let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get((i + 1) % n)) {
                            site_lines.push(([a.p.x, a.p.y], [b.p.x, b.p.y]));
                        }
                    }
                }
            }
            EntityKind::Text(t) => {
                let pt = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                if is_site {
                    site_texts.push((t.value.clone(), pt));
                } else {
                    undr_texts.push((t.value.clone(), pt));
                }
            }
            EntityKind::MText(t) => {
                let pt = [t.insert.x, t.insert.y];
                if is_site {
                    site_texts.push((t.contents.clone(), pt));
                } else {
                    undr_texts.push((t.contents.clone(), pt));
                }
            }
            _ => {}
        }
    }

    // 1. Classify circles on C-SITE layer.
    for &(center, radius) in &site_circles {
        let at = building_from_model(sh, center);

        // Check for cross inside (light pole): two perpendicular lines through center.
        let has_cross = has_cross_lines(&site_lines, center, radius);
        // Check for triangle inside (fire hydrant): exactly 3 connected lines forming a triangle.
        let has_triangle = !has_cross && has_triangle_inside(&site_lines, center, radius);

        let nearby_tag = find_nearest_text(&site_texts, center, radius * 6.0);

        let (kind, tag) = if has_cross {
            let has_radials = has_radial_lines(&site_lines, center, radius);
            if has_radials {
                let tag_str = nearby_tag.clone().unwrap_or_default();
                if tag_str.to_uppercase().starts_with('T') {
                    ("tree".to_string(), tag_str)
                } else {
                    ("light_pole".to_string(), tag_str)
                }
            } else {
                ("light_pole".to_string(), nearby_tag.unwrap_or_default())
            }
        } else if has_triangle {
            let tag_str = nearby_tag.clone().unwrap_or_default();
            let upper = tag_str.to_uppercase();
            if upper.starts_with("IRR") {
                ("irrigation_head".to_string(), tag_str)
            } else {
                ("fire_hydrant".to_string(), if tag_str.is_empty() { "FH".to_string() } else { tag_str })
            }
        } else if radius < site_circles.iter().map(|(_, r)| *r).fold(f64::INFINITY, f64::min) * 1.5 {
            let tag_str = nearby_tag.clone().unwrap_or_default();
            let upper = tag_str.to_uppercase();
            if upper.starts_with("SH-") {
                ("shrub".to_string(), tag_str)
            } else if upper.starts_with("TC-") {
                ("trash_can".to_string(), tag_str)
            } else if upper.starts_with("BN-") {
                ("bench".to_string(), tag_str)
            } else {
                ("bollard".to_string(), tag_str)
            }
        } else {
            ("unknown".to_string(), nearby_tag.unwrap_or_default())
        };

        let confidence = if kind == "unknown" { 0.3 } else { 0.9 };
        result.push(ReadSiteElement {
            kind,
            tag,
            at_ft: at,
            min_ft: None,
            max_ft: None,
            depth_ft: None,
            sheet: sh.number.clone(),
            confidence,
            ..ReadSiteElement::default()
        });
    }

    // 2. Classify rectangles on C-SITE layer.
    // The building footprint is the largest rectangle with "BUILDING" text inside.
    let mut rects_sorted: Vec<_> = site_rects.clone();
    rects_sorted.sort_by(|a, b| {
        let area_a = (a.1[0] - a.0[0]) * (a.1[1] - a.0[1]);
        let area_b = (b.1[0] - b.0[0]) * (b.1[1] - b.0[1]);
        area_b.partial_cmp(&area_a).unwrap_or(std::cmp::Ordering::Equal)
    });
    let building_rect = rects_sorted.first().and_then(|r| {
        let cx = (r.0[0] + r.1[0]) / 2.0;
        let cy = (r.0[1] + r.1[1]) / 2.0;
        let has_bldg_text = site_texts.iter().any(|(t, pt)| {
            t.to_uppercase().contains("BUILDING")
                && (pt[0] - cx).abs() < (r.1[0] - r.0[0]) / 2.0
                && (pt[1] - cy).abs() < (r.1[1] - r.0[1]) / 2.0
        });
        if has_bldg_text { Some(*r) } else { None }
    });

    for &(mn, mx) in &site_rects {
        // Skip the building footprint.
        if let Some(bldg) = building_rect
            && (mn[0] - bldg.0[0]).abs() < 1.0
            && (mn[1] - bldg.0[1]).abs() < 1.0
            && (mx[0] - bldg.1[0]).abs() < 1.0
            && (mx[1] - bldg.1[1]).abs() < 1.0
        {
            continue;
        }

        let cx = (mn[0] + mx[0]) / 2.0;
        let cy = (mn[1] + mx[1]) / 2.0;
        let at = building_from_model(sh, [cx, cy]);
        let mn_ft = building_from_model(sh, mn);
        let mx_ft = building_from_model(sh, mx);
        let min_ft = [mn_ft[0].min(mx_ft[0]), mn_ft[1].min(mx_ft[1])];
        let max_ft = [mn_ft[0].max(mx_ft[0]), mn_ft[1].max(mx_ft[1])];

        let w = mx[0] - mn[0];
        let h = mx[1] - mn[1];

        // Search for tag: inside the rect first, then ALL texts in range (sorted by distance),
        // picking the first that classifies to a known kind. This handles co-located elements
        // (e.g. pole_base at the same position as a light_pole) where the nearest text may
        // belong to the other element.
        // Use a generous radius: at least the sum of w and h (accounts for text drawn
        // above/beside the rect at scaled paper offsets).
        let search_r = (w + h).max(w.max(h) * 2.0);
        let has_diag = has_diagonal_cross(&site_lines, mn, mx);

        // Try ALL texts near the rect (inside and nearby), sorted by distance, and use the
        // first that classifies to a known kind. This handles co-located elements where the
        // nearest text may belong to a different element at the same position.
        let mut nearby = find_all_texts_in_range(&site_texts, [cx, cy], search_r);
        nearby.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut best = ("unknown".to_string(), String::new());
        for (_, text) in &nearby {
            let (k, t) = if has_diag && is_xfmr_tag(text) {
                ("transformer_pad".to_string(), text.clone())
            } else {
                classify_rect_by_tag(text, w, h)
            };
            if k != "unknown" {
                best = (k, t);
                break;
            }
        }
        let (kind, tag) = if best.0 == "unknown" && has_diag {
            ("transformer_pad".to_string(), "XFMR-PAD".to_string())
        } else if best.0 == "unknown" {
            classify_rect_by_size(w, h, nearby.first().map_or(String::new(), |(_, t)| t.clone()))
        } else {
            best
        };

        let confidence = if kind == "unknown" { 0.3 } else { 0.85 };
        result.push(ReadSiteElement {
            kind,
            tag,
            at_ft: at,
            min_ft: Some(min_ft),
            max_ft: Some(max_ft),
            depth_ft: None,
            sheet: sh.number.clone(),
            confidence,
            ..ReadSiteElement::default()
        });
    }

    // 3. Underground utilities on C-UNDR layer.
    // Use the shared route-joining logic from site_services.
    let undr_pieces: Vec<Vec<[f64; 2]>> = undr_pieces
        .into_iter()
        .filter(|p| p.len() >= 2 && p.iter().all(|q| q[0].is_finite() && q[1].is_finite()))
        .collect();
    if !undr_pieces.is_empty() {
        let avg_len = undr_pieces.iter().map(|p| length(p)).sum::<f64>() / undr_pieces.len() as f64;
        let runs = join(
            undr_pieces
                .into_iter()
                .filter(|p| length(p) >= avg_len * TICK_SHARE)
                .collect(),
        );

        // Assign text labels to nearest run.
        let mut by_run: Vec<Vec<&str>> = vec![Vec::new(); runs.len()];
        for (text, at) in &undr_texts {
            let best = runs
                .iter()
                .enumerate()
                .map(|(i, r)| (i, point_to_route_dist(*at, r)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, dist)) = best
                && runs.get(i).is_some_and(|r| dist <= length(r) * 0.5)
                && let Some(v) = by_run.get_mut(i)
            {
                v.push(text.as_str());
            }
        }

        for (run, texts) in runs.iter().zip(by_run) {
            // Parse all associated text labels.
            let mut utility_kind: Option<&str> = None;
            let mut pipe_size: Option<f64> = None;
            let mut conduit_count: Option<u32> = None;
            let mut depth: Option<f64> = None;
            let mut conduit_type = String::new();
            let mut all_labels = Vec::new();

            for t in &texts {
                let words = t.to_ascii_uppercase();
                if utility_kind.is_none() {
                    utility_kind = kind_of(&words);
                }
                if pipe_size.is_none() {
                    pipe_size = size_of(&words);
                }
                if conduit_count.is_none() {
                    conduit_count = conduits_of(&words);
                }
                if depth.is_none() {
                    depth = depth_of(&words);
                }
                // Extract conduit type: look for PVC SCH 40/80 patterns.
                if conduit_type.is_empty()
                    && let Some(ct) = extract_conduit_type(&words)
                {
                    conduit_type = ct;
                }
                all_labels.push(t.trim().to_string());
            }

            // MUST 1: If no depth from label association, find the nearest depth text to the run.
            if depth.is_none() {
                depth = find_nearest_depth(&undr_texts, run);
            }

            // MUST 6: Use kind_of for classification; unrecognised = "unknown".
            let kind = match utility_kind {
                Some("water") => "underground_water",
                Some("sewer") => "underground_sewer",
                Some("storm") => "underground_storm",
                Some("gas") => "underground_gas",
                Some("power") => "underground_electric",
                Some("telephone") | Some("catv") => "underground_telecom",
                _ => "unknown",
            };

            let tag = all_labels.first().cloned().unwrap_or_default();

            // Convert route to building feet.
            let route_ft: Vec<[f64; 2]> = run.iter().map(|p| building_from_model(sh, *p)).collect();

            // Bounding box.
            let (mut mn, mut mx) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for p in &route_ft {
                mn = [mn[0].min(p[0]), mn[1].min(p[1])];
                mx = [mx[0].max(p[0]), mx[1].max(p[1])];
            }
            let mid = [(mn[0] + mx[0]) / 2.0, (mn[1] + mx[1]) / 2.0];

            let confidence = if kind == "unknown" {
                0.3
            } else {
                0.7 + [pipe_size.is_some(), depth.is_some()].iter().filter(|b| **b).count() as f64 * 0.1
            };

            result.push(ReadSiteElement {
                kind: kind.into(),
                tag,
                at_ft: mid,
                min_ft: Some(mn),
                max_ft: Some(mx),
                depth_ft: depth,
                sheet: sh.number.clone(),
                confidence,
                size_in: pipe_size,
                conduit_type,
                conduit_count,
                route_ft,
            });
        }
    }

    result
}

/// Extract conduit/pipe type from text, e.g. "PVC SCH 40", "DIP", "RCP", "HDPE".
fn extract_conduit_type(words: &str) -> Option<String> {
    let upper = words.to_uppercase();
    if upper.contains("PVC") {
        if upper.contains("SCH 80") || upper.contains("SCHEDULE 80") {
            return Some("PVC SCH 80".into());
        }
        if upper.contains("SCH 40") || upper.contains("SCHEDULE 40") {
            return Some("PVC SCH 40".into());
        }
        // Plain PVC (e.g. sewer).
        return Some("PVC".into());
    }
    // Ductile iron pipe.
    if upper.contains("DIP") || upper.contains("DUCTILE") {
        return Some("DIP".into());
    }
    // Reinforced concrete pipe.
    if upper.contains("RCP") {
        return Some("RCP".into());
    }
    if upper.contains("HDPE") {
        return Some("HDPE".into());
    }
    None
}

/// Classify a rectangle by its nearby tag text.
fn classify_rect_by_tag(t: &str, w: f64, h: f64) -> (String, String) {
    let upper = t.to_uppercase();
    if upper.contains("DUMPSTER") {
        ("dumpster_enclosure".to_string(), t.to_string())
    } else if upper.contains("BLDG PAD") || upper.contains("BUILDING PAD") {
        ("building_pad".to_string(), t.to_string())
    } else if upper.contains("ASPHALT") || upper.contains("PARKING LOT ASPHALT") {
        ("paving_asphalt".to_string(), t.to_string())
    } else if upper.contains("ENTRY WALK") || (upper.contains("CONCRETE") && upper.contains("PAV")) {
        ("paving_concrete".to_string(), t.to_string())
    } else if upper.contains("DRIVEWAY")
        || (upper.contains("ENTRY") && !upper.contains("WALK"))
        || (upper.contains("DRIVE") && !upper.contains("DRIVER") && !upper.contains("DRIVEN"))
    {
        ("driveway".to_string(), t.to_string())
    } else if upper.starts_with("CURB") {
        ("curb".to_string(), t.to_string())
    } else if upper.starts_with("SIDEWALK") {
        ("sidewalk".to_string(), t.to_string())
    } else if upper.starts_with("STR-") || upper.starts_with("HC-") {
        ("striping".to_string(), t.to_string())
    } else if upper.starts_with("FENCE") {
        ("fence".to_string(), t.to_string())
    } else if upper.starts_with("GATE") {
        ("gate".to_string(), t.to_string())
    } else if upper.starts_with("RW-") || upper.contains("RETAINING") {
        ("retaining_wall".to_string(), t.to_string())
    } else if upper.starts_with("PL-") || upper.contains("PLANTER") {
        ("planter".to_string(), t.to_string())
    } else if upper.starts_with("BR-") || upper.contains("BIKE") {
        ("bike_rack".to_string(), t.to_string())
    } else if upper.starts_with("PB-") || upper.contains("PULL BOX") || upper.contains("HANDHOLE") || upper.starts_with("HH") {
        ("pull_box".to_string(), t.to_string())
    } else if upper.starts_with("PLB-") || upper.contains("POLE BASE") {
        ("pole_base".to_string(), t.to_string())
    } else if upper.starts_with("EV") || upper.contains("CHARGER") {
        ("ev_charger".to_string(), t.to_string())
    } else if upper.contains("MONUMENT") {
        ("monument_sign".to_string(), t.to_string())
    } else if upper.starts_with('P') && upper.len() <= 4 {
        ("parking_space".to_string(), t.to_string())
    } else {
        classify_rect_by_size(w, h, t.to_string())
    }
}

/// Check if two lines form a cross through the circle center.
fn has_cross_lines(lines: &[([f64; 2], [f64; 2])], center: [f64; 2], radius: f64) -> bool {
    let tol = radius * 0.6;
    let mut vert = false;
    let mut horiz = false;
    for &(a, b) in lines {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if len < radius * 0.5 || len > radius * 3.0 {
            continue;
        }
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        if (mid[0] - center[0]).abs() > tol || (mid[1] - center[1]).abs() > tol {
            continue;
        }
        let dx = (b[0] - a[0]).abs();
        let dy = (b[1] - a[1]).abs();
        if dx > dy * 3.0 {
            horiz = true;
        }
        if dy > dx * 3.0 {
            vert = true;
        }
    }
    horiz && vert
}

/// Check if there's a triangle (3 connected lines) inside the circle.
/// Requires exactly 3 interior lines that form a closed triangle shape, verified by checking
/// that endpoints connect (each endpoint is shared with another line).
fn has_triangle_inside(lines: &[([f64; 2], [f64; 2])], center: [f64; 2], radius: f64) -> bool {
    let inside = |p: [f64; 2]| ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2)).sqrt() < radius * 1.2;
    let tri_lines: Vec<_> = lines
        .iter()
        .filter(|&&(a, b)| {
            inside(a) && inside(b) && {
                let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                len > radius * 0.3 && len < radius * 2.5
            }
        })
        .collect();
    // Require exactly 3 lines that form a connected triangle (endpoints match within tolerance).
    if tri_lines.len() != 3 {
        return false;
    }
    let eps = radius * 0.3;
    let close = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]) < eps;
    // Check connectivity: each line shares an endpoint with each of the other two.
    let pts: Vec<([f64; 2], [f64; 2])> = tri_lines.iter().map(|&&(a, b)| (a, b)).collect();
    let connected = |i: usize, j: usize| {
        close(pts[i].0, pts[j].0)
            || close(pts[i].0, pts[j].1)
            || close(pts[i].1, pts[j].0)
            || close(pts[i].1, pts[j].1)
    };
    connected(0, 1) && connected(1, 2) && connected(0, 2)
}

/// Check if there are diagonal cross lines inside a rectangle.
/// Both diagonals must be present and their lengths must approximate the rectangle's diagonal.
fn has_diagonal_cross(lines: &[([f64; 2], [f64; 2])], mn: [f64; 2], mx: [f64; 2]) -> bool {
    let w = mx[0] - mn[0];
    let h = mx[1] - mn[1];
    let diag_len = (w * w + h * h).sqrt();
    let tol = (w + h) * 0.15;
    let len_tol = diag_len * 0.3;
    let mut diag1 = false;
    let mut diag2 = false;
    for &(a, b) in lines {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        // The line must be approximately the length of the diagonal.
        if (len - diag_len).abs() > len_tol {
            continue;
        }
        // Diagonal 1: bottom-left to top-right (either direction).
        if ((a[0] - mn[0]).abs() < tol && (a[1] - mn[1]).abs() < tol && (b[0] - mx[0]).abs() < tol && (b[1] - mx[1]).abs() < tol)
            || ((b[0] - mn[0]).abs() < tol && (b[1] - mn[1]).abs() < tol && (a[0] - mx[0]).abs() < tol && (a[1] - mx[1]).abs() < tol)
        {
            diag1 = true;
        }
        // Diagonal 2: top-left to bottom-right (either direction).
        if ((a[0] - mn[0]).abs() < tol && (a[1] - mx[1]).abs() < tol && (b[0] - mx[0]).abs() < tol && (b[1] - mn[1]).abs() < tol)
            || ((b[0] - mn[0]).abs() < tol && (b[1] - mx[1]).abs() < tol && (a[0] - mx[0]).abs() < tol && (a[1] - mn[1]).abs() < tol)
        {
            diag2 = true;
        }
    }
    diag1 && diag2
}

/// Check if a tag looks like a transformer pad label.
fn is_xfmr_tag(t: &str) -> bool {
    let u = t.to_uppercase();
    (u.contains("XFMR") || u.contains("TRANSFORMER") || u.contains("PAD"))
        && !u.starts_with("PB")
        && !u.starts_with("PLB")
        && !u.starts_with("BLDG")
        && !u.starts_with("BUILDING")
}

/// Find all texts within a radius, returned with their distances.
fn find_all_texts_in_range(texts: &[(String, [f64; 2])], center: [f64; 2], radius: f64) -> Vec<(f64, String)> {
    let mut result = Vec::new();
    for (text, pt) in texts {
        let dist = ((pt[0] - center[0]).powi(2) + (pt[1] - center[1]).powi(2)).sqrt();
        if dist < radius {
            result.push((dist, text.clone()));
        }
    }
    result
}

/// Find the nearest text entity within a radius.
fn find_nearest_text(texts: &[(String, [f64; 2])], center: [f64; 2], radius: f64) -> Option<String> {
    let mut best: Option<(f64, &str)> = None;
    for (text, pt) in texts {
        let dist = ((pt[0] - center[0]).powi(2) + (pt[1] - center[1]).powi(2)).sqrt();
        if dist < radius {
            if let Some((d, _)) = best {
                if dist < d {
                    best = Some((dist, text));
                }
            } else {
                best = Some((dist, text));
            }
        }
    }
    best.map(|(_, t)| t.to_string())
}

/// MUST 1: Find the nearest depth text to a route, using the shared depth_of parser.
/// Searches all underground texts and returns the depth from the text closest to the route.
fn find_nearest_depth(texts: &[(String, [f64; 2])], route: &[[f64; 2]]) -> Option<f64> {
    let mut best: Option<(f64, f64)> = None; // (distance, depth)
    for (text, pt) in texts {
        let words = text.to_ascii_uppercase();
        if let Some(d) = depth_of(&words) {
            let dist = point_to_route_dist(*pt, route);
            if let Some((bd, _)) = best {
                if dist < bd {
                    best = Some((dist, d));
                }
            } else {
                best = Some((dist, d));
            }
        }
    }
    best.map(|(_, d)| d)
}

/// Minimum distance from a point to a polyline route.
fn point_to_route_dist(p: [f64; 2], route: &[[f64; 2]]) -> f64 {
    if route.is_empty() {
        return f64::INFINITY;
    }
    let mut min_d = f64::INFINITY;
    for w in route.windows(2) {
        let d = point_to_segment_dist(p, w[0], w[1]);
        if d < min_d {
            min_d = d;
        }
    }
    // Also check distance to endpoints for single-point routes.
    for q in route {
        let d = (p[0] - q[0]).hypot(p[1] - q[1]);
        if d < min_d {
            min_d = d;
        }
    }
    min_d
}

/// Distance from point p to line segment a–b.
fn point_to_segment_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        return (p[0] - a[0]).hypot(p[1] - a[1]);
    }
    let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let proj = [a[0] + t * dx, a[1] + t * dy];
    (p[0] - proj[0]).hypot(p[1] - proj[1])
}

/// Classify a rectangle by its size when the text doesn't give a clear signal.
/// Without a tag, only very small rectangles are classified (as signs); everything else is unknown.
fn classify_rect_by_size(_w: f64, _h: f64, tag: String) -> (String, String) {
    // Without a clear tag, don't guess — it causes false positives.
    ("unknown".to_string(), tag)
}

/// Check if there are 4+ radial lines from the center (tree crown pattern).
fn has_radial_lines(lines: &[([f64; 2], [f64; 2])], center: [f64; 2], radius: f64) -> bool {
    let tol = radius * 0.8;
    let mut count = 0;
    for &(a, b) in lines {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if len < radius * 0.2 || len > radius * 1.5 {
            continue;
        }
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        if (mid[0] - center[0]).abs() < tol && (mid[1] - center[1]).abs() < tol {
            count += 1;
        }
    }
    count >= 4
}

/// A site utility structure found by the reader.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadSiteStructure {
    pub kind: String,
    pub tag: String,
    pub at_ft: [f64; 2],
    pub rim_ft: Option<f64>,
    pub invert_ft: Option<f64>,
    pub system: String,
}

/// Read site utility structures from a drawing (the C-103 utility plan).
///
/// The symbol's geometry decides first: a circle with a cross is a manhole, a square with an X
/// a catch basin, a diamond a valve box. A plain circle, square or triangle is only named by its
/// tag (`CO` cleanout, `JB` junction box, `WM` meter, `BFP` backflow preventer); anything else
/// stays `unknown` with an empty system, never a guess.
pub fn read_site_structures(d: &Drawing, sh: &Sheet) -> Vec<ReadSiteStructure> {
    let mut result = Vec::new();

    let mut strc_circles: Vec<([f64; 2], f64)> = Vec::new();
    let mut strc_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut strc_texts: Vec<(String, [f64; 2])> = Vec::new();
    let mut strc_polys: Vec<Vec<[f64; 2]>> = Vec::new();

    for e in d.model.iter() {
        let layer = e.common.layer.as_str();
        if !layer.eq_ignore_ascii_case(crate::sitework::STRUCTURE_LAYER) {
            continue;
        }
        match &e.kind {
            EntityKind::Circle(c) => strc_circles.push(([c.center.x, c.center.y], c.radius)),
            EntityKind::Line(l) => strc_lines.push(([l.a.x, l.a.y], [l.b.x, l.b.y])),
            EntityKind::LwPolyline(p) if p.closed && p.vertices.len() >= 3 => {
                strc_polys.push(p.vertices.iter().map(|v| [v.p.x, v.p.y]).collect())
            }
            EntityKind::Text(t) => {
                let pt = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                strc_texts.push((t.value.clone(), pt));
            }
            EntityKind::MText(t) => strc_texts.push((t.contents.clone(), [t.insert.x, t.insert.y])),
            _ => {}
        }
    }
    let dist = |a: [f64; 2], b: [f64; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
    let through = |c: [f64; 2], span: f64| -> Vec<[f64; 2]> {
        strc_lines
            .iter()
            .filter(|(a, b)| {
                dist([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0], c) < span * 0.1
                    && (dist(*a, *b) - span).abs() < span * 0.2
            })
            .map(|(a, b)| [b[0] - a[0], b[1] - a[1]])
            .collect()
    };
    let crossed = |dirs: &[[f64; 2]]| {
        dirs.iter().any(|u| {
            dirs.iter()
                .any(|v| (u[0] * v[0] + u[1] * v[1]).abs() < 0.2 * (u[0].hypot(u[1]) * v[0].hypot(v[1])))
        })
    };
    let tag_system = |upper: &str| -> &'static str {
        if upper.starts_with("STMH") || upper.starts_with("SDMH") || upper.contains("STORM") {
            "storm"
        } else if upper.starts_with("SSMH") || upper.starts_with("CO") || upper.contains("SANITARY") || upper.contains("SEWER") {
            "sanitary"
        } else if upper.starts_with("WM") || upper.starts_with("BFP") || upper.starts_with("VB") || upper.contains("WATER") {
            "water"
        } else if upper.starts_with("JB") || upper.contains("ELEC") {
            "electric"
        } else {
            ""
        }
    };
    let tag_texts: Vec<(String, [f64; 2])> =
        strc_texts.iter().filter(|(t, _)| !t.starts_with("RIM") && !t.starts_with("INV")).cloned().collect();
    let mut push = |center: [f64; 2], reach: f64, shape: &str| {
        let tag_str = find_nearest_text(&tag_texts, center, reach).unwrap_or_default();
        let upper = tag_str.to_uppercase();
        let kind = match shape {
            "circle_cross" => "manhole",
            "square_x" => "catch_basin",
            "diamond" => "valve_box",
            "circle" if upper.starts_with("CO") => "cleanout",
            "square" if upper.starts_with("JB") => "junction_box",
            "triangle" if upper.starts_with("WM") => "meter",
            "triangle" if upper.starts_with("BFP") => "backflow_preventer",
            _ => "unknown",
        };
        let system = match kind {
            "catch_basin" => "storm",
            "unknown" => "",
            _ => tag_system(&upper),
        };
        let rim = parse_elevation_text(&strc_texts, center, reach, "RIM");
        let inv = parse_elevation_text(&strc_texts, center, reach, "INV");
        result.push(ReadSiteStructure {
            kind: kind.into(),
            tag: tag_str,
            at_ft: building_from_model(sh, center),
            rim_ft: rim,
            invert_ft: inv,
            system: system.into(),
        });
    };

    for &(center, radius) in &strc_circles {
        let shape = if crossed(&through(center, 2.0 * radius)) { "circle_cross" } else { "circle" };
        push(center, radius.max(1e-9) * 8.0, shape);
    }
    for pts in &strc_polys {
        let n = pts.len() as f64;
        let c = [pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n];
        let r = pts.iter().map(|p| dist(*p, c)).fold(0.0, f64::max);
        let shape = match pts.as_slice() {
            [_, _, _] => "triangle",
            [a, b, _, _] => {
                let axis = (a[0] - b[0]).abs() < r * 0.05 || (a[1] - b[1]).abs() < r * 0.05;
                if !axis {
                    "diamond"
                } else if through(c, 2.0 * r).len() >= 2 {
                    "square_x"
                } else {
                    "square"
                }
            }
            _ => "other",
        };
        push(c, r.max(1e-9) * 6.0, shape);
    }

    result
}

/// Parse a RIM or INV elevation from nearby text.
fn parse_elevation_text(texts: &[(String, [f64; 2])], center: [f64; 2], radius: f64, prefix: &str) -> Option<f64> {
    for (text, pt) in texts {
        let dist = ((pt[0] - center[0]).powi(2) + (pt[1] - center[1]).powi(2)).sqrt();
        if dist < radius {
            let upper = text.to_uppercase();
            if upper.starts_with(prefix) {
                let num_part = upper.trim_start_matches(prefix).trim();
                if let Ok(v) = num_part.parse::<f64>() {
                    return Some(v);
                }
            }
        }
    }
    None
}
