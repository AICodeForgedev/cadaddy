//! DRAWING.SUMMARY and DRAWING.QUERY: what an agent needs to know about a drawing before it
//! reads any geometry, and the objects it then asks for, as handles. Every list is capped and the
//! text index is paged, so a huge drawing answers quickly and in a bounded size.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity, EntityKind, Space};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use serde_json::{Value, json};

use super::super::*;
use crate::{Result, Session};

/// Entries of each table listed (layers, blocks, layouts, title blocks, references).
const MAX_ROWS: usize = 2000;
/// Objects looked at (a hostile drawing's count can't make a summary run long).
const MAX_SCAN: usize = 5_000_000;

pub(super) const SUMMARY_PARAMS: &str = "{sections?: [units|extents|layouts|layers|blocks|texts|titleBlocks|references|dimStyles] (default all), \
     textOffset?, textLimit? (default 200, at most 5000), textContains?} → {units, extents, layouts, layers: [{name, objects, …}], \
     blocks: [{name, references, nested, definitionObjects, xref}], texts: {count, offset, returned, items: [{space, handle, type, layer, text, at, height}]}, \
     titleBlocks: [{space, handle, block, attributes: {TAG: value}}], references: {xrefs: [{name, path, status, found}], images: [{path, found, handles}], \
     underlays: […]}, dimStyles: {current, used: {style: n}}, truncated?}";

pub(super) const QUERY_PARAMS: &str = "{type?: Line|Circle|… (or DXF name), layer?: name or wildcard, space?: all (default)|current|model|<layout>, \
     window?: [[x,y],[x,y]] (crossing; crossing: false = inside), text?: contains (texts, attributes, multileaders), block?: references of this block, \
     limit? (default 1000, at most 100000), offset?} → {count (all matches), returned, offset, handles, objects: [{handle, type, layer, space}]}";

/// INSUNITS names, as the Drawing Units dialog shows them.
fn units_name(code: i64) -> &'static str {
    match code {
        1 => "Inches",
        2 => "Feet",
        3 => "Miles",
        4 => "Millimeters",
        5 => "Centimeters",
        6 => "Meters",
        7 => "Kilometers",
        8 => "Microinches",
        9 => "Mils",
        10 => "Yards",
        11 => "Angstroms",
        12 => "Nanometers",
        13 => "Microns",
        14 => "Decimeters",
        15 => "Dekameters",
        16 => "Hectometers",
        17 => "Gigameters",
        18 => "Astronomical Units",
        19 => "Light Years",
        20 => "Parsecs",
        21 => "US Survey Feet",
        _ => "Unitless",
    }
}

/// Model space and every layout, with their tab names.
fn spaces(d: &Drawing) -> Vec<Space> {
    std::iter::once(Space::Model).chain(d.layouts.iter().map(|l| Space::Paper(l.name.clone()))).collect()
}

fn objects<'a>(d: &'a Drawing, sp: &Space) -> impl Iterator<Item = &'a Arc<Entity>> {
    d.space(sp).into_iter().flat_map(|st| st.iter())
}

/// The readable text of an object: (type, text, insertion point, height, attribute tag).
fn texts_of(e: &Entity) -> Vec<(&'static str, String, Vec3, f64, Option<String>)> {
    match &e.kind {
        EntityKind::Text(t) => vec![("Text", cadaddy_fonts::plain_text(&t.value), t.insert, t.height, None)],
        EntityKind::MText(t) => vec![("MText", cadaddy_fonts::plain_mtext(&t.contents), t.insert, t.height, None)],
        EntityKind::Insert(i) => i
            .attribs
            .iter()
            .filter(|a| !a.invisible)
            .map(|a| ("Attribute", cadaddy_fonts::plain_text(&a.text.value), a.text.insert, a.text.height, Some(a.tag.clone())))
            .collect(),
        EntityKind::MLeader(m) => m.text.iter().map(|t| ("MLeader", cadaddy_fonts::plain_mtext(&t.contents), t.insert, t.height, None)).collect(),
        _ => Vec::new(),
    }
    .into_iter()
    .map(|(ty, t, at, h, tag)| (ty, t.trim().to_string(), at, h, tag))
    .filter(|(_, t, ..)| !t.is_empty())
    .collect()
}

/// Whether the file a drawing names exists: as given when absolute, else beside the drawing.
fn found(path: &str, host: Option<&str>) -> bool {
    let norm = if cfg!(windows) { path.to_string() } else { path.replace('\\', "/") };
    let p = Path::new(norm.trim());
    if p.as_os_str().is_empty() {
        return false;
    }
    if p.is_absolute() {
        return p.is_file();
    }
    let dir = host.and_then(|h| Path::new(h).parent());
    let name = p.file_name().map(Path::new);
    dir.is_some_and(|d| d.join(p).is_file() || name.is_some_and(|n| d.join(n).is_file()))
}

fn want(p: &Value, section: &str) -> bool {
    match p.get("sections").and_then(Value::as_array) {
        Some(a) => a.iter().any(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case(section))),
        None => true,
    }
}

fn usize_param(cmd: &str, p: &Value, key: &str, default: usize, max: usize) -> Result<usize> {
    match p.get(key).filter(|v| !v.is_null()) {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(|n| n.min(max))
            .ok_or_else(|| bad(cmd, format!("`{key}` must be a whole number, 0 or more"))),
    }
}

pub(super) fn run_summary(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "drawing.summary";
    if p.get("sections").is_some_and(|v| !v.is_null() && !v.is_array()) {
        return Err(bad(CMD, "`sections` is a list of section names"));
    }
    let text_offset = usize_param(CMD, p, "textOffset", 0, usize::MAX)?;
    let text_limit = usize_param(CMD, p, "textLimit", 200, 5000)?;
    let contains = str_param(p, "textContains").map(str::to_lowercase);
    let st = s.state()?;
    let d = &st.doc;
    let host = st.path.clone();
    let mut out = serde_json::Map::new();
    let mut truncated = false;

    if want(p, "units") {
        let code = d.header.i64("INSUNITS", 0);
        out.insert(
            "units".into(),
            json!({
                "insunits": code, "name": units_name(code), "lunits": d.header.i64("LUNITS", 2), "luprec": d.header.i64("LUPREC", 4),
                "measurement": if d.header.i64("MEASUREMENT", 0) == 1 { "metric" } else { "imperial" },
            }),
        );
    }
    if want(p, "extents") {
        let mut ext = serde_json::Map::new();
        for sp in spaces(d) {
            let b = crate::cached_extents(d, &sp);
            ext.insert(sp.tab_name().to_string(), if b.is_empty() { Value::Null } else { json!([[b.min.x, b.min.y], [b.max.x, b.max.y]]) });
        }
        out.insert("extents".into(), Value::Object(ext));
    }
    if want(p, "layouts") {
        let rows: Vec<Value> = d
            .layouts
            .iter()
            .take(MAX_ROWS)
            .map(|l| {
                let vps = l.entities.iter().filter(|e| matches!(&e.kind, EntityKind::Viewport(v) if v.id != 1)).count();
                json!({ "name": l.name, "tabOrder": l.tab_order, "paper": l.page.paper, "objects": l.entities.len(), "viewports": vps })
            })
            .collect();
        truncated |= d.layouts.len() > MAX_ROWS;
        out.insert("layouts".into(), Value::Array(rows));
    }
    if want(p, "layers") {
        // Objects per layer: placed in a space, and inside block definitions.
        let mut placed: BTreeMap<String, usize> = BTreeMap::new();
        let mut inside: BTreeMap<String, usize> = BTreeMap::new();
        for sp in spaces(d) {
            for e in objects(d, &sp).take(MAX_SCAN) {
                *placed.entry(e.common.layer.to_ascii_lowercase()).or_default() += 1;
            }
        }
        for b in d.blocks.values() {
            for e in b.entities.iter().take(MAX_SCAN) {
                *inside.entry(e.common.layer.to_ascii_lowercase()).or_default() += 1;
            }
        }
        let cur = d.header.str("CLAYER", "0");
        let rows: Vec<Value> = d
            .layers
            .iter()
            .take(MAX_ROWS)
            .map(|l| {
                let k = l.name.to_ascii_lowercase();
                json!({
                    "name": l.name, "objects": placed.get(&k).copied().unwrap_or(0), "inBlocks": inside.get(&k).copied().unwrap_or(0),
                    "on": l.on, "frozen": l.frozen, "locked": l.locked, "color": l.color.name(), "linetype": l.linetype, "current": l.name.eq_ignore_ascii_case(&cur),
                })
            })
            .collect();
        truncated |= d.layers.len() > MAX_ROWS;
        out.insert("layers".into(), Value::Array(rows));
    }
    if want(p, "blocks") {
        let (all, nested) = super::count::all_counts(d);
        let mut names: Vec<&String> = d.blocks.keys().filter(|k| !k.starts_with('*')).collect();
        names.sort();
        let rows: Vec<Value> = names
            .iter()
            .take(MAX_ROWS)
            .filter_map(|n| d.block(n))
            .map(|b| {
                json!({
                    "name": b.name, "references": all.get(&b.name).copied().unwrap_or(0), "nested": nested.get(&b.name).copied().unwrap_or(0),
                    "definitionObjects": b.entities.len(), "xref": b.xref_path.is_some(),
                })
            })
            .collect();
        truncated |= names.len() > MAX_ROWS;
        out.insert("blocks".into(), Value::Array(rows));
    }
    if want(p, "texts") {
        let mut count = 0usize;
        let mut items = Vec::new();
        'all: for sp in spaces(d) {
            for e in objects(d, &sp).take(MAX_SCAN) {
                for (ty, t, at, h, tag) in texts_of(e) {
                    if contains.as_ref().is_some_and(|c| !t.to_lowercase().contains(c.as_str())) {
                        continue;
                    }
                    if count >= text_offset && items.len() < text_limit {
                        let mut v = json!({ "space": sp.tab_name(), "handle": e.handle.hex(), "type": ty, "layer": e.common.layer, "text": t, "at": [at.x, at.y], "height": h });
                        if let Some(tag) = tag {
                            v["tag"] = json!(tag);
                        }
                        items.push(v);
                    }
                    count += 1;
                    if count >= MAX_SCAN {
                        truncated = true;
                        break 'all;
                    }
                }
            }
        }
        out.insert("texts".into(), json!({ "count": count, "offset": text_offset, "returned": items.len(), "items": items }));
    }
    if want(p, "titleBlocks") {
        // References with attributes on the layouts, and in model space those whose block is
        // named like a title block or border.
        let named = |n: &str| {
            let l = n.to_ascii_lowercase();
            ["title", "tblock", "titleblock", "border", "sheet"].iter().any(|k| l.contains(k)) || l.starts_with("tb")
        };
        let mut rows = Vec::new();
        for sp in spaces(d) {
            for e in objects(d, &sp).take(MAX_SCAN) {
                let EntityKind::Insert(i) = &e.kind else { continue };
                if i.attribs.is_empty() || (sp == Space::Model && !named(&i.block)) {
                    continue;
                }
                let attrs: serde_json::Map<String, Value> =
                    i.attribs.iter().map(|a| (a.tag.clone(), json!(cadaddy_fonts::plain_text(&a.text.value)))).collect();
                rows.push(json!({ "space": sp.tab_name(), "handle": e.handle.hex(), "block": i.block, "attributes": attrs }));
                if rows.len() >= MAX_ROWS {
                    truncated = true;
                    break;
                }
            }
        }
        out.insert("titleBlocks".into(), Value::Array(rows));
    }
    if want(p, "references") {
        let xrefs: Vec<Value> = d
            .blocks
            .values()
            .filter(|b| b.xref_path.is_some() && !b.name.contains('|'))
            .take(MAX_ROWS)
            .map(|b| {
                let path = b.xref_path.clone().unwrap_or_default();
                json!({ "name": b.name, "path": path, "status": b.xref_status.label(), "found": found(&path, host.as_deref()), "overlay": b.xref_overlay })
            })
            .collect();
        let mut images: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut underlays: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for sp in spaces(d) {
            for e in objects(d, &sp).take(MAX_SCAN) {
                let (map, path) = match &e.kind {
                    EntityKind::Image(i) => (&mut images, &i.path),
                    EntityKind::Underlay(u) => (&mut underlays, &u.path),
                    _ => continue,
                };
                if map.len() < MAX_ROWS || map.contains_key(path) {
                    let hs = map.entry(path.clone()).or_default();
                    if hs.len() < 100 {
                        hs.push(e.handle.hex());
                    }
                }
            }
        }
        let list = |m: BTreeMap<String, Vec<String>>| -> Vec<Value> {
            m.into_iter().map(|(path, hs)| json!({ "path": path, "found": found(&path, host.as_deref()), "handles": hs })).collect()
        };
        out.insert("references".into(), json!({ "xrefs": xrefs, "images": list(images), "underlays": list(underlays) }));
    }
    if want(p, "dimStyles") {
        let mut used: BTreeMap<String, usize> = BTreeMap::new();
        for sp in spaces(d) {
            for e in objects(d, &sp).take(MAX_SCAN) {
                if let EntityKind::Dimension(dm) = &e.kind {
                    *used.entry(dm.style.clone()).or_default() += 1;
                }
            }
        }
        out.insert(
            "dimStyles".into(),
            json!({ "current": d.header.str("DIMSTYLE", "Standard"), "defined": d.dim_styles.iter().take(MAX_ROWS).map(|x| x.name.clone()).collect::<Vec<_>>(), "used": used }),
        );
    }
    if truncated {
        out.insert("truncated".into(), json!(true));
    }
    Ok(Value::Object(out))
}

pub(super) fn run_query(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "drawing.query";
    let text = |k: &str| -> Result<Option<String>> {
        match p.get(k).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(v) => v.as_str().map(|t| Some(t.to_string())).ok_or_else(|| bad(CMD, format!("`{k}` must be a string"))),
        }
    };
    let ty = text("type")?.map(|t| t.to_ascii_lowercase());
    let layer = text("layer")?.map(|t| t.to_lowercase());
    let contains = text("text")?.map(|t| t.to_lowercase());
    let block = text("block")?;
    let limit = usize_param(CMD, p, "limit", 1000, 100_000)?;
    let offset = usize_param(CMD, p, "offset", 0, usize::MAX)?;
    let window = match p.get("window").filter(|v| !v.is_null()) {
        None => None,
        Some(w) => {
            let corners: Option<Vec<Vec2>> = w.as_array().filter(|a| a.len() == 2).and_then(|a| a.iter().map(point_value).collect());
            match corners.as_deref() {
                Some([a, b]) => Some(Bounds2::new(*a, *b)),
                _ => return Err(bad(CMD, "`window` must be two corner points [[x0, y0], [x1, y1]]")),
            }
        }
    };
    let crossing = bool_or(p, "crossing", true);
    let st = s.state()?;
    let d = &st.doc;
    let which: Vec<Space> = match text("space")?.as_deref().map(str::to_ascii_lowercase).as_deref() {
        None | Some("all") => spaces(d),
        Some("current") => vec![st.space.clone()],
        Some("model") => vec![Space::Model],
        Some(_) => {
            let n = text("space")?.unwrap_or_default();
            let l = d.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(&n)).ok_or_else(|| bad(CMD, format!("no layout named \"{n}\"")))?;
            vec![Space::Paper(l.name.clone())]
        }
    };
    let block_name = block.as_deref().map(|b| d.block(b).map_or_else(|| b.to_string(), |x| x.name.clone()));
    let tol = window.as_ref().map(crate::select::window_tol).unwrap_or(0.0);
    let mut count = 0usize;
    let mut page = Vec::new();
    let mut handles = Vec::new();
    for sp in &which {
        for e in objects(d, sp).take(MAX_SCAN) {
            if ty.as_ref().is_some_and(|t| e.kind.type_name().to_ascii_lowercase() != *t && e.kind.dxf_name().to_ascii_lowercase() != *t) {
                continue;
            }
            if layer.as_ref().is_some_and(|l| !crate::cmd::qselect::wildcard(l, &e.common.layer.to_lowercase())) {
                continue;
            }
            if let Some(b) = &block_name
                && !matches!(&e.kind, EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(b))
            {
                continue;
            }
            if contains.as_ref().is_some_and(|c| !texts_of(e).iter().any(|(_, t, ..)| t.to_lowercase().contains(c.as_str()))) {
                continue;
            }
            if window.as_ref().is_some_and(|w| !crate::select::in_window(d, e, None, w, crossing, tol)) {
                continue;
            }
            if count >= offset && page.len() < limit {
                handles.push(e.handle.hex());
                page.push(json!({ "handle": e.handle.hex(), "type": e.kind.type_name(), "layer": e.common.layer, "space": sp.tab_name() }));
            }
            count += 1;
        }
    }
    Ok(json!({ "count": count, "returned": page.len(), "offset": offset, "handles": handles, "objects": page }))
}
