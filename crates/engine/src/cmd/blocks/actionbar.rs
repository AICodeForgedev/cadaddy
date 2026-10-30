//! The Block Editor's action bars and visibility states: BACTIONBAR shows, hides or resets the
//! bar under each parameter that lists the actions following it (BACTIONBARMODE 1 draws them);
//! BVSHOW and BVHIDE make objects visible or hidden in a visibility state; BVSTATE's current
//! state (`action: current`) is the one the editor shows, its hidden objects hidden.

use cadaddy_doc::Handle;
use cadaddy_doc::dynblock::{self, ParameterKind};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use crate::cmd::curves::handle_value;
use crate::cmd::machines::SelectRun;
use crate::cmd::{CommandSpec, bad, bool_or, str_param};
use crate::{Result, Session};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bactionbar", "Action Bars", run_actionbar)
            .params("{action?: list (default) | show | hide | reset, parameters?: [labels or graph ids] (default every parameter)} → {mode, bars: [{node, parameter, element, at: [x,y], shown, actions: [{node, name, type}]}]} (Block Editor)")
            .enabled(editing_a_block)
            .noundo(),
        CommandSpec::new("bvshow", "Make Visible", |s, p| run_visibility(s, p, true))
            .params("{handles?, state? (default the current state), allStates?: bool, block? (default the Block Editor's)} → {state, states: [{name, visible}]}")
            .enabled(editing_a_block)
            .interactive(|_| Ok(Box::new(SelectRun::new("bvshow", "BVSHOW")))),
        CommandSpec::new("bvhide", "Make Invisible", |s, p| run_visibility(s, p, false))
            .params("{handles?, state? (default the current state), allStates?: bool, block? (default the Block Editor's)} → {state, states: [{name, visible}]}")
            .enabled(editing_a_block)
            .interactive(|_| Ok(Box::new(SelectRun::new("bvhide", "BVHIDE")))),
    ]
}

fn editing_a_block(s: &Session) -> std::result::Result<(), String> {
    match s.state() {
        Ok(st) => match &st.block_edit {
            Some(be) if !be.current => Ok(()),
            Some(_) => Err("The Block Editor is open on the current drawing, not on a block.".into()),
            None => Err("The Block Editor is not open (BEDIT).".into()),
        },
        Err(e) => Err(e.to_string()),
    }
}

/// The block the Block Editor is editing, and its base point (the editor's origin).
fn edited(s: &Session, cmd: &str) -> Result<(String, Vec2)> {
    match &s.state()?.block_edit {
        Some(be) if !be.current => Ok((be.name.clone(), be.base())),
        _ => Err(bad(cmd, "the Block Editor is not open on a block (BEDIT)")),
    }
}

/// Where a parameter's action bar goes (block coordinates): under its middle.
fn parameter_at(k: &ParameterKind) -> Option<Vec2> {
    Some(match k {
        ParameterKind::Linear { base, end, .. }
        | ParameterKind::Polar { base, end, .. }
        | ParameterKind::Xy { base, end, .. }
        | ParameterKind::Alignment { base, end, .. }
        | ParameterKind::Flip { base, end, .. } => base.xy().mid(end.xy()),
        ParameterKind::Rotation { base, .. } => base.xy(),
        ParameterKind::Point { at, .. }
        | ParameterKind::Visibility { at, .. }
        | ParameterKind::Lookup { at, .. }
        | ParameterKind::BasePoint { at } => at.xy(),
        ParameterKind::Other { .. } => return None,
    })
}

/// The parameters named in `parameters` (labels, element names or graph ids); all when absent.
fn chosen(m: &dynblock::DynamicBlock, cmd: &str, v: Option<&Value>) -> Result<Vec<i32>> {
    let Some(list) = v.filter(|v| !v.is_null()) else { return Ok(m.parameters.iter().map(|p| p.node).collect()) };
    let items = list.as_array().ok_or_else(|| bad(cmd, "`parameters` must be an array"))?;
    let mut out = Vec::new();
    for it in items.iter().take(10_000) {
        let node = match it {
            Value::Number(n) => n.as_i64().and_then(|n| i32::try_from(n).ok()).filter(|n| m.parameters.iter().any(|p| p.node == *n)),
            Value::String(t) => {
                let t = t.trim();
                m.parameters
                    .iter()
                    .find(|p| {
                        p.kind.label().is_some_and(|l| l.eq_ignore_ascii_case(t))
                            || p.element.eq_ignore_ascii_case(t)
                            || t.parse::<i32>().ok() == Some(p.node)
                    })
                    .map(|p| p.node)
            }
            _ => None,
        };
        out.push(node.ok_or_else(|| bad(cmd, format!("no parameter {it}")))?);
    }
    Ok(out)
}

fn run_actionbar(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "bactionbar";
    let (block, _) = edited(s, cmd)?;
    let m = dynblock::dynamic_block(s.doc()?, &block).unwrap_or_default();
    let action = str_param(p, "action").unwrap_or("list").trim().to_ascii_lowercase();
    if action != "list" {
        let nodes = chosen(&m, cmd, p.get("parameters"))?;
        let st = s.state_mut()?;
        let Some(be) = st.block_edit.as_mut() else { return Err(bad(cmd, "the Block Editor is not open")) };
        match action.as_str() {
            "show" => be.hidden_bars.retain(|n| !nodes.contains(n)),
            "hide" => {
                for n in nodes {
                    if !be.hidden_bars.contains(&n) {
                        be.hidden_bars.push(n);
                    }
                }
            }
            "reset" => be.hidden_bars.clear(),
            other => return Err(bad(cmd, format!("unknown action \"{other}\" (list, show, hide or reset)"))),
        }
        st.revision += 1;
    }
    action_bars(s)
}

/// The Block Editor's action bars: `{mode (BACTIONBARMODE), bars: [{node, parameter, element,
/// at: [x,y] (editor coordinates), shown, actions: [{node, name, type}]}]}` (BACTIONBAR's list).
pub fn action_bars(s: &Session) -> Result<Value> {
    let cmd = "bactionbar";
    let (block, base) = edited(s, cmd)?;
    let m = dynblock::dynamic_block(s.doc()?, &block).unwrap_or_default();
    let hidden = s.state()?.block_edit.as_ref().map(|b| b.hidden_bars.clone()).unwrap_or_default();
    let bars: Vec<Value> = m
        .parameters
        .iter()
        .filter_map(|prm| {
            let at = parameter_at(&prm.kind)? - base;
            let actions: Vec<Value> = m
                .actions
                .iter()
                .filter(|a| a.connections.iter().any(|(n, _)| *n == prm.node))
                .map(|a| {
                    let kind = serde_json::to_value(&a.kind)
                        .ok()
                        .and_then(|k| k.get("type").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_default();
                    json!({ "node": a.node, "name": a.name, "type": kind })
                })
                .collect();
            Some(json!({
                "node": prm.node,
                "parameter": prm.kind.label().unwrap_or(&prm.element),
                "element": prm.element,
                "at": [at.x, at.y],
                "shown": !hidden.contains(&prm.node),
                "actions": actions,
            }))
        })
        .collect();
    let mode = s.doc()?.header.i64("BACTIONBARMODE", 1);
    Ok(json!({ "mode": mode, "bars": bars }))
}

fn handles_param(s: &Session, cmd: &str, p: &Value) -> Result<Vec<Handle>> {
    match p.get("handles") {
        None | Some(Value::Null) => Ok(s.selection()),
        Some(Value::Array(a)) => {
            a.iter().take(100_000).map(|h| handle_value(h).ok_or_else(|| bad(cmd, format!("{h} is not an object handle")))).collect()
        }
        Some(_) => Err(bad(cmd, "`handles` must be an array of handles")),
    }
}

/// The visibility states of `block` (name, visible objects) and the controlled objects.
fn states(s: &Session, cmd: &str, block: &str) -> Result<(Vec<(String, Vec<Handle>)>, Vec<Handle>)> {
    let m = dynblock::dynamic_block(s.doc()?, block);
    m.and_then(|m| {
        m.parameters.into_iter().find_map(|p| match p.kind {
            ParameterKind::Visibility { states, controlled, .. } => Some((states.into_iter().map(|s| (s.name, s.visible)).collect(), controlled)),
            _ => None,
        })
    })
    .ok_or_else(|| bad(cmd, format!("block `{block}` has no visibility parameter (add one with BPARAMETER)")))
}

/// The state the Block Editor shows: the one BVSTATE made current, else the first.
pub(super) fn current_state(s: &Session, list: &[(String, Vec<Handle>)]) -> Option<String> {
    let cur = s.state().ok().and_then(|st| st.block_edit.as_ref().and_then(|b| b.vis_state.clone()));
    cur.filter(|c| list.iter().any(|(n, _)| n.eq_ignore_ascii_case(c))).or_else(|| list.first().map(|(n, _)| n.clone()))
}

fn run_visibility(s: &mut Session, p: &Value, show: bool) -> Result<Value> {
    let cmd = if show { "bvshow" } else { "bvhide" };
    let block = match str_param(p, "block") {
        Some(b) => b.to_string(),
        None => edited(s, cmd)?.0,
    };
    let (mut list, _) = states(s, cmd, &block)?;
    let hs = handles_param(s, cmd, p)?;
    if hs.is_empty() {
        return Err(bad(cmd, "select the objects to make visible or invisible"));
    }
    {
        let b = s.doc()?.block(&block).ok_or_else(|| bad(cmd, format!("no block `{block}`")))?;
        if let Some(h) = hs.iter().find(|h| b.entities.get(**h).is_none()) {
            return Err(bad(cmd, format!("{} is not an object of block `{block}` (save the block first with BSAVE)", h.hex())));
        }
    }
    let state = match str_param(p, "state") {
        Some(n) => list
            .iter()
            .find(|(m, _)| m.eq_ignore_ascii_case(n.trim()))
            .map(|(m, _)| m.clone())
            .ok_or_else(|| bad(cmd, format!("no state \"{n}\"")))?,
        None => current_state(s, &list).ok_or_else(|| bad(cmd, "the visibility parameter has no states"))?,
    };
    let all = bool_or(p, "allStates", false);
    for (name, visible) in list.iter_mut() {
        if !(all || name.eq_ignore_ascii_case(&state)) {
            continue;
        }
        if show {
            for h in &hs {
                if !visible.contains(h) {
                    visible.push(*h);
                }
            }
        } else {
            visible.retain(|h| !hs.contains(h));
        }
    }
    let mut d = s.doc()?.clone();
    cadaddy_doc::dynblock::author::set_states(&mut d, &block, &list).map_err(|e| bad(cmd, e))?;
    *s.doc_mut()? = d;
    show_state(s, &block, &state)?;
    let (list, _) = states(s, cmd, &block)?;
    Ok(json!({
        "state": state,
        "states": list.iter().map(|(n, v)| json!({ "name": n, "visible": v.iter().map(|h| h.hex()).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        "message": format!("{} object(s) {} in {}.", hs.len(), if show { "made visible" } else { "made invisible" }, if all { "every state".to_string() } else { format!("state \"{state}\"") }),
    }))
}

/// In the Block Editor on `block`: hide the objects `state` hides (BVMODE 0), or show them all
/// (BVMODE 1, which this editor shows at full strength); `state` becomes the current one.
pub(super) fn show_state(s: &mut Session, block: &str, state: &str) -> Result<()> {
    let editing = s.state()?.block_edit.as_ref().is_some_and(|b| !b.current && b.name.eq_ignore_ascii_case(block));
    if !editing {
        return Ok(());
    }
    let Ok((list, controlled)) = states(s, "bvstate", block) else { return Ok(()) };
    let visible = list.iter().find(|(n, _)| n.eq_ignore_ascii_case(state)).map(|(_, v)| v.clone()).unwrap_or_default();
    let bvmode = s.doc()?.header.i64("BVMODE", 0);
    let hidden: std::collections::BTreeSet<Handle> =
        if bvmode == 0 { controlled.iter().filter(|h| !visible.contains(h)).copied().collect() } else { Default::default() };
    if let Some(be) = s.state_mut()?.block_edit.as_mut() {
        be.vis_state = Some(state.to_string());
    }
    if s.doc()?.hidden_objects != hidden {
        s.doc_mut()?.hidden_objects = hidden;
    }
    Ok(())
}
