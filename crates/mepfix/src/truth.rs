//! The ground truth of a fixture set: everything the sheets show, as numbers. A print reader is
//! scored by comparing what it found with this.
//!
//! Coordinates are **building coordinates in feet**: x east, y north, from the intersection of the
//! first grid lines; z is the elevation above the level 1 finished floor. Sizes are **inches**.
//! Each sheet records how building coordinates map onto its drawing (model) space and its paper.
//!
//! The MEP systems are a graph: [`Node`]s (fittings, connections to terminals, equipment and
//! fixtures, open ends) joined by straight [`Segment`]s. A segment is either horizontal (it may
//! slope, for drains) or vertical (risers, drops to diffusers and heads).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The truth file's format; bumped when a field changes meaning.
pub const SCHEMA: &str = "cadaddy-mepfix/1";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Truth {
    pub schema: String,
    pub seed: u64,
    /// The made-up project's name, as the title blocks print it.
    pub project: String,
    pub building: Building,
    /// The systems on the sheets, with the abbreviation the prints use for each.
    pub systems: Vec<System>,
    pub nodes: Vec<Node>,
    pub segments: Vec<Segment>,
    pub terminals: Vec<Terminal>,
    pub equipment: Vec<Equipment>,
    pub fixtures: Vec<Fixture>,
    pub risers: Vec<Riser>,
    pub sheets: Vec<Sheet>,
    /// Every clash between the modelled elements (hard clashes, runs below a ceiling, runs into
    /// structure), computed from the geometry; `planted` ones were put there on purpose.
    pub clashes: Vec<Clash>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Building {
    pub name: String,
    pub levels: Vec<Level>,
    pub grid: Grid,
    pub rooms: Vec<Room>,
    pub columns: Vec<Column>,
    pub beams: Vec<Beam>,
    /// Concrete slab on deck, inches (top of slab = the level's elevation).
    pub slab_in: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Level {
    /// As the title blocks print it: `LEVEL 1`, `LEVEL 2`, `ROOF`.
    pub name: String,
    /// Short code used in tags and ids: `1`, `2`, `R`.
    pub code: String,
    pub elevation_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    /// Lines of constant x (they run north–south on the plans), west to east.
    pub x: Vec<GridLine>,
    /// Lines of constant y, south to north.
    pub y: Vec<GridLine>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GridLine {
    pub label: String,
    pub at_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub level: String,
    pub number: String,
    pub name: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// `None`: open to structure.
    pub ceiling: Option<Ceiling>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ceiling {
    /// Above the room's floor.
    pub height_ft: f64,
    /// `ACT` (acoustic tile in a grid) or `GYP` (gypsum board).
    pub kind: String,
    /// The tag as the reflected ceiling plan prints it.
    pub tag: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub id: String,
    /// Grid intersection, `B-2` (x line label, y line label).
    pub grid: String,
    pub at_ft: [f64; 2],
    pub size: String,
    /// Along x and along y, inches.
    pub footprint_in: [f64; 2],
    pub bottom_ft: f64,
    pub top_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Beam {
    pub id: String,
    /// The level whose floor (or the roof) it carries.
    pub level: String,
    /// The grid line it runs on.
    pub grid: String,
    pub size: String,
    pub depth_in: f64,
    pub flange_in: f64,
    pub from_ft: [f64; 2],
    pub to_ft: [f64; 2],
    /// Top of steel, elevation.
    pub tos_ft: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct System {
    /// Our code: `SA` supply air, `RA` return, `EA` exhaust, `OA` outside air, `CW` cold water,
    /// `HW` hot water, `HWR` hot water return, `SAN` sanitary, `V` vent, `G` gas, `FP` fire
    /// protection.
    pub code: String,
    pub name: String,
    /// `duct` or `pipe`.
    pub kind: String,
    /// The abbreviation the prints of this set use (varied by seed: `SA`, `S`…).
    pub printed: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    /// `elbow`, `tee`, `cross`, `transition` (rectangular size change), `reducer` (round or pipe
    /// size change), `offset` (an elbow in a vertical plane), `terminal`, `equipment`, `fixture`,
    /// `cap`, `open_end`, `service` (where a utility enters the building), `vtr` (vent through
    /// roof).
    pub kind: String,
    pub system: String,
    pub level: String,
    pub at_ft: [f64; 3],
    pub segments: Vec<String>,
    /// For `terminal`, `equipment`, `fixture`: the id of the thing it connects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connects: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub system: String,
    /// `duct` or `pipe`.
    pub kind: String,
    /// `rect`, `round` (ducts) or `pipe`.
    pub shape: String,
    /// Rectangular: width (across, on the plan) × depth; round duct and pipe: [diameter, diameter]
    /// (pipe: nominal size).
    pub size_in: [f64; 2],
    pub from: String,
    pub to: String,
    pub level: String,
    /// Centreline, from → to.
    pub path_ft: [[f64; 3]; 2],
    pub vertical: bool,
    /// Horizontal segments: how high it hangs and where that came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<Height>,
    /// The size tag, if the sheet tags this segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<Tag>,
    /// Drainage slope as printed (`1/4"/FT`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slope: Option<String>,
    /// Drawn on the plan as hidden (below the floor).
    #[serde(default)]
    pub below_floor: bool,
    /// The sheet it is drawn on.
    pub sheet: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Height {
    /// Bottom of the duct or pipe above the level's floor, feet.
    pub bottom_ft: f64,
    pub top_ft: f64,
    pub centre_ft: f64,
    /// The note on the sheet that gives it (`BOD @ 10'-6"`), when there is one; a reader has to
    /// work out the others (ceilings, structure, rules).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    /// The lines of text as printed.
    pub text: Vec<String>,
    /// Where the tag's text sits, building feet (plan).
    pub at_ft: [f64; 2],
    /// Joined to the run by a leader line.
    pub leader: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Terminal {
    pub id: String,
    /// `supply_diffuser`, `return_grille`, `exhaust_grille`, `louver`, `sprinkler_head`.
    pub kind: String,
    /// Type tag from the schedule (`CD-2`, `RG-1`, `EG-1`, `L-1`, `SH-1`).
    pub type_tag: String,
    pub system: String,
    pub level: String,
    pub room: String,
    pub at_ft: [f64; 3],
    pub face_in: [f64; 2],
    pub neck_in: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfm: Option<f64>,
    /// The node it connects to (none for a plenum return grille).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Equipment {
    pub id: String,
    pub tag: String,
    /// `ahu`, `vav`, `exhaust_fan`, `water_heater`, `gas_meter`.
    pub kind: String,
    pub level: String,
    /// Box: minimum and maximum corners, building feet.
    pub min_ft: [f64; 3],
    pub max_ft: [f64; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfm: Option<f64>,
    /// The schedule's columns for it, as printed.
    pub schedule: BTreeMap<String, String>,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fixture {
    pub id: String,
    pub tag: String,
    /// `water_closet`, `lavatory`.
    pub kind: String,
    pub level: String,
    pub room: String,
    pub at_ft: [f64; 3],
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Riser {
    pub id: String,
    pub system: String,
    pub at_ft: [f64; 2],
    pub size_in: [f64; 2],
    pub bottom_ft: f64,
    pub top_ft: f64,
    /// The vertical segments it is made of, bottom to top.
    pub segments: Vec<String>,
    /// Per level it shows on: the symbol there (`up`: it rises from this level; `down`: it comes
    /// from below and ends or turns here; `through`: it passes) and the note printed beside it.
    pub shown: Vec<RiserMark>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RiserMark {
    pub level: String,
    pub symbol: String,
    pub note: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    /// `M-101`.
    pub number: String,
    pub title: String,
    /// `G`, `A`, `S`, `M`, `P`, `FP`.
    pub discipline: String,
    /// The level the plan shows (none for the cover and schedule sheets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    pub pdf: String,
    pub dxf: String,
    /// Width × height, inches (landscape).
    pub paper_in: [f64; 2],
    /// The plan's scale as printed (`1/8" = 1'-0"`), for plan sheets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_text: Option<String>,
    /// Enlarged plans: the part of the level they show (building feet, min and max corners).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ft: Option<[[f64; 2]; 2]>,
    /// Paper inches per building foot (1/8" = 1'-0" → 0.125).
    pub scale: f64,
    /// Building feet → drawing (model space, inches): rotate by `rotation_deg` about the origin
    /// after scaling to inches, then add `offset_in`.
    pub model_from_building: Placement,
    /// Drawing (model inches) → paper inches: `(model - origin_in) * scale`, paper origin at the
    /// sheet's lower-left corner.
    pub paper_from_model: PaperMap,
    /// The legend's entries as printed (symbol name and its text).
    pub legend: Vec<LegendItem>,
    /// Abbreviations listed on the sheet.
    pub abbreviations: Vec<LegendItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub rotation_deg: f64,
    pub offset_in: [f64; 2],
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaperMap {
    pub origin_in: [f64; 2],
    pub scale: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LegendItem {
    pub symbol: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Clash {
    pub id: String,
    /// `hard` (two elements overlap), `below_ceiling` (a run hangs below the room's ceiling),
    /// `into_structure` (a run reaches into a beam).
    pub kind: String,
    /// Element ids (segments, equipment, beams, rooms).
    pub a: String,
    pub b: String,
    /// The middle of the overlap.
    pub at_ft: [f64; 3],
    /// How far they overlap, inches (the smallest of the overlap's sizes).
    pub depth_in: f64,
    pub planted: bool,
}

impl Truth {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn segment(&self, id: &str) -> Option<&Segment> {
        self.segments.iter().find(|s| s.id == id)
    }
    pub fn level(&self, code: &str) -> Option<&Level> {
        self.building.levels.iter().find(|l| l.code == code)
    }
    pub fn system(&self, code: &str) -> Option<&System> {
        self.systems.iter().find(|s| s.code == code)
    }
    /// The segments of one system kind (`duct` / `pipe`).
    pub fn segments_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Segment> + 'a {
        self.segments.iter().filter(move |s| s.kind == kind)
    }
}

impl Segment {
    /// Length, feet.
    pub fn length_ft(&self) -> f64 {
        let [a, b] = self.path_ft;
        ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
    }
    /// The size as tags print it: `24x14`, `12"Ø`, `2"`.
    pub fn size_label(&self) -> String {
        match self.shape.as_str() {
            "rect" => format!("{}x{}", fmt_in(self.size_in[0]), fmt_in(self.size_in[1])),
            "round" => format!("{}\"Ø", fmt_in(self.size_in[0])),
            _ => format!("{}\"", pipe_size(self.size_in[0])),
        }
    }
}

/// Whole inches without a decimal point.
pub fn fmt_in(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{v}") }
}

/// A nominal pipe size as printed: `3/4`, `1`, `1-1/4`, `1-1/2`, `2`, `2-1/2`.
pub fn pipe_size(v: f64) -> String {
    let whole = v.trunc();
    let frac = v - whole;
    let f = [(0.25, "1/4"), (0.5, "1/2"), (0.75, "3/4")].iter().find(|(x, _)| (frac - x).abs() < 1e-6).map(|(_, t)| *t);
    match (whole as i64, f) {
        (0, Some(t)) => t.to_string(),
        (w, Some(t)) => format!("{w}-{t}"),
        (w, None) => format!("{w}"),
    }
}

/// Feet as printed on prints: `10'-6"`, `9'-0"`, `8'-7 1/2"`.
pub fn feet_inches(ft: f64) -> String {
    let neg = ft < 0.0;
    let total = (ft.abs() * 24.0).round() / 2.0; // half inches
    let feet = (total / 12.0).floor();
    let inches = total - feet * 12.0;
    let i = if (inches - inches.round()).abs() < 1e-9 { format!("{}", inches.round() as i64) } else { format!("{} 1/2", inches.floor() as i64) };
    format!("{}{}'-{}\"", if neg { "-" } else { "" }, feet as i64, i)
}
