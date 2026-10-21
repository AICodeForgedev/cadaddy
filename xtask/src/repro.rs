//! `cargo xtask repro FILE [--save OUT]`: replay a recorded journal or action macro headless.
//!
//! A journal is a `.scr` script (one command per line, points as `*x,y`). An action macro is a
//! `.actm` JSON file from the ACTRECORD command. Both are replayed through `cadaddy-cli run
//! --script-file FILE`, the same way the CLI runs scripts.

use std::path::Path;

use crate::{cargo, root, run};

pub fn run_repro(args: &[&str]) -> Result<(), String> {
    let mut it = args.iter();
    let mut file: Option<&str> = None;
    let mut save: Option<&str> = None;
    while let Some(a) = it.next() {
        match *a {
            "--save" => save = Some(it.next().ok_or("--save needs a path")?),
            f if !f.starts_with("--") && file.is_none() => file = Some(f),
            other => return Err(format!("unknown option or duplicate file: {other}")),
        }
    }
    let file = file.ok_or("usage: cargo xtask repro FILE [--save OUT]")?;
    let path = Path::new(file);
    if !path.is_file() {
        return Err(format!("{file}: not found"));
    }

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "scr" => run_script(file, save),
        "actm" => run_action_macro(file, save),
        _ => {
            // Try as a script (text lines).
            run_script(file, save)
        }
    }
}

/// Replay a script (.scr) file.
fn run_script(file: &str, save: Option<&str>) -> Result<(), String> {
    let mut c = cargo();
    c.current_dir(root());
    c.args(["run", "-q", "-p", "cadaddy-cli", "--", "run", "--script-file", file]);
    if let Some(out) = save {
        c.args(["--save", out]);
    }
    run(c, &format!("cadaddy-cli run --script-file {file}"))
}

/// Replay an action macro (.actm) file: convert its steps to a script and replay.
fn run_action_macro(file: &str, save: Option<&str>) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{file}: bad JSON: {e}"))?;
    let steps = v.get("steps").and_then(|s| s.as_array()).ok_or_else(|| format!("{file}: no \"steps\" array"))?;
    let mut script = String::new();
    for step in steps {
        if let Some(msg) = step.get("message").and_then(|m| m.as_str()) {
            script.push_str(&format!("; {msg}\n"));
        }
        if let Some(input) = step.get("input").and_then(|i| i.as_str()) {
            script.push_str(input);
            script.push('\n');
        }
    }
    // Write the script to a temp file and replay it.
    let dir = std::env::temp_dir().join("cadaddy-repro");
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join(format!("repro-{}.scr", std::process::id()));
    std::fs::write(&tmp, &script).map_err(|e| format!("temp script: {e}"))?;
    let result = run_script(&tmp.to_string_lossy(), save);
    let _ = std::fs::remove_file(&tmp);
    result
}
