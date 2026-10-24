//! The field package: the building model as a tablet or phone on the job needs it, in one file
//! that opens offline (`fieldview.export` writes it; the field viewer reads it).
//!
//! It holds the levels, the rooms (number, name, outline), every model element as a box or a run
//! with its trade, room and BIM type (the type's spec fields with their source and confidence,
//! shared by mark so a type is stored once), the open clashes and RFIs, and the notes and
//! verified points made in the field. Notes travel back to the drawing as a [`FieldNotes`] file
//! (`fieldview.sync` merges them by id, so syncing twice adds nothing).
//!
//! Coordinates are building feet: x east, y north from the level's base point, z up from the
//! building's datum (a level's floor is at its `elevation_ft`). The JSON shape is the struct
//! shape below (snake case); [`FIELD_SCHEMA`] names the version, and a package from a newer
//! CADaddy is refused rather than half read. Everything read is capped (hostile files).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The package format this CADaddy reads and writes.
pub const FIELD_SCHEMA: &str = "cadaddy-field/1";
/// The notes file format (field → drawing).
pub const NOTES_SCHEMA: &str = "cadaddy-fieldnotes/1";
/// Largest package file read.
pub const MAX_PACKAGE_BYTES: usize = 96 << 20;
/// Most elements in a package.
pub const MAX_ELEMENTS: usize = 250_000;
/// Most rooms, levels, types, issues and notes.
pub const MAX_ROOMS: usize = 20_000;
pub const MAX_LEVELS: usize = 500;
pub const MAX_TYPES: usize = 20_000;
pub const MAX_ISSUES: usize = 50_000;
pub const MAX_NOTES: usize = 50_000;
/// Most points in a room outline or an element's run path, and spec fields on a type.
pub const MAX_POINTS: usize = 512;
pub const MAX_FIELDS: usize = 128;
/// Longest text kept in a note.
pub const MAX_TEXT: usize = 4_000;
/// Room search results returned.
pub const MAX_SEARCH: usize = 50;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldPackage {
    pub schema: String,
    pub project: String,
    pub levels: Vec<FieldLevel>,
    pub rooms: Vec<FieldRoom>,
    /// The trades the elements belong to, in display order.
    pub trades: Vec<FieldTrade>,
    /// BIM types by mark (Q297): what each element is.
    pub types: Vec<FieldType>,
    pub elements: Vec<FieldElement>,
    /// Clashes and RFIs.
    pub issues: Vec<FieldIssue>,
    /// Notes and verified points made in the field (and ones already synced).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<FieldNote>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldLevel {
    pub name: String,
    pub elevation_ft: f64,
    pub height_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldRoom {
    /// Room number (`214`), or the name when the room has no number.
    pub number: String,
    pub name: String,
    pub level: String,
    /// Closed outline, feet (empty when only the tag position is known).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<[f64; 2]>,
    /// Where its tag is (or the outline's centre).
    pub at: [f64; 2],
    /// Its typical unit and hand (Q305), e.g. `King (mirrored)`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unit: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldTrade {
    /// `arch`, `struct`, `mech`, `plumb`, `elec`, `fire`, `lowvolt`.
    pub key: String,
    pub label: String,
    pub color: [u8; 3],
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldType {
    pub mark: String,
    pub category: String,
    pub fields: Vec<FieldSpec>,
}

/// One spec field of a type: its value, where it was stated and how sure it is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldSpec {
    pub key: String,
    pub value: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// `stated`, `inferred` or `assumed`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub confidence: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldElement {
    /// Stable id: the drawing handle, or the generator's key.
    pub id: String,
    /// A [`FieldTrade::key`].
    pub trade: String,
    /// `wall`, `door`, `duct`, `pipe`, `conduit`, `device`, `fixture`, `terminal`, `equipment`…
    pub kind: String,
    pub level: String,
    /// The room it is in ([`FieldRoom::number`]); empty when in none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub room: String,
    /// Bounding box, feet.
    pub min: [f64; 3],
    pub max: [f64; 3],
    /// A run's centreline (duct, pipe, conduit), feet; empty for boxes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<[f64; 3]>,
    /// A run's size, inches (width × height, or diameter × 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_in: Option<[f64; 2]>,
    /// Its BIM type ([`FieldType::mark`]); empty when untyped.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mark: String,
    /// Where the element came from (sheet, layer, system).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    #[default]
    Clash,
    Rfi,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldIssue {
    pub id: String,
    pub kind: IssueKind,
    pub title: String,
    /// `new`, `active`, `open`, `resolved`, `approved`, `answered`, `closed`…
    pub status: String,
    pub level: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub room: String,
    pub at: [f64; 3],
    /// The elements it involves ([`FieldElement::id`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<String>,
}

impl FieldIssue {
    /// Still to be dealt with.
    pub fn is_open(&self) -> bool {
        let s = self.status.trim().to_ascii_lowercase();
        !matches!(s.as_str(), "resolved" | "approved" | "answered" | "closed" | "void")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    #[default]
    Note,
    /// A point measured on site (a stud, a sleeve, a device as built).
    VerifiedPoint,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldNote {
    /// Unique across devices: the device tag and a running number (`FN-tab1-0007`).
    pub id: String,
    pub kind: NoteKind,
    pub level: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub room: String,
    pub at: [f64; 3],
    /// The element it is about, when one was tapped.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub element: String,
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author: String,
    /// When it was made, as the device wrote it (ISO 8601 when it has a clock).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub when: String,
    /// Sent back to the drawing already.
    #[serde(skip_serializing_if = "is_false")]
    pub synced: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Notes on their way back to the drawing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldNotes {
    pub schema: String,
    pub project: String,
    pub notes: Vec<FieldNote>,
}

impl FieldNotes {
    pub fn new(project: &str, notes: Vec<FieldNote>) -> FieldNotes {
        FieldNotes { schema: NOTES_SCHEMA.into(), project: project.into(), notes }
    }

    pub fn from_json(bytes: &[u8]) -> Result<FieldNotes, String> {
        if bytes.len() > MAX_PACKAGE_BYTES {
            return Err(format!("the notes file is larger than {} MB", MAX_PACKAGE_BYTES >> 20));
        }
        let mut n: FieldNotes = serde_json::from_slice(bytes).map_err(|e| format!("not a field notes file: {e}"))?;
        if !n.schema.is_empty() && n.schema != NOTES_SCHEMA {
            return Err(format!("field notes `{}` are from a newer CADaddy (this one reads {NOTES_SCHEMA})", n.schema));
        }
        n.notes.truncate(MAX_NOTES);
        n.notes.iter_mut().for_each(clean_note);
        n.notes.retain(|x| !x.id.trim().is_empty());
        Ok(n)
    }
}

/// What [`merge_notes`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
}

/// Merges `incoming` notes into `into` by id: a new id is added, a known one replaced when it
/// differs (the later `when` wins; equal times take the incoming one). Idempotent.
pub fn merge_notes(into: &mut Vec<FieldNote>, incoming: &[FieldNote]) -> Merged {
    let mut m = Merged::default();
    let mut at: BTreeMap<String, usize> = into.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect();
    for n in incoming.iter().take(MAX_NOTES) {
        if n.id.trim().is_empty() {
            continue;
        }
        let mut n = n.clone();
        n.synced = false;
        match at.get(&n.id).and_then(|i| into.get_mut(*i)) {
            Some(old) => {
                let mut cmp = old.clone();
                cmp.synced = false;
                if cmp == n || n.when < old.when {
                    m.unchanged += 1;
                } else {
                    *old = n;
                    m.updated += 1;
                }
            }
            None => {
                if into.len() < MAX_NOTES {
                    at.insert(n.id.clone(), into.len());
                    into.push(n);
                    m.added += 1;
                }
            }
        }
    }
    m
}

fn clip(s: &mut String, n: usize) {
    if s.len() > n {
        let mut end = n;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
}

fn finite3(p: &mut [f64; 3]) {
    p.iter_mut().filter(|v| !v.is_finite()).for_each(|v| *v = 0.0);
}

fn clean_note(n: &mut FieldNote) {
    clip(&mut n.text, MAX_TEXT);
    for s in [&mut n.id, &mut n.level, &mut n.room, &mut n.element, &mut n.author, &mut n.when] {
        clip(s, 200);
    }
    finite3(&mut n.at);
}

/// A trade's default label and colour.
pub fn trade_info(key: &str) -> FieldTrade {
    let (label, color) = match key {
        "arch" => ("Architectural", [200, 196, 186]),
        "struct" => ("Structural", [150, 150, 160]),
        "mech" => ("Mechanical", [60, 120, 230]),
        "plumb" => ("Plumbing", [40, 170, 90]),
        "elec" => ("Electrical", [235, 170, 30]),
        "fire" => ("Fire protection", [220, 50, 50]),
        "lowvolt" => ("Low voltage", [170, 90, 210]),
        _ => ("Other", [140, 140, 140]),
    };
    FieldTrade { key: key.into(), label: label.into(), color }
}

/// The trade an element of `kind` (and MEP system or layer name `system`) belongs to.
pub fn trade_of(kind: &str, system: &str) -> &'static str {
    let s = system.to_ascii_uppercase();
    let k = kind.to_ascii_lowercase();
    if s.starts_with("FA-") || s.starts_with("F-") || s.contains("SPRK") || k.contains("sprinkler") || k == "fire_alarm" {
        return "fire";
    }
    if s.starts_with("T-") || s.starts_with("LV-") || k == "telecom" || k == "low_voltage" {
        return "lowvolt";
    }
    match k.as_str() {
        "duct" | "terminal" | "air_terminal" | "fitting" if !s.starts_with('P') && !s.starts_with('E') => "mech",
        "pipe" | "fixture" | "plumbing" | "rough_in" => "plumb",
        "conduit" | "device" | "panel" | "luminaire" | "receptacle" | "switch" => "elec",
        "column" | "beam" | "joist" | "slab" | "footing" | "stud" => "struct",
        "equipment" if s.starts_with('E') => "elec",
        "equipment" if s.starts_with('P') => "plumb",
        "equipment" => "mech",
        _ if s.starts_with('M') || s.starts_with("H-") => "mech",
        _ if s.starts_with('P') => "plumb",
        _ if s.starts_with('E') => "elec",
        _ if s.starts_with('S') => "struct",
        _ => "arch",
    }
}

/// A room search match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchRank {
    /// The number, exactly.
    Number,
    /// The number starts with the query.
    NumberPrefix,
    /// The name contains it.
    Name,
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// A measured distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    pub dist: f64,
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
}

impl Measured {
    pub fn between(a: [f64; 3], b: [f64; 3]) -> Measured {
        let [dx, dy, dz] = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        Measured { dist: (dx * dx + dy * dy + dz * dz).sqrt(), dx, dy, dz }
    }

    /// `12'-3 1/2"` and its plan and height parts.
    pub fn text(&self) -> String {
        format!("{}  (plan {}, rise {})", crate::feet(self.dist), crate::feet((self.dx * self.dx + self.dy * self.dy).sqrt()), crate::feet(self.dz))
    }
}

impl FieldPackage {
    pub fn new(project: &str) -> FieldPackage {
        FieldPackage { schema: FIELD_SCHEMA.into(), project: project.into(), ..FieldPackage::default() }
    }

    /// Reads a package, refusing one too large or from a newer CADaddy, capping every list and
    /// clearing numbers that aren't finite.
    pub fn from_json(bytes: &[u8]) -> Result<FieldPackage, String> {
        if bytes.len() > MAX_PACKAGE_BYTES {
            return Err(format!("the package is larger than {} MB", MAX_PACKAGE_BYTES >> 20));
        }
        let mut p: FieldPackage = serde_json::from_slice(bytes).map_err(|e| format!("not a field package: {e}"))?;
        if p.schema != FIELD_SCHEMA {
            return Err(if p.schema.starts_with("cadaddy-field/") {
                format!("the package `{}` is from a newer CADaddy (this one reads {FIELD_SCHEMA})", p.schema)
            } else {
                "not a field package (no `cadaddy-field` schema)".into()
            });
        }
        p.clean();
        Ok(p)
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }

    /// Caps every list and clears numbers that aren't finite.
    pub fn clean(&mut self) {
        self.levels.truncate(MAX_LEVELS);
        self.rooms.truncate(MAX_ROOMS);
        self.trades.truncate(64);
        self.types.truncate(MAX_TYPES);
        self.elements.truncate(MAX_ELEMENTS);
        self.issues.truncate(MAX_ISSUES);
        self.notes.truncate(MAX_NOTES);
        for l in &mut self.levels {
            for v in [&mut l.elevation_ft, &mut l.height_ft] {
                if !v.is_finite() {
                    *v = 0.0;
                }
            }
        }
        for r in &mut self.rooms {
            r.outline.truncate(MAX_POINTS);
            r.outline.retain(|p| p.iter().all(|v| v.is_finite()));
            if !r.at.iter().all(|v| v.is_finite()) {
                r.at = [0.0; 2];
            }
        }
        for t in &mut self.types {
            t.fields.truncate(MAX_FIELDS);
        }
        for e in &mut self.elements {
            finite3(&mut e.min);
            finite3(&mut e.max);
            for (lo, hi) in e.min.iter_mut().zip(e.max.iter_mut()) {
                if *lo > *hi {
                    std::mem::swap(lo, hi);
                }
            }
            e.path.truncate(MAX_POINTS);
            e.path.retain(|p| p.iter().all(|v| v.is_finite()));
            if e.size_in.is_some_and(|s| !s.iter().all(|v| v.is_finite())) {
                e.size_in = None;
            }
        }
        for i in &mut self.issues {
            finite3(&mut i.at);
            i.elements.truncate(64);
        }
        self.notes.iter_mut().for_each(clean_note);
    }

    pub fn level(&self, name: &str) -> Option<&FieldLevel> {
        self.levels.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }

    pub fn trade(&self, key: &str) -> FieldTrade {
        self.trades.iter().find(|t| t.key == key).cloned().unwrap_or_else(|| trade_info(key))
    }

    pub fn type_of(&self, e: &FieldElement) -> Option<&FieldType> {
        if e.mark.is_empty() {
            return None;
        }
        self.types.iter().find(|t| t.mark.eq_ignore_ascii_case(&e.mark))
    }

    /// Rooms whose number or name matches `query`, best first: the number exactly, numbers
    /// starting with it, then names containing it (spaces, dashes and case ignored).
    pub fn search_rooms(&self, query: &str) -> Vec<(usize, MatchRank)> {
        let q = norm(query);
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<(usize, MatchRank)> = self
            .rooms
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let n = norm(&r.number);
                if n == q {
                    Some((i, MatchRank::Number))
                } else if n.starts_with(&q) {
                    Some((i, MatchRank::NumberPrefix))
                } else if norm(&r.name).contains(&q) {
                    Some((i, MatchRank::Name))
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| self.room_key(a.0).cmp(&self.room_key(b.0))));
        hits.truncate(MAX_SEARCH);
        hits
    }

    fn room_key(&self, i: usize) -> (usize, String) {
        self.rooms.get(i).map(|r| (r.number.len(), r.number.clone())).unwrap_or_default()
    }

    /// The room on `level` whose outline holds plan point `p`.
    pub fn room_at(&self, level: &str, p: [f64; 2]) -> Option<usize> {
        self.rooms.iter().position(|r| r.level.eq_ignore_ascii_case(level) && r.outline.len() >= 3 && inside(&r.outline, p))
    }

    /// Gives every element and issue without a room the room its plan centre is in.
    pub fn assign_rooms(&mut self) {
        let mut by_level: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, r) in self.rooms.iter().enumerate() {
            if r.outline.len() >= 3 {
                by_level.entry(r.level.to_ascii_lowercase()).or_default().push(i);
            }
        }
        let find = |rooms: &[FieldRoom], level: &str, p: [f64; 2]| -> Option<String> {
            by_level.get(&level.to_ascii_lowercase())?.iter().filter_map(|i| rooms.get(*i)).find(|r| inside(&r.outline, p)).map(|r| r.number.clone())
        };
        for e in &mut self.elements {
            if e.room.is_empty() {
                let c = [(e.min[0] + e.max[0]) * 0.5, (e.min[1] + e.max[1]) * 0.5];
                e.room = find(&self.rooms, &e.level, c).unwrap_or_default();
            }
        }
        for i in &mut self.issues {
            if i.room.is_empty() {
                i.room = find(&self.rooms, &i.level, [i.at[0], i.at[1]]).unwrap_or_default();
            }
        }
    }

    /// The elements in room `room` (its number), the hidden trades left out.
    pub fn elements_in_room<'a>(&'a self, room: &'a FieldRoom, hidden: &'a BTreeSet<String>) -> impl Iterator<Item = usize> + 'a {
        self.elements
            .iter()
            .enumerate()
            .filter(move |(_, e)| {
                !hidden.contains(&e.trade) && e.level.eq_ignore_ascii_case(&room.level) && e.room.eq_ignore_ascii_case(&room.number)
            })
            .map(|(i, _)| i)
    }

    /// The open clashes and RFIs in a room.
    pub fn open_issues<'a>(&'a self, room: &'a FieldRoom) -> impl Iterator<Item = &'a FieldIssue> + 'a {
        self.issues.iter().filter(move |i| {
            i.is_open()
                && i.level.eq_ignore_ascii_case(&room.level)
                && (i.room.eq_ignore_ascii_case(&room.number)
                    || (i.room.is_empty() && room.outline.len() >= 3 && inside(&room.outline, [i.at[0], i.at[1]])))
        })
    }

    /// The element tapped on a level's plan at `p` (feet) within `tol` feet: the one whose
    /// footprint holds it (or whose run passes nearest), smallest first.
    pub fn pick_plan(&self, level: &str, p: [f64; 2], tol: f64, hidden: &BTreeSet<String>) -> Option<usize> {
        let mut best: Option<(f64, f64, usize)> = None;
        for (i, e) in self.elements.iter().enumerate() {
            if hidden.contains(&e.trade) || !e.level.eq_ignore_ascii_case(level) {
                continue;
            }
            let d = if e.path.len() >= 2 {
                let half = e.size_in.map_or(0.0, |s| s[0].max(0.0) / 24.0);
                (path_dist(&e.path, p) - half).max(0.0)
            } else {
                box_dist(e.min, e.max, p)
            };
            if d > tol {
                continue;
            }
            let area = (e.max[0] - e.min[0]).abs() * (e.max[1] - e.min[1]).abs();
            // Rooms' walls and slabs are large; what was tapped is the small thing on them.
            if best.is_none_or(|(bd, ba, _)| d < bd - 1e-9 || ((d - bd).abs() <= 1e-9 && area < ba)) {
                best = Some((d, area, i));
            }
        }
        best.map(|(_, _, i)| i)
    }

    /// The element a ray from `origin` along `dir` hits first, and how far along.
    pub fn pick_ray(&self, origin: [f64; 3], dir: [f64; 3], level: Option<&str>, hidden: &BTreeSet<String>) -> Option<(usize, f64)> {
        let mut best: Option<(usize, f64)> = None;
        for (i, e) in self.elements.iter().enumerate() {
            if hidden.contains(&e.trade) || level.is_some_and(|l| !e.level.eq_ignore_ascii_case(l)) {
                continue;
            }
            if let Some(t) = ray_box(origin, dir, e.min, e.max)
                && best.is_none_or(|(_, bt)| t < bt)
            {
                best = Some((i, t));
            }
        }
        best
    }

    /// Element count by trade.
    pub fn trade_counts(&self) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for e in &self.elements {
            *m.entry(e.trade.clone()).or_insert(0) += 1;
        }
        m
    }

    /// The notes not yet sent back.
    pub fn unsynced(&self) -> Vec<FieldNote> {
        self.notes.iter().filter(|n| !n.synced).cloned().collect()
    }

    /// Marks notes as sent back.
    pub fn mark_synced(&mut self, ids: &[String]) {
        let ids: BTreeSet<&String> = ids.iter().collect();
        self.notes.iter_mut().filter(|n| ids.contains(&n.id)).for_each(|n| n.synced = true);
    }

    /// Adds a note made on this device, giving it the next id for `device`.
    pub fn add_note(&mut self, device: &str, mut note: FieldNote) -> Option<String> {
        if self.notes.len() >= MAX_NOTES {
            return None;
        }
        let tag: String = device.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect();
        let tag = if tag.is_empty() { "dev".to_string() } else { tag };
        let prefix = format!("FN-{tag}-");
        let next = self.notes.iter().filter_map(|n| n.id.strip_prefix(&prefix)?.parse::<u32>().ok()).max().map_or(1, |n| n.saturating_add(1));
        note.id = format!("{prefix}{next:04}");
        if note.room.is_empty() {
            note.room =
                self.room_at(&note.level, [note.at[0], note.at[1]]).and_then(|i| self.rooms.get(i)).map(|r| r.number.clone()).unwrap_or_default();
        }
        clean_note(&mut note);
        self.notes.push(note);
        self.notes.last().map(|n| n.id.clone())
    }
}

/// Even-odd point in polygon.
pub fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0];
            if p[0] < x {
                c = !c;
            }
        }
    }
    c
}

fn seg_dist(a: [f64; 3], b: [f64; 3], p: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 1e-12 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (x, y) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    (x * x + y * y).sqrt()
}

fn path_dist(path: &[[f64; 3]], p: [f64; 2]) -> f64 {
    path.windows(2).filter_map(|w| Some(seg_dist(*w.first()?, *w.get(1)?, p))).fold(f64::INFINITY, f64::min)
}

fn box_dist(min: [f64; 3], max: [f64; 3], p: [f64; 2]) -> f64 {
    let dx = (min[0] - p[0]).max(p[0] - max[0]).max(0.0);
    let dy = (min[1] - p[1]).max(p[1] - max[1]).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

/// Where a ray first enters a box (slab method), if it does in front of the origin.
pub fn ray_box(o: [f64; 3], d: [f64; 3], min: [f64; 3], max: [f64; 3]) -> Option<f64> {
    let (mut t0, mut t1) = (0.0_f64, f64::INFINITY);
    for i in 0..3 {
        let (oi, di, lo, hi) = (*o.get(i)?, *d.get(i)?, *min.get(i)?, *max.get(i)?);
        if di.abs() < 1e-12 {
            if oi < lo || oi > hi {
                return None;
            }
            continue;
        }
        let (mut a, mut b) = ((lo - oi) / di, (hi - oi) / di);
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    t0.is_finite().then_some(t0)
}

/// An orbit or walk camera for the viewer: eye, the point looked at, vertical field of view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub fov_deg: f64,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    if l > 1e-12 { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0, 1.0, 0.0] }
}

impl Camera {
    /// Looking at `target` from `dist` feet away, `yaw` degrees round from east, `pitch` up.
    pub fn orbit(target: [f64; 3], dist: f64, yaw_deg: f64, pitch_deg: f64) -> Camera {
        let (y, p) = (yaw_deg.to_radians(), pitch_deg.clamp(-89.0, 89.0).to_radians());
        let d = dist.max(0.5);
        let eye = [target[0] + d * p.cos() * y.cos(), target[1] + d * p.cos() * y.sin(), target[2] + d * p.sin()];
        Camera { eye, target, fov_deg: 50.0 }
    }

    /// Standing at `at` (eye height above `floor_z`), facing `heading_deg` from east.
    pub fn walk(at: [f64; 2], floor_z: f64, heading_deg: f64) -> Camera {
        let h = heading_deg.to_radians();
        let eye = [at[0], at[1], floor_z + 5.5];
        Camera { eye, target: [at[0] + h.cos(), at[1] + h.sin(), eye[2]], fov_deg: 70.0 }
    }

    fn basis(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let f = unit(sub(self.target, self.eye));
        let r = unit(cross(f, [0.0, 0.0, 1.0]));
        let u = cross(r, f);
        (f, r, u)
    }

    /// A world point on a `w`×`h` screen (pixels, y down) and its depth; none behind the eye.
    pub fn project(&self, p: [f64; 3], w: f64, h: f64) -> Option<(f64, f64, f64)> {
        let (f, r, u) = self.basis();
        let v = sub(p, self.eye);
        let z = dot(v, f);
        if z < 0.1 {
            return None;
        }
        let k = (h * 0.5) / (self.fov_deg.clamp(5.0, 150.0).to_radians() * 0.5).tan();
        Some((w * 0.5 + dot(v, r) / z * k, h * 0.5 - dot(v, u) / z * k, z))
    }

    /// The ray through screen point (`x`, `y`) of a `w`×`h` screen.
    pub fn ray(&self, x: f64, y: f64, w: f64, h: f64) -> ([f64; 3], [f64; 3]) {
        let (f, r, u) = self.basis();
        let k = (h * 0.5) / (self.fov_deg.clamp(5.0, 150.0).to_radians() * 0.5).tan();
        let (sx, sy) = ((x - w * 0.5) / k, (h * 0.5 - y) / k);
        (self.eye, unit([f[0] + r[0] * sx + u[0] * sy, f[1] + r[1] * sx + u[1] * sy, f[2] + r[2] * sx + u[2] * sy]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg() -> FieldPackage {
        let mut p = FieldPackage::new("Project A");
        p.levels = vec![FieldLevel { name: "L2".into(), elevation_ft: 12.0, height_ft: 10.0 }];
        p.trades = ["arch", "mech", "elec"].iter().map(|k| trade_info(k)).collect();
        let room = |n: &str, name: &str, x: f64| FieldRoom {
            number: n.into(),
            name: name.into(),
            level: "L2".into(),
            outline: vec![[x, 0.0], [x + 13.0, 0.0], [x + 13.0, 30.0], [x, 30.0]],
            at: [x + 6.5, 15.0],
            unit: String::new(),
        };
        p.rooms = vec![room("201", "King", 0.0), room("202", "King", 13.0), room("210", "Suite", 26.0), room("21", "Linen", 52.0)];
        p.types = vec![FieldType {
            mark: "H".into(),
            category: "luminaire".into(),
            fields: vec![FieldSpec {
                key: "model".into(),
                value: "LX-200".into(),
                source: "E-601 fixture schedule".into(),
                confidence: "stated".into(),
            }],
        }];
        let el = |id: &str, trade: &str, kind: &str, min: [f64; 3], max: [f64; 3]| FieldElement {
            id: id.into(),
            trade: trade.into(),
            kind: kind.into(),
            level: "L2".into(),
            min,
            max,
            ..FieldElement::default()
        };
        let mut light = el("1A", "elec", "device", [5.0, 10.0, 20.5], [6.0, 11.0, 21.0]);
        light.mark = "H".into();
        let mut duct = el("2B", "mech", "duct", [0.0, 24.0, 21.0], [40.0, 26.0, 22.0]);
        duct.path = vec![[0.0, 25.0, 21.5], [40.0, 25.0, 21.5]];
        duct.size_in = Some([24.0, 12.0]);
        p.elements = vec![el("W1", "arch", "wall", [0.0, -0.25, 12.0], [13.0, 0.25, 21.0]), light, duct];
        p.issues = vec![
            FieldIssue {
                id: "CL-0001".into(),
                kind: IssueKind::Clash,
                title: "duct below ceiling".into(),
                status: "new".into(),
                level: "L2".into(),
                at: [6.0, 25.0, 21.0],
                ..FieldIssue::default()
            },
            FieldIssue {
                id: "RFI-001".into(),
                kind: IssueKind::Rfi,
                title: "wall type".into(),
                status: "answered".into(),
                level: "L2".into(),
                at: [6.0, 5.0, 12.0],
                ..FieldIssue::default()
            },
        ];
        p.assign_rooms();
        p
    }

    #[test]
    fn room_search_ranks_number_then_prefix_then_name() {
        let p = pkg();
        let hits: Vec<(String, MatchRank)> =
            p.search_rooms("21").iter().map(|(i, r)| (p.rooms.get(*i).map(|x| x.number.clone()).unwrap_or_default(), *r)).collect();
        assert_eq!(hits, vec![("21".into(), MatchRank::Number), ("210".into(), MatchRank::NumberPrefix)]);
        assert_eq!(p.search_rooms(" 2-01 ").first().map(|h| h.1), Some(MatchRank::Number));
        let suite = p.search_rooms("suite");
        assert_eq!(suite.len(), 1);
        assert_eq!(suite.first().map(|h| h.1), Some(MatchRank::Name));
        assert!(p.search_rooms("   ").is_empty());
    }

    #[test]
    fn elements_and_issues_land_in_their_rooms() {
        let p = pkg();
        let r201 = p.rooms.first().cloned().unwrap_or_default();
        let none = BTreeSet::new();
        let ids: Vec<&str> = p.elements_in_room(&r201, &none).filter_map(|i| p.elements.get(i)).map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["W1", "1A"], "the 40' duct centres in the next room");
        let hidden: BTreeSet<String> = ["elec".to_string()].into();
        assert_eq!(p.elements_in_room(&r201, &hidden).count(), 1);
        // The answered RFI is closed; the new clash is open.
        let open: Vec<&str> = p.open_issues(&r201).map(|i| i.id.as_str()).collect();
        assert_eq!(open, vec!["CL-0001"]);
    }

    #[test]
    fn tap_picks_the_small_thing_and_its_type() {
        let p = pkg();
        let none = BTreeSet::new();
        let i = p.pick_plan("L2", [5.5, 10.5], 0.5, &none);
        let e = i.and_then(|i| p.elements.get(i));
        assert_eq!(e.map(|e| e.id.as_str()), Some("1A"));
        let ty = e.and_then(|e| p.type_of(e));
        let f = ty.and_then(|t| t.fields.first());
        assert_eq!(f.map(|f| (f.value.as_str(), f.source.as_str())), Some(("LX-200", "E-601 fixture schedule")));
        // A run is picked by its width round its centreline.
        assert_eq!(p.pick_plan("L2", [30.0, 25.9], 0.2, &none).and_then(|i| p.elements.get(i)).map(|e| e.id.as_str()), Some("2B"));
        // Hidden trades and other levels aren't picked.
        let hidden: BTreeSet<String> = ["mech".to_string()].into();
        assert_eq!(p.pick_plan("L2", [30.0, 25.9], 0.2, &hidden), None);
        assert_eq!(p.pick_plan("L3", [5.5, 10.5], 0.5, &none), None);
    }

    #[test]
    fn ray_picks_the_nearest_box_and_camera_round_trips() {
        let p = pkg();
        let none = BTreeSet::new();
        // Looking straight up from below the light.
        let hit = p.pick_ray([5.5, 10.5, 14.0], [0.0, 0.0, 1.0], Some("L2"), &none);
        assert_eq!(hit.and_then(|(i, _)| p.elements.get(i)).map(|e| e.id.as_str()), Some("1A"));
        assert!(hit.is_some_and(|(_, t)| (t - 6.5).abs() < 1e-9));
        assert_eq!(ray_box([0.0; 3], [1.0, 0.0, 0.0], [-2.0, 1.0, -1.0], [-1.0, 2.0, 1.0]), None);
        let cam = Camera::orbit([10.0, 10.0, 15.0], 60.0, 225.0, 30.0);
        let (x, y, _) = cam.project([5.5, 10.5, 20.75], 800.0, 600.0).unwrap_or_default();
        let (o, d) = cam.ray(x, y, 800.0, 600.0);
        let t = ray_box(o, d, [5.0, 10.0, 20.5], [6.0, 11.0, 21.0]);
        assert!(t.is_some(), "the ray through a projected point passes through it");
        assert_eq!(cam.project([10.0 - 200.0, 10.0 - 200.0, 15.0], 800.0, 600.0), None, "behind the eye");
        let walk = Camera::walk([6.0, 15.0], 12.0, 90.0);
        assert!((walk.eye[2] - 17.5).abs() < 1e-9);
    }

    #[test]
    fn measure_reads_feet_and_inches() {
        let m = Measured::between([0.0, 0.0, 12.0], [3.0, 4.0, 12.5]);
        assert!((m.dist - (25.25_f64).sqrt()).abs() < 1e-9);
        assert_eq!(m.text(), "5'-1/4\"  (plan 5'-0\", rise 0'-6\")");
    }

    #[test]
    fn notes_get_ids_rooms_and_merge_once() {
        let mut p = pkg();
        let n = |text: &str| FieldNote {
            level: "L2".into(),
            at: [20.0, 5.0, 12.0],
            text: text.into(),
            when: "2026-01-02T08:00".into(),
            ..FieldNote::default()
        };
        let a = p.add_note("tab 1", n("sleeve moved 6\" east"));
        let b = p.add_note("tab 1", FieldNote { kind: NoteKind::VerifiedPoint, ..n("stud at 16\" o.c. verified") });
        assert_eq!(a.as_deref(), Some("FN-tab1-0001"));
        assert_eq!(b.as_deref(), Some("FN-tab1-0002"));
        assert_eq!(p.notes.first().map(|x| x.room.as_str()), Some("202"));
        let mut drawing: Vec<FieldNote> = Vec::new();
        let first = merge_notes(&mut drawing, &p.unsynced());
        assert_eq!(first, Merged { added: 2, updated: 0, unchanged: 0 });
        let again = merge_notes(&mut drawing, &p.unsynced());
        assert_eq!(again, Merged { added: 0, updated: 0, unchanged: 2 }, "syncing twice adds nothing");
        // An edit made later on the device replaces the drawing's copy; an older one doesn't.
        let mut later = p.unsynced();
        if let Some(x) = later.first_mut() {
            x.text = "sleeve moved 8\" east".into();
            x.when = "2026-01-02T09:00".into();
        }
        assert_eq!(merge_notes(&mut drawing, &later).updated, 1);
        assert_eq!(drawing.first().map(|x| x.text.as_str()), Some("sleeve moved 8\" east"));
        let older: Vec<FieldNote> = p
            .unsynced()
            .into_iter()
            .map(|mut x| {
                x.when = "2025-12-31T00:00".into();
                x.text = "stale".into();
                x
            })
            .collect();
        assert_eq!(merge_notes(&mut drawing, &older).updated, 0);
        let ids: Vec<String> = p.unsynced().iter().map(|x| x.id.clone()).collect();
        p.mark_synced(&ids);
        assert!(p.unsynced().is_empty());
    }

    #[test]
    fn package_round_trips_and_hostile_input_is_refused_or_capped() {
        let p = pkg();
        let text = p.to_json().unwrap_or_default();
        let back = FieldPackage::from_json(text.as_bytes());
        assert_eq!(back.as_ref().ok(), Some(&p));
        assert!(FieldPackage::from_json(b"{\"schema\":\"cadaddy-field/9\"}").is_err_and(|e| e.contains("newer")));
        assert!(FieldPackage::from_json(b"{}").is_err());
        assert!(FieldPackage::from_json(b"[1,2").is_err());
        let huge = vec![b' '; MAX_PACKAGE_BYTES + 1];
        assert!(FieldPackage::from_json(&huge).is_err());
        // Inverted boxes are put right; long notes are clipped on a char boundary.
        let mut bad = p.clone();
        if let Some(e) = bad.elements.first_mut() {
            e.min = [5.0, 0.0, 0.0];
            e.max = [1.0, 1.0, 1.0];
        }
        bad.notes.push(FieldNote { id: "x".into(), text: "é".repeat(MAX_TEXT), ..FieldNote::default() });
        let back = FieldPackage::from_json(bad.to_json().unwrap_or_default().as_bytes()).unwrap_or_default();
        let e = back.elements.first().cloned().unwrap_or_default();
        assert_eq!((e.min[0], e.max[0]), (1.0, 5.0));
        assert!(back.notes.last().is_some_and(|n| n.text.len() <= MAX_TEXT));
        let notes = FieldNotes::from_json(b"{\"schema\":\"cadaddy-fieldnotes/1\",\"notes\":[{\"id\":\"\"},{\"id\":\"FN-a-0001\",\"at\":[1,2,3]}]}");
        assert_eq!(notes.map(|n| n.notes.len()).ok(), Some(1));
    }

    #[test]
    fn trades_from_kinds_and_systems() {
        assert_eq!(trade_of("duct", "M-DUCT-SUPP"), "mech");
        assert_eq!(trade_of("pipe", "P-DOMW-CW"), "plumb");
        assert_eq!(trade_of("conduit", "E-COND"), "elec");
        assert_eq!(trade_of("pipe", "F-SPRK"), "fire");
        assert_eq!(trade_of("wall", "3D-Walls"), "arch");
        assert_eq!(trade_of("equipment", "E-PANEL"), "elec");
        assert_eq!(trade_of("column", ""), "struct");
    }
}
