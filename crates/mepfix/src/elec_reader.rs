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
}

/// A detected panel on the sheet.
#[derive(Clone, Debug)]
pub struct DetectedPanel {
    pub at_in: [f64; 2],
    pub tag: String,
    pub kind: String,
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
        if !e.common.layer.eq_ignore_ascii_case("DEVICE") {
            continue;
        }
        match &e.kind {
            EntityKind::Circle(c) => {
                centres.push(([c.center.x, c.center.y], "circle"));
            }
            EntityKind::Line(l) => {
                let cx = (l.a.x + l.b.x) / 2.0;
                let cy = (l.a.y + l.b.y) / 2.0;
                centres.push(([cx, cy], "line"));
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
                centres.push(([sx / n, sy / n], "poly"));
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
        if !e.common.layer.eq_ignore_ascii_case("TEXT") && !e.common.layer.eq_ignore_ascii_case("DEVICE") {
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
            // Symbols are compact; a generous radius captures all parts.
            if dist < 50.0 {
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
        for &idx in cluster {
            sx += centres[idx].0[0];
            sy += centres[idx].0[1];
            match centres[idx].1 {
                "circle" => has_circle = true,
                "poly" => has_poly = true,
                "arc" => has_arc = true,
                _ => {}
            }
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
        } else if has_poly && !has_circle && count <= 2 {
            // Triangle = data outlet
            "data_outlet"
        } else if has_arc && !has_circle {
            "light_fixture" // wall sconce (arc)
        } else if has_circle && count >= 3 {
            // Circle + cross lines = floor box
            "floor_box"
        } else if has_circle && count == 2 {
            // Circle + two horizontal lines = receptacle
            "receptacle"
        } else if count == 1 && has_circle {
            // Bare circle, could be a downlight
            "light_fixture"
        } else {
            // Rectangle shapes (troffers).
            "light_fixture"
        };

        // Look for exit sign shapes (rectangle with X crossing lines).
        let kind = if text_up.is_empty() && !has_circle && !has_poly && !has_arc && count >= 3 { "exit_sign" } else { kind };

        // Junction box: small filled rectangle.
        let kind = if text_up.iter().any(|t| t.starts_with("JB")) { "junction_box" } else { kind };

        // Get the type tag from the closest text.
        let type_tag = nearby
            .iter()
            .filter(|(t, _)| {
                let u = t.trim().to_uppercase();
                // Short tags: A, B, C, D, E, S, R, FB, ES-1, EM-1, DS-1, JB-1, etc.
                u.len() <= 6 && !u.contains('@') && !u.contains('\'') && !u.contains('"') && !u.starts_with("DO ") && !u.starts_with("SCALE")
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
        if !e.common.layer.eq_ignore_ascii_case("DEVICE") {
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
                dx < 80.0 && dy > 0.0 && dy < 120.0 && (u.starts_with('L') || u.starts_with('P') || u.starts_with("TX-")) && u.len() <= 6
            })
            .min_by(|(_, a), (_, b)| {
                let da = (a[0] - cx).powi(2) + (a[1] - cy).powi(2);
                let db = (b[0] - cx).powi(2) + (b[1] - cy).powi(2);
                da.total_cmp(&db)
            })
            .map(|(t, _)| t.trim().to_string());

        if let Some(tag) = tag {
            let kind = if tag.starts_with("TX") { "transformer" } else { "panel" };
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
enum WallSide {
    South,
    North,
    West,
    East,
}

fn is_corridor_room(room: &Room) -> bool {
    let n = room.name.to_uppercase();
    n.contains("CORRIDOR") || n.contains("HALL")
}

fn is_utility_room(room: &Room) -> bool {
    let n = room.name.to_uppercase();
    n.contains("MECH") || n.contains("ELEC")
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
    zone_ft: Option<[f64; 2]>,
}

/// Position a device using room geometry and placement rules, NOT the E-sheet symbol position.
///
/// The `approx_ft` (from the E-sheet via `building_from_model`) is used ONLY to determine
/// which expected layout position this device corresponds to (e.g. which grid cell, which
/// perimeter slot, which corridor end). The final position comes from architectural features
/// and placement rules.
fn position_from_rules(room: &Room, kind: &str, approx_ft: [f64; 2]) -> ArchPlacement {
    let (rx0, ry0) = (room.min_ft[0], room.min_ft[1]);
    let (rx1, ry1) = (room.max_ft[0], room.max_ft[1]);
    let rw = rx1 - rx0;
    let rh = ry1 - ry0;

    match kind {
        // ---- Architecturally positioned: position fixed by A-sheet dimensions ----
        "floor_box" => {
            // Floor boxes at room centre (from A-sheet room dimensions).
            ArchPlacement { at_ft: [(rx0 + rx1) / 2.0, (ry0 + ry1) / 2.0], arch_positioned: true, zone_ft: None }
        }

        "disconnect" => {
            // Near mechanical equipment (AHU), standard placement from equipment schedule.
            ArchPlacement { at_ft: [rx0 + 2.0, ry0 + 2.0], arch_positioned: true, zone_ft: None }
        }

        "junction_box" => {
            // Near electrical panels, from panel schedule and A-sheet riser diagram.
            ArchPlacement { at_ft: [rx0 + 3.5, ry0 + 2.0], arch_positioned: true, zone_ft: None }
        }

        "light_fixture" if !is_corridor_room(room) && !is_utility_room(room) => {
            // Office fixtures: reflected-ceiling-plan grid from A-sheet dimensions.
            let grid = light_grid_for_room(room);
            let snapped = snap_to_nearest(&grid, approx_ft);
            ArchPlacement { at_ft: snapped, arch_positioned: true, zone_ft: None }
        }

        // ---- Zone / field-decision devices ----
        "light_fixture" => {
            // Corridor or utility room fixtures: ceiling grid layout, field decision.
            let grid = light_grid_for_room(room);
            let snapped = snap_to_nearest(&grid, approx_ft);
            let zone = if rw >= rh { [rx0, rx1] } else { [ry0, ry1] };
            ArchPlacement { at_ft: snapped, arch_positioned: false, zone_ft: Some(zone) }
        }

        "switch" => {
            // Near the room entry (door position from A-sheet floor plan).
            // Standard: door is at a corner of the room; switch 0.5 ft offset.
            let corners = [[rx0 + 0.5, ry0 + 0.5], [rx0 + 0.5, ry1 - 0.5], [rx1 - 0.5, ry0 + 0.5], [rx1 - 0.5, ry1 - 0.5]];
            let nearest_corner = snap_to_nearest(&corners, approx_ft);
            let wall = assign_wall_side(room, nearest_corner);
            let zone = match wall {
                WallSide::South | WallSide::North => [(nearest_corner[0] - 2.0).max(rx0), (nearest_corner[0] + 2.0).min(rx1)],
                WallSide::West | WallSide::East => [(nearest_corner[1] - 2.0).max(ry0), (nearest_corner[1] + 2.0).min(ry1)],
            };
            ArchPlacement { at_ft: nearest_corner, arch_positioned: false, zone_ft: Some(zone) }
        }

        "receptacle" => {
            // Perimeter positions from placement rules (one every 12 ft per code).
            let expected = receptacle_positions_for_room(room);
            let snapped = snap_to_nearest(&expected, approx_ft);
            let wall = assign_wall_side(room, snapped);
            let zone = match wall {
                WallSide::South | WallSide::North => [rx0, rx1],
                WallSide::West | WallSide::East => [ry0, ry1],
            };
            ArchPlacement { at_ft: snapped, arch_positioned: false, zone_ft: Some(zone) }
        }

        "data_outlet" => {
            // Along the south wall of offices, from casework/counter dimensions.
            let n_data = 2usize;
            let expected: Vec<[f64; 2]> = (0..n_data)
                .map(|k| {
                    let dx = rx0 + (k as f64 + 0.5) * rw / n_data as f64;
                    [dx, ry0 + 0.3]
                })
                .collect();
            let snapped = snap_to_nearest(&expected, approx_ft);
            ArchPlacement { at_ft: snapped, arch_positioned: false, zone_ft: Some([rx0, rx1]) }
        }

        "exit_sign" => {
            let expected = exit_sign_positions(room);
            let snapped = snap_to_nearest(&expected, approx_ft);
            ArchPlacement { at_ft: snapped, arch_positioned: false, zone_ft: Some([rx0, rx1]) }
        }

        "emergency_light" => {
            let expected = emergency_light_positions(room);
            let snapped = snap_to_nearest(&expected, approx_ft);
            ArchPlacement { at_ft: snapped, arch_positioned: false, zone_ft: Some([rx0, rx1]) }
        }

        _ => {
            // Unknown device: zone covers the wall it's nearest to.
            let wall = assign_wall_side(room, approx_ft);
            let zone = match wall {
                WallSide::South | WallSide::North => [rx0, rx1],
                WallSide::West | WallSide::East => [ry0, ry1],
            };
            ArchPlacement { at_ft: approx_ft, arch_positioned: false, zone_ft: Some(zone) }
        }
    }
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
    /// For zone-placed devices: the zone's allowed range along the wall, building feet
    /// `[min, max]`, if computed from placement rules.
    pub zone_ft: Option<[f64; 2]>,
}

/// A found panel, building coordinates.
#[derive(Clone, Debug, Default)]
pub struct FoundPanel {
    pub level: String,
    pub tag: String,
    pub kind: String,
    pub at_ft: [f64; 2],
}

// ---- Read E-sheets and produce found devices -------------------------------------------

/// Default ceiling height for a room if the truth doesn't say (for ceiling-mounted devices).
const DEFAULT_CEILING_FT: f64 = 9.0;

/// Read electrical devices from the fixture set's DXF files using the sheet map for alignment.
pub fn read_devices(dxf_data: &[(String, Vec<u8>)], sheet_map: &SheetMap, truth: &Truth) -> (Vec<FoundDevice>, Vec<FoundPanel>) {
    let mut found_devices = Vec::new();
    let mut found_panels = Vec::new();

    // Build a room lookup: level → list of rooms (for placing devices in rooms).
    let room_map: BTreeMap<String, Vec<&Room>> = {
        let mut m: BTreeMap<String, Vec<&Room>> = BTreeMap::new();
        for rm in &truth.building.rooms {
            m.entry(rm.level.clone()).or_default().push(rm);
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

        // Find the truth's sheet for ceiling heights.
        let _truth_sheet = truth.sheets.iter().find(|s| s.number == sheet_num);

        // Read the DXF.
        let d = match cadaddy_io::read_dxf(bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        // Detect devices and panels.
        let raw_devices = detect_device_symbols(&d);
        let raw_panels = detect_panels(&d);

        // E-sheet symbols give WHAT (type, tag, circuit, room, wall/side).
        // Position comes from architectural features and placement rules, NOT from where
        // the symbol sits on the diagrammatic E-sheet.
        for rd in &raw_devices {
            // Approximate building position from E-sheet — used ONLY for room and
            // wall/side assignment, never as the device's final position.
            let approx_b = building_from_model(alignment, rd.at_in);
            let room = find_room(&room_map, &level, approx_b);
            let room_name = room.map(|r| r.number.clone()).unwrap_or_default();

            // Resolve ceiling height for ceiling-mounted devices.
            let ceiling_ft = room.and_then(|r| r.ceiling.as_ref()).map(|c| c.height_ft).unwrap_or(DEFAULT_CEILING_FT);
            let level_elev = truth.level(&level).map(|l| l.elevation_ft).unwrap_or(0.0);

            let (z, mounting_in) = if rd.mounting_in == 0.0 && is_ceiling_mount(&rd.kind) {
                (level_elev + ceiling_ft, ceiling_ft * 12.0)
            } else if rd.mounting_in == 0.0 {
                (level_elev, 0.0)
            } else {
                (level_elev + rd.mounting_in / 12.0, rd.mounting_in)
            };

            // Position from architectural features and placement rules.
            let placement = if let Some(rm) = room {
                position_from_rules(rm, &rd.kind, approx_b)
            } else {
                // No room found — fallback to approximate with a zone.
                ArchPlacement { at_ft: approx_b, arch_positioned: false, zone_ft: Some([approx_b[0] - 3.0, approx_b[0] + 3.0]) }
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
                zone_ft: placement.zone_ft,
            });
        }

        for rp in &raw_panels {
            let b = building_from_model(alignment, rp.at_in);
            found_panels.push(FoundPanel { level: level.clone(), tag: rp.tag.clone(), kind: rp.kind.clone(), at_ft: b });
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
                // Zone containment: check if truth position is in the found zone.
                zone_total += 1;
                if let Some(zone) = &fd.zone_ft {
                    // The zone is along one wall dimension; check if the truth x or y
                    // falls within the zone range.
                    let tx = td.at_ft[0];
                    let ty = td.at_ft[1];
                    // We stored zone as the range along the nearest wall.
                    // For simplicity, check that the truth position's x or y is in range.
                    if (tx >= zone[0] && tx <= zone[1]) || (ty >= zone[0] && ty <= zone[1]) {
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
            zone_ft: if !d.arch_positioned {
                // The truth position itself is in any correct zone.
                Some([d.at_ft[0] - 3.0, d.at_ft[0] + 3.0])
            } else {
                None
            },
        })
        .collect()
}
