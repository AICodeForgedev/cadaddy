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
    ]
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
