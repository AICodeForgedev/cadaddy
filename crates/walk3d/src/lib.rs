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

pub mod codebook;
pub mod concrete;
mod derive;
mod doors;
pub mod field;
pub mod fixtures;
pub mod fourd;
mod ground;
pub mod levels;
mod math;
mod measure;
mod openings;
pub mod package;
mod pick;
pub mod rfi;
mod rules;
mod sbox;
mod spec;
pub mod stairs;
pub mod tags;
mod takeoff;
mod tour;
pub mod validate;
pub mod validate_kinds;
pub mod video;
mod walker;

pub use derive::{DeviceInfo, LevelInfo, Owner, Part, Room, Scene, Seg, Vertex, derive, derive_set, level_bases, rooms};
pub use doors::DoorLeaf;
pub use ground::Ground;
pub use levels::{Datum, DatumKind, LevelNote, ShaftMark, ShaftRun, SheetLink};
pub use math::{Mat4, V3};
pub use measure::{Dim, DimKind, measure};
pub use pick::{Hit, pick};
pub use rfi::{Rfi, RfiKind, RfiSide, RfiStatus, Severity};
pub use rules::{DeviceRules, LayerKind, LayerRule, Level, Mount, Openings, Rules};
// A room of the building spec (`derive::Room` is a room found in the plan).
pub use concrete::{Concrete, ConcreteElement, Depression, Embed, MAX_CONCRETE, PadDecision, Pour, Rebar, Signoff};
pub use spec::{
    ArcFlash, Assembly, AssemblyLayer, Band, BandKind, Bond, BuildingSpec, Busbar, CeilingArea, CeilingGrid, CeilingItem, CeilingKind, CeilingType,
    Circulation, CirculationKind, Cricket, Described, DuctType, ElecDevice, Electrode, Enclosure, Equipment, EquipmentElectrical, Exterior, Facade,
    Family, FamilyKind, Feeder, FixtureType, GridColumn, GroundRequirement, Grounding, Hang, Head, HeadTo, HeadTrack, HeightRule, Hoistway, Issue,
    ItemKind, LayerRole, LayerSide, LevelParapet, LoadSummary, LoadSummaryLine, MAX_ASSEMBLY_LAYERS, MarkStatus, Mep, MepEquipment, MepInvert,
    MepKind, MepRevision, MepRun, MepSystem, MepTerminal, MountNote, OpeningTag, PanelCircuit, PanelSchedule, PartPath, Pick, PointOfEntry, Rcp,
    Role, Roof, RoofDrain, RoofSlope, Room as SpecRoom, SPEC_VERSION, Schedule, ScheduleRow, Service, Side, Site, Soffit, SoffitType, Stair,
    Surfaces, Termination, UferNote, ValueStatus, WallFrom, WallMark, WallType, feet, inches, level_at, merge, pipe_system_default_color, pitch_text,
};
pub use takeoff::{Takeoff, TakeoffRow, takeoff};
pub use tour::{Look, MAX_FRAMES, MAX_TOUR_STOPS, Pose, TOURS_VERSION, Tour, TourPath, Tours, Viewpoint};
pub use walker::Walker;
