//! The whole interface gives memory back (Q240): drawings opened and closed in the app, every
//! interface language and theme switched to, Walk turned on and off, with real frames between,
//! pass after pass. After a warm-up the process's memory must stay within a budget: a leak (a
//! texture, font or panel state kept per drawing or per switch) grows with every pass.
//!
//! One test in its own binary: the process's memory is the measurement. It is its private bytes on
//! Windows, anonymous resident memory on Linux and resident memory on macOS; where none can be
//! read, the structural checks still run. No GPU here: the canvas's GPU buffers are the soak's
//! (`cargo xtask soak --leak --mode window`).

use crate::e2e_common::{E2e, folder};
use serde_json::json;

/// Growth allowed from the first measured pass to the last, in MB.
const BUDGET_MB: f64 = 16.0;
/// Growth allowed from the warm-up's end to the last pass, in MB (the allocator keeping freed
/// memory once included).
const STEP_MB: f64 = 64.0;
const DRAWINGS: usize = 10;
const WARM_UP: usize = 2;
const PASSES: usize = 4;

const LANGUAGES: [&str; 18] = ["en", "uk", "pt-BR", "de", "es", "cs", "ru", "fr", "zh", "it", "ko", "sv", "ar", "ja", "zh-TW", "pl", "hu", "tr"];
const THEMES: [&str; 4] = ["ui.theme.light", "ui.theme.high-contrast", "ui.theme.system", "ui.theme.dark"];

/// This process's memory in MB, when the system says.
fn memory_mb() -> Option<f64> {
    let pid = std::process::id();
    if cfg!(windows) {
        let script = format!("(Get-Process -Id {pid}).PrivateMemorySize64");
        let out = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().ok()?;
        return String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|b| b / 1_048_576.0);
    }
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        let kb = status.lines().find_map(|l| l.strip_prefix("RssAnon:"))?.trim().trim_end_matches("kB").trim().parse::<f64>().ok()?;
        return Some(kb / 1024.0);
    }
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|kb| kb / 1024.0)
}

/// One pass: drawings made, saved, closed, opened again and closed; every language and theme;
/// Walk on and off. The weak references of the drawings it closed.
fn pass(e: &mut E2e, p: usize, dir: &std::path::Path) -> Vec<std::sync::Weak<cadaddy_engine::doc::Drawing>> {
    let mut closed = Vec::new();
    for n in 0..DRAWINGS {
        e.exec("new", json!({}));
        for i in 0..10 {
            let x = f64::from(i) * 12.0;
            e.exec("line", json!({ "points": [[x, 0.0], [x + 10.0, 0.0], [x + 10.0, 6.0]] }));
            e.exec("circle", json!({ "center": [x + 5.0, 3.0], "radius": 2 }));
        }
        e.exec("text", json!({ "at": [0, -5], "text": format!("Pass {p} drawing {n}"), "height": 1.5 }));
        e.typed("MOVE L  0,0 3,3");
        e.typed("U");
        let file = dir.join(format!("ui-{p}-{n}.{}", ["dxf", "dwg"][n % 2]));
        e.exec("saveas", json!({ "path": file }));
        e.exec("close", json!({}));
        e.exec("open", json!({ "path": file }));
        if let Ok(st) = e.app.session.state() {
            closed.push(std::sync::Arc::downgrade(&st.doc));
        }
        e.exec("close", json!({}));
        let _ = std::fs::remove_file(file);
    }
    for (i, lang) in LANGUAGES.iter().enumerate() {
        e.exec("ui.language", json!({ "lang": lang }));
        if let Some(theme) = THEMES.get(i % THEMES.len()) {
            e.exec(theme, json!({}));
        }
    }
    e.exec("ui.language", json!({ "lang": "en" }));
    e.exec("ui.theme.dark", json!({}));
    e.exec("new", json!({}));
    for _ in 0..10 {
        let _ = e.call("engine.execute", json!({ "command": "ui.walk", "params": {} }));
        let _ = e.call("engine.execute", json!({ "command": "ui.walk", "params": {} }));
    }
    e.exec("close", json!({}));
    closed
}

#[test]
fn memory_returns_after_drawings_languages_themes_and_walks() {
    let dir = folder("memory-budget");
    let mut e = E2e::new();
    let mut closed = Vec::new();
    for p in 0..WARM_UP {
        closed.extend(pass(&mut e, p, &dir));
    }
    let base = memory_mb();
    let mut after = Vec::new();
    for p in WARM_UP..WARM_UP + PASSES {
        closed.extend(pass(&mut e, p, &dir));
        after.push(memory_mb());
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(closed.len() == (WARM_UP + PASSES) * DRAWINGS, "every drawing opened again: {}", closed.len());
    assert!(closed.iter().all(|w| w.upgrade().is_none()), "the interface still holds a closed drawing");
    if let (Some(base), Some(Some(first)), Some(Some(end))) = (base, after.first(), after.last()) {
        eprintln!("memory: {base:.1} MB after the warm-up, then {after:.1?} MB after each pass");
        let work = format!("{} drawings and {} language switches", (PASSES - 1) * DRAWINGS, (PASSES - 1) * LANGUAGES.len());
        assert!(end - first < BUDGET_MB, "memory grew {:.1} MB (from {first:.1} to {end:.1}) over {work}; the budget is {BUDGET_MB} MB", end - first);
        assert!(end - base < STEP_MB, "memory ended {:.1} MB above where the warm-up left it ({base:.1} MB); at most {STEP_MB} MB", end - base);
    }
}
