//! As-built capture: field changes back into the model.
//!
//! Records what was actually built versus what the design drawings show: moved devices, rerouted
//! conduit, changed types, field-verified stud and joist positions.  Each change carries who,
//! when, why and a status (field decision, RFI answer, change order, pending).
//!
//! Markups from PDF sheets (revision clouds, arrows, callout text) are mapped to model elements
//! by their position on a sheet (through the sheet's coordinate transform).  The result is an
//! as-built truth that can produce as-built sheets and an as-built IFC, and compare design vs
//! as-built.

use crate::truth::{self, Segment, Sheet, Truth};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum number of changes in a single as-built record (hostile input cap).
const MAX_CHANGES: usize = 10_000;
/// Maximum length of a free-text field (reason, notes).
const MAX_TEXT: usize = 2_000;
/// Maximum number of markups to process from one PDF page.
const MAX_MARKUPS_PER_PAGE: usize = 1_000;

// ---------------------------------------------------------------------------
// Core data model
// ---------------------------------------------------------------------------

/// The status of a field change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    /// Not yet reviewed or approved.
    #[default]
    Pending,
    /// Made by the crew on site without prior approval.
    FieldDecision,
    /// The answer to an RFI.
    RfiAnswer,
    /// Covered by a formal change order.
    ChangeOrder,
    /// Rejected — the as-built reverts to the design.
    Rejected,
}

/// What kind of change was made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// An element was moved to a new position.
    Moved {
        /// Design position, building feet.
        from_ft: [f64; 3],
        /// As-built position, building feet.
        to_ft: [f64; 3],
    },
    /// A conduit or pipe run was rerouted (new path).
    Rerouted {
        /// Design path endpoints.
        design_path_ft: [[f64; 3]; 2],
        /// As-built path endpoints.
        asbuilt_path_ft: [[f64; 3]; 2],
    },
    /// An element's type or spec was changed (e.g. fixture type, conduit type).
    TypeChanged {
        design_type: String,
        asbuilt_type: String,
    },
    /// An element's size was changed.
    SizeChanged {
        design_size: String,
        asbuilt_size: String,
    },
    /// A new element was added in the field (not on design drawings).
    Added,
    /// A design element was deleted (not built).
    Deleted,
    /// A field-verified position (stud, joist, device confirmed at location).
    FieldVerified {
        /// Verified position, building feet.
        at_ft: [f64; 3],
    },
    /// A height change (run raised or lowered).
    HeightChanged {
        design_ft: f64,
        asbuilt_ft: f64,
    },
    /// General note or observation without a specific geometric change.
    Note,
}

/// One field change to a model element.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AsBuiltChange {
    pub id: String,
    /// The truth element this change applies to (segment id, device id, equipment id, etc.).
    /// Empty for `Added` elements that have no design counterpart.
    pub element_id: String,
    /// What kind of element: `segment`, `elec_device`, `elec_conduit`, `equipment`,
    /// `terminal`, `fixture`, `wall`, `node`.
    pub element_kind: String,
    pub kind: ChangeKind,
    pub status: ChangeStatus,
    /// Who made or recorded the change.
    pub author: String,
    /// ISO 8601 date string.
    pub date: String,
    /// Why (RFI number, change order number, field reason).
    pub reason: String,
    /// Free-form notes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// The level this change is on.
    pub level: String,
    /// Position on the plan for display, building feet.
    pub at_ft: [f64; 2],
    /// The markup id that originated this change (when imported from a PDF).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markup_id: Option<String>,
}

/// A markup from a PDF sheet mapped to the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MappedMarkup {
    pub id: String,
    /// The source markup kind: `cloud`, `arrow`, `callout`, `note`, `line`, etc.
    pub kind: String,
    /// The text content of the markup.
    pub text: String,
    /// Who created the markup.
    pub author: String,
    /// When.
    pub date: String,
    /// The sheet this markup was on.
    pub sheet: String,
    /// Position on the sheet, paper inches.
    pub paper_pt: [f64; 2],
    /// Position in building coordinates, feet.
    pub building_ft: [f64; 2],
    /// The level (from the sheet).
    pub level: String,
    /// Model element ids near this markup (candidates for what it refers to).
    pub near_elements: Vec<String>,
    /// The bounding rect in building feet.
    pub bounds_ft: [[f64; 2]; 2],
}

/// The full as-built record for a project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AsBuiltRecord {
    pub schema: String,
    pub project: String,
    /// All field changes.
    pub changes: Vec<AsBuiltChange>,
    /// Markups imported from PDF sheets, mapped to building coordinates.
    pub markups: Vec<MappedMarkup>,
}

impl AsBuiltRecord {
    pub fn new(project: &str) -> Self {
        Self {
            schema: "cadaddy-mepfix-asbuilt/1".into(),
            project: project.into(),
            changes: Vec::new(),
            markups: Vec::new(),
        }
    }

    /// Add a change, respecting the cap.
    pub fn add_change(&mut self, change: AsBuiltChange) -> bool {
        if self.changes.len() >= MAX_CHANGES {
            return false;
        }
        self.changes.push(change);
        true
    }

    /// Changes for a specific element.
    pub fn changes_for(&self, element_id: &str) -> Vec<&AsBuiltChange> {
        self.changes.iter().filter(|c| c.element_id == element_id).collect()
    }

    /// All changes with a given status.
    pub fn by_status(&self, status: ChangeStatus) -> Vec<&AsBuiltChange> {
        self.changes.iter().filter(|c| c.status == status).collect()
    }

    /// Changes on a given level.
    pub fn on_level(&self, level: &str) -> Vec<&AsBuiltChange> {
        self.changes.iter().filter(|c| c.level == level).collect()
    }
}

// ---------------------------------------------------------------------------
// Markup import: PDF markups → model coordinates
// ---------------------------------------------------------------------------

/// A synthetic markup for testing (mirrors the shape of `cadaddy_doc::markup::Markup`).
#[derive(Clone, Debug)]
pub struct SyntheticMarkup {
    pub kind: String,
    pub text: String,
    pub author: String,
    pub date: String,
    /// Centre point in paper inches on the sheet.
    pub paper_in: [f64; 2],
    /// Bounding rect in paper inches: [min, max].
    pub paper_rect_in: [[f64; 2]; 2],
}

/// Map a paper-space point (inches) to building coordinates (feet) through a sheet's transform.
pub fn paper_to_building(sh: &Sheet, paper_in: [f64; 2]) -> [f64; 2] {
    // Paper → model: model = (paper / paper_from_model.scale) + paper_from_model.origin_in
    let pm = &sh.paper_from_model;
    let scale = if pm.scale.abs() < 1e-12 { 1.0 } else { pm.scale };
    let model_in = [
        paper_in[0] / scale + pm.origin_in[0],
        paper_in[1] / scale + pm.origin_in[1],
    ];
    // Model (inches) → building (feet): undo the placement
    let mb = &sh.model_from_building;
    let rot_rad = -mb.rotation_deg.to_radians();
    let cos_r = rot_rad.cos();
    let sin_r = rot_rad.sin();
    // model_in = rotate(building_ft * 12.0) + offset_in
    // building_ft * 12.0 = un-rotate(model_in - offset_in)
    let dx = model_in[0] - mb.offset_in[0];
    let dy = model_in[1] - mb.offset_in[1];
    let bldg_in_x = dx * cos_r - dy * sin_r;
    let bldg_in_y = dx * sin_r + dy * cos_r;
    [bldg_in_x / 12.0, bldg_in_y / 12.0]
}

/// Map a building-coordinate point (feet) to paper inches on a sheet.
pub fn building_to_paper(sh: &Sheet, bldg_ft: [f64; 2]) -> [f64; 2] {
    let mb = &sh.model_from_building;
    let rot_rad = mb.rotation_deg.to_radians();
    let cos_r = rot_rad.cos();
    let sin_r = rot_rad.sin();
    let bx = bldg_ft[0] * 12.0;
    let by = bldg_ft[1] * 12.0;
    let model_in = [
        bx * cos_r - by * sin_r + mb.offset_in[0],
        bx * sin_r + by * cos_r + mb.offset_in[1],
    ];
    let pm = &sh.paper_from_model;
    let scale = if pm.scale.abs() < 1e-12 { 1.0 } else { pm.scale };
    [
        (model_in[0] - pm.origin_in[0]) * scale,
        (model_in[1] - pm.origin_in[1]) * scale,
    ]
}

/// Import synthetic markups from a sheet and map them to building coordinates and nearby elements.
pub fn import_markups(
    truth: &Truth,
    sheet: &Sheet,
    markups: &[SyntheticMarkup],
) -> Vec<MappedMarkup> {
    let level = sheet.level.clone().unwrap_or_default();
    let mut result = Vec::new();
    for (i, m) in markups.iter().take(MAX_MARKUPS_PER_PAGE).enumerate() {
        let bldg = paper_to_building(sheet, m.paper_in);
        let bmin = paper_to_building(sheet, m.paper_rect_in[0]);
        let bmax = paper_to_building(sheet, m.paper_rect_in[1]);
        let near = find_near_elements(truth, &level, bldg, 3.0);
        result.push(MappedMarkup {
            id: format!("MK-{}-{}", sheet.number, i + 1),
            kind: m.kind.clone(),
            text: truncate(&m.text, MAX_TEXT),
            author: truncate(&m.author, MAX_TEXT),
            date: m.date.clone(),
            sheet: sheet.number.clone(),
            paper_pt: m.paper_in,
            building_ft: bldg,
            level: level.clone(),
            near_elements: near,
            bounds_ft: [bmin, bmax],
        });
    }
    result
}

/// Find truth element ids within `radius_ft` of a point on a given level.
fn find_near_elements(truth: &Truth, level: &str, at_ft: [f64; 2], radius_ft: f64) -> Vec<String> {
    let r2 = radius_ft * radius_ft;
    let mut ids = Vec::new();
    // Segments on this level
    for s in &truth.segments {
        if s.level != level {
            continue;
        }
        let mid = [
            (s.path_ft[0][0] + s.path_ft[1][0]) * 0.5,
            (s.path_ft[0][1] + s.path_ft[1][1]) * 0.5,
        ];
        if dist2(at_ft, mid) < r2 || point_seg_dist2(at_ft, s) < r2 {
            ids.push(s.id.clone());
        }
    }
    // Electrical devices
    for d in &truth.elec_devices {
        if d.level != level {
            continue;
        }
        if dist2(at_ft, [d.at_ft[0], d.at_ft[1]]) < r2 {
            ids.push(d.id.clone());
        }
    }
    // Equipment
    for e in &truth.equipment {
        if e.level != level {
            continue;
        }
        let cx = (e.min_ft[0] + e.max_ft[0]) * 0.5;
        let cy = (e.min_ft[1] + e.max_ft[1]) * 0.5;
        if dist2(at_ft, [cx, cy]) < r2 {
            ids.push(e.id.clone());
        }
    }
    // Electrical conduits
    for c in &truth.elec_conduits {
        if c.level != level {
            continue;
        }
        let mid = [
            (c.from_ft[0] + c.to_ft[0]) * 0.5,
            (c.from_ft[1] + c.to_ft[1]) * 0.5,
        ];
        if dist2(at_ft, mid) < r2 {
            ids.push(c.id.clone());
        }
    }
    // Terminals
    for t in &truth.terminals {
        if t.level != level {
            continue;
        }
        if dist2(at_ft, [t.at_ft[0], t.at_ft[1]]) < r2 {
            ids.push(t.id.clone());
        }
    }
    // Fixtures
    for f in &truth.fixtures {
        if f.level != level {
            continue;
        }
        if dist2(at_ft, [f.at_ft[0], f.at_ft[1]]) < r2 {
            ids.push(f.id.clone());
        }
    }
    ids
}

fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1])
}

/// Squared distance from a 2D point to a segment's centreline (projected to plan).
fn point_seg_dist2(p: [f64; 2], seg: &Segment) -> f64 {
    let a = [seg.path_ft[0][0], seg.path_ft[0][1]];
    let b = [seg.path_ft[1][0], seg.path_ft[1][1]];
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-12 {
        return dist2(p, a);
    }
    let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2;
    let t = t.clamp(0.0, 1.0);
    let proj = [a[0] + t * dx, a[1] + t * dy];
    dist2(p, proj)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s[..max].to_string()
    }
}

// ---------------------------------------------------------------------------
// Markup → change conversion
// ---------------------------------------------------------------------------

/// Heuristics: interpret a markup's text to create an `AsBuiltChange`.
///
/// Cloud markups → area change (moved/rerouted for the nearest element).
/// Arrow markups → relocation.
/// Callout/note markups → notes or type changes when text contains keywords.
pub fn markup_to_change(
    markup: &MappedMarkup,
    truth: &Truth,
    change_id: &str,
) -> Option<AsBuiltChange> {
    if markup.near_elements.is_empty() {
        return None;
    }
    let element_id = markup.near_elements.first()?.clone();
    let text_lower = markup.text.to_ascii_lowercase();

    let (element_kind, kind) = match markup.kind.as_str() {
        "cloud" => {
            // A cloud around an area → the nearest element was changed.
            let ek = element_kind_of(truth, &element_id);
            let ck = if text_lower.contains("move") || text_lower.contains("relocat") {
                ChangeKind::Moved {
                    from_ft: element_position(truth, &element_id),
                    to_ft: [markup.building_ft[0], markup.building_ft[1], 0.0],
                }
            } else if text_lower.contains("delet") || text_lower.contains("remov") {
                ChangeKind::Deleted
            } else if text_lower.contains("add") || text_lower.contains("new") {
                ChangeKind::Added
            } else if text_lower.contains("type") || text_lower.contains("change to") {
                ChangeKind::TypeChanged {
                    design_type: String::new(),
                    asbuilt_type: markup.text.clone(),
                }
            } else {
                ChangeKind::Note
            };
            (ek, ck)
        }
        "arrow" => {
            let ek = element_kind_of(truth, &element_id);
            let pos = element_position(truth, &element_id);
            (
                ek,
                ChangeKind::Moved {
                    from_ft: pos,
                    to_ft: [markup.building_ft[0], markup.building_ft[1], pos[2]],
                },
            )
        }
        "callout" | "note" => {
            let ek = element_kind_of(truth, &element_id);
            let ck = if text_lower.contains("verif") || text_lower.contains("confirmed") {
                ChangeKind::FieldVerified {
                    at_ft: element_position(truth, &element_id),
                }
            } else if text_lower.contains("reroute") || text_lower.contains("rerouted") {
                ChangeKind::Rerouted {
                    design_path_ft: [[0.0; 3]; 2],
                    asbuilt_path_ft: [[0.0; 3]; 2],
                }
            } else if text_lower.contains("raise") || text_lower.contains("lower") {
                ChangeKind::HeightChanged {
                    design_ft: 0.0,
                    asbuilt_ft: 0.0,
                }
            } else {
                ChangeKind::Note
            };
            (ek, ck)
        }
        _ => {
            let ek = element_kind_of(truth, &element_id);
            (ek, ChangeKind::Note)
        }
    };

    Some(AsBuiltChange {
        id: change_id.into(),
        element_id,
        element_kind,
        kind,
        status: ChangeStatus::Pending,
        author: markup.author.clone(),
        date: markup.date.clone(),
        reason: markup.text.clone(),
        notes: String::new(),
        level: markup.level.clone(),
        at_ft: markup.building_ft,
        markup_id: Some(markup.id.clone()),
    })
}

/// Determine what kind of truth element an id belongs to.
fn element_kind_of(truth: &Truth, id: &str) -> String {
    if truth.segments.iter().any(|s| s.id == id) {
        "segment".into()
    } else if truth.elec_devices.iter().any(|d| d.id == id) {
        "elec_device".into()
    } else if truth.equipment.iter().any(|e| e.id == id) {
        "equipment".into()
    } else if truth.elec_conduits.iter().any(|c| c.id == id) {
        "elec_conduit".into()
    } else if truth.terminals.iter().any(|t| t.id == id) {
        "terminal".into()
    } else if truth.fixtures.iter().any(|f| f.id == id) {
        "fixture".into()
    } else if truth.nodes.iter().any(|n| n.id == id) {
        "node".into()
    } else {
        "unknown".into()
    }
}

/// Get the 3D position of a truth element by id.
fn element_position(truth: &Truth, id: &str) -> [f64; 3] {
    if let Some(s) = truth.segments.iter().find(|s| s.id == id) {
        let mid = [
            (s.path_ft[0][0] + s.path_ft[1][0]) * 0.5,
            (s.path_ft[0][1] + s.path_ft[1][1]) * 0.5,
            (s.path_ft[0][2] + s.path_ft[1][2]) * 0.5,
        ];
        return mid;
    }
    if let Some(d) = truth.elec_devices.iter().find(|d| d.id == id) {
        return d.at_ft;
    }
    if let Some(e) = truth.equipment.iter().find(|e| e.id == id) {
        return [
            (e.min_ft[0] + e.max_ft[0]) * 0.5,
            (e.min_ft[1] + e.max_ft[1]) * 0.5,
            (e.min_ft[2] + e.max_ft[2]) * 0.5,
        ];
    }
    if let Some(c) = truth.elec_conduits.iter().find(|c| c.id == id) {
        return [(c.from_ft[0] + c.to_ft[0]) * 0.5, (c.from_ft[1] + c.to_ft[1]) * 0.5, 0.0];
    }
    if let Some(t) = truth.terminals.iter().find(|t| t.id == id) {
        return t.at_ft;
    }
    if let Some(f) = truth.fixtures.iter().find(|f| f.id == id) {
        return f.at_ft;
    }
    if let Some(n) = truth.nodes.iter().find(|n| n.id == id) {
        return n.at_ft;
    }
    [0.0; 3]
}

// ---------------------------------------------------------------------------
// Design vs as-built comparison
// ---------------------------------------------------------------------------

/// A single difference between design and as-built.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Deviation {
    pub element_id: String,
    pub element_kind: String,
    pub description: String,
    pub level: String,
    pub at_ft: [f64; 2],
    /// Distance of the deviation, feet (for moves) or a severity score.
    pub magnitude: f64,
    /// The change id that caused it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
}

/// Compare the design truth with the as-built record and produce a list of deviations.
pub fn compare_design_vs_asbuilt(_truth: &Truth, record: &AsBuiltRecord) -> Vec<Deviation> {
    let mut devs = Vec::new();
    for change in &record.changes {
        if change.status == ChangeStatus::Rejected {
            continue;
        }
        let dev = match &change.kind {
            ChangeKind::Moved { from_ft, to_ft } => {
                let d = ((to_ft[0] - from_ft[0]).powi(2)
                    + (to_ft[1] - from_ft[1]).powi(2)
                    + (to_ft[2] - from_ft[2]).powi(2))
                .sqrt();
                Deviation {
                    element_id: change.element_id.clone(),
                    element_kind: change.element_kind.clone(),
                    description: format!(
                        "Moved {:.1}' from design position ({})",
                        d,
                        change.reason
                    ),
                    level: change.level.clone(),
                    at_ft: change.at_ft,
                    magnitude: d,
                    change_id: Some(change.id.clone()),
                }
            }
            ChangeKind::Rerouted { design_path_ft, asbuilt_path_ft } => {
                let d = ((asbuilt_path_ft[0][0] - design_path_ft[0][0]).powi(2)
                    + (asbuilt_path_ft[0][1] - design_path_ft[0][1]).powi(2))
                .sqrt();
                Deviation {
                    element_id: change.element_id.clone(),
                    element_kind: change.element_kind.clone(),
                    description: format!("Rerouted ({})", change.reason),
                    level: change.level.clone(),
                    at_ft: change.at_ft,
                    magnitude: d,
                    change_id: Some(change.id.clone()),
                }
            }
            ChangeKind::TypeChanged { design_type, asbuilt_type } => Deviation {
                element_id: change.element_id.clone(),
                element_kind: change.element_kind.clone(),
                description: format!(
                    "Type changed: {} -> {} ({})",
                    design_type, asbuilt_type, change.reason
                ),
                level: change.level.clone(),
                at_ft: change.at_ft,
                magnitude: 1.0,
                change_id: Some(change.id.clone()),
            },
            ChangeKind::SizeChanged { design_size, asbuilt_size } => Deviation {
                element_id: change.element_id.clone(),
                element_kind: change.element_kind.clone(),
                description: format!(
                    "Size changed: {} -> {} ({})",
                    design_size, asbuilt_size, change.reason
                ),
                level: change.level.clone(),
                at_ft: change.at_ft,
                magnitude: 1.0,
                change_id: Some(change.id.clone()),
            },
            ChangeKind::Added => Deviation {
                element_id: change.element_id.clone(),
                element_kind: change.element_kind.clone(),
                description: format!("Added in field ({})", change.reason),
                level: change.level.clone(),
                at_ft: change.at_ft,
                magnitude: 1.0,
                change_id: Some(change.id.clone()),
            },
            ChangeKind::Deleted => Deviation {
                element_id: change.element_id.clone(),
                element_kind: change.element_kind.clone(),
                description: format!("Deleted / not built ({})", change.reason),
                level: change.level.clone(),
                at_ft: change.at_ft,
                magnitude: 1.0,
                change_id: Some(change.id.clone()),
            },
            ChangeKind::FieldVerified { at_ft } => Deviation {
                element_id: change.element_id.clone(),
                element_kind: change.element_kind.clone(),
                description: format!("Field verified at ({:.2}', {:.2}', {:.2}')", at_ft[0], at_ft[1], at_ft[2]),
                level: change.level.clone(),
                at_ft: change.at_ft,
                magnitude: 0.0,
                change_id: Some(change.id.clone()),
            },
            ChangeKind::HeightChanged { design_ft, asbuilt_ft } => Deviation {
                element_id: change.element_id.clone(),
                element_kind: change.element_kind.clone(),
                description: format!(
                    "Height changed: {} -> {} ({})",
                    truth::feet_inches(*design_ft),
                    truth::feet_inches(*asbuilt_ft),
                    change.reason
                ),
                level: change.level.clone(),
                at_ft: change.at_ft,
                magnitude: (asbuilt_ft - design_ft).abs(),
                change_id: Some(change.id.clone()),
            },
            ChangeKind::Note => Deviation {
                element_id: change.element_id.clone(),
                element_kind: change.element_kind.clone(),
                description: format!("Note: {}", change.reason),
                level: change.level.clone(),
                at_ft: change.at_ft,
                magnitude: 0.0,
                change_id: Some(change.id.clone()),
            },
        };
        devs.push(dev);
    }
    devs
}

// ---------------------------------------------------------------------------
// As-built truth: apply changes to produce the as-built state
// ---------------------------------------------------------------------------

/// Apply all non-rejected changes to produce an as-built copy of the truth.
/// Elements with `Moved` changes get new positions; `Deleted` elements are removed;
/// `Added` elements are noted but not created (they have no truth definition).
pub fn asbuilt_truth(design: &Truth, record: &AsBuiltRecord) -> Truth {
    let mut t = design.clone();
    for change in &record.changes {
        if change.status == ChangeStatus::Rejected {
            continue;
        }
        match &change.kind {
            ChangeKind::Moved { to_ft, .. } => {
                apply_move(&mut t, &change.element_id, &change.element_kind, *to_ft);
            }
            ChangeKind::Rerouted { asbuilt_path_ft, .. } => {
                if let Some(seg) = t.segments.iter_mut().find(|s| s.id == change.element_id) {
                    seg.path_ft = *asbuilt_path_ft;
                }
            }
            ChangeKind::TypeChanged { asbuilt_type, .. } => {
                apply_type_change(&mut t, &change.element_id, &change.element_kind, asbuilt_type);
            }
            ChangeKind::SizeChanged { asbuilt_size, .. } => {
                apply_size_change(&mut t, &change.element_id, asbuilt_size);
            }
            ChangeKind::Deleted => {
                apply_delete(&mut t, &change.element_id, &change.element_kind);
            }
            ChangeKind::HeightChanged { asbuilt_ft, .. } => {
                if let Some(seg) = t.segments.iter_mut().find(|s| s.id == change.element_id)
                    && let Some(h) = seg.height.as_mut()
                {
                    let delta = asbuilt_ft - h.centre_ft;
                    h.bottom_ft += delta;
                    h.top_ft += delta;
                    h.centre_ft = *asbuilt_ft;
                }
            }
            ChangeKind::FieldVerified { .. }
            | ChangeKind::Added
            | ChangeKind::Note => {}
        }
    }
    t
}

fn apply_move(t: &mut Truth, id: &str, kind: &str, to: [f64; 3]) {
    match kind {
        "elec_device" => {
            if let Some(d) = t.elec_devices.iter_mut().find(|d| d.id == id) {
                d.at_ft = to;
                d.drawn_ft = [to[0], to[1]];
            }
        }
        "equipment" => {
            if let Some(e) = t.equipment.iter_mut().find(|e| e.id == id) {
                let dx = to[0] - (e.min_ft[0] + e.max_ft[0]) * 0.5;
                let dy = to[1] - (e.min_ft[1] + e.max_ft[1]) * 0.5;
                let dz = to[2] - (e.min_ft[2] + e.max_ft[2]) * 0.5;
                e.min_ft = [e.min_ft[0] + dx, e.min_ft[1] + dy, e.min_ft[2] + dz];
                e.max_ft = [e.max_ft[0] + dx, e.max_ft[1] + dy, e.max_ft[2] + dz];
            }
        }
        "terminal" => {
            if let Some(tm) = t.terminals.iter_mut().find(|tm| tm.id == id) {
                tm.at_ft = to;
            }
        }
        "fixture" => {
            if let Some(f) = t.fixtures.iter_mut().find(|f| f.id == id) {
                f.at_ft = to;
            }
        }
        "elec_conduit" => {
            if let Some(c) = t.elec_conduits.iter_mut().find(|c| c.id == id) {
                let dx = to[0] - (c.from_ft[0] + c.to_ft[0]) * 0.5;
                let dy = to[1] - (c.from_ft[1] + c.to_ft[1]) * 0.5;
                c.from_ft = [c.from_ft[0] + dx, c.from_ft[1] + dy];
                c.to_ft = [c.to_ft[0] + dx, c.to_ft[1] + dy];
            }
        }
        "segment" => {
            if let Some(s) = t.segments.iter_mut().find(|s| s.id == id) {
                let dx = to[0] - (s.path_ft[0][0] + s.path_ft[1][0]) * 0.5;
                let dy = to[1] - (s.path_ft[0][1] + s.path_ft[1][1]) * 0.5;
                let dz = to[2] - (s.path_ft[0][2] + s.path_ft[1][2]) * 0.5;
                s.path_ft[0] = [s.path_ft[0][0] + dx, s.path_ft[0][1] + dy, s.path_ft[0][2] + dz];
                s.path_ft[1] = [s.path_ft[1][0] + dx, s.path_ft[1][1] + dy, s.path_ft[1][2] + dz];
            }
        }
        _ => {}
    }
}

fn apply_type_change(t: &mut Truth, id: &str, kind: &str, new_type: &str) {
    match kind {
        "elec_device" => {
            if let Some(d) = t.elec_devices.iter_mut().find(|d| d.id == id) {
                d.type_tag = new_type.into();
            }
        }
        "elec_conduit" => {
            if let Some(c) = t.elec_conduits.iter_mut().find(|c| c.id == id) {
                c.conduit_type = new_type.into();
            }
        }
        "terminal" => {
            if let Some(tm) = t.terminals.iter_mut().find(|tm| tm.id == id) {
                tm.type_tag = new_type.into();
            }
        }
        _ => {}
    }
}

fn apply_size_change(t: &mut Truth, id: &str, new_size: &str) {
    // Try parsing as a number for segments/conduits
    if let Some(seg) = t.segments.iter_mut().find(|s| s.id == id)
        && let Ok(v) = new_size.parse::<f64>()
    {
        seg.size_in = [v, v];
    }
    if let Some(c) = t.elec_conduits.iter_mut().find(|c| c.id == id) {
        c.size = new_size.into();
        if let Ok(v) = new_size.parse::<f64>() {
            c.size_in = v;
        }
    }
}

fn apply_delete(t: &mut Truth, id: &str, kind: &str) {
    match kind {
        "segment" => t.segments.retain(|s| s.id != id),
        "elec_device" => t.elec_devices.retain(|d| d.id != id),
        "elec_conduit" => t.elec_conduits.retain(|c| c.id != id),
        "equipment" => t.equipment.retain(|e| e.id != id),
        "terminal" => t.terminals.retain(|tm| tm.id != id),
        "fixture" => t.fixtures.retain(|f| f.id != id),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// As-built drawing helpers
// ---------------------------------------------------------------------------

/// Produce an as-built drawing (cadaddy_doc::Drawing) for a level, with changes marked.
///
/// Design elements are drawn normally; changes are overlaid:
/// - Moved: arrow from old to new, green.
/// - Deleted: X through the element, red.
/// - Added: circle at position, blue.
/// - Type/size change: diamond at position, magenta.
/// - Field verified: checkmark at position, green.
/// - Rerouted: dashed new path in cyan.
/// - Note: text at position, yellow.
///
/// Returns a `Drawing` with the as-built overlay on layer `ASBUILT-CHANGES`.
pub fn asbuilt_overlay(
    record: &AsBuiltRecord,
    level: &str,
) -> Vec<AsBuiltMark> {
    let mut marks = Vec::new();
    for change in &record.changes {
        if change.level != level || change.status == ChangeStatus::Rejected {
            continue;
        }
        let symbol = match &change.kind {
            ChangeKind::Moved { from_ft, to_ft } => MarkSymbol::Arrow {
                from: [from_ft[0], from_ft[1]],
                to: [to_ft[0], to_ft[1]],
            },
            ChangeKind::Rerouted { design_path_ft, asbuilt_path_ft } => MarkSymbol::Reroute {
                old_from: [design_path_ft[0][0], design_path_ft[0][1]],
                old_to: [design_path_ft[1][0], design_path_ft[1][1]],
                new_from: [asbuilt_path_ft[0][0], asbuilt_path_ft[0][1]],
                new_to: [asbuilt_path_ft[1][0], asbuilt_path_ft[1][1]],
            },
            ChangeKind::TypeChanged { .. } | ChangeKind::SizeChanged { .. } => {
                MarkSymbol::Diamond(change.at_ft)
            }
            ChangeKind::Added => MarkSymbol::Circle(change.at_ft),
            ChangeKind::Deleted => MarkSymbol::Cross(change.at_ft),
            ChangeKind::FieldVerified { at_ft } => {
                MarkSymbol::Check([at_ft[0], at_ft[1]])
            }
            ChangeKind::HeightChanged { .. } => MarkSymbol::Diamond(change.at_ft),
            ChangeKind::Note => MarkSymbol::Text(change.at_ft, change.reason.clone()),
        };
        marks.push(AsBuiltMark {
            change_id: change.id.clone(),
            element_id: change.element_id.clone(),
            status: change.status,
            symbol,
            label: mark_label(change),
        });
    }
    marks
}

/// A symbol on the as-built overlay.
#[derive(Clone, Debug, PartialEq)]
pub enum MarkSymbol {
    /// Arrow from old position to new, feet.
    Arrow { from: [f64; 2], to: [f64; 2] },
    /// Reroute: old and new paths in plan, feet.
    Reroute {
        old_from: [f64; 2],
        old_to: [f64; 2],
        new_from: [f64; 2],
        new_to: [f64; 2],
    },
    /// Diamond at a point (type/size change).
    Diamond([f64; 2]),
    /// Circle at a point (added element).
    Circle([f64; 2]),
    /// X through a point (deleted element).
    Cross([f64; 2]),
    /// Checkmark at a point (field verified).
    Check([f64; 2]),
    /// Text at a point.
    Text([f64; 2], String),
}

/// An as-built mark for the overlay drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct AsBuiltMark {
    pub change_id: String,
    pub element_id: String,
    pub status: ChangeStatus,
    pub symbol: MarkSymbol,
    pub label: String,
}

fn mark_label(change: &AsBuiltChange) -> String {
    let status = match change.status {
        ChangeStatus::Pending => "PENDING",
        ChangeStatus::FieldDecision => "FIELD",
        ChangeStatus::RfiAnswer => "RFI",
        ChangeStatus::ChangeOrder => "CO",
        ChangeStatus::Rejected => "REJ",
    };
    let kind = match &change.kind {
        ChangeKind::Moved { .. } => "MOVED",
        ChangeKind::Rerouted { .. } => "REROUTED",
        ChangeKind::TypeChanged { .. } => "TYPE CHG",
        ChangeKind::SizeChanged { .. } => "SIZE CHG",
        ChangeKind::Added => "ADDED",
        ChangeKind::Deleted => "DELETED",
        ChangeKind::FieldVerified { .. } => "VERIFIED",
        ChangeKind::HeightChanged { .. } => "HT CHG",
        ChangeKind::Note => "NOTE",
    };
    format!("{} [{}]", kind, status)
}

// ---------------------------------------------------------------------------
// As-built IFC property set
// ---------------------------------------------------------------------------

/// IFC property set entries for an as-built change, to be added to the element's IFC export.
/// Returns key-value pairs for an `IfcPropertySet` named `CaDaddy_AsBuilt`.
pub fn asbuilt_ifc_properties(change: &AsBuiltChange) -> BTreeMap<String, String> {
    let mut props = BTreeMap::new();
    props.insert("AsBuiltChangeId".into(), change.id.clone());
    props.insert(
        "AsBuiltStatus".into(),
        match change.status {
            ChangeStatus::Pending => "Pending",
            ChangeStatus::FieldDecision => "Field Decision",
            ChangeStatus::RfiAnswer => "RFI Answer",
            ChangeStatus::ChangeOrder => "Change Order",
            ChangeStatus::Rejected => "Rejected",
        }
        .into(),
    );
    props.insert("AsBuiltAuthor".into(), change.author.clone());
    props.insert("AsBuiltDate".into(), change.date.clone());
    props.insert("AsBuiltReason".into(), change.reason.clone());
    match &change.kind {
        ChangeKind::Moved { from_ft, to_ft } => {
            props.insert(
                "AsBuiltDescription".into(),
                format!(
                    "Moved from ({:.2}, {:.2}, {:.2}) to ({:.2}, {:.2}, {:.2})",
                    from_ft[0], from_ft[1], from_ft[2], to_ft[0], to_ft[1], to_ft[2]
                ),
            );
        }
        ChangeKind::Rerouted { .. } => {
            props.insert("AsBuiltDescription".into(), "Run rerouted".into());
        }
        ChangeKind::TypeChanged { design_type, asbuilt_type } => {
            props.insert(
                "AsBuiltDescription".into(),
                format!("Type changed: {} -> {}", design_type, asbuilt_type),
            );
        }
        ChangeKind::SizeChanged { design_size, asbuilt_size } => {
            props.insert(
                "AsBuiltDescription".into(),
                format!("Size changed: {} -> {}", design_size, asbuilt_size),
            );
        }
        ChangeKind::Added => {
            props.insert("AsBuiltDescription".into(), "Added in field".into());
        }
        ChangeKind::Deleted => {
            props.insert("AsBuiltDescription".into(), "Not built (deleted)".into());
        }
        ChangeKind::FieldVerified { at_ft } => {
            props.insert(
                "AsBuiltDescription".into(),
                format!("Field verified at ({:.2}, {:.2}, {:.2})", at_ft[0], at_ft[1], at_ft[2]),
            );
        }
        ChangeKind::HeightChanged { design_ft, asbuilt_ft } => {
            props.insert(
                "AsBuiltDescription".into(),
                format!(
                    "Height changed: {} -> {}",
                    truth::feet_inches(*design_ft),
                    truth::feet_inches(*asbuilt_ft)
                ),
            );
        }
        ChangeKind::Note => {
            props.insert("AsBuiltDescription".into(), change.reason.clone());
        }
    }
    props
}

// ---------------------------------------------------------------------------
// Summary / report
// ---------------------------------------------------------------------------

/// Summary statistics of an as-built record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AsBuiltSummary {
    pub total_changes: usize,
    pub pending: usize,
    pub field_decisions: usize,
    pub rfi_answers: usize,
    pub change_orders: usize,
    pub rejected: usize,
    pub moves: usize,
    pub reroutes: usize,
    pub type_changes: usize,
    pub size_changes: usize,
    pub additions: usize,
    pub deletions: usize,
    pub verifications: usize,
    pub height_changes: usize,
    pub notes: usize,
    pub levels: BTreeMap<String, usize>,
}

/// Compute summary statistics.
pub fn summarize(record: &AsBuiltRecord) -> AsBuiltSummary {
    let mut s = AsBuiltSummary {
        total_changes: record.changes.len(),
        ..Default::default()
    };
    for c in &record.changes {
        match c.status {
            ChangeStatus::Pending => s.pending += 1,
            ChangeStatus::FieldDecision => s.field_decisions += 1,
            ChangeStatus::RfiAnswer => s.rfi_answers += 1,
            ChangeStatus::ChangeOrder => s.change_orders += 1,
            ChangeStatus::Rejected => s.rejected += 1,
        }
        match &c.kind {
            ChangeKind::Moved { .. } => s.moves += 1,
            ChangeKind::Rerouted { .. } => s.reroutes += 1,
            ChangeKind::TypeChanged { .. } => s.type_changes += 1,
            ChangeKind::SizeChanged { .. } => s.size_changes += 1,
            ChangeKind::Added => s.additions += 1,
            ChangeKind::Deleted => s.deletions += 1,
            ChangeKind::FieldVerified { .. } => s.verifications += 1,
            ChangeKind::HeightChanged { .. } => s.height_changes += 1,
            ChangeKind::Note => s.notes += 1,
        }
        *s.levels.entry(c.level.clone()).or_insert(0) += 1;
    }
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_truth() -> Truth {
        crate::truth_for(1)
    }

    fn make_truth_with_sheets() -> Truth {
        crate::generate(1).map(|fs| fs.truth).unwrap_or_else(|_| make_truth())
    }

    #[test]
    fn asbuilt_record_add_and_query() {
        let mut rec = AsBuiltRecord::new("Test Project");
        assert_eq!(rec.schema, "cadaddy-mepfix-asbuilt/1");
        let change = AsBuiltChange {
            id: "C-1".into(),
            element_id: "seg-1".into(),
            element_kind: "segment".into(),
            kind: ChangeKind::Moved {
                from_ft: [10.0, 20.0, 8.0],
                to_ft: [12.0, 20.0, 8.0],
            },
            status: ChangeStatus::FieldDecision,
            author: "Foreman A".into(),
            date: "2026-10-21".into(),
            reason: "Avoided existing conduit".into(),
            notes: String::new(),
            level: "1".into(),
            at_ft: [12.0, 20.0],
            markup_id: None,
        };
        assert!(rec.add_change(change));
        assert_eq!(rec.changes.len(), 1);
        assert_eq!(rec.changes_for("seg-1").len(), 1);
        assert_eq!(rec.changes_for("seg-2").len(), 0);
        assert_eq!(rec.by_status(ChangeStatus::FieldDecision).len(), 1);
        assert_eq!(rec.by_status(ChangeStatus::Pending).len(), 0);
        assert_eq!(rec.on_level("1").len(), 1);
        assert_eq!(rec.on_level("2").len(), 0);
    }

    #[test]
    fn asbuilt_record_cap() {
        let mut rec = AsBuiltRecord::new("Cap Test");
        for i in 0..MAX_CHANGES {
            assert!(rec.add_change(AsBuiltChange {
                id: format!("C-{i}"),
                element_id: "e".into(),
                element_kind: "segment".into(),
                kind: ChangeKind::Note,
                status: ChangeStatus::Pending,
                author: String::new(),
                date: String::new(),
                reason: String::new(),
                notes: String::new(),
                level: "1".into(),
                at_ft: [0.0, 0.0],
                markup_id: None,
            }));
        }
        // Next add should fail
        assert!(!rec.add_change(AsBuiltChange {
            id: "overflow".into(),
            element_id: "e".into(),
            element_kind: "segment".into(),
            kind: ChangeKind::Note,
            status: ChangeStatus::Pending,
            author: String::new(),
            date: String::new(),
            reason: String::new(),
            notes: String::new(),
            level: "1".into(),
            at_ft: [0.0, 0.0],
            markup_id: None,
        }));
    }

    #[test]
    fn paper_to_building_round_trip() {
        let t = make_truth_with_sheets();
        // Use a plan sheet (has coordinate transforms)
        let Some(sh) = t.sheets.iter().find(|s| s.level.is_some()) else {
            return; // No plan sheets in this seed
        };
        let level = sh.level.as_deref().unwrap_or("1");
        if let Some(seg) = t.segments.iter().find(|s| s.level == level) {
            let bldg = [
                (seg.path_ft[0][0] + seg.path_ft[1][0]) * 0.5,
                (seg.path_ft[0][1] + seg.path_ft[1][1]) * 0.5,
            ];
            let paper = building_to_paper(sh, bldg);
            let back = paper_to_building(sh, paper);
            assert!(
                (back[0] - bldg[0]).abs() < 0.01 && (back[1] - bldg[1]).abs() < 0.01,
                "round trip failed: {bldg:?} -> {paper:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn import_synthetic_markups_maps_to_elements() {
        let t = make_truth_with_sheets();
        let Some(sh) = t.sheets.iter().find(|s| s.level.is_some()) else {
            return;
        };
        let level = sh.level.as_deref().unwrap_or("1");

        // Find a real segment on this level and compute its paper position
        if let Some(seg) = t.segments.iter().find(|s| s.level == level) {
            let bldg = [
                (seg.path_ft[0][0] + seg.path_ft[1][0]) * 0.5,
                (seg.path_ft[0][1] + seg.path_ft[1][1]) * 0.5,
            ];
            let paper = building_to_paper(sh, bldg);
            let markup = SyntheticMarkup {
                kind: "cloud".into(),
                text: "Move duct 2 feet north".into(),
                author: "Field Inspector".into(),
                date: "2026-10-21".into(),
                paper_in: paper,
                paper_rect_in: [[paper[0] - 0.5, paper[1] - 0.5], [paper[0] + 0.5, paper[1] + 0.5]],
            };
            let mapped = import_markups(&t, sh, &[markup]);
            assert_eq!(mapped.len(), 1);
            let m = &mapped[0];
            assert_eq!(m.kind, "cloud");
            assert_eq!(m.sheet, sh.number);
            assert_eq!(m.level, level);
            assert!(!m.near_elements.is_empty(), "should find nearby elements");
            // The segment we targeted should be among the near elements
            assert!(
                m.near_elements.contains(&seg.id),
                "expected {} in near_elements {:?}",
                seg.id,
                m.near_elements
            );
        }
    }

    #[test]
    fn markup_to_change_cloud_move() {
        let t = make_truth();
        let markup = MappedMarkup {
            id: "MK-1".into(),
            kind: "cloud".into(),
            text: "Move receptacle to this wall".into(),
            author: "Inspector".into(),
            date: "2026-10-21".into(),
            sheet: "E-101".into(),
            paper_pt: [5.0, 5.0],
            building_ft: [20.0, 15.0],
            level: "1".into(),
            near_elements: vec![
                t.elec_devices.first().map(|d| d.id.clone()).unwrap_or_else(|| "dev-1".into()),
            ],
            bounds_ft: [[19.5, 14.5], [20.5, 15.5]],
        };
        let change = markup_to_change(&markup, &t, "C-1");
        assert!(change.is_some());
        let c = change.unwrap();
        assert_eq!(c.id, "C-1");
        assert!(matches!(c.kind, ChangeKind::Moved { .. }));
        assert_eq!(c.status, ChangeStatus::Pending);
        assert_eq!(c.markup_id, Some("MK-1".into()));
    }

    #[test]
    fn markup_to_change_callout_verify() {
        let t = make_truth();
        let dev_id = t.elec_devices.first().map(|d| d.id.clone()).unwrap_or_else(|| "dev-1".into());
        let markup = MappedMarkup {
            id: "MK-2".into(),
            kind: "callout".into(),
            text: "Verified in field - receptacle at correct height".into(),
            author: "QC".into(),
            date: "2026-10-21".into(),
            sheet: "E-101".into(),
            paper_pt: [5.0, 5.0],
            building_ft: [20.0, 15.0],
            level: "1".into(),
            near_elements: vec![dev_id.clone()],
            bounds_ft: [[19.5, 14.5], [20.5, 15.5]],
        };
        let change = markup_to_change(&markup, &t, "C-2");
        assert!(change.is_some());
        let c = change.unwrap();
        assert!(matches!(c.kind, ChangeKind::FieldVerified { .. }));
    }

    #[test]
    fn markup_to_change_empty_near_returns_none() {
        let t = make_truth();
        let markup = MappedMarkup {
            id: "MK-3".into(),
            kind: "cloud".into(),
            text: "Random cloud".into(),
            author: "A".into(),
            date: "2026-10-21".into(),
            sheet: "E-101".into(),
            paper_pt: [5.0, 5.0],
            building_ft: [20.0, 15.0],
            level: "1".into(),
            near_elements: vec![],
            bounds_ft: [[19.5, 14.5], [20.5, 15.5]],
        };
        assert!(markup_to_change(&markup, &t, "C-3").is_none());
    }

    #[test]
    fn compare_design_vs_asbuilt_produces_deviations() {
        let t = make_truth();
        let mut rec = AsBuiltRecord::new(&t.project);
        // Add a move
        if let Some(dev) = t.elec_devices.first() {
            rec.add_change(AsBuiltChange {
                id: "C-1".into(),
                element_id: dev.id.clone(),
                element_kind: "elec_device".into(),
                kind: ChangeKind::Moved {
                    from_ft: dev.at_ft,
                    to_ft: [dev.at_ft[0] + 2.0, dev.at_ft[1], dev.at_ft[2]],
                },
                status: ChangeStatus::FieldDecision,
                author: "A".into(),
                date: "2026-10-21".into(),
                reason: "Avoid pipe".into(),
                notes: String::new(),
                level: dev.level.clone(),
                at_ft: [dev.at_ft[0] + 2.0, dev.at_ft[1]],
                markup_id: None,
            });
        }
        // Add a rejected change (should not appear)
        rec.add_change(AsBuiltChange {
            id: "C-REJ".into(),
            element_id: "fake".into(),
            element_kind: "segment".into(),
            kind: ChangeKind::Deleted,
            status: ChangeStatus::Rejected,
            author: "A".into(),
            date: "2026-10-21".into(),
            reason: "Rejected".into(),
            notes: String::new(),
            level: "1".into(),
            at_ft: [0.0, 0.0],
            markup_id: None,
        });
        let devs = compare_design_vs_asbuilt(&t, &rec);
        // Should have the move but not the rejected one
        assert_eq!(devs.len(), 1);
        assert!(devs[0].description.contains("Moved"));
        assert!((devs[0].magnitude - 2.0).abs() < 0.01);
    }

    #[test]
    fn asbuilt_truth_applies_changes() {
        let t = make_truth();
        let mut rec = AsBuiltRecord::new(&t.project);
        // Move a device
        if let Some(dev) = t.elec_devices.first() {
            let new_pos = [dev.at_ft[0] + 3.0, dev.at_ft[1] + 1.0, dev.at_ft[2]];
            rec.add_change(AsBuiltChange {
                id: "C-1".into(),
                element_id: dev.id.clone(),
                element_kind: "elec_device".into(),
                kind: ChangeKind::Moved {
                    from_ft: dev.at_ft,
                    to_ft: new_pos,
                },
                status: ChangeStatus::FieldDecision,
                author: "A".into(),
                date: "2026-10-21".into(),
                reason: "Field move".into(),
                notes: String::new(),
                level: dev.level.clone(),
                at_ft: [new_pos[0], new_pos[1]],
                markup_id: None,
            });
            let ab = asbuilt_truth(&t, &rec);
            let ab_dev = ab.elec_devices.iter().find(|d| d.id == dev.id);
            assert!(ab_dev.is_some());
            let abd = ab_dev.unwrap();
            assert!((abd.at_ft[0] - new_pos[0]).abs() < 0.001);
            assert!((abd.at_ft[1] - new_pos[1]).abs() < 0.001);
        }
    }

    #[test]
    fn asbuilt_truth_delete_removes_element() {
        let t = make_truth();
        let mut rec = AsBuiltRecord::new(&t.project);
        let orig_count = t.elec_devices.len();
        if let Some(dev) = t.elec_devices.first() {
            rec.add_change(AsBuiltChange {
                id: "C-DEL".into(),
                element_id: dev.id.clone(),
                element_kind: "elec_device".into(),
                kind: ChangeKind::Deleted,
                status: ChangeStatus::ChangeOrder,
                author: "A".into(),
                date: "2026-10-21".into(),
                reason: "CO #5: remove per revised drawing".into(),
                notes: String::new(),
                level: dev.level.clone(),
                at_ft: [dev.at_ft[0], dev.at_ft[1]],
                markup_id: None,
            });
            let ab = asbuilt_truth(&t, &rec);
            assert_eq!(ab.elec_devices.len(), orig_count - 1);
        }
    }

    #[test]
    fn asbuilt_truth_rejected_change_not_applied() {
        let t = make_truth();
        let mut rec = AsBuiltRecord::new(&t.project);
        if let Some(dev) = t.elec_devices.first() {
            rec.add_change(AsBuiltChange {
                id: "C-REJ".into(),
                element_id: dev.id.clone(),
                element_kind: "elec_device".into(),
                kind: ChangeKind::Deleted,
                status: ChangeStatus::Rejected,
                author: "A".into(),
                date: "2026-10-21".into(),
                reason: "Rejected".into(),
                notes: String::new(),
                level: dev.level.clone(),
                at_ft: [dev.at_ft[0], dev.at_ft[1]],
                markup_id: None,
            });
            let ab = asbuilt_truth(&t, &rec);
            assert_eq!(ab.elec_devices.len(), t.elec_devices.len());
        }
    }

    #[test]
    fn asbuilt_overlay_marks() {
        let mut rec = AsBuiltRecord::new("P");
        rec.add_change(AsBuiltChange {
            id: "C-1".into(),
            element_id: "d-1".into(),
            element_kind: "elec_device".into(),
            kind: ChangeKind::Moved {
                from_ft: [10.0, 20.0, 4.0],
                to_ft: [12.0, 20.0, 4.0],
            },
            status: ChangeStatus::FieldDecision,
            author: "A".into(),
            date: "2026-10-21".into(),
            reason: "Move".into(),
            notes: String::new(),
            level: "1".into(),
            at_ft: [12.0, 20.0],
            markup_id: None,
        });
        rec.add_change(AsBuiltChange {
            id: "C-2".into(),
            element_id: "d-2".into(),
            element_kind: "elec_device".into(),
            kind: ChangeKind::Deleted,
            status: ChangeStatus::Rejected,
            author: "A".into(),
            date: "2026-10-21".into(),
            reason: "Rej".into(),
            notes: String::new(),
            level: "1".into(),
            at_ft: [5.0, 5.0],
            markup_id: None,
        });
        let marks = asbuilt_overlay(&rec, "1");
        assert_eq!(marks.len(), 1); // rejected filtered out
        assert!(matches!(marks[0].symbol, MarkSymbol::Arrow { .. }));
        assert!(marks[0].label.contains("MOVED"));
        assert!(marks[0].label.contains("FIELD"));
    }

    #[test]
    fn asbuilt_ifc_properties_populated() {
        let change = AsBuiltChange {
            id: "C-1".into(),
            element_id: "seg-1".into(),
            element_kind: "segment".into(),
            kind: ChangeKind::TypeChanged {
                design_type: "EMT".into(),
                asbuilt_type: "RGS".into(),
            },
            status: ChangeStatus::RfiAnswer,
            author: "Engineer".into(),
            date: "2026-10-21".into(),
            reason: "RFI-42: wet location".into(),
            notes: String::new(),
            level: "1".into(),
            at_ft: [15.0, 25.0],
            markup_id: None,
        };
        let props = asbuilt_ifc_properties(&change);
        assert_eq!(props.get("AsBuiltStatus").map(|s| s.as_str()), Some("RFI Answer"));
        assert_eq!(props.get("AsBuiltAuthor").map(|s| s.as_str()), Some("Engineer"));
        assert!(props.get("AsBuiltDescription").unwrap().contains("EMT"));
        assert!(props.get("AsBuiltDescription").unwrap().contains("RGS"));
    }

    #[test]
    fn summarize_counts() {
        let mut rec = AsBuiltRecord::new("P");
        let kinds = [
            (ChangeKind::Moved { from_ft: [0.0; 3], to_ft: [1.0; 3] }, ChangeStatus::FieldDecision),
            (ChangeKind::Deleted, ChangeStatus::ChangeOrder),
            (ChangeKind::Added, ChangeStatus::Pending),
            (ChangeKind::TypeChanged { design_type: "A".into(), asbuilt_type: "B".into() }, ChangeStatus::RfiAnswer),
            (ChangeKind::FieldVerified { at_ft: [0.0; 3] }, ChangeStatus::FieldDecision),
            (ChangeKind::Note, ChangeStatus::Rejected),
        ];
        for (i, (kind, status)) in kinds.iter().enumerate() {
            rec.add_change(AsBuiltChange {
                id: format!("C-{i}"),
                element_id: format!("e-{i}"),
                element_kind: "segment".into(),
                kind: kind.clone(),
                status: *status,
                author: String::new(),
                date: String::new(),
                reason: String::new(),
                notes: String::new(),
                level: if i < 3 { "1" } else { "2" }.into(),
                at_ft: [0.0, 0.0],
                markup_id: None,
            });
        }
        let s = summarize(&rec);
        assert_eq!(s.total_changes, 6);
        assert_eq!(s.pending, 1);
        assert_eq!(s.field_decisions, 2);
        assert_eq!(s.rfi_answers, 1);
        assert_eq!(s.change_orders, 1);
        assert_eq!(s.rejected, 1);
        assert_eq!(s.moves, 1);
        assert_eq!(s.deletions, 1);
        assert_eq!(s.additions, 1);
        assert_eq!(s.type_changes, 1);
        assert_eq!(s.verifications, 1);
        assert_eq!(s.notes, 1);
        assert_eq!(s.levels.get("1"), Some(&3));
        assert_eq!(s.levels.get("2"), Some(&3));
    }

    #[test]
    fn serde_round_trip() {
        let mut rec = AsBuiltRecord::new("Test");
        rec.add_change(AsBuiltChange {
            id: "C-1".into(),
            element_id: "seg-1".into(),
            element_kind: "segment".into(),
            kind: ChangeKind::HeightChanged {
                design_ft: 10.5,
                asbuilt_ft: 11.0,
            },
            status: ChangeStatus::RfiAnswer,
            author: "Eng".into(),
            date: "2026-10-21".into(),
            reason: "RFI-7".into(),
            notes: "Raised for clearance".into(),
            level: "1".into(),
            at_ft: [25.0, 30.0],
            markup_id: Some("MK-5".into()),
        });
        let json = serde_json::to_string_pretty(&rec).unwrap();
        let back: AsBuiltRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(rec, back);
    }

    #[test]
    fn hostile_inputs_never_panic() {
        let t = make_truth_with_sheets();
        // Empty markup list
        let Some(sh) = t.sheets.first() else {
            return;
        };
        let mapped = import_markups(&t, sh, &[]);
        assert!(mapped.is_empty());

        // Markup with NaN coordinates
        let nan_markup = SyntheticMarkup {
            kind: "cloud".into(),
            text: String::new(),
            author: String::new(),
            date: String::new(),
            paper_in: [f64::NAN, f64::NAN],
            paper_rect_in: [[f64::NAN; 2]; 2],
        };
        let mapped = import_markups(&t, sh, &[nan_markup]);
        assert_eq!(mapped.len(), 1); // Still produces a result, just with NaN building coords

        // Infinity coordinates
        let inf_markup = SyntheticMarkup {
            kind: "arrow".into(),
            text: String::new(),
            author: String::new(),
            date: String::new(),
            paper_in: [f64::INFINITY, f64::NEG_INFINITY],
            paper_rect_in: [[f64::INFINITY; 2]; 2],
        };
        let mapped = import_markups(&t, sh, &[inf_markup]);
        assert_eq!(mapped.len(), 1);

        // Very long text
        let long_text = "x".repeat(5000);
        let long_markup = SyntheticMarkup {
            kind: "note".into(),
            text: long_text.clone(),
            author: long_text,
            date: String::new(),
            paper_in: [0.0, 0.0],
            paper_rect_in: [[0.0; 2]; 2],
        };
        let mapped = import_markups(&t, sh, &[long_markup]);
        assert_eq!(mapped.len(), 1);
        assert!(mapped[0].text.len() <= MAX_TEXT);
        assert!(mapped[0].author.len() <= MAX_TEXT);

        // Empty record operations
        let rec = AsBuiltRecord::new("P");
        assert!(rec.changes_for("anything").is_empty());
        assert!(rec.by_status(ChangeStatus::Pending).is_empty());
        let devs = compare_design_vs_asbuilt(&t, &rec);
        assert!(devs.is_empty());
        let ab = asbuilt_truth(&t, &rec);
        assert_eq!(ab.segments.len(), t.segments.len());

        // Empty overlay
        let marks = asbuilt_overlay(&rec, "1");
        assert!(marks.is_empty());

        // Summary of empty
        let s = summarize(&rec);
        assert_eq!(s.total_changes, 0);

        // Markup to change with no near elements
        let empty_markup = MappedMarkup {
            id: "MK-0".into(),
            kind: "cloud".into(),
            text: String::new(),
            author: String::new(),
            date: String::new(),
            sheet: "E-101".into(),
            paper_pt: [0.0, 0.0],
            building_ft: [0.0, 0.0],
            level: "1".into(),
            near_elements: vec![],
            bounds_ft: [[0.0; 2]; 2],
        };
        assert!(markup_to_change(&empty_markup, &t, "C-0").is_none());

        // Change with nonexistent element id
        let mut rec2 = AsBuiltRecord::new("P");
        rec2.add_change(AsBuiltChange {
            id: "C-BAD".into(),
            element_id: "nonexistent-id".into(),
            element_kind: "segment".into(),
            kind: ChangeKind::Moved { from_ft: [0.0; 3], to_ft: [1.0; 3] },
            status: ChangeStatus::Pending,
            author: String::new(),
            date: String::new(),
            reason: String::new(),
            notes: String::new(),
            level: "1".into(),
            at_ft: [0.0, 0.0],
            markup_id: None,
        });
        // Should not panic when applying to truth
        let ab2 = asbuilt_truth(&t, &rec2);
        assert_eq!(ab2.segments.len(), t.segments.len()); // No segment matched, nothing changed
    }
}
