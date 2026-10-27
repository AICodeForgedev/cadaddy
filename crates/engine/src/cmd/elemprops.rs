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
//! the build's tag), a room's area, and a room volume's name and number (read from its tag on the
//! plan, so `print`). A stored value always wins.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::props::{self, Entry, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const GET: &str = "bim.props.get";
const SET: &str = "bim.props.set";
const FIND: &str = "bim.props.find";
const SUMMARY: &str = "bim.props.summary";

/// Most model objects one call looks at.
const MAX_SCAN: usize = 2_000_000;
/// Most elements `bim.props.get` returns, and `bim.props.find` lists by default and at all.
const GET_MAX: usize = 500;
const LIST_DEFAULT: usize = 200;
const LIST_MAX: usize = 100_000;
/// Most elements one `bim.props.set` changes.
const SET_MAX: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(GET, "Element Properties (BIM)", run_get)
            .params("{handle? | handles?: [string] (default: the selection)} → {elements: [{handle, kind, props: {key: {value, unit?, source: {kind: print|legend|schedule|spec|note|rule|default|user|computed, doc?, at?}, confidence?, note?, stored}}, order: [key], standard: [key], missing: [key]}]}")
            .noundo(),
        CommandSpec::new(SET, "Set Element Property (BIM)", run_set)
            .params("{handle? | handles? | filter?: {kind?, where?: {key: value | \"exists\" | \"missing\"}} (default: the selection), key?: string, value?: string | number | bool | null (null clears), unit?, note?, props?: {key: value} (several at once; with or instead of key)} → {set, elements: [{handle, kind, override?}], skipped}: source user; kept through rebuilds"),
        CommandSpec::new(FIND, "Find Elements by Property", run_find)
            .params("{kind?: string (wall, door, window, room, floor, ceiling, roof, device, fixture, light_fixture, equipment, duct, pipe, conduit, or a build part such as column), where?: {key: value | \"exists\" | \"missing\"}, select?: bool, limit? (default 200)} → {count, handles, elements: [{handle, kind, level?}], more}")
            .noundo(),
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

fn kind_of_part<'a>(e: &Entity, part: &'a str) -> &'a str {
    let layer = e.common.layer.to_ascii_uppercase();
    match part {
        "glass" | "window" | "curtainwall" => "window",
        "soffit" | "grid" | "ceiling" => "ceiling",
        "roof" | "cricket" => "roof",
        "light" => "light_fixture",
        "diffuser" => "fixture",
        "device" if layer.contains("LIGHT") || layer.contains("LTG") => "light_fixture",
        "device" if layer.contains("PLUMB") || layer.contains("FIXT") || layer.contains("SPRINK") => "fixture",
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
    d.building_spec
        .as_deref()
        .and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok())
        .map(|sp| sp.units_per_foot)
        .filter(|u| u.is_finite() && *u > 0.0)
        .unwrap_or(12.0)
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

/// The model's elements: (entity, kind). A room counts once: a `rooms.build` volume of a room
/// the model holds as a room element (same number, or unnumbered the same name) is that room's
/// 3D view, not a second room.
pub(crate) fn elements(d: &Drawing) -> impl Iterator<Item = (&std::sync::Arc<Entity>, String)> {
    let modelled: BTreeSet<String> = super::bspec::room_elements(d)
        .into_iter()
        .map(|r| r.number.filter(|n| !n.trim().is_empty()).unwrap_or(r.name).trim().to_ascii_uppercase())
        .collect();
    d.model.iter().take(MAX_SCAN).filter_map(move |e| {
        let k = element_kind(e)?;
        let volume_of_element = !modelled.is_empty()
            && super::wallgen::made_as(e).is_none()
            && room_tag(e)
                .is_some_and(|(name, number)| modelled.contains(&if number.trim().is_empty() { name } else { number }.trim().to_ascii_uppercase()));
        (!volume_of_element).then_some((e, k))
    })
}

fn found(d: &Drawing, f: &Filter) -> Vec<(Handle, String, Option<String>)> {
    let upf = units_per_foot(d);
    elements(d)
        .filter(|(_, k)| f.kind_ok(k))
        .filter(|(e, k)| f.wants.is_empty() || f.matches(&effective(e, k, upf)))
        .map(|(e, k)| (e.handle, k, level_of(e)))
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
