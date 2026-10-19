//! Making blocks dynamic: BPARAMETER adds a parameter, BACTIONTOOL an action that follows one,
//! BVSTATE edits the visibility states (all on a block definition, by name; each one undo step).

use cadaddy_doc::Handle;
use cadaddy_doc::dynblock::author::{self, NewActionKind, NewParameter};
use cadaddy_doc::dynblock::{self, ValueSet};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use crate::cmd::curves::handle_value;
use crate::cmd::{CommandSpec, bad, bool_or, has_doc, point_param, points_param, str_param};
use crate::{Result, Session};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bparameter", "Parameter", run_parameter)
            .params("{block, type: point|linear|polar|xy|rotation|alignment|flip|visibility|lookup|basepoint, base?: [x,y], end?: [x,y] (two-point types), at?: [x,y] (one-point types), label?, description?, values?: {list: [d…]} | {minimum?, maximum?, increment?} (linear), fromMidpoint?: bool, states?: [{name, handles}] (visibility)} → {node, label, dynamic}")
            .enabled(has_doc),
        CommandSpec::new("bactiontool", "Action", run_action)
            .params("{block, type: move|stretch|scale|rotate|flip|array|polarstretch|lookup, parameter (its label or graph id), handles: [objects of the block], frame?: [[x,y]…] (stretch), at?: [x,y], name?, columns?: [{parameter, property?, lookup?: bool}], rows?: [[value…]] (lookup)} → {node, name, dynamic}")
            .enabled(has_doc),
        CommandSpec::new("bvstate", "Visibility States", run_vstate)
            .params("{block, action?: list (default) | new | set | rename | delete, name?, newName?, handles?: [the state's visible objects]} → {states: [{name, visible}]}")
            .enabled(has_doc),
        CommandSpec::new("bremove", "Remove Parameter or Action", run_remove)
            .params("{block, element (a parameter's label, an action's name or a graph id)} → {dynamic}: a parameter goes with its grips and the actions following it")
            .enabled(has_doc),
    ]
}

fn block_name(s: &Session, cmd: &str, p: &Value) -> Result<String> {
    let name = str_param(p, "block").or_else(|| str_param(p, "name")).ok_or_else(|| bad(cmd, "`block` is required"))?;
    let b = s.doc()?.block(name).ok_or_else(|| bad(cmd, format!("no block `{name}`")))?;
    if b.name.starts_with('*') {
        return Err(bad(cmd, "layouts and anonymous blocks can't be made dynamic"));
    }
    Ok(b.name.clone())
}

fn v3(p: Vec2) -> Vec3 {
    Vec3::new(p.x, p.y, 0.0)
}

fn handles(cmd: &str, v: Option<&Value>) -> Result<Vec<Handle>> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(|h| handle_value(h).ok_or_else(|| bad(cmd, format!("{h} is not an object handle")))).collect(),
        Some(_) => Err(bad(cmd, "`handles` must be an array of handles")),
    }
}

/// A parameter or action of the block by its label, name or graph id.
fn element(s: &Session, cmd: &str, block: &str, v: Option<&Value>) -> Result<i32> {
    let m = dynblock::dynamic_block(s.doc()?, block).ok_or_else(|| bad(cmd, format!("block `{block}` has no parameters yet")))?;
    if let Some(n) = v.and_then(Value::as_i64).and_then(|n| i32::try_from(n).ok()) {
        return Ok(n);
    }
    let key = v.and_then(Value::as_str).ok_or_else(|| bad(cmd, "a parameter's label or graph id is required"))?.trim();
    if let Ok(n) = key.parse::<i32>() {
        return Ok(n);
    }
    m.parameters
        .iter()
        .find(|p| p.kind.label().is_some_and(|l| l.eq_ignore_ascii_case(key)) || p.element.eq_ignore_ascii_case(key))
        .map(|p| p.node)
        .or_else(|| m.actions.iter().find(|a| a.name.eq_ignore_ascii_case(key)).map(|a| a.node))
        .ok_or_else(|| bad(cmd, format!("block `{block}` has no parameter or action \"{key}\"")))
}

/// Change the session's drawing with `f` (one undo step).
fn change<T>(s: &mut Session, cmd: &str, f: impl FnOnce(&mut cadaddy_doc::Drawing) -> std::result::Result<T, String>) -> Result<T> {
    let mut d = s.doc()?.clone();
    let out = f(&mut d).map_err(|e| bad(cmd, e))?;
    *s.doc_mut()? = d;
    Ok(out)
}

fn dynamic(s: &Session, block: &str) -> Result<Value> {
    serde_json::to_value(dynblock::dynamic_block(s.doc()?, block)).map_err(|e| bad("bparameter", e.to_string()))
}

fn value_set(cmd: &str, v: Option<&Value>) -> Result<ValueSet> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(ValueSet::default()) };
    let num = |k: &str| -> Result<Option<f64>> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(n) => n.as_f64().filter(|n| n.is_finite()).map(Some).ok_or_else(|| bad(cmd, format!("`values.{k}` must be a number"))),
        }
    };
    let mut set = ValueSet::default();
    if let Some(list) = v.get("list") {
        let a = list.as_array().ok_or_else(|| bad(cmd, "`values.list` must be an array of distances"))?;
        let mut l: Vec<f64> = a
            .iter()
            .map(|n| n.as_f64().filter(|n| n.is_finite() && *n > 0.0).ok_or_else(|| bad(cmd, "`values.list` must hold positive numbers")))
            .collect::<Result<_>>()?;
        l.sort_by(f64::total_cmp);
        l.dedup();
        set.flags |= 8;
        set.list = l;
    }
    if let Some(n) = num("minimum")? {
        set.flags |= 1;
        set.minimum = n;
    }
    if let Some(n) = num("maximum")? {
        set.flags |= 2;
        set.maximum = n;
    }
    if let Some(n) = num("increment")? {
        if n <= 0.0 {
            return Err(bad(cmd, "`values.increment` must be positive"));
        }
        set.flags |= 4;
        set.increment = n;
    }
    if set.has_minimum() && set.has_maximum() && set.minimum > set.maximum {
        return Err(bad(cmd, "`values.minimum` is above `values.maximum`"));
    }
    Ok(set)
}

fn run_parameter(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "bparameter";
    let block = block_name(s, cmd, p)?;
    let kind = str_param(p, "type").ok_or_else(|| bad(cmd, "`type` is required"))?.to_ascii_lowercase().replace([' ', '-', '_'], "");
    let two = || -> Result<(Vec3, Vec3)> {
        let b = point_param(p, "base").ok_or_else(|| bad(cmd, "`base` [x, y] is required"))?;
        let e = point_param(p, "end").ok_or_else(|| bad(cmd, "`end` [x, y] is required"))?;
        if b.near(e, 1e-9) {
            return Err(bad(cmd, "`base` and `end` must differ"));
        }
        Ok((v3(b), v3(e)))
    };
    let at = || point_param(p, "at").map(v3).ok_or_else(|| bad(cmd, "`at` [x, y] is required"));
    let np = match kind.as_str() {
        "linear" => {
            let (base, end) = two()?;
            NewParameter::Linear { base, end, values: value_set(cmd, p.get("values"))?, from_midpoint: bool_or(p, "fromMidpoint", false) }
        }
        "polar" => two().map(|(base, end)| NewParameter::Polar { base, end })?,
        "xy" => two().map(|(base, end)| NewParameter::Xy { base, end })?,
        "rotation" => two().map(|(base, end)| NewParameter::Rotation { base, end })?,
        "alignment" => two().map(|(base, end)| NewParameter::Alignment { base, end })?,
        "flip" => two().map(|(base, end)| NewParameter::Flip { base, end })?,
        "point" => NewParameter::Point { at: at()? },
        "lookup" => NewParameter::Lookup { at: at()? },
        "basepoint" => NewParameter::BasePoint { at: at()? },
        "visibility" => {
            let mut states = Vec::new();
            match p.get("states") {
                None | Some(Value::Null) => {
                    let all: Vec<Handle> = s.doc()?.block(&block).map(|b| b.entities.iter().map(|e| e.handle).collect()).unwrap_or_default();
                    states.push(("VisibilityState0".to_string(), all));
                }
                Some(Value::Array(a)) => {
                    for st in a.iter().take(1000) {
                        let n = str_param(st, "name").ok_or_else(|| bad(cmd, "each state needs a `name`"))?;
                        states.push((n.trim().to_string(), handles(cmd, st.get("handles"))?));
                    }
                }
                Some(_) => return Err(bad(cmd, "`states` must be an array of {name, handles}")),
            }
            NewParameter::Visibility { at: at()?, states }
        }
        other => return Err(bad(cmd, format!("unknown parameter type \"{other}\""))),
    };
    let label = str_param(p, "label");
    let description = str_param(p, "description").unwrap_or("");
    let (node, label) = change(s, cmd, |d| author::add_parameter(d, &block, &np, label, description))?;
    Ok(json!({ "node": node, "label": label, "dynamic": dynamic(s, &block)? }))
}

fn run_action(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "bactiontool";
    let block = block_name(s, cmd, p)?;
    let kind = str_param(p, "type").ok_or_else(|| bad(cmd, "`type` is required"))?.to_ascii_lowercase().replace([' ', '-', '_'], "");
    let node = element(s, cmd, &block, p.get("parameter"))?;
    let selection = handles(cmd, p.get("handles"))?;
    let ak = match kind.as_str() {
        "stretch" => NewActionKind::Stretch { frame: points_param(p, "frame").ok_or_else(|| bad(cmd, "`frame` [[x, y]…] is required"))? },
        "move" => NewActionKind::Move,
        "scale" => NewActionKind::Scale,
        "rotate" => NewActionKind::Rotate,
        "flip" => NewActionKind::Flip,
        "array" => NewActionKind::Array,
        "polarstretch" => NewActionKind::PolarStretch,
        "lookup" => {
            let cols = p.get("columns").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`columns` is required"))?;
            let mut columns = Vec::new();
            for c in cols.iter().take(100) {
                let n = element(s, cmd, &block, c.get("parameter"))?;
                let lookup = bool_or(c, "lookup", false);
                // The value the column reads or sets, by the parameter's kind.
                let kind = dynblock::dynamic_block(s.doc()?, &block).and_then(|m| m.parameters.into_iter().find(|p| p.node == n).map(|p| p.kind));
                let default = match kind {
                    Some(dynblock::ParameterKind::Lookup { .. }) => "LookupString",
                    Some(dynblock::ParameterKind::Visibility { .. }) => "VisibilityState",
                    _ => "UpdatedDistance",
                };
                columns.push((n, str_param(c, "property").unwrap_or(default).to_string(), lookup));
            }
            let rows = p
                .get("rows")
                .and_then(Value::as_array)
                .ok_or_else(|| bad(cmd, "`rows` is required"))?
                .iter()
                .take(10_000)
                .map(|r| {
                    r.as_array()
                        .ok_or_else(|| bad(cmd, "each row must be an array of values"))
                        .map(|a| a.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string)).collect())
                })
                .collect::<Result<Vec<Vec<String>>>>()?;
            NewActionKind::Lookup { columns, rows }
        }
        other => return Err(bad(cmd, format!("unknown action type \"{other}\""))),
    };
    let at = point_param(p, "at").map(v3).unwrap_or_default();
    let (me, name) = change(s, cmd, |d| author::add_action(d, &block, node, &ak, &selection, at, str_param(p, "name")))?;
    Ok(json!({ "node": me, "name": name, "dynamic": dynamic(s, &block)? }))
}

/// The visibility states of `block`: names and visible objects.
fn states(s: &Session, block: &str) -> Result<Vec<(String, Vec<Handle>)>> {
    let m = dynblock::dynamic_block(s.doc()?, block);
    let st = m.and_then(|m| {
        m.parameters.into_iter().find_map(|p| match p.kind {
            dynblock::ParameterKind::Visibility { states, .. } => Some(states.into_iter().map(|s| (s.name, s.visible)).collect()),
            _ => None,
        })
    });
    st.ok_or_else(|| bad("bvstate", format!("block `{block}` has no visibility parameter (add one with BPARAMETER)")))
}

fn listing(s: &Session, block: &str) -> Result<Value> {
    let st = states(s, block)?;
    Ok(
        json!({ "states": st.iter().map(|(n, v)| json!({ "name": n, "visible": v.iter().map(|h| h.hex()).collect::<Vec<_>>() })).collect::<Vec<_>>() }),
    )
}

fn run_vstate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "bvstate";
    let block = block_name(s, cmd, p)?;
    let action = str_param(p, "action").unwrap_or("list").to_ascii_lowercase();
    let mut st = states(s, &block)?;
    let name = || {
        str_param(p, "name")
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| bad(cmd, format!("`name` is required for \"{action}\"")))
    };
    let find = |st: &[(String, Vec<Handle>)], n: &str| st.iter().position(|(m, _)| m.eq_ignore_ascii_case(n));
    match action.as_str() {
        "list" => return listing(s, &block),
        "new" => {
            let n = name()?;
            if find(&st, &n).is_some() {
                return Err(bad(cmd, format!("state \"{n}\" already exists")));
            }
            let visible = match p.get("handles") {
                None | Some(Value::Null) => st.first().map(|s| s.1.clone()).unwrap_or_default(),
                v => handles(cmd, v)?,
            };
            st.push((n, visible));
        }
        "set" => {
            let n = name()?;
            let i = find(&st, &n).ok_or_else(|| bad(cmd, format!("no state \"{n}\"")))?;
            if let Some(e) = st.get_mut(i) {
                e.1 = handles(cmd, p.get("handles"))?;
            }
        }
        "rename" => {
            let n = name()?;
            let to = str_param(p, "newName").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "`newName` is required"))?;
            let i = find(&st, &n).ok_or_else(|| bad(cmd, format!("no state \"{n}\"")))?;
            if find(&st, to).is_some_and(|j| j != i) {
                return Err(bad(cmd, format!("state \"{to}\" already exists")));
            }
            if let Some(e) = st.get_mut(i) {
                e.0 = to.to_string();
            }
        }
        "delete" => {
            let n = name()?;
            let i = find(&st, &n).ok_or_else(|| bad(cmd, format!("no state \"{n}\"")))?;
            if st.len() == 1 {
                return Err(bad(cmd, "a visibility parameter keeps at least one state"));
            }
            st.remove(i);
        }
        other => return Err(bad(cmd, format!("unknown action \"{other}\" (list, new, set, rename or delete)"))),
    }
    change(s, cmd, |d| author::set_states(d, &block, &st))?;
    listing(s, &block)
}

fn run_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "bremove";
    let block = block_name(s, cmd, p)?;
    let node = element(s, cmd, &block, p.get("element"))?;
    change(s, cmd, |d| author::remove(d, &block, node))?;
    Ok(json!({ "dynamic": dynamic(s, &block)? }))
}
