//! Verify that every back-ticked UPPERCASE command name in `docs/guide/*.md`
//! is a registered command id or alias.  This catches invented commands
//! (like DBLIST) that would fail if a user or agent copied them.

use std::collections::HashSet;

/// Workspace root (three parents up from this crate's manifest dir).
fn workspace_root() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Build the set of known command ids and aliases (all uppercase).
fn known_commands() -> HashSet<String> {
    let specs = cadaddy_engine::cmd::command_specs();
    let mut set = HashSet::new();
    for s in specs {
        set.insert(s.id.to_ascii_uppercase());
        for a in s.aliases {
            set.insert(a.to_ascii_uppercase());
        }
    }
    set
}

/// Extract back-ticked words from markdown that look like commands: `WORD`
/// where WORD is all uppercase ASCII letters (at least 2 chars) and optionally
/// contains dots or hyphens (e.g. `ZOOM.EXTENTS`, `PLAN.WALLS`).
fn extract_backtick_commands(text: &str) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        // Skip code blocks (``` fenced)
        if line.trim_start().starts_with("```") {
            continue;
        }
        let mut rest = line;
        while let Some(start) = rest.find('`') {
            rest = &rest[start + 1..];
            if let Some(end) = rest.find('`') {
                let inside = &rest[..end];
                rest = &rest[end + 1..];
                // Must be all uppercase ASCII + dots/hyphens, at least 2 chars,
                // and at least 2 ASCII letters
                let alpha_count = inside.chars().filter(|c| c.is_ascii_uppercase()).count();
                if alpha_count >= 2 && inside.len() >= 2 && inside.chars().all(|c| c.is_ascii_uppercase() || c == '.' || c == '-' || c == '_') {
                    results.push((line_no + 1, inside.to_string()));
                }
            } else {
                break;
            }
        }
    }
    results
}

/// Commands that appear as keywords in the guides but are not engine command
/// ids (e.g. mode names, parameter names, layer names).  These are excluded
/// from the check.
fn allowed_non_commands() -> HashSet<&'static str> {
    [
        // Mode keywords for MEASUREGEOM
        "AREA",
        // File extensions / format names mentioned in tables
        "DWG",
        "DXF",
        "DWT",
        "PDF",
        "SVG",
        "PNG",
        "CDY",
        "IFC",
        "GLB",
        "GLTF",
        "OBJ",
        "STL",
        // Layer name patterns
        "E-POWER",
        "A-WALL",
        "A-DOOR",
        "A-GLAZ",
        "A-ANNO",
        "A-DIMS",
        // System variable names
        "INSUNITS",
        "DIMSCALE",
        "LTSCALE",
        "FILEDIA",
        "CLAYER",
        "STEPSIZE",
        "STEPSPERSEC",
        "CADADDY_TAKEOFF_WASTE",
        // Keyboard keys
        "CTRL",
        "ENTER",
    ]
    .into_iter()
    .collect()
}

#[test]
fn guide_commands_exist_in_registry() {
    let root = workspace_root();
    let guide_dir = root.join("docs/guide");

    let known = known_commands();
    let allowed = allowed_non_commands();

    let mut unknown: Vec<(String, usize, String)> = Vec::new();

    for entry in std::fs::read_dir(&guide_dir).expect("docs/guide/ must exist") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        // Skip command-reference.md (auto-generated, not hand-written)
        if path.file_name().unwrap().to_str() == Some("command-reference.md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        for (line, cmd) in extract_backtick_commands(&text) {
            let upper = cmd.to_ascii_uppercase();
            if !known.contains(&upper) && !allowed.contains(upper.as_str()) {
                unknown.push((file_name.clone(), line, cmd));
            }
        }
    }

    assert!(
        unknown.is_empty(),
        "Guide files reference commands that are not registered ids or aliases \
         (add them to the registry, fix the guide, or add to allowed_non_commands):\n{}",
        unknown.iter().map(|(f, l, c)| format!("  {f}:{l}: `{c}`")).collect::<Vec<_>>().join("\n")
    );
}
