//! Safety rails for agents driving CADaddy (MCP, the control channel):
//!
//! - **Requests** (`agent.begin` / `agent.end`): everything an agent does for one request is one
//!   undo step, and the change journal records which objects it added, removed and changed.
//! - **Dry run** (`agent.dryrun`): commands report what they would change and then everything is
//!   put back; no file is written meanwhile.
//! - **Confirmation** (`agent.confirm`, on by default): saving over an existing file or closing a
//!   drawing with unsaved changes, asked for by an agent, waits for the user to allow it in the
//!   app ([`AgentGuard::pending`]).

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use cadaddy_doc::{Entity, Handle};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, bad, bool_or, str_param};
use crate::{EngineError, Result, Session};

/// Journal entries kept (the oldest go first), and handles listed per entry and kind.
const MAX_ENTRIES: usize = 1_000;
const MAX_LISTED: usize = 200;

thread_local! {
    /// A dry run is in progress: files must not be written ([`writes_allowed`]).
    static DRY: Cell<bool> = const { Cell::new(false) };
}

/// Whether files may be written now (not during a dry run).
pub fn writes_allowed() -> bool {
    !DRY.with(Cell::get)
}

/// One agent request in the change journal.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub id: u64,
    pub label: String,
    /// The drawing it changed.
    pub drawing: String,
    /// Commands run while it was open.
    pub commands: usize,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    /// Counts (the lists stop at a few hundred).
    pub counts: [usize; 3],
}

struct Request {
    id: u64,
    label: String,
    uid: u64,
    before: HashMap<Handle, Arc<Entity>>,
    commands: usize,
}

/// An agent's call waiting for the user.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pending {
    pub command: String,
    pub params: Value,
    /// What the user is asked, in words.
    pub question: String,
}

/// The rails' state for a session.
pub struct AgentGuard {
    pub dry_run: bool,
    /// Ask before an agent saves over a file or closes with changes.
    pub confirm: bool,
    /// The call being run came from an agent (set by the host around agent calls).
    pub from_agent: bool,
    /// The agent call waiting for the user's answer, and the user's last answer (question,
    /// allowed) for the agent to read (`agent.journal`).
    pub pending: Option<Pending>,
    pub answered: Option<(String, bool)>,
    pub journal: Vec<Entry>,
    request: Option<Request>,
    next_id: u64,
}

impl Default for AgentGuard {
    fn default() -> Self {
        AgentGuard { dry_run: false, confirm: true, from_agent: false, pending: None, answered: None, journal: Vec::new(), request: None, next_id: 1 }
    }
}

impl std::fmt::Debug for AgentGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGuard").field("dry_run", &self.dry_run).field("confirm", &self.confirm).field("journal", &self.journal.len()).finish()
    }
}

impl Clone for AgentGuard {
    /// A copy without the open request (it belongs to the session it was begun in).
    fn clone(&self) -> Self {
        AgentGuard {
            dry_run: self.dry_run,
            confirm: self.confirm,
            from_agent: self.from_agent,
            pending: self.pending.clone(),
            answered: self.answered.clone(),
            journal: self.journal.clone(),
            request: None,
            next_id: self.next_id,
        }
    }
}

fn snapshot(s: &Session) -> HashMap<Handle, Arc<Entity>> {
    s.doc().map(|d| d.model.iter().map(|e| (e.handle, Arc::clone(e))).collect()).unwrap_or_default()
}

/// Added, removed and changed objects between `before` and the drawing now.
fn diff(before: &HashMap<Handle, Arc<Entity>>, s: &Session) -> (Vec<String>, Vec<String>, Vec<String>) {
    let now = snapshot(s);
    let mut added: Vec<Handle> = now.keys().filter(|h| !before.contains_key(h)).copied().collect();
    let mut removed: Vec<Handle> = before.keys().filter(|h| !now.contains_key(h)).copied().collect();
    let mut changed: Vec<Handle> =
        now.iter().filter(|(h, e)| before.get(h).is_some_and(|b| !Arc::ptr_eq(b, e) && b != *e)).map(|(h, _)| *h).collect();
    for v in [&mut added, &mut removed, &mut changed] {
        v.sort_unstable_by_key(|h| h.0);
    }
    let hex = |v: Vec<Handle>| v.into_iter().map(|h| h.hex()).collect::<Vec<_>>();
    (hex(added), hex(removed), hex(changed))
}

fn listed(v: &[String]) -> Vec<String> {
    v.iter().take(MAX_LISTED).cloned().collect()
}

impl Session {
    /// Run `id` for an agent: dry run and confirmation apply ([`AgentGuard`]); requests count it.
    pub fn execute_agent(&mut self, id: &str, params: &Value) -> Result<Value> {
        let was = std::mem::replace(&mut self.agent.from_agent, true);
        let r = self.execute(id, params);
        self.agent.from_agent = was;
        r
    }

    /// Run a batch for an agent ([`Session::execute_many`]): each command under the rails, as
    /// [`Session::execute_agent`] runs one (a save over a file inside a batch waits for the user).
    pub fn execute_many_agent(&mut self, commands: &[Value], stop_on_error: bool) -> Result<Value> {
        let was = std::mem::replace(&mut self.agent.from_agent, true);
        let r = self.execute_many(commands, stop_on_error);
        self.agent.from_agent = was;
        r
    }

    /// The rails around [`Session::execute`]'s outermost call: a dry run, or a confirmation
    /// first. `None`: run it normally.
    pub(crate) fn guard_call(&mut self, id: &str, params: &Value) -> Option<Result<Value>> {
        let cmd = crate::cmd::resolve_alias(id);
        if cmd.starts_with("agent.") {
            return None;
        }
        if let Some(r) = self.agent.request.as_mut()
            && self.agent.from_agent
        {
            r.commands += 1;
        }
        if self.agent.from_agent
            && self.agent.confirm
            // (No way for the agent to say it was confirmed: the user's Allow runs the call as the
            // user's own, outside these rails.)
            && !self.agent.dry_run
            && let Some(question) = self.needs_confirmation(&cmd, params)
        {
            self.agent.pending = Some(Pending { command: cmd.clone(), params: params.clone(), question: question.clone() });
            return Some(Err(EngineError::Other(format!("Waiting for the user: {question} (they answer in CADaddy; agent.confirm turns this off)"))));
        }
        if !self.agent.dry_run {
            return None;
        }
        Some(self.dry_call(&cmd, params))
    }

    /// What the user must allow, if anything: saving over an existing file, closing with changes.
    fn needs_confirmation(&self, cmd: &str, params: &Value) -> Option<String> {
        let st = self.state().ok();
        match cmd {
            "qsave" | "saveas" => {
                let path = str_param(params, "path").map(str::to_string).or_else(|| st.and_then(|s| s.path.clone()))?;
                #[cfg(not(target_arch = "wasm32"))]
                let exists = std::path::Path::new(&path).exists();
                #[cfg(target_arch = "wasm32")]
                let exists = false;
                exists.then(|| format!("an agent wants to save over {path}"))
            }
            "close" => {
                let i = params.get("index").and_then(Value::as_u64).map_or(self.active, |i| i as usize);
                let d = self.docs.get(i)?;
                d.is_dirty().then(|| format!("an agent wants to close {} without saving its changes", d.title))
            }
            "closeall" => {
                self.docs.iter().any(|d| d.is_dirty()).then(|| "an agent wants to close every drawing, some with unsaved changes".to_string())
            }
            _ => None,
        }
    }

    /// Run `cmd` and put everything back: what it would change, and its result.
    fn dry_call(&mut self, cmd: &str, params: &Value) -> Result<Value> {
        let docs = self.docs.clone();
        let active = self.active;
        let settings = self.settings.clone();
        let log = self.log.len();
        let before = snapshot(self);
        let title = self.state().map(|s| s.title.clone()).unwrap_or_default();
        DRY.with(|d| d.set(true));
        let was = std::mem::replace(&mut self.agent.dry_run, false);
        let r = self.execute(cmd, params);
        self.agent.dry_run = was;
        DRY.with(|d| d.set(false));
        let same_drawing = self.state().is_ok_and(|s| s.title == title);
        let (added, removed, changed) = if same_drawing { diff(&before, self) } else { (Vec::new(), Vec::new(), Vec::new()) };
        let opened_or_closed = self.docs.len() != docs.len() || !same_drawing;
        self.docs = docs;
        self.active = active;
        self.settings = settings;
        self.log.truncate(log);
        let result = r?;
        Ok(json!({
            "dryRun": true,
            "result": result,
            "wouldChange": {
                "added": listed(&added), "removed": listed(&removed), "changed": listed(&changed),
                "counts": [added.len(), removed.len(), changed.len()],
                "drawings": opened_or_closed,
            },
        }))
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("agent.begin", "Begin Agent Request", run_begin)
            .params("{label?: what the request is (shown in the journal and as its undo step)} → {request}: what follows, until agent.end, is one undo step and one journal entry")
            .noundo(),
        CommandSpec::new("agent.end", "End Agent Request", run_end).params("{} → the journal entry: {id, label, drawing, commands, added, removed, changed, counts}").noundo(),
        CommandSpec::new("agent.dryrun", "Agent Dry Run", run_dry).params("{on: bool} → {dryRun}: on, commands report what they would change (`wouldChange`) and change nothing; no file is written").noundo(),
        CommandSpec::new("agent.confirm", "Agent Confirmations", run_confirm)
            .params("{on?: bool} → {confirm}: on (the default), an agent saving over a file or closing with changes waits for the user")
            .noundo(),
        CommandSpec::new("agent.journal", "Agent Change Journal", run_journal).params("{limit?, clear?: true} → {entries: [newest first], open?: the request in progress}").noundo(),
    ]
}

fn run_begin(s: &mut Session, p: &Value) -> Result<Value> {
    if s.agent.request.is_some() {
        let _ = run_end(s, &json!({}));
    }
    let label: String = str_param(p, "label").map(str::trim).filter(|l| !l.is_empty()).unwrap_or("Agent request").chars().take(200).collect();
    let id = s.agent.next_id;
    s.agent.next_id += 1;
    let before = snapshot(s);
    let st = s.state_mut()?;
    st.begin_undo_group();
    let uid = st.uid;
    s.agent.request = Some(Request { id, label, uid, before, commands: 0 });
    Ok(json!({ "request": id }))
}

fn run_end(s: &mut Session, _p: &Value) -> Result<Value> {
    let Some(r) = s.agent.request.take() else { return Err(bad("agent.end", "no agent request is open (agent.begin)")) };
    let (added, removed, changed) = if s.state().is_ok_and(|st| st.uid == r.uid) { diff(&r.before, s) } else { (Vec::new(), Vec::new(), Vec::new()) };
    let mut drawing = String::new();
    if let Some(st) = s.docs.iter_mut().find(|d| d.uid == r.uid) {
        st.end_undo_group();
        // The request's undo step says what it was.
        if let Some(last) = st.undo.last_mut()
            && (last.label == "Group" || !added.is_empty() || !removed.is_empty() || !changed.is_empty())
        {
            last.label = format!("Agent: {}", r.label);
        }
        drawing = st.title.clone();
    }
    let entry = Entry {
        id: r.id,
        label: r.label,
        drawing,
        commands: r.commands,
        counts: [added.len(), removed.len(), changed.len()],
        added: listed(&added),
        removed: listed(&removed),
        changed: listed(&changed),
    };
    s.agent.journal.push(entry.clone());
    if s.agent.journal.len() > MAX_ENTRIES {
        s.agent.journal.remove(0);
    }
    Ok(serde_json::to_value(&entry).unwrap_or(Value::Null))
}

fn run_dry(s: &mut Session, p: &Value) -> Result<Value> {
    let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("agent.dryrun", "`on` is true or false"))?;
    s.agent.dry_run = on;
    Ok(json!({ "dryRun": on }))
}

fn run_confirm(s: &mut Session, p: &Value) -> Result<Value> {
    // Turning the question off is the user's choice, not an agent's.
    if let Some(on) = p.get("on").and_then(Value::as_bool) {
        if !on && s.agent.from_agent {
            return Err(EngineError::Other("Only the user can turn agent confirmations off (Options, or AGENT.CONFIRM typed in CADaddy).".into()));
        }
        s.agent.confirm = on;
    }
    Ok(json!({ "confirm": s.agent.confirm }))
}

fn run_journal(s: &mut Session, p: &Value) -> Result<Value> {
    if bool_or(p, "clear", false) {
        s.agent.journal.clear();
    }
    let limit = p.get("limit").and_then(Value::as_u64).map_or(50, |l| l.min(MAX_ENTRIES as u64) as usize);
    let entries: Vec<&Entry> = s.agent.journal.iter().rev().take(limit).collect();
    let mut out = json!({ "entries": entries });
    if let Some(r) = &s.agent.request {
        out["open"] = json!({ "id": r.id, "label": r.label, "commands": r.commands });
    }
    if let Some(p) = &s.agent.pending {
        out["waiting"] = json!({ "question": p.question, "command": p.command });
    }
    if let Some((q, allowed)) = &s.agent.answered {
        out["answered"] = json!({ "question": q, "allowed": allowed });
    }
    Ok(out)
}
