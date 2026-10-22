//! Casework reader: recognises cabinets, counters, vanities, appliances and backsplashes from
//! architectural floor plan DXF sheets (A-10x). Symbols appear as rectangles on the `CASEWORK`
//! layer; tags and height notes as nearby text. The reader clusters the geometry, classifies
//! each item by its hatch pattern and tag prefix, and converts positions from model inches to
//! building feet using the sheet's alignment.
//!
//! Output feeds device placement zones (counter receptacles over the backsplash, TV outlets,
//! sink-adjacent GFCIs) and in-wall clash detection (cabinet backs, sink drains, dishwasher and
//! fridge boxes).

use crate::score;
use crate::truth::{Sheet, Truth};
use cadaddy_doc::{Drawing, EntityKind};

/// A casework item detected on the sheet (model-inch coordinates).
#[derive(Clone, Debug)]
pub struct DetectedCasework {
    /// Bounding-box minimum in model inches.
    pub min_in: [f64; 2],
    /// Bounding-box maximum in model inches.
    pub max_in: [f64; 2],
    /// Classified kind: `base_cabinet`, `upper_cabinet`, `counter`, `vanity`, `appliance`,
    /// `backsplash`.
    pub kind: String,
    /// Tag text found on or near the item (e.g. `BC-1`, `V-1`, `CT-1`).
    pub tag: String,
    /// Height in inches AFF parsed from a nearby note, or 0 if none found.
    pub height_in: f64,
}

/// Read casework from a DXF drawing (an architectural floor plan sheet).
///
/// Returns all detected casework items in model-inch coordinates.
pub fn read_casework(d: &Drawing) -> Vec<DetectedCasework> {
    // 1. Collect all CASEWORK-layer closed rectangles.
    let rects = collect_casework_rects(d);
    // 2. Collect all lines on CASEWORK layer (hatch lines, X marks).
    let lines = collect_casework_lines(d);
    // 3. Collect all text (CASEWORK + TEXT layers).
    let texts = collect_text(d);

    let mut items = Vec::new();
    for (min, max) in &rects {
        let cx = (min[0] + max[0]) / 2.0;
        let cy = (min[1] + max[1]) / 2.0;
        let w = max[0] - min[0];
        let h = max[1] - min[1];

        // Skip tiny or huge rectangles (noise or the background).
        if w < 1.0 || h < 1.0 || w > 2000.0 || h > 2000.0 {
            continue;
        }

        // Classify by hatch pattern inside the rectangle.
        let has_diag = lines.iter().any(|&(a, b)| inside_rect(a, *min, *max, 2.0) && inside_rect(b, *min, *max, 2.0) && !is_axis_aligned(a, b));
        let has_x_mark = {
            let cross_lines: Vec<_> = lines
                .iter()
                .filter(|&&(a, b)| inside_rect(a, *min, *max, 2.0) && inside_rect(b, *min, *max, 2.0) && !is_axis_aligned(a, b))
                .collect();
            // An X has exactly 2 crossing diagonal lines spanning most of the rect.
            cross_lines.len() == 2
                && cross_lines.iter().all(|&&(a, b)| {
                    let dx = (b[0] - a[0]).abs();
                    let dy = (b[1] - a[1]).abs();
                    dx > w * 0.6 && dy > h * 0.6
                })
        };
        let has_inner_rect = rects.iter().any(|(imin, imax)| {
            imin[0] > min[0] + 0.5
                && imin[1] > min[1] + 0.5
                && imax[0] < max[0] - 0.5
                && imax[1] < max[1] - 0.5
                && (imax[0] - imin[0]) > w * 0.3
                && (imax[1] - imin[1]) > h * 0.3
        });

        // Find nearby tag text (on CASEWORK layer, short, within search radius).
        let search_r = 120.0; // model inches
        let tag = texts
            .iter()
            .filter(|(t, p, layer)| {
                let d = ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt();
                d < search_r && layer.eq_ignore_ascii_case("CASEWORK") && t.len() <= 8 && !t.contains("AFF") && !t.contains('"')
            })
            .min_by(|(_, a, _), (_, b, _)| {
                let da = (a[0] - cx).powi(2) + (a[1] - cy).powi(2);
                let db = (b[0] - cx).powi(2) + (b[1] - cy).powi(2);
                da.total_cmp(&db)
            })
            .map(|(t, _, _)| t.trim().to_string())
            .unwrap_or_default();

        // Parse height from nearby "NNN\" AFF" or "NNN\" text on TEXT layer.
        let height_in = texts
            .iter()
            .filter(|(_, p, _)| {
                let d = ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt();
                d < search_r
            })
            .filter_map(|(t, _, _)| parse_height_note(t))
            .next()
            .unwrap_or(0.0);

        // Classify kind from pattern and tag prefix.
        let kind = if has_x_mark {
            "appliance"
        } else if has_inner_rect && !has_diag {
            "upper_cabinet"
        } else if has_diag {
            classify_by_tag(&tag, "base_cabinet")
        } else {
            classify_by_tag(&tag, "counter")
        };

        items.push(DetectedCasework { min_in: *min, max_in: *max, kind: kind.to_string(), tag, height_in });
    }
    items
}

/// Convert detected casework to found items for scoring.
pub fn to_found(items: &[DetectedCasework], sh: &Sheet) -> Vec<score::FoundCaseworkItem> {
    let level = sh.level.clone().unwrap_or_default();
    items
        .iter()
        .map(|it| {
            let min_ft = score::building_from_model(sh, it.min_in);
            let max_ft = score::building_from_model(sh, it.max_in);
            let (lo, hi) = ([min_ft[0].min(max_ft[0]), min_ft[1].min(max_ft[1])], [min_ft[0].max(max_ft[0]), min_ft[1].max(max_ft[1])]);
            score::FoundCaseworkItem {
                level: level.clone(),
                kind: it.kind.clone(),
                tag: it.tag.clone(),
                min_ft: lo,
                max_ft: hi,
                height_in: it.height_in,
            }
        })
        .collect()
}

/// Read casework from a drawing and score against truth.
pub fn read_and_score(d: &Drawing, t: &Truth, sh: &Sheet) -> score::CaseworkScore {
    let items = read_casework(d);
    let found = to_found(&items, sh);
    let level = sh.level.clone().unwrap_or_default();
    score::score_casework(t, &level, &found, 3.0)
}

// ---- Internal helpers ------------------------------------------------------------------

/// Collect closed rectangles from the CASEWORK layer.
fn collect_casework_rects(d: &Drawing) -> Vec<([f64; 2], [f64; 2])> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("CASEWORK") {
            continue;
        }
        if let EntityKind::LwPolyline(p) = &e.kind {
            if !p.closed || p.vertices.len() != 4 {
                continue;
            }
            let pts: Vec<[f64; 2]> = p.vertices.iter().map(|v| [v.p.x, v.p.y]).collect();
            if pts.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
                continue;
            }
            let min = [pts.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), pts.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)];
            let max = [pts.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max), pts.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max)];
            out.push((min, max));
        }
    }
    out
}

/// Collect line segments from the CASEWORK layer (hatch lines, X marks).
fn collect_casework_lines(d: &Drawing) -> Vec<([f64; 2], [f64; 2])> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("CASEWORK") {
            continue;
        }
        if let EntityKind::Line(l) = &e.kind {
            let a = [l.a.x, l.a.y];
            let b = [l.b.x, l.b.y];
            if a[0].is_finite() && a[1].is_finite() && b[0].is_finite() && b[1].is_finite() {
                out.push((a, b));
            }
        }
    }
    out
}

/// Collect text from CASEWORK and TEXT layers: (content, position, layer).
fn collect_text(d: &Drawing) -> Vec<(String, [f64; 2], String)> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        let layer = &e.common.layer;
        if !layer.eq_ignore_ascii_case("CASEWORK") && !layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        match &e.kind {
            EntityKind::Text(t) => {
                let pt = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                out.push((t.value.clone(), pt, layer.to_string()));
            }
            EntityKind::MText(t) => {
                out.push((t.contents.clone(), [t.insert.x, t.insert.y], layer.to_string()));
            }
            _ => {}
        }
    }
    out
}

/// Is a point inside a rectangle (with tolerance)?
fn inside_rect(p: [f64; 2], min: [f64; 2], max: [f64; 2], tol: f64) -> bool {
    p[0] >= min[0] - tol && p[0] <= max[0] + tol && p[1] >= min[1] - tol && p[1] <= max[1] + tol
}

/// Is a line segment roughly axis-aligned?
fn is_axis_aligned(a: [f64; 2], b: [f64; 2]) -> bool {
    let dx = (b[0] - a[0]).abs();
    let dy = (b[1] - a[1]).abs();
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return true;
    }
    dx / len < 0.05 || dy / len < 0.05
}

/// Parse a height note like "34\" AFF" or "34\"" → height in inches.
fn parse_height_note(s: &str) -> Option<f64> {
    let s = s.trim().to_uppercase();
    // Match patterns: "34\" AFF", "34\"", "34 AFF"
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if digits.is_empty() {
        return None;
    }
    let val: f64 = digits.parse().ok()?;
    if val > 0.0 && val <= 120.0 && (s.contains("AFF") || s.contains('"') || s.contains('\"')) { Some(val) } else { None }
}

/// Classify kind by tag prefix when the hatch pattern alone is ambiguous.
fn classify_by_tag<'a>(tag: &str, default: &'a str) -> &'a str {
    let t = tag.to_uppercase();
    if t.starts_with("BC") || t.starts_with("CR") {
        "base_cabinet"
    } else if t.starts_with("UC") {
        "upper_cabinet"
    } else if t.starts_with("CT") || t.starts_with("SK") {
        "counter"
    } else if t.starts_with('V') {
        "vanity"
    } else if t.starts_with("RF") || t.starts_with("MW") || t.starts_with("DW") || t.starts_with("AP") {
        "appliance"
    } else if t.starts_with("BS") {
        "backsplash"
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_height_note_basic() {
        assert_eq!(parse_height_note("34\" AFF"), Some(34.0));
        assert_eq!(parse_height_note("36\""), Some(36.0));
        assert_eq!(parse_height_note("HELLO"), None);
        assert_eq!(parse_height_note(""), None);
    }

    #[test]
    fn classify_by_tag_prefixes() {
        assert_eq!(classify_by_tag("BC-1", "counter"), "base_cabinet");
        assert_eq!(classify_by_tag("UC-1", "counter"), "upper_cabinet");
        assert_eq!(classify_by_tag("V-1", "counter"), "vanity");
        assert_eq!(classify_by_tag("CT-1", "base_cabinet"), "counter");
        assert_eq!(classify_by_tag("RF-1", "counter"), "appliance");
        assert_eq!(classify_by_tag("BS-1", "counter"), "backsplash");
        assert_eq!(classify_by_tag("UNKNOWN", "counter"), "counter");
    }

    #[test]
    fn inside_rect_works() {
        assert!(inside_rect([5.0, 5.0], [0.0, 0.0], [10.0, 10.0], 0.0));
        assert!(!inside_rect([15.0, 5.0], [0.0, 0.0], [10.0, 10.0], 0.0));
        assert!(inside_rect([10.5, 5.0], [0.0, 0.0], [10.0, 10.0], 1.0));
    }

    #[test]
    fn is_axis_aligned_works() {
        assert!(is_axis_aligned([0.0, 0.0], [10.0, 0.0]));
        assert!(is_axis_aligned([0.0, 0.0], [0.0, 10.0]));
        assert!(!is_axis_aligned([0.0, 0.0], [10.0, 10.0]));
    }
}
