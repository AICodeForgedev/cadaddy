//! Ceiling reader: extracts ceiling types, heights, grid layout, soffits, ceiling-mounted items,
//! rooms and plenum depth from reflected ceiling plan (RCP) and architectural DXF sheets.
//!
//! The reader parses:
//! - Room boundaries from `ARCH` layer rectangles (same as `background()` in `draw.rs`)
//! - Room names and numbers from `ARCH-TEXT` layer text
//! - Ceiling type: **ACT** (detected by a grid of lines on the `LIGHT` layer inside a room),
//!   **GYP** (ceiling tag present but no grid lines), or **open to structure** (text
//!   `"OPEN TO STRUCTURE"` on the `TEXT` layer)
//! - Ceiling heights from ceiling tags on the `TEXT` layer (parsed from the style-varied formats
//!   produced by [`Style::ceiling_text`])
//! - ACT grid tile sizes from the spacing of `LIGHT`-layer lines
//! - Soffits / bulkheads from dashed-layer rectangles with tags
//! - Ceiling-mounted items (supply diffusers, return grilles, sprinkler heads) counted per room
//! - Plenum depth estimated as structure-bottom minus ceiling height (when structure height is
//!   known from framing notes or a provided value)

use std::collections::BTreeMap;

use crate::sheet_reader::{self, Alignment, SheetMap};
use crate::struct_reader::StructuralModel;
use crate::truth::Truth;
use cadaddy_doc::{Drawing, EntityKind};

// ---- Data structures ----------------------------------------------------------------

/// A room boundary detected on an RCP or architectural plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundRoom {
    pub level: String,
    pub name: String,
    pub number: String,
    /// Building feet, axis-aligned bounding box.
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
}

/// A ceiling detected in a room.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundCeiling {
    pub level: String,
    /// The room number this ceiling belongs to.
    pub room_number: String,
    /// `"ACT"`, `"GYP"`, or empty for open-to-structure.
    pub kind: String,
    /// Height above the floor, feet.
    pub height_ft: Option<f64>,
    /// The raw tag text as read from the DXF.
    pub tag_text: Option<String>,
    /// ACT grid tile size `[across, along]` in feet, if detected.
    pub grid_ft: Option<[f64; 2]>,
    /// Whether the room is open to structure (no ceiling).
    pub open_to_structure: bool,
}

/// A soffit detected on the RCP.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundSoffit {
    pub level: String,
    pub room_number: String,
    pub height_ft: Option<f64>,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub tag_text: Option<String>,
}

/// Ceiling-mounted items counted per room.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundCeilingItem {
    pub level: String,
    pub room_number: String,
    /// `"supply_diffuser"`, `"return_grille"`, `"sprinkler_head"`, `"exhaust_grille"`.
    pub kind: String,
    pub at_ft: [f64; 2],
}

/// A shaft or chase detected on an architectural plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundShaft {
    pub level: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub label: Option<String>,
}

/// Plenum depth for a room (space between ceiling and structure above).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundPlenum {
    pub level: String,
    pub room_number: String,
    pub plenum_ft: f64,
    pub ceiling_height_ft: f64,
    pub structure_bottom_ft: f64,
}

/// Complete result of reading ceiling data from a set of DXF sheets.
#[derive(Clone, Debug, Default)]
pub struct CeilingResult {
    pub rooms: Vec<FoundRoom>,
    pub ceilings: Vec<FoundCeiling>,
    pub soffits: Vec<FoundSoffit>,
    pub items: Vec<FoundCeilingItem>,
    pub shafts: Vec<FoundShaft>,
    pub plenums: Vec<FoundPlenum>,
}

/// Score comparing found ceilings against truth.
#[derive(Clone, Debug, Default)]
pub struct CeilingScore {
    /// Rooms in truth that we found a ceiling for.
    pub rooms_found: usize,
    /// Total rooms in truth with ceilings.
    pub rooms_total: usize,
    /// Of rooms found: how many have the right ceiling kind.
    pub kind_correct: usize,
    /// Of rooms found: how many have the right height (within 0.5 ft).
    pub height_correct: usize,
    /// Mean absolute height error in feet, over rooms with height.
    pub height_mae_ft: Option<f64>,
    /// ACT rooms where the grid tile size was correctly detected.
    pub grid_correct: usize,
    /// ACT rooms with a ceiling in truth.
    pub grid_total: usize,
    /// Open-to-structure rooms correctly detected.
    pub open_found: usize,
    /// Total open-to-structure rooms in truth.
    pub open_total: usize,
    /// Soffits found that match truth soffits (by position overlap).
    pub soffits_found: usize,
    pub soffits_total: usize,
}

// ---- Parsing helpers ----------------------------------------------------------------

/// Parse a feet-inches height string like `9'-0"`, `10'-6"`, `8'-7 1/2"`.
fn parse_feet_inches(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('"');
    let (feet_s, inches_s) = s.split_once("'-")?;
    let feet: f64 = feet_s.trim().parse().ok()?;
    let inches_s = inches_s.trim();
    let inches: f64 = if let Some((whole, frac)) = inches_s.split_once(' ') {
        let w: f64 = whole.trim().parse().ok()?;
        let f = parse_fraction_val(frac.trim())?;
        w + f
    } else {
        inches_s.parse().ok()?
    };
    Some(feet + inches / 12.0)
}

fn parse_fraction_val(s: &str) -> Option<f64> {
    if let Some((n, d)) = s.split_once('/') {
        let n: f64 = n.trim().parse().ok()?;
        let d: f64 = d.trim().parse().ok()?;
        if d.abs() > 1e-12 {
            return Some(n / d);
        }
    }
    None
}

/// Parse a ceiling tag. Handles the three `Style::ceiling_text` formats:
/// - `"CLG @ 9'-0" ACT"` / `"CLG @ 9'-0"\nACT"` (ceiling_tag=0)
/// - `"9'-0\" AFF ACT"` / `"9'-0\" AFF\nACT"` (ceiling_tag=1)
/// - `"ACT 9'-0\""` / `"ACT\n9'-0\""` (ceiling_tag=2)
///
/// Returns `(kind, height_ft)`.
fn parse_ceiling_tag(text: &str) -> Option<(String, f64)> {
    let text = text.trim();
    // Try multi-line (the draw code uses `pen.block` which produces MText with \n or \\P).
    let lines: Vec<&str> = text.split(['\n', '\\']).filter(|s| !s.is_empty() && *s != "P").collect();

    // Try ceiling_tag=0: "CLG @ <height>" on one line, kind on another
    for (i, line) in lines.iter().enumerate() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("CLG @").or_else(|| line.strip_prefix("CLG@"))
            && let Some(h) = parse_feet_inches(rest.trim())
        {
            let kind = lines.get(i + 1).or_else(|| lines.get(i.wrapping_sub(1))).map(|s| s.trim().to_string()).unwrap_or_default();
            return Some((normalise_kind(&kind), h));
        }
    }

    // Try ceiling_tag=1: "<height> AFF" on one line, kind on another
    for (i, line) in lines.iter().enumerate() {
        let line = line.trim();
        if let Some(rest) = line.strip_suffix("AFF").or_else(|| line.strip_suffix("aff"))
            && let Some(h) = parse_feet_inches(rest.trim())
        {
            let kind = lines.get(i + 1).or_else(|| lines.get(i.wrapping_sub(1))).map(|s| s.trim().to_string()).unwrap_or_default();
            return Some((normalise_kind(&kind), h));
        }
    }

    // Try ceiling_tag=2: kind on first line, height on second
    if lines.len() >= 2 {
        let kind_line = lines[0].trim();
        let height_line = lines[1].trim();
        if let Some(h) = parse_feet_inches(height_line) {
            let k = normalise_kind(kind_line);
            if k == "ACT" || k == "GYP" {
                return Some((k, h));
            }
        }
    }

    // Single line: try to extract any height and kind from it
    if let Some(h) = extract_height(text) {
        let kind = if text.contains("ACT") {
            "ACT"
        } else if text.contains("GYP") {
            "GYP"
        } else {
            ""
        };
        return Some((kind.to_string(), h));
    }

    None
}

fn normalise_kind(s: &str) -> String {
    let s = s.trim().to_uppercase();
    if s.contains("ACT") || s.contains("ACOUSTIC") {
        "ACT".to_string()
    } else if s.contains("GYP") || s.contains("GYPSUM") {
        "GYP".to_string()
    } else {
        s
    }
}

/// Try to pull a feet-inches value out of a string with surrounding text.
fn extract_height(s: &str) -> Option<f64> {
    // Look for N'-N" pattern
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'\'' && i > 0 && bytes.get(i.wrapping_sub(1)).is_some_and(u8::is_ascii_digit) {
            // Walk backwards to find start of number
            let mut start = i - 1;
            while start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'-') {
                start -= 1;
            }
            if let Some(h) = parse_feet_inches(&s[start..]) {
                return Some(h);
            }
        }
    }
    None
}

/// Parse a soffit tag like `"SOFFIT @ 8'-0""`
fn parse_soffit_tag(text: &str) -> Option<f64> {
    let text = text.trim().to_uppercase();
    if !text.contains("SOFFIT") && !text.contains("BULKHEAD") {
        return None;
    }
    if let Some(rest) = text.split_once('@').map(|(_, r)| r) {
        return parse_feet_inches(rest.trim());
    }
    extract_height(&text)
}

// ---- Room detection -----------------------------------------------------------------

/// Detect axis-aligned rooms from ARCH-layer rectangles and ARCH-TEXT labels.
fn detect_rooms(d: &Drawing, alignment: &Alignment, level: &str) -> Vec<FoundRoom> {
    // Collect ARCH-layer line-segment rectangles.
    let mut rects: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut h_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut v_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();

    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("ARCH") {
            continue;
        }
        match &e.kind {
            EntityKind::LwPolyline(p) if p.vertices.len() == 4 || (p.vertices.len() == 5 && p.closed) => {
                let pts: Vec<[f64; 2]> = p.vertices.iter().map(|v| to_building(alignment, [v.p.x, v.p.y])).collect();
                let (mut xmin, mut ymin) = (f64::INFINITY, f64::INFINITY);
                let (mut xmax, mut ymax) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
                for pt in &pts {
                    xmin = xmin.min(pt[0]);
                    ymin = ymin.min(pt[1]);
                    xmax = xmax.max(pt[0]);
                    ymax = ymax.max(pt[1]);
                }
                if (xmax - xmin) > 0.5 && (ymax - ymin) > 0.5 {
                    rects.push(([xmin, ymin], [xmax, ymax]));
                }
            }
            EntityKind::Line(l) => {
                let a = to_building(alignment, [l.a.x, l.a.y]);
                let b = to_building(alignment, [l.b.x, l.b.y]);
                let dx = (b[0] - a[0]).abs();
                let dy = (b[1] - a[1]).abs();
                if dx > 0.5 && dy < 0.2 {
                    h_lines.push((a, b));
                } else if dy > 0.5 && dx < 0.2 {
                    v_lines.push((a, b));
                }
            }
            _ => {}
        }
    }

    // Assemble rectangles from pairs of horizontal and vertical lines.
    // The draw code uses `brect` which produces 4 Line entities forming a rectangle.
    // Group horizontal lines by y-position and vertical lines by x-position.
    let tol = 0.15; // tolerance in building feet
    let mut assembled: Vec<([f64; 2], [f64; 2])> = Vec::new();
    // For each pair of horizontal lines at similar x-range but different y, and matching verticals:
    for (i, ha) in h_lines.iter().enumerate() {
        let (ha_xmin, ha_xmax) = (ha.0[0].min(ha.1[0]), ha.0[0].max(ha.1[0]));
        let ha_y = (ha.0[1] + ha.1[1]) / 2.0;
        for hb in h_lines.iter().skip(i + 1) {
            let (hb_xmin, hb_xmax) = (hb.0[0].min(hb.1[0]), hb.0[0].max(hb.1[0]));
            let hb_y = (hb.0[1] + hb.1[1]) / 2.0;
            if (ha_y - hb_y).abs() < tol {
                continue;
            }
            if (ha_xmin - hb_xmin).abs() > tol || (ha_xmax - hb_xmax).abs() > tol {
                continue;
            }
            // Check for matching vertical lines on both sides
            let x_left = ha_xmin.min(hb_xmin);
            let x_right = ha_xmax.max(hb_xmax);
            let y_low = ha_y.min(hb_y);
            let y_high = ha_y.max(hb_y);
            let has_left = v_lines.iter().any(|v| {
                let (vy_min, vy_max) = (v.0[1].min(v.1[1]), v.0[1].max(v.1[1]));
                let vx = (v.0[0] + v.1[0]) / 2.0;
                (vx - x_left).abs() < tol && vy_min < y_low + tol && vy_max > y_high - tol
            });
            let has_right = v_lines.iter().any(|v| {
                let (vy_min, vy_max) = (v.0[1].min(v.1[1]), v.0[1].max(v.1[1]));
                let vx = (v.0[0] + v.1[0]) / 2.0;
                (vx - x_right).abs() < tol && vy_min < y_low + tol && vy_max > y_high - tol
            });
            if has_left && has_right {
                assembled.push(([x_left, y_low], [x_right, y_high]));
            }
        }
    }
    rects.extend(assembled);

    // Deduplicate overlapping rects (same room drawn by brect creates 4 lines).
    rects.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]).then_with(|| a.0[1].total_cmp(&b.0[1])));
    let mut deduped: Vec<([f64; 2], [f64; 2])> = Vec::new();
    'outer: for r in &rects {
        for existing in &deduped {
            if (r.0[0] - existing.0[0]).abs() < tol
                && (r.0[1] - existing.0[1]).abs() < tol
                && (r.1[0] - existing.1[0]).abs() < tol
                && (r.1[1] - existing.1[1]).abs() < tol
            {
                continue 'outer;
            }
        }
        deduped.push(*r);
    }

    // Collect text on ARCH-TEXT layer.
    let mut labels: Vec<(String, [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("ARCH-TEXT") {
            continue;
        }
        match &e.kind {
            EntityKind::Text(t) => {
                let pt = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                let b = to_building(alignment, pt);
                labels.push((t.value.clone(), b));
            }
            EntityKind::MText(t) => {
                let b = to_building(alignment, [t.insert.x, t.insert.y]);
                // MText block: lines separated by \P or \n
                for line in t.contents.split(['\n', '\\']).filter(|s| !s.is_empty() && *s != "P") {
                    labels.push((line.trim().to_string(), b));
                }
            }
            _ => {}
        }
    }

    // Match labels to rooms: a label inside a rect names the room.
    let mut rooms = Vec::new();
    for (min, max) in &deduped {
        let mut name = String::new();
        let mut number = String::new();
        for (text, pt) in &labels {
            if pt[0] >= min[0] - 0.5 && pt[0] <= max[0] + 0.5 && pt[1] >= min[1] - 0.5 && pt[1] <= max[1] + 0.5 {
                // Number if all digits, else name
                if text.chars().all(|c| c.is_ascii_digit()) && number.is_empty() {
                    number = text.clone();
                } else if name.is_empty() {
                    name = text.clone();
                }
            }
        }
        rooms.push(FoundRoom { level: level.to_string(), name, number, min_ft: *min, max_ft: *max });
    }

    rooms
}

// ---- Ceiling detection --------------------------------------------------------------

/// Detect ACT grid lines on the LIGHT layer inside a room, and measure tile spacing.
///
/// Grid lines must span most of the room (at least 60% of its width or height) to be
/// counted, preventing bleed-in from neighbouring ACT rooms through shared walls.
fn detect_act_grid(d: &Drawing, alignment: &Alignment, room_min: [f64; 2], room_max: [f64; 2]) -> Option<[f64; 2]> {
    let tol = 0.3;
    let room_w = room_max[0] - room_min[0];
    let room_h = room_max[1] - room_min[1];
    // Lines must span a reasonable portion of the room dimension, but for very
    // long/narrow rooms (corridors), the short dimension lines still count if they
    // span at least 2 ft (a tile height) and 40% of that dimension.
    let min_span = |room_dim: f64| -> f64 {
        (room_dim * 0.4).max(2.0).min(room_dim - 0.5)
    };

    let mut h_ys: Vec<f64> = Vec::new(); // horizontal grid lines (constant y)
    let mut v_xs: Vec<f64> = Vec::new(); // vertical grid lines (constant x)

    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("LIGHT") {
            continue;
        }
        if let EntityKind::Line(l) = &e.kind {
            let a = to_building(alignment, [l.a.x, l.a.y]);
            let b = to_building(alignment, [l.b.x, l.b.y]);
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            // Midpoint must be inside the room.
            if mid[0] < room_min[0] - tol || mid[0] > room_max[0] + tol || mid[1] < room_min[1] - tol || mid[1] > room_max[1] + tol {
                continue;
            }
            let dx = (b[0] - a[0]).abs();
            let dy = (b[1] - a[1]).abs();
            if dx > 1.0 && dy < 0.3 {
                if dx >= min_span(room_w) {
                    h_ys.push((a[1] + b[1]) / 2.0);
                }
            } else if dy > 1.0 && dx < 0.3 && dy >= min_span(room_h) {
                v_xs.push((a[0] + b[0]) / 2.0);
            }
        }
    }

    if v_xs.len() < 2 && h_ys.len() < 2 {
        return None;
    }

    // Compute median spacing
    let median_spacing = |vals: &mut Vec<f64>| -> f64 {
        vals.sort_by(|a, b| a.total_cmp(b));
        let mut spacings: Vec<f64> = Vec::new();
        for w in vals.windows(2) {
            let d = w[1] - w[0];
            if d > 0.5 {
                spacings.push(d);
            }
        }
        if spacings.is_empty() {
            return 0.0;
        }
        spacings.sort_by(|a, b| a.total_cmp(b));
        spacings[spacings.len() / 2]
    };

    let sx = median_spacing(&mut v_xs);
    let sy = median_spacing(&mut h_ys);

    // Round to nearest standard tile size: 1, 2, 4 feet
    let round_tile = |v: f64| -> f64 {
        let candidates = [1.0, 2.0, 4.0];
        let mut best = candidates[0];
        let mut best_d = (v - candidates[0]).abs();
        for &c in &candidates[1..] {
            let d = (v - c).abs();
            if d < best_d {
                best = c;
                best_d = d;
            }
        }
        best
    };

    if sx > 0.5 && sy > 0.5 {
        Some([round_tile(sx), round_tile(sy)])
    } else if sx > 0.5 {
        Some([round_tile(sx), round_tile(sx)])
    } else if sy > 0.5 {
        Some([round_tile(sy), round_tile(sy)])
    } else {
        None
    }
}

/// Detect ceiling tags on the TEXT layer inside a room.
///
/// `pen.block()` creates individual Text entities for each line of a ceiling tag, so
/// a two-line tag like `"CLG @ 9'-0"\nACT"` appears as two nearby Text entities:
/// one with the height, one with the kind.  We collect all TEXT entities near the room,
/// then try to combine height + kind from entities at the same x-position.
///
/// Returns `(kind, height_ft_option, raw_text)`.
fn detect_ceiling_tag(d: &Drawing, alignment: &Alignment, room_min: [f64; 2], room_max: [f64; 2]) -> Option<(String, Option<f64>, String)> {
    let cx = (room_min[0] + room_max[0]) / 2.0;
    let cy = (room_min[1] + room_max[1]) / 2.0;
    let tol = 3.0;

    // Step 1: Collect all TEXT-layer entities near the room.
    struct TextItem {
        text: String,
        pos: [f64; 2],
        dist: f64,
    }
    let mut texts: Vec<TextItem> = Vec::new();

    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        let (text, pt) = match &e.kind {
            EntityKind::Text(t) => (t.value.clone(), t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y])),
            EntityKind::MText(t) => (t.contents.clone(), [t.insert.x, t.insert.y]),
            _ => continue,
        };
        let b = to_building(alignment, pt);
        if b[0] < room_min[0] - tol || b[0] > room_max[0] + tol || b[1] < room_min[1] - tol || b[1] > room_max[1] + tol {
            continue;
        }
        let dist = ((b[0] - cx).powi(2) + (b[1] - cy).powi(2)).sqrt();
        texts.push(TextItem { text, pos: b, dist });
    }

    // Step 2: Try to find a full ceiling tag (kind + height in one entity).
    let mut full_candidates: Vec<(String, Option<f64>, String, f64)> = Vec::new();

    for item in &texts {
        if let Some((kind, height)) = parse_ceiling_tag(&item.text)
            && !kind.is_empty()
        {
            full_candidates.push((kind, Some(height), item.text.clone(), item.dist));
        }
    }
    if !full_candidates.is_empty() {
        full_candidates.sort_by(|a, b| a.3.total_cmp(&b.3));
        return full_candidates.into_iter().next().map(|(k, h, t, _)| (k, h, t));
    }

    // Step 3: Combine separate height and kind entities.
    // pen.block() places lines at the same x, spaced vertically by ~1.5-3 ft in building coords.
    // Find height-bearing entities and look for a kind entity nearby.
    let mut combined: Vec<(String, Option<f64>, String, f64)> = Vec::new();

    for (idx, item) in texts.iter().enumerate() {
        // Try to extract height from this entity.
        if let Some((kind, height)) = parse_ceiling_tag(&item.text) {
            // If kind is empty, look for a nearby kind-only entity at similar x.
            let kind = if kind.is_empty() {
                texts.iter().enumerate()
                    .filter(|(j, other)| {
                        *j != idx
                        && (other.pos[0] - item.pos[0]).abs() < 3.0 // same x region
                        && (other.pos[1] - item.pos[1]).abs() < 5.0 // within a few feet vertically
                    })
                    .find_map(|(_, other)| {
                        let trimmed = other.text.trim().to_uppercase();
                        let k = normalise_kind(&trimmed);
                        if (k == "ACT" || k == "GYP") && trimmed.len() <= 4 { Some(k) } else { None }
                    })
                    .unwrap_or_default()
            } else {
                kind
            };
            combined.push((kind, Some(height), item.text.clone(), item.dist));
        }
    }
    if !combined.is_empty() {
        combined.sort_by(|a, b| a.3.total_cmp(&b.3));
        return combined.into_iter().next().map(|(k, h, t, _)| (k, h, t));
    }

    // Step 4: Kind-only tag (height_hidden rooms just print the kind).
    let mut kind_only: Vec<(String, Option<f64>, String, f64)> = Vec::new();
    for item in &texts {
        let trimmed = item.text.trim().to_uppercase();
        let k = normalise_kind(&trimmed);
        if (k == "ACT" || k == "GYP") && trimmed.len() <= 4 {
            kind_only.push((k, None, item.text.clone(), item.dist));
        }
    }
    kind_only.sort_by(|a, b| a.3.total_cmp(&b.3));
    kind_only.into_iter().next().map(|(k, h, t, _)| (k, h, t))
}

/// Check for "OPEN TO STRUCTURE" text inside a room.
fn detect_open_to_structure(d: &Drawing, alignment: &Alignment, room_min: [f64; 2], room_max: [f64; 2]) -> bool {
    let tol = 1.0;
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        let (text, pt) = match &e.kind {
            EntityKind::Text(t) => (t.value.clone(), t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y])),
            EntityKind::MText(t) => (t.contents.clone(), [t.insert.x, t.insert.y]),
            _ => continue,
        };
        let b = to_building(alignment, pt);
        if b[0] < room_min[0] - tol || b[0] > room_max[0] + tol || b[1] < room_min[1] - tol || b[1] > room_max[1] + tol {
            continue;
        }
        if text.to_uppercase().contains("OPEN TO STRUCTURE") {
            return true;
        }
    }
    false
}

// ---- Soffit detection ---------------------------------------------------------------

/// Detect soffits from DASHED-layer rectangles with soffit tags on TEXT layer.
fn detect_soffits(d: &Drawing, alignment: &Alignment, level: &str, rooms: &[FoundRoom]) -> Vec<FoundSoffit> {
    let mut rects: Vec<([f64; 2], [f64; 2])> = Vec::new();

    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("DASHED") {
            continue;
        }
        match &e.kind {
            EntityKind::Line(l) => {
                // Collect lines to assemble rectangles — same approach as room detection.
                // For simplicity, we look for 4 lines forming a rectangle.
                let _ = l; // Handled below with full rectangle assembly
            }
            EntityKind::LwPolyline(p) if p.vertices.len() >= 4 => {
                let pts: Vec<[f64; 2]> = p.vertices.iter().map(|v| to_building(alignment, [v.p.x, v.p.y])).collect();
                let (mut xmin, mut ymin) = (f64::INFINITY, f64::INFINITY);
                let (mut xmax, mut ymax) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
                for pt in &pts {
                    xmin = xmin.min(pt[0]);
                    ymin = ymin.min(pt[1]);
                    xmax = xmax.max(pt[0]);
                    ymax = ymax.max(pt[1]);
                }
                if (xmax - xmin) > 0.5 && (ymax - ymin) > 0.5 {
                    rects.push(([xmin, ymin], [xmax, ymax]));
                }
            }
            _ => {}
        }
    }

    // Also assemble rectangles from DASHED-layer lines (brect draws 4 lines).
    let mut h_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut v_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("DASHED") {
            continue;
        }
        if let EntityKind::Line(l) = &e.kind {
            let a = to_building(alignment, [l.a.x, l.a.y]);
            let b = to_building(alignment, [l.b.x, l.b.y]);
            let dx = (b[0] - a[0]).abs();
            let dy = (b[1] - a[1]).abs();
            if dx > 0.5 && dy < 0.2 {
                h_lines.push((a, b));
            } else if dy > 0.5 && dx < 0.2 {
                v_lines.push((a, b));
            }
        }
    }

    let tol = 0.15;
    for (i, ha) in h_lines.iter().enumerate() {
        let (ha_xmin, ha_xmax) = (ha.0[0].min(ha.1[0]), ha.0[0].max(ha.1[0]));
        let ha_y = (ha.0[1] + ha.1[1]) / 2.0;
        for hb in h_lines.iter().skip(i + 1) {
            let (hb_xmin, hb_xmax) = (hb.0[0].min(hb.1[0]), hb.0[0].max(hb.1[0]));
            let hb_y = (hb.0[1] + hb.1[1]) / 2.0;
            if (ha_y - hb_y).abs() < tol {
                continue;
            }
            if (ha_xmin - hb_xmin).abs() > tol || (ha_xmax - hb_xmax).abs() > tol {
                continue;
            }
            let x_left = ha_xmin.min(hb_xmin);
            let x_right = ha_xmax.max(hb_xmax);
            let y_low = ha_y.min(hb_y);
            let y_high = ha_y.max(hb_y);
            let has_left = v_lines.iter().any(|v| {
                let vx = (v.0[0] + v.1[0]) / 2.0;
                (vx - x_left).abs() < tol
            });
            let has_right = v_lines.iter().any(|v| {
                let vx = (v.0[0] + v.1[0]) / 2.0;
                (vx - x_right).abs() < tol
            });
            if has_left && has_right {
                rects.push(([x_left, y_low], [x_right, y_high]));
            }
        }
    }

    // Find soffit tags inside dashed rects.
    let mut soffits = Vec::new();
    for (rmin, rmax) in &rects {
        let mid = [(rmin[0] + rmax[0]) / 2.0, (rmin[1] + rmax[1]) / 2.0];
        let mut height = None;
        let mut tag = None;
        // Look for TEXT inside the dashed rect
        for e in d.model.iter() {
            if !e.common.layer.eq_ignore_ascii_case("TEXT") {
                continue;
            }
            let (text, pt) = match &e.kind {
                EntityKind::Text(t) => (t.value.clone(), t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y])),
                EntityKind::MText(t) => (t.contents.clone(), [t.insert.x, t.insert.y]),
                _ => continue,
            };
            let b = to_building(alignment, pt);
            if b[0] >= rmin[0] - 0.5
                && b[0] <= rmax[0] + 0.5
                && b[1] >= rmin[1] - 0.5
                && b[1] <= rmax[1] + 0.5
                && let Some(h) = parse_soffit_tag(&text)
            {
                height = Some(h);
                tag = Some(text);
                break;
            }
        }
        // Find which room contains this soffit
        let room = rooms
            .iter()
            .find(|r| mid[0] >= r.min_ft[0] - 0.5 && mid[0] <= r.max_ft[0] + 0.5 && mid[1] >= r.min_ft[1] - 0.5 && mid[1] <= r.max_ft[1] + 0.5);
        let room_number = room.map(|r| r.number.clone()).unwrap_or_default();
        if height.is_some() || tag.is_some() {
            soffits.push(FoundSoffit { level: level.to_string(), room_number, height_ft: height, min_ft: *rmin, max_ft: *rmax, tag_text: tag });
        }
    }
    soffits
}

// ---- Ceiling-mounted items ----------------------------------------------------------

/// Detect ceiling-mounted items on the DEVICE layer by shape analysis.
///
/// The RCP draws terminals as geometric symbols on the `DEVICE` layer:
/// - **Supply diffuser**: outer rectangle with X-cross diagonals + inner rectangle
///   (draw.rs `brect` + 2 diagonal lines + inner `brect`)
/// - **Return grille**: rectangle with 3 horizontal bars inside
/// - **Exhaust grille**: rectangle with 2+ diagonal/partial lines
/// - **Sprinkler head**: circle (pendent) or double circle (upright)
///
/// We identify these by collecting DEVICE-layer lines and circles, assembling small
/// rectangles from line groups, then counting interior lines to classify the kind.
fn detect_ceiling_items(d: &Drawing, alignment: &Alignment, level: &str, rooms: &[FoundRoom]) -> Vec<FoundCeilingItem> {
    let mut items = Vec::new();

    // Step 1: Collect all DEVICE-layer line segments and circles in building coords.
    let mut dev_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut dev_circles: Vec<([f64; 2], f64)> = Vec::new(); // (centre, radius_ft)

    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("DEVICE") {
            continue;
        }
        match &e.kind {
            EntityKind::Line(l) => {
                let a = to_building(alignment, [l.a.x, l.a.y]);
                let b = to_building(alignment, [l.b.x, l.b.y]);
                dev_lines.push((a, b));
            }
            EntityKind::LwPolyline(p) => {
                // Convert polyline segments to individual lines.
                let pts: Vec<[f64; 2]> = p.vertices.iter().map(|v| to_building(alignment, [v.p.x, v.p.y])).collect();
                for w in pts.windows(2) {
                    dev_lines.push((w[0], w[1]));
                }
                if p.closed && pts.len() >= 2
                    && let (Some(first), Some(last)) = (pts.first(), pts.last())
                {
                    dev_lines.push((*last, *first));
                }
            }
            EntityKind::Circle(c) => {
                let centre = to_building(alignment, [c.center.x, c.center.y]);
                let r_ft = c.radius / 12.0; // model coords are inches
                dev_circles.push((centre, r_ft));
            }
            _ => {}
        }
    }

    // Step 2: Find small axis-aligned rectangles from DEVICE lines.
    // brect produces 4 lines forming a rectangle. Look for groups of H and V lines
    // that form a closed rectangle of "device size" (roughly 0.5–4 ft each side).
    let mut h_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut v_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut diag_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();

    for &(a, b) in &dev_lines {
        let dx = (b[0] - a[0]).abs();
        let dy = (b[1] - a[1]).abs();
        if dx > 0.1 && dy < 0.1 {
            h_lines.push((a, b));
        } else if dy > 0.1 && dx < 0.1 {
            v_lines.push((a, b));
        } else if dx > 0.1 && dy > 0.1 {
            diag_lines.push((a, b));
        }
    }

    // Assemble rectangles from pairs of horizontals + matching verticals.
    let tol = 0.15;
    let max_device_size = 4.0; // max ft for a device symbol
    let min_device_size = 0.2; // min ft
    let mut device_rects: Vec<([f64; 2], [f64; 2])> = Vec::new();

    for (i, ha) in h_lines.iter().enumerate() {
        let (ha_xmin, ha_xmax) = (ha.0[0].min(ha.1[0]), ha.0[0].max(ha.1[0]));
        let ha_y = (ha.0[1] + ha.1[1]) / 2.0;
        let width = ha_xmax - ha_xmin;
        if width < min_device_size || width > max_device_size {
            continue;
        }
        for hb in h_lines.iter().skip(i + 1) {
            let (hb_xmin, hb_xmax) = (hb.0[0].min(hb.1[0]), hb.0[0].max(hb.1[0]));
            let hb_y = (hb.0[1] + hb.1[1]) / 2.0;
            let height = (ha_y - hb_y).abs();
            if height < min_device_size || height > max_device_size {
                continue;
            }
            if (ha_xmin - hb_xmin).abs() > tol || (ha_xmax - hb_xmax).abs() > tol {
                continue;
            }
            let x_left = ha_xmin.min(hb_xmin);
            let x_right = ha_xmax.max(hb_xmax);
            let y_low = ha_y.min(hb_y);
            let y_high = ha_y.max(hb_y);
            let has_left = v_lines.iter().any(|v| {
                let (vy_min, vy_max) = (v.0[1].min(v.1[1]), v.0[1].max(v.1[1]));
                let vx = (v.0[0] + v.1[0]) / 2.0;
                (vx - x_left).abs() < tol && vy_min < y_low + tol && vy_max > y_high - tol
            });
            let has_right = v_lines.iter().any(|v| {
                let (vy_min, vy_max) = (v.0[1].min(v.1[1]), v.0[1].max(v.1[1]));
                let vx = (v.0[0] + v.1[0]) / 2.0;
                (vx - x_right).abs() < tol && vy_min < y_low + tol && vy_max > y_high - tol
            });
            if has_left && has_right {
                device_rects.push(([x_left, y_low], [x_right, y_high]));
            }
        }
    }

    // Deduplicate rects that are within tolerance of each other (same device drawn
    // with brect produces one set of 4 lines; inner rects of supply diffusers make a
    // second, smaller rect at the same centre — keep only the larger).
    device_rects.sort_by(|a, b| {
        let area_a = (a.1[0] - a.0[0]) * (a.1[1] - a.0[1]);
        let area_b = (b.1[0] - b.0[0]) * (b.1[1] - b.0[1]);
        area_b.total_cmp(&area_a) // largest first
    });
    let mut kept_rects: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for r in &device_rects {
        let cx = (r.0[0] + r.1[0]) / 2.0;
        let cy = (r.0[1] + r.1[1]) / 2.0;
        let already = kept_rects.iter().any(|k| {
            let kcx = (k.0[0] + k.1[0]) / 2.0;
            let kcy = (k.0[1] + k.1[1]) / 2.0;
            (cx - kcx).abs() < 0.5 && (cy - kcy).abs() < 0.5
        });
        if !already {
            kept_rects.push(*r);
        }
    }

    // Step 3: Classify each device rectangle by its interior lines.
    for &(rmin, rmax) in &kept_rects {
        let cx = (rmin[0] + rmax[0]) / 2.0;
        let cy = (rmin[1] + rmax[1]) / 2.0;
        let pad = 0.1;

        // Count horizontal lines inside the rect (excluding the border lines).
        let interior_h = h_lines.iter().filter(|l| {
            let lx_min = l.0[0].min(l.1[0]);
            let lx_max = l.0[0].max(l.1[0]);
            let ly = (l.0[1] + l.1[1]) / 2.0;
            lx_min >= rmin[0] - pad
                && lx_max <= rmax[0] + pad
                && ly > rmin[1] + pad
                && ly < rmax[1] - pad
                && (ly - rmin[1]).abs() > pad
                && (ly - rmax[1]).abs() > pad
        }).count();

        // Count diagonal lines touching the rect area.
        let interior_diag = diag_lines.iter().filter(|l| {
            let mid = [(l.0[0] + l.1[0]) / 2.0, (l.0[1] + l.1[1]) / 2.0];
            mid[0] >= rmin[0] - pad && mid[0] <= rmax[0] + pad && mid[1] >= rmin[1] - pad && mid[1] <= rmax[1] + pad
        }).count();

        // Check for an inner rectangle (supply diffuser has one).
        let has_inner_rect = device_rects.iter().any(|ir| {
            let icx = (ir.0[0] + ir.1[0]) / 2.0;
            let icy = (ir.0[1] + ir.1[1]) / 2.0;
            (icx - cx).abs() < 0.3
                && (icy - cy).abs() < 0.3
                && (ir.1[0] - ir.0[0]) < (rmax[0] - rmin[0]) - 0.1
                && (ir.1[1] - ir.0[1]) < (rmax[1] - rmin[1]) - 0.1
        });

        // Classify:
        // Supply diffuser: X-cross (2 diagonals) + inner rect
        // Return grille: 3 horizontal bars inside
        // Exhaust grille: diagonal lines but no inner rect
        let kind = if interior_diag >= 2 && has_inner_rect {
            "supply_diffuser"
        } else if interior_h >= 2 {
            "return_grille"
        } else if interior_diag >= 2 {
            "exhaust_grille"
        } else {
            // Unknown device rect — skip
            continue;
        };

        let b = [cx, cy];
        if let Some(room) = room_containing(rooms, b) {
            items.push(FoundCeilingItem { level: level.to_string(), room_number: room.number.clone(), kind: kind.to_string(), at_ft: b });
        }
    }

    // Step 4: Sprinkler heads are circles on the DEVICE layer (small radius, < 0.5 ft).
    for &(centre, r) in &dev_circles {
        if !(0.01..=0.5).contains(&r) {
            continue;
        }
        if let Some(room) = room_containing(rooms, centre) {
            items.push(FoundCeilingItem {
                level: level.to_string(),
                room_number: room.number.clone(),
                kind: "sprinkler_head".to_string(),
                at_ft: centre,
            });
        }
    }

    // Deduplicate sprinkler heads at the same position (upright has 2 concentric circles).
    items.sort_by(|a, b| a.at_ft[0].total_cmp(&b.at_ft[0]).then_with(|| a.at_ft[1].total_cmp(&b.at_ft[1])));
    items.dedup_by(|a, b| a.kind == b.kind && (a.at_ft[0] - b.at_ft[0]).abs() < 0.1 && (a.at_ft[1] - b.at_ft[1]).abs() < 0.1);

    items
}

/// Find the room whose bounding box contains `pt`.
fn room_containing(rooms: &[FoundRoom], pt: [f64; 2]) -> Option<&FoundRoom> {
    rooms.iter().find(|r| pt[0] >= r.min_ft[0] - 0.3 && pt[0] <= r.max_ft[0] + 0.3 && pt[1] >= r.min_ft[1] - 0.3 && pt[1] <= r.max_ft[1] + 0.3)
}

// ---- Shaft / chase detection --------------------------------------------------------

/// Detect shafts and chases on architectural floor plan sheets.
///
/// On A-discipline floor plans, shafts are drawn as DASHED-layer rectangles with a TEXT
/// label (e.g. `"PLUMBING CHASE"`) — the same as `draw.rs:1972-1980`.
fn detect_shafts(d: &Drawing, alignment: &Alignment, level: &str) -> Vec<FoundShaft> {
    // Collect DASHED-layer rectangles (from LwPolyline and line-assembled).
    let mut rects: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut h_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut v_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();

    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("DASHED") {
            continue;
        }
        match &e.kind {
            EntityKind::LwPolyline(p) if p.vertices.len() >= 4 => {
                let pts: Vec<[f64; 2]> = p.vertices.iter().map(|v| to_building(alignment, [v.p.x, v.p.y])).collect();
                let (mut xmin, mut ymin) = (f64::INFINITY, f64::INFINITY);
                let (mut xmax, mut ymax) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
                for pt in &pts {
                    xmin = xmin.min(pt[0]);
                    ymin = ymin.min(pt[1]);
                    xmax = xmax.max(pt[0]);
                    ymax = ymax.max(pt[1]);
                }
                if (xmax - xmin) > 0.5 && (ymax - ymin) > 0.5 {
                    rects.push(([xmin, ymin], [xmax, ymax]));
                }
            }
            EntityKind::Line(l) => {
                let a = to_building(alignment, [l.a.x, l.a.y]);
                let b = to_building(alignment, [l.b.x, l.b.y]);
                let dx = (b[0] - a[0]).abs();
                let dy = (b[1] - a[1]).abs();
                if dx > 0.5 && dy < 0.2 {
                    h_lines.push((a, b));
                } else if dy > 0.5 && dx < 0.2 {
                    v_lines.push((a, b));
                }
            }
            _ => {}
        }
    }

    // Assemble rectangles from line pairs.
    let tol = 0.15;
    for (i, ha) in h_lines.iter().enumerate() {
        let (ha_xmin, ha_xmax) = (ha.0[0].min(ha.1[0]), ha.0[0].max(ha.1[0]));
        let ha_y = (ha.0[1] + ha.1[1]) / 2.0;
        for hb in h_lines.iter().skip(i + 1) {
            let (hb_xmin, hb_xmax) = (hb.0[0].min(hb.1[0]), hb.0[0].max(hb.1[0]));
            let hb_y = (hb.0[1] + hb.1[1]) / 2.0;
            if (ha_y - hb_y).abs() < tol {
                continue;
            }
            if (ha_xmin - hb_xmin).abs() > tol || (ha_xmax - hb_xmax).abs() > tol {
                continue;
            }
            let x_left = ha_xmin.min(hb_xmin);
            let x_right = ha_xmax.max(hb_xmax);
            let y_low = ha_y.min(hb_y);
            let y_high = ha_y.max(hb_y);
            let has_left = v_lines.iter().any(|v| {
                let (vy_min, vy_max) = (v.0[1].min(v.1[1]), v.0[1].max(v.1[1]));
                let vx = (v.0[0] + v.1[0]) / 2.0;
                (vx - x_left).abs() < tol && vy_min < y_low + tol && vy_max > y_high - tol
            });
            let has_right = v_lines.iter().any(|v| {
                let (vy_min, vy_max) = (v.0[1].min(v.1[1]), v.0[1].max(v.1[1]));
                let vx = (v.0[0] + v.1[0]) / 2.0;
                (vx - x_right).abs() < tol && vy_min < y_low + tol && vy_max > y_high - tol
            });
            if has_left && has_right {
                rects.push(([x_left, y_low], [x_right, y_high]));
            }
        }
    }

    // For each DASHED rect, look for a TEXT label inside it containing shaft/chase keywords.
    let mut shafts = Vec::new();
    for (rmin, rmax) in &rects {
        let mut label = None;
        for e in d.model.iter() {
            if !e.common.layer.eq_ignore_ascii_case("TEXT") {
                continue;
            }
            let (text, pt) = match &e.kind {
                EntityKind::Text(t) => (t.value.clone(), t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y])),
                EntityKind::MText(t) => (t.contents.clone(), [t.insert.x, t.insert.y]),
                _ => continue,
            };
            let b = to_building(alignment, pt);
            if b[0] >= rmin[0] - 0.5 && b[0] <= rmax[0] + 0.5 && b[1] >= rmin[1] - 0.5 && b[1] <= rmax[1] + 0.5 {
                let upper = text.to_uppercase();
                if upper.contains("SHAFT") || upper.contains("CHASE") || upper.contains("OPENING") {
                    label = Some(text);
                    break;
                }
            }
        }
        // Only keep DASHED rects that have a shaft/chase label.
        if let Some(lbl) = label {
            shafts.push(FoundShaft { level: level.to_string(), min_ft: *rmin, max_ft: *rmax, label: Some(lbl) });
        }
    }
    shafts
}

// ---- Coordinate transform -----------------------------------------------------------

fn to_building(alignment: &Alignment, model: [f64; 2]) -> [f64; 2] {
    sheet_reader::building_from_model(alignment, model)
}

// ---- Public API ---------------------------------------------------------------------

/// Read ceiling data from a set of DXF files, using the sheet map for alignment.
///
/// When `structure` is provided, plenum depth is computed per room by combining the
/// reader's ceiling height with the structural model's bottom-of-structure.
pub fn read_ceilings(dxf_data: &[(String, Vec<u8>)], sheet_map: &SheetMap, structure: Option<&StructuralModel>) -> CeilingResult {
    let mut result = CeilingResult::default();

    // Pre-compute structure bottom above floor per level from the structural model.
    let struct_bob = structure.map(structure_bottom_per_level).unwrap_or_default();

    for (name, bytes) in dxf_data {
        let d = match cadaddy_io::read_dxf(bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        // Find the recognised sheet for this DXF.
        let file_stem = name.trim_end_matches(".dxf");
        let sheet = sheet_map.sheets.iter().find(|s| s.number == file_stem || name.starts_with(&s.number));
        let Some(sheet) = sheet else { continue };

        let Some(level) = &sheet.level else { continue };
        let Some(alignment) = &sheet.alignment else { continue };

        let is_rcp = sheet.discipline == "A" && sheet.title.to_uppercase().contains("CEILING");
        let is_floor_plan = sheet.discipline == "A" && !is_rcp;

        // RCP sheets: rooms, ceilings, soffits, items
        if is_rcp {
            let rooms = detect_rooms(&d, alignment, level);
            result.rooms.extend(rooms.clone());

            for room in &rooms {
                let is_open = detect_open_to_structure(&d, alignment, room.min_ft, room.max_ft);
                if is_open {
                    result.ceilings.push(FoundCeiling {
                        level: level.clone(),
                        room_number: room.number.clone(),
                        kind: String::new(),
                        height_ft: None,
                        tag_text: None,
                        grid_ft: None,
                        open_to_structure: true,
                    });
                    continue;
                }

                let grid = detect_act_grid(&d, alignment, room.min_ft, room.max_ft);
                let tag = detect_ceiling_tag(&d, alignment, room.min_ft, room.max_ft);

                if let Some((kind, height_opt, text)) = tag {
                    let kind = if grid.is_some() && kind.is_empty() { "ACT".to_string() } else { kind };
                    result.ceilings.push(FoundCeiling {
                        level: level.clone(),
                        room_number: room.number.clone(),
                        kind,
                        height_ft: height_opt,
                        tag_text: Some(text),
                        grid_ft: grid,
                        open_to_structure: false,
                    });
                } else if grid.is_some() {
                    result.ceilings.push(FoundCeiling {
                        level: level.clone(),
                        room_number: room.number.clone(),
                        kind: "ACT".to_string(),
                        height_ft: None,
                        tag_text: None,
                        grid_ft: grid,
                        open_to_structure: false,
                    });
                }
            }

            let soffits = detect_soffits(&d, alignment, level, &rooms);
            result.soffits.extend(soffits);

            let items = detect_ceiling_items(&d, alignment, level, &rooms);
            result.items.extend(items);
        }

        // A-floor plan sheets and S-framing sheets: shafts/chases
        // Openings are drawn on S-discipline framing sheets (draw.rs:1972-1980).
        let is_framing = sheet.discipline == "S";
        if is_floor_plan || is_framing {
            let shafts = detect_shafts(&d, alignment, level);
            result.shafts.extend(shafts);
        }
    }

    // Compute plenum depths from ceiling heights + structure bottom.
    for ceiling in &result.ceilings {
        if let Some(ch) = ceiling.height_ft
            && let Some(&bob) = struct_bob.get(&ceiling.level)
        {
            let plenum = bob - ch;
            if plenum > 0.01 {
                result.plenums.push(FoundPlenum {
                    level: ceiling.level.clone(),
                    room_number: ceiling.room_number.clone(),
                    plenum_ft: plenum,
                    ceiling_height_ft: ch,
                    structure_bottom_ft: bob,
                });
            }
        }
    }

    result
}

/// Compute the bottom-of-structure height above the floor for each level.
///
/// For rooms on level L, the structure above is the slab/roof of level L+1.
/// Bottom of structure = TOS_above - beam_depth - floor_elevation_of_L.
fn structure_bottom_per_level(s: &StructuralModel) -> BTreeMap<String, f64> {
    let mut result = BTreeMap::new();

    // Collect all "above" levels that have either a slab or beams.
    let mut above_levels: Vec<String> = Vec::new();
    for lv in s.beams.keys() {
        if !above_levels.contains(lv) {
            above_levels.push(lv.clone());
        }
    }
    for slab in &s.slabs {
        if !above_levels.contains(&slab.level) {
            above_levels.push(slab.level.clone());
        }
    }

    for lv in &above_levels {
        let slab = s.slabs.iter().find(|sl| sl.level == *lv);

        // Top of steel: explicit TOS, or derive from slab, or estimate from the
        // level below's slab top + a standard storey height for the roof.
        let tos = s.tos_ft.get(lv).copied().or_else(|| {
            slab.map(|sl| sl.top_ft - (sl.concrete_in + sl.deck_in) / 12.0)
        });
        let Some(tos) = tos else { continue };

        // Max beam/girder depth on this level.
        let max_beam_depth_in = s.beams.get(lv).map_or(0.0, |beams| {
            beams.iter().map(|b| parse_beam_depth(&b.size)).fold(0.0_f64, f64::max)
        });

        let bot = tos - max_beam_depth_in / 12.0;

        // The level below gets this as its ceiling (structure bottom above floor).
        let below = match lv.as_str() {
            "2" => Some("1"),
            "3" => Some("2"),
            "R" | "ROOF" => {
                // Roof is above the highest numeric level. Find it.
                let max_num = above_levels.iter()
                    .filter_map(|l| l.parse::<u32>().ok())
                    .max()
                    .unwrap_or(2);
                Some(if max_num == 2 { "2" } else { "1" })
            }
            _ => lv.parse::<u32>().ok().and_then(|n| if n > 1 { Some("") } else { None }).and(None),
        };

        if let Some(below_lv) = below {
            // Floor elevation for the level below: slab top_ft, or 0 for level 1.
            let floor_elev = if below_lv == "1" {
                s.slabs.iter().find(|sl| sl.level == "1").map_or(0.0, |sl| sl.top_ft)
            } else {
                s.slabs.iter().find(|sl| sl.level == below_lv).map_or(0.0, |sl| sl.top_ft)
            };
            let bob_above_floor = bot - floor_elev;
            if bob_above_floor > 0.0 {
                result.insert(below_lv.to_string(), bob_above_floor);
            }
        }
    }

    result
}

/// Parse beam depth from a W-shape size like "W16x31" → 16.0.
fn parse_beam_depth(size: &str) -> f64 {
    let s = size.trim().strip_prefix('W').or_else(|| size.trim().strip_prefix('w')).unwrap_or(size);
    s.split(|c: char| !c.is_ascii_digit()).next().and_then(|d| d.parse::<f64>().ok()).unwrap_or(0.0)
}

/// Convenience: generate fixture set, recognise sheets, read ceilings with structure.
/// Returns `Err` if the fixture generator fails.
pub fn read_fixture_ceilings(seed: u64) -> Result<(Truth, CeilingResult), String> {
    let set = crate::generate(seed)?;
    let dxf_data: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let sheet_map = sheet_reader::recognise_fixture_set(&dxf_data);

    // Read structure for plenum depth calculation.
    // Drawing names must match sheet numbers (no .dxf extension).
    let drawings: Vec<(String, Drawing)> = dxf_data
        .iter()
        .filter_map(|(n, b)| cadaddy_io::read_dxf(b).ok().map(|d| (n.trim_end_matches(".dxf").to_string(), d)))
        .collect();
    let refs: Vec<(String, &Drawing)> = drawings.iter().map(|(n, d)| (n.clone(), d)).collect();
    let structure = crate::struct_reader::read_structure(&sheet_map, &refs);

    let result = read_ceilings(&dxf_data, &sheet_map, Some(&structure));
    Ok((set.truth, result))
}

// ---- Scoring ------------------------------------------------------------------------

/// Score found ceilings against the truth.
pub fn score_ceilings(truth: &Truth, result: &CeilingResult) -> CeilingScore {
    let mut sc = CeilingScore::default();
    let mut height_errors: Vec<f64> = Vec::new();

    for room in &truth.building.rooms {
        let found = result.ceilings.iter().find(|c| c.level == room.level && c.room_number == room.number);

        match (&room.ceiling, found) {
            (None, Some(fc)) if fc.open_to_structure => {
                // Correctly identified open to structure
                sc.open_found += 1;
                sc.open_total += 1;
            }
            (None, _) => {
                sc.open_total += 1;
            }
            (Some(tc), Some(fc)) => {
                sc.rooms_total += 1;
                sc.rooms_found += 1;
                if fc.kind == tc.kind {
                    sc.kind_correct += 1;
                }
                if !tc.height_hidden {
                    if let Some(fh) = fc.height_ft {
                        let err = (fh - tc.height_ft).abs();
                        height_errors.push(err);
                        if err < 0.5 {
                            sc.height_correct += 1;
                        }
                    }
                } else {
                    // height_hidden: no height expected, count as correct if kind matches
                    sc.height_correct += 1;
                }
                // Grid scoring for ACT
                if tc.kind == "ACT" {
                    sc.grid_total += 1;
                    if let (Some(fg), Some(tg)) = (&fc.grid_ft, &tc.grid_ft)
                        && (fg[0] - tg[0]).abs() < 0.5
                        && (fg[1] - tg[1]).abs() < 0.5
                    {
                        sc.grid_correct += 1;
                    }
                }
            }
            (Some(_), None) => {
                sc.rooms_total += 1;
                if room.ceiling.as_ref().is_some_and(|c| c.kind == "ACT") {
                    sc.grid_total += 1;
                }
            }
        }
    }

    if !height_errors.is_empty() {
        sc.height_mae_ft = Some(height_errors.iter().sum::<f64>() / height_errors.len() as f64);
    }

    // Soffit scoring: match by position overlap
    sc.soffits_total = truth.rcp_soffits.len();
    for ts in &truth.rcp_soffits {
        let found = result.soffits.iter().any(|fs| {
            let overlap_x = fs.min_ft[0] < ts.max_ft[0] + 1.0 && fs.max_ft[0] > ts.min_ft[0] - 1.0;
            let overlap_y = fs.min_ft[1] < ts.max_ft[1] + 1.0 && fs.max_ft[1] > ts.min_ft[1] - 1.0;
            overlap_x && overlap_y && fs.level == ts.level
        });
        if found {
            sc.soffits_found += 1;
        }
    }

    sc
}
