//! Walk mode: the 3D model is derived from the 2D drawing, never modelled by hand.
//!
//! [`Rules`] say where each floor plan sits (a region of the sheet, a base point, an elevation) and
//! what each layer becomes (walls extruded to a height, devices placed at their mounting height).
//! [`derive`] turns a drawing plus rules into a [`Scene`] of triangles, edges, wall segments for
//! collision and device records for picking. [`Walker`] is the first-person camera.
//!
//! World units are feet: x east, y north, z up.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod derive;
mod math;
mod measure;
mod openings;
mod pick;
mod rules;
mod walker;

pub use derive::{DeviceInfo, LevelInfo, Owner, Scene, Seg, Vertex, derive, derive_set};
pub use math::{Mat4, V3};
pub use measure::{Dim, DimKind, measure};
pub use pick::{Hit, pick};
pub use rules::{DeviceRules, LayerKind, LayerRule, Level, Mount, Openings, Rules};
pub use walker::Walker;
