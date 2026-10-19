//! The drawing in .cdy chunks (schema v1): what each chunk holds, and the mapping both ways.
//!
//! | tag  | holds | encoding |
//! |------|-------|----------|
//! | MANI | manifest: format and writer versions, counts, units, extents, the thumbnail's chunk | JSON |
//! | HEAD | the header variables | tokens (postcard) |
//! | TABL | layers, linetypes, styles, views, viewports, UCSs, layer states, groups, plot style tables, constraints, parameters, scale list, handle seed, saved hidden objects | tokens |
//! | BLKS | block definitions, each `[name, block, objects]` (one per chunk) | tokens |
//! | ENTS | model space objects, a chunk of the drawing's object store (up to [`ENTITY_CHUNK`]) each, in draw order | tokens |
//! | LAYO | one layout with its paper space objects, `[layout, objects]` | tokens |
//! | FRGN | file data CADaddy keeps without modelling it | tokens |
//! | THMB | a PNG preview | raw |

use std::collections::BTreeMap;
use std::sync::Arc;

use cadaddy_doc::foreign::{Foreign, ForeignExtra, ForeignObject};
use cadaddy_doc::{Block, Drawing, Entity, EntityStore, Handle, Layout, RawTag};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::container::{self, Read};
use super::tokens;

/// Model space objects per ENTS chunk (as many as one `EntityStore` chunk holds).
pub const ENTITY_CHUNK: usize = 1024;
/// The schema version of each chunk type this reader and writer know.
const V1: u16 = 1;
const KNOWN: [&[u8; 4]; 8] = [b"MANI", b"HEAD", b"TABL", b"BLKS", b"ENTS", b"LAYO", b"FRGN", b"THMB"];

/// What the file was read with besides the drawing: notes about damaged or unread parts.
#[derive(Clone, Debug, Default)]
pub struct ReadReport {
    pub notes: Vec<String>,
    pub manifest: Option<Value>,
    pub thumbnail: Option<Vec<u8>>,
}

/// The tables chunk.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tables {
    layers: Value,
    linetypes: Value,
    text_styles: Value,
    dim_styles: Value,
    mleader_styles: Value,
    table_styles: Value,
    mline_styles: Value,
    views: Value,
    vports: Value,
    ucss: Value,
    layer_states: Value,
    groups: Value,
    plot_style_tables: Value,
    constraints: Value,
    parametric: Value,
    scales: Value,
    handseed: u64,
    hidden_saved: bool,
    hidden: Vec<u64>,
    // Added after the first files (keys a file lacks read as empty). They were left out, so a
    // .cdy file lost its materials, layer filters and field values (tester 0.7.7e).
    #[serde(default)]
    layer_filters: Value,
    #[serde(default)]
    materials: Value,
    /// The sun and render exposure (tester 0.7.7g: SUNPROPERTIES and RENDEREXPOSURE came back
    /// as defaults from .cdy).
    #[serde(default)]
    sun: Value,
    /// (object handle, its field values) in handle order.
    #[serde(default)]
    field_values: Vec<(u64, Vec<String>)>,
    /// Handles another program's file gave its table records and objects (`KIND/NAME`, handle),
    /// kept for when the drawing is saved as DXF or DWG again.
    #[serde(default)]
    kept_handles: Vec<(String, u64)>,
}

/// The unmodelled file data, as a stable mirror of [`Foreign`] (its tuple-keyed maps as lists).
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(super) struct ForeignV1 {
    classes: Vec<Vec<RawTag>>,
    objects: Vec<(u64, String, Vec<RawTag>)>,
    dictionary: Vec<(String, u64)>,
    entities: Vec<(u64, ExtraV1)>,
    records: Vec<((String, String), ExtraV1)>,
    targets: Vec<(u64, (String, String))>,
    data_store: Vec<RawTag>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ExtraV1 {
    xdict: Option<u64>,
    reactors: Vec<u64>,
    xdata: Vec<RawTag>,
    color_book: Option<(String, String)>,
    props: Vec<RawTag>,
}

impl From<&ForeignExtra> for ExtraV1 {
    fn from(x: &ForeignExtra) -> Self {
        ExtraV1 {
            xdict: x.xdict.map(|h| h.0),
            reactors: x.reactors.iter().map(|h| h.0).collect(),
            xdata: x.xdata.clone(),
            color_book: x.color_book.clone(),
            props: x.props.clone(),
        }
    }
}

impl From<ExtraV1> for ForeignExtra {
    fn from(x: ExtraV1) -> Self {
        ForeignExtra {
            xdict: x.xdict.map(Handle),
            reactors: x.reactors.into_iter().map(Handle).collect(),
            xdata: x.xdata,
            color_book: x.color_book,
            props: x.props,
        }
    }
}

pub(super) fn foreign_v1(f: &Foreign) -> ForeignV1 {
    ForeignV1 {
        classes: f.classes.clone(),
        objects: f.objects.iter().map(|o| (o.handle.0, o.kind.clone(), o.tags.clone())).collect(),
        dictionary: f.dictionary.iter().map(|(k, h)| (k.clone(), h.0)).collect(),
        entities: f.entities.iter().map(|(h, x)| (h.0, x.into())).collect(),
        records: f.records.iter().map(|(k, x)| (k.clone(), x.into())).collect(),
        targets: f.targets.iter().map(|(h, k)| (h.0, k.clone())).collect(),
        data_store: f.data_store.clone(),
    }
}

pub(super) fn foreign_from(v: ForeignV1) -> Foreign {
    Foreign {
        classes: v.classes,
        objects: v.objects.into_iter().map(|(h, kind, tags)| ForeignObject { handle: Handle(h), kind, tags }).collect(),
        dictionary: v.dictionary.into_iter().map(|(k, h)| (k, Handle(h))).collect(),
        entities: v.entities.into_iter().map(|(h, x)| (Handle(h), x.into())).collect::<BTreeMap<_, _>>(),
        records: v.records.into_iter().map(|(k, x)| (k, x.into())).collect::<BTreeMap<_, _>>(),
        targets: v.targets.into_iter().map(|(h, k)| (Handle(h), k)).collect::<BTreeMap<_, _>>(),
        data_store: v.data_store,
    }
}

fn value<T: Serialize>(t: &T) -> Result<Value, String> {
    serde_json::to_value(t).map_err(|e| e.to_string())
}

/// The objects of a block or layout (which their own serde form leaves out), in draw order.
fn objects(store: &EntityStore) -> Result<Value, String> {
    store.iter().map(|e| value(e.as_ref())).collect::<Result<Vec<_>, _>>().map(Value::Array)
}

/// The objects of a block or layout read back; damaged ones are noted and left out.
fn store_from(v: Value, notes: &mut Vec<String>) -> EntityStore {
    let mut store = EntityStore::new();
    if let Value::Array(list) = v {
        for item in list {
            if let Some(e) = take::<Entity>(item, "an object", notes) {
                store.push(e);
            }
        }
    } else {
        notes.push("a list of objects was damaged".into());
    }
    store
}

/// A `[record, objects]` or `[name, record, objects]` item split into its parts.
fn parts<const N: usize>(v: Value) -> Option<[Value; N]> {
    let Value::Array(list) = v else { return None };
    list.try_into().ok()
}

fn take<T: DeserializeOwned>(v: Value, what: &str, notes: &mut Vec<String>) -> Option<T> {
    match serde_json::from_value(v) {
        Ok(t) => Some(t),
        Err(e) => {
            notes.push(format!("{what} couldn't be read ({e})"));
            None
        }
    }
}

/// The manifest chunk's JSON: format and writer, units, extents, counts, the preview's chunk.
pub(super) fn manifest(d: &Drawing, thumbnail: bool) -> Vec<u8> {
    let ext = d.extents(&cadaddy_doc::Space::Model);
    let manifest = json!({
        "format": container::FORMAT_VERSION,
        "writer": format!("CADaddy {}", env!("CARGO_PKG_VERSION")),
        "insunits": d.header.i64("INSUNITS", 0),
        "measurement": d.header.i64("MEASUREMENT", 0),
        "extents": if ext.is_empty() { Value::Null } else { json!([ext.min.x, ext.min.y, ext.max.x, ext.max.y]) },
        "objects": d.model.len(),
        "blocks": d.blocks.len(),
        "layouts": d.layouts.iter().map(|l| l.name.clone()).collect::<Vec<_>>(),
        "thumbnail": thumbnail.then_some("THMB"),
    });
    serde_json::to_string_pretty(&manifest).unwrap_or_default().into_bytes()
}

/// The header chunk's payload.
pub(super) fn header(d: &Drawing) -> Result<Vec<u8>, String> {
    Ok(tokens::encode(&value(&d.header)?))
}

/// The tables chunk's payload.
pub(super) fn tables(d: &Drawing) -> Result<Vec<u8>, String> {
    let tables = Tables {
        layers: value(&d.layers)?,
        linetypes: value(&d.linetypes)?,
        text_styles: value(&d.text_styles)?,
        dim_styles: value(&d.dim_styles)?,
        mleader_styles: value(&d.mleader_styles)?,
        table_styles: value(&d.table_styles)?,
        mline_styles: value(&d.mline_styles)?,
        views: value(&d.views)?,
        vports: value(&d.vports)?,
        ucss: value(&d.ucss)?,
        layer_states: value(&d.layer_states)?,
        groups: value(&d.groups)?,
        plot_style_tables: value(&d.plot_style_tables)?,
        constraints: value(&d.constraints)?,
        parametric: value(&d.parametric)?,
        scales: value(&d.scales)?,
        handseed: d.handseed,
        hidden_saved: d.hidden_saved,
        hidden: if d.hidden_saved { d.hidden_objects.iter().map(|h| h.0).collect() } else { Vec::new() },
        layer_filters: value(&d.layer_filters)?,
        materials: value(&d.materials)?,
        sun: value(&d.sun)?,
        field_values: d.field_values.iter().map(|(h, v)| (h.0, v.clone())).collect(),
        kept_handles: d.kept_handles.iter().map(|(k, h)| (k.clone(), *h)).collect(),
    };
    Ok(tokens::encode(&value(&tables)?))
}

/// A block chunk's payload: one `[name, block, objects]`.
pub(super) fn block(name: &str, b: &Block) -> Result<Vec<u8>, String> {
    Ok(tokens::encode(&json!([[name, value(b)?, objects(&b.entities)?]])))
}

/// A model space chunk's payload: one chunk of the store's objects.
pub(super) fn entities(items: &[Arc<Entity>]) -> Result<Vec<u8>, String> {
    let list = items.iter().map(|e| value(e.as_ref())).collect::<Result<Vec<_>, _>>()?;
    Ok(tokens::encode(&Value::Array(list)))
}

/// A layout chunk's payload.
pub(super) fn layout(l: &Layout) -> Result<Vec<u8>, String> {
    Ok(tokens::encode(&json!([value(l)?, objects(&l.entities)?])))
}

/// The kept file data's payload.
pub(super) fn foreign(f: &Foreign) -> Result<Vec<u8>, String> {
    Ok(tokens::encode(&value(&foreign_v1(f))?))
}

/// The drawing as a .cdy file; `thumbnail` a PNG preview to embed.
pub fn write(d: &Drawing, thumbnail: Option<&[u8]>) -> Result<Vec<u8>, String> {
    super::save::write_full(d, thumbnail.map(<[u8]>::to_vec)).map(|(bytes, _)| bytes)
}

/// A chunk tag as notes name it: its four letters, or "damaged" when they aren't letters (a
/// note printed "a ���� part").
fn tag_name(tag: &[u8; 4]) -> String {
    if tag.iter().all(u8::is_ascii_alphanumeric) { String::from_utf8_lossy(tag).into_owned() } else { "damaged".into() }
}

/// The drawing in a .cdy file, with notes about anything damaged that was left out. Errors only
/// when the file isn't a .cdy file this version can read.
pub fn read(bytes: &[u8]) -> Result<(Drawing, ReadReport), String> {
    let (chunks, indexed) = container::chunks(bytes)?;
    let mut rep = ReadReport::default();
    if !indexed {
        rep.notes.push("the file's index was damaged; its parts were found one by one".into());
    }
    let mut d = Drawing::new_imperial();
    d.layouts.clear();
    let mut model = EntityStore::new();
    let mut have_tables = false;
    for c in chunks {
        let c = match c {
            Read::Ok(c) => c,
            Read::Damaged { tag, why, salvage } => {
                // Model space objects before the damage are kept (one bad spot cost every object in
                // its part: 2 of 14 survived, UI review 0.7.7c).
                if &tag == b"ENTS"
                    && let Some(list) = tokens::salvage_list(&salvage)
                {
                    let items: Vec<Entity> = list.into_iter().filter_map(|item| take::<Entity>(item, "an object", &mut rep.notes)).collect();
                    rep.notes.push(format!(
                        "a model space part was damaged ({why}): {} objects before the damage were recovered, the ones after it left out",
                        items.len()
                    ));
                    if !items.is_empty() {
                        model.push_chunk(items);
                    }
                    continue;
                }
                rep.notes.push(format!("a {} part was left out: {why}", tag_name(&tag)));
                continue;
            }
        };
        if !KNOWN.contains(&&c.tag) {
            // A part added by a newer CADaddy: skipped.
            continue;
        }
        if c.version > V1 {
            return Err(format!(
                "made by a newer CADaddy: its {} data is version {} (this one reads version {V1})",
                String::from_utf8_lossy(&c.tag),
                c.version
            ));
        }
        let what = String::from_utf8_lossy(&c.tag).into_owned();
        if &c.tag == b"THMB" {
            rep.thumbnail = Some(c.data);
            continue;
        }
        if &c.tag == b"MANI" {
            rep.manifest = serde_json::from_slice(&c.data).ok();
            continue;
        }
        let Some(v) = tokens::decode(&c.data) else {
            rep.notes.push(format!("a {what} part couldn't be decoded and was left out"));
            continue;
        };
        match &c.tag {
            b"HEAD" => {
                if let Some(h) = take(v, "the header", &mut rep.notes) {
                    d.header = h;
                }
            }
            b"TABL" => {
                let Some(t) = take::<Tables>(v, "the tables", &mut rep.notes) else { continue };
                let n = &mut rep.notes;
                macro_rules! field {
                    ($f:ident, $name:expr) => {
                        if let Some(x) = take(t.$f, $name, n) {
                            d.$f = x;
                        }
                    };
                }
                field!(layers, "layers");
                field!(linetypes, "linetypes");
                field!(text_styles, "text styles");
                field!(dim_styles, "dimension styles");
                field!(mleader_styles, "multileader styles");
                field!(table_styles, "table styles");
                field!(mline_styles, "multiline styles");
                field!(views, "named views");
                field!(vports, "viewport configurations");
                field!(ucss, "UCSs");
                field!(layer_states, "layer states");
                field!(groups, "groups");
                field!(plot_style_tables, "plot style tables");
                field!(constraints, "constraints");
                field!(parametric, "parameters");
                field!(scales, "the scale list");
                if !t.layer_filters.is_null() {
                    field!(layer_filters, "layer filters");
                }
                if !t.materials.is_null() {
                    field!(materials, "materials");
                }
                if !t.sun.is_null() {
                    field!(sun, "the sun");
                }
                d.field_values = t.field_values.iter().map(|(h, v)| (cadaddy_doc::Handle(*h), v.clone())).collect();
                d.kept_handles = t.kept_handles.iter().take(10_000_000).map(|(k, h)| (k.clone(), *h)).collect();
                d.handseed = t.handseed;
                d.hidden_saved = t.hidden_saved;
                d.hidden_objects = t.hidden.into_iter().map(Handle).collect();
                have_tables = true;
            }
            b"BLKS" => {
                let Value::Array(list) = v else {
                    rep.notes.push("a block part was damaged".into());
                    continue;
                };
                for item in list {
                    let Some([name, b, objs]) = parts::<3>(item) else {
                        rep.notes.push("a block was damaged and left out".into());
                        continue;
                    };
                    let (Some(name), Some(mut b)) = (name.as_str().map(str::to_string), take::<Block>(b, "a block", &mut rep.notes)) else {
                        continue;
                    };
                    b.entities = store_from(objs, &mut rep.notes);
                    d.blocks.insert(name, Arc::new(b));
                }
            }
            b"ENTS" => {
                let Value::Array(list) = v else {
                    rep.notes.push("a model space part was damaged".into());
                    continue;
                };
                // One chunk of the store per chunk of the file, as it was saved (an incremental
                // save then finds them unchanged).
                let items: Vec<Entity> = list.into_iter().filter_map(|item| take::<Entity>(item, "an object", &mut rep.notes)).collect();
                model.push_chunk(items);
            }
            b"LAYO" => {
                let Some([l, objs]) = parts::<2>(v) else {
                    rep.notes.push("a layout was damaged and left out".into());
                    continue;
                };
                if let Some(mut l) = take::<Layout>(l, "a layout", &mut rep.notes) {
                    l.entities = store_from(objs, &mut rep.notes);
                    d.layouts.push(l);
                }
            }
            b"FRGN" => {
                if let Some(f) = take::<ForeignV1>(v, "the kept file data", &mut rep.notes) {
                    d.foreign = Arc::new(foreign_from(f));
                }
            }
            _ => {}
        }
    }
    if !have_tables {
        rep.notes.push("the tables were missing; the standard ones are used".into());
    }
    d.model = model;
    // Handles allocated from here on can't collide with the ones read.
    let blocks = d.blocks.values().flat_map(|b| b.entities.iter());
    let papers = d.layouts.iter().flat_map(|l| l.entities.iter());
    let top = d.model.iter().chain(blocks).chain(papers).map(|e| e.handle.0).max().unwrap_or(0);
    d.handseed = d.handseed.max(top.saturating_add(1));
    Ok((d, rep))
}
