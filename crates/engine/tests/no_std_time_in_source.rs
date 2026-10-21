//! Guard against `std::time::Instant` in non-test engine and render source (webtest #1/#2:
//! `std::time::Instant` panics on wasm32; all production code must use `web_time`).

#[test]
fn engine_source_does_not_use_std_time_instant() {
    check_crate("engine");
}

#[test]
fn render_source_does_not_use_std_time_instant() {
    check_crate("render");
}

fn check_crate(name: &str) {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(name)
        .join("src");
    let mut bad = Vec::new();
    walk(&src, &mut bad);
    assert!(bad.is_empty(), "std::time::Instant in production source (use web_time instead):\n{}", bad.join("\n"));
}

fn walk(dir: &std::path::Path, bad: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, bad);
        } else if path.extension().is_some_and(|e| e == "rs")
            // tests.rs files are #[cfg(test)] modules: they never run on wasm.
            && !path.file_name().is_some_and(|n| n == "tests.rs")
        {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for (i, line) in text.lines().enumerate() {
                // Skip comments and #[cfg(test)] test code.
                if line.contains("std::time::Instant") && !line.trim_start().starts_with("//") {
                    bad.push(format!("  {}:{}: {}", path.display(), i + 1, line.trim()));
                }
            }
        }
    }
}
