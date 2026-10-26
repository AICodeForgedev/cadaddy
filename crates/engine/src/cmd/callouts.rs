//! Callouts command: list cross-reference callouts from the project's imported print set.
//!
//! - `callouts.list {level?, kind?, status?}`: the callout list, paged;

use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("callouts.list", "List Callouts", run_list)
            .params(
                "{level?: name, kind?: section_mark|elevation_tag|detail_bubble|enlarged_plan|match_line|key_plan, status?: resolved|broken, page?: number, page_size?: number} → {items: [{on_sheet, kind, detail, target, status, reason}], total, resolved, broken}: cross-reference callouts from the imported print set (section marks, elevation tags, detail bubbles, enlarged-plan boundaries, match lines, key plans) and whether they resolve to real targets",
            )
            .noundo(),
    ]
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    // Check if a print set project is loaded by looking for DXF files in the drawing's
    // embedded store (where printset.open / bim_job import places the classified sheets).
    let doc = s.doc()?;
    let dxf_files: Vec<(String, Vec<u8>)> =
        doc.embedded.iter().filter(|(name, _)| name.ends_with(".dxf")).map(|(name, bytes)| (name.clone(), (**bytes).clone())).collect();

    let page = p.get("page").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
    let page_size = p.get("page_size").and_then(Value::as_u64).unwrap_or(50).clamp(1, 200) as usize;
    if dxf_files.is_empty() {
        // Nothing to read yet: an empty answer that says what to do (an agent's first look is
        // not an error).
        return Ok(json!({
            "items": [],
            "total": 0,
            "resolved": 0,
            "broken": 0,
            "page": page,
            "page_size": page_size,
            "hint": "No imported print set found. Import a set of DXF/PDF sheets first (bim_job import or printset.open) so the callout reader has drawings to scan.",
        }));
    }

    let sheet_numbers: Vec<String> = dxf_files.iter().map(|(name, _)| name.trim_end_matches(".dxf").to_string()).collect();

    let (resolved, broken, _errors) = cadaddy_mepfix::callout_reader::resolve_callouts(&dxf_files, &sheet_numbers);

    let summary = cadaddy_mepfix::callout_reader::callout_summary(&resolved, &broken);

    // Apply filters.
    let kind_filter = str_param(p, "kind");
    let status_filter = str_param(p, "status");
    let level_filter = str_param(p, "level");
    let filtered: Vec<_> = summary
        .iter()
        .filter(|s| kind_filter.is_none_or(|k| s.kind == k))
        .filter(|s| status_filter.is_none_or(|st| s.status == st))
        .filter(|s| {
            level_filter.is_none_or(|l| {
                // Match sheets that contain the level number.
                s.on_sheet.contains(l)
            })
        })
        .collect();

    let total = filtered.len();
    let resolved_count = filtered.iter().filter(|s| s.status == "resolved").count();
    let broken_count = filtered.iter().filter(|s| s.status == "broken").count();

    let start = (page - 1).saturating_mul(page_size);
    let items: Vec<Value> = filtered
        .iter()
        .skip(start)
        .take(page_size)
        .map(|s| {
            json!({
                "on_sheet": s.on_sheet,
                "kind": s.kind,
                "detail": s.detail,
                "target": s.target,
                "status": s.status,
                "reason": s.reason,
            })
        })
        .collect();

    Ok(json!({
        "items": items,
        "total": total,
        "resolved": resolved_count,
        "broken": broken_count,
        "page": page,
        "page_size": page_size,
    }))
}
