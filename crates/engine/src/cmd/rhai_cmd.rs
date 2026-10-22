//! Rhai scripting commands: `rhai` (evaluate code), `rhairun` (run a `.rhai` file), and
//! `rhaiedit` (open the Script Editor palette).
//!
//! These follow the same patterns as the AutoLISP commands in `cmd/lisp.rs`: programmatic callers
//! pass `{code}` or `{path}`; the interactive form prompts for a file name.  SECURELOAD applies
//! to `.rhai` files the same way it applies to `.lsp` files.

use serde_json::{Value, json};

use super::{CommandSpec, bad, str_param};
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Maximum scripted inputs a single `rhai` call may provide.
const MAX_INPUTS: usize = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("rhai", "Evaluate Rhai", run_rhai)
            .menu(&["Tools", "Rhai", "Evaluate"])
            .params(
                "{code: Rhai source, inputs?: [answers to getpoint/getreal/getstring calls]} \
                 → {value, output: [lines printed], error?}",
            )
            .noundo(),
        CommandSpec::new("rhairun", "Run Rhai Script", run_rhairun)
            .menu(&["Tools", "Rhai", "Run Script..."])
            .alias(&["rsr"])
            .params("{path: .rhai file} | {name, text: source} → {value, output, error?}")
            .noundo()
            .interactive(|_| Ok(Box::new(RhaiRunM))),
        CommandSpec::new("rhaiedit", "Script Editor", run_rhaiedit)
            .menu(&["Tools", "Rhai", "Script Editor"])
            .params("{} — opens the Script Editor palette")
            .enabled(super::always)
            .noundo(),
    ]
}

/// `rhai {code, inputs?}` — evaluate Rhai source inline.
fn run_rhai(s: &mut Session, p: &Value) -> Result<Value> {
    let code = str_param(p, "code").ok_or_else(|| bad("rhai", "`code` is required"))?;
    let inputs = match p.get("inputs") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) if a.len() <= MAX_INPUTS => a.clone(),
        Some(Value::Array(_)) => return Err(bad("rhai", format!("`inputs` holds at most {MAX_INPUTS} values"))),
        Some(_) => return Err(bad("rhai", "`inputs` must be an array")),
    };
    eval_rhai(s, code, inputs)
}

/// `rhairun {path} | {name, text}` — load and run a `.rhai` file.
fn run_rhairun(s: &mut Session, p: &Value) -> Result<Value> {
    let (_name, text) = rhai_source(s, p)?;
    eval_rhai(s, &text, Vec::new())
}

/// `rhaiedit {}` — the UI opens the Script Editor palette (a dialog name the front end knows).
fn run_rhaiedit(s: &mut Session, _p: &Value) -> Result<Value> {
    s.echo("RHAIEDIT".to_string());
    Ok(json!({ "palette": "scripteditor" }))
}

/// Evaluate Rhai code and return the JSON result.
fn eval_rhai(s: &mut Session, code: &str, inputs: Vec<Value>) -> Result<Value> {
    // Mark that a script is running so SECURELOAD changes are blocked.
    let was_in_script = s.in_script;
    s.in_script = true;
    let result = s.rhai_eval(code, inputs);
    s.in_script = was_in_script;
    match result {
        Ok((val, output)) => Ok(json!({ "value": val, "output": output })),
        Err(e) => Ok(json!({ "error": e, "output": [] })),
    }
}

/// Read a `.rhai` file from `{path}` or `{name, text}`, applying SECURELOAD to path-based loads.
fn rhai_source(s: &mut Session, p: &Value) -> Result<(String, String)> {
    if let Some(text) = str_param(p, "text") {
        return Ok((str_param(p, "name").unwrap_or("untitled.rhai").to_string(), text.to_string()));
    }
    let path = str_param(p, "path").ok_or_else(|| bad("rhairun", "`path` or `text` is required"))?;
    s.check_load(std::path::Path::new(path), "RHAIRUN", Some(("rhairun".into(), json!({ "path": path }))))?;
    let bytes = crate::readfile::read(path).map_err(|e| bad("rhairun", format!("{path}: {e}")))?;
    Ok((path.to_string(), String::from_utf8_lossy(&bytes).into_owned()))
}

/// Interactive RHAIRUN: prompts for a file name.
struct RhaiRunM;

impl Interactive for RhaiRunM {
    fn name(&self) -> &'static str {
        "RHAIRUN"
    }
    fn prompt(&self, _: &Session) -> Prompt {
        Prompt::new("Enter name of Rhai script to run", Accept::TEXT)
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
        match rhai_source(s, &json!({ "path": path })) {
            Ok((_name, text)) => {
                match eval_rhai(s, &text, Vec::new()) {
                    Ok(r) => {
                        if let Some(err) = r.get("error").and_then(Value::as_str) {
                            s.echo(format!("Error: {err}"));
                        }
                    }
                    Err(e) => s.echo(e.to_string()),
                }
                Ok(Step::Done)
            }
            Err(e) => {
                s.echo(e.to_string());
                Ok(Step::Done)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn rhai_eval_simple_math() {
        let mut s = Session::new();
        let r = s.execute("rhai", &json!({"code": "40 + 2"})).unwrap();
        assert_eq!(r["value"], json!(42));
    }

    #[test]
    fn rhai_eval_prints_to_output() {
        let mut s = Session::new();
        let r = s.execute("rhai", &json!({"code": r#"print("hello from rhai");"#})).unwrap();
        let output = r["output"].as_array().unwrap();
        assert!(output.iter().any(|l| l.as_str().unwrap().contains("hello from rhai")));
    }

    #[test]
    fn rhai_eval_returns_error_on_bad_code() {
        let mut s = Session::new();
        let r = s.execute("rhai", &json!({"code": "let x = ??? "})).unwrap();
        assert!(r.get("error").is_some(), "expected error: {r}");
    }

    #[test]
    fn rhai_scope_persists() {
        let mut s = Session::new();
        s.execute("rhai", &json!({"code": "let x = 10;"})).unwrap();
        let r = s.execute("rhai", &json!({"code": "x + 5"})).unwrap();
        assert_eq!(r["value"], json!(15));
    }

    #[test]
    fn rhai_command_call_works() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        let r = s.execute("rhai", &json!({"code": r#"let r = command("line", #{ points: [[0.0, 0.0], [10.0, 10.0]] }); r"#})).unwrap();
        assert!(r.get("error").is_none(), "unexpected error: {r}");
    }

    #[test]
    fn rhai_getvar_setvar() {
        let mut s = Session::new();
        let r = s.execute("rhai", &json!({"code": r#"let v = getvar("SECURELOAD"); v"#})).unwrap();
        assert_eq!(r["value"], json!(1), "SECURELOAD default is 1");
    }

    #[test]
    fn rhai_selection_empty() {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        let r = s.execute("rhai", &json!({"code": "let ss = selection(); sslength(ss)"})).unwrap();
        assert_eq!(r["value"], json!(0));
    }

    #[test]
    fn rhairun_with_inline_text() {
        let mut s = Session::new();
        let r = s.execute("rhairun", &json!({"name": "test.rhai", "text": "42 + 1"})).unwrap();
        assert_eq!(r["value"], json!(43));
    }

    #[test]
    fn rhai_code_required() {
        let mut s = Session::new();
        let r = s.execute("rhai", &json!({}));
        assert!(r.is_err());
    }

    #[test]
    fn rhai_inputs_too_many_refused() {
        let mut s = Session::new();
        let big: Vec<serde_json::Value> = (0..10_001).map(|i| json!(i)).collect();
        let r = s.execute("rhai", &json!({"code": "42", "inputs": big}));
        assert!(r.is_err());
    }
}
