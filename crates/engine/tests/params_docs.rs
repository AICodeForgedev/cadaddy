//! Every command's parameter documentation and what callers pass agree (battle test 0.7.8 #9): an
//! agent's call with a parameter a command doesn't document is refused, so every parameter the
//! code base itself passes — in the app, the tests, the CLI — must be in the command's params doc.
//! The calls are read from the sources: `execute("id", &json!({…}))`, `exec(…)`, `run(…)` and
//! `execute_agent(…)` with an object literal. And the refusal itself: unknown parameters fail for
//! agents (naming them and the right ones), warn for the app's own calls. The other way round
//! too: every parameter a command's own run function reads (`str_param(p, "name")`,
//! `p.get("name")`…) is in its params doc, or an agent's correct call would be refused.

use std::path::{Path, PathBuf};

use cadaddy_engine::{Session, cmd::find_command, cmd::resolve_alias, unknown_params};
use serde_json::{Value, json};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let p = e.path();
        // This file passes wrong parameters on purpose.
        if p.ends_with("params_docs.rs") {
            continue;
        }
        if p.is_dir() && !p.ends_with("target") {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The top-level keys of the object literal starting at `text[0] == '{'`.
fn keys(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let (mut depth, mut i, mut out) = (0i32, 0usize, Vec::new());
    while i < b.len() {
        match b[i] {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                let key = &text[start.min(i)..i.min(text.len())];
                let after = text.get(i + 1..).unwrap_or_default().trim_start();
                if depth == 1 && after.starts_with(':') {
                    out.push(key.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// (file:line, command, key) of every literal parameter passed in the sources.
fn calls(root: &Path) -> Vec<(String, String, String)> {
    let mut files = Vec::new();
    for d in ["crates", "apps"] {
        sources(&root.join(d), &mut files);
    }
    let mut out = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        for call in ["execute(\"", "exec(\"", ".run(\"", "execute_agent(\"", "ok(\"engine.execute\""] {
            let mut from = 0;
            while let Some(at) = text[from..].find(call) {
                let start = from + at + call.len();
                from = start;
                let Some(end) = text[start..].find('"') else { break };
                let id = &text[start..start + end];
                if id.is_empty() || id.contains(char::is_whitespace) {
                    continue;
                }
                // The object literal right after: `, &json!({` or `, json!({`.
                let rest = &text[start + end + 1..];
                let Some(j) = rest.find("json!(") else { continue };
                if rest[..j].trim_start_matches([',', ' ', '&', '\n']).trim() != "" {
                    continue;
                }
                let lit = rest[j + 6..].trim_start();
                if !lit.starts_with('{') {
                    continue;
                }
                let line = text[..start].lines().count();
                for k in keys(lit) {
                    out.push((format!("{}:{line}", f.strip_prefix(root).unwrap_or(&f).display()), id.to_string(), k));
                }
            }
        }
    }
    out
}

/// The body of `fn name(` in `text` (to its matching brace), if it is defined there.
fn fn_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let at = text.find(&format!("fn {name}("))?;
    let open = at + text[at..].find('{')?;
    let mut depth = 0i32;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return text.get(open..open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The parameter readers the commands use: `name(…, p, "key"` with `p` the parameters.
const READERS: &[&str] = &[
    "str_param", "bool_or", "point_param", "point_req", "f64_or", "f64_req", "points_param", "handle_param",
    "usize_param", "size_param", "vec3_param", "num_param", "count_param", "radius_req", "opt_text", "count_or",
    "signed_req", "point3_req", "opt_bool", "on_param", "p3_req", "point3",
];

/// The keys `body` reads from its parameters `p`.
fn reads(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let quoted = |rest: &str| rest.strip_prefix('"').and_then(|r| r.split_once('"')).map(|(k, _)| k.to_string());
    for pat in ["p.get(", "p[", "params.get("] {
        let mut from = 0;
        while let Some(at) = body[from..].find(pat) {
            from += at + pat.len();
            // `p.get(` only when `p` is a whole word.
            let before = body[..from - pat.len()].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                continue;
            }
            out.extend(quoted(&body[from..]));
        }
    }
    for r in READERS {
        let pat = format!("{r}(");
        let mut from = 0;
        while let Some(at) = body[from..].find(&pat) {
            from += at + pat.len();
            if body[..from - pat.len()].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            // `(p, "key"` or `(cmd, p, "key"` / `("id", p, "key"`.
            let args = &body[from..];
            let Some(i) = args.find(|c: char| c == ')' || c == '"').filter(|&i| args[..i].split(',').any(|a| a.trim() == "p" || a.trim() == "&p")) else {
                continue;
            };
            let head: Vec<&str> = args[..i].split(',').map(str::trim).collect();
            if head.last().is_some_and(|a| a.is_empty()) && head.get(head.len().saturating_sub(2)).is_some_and(|a| *a == "p" || *a == "&p") {
                out.extend(quoted(&args[i..]));
            }
        }
    }
    out
}

/// (file, command, key) of every parameter a command's own run function reads directly.
fn command_reads(root: &Path) -> Vec<(String, String, String)> {
    let mut files = Vec::new();
    sources(&root.join("crates/engine/src"), &mut files);
    let texts: Vec<(PathBuf, String)> = files.into_iter().filter_map(|f| std::fs::read_to_string(&f).ok().map(|t| (f, t))).collect();
    let mut out = Vec::new();
    for (f, text) in &texts {
        let mut from = 0;
        let pat = "CommandSpec::new(\"";
        while let Some(at) = text[from..].find(pat) {
            from += at + pat.len();
            let rest = &text[from..];
            let Some((id, rest)) = rest.split_once('"') else { break };
            // `, "Title", run_fn)`: the run function's name.
            let Some(rest) = rest.strip_prefix(", \"").and_then(|r| r.split_once("\", ")).map(|(_, r)| r) else { continue };
            let path: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':').collect();
            if path.is_empty() || !rest[path.len()..].trim_start().starts_with(')') {
                continue;
            }
            let mut segs: Vec<&str> = path.split("::").filter(|s| !matches!(*s, "super" | "crate" | "self" | "cmd")).collect();
            let Some(name) = segs.pop() else { continue };
            // The file defining it: the one its module path names (`filters::run_edit` → `filters.rs`
            // or `filters/mod.rs`), else this one, else the only one.
            let defining: Vec<&(PathBuf, String)> = texts.iter().filter(|(_, t)| fn_body(t, name).is_some()).collect();
            let named = |g: &&&(PathBuf, String)| {
                segs.last().is_some_and(|m| g.0.file_stem().is_some_and(|s| s == *m) || g.0.ends_with(format!("{m}/mod.rs")))
            };
            let body = defining
                .iter()
                .find(named)
                .and_then(|g| fn_body(&g.1, name))
                .or_else(|| if segs.is_empty() { fn_body(text, name) } else { None })
                .or_else(|| (defining.len() == 1).then(|| defining.first().and_then(|g| fn_body(&g.1, name))).flatten());
            let Some(body) = body else { continue };
            for k in reads(body) {
                out.push((f.strip_prefix(root).unwrap_or(f).display().to_string(), id.to_string(), k));
            }
        }
    }
    out
}

#[test]
fn every_parameter_a_command_reads_is_documented() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let found = command_reads(&root);
    assert!(found.len() > 1000, "the scan found the reads: {}", found.len());
    let mut bad = Vec::new();
    for (at, id, key) in &found {
        let Some(spec) = find_command(id) else { continue };
        if !unknown_params(spec, &json!({ key: null })).is_empty() {
            bad.push(format!("{at}: {id} `{key}`"));
        }
    }
    bad.sort();
    bad.dedup();
    assert!(bad.is_empty(), "{} parameters read but not in the command's params doc:\n{}", bad.len(), bad.join("\n"));
}

/// Calls that pass an unknown parameter on purpose (tests of the refusal and the warning).
const ON_PURPOSE: &[&str] = &["raduis", "bogus", "nosuchparam", "notAParameter"];

#[test]
fn every_parameter_the_code_passes_is_documented() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let found = calls(&root);
    assert!(found.len() > 2000, "the scan found the calls: {}", found.len());
    let mut bad = Vec::new();
    for (at, id, key) in &found {
        let Some(spec) = find_command(&resolve_alias(&id.to_ascii_lowercase())) else { continue };
        if ON_PURPOSE.contains(&key.as_str()) {
            continue;
        }
        if !unknown_params(spec, &json!({ key: null })).is_empty() {
            bad.push(format!("{at}: {} `{key}`", spec.id));
        }
    }
    bad.sort();
    bad.dedup();
    assert!(bad.is_empty(), "{} parameters passed but not in the command's params doc:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn agents_unknown_parameters_are_refused_and_the_apps_warned() {
    let mut s = Session::new();
    s.execute("layout", &json!({ "option": "new", "name": "Sheet B" })).ok();
    // The battle test's call: `name` instead of `from`.
    let e = s.execute_agent("layout.rename", &json!({ "name": "Layout1", "to": "E-101 Power" })).unwrap_err().to_string();
    assert!(e.contains("`name` is not a parameter of layout.rename") && e.contains("`from`") && e.contains("`to`"), "{e}");
    let layouts = s.execute("layout.list", &json!({})).unwrap().to_string();
    assert!(!layouts.contains("E-101 Power"), "nothing renamed: {layouts}");
    // The right call works.
    s.execute_agent("layout.rename", &json!({ "from": "Layout1", "to": "E-101 Power" })).unwrap();
    assert!(s.execute("layout.list", &json!({})).unwrap().to_string().contains("E-101 Power"));
    // Valid parameters the old check called unknown: no complaint.
    let r = s.execute_agent("rectang", &json!({ "p1": [0, 0], "p2": [10, 5] })).unwrap();
    assert!(r.get("warnings").is_none(), "{r}");
    let r = s.execute_agent("dimstyle", &json!({ "name": "Arch", "DIMSCALE": 48, "DIMLUNIT": 4 })).unwrap();
    assert!(r.get("warnings").is_none(), "{r}");
    // A misspelt parameter from the app's own code still runs, with a warning.
    let r = s.execute("circle", &json!({ "center": [0, 0], "radius": 2, "raduis": 3 })).unwrap();
    assert!(r["warnings"].to_string().contains("raduis"), "{r}");
    assert!(s.execute_agent("circle", &json!({ "center": [0, 0], "raduis": 3 })).is_err());
    // A dry run refuses it too.
    s.execute("agent.dryrun", &json!({ "on": true })).ok();
    assert!(s.execute_agent("circle", &json!({ "center": [0, 0], "raduis": 3 })).is_err());
    let _: Value = Value::Null;
}
