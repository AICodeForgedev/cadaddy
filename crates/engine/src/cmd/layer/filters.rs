//! Layer filters (the Layer Properties Manager's filter tree): property filters (rules on name,
//! colour, linetype, description and the on / frozen / locked / plot states) and group filters
//! (chosen layers), optionally nested: a layer shown by a nested filter passes its parents too.
//! They are kept with the drawing.

use cadaddy_color::Color;
use cadaddy_doc::{Drawing, Layer, LayerFilter, LayerFilterKind, LayerRules};
use serde_json::{Value, json};

use super::super::qselect::wildcard;
use super::super::*;
use crate::{Result, Session};

/// Most filters a drawing keeps, and most layers a group lists.
const MAX_FILTERS: usize = 1000;
const MAX_GROUP: usize = 100_000;

/// A comma-separated list of patterns: a value passes when it matches one pattern without `~`
/// and none with it (only `~` patterns: anything they don't exclude).
pub fn patterns_match(spec: &str, text: &str) -> bool {
    let mut any_plain = false;
    let mut hit = false;
    for p in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match p.strip_prefix('~') {
            Some(x) => {
                if wildcard(x, text) {
                    return false;
                }
            }
            None => {
                any_plain = true;
                hit |= wildcard(p, text);
            }
        }
    }
    hit || !any_plain
}

fn color_matches(spec: &str, c: Color) -> bool {
    spec.split(',').map(str::trim).filter(|p| !p.is_empty()).any(|p| Color::parse(p).is_some_and(|x| x == c) || p.eq_ignore_ascii_case(&c.name()))
}

/// Whether layer `l` passes `rules`.
pub fn passes(rules: &LayerRules, l: &Layer) -> bool {
    rules.name.as_deref().is_none_or(|s| patterns_match(s, &l.name))
        && rules.color.as_deref().is_none_or(|s| color_matches(s, l.color))
        && rules.linetype.as_deref().is_none_or(|s| patterns_match(s, &l.linetype))
        && rules.description.as_deref().is_none_or(|s| patterns_match(s, &l.description))
        && rules.on.is_none_or(|v| v == l.on)
        && rules.frozen.is_none_or(|v| v == l.frozen)
        && rules.locked.is_none_or(|v| v == l.locked)
        && rules.plot.is_none_or(|v| v == l.plot)
}

fn find<'a>(d: &'a Drawing, name: &str) -> Option<&'a LayerFilter> {
    d.layer_filters.iter().find(|f| f.name.eq_ignore_ascii_case(name.trim()))
}

/// Whether `l` is shown by filter `f` (and its parents).
fn shows(d: &Drawing, f: &LayerFilter, l: &Layer, depth: usize) -> bool {
    if depth > 32 {
        return false;
    }
    let own = match &f.kind {
        LayerFilterKind::Property(r) => passes(r, l),
        LayerFilterKind::Group { layers } => layers.iter().any(|n| n.eq_ignore_ascii_case(&l.name)),
    };
    own && f.parent.as_deref().and_then(|p| find(d, p)).is_none_or(|p| shows(d, p, l, depth + 1))
}

/// The layers filter `name` shows (with `invert`, the others), in table order.
pub fn layers_of(d: &Drawing, name: &str, invert: bool) -> Result<Vec<String>> {
    let f = find(d, name).ok_or_else(|| other(format!("No layer filter named \"{}\".", name.trim())))?;
    Ok(d.layers.iter().filter(|l| shows(d, f, l, 0) != invert).map(|l| l.name.clone()).collect())
}

fn other(m: impl Into<String>) -> crate::EngineError {
    crate::EngineError::Other(m.into())
}

fn opt_bool(p: &Value, k: &str) -> Result<Option<bool>> {
    match p.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad("layer.filter", format!("`{k}` must be true or false"))),
    }
}

fn opt_text(p: &Value, k: &str) -> Result<Option<String>> {
    match p.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.len() <= 4096 => Ok(Some(s.trim().to_string()).filter(|s| !s.is_empty())),
        Some(_) => Err(bad("layer.filter", format!("`{k}` must be text"))),
    }
}

/// Property rules from JSON `{name?, color?, linetype?, description?, on?, frozen?, locked?, plot?}`.
fn rules(p: &Value) -> Result<LayerRules> {
    if !p.is_object() {
        return Err(bad("layer.filter", "`rules` is an object"));
    }
    Ok(LayerRules {
        name: opt_text(p, "name")?,
        color: opt_text(p, "color")?,
        linetype: opt_text(p, "linetype")?,
        description: opt_text(p, "description")?,
        on: opt_bool(p, "on")?,
        frozen: opt_bool(p, "frozen")?,
        locked: opt_bool(p, "locked")?,
        plot: opt_bool(p, "plot")?,
    })
}

fn names(v: &Value) -> Result<Vec<String>> {
    let a = v.as_array().filter(|a| a.len() <= MAX_GROUP).ok_or_else(|| bad("layer.filter", "layers are a list of names"))?;
    a.iter().map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or_else(|| bad("layer.filter", "layers are a list of names"))).collect()
}

/// The layer names of `want` as the drawing spells them; unknown ones are refused.
fn existing(d: &Drawing, want: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for n in want {
        let l = d.layer(n).ok_or_else(|| other(format!("No layer named \"{n}\".")))?;
        if !out.iter().any(|x| x.eq_ignore_ascii_case(&l.name)) {
            out.push(l.name.clone());
        }
    }
    Ok(out)
}

fn valid_filter_name(n: &str) -> bool {
    let n = n.trim();
    !n.is_empty()
        && n.chars().count() <= 255
        && !n.contains(['<', '>', '/', '\\', '"', ':', ';', '?', '*', '|', '=', '`'])
        && !n.eq_ignore_ascii_case("All")
}

fn filter_json(d: &Drawing, f: &LayerFilter) -> Value {
    let count = layers_of(d, &f.name, false).map_or(0, |v| v.len());
    let mut v = json!({"name": f.name, "parent": f.parent, "layers": count});
    match &f.kind {
        LayerFilterKind::Property(r) => {
            v["type"] = json!("property");
            v["rules"] = serde_json::to_value(r).unwrap_or(Value::Null);
        }
        LayerFilterKind::Group { layers } => {
            v["type"] = json!("group");
            v["members"] = json!(layers);
        }
    }
    v
}

pub(super) fn run_new(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").map(str::trim).ok_or_else(|| bad("layer.filter.new", "`name` is required"))?.to_string();
    if !valid_filter_name(&name) {
        return Err(bad("layer.filter.new", format!("\"{name}\" is not a valid filter name")));
    }
    let d = s.doc()?;
    if find(d, &name).is_some() {
        return Err(other(format!("A layer filter named \"{name}\" already exists.")));
    }
    if d.layer_filters.len() >= MAX_FILTERS {
        return Err(other("The drawing has too many layer filters."));
    }
    let parent = match opt_text(p, "parent")? {
        Some(pn) => Some(find(d, &pn).map(|f| f.name.clone()).ok_or_else(|| other(format!("No layer filter named \"{pn}\".")))?),
        None => None,
    };
    let kind = match p.get("type").and_then(Value::as_str).unwrap_or(if p.get("layers").is_some() { "group" } else { "property" }) {
        "group" => LayerFilterKind::Group { layers: existing(d, &p.get("layers").map(names).transpose()?.unwrap_or_default())? },
        "property" => LayerFilterKind::Property(rules(p.get("rules").unwrap_or(&json!({})))?),
        t => return Err(bad("layer.filter.new", format!("`type` is \"property\" or \"group\", not \"{t}\""))),
    };
    let f = LayerFilter { name, parent, kind };
    s.doc_mut()?.layer_filters.push(f.clone());
    Ok(filter_json(s.doc()?, &f))
}

pub(super) fn run_edit(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").ok_or_else(|| bad("layer.filter.edit", "`name` is required"))?.to_string();
    let d = s.doc()?;
    let mut f = find(d, &name).cloned().ok_or_else(|| other(format!("No layer filter named \"{}\".", name.trim())))?;
    let old = f.name.clone();
    if let Some(n) = opt_text(p, "newName")? {
        if !valid_filter_name(&n) {
            return Err(bad("layer.filter.edit", format!("\"{n}\" is not a valid filter name")));
        }
        if !n.eq_ignore_ascii_case(&old) && find(d, &n).is_some() {
            return Err(other(format!("A layer filter named \"{n}\" already exists.")));
        }
        f.name = n;
    }
    match &mut f.kind {
        LayerFilterKind::Property(r) => {
            if let Some(v) = p.get("rules") {
                *r = rules(v)?;
            }
        }
        LayerFilterKind::Group { layers } => {
            if let Some(v) = p.get("layers") {
                *layers = existing(d, &names(v)?)?;
            }
            if let Some(v) = p.get("addLayers") {
                for n in existing(d, &names(v)?)? {
                    if !layers.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                        layers.push(n);
                    }
                }
            }
            if let Some(v) = p.get("removeLayers") {
                let gone = names(v)?;
                layers.retain(|x| !gone.iter().any(|g| g.eq_ignore_ascii_case(x)));
            }
        }
    }
    let new_name = f.name.clone();
    let d = s.doc_mut()?;
    for x in &mut d.layer_filters {
        if x.name == old {
            *x = f.clone();
        } else if x.parent.as_deref() == Some(old.as_str()) {
            x.parent = Some(new_name.clone());
        }
    }
    Ok(filter_json(s.doc()?, &f))
}

pub(super) fn run_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").ok_or_else(|| bad("layer.filter.delete", "`name` is required"))?;
    let d = s.doc()?;
    let f = find(d, name).cloned().ok_or_else(|| other(format!("No layer filter named \"{}\".", name.trim())))?;
    // The filter and the ones nested in it.
    let mut gone = vec![f.name.clone()];
    loop {
        let more: Vec<String> = d
            .layer_filters
            .iter()
            .filter(|x| !gone.contains(&x.name) && x.parent.as_ref().is_some_and(|p| gone.contains(p)))
            .map(|x| x.name.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        gone.extend(more);
    }
    s.doc_mut()?.layer_filters.retain(|x| !gone.contains(&x.name));
    Ok(json!({ "deleted": gone }))
}

pub(super) fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    Ok(json!({ "filters": d.layer_filters.iter().map(|f| filter_json(d, f)).collect::<Vec<_>>() }))
}

pub(super) fn run_layers(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let invert = bool_or(p, "invert", false);
    let layers = match str_param(p, "name").filter(|n| !n.trim().is_empty() && !n.trim().eq_ignore_ascii_case("All")) {
        Some(n) => layers_of(d, n, invert)?,
        None if invert => Vec::new(),
        None => d.layers.iter().map(|l| l.name.clone()).collect(),
    };
    Ok(json!({ "layers": layers }))
}
