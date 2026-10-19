//! The Action Recorder: ACTRECORD … ACTSTOP records what is done at the command line (the session
//! journal's lines: commands, points, typed values, `cmd {json}` calls) as an action macro, our
//! own JSON file (`.cdyact`; not Autodesk's .actm). While recording, ACTUSERINPUT makes the next
//! input one to ask for when the macro plays, ACTUSERMESSAGE adds a message, and ACTBASEPOINT a
//! base point the points after it are relative to (asked for at playback). A macro plays by
//! typing its name or with ACTPLAY: the inputs marked "ask" are prompted for (Enter keeps the
//! recorded value), then the steps run as a script. Macros live in the session and, when the app
//! names one, in the actions folder (`<settings>/actions`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use cadaddy_geom::Vec3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, EngineError, Input, Prompt, Result, Session, Step};

/// The file extension of action macros.
pub const EXTENSION: &str = "cdyact";
/// What a macro file says it is.
const FORMAT: &str = "cadaddy-action-macro";
/// Most steps one macro holds.
const MAX_STEPS: usize = 100_000;
/// Most macros listed from the actions folder.
const MAX_MACROS: usize = 10_000;

/// One step of a macro.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacroStep {
    /// A command-line line: a command name, an input (`*x,y`, a value, a keyword, "" for Enter)
    /// or `cmd {json}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// Ask for this input when playing (the recorded one is the default).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask: bool,
    /// What the prompt said when it was recorded (shown when asking).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// A message shown when playback reaches it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// A base point: absolute points after it move with it (asked for when playing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basepoint: Option<[f64; 3]>,
}

/// An action macro.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Macro {
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub steps: Vec<MacroStep>,
}

impl Macro {
    /// The steps asked for when playing, in order (with their indexes).
    pub fn asks(&self) -> Vec<(usize, &MacroStep)> {
        self.steps.iter().enumerate().filter(|(_, s)| s.ask || s.basepoint.is_some()).collect()
    }
}

/// A recording in progress.
#[derive(Clone, Debug, Default)]
pub struct Recording {
    /// Where in the journal it began.
    start: usize,
    /// Journal lengths at each ACTUSERINPUT: the first input after each is asked for (with the
    /// prompt then showing).
    asks: Vec<(usize, String)>,
    /// Steps inserted before journal line `at` (messages, base points).
    inserted: Vec<(usize, MacroStep)>,
}

/// The session's action macros and recording.
#[derive(Clone, Debug, Default)]
pub struct Actions {
    pub recording: Option<Recording>,
    /// Macros of this session, by name in lower case.
    pub macros: BTreeMap<String, Macro>,
    /// Where macros are saved and found (`None`: the session only).
    pub dir: Option<PathBuf>,
}

/// The recorder's own commands, left out of what it records.
fn is_recorder_line(line: &str) -> bool {
    let first = line.trim_start_matches(['\'', '_', '.']).split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    matches!(first.as_str(), "actrecord" | "actstop" | "actuserinput" | "actusermessage" | "actbasepoint" | "actmanager")
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && find_command(name).is_none()
}

impl Session {
    /// The folder action macros are saved in and read from (`<settings>/actions`; set by the app).
    pub fn set_action_dir(&mut self, dir: Option<PathBuf>) {
        self.actions.dir = dir;
    }

    /// The macro called `name` (any case): this session's, else the actions folder's.
    pub fn action_macro(&self, name: &str) -> Option<Macro> {
        let key = name.trim().to_ascii_lowercase();
        if let Some(m) = self.actions.macros.get(&key) {
            return Some(m.clone());
        }
        let dir = self.actions.dir.as_ref()?;
        read_macro(&dir.join(format!("{key}.{EXTENSION}"))).ok()
    }
}

/// A macro file read and checked.
fn read_macro(path: &std::path::Path) -> std::result::Result<Macro, String> {
    let bytes = crate::readfile::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let m: Macro = serde_json::from_slice(&bytes).map_err(|e| format!("{}: not an action macro ({e})", path.display()))?;
    if m.format != FORMAT {
        return Err(format!("{}: not a CADaddy action macro", path.display()));
    }
    if m.steps.len() > MAX_STEPS {
        return Err(format!("{}: more than {MAX_STEPS} steps", path.display()));
    }
    Ok(m)
}

fn macro_json(m: &Macro) -> Value {
    json!({
        "name": m.name,
        "description": m.description,
        "steps": m.steps,
        "asks": m.asks().len(),
    })
}

fn run_record(s: &mut Session, _p: &Value) -> Result<Value> {
    if s.actions.recording.is_some() {
        return Err(EngineError::Other("ACTRECORD: already recording; ACTSTOP ends it".into()));
    }
    let start = s.journal.lines().len();
    s.actions.recording = Some(Recording { start, ..Recording::default() });
    s.echo("Recording an action macro. ACTSTOP saves it; ACTUSERINPUT asks for the next input when it plays.");
    Ok(json!({ "recording": true }))
}

/// The recorded lines and marks as macro steps.
fn steps_of(s: &Session, rec: &Recording) -> Vec<MacroStep> {
    let lines = s.journal.lines();
    let start = rec.start.min(lines.len());
    let mut steps = Vec::new();
    let mut asks = rec.asks.clone();
    for (i, line) in lines.iter().enumerate().skip(start) {
        for (_, st) in rec.inserted.iter().filter(|(at, _)| *at == i) {
            steps.push(st.clone());
        }
        if is_recorder_line(line) {
            continue;
        }
        // The first input at or after an ACTUSERINPUT is asked for (not a command name: a line
        // that starts a command is never marked, inputs follow it).
        let mut step = MacroStep { input: Some(line.clone()), ask: false, prompt: String::new(), message: None, basepoint: None };
        let starts_command = !line.contains(' ') && find_command(&resolve_alias(line.trim_start_matches(['_', '.', '\'', '-']))).is_some();
        if let Some(k) = asks.iter().position(|(at, _)| *at <= i).filter(|_| !starts_command && !line.is_empty()) {
            let (_, prompt) = asks.remove(k);
            step.ask = true;
            step.prompt = prompt;
        }
        steps.push(step);
        if steps.len() >= MAX_STEPS {
            break;
        }
    }
    for (at, st) in &rec.inserted {
        if *at >= lines.len() {
            steps.push(st.clone());
        }
    }
    steps
}

fn run_stop(s: &mut Session, p: &Value) -> Result<Value> {
    let rec = s.actions.recording.take().ok_or_else(|| EngineError::Other("ACTSTOP: nothing is being recorded (ACTRECORD starts)".into()))?;
    let steps = steps_of(s, &rec);
    if steps.iter().all(|st| st.input.is_none()) {
        return Err(EngineError::Other("ACTSTOP: nothing was recorded, so no macro was made".into()));
    }
    let name = match str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => (1..).map(|i| format!("ActMacro{i}")).find(|n| s.action_macro(n).is_none()).unwrap_or_else(|| "ActMacro".into()),
    };
    if !valid_name(&name) {
        // Put the recording back: a better name can still save it.
        s.actions.recording = Some(rec);
        return Err(bad("actstop", format!("\"{name}\" can't name a macro: letters, digits, - and _ only, and not a command's name")));
    }
    let m = Macro {
        format: FORMAT.into(),
        version: 1,
        name: name.clone(),
        description: str_param(p, "description").unwrap_or_default().to_string(),
        steps,
    };
    let mut r = macro_json(&m);
    let path = match str_param(p, "path") {
        Some(path) => Some(PathBuf::from(path)),
        None => s.actions.dir.as_ref().map(|d| d.join(format!("{}.{EXTENSION}", name.to_ascii_lowercase()))),
    };
    if let Some(path) = path {
        let text = serde_json::to_string_pretty(&m).unwrap_or_default();
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir);
        }
        crate::writefile::write(&path, text).map_err(|e| EngineError::Other(format!("ACTSTOP: {}: {e}", path.display())))?;
        r["path"] = json!(path.display().to_string());
    }
    s.echo(format!("Action macro {name} saved ({} steps). Type {} to play it.", m.steps.len(), name.to_ascii_uppercase()));
    s.actions.macros.insert(name.to_ascii_lowercase(), m);
    Ok(r)
}

fn recording<'a>(s: &'a mut Session, cmd: &str) -> Result<&'a mut Recording> {
    s.actions.recording.as_mut().ok_or_else(|| EngineError::Other(format!("{cmd}: only while recording (ACTRECORD starts)")))
}

fn run_user_input(s: &mut Session, _p: &Value) -> Result<Value> {
    let prompt = if s.running.is_some() { s.prompt_text() } else { String::new() };
    let at = s.journal.lines().len();
    recording(s, "ACTUSERINPUT")?.asks.push((at, prompt));
    s.echo("The next input will be asked for when the macro plays.");
    Ok(json!({ "ask": true }))
}

fn run_user_message(s: &mut Session, p: &Value) -> Result<Value> {
    let text = str_param(p, "text").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("actusermessage", "`text` is required"))?;
    let at = s.journal.lines().len();
    let text: String = text.chars().take(2000).collect();
    recording(s, "ACTUSERMESSAGE")?
        .inserted
        .push((at, MacroStep { input: None, ask: false, prompt: String::new(), message: Some(text), basepoint: None }));
    Ok(json!({ "message": true }))
}

fn run_base_point(s: &mut Session, p: &Value) -> Result<Value> {
    let pt = p.get("point").and_then(|v| {
        let a = v.as_array()?;
        let c: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
        (c.len() >= 2 && c.iter().all(|x| x.is_finite())).then(|| [c[0], c[1], c.get(2).copied().unwrap_or(0.0)])
    });
    let pt = pt.ok_or_else(|| bad("actbasepoint", "`point` [x, y(, z)] is required"))?;
    let at = s.journal.lines().len();
    recording(s, "ACTBASEPOINT")?
        .inserted
        .push((at, MacroStep { input: None, ask: true, prompt: "Specify base point".into(), message: None, basepoint: Some(pt) }));
    Ok(json!({ "basepoint": pt }))
}

/// An absolute point line (`*x,y`, `x,y,z` or `x,y`), as numbers.
fn point_line(line: &str) -> Option<(bool, Vec<f64>)> {
    let star = line.starts_with('*');
    let c: Option<Vec<f64>> = line.trim_start_matches('*').split(',').map(|t| t.trim().parse::<f64>().ok().filter(|v| v.is_finite())).collect();
    c.filter(|c| (2..=3).contains(&c.len())).map(|c| (star, c))
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.9}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

/// The value given for an asked step as a command-line line: a point `[x, y(, z)]` (world, as
/// recorded points are) or text.
fn given_line(v: &Value) -> Option<String> {
    match v {
        Value::Array(a) => {
            let c: Vec<f64> = a.iter().filter_map(Value::as_f64).filter(|x| x.is_finite()).collect();
            match c.len() {
                2 => Some(format!("*{},{}", fmt(c[0]), fmt(c[1]))),
                3 => Some(format!("{},{},{}", fmt(c[0]), fmt(c[1]), fmt(c[2]))),
                _ => None,
            }
        }
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Null => None,
        _ => None,
    }
}

/// Play macro `m` with `inputs` for its asked steps (in order; a missing or null one keeps the
/// recorded value). Messages are echoed as they're reached.
fn play(s: &mut Session, m: &Macro, inputs: &[Value]) -> Result<Value> {
    let mut given = inputs.iter();
    let mut base_shift = [0.0f64; 3];
    let mut chunk: Vec<String> = Vec::new();
    let mut ran = 0usize;
    let flush = |s: &mut Session, chunk: &mut Vec<String>| -> Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        let text = chunk.join("\n") + "\n";
        chunk.clear();
        s.script(&text)
    };
    for st in &m.steps {
        if let Some(text) = &st.message {
            flush(s, &mut chunk)?;
            s.echo(text.clone());
            continue;
        }
        if let Some(bp) = st.basepoint {
            let new = given
                .next()
                .and_then(given_line)
                .and_then(|l| point_line(&l))
                .map(|(_, c)| [c[0], c[1], c.get(2).copied().unwrap_or(bp[2])])
                .unwrap_or(bp);
            base_shift = [new[0] - bp[0], new[1] - bp[1], new[2] - bp[2]];
            continue;
        }
        let Some(recorded) = &st.input else { continue };
        let line = if st.ask { given.next().and_then(given_line).unwrap_or_else(|| recorded.clone()) } else { recorded.clone() };
        // Absolute points move with the base point.
        let line = match point_line(&line) {
            Some((star, c)) if base_shift != [0.0; 3] && (star || c.len() == 3) => {
                let mut out: Vec<String> = c.iter().enumerate().map(|(k, v)| fmt(v + base_shift[k])).collect();
                if c.len() == 2 {
                    out[0] = format!("*{}", out[0]);
                }
                out.join(",")
            }
            _ => line,
        };
        chunk.push(line);
        ran += 1;
    }
    flush(s, &mut chunk)?;
    Ok(json!({ "macro": m.name, "steps": ran }))
}

fn run_play(s: &mut Session, p: &Value) -> Result<Value> {
    let m = match (str_param(p, "name"), str_param(p, "path")) {
        (_, Some(path)) => read_macro(std::path::Path::new(path)).map_err(|e| bad("actplay", e))?,
        (Some(name), None) => s.action_macro(name).ok_or_else(|| bad("actplay", format!("there is no action macro called \"{name}\"")))?,
        (None, None) => return Err(bad("actplay", "`name` or `path` is required")),
    };
    let inputs = p.get("inputs").and_then(Value::as_array).cloned().unwrap_or_default();
    play(s, &m, &inputs)
}

/// Every macro: this session's and the actions folder's.
fn all_macros(s: &Session) -> Vec<Macro> {
    let mut by_name: BTreeMap<String, Macro> = BTreeMap::new();
    if let Some(dir) = &s.actions.dir
        && let Ok(rd) = std::fs::read_dir(dir)
    {
        for e in rd.flatten().take(MAX_MACROS) {
            let path = e.path();
            if path.extension().is_some_and(|x| x.eq_ignore_ascii_case(EXTENSION))
                && let Ok(m) = read_macro(&path)
            {
                by_name.insert(m.name.to_ascii_lowercase(), m);
            }
        }
    }
    for (k, m) in &s.actions.macros {
        by_name.insert(k.clone(), m.clone());
    }
    by_name.into_values().collect()
}

/// `actions {action?: list|delete|ask|rename, name, step?, ask?, to?}`: the Action Macro Manager's work.
fn run_manage(s: &mut Session, p: &Value) -> Result<Value> {
    let action = str_param(p, "action").unwrap_or("list");
    if action == "list" {
        let list: Vec<Value> = all_macros(s).iter().map(macro_json).collect();
        return Ok(json!({ "macros": list, "recording": s.actions.recording.is_some() }));
    }
    let name = str_param(p, "name").ok_or_else(|| bad("actions", "`name` is required"))?;
    let mut m = s.action_macro(name).ok_or_else(|| bad("actions", format!("there is no action macro called \"{name}\"")))?;
    let file = s.actions.dir.as_ref().map(|d| d.join(format!("{}.{EXTENSION}", m.name.to_ascii_lowercase())));
    match action {
        "delete" => {
            s.actions.macros.remove(&m.name.to_ascii_lowercase());
            if let Some(f) = file.filter(|f| f.is_file()) {
                std::fs::remove_file(&f).map_err(|e| EngineError::Other(format!("{}: {e}", f.display())))?;
            }
            return Ok(json!({ "deleted": m.name }));
        }
        "ask" => {
            let i = p
                .get("step")
                .and_then(Value::as_u64)
                .and_then(|i| usize::try_from(i).ok())
                .ok_or_else(|| bad("actions", "`step` (its index) is required"))?;
            let n = m.steps.len();
            let st = m.steps.get_mut(i).ok_or_else(|| bad("actions", format!("the macro has {n} steps")))?;
            if st.input.is_none() {
                return Err(bad("actions", "only an input step can be asked for"));
            }
            st.ask = bool_or(p, "ask", true);
        }
        "rename" => {
            let to = str_param(p, "to")
                .map(str::trim)
                .filter(|t| valid_name(t))
                .ok_or_else(|| bad("actions", "`to`: a new name (letters, digits, - and _)"))?;
            s.actions.macros.remove(&m.name.to_ascii_lowercase());
            if let Some(f) = file.filter(|f| f.is_file()) {
                let _ = std::fs::remove_file(f);
            }
            m.name = to.to_string();
        }
        other => return Err(bad("actions", format!("unknown action \"{other}\" (list, delete, ask or rename)"))),
    }
    if let Some(dir) = &s.actions.dir {
        let path = dir.join(format!("{}.{EXTENSION}", m.name.to_ascii_lowercase()));
        crate::writefile::write(&path, serde_json::to_string_pretty(&m).unwrap_or_default())
            .map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    }
    let r = macro_json(&m);
    s.actions.macros.insert(m.name.to_ascii_lowercase(), m);
    Ok(r)
}

/// Prompts for the steps asked for, then plays (a macro name typed, or ACTPLAY).
struct PlayM {
    m: Option<Macro>,
    /// Index into the macro's asked steps.
    next: usize,
    given: Vec<Value>,
}

impl PlayM {
    fn finish(&mut self, s: &mut Session) -> Result<Step> {
        let Some(m) = self.m.take() else { return Ok(Step::Cancel) };
        match play(s, &m, &self.given) {
            Ok(_) => Ok(Step::Done),
            Err(e) => {
                s.echo(e.user_message());
                Ok(Step::Done)
            }
        }
    }
}

impl crate::prompt::Interactive for PlayM {
    fn name(&self) -> &'static str {
        "ACTPLAY"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        let Some(m) = &self.m else { return Prompt::new("Enter action macro name", Accept::TEXT) };
        let asks = m.asks();
        match asks.get(self.next) {
            Some((_, st)) => {
                let what = if st.basepoint.is_some() {
                    "Specify base point".to_string()
                } else if st.prompt.is_empty() {
                    "Enter a value".into()
                } else {
                    st.prompt.clone()
                };
                let shown = st
                    .basepoint
                    .map(|b| format!("{},{}", fmt(b[0]), fmt(b[1])))
                    .or_else(|| st.input.clone().map(|i| i.trim_start_matches('*').to_string()))
                    .unwrap_or_default();
                Prompt::new(format!("{} ({})", m.name, what), Accept { point: true, number: true, text: true, select: false, enter: true })
                    .default(shown)
            }
            None => Prompt::new(format!("Playing {}", m.name), Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.m.is_none() {
            let Input::Text(t) = i else { return Ok(Step::Cancel) };
            let Some(m) = s.action_macro(t.trim()) else {
                s.echo(format!("There is no action macro called \"{}\".", t.trim()));
                return Ok(Step::Cancel);
            };
            for (_, st) in m.steps.iter().enumerate().take_while(|(_, st)| !st.ask && st.basepoint.is_none()) {
                if let Some(msg) = &st.message {
                    s.echo(msg.clone());
                }
            }
            self.m = Some(m);
        } else {
            let v = match i {
                Input::Point(p) => json!([p.x, p.y]),
                Input::Point3(p) => json!([p.x, p.y, p.z]),
                Input::Text(t) | Input::Keyword(t) => json!(t),
                // Enter keeps the recorded value.
                _ => Value::Null,
            };
            self.given.push(v);
            self.next += 1;
        }
        let count = self.m.as_ref().map_or(0, |m| m.asks().len());
        if self.next >= count {
            return self.finish(s);
        }
        Ok(Step::Continue)
    }
}

/// ACTUSERMESSAGE typed: asks for the message.
struct MessageM;

impl crate::prompt::Interactive for MessageM {
    fn name(&self) -> &'static str {
        "ACTUSERMESSAGE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Enter the message to show when the macro plays", Accept::TEXT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Text(t) if !t.trim().is_empty() => {
                if let Err(e) = run_user_message(s, &json!({ "text": t })) {
                    s.echo(e.user_message());
                }
                Ok(Step::Done)
            }
            _ => Ok(Step::Cancel),
        }
    }
}

/// ACTBASEPOINT typed: asks for the point.
struct BaseM;

impl crate::prompt::Interactive for BaseM {
    fn name(&self) -> &'static str {
        "ACTBASEPOINT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Specify base point (asked for again when the macro plays)", Accept::POINT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let p: Option<Vec3> = match i {
            Input::Point(p) => Some(Vec3::new(p.x, p.y, 0.0)),
            Input::Point3(p) => Some(p),
            Input::Text(t) => point_line(t.trim()).map(|(_, c)| Vec3::new(c[0], c[1], c.get(2).copied().unwrap_or(0.0))),
            _ => return Ok(Step::Cancel),
        };
        let Some(p) = p else { return Ok(Step::Continue) };
        if let Err(e) = run_base_point(s, &json!({ "point": [p.x, p.y, p.z] })) {
            s.echo(e.user_message());
        }
        Ok(Step::Done)
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("actrecord", "Record", run_record).menu(&["Tools", "Action Recorder", "Record"]).enabled(has_doc).noundo(),
        CommandSpec::new("actstop", "Stop", run_stop)
            .menu(&["Tools", "Action Recorder", "Stop"])
            .params("{name? (default ActMacroN), description?, path? (default the actions folder)} → {name, steps, asks, path?}")
            .enabled(has_doc)
            .noundo(),
        CommandSpec::new("actuserinput", "Request User Input", run_user_input)
            .menu(&["Tools", "Action Recorder", "Request User Input"])
            .transparent()
            .enabled(has_doc)
            .noundo(),
        CommandSpec::new("actusermessage", "Insert Message", run_user_message)
            .menu(&["Tools", "Action Recorder", "Insert Message"])
            .params("{text}")
            .enabled(has_doc)
            .noundo()
            .interactive(|_| Ok(Box::new(MessageM))),
        CommandSpec::new("actbasepoint", "Insert Base Point", run_base_point)
            .menu(&["Tools", "Action Recorder", "Insert Base Point"])
            .params("{point: [x, y(, z)]}")
            .enabled(has_doc)
            .noundo()
            .interactive(|_| Ok(Box::new(BaseM))),
        CommandSpec::new("actplay", "Play Action Macro", run_play)
            .menu(&["Tools", "Action Recorder", "Play..."])
            .params("{name | path, inputs?: [a value per asked step, in order: [x, y(, z)] or text; null keeps the recorded one]} → {macro, steps}")
            .enabled(has_doc)
            .interactive(|_| Ok(Box::new(PlayM { m: None, next: 0, given: Vec::new() }))),
        CommandSpec::new("actions", "Action Macros", run_manage)
            .params("{action?: list|delete|ask|rename, name, step?, ask?: bool, to?} → list: {macros: [{name, description, steps, asks}], recording}")
            .enabled(always)
            .noundo(),
    ]
}

/// A typed name that is a macro's: play it (asking for its inputs).
pub(crate) fn start_by_name(s: &mut Session, name: &str) -> Option<Result<()>> {
    if find_command(name).is_some() || s.action_macro(name).is_none() {
        return None;
    }
    Some(s.start("actplay").and_then(|()| s.input(Input::Text(name.to_string()))))
}
