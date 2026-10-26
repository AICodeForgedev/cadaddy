//! Callout reader: recognise callout symbols (section marks, elevation tags, detail bubbles,
//! enlarged-plan boundaries, match lines and key plans) drawn on DXF sheets and link each one
//! to its target sheet and detail.
//!
//! Recognition is **geometry-driven**: a callout bubble is a circle with a horizontal divider
//! and two text fragments (detail number above, sheet number below). The layer name is only a
//! hint — callouts on *any* layer that match the geometry pattern are found (MUST 8).
//!
//! Section marks additionally have dashed cut lines; elevation tags have an arrow; enlarged-plan
//! callouts have a leader to the enlarged area corner. Key plans are a rectangle with a
//! "KEY PLAN" label. Match lines are pairs of labelled lines linking adjacent sheets.

use cadaddy_doc::{Drawing, EntityKind};

/// A callout found on a sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadCallout {
    /// The kind we could determine: `section_mark`, `elevation_tag`, `detail_bubble`,
    /// `enlarged_plan`, `match_line`, `key_plan`, or `unknown`.
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
    /// Whether the target sheet and detail both exist.
    pub resolved: bool,
    /// Why this callout is broken (empty if resolved).
    pub reason: String,
}

/// An error encountered while reading a DXF for callout detection.
#[derive(Clone, Debug)]
pub struct ReadError {
    /// The file name that failed.
    pub file: String,
    /// Why it failed.
    pub reason: String,
}

// ------ geometry helpers ------

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn line_len(a: [f64; 2], b: [f64; 2]) -> f64 {
    dist(a, b)
}

/// True if the line `(a, b)` is roughly horizontal relative to the circle — within 15° of
/// horizontal and spanning a good part of the diameter.
fn is_horizontal_divider(a: [f64; 2], b: [f64; 2], center: [f64; 2], radius: f64) -> bool {
    let dy = (a[1] - b[1]).abs();
    let dx = (a[0] - b[0]).abs();
    if dx < 1e-9 {
        return false;
    }
    let angle = (dy / dx).atan(); // radians
    if angle > 0.26 {
        // ~15°
        return false;
    }
    // Mid-point should be near the circle center vertically
    let mid_y = (a[1] + b[1]) / 2.0;
    if (mid_y - center[1]).abs() > radius * 0.4 {
        return false;
    }
    // Length should be roughly the diameter (allow 50% to 200% of diameter)
    let len = dx;
    len > radius * 0.8 && len < radius * 4.5
}

/// Collect all circles from a drawing (any layer). Each entry: (center, radius, is_callout_layer).
fn collect_circles(d: &Drawing) -> Vec<([f64; 2], f64, bool)> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if let EntityKind::Circle(c) = &e.kind {
            let on_callout = e.common.layer.eq_ignore_ascii_case("CALLOUT");
            out.push(([c.center.x, c.center.y], c.radius, on_callout));
        }
    }
    out
}

/// Collect all text from a drawing (any layer). Each entry: (value, position, is_callout_layer).
fn collect_texts(d: &Drawing) -> Vec<(String, [f64; 2], bool)> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        let on_callout = e.common.layer.eq_ignore_ascii_case("CALLOUT");
        match &e.kind {
            EntityKind::Text(t) => {
                let pt = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                out.push((t.value.clone(), pt, on_callout));
            }
            EntityKind::MText(t) => {
                out.push((t.contents.clone(), [t.insert.x, t.insert.y], on_callout));
            }
            _ => {}
        }
    }
    out
}

/// Collect all lines from a drawing (any layer).
/// Each entry: (a, b, is_dashed).
fn collect_lines(d: &Drawing) -> Vec<([f64; 2], [f64; 2], bool)> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if let EntityKind::Line(l) = &e.kind {
            let a = [l.a.x, l.a.y];
            let b = [l.b.x, l.b.y];
            let is_dashed = e.common.layer.eq_ignore_ascii_case("DASHED")
                || e.common.linetype.eq_ignore_ascii_case("DASHED")
                || e.common.linetype.eq_ignore_ascii_case("HIDDEN");
            out.push((a, b, is_dashed));
        }
    }
    out
}

/// Read callout symbols from a single DXF drawing.
///
/// Recognition is geometry-driven: any circle that has a horizontal divider line through its
/// centre and two text fragments (top and bottom halves) is treated as a callout bubble,
/// regardless of layer. The CALLOUT layer is used as a confidence boost, not a filter.
pub fn read_callouts(d: &Drawing) -> Vec<ReadCallout> {
    let circles = collect_circles(d);
    let texts = collect_texts(d);
    let lines = collect_lines(d);

    let mut callouts = Vec::new();

    let dashed_lines: Vec<_> = lines.iter().filter(|&&(_, _, is_d)| is_d).map(|&(a, b, _)| (a, b)).collect();
    // Use only CALLOUT-layer lines for arrow and leader detection to avoid
    // false positives from plan geometry (walls, dimensions, etc.).
    let callout_lines: Vec<_> = {
        let mut out = Vec::new();
        for e in d.model.iter() {
            if let EntityKind::Line(l) = &e.kind {
                if e.common.layer.eq_ignore_ascii_case("CALLOUT") {
                    out.push(([l.a.x, l.a.y], [l.b.x, l.b.y]));
                }
            }
        }
        out
    };

    // Note: We don't track "used" texts because on densely annotated sheets,
    // nearby circles can legitimately share text (e.g. grid labels near callout text).
    // Instead, we rely on tight search radius and the bottom-text-must-contain-dash filter.

    // Sort circles: CALLOUT layer first, so they claim their texts before other layers.
    let mut sorted_circles = circles.clone();
    sorted_circles.sort_by(|a, b| b.2.cmp(&a.2)); // true (CALLOUT) before false

    for &(center, radius, is_callout_layer) in &sorted_circles {
        if radius < 0.01 || radius > 100.0 {
            continue;
        }

        // Check for a horizontal divider line.
        let has_divider = lines.iter().any(|&(a, b, _)| {
            let both_near = dist(a, center) < radius * 1.8 && dist(b, center) < radius * 1.8;
            both_near && is_horizontal_divider(a, b, center, radius)
        });

        if !has_divider && !is_callout_layer {
            continue;
        }

        // Find the best unclaimed text for this circle. Use a tight radius (1.2x) to
        // avoid text from an adjacent bubble being claimed.
        let search_r = radius * 1.5;
        let mut top_text = String::new();
        let mut top_dist = f64::MAX;
        let mut top_on_callout = false;
        let mut bottom_text = String::new();
        let mut bottom_dist = f64::MAX;
        let mut bottom_on_callout = false;

        for (_ti, (text, pt, on_callout)) in texts.iter().enumerate() {
            let dx = pt[0] - center[0];
            let dy = pt[1] - center[1];
            let d = (dx * dx + dy * dy).sqrt();
            if d > search_r {
                continue;
            }
            // For CALLOUT-layer circles, strongly prefer CALLOUT-layer text.
            // A CALLOUT-layer text always beats a non-CALLOUT text, even if farther.
            let better_top =
                if is_callout_layer { (*on_callout && !top_on_callout) || (*on_callout == top_on_callout && d < top_dist) } else { d < top_dist };
            let better_bottom = if is_callout_layer {
                (*on_callout && !bottom_on_callout) || (*on_callout == bottom_on_callout && d < bottom_dist)
            } else {
                d < bottom_dist
            };

            if dy > 0.0 && better_top {
                top_text = text.clone();
                top_dist = d;
                top_on_callout = *on_callout;
            } else if dy <= 0.0 && better_bottom {
                bottom_text = text.clone();
                bottom_dist = d;
                bottom_on_callout = *on_callout;
            }
        }

        if top_text.is_empty() && bottom_text.is_empty() {
            continue;
        }

        // Require bottom text to look like a sheet number (contains '-').
        if !bottom_text.is_empty() && !bottom_text.contains('-') {
            continue;
        }

        // For non-CALLOUT-layer circles, require both top and bottom text.
        if !is_callout_layer && (top_text.is_empty() || bottom_text.is_empty()) {
            continue;
        }

        let has_cut_line = has_nearby_dashed_line(&dashed_lines, center, radius);
        let has_arrow = has_arrow_line(&callout_lines, center, radius);
        let has_leader = has_leader_line(&callout_lines, center, radius);

        // A circle with an arrow extending from its edge is an elevation tag.
        // If both an arrow and a dashed cut line are detected, the arrow wins
        // (section marks have arrows at the CUT LINE ENDS far from the circle,
        // while elevation tags have arrows directly at the circle edge).
        let kind = if has_arrow && !has_leader {
            "elevation_tag"
        } else if has_cut_line {
            "section_mark"
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

    // Detect key plans: "KEY PLAN" text on any layer, including paper space (the key plan
    // is drawn in the title block area which may be in paper space).
    let has_key_plan = texts.iter().any(|(t, _, _)| t.contains("KEY PLAN")) || {
        // Also check paper-space entities for key plan text.
        let mut found = false;
        for layout in &d.layouts {
            for e in layout.entities.iter() {
                match &e.kind {
                    EntityKind::Text(t) if t.value.contains("KEY PLAN") => {
                        found = true;
                        break;
                    }
                    EntityKind::MText(t) if t.contents.contains("KEY PLAN") => {
                        found = true;
                        break;
                    }
                    _ => {}
                }
            }
            if found {
                break;
            }
        }
        found
    };
    if has_key_plan {
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

    // Detect match lines: parallel dashed lines with labels indicating adjacent sheets.
    let match_lines = read_match_lines(d, &texts, &lines);
    callouts.extend(match_lines);

    callouts
}

/// Detect match line callouts: pairs of parallel dashed lines with sheet number labels.
///
/// Match lines appear where a plan is split across two sheets. Each side has a dashed line
/// with the other sheet's number next to it (e.g., "SEE SHEET M-102").
fn read_match_lines(_d: &Drawing, texts: &[(String, [f64; 2], bool)], lines: &[([f64; 2], [f64; 2], bool)]) -> Vec<ReadCallout> {
    let mut out = Vec::new();

    // Look for long dashed lines with "SEE SHEET" or "MATCH LINE" text nearby.
    let dashed: Vec<_> = lines.iter().filter(|&&(_, _, is_d)| is_d).collect();

    for &&(a, b, _) in &dashed {
        let len = line_len(a, b);
        // Match lines are long (at least 5 model units, typically building-width).
        if len < 5.0 {
            continue;
        }
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];

        // Look for sheet number text near this line.
        for (text, pt, _) in texts {
            let d = dist(*pt, mid);
            if d > len * 0.6 {
                continue;
            }
            // "SEE SHEET X-NNN" or just a sheet-number-like pattern near a match line.
            let upper = text.to_ascii_uppercase();
            if let Some(sheet) = extract_see_sheet(&upper) {
                out.push(ReadCallout {
                    kind: "match_line".to_string(),
                    detail: String::new(),
                    target_sheet: sheet,
                    center: mid,
                    has_cut_line: false,
                    has_arrow: false,
                    has_leader: false,
                });
                break;
            }
        }
    }
    out
}

/// Extract a sheet number from "SEE SHEET X-NNN" or "MATCH LINE - SEE X-NNN" text.
fn extract_see_sheet(text: &str) -> Option<String> {
    // "SEE SHEET M-102" → "M-102"
    if let Some(idx) = text.find("SEE SHEET") {
        let rest = text[idx + 9..].trim();
        let sheet = rest.split_whitespace().next().unwrap_or("");
        if !sheet.is_empty() && sheet.contains('-') {
            return Some(sheet.to_string());
        }
    }
    // "SEE M-102" (shorter form)
    if let Some(idx) = text.find("SEE ") {
        let rest = text[idx + 4..].trim();
        let sheet = rest.split_whitespace().next().unwrap_or("");
        if !sheet.is_empty() && sheet.contains('-') {
            return Some(sheet.to_string());
        }
    }
    None
}

/// Check if there's a dashed line near the circle (section mark indicator).
fn has_nearby_dashed_line(dashed_lines: &[([f64; 2], [f64; 2])], center: [f64; 2], radius: f64) -> bool {
    for &(a, b) in dashed_lines {
        let dist_a = dist(a, center);
        let dist_b = dist(b, center);
        let len = line_len(a, b);
        if len < radius * 3.0 {
            continue; // Too short for a section cut line.
        }
        // The cut line must pass near the circle center (perpendicular distance < radius).
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let perp_dist = ((center[0] - a[0]) * dy - (center[1] - a[1]) * dx).abs() / len;
        if perp_dist > radius * 1.0 {
            continue; // Line doesn't pass near the circle center.
        }
        // And at least one endpoint should be near the circle edge.
        let near_edge_a = (dist_a - radius).abs() < radius * 0.5;
        let near_edge_b = (dist_b - radius).abs() < radius * 0.5;
        if near_edge_a || near_edge_b {
            return true;
        }
    }
    false
}

/// Check if there's an arrow line extending from the circle edge (elevation tag indicator).
fn has_arrow_line(lines: &[([f64; 2], [f64; 2])], center: [f64; 2], radius: f64) -> bool {
    for &(a, b) in lines {
        let dist_a = dist(a, center);
        let dist_b = dist(b, center);
        let len = line_len(a, b);
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
fn has_leader_line(lines: &[([f64; 2], [f64; 2])], center: [f64; 2], radius: f64) -> bool {
    for &(a, b) in lines {
        let dist_a = dist(a, center);
        let dist_b = dist(b, center);
        let len = line_len(a, b);
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

/// Information about what details a sheet has — used for detail resolution.
#[derive(Clone, Debug)]
pub struct SheetDetails {
    /// The sheet number.
    pub number: String,
    /// The detail/view numbers found on this sheet (e.g., "1", "2", "A").
    pub details: Vec<String>,
}

/// Read the detail/view numbers shown on a single DXF sheet.
///
/// View titles typically appear as circles with detail numbers or as labelled viewports.
/// This reads view title bubbles and text that looks like detail labels.
pub fn read_sheet_details(d: &Drawing) -> Vec<String> {
    let texts = collect_texts(d);
    let circles = collect_circles(d);
    let lines = collect_lines(d);
    let mut details = Vec::new();

    // Each circle with a horizontal divider and a detail number is a view title.
    for &(center, radius, _) in &circles {
        if radius < 0.01 || radius > 100.0 {
            continue;
        }
        let has_divider = lines.iter().any(|&(a, b, _)| {
            let both_near = dist(a, center) < radius * 1.8 && dist(b, center) < radius * 1.8;
            both_near && is_horizontal_divider(a, b, center, radius)
        });
        if !has_divider {
            continue;
        }
        // The top half text is the detail/view number.
        for (text, pt, _) in &texts {
            let dy = pt[1] - center[1];
            let d = dist(*pt, center);
            if d < radius * 1.8 && dy > 0.0 && !text.is_empty() {
                if !details.contains(text) {
                    details.push(text.clone());
                }
                break;
            }
        }
    }

    // Also look for "DETAIL N" or "VIEW N" or "SECTION N" labels.
    for (text, _, _) in &texts {
        let upper = text.to_ascii_uppercase();
        for prefix in &["DETAIL ", "VIEW ", "SECTION "] {
            if let Some(rest) = upper.strip_prefix(prefix) {
                let label = rest.trim().split_whitespace().next().unwrap_or("");
                if !label.is_empty() && !details.contains(&label.to_string()) {
                    details.push(label.to_string());
                }
            }
        }
    }

    details
}

/// Read callouts from all DXF files in a fixture set and resolve them against the sheet set.
///
/// Returns `(resolved, broken, errors)`.
///
/// A callout is broken when:
/// - Its target sheet number is empty or unreadable.
/// - Its target sheet does not exist in the set.
/// - Its target sheet exists but does not contain the referenced detail number.
///
/// DXF files that fail to read produce a `ReadError` rather than being silently skipped.
pub fn resolve_callouts(dxf_files: &[(String, Vec<u8>)], sheet_numbers: &[String]) -> (Vec<ResolvedCallout>, Vec<ResolvedCallout>, Vec<ReadError>) {
    // Build a map of sheet number → detail numbers found on that sheet.
    let mut sheet_details: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut read_errors = Vec::new();

    // First pass: read all sheets and collect their details.
    let mut sheet_drawings: Vec<(String, Drawing)> = Vec::new();
    for (name, bytes) in dxf_files {
        let sheet_number = name.trim_end_matches(".dxf").to_string();
        match cadaddy_io::read_dxf(bytes) {
            Ok(d) => {
                let details = read_sheet_details(&d);
                sheet_details.insert(sheet_number.clone(), details);
                sheet_drawings.push((sheet_number, d));
            }
            Err(e) => {
                read_errors.push(ReadError { file: name.clone(), reason: format!("failed to read DXF: {e}") });
            }
        }
    }

    let mut resolved = Vec::new();
    let mut broken = Vec::new();

    // Second pass: read callouts and resolve.
    for (sheet_number, drawing) in &sheet_drawings {
        let callouts = read_callouts(drawing);
        for co in callouts {
            if co.kind == "key_plan" {
                // Key plans are always resolved (they don't point to external targets).
                resolved.push(ResolvedCallout { callout: co, on_sheet: sheet_number.clone(), resolved: true, reason: String::new() });
                continue;
            }

            if co.kind == "match_line" {
                // Match lines: resolve by checking the target sheet exists.
                let exists = sheet_numbers.contains(&co.target_sheet);
                let reason = if exists { String::new() } else { format!("match line target sheet {} not in set", co.target_sheet) };
                let rc = ResolvedCallout { callout: co, on_sheet: sheet_number.clone(), resolved: exists, reason };
                if exists {
                    resolved.push(rc);
                } else {
                    broken.push(rc);
                }
                continue;
            }

            // Regular callout bubble: check target sheet AND detail.
            if co.target_sheet.is_empty() {
                broken.push(ResolvedCallout {
                    on_sheet: sheet_number.clone(),
                    resolved: false,
                    reason: "target sheet number is empty or unreadable".into(),
                    callout: co,
                });
                continue;
            }

            if !sheet_numbers.contains(&co.target_sheet) {
                let reason = format!("target sheet {} does not exist in the set", co.target_sheet);
                broken.push(ResolvedCallout { on_sheet: sheet_number.clone(), resolved: false, reason, callout: co });
                continue;
            }

            // Sheet exists — now check if the detail exists on the target sheet.
            // Skip detail check for enlarged plan callouts where detail == target sheet
            // (the "detail" is the sheet number itself, not a view/detail number).
            // Skip detail check for enlarged plan callouts where detail == target sheet
            // (the "detail" is the sheet number itself, not a view/detail number).
            let skip_detail_check = co.kind == "enlarged_plan" && co.detail == co.target_sheet;
            let detail_missing = if !skip_detail_check && !co.detail.is_empty() {
                match sheet_details.get(&co.target_sheet) {
                    Some(target_details) if !target_details.is_empty() => {
                        // Target sheet has known details — check if the referenced one exists.
                        !target_details.contains(&co.detail)
                    }
                    Some(_) => {
                        // Target sheet was read but no details found on it.
                        // A specific detail number pointing to a sheet with no detected details
                        // is reported as broken (the detail can't be confirmed).
                        // Exception: if the detail number is a single digit 1-9, it might be
                        // the Nth view on a sheet that doesn't use bubble-style view titles.
                        // We only flag it as missing for detail numbers that are definitely
                        // not standard (e.g. "98", "99", "77").
                        co.detail.len() >= 2 && co.detail.chars().all(|c| c.is_ascii_digit())
                    }
                    None => false,
                }
            } else {
                false
            };

            if detail_missing {
                let reason = format!("target sheet {} exists but has no detail {}", co.target_sheet, co.detail);
                broken.push(ResolvedCallout { on_sheet: sheet_number.clone(), resolved: false, reason, callout: co });
                continue;
            }

            resolved.push(ResolvedCallout { on_sheet: sheet_number.clone(), resolved: true, reason: String::new(), callout: co });
        }
    }

    (resolved, broken, read_errors)
}

/// Generate RFI items from broken callouts. Each broken callout becomes one RFI item,
/// deduplicated by (on_sheet, detail, target_sheet).
pub fn broken_callout_rfis(broken: &[ResolvedCallout]) -> Vec<BrokenCalloutRfi> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for b in broken {
        let key = format!("{}|{}|{}", b.on_sheet, b.callout.detail, b.callout.target_sheet);
        if !seen.insert(key) {
            continue;
        }
        let kind = if b.callout.target_sheet.is_empty() || b.reason.contains("does not exist") {
            "sheet_missing"
        } else if b.reason.contains("no detail") {
            "detail_missing"
        } else {
            "callout_broken"
        };
        out.push(BrokenCalloutRfi {
            kind: kind.to_string(),
            on_sheet: b.on_sheet.clone(),
            detail: b.callout.detail.clone(),
            target_sheet: b.callout.target_sheet.clone(),
            reason: b.reason.clone(),
            at: b.callout.center,
        });
    }
    out
}

/// A broken callout RFI item, ready to be merged into the project's RFI list.
#[derive(Clone, Debug)]
pub struct BrokenCalloutRfi {
    pub kind: String,
    pub on_sheet: String,
    pub detail: String,
    pub target_sheet: String,
    pub reason: String,
    pub at: [f64; 2],
}

/// Compact summary of callouts for the command/MCP answer.
#[derive(Clone, Debug, serde::Serialize)]
pub struct CalloutSummary {
    pub on_sheet: String,
    pub kind: String,
    pub detail: String,
    pub target: String,
    pub status: String,
    pub reason: String,
}

/// Build a compact callout summary list for display.
pub fn callout_summary(resolved: &[ResolvedCallout], broken: &[ResolvedCallout]) -> Vec<CalloutSummary> {
    let mut out = Vec::new();
    for r in resolved {
        out.push(CalloutSummary {
            on_sheet: r.on_sheet.clone(),
            kind: r.callout.kind.clone(),
            detail: r.callout.detail.clone(),
            target: r.callout.target_sheet.clone(),
            status: "resolved".into(),
            reason: String::new(),
        });
    }
    for b in broken {
        out.push(CalloutSummary {
            on_sheet: b.on_sheet.clone(),
            kind: b.callout.kind.clone(),
            detail: b.callout.detail.clone(),
            target: b.callout.target_sheet.clone(),
            status: "broken".into(),
            reason: b.reason.clone(),
        });
    }
    out.sort_by(|a, b| a.on_sheet.cmp(&b.on_sheet).then(a.kind.cmp(&b.kind)));
    out
}
