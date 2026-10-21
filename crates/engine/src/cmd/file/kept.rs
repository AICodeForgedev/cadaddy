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
    let blocks: Vec<_> = d.blocks.iter().filter(|(_, b)| !b.anonymous).map(|(n, b)| (n.clone(), block_head(b), objects(&b.entities))).collect();
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

/// A block definition's own properties (base point, description, insert units, explodability…),
/// without its objects (compared one by one) and the session's xref load state. A DWG older than
/// 2007 has no insert units or explodability (savefuzz: they were lost unreported).
fn block_head(b: &cadaddy_doc::Block) -> serde_json::Value {
    let mut v = serde_json::to_value(b).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("xrefStatus");
    }
    v
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
        // A false that came back true is a change: properties that are true by default (a block's
        // explodability, a layer's on and plot, an object's visibility) set false (savefuzz).
        (Value::Bool(false), Value::Bool(true)) => false,
        (Value::Null, _) | (Value::Bool(false), _) => true,
        (Value::String(x), _) if x.is_empty() => true,
        (Value::Array(x), _) if x.is_empty() => true,
        (Value::Object(x), _) if x.is_empty() => true,
        (Value::Number(x), _) if x.as_f64() == Some(0.0) => true,
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            // Relative only: a drawing in tiny units (or a thickness of 1e-10 a format drops) is
            // compared as closely as any other (savefuzz found 1e-10 thicknesses dropped unreported).
            (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * x.abs().max(y.abs()),
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
    lost_parts(d, &opened(back))
}

/// The parts of `d` that `back` (`d` written and read back) no longer holds.
fn lost_parts(d: &Drawing, back: &Drawing) -> Vec<&'static str> {
    let lost: Vec<&'static str> = parts(d).into_iter().zip(parts(back)).filter(|((_, a), (_, b))| !kept(a, b, 0)).map(|((n, _), _)| n).collect();
    // Properties are compared object by object: with objects missing they no longer line up.
    if d.model.len() != back.model.len() { lost.into_iter().filter(|n| *n != "object properties").collect() } else { lost }
}

/// One thing a DWG or DXF file wouldn't keep, as the exchange notice lists it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Loss {
    /// What is lost, in words: "mesh objects", "line thickness", "object transparency", "layers".
    pub feature: String,
    /// `dropped` (gone from the file) or `changed` (in it, but not as it was).
    pub what: &'static str,
    /// How many objects or records.
    pub count: usize,
    /// The objects concerned (hex handles), at most [`MAX_LISTED`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub handles: Vec<String>,
    /// The table records or blocks concerned, at most [`MAX_LISTED`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

/// Handles or names listed per loss.
pub const MAX_LISTED: usize = 50;
/// Objects compared one by one per space or block for the detailed list (past it, spread evenly).
const DETAIL_SAMPLE: usize = 50_000;

/// Losses gathered by feature.
#[derive(Default)]
struct Losses(std::collections::BTreeMap<(String, &'static str), Loss>);

impl Losses {
    fn add(&mut self, feature: String, what: &'static str, handle: Option<String>, name: Option<String>) {
        let l = self.0.entry((feature.clone(), what)).or_insert_with(|| Loss { feature, what, count: 0, handles: Vec::new(), names: Vec::new() });
        l.count = l.count.saturating_add(1);
        if let Some(h) = handle.filter(|_| l.handles.len() < MAX_LISTED) {
            l.handles.push(h);
        }
        if let Some(n) = name.filter(|n| l.names.len() < MAX_LISTED && !l.names.contains(n)) {
            l.names.push(n);
        }
    }
}

/// The type an object is listed under: its kind (`line`, `mesh`), or the file's type name for
/// objects CADaddy keeps as raw data.
fn kind_name(kind: &serde_json::Value) -> String {
    match kind.get("type").and_then(serde_json::Value::as_str) {
        Some("unknown") => {
            kind.get("dxf_type").or_else(|| kind.get("dxfType")).and_then(serde_json::Value::as_str).unwrap_or("unknown").to_ascii_lowercase()
        }
        Some(t) => t.to_ascii_lowercase(),
        None => "object".into(),
    }
}

/// Words for a field name: `plotStyle` → "plot style", `anno_scales` → "anno scales".
fn words(key: &str) -> String {
    let mut out = String::new();
    for c in key.chars() {
        if c == '_' {
            out.push(' ');
        } else if c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// The top-level fields of `a` that `b` no longer holds.
fn changed_fields(a: &serde_json::Value, b: &serde_json::Value) -> Vec<String> {
    match (a.as_object(), b.as_object()) {
        (Some(x), Some(y)) => x
            .iter()
            .filter(|(k, v)| k.as_str() != "type" && !y.get(k.as_str()).map_or_else(|| kept(v, &serde_json::Value::Null, 1), |w| kept(v, w, 1)))
            .map(|(k, _)| k.clone())
            .collect(),
        _ => vec![String::new()],
    }
}

/// The objects of one space or block (of `da`, and as `db` read them back) compared one by one,
/// matched by handle. An object the file hasn't under its handle but whose place new objects
/// took (an older DWG's multileader written as a leader and its text) was written as those.
fn compare_objects(
    (da, a): (&Drawing, &cadaddy_doc::EntityStore),
    (db, b): (&Drawing, &cadaddy_doc::EntityStore),
    block: Option<&str>,
    out: &mut Losses,
) {
    fn v<T: Serialize>(t: &T) -> serde_json::Value {
        serde_json::to_value(t).unwrap_or_default()
    }
    let back: std::collections::HashMap<cadaddy_doc::Handle, &std::sync::Arc<cadaddy_doc::Entity>> = b.iter().map(|e| (e.handle, e)).collect();
    let step = a.len().div_ceil(DETAIL_SAMPLE).max(1);
    let mut missing = Vec::new();
    for e in a.iter().step_by(step) {
        let name = block.map(str::to_string);
        match back.get(&e.handle) {
            Some(f) => compare_pair(e, f, out, name),
            None => missing.push(e),
        }
    }
    if missing.is_empty() {
        return;
    }
    // Objects the file holds that the drawing hadn't: what the missing ones were written as.
    let ours: std::collections::HashSet<cadaddy_doc::Handle> = a.iter().map(|e| e.handle).collect();
    let mut new: Vec<(cadaddy_geom::Bounds2, String)> =
        b.iter().filter(|f| !ours.contains(&f.handle)).map(|f| (cadaddy_doc::entity_bounds(db, f, 0), kind_name(&v(&f.kind)))).collect();
    for e in missing {
        let kind = kind_name(&content(v(&e.kind)));
        let (h, name) = (Some(e.handle.hex()), block.map(str::to_string));
        let area = cadaddy_doc::entity_bounds(da, e, 0);
        let area = area.expand(area.width().max(area.height()) * 0.01 + 1e-9);
        let mut became: Vec<String> = Vec::new();
        if !area.is_empty() {
            new.retain(|(bounds, k)| {
                let inside = !bounds.is_empty() && area.contains_box(bounds);
                if inside {
                    became.push(k.clone());
                }
                !inside
            });
        }
        if became.is_empty() {
            out.add(format!("{kind} objects"), "dropped", h, name);
        } else {
            became.sort();
            became.dedup();
            out.add(format!("{kind} objects (written as {})", became.join(", ")), "changed", h, name);
        }
    }
}

/// One object against what the file holds for it: its type, fields and properties.
fn compare_pair(e: &cadaddy_doc::Entity, f: &cadaddy_doc::Entity, out: &mut Losses, name: Option<String>) {
    fn v<T: Serialize>(t: &T) -> serde_json::Value {
        serde_json::to_value(t).unwrap_or_default()
    }
    let (ka, kb) = (content(v(&e.kind)), content(v(&f.kind)));
    let (kind, h) = (kind_name(&ka), Some(e.handle.hex()));
    if kind_name(&kb) != kind {
        out.add(format!("{kind} objects (written as {})", kind_name(&kb)), "changed", h, name);
        return;
    }
    for field in changed_fields(&ka, &kb) {
        // A subdivision mesh in a file older than meshes: its faces as a polyface mesh.
        if kind == "mesh" && field == "kind" {
            let became = match kb.get("kind") {
                Some(serde_json::Value::String(k)) => words(k),
                Some(serde_json::Value::Object(o)) => o
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| o.keys().next().map(String::as_str))
                    .map_or_else(|| "another".into(), words),
                _ => "another".into(),
            };
            out.add(format!("mesh objects (written as a {became} mesh)"), "changed", h.clone(), name.clone());
            continue;
        }
        let feature = if field.is_empty() { format!("{kind} objects") } else { format!("{kind} {}", words(&field)) };
        out.add(feature, "changed", h.clone(), name.clone());
    }
    for field in changed_fields(&v(&e.common), &v(&f.common)) {
        out.add(format!("object {}", words(&field)), "changed", h.clone(), name.clone());
    }
}

/// Whether `back` was read from a Release 12 DXF file, which has no handles: its objects are
/// numbered as they are read, so they can't be matched to the drawing's by handle.
fn is_r12(back: &Drawing) -> bool {
    back.header.get("ACADVER").and_then(cadaddy_doc::HVal::as_str).is_some_and(|v| v.trim().eq_ignore_ascii_case("AC1009"))
}

/// [`compare_objects`] for a Release 12 file: each object of `a` matched to what the R12 writer
/// made of it ([`super::r12::pieces`]), in the order it wrote them. An object R12 has no form for
/// is dropped; one written as other objects (an ellipse as a polyline, a dimension as lines and
/// text) is changed; one written as itself is compared field by field. When the file doesn't
/// hold what the writer made (it left a layout's objects out, or another program wrote it), the
/// objects written as themselves are counted by type instead: those the file has fewer of are
/// dropped.
fn compare_r12(d: &Drawing, a: &cadaddy_doc::EntityStore, b: &cadaddy_doc::EntityStore, block: Option<&str>, out: &mut Losses) {
    fn v<T: Serialize>(t: &T) -> serde_json::Value {
        serde_json::to_value(t).unwrap_or_default()
    }
    let name = block.map(str::to_string);
    let kind_of = |e: &cadaddy_doc::Entity| kind_name(&content(v(&e.kind)));
    let made: Vec<(&std::sync::Arc<cadaddy_doc::Entity>, Vec<cadaddy_doc::Entity>)> = a.iter().map(|e| (e, super::r12::pieces(d, e))).collect();
    let back: Vec<&std::sync::Arc<cadaddy_doc::Entity>> = b.iter().collect();
    let aligned = made.iter().map(|(_, p)| p.len()).sum::<usize>() == back.len();
    let mut in_file = std::collections::BTreeMap::<String, usize>::new();
    if !aligned {
        for f in &back {
            *in_file.entry(kind_of(f)).or_default() += 1;
        }
    }
    let mut at = 0;
    for (e, pieces) in &made {
        let kind = kind_of(e);
        let h = Some(e.handle.hex());
        let written: &[&std::sync::Arc<cadaddy_doc::Entity>] = if aligned { back.get(at..at + pieces.len()).unwrap_or_default() } else { &[] };
        at += pieces.len();
        match pieces.as_slice() {
            [] => out.add(format!("{kind} objects"), "dropped", h, name.clone()),
            [p] if kind_of(p) == kind => match written {
                [f] => compare_pair(e, f, out, name.clone()),
                _ => match in_file.get_mut(&kind).filter(|n| **n > 0) {
                    Some(n) => *n -= 1,
                    None => out.add(format!("{kind} objects"), "dropped", h, name.clone()),
                },
            },
            many => {
                let mut kinds: Vec<String> = many.iter().map(kind_of).collect();
                kinds.sort();
                kinds.dedup();
                out.add(format!("{kind} objects (written as {})", kinds.join(", ")), "changed", h, name.clone());
            }
        }
    }
}

/// The records of one table compared by name.
fn compare_table(table: &str, a: &serde_json::Value, b: &serde_json::Value, out: &mut Losses) {
    let by_name = |v: &serde_json::Value| -> Vec<(String, serde_json::Value)> {
        v.as_array()
            .map(|a| a.iter().map(|r| (r.get("name").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(), r.clone())).collect())
            .unwrap_or_default()
    };
    let back: std::collections::HashMap<String, serde_json::Value> = by_name(b).into_iter().map(|(n, r)| (n.to_ascii_lowercase(), r)).collect();
    for (name, rec) in by_name(a) {
        match back.get(&name.to_ascii_lowercase()) {
            None => out.add(table.to_string(), "dropped", None, Some(name)),
            Some(r) => {
                // Names are matched without regard to case (Release 12 upper-cases them).
                for field in changed_fields(&rec, r).into_iter().filter(|f| f != "name") {
                    let feature = if field.is_empty() { table.to_string() } else { format!("{table}: {}", words(&field)) };
                    out.add(feature, "changed", None, Some(name.clone()));
                }
            }
        }
    }
}

/// What a DWG or DXF file named `name` wouldn't keep of `d`, feature by feature with counts and
/// the objects or records concerned: `d` written with `write`, read back with `read` and compared
/// object by object (by handle) and record by record (by name). An error when the file can't be
/// written or read back.
pub fn losses(
    d: &Drawing,
    name: &str,
    write: fn(&Drawing, &str) -> Result<Vec<u8>, String>,
    read: fn(&[u8], &str) -> Result<Drawing, String>,
) -> Result<Vec<Loss>, String> {
    let bytes = write(d, name)?;
    let back = opened(read(&bytes, name)?);
    Ok(losses_of(d, &back))
}

/// [`not_kept`] and [`losses`] of `d` against `bytes` (`d` written as `name`), read back with `read`.
pub fn check_written(
    d: &Drawing,
    name: &str,
    bytes: &[u8],
    read: fn(&[u8], &str) -> Result<Drawing, String>,
) -> Result<(Vec<&'static str>, Vec<Loss>), String> {
    let back = opened(read(bytes, name)?);
    Ok((lost_parts(d, &back), losses_of(d, &back)))
}

/// A drawing read back as opening it gives it: table formulas whose values the file didn't keep
/// worked out again ([`crate::cmd::table::cells::fill_missing_values`]).
fn opened(mut back: Drawing) -> Drawing {
    crate::cmd::table::cells::fill_missing_values(&mut back);
    back
}

/// [`losses`] of `d` against `back`, a copy of it written and read back by the caller.
pub fn losses_between(d: &Drawing, back: &Drawing) -> Vec<Loss> {
    losses_of(d, &opened(back.clone()))
}

fn losses_of(d: &Drawing, back: &Drawing) -> Vec<Loss> {
    fn v<T: Serialize>(t: &T) -> serde_json::Value {
        serde_json::to_value(t).unwrap_or_default()
    }
    let mut out = Losses::default();
    let r12 = is_r12(back);
    let objects = |a: &cadaddy_doc::EntityStore, b: &cadaddy_doc::EntityStore, block: Option<&str>, out: &mut Losses| {
        if r12 { compare_r12(d, a, b, block, out) } else { compare_objects((d, a), (back, b), block, out) }
    };
    objects(&d.model, &back.model, None, &mut out);
    for l in &d.layouts {
        match back.layouts.iter().find(|x| x.name.eq_ignore_ascii_case(&l.name)) {
            Some(m) => objects(&l.entities, &m.entities, Some(&l.name), &mut out),
            None => out.add("layouts".into(), "dropped", None, Some(l.name.clone())),
        }
    }
    for (n, b) in d.blocks.iter().filter(|(_, b)| !b.anonymous) {
        match back.blocks.get(n) {
            Some(m) => {
                objects(&b.entities, &m.entities, Some(n), &mut out);
                for field in changed_fields(&block_head(b), &block_head(m)).into_iter().filter(|f| f != "name") {
                    out.add(format!("blocks: {}", words(&field)), "changed", None, Some(n.clone()));
                }
            }
            None => out.add("blocks".into(), "dropped", None, Some(n.clone())),
        }
    }
    let tables: [(&str, serde_json::Value, serde_json::Value); 7] = [
        ("layers", v(&d.layers), v(&back.layers)),
        ("linetypes", v(&d.linetypes), v(&back.linetypes)),
        ("text styles", v(&d.text_styles), v(&back.text_styles)),
        ("dimension styles", v(&d.dim_styles), v(&back.dim_styles)),
        ("multileader styles", v(&d.mleader_styles), v(&back.mleader_styles)),
        ("table styles", v(&d.table_styles), v(&back.table_styles)),
        ("multiline styles", v(&d.mline_styles), v(&back.mline_styles)),
    ];
    for (t, a, b) in &tables {
        compare_table(t, a, b, &mut out);
    }
    // The rest as whole parts (counted by their items).
    let whole: [(&str, serde_json::Value, serde_json::Value); 10] = [
        ("named views", v(&d.views), v(&back.views)),
        ("viewport configurations", v(&d.vports), v(&back.vports)),
        ("UCSs", v(&d.ucss), v(&back.ucss)),
        ("layer states", v(&d.layer_states), v(&back.layer_states)),
        ("groups", v(&d.groups), v(&back.groups)),
        ("plot style tables", v(&d.plot_style_tables), v(&back.plot_style_tables)),
        ("geometric and dimensional constraints", v(&d.constraints), v(&back.constraints)),
        ("parameters", v(&d.parametric), v(&back.parametric)),
        ("the scale list", v(&d.scales), v(&back.scales)),
        ("materials", v(&d.materials), v(&back.materials)),
    ];
    for (part, a, b) in whole {
        if !kept(&a, &b, 0) {
            let count = a.as_array().map_or(1, Vec::len);
            let l = out.0.entry((part.to_string(), "changed")).or_insert_with(|| Loss {
                feature: part.into(),
                what: "changed",
                count: 0,
                handles: Vec::new(),
                names: Vec::new(),
            });
            l.count = count;
        }
    }
    compare_foreign(&d.foreign, &back.foreign, &mut out);
    let mut list: Vec<Loss> = out.0.into_values().collect();
    list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.feature.cmp(&b.feature)));
    list
}

/// Whether group `code` holds a handle (the object's own, or a reference to another), which a
/// writer renumbers.
fn handle_group(code: i32) -> bool {
    matches!(code, 5 | 105 | 320..=369 | 390..=399 | 480 | 481 | 1005) || code == 102
}

/// Other programs' data CADaddy keeps without modelling it (`Drawing::foreign`): their objects
/// (matched by type and content, since a writer renumbers their handles: every group of the
/// original but its handles must still be there), their CLASS records, the named object
/// dictionary's entries, and the reactors and extension dictionaries of objects.
fn compare_foreign(a: &cadaddy_doc::foreign::Foreign, b: &cadaddy_doc::foreign::Foreign, out: &mut Losses) {
    type Content = Vec<(i32, String)>;
    let content = |o: &cadaddy_doc::foreign::ForeignObject| -> Content {
        o.tags.iter().filter(|t| !handle_group(t.code)).map(|t| (t.code, t.value.trim().to_string())).collect()
    };
    // Every group of `x`, as many times, is in `y`.
    let holds = |x: &Content, y: &Content| {
        let mut rest = y.clone();
        x.iter().all(|g| match rest.iter().position(|h| h == g) {
            Some(i) => {
                rest.swap_remove(i);
                true
            }
            None => false,
        })
    };
    let mut unused: Vec<(String, Content)> = b.objects.iter().map(|o| (o.kind.to_ascii_uppercase(), content(o))).collect();
    let step = a.objects.len().div_ceil(DETAIL_SAMPLE).max(1);
    for o in a.objects.iter().step_by(step) {
        let (kind, c) = (o.kind.to_ascii_uppercase(), content(o));
        let feature = format!("{} objects of other programs", o.kind.to_ascii_lowercase());
        if let Some(i) = unused.iter().position(|(k, d)| *k == kind && holds(&c, d)) {
            unused.swap_remove(i);
        } else if let Some(i) = unused.iter().position(|(k, _)| *k == kind) {
            // There, but not as it was.
            unused.swap_remove(i);
            out.add(feature, "changed", Some(o.handle.hex()), None);
        } else {
            out.add(feature, "dropped", Some(o.handle.hex()), None);
        }
    }
    let class_name = |c: &Vec<cadaddy_doc::RawTag>| c.iter().find(|t| t.code == 1).map(|t| t.value.trim().to_ascii_uppercase());
    let classes: std::collections::HashSet<String> = b.classes.iter().filter_map(class_name).collect();
    for name in a.classes.iter().filter_map(class_name).filter(|n| !classes.contains(n)) {
        out.add("classes of other programs' objects".into(), "dropped", None, Some(name));
    }
    let keys: std::collections::HashSet<String> = b.dictionary.iter().map(|(k, _)| k.to_ascii_uppercase()).collect();
    for (key, _) in a.dictionary.iter().filter(|(k, _)| !keys.contains(&k.to_ascii_uppercase())) {
        out.add("named dictionary entries".into(), "dropped", None, Some(key.clone()));
    }
    for (h, x) in &a.entities {
        let y = b.entities.get(h);
        if x.xdict.is_some() && y.is_none_or(|y| y.xdict.is_none()) {
            out.add("extension dictionaries".into(), "dropped", Some(h.hex()), None);
        }
        let kept_reactors = y.map_or(0, |y| y.reactors.len());
        for _ in kept_reactors..x.reactors.len() {
            out.add("reactors".into(), "dropped", Some(h.hex()), None);
        }
    }
}

/// The losses in a line for the command line: "12 mesh objects dropped, 3 line thickness changed".
pub fn losses_line(list: &[Loss]) -> String {
    list.iter().map(|l| format!("{} {} {}", l.count, l.feature, l.what)).collect::<Vec<_>>().join(", ")
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
