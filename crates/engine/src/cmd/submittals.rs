//! Submittals onto the model (`bim.submittals`).
//!
//! Once the architecture is built and the MEP objects are made, the submittals say exactly what
//! each object is. Every element of a type mark gets its submittal product as properties: maker,
//! catalog number, how it dims, and which submittal says so. The source is the submittal and
//! page. The products also go into the drawing's type catalog. A person's values stay.

use std::collections::BTreeMap;

use cadaddy_doc::bimspec::keys;
use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Entity, Handle};
use serde_json::{Value, json};

use super::propfill::WithNote;
use super::*;
use crate::{Result, Session};

const CMD: &str = "bim.submittals";
/// Submittal PDFs read in one call.
const MAX_FILES: usize = 64;
/// Elements kept.
const MAX_ELEMENTS: usize = 500_000;
/// Marks listed in a report.
const MAX_LISTED: usize = 200;
/// The element kinds a submittal product goes on (doors and walls have type marks of their own).
const PRODUCT_KINDS: &[&str] = &["device", "light_fixture", "fixture", "equipment"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Apply Submittals", run)
            .params("{paths: [path] (submittal PDFs, or folders of them)} → {files: [{file, products, marks}], applied: {mark: elements}, elements, catalog, dimming: {mark: control}, no_submittal: {mark: elements}, not_in_model: [mark], questions: [{mark, text}], unread: [{file, error}]}: each device, fixture and equipment element of a type mark gets its submittal product (manufacturer, catalog_number, dimming, submittal), with the submittal and page as the source; the products go into the type catalog; a person's values stay")
            .menu(&["Tools", "Building", "Apply Submittals"]),
    ]
}

/// A submittal's product for one type mark.
struct Product {
    /// The submittal it is in (the file's name) and the page its data starts on.
    doc: String,
    /// What `bim.spec_import` proposed for it.
    proposal: Value,
}

impl Product {
    fn field(&self, key: &str) -> Option<&Value> {
        self.proposal.get("fields").and_then(|f| f.get(key)).filter(|f| f.get("value").and_then(Value::as_str).is_some_and(|v| !v.trim().is_empty()))
    }
    fn value(&self, key: &str) -> Option<String> {
        self.field(key).and_then(|f| f.get("value")).and_then(Value::as_str).map(|v| v.trim().to_string())
    }
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let mut given: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if let Some(one) = p.get("path").and_then(Value::as_str) {
        given.push(one.to_string());
    }
    if given.is_empty() {
        return Err(bad(CMD, "`paths` (submittal PDFs, or folders of them) is required"));
    }
    let files = pdf_files(&given);
    if files.is_empty() {
        return Err(bad(CMD, format!("no PDF at {}", given.join(", "))));
    }

    let Read { products, read, unread, questions } = read_products(s, &files);

    // The model's elements by type mark.
    let d = s.doc()?;
    let mut by_mark: BTreeMap<String, Vec<Handle>> = BTreeMap::new();
    // Every entity is looked at (a print set's linework comes first), the elements kept capped.
    let mut kept = 0usize;
    for e in d.model.iter() {
        if kept >= MAX_ELEMENTS {
            break;
        }
        if let Some(mark) = product_mark(e) {
            by_mark.entry(mark.to_ascii_uppercase()).or_default().push(e.handle);
            kept += 1;
        }
    }

    // The products of the model's types into the catalog.
    let accepted: Vec<Value> = products.iter().filter(|(m, _)| by_mark.contains_key(*m)).map(|(_, p)| p.proposal.clone()).collect();
    let catalog = if accepted.is_empty() {
        0
    } else {
        s.execute("bim.spec_accept", &json!({"proposals": accepted}))?.get("accepted").and_then(Value::as_u64).unwrap_or(0)
    };

    // Each element of a type: what its submittal says it is.
    let d = s.doc()?;
    let mut changes: Vec<(Handle, Props)> = Vec::new();
    let mut applied: BTreeMap<String, usize> = BTreeMap::new();
    let mut dimming: BTreeMap<String, String> = BTreeMap::new();
    for (mark, handles) in &by_mark {
        let Some(product) = products.get(mark) else { continue };
        let src = Source::from_doc(SourceKind::Submittal, &product.doc);
        let mut set: Vec<(&str, Prop)> = Vec::new();
        if let Some(v) = product.value(keys::MANUFACTURER) {
            set.push(("manufacturer", Prop::new(v, src.clone())));
        }
        if let Some(v) = product.value(keys::CATALOG_NUMBER) {
            set.push(("catalog_number", Prop::new(v, src.clone())));
        }
        if let Some(control) = product.value(keys::DIMMING) {
            let f = product.field(keys::DIMMING);
            // "p.3 (ordering row for X)": how it was found.
            let how = f
                .and_then(|f| f.get("source"))
                .and_then(Value::as_str)
                .and_then(|s| s.split_once('('))
                .map(|(_, h)| h.trim_end_matches(')').to_string());
            let inferred = f.and_then(|f| f.get("confidence")).and_then(Value::as_str).is_some_and(|c| c != "stated");
            let note = match (how, inferred) {
                (Some(h), true) => format!("inferred: {h}"),
                (Some(h), false) => h,
                (None, true) => "inferred".into(),
                (None, false) => String::new(),
            };
            let prop = Prop::new(control.clone(), src.clone());
            set.push(("dimming", if note.is_empty() { prop } else { prop.with_note(&note) }));
            dimming.insert(mark.clone(), control);
        }
        set.push(("submittal", Prop::new(product.doc.clone(), src.clone())));
        for h in handles {
            let Some(e) = d.model.get(*h) else { continue };
            let mut store = props::read(&e.common.xdata);
            let before = store.clone();
            for (key, prop) in &set {
                // A person's value stays.
                if store.get(key).is_none_or(|old| old.source.kind != SourceKind::User) {
                    store.set(key, prop.clone());
                }
            }
            if store != before {
                changes.push((*h, store));
            }
            *applied.entry(mark.clone()).or_insert(0) += 1;
        }
    }
    let d = s.doc_mut()?;
    let mut changed = 0usize;
    for (h, store) in changes {
        if d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).is_ok() {
            changed += 1;
        }
    }
    if changed > 0 {
        s.touch();
    }

    let no_submittal: BTreeMap<&String, usize> =
        by_mark.iter().filter(|(m, _)| !products.contains_key(*m)).map(|(m, h)| (m, h.len())).take(MAX_LISTED).collect();
    let not_in_model: Vec<&String> = products.keys().filter(|m| !by_mark.contains_key(*m)).take(MAX_LISTED).collect();
    let questions: Vec<Value> = questions.into_iter().take(MAX_LISTED).map(|(mark, text)| json!({"mark": mark, "text": text})).collect();
    Ok(json!({
        "files": read,
        "applied": applied,
        "elements": changed,
        "catalog": catalog,
        "dimming": dimming,
        "no_submittal": no_submittal,
        "not_in_model": not_in_model,
        "questions": questions,
        "unread": unread,
    }))
}

/// What the submittal files say.
struct Read {
    /// The products by mark (upper case): the first submittal giving a mark a maker or number.
    products: BTreeMap<String, Product>,
    /// Each file read: its products.
    read: Vec<Value>,
    /// Each file that couldn't be read, and why.
    unread: Vec<Value>,
    /// (mark, question) the submittals leave open.
    questions: Vec<(String, String)>,
}

fn read_products(s: &mut Session, files: &[String]) -> Read {
    let mut out = Read { products: BTreeMap::new(), read: Vec::new(), unread: Vec::new(), questions: Vec::new() };
    for f in files {
        let name = std::path::Path::new(f).file_stem().map_or_else(|| f.clone(), |n| n.to_string_lossy().to_string());
        let r = match s.execute("bim.spec_import", &json!({"file": f, "format": "submittal"})) {
            Ok(r) => r,
            Err(e) => {
                out.unread.push(json!({"file": name, "error": e.to_string()}));
                continue;
            }
        };
        let mut marks = Vec::new();
        for prop in r.get("proposals").and_then(Value::as_array).into_iter().flatten() {
            let mark = prop.get("mark").and_then(Value::as_str).unwrap_or("").trim();
            let product = Product { doc: name.clone(), proposal: prop.clone() };
            // A product is what its data sheet says it is: a mark with no maker or number is
            // only a word on a page, unless its cut sheet shows a luminaire (a custom fixture's
            // shop drawing names its type and no maker).
            let luminaire = prop.get("category").and_then(Value::as_str) == Some("luminaire");
            if mark.is_empty() || (!luminaire && product.value(keys::MANUFACTURER).is_none() && product.value(keys::CATALOG_NUMBER).is_none()) {
                continue;
            }
            let page = [keys::CATALOG_NUMBER, keys::MANUFACTURER]
                .iter()
                .find_map(|k| product.field(k).and_then(|f| f.get("source")).and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            marks.push(mark.to_string());
            let doc = if page.is_empty() { name.clone() } else { format!("{name} {page}") };
            out.products.entry(mark.to_ascii_uppercase()).or_insert(Product { doc, ..product });
        }
        for q in r.get("rfis").and_then(Value::as_array).into_iter().flatten() {
            let mark = q.get("mark").and_then(Value::as_str).unwrap_or("").to_string();
            let text = q.get("description").and_then(Value::as_str).unwrap_or("").to_string();
            out.questions.push((mark, format!("{name}: {text}")));
        }
        out.read.push(json!({"file": name, "products": marks.len(), "marks": marks}));
    }
    out
}

/// The type marks the submittals at `paths` give products for (upper case).
#[derive(Default)]
pub(crate) struct ProductMarks {
    /// Every one: the device types they list, for a print set whose fixture schedule is only in
    /// the submittals.
    pub types: Vec<String>,
    /// The luminaires (by their cut sheets) that hang at the ceiling.
    pub ceiling: Vec<String>,
    /// The luminaires that go on a wall (an exit sign, a sconce, a wall pack).
    pub wall: Vec<String>,
}

pub(crate) fn product_marks(s: &mut Session, paths: &[String]) -> ProductMarks {
    let files = pdf_files(paths);
    let products = read_products(s, &files).products;
    let mut out = ProductMarks::default();
    for (mark, p) in &products {
        if p.proposal.get("category").and_then(Value::as_str) == Some("luminaire") {
            if p.value(keys::MOUNTING).is_some_and(|m| m.eq_ignore_ascii_case("wall")) {
                out.wall.push(mark.clone());
            } else {
                out.ceiling.push(mark.clone());
            }
        }
    }
    out.types = products.into_keys().collect();
    out
}

/// The type mark of a device, fixture or equipment element.
fn product_mark(e: &Entity) -> Option<String> {
    let kind = super::elemprops::element_kind(e)?;
    if !PRODUCT_KINDS.contains(&kind.as_str()) {
        return None;
    }
    let store = props::read(&e.common.xdata);
    let mark = store.get("type_mark").filter(|p| p.has_value())?.text();
    let mark = mark.trim();
    (!mark.is_empty()).then(|| mark.to_string())
}

/// The submittals at `paths`: each PDF (or a submittal's text, `.txt`) given, and those in each
/// folder given (not its subfolders), in name order.
fn pdf_files(paths: &[String]) -> Vec<String> {
    let is_pdf = |p: &std::path::Path| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf") || x.eq_ignore_ascii_case("txt"));
    let mut out = Vec::new();
    for given in paths {
        let path = std::path::Path::new(given.trim());
        if path.is_dir() {
            let mut inside: Vec<String> = std::fs::read_dir(path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && is_pdf(p))
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            inside.sort();
            out.extend(inside);
        } else if is_pdf(path) {
            out.push(given.trim().to_string());
        }
    }
    out.dedup();
    out.truncate(MAX_FILES);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, EntityKind, Space};
    use cadaddy_geom::Vec2;

    /// A device element of type `mark`, as the build makes them.
    fn device(s: &mut Session, mark: &str, at: f64) -> Handle {
        let mut m = super::super::wallgen::empty_mesh();
        super::super::wallgen::add_box(&mut m, Vec2::new(at, 0.0), Vec2::new(at + 4.0, 0.0), 2.0, 0.0, 4.0);
        let tags = super::super::wallgen::tag(cadaddy_walk3d::Part::Device, "Level 1", Handle(0));
        let mut store = Props::default();
        store.set("type_mark", Prop::new(mark, Source::print("E-1", None)));
        let xdata = props::write(&tags, &store);
        let d = s.doc_mut().unwrap();
        d.add(&Space::Model, Common { layer: "3D-Devices".into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).unwrap()
    }

    fn prop(s: &Session, h: Handle, key: &str) -> Option<Prop> {
        s.doc().ok().and_then(|d| d.model.get(h)).and_then(|e| props::read(&e.common.xdata).get(key).cloned())
    }

    #[test]
    fn each_element_of_a_type_gets_its_submittal_product() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        let a = device(&mut s, "Q7", 0.0);
        let b = device(&mut s, "Q7", 10.0);
        let other = device(&mut s, "Z9", 20.0);
        // A person's catalog number on one of them stays.
        {
            let d = s.doc_mut().unwrap();
            let mut store = props::read(&d.model.get(b).unwrap().common.xdata);
            store.set("catalog_number", Prop::new("MINE-1", Source::user()));
            d.modify_entity(b, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).unwrap();
        }
        let file = std::env::temp_dir().join(format!("cadaddy-submittal-{}.txt", std::process::id()));
        let text = "\
--- page 1 of 2 ---
Job Name : SOME JOB   Type : Q7
Manufacturer : ACME LIGHTING
Model Number : QX100E1/TRIM
--- page 2 of 2 ---
Job Name : SOME JOB   Type : Q7
QX100E1   4in downlight, 120V, LE/TE 5% Dim
--- page 3 of 3 ---
Type : Q8
Manufacturer : OTHER CO
Model Number : WRAP 48 840
";
        std::fs::write(&file, text).unwrap();
        let r = s.execute(CMD, &json!({"paths": [file.to_string_lossy()]})).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!(r["applied"]["Q7"], 2, "{r}");
        assert_eq!(r["dimming"]["Q7"], "phase");
        assert_eq!(r["no_submittal"]["Z9"], 1, "a type no submittal covers: {r}");
        assert_eq!(r["not_in_model"], json!(["Q8"]));
        let maker = prop(&s, a, "manufacturer").unwrap();
        assert_eq!(maker.text(), "ACME LIGHTING");
        assert_eq!(maker.source.kind, SourceKind::Submittal);
        assert!(maker.source.doc.as_deref().is_some_and(|d| d.ends_with("p.1")), "the page it says so on: {:?}", maker.source);
        assert_eq!(prop(&s, a, "dimming").unwrap().note.as_deref(), Some("ordering row for QX100E1"));
        assert_eq!(prop(&s, b, "catalog_number").unwrap().text(), "MINE-1", "a person's value stays");
        assert!(prop(&s, other, "manufacturer").is_none());
        assert!(s.doc().unwrap().bim_types.get("Q7").is_some(), "the product is in the type catalog");
    }

    #[test]
    fn the_product_types_are_the_marks_with_a_maker_or_number() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        let file = std::env::temp_dir().join(format!("cadaddy-submittal-types-{}.txt", std::process::id()));
        let text = "--- page 1 of 2 ---\nType : Q7A\nManufacturer : ACME LIGHTING\nModel Number : HX6D010\n--- page 2 of 2 ---\nType : Q9\nA shop drawing with no header\n";
        std::fs::write(&file, text).unwrap();
        let marks = product_marks(&mut s, &[file.to_string_lossy().to_string()]);
        std::fs::remove_file(&file).ok();
        assert_eq!(marks.types, ["Q7A"]);
        assert!(marks.ceiling.is_empty() && marks.wall.is_empty(), "its page says nothing of light");
    }

    #[test]
    fn no_paths_or_no_files_is_an_error() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        assert!(s.execute(CMD, &json!({})).is_err());
        assert!(s.execute(CMD, &json!({"paths": ["/no/such/folder"]})).is_err());
    }
}
