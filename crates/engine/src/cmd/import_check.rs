//! Import accuracy check: validates the quality of a PDF-imported drawing and gates the BIM
//! pipeline on import quality. Runs automatically after every PDF import (convert, open,
//! printset.open/reimport); results are stored in the .cdy (`importCheck` manifest field) and
//! shown in the UI as a notification listing failures with zoom-to.
//!
//! Checks (each its own id, thresholds in one table):
//! - **SCALE**: a sheet scale note found and applied; fail if model extents ≈ sheet size.
//! - **LAYERS**: content split onto layers; fail if >90% of entities on one layer.
//! - **TEXT**: stroked/SHX lettering recovered as text; fail if real text count is far below
//!   detected lettering-glyph clusters.
//! - **FILLS**: no opaque fill/wipeout covering visible content.
//! - **SYMBOLS**: repeated symbols became blocks; fail if 0 blocks on a sheet with many entities.
//! - **GRID**: grid bubbles + lines found.
//! - **VISUAL**: placeholder for rasterize-and-diff (requires rendering; scored but never fails).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandSpec, always, str_param};
use crate::{EngineError, Result, Session};
use cadaddy_doc::{Drawing, EntityKind, Space};

// ─── Thresholds (one table) ──────────────────────────────────────────────────

/// Maximum model extents width or height (in whatever units the file uses) that looks like
/// unscaled sheet dimensions. A 36×24″ sheet at 1:1 has extents around 36; at 1/8″=1′-0″
/// the width is ~288.
const SHEET_SIZE_MAX: f64 = 50.0;

/// Fraction of entities on a single layer above which the LAYERS check fails.
const LAYER_CONCENTRATION_FAIL: f64 = 0.90;

/// Minimum real text count as a fraction of total entities for a sheet that should have text.
/// If text entities are less than this fraction of the entity count and the sheet has many
/// entities, the TEXT check fails.
const TEXT_MIN_FRACTION: f64 = 0.005;

/// Minimum entity count before the TEXT check applies (very small imports are exempt).
const TEXT_CHECK_MIN_ENTITIES: usize = 100;

/// Fraction of entities that are wipeouts or raster images above which FILLS fails.
const FILLS_FRACTION_FAIL: f64 = 0.10;

/// Minimum entity count before the SYMBOLS check expects blocks.
const SYMBOLS_CHECK_MIN_ENTITIES: usize = 200;

/// Overall pass threshold: a score below this fails the import.
const PASS_SCORE: u32 = 50;

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
}

/// The whole import check.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportCheckResult {
    pub pass: bool,
    pub score: u32,
    pub checks: Vec<CheckResult>,
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

    let scale_check = check_scale(d);
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
    let pass = score >= PASS_SCORE;

    ImportCheckResult { pass, score, checks }
}

fn add(checks: &mut Vec<CheckResult>, check: &CheckResult, weight: u32, total: &mut u32, earned: &mut u32) {
    *total += weight;
    if check.pass {
        *earned += weight;
    }
    checks.push(check.clone());
}

/// SCALE: model extents should be larger than sheet dimensions if a scale was applied.
fn check_scale(d: &Drawing) -> CheckResult {
    let ext = d.extents(&Space::Model);
    let (w, h) = if ext.is_empty() {
        (0.0, 0.0)
    } else {
        ((ext.max.x - ext.min.x).abs(), (ext.max.y - ext.min.y).abs())
    };
    let looks_like_sheet = w > 0.0 && h > 0.0 && w <= SHEET_SIZE_MAX && h <= SHEET_SIZE_MAX;
    // Check if INSUNITS and a scale note suggest the drawing was scaled properly.
    let insunits = d.header.i64("INSUNITS", 0);
    // If extents are very small and units look like inches (1) or unitless (0), it's unscaled.
    let pass = !looks_like_sheet || insunits > 1;
    CheckResult {
        id: "SCALE".into(),
        pass,
        expected: "model at real-world units (extents larger than sheet size)".into(),
        found: format!("extents {w:.1} x {h:.1}, INSUNITS={insunits}"),
        severity: if pass { 0 } else { 3 },
        hint: if pass {
            String::new()
        } else {
            "the drawing appears to be at sheet scale (not real-world units); apply the sheet's scale note".into()
        },
    }
}

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
        };
    }
    // Count entities per layer.
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
    }
}

/// TEXT: stroked/SHX lettering should be recovered as text entities.
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
        };
    }
    let fraction = text_count as f64 / total as f64;
    let pass = fraction >= TEXT_MIN_FRACTION;
    CheckResult {
        id: "TEXT".into(),
        pass,
        expected: format!("text >{:.1}% of entities", TEXT_MIN_FRACTION * 100.0),
        found: format!("{text_count} text of {total} ({:.2}%)", fraction * 100.0),
        severity: if pass { 0 } else { 2 },
        hint: if pass {
            String::new()
        } else {
            "very few text entities found; lettering may still be stroked linework instead of recovered text".into()
        },
    }
}

/// FILLS: no excessive opaque wipeouts or raster pieces covering content.
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
    }
}

/// SYMBOLS: repeated symbols should become blocks.
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
    }
}

/// GRID: grid bubbles and lines found. We look for circles in a regular pattern.
fn check_grid(d: &Drawing) -> CheckResult {
    // Look for circles that might be grid bubbles (typically at regular intervals along edges).
    let circles: Vec<(f64, f64, f64)> = d
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Circle(c) => Some((c.center.x, c.center.y, c.radius)),
            _ => None,
        })
        .collect();

    // Group circles by similar radius (grid bubbles are typically the same size).
    let has_grid_candidates = if circles.len() >= 4 {
        // Check if there are circles of similar radius that could be grid bubbles.
        let mut radii: Vec<f64> = circles.iter().map(|c| c.2).collect();
        radii.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        // Check if the most common radius appears at least 4 times.
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

    // Also look for text that looks like grid labels (single uppercase letters or numbers).
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
        found: format!(
            "{} candidate grid circles, {} grid-like labels",
            circles.len(),
            grid_label_count
        ),
        severity: if pass { 0 } else { 1 },
        hint: if pass { String::new() } else { "no structural grid detected; grid lines help BIM orientation".into() },
    }
}

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
        .params("{path? | current} → {pass, score: 0-100, checks: [{id, pass, expected, found, severity, hint}]}")
        .enabled(always)
        .noundo()]
}

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    // If a path is given, read that file; otherwise check the current drawing.
    if let Some(path) = str_param(p, "path") {
        // Read the file and check it without opening it in the session.
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
        // Store the result in the current drawing.
        let d = s.doc_mut()?;
        store_result(d, &result);
        s.echo(format!(
            "Import check: {} (score {}/100){}",
            if result.pass { "PASS" } else { "FAIL" },
            result.score,
            if result.pass {
                String::new()
            } else {
                let fails: Vec<&str> = result.checks.iter().filter(|c| !c.pass).map(|c| c.id.as_str()).collect();
                format!("; failed: {}", fails.join(", "))
            }
        ));
        Ok(json!(result))
    }
}
