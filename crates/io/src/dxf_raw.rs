//! What a DXF file holds that CADaddy does not model, kept as raw groups so that saving writes
//! it back (#528): entities and objects of other types, other applications' xdata, extension
//! dictionaries, persistent reactors, colour book colours and other unmodelled entity properties.
//!
//! Reading ([`read`]) keeps the data with the file's handles in [`Drawing::foreign`], plus the
//! role of every handle it refers to that CADaddy writes itself (a table record, a block, the
//! named object dictionary…). Writing ([`finish`]) runs over the finished tag list: it places the
//! entities of types CADaddy doesn't model among the written ones (the entity writer leaves them
//! out), puts the data back on the records that still exist and points each reference at what now
//! has the role it named. Kept objects get new handles above the written ones (so they never
//! collide with the handles the writer hands out). A reference to something that is gone is
//! dropped (from reactor lists and dictionaries) or written as the null handle `0`; objects whose
//! owner is gone go with it.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cadaddy_doc::foreign::{Foreign, ForeignExtra, ForeignKey, ForeignObject};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, RawTag, Unknown};
use cadaddy_dxf::{Section, Tag};

/// Named object dictionary entries CADaddy writes itself from its model (plus every key that
/// starts with `CADADDY`): they are read into the model, not kept raw. A change that starts
/// modelling another entry adds it here. (An entry the writer writes is also never written
/// twice: the kept one is left out.)
const MODELLED_KEYS: &[&str] = &[
    "ACAD_GROUP",
    "ACAD_LAYOUT",
    "ACAD_MATERIAL",
    "ACAD_MLEADERSTYLE",
    "ACAD_MLINESTYLE",
    "ACAD_PLOTSTYLENAME",
    "ACAD_SCALELIST",
    "ACAD_TABLESTYLE",
    "ACAD_IMAGE_DICT",
    "ACAD_IMAGE_VARS",
    "ACAD_PDFDEFINITIONS",
    "ACAD_DWFDEFINITIONS",
    "DWGPROPS",
];

/// Extension dictionary entries not kept raw: dimension associativity, fields and XCLIP
/// boundaries (`ACAD_FILTER`; CADaddy reads and rewrites all three), draw order and annotation scale contexts
/// (written from their own models by the changes that model them; stale copies would contradict
/// the drawing).
const MODELLED_XDICT_KEYS: &[&str] = &["ACAD_DIMASSOC", "ACAD_FIELD", "ACAD_FILTER", "ACAD_SORTENTS", "ACDBCONTEXTDATAMANAGER"];

/// Applications whose xdata CADaddy reads and writes itself. `ACAD` holds dimension overrides
/// (DSTYLE) on dimensions and dimension styles; elsewhere it is kept.
const OWN_APPS: &[&str] = &["CADADDY", "ACADANNOTATIVE", "ACCMTRANSPARENCY", "ACAECLAYERSTANDARD"];

/// Most objects kept from one file.
const MAX_OBJECTS: usize = 1_000_000;
/// Most xdata groups kept per record (AutoCAD allows 16 KB of xdata per object).
const MAX_XDATA_GROUPS: usize = 16_384;
/// Longest ownership chain followed.
const MAX_DEPTH: usize = 64;

/// Record types that structure a file: never written as an entity of another type.
const STRUCTURAL: &[&str] = &["", "SECTION", "ENDSEC", "EOF", "TABLE", "ENDTAB", "BLOCK", "ENDBLK", "SEQEND", "VERTEX", "ATTRIB", "CLASS"];

fn up(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

/// A handle as text, normalised (`002a` → `2A`); other text upper-cased.
fn norm(s: &str) -> String {
    Handle::parse_hex(s).map_or_else(|| up(s), Handle::hex)
}

fn raw(t: &Tag) -> RawTag {
    RawTag { code: t.code, value: t.str() }
}

/// A kept group with the value type its code has (so binary output encodes it right); text that
/// doesn't parse as that type stays text.
fn tag(r: &RawTag) -> Tag {
    use cadaddy_dxf::{Kind, Value, kind_of};
    let v = r.value.trim();
    let value = match kind_of(r.code) {
        Kind::Real => v.parse::<f64>().ok().filter(|x| x.is_finite()).map(Value::Real),
        Kind::Int16 | Kind::Int32 | Kind::Int64 => v.parse::<i64>().ok().map(Value::Int),
        Kind::Bool => v.parse::<i64>().ok().map(|x| Value::Bool(x != 0)),
        Kind::Hex => Some(Value::Hex(v.to_string())),
        Kind::Str => None,
    };
    Tag::new(r.code, value.unwrap_or_else(|| Value::Str(r.value.clone().into())))
}

/// A handle value we believe: hexadecimal and not null. DXF handles are up to 16 hex digits and
/// AutoCAD writes some above 2^48 (e.g. 0xDAD1…), so the whole 64-bit range is believed.
fn handle(s: &str) -> Option<Handle> {
    Handle::parse_hex(s).filter(|h| h.0 != 0)
}

/// Group codes whose values are handles of other objects (DXF Reference, group code ranges).
fn is_pointer(code: i32) -> bool {
    matches!(code, 320..=369 | 390..=399 | 480..=481 | 1005)
}

/// The records of a run of tags: (type, index of the record's `0` group, groups after it).
fn split(tags: &[Tag]) -> Vec<(String, usize, &[Tag])> {
    let mut out = Vec::new();
    let mut open: Option<(String, usize)> = None;
    for (i, t) in tags.iter().enumerate() {
        if t.code == 0 {
            if let Some((k, s)) = open.take() {
                out.push((k, s, tags.get(s + 1..i).unwrap_or(&[])));
            }
            open = Some((t.str(), i));
        }
    }
    if let Some((k, s)) = open {
        out.push((k, s, tags.get(s + 1..).unwrap_or(&[])));
    }
    out
}

type Recs<'a> = Vec<(String, usize, &'a [Tag])>;

/// A record's handle, owner, reactors and extension dictionary (groups before its xdata).
#[derive(Default)]
struct Head {
    handle: Option<String>,
    owner: Option<String>,
    reactors: Vec<String>,
    xdict: Option<String>,
    has_reactors: bool,
}

fn head(tags: &[Tag]) -> Head {
    let mut h = Head::default();
    let mut group: Option<String> = None;
    for t in tags.iter().take_while(|t| t.code < 1000) {
        match (t.code, group.as_deref()) {
            (102, _) => {
                let v = t.str();
                group = v.trim().strip_prefix('{').map(str::to_string);
                h.has_reactors |= group.as_deref() == Some("ACAD_REACTORS");
            }
            (_, Some("ACAD_REACTORS")) => h.reactors.push(norm(&t.str())),
            (_, Some("ACAD_XDICTIONARY")) => {
                if h.xdict.is_none() {
                    h.xdict = Some(norm(&t.str()));
                }
            }
            (_, Some(_)) => {}
            (5 | 105, None) if h.handle.is_none() => h.handle = Some(norm(&t.str())),
            (330, None) if h.owner.is_none() => h.owner = Some(norm(&t.str())),
            _ => {}
        }
    }
    h
}

fn val(tags: &[Tag], code: i32) -> Option<String> {
    tags.iter().take_while(|t| t.code < 1000).find(|t| t.code == code).map(Tag::str)
}

/// A dictionary's entries: (key, upper-case handle).
fn entries(tags: &[Tag]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut key: Option<String> = None;
    for t in tags.iter().take_while(|t| t.code < 1000) {
        match t.code {
            3 => key = Some(t.str()),
            350 | 360 => {
                if let Some(k) = key.take() {
                    out.push((k, norm(&t.str())));
                }
            }
            _ => {}
        }
    }
    out
}

fn is_dictionary(kind: &str) -> bool {
    kind == "DICTIONARY" || kind == "ACDBDICTIONARYWDFLT"
}

fn modelled_key(key: &str) -> bool {
    let k = up(key);
    k.starts_with("CADADDY") || MODELLED_KEYS.contains(&k.as_str())
}

/// The roles of a file's handles (see [`ForeignKey`]): table records and tables by type and
/// name, blocks by name, the named object dictionary, its entries, and their entries.
fn identities(tables: &Recs, blocks: &Recs, objects: &Recs) -> HashMap<String, ForeignKey> {
    let mut out: HashMap<String, ForeignKey> = HashMap::new();
    let mut put = |h: Option<String>, k: ForeignKey| {
        if let Some(h) = h {
            out.entry(h).or_insert(k);
        }
    };
    for (kind, _, t) in tables {
        let name = up(&val(t, 2).unwrap_or_default());
        if kind == "TABLE" || !name.is_empty() {
            put(head(t).handle, (if kind == "TABLE" { "TABLE".into() } else { kind.clone() }, name));
        }
    }
    let mut block = String::new();
    for (kind, _, t) in blocks {
        match kind.as_str() {
            "BLOCK" => {
                block = up(&val(t, 2).unwrap_or_default());
                put(head(t).handle, ("BLOCK".into(), block.clone()));
            }
            "ENDBLK" => put(head(t).handle, ("ENDBLK".into(), block.clone())),
            _ => {}
        }
    }
    let Some((_, _, root)) = objects.first().filter(|(k, _, _)| is_dictionary(k)) else { return out };
    put(head(root).handle, ("NOD".into(), String::new()));
    let by_handle: HashMap<String, (&str, &[Tag])> = objects.iter().filter_map(|(k, _, t)| Some((head(t).handle?, (k.as_str(), *t)))).rev().collect();
    for (key, h) in entries(root) {
        put(Some(h.clone()), ("NOD".into(), up(&key)));
        if let Some((k, t)) = by_handle.get(&h)
            && is_dictionary(k)
        {
            for (name, eh) in entries(t) {
                put(Some(eh), (format!("NOD/{}", up(&key)), up(&name)));
            }
        }
    }
    out
}

/// Other applications' xdata of a record of type `kind`: whole applications, each with valid
/// groups (codes 1000 and up, balanced `1002` braces), up to [`MAX_XDATA_GROUPS`].
fn foreign_xdata(kind: &str, tags: &[Tag]) -> Vec<RawTag> {
    let Some(start) = tags.iter().position(|t| t.code == 1001) else { return Vec::new() };
    let rest = tags.get(start..).unwrap_or(&[]);
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(first) = rest.get(i) {
        let len = rest.get(i + 1..).map_or(0, |r| r.iter().position(|t| t.code == 1001).unwrap_or(r.len()));
        let chunk = rest.get(i..i + 1 + len).unwrap_or(&[]);
        i += 1 + len;
        let app = up(&first.str());
        let own = OWN_APPS.contains(&app.as_str()) || (app == "ACAD" && matches!(kind, "DIMENSION" | "DIMSTYLE"));
        if app.is_empty() || own || out.len() + chunk.len() > MAX_XDATA_GROUPS {
            continue;
        }
        let mut depth = 0i32;
        let mut ok = true;
        for t in chunk.iter().skip(1) {
            match (t.code, t.str().trim()) {
                (c, _) if c < 1000 => ok = false,
                (1002, "{") => depth += 1,
                (1002, "}") => {
                    depth -= 1;
                    ok &= depth >= 0;
                }
                _ => {}
            }
        }
        if ok && depth == 0 {
            out.extend(chunk.iter().map(raw));
        }
    }
    out
}

/// The groups of an entity's AcDbEntity subclass (after `100 AcDbEntity`, up to the next `100`).
fn entity_groups(tags: &[Tag]) -> &[Tag] {
    let Some(start) = tags.iter().position(|t| t.code == 100 && t.str().trim() == "AcDbEntity") else { return &[] };
    let rest = tags.get(start + 1..).unwrap_or(&[]);
    let end = rest.iter().position(|t| t.code == 100 || t.code >= 1000).unwrap_or(rest.len());
    rest.get(..end).unwrap_or(&[])
}

/// What an entity record (`entity`) or a table record carries beyond the model; reactors and
/// the extension dictionary only when kept (`kept`).
fn extra(kind: &str, tags: &[Tag], entity: bool, unknown: bool, kept: &HashSet<String>) -> ForeignExtra {
    let h = head(tags);
    let mut x = ForeignExtra {
        xdict: h.xdict.filter(|x| kept.contains(x)).and_then(|x| handle(&x)),
        reactors: h.reactors.iter().filter(|r| kept.contains(*r)).filter_map(|r| handle(r)).collect(),
        xdata: foreign_xdata(kind, tags),
        ..ForeignExtra::default()
    };
    // An unknown entity's own groups are written as they are; of table records, layers have a
    // colour book colour. (Materials, group 347, are modelled: read and written by the model.)
    if !unknown && (entity || kind == "LAYER") {
        let props =
            if entity { entity_groups(tags) } else { tags.get(..tags.iter().position(|t| t.code >= 1000).unwrap_or(tags.len())).unwrap_or(&[]) };
        let book = props.iter().find(|t| t.code == 430).map(Tag::str);
        let rgb = props.iter().find(|t| t.code == 420).map(Tag::str);
        if let (Some(book), Some(rgb)) = (book, rgb)
            && !book.trim().is_empty()
        {
            x.color_book = Some((rgb, book));
        }
        x.props = props.iter().filter(|t| t.code == 284).map(raw).collect();
    }
    x
}

/// Every entity of the drawing (model space, layouts, blocks).
fn all_entities(d: &Drawing) -> impl Iterator<Item = &Entity> {
    std::iter::once(&d.model)
        .chain(d.layouts.iter().map(|l| &l.entities))
        .chain(d.blocks.values().map(|b| &b.entities))
        .flat_map(|s| s.iter().map(|e| e.as_ref()))
}

/// Give each entity the xdata read for it (`xdata` by entity handle).
fn put_xdata(d: &mut Drawing, mut xdata: HashMap<Handle, Vec<RawTag>>) {
    if xdata.is_empty() {
        return;
    }
    let mut give = |store: &mut cadaddy_doc::EntityStore| {
        let hs: Vec<Handle> = store.iter().map(|e| e.handle).filter(|h| xdata.contains_key(h)).collect();
        for h in hs {
            if let Some(xd) = xdata.remove(&h) {
                store.modify(h, |e| e.common.xdata = xd.into());
            }
        }
    };
    give(&mut d.model);
    for l in &mut d.layouts {
        give(&mut l.entities);
    }
    for b in d.blocks.values_mut() {
        give(&mut Arc::make_mut(b).entities);
    }
}

/// Keep what `secs` holds that the reader did not model in `d.foreign`.
pub(crate) fn read(secs: &[Section], d: &mut Drawing) {
    let get = |name: &str| secs.iter().find(|s| s.name == name).map(|s| split(&s.tags)).unwrap_or_default();
    let (classes, tables, blocks, entities, objects) = (get("CLASSES"), get("TABLES"), get("BLOCKS"), get("ENTITIES"), get("OBJECTS"));
    // The drawing's entities by handle: whether each is of a type CADaddy doesn't model.
    let mut ents: HashMap<String, bool> = HashMap::new();
    for e in all_entities(d) {
        ents.entry(e.handle.hex()).or_insert(matches!(e.kind, EntityKind::Unknown(_)));
    }
    let ids = identities(&tables, &blocks, &objects);
    // Extension dictionaries of entities and records: owner → dictionary.
    let mut xdict_of: HashMap<String, String> = HashMap::new();
    for (kind, _, t) in tables.iter().chain(&blocks).chain(&entities) {
        if kind == "ENDBLK" {
            continue;
        }
        let h = head(t);
        if let (Some(o), Some(x)) = (h.handle, h.xdict) {
            xdict_of.entry(o).or_insert(x);
        }
    }
    // Objects by handle: the first of each, and none that has an entity's handle.
    let mut by_handle: HashMap<String, usize> = HashMap::new();
    for (i, (_, _, t)) in objects.iter().enumerate() {
        if let Some(h) = head(t).handle.filter(|h| handle(h).is_some() && !ents.contains_key(h)) {
            by_handle.entry(h).or_insert(i);
        }
    }
    let root = objects.first().filter(|(k, _, _)| is_dictionary(k)).and_then(|(_, _, t)| head(t).handle);
    let root_entries = objects.first().filter(|(k, _, _)| is_dictionary(k)).map(|(_, _, t)| entries(t)).unwrap_or_default();
    let root_key = |h: &str| root_entries.iter().find(|(_, eh)| eh == h).map(|(k, _)| k.clone());
    // An extension dictionary of an entity or record (not of another object).
    let is_xdict = |k: usize| {
        let Some((_, _, t)) = objects.get(k) else { return false };
        let hd = head(t);
        let (Some(h), Some(o)) = (hd.handle, hd.owner) else { return false };
        !by_handle.contains_key(&o) && xdict_of.get(&o) == Some(&h) && (ents.contains_key(&o) || ids.contains_key(&o))
    };
    let modelled_entry = |t: &[Tag], h: &str| entries(t).iter().any(|(k, eh)| eh == h && MODELLED_XDICT_KEYS.contains(&up(k).as_str()));
    // Which objects to keep: those under a named object dictionary entry CADaddy doesn't model,
    // under the extension dictionary of an entity or record (minus modelled entries; one left
    // empty goes), or owned by an entity of a type CADaddy doesn't model. Memoized along each
    // ownership chain; cycles and over-long chains are not kept.
    let mut memo: HashMap<usize, bool> = HashMap::new();
    for i in 0..objects.len() {
        let mut path: Vec<usize> = Vec::new();
        let mut cur = i;
        let verdict = loop {
            if let Some(&v) = memo.get(&cur) {
                break v;
            }
            if path.contains(&cur) || path.len() >= MAX_DEPTH {
                break false;
            }
            path.push(cur);
            let Some((_, _, t)) = objects.get(cur) else { break false };
            let hd = head(t);
            let (Some(h), Some(o)) = (hd.handle, hd.owner) else { break false };
            if by_handle.get(&h) != Some(&cur) {
                break false;
            }
            if Some(&o) == root.as_ref() {
                break root_key(&h).is_some_and(|k| !modelled_key(&k));
            }
            if let Some(&j) = by_handle.get(&o) {
                if is_xdict(j) && objects.get(j).is_some_and(|p| modelled_entry(p.2, &h)) {
                    break false;
                }
                cur = j;
                continue;
            }
            if is_xdict(cur) {
                break entries(t).iter().any(|(k, _)| !MODELLED_XDICT_KEYS.contains(&up(k).as_str()));
            }
            break ents.get(&o) == Some(&true);
        };
        for p in path {
            memo.insert(p, verdict);
        }
    }
    let mut f = Foreign::default();
    let mut kept: HashSet<String> = HashSet::new();
    for (i, (kind, _, t)) in objects.iter().enumerate() {
        if memo.get(&i) != Some(&true) || f.objects.len() >= MAX_OBJECTS {
            continue;
        }
        let Some(h) = head(t).handle.and_then(|h| handle(&h)) else { continue };
        let mut tags: Vec<RawTag> = t.iter().map(raw).collect();
        if is_xdict(i) {
            strip_entries(&mut tags);
        }
        if head(t).owner == root
            && let Some(key) = root_key(&h.hex())
        {
            f.dictionary.push((key, h));
        }
        kept.insert(h.hex());
        f.objects.push(ForeignObject { handle: h, kind: kind.clone(), tags });
    }
    // Extras of entities, tables, table records and blocks.
    let mut xdata_of: HashMap<Handle, Vec<RawTag>> = HashMap::new();
    for (kind, _, t) in blocks.iter().chain(&entities) {
        if matches!(kind.as_str(), "BLOCK" | "ENDBLK") {
            continue;
        }
        let Some(h) = head(t).handle else { continue };
        let Some(&unknown) = ents.get(&h) else { continue };
        let mut x = extra(kind, t, true, unknown, &kept);
        let Some(hh) = handle(&h) else { continue };
        // Xdata belongs to the entity itself (it follows the entity's copies).
        let xd = std::mem::take(&mut x.xdata);
        if !xd.is_empty() {
            xdata_of.entry(hh).or_insert(xd);
        }
        if !x.is_empty() {
            f.entities.entry(hh).or_insert(x);
        }
    }
    put_xdata(d, xdata_of);
    for (kind, _, t) in tables.iter().chain(blocks.iter().filter(|(k, _, _)| k == "BLOCK")) {
        let Some(key) = head(t).handle.and_then(|h| ids.get(&h).cloned()) else { continue };
        let x = extra(kind, t, kind == "BLOCK", false, &kept);
        if !x.is_empty() {
            f.records.entry(key).or_insert(x);
        }
    }
    // The classes of kept objects and of entities of types CADaddy doesn't model.
    let mut types: HashSet<String> = f.objects.iter().map(|o| up(&o.kind)).collect();
    types.extend(all_entities(d).filter_map(|e| match &e.kind {
        EntityKind::Unknown(u) => Some(up(&u.dxf_type)),
        _ => None,
    }));
    // A proxy's class is the one its group 91 numbers in the file's CLASSES: keep it too, and the
    // file's class order, by which saving renumbers the proxy.
    let class_order: Vec<String> =
        classes.iter().filter(|(k, _, _)| k == "CLASS").take(MAX_OBJECTS).map(|(_, _, t)| up(&val(t, 1).unwrap_or_default())).collect();
    let mut proxies = false;
    let proxy_tags = f.objects.iter().filter(|o| dxf_proxy(&o.kind, o.tags.iter().map(|r| (r.code, r.value.as_str())))).map(|o| &o.tags);
    let proxy_tags = proxy_tags.chain(all_entities(d).filter_map(|e| match &e.kind {
        EntityKind::Unknown(u) if dxf_proxy(&u.dxf_type, u.tags.iter().map(|r| (r.code, r.value.as_str()))) => Some(&u.tags),
        _ => None,
    }));
    for tags in proxy_tags {
        proxies = true;
        if let Some(name) = class_index(class_number(tags.iter().map(|r| (r.code, r.value.as_str())))).and_then(|i| class_order.get(i)) {
            types.insert(name.clone());
        }
    }
    if proxies {
        f.class_order = class_order;
    }
    for (kind, _, t) in &classes {
        if kind == "CLASS" && val(t, 1).is_some_and(|n| types.contains(&up(&n))) && f.classes.len() < MAX_OBJECTS {
            f.classes.push(t.iter().map(raw).collect());
        }
    }
    // The roles of the handles kept data refers to.
    let mut refs: Vec<String> = Vec::new();
    let mut note = |tags: &[RawTag]| refs.extend(tags.iter().filter(|r| is_pointer(r.code)).map(|r| norm(&r.value)));
    for o in &f.objects {
        note(&o.tags);
    }
    for x in f.entities.values().chain(f.records.values()) {
        note(&x.xdata);
        note(&x.props);
    }
    for e in all_entities(d) {
        note(&e.common.xdata);
    }
    for e in all_entities(d) {
        if let EntityKind::Unknown(u) = &e.kind {
            note(&u.tags);
        }
    }
    for r in &refs {
        if let (Some(h), Some(k), false) = (handle(r), ids.get(r), kept.contains(r)) {
            f.targets.insert(h, k.clone());
        }
    }
    // The data store (ACDSDATA): the ACIS geometry of solids, regions and bodies from 2013 on.
    if let Some(ds) = secs.iter().find(|s| s.name.eq_ignore_ascii_case("ACDSDATA")) {
        f.data_store = ds.tags.iter().take(MAX_DATA_STORE_GROUPS).map(raw).collect();
    }
    if !f.is_empty() {
        d.foreign = Arc::new(f);
    }
}

/// Proxy records (DXF Reference, ACAD_PROXY_OBJECT and ACAD_PROXY_ENTITY): another application's
/// objects and entities, whose class is named by number in group 91.
fn is_proxy(kind: &str) -> bool {
    matches!(up(kind).as_str(), "ACAD_PROXY_OBJECT" | "ACAD_PROXY_ENTITY")
}

/// Whether a record is a proxy as a DXF holds one (its class numbered by group 91), not one the
/// native DWG reader kept as it was stored ([`cadaddy_dwgcodec::entities::RAW_SUBCLASS`]): that
/// one's class is its DWG type code and its data the stored bytes, so it has no group 91 to
/// renumber and is written back as it is.
fn dxf_proxy<S: AsRef<str>>(kind: &str, groups: impl IntoIterator<Item = (i32, S)>) -> bool {
    is_proxy(kind) && !groups.into_iter().any(|(code, v)| code == 100 && v.as_ref().trim() == cadaddy_dwgcodec::entities::RAW_SUBCLASS)
}

/// Where a proxy's class number is: its first group 91 outside the `102 {NAME` … `102 }` groups
/// (an application's can hold a 91 of its own) and before any xdata.
fn class_group<S: AsRef<str>>(groups: impl IntoIterator<Item = (i32, S)>) -> Option<usize> {
    let mut in_group = false;
    for (i, (code, value)) in groups.into_iter().enumerate() {
        match code {
            1000.. => return None,
            102 => in_group = value.as_ref().trim().starts_with('{'),
            91 if !in_group => return Some(i),
            _ => {}
        }
    }
    None
}

/// A proxy's class number (see [`class_group`]).
fn class_number<S: AsRef<str>>(groups: impl IntoIterator<Item = (i32, S)> + Clone) -> Option<i64> {
    let i = class_group(groups.clone())?;
    groups.into_iter().nth(i).and_then(|(_, v)| v.as_ref().trim().parse().ok())
}

/// The place in CLASSES a class number names (500 is the first class).
fn class_index(number: Option<i64>) -> Option<usize> {
    usize::try_from(number?.checked_sub(500)?).ok()
}

/// Set a proxy's class number (see [`class_group`]).
fn set_class_number(tags: &mut [Tag], n: i64) {
    if let Some(t) = class_group(tags.iter().map(|t| (t.code, t.str()))).and_then(|i| tags.get_mut(i)) {
        *t = Tag::i(91, n);
    }
}

/// Most groups of a data store section kept (a damaged file's can be endless).
const MAX_DATA_STORE_GROUPS: usize = 1 << 26;

/// The drawing's kept data store section (ACDSDATA), written before the file's `EOF` without the
/// records of entities that are no longer written. Only for files of AutoCAD 2013 and later
/// (`modern`), whose solids, regions and bodies keep their geometry there; the DWG bridge reads it
/// too. Nothing is written when no record's entity remains. The records go by first
/// ([`Self::note`]: which owners were written), so the file is never held.
pub(crate) struct DataStore<'d> {
    ds: &'d [RawTag],
    /// The records' owners (normalised handles), and those written.
    owners: HashSet<String>,
    written: HashSet<String>,
}

impl<'d> DataStore<'d> {
    /// `None` when nothing is kept (or the file is older than 2013).
    pub(crate) fn of(d: &'d Drawing, modern: bool) -> Option<Self> {
        let ds = d.foreign.data_store.as_slice();
        if !modern || ds.is_empty() {
            return None;
        }
        let owners = cadaddy_doc::foreign::data_store_records(ds)
            .into_iter()
            .filter_map(cadaddy_doc::foreign::data_store_owner)
            .map(|t| norm(&t.value))
            .collect();
        Some(DataStore { ds, owners, written: HashSet::new() })
    }

    /// A written record's groups: their handles (group 5) that own data store records.
    pub(crate) fn note(&mut self, rec: &[Tag]) {
        for t in rec.iter().filter(|t| t.code == 5) {
            let h = norm(&t.str());
            if self.owners.contains(&h) {
                self.written.insert(h);
            }
        }
    }

    /// The section, once every record before the `EOF` was noted (empty when no record's entity
    /// was written).
    pub(crate) fn section(&self) -> Vec<Tag> {
        let ds = self.ds;
        // The section's own groups before its first record, then its schemas and records.
        let mut out: Vec<Tag> = vec![Tag::s(0, "SECTION"), Tag::s(2, "ACDSDATA")];
        let first = ds.iter().position(|t| t.code == 0).unwrap_or(ds.len());
        out.extend(ds.iter().take(first).map(tag));
        let mut records = 0usize;
        for rec in cadaddy_doc::foreign::data_store_records(ds) {
            let is_record = rec.first().is_some_and(|t| t.value.trim().eq_ignore_ascii_case("ACDSRECORD"));
            let owner = cadaddy_doc::foreign::data_store_owner(rec).map(|t| norm(&t.value));
            if !is_record || owner.as_ref().is_some_and(|h| self.written.contains(h)) {
                records += usize::from(is_record);
                out.extend(rec.iter().map(tag));
            }
        }
        if records == 0 {
            return Vec::new();
        }
        out.push(Tag::s(0, "ENDSEC"));
        out
    }
}

/// Drop a dictionary's modelled entries (key and handle) from its groups.
fn strip_entries(tags: &mut Vec<RawTag>) {
    let mut out = Vec::with_capacity(tags.len());
    let mut i = 0;
    while let Some(t) = tags.get(i) {
        if t.code == 3 && MODELLED_XDICT_KEYS.contains(&up(&t.value).as_str()) && tags.get(i + 1).is_some_and(|n| matches!(n.code, 350 | 360)) {
            i += 2;
            continue;
        }
        out.push(t.clone());
        i += 1;
    }
    *tags = out;
}

/// An entity of a type CADaddy doesn't model, as read, with its handle, owner, space and layer
/// from the drawing. Its reactors, extension dictionary and xdata are added by [`finish`] (they
/// are kept with the drawing's other foreign data); its references are remapped there too.
pub(crate) fn unknown_entity(u: &Unknown, e: &Entity, owner: &str, paper: bool) -> Vec<Tag> {
    let kind = up(&u.dxf_type);
    if STRUCTURAL.contains(&kind.as_str()) || u.dxf_type.trim() != u.dxf_type {
        return Vec::new();
    }
    let mut out = vec![Tag::s(0, u.dxf_type.clone()), Tag::s(5, e.handle.hex()), Tag::s(330, owner)];
    let fixed = out.len();
    let mut in_group = false;
    let mut owner_seen = false;
    let mut layer = false;
    let mut paper_done = !paper;
    for r in u.tags.iter().take_while(|r| r.code < 1000) {
        match r.code {
            102 => {
                in_group = r.value.trim().starts_with('{');
                continue;
            }
            _ if in_group => continue,
            5 | 67 => continue,
            330 if !owner_seen => {
                owner_seen = true;
                continue;
            }
            8 => {
                layer = true;
                out.push(Tag::s(8, e.common.layer.clone()));
                continue;
            }
            _ => {}
        }
        out.push(tag(r));
        if r.code == 100 && r.value.trim() == "AcDbEntity" && !paper_done {
            out.push(Tag::i(67, 1));
            paper_done = true;
        }
    }
    let mut lead = Vec::new();
    if !paper_done {
        lead.push(Tag::i(67, 1));
    }
    if !layer {
        lead.push(Tag::s(8, e.common.layer.clone()));
    }
    out.splice(fixed..fixed, lead);
    out
}

/// A written record: its section and type.
struct Rec {
    sec: String,
    kind: String,
}

/// Where kept entities of types CADaddy doesn't model are put back among the written records:
/// before a written entity (its handle), before the end of the ENTITIES section, or before the
/// ENDBLK of a block (its name, upper case).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Before {
    Entity(String),
    EntitiesEnd,
    EndBlk(String),
}

/// Follows a file's records as they go by: the section each is in, the block the last BLOCK
/// record named, and how many records of OBJECTS came so far.
#[derive(Default)]
struct Place {
    sec: String,
    block: String,
    objects: usize,
}

impl Place {
    /// Move past a record of type `kind` with groups `body` (after its `0`): its section, and
    /// whether it is the first record of the OBJECTS section.
    fn next(&mut self, kind: &str, body: &[Tag]) -> (String, bool) {
        if kind == "SECTION" {
            self.sec = body.iter().find(|t| t.code == 2).map(Tag::str).unwrap_or_default();
            self.objects = 0;
        }
        if kind == "BLOCK" {
            self.block = up(&val(body, 2).unwrap_or_default());
        }
        let sec = self.sec.clone();
        let mut first_object = false;
        if sec == "OBJECTS" && kind != "SECTION" && kind != "ENDSEC" {
            self.objects += 1;
            first_object = self.objects == 1;
        }
        if kind == "ENDSEC" {
            self.sec.clear();
        }
        (sec, first_object)
    }
}

/// The handles of the drawing's entities.
fn entity_handles(d: &Drawing) -> HashSet<u64> {
    all_entities(d).map(|e| e.handle.0).collect()
}

/// What putting the drawing's foreign data back ([`Finisher`]) needs to know of the written file,
/// gathered from its records as the writer makes them: the symbol tables, the block headers, the
/// first record of OBJECTS and its dictionaries, the classes, which of the drawing's entities
/// were written (and which of those with extras have an extension dictionary of their own) and
/// the largest handle. The entities' own groups are not kept, so the file is never held.
pub(crate) struct Scan<'d> {
    d: &'d Drawing,
    ents: HashSet<u64>,
    /// Entities the drawing keeps extras for (kept data or their own xdata).
    extras: HashSet<u64>,
    place: Place,
    tables: Vec<(String, Vec<Tag>)>,
    blocks: Vec<(String, Vec<Tag>)>,
    objects: Vec<(String, Vec<Tag>)>,
    classes: Vec<String>,
    at: HashSet<u64>,
    own_xdict: HashSet<u64>,
    max_handle: u64,
}

impl<'d> Scan<'d> {
    /// A scan for `d`; `None` when it has nothing to put back (no foreign data, no entity of a
    /// type CADaddy doesn't model, no entity xdata).
    pub(crate) fn of(d: &'d Drawing) -> Option<Self> {
        let f = d.foreign.as_ref();
        let unknown = all_entities(d).any(|e| matches!(e.kind, EntityKind::Unknown(_)));
        let xd = all_entities(d).any(|e| !e.common.xdata.is_empty());
        if f.is_empty() && !unknown && !xd {
            return None;
        }
        let mut extras: HashSet<u64> = f.entities.keys().map(|h| h.0).collect();
        extras.extend(all_entities(d).filter(|e| !e.common.xdata.is_empty()).map(|e| e.handle.0));
        Some(Scan {
            d,
            ents: entity_handles(d),
            extras,
            place: Place::default(),
            tables: Vec::new(),
            blocks: Vec::new(),
            objects: Vec::new(),
            classes: Vec::new(),
            at: HashSet::new(),
            own_xdict: HashSet::new(),
            max_handle: 0,
        })
    }

    /// One written record (its `0` group first).
    pub(crate) fn record(&mut self, rec: &[Tag]) {
        let Some(zero) = rec.first() else { return };
        let body = rec.get(1..).unwrap_or(&[]);
        let kind = zero.str();
        let (sec, first_object) = self.place.next(&kind, body);
        if let Some(m) = max_handle(body) {
            self.max_handle = self.max_handle.max(m);
        }
        let framing = matches!(kind.as_str(), "SECTION" | "ENDSEC");
        match sec.as_str() {
            "TABLES" if !framing => self.tables.push((kind, body.to_vec())),
            "BLOCKS" if matches!(kind.as_str(), "BLOCK" | "ENDBLK") => self.blocks.push((kind, body.to_vec())),
            "OBJECTS" if first_object || (!framing && is_dictionary(&kind)) => self.objects.push((kind, body.to_vec())),
            "CLASSES" if kind == "CLASS" => self.classes.extend(val(body, 1).map(|n| up(&n))),
            "ENTITIES" | "BLOCKS" if !framing => {
                let hd = head(body);
                if let Some(h) = hd.handle.as_deref().and_then(handle).map(|h| h.0).filter(|h| self.ents.contains(h)) {
                    self.at.insert(h);
                    if hd.xdict.is_some() && self.extras.contains(&h) {
                        self.own_xdict.insert(h);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Records a [`Scan`] kept, as [`identities`] takes them.
fn kept_recs(v: &[(String, Vec<Tag>)]) -> Recs<'_> {
    v.iter().map(|(k, t)| (k.clone(), 0, t.as_slice())).collect()
}

/// A record's handle (normalised), from its groups after the `0`.
pub(crate) fn record_handle(body: &[Tag]) -> Option<String> {
    head(body).handle
}

/// The largest handle a record's groups 5 and 105 hold (`None`: none).
pub(crate) fn max_handle(tags: &[Tag]) -> Option<u64> {
    tags.iter().filter(|t| t.code == 5 || t.code == 105).filter_map(|t| Handle::parse_hex(&t.str())).map(|h| h.0).max()
}

/// Remaps the kept data's handles onto a written file.
struct Map<'a> {
    f: &'a Foreign,
    /// Kept objects that are written: file handle → new handle.
    alive: HashMap<String, String>,
    /// Kept objects, written or not (file handles).
    raw: HashSet<String>,
    /// Role → handle in the written file.
    roles: HashMap<ForeignKey, String>,
    /// Entities in the written file.
    entities: HashSet<u64>,
}

impl Map<'_> {
    /// The written handle for a kept reference, `Some("0")` for the null handle, `None` when what
    /// it named is gone.
    fn get(&self, old: &str) -> Option<String> {
        let v = norm(old);
        if v.is_empty() || v == "0" {
            return Some("0".into());
        }
        if self.raw.contains(&v) {
            return self.alive.get(&v).cloned();
        }
        let h = handle(&v)?;
        if let Some(role) = self.f.targets.get(&h) {
            return self.roles.get(role).cloned();
        }
        self.entities.contains(&h.0).then_some(v)
    }

    /// A kept group with its handle remapped (a dangling one becomes null).
    fn group(&self, r: &RawTag) -> Tag {
        if is_pointer(r.code) { Tag::s(r.code, self.get(&r.value).unwrap_or_else(|| "0".into())) } else { tag(r) }
    }

    /// A `102 {NAME` group of the handles that still exist (nothing if none does).
    fn handle_group(&self, out: &mut Vec<Tag>, name: &str, code: i32, handles: &[String]) {
        let live: Vec<String> = handles.iter().filter_map(|h| self.get(h)).filter(|h| h != "0").collect();
        if live.is_empty() {
            return;
        }
        out.push(Tag::s(102, format!("{{{name}")));
        out.extend(live.into_iter().map(|h| Tag::s(code, h)));
        out.push(Tag::s(102, "}"));
    }

    /// A kept object's groups, remapped.
    fn object(&self, o: &ForeignObject, out: &mut Vec<Tag>) {
        out.push(Tag::s(0, o.kind.clone()));
        let dict = is_dictionary(&o.kind);
        let mut i = 0;
        while let Some(r) = o.tags.get(i) {
            i += 1;
            if r.code == 102 && r.value.trim().starts_with('{') {
                // A `{NAME` group is a handle list (`{ACAD_REACTORS 330 … }`) only when every
                // group inside it is a reference. Application data groups also use 102 brackets
                // with mixed values and their own closing form — the AcDs thumbnail schema's
                // `{ATTRRECORD 341 h 2 name 280 1 291 1 ATTRRECORD}` — and collapsing those into
                // a handle list rewrote the handle under the LAST code read (291, a bool) and
                // dropped the rest, which strict DXF readers reject. Those are written as read,
                // references remapped, by the plain path below.
                let end = (i..o.tags.len()).find(|&k| o.tags[k].code == 102).unwrap_or(o.tags.len());
                let inner = &o.tags[i..end];
                if !inner.is_empty() && !inner.iter().all(|n| is_pointer(n.code)) {
                    out.push(tag(r));
                    continue;
                }
                let name = r.value.trim().trim_start_matches('{').to_string();
                let mut hs = Vec::new();
                let mut code = 330;
                while let Some(n) = o.tags.get(i) {
                    if n.code == 102 {
                        if n.value.trim() == "}" {
                            i += 1;
                        }
                        break;
                    }
                    code = n.code;
                    hs.push(n.value.clone());
                    i += 1;
                }
                self.handle_group(out, &name, code, &hs);
                continue;
            }
            if dict && r.code == 3 && o.tags.get(i).is_some_and(|n| matches!(n.code, 350 | 360)) {
                let target = o.tags.get(i).map(|n| (n.code, self.get(&n.value)));
                i += 1;
                if let Some((code, Some(h))) = target.filter(|(_, h)| h.as_deref().is_some_and(|h| h != "0")) {
                    out.push(tag(r));
                    out.push(Tag::s(code, h));
                }
                continue;
            }
            if r.code == 5 {
                out.push(Tag::s(5, self.get(&r.value).unwrap_or_else(|| "0".into())));
                continue;
            }
            out.push(self.group(r));
        }
    }
}

/// The extras of a written record, if the drawing keeps any for it.
fn extra_for<'a>(f: &'a Foreign, xd: &XdataOf, rec: &Rec, body: &[Tag], block: &str) -> Option<Cow<'a, ForeignExtra>> {
    match (rec.sec.as_str(), rec.kind.as_str()) {
        ("BLOCKS", "BLOCK") => f.records.get(&("BLOCK".to_string(), block.to_string())).map(Cow::Borrowed),
        ("BLOCKS", "ENDBLK") => None,
        ("ENTITIES" | "BLOCKS", _) => entity_extra(f, xd, handle(&head(body).handle?)?),
        ("TABLES", kind) => {
            let name = up(&val(body, 2).unwrap_or_default());
            f.records.get(&(kind.to_string(), name)).map(Cow::Borrowed)
        }
        _ => None,
    }
}

/// Entities' own xdata ([`cadaddy_doc::Common::xdata`]), by handle.
type XdataOf<'a> = HashMap<Handle, &'a [RawTag]>;

/// The extras of entity `h`: what the drawing keeps for it, with the entity's own xdata.
fn entity_extra<'a>(f: &'a Foreign, xd: &XdataOf, h: Handle) -> Option<Cow<'a, ForeignExtra>> {
    let kept = f.entities.get(&h);
    match xd.get(&h) {
        None => kept.map(Cow::Borrowed),
        Some(xdata) => {
            let mut x = kept.cloned().unwrap_or_default();
            x.xdata = xdata.to_vec();
            Some(Cow::Owned(x))
        }
    }
}

/// Every Unknown entity's handle (upper case).
fn unknown_handles(d: &Drawing) -> HashSet<String> {
    all_entities(d).filter(|e| matches!(e.kind, EntityKind::Unknown(_))).map(|e| e.handle.hex()).collect()
}

/// Puts the drawing's foreign data back into the written file (see the module comment), one
/// record at a time ([`Finisher::record`]): planned from a [`Scan`] of the records, so the file is
/// never held. Each pass over the records starts with [`Finisher::rewind`].
pub(crate) struct Finisher<'d> {
    f: &'d Foreign,
    xd: XdataOf<'d>,
    map: Map<'d>,
    /// The written CLASSES as numbered (500 for the first), and the kept ones appended.
    numbers: HashMap<String, i64>,
    class_names: HashSet<String>,
    /// Entities of types CADaddy doesn't model, by where they go; the plan, and what is left of
    /// it in this pass.
    planned: HashMap<Before, Vec<(String, Vec<Tag>)>>,
    left: HashMap<Before, Vec<(String, Vec<Tag>)>>,
    place: Place,
    /// Reactors the writer planned for sources it doesn't write (a DIMASSOC on an entity of a
    /// type CADaddy doesn't model): written handles, by the source.
    reactors: HashMap<Handle, Vec<String>>,
}

impl<'d> Finisher<'d> {
    /// The plan for `d` from the scan of its written records; `seed` is the next free handle the
    /// writer left (the header's `$HANDSEED`), `reactors` the reactors it planned by source. A
    /// `modern` file (a DXF of 2013 or later) leaves out solids with SAT text (#2334).
    pub(crate) fn new(scan: Scan<'d>, seed: u64, modern: bool, reactors: HashMap<Handle, Vec<String>>) -> Self {
        let d = scan.d;
        let f = d.foreign.as_ref();
        let xd: XdataOf = all_entities(d).filter(|e| !e.common.xdata.is_empty()).map(|e| (e.handle, e.common.xdata.as_slice())).collect();
        let unknown = unknown_handles(d);
        let (tables, blocks, objects) = (kept_recs(&scan.tables), kept_recs(&scan.blocks), kept_recs(&scan.objects));
        let mut roles: HashMap<ForeignKey, String> = HashMap::new();
        for (h, k) in identities(&tables, &blocks, &objects) {
            roles.entry(k).or_insert(h);
        }
        // The drawing's entities that are written.
        let mut written: HashSet<u64> = scan.at.clone();
        // The written CLASSES as numbered (500 for the first): the writer's own, then the kept ones it
        // doesn't write (appended below, in this order). A kept proxy's group 91 is renumbered to its
        // class's place here; a proxy whose class isn't written is dropped (#1812).
        let class_names: HashSet<String> = scan.classes.iter().cloned().collect();
        let appended = f.classes.iter().filter_map(|c| c.iter().find(|t| t.code == 1)).map(|n| up(&n.value)).filter(|n| !class_names.contains(n));
        let mut numbers: HashMap<String, i64> = HashMap::new();
        for (i, n) in scan.classes.iter().cloned().chain(appended).enumerate() {
            numbers.entry(n).or_insert(500_i64.saturating_add(i64::try_from(i).unwrap_or(i64::MAX)));
        }
        let renumber = |old: Option<i64>| class_index(old).and_then(|i| f.class_order.get(i)).and_then(|n| numbers.get(n).copied());
        // Entities of types CADaddy doesn't model, placed among the written ones.
        let mut planned = plan_unknown(d, &scan, &roles, modern);
        for list in planned.values_mut() {
            list.retain_mut(|(_, t)| {
                if !t.first().is_some_and(|z| dxf_proxy(&z.str(), t.iter().map(|x| (x.code, x.str())))) {
                    return true;
                }
                let Some(n) = renumber(class_number(t.iter().map(|x| (x.code, x.str())))) else { return false };
                set_class_number(t, n);
                true
            });
        }
        for list in planned.values() {
            for (_, t) in list {
                written.extend(head(t.get(1..).unwrap_or(&[])).handle.as_deref().and_then(handle).map(|h| h.0));
            }
        }
        let root = scan.objects.first().filter(|(k, _)| is_dictionary(k)).map(|(_, t)| t.as_slice());
        let root_keys: HashSet<String> = root.map(|t| entries(t).into_iter().map(|(k, _)| up(&k)).collect()).unwrap_or_default();
        // Extension dictionaries that go on a written record (one without its own).
        let mut attached: HashSet<String> = HashSet::new();
        let tables_and_blocks = scan.tables.iter().map(|(k, t)| ("TABLES", k, t)).chain(scan.blocks.iter().map(|(k, t)| ("BLOCKS", k, t)));
        for (sec, kind, body) in tables_and_blocks {
            let block = if kind == "BLOCK" { up(&val(body, 2).unwrap_or_default()) } else { String::new() };
            let r = Rec { sec: sec.to_string(), kind: kind.clone() };
            if let Some(x) = extra_for(f, &xd, &r, body, &block)
                && let Some(xd) = x.xdict
                && head(body).xdict.is_none()
            {
                attached.insert(xd.hex());
            }
        }
        for &h in scan.extras.iter().filter(|h| scan.at.contains(h) && !scan.own_xdict.contains(h)) {
            attached.extend(entity_extra(f, &xd, Handle(h)).and_then(|x| x.xdict).map(Handle::hex));
        }
        for (_, t) in planned.values().flatten() {
            let x = head(t.get(1..).unwrap_or(&[])).handle.and_then(|h| handle(&h)).and_then(|h| entity_extra(f, &xd, h));
            attached.extend(x.and_then(|x| x.xdict).map(Handle::hex));
        }
        // Which kept objects are written: those whose owner chain ends at a written owner.
        let objs: HashMap<String, &ForeignObject> = f.objects.iter().map(|o| (o.handle.hex(), o)).collect();
        let dict_entries: HashMap<String, &str> = f.dictionary.iter().map(|(k, h)| (h.hex(), k.as_str())).collect();
        let nod = f.targets.iter().filter(|(_, k)| k.0 == "NOD" && k.1.is_empty()).map(|(h, _)| h.hex()).collect::<HashSet<_>>();
        let owners: HashMap<String, String> = f
            .objects
            .iter()
            .filter_map(|o| Some((o.handle.hex(), head(&o.tags.iter().take_while(|t| t.code < 1000).map(tag).collect::<Vec<_>>()).owner?)))
            .collect();
        let mut alive_memo: HashMap<String, bool> = HashMap::new();
        let proxy = |o: &ForeignObject| dxf_proxy(&o.kind, o.tags.iter().map(|r| (r.code, r.value.as_str())));
        for o in f.objects.iter().filter(|o| proxy(o) && renumber(class_number(o.tags.iter().map(|r| (r.code, r.value.as_str())))).is_none()) {
            alive_memo.insert(o.handle.hex(), false);
        }
        let is_written = |h: &str| handle(h).is_some_and(|h| written.contains(&h.0));
        for o in &f.objects {
            let mut path: Vec<String> = Vec::new();
            let mut cur = o.handle.hex();
            let verdict = loop {
                if let Some(&v) = alive_memo.get(&cur) {
                    break v;
                }
                if path.contains(&cur) || path.len() >= MAX_DEPTH {
                    break false;
                }
                path.push(cur.clone());
                let Some(owner) = owners.get(&cur).cloned() else { break false };
                if objs.contains_key(&owner) {
                    cur = owner;
                    continue;
                }
                if nod.contains(&owner) {
                    break dict_entries.get(&cur).is_some_and(|k| !root_keys.contains(&up(k))) && root.is_some();
                }
                if attached.contains(&cur) {
                    break true;
                }
                break unknown.contains(&owner) && is_written(&owner);
            };
            for p in path {
                alive_memo.insert(p, verdict);
            }
        }
        // New handles for the written kept objects, above every written handle and the seed.
        let top = scan.max_handle.max(seed);
        let mut next = seed.max(top.saturating_add(1));
        let mut alive: HashMap<String, String> = HashMap::new();
        for o in f.objects.iter().filter(|o| alive_memo.get(&o.handle.hex()) == Some(&true)) {
            alive.insert(o.handle.hex(), format!("{next:X}"));
            next = next.saturating_add(1);
        }
        let entities: HashSet<u64> = all_entities(d).map(|e| e.handle.0).filter(|h| written.contains(h)).collect();
        let map = Map { f, alive, raw: objs.keys().cloned().collect(), roles, entities };
        let left = planned.clone();
        Finisher { f, xd, map, numbers, class_names, planned, left, place: Place::default(), reactors }
    }

    /// Start another pass over the records.
    pub(crate) fn rewind(&mut self) {
        self.left = self.planned.clone();
        self.place = Place::default();
    }

    fn renumber(&self, old: Option<i64>) -> Option<i64> {
        class_index(old).and_then(|i| self.f.class_order.get(i)).and_then(|n| self.numbers.get(n).copied())
    }

    /// One written record (its `0` group first) with what goes with it into `out`: the kept
    /// classes before the end of CLASSES, the kept objects before the end of OBJECTS, the kept
    /// entities that go before it, then the record with its extras.
    pub(crate) fn record(&mut self, rec: &[Tag], out: &mut Vec<Tag>) {
        let Some(zero) = rec.first() else { return };
        let b = rec.get(1..).unwrap_or(&[]);
        let kind = zero.str();
        let (sec, first_object) = self.place.next(&kind, b);
        let f = self.f;
        let map = &self.map;
        if kind == "ENDSEC" && sec == "CLASSES" {
            for c in f.classes.iter().filter(|c| c.iter().find(|t| t.code == 1).is_some_and(|n| !self.class_names.contains(&up(&n.value)))) {
                out.push(Tag::s(0, "CLASS"));
                out.extend(c.iter().map(tag));
            }
        }
        if kind == "ENDSEC" && sec == "OBJECTS" {
            let proxy = |o: &ForeignObject| dxf_proxy(&o.kind, o.tags.iter().map(|r| (r.code, r.value.as_str())));
            for o in f.objects.iter().filter(|o| map.alive.contains_key(&o.handle.hex())) {
                let number = proxy(o).then(|| self.renumber(class_number(o.tags.iter().map(|r| (r.code, r.value.as_str()))))).flatten();
                match number {
                    Some(n) => {
                        let mut tags: Vec<Tag> = o.tags.iter().map(tag).collect();
                        set_class_number(&mut tags, n);
                        let o = ForeignObject { tags: tags.iter().map(raw).collect(), ..o.clone() };
                        map.object(&o, out);
                    }
                    None => map.object(o, out),
                }
            }
        }
        let before = match (sec.as_str(), kind.as_str()) {
            ("ENTITIES", "ENDSEC") => Some(Before::EntitiesEnd),
            ("BLOCKS", "ENDBLK") => Some(Before::EndBlk(self.place.block.clone())),
            ("ENTITIES" | "BLOCKS", k) if !matches!(k, "BLOCK" | "SECTION" | "ENDSEC") => head(b).handle.map(Before::Entity),
            _ => None,
        };
        if let Some(list) = before.and_then(|k| self.left.remove(&k)) {
            for (sec, t) in list {
                let (Some(zero), Some(ub)) = (t.first(), t.get(1..)) else { continue };
                let ur = Rec { sec, kind: zero.str() };
                let x = head(ub).handle.and_then(|h| handle(&h)).and_then(|h| entity_extra(f, &self.xd, h));
                // Reactors the writer planned for this source (a DIMASSOC, an associative hatch).
                let extra = head(ub).handle.and_then(|h| handle(&h)).and_then(|h| self.reactors.get(&h)).map_or(&[][..], Vec::as_slice);
                out.push(zero.clone());
                record(map, &ur, ub, x.as_deref(), Some(extra), false, out);
            }
        }
        out.push(zero.clone());
        let r = Rec { sec, kind };
        let root = first_object && is_dictionary(&r.kind);
        let x = extra_for(f, &self.xd, &r, b, &self.place.block);
        if x.is_none() && !root {
            out.extend(b.iter().cloned());
            return;
        }
        record(map, &r, b, x.as_deref(), None, root, out);
    }
}

/// Where the entities of types CADaddy doesn't model go: each before the next written entity of
/// its space or block (the drawing's order), else at the end of its space or block. A `modern`
/// file leaves out solids with SAT text ([`cadaddy_doc::foreign::sat_text_acis`]).
fn plan_unknown(d: &Drawing, scan: &Scan, roles: &HashMap<ForeignKey, String>, modern: bool) -> HashMap<Before, Vec<(String, Vec<Tag>)>> {
    let stored = d.foreign.data_store_entities();
    let mut out: HashMap<Before, Vec<(String, Vec<Tag>)>> = HashMap::new();
    let written = |e: &Entity| scan.at.contains(&e.handle.0);
    let endblk = |name: &str| {
        let i = scan.blocks.iter().position(|(k, t)| k == "BLOCK" && val(t, 2).is_some_and(|n| up(&n) == up(name)))?;
        scan.blocks.iter().skip(i).any(|(k, _)| k == "ENDBLK").then(|| Before::EndBlk(up(name)))
    };
    let br = |name: &str| roles.get(&("BLOCK_RECORD".to_string(), up(name))).cloned();
    let first_at = |s: &cadaddy_doc::EntityStore| s.iter().find(|e| written(e)).map(|e| Before::Entity(e.handle.hex()));
    let layouts = d.paper_space_layouts();
    let ents_end = Some(Before::EntitiesEnd);
    let mut stores: Vec<(&cadaddy_doc::EntityStore, Option<String>, bool, Option<Before>, &str)> = Vec::new();
    let model_end = layouts.first().and_then(|l| first_at(&l.entities)).or(ents_end.clone());
    stores.push((&d.model, br("*Model_Space"), false, model_end, "ENTITIES"));
    for (i, l) in layouts.iter().enumerate() {
        if i == 0 {
            stores.push((&l.entities, br("*Paper_Space"), true, ents_end.clone(), "ENTITIES"));
        } else {
            let name = format!("*Paper_Space{}", i - 1);
            stores.push((&l.entities, br(&name), true, endblk(&name), "BLOCKS"));
        }
    }
    for b in d.blocks.values() {
        stores.push((&b.entities, br(&b.name), false, endblk(&b.name), "BLOCKS"));
    }
    for (store, owner, paper, end, sec) in stores {
        let (Some(owner), Some(end)) = (owner, end) else { continue };
        let mut pending: Vec<(String, Vec<Tag>)> = Vec::new();
        for e in store.iter() {
            match &e.kind {
                EntityKind::Unknown(u) => {
                    if modern && cadaddy_doc::foreign::sat_text_acis(u, e.handle, &stored) {
                        continue;
                    }
                    let t = unknown_entity(u, e, &owner, paper);
                    if !t.is_empty() {
                        pending.push((sec.to_string(), t));
                    }
                }
                _ => {
                    if written(e) && !pending.is_empty() {
                        out.entry(Before::Entity(e.handle.hex())).or_default().append(&mut pending);
                    }
                }
            }
        }
        if !pending.is_empty() {
            out.entry(end).or_default().append(&mut pending);
        }
    }
    out
}

/// The reactor handles a written record gains: the kept foreign ones (remapped) unless the record
/// is an unknown entity that already carries its own, then `extra` (handles of the written file,
/// such as a DIMASSOC), without repeating any the record already names. They go into the one
/// `{ACAD_REACTORS` group the record has, or a new one, so no reactor is lost to a second group.
fn reactors_to_add(map: &Map, h: &Head, x: Option<&ForeignExtra>, unknown: Option<&[String]>) -> Vec<String> {
    let extra = unknown.unwrap_or_default();
    let mut have: HashSet<String> = h.reactors.iter().cloned().collect();
    let kept = x.filter(|_| !(unknown.is_some() && h.has_reactors)).into_iter().flat_map(|x| x.reactors.iter()).filter_map(|r| map.get(&r.hex()));
    kept.chain(extra.iter().cloned()).filter(|r| r != "0" && have.insert(norm(r))).collect()
}

/// A written record with its kept extras, remapped references (unknown entities) and kept
/// dictionary entries (the named object dictionary).
fn record(map: &Map, r: &Rec, b: &[Tag], x: Option<&ForeignExtra>, unknown: Option<&[String]>, root: bool, out: &mut Vec<Tag>) {
    let h = head(b);
    let adds = reactors_to_add(map, &h, x, unknown);
    let mut in_reactors = false;
    let entity = matches!(r.sec.as_str(), "ENTITIES" | "BLOCKS") && r.kind != "BLOCK";
    let xdata_at = b.iter().position(|t| t.code == 1001).unwrap_or(b.len());
    let own_apps: HashSet<String> = b.iter().filter(|t| t.code == 1001).map(|t| up(&t.str())).collect();
    // Where unmodelled properties go: the end of AcDbEntity, else before the xdata.
    let props_at = if entity {
        b.iter().position(|t| t.code == 100 && t.str().trim() == "AcDbEntity").and_then(|s| {
            let rest = b.get(s + 1..xdata_at)?;
            Some(s + 1 + rest.iter().position(|t| t.code == 100).unwrap_or(rest.len()))
        })
    } else {
        None
    }
    .unwrap_or(xdata_at);
    let rgb = b.iter().take(xdata_at).find(|t| t.code == 420).map(Tag::str);
    let has_book = b.iter().take(xdata_at).any(|t| t.code == 430);
    let handle_at = b.iter().position(|t| matches!(t.code, 5 | 105));
    // A kept extension dictionary follows the reactors group, as the DXF reference orders them.
    let reactors_end = b.iter().position(|t| t.code == 102 && t.str().trim() == "{ACAD_REACTORS").and_then(|s| {
        let rest = b.get(s + 1..)?;
        rest.iter().position(|t| t.code == 102 && t.str().trim() == "}").map(|e| s + 1 + e)
    });
    let xdict_at = if h.has_reactors { reactors_end.or(handle_at) } else { handle_at };
    let mut seen_owner = false;
    for (i, t) in b.iter().enumerate() {
        if i == props_at
            && let Some(x) = x
        {
            if let Some((want, book)) = &x.color_book
                && !has_book
                && rgb.as_deref() == Some(want.as_str())
            {
                out.push(Tag::s(430, book.clone()));
            }
            out.extend(x.props.iter().map(|p| map.group(p)));
        }
        if i == xdata_at
            && let Some(x) = x
        {
            xdata(map, x, &own_apps, out);
        }
        if t.code == 102 {
            match t.str().trim() {
                "{ACAD_REACTORS" => in_reactors = true,
                "}" if in_reactors => {
                    in_reactors = false;
                    out.extend(adds.iter().map(|a| Tag::s(330, a.clone())));
                }
                _ => {}
            }
        }
        let mut t = t.clone();
        if unknown.is_some() && i < xdata_at && is_pointer(t.code) && t.code != 102 {
            if t.code == 330 && !seen_owner {
                seen_owner = true;
            } else {
                t = Tag::s(t.code, map.get(&t.str()).unwrap_or_else(|| "0".into()));
            }
        }
        out.push(t);
        if Some(i) == handle_at && !h.has_reactors && !adds.is_empty() {
            out.push(Tag::s(102, "{ACAD_REACTORS"));
            out.extend(adds.iter().map(|a| Tag::s(330, a.clone())));
            out.push(Tag::s(102, "}"));
        }
        if Some(i) == xdict_at
            && let Some(x) = x
            && h.xdict.is_none()
            && let Some(xd) = x.xdict
        {
            map.handle_group(out, "ACAD_XDICTIONARY", 360, &[xd.hex()]);
        }
    }
    if let Some(x) = x {
        if props_at == b.len() {
            if let Some((want, book)) = &x.color_book
                && !has_book
                && rgb.as_deref() == Some(want.as_str())
            {
                out.push(Tag::s(430, book.clone()));
            }
            out.extend(x.props.iter().map(|p| map.group(p)));
        }
        if xdata_at == b.len() {
            xdata(map, x, &own_apps, out);
        }
    }
    if root {
        for (key, h) in &map.f.dictionary {
            if let Some(nh) = map.alive.get(&h.hex())
                && !b.iter().any(|t| t.code == 3 && up(&t.str()) == up(key))
            {
                out.push(Tag::s(3, key.clone()));
                out.push(Tag::s(350, nh.clone()));
            }
        }
    }
}

/// Kept xdata of applications the record doesn't already carry, handles remapped.
fn xdata(map: &Map, x: &ForeignExtra, own: &HashSet<String>, out: &mut Vec<Tag>) {
    let mut skip = false;
    for r in &x.xdata {
        if r.code == 1001 {
            skip = own.contains(&up(&r.value));
        }
        if !skip {
            out.push(map.group(r));
        }
    }
}
