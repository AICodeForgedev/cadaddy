//! UNDOHISTORY and COMMANDHISTORY, behind the Undo History and Command History palettes.
//!
//! UNDOHISTORY lists every undo step by label (the steps that can be redone after them), with
//! the UNDO marks and the open BEgin group, and goes to any step: the steps after it are undone,
//! or redone up to it, exactly as repeated U or REDO would. COMMANDHISTORY lists the session as a
//! script ([`crate::journal`]), saves it as a `.scr` file that SCRIPT replays, or clears it.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, str_param};
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("undohistory", "Undo History", run_undo)
            .menu(&["Edit", "Undo History"])
            .params("{action?: list (default) | to, step: n (to: the number of steps left done, 0 = everything undone)} → {steps: [{step, label, done}], current, marks, group?}")
            .noundo()
            .enabled(always),
        CommandSpec::new("commandhistory", "Command History", run_commands)
            .params("{action?: list (default) | save (path) | clear, filter?: text} → {lines} | {path, lines}")
            .noundo()
            .enabled(always),
        CommandSpec::new("activity", "Activity History", run_activity)
            .menu(&["Tools", "Activity History"])
            .params("{action?: list (default) | show (select the objects step `seq` added or changed that still exist) | clear, command?: filter (any case), limit? (default 500), offset? (newest first)} → {enabled, count, entries: [{seq, time, command, added, modified, erased, blocks}]} | {seq, handles, missing}")
            .noundo(),
    ]
}

/// ACTIVITY: the drawing's activity log (newest first), one step's objects selected, or cleared.
fn run_activity(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "activity";
    let num = |k: &str, d: usize| p.get(k).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).unwrap_or(d);
    match str_param(p, "action").unwrap_or("list") {
        "list" => {
            let filter = str_param(p, "command").map(str::to_lowercase);
            let st = s.state()?;
            let hits: Vec<&crate::activity::Entry> =
                st.activity.entries.iter().rev().filter(|e| filter.as_ref().is_none_or(|f| e.command.to_lowercase().contains(f.as_str()))).collect();
            let page: Vec<Value> = hits
                .iter()
                .skip(num("offset", 0))
                .take(num("limit", 500).min(10_000))
                .map(|e| json!({ "seq": e.seq, "time": e.time, "command": e.command, "added": e.added, "modified": e.modified, "erased": e.erased, "blocks": e.blocks, "objects": e.handles.len() }))
                .collect();
            Ok(json!({ "enabled": crate::activity::enabled(), "count": hits.len(), "entries": page }))
        }
        "show" => {
            let seq = p.get("seq").and_then(Value::as_u64).ok_or_else(|| bad(CMD, "`seq` (a step's number) is required"))?;
            let st = s.state()?;
            let e = st.activity.entries.iter().find(|e| e.seq == seq).ok_or_else(|| bad(CMD, format!("no step {seq} in the activity log")))?;
            let d = st.drawing();
            let (live, gone): (Vec<cadaddy_doc::Handle>, Vec<String>) = e.handles.iter().fold((Vec::new(), Vec::new()), |(mut l, mut g), h| {
                match cadaddy_doc::Handle::parse_hex(h).filter(|h| d.entity(*h).is_some()) {
                    Some(h) => l.push(h),
                    None => g.push(h.clone()),
                }
                (l, g)
            });
            let handles: Vec<String> = live.iter().map(|h| h.hex()).collect();
            s.set_selection(live);
            Ok(json!({ "seq": seq, "handles": handles, "missing": gone.len() }))
        }
        "clear" => {
            s.state_mut()?.activity.entries.clear();
            Ok(json!({ "message": "Activity log cleared." }))
        }
        other => Err(bad(CMD, format!("unknown action `{other}` (list, show, clear)"))),
    }
}

fn listing(s: &Session) -> Result<Value> {
    let st = s.state()?;
    let mut steps: Vec<Value> = st.undo.iter().enumerate().map(|(i, u)| json!({ "step": i + 1, "label": u.label, "done": true })).collect();
    // The redo stack holds the most recently undone step last.
    let done = st.undo.len();
    steps.extend(st.redo.iter().rev().enumerate().map(|(i, u)| json!({ "step": done + i + 1, "label": u.label, "done": false })));
    Ok(json!({ "steps": steps, "current": done, "marks": st.undo_ctl.marks, "group": st.undo_ctl.group }))
}

fn run_undo(s: &mut Session, p: &Value) -> Result<Value> {
    match str_param(p, "action").unwrap_or("list").to_ascii_lowercase().as_str() {
        "list" => listing(s),
        "to" => {
            let target = p.get("step").and_then(Value::as_u64).ok_or_else(|| bad("undohistory", "`step` is required"))?;
            let (done, total) = {
                let st = s.state()?;
                (st.undo.len(), st.undo.len() + st.redo.len())
            };
            let target = usize::try_from(target).unwrap_or(usize::MAX);
            if target > total {
                return Err(bad("undohistory", format!("there are {total} steps")));
            }
            // A BEgin group still open is one step from here on.
            s.state_mut()?.end_undo_group();
            if target < done {
                s.execute("undo", &json!({ "count": done - target }))?;
            } else if target > done {
                s.execute("redo", &json!({ "count": target - done }))?;
            }
            listing(s)
        }
        other => Err(bad("undohistory", format!("unknown action \"{other}\" (list or to)"))),
    }
}

fn run_commands(s: &mut Session, p: &Value) -> Result<Value> {
    let filter = str_param(p, "filter").map(str::to_lowercase).filter(|f| !f.is_empty());
    let lines = |s: &Session| -> Vec<String> {
        s.journal.lines().iter().filter(|l| filter.as_ref().is_none_or(|f| l.to_lowercase().contains(f))).cloned().collect()
    };
    match str_param(p, "action").unwrap_or("list").to_ascii_lowercase().as_str() {
        "list" => Ok(json!({ "lines": lines(s) })),
        "clear" => {
            s.journal.clear();
            Ok(json!({ "lines": [] }))
        }
        "save" => {
            let path = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad("commandhistory", "`path` is required"))?;
            let text = s.journal.script();
            super::file::store(path, text.as_bytes())?;
            Ok(json!({ "path": path, "lines": s.journal.lines().len() }))
        }
        other => Err(bad("commandhistory", format!("unknown action \"{other}\" (list, save or clear)"))),
    }
}
