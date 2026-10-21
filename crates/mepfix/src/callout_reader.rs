//! Callout reader: recognise callout symbols (section marks, elevation tags, detail bubbles,
//! enlarged-plan boundaries, match lines and key plans) drawn on DXF sheets and link each one
//! to its target sheet/detail.
//!
//! A callout symbol, as drawn by [`crate::draw`], is a circle on the `CALLOUT` layer with a
//! horizontal divider line and two text entities: the detail/view number in the top half and
//! the target sheet number in the bottom half. Section marks additionally have dashed cut
//! lines; elevation tags have an arrow; enlarged-plan callouts have a leader to the enlarged
//! area's corner. Key plans are a rectangle on the `CALLOUT` layer with a "KEY PLAN" text.
//!
//! The reader is text- and geometry-driven: it uses the `CALLOUT` layer and the spatial
//! relationship between circles, lines and text, not block definitions.

use crate::layout::CALLOUT_LAYER;
use cadaddy_doc::{Drawing, EntityKind};

/// A callout found on a sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadCallout {
    /// The kind we could determine: `section_mark`, `elevation_tag`, `detail_bubble`,
    /// `enlarged_plan`, `key_plan`, or `unknown`.
    pub kind: String,
    /// The detail/view number (top half of the bubble).
    pub detail: String,
    /// The target sheet number (bottom half of the bubble).
    pub target_sheet: String,
    /// Centre of the callout circle in model coordinates.
    pub center: [f64; 2],
    /// Whether a dashed cut line was found (section mark indicator).
    pub has_cut_line: bool,
    /// Whether an arrow line extending from the circle was found (elevation tag indicator).
    pub has_arrow: bool,
    /// Whether a leader line to a corner was found (enlarged-plan indicator).
    pub has_leader: bool,
}

/// A resolved callout: a read callout matched against the sheet set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedCallout {
    /// The read callout.
    pub callout: ReadCallout,
    /// The sheet number it was found on.
    pub on_sheet: String,
    /// Whether the target sheet exists in the set.
    pub resolved: bool,
}

/// Read callout symbols from a single DXF drawing.
pub fn read_callouts(d: &Drawing) -> Vec<ReadCallout> {
    let mut callouts = Vec::new();

    // Step 1: Collect circles on the CALLOUT layer.
    let mut circles: Vec<([f64; 2], f64)> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case(CALLOUT_LAYER) {
            continue;
        }
        if let EntityKind::Circle(c) = &e.kind {
            circles.push(([c.center.x, c.center.y], c.radius));
        }
    }

    // Step 2: Collect text entities on the CALLOUT layer.
    let mut texts: Vec<(String, [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case(CALLOUT_LAYER) {
            continue;
        }
        match &e.kind {
            EntityKind::Text(t) => {
                let pt = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                texts.push((t.value.clone(), pt));
            }
            EntityKind::MText(t) => {
                texts.push((t.contents.clone(), [t.insert.x, t.insert.y]));
            }
            _ => {}
        }
    }

    // Step 3: Collect lines on the CALLOUT layer and the DASHED layer (for cut lines).
    let mut callout_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut dashed_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if let EntityKind::Line(l) = &e.kind {
            let a = [l.a.x, l.a.y];
            let b = [l.b.x, l.b.y];
            if e.common.layer.eq_ignore_ascii_case(CALLOUT_LAYER) {
                callout_lines.push((a, b));
            }
            if e.common.layer.eq_ignore_ascii_case("DASHED") {
                dashed_lines.push((a, b));
            }
        }
    }

    // Step 4: For each circle, find texts inside it (top half = detail, bottom half = target).
    for &(center, radius) in &circles {
        let mut top_text = String::new();
        let mut bottom_text = String::new();
        for (text, pt) in &texts {
            let dx = pt[0] - center[0];
            let dy = pt[1] - center[1];
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < radius * 1.5 {
                if dy > 0.0 {
                    if top_text.is_empty() || dy < radius * 0.7 {
                        top_text = text.clone();
                    }
                } else {
                    if bottom_text.is_empty() || dy.abs() < radius * 0.7 {
                        bottom_text = text.clone();
                    }
                }
            }
        }

        if top_text.is_empty() && bottom_text.is_empty() {
            continue;
        }

        // Determine the kind based on surrounding geometry.
        let has_cut_line = has_nearby_dashed_line(&dashed_lines, center, radius);
        let has_arrow = has_arrow_line(&callout_lines, center, radius);
        let has_leader = has_leader_line(&callout_lines, center, radius);

        let kind = if has_cut_line {
            "section_mark"
        } else if has_arrow && !has_leader {
            "elevation_tag"
        } else if has_leader {
            "enlarged_plan"
        } else {
            "detail_bubble"
        };

        callouts.push(ReadCallout {
            kind: kind.to_string(),
            detail: top_text,
            target_sheet: bottom_text,
            center,
            has_cut_line,
            has_arrow,
            has_leader,
        });
    }

    // Step 5: Detect key plans (rectangles on CALLOUT layer with "KEY PLAN" text).
    if texts.iter().any(|(t, _)| t.contains("KEY PLAN")) {
        callouts.push(ReadCallout {
            kind: "key_plan".to_string(),
            detail: String::new(),
            target_sheet: String::new(),
            center: [0.0, 0.0],
            has_cut_line: false,
            has_arrow: false,
            has_leader: false,
        });
    }

    callouts
}

/// Check if there's a dashed line near the circle (section mark indicator).
fn has_nearby_dashed_line(dashed_lines: &[([f64; 2], [f64; 2])], center: [f64; 2], radius: f64) -> bool {
    let threshold = radius * 8.0;
    for &(a, b) in dashed_lines {
        let dist_a = ((a[0] - center[0]).powi(2) + (a[1] - center[1]).powi(2)).sqrt();
        let dist_b = ((b[0] - center[0]).powi(2) + (b[1] - center[1]).powi(2)).sqrt();
        // The cut line should start or end near the circle edge
        if (dist_a - radius).abs() < radius * 0.5 || (dist_b - radius).abs() < radius * 0.5 {
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            if len > radius * 2.0 {
                return true;
            }
        }
        // Or be close to the center but long
        if dist_a < threshold && dist_b < threshold {
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            if len > radius * 4.0 {
                return true;
            }
        }
    }
    false
}

/// Check if there's an arrow line extending from the circle edge (elevation tag indicator).
fn has_arrow_line(callout_lines: &[([f64; 2], [f64; 2])], center: [f64; 2], radius: f64) -> bool {
    for &(a, b) in callout_lines {
        let dist_a = ((a[0] - center[0]).powi(2) + (a[1] - center[1]).powi(2)).sqrt();
        let dist_b = ((b[0] - center[0]).powi(2) + (b[1] - center[1]).powi(2)).sqrt();
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        // An arrow starts near the edge and extends a short distance outward
        let near_edge = |d: f64| (d - radius).abs() < radius * 0.3;
        let beyond_edge = |d: f64| d > radius * 1.1;
        if near_edge(dist_a) && beyond_edge(dist_b) && len < radius * 2.0 && len > radius * 0.3 {
            return true;
        }
        if near_edge(dist_b) && beyond_edge(dist_a) && len < radius * 2.0 && len > radius * 0.3 {
            return true;
        }
    }
    false
}

/// Check if there's a leader line (enlarged-plan callout indicator).
fn has_leader_line(callout_lines: &[([f64; 2], [f64; 2])], center: [f64; 2], radius: f64) -> bool {
    for &(a, b) in callout_lines {
        let dist_a = ((a[0] - center[0]).powi(2) + (a[1] - center[1]).powi(2)).sqrt();
        let dist_b = ((b[0] - center[0]).powi(2) + (b[1] - center[1]).powi(2)).sqrt();
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        // A leader starts near the edge and extends farther out than an arrow
        let near_edge = |d: f64| (d - radius).abs() < radius * 0.5;
        let far_away = |d: f64| d > radius * 2.0;
        if near_edge(dist_a) && far_away(dist_b) && len > radius * 2.0 {
            return true;
        }
        if near_edge(dist_b) && far_away(dist_a) && len > radius * 2.0 {
            return true;
        }
    }
    false
}

/// Read callouts from all DXF files in a fixture set and resolve them against the sheet set.
///
/// Returns (resolved, broken) where `broken` are callouts whose target sheet doesn't exist.
pub fn resolve_callouts(dxf_files: &[(String, Vec<u8>)], sheet_numbers: &[String]) -> (Vec<ResolvedCallout>, Vec<ResolvedCallout>) {
    let mut resolved = Vec::new();
    let mut broken = Vec::new();

    for (name, bytes) in dxf_files {
        let sheet_number = name.trim_end_matches(".dxf").to_string();
        let drawing = match cadaddy_io::read_dxf(bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let callouts = read_callouts(&drawing);
        for co in callouts {
            let target_exists = co.target_sheet.is_empty() || sheet_numbers.contains(&co.target_sheet) || co.kind == "key_plan";
            let rc = ResolvedCallout { callout: co, on_sheet: sheet_number.clone(), resolved: target_exists };
            if target_exists {
                resolved.push(rc);
            } else {
                broken.push(rc);
            }
        }
    }

    (resolved, broken)
}
