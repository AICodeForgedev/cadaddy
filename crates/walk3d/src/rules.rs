//! What the 2D drawing means in 3D: levels and layer rules, kept beside the drawing as
//! `<name>.walk.json` (see [`Rules::sidecar_path`]).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    /// Drawing units per foot (12 for an inch drawing).
    #[serde(default = "twelve")]
    pub units_per_foot: f64,
    pub levels: Vec<Level>,
    pub layers: Vec<LayerRule>,
    #[serde(default)]
    pub devices: DeviceRules,
    /// Drawings that only add devices, relative to the rules file (one devices drawing for a
    /// whole building): each device's level attribute (`devices.level_attr`) picks its level, and
    /// that level's base point places it.
    #[serde(default)]
    pub device_sheets: Vec<String>,
    /// Doors, windows and cased openings found in the walls (see [`Openings`]).
    #[serde(default)]
    pub openings: Openings,
    /// Stairs and ramps between the levels (the building spec's `stairs`, heights worked out).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stairs: Vec<crate::stairs::StairRule>,
}

/// How the gaps between two pieces of one wall are filled: a door (a swing arc at a jamb) gets a
/// header above its head and its leaf standing open; a gap in an outside wall is a window (sill and
/// head); any other gap up to 8' is a cased opening with a header. Heights are above the floor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Openings {
    pub enabled: bool,
    pub door_head_ft: f64,
    pub window_sill_ft: f64,
    pub window_head_ft: f64,
    /// Gaps narrower than this (feet) are not openings.
    pub min_ft: f64,
    /// Nor wider than this.
    pub max_ft: f64,
}

impl Default for Openings {
    fn default() -> Self {
        Openings { enabled: true, door_head_ft: 7.0, window_sill_ft: 2.0, window_head_ft: 7.0, min_ft: 1.5, max_ft: 16.0 }
    }
}

/// One floor: the part of the sheet its plan occupies and where that plan sits in the building.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub name: String,
    /// Finished floor elevation, feet.
    pub elevation_ft: f64,
    /// Floor to floor (walls reach this high), feet.
    pub height_ft: f64,
    /// Ceiling height above the floor, feet (ceiling-mounted devices hang here).
    #[serde(default = "eight")]
    pub ceiling_ft: f64,
    /// Sheet rectangle `[x0, y0, x1, y1]` holding this level's plan; none = the whole drawing.
    #[serde(default)]
    pub region: Option<[f64; 4]>,
    /// Sheet point that maps to the building origin; none = the lower-left corner of this level's
    /// walls, which lines up stacked plans drawn to the same outline.
    #[serde(default)]
    pub base: Option<[f64; 2]>,
    /// A block attribute value (`devices.level_attr`, e.g. FLOOR = "3") that puts a device on
    /// this level wherever it is drawn.
    #[serde(default)]
    pub attr: Option<String>,
    /// The drawing holding this level's plan, relative to the rules file (one sheet per floor, as
    /// A111…A114 are); none = the drawing the rules were opened with.
    #[serde(default)]
    pub sheet: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerRule {
    /// Layer name; a trailing `*` matches any ending. Case does not matter.
    pub layer: String,
    #[serde(flatten)]
    pub kind: LayerKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayerKind {
    /// Lines and polylines stand up as wall faces from the floor to `height_ft` (none = floor to
    /// floor).
    Wall {
        #[serde(default)]
        height_ft: Option<f64>,
        /// Drop curved runs (door swings flattened to short chords share the wall pen on
        /// PDF-converted plans): segments that turn gently at an end, not at a square corner.
        #[serde(default)]
        skip_curves: bool,
        /// Keep only faces with a partner face a wall's thickness away (3½" to 1'-7"), and only
        /// where they overlap it. Plans draw both sides of every wall; dimension and leader
        /// lines, casework edges, door leaves and the lines across door openings have no
        /// partner, so they drop out (and doorways open up).
        #[serde(default)]
        paired_only: bool,
    },
    /// Block references are devices, placed by [`DeviceRules`].
    Device,
    /// Lines and polylines are runs (conduit, cable) drawn as lines `height_ft` above the floor.
    Run { height_ft: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceRules {
    /// Attribute naming the device type, looked up in `by_type`.
    #[serde(default = "type_tag")]
    pub type_attr: String,
    /// Attribute naming the level (matched against [`Level::attr`]).
    #[serde(default = "floor_tag")]
    pub level_attr: String,
    /// Attribute giving the mounting height in inches (overrides the type's height).
    #[serde(default = "aff_tag")]
    pub aff_attr: String,
    #[serde(default)]
    pub default: Mount,
    #[serde(default)]
    pub by_type: BTreeMap<String, Mount>,
}

impl Default for DeviceRules {
    fn default() -> Self {
        DeviceRules { type_attr: type_tag(), level_attr: floor_tag(), aff_attr: aff_tag(), default: Mount::default(), by_type: BTreeMap::new() }
    }
}

/// Where a device hangs: on the ceiling, or on the nearest wall at a height above the floor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mount {
    #[serde(default)]
    pub ceiling: bool,
    /// Centre height above the floor, feet (wall-mounted).
    #[serde(default = "aff18")]
    pub aff_ft: f64,
    /// Box size `[width, depth, height]`, feet.
    #[serde(default = "plate")]
    pub size_ft: [f64; 3],
}

impl Default for Mount {
    fn default() -> Self {
        Mount { ceiling: true, aff_ft: aff18(), size_ft: [1.0, 1.0, 0.25] }
    }
}

fn twelve() -> f64 {
    12.0
}
fn eight() -> f64 {
    8.0
}
fn aff18() -> f64 {
    1.5
}
fn plate() -> [f64; 3] {
    [0.3, 0.15, 0.45]
}
fn type_tag() -> String {
    "TYPE".into()
}
fn floor_tag() -> String {
    "FLOOR".into()
}
fn aff_tag() -> String {
    "AFF".into()
}

impl Rules {
    /// One level at elevation 0; layers named like walls become walls, `E-DEV*` blocks devices.
    pub fn fallback() -> Rules {
        Rules {
            units_per_foot: 12.0,
            levels: vec![Level {
                name: "Level 1".into(),
                elevation_ft: 0.0,
                height_ft: 10.0,
                ceiling_ft: 8.0,
                region: None,
                base: None,
                attr: None,
                sheet: None,
            }],
            layers: vec![
                LayerRule { layer: "*WALL*".into(), kind: LayerKind::Wall { height_ft: None, skip_curves: false, paired_only: false } },
                LayerRule { layer: "E-DEV*".into(), kind: LayerKind::Device },
            ],
            devices: DeviceRules::default(),
            device_sheets: Vec::new(),
            openings: Openings::default(),
            stairs: Vec::new(),
        }
    }

    /// `plans/E-4.dxf` → `plans/E-4.walk.json`.
    pub fn sidecar_path(drawing: &std::path::Path) -> std::path::PathBuf {
        drawing.with_extension("walk.json")
    }

    pub fn from_json(s: &str) -> Result<Rules, String> {
        serde_json::from_str(s).map_err(|e| format!("walk rules: {e}"))
    }

    /// The rule for a layer: the first one whose pattern matches.
    /// The distinct sheets in order: the levels' sheets, then the device sheets. A level's
    /// drawing is source `0` (the opened drawing) without a sheet, else `1 +` its sheet's position
    /// here.
    pub fn sheets(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for s in self.levels.iter().filter_map(|l| l.sheet.as_ref()).chain(self.device_sheets.iter()) {
            if !v.contains(s) {
                v.push(s.clone());
            }
        }
        v
    }

    /// Source indexes of the device sheets (see [`Rules::sheets`]).
    pub fn device_sources(&self) -> Vec<usize> {
        let all = self.sheets();
        self.device_sheets.iter().filter_map(|d| all.iter().position(|x| x == d)).map(|i| i + 1).collect()
    }

    /// Source index of a level's drawing (see [`Rules::sheets`]).
    pub fn level_source(&self, level: usize) -> usize {
        match self.levels.get(level).and_then(|l| l.sheet.as_ref()) {
            Some(s) => self.sheets().iter().position(|x| x == s).map_or(0, |i| i + 1),
            None => 0,
        }
    }

    pub fn layer(&self, name: &str) -> Option<&LayerKind> {
        self.layers.iter().find(|r| layer_match(&r.layer, name)).map(|r| &r.kind)
    }

    pub fn mount(&self, device_type: &str) -> &Mount {
        self.devices.by_type.get(device_type).unwrap_or(&self.devices.default)
    }
}

/// `*` anywhere matches any run of characters; case-insensitive.
pub(crate) fn layer_match(pattern: &str, name: &str) -> bool {
    let p = pattern.to_ascii_uppercase();
    let n = name.to_ascii_uppercase();
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == n;
    }
    let mut rest = n.as_str();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_patterns() {
        assert!(layer_match("A-WALL", "a-wall"));
        assert!(!layer_match("A-WALL", "A-WALL-EXT"));
        assert!(layer_match("A-WALL*", "A-WALL-EXT"));
        assert!(layer_match("*WALL*", "A-WALL-EXT"));
        assert!(layer_match("E-DEV*", "E-DEVC"));
        assert!(!layer_match("E-DEV*", "A-DEVC"));
    }

    #[test]
    fn json_round_trip() {
        let r = Rules::fallback();
        let s = serde_json::to_string(&r).unwrap_or_default();
        assert_eq!(Rules::from_json(&s), Ok(r));
        let small = r#"{"levels":[{"name":"L1","elevation_ft":0,"height_ft":10}],"layers":[{"layer":"A-WALL","kind":"wall"},{"layer":"E-PWR","kind":"run","height_ft":9}]}"#;
        let r = Rules::from_json(small).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.layer("a-wall"), Some(&LayerKind::Wall { height_ft: None, skip_curves: false, paired_only: false }));
        assert_eq!(r.layer("E-PWR"), Some(&LayerKind::Run { height_ft: 9.0 }));
        assert_eq!(r.units_per_foot, 12.0);
    }
}
