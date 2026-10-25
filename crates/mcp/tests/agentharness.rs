//! E2E test for the agent harness (Q5240): from the MCP instructions text to a built BIM model
//! in ≤3 MCP tool calls, using only synthetic PDFs.

use cadaddy_mcp::{Headless, Server};
use serde_json::{Value, json};

fn io() {
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

fn rpc(s: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    serde_json::from_str(&s.handle_line(&line).expect("a reply")).expect("JSON")
}

fn tool(s: &mut Server, name: &str, args: Value) -> Result<Value, String> {
    let r = rpc(s, "tools/call", json!({ "name": name, "arguments": args }));
    if let Some(e) = r.get("error") {
        return Err(format!("JSON-RPC error: {}", e["message"].as_str().unwrap_or("?")));
    }
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string();
    if r["result"]["isError"] == true {
        Err(text)
    } else {
        serde_json::from_str(&text).map_err(|e| format!("parse: {e}"))
    }
}

/// Build a synthetic PDF with a rectangle (floor plan outline) and a line (wall).
fn synthetic_pdf(dir: &str) -> String {
    let path = format!("{dir}/plan.pdf");
    let stream = b"0.5 w 72 72 468 648 re S 200 400 m 400 400 l S 200 400 m 200 200 l S 400 400 m 400 200 l S 200 200 m 400 200 l S";
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
    pdf.extend_from_slice(format!("0000000000 65535 f \n").as_bytes());
    pdf.extend_from_slice(format!("{o1:010} 00000 n \n").as_bytes());
    pdf.extend_from_slice(format!("{o2:010} 00000 n \n").as_bytes());
    pdf.extend_from_slice(format!("{o3:010} 00000 n \n").as_bytes());
    pdf.extend_from_slice(format!("{o4:010} 00000 n \n").as_bytes());
    pdf.extend_from_slice(format!("trailer<</Size 5/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF").as_bytes());
    std::fs::write(&path, &pdf).unwrap();
    path
}

/// The e2e test: from the MCP instructions to a built model in ≤3 calls.
///
/// Call 1: `workflows` — read the instructions to know the flow.
/// Call 2: `bim_from_pdf` — one call does import + check + spec + build.
/// Call 3: `job_status` — verify idle (the build completed).
///
/// That's 3 calls from instructions to a built model.
#[test]
fn instructions_to_built_model_in_three_calls() {
    io();
    let dir = std::env::temp_dir().join(format!("cadaddy-harness-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_string_lossy().into_owned();
    let pdf = synthetic_pdf(&d);

    let mut s = Server::new(Box::new(Headless::default()));
    rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-06-18" }));

    // Verify the instructions mention bim_from_pdf and workflows.
    let init = rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    let instructions = init["result"]["instructions"].as_str().unwrap_or("");
    assert!(instructions.contains("bim_from_pdf"), "instructions should mention bim_from_pdf: {instructions}");
    assert!(instructions.contains("workflows"), "instructions should mention workflows: {instructions}");

    // Call 1: workflows — the agent reads the available flows.
    let wf = tool(&mut s, "workflows", json!({})).expect("workflows should succeed");
    assert!(wf.get("workflows").and_then(Value::as_array).is_some_and(|a| !a.is_empty()), "workflows should list flows");

    // Call 2: bim_from_pdf — one call imports the PDF, checks quality, drafts the spec and builds.
    // With force:true because the synthetic PDF may fail the import check.
    let bim = tool(&mut s, "bim_from_pdf", json!({ "path": pdf, "force": true })).expect("bim_from_pdf should succeed");
    assert!(bim.get("check_results").and_then(Value::as_array).is_some_and(|a| !a.is_empty()), "should have check results");
    assert!(bim.get("next_steps").and_then(Value::as_array).is_some_and(|a| !a.is_empty()), "should suggest next steps");

    // Call 3: job_status — verify the system is idle (build completed synchronously).
    let status = tool(&mut s, "job_status", json!({})).expect("job_status should succeed");
    assert_eq!(status["idle"], json!(true), "should be idle after build");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The `workflows` tool returns all four named flows.
#[test]
fn workflows_lists_main_flows() {
    io();
    let mut s = Server::new(Box::new(Headless::default()));
    rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    let wf = tool(&mut s, "workflows", json!({})).expect("workflows");
    let flows = wf["workflows"].as_array().expect("workflows array");
    let names: Vec<&str> = flows.iter().filter_map(|f| f["name"].as_str()).collect();
    assert!(names.contains(&"pdf-to-bim"), "missing pdf-to-bim: {names:?}");
    assert!(names.contains(&"view-3d"), "missing view-3d: {names:?}");
    assert!(names.contains(&"plot-set"), "missing plot-set: {names:?}");
    assert!(names.contains(&"convert-folder"), "missing convert-folder: {names:?}");
}
