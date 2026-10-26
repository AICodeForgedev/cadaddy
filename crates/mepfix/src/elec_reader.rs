//! Electrical device reader: recognises device symbols on E-sheets (lighting and power plans),
//! attaches types and circuit tags from the legend, places each device at its mounting height
//! (from notes on the device, the legend, or a default rules file), and records the height
//! source so guesses are visible. Position comes from architectural sheets where they fix it,
//! else from placement rules as a zone (flagged field-decision).
//!
//! 3D output: simple shapes on per-type layers (`3D-ELEC-<kind>`); IFC types are
//! IfcLightFixture / IfcOutlet / IfcSwitchingDevice / IfcJunctionBox.

use crate::layout;
use crate::sheet_reader::{Alignment, SheetMap};
use crate::truth::{Room, Truth};
use cadaddy_doc::{Drawing, EntityKind};
use std::collections::BTreeMap;

// ---- Zone type: names the wall, range along it, height band, and decision status --------

/// Which axis a zone range runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneAxis {
    /// Range is along the X axis (south or north wall).
    X,
    /// Range is along the Y axis (west or east wall).
    Y,
}

/// A placement zone for a device whose exact position is not fixed by architectural documents.
/// Names the wall (room + side), the allowed range along it, a height band, and whether the
/// installer decides the exact spot in the field.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceZone {
    /// Room this zone belongs to.
    pub room: String,
    /// Which wall side of the room.
    pub wall_side: WallSide,
    /// Which axis the range runs along.
    pub axis: ZoneAxis,
    /// Allowed range along the wall axis, building feet `[min, max]`.
    pub range_ft: [f64; 2],
    /// Mounting height band, inches AFF `[min, max]`.
    pub height_in: [f64; 2],
    /// True when the installer decides the exact spot in the field (no drawing fixes it).
    pub field_decision: bool,
}

/// Whether a layer name is a device layer (DEVICE, E-DEVICE, etc.).
fn is_device_layer(layer: &str) -> bool {
    layer.to_ascii_uppercase().contains("DEVICE")
}

// ---- Default mounting height rules (inches AFF) ----------------------------------------

/// Default mounting height rules by device kind, used when no note is present on the device.
fn default_mounting_in(kind: &str) -> f64 {
    match kind {
        "receptacle" | "data_outlet" => 18.0,
        "switch" => 46.0,
        "disconnect" => 60.0,
        "emergency_light" => 84.0,
        "floor_box" => 0.0,
        // Ceiling-mounted by default.
        "light_fixture" | "exit_sign" | "junction_box" => 0.0, // 0 = ceiling
        _ => 18.0,
    }
}

/// Whether a device kind is typically ceiling-mounted.
fn is_ceiling_mount(kind: &str) -> bool {
    matches!(kind, "light_fixture" | "exit_sign" | "junction_box")
}

// ---- Legend parsing ---------------------------------------------------------------------

/// A legend entry parsed from the E-sheet: symbol description and associated device kind.
#[derive(Clone, Debug)]
pub struct LegendEntry {
    pub symbol: String,
    pub description: String,
    pub kind: String,
}

/// Classify a legend description text into a device kind.
pub fn classify_legend(sym: &str, desc: &str) -> Option<String> {
    let upper = desc.to_uppercase();
    let su = sym.to_uppercase();
    // Disconnect must be checked before switch (a disconnect switch contains both words).
    if upper.contains("DISCONNECT") || su == "DS" || su.starts_with("DS-") {
        return Some("disconnect".into());
    }
    if upper.contains("TROFFER") || upper.contains("DOWNLIGHT") || upper.contains("SCONCE") || upper.contains("PENDANT") || upper.contains("FIXTURE")
    {
        return Some("light_fixture".into());
    }
    if upper.contains("SWITCH") || su == "S" || su.starts_with("S-") {
        return Some("switch".into());
    }
    if upper.contains("EXIT") {
        return Some("exit_sign".into());
    }
    if upper.contains("EMERGENCY") {
        return Some("emergency_light".into());
    }
    if upper.contains("RECEPTACLE") || su == "R" {
        return Some("receptacle".into());
    }
    if upper.contains("DATA") || upper.contains("TELECOM") || su == "D" {
        return Some("data_outlet".into());
    }
    if upper.contains("FLOOR BOX") || su == "FB" {
        return Some("floor_box".into());
    }
    if upper.contains("JUNCTION") || su == "JB" || su.starts_with("JB-") {
        return Some("junction_box".into());
    }
    if upper.contains("PANEL") || upper.contains("PANELBOARD") {
        return Some("panel".into());
    }
    None
}

// ---- Symbol geometry recognition -------------------------------------------------------

/// A detected device on the sheet (model coordinates).
#[derive(Clone, Debug)]
pub struct DetectedDevice {
    /// Position in model inches.
    pub at_in: [f64; 2],
    /// Device kind inferred from the symbol shape and/or legend.
    pub kind: String,
    /// Type tag from nearby text (fixture type letter, `S`, `R`, `D`, etc.).
    pub type_tag: String,
    /// Mounting height in inches AFF, from a note on the device or from rules.
    pub mounting_in: f64,
    /// How the mounting height was determined: `"note"`, `"legend"`, or `"rule"`.
    pub height_source: String,
    /// Mounting note text if found on the sheet.
    pub mounting_note: Option<String>,
    /// Circuit label from a nearby homerun arrow, if found.
    pub circuit_label: Option<String>,
    /// Every text near the symbol (subscripts, the circuit beside it, notes), as printed.
    pub notes: Vec<String>,
}

/// Words printed beside a device that say what it is or how it's fed, not its type tag:
/// subscripts (GFCI, WR, USB…) and a circuit (`P1-5`).
pub fn is_subscript_or_circuit(t: &str) -> bool {
    let u = t.trim().to_ascii_uppercase();
    let words: Vec<&str> = u.split_whitespace().collect();
    let sub = |w: &str| matches!(w, "GFCI" | "GFI" | "WR" | "WP" | "USB" | "IG" | "DED" | "TV" | "CH" | "AFCI" | "TR" | "DATA");
    if !words.is_empty() && words.iter().all(|w| sub(w)) {
        return true;
    }
    // A circuit: panel tag, dash, numbers (`P1-5`, `LP2-12,14`).
    u.split_once('-').is_some_and(|(p, c)| {
        (1..=4).contains(&p.len())
            && p.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic())
            && p.chars().any(|ch| ch.is_ascii_digit())
            && !c.is_empty()
            && c.chars().all(|ch| ch.is_ascii_digit() || ch == ',')
    })
}

/// A detected panel on the sheet.
#[derive(Clone, Debug)]
pub struct DetectedPanel {
    pub at_in: [f64; 2],
    pub tag: String,
    pub kind: String,
}

/// Device-layer strokes longer than this (model inches) are runs, not symbol parts.
const MAX_SYMBOL_STROKE_IN: f64 = 24.0;
/// Geometry within this distance (model inches) is one symbol.
const SYMBOL_RADIUS_IN: f64 = 12.0;
/// A symbol's own texts are within this distance of it (model inches).
const CLOSE_TEXT_IN: f64 = 12.0;

/// Public entry point for BOM and other callers that need device detection on a drawing.
pub fn detect_device_symbols_pub(d: &Drawing) -> Vec<DetectedDevice> {
    detect_device_symbols(d)
}

/// Detect device symbols on a DEVICE layer in the DXF.
///
/// Strategy: on E-sheets, devices are drawn as circles, rectangles, arcs, polygons on the
/// DEVICE layer; nearby text on TEXT gives tags and mounting notes. We cluster DEVICE-layer
/// geometry into symbol groups, then classify each by shape and nearby text.
fn detect_device_symbols(d: &Drawing) -> Vec<DetectedDevice> {
    // Collect all DEVICE-layer geometry centres.
    let mut centres: Vec<([f64; 2], &str)> = Vec::new(); // (centre, shape hint)
    for e in d.model.iter() {
        if !is_device_layer(&e.common.layer) {
            continue;
        }
        match &e.kind {
            EntityKind::Circle(c) => {
                centres.push(([c.center.x, c.center.y], "circle"));
            }
            EntityKind::Line(l) => {
                let (dx, dy) = ((l.b.x - l.a.x).abs(), (l.b.y - l.a.y).abs());
                // Conduit and other runs are long; a symbol's strokes are short.
                if dx.hypot(dy) > MAX_SYMBOL_STROKE_IN {
                    continue;
                }
                let cx = (l.a.x + l.b.x) / 2.0;
                let cy = (l.a.y + l.b.y) / 2.0;
                let dir = if dy <= dx * 0.1 {
                    "line_h"
                } else if dx <= dy * 0.1 {
                    "line_v"
                } else {
                    "line_d"
                };
                centres.push(([cx, cy], dir));
            }
            EntityKind::LwPolyline(p) => {
                if p.vertices.is_empty() {
                    continue;
                }
                let (mut sx, mut sy) = (0.0, 0.0);
                for v in &p.vertices {
                    sx += v.p.x;
                    sy += v.p.y;
                }
                let n = p.vertices.len() as f64;
                // Corners: a closed triangle (data outlet) or rectangle (troffer, box).
                let corners = if p.closed { p.vertices.len() } else { p.vertices.len().saturating_sub(1) };
                centres.push((
                    [sx / n, sy / n],
                    if corners == 3 {
                        "poly3"
                    } else if corners == 4 {
                        "poly4"
                    } else {
                        "poly"
                    },
                ));
            }
            EntityKind::Arc(a) => {
                centres.push(([a.center.x, a.center.y], "arc"));
            }
            _ => {}
        }
    }

    // Collect all TEXT-layer text with positions.
    let mut texts: Vec<(String, [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("TEXT") && !is_device_layer(&e.common.layer) {
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

    // Cluster DEVICE geometry into symbol groups (nearby geometry = one symbol).
    // Use a simple radius-based clustering.
    let mut used = vec![false; centres.len()];
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for i in 0..centres.len() {
        if used[i] {
            continue;
        }
        let mut group = vec![i];
        used[i] = true;
        // Gather nearby geometry (within a small radius).
        for j in (i + 1)..centres.len() {
            if used[j] {
                continue;
            }
            let dx = centres[i].0[0] - centres[j].0[0];
            let dy = centres[i].0[1] - centres[j].0[1];
            let dist = (dx * dx + dy * dy).sqrt();
            // Symbols are compact (a few inches across); neighbouring devices are feet apart.
            if dist < SYMBOL_RADIUS_IN {
                // This is in model inches; symbols are small.
                group.push(j);
                used[j] = true;
            }
        }
        clusters.push(group);
    }

    // For each cluster, find the centroid and classify.
    let mut devices = Vec::new();
    for cluster in &clusters {
        if cluster.is_empty() {
            continue;
        }
        let (mut sx, mut sy) = (0.0, 0.0);
        let mut has_circle = false;
        let mut has_poly = false;
        let mut has_arc = false;
        let count = cluster.len();
        let (mut n_h, mut n_v, mut tri, mut rect) = (0usize, 0usize, false, false);
        let mut diag: Vec<[f64; 2]> = Vec::new();
        let mut rect_at = [0.0, 0.0];
        for &idx in cluster {
            let Some((at, hint)) = centres.get(idx) else { continue };
            sx += at[0];
            sy += at[1];
            match *hint {
                "circle" => has_circle = true,
                "poly" => has_poly = true,
                "poly3" => {
                    has_poly = true;
                    tri = true;
                }
                "poly4" => {
                    has_poly = true;
                    rect = true;
                    rect_at = *at;
                }
                "arc" => has_arc = true,
                "line_h" => n_h += 1,
                "line_v" => n_v += 1,
                "line_d" => diag.push(*at),
                _ => {}
            }
        }
        // An exit sign's X: two diagonals crossing at its box's middle (a homerun's shaft
        // starts beside a device, not across it).
        let crossed = rect && diag.iter().filter(|m| (m[0] - rect_at[0]).hypot(m[1] - rect_at[1]) < 1.0).count() >= 2;
        // Bare strokes with a diagonal among them (no circle, box or arc): a homerun's arrow
        // and tick, not a device.
        if !has_circle && !has_poly && !has_arc && !diag.is_empty() {
            continue;
        }
        let cx = sx / count as f64;
        let cy = sy / count as f64;

        // Find nearby text (within search radius).
        let search_r = 80.0; // model inches
        let nearby: Vec<&(String, [f64; 2])> = texts
            .iter()
            .filter(|(_, p)| {
                let dx = p[0] - cx;
                let dy = p[1] - cy;
                (dx * dx + dy * dy).sqrt() < search_r
            })
            .collect();

        // Classify by text hints and shape.
        let text_up: Vec<String> = nearby.iter().map(|(t, _)| t.to_uppercase()).collect();

        // Skip if this looks like a panel (large filled rectangle with a tag).
        let is_panel_text = text_up.iter().any(|t| t.starts_with('L') && t.len() <= 3 || t.starts_with('P') && t.len() <= 3 || t.starts_with("TX-"));
        if is_panel_text && !has_circle && !has_arc {
            continue; // Panels are detected separately.
        }

        // Classify device kind from shape and text.
        let kind = if text_up.iter().any(|t| t == "S") && has_circle {
            "switch"
        } else if text_up.iter().any(|t| t == "EM") && has_circle {
            "emergency_light"
        } else if text_up.iter().any(|t| t == "DS") {
            "disconnect"
        } else if tri && !has_circle {
            // Triangle = data outlet
            "data_outlet"
        } else if has_arc && !has_circle {
            "light_fixture" // wall sconce (arc)
        } else if has_circle && n_h >= 1 && n_v >= 1 {
            // Circle + cross lines = floor box
            "floor_box"
        } else if has_circle && (n_h >= 2 || n_v >= 2) {
            // Circle + two parallel lines = receptacle
            "receptacle"
        } else if count == 1 && has_circle {
            // Bare circle, could be a downlight
            "light_fixture"
        } else {
            // Rectangle shapes (troffers).
            "light_fixture"
        };

        // Look for exit sign shapes (rectangle with X crossing lines).
        let kind =
            if crossed || (text_up.is_empty() && !has_circle && !has_poly && !has_arc && count >= 3 && diag.len() >= 2) { "exit_sign" } else { kind };

        // Junction box: small filled rectangle.
        let kind = if text_up.iter().any(|t| t.starts_with("JB")) { "junction_box" } else { kind };

        // The symbol's own texts (its tag, subscripts, circuit): right beside it, nearest first;
        // a neighbour's are feet away.
        let mut close: Vec<&(String, [f64; 2])> = nearby.iter().copied().filter(|(_, p)| (p[0] - cx).hypot(p[1] - cy) < CLOSE_TEXT_IN).collect();
        close.sort_by(|a, b| (a.1[0] - cx).hypot(a.1[1] - cy).total_cmp(&(b.1[0] - cx).hypot(b.1[1] - cy)));
        // Get the type tag from the closest text.
        let type_tag = close
            .iter()
            .filter(|(t, _)| {
                let u = t.trim().to_uppercase();
                // Short tags: A, B, C, D, E, S, R, FB, ES-1, EM-1, DS-1, JB-1, etc.; not a
                // circuit label (`L1/10`), which says the circuit, not the type.
                u.len() <= 6
                    && !u.contains('@')
                    && !u.contains('\'')
                    && !u.contains('"')
                    && !u.starts_with("DO ")
                    && !u.starts_with("SCALE")
                    && !is_subscript_or_circuit(&u)
                    && !u.contains('/')
            })
            .min_by(|(_, a), (_, b)| {
                let da = (a[0] - cx).powi(2) + (a[1] - cy).powi(2);
                let db = (b[0] - cx).powi(2) + (b[1] - cy).powi(2);
                da.total_cmp(&db)
            })
            .map(|(t, _)| t.trim().to_string())
            .unwrap_or_default();

        // Parse mounting height from nearby text.
        let (mounting_in, height_source, mounting_note) = parse_mounting_height(&nearby, kind);

        devices.push(DetectedDevice {
            at_in: [cx, cy],
            kind: kind.to_string(),
            type_tag,
            mounting_in,
            height_source: height_source.to_string(),
            mounting_note,
            circuit_label: None,
            notes: close.iter().map(|(t, _)| t.trim().to_string()).filter(|t| !t.is_empty()).take(16).collect(),
        });
    }

    // Detect homerun arrows and associate circuit labels with nearby devices.
    // Homeruns are diagonal lines with arrow tips and a circuit label (e.g., "L1/1").
    let circuit_labels: Vec<(String, [f64; 2])> = texts
        .iter()
        .filter(|(t, _)| {
            let u = t.trim();
            // Circuit labels: "L1/1", "P1/3", etc.
            u.contains('/') && u.len() <= 8 && u.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        })
        .cloned()
        .collect();

    for cl in &circuit_labels {
        // Find the nearest device to this circuit label.
        if let Some(dev) = devices
            .iter_mut()
            .min_by(|a, b| {
                let da = (a.at_in[0] - cl.1[0]).powi(2) + (a.at_in[1] - cl.1[1]).powi(2);
                let db = (b.at_in[0] - cl.1[0]).powi(2) + (b.at_in[1] - cl.1[1]).powi(2);
                da.total_cmp(&db)
            })
            .filter(|d| d.circuit_label.is_none())
        {
            dev.circuit_label = Some(cl.0.clone());
        }
    }

    devices
}

/// Parse mounting height from nearby text annotations.
fn parse_mounting_height(texts: &[&(String, [f64; 2])], kind: &str) -> (f64, &'static str, Option<String>) {
    // Look for explicit mounting height notes: +18" AFF, +46", +60", CLG, FLOOR, +84", etc.
    for (text, _) in texts {
        let upper = text.to_uppercase();
        if upper.contains("CLG") || upper.contains("CEILING") {
            return (0.0, "note", Some("CLG".into())); // 0 = ceiling, resolved later
        }
        if upper.contains("FLOOR") {
            return (0.0, "note", Some("FLOOR".into()));
        }
        // Parse "+NN" or "+NN" AFF" patterns.
        if let Some(rest) = upper.strip_prefix('+') {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(inches) = digits.parse::<f64>()
                && inches > 0.0
                && inches <= 200.0
            {
                return (inches, "note", Some(text.clone()));
            }
        }
    }
    // Fall back to default rules.
    let h = default_mounting_in(kind);
    let note = if h == 0.0 && is_ceiling_mount(kind) {
        Some("CLG".into())
    } else if h == 0.0 {
        Some("FLOOR".into())
    } else {
        Some(format!("+{}\"", h as i64))
    };
    (h, "rule", note)
}

/// Detect panels on the DEVICE layer (filled rectangles with panel tags).
pub(crate) fn detect_panels(d: &Drawing) -> Vec<DetectedPanel> {
    let mut rects: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !is_device_layer(&e.common.layer) {
            continue;
        }
        if let EntityKind::LwPolyline(p) = &e.kind
            && (p.vertices.len() == 4 || (p.vertices.len() == 5 && p.closed))
        {
            let mut minx = f64::INFINITY;
            let mut miny = f64::INFINITY;
            let mut maxx = f64::NEG_INFINITY;
            let mut maxy = f64::NEG_INFINITY;
            for v in &p.vertices {
                minx = minx.min(v.p.x);
                miny = miny.min(v.p.y);
                maxx = maxx.max(v.p.x);
                maxy = maxy.max(v.p.y);
            }
            rects.push(([minx, miny], [maxx, maxy]));
        }
    }

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

    let mut panels = Vec::new();
    for (lo, hi) in &rects {
        let cx = (lo[0] + hi[0]) / 2.0;
        let cy = (lo[1] + hi[1]) / 2.0;
        // Look for panel tag text above the rectangle.
        let tag = texts
            .iter()
            .filter(|(t, p)| {
                let dx = (p[0] - cx).abs();
                let dy = p[1] - cy; // above
                let u = t.trim().to_uppercase();
                dx < 80.0
                    && dy > 0.0
                    && dy < 120.0
                    && (u.starts_with('L') || u.starts_with('P') || u.starts_with("TX-") || u.starts_with("MDP") || u.starts_with("DS-"))
                    && u.len() <= 6
            })
            .min_by(|(_, a), (_, b)| {
                let da = (a[0] - cx).powi(2) + (a[1] - cy).powi(2);
                let db = (b[0] - cx).powi(2) + (b[1] - cy).powi(2);
                da.total_cmp(&db)
            })
            .map(|(t, _)| t.trim().to_string());

        if let Some(tag) = tag {
            let kind = if tag.starts_with("TX") {
                "transformer"
            } else if tag.starts_with("MDP") {
                "switchboard"
            } else if tag.starts_with("DS-") {
                "disconnect"
            } else {
                "panel"
            };
            panels.push(DetectedPanel { at_in: [cx, cy], tag, kind: kind.to_string() });
        }
    }
    panels
}

// ---- Architectural placement rules ------------------------------------------------------
//
// Per the CORRECTION: E-sheets are diagrammatic — positions of symbols on E-sheets do NOT
// represent the real device locations. The E-sheet tells us WHAT (type, tag, circuit, room,
// wall/side). The position along the wall comes from architectural sheets (A-sheets):
// dimensioned plans, interior elevations, casework. A device whose position the A-sheets
// don't fix is a placement ZONE (the stretch of wall/room where it can go), flagged
// field-decision, not a fake exact point.

/// Which wall of a rectangular room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallSide {
    South,
    North,
    West,
    East,
}

fn is_corridor_room(room: &Room) -> bool {
    let n = room.name.to_uppercase();
    n.contains("CORRIDOR") || n.contains("HALL")
}

fn assign_wall_side(room: &Room, pt: [f64; 2]) -> WallSide {
    let ds = (pt[1] - room.min_ft[1]).abs();
    let dn = (room.max_ft[1] - pt[1]).abs();
    let dw = (pt[0] - room.min_ft[0]).abs();
    let de = (room.max_ft[0] - pt[0]).abs();
    let m = ds.min(dn).min(dw).min(de);
    if (ds - m).abs() < 1e-9 {
        WallSide::South
    } else if (dn - m).abs() < 1e-9 {
        WallSide::North
    } else if (dw - m).abs() < 1e-9 {
        WallSide::West
    } else {
        WallSide::East
    }
}

/// Light fixture grid positions from room geometry, matching the fixture generator's layout.
fn light_grid_for_room(room: &Room) -> Vec<[f64; 2]> {
    let rw = room.max_ft[0] - room.min_ft[0];
    let rh = room.max_ft[1] - room.min_ft[1];
    let area = rw * rh;
    let is_corr = is_corridor_room(room);
    let per_fix = if is_corr { 60.0 } else { 80.0 };
    let count = ((area / per_fix).round() as usize).clamp(1, 12);
    let cols = if rw > rh { ((rw / 8.0).ceil() as usize).clamp(1, 4) } else { 1 };
    let rows_n = ((count as f64 / cols as f64).ceil() as usize).max(1);
    let mut positions = Vec::new();
    for row in 0..rows_n {
        for col in 0..cols {
            if row * cols + col >= count {
                break;
            }
            let fx = room.min_ft[0] + (col as f64 + 0.5) * rw / cols as f64;
            let fy = room.min_ft[1] + (row as f64 + 0.5) * rh / rows_n as f64;
            positions.push([fx, fy]);
        }
    }
    positions
}

/// Expected receptacle positions around a room's perimeter (placement rules: one every 12 ft).
fn receptacle_positions_for_room(room: &Room) -> Vec<[f64; 2]> {
    let (rx0, ry0) = (room.min_ft[0], room.min_ft[1]);
    let (rx1, ry1) = (room.max_ft[0], room.max_ft[1]);
    let rw = rx1 - rx0;
    let rh = ry1 - ry0;
    let perimeter = 2.0 * (rw + rh);
    let n = if is_corridor_room(room) { 2 } else { ((perimeter / 12.0).ceil() as usize).clamp(2, 8) };
    let mut positions = Vec::new();
    for k in 0..n {
        let frac = k as f64 / n as f64;
        let perim_at = frac * perimeter;
        let (rx, ry) = if perim_at < rw {
            (rx0 + perim_at, ry0)
        } else if perim_at < rw + rh {
            (rx1, ry0 + (perim_at - rw))
        } else if perim_at < 2.0 * rw + rh {
            (rx1 - (perim_at - rw - rh), ry1)
        } else {
            (rx0, ry1 - (perim_at - 2.0 * rw - rh))
        };
        positions.push([rx, ry]);
    }
    positions
}

/// Expected exit sign positions in a corridor (at each end).
fn exit_sign_positions(room: &Room) -> Vec<[f64; 2]> {
    let cy = (room.min_ft[1] + room.max_ft[1]) / 2.0;
    vec![[room.min_ft[0] + 1.0, cy], [room.max_ft[0] - 1.0, cy]]
}

/// Expected emergency light positions in a corridor (every 75 ft).
fn emergency_light_positions(room: &Room) -> Vec<[f64; 2]> {
    let corr_len = room.max_ft[0] - room.min_ft[0];
    let cy = (room.min_ft[1] + room.max_ft[1]) / 2.0;
    let n = ((corr_len / 75.0).ceil() as usize).max(1);
    (0..n)
        .map(|k| {
            let x = room.min_ft[0] + (k as f64 + 0.5) * corr_len / n as f64;
            [x, cy]
        })
        .collect()
}

/// Find the nearest position from a list to a given point.
fn snap_to_nearest(expected: &[[f64; 2]], approx: [f64; 2]) -> [f64; 2] {
    expected
        .iter()
        .min_by(|a, b| {
            let da = (a[0] - approx[0]).powi(2) + (a[1] - approx[1]).powi(2);
            let db = (b[0] - approx[0]).powi(2) + (b[1] - approx[1]).powi(2);
            da.total_cmp(&db)
        })
        .copied()
        .unwrap_or(approx)
}

/// Architectural placement result.
struct ArchPlacement {
    at_ft: [f64; 2],
    arch_positioned: bool,
    zone: Option<DeviceZone>,
    /// Source of the position: sheet number + item that fixed it, or empty.
    source: String,
}

/// A door read from the A-sheets, for placement context.
pub struct PlacementDoor {
    pub room: String,
    pub at_ft: [f64; 2],
    pub latch_ft: [f64; 2],
    pub hinge_ft: [f64; 2],
    pub hand: String,
    pub sheet: String,
}

/// A casework/backsplash item read from the A-sheets, for placement context.
pub struct PlacementCasework {
    pub room: String,
    pub kind: String,
    pub tag: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub height_in: f64,
    pub sheet: String,
}

/// Context for device placement: rooms, doors, casework read from A-sheets (never from truth).
pub struct PlacementContext {
    /// Rooms by level, from the ceiling or arch reader.
    pub rooms: BTreeMap<String, Vec<Room>>,
    /// Level elevations (feet) from the reader.
    pub level_elev: BTreeMap<String, f64>,
    /// Ceiling heights per (level, room_number), from the ceiling reader.
    pub ceiling_ft: BTreeMap<(String, String), f64>,
    /// Doors per room from the A-sheet arch reader.
    pub doors: Vec<PlacementDoor>,
    /// Casework items per room from the A-sheet casework reader.
    pub casework: Vec<PlacementCasework>,
}

impl PlacementContext {
    /// Build from truth (temporary bridge until all callers pass reader output).
    pub fn from_truth(truth: &Truth) -> Self {
        let mut rooms: BTreeMap<String, Vec<Room>> = BTreeMap::new();
        for rm in &truth.building.rooms {
            rooms.entry(rm.level.clone()).or_default().push(rm.clone());
        }
        let mut level_elev = BTreeMap::new();
        for lev in &truth.building.levels {
            level_elev.insert(lev.name.clone(), lev.elevation_ft);
        }
        let mut ceiling_ft = BTreeMap::new();
        for rm in &truth.building.rooms {
            if let Some(c) = &rm.ceiling {
                ceiling_ft.insert((rm.level.clone(), rm.number.clone()), c.height_ft);
            }
        }
        // Doors from truth.arch.doors (when the truth carries architecture).
        let doors = truth
            .arch
            .as_ref()
            .map(|a| {
                a.doors
                    .iter()
                    .map(|d| {
                        let room = d.swings_into.as_ref().or(d.from_room.as_ref()).cloned().unwrap_or_default();
                        PlacementDoor {
                            room,
                            at_ft: d.at_ft,
                            latch_ft: d.latch_ft,
                            hinge_ft: d.hinge_ft,
                            hand: d.hand.clone(),
                            sheet: String::new(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Casework from truth.casework.
        let casework = truth
            .casework
            .iter()
            .map(|c| PlacementCasework {
                room: c.room.clone(),
                kind: c.kind.clone(),
                tag: c.tag.clone(),
                min_ft: c.min_ft,
                max_ft: c.max_ft,
                height_in: c.height_in,
                sheet: String::new(),
            })
            .collect();
        PlacementContext { rooms, level_elev, ceiling_ft, doors, casework }
    }

    /// Build from reader output (the production path — no truth dependency).
    pub fn from_readers(
        ceiling_rooms: &[crate::ceiling_reader::FoundRoom],
        ceilings: &[crate::ceiling_reader::FoundCeiling],
        arch_doors: &[crate::arch_reader::FoundDoor],
        read_casework: &[crate::score::FoundCaseworkItem],
        level_elevations: &BTreeMap<String, f64>,
    ) -> Self {
        let mut rooms: BTreeMap<String, Vec<Room>> = BTreeMap::new();
        for cr in ceiling_rooms {
            rooms.entry(cr.level.clone()).or_default().push(Room {
                id: cr.number.clone(),
                level: cr.level.clone(),
                number: cr.number.clone(),
                name: cr.name.clone(),
                min_ft: cr.min_ft,
                max_ft: cr.max_ft,
                ceiling: None,
                plenum_ft: None,
            });
        }
        let mut ceiling_ft = BTreeMap::new();
        for c in ceilings {
            if let Some(h) = c.height_ft {
                ceiling_ft.insert((c.level.clone(), c.room_number.clone()), h);
            }
        }
        let doors = arch_doors
            .iter()
            .filter_map(|d| {
                Some(PlacementDoor {
                    room: d.swings_into.as_ref().or(d.from_room.as_ref())?.clone(),
                    at_ft: d.at_ft,
                    latch_ft: d.latch_ft,
                    hinge_ft: d.hinge_ft,
                    hand: d.hand.clone(),
                    sheet: String::new(),
                })
            })
            .collect();
        // Casework: assign room by checking which room the centre falls in.
        let casework = read_casework
            .iter()
            .map(|c| {
                let cx = (c.min_ft[0] + c.max_ft[0]) / 2.0;
                let cy = (c.min_ft[1] + c.max_ft[1]) / 2.0;
                let room_num = rooms.get(&c.level).and_then(|rms| {
                    rms.iter()
                        .find(|r| cx >= r.min_ft[0] - 0.5 && cx <= r.max_ft[0] + 0.5 && cy >= r.min_ft[1] - 0.5 && cy <= r.max_ft[1] + 0.5)
                        .map(|r| r.number.clone())
                }).unwrap_or_default();
                PlacementCasework {
                    room: room_num,
                    kind: c.kind.clone(),
                    tag: c.tag.clone(),
                    min_ft: c.min_ft,
                    max_ft: c.max_ft,
                    height_in: c.height_in,
                    sheet: String::new(),
                }
            })
            .collect();
        PlacementContext { rooms, level_elev: level_elevations.clone(), ceiling_ft, doors, casework }
    }

    fn room_doors(&self, room: &str) -> Vec<&PlacementDoor> {
        self.doors.iter().filter(|d| d.room == room).collect()
    }

    fn room_casework(&self, room: &str, kind: &str) -> Vec<&PlacementCasework> {
        self.casework.iter().filter(|c| c.room == room && c.kind == kind).collect()
    }
}

/// Position a device using room geometry and A-sheet evidence.
///
/// The `approx_ft` (from the E-sheet via `building_from_model`) is used ONLY to determine
/// which expected layout position this device corresponds to (e.g. which grid cell, which
/// perimeter slot, which corridor end). The final position comes from A-sheet evidence when
/// available, otherwise from a placement zone (flagged field-decision).
fn position_from_rules(room: &Room, kind: &str, approx_ft: [f64; 2], ctx: &PlacementContext) -> ArchPlacement {
    let (rx0, ry0) = (room.min_ft[0], room.min_ft[1]);
    let (rx1, ry1) = (room.max_ft[0], room.max_ft[1]);
    let rw = rx1 - rx0;
    let rh = ry1 - ry0;
    let room_num = &room.number;

    // Helper: build a zone for a given wall side of this room.
    let make_zone = |wall: WallSide, range: [f64; 2], height_band: [f64; 2], field: bool| -> DeviceZone {
        let axis = match wall {
            WallSide::South | WallSide::North => ZoneAxis::X,
            WallSide::West | WallSide::East => ZoneAxis::Y,
        };
        DeviceZone { room: room_num.clone(), wall_side: wall, axis, range_ft: range, height_in: height_band, field_decision: field }
    };

    // Default height bands by kind (inches AFF).
    let default_height = |k: &str| -> [f64; 2] {
        match k {
            "receptacle" | "data_outlet" => [14.0, 22.0],
            "switch" => [42.0, 50.0],
            "light_fixture" | "exit_sign" | "emergency_light" => [84.0, 120.0],
            "disconnect" => [48.0, 72.0],
            "junction_box" => [72.0, 120.0],
            "floor_box" => [0.0, 0.0],
            _ => [0.0, 120.0],
        }
    };

    match kind {
        // ---- Switches: position from A-sheet door latch side when available ----
        "switch" => {
            let doors = ctx.room_doors(room_num);
            if let Some(door) = doors.iter().min_by(|a, b| {
                let da = (a.latch_ft[0] - approx_ft[0]).powi(2) + (a.latch_ft[1] - approx_ft[1]).powi(2);
                let db = (b.latch_ft[0] - approx_ft[0]).powi(2) + (b.latch_ft[1] - approx_ft[1]).powi(2);
                da.total_cmp(&db)
            }) {
                // Switch on latch side of the actual door.
                let latch = door.latch_ft;
                let wall = assign_wall_side(room, latch);
                let (range, at) = match wall {
                    WallSide::South | WallSide::North => {
                        let r = [(latch[0] - 1.0).max(rx0), (latch[0] + 1.0).min(rx1)];
                        ([r[0], r[1]], [latch[0], if wall == WallSide::South { ry0 + 0.3 } else { ry1 - 0.3 }])
                    }
                    WallSide::West | WallSide::East => {
                        let r = [(latch[1] - 1.0).max(ry0), (latch[1] + 1.0).min(ry1)];
                        ([r[0], r[1]], [if wall == WallSide::West { rx0 + 0.3 } else { rx1 - 0.3 }, latch[1]])
                    }
                };
                let source = format!("{} door-latch", door.sheet);
                let zone = make_zone(wall, range, default_height("switch"), true);
                ArchPlacement { at_ft: at, arch_positioned: false, zone: Some(zone), source }
            } else {
                // No door found: zone at nearest corner.
                let corners = [[rx0 + 0.5, ry0 + 0.5], [rx0 + 0.5, ry1 - 0.5], [rx1 - 0.5, ry0 + 0.5], [rx1 - 0.5, ry1 - 0.5]];
                let nearest_corner = snap_to_nearest(&corners, approx_ft);
                let wall = assign_wall_side(room, nearest_corner);
                let range = match wall {
                    WallSide::South | WallSide::North => [(nearest_corner[0] - 2.0).max(rx0), (nearest_corner[0] + 2.0).min(rx1)],
                    WallSide::West | WallSide::East => [(nearest_corner[1] - 2.0).max(ry0), (nearest_corner[1] + 2.0).min(ry1)],
                };
                let zone = make_zone(wall, range, default_height("switch"), true);
                ArchPlacement { at_ft: nearest_corner, arch_positioned: false, zone: Some(zone), source: String::new() }
            }
        }

        // ---- Receptacles: check for backsplash/counter evidence ----
        "receptacle" => {
            // Check if there's a backsplash/counter fixing this receptacle.
            let backsplashes = ctx.room_casework(room_num, "backsplash");
            let counters = ctx.room_casework(room_num, "counter");
            let vanities = ctx.room_casework(room_num, "vanity");
            let counter_items: Vec<&PlacementCasework> = counters.into_iter().chain(vanities).collect();

            // Find a backsplash near the approx position.
            let bs_hit = backsplashes.iter().find(|bs| {
                let cx = (bs.min_ft[0] + bs.max_ft[0]) / 2.0;
                let cy = (bs.min_ft[1] + bs.max_ft[1]) / 2.0;
                (cx - approx_ft[0]).abs() < 3.0 && (cy - approx_ft[1]).abs() < 3.0
            });

            if let Some(bs) = bs_hit {
                // Find counter under this backsplash.
                let bsc = [(bs.min_ft[0] + bs.max_ft[0]) / 2.0, (bs.min_ft[1] + bs.max_ft[1]) / 2.0];
                let counter = counter_items.iter().find(|c| {
                    bsc[0] >= c.min_ft[0] - 0.1 && bsc[0] <= c.max_ft[0] + 0.1 && bsc[1] >= c.min_ft[1] - 0.1 && bsc[1] <= c.max_ft[1] + 0.1
                });
                let wall = assign_wall_side(room, bsc);
                let (range, axis) = match wall {
                    WallSide::South | WallSide::North => ([bs.min_ft[0], bs.max_ft[0]], ZoneAxis::X),
                    WallSide::West | WallSide::East => ([bs.min_ft[1], bs.max_ft[1]], ZoneAxis::Y),
                };
                let counter_top = counter.map(|c| c.height_in).unwrap_or(36.0);
                let low = counter_top + 2.0;
                let high = (counter_top + 20.0).max(low);
                let source = format!("{} backsplash {}", bs.sheet, bs.tag);
                let zone = DeviceZone {
                    room: room_num.clone(), wall_side: wall, axis, range_ft: range,
                    height_in: [low, high], field_decision: true,
                };
                let at = [(range[0] + range[1]) / 2.0, if axis == ZoneAxis::X { if wall == WallSide::South { ry0 + 0.3 } else { ry1 - 0.3 } } else { approx_ft[1] }];
                let at = if axis == ZoneAxis::Y {
                    [if wall == WallSide::West { rx0 + 0.3 } else { rx1 - 0.3 }, (range[0] + range[1]) / 2.0]
                } else {
                    at
                };
                ArchPlacement { at_ft: at, arch_positioned: false, zone: Some(zone), source }
            } else {
                // No casework evidence: zone covers the wall.
                let expected = receptacle_positions_for_room(room);
                let snapped = snap_to_nearest(&expected, approx_ft);
                let wall = assign_wall_side(room, snapped);
                let range = match wall {
                    WallSide::South | WallSide::North => [rx0, rx1],
                    WallSide::West | WallSide::East => [ry0, ry1],
                };
                let zone = make_zone(wall, range, default_height("receptacle"), true);
                ArchPlacement { at_ft: snapped, arch_positioned: false, zone: Some(zone), source: String::new() }
            }
        }

        // ---- Light fixtures: always zones (rule-based grid, no A-sheet fixes them) ----
        "light_fixture" => {
            let grid = light_grid_for_room(room);
            let snapped = snap_to_nearest(&grid, approx_ft);
            let (wall, range) = if rw >= rh {
                (WallSide::South, [rx0, rx1])
            } else {
                (WallSide::West, [ry0, ry1])
            };
            let zone = make_zone(wall, range, default_height("light_fixture"), true);
            ArchPlacement { at_ft: snapped, arch_positioned: false, zone: Some(zone), source: String::new() }
        }

        // ---- Floor box: zone at room centre (rule-based, not arch-fixed) ----
        "floor_box" => {
            let centre = [(rx0 + rx1) / 2.0, (ry0 + ry1) / 2.0];
            let wall = WallSide::South;
            let zone = make_zone(wall, [rx0, rx1], default_height("floor_box"), true);
            ArchPlacement { at_ft: centre, arch_positioned: false, zone: Some(zone), source: String::new() }
        }

        // ---- Disconnect: zone in mechanical room (rule-based) ----
        "disconnect" => {
            let at = [rx0 + 2.0, ry0 + 2.0];
            let wall = assign_wall_side(room, at);
            let range = match wall {
                WallSide::South | WallSide::North => [rx0, (rx0 + 5.0).min(rx1)],
                WallSide::West | WallSide::East => [ry0, (ry0 + 5.0).min(ry1)],
            };
            let zone = make_zone(wall, range, default_height("disconnect"), true);
            ArchPlacement { at_ft: at, arch_positioned: false, zone: Some(zone), source: String::new() }
        }

        // ---- Junction box: zone near panels (rule-based) ----
        "junction_box" => {
            let at = [rx0 + 3.5, ry0 + 2.0];
            let wall = assign_wall_side(room, at);
            let range = match wall {
                WallSide::South | WallSide::North => [rx0, (rx0 + 6.0).min(rx1)],
                WallSide::West | WallSide::East => [ry0, (ry0 + 6.0).min(ry1)],
            };
            let zone = make_zone(wall, range, default_height("junction_box"), true);
            ArchPlacement { at_ft: at, arch_positioned: false, zone: Some(zone), source: String::new() }
        }

        // ---- Data outlet: zone along south wall ----
        "data_outlet" => {
            let n_data = 2usize;
            let expected: Vec<[f64; 2]> = (0..n_data)
                .map(|k| {
                    let dx = rx0 + (k as f64 + 0.5) * rw / n_data as f64;
                    [dx, ry0 + 0.3]
                })
                .collect();
            let snapped = snap_to_nearest(&expected, approx_ft);
            let zone = make_zone(WallSide::South, [rx0, rx1], default_height("data_outlet"), true);
            ArchPlacement { at_ft: snapped, arch_positioned: false, zone: Some(zone), source: String::new() }
        }

        // ---- Exit sign: zone at corridor ends ----
        "exit_sign" => {
            let expected = exit_sign_positions(room);
            let snapped = snap_to_nearest(&expected, approx_ft);
            let wall = if rw >= rh { WallSide::South } else { WallSide::West };
            let range = if rw >= rh { [rx0, rx1] } else { [ry0, ry1] };
            let zone = make_zone(wall, range, default_height("exit_sign"), true);
            ArchPlacement { at_ft: snapped, arch_positioned: false, zone: Some(zone), source: String::new() }
        }

        // ---- Emergency light: zone along corridor ----
        "emergency_light" => {
            let expected = emergency_light_positions(room);
            let snapped = snap_to_nearest(&expected, approx_ft);
            let wall = if rw >= rh { WallSide::South } else { WallSide::West };
            let range = if rw >= rh { [rx0, rx1] } else { [ry0, ry1] };
            let zone = make_zone(wall, range, default_height("emergency_light"), true);
            ArchPlacement { at_ft: snapped, arch_positioned: false, zone: Some(zone), source: String::new() }
        }

        _ => {
            // Unknown device: zone covers the wall it's nearest to.
            let wall = assign_wall_side(room, approx_ft);
            let range = match wall {
                WallSide::South | WallSide::North => [rx0, rx1],
                WallSide::West | WallSide::East => [ry0, ry1],
            };
            let zone = make_zone(wall, range, default_height(kind), true);
            ArchPlacement { at_ft: approx_ft, arch_positioned: false, zone: Some(zone), source: String::new() }
        }
    }
}

// ---- Counter receptacle zones from read casework (Q301 → Q288) ---------------------------

/// A counter receptacle zone from casework read off the architectural sheets: the stretch of
/// the wall a read backsplash covers, at the heights a receptacle over its counter can take.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CounterZone {
    pub level: String,
    /// The wall behind the backsplash.
    pub wall: String,
    /// Along the wall from its start, inches.
    pub along_in: [f64; 2],
    /// The same stretch in building feet, along the wall's axis (x for a wall running east-west,
    /// else y), as [`DeviceZone::range_ft`] holds it.
    pub zone_ft: [f64; 2],
    /// Box-centre heights, inches AFF: from 2" over the backsplash (or over the counter when the
    /// backsplash runs full height, 18" or more) to 20" over the counter top.
    pub height_in: [f64; 2],
    /// The read backsplash's and counter's tags.
    pub backsplash: String,
    pub counter: String,
}

/// Counter receptacle zones over every read backsplash that sits on a read counter or vanity
/// (its height is the counter top) against a wall. Only the reader's items are used.
pub fn counter_receptacle_zones(walls: &[crate::truth::Wall], read: &[crate::score::FoundCaseworkItem]) -> Vec<CounterZone> {
    let mut out = Vec::new();
    for bs in read.iter().filter(|c| c.kind == "backsplash") {
        let c = [(bs.min_ft[0] + bs.max_ft[0]) / 2.0, (bs.min_ft[1] + bs.max_ft[1]) / 2.0];
        let half = [(bs.max_ft[0] - bs.min_ft[0]) / 2.0, (bs.max_ft[1] - bs.min_ft[1]) / 2.0];
        let counter = read
            .iter()
            .filter(|k| k.level == bs.level && (k.kind == "counter" || k.kind == "vanity") && k.height_in > 0.0)
            .filter(|k| c[0] >= k.min_ft[0] - 0.1 && c[0] <= k.max_ft[0] + 0.1 && c[1] >= k.min_ft[1] - 0.1 && c[1] <= k.max_ft[1] + 0.1)
            .max_by(|a, b| {
                ((a.max_ft[0] - a.min_ft[0]) * (a.max_ft[1] - a.min_ft[1])).total_cmp(&((b.max_ft[0] - b.min_ft[0]) * (b.max_ft[1] - b.min_ft[1])))
            });
        let Some(counter) = counter else { continue };
        let Some(wall) = crate::clash::backing_wall(walls, &bs.level, c, half) else { continue };
        let (dx, dy) = (wall.to_ft[0] - wall.from_ft[0], wall.to_ft[1] - wall.from_ft[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            continue;
        }
        let along = |p: [f64; 2]| ((p[0] - wall.from_ft[0]) * dx + (p[1] - wall.from_ft[1]) * dy) / len * 12.0;
        let (a, b) = (along(bs.min_ft), along(bs.max_ft));
        let x_axis = dx.abs() >= dy.abs();
        let zone_ft = if x_axis { [bs.min_ft[0], bs.max_ft[0]] } else { [bs.min_ft[1], bs.max_ft[1]] };
        let top = counter.height_in;
        let low = if bs.height_in >= 18.0 { top + 2.0 } else { top + bs.height_in.max(0.0) + 2.0 };
        out.push(CounterZone {
            level: bs.level.clone(),
            wall: wall.id.clone(),
            along_in: [a.min(b), a.max(b)],
            zone_ft,
            height_in: [low, (top + 20.0).max(low)],
            backsplash: bs.tag.clone(),
            counter: counter.tag.clone(),
        });
    }
    out
}

/// Gives every found receptacle drawn over a counter zone (within 2 ft of its wall, along its
/// stretch give or take a foot) that zone: `zone_ft` becomes the backsplash's stretch, the
/// mounting height is kept inside the zone's band and `height_source` becomes `casework`.
/// Returns how many devices changed.
pub fn apply_counter_zones(devices: &mut [FoundDevice], walls: &[crate::truth::Wall], zones: &[CounterZone]) -> usize {
    let mut n = 0;
    for d in devices.iter_mut().filter(|d| d.kind == "receptacle") {
        let at = [d.at_ft[0], d.at_ft[1]];
        let hit = zones.iter().find(|z| {
            let Some(w) = walls.iter().find(|w| w.id == z.wall) else { return false };
            let (dx, dy) = (w.to_ft[0] - w.from_ft[0], w.to_ft[1] - w.from_ft[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if z.level != d.level || len < 1e-6 {
                return false;
            }
            let off = ((at[0] - w.from_ft[0]) * -dy + (at[1] - w.from_ft[1]) * dx).abs() / len;
            let v = if dx.abs() >= dy.abs() { at[0] } else { at[1] };
            off <= 2.0 && v >= z.zone_ft[0] - 1.0 && v <= z.zone_ft[1] + 1.0
        });
        if let Some(z) = hit {
            let x_axis = {
                let Some(w) = walls.iter().find(|w| w.id == z.wall) else { continue };
                (w.to_ft[0] - w.from_ft[0]).abs() >= (w.to_ft[1] - w.from_ft[1]).abs()
            };
            d.zone = Some(DeviceZone {
                room: d.room.clone(),
                wall_side: if x_axis { WallSide::South } else { WallSide::West },
                axis: if x_axis { ZoneAxis::X } else { ZoneAxis::Y },
                range_ft: z.zone_ft,
                height_in: z.height_in,
                field_decision: true,
            });
            d.source = format!("casework {} / {}", z.backsplash, z.counter);
            d.arch_positioned = false;
            d.mounting_in = if d.mounting_in.is_finite() { d.mounting_in.clamp(z.height_in[0], z.height_in[1]) } else { z.height_in[0] };
            d.height_source = "casework".into();
            n += 1;
        }
    }
    n
}

// ---- Found device (building coordinates) for scoring ------------------------------------

/// A device a reader found, in building coordinates, ready for scoring against the truth.
#[derive(Clone, Debug, Default)]
pub struct FoundDevice {
    pub level: String,
    pub kind: String,
    pub type_tag: String,
    pub room: String,
    /// Building feet.
    pub at_ft: [f64; 3],
    /// Mounting height inches AFF.
    pub mounting_in: f64,
    /// `"note"`, `"legend"`, or `"rule"`.
    pub height_source: String,
    /// Whether this device's plan position was fixed by architectural sheets (true) or is a
    /// zone / field decision (false).
    pub arch_positioned: bool,
    /// For zone-placed devices: the zone naming wall, range, height band, and field-decision flag.
    pub zone: Option<DeviceZone>,
    /// Source of the architectural position: sheet number + item/dimension that fixed it.
    /// Empty when the device is a zone (not fixed by a document).
    pub source: String,
    /// Its type's columns in the light fixture schedule (description, mounting, lamp and
    /// wattage, as text), when the schedule has its type ([`crate::gear_reader::link_fixtures`]).
    pub properties: BTreeMap<String, String>,
    /// Where the symbol is drawn on the plan, building feet.
    pub drawn_ft: [f64; 2],
    /// The texts beside the symbol (subscripts, its circuit), as printed.
    pub notes: Vec<String>,
}

/// A found panel, building coordinates.
#[derive(Clone, Debug, Default)]
pub struct FoundPanel {
    pub level: String,
    pub tag: String,
    pub kind: String,
    pub at_ft: [f64; 2],
    /// The plan sheet its tag was read from (`E-101`).
    pub sheet: String,
}

// ---- Read E-sheets and produce found devices -------------------------------------------

/// Default ceiling height for a room if the truth doesn't say (for ceiling-mounted devices).
const DEFAULT_CEILING_FT: f64 = 9.0;

/// Read electrical devices from the fixture set's DXF files using the sheet map for alignment.
/// Rooms, ceilings and level elevations come from the `PlacementContext` (reader output), not
/// from the truth. The `truth` parameter is kept for backward-compatible callers; new callers
/// should use `read_devices_ctx` with a reader-built context.
pub fn read_devices(dxf_data: &[(String, Vec<u8>)], sheet_map: &SheetMap, truth: &Truth) -> (Vec<FoundDevice>, Vec<FoundPanel>) {
    let ctx = PlacementContext::from_truth(truth);
    read_devices_ctx(dxf_data, sheet_map, &ctx)
}

/// Read electrical devices using a `PlacementContext` built from reader output (no truth).
pub fn read_devices_ctx(dxf_data: &[(String, Vec<u8>)], sheet_map: &SheetMap, ctx: &PlacementContext) -> (Vec<FoundDevice>, Vec<FoundPanel>) {
    let mut found_devices = Vec::new();
    let mut found_panels = Vec::new();

    // Build a room lookup: level → list of rooms (from the placement context).
    let room_map: BTreeMap<String, Vec<&Room>> = {
        let mut m: BTreeMap<String, Vec<&Room>> = BTreeMap::new();
        for (level, rooms) in &ctx.rooms {
            for rm in rooms {
                m.entry(level.clone()).or_default().push(rm);
            }
        }
        m
    };

    for (name, bytes) in dxf_data {
        // Only read E-sheets (lighting and power plans).
        let sheet_num = name.trim_end_matches(".dxf");
        if !sheet_num.starts_with("E-") || sheet_num.contains("60") {
            continue; // Skip schedule sheets.
        }

        let recognised = sheet_map.sheets.iter().find(|s| s.number == sheet_num);
        let Some(rs) = recognised else { continue };
        if rs.discipline != "E" {
            continue;
        }
        let Some(alignment) = &rs.alignment else { continue };
        let level = rs.level.clone().unwrap_or_default();
        if level.is_empty() {
            continue;
        }

        // Read the DXF.
        let d = match cadaddy_io::read_dxf(bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        // Detect devices and panels.
        let raw_devices = detect_device_symbols(&d);
        let raw_panels = detect_panels(&d);

        // E-sheet symbols give WHAT (type, tag, circuit, room, wall/side).
        // Position comes from A-sheet evidence and placement rules, NOT from where
        // the symbol sits on the diagrammatic E-sheet.
        for rd in &raw_devices {
            // Approximate building position from E-sheet — used ONLY for room and
            // wall/side assignment, never as the device's final position.
            let approx_b = building_from_model(alignment, rd.at_in);
            let room = find_room(&room_map, &level, approx_b);
            let room_name = room.map(|r| r.number.clone()).unwrap_or_default();

            // Resolve ceiling height from context (reader output).
            let ceiling_ft = room
                .and_then(|r| ctx.ceiling_ft.get(&(level.clone(), r.number.clone())).copied())
                .or_else(|| room.and_then(|r| r.ceiling.as_ref()).map(|c| c.height_ft))
                .unwrap_or(DEFAULT_CEILING_FT);
            let level_elev = ctx.level_elev.get(&level).copied().unwrap_or(0.0);

            let (z, mounting_in) = if rd.mounting_in == 0.0 && is_ceiling_mount(&rd.kind) {
                (level_elev + ceiling_ft, ceiling_ft * 12.0)
            } else if rd.mounting_in == 0.0 {
                (level_elev, 0.0)
            } else {
                (level_elev + rd.mounting_in / 12.0, rd.mounting_in)
            };

            // Position from A-sheet evidence and placement rules.
            let placement = if let Some(rm) = room {
                position_from_rules(rm, &rd.kind, approx_b, ctx)
            } else {
                // No room found — fallback to approximate with a zone.
                let zone = DeviceZone {
                    room: String::new(),
                    wall_side: WallSide::South,
                    axis: ZoneAxis::X,
                    range_ft: [approx_b[0] - 3.0, approx_b[0] + 3.0],
                    height_in: [0.0, 120.0],
                    field_decision: true,
                };
                ArchPlacement { at_ft: approx_b, arch_positioned: false, zone: Some(zone), source: String::new() }
            };

            found_devices.push(FoundDevice {
                level: level.clone(),
                kind: rd.kind.clone(),
                type_tag: rd.type_tag.clone(),
                room: room_name,
                at_ft: [placement.at_ft[0], placement.at_ft[1], z],
                mounting_in,
                height_source: rd.height_source.clone(),
                arch_positioned: placement.arch_positioned,
                zone: placement.zone,
                source: placement.source,
                properties: BTreeMap::new(),
                drawn_ft: approx_b,
                notes: rd.notes.clone(),
            });
        }

        for rp in &raw_panels {
            let b = building_from_model(alignment, rp.at_in);
            found_panels.push(FoundPanel {
                level: level.clone(),
                tag: rp.tag.clone(),
                kind: rp.kind.clone(),
                at_ft: b,
                sheet: sheet_num.to_string(),
            });
        }
    }

    (found_devices, found_panels)
}

/// Convert model inches to building feet using a sheet alignment.
fn building_from_model(alignment: &Alignment, m: [f64; 2]) -> [f64; 2] {
    let p = [m[0] - alignment.offset_in[0], m[1] - alignment.offset_in[1]];
    let r = layout::rotate(p, -alignment.rotation_deg);
    [r[0] / 12.0, r[1] / 12.0]
}

/// Find which room a point (building feet) is in.
fn find_room<'a>(rooms: &'a BTreeMap<String, Vec<&'a Room>>, level: &str, pt: [f64; 2]) -> Option<&'a Room> {
    rooms
        .get(level)?
        .iter()
        .find(|r| pt[0] >= r.min_ft[0] - 0.5 && pt[0] <= r.max_ft[0] + 0.5 && pt[1] >= r.min_ft[1] - 0.5 && pt[1] <= r.max_ft[1] + 0.5)
        .copied()
}

// ---- 3D generation ---------------------------------------------------------------------

/// Generate 3D geometry for found electrical devices: simple shapes on per-type layers.
///
/// Returns entities to add to a Drawing (layer name, position, size).
pub struct Device3D {
    pub layer: String,
    pub kind: String,
    pub at_ft: [f64; 3],
    /// Box dimensions in inches [width, depth, height].
    pub box_in: [f64; 3],
    /// The IFC kind tag for export.
    pub ifc_kind: String,
}

/// Build 3D representations of electrical devices.
pub fn build_3d(devices: &[FoundDevice]) -> Vec<Device3D> {
    devices
        .iter()
        .map(|d| {
            let (box_in, ifc_kind) = match d.kind.as_str() {
                "light_fixture" => ([24.0, 24.0, 4.0], "light_fixture"),
                "receptacle" => ([4.0, 2.0, 4.0], "outlet"),
                "data_outlet" => ([4.0, 2.0, 4.0], "outlet"),
                "switch" => ([3.0, 2.0, 5.0], "switch"),
                "exit_sign" => ([12.0, 2.0, 8.0], "light_fixture"),
                "emergency_light" => ([12.0, 4.0, 6.0], "light_fixture"),
                "floor_box" => ([6.0, 6.0, 4.0], "outlet"),
                "junction_box" => ([6.0, 6.0, 6.0], "junction_box"),
                "disconnect" => ([12.0, 6.0, 18.0], "switch"),
                _ => ([4.0, 4.0, 4.0], "outlet"),
            };
            Device3D {
                layer: format!("3D-ELEC-{}", d.kind.to_uppercase().replace('_', "-")),
                kind: d.kind.clone(),
                at_ft: d.at_ft,
                box_in,
                ifc_kind: ifc_kind.to_string(),
            }
        })
        .collect()
}

// ---- Scoring ---------------------------------------------------------------------------

/// Score for the electrical device reader.
#[derive(Clone, Debug, Default)]
pub struct DeviceScore {
    /// True positives (found device matches a truth device).
    pub true_positives: usize,
    /// False positives (found device has no matching truth device).
    pub false_positives: usize,
    /// False negatives (truth device not found).
    pub false_negatives: usize,
    /// Precision: TP / (TP + FP).
    pub precision: f64,
    /// Recall: TP / (TP + FN).
    pub recall: f64,
    /// Mean absolute error of mounting height for matched devices, inches.
    pub height_mae_in: f64,
    /// Of arch-positioned truth devices: mean position error, feet.
    pub position_mae_ft: f64,
    /// Of zone-placed truth devices: fraction where the zone contains the truth position.
    pub zone_containment: f64,
    /// Kind accuracy: fraction of matched devices with the correct kind.
    pub kind_accuracy: f64,
}

/// Score found devices against the truth.
///
/// Matching: a found device matches a truth device if they are on the same level, the same
/// kind, and within `tol_ft` of each other's plan position (x, y).
///
/// For `arch_positioned` truth devices, we score plan position error.
/// For zone devices (!arch_positioned), we score whether the truth position falls inside the
/// found device's zone.
pub fn score_devices(truth: &Truth, found: &[FoundDevice], tol_ft: f64) -> DeviceScore {
    let truth_devs = &truth.elec_devices;
    let mut matched_truth: Vec<bool> = vec![false; truth_devs.len()];
    let mut matched_found: Vec<bool> = vec![false; found.len()];

    let mut height_err_sum = 0.0;
    let mut height_count = 0usize;
    let mut pos_err_sum = 0.0;
    let mut pos_count = 0usize;
    let mut zone_ok = 0usize;
    let mut zone_total = 0usize;
    let mut kind_ok = 0usize;

    // Match each truth device to the closest found device of the same kind.
    for (ti, td) in truth_devs.iter().enumerate() {
        let mut best_dist = f64::INFINITY;
        let mut best_fi: Option<usize> = None;

        for (fi, fd) in found.iter().enumerate() {
            if matched_found[fi] || fd.level != td.level {
                continue;
            }
            let dx = fd.at_ft[0] - td.at_ft[0];
            let dy = fd.at_ft[1] - td.at_ft[1];
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < tol_ft && dist < best_dist {
                best_dist = dist;
                best_fi = Some(fi);
            }
        }

        if let Some(fi) = best_fi {
            matched_truth[ti] = true;
            matched_found[fi] = true;
            let fd = &found[fi];

            // Kind accuracy.
            if fd.kind == td.kind {
                kind_ok += 1;
            }

            // Height error.
            height_err_sum += (fd.mounting_in - td.mounting_in).abs();
            height_count += 1;

            // Position error (for architecturally positioned devices).
            if td.arch_positioned {
                let dx = fd.at_ft[0] - td.at_ft[0];
                let dy = fd.at_ft[1] - td.at_ft[1];
                pos_err_sum += (dx * dx + dy * dy).sqrt();
                pos_count += 1;
            } else {
                // Zone containment: check if truth position falls inside the found zone
                // along the zone's own wall axis (axis-correct check).
                zone_total += 1;
                if let Some(zone) = &fd.zone {
                    let v = match zone.axis {
                        ZoneAxis::X => td.at_ft[0],
                        ZoneAxis::Y => td.at_ft[1],
                    };
                    if v >= zone.range_ft[0] && v <= zone.range_ft[1] {
                        zone_ok += 1;
                    }
                }
            }
        }
    }

    let tp = matched_truth.iter().filter(|&&m| m).count();
    let fp = matched_found.iter().filter(|&&m| !m).count();
    let fneg = matched_truth.iter().filter(|&&m| !m).count();

    let precision = if tp + fp > 0 { tp as f64 / (tp + fp) as f64 } else { 0.0 };
    let recall = if tp + fneg > 0 { tp as f64 / (tp + fneg) as f64 } else { 0.0 };
    let height_mae = if height_count > 0 { height_err_sum / height_count as f64 } else { 0.0 };
    let position_mae = if pos_count > 0 { pos_err_sum / pos_count as f64 } else { 0.0 };
    let zone_cont = if zone_total > 0 { zone_ok as f64 / zone_total as f64 } else { 0.0 };
    let kind_acc = if tp > 0 { kind_ok as f64 / tp as f64 } else { 0.0 };

    DeviceScore {
        true_positives: tp,
        false_positives: fp,
        false_negatives: fneg,
        precision,
        recall,
        height_mae_in: height_mae,
        position_mae_ft: position_mae,
        zone_containment: zone_cont,
        kind_accuracy: kind_acc,
    }
}

/// Create perfect found devices from the truth (for testing the scorer baseline).
pub fn perfect_devices(truth: &Truth) -> Vec<FoundDevice> {
    truth
        .elec_devices
        .iter()
        .map(|d| FoundDevice {
            level: d.level.clone(),
            kind: d.kind.clone(),
            type_tag: d.type_tag.clone(),
            room: d.room.clone(),
            at_ft: d.at_ft,
            mounting_in: d.mounting_in,
            height_source: if d.mounting_note.is_some() { "note" } else { "rule" }.into(),
            arch_positioned: d.arch_positioned,
            zone: if !d.arch_positioned {
                // The truth position itself is in any correct zone centred on it.
                Some(DeviceZone {
                    room: d.room.clone(),
                    wall_side: WallSide::South,
                    axis: ZoneAxis::X,
                    range_ft: [d.at_ft[0] - 3.0, d.at_ft[0] + 3.0],
                    height_in: [0.0, 120.0],
                    field_decision: true,
                })
            } else {
                None
            },
            source: String::new(),
            properties: BTreeMap::new(),
            drawn_ft: d.drawn_ft,
            notes: truth
                .elec_spec
                .note(&d.id)
                .map(|n| n.subscripts.iter().cloned().chain((!n.circuit_ref.is_empty()).then(|| n.circuit_ref.clone())).collect())
                .unwrap_or_default(),
        })
        .collect()
}
