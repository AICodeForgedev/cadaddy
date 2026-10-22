//! Concrete in the building spec: slabs, footings, grade beams, piers and pole bases, equipment
//! housekeeping pads, ductbank encasement and handhole bases, each an element with its
//! reinforcing and what is cast into it; the pad decision for each piece of floor- or
//! grade-mounted equipment (required, why, whose scope); and the pours, with the electrical items
//! that must be in or under each before concrete (the hold points).
//!
//! The quantities (volume, forms, rebar, embeds) and the coordination checks are the engine's
//! (`concrete.*` commands); this module only holds what the prints and the project say.

use serde::{Deserialize, Serialize};

/// Elements, pads, pours and rebar or embeds per element, at most (hostile specs).
pub const MAX_CONCRETE: usize = 10_000;

/// The concrete of the building.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Concrete {
    /// Slabs, footings, grade beams, piers, pads, encasement, handhole bases.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<ConcreteElement>,
    /// A pad decision per piece of floor- or grade-mounted equipment (by its tag).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pads: Vec<PadDecision>,
    /// The pours, with their dates and hold points.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pours: Vec<Pour>,
    /// The project's rules (pad margin and height, chamfer, waste, covers, unit costs…) when
    /// they differ from the defaults; see the engine's `concrete.rules`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<serde_json::Value>,
}

impl Concrete {
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty() && self.pads.is_empty() && self.pours.is_empty() && self.rules.is_none()
    }
}

/// One piece of concrete.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConcreteElement {
    /// Its mark or a made-up id (`F-1`, `SOG-L1`, `PB-3`).
    pub id: String,
    /// `slab`, `footing`, `grade_beam`, `pier`, `pole_base`, `pad`, `encasement`, `handhole_base`.
    pub kind: String,
    pub level: String,
    /// Plan corners (sheet units) of a slab, footing, pad or handhole base, or the box round a
    /// pier or pole base (`round`).
    pub min: [f64; 2],
    pub max: [f64; 2],
    /// A grade beam, continuous footing or ductbank: its centreline (sheet units); its width is
    /// `width_in`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<[f64; 2]>,
    pub width_in: f64,
    /// Thickness (a slab), depth (a footing, beam, pier, encasement) or height (a pad), inches.
    pub depth_in: f64,
    /// Top of the element above the level's floor, feet (a footing: negative).
    pub top_ft: f64,
    /// Round in plan (a drilled pier, a pole base): the diameter is the box's smaller side.
    pub round: bool,
    /// A pad's top edge chamfer, inches (0: none).
    pub chamfer_in: f64,
    /// A slab's thickened edge: width and total depth, inches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thickened_edge_in: Option<[f64; 2]>,
    /// A slab's depressions (recesses): plan corners and depth.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depressions: Vec<Depression>,
    /// Placed against earth (footings, piers, encasement): no forms on the sides.
    pub earth_formed: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rebar: Vec<Rebar>,
    /// Welded wire reinforcement, as printed (`6x6-W2.9xW2.9`), over the whole top.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mesh: String,
    /// What is cast in: the Ufer electrode, conduits and stub-ups, sleeves, anchor bolts, embeds.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub embeds: Vec<Embed>,
    /// Whose scope: `electrical`, `concrete`, `gc`, `utility`, `mechanical`… (empty: not given).
    pub owner: String,
    /// Where it came from: a sheet and detail, a spec section.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// The pour it is placed in (a `Pour::id`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pour: String,
    /// A pad: the equipment it carries (its tag).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub equipment: String,
}

impl Default for ConcreteElement {
    fn default() -> Self {
        ConcreteElement {
            id: String::new(),
            kind: "slab".into(),
            level: String::new(),
            min: [0.0; 2],
            max: [0.0; 2],
            path: Vec::new(),
            width_in: 0.0,
            depth_in: 0.0,
            top_ft: 0.0,
            round: false,
            chamfer_in: 0.0,
            thickened_edge_in: None,
            depressions: Vec::new(),
            earth_formed: false,
            rebar: Vec::new(),
            mesh: String::new(),
            embeds: Vec::new(),
            owner: String::new(),
            source: String::new(),
            pour: String::new(),
            equipment: String::new(),
        }
    }
}

/// A recess in a slab's top (a shower, a mat well, an equipment pit).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Depression {
    pub min: [f64; 2],
    pub max: [f64; 2],
    pub depth_in: f64,
}

/// Reinforcing as drawn in the details.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rebar {
    /// Bar size: `#4`, `#5`.
    pub bar: String,
    /// `x`, `y` or `each_way` (a mat; bars across the plan box), `long` (along a path or a pier:
    /// `count` bars), `ties` (stirrups or ties round the section at `spacing_in`), `dowels`
    /// (`count` bars of `length_in`).
    pub direction: String,
    /// Spacing, inches (mats and ties).
    pub spacing_in: f64,
    /// Bars (long bars, dowels).
    pub count: u32,
    /// Dowel length, inches.
    pub length_in: f64,
    /// Clear cover, inches (0: the rules' cover for the element).
    pub cover_in: f64,
    /// Lap splice, inches (0: the rules' laps, by bar).
    pub lap_in: f64,
    /// Mats: top and bottom = 2.
    pub layers: u32,
}

impl Default for Rebar {
    fn default() -> Self {
        Rebar { bar: "#4".into(), direction: "each_way".into(), spacing_in: 12.0, count: 0, length_in: 0.0, cover_in: 0.0, lap_in: 0.0, layers: 1 }
    }
}

/// Something cast into the concrete.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Embed {
    /// `ufer` (the concrete-encased electrode), `conduit` (under-slab or through a footing),
    /// `stub_up` (conduit up through a pad or slab), `sleeve`, `anchor_bolt`, `embed_plate`,
    /// `ground_rod`, `bond` (a bond to the rebar).
    pub kind: String,
    /// Size as printed: `#4 bar`, `#4 Cu`, `2"`, `3/4" x 18"`.
    pub size: String,
    pub count: u32,
    /// Plan point (sheet units), or none: the element's centre.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    /// A run (a Ufer conductor, an under-slab conduit): its length, feet (0: a point item).
    pub length_ft: f64,
    /// Diameter, inches (conduit OD, sleeve, bolt) when the size doesn't give it.
    pub diameter_in: f64,
    /// Depth of its centre below the element's top, inches (0: mid-depth).
    pub depth_in: f64,
    /// An anchor bolt's embedment and projection, inches.
    pub embed_in: f64,
    pub projection_in: f64,
    /// Whose scope (empty: the element's owner).
    pub owner: String,
    /// The sheet or detail that calls for it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
}

impl Default for Embed {
    fn default() -> Self {
        Embed {
            kind: "conduit".into(),
            size: String::new(),
            count: 1,
            at: None,
            length_ft: 0.0,
            diameter_in: 0.0,
            depth_in: 0.0,
            embed_in: 0.0,
            projection_in: 0.0,
            owner: String::new(),
            source: String::new(),
        }
    }
}

/// Whether a piece of equipment sits on a housekeeping pad, why, and whose the pad is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PadDecision {
    /// The equipment's tag.
    pub equipment: String,
    /// `Some(true)`: a pad; `Some(false)`: none (on the slab, on a curb, on a stand); none:
    /// not decided yet.
    pub required: Option<bool>,
    /// Why: a spec section, a detail, a utility or manufacturer requirement.
    pub basis: String,
    /// Whose scope: `electrical`, `concrete`, `gc`, `utility`, `mechanical`.
    pub owner: String,
    /// Overrides of the rules for this pad: the margin beyond the equipment on each side, the
    /// height and the chamfer, inches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chamfer_in: Option<f64>,
    /// A size fixed by a detail (width × depth, inches); none: sized from the footprint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_in: Option<[f64; 2]>,
    /// The side the equipment is worked from (`n`, `s`, `e`, `w`), for the clearance check;
    /// none: either long side.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub front: String,
    /// The pour it is placed in.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pour: String,
    /// Reinforcing and embeds of the pad (beyond the anchor bolts and stub-ups found from the
    /// equipment and its feeders).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rebar: Vec<Rebar>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub embeds: Vec<Embed>,
}

/// A concrete placement.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pour {
    /// `P-1`, `FDN-A`.
    pub id: String,
    /// What it is (`Footings grid A–C`).
    pub name: String,
    /// The scheduled date, `YYYY-MM-DD` (from the 4D schedule or entered).
    pub date: String,
    /// Done: concrete is in (no more embeds can go in).
    pub placed: bool,
    /// Hold points confirmed in place before the pour.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub signoffs: Vec<Signoff>,
}

/// A hold point confirmed: the item (its key in the pour's checklist), by whom and when.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Signoff {
    pub item: String,
    pub by: String,
    /// `YYYY-MM-DD`.
    pub on: String,
}
