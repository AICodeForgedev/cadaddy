//! The building spec: everything a plan drawing needs said about it to become a 3D building,
//! versioned and kept in the drawing itself (the `CADADDY_BSPEC` entry of its named-object
//! dictionary, so it travels in DXF and DWG). It grows Walk mode's [`Rules`] (which it converts
//! to with [`BuildingSpec::to_rules`]) with:
//!
//! - **levels**: the plan's region of the sheet (or its own sheet), the base point that maps to
//!   the building origin, finished floor elevation, floor to floor and default ceiling height;
//! - **layer roles**: what each layer's objects are (wall, door, window, curtain wall, column,
//!   stair, ceiling grid, fixture, room tag, device, run, ignore), a wall layer naming its type;
//! - **wall types**: height, thickness, and whether the plan draws both faces (line pairs) or the
//!   centreline; a type read from a wall type legend also carries its **assembly** (each board,
//!   stud, insulation… layer with its material, thickness and side; height rule; fire and sound
//!   ratings);
//! - **walls**: the type and height of each wall read off the plans (its tag, or a flagged guess);
//! - **rooms**: per-room ceiling heights and materials, found by their room tag text;
//! - **door and window families**: blocks by name pattern, sized from the block or its
//!   attributes, with head and sill heights;
//! - **materials** per surface for the building, overridden per room;
//! - **site**: a ground plane around the building.
//!
//! The JSON shape is the struct shape below (snake case). `version` is [`SPEC_VERSION`]; a spec
//! from a newer CADaddy is refused rather than half read. Lengths in feet end in `_ft`, in inches
//! in `_in`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::rules::{DeviceRules, LayerKind, LayerRule, Level, Openings, Rules, layer_match};

/// The spec version this CADaddy reads and writes.
pub const SPEC_VERSION: u32 = 1;
/// Most items of each list a spec may hold (hostile input).
pub const MAX_ITEMS: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildingSpec {
    #[serde(default = "one")]
    pub version: u32,
    /// Drawing units per foot (12 for an inch drawing).
    #[serde(default = "twelve")]
    pub units_per_foot: f64,
    #[serde(default)]
    pub levels: Vec<Level>,
    /// Elevations the prints give for levels with no plan here (the roof, a parapet, a penthouse
    /// drawn elsewhere): they top the floor below and become IFC storeys (`levels.read`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub datums: Vec<crate::levels::Datum>,
    /// The sheets of the set and the level each shows (`levels.sheets`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sheets: Vec<crate::levels::SheetLink>,
    /// What each element is from the documents, with sources and open issues (`tags.list`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<crate::tags::ElementTag>,
    /// The estimate's state (`estimate.*`): the imported baseline and prices, each line's
    /// assembly and scope with reasons, and the markup settings.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub estimate: serde_json::Value,
    /// Field progress (`progress.*`): the plan's driver quantities, installed-work marks and
    /// timecard hours. Production rates are never kept here: they go to the company's private
    /// field observations.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub progress: serde_json::Value,
    #[serde(default)]
    pub layers: Vec<LayerRole>,
    #[serde(default)]
    pub wall_types: Vec<WallType>,
    #[serde(default)]
    pub rooms: Vec<Room>,
    #[serde(default)]
    pub families: Vec<Family>,
    /// The building's materials by surface (rooms override theirs).
    #[serde(default)]
    pub materials: Surfaces,
    #[serde(default)]
    pub site: Option<Site>,
    /// Stairs and ramps between the levels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stairs: Vec<Stair>,
    /// Facades, roofs and what stands on them, from the elevations and the roof plan.
    #[serde(default, skip_serializing_if = "Exterior::is_empty")]
    pub exterior: Exterior,
    /// Ceilings, soffits and what is set in them, from the reflected ceiling plans.
    #[serde(default, skip_serializing_if = "Rcp::is_empty")]
    pub rcp: Rcp,
    /// Ceiling build-ups from the ceiling legend: what an RCP ceiling of each type is made of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ceiling_types: Vec<CeilingType>,
    /// Soffit build-ups from their details.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub soffit_types: Vec<SoffitType>,
    /// Duct, pipe and conduit systems and their runs.
    #[serde(default, skip_serializing_if = "Mep::is_empty")]
    pub mep: Mep,
    /// Concrete: slabs, footings, piers, equipment pads, encasement; pad decisions and pours.
    #[serde(default, skip_serializing_if = "crate::concrete::Concrete::is_empty")]
    pub concrete: crate::concrete::Concrete,
    /// Columns at structural grid intersections (`grids.columns`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<GridColumn>,
    /// Door and window schedules: their rows, and the tags on the plans that name them.
    #[serde(default, skip_serializing_if = "Schedule::is_empty")]
    pub schedule: Schedule,
    /// Walls whose type (and height) the plans give: the wall built on each centreline gets them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub walls: Vec<WallMark>,
    /// Gaps and conflicts in the prints the readers found (with the sheets involved), and their
    /// status and answers: see [`crate::rfi`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rfis: Vec<crate::rfi::Rfi>,
    /// Procurement: releases, the submittal log, lead times, order status (the engine's
    /// `procure.*`, kept with the model it orders for).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procurement: Option<serde_json::Value>,
    /// What the documents say, for the print validator ([`crate::validate`]): symbols, schedules,
    /// dimension strings, rooms, the rules file and its gate setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prints: Option<Box<crate::validate::Prints>>,
    /// Devices (fixture and device layers): types, mounting heights, attributes.
    #[serde(default)]
    pub devices: DeviceRules,
    /// Drawings that only add devices (see [`Rules::device_sheets`]).
    #[serde(default)]
    pub device_sheets: Vec<String>,
    /// Openings found as gaps in walls, and the default door head and window sill and head.
    #[serde(default)]
    pub openings: Openings,
    /// The parts a person has checked and approved, by key ([`Described::key`]: `levels/L1`,
    /// `layers/A-WALL`, `materials`…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approved: Vec<String>,
    /// Parts whose values were guessed (by `bspec.draft`), by key, with why: what a person or an
    /// agent should confirm.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub guess: BTreeMap<String, String>,
}

/// One part of a spec in plain language: its section, key (for approval and editing) and text.
#[derive(Clone, Debug, PartialEq)]
pub struct Described {
    pub section: &'static str,
    /// `levels/L1`, `layers/A-WALL`, `wall_types/Partition`, `rooms/Office`, `families/Door`,
    /// `materials`, `site`, `openings`.
    pub key: String,
    pub text: String,
    pub approved: bool,
    /// Why its values are a guess, when they are.
    pub guess: Option<String>,
}

/// What a layer's objects are. `layer` is a pattern: `*` matches any run of characters, case
/// doesn't matter; the first matching role wins.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerRole {
    pub layer: String,
    pub role: Role,
    /// A wall or curtain wall layer's type ([`WallType::name`]); none = line pairs, floor to floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_type: Option<String>,
    /// A run's height above the floor, feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_ft: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Wall faces (or centrelines, by the wall type).
    Wall,
    /// Door blocks or swings.
    Door,
    /// Window blocks.
    Window,
    /// Lines that stand up as glass from floor to wall top.
    CurtainWall,
    /// Closed outlines and circles that stand up from floor to wall top.
    Column,
    Stair,
    CeilingGrid,
    /// Block references placed as devices by their type (lights, receptacles…).
    Fixture,
    /// Room names and numbers: the text that finds each room's outline.
    RoomTag,
    /// Block references placed as devices.
    Device,
    /// Lines drawn as runs (conduit, cable) `height_ft` above the floor.
    Run,
    /// Not part of the building.
    Ignore,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallFrom {
    /// The plan draws both faces; a wall is a pair of parallel lines its thickness apart.
    #[default]
    Pairs,
    /// The plan draws one line down the middle; the wall is `thickness_in` thick about it.
    Centerline,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WallType {
    pub name: String,
    /// Wall height above the floor, feet; none = floor to floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_ft: Option<f64>,
    /// Thickness, inches: the wall's for centrelines; for line pairs the faces give it.
    #[serde(default = "six")]
    pub thickness_in: f64,
    #[serde(default)]
    pub from: WallFrom,
    /// What the wall is made of, from the wall type legend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly: Option<Assembly>,
}

/// Most layers one wall assembly has (hostile input).
pub const MAX_ASSEMBLY_LAYERS: usize = 64;

/// A wall type's build-up as the wall type legend (partition schedule) gives it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Assembly {
    /// The tag the plans print on the walls (`1B`, `P3`, `W-4`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tag: String,
    /// The legend's title for it (`1 HOUR RATED CORRIDOR PARTITION`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The layers from side 1's face to side 2's (cavity insulation where the studs are).
    #[serde(default)]
    pub layers: Vec<AssemblyLayer>,
    /// How high the wall goes; none = the wall type's height (or floor to floor).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<HeightRule>,
    /// How far past the ceiling an [`HeightRule::AboveCeiling`] wall goes, inches.
    #[serde(default = "six", skip_serializing_if = "is_six")]
    pub above_ceiling_in: f64,
    /// `1 HR`, `2 HR`; none = not rated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_rating: Option<String>,
    /// Sound rating as printed: `STC 50`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acoustic_rating: Option<String>,
    /// An exterior wall.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub external: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// How its head meets what is above, and its bottom track (head of wall details).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Head>,
}

/// A wall type's head and bottom, as its head of wall detail gives them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Head {
    pub track: HeadTrack,
    /// What the head goes up to.
    pub to: HeadTo,
    /// The movement the head takes (deflection and double tracks), inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deflection_in: Option<f64>,
    /// The wall stands on a bottom track (stud walls).
    #[serde(default)]
    pub bottom_track: bool,
    /// The detail it comes from (`5`, `5/A-502`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadTrack {
    /// A top track, the wall braced to the structure above.
    Single,
    /// A slotted deflection track.
    Deflection,
    /// A nested double track.
    Double,
    /// Steel angles each side (masonry).
    Angle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadTo {
    /// The underside of the deck or structure.
    Deck,
    Ceiling,
}

impl Head {
    pub fn text(&self) -> String {
        let track = match self.track {
            HeadTrack::Single => "top track",
            HeadTrack::Deflection => "slotted deflection track",
            HeadTrack::Double => "double (nested) track",
            HeadTrack::Angle => "steel angles each side",
        };
        let to = match self.to {
            HeadTo::Deck => "at the deck",
            HeadTo::Ceiling => "at the ceiling",
        };
        let d = self.deflection_in.map(|d| format!(", {} deflection", inches(d))).unwrap_or_default();
        let b = if self.bottom_track { ", bottom track" } else { "" };
        format!("head: {track} {to}{d}{b}")
    }
}

/// A ceiling build-up from the ceiling legend.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CeilingType {
    /// Its tag (`C2`, `GYP-1`): RCP ceilings name it.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub kind: CeilingKind,
    /// From the room's face up: boards or tile, then their framing (with its spacing).
    #[serde(default)]
    pub layers: Vec<AssemblyLayer>,
    /// Tile size, inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_in: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_rating: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CeilingKind {
    /// Gypsum board on framing.
    #[default]
    Gypsum,
    /// Tile in a suspended grid.
    Grid,
    /// No ceiling: the structure exposed.
    Open,
}

/// A soffit build-up from its detail.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SoffitType {
    /// Its tag (the detail number): RCP soffits name it.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The bottom's boards, room face first.
    #[serde(default)]
    pub bottom: Vec<AssemblyLayer>,
    /// The vertical faces' boards, room face first.
    #[serde(default)]
    pub sides: Vec<AssemblyLayer>,
    /// Studs (vertical, on its sides) and channels or runners (across its bottom).
    #[serde(default)]
    pub framing: Vec<AssemblyLayer>,
    #[serde(default)]
    pub hangs_from: Hang,
    /// How far the side boards go past the ceiling, inches (soffits hung from the structure).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub above_ceiling_in: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hang {
    /// Built under the ceiling, which runs on above it.
    #[default]
    Ceiling,
    /// Hung from the structure: the ceiling stops at its sides.
    Structure,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl Assembly {
    /// The overall thickness, inches: every layer but the insulation inside the stud cavity.
    pub fn thickness_in(&self) -> f64 {
        self.layers.iter().filter(|l| !l.in_cavity).map(|l| l.thickness_in).sum()
    }
}

/// One layer of a wall assembly.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AssemblyLayer {
    /// What it is, in plain words: `5/8" Type X gypsum board`, `3 5/8" 20 ga metal studs @ 16" o.c.`.
    pub material: String,
    pub thickness_in: f64,
    #[serde(default)]
    pub side: LayerSide,
    /// `board`, `framing`, `insulation`, `furring`, `masonry`, `sheathing`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// Insulation filling the stud cavity: it takes no thickness of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_cavity: bool,
    /// Framing's spacing, inches (`@ 16" o.c.`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spacing_in: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerSide {
    /// Side 1's finish (the exterior side of an exterior wall).
    Side1,
    /// The studs, masonry or cavity.
    #[default]
    Core,
    Side2,
}

/// How high a wall type goes, as the legend says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeightRule {
    /// To the ceiling.
    Ceiling,
    /// Past the ceiling by [`Assembly::above_ceiling_in`] (6" usually).
    AboveCeiling,
    /// To the underside of the deck or structure above.
    Deck,
    /// Full height: floor to the structure above.
    Full,
}

impl HeightRule {
    pub fn text(self, above_in: f64) -> String {
        match self {
            HeightRule::Ceiling => "to the ceiling".into(),
            HeightRule::AboveCeiling => format!("{} above the ceiling", inches(above_in)),
            HeightRule::Deck => "to the underside of the deck".into(),
            HeightRule::Full => "full height".into(),
        }
    }
}

/// A wall the plans give the type of: its centreline on the level's plan (drawing units), the
/// wall type ([`WallType::name`]) and the height its rule gives there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WallMark {
    pub level: String,
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub wall_type: String,
    /// Height above the floor, feet; none = the wall type's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_ft: Option<f64>,
    /// Why the type is a guess (no tag on the wall), when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guess: Option<String>,
}

fn is_six(v: &f64) -> bool {
    (*v - 6.0).abs() < 1e-9
}

/// A room, found by its tag text (`name`, or `number`) on the plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    /// The level it's on; none = whichever level's region holds its tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// Ceiling height above the floor, feet; none = the level's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling_ft: Option<f64>,
    #[serde(default)]
    pub materials: Surfaces,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FamilyKind {
    Door,
    Window,
}

/// A door or window family: the blocks it covers and how big and how high they are.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Family {
    pub name: String,
    pub kind: FamilyKind,
    /// Block name pattern (`*` wildcards).
    pub block: String,
    /// Width, inches; none = the attribute `width_attr`, else the block's size along its X axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_in: Option<f64>,
    /// Attribute holding the width in inches (`36`, `3'-0"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_attr: Option<String>,
    /// Head height above the floor, feet; none = the spec's `openings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ft: Option<f64>,
    /// Sill height above the floor, feet (windows); none = the spec's `openings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sill_ft: Option<f64>,
    /// Attribute holding the opening height in inches (head = sill + height).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_attr: Option<String>,
}

/// A stair or ramp: the level it starts on, the level it reaches, its runs on the plan (bottom
/// to top) and its width. `stairs.detect` fills these in from the plan; a person or an agent
/// confirms them (`stairs.confirm`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stair {
    pub name: String,
    #[serde(default)]
    pub ramp: bool,
    pub level: String,
    /// The level it reaches; none = the next one up (else one floor to floor higher).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_level: Option<String>,
    pub runs: Vec<crate::stairs::StairRun>,
    #[serde(default = "forty_four")]
    pub width_in: f64,
    #[serde(default)]
    pub confirmed: bool,
    /// The plan objects it was found from (hex handles).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<String>,
}

/// The outside of the building: facades read from the elevations, roofs from the roof plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Exterior {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub facades: Vec<Facade>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roofs: Vec<Roof>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub crickets: Vec<Cricket>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub equipment: Vec<Equipment>,
}

impl Exterior {
    pub fn is_empty(&self) -> bool {
        self.facades.is_empty() && self.roofs.is_empty() && self.crickets.is_empty() && self.equipment.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    North,
    South,
    East,
    West,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::North => "North",
            Side::South => "South",
            Side::East => "East",
            Side::West => "West",
        }
    }
}

/// One facade as its elevation shows it: bands of glass (storefront, curtain wall) and of
/// materials, by height above the ground floor and, for glass, along the facade from its left
/// end as seen in the elevation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Facade {
    pub side: Side,
    #[serde(default)]
    pub bands: Vec<Band>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Band {
    /// `glass` (storefront or curtain wall) or `material`.
    pub kind: BandKind,
    pub from_ft: f64,
    pub to_ft: f64,
    /// Along the facade from its left end as seen, feet; none = the whole facade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ft: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ft: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    /// What the band is built of, outside in (an exterior wall assembly over the backup wall).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly: Option<Assembly>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandKind {
    Glass,
    Material,
}

/// A roof area on the roof plan: its outline (sheet units, on its level's plan), the way it
/// slopes and its parapet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Roof {
    pub name: String,
    pub level: String,
    pub outline: Vec<[f64; 2]>,
    /// Downhill direction on the plan and the fall per foot; none = flat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slope: Option<RoofSlope>,
    /// Parapet height above the roof's high point, feet; none = no parapet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parapet_ft: Option<f64>,
    /// What the roof is built of, top down (membrane, cover board, insulation, deck).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly: Option<Assembly>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoofSlope {
    pub down: [f64; 2],
    /// Rise per foot of run (¼"/ft = 0.0208, 4:12 = 0.333).
    pub rise_per_ft: f64,
}

/// A cricket: a raised ridge diverting water, its outline on the roof and its height.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cricket {
    pub level: String,
    pub outline: Vec<[f64; 2]>,
    pub height_ft: f64,
}

/// Rooftop equipment (RTU, AHU, exhaust fan) as a box over its outline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Equipment {
    pub name: String,
    pub level: String,
    pub outline: Vec<[f64; 2]>,
    pub height_ft: f64,
}

/// Door and window schedules read from the sheets (`schedules.read`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<ScheduleRow>,
    /// The tags on the plans (a mark at a place: the opening nearest it is that row's).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<OpeningTag>,
}

impl Schedule {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.tags.is_empty()
    }

    /// The row a mark names (any case).
    pub fn row(&self, mark: &str) -> Option<&ScheduleRow> {
        self.rows.iter().find(|r| r.mark.trim().eq_ignore_ascii_case(mark.trim()))
    }
}

/// One row of a door or window schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduleRow {
    pub kind: FamilyKind,
    pub mark: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_in: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_in: Option<f64>,
    /// Window sill and head above the floor, feet (a door's head is its height).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sill_ft: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ft: Option<f64>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_rating: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware: Option<String>,
}

impl ScheduleRow {
    /// Head and sill above the floor, feet: a door from the floor to its height; a window from
    /// its sill (else `default_sill`) to its head (else sill + height).
    pub fn heights(&self, default_sill: f64) -> (Option<f64>, Option<f64>) {
        match self.kind {
            FamilyKind::Door => (self.head_ft.or(self.height_in.map(|h| h / 12.0)), None),
            FamilyKind::Window => {
                let sill = self.sill_ft.unwrap_or(default_sill);
                (self.head_ft.or(self.height_in.map(|h| sill + h / 12.0)), Some(sill))
            }
        }
    }
}

/// A door or window tag on a plan: its mark and where it is (sheet units).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpeningTag {
    pub mark: String,
    pub at: [f64; 2],
}

/// A structural column at a grid intersection: its outline's size and shape, from its level's
/// floor up through `to_level` (none: its own storey).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridColumn {
    pub level: String,
    /// The intersection's label, `A-1`.
    pub grid: String,
    pub at: [f64; 2],
    pub size_in: [f64; 2],
    #[serde(default)]
    pub round: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_level: Option<String>,
}

/// Mechanical, electrical and plumbing runs drawn single-line on the plans.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mep {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub systems: Vec<MepSystem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<MepRun>,
    /// Air terminals (diffusers, registers, grilles) connected to duct systems.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub terminals: Vec<MepTerminal>,
    /// Equipment from the schedules (air handlers, rooftop units, VAV boxes, fans, pumps, water
    /// heaters, condensing units, meters; electrical gear: panels, switchboards, transformers,
    /// disconnects), each a box at its tag on the plan.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub equipment: Vec<MepEquipment>,
    /// What each revision of the prints changed, run by run, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<MepRevision>,
    /// Electrical feeders between gear (the feeder schedule, the one-line): what lands on each
    /// gear's lugs and what comes in through its entry plates.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub feeders: Vec<Feeder>,
    /// The tables gear is checked with (conduit sizes, conductor areas, wire-bending space),
    /// when a project's differ from the defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gear_rules: Option<serde_json::Value>,
    /// Electrical devices and the panel circuit each is on (receptacles, switches, fixtures,
    /// disconnects): what their plates are labelled with.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<ElecDevice>,
    /// Arc flash study results per piece of gear (from the engineer's study: CADaddy doesn't
    /// calculate them), for its arc flash label.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub arc_flash: Vec<ArcFlash>,
    /// Labels as last printed: label id → the text it was printed with (what changed since).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub labels_printed: BTreeMap<String, String>,
    /// The grounding electrode system and bonding, as the one-line, the grounding details and
    /// the notes show them (sizes as drawn: no sizing calculations).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grounding: Option<Grounding>,
}

/// The grounding electrode system, the bonds and the telecom grounding.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grounding {
    /// The gear the grounding starts from (its ground bar): default the first switchboard.
    pub service: String,
    pub electrodes: Vec<Electrode>,
    pub bonds: Vec<Bond>,
    pub busbars: Vec<Busbar>,
    /// Water and gas points of entry, when given (else found from the cold water and gas runs).
    pub entries: Vec<PointOfEntry>,
    /// What the drawings call for (notes, one-line, details): each must be in the model.
    pub required: Vec<GroundRequirement>,
    /// The concrete-encased electrode's requirement as the drawings' note states it.
    pub ufer_note: UferNote,
}

/// A grounding electrode and its grounding electrode conductor (GEC) from the service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Electrode {
    pub tag: String,
    /// `water_pipe`, `concrete_encased`, `ground_rod`, `ground_ring`, `building_steel`.
    pub kind: String,
    pub level: String,
    /// Where the GEC connects (plan, drawing units) and its elevation (feet, absolute; below
    /// grade is negative). A water pipe electrode names its point of entry instead (`entry`).
    pub at: [f64; 2],
    pub z_ft: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub entry: String,
    /// The GEC: size as drawn, run in conduit, and its route (plan x, y in drawing units, z in
    /// feet; empty until `grounding.route`).
    pub gec_size: String,
    pub in_conduit: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<[f64; 3]>,
    /// Concrete-encased (Ufer): the conductor's length in the footing, its size and kind
    /// (`bare_cu` or `rebar`, rebar by diameter in inches), the concrete cover, whether that part
    /// of the footing is in direct contact with earth, the direction it runs (plan, degrees) and
    /// whether its stub-up connection is accessible.
    pub length_ft: f64,
    pub conductor: String,
    pub material: String,
    pub rebar_in: f64,
    pub cover_in: f64,
    pub earth_contact: bool,
    pub angle_deg: f64,
    pub connection_accessible: bool,
    /// The footing's pour: `pending` or `poured`, and whether the electrode is documented as
    /// installed (photos, an inspection) — the pour hold point.
    pub pour: String,
    pub documented: bool,
    /// Rods: how many and how long; a ring: its length.
    pub count: u32,
    pub rod_length_ft: f64,
    pub ring_length_ft: f64,
    /// Set on an electrode modelled in place of a missed one (its tag).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub alternative_of: String,
}

impl Default for Electrode {
    fn default() -> Self {
        Electrode {
            tag: String::new(),
            kind: String::new(),
            level: String::new(),
            at: [0.0, 0.0],
            z_ft: 0.0,
            entry: String::new(),
            gec_size: String::new(),
            in_conduit: false,
            path: Vec::new(),
            length_ft: 0.0,
            conductor: "#4".into(),
            material: "bare_cu".into(),
            rebar_in: 0.0,
            cover_in: 0.0,
            earth_contact: true,
            angle_deg: 0.0,
            connection_accessible: true,
            pour: "pending".into(),
            documented: false,
            count: 1,
            rod_length_ft: 10.0,
            ring_length_ft: 0.0,
            alternative_of: String::new(),
        }
    }
}

/// A bonding conductor or jumper.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bond {
    pub tag: String,
    /// `water`, `gas`, `metal_piping`, `structural_steel`, `main_bonding_jumper`,
    /// `system_bonding_jumper`, `separately_derived` (a transformer), `telecom`, `rack`, `tray`.
    pub kind: String,
    /// From (gear or busbar tag; default the service) to (a point of entry, busbar, gear or
    /// equipment tag; or a plan point `at` on `level`).
    pub from: String,
    pub to: String,
    pub level: String,
    pub at: [f64; 2],
    pub z_ft: f64,
    pub size: String,
    pub in_conduit: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<[f64; 3]>,
}

/// A grounding busbar: the telecom main grounding busbar (TMGB) or a room's (TGB).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Busbar {
    pub tag: String,
    /// `tmgb` or `tgb`.
    pub kind: String,
    /// The room it is in (name or number, as the room list has it).
    pub room: String,
    pub level: String,
    pub at: [f64; 2],
    pub z_ft: f64,
    /// As drawn: `1/4" x 4" x 12"`.
    pub size: String,
}

/// Where water or gas piping enters the building: where its bond or electrode clamps on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PointOfEntry {
    pub tag: String,
    /// `water` or `gas`.
    pub kind: String,
    pub level: String,
    pub at: [f64; 2],
    pub z_ft: f64,
    /// The clamp can be reached (not above a hard ceiling or in a wall), and why not.
    pub accessible: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
}

/// Something the drawings call for: a bond or electrode of `kind` (to `to`, when given).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroundRequirement {
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
    /// The note as printed, and the sheet.
    pub note: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sheet: String,
}

/// The concrete-encased electrode's requirement (typical values; check the drawings and the
/// code edition): its length, the smallest bare copper or rebar, the concrete cover.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UferNote {
    pub length_ft: f64,
    pub min_conductor: String,
    pub min_rebar_in: f64,
    pub cover_in: f64,
}

impl Default for UferNote {
    fn default() -> Self {
        UferNote { length_ft: 20.0, min_conductor: "#4".into(), min_rebar_in: 0.5, cover_in: 2.0 }
    }
}

/// An electrical device and the circuit it is on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ElecDevice {
    /// Its id or tag (`R-101-1`).
    pub tag: String,
    /// `receptacle`, `switch`, `light_fixture`, `data_outlet`, `disconnect`, `junction_box`…
    pub kind: String,
    pub level: String,
    /// The room it is in (number or name, as the room schedule has it).
    pub room: String,
    /// The panel and circuit number(s): `P1`, `12` (or `1,3,5` for three poles).
    pub panel: String,
    pub circuit: String,
}

/// One piece of gear's arc flash study result.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArcFlash {
    /// The gear's tag.
    pub equipment: String,
    /// Incident energy at the working distance, cal/cm².
    pub incident_energy: f64,
    pub working_distance_in: f64,
    pub arc_flash_boundary_in: f64,
    /// PPE category or the PPE required, as the study states it.
    pub ppe: String,
    /// Nominal voltage and the shock approach boundaries, as stated.
    pub voltage: String,
    pub limited_approach: String,
    pub restricted_approach: String,
    /// The study it comes from and its date.
    pub study: String,
    pub date: String,
}

impl Mep {
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
            && self.runs.is_empty()
            && self.terminals.is_empty()
            && self.equipment.is_empty()
            && self.revisions.is_empty()
            && self.feeders.is_empty()
            && self.devices.is_empty()
            && self.arc_flash.is_empty()
            && self.grounding.is_none()
    }
}

/// Where conductors land on a piece of electrical gear and what its lugs take (the gear schedule,
/// the one-line, the approved submittal).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Termination {
    /// `main`, `feed_through`, `breaker` (feeders out), `primary`, `secondary` (a transformer),
    /// `line`, `load` (a disconnect), `neutral_bar`, `ground_bar`.
    pub point: String,
    /// Lugs per phase; on a bar, its positions.
    pub lugs: u32,
    /// Conductors one lug or position takes.
    pub wires_per_lug: u32,
    /// The wire range as printed: `#6 - 350 kcmil`, `1/0-750`.
    pub range: String,
    /// `cu`, `al` or `cu/al`.
    pub material: String,
    /// `mechanical` or `compression`.
    pub lug_type: String,
    /// The lugs come with the gear; `false`: ordered separately (compression lugs on a
    /// transformer's pads), with their bolts.
    pub supplied: bool,
    /// Bolts for separately supplied lugs: diameter (inches), holes per lug, and the thickness
    /// the bolt passes through (pad and lug tongues, inches).
    pub bolt_in: f64,
    pub holes: u32,
    pub stack_in: f64,
    /// Wire-bending space at the terminal, inches (0: not given), and whether the conductors
    /// enter or leave the enclosure through the wall opposite it.
    pub bending_in: f64,
    pub opposite: bool,
}

impl Default for Termination {
    fn default() -> Self {
        Termination {
            point: String::new(),
            lugs: 1,
            wires_per_lug: 1,
            range: String::new(),
            material: "cu/al".into(),
            lug_type: "mechanical".into(),
            supplied: true,
            bolt_in: 0.5,
            holes: 2,
            stack_in: 0.75,
            bending_in: 0.0,
            opposite: true,
        }
    }
}

/// An electrical enclosure's room for what has to go into it, inches.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Enclosure {
    /// Usable entry plate areas, width × depth (0 × 0: no entry that way).
    pub top_entry_in: [f64; 2],
    pub bottom_entry_in: [f64; 2],
    /// The wiring gutter's cross-section, width × depth.
    pub gutter_in: [f64; 2],
}

/// An electrical feeder: conductors and raceways from one piece of gear to another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Feeder {
    pub tag: String,
    /// Gear tags; the termination points it lands on at each end (default: `breaker` or `load`
    /// at the source, `main`, `primary` or `line` at the load).
    pub from: String,
    pub to: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from_point: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to_point: String,
    /// Parallel sets.
    pub sets: u32,
    pub phases: u32,
    /// Phase conductor size as printed (`500 kcmil`, `#2`, `3/0`) and `cu` or `al`.
    pub size: String,
    pub material: String,
    /// Neutrals per set and their size (empty: the phase size).
    pub neutrals: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub neutral_size: String,
    /// Equipment grounding conductors per set and their size.
    pub grounds: u32,
    pub ground_size: String,
    /// More conductors landing on the load end's ground bar: grounding electrode conductors,
    /// bonding jumpers (sizes).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_grounds: Vec<String>,
    /// Raceways: count and trade size (inches); `top` or `bottom` entry at each end.
    pub conduits: u32,
    pub conduit_in: f64,
    pub entry: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from_entry: String,
}

impl Default for Feeder {
    fn default() -> Self {
        Feeder {
            tag: String::new(),
            from: String::new(),
            to: String::new(),
            from_point: String::new(),
            to_point: String::new(),
            sets: 1,
            phases: 3,
            size: String::new(),
            material: "cu".into(),
            neutrals: 1,
            neutral_size: String::new(),
            grounds: 1,
            ground_size: String::new(),
            extra_grounds: Vec::new(),
            conduits: 1,
            conduit_in: 0.0,
            entry: "top".into(),
            from_entry: String::new(),
        }
    }
}

/// A piece of scheduled equipment: a box (or upright cylinder) on a level, with the schedule's
/// row as its properties and where each of its numbers came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MepEquipment {
    /// The schedule's tag: `AHU-1`, `VAV-2-03`.
    pub tag: String,
    /// `ahu`, `rtu`, `vav`, `exhaust_fan`, `fan`, `pump`, `water_heater`, `condensing_unit`,
    /// `gas_meter` (anything else is written to IFC as a proxy).
    pub kind: String,
    pub level: String,
    /// Plan corners (sheet units).
    pub min: [f64; 2],
    pub max: [f64; 2],
    /// Bottom above the level's floor and height, feet.
    pub bottom_ft: f64,
    pub height_ft: f64,
    /// An upright cylinder (a tank) in the plan box.
    pub round: bool,
    /// The systems joined to it (names or codes).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub systems: Vec<String>,
    /// The schedule it came from and its row (column → value as printed).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
    /// Where its footprint, height and elevation came from (`outline`, `schedule`, `rule`…).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, String>,
    pub confidence: f64,
    /// Electrical gear: where conductors land and what its lugs take.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub terminations: Vec<Termination>,
    /// Electrical gear: its entry plates and gutter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enclosure: Option<Enclosure>,
    /// Review findings accepted as they are (`entry`, `gutter`, `bending`…): the gear goes in as
    /// submitted and the takeoff carries the harder installation.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub accepted: Vec<String>,
    /// A panel's schedule: its circuits, what each feeds and where (its directory).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub circuits: Vec<PanelCircuit>,
}

/// One circuit in a panel schedule.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelCircuit {
    /// Its number (`1`; the first pole of a multi-pole breaker).
    pub number: String,
    pub description: String,
    /// The room or area it serves (number or name).
    pub room: String,
    /// Breaker amps and poles, as the schedule prints them.
    pub breaker: String,
    pub poles: u32,
}

/// An air terminal (diffuser, register, grille) or VAV/mixing box on a duct run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MepTerminal {
    /// The system it belongs to.
    pub system: String,
    pub level: String,
    /// Plan position (sheet units).
    pub at: [f64; 2],
    /// Size, inches: `[width, height]` for rectangular; `[diameter, 0]` for round.
    pub size_in: [f64; 2],
    /// Bottom elevation above the floor, feet.
    pub bottom_ft: f64,
    /// What it is: `diffuser`, `register`, `grille`, `vav_box`, `mixing_box`; sprinkler heads
    /// `sprinkler_pendent`, `sprinkler_upright`, `sprinkler_sidewall` (with `bottom_ft` the
    /// deflector's height; IFC: IfcFireSuppressionTerminal).
    #[serde(default = "default_terminal_kind")]
    pub terminal_kind: String,
    /// The plan objects it was read from (hex handles).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<String>,
}

/// One change a revision of the prints made to a run: the revision log.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MepRevision {
    /// The revision (`ASI-01`).
    pub revision: String,
    /// The run's [`MepRun::id`].
    pub item: String,
    pub system: String,
    pub level: String,
    /// `resized`, `raised` (a height changed), `rerouted`, `added` or `removed`.
    pub change: String,
    /// As it was and as the revision has it (sizes in inches, heights in feet, as text).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
}

fn default_terminal_kind() -> String {
    "diffuser".into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MepKind {
    Duct,
    Pipe,
    Conduit,
    /// Cable tray (width × depth), a coordination trade like the others.
    Tray,
    /// Cable outside conduit (a homerun's MC cable), round like conduit.
    Cable,
}

/// What a duct system carries — determines the default colour when none is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuctType {
    Supply,
    Return,
    Exhaust,
    OutsideAir,
    Other,
}

impl DuctType {
    /// The default ACI colour index for this duct type (supply=blue 5, return=magenta 6,
    /// exhaust=green 3, outside air=cyan 4, other=white 7).
    pub fn default_color(self) -> i16 {
        match self {
            DuctType::Supply => 5,
            DuctType::Return => 6,
            DuctType::Exhaust => 3,
            DuctType::OutsideAir => 4,
            DuctType::Other => 7,
        }
    }
}

impl MepSystem {
    /// The ACI colour index to use for this system's 3D layer: the explicit `color_index` when
    /// set, else the duct type's default for ducts, else the MEP kind's default.
    pub fn aci_color(&self) -> i16 {
        if self.color_index > 0 {
            return self.color_index;
        }
        if let Some(dt) = self.duct_type {
            return dt.default_color();
        }
        match self.kind {
            MepKind::Duct => 4,    // cyan (generic duct, no type set)
            MepKind::Pipe => 3,    // green
            MepKind::Conduit => 6, // magenta
            MepKind::Tray => 30,   // orange
            MepKind::Cable => 210, // pink: an assumed homerun route
        }
    }
}

/// A system: the layers its runs are drawn on, what they are, their usual size and height, and
/// whether the model shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MepSystem {
    pub name: String,
    pub layer: String,
    pub kind: MepKind,
    /// Width and height, inches (height 0: round, width = diameter).
    pub size_in: [f64; 2],
    /// Bottom above the floor, feet.
    pub bottom_ft: f64,
    #[serde(default = "yes")]
    pub show: bool,
    /// For duct systems: supply, return, exhaust, outside_air or other (determines the default
    /// colour). Pipes and conduits leave this at the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duct_type: Option<DuctType>,
    /// ACI colour index for the system's 3D layer; 0 means "use the default for the duct type
    /// or MEP kind".
    #[serde(default, skip_serializing_if = "is_zero_i16")]
    pub color_index: i16,
}

fn yes() -> bool {
    true
}

fn is_zero_i16(v: &i16) -> bool {
    *v == 0
}

/// A run: its plan path (sheet units), its size, and the bottom height of each segment (a change
/// between two is a drop at their corner).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MepRun {
    pub system: String,
    pub level: String,
    pub points: Vec<[f64; 2]>,
    pub size_in: [f64; 2],
    pub bottoms_ft: Vec<f64>,
    /// The plan objects it was read from (hex handles).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<String>,
    /// A name that stays with the run from one revision of the prints to the next (`R12`), given
    /// when a revision first compares it; empty before.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
}

/// What the reflected ceiling plans say, on the floor plans' coordinates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rcp {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ceilings: Vec<CeilingArea>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub soffits: Vec<Soffit>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<CeilingItem>,
}

impl Rcp {
    pub fn is_empty(&self) -> bool {
        self.ceilings.is_empty() && self.soffits.is_empty() && self.items.is_empty()
    }
}

/// A ceiling area: its outline, height above the floor and what it is (a suspended grid with its
/// layout, or gypsum board).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CeilingArea {
    pub level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    pub outline: Vec<[f64; 2]>,
    pub height_ft: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<CeilingGrid>,
    /// Its build-up ([`CeilingType::name`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling_type: Option<String>,
    /// Why the type is a guess, when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guess: Option<String>,
}

/// A suspended ceiling grid: tile size (inches, along and across `angle_deg`) and a line
/// crossing at `origin`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CeilingGrid {
    pub spacing_in: [f64; 2],
    pub origin: [f64; 2],
    #[serde(default)]
    pub angle_deg: f64,
}

/// A gypsum soffit or bulkhead: an area of ceiling dropped to `height_ft`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Soffit {
    pub level: String,
    pub outline: Vec<[f64; 2]>,
    pub height_ft: f64,
    /// Its build-up ([`SoffitType::name`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soffit_type: Option<String>,
    /// The ceiling round it (where its sides meet it), feet above the floor; none = the highest
    /// ceiling on its level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_ft: Option<f64>,
    /// The length of its sides that show (not against a wall), feet; none = its whole outline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sides_ft: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guess: Option<String>,
}

/// A light fixture or air diffuser set in the ceiling: its block, centre, size (inches) and
/// turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CeilingItem {
    pub kind: ItemKind,
    pub block: String,
    pub level: String,
    pub at: [f64; 2],
    pub size_in: [f64; 2],
    #[serde(default)]
    pub rotation_deg: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Fixture,
    Diffuser,
}

/// A fall per foot in words: `1/4"/ft`, `4:12`.
pub fn pitch_text(rise_per_ft: f64) -> String {
    if rise_per_ft >= 1.0 / 12.0 - 1e-9 {
        let r = (rise_per_ft * 12.0 * 8.0).round() / 8.0;
        format!("{}:12", if r.fract() == 0.0 { format!("{r}") } else { format!("{r:.3}").trim_end_matches('0').to_string() })
    } else {
        format!("{}/ft", inches(rise_per_ft * 12.0))
    }
}

fn forty_four() -> f64 {
    44.0
}

/// Material names by surface; none = the renderer's default for that part.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Surfaces {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ceiling: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub door: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground: Option<String>,
}

impl Surfaces {
    /// The material of a surface by its part name (`wall`, `ceiling`…).
    pub fn get(&self, part: &str) -> Option<&str> {
        match part {
            "wall" => self.wall.as_deref(),
            "floor" => self.floor.as_deref(),
            "ceiling" => self.ceiling.as_deref(),
            "door" => self.door.as_deref(),
            "window" | "curtainwall" => self.window.as_deref(),
            "column" => self.column.as_deref(),
            "ground" => self.ground.as_deref(),
            _ => None,
        }
    }
}

/// The ground around the building.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Site {
    /// Ground elevation, feet (usually a little under the first floor).
    #[serde(default)]
    pub elevation_ft: f64,
    /// How far the ground reaches past the building, feet.
    #[serde(default = "fifty")]
    pub margin_ft: f64,
}

fn one() -> u32 {
    SPEC_VERSION
}
fn twelve() -> f64 {
    12.0
}
fn six() -> f64 {
    6.0
}
fn fifty() -> f64 {
    50.0
}

/// A problem found in a spec: an error stops a build, a warning doesn't. `path` names the item
/// (`levels[1].height_ft`); `handles` the drawing's objects it is about.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub error: bool,
    pub path: String,
    pub message: String,
    pub handles: Vec<cadaddy_doc::Handle>,
}

impl Issue {
    pub fn error(path: impl Into<String>, message: impl Into<String>) -> Issue {
        Issue { error: true, path: path.into(), message: message.into(), handles: Vec::new() }
    }
    pub fn warning(path: impl Into<String>, message: impl Into<String>) -> Issue {
        Issue { error: false, path: path.into(), message: message.into(), handles: Vec::new() }
    }
}

impl Default for BuildingSpec {
    fn default() -> Self {
        BuildingSpec::from_rules(&Rules::fallback())
    }
}

/// Feet as feet and inches to the nearest eighth: `10'-0"`, `-0'-6"`.
pub fn feet(ft: f64) -> String {
    if !ft.is_finite() {
        return "?".into();
    }
    let eighths = (ft.abs() * 96.0).round();
    let sign = if ft < 0.0 && eighths > 0.0 { "-" } else { "" };
    let whole = (eighths / 96.0).floor();
    format!("{sign}{whole}'-{}", inches_text(eighths - whole * 96.0))
}

/// Inches to the nearest eighth: `4 7/8"`, `6"`.
pub fn inches(inch: f64) -> String {
    if !inch.is_finite() {
        return "?".into();
    }
    inches_text((inch.abs() * 8.0).round())
}

fn inches_text(eighths: f64) -> String {
    let whole = (eighths / 8.0).floor();
    let mut num = eighths - whole * 8.0;
    let mut den = 8.0;
    while num > 0.0 && num % 2.0 == 0.0 {
        num /= 2.0;
        den /= 2.0;
    }
    match (whole > 0.0, num > 0.0) {
        (true, true) => format!("{whole} {num}/{den}\""),
        (false, true) => format!("{num}/{den}\""),
        _ => format!("{whole}\""),
    }
}

impl BuildingSpec {
    /// The spec in plain language, part by part ("L2 = sheet A112 at 10'-0"", "A-WALL = full
    /// height walls 4 7/8" thick (line pairs)").
    pub fn describe(&self) -> Vec<Described> {
        let mut out = Vec::new();
        let mut put = |section: &'static str, key: String, text: String| {
            let approved = self.approved.iter().any(|a| a.eq_ignore_ascii_case(&key));
            let guess = self.guess.iter().find(|(k, _)| k.eq_ignore_ascii_case(&key)).map(|(_, why)| why.clone());
            out.push(Described { section, key, text, approved, guess });
        };
        for l in &self.levels {
            let place = match &l.sheet {
                Some(sh) => format!("sheet {sh}"),
                None if l.region.is_some() => "a region of this sheet".to_string(),
                None => "this sheet".to_string(),
            };
            put(
                "levels",
                format!("levels/{}", l.name),
                format!("{} = {place} at {}; {} floor to floor, {} ceilings", l.name, feet(l.elevation_ft), feet(l.height_ft), feet(l.ceiling_ft)),
            );
        }
        for r in &self.layers {
            let ty = self.wall_type_of(r);
            let what = match r.role {
                Role::Wall | Role::CurtainWall => {
                    let kind = if r.role == Role::Wall { "walls" } else { "curtain walls" };
                    let height = match ty.and_then(|t| t.height_ft) {
                        Some(h) => format!("{kind} {} high", feet(h)),
                        None => format!("full height {kind}"),
                    };
                    match ty {
                        Some(t) if t.from == WallFrom::Centerline => format!("{height}, {} thick (centrelines)", inches(t.thickness_in)),
                        Some(t) => format!("{height} {} thick (line pairs)", inches(t.thickness_in)),
                        None => format!("{height} (line pairs)"),
                    }
                }
                Role::Door => "doors".into(),
                Role::Window => "windows".into(),
                Role::Column => "columns, floor to wall top".into(),
                Role::Stair => "stairs".into(),
                Role::CeilingGrid => "ceiling grids (not built yet)".into(),
                Role::Fixture => "fixtures".into(),
                Role::RoomTag => "room tags".into(),
                Role::Device => "devices".into(),
                Role::Run => format!("runs {} above the floor", feet(r.height_ft.unwrap_or(9.0))),
                Role::Ignore => "not part of the building".into(),
            };
            put("layers", format!("layers/{}", r.layer), format!("{} = {what}", r.layer));
        }
        for t in &self.wall_types {
            let height = t.height_ft.map_or("floor to floor".to_string(), |h| format!("{} high", feet(h)));
            let from = if t.from == WallFrom::Centerline { "drawn as centrelines" } else { "drawn as line pairs" };
            let mut text = format!("{}: {} thick, {height}, {from}", t.name, inches(t.thickness_in));
            if let Some(a) = &t.assembly {
                if let Some(h) = a.height {
                    let _ = write!(text, ", {}", h.text(a.above_ceiling_in));
                }
                if !a.layers.is_empty() {
                    let parts: Vec<&str> = a.layers.iter().map(|l| l.material.as_str()).collect();
                    let _ = write!(text, "; {}", parts.join(" + "));
                }
                for r in [&a.fire_rating, &a.acoustic_rating].into_iter().flatten() {
                    let _ = write!(text, "; {r}");
                }
                if a.external {
                    text.push_str("; exterior");
                }
                if let Some(h) = &a.head {
                    let _ = write!(text, "; {}", h.text());
                }
            }
            put("wall_types", format!("wall_types/{}", t.name), text);
        }
        if !self.walls.is_empty() {
            let guessed = self.walls.iter().filter(|w| w.guess.is_some()).count();
            put("walls", "walls".into(), format!("{} walls typed from their tags on the plans, {guessed} guessed (no tag)", self.walls.len()));
        }
        for r in &self.rooms {
            let name = if r.name.trim().is_empty() { r.number.clone().unwrap_or_default() } else { r.name.clone() };
            let ceiling = r.ceiling_ft.map_or("the level's ceiling".to_string(), |c| format!("ceiling {}", feet(c)));
            let mats = materials_text(&r.materials);
            let on = r.level.as_ref().map_or(String::new(), |l| format!(" on {l}"));
            put(
                "rooms",
                format!("rooms/{name}"),
                format!("{name}{on}: {ceiling}{}", if mats.is_empty() { String::new() } else { format!("; {mats}") }),
            );
        }
        for f in &self.families {
            let kind = if f.kind == FamilyKind::Door { "doors" } else { "windows" };
            let width = match (f.width_in, &f.width_attr) {
                (Some(w), _) => format!("{} wide", inches(w)),
                (None, Some(a)) => format!("width from {a}"),
                (None, None) => "width from the block".into(),
            };
            let head = f.head_ft.unwrap_or(if f.kind == FamilyKind::Door { self.openings.door_head_ft } else { self.openings.window_head_ft });
            let heights = match f.kind {
                FamilyKind::Door => format!("{} high", feet(head)),
                FamilyKind::Window => format!("{} to {}", feet(f.sill_ft.unwrap_or(self.openings.window_sill_ft)), feet(head)),
            };
            put("families", format!("families/{}", f.name), format!("{}: {kind} from blocks {}, {width}, {heights}", f.name, f.block));
        }
        for st in &self.stairs {
            let to = match (&st.to_level, self.stair_heights(st)) {
                (Some(n), _) => n.clone(),
                (None, Some((a, b))) => {
                    self.levels.iter().find(|l| (l.elevation_ft - b).abs() < 1e-6).map_or_else(|| format!("{} up", feet(b - a)), |l| l.name.clone())
                }
                (None, None) => "?".into(),
            };
            let what = match self.stair_measures(st) {
                Some((_, _, _, len)) if st.ramp => {
                    let rise = self.stair_heights(st).map_or(0.0, |(a, b)| b - a);
                    format!("ramp from {} to {to}, rising {} over {}", st.level, feet(rise), feet(len))
                }
                Some((n, r, t, _)) => {
                    let runs = if st.runs.len() == 1 { "1 run".to_string() } else { format!("{} runs", st.runs.len()) };
                    let tread = if t.is_finite() { format!(", {} treads", inches(t)) } else { String::new() };
                    format!("stair from {} to {to}, {runs}, {n} risers of {}{tread}", st.level, inches(r))
                }
                None => format!("stair from {}", st.level),
            };
            let state = if st.confirmed { "" } else { " (detected; confirm)" };
            put("stairs", format!("stairs/{}", st.name), format!("{}: {what}, {} wide{state}", st.name, inches(st.width_in)));
        }
        for f in &self.exterior.facades {
            let bands: Vec<String> = f
                .bands
                .iter()
                .map(|b| {
                    let what = match (b.kind, &b.material) {
                        (BandKind::Glass, _) => "glass".to_string(),
                        (BandKind::Material, Some(m)) => m.clone(),
                        (BandKind::Material, None) => "material".to_string(),
                    };
                    let asm = b.assembly.as_ref().map_or(String::new(), |a| {
                        format!(" ({}{} layers)", if a.tag.is_empty() { String::new() } else { format!("{}, ", a.tag) }, a.layers.len())
                    });
                    format!("{what} {} to {}{asm}", feet(b.from_ft), feet(b.to_ft))
                })
                .collect();
            put(
                "exterior",
                format!("facades/{}", f.side.name()),
                format!("{} facade: {}", f.side.name(), if bands.is_empty() { "plain".into() } else { bands.join("; ") }),
            );
        }
        for r in &self.exterior.roofs {
            let slope = r.slope.map_or("flat".to_string(), |s| format!("sloping {}", pitch_text(s.rise_per_ft)));
            let parapet = r.parapet_ft.map_or(String::new(), |p| format!(", {} parapet", feet(p)));
            let asm = r.assembly.as_ref().map_or(String::new(), |a| {
                let words: Vec<&str> = a.layers.iter().map(|l| l.material.as_str()).collect();
                format!("; {} top down: {}", if a.tag.is_empty() { "built" } else { a.tag.as_str() }, words.join(", "))
            });
            put("exterior", format!("roofs/{}", r.name), format!("{}: roof on {}, {slope}{parapet}{asm}", r.name, r.level));
        }
        for (i, c) in self.exterior.crickets.iter().enumerate() {
            put("exterior", format!("crickets/{}", i + 1), format!("Cricket {} on {}, {} high", i + 1, c.level, feet(c.height_ft)));
        }
        for (i, c) in self.rcp.ceilings.iter().enumerate() {
            let what = match c.grid {
                Some(g) => format!("{}×{} grid", inches(g.spacing_in[0]).trim_end_matches('"'), inches(g.spacing_in[1])),
                None => "gypsum".into(),
            };
            let room = c.room.as_ref().map_or(String::new(), |r| format!(" ({r})"));
            let what = c.ceiling_type.as_ref().map_or(what.clone(), |t| format!("type {t}, {what}"));
            let guess = c.guess.as_ref().map_or(String::new(), |g| format!(" (guess: {g})"));
            put("rcp", format!("ceilings/{}", i + 1), format!("Ceiling {}{room} on {}: {what} at {}{guess}", i + 1, c.level, feet(c.height_ft)));
        }
        for (i, sf) in self.rcp.soffits.iter().enumerate() {
            let what = sf.soffit_type.as_ref().map_or(String::new(), |t| format!(", type {t}"));
            let guess = sf.guess.as_ref().map_or(String::new(), |g| format!(" (guess: {g})"));
            put("rcp", format!("soffits/{}", i + 1), format!("Soffit {} on {} at {}{what}{guess}", i + 1, sf.level, feet(sf.height_ft)));
        }
        let words = |ls: &[AssemblyLayer]| ls.iter().map(|l| l.material.as_str()).collect::<Vec<_>>().join(" + ");
        for c in &self.ceiling_types {
            let kind = match c.kind {
                CeilingKind::Gypsum => "gypsum board",
                CeilingKind::Grid => "tile in a grid",
                CeilingKind::Open => "open to the structure",
            };
            let mut text = format!("{}: {kind}", c.name);
            if !c.layers.is_empty() {
                let _ = write!(text, "; {}", words(&c.layers));
            }
            if let Some(r) = &c.fire_rating {
                let _ = write!(text, "; {r}");
            }
            put("ceiling_types", format!("ceiling_types/{}", c.name), text);
        }
        for t in &self.soffit_types {
            let hang = match t.hangs_from {
                Hang::Ceiling => "built under the ceiling".to_string(),
                Hang::Structure => format!("hung from the structure, sides {} past the ceiling", inches(t.above_ceiling_in)),
            };
            put(
                "soffit_types",
                format!("soffit_types/{}", t.name),
                format!("{}: {hang}; bottom {}; sides {}; framing {}", t.name, words(&t.bottom), words(&t.sides), words(&t.framing)),
            );
        }
        if !self.rcp.items.is_empty() {
            let n = |k: ItemKind| self.rcp.items.iter().filter(|x| x.kind == k).count();
            put(
                "rcp",
                "ceiling_items".into(),
                format!("In the ceilings: {} light fixtures, {} diffusers", n(ItemKind::Fixture), n(ItemKind::Diffuser)),
            );
        }
        for (kind, label) in [(FamilyKind::Door, "Door"), (FamilyKind::Window, "Window")] {
            let rows: Vec<&ScheduleRow> = self.schedule.rows.iter().filter(|r| r.kind == kind).collect();
            if rows.is_empty() {
                continue;
            }
            let list: Vec<String> = rows
                .iter()
                .take(12)
                .map(|r| match (r.width_in, r.height_in) {
                    (Some(w), Some(h)) => format!("{} {}×{}", r.mark, feet(w / 12.0), feet(h / 12.0)),
                    _ => r.mark.clone(),
                })
                .collect();
            let more = if rows.len() > 12 { format!(" and {} more", rows.len() - 12) } else { String::new() };
            put("schedule", format!("schedule/{}s", label.to_ascii_lowercase()), format!("{label} schedule: {}{more}", list.join(", ")));
        }
        if !self.columns.is_empty() {
            let mut by: Vec<(&str, usize)> = Vec::new();
            for c in &self.columns {
                match by.iter_mut().find(|(l, _)| *l == c.level) {
                    Some(x) => x.1 += 1,
                    None => by.push((&c.level, 1)),
                }
            }
            for (l, n) in by {
                put("columns", format!("columns/{l}"), format!("{n} columns on {l} at the grid intersections"));
            }
        }
        for sy in &self.mep.systems {
            let n = self.mep.runs.iter().filter(|r| r.system == sy.name).count();
            let size = if sy.size_in[1] > 0.0 {
                format!("{}x{}", inches(sy.size_in[0]).trim_end_matches('"'), inches(sy.size_in[1]))
            } else {
                format!("{} round", inches(sy.size_in[0]))
            };
            let kind = match sy.kind {
                MepKind::Duct => "duct",
                MepKind::Pipe => "pipe",
                MepKind::Conduit => "conduit",
                MepKind::Tray => "tray",
                MepKind::Cable => "cable",
            };
            let hidden = if sy.show { "" } else { ", hidden" };
            put(
                "mep",
                format!("systems/{}", sy.name),
                format!("{}: {kind} on {}, {n} runs, usually {size} with its bottom at {}{hidden}", sy.name, sy.layer, feet(sy.bottom_ft)),
            );
        }
        for e in &self.mep.equipment {
            let guessed = e.sources.values().any(|v| v == "rule");
            put(
                "mep",
                format!("equipment/{}", e.tag),
                format!(
                    "{}: {} on {}, {} high{}",
                    e.tag,
                    e.kind.replace('_', " "),
                    e.level,
                    feet(e.height_ft),
                    if guessed { ", partly guessed" } else { "" }
                ),
            );
        }
        for e in &self.exterior.equipment {
            put("exterior", format!("equipment/{}", e.name), format!("{}: rooftop equipment on {}, {} high", e.name, e.level, feet(e.height_ft)));
        }
        let mats = materials_text(&self.materials);
        if !mats.is_empty() {
            put("materials", "materials".into(), format!("Materials: {mats}"));
        }
        if let Some(site) = &self.site {
            put("site", "site".into(), format!("Ground at {}, {} round the building", feet(site.elevation_ft), feet(site.margin_ft)));
        }
        let o = &self.openings;
        put(
            "openings",
            "openings".into(),
            format!("Openings in walls: doors {} high, windows {} to {}", feet(o.door_head_ft), feet(o.window_sill_ft), feet(o.window_head_ft)),
        );
        out
    }

    /// The spec saying what Walk mode's rules say.
    pub fn from_rules(r: &Rules) -> BuildingSpec {
        let layers = r
            .layers
            .iter()
            .map(|l| match &l.kind {
                LayerKind::Wall { .. } => LayerRole { layer: l.layer.clone(), role: Role::Wall, wall_type: None, height_ft: None },
                LayerKind::Device => LayerRole { layer: l.layer.clone(), role: Role::Device, wall_type: None, height_ft: None },
                LayerKind::Run { height_ft } => LayerRole { layer: l.layer.clone(), role: Role::Run, wall_type: None, height_ft: Some(*height_ft) },
            })
            .collect();
        // A wall height in the rules becomes the wall type of those layers.
        let mut wall_types: Vec<WallType> = Vec::new();
        let mut layers: Vec<LayerRole> = layers;
        for (role, rule) in layers.iter_mut().zip(&r.layers) {
            if let LayerKind::Wall { height_ft: Some(h), .. } = rule.kind {
                let name = format!("Walls {h}'");
                if !wall_types.iter().any(|t| t.name == name) {
                    wall_types.push(WallType { name: name.clone(), height_ft: Some(h), thickness_in: 6.0, from: WallFrom::Pairs, assembly: None });
                }
                role.wall_type = Some(name);
            }
        }
        BuildingSpec {
            version: SPEC_VERSION,
            units_per_foot: r.units_per_foot,
            levels: r.levels.clone(),
            datums: Vec::new(),
            sheets: Vec::new(),
            tags: Vec::new(),
            estimate: serde_json::Value::Null,
            progress: serde_json::Value::Null,
            layers,
            wall_types,
            rooms: Vec::new(),
            families: Vec::new(),
            materials: Surfaces::default(),
            site: None,
            stairs: Vec::new(),
            exterior: Exterior::default(),
            rcp: Rcp::default(),
            mep: Mep::default(),
            concrete: crate::concrete::Concrete::default(),
            columns: Vec::new(),
            schedule: Schedule::default(),
            walls: Vec::new(),
            rfis: Vec::new(),
            procurement: None,
            prints: None,
            ceiling_types: Vec::new(),
            soffit_types: Vec::new(),
            devices: r.devices.clone(),
            device_sheets: r.device_sheets.clone(),
            openings: r.openings.clone(),
            approved: Vec::new(),
            guess: BTreeMap::new(),
        }
    }

    /// Walk mode's rules for this spec: walls, devices (and fixtures) and runs by layer; the other
    /// roles are built by `model.build` itself.
    pub fn to_rules(&self) -> Rules {
        let layers = self
            .layers
            .iter()
            .filter_map(|l| {
                let kind = match l.role {
                    Role::Wall => {
                        LayerKind::Wall { height_ft: self.wall_type_of(l).and_then(|t| t.height_ft), skip_curves: false, paired_only: false }
                    }
                    Role::Device | Role::Fixture => LayerKind::Device,
                    Role::Run => LayerKind::Run { height_ft: l.height_ft.unwrap_or(9.0) },
                    _ => return None,
                };
                Some(LayerRule { layer: l.layer.clone(), kind })
            })
            .collect();
        Rules {
            units_per_foot: self.units_per_foot,
            levels: self.levels.clone(),
            layers,
            devices: self.devices.clone(),
            device_sheets: self.device_sheets.clone(),
            openings: self.openings.clone(),
            stairs: self.stairs.iter().filter_map(|st| self.stair_rule(st)).collect(),
        }
    }

    /// The heights a stair climbs between, feet: its level's floor, and the floor it reaches.
    pub fn stair_heights(&self, st: &Stair) -> Option<(f64, f64)> {
        let bottom = self.level(&st.level)?;
        let top = match &st.to_level {
            Some(n) => self.level(n)?.elevation_ft,
            None => self
                .levels
                .iter()
                .map(|l| l.elevation_ft)
                .filter(|e| e.is_finite() && *e > bottom.elevation_ft + 1e-6)
                .min_by(f64::total_cmp)
                .unwrap_or(bottom.elevation_ft + bottom.height_ft),
        };
        Some((bottom.elevation_ft, top))
    }

    /// A stair as Walk mode and the build take it (heights worked out).
    pub fn stair_rule(&self, st: &Stair) -> Option<crate::stairs::StairRule> {
        let (z0, z1) = self.stair_heights(st)?;
        Some(crate::stairs::StairRule {
            name: st.name.clone(),
            level: st.level.clone(),
            runs: st.runs.iter().take(100).cloned().collect(),
            width: st.width_in / 12.0 * self.units_per_foot,
            z_bottom_ft: z0,
            z_top_ft: z1,
            ramp: st.ramp,
        })
    }

    /// A stair's risers (count and inches), tread depth (inches, shallowest run) and run length
    /// (feet), as built.
    pub fn stair_measures(&self, st: &Stair) -> Option<(u32, f64, f64, f64)> {
        let r = self.stair_rule(st)?;
        let upf = if self.units_per_foot.is_finite() && self.units_per_foot > 0.0 { self.units_per_foot } else { 12.0 };
        let len_ft = |run: &crate::stairs::StairRun| ((run.to[0] - run.from[0]).powi(2) + (run.to[1] - run.from[1]).powi(2)).sqrt() / upf;
        let risers = r.risers();
        let rise = r.z_top_ft - r.z_bottom_ft;
        let riser_in = if risers > 0 { rise * 12.0 / f64::from(risers) } else { 0.0 };
        let tread_in = st.runs.iter().filter(|x| x.treads > 0).map(|x| len_ft(x) * 12.0 / f64::from(x.treads)).fold(f64::INFINITY, f64::min);
        let length: f64 = st.runs.iter().map(len_ft).sum();
        Some((risers, riser_in, tread_in, length))
    }

    pub fn from_json(s: &str) -> Result<BuildingSpec, String> {
        let spec: BuildingSpec = serde_json::from_str(s).map_err(|e| format!("building spec: {e}"))?;
        if spec.version > SPEC_VERSION {
            return Err(format!("building spec: version {} is newer than this CADaddy reads ({SPEC_VERSION})", spec.version));
        }
        Ok(spec)
    }

    /// The JSON text kept in the drawing (fields in a fixed order: the same spec is the same text).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The role of a layer: the first pattern that matches.
    pub fn role(&self, layer: &str) -> Option<&LayerRole> {
        self.layers.iter().find(|r| layer_match(&r.layer, layer))
    }

    /// A wall layer's type: the one it names (none: line pairs, floor to floor).
    pub fn wall_type_of(&self, role: &LayerRole) -> Option<&WallType> {
        let n = role.wall_type.as_ref()?;
        self.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(n))
    }

    /// The family whose block pattern matches a block name.
    pub fn family(&self, block: &str) -> Option<&Family> {
        self.families.iter().find(|f| layer_match(&f.block, block))
    }

    pub fn level(&self, name: &str) -> Option<&Level> {
        self.levels.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }

    /// Problems found in the spec alone (see `bspec.validate` for those against the drawing).
    pub fn check(&self) -> Vec<Issue> {
        let mut out = Vec::new();
        let fin = |v: f64| v.is_finite();
        if self.version == 0 || self.version > SPEC_VERSION {
            out.push(Issue::error("version", format!("version must be 1 to {SPEC_VERSION}")));
        }
        if !(fin(self.units_per_foot) && self.units_per_foot > 0.0 && self.units_per_foot <= 1e6) {
            out.push(Issue::error("units_per_foot", "units per foot must be a positive number (12 for inches, 304.8 for millimetres)"));
        }
        for (what, n) in [
            ("levels", self.levels.len()),
            ("layers", self.layers.len()),
            ("wall_types", self.wall_types.len()),
            ("walls", self.walls.len()),
            ("rfis", self.rfis.len()),
            ("datums", self.datums.len()),
            ("sheets", self.sheets.len()),
            ("rooms", self.rooms.len()),
            ("families", self.families.len()),
        ] {
            if n > MAX_ITEMS {
                out.push(Issue::error(what, format!("at most {MAX_ITEMS} {what}")));
            }
        }
        if self.levels.is_empty() {
            out.push(Issue::error("levels", "a building needs at least one level"));
        }
        for (i, l) in self.levels.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("levels[{i}]");
            if l.name.trim().is_empty() {
                out.push(Issue::error(format!("{at}.name"), "a level needs a name"));
            } else if self.levels.iter().take(i).any(|o| o.name.eq_ignore_ascii_case(&l.name)) {
                out.push(Issue::error(format!("{at}.name"), format!("two levels are named {}", l.name)));
            }
            if !fin(l.elevation_ft) || l.elevation_ft.abs() > 1e5 {
                out.push(Issue::error(format!("{at}.elevation_ft"), "the elevation must be a number of feet"));
            }
            if !(fin(l.height_ft) && l.height_ft > 0.0 && l.height_ft <= 1000.0) {
                out.push(Issue::error(format!("{at}.height_ft"), "floor to floor must be more than 0 feet"));
            }
            if !(fin(l.ceiling_ft) && l.ceiling_ft > 0.0) {
                out.push(Issue::error(format!("{at}.ceiling_ft"), "the ceiling height must be more than 0 feet"));
            } else if fin(l.height_ft) && l.ceiling_ft > l.height_ft {
                out.push(Issue::error(
                    format!("{at}.ceiling_ft"),
                    format!("the ceiling ({}') is above the next floor ({}')", l.ceiling_ft, l.height_ft),
                ));
            }
            if let Some(r) = l.region
                && (r.iter().any(|v| !fin(*v)) || (r[2] - r[0]).abs() < 1e-9 || (r[3] - r[1]).abs() < 1e-9)
            {
                out.push(Issue::error(format!("{at}.region"), "the region must be a rectangle [x0, y0, x1, y1] with an area"));
            }
            if l.base.is_some_and(|b| b.iter().any(|v| !fin(*v))) {
                out.push(Issue::error(format!("{at}.base"), "the base point must be two numbers"));
            }
        }
        // Storeys that overlap: a level starting below the one under it ends.
        let mut by_elev: Vec<(usize, &Level)> = self.levels.iter().enumerate().filter(|(_, l)| fin(l.elevation_ft) && fin(l.height_ft)).collect();
        by_elev.sort_by(|a, b| a.1.elevation_ft.total_cmp(&b.1.elevation_ft));
        for w in by_elev.windows(2) {
            if let [(_, lo), (j, hi)] = w
                && hi.elevation_ft < lo.elevation_ft + lo.height_ft - 1e-6
                && lo.region == hi.region
                && lo.sheet == hi.sheet
            {
                out.push(Issue::warning(
                    format!("levels[{j}].elevation_ft"),
                    format!("{} starts below the top of {}, on the same plan", hi.name, lo.name),
                ));
            }
        }
        for (i, d) in self.datums.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("datums[{i}]");
            if d.name.trim().is_empty() {
                out.push(Issue::error(format!("{at}.name"), "a datum needs a name"));
            } else if self.datums.iter().take(i).any(|o| o.name.eq_ignore_ascii_case(&d.name)) {
                out.push(Issue::error(format!("{at}.name"), format!("two datums are named {}", d.name)));
            } else if self.levels.iter().any(|l| l.name.eq_ignore_ascii_case(&d.name)) {
                out.push(Issue::warning(format!("{at}.name"), format!("{} is both a level and a datum; the level wins", d.name)));
            }
            if !fin(d.elevation_ft) || d.elevation_ft.abs() > 1e5 {
                out.push(Issue::error(format!("{at}.elevation_ft"), "the elevation must be a number of feet"));
            }
        }
        for (i, t) in self.wall_types.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("wall_types[{i}]");
            if t.name.trim().is_empty() {
                out.push(Issue::error(format!("{at}.name"), "a wall type needs a name"));
            } else if self.wall_types.iter().take(i).any(|o| o.name.eq_ignore_ascii_case(&t.name)) {
                out.push(Issue::error(format!("{at}.name"), format!("two wall types are named {}", t.name)));
            }
            if !(fin(t.thickness_in) && t.thickness_in > 0.0 && t.thickness_in <= 120.0) {
                out.push(Issue::error(format!("{at}.thickness_in"), "the thickness must be more than 0 and at most 120 inches"));
            }
            if t.height_ft.is_some_and(|h| !(fin(h) && h > 0.0 && h <= 1000.0)) {
                out.push(Issue::error(format!("{at}.height_ft"), "the wall height must be more than 0 feet"));
            }
            if let Some(a) = &t.assembly {
                if a.layers.len() > MAX_ASSEMBLY_LAYERS {
                    out.push(Issue::error(format!("{at}.assembly.layers"), format!("at most {MAX_ASSEMBLY_LAYERS} layers")));
                }
                for (j, l) in a.layers.iter().enumerate().take(MAX_ASSEMBLY_LAYERS) {
                    if !(fin(l.thickness_in) && l.thickness_in > 0.0 && l.thickness_in <= 48.0) {
                        out.push(Issue::error(
                            format!("{at}.assembly.layers[{j}].thickness_in"),
                            "a layer is more than 0 and at most 48 inches thick",
                        ));
                    }
                }
                if a.head.as_ref().and_then(|h| h.deflection_in).is_some_and(|d| !(fin(d) && (0.0..=12.0).contains(&d))) {
                    out.push(Issue::error(format!("{at}.assembly.head.deflection_in"), "a head's deflection is 0 to 12 inches"));
                }
                if !(fin(a.above_ceiling_in) && (0.0..=120.0).contains(&a.above_ceiling_in)) {
                    out.push(Issue::error(format!("{at}.assembly.above_ceiling_in"), "from 0 to 120 inches above the ceiling"));
                }
                let sum = a.thickness_in();
                if !a.layers.is_empty() && fin(sum) && (sum - t.thickness_in).abs() > 0.125 {
                    out.push(Issue::warning(
                        format!("{at}.assembly"),
                        format!("its layers add up to {}, the wall type is {} thick", inches(sum), inches(t.thickness_in)),
                    ));
                }
            }
        }
        for (i, w) in self.walls.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("walls[{i}]");
            if self.level(&w.level).is_none() {
                out.push(Issue::error(format!("{at}.level"), format!("there is no level named {}", w.level)));
            }
            if !self.wall_types.iter().any(|t| t.name.eq_ignore_ascii_case(&w.wall_type)) {
                out.push(Issue::error(format!("{at}.wall_type"), format!("there is no wall type named {}", w.wall_type)));
            }
            if w.a.iter().chain(&w.b).any(|v| !fin(*v)) || (w.a[0] - w.b[0]).hypot(w.a[1] - w.b[1]) < 1e-9 {
                out.push(Issue::error(at.clone(), "a wall needs two different ends"));
            }
            if w.height_ft.is_some_and(|h| !(fin(h) && h > 0.0 && h <= 1000.0)) {
                out.push(Issue::error(format!("{at}.height_ft"), "the wall height must be more than 0 feet"));
            }
        }
        for (i, r) in self.layers.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("layers[{i}]");
            if r.layer.trim().is_empty() {
                out.push(Issue::error(format!("{at}.layer"), "a layer role needs a layer name or pattern"));
            }
            if let Some(n) = &r.wall_type {
                if !matches!(r.role, Role::Wall | Role::CurtainWall) {
                    out.push(Issue::warning(format!("{at}.wall_type"), "only wall and curtain wall layers have a wall type"));
                } else if !self.wall_types.iter().any(|t| t.name.eq_ignore_ascii_case(n)) {
                    out.push(Issue::error(format!("{at}.wall_type"), format!("there is no wall type named {n}")));
                }
            }
            if r.height_ft.is_some_and(|h| !(fin(h) && (0.0..=1000.0).contains(&h))) {
                out.push(Issue::error(format!("{at}.height_ft"), "the height must be from 0 to 1000 feet"));
            }
            if let Some(j) = self.layers.iter().take(i).position(|o| o.layer.eq_ignore_ascii_case(&r.layer)) {
                out.push(Issue::warning(format!("{at}.layer"), format!("{} already has a role (layers[{j}]); this one is never used", r.layer)));
            }
        }
        for (i, r) in self.rooms.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("rooms[{i}]");
            if r.name.trim().is_empty() && r.number.as_deref().is_none_or(|n| n.trim().is_empty()) {
                out.push(Issue::error(at.clone(), "a room needs a name or a number"));
            }
            let level = r.level.as_ref().and_then(|n| self.level(n));
            if let (Some(n), None) = (&r.level, level) {
                out.push(Issue::error(format!("{at}.level"), format!("there is no level named {n}")));
            }
            if let Some(c) = r.ceiling_ft {
                if !(fin(c) && c > 0.0) {
                    out.push(Issue::error(format!("{at}.ceiling_ft"), "the ceiling height must be more than 0 feet"));
                } else if let Some(l) = level.filter(|l| c > l.height_ft) {
                    out.push(Issue::error(
                        format!("{at}.ceiling_ft"),
                        format!("the ceiling ({c}') is above the next floor ({}' on {})", l.height_ft, l.name),
                    ));
                }
            }
        }
        for (i, f) in self.families.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("families[{i}]");
            if f.name.trim().is_empty() {
                out.push(Issue::error(format!("{at}.name"), "a family needs a name"));
            }
            if f.block.trim().is_empty() {
                out.push(Issue::error(format!("{at}.block"), "a family needs a block name or pattern"));
            }
            if f.width_in.is_some_and(|w| !(fin(w) && w > 0.0 && w <= 1200.0)) {
                out.push(Issue::error(format!("{at}.width_in"), "the width must be more than 0 and at most 1200 inches"));
            }
            let head = f.head_ft.unwrap_or(match f.kind {
                FamilyKind::Door => self.openings.door_head_ft,
                FamilyKind::Window => self.openings.window_head_ft,
            });
            let sill = match f.kind {
                FamilyKind::Door => 0.0,
                FamilyKind::Window => f.sill_ft.unwrap_or(self.openings.window_sill_ft),
            };
            if !(fin(head) && head > 0.0) {
                out.push(Issue::error(format!("{at}.head_ft"), "the head height must be more than 0 feet"));
            } else if !fin(sill) || sill < 0.0 || sill >= head {
                out.push(Issue::error(format!("{at}.sill_ft"), format!("the sill ({sill}') must be at least 0 and below the head ({head}')")));
            }
            if f.kind == FamilyKind::Door && f.sill_ft.is_some_and(|s| s != 0.0) {
                out.push(Issue::warning(format!("{at}.sill_ft"), "a door has no sill; it is ignored"));
            }
        }
        let outline_ok = |o: &[[f64; 2]]| o.len() >= 3 && o.len() <= 100_000 && o.iter().all(|p| fin(p[0]) && fin(p[1]));
        for (i, r) in self.exterior.roofs.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("exterior.roofs[{i}]");
            if self.level(&r.level).is_none() {
                out.push(Issue::error(format!("{at}.level"), format!("there is no level named {}", r.level)));
            }
            if !outline_ok(&r.outline) {
                out.push(Issue::error(format!("{at}.outline"), "a roof outline needs at least 3 points"));
            }
            if let Some(sl) = r.slope {
                let len = (sl.down[0] * sl.down[0] + sl.down[1] * sl.down[1]).sqrt();
                if !(fin(sl.rise_per_ft) && (0.0..=3.0).contains(&sl.rise_per_ft) && len.is_finite() && len > 0.0) {
                    out.push(Issue::error(format!("{at}.slope"), "a slope needs a direction and a rise per foot from 0 to 3"));
                }
            }
            if r.parapet_ft.is_some_and(|p| !(fin(p) && (0.0..=50.0).contains(&p))) {
                out.push(Issue::error(format!("{at}.parapet_ft"), "the parapet must be from 0 to 50 feet"));
            }
            if let Some(a) = &r.assembly {
                envelope_assembly_issues(&format!("{at}.assembly"), a, &mut out);
            }
        }
        for (i, c) in self.exterior.crickets.iter().enumerate().take(MAX_ITEMS) {
            if self.level(&c.level).is_none() || !outline_ok(&c.outline) || !(fin(c.height_ft) && c.height_ft > 0.0 && c.height_ft <= 20.0) {
                out.push(Issue::error(format!("exterior.crickets[{i}]"), "a cricket needs a level, an outline and a height from 0 to 20 feet"));
            }
        }
        for (i, e) in self.exterior.equipment.iter().enumerate().take(MAX_ITEMS) {
            if self.level(&e.level).is_none() || !outline_ok(&e.outline) || !(fin(e.height_ft) && e.height_ft > 0.0 && e.height_ft <= 100.0) {
                out.push(Issue::error(format!("exterior.equipment[{i}]"), "equipment needs a level, an outline and a height from 0 to 100 feet"));
            }
        }
        for (i, r) in self.schedule.rows.iter().enumerate().take(100_000) {
            let at = format!("schedule.rows[{i}]");
            if r.mark.trim().is_empty() {
                out.push(Issue::error(format!("{at}.mark"), "a schedule row needs a mark"));
            } else if self.schedule.rows.iter().take(i).any(|o| o.kind == r.kind && o.mark.trim().eq_ignore_ascii_case(r.mark.trim())) {
                out.push(Issue::warning(format!("{at}.mark"), format!("mark {} is in the schedule twice", r.mark)));
            }
            for (name, v) in [("width_in", r.width_in), ("height_in", r.height_in)] {
                if v.is_some_and(|v| !(fin(v) && v > 0.0 && v <= 1200.0)) {
                    out.push(Issue::error(format!("{at}.{name}"), "a size must be more than 0 and at most 1200 inches"));
                }
            }
        }
        for (i, c) in self.columns.iter().enumerate().take(100_000) {
            let at = format!("columns[{i}]");
            if self.level(&c.level).is_none() || c.to_level.as_ref().is_some_and(|l| self.level(l).is_none()) {
                out.push(Issue::error(at.clone(), "a column's levels must exist"));
            }
            if !c.at.iter().all(|v| fin(*v)) || !c.size_in.iter().all(|v| fin(*v) && *v > 0.0 && *v <= 240.0) {
                out.push(Issue::error(at, "a column needs a place and a size (0–240\")"));
            }
        }
        for (i, sy) in self.mep.systems.iter().enumerate().take(MAX_ITEMS) {
            if sy.name.trim().is_empty()
                || sy.layer.trim().is_empty()
                || !(sy.size_in.iter().all(|v| fin(*v) && *v >= 0.0 && *v <= 240.0) && sy.size_in[0] > 0.0)
                || !fin(sy.bottom_ft)
            {
                out.push(Issue::error(format!("mep.systems[{i}]"), "a system needs a name, a layer, a size (0–240\") and a height"));
            }
        }
        for (i, r) in self.mep.runs.iter().enumerate().take(100_000) {
            let at = format!("mep.runs[{i}]");
            if !self.mep.systems.iter().any(|s| s.name == r.system) {
                out.push(Issue::error(format!("{at}.system"), format!("there is no system named {}", r.system)));
            }
            if self.level(&r.level).is_none() {
                out.push(Issue::error(format!("{at}.level"), format!("there is no level named {}", r.level)));
            }
            if r.points.len() < 2
                || r.bottoms_ft.len() + 1 != r.points.len()
                || !r.points.iter().flatten().chain(r.bottoms_ft.iter()).chain(r.size_in.iter()).all(|v| fin(*v))
                || r.size_in[0].is_nan()
                || r.size_in[0] <= 0.0
            {
                out.push(Issue::error(at, "a run needs 2 points or more, a size and one bottom height per segment"));
            }
        }
        for (i, e) in self.mep.equipment.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("mep.equipment[{i}]");
            if self.level(&e.level).is_none() {
                out.push(Issue::error(format!("{at}.level"), format!("there is no level named {}", e.level)));
            }
            let corners = e.min.iter().chain(&e.max).all(|v| fin(*v)) && e.max[0] > e.min[0] && e.max[1] > e.min[1];
            if e.tag.trim().is_empty() || !corners || !(fin(e.bottom_ft) && fin(e.height_ft) && e.height_ft > 0.0 && e.height_ft <= 100.0) {
                out.push(Issue::error(at.clone(), "equipment needs a tag, a plan box and a height from 0 to 100 feet"));
            }
            for (k, t) in e.terminations.iter().enumerate().take(MAX_ITEMS) {
                if t.point.trim().is_empty() || t.lugs == 0 || t.lugs > 1000 || t.wires_per_lug == 0 || t.wires_per_lug > 10 {
                    out.push(Issue::error(
                        format!("{at}.terminations[{k}]"),
                        "a termination needs a point and 1 to 1000 lugs of 1 to 10 conductors each",
                    ));
                }
            }
        }
        for (i, f) in self.mep.feeders.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("mep.feeders[{i}]");
            for end in [&f.from, &f.to] {
                if !end.trim().is_empty() && !self.mep.equipment.iter().any(|e| e.tag.eq_ignore_ascii_case(end.trim())) {
                    out.push(Issue::warning(at.clone(), format!("{} feeds {end}, which isn't in the equipment", f.tag)));
                }
            }
            let counts_ok = (1..=100).contains(&f.sets) && (1..=4).contains(&f.phases) && f.neutrals <= 4 && f.grounds <= 4 && f.conduits <= 400;
            if f.tag.trim().is_empty() || f.size.trim().is_empty() || !counts_ok || !(fin(f.conduit_in) && (0.0..=8.0).contains(&f.conduit_in)) {
                out.push(Issue::error(at, "a feeder needs a tag, a conductor size, 1 to 100 sets of 1 to 4 phases and raceways up to 8 inches"));
            }
        }
        for (i, c) in self.rcp.ceilings.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("rcp.ceilings[{i}]");
            if self.level(&c.level).is_none() || !outline_ok(&c.outline) {
                out.push(Issue::error(at.clone(), "a ceiling needs a level and an outline"));
            }
            if !(fin(c.height_ft) && c.height_ft > 0.0 && c.height_ft <= 100.0) {
                out.push(Issue::error(format!("{at}.height_ft"), "the ceiling height must be more than 0 feet"));
            } else if let Some(l) = self.level(&c.level).filter(|l| c.height_ft > l.height_ft) {
                out.push(Issue::warning(
                    format!("{at}.height_ft"),
                    format!("the ceiling ({}) is above the next floor ({})", feet(c.height_ft), feet(l.height_ft)),
                ));
            }
            if let Some(g) = c.grid
                && (!g.spacing_in.iter().all(|v| fin(*v) && *v >= 6.0 && *v <= 120.0) || !g.origin.iter().all(|v| fin(*v)) || !fin(g.angle_deg))
            {
                out.push(Issue::error(format!("{at}.grid"), "a grid's tiles are 6\" to 120\""));
            }
        }
        for (i, sf) in self.rcp.soffits.iter().enumerate().take(MAX_ITEMS) {
            if self.level(&sf.level).is_none() || !outline_ok(&sf.outline) || !(fin(sf.height_ft) && sf.height_ft > 0.0) {
                out.push(Issue::error(format!("rcp.soffits[{i}]"), "a soffit needs a level, an outline and a height"));
            }
            if let Some(t) = &sf.soffit_type
                && !self.soffit_types.iter().any(|x| x.name.eq_ignore_ascii_case(t))
            {
                out.push(Issue::error(format!("rcp.soffits[{i}].soffit_type"), format!("there is no soffit type named {t}")));
            }
            if sf.top_ft.is_some_and(|v| !(fin(v) && v > sf.height_ft && v <= 100.0)) {
                out.push(Issue::error(format!("rcp.soffits[{i}].top_ft"), "a soffit's top is above its bottom"));
            }
            if sf.sides_ft.is_some_and(|v| !(fin(v) && (0.0..=100_000.0).contains(&v))) {
                out.push(Issue::error(format!("rcp.soffits[{i}].sides_ft"), "its sides are 0 feet long or more"));
            }
        }
        for (i, c) in self.rcp.ceilings.iter().enumerate().take(MAX_ITEMS) {
            if let Some(t) = &c.ceiling_type
                && !self.ceiling_types.iter().any(|x| x.name.eq_ignore_ascii_case(t))
            {
                out.push(Issue::error(format!("rcp.ceilings[{i}].ceiling_type"), format!("there is no ceiling type named {t}")));
            }
        }
        let layers_ok = |at: String, ls: &[AssemblyLayer], out: &mut Vec<Issue>| {
            if ls.len() > MAX_ASSEMBLY_LAYERS {
                out.push(Issue::error(at.clone(), format!("at most {MAX_ASSEMBLY_LAYERS} layers")));
            }
            for (j, l) in ls.iter().enumerate().take(MAX_ASSEMBLY_LAYERS) {
                if !(fin(l.thickness_in) && l.thickness_in > 0.0 && l.thickness_in <= 48.0) {
                    out.push(Issue::error(format!("{at}[{j}].thickness_in"), "a layer is more than 0 and at most 48 inches thick"));
                }
                if l.spacing_in.is_some_and(|v| !(fin(v) && (1.0..=240.0).contains(&v))) {
                    out.push(Issue::error(format!("{at}[{j}].spacing_in"), "framing is 1\" to 240\" apart"));
                }
            }
        };
        for (i, c) in self.ceiling_types.iter().enumerate().take(MAX_ITEMS) {
            layers_ok(format!("ceiling_types[{i}].layers"), &c.layers, &mut out);
            if c.grid_in.is_some_and(|g| !g.iter().all(|v| fin(*v) && (6.0..=120.0).contains(v))) {
                out.push(Issue::error(format!("ceiling_types[{i}].grid_in"), "a grid's tiles are 6\" to 120\""));
            }
        }
        for (i, t) in self.soffit_types.iter().enumerate().take(MAX_ITEMS) {
            layers_ok(format!("soffit_types[{i}].bottom"), &t.bottom, &mut out);
            layers_ok(format!("soffit_types[{i}].sides"), &t.sides, &mut out);
            layers_ok(format!("soffit_types[{i}].framing"), &t.framing, &mut out);
            if !(fin(t.above_ceiling_in) && (0.0..=120.0).contains(&t.above_ceiling_in)) {
                out.push(Issue::error(format!("soffit_types[{i}].above_ceiling_in"), "from 0 to 120 inches above the ceiling"));
            }
        }
        for (i, it) in self.rcp.items.iter().enumerate().take(100_000) {
            if self.level(&it.level).is_none() || !it.at.iter().chain(it.size_in.iter()).all(|v| fin(*v)) {
                out.push(Issue::error(format!("rcp.items[{i}]"), "a ceiling item needs a level, a place and a size"));
            }
        }
        for (i, f) in self.exterior.facades.iter().enumerate().take(MAX_ITEMS) {
            for (j, b) in f.bands.iter().enumerate().take(MAX_ITEMS) {
                if !(fin(b.from_ft) && fin(b.to_ft) && b.to_ft > b.from_ft) {
                    out.push(Issue::error(format!("exterior.facades[{i}].bands[{j}]"), "a band runs from a height up to a higher one"));
                }
                if let Some(a) = &b.assembly {
                    envelope_assembly_issues(&format!("exterior.facades[{i}].bands[{j}].assembly"), a, &mut out);
                }
            }
        }
        for (i, st) in self.stairs.iter().enumerate().take(MAX_ITEMS) {
            let at = format!("stairs[{i}]");
            if st.name.trim().is_empty() {
                out.push(Issue::error(format!("{at}.name"), "a stair needs a name"));
            } else if self.stairs.iter().take(i).any(|o| o.name.eq_ignore_ascii_case(&st.name)) {
                out.push(Issue::error(format!("{at}.name"), format!("two stairs are named {}", st.name)));
            }
            if self.level(&st.level).is_none() {
                out.push(Issue::error(format!("{at}.level"), format!("there is no level named {}", st.level)));
            }
            if let Some(n) = st.to_level.as_ref().filter(|n| self.level(n).is_none()) {
                out.push(Issue::error(format!("{at}.to_level"), format!("there is no level named {n}")));
            }
            if !(fin(st.width_in) && st.width_in > 0.0 && st.width_in <= 600.0) {
                out.push(Issue::error(format!("{at}.width_in"), "the width must be more than 0 and at most 600 inches"));
            }
            if st.runs.is_empty() || st.runs.len() > 100 {
                out.push(Issue::error(format!("{at}.runs"), "a stair has 1 to 100 runs"));
            }
            for (j, r) in st.runs.iter().enumerate().take(100) {
                let len = ((r.to[0] - r.from[0]).powi(2) + (r.to[1] - r.from[1]).powi(2)).sqrt();
                if !(len.is_finite() && len > 0.0) {
                    out.push(Issue::error(format!("{at}.runs[{j}]"), "a run must go somewhere: `from` and `to` differ"));
                }
                if !st.ramp && !(1..=200).contains(&r.treads) {
                    out.push(Issue::error(format!("{at}.runs[{j}].treads"), "a stair run has 1 to 200 treads"));
                }
            }
            if let Some((z0, z1)) = self.stair_heights(st) {
                if z1 <= z0 || z1.is_nan() || z0.is_nan() {
                    out.push(Issue::error(format!("{at}.to_level"), "it must reach a level above the one it starts on"));
                } else if let Some((n, riser, tread, len)) = self.stair_measures(st) {
                    if st.ramp {
                        let slope = (z1 - z0) / len.max(1e-9);
                        if slope > crate::stairs::RAMP_MAX_SLOPE + 1e-9 {
                            out.push(Issue::warning(&at, format!("the ramp rises 1:{:.1}, steeper than 1:12 (IBC 1012.2)", 1.0 / slope)));
                        }
                    } else if n > 0 {
                        if riser > crate::stairs::RISER_MAX_IN + 1e-9 {
                            out.push(Issue::warning(
                                &at,
                                format!(
                                    "{n} risers of {}: above the 7¾\" limit (IRC R311.7.5.1; 7\" under IBC 1011.5.2) — add treads",
                                    inches(riser)
                                ),
                            ));
                        } else if riser < crate::stairs::RISER_MIN_IN - 1e-9 {
                            out.push(Issue::warning(
                                &at,
                                format!("{n} risers of {}: under the 4\" minimum (IBC 1011.5.2) — fewer treads", inches(riser)),
                            ));
                        }
                        if tread.is_finite() && tread < crate::stairs::TREAD_MIN_IN - 1e-9 {
                            out.push(Issue::warning(
                                &at,
                                format!("treads of {}: under the 10\" minimum (IRC R311.7.5.2; 11\" under IBC)", inches(tread)),
                            ));
                        }
                    }
                }
            }
        }
        if let Some(s) = &self.site {
            if !fin(s.elevation_ft) {
                out.push(Issue::error("site.elevation_ft", "the ground elevation must be a number of feet"));
            }
            if !(fin(s.margin_ft) && (0.0..=10_000.0).contains(&s.margin_ft)) {
                out.push(Issue::error("site.margin_ft", "the margin must be from 0 to 10000 feet"));
            }
        }
        let o = &self.openings;
        if [o.door_head_ft, o.window_sill_ft, o.window_head_ft, o.min_ft, o.max_ft].iter().any(|v| !fin(*v)) || o.window_sill_ft >= o.window_head_ft {
            out.push(Issue::error("openings", "door head, window sill and head must be numbers, the sill below the head"));
        }
        out
    }
}

fn materials_text(m: &Surfaces) -> String {
    let parts = [
        ("walls", &m.wall),
        ("floors", &m.floor),
        ("ceilings", &m.ceiling),
        ("doors", &m.door),
        ("windows", &m.window),
        ("columns", &m.column),
        ("ground", &m.ground),
    ];
    parts.iter().filter_map(|(k, v)| v.as_ref().map(|v| format!("{k} {v}"))).collect::<Vec<_>>().join(", ")
}

/// Merge `patch` into `base` (both spec JSON): objects merge key by key, `null` removes a key; a
/// list of named items (levels, wall types, rooms, families by `name`; layers by `layer`) merges
/// item by item, an item with `"remove": true` removing it; other lists are replaced.
/// A roof's or a facade band's assembly: at most [`MAX_ASSEMBLY_LAYERS`] layers, each from 0
/// (a membrane or a barrier) to 48 inches thick.
fn envelope_assembly_issues(at: &str, a: &Assembly, out: &mut Vec<Issue>) {
    if a.layers.len() > MAX_ASSEMBLY_LAYERS {
        out.push(Issue::error(format!("{at}.layers"), format!("at most {MAX_ASSEMBLY_LAYERS} layers")));
    }
    for (j, l) in a.layers.iter().enumerate().take(MAX_ASSEMBLY_LAYERS) {
        if !(l.thickness_in.is_finite() && (0.0..=48.0).contains(&l.thickness_in)) {
            out.push(Issue::error(format!("{at}.layers[{j}].thickness_in"), "a layer is from 0 to 48 inches thick"));
        }
    }
}

pub fn merge(base: &mut serde_json::Value, patch: &serde_json::Value) {
    use serde_json::Value;
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() {
                    b.remove(k);
                    continue;
                }
                let key = match k.as_str() {
                    "layers" => Some("layer"),
                    "levels" | "wall_types" | "rooms" | "families" | "stairs" | "systems" => Some("name"),
                    _ => None,
                };
                match (b.get_mut(k), key, v) {
                    (Some(Value::Array(items)), Some(key), Value::Array(changes)) => {
                        for c in changes.iter().take(MAX_ITEMS) {
                            let id = c.get(key).and_then(Value::as_str).map(str::to_ascii_lowercase);
                            let at = id.as_ref().and_then(|id| {
                                items.iter().position(|x| x.get(key).and_then(Value::as_str).map(str::to_ascii_lowercase).as_ref() == Some(id))
                            });
                            let remove = c.get("remove").and_then(Value::as_bool) == Some(true);
                            match (at, remove) {
                                (Some(i), true) => {
                                    items.remove(i);
                                }
                                (Some(i), false) => {
                                    if let Some(x) = items.get_mut(i) {
                                        // The item keeps its own spelling of the name.
                                        let id = x.get(key).cloned();
                                        merge(x, c);
                                        if let (Some(id), Value::Object(o)) = (id, x) {
                                            o.insert(key.to_string(), id);
                                        }
                                    }
                                }
                                (None, false) => {
                                    if items.len() < MAX_ITEMS {
                                        items.push(c.clone());
                                    }
                                }
                                (None, true) => {}
                            }
                        }
                    }
                    (Some(old @ Value::Object(_)), _, Value::Object(_)) => merge(old, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

/// The level whose region holds a sheet point (the first without a region when none does).
pub fn level_at(levels: &[Level], x: f64, y: f64) -> Option<&Level> {
    levels
        .iter()
        .filter(|l| l.sheet.is_none())
        .find(|l| l.region.is_some_and(|[x0, y0, x1, y1]| x >= x0.min(x1) && x <= x0.max(x1) && y >= y0.min(y1) && y <= y0.max(y1)))
        .or_else(|| levels.iter().find(|l| l.sheet.is_none() && l.region.is_none()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_round_trip_through_a_spec() {
        let r = Rules::fallback();
        let s = BuildingSpec::from_rules(&r);
        assert_eq!(s.to_rules(), r);
        assert!(s.check().iter().all(|i| !i.error), "{:?}", s.check());
        assert_eq!(BuildingSpec::from_json(&s.to_json()), Ok(s.clone()));
        let mut newer = serde_json::to_value(&s).unwrap_or_default();
        newer["version"] = serde_json::json!(SPEC_VERSION + 1);
        assert!(BuildingSpec::from_json(&newer.to_string()).is_err());
    }

    #[test]
    fn plain_language() {
        assert_eq!(feet(10.0), "10'-0\"");
        assert_eq!(feet(-0.5), "-0'-6\"");
        assert_eq!(inches(4.875), "4 7/8\"");
        assert_eq!(inches(6.0), "6\"");
        let mut s = BuildingSpec::from_rules(&Rules::fallback());
        s.levels[0].sheet = Some("A112".into());
        s.levels[0].elevation_ft = 10.0;
        s.wall_types.push(WallType { name: "W".into(), height_ft: None, thickness_in: 4.875, from: WallFrom::Pairs, assembly: None });
        s.layers[0].wall_type = Some("w".into());
        s.approved.push("LEVELS/level 1".into());
        let d = s.describe();
        assert_eq!(d[0].text, "Level 1 = sheet A112 at 10'-0\"; 10'-0\" floor to floor, 8'-0\" ceilings");
        assert!(d[0].approved);
        assert_eq!(d[1].text, "*WALL* = full height walls 4 7/8\" thick (line pairs)");
        assert!(!d[1].approved);
    }

    #[test]
    fn wall_assemblies_and_marks() {
        let mut s = BuildingSpec::from_rules(&Rules::fallback());
        let layer = |material: &str, thickness_in: f64, side: LayerSide, in_cavity: bool| AssemblyLayer {
            material: material.into(),
            thickness_in,
            side,
            category: String::new(),
            in_cavity,
            spacing_in: None,
        };
        let a = Assembly {
            tag: "1B".into(),
            layers: vec![
                layer("5/8\" Type X gypsum board", 0.625, LayerSide::Side1, false),
                layer("3 5/8\" metal studs", 3.625, LayerSide::Core, false),
                layer("3 1/2\" batt insulation", 3.5, LayerSide::Core, true),
                layer("5/8\" Type X gypsum board", 0.625, LayerSide::Side2, false),
            ],
            height: Some(HeightRule::AboveCeiling),
            above_ceiling_in: 6.0,
            fire_rating: Some("1 HR".into()),
            acoustic_rating: Some("STC 45".into()),
            ..Assembly::default()
        };
        assert!((a.thickness_in() - 4.875).abs() < 1e-12);
        s.wall_types.push(WallType { name: "1B".into(), height_ft: None, thickness_in: 4.875, from: WallFrom::Pairs, assembly: Some(a) });
        let level = s.levels[0].name.clone();
        s.walls.push(WallMark { level: level.clone(), a: [0.0, 0.0], b: [120.0, 0.0], wall_type: "1b".into(), height_ft: Some(9.5), guess: None });
        assert!(s.check().iter().all(|i| !i.error), "{:?}", s.check());
        assert_eq!(BuildingSpec::from_json(&s.to_json()), Ok(s.clone()));
        let d = s.describe();
        let t = d.iter().find(|x| x.key == "wall_types/1B").map(|x| x.text.clone()).unwrap_or_default();
        assert!(t.contains("6\" above the ceiling") && t.contains("1 HR") && t.contains("STC 45"), "{t}");
        // A thickness the layers don't make, a mark on no type, a bad layer: said.
        s.wall_types[0].thickness_in = 6.0;
        s.walls.push(WallMark { level, a: [0.0, 0.0], b: [0.0, 0.0], wall_type: "nope".into(), height_ft: Some(f64::NAN), guess: None });
        if let Some(a) = s.wall_types[0].assembly.as_mut() {
            a.layers[0].thickness_in = -1.0;
        }
        let issues = s.check();
        for path in
            ["wall_types[0].assembly", "walls[1].wall_type", "walls[1]", "walls[1].height_ft", "wall_types[0].assembly.layers[0].thickness_in"]
        {
            assert!(issues.iter().any(|i| i.path == path), "{path}: {issues:?}");
        }
    }

    #[test]
    fn merge_by_name() {
        let mut base = serde_json::json!({"levels": [{"name": "L1", "height_ft": 10}, {"name": "L2", "height_ft": 10}], "site": {"margin_ft": 5}});
        merge(
            &mut base,
            &serde_json::json!({"levels": [{"name": "l2", "height_ft": 12}, {"name": "L1", "remove": true}, {"name": "L3", "height_ft": 9}], "site": null}),
        );
        assert_eq!(base, serde_json::json!({"levels": [{"name": "L2", "height_ft": 12}, {"name": "L3", "height_ft": 9}]}));
    }

    #[test]
    fn roof_and_band_assemblies() {
        let mut s = BuildingSpec::from_rules(&Rules::fallback());
        let level = s.levels.first().map(|l| l.name.clone()).unwrap_or_default();
        let layer = |material: &str, thickness_in: f64| AssemblyLayer { material: material.into(), thickness_in, ..AssemblyLayer::default() };
        let asm = Assembly {
            tag: "R-1".into(),
            layers: vec![layer("60 mil TPO membrane", 0.0), layer("1/2\" cover board", 0.5), layer("(2) 2\" polyiso", 4.0)],
            ..Assembly::default()
        };
        s.exterior.roofs.push(Roof {
            name: "R-1".into(),
            level: level.clone(),
            outline: vec![[0.0, 0.0], [100.0, 0.0], [100.0, 50.0]],
            slope: None,
            parapet_ft: Some(3.0),
            assembly: Some(asm.clone()),
        });
        s.exterior.facades.push(Facade {
            side: Side::North,
            bands: vec![Band {
                kind: BandKind::Material,
                from_ft: 0.0,
                to_ft: 14.0,
                start_ft: None,
                end_ft: None,
                material: Some("Brick".into()),
                assembly: Some(Assembly { tag: "EW-1".into(), ..asm.clone() }),
            }],
        });
        assert!(s.check().iter().all(|i| !i.error), "{:?}", s.check());
        let back = BuildingSpec::from_json(&s.to_json()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back, s);
        let d = s.describe();
        assert!(d.iter().any(|x| x.text.contains("R-1 top down: 60 mil TPO membrane, 1/2\" cover board")), "{d:?}");
        assert!(d.iter().any(|x| x.text.contains("(EW-1, 3 layers)")), "{d:?}");
        // Hostile layers are refused.
        if let Some(a) = s.exterior.roofs.first_mut().and_then(|r| r.assembly.as_mut()) {
            a.layers.push(layer("bad", f64::NAN));
            a.layers.extend((0..100).map(|_| layer("x", 1.0)));
        }
        let issues = s.check();
        assert!(issues.iter().any(|i| i.error && i.path.ends_with("layers")), "{issues:?}");
        assert!(issues.iter().any(|i| i.error && i.path.contains("thickness_in")), "{issues:?}");
    }
}
