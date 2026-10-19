//! What a DWG or DXF file wouldn't keep of a drawing that a .cdy file keeps: found by writing
//! the drawing in that format and reading it back, then comparing part by part. Saving into DWG
//! or DXF says what, once per drawing.

use cadaddy_doc::Drawing;
use serde::Serialize;

/// Groups of a kept object's raw data that are the file's bookkeeping, not its content: its
/// handle, owner and reactor brackets, which a writer renumbers and places itself.
const BOOKKEEPING: [i64; 4] = [5, 102, 330, 360];

/// An object as compared: an object of a type CADaddy doesn't model (kept as the file's groups)
/// without its [`BOOKKEEPING`] groups.
fn content(mut v: serde_json::Value) -> serde_json::Value {
    // A viewport's number (DXF group 69) is renumbered by the writer, like a handle: a new
    // drawing's layouts were reported as not kept in DWG (UI review 0.7.7c).
    if v.get("type").and_then(serde_json::Value::as_str) == Some("viewport")
        && let Some(o) = v.as_object_mut()
    {
        o.remove("id");
    }
    if v.get("type").and_then(serde_json::Value::as_str) == Some("unknown")
        && let Some(serde_json::Value::Array(tags)) = v.get_mut("tags")
    {
        tags.retain(|t| !t.get("code").and_then(serde_json::Value::as_i64).is_some_and(|c| BOOKKEEPING.contains(&c)));
    }
    v
}

/// Most objects of one space or block compared one by one: past it, objects spread evenly
/// through it (and their count). What a format drops shows in any few thousand objects; comparing
/// all 167k of a large drawing made its first DWG save take 20 s instead of 3.5 (battle test
/// 0.7.7c).
const SAMPLE: usize = 2000;

/// Every `n`-th of `len` items so that at most [`SAMPLE`] are compared.
fn step(len: usize) -> usize {
    len.div_ceil(SAMPLE).max(1)
}

/// The parts of a drawing compared, each named as the notice names it.
fn parts(d: &Drawing) -> Vec<(&'static str, serde_json::Value)> {
    fn v<T: Serialize>(t: &T) -> serde_json::Value {
        serde_json::to_value(t).unwrap_or_default()
    }
    let objects = |s: &cadaddy_doc::EntityStore| {
        let n = s.len();
        let list = s.iter().step_by(step(n)).map(|e| content(v(&e.kind))).collect::<Vec<_>>();
        serde_json::json!({ "count": n, "objects": list })
    };
    let blocks: Vec<_> = d.blocks.iter().filter(|(_, b)| !b.anonymous).map(|(n, b)| (n.clone(), objects(&b.entities))).collect();
    let layouts: Vec<_> = d.layouts.iter().map(|l| (l.name.clone(), objects(&l.entities))).collect();
    vec![
        ("objects", v(&objects(&d.model))),
        ("object properties", v(&d.model.iter().step_by(step(d.model.len())).map(|e| v(&e.common)).collect::<Vec<_>>())),
        ("layers", v(&d.layers)),
        ("linetypes", v(&d.linetypes)),
        ("text styles", v(&d.text_styles)),
        ("dimension styles", v(&d.dim_styles)),
        ("multileader styles", v(&d.mleader_styles)),
        ("table styles", v(&d.table_styles)),
        ("multiline styles", v(&d.mline_styles)),
        ("blocks", v(&blocks)),
        ("layouts", v(&layouts)),
        ("named views", v(&d.views)),
        ("viewport configurations", v(&d.vports)),
        ("UCSs", v(&d.ucss)),
        ("layer states", v(&d.layer_states)),
        ("groups", v(&d.groups)),
        ("plot style tables", v(&d.plot_style_tables)),
        ("geometric and dimensional constraints", v(&d.constraints)),
        ("parameters", v(&d.parametric)),
        ("the scale list", v(&d.scales)),
    ]
}

/// Deepest nesting compared (model data nests a dozen levels).
const MAX_DEPTH: usize = 64;

/// Whether `b` still holds everything `a` holds: every value of `a` that is set is in `b` (numbers
/// equal to 1 part in 10⁹, the precision a DXF number keeps). Unset values of `a` (null, zero,
/// false, empty) match anything: reading a file fills in derived data (a dimension's block and
/// measurement, a hatch pattern's lines), which loses nothing.
fn kept(a: &serde_json::Value, b: &serde_json::Value, depth: usize) -> bool {
    use serde_json::Value;
    if depth > MAX_DEPTH {
        return a == b;
    }
    match (a, b) {
        (Value::Null, _) | (Value::Bool(false), _) => true,
        (Value::String(x), _) if x.is_empty() => true,
        (Value::Array(x), _) if x.is_empty() => true,
        (Value::Object(x), _) if x.is_empty() => true,
        (Value::Number(x), _) if x.as_f64() == Some(0.0) => true,
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0),
            _ => false,
        },
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| kept(a, b, depth + 1)),
        (Value::Object(x), Value::Object(y)) => {
            x.iter().all(|(k, a)| y.get(k).map_or_else(|| kept(a, &Value::Null, depth + 1), |b| kept(a, b, depth + 1)))
        }
        _ => a == b,
    }
}

/// The parts of `d` a file would lose: written with `write` as `name` and read back with `read`,
/// what no longer holds everything it held. Empty when the file keeps everything (or can't be
/// written or read back: saving reports that itself).
pub fn not_kept(
    d: &Drawing,
    name: &str,
    write: fn(&Drawing, &str) -> Result<Vec<u8>, String>,
    read: fn(&[u8], &str) -> Result<Drawing, String>,
) -> Vec<&'static str> {
    let Ok(bytes) = write(d, name) else { return Vec::new() };
    let Ok(back) = read(&bytes, name) else { return Vec::new() };
    let lost: Vec<&'static str> = parts(d).into_iter().zip(parts(&back)).filter(|((_, a), (_, b))| !kept(a, b, 0)).map(|((n, _), _)| n).collect();
    // Properties are compared object by object: with objects missing they no longer line up.
    if d.model.len() != back.model.len() { lost.into_iter().filter(|n| *n != "object properties").collect() } else { lost }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn derived_data_filled_in_loses_nothing() {
        // A dimension read back with its block and measurement, a hatch with its pattern lines.
        let a = json!({"block": null, "measurement": 0.0, "text": "", "p": {"x": 1.0, "y": 2.0}, "lines": []});
        let b = json!({"block": "*D1", "measurement": 8.0, "text": "", "p": {"x": 1.0000000000001, "y": 2.0}, "lines": [1, 2]});
        assert!(kept(&a, &b, 0));
        // What was set and is gone or different is lost.
        assert!(!kept(&json!({"name": "Walls"}), &json!({}), 0));
        assert!(!kept(&json!({"x": 1.0}), &json!({"x": 1.001}), 0));
        assert!(!kept(&json!([1, 2]), &json!([1]), 0));
        assert!(!kept(&json!(true), &json!(false), 0));
        // Deep values are compared exactly past the depth cap, without recursing further.
        let mut deep = json!(1);
        for _ in 0..200 {
            deep = json!([deep]);
        }
        assert!(kept(&deep, &deep.clone(), 0));
    }

    #[test]
    fn a_file_that_drops_parts_names_them() {
        let mut d = Drawing::new_imperial();
        d.layers.push(cadaddy_doc::Layer { name: "Walls".into(), ..Default::default() });
        // A writer that keeps nothing but what a new drawing has.
        let write: fn(&Drawing, &str) -> Result<Vec<u8>, String> = |_, _| Ok(Vec::new());
        let read: fn(&[u8], &str) -> Result<Drawing, String> = |_, _| Ok(Drawing::new_imperial());
        assert_eq!(not_kept(&d, "x.dwg", write, read), vec!["layers"]);
        assert!(not_kept(&Drawing::new_imperial(), "x.dwg", write, read).is_empty());
        // A file that can't be written or read back: nothing to say here (saving reports it).
        let fails: fn(&Drawing, &str) -> Result<Vec<u8>, String> = |_, _| Err("no".into());
        assert!(not_kept(&d, "x.dwg", fails, read).is_empty());
    }

    #[test]
    fn renumbered_viewports_are_kept() {
        let a = content(json!({"type": "viewport", "id": 3, "width": 10.5}));
        let b = content(json!({"type": "viewport", "id": 2, "width": 10.5}));
        assert!(kept(&a, &b, 0));
        assert!(!kept(&a, &content(json!({"type": "viewport", "id": 3, "width": 9.0})), 0));
    }

    #[test]
    fn kept_objects_ignore_the_files_bookkeeping() {
        let a = json!({"type": "unknown", "tags": [{"code": 8, "value": "0"}, {"code": 5, "value": "3A0"}, {"code": 10, "value": "1.0"}]});
        let b = json!({"type": "unknown", "tags": [{"code": 5, "value": "3A1"}, {"code": 330, "value": "1F"}, {"code": 8, "value": "0"}, {"code": 10, "value": "1.0"}]});
        assert!(kept(&content(a), &content(b), 0));
    }
}
