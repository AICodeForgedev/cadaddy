//! The CADaddy drawing database.
//!
//! A [`Drawing`] holds the header variables, symbol tables (layers, linetypes, styles), block
//! definitions, model space and paper-space layouts. Entity collections are copy-on-write
//! ([`EntityStore`]), so a whole-drawing clone is cheap and serves as an undo snapshot.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod array;
pub mod bimspec;
mod constraint;
mod entity;
mod extents;
pub mod field;
mod gdt;
mod header;
pub mod library;
mod plotstyle;
mod plotter;
pub mod projrules;
pub mod props;
mod rare;
mod refedit;
mod section;
pub mod shared;
mod solid;
mod store;
mod surface;
mod tables;
mod transform3;

use std::collections::{BTreeMap, BTreeSet};

pub use array::*;
pub use cadaddy_color as color;
pub use cadaddy_geom as geom;
pub use constraint::*;
pub use entity::*;
pub use extents::{BlockBoundsCache, MAX_BLOCK_DEPTH, entity_bounds, entity_bounds_cached, extent_bounds, extent_bounds_cached};
pub use gdt::*;
pub use header::{HVal, Header};
pub use plotstyle::*;
pub use plotter::*;
pub use rare::{Rare, RareEmpty};
pub use refedit::{RefEdit, WorkItem};
pub use section::*;
pub use solid::*;
pub use store::{ChunkRef, EntityStore, KIND_DIMENSION, KIND_HATCH, KIND_INSERT, KIND_OTHER, KIND_TABLE, kind_bit};
pub use surface::*;
pub use tables::*;
pub use transform3::*;

pub mod dwgprops;
/// File data CADaddy doesn't model, kept for saving (#528).
pub mod dynblock;
pub mod foreign;
pub mod geo;
pub mod hyperlink;
pub mod lighting;
pub mod markup;
pub mod material;
pub mod numfmt;
pub mod overrides;
pub mod plotstamp;
pub mod pointcloud;

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

/// What a drawing's header and small tables hold, as far as undo counts it per step.
const DRAWING_FIXED_BYTES: usize = 16 * 1024;

/// The name an embedded texture image is kept under in [`Drawing::embedded`]: this, then its path.
pub const TEXTURE_PREFIX: &str = "texture:";

#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    pub header: Header,
    /// File data CADaddy does not model, written back on save.
    pub foreign: std::sync::Arc<foreign::Foreign>,
    pub layers: Vec<Layer>,
    pub linetypes: Vec<Linetype>,
    pub text_styles: Vec<TextStyle>,
    /// Shape files loaded for SHAPE (LOAD): file names or paths, kept in the STYLE table as
    /// unnamed shape-file records.
    pub shape_files: Vec<String>,
    pub dim_styles: Vec<DimStyle>,
    pub mleader_styles: Vec<MLeaderStyle>,
    pub table_styles: Vec<TableStyle>,
    /// Multiline styles (MLSTYLE); STANDARD is always there.
    pub mline_styles: Vec<MLineStyle>,
    pub blocks: shared::Shared<BTreeMap<String, std::sync::Arc<Block>>>,
    pub model: EntityStore,
    pub layouts: Vec<Layout>,
    /// The layout a file marks active (its block is `*Paper_Space`, the others `*Paper_SpaceN`),
    /// written active again. `None`, or a layout that no longer exists: the first by tab order.
    pub active_layout: Option<String>,
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
    pub constraints: shared::Shared<Vec<Constraint>>,
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
    /// Where the drawing is on the earth (GEOGRAPHICLOCATION), if set.
    pub geo: Option<geo::GeoLocation>,
    /// BIM element types (type marks → spec records). Shared: every entity referencing the same
    /// mark inherits the type's fields. Persisted as `CADADDY_BIMTYPES` in the named-object
    /// dictionary.
    pub bim_types: bimspec::BimTypes,
    /// Project rules: mounting heights, wiring methods, clearances, tolerances, etc., each with
    /// source provenance and tier-based precedence. Persisted as `CADADDY_PROJRULES` in the
    /// named-object dictionary.
    pub project_rules: projrules::ProjectRules,
    /// The building spec (`cadaddy-walk3d`'s `BuildingSpec` as JSON text, `bspec.set`): what the
    /// plan means as a 3D building. Kept in the named-object dictionary (`CADADDY_BSPEC`).
    pub building_spec: Option<String>,
    /// Saved viewpoints and camera tours (`cadaddy-walk3d`'s `Tours` as JSON text, `tour.*`).
    /// Kept in the named-object dictionary (`CADADDY_TOURS`).
    pub tours: Option<String>,
    /// BCF issues (`cmd::bcf`'s topics as JSON text, `bcf.*`): coordination issues raised on the
    /// model, each with its viewpoint, selection, comments and snapshot. Kept in the named-object
    /// dictionary (`CADADDY_BCF`).
    pub bcf: Option<String>,
    /// The 4D construction schedule and its links to the model (`cadaddy-walk3d`'s `Schedule4d` as
    /// JSON text, `fourd.*`). Kept in the named-object dictionary (`CADADDY_4D`).
    pub schedule4d: Option<String>,
    /// Parametric parts (`cmd::part`'s parts as JSON text, `part.*`): each part's parameters and
    /// feature tree (sketches, pads, pockets, revolves, fillets, chamfers); its evaluated solid is
    /// an ordinary 3D solid in the drawing. Kept in the named-object dictionary (`CADADDY_PARTS`).
    pub parts: Option<String>,
    /// Overrides of generated elements (`overrides::Overrides` as JSON text, `override.*`): the
    /// edits a person, an agent or an import made to what a build generated, put back after every
    /// re-run. Kept in the named-object dictionary (`CADADDY_OVERRIDES`).
    pub overrides: Option<String>,
    /// What agents (and people) working on the project through its tools keep in it (JSON text,
    /// `project.*` / `review.*`): handoff notes, review decisions with their citations, the
    /// change log and the revision count, so a fresh agent resumes from the file alone. Kept in
    /// the named-object dictionary (`CADADDY_AGENT`).
    pub agent: Option<String>,
    /// Import accuracy check results (JSON text, `import.check`): the checks that ran on this
    /// drawing after a PDF import, their pass/fail status and scores, so the BIM pipeline can
    /// gate on import quality. Kept in the .cdy manifest (`importCheck`).
    pub import_check: Option<String>,
    /// Files the drawing carries inside itself, by name: texture images its materials name
    /// (`texture:<path>`, see [`Drawing::embedded_texture`]) and the baked lightmap
    /// (`lightmap.png`, `lightmap.json`). Kept in .cdy files, so a walkthrough model opens on
    /// another machine as it was.
    pub embedded: BTreeMap<String, std::sync::Arc<Vec<u8>>>,
    /// The plot stamp's settings (PLOTSTAMP); `None` until set. Each layout's page setup says
    /// whether its plots carry it.
    pub plot_stamp: Option<plotstamp::PlotStamp>,
    /// Drawing properties (DWGPROPS): summary and custom properties.
    pub properties: dwgprops::DwgProps,
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
    pub kept_handles: shared::Shared<BTreeMap<String, u64>>,
    pub field_values: BTreeMap<Handle, Vec<String>>,
    /// A save option, set on the copy SAVEAS `portableFonts` writes: the file gets Windows fonts
    /// for the Linux fonts of text styles, and each style keeps its own in CADaddy xdata, so the
    /// drawing reopens with it. Never set on an open drawing.
    pub portable_fonts: bool,
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
    /// Roughly how many bytes this drawing holds that `newer` doesn't share with it (objects of
    /// model space, the layouts and the blocks): what keeping it as an undo step costs.
    pub fn unique_bytes(&self, newer: &Drawing) -> usize {
        // Every step keeps its own copy of the header and the small tables (measured: about 12 KB
        // for the header, the rest is a few hundred bytes a table); the big tables are shared
        // until an edit changes them.
        let mut bytes = DRAWING_FIXED_BYTES.saturating_add(self.model.unique_bytes(&newer.model));
        if !self.blocks.same(&newer.blocks) {
            bytes = bytes.saturating_add(self.blocks.keys().map(|k| k.len().saturating_add(96)).fold(0, usize::saturating_add));
        }
        if !self.constraints.same(&newer.constraints) {
            bytes = bytes.saturating_add(self.constraints.len().saturating_mul(256));
        }
        if !self.kept_handles.same(&newer.kept_handles) {
            bytes = bytes.saturating_add(self.kept_handles.keys().map(|k| k.len().saturating_add(56)).fold(0, usize::saturating_add));
        }
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

    /// An empty metric drawing (millimetres, ISO-25 dimensions, multileaders and tables to match).
    pub fn new_metric() -> Self {
        let mut d = Drawing::bare();
        d.header = Header::metric();
        d.scales = metric_scales();
        d.dim_styles.push(DimStyle::iso25());
        d.mleader_styles = vec![MLeaderStyle::metric()];
        d.table_styles = vec![TableStyle::metric()];
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
            shape_files: Vec::new(),
            dim_styles: vec![DimStyle::default()],
            mleader_styles: vec![MLeaderStyle::default()],
            table_styles: vec![TableStyle::default()],
            mline_styles: vec![MLineStyle::default()],
            blocks: Default::default(),
            model: EntityStore::new(),
            layouts: vec![Layout::new("Layout1", 1), Layout::new("Layout2", 2)],
            active_layout: None,
            views: Vec::new(),
            vports: Vec::new(),
            ucss: Vec::new(),
            layer_states: Vec::new(),
            layer_filters: Vec::new(),
            groups: Vec::new(),
            plot_style_tables: Vec::new(),
            constraints: Default::default(),
            parametric: Parametric::default(),
            scales: imperial_scales(),
            handseed: 0x100,
            hidden_objects: BTreeSet::new(),
            hidden_saved: false,
            materials: Vec::new(),
            sun: lighting::Sun::default(),
            geo: None,
            bim_types: bimspec::BimTypes::new(),
            project_rules: projrules::ProjectRules::new(),
            building_spec: None,
            tours: None,
            bcf: None,
            schedule4d: None,
            parts: None,
            overrides: None,
            agent: None,
            import_check: None,
            embedded: BTreeMap::new(),
            plot_stamp: None,
            properties: dwgprops::DwgProps::default(),
            read_notes: Vec::new(),
            refedit: None,
            skipped_records: BTreeMap::new(),
            field_values: BTreeMap::new(),
            kept_handles: Default::default(),
            portable_fonts: false,
        }
    }

    /// The embedded copy of the texture image file `path` (as a material names it), if the
    /// drawing carries one.
    pub fn embedded_texture(&self, path: &str) -> Option<&std::sync::Arc<Vec<u8>>> {
        self.embedded.get(&format!("{TEXTURE_PREFIX}{path}"))
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

    /// The layouts in the order their paper space blocks are named in a file: the active one
    /// (`*Paper_Space`) first, then the others by tab order (`*Paper_Space0`, `*Paper_Space1`, …).
    pub fn paper_space_layouts(&self) -> Vec<&Layout> {
        let mut ls: Vec<&Layout> = self.layouts.iter().collect();
        ls.sort_by_key(|l| l.tab_order);
        if let Some(i) = self.active_layout.as_ref().and_then(|a| ls.iter().position(|l| l.name.eq_ignore_ascii_case(a))) {
            let active = ls.remove(i);
            ls.insert(0, active);
        }
        ls
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

    /// Whether an entity counts in the extents: as AutoCAD 2024 counts it for ZOOM Extents and
    /// EXTMIN/EXTMAX, on a layer that is off too, but not on a frozen one.
    pub fn in_extents(&self, e: &Entity) -> bool {
        e.common.visible && self.layer(&e.common.layer).is_none_or(|l| !l.frozen) && !self.is_hidden_object(e.handle)
    }

    /// Bounds of the entities in a space that count in its extents ([`Drawing::in_extents`]).
    pub fn extents(&self, space: &Space) -> cadaddy_geom::Bounds2 {
        let mut b = cadaddy_geom::Bounds2::EMPTY;
        // One block-bounds cache for the whole space: every reference to a block reuses its
        // measured contents instead of measuring the definition again.
        let mut cache = BlockBoundsCache::default();
        if let Some(store) = self.space(space) {
            for e in store.iter() {
                if self.in_extents(e) {
                    b = b.union(&extent_bounds_cached(self, e, &mut cache));
                }
            }
        }
        b
    }

    /// The extents of the entities whose own bounds are finite and within `limit` of the origin,
    /// and how many visible entities were left out for being beyond it.
    pub fn extents_within(&self, space: &Space, limit: f64) -> (cadaddy_geom::Bounds2, usize) {
        let mut b = cadaddy_geom::Bounds2::EMPTY;
        let mut skipped = 0;
        if let Some(store) = self.space(space) {
            for e in store.iter().filter(|e| self.is_visible(e)) {
                let eb = entity_bounds(self, e, 0);
                if eb.is_empty() {
                    continue;
                }
                if within_limit(&eb, limit) {
                    b = b.union(&eb);
                } else {
                    skipped += 1;
                }
            }
        }
        (b, skipped)
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

/// Whether both corners of `b` are finite and no further than `limit` from the origin on each axis.
pub fn within_limit(b: &cadaddy_geom::Bounds2, limit: f64) -> bool {
    [b.min.x, b.min.y, b.max.x, b.max.y].iter().all(|v| v.is_finite() && v.abs() <= limit)
}
