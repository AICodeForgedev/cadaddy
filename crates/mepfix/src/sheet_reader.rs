//! Sheet recognition and alignment: reading DXF sheets, identifying each sheet's discipline,
//! number, level and scale from its title block, detecting the structural grid, and computing
//! the building-coordinate transform so later pipeline steps work in a common frame.
//!
//! The reader is text-driven: it uses layer names (`TITLE`, `GRID`, `GRID-BUBBLE`, `TEXT`) and
//! the textual content of the entities, not their absolute positions or sizes.

use crate::layout;
use crate::truth::{GridLine, Sheet};
use cadaddy_doc::{Drawing, EntityKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- Data structures ----------------------------------------------------------------

/// A recognised sheet, carrying everything later pipeline steps need.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RecognisedSheet {
    /// The sheet number as read from the title block (`M-101`).
    pub number: String,
    /// The sheet title (`LEVEL 1 MECHANICAL PLAN`).
    pub title: String,
    /// Discipline code: `G`, `A`, `S`, `M`, `P`, `FP`.
    pub discipline: String,
    /// Level code (`1`, `2`, `R`) for plan sheets; `None` for covers and schedule sheets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// The scale text as printed (`1/8" = 1'-0"`), if found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_text: Option<String>,
    /// Paper inches per building foot.
    pub scale: f64,
    /// Detected grid lines of constant x (running north–south on the plan), in model inches.
    pub grid_x: Vec<DetectedGrid>,
    /// Detected grid lines of constant y, in model inches.
    pub grid_y: Vec<DetectedGrid>,
    /// The computed model-inches → building-feet transform, if alignment succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<Alignment>,
    /// Raw grid bubble data: (label, [model_x, model_y]) for each detected bubble.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bubbles: Vec<(String, [f64; 2])>,
}

/// A grid line detected in a DXF drawing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectedGrid {
    pub label: String,
    /// Position in model inches (x for x-lines, y for y-lines).
    pub at_in: f64,
}

/// The computed transform from model inches to building feet, derived from grid alignment.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Alignment {
    pub rotation_deg: f64,
    pub offset_in: [f64; 2],
}

/// The sheet map: the full recognition result for a set of sheets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetMap {
    pub sheets: Vec<RecognisedSheet>,
    /// The common grid, assembled from all sheets: labels and building-feet positions.
    pub grid_x: Vec<GridLine>,
    pub grid_y: Vec<GridLine>,
}

// ---- Title block parsing ------------------------------------------------------------

/// Known discipline prefixes: sheet number prefix → discipline code.
const DISCIPLINE_PREFIX: &[(&str, &str)] = &[("FP-", "FP"), ("M-", "M"), ("P-", "P"), ("S-", "S"), ("A-", "A"), ("G-", "G"), ("E-", "E")];

/// Parse a scale string like `1/8" = 1'-0"` → paper inches per building foot.
pub fn parse_scale(s: &str) -> Option<f64> {
    let s = s.trim();
    for &(text, val) in layout::ENLARGED_SCALES.iter().chain(crate::style::SCALES.iter()) {
        if s == text {
            return Some(val);
        }
    }
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() == 2 {
        let left = parts[0].trim().trim_end_matches('"').trim();
        if let Some(val) = parse_fraction(left) {
            return Some(val);
        }
    }
    None
}

fn parse_fraction(s: &str) -> Option<f64> {
    if let Some((n, d)) = s.split_once('/') {
        let n: f64 = n.trim().parse().ok()?;
        let d: f64 = d.trim().parse().ok()?;
        if d.abs() > 1e-12 {
            return Some(n / d);
        }
    }
    s.trim().parse::<f64>().ok()
}

fn detect_sheet_number(d: &Drawing) -> Option<String> {
    let mut best: Option<(f64, String)> = None;
    for e in d.model.iter() {
        let on_title = e.common.layer.eq_ignore_ascii_case("TITLE") || e.common.layer.eq_ignore_ascii_case("BORDER");
        if !on_title {
            continue;
        }
        let (h, val) = match &e.kind {
            EntityKind::Text(t) => (t.height, t.value.clone()),
            EntityKind::MText(t) => (t.height, t.contents.clone()),
            _ => continue,
        };
        if h > best.as_ref().map_or(0.0, |b| b.0) && !val.is_empty() {
            best = Some((h, val));
        }
    }
    best.map(|(_, v)| v)
}

fn detect_scale_text(d: &Drawing) -> Option<(String, f64)> {
    for e in d.model.iter() {
        let on_title = e.common.layer.eq_ignore_ascii_case("TITLE") || e.common.layer.eq_ignore_ascii_case("TEXT");
        if !on_title {
            continue;
        }
        let val = match &e.kind {
            EntityKind::Text(t) => &t.value,
            EntityKind::MText(t) => &t.contents,
            _ => continue,
        };
        if let Some(rest) = val.strip_prefix("SCALE:").or_else(|| val.strip_prefix("SCALE :")) {
            let rest = rest.trim();
            if let Some(scale) = parse_scale(rest) {
                return Some((rest.to_string(), scale));
            }
        }
    }
    None
}

fn detect_discipline(number: &str, texts: &[(String, [f64; 2])]) -> String {
    for &(prefix, disc) in DISCIPLINE_PREFIX {
        if number.starts_with(prefix) {
            return disc.to_string();
        }
    }
    let keywords =
        [("MECHANICAL", "M"), ("PLUMBING", "P"), ("FIRE PROTECTION", "FP"), ("STRUCTURAL", "S"), ("ARCHITECTURAL", "A"), ("REFLECTED CEILING", "A")];
    for (text, _pos) in texts {
        let upper = text.to_uppercase();
        for &(kw, disc) in &keywords {
            if upper.contains(kw) {
                return disc.to_string();
            }
        }
    }
    "G".to_string()
}

fn detect_level(title: &str) -> Option<String> {
    let upper = title.to_uppercase();
    if let Some(rest) = upper.strip_prefix("LEVEL ").or_else(|| upper.find("LEVEL ").map(|i| &upper[i + 6..])) {
        let code = rest.split_whitespace().next().unwrap_or("");
        if !code.is_empty() {
            return Some(code.to_string());
        }
    }
    if upper.contains("ROOF") {
        return Some("R".to_string());
    }
    None
}

fn detect_title(d: &Drawing, number: &str) -> String {
    let mut candidates: Vec<(f64, String)> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("TITLE") {
            continue;
        }
        let (h, val) = match &e.kind {
            EntityKind::Text(t) => (t.height, t.value.clone()),
            EntityKind::MText(t) => (t.height, t.contents.clone()),
            _ => continue,
        };
        if val == number || val.is_empty() {
            continue;
        }
        candidates.push((h, val));
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, val) in &candidates {
        let upper = val.to_uppercase();
        if upper.contains("PLAN") || upper.contains("SCHEDULE") || upper.contains("COVER") || upper.contains("FRAMING") || upper.contains("CEILING") {
            return val.clone();
        }
    }
    candidates.first().map_or_else(String::new, |(_, v)| v.clone())
}

// ---- Grid detection -----------------------------------------------------------------

/// Detect grid bubbles: circles on GRID-BUBBLE layer, with text labels inside.
fn detect_grid_bubbles(d: &Drawing) -> Vec<(String, [f64; 2])> {
    let mut circles: Vec<(f64, f64, f64)> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE") {
            continue;
        }
        if let EntityKind::Circle(c) = &e.kind {
            circles.push((c.center.x, c.center.y, c.radius));
        }
    }
    let mut texts: Vec<(String, [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE") {
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
    let mut bubbles: Vec<(String, [f64; 2])> = Vec::new();
    for (label, pt) in &texts {
        if label.trim().is_empty() {
            continue;
        }
        let mut best_dist = f64::INFINITY;
        let mut best_c: Option<[f64; 2]> = None;
        for &(cx, cy, r) in &circles {
            let dist = ((pt[0] - cx).powi(2) + (pt[1] - cy).powi(2)).sqrt();
            if dist < r * 1.5 && dist < best_dist {
                best_dist = dist;
                best_c = Some([cx, cy]);
            }
        }
        if let Some(centre) = best_c {
            bubbles.push((label.clone(), centre));
        }
    }
    bubbles
}

/// Detect grid lines on the GRID layer.
fn detect_grid_lines(d: &Drawing) -> Vec<([f64; 2], [f64; 2])> {
    let mut lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("GRID") {
            continue;
        }
        if let EntityKind::Line(l) = &e.kind {
            lines.push(([l.a.x, l.a.y], [l.b.x, l.b.y]));
        }
    }
    lines
}

/// Classify detected grid lines into x-constant and y-constant, for unrotated plans.
fn group_grid_lines_simple(bubbles: &[(String, [f64; 2])]) -> (Vec<DetectedGrid>, Vec<DetectedGrid>) {
    let mut by_label: BTreeMap<String, Vec<[f64; 2]>> = BTreeMap::new();
    for (label, pos) in bubbles {
        by_label.entry(label.clone()).or_default().push(*pos);
    }

    let mut x_grids = Vec::new();
    let mut y_grids = Vec::new();

    for (label, positions) in &by_label {
        if positions.len() < 2 {
            continue;
        }
        let (a, b) = (positions[0], positions[1]);
        let dx = (b[0] - a[0]).abs();
        let dy = (b[1] - a[1]).abs();
        let mid_x = (a[0] + b[0]) / 2.0;
        let mid_y = (a[1] + b[1]) / 2.0;

        if dx < dy * 0.3 + 1e-6 {
            // Nearly same x → x-constant grid line.
            x_grids.push(DetectedGrid { label: label.clone(), at_in: mid_x });
        } else if dy < dx * 0.3 + 1e-6 {
            // Nearly same y → y-constant grid line.
            y_grids.push(DetectedGrid { label: label.clone(), at_in: mid_y });
        }
        // else: rotated plan, classification deferred to alignment.
    }

    x_grids.sort_by(|a, b| a.at_in.total_cmp(&b.at_in));
    y_grids.sort_by(|a, b| a.at_in.total_cmp(&b.at_in));

    (x_grids, y_grids)
}

// ---- Alignment computation ----------------------------------------------------------

/// Compute alignment using bubble pairs and truth grid.
///
/// Uses the direction of bubble pairs to determine rotation, then computes offset using
/// grid line intersections or perpendicular projections.
fn compute_alignment_from_bubbles(bubbles: &[(String, [f64; 2])], grid_x: &[GridLine], grid_y: &[GridLine]) -> Option<Alignment> {
    let x_map: BTreeMap<&str, f64> = grid_x.iter().map(|g| (g.label.as_str(), g.at_ft)).collect();
    let y_map: BTreeMap<&str, f64> = grid_y.iter().map(|g| (g.label.as_str(), g.at_ft)).collect();

    let mut by_label: BTreeMap<&str, Vec<[f64; 2]>> = BTreeMap::new();
    for (label, pos) in bubbles {
        by_label.entry(label.as_str()).or_default().push(*pos);
    }

    // Step 1: determine rotation from bubble pair directions.
    // x-grid labels (constant x, runs along building y): direction = rotated [0, 1] = [-sin θ, cos θ]
    // y-grid labels (constant y, runs along building x): direction = rotated [1, 0] = [cos θ, sin θ]
    let mut rotation_estimates = Vec::new();

    for (label, positions) in &by_label {
        if positions.len() < 2 {
            continue;
        }
        let (a, b) = (positions[0], positions[1]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = (d[0].powi(2) + d[1].powi(2)).sqrt();
        if l < 1e-9 {
            continue;
        }
        let dir = [d[0] / l, d[1] / l];

        if x_map.contains_key(label) {
            // x-grid line direction in model = [-sin θ, cos θ] (or flipped)
            // θ = atan2(-dir[0], dir[1]) or atan2(dir[0], -dir[1])
            // Use both signs since the bubble order is arbitrary.
            rotation_estimates.push((-dir[0]).atan2(dir[1]));
        } else if y_map.contains_key(label) {
            // y-grid line direction in model = [cos θ, sin θ] (or flipped)
            rotation_estimates.push(dir[1].atan2(dir[0]));
        }
    }

    if rotation_estimates.is_empty() {
        return None;
    }

    // Normalise angles to a consistent range and average.
    // Angles might be off by π due to bubble order. Normalise to [0, π).
    let normalise = |a: f64| {
        let mut a = a % std::f64::consts::PI;
        if a < 0.0 {
            a += std::f64::consts::PI;
        }
        a
    };
    let norm: Vec<f64> = rotation_estimates.iter().map(|a| normalise(*a)).collect();
    let avg_norm = norm.iter().sum::<f64>() / norm.len() as f64;

    // Pick the candidate (avg_norm or avg_norm - π) closest to the original estimates.
    let candidates = [avg_norm, avg_norm - std::f64::consts::PI];
    let theta = *candidates
        .iter()
        .min_by(|a, b| {
            let da: f64 = rotation_estimates.iter().map(|r| (r - *a).abs()).sum();
            let db: f64 = rotation_estimates.iter().map(|r| (r - *b).abs()).sum();
            da.total_cmp(&db)
        })
        .unwrap_or(&avg_norm);

    let rotation_deg = theta.to_degrees();

    // Step 2: compute offset using grid line midpoints.
    // For an x-grid bubble pair with label L:
    //   midpoint_model = rotate([L_ft * 12, C_y * 12], θ) + offset
    // where C_y is the building y-centre of the grid.
    // We don't know C_y, but the perpendicular projection of the midpoint onto the building
    // x-axis gives:
    //   midpoint · [cos θ, sin θ] = L_ft * 12 + offset · [cos θ, sin θ]
    //
    // Similarly for y-grid:
    //   midpoint · [-sin θ, cos θ] = L_ft * 12 + offset · [-sin θ, cos θ]

    let (cos_t, sin_t) = theta.sin_cos();
    // Note: sin_cos returns (sin, cos), so swap.
    let (sin_t, cos_t) = (cos_t, sin_t);
    let bx_dir = [cos_t, sin_t]; // building x direction in model
    let by_dir = [-sin_t, cos_t]; // building y direction in model

    let mut off_bx_estimates = Vec::new(); // offset projected onto building-x direction
    let mut off_by_estimates = Vec::new();

    for (label, positions) in &by_label {
        if positions.len() < 2 {
            continue;
        }
        let mid = [(positions[0][0] + positions[1][0]) / 2.0, (positions[0][1] + positions[1][1]) / 2.0];

        if let Some(&ft) = x_map.get(label) {
            // x-grid line: building x = ft.
            // mid · bx_dir = ft * 12 + offset · bx_dir
            let proj = mid[0] * bx_dir[0] + mid[1] * bx_dir[1];
            off_bx_estimates.push(proj - ft * 12.0);
        }
        if let Some(&ft) = y_map.get(label) {
            // y-grid line: building y = ft.
            // mid · by_dir = ft * 12 + offset · by_dir
            let proj = mid[0] * by_dir[0] + mid[1] * by_dir[1];
            off_by_estimates.push(proj - ft * 12.0);
        }
    }

    if off_bx_estimates.is_empty() || off_by_estimates.is_empty() {
        return None;
    }

    let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let off_bx = avg(&off_bx_estimates);
    let off_by = avg(&off_by_estimates);

    // Reconstruct offset in model coordinates.
    let offset_in = [off_bx * bx_dir[0] + off_by * by_dir[0], off_bx * bx_dir[1] + off_by * by_dir[1]];

    Some(Alignment { rotation_deg, offset_in: [round(offset_in[0]), round(offset_in[1])] })
}

fn round(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

// ---- Public API ---------------------------------------------------------------------

fn all_texts(d: &Drawing) -> Vec<(String, [f64; 2])> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some((t.value.clone(), [t.insert.x, t.insert.y])),
            EntityKind::MText(t) => Some((t.contents.clone(), [t.insert.x, t.insert.y])),
            _ => None,
        })
        .collect()
}

/// Recognise one DXF sheet: title block, scale, grid.
pub fn recognise_dxf(d: &Drawing) -> RecognisedSheet {
    let number = detect_sheet_number(d).unwrap_or_default();
    let title = detect_title(d, &number);
    let texts = all_texts(d);
    let discipline = detect_discipline(&number, &texts);
    let level = detect_level(&title);
    let (scale_text, scale) = detect_scale_text(d).map(|(t, s)| (Some(t), s)).unwrap_or((None, 0.0));

    let bubbles = detect_grid_bubbles(d);
    let _grid_lines = detect_grid_lines(d);
    let (grid_x, grid_y) = group_grid_lines_simple(&bubbles);

    RecognisedSheet { number, title, discipline, level, scale_text, scale, grid_x, grid_y, alignment: None, bubbles }
}

/// Build a sheet map from a set of DXF drawings: recognise each sheet, assemble the common
/// grid, then compute each sheet's alignment.
pub fn build_sheet_map(drawings: &[(String, &Drawing)]) -> SheetMap {
    let mut sheets: Vec<RecognisedSheet> = Vec::new();

    // Phase 1: recognise each sheet.
    for (name, d) in drawings {
        let mut sh = recognise_dxf(d);
        if sh.number.is_empty() {
            sh.number = name.clone();
        }
        sheets.push(sh);
    }

    // Phase 2: assemble the common grid from all sheets' detected grid lines.
    let (grid_x, grid_y) = assemble_grid(&sheets);

    // Phase 3: compute alignment for each sheet.
    for sh in &mut sheets {
        if sh.bubbles.is_empty() {
            continue;
        }
        // Try bubble-based alignment (works for rotated and unrotated plans).
        sh.alignment = compute_alignment_from_bubbles(&sh.bubbles, &grid_x, &grid_y);
    }

    SheetMap { sheets, grid_x, grid_y }
}

/// Compute a fingerprint of a sheet's grid label classification: which labels are in grid_x.
fn grid_x_labels(sh: &RecognisedSheet) -> Vec<String> {
    let mut labels: Vec<String> = sh.grid_x.iter().map(|g| g.label.clone()).collect();
    labels.sort();
    labels
}

/// Assemble the common grid from detected grids across all sheets.
/// Uses majority-vote label classification to handle rotated sheets correctly:
/// most sheets are unrotated, so the majority classification of labels into x/y is correct.
fn assemble_grid(sheets: &[RecognisedSheet]) -> (Vec<GridLine>, Vec<GridLine>) {
    // Count label classifications across all sheets. The majority determines the correct
    // assignment of labels to x-grid vs y-grid (rotated sheets are the minority).
    let valid: Vec<&RecognisedSheet> = sheets.iter().filter(|s| s.scale > 1e-9 && !s.grid_x.is_empty() && !s.grid_y.is_empty()).collect();
    if valid.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Group by label classification fingerprint, pick the most common one.
    let mut classification_counts: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    for s in &valid {
        *classification_counts.entry(grid_x_labels(s)).or_default() += 1;
    }
    let majority_x_labels = classification_counts.into_iter().max_by_key(|(_, count)| *count).map(|(labels, _)| labels);

    let Some(majority_x) = majority_x_labels else {
        return (Vec::new(), Vec::new());
    };

    // Pick the best sheet matching the majority classification.
    let best = valid.iter().filter(|s| grid_x_labels(s) == majority_x).max_by_key(|s| s.grid_x.len() + s.grid_y.len());

    let Some(ref_sheet) = best else {
        return (Vec::new(), Vec::new());
    };

    let to_ft = |grids: &[DetectedGrid]| -> Vec<GridLine> {
        if grids.is_empty() {
            return Vec::new();
        }
        let origin = grids[0].at_in;
        grids.iter().map(|g| GridLine { label: g.label.clone(), at_ft: (g.at_in - origin) / 12.0 }).collect()
    };

    (to_ft(&ref_sheet.grid_x), to_ft(&ref_sheet.grid_y))
}

/// Convert model inches to building feet using a sheet's alignment.
pub fn building_from_model(alignment: &Alignment, m: [f64; 2]) -> [f64; 2] {
    let p = [m[0] - alignment.offset_in[0], m[1] - alignment.offset_in[1]];
    let r = layout::rotate(p, -alignment.rotation_deg);
    [r[0] / 12.0, r[1] / 12.0]
}

/// Build a sheet map directly from a fixture set's DXF data.
pub fn recognise_fixture_set(dxf_data: &[(String, Vec<u8>)]) -> SheetMap {
    let mut drawings: Vec<(String, Drawing)> = Vec::new();
    for (name, bytes) in dxf_data {
        match cadaddy_io::read_dxf(bytes) {
            Ok(d) => drawings.push((name.clone(), d)),
            Err(_) => continue,
        }
    }
    let refs: Vec<(String, &Drawing)> = drawings.iter().map(|(n, d)| (n.clone(), d)).collect();
    build_sheet_map(&refs)
}

// ---- Scoring ------------------------------------------------------------------------

/// Score for recognising one sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetScore {
    pub number_correct: bool,
    pub discipline_correct: bool,
    pub level_correct: bool,
    pub scale_correct: bool,
    /// Grid alignment error in feet (mean absolute error of grid intersection positions).
    pub grid_error_ft: Option<f64>,
}

/// Score the recognised sheet map against the truth.
pub fn score_sheet_map(map: &SheetMap, truth_sheets: &[Sheet], truth_grid_x: &[GridLine], truth_grid_y: &[GridLine]) -> Vec<SheetScore> {
    let mut scores = Vec::new();

    for ts in truth_sheets {
        let found = map.sheets.iter().find(|s| s.number == ts.number);
        let mut sc = SheetScore::default();
        if let Some(f) = found {
            sc.number_correct = true;
            sc.discipline_correct = f.discipline == ts.discipline;
            sc.level_correct = f.level == ts.level;
            sc.scale_correct =
                ts.scale_text.as_ref().is_some_and(|st| f.scale_text.as_ref().is_some_and(|ft| ft == st)) || (f.scale - ts.scale).abs() < 0.01;

            if let Some(alignment) = &f.alignment {
                let mut errors = Vec::new();
                for gx in truth_grid_x {
                    for gy in truth_grid_y {
                        let building_pt = [gx.at_ft, gy.at_ft];
                        let model_pt = layout::to_model(ts, building_pt);
                        let back = building_from_model(alignment, model_pt);
                        let err = ((back[0] - building_pt[0]).powi(2) + (back[1] - building_pt[1]).powi(2)).sqrt();
                        errors.push(err);
                    }
                }
                if !errors.is_empty() {
                    sc.grid_error_ft = Some(errors.iter().sum::<f64>() / errors.len() as f64);
                }
            }
        }
        scores.push(sc);
    }

    scores
}
