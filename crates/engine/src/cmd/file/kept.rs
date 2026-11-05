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
    // An anonymous block (a dimension's *D1, a hatch or group's *U2…) is numbered again by the
    // writer: the name is not content, the block's own geometry is what the object draws.
    if matches!(v.get("type").and_then(serde_json::Value::as_str), Some("dimension" | "insert"))
        && let Some(o) = v.as_object_mut()
        && o.get("block").and_then(serde_json::Value::as_str).is_some_and(is_anonymous_block_name)
    {
        o.remove("block");
    }
    if v.get("type").and_then(serde_json::Value::as_str) == Some("unknown")
        && let Some(serde_json::Value::Array(tags)) = v.get_mut("tags")
    {
        tags.retain(|t| !t.get("code").and_then(serde_json::Value::as_i64).is_some_and(|c| BOOKKEEPING.contains(&c)));
    }
    v
}

/// Removes from the compared forms of `kind` the fields that mean nothing for it: a linear
/// dimension's `p15` and `p16` placeholders, the pattern angle and scale of a solid fill. They are
/// not content, so a file that doesn't hold them, or an edit that carried them along, loses nothing.
fn drop_placeholders(kind: &cadaddy_doc::EntityKind, forms: &mut [&mut serde_json::Value]) {
    let unused: Vec<&str> = match kind {
        cadaddy_doc::EntityKind::Dimension(d) => d.unused_points().to_vec(),
        cadaddy_doc::EntityKind::Hatch(h) if h.solid => vec!["angle", "scale"],
        _ => return,
    };
    for o in forms.iter_mut().filter_map(|f| f.as_object_mut()) {
        for k in &unused {
            o.remove(*k);
        }
    }
}

/// A mesh's faces as a polyface mesh holds them: a face of three or four vertices as it is, a
/// larger one as a fan of triangles from its first vertex whose inner edges are hidden (the DWG and
/// DXF writers' form: a polyface face has at most four vertices). Faces a file can't hold (fewer
/// than three vertices, or a vertex that isn't there) are left as they are, so they still differ.
fn polyface_faces(m: &cadaddy_doc::Mesh) -> Vec<cadaddy_doc::MeshFace> {
    let mut out = Vec::with_capacity(m.faces.len());
    for f in &m.faces {
        let n = f.v.len();
        if n <= 4 || f.v.iter().any(|i| *i as usize >= m.vertices.len()) {
            out.push(f.clone());
            continue;
        }
        let hidden = |k: usize| k < 64 && f.hidden & (1 << k) != 0;
        for k in 1..n - 1 {
            // Triangle 0, k, k+1: its edge 0→k is inner unless k = 1, k→k+1 is the face's own,
            // k+1→0 is inner unless k+1 = n-1.
            let (Some(&a), Some(&b), Some(&c)) = (f.v.first(), f.v.get(k), f.v.get(k + 1)) else { continue };
            let bits = u64::from(k != 1 || hidden(0)) | u64::from(hidden(k)) << 1 | u64::from(k + 1 != n - 1 || hidden(n - 1)) << 2;
            out.push(cadaddy_doc::MeshFace { v: vec![a, b, c], hidden: bits });
        }
    }
    out
}

/// A mesh whose faces of more than four vertices came back as their polyface fans
/// ([`polyface_faces`]: the same triangles, the inner edges hidden, the face's own edges as they
/// were) keeps its shape: `a`'s faces are taken as the file's. Any other difference in the faces
/// (a vertex, a triangle or an edge's visibility) still shows.
fn take_fanned_faces(kind: &cadaddy_doc::EntityKind, a: &mut serde_json::Value, b: &serde_json::Value) {
    let cadaddy_doc::EntityKind::Mesh(m) = kind else { return };
    if !m.faces.iter().any(|f| f.v.len() > 4) {
        return;
    }
    let Some(theirs) = b.get("faces") else { return };
    if serde_json::to_value(polyface_faces(m)).is_ok_and(|fan| fan == *theirs)
        && let Some(o) = a.as_object_mut()
    {
        o.insert("faces".into(), theirs.clone());
    }
}

/// Whether `n` is an anonymous block's name: `*`, a letter and a number (`*D12`, `*U3`, `*X1`).
fn is_anonymous_block_name(n: &str) -> bool {
    let b = n.as_bytes();
    b.len() > 2 && b.first() == Some(&b'*') && b.get(1).is_some_and(u8::is_ascii_alphabetic) && b.iter().skip(2).all(u8::is_ascii_digit)
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

/// The parts of a drawing compared, named as the notice names them.
const PARTS: [&str; 22] = [
    "objects",
    "object properties",
    "layers",
    "linetypes",
    "text styles",
    "dimension styles",
    "multileader styles",
    "table styles",
    "multiline styles",
    "blocks",
    "layouts",
    "named views",
    "viewport configurations",
    "UCSs",
    "layer states",
    "groups",
    "plot style tables",
    "geometric and dimensional constraints",
    "parameters",
    "the scale list",
    "materials",
    "other programs' data",
];

fn v<T: Serialize>(t: &T) -> serde_json::Value {
    serde_json::to_value(t).unwrap_or_default()
}

/// Whether `back` still holds the part of `d` named `part`, as [`kept`] compares them. Objects are
/// made into JSON a pair at a time: the whole drawing and its copy read back as JSON at once were
/// several times the drawing's size on every DWG/DXF save, and each saving thread's memory kept
/// the largest of them (B5-005).
fn part_kept(part: &str, d: &Drawing, back: &Drawing) -> bool {
    fn same<T: Serialize>(a: &T, b: &T) -> bool {
        kept(&v(a), &v(b), 0)
    }
    match part {
        "objects" => objects_kept(&d.model, &back.model, 0),
        "object properties" => {
            let (a, b) = (&d.model, &back.model);
            // Two lists: an empty one is kept, else as many, each kept.
            a.is_empty() || (sampled(a.len()) == sampled(b.len()) && sample(a).zip(sample(b)).all(|(e, f)| kept(&v(&e.common), &v(&f.common), 1)))
        }
        "layers" => same(&d.layers, &back.layers),
        "linetypes" => same(&d.linetypes, &back.linetypes),
        "text styles" => same(&d.text_styles, &back.text_styles),
        "dimension styles" => same(&d.dim_styles, &back.dim_styles),
        "multileader styles" => same(&d.mleader_styles, &back.mleader_styles),
        "table styles" => same(&d.table_styles, &back.table_styles),
        "multiline styles" => same(&d.mline_styles, &back.mline_styles),
        // By name, not by place: a block a writer adds (a dynamic block's definition made again from
        // its anonymous copy) or a layout it puts elsewhere in the list loses nothing.
        "blocks" => {
            fn named(d: &Drawing) -> std::collections::HashMap<String, &cadaddy_doc::Block> {
                d.blocks.iter().filter(|(_, b)| !b.anonymous).map(|(n, b)| (n.to_ascii_lowercase(), &**b)).collect()
            }
            let theirs = named(back);
            named(d)
                .into_iter()
                .all(|(n, a)| theirs.get(&n).is_some_and(|b| kept(&block_head(a), &block_head(b), 2) && objects_kept(&a.entities, &b.entities, 2)))
        }
        "layouts" => {
            fn named(d: &Drawing) -> std::collections::HashMap<String, &cadaddy_doc::EntityStore> {
                d.layouts.iter().map(|l| (l.name.to_ascii_lowercase(), &l.entities)).collect()
            }
            let theirs = named(back);
            named(d).into_iter().all(|(n, a)| theirs.get(&n).is_some_and(|b| objects_kept(a, b, 1)))
        }
        "named views" => same(&d.views, &back.views),
        "viewport configurations" => same(&d.vports, &back.vports),
        "UCSs" => same(&d.ucss, &back.ucss),
        "layer states" => same(&d.layer_states, &back.layer_states),
        "groups" => same(&d.groups, &back.groups),
        "plot style tables" => same(&d.plot_style_tables, &back.plot_style_tables),
        "geometric and dimensional constraints" => same(&d.constraints, &back.constraints),
        "parameters" => same(&d.parametric, &back.parametric),
        "the scale list" => same(&d.scales, &back.scales),
        _ => true,
    }
}

/// [`kept`] of `{count, objects}` for two object lists (`depth`: where the pair sits), the objects
/// spread evenly ([`SAMPLE`]) and compared one pair at a time.
fn objects_kept(a: &cadaddy_doc::EntityStore, b: &cadaddy_doc::EntityStore, depth: usize) -> bool {
    if !kept(&a.len().into(), &b.len().into(), depth + 1) {
        return false;
    }
    a.is_empty()
        || (sampled(a.len()) == sampled(b.len())
            && sample(a).zip(sample(b)).all(|(e, f)| {
                // The same placeholders the detail list skips ([`compare_pair`]), or a category is
                // reported with nothing listed under it.
                let (mut ka, mut kb) = (content(v(&e.kind)), content(v(&f.kind)));
                drop_placeholders(&e.kind, &mut [&mut ka, &mut kb]);
                take_fanned_faces(&e.kind, &mut ka, &kb);
                kept(&ka, &kb, depth + 2)
            }))
}

/// The objects of a list compared: every [`step`]th.
fn sample(s: &cadaddy_doc::EntityStore) -> impl Iterator<Item = &std::sync::Arc<cadaddy_doc::Entity>> {
    s.iter().step_by(step(s.len()))
}

/// How many objects [`sample`] gives for a list of `len`.
fn sampled(len: usize) -> usize {
    len.div_ceil(step(len))
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

/// The size of the difference between two numbers that is rounding noise of a calculation (an
/// extrusion of (-1e-16, 0, -1), a pattern line's offset of 1e-17), not a change.
const FLOAT_NOISE: f64 = 1e-12;

/// Fields that name a table record (a layer, linetype, style), compared without case.
const SYMBOL_KEYS: [&str; 5] = ["linetype", "textStyle", "style", "dimStyle", "layer"];

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
            // Below FLOAT_NOISE, a number is arithmetic noise (a cosine of 1e-17): equal to zero.
            (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * x.abs().max(y.abs()) || (x - y).abs() <= FLOAT_NOISE,
            _ => false,
        },
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| kept(a, b, depth + 1)),
        (Value::Object(x), Value::Object(y)) => x.iter().all(|(k, a)| match (y.get(k), a) {
            // Symbol names are not case sensitive: a writer may give a table record's name in
            // capitals (CONTINUOUS for Continuous), which every reference to it follows.
            (Some(Value::String(b)), Value::String(a)) if SYMBOL_KEYS.contains(&k.as_str()) => a.eq_ignore_ascii_case(b),
            (Some(b), a) => kept(a, b, depth + 1),
            (None, a) => kept(a, &Value::Null, depth + 1),
        }),
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
    let lost: Vec<&'static str> = PARTS.into_iter().filter(|part| !part_kept(part, d, back)).collect();
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

/// Losses gathered by feature, and the parts ([`PARTS`]) they are in.
#[derive(Default)]
struct Losses {
    by_feature: std::collections::BTreeMap<(String, &'static str), Loss>,
    /// The part being compared, which [`Losses::add`] counts as losing something.
    part: &'static str,
    parts: std::collections::BTreeSet<&'static str>,
}

impl Losses {
    /// Compare `part` next.
    fn in_part(&mut self, part: &'static str) -> &mut Self {
        self.part = part;
        self
    }

    fn add(&mut self, feature: String, what: &'static str, handle: Option<String>, name: Option<String>) {
        self.parts.insert(self.part);
        let l = self.by_feature.entry((feature.clone(), what)).or_insert_with(|| Loss {
            feature,
            what,
            count: 0,
            handles: Vec::new(),
            names: Vec::new(),
        });
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
            .filter(|(k, v)| {
                k.as_str() != "type"
                    && !match (y.get(k.as_str()), v) {
                        (Some(serde_json::Value::String(w)), serde_json::Value::String(v)) if SYMBOL_KEYS.contains(&k.as_str()) => {
                            v.eq_ignore_ascii_case(w)
                        }
                        (Some(w), v) => kept(v, w, 1),
                        (None, v) => kept(v, &serde_json::Value::Null, 1),
                    }
            })
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
    let (mut ka, mut kb) = (content(v(&e.kind)), content(v(&f.kind)));
    drop_placeholders(&e.kind, &mut [&mut ka, &mut kb]);
    take_fanned_faces(&e.kind, &mut ka, &kb);
    let (kind, h) = (kind_name(&ka), Some(e.handle.hex()));
    if kind_name(&kb) != kind {
        out.add(format!("{kind} objects (written as {})", kind_name(&kb)), "changed", h, name);
        return;
    }
    // A typed object read back as the file's raw record (a HELIX or LIGHT the native DWG reader
    // doesn't decode): kept as stored, but no longer editable as its kind.
    let raw = |k: &serde_json::Value| k.get("type").and_then(serde_json::Value::as_str) == Some("unknown");
    if raw(&kb) && !raw(&ka) {
        out.add(format!("{kind} objects (written as a stored {kind} record, not editable)"), "changed", h, name);
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
    // In model space an object's properties are a part of their own; in a block or layout, that one's.
    let part = out.part;
    if part == "objects" {
        out.part = "object properties";
    }
    for field in changed_fields(&v(&e.common), &v(&f.common)) {
        out.add(format!("object {}", words(&field)), "changed", h.clone(), name.clone());
    }
    out.part = part;
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
    // One comparison (one sample) for both: the categories are the parts the detail lists losses
    // in. A big drawing's categories came from a 2,000-object sample and its detail from 50,000,
    // so a detail could come with no category at all (S5-006).
    let (detail, lost) = losses_and_parts(d, &back);
    Ok((lost, detail))
}

/// A drawing read back as opening it gives it: table formulas whose values the file didn't keep
/// worked out again ([`crate::cmd::table::cells::fill_missing_values`]).
fn opened(mut back: Drawing) -> Drawing {
    crate::cmd::table::cells::fill_missing_values(&mut back);
    back
}

/// The categories the save notice names for `d` against `back` ([`check_written`]'s first half):
/// for the differential fuzzing, which checks that none is named without a detail.
pub fn parts_between(d: &Drawing, back: &Drawing) -> Vec<&'static str> {
    lost_parts(d, &opened(back.clone()))
}

/// [`losses`] of `d` against `back`, a copy of it written and read back by the caller.
pub fn losses_between(d: &Drawing, back: &Drawing) -> Vec<Loss> {
    losses_of(d, &opened(back.clone()))
}

fn losses_of(d: &Drawing, back: &Drawing) -> Vec<Loss> {
    losses_and_parts(d, back).0
}

/// [`losses_of`], and the parts ([`PARTS`], in that order) the losses are in: the save notice's
/// categories come from the same comparison as its detail, so neither names what the other doesn't (S5-006).
fn losses_and_parts(d: &Drawing, back: &Drawing) -> (Vec<Loss>, Vec<&'static str>) {
    fn v<T: Serialize>(t: &T) -> serde_json::Value {
        serde_json::to_value(t).unwrap_or_default()
    }
    let mut out = Losses::default();
    let r12 = is_r12(back);
    let objects = |a: &cadaddy_doc::EntityStore, b: &cadaddy_doc::EntityStore, block: Option<&str>, out: &mut Losses| {
        if r12 { compare_r12(d, a, b, block, out) } else { compare_objects((d, a), (back, b), block, out) }
    };
    objects(&d.model, &back.model, None, out.in_part("objects"));
    out.in_part("layouts");
    for l in &d.layouts {
        match back.layouts.iter().find(|x| x.name.eq_ignore_ascii_case(&l.name)) {
            Some(m) => objects(&l.entities, &m.entities, Some(&l.name), &mut out),
            None => out.add("layouts".into(), "dropped", None, Some(l.name.clone())),
        }
    }
    out.in_part("blocks");
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
        compare_table(t, a, b, out.in_part(t));
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
            out.parts.insert(part);
            let l = out.by_feature.entry((part.to_string(), "changed")).or_insert_with(|| Loss {
                feature: part.into(),
                what: "changed",
                count: 0,
                handles: Vec::new(),
                names: Vec::new(),
            });
            l.count = count;
        }
    }
    compare_foreign(&d.foreign, &back.foreign, out.in_part("other programs' data"));
    let mut list: Vec<Loss> = out.by_feature.into_values().collect();
    list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.feature.cmp(&b.feature)));
    (list, PARTS.into_iter().filter(|p| out.parts.contains(p)).collect())
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
    fn pattern_line_float_residue_is_not_a_change_but_a_real_change_is() {
        let line = |dx: f64, dy: f64| json!([{ "angle": 0.5, "base": [0.0, 0.0], "delta": [dx, dy], "dashes": [] }]);
        // 1.19e-15 read back as 0, and a last-digit difference, are rounding.
        assert!(kept(&line(1.19e-15, 12.699999999999998), &line(0.0, 12.699999999999996), 0));
        // A pattern line that really moved is flagged.
        assert!(!kept(&line(1.19e-15, 12.7), &line(0.001, 12.7), 0));
        assert!(!kept(&line(0.0, 12.7), &line(0.0, 12.0), 0));
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
    fn a_typed_object_read_back_as_a_raw_record_is_reported() {
        let typed = |h: u64| {
            cadaddy_doc::Entity::new(
                cadaddy_doc::Handle(h),
                cadaddy_doc::EntityKind::Circle(cadaddy_doc::Circle { center: cadaddy_geom::Vec3::new(1.0, 2.0, 0.0), radius: 1.0 }),
            )
        };
        let raw = |h: u64| {
            cadaddy_doc::Entity::new(
                cadaddy_doc::Handle(h),
                cadaddy_doc::EntityKind::Unknown(cadaddy_doc::Unknown { dxf_type: "CIRCLE".into(), tags: Vec::new() }),
            )
        };
        let (mut d, mut back) = (Drawing::new_imperial(), Drawing::new_imperial());
        d.model.push(typed(0x100));
        back.model.push(raw(0x100));
        let notes = losses_between(&d, &back);
        assert!(notes.iter().any(|l| l.feature.starts_with("circle objects (written as") && l.handles == vec!["100".to_string()]), "{notes:?}");
        // A raw record read back raw loses nothing.
        let mut same = Drawing::new_imperial();
        same.model.push(raw(0x100));
        assert!(losses_between(&same, &same.clone()).is_empty());
    }

    #[test]
    fn objects_compared_a_pair_at_a_time_still_name_what_changed() {
        let circle = |h: u64, r: f64| {
            cadaddy_doc::Entity::new(
                cadaddy_doc::Handle(h),
                cadaddy_doc::EntityKind::Circle(cadaddy_doc::Circle { center: cadaddy_geom::Vec3::new(1.0, 2.0, 0.0), radius: r }),
            )
        };
        let mut d = Drawing::new_imperial();
        for h in 1..=3 {
            d.model.push(circle(0x100 + h, 1.0));
        }
        let mut block = cadaddy_doc::Block::new("Door");
        block.entities.push(circle(0x200, 1.0));
        d.blocks.insert("Door".into(), std::sync::Arc::new(block));
        assert!(!d.layouts.is_empty());
        d.layouts[0].entities.push(circle(0x300, 1.0));
        assert!(lost_parts(&d, &d.clone()).is_empty());
        // One object changed, one object's layer, a block's object, a layout's objects gone.
        let mut back = d.clone();
        back.model = Default::default();
        for h in 1..=3 {
            back.model.push(circle(0x100 + h, if h == 2 { 1.5 } else { 1.0 }));
        }
        assert_eq!(lost_parts(&d, &back), vec!["objects"]);
        let mut moved = d.clone();
        moved.model = Default::default();
        for h in 1..=3 {
            let mut e = circle(0x100 + h, 1.0);
            if h == 3 {
                e.common.layer = "Walls".into();
            }
            moved.model.push(e);
        }
        assert_eq!(lost_parts(&d, &moved), vec!["object properties"]);
        let mut back = d.clone();
        let mut other = cadaddy_doc::Block::new("Door");
        other.entities.push(circle(0x200, 2.0));
        back.blocks.insert("Door".into(), std::sync::Arc::new(other));
        back.layouts[0].entities = Default::default();
        assert_eq!(lost_parts(&d, &back), vec!["blocks", "layouts"]);
        // Fewer objects: counted, and their properties not compared out of line.
        let mut back = d.clone();
        back.model = Default::default();
        back.model.push(circle(0x101, 1.0));
        assert_eq!(lost_parts(&d, &back), vec!["objects"]);
    }

    /// S5-006: the save's categories come from the detail's own comparison. A change the
    /// 2,000-object sample steps over (every other object of 3,000) was listed in the detail with
    /// no category, and an agent reading `notKept` was told the save was clean.
    #[test]
    fn categories_come_from_the_same_sample_as_the_detail() {
        thread_local! {
            static BACK: std::cell::RefCell<Option<Drawing>> = const { std::cell::RefCell::new(None) };
        }
        let circle = |h: u64, r: f64| {
            cadaddy_doc::Entity::new(
                cadaddy_doc::Handle(h),
                cadaddy_doc::EntityKind::Circle(cadaddy_doc::Circle { center: cadaddy_geom::Vec3::new(1.0, 2.0, 0.0), radius: r }),
            )
        };
        let mut d = Drawing::new_imperial();
        for h in 0..3000 {
            d.model.push(circle(0x100 + h, 1.0));
        }
        let mut block = cadaddy_doc::Block::new("Door");
        block.entities.push(circle(0x10000, 1.0));
        d.blocks.insert("Door".into(), std::sync::Arc::new(block));
        let read: fn(&[u8], &str) -> Result<Drawing, String> = |_, _| BACK.with(|b| b.borrow().clone()).ok_or_else(|| "nothing".to_string());
        let check = |back: Drawing| {
            BACK.with(|b| *b.borrow_mut() = Some(back));
            check_written(&d, "x.dxf", b"", read).unwrap()
        };
        // One radius changed, at an object the sample steps over.
        let mut back = d.clone();
        back.model = Default::default();
        for h in 0..3000 {
            back.model.push(circle(0x100 + h, if h == 1 { 1.5 } else { 1.0 }));
        }
        assert!(lost_parts(&d, &back).is_empty(), "the 2,000-object sample misses it");
        let (lost, detail) = check(back);
        assert_eq!(lost, vec!["objects"], "{detail:?}");
        assert!(detail.iter().any(|l| l.feature == "circle radius" && l.handles == vec!["101".to_string()]), "{detail:?}");
        // An object's layer: its properties; a block object's anything: the blocks.
        let mut back = d.clone();
        back.model.modify(cadaddy_doc::Handle(0x103), |e| e.common.layer = "Walls".into());
        let mut other = cadaddy_doc::Block::new("Door");
        let mut c = circle(0x10000, 1.0);
        c.common.layer = "Walls".into();
        other.entities.push(c);
        back.blocks.insert("Door".into(), std::sync::Arc::new(other));
        let (lost, detail) = check(back);
        assert_eq!(lost, vec!["object properties", "blocks"], "{detail:?}");
        // Nothing lost: no category, no detail.
        assert_eq!(check(d.clone()), (Vec::new(), Vec::new()));
    }

    /// A polyface face holds at most four vertices: a larger face is stored as a fan of triangles
    /// from its first vertex, the fan's inner edges hidden. That keeps its shape and visible
    /// edges, so it is no loss (every BIM model's first DWG/DXF save said "mesh faces changed");
    /// any other change to the faces still is.
    #[test]
    fn an_n_gon_stored_as_its_fan_is_kept_but_a_changed_fan_is_not() {
        use cadaddy_doc::{Mesh, MeshFace, MeshKind};
        let face = |v: &[u32], hidden: u64| MeshFace { v: v.to_vec(), hidden };
        let mesh = |faces: Vec<MeshFace>| {
            let vertices = (0..5u32).map(|i| cadaddy_geom::Vec3::new(f64::from(i), f64::from(i * i), 0.0)).collect();
            cadaddy_doc::Entity::new(cadaddy_doc::Handle(0x100), cadaddy_doc::EntityKind::Mesh(Mesh { vertices, faces, kind: MeshKind::Polyface }))
        };
        let mut d = Drawing::new_imperial();
        // Its edge 2→3 hidden.
        d.model.push(mesh(vec![face(&[0, 1, 2, 3, 4], 0b100)]));
        let back = |faces: Vec<MeshFace>| {
            let mut b = d.clone();
            b.model = Default::default();
            b.model.push(mesh(faces));
            b
        };
        // Triangles 0-1-2, 0-2-3, 0-3-4: edges 0→2 and 0→3 inner (hidden), 2→3 hidden as it was.
        let fan = back(vec![face(&[0, 1, 2], 0b100), face(&[0, 2, 3], 0b111), face(&[0, 3, 4], 0b001)]);
        assert!(losses_between(&d, &fan).is_empty(), "{:?}", losses_between(&d, &fan));
        assert!(lost_parts(&d, &fan).is_empty());
        for (what, faces) in [
            ("an inner edge drawn", vec![face(&[0, 1, 2], 0), face(&[0, 2, 3], 0b111), face(&[0, 3, 4], 0b001)]),
            ("a hidden edge drawn", vec![face(&[0, 1, 2], 0b100), face(&[0, 2, 3], 0b101), face(&[0, 3, 4], 0b001)]),
            ("another triangle", vec![face(&[0, 1, 3], 0b100), face(&[0, 2, 3], 0b111), face(&[0, 3, 4], 0b001)]),
            ("a triangle missing", vec![face(&[0, 1, 2], 0b100), face(&[0, 2, 3], 0b111)]),
        ] {
            let b = back(faces);
            let l = losses_between(&d, &b);
            assert!(l.iter().any(|l| l.feature == "mesh faces" && l.what == "changed"), "{what}: {l:?}");
            assert_eq!(lost_parts(&d, &b), vec!["objects"], "{what}");
        }
    }

    #[test]
    fn blocks_are_matched_by_name_not_by_place() {
        let block = |n: &str| std::sync::Arc::new(cadaddy_doc::Block::new(n));
        let mut d = Drawing::new_imperial();
        d.blocks.insert("Door".into(), block("Door"));
        let mut back = d.clone();
        // A block the file has more, or earlier in its list, loses nothing.
        back.blocks.insert("Alpha".into(), block("Alpha"));
        assert!(lost_parts(&d, &back).is_empty());
        // One it lacks is a loss.
        back.blocks.remove("Door");
        assert_eq!(lost_parts(&d, &back), vec!["blocks"]);
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

    #[test]
    fn rounding_noise_is_not_a_change() {
        // An extrusion of (-1e-16, 0, -1) written and read back as (0, 0, -1); a pattern line's
        // offset of 1e-17 as 2e-17.
        assert!(kept(&json!({"x": -9.8e-17, "z": -1.0}), &json!({"x": 0.0, "z": -1.0}), 0));
        assert!(kept(&json!([1.1e-17]), &json!([2.2e-17]), 0));
        // A real difference, a tiny one included, still is one.
        assert!(!kept(&json!([1e-10]), &json!([0.0]), 0));
        assert!(!kept(&json!({"z": -1.0}), &json!({"z": 1.0}), 0));
    }

    #[test]
    fn table_references_are_matched_without_case() {
        assert!(kept(&json!({"linetype": "Continuous"}), &json!({"linetype": "CONTINUOUS"}), 0));
        assert!(changed_fields(&json!({"textStyle": "Standard"}), &json!({"textStyle": "STANDARD"})).is_empty());
        // Text is compared as it is.
        assert!(!kept(&json!({"contents": "Abc"}), &json!({"contents": "ABC"}), 0));
    }

    #[test]
    fn anonymous_block_numbers_are_not_content() {
        let dim = |b: &str| content(json!({"type": "dimension", "block": b, "text": "<>"}));
        assert_eq!(dim("*D20"), dim("*D7"));
        let ins = |b: &str| content(json!({"type": "insert", "block": b}));
        assert_eq!(ins("*U2"), ins("*U3"));
        // A named block is content.
        assert_ne!(ins("Door"), ins("Window"));
        assert_ne!(ins("*Model_Space"), ins("*Paper_Space"));
    }

    fn linear() -> cadaddy_doc::Dimension {
        let z = cadaddy_geom::Vec3::ZERO;
        cadaddy_doc::Dimension {
            kind: cadaddy_doc::DimKind::Linear { rotation: 0.0 },
            defpt: z,
            text_mid: z,
            p13: z,
            p14: cadaddy_geom::Vec3::new(5.0, 0.0, 0.0),
            p15: z,
            p16: z,
            text: String::new(),
            style: "Standard".into(),
            measurement: 5.0,
            text_rotation: 0.0,
            user_text_pos: false,
            block: None,
            overrides: Default::default(),
            assoc: Vec::new(),
            marks: Default::default(),
        }
    }

    /// The compared forms of two dimensions, as `compare_pair` makes them.
    fn dim_changes(a: &cadaddy_doc::Dimension, b: &cadaddy_doc::Dimension) -> Vec<String> {
        let kind = cadaddy_doc::EntityKind::Dimension(Box::new(a.clone()));
        let (mut x, mut y) = (content(serde_json::to_value(a).unwrap_or_default()), content(serde_json::to_value(b).unwrap_or_default()));
        for v in [&mut x, &mut y] {
            if let Some(o) = v.as_object_mut() {
                o.insert("type".into(), json!("dimension"));
            }
        }
        drop_placeholders(&kind, &mut [&mut x, &mut y]);
        changed_fields(&x, &y)
    }

    #[test]
    fn a_linear_dimensions_placeholder_points_are_not_content() {
        let a = linear();
        let mut b = a.clone();
        // The file holds other values for the points a linear dimension doesn't use.
        let q = cadaddy_geom::Vec3::new(3.3, -1.7, 0.0);
        (b.p15, b.p16, b.text_mid) = (q, q, q);
        assert!(dim_changes(&a, &b).is_empty());
        // A moved dimension carries only the points it uses.
        let mut m = a.clone();
        for p in m.used_points_mut() {
            *p = *p + q;
        }
        assert_eq!((m.p15, m.p16, m.text_mid), (a.p15, a.p16, a.text_mid));
        assert_ne!(m.p14, a.p14);
        // A real loss still shows: the measurement, or a used point.
        let mut c = a.clone();
        c.measurement = 6.0;
        assert_eq!(dim_changes(&a, &c), vec!["measurement"]);
        let mut d = a.clone();
        d.p14 = q;
        assert_eq!(dim_changes(&a, &d), vec!["p14"]);
        // Text the user placed is content.
        let (mut e, mut f) = (a.clone(), a.clone());
        (e.user_text_pos, f.user_text_pos) = (true, true);
        e.text_mid = q;
        assert_eq!(dim_changes(&e, &f), vec!["textMid"]);
    }

    #[test]
    fn a_solid_fills_pattern_angle_and_scale_are_not_content_but_a_patterns_are() {
        let hatch = |solid: bool, scale: f64| {
            cadaddy_doc::EntityKind::Hatch(Box::new(cadaddy_doc::Hatch {
                pattern: if solid { "SOLID" } else { "ANSI31" }.into(),
                solid,
                loops: Vec::new(),
                scale,
                angle: 0.0,
                associative: false,
                style: 0,
                elevation: 0.0,
                gradient: None,
                origin: cadaddy_geom::Vec2::ZERO,
                background: None,
                pattern_lines: Vec::new(),
            }))
        };
        let form = |k: &cadaddy_doc::EntityKind| content(serde_json::to_value(k).unwrap_or_default());
        let changes = |a: cadaddy_doc::EntityKind, b: cadaddy_doc::EntityKind| {
            let (mut x, mut y) = (form(&a), form(&b));
            drop_placeholders(&a, &mut [&mut x, &mut y]);
            changed_fields(&x, &y)
        };
        assert!(changes(hatch(true, 1.0), hatch(true, 0.526)).is_empty());
        assert_eq!(changes(hatch(false, 1.0), hatch(false, 0.526)), vec!["scale"]);
    }
}
