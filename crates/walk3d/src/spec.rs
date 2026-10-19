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
//!   centreline;
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
    if num > 0.0 { format!("{whole} {num}/{den}\"") } else { format!("{whole}\"") }
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
                Role::Stair => "stairs (not built yet)".into(),
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
            put("wall_types", format!("wall_types/{}", t.name), format!("{}: {} thick, {height}, {from}", t.name, inches(t.thickness_in)));
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
                    wall_types.push(WallType { name: name.clone(), height_ft: Some(h), thickness_in: 6.0, from: WallFrom::Pairs });
                }
                role.wall_type = Some(name);
            }
        }
        BuildingSpec {
            version: SPEC_VERSION,
            units_per_foot: r.units_per_foot,
            levels: r.levels.clone(),
            layers,
            wall_types,
            rooms: Vec::new(),
            families: Vec::new(),
            materials: Surfaces::default(),
            site: None,
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
                    "levels" | "wall_types" | "rooms" | "families" => Some("name"),
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
        s.wall_types.push(WallType { name: "W".into(), height_ft: None, thickness_in: 4.875, from: WallFrom::Pairs });
        s.layers[0].wall_type = Some("w".into());
        s.approved.push("LEVELS/level 1".into());
        let d = s.describe();
        assert_eq!(d[0].text, "Level 1 = sheet A112 at 10'-0\"; 10'-0\" floor to floor, 8'-0\" ceilings");
        assert!(d[0].approved);
        assert_eq!(d[1].text, "*WALL* = full height walls 4 7/8\" thick (line pairs)");
        assert!(!d[1].approved);
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
}
