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

use crate::framing::{self, FieldSnap, Framing};
use crate::truth::{self, Sheet, Truth};
use cadaddy_color::Color;
use cadaddy_doc::markup::{MarkupKind, MarkupPage};
use cadaddy_doc::{Circle, Common, Drawing, EntityKind, HAlign, Layer, LwPolyline, Mesh, MeshFace, RawTag, Space, Text, VAlign};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum number of changes in a single as-built record (hostile input cap).
const MAX_CHANGES: usize = 10_000;
/// Maximum length of a free-text field (reason, notes), bytes.
pub const MAX_TEXT: usize = 2_000;
/// Maximum number of markups to process from one PDF page.
const MAX_MARKUPS_PER_PAGE: usize = 1_000;
/// A markup refers to model elements within this distance of where it points, feet (an area
/// markup: also the elements inside it).
const NEAR_FT: f64 = 3.0;
/// The farthest an area markup's own extent reaches for elements, feet.
const MAX_REACH_FT: f64 = 50.0;
/// Elements kept as candidates for one markup, nearest first.
const MAX_NEAR: usize = 50;

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
    TypeChanged { design_type: String, asbuilt_type: String },
    /// An element's size was changed.
    SizeChanged { design_size: String, asbuilt_size: String },
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
    HeightChanged { design_ft: f64, asbuilt_ft: f64 },
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
    /// Where an arrow (or a line) points to, building feet: its head, when its tail is on the
    /// element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_ft: Option<[f64; 2]>,
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
        Self { schema: "cadaddy-mepfix-asbuilt/1".into(), project: project.into(), changes: Vec::new(), markups: Vec::new() }
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
#[derive(Clone, Debug, Default)]
pub struct SyntheticMarkup {
    pub kind: String,
    pub text: String,
    pub author: String,
    pub date: String,
    /// Centre point in paper inches on the sheet.
    pub paper_in: [f64; 2],
    /// Bounding rect in paper inches: [min, max].
    pub paper_rect_in: [[f64; 2]; 2],
    /// An arrow's head in paper inches (its tail is `paper_in`, on the element).
    pub to_in: Option<[f64; 2]>,
}

/// Map a paper-space point (inches) to building coordinates (feet) through a sheet's transform.
pub fn paper_to_building(sh: &Sheet, paper_in: [f64; 2]) -> [f64; 2] {
    // Paper → model: model = (paper / paper_from_model.scale) + paper_from_model.origin_in
    let pm = &sh.paper_from_model;
    let scale = if pm.scale.abs() < 1e-12 { 1.0 } else { pm.scale };
    let model_in = [paper_in[0] / scale + pm.origin_in[0], paper_in[1] / scale + pm.origin_in[1]];
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
    let model_in = [bx * cos_r - by * sin_r + mb.offset_in[0], bx * sin_r + by * cos_r + mb.offset_in[1]];
    let pm = &sh.paper_from_model;
    let scale = if pm.scale.abs() < 1e-12 { 1.0 } else { pm.scale };
    [(model_in[0] - pm.origin_in[0]) * scale, (model_in[1] - pm.origin_in[1]) * scale]
}

/// Import synthetic markups from a sheet and map them to building coordinates and nearby elements.
pub fn import_markups(truth: &Truth, sheet: &Sheet, markups: &[SyntheticMarkup]) -> Vec<MappedMarkup> {
    let level = sheet.level.clone().unwrap_or_default();
    let mut result = Vec::new();
    for (i, m) in markups.iter().take(MAX_MARKUPS_PER_PAGE).enumerate() {
        let bldg = paper_to_building(sheet, m.paper_in);
        let bmin = paper_to_building(sheet, m.paper_rect_in[0]);
        let bmax = paper_to_building(sheet, m.paper_rect_in[1]);
        let near = find_near_elements(truth, &level, bldg, reach(&m.kind, [bmin, bmax]));
        result.push(MappedMarkup {
            id: format!("MK-{}-{}", sheet.number, i + 1),
            kind: m.kind.clone(),
            text: truncate(&m.text, MAX_TEXT),
            author: truncate(&m.author, MAX_TEXT),
            date: truncate(&m.date, 64),
            sheet: sheet.number.clone(),
            paper_pt: m.paper_in,
            building_ft: bldg,
            level: level.clone(),
            near_elements: near,
            bounds_ft: [bmin, bmax],
            to_ft: m.to_in.map(|p| paper_to_building(sheet, p)),
        });
    }
    result
}

/// Whether a markup kind marks an area (a cloud round something) rather than a point.
fn is_area(kind: &str) -> bool {
    matches!(kind, "cloud" | "rectangle" | "ellipse" | "polygon" | "ink" | "stamp" | "highlight")
}

/// How far from its point a markup reaches for elements: [`NEAR_FT`], or half across an area
/// markup's extent when that is bigger (at most [`MAX_REACH_FT`]).
fn reach(kind: &str, bounds: [[f64; 2]; 2]) -> f64 {
    if !is_area(kind) {
        return NEAR_FT;
    }
    let half = ((bounds[1][0] - bounds[0][0]).hypot(bounds[1][1] - bounds[0][1])) / 2.0;
    if half.is_finite() { half.clamp(NEAR_FT, MAX_REACH_FT) } else { NEAR_FT }
}

/// The markups of a PDF page (as [`cadaddy_io::pdf_markups::read`] gives them) mapped onto a
/// plan sheet of the set: each one's point and extent in building feet and the model elements it
/// refers to, nearest first. Where a markup points: a cloud (rectangle, ellipse, polygon, ink,
/// stamp) at its middle; an arrow at its tail (the element) with `to_ft` its head (where it goes);
/// a callout at its leader's tip; a line from its first point to its last; a note at its corner.
/// Page points are mapped to the sheet's paper by its width, so a half-size print maps too.
/// Ids are `MK-<sheet>-<n>` from `first_id`.
pub fn map_markups(truth: &Truth, sheet: &Sheet, page: &MarkupPage, first_id: usize) -> Vec<MappedMarkup> {
    let level = sheet.level.clone().unwrap_or_default();
    // Paper inches per page point.
    let k = if page.width.is_finite() && page.width > 1.0 && sheet.paper_in[0].is_finite() && sheet.paper_in[0] > 0.0 {
        sheet.paper_in[0] / page.width
    } else {
        1.0 / 72.0
    };
    let to_bldg = |p: [f64; 2]| paper_to_building(sheet, [p[0] * k, p[1] * k]);
    let mut out = Vec::new();
    for (i, m) in page.markups.iter().take(MAX_MARKUPS_PER_PAGE).enumerate() {
        let pts: Vec<[f64; 2]> = if m.points.is_empty() { m.strokes.iter().flatten().copied().collect() } else { m.points.clone() };
        let centre = [(m.rect[0] + m.rect[2]) / 2.0, (m.rect[1] + m.rect[3]) / 2.0];
        let (kind, at, to) = match m.kind {
            MarkupKind::Cloud => ("cloud", centre, None),
            MarkupKind::Rectangle => ("rectangle", centre, None),
            MarkupKind::Ellipse => ("ellipse", centre, None),
            MarkupKind::Polygon => ("polygon", centre, None),
            MarkupKind::Ink => ("ink", centre, None),
            MarkupKind::Stamp => ("stamp", centre, None),
            MarkupKind::Highlight => ("highlight", centre, None),
            // The head is the first point: the tail sits on what moves, the head where it goes.
            MarkupKind::Arrow => ("arrow", pts.last().copied().unwrap_or(centre), pts.first().copied()),
            MarkupKind::Callout => ("callout", pts.first().copied().unwrap_or(centre), None),
            MarkupKind::Line => ("line", pts.first().copied().unwrap_or(centre), pts.last().copied()),
            MarkupKind::Polyline => ("polyline", pts.first().copied().unwrap_or(centre), pts.last().copied()),
            MarkupKind::Note => ("note", pts.first().copied().unwrap_or([m.rect[0], m.rect[3]]), None),
        };
        let bounds = {
            let a = to_bldg([m.rect[0], m.rect[1]]);
            let b = to_bldg([m.rect[2], m.rect[3]]);
            [[a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]]
        };
        let bldg = to_bldg(at);
        let text = if m.text.trim().is_empty() { &m.subject } else { &m.text };
        out.push(MappedMarkup {
            id: format!("MK-{}-{}", sheet.number, first_id.saturating_add(i)),
            kind: kind.into(),
            text: truncate(text.trim(), MAX_TEXT),
            author: truncate(&m.author, MAX_TEXT),
            date: truncate(&m.date, 64),
            sheet: sheet.number.clone(),
            paper_pt: [at[0] * k, at[1] * k],
            building_ft: bldg,
            level: level.clone(),
            near_elements: find_near_elements(truth, &level, bldg, reach(kind, bounds)),
            bounds_ft: bounds,
            to_ft: to.map(to_bldg),
        });
    }
    out
}

/// [`map_markups`] for page `page` (1-based) of a PDF: the review markups a person or a tablet
/// app put on a printed sheet.
pub fn read_pdf_markups(truth: &Truth, sheet: &Sheet, pdf: &[u8], page: u32, first_id: usize) -> Result<Vec<MappedMarkup>, String> {
    let p = cadaddy_io::pdf_markups::read(pdf, page)?;
    Ok(map_markups(truth, sheet, &p, first_id))
}

/// Truth element ids within `radius_ft` of a point on a given level, nearest first (at most
/// [`MAX_NEAR`]): segments and conduits by their centreline, walls by theirs, equipment by its
/// box, devices, terminals and fixtures by their point.
pub fn find_near_elements(truth: &Truth, level: &str, at_ft: [f64; 2], radius_ft: f64) -> Vec<String> {
    if !(at_ft[0].is_finite() && at_ft[1].is_finite()) {
        return Vec::new();
    }
    let r2 = radius_ft * radius_ft;
    let mut hits: Vec<(f64, String)> = Vec::new();
    let mut hit = |d2: f64, id: &str| {
        if d2 < r2 {
            hits.push((d2, id.to_string()));
        }
    };
    for s in truth.segments.iter().filter(|s| s.level == level) {
        hit(seg_dist2(at_ft, [s.path_ft[0][0], s.path_ft[0][1]], [s.path_ft[1][0], s.path_ft[1][1]]), &s.id);
    }
    for d in truth.elec_devices.iter().filter(|d| d.level == level) {
        hit(dist2(at_ft, [d.at_ft[0], d.at_ft[1]]), &d.id);
    }
    for e in truth.equipment.iter().filter(|e| e.level == level) {
        let dx = (e.min_ft[0] - at_ft[0]).max(at_ft[0] - e.max_ft[0]).max(0.0);
        let dy = (e.min_ft[1] - at_ft[1]).max(at_ft[1] - e.max_ft[1]).max(0.0);
        hit(dx * dx + dy * dy, &e.id);
    }
    for c in truth.elec_conduits.iter().filter(|c| c.level == level) {
        hit(seg_dist2(at_ft, c.from_ft, c.to_ft), &c.id);
    }
    for t in truth.terminals.iter().filter(|t| t.level == level) {
        hit(dist2(at_ft, [t.at_ft[0], t.at_ft[1]]), &t.id);
    }
    for f in truth.fixtures.iter().filter(|f| f.level == level) {
        hit(dist2(at_ft, [f.at_ft[0], f.at_ft[1]]), &f.id);
    }
    for w in truth.plan_walls.iter().filter(|w| w.level == level) {
        hit(seg_dist2(at_ft, w.a_ft, w.b_ft), &w.id);
    }
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    hits.into_iter().take(MAX_NEAR).map(|(_, id)| id).collect()
}

fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1])
}

/// Squared distance from a point to the segment `a`–`b` (plan).
fn seg_dist2(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-12 {
        return dist2(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
    dist2(p, [a[0] + t * dx, a[1] + t * dy])
}

/// At most `max` bytes of `s`, cut on a character boundary.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------
// Markup → change conversion
// ---------------------------------------------------------------------------

/// What a markup's words ask for, strongest first: a deletion beats a move ("remove" is not
/// "move"), a verification beats both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Intent {
    Delete,
    Verify,
    Reroute,
    Height,
    Move,
    Size,
    Type,
    Add,
    None,
}

/// Lower-case words of a text (letters and digits; everything else separates).
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).take(2_000).collect()
}

/// Whether `ws` holds `phrase` (one or more whole words) in order.
fn has(ws: &[String], phrase: &str) -> bool {
    let p: Vec<&str> = phrase.split(' ').collect();
    !p.is_empty() && ws.windows(p.len()).any(|w| w.iter().zip(&p).all(|(a, b)| a == b))
}

fn any_of(ws: &[String], phrases: &[&str]) -> bool {
    phrases.iter().any(|p| has(ws, p))
}

fn intent(ws: &[String]) -> Intent {
    if any_of(
        ws,
        &[
            "delete",
            "deleted",
            "remove",
            "removed",
            "omit",
            "omitted",
            "eliminate",
            "eliminated",
            "abandon",
            "abandoned",
            "demolish",
            "demolished",
            "not built",
            "not installed",
        ],
    ) {
        Intent::Delete
    } else if any_of(ws, &["verify", "verified", "confirm", "confirmed"]) {
        Intent::Verify
    } else if any_of(ws, &["reroute", "rerouted", "rerouting"]) {
        Intent::Reroute
    } else if any_of(ws, &["raise", "raised", "lower", "lowered"]) {
        Intent::Height
    } else if any_of(ws, &["move", "moved", "relocate", "relocated", "shift", "shifted"]) {
        Intent::Move
    } else if any_of(ws, &["upsize", "upsized", "downsize", "downsized", "resize", "resized"]) {
        Intent::Size
    } else if any_of(ws, &["type", "change to", "changed to", "replace with", "replaced with"]) {
        Intent::Type
    } else if any_of(ws, &["add", "added", "new", "additional"]) {
        Intent::Add
    } else {
        Intent::None
    }
}

/// The element kinds a text names, in the order it names them: `receptacle` → a device, `duct`
/// → a duct run… (`segment:duct` narrows segments to ducts).
fn named_kinds(ws: &[String]) -> Vec<&'static str> {
    const NAMES: &[(&str, &str)] = &[
        ("receptacle", "elec_device"),
        ("receptacles", "elec_device"),
        ("outlet", "elec_device"),
        ("switch", "elec_device"),
        ("switches", "elec_device"),
        ("device", "elec_device"),
        ("light", "elec_device"),
        ("luminaire", "elec_device"),
        ("jbox", "elec_device"),
        ("junction", "elec_device"),
        ("disconnect", "elec_device"),
        ("duct", "segment:duct"),
        ("ductwork", "segment:duct"),
        ("pipe", "segment:pipe"),
        ("piping", "segment:pipe"),
        ("conduit", "elec_conduit"),
        ("raceway", "elec_conduit"),
        ("diffuser", "terminal"),
        ("grille", "terminal"),
        ("register", "terminal"),
        ("louver", "terminal"),
        ("sprinkler", "terminal"),
        ("head", "terminal"),
        ("ahu", "equipment"),
        ("vav", "equipment"),
        ("fan", "equipment"),
        ("pump", "equipment"),
        ("heater", "equipment"),
        ("unit", "equipment"),
        ("equipment", "equipment"),
        ("lavatory", "fixture"),
        ("lav", "fixture"),
        ("sink", "fixture"),
        ("toilet", "fixture"),
        ("wc", "fixture"),
        ("urinal", "fixture"),
        ("shower", "fixture"),
        ("drain", "fixture"),
        ("wall", "wall"),
        ("partition", "wall"),
        ("stud", "wall"),
    ];
    let mut out = Vec::new();
    for w in ws {
        if let Some((_, k)) = NAMES.iter().find(|(n, _)| n == w)
            && !out.contains(k)
        {
            out.push(*k);
        }
    }
    out
}

/// Whether element `id` is of a named kind (`segment:duct` → a duct segment).
fn is_kind(truth: &Truth, id: &str, named: &str) -> bool {
    match named.split_once(':') {
        Some(("segment", k)) => truth.segments.iter().any(|s| s.id == id && s.kind == k),
        _ => element_kind_of(truth, id) == named,
    }
}

/// The first length in a text, feet: `11'-6"`, `11' 6"`, `11'`, `6"`, `10.5 ft`, `18 in`,
/// `2 feet` (`ws`: the text's [`words`]).
fn length_at(ws: &[String], raw: &str) -> Option<f64> {
    // Feet-and-inches as printed (quotes are separators to `words`, so read the raw text).
    let t = raw.replace(['\u{2019}', '\u{2032}'], "'").replace(['\u{201d}', '\u{2033}'], "\"");
    let b = t.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b.get(i).is_some_and(|c| c.is_ascii_digit()) && (i == 0 || !b.get(i - 1).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'.')) {
            let start = i;
            while b.get(i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                i += 1;
            }
            let v: f64 = t.get(start..i)?.parse().ok()?;
            let rest = t.get(i..).unwrap_or_default().trim_start();
            if let Some(after) = rest.strip_prefix('\'') {
                // Feet, then perhaps inches: `-6"`, ` 6"`.
                let after = after.trim_start().trim_start_matches('-').trim_start();
                let digits: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                let inches = if !digits.is_empty() && after.get(digits.len()..).is_some_and(|r| r.trim_start().starts_with('"')) {
                    digits.parse::<f64>().unwrap_or(0.0)
                } else {
                    0.0
                };
                return Some(v + inches / 12.0).filter(|x| x.is_finite());
            }
            if rest.starts_with('"') {
                return Some(v / 12.0).filter(|x| x.is_finite());
            }
            continue;
        }
        i += 1;
    }
    // Spelled units: `2 feet`, `18 in`.
    for w in ws.windows(2) {
        let (Some(n), Some(u)) = (w.first(), w.get(1)) else { continue };
        let Ok(v) = n.parse::<f64>() else { continue };
        let f = match u.as_str() {
            "ft" | "feet" | "foot" => v,
            "in" | "inch" | "inches" => v / 12.0,
            _ => continue,
        };
        return Some(f).filter(|x| x.is_finite());
    }
    None
}

/// A displacement a text gives: `2 feet north`, `3' east`, `18" up`, building feet.
fn displacement(ws: &[String], raw: &str) -> Option<[f64; 3]> {
    let d = length_at(ws, raw)?;
    let dir = ws.iter().find_map(|w| match w.as_str() {
        "north" => Some([0.0, 1.0, 0.0]),
        "south" => Some([0.0, -1.0, 0.0]),
        "east" => Some([1.0, 0.0, 0.0]),
        "west" => Some([-1.0, 0.0, 0.0]),
        "up" => Some([0.0, 0.0, 1.0]),
        "down" => Some([0.0, 0.0, -1.0]),
        _ => None,
    })?;
    Some([dir[0] * d, dir[1] * d, dir[2] * d])
}

/// The height above its floor an element hangs or is mounted at, feet, when the model keeps one
/// (duct and pipe runs, devices).
fn height_of(truth: &Truth, id: &str) -> Option<f64> {
    if let Some(s) = truth.segments.iter().find(|s| s.id == id) {
        return s.height.as_ref().map(|h| h.centre_ft);
    }
    truth.elec_devices.iter().find(|d| d.id == id).map(|d| d.mounting_in / 12.0)
}

/// Heuristics: interpret a markup's text to create an `AsBuiltChange` (status `Pending`).
///
/// The element is the nearest one the markup refers to, preferring the kind its text names
/// ("receptacle", "duct", "diffuser"…). Words are matched whole, deletions before moves. A move
/// goes by the distance the text gives ("2 feet north"), else to an arrow's head, else to the
/// middle of a cloud drawn away from the element; a height change needs its value ("raise to
/// 11'-6"", "lower 6""). What has no value to apply ("rerouted", "raise conduit", "relocate") is
/// kept as a `Note` that says what value it needs, never as made-up geometry. A move keeps the
/// element's height.
pub fn markup_to_change(markup: &MappedMarkup, truth: &Truth, change_id: &str) -> Option<AsBuiltChange> {
    let ws = words(&markup.text);
    let named = named_kinds(&ws);
    let element_id =
        named.iter().find_map(|k| markup.near_elements.iter().find(|id| is_kind(truth, id, k))).or_else(|| markup.near_elements.first())?.clone();
    let element_kind = element_kind_of(truth, &element_id);
    let pos = element_position(truth, &element_id);
    let area = is_area(&markup.kind);
    let mut notes = String::new();
    let mut needs = |what: &str| {
        notes = format!("needs value: {what}");
        ChangeKind::Note
    };
    let it = intent(&ws);
    // An arrow moves what its tail is on to its head; a line does when its words say move.
    let pointed = markup.to_ft.is_some() && (markup.kind == "arrow" || it == Intent::Move);
    let kind = match it {
        Intent::Delete => ChangeKind::Deleted,
        Intent::Verify => match pos {
            Some(p) => ChangeKind::FieldVerified { at_ft: p },
            None => needs("where it was verified"),
        },
        Intent::Reroute => needs("the new route (record it with asbuilt.record kind reroute and path)"),
        Intent::Height => {
            let lower = any_of(&ws, &["lower", "lowered"]);
            let to_value = has(&ws, "to") || has(&ws, "at");
            match (height_of(truth, &element_id), length_at(&ws, &markup.text)) {
                (Some(h), Some(v)) => {
                    let new = if to_value {
                        v
                    } else if lower {
                        h - v
                    } else {
                        h + v
                    };
                    ChangeKind::HeightChanged { design_ft: h, asbuilt_ft: new }
                }
                (None, _) => needs("a height; the model keeps none for this element"),
                (Some(_), None) => needs("the new height (\"raise to 11'-6\\\"\")"),
            }
        }
        Intent::Move | Intent::None if pointed => match (pos, displacement(&ws, &markup.text), markup.to_ft) {
            (Some(p), Some(d), _) => ChangeKind::Moved { from_ft: p, to_ft: [p[0] + d[0], p[1] + d[1], p[2] + d[2]] },
            (Some(p), None, Some(t)) => ChangeKind::Moved { from_ft: p, to_ft: [t[0], t[1], p[2]] },
            _ => needs("where it went"),
        },
        Intent::Move => match (pos, displacement(&ws, &markup.text)) {
            (Some(p), Some(d)) => ChangeKind::Moved { from_ft: p, to_ft: [p[0] + d[0], p[1] + d[1], p[2] + d[2]] },
            // A cloud drawn round the new spot, away from where the element was.
            (Some(p), None) if area && dist2([p[0], p[1]], markup.building_ft) >= 0.25 => {
                ChangeKind::Moved { from_ft: p, to_ft: [markup.building_ft[0], markup.building_ft[1], p[2]] }
            }
            _ => needs("where it went (\"2 feet north\", or an arrow to the new spot)"),
        },
        Intent::Size => needs("the new size (record it with asbuilt.record kind size)"),
        Intent::Type => ChangeKind::TypeChanged { design_type: design_type_of(truth, &element_id), asbuilt_type: markup.text.clone() },
        Intent::Add => ChangeKind::Added,
        Intent::None => ChangeKind::Note,
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
        notes,
        level: markup.level.clone(),
        at_ft: markup.building_ft,
        markup_id: Some(markup.id.clone()),
    })
}

/// The type an element is designed as (a device's or terminal's schedule type, a conduit's).
fn design_type_of(truth: &Truth, id: &str) -> String {
    if let Some(d) = truth.elec_devices.iter().find(|d| d.id == id) {
        return d.type_tag.clone();
    }
    if let Some(t) = truth.terminals.iter().find(|t| t.id == id) {
        return t.type_tag.clone();
    }
    if let Some(c) = truth.elec_conduits.iter().find(|c| c.id == id) {
        return c.conduit_type.clone();
    }
    if let Some(w) = truth.plan_walls.iter().find(|w| w.id == id) {
        return w.wall_type.clone();
    }
    String::new()
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
    } else if truth.plan_walls.iter().any(|w| w.id == id) {
        "wall".into()
    } else if truth.nodes.iter().any(|n| n.id == id) {
        "node".into()
    } else {
        "unknown".into()
    }
}

/// A level's floor elevation, feet.
fn level_elevation(truth: &Truth, level: &str) -> f64 {
    truth.building.levels.iter().find(|l| l.code == level || l.name == level).map_or(0.0, |l| l.elevation_ft)
}

/// The 3D position of a truth element by id (building feet; heights as the model keeps them).
/// A conduit carries no height in the model: it runs at the ceiling of the room it is in, else on
/// its floor. A wall stands on its floor.
pub fn element_position(truth: &Truth, id: &str) -> Option<[f64; 3]> {
    if let Some(s) = truth.segments.iter().find(|s| s.id == id) {
        return Some([
            (s.path_ft[0][0] + s.path_ft[1][0]) * 0.5,
            (s.path_ft[0][1] + s.path_ft[1][1]) * 0.5,
            (s.path_ft[0][2] + s.path_ft[1][2]) * 0.5,
        ]);
    }
    if let Some(d) = truth.elec_devices.iter().find(|d| d.id == id) {
        return Some(d.at_ft);
    }
    if let Some(e) = truth.equipment.iter().find(|e| e.id == id) {
        return Some([(e.min_ft[0] + e.max_ft[0]) * 0.5, (e.min_ft[1] + e.max_ft[1]) * 0.5, (e.min_ft[2] + e.max_ft[2]) * 0.5]);
    }
    if let Some(c) = truth.elec_conduits.iter().find(|c| c.id == id) {
        let mid = [(c.from_ft[0] + c.to_ft[0]) * 0.5, (c.from_ft[1] + c.to_ft[1]) * 0.5];
        let floor = level_elevation(truth, &c.level);
        let ceiling = truth
            .building
            .rooms
            .iter()
            .filter(|r| r.level == c.level)
            .find(|r| mid[0] >= r.min_ft[0] && mid[0] <= r.max_ft[0] && mid[1] >= r.min_ft[1] && mid[1] <= r.max_ft[1])
            .and_then(|r| r.ceiling.as_ref())
            .map_or(0.0, |cl| cl.height_ft);
        return Some([mid[0], mid[1], floor + ceiling]);
    }
    if let Some(t) = truth.terminals.iter().find(|t| t.id == id) {
        return Some(t.at_ft);
    }
    if let Some(f) = truth.fixtures.iter().find(|f| f.id == id) {
        return Some(f.at_ft);
    }
    if let Some(w) = truth.plan_walls.iter().find(|w| w.id == id) {
        return Some([(w.a_ft[0] + w.b_ft[0]) * 0.5, (w.a_ft[1] + w.b_ft[1]) * 0.5, level_elevation(truth, &w.level)]);
    }
    if let Some(n) = truth.nodes.iter().find(|n| n.id == id) {
        return Some(n.at_ft);
    }
    None
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
                let d = ((to_ft[0] - from_ft[0]).powi(2) + (to_ft[1] - from_ft[1]).powi(2) + (to_ft[2] - from_ft[2]).powi(2)).sqrt();
                Deviation {
                    element_id: change.element_id.clone(),
                    element_kind: change.element_kind.clone(),
                    description: format!("Moved {:.1}' from design position ({})", d, change.reason),
                    level: change.level.clone(),
                    at_ft: change.at_ft,
                    magnitude: d,
                    change_id: Some(change.id.clone()),
                }
            }
            ChangeKind::Rerouted { design_path_ft, asbuilt_path_ft } => {
                let d = ((asbuilt_path_ft[0][0] - design_path_ft[0][0]).powi(2) + (asbuilt_path_ft[0][1] - design_path_ft[0][1]).powi(2)).sqrt();
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
                description: format!("Type changed: {} -> {} ({})", design_type, asbuilt_type, change.reason),
                level: change.level.clone(),
                at_ft: change.at_ft,
                magnitude: 1.0,
                change_id: Some(change.id.clone()),
            },
            ChangeKind::SizeChanged { design_size, asbuilt_size } => Deviation {
                element_id: change.element_id.clone(),
                element_kind: change.element_kind.clone(),
                description: format!("Size changed: {} -> {} ({})", design_size, asbuilt_size, change.reason),
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
                description: format!("Height changed: {} -> {} ({})", truth::feet_inches(*design_ft), truth::feet_inches(*asbuilt_ft), change.reason),
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

/// Whether a change is applied to the as-built model: decided ones (field decision, RFI answer,
/// change order) always, `Pending` ones only when asked, `Rejected` never.
pub fn applies(status: ChangeStatus, include_pending: bool) -> bool {
    match status {
        ChangeStatus::FieldDecision | ChangeStatus::RfiAnswer | ChangeStatus::ChangeOrder => true,
        ChangeStatus::Pending => include_pending,
        ChangeStatus::Rejected => false,
    }
}

/// The as-built copy of the truth: the decided changes applied (not `Pending` ones: nothing an
/// unreviewed markup says changes the model). See [`asbuilt_truth_with`].
pub fn asbuilt_truth(design: &Truth, record: &AsBuiltRecord) -> Truth {
    asbuilt_truth_with(design, record, false)
}

/// Apply the changes [`applies`] picks to a copy of the truth. Elements with `Moved` changes get
/// new positions; `Deleted` elements are removed; `Added` elements are noted but not created
/// (they have no truth definition); notes and verifications change nothing.
pub fn asbuilt_truth_with(design: &Truth, record: &AsBuiltRecord, include_pending: bool) -> Truth {
    let mut t = design.clone();
    for change in &record.changes {
        if !applies(change.status, include_pending) {
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
                if let Some(c) = t.elec_conduits.iter_mut().find(|c| c.id == change.element_id) {
                    c.from_ft = [asbuilt_path_ft[0][0], asbuilt_path_ft[0][1]];
                    c.to_ft = [asbuilt_path_ft[1][0], asbuilt_path_ft[1][1]];
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
                    seg.path_ft[0][2] += delta;
                    seg.path_ft[1][2] += delta;
                }
                if let Some(d) = t.elec_devices.iter_mut().find(|d| d.id == change.element_id) {
                    let delta = asbuilt_ft - d.mounting_in / 12.0;
                    d.mounting_in = asbuilt_ft * 12.0;
                    d.at_ft[2] += delta;
                }
            }
            ChangeKind::FieldVerified { .. } | ChangeKind::Added | ChangeKind::Note => {}
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
        "wall" => {
            if let Some(w) = t.plan_walls.iter_mut().find(|w| w.id == id) {
                let dx = to[0] - (w.a_ft[0] + w.b_ft[0]) * 0.5;
                let dy = to[1] - (w.a_ft[1] + w.b_ft[1]) * 0.5;
                w.a_ft = [w.a_ft[0] + dx, w.a_ft[1] + dy];
                w.b_ft = [w.b_ft[0] + dx, w.b_ft[1] + dy];
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
        "wall" => {
            if let Some(w) = t.plan_walls.iter_mut().find(|w| w.id == id) {
                w.wall_type = new_type.into();
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
        "wall" => t.plan_walls.retain(|w| w.id != id),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// As-built drawing helpers
// ---------------------------------------------------------------------------

/// The as-built marks for a level: one per change that isn't rejected (pending ones too, labelled
/// `PENDING`), as [`asbuilt_sheet`] draws them on layer `ASBUILT-CHANGES`:
/// - Moved: arrow from old to new.
/// - Deleted: X through the element.
/// - Added: circle at position.
/// - Type/size/height change: diamond at position.
/// - Field verified: checkmark at position.
/// - Rerouted: the new path.
/// - Note: text at position.
pub fn asbuilt_overlay(record: &AsBuiltRecord, level: &str) -> Vec<AsBuiltMark> {
    let mut marks = Vec::new();
    for change in &record.changes {
        if change.level != level || change.status == ChangeStatus::Rejected {
            continue;
        }
        let symbol = match &change.kind {
            ChangeKind::Moved { from_ft, to_ft } => MarkSymbol::Arrow { from: [from_ft[0], from_ft[1]], to: [to_ft[0], to_ft[1]] },
            ChangeKind::Rerouted { design_path_ft, asbuilt_path_ft } => MarkSymbol::Reroute {
                old_from: [design_path_ft[0][0], design_path_ft[0][1]],
                old_to: [design_path_ft[1][0], design_path_ft[1][1]],
                new_from: [asbuilt_path_ft[0][0], asbuilt_path_ft[0][1]],
                new_to: [asbuilt_path_ft[1][0], asbuilt_path_ft[1][1]],
            },
            ChangeKind::TypeChanged { .. } | ChangeKind::SizeChanged { .. } => MarkSymbol::Diamond(change.at_ft),
            ChangeKind::Added => MarkSymbol::Circle(change.at_ft),
            ChangeKind::Deleted => MarkSymbol::Cross(change.at_ft),
            ChangeKind::FieldVerified { at_ft } => MarkSymbol::Check([at_ft[0], at_ft[1]]),
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
    Reroute { old_from: [f64; 2], old_to: [f64; 2], new_from: [f64; 2], new_to: [f64; 2] },
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
// As-built sheets
// ---------------------------------------------------------------------------

/// The layer an as-built sheet draws its change marks on (one object per mark).
pub const LAYER_CHANGES: &str = "ASBUILT-CHANGES";
/// Each mark's label (kind, status, change id).
pub const LAYER_LABELS: &str = "ASBUILT-LABELS";
/// The status legend.
pub const LAYER_LEGEND: &str = "ASBUILT-LEGEND";
/// The design, as drawn under the marks.
pub const LAYER_DESIGN_WALLS: &str = "ASBUILT-DESIGN-WALLS";
pub const LAYER_DESIGN_MEP: &str = "ASBUILT-DESIGN-MEP";
pub const LAYER_DESIGN_ELEC: &str = "ASBUILT-DESIGN-ELEC";
/// Most design objects one sheet draws.
const MAX_SHEET_OBJECTS: usize = 200_000;

/// A status's colour on the sheet (ACI) and its legend words.
fn status_style(s: ChangeStatus) -> (u8, &'static str) {
    match s {
        ChangeStatus::Pending => (2, "PENDING - not reviewed, not in the as-built model"),
        ChangeStatus::FieldDecision => (3, "FIELD - field decision"),
        ChangeStatus::RfiAnswer => (4, "RFI - answer to an RFI"),
        ChangeStatus::ChangeOrder => (5, "CO - change order"),
        ChangeStatus::Rejected => (1, "REJ - rejected"),
    }
}

fn pv(x: f64, y: f64) -> PolyVertex {
    PolyVertex { p: Vec2::new(x, y), bulge: 0.0, start_width: 0.0, end_width: 0.0 }
}

fn put(d: &mut Drawing, layer: &str, color: Option<u8>, kind: EntityKind) {
    let common = Common { layer: layer.into(), color: color.map_or(Color::ByLayer, Color::Index), ..Common::default() };
    let _ = d.add(&Space::Model, common, kind);
}

fn pline(d: &mut Drawing, layer: &str, color: Option<u8>, pts: &[[f64; 2]], closed: bool) {
    let vertices = pts.iter().map(|p| pv(p[0], p[1])).collect();
    put(d, layer, color, EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: false }));
}

fn label(d: &mut Drawing, layer: &str, color: Option<u8>, at: [f64; 2], h: f64, s: &str) {
    let p = Vec3::new(at[0], at[1], 0.0);
    put(
        d,
        layer,
        color,
        EntityKind::Text(Text {
            insert: p,
            align_pt: Some(p),
            height: h,
            value: truncate(s, 250),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::Left,
            valign: VAlign::Middle,
        }),
    );
}

/// An as-built sheet for a level (drawing inches, building feet × 12): the design on
/// `ASBUILT-DESIGN-*` layers (walls, duct and pipe runs, equipment, terminals, fixtures, conduits,
/// devices), one mark per change that isn't rejected on [`LAYER_CHANGES`] coloured by its status,
/// each mark's label on [`LAYER_LABELS`], and a status legend on [`LAYER_LEGEND`].
pub fn asbuilt_sheet(design: &Truth, record: &AsBuiltRecord, level: &str) -> Drawing {
    let mut d = Drawing::new_imperial();
    for (name, color) in
        [(LAYER_DESIGN_WALLS, 8u8), (LAYER_DESIGN_MEP, 9), (LAYER_DESIGN_ELEC, 9), (LAYER_CHANGES, 6), (LAYER_LABELS, 7), (LAYER_LEGEND, 7)]
    {
        let mut l = Layer::new(name);
        l.color = Color::Index(color);
        d.layers.push(l);
    }
    let i = |p: [f64; 2]| [p[0] * 12.0, p[1] * 12.0];
    let mut n = 0usize;
    let mut more = || {
        n += 1;
        n <= MAX_SHEET_OBJECTS
    };
    for w in design.plan_walls.iter().filter(|w| w.level == level) {
        if more() {
            pline(&mut d, LAYER_DESIGN_WALLS, None, &[i(w.a_ft), i(w.b_ft)], false);
        }
    }
    for s in design.segments.iter().filter(|s| s.level == level && !s.vertical) {
        if more() {
            pline(&mut d, LAYER_DESIGN_MEP, None, &[i([s.path_ft[0][0], s.path_ft[0][1]]), i([s.path_ft[1][0], s.path_ft[1][1]])], false);
        }
    }
    for e in design.equipment.iter().filter(|e| e.level == level) {
        if more() {
            let (a, b) = (i([e.min_ft[0], e.min_ft[1]]), i([e.max_ft[0], e.max_ft[1]]));
            pline(&mut d, LAYER_DESIGN_MEP, None, &[a, [b[0], a[1]], b, [a[0], b[1]]], true);
        }
    }
    for t in design.terminals.iter().filter(|t| t.level == level) {
        if more() {
            let c = i([t.at_ft[0], t.at_ft[1]]);
            let h = (t.face_in[0].max(t.face_in[1]) / 2.0).clamp(3.0, 36.0);
            pline(&mut d, LAYER_DESIGN_MEP, None, &[[c[0] - h, c[1] - h], [c[0] + h, c[1] - h], [c[0] + h, c[1] + h], [c[0] - h, c[1] + h]], true);
        }
    }
    for f in design.fixtures.iter().filter(|f| f.level == level) {
        if more() {
            put(
                &mut d,
                LAYER_DESIGN_MEP,
                None,
                EntityKind::Circle(Circle { center: Vec3::new(f.at_ft[0] * 12.0, f.at_ft[1] * 12.0, 0.0), radius: 9.0 }),
            );
        }
    }
    for c in design.elec_conduits.iter().filter(|c| c.level == level) {
        if more() {
            pline(&mut d, LAYER_DESIGN_ELEC, None, &[i(c.from_ft), i(c.to_ft)], false);
        }
    }
    for e in design.elec_devices.iter().filter(|e| e.level == level) {
        if more() {
            put(
                &mut d,
                LAYER_DESIGN_ELEC,
                None,
                EntityKind::Circle(Circle { center: Vec3::new(e.at_ft[0] * 12.0, e.at_ft[1] * 12.0, 0.0), radius: 4.0 }),
            );
        }
    }
    // The marks: one object each.
    let r = 12.0;
    let marks = asbuilt_overlay(record, level);
    for m in &marks {
        let (color, _) = status_style(m.status);
        let c = Some(color);
        let at = match &m.symbol {
            MarkSymbol::Arrow { from, to } => {
                let (a, b) = (i(*from), i(*to));
                let len = (b[0] - a[0]).hypot(b[1] - a[1]);
                let u = if len > 1e-9 { [(b[0] - a[0]) / len, (b[1] - a[1]) / len] } else { [1.0, 0.0] };
                let back = [b[0] - u[0] * r, b[1] - u[1] * r];
                let w1 = [back[0] - u[1] * r / 2.0, back[1] + u[0] * r / 2.0];
                let w2 = [back[0] + u[1] * r / 2.0, back[1] - u[0] * r / 2.0];
                pline(&mut d, LAYER_CHANGES, c, &[a, b, w1, b, w2], false);
                b
            }
            MarkSymbol::Reroute { new_from, new_to, .. } => {
                pline(&mut d, LAYER_CHANGES, c, &[i(*new_from), i(*new_to)], false);
                i(*new_to)
            }
            MarkSymbol::Diamond(p) => {
                let q = i(*p);
                pline(&mut d, LAYER_CHANGES, c, &[[q[0], q[1] - r], [q[0] + r, q[1]], [q[0], q[1] + r], [q[0] - r, q[1]]], true);
                q
            }
            MarkSymbol::Circle(p) => {
                let q = i(*p);
                put(&mut d, LAYER_CHANGES, c, EntityKind::Circle(Circle { center: Vec3::new(q[0], q[1], 0.0), radius: r }));
                q
            }
            MarkSymbol::Cross(p) => {
                let q = i(*p);
                let k = |dx: f64, dy: f64| [q[0] + dx * r, q[1] + dy * r];
                pline(&mut d, LAYER_CHANGES, c, &[k(-1.0, -1.0), q, k(1.0, -1.0), q, k(1.0, 1.0), q, k(-1.0, 1.0)], false);
                q
            }
            MarkSymbol::Check(p) => {
                let q = i(*p);
                pline(&mut d, LAYER_CHANGES, c, &[[q[0] - r, q[1]], [q[0] - r / 3.0, q[1] - r * 0.7], [q[0] + r, q[1] + r]], false);
                q
            }
            MarkSymbol::Text(p, t) => {
                let q = i(*p);
                label(&mut d, LAYER_CHANGES, c, q, 6.0, t);
                q
            }
        };
        label(&mut d, LAYER_LABELS, c, [at[0] + r * 1.5, at[1] + r], 6.0, &format!("{} {}", m.label, m.change_id));
    }
    // The legend, right of everything drawn.
    let (mut right, mut top) = (f64::MIN, f64::MIN);
    for e in d.model.iter() {
        let pts: Vec<[f64; 2]> = match &e.kind {
            EntityKind::LwPolyline(p) => p.vertices.iter().map(|v| [v.p.x, v.p.y]).collect(),
            EntityKind::Circle(c) => vec![[c.center.x + c.radius, c.center.y + c.radius]],
            EntityKind::Text(t) => vec![[t.insert.x, t.insert.y]],
            _ => Vec::new(),
        };
        for p in pts.into_iter().filter(|p| p[0].is_finite() && p[1].is_finite()) {
            right = right.max(p[0]);
            top = top.max(p[1]);
        }
    }
    let (x, mut y) = if right > f64::MIN && top > f64::MIN { (right + 48.0, top) } else { (0.0, 0.0) };
    label(&mut d, LAYER_LEGEND, None, [x, y], 9.0, &format!("AS-BUILT CHANGES - LEVEL {level} ({} marked)", marks.len()));
    for st in [ChangeStatus::Pending, ChangeStatus::FieldDecision, ChangeStatus::RfiAnswer, ChangeStatus::ChangeOrder] {
        y -= 18.0;
        let (color, words) = status_style(st);
        pline(&mut d, LAYER_LEGEND, Some(color), &[[x, y], [x + 24.0, y]], false);
        label(&mut d, LAYER_LEGEND, Some(color), [x + 30.0, y], 6.0, words);
    }
    for line in [
        "ARROW: moved (old to new)",
        "X: deleted, not built",
        "CIRCLE: added in the field",
        "DIAMOND: type, size or height changed",
        "CHECK: field verified",
        "LINE: rerouted (new path)",
        "TEXT: note",
    ] {
        y -= 12.0;
        label(&mut d, LAYER_LEGEND, None, [x, y], 5.0, line);
    }
    d
}

/// One as-built sheet as a file.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetFile {
    pub level: String,
    /// `ASBUILT-<level>.dxf`.
    pub name: String,
    pub dxf: Vec<u8>,
    /// Change marks on it.
    pub marks: usize,
}

/// The as-built sheet of each of the building's levels ([`asbuilt_sheet`]) as a DXF file.
pub fn asbuilt_sheet_files(design: &Truth, record: &AsBuiltRecord) -> Vec<SheetFile> {
    design
        .building
        .levels
        .iter()
        .map(|l| {
            let d = asbuilt_sheet(design, record, &l.code);
            let safe: String = l.code.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(40).collect();
            SheetFile {
                level: l.code.clone(),
                name: format!("ASBUILT-{safe}.dxf"),
                dxf: cadaddy_io::write_dxf(&d).into_bytes(),
                marks: asbuilt_overlay(record, &l.code).len(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// As-built IFC
// ---------------------------------------------------------------------------

/// What an as-built IFC holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AsBuiltIfc {
    /// Elements written.
    pub elements: usize,
    /// Elements carrying a `CaDaddy_AsBuilt` property set (by id).
    pub changed: Vec<String>,
    /// Changed elements not in the as-built model (deleted, or ids the model doesn't have).
    pub not_written: Vec<String>,
}

/// A prism over the plan quadrilateral `base` (inches) from `z0` to `z1`.
fn prism(base: [[f64; 2]; 4], z0: f64, z1: f64) -> Mesh {
    let mut vertices: Vec<Vec3> = base.iter().map(|p| Vec3::new(p[0], p[1], z0)).collect();
    vertices.extend(base.iter().map(|p| Vec3::new(p[0], p[1], z1)));
    let f = |a: u32, b: u32, c: u32, e: u32| MeshFace { v: vec![a, b, c, e], hidden: 0 };
    Mesh { vertices, faces: vec![f(0, 3, 2, 1), f(4, 5, 6, 7), f(0, 1, 5, 4), f(1, 2, 6, 5), f(2, 3, 7, 6), f(3, 0, 4, 7)], ..Mesh::default() }
}

/// A box round a run from `a` to `b` (plan feet), `half` feet either side, `z0`–`z1` feet.
fn run_box(a: [f64; 2], b: [f64; 2], half: f64, z0: f64, z1: f64) -> Mesh {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let u = if len > 1e-9 { [(b[0] - a[0]) / len, (b[1] - a[1]) / len] } else { [1.0, 0.0] };
    let n = [-u[1] * half, u[0] * half];
    let (a, b) = if len > 1e-9 { (a, b) } else { ([a[0] - half, a[1]], [a[0] + half, a[1]]) };
    let q = |p: [f64; 2], k: f64| [(p[0] + n[0] * k) * 12.0, (p[1] + n[1] * k) * 12.0];
    prism([q(a, -1.0), q(b, -1.0), q(b, 1.0), q(a, 1.0)], z0 * 12.0, z1.max(z0 + 0.01) * 12.0)
}

/// A box round a point (feet), `half` feet across in plan and `hz` up and down.
fn point_box(p: [f64; 3], half: f64, hz: f64) -> Mesh {
    run_box([p[0] - half, p[1]], [p[0] + half, p[1]], half, p[2] - hz, p[2] + hz)
}

/// The as-built model as IFC: every element of the as-built truth ([`asbuilt_truth_with`]) as a
/// body, named by its id, and on each element a change touches (the latest one that isn't
/// rejected) an `IfcPropertySet` `CaDaddy_AsBuilt` with [`asbuilt_ifc_properties`] and the
/// element's id (`AsBuiltElementId`). Heights: the level's elevation plus what the model keeps.
pub fn asbuilt_ifc(design: &Truth, record: &AsBuiltRecord, include_pending: bool, name: &str) -> Result<(Vec<u8>, AsBuiltIfc), String> {
    let t = asbuilt_truth_with(design, record, include_pending);
    // The latest change on each element.
    let mut latest: BTreeMap<&str, &AsBuiltChange> = BTreeMap::new();
    for c in record.changes.iter().filter(|c| c.status != ChangeStatus::Rejected && !c.element_id.is_empty()) {
        latest.insert(c.element_id.as_str(), c);
    }
    let elev = |level: &str| level_elevation(&t, level);
    let mut bodies: Vec<(String, Mesh)> = Vec::new();
    for s in &t.segments {
        let half = (s.size_in[0].max(s.size_in[1]) / 24.0).max(0.1);
        let (a, b) = ([s.path_ft[0][0], s.path_ft[0][1]], [s.path_ft[1][0], s.path_ft[1][1]]);
        let (z0, z1) = (s.path_ft[0][2].min(s.path_ft[1][2]), s.path_ft[0][2].max(s.path_ft[1][2]));
        bodies.push((s.id.clone(), run_box(a, b, half, z0 - half, z1 + half)));
    }
    for c in &t.elec_conduits {
        let half = (c.size_in / 24.0).max(0.05);
        let z = element_position(&t, &c.id).map_or(elev(&c.level), |p| p[2]);
        bodies.push((c.id.clone(), run_box(c.from_ft, c.to_ft, half, z - half, z + half)));
    }
    for w in &t.plan_walls {
        let z = elev(&w.level);
        bodies.push((w.id.clone(), run_box(w.a_ft, w.b_ft, 0.25, z, z + w.height_ft.max(1.0))));
    }
    for e in &t.equipment {
        let base = [[e.min_ft[0], e.min_ft[1]], [e.max_ft[0], e.min_ft[1]], [e.max_ft[0], e.max_ft[1]], [e.min_ft[0], e.max_ft[1]]]
            .map(|p| [p[0] * 12.0, p[1] * 12.0]);
        bodies.push((e.id.clone(), prism(base, e.min_ft[2] * 12.0, e.max_ft[2].max(e.min_ft[2] + 0.1) * 12.0)));
    }
    for d in &t.elec_devices {
        bodies.push((d.id.clone(), point_box(d.at_ft, 0.25, 0.25)));
    }
    for tm in &t.terminals {
        bodies.push((tm.id.clone(), point_box(tm.at_ft, (tm.face_in[0].max(tm.face_in[1]) / 24.0).max(0.25), 0.1)));
    }
    for f in &t.fixtures {
        bodies.push((f.id.clone(), point_box(f.at_ft, 1.0, 0.75)));
    }
    let mut d = Drawing::new_imperial();
    let mut report = AsBuiltIfc::default();
    for (id, mesh) in bodies.into_iter().take(MAX_SHEET_OBJECTS) {
        if mesh.vertices.iter().any(|v| !(v.x.is_finite() && v.y.is_finite() && v.z.is_finite())) {
            continue;
        }
        let mut xdata = vec![RawTag { code: 1001, value: cadaddy_io::ifc::ASBUILT_APP.into() }, RawTag { code: 1000, value: truncate(&id, 200) }];
        if let Some(c) = latest.get(id.as_str()) {
            xdata.push(RawTag { code: 1000, value: format!("AsBuiltElementId={}", truncate(&id, 200)) });
            for (k, v) in asbuilt_ifc_properties(c) {
                xdata.push(RawTag { code: 1000, value: format!("{k}={}", truncate(&v.replace(['\r', '\n'], " "), 240)) });
            }
            report.changed.push(id.clone());
        }
        let common = Common { layer: "ASBUILT-MODEL".into(), xdata: xdata.into(), ..Common::default() };
        d.add(&Space::Model, common, EntityKind::Mesh(mesh)).map_err(|e| e.to_string())?;
        report.elements += 1;
    }
    report.not_written = latest.keys().filter(|id| !report.changed.iter().any(|c| c == *id)).map(|id| id.to_string()).collect();
    let bytes = cadaddy_io::ifc::write_ifc(&d, name).map_err(|e| e.to_string())?;
    Ok((bytes, report))
}

// ---------------------------------------------------------------------------
// Field-verified studs (framing, Q303)
// ---------------------------------------------------------------------------

/// Who records a change and why.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recorded {
    pub id: String,
    pub status: ChangeStatus,
    pub author: String,
    pub date: String,
    pub reason: String,
}

/// The change recording a stud measured on wall `wall_id` (its centre `centre_in` from the `a`
/// end): kind `FieldVerified` at the stud's plan point on that wall, on its floor.
pub fn stud_change(fr: &Framing, wall_id: &str, centre_in: f64, snap: &FieldSnap, who: &Recorded) -> Result<AsBuiltChange, String> {
    let w = fr.walls.iter().find(|w| w.wall.id == wall_id).ok_or_else(|| format!("no framed wall {wall_id}"))?;
    let at = w.wall.at(centre_in);
    let matches = match snap.matches {
        Some(e) => format!("laid out from the {} end", e.letter()),
        None => format!("neither layout ({} off)", framing::inch(snap.off_in)),
    };
    Ok(AsBuiltChange {
        id: who.id.clone(),
        element_id: wall_id.into(),
        element_kind: "wall".into(),
        kind: ChangeKind::FieldVerified { at_ft: [at[0], at[1], 0.0] },
        status: who.status,
        author: truncate(&who.author, MAX_TEXT),
        date: truncate(&who.date, 64),
        reason: truncate(&who.reason, MAX_TEXT),
        notes: format!("stud centre {} from the a end: {matches}", framing::inch(centre_in)),
        level: w.wall.level.clone(),
        at_ft: at,
        markup_id: None,
    })
}

/// A measured stud into the framing and the record: the wall's run (and the walls stacked with
/// it) snaps to field verified ([`Framing::field_point`]) and a `FieldVerified` change on the wall
/// is recorded. Nothing changes when the stud can't be placed.
pub fn verify_stud(
    record: &mut AsBuiltRecord,
    fr: &mut Framing,
    wall_id: &str,
    centre_in: f64,
    who: &Recorded,
) -> Result<(AsBuiltChange, FieldSnap), String> {
    let i = fr.walls.iter().position(|w| w.wall.id == wall_id).ok_or_else(|| format!("no framed wall {wall_id}"))?;
    let mut trial = fr.clone();
    let snap = trial.field_point(i, centre_in)?;
    let change = stud_change(&trial, wall_id, centre_in, &snap, who)?;
    if !record.add_change(change.clone()) {
        return Err(format!("the record holds {MAX_CHANGES} changes; that is the most it keeps"));
    }
    *fr = trial;
    Ok((change, snap))
}

/// The recorded studs again, on a framing rebuilt from the prints: every wall `FieldVerified`
/// change the status lets apply ([`applies`]) snaps its wall. Returns each one's wall and result.
pub fn apply_studs(record: &AsBuiltRecord, fr: &mut Framing, include_pending: bool) -> Vec<(String, Result<FieldSnap, String>)> {
    let mut out = Vec::new();
    for c in record.changes.iter().filter(|c| c.element_kind == "wall" && applies(c.status, include_pending)) {
        let ChangeKind::FieldVerified { at_ft } = c.kind else { continue };
        let Some(i) = fr.walls.iter().position(|w| w.wall.id == c.element_id) else {
            out.push((c.element_id.clone(), Err(format!("no framed wall {}", c.element_id))));
            continue;
        };
        let s = fr.walls.get(i).map_or(0.0, |w| w.wall.local([at_ft[0], at_ft[1]]).0);
        out.push((c.element_id.clone(), fr.field_point(i, s)));
    }
    out
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
            props.insert("AsBuiltDescription".into(), format!("Type changed: {} -> {}", design_type, asbuilt_type));
        }
        ChangeKind::SizeChanged { design_size, asbuilt_size } => {
            props.insert("AsBuiltDescription".into(), format!("Size changed: {} -> {}", design_size, asbuilt_size));
        }
        ChangeKind::Added => {
            props.insert("AsBuiltDescription".into(), "Added in field".into());
        }
        ChangeKind::Deleted => {
            props.insert("AsBuiltDescription".into(), "Not built (deleted)".into());
        }
        ChangeKind::FieldVerified { at_ft } => {
            props.insert("AsBuiltDescription".into(), format!("Field verified at ({:.2}, {:.2}, {:.2})", at_ft[0], at_ft[1], at_ft[2]));
        }
        ChangeKind::HeightChanged { design_ft, asbuilt_ft } => {
            props.insert(
                "AsBuiltDescription".into(),
                format!("Height changed: {} -> {}", truth::feet_inches(*design_ft), truth::feet_inches(*asbuilt_ft)),
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
    let mut s = AsBuiltSummary { total_changes: record.changes.len(), ..Default::default() };
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
            kind: ChangeKind::Moved { from_ft: [10.0, 20.0, 8.0], to_ft: [12.0, 20.0, 8.0] },
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
            let bldg = [(seg.path_ft[0][0] + seg.path_ft[1][0]) * 0.5, (seg.path_ft[0][1] + seg.path_ft[1][1]) * 0.5];
            let paper = building_to_paper(sh, bldg);
            let back = paper_to_building(sh, paper);
            assert!((back[0] - bldg[0]).abs() < 0.01 && (back[1] - bldg[1]).abs() < 0.01, "round trip failed: {bldg:?} -> {paper:?} -> {back:?}");
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
            let bldg = [(seg.path_ft[0][0] + seg.path_ft[1][0]) * 0.5, (seg.path_ft[0][1] + seg.path_ft[1][1]) * 0.5];
            let paper = building_to_paper(sh, bldg);
            let markup = SyntheticMarkup {
                kind: "cloud".into(),
                text: "Move duct 2 feet north".into(),
                author: "Field Inspector".into(),
                date: "2026-10-21".into(),
                paper_in: paper,
                paper_rect_in: [[paper[0] - 0.5, paper[1] - 0.5], [paper[0] + 0.5, paper[1] + 0.5]],
                to_in: None,
            };
            let mapped = import_markups(&t, sh, &[markup]);
            assert_eq!(mapped.len(), 1);
            let m = &mapped[0];
            assert_eq!(m.kind, "cloud");
            assert_eq!(m.sheet, sh.number);
            assert_eq!(m.level, level);
            assert!(!m.near_elements.is_empty(), "should find nearby elements");
            // The segment we targeted should be among the near elements
            assert!(m.near_elements.contains(&seg.id), "expected {} in near_elements {:?}", seg.id, m.near_elements);
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
            near_elements: vec![t.elec_devices.first().map(|d| d.id.clone()).unwrap_or_else(|| "dev-1".into())],
            bounds_ft: [[19.5, 14.5], [20.5, 15.5]],
            to_ft: None,
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
            to_ft: None,
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
            to_ft: None,
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
                kind: ChangeKind::Moved { from_ft: dev.at_ft, to_ft: [dev.at_ft[0] + 2.0, dev.at_ft[1], dev.at_ft[2]] },
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
                kind: ChangeKind::Moved { from_ft: dev.at_ft, to_ft: new_pos },
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
            kind: ChangeKind::Moved { from_ft: [10.0, 20.0, 4.0], to_ft: [12.0, 20.0, 4.0] },
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
            kind: ChangeKind::TypeChanged { design_type: "EMT".into(), asbuilt_type: "RGS".into() },
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
            kind: ChangeKind::HeightChanged { design_ft: 10.5, asbuilt_ft: 11.0 },
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
            to_in: None,
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
            to_in: None,
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
            to_in: None,
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
            to_ft: None,
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
