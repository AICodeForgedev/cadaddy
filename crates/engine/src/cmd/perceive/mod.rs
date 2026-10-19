//! Perception for agents: compact summaries of a plan drawing an agent reasons over instead of
//! raw entities. `plan.layers` (per layer: counts, line pairs, an optional PNG crop),
//! `plan.walls` (parallel face pairs a wall's thickness apart), `plan.openings` (door swings,
//! window and door blocks), `plan.rooms` (the closed outline around each room name, with its
//! number), `plan.notes` (ceiling heights, levels, scales, finish codes from the text) and
//! `plan.sheets` (title blocks and sheet names). Every list is paged (`offset`, `limit`) and every
//! item carries a confidence from 0 to 1. Distances are drawing units; the wall and door size
//! limits default to common building sizes in the drawing's units.

pub(crate) mod geometry;

use cadaddy_doc::{Drawing, Entity, EntityKind, Space};
use cadaddy_geom::{Bounds2, Vec2};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Items a page holds by default and at most.
const PAGE: usize = 20;
const MAX_PAGE: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("plan.layers", "Plan Layers", run_layers)
            .params("{offset?, limit?, crops?: folder (writes a PNG of each layer's objects there)} → {items: [{layer, objects, counts: {type: n}, segments, length, pairs, pairSpacing, extents, crop?}], total, next?}")
            .noundo(),
        CommandSpec::new("plan.walls", "Plan Walls", geometry::run_walls)
            .params("{layers? (patterns), minThickness?, maxThickness? (default 2½\"–20\" in drawing units), minLength?, offset?, limit?} → {items: [{layer, centerline: [[x,y],[x,y]], thickness, length, confidence}], total, next?}")
            .noundo(),
        CommandSpec::new("plan.openings", "Plan Openings", geometry::run_openings)
            .params("{offset?, limit?} → {items: [{kind: door|window, at, width, layer, handle, swing?, confidence}], total, next?}")
            .noundo(),
        CommandSpec::new("plan.rooms", "Plan Rooms", geometry::run_rooms)
            .params("{offset?, limit?} → {items: [{name, number?, at, area, perimeter, outline: [[x,y]…], confidence}], total, next?}: the closed outline around each room name")
            .noundo(),
        CommandSpec::new("plan.notes", "Plan Notes", run_notes)
            .params("{kind?: ceiling|level|scale|finish, offset?, limit?} → {items: [{kind, text, value, at, handle, confidence}], total, next?}")
            .noundo(),
        CommandSpec::new("plan.sheets", "Plan Sheets", run_sheets)
            .params("{offset?, limit?} → {items: [{layout, number?, title?, from: attribute|text|layout, confidence}], total, next?}")
            .noundo(),
    ]
}

/// A page of `items`: `{items, total, offset, next?}`.
pub(crate) fn page(items: Vec<Value>, p: &Value) -> Value {
    let total = items.len();
    let offset = p.get("offset").and_then(Value::as_u64).map_or(0, |o| usize::try_from(o).unwrap_or(usize::MAX)).min(total);
    let limit = p.get("limit").and_then(Value::as_u64).map_or(PAGE, |l| usize::try_from(l).unwrap_or(MAX_PAGE)).clamp(1, MAX_PAGE);
    let end = offset.saturating_add(limit).min(total);
    let mut out = json!({ "items": items[offset..end].to_vec(), "total": total, "offset": offset });
    if end < total {
        out["next"] = json!(end);
    }
    out
}

/// Drawing units per inch (INSUNITS; unitless: inches unless the drawing is metric).
pub(crate) fn units_per_inch(d: &Drawing) -> f64 {
    match d.header.i64("INSUNITS", 0) {
        1 => 1.0,
        2 => 1.0 / 12.0,
        4 => 25.4,
        5 => 2.54,
        6 => 0.0254,
        _ if d.header.i64("MEASUREMENT", 0) == 1 => 25.4,
        _ => 1.0,
    }
}

pub(crate) fn round(v: f64) -> f64 {
    if v.is_finite() { (v * 1000.0).round() / 1000.0 } else { 0.0 }
}

pub(crate) fn pt(p: Vec2) -> Value {
    json!([round(p.x), round(p.y)])
}

/// The current space's objects.
pub(crate) fn objects(s: &Session) -> Result<(&Drawing, Vec<&std::sync::Arc<Entity>>)> {
    let d = s.doc()?;
    let space = s.space();
    Ok((d, d.space(&space).map(|st| st.iter().collect()).unwrap_or_default()))
}

// ---------------- plan.layers ----------------

fn run_layers(s: &mut Session, p: &Value) -> Result<Value> {
    let (d, ents) = objects(s)?;
    let segs = geometry::segments(&ents);
    let pairs = geometry::pairs(d, &segs, &geometry::Limits::default_for(d));
    let mut by: std::collections::BTreeMap<String, (usize, std::collections::BTreeMap<&'static str, usize>, Bounds2)> =
        std::collections::BTreeMap::new();
    for e in &ents {
        let entry = by.entry(e.common.layer.clone()).or_insert((0, std::collections::BTreeMap::new(), Bounds2::EMPTY));
        entry.0 += 1;
        *entry.1.entry(e.kind.type_name()).or_default() += 1;
        entry.2 = entry.2.union(&cadaddy_doc::entity_bounds(d, e, 0));
    }
    let crops = str_param(p, "crops").filter(|c| !c.trim().is_empty()).map(str::to_string);
    let mut items = Vec::new();
    let mut layers: Vec<_> = by.into_iter().collect();
    // Busiest layers first.
    layers.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
    for (i, (layer, (n, counts, ext))) in layers.into_iter().enumerate() {
        let mine: Vec<&geometry::Seg> = segs.iter().filter(|g| g.layer == layer).collect();
        let length: f64 = mine.iter().map(|g| g.a.dist(g.b)).sum();
        let lp: Vec<f64> = pairs.iter().filter(|w| w.layer == layer).map(|w| w.thickness).collect();
        let spacing = median(&lp);
        let mut v = json!({
            "layer": layer, "objects": n, "counts": counts, "segments": mine.len(), "length": round(length),
            "pairs": lp.len(), "pairSpacing": spacing.map(round),
            "extents": if ext.is_empty() { Value::Null } else { json!([pt(ext.min), pt(ext.max)]) },
        });
        if let Some(dir) = &crops
            && i < MAX_PAGE
            && let Some(path) = crop(d, &ents, &layer, ext, dir, i)?
        {
            v["crop"] = json!(path);
        }
        items.push(v);
    }
    Ok(page(items, p))
}

fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied()
}

/// A 256-pixel PNG of `layer`'s objects, written into `dir`; its path.
fn crop(d: &Drawing, ents: &[&std::sync::Arc<Entity>], layer: &str, ext: Bounds2, dir: &str, i: usize) -> Result<Option<String>> {
    if ext.is_empty() {
        return Ok(None);
    }
    let mine: Vec<&Entity> = ents.iter().filter(|e| e.common.layer == layer).map(|e| e.as_ref()).collect();
    let list = cadaddy_render::build_entities(d, mine, &cadaddy_render::Options::default());
    let view = cadaddy_render::raster::View::fit(&ext, 256, 256, 0.05);
    let ro = cadaddy_render::raster::RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 1.0, px_per_mm: 0.0, antialias: true };
    let Some(png) = cadaddy_render::raster::render_png(&list, &view, &ro) else { return Ok(None) };
    let safe: String = layer.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
    let path = std::path::Path::new(dir).join(format!("{i:03}-{safe}.png"));
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::create_dir_all(dir).map_err(|e| crate::EngineError::Other(format!("{dir}: {e}")))?;
        std::fs::write(&path, png).map_err(|e| crate::EngineError::Other(format!("{}: {e}", path.display())))?;
        Ok(Some(path.to_string_lossy().into_owned()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (png, path);
        Ok(None)
    }
}

// ---------------- text ----------------

/// The texts of the space: (handle, text as shown, where, height).
pub(crate) fn texts(ents: &[&std::sync::Arc<Entity>]) -> Vec<(cadaddy_doc::Handle, String, Vec2, f64)> {
    let mut out = Vec::new();
    for e in ents {
        match &e.kind {
            EntityKind::Text(t) => out.push((e.handle, t.value.trim().to_string(), t.justify_point(), t.height)),
            EntityKind::MText(t) => {
                out.push((e.handle, cadaddy_fonts::plain_text(&t.contents).split_whitespace().collect::<Vec<_>>().join(" "), t.insert.xy(), t.height))
            }
            _ => {}
        }
    }
    out
}

/// A feet-and-inches height in `s` (`9'-0"`, `8'6"`, `10'`, `96"`), in inches.
pub(crate) fn feet_inches(s: &str) -> Option<f64> {
    let chars: Vec<char> = s.chars().collect();
    for start in 0..chars.len() {
        if !chars[start].is_ascii_digit() || (start > 0 && (chars[start - 1].is_ascii_digit() || chars[start - 1] == '.')) {
            continue;
        }
        let mut i = start;
        let num = |i: &mut usize| -> Option<f64> {
            let b = *i;
            while *i < chars.len() && (chars[*i].is_ascii_digit() || chars[*i] == '.') {
                *i += 1;
            }
            chars[b..*i].iter().collect::<String>().parse().ok()
        };
        let a = num(&mut i)?;
        match chars.get(i) {
            Some('\'') => {
                i += 1;
                while i < chars.len() && matches!(chars[i], '-' | ' ') {
                    i += 1;
                }
                let inches = if chars.get(i).is_some_and(char::is_ascii_digit) { num(&mut i).unwrap_or(0.0) } else { 0.0 };
                return Some(a * 12.0 + inches);
            }
            Some('"') => return Some(a),
            _ => continue,
        }
    }
    None
}

/// Finish code tokens: 2–4 capital letters, a dash, 1–2 digits (`CPT-1`, `VCT-2`, `ACT-1`).
pub(crate) fn finish_codes(t: &str) -> Vec<String> {
    t.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '/'))
        .filter(|w| {
            let Some((a, b)) = w.split_once('-') else { return false };
            (2..=4).contains(&a.len())
                && a.chars().all(|c| c.is_ascii_uppercase())
                && (1..=2).contains(&b.len())
                && b.chars().all(|c| c.is_ascii_digit())
        })
        .map(str::to_string)
        .collect()
}

fn run_notes(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, ents) = objects(s)?;
    let want = str_param(p, "kind").map(str::to_ascii_lowercase);
    let mut items = Vec::new();
    for (h, text, at, _) in texts(&ents) {
        let up = text.to_ascii_uppercase();
        let mut add = |kind: &str, value: Value, confidence: f64| {
            if want.as_deref().is_none_or(|w| w == kind) {
                items.push(json!({"kind": kind, "text": text, "value": value, "at": pt(at), "handle": h.hex(), "confidence": confidence}));
            }
        };
        if (up.contains("CLG") || up.contains("CEILING") || (up.contains("AFF") && up.contains('\'')))
            && let Some(v) = feet_inches(&up)
        {
            add("ceiling", json!(round(v)), if up.contains("CLG") || up.contains("CEILING") { 0.9 } else { 0.6 });
        }
        if up.starts_with("LEVEL") || up.starts_with("FLOOR") || up.contains("EL.") || up.contains("ELEV") || up.contains("T.O.") {
            let v = feet_inches(&up).map(|v| json!(round(v))).unwrap_or_else(|| json!(text));
            add("level", v, if up.contains("EL") { 0.8 } else { 0.6 });
        }
        if up.contains("SCALE") && (up.contains('=') || up.contains(':')) {
            let v = up.split_once("SCALE").map(|(_, r)| r.trim_start_matches([':', ' ']).trim().to_string()).unwrap_or_default();
            add("scale", json!(v), if up.contains('=') { 0.9 } else { 0.7 });
        } else if up.contains("NTS") || up.contains("NOT TO SCALE") {
            add("scale", json!("NTS"), 0.8);
        }
        for code in finish_codes(&text) {
            add("finish", json!(code), 0.7);
        }
    }
    Ok(page(items, p))
}

// ---------------- plan.sheets ----------------

/// A sheet number: one or two letters, maybe a dash, then digits with maybe a dotted part
/// (`A101`, `E-1.1`, `M2.01`).
fn sheet_number(t: &str) -> bool {
    let t = t.trim();
    let letters = t.chars().take_while(char::is_ascii_uppercase).count();
    let rest = t[letters..].trim_start_matches('-');
    (1..=2).contains(&letters)
        && !rest.is_empty()
        && rest.len() <= 6
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn run_sheets(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let mut spaces: Vec<(String, Space)> = d.layouts.iter().map(|l| (l.name.clone(), Space::Paper(l.name.clone()))).collect();
    spaces.push(("Model".into(), Space::Model));
    let mut items = Vec::new();
    for (name, space) in spaces {
        let Some(store) = d.space(&space) else { continue };
        let ents: Vec<&std::sync::Arc<Entity>> = store.iter().collect();
        let (mut number, mut title, mut from, mut conf) = (None::<String>, None::<String>, "layout", 0.3);
        // A title block: a block reference with attributes naming the sheet.
        for e in &ents {
            let EntityKind::Insert(i) = &e.kind else { continue };
            let named_tb = {
                let b = i.block.to_ascii_uppercase();
                b.contains("TITLE") || b.contains("TB") || b.contains("BORDER")
            };
            if i.attribs.len() < 2 && !named_tb {
                continue;
            }
            for a in &i.attribs {
                let tag = a.tag.to_ascii_uppercase();
                let v = a.text.value.trim();
                if v.is_empty() {
                    continue;
                }
                if number.is_none()
                    && (tag.contains("SHEET") && (tag.contains("NO") || tag.contains("NUM"))
                        || tag == "SHEET"
                        || tag.contains("DWG")
                        || tag == "NUMBER"
                        || tag == "SHT")
                {
                    number = Some(v.to_string());
                }
                if title.is_none() && (tag.contains("TITLE") || tag == "NAME" || tag.contains("SHEET_NAME") || tag.contains("SHEETNAME")) {
                    title = Some(v.to_string());
                }
            }
            if number.is_some() || title.is_some() {
                from = "attribute";
                conf = if number.is_some() && title.is_some() { 0.95 } else { 0.8 };
                break;
            }
        }
        // Else the largest text that reads as a sheet number.
        if number.is_none() {
            let mut best: Option<(f64, String)> = None;
            for (_, t, _, h) in texts(&ents) {
                if sheet_number(&t) && best.as_ref().is_none_or(|b| h > b.0) {
                    best = Some((h, t));
                }
            }
            if let Some((_, t)) = best {
                number = Some(t);
                from = "text";
                conf = 0.6;
            }
        }
        if number.is_none() && title.is_none() && space == Space::Model {
            continue;
        }
        if number.is_none() && sheet_number(&name) {
            number = Some(name.clone());
        }
        items.push(json!({"layout": name, "number": number, "title": title, "from": from, "confidence": conf}));
    }
    Ok(page(items, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_codes_and_sheet_numbers_parse() {
        assert_eq!(feet_inches("CLG 9'-0\" AFF"), Some(108.0));
        assert_eq!(feet_inches("ACT @ 8'6\""), Some(102.0));
        assert_eq!(feet_inches("EL. 100'"), Some(1200.0));
        assert_eq!(feet_inches("96\" AFF"), Some(96.0));
        assert_eq!(feet_inches("ROOM 101"), None);
        assert_eq!(finish_codes("FLOOR: CPT-1, BASE RB-2 (WALLS PT-12)"), ["CPT-1", "RB-2", "PT-12"]);
        assert!(finish_codes("SEE A-101").is_empty());
        for t in ["A101", "E-1.1", "M2.01", "FP1"] {
            assert!(sheet_number(t), "{t}");
        }
        for t in ["ROOM", "101", "ABC1", "A-", "A101X"] {
            assert!(!sheet_number(t), "{t}");
        }
    }
}
