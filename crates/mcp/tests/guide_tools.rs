//! Verify that `docs/guide/for-ai-agents.md` names every MCP tool registered in
//! `tools.rs`.  This test fails when a tool is added to the server but not to
//! the guide, so the docs cannot silently drift.

use serde_json::Value;

/// Return the workspace root (two parents up from this crate's manifest dir).
fn workspace_root() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Extract every `"name"` from the MCP tool definitions (headless mode:
/// has_ui = false gives the base set; with has_ui = true adds the UI tools).
fn tool_names() -> Vec<String> {
    let defs = cadaddy_mcp::tools::tool_definitions(true);
    let tools = defs.get("tools").and_then(Value::as_array).expect("tools array");
    tools.iter().filter_map(|t| t.get("name").and_then(Value::as_str)).map(str::to_string).collect()
}

#[test]
fn guide_lists_every_mcp_tool() {
    let guide_path = workspace_root().join("docs/guide/for-ai-agents.md");
    let guide = std::fs::read_to_string(&guide_path).unwrap_or_else(|e| {
        panic!("cannot read {}: {e}", guide_path.display());
    });

    let names = tool_names();
    assert!(!names.is_empty(), "tool_definitions returned no tools");

    let mut missing: Vec<&str> = Vec::new();
    for name in &names {
        // The guide uses `tool_name` in backtick table cells
        if !guide.contains(&format!("`{name}`")) {
            missing.push(name);
        }
    }

    assert!(missing.is_empty(), "MCP tools not mentioned in for-ai-agents.md (add them to the guide):\n  {}", missing.join(", "));
}
