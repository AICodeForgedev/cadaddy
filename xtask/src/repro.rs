//! `cargo xtask repro FILE [--save OUT]`: replay a recorded journal or action macro headless.
//!
//! A journal is a `.scr` script (one command per line, points as `*x,y`). An action macro is a
//! `.cdyact` (or `.actm`) JSON file from the ACTRECORD command. Both are replayed through
//! `cadaddy-cli run --script-file FILE`, the same way the CLI runs scripts.

use std::path::Path;

#[cfg(test)]
use cadaddy_engine::cmd::actions::MacroStep;
use cadaddy_engine::cmd::actions::{EXTENSION, Macro};

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

    // Resolve FILE and --save against the caller's working directory, not the workspace root.
    let cwd = std::env::current_dir().map_err(|e| format!("can't read the working directory: {e}"))?;
    let path = cwd.join(file);
    if !path.is_file() {
        return Err(format!("{file}: not found"));
    }
    let save_abs = save.map(|s| cwd.join(s));

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let file_abs = path.to_string_lossy().into_owned();
    let save_ref = save_abs.as_ref().map(|p| p.to_string_lossy().into_owned());
    match ext.as_str() {
        "scr" => run_script(&file_abs, save_ref.as_deref()),
        ext if ext == EXTENSION || ext == "actm" => run_action_macro(&path, save_ref.as_deref()),
        _ => {
            // Try as a script (text lines).
            run_script(&file_abs, save_ref.as_deref())
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

/// Convert a [`Macro`]'s steps to a script string for replay: messages become `; comment` lines,
/// inputs are written as-is (one per line), and `ask` steps use the recorded value (there is no
/// interactive prompt in headless replay).
pub fn macro_to_script(m: &Macro) -> String {
    let mut script = String::new();
    for step in &m.steps {
        if let Some(msg) = &step.message {
            script.push_str(&format!("; {msg}\n"));
            continue;
        }
        if let Some(input) = &step.input {
            script.push_str(input);
            script.push('\n');
        }
    }
    script
}

/// Replay an action macro (`.cdyact` or `.actm`): parse with the engine's [`Macro`] type,
/// convert its steps to a script and replay through `cadaddy-cli`.
fn run_action_macro(path: &Path, save: Option<&str>) -> Result<(), String> {
    let display = path.display();
    let text = std::fs::read_to_string(path).map_err(|e| format!("{display}: {e}"))?;
    let m: Macro = serde_json::from_str(&text).map_err(|e| format!("{display}: not an action macro ({e})"))?;
    if m.format != "cadaddy-action-macro" {
        return Err(format!("{display}: not a CADaddy action macro"));
    }
    let script = macro_to_script(&m);
    // Write the script to a temp file and replay it.
    let dir = std::env::temp_dir().join("cadaddy-repro");
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join(format!("repro-{}.scr", std::process::id()));
    std::fs::write(&tmp, &script).map_err(|e| format!("temp script: {e}"))?;
    let result = run_script(&tmp.to_string_lossy(), save);
    let _ = std::fs::remove_file(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_to_script_converts_steps_correctly() {
        let m = Macro {
            format: "cadaddy-action-macro".into(),
            version: 1,
            name: "TestMacro".into(),
            description: String::new(),
            steps: vec![
                MacroStep { input: Some("line".into()), ask: false, prompt: String::new(), message: None, basepoint: None },
                MacroStep { input: Some("*0,0".into()), ask: false, prompt: String::new(), message: None, basepoint: None },
                MacroStep { input: None, ask: false, prompt: String::new(), message: Some("Drawing a line to (10,0)".into()), basepoint: None },
                MacroStep { input: Some("*10,0".into()), ask: false, prompt: String::new(), message: None, basepoint: None },
                // An `ask` step: headless replay uses the recorded value.
                MacroStep { input: Some("".into()), ask: true, prompt: "Specify next point".into(), message: None, basepoint: None },
            ],
        };
        let script = macro_to_script(&m);
        assert!(script.contains("line\n"), "command name: {script}");
        assert!(script.contains("*0,0\n"), "first point: {script}");
        assert!(script.contains("; Drawing a line to (10,0)\n"), "message as comment: {script}");
        assert!(script.contains("*10,0\n"), "second point: {script}");
        // The ask step's recorded value (empty = Enter) appears as an empty line.
        assert!(script.contains("\n\n"), "ask step uses recorded value: {script}");
    }

    #[test]
    fn cdyact_extension_is_accepted() {
        assert_eq!(EXTENSION, "cdyact");
        // Both .cdyact and .actm route to the action-macro parser (tested by extension matching
        // in run_repro; the actual replay needs the CLI binary, so this test checks the parse path
        // with a temp file).
        let dir = std::env::temp_dir().join(format!("cadaddy-repro-ext-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let m = Macro {
            format: "cadaddy-action-macro".into(),
            version: 1,
            name: "Ext".into(),
            description: String::new(),
            steps: vec![MacroStep { input: Some("line".into()), ask: false, prompt: String::new(), message: None, basepoint: None }],
        };
        let json = serde_json::to_string(&m).unwrap();
        let cdyact = dir.join("test.cdyact");
        let actm = dir.join("test.actm");
        std::fs::write(&cdyact, &json).unwrap();
        std::fs::write(&actm, &json).unwrap();

        // Both parse as valid Macro types with the correct format field.
        for path in [&cdyact, &actm] {
            let text = std::fs::read_to_string(path).unwrap();
            let parsed: Macro = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed.format, "cadaddy-action-macro");
            assert_eq!(parsed.steps.len(), 1);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
