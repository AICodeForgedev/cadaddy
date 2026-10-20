//! AutoLISP commands: APPLOAD loads a `.lsp` file; `lisp` evaluates LISP for programmatic
//! callers (scripts, the control channel, MCP, the CLI).

use serde_json::{Value, json};

use super::{bad, str_param};
use crate::lisp::dialog::Event;
use crate::lisp::value::{Value as Lv, to_text};
use crate::{Accept, CommandSpec, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("appload", "Load Application...", run_appload)
            .menu(&["Tools", "Load Application..."])
            .alias(&["ap"])
            .params("{path: .lsp file} | {name, data: base64 | text} → {value?, error?}")
            .noundo()
            .interactive(|_| Ok(Box::new(ApploadM))),
        CommandSpec::new("lisp", "Evaluate LISP", run_lisp)
            .params(
                "{code: LISP source, inputs?: [answers to get… calls: numbers, strings, [x,y] points], dialog?: [tile actions for start_dialog, in turn: {key, value?, reason?}]} → {value (printed as prin1), output: lines printed, error?}",
            )
            .noundo(),
        CommandSpec::new("lisp.dialog", "Dialog State", run_dcl_state).params("{} → {dialog, tile: {type, key?, value?, items?, disabled?, tiles?, …attributes}} | {dialog: null}").noundo(),
        CommandSpec::new("lisp.tile", "Dialog Action", run_dcl_action)
            .params("{key, value?, reason?: 1} | {cancel: true} — act on a tile of the dialog a LISP routine waits in → {dialog: state after, or null when it closed}")
            .noundo(),
    ]
}

/// The text of a LISP file from `{path}` or `{name, data | text}`, and its name. A file by path
/// loads only from a trusted folder or when allowed (SECURELOAD, [`crate::secureload`]).
fn source(s: &mut Session, p: &Value) -> Result<(String, String)> {
    if let Some(text) = str_param(p, "text") {
        return Ok((str_param(p, "name").unwrap_or("untitled.lsp").to_string(), text.to_string()));
    }
    if let Some(data) = str_param(p, "data") {
        let bytes = super::file::base64_decode(data).ok_or_else(|| bad("appload", "invalid base64"))?;
        return Ok((str_param(p, "name").unwrap_or("untitled.lsp").to_string(), String::from_utf8_lossy(&bytes).into_owned()));
    }
    let path = str_param(p, "path").ok_or_else(|| bad("appload", "`path`, or `name` with `data` (base64) or `text`, is required"))?;
    s.check_load(std::path::Path::new(path), "APPLOAD", Some(("appload".into(), json!({ "path": path }))))?;
    let text = crate::lisp::builtins::read_text_file(path).ok_or_else(|| bad("appload", format!("{path}: can't read the file")))?;
    Ok((path.to_string(), text))
}

/// The result of a blocking LISP run as JSON, with the lines it printed.
fn report(s: &Session, from_log: usize, r: Option<std::result::Result<Lv, String>>) -> Value {
    let output: Vec<String> = s.log.get(from_log..).map(|l| l.to_vec()).unwrap_or_default();
    match r {
        Some(Ok(v)) => json!({ "value": to_text(&v, true), "output": output }),
        Some(Err(e)) => json!({ "error": e, "output": output }),
        None => json!({ "output": output }),
    }
}

fn run_appload(s: &mut Session, p: &Value) -> Result<Value> {
    let (name, text) = source(s, p)?;
    let from = s.log.len();
    let r = s.lisp_load(&name, &text, true)?;
    Ok(report(s, from, r))
}

/// A JSON input as a LISP value: numbers, strings, `[x, y(, z)]` points, null for Enter.
fn input_value(v: &Value) -> Lv {
    match v {
        Value::Array(a) if a.len() >= 2 && a.iter().all(Value::is_number) => Lv::point(&a.iter().filter_map(Value::as_f64).collect::<Vec<_>>()),
        other => crate::lisp::drawing_from_json(other),
    }
}

/// A tile action from JSON: `{key, value?, reason?}`.
fn dialog_event(cmd: &str, v: &Value) -> Result<Event> {
    let key = str_param(v, "key").ok_or_else(|| bad(cmd, "each tile action needs a `key`"))?;
    let value = match v.get("value") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
    };
    let reason = v.get("reason").and_then(Value::as_i64).and_then(|r| i32::try_from(r).ok()).unwrap_or(1);
    Ok(Event { key: key.to_string(), value, reason })
}

fn run_lisp(s: &mut Session, p: &Value) -> Result<Value> {
    let code = str_param(p, "code").ok_or_else(|| bad("lisp", "`code` is required"))?;
    let inputs = p.get("inputs").and_then(Value::as_array).map(|a| a.iter().map(input_value).collect()).unwrap_or_default();
    let dialog = match p.get("dialog") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) if a.len() <= MAX_SCRIPT => a.iter().map(|v| dialog_event("lisp", v)).collect::<Result<_>>()?,
        Some(Value::Array(_)) => return Err(bad("lisp", format!("`dialog` holds at most {MAX_SCRIPT} tile actions"))),
        Some(_) => return Err(bad("lisp", "`dialog` must be an array of {key, value?}")),
    };
    let from = s.log.len();
    let r = s.lisp_eval_scripted(code, inputs, dialog).map_err(|e| match e {
        EngineError::Other(m) => bad("lisp", m),
        e => e,
    })?;
    Ok(report(s, from, r))
}

/// Tile actions one `lisp` call may script, at most.
const MAX_SCRIPT: usize = 10_000;

fn dialog_state(s: &Session) -> Value {
    match s.lisp_dialog() {
        Some(d) => crate::lisp::dialog::to_json(d),
        None => json!({ "dialog": null }),
    }
}

fn run_dcl_state(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(dialog_state(s))
}

fn run_dcl_action(s: &mut Session, p: &Value) -> Result<Value> {
    if s.lisp_dialog().is_none() {
        return Err(bad("lisp.tile", "no LISP dialog is waiting"));
    }
    if p.get("cancel").and_then(Value::as_bool) == Some(true) {
        s.dcl_cancel();
    } else {
        s.dcl_event(dialog_event("lisp.tile", p)?).map_err(|m| bad("lisp.tile", m))?;
    }
    Ok(dialog_state(s))
}

/// APPLOAD typed or chosen from the menu: asks for the file by name.
struct ApploadM;

impl Interactive for ApploadM {
    fn name(&self) -> &'static str {
        "APPLOAD"
    }
    fn prompt(&self, _: &Session) -> Prompt {
        Prompt::new("Enter name of LISP file to load", Accept::TEXT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel | Input::Enter) {
            return Ok(Step::Cancel);
        }
        let Input::Text(t) = i else { return Ok(Step::Continue) };
        let path = t.trim().trim_matches('"').to_string();
        if path.is_empty() {
            return Ok(Step::Cancel);
        }
        match source(s, &json!({ "path": path })) {
            Ok((name, text)) => {
                // The routine may wait for the user: it carries on after this command ends.
                if let Err(e) = s.lisp_load(&name, &text, false) {
                    s.echo(e.to_string());
                }
                Ok(Step::Done)
            }
            Err(EngineError::BadParams { msg, .. } | EngineError::Other(msg)) => {
                s.echo(msg);
                Ok(Step::Done)
            }
            Err(e) => Err(e),
        }
    }
}
