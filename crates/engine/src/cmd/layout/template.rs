//! LAYOUT Template and SAveas: layouts brought in from a template drawing (.dwt, .dwg or .dxf),
//! and a layout written out as one.
//!
//! A layout from a template comes with its page setup, its paper space objects and what they
//! refer to (layers, linetypes, styles and blocks, nested ones too). A name the drawing already
//! has keeps the drawing's definition, as in AutoCAD; a layout whose name is taken gets the next
//! free "Name (2)". Everything copied gets new handles.
//!
//! SAveas writes a drawing holding the layout, model space, and the definitions they use (unused
//! ones are left out).

use std::collections::HashMap;

use cadaddy_doc::{Drawing, Entity, EntityKind, EntityStore, Handle, Layout, Space};
use serde_json::{Value, json};

use super::super::clipboard::{adopt, defs_for, merge};
use super::super::file::io;
use super::super::qselect::wildcard;
use super::{bad, layout_name, str_param};
use crate::{Result, Session};

/// The file name a typed or passed path means: AutoCAD adds `.dwt` when there's no extension.
pub(super) fn with_extension(path: &str) -> String {
    let p = path.trim().trim_matches('"');
    if std::path::Path::new(p).extension().is_some() { p.to_string() } else { format!("{p}.dwt") }
}

/// Read a template drawing through the io hooks. A name without an extension tries `.dwt`, `.dwg`
/// and `.dxf` in turn.
pub(super) fn read_template(path: &str) -> Result<Drawing> {
    let hooks = io().ok_or_else(|| bad("layout", "file formats are not available in this build"))?;
    let p = path.trim().trim_matches('"');
    if p.is_empty() {
        return Err(bad("layout", "Requires a template file name."));
    }
    let candidates: Vec<String> = if std::path::Path::new(p).extension().is_some() {
        vec![p.to_string()]
    } else {
        ["dwt", "dwg", "dxf"].iter().map(|e| format!("{p}.{e}")).collect()
    };
    let mut last = None;
    for c in &candidates {
        match read_bytes(c) {
            Ok(bytes) => return (hooks.read)(&bytes, c).map_err(|e| bad("layout", format!("{c}: {e}"))),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| bad("layout", format!("Can't find file \"{p}\"."))))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_bytes(path: &str) -> Result<Vec<u8>> {
    crate::readfile::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => bad("layout", format!("Can't find file \"{path}\".")),
        _ => bad("layout", format!("{path}: {e}")),
    })
}

#[cfg(target_arch = "wasm32")]
fn read_bytes(path: &str) -> Result<Vec<u8>> {
    Err(bad("layout", format!("{path}: paths are not available on the web")))
}

/// The template's layouts in tab order.
pub(super) fn layout_names(src: &Drawing) -> Vec<String> {
    let mut ls: Vec<&Layout> = src.layouts.iter().collect();
    ls.sort_by_key(|l| l.tab_order);
    ls.iter().map(|l| l.name.clone()).collect()
}

/// The template's layouts matching comma-separated names or wildcards (`*` = all), in tab order.
pub(super) fn matching(src: &Drawing, pats: &[String]) -> Vec<String> {
    layout_names(src).into_iter().filter(|n| pats.iter().any(|p| wildcard(p, n))).collect()
}

/// Names from a `layouts` value: an array of names, or a string of comma-separated ones.
fn patterns(v: Option<&Value>) -> Vec<String> {
    let split = |s: &str| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).take(1000).collect::<Vec<_>>();
    match v {
        Some(Value::String(s)) => split(s),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).flat_map(split).take(1000).collect(),
        _ => vec!["*".into()],
    }
}

/// `layout {option: template, path, layouts?: [names] | "a,b" | "*" (default)}`.
pub(super) fn run_template(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").ok_or_else(|| bad("layout", "`path` is required for template"))?;
    let src = read_template(path)?;
    let pats = patterns(p.get("layouts"));
    insert(s, &src, &pats, path)
}

/// Copy the template's layouts matching `pats` into the active drawing.
pub(super) fn insert(s: &mut Session, src: &Drawing, pats: &[String], file: &str) -> Result<Value> {
    let names = matching(src, pats);
    if names.is_empty() {
        return Err(bad("layout", format!("No matching layout found in \"{file}\".")));
    }
    let d = s.doc_mut()?;
    let mut out = Vec::new();
    for name in &names {
        let Some(l) = src.layouts.iter().find(|l| &l.name == name) else { continue };
        let to = copy_layout(d, src, l);
        out.push(json!({ "from": name, "to": to }));
    }
    s.touch();
    for o in &out {
        let (from, to) = (o["from"].as_str().unwrap_or_default(), o["to"].as_str().unwrap_or_default());
        if from == to {
            s.echo(format!("Layout \"{to}\" inserted."));
        } else {
            s.echo(format!("Layout \"{from}\" inserted as \"{to}\"."));
        }
    }
    Ok(json!({ "file": file, "inserted": out }))
}

/// Copy one layout of `src` into `d`; returns its name in `d`.
fn copy_layout(d: &mut Drawing, src: &Drawing, l: &Layout) -> String {
    let ents: Vec<Entity> = l.entities.iter().map(|e| Entity::clone(e)).collect();
    let mut defs = defs_for(src, &ents);
    // Layers only a viewport's freeze or colour override names are kept too, so those settings
    // still mean something.
    for e in &ents {
        if let EntityKind::Viewport(v) = &e.kind {
            for n in v.frozen_layers.iter().chain(v.layer_colors.iter().map(|(n, _)| n)) {
                if !defs.layers.iter().any(|x| x.name.eq_ignore_ascii_case(n))
                    && let Some(x) = src.layers.iter().find(|x| x.name.eq_ignore_ascii_case(n))
                {
                    defs.layers.push(x.clone());
                }
            }
        }
    }
    let renames = merge(d, &defs);
    // A plot style table kept with the template, unless the drawing has one of that name.
    let table = &l.page.plot_style_table;
    if !table.is_empty()
        && !d.plot_style_tables.iter().any(|t| t.name.eq_ignore_ascii_case(table))
        && let Some(t) = src.plot_style_tables.iter().find(|t| t.name.eq_ignore_ascii_case(table))
    {
        d.plot_style_tables.push(t.clone());
    }
    let name = free_name(d, &l.name);
    let mut next_id = super::viewports::next_viewport_id(d);
    let mut store = EntityStore::new();
    let mut handles: HashMap<Handle, Handle> = HashMap::new();
    let mut copied = Vec::with_capacity(ents.len());
    for e in &ents {
        let n = adopt(d, e, &renames);
        handles.insert(e.handle, n.handle);
        d.ensure_layer(&n.common.layer);
        copied.push(n);
    }
    for mut e in copied {
        if let EntityKind::Viewport(v) = &mut e.kind {
            // Viewport 1 is the sheet itself; the others are numbered after the drawing's own.
            if v.id != 1 {
                v.id = next_id;
                next_id = next_id.saturating_add(1);
            }
            v.clip = v.clip.and_then(|h| handles.get(&h).copied());
        }
        store.push(e);
    }
    let order = d.layouts.iter().map(|x| x.tab_order).max().unwrap_or(0).saturating_add(1);
    d.layouts.push(Layout {
        name: name.clone(),
        tab_order: order,
        page: l.page.clone(),
        entities: store,
        view: l.view,
        origin: l.origin,
        pending_setup: false,
    });
    name
}

/// `name`, or the first free "name (2)", "name (3)", … when the drawing already has it.
fn free_name(d: &Drawing, name: &str) -> String {
    let taken = |n: &str| n.eq_ignore_ascii_case("model") || d.layout(n).is_some();
    if !taken(name) {
        return name.to_string();
    }
    (2..=100_000).map(|i| format!("{name} ({i})")).find(|n| !taken(n)).unwrap_or_else(|| format!("{name} ({})", d.handseed))
}

/// `layout {option: saveas, layout?: current, path}`: write the layout as a template.
pub(super) fn run_saveas(s: &mut Session, p: &Value) -> Result<Value> {
    let name = match (str_param(p, "layout").or_else(|| str_param(p, "name")), s.layout_space()) {
        (Some(n), _) => n.to_string(),
        (None, Space::Paper(n)) => n,
        (None, Space::Model) => return Err(bad("layout", "`layout` is required in the Model tab")),
    };
    let name = layout_name("layout", s.doc()?, &name)?;
    let path = with_extension(str_param(p, "path").ok_or_else(|| bad("layout", "`path` is required for saveas"))?);
    let hooks = io().ok_or_else(|| bad("layout", "file formats are not available in this build"))?;
    let t = template_of(s.doc()?, &name)?;
    let bytes = (hooks.write)(&t, &path).map_err(|e| bad("layout", format!("{path}: {e}")))?;
    #[cfg(not(target_arch = "wasm32"))]
    crate::writefile::write(&path, &bytes).map_err(|e| bad("layout", format!("{path}: {e}")))?;
    s.echo(format!("Layout \"{name}\" saved to \"{path}\"."));
    #[cfg(target_arch = "wasm32")]
    return Ok(json!({ "layout": name, "path": path, "bytes": bytes.len(), "data": super::base64_encode(&bytes) }));
    #[cfg(not(target_arch = "wasm32"))]
    Ok(json!({ "layout": name, "path": path, "bytes": bytes.len() }))
}

/// The template drawing SAveas writes: the layout (its only one), model space, the header, and the
/// definitions they use. Objects keep their handles, so viewport clip boundaries and
/// associative dimensions still point at the right objects.
fn template_of(src: &Drawing, name: &str) -> Result<Drawing> {
    let l = src.layouts.iter().find(|l| l.name == name).ok_or_else(|| bad("layout", format!("no layout `{name}`")))?;
    let ents: Vec<Entity> = l.entities.iter().chain(src.model.iter()).map(|e| Entity::clone(e)).collect();
    let mut t = defs_for(src, &ents);
    // The standard records and the current ones the header names come from the drawing; any it
    // lacks from a new drawing's defaults.
    let h = &src.header;
    let current = |k: &str| h.str(k, "");
    let names = ["0", "Defpoints", "ByBlock", "ByLayer", "Continuous", "Standard"].map(String::from);
    let wanted: Vec<String> =
        names.into_iter().chain(["CLAYER", "CELTYPE", "TEXTSTYLE", "DIMSTYLE", "CMLEADERSTYLE", "CTABLESTYLE"].map(current)).collect();
    let want = |n: &str| wanted.iter().any(|w| w.eq_ignore_ascii_case(n));
    let mut base = Drawing::new_imperial();
    base.layouts.clear();
    base.layers = src.layers.iter().filter(|x| want(&x.name)).cloned().chain(base.layers).collect();
    base.linetypes = src.linetypes.iter().filter(|x| want(&x.name)).cloned().chain(base.linetypes).collect();
    base.text_styles = src.text_styles.iter().filter(|x| want(&x.name)).cloned().chain(base.text_styles).collect();
    base.dim_styles = src.dim_styles.iter().filter(|x| want(&x.name)).cloned().chain(base.dim_styles).collect();
    base.mleader_styles = src.mleader_styles.iter().filter(|x| want(&x.name)).cloned().chain(base.mleader_styles).collect();
    base.table_styles = src.table_styles.iter().filter(|x| want(&x.name)).cloned().chain(base.table_styles).collect();
    merge(&mut t, &base);
    for e in &ents {
        if let EntityKind::Viewport(v) = &e.kind {
            for n in v.frozen_layers.iter().chain(v.layer_colors.iter().map(|(n, _)| n)) {
                if !t.layers.iter().any(|x| x.name.eq_ignore_ascii_case(n))
                    && let Some(x) = src.layers.iter().find(|x| x.name.eq_ignore_ascii_case(n))
                {
                    t.layers.push(x.clone());
                }
            }
        }
    }
    t.mline_styles = src.mline_styles.clone();
    t.scales = src.scales.clone();
    t.plot_style_tables = src.plot_style_tables.iter().filter(|x| x.name.eq_ignore_ascii_case(&l.page.plot_style_table)).cloned().collect();
    t.model = src.model.clone();
    t.layouts = vec![Layout { tab_order: 1, ..l.clone() }];
    t.handseed = src.handseed;
    Ok(t)
}
