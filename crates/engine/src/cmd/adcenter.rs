//! ADCENTER (DesignCenter): look into folders and other drawings (DWG, DXF, .cdy) without
//! opening them, list what they define (blocks with previews, layers, linetypes, text and
//! dimension styles, layouts, xrefs), and bring any of it into the current drawing. Definitions
//! are merged without overwriting (a name the drawing has keeps the drawing's), with what they
//! need (a layer's linetype, a block's nested blocks and layers), through the clipboard's
//! `defs_for`, `merge` and `adopt`; a block can be inserted at a point as it comes in.

use std::collections::BTreeMap;

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Insert};
use cadaddy_geom::Vec3;
use serde_json::{Value, json};

use super::clipboard::{adopt, defs_for, merge};
use super::file::io;
use super::{CommandSpec, bad, str_param};
use crate::{Result, Session};

const CMD: &str = "adcenter";
/// Most entries listed from one folder.
const MAX_ENTRIES: usize = 2000;
/// Most points in one block preview.
const MAX_PREVIEW_POINTS: usize = 1500;
/// File types DesignCenter shows.
const DRAWING_EXTS: [&str; 5] = ["dwg", "dxf", "cdy", "dwt", "dws"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("adcenter", "DesignCenter", run)
            .menu(&["Tools", "Palettes", "DesignCenter"])
            .key("Cmd+2")
            .alias(&["dc", "adc", "dcenter"])
            .params(
                "{action?: list (default) | folder | import, path: a drawing (list, import) or a folder (folder), \
                 kind?: blocks|layers|linetypes|textStyles|dimStyles|layouts|xrefs (import), names?: [name] (import), \
                 at?: [x,y] (import a block: insert it there), scale?, rotation? (degrees)} → \
                 list: {blocks: [{name, objects, preview: [[[x,y]…]…] in a unit box}], layers, linetypes, textStyles, dimStyles, layouts, xrefs: [{name, path}]}; \
                 folder: {folders, drawings}; import: {added, kept (already in the drawing), handle?}",
            )
            .enabled(super::always),
    ]
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(CMD, "`path` is required"))?.to_string();
    match str_param(p, "action").unwrap_or("list").to_ascii_lowercase().as_str() {
        "folder" => folder(&path),
        "list" => Ok(listing(&read(&path)?)),
        "import" => import(s, &path, p),
        other => Err(bad(CMD, format!("unknown action \"{other}\" (list, folder or import)"))),
    }
}

/// Another drawing, read without opening it.
fn read(path: &str) -> Result<Drawing> {
    let hooks = io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bytes = crate::readfile::read(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        (hooks.read)(&bytes, path).map_err(|e| bad(CMD, format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = hooks;
        Err(bad(CMD, "paths are not available on the web"))
    }
}

/// The subfolders and drawings of a folder, by name.
fn folder(path: &str) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let rd = std::fs::read_dir(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        let (mut folders, mut drawings) = (Vec::new(), Vec::new());
        for entry in rd.flatten().take(MAX_ENTRIES) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let p = entry.path();
            if p.is_dir() {
                folders.push(name);
            } else if p.extension().is_some_and(|e| DRAWING_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x))) {
                drawings.push(name);
            }
        }
        folders.sort_by_key(|n| n.to_lowercase());
        drawings.sort_by_key(|n| n.to_lowercase());
        Ok(json!({ "path": path, "folders": folders, "drawings": drawings }))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err(bad(CMD, "folders are not available on the web"))
    }
}

/// Whether a block is one a person would insert: not a layout, an xref or a generated block.
fn insertable(name: &str, b: &cadaddy_doc::Block) -> bool {
    !name.starts_with('*') && !b.anonymous && b.xref_path.is_none() && !name.contains('|')
}

/// A block's look as polylines in a unit box (y up), for a preview.
fn preview(d: &Drawing, b: &cadaddy_doc::Block) -> Vec<Vec<[f32; 2]>> {
    let ents: Vec<&Entity> = b.entities.iter().map(|e| &**e).collect();
    let list = cadaddy_render::build_entities(d, ents, &cadaddy_render::Options::default());
    let bounds = list.bounds;
    if bounds.is_empty() {
        return Vec::new();
    }
    let size = bounds.width().max(bounds.height()).max(1e-9);
    let c = bounds.center();
    let mut out = Vec::new();
    let mut n = 0usize;
    for prim in list.prims.iter().filter(|p| matches!(p.kind, cadaddy_render::Kind::Polyline)) {
        let (start, len) = (prim.start as usize, prim.len as usize);
        let Some(pts) = list.verts.get(start..start.saturating_add(len)) else { continue };
        let step = (pts.len() / 200).max(1);
        let line: Vec<[f32; 2]> = pts.iter().step_by(step).map(|v| [((v.x - c.x) / size + 0.5) as f32, ((v.y - c.y) / size + 0.5) as f32]).collect();
        n += line.len();
        if n > MAX_PREVIEW_POINTS {
            break;
        }
        if line.len() >= 2 {
            out.push(line);
        }
    }
    out
}

fn listing(d: &Drawing) -> Value {
    let blocks: Vec<Value> = d
        .blocks
        .iter()
        .filter(|(n, b)| insertable(n, b))
        .map(|(n, b)| json!({ "name": n, "objects": b.entities.len(), "preview": preview(d, b) }))
        .collect();
    let xrefs: Vec<Value> = d.blocks.iter().filter_map(|(n, b)| Some(json!({ "name": n, "path": b.xref_path.clone()? }))).collect();
    fn names(v: Vec<&str>) -> Vec<&str> {
        v.into_iter().filter(|n| !n.contains('|')).collect()
    }
    json!({
        "blocks": blocks,
        "layers": names(d.layers.iter().map(|l| l.name.as_str()).collect()),
        "linetypes": names(d.linetypes.iter().map(|l| l.name.as_str()).filter(|n| !matches!(n.to_ascii_lowercase().as_str(), "bylayer" | "byblock")).collect()),
        "textStyles": names(d.text_styles.iter().map(|l| l.name.as_str()).collect()),
        "dimStyles": names(d.dim_styles.iter().map(|l| l.name.as_str()).collect()),
        "layouts": d.layouts.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        "xrefs": xrefs,
    })
}

/// Records of `src` named in `names` (any case) that exist there.
fn chosen<T: Clone>(all: &[T], names: &[String], name: fn(&T) -> &str) -> Vec<T> {
    all.iter().filter(|t| names.iter().any(|n| n.eq_ignore_ascii_case(name(t)))).cloned().collect()
}

fn import(s: &mut Session, path: &str, p: &Value) -> Result<Value> {
    let src = read(path)?;
    let kind = str_param(p, "kind").ok_or_else(|| bad(CMD, "`kind` is required to import"))?.to_ascii_lowercase();
    let names: Vec<String> = match p.get("names") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(Value::String(n)) => vec![n.clone()],
        _ => return Err(bad(CMD, "`names` is a list of names to import")),
    };
    if names.is_empty() {
        return Err(bad(CMD, "`names` is empty"));
    }
    // What the drawing has before, to report added and kept.
    let had = |d: &Drawing, kind: &str, n: &str| -> bool {
        match kind {
            "layers" => d.layer(n).is_some(),
            "linetypes" => d.linetype(n).is_some(),
            "textstyles" => d.text_style(n).is_some(),
            "dimstyles" => d.dim_style(n).is_some(),
            "blocks" => d.block(n).is_some(),
            "layouts" => d.layouts.iter().any(|l| l.name.eq_ignore_ascii_case(n)),
            _ => d.block(n).is_some(),
        }
    };
    let kind = match kind.trim_end_matches('s') {
        "layer" => "layers",
        "linetype" => "linetypes",
        "textstyle" | "style" => "textstyles",
        "dimstyle" => "dimstyles",
        "block" => "blocks",
        "layout" => "layouts",
        "xref" => "xrefs",
        _ => return Err(bad(CMD, format!("can't import `{kind}` (blocks, layers, linetypes, textStyles, dimStyles, layouts or xrefs)"))),
    };
    let missing: Vec<&String> = names
        .iter()
        .filter(|n| match kind {
            "xrefs" => !src.blocks.iter().any(|(b, blk)| b.eq_ignore_ascii_case(n) && blk.xref_path.is_some()),
            _ => !had(&src, kind, n),
        })
        .collect();
    if let Some(n) = missing.first() {
        return Err(bad(CMD, format!("{path} has no {} \"{n}\"", kind.trim_end_matches('s'))));
    }
    if kind == "xrefs" {
        return import_xrefs(s, path, &src, &names);
    }
    let (added, kept): (Vec<String>, Vec<String>) = {
        let d = s.doc()?;
        names.iter().cloned().partition(|n| !had(d, kind, n))
    };
    let mut d = s.doc()?.clone();
    let mut handle = None;
    match kind {
        "blocks" => {
            // The blocks with what they need, through stand-in references.
            let refs: Vec<Entity> = names
                .iter()
                .map(|n| {
                    let ins = Insert {
                        block: src.blocks.keys().find(|k| k.eq_ignore_ascii_case(n)).cloned().unwrap_or_else(|| n.clone()),
                        insert: Vec3::ZERO,
                        scale: Vec3::new(1.0, 1.0, 1.0),
                        rotation: 0.0,
                        attribs: Vec::new(),
                        cols: 1,
                        rows: 1,
                        col_spacing: 0.0,
                        row_spacing: 0.0,
                        clip: None,
                    };
                    Entity::new(Handle(0), EntityKind::Insert(ins))
                })
                .collect();
            let defs = defs_for(&src, &refs);
            merge(&mut d, &defs);
            if let Some(at) = p.get("at").and_then(super::point_value) {
                let name = d.blocks.keys().find(|k| k.eq_ignore_ascii_case(&names[0])).cloned().unwrap_or_else(|| names[0].clone());
                let scale = p.get("scale").and_then(Value::as_f64).filter(|v| v.is_finite() && *v != 0.0).unwrap_or(1.0);
                let rotation = p.get("rotation").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0).to_radians();
                let ins = Insert {
                    block: name,
                    insert: at.to3(0.0),
                    scale: Vec3::new(scale, scale, scale),
                    rotation,
                    attribs: Vec::new(),
                    cols: 1,
                    rows: 1,
                    col_spacing: 0.0,
                    row_spacing: 0.0,
                    clip: None,
                };
                let space = s.space();
                handle = Some(d.add(&space, Common::default(), EntityKind::Insert(ins)).map_err(|e| bad(CMD, e.to_string()))?);
            }
        }
        "layouts" => {
            for n in &names {
                let Some(l) = src.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(n)) else { continue };
                if d.layouts.iter().any(|o| o.name.eq_ignore_ascii_case(n)) {
                    continue;
                }
                let ents: Vec<Entity> = l.entities.iter().map(|e| (**e).clone()).collect();
                let defs = defs_for(&src, &ents);
                let renames: BTreeMap<String, String> = merge(&mut d, &defs);
                let mut nl = l.clone();
                nl.entities = Default::default();
                nl.tab_order = d.layouts.iter().map(|o| o.tab_order).max().unwrap_or(0).saturating_add(1);
                for e in &ents {
                    nl.entities.push(adopt(&mut d, e, &renames));
                }
                d.layouts.push(nl);
            }
        }
        _ => {
            let mut defs = Drawing::new_imperial();
            defs.layers.clear();
            defs.linetypes.clear();
            defs.text_styles.clear();
            defs.dim_styles.clear();
            match kind {
                "layers" => {
                    defs.layers = chosen(&src.layers, &names, |l| &l.name);
                    let lts: Vec<String> = defs.layers.iter().map(|l| l.linetype.clone()).collect();
                    defs.linetypes = chosen(&src.linetypes, &lts, |l| &l.name);
                }
                "linetypes" => defs.linetypes = chosen(&src.linetypes, &names, |l| &l.name),
                "textstyles" => defs.text_styles = chosen(&src.text_styles, &names, |l| &l.name),
                _ => {
                    defs.dim_styles = chosen(&src.dim_styles, &names, |l| &l.name);
                    let ts: Vec<String> = defs.dim_styles.iter().map(|l| l.text_style.clone()).collect();
                    defs.text_styles = chosen(&src.text_styles, &ts, |l| &l.name);
                }
            }
            // A linetype's text and shapes use text styles.
            let lt_styles: Vec<String> = defs.linetypes.iter().flat_map(|t| t.pattern.iter().filter_map(|p| p.style.clone())).collect();
            let mut more = chosen(&src.text_styles, &lt_styles, |l| &l.name);
            defs.text_styles.append(&mut more);
            merge(&mut d, &defs);
        }
    }
    *s.doc_mut()? = d;
    let mut out = json!({ "added": added, "kept": kept });
    if let Some(h) = handle {
        out["handle"] = json!(h.hex());
    }
    Ok(out)
}

/// Xrefs come in attached to the files they name (a relative path from the other drawing's
/// folder).
fn import_xrefs(s: &mut Session, path: &str, src: &Drawing, names: &[String]) -> Result<Value> {
    let base = std::path::Path::new(path).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let (mut added, mut kept) = (Vec::new(), Vec::new());
    for n in names {
        let Some((name, b)) = src.blocks.iter().find(|(k, b)| k.eq_ignore_ascii_case(n) && b.xref_path.is_some()) else { continue };
        if s.doc()?.block(name).is_some() {
            kept.push(name.clone());
            continue;
        }
        let file = b.xref_path.clone().unwrap_or_default();
        let full = if std::path::Path::new(&file).is_absolute() { file.clone() } else { base.join(&file).to_string_lossy().into_owned() };
        s.execute("xattach", &json!({ "path": full, "name": name }))?;
        added.push(name.clone());
    }
    Ok(json!({ "added": added, "kept": kept }))
}
