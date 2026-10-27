//! Pre-cover hold points and in-wall photos (Q334).
//!
//! Before a wall or ceiling is closed (drywall hung, ceiling tiles installed), a rough-in
//! checklist must be complete for every area.  The checklist items (boxes installed, cable
//! pulled, protection plates, firestop, sleeves, grounding, inspection signed) are per wall
//! face or ceiling area.  Photos taken from the field viewer are tagged to their wall face
//! and elevation, so tapping any wall later shows what's behind it.
//!
//! Areas can't be marked "ready to cover" until every checklist item is done and every
//! required inspection phase is signed.  AHJ (Authority Having Jurisdiction) inspection
//! checklists tie in per phase: rough-in electrical, rough-in mechanical, rough-in plumbing,
//! fire-stopping, insulation, etc.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum photos per area.
const MAX_PHOTOS: usize = 500;
/// Maximum areas in one record.
const MAX_AREAS: usize = 10_000;
/// Maximum text field length.
const MAX_TEXT: usize = 2_000;

/// `s` cut to at most [`MAX_TEXT`] bytes, on a character boundary.
pub fn clamp_text(s: &str) -> String {
    let mut end = s.len().min(MAX_TEXT);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------
// Checklist items
// ---------------------------------------------------------------------------

/// A rough-in checklist item for a wall or ceiling area.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChecklistItem {
    /// Unique id within its area.
    pub id: String,
    /// What must be done: `boxes`, `cable`, `protection_plates`, `firestop`, `sleeves`,
    /// `grounding`, `inspection_signed`, or a free-form trade-specific item.
    pub category: String,
    /// Human description: "All boxes installed and secured", "MC cable pulled to all outlets".
    pub description: String,
    /// Whether this item is satisfied.
    pub done: bool,
    /// Who marked it done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_by: Option<String>,
    /// When (ISO 8601).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_date: Option<String>,
    /// Photo ids that prove it (references into `AreaPhotos`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub photo_ids: Vec<String>,
    /// Free-form notes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// The standard rough-in checklist categories.  Each area gets one of each unless the trade
/// doesn't apply (e.g. no sleeves in a stud wall with no penetrations).
pub const STANDARD_CATEGORIES: &[(&str, &str)] = &[
    ("boxes", "Outlet/switch/junction boxes installed and secured"),
    ("cable", "Cable/wire pulled to all devices"),
    ("protection_plates", "Nail/screw protection plates on studs where required"),
    ("firestop", "Firestop sealant at all rated penetrations"),
    ("sleeves", "Sleeves and bushings installed at floor/wall penetrations"),
    ("grounding", "Equipment grounding conductors landed and bonded"),
    ("inspection_signed", "Rough-in inspection signed by the AHJ"),
];

/// Build the default checklist for an area.
pub fn default_checklist(area_id: &str) -> Vec<ChecklistItem> {
    STANDARD_CATEGORIES
        .iter()
        .enumerate()
        .map(|(i, (cat, desc))| ChecklistItem {
            id: format!("{area_id}-CK{:02}", i + 1),
            category: (*cat).into(),
            description: (*desc).into(),
            done: false,
            done_by: None,
            done_date: None,
            photo_ids: Vec::new(),
            notes: String::new(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// In-wall / in-ceiling photos
// ---------------------------------------------------------------------------

/// A photo tagged to a wall face or ceiling area, taken before the wall/ceiling is closed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InWallPhoto {
    /// Unique photo id.
    pub id: String,
    /// Which side of the wall this was taken from (`A` or `B` for walls; ignored for ceilings).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub face: String,
    /// Height of the camera above the floor, feet.  For ceilings this is the plenum height.
    pub elevation_ft: f64,
    /// Position along the wall from `a_ft` in the plan, feet.  For ceilings: [x, y] in the
    /// ceiling area's local coordinates.
    pub along_ft: f64,
    /// Path or URI to the photo file.
    pub uri: String,
    /// Who took it.
    pub author: String,
    /// When (ISO 8601).
    pub date: String,
    /// Description / caption.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub caption: String,
    /// Building-coordinate position where the photo was taken, feet.
    pub at_ft: [f64; 2],
    /// The level.
    pub level: String,
}

// ---------------------------------------------------------------------------
// Cover areas
// ---------------------------------------------------------------------------

/// What kind of area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AreaKind {
    /// A wall face (one side of a wall).
    WallFace,
    /// A ceiling area.
    Ceiling,
}

/// A single wall face or ceiling area that must be checked before covering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoverArea {
    /// Unique area id: `W1-003/A` (wall W1-003 face A), `CLG-L1-OFFICE` (ceiling).
    pub id: String,
    pub kind: AreaKind,
    /// The level.
    pub level: String,
    /// For walls: the plan_wall id (matches `PlanWall::id`).  For ceilings: a ceiling area id.
    pub element_id: String,
    /// For walls: which face (`A` = left of a→b, `B` = right).  Empty for ceilings.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub face: String,
    /// Wall type or ceiling assembly tag.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub type_tag: String,
    /// Room(s) on this side.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rooms: Vec<String>,
    /// Fire rating of the assembly (propagated from the wall/ceiling type).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_rating: Option<String>,
    /// The rough-in checklist.
    pub checklist: Vec<ChecklistItem>,
    /// Photos behind this face / above this ceiling.
    pub photos: Vec<InWallPhoto>,
    /// Whether this area has been marked "ready to cover".
    pub ready_to_cover: bool,
    /// Who marked it ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_by: Option<String>,
    /// When.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_date: Option<String>,
}

impl CoverArea {
    /// Whether every checklist item is done.
    pub fn checklist_complete(&self) -> bool {
        !self.checklist.is_empty() && self.checklist.iter().all(|c| c.done)
    }

    /// Count of items done / total.
    pub fn checklist_progress(&self) -> (usize, usize) {
        let done = self.checklist.iter().filter(|c| c.done).count();
        (done, self.checklist.len())
    }

    /// Whether this area can be marked ready to cover: every checklist item done and every
    /// inspection phase that applies to it signed (photos are not required).
    pub fn can_mark_ready(&self, phases: &[InspectionPhase]) -> bool {
        if !self.checklist_complete() {
            return false;
        }
        // Every required inspection phase for this area's level must be signed.
        for ph in phases {
            if ph.applies_to(self) && !ph.signed {
                return false;
            }
        }
        true
    }

    /// Try to mark ready; returns false if prerequisites aren't met.
    pub fn mark_ready(&mut self, by: &str, date: &str, phases: &[InspectionPhase]) -> bool {
        if !self.can_mark_ready(phases) {
            return false;
        }
        self.ready_to_cover = true;
        self.ready_by = Some(clamp_text(by));
        self.ready_date = Some(clamp_text(date));
        true
    }

    /// What still stands between this area and "ready to cover": the open checklist items
    /// (`id: category`) and the names of the unsigned phases that apply to it (`name (level)`).
    pub fn open_reasons(&self, phases: &[InspectionPhase]) -> (Vec<String>, Vec<String>) {
        let items = self.checklist.iter().filter(|c| !c.done).map(|c| format!("{}: {}", c.id, c.category)).collect();
        let unsigned = phases.iter().filter(|ph| ph.applies_to(self) && !ph.signed).map(InspectionPhase::label).collect();
        (items, unsigned)
    }

    /// Add a photo, respecting the cap. Its text is cut to [`MAX_TEXT`] bytes; a photo with a
    /// position or elevation that isn't a finite number is refused.
    pub fn add_photo(&mut self, mut photo: InWallPhoto) -> bool {
        if self.photos.len() >= MAX_PHOTOS {
            return false;
        }
        if !(photo.elevation_ft.is_finite() && photo.along_ft.is_finite() && photo.at_ft.iter().all(|v| v.is_finite())) {
            return false;
        }
        for t in [&mut photo.id, &mut photo.face, &mut photo.uri, &mut photo.author, &mut photo.date, &mut photo.caption, &mut photo.level] {
            *t = clamp_text(t);
        }
        self.photos.push(photo);
        true
    }

    /// Mark a checklist item done.
    pub fn check_item(&mut self, item_id: &str, by: &str, date: &str) -> bool {
        if let Some(item) = self.checklist.iter_mut().find(|c| c.id == item_id) {
            item.done = true;
            item.done_by = Some(clamp_text(by));
            item.done_date = Some(clamp_text(date));
            true
        } else {
            false
        }
    }

    /// Un-check a checklist item (work was undone or failed re-inspection).
    pub fn uncheck_item(&mut self, item_id: &str) -> bool {
        if let Some(item) = self.checklist.iter_mut().find(|c| c.id == item_id) {
            item.done = false;
            item.done_by = None;
            item.done_date = None;
            // Area is no longer ready.
            self.ready_to_cover = false;
            self.ready_by = None;
            self.ready_date = None;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// AHJ inspection phases
// ---------------------------------------------------------------------------

/// An AHJ (Authority Having Jurisdiction) inspection phase.  Each phase covers a trade or
/// system and must be signed before any area on that level can be covered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InspectionPhase {
    /// Unique phase id.
    pub id: String,
    /// Phase name: "Rough-In Electrical", "Rough-In Mechanical", "Rough-In Plumbing",
    /// "Fire-Stopping", "Insulation", "Vapor Barrier", etc.
    pub name: String,
    /// Which level(s) this phase applies to.  Empty = all levels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<String>,
    /// Which area kinds this applies to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub area_kinds: Vec<AreaKind>,
    /// Whether the AHJ has signed off.
    pub signed: bool,
    /// Inspector name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspector: Option<String>,
    /// Sign-off date (ISO 8601).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_date: Option<String>,
    /// Permit or inspection number.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub permit_number: String,
    /// Notes from the inspector.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

impl InspectionPhase {
    /// Whether this phase applies to a given area.
    pub fn applies_to(&self, area: &CoverArea) -> bool {
        let level_ok = self.levels.is_empty() || self.levels.contains(&area.level);
        let kind_ok = self.area_kinds.is_empty() || self.area_kinds.contains(&area.kind);
        level_ok && kind_ok
    }

    /// The phase's name with its level(s): `Rough-In Electrical (L1)`.
    pub fn label(&self) -> String {
        if self.levels.is_empty() { self.name.clone() } else { format!("{} ({})", self.name, self.levels.join(", ")) }
    }

    /// Sign off the phase.
    pub fn sign(&mut self, inspector: &str, date: &str) {
        self.signed = true;
        self.inspector = Some(clamp_text(inspector));
        self.signed_date = Some(clamp_text(date));
    }
}

/// The standard AHJ inspection phases for a typical project.
pub const STANDARD_PHASES: &[(&str, &str)] = &[
    ("rough_elec", "Rough-In Electrical"),
    ("rough_mech", "Rough-In Mechanical"),
    ("rough_plumb", "Rough-In Plumbing"),
    ("rough_fa", "Rough-In Fire Alarm"),
    ("firestop", "Fire-Stopping"),
    ("insulation", "Insulation"),
];

/// Build the default inspection phases for a set of levels: one instance of each standard
/// phase per level (`rough_elec@L1`), so the AHJ signs each floor on its own. With no levels,
/// one instance of each that applies to every level.
pub fn default_phases(levels: &[String]) -> Vec<InspectionPhase> {
    let phase = |id: &str, name: &str, level: Option<&String>| InspectionPhase {
        id: level.map_or_else(|| id.to_string(), |l| format!("{id}@{l}")),
        name: name.into(),
        levels: level.cloned().into_iter().collect(),
        area_kinds: Vec::new(), // applies to all
        signed: false,
        inspector: None,
        signed_date: None,
        permit_number: String::new(),
        notes: String::new(),
    };
    if levels.is_empty() {
        return STANDARD_PHASES.iter().map(|(id, name)| phase(id, name, None)).collect();
    }
    levels.iter().flat_map(|l| STANDARD_PHASES.iter().map(move |(id, name)| phase(id, name, Some(l)))).collect()
}

// ---------------------------------------------------------------------------
// The full cover-check record
// ---------------------------------------------------------------------------

/// The full pre-cover record for a project: all areas, their checklists and photos, and the
/// inspection phases.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CoverRecord {
    pub schema: String,
    pub project: String,
    pub areas: Vec<CoverArea>,
    pub phases: Vec<InspectionPhase>,
}

impl CoverRecord {
    pub fn new(project: &str) -> Self {
        Self { schema: "cadaddy-covercheck/1".into(), project: project.into(), areas: Vec::new(), phases: Vec::new() }
    }

    /// Find an area by id.
    pub fn area(&self, id: &str) -> Option<&CoverArea> {
        self.areas.iter().find(|a| a.id == id)
    }

    /// Find an area by id (mutable).
    pub fn area_mut(&mut self, id: &str) -> Option<&mut CoverArea> {
        self.areas.iter_mut().find(|a| a.id == id)
    }

    /// Add an area, respecting the cap.
    pub fn add_area(&mut self, area: CoverArea) -> bool {
        if self.areas.len() >= MAX_AREAS {
            return false;
        }
        self.areas.push(area);
        true
    }

    /// Areas for a specific wall (both faces).
    pub fn wall_areas(&self, wall_id: &str) -> Vec<&CoverArea> {
        self.areas.iter().filter(|a| a.kind == AreaKind::WallFace && a.element_id == wall_id).collect()
    }

    /// Areas for a specific ceiling.
    pub fn ceiling_areas(&self, ceiling_id: &str) -> Vec<&CoverArea> {
        self.areas.iter().filter(|a| a.kind == AreaKind::Ceiling && a.element_id == ceiling_id).collect()
    }

    /// All areas on a level.
    pub fn on_level(&self, level: &str) -> Vec<&CoverArea> {
        self.areas.iter().filter(|a| a.level == level).collect()
    }

    /// All photos for a given wall (both faces combined).
    pub fn wall_photos(&self, wall_id: &str) -> Vec<&InWallPhoto> {
        self.wall_areas(wall_id).iter().flat_map(|a| a.photos.iter()).collect()
    }

    /// Sign the phase `phase` (its id, standard id such as `rough_elec`, or name, any case) for
    /// `level`: the instances that cover that level. Returns how many were signed (0: no such
    /// phase on that level).
    pub fn sign_phase(&mut self, phase: &str, level: &str, inspector: &str, date: &str) -> usize {
        let want = phase.trim();
        let mut n = 0;
        for ph in &mut self.phases {
            let base = ph.id.split('@').next().unwrap_or_default();
            let named = ph.id.eq_ignore_ascii_case(want) || base.eq_ignore_ascii_case(want) || ph.name.eq_ignore_ascii_case(want);
            let on_level = ph.levels.is_empty() || ph.levels.iter().any(|l| l == level);
            if named && on_level {
                ph.sign(inspector, date);
                n += 1;
            }
        }
        n
    }

    /// Close the gate's back door: an area marked ready whose checklist or applicable phases
    /// aren't complete (a hand-edited or older record) is no longer ready. Called whenever a
    /// record is loaded; returns the areas it cleared and why.
    pub fn validate(&mut self) -> Vec<ClearedArea> {
        let phases = self.phases.clone();
        let mut cleared = Vec::new();
        for a in &mut self.areas {
            if a.ready_to_cover && !a.can_mark_ready(&phases) {
                let (open_items, unsigned_phases) = a.open_reasons(&phases);
                a.ready_to_cover = false;
                a.ready_by = None;
                a.ready_date = None;
                cleared.push(ClearedArea { area: a.id.clone(), open_items, unsigned_phases });
            }
        }
        cleared
    }

    /// Summary of what's inside a wall: the checklist items, photos, and whether it's ready.
    pub fn wall_contents(&self, wall_id: &str) -> WallContents {
        let areas = self.wall_areas(wall_id);
        let mut contents = WallContents::default();
        for a in &areas {
            let (done, total) = a.checklist_progress();
            contents.checklist_done += done;
            contents.checklist_total += total;
            contents.photo_count += a.photos.len();
            contents.faces.push(FaceContents {
                face: a.face.clone(),
                rooms: a.rooms.clone(),
                fire_rating: a.fire_rating.clone(),
                checklist_done: done,
                checklist_total: total,
                photo_count: a.photos.len(),
                ready: a.ready_to_cover,
            });
        }
        contents.all_ready = !areas.is_empty() && areas.iter().all(|a| a.ready_to_cover);
        contents
    }

    /// Progress summary for the whole record.
    pub fn progress(&self) -> CoverProgress {
        let total = self.areas.len();
        let ready = self.areas.iter().filter(|a| a.ready_to_cover).count();
        let complete = self.areas.iter().filter(|a| a.checklist_complete()).count();
        let photos: usize = self.areas.iter().map(|a| a.photos.len()).sum();
        let phases_total = self.phases.len();
        let phases_signed = self.phases.iter().filter(|p| p.signed).count();
        CoverProgress { total, ready, checklist_complete: complete, photos, phases_total, phases_signed }
    }
}

/// An area [`CoverRecord::validate`] took "ready to cover" off, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClearedArea {
    pub area: String,
    pub open_items: Vec<String>,
    pub unsigned_phases: Vec<String>,
}

/// What's behind a wall (both faces).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WallContents {
    pub faces: Vec<FaceContents>,
    pub checklist_done: usize,
    pub checklist_total: usize,
    pub photo_count: usize,
    pub all_ready: bool,
}

/// One face of a wall.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceContents {
    pub face: String,
    pub rooms: Vec<String>,
    pub fire_rating: Option<String>,
    pub checklist_done: usize,
    pub checklist_total: usize,
    pub photo_count: usize,
    pub ready: bool,
}

/// Progress across the whole project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoverProgress {
    pub total: usize,
    pub ready: usize,
    pub checklist_complete: usize,
    pub photos: usize,
    pub phases_total: usize,
    pub phases_signed: usize,
}

// ---------------------------------------------------------------------------
// Building the cover areas from a truth (fixture set)
// ---------------------------------------------------------------------------

use crate::truth::Truth;

/// Build cover areas from the truth's plan walls: each wall gets two face areas (A and B),
/// and each ceiling area gets one.
pub fn areas_from_truth(t: &Truth) -> Vec<CoverArea> {
    let mut areas = Vec::new();
    // Wall faces.
    for w in t.plan_walls.iter().take(MAX_AREAS / 2) {
        let wt = t.wall_types.iter().find(|x| x.tag == w.wall_type);
        let fire_rating = wt.and_then(|x| x.fire_rating.clone());
        let sides = face_rooms(w, &t.building.rooms);
        for face in ["A", "B"] {
            let face_rooms: Vec<String> = sides.iter().filter(|(f, _)| *f == face).map(|(_, r)| r.clone()).collect();
            let area_id = format!("{}/{face}", w.id);
            areas.push(CoverArea {
                id: area_id.clone(),
                kind: AreaKind::WallFace,
                level: w.level.clone(),
                element_id: w.id.clone(),
                face: face.into(),
                type_tag: w.wall_type.clone(),
                rooms: face_rooms,
                fire_rating: fire_rating.clone(),
                checklist: default_checklist(&area_id),
                photos: Vec::new(),
                ready_to_cover: false,
                ready_by: None,
                ready_date: None,
            });
        }
    }
    // Ceiling areas from the building spec's rooms (each room with a ceiling is an area).
    for (i, rm) in t.building.rooms.iter().enumerate().take(MAX_AREAS) {
        if rm.ceiling.is_none() {
            continue;
        }
        let area_id = format!("CLG-{}-{}", rm.level, rm.number);
        areas.push(CoverArea {
            id: area_id.clone(),
            kind: AreaKind::Ceiling,
            level: rm.level.clone(),
            element_id: format!("ceiling-{i}"),
            face: String::new(),
            type_tag: String::new(),
            rooms: vec![rm.number.clone()],
            fire_rating: None,
            checklist: default_checklist(&area_id),
            photos: Vec::new(),
            ready_to_cover: false,
            ready_by: None,
            ready_date: None,
        });
    }
    areas
}

/// Which side of wall `w` a plan point is on: `A` left of a→b, `B` right of it, `None` on the
/// centreline (or a zero-length wall). The rule [`resolve_wall_tap`] uses.
pub fn side_of(w: &crate::truth::PlanWall, p: [f64; 2]) -> Option<&'static str> {
    let dx = w.b_ft[0] - w.a_ft[0];
    let dy = w.b_ft[1] - w.a_ft[1];
    let len = (dx * dx + dy * dy).sqrt();
    if len.is_nan() || len <= 1e-6 {
        return None;
    }
    // Signed distance from the centreline, feet.
    let off = (dx * (p[1] - w.a_ft[1]) - dy * (p[0] - w.a_ft[0])) / len;
    if !off.is_finite() || off.abs() < 1e-6 {
        None
    } else if off > 0.0 {
        Some("A")
    } else {
        Some("B")
    }
}

/// The faces the rooms `w` names are on, by geometry: each room (found by number on the wall's
/// level) is on the side of the wall its outline's centre is. When the centre sits on the
/// centreline, a point just off the wall's middle on each side decides by which outline holds
/// it. A room that can't be found or placed is left off both faces, never guessed, so an
/// exterior wall gets a room on one face only.
fn face_rooms(w: &crate::truth::PlanWall, rooms: &[crate::truth::Room]) -> Vec<(&'static str, String)> {
    let mid = [(w.a_ft[0] + w.b_ft[0]) / 2.0, (w.a_ft[1] + w.b_ft[1]) / 2.0];
    let dx = w.b_ft[0] - w.a_ft[0];
    let dy = w.b_ft[1] - w.a_ft[1];
    let len = (dx * dx + dy * dy).sqrt();
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for number in &w.rooms {
        let Some(rm) = rooms.iter().find(|r| r.level == w.level && &r.number == number) else { continue };
        let centre = [(rm.min_ft[0] + rm.max_ft[0]) / 2.0, (rm.min_ft[1] + rm.max_ft[1]) / 2.0];
        let side = side_of(w, centre).or_else(|| {
            if len.is_nan() || len <= 1e-6 {
                return None;
            }
            // Half a foot off the middle, left (A) then right (B).
            let n = [-dy / len * 0.5, dx / len * 0.5];
            let inside = |p: [f64; 2]| p[0] >= rm.min_ft[0] && p[0] <= rm.max_ft[0] && p[1] >= rm.min_ft[1] && p[1] <= rm.max_ft[1];
            match (inside([mid[0] + n[0], mid[1] + n[1]]), inside([mid[0] - n[0], mid[1] - n[1]])) {
                (true, false) => Some("A"),
                (false, true) => Some("B"),
                _ => None,
            }
        });
        if let Some(face) = side
            && !out.iter().any(|(f, r)| *f == face && r == number)
        {
            out.push((face, number.clone()));
        }
    }
    out
}

/// Build a full cover record from a truth.
pub fn record_from_truth(t: &Truth, project: &str) -> CoverRecord {
    let levels: Vec<String> = {
        let mut lvs: Vec<String> = t.building.levels.iter().map(|l| l.name.clone()).collect();
        lvs.sort();
        lvs.dedup();
        lvs
    };
    CoverRecord { schema: "cadaddy-covercheck/1".into(), project: project.into(), areas: areas_from_truth(t), phases: default_phases(&levels) }
}

// ---------------------------------------------------------------------------
// Resolving a plan-tap to the wall behind it
// ---------------------------------------------------------------------------

/// Given a tap position on the plan (building feet) and a level, find the wall whose face the
/// user tapped.  Returns the area id of the face nearest the tap point and the distance to the
/// wall centreline.
pub fn resolve_wall_tap(
    record: &CoverRecord,
    plan_walls: &[crate::truth::PlanWall],
    tap_ft: [f64; 2],
    level: &str,
    max_distance_ft: f64,
) -> Option<(String, f64)> {
    let mut best: Option<(String, f64)> = None;
    for w in plan_walls {
        if w.level != level {
            continue;
        }
        let dx = w.b_ft[0] - w.a_ft[0];
        let dy = w.b_ft[1] - w.a_ft[1];
        let len_sq = dx * dx + dy * dy;
        if len_sq < 1e-12 {
            continue;
        }
        // Parameter along the wall.
        let t = ((tap_ft[0] - w.a_ft[0]) * dx + (tap_ft[1] - w.a_ft[1]) * dy) / len_sq;
        if !(-0.05..=1.05).contains(&t) {
            continue;
        }
        let t_clamped = t.clamp(0.0, 1.0);
        let closest = [w.a_ft[0] + t_clamped * dx, w.a_ft[1] + t_clamped * dy];
        let dist = ((tap_ft[0] - closest[0]).powi(2) + (tap_ft[1] - closest[1]).powi(2)).sqrt();
        if dist > max_distance_ft {
            continue;
        }
        if best.as_ref().is_none_or(|(_, d)| dist < *d) {
            // Determine which face: the side of the wall the tap is on.
            let cross = dx * (tap_ft[1] - w.a_ft[1]) - dy * (tap_ft[0] - w.a_ft[0]);
            let face = if cross >= 0.0 { "A" } else { "B" };
            let area_id = format!("{}/{face}", w.id);
            // Only return if the area actually exists in the record.
            if record.area(&area_id).is_some() {
                best = Some((area_id, dist));
            }
        }
    }
    best
}

/// How far from a wall's centreline a photo may be taken and still be filed on it, feet.
pub const PHOTO_REACH_FT: f64 = 3.0;

/// File `photo` by where it was taken: its `at_ft` and `level` pick the wall face (the side of
/// the nearest wall within `max_distance_ft`, as a tap does), `along_ft` becomes its distance
/// from the wall's `a` end, and `face` is set. A photo that states a face the position
/// contradicts, has no wall near it, has numbers that aren't finite, or would pass an area's
/// cap is refused with the reason. Returns the area id it was filed on.
pub fn add_photo_at(
    record: &mut CoverRecord,
    plan_walls: &[crate::truth::PlanWall],
    mut photo: InWallPhoto,
    max_distance_ft: f64,
) -> Result<String, String> {
    if !(photo.at_ft.iter().all(|v| v.is_finite()) && photo.elevation_ft.is_finite()) {
        return Err(format!("photo {}: its position and elevation must be finite numbers", clamp_text(&photo.id)));
    }
    let reach = if max_distance_ft.is_finite() && max_distance_ft > 0.0 { max_distance_ft } else { PHOTO_REACH_FT };
    let Some((area_id, _)) = resolve_wall_tap(record, plan_walls, photo.at_ft, &photo.level, reach) else {
        return Err(format!(
            "photo {}: no wall face within {reach} ft of [{:.2}, {:.2}] on level {}",
            clamp_text(&photo.id),
            photo.at_ft[0],
            photo.at_ft[1],
            clamp_text(&photo.level)
        ));
    };
    let (wall_id, face) = area_id.rsplit_once('/').unwrap_or((area_id.as_str(), ""));
    let stated = photo.face.trim();
    if !stated.is_empty() && !stated.eq_ignore_ascii_case(face) {
        return Err(format!(
            "photo {}: it says face {} but was taken on the face {face} side of {wall_id} ({area_id})",
            clamp_text(&photo.id),
            clamp_text(stated)
        ));
    }
    if let Some(w) = plan_walls.iter().find(|w| w.id == wall_id && w.level == photo.level) {
        let dx = w.b_ft[0] - w.a_ft[0];
        let dy = w.b_ft[1] - w.a_ft[1];
        let len = (dx * dx + dy * dy).sqrt();
        if len > 1e-6 {
            let along = ((photo.at_ft[0] - w.a_ft[0]) * dx + (photo.at_ft[1] - w.a_ft[1]) * dy) / len;
            photo.along_ft = along.clamp(0.0, len);
        }
    }
    photo.face = face.to_string();
    let id = clamp_text(&photo.id);
    let area = record.area_mut(&area_id).ok_or_else(|| format!("photo {id}: area {area_id} is not in the record"))?;
    if area.photos.iter().any(|p| p.id == id) {
        return Err(format!("photo {id} is already filed on {area_id}"));
    }
    if !area.add_photo(photo) {
        return Err(format!("photo {id}: {area_id} already has {MAX_PHOTOS} photos"));
    }
    Ok(area_id)
}

/// Resolve a ceiling tap: given a plan position and level, find the ceiling area the user tapped.
pub fn resolve_ceiling_tap(record: &CoverRecord, rooms: &[crate::truth::Room], tap_ft: [f64; 2], level: &str) -> Option<String> {
    for rm in rooms {
        if rm.level != level || rm.ceiling.is_none() {
            continue;
        }
        if tap_ft[0] >= rm.min_ft[0] && tap_ft[0] <= rm.max_ft[0] && tap_ft[1] >= rm.min_ft[1] && tap_ft[1] <= rm.max_ft[1] {
            let area_id = format!("CLG-{}-{}", rm.level, rm.number);
            if record.area(&area_id).is_some() {
                return Some(area_id);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Summary / reporting helpers
// ---------------------------------------------------------------------------

/// A per-level summary for reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelSummary {
    pub level: String,
    pub walls_total: usize,
    pub walls_ready: usize,
    pub ceilings_total: usize,
    pub ceilings_ready: usize,
    pub photos: usize,
    pub phases_signed: usize,
    pub phases_total: usize,
}

/// Summarize the cover status by level.
pub fn summary_by_level(record: &CoverRecord) -> Vec<LevelSummary> {
    let mut map: BTreeMap<String, LevelSummary> = BTreeMap::new();
    for a in &record.areas {
        let e = map.entry(a.level.clone()).or_insert_with(|| LevelSummary { level: a.level.clone(), ..LevelSummary::default() });
        match a.kind {
            AreaKind::WallFace => {
                e.walls_total += 1;
                if a.ready_to_cover {
                    e.walls_ready += 1;
                }
            }
            AreaKind::Ceiling => {
                e.ceilings_total += 1;
                if a.ready_to_cover {
                    e.ceilings_ready += 1;
                }
            }
        }
        e.photos += a.photos.len();
    }
    for ph in &record.phases {
        for level in &ph.levels {
            if let Some(e) = map.get_mut(level) {
                e.phases_total += 1;
                if ph.signed {
                    e.phases_signed += 1;
                }
            }
        }
    }
    map.into_values().collect()
}
