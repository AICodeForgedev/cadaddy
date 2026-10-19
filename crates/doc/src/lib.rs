//! The CADaddy drawing database.
//!
//! A [`Drawing`] holds the header variables, symbol tables (layers, linetypes, styles), block
//! definitions, model space and paper-space layouts. Entity collections are copy-on-write
//! ([`EntityStore`]), so a whole-drawing clone is cheap and serves as an undo snapshot.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod array;
mod constraint;
mod entity;
mod extents;
pub mod field;
mod gdt;
mod header;
pub mod library;
mod plotstyle;
mod plotter;
mod refedit;
mod section;
mod solid;
mod store;
mod tables;
mod transform3;

use std::collections::{BTreeMap, BTreeSet};

pub use array::*;
pub use cadaddy_color as color;
pub use cadaddy_geom as geom;
pub use constraint::*;
pub use entity::*;
pub use extents::{MAX_BLOCK_DEPTH, entity_bounds};
pub use gdt::*;
pub use header::{HVal, Header};
pub use plotstyle::*;
pub use plotter::*;
pub use refedit::{RefEdit, WorkItem};
pub use section::*;
pub use solid::*;
pub use store::{ChunkRef, EntityStore};
pub use tables::*;
pub use transform3::*;

/// File data CADaddy doesn't model, kept for saving (#528).
pub mod dynblock;
pub mod foreign;
pub mod hyperlink;
pub mod lighting;
pub mod material;

#[derive(Debug, thiserror::Error)]
pub enum DocError {
    #[error("no such layer `{0}`")]
    NoLayer(String),
    #[error("no such block `{0}`")]
    NoBlock(String),
    #[error("no such entity {0:?}")]
    NoEntity(Handle),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, DocError>;

/// Which entity collection: model space or a named layout's paper space.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Space {
    #[default]
    Model,
    Paper(String),
}

impl Space {
    /// Whether this is a layout's paper space.
    pub fn is_paper(&self) -> bool {
        matches!(self, Space::Paper(_))
    }

    /// The layout whose paper space this is; `None` for model space.
    pub fn layout(&self) -> Option<&str> {
        match self {
            Space::Paper(n) => Some(n),
            Space::Model => None,
        }
    }

    /// The tab name (CTAB): `Model`, or the layout's name.
    pub fn tab_name(&self) -> &str {
        self.layout().unwrap_or("Model")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    pub header: Header,
    /// File data CADaddy does not model, written back on save.
    pub foreign: std::sync::Arc<foreign::Foreign>,
    pub layers: Vec<Layer>,
    pub linetypes: Vec<Linetype>,
    pub text_styles: Vec<TextStyle>,
    pub dim_styles: Vec<DimStyle>,
    pub mleader_styles: Vec<MLeaderStyle>,
    pub table_styles: Vec<TableStyle>,
    /// Multiline styles (MLSTYLE); STANDARD is always there.
    pub mline_styles: Vec<MLineStyle>,
    pub blocks: BTreeMap<String, std::sync::Arc<Block>>,
    pub model: EntityStore,
    pub layouts: Vec<Layout>,
    pub views: Vec<NamedView>,
    /// Model space viewport configurations (VPORTS): the tiles shown and named ones.
    pub vports: Vec<VportConfig>,
    pub ucss: Vec<Ucs>,
    pub layer_states: Vec<LayerState>,
    /// Layer filters of the Layer Properties Manager.
    pub layer_filters: Vec<LayerFilter>,
    pub groups: Vec<Group>,
    /// Plot style tables kept with the drawing (STYLESMANAGER); they shadow built-in tables of
    /// the same name.
    pub plot_style_tables: Vec<PlotStyleTable>,
    /// Parametric constraints (GEOMCONSTRAINT / DIMCONSTRAINT).
    pub constraints: Vec<Constraint>,
    /// User parameters and parametric settings.
    pub parametric: Parametric,
    /// The annotation scale list (SCALELISTEDIT).
    pub scales: Vec<AnnoScale>,
    /// Next free handle.
    pub handseed: u64,
    /// Objects hidden by ISOLATEOBJECTS / HIDEOBJECTS. Written to the file only when
    /// `hidden_saved` is set (OBJECTISOLATIONMODE 1); otherwise a drawing opens with everything
    /// shown again.
    pub hidden_objects: BTreeSet<Handle>,
    /// The hidden objects are saved with the drawing (OBJECTISOLATIONMODE 1, or read from a file
    /// that saved them).
    pub hidden_saved: bool,
    /// The drawing's own materials (MATERIALS), by name; Global, ByLayer and ByBlock are implied.
    pub materials: Vec<material::Material>,
    /// The sun of rendered views and their exposure (SUNPROPERTIES, RENDEREXPOSURE).
    pub sun: lighting::Sun,
    /// The building spec (`cadaddy-walk3d`'s `BuildingSpec` as JSON text, `bspec.set`): what the
    /// plan means as a 3D building. Kept in the named-object dictionary (`CADADDY_BSPEC`).
    pub building_spec: Option<String>,
    /// What reading the file left out (a DWG's object records the reader couldn't decode), told
    /// to the user when the drawing opens. Never written to a file.
    pub read_notes: Vec<String>,
    /// The in-place block reference edit in progress (REFEDIT). Never written to a file; files are
    /// written from [`Drawing::committed`].
    pub refedit: Option<RefEdit>,
    /// Records of the file this drawing was read from that the reader could not place (a damaged
    /// file), counted by the section they came from. Never written to a file.
    pub skipped_records: BTreeMap<String, usize>,
    /// The values fields last evaluated to ([`field`]): per object, in the order the codes
    /// appear in its text ([`field::texts`]).
    /// Handles another program's file gave its table records and objects, by `KIND/NAME`
    /// (`LAYER/WALLS`, `TABLE/LAYER`, `BLOCK_RECORD/DOOR`, `DICT/ACAD_GROUP`, `LAYOUT/LAYOUT1`…):
    /// saving writes them again, so links into the file from outside stay valid.
    pub kept_handles: BTreeMap<String, u64>,
    pub field_values: BTreeMap<Handle, Vec<String>>,
}

impl Default for Drawing {
    fn default() -> Self {
        Drawing::new_imperial()
    }
}

impl Drawing {
    /// An empty drawing with the standard tables (inch-based, like acad.dwt-style defaults).
    pub fn new_imperial() -> Self {
        let mut d = Drawing::bare();
        d.header = Header::imperial();
        d
    }
    /// An empty metric drawing (millimetres, ISO-25 dimensions).
    /// Roughly how many bytes this drawing holds that `newer` doesn't share with it (objects of
    /// model space, the layouts and the blocks): what keeping it as an undo step costs.
    pub fn unique_bytes(&self, newer: &Drawing) -> usize {
        let mut bytes = self.model.unique_bytes(&newer.model);
        for l in &self.layouts {
            bytes = bytes.saturating_add(match newer.layouts.iter().find(|n| n.name == l.name) {
                Some(n) => l.entities.unique_bytes(&n.entities),
                None => l.entities.unique_bytes(&EntityStore::new()),
            });
        }
        for (name, b) in &self.blocks {
            bytes = bytes.saturating_add(match newer.blocks.get(name) {
                Some(n) if std::sync::Arc::ptr_eq(b, n) => 0,
                Some(n) => b.entities.unique_bytes(&n.entities),
                None => b.entities.unique_bytes(&EntityStore::new()),
            });
        }
        bytes
    }

    pub fn new_metric() -> Self {
        let mut d = Drawing::bare();
        d.header = Header::metric();
        d.scales = metric_scales();
        d.dim_styles.push(DimStyle::iso25());
        d.header.set_str("DIMSTYLE", "ISO-25");
        d
    }
    fn bare() -> Self {
        Drawing {
            header: Header::default(),
            foreign: std::sync::Arc::default(),
            layers: vec![Layer::default(), Layer { name: "Defpoints".into(), plot: false, ..Layer::default() }],
            linetypes: vec![
                Linetype { name: "ByBlock".into(), description: String::new(), pattern: Vec::new() },
                Linetype { name: "ByLayer".into(), description: String::new(), pattern: Vec::new() },
                Linetype::continuous(),
            ],
            text_styles: vec![TextStyle::default()],
            dim_styles: vec![DimStyle::default()],
            mleader_styles: vec![MLeaderStyle::default()],
            table_styles: vec![TableStyle::default()],
            mline_styles: vec![MLineStyle::default()],
            blocks: BTreeMap::new(),
            model: EntityStore::new(),
            layouts: vec![Layout::new("Layout1", 1), Layout::new("Layout2", 2)],
            views: Vec::new(),
            vports: Vec::new(),
            ucss: Vec::new(),
            layer_states: Vec::new(),
            layer_filters: Vec::new(),
            groups: Vec::new(),
            plot_style_tables: Vec::new(),
            constraints: Vec::new(),
            parametric: Parametric::default(),
            scales: imperial_scales(),
            handseed: 0x100,
            hidden_objects: BTreeSet::new(),
            hidden_saved: false,
            materials: Vec::new(),
            sun: lighting::Sun::default(),
            building_spec: None,
            read_notes: Vec::new(),
            refedit: None,
            skipped_records: BTreeMap::new(),
            field_values: BTreeMap::new(),
            kept_handles: BTreeMap::new(),
        }
    }

    /// Allocate a fresh handle.
    pub fn new_handle(&mut self) -> Handle {
        let h = Handle(self.handseed);
        self.handseed = self.handseed.saturating_add(1);
        h
    }
    /// Make sure future handles don't collide with `h` (after loading a file).
    pub fn bump_handseed(&mut self, h: Handle) {
        if h.0 >= self.handseed {
            self.handseed = h.0.saturating_add(1);
        }
    }

    pub fn layer(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }
    pub fn layer_mut(&mut self, name: &str) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(name))
    }
    pub fn linetype(&self, name: &str) -> Option<&Linetype> {
        self.linetypes.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }
    pub fn text_style(&self, name: &str) -> Option<&TextStyle> {
        self.text_styles.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }
    pub fn dim_style(&self, name: &str) -> Option<&DimStyle> {
        self.dim_styles.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }

    /// The DIM* variables in the header that differ from the current dimension style, as
    /// [`DimStyle`] fields: AutoCAD's style overrides (set with SETVAR), which new dimensions
    /// take on.
    pub fn dim_overrides(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut out = serde_json::Map::new();
        let Some(st) = self.dim_style(&self.header.str("DIMSTYLE", "Standard")) else { return out };
        let base = serde_json::to_value(st).unwrap_or_default();
        for (var, field) in DIMVARS {
            let Some(v) = self.header.get(var).and_then(|h| serde_json::to_value(h).ok()) else { continue };
            let mut cand = st.clone();
            if !cand.apply_fields(&serde_json::Map::from_iter([((*field).to_string(), v)])).is_empty() {
                continue;
            }
            let Some(cv) = serde_json::to_value(&cand).ok().and_then(|c| c.get(*field).cloned()) else { continue };
            let same = match (base.get(*field), &cv) {
                // Text style and arrow block names compare without case.
                (Some(serde_json::Value::String(a)), serde_json::Value::String(b))
                    if matches!(
                        *field,
                        "textStyle"
                            | "arrowBlock"
                            | "arrowBlock1"
                            | "arrowBlock2"
                            | "leaderArrowBlock"
                            | "dimLinetype"
                            | "extLinetype1"
                            | "extLinetype2"
                    ) =>
                {
                    a.eq_ignore_ascii_case(b)
                }
                (b, cv) => b == Some(cv),
            };
            if !same {
                out.insert((*field).to_string(), cv);
            }
        }
        out
    }

    /// Set the header's DIM* variables to the current dimension style's values, clearing the
    /// overrides (as making a style current or editing it does). Variables the header doesn't
    /// carry stay absent; ones that can't hold the style's value are removed.
    pub fn sync_dim_vars(&mut self) {
        let Some(st) = self.dim_style(&self.header.str("DIMSTYLE", "Standard")) else { return };
        let v = serde_json::to_value(st).unwrap_or_default();
        for (var, field) in DIMVARS {
            let Some(old) = self.header.get(var) else { continue };
            let new = match (old, v.get(*field)) {
                (HVal::Int(_), Some(serde_json::Value::Bool(b))) => Some(HVal::Int(i64::from(*b))),
                (HVal::Int(_), Some(serde_json::Value::Number(n))) => {
                    n.as_i64().or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 1e15).map(|f| f as i64)).map(HVal::Int)
                }
                (HVal::Real(_), Some(serde_json::Value::Number(n))) => n.as_f64().map(HVal::Real),
                (HVal::Str(_), Some(serde_json::Value::String(s))) => Some(HVal::Str(s.clone())),
                _ => None,
            };
            match new {
                Some(h) => self.header.set(var, h),
                None => {
                    self.header.vars.remove(*var);
                }
            }
        }
    }
    pub fn block(&self, name: &str) -> Option<&std::sync::Arc<Block>> {
        self.blocks.get(name).or_else(|| self.blocks.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v))
    }
    pub fn layout(&self, name: &str) -> Option<&Layout> {
        self.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }

    /// Add a layer if missing; returns whether it was created.
    pub fn ensure_layer(&mut self, name: &str) -> bool {
        if self.layer(name).is_some() {
            return false;
        }
        self.layers.push(Layer::new(name));
        true
    }

    pub fn space(&self, space: &Space) -> Option<&EntityStore> {
        match space {
            Space::Model => Some(&self.model),
            Space::Paper(n) => self.layouts.iter().find(|l| &l.name == n).map(|l| &l.entities),
        }
    }
    pub fn space_mut(&mut self, space: &Space) -> Option<&mut EntityStore> {
        match space {
            Space::Model => Some(&mut self.model),
            Space::Paper(n) => self.layouts.iter_mut().find(|l| &l.name == n).map(|l| &mut l.entities),
        }
    }

    /// Find an entity in model space or any layout.
    pub fn entity(&self, h: Handle) -> Option<&std::sync::Arc<Entity>> {
        self.model.get(h).or_else(|| self.layouts.iter().find_map(|l| l.entities.get(h)))
    }
    pub fn space_of(&self, h: Handle) -> Option<Space> {
        if self.model.contains(h) {
            return Some(Space::Model);
        }
        self.layouts.iter().find(|l| l.entities.contains(h)).map(|l| Space::Paper(l.name.clone()))
    }
    pub fn modify_entity<F: FnOnce(&mut Entity)>(&mut self, h: Handle, f: F) -> Result<()> {
        let sp = self.space_of(h).ok_or(DocError::NoEntity(h))?;
        let store = self.space_mut(&sp).ok_or(DocError::NoEntity(h))?;
        if store.modify(h, f) { Ok(()) } else { Err(DocError::NoEntity(h)) }
    }
    pub fn remove_entity(&mut self, h: Handle) -> Option<std::sync::Arc<Entity>> {
        let sp = self.space_of(h)?;
        self.space_mut(&sp)?.remove(h)
    }

    /// Add an entity to a space with a new handle, using the given common properties.
    pub fn add(&mut self, space: &Space, common: Common, kind: EntityKind) -> Result<Handle> {
        let h = self.new_handle();
        self.ensure_layer(&common.layer);
        let store = self.space_mut(space).ok_or_else(|| DocError::Invalid(format!("no such space {space:?}")))?;
        store.push(Entity { handle: h, common, kind });
        Ok(h)
    }

    /// Effective visibility of an entity (its own flag, its layer on/thawed, not hidden by object
    /// isolation).
    pub fn is_visible(&self, e: &Entity) -> bool {
        e.common.visible && self.layer(&e.common.layer).is_none_or(Layer::visible) && !self.is_hidden_object(e.handle)
    }

    /// Whether ISOLATEOBJECTS / HIDEOBJECTS hid `h` for this session.
    pub fn is_hidden_object(&self, h: Handle) -> bool {
        !self.hidden_objects.is_empty() && self.hidden_objects.contains(&h)
    }

    /// Bounds of all visible entities in a space.
    pub fn extents(&self, space: &Space) -> cadaddy_geom::Bounds2 {
        let mut b = cadaddy_geom::Bounds2::EMPTY;
        if let Some(store) = self.space(space) {
            for e in store.iter() {
                if self.is_visible(e) {
                    b = b.union(&entity_bounds(self, e, 0));
                }
            }
        }
        b
    }

    /// Total entity count across model and paper space.
    pub fn entity_count(&self) -> usize {
        self.model.len() + self.layouts.iter().map(|l| l.entities.len()).sum::<usize>()
    }

    /// Next unused anonymous block name with the given prefix (e.g. `*D`).
    pub fn anonymous_block_name(&self, prefix: &str) -> String {
        let mut i = 1usize;
        loop {
            let n = format!("{prefix}{i}");
            if self.block(&n).is_none() || i > 1_000_000 {
                return n;
            }
            i += 1;
        }
    }
}

mod anno;
pub use anno::*;

#[cfg(test)]
mod tests;
