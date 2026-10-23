//! Structural element recognition from framing plans: grids, columns, beams, slabs, openings.
//!
//! Reads DXF structural sheets (discipline "S") and extracts:
//! - **Columns**: filled solids on the STRUCT layer at grid intersections.
//! - **Beams**: heavy lines on the HEAVY layer with size labels on TEXT.
//! - **Slabs**: from the general notes (thickness, deck type, top of slab elevation).
//! - **Openings / shafts**: dashed rectangles on the DASHED layer with labels.
//! - **Top of steel**: from the general notes.
//!
//! Everything is converted to building coordinates (feet) using the sheet's alignment.

use crate::sheet_reader::{Alignment, RecognisedSheet, SheetMap, building_from_model};
use crate::truth::{self, GridLine};
use cadaddy_doc::{Drawing, EntityKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- Detected structures ---------------------------------------------------------------

/// A column detected on a structural sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectedColumn {
    /// Grid intersection label (`A-1`, `B-2`).
    pub grid: String,
    /// Centre position, building feet.
    pub at_ft: [f64; 2],
    /// Footprint along x and y, inches.
    pub footprint_in: [f64; 2],
}

/// A beam detected on a structural sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectedBeam {
    /// Size designation as read from the plan (`W16x31`, `W12x19`).
    pub size: String,
    /// Depth of the section, inches (from the steel shape table; 0 if unknown).
    pub depth_in: f64,
    /// Flange width, inches (from the steel shape table; 0 if unknown).
    pub flange_in: f64,
    /// Start point, building feet.
    pub from_ft: [f64; 2],
    /// End point, building feet.
    pub to_ft: [f64; 2],
    /// The grid line it is nearest to.
    pub grid: String,
}

/// A joist detected on a structural sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectedJoist {
    /// Joist designation (`18K4`, `22K7`).
    pub size: String,
    /// Nominal depth, inches.
    pub depth_in: f64,
    /// Centre-to-centre spacing, inches.
    pub spacing_in: f64,
    /// Start point, building feet.
    pub from_ft: [f64; 2],
    /// End point, building feet.
    pub to_ft: [f64; 2],
}

/// Slab information extracted from general notes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectedSlab {
    /// The level code.
    pub level: String,
    /// `composite` or `roof`.
    pub kind: String,
    /// Concrete thickness, inches.
    pub concrete_in: f64,
    /// Metal deck depth, inches.
    pub deck_in: f64,
    /// Top of slab, feet.
    pub top_ft: f64,
}

/// An opening or shaft detected on a framing plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectedOpening {
    /// Minimum corner, building feet.
    pub min_ft: [f64; 2],
    /// Maximum corner, building feet.
    pub max_ft: [f64; 2],
    /// Label text if found.
    pub label: Option<String>,
}

/// The full structural model read from all structural sheets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StructuralModel {
    pub columns: Vec<DetectedColumn>,
    /// Beams per level code.
    pub beams: BTreeMap<String, Vec<DetectedBeam>>,
    /// Joists per level code.
    pub joists: BTreeMap<String, Vec<DetectedJoist>>,
    pub slabs: Vec<DetectedSlab>,
    pub openings: BTreeMap<String, Vec<DetectedOpening>>,
    /// Top of steel per level, feet (from general notes).
    pub tos_ft: BTreeMap<String, f64>,
    /// Column size from general notes.
    pub column_size: Option<String>,
}

// ---- Steel shape table ------------------------------------------------------------------

/// Look up (depth_in, flange_in) for a steel designation.
pub fn steel_shape(designation: &str) -> (f64, f64) {
    let d = designation.to_ascii_uppercase();
    // W-shapes: the first number is the nominal depth, the second is weight per foot.
    if let Some(rest) = d.strip_prefix('W')
        && let Some((depth_s, _)) = rest.split_once('X')
        && let Ok(depth) = depth_s.trim().parse::<f64>()
    {
        let flange = match depth as u32 {
            8 => 8.0,
            10 => 8.0,
            12 => 4.0,
            14 => 5.0,
            16 => 5.5,
            18 => 6.0,
            21 => 6.5,
            24 => 7.0,
            _ => depth * 0.4,
        };
        let actual = match d.as_str() {
            "W8X31" => 8.0,
            "W10X33" => 9.7,
            "W10X49" => 10.0,
            "W12X19" => 12.2,
            "W14X22" => 13.7,
            "W16X26" => 15.7,
            "W16X31" => 15.9,
            "W18X35" => 17.7,
            "W21X44" => 20.7,
            "W24X55" => 23.6,
            _ => depth,
        };
        return (actual, flange);
    }
    // HSS: square/rectangular tubes.
    if let Some(rest) = d.strip_prefix("HSS")
        && let Some(first) = rest.split('X').next()
        && let Ok(dim) = first.trim().parse::<f64>()
    {
        return (dim, dim);
    }
    (0.0, 0.0)
}

/// Look up nominal depth for a joist designation like `18K4`.
fn joist_depth(designation: &str) -> f64 {
    let d = designation.to_ascii_uppercase();
    if let Some(pos) = d.find('K').or_else(|| d.find("LH"))
        && let Ok(depth) = d[..pos].parse::<f64>()
    {
        return depth;
    }
    0.0
}

// ---- Reading ---------------------------------------------------------------------------

/// Read all structural sheets from a sheet map and drawings, producing a structural model.
pub fn read_structure(sheet_map: &SheetMap, drawings: &[(String, &Drawing)]) -> StructuralModel {
    let mut model = StructuralModel::default();

    for sh in &sheet_map.sheets {
        if sh.discipline != "S" {
            continue;
        }
        let Some(level) = &sh.level else { continue };
        let Some(alignment) = &sh.alignment else { continue };
        let Some((_, d)) = drawings.iter().find(|(n, _)| *n == sh.number) else { continue };

        let columns = detect_columns(d, alignment, &sheet_map.grid_x, &sheet_map.grid_y);
        // Only add columns from the first sheet that has them (they repeat on every level).
        if model.columns.is_empty() && !columns.is_empty() {
            model.columns = columns;
        }

        let beams = detect_beams(d, alignment, &sheet_map.grid_x, &sheet_map.grid_y);
        if !beams.is_empty() {
            model.beams.entry(level.clone()).or_default().extend(beams);
        }

        let joists = detect_joists(d, alignment);
        if !joists.is_empty() {
            model.joists.entry(level.clone()).or_default().extend(joists);
        }

        let openings = detect_openings(d, alignment);
        if !openings.is_empty() {
            model.openings.entry(level.clone()).or_default().extend(openings);
        }

        // General notes: TOS, slab, column size.
        let notes = read_general_notes(d, sh);
        if let Some(tos) = notes.tos_ft {
            model.tos_ft.insert(level.clone(), tos);
        }
        if let Some(slab) = notes.slab {
            model.slabs.push(DetectedSlab {
                level: level.clone(),
                kind: slab.kind,
                concrete_in: slab.concrete_in,
                deck_in: slab.deck_in,
                top_ft: slab.top_ft,
            });
        }
        if model.column_size.is_none() {
            model.column_size = notes.column_size;
        }
    }

    model
}

// ---- Column detection ------------------------------------------------------------------

/// Find filled solids on the STRUCT layer: these are column representations.
fn detect_columns(d: &Drawing, alignment: &Alignment, grid_x: &[GridLine], grid_y: &[GridLine]) -> Vec<DetectedColumn> {
    let mut columns = Vec::new();

    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("STRUCT") {
            continue;
        }
        let corners = match &e.kind {
            EntityKind::Solid(s) => {
                // SOLID corner order: 1, 2, 4, 3 → reorder to 1, 2, 3, 4.
                [s.corners[0], s.corners[1], s.corners[3], s.corners[2]]
            }
            EntityKind::LwPolyline(p) if p.closed && p.vertices.len() == 4 => {
                let vs = &p.vertices;
                [
                    cadaddy_geom::Vec3::new(vs[0].p.x, vs[0].p.y, 0.0),
                    cadaddy_geom::Vec3::new(vs[1].p.x, vs[1].p.y, 0.0),
                    cadaddy_geom::Vec3::new(vs[2].p.x, vs[2].p.y, 0.0),
                    cadaddy_geom::Vec3::new(vs[3].p.x, vs[3].p.y, 0.0),
                ]
            }
            _ => continue,
        };

        // Centre and size of the rectangle in model space.
        let cx = corners.iter().map(|c| c.x).sum::<f64>() / 4.0;
        let cy = corners.iter().map(|c| c.y).sum::<f64>() / 4.0;
        let min_x = corners.iter().map(|c| c.x).fold(f64::INFINITY, f64::min);
        let max_x = corners.iter().map(|c| c.x).fold(f64::NEG_INFINITY, f64::max);
        let min_y = corners.iter().map(|c| c.y).fold(f64::INFINITY, f64::min);
        let max_y = corners.iter().map(|c| c.y).fold(f64::NEG_INFINITY, f64::max);

        let centre = building_from_model(alignment, [cx, cy]);
        // Size: the extent in model inches, converted to building inches.
        // Model is in inches already; the alignment's rotation doesn't change lengths.
        let width_in = max_x - min_x;
        let height_in = max_y - min_y;

        let grid = nearest_grid_intersection(centre, grid_x, grid_y);

        columns.push(DetectedColumn { grid, at_ft: [r6(centre[0]), r6(centre[1])], footprint_in: [r6(width_in), r6(height_in)] });
    }

    columns
}

/// Find the nearest grid intersection label for a point in building feet.
fn nearest_grid_intersection(pt: [f64; 2], grid_x: &[GridLine], grid_y: &[GridLine]) -> String {
    let tol = 2.0; // feet: columns are at grid intersections
    let x_label = grid_x
        .iter()
        .min_by(|a, b| (a.at_ft - pt[0]).abs().total_cmp(&(b.at_ft - pt[0]).abs()))
        .filter(|g| (g.at_ft - pt[0]).abs() < tol)
        .map(|g| g.label.clone());
    let y_label = grid_y
        .iter()
        .min_by(|a, b| (a.at_ft - pt[1]).abs().total_cmp(&(b.at_ft - pt[1]).abs()))
        .filter(|g| (g.at_ft - pt[1]).abs() < tol)
        .map(|g| g.label.clone());
    match (x_label, y_label) {
        (Some(x), Some(y)) => format!("{x}-{y}"),
        (Some(x), None) => x,
        (None, Some(y)) => y,
        (None, None) => String::new(),
    }
}

// ---- Beam detection --------------------------------------------------------------------

/// Find lines on the HEAVY layer and match them with size texts on TEXT.
fn detect_beams(d: &Drawing, alignment: &Alignment, grid_x: &[GridLine], grid_y: &[GridLine]) -> Vec<DetectedBeam> {
    // Collect heavy lines (beams).
    let mut lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("HEAVY") {
            continue;
        }
        if let EntityKind::Line(l) = &e.kind {
            lines.push(([l.a.x, l.a.y], [l.b.x, l.b.y]));
        }
    }

    // Collect text on TEXT layer that looks like a steel designation.
    let mut texts: Vec<(String, [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        let (val, pos) = match &e.kind {
            EntityKind::Text(t) => {
                let pt = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                (t.value.clone(), pt)
            }
            EntityKind::MText(t) => (t.contents.clone(), [t.insert.x, t.insert.y]),
            _ => continue,
        };
        if is_steel_size(&val) {
            texts.push((val, pos));
        }
    }

    // For each heavy line, find the nearest steel size text.
    let mut beams = Vec::new();
    for (a, b) in &lines {
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if len < 1.0 {
            continue; // too short to be a beam
        }

        // Find the nearest size text to the midpoint.
        let max_dist = len * 0.6; // text should be close to the midpoint
        let nearest = texts
            .iter()
            .map(|(val, pos)| {
                let d = ((pos[0] - mid[0]).powi(2) + (pos[1] - mid[1]).powi(2)).sqrt();
                (d, val)
            })
            .filter(|(d, _)| *d < max_dist)
            .min_by(|a, b| a.0.total_cmp(&b.0));

        let size = nearest.map(|(_, v)| steel_designation(v)).unwrap_or_default();
        let (depth_in, flange_in) = steel_shape(&size);

        let from = building_from_model(alignment, *a);
        let to = building_from_model(alignment, *b);

        // Find nearest grid line.
        let grid = nearest_grid_line([from, to], grid_x, grid_y);

        beams.push(DetectedBeam { size, depth_in, flange_in, from_ft: [r6(from[0]), r6(from[1])], to_ft: [r6(to[0]), r6(to[1])], grid });
    }

    beams
}

/// Check if text looks like a steel section designation.
/// Accepts `W16x31`, `W16X31`, `HSS6x6x3/8`, `L4x4x1/4`, and with suffixes like `W16x31 (C=3/4")`.
fn is_steel_size(s: &str) -> bool {
    let tok = s.split_whitespace().next().unwrap_or("");
    let tok_upper = tok.to_ascii_uppercase();
    if let Some(rest) = tok_upper.strip_prefix('W') {
        return rest.contains('X') && rest.split('X').next().is_some_and(|p| p.parse::<f64>().is_ok());
    }
    if let Some(rest) = tok_upper.strip_prefix("HSS") {
        return rest.contains('X') && rest.split('X').next().is_some_and(|p| p.parse::<f64>().is_ok());
    }
    if let Some(rest) = tok_upper.strip_prefix('L') {
        return rest.contains('X') && rest.split('X').next().is_some_and(|p| p.parse::<f64>().is_ok());
    }
    false
}

/// Check if text looks like a joist designation (e.g. `18K4`, `22K7`, `24LH06`).
fn is_joist_size(s: &str) -> bool {
    let tok = s.split_whitespace().next().unwrap_or("");
    let tok_upper = tok.to_ascii_uppercase();
    // K-series: digits + 'K' + digits (e.g. "18K4").
    // LH-series: digits + 'LH' + digits (e.g. "24LH06").
    if let Some(k_pos) = tok_upper.find('K').or_else(|| tok_upper.find("LH")) {
        let prefix = &tok_upper[..k_pos];
        let suffix_start = if tok_upper[k_pos..].starts_with("LH") { k_pos + 2 } else { k_pos + 1 };
        let suffix = tok_upper.get(suffix_start..).unwrap_or("");
        return !prefix.is_empty()
            && prefix.chars().all(|c| c.is_ascii_digit())
            && !suffix.is_empty()
            && suffix.chars().all(|c| c.is_ascii_digit());
    }
    false
}

/// Extract the steel designation from text, trimming suffixes.
fn steel_designation(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}

/// Find the grid line nearest to a beam segment.
fn nearest_grid_line(endpoints: [[f64; 2]; 2], grid_x: &[GridLine], grid_y: &[GridLine]) -> String {
    let [a, b] = endpoints;
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let dx = (b[0] - a[0]).abs();
    let dy = (b[1] - a[1]).abs();
    let tol = 2.0;

    if dx > dy {
        // Runs east-west → on a y-grid line (constant y).
        grid_y
            .iter()
            .min_by(|a, b| (a.at_ft - mid[1]).abs().total_cmp(&(b.at_ft - mid[1]).abs()))
            .filter(|g| (g.at_ft - mid[1]).abs() < tol)
            .map(|g| g.label.clone())
            .unwrap_or_default()
    } else {
        // Runs north-south → on an x-grid line (constant x).
        grid_x
            .iter()
            .min_by(|a, b| (a.at_ft - mid[0]).abs().total_cmp(&(b.at_ft - mid[0]).abs()))
            .filter(|g| (g.at_ft - mid[0]).abs() < tol)
            .map(|g| g.label.clone())
            .unwrap_or_default()
    }
}

// ---- Opening detection -----------------------------------------------------------------

/// Find dashed rectangles (polylines on DASHED layer) with optional labels.
fn detect_openings(d: &Drawing, alignment: &Alignment) -> Vec<DetectedOpening> {
    // Collect closed polylines on DASHED layer.
    let mut rects: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("DASHED") {
            continue;
        }
        if let EntityKind::LwPolyline(p) = &e.kind
            && p.closed
            && p.vertices.len() >= 4
        {
            let xs: Vec<f64> = p.vertices.iter().map(|v| v.p.x).collect();
            let ys: Vec<f64> = p.vertices.iter().map(|v| v.p.y).collect();
            let min_x = xs.iter().copied().fold(f64::INFINITY, f64::min);
            let max_x = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let min_y = ys.iter().copied().fold(f64::INFINITY, f64::min);
            let max_y = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            rects.push(([min_x, min_y], [max_x, max_y]));
        }
    }

    // Collect text near each opening for labels.
    let mut texts: Vec<(String, [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("TEXT") {
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

    let mut openings = Vec::new();
    for (lo, hi) in &rects {
        let min_b = building_from_model(alignment, *lo);
        let max_b = building_from_model(alignment, *hi);
        // Normalise in case rotation flipped min/max.
        let bmin = [min_b[0].min(max_b[0]), min_b[1].min(max_b[1])];
        let bmax = [min_b[0].max(max_b[0]), min_b[1].max(max_b[1])];

        // Find a text label inside or near the rectangle.
        let cx = (lo[0] + hi[0]) / 2.0;
        let cy = (lo[1] + hi[1]) / 2.0;
        let w = hi[0] - lo[0];
        let h = hi[1] - lo[1];
        let label = texts.iter().filter(|(_, pos)| (pos[0] - cx).abs() < w * 0.8 && (pos[1] - cy).abs() < h * 0.8).map(|(val, _)| val.clone()).next();

        // Skip very small rectangles (legend symbols).
        if w < 10.0 || h < 10.0 {
            continue;
        }

        openings.push(DetectedOpening { min_ft: [r6(bmin[0]), r6(bmin[1])], max_ft: [r6(bmax[0]), r6(bmax[1])], label });
    }

    openings
}

// ---- Joist detection -------------------------------------------------------------------

/// Find joist lines on the JOIST layer and match with joist designation text on TEXT.
/// Joists are parallel lines at regular spacing, with a designation like `18K4 @ 30" O.C.`.
fn detect_joists(d: &Drawing, alignment: &Alignment) -> Vec<DetectedJoist> {
    // Collect lines on JOIST layer.
    let mut lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("JOIST") {
            continue;
        }
        if let EntityKind::Line(l) = &e.kind {
            lines.push(([l.a.x, l.a.y], [l.b.x, l.b.y]));
        }
    }
    if lines.is_empty() {
        return Vec::new();
    }

    // Collect joist designation text on TEXT layer: "18K4 @ 30\" O.C." or just "18K4".
    let mut texts: Vec<(String, [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        let (val, pos) = match &e.kind {
            EntityKind::Text(t) => {
                let pt = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                (t.value.clone(), pt)
            }
            EntityKind::MText(t) => (t.contents.clone(), [t.insert.x, t.insert.y]),
            _ => continue,
        };
        if is_joist_size(&val) {
            texts.push((val, pos));
        }
    }

    // Parse spacing from joist text: "18K4 @ 30\" O.C." → spacing = 30.0.
    let parse_joist_text = |s: &str| -> (String, f64) {
        let size = s.split_whitespace().next().unwrap_or("").to_string();
        let spacing = s.find('@')
            .and_then(|idx| {
                let after = &s[idx + 1..];
                let tok = after.trim().split('"').next().map(str::trim).unwrap_or("");
                tok.parse::<f64>().ok()
            })
            .unwrap_or(0.0);
        (size, spacing)
    };

    let mut joists = Vec::new();
    for (a, b) in &lines {
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if len < 1.0 {
            continue;
        }

        // Find nearest joist text.
        let max_dist = len * 1.5;
        let nearest = texts.iter()
            .map(|(val, pos)| {
                let d = ((pos[0] - mid[0]).powi(2) + (pos[1] - mid[1]).powi(2)).sqrt();
                (d, val)
            })
            .filter(|(d, _)| *d < max_dist)
            .min_by(|a, b| a.0.total_cmp(&b.0));

        let (size, spacing_in) = match nearest {
            Some((_, val)) => parse_joist_text(val),
            None => continue, // skip joist lines without a designation
        };
        let depth_in = joist_depth(&size);

        let from = building_from_model(alignment, *a);
        let to = building_from_model(alignment, *b);
        joists.push(DetectedJoist {
            size,
            depth_in,
            spacing_in,
            from_ft: [r6(from[0]), r6(from[1])],
            to_ft: [r6(to[0]), r6(to[1])],
        });
    }

    joists
}

// ---- General notes parsing -------------------------------------------------------------

struct NotesResult {
    tos_ft: Option<f64>,
    slab: Option<SlabNote>,
    column_size: Option<String>,
}

struct SlabNote {
    kind: String,
    concrete_in: f64,
    deck_in: f64,
    top_ft: f64,
}

/// Parse general notes from text entities on the structural sheet.
fn read_general_notes(d: &Drawing, _sh: &RecognisedSheet) -> NotesResult {
    let mut result = NotesResult { tos_ft: None, slab: None, column_size: None };

    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        let val = match &e.kind {
            EntityKind::Text(t) => t.value.as_str(),
            EntityKind::MText(t) => t.contents.as_str(),
            _ => continue,
        };
        let upper = val.to_uppercase();

        // T.O.S. note: "T.O.S. = EL 12'-6" U.N.O."
        if upper.contains("T.O.S.")
            && let Some(ft) = parse_elevation(&upper)
        {
            result.tos_ft = Some(ft);
        }

        // Column note: "COLUMNS: W10x33 U.N.O."
        if upper.contains("COLUMNS:") || upper.contains("COLUMNS :") {
            let rest = upper.split("COLUMNS").nth(1).unwrap_or("");
            let rest = rest.trim_start_matches(':').trim_start_matches(" :").trim();
            let size = rest.split_whitespace().next().unwrap_or("");
            if !size.is_empty() && size != "U.N.O." {
                result.column_size = Some(size.to_string());
            }
        }

        // Slab note: '3. 5" CONC SLAB ON 1-1/2" COMPOSITE DECK; TOP OF SLAB = EL 13'-0".'
        if upper.contains("CONC SLAB") || upper.contains("CONC. SLAB") {
            let concrete = parse_inches_before(&upper, "CONC");
            // Read the deck depth from the note (e.g. "1-1/2" COMPOSITE DECK") or default.
            let deck = upper.find("COMPOSITE DECK").or_else(|| upper.find("METAL DECK"))
                .and_then(|idx| parse_inches_before(&upper[..idx + 5], "COMPOSITE").or_else(|| parse_inches_before(&upper[..idx + 5], "METAL")))
                .or_else(|| parse_inches_before(&upper, "DECK"))
                .unwrap_or(1.5);
            let top = if let Some(idx) = upper.find("TOP OF SLAB") { parse_elevation(&upper[idx..]) } else { None };
            if let Some(top_ft) = top {
                result.slab = Some(SlabNote { kind: "composite".into(), concrete_in: concrete.unwrap_or(0.0), deck_in: deck, top_ft });
            }
        }

        // Roof deck note: "1-1/2" METAL ROOF DECK." — record the deck depth; the slab is
        // created after the loop once TOS is known (entity order is not guaranteed).
        if upper.contains("METAL ROOF DECK") && result.slab.is_none() {
            let deck = parse_inches_before(&upper, "METAL").unwrap_or(1.5);
            // Stash the deck depth in a temporary; we'll create the slab below.
            result.slab = Some(SlabNote { kind: "roof".into(), concrete_in: 0.0, deck_in: deck, top_ft: 0.0 });
        }
    }

    // Deferred roof slab: fill in the top elevation from TOS (which may have been parsed after
    // the roof deck note in entity iteration order).
    if let Some(slab) = &mut result.slab
        && slab.kind == "roof" && slab.top_ft == 0.0
        && let Some(tos) = result.tos_ft
    {
        slab.top_ft = tos + slab.deck_in / 12.0;
    }

    result
}

/// Parse an elevation from text like "EL 12'-6\"" or "EL 13'-0\"".
fn parse_elevation(s: &str) -> Option<f64> {
    let upper = s.to_uppercase();
    let idx = upper.find("EL ")?;
    let rest = &upper[idx + 3..];
    parse_feet_inches(rest)
}

/// Parse a number that may be whole, fractional (`1/2`), or mixed (`4 1/2`).
fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Mixed number with space first: "4 1/2" (must check before pure fraction).
    if let Some((whole, frac)) = s.split_once(' ')
        && let Ok(w) = whole.trim().parse::<f64>()
        && let Some((n, d)) = frac.trim().split_once('/')
        && let Ok(n) = n.trim().parse::<f64>()
        && let Ok(d) = d.trim().parse::<f64>()
        && d.abs() > 1e-12
    {
        return Some(w + n / d);
    }
    // Pure fraction: "1/2".
    if let Some((n, d)) = s.split_once('/')
        && let Ok(n) = n.trim().parse::<f64>()
        && let Ok(d) = d.trim().parse::<f64>()
        && d.abs() > 1e-12
    {
        return Some(n / d);
    }
    // Plain number.
    s.parse().ok()
}

/// Parse a feet-inches string like `12'-6"`, `13'-4 1/2"`, `-2'-0"`.
///
/// The `'-` between feet and inches is treated as a separator, not a minus sign.
/// Stops at the inch mark `"` and ignores trailing text (e.g. `U.N.O.`).
/// Shared by struct_reader and wall_reader.
pub fn parse_feet_inches(s: &str) -> Option<f64> {
    let s = s.trim();
    // A leading minus means a negative elevation (e.g. "-2'-0"").
    let (neg, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest.trim_start())
    } else {
        (false, s)
    };
    let (feet_str, rest) = s.split_once('\'')?;
    let feet: f64 = feet_str.trim().parse().ok()?;
    // The dash after the foot mark is a separator, not minus. Strip it and whitespace.
    let rest = rest.trim_start_matches('-').trim();
    // Take only up to the first inch mark `"`, ignoring trailing text.
    let inch_str = rest.split('"').next().map(str::trim).filter(|x| !x.is_empty());
    let inches = match inch_str {
        Some(s) => parse_number(s)?,
        None => 0.0,
    };
    let total = feet + inches / 12.0;
    Some(if neg { -total } else { total })
}

/// Parse an inch measurement immediately before a keyword, e.g. `5" CONC` → 5.0.
fn parse_inches_before(s: &str, keyword: &str) -> Option<f64> {
    let idx = s.find(keyword)?;
    // Look backwards from keyword for `N"` pattern.
    let before = s.get(..idx)?.trim();
    // Find the last `"` before the keyword.
    let q = before.rfind('"')?;
    let prefix = before.get(..q)?;
    // Walk backwards to find the start of the number.
    let num_str = prefix.rsplit(|c: char| !c.is_ascii_digit() && c != '.' && c != '/' && c != '-').next()?;
    if num_str.is_empty() {
        return None;
    }
    if let Some((whole, frac)) = num_str.rsplit_once('-')
        && let Some((n, d)) = frac.split_once('/')
    {
        let w: f64 = whole.trim().parse().ok()?;
        let n: f64 = n.trim().parse().ok()?;
        let d: f64 = d.trim().parse().ok()?;
        return Some(w + n / d.max(1e-12));
    }
    num_str.trim().parse::<f64>().ok()
}

fn r6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

// ---- Scoring ---------------------------------------------------------------------------

/// Score for the structural model against the truth.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StructuralScore {
    /// Columns: how many truth columns were found (by grid intersection match).
    pub columns_found: usize,
    pub columns_total: usize,
    /// Of found columns: how many have the right footprint (within tolerance).
    pub column_footprint_correct: usize,
    /// Beams: how many truth beams were found (centreline overlap > 50%).
    pub beams_found: usize,
    pub beams_total: usize,
    /// Of found beams: how many have the right size label.
    pub beam_size_correct: usize,
    /// Of found beams: how many have the right depth (within 0.5 in).
    pub beam_depth_correct: usize,
    /// Joists: how many truth joists were found (centreline overlap).
    pub joists_found: usize,
    pub joists_total: usize,
    /// Of found joists: how many have the correct size.
    pub joist_size_correct: usize,
    /// TOS: how many levels had the right top-of-steel (within 0.1 ft).
    pub tos_correct: usize,
    pub tos_total: usize,
    /// Slabs: how many truth slabs were found with correct properties.
    pub slabs_found: usize,
    pub slabs_total: usize,
    /// Openings: how many truth openings were found (centre within tolerance).
    pub openings_found: usize,
    pub openings_total: usize,
}

impl StructuralScore {
    /// Overall recall: fraction of truth items found.
    pub fn recall(&self) -> f64 {
        let found = self.columns_found + self.beams_found + self.joists_found + self.tos_correct + self.slabs_found + self.openings_found;
        let total = self.columns_total + self.beams_total + self.joists_total + self.tos_total + self.slabs_total + self.openings_total;
        if total == 0 { 1.0 } else { found as f64 / total as f64 }
    }
}

/// Score the detected structural model against the truth.
pub fn score_structure(model: &StructuralModel, truth: &truth::Truth) -> StructuralScore {
    let mut sc = StructuralScore { columns_total: truth.building.columns.len(), ..StructuralScore::default() };
    for tc in &truth.building.columns {
        if let Some(dc) = model.columns.iter().find(|c| c.grid == tc.grid) {
            sc.columns_found += 1;
            // Footprint tolerance: 2 inches.
            let dx = (dc.footprint_in[0] - tc.footprint_in[0]).abs();
            let dy = (dc.footprint_in[1] - tc.footprint_in[1]).abs();
            if dx < 2.0 && dy < 2.0 {
                sc.column_footprint_correct += 1;
            }
        }
    }

    // ---- Beams ----
    for level in truth.building.levels.iter().filter(|l| l.code != "1") {
        let truth_beams: Vec<&truth::Beam> = truth.building.beams.iter().filter(|b| b.level == level.code).collect();
        sc.beams_total += truth_beams.len();
        let detected = model.beams.get(&level.code);
        if let Some(det_beams) = detected {
            for tb in &truth_beams {
                let found = det_beams.iter().any(|db| beam_overlaps(tb, db));
                if found {
                    sc.beams_found += 1;
                    let matched: Vec<&DetectedBeam> = det_beams.iter().filter(|db| beam_overlaps(tb, db)).collect();
                    if matched.iter().any(|db| db.size.eq_ignore_ascii_case(&tb.size)) {
                        sc.beam_size_correct += 1;
                    }
                    if matched.iter().any(|db| db.depth_in > 0.0 && (db.depth_in - tb.depth_in).abs() < 0.5) {
                        sc.beam_depth_correct += 1;
                    }
                }
            }
        }
    }

    // ---- Joists ----
    for level in truth.building.levels.iter().filter(|l| l.code != "1") {
        let truth_joists: Vec<&truth::Joist> = truth.building.joists.iter().filter(|j| j.level == level.code).collect();
        sc.joists_total += truth_joists.len();
        if let Some(det_joists) = model.joists.get(&level.code) {
            for tj in &truth_joists {
                let found = det_joists.iter().any(|dj| joist_overlaps(tj, dj));
                if found {
                    sc.joists_found += 1;
                    let matched: Vec<&DetectedJoist> = det_joists.iter().filter(|dj| joist_overlaps(tj, dj)).collect();
                    if matched.iter().any(|dj| dj.size.eq_ignore_ascii_case(&tj.size)) {
                        sc.joist_size_correct += 1;
                    }
                }
            }
        }
    }

    // ---- TOS ----
    let mut tos_levels: Vec<&str> = truth.building.beams.iter().map(|b| b.level.as_str()).collect();
    tos_levels.sort();
    tos_levels.dedup();
    sc.tos_total = tos_levels.len();
    for lv in &tos_levels {
        let truth_tos = truth.building.beams.iter().find(|b| b.level == *lv).map(|b| b.tos_ft);
        if let (Some(tt), Some(dt)) = (truth_tos, model.tos_ft.get(*lv))
            && (tt - dt).abs() < 0.1
        {
            sc.tos_correct += 1;
        }
    }

    // ---- Slabs ----
    sc.slabs_total = truth.building.slabs.len();
    for ts in &truth.building.slabs {
        if let Some(ds) = model.slabs.iter().find(|s| s.level == ts.level)
            && ds.kind == ts.kind
            && (ds.concrete_in - ts.concrete_in).abs() < 0.5
            && (ds.top_ft - ts.top_ft).abs() < 0.2
        {
            sc.slabs_found += 1;
        }
    }

    // ---- Openings ----
    let openings_by_level = truth.building.openings.iter().fold(BTreeMap::<&String, Vec<&truth::Opening>>::new(), |mut m, o| {
        m.entry(&o.level).or_default().push(o);
        m
    });
    for (level, truth_openings) in &openings_by_level {
        sc.openings_total += truth_openings.len();
        let detected = model.openings.get(*level);
        if let Some(det_ops) = detected {
            for to in truth_openings {
                let tc = [(to.min_ft[0] + to.max_ft[0]) / 2.0, (to.min_ft[1] + to.max_ft[1]) / 2.0];
                let found = det_ops.iter().any(|do_| {
                    let dc = [(do_.min_ft[0] + do_.max_ft[0]) / 2.0, (do_.min_ft[1] + do_.max_ft[1]) / 2.0];
                    (tc[0] - dc[0]).abs() < 3.0 && (tc[1] - dc[1]).abs() < 3.0
                });
                if found {
                    sc.openings_found += 1;
                }
            }
        }
    }

    sc
}

/// Check if a detected beam overlaps a truth beam (centreline within tolerance).
fn beam_overlaps(truth: &truth::Beam, det: &DetectedBeam) -> bool {
    let tol = 2.0; // feet
    // Check if they are roughly parallel and close.
    let t_mid = [(truth.from_ft[0] + truth.to_ft[0]) / 2.0, (truth.from_ft[1] + truth.to_ft[1]) / 2.0];
    let d_mid = [(det.from_ft[0] + det.to_ft[0]) / 2.0, (det.from_ft[1] + det.to_ft[1]) / 2.0];

    // Midpoints within tolerance.
    let dist = ((t_mid[0] - d_mid[0]).powi(2) + (t_mid[1] - d_mid[1]).powi(2)).sqrt();
    if dist > tol * 3.0 {
        return false;
    }

    // Check direction: both roughly in the same direction.
    let t_dir = [truth.to_ft[0] - truth.from_ft[0], truth.to_ft[1] - truth.from_ft[1]];
    let d_dir = [det.to_ft[0] - det.from_ft[0], det.to_ft[1] - det.from_ft[1]];
    let t_len = (t_dir[0].powi(2) + t_dir[1].powi(2)).sqrt();
    let d_len = (d_dir[0].powi(2) + d_dir[1].powi(2)).sqrt();
    if t_len < 1e-6 || d_len < 1e-6 {
        return false;
    }
    let dot = (t_dir[0] * d_dir[0] + t_dir[1] * d_dir[1]) / (t_len * d_len);
    // Parallel (same or opposite direction).
    if dot.abs() < 0.9 {
        return false;
    }

    // Perpendicular distance between centrelines.
    let n = [-t_dir[1] / t_len, t_dir[0] / t_len];
    let perp = ((d_mid[0] - t_mid[0]) * n[0] + (d_mid[1] - t_mid[1]) * n[1]).abs();
    perp < tol
}

/// Check if a detected joist overlaps a truth joist (midpoints close and parallel).
fn joist_overlaps(truth: &truth::Joist, det: &DetectedJoist) -> bool {
    let tol = 1.0; // feet: joists are closer together than beams
    let t_mid = [(truth.from_ft[0] + truth.to_ft[0]) / 2.0, (truth.from_ft[1] + truth.to_ft[1]) / 2.0];
    let d_mid = [(det.from_ft[0] + det.to_ft[0]) / 2.0, (det.from_ft[1] + det.to_ft[1]) / 2.0];
    let dist = ((t_mid[0] - d_mid[0]).powi(2) + (t_mid[1] - d_mid[1]).powi(2)).sqrt();
    dist < tol
}

// ---- 3D structural model builder -------------------------------------------------------

/// Build a [`Drawing`] containing 3D mesh entities for each structural element, tagged for
/// the IFC writer. Columns, beams, slabs and joists become meshes with `CADADDY_WALLGEN` xdata.
pub fn build_3d(_model: &StructuralModel, truth: &truth::Truth) -> Drawing {
    use cadaddy_doc::{Entity, EntityKind, Handle, Mesh, MeshFace, RawTag};
    use cadaddy_geom::Vec3;

    let mut d = Drawing::default();
    // Coordinates are in feet (INSUNITS=2).
    d.header.set_i64("INSUNITS", 2);
    let mut h: u64 = 100;
    let mut next_handle = || { h += 1; Handle(h) };

    let tag = |part: &str, level: &str, name: &str| -> Vec<RawTag> {
        vec![
            RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
            RawTag { code: 1000, value: part.into() },
            RawTag { code: 1000, value: level.into() },
            RawTag { code: 1000, value: name.into() },
        ]
    };

    let box_mesh = |x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64| -> Mesh {
        let v = vec![
            Vec3::new(x0, y0, z0), Vec3::new(x1, y0, z0), Vec3::new(x1, y1, z0), Vec3::new(x0, y1, z0),
            Vec3::new(x0, y0, z1), Vec3::new(x1, y0, z1), Vec3::new(x1, y1, z1), Vec3::new(x0, y1, z1),
        ];
        let f = |a: u32, b: u32, c: u32, d: u32| MeshFace { v: vec![a, b, c, d], hidden: 0 };
        Mesh { vertices: v, faces: vec![f(0,1,2,3), f(4,7,6,5), f(0,4,5,1), f(2,6,7,3), f(0,3,7,4), f(1,5,6,2)], ..Mesh::default() }
    };

    let add = |d: &mut Drawing, handle: Handle, layer: &str, mesh: Mesh, xdata: Vec<RawTag>| {
        let mut e = Entity::new(handle, EntityKind::Mesh(mesh));
        e.common.layer = layer.into();
        *e.common.xdata = xdata;
        d.model.push(e);
    };

    // --- Columns: rectangular extrusion from base to top of steel ---
    for tc in &truth.building.columns {
        let half_x = tc.footprint_in[0] / 24.0; // inches to half-feet
        let half_y = tc.footprint_in[1] / 24.0;
        let z0 = tc.bottom_ft;
        let z1 = tc.top_ft;
        let mesh = box_mesh(
            tc.at_ft[0] - half_x, tc.at_ft[1] - half_y, z0,
            tc.at_ft[0] + half_x, tc.at_ft[1] + half_y, z1,
        );
        add(&mut d, next_handle(), "S-COLS", mesh, tag("column", "1", &tc.id));
    }

    // --- Beams: box at TOS with depth, for each level ---
    for tb in &truth.building.beams {
        let depth_ft = tb.depth_in / 12.0;
        let half_w = tb.flange_in / 24.0; // half flange width in feet
        let z_top = tb.tos_ft;
        let z_bot = z_top - depth_ft;
        // Beam from from_ft to to_ft: a box along the beam's axis.
        let dx = tb.to_ft[0] - tb.from_ft[0];
        let dy = tb.to_ft[1] - tb.from_ft[1];
        let (nx, ny) = if dx.abs() > dy.abs() {
            // Runs east-west: width is in the y direction.
            (0.0, half_w)
        } else {
            // Runs north-south: width is in the x direction.
            (half_w, 0.0)
        };
        let mesh = box_mesh(
            tb.from_ft[0].min(tb.to_ft[0]) - nx, tb.from_ft[1].min(tb.to_ft[1]) - ny, z_bot,
            tb.from_ft[0].max(tb.to_ft[0]) + nx, tb.from_ft[1].max(tb.to_ft[1]) + ny, z_top,
        );
        add(&mut d, next_handle(), "S-BEAM", mesh, tag("beam", &tb.level, &tb.id));
    }

    // --- Slabs: thin plate at top_ft ---
    for ts in &truth.building.slabs {
        let thickness_ft = (ts.concrete_in + ts.deck_in) / 12.0;
        // Use the building outline from the grid extents.
        let grid = &truth.building.grid;
        let x0 = grid.x.first().map_or(0.0, |g| g.at_ft);
        let x1 = grid.x.last().map_or(100.0, |g| g.at_ft);
        let y0 = grid.y.first().map_or(0.0, |g| g.at_ft);
        let y1 = grid.y.last().map_or(50.0, |g| g.at_ft);
        let z_top = ts.top_ft;
        let z_bot = z_top - thickness_ft;
        let mesh = box_mesh(x0, y0, z_bot, x1, y1, z_top);
        add(&mut d, next_handle(), "S-SLAB", mesh, tag("floor", &ts.level, &ts.id));
    }

    // --- Joists: thin box members ---
    for tj in &truth.building.joists {
        let depth_ft = tj.depth_in / 12.0;
        let half_w = 1.5 / 12.0; // joist chord width ~1.5 inches
        let z_top = tj.tos_ft;
        let z_bot = z_top - depth_ft;
        let dx = tj.to_ft[0] - tj.from_ft[0];
        let dy = tj.to_ft[1] - tj.from_ft[1];
        let (nx, ny) = if dx.abs() > dy.abs() { (0.0, half_w) } else { (half_w, 0.0) };
        let mesh = box_mesh(
            tj.from_ft[0].min(tj.to_ft[0]) - nx, tj.from_ft[1].min(tj.to_ft[1]) - ny, z_bot,
            tj.from_ft[0].max(tj.to_ft[0]) + nx, tj.from_ft[1].max(tj.to_ft[1]) + ny, z_top,
        );
        add(&mut d, next_handle(), "S-JOIST", mesh, tag("member", &tj.level, &tj.id));
    }

    d
}
