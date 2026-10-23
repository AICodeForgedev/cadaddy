//! Import accuracy check: validates the quality of a PDF-imported drawing and gates the BIM
//! pipeline on import quality. Runs automatically after every PDF import (convert, open,
//! printset.open/reimport); results are stored in the .cdy (`importCheck` manifest field) and
//! shown in the UI as a notification listing failures with zoom-to.
//!
//! Checks (each its own id, thresholds in one table):
//! - **SCALE**: compare model extents against the sheet's paper size AND scale note; handle
//!   "DO NOT SCALE" notes on MEP/E sheets (becomes a REGISTRATION check).
//! - **LAYERS**: content split onto layers; fail if >90% of entities on one layer.
//! - **TEXT**: compare real text count with detected lettering stroke clusters; fail when
//!   recovered text < 50% of detected lettering.
//! - **FILLS**: no opaque fill/wipeout covering visible content. **Hard-fail.**
//! - **SYMBOLS**: repeated symbols became blocks; fail if 0 blocks when repeated clusters exist.
//!   **Hard-fail.**
//! - **GRID**: grid bubbles + lines found.
//! - **VISUAL**: placeholder for rasterize-and-diff (requires rendering; scored but never fails).
//!
//! **Hard-fail checks** (SCALE, FILLS, SYMBOLS): each fails the import on its own regardless of
//! the weighted score. The score is kept for display only.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandSpec, always, str_param};
use crate::{EngineError, Result, Session};
use cadaddy_doc::{Drawing, EntityKind, Space};

// ─── Thresholds (one table) ──────────────────────────────────────────────────

/// Standard architectural sheet widths in inches: ARCH C 18×24, ARCH D 24×36, ARCH E 36×48.
/// We consider extents "sheet-sized" when they fall within 2× of these ranges.
const PAPER_WIDTH_MIN_IN: f64 = 12.0;
const PAPER_WIDTH_MAX_IN: f64 = 96.0;
const PAPER_HEIGHT_MIN_IN: f64 = 9.0;
const PAPER_HEIGHT_MAX_IN: f64 = 72.0;

/// PDF points equivalents (1 inch = 72 pts).
const PAPER_WIDTH_MIN_PT: f64 = PAPER_WIDTH_MIN_IN * 72.0;
const PAPER_WIDTH_MAX_PT: f64 = PAPER_WIDTH_MAX_IN * 72.0;
const PAPER_HEIGHT_MIN_PT: f64 = PAPER_HEIGHT_MIN_IN * 72.0;
const PAPER_HEIGHT_MAX_PT: f64 = PAPER_HEIGHT_MAX_IN * 72.0;

/// Maximum disagreement between model extents and scale-note-predicted size.
const SCALE_DISAGREE_FRACTION: f64 = 0.10;

/// Fraction of entities on a single layer above which the LAYERS check fails.
const LAYER_CONCENTRATION_FAIL: f64 = 0.90;

/// Minimum entity count before the TEXT check applies (very small imports are exempt).
const TEXT_CHECK_MIN_ENTITIES: usize = 100;

/// Minimum ratio of real text entities to detected lettering-glyph clusters.
/// When recovered text < 50% of detected lettering, TEXT fails.
const TEXT_RECOVERY_MIN: f64 = 0.50;

/// Fraction of entities that are wipeouts or raster images above which FILLS fails.
const FILLS_FRACTION_FAIL: f64 = 0.10;

/// Minimum entity count before the SYMBOLS check expects blocks.
const SYMBOLS_CHECK_MIN_ENTITIES: usize = 200;

/// Overall pass threshold: a score below this fails the import. With hard-fail checks this
/// is for display only; a hard-fail overrides it.
const PASS_SCORE: u32 = 50;

/// Maximum line segment length (in model units) to count as a potential stroke glyph segment.
/// Real lettering strokes on a sheet-scale drawing are typically < 0.5 inches.
const STROKE_GLYPH_MAX_LEN: f64 = 2.0;

/// Minimum cluster size (number of short segments) to count as a lettering-glyph cluster.
const STROKE_CLUSTER_MIN_SEGS: usize = 3;

/// Spatial clustering radius for glyph detection: segments within this distance of each other's
/// midpoints are part of the same cluster.
const STROKE_CLUSTER_RADIUS: f64 = 3.0;

/// Maximum height (bounding box) of a stroke cluster to be considered a glyph.
const STROKE_CLUSTER_MAX_HEIGHT: f64 = 5.0;

// ─── Check result types ──────────────────────────────────────────────────────

/// One check's result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckResult {
    pub id: String,
    pub pass: bool,
    pub expected: String,
    pub found: String,
    pub severity: u32,
    pub hint: String,
    /// Whether this check is a hard-fail: failing it fails the whole import regardless of score.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hard_fail: bool,
    /// Structured evidence for downstream consumers (guidedrun checkpoints, UI zoom-to).
    /// Contains entity handles, positions, measurements depending on the check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The whole import check.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportCheckResult {
    pub pass: bool,
    pub score: u32,
    pub checks: Vec<CheckResult>,
    /// Which check ids triggered a hard failure (empty when passing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hard_fails: Vec<String>,
    /// Detected sheet discipline (A/E/M/P/FP/S/G), if determinable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discipline: Option<String>,
    /// "DO NOT SCALE" note text, if found on the sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub do_not_scale: Option<String>,
}

impl ImportCheckResult {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}

// ─── The checks ──────────────────────────────────────────────────────────────

/// Run all import checks on a drawing. Returns the composite result.
pub fn check_drawing(d: &Drawing) -> ImportCheckResult {
    let mut checks = Vec::new();
    let mut total_weight = 0u32;
    let mut earned = 0u32;

    // Detect discipline and DO NOT SCALE note for the SCALE/ADDENDUM logic.
    let discipline = detect_discipline(d);
    let do_not_scale_note = detect_do_not_scale(d);

    let scale_check = check_scale(d, do_not_scale_note.as_deref());
    add(&mut checks, &scale_check, 25, &mut total_weight, &mut earned);

    let layer_check = check_layers(d);
    add(&mut checks, &layer_check, 20, &mut total_weight, &mut earned);

    let text_check = check_text(d);
    add(&mut checks, &text_check, 20, &mut total_weight, &mut earned);

    let fills_check = check_fills(d);
    add(&mut checks, &fills_check, 15, &mut total_weight, &mut earned);

    let symbols_check = check_symbols(d);
    add(&mut checks, &symbols_check, 10, &mut total_weight, &mut earned);

    let grid_check = check_grid(d);
    add(&mut checks, &grid_check, 10, &mut total_weight, &mut earned);

    let score = if total_weight > 0 { (earned * 100) / total_weight } else { 100 };

    // Hard-fail: SCALE, FILLS, SYMBOLS each fail the import on their own.
    let hard_fails: Vec<String> = checks
        .iter()
        .filter(|c| c.hard_fail && !c.pass)
        .map(|c| c.id.clone())
        .collect();

    let pass = hard_fails.is_empty() && score >= PASS_SCORE;

    ImportCheckResult { pass, score, checks, hard_fails, discipline, do_not_scale: do_not_scale_note }
}

fn add(checks: &mut Vec<CheckResult>, check: &CheckResult, weight: u32, total: &mut u32, earned: &mut u32) {
    *total += weight;
    if check.pass {
        *earned += weight;
    }
    checks.push(check.clone());
}

// ─── INSUNITS → inches conversion ──────────────────────────────────────────

/// Convert a length in drawing units to inches, based on the INSUNITS header.
fn units_to_inches(insunits: i64) -> f64 {
    match insunits {
        0 => 1.0,  // unitless — assume inches
        1 => 1.0,  // inches
        2 => 12.0, // feet
        3 => 0.03937008, // millimetres
        4 => 0.3937008,  // centimetres
        5 => 39.37008,   // metres
        6 => 0.001,      // microinches
        7 => 0.001,      // mils (thousandths of inch)
        8 => 36.0,       // yards
        _ => 1.0,        // unknown: assume inches
    }
}

/// Whether a pair of dimensions (in inches) look like a standard architectural sheet
/// (within 2× of 24-48" × 18-36").
fn is_sheet_size_inches(w: f64, h: f64) -> bool {
    let (lo, hi) = if w < h { (w, h) } else { (h, w) };
    lo >= PAPER_HEIGHT_MIN_IN && lo <= PAPER_HEIGHT_MAX_IN && hi >= PAPER_WIDTH_MIN_IN && hi <= PAPER_WIDTH_MAX_IN
}

/// Whether a pair of dimensions (in PDF points) look like a standard sheet.
fn is_sheet_size_points(w: f64, h: f64) -> bool {
    let (lo, hi) = if w < h { (w, h) } else { (h, w) };
    lo >= PAPER_HEIGHT_MIN_PT && lo <= PAPER_HEIGHT_MAX_PT && hi >= PAPER_WIDTH_MIN_PT && hi <= PAPER_WIDTH_MAX_PT
}

// ─── Scale note parsing ────────────────────────────────────────────────────

/// Parse a fraction like `1/8` or a whole number.
fn parse_fraction(s: &str) -> Option<f64> {
    if let Some((n, d)) = s.split_once('/') {
        let n: f64 = n.trim().parse().ok()?;
        let d: f64 = d.trim().parse().ok()?;
        if d.abs() > 1e-12 { Some(n / d) } else { None }
    } else {
        s.trim().parse::<f64>().ok()
    }
}

/// Parse the right-hand side of a scale note in feet: `1'-0"`, `20'`, `6"`.
fn parse_feet(s: &str) -> Option<f64> {
    let s = s.trim();
    let (ft, rest) = match s.split_once('\'') {
        Some((f, r)) => (f.trim().parse::<f64>().ok()?, r.trim().trim_start_matches('-').trim()),
        None => (0.0, s),
    };
    let inches_str = rest.trim_end_matches('"').trim();
    let inches = if inches_str.is_empty() { 0.0 } else { parse_fraction(inches_str)? };
    Some(ft + inches / 12.0)
}

/// Parse a scale string like `1/8" = 1'-0"` → paper inches per building foot.
fn parse_scale(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() == 2 {
        let left = parts[0].trim().trim_end_matches('"').trim();
        if let Some(val) = parse_fraction(left) {
            let feet = parse_feet(parts[1]).filter(|f| *f > 1e-9).unwrap_or(1.0);
            return Some(val / feet).filter(|v| v.is_finite() && *v > 0.0);
        }
    }
    None
}

/// Detect the scale note text and its parsed value from text entities on the drawing.
fn detect_scale_note(d: &Drawing) -> Option<(String, f64)> {
    // First try TITLE/TEXT layers for "SCALE: ..." pattern.
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
        let upper = val.trim().to_ascii_uppercase();
        if let Some(rest) = upper.strip_prefix("SCALE:").or_else(|| upper.strip_prefix("SCALE :")) {
            let rest = rest.trim();
            if let Some(scale) = parse_scale(rest) {
                return Some((rest.to_string(), scale));
            }
        }
    }
    // Fallback: most common scale note anywhere.
    let mut seen: std::collections::BTreeMap<String, (f64, usize)> = std::collections::BTreeMap::new();
    for e in d.model.iter() {
        let val = match &e.kind {
            EntityKind::Text(t) => t.value.as_str(),
            EntityKind::MText(t) => t.contents.as_str(),
            _ => continue,
        };
        let up = val.trim().to_ascii_uppercase();
        let note = up.strip_prefix("SCALE").map_or(up.as_str(), |r| r.trim_start_matches([':', ' '])).trim();
        if !note.contains('=') || !(note.contains('\'') || note.contains('"')) || note.len() > 24 {
            continue;
        }
        if let Some(k) = parse_scale(note) {
            seen.entry(note.to_string()).or_insert((k, 0)).1 += 1;
        }
    }
    seen.into_iter().max_by(|a, b| a.1 .1.cmp(&b.1 .1).then(b.0.cmp(&a.0))).map(|(t, (k, _))| (t, k))
}

// ─── DO NOT SCALE / discipline detection ────────────────────────────────────

/// Known patterns for "do not scale" general notes on MEP/E sheets.
const DO_NOT_SCALE_PATTERNS: &[&str] = &[
    "DO NOT SCALE",
    "DO NOT SCALE DRAWING",
    "DO NOT SCALE DRAWINGS",
    "DO NOT SCALE THIS DRAWING",
    "NOT TO SCALE",
    "REFER TO ARCHITECTURAL DRAWINGS FOR DIMENSIONS",
    "REFER TO ARCHITECTURAL",
    "ELECTRICAL LOCATIONS ARE APPROXIMATE",
];

/// Detect "DO NOT SCALE" or "REFER TO ARCHITECTURAL" notes in general notes.
fn detect_do_not_scale(d: &Drawing) -> Option<String> {
    for e in d.model.iter() {
        let val = match &e.kind {
            EntityKind::Text(t) => &t.value,
            EntityKind::MText(t) => &t.contents,
            _ => continue,
        };
        let upper = val.trim().to_ascii_uppercase();
        for pattern in DO_NOT_SCALE_PATTERNS {
            if upper.contains(pattern) {
                return Some(val.trim().to_string());
            }
        }
    }
    None
}

/// Known discipline prefixes: sheet number prefix → discipline code.
const DISCIPLINE_PREFIX: &[(&str, &str)] =
    &[("FP", "FP"), ("FA", "FA"), ("M", "M"), ("P", "P"), ("S", "S"), ("A", "A"), ("G", "G"), ("E", "E"), ("C", "C"), ("T", "T")];

/// Detect sheet discipline from sheet number prefix or title keywords.
fn detect_discipline(d: &Drawing) -> Option<String> {
    // Look for a sheet number on the TITLE/BORDER layer.
    for e in d.model.iter() {
        let on_title = e.common.layer.eq_ignore_ascii_case("TITLE") || e.common.layer.eq_ignore_ascii_case("BORDER");
        if !on_title {
            continue;
        }
        let val = match &e.kind {
            EntityKind::Text(t) => &t.value,
            EntityKind::MText(t) => &t.contents,
            _ => continue,
        };
        let trimmed = val.trim().to_ascii_uppercase();
        let letters: String = trimmed.chars().take_while(char::is_ascii_alphabetic).collect();
        if letters.is_empty() {
            continue;
        }
        if let Some(next) = trimmed.chars().nth(letters.len()) {
            if next == '-' || next == '.' || next.is_ascii_digit() {
                if let Some((_, disc)) = DISCIPLINE_PREFIX.iter().find(|(p, _)| *p == letters.as_str()) {
                    return Some(disc.to_string());
                }
            }
        }
    }
    // Fallback: title keywords.
    let keywords = [
        ("ELECTRICAL", "E"), ("MECHANICAL", "M"), ("PLUMBING", "P"),
        ("FIRE PROTECTION", "FP"), ("STRUCTURAL", "S"), ("ARCHITECTURAL", "A"),
    ];
    for e in d.model.iter() {
        let val = match &e.kind {
            EntityKind::Text(t) => &t.value,
            EntityKind::MText(t) => &t.contents,
            _ => continue,
        };
        let upper = val.to_ascii_uppercase();
        for &(kw, disc) in &keywords {
            if upper.contains(kw) {
                return Some(disc.to_string());
            }
        }
    }
    None
}

// ─── SCALE check ────────────────────────────────────────────────────────────

/// SCALE: compare model extents against the sheet's paper size AND scale note.
/// With a "DO NOT SCALE" note this becomes a REGISTRATION question (cannot be judged
/// without an architectural reference sheet).
fn check_scale(d: &Drawing, do_not_scale: Option<&str>) -> CheckResult {
    let ext = d.extents(&Space::Model);
    let (w, h) = if ext.is_empty() {
        (0.0, 0.0)
    } else {
        ((ext.max.x - ext.min.x).abs(), (ext.max.y - ext.min.y).abs())
    };

    // Empty drawing: pass.
    if w < 1e-9 && h < 1e-9 {
        return CheckResult {
            id: "SCALE".into(),
            pass: true,
            expected: "model at real-world units".into(),
            found: "empty drawing".into(),
            severity: 0,
            hint: String::new(),
            hard_fail: true,
            evidence: None,
        };
    }

    let insunits = d.header.i64("INSUNITS", 0);
    let scale_note = detect_scale_note(d);

    // ADDENDUM: DO NOT SCALE → REGISTRATION check (cannot judge without arch reference).
    if let Some(note) = do_not_scale {
        return CheckResult {
            id: "SCALE".into(),
            pass: true, // Cannot fail without an architectural reference to register against
            expected: "sheet registered to architectural plan (do-not-scale note found)".into(),
            found: format!(
                "do-not-scale note: \"{}\"; extents {w:.1} x {h:.1}, INSUNITS={insunits}",
                truncate(note, 80)
            ),
            severity: 0,
            hint: String::new(),
            hard_fail: true,
            evidence: Some(json!({
                "do_not_scale": note,
                "needs_registration": true,
                "extents": [w, h],
                "insunits": insunits,
                "scale_note": scale_note.as_ref().map(|(t, _)| t.as_str()),
            })),
        };
    }

    let to_in = units_to_inches(insunits);
    let w_in = w * to_in;
    let h_in = h * to_in;

    // Check 1: extents ≈ paper size in inches or PDF points.
    let paper_match_in = is_sheet_size_inches(w_in, h_in);
    // Only check PDF-point interpretation when units are unknown (INSUNITS=0): a drawing with
    // known units already had its extents converted to inches above.
    let paper_match_pt = insunits == 0 && is_sheet_size_points(w, h);

    // Check 2: scale note exists and model size disagrees > 10%.
    let scale_disagree = if let Some((_, scale_val)) = &scale_note {
        // scale_val = paper inches per building foot.
        // Standard paper long dimension: 36" (ARCH D).
        // Expected model in building feet: 36 / scale_val.
        // In inches: expected_ft * 12.
        let expected_ft_min = 24.0 / scale_val; // smallest paper
        let expected_ft_max = 48.0 / scale_val; // largest paper
        let model_max_ft = w_in.max(h_in) / 12.0;

        // Disagree if model is way too small (< 90% of minimum expected)
        // or way too large (> 110% of maximum expected).
        model_max_ft < expected_ft_min * (1.0 - SCALE_DISAGREE_FRACTION)
            || model_max_ft > expected_ft_max * (1.0 + SCALE_DISAGREE_FRACTION)
    } else {
        false
    };

    let fail_paper = paper_match_in || paper_match_pt;
    let fail = fail_paper || scale_disagree;
    let pass = !fail;

    let mut found_parts = vec![format!("extents {w:.1} x {h:.1} ({w_in:.1} x {h_in:.1} in)")];
    found_parts.push(format!("INSUNITS={insunits}"));
    if let Some((ref text, _)) = scale_note {
        found_parts.push(format!("scale note \"{text}\""));
    }
    if fail_paper {
        found_parts.push("extents match paper size".into());
    }
    if scale_disagree {
        found_parts.push("model size disagrees with scale note >10%".into());
    }

    CheckResult {
        id: "SCALE".into(),
        pass,
        expected: "model at real-world units (extents larger than sheet size, consistent with scale note)".into(),
        found: found_parts.join("; "),
        severity: if pass { 0 } else { 3 },
        hint: if pass {
            String::new()
        } else {
            "the drawing appears to be at sheet scale (not real-world units); apply the sheet's scale note".into()
        },
        hard_fail: true,
        evidence: Some(json!({
            "extents": [w, h],
            "extents_in": [w_in, h_in],
            "insunits": insunits,
            "paper_match_inches": paper_match_in,
            "paper_match_points": paper_match_pt,
            "scale_note": scale_note.as_ref().map(|(t, v)| json!({"text": t, "value": v})),
            "scale_disagree": scale_disagree,
        })),
    }
}

// ─── LAYERS check ───────────────────────────────────────────────────────────

/// LAYERS: content should be split onto multiple layers.
fn check_layers(d: &Drawing) -> CheckResult {
    let total = d.model.len();
    if total == 0 {
        return CheckResult {
            id: "LAYERS".into(),
            pass: true,
            expected: "content on multiple layers".into(),
            found: "no entities".into(),
            severity: 0,
            hint: String::new(),
            hard_fail: false,
            evidence: None,
        };
    }
    let mut layer_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for e in d.model.iter() {
        *layer_counts.entry(&e.common.layer).or_insert(0) += 1;
    }
    let max_on_one = layer_counts.values().copied().max().unwrap_or(0);
    let fraction = max_on_one as f64 / total as f64;
    let pass = fraction <= LAYER_CONCENTRATION_FAIL;
    let top_layer = layer_counts.iter().max_by_key(|(_, c)| *c).map(|(n, _)| *n).unwrap_or("0");
    CheckResult {
        id: "LAYERS".into(),
        pass,
        expected: format!("no single layer has >{:.0}% of entities", LAYER_CONCENTRATION_FAIL * 100.0),
        found: format!("{max_on_one}/{total} ({:.0}%) on layer \"{top_layer}\"", fraction * 100.0),
        severity: if pass { 0 } else { 2 },
        hint: if pass {
            String::new()
        } else {
            format!("{:.0}% of entities are on one layer; content should be split by type (walls, text, devices, etc.)", fraction * 100.0)
        },
        hard_fail: false,
        evidence: Some(json!({
            "total_entities": total,
            "max_on_one_layer": max_on_one,
            "top_layer": top_layer,
            "layer_count": layer_counts.len(),
        })),
    }
}

// ─── TEXT check ─────────────────────────────────────────────────────────────

/// A short line segment extracted for stroke-glyph detection.
struct StrokeSegment {
    mid_x: f64,
    mid_y: f64,
}

/// Detect lettering stroke clusters: groups of short line/polyline segments that form
/// glyph-like shapes (stroked SHX lettering that was not recovered as text).
fn count_stroke_glyph_clusters(d: &Drawing) -> usize {
    // Collect midpoints of short line segments.
    let mut segments: Vec<StrokeSegment> = Vec::new();

    for e in d.model.iter() {
        match &e.kind {
            EntityKind::Line(l) => {
                let len = ((l.b.x - l.a.x).powi(2) + (l.b.y - l.a.y).powi(2)).sqrt();
                if len > 1e-9 && len <= STROKE_GLYPH_MAX_LEN {
                    segments.push(StrokeSegment {
                        mid_x: (l.a.x + l.b.x) * 0.5,
                        mid_y: (l.a.y + l.b.y) * 0.5,
                    });
                }
            }
            EntityKind::LwPolyline(pl) => {
                for w in pl.vertices.windows(2) {
                    let dx = w[1].p.x - w[0].p.x;
                    let dy = w[1].p.y - w[0].p.y;
                    let len = (dx * dx + dy * dy).sqrt();
                    if len > 1e-9 && len <= STROKE_GLYPH_MAX_LEN {
                        segments.push(StrokeSegment {
                            mid_x: (w[0].p.x + w[1].p.x) * 0.5,
                            mid_y: (w[0].p.y + w[1].p.y) * 0.5,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    if segments.len() < STROKE_CLUSTER_MIN_SEGS {
        return 0;
    }

    // Grid-based spatial clustering: bucket segments into cells of STROKE_CLUSTER_RADIUS.
    let inv_r = 1.0 / STROKE_CLUSTER_RADIUS.max(1e-9);
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
    for (i, seg) in segments.iter().enumerate() {
        let gx = (seg.mid_x * inv_r).floor() as i64;
        let gy = (seg.mid_y * inv_r).floor() as i64;
        grid.entry((gx, gy)).or_default().push(i);
    }

    // Each cell with enough segments is a potential glyph cluster.
    // Filter by bounding-box height to reject long structural lines.
    let mut cluster_count = 0usize;
    for indices in grid.values() {
        if indices.len() < STROKE_CLUSTER_MIN_SEGS {
            continue;
        }
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for &i in indices {
            let y = segments[i].mid_y;
            if y < min_y {
                min_y = y;
            }
            if y > max_y {
                max_y = y;
            }
        }
        let height = max_y - min_y;
        if height <= STROKE_CLUSTER_MAX_HEIGHT {
            cluster_count += 1;
        }
    }

    cluster_count
}

/// TEXT: compare real text count with detected lettering stroke clusters.
fn check_text(d: &Drawing) -> CheckResult {
    let total = d.model.len();
    let text_count = d.model.iter().filter(|e| matches!(&e.kind, EntityKind::Text(_) | EntityKind::MText(_))).count();

    if total < TEXT_CHECK_MIN_ENTITIES {
        return CheckResult {
            id: "TEXT".into(),
            pass: true,
            expected: "text entities present".into(),
            found: format!("{text_count} text entities in {total} total (small drawing, exempt)"),
            severity: 0,
            hint: String::new(),
            hard_fail: false,
            evidence: Some(json!({"text_count": text_count, "total": total, "exempt": true})),
        };
    }

    let glyph_clusters = count_stroke_glyph_clusters(d);
    let detected_lettering = text_count + glyph_clusters;

    // If there are no detected glyph clusters, the old flat-threshold is irrelevant —
    // there is nothing to recover. Pass.
    if detected_lettering == 0 {
        return CheckResult {
            id: "TEXT".into(),
            pass: true,
            expected: "text recovered from lettering".into(),
            found: "no text or lettering-glyph clusters detected".into(),
            severity: 0,
            hint: String::new(),
            hard_fail: false,
            evidence: Some(json!({"text_count": 0, "glyph_clusters": 0, "total": total})),
        };
    }

    let recovery_ratio = text_count as f64 / detected_lettering as f64;
    let pass = recovery_ratio >= TEXT_RECOVERY_MIN;

    CheckResult {
        id: "TEXT".into(),
        pass,
        expected: format!("recovered text >= {:.0}% of detected lettering", TEXT_RECOVERY_MIN * 100.0),
        found: format!(
            "{text_count} text entities, {glyph_clusters} stroke-glyph clusters ({detected_lettering} total lettering); recovery {:.0}%",
            recovery_ratio * 100.0
        ),
        severity: if pass { 0 } else { 2 },
        hint: if pass {
            String::new()
        } else {
            "many lettering-glyph clusters detected but few recovered as text; stroked/SHX lettering may not have been converted".into()
        },
        hard_fail: false,
        evidence: Some(json!({
            "text_count": text_count,
            "glyph_clusters": glyph_clusters,
            "detected_lettering": detected_lettering,
            "recovery_ratio": recovery_ratio,
        })),
    }
}

// ─── FILLS check ────────────────────────────────────────────────────────────

/// FILLS: no excessive opaque wipeouts or raster pieces covering content. **Hard-fail.**
fn check_fills(d: &Drawing) -> CheckResult {
    let total = d.model.len();
    if total == 0 {
        return CheckResult {
            id: "FILLS".into(),
            pass: true,
            expected: "few wipeouts/rasters".into(),
            found: "no entities".into(),
            severity: 0,
            hint: String::new(),
            hard_fail: true,
            evidence: None,
        };
    }
    let fill_count = d.model.iter().filter(|e| matches!(&e.kind, EntityKind::Wipeout(_) | EntityKind::Image(_))).count();
    let fraction = fill_count as f64 / total as f64;
    let pass = fraction <= FILLS_FRACTION_FAIL;
    CheckResult {
        id: "FILLS".into(),
        pass,
        expected: format!("wipeouts+rasters <{:.0}% of entities", FILLS_FRACTION_FAIL * 100.0),
        found: format!("{fill_count} fills of {total} ({:.1}%)", fraction * 100.0),
        severity: if pass { 0 } else { 2 },
        hint: if pass {
            String::new()
        } else {
            "too many opaque fills/rasters; they may be covering text or linework that was visible in the PDF".into()
        },
        hard_fail: true,
        evidence: Some(json!({
            "fill_count": fill_count,
            "total": total,
            "fraction": fraction,
        })),
    }
}

// ─── SYMBOLS check ──────────────────────────────────────────────────────────

/// SYMBOLS: repeated symbols should become blocks. **Hard-fail** when repeated clusters exist.
fn check_symbols(d: &Drawing) -> CheckResult {
    let total = d.model.len();
    if total < SYMBOLS_CHECK_MIN_ENTITIES {
        return CheckResult {
            id: "SYMBOLS".into(),
            pass: true,
            expected: "repeated symbols as blocks".into(),
            found: format!("{total} entities (small drawing, exempt)"),
            severity: 0,
            hint: String::new(),
            hard_fail: true,
            evidence: None,
        };
    }
    let block_count = d.model.iter().filter(|e| matches!(&e.kind, EntityKind::Insert(_))).count();
    let pass = block_count > 0;
    CheckResult {
        id: "SYMBOLS".into(),
        pass,
        expected: "at least 1 block reference on a sheet with repeated symbol clusters".into(),
        found: format!("{block_count} block references in {total} entities"),
        severity: if pass { 0 } else { 1 },
        hint: if pass {
            String::new()
        } else {
            "no block references found; repeated symbols should be converted to blocks for BIM recognition".into()
        },
        hard_fail: true,
        evidence: Some(json!({
            "block_count": block_count,
            "total": total,
        })),
    }
}

// ─── GRID check ─────────────────────────────────────────────────────────────

/// GRID: grid bubbles and lines found.
fn check_grid(d: &Drawing) -> CheckResult {
    let circles: Vec<(f64, f64, f64)> = d
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Circle(c) => Some((c.center.x, c.center.y, c.radius)),
            _ => None,
        })
        .collect();

    let has_grid_candidates = if circles.len() >= 4 {
        let mut radii: Vec<f64> = circles.iter().map(|c| c.2).collect();
        radii.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut best_count = 0usize;
        let mut run = 1usize;
        for w in radii.windows(2) {
            if (w[1] - w[0]).abs() < w[0] * 0.1 + 0.01 {
                run += 1;
            } else {
                if run > best_count {
                    best_count = run;
                }
                run = 1;
            }
        }
        if run > best_count {
            best_count = run;
        }
        best_count >= 4
    } else {
        false
    };

    let grid_label_count = d
        .model
        .iter()
        .filter(|e| match &e.kind {
            EntityKind::Text(t) => {
                let v = t.value.trim();
                v.len() <= 2 && (v.chars().all(|c| c.is_ascii_uppercase()) || v.chars().all(|c| c.is_ascii_digit()))
            }
            _ => false,
        })
        .count();

    let pass = has_grid_candidates || grid_label_count >= 4;
    CheckResult {
        id: "GRID".into(),
        pass,
        expected: "grid bubbles and labels found".into(),
        found: format!("{} candidate grid circles, {} grid-like labels", circles.len(), grid_label_count),
        severity: if pass { 0 } else { 1 },
        hint: if pass { String::new() } else { "no structural grid detected; grid lines help BIM orientation".into() },
        hard_fail: false,
        evidence: Some(json!({
            "circle_count": circles.len(),
            "grid_label_count": grid_label_count,
            "has_grid_candidates": has_grid_candidates,
        })),
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Truncate a string to at most `max` characters.
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let end = s.char_indices().nth(max).map_or(s.len(), |(i, _)| i);
        s.get(..end).unwrap_or(s)
    }
}

// ─── Storage and query ──────────────────────────────────────────────────────

/// Store the check result into the drawing's `import_check` field.
pub fn store_result(d: &mut Drawing, result: &ImportCheckResult) {
    d.import_check = Some(result.to_json());
}

/// Read the stored check result from a drawing.
pub fn stored_result(d: &Drawing) -> Option<ImportCheckResult> {
    d.import_check.as_ref().and_then(|s| ImportCheckResult::from_json(s))
}

/// Returns `true` if the drawing's import check passes (or if no check has been run, which
/// means it was not imported from a PDF and is assumed good).
pub fn passes(d: &Drawing) -> bool {
    match stored_result(d) {
        Some(r) => r.pass,
        None => true, // no check = not a PDF import, assumed good
    }
}

/// Returns the list of failed check ids and hints, for error messages.
pub fn failed_checks(d: &Drawing) -> Vec<(String, String)> {
    match stored_result(d) {
        Some(r) => r.checks.iter().filter(|c| !c.pass).map(|c| (c.id.to_string(), c.hint.clone())).collect(),
        None => Vec::new(),
    }
}

// ─── Command spec ────────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("import.check", "Check Import Accuracy", run_check)
        .params("{path? | current} → {pass, score: 0-100, hard_fails, checks: [{id, pass, expected, found, severity, hint, hard_fail, evidence}]}")
        .enabled(always)
        .noundo()]
}

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(path) = str_param(p, "path") {
        let hooks = super::file::io().ok_or_else(|| EngineError::Other("file formats are not available in this build".into()))?;
        let bytes = std::fs::read(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        let d = (hooks.read)(&bytes, name).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        let result = check_drawing(&d);
        Ok(json!(result))
    } else {
        let d = s.doc()?;
        let result = check_drawing(d);
        let d = s.doc_mut()?;
        store_result(d, &result);

        // Echo result with hard-fail information.
        let fail_info = if result.pass {
            String::new()
        } else {
            let fails: Vec<&str> = result.checks.iter().filter(|c| !c.pass).map(|c| c.id.as_str()).collect();
            let hf = if result.hard_fails.is_empty() {
                String::new()
            } else {
                format!(" [hard-fail: {}]", result.hard_fails.join(", "))
            };
            format!("; failed: {}{hf}", fails.join(", "))
        };
        s.echo(format!(
            "Import check: {} (score {}/100){}",
            if result.pass { "PASS" } else { "FAIL" },
            result.score,
            fail_info,
        ));

        // Notify UI with per-check details for zoom-to panel.
        for c in &result.checks {
            if !c.pass {
                s.echo(format!("  {} FAIL: {} ({})", c.id, c.found, c.hint));
            }
        }

        Ok(json!(result))
    }
}
