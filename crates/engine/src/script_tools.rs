//! Script tools: what a script's own lines can say ([`Session::script`]) and the commands behind
//! them, and SYSVARMONITOR.
//!
//! - `SCRIPTCALL file`: run another script, then carry on with this one (nested at most
//!   [`MAX_DEPTH`] deep).
//! - `RSCRIPT`: start this script again (in AutoCAD until Esc; here at most [`MAX_REPEATS`]
//!   times per run, since a script can't press Esc). At the command line it runs the last
//!   script again.
//! - `DELAY ms`: wait (at most [`MAX_DELAY_MS`], as AutoCAD).
//! - `RESUME`: carry on with a script a failed line stopped, from the next line.
//! - `SYSVARMONITOR`: system variables to watch; after each command, one that differs from the
//!   value it was watched with is reported, and RESET puts them all back.

use serde_json::{Value, json};

use crate::cmd::{CommandSpec, bad, str_param};
use crate::{Result, Session};

/// Most RSCRIPT repeats in one run of a script.
pub const MAX_REPEATS: usize = 1000;
/// Deepest SCRIPTCALL nesting.
pub const MAX_DEPTH: usize = 8;
/// Longest DELAY, milliseconds (AutoCAD's limit).
pub const MAX_DELAY_MS: u64 = 32_767;
/// Most system variables watched.
pub const MAX_WATCHED: usize = 256;

/// A script line that controls the script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    Repeat,
    Delay(u64),
    Call(String),
    Resume,
}

/// The command a line starts, without AutoCAD's `_` (English name) and `.` (built-in) prefixes.
fn command_word(line: &str) -> (String, &str) {
    let line = line.trim_start();
    let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    (word.trim_start_matches(['_', '.', '-']).to_ascii_uppercase(), rest.trim())
}

/// What `line` asks of the script, if it is one of the script's own commands.
pub fn control(line: &str) -> Option<Control> {
    let (word, rest) = command_word(line);
    match word.as_str() {
        "RSCRIPT" => Some(Control::Repeat),
        "DELAY" => Some(Control::Delay(
            rest.split_whitespace().next().and_then(|n| n.parse::<f64>().ok()).map_or(0, |n| n.clamp(0.0, MAX_DELAY_MS as f64) as u64),
        )),
        "SCRIPTCALL" => Some(Control::Call(rest.trim_matches('"').to_string())),
        "RESUME" => Some(Control::Resume),
        _ => None,
    }
}

/// Wait `ms` milliseconds (capped).
pub fn delay(ms: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(std::time::Duration::from_millis(ms.min(MAX_DELAY_MS)));
    #[cfg(target_arch = "wasm32")]
    let _ = ms;
}

/// The text of the script SCRIPTCALL names (checked as SCRIPT checks a file: SECURELOAD).
pub fn read_called(s: &mut Session, path: &str, depth: usize) -> Result<String> {
    if path.trim().is_empty() {
        return Err(bad("scriptcall", "SCRIPTCALL needs a script file"));
    }
    if depth + 1 > MAX_DEPTH {
        return Err(bad("scriptcall", format!("scripts call each other more than {MAX_DEPTH} deep")));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.check_load(std::path::Path::new(path), "SCRIPT", None)?;
        let bytes = crate::readfile::read(path).map_err(|e| bad("scriptcall", format!("{path}: {e}")))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        Err(bad("scriptcall", "script files are not available on the web"))
    }
}

/// The system variables SYSVARMONITOR watches, with the values they were watched with.
#[derive(Clone, Debug, Default)]
pub struct Monitor {
    pub watched: Vec<(String, Value)>,
    /// Report changes on the command line.
    pub notify: bool,
    /// The changes reported so far (name, was, now), latest last, at most [`MAX_WATCHED`].
    pub changes: Vec<(String, Value, Value)>,
}

/// After each command: report watched variables that moved since the last look.
pub fn monitor_after(s: &mut Session) {
    if s.monitor.watched.is_empty() {
        return;
    }
    let mut lines = Vec::new();
    let mut changes = Vec::new();
    let names: Vec<(String, Value)> = s.monitor.watched.clone();
    let mut last = s.monitor_seen.clone();
    for (name, _) in &names {
        let now = crate::sysvars::get(s, name).unwrap_or(Value::Null);
        let seen = last.get(name).cloned().unwrap_or(Value::Null);
        if now != seen {
            changes.push((name.clone(), seen.clone(), now.clone()));
            lines.push(format!("SYSVARMONITOR: {name} changed from {seen} to {now}."));
            last.insert(name.clone(), now);
        }
    }
    s.monitor_seen = last;
    for c in changes {
        if s.monitor.changes.len() >= MAX_WATCHED {
            s.monitor.changes.remove(0);
        }
        s.monitor.changes.push(c);
    }
    if s.monitor.notify {
        for l in lines {
            s.echo(l);
        }
    }
}

fn run_delay(_s: &mut Session, p: &Value) -> Result<Value> {
    let ms = p.get("ms").and_then(Value::as_f64).filter(|m| m.is_finite() && *m >= 0.0).ok_or_else(|| bad("delay", "`ms`: how many milliseconds"))?;
    let ms = ms.min(MAX_DELAY_MS as f64) as u64;
    delay(ms);
    Ok(json!({ "waited": ms }))
}

fn run_rscript(s: &mut Session, _p: &Value) -> Result<Value> {
    let text = s.script_last.clone().ok_or_else(|| bad("rscript", "no script has run yet"))?;
    s.script(&text)?;
    Ok(json!({ "lines": text.lines().count() }))
}

fn run_scriptcall(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").ok_or_else(|| bad("scriptcall", "`path`: the script file"))?.to_string();
    let text = read_called(s, &path, 0)?;
    s.script(&text)?;
    Ok(json!({ "lines": text.lines().count() }))
}

fn run_resume(s: &mut Session, _p: &Value) -> Result<Value> {
    let resumed = s.script_resume()?;
    if !resumed {
        s.echo("RESUME: no script was stopped.");
    }
    Ok(json!({ "resumed": resumed }))
}

/// SYSVARMONITOR's answer: the watched variables (watched and current values), notify, the
/// changes reported.
pub fn state(s: &Session) -> Value {
    let vars: Vec<Value> = s
        .monitor
        .watched
        .iter()
        .map(|(n, was)| json!({ "name": n, "watched": was, "now": crate::sysvars::get(s, n).unwrap_or(Value::Null) }))
        .collect();
    let changes: Vec<Value> = s.monitor.changes.iter().map(|(n, a, b)| json!({ "name": n, "was": a, "now": b })).collect();
    json!({ "variables": vars, "notify": s.monitor.notify, "changes": changes })
}

fn run_monitor(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sysvarmonitor";
    let names = |k: &str| -> Result<Vec<String>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(a)) => {
                a.iter().map(|n| n.as_str().map(|n| n.trim().to_ascii_uppercase()).ok_or_else(|| bad(cmd, format!("`{k}`: names")))).collect()
            }
            Some(Value::String(n)) => Ok(vec![n.trim().to_ascii_uppercase()]),
            Some(_) => Err(bad(cmd, format!("`{k}`: names"))),
        }
    };
    let (set, add, remove) = (p.get("variables").filter(|v| !v.is_null()).map(|_| names("variables")).transpose()?, names("add")?, names("remove")?);
    let mut list: Vec<String> = match set {
        Some(v) => v,
        None => s.monitor.watched.iter().map(|(n, _)| n.clone()).collect(),
    };
    list.extend(add);
    list.retain(|n| !remove.contains(n));
    let mut seen = std::collections::HashSet::new();
    list.retain(|n| seen.insert(n.clone()));
    if list.len() > MAX_WATCHED {
        return Err(bad(cmd, format!("at most {MAX_WATCHED} variables")));
    }
    for n in &list {
        if crate::sysvars::get(s, n).is_none() {
            return Err(bad(cmd, format!("{n} is not a system variable")));
        }
    }
    if p.get("reset").and_then(Value::as_bool) == Some(true) {
        // Every watched variable back to the value it was watched with.
        let back: Vec<(String, Value)> = s.monitor.watched.clone();
        for (n, v) in back {
            if !crate::sysvars::is_read_only(&n) && crate::sysvars::get(s, &n).as_ref() != Some(&v) {
                crate::sysvars::set(s, &n, &v)?;
            }
        }
        s.monitor_seen = s.monitor.watched.iter().cloned().collect();
        s.monitor.changes.clear();
    }
    // Newly watched variables are watched with their value now; others keep theirs.
    let old: Vec<(String, Value)> = std::mem::take(&mut s.monitor.watched);
    for n in list {
        let v = old.iter().find(|(o, _)| *o == n).map(|(_, v)| v.clone()).unwrap_or_else(|| crate::sysvars::get(s, &n).unwrap_or(Value::Null));
        s.monitor_seen.entry(n.clone()).or_insert_with(|| v.clone());
        s.monitor.watched.push((n, v));
    }
    s.monitor_seen.retain(|n, _| s.monitor.watched.iter().any(|(w, _)| w == n));
    if let Some(b) = p.get("notify").and_then(Value::as_bool) {
        s.monitor.notify = b;
    }
    Ok(state(s))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("delay", "Delay", run_delay).params("{ms: milliseconds (at most 32767)} → {waited}: a pause (in a script: `DELAY 500`)").noundo(),
        CommandSpec::new("rscript", "Repeat Script", run_rscript)
            .params("{} → {lines}: the last script again (in a script: start it again, at most 1000 times)")
            .noundo(),
        CommandSpec::new("scriptcall", "Script Call", run_scriptcall)
            .params("{path: script file} → {lines}: run a script (in a script: run it and carry on, at most 8 deep)")
            .noundo(),
        CommandSpec::new("resume", "Resume Script", run_resume).params("{} → {resumed}: carry on with the script a failed line stopped").noundo(),
        CommandSpec::new("sysvarmonitor", "System Variable Monitor...", run_monitor)
            .menu(&["Tools", "System Variable Monitor..."])
            .params(
                "{variables?: [names] (the whole list), add?: [names], remove?: [names], notify?: bool (report changes on the command line), \
                 reset?: true (every watched variable back to its value when watched)} → {variables: [{name, watched, now}], notify, changes: [{name, was, now}]}",
            )
            .noundo(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_lines_that_control_the_script() {
        assert_eq!(control("RSCRIPT"), Some(Control::Repeat));
        assert_eq!(control("_.rscript "), Some(Control::Repeat));
        assert_eq!(control("DELAY 250"), Some(Control::Delay(250)));
        assert_eq!(control("delay 99999999"), Some(Control::Delay(MAX_DELAY_MS)));
        assert_eq!(control("delay -5"), Some(Control::Delay(0)));
        assert_eq!(control("SCRIPTCALL \"Project A setup.scr\""), Some(Control::Call("Project A setup.scr".into())));
        assert_eq!(control("LINE 0,0 1,1"), None);
        assert_eq!(control("delayed"), None);
    }
}
