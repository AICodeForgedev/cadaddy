//! Element properties (`bim.props.*`): the values every element of the model carries itself.
//!
//! Each element of the built model — wall, door, window, room, floor, ceiling, roof, device,
//! fixture, light fixture, equipment, duct, pipe, conduit — holds one property store
//! ([`cadaddy_doc::props`]): key → {value, unit?, source, confidence?, note?}. The readers fill it
//! (source `print`, `legend`, `schedule`, `spec`, `note`), assumptions are marked (`rule`,
//! `default`), a person's edits are `user`, and what the model gives is `computed`.
//!
//! - `bim.props.get {handle | handles}`: an element's properties with their sources, its kind,
//!   its kind's standard keys and which of them have no value yet.
//! - `bim.props.set {handle | handles | filter, key, value, unit?, note?}` (or `props: {key:
//!   value}`): set values as the user's. On a generated element the edit is kept as an override
//!   (`override.list` shows it as kind `props`), so a rebuild of the model from the prints puts
//!   it back.
//! - `bim.props.find {kind?, where: {key: value | "exists" | "missing"}}`: the elements whose
//!   properties match.
//! - `bim.props.summary {kind?}`: per kind, how many elements, how many have a value for each key,
//!   and where those values came from.
//!
//! What an element shows besides its store, marked `computed` and never stored: its level (from
//! the build's tag), a room's area, a room volume's name and number (read from its tag on the
//! plan, so `print`), and a floor's type, assembly and thickness (its build tags, sourced as the
//! build knew them: a note, a rule, the default). A stored value always wins.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::{self, Entry, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const GET: &str = "bim.props.get";
const SET: &str = "bim.props.set";
const FIND: &str = "bim.props.find";
const SUMMARY: &str = "bim.props.summary";
const BATCH: &str = "bim.props.batch";

/// Most model objects one call looks at.
const MAX_SCAN: usize = 2_000_000;
/// Most elements `bim.props.get` returns, and `bim.props.find` lists by default and at all.
const GET_MAX: usize = 500;
const LIST_DEFAULT: usize = 200;
const LIST_MAX: usize = 100_000;
/// Most elements one `bim.props.set` changes.
const SET_MAX: usize = 100_000;
/// Most steps one `bim.props.batch` takes.
const BATCH_MAX: usize = 20_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(GET, "Element Properties (BIM)", run_get)
            .params("{handle? | handles?: [string] (default: the selection)} → {elements: [{handle, kind, props: {key: {value, unit?, source: {kind: print|legend|schedule|spec|submittal|note|rule|default|user|computed, doc?, at?}, confidence?, note?, stored}}, order: [key], standard: [key], missing: [key]}]}")
            .noundo(),
        CommandSpec::new(SET, "Set Element Property (BIM)", run_set)
            .params("{handle? | handles? | filter?: {kind?, where?: {key: value | \"exists\" | \"missing\"}} (default: the selection), key?: string, value?: string | number | bool | null (null clears), unit?, note?, props?: {key: value} (several at once; with or instead of key)} → {set, elements: [{handle, kind, override?}], skipped}: source user; kept through rebuilds"),
        CommandSpec::new(FIND, "Find Elements by Property", run_find)
            .params("{kind?: string (wall, door, window, room, floor, ceiling, roof, device, light_fixture, plumbing_fixture, fixture, equipment, duct, pipe, conduit, or a build part such as column), where?: {key: value | \"exists\" | \"missing\"}, select?: bool, limit? (default 200)} → {count, handles, elements: [{handle, kind, level?}], more}")
            .noundo(),
        CommandSpec::new(BATCH, "Correct Elements by Property (BIM)", run_batch)
            .params("{steps: [{kind?, where?: {key: value | \"exists\" | \"missing\"}, delta?: [dx, dy, dz?] (move the elements found), props?: {key: value} (set them as the user's), unit?, note?}], select?: bool (select every element changed)} → {steps: [{found, moved, set}], found, moved, set}: the steps in turn, one undo step; every step's params are checked before the first runs"),
        CommandSpec::new(SUMMARY, "Element Property Summary", run_summary)
            .params("{kind?} → {kinds: {kind: {count, filled: {key: n_with_value}, sources: {key: {source: n}}, standard: [key]}}, elements}")
            .noundo(),
    ]
}

// ─── What an element is ──────────────────────────────────────────────────────

/// The kind of model element `e` is (`wall`, `door`, `device`, `duct`…; the build's part name for
/// one without a standard set, such as `column` or `stair`); `None` for anything that isn't an
/// element of the model (the sheets' linework).
pub fn element_kind(e: &Entity) -> Option<String> {
    if let Some(part) = super::wallgen::made_as(e) {
        return Some(kind_of_part(e, &part.to_ascii_lowercase()).to_string());
    }
    if super::rooms3d::room_of(&e.common).is_some() {
        return Some("room".into());
    }
    if let Some(t) = super::ifcref::tag(e) {
        return Some(props::kind_of_ifc_class(&t.class).to_string());
    }
    None
}

/// What an element is, said as a reader would: a device's own kind as the build wrote it
/// (`receptacle`, `smoke_detector`, `wall_luminaire`, from its legend row or its sheet), else
/// [`element_kind`]'s.
pub(crate) fn reported_kind(e: &Entity, kind: String) -> String {
    if !is_device_kind(&kind) || kind == "fixture" {
        return kind;
    }
    cadaddy_doc::props::read(&e.common.xdata).get("kind").map(|p| p.text()).filter(|k| !k.trim().is_empty() && k != "device").unwrap_or(kind)
}

/// The build tag that says what element kind a built device is (`element:light_fixture`), set
/// when it is made.
pub(crate) const ELEMENT_TAG: &str = "element";

/// Whether an element kind is one of a built device's: a device, a light fixture, a plumbing
/// fixture (or a fixture of a trade's layer).
pub(crate) fn is_device_kind(kind: &str) -> bool {
    matches!(kind, "device" | "light_fixture" | "plumbing_fixture" | "fixture")
}

/// The element kind of a built device (Q6315): a luminaire a `light_fixture`, a plumbing fixture
/// a `plumbing_fixture`, as the build set it ([`ELEMENT_TAG`]), else (a model built before it
/// did) as its kind as read says (`fixturebody.json`); else its layer's (a lighting layer's, a
/// plumbing layer's), else a `device`.
fn device_kind(e: &Entity, layer: &str) -> &'static str {
    let tagged = super::bimlook::made_tag(e, ELEMENT_TAG);
    let class = match tagged {
        Some("light_fixture") => Some(super::fixturebody::Class::Light),
        Some("plumbing_fixture") => Some(super::fixturebody::Class::Plumbing),
        Some(_) => None,
        None => props::read(&e.common.xdata).get("kind").and_then(|p| super::fixturebody::class_of_kind(&p.text())),
    };
    if let Some(c) = class {
        return c.element_kind();
    }
    if layer.contains("LIGHT") || layer.contains("LTG") {
        "light_fixture"
    } else if layer.contains("PLUMB") || layer.contains("FIXT") || layer.contains("SPRINK") {
        "fixture"
    } else {
        "device"
    }
}

fn kind_of_part<'a>(e: &Entity, part: &'a str) -> &'a str {
    let layer = e.common.layer.to_ascii_uppercase();
    match part {
        "glass" | "window" | "curtainwall" => "window",
        "soffit" | "grid" | "ceiling" => "ceiling",
        "roof" | "cricket" => "roof",
        "light" => "light_fixture",
        "diffuser" => "fixture",
        super::grids::GRID_PART => "structural_grid",
        "device" => device_kind(e, &layer),
        "mep" => {
            let kind = super::bimlook::made_tag(e, "kind").unwrap_or("").to_ascii_lowercase();
            let mpart = super::bimlook::made_tag(e, "part").unwrap_or("").to_ascii_lowercase();
            if mpart.starts_with("equipment") || kind == "equipment" {
                "equipment"
            } else if mpart.starts_with("terminal") {
                if matches!(kind.as_str(), "duct" | "pipe") { "fixture" } else { "device" }
            } else {
                match kind.as_str() {
                    "duct" => "duct",
                    "pipe" => "pipe",
                    "conduit" | "cable" | "tray" | "grounding" => "conduit",
                    // A breaker in its panel (Q6166): a piece, not a piece of equipment.
                    "breaker" => "breaker",
                    _ => "equipment",
                }
            }
        }
        other => other,
    }
}

/// The level an element stands on, as the model knows it.
pub(crate) fn level_of(e: &Entity) -> Option<String> {
    super::bspec::made_level(e).or_else(|| super::ifcref::tag(e).and_then(|t| t.level)).filter(|l| !l.trim().is_empty())
}

/// The plan area of a mesh: its faces looking up (a room volume's ceiling, a slab's top).
pub(crate) fn up_area(e: &Entity) -> Option<f64> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let a: f64 = m
        .triangles()
        .into_iter()
        .take(400_000)
        .map(|t| (t[1] - t[0]).cross(t[2] - t[0]))
        .filter(|n| n.is_finite() && n.len() > 0.0 && (n.z / n.len()) > 0.7)
        .map(|n| n.len() * 0.5)
        .sum();
    (a.is_finite() && a > 0.0).then_some(a)
}

/// Drawing units per foot (the building spec's, else inches).
pub(crate) fn units_per_foot(d: &Drawing) -> f64 {
    // Read once per spec text: the Properties palette asks for it every frame.
    crate::specread::units_per_foot(d)
}

/// An element's properties: its store, and what the model gives (marked `computed`, or `print`
/// for a room volume's tag) for the keys the store has no value for. The flag is whether the
/// property is stored.
pub fn effective(e: &Entity, kind: &str, upf: f64) -> Vec<(String, Prop, bool)> {
    let store = props::read(&e.common.xdata);
    let mut out: Vec<(String, Prop, bool)> = store.iter().map(|(k, p)| (k.to_string(), p.clone(), true)).collect();
    let mut derive = |key: &str, p: Prop| {
        if !out.iter().any(|(k, q, _)| k.eq_ignore_ascii_case(key) && q.has_value()) && p.has_value() {
            out.retain(|(k, _, _)| !k.eq_ignore_ascii_case(key));
            out.push((key.to_string(), p, false));
        }
    };
    if let Some(l) = level_of(e) {
        derive("level", Prop::new(l, Source::computed()));
    }
    if kind == "floor" {
        // A floor element of the model: its build-up as the build read it (Q6408).
        for (key, p) in super::floorasm::props_of(e) {
            derive(key, p);
        }
    }
    if kind == "room" && super::wallgen::made_as(e).as_deref() == Some("room") {
        // A room element of the model (model.build, Q5291): its tag's values.
        let tag = |k: &str| super::bspec::room_tag(e, k).filter(|v| !v.is_empty());
        if let Some(n) = tag("number:") {
            derive("number", Prop::new(n, Source::of(SourceKind::Print)));
        }
        if let Some(n) = tag("name:") {
            derive("name", Prop::new(n, Source::of(SourceKind::Print)));
        }
        if let Some(a) = tag("area_sf:").and_then(num).filter(|a| *a > 0.0) {
            derive("area", Prop::new((a * 100.0).round() / 100.0, Source::computed()).with_unit("sf"));
        }
        if let Some(c) = tag("ceiling_ft:").and_then(num).filter(|c| *c > 0.0) {
            derive("ceiling_height", Prop::new(c, Source::of(SourceKind::Spec)).with_unit("ft"));
        }
        if let Some(f) = tag("finish:") {
            derive("floor_finish", Prop::new(f, Source::of(SourceKind::Spec)));
        }
    } else if kind == "room" {
        if let Some((name, number)) = room_tag(e) {
            derive("number", Prop::new(number, Source::of(SourceKind::Print)));
            derive("name", Prop::new(name, Source::of(SourceKind::Print)));
        }
        if let Some(a) = up_area(e) {
            let sf = (a / (upf * upf) * 100.0).round() / 100.0;
            derive("area", Prop::new(sf, Source::computed()).with_unit("sf"));
        }
    }
    out
}

/// A room volume's name and number, from its `rooms.build` tag.
fn room_tag(e: &Entity) -> Option<(String, String)> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value == super::rooms3d::ROOM_APP)?;
    let v = |k: usize| x.get(at + k).filter(|t| t.code == 1000).map(|t| t.value.clone()).unwrap_or_default();
    Some((v(1), v(2)))
}

fn prop_json(p: &Prop, stored: bool) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
    v["stored"] = json!(stored);
    v
}

fn element_json(e: &Entity, kind: &str, upf: f64) -> Value {
    let eff = effective(e, kind, upf);
    let standard = props::standard_keys(kind);
    let missing: Vec<&str> = standard.iter().copied().filter(|k| !eff.iter().any(|(q, p, _)| q.eq_ignore_ascii_case(k) && p.has_value())).collect();
    json!({
        "handle": e.handle.hex(),
        "kind": kind,
        "props": eff.iter().map(|(k, p, s)| (k.clone(), prop_json(p, *s))).collect::<serde_json::Map<_, _>>(),
        "order": eff.iter().map(|(k, _, _)| k.as_str()).collect::<Vec<_>>(),
        "standard": standard,
        "missing": missing,
    })
}

// ─── Matching ────────────────────────────────────────────────────────────────

/// What `where` asks of one key.
#[derive(Clone, Debug, PartialEq)]
enum Want {
    Exists,
    Missing,
    Is(String),
}

/// A `{kind?, where?}` filter.
#[derive(Clone, Debug, Default)]
struct Filter {
    kind: Option<String>,
    wants: Vec<(String, Want)>,
}

impl Filter {
    fn parse(cmd: &str, p: &Value) -> Result<Filter> {
        let kind = p.get("kind").and_then(Value::as_str).map(|k| k.trim().to_ascii_lowercase()).filter(|k| !k.is_empty());
        let mut wants = Vec::new();
        match p.get("where") {
            None | Some(Value::Null) => {}
            Some(Value::Object(m)) => {
                for (k, v) in m.iter().take(64) {
                    let want = match v {
                        Value::String(t) if t.eq_ignore_ascii_case("exists") => Want::Exists,
                        Value::String(t) if t.eq_ignore_ascii_case("missing") => Want::Missing,
                        Value::Object(o) if o.get("exists").and_then(Value::as_bool) == Some(true) => Want::Exists,
                        Value::Object(o) if o.get("missing").and_then(Value::as_bool) == Some(true) => Want::Missing,
                        Value::String(t) => Want::Is(t.trim().to_string()),
                        Value::Number(_) | Value::Bool(_) => Want::Is(v.to_string()),
                        _ => return Err(bad(cmd, format!("`where.{k}`: a value, \"exists\" or \"missing\""))),
                    };
                    wants.push((k.trim().to_string(), want));
                }
            }
            Some(_) => return Err(bad(cmd, "`where` must be an object: {key: value | \"exists\" | \"missing\"}")),
        }
        Ok(Filter { kind, wants })
    }

    fn kind_ok(&self, kind: &str) -> bool {
        self.kind.as_deref().is_none_or(|k| k == kind)
    }

    /// Whether an element's properties meet the filter's `where` (what the index answers; the
    /// tests hold it to this).
    #[cfg(test)]
    fn matches(&self, eff: &[(String, Prop, bool)]) -> bool {
        self.wants.iter().all(|(key, want)| {
            let p = eff.iter().find(|(k, p, _)| k.eq_ignore_ascii_case(key) && p.has_value()).map(|(_, p, _)| p);
            match want {
                Want::Exists => p.is_some(),
                Want::Missing => p.is_none(),
                Want::Is(v) => p.is_some_and(|p| p.text().trim().eq_ignore_ascii_case(v) || (num(&p.text()).is_some() && num(&p.text()) == num(v))),
            }
        })
    }
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

// ─── The element index ───────────────────────────────────────────────────────
//
// What an element is, its level and its properties' values are worked out once per chunk of
// the model and kept while the chunk is unchanged ([`crate::chunkpool`]): a find, a filtered set
// or a summary after an edit only looks again at the chunks the edit touched (Q6438: a
// type-wide scripted correction ran one full scan of a 1.4 M-object model per find).

/// One element of a chunk.
#[derive(Clone)]
struct Row {
    /// Where it is in its chunk.
    at: u32,
    handle: Handle,
    kind: String,
    level: Option<String>,
    /// A `rooms.build` volume's room (number, or unnumbered its name; upper case): it is that
    /// room's 3D view when the model holds the room as an element.
    volume: Option<String>,
    /// A room element's room (number, or unnumbered its name; upper case).
    room: Option<String>,
}

/// One property key's values among a chunk's elements: rows (ascending) with a value, by the
/// value's text (trimmed, lower case) and by its number.
#[derive(Default)]
struct KeyValues {
    with: Vec<u32>,
    text: HashMap<String, Vec<u32>>,
    num: HashMap<u64, Vec<u32>>,
}

type Values = HashMap<String, KeyValues>;

/// A chunk's elements, and (worked out when first asked) their property values by key.
struct ChunkElements {
    /// Per object of the chunk: its row, or [`NO_ROW`] (not an element).
    slot: Vec<u32>,
    rows: Vec<Row>,
    /// The rooms held as room elements (number or name, upper case).
    rooms: Vec<String>,
    values: std::sync::OnceLock<Values>,
}

const NO_ROW: u32 = u32::MAX;

/// A number's key in [`KeyValues::num`] (−0 is 0).
fn num_key(v: f64) -> u64 {
    if v == 0.0 { 0 } else { v.to_bits() }
}

fn idx(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(NO_ROW)
}

/// One element's row (`None`: not an element).
fn row_of(at: usize, e: &Entity) -> Option<Row> {
    let made = super::wallgen::made_as(e);
    let room = (made.as_deref() == Some("room") && matches!(e.kind, EntityKind::Mesh(_))).then(|| {
        let tag = |k: &str| super::bspec::room_tag(e, k).filter(|v| !v.is_empty());
        tag("number:").filter(|n| !n.trim().is_empty()).or_else(|| tag("name:")).unwrap_or("Room").trim().to_ascii_uppercase()
    });
    let kind = element_kind(e)?;
    let volume = made
        .is_none()
        .then(|| room_tag(e))
        .flatten()
        .map(|(name, number)| if number.trim().is_empty() { name } else { number }.trim().to_ascii_uppercase());
    Some(Row { at: idx(at), handle: e.handle, kind, level: level_of(e), volume, room })
}

/// Put row `ri`'s property values in `out`: of each key, the first property that has a value, as
/// [`Filter::matches`] reads them.
fn add_values(out: &mut Values, ri: u32, e: &Entity, kind: &str, upf: f64) {
    for (k, p, _) in effective(e, kind, upf) {
        if !p.has_value() {
            continue;
        }
        let kv = out.entry(k.to_ascii_lowercase()).or_default();
        if kv.with.last() == Some(&ri) {
            continue;
        }
        kv.with.push(ri);
        let text = p.text();
        kv.text.entry(text.trim().to_ascii_lowercase()).or_default().push(ri);
        if let Some(n) = num(&text) {
            kv.num.entry(num_key(n)).or_default().push(ri);
        }
    }
}

impl ChunkElements {
    /// The chunk's elements. `before`: the chunk as it was before an edit and its elements; the
    /// objects the edit did not touch (the same objects) keep their rows, and their values when
    /// those were worked out.
    fn build(items: &[std::sync::Arc<Entity>], before: Option<(&[std::sync::Arc<Entity>], &ChunkElements)>, upf: f64) -> ChunkElements {
        let old: HashMap<usize, usize> =
            before.map(|(items, _)| items.iter().enumerate().map(|(i, e)| (std::sync::Arc::as_ptr(e) as usize, i)).collect()).unwrap_or_default();
        let mut slot = Vec::with_capacity(items.len());
        let mut rows = Vec::new();
        // Per row: the row it was before (or NO_ROW: new or changed).
        let mut was: Vec<u32> = Vec::new();
        for (at, e) in items.iter().enumerate() {
            let kept = before.zip(old.get(&(std::sync::Arc::as_ptr(e) as usize))).and_then(|((_, b), &i)| b.slot.get(i).map(|&r| (b, r)));
            let (row, from) = match kept {
                Some((_, NO_ROW)) => (None, NO_ROW),
                Some((b, r)) => (b.rows.get(r as usize).cloned().map(|row| Row { at: idx(at), ..row }), r),
                None => (row_of(at, e), NO_ROW),
            };
            match row {
                Some(row) => {
                    slot.push(idx(rows.len()));
                    rows.push(row);
                    was.push(from);
                }
                None => slot.push(NO_ROW),
            }
        }
        let rooms = rows.iter().filter_map(|r| r.room.clone()).collect();
        let out = ChunkElements { slot, rows, rooms, values: std::sync::OnceLock::new() };
        // The values the chunk had, carried over to the rows kept; the others read again.
        if let Some(prev) = before.and_then(|(_, b)| b.values.get()) {
            let mut to: Vec<u32> = vec![NO_ROW; before.map_or(0, |(_, b)| b.rows.len())];
            for (ri, &w) in was.iter().enumerate() {
                if let Some(t) = to.get_mut(w as usize) {
                    *t = idx(ri);
                }
            }
            let remap = |v: &[u32]| -> Vec<u32> { v.iter().filter_map(|r| to.get(*r as usize).copied().filter(|t| *t != NO_ROW)).collect() };
            let mut values: Values = HashMap::with_capacity(prev.len());
            for (k, kv) in prev {
                values.insert(
                    k.clone(),
                    KeyValues {
                        with: remap(&kv.with),
                        text: kv.text.iter().map(|(t, v)| (t.clone(), remap(v))).filter(|(_, v)| !v.is_empty()).collect(),
                        num: kv.num.iter().map(|(n, v)| (*n, remap(v))).filter(|(_, v)| !v.is_empty()).collect(),
                    },
                );
            }
            for (ri, row) in out.rows.iter().enumerate() {
                if was.get(ri) == Some(&NO_ROW)
                    && let Some(e) = items.get(row.at as usize)
                {
                    add_values(&mut values, idx(ri), e, &row.kind, upf);
                }
            }
            for kv in values.values_mut() {
                kv.with.sort_unstable();
                kv.with.dedup();
                for v in kv.text.values_mut().chain(kv.num.values_mut()) {
                    v.sort_unstable();
                    v.dedup();
                }
            }
            values.retain(|_, kv| !kv.with.is_empty());
            let _ = out.values.set(values);
        }
        out
    }

    /// Its elements' property values by key (lower case).
    fn values(&self, items: &[std::sync::Arc<Entity>], upf: f64) -> &Values {
        self.values.get_or_init(|| {
            let mut out = Values::new();
            for (ri, row) in self.rows.iter().enumerate() {
                if let Some(e) = items.get(row.at as usize) {
                    add_values(&mut out, idx(ri), e, &row.kind, upf);
                }
            }
            out
        })
    }

    /// Whether row `ri` meets every `want`.
    fn meets(values: &Values, wants: &[(String, Want)], ri: u32) -> bool {
        wants.iter().all(|(key, want)| {
            let kv = values.get(&key.to_ascii_lowercase());
            let has = |rows: Option<&Vec<u32>>| rows.is_some_and(|r| r.binary_search(&ri).is_ok());
            match want {
                Want::Exists => has(kv.map(|kv| &kv.with)),
                Want::Missing => !has(kv.map(|kv| &kv.with)),
                Want::Is(v) => kv.is_some_and(|kv| has(kv.text.get(&v.to_ascii_lowercase())) || num(v).is_some_and(|n| has(kv.num.get(&num_key(n))))),
            }
        })
    }
}

static INDEX: crate::chunkpool::ChunkPool<ChunkElements, u64> = crate::chunkpool::ChunkPool::new();

/// Let go of the kept element index (a drawing closed).
pub(crate) fn clear_index() {
    INDEX.clear();
}

/// The model's chunks with their elements (draw order), and how many chunks were worked out.
fn indexed(d: &Drawing) -> (Vec<crate::chunkpool::Part<ChunkElements>>, crate::chunkpool::Refresh) {
    let upf = units_per_foot(d);
    INDEX.parts(&d.model, upf.to_bits(), |c, before| ChunkElements::build(c.items(), before.map(|b| (b.chunk.items(), &*b.data)), upf))
}

/// The model's elements in draw order, among its first [`MAX_SCAN`] objects: (chunk's objects,
/// chunk's elements, row), without a room volume of a room the model holds as an element.
fn each_element<'a>(
    parts: &'a [crate::chunkpool::Part<ChunkElements>],
) -> impl Iterator<Item = (&'a [std::sync::Arc<Entity>], &'a ChunkElements, usize, &'a Row)> + 'a {
    let modelled: std::sync::Arc<BTreeSet<&str>> = std::sync::Arc::new(parts.iter().flat_map(|p| p.data.rooms.iter().map(String::as_str)).collect());
    let mut seen = 0usize;
    parts.iter().flat_map(move |p| {
        let left = MAX_SCAN.saturating_sub(seen);
        seen = seen.saturating_add(p.data.slot.len());
        let items = p.chunk.items();
        let c: &ChunkElements = &p.data;
        let modelled = std::sync::Arc::clone(&modelled);
        c.rows
            .iter()
            .enumerate()
            .filter(move |(_, r)| (r.at as usize) < left && !(r.volume.as_deref().is_some_and(|v| modelled.contains(v))))
            .map(move |(i, r)| (items, c, i, r))
    })
}

/// The model's elements: (entity, kind). A room counts once: a `rooms.build` volume of a room
/// the model holds as a room element (same number, or unnumbered the same name) is that room's
/// 3D view, not a second room.
pub(crate) fn elements(d: &Drawing) -> impl Iterator<Item = (&std::sync::Arc<Entity>, String)> {
    let (parts, _) = indexed(d);
    // Draw order: the drawing's own chunks, the index's rows of each.
    let found: Vec<(usize, u32, String)> = each_element(&parts).map(|(items, _, _, r)| (items.as_ptr() as usize, r.at, r.kind.clone())).collect();
    let slices: HashMap<usize, &[std::sync::Arc<Entity>]> = d.model.chunk_slices().map(|(_, items)| (items.as_ptr() as usize, items)).collect();
    found.into_iter().filter_map(move |(chunk, at, kind)| slices.get(&chunk).and_then(|items| items.get(at as usize)).map(|e| (e, kind)))
}

fn found(d: &Drawing, f: &Filter) -> Vec<(Handle, String, Option<String>)> {
    let upf = units_per_foot(d);
    let (parts, _) = indexed(d);
    each_element(&parts)
        .filter(|(_, _, _, r)| f.kind_ok(&r.kind))
        .filter(|(items, c, i, _)| f.wants.is_empty() || ChunkElements::meets(c.values(items, upf), &f.wants, u32::try_from(*i).unwrap_or(u32::MAX)))
        .map(|(_, _, _, r)| (r.handle, r.kind.clone(), r.level.clone()))
        .collect()
}

// ─── Commands ────────────────────────────────────────────────────────────────

fn run_get(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(GET, "give `handle` or `handles`, or select elements of the model"));
    }
    let d = s.doc()?;
    let upf = units_per_foot(d);
    let mut out = Vec::new();
    for h in hs.iter().take(GET_MAX) {
        let e = d.model.get(*h).ok_or_else(|| bad(GET, format!("there is no object {} in the model", h.hex())))?;
        let kind =
            element_kind(e).ok_or_else(|| bad(GET, format!("{} is not an element of the model (build it with model.build / bim.build)", h.hex())))?;
        out.push(element_json(e, &kind, upf));
    }
    let mut r = json!({ "elements": out });
    if hs.len() > GET_MAX {
        r["more"] = json!(hs.len() - GET_MAX);
    }
    Ok(r)
}

/// The value a call gives: a scalar (null clears: the user says it has none).
fn value_param(cmd: &str, key: &str, v: &Value) -> Result<Value> {
    match v {
        Value::String(_) | Value::Bool(_) | Value::Null => Ok(v.clone()),
        Value::Number(n) if n.as_f64().is_some_and(f64::is_finite) => Ok(v.clone()),
        _ => Err(bad(cmd, format!("`{key}`: the value must be a string, number, boolean or null"))),
    }
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let unit = p.get("unit").and_then(Value::as_str);
    let note = p.get("note").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty());
    let mut edits: Vec<(String, Value)> = Vec::new();
    if let Some(k) = p.get("key") {
        let k = k.as_str().map(str::trim).filter(|k| !k.is_empty()).ok_or_else(|| bad(SET, "`key` must be a non-empty string"))?;
        let v = p.get("value").ok_or_else(|| bad(SET, "`value` is required with `key` (null clears the value)"))?;
        edits.push((k.to_string(), value_param(SET, k, v)?));
    }
    match p.get("props") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (k, v) in m.iter().take(props::MAX_PROPS) {
                if k.trim().is_empty() {
                    return Err(bad(SET, "`props`: keys must not be empty"));
                }
                edits.push((k.trim().to_string(), value_param(SET, k, v)?));
            }
        }
        Some(_) => return Err(bad(SET, "`props` must be an object {key: value}")),
    }
    if edits.is_empty() {
        return Err(bad(SET, "give `key` and `value`, or `props: {key: value}`"));
    }
    if let Some((k, _)) = edits.iter().find(|(k, _)| k.chars().count() > props::MAX_KEY) {
        return Err(bad(SET, format!("`{}…`: keys are at most {} characters", k.chars().take(20).collect::<String>(), props::MAX_KEY)));
    }
    let entries: Vec<Entry> = edits
        .into_iter()
        .map(|(key, value)| {
            let mut prop = Prop::new(value, Source::user());
            if let Some(u) = unit {
                prop = prop.with_unit(u);
            }
            prop.note = note.map(str::to_string);
            Entry { key, prop }
        })
        .collect();
    // Which elements.
    let d = s.doc()?;
    let hs: Vec<(Handle, String)> = match p.get("filter") {
        Some(f @ Value::Object(_)) => found(d, &Filter::parse(SET, f)?).into_iter().map(|(h, k, _)| (h, k)).collect(),
        Some(Value::Null) | None => {
            let hs = targets(s, p)?;
            let mut out = Vec::new();
            for h in hs {
                let e = d.model.get(h).ok_or_else(|| bad(SET, format!("there is no object {} in the model", h.hex())))?;
                let kind = element_kind(e).ok_or_else(|| bad(SET, format!("{} is not an element of the model", h.hex())))?;
                out.push((h, kind));
            }
            out
        }
        Some(_) => return Err(bad(SET, "`filter` must be an object {kind?, where?}")),
    };
    if hs.is_empty() {
        return Err(bad(SET, "no elements to set: give `handle`, `handles` or a `filter` that matches, or select elements of the model"));
    }
    if hs.len() > SET_MAX {
        return Err(bad(SET, format!("{} elements match: at most {SET_MAX} at a time", hs.len())));
    }
    // Generated elements: as overrides, so rebuilds keep them. The others: straight into their store.
    let d = s.doc()?;
    let (generated, own): (Vec<&(Handle, String)>, Vec<&(Handle, String)>) =
        hs.iter().partition(|(h, _)| d.model.get(*h).is_some_and(|e| super::revise3d::tag_key(e).is_some()));
    let batch: Vec<(Handle, Vec<Entry>)> = generated.iter().map(|(h, _)| (*h, entries.clone())).collect();
    let labels = super::overrides::set_props(s, SET, &batch, cadaddy_doc::overrides::Author::User)?;
    let d = s.doc_mut()?;
    let mut touched = Vec::new();
    for (h, _) in &own {
        let ok = d
            .modify_entity(*h, |e| {
                let mut store: Props = props::read(&e.common.xdata);
                for en in &entries {
                    store.set(&en.key, en.prop.clone());
                }
                e.common.xdata = props::write(&e.common.xdata, &store).into();
            })
            .is_ok();
        if ok {
            touched.push(*h);
        }
    }
    // A circuit or panel set: the links derived from them follow (Q6501; a generated element's
    // when its override is applied).
    if entries.iter().any(|en| super::circuitlinks::is_input(&en.key)) {
        super::circuitlinks::relink(d, &touched);
    }
    let label = |h: Handle| labels.iter().find(|(x, _)| *x == h).map(|(_, l)| l.clone());
    let done: Vec<Value> = hs
        .iter()
        .filter(|(h, _)| label(*h).is_some() || touched.contains(h))
        .take(LIST_DEFAULT)
        .map(|(h, k)| {
            let mut v = json!({ "handle": h.hex(), "kind": k });
            if let Some(l) = label(*h) {
                v["override"] = json!(l);
            }
            v
        })
        .collect();
    let set = labels.len() + touched.len();
    s.touch();
    s.echo(format!("Set {} on {set} element(s).", entries.iter().map(|e| e.key.as_str()).collect::<Vec<_>>().join(", ")));
    Ok(json!({ "set": set, "elements": done, "skipped": hs.len().saturating_sub(set) }))
}

fn run_find(s: &mut Session, p: &Value) -> Result<Value> {
    let f = Filter::parse(FIND, p)?;
    let limit = p.get("limit").and_then(Value::as_u64).map_or(LIST_DEFAULT, |n| usize::try_from(n).unwrap_or(LIST_MAX).min(LIST_MAX));
    let hits = found(s.doc()?, &f);
    if bool_or(p, "select", false) {
        s.set_selection(hits.iter().map(|(h, _, _)| *h).collect());
    }
    let shown: Vec<Value> = hits
        .iter()
        .take(limit)
        .map(|(h, k, l)| {
            let mut v = json!({ "handle": h.hex(), "kind": k });
            if let Some(l) = l {
                v["level"] = json!(l);
            }
            v
        })
        .collect();
    Ok(json!({
        "count": hits.len(),
        "handles": hits.iter().take(limit).map(|(h, _, _)| h.hex()).collect::<Vec<_>>(),
        "elements": shown,
        "more": hits.len().saturating_sub(limit),
    }))
}

/// One step of a `bim.props.batch`, checked.
struct BatchStep {
    filter: Filter,
    delta: Option<[f64; 3]>,
    props: Option<Value>,
    unit: Option<String>,
    note: Option<String>,
}

fn batch_step(i: usize, p: &Value) -> Result<BatchStep> {
    let at = |m: String| bad(BATCH, format!("step {}: {m}", i + 1));
    if !p.is_object() {
        return Err(at("each step is an object {kind?, where?, delta?, props?}".into()));
    }
    let filter = Filter::parse(BATCH, p).map_err(|e| at(e.to_string()))?;
    let delta = match p.get("delta") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) if (2..=3).contains(&a.len()) => {
            let n: Vec<f64> = a.iter().filter_map(Value::as_f64).filter(|v| v.is_finite()).collect();
            if n.len() != a.len() {
                return Err(at("`delta`: [dx, dy, dz?] as finite numbers".into()));
            }
            Some([n.first().copied().unwrap_or(0.0), n.get(1).copied().unwrap_or(0.0), n.get(2).copied().unwrap_or(0.0)])
        }
        Some(_) => return Err(at("`delta`: [dx, dy, dz?] as finite numbers".into())),
    };
    let props = match p.get("props") {
        None | Some(Value::Null) => None,
        Some(Value::Object(m)) if !m.is_empty() => {
            for (k, v) in m {
                if k.trim().is_empty() || k.chars().count() > props::MAX_KEY {
                    return Err(at(format!("`props`: keys are 1 to {} characters", props::MAX_KEY)));
                }
                value_param(BATCH, k, v).map_err(|e| at(e.to_string()))?;
            }
            Some(Value::Object(m.clone()))
        }
        Some(_) => return Err(at("`props` must be a non-empty object {key: value}".into())),
    };
    if delta.is_none() && props.is_none() {
        return Err(at("give `delta` (a move), `props` (values to set) or both".into()));
    }
    let text = |k: &str| p.get(k).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    Ok(BatchStep { filter, delta, props, unit: text("unit"), note: text("note") })
}

/// Many corrections in one call (Q6438): each step finds elements by kind and property, then
/// moves them and/or sets values on them as `3dmove` and `bim.props.set` would. A scripted
/// correction of a typical room's devices is one call instead of a find, a move and a set per
/// device.
fn run_batch(s: &mut Session, p: &Value) -> Result<Value> {
    let steps = match p.get("steps") {
        Some(Value::Array(a)) if !a.is_empty() => a,
        _ => return Err(bad(BATCH, "`steps` must be a non-empty array of {kind?, where?, delta?, props?}")),
    };
    if steps.len() > BATCH_MAX {
        return Err(bad(BATCH, format!("{} steps: at most {BATCH_MAX} in one call", steps.len())));
    }
    let steps: Vec<BatchStep> = steps.iter().enumerate().map(|(i, st)| batch_step(i, st)).collect::<Result<_>>()?;
    // In turn: a step finds what the steps before it left (a value one sets, a later one finds).
    let mut out = Vec::new();
    let (mut found_n, mut moved_n, mut set_n) = (0usize, 0usize, 0usize);
    let mut changed: Vec<Handle> = Vec::new();
    for (i, st) in steps.iter().enumerate() {
        let hits: Vec<Handle> = found(s.doc()?, &st.filter).into_iter().map(|(h, _, _)| h).collect();
        if hits.len() > SET_MAX {
            return Err(bad(BATCH, format!("step {}: {} elements match: at most {SET_MAX} at a time", i + 1, hits.len())));
        }
        let handles: Vec<String> = hits.iter().map(|h| h.hex()).collect();
        let (mut moved, mut set) = (0usize, 0usize);
        if !hits.is_empty() {
            if let Some(d) = st.delta {
                s.execute("3dmove", &json!({"handles": handles, "delta": d})).map_err(|e| bad(BATCH, format!("step {}: {e}", i + 1)))?;
                moved = hits.len();
            }
            if let Some(props) = &st.props {
                let mut q = json!({"handles": handles, "props": props});
                if let Some(u) = &st.unit {
                    q["unit"] = json!(u);
                }
                if let Some(n) = &st.note {
                    q["note"] = json!(n);
                }
                let r = s.execute(SET, &q).map_err(|e| bad(BATCH, format!("step {}: {e}", i + 1)))?;
                set = r["set"].as_u64().map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
            }
            changed.extend(hits.iter().copied());
        }
        found_n += hits.len();
        moved_n += moved;
        set_n += set;
        if out.len() < LIST_MAX {
            out.push(json!({"found": hits.len(), "moved": moved, "set": set}));
        }
    }
    if bool_or(p, "select", false) {
        changed.sort_unstable();
        changed.dedup();
        s.set_selection(changed);
    }
    s.touch();
    s.echo(format!("{} step(s): {found_n} element(s) found, {moved_n} moved, {set_n} set.", steps.len()));
    Ok(json!({"steps": out, "found": found_n, "moved": moved_n, "set": set_n}))
}

/// Per kind: elements, values per key, sources per key.
#[derive(Default)]
struct KindTally {
    count: usize,
    filled: BTreeMap<String, usize>,
    sources: BTreeMap<String, BTreeMap<&'static str, usize>>,
}

/// The summary of the model's properties (`bim.props.summary`'s `kinds`).
pub fn summary(d: &Drawing, kind: Option<&str>) -> Value {
    let upf = units_per_foot(d);
    let mut kinds: BTreeMap<String, KindTally> = BTreeMap::new();
    let mut total = 0usize;
    for (e, k) in elements(d) {
        if kind.is_some_and(|w| !w.eq_ignore_ascii_case(&k)) {
            continue;
        }
        total += 1;
        let t = kinds.entry(k.clone()).or_default();
        t.count += 1;
        for (key, p, _) in effective(e, &k, upf) {
            if !p.has_value() {
                continue;
            }
            let key = key.to_ascii_lowercase();
            *t.filled.entry(key.clone()).or_insert(0) += 1;
            *t.sources.entry(key).or_default().entry(p.source.kind.as_str()).or_insert(0) += 1;
        }
    }
    let out: serde_json::Map<String, Value> = kinds
        .into_iter()
        .map(|(k, t)| {
            let standard = props::standard_keys(&k);
            let mut filled: serde_json::Map<String, Value> = standard.iter().map(|s| (s.to_string(), json!(0))).collect();
            for (key, n) in t.filled {
                filled.insert(key, json!(n));
            }
            (k, json!({ "count": t.count, "filled": filled, "sources": t.sources, "standard": standard }))
        })
        .collect();
    json!({ "kinds": out, "elements": total })
}

fn run_summary(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = p.get("kind").and_then(Value::as_str).map(str::trim).filter(|k| !k.is_empty());
    let r = summary(s.doc()?, kind);
    if r["elements"] == json!(0) {
        let mut r = r;
        r["next_step"] = json!("the model has no elements yet: build it with model.build or bim.build");
        return Ok(r);
    }
    Ok(r)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub(crate) mod tests {
    use super::*;
    use cadaddy_doc::{Circle, Line, RawTag};
    use cadaddy_geom::Vec3;

    /// A synthetic model: `lines` sheet lines (plan objects, not elements) and `devices` built
    /// devices on `levels` levels, each with `keys` properties and a `location`.
    pub(crate) fn model(lines: usize, devices: usize, levels: usize, keys: usize) -> Session {
        let mut d = Drawing::new_imperial();
        let mut h = 0x100u64;
        let mut next = || {
            h += 1;
            Handle(h)
        };
        for i in 0..lines {
            let (x, y) = ((i % 1000) as f64 * 50.0, (i / 1000) as f64 * 50.0);
            let mut e = Entity::new(next(), EntityKind::Line(Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 30.0, y + 10.0, 0.0) }));
            e.common.layer = "SHEET".into();
            d.model.push(e);
        }
        for i in 0..devices {
            let level = format!("Level {}", i % levels.max(1) + 1);
            let (x, y) = ((i % 100) as f64 * 480.0, (i / 100) as f64 * 480.0);
            let mut e = Entity::new(next(), EntityKind::Circle(Circle { center: Vec3::new(x, y, 18.0), radius: 3.0 }));
            e.common.layer = "E-DEVICE".into();
            let tag = |c: i32, v: String| RawTag { code: c, value: v };
            let mut x = vec![
                tag(1001, super::super::wallgen::APP.into()),
                tag(1000, "device".into()),
                tag(1000, level.clone()),
                tag(1000, Handle(0x100 + i as u64).hex()),
            ];
            let mut store = Props::default();
            store.set("location", Prop::new(format!("Space {}", i / 25), Source::of(SourceKind::Print)));
            store.set("kind", Prop::new(if i % 3 == 0 { "receptacle" } else { "switch" }, Source::of(SourceKind::Legend)));
            for k in 0..keys.saturating_sub(2) {
                store.set(&format!("key_{k}"), Prop::new(format!("value {} {}", k, i % 7), Source::of(SourceKind::Schedule)));
            }
            x = props::write(&x, &store);
            e.common.xdata = x.into();
            d.model.push(e);
        }
        d.handseed = h + 1;
        let mut s = Session::empty();
        s.open_drawing(d, "Sample Model", None);
        s
    }

    /// What `bim.props.find` answered before the index (Q6438): every object of the model looked
    /// at again on every call. The index must give the same.
    fn found_by_scan(d: &Drawing, f: &Filter) -> Vec<(Handle, String, Option<String>)> {
        let upf = units_per_foot(d);
        let modelled: BTreeSet<String> = super::super::bspec::room_elements(d)
            .into_iter()
            .map(|r| r.number.filter(|n| !n.trim().is_empty()).unwrap_or(r.name).trim().to_ascii_uppercase())
            .collect();
        d.model
            .iter()
            .take(MAX_SCAN)
            .filter_map(|e| {
                let k = element_kind(e)?;
                let volume_of_element = !modelled.is_empty()
                    && super::super::wallgen::made_as(e).is_none()
                    && room_tag(e).is_some_and(|(name, number)| {
                        modelled.contains(&if number.trim().is_empty() { name } else { number }.trim().to_ascii_uppercase())
                    });
                (!volume_of_element).then_some((e, k))
            })
            .filter(|(_, k)| f.kind_ok(k))
            .filter(|(e, k)| f.wants.is_empty() || f.matches(&effective(e, k, upf)))
            .map(|(e, k)| (e.handle, k, level_of(e)))
            .collect()
    }

    fn tag(c: i32, v: &str) -> RawTag {
        RawTag { code: c, value: v.into() }
    }

    /// A model of every shape the matching cares about, over several chunks: keys in either case,
    /// a key twice, numbers written differently, blank values, room elements and the volumes of
    /// the same rooms and of others, and plan objects between.
    fn varied() -> Session {
        let mut d = Drawing::new_imperial();
        let mut h = 0x100u64;
        let values = ["2", "2.0", " 2 ", "-0", "0", "ABC", "abc", " ", "", "1e1", "10", "true", "x y"];
        for i in 0..3_000usize {
            h += 1;
            let at = Vec3::new(i as f64 * 10.0, 0.0, 0.0);
            let mut e = Entity::new(Handle(h), EntityKind::Circle(Circle { center: at, radius: 2.0 }));
            match i % 6 {
                // A plan object: not an element.
                0 => e.common.layer = "SHEET".into(),
                // A room element (a mesh made as `room`), numbered or only named.
                1 => {
                    let tri = vec![Vec3::new(at.x, 0.0, 0.0), Vec3::new(at.x + 9.0, 0.0, 0.0), Vec3::new(at.x, 9.0, 0.0)];
                    e = Entity::new(
                        Handle(h),
                        EntityKind::Mesh(cadaddy_doc::Mesh {
                            vertices: tri,
                            faces: vec![cadaddy_doc::MeshFace { v: vec![0, 1, 2], hidden: 0 }],
                            kind: Default::default(),
                        }),
                    );
                    let number = if i % 4 == 1 { format!("number:{}", i % 50) } else { "number:".into() };
                    e.common.xdata = vec![
                        tag(1001, super::super::wallgen::APP),
                        tag(1000, "room"),
                        tag(1000, if i % 2 == 0 { "Level 1" } else { "Level 2" }),
                        tag(1000, "1"),
                        tag(1000, &number),
                        tag(1000, &format!("name:Space {}", i % 9)),
                        tag(1000, if i % 4 == 1 { "area_sf:120" } else { "area_sf:" }),
                    ]
                    .into();
                }
                // A rooms.build volume: some of a room the model holds, some of none.
                2 => {
                    let number = if i % 4 == 0 { format!("{}", i % 50) } else { format!("{}", 1000 + i) };
                    e.common.xdata =
                        vec![tag(1001, super::super::rooms3d::ROOM_APP), tag(1000, &format!("Space {}", i % 9)), tag(1000, &number)].into();
                }
                // A built device with properties.
                _ => {
                    let mut x = vec![
                        tag(1001, super::super::wallgen::APP),
                        tag(1000, "device"),
                        tag(
                            1000,
                            if i % 5 == 0 {
                                ""
                            } else if i % 2 == 0 {
                                "Level 1"
                            } else {
                                "Level 2"
                            },
                        ),
                        tag(1000, "1"),
                    ];
                    let mut store = Props::default();
                    store.set(if i % 4 == 0 { "Mounting" } else { "mounting" }, Prop::new(values[i % values.len()], Source::of(SourceKind::Print)));
                    if i % 3 == 0 {
                        store.set("circuit", Prop::new(json!(i % 4), Source::user()));
                    }
                    if i % 7 == 0 {
                        store.set("checked", Prop::new(json!(i % 2 == 0), Source::user()));
                    }
                    x = props::write(&x, &store);
                    if i % 11 == 0 {
                        // The same key twice, in another case: the first with a value counts.
                        x = props::write(&x, &store);
                    }
                    e.common.xdata = x.into();
                }
            }
            d.model.push(e);
        }
        d.handseed = h + 1;
        let mut s = Session::empty();
        s.open_drawing(d, "Sample Model", None);
        s
    }

    #[test]
    fn the_index_finds_what_a_full_scan_finds() {
        let mut s = varied();
        let filters = [
            json!({}),
            json!({"kind": "room"}),
            json!({"kind": "device"}),
            json!({"kind": "device", "where": {"level": "Level 2"}}),
            json!({"where": {"level": "level 1"}}),
            json!({"where": {"mounting": "2"}}),
            json!({"where": {"MOUNTING": "abc"}}),
            json!({"where": {"mounting": 0}}),
            json!({"where": {"mounting": "10"}}),
            json!({"where": {"mounting": "x y"}}),
            json!({"where": {"mounting": "missing"}}),
            json!({"where": {"mounting": "exists", "circuit": 3}}),
            json!({"where": {"checked": true}}),
            json!({"where": {"checked": "missing", "level": "exists"}}),
            json!({"kind": "room", "where": {"number": "exists"}}),
            json!({"kind": "room", "where": {"name": "space 4"}}),
            json!({"where": {"area": "exists"}}),
            json!({"where": {"nothing": "x"}}),
        ];
        let check = |s: &Session| {
            let mut answered = 0;
            for f in &filters {
                let f = Filter::parse(FIND, f).unwrap();
                let d = s.doc().unwrap();
                let hits = found(d, &f);
                answered += usize::from(!hits.is_empty());
                assert_eq!(hits, found_by_scan(d, &f), "{f:?}");
            }
            assert!(answered >= filters.len() - 1, "the filters find something: {answered} of {}", filters.len());
            let d = s.doc().unwrap();
            let by_index: Vec<(Handle, String)> = elements(d).map(|(e, k)| (e.handle, k)).collect();
            let by_scan: Vec<(Handle, String)> = found_by_scan(d, &Filter::default()).into_iter().map(|(h, k, _)| (h, k)).collect();
            assert_eq!(by_index, by_scan);
        };
        check(&s);
        // After edits: a property set, an object erased, one added.
        let some = found(s.doc().unwrap(), &Filter::parse(FIND, &json!({"kind": "device"})).unwrap());
        let h = some.get(17).unwrap().0;
        s.execute(SET, &json!({"handle": h.hex(), "key": "mounting", "value": "ABC"})).unwrap();
        let gone = some.get(400).unwrap().0;
        s.doc_mut().unwrap().model.remove(gone);
        check(&s);
        assert!(found(s.doc().unwrap(), &Filter::parse(FIND, &json!({"where": {"mounting": "abc"}})).unwrap()).iter().any(|(x, _, _)| *x == h));
    }

    #[test]
    fn a_find_after_an_edit_looks_again_only_at_the_chunk_it_changed() {
        let mut s = model(50_000, 2_000, 4, 20);
        let chunks = s.doc().unwrap().model.chunk_refs().len();
        let r = s.execute(FIND, &json!({"kind": "device", "where": {"level": "Level 2", "location": "Space 9"}, "select": true})).unwrap();
        assert!(r["count"].as_u64().unwrap() > 0, "{r}");
        let (_, again) = indexed(s.doc().unwrap());
        assert_eq!((again.built, again.reused), (0, chunks), "an unchanged model is not looked at again");
        s.execute("3dmove", &json!({"delta": [6, 0, 0]})).unwrap();
        s.execute(SET, &json!({"props": {"mounting": "18 in AFF"}})).unwrap();
        let (_, after) = indexed(s.doc().unwrap());
        assert!(after.built <= 2 && after.reused + after.built == chunks, "{after:?} of {chunks}");
    }

    fn center_of(s: &Session, h: Handle) -> Vec3 {
        match &s.doc().unwrap().model.get(h).unwrap().kind {
            EntityKind::Circle(c) => c.center,
            k => panic!("{k:?}"),
        }
    }

    #[test]
    fn a_batch_finds_moves_and_sets_in_turn_as_one_undo_step() {
        let mut s = model(2_000, 500, 4, 6);
        let q = json!({"kind": "device", "where": {"level": "Level 2", "location": "Space 3"}});
        let hits: Vec<Handle> = found(s.doc().unwrap(), &Filter::parse(FIND, &q).unwrap()).into_iter().map(|(h, _, _)| h).collect();
        assert!(!hits.is_empty());
        let before: Vec<Vec3> = hits.iter().map(|h| center_of(&s, *h)).collect();
        let r = s
            .execute(
                BATCH,
                &json!({"select": true, "steps": [
                    {"kind": "device", "where": {"level": "Level 2", "location": "Space 3"}, "delta": [6, 0, 0], "props": {"mounting": "18 in AFF"}, "note": "typical room"},
                    // Finds what the step before it set.
                    {"where": {"mounting": "18 in aff"}, "props": {"checked": true}},
                    {"kind": "device", "where": {"location": "No Such Space"}, "props": {"checked": false}},
                ]}),
            )
            .unwrap();
        let n = hits.len() as u64;
        assert_eq!(
            r["steps"],
            json!([{"found": n, "moved": n, "set": n}, {"found": n, "moved": 0, "set": n}, {"found": 0, "moved": 0, "set": 0}]),
            "{r}"
        );
        assert_eq!((r["found"].as_u64(), r["moved"].as_u64(), r["set"].as_u64()), (Some(2 * n), Some(n), Some(2 * n)));
        for (h, b) in hits.iter().zip(&before) {
            assert!((center_of(&s, *h).x - b.x - 6.0).abs() < 1e-9);
        }
        let mut sel = s.selection();
        sel.sort_unstable();
        let mut want = hits.clone();
        want.sort_unstable();
        assert_eq!(sel, want);
        let checked = s.execute(FIND, &json!({"where": {"checked": true, "mounting": "18 in AFF"}})).unwrap();
        assert_eq!(checked["count"].as_u64(), Some(n));
        // One undo step takes the whole batch back.
        s.execute("undo", &json!({})).unwrap();
        for (h, b) in hits.iter().zip(&before) {
            assert!((center_of(&s, *h).x - b.x).abs() < 1e-9);
        }
        assert_eq!(s.execute(FIND, &json!({"where": {"checked": "exists"}})).unwrap()["count"], json!(0));
    }

    #[test]
    fn a_batch_with_a_bad_step_changes_nothing_and_says_which_step() {
        let mut s = model(100, 50, 2, 4);
        let bad_steps = [
            json!({}),
            json!({"steps": []}),
            json!({"steps": "x"}),
            json!({"steps": [{"kind": "device", "props": {"a": 1}}, {"kind": "device"}]}),
            json!({"steps": [{"kind": "device", "props": {"a": 1}}, {"kind": "device", "delta": [1, "x"]}]}),
            json!({"steps": [{"kind": "device", "props": {"a": 1}}, {"kind": "device", "delta": [1, 2, 3, 4]}]}),
            json!({"steps": [{"kind": "device", "props": {"a": 1}}, {"kind": "device", "props": {}}]}),
            json!({"steps": [{"kind": "device", "props": {"a": 1}}, {"kind": "device", "props": {"a": [1]}}]}),
            json!({"steps": [{"kind": "device", "props": {"a": 1}}, {"kind": "device", "where": "x", "props": {"a": 1}}]}),
            json!({"steps": [{"kind": "device", "props": {"a": 1}}, 7]}),
        ];
        for p in &bad_steps {
            let e = s.execute(BATCH, p).unwrap_err().to_string();
            assert!(p["steps"].as_array().is_none_or(|a| a.len() < 2) || e.contains("step 2"), "{p}: {e}");
        }
        assert_eq!(s.execute(FIND, &json!({"where": {"a": "exists"}})).unwrap()["count"], json!(0));
        let many: Vec<Value> = (0..BATCH_MAX + 1).map(|_| json!({"props": {"a": 1}})).collect();
        assert!(s.execute(BATCH, &json!({"steps": many})).is_err());
    }

    /// Timings of a scripted correction on a large synthetic model (run by hand:
    /// `cargo test -p cadaddy-engine --lib props_find_speed -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn props_find_speed() {
        let lines: usize = std::env::var("PF_LINES").ok().and_then(|v| v.parse().ok()).unwrap_or(1_000_000);
        let steps: usize = std::env::var("PF_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
        let t = std::time::Instant::now();
        let mut s = model(lines, 5_000, 4, 20);
        eprintln!("model: {:?} ({} objects)", t.elapsed(), s.doc().unwrap().model.len());
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
        let (mut find_any, mut find_dev, mut mv, mut set1, mut set2) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for i in 0..steps {
            let loc = format!("Space {}", i * 7 % 200);
            let t = std::time::Instant::now();
            let r = s.execute(FIND, &json!({"where": {"level": "Level 2", "location": loc}})).unwrap();
            find_any += ms(t);
            let t = std::time::Instant::now();
            let r2 = s.execute(FIND, &json!({"kind": "device", "where": {"level": "Level 2", "location": loc}, "select": true})).unwrap();
            find_dev += ms(t);
            assert_eq!(r["count"], r2["count"]);
            assert!(r2["count"].as_u64().unwrap() > 0, "{r2}");
            let t = std::time::Instant::now();
            s.execute("3dmove", &json!({"delta": [6, 0, 0]})).unwrap();
            mv += ms(t);
            let t = std::time::Instant::now();
            s.execute(SET, &json!({"props": {"mounting": "18 in AFF", "note_k": i}})).unwrap();
            set1 += ms(t);
            let t = std::time::Instant::now();
            s.execute(SET, &json!({"props": {"checked": true}})).unwrap();
            set2 += ms(t);
        }
        // The same corrections as one batch: a step per typical room.
        let batch: Vec<Value> = (0..steps)
            .map(|i| json!({"kind": "device", "where": {"level": "Level 3", "location": format!("Space {}", i * 7 % 200)}, "delta": [6, 0, 0], "props": {"mounting": "18 in AFF"}}))
            .collect();
        let t = std::time::Instant::now();
        let r = s.execute(BATCH, &json!({"steps": batch})).unwrap();
        eprintln!("batch of {steps} rooms: {:.1} ms ({} found)", ms(t), r["found"]);
        let n = steps as f64;
        eprintln!(
            "per step (ms): find no kind {:.1}, find device {:.1}, 3dmove {:.1}, first set {:.1}, second set {:.1}",
            find_any / n,
            find_dev / n,
            mv / n,
            set1 / n,
            set2 / n
        );
    }
}
