//! 2-D CAM toolpath generation for CNC machining.
//!
//! Given closed 2-D profiles (polylines of lines and arcs), this crate computes offset toolpaths
//! with tool-radius compensation (inside / outside / on the line), lead-in and lead-out moves,
//! holding tabs, depth passes and cut ordering, then emits G-code in several dialects and
//! CNC-clean polylines (arcs preserved, no blocks).
//!
//! The crate depends only on [`cadaddy_geom`] — entity extraction from a drawing happens in the
//! engine command that calls these functions.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod gcode;
pub mod profile;
pub mod toolpath;

use serde::{Deserialize, Serialize};

/// Errors from CAM operations.
#[derive(Debug, thiserror::Error)]
pub enum CamError {
    #[error("no closed profiles in selection")]
    NoProfiles,
    #[error("tool diameter must be positive, got {0}")]
    BadToolDiameter(f64),
    #[error("offset failed for profile {index}: {reason}")]
    OffsetFailed { index: usize, reason: String },
    #[error("G-code generation: {0}")]
    Gcode(String),
}

pub type Result<T> = std::result::Result<T, CamError>;

/// Which side of the profile the tool centre travels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OffsetSide {
    /// Tool centre inside the profile (pocket / hole finishing).
    Inside,
    /// Tool centre outside the profile (part outline).
    Outside,
    /// Tool centre on the profile line (engraving, no compensation).
    On,
}

/// How a lead-in or lead-out approaches or leaves the profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeadKind {
    /// A tangent arc (quarter-circle by default).
    Arc,
    /// A straight perpendicular approach.
    Line,
    /// No lead move.
    None,
}

/// Lead-in or lead-out parameters.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LeadParams {
    pub kind: LeadKind,
    /// Radius (arc) or length (line) of the approach / departure.
    pub length: f64,
}

impl Default for LeadParams {
    fn default() -> Self {
        Self { kind: LeadKind::Arc, length: 2.0 }
    }
}

/// Holding-tab parameters (small bridges left connecting the part to the stock).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct TabParams {
    /// Width of each tab along the profile.
    pub width: f64,
    /// Height the tool lifts for the tab (typically the last pass depth).
    pub height: f64,
    /// Number of tabs around the profile (0 = none).  Distributed evenly by arc length.
    pub count: usize,
}

impl Default for TabParams {
    fn default() -> Self {
        Self { width: 3.0, height: 1.0, count: 0 }
    }
}

/// Tool and machining parameters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolParams {
    /// Tool diameter (must be > 0 unless offset is `On`).
    pub diameter: f64,
    /// Horizontal feed rate (drawing units / min).
    pub feed_rate: f64,
    /// Vertical plunge rate (drawing units / min).
    pub plunge_rate: f64,
    /// Spindle speed (RPM).
    pub spindle_speed: f64,
    /// Total cut depth below the surface (positive downward).
    pub depth: f64,
    /// Height for rapid travel above the work (positive above Z 0).
    pub safe_z: f64,
    /// Maximum depth per pass; 0 = full depth in one pass.
    pub step_down: f64,
}

/// How to order the cuts when there are multiple profiles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CutOrder {
    /// Nearest-neighbour: always go to the closest uncut profile next.
    Nearest,
    /// Inside-out: cut holes first, then outer profiles (avoids releasing parts too early).
    #[default]
    InsideOut,
}

/// Full set of CAM parameters for a 2-D profile job.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CamJob {
    pub tool: ToolParams,
    pub offset: OffsetSide,
    pub lead_in: LeadParams,
    pub lead_out: LeadParams,
    pub tabs: TabParams,
    pub order: CutOrder,
}
