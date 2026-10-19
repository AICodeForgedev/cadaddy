//! DATAEXTRACTION / EATTEXT: block references and their attribute values, and other objects
//! with their properties (length, area, layer, colour…), as rows — identical rows counted once,
//! or one row per object with its handle, layer, position and rotation — from this drawing
//! and/or other drawing files; returned, written to a CSV file (with a byte order mark, so
//! spreadsheet programs read it as UTF-8) and/or inserted as a table. The settings can be saved to
//! a `.dxe` file and used again ([`settings`]); a table made by extraction keeps them, and
//! DATAEXTRACTION Update fills it again. Tags match in any case.

mod props;
mod settings;

pub use props::PROPERTIES as EXTRACT_PROPERTIES;
pub(crate) use settings::Settings;

use std::collections::BTreeMap;

use cadaddy_doc::MAX_BLOCK_DEPTH;
use cadaddy_doc::{EntityKind, Insert};
use cadaddy_geom::{Mat3, Vec2};
use serde_json::{Value, json};

use crate::cmd::curves::KW;
use crate::cmd::qselect::wildcard;
use crate::cmd::table::make_table;
use crate::cmd::{CommandSpec, bad, bool_or, point_param, str_param, targets};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// Table size limits (the TABLE command's).
const MAX_TABLE_ROWS: usize = 2000;
const MAX_TABLE_COLS: usize = 200;
/// Attribute columns kept at most.
const MAX_TAGS: usize = 190;

const PARAMS: &str = "{blocks?: \"A*,B\" (block names, wildcards, default *), tags?: attribute tags to include (wildcards, default *), includeBlocks?: bool (default true), types?: [\"Line\", \"Circle\", …] (other object types, as the Properties palette names them; wildcards), properties?: [\"Layer\", \"Length\", \"Area\", …] (columns: Layer, Color, Linetype, Lineweight, Length, Area, Radius, Center X/Y, Start X/Y, End X/Y, Position X/Y, Rotation, Contents, Text height, Closed), handles? (default: everything in model space and every layout), drawings?: [paths] (other drawing files; adds a Drawing column), includeCurrent?: bool (default true), nested?: bool (default true: references inside blocks and arrays too), combine?: bool (default true: identical rows once, with a Count column; false: one row per object with handle, layer, position and rotation), file?: path (.csv for spreadsheets; .txt is tab-separated), at?: [x,y] (insert a table there; it keeps the settings), title?: table title, settings?: path (a .dxe settings file to start from), saveSettings?: path (.dxe), update?: bool (fill the extraction tables again; with handles, those)} → {columns, rows, file?, table?, settings} | {updated}";

pub(super) fn dataextraction_spec() -> CommandSpec {
    CommandSpec::new("dataextraction", "Data Extraction...", run)
        .menu(&["Tools", "Data Extraction..."])
        .alias(&["dx", "-dataextraction"])
        .params(PARAMS)
        .interactive(|_| Ok(Box::new(ExtractM::default())))
}

/// EATTEXT: the older name of the same command.
pub(super) fn eattext_spec() -> CommandSpec {
    CommandSpec::new("eattext", "Data Extraction...", run).params(PARAMS).interactive(|_| Ok(Box::new(ExtractM::default())))
}

/// A cell: a count, a number or text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cell {
    Count(u64),
    Num(f64),
    Text(String),
}

impl Cell {
    pub(crate) fn json(&self) -> Value {
        match self {
            Cell::Count(n) => json!(n),
            Cell::Num(v) => json!(v),
            Cell::Text(t) => json!(t),
        }
    }
    pub(crate) fn text(&self) -> String {
        match self {
            Cell::Count(n) => n.to_string(),
            Cell::Num(v) => {
                let t = format!("{v:.4}");
                let t = if t.contains('.') { t.trim_end_matches('0').trim_end_matches('.').to_string() } else { t };
                if t == "-0" { "0".into() } else { t }
            }
            Cell::Text(t) => t.clone(),
        }
    }
}

/// What to extract (the settings with their lists made ready).
#[derive(Clone, Debug)]
struct Options {
    st: Settings,
    /// The property columns, as headed.
    props: Vec<&'static str>,
}

impl Options {
    fn new(st: Settings) -> Result<Options> {
        st.check()?;
        let props = st.properties.iter().filter_map(|p| props::canonical(p)).fold(Vec::new(), |mut v, p| {
            if !v.contains(&p) {
                v.push(p);
            }
            v
        });
        Ok(Options { st, props })
    }
    fn many_drawings(&self) -> bool {
        !self.st.drawings.is_empty()
    }
}

/// One row's object: a block reference (its block name and attribute values) or another object
/// (its type), with the drawing it is in, the handle of the top-level object holding it, layer,
/// position, rotation (degrees) and the chosen properties.
struct Found {
    block: String,
    drawing: String,
    handle: String,
    layer: String,
    at: Vec2,
    rotation: f64,
    values: Vec<(String, String)>,
    props: Vec<Cell>,
}

/// A spec of comma-separated wildcards matching `text` (any case).
fn matches(spec: &str, text: &str) -> bool {
    spec.split(',').any(|p| wildcard(p.trim(), text))
}

/// The attribute values of `ins`: its own, then the constant ones of its block.
fn values(d: &cadaddy_doc::Drawing, ins: &Insert) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = ins.attribs.iter().map(|a| (a.tag.to_ascii_uppercase(), a.text.value.clone())).collect();
    if let Some(b) = d.block(&ins.block) {
        for e in b.entities.iter() {
            if let EntityKind::AttDef(a) = &e.kind
                && a.constant
                && !v.iter().any(|(t, _)| t.eq_ignore_ascii_case(&a.tag))
            {
                v.push((a.tag.to_ascii_uppercase(), a.text.value.clone()));
            }
        }
    }
    v
}

/// References found at most (MINSERT grids and arrays of nested blocks multiply quickly).
const MAX_FOUND: usize = 200_000;
/// Largest drawing file read for extraction.
const MAX_DRAWING_BYTES: u64 = 512 * 1024 * 1024;

/// A drawing file to extract from, read with the host's file reader.
fn read_drawing(path: &str) -> Result<cadaddy_doc::Drawing> {
    let hooks = crate::cmd::file::io().ok_or_else(|| EngineError::Other("No drawing file reader is available.".into()))?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let meta = std::fs::metadata(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        if meta.len() > MAX_DRAWING_BYTES {
            return Err(EngineError::Other(format!("{path}: the file is too large.")));
        }
        let bytes = crate::readfile::read(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
        (hooks.read)(&bytes, &name).map_err(|e| EngineError::Other(format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = hooks;
        Err(EngineError::Other(format!("{path}: files can't be read in the browser")))
    }
}

/// What to extract from: the current drawing (its listed objects, or everything) and the other
/// drawings named, each with its name for the Drawing column.
fn found(s: &Session, o: &Options) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    let d = s.doc()?;
    if o.st.include_current || o.st.drawings.is_empty() {
        let name = s.state().map(|st| st.title.clone()).unwrap_or_default();
        let ents: Vec<std::sync::Arc<cadaddy_doc::Entity>> = if o.st.handles.is_empty() {
            d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())).cloned().collect()
        } else {
            o.st.handles
                .iter()
                .filter_map(|h| u64::from_str_radix(h.trim(), 16).ok())
                .filter_map(|h| d.entity(cadaddy_doc::Handle(h)).cloned())
                .collect()
        };
        from_drawing(d, &name, &ents, o, &mut out);
    }
    for path in o.st.drawings.iter().take(settings::MAX_LIST) {
        let other = read_drawing(path)?;
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
        let ents: Vec<std::sync::Arc<cadaddy_doc::Entity>> =
            other.model.iter().chain(other.layouts.iter().flat_map(|l| l.entities.iter())).cloned().collect();
        from_drawing(&other, &name, &ents, o, &mut out);
        if out.len() >= MAX_FOUND {
            break;
        }
    }
    Ok(out)
}

fn from_drawing(d: &cadaddy_doc::Drawing, name: &str, ents: &[std::sync::Arc<cadaddy_doc::Entity>], o: &Options, out: &mut Vec<Found>) {
    for e in ents {
        if out.len() >= MAX_FOUND {
            return;
        }
        match &e.kind {
            EntityKind::Insert(_) => {
                if o.st.include_blocks {
                    walk(d, o, name, &e.handle.hex(), e, &e.common.layer, Mat3::IDENTITY, 0, out);
                }
            }
            k if o.st.types.iter().any(|t| matches(t, k.type_name())) => {
                let at = props::position(k);
                let rotation = props::rotation(k);
                let subject = props::Subject { common: &e.common, layer: &e.common.layer, kind: k, at, rotation };
                out.push(Found {
                    block: k.type_name().to_string(),
                    drawing: name.to_string(),
                    handle: e.handle.hex(),
                    layer: e.common.layer.clone(),
                    at,
                    rotation,
                    values: Vec::new(),
                    props: o.props.iter().map(|p| props::value(p, &subject)).collect(),
                });
            }
            _ => {}
        }
    }
}

/// Add block reference `e` (on `layer`, placed by `outer`, held by the top-level object `holder`)
/// and, when asked, the references inside it. Anonymous blocks (associative arrays, dynamic
/// block copies) are always looked into, never listed. A nested reference reports the top-level
/// object holding it, its world position and rotation, and the holder's layer when it is on
/// layer 0.
#[allow(clippy::too_many_arguments)]
fn walk(
    d: &cadaddy_doc::Drawing,
    o: &Options,
    drawing: &str,
    holder: &str,
    e: &cadaddy_doc::Entity,
    layer: &str,
    outer: Mat3,
    depth: usize,
    out: &mut Vec<Found>,
) {
    let EntityKind::Insert(ins) = &e.kind else { return };
    if depth >= MAX_BLOCK_DEPTH || out.len() >= MAX_FOUND {
        return;
    }
    let def = d.block(&ins.block);
    let anonymous = ins.block.starts_with('*') || def.is_some_and(|b| b.anonymous);
    let listed = !anonymous && matches(&o.st.blocks, &ins.block);
    let base = def.map_or(Vec2::new(0.0, 0.0), |b| b.base.xy());
    let cols = ins.cols.clamp(1, 10_000);
    let rows = ins.rows.clamp(1, 10_000);
    for r in 0..rows {
        for c in 0..cols {
            if out.len() >= MAX_FOUND {
                return;
            }
            let off = Vec2::new(ins.col_spacing * f64::from(c), ins.row_spacing * f64::from(r)).rotate(ins.rotation);
            let m = outer.then_before(Mat3::translate(off).then_before(ins.transform(base)));
            if listed {
                let (at, rotation) = (m.apply(base), m.rotation().to_degrees());
                let subject = props::Subject { common: &e.common, layer, kind: &e.kind, at, rotation };
                out.push(Found {
                    block: def.map(|b| b.name.clone()).unwrap_or_else(|| ins.block.clone()),
                    drawing: drawing.to_string(),
                    handle: holder.to_string(),
                    layer: layer.to_string(),
                    at,
                    rotation,
                    values: values(d, ins),
                    props: o.props.iter().map(|p| props::value(p, &subject)).collect(),
                });
            }
            if !(o.st.nested || anonymous) {
                continue;
            }
            let Some(b) = def else { continue };
            for be in b.entities.iter() {
                if !matches!(be.kind, EntityKind::Insert(_)) {
                    continue;
                }
                let inner_layer = if be.common.layer == "0" { layer } else { &be.common.layer };
                walk(d, o, drawing, holder, be, inner_layer, m, depth + 1, out);
            }
        }
    }
}

/// Columns and rows for what was found. Combined, identical rows (same name, drawing, property
/// values and attribute values) are counted once.
fn table(found: &[Found], o: &Options) -> (Vec<String>, Vec<Vec<Cell>>) {
    let mut tags: Vec<String> = Vec::new();
    for f in found {
        for (t, _) in &f.values {
            if tags.len() < MAX_TAGS && matches(&o.st.tags, t) && !tags.contains(t) {
                tags.push(t.clone());
            }
        }
    }
    let attrs = |f: &Found| -> Vec<Cell> {
        tags.iter().map(|t| Cell::Text(f.values.iter().find(|(k, _)| k == t).map(|(_, v)| v.clone()).unwrap_or_default())).collect()
    };
    let many = o.many_drawings();
    let mut columns: Vec<String>;
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    if o.st.combine {
        columns = vec!["Count".into(), "Name".into()];
        if many {
            columns.push("Drawing".into());
        }
        // Rows keyed by name (any case), drawing and values, sorted by them.
        let mut counted: BTreeMap<(String, String, Vec<String>), (Found, Vec<Cell>, u64)> = BTreeMap::new();
        for f in found {
            let a = attrs(f);
            let key = (
                f.block.to_ascii_uppercase(),
                if many { f.drawing.clone() } else { String::new() },
                f.props.iter().chain(&a).map(Cell::text).collect(),
            );
            match counted.get_mut(&key) {
                Some(entry) => entry.2 = entry.2.saturating_add(1),
                None => {
                    let copy = Found {
                        block: f.block.clone(),
                        drawing: f.drawing.clone(),
                        handle: String::new(),
                        layer: String::new(),
                        at: f.at,
                        rotation: f.rotation,
                        values: Vec::new(),
                        props: f.props.clone(),
                    };
                    counted.insert(key, (copy, a, 1));
                }
            }
        }
        for (_, (f, a, n)) in counted {
            let mut r = vec![Cell::Count(n), Cell::Text(f.block)];
            if many {
                r.push(Cell::Text(f.drawing));
            }
            r.extend(f.props);
            r.extend(a);
            rows.push(r);
        }
    } else {
        columns = vec!["Name".into()];
        if many {
            columns.push("Drawing".into());
        }
        columns.extend(["Handle", "Layer", "Position X", "Position Y", "Rotation"].map(String::from));
        let mut sorted: Vec<&Found> = found.iter().collect();
        sorted.sort_by_key(|f| (f.block.to_ascii_uppercase(), f.drawing.clone()));
        for f in sorted {
            let mut r = vec![Cell::Text(f.block.clone())];
            if many {
                r.push(Cell::Text(f.drawing.clone()));
            }
            r.extend([Cell::Text(f.handle.clone()), Cell::Text(f.layer.clone()), Cell::Num(f.at.x), Cell::Num(f.at.y), Cell::Num(f.rotation)]);
            r.extend(f.props.iter().cloned());
            r.extend(attrs(f));
            rows.push(r);
        }
    }
    columns.extend(o.props.iter().map(|p| (*p).to_string()));
    columns.extend(tags);
    (columns, rows)
}

/// `text` as a CSV field: quoted when it holds a separator, quote or line break.
fn csv_field(text: &str, sep: char) -> String {
    if text.contains([sep, '"', '\n', '\r']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_string() }
}

/// The rows as CSV (or tab-separated with `sep` '\t'), header first.
pub(crate) fn csv(columns: &[String], rows: &[Vec<Cell>], sep: char) -> String {
    let line = |cells: Vec<String>| cells.iter().map(|c| csv_field(c, sep)).collect::<Vec<_>>().join(&sep.to_string());
    let mut out = line(columns.to_vec());
    out.push_str("\r\n");
    for r in rows {
        out.push_str(&line(r.iter().map(Cell::text).collect()));
        out.push_str("\r\n");
    }
    out
}

pub(crate) fn write_file(path: &str, text: &str) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::writefile::write(path, text).map_err(|e| EngineError::Other(format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = text;
        Err(EngineError::Other(format!("{path}: files can't be written in the browser")))
    }
}

fn read_file(path: &str) -> Result<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::readfile::read_string(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err(EngineError::Other(format!("{path}: files can't be read in the browser")))
    }
}

/// A table of the rows at `at` (a header row of the column names, an optional title) in table
/// style `style` (default the current one); the caller checks the size against the table limits.
fn build_table(
    s: &Session,
    at: Vec2,
    title: Option<&str>,
    columns: &[String],
    rows: &[Vec<Cell>],
    style: Option<&str>,
) -> Result<cadaddy_doc::Table> {
    let mut st = {
        let d = s.doc()?;
        let cur = style.map(str::to_string).unwrap_or_else(|| d.header.str("CTABLESTYLE", "Standard"));
        d.table_styles.iter().find(|t| t.name.eq_ignore_ascii_case(&cur)).or(d.table_styles.first()).cloned().unwrap_or_default()
    };
    // Sized for the drawing's scale (annotation scale, else DIMSCALE), as symbols are: on a
    // 1/8" = 1'-0" plan the table reads like the rest of the sheet's text.
    let k = crate::cmd::symbol::drawing_scale(s)?;
    if k.is_finite() && k > 0.0 && (k - 1.0).abs() > 1e-12 {
        st.text_height *= k;
        st.margin *= k;
        for c in &mut st.cell_styles {
            c.text_height *= k;
            c.margin_h *= k;
            c.margin_v *= k;
        }
    }
    let cells: Vec<Vec<String>> = rows.iter().map(|r| r.iter().map(Cell::text).collect()).collect();
    let mut t = make_table(&st, at, rows.len(), columns.len(), None, None, title, Some(columns.to_vec()), &cells);
    // Columns as wide as their longest text.
    for (c, w) in t.col_widths.iter_mut().enumerate() {
        let chars = std::iter::once(columns.get(c).map_or(0, |h| h.chars().count()))
            .chain(cells.iter().map(|r| r.get(c).map_or(0, |v| v.chars().count())))
            .max()
            .unwrap_or(0)
            .clamp(1, 80);
        *w = chars as f64 * st.text_height * 0.9 + 2.0 * st.margin + st.text_height;
    }
    Ok(t)
}

/// Insert the rows as a table at `at`; the caller checks the size against the table limits.
pub(crate) fn insert_table(s: &mut Session, at: Vec2, title: Option<&str>, columns: &[String], rows: &[Vec<Cell>]) -> Result<String> {
    let t = build_table(s, at, title, columns, rows, None)?;
    Ok(s.add_entity(EntityKind::Table(t))?.hex())
}

fn check_table_size(columns: &[String], rows: &[Vec<Cell>]) -> Result<()> {
    if rows.len().saturating_add(2) > MAX_TABLE_ROWS || columns.len() > MAX_TABLE_COLS {
        return Err(EngineError::Other(format!("Too much data for a table ({} rows, {} columns).", rows.len(), columns.len())));
    }
    Ok(())
}

/// Extract; write `file` and insert a table at `at` when given. A table made here keeps the
/// settings, so DATAEXTRACTION Update can fill it again.
fn extract(s: &mut Session, o: &Options, file: Option<&str>, at: Option<Vec2>) -> Result<Value> {
    let found = found(s, o)?;
    let (columns, rows) = table(&found, o);
    let mut r = json!({
        "columns": columns,
        "rows": rows.iter().map(|r| r.iter().map(Cell::json).collect::<Vec<_>>()).collect::<Vec<_>>(),
    });
    if at.is_some() {
        check_table_size(&columns, &rows)?;
    }
    if let Some(path) = file {
        let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
        let text = if ext.as_deref() == Some("txt") {
            csv(&columns, &rows, '\t')
        } else {
            // A byte order mark tells spreadsheet programs (Excel among them) the file is UTF-8.
            format!("\u{feff}{}", csv(&columns, &rows, ','))
        };
        write_file(path, &text)?;
        r["file"] = json!(path);
    }
    if let Some(at) = at {
        let h = insert_table(s, at, o.st.title.as_deref(), &columns, &rows)?;
        if let Ok(hh) = u64::from_str_radix(&h, 16).map(cadaddy_doc::Handle) {
            let xd = o.st.clone();
            s.doc_mut()?.modify_entity(hh, |e| e.common.xdata = xd.replace_in(&e.common.xdata))?;
        }
        r["table"] = json!(h);
    }
    Ok(r)
}

/// DATAEXTRACTION Update: tables made by data extraction (`only` these, else all of them in the
/// drawing) are filled again from their settings, in place.
fn update_tables(s: &mut Session, only: Option<Vec<cadaddy_doc::Handle>>) -> Result<Value> {
    let d = s.doc()?;
    let tables: Vec<(cadaddy_doc::Handle, Settings, cadaddy_doc::Table)> = d
        .model
        .iter()
        .chain(d.layouts.iter().flat_map(|l| l.entities.iter()))
        .filter(|e| only.as_ref().is_none_or(|hs| hs.contains(&e.handle)))
        .filter_map(|e| match &e.kind {
            EntityKind::Table(t) => Settings::from_xdata(&e.common.xdata).map(|st| (e.handle, st, t.clone())),
            _ => None,
        })
        .collect();
    if only.is_some() && tables.is_empty() {
        return Err(EngineError::Other("The selected objects are not data extraction tables.".into()));
    }
    let mut updated = Vec::new();
    for (h, st, old) in tables {
        let o = Options::new(st)?;
        let found = found(s, &o)?;
        let (columns, rows) = table(&found, &o);
        check_table_size(&columns, &rows)?;
        let t = build_table(s, old.insert.xy(), o.st.title.as_deref(), &columns, &rows, Some(&old.style))?;
        let t = cadaddy_doc::Table { insert: old.insert, ..t };
        s.doc_mut()?.modify_entity(h, |e| e.kind = EntityKind::Table(t))?;
        updated.push(h.hex());
    }
    Ok(json!({ "updated": updated }))
}

fn str_list(p: &Value, key: &str) -> Result<Option<Vec<String>>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) => Ok(Some(t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())),
        Some(Value::Array(a)) if a.len() <= settings::MAX_LIST => a
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad("dataextraction", format!("`{key}` is a list of names"))))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        Some(_) => Err(bad("dataextraction", format!("`{key}` is a list of names"))),
    }
}

/// The settings a call asks for: those of the `settings` file, changed by the other parameters.
fn settings_of(s: &Session, p: &Value) -> Result<Settings> {
    let mut st = match str_param(p, "settings").filter(|f| !f.trim().is_empty()) {
        Some(path) => Settings::parse(&read_file(path)?)?,
        None => Settings::default(),
    };
    if let Some(b) = str_param(p, "blocks") {
        st.blocks = if b.trim().is_empty() { "*".into() } else { b.to_string() };
    }
    if let Some(t) = str_param(p, "tags") {
        st.tags = if t.trim().is_empty() { "*".into() } else { t.to_string() };
    }
    if p.get("combine").is_some() {
        st.combine = bool_or(p, "combine", true);
    }
    if p.get("nested").is_some() {
        st.nested = bool_or(p, "nested", true);
    }
    if p.get("includeBlocks").is_some() {
        st.include_blocks = bool_or(p, "includeBlocks", true);
    }
    if p.get("includeCurrent").is_some() {
        st.include_current = bool_or(p, "includeCurrent", true);
    }
    if let Some(v) = str_list(p, "types")? {
        st.types = v;
    }
    if let Some(v) = str_list(p, "properties")? {
        st.properties = v;
    }
    if let Some(v) = str_list(p, "drawings")? {
        st.drawings = v;
    }
    if p.get("handles").is_some() || p.get("handle").is_some() {
        st.handles = targets(s, p)?.iter().map(|h| h.hex()).collect();
    }
    if let Some(f) = str_param(p, "file") {
        st.file = Some(f.to_string()).filter(|f| !f.trim().is_empty());
    }
    if let Some(t) = str_param(p, "title") {
        st.title = Some(t.to_string());
    }
    st.check()?;
    Ok(st)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    if bool_or(p, "update", false) {
        let only = if p.get("handles").is_some() || p.get("handle").is_some() { Some(targets(s, p)?) } else { None };
        return update_tables(s, only);
    }
    let st = settings_of(s, p)?;
    if let Some(path) = str_param(p, "saveSettings").filter(|f| !f.trim().is_empty()) {
        write_file(path, &st.to_text())?;
    }
    let at = match p.get("at") {
        None | Some(Value::Null) => None,
        Some(_) => Some(point_param(p, "at").ok_or_else(|| bad("dataextraction", "`at` must be a point [x, y]"))?),
    };
    let file = st.file.clone();
    let o = Options::new(st)?;
    let mut r = extract(s, &o, file.as_deref(), at)?;
    r["settings"] = serde_json::to_value(&o.st).unwrap_or(Value::Null);
    Ok(r)
}

// ---------------- command line ----------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Stage {
    #[default]
    Blocks,
    Combine,
    Output,
    Point,
    File,
}

/// DATAEXTRACTION at the command line: blocks, combine, then a table and/or a file.
#[derive(Default)]
struct ExtractM {
    stage: Stage,
    blocks: Option<String>,
    combine: bool,
    table: bool,
    file: bool,
    at: Option<Vec2>,
}

impl ExtractM {
    fn options(&self) -> Result<Options> {
        Options::new(Settings { blocks: self.blocks.clone().unwrap_or_else(|| "*".into()), combine: self.combine, ..Settings::default() })
    }

    /// The default file name: the drawing's, with a .csv extension.
    fn default_file(s: &Session) -> String {
        let Ok(st) = s.state() else { return "extraction.csv".into() };
        let base = st.path.clone().unwrap_or_else(|| st.title.clone());
        std::path::Path::new(&base).with_extension("csv").to_string_lossy().into_owned()
    }

    fn finish(&mut self, s: &mut Session, file: Option<&str>) -> Result<Step> {
        let r = extract(s, &self.options()?, file, self.at)?;
        let n = r["rows"].as_array().map_or(0, Vec::len);
        s.echo(format!("{n} row{} extracted.", if n == 1 { "" } else { "s" }));
        if let Some(f) = r["file"].as_str() {
            s.echo(format!("Written to {f}."));
        }
        Ok(Step::Done)
    }

    fn after_point(&mut self, s: &mut Session) -> Result<Step> {
        if self.file {
            self.stage = Stage::File;
            Ok(Step::Continue)
        } else {
            self.finish(s, None)
        }
    }
}

impl Interactive for ExtractM {
    fn name(&self) -> &'static str {
        "DATAEXTRACTION"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.stage {
            Stage::Blocks => Prompt::new("Enter block names to extract", Accept::TEXT).kw(&["UPdate tables"]).default("*"),
            Stage::Combine => Prompt::new("Combine identical rows?", KW).kw(&["Yes", "No"]).default("Yes"),
            Stage::Output => Prompt::new("Where should the data go?", KW).kw(&["Table", "File", "Both"]).default("Table"),
            Stage::Point => Prompt::new("Specify insertion point of the table", Accept::POINT),
            Stage::File => Prompt::new("Enter file name", Accept::TEXT).default(Self::default_file(s)),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.stage {
            Stage::Blocks => {
                if let Input::Keyword(_) = &i {
                    let r = update_tables(s, None)?;
                    let n = r["updated"].as_array().map_or(0, Vec::len);
                    s.echo(format!("{n} data extraction table{} updated.", if n == 1 { "" } else { "s" }));
                    return Ok(Step::Done);
                }
                let spec = match &i {
                    Input::Text(t) if !t.trim().is_empty() => t.trim().to_string(),
                    Input::Text(_) | Input::Enter => "*".into(),
                    _ => return Ok(Step::Continue),
                };
                self.blocks = Some(spec);
                self.stage = Stage::Combine;
            }
            Stage::Combine => {
                self.combine = match &i {
                    Input::Keyword(k) if k == "No" => false,
                    Input::Keyword(_) | Input::Enter => true,
                    _ => return Err(EngineError::Other("Invalid option keyword.".into())),
                };
                self.stage = Stage::Output;
            }
            Stage::Output => {
                (self.table, self.file) = match &i {
                    Input::Keyword(k) if k == "File" => (false, true),
                    Input::Keyword(k) if k == "Both" => (true, true),
                    Input::Keyword(_) | Input::Enter => (true, false),
                    _ => return Err(EngineError::Other("Invalid option keyword.".into())),
                };
                self.stage = if self.table { Stage::Point } else { Stage::File };
            }
            Stage::Point => match i {
                Input::Point(p) => {
                    self.at = Some(p);
                    return self.after_point(s);
                }
                Input::Enter => return Ok(Step::Cancel),
                _ => {}
            },
            Stage::File => {
                let f = match &i {
                    Input::Text(t) if !t.trim().is_empty() => t.trim().to_string(),
                    Input::Text(_) | Input::Enter => Self::default_file(s),
                    _ => return Ok(Step::Continue),
                };
                return self.finish(s, Some(&f));
            }
        }
        Ok(Step::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_separators_quotes_and_line_breaks() {
        assert_eq!(csv_field("plain", ','), "plain");
        assert_eq!(csv_field("a,b", ','), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\"", ','), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("a,b", '\t'), "a,b");
        assert_eq!(Cell::Num(-0.00001).text(), "0");
        assert_eq!(Cell::Num(2.5).text(), "2.5");
    }
}
