//! Submittals onto the model (`bim.submittals`).
//!
//! Once the architecture is built and the MEP objects are made, the submittals say exactly what
//! each object is. Every element of a type mark gets its submittal product as properties: maker,
//! catalog number, how it dims, and which submittal says so. The source is the submittal and
//! page. The products also go into the drawing's type catalog. A person's values stay.
//!
//! A dimmer then takes the way the fixtures on its leg dim (`dimming`: 0-10V, phase, DALI, or
//! `mixed` where its leg's fixtures dim two ways, a question; `bim.dimming`), worked out from
//! their submittals, unless its own product or a person says.

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
/// Products of one mark kept (one per category).
const MAX_CATEGORIES: usize = 8;
/// The element kinds a submittal product goes on (doors and walls have type marks of their own).
const PRODUCT_KINDS: &[&str] = &["device", "light_fixture", "plumbing_fixture", "fixture", "equipment"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Apply Submittals", run)
            .params("{paths: [path] (submittal PDFs, or folders of them)} → {products: {mark: {manufacturer, catalog_number, header ({named, why}: a page header naming a product its own cut sheet doesn't show: not taken, said as the elements' submittal_header), category, quantity, doc}}, files: [{file, products, marks}], applied: {mark: elements}, elements, catalog, dimming: {mark: control}, dimmers: {protocol: n}, no_submittal: {mark: elements}, not_in_model: [mark], questions: [{mark, text}], unread: [{file, error}]}: each device, fixture and equipment element of a type mark gets its submittal product (manufacturer, catalog_number, dimming, and what its cut sheet says of voltage, finish, mounting_type, cct, cri, lumens, wattage; submittal), with the submittal and page as the source; the products go into the type catalog; each dimmer takes the dimming of the fixtures on its leg (mixed: a question); a person's values stay; luminaire_boxes_removed / luminaire_boxes_needed: a luminaire whose cut sheet gives it its own junction box (canless, remote driver, integral) loses the outlet box the build gave it (one that now needs one gets it at the next build)")
            .menu(&["Tools", "Building", "Apply Submittals"]),
    ]
}

/// A submittal's product for one type mark.
struct Product {
    /// The submittal it is in (the file's name) and the page its data starts on.
    doc: String,
    /// What `bim.spec_import` proposed for it.
    proposal: Value,
    /// Its category as the submittal categories say it ([`category_of`]; empty: not said).
    category: &'static str,
}

impl Product {
    fn field(&self, key: &str) -> Option<&Value> {
        self.proposal.get("fields").and_then(|f| f.get(key)).filter(|f| f.get("value").and_then(Value::as_str).is_some_and(|v| !v.trim().is_empty()))
    }
    fn value(&self, key: &str) -> Option<String> {
        self.field(key).and_then(|f| f.get("value")).and_then(Value::as_str).map(|v| v.trim().to_string())
    }
    /// The maker and catalog number its page header named that its own cut sheet doesn't show
    /// (Q6336), and why it isn't taken: `("OTHERCO ZQ50RB", "page header only: …")`.
    fn header(&self) -> Option<(String, String)> {
        use super::spec_import::{CUT_SHEET_CODES, HEADER_CATALOG, HEADER_MANUFACTURER};
        let cat = self.value(HEADER_CATALOG)?;
        let maker = self.value(HEADER_MANUFACTURER).or_else(|| self.value(keys::MANUFACTURER)).unwrap_or_default();
        let shows = self.value(CUT_SHEET_CODES).map(|c| format!(" (it shows {c})")).unwrap_or_default();
        Some((
            format!("{maker} {cat}").trim().to_string(),
            format!(
                "page header only: its own cut sheet doesn't show {cat}{shows}; not taken as the product (G-DOC-ORDER: the cut sheet outranks its page header)"
            ),
        ))
    }
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    // `paths` a list of paths, `path` one: anything else is refused, not taken for none.
    let mut given: Vec<String> = Vec::new();
    match p.get("paths") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for v in a.iter().take(MAX_FILES) {
                let Some(path) = v.as_str() else { return Err(bad(CMD, format!("`paths` holds paths (strings), not {v}"))) };
                given.push(path.to_string());
            }
        }
        Some(v) => return Err(bad(CMD, format!("`paths` is a list of paths ([\"a.pdf\", \"folder\"]), not {v}; `path` takes one"))),
    }
    match p.get("path") {
        None | Some(Value::Null) => {}
        Some(Value::String(one)) => given.push(one.clone()),
        Some(v) => return Err(bad(CMD, format!("`path` is a path (a string), not {v}"))),
    }
    // Each path given is there (a file or a folder): a missing one is named, not skipped.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let missing: Vec<&str> = given.iter().map(|g| g.trim()).filter(|g| !g.is_empty() && !std::path::Path::new(g).exists()).collect();
        if !missing.is_empty() {
            return Err(bad(CMD, format!("not found: {}", missing.join(", "))));
        }
    }
    // From the menu, with nothing chosen: what it needs, not an error.
    if given.is_empty() {
        s.echo("Apply Submittals: give the submittal PDFs, or a folder of them (`paths`).");
        return Ok(json!({
            "files": [], "applied": {}, "elements": 0,
            "next_step": "bim.submittals {paths: [\"submittals folder or PDF\"]}: each device, fixture and equipment element of a type mark gets its submittal product",
        }));
    }
    let files = pdf_files(&given);
    if files.is_empty() {
        return Err(bad(CMD, format!("no PDF at {}", given.join(", "))));
    }

    let Read { products, read, unread, mut questions } = read_products(s, &files);

    // The model's elements by type mark, each with its category: a type mark is unique only
    // within a category (luminaire type A, panel A), so a product goes only on its own.
    let d = s.doc()?;
    let mut by_mark: BTreeMap<String, Vec<(Handle, &'static str)>> = BTreeMap::new();
    // Every entity is looked at (a print set's linework comes first), the elements kept capped.
    let mut kept = 0usize;
    for e in d.model.iter() {
        if kept >= MAX_ELEMENTS {
            break;
        }
        if let Some((mark, category)) = product_element(e) {
            by_mark.entry(mark.to_ascii_uppercase()).or_default().push((e.handle, category));
            kept += 1;
        }
    }

    // Which product each element takes: its own category's.
    let mut picks: BTreeMap<&String, Pick> = BTreeMap::new();
    for (mark, elems) in &by_mark {
        let Some(ps) = products.get(mark) else { continue };
        let pick = pick_products(ps, elems);
        if let Some(q) = pick.question(mark, ps, elems) {
            questions.push((mark.clone(), q));
        }
        picks.insert(mark, pick);
    }

    // The products of the model's types into the catalog: one per mark (the catalog is by mark
    // alone), and none over a type the catalog holds as another category.
    let mut accepted: Vec<Value> = Vec::new();
    let mut catalog_skipped: Vec<Value> = Vec::new();
    for (mark, pick) in &picks {
        let Some(ps) = products.get(*mark) else { continue };
        let used: Vec<&Product> = ps.iter().enumerate().filter(|(i, _)| pick.picks.contains(&Some(*i))).map(|(_, p)| p).collect();
        let held = d.bim_types.get(mark.as_str()).map(|t| category_of(&t.category)).unwrap_or("");
        match used.as_slice() {
            [] => {}
            [one] if held.is_empty() || one.category.is_empty() || held == one.category => accepted.push(one.proposal.clone()),
            [one] => catalog_skipped.push(json!({"mark": mark, "why": format!("the type catalog holds {mark} as {held}; its {} product stays on its elements only", one.category)})),
            _ => catalog_skipped.push(json!({"mark": mark, "why": format!("{mark} is a type of {} categories: the type catalog (by mark alone) takes none of them; each stays on its own elements", used.len())})),
        }
    }
    let catalog = if accepted.is_empty() {
        0
    } else {
        s.execute("bim.spec_accept", &json!({"proposals": accepted}))?.get("accepted").and_then(Value::as_u64).unwrap_or(0)
    };

    // Each element of a type: what its own category's submittal says it is.
    let d = s.doc()?;
    let mut changes: Vec<(Handle, Props)> = Vec::new();
    let mut applied: BTreeMap<String, usize> = BTreeMap::new();
    let mut dimming: BTreeMap<String, String> = BTreeMap::new();
    let mut other_category: BTreeMap<String, BTreeMap<&'static str, usize>> = BTreeMap::new();
    for (mark, elems) in &by_mark {
        let (Some(ps), Some(pick)) = (products.get(mark), picks.get(mark)) else { continue };
        if !pick.other.is_empty() {
            other_category.insert(mark.clone(), pick.other.clone());
        }
        let sets: Vec<ProductProps> = ps.iter().map(product_props).collect();
        for ((h, _), chosen) in elems.iter().zip(&pick.picks) {
            let Some(e) = d.model.get(*h) else { continue };
            let mut store = props::read(&e.common.xdata);
            let before = store.clone();
            // What a product of another category put on it before (its mark matched alone) goes.
            for (j, p) in ps.iter().enumerate() {
                if *chosen != Some(j) {
                    drop_product(&mut store, &p.doc);
                }
            }
            if let Some((product, set)) = chosen.and_then(|i| ps.get(i).zip(sets.get(i))) {
                for (key, prop) in &set.props {
                    // A person's value stays.
                    if store.get(key).is_none_or(|old| old.source.kind != SourceKind::User) {
                        store.set(key, prop.clone());
                    }
                }
                if set.header {
                    // What an earlier reading took from that header goes.
                    for (key, field) in [("catalog_number", keys::CATALOG_NUMBER), ("manufacturer", keys::MANUFACTURER)] {
                        if product.value(field).is_none() && store.get(key).is_some_and(|old| old.source.kind == SourceKind::Submittal) {
                            store.remove(key);
                        }
                    }
                } else if store.get("submittal_header").is_some_and(|old| old.source.kind == SourceKind::Submittal) {
                    store.remove("submittal_header");
                }
                if let Some(control) = &set.dimming {
                    dimming.entry(mark.clone()).or_insert_with(|| control.clone());
                }
                *applied.entry(mark.clone()).or_insert(0) += 1;
            }
            if store != before {
                changes.push((*h, store));
            }
        }
    }
    let d = s.doc_mut()?;
    let mut changed = 0usize;
    for (h, store) in changes {
        if d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).is_ok() {
            changed += 1;
        }
    }
    // The pieces follow: a luminaire whose cut sheet gives it its own junction box loses the
    // outlet box the build gave it.
    let (boxes_removed, boxes_needed) =
        if changed > 0 && d.embedded.contains_key(super::mepatoms::INDEX_FILE) { super::mepatoms::build::sync_luminaire_boxes(d) } else { (0, 0) };

    // Each dimmer: the way the fixtures on its leg dim now, unless its own product or a person
    // says. One worked out before and no longer settled goes.
    let d = s.doc()?;
    let mut dimmers: BTreeMap<String, usize> = BTreeMap::new();
    let mut changes: Vec<(Handle, Props)> = Vec::new();
    for x in super::dimcheck::dimmer_protocols(d) {
        *dimmers.entry(x.protocol.clone()).or_default() += 1;
        let Some(e) = d.model.get(x.handle) else { continue };
        let mut store = props::read(&e.common.xdata);
        if store.get("dimming").is_some_and(|p| matches!(p.source.kind, SourceKind::User | SourceKind::Submittal)) {
            continue;
        }
        let before = store.clone();
        if x.settled() || x.protocol == "mixed" {
            store.set("dimming", Prop::new(x.protocol.clone(), Source::computed()).with_note(&x.why));
        } else {
            store.remove("dimming");
        }
        if store != before {
            changes.push((x.handle, store));
        }
    }
    let d = s.doc_mut()?;
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
    // Every product read, in the model or not (the lighting step checks the submittal's types
    // against the schedule's).
    let listed: serde_json::Map<String, Value> = products
        .iter()
        .take(MAX_LISTED)
        .filter_map(|(m, ps)| {
            let (first, rest) = ps.split_first()?;
            let mut j = product_json(first);
            // The same mark's products of other categories (a luminaire type A, a panel A).
            if !rest.is_empty() {
                j["also"] = rest.iter().map(product_json).collect::<Vec<_>>().into();
            }
            Some((m.clone(), j))
        })
        .collect();
    let mut out = json!({
        "products": listed,
        "files": read,
        "applied": applied,
        "elements": changed,
        "catalog": catalog,
        "dimming": dimming,
        "dimmers": dimmers,
        "no_submittal": no_submittal,
        "not_in_model": not_in_model,
        "questions": questions,
        "unread": unread,
        "luminaire_boxes_removed": boxes_removed,
        "luminaire_boxes_needed": boxes_needed,
    });
    // Elements of a submitted mark whose own category no submittal covers (a panel A beside
    // luminaire type A's cut sheet), and the products the catalog (by mark alone) didn't take.
    out["other_category"] = json!(other_category);
    out["catalog_skipped"] = json!(catalog_skipped);
    Ok(out)
}

/// What the submittal files say.
struct Read {
    /// The products by mark (upper case): for each category, the first submittal giving a mark
    /// a maker or number.
    products: BTreeMap<String, Vec<Product>>,
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
            let product = Product {
                doc: name.clone(),
                proposal: prop.clone(),
                category: category_of(prop.get("category").and_then(Value::as_str).unwrap_or("")),
            };
            // A product is what its data sheet says it is: a mark with no maker or number is
            // only a word on a page, unless its cut sheet shows a luminaire (a custom fixture's
            // shop drawing names its type and no maker).
            let luminaire = prop.get("category").and_then(Value::as_str) == Some("luminaire");
            if mark.is_empty()
                || (!luminaire
                    && product.value(keys::MANUFACTURER).is_none()
                    && product.value(keys::CATALOG_NUMBER).is_none()
                    && product.header().is_none())
            {
                continue;
            }
            let page = [keys::CATALOG_NUMBER, keys::MANUFACTURER, super::spec_import::HEADER_CATALOG]
                .iter()
                .find_map(|k| product.field(k).and_then(|f| f.get("source")).and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            marks.push(mark.to_string());
            let doc = if page.is_empty() { name.clone() } else { format!("{name} {page}") };
            let same = out.products.entry(mark.to_ascii_uppercase()).or_default();
            if !same.iter().any(|p| p.category == product.category) && same.len() < MAX_CATEGORIES {
                same.push(Product { doc, ..product });
            }
        }
        for q in r.get("rfis").and_then(Value::as_array).into_iter().flatten() {
            let mark = q.get("mark").and_then(Value::as_str).unwrap_or("").to_string();
            let text = q.get("description").and_then(Value::as_str).unwrap_or("").to_string();
            out.questions.push((mark, format!("{name}: {text}")));
        }
        // A file that gave no product (empty, or no data sheet in it) is said, not passed over.
        if marks.is_empty() {
            out.unread.push(json!({"file": name, "error": "no product data read (no maker, catalog number or luminaire cut sheet)"}));
        }
        out.read.push(json!({"file": name, "products": marks.len(), "marks": marks}));
    }
    for u in &out.unread {
        s.echo(format!("Apply Submittals: {}: {}", u["file"].as_str().unwrap_or(""), u["error"].as_str().unwrap_or("")));
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
    for (mark, ps) in &products {
        if let Some(p) = ps.iter().find(|p| p.category == "luminaire") {
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

/// The type mark of a device, fixture or equipment element, and its category
/// ([`element_category`]).
fn product_element(e: &Entity) -> Option<(String, &'static str)> {
    let kind = super::elemprops::element_kind(e)?;
    if !PRODUCT_KINDS.contains(&kind.as_str()) {
        return None;
    }
    let store = props::read(&e.common.xdata);
    let mark = store.get("type_mark").filter(|p| p.has_value())?.text();
    let mark = mark.trim();
    (!mark.is_empty()).then(|| (mark.to_string(), element_category(e, &kind, &store)))
}

/// The submittal category of an element of kind `kind` (Q6588): a light fixture a `luminaire`,
/// a plumbing fixture `plumbing`, a piece of equipment `gear` where its words name electrical
/// gear (`submittals.element_category`), else `equipment`. A device or fixture that says
/// nothing more is of no known category (empty): any one product of its mark goes on it.
fn element_category(e: &Entity, kind: &str, store: &Props) -> &'static str {
    match kind {
        "light_fixture" => "luminaire",
        "plumbing_fixture" => "plumbing",
        "equipment" => {
            let mut words: Vec<String> =
                ["kind", "part", "title"].iter().filter_map(|k| super::bimlook::made_tag(e, k)).map(str::to_string).collect();
            words.extend(["kind", "type", "description", "name", "title", "equipment_type"].iter().filter_map(|k| store.get(k)).map(Prop::text));
            let text = words.join(" ").to_ascii_uppercase().replace([':', '_', '-'], " ");
            cadaddy_mepfix::wordrules::first_rule("submittals.element_category", |w| has_word(&text, w)).map_or("equipment", |r| r.value)
        }
        _ => "",
    }
}

/// A type's category (as a submittal, a schedule or a spec section read it) said as a submittal
/// category (`submittals.category_aliases`); empty where it says nothing of one.
pub(crate) fn category_of(category: &str) -> &'static str {
    let c = category.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    if c.is_empty() {
        return "";
    }
    cadaddy_mepfix::wordrules::pair_value("submittals.category_aliases", &c).unwrap_or("")
}

/// The submittal category of a CSI section (`26 51 19`: `luminaire`;
/// `submittals.section_category`, the first whose number starts it).
pub(crate) fn section_category(section: &str) -> Option<&'static str> {
    let s = section.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() {
        return None;
    }
    cadaddy_mepfix::wordrules::pairs("submittals.section_category")
        .iter()
        .find(|(w, _)| s == *w || s.strip_prefix(*w).is_some_and(|rest| rest.starts_with(' ')))
        .map(|(_, v)| *v)
}

/// Whether upper-case `text` has `word` (a word or phrase) whole: not inside a longer word.
pub(crate) fn has_word(text: &str, word: &str) -> bool {
    let w = word.trim().to_ascii_uppercase();
    if w.is_empty() {
        return false;
    }
    let b = text.as_bytes();
    text.match_indices(w.as_str()).any(|(i, _)| {
        let before = i.checked_sub(1).and_then(|j| b.get(j)).is_none_or(|c| !c.is_ascii_alphanumeric());
        let after = b.get(i + w.len()).is_none_or(|c| !c.is_ascii_alphanumeric());
        before && after
    })
}

/// Which product of a mark each of its elements takes (Q6588).
struct Pick {
    /// Per element (in order): the index of its product, if it takes one.
    picks: Vec<Option<usize>>,
    /// Elements that take none because their mark's products could be either's.
    ambiguous: usize,
    /// Elements of a category none of the mark's products is (a panel beside a luminaire type).
    other: BTreeMap<&'static str, usize>,
}

/// Each element takes its own category's product; a product of no stated category goes on the
/// elements no product of their own covers when they are of one category (or none known), and
/// is a question when they are of two; an element of no known category takes its mark's one
/// product. A mark is never matched alone across categories.
fn pick_products(ps: &[Product], elems: &[(Handle, &'static str)]) -> Pick {
    let unknown = ps.iter().position(|p| p.category.is_empty());
    let exact = |c: &str| ps.iter().position(|p| !c.is_empty() && p.category == c);
    // The categories the unknown product would go on: those no product of their own covers.
    let falls: std::collections::BTreeSet<&str> = elems.iter().map(|(_, c)| *c).filter(|c| !c.is_empty() && exact(c).is_none()).collect();
    let unknown_ok = unknown.is_some() && falls.len() <= 1;
    let mut out = Pick { picks: Vec::with_capacity(elems.len()), ambiguous: 0, other: BTreeMap::new() };
    for (_, c) in elems {
        let pick = if c.is_empty() {
            match ps {
                [one] if !one.category.is_empty() || unknown_ok => Some(0),
                _ => None,
            }
        } else {
            exact(c).or(if unknown_ok { unknown } else { None })
        };
        if pick.is_none() {
            if c.is_empty() || unknown.is_some() {
                out.ambiguous += 1;
            } else {
                *out.other.entry(*c).or_default() += 1;
            }
        }
        out.picks.push(pick);
    }
    out
}

impl Pick {
    /// The question a mark its submittals leave open between categories.
    fn question(&self, mark: &str, ps: &[Product], elems: &[(Handle, &'static str)]) -> Option<String> {
        if self.ambiguous == 0 {
            return None;
        }
        let mut cats: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, c) in elems {
            *cats.entry(if c.is_empty() { "of no known category" } else { c }).or_default() += 1;
        }
        let cats: Vec<String> = cats.iter().map(|(c, n)| format!("{n} {c}")).collect();
        let prods: Vec<String> = ps
            .iter()
            .map(|p| format!("{} ({})", if p.category.is_empty() { "a product of no stated category" } else { p.category }, p.doc))
            .collect();
        Some(format!(
            "type {mark}: its elements are {} and the submittals give {mark} as {}: a type mark is unique only within a category, so {} of them take no product; say which each is (its category, or the submittal's section)",
            cats.join(", "),
            prods.join(", "),
            self.ambiguous
        ))
    }
}

/// The properties a product gives each element it goes on.
struct ProductProps {
    props: Vec<(&'static str, Prop)>,
    /// Its page header is contradicted by its cut sheet (Q6336).
    header: bool,
    /// How it dims.
    dimming: Option<String>,
}

fn product_props(product: &Product) -> ProductProps {
    let src = Source::from_doc(SourceKind::Submittal, &product.doc);
    let mut set: Vec<(&'static str, Prop)> = Vec::new();
    if let Some(v) = product.value(keys::MANUFACTURER) {
        set.push(("manufacturer", Prop::new(v, src.clone())));
    }
    if let Some(v) = product.value(keys::CATALOG_NUMBER) {
        set.push(("catalog_number", Prop::new(v, src.clone())));
    }
    let dimming = product.value(keys::DIMMING);
    if let Some(control) = &dimming {
        let f = product.field(keys::DIMMING);
        // "p.3 (ordering row for X)": how it was found.
        let how =
            f.and_then(|f| f.get("source")).and_then(Value::as_str).and_then(|s| s.split_once('(')).map(|(_, h)| h.trim_end_matches(')').to_string());
        let inferred = f.and_then(|f| f.get("confidence")).and_then(Value::as_str).is_some_and(|c| c != "stated");
        let note = match (how, inferred) {
            (Some(h), true) => format!("inferred: {h}"),
            (Some(h), false) => h,
            (None, true) => "inferred".into(),
            (None, false) => String::new(),
        };
        let prop = Prop::new(control.clone(), src.clone());
        set.push(("dimming", if note.is_empty() { prop } else { prop.with_note(&note) }));
    }
    // What its cut sheet says of it besides (a luminaire's voltage, finish, mounting, CCT,
    // CRI, lumens, wattage).
    for (key, prop_key) in [
        (keys::VOLTS, "voltage"),
        (keys::FINISH, "finish"),
        (keys::MOUNTING, "mounting_type"),
        (keys::CCT, "cct"),
        (keys::LUMENS, "lumens"),
        (keys::WATTAGE, "wattage"),
        (super::spec_import::CRI, "cri"),
        (super::spec_import::JUNCTION_BOX, "junction_box"),
    ] {
        if let Some(v) = product.value(key) {
            set.push((prop_key, Prop::new(v, src.clone())));
        }
    }
    set.push(("submittal", Prop::new(product.doc.clone(), src.clone())));
    // A page header its own cut sheet contradicts (Q6336) is said beside the element, never
    // built: the maker and catalog number it named stay off (the schedule's stand).
    let header = product.header();
    if let Some((named, why)) = &header {
        set.push(("submittal_header", Prop::new(named.clone(), src.clone()).with_note(why)));
    }
    ProductProps { props: set, header: header.is_some(), dimming }
}

/// Takes off an element what product `doc` put on it (its values sourced to that submittal
/// page): a product of another category its mark alone gave it before (Q6588).
fn drop_product(store: &mut Props, doc: &str) {
    let Some(doc) = Source::from_doc(SourceKind::Submittal, doc).doc else { return };
    let keys: Vec<String> = store
        .iter()
        .filter(|(_, p)| p.source.kind == SourceKind::Submittal && p.source.doc.as_deref() == Some(doc.as_str()))
        .map(|(k, _)| k.to_string())
        .collect();
    for k in keys {
        store.remove(&k);
    }
}

/// A product as the report lists it.
fn product_json(p: &Product) -> Value {
    json!({
        "manufacturer": p.value(keys::MANUFACTURER).unwrap_or_default(),
        "catalog_number": p.value(keys::CATALOG_NUMBER).unwrap_or_default(),
        "header": p.header().map(|(named, why)| json!({"named": named, "why": why})),
        "category": p.proposal.get("category").and_then(Value::as_str).unwrap_or_default(),
        "quantity": p.value(super::spec_import::QUANTITY).and_then(|q| q.parse::<u32>().ok()),
        "doc": p.doc,
    })
}

/// The PDFs in the folders under `dir` (not in it) whose path names a product package
/// ([`PRODUCT_PACKAGE_WORDS`]), four folders deep at most.
fn product_packages(dir: &std::path::Path, out: &mut Vec<String>, depth: usize) {
    if depth > 4 || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            product_packages(&p, out, depth + 1);
        } else if depth > 0 && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
            let up = p.to_string_lossy().to_ascii_uppercase();
            if cadaddy_mepfix::wordrules::words("equipment.submittal_package_words").iter().any(|w| up.contains(w))
                && !up.contains("_NET.PDF")
                && out.len() < MAX_FILES
            {
                out.push(p.to_string_lossy().to_string());
            }
        }
    }
}

pub(crate) fn pdf_files(paths: &[String]) -> Vec<String> {
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
            // Below it (a project's export by package and revision): the product packages only.
            let mut below = Vec::new();
            product_packages(path, &mut below, 0);
            below.sort();
            inside.extend(below);
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

    /// Q6336: a type whose page header names a product its own cut sheet doesn't show gets neither
    /// that maker nor that catalog number (one an earlier reading put there goes); the header is
    /// said beside it, and the type is still a submitted product.
    #[test]
    fn a_page_header_its_cut_sheet_contradicts_is_said_not_applied() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        let a = device(&mut s, "Q4", 0.0);
        {
            let d = s.doc_mut().unwrap();
            let mut store = props::read(&d.model.get(a).unwrap().common.xdata);
            store.set("catalog_number", Prop::new("ZQ50RB", Source::from_doc(SourceKind::Submittal, "Old Package p.1")));
            store.set("manufacturer", Prop::new("OTHERCO LIGHTING", Source::from_doc(SourceKind::Submittal, "Old Package p.1")));
            d.modify_entity(a, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).unwrap();
        }
        let file = std::env::temp_dir().join(format!("cadaddy-submittal-header-{}.txt", std::process::id()));
        let text = "\
--- page 1 of 1 ---
Job Name : SOME JOB   Type : Q4
Manufacturer : OTHERCO LIGHTING
Model Number : ZQ50RB
Brightco Duo battery units throw two beams of light down a corridor when the power goes out.
Each head turns and tilts on its own, and a test button on the front checks the battery every month.
Mount it high on a wall or under a soffit; it ships with a knockout plate and a hanging bracket.
Ordering guide   Example: EU2BK
EU2 Duo twin head battery unit   B Lithium cells   K Black   4M Remote heads
";
        std::fs::write(&file, text).unwrap();
        let r = s.execute(CMD, &json!({"paths": [file.to_string_lossy()]})).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!(r["applied"]["Q4"], 1, "still the type's submittal: {r}");
        assert_eq!(r["products"]["Q4"]["catalog_number"], "", "{r}");
        assert_eq!(r["products"]["Q4"]["header"]["named"], "OTHERCO LIGHTING ZQ50RB", "{r}");
        assert!(prop(&s, a, "catalog_number").is_none(), "the header's number, read before, goes");
        assert!(prop(&s, a, "manufacturer").is_none(), "and its maker");
        let h = prop(&s, a, "submittal_header").unwrap();
        assert_eq!((h.text(), h.source.kind), ("OTHERCO LIGHTING ZQ50RB".to_string(), SourceKind::Submittal));
        assert!(h.note.as_deref().is_some_and(|n| n.contains("page header only") && n.contains("EU2")), "{:?}", h.note);
        assert!(r["questions"].as_array().is_some_and(|q| q.iter().any(|q| q["mark"] == "Q4")), "a question: {r}");
    }

    /// A luminaire or a switch element of type `mark` on leg `leg` of room 101.
    fn on_leg(s: &mut Session, mark: &str, item: &str, leg: &str, at: f64) -> Handle {
        let h = device(s, mark, at);
        let d = s.doc_mut().unwrap();
        let mut store = props::read(&d.model.get(h).unwrap().common.xdata);
        store.set("room", Prop::new("101", Source::computed()));
        store.set("switch_leg", Prop::new(leg, Source::print("E-1", None)));
        if !item.is_empty() {
            store.set("type", Prop::new(item, Source::print("E-1", None)));
        }
        d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).unwrap();
        h
    }

    #[test]
    fn each_dimmer_takes_the_dimming_of_the_fixtures_on_its_leg() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        on_leg(&mut s, "Q7", "", "a", 0.0);
        let a = on_leg(&mut s, "SD", "switch, dimmer", "a", 10.0);
        on_leg(&mut s, "Q7A", "", "b", 20.0);
        let b = on_leg(&mut s, "SD", "switch, dimmer", "b", 30.0);
        on_leg(&mut s, "Q7", "", "c", 40.0);
        on_leg(&mut s, "Q7A", "", "c", 50.0);
        let c = on_leg(&mut s, "SD", "switch, dimmer", "c", 60.0);
        // A person's word on one stays; a plain switch takes none.
        let mine = on_leg(&mut s, "SD", "switch, dimmer", "a", 70.0);
        {
            let d = s.doc_mut().unwrap();
            let mut store = props::read(&d.model.get(mine).unwrap().common.xdata);
            store.set("dimming", Prop::new("ELV", Source::user()));
            d.modify_entity(mine, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).unwrap();
        }
        let plain = on_leg(&mut s, "S3", "switch, 3-way", "b", 80.0);
        let file = std::env::temp_dir().join(format!("cadaddy-submittal-dimmers-{}.txt", std::process::id()));
        let text = "--- page 1 of 2 ---\nType : Q7\nManufacturer : ACME LIGHTING\nModel Number : QX100E1\nQX100E1   4in downlight, 120V, LE/TE 5% Dim\n--- page 2 of 2 ---\nType : Q7A\nManufacturer : ACME LIGHTING\nModel Number : HX6D010\nHX6D010   4in downlight, 120-277V 0-10V 5% Dim\n";
        std::fs::write(&file, text).unwrap();
        let r = s.execute(CMD, &json!({"paths": [file.to_string_lossy()]})).unwrap();
        std::fs::remove_file(&file).ok();
        assert_eq!((r["dimming"]["Q7"].clone(), r["dimming"]["Q7A"].clone()), (json!("phase"), json!("0-10V")), "{r}");
        assert_eq!(r["dimmers"], json!({"0-10V": 1, "ELV": 1, "mixed": 1, "phase": 1}), "{r}");
        let pa = prop(&s, a, "dimming").unwrap();
        assert_eq!((pa.text(), pa.source.kind), ("phase".to_string(), SourceKind::Computed));
        assert_eq!(pa.note.as_deref(), Some("its leg's fixtures: phase Q7"));
        assert_eq!(prop(&s, b, "dimming").unwrap().text(), "0-10V");
        assert_eq!(prop(&s, c, "dimming").unwrap().text(), "mixed", "a question, said on the dimmer");
        assert_eq!(prop(&s, mine, "dimming").unwrap().text(), "ELV", "a person's value stays");
        assert!(prop(&s, plain, "dimming").is_none(), "a 3-way switch doesn't dim");
    }

    /// An element of type `mark` with the build's tags `extra` (an element kind, a part).
    fn element(s: &mut Session, mark: &str, made: &str, extra: &[&str], at: f64) -> Handle {
        let mut m = super::super::wallgen::empty_mesh();
        super::super::wallgen::add_box(&mut m, Vec2::new(at, 0.0), Vec2::new(at + 4.0, 0.0), 2.0, 0.0, 4.0);
        let t = |v: &str| cadaddy_doc::RawTag { code: 1000, value: v.to_string() };
        let mut tags = vec![cadaddy_doc::RawTag { code: 1001, value: super::super::wallgen::APP.into() }, t(made), t("Level 1"), t(&Handle(0).hex())];
        tags.extend(extra.iter().map(|x| t(x)));
        let mut store = Props::default();
        store.set("type_mark", Prop::new(mark, Source::print("E-1", None)));
        let xdata = props::write(&tags, &store);
        let d = s.doc_mut().unwrap();
        d.add(&Space::Model, Common { layer: "3D-MEP".into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).unwrap()
    }

    fn luminaire(s: &mut Session, mark: &str, at: f64) -> Handle {
        element(s, mark, "device", &["element:light_fixture"], at)
    }

    fn panel(s: &mut Session, tag: &str, at: f64) -> Handle {
        element(s, tag, "mep", &["kind:equipment", "part:equipment:panel", &format!("tag:{tag}")], at)
    }

    fn write(name: &str, text: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cadaddy-submittal-cat-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{name}.txt"));
        std::fs::write(&file, text).unwrap();
        file
    }

    /// Two luminaire cut sheets, types A and B, as a lighting submittal prints them.
    const LIGHTING: &str = "\
--- page 1 of 2 ---
Job Name : SOME JOB   Type : A
Manufacturer : ACME LIGHTING
Model Number : QX100E1
QX100E1   4in LED downlight, 120V, 1000 lumens, 3000K CCT, 90 CRI, LE/TE 5% Dim
--- page 2 of 2 ---
Job Name : SOME JOB   Type : B
Manufacturer : ACME LIGHTING
Model Number : STRIP4 840
4ft LED strip luminaire, 4000K CCT, 80 CRI, 4400 lumens, 0-10V dimming
";

    /// Q6588: luminaire types and panelboards both use single letters. A lighting submittal's
    /// type A goes on luminaires of type A, never on panelboard A; what an earlier mark-only
    /// reading put on the panel goes.
    #[test]
    fn a_lighting_submittal_goes_on_luminaires_not_panels_of_the_same_letter() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        let la = luminaire(&mut s, "A", 0.0);
        let lb = luminaire(&mut s, "B", 10.0);
        let pa = panel(&mut s, "A", 20.0);
        let pb = panel(&mut s, "B", 30.0);
        let file = write("lighting", LIGHTING);
        // An earlier reading (by mark alone) put luminaire A's product on panel B... A's page.
        {
            let d = s.doc_mut().unwrap();
            let mut store = props::read(&d.model.get(pb).unwrap().common.xdata);
            let src = Source::from_doc(SourceKind::Submittal, "lighting p.2");
            store.set("manufacturer", Prop::new("ACME LIGHTING", src.clone()));
            store.set("dimming", Prop::new("0-10V", src.clone()));
            store.set("submittal", Prop::new("lighting p.2", src));
            store.set("rating", Prop::new("225A", Source::print("E-1", None)));
            d.modify_entity(pb, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).unwrap();
        }
        let r = s.execute(CMD, &json!({"paths": [file.to_string_lossy()]})).unwrap();
        std::fs::remove_dir_all(file.parent().unwrap()).ok();
        assert_eq!(r["products"]["A"]["category"], "luminaire", "{r}");
        assert_eq!((r["applied"]["A"].clone(), r["applied"]["B"].clone()), (json!(1), json!(1)), "the luminaires only: {r}");
        assert_eq!(r["other_category"]["A"]["gear"], 1, "{r}");
        assert_eq!(prop(&s, la, "manufacturer").unwrap().text(), "ACME LIGHTING");
        assert_eq!(prop(&s, lb, "dimming").unwrap().text(), "0-10V");
        for (h, name) in [(pa, "A"), (pb, "B")] {
            for key in ["manufacturer", "catalog_number", "dimming", "cct", "submittal"] {
                assert!(prop(&s, h, key).is_none(), "panel {name} has no luminaire {key}: {:?}", prop(&s, h, key));
            }
        }
        assert_eq!(prop(&s, pb, "rating").unwrap().text(), "225A", "the panel's own values stay");
        assert_eq!(s.doc().unwrap().bim_types.get("A").map(|t| t.category.clone()).as_deref(), Some("luminaire"));
    }

    /// A product its submittal doesn't say the category of, whose mark is on elements of two
    /// categories, is a question: written to neither.
    #[test]
    fn a_mark_on_two_categories_with_an_unsaid_product_is_a_question() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        let la = luminaire(&mut s, "C", 0.0);
        let pc = panel(&mut s, "C", 10.0);
        let file = write("unsaid", "--- page 1 of 1 ---\nType : C\nManufacturer : SOME MAKER\nModel Number : ZX-9\nA product.\n");
        let r = s.execute(CMD, &json!({"paths": [file.to_string_lossy()]})).unwrap();
        std::fs::remove_dir_all(file.parent().unwrap()).ok();
        assert!(r["applied"].get("C").is_none(), "{r}");
        assert!(prop(&s, la, "manufacturer").is_none() && prop(&s, pc, "manufacturer").is_none());
        let q = r["questions"].as_array().unwrap().iter().find(|q| q["mark"] == "C").cloned().unwrap_or_default();
        assert!(q["text"].as_str().is_some_and(|t| t.contains("1 gear") && t.contains("1 luminaire")), "{r}");
        // One category only: it goes on them.
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        let only = panel(&mut s, "C", 0.0);
        let file = write("unsaid-one", "--- page 1 of 1 ---\nType : C\nManufacturer : SOME MAKER\nModel Number : ZX-9\nA product.\n");
        let r = s.execute(CMD, &json!({"paths": [file.to_string_lossy()]})).unwrap();
        std::fs::remove_dir_all(file.parent().unwrap()).ok();
        assert_eq!(r["applied"]["C"], 1, "{r}");
        assert_eq!(prop(&s, only, "manufacturer").unwrap().text(), "SOME MAKER");
    }

    /// A gear submittal (its CSI section, or a panelboard cut sheet) and a lighting submittal
    /// both giving type A: each goes on its own category's elements.
    #[test]
    fn a_gear_and_a_lighting_product_of_one_mark_each_go_on_their_own() {
        for gear in [
            "--- page 1 of 1 ---\n26 24 16   PANELBOARDS\nType : A\nManufacturer : VOLTCO\nModel Number : PNL-42\n",
            "--- page 1 of 1 ---\nType : A\nManufacturer : VOLTCO\nModel Number : PNL-42\nLighting and appliance panelboard, main lugs only, 42 circuits\n",
        ] {
            let mut s = Session::new();
            s.execute("new", &json!({})).unwrap();
            let la = luminaire(&mut s, "A", 0.0);
            let pa = panel(&mut s, "A", 10.0);
            let lighting = write("lighting2", LIGHTING);
            let gear_file = write("gear2", gear);
            let r = s.execute(CMD, &json!({"paths": [lighting.to_string_lossy(), gear_file.to_string_lossy()]})).unwrap();
            std::fs::remove_dir_all(lighting.parent().unwrap()).ok();
            std::fs::remove_dir_all(gear_file.parent().unwrap()).ok();
            assert_eq!(r["applied"]["A"], 2, "{r}");
            assert_eq!(prop(&s, la, "manufacturer").unwrap().text(), "ACME LIGHTING", "{r}");
            assert_eq!(prop(&s, pa, "manufacturer").unwrap().text(), "VOLTCO", "{r}");
            assert_eq!(prop(&s, pa, "catalog_number").unwrap().text(), "PNL-42");
            assert!(prop(&s, pa, "dimming").is_none() && prop(&s, pa, "cct").is_none(), "{r}");
            assert_eq!(r["products"]["A"]["also"][0]["catalog_number"], "PNL-42", "{r}");
            assert!(r["catalog_skipped"].as_array().is_some_and(|c| c.iter().any(|c| c["mark"] == "A")), "the catalog is by mark alone: {r}");
        }
    }

    #[test]
    fn categories_are_rule_data() {
        assert_eq!(section_category("26 51 19"), Some("luminaire"));
        assert_eq!(section_category("26  24 16.13"), Some("gear"));
        assert_eq!(section_category("26 5"), None);
        assert_eq!(section_category("23 34 23"), Some("equipment"));
        assert_eq!(category_of("Switchboard"), "gear");
        assert_eq!(category_of("electrical"), "");
        assert!(has_word("TYPE A PANELBOARD", "panelboard") && !has_word("PANELBOARDS", "PANELBOARD") && !has_word("SUBPANEL", "PANEL"));
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
    fn no_paths_says_what_it_needs_and_no_files_is_an_error() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        // From the menu, nothing chosen: what it needs, not an error (menu commands never demand
        // parameters).
        let r = s.execute(CMD, &json!({})).unwrap();
        assert_eq!(r["elements"], json!(0));
        assert!(r["next_step"].as_str().is_some_and(|n| n.contains("paths")), "{r}");
        assert!(s.execute(CMD, &json!({"paths": ["/no/such/folder"]})).is_err_and(|e| e.to_string().contains("/no/such/folder")));
        // Wrong-typed paths are refused, not taken for none.
        for bad in [json!({"paths": "a.pdf"}), json!({"paths": 12345}), json!({"paths": [12]}), json!({"path": 7})] {
            assert!(s.execute(CMD, &bad).is_err(), "{bad}");
        }
        // A file that gives no product is named.
        let dir = std::env::temp_dir().join(format!("cadaddy-submittals-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty.txt");
        std::fs::write(&empty, "").unwrap();
        if let Ok(r) = s.execute(CMD, &json!({"paths": [empty.to_string_lossy()]})) {
            assert!(r["unread"].as_array().is_some_and(|u| u.iter().any(|x| x["file"] == json!("empty"))), "{r}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
