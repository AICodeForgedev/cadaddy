//! Site element reader: recognises site elements (parking spaces, light poles, transformer pads,
//! fire hydrants, dumpster enclosures, underground utilities, driveways, bollards, signs) drawn
//! on civil/site sheets.
//!
//! Detection is layer- and geometry-driven: elements on `C-SITE` are above-grade, elements on
//! `C-UNDR` are underground utilities. Shape classification uses the same conventions as
//! [`crate::draw`]: circles with crosses = light poles, circles with triangles = fire hydrants,
//! rectangles with diagonal crosses = transformer pads, and so on.

use crate::layout::{SITE_LAYER, UNDERGROUND_LAYER};
use crate::score::building_from_model;
use crate::truth::Sheet;
use cadaddy_doc::{Drawing, EntityKind};

/// A site element found by the reader.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadSiteElement {
    /// Kind: `parking_space`, `light_pole`, `transformer_pad`, `fire_hydrant`,
    /// `dumpster_enclosure`, `driveway`, `underground_*`, `bollard`, `sign`, or `unknown`.
    pub kind: String,
    /// Tag text found near the element.
    pub tag: String,
    /// Position in building feet.
    pub at_ft: [f64; 2],
    /// For area elements: bounding box in building feet.
    pub min_ft: Option<[f64; 2]>,
    pub max_ft: Option<[f64; 2]>,
    /// For underground: depth below grade, feet (parsed from nearby text).
    pub depth_ft: Option<f64>,
}

/// Read site elements from a single DXF drawing (a C-sheet).
pub fn read_sites(d: &Drawing, sh: &Sheet) -> Vec<ReadSiteElement> {
    let mut result = Vec::new();

    // Collect geometry by layer.
    let mut site_circles: Vec<([f64; 2], f64)> = Vec::new();
    let mut site_rects: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut site_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut site_texts: Vec<(String, [f64; 2])> = Vec::new();
    let mut undr_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut undr_texts: Vec<(String, [f64; 2])> = Vec::new();

    for e in d.model.iter() {
        let layer = e.common.layer.as_str();
        let is_site = layer.eq_ignore_ascii_case(SITE_LAYER);
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
                    undr_lines.push((a, b));
                }
            }
            EntityKind::LwPolyline(p) => {
                if p.closed && p.vertices.len() == 4 {
                    // Closed 4-vertex polyline = rectangle.
                    let xs: Vec<f64> = p.vertices.iter().map(|v| v.p.x).collect();
                    let ys: Vec<f64> = p.vertices.iter().map(|v| v.p.y).collect();
                    let x_min = xs.iter().copied().fold(f64::INFINITY, f64::min);
                    let y_min = ys.iter().copied().fold(f64::INFINITY, f64::min);
                    let x_max = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    let y_max = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    if is_site {
                        site_rects.push(([x_min, y_min], [x_max, y_max]));
                    }
                } else if !p.closed && is_site {
                    // Open polyline: treat as connected lines.
                    for i in 0..p.vertices.len().saturating_sub(1) {
                        if let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get(i + 1)) {
                            site_lines.push(([a.p.x, a.p.y], [b.p.x, b.p.y]));
                        }
                    }
                } else if p.closed && is_site {
                    // Closed polyline with != 4 vertices (triangle, etc.): record as lines.
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
        // Check for triangle inside (fire hydrant): three connected short lines inside circle.
        let has_triangle = has_triangle_inside(&site_lines, center, radius);

        let nearby_tag = find_nearest_text(&site_texts, center, radius * 6.0);

        let (kind, tag) = if has_cross {
            ("light_pole".to_string(), nearby_tag.unwrap_or_default())
        } else if has_triangle {
            ("fire_hydrant".to_string(), nearby_tag.unwrap_or_else(|| "FH".to_string()))
        } else if radius < site_circles.iter().map(|(_, r)| *r).fold(f64::INFINITY, f64::min) * 1.5 {
            // Very small circle = bollard.
            ("bollard".to_string(), nearby_tag.unwrap_or_default())
        } else {
            ("unknown".to_string(), nearby_tag.unwrap_or_default())
        };

        result.push(ReadSiteElement { kind, tag, at_ft: at, min_ft: None, max_ft: None, depth_ft: None });
    }

    // 2. Classify rectangles on C-SITE layer.
    // First, find the building footprint rectangle (largest) and exclude it.
    let mut rects_sorted: Vec<_> = site_rects.clone();
    rects_sorted.sort_by(|a, b| {
        let area_a = (a.1[0] - a.0[0]) * (a.1[1] - a.0[1]);
        let area_b = (b.1[0] - b.0[0]) * (b.1[1] - b.0[1]);
        area_b.partial_cmp(&area_a).unwrap_or(std::cmp::Ordering::Equal)
    });

    // The building footprint is the largest rectangle with "BUILDING" text inside.
    let building_rect = rects_sorted.first().and_then(|r| {
        let cx = (r.0[0] + r.1[0]) / 2.0;
        let cy = (r.0[1] + r.1[1]) / 2.0;
        let has_bldg_text = site_texts.iter().any(|(t, pt)| {
            t.to_uppercase().contains("BUILDING") && (pt[0] - cx).abs() < (r.1[0] - r.0[0]) / 2.0 && (pt[1] - cy).abs() < (r.1[1] - r.0[1]) / 2.0
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

        let nearby_tag = find_text_inside(&site_texts, mn, mx).or_else(|| find_nearest_text(&site_texts, [cx, cy], (w + h) / 2.0));

        // Check for diagonal cross (transformer pad).
        let has_diag = has_diagonal_cross(&site_lines, mn, mx);

        let (kind, tag) = if has_diag {
            ("transformer_pad".to_string(), nearby_tag.unwrap_or_else(|| "XFMR-PAD".to_string()))
        } else if let Some(ref t) = nearby_tag {
            let upper = t.to_uppercase();
            if upper.contains("DUMPSTER") {
                ("dumpster_enclosure".to_string(), t.clone())
            } else if upper.contains("ENTRY") || upper.contains("DRIVE") {
                ("driveway".to_string(), t.clone())
            } else if upper.starts_with('P') && upper.len() <= 4 {
                // Short tag starting with P = parking space (P1, P2, ...).
                ("parking_space".to_string(), t.clone())
            } else {
                classify_rect_by_size(w, h, t.clone())
            }
        } else {
            classify_rect_by_size(w, h, String::new())
        };

        result.push(ReadSiteElement { kind, tag, at_ft: at, min_ft: Some(min_ft), max_ft: Some(max_ft), depth_ft: None });
    }

    // 3. Classify small rectangles on C-SITE that aren't polylines but two-line rects (signs).
    // Already handled above via LwPolyline rects.

    // 4. Underground utilities on C-UNDR layer.
    // Group lines and their nearby texts.
    let mut used_lines = vec![false; undr_lines.len()];
    for (i, &(a, b)) in undr_lines.iter().enumerate() {
        if used_lines[i] {
            continue;
        }
        // Check if this is a main run line (longer than tick marks).
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        // Tick marks are short; run lines are longer.
        let avg_len: f64 =
            undr_lines.iter().map(|(a, b)| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()).sum::<f64>() / undr_lines.len().max(1) as f64;
        if len < avg_len * 0.3 {
            continue; // Skip tick marks.
        }
        used_lines[i] = true;

        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let at = building_from_model(sh, mid);
        let a_ft = building_from_model(sh, a);
        let b_ft = building_from_model(sh, b);
        let min_ft = [a_ft[0].min(b_ft[0]), a_ft[1].min(b_ft[1])];
        let max_ft = [a_ft[0].max(b_ft[0]), a_ft[1].max(b_ft[1])];

        // Find nearby text for utility type and depth.
        let search_r = len * 0.5;
        let nearby = find_nearest_text(&undr_texts, mid, search_r);
        let depth = find_depth_text(&undr_texts, mid, search_r);

        let tag = nearby.clone().unwrap_or_default();
        let kind = classify_underground(&tag);

        result.push(ReadSiteElement { kind, tag, at_ft: at, min_ft: Some(min_ft), max_ft: Some(max_ft), depth_ft: depth });
    }

    result
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

/// Check if there's a triangle (3 connected short lines) inside the circle.
fn has_triangle_inside(lines: &[([f64; 2], [f64; 2])], center: [f64; 2], radius: f64) -> bool {
    let inside = |p: [f64; 2]| ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2)).sqrt() < radius * 1.2;
    let tri_lines = lines
        .iter()
        .filter(|&&(a, b)| {
            inside(a) && inside(b) && {
                let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                len > radius * 0.2 && len < radius * 2.0
            }
        })
        .count();
    // A triangle = 3 lines forming a closed shape inside the circle.
    // But we also have the circle + triangle, so look for exactly 3 interior lines.
    tri_lines >= 3
}

/// Check if there are diagonal cross lines inside a rectangle.
fn has_diagonal_cross(lines: &[([f64; 2], [f64; 2])], mn: [f64; 2], mx: [f64; 2]) -> bool {
    let w = mx[0] - mn[0];
    let h = mx[1] - mn[1];
    let tol = (w + h) * 0.15;
    let mut diag1 = false;
    let mut diag2 = false;
    for &(a, b) in lines {
        // Diagonal 1: bottom-left to top-right.
        if (a[0] - mn[0]).abs() < tol && (a[1] - mn[1]).abs() < tol && (b[0] - mx[0]).abs() < tol && (b[1] - mx[1]).abs() < tol {
            diag1 = true;
        }
        if (b[0] - mn[0]).abs() < tol && (b[1] - mn[1]).abs() < tol && (a[0] - mx[0]).abs() < tol && (a[1] - mx[1]).abs() < tol {
            diag1 = true;
        }
        // Diagonal 2: top-left to bottom-right.
        if (a[0] - mn[0]).abs() < tol && (a[1] - mx[1]).abs() < tol && (b[0] - mx[0]).abs() < tol && (b[1] - mn[1]).abs() < tol {
            diag2 = true;
        }
        if (b[0] - mn[0]).abs() < tol && (b[1] - mx[1]).abs() < tol && (a[0] - mx[0]).abs() < tol && (a[1] - mn[1]).abs() < tol {
            diag2 = true;
        }
    }
    diag1 && diag2
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

/// Find text inside a rectangle.
fn find_text_inside(texts: &[(String, [f64; 2])], mn: [f64; 2], mx: [f64; 2]) -> Option<String> {
    for (text, pt) in texts {
        if pt[0] >= mn[0] && pt[0] <= mx[0] && pt[1] >= mn[1] && pt[1] <= mx[1] {
            return Some(text.clone());
        }
    }
    None
}

/// Parse a depth value from nearby text (e.g., "3' DEEP" or "3 DEEP").
fn find_depth_text(texts: &[(String, [f64; 2])], center: [f64; 2], radius: f64) -> Option<f64> {
    for (text, pt) in texts {
        let dist = ((pt[0] - center[0]).powi(2) + (pt[1] - center[1]).powi(2)).sqrt();
        if dist < radius {
            let upper = text.to_uppercase();
            if upper.contains("DEEP") {
                // Parse "N' DEEP" or "N DEEP".
                let num_str: String = upper.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '\'').collect();
                let num_str = num_str.trim_end_matches('\'');
                if let Ok(v) = num_str.parse::<f64>() {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Classify an underground utility by its tag text.
fn classify_underground(tag: &str) -> String {
    let upper = tag.to_uppercase();
    if upper.contains("WATER") || upper == "CW" || upper == "WATER" {
        "underground_water".into()
    } else if upper.contains("SEWER") || upper == "SAN" || upper == "SEWER" {
        "underground_sewer".into()
    } else if upper.contains("ELECTRIC") || upper == "ELECTRIC" {
        "underground_electric".into()
    } else if upper.contains("GAS") || upper == "GAS" {
        "underground_gas".into()
    } else if upper.contains("STORM") || upper == "STORM" {
        "underground_storm".into()
    } else if upper.contains("TELECOM") || upper == "TELECOM" {
        "underground_telecom".into()
    } else {
        format!("underground_{}", upper.to_lowercase())
    }
}

/// Classify a rectangle by its size when the text doesn't give a clear signal.
fn classify_rect_by_size(w: f64, h: f64, tag: String) -> (String, String) {
    let area = w * h;
    // Very small rectangles are signs.
    if area < 100.0 {
        return ("sign".to_string(), tag);
    }
    // Parking-space-sized rectangles (roughly 9x18 ft at scale).
    // The scale varies, but parking spaces are small thin rectangles.
    let ratio = w.max(h) / w.min(h).max(1e-9);
    if ratio > 1.5 && ratio < 3.0 {
        return ("parking_space".to_string(), tag);
    }
    ("unknown".to_string(), tag)
}
