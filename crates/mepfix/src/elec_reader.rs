//! Electrical device reader: recognises device symbols on E-sheets (lighting and power plans),
//! attaches types and circuit tags, places each device at its mounting height (from a note on the
//! device, else the sheet legend, else the mounting-height rules file) and records the height
//! source so guesses are visible. Position comes from the A-sheets where they fix it (a device
//! dimensioned on the floor plan, a light fixture on the reflected ceiling plan); everything else
//! is a zone (a wall or ceiling stretch with its allowed range), flagged field-decision.
//!
//! The reader never sees the answer key: rooms and ceiling heights come from the ceiling reader,
//! level elevations from the cover sheet's level list, locations from the A-sheets
//! ([`DeviceInputs::read`]), or from an input model the caller builds.
//!
//! 3D output: simple shapes on per-type layers (`3D-ELEC-<kind>`); IFC types are
//! IfcLightFixture / IfcOutlet / IfcSwitchingDevice / IfcJunctionBox.

use crate::sheet_reader::{Alignment, SheetMap};
use crate::truth::Truth;
use cadaddy_doc::{Drawing, EntityKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

// ---- Mounting-height rules file ---------------------------------------------------------

/// The shipped mounting-height rules (`data/mounting_heights.json`).
pub const DEFAULT_RULES_JSON: &str = include_str!("../data/mounting_heights.json");
/// The name of a project's own rules file, in the project folder.
pub const RULES_FILE_NAME: &str = "mounting_heights.json";
/// Largest rules file read (bytes) and most kinds in it: it's a short hand-edited list.
const MAX_RULES_BYTES: u64 = 64 * 1024;
const MAX_RULE_KINDS: usize = 256;

/// Default mounting heights by device kind: used when a device has no note on the E-sheet and
/// the sheet's legend gives no height for its kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MountingRules {
    /// Wall and floor devices: inches above the finished floor, by kind.
    #[serde(default)]
    pub heights_in: BTreeMap<String, f64>,
    /// Kinds mounted on the ceiling: their height is the room's ceiling.
    #[serde(default)]
    pub ceiling: Vec<String>,
    /// A wall device whose kind isn't listed, inches.
    #[serde(default = "default_other_in")]
    pub other_in: f64,
    /// Ceiling height when the readers give none for the room, feet.
    #[serde(default = "default_ceiling_ft")]
    pub default_ceiling_ft: f64,
}

fn default_other_in() -> f64 {
    18.0
}
fn default_ceiling_ft() -> f64 {
    9.0
}

impl Default for MountingRules {
    /// The shipped rules file (a ceiling-only fallback if it ever failed to parse; a test proves
    /// it parses).
    fn default() -> Self {
        Self::from_json(DEFAULT_RULES_JSON).unwrap_or_else(|_| MountingRules {
            heights_in: BTreeMap::new(),
            ceiling: vec!["light_fixture".into(), "exit_sign".into(), "junction_box".into()],
            other_in: default_other_in(),
            default_ceiling_ft: default_ceiling_ft(),
        })
    }
}

impl MountingRules {
    /// Parse a rules file. Heights must be finite, 0–240" (ceiling 0–60').
    pub fn from_json(text: &str) -> Result<Self, String> {
        let r: MountingRules = serde_json::from_str(text).map_err(|e| format!("mounting rules: {e}"))?;
        if r.heights_in.len() + r.ceiling.len() > MAX_RULE_KINDS {
            return Err(format!("mounting rules: more than {MAX_RULE_KINDS} kinds"));
        }
        let ok_in = |v: f64| v.is_finite() && (0.0..=240.0).contains(&v);
        if let Some((k, v)) = r.heights_in.iter().find(|(_, v)| !ok_in(**v)) {
            return Err(format!("mounting rules: {k} = {v} is not a height in inches (0-240)"));
        }
        if !ok_in(r.other_in) {
            return Err(format!("mounting rules: other_in = {} is not a height in inches (0-240)", r.other_in));
        }
        if !(r.default_ceiling_ft.is_finite() && r.default_ceiling_ft > 0.0 && r.default_ceiling_ft <= 60.0) {
            return Err(format!("mounting rules: default_ceiling_ft = {} is not a ceiling height (feet)", r.default_ceiling_ft));
        }
        Ok(r)
    }

    /// Read a rules file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        if len > MAX_RULES_BYTES {
            return Err(format!("{}: {len} bytes is too big for a rules file", path.display()));
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A project's rules: its folder's `mounting_heights.json` when there is one, else the
    /// shipped defaults.
    pub fn for_project(dir: &Path) -> Result<Self, String> {
        let p = dir.join(RULES_FILE_NAME);
        if p.is_file() { Self::load(&p) } else { Ok(Self::default()) }
    }

    /// Whether a kind is mounted on the ceiling.
    pub fn is_ceiling(&self, kind: &str) -> bool {
        self.ceiling.iter().any(|k| k == kind)
    }

    /// The rule height for a wall or floor device, inches AFF.
    pub fn height_in(&self, kind: &str) -> f64 {
        self.heights_in.get(kind).copied().unwrap_or(self.other_in)
    }
}

// ---- Legend parsing ---------------------------------------------------------------------

/// A legend row read from the E-sheet: its sample symbol, its description, the device kind the
/// description names, and the mounting height it gives (`+18" AFF U.N.O.`), if any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LegendEntry {
    /// The sample symbol's shapes and inner letters (`circle 2-line`, `circle S`).
    pub symbol: String,
    pub description: String,
    pub kind: String,
    pub mounting_in: Option<f64>,
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
    /// Mounting height in inches AFF from a note on the device (`0` = ceiling or floor, as the
    /// note says).
    pub mounting_in: f64,
    /// `"note"` when a note on the device gave the height, else empty (the legend or the rules
    /// decide).
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

/// Detect device symbols on a DEVICE layer in the DXF.
///
/// Strategy: on E-sheets, devices are drawn as circles, rectangles, arcs, polygons on the
/// DEVICE layer; nearby text on TEXT gives tags and mounting notes. We cluster DEVICE-layer
/// geometry into symbol groups, then classify each by shape and nearby text. Geometry inside a
/// `skip` box (model inches: the legend's sample symbols) is not a device.
fn detect_device_symbols(d: &Drawing, skip: &[[[f64; 2]; 2]]) -> Vec<DetectedDevice> {
    let skipped = |p: [f64; 2]| skip.iter().any(|b| p[0] >= b[0][0] && p[0] <= b[1][0] && p[1] >= b[0][1] && p[1] <= b[1][1]);
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
    centres.retain(|(c, _)| !skipped(*c));

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

        // The symbol's own texts (its tag, letters inside it, subscripts, circuit, height note):
        // right beside it, nearest first; a neighbour's are feet away.
        let mut close: Vec<&(String, [f64; 2])> = nearby.iter().copied().filter(|(_, p)| (p[0] - cx).hypot(p[1] - cy) < CLOSE_TEXT_IN).collect();
        close.sort_by(|a, b| (a.1[0] - cx).hypot(a.1[1] - cy).total_cmp(&(b.1[0] - cx).hypot(b.1[1] - cy)));

        // Classify by its own texts and shape.
        let text_up: Vec<String> = close.iter().map(|(t, _)| t.trim().to_uppercase()).collect();

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

        // A height note printed on the device; else the legend or the rules decide later.
        let (mounting_in, height_source, mounting_note) = match parse_mounting_height(&close) {
            Some((h, note)) => (h, "note", Some(note)),
            None => (0.0, "", None),
        };

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
        // The nearest device without a label yet gets it.
        if let Some(dev) = devices.iter_mut().filter(|d| d.circuit_label.is_none()).min_by(|a, b| {
            let da = (a.at_in[0] - cl.1[0]).powi(2) + (a.at_in[1] - cl.1[1]).powi(2);
            let db = (b.at_in[0] - cl.1[0]).powi(2) + (b.at_in[1] - cl.1[1]).powi(2);
            da.total_cmp(&db)
        }) {
            dev.circuit_label = Some(cl.0.clone());
        }
    }

    devices
}

/// A mounting height note among a device's texts: inches AFF (`0` for `CLG` / `FLOOR`) and the
/// note as printed.
fn parse_mounting_height(texts: &[&(String, [f64; 2])]) -> Option<(f64, String)> {
    // Look for explicit mounting height notes: +18" AFF, +46", +60", CLG, FLOOR, +84", etc.
    for (text, _) in texts {
        let upper = text.to_uppercase();
        if upper.contains("CLG") || upper.contains("CEILING") {
            return Some((0.0, "CLG".into())); // 0 = ceiling, resolved later
        }
        if upper.contains("FLOOR") {
            return Some((0.0, "FLOOR".into()));
        }
        // Parse "+NN" or "+NN" AFF" patterns.
        if let Some(rest) = upper.strip_prefix('+') {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(inches) = digits.parse::<f64>()
                && inches > 0.0
                && inches <= 200.0
            {
                return Some((inches, text.clone()));
            }
        }
    }
    None
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

/// Read a sheet's legend: each LEGEND-layer description with the sample symbol drawn left of it
/// (DEVICE layer), the kind the description names ([`classify_legend`]) and the height it gives.
/// Also returns the boxes (model inches) the sample symbols sit in, so they aren't read as
/// devices. `model_per_paper_in`: model inches per paper inch on this sheet.
pub fn read_legend(d: &Drawing, model_per_paper_in: f64) -> (Vec<LegendEntry>, Vec<[[f64; 2]; 2]>) {
    let k = model_per_paper_in;
    if !(k.is_finite() && k > 0.0) {
        return (Vec::new(), Vec::new());
    }
    // The sample symbol sits half a paper inch left of its description, a little above its
    // baseline; rows are a quarter inch apart.
    let (dx, dy, half_w, half_h) = (-0.5 * k, 0.04 * k, 0.35 * k, 0.11 * k);
    let mut parts: Vec<([f64; 2], String)> = Vec::new();
    let mut letters: Vec<([f64; 2], String)> = Vec::new();
    for e in d.model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case("DEVICE")) {
        match &e.kind {
            EntityKind::Circle(c) => parts.push(([c.center.x, c.center.y], "circle".into())),
            EntityKind::Arc(a) => parts.push(([a.center.x, a.center.y], "arc".into())),
            EntityKind::Line(l) => parts.push(([(l.a.x + l.b.x) / 2.0, (l.a.y + l.b.y) / 2.0], "line".into())),
            EntityKind::LwPolyline(p) if !p.vertices.is_empty() => {
                let n = p.vertices.len() as f64;
                let c = [p.vertices.iter().map(|v| v.p.x).sum::<f64>() / n, p.vertices.iter().map(|v| v.p.y).sum::<f64>() / n];
                let corners = if p.closed { p.vertices.len() } else { p.vertices.len().saturating_sub(1) };
                parts.push((c, format!("poly{corners}")));
            }
            EntityKind::Solid(s) => {
                let c = [s.corners.iter().map(|p| p.x).sum::<f64>() / 4.0, s.corners.iter().map(|p| p.y).sum::<f64>() / 4.0];
                parts.push((c, "fill".into()));
            }
            EntityKind::Text(t) => letters.push(([t.insert.x, t.insert.y], t.value.trim().to_ascii_uppercase())),
            _ => {}
        }
    }
    let mut entries = Vec::new();
    let mut boxes = Vec::new();
    for e in d.model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case("LEGEND")) {
        let EntityKind::Text(t) = &e.kind else { continue };
        let desc = t.value.trim();
        let up = desc.to_ascii_uppercase();
        if desc.is_empty() || up == "LEGEND" || up == "ABBREVIATIONS" || desc.contains(" = ") {
            continue;
        }
        let c = [t.insert.x + dx, t.insert.y + dy];
        let inside = |p: &[f64; 2]| (p[0] - c[0]).abs() <= half_w && (p[1] - c[1]).abs() <= half_h;
        let mut shapes: Vec<&str> = parts.iter().filter(|(p, _)| inside(p)).map(|(_, s)| s.as_str()).collect();
        if shapes.is_empty() {
            continue; // Not a legend row with a sample symbol.
        }
        shapes.sort_unstable();
        let mut sig: Vec<String> = Vec::new();
        for s in shapes.iter().copied().collect::<BTreeSet<&str>>() {
            let n = shapes.iter().filter(|x| **x == s).count();
            sig.push(if n > 1 { format!("{n}-{s}") } else { s.to_string() });
        }
        let inner: Vec<&str> = letters.iter().filter(|(p, _)| inside(p)).map(|(_, s)| s.as_str()).collect();
        sig.extend(inner.iter().map(|s| s.to_string()));
        boxes.push([[c[0] - half_w, c[1] - half_h], [c[0] + half_w, c[1] + half_h]]);
        let Some(kind) = classify_legend(inner.first().copied().unwrap_or(""), desc) else { continue };
        entries.push(LegendEntry { symbol: sig.join(" "), description: desc.to_string(), kind, mounting_in: legend_height_in(&up) });
    }
    (entries, boxes)
}

/// A height a legend description gives: `+18" AFF U.N.O.` → 18.
fn legend_height_in(up: &str) -> Option<f64> {
    let (_, rest) = up.split_once('+')?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let v: f64 = digits.parse().ok()?;
    (v > 0.0 && v <= 240.0 && rest.get(digits.len()..)?.starts_with('"')).then_some(v)
}

// ---- What the reader is given: rooms, levels, A-sheet locations, rules --------------------
//
// E-sheets are diagrammatic: a symbol says WHAT (type, tag, circuit, room, wall/side), not where.
// Where comes from the A-sheets: a device dimensioned on the floor plan, a light fixture drawn
// on the reflected ceiling plan. A device the A-sheets don't fix is a ZONE (the wall or ceiling
// stretch it may go on), flagged field-decision with status `rule` — never an invented point.

/// A level and its floor elevation, feet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InputLevel {
    pub code: String,
    pub elevation_ft: f64,
}

/// A room from the readers (or an input model): its box in building feet and its ceiling height
/// above the floor, when known.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InputRoom {
    pub level: String,
    pub number: String,
    pub name: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub ceiling_ft: Option<f64>,
}

/// A place an A-sheet fixes for a device: a dimensioned device on a floor plan (`source =
/// "dimension"`) or a light fixture drawn on a reflected ceiling plan (`"rcp"`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Locator {
    pub level: String,
    /// The A-sheet it is on.
    pub sheet: String,
    pub kind: String,
    /// Building feet.
    pub at_ft: [f64; 2],
    pub source: String,
}

/// Everything the device reader needs besides the E-sheets. Build it from the documents with
/// [`DeviceInputs::read`], or fill it from a model the caller already has.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceInputs {
    pub levels: Vec<InputLevel>,
    pub rooms: Vec<InputRoom>,
    pub locators: Vec<Locator>,
    pub rules: MountingRules,
    /// What reading the inputs couldn't do (a sheet that failed to read, no level list…).
    pub findings: Vec<String>,
}

/// Layer the A floor plans put dimensioned device locations on, and the reflected ceiling plans
/// their light fixtures.
pub const LOCATOR_LAYER: &str = "E-DEVC-IDEN";
pub const RCP_FIXTURE_LAYER: &str = "E-LITE";
/// A dimension line ends at a device's mark within this (model inches).
const DIM_END_IN: f64 = 1.0;
/// A device's mark letters are within this of its circle (model inches).
const MARK_TEXT_IN: f64 = 18.0;
/// An E-symbol takes an A-sheet location at most this far from where it is drawn (feet): E-sheets
/// are diagrammatic, but not by more than this.
const LOCATOR_REACH_FT: f64 = 6.0;
/// A symbol drawn outside every room belongs to the nearest one this close (feet).
const ROOM_REACH_FT: f64 = 1.5;

/// The device kind an A-sheet mark names.
fn kind_of_mark(mark: &str) -> Option<&'static str> {
    Some(match mark.trim().to_ascii_uppercase().as_str() {
        "R" => "receptacle",
        "S" => "switch",
        "D" => "data_outlet",
        "FB" => "floor_box",
        "DS" => "disconnect",
        "JB" => "junction_box",
        "EM" => "emergency_light",
        "X" => "exit_sign",
        _ => return None,
    })
}

impl DeviceInputs {
    /// Read the inputs from the set's documents: rooms and ceiling heights from the ceiling
    /// reader (reflected ceiling plans), level elevations from the cover sheet's level list,
    /// device locations from the A floor plans and reflected ceiling plans, default rules.
    pub fn read(dxf_data: &[(String, Vec<u8>)], sheet_map: &SheetMap) -> DeviceInputs {
        let mut inputs = DeviceInputs::default();
        let ceil = crate::ceiling_reader::read_ceilings(dxf_data, sheet_map);
        inputs.rooms = ceil
            .rooms
            .iter()
            .map(|r| InputRoom {
                level: r.level.clone(),
                number: r.number.clone(),
                name: r.name.clone(),
                min_ft: r.min_ft,
                max_ft: r.max_ft,
                ceiling_ft: ceil
                    .ceilings
                    .iter()
                    .find(|c| c.level == r.level && c.room_number == r.number && !c.open_to_structure)
                    .and_then(|c| c.height_ft),
            })
            .collect();
        for (name, bytes) in dxf_data {
            let stem = sheet_stem(name);
            let Some(rs) = sheet_map.sheets.iter().find(|s| s.number == stem) else { continue };
            let title = rs.title.to_ascii_uppercase();
            let cover = rs.discipline == "G";
            let floor = rs.discipline == "A" && title.contains("FLOOR PLAN") && !title.contains("ENLARGED");
            let rcp = rs.discipline == "A" && title.contains("REFLECTED CEILING");
            if !(cover || floor || rcp) {
                continue;
            }
            let d = match cadaddy_io::read_dxf(bytes) {
                Ok(d) => d,
                Err(e) => {
                    inputs.findings.push(format!("{stem}: could not read the drawing ({e})"));
                    continue;
                }
            };
            if cover {
                for l in read_level_list(&d) {
                    if !inputs.levels.iter().any(|x| x.code == l.code) {
                        inputs.levels.push(l);
                    }
                }
                continue;
            }
            let (Some(level), Some(al)) = (&rs.level, &rs.alignment) else { continue };
            if floor {
                inputs.locators.extend(read_locators(&d, al, level, stem));
            } else {
                inputs.locators.extend(read_rcp_fixtures(&d, al, level, stem));
            }
        }
        if inputs.levels.is_empty() {
            inputs.findings.push("no level list found on the cover sheet: device heights are above each level's floor at elevation 0".into());
        }
        inputs
    }

    /// The room a point is in (the smallest, where rooms overlap); else the nearest room within
    /// [`ROOM_REACH_FT`] (E-sheet symbols at a wall are often drawn just past it).
    fn room(&self, level: &str, p: [f64; 2]) -> Option<&InputRoom> {
        let gap = |r: &InputRoom| {
            let dx = (r.min_ft[0] - p[0]).max(p[0] - r.max_ft[0]).max(0.0);
            let dy = (r.min_ft[1] - p[1]).max(p[1] - r.max_ft[1]).max(0.0);
            dx.hypot(dy)
        };
        let on_level = || self.rooms.iter().filter(|r| r.level == level);
        on_level()
            .filter(|r| gap(r) == 0.0)
            .min_by(|a, b| area(a).total_cmp(&area(b)))
            .or_else(|| on_level().map(|r| (gap(r), r)).filter(|(g, _)| *g <= ROOM_REACH_FT).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, r)| r))
    }

    fn elevation(&self, level: &str) -> Option<f64> {
        self.levels.iter().find(|l| l.code == level).map(|l| l.elevation_ft)
    }
}

fn area(r: &InputRoom) -> f64 {
    (r.max_ft[0] - r.min_ft[0]) * (r.max_ft[1] - r.min_ft[1])
}

fn sheet_stem(name: &str) -> &str {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.strip_suffix(".dxf").or_else(|| base.strip_suffix(".DXF")).unwrap_or(base)
}

/// The cover sheet's level list: `LEVEL 2 = EL 14'-0"`, `ROOF = EL 28'-0"`.
pub fn read_level_list(d: &Drawing) -> Vec<InputLevel> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        let EntityKind::Text(t) = &e.kind else { continue };
        let up = t.value.trim().to_ascii_uppercase();
        let Some((name, el)) = up.split_once(" = EL ") else { continue };
        let name = name.trim();
        let code = match name.strip_prefix("LEVEL ") {
            Some(c) => c.trim().to_string(),
            None if name == "ROOF" => "R".to_string(),
            None => continue,
        };
        let Some(ft) = crate::heights::parse_feet_inches(el) else { continue };
        if !code.is_empty() && ft.is_finite() && out.len() < 200 {
            out.push(InputLevel { code, elevation_ft: ft });
        }
    }
    out
}

/// Dimensioned device locations on an A floor plan: a mark circle on [`LOCATOR_LAYER`] that a
/// dimension line ends at, with its mark letters beside it. A mark no dimension reaches fixes
/// nothing.
fn read_locators(d: &Drawing, al: &Alignment, level: &str, sheet: &str) -> Vec<Locator> {
    let mut ends: Vec<[f64; 2]> = Vec::new();
    let mut marks: Vec<([f64; 2], String)> = Vec::new();
    let mut circles: Vec<[f64; 2]> = Vec::new();
    for e in d.model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case(LOCATOR_LAYER)) {
        match &e.kind {
            EntityKind::Line(l) => ends.extend([[l.a.x, l.a.y], [l.b.x, l.b.y]]),
            EntityKind::Circle(c) => circles.push([c.center.x, c.center.y]),
            EntityKind::Text(t) => {
                let p = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                marks.push((p, t.value.clone()));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in circles {
        if !ends.iter().any(|p| (p[0] - c[0]).hypot(p[1] - c[1]) <= DIM_END_IN) {
            continue;
        }
        let kind = marks
            .iter()
            .filter_map(|(p, m)| Some(((p[0] - c[0]).hypot(p[1] - c[1]), kind_of_mark(m)?)))
            .filter(|(dist, _)| *dist <= MARK_TEXT_IN)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, k)| k);
        if let Some(kind) = kind {
            let at = crate::sheet_reader::building_from_model(al, c);
            out.push(Locator { level: level.to_string(), sheet: sheet.to_string(), kind: kind.to_string(), at_ft: at, source: "dimension".into() });
        }
    }
    out
}

/// Light fixtures drawn on a reflected ceiling plan ([`RCP_FIXTURE_LAYER`]): their centres.
fn read_rcp_fixtures(d: &Drawing, al: &Alignment, level: &str, sheet: &str) -> Vec<Locator> {
    let mut out = Vec::new();
    for e in d.model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case(RCP_FIXTURE_LAYER)) {
        let c = match &e.kind {
            EntityKind::Circle(c) => [c.center.x, c.center.y],
            EntityKind::LwPolyline(p) if p.closed && !p.vertices.is_empty() => {
                let n = p.vertices.len() as f64;
                [p.vertices.iter().map(|v| v.p.x).sum::<f64>() / n, p.vertices.iter().map(|v| v.p.y).sum::<f64>() / n]
            }
            _ => continue,
        };
        let at = crate::sheet_reader::building_from_model(al, c);
        out.push(Locator { level: level.to_string(), sheet: sheet.to_string(), kind: "light_fixture".into(), at_ft: at, source: "rcp".into() });
    }
    out
}

/// A zone: the wall (or ceiling line) a device goes on and the range along it, and a point on it
/// near where the E-sheet draws the device (for display; not a location).
struct Zone {
    at_ft: [f64; 2],
    line: [[f64; 2]; 2],
    range: [f64; 2],
}

/// The zone for a device the A-sheets don't fix: a wall device on the wall of its room nearest
/// where it is drawn (the whole wall), a ceiling or floor device along its room's long axis; no
/// room: six feet around where it is drawn.
fn zone_for(room: Option<&InputRoom>, wall_mounted: bool, approx: [f64; 2]) -> Zone {
    let Some(r) = room else {
        return Zone { at_ft: approx, line: [[approx[0] - 3.0, approx[1]], [approx[0] + 3.0, approx[1]]], range: [approx[0] - 3.0, approx[0] + 3.0] };
    };
    let ([x0, y0], [x1, y1]) = (r.min_ft, r.max_ft);
    if wall_mounted {
        let sides = [
            ((approx[1] - y0).abs(), [[x0, y0], [x1, y0]]),
            ((y1 - approx[1]).abs(), [[x0, y1], [x1, y1]]),
            ((approx[0] - x0).abs(), [[x0, y0], [x0, y1]]),
            ((x1 - approx[0]).abs(), [[x1, y0], [x1, y1]]),
        ];
        let line = sides.iter().min_by(|a, b| a.0.total_cmp(&b.0)).map_or([[x0, y0], [x1, y0]], |s| s.1);
        let horizontal = (line[0][1] - line[1][1]).abs() < 1e-9;
        let (at, range) =
            if horizontal { ([approx[0].clamp(x0, x1), line[0][1]], [x0, x1]) } else { ([line[0][0], approx[1].clamp(y0, y1)], [y0, y1]) };
        return Zone { at_ft: at, line, range };
    }
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let at = [approx[0].clamp(x0, x1), approx[1].clamp(y0, y1)];
    if x1 - x0 >= y1 - y0 {
        Zone { at_ft: at, line: [[x0, cy], [x1, cy]], range: [x0, x1] }
    } else {
        Zone { at_ft: at, line: [[cx, y0], [cx, y1]], range: [y0, y1] }
    }
}

// ---- Found device (building coordinates) for scoring ------------------------------------

/// A device a reader found, in building coordinates, ready for scoring against the truth.
#[derive(Clone, Debug, Default)]
pub struct FoundDevice {
    pub level: String,
    pub kind: String,
    pub type_tag: String,
    /// The room the readers put it in (empty: none; it is then a zone and a finding).
    pub room: String,
    /// Building feet: the A-sheet location when `placement == "located"`, else a point on its
    /// zone near where the E-sheet draws it (display only).
    pub at_ft: [f64; 3],
    /// Mounting height inches AFF.
    pub mounting_in: f64,
    /// `"note"` (printed on the device), `"legend"` (the sheet legend's height for its kind) or
    /// `"rule"` (the mounting-height rules file; ceiling devices: the room's ceiling).
    pub height_source: String,
    /// Whether an A-sheet fixes its plan position (dimensioned on the floor plan, drawn on the
    /// reflected ceiling plan). `false`: a zone / field decision.
    pub arch_positioned: bool,
    /// For zone devices: the allowed range along `zone_wall_ft`'s own axis (x for a wall running
    /// east–west, y for one running north–south), building feet `[min, max]`.
    pub zone_ft: Option<[f64; 2]>,
    /// For zone devices: the wall (or ceiling line) the zone runs along, building feet.
    pub zone_wall_ft: Option<[[f64; 2]; 2]>,
    /// `"located"` (an A-sheet fixes it) or `"rule"` (a zone from the placement rules).
    pub placement: String,
    /// Where it goes is decided in the field (every zone device).
    pub field_decision: bool,
    /// The A-sheet that fixes its position (`placement == "located"`).
    pub position_source: String,
    /// The E-sheet it was read from.
    pub source_sheet: String,
    /// Its circuit as printed beside it (`P1-5`) or on its homerun (`L1/3`).
    pub circuit: Option<String>,
    /// The circuit label of the homerun drawn from it (`L1/3`), if one is.
    pub homerun_circuit: Option<String>,
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
}

/// What [`read_devices`] read.
#[derive(Clone, Debug, Default)]
pub struct DeviceRead {
    pub devices: Vec<FoundDevice>,
    pub panels: Vec<FoundPanel>,
    /// Each E-sheet's legend.
    pub legends: BTreeMap<String, Vec<LegendEntry>>,
    /// What the reader couldn't settle: devices in no room, unreadable sheets, guessed ceilings.
    pub findings: Vec<String>,
}

// ---- Read E-sheets and produce found devices -------------------------------------------

/// A device read off one E-sheet, before placing.
struct Raw {
    sheet: String,
    level: String,
    approx: [f64; 2],
    det: DetectedDevice,
    legend_h: Option<f64>,
}

/// Read electrical devices from the E-sheets (lighting, power and device plans) using the sheet
/// map for alignment and `inputs` for rooms, levels, A-sheet locations and rules.
pub fn read_devices(dxf_data: &[(String, Vec<u8>)], sheet_map: &SheetMap, inputs: &DeviceInputs) -> DeviceRead {
    let mut out = DeviceRead { findings: inputs.findings.clone(), ..DeviceRead::default() };
    let mut raws: Vec<Raw> = Vec::new();
    for (name, bytes) in dxf_data {
        let stem = sheet_stem(name);
        let Some(rs) = sheet_map.sheets.iter().find(|s| s.number == stem) else { continue };
        // Plans only: a sheet with a level (schedules and the cover have none).
        if rs.discipline != "E" || rs.title.to_ascii_uppercase().contains("SCHEDULE") {
            continue;
        }
        let Some(level) = rs.level.clone().filter(|l| !l.is_empty()) else { continue };
        let Some(alignment) = &rs.alignment else {
            out.findings.push(format!("{stem}: not aligned to the building grid; its devices were not read"));
            continue;
        };
        let d = match cadaddy_io::read_dxf(bytes) {
            Ok(d) => d,
            Err(e) => {
                out.findings.push(format!("{stem}: could not read the drawing ({e})"));
                continue;
            }
        };
        let k = if rs.scale > 0.0 { 12.0 / rs.scale } else { 0.0 };
        let (legend, skip) = read_legend(&d, k);
        for det in detect_device_symbols(&d, &skip) {
            let approx = building_from_model(alignment, det.at_in);
            let legend_h = legend.iter().find(|e| e.kind == det.kind).and_then(|e| e.mounting_in);
            raws.push(Raw { sheet: stem.to_string(), level: level.clone(), approx, det, legend_h });
        }
        for rp in &detect_panels(&d) {
            let b = building_from_model(alignment, rp.at_in);
            out.panels.push(FoundPanel { level: level.clone(), tag: rp.tag.clone(), kind: rp.kind.clone(), at_ft: b });
        }
        out.legends.insert(stem.to_string(), legend);
    }

    // Rooms, then A-sheet locations: nearest pairs first, each location used once.
    let rooms: Vec<Option<&InputRoom>> = raws.iter().map(|r| inputs.room(&r.level, r.approx)).collect();
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, r) in raws.iter().enumerate() {
        for (j, l) in inputs.locators.iter().enumerate() {
            if l.level != r.level || l.kind != r.det.kind {
                continue;
            }
            let dist = (l.at_ft[0] - r.approx[0]).hypot(l.at_ft[1] - r.approx[1]);
            let in_room = rooms.get(i).copied().flatten().is_none_or(|rm| {
                l.at_ft[0] >= rm.min_ft[0] - 1.0
                    && l.at_ft[0] <= rm.max_ft[0] + 1.0
                    && l.at_ft[1] >= rm.min_ft[1] - 1.0
                    && l.at_ft[1] <= rm.max_ft[1] + 1.0
            });
            if dist <= LOCATOR_REACH_FT && in_room {
                pairs.push((dist, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut located: Vec<Option<usize>> = vec![None; raws.len()];
    let mut taken = vec![false; inputs.locators.len()];
    for (_, i, j) in pairs {
        if located.get(i).is_some_and(Option::is_none) && taken.get(j).is_some_and(|t| !t) {
            if let Some(x) = located.get_mut(i) {
                *x = Some(j);
            }
            if let Some(x) = taken.get_mut(j) {
                *x = true;
            }
        }
    }

    let mut no_level: BTreeSet<String> = BTreeSet::new();
    let mut default_ceiling = 0usize;
    for (i, r) in raws.iter().enumerate() {
        let room = rooms.get(i).copied().flatten();
        let kind = r.det.kind.as_str();
        let rules = &inputs.rules;
        // Height: a note on the device, else the legend, else the rules.
        let ceiling_note = r.det.height_source == "note" && r.det.mounting_in == 0.0 && r.det.mounting_note.as_deref() == Some("CLG");
        let (on_ceiling, wall_in, height_source) = if r.det.height_source == "note" {
            (ceiling_note, r.det.mounting_in, "note")
        } else if let Some(h) = r.legend_h {
            (false, h, "legend")
        } else {
            (rules.is_ceiling(kind), rules.height_in(kind), "rule")
        };
        let mounting_in = if on_ceiling {
            match room.and_then(|rm| rm.ceiling_ft) {
                Some(c) => c * 12.0,
                None => {
                    default_ceiling += 1;
                    rules.default_ceiling_ft * 12.0
                }
            }
        } else {
            wall_in
        };
        let elev = inputs.elevation(&r.level).unwrap_or_else(|| {
            no_level.insert(r.level.clone());
            0.0
        });
        let z = elev + mounting_in / 12.0;

        let loc = located.get(i).copied().flatten().and_then(|j| inputs.locators.get(j));
        let wall_mounted = !on_ceiling && mounting_in > 0.0;
        let (at, zone, arch) = match loc {
            Some(l) => (l.at_ft, None, true),
            None => {
                let zn = zone_for(room, wall_mounted, r.approx);
                (zn.at_ft, Some(zn), false)
            }
        };
        if room.is_none() {
            out.findings
                .push(format!("{}: {} at ({:.1}, {:.1}) is in no room the readers found; placed as a zone", r.sheet, kind, r.approx[0], r.approx[1]));
        }
        let circuit = r.det.notes.iter().find(|n| n.contains('-') && is_subscript_or_circuit(n)).cloned().or_else(|| r.det.circuit_label.clone());
        out.devices.push(FoundDevice {
            level: r.level.clone(),
            kind: kind.to_string(),
            type_tag: r.det.type_tag.clone(),
            room: room.map(|rm| rm.number.clone()).unwrap_or_default(),
            at_ft: [at[0], at[1], z],
            mounting_in,
            height_source: height_source.to_string(),
            arch_positioned: arch,
            zone_ft: zone.as_ref().map(|z| z.range),
            zone_wall_ft: zone.as_ref().map(|z| z.line),
            placement: if arch { "located" } else { "rule" }.to_string(),
            field_decision: !arch,
            position_source: loc.map(|l| l.sheet.clone()).unwrap_or_default(),
            source_sheet: r.sheet.clone(),
            circuit,
            homerun_circuit: r.det.circuit_label.clone(),
            properties: BTreeMap::new(),
            drawn_ft: r.approx,
            notes: r.det.notes.clone(),
        });
    }
    if default_ceiling > 0 {
        out.findings.push(format!(
            "{default_ceiling} ceiling devices are in rooms with no ceiling height on the reflected ceiling plans: placed at the rules' {} ft",
            inputs.rules.default_ceiling_ft
        ));
    }
    for l in no_level {
        out.findings.push(format!("level {l}: no elevation in the level list; its devices are above elevation 0"));
    }
    out
}

/// Convert model inches to building feet using a sheet alignment.
fn building_from_model(alignment: &Alignment, m: [f64; 2]) -> [f64; 2] {
    crate::sheet_reader::building_from_model(alignment, m)
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
    /// True positives: a found device of the same kind drawn where a truth device is drawn.
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
    /// Of matched truth devices the A-sheets fix: mean position error, feet.
    pub position_mae_ft: f64,
    /// Of matched truth devices the A-sheets fix: how many the reader located (not a zone).
    pub arch_located: usize,
    pub arch_total: usize,
    /// Of matched zone truth devices: the share whose zone contains the truth along the zone's
    /// own wall.
    pub zone_containment: f64,
    /// Kind confusions: a truth device not matched because the device found where it is drawn
    /// is of another kind (counted once per pair).
    pub kind_confusions: usize,
    /// TP / (TP + kind confusions).
    pub kind_accuracy: f64,
    /// Of matched devices whose truth type tag is printed beside the symbol (light fixtures
    /// not planted untagged): the share read exactly.
    pub type_tag_accuracy: f64,
    pub type_tag_total: usize,
    /// Of matched devices with a circuit printed beside them, and of matched devices the reader
    /// hung a homerun's circuit label on: the share whose circuit is the truth's.
    pub circuit_accuracy: f64,
    pub circuit_total: usize,
    /// Found devices by height source (`note` / `legend` / `rule`).
    pub height_sources: BTreeMap<String, usize>,
}

fn share(ok: usize, total: usize) -> f64 {
    if total > 0 { ok as f64 / total as f64 } else { 0.0 }
}

/// Score found devices against the truth.
///
/// Matching (detection): a found device matches a truth device when they are on the same level,
/// of the same kind, and drawn within `tol_ft` of each other on the E-sheet (nearest pairs
/// first). A truth device left unmatched next to an unmatched found device of another kind is a
/// kind confusion.
///
/// Location: for truth devices the A-sheets fix (`arch_positioned`), the plan position error;
/// for zone devices, whether the found zone contains the truth position along the zone's own
/// wall (x for a wall running east–west, y for one running north–south).
///
/// Height: the truth's mounting height; a ceiling device (`CLG`) is at its ceiling, its height
/// above its level's floor.
pub fn score_devices(truth: &Truth, found: &[FoundDevice], tol_ft: f64) -> DeviceScore {
    let truth_devs = &truth.elec_devices;
    let dist = |f: &FoundDevice, t: &crate::truth::ElecDevice| (f.drawn_ft[0] - t.drawn_ft[0]).hypot(f.drawn_ft[1] - t.drawn_ft[1]);
    let pairs = |same_kind: bool, mt: &[bool], mf: &[bool]| {
        let mut p: Vec<(f64, usize, usize)> = Vec::new();
        for (ti, td) in truth_devs.iter().enumerate() {
            if mt.get(ti).copied().unwrap_or(true) {
                continue;
            }
            for (fi, fd) in found.iter().enumerate() {
                if mf.get(fi).copied().unwrap_or(true) || fd.level != td.level || (fd.kind == td.kind) != same_kind {
                    continue;
                }
                let d = dist(fd, td);
                if d < tol_ft {
                    p.push((d, ti, fi));
                }
            }
        }
        p.sort_by(|a, b| a.0.total_cmp(&b.0));
        p
    };
    let mut matched_truth = vec![false; truth_devs.len()];
    let mut matched_found = vec![false; found.len()];
    let mut matches: Vec<(usize, usize)> = Vec::new();
    for (_, ti, fi) in pairs(true, &matched_truth, &matched_found) {
        if !matched_truth.get(ti).copied().unwrap_or(true) && !matched_found.get(fi).copied().unwrap_or(true) {
            matched_truth[ti] = true;
            matched_found[fi] = true;
            matches.push((ti, fi));
        }
    }
    let (mut ct, mut cf) = (matched_truth.clone(), matched_found.clone());
    let mut kind_confusions = 0;
    for (_, ti, fi) in pairs(false, &ct.clone(), &cf.clone()) {
        if !ct.get(ti).copied().unwrap_or(true) && !cf.get(fi).copied().unwrap_or(true) {
            ct[ti] = true;
            cf[fi] = true;
            kind_confusions += 1;
        }
    }

    let mut s = DeviceScore { kind_confusions, ..DeviceScore::default() };
    let (mut h_sum, mut pos_sum, mut zone_ok, mut zone_total, mut tag_ok, mut ckt_ok) = (0.0, 0.0, 0, 0, 0, 0);
    for &(ti, fi) in &matches {
        let (Some(td), Some(fd)) = (truth_devs.get(ti), found.get(fi)) else { continue };
        let elev = truth.level(&td.level).map_or(0.0, |l| l.elevation_ft);
        let want_in = if td.mounting_note.as_deref() == Some("CLG") { (td.at_ft[2] - elev) * 12.0 } else { td.mounting_in };
        h_sum += (fd.mounting_in - want_in).abs();
        if td.arch_positioned {
            s.arch_total += 1;
            s.arch_located += usize::from(fd.arch_positioned);
            pos_sum += (fd.at_ft[0] - td.at_ft[0]).hypot(fd.at_ft[1] - td.at_ft[1]);
        } else {
            zone_total += 1;
            if let (Some(z), Some(w)) = (fd.zone_ft, fd.zone_wall_ft) {
                let along_x = (w[1][0] - w[0][0]).abs() >= (w[1][1] - w[0][1]).abs();
                let v = if along_x { td.at_ft[0] } else { td.at_ft[1] };
                zone_ok += usize::from(v >= z[0].min(z[1]) - 1e-6 && v <= z[0].max(z[1]) + 1e-6);
            }
        }
        if td.kind == "light_fixture" && !truth.elec_spec.untagged.contains(&td.id) {
            s.type_tag_total += 1;
            tag_ok += usize::from(fd.type_tag == td.tag);
        }
        if let Some(n) = truth.elec_spec.note(&td.id)
            && !n.circuit_ref.is_empty()
        {
            s.circuit_total += 1;
            ckt_ok += usize::from(fd.circuit.as_deref() == Some(n.circuit_ref.as_str()));
        }
        if let Some(label) = &fd.homerun_circuit {
            // The truth names a circuit by id (its label in the circuit list) or by its label.
            let want = truth.elec_circuits.iter().find(|c| c.id == td.circuit).map_or(td.circuit.as_str(), |c| c.label.as_str());
            s.circuit_total += 1;
            ckt_ok += usize::from(label == want);
        }
    }
    for f in found {
        *s.height_sources.entry(f.height_source.clone()).or_default() += 1;
    }
    let tp = matches.len();
    s.true_positives = tp;
    s.false_positives = found.len() - tp;
    s.false_negatives = truth_devs.len() - tp;
    s.precision = share(tp, found.len());
    s.recall = share(tp, truth_devs.len());
    s.height_mae_in = if tp > 0 { h_sum / tp as f64 } else { 0.0 };
    s.position_mae_ft = if s.arch_total > 0 { pos_sum / s.arch_total as f64 } else { 0.0 };
    s.zone_containment = share(zone_ok, zone_total);
    s.kind_accuracy = share(tp, tp + kind_confusions);
    s.type_tag_accuracy = share(tag_ok, s.type_tag_total);
    s.circuit_accuracy = share(ckt_ok, s.circuit_total);
    s
}

/// Create perfect found devices from the truth (for testing the scorer baseline).
pub fn perfect_devices(truth: &Truth) -> Vec<FoundDevice> {
    truth
        .elec_devices
        .iter()
        .map(|d| {
            let elev = truth.level(&d.level).map_or(0.0, |l| l.elevation_ft);
            let note = truth.elec_spec.note(&d.id);
            FoundDevice {
                level: d.level.clone(),
                kind: d.kind.clone(),
                type_tag: d.tag.clone(),
                room: d.room.clone(),
                at_ft: d.at_ft,
                mounting_in: if d.mounting_note.as_deref() == Some("CLG") { (d.at_ft[2] - elev) * 12.0 } else { d.mounting_in },
                height_source: if d.mounting_note.is_some() { "note" } else { "rule" }.into(),
                arch_positioned: d.arch_positioned,
                // A zone along an east–west line through the truth.
                zone_ft: (!d.arch_positioned).then(|| [d.at_ft[0] - 3.0, d.at_ft[0] + 3.0]),
                zone_wall_ft: (!d.arch_positioned).then(|| [[d.at_ft[0] - 3.0, d.at_ft[1]], [d.at_ft[0] + 3.0, d.at_ft[1]]]),
                placement: if d.arch_positioned { "located" } else { "rule" }.into(),
                field_decision: !d.arch_positioned,
                position_source: String::new(),
                source_sheet: d.sheet.clone(),
                circuit: note.map(|n| n.circuit_ref.clone()).filter(|c| !c.is_empty()),
                homerun_circuit: None,
                properties: BTreeMap::new(),
                drawn_ft: d.drawn_ft,
                notes: note
                    .map(|n| n.subscripts.iter().cloned().chain((!n.circuit_ref.is_empty()).then(|| n.circuit_ref.clone())).collect())
                    .unwrap_or_default(),
            }
        })
        .collect()
}
