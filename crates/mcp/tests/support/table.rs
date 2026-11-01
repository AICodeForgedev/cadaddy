//! The MCP suite's tool table, shared by `crates/mcp/tests/suite.rs` (the headless server) and
//! `xtask/src/mcptest.rs` (the running app, through `cadaddy-cli mcp --connect`): one list, so a
//! tool is added in one place and both runs cover it. Included with `#[path]`, not a crate.

use serde_json::{Value, json};

/// A minimal synthetic PDF with a rectangle (a "floor plan") in the temp dir.
pub fn synthetic_pdf(dir: &str) -> String {
    let path = format!("{dir}/synthetic.pdf");
    // Build a valid PDF with correct xref offsets.
    let stream = b"0.5 w 72 72 468 648 re S 200 400 m 400 400 l S";
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");
    let o1 = pdf.len();
    pdf.extend_from_slice(b"1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n");
    let o2 = pdf.len();
    pdf.extend_from_slice(b"2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n");
    let o3 = pdf.len();
    pdf.extend_from_slice(b"3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R/Resources<<>>>>endobj\n");
    let o4 = pdf.len();
    pdf.extend_from_slice(format!("4 0 obj<</Length {}>>stream\n", stream.len()).as_bytes());
    pdf.extend_from_slice(stream);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = pdf.len();
    pdf.extend_from_slice(b"xref\n0 5\n");
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    pdf.extend_from_slice(format!("{o1:010} 00000 n \n").as_bytes());
    pdf.extend_from_slice(format!("{o2:010} 00000 n \n").as_bytes());
    pdf.extend_from_slice(format!("{o3:010} 00000 n \n").as_bytes());
    pdf.extend_from_slice(format!("{o4:010} 00000 n \n").as_bytes());
    pdf.extend_from_slice(format!("trailer<</Size 5/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF").as_bytes());
    std::fs::write(&path, &pdf).unwrap();
    path
}

/// Every tool: (name, a normal call, a bad call). Normal calls run in this order on one session.
pub fn table(dir: &str) -> Vec<(&'static str, Value, Value)> {
    vec![
        ("new_drawing", json!({ "metric": true }), json!({ "metric": "yes" })),
        ("command_line", json!({ "text": "circle 0,0 5" }), json!({ "text": 5 })),
        ("script", json!({ "text": "LINE 0,0 10,10\n\n" }), json!({})),
        ("execute", json!({ "command": "rectang", "params": { "p1": [20, 0], "p2": [30, 10] } }), json!({ "command": ["line"] })),
        ("list_commands", json!({ "filter": "circle" }), json!({ "filter": 1 })),
        ("inspect_drawing", json!({ "entities": false }), json!({ "limit": "ten" })),
        ("query_entities", json!({ "type": "Circle" }), json!({ "window": [[0, 0]] })),
        ("find_text", json!({ "contains": "x" }), json!({ "offset": -1 })),
        ("render", json!({ "width": 64, "height": 64 }), json!({ "width": 3 })),
        ("stairs", json!({ "action": "list" }), json!({})),
        ("rooms", json!({ "action": "list" }), json!({})),
        ("mep", json!({ "action": "list" }), json!({ "cursor": -1 })),
        ("site_takeoff", json!({ "limit": 5 }), json!({ "limit": "ten" })),
        ("finishes_takeoff", json!({ "trade": "flooring" }), json!({ "page": "one" })),
        ("callouts", json!({ "page": 1 }), json!({ "page": "one" })),
        ("lv_devices", json!({ "page": 1 }), json!({ "page": "one" })),
        ("lv_sleeves", json!({ "page": 1 }), json!({ "page": "one" })),
        ("lv_bom", json!({ "has_lv_docs": true }), json!({ "has_lv_docs": "yes" })),
        ("lv_connections", json!({ "page": 1 }), json!({ "page": "one" })),
        ("codebook", json!({ "action": "rules", "topic": "framing", "limit": 5 }), json!({ "action": "fly" })),
        ("arch_read", json!({ "limit": 5 }), json!({ "limit": "ten" })),
        ("bim_job", json!({ "action": "briefing" }), json!({ "action": "fly" })),
        ("guided_job", json!({ "action": "explain", "stage": "classify" }), json!({ "action": "fly" })),
        ("import_check", json!({}), json!({ "path": 5 })),
        ("bim_types", json!({ "action": "list" }), json!({ "action": "set", "fields": 5 })),
        ("bim_props", json!({ "action": "summary" }), json!({ "action": "set", "key": 5 })),
        (
            "bim_spec_import",
            json!({ "action": "import", "text": "LUMINAIRE SCHEDULE\nTYPE A  2X4 LED TROFFER  ACME LT-24  120V" }),
            json!({ "action": "import", "text": 5 }),
        ),
        ("project_rules", json!({ "action": "list" }), json!({ "action": "set", "domain": "mounting_height", "key": 5 })),
        (
            "project",
            json!({ "action": "create", "path": format!("{dir}/suite-project"), "name": "Project A" }),
            json!({ "action": "open", "path": 5 }),
        ),
        ("import_check", json!({}), json!({ "path": 5 })),
        // bim_from_pdf with a synthetic PDF — the PDF is minimal, import will produce a mostly
        // empty drawing, but the pipeline runs through all stages.
        ("bim_from_pdf", json!({ "path": &synthetic_pdf(dir) }), json!({ "path": 5 })),
        ("workflows", json!({}), json!("bad")),
        ("job_status", json!({}), json!({ "id": 5 })),
        ("guided_job", json!({ "action": "status" }), json!({ "action": "fly" })),
        ("select", json!({ "all": true }), json!({ "handles": "1A" })),
        ("get_variable", json!({ "name": "CLAYER" }), json!({})),
        ("set_variable", json!({ "name": "LTSCALE", "value": 2 }), json!({ "name": "LTSCALE" })),
        ("run_lisp", json!({ "code": "(+ 1 2)" }), json!({ "code": 3 })),
        ("execute_many", json!({ "commands": [{ "command": "line", "params": { "points": [[0, 0], [1, 1]] } }] }), json!({ "commands": "line" })),
        // Off, so no event notifications land between the suite's replies.
        ("watch_events", json!({ "on": false }), json!({ "on": "yes" })),
        ("drawing_summary", json!({ "textLimit": 2 }), json!({ "sections": "texts" })),
        ("drawing_query", json!({ "type": "Line" }), json!({ "limit": "all" })),
        ("undo", json!({ "count": 1 }), json!({ "count": 0 })),
        ("redo", json!({ "count": 1 }), json!({ "count": "1" })),
        ("cancel", json!({}), json!("now")),
        ("get_prompt", json!({}), json!([1])),
        ("wait_until_idle", json!({ "timeoutMs": 100 }), json!({ "timeoutMs": -5 })),
        ("save", json!({ "path": format!("{dir}/suite.dxf") }), json!({ "path": 7 })),
        // Frames the walls of the sheet just saved (none: an empty framing, still an answer).
        ("framing", json!({ "action": "build", "params": { "paths": [format!("{dir}/suite.dxf")] } }), json!({ "action": "frame" })),
        // The drawing's as-built record (empty: an empty page, still an answer).
        ("asbuilt", json!({ "action": "list", "params": { "limit": 5 } }), json!({ "action": "fly" })),
        // The drawing's cover record (none yet: an empty page with a hint, still an answer).
        ("cover", json!({ "action": "status", "params": { "limit": 5 } }), json!({ "action": "fly" })),
        ("plot", json!({ "path": format!("{dir}/suite.pdf") }), json!({})),
        ("open", json!({ "path": format!("{dir}/suite.dxf") }), json!({})),
        ("list_drawings", json!({}), json!("all")),
        ("switch_drawing", json!({ "index": 0 }), json!({ "index": -1 })),
        ("close_drawing", json!({ "discard": true }), json!({ "index": "a" })),
        ("close_all", json!({ "discard": true }), json!({ "save": "no" })),
        // The app's own (not listed headless).
        ("screenshot", json!({}), json!({ "path": 1 })),
        ("ui_inspect", json!({}), json!("x")),
        ("answer_close_prompt", json!({ "choice": "cancel" }), json!({})),
        ("quit_app", json!({ "discard": true }), json!({ "save": 1 })),
        ("ui_elements", json!({}), json!({ "label": 1 })),
        ("ui_click", json!({ "x": 1, "y": 1 }), json!({ "x": "a", "y": 1 })),
    ]
}

#[cfg(test)]
pub mod tests {
    use super::table;
    use serde_json::Value;

    /// The table with its scratch folder (the synthetic PDF is written there).
    pub fn sample() -> Vec<(&'static str, Value, Value)> {
        let dir = std::env::temp_dir().join(format!("cadaddy-mcp-table-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_default();
        table(&dir.to_string_lossy())
    }

    /// The table names every tool the server can list: the headless ones and the app-only ones.
    #[test]
    fn table_has_an_entry_for_every_headless_and_app_only_tool() {
        let t = sample();
        for has_ui in [false, true] {
            let defs = cadaddy_mcp::tools::tool_definitions(has_ui);
            let names: Vec<&str> = defs["tools"].as_array().into_iter().flatten().filter_map(|d| d["name"].as_str()).collect();
            assert!(names.len() > 40, "{has_ui}: only {} tools", names.len());
            let missing: Vec<&&str> = names.iter().filter(|n| !t.iter().any(|(m, ..)| m == *n)).collect();
            assert!(missing.is_empty(), "has_ui={has_ui}: tools without a table entry: {missing:?}");
        }
        // Every entry has object arguments (or a deliberately wrong type) for both calls.
        assert!(t.iter().all(|(n, g, _): &(&str, Value, Value)| !n.is_empty() && g.is_object()));
    }
}
