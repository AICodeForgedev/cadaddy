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
    /// Stack the built levels into one building: `model.build` moves each level's 3D from over
    /// its own plan (plans drawn side by side on one sheet) onto the lowest level's, by the
    /// levels' bases (else the lower-left corners of their walls). The plans stay where they are.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stack: bool,
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
    /// The whole-job estimate (`wholejob.*`): every trade's quantities and scope, price packs,
    /// markups, general conditions and sub bids.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub wholejob: serde_json::Value,
    /// Field progress (`progress.*`): the plan's driver quantities, installed-work marks and
    /// timecard hours. Production rates are never kept here: they go to the company's private
    /// field observations.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub progress: serde_json::Value,
    /// The BIM view (`bim.look`, `bim.trades`, `bim.tree`): the trade colour table, ghosting,
    /// trades and tree branches shown, the level isolated.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub bim_view: serde_json::Value,
    /// The construction sequence's settings (`schedule.*`): start date, crews, working days,
    /// zone size, rates and the edits made to activities. The sequence itself is built from the
    /// model each time.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub sequence: serde_json::Value,
    /// Typical units (`unittypes.*`, Q305): the unit types read from the enlarged unit plans and
    /// their instances on the floor plans, with hands and overrides. Kept as written.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub unit_types: serde_json::Value,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub unit_instances: serde_json::Value,
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
    /// Elevator hoistways with their pits (`arch.read`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elevators: Vec<Hoistway>,
    /// Each storey's corridors, stairs and elevator hoistways, traced from their labels inside the
    /// shell before the rooms: the rooms are traced in what they leave.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub circulation: Vec<Circulation>,
    /// Each storey's shell (its exterior walls' boxes `[x0, y0, x1, y1]`, sheet units) by level
    /// name: a wall line reaching well past it (a site wall, a stray line) is no wall of the
    /// storey, and doesn't stretch its slab or the roof.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shells: BTreeMap<String, Vec<[f64; 4]>>,
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
    /// What the trades' documents say (`trades.read`, Q6162: concrete mixes, the PT design,
    /// joist and truss layouts, door hardware sets, accessories…, each fact with its file and
    /// page) and the project's trade rules: the engine's trade atoms are laid out from them.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub trades: serde_json::Value,
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
    /// The mounting heights the set's notes give kinds of items (a general note's list, an
    /// accessibility sheet's), read off sheets the model doesn't hold (`job.start` reads them):
    /// the build and Validate take a device's height from them when nothing nearer gives one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mount_notes: Vec<MountNote>,
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
    /// Where values read from the prints came from, by key (`levels/Level 2` → `layout:A-301:
    /// LEVEL 2 = 14'-0"`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, String>,
    /// Hashed signatures of the sheet's print objects at the time of the first revision compare,
    /// so later compares keep user markup (objects not in this set).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub print_baseline: Vec<u64>,
    /// Building-code rule book selection and project-level amendments (`code.*`): which codes,
    /// editions and jurisdiction apply to this project. The rules themselves are the starter set
    /// shipped with CADaddy plus any user additions.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub codebook: serde_json::Value,
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
    /// Where it is in the spec's JSON, to show and patch it; none: it can't be edited part by
    /// part (a room without a name).
    pub path: Option<PartPath>,
}

/// Where a described part is in the spec's JSON (`serde_json::to_value` of the spec): what an
/// editor shows for it, and the [`merge`] patch that changes or removes just that part. Every key
/// [`BuildingSpec::describe`] gives has one here, so a palette or an agent never patches the
/// wrong place.
#[derive(Clone, Debug, PartialEq)]
pub enum PartPath {
    /// A whole value (`materials`, `site`, `openings`, `walls`, `rcp.items`); `removable`: a
    /// `null` patch removes it (back to its default).
    Value { at: &'static [&'static str], removable: bool },
    /// The item of a list [`merge`] merges item by item, whose `field` is `id` (case ignored):
    /// levels, layers, wall, ceiling and soffit types, rooms, families, stairs, MEP systems.
    Named { at: &'static [&'static str], field: &'static str, id: String },
    /// Items of a list [`merge`] replaces whole (roofs, facades, RCP ceilings, columns, schedule
    /// rows…): the patch is the whole list with them changed or left out.
    Items { at: &'static [&'static str], pick: Pick },
}

/// Which items of a list a [`PartPath::Items`] is.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// The one item at this place in the list.
    Index(usize),
    /// Every item whose `field` is this text (`columns` on a level, `schedule.rows` of a kind),
    /// shown and edited as a list.
    Field(&'static str, String),
}

impl PartPath {
    fn list<'a>(&self, spec: &'a serde_json::Value) -> &'a serde_json::Value {
        let at = match self {
            PartPath::Value { at, .. } | PartPath::Named { at, .. } | PartPath::Items { at, .. } => at,
        };
        at.iter().fold(spec, |v, k| &v[*k])
    }

    fn wrap(&self, v: serde_json::Value) -> serde_json::Value {
        let at = match self {
            PartPath::Value { at, .. } | PartPath::Named { at, .. } | PartPath::Items { at, .. } => at,
        };
        at.iter().rev().fold(v, |v, k| serde_json::json!({ *k: v }))
    }

    /// The part's JSON in `spec`: the item, the picked items (a list for [`Pick::Field`]), or
    /// the value. `null` when it isn't there.
    pub fn get(&self, spec: &serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        let items = || self.list(spec).as_array().into_iter().flatten();
        match self {
            PartPath::Value { .. } => self.list(spec).clone(),
            PartPath::Named { field, id, .. } => {
                items().find(|x| x[*field].as_str().is_some_and(|n| n.eq_ignore_ascii_case(id))).cloned().unwrap_or(Value::Null)
            }
            PartPath::Items { pick: Pick::Index(i), .. } => self.list(spec).get(*i).cloned().unwrap_or(Value::Null),
            PartPath::Items { pick: Pick::Field(f, id), .. } => Value::Array(items().filter(|x| x[*f].as_str() == Some(id)).cloned().collect()),
        }
    }

    /// The [`merge`] patch that puts `edited` in the part's place, or removes the part (`None`).
    /// None when the part can't be removed (`openings`).
    pub fn patch(&self, spec: &serde_json::Value, edited: Option<serde_json::Value>) -> Option<serde_json::Value> {
        use serde_json::Value;
        let inner = match self {
            PartPath::Value { removable, .. } => match edited {
                Some(v) => v,
                None if *removable => Value::Null,
                None => return None,
            },
            PartPath::Named { field, id, .. } => {
                let old = self.get(spec);
                let name = old.get(*field).cloned().unwrap_or_else(|| Value::String(id.clone()));
                let mut items = Vec::new();
                let edited = edited.map(|mut e| {
                    // An edit without the name keeps the part's own.
                    if let Value::Object(o) = &mut e {
                        o.entry(field.to_string()).or_insert_with(|| name.clone());
                    }
                    e
                });
                let renamed = edited.as_ref().and_then(|e| e[*field].as_str()).is_some_and(|n| !n.eq_ignore_ascii_case(id));
                if edited.is_none() || renamed {
                    items.push(serde_json::json!({ *field: name, "remove": true }));
                }
                items.extend(edited);
                Value::Array(items)
            }
            PartPath::Items { pick, .. } => {
                let old = self.list(spec).as_array().cloned().unwrap_or_default();
                let new: Vec<Value> = match edited {
                    None => Vec::new(),
                    Some(Value::Array(a)) if matches!(pick, Pick::Field(..)) => a,
                    Some(v) => vec![v],
                };
                let picked = |i: usize, x: &Value| match pick {
                    Pick::Index(at) => i == *at,
                    Pick::Field(f, id) => x[*f].as_str() == Some(id),
                };
                // Each picked item's place takes the next edited one (so an unchanged edit keeps
                // the order); extra ones follow the last picked, missing ones are left out.
                let last = old.iter().enumerate().filter(|(i, x)| picked(*i, x)).map(|(i, _)| i).next_back();
                let mut out = Vec::with_capacity(old.len() + new.len());
                let mut new = new.into_iter();
                for (i, x) in old.into_iter().enumerate() {
                    if !picked(i, &x) {
                        out.push(x);
                        continue;
                    }
                    out.extend(new.next());
                    if Some(i) == last {
                        out.extend(new.by_ref());
                    }
                }
                out.extend(new);
                Value::Array(out)
            }
        };
        Some(self.wrap(inner))
    }
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
    /// Where it was read from: the legend sheet and tag (`A-501 wall type legend, tag 1B`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
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
    /// The plan sheet it was read from (`A-101`, `A-101 (no tag)`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Its type read from a tag on the drawing, or guessed; none = not said (older specs: a mark
    /// with a `guess` is a guess).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<MarkStatus>,
    /// Where its height came from (`ceiling`, `deck`, `no ceiling beside it: deck`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub height_source: String,
}

/// Whether a wall mark's type is from the drawing or a guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkStatus {
    /// A tag on the plan names it.
    Drawing,
    /// No tag: the likeliest type (`guess` says why).
    Guess,
}

impl WallMark {
    /// Its status as written, else a guess when it says why.
    pub fn status(&self) -> MarkStatus {
        self.status.unwrap_or(if self.guess.is_some() { MarkStatus::Guess } else { MarkStatus::Drawing })
    }
}

fn is_six(v: &f64) -> bool {
    (*v - 6.0).abs() < 1e-9
}

/// A room, found by its tag text (`name`, or `number`) on the plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// Where `ceiling_ft` was read (`9'-0" on A-501 ENLARGED UNIT RCP`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling_source: Option<String>,
    #[serde(default)]
    pub materials: Surfaces,
    /// Base finish from the room finish schedule (`RB-1 4" RUBBER BASE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Device plate colour from its wall finish (the finish legend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plates: Option<String>,
    /// Where its tag is when the plans print none (another trade's plan tags it): the room is
    /// traced from here, and no label of the plans claims it by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
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
    /// The code its risers and treads are checked against: "ibc" (commercial, IBC 1011.5.2:
    /// risers 4″–7″, treads ≥ 11″; the default) or "irc" (residential, IRC R311.7.5: risers
    /// ≤ 7¾″, treads ≥ 10″).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl Stair {
    /// Checked against the residential code (`code: "irc"`) rather than the commercial one.
    pub fn residential(&self) -> bool {
        self.code.as_deref().is_some_and(|c| c.trim().eq_ignore_ascii_case("irc"))
    }
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drains: Vec<RoofDrain>,
    /// Parapet heights the elevations give, by roof level: kept here so they reach roofs read
    /// later, whatever the reading order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parapets: Vec<LevelParapet>,
}

impl Exterior {
    pub fn is_empty(&self) -> bool {
        self.facades.is_empty()
            && self.roofs.is_empty()
            && self.crickets.is_empty()
            && self.equipment.is_empty()
            && self.drains.is_empty()
            && self.parapets.is_empty()
    }

    /// The parapet height an elevation gives for a level, feet.
    pub fn level_parapet(&self, level: &str) -> Option<f64> {
        self.parapets.iter().find(|p| p.level.eq_ignore_ascii_case(level)).map(|p| p.height_ft)
    }
}

/// A parapet height read from an elevation (above the roof line), for the roofs of a level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelParapet {
    pub level: String,
    pub height_ft: f64,
    /// The facade whose elevation gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facade: Option<Side>,
}

/// Whether a value was read from the drawings or is a default put in because they don't say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueStatus {
    Read,
    Default,
}

impl ValueStatus {
    pub fn is_default(status: &Option<ValueStatus>) -> bool {
        *status == Some(ValueStatus::Default)
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
    /// Whether the parapet height was read (`parapet_from` says where) or is a default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parapet_status: Option<ValueStatus>,
    /// Where the parapet height came from: `roof plan` (its note), `elevation` or `default`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parapet_from: Option<String>,
    /// What the roof is built of, top down (membrane, cover board, insulation, deck).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly: Option<Assembly>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoofSlope {
    pub down: [f64; 2],
    /// Rise per foot of run (¼"/ft = 0.0208, 4:12 = 0.333).
    pub rise_per_ft: f64,
    /// Whether the downhill direction was read from the arrow's head or is a default (the
    /// arrow's drawing order, when its head can't be told).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction_status: Option<ValueStatus>,
}

/// A cricket: a raised ridge diverting water, its outline on the roof and its height.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cricket {
    pub level: String,
    pub outline: Vec<[f64; 2]>,
    pub height_ft: f64,
    /// Whether the height was read from its note or is the 6" default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_status: Option<ValueStatus>,
}

/// Rooftop equipment (RTU, AHU, exhaust fan) as a box over its outline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Equipment {
    pub name: String,
    pub level: String,
    pub outline: Vec<[f64; 2]>,
    pub height_ft: f64,
    /// Whether the height was read from its note or is the 4'-0" default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_status: Option<ValueStatus>,
}

/// A roof drain on the roof plan (sheet units, on its level's plan).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoofDrain {
    pub level: String,
    pub at: [f64; 2],
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

/// A door or window tag on a plan: its mark and where it is (sheet units). Read off the
/// architectural plans (`arch.read`), a door's tag also says how it swings: its hand, the rooms
/// it swings into and from, its hinge and latch points (sheet units) and whether its hardware
/// has a card reader; and where it was read (`source` sheet, `status`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpeningTag {
    pub mark: String,
    pub at: [f64; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// `LH`, `RH`, `LHR` or `RHR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hand: Option<String>,
    /// Room numbers either side: the one it swings into, the one it swings from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swings_into: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_room: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hinge: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latch: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub card_reader: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// `drawing`, `schedule` or `guess`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// What a circulation space is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CirculationKind {
    #[default]
    Corridor,
    Stair,
    /// An elevator's shaft: closed floor to roof, no duct, pipe or conduit runs through it.
    Hoistway,
}

/// A corridor, stair or elevator hoistway on a level's plan: its floor as rectangles (sheet
/// units), traced from its label.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Circulation {
    pub kind: CirculationKind,
    pub level: String,
    /// Its label (`CORRIDOR`, `STAIR 01`, `ELEV.`).
    pub name: String,
    pub at: [f64; 2],
    pub rects: Vec<[[f64; 2]; 2]>,
}

impl Circulation {
    /// Whether `p` is on its floor.
    pub fn holds(&self, p: [f64; 2]) -> bool {
        self.rects.iter().any(|[a, b]| p[0] >= a[0] && p[0] <= b[0] && p[1] >= a[1] && p[1] <= b[1])
    }

    /// Its floor area, sheet units squared.
    pub fn area(&self) -> f64 {
        self.rects.iter().map(|[a, b]| (b[0] - a[0]).max(0.0) * (b[1] - a[1]).max(0.0)).sum()
    }

    /// The edges round its floor: the rectangles' sides less where one rectangle meets another.
    pub fn edges(&self) -> Vec<([f64; 2], [f64; 2])> {
        let mut out = Vec::new();
        // Along x (the rectangles' bottoms and tops), then along y (their sides): at each line,
        // the spans one side of it covers and the other doesn't.
        for axis in [1usize, 0] {
            let along = 1 - axis;
            let mut lines: std::collections::BTreeMap<u64, (f64, Vec<(f64, f64)>, Vec<(f64, f64)>)> = std::collections::BTreeMap::new();
            for [a, b] in self.rects.iter().filter(|[a, b]| b[0] > a[0] && b[1] > a[1]) {
                let span = (a[along], b[along]);
                lines.entry(a[axis].to_bits()).or_insert((a[axis], Vec::new(), Vec::new())).1.push(span);
                lines.entry(b[axis].to_bits()).or_insert((b[axis], Vec::new(), Vec::new())).2.push(span);
            }
            for (at, starts, ends) in lines.values() {
                let mut cuts: Vec<f64> = starts.iter().chain(ends).flat_map(|s| [s.0, s.1]).collect();
                cuts.sort_by(f64::total_cmp);
                cuts.dedup();
                let covers = |spans: &[(f64, f64)], m: f64| spans.iter().any(|s| m > s.0 && m < s.1);
                let mut runs: Vec<(f64, f64)> = Vec::new();
                for w in cuts.windows(2) {
                    let m = (w[0] + w[1]) / 2.0;
                    if covers(starts, m) != covers(ends, m) {
                        match runs.last_mut() {
                            Some(r) if r.1 == w[0] => r.1 = w[1],
                            _ => runs.push((w[0], w[1])),
                        }
                    }
                }
                out.extend(runs.into_iter().map(|r| Self::segment(axis, *at, r)));
            }
        }
        out
    }

    /// Its floor's outline: its edges joined end to end, the loop round the most area (any other
    /// is a hole in it).
    pub fn outline(&self) -> Vec<[f64; 2]> {
        let edges = self.edges();
        let key = |p: [f64; 2]| (p[0].to_bits(), p[1].to_bits());
        let mut ends: std::collections::HashMap<(u64, u64), Vec<usize>> = std::collections::HashMap::new();
        for (i, (a, b)) in edges.iter().enumerate() {
            ends.entry(key(*a)).or_default().push(i);
            ends.entry(key(*b)).or_default().push(i);
        }
        let mut used = vec![false; edges.len()];
        let mut best: (f64, Vec<[f64; 2]>) = (0.0, Vec::new());
        for first in 0..edges.len() {
            if used[first] {
                continue;
            }
            used[first] = true;
            let (start, mut p) = edges[first];
            let mut ring = vec![start];
            while key(p) != key(start) && ring.len() <= edges.len() {
                ring.push(p);
                let Some(&next) = ends.get(&key(p)).and_then(|ids| ids.iter().find(|i| !used[**i])) else { break };
                used[next] = true;
                let (a, b) = edges[next];
                p = if key(a) == key(p) { b } else { a };
            }
            let area = ring.iter().zip(ring.iter().cycle().skip(1)).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum::<f64>().abs() / 2.0;
            if area > best.0 {
                best = (area, ring);
            }
        }
        best.1
    }

    fn segment(axis: usize, at: f64, (a, b): (f64, f64)) -> ([f64; 2], [f64; 2]) {
        if axis == 1 { ([a, at], [b, at]) } else { ([at, a], [at, b]) }
    }
}

/// An elevator hoistway read off the plans: its box (sheet units, on the plan of the lowest level
/// it serves), the levels it serves, its pit depth and capacity.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hoistway {
    pub name: String,
    #[serde(default)]
    pub levels: Vec<String>,
    pub min: [f64; 2],
    pub max: [f64; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pit_ft: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_lb: Option<u32>,
    /// The sheet it was read from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
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
    /// The drawn outline it was read from (its handle, hex); empty for a column typed in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// How it is known: `rule` (read from the plan by `grids.columns`); empty for a column typed in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
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
    /// The panel schedules as printed (their circuits and breaker types), panels the plans place
    /// or not.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub panel_schedules: Vec<PanelSchedule>,
    /// Equipment connections' electrical data from their submittals, beside what the drawings
    /// give them (protection, feeder): what the build checks and reports.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub equipment_electrical: Vec<EquipmentElectrical>,
    /// The light fixture schedule as printed: each type's basis of design and data.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fixture_schedule: Vec<FixtureType>,
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
    /// Utility and service entrances (power, telephone, CATV, fire alarm monitoring, gas meter
    /// bonding): what comes into the building underground and where.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<Service>,
    /// The service kinds this building must have (empty: power and telephone), each one an
    /// underground entrance laid out before the slab.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub required_services: Vec<String>,
    /// Per-trade default elevation bands for MEP elements (configurable; overrides the hard-coded
    /// defaults when set). Each entry maps a trade to its default bottom-of-duct/pipe/equipment
    /// height above the floor, in feet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elevation_bands: Vec<ElevationBand>,
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
    /// Concrete-encased: the concrete element it is cast in (a footing, grade beam, pier or pole
    /// base id in `concrete.elements`), when named; else the one under `at` is taken.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub element: String,
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
            element: String::new(),
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
    /// The sheet its circuit was read from (`E-101`), empty when entered by hand.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// A light fixture's type (`A`, `B`) as the plan tags it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub type_tag: String,
    /// Its type's row in the light fixture schedule (column → value as printed: description,
    /// mounting, lamp and wattage).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
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
            && self.panel_schedules.is_empty()
            && self.equipment_electrical.is_empty()
            && self.fixture_schedule.is_empty()
            && self.gear_rules.is_none()
            && self.devices.is_empty()
            && self.arc_flash.is_empty()
            && self.labels_printed.is_empty()
            && self.grounding.is_none()
            && self.services.is_empty()
            && self.required_services.is_empty()
            && self.elevation_bands.is_empty()
    }
}

/// A utility or service entrance: conduits from the property line or the utility's structure
/// (a transformer pad, a handhole, the pedestal) to where they come up in the building, and the
/// equipment they feed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Service {
    /// `power`, `telephone`, `catv`, `fire_alarm`, `gas_bond`, or the project's own word.
    pub kind: String,
    pub tag: String,
    pub level: String,
    /// Where it starts outside the building and where it comes up inside (sheet units); `at`
    /// missing: the drawings don't say where it enters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    /// The equipment it feeds (an equipment tag: `MSB`, the MDF's backboard, the FACP).
    pub to: String,
    pub conduits: u32,
    pub conduit_in: f64,
    /// `PVC Sch 40`, `PVC Sch 80`, `RGS`.
    pub material: String,
    /// Cover over the conduits, inches; encased in concrete.
    pub cover_in: f64,
    pub encased: bool,
    /// Drawn on the plans as a route; `false`: only named in a note, a detail or the specs (the
    /// field can't lay it out from the drawings).
    pub shown: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// The route as drawn, from the site end to where it enters (sheet units); empty: `from`
    /// and `at`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub route: Vec<[f64; 2]>,
    /// A pipe's size as printed (inches): water, sewer, storm, gas.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_in: Option<f64>,
    /// Depth below grade and invert elevation as printed (feet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth_ft: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invert_ft: Option<f64>,
    /// Where a print reader found it: the box on `source`'s drawing (its own coordinates), and
    /// how sure the reader is (0 … 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub found_in: Option<[[f64; 2]; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

impl Default for Service {
    fn default() -> Self {
        Service {
            kind: String::new(),
            tag: String::new(),
            level: String::new(),
            from: None,
            at: None,
            to: String::new(),
            conduits: 1,
            conduit_in: 0.0,
            material: String::new(),
            cover_in: 0.0,
            encased: false,
            shown: true,
            source: String::new(),
            route: Vec::new(),
            size_in: None,
            depth_ft: None,
            invert_ft: None,
            found_in: None,
            confidence: None,
        }
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
    /// Where these provisions came from: `LUG SCHEDULE E-601`, `one-line E-501`,
    /// `submittal 26 24 16-01` (an approved submittal overrides the schedule). Empty: by hand.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
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
            source: String::new(),
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
    /// Where it was read from: `FEEDER SCHEDULE E-601`, `one-line E-501`. Empty: by hand.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
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
            source: String::new(),
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
    /// Where its footprint, height and elevation came from (`outline`, `schedule`, `rule`…);
    /// gear read off the E-sheets also has `position` (`E-sheet tag E-101` until an
    /// architectural position confirms it) and `status` (`unconfirmed` / `confirmed`).
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
    /// Its breaker's type codes as the schedule marks it (`AF`, `GF`; `AF/GF` both), the
    /// schedule's legend saying what each is ([`PanelSchedule::legend`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub protection: Vec<String>,
    /// Its wire size as the schedule prints it (`12`, `4/0`), and its load, VA.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub wire: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_va: Option<f64>,
    /// The sheet it was read from (`E-601`), empty when entered by hand.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
}

/// A light fixture schedule's row: the type and its basis of design.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FixtureType {
    pub tag: String,
    pub manufacturer: String,
    pub model: String,
    pub lamp: String,
    pub va: Option<f64>,
    pub voltage: String,
    pub installation: String,
    pub description: String,
    /// The schedule's note numbers on it (`2,3,5`).
    pub notes: String,
    /// The sheet it was read from.
    pub source: String,
    /// The quantity the schedule gives, when it has a quantity column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<u32>,
}

/// An equipment connection's electrical data (its submittal's, else unknown) and the drawings'
/// provision for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EquipmentElectrical {
    /// Its tag on the one-line (`ELEV-1`) and what the schedule calls it.
    pub tag: String,
    pub description: String,
    /// The drawings': its source panel, protection (`70A/3P`) and feeder type tag.
    pub source_panel: String,
    pub drawing_ocpd: String,
    pub feeder: String,
    /// The submittal's: volts and phases, nameplate / full-load / minimum circuit amps, the most
    /// protection it allows (amps, and the fuse class when it names one), acceleration amps,
    /// horsepower and kW, the controller's short-circuit current rating (amps).
    pub volts: Option<f64>,
    pub phase: Option<u32>,
    pub nameplate_a: Option<f64>,
    pub fla: Option<f64>,
    pub mca: Option<f64>,
    pub max_ocpd_a: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fuse_class: String,
    pub accel_a: Option<f64>,
    pub hp: Option<f64>,
    pub kw: Option<f64>,
    pub sccr_a: Option<f64>,
    /// Where its data came from (the submittal's file and page, or an agent's reading of it) and
    /// the submittal's review status.
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
}

/// A panel's schedule as printed (a panel the plans may not place, or a typical one every guest
/// room has): its circuits, and the legend its breaker codes are read with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelSchedule {
    /// The panel's tag (`G`, `DP1`).
    pub panel: String,
    /// "TYPICAL OF 124 REQUIRED": the number of panels it is the schedule of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typical_of: Option<u32>,
    /// The sheet it is on.
    pub source: String,
    pub circuits: Vec<PanelCircuit>,
    /// The schedule legend's codes: `GF` → `GROUND FAULT INTERRUPTER`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub legend: BTreeMap<String, String>,
    /// Its header's facts as printed: `bus amps` → `125A`, `volts/phase`, `mounting`, `location`…
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub header: BTreeMap<String, String>,
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

/// A per-trade default elevation band: the default bottom height (feet above the floor) for
/// MEP elements of a given trade when no height tag is present on the drawing. The agent or
/// owner can set these via `bspec.set` to match the project's coordination drawings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ElevationBand {
    /// Trade identifier: `"duct"`, `"pipe"`, `"conduit"`, `"terminal"`, `"equipment"`.
    pub trade: String,
    /// Default bottom-of-element height above the floor, feet.
    pub bottom_ft: f64,
}

impl Mep {
    /// Look up the elevation band for a trade; returns `None` if no band is set (use hard-coded
    /// default).
    pub fn band_bottom_ft(&self, trade: &str) -> Option<f64> {
        self.elevation_bands.iter().find(|b| b.trade == trade).map(|b| b.bottom_ft)
    }
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
        if self.kind == MepKind::Pipe {
            return pipe_system_default_color(&self.name);
        }
        match self.kind {
            MepKind::Duct => 4,    // cyan (generic duct, no type set)
            MepKind::Pipe => 3,    // green (unreachable due to the check above)
            MepKind::Conduit => 6, // magenta
            MepKind::Tray => 30,   // orange
            MepKind::Cable => 210, // pink: an assumed homerun route
        }
    }
}

/// Default ACI colour for a pipe system by name. Each major plumbing / fire-protection system
/// gets a distinct colour so runs are visually separable in the 3D model.
pub fn pipe_system_default_color(system: &str) -> i16 {
    match system {
        "CW" => 5,     // blue — cold water
        "HW" => 1,     // red — hot water
        "HWR" => 14,   // dark red — hot water return
        "SAN" => 30,   // orange — sanitary
        "V" => 3,      // green — vent
        "G" => 50,     // yellow — gas
        "FP" => 6,     // magenta — fire protection
        "CHW" => 4,    // cyan — chilled water
        "CHWR" => 140, // light blue — chilled water return
        "ST" => 8,     // dark grey — steam
        _ => 3,        // green — generic pipe
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
    /// Buried in a concrete encasement (a duct bank), as the drawings say.
    #[serde(default, skip_serializing_if = "is_false")]
    pub encased: bool,
    /// Where the size came from: tag, note, ceiling, rule, default, override.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub size_source: String,
    /// Where the height came from: tag, note, ceiling, rule, default, override.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub height_source: String,
    /// Whether this run has been reviewed by a person or agent.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reviewed: bool,
    /// Who last edited this run (empty if not edited).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub edited_by: String,
    /// A vertical run (riser) from `bottoms_ft[0]` up to this elevation (feet above the floor of
    /// the level it starts on). When set, the run has a single plan point and goes straight up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_ft: Option<f64>,
    /// The sheet this run was read from (e.g. `"M-201"`, `"M-301"`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_sheet: String,
    /// A drainage run's fall (inches per foot, toward its outlet), as the code or the drawings
    /// set it; 0: level (pressure piping, ducts, conduit). Its bottoms are its invert at its
    /// upstream end.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub slope_in_per_ft: f64,
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

fn is_false(v: &bool) -> bool {
    !*v
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
    /// Where its height was read (the tag and the sheet or view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
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

/// A mounting height a sheet's notes give a kind of item: `RECEPTACLES 1'-4" CENTERLINE` under a
/// general note's `DEVICE MOUNTED HEIGHTS`, `WATER CLOSETS (TO TOP OF SEAT): STANDARD = 15" TO 16".
/// ADA = 17" TO 19"` on an accessibility sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MountNote {
    /// The sheet it is on.
    pub sheet: String,
    /// What it is for, as written (`LIGHT SWITCH`, `WATER CLOSETS`).
    pub subject: String,
    /// Its height above the floor, inches: the one it gives (`STANDARD =`), else none.
    pub inches: Option<f64>,
    /// The height it gives an accessible room's (`ADA =`), inches.
    pub ada_inches: Option<f64>,
    /// What the height is measured to on the item: `centre`, `top` or `bottom`.
    pub datum: String,
    /// It says the item is at the ceiling (`STANDARD = CEILING HEIGHT`).
    pub ceiling: bool,
    /// The words read.
    pub text: String,
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

/// A length in inches to the eighth, as [`inches`] prints it.
fn eighths(inch: f64) -> f64 {
    (inch * 8.0).round() / 8.0
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
        let mut put = |section: &'static str, key: String, text: String, path: Option<PartPath>| {
            let approved = self.approved.iter().any(|a| a.eq_ignore_ascii_case(&key));
            let guess = self.guess.iter().find(|(k, _)| k.eq_ignore_ascii_case(&key)).map(|(_, why)| why.clone());
            out.push(Described { section, key, text, approved, guess, path });
        };
        let named = |at: &'static [&'static str], field: &'static str, id: &str| Some(PartPath::Named { at, field, id: id.to_string() });
        let index = |at: &'static [&'static str], i: usize| Some(PartPath::Items { at, pick: Pick::Index(i) });
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
                named(&["levels"], "name", &l.name),
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
            put("layers", format!("layers/{}", r.layer), format!("{} = {what}", r.layer), named(&["layers"], "layer", &r.layer));
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
                if !a.source.is_empty() {
                    let _ = write!(text, "; from {}", a.source);
                }
            }
            put("wall_types", format!("wall_types/{}", t.name), text, named(&["wall_types"], "name", &t.name));
        }
        if !self.walls.is_empty() {
            let guessed = self.walls.iter().filter(|w| w.status() == MarkStatus::Guess).count();
            let drawing = self.walls.len() - guessed;
            put(
                "walls",
                "walls".into(),
                format!("{} walls: {drawing} typed from their tags on the plans (drawing), {guessed} guessed (no tag)", self.walls.len()),
                Some(PartPath::Value { at: &["walls"], removable: true }),
            );
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
                // Rooms merge by name: one without a name can't be picked out.
                if r.name.trim().is_empty() { None } else { named(&["rooms"], "name", &r.name) },
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
            put(
                "families",
                format!("families/{}", f.name),
                format!("{}: {kind} from blocks {}, {width}, {heights}", f.name, f.block),
                named(&["families"], "name", &f.name),
            );
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
            put(
                "stairs",
                format!("stairs/{}", st.name),
                format!("{}: {what}, {} wide{state}", st.name, inches(st.width_in)),
                named(&["stairs"], "name", &st.name),
            );
        }
        for (i, f) in self.exterior.facades.iter().enumerate() {
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
                index(&["exterior", "facades"], i),
            );
        }
        for (i, r) in self.exterior.roofs.iter().enumerate() {
            let slope = r.slope.map_or("flat".to_string(), |s| format!("sloping {}", pitch_text(s.rise_per_ft)));
            let slope =
                if r.slope.is_some_and(|s| ValueStatus::is_default(&s.direction_status)) { format!("{slope} (direction assumed)") } else { slope };
            let assumed = if ValueStatus::is_default(&r.parapet_status) { " (assumed)" } else { "" };
            let parapet = r.parapet_ft.map_or(String::new(), |p| format!(", {} parapet{assumed}", feet(p)));
            let asm = r.assembly.as_ref().map_or(String::new(), |a| {
                let words: Vec<&str> = a.layers.iter().map(|l| l.material.as_str()).collect();
                format!("; {} top down: {}", if a.tag.is_empty() { "built" } else { a.tag.as_str() }, words.join(", "))
            });
            put(
                "exterior",
                format!("roofs/{}", r.name),
                format!("{}: roof on {}, {slope}{parapet}{asm}", r.name, r.level),
                index(&["exterior", "roofs"], i),
            );
        }
        for (i, c) in self.exterior.crickets.iter().enumerate() {
            put(
                "exterior",
                format!("crickets/{}", i + 1),
                format!(
                    "Cricket {} on {}, {} high{}",
                    i + 1,
                    c.level,
                    feet(c.height_ft),
                    if ValueStatus::is_default(&c.height_status) { " (assumed)" } else { "" }
                ),
                index(&["exterior", "crickets"], i),
            );
        }
        for (i, c) in self.rcp.ceilings.iter().enumerate() {
            let what = match c.grid {
                Some(g) => format!("{}×{} grid", inches(g.spacing_in[0]).trim_end_matches('"'), inches(g.spacing_in[1])),
                None => "gypsum".into(),
            };
            let room = c.room.as_ref().map_or(String::new(), |r| format!(" ({r})"));
            let what = c.ceiling_type.as_ref().map_or(what.clone(), |t| format!("type {t}, {what}"));
            let guess = c.guess.as_ref().map_or(String::new(), |g| format!(" (guess: {g})"));
            put(
                "rcp",
                format!("ceilings/{}", i + 1),
                format!("Ceiling {}{room} on {}: {what} at {}{guess}", i + 1, c.level, feet(c.height_ft)),
                index(&["rcp", "ceilings"], i),
            );
        }
        for (i, sf) in self.rcp.soffits.iter().enumerate() {
            let what = sf.soffit_type.as_ref().map_or(String::new(), |t| format!(", type {t}"));
            let guess = sf.guess.as_ref().map_or(String::new(), |g| format!(" (guess: {g})"));
            put(
                "rcp",
                format!("soffits/{}", i + 1),
                format!("Soffit {} on {} at {}{what}{guess}", i + 1, sf.level, feet(sf.height_ft)),
                index(&["rcp", "soffits"], i),
            );
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
            put("ceiling_types", format!("ceiling_types/{}", c.name), text, named(&["ceiling_types"], "name", &c.name));
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
                named(&["soffit_types"], "name", &t.name),
            );
        }
        if !self.rcp.items.is_empty() {
            let n = |k: ItemKind| self.rcp.items.iter().filter(|x| x.kind == k).count();
            put(
                "rcp",
                "ceiling_items".into(),
                format!("In the ceilings: {} light fixtures, {} diffusers", n(ItemKind::Fixture), n(ItemKind::Diffuser)),
                Some(PartPath::Value { at: &["rcp", "items"], removable: true }),
            );
        }
        for (kind, label, id) in [(FamilyKind::Door, "Door", "door"), (FamilyKind::Window, "Window", "window")] {
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
            put(
                "schedule",
                format!("schedule/{id}s"),
                format!("{label} schedule: {}{more}", list.join(", ")),
                Some(PartPath::Items { at: &["schedule", "rows"], pick: Pick::Field("kind", id.into()) }),
            );
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
                put(
                    "columns",
                    format!("columns/{l}"),
                    format!("{n} columns on {l} at the grid intersections"),
                    Some(PartPath::Items { at: &["columns"], pick: Pick::Field("level", l.to_string()) }),
                );
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
                named(&["mep", "systems"], "name", &sy.name),
            );
        }
        for (i, e) in self.mep.equipment.iter().enumerate() {
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
                index(&["mep", "equipment"], i),
            );
        }
        for (i, e) in self.exterior.equipment.iter().enumerate() {
            put(
                "exterior",
                format!("equipment/{}", e.name),
                format!(
                    "{}: rooftop equipment on {}, {} high{}",
                    e.name,
                    e.level,
                    feet(e.height_ft),
                    if ValueStatus::is_default(&e.height_status) { " (assumed)" } else { "" }
                ),
                index(&["exterior", "equipment"], i),
            );
        }
        let mats = materials_text(&self.materials);
        if !mats.is_empty() {
            put("materials", "materials".into(), format!("Materials: {mats}"), Some(PartPath::Value { at: &["materials"], removable: true }));
        }
        if let Some(site) = &self.site {
            put(
                "site",
                "site".into(),
                format!("Ground at {}, {} round the building", feet(site.elevation_ft), feet(site.margin_ft)),
                Some(PartPath::Value { at: &["site"], removable: true }),
            );
        }
        let o = &self.openings;
        put(
            "openings",
            "openings".into(),
            format!("Openings in walls: doors {} high, windows {} to {}", feet(o.door_head_ft), feet(o.window_sill_ft), feet(o.window_head_ft)),
            Some(PartPath::Value { at: &["openings"], removable: false }),
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
            stack: false,
            datums: Vec::new(),
            sheets: Vec::new(),
            tags: Vec::new(),
            estimate: serde_json::Value::Null,
            wholejob: serde_json::Value::Null,
            bim_view: serde_json::Value::Null,
            progress: serde_json::Value::Null,
            sequence: serde_json::Value::Null,
            unit_types: serde_json::Value::Null,
            unit_instances: serde_json::Value::Null,
            layers,
            wall_types,
            rooms: Vec::new(),
            families: Vec::new(),
            materials: Surfaces::default(),
            site: None,
            stairs: Vec::new(),
            elevators: Vec::new(),
            circulation: Vec::new(),
            shells: BTreeMap::new(),
            exterior: Exterior::default(),
            rcp: Rcp::default(),
            mep: Mep::default(),
            concrete: crate::concrete::Concrete::default(),
            trades: serde_json::Value::Null,
            columns: Vec::new(),
            schedule: Schedule::default(),
            walls: Vec::new(),
            rfis: Vec::new(),
            procurement: None,
            prints: None,
            mount_notes: Vec::new(),
            ceiling_types: Vec::new(),
            soffit_types: Vec::new(),
            devices: r.devices.clone(),
            device_sheets: r.device_sheets.clone(),
            openings: r.openings.clone(),
            approved: Vec::new(),
            guess: BTreeMap::new(),
            sources: BTreeMap::new(),
            print_baseline: Vec::new(),
            codebook: serde_json::Value::Null,
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

    /// Why a stair can't be built as measured, if it can't: no risers, or risers outside what
    /// any code allows (4" to 7¾", to the eighth printed) — a misread flight, or the wrong
    /// levels. A ramp is always buildable here (its slope is a warning).
    pub fn stair_unbuildable(&self, st: &Stair) -> Option<String> {
        if st.ramp {
            return None;
        }
        let Some((n, riser, _, _)) = self.stair_measures(st) else {
            return Some("its levels aren't known".into());
        };
        let shown = eighths(riser);
        if n == 0 || !shown.is_finite() {
            Some("no risers".into())
        } else if shown > crate::stairs::RISER_MAX_IN {
            Some(format!("{n} risers of {}: over 7¾\" (IRC R311.7.5.1), the most any stair rises", inches(riser)))
        } else if shown < crate::stairs::RISER_MIN_IN {
            Some(format!("{n} risers of {}: under the 4\" minimum (IBC 1011.5.2)", inches(riser)))
        } else {
            None
        }
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

    /// A family whose name matches a schedule row's TYPE (case-insensitive) and kind.
    pub fn family_by_type(&self, kind: FamilyKind, type_name: &str) -> Option<&Family> {
        self.families.iter().find(|f| f.kind == kind && f.name.eq_ignore_ascii_case(type_name.trim()))
    }

    pub fn level(&self, name: &str) -> Option<&Level> {
        self.level_matching(name).and_then(|i| self.levels.get(i))
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
        for (i, p) in self.exterior.parapets.iter().enumerate().take(MAX_ITEMS) {
            if !(fin(p.height_ft) && (0.0..=50.0).contains(&p.height_ft)) {
                out.push(Issue::error(format!("exterior.parapets[{i}].height_ft"), "the parapet must be from 0 to 50 feet"));
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
            let vertical = r.top_ft.is_some();
            let min_pts = if vertical { 1 } else { 2 };
            let btm_ok = if vertical { r.bottoms_ft.len() == 1 && r.points.len() == 1 } else { r.bottoms_ft.len() + 1 == r.points.len() };
            if r.points.len() < min_pts
                || !btm_ok
                || !r.points.iter().flatten().chain(r.bottoms_ft.iter()).chain(r.size_in.iter()).all(|v| fin(*v))
                || r.size_in[0].is_nan()
                || r.size_in[0] <= 0.0
                || r.top_ft.is_some_and(|t| !fin(t) || t <= r.bottoms_ft.first().copied().unwrap_or(0.0))
            {
                out.push(Issue::error(
                    at,
                    "a run needs 2 points or more (or 1 point with top_ft for a vertical riser), a size, and one bottom height per segment",
                ));
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
            // A system that isn't one of mep.systems: its runs (if any) are grouped apart from it.
            for (k, sy) in e.systems.iter().enumerate().take(MAX_ITEMS) {
                if !self.mep.systems.iter().any(|s| s.name == *sy) {
                    out.push(Issue::warning(
                        format!("{at}.systems[{k}]"),
                        format!("{} joins {sy}, which isn't a system in mep.systems: it won't be grouped with that system's runs", e.tag),
                    ));
                }
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
            if let Some(c) = st.code.as_deref().filter(|c| !["ibc", "irc"].iter().any(|k| c.trim().eq_ignore_ascii_case(k))) {
                out.push(Issue::error(
                    format!("{at}.code"),
                    format!(
                        "the stair code is \"ibc\" (commercial, the default) or \"irc\" (residential), not \"{}\"",
                        c.chars().take(40).collect::<String>()
                    ),
                ));
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
                        // Checked as printed (to the eighth): a tread of 10.994" is the 11" it reads.
                        let (riser, tread) = (eighths(riser), eighths(tread));
                        let irc = st.residential();
                        let (riser_max, tread_min) = if irc {
                            (crate::stairs::RISER_MAX_IN, crate::stairs::TREAD_MIN_IN)
                        } else {
                            (crate::stairs::RISER_MAX_COMMERCIAL_IN, crate::stairs::TREAD_MIN_COMMERCIAL_IN)
                        };
                        if riser > riser_max + 1e-9 {
                            let limit = if irc {
                                "the 7¾\" limit (IRC R311.7.5.1; 7\" under IBC 1011.5.2)"
                            } else {
                                "the 7\" limit (IBC 1011.5.2; 7¾\" under IRC R311.7.5.1, code \"irc\")"
                            };
                            out.push(Issue::warning(&at, format!("{n} risers of {}: above {limit} — add treads", inches(riser))));
                        } else if riser < crate::stairs::RISER_MIN_IN - 1e-9 {
                            out.push(Issue::warning(
                                &at,
                                format!("{n} risers of {}: under the 4\" minimum (IBC 1011.5.2) — fewer treads", inches(riser)),
                            ));
                        } else if riser < crate::stairs::RISER_SHALLOW_IN - 1e-9 {
                            out.push(Issue::warning(
                                &at,
                                format!(
                                    "{n} risers of {}: lawful, but shallower than stairs are drawn (5\") — check the tread count read off the plan, or whether it is a ladder",
                                    inches(riser)
                                ),
                            ));
                        }
                        if tread.is_finite() && tread < tread_min - 1e-9 {
                            let limit = if irc {
                                "the 10\" minimum (IRC R311.7.5.2; 11\" under IBC 1011.5.2)"
                            } else {
                                "the 11\" minimum (IBC 1011.5.2; 10\" under IRC R311.7.5.2, code \"irc\")"
                            };
                            out.push(Issue::warning(&at, format!("treads of {}: under {limit}", inches(tread))));
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

/// Merge `patch` into `base` (both spec JSON): objects merge key by key, `null` removes a key; a
/// list of named items (levels, wall, ceiling and soffit types, rooms, families, stairs,
/// elevators, MEP systems by `name`; layers by `layer`) merges item by item, an item with
/// `"remove": true` removing it; other lists are replaced.
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
                    "levels" | "wall_types" | "rooms" | "families" | "stairs" | "elevators" | "systems" | "ceiling_types" | "soffit_types" => {
                        Some("name")
                    }
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

    /// A spec whose MEP holds only panel schedules (read before anything is built) keeps them
    /// when it is saved and read back.
    #[test]
    fn mep_with_only_panel_schedules_survives_a_save() {
        let sp = BuildingSpec {
            mep: Mep {
                panel_schedules: vec![PanelSchedule {
                    panel: "P1".into(),
                    circuits: vec![PanelCircuit { number: "7".into(), description: "RADIO AMPLIFIER".into(), ..Default::default() }],
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!sp.mep.is_empty());
        let back = BuildingSpec::from_json(&sp.to_json()).unwrap_or_default();
        assert_eq!(back.mep.panel_schedules, sp.mep.panel_schedules);
        let labels = Mep { labels_printed: [("P1".to_string(), "PANEL P1".to_string())].into_iter().collect(), ..Default::default() };
        assert!(!labels.is_empty() && Mep::default().is_empty());
    }

    #[test]
    fn a_spec_holding_only_schedules_keeps_them_when_saved() {
        let mut sp = BuildingSpec::default();
        sp.mep.fixture_schedule.push(FixtureType { tag: "Q1".into(), quantity: Some(4), ..FixtureType::default() });
        sp.mep.panel_schedules.push(PanelSchedule { panel: "P1".into(), ..PanelSchedule::default() });
        let back = BuildingSpec::from_json(&sp.to_json()).unwrap_or_default();
        assert_eq!(back.mep.fixture_schedule.len(), 1, "the fixture schedule is MEP data too");
        assert_eq!(back.mep.fixture_schedule[0].quantity, Some(4));
        assert_eq!(back.mep.panel_schedules.len(), 1);
    }

    #[test]
    fn a_corridors_edges_run_round_its_floor_not_between_its_rectangles() {
        // An L: a 100 × 10 run and a 10 × 40 leg on its east end, as two rectangles meeting at
        // y = 10, and the run split again at x = 50.
        let c = Circulation {
            rects: vec![[[0.0, 0.0], [50.0, 10.0]], [[50.0, 0.0], [100.0, 10.0]], [[90.0, 10.0], [100.0, 50.0]]],
            ..Circulation::default()
        };
        let edges = c.edges();
        let perimeter: f64 = edges.iter().map(|(a, b)| (b[0] - a[0]).abs() + (b[1] - a[1]).abs()).sum();
        assert!((perimeter - 2.0 * (100.0 + 50.0)).abs() < 1e-9, "{perimeter}: {edges:?}");
        // The split and the leg's foot are inside: no edge on x = 50 or along y = 10 under the leg.
        assert!(!edges.iter().any(|(a, b)| a[0] == 50.0 && b[0] == 50.0), "{edges:?}");
        assert!(edges.contains(&([0.0, 10.0], [90.0, 10.0])), "{edges:?}");
        assert!(c.holds([95.0, 45.0]) && !c.holds([50.0, 45.0]));
        // Its outline: the L's six corners, round its area.
        let o = c.outline();
        let area = o.iter().zip(o.iter().cycle().skip(1)).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum::<f64>().abs() / 2.0;
        assert_eq!((o.len(), area), (6, c.area()), "{o:?}");
        assert_eq!(c.area(), 100.0 * 10.0 + 10.0 * 40.0);
    }

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
        s.walls.push(WallMark {
            level: level.clone(),
            a: [0.0, 0.0],
            b: [120.0, 0.0],
            wall_type: "1b".into(),
            height_ft: Some(9.5),
            guess: None,
            source: String::new(),
            status: None,
            height_source: String::new(),
        });
        assert!(s.check().iter().all(|i| !i.error), "{:?}", s.check());
        assert_eq!(BuildingSpec::from_json(&s.to_json()), Ok(s.clone()));
        let d = s.describe();
        let t = d.iter().find(|x| x.key == "wall_types/1B").map(|x| x.text.clone()).unwrap_or_default();
        assert!(t.contains("6\" above the ceiling") && t.contains("1 HR") && t.contains("STC 45"), "{t}");
        // A thickness the layers don't make, a mark on no type, a bad layer: said.
        s.wall_types[0].thickness_in = 6.0;
        s.walls.push(WallMark {
            level,
            a: [0.0, 0.0],
            b: [0.0, 0.0],
            wall_type: "nope".into(),
            height_ft: Some(f64::NAN),
            guess: None,
            source: String::new(),
            status: None,
            height_source: String::new(),
        });
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
            parapet_status: None,
            parapet_from: None,
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

    /// A spec with two of every kind of part `describe()` gives.
    fn every_part() -> serde_json::Value {
        let sq = |x: f64| serde_json::json!([[x, 0], [x + 10.0, 0], [x + 10.0, 10], [x, 10]]);
        serde_json::json!({
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}, {"name": "L2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9}],
            "layers": [{"layer": "A-WALL", "role": "wall"}, {"layer": "A-DOOR", "role": "door"}],
            "wall_types": [{"name": "WT1", "thickness_in": 5}, {"name": "WT2", "thickness_in": 8}],
            "walls": [{"level": "L1", "a": [0, 0], "b": [10, 0], "wall_type": "WT1"}],
            "rooms": [{"name": "Office"}, {"name": "", "number": "102"}],
            "families": [{"name": "D1", "kind": "door", "block": "DOOR"}, {"name": "W1", "kind": "window", "block": "WIN"}],
            "stairs": [{"name": "S1", "level": "L1", "runs": [{"from": [0, 0], "to": [0, 10]}]}],
            "exterior": {
                "facades": [{"side": "north"}, {"side": "south"}],
                "roofs": [{"name": "R1", "level": "L2", "outline": sq(0.0)}, {"name": "R2", "level": "L2", "outline": sq(20.0)}],
                "crickets": [{"level": "L2", "outline": sq(0.0), "height_ft": 1}],
                "equipment": [{"name": "RTU-1", "level": "L2", "outline": sq(0.0), "height_ft": 4}]
            },
            "rcp": {
                "ceilings": [{"level": "L1", "outline": sq(0.0), "height_ft": 9}],
                "soffits": [{"level": "L1", "outline": sq(0.0), "height_ft": 8}],
                "items": [{"kind": "fixture", "block": "LT", "level": "L1", "at": [1, 1], "size_in": [24, 24]}]
            },
            "ceiling_types": [{"name": "C1"}, {"name": "C2", "kind": "grid"}],
            "soffit_types": [{"name": "SF1"}, {"name": "SF2", "hangs_from": "structure"}],
            "columns": [
                {"level": "L1", "grid": "A-1", "at": [0, 0], "size_in": [12, 12]},
                {"level": "L2", "grid": "A-1", "at": [0, 0], "size_in": [12, 12]},
                {"level": "L1", "grid": "B-1", "at": [20, 0], "size_in": [12, 12]}
            ],
            "schedule": {"rows": [{"kind": "door", "mark": "101"}, {"kind": "window", "mark": "A"}, {"kind": "door", "mark": "102"}]},
            "mep": {
                "systems": [{"name": "SA", "layer": "M-SUPP", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9}],
                "equipment": [{"tag": "AHU-1", "kind": "air_handler", "level": "L1", "height_ft": 4}]
            },
            "materials": {"wall": "Brick"},
            "site": {"elevation_ft": 0, "margin_ft": 20}
        })
    }

    #[test]
    fn every_described_part_has_its_own_place_in_the_json() {
        let s: BuildingSpec = serde_json::from_value(every_part()).unwrap();
        let v = serde_json::to_value(&s).unwrap();
        let d = s.describe();
        let mut sections: Vec<&str> = d.iter().map(|p| p.section).collect();
        sections.dedup();
        assert_eq!(
            sections,
            [
                "levels",
                "layers",
                "wall_types",
                "walls",
                "rooms",
                "families",
                "stairs",
                "exterior",
                "rcp",
                "ceiling_types",
                "soffit_types",
                "rcp",
                "schedule",
                "columns",
                "mep",
                "exterior",
                "materials",
                "site",
                "openings"
            ]
        );
        let at = |p: &str| v.pointer(p).cloned().unwrap();
        let expect = [
            ("levels/L1", at("/levels/0")),
            ("layers/A-DOOR", at("/layers/1")),
            ("wall_types/WT2", at("/wall_types/1")),
            ("walls", at("/walls")),
            ("rooms/Office", at("/rooms/0")),
            ("families/W1", at("/families/1")),
            ("stairs/S1", at("/stairs/0")),
            ("facades/South", at("/exterior/facades/1")),
            ("roofs/R2", at("/exterior/roofs/1")),
            ("crickets/1", at("/exterior/crickets/0")),
            ("ceilings/1", at("/rcp/ceilings/0")),
            ("soffits/1", at("/rcp/soffits/0")),
            ("ceiling_types/C2", at("/ceiling_types/1")),
            ("soffit_types/SF1", at("/soffit_types/0")),
            ("ceiling_items", at("/rcp/items")),
            ("schedule/doors", serde_json::json!([at("/schedule/rows/0"), at("/schedule/rows/2")])),
            ("schedule/windows", serde_json::json!([at("/schedule/rows/1")])),
            ("columns/L1", serde_json::json!([at("/columns/0"), at("/columns/2")])),
            ("columns/L2", serde_json::json!([at("/columns/1")])),
            ("systems/SA", at("/mep/systems/0")),
            ("equipment/AHU-1", at("/mep/equipment/0")),
            ("equipment/RTU-1", at("/exterior/equipment/0")),
            ("materials", at("/materials")),
            ("site", at("/site")),
            ("openings", at("/openings")),
        ];
        for (key, want) in &expect {
            let p = d.iter().find(|p| p.key == *key).unwrap_or_else(|| panic!("{key} not described"));
            assert_eq!(&p.path.as_ref().unwrap().get(&v), want, "{key}");
        }
        for p in &d {
            // The room with only a number can't be picked out by name: no path.
            let Some(path) = &p.path else {
                assert_eq!(p.key, "rooms/102");
                continue;
            };
            assert!(!path.get(&v).is_null(), "{}", p.key);
            // Put back unchanged: the spec is identical.
            let mut same = v.clone();
            merge(&mut same, &path.patch(&v, Some(path.get(&v))).unwrap());
            let same: BuildingSpec = serde_json::from_value(same).unwrap();
            assert_eq!(same, s, "{}", p.key);
            // Removed: that part, and only it, is gone.
            let Some(remove) = path.patch(&v, None) else {
                assert_eq!(p.key, "openings");
                continue;
            };
            let mut gone = v.clone();
            merge(&mut gone, &remove);
            let gone: BuildingSpec = serde_json::from_value(gone).unwrap();
            let left: Vec<String> = gone.describe().into_iter().map(|x| x.key).collect();
            let want: Vec<String> = d.iter().map(|x| x.key.clone()).filter(|k| k != &p.key).collect();
            assert_eq!(left, want, "{}", p.key);
        }
    }
}
