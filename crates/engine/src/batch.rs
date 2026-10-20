//! Batch calls for agents (MCP `execute_many`, control `engine.executeMany`): many commands in
//! one request and one undo step, each result returned. With `stopOnError` the first failure
//! stops the batch and puts the drawings back as they were before it.

use serde_json::{Value, json};

use crate::{EngineError, Result, Session};

/// Most commands one batch runs.
pub const MAX_BATCH: usize = 10_000;

impl Session {
    /// Run `commands` (`[{command, params?}]`) in order as one undo step. Returns `{results,
    /// ran}`; a failed command's result is `{error}`, or with `stop_on_error` the batch stops
    /// there, every drawing goes back to how it was before the batch, and the reply says which
    /// command failed and how many were undone.
    pub fn execute_many(&mut self, commands: &[Value], stop_on_error: bool) -> Result<Value> {
        let bad = |m: String| EngineError::BadParams { cmd: "executeMany".into(), msg: m };
        if commands.len() > MAX_BATCH {
            return Err(bad(format!("at most {MAX_BATCH} commands in one batch ({} given)", commands.len())));
        }
        // Each entry checked before anything runs.
        let calls: Vec<(&str, Value)> = commands
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let id = c.get("command").and_then(Value::as_str).ok_or_else(|| bad(format!("command {i}: `command` is required")))?;
                let params = c.get("params").cloned().filter(|p| !p.is_null()).unwrap_or_else(|| json!({}));
                Ok((id, params))
            })
            .collect::<Result<_>>()?;
        let docs = self.docs.clone();
        let active = self.active;
        let uid = self.state().ok().map(|s| s.uid);
        // One undo step (inside an UNDO BEgin group the user opened, theirs is the step).
        let grouped = match self.state_mut() {
            Ok(st) if st.undo_ctl.group.is_none() => {
                st.begin_undo_group();
                true
            }
            _ => false,
        };
        // Its commands are one event for a watcher (told below), not thousands.
        let was_batch = std::mem::replace(&mut self.events.batch, true);
        let mut results = Vec::with_capacity(calls.len());
        let mut failed: Option<(usize, String, String)> = None;
        for (i, (id, params)) in calls.iter().enumerate() {
            match self.execute(id, params) {
                Ok(v) => results.push(v),
                Err(e) if stop_on_error => {
                    failed = Some((i, (*id).to_string(), e.to_string()));
                    break;
                }
                Err(e) => results.push(json!({ "error": e.to_string() })),
            }
        }
        self.events.batch = was_batch;
        if !was_batch {
            let ok = failed.is_none();
            self.event("command.run", json!({ "command": "executeMany", "ok": ok, "count": results.len() }));
        }
        if let Some((index, command, message)) = failed {
            // Everything as it was before the batch (its undo history too).
            let undone = results.len();
            self.docs = docs;
            self.active = active.min(self.docs.len().saturating_sub(1));
            self.echo(format!("Batch stopped at command {} ({command}): {message}. {undone} commands undone.", index + 1));
            return Ok(
                json!({ "results": results, "ran": undone, "error": { "index": index, "command": command, "message": message }, "undone": undone }),
            );
        }
        let n = results.len();
        if grouped && let Some(st) = self.docs.iter_mut().find(|d| Some(d.uid) == uid) {
            let depth = st.undo.len();
            st.end_undo_group();
            // Name the step when the batch changed the drawing.
            if st.undo.len() <= depth
                && let Some(last) = st.undo.last_mut()
                && last.label == "Group"
            {
                last.label = format!("Batch ({n} commands)");
            }
        }
        Ok(json!({ "results": results, "ran": n }))
    }
}
