//! End to end: an Excel data link. A table is exported to a workbook (TABLEEXPORT .xlsx), a second
//! table is linked to it (DATALINK) and shows its values; the workbook changes (the first table
//! edited and exported again) and DATALINKUPDATE brings the new value into the linked table.

mod e2e_common;

use e2e_common::{E2e, folder};
use serde_json::{Value, json};

/// A table's cell texts, row by row, as `entities` reports the table.
fn cells(e: &mut E2e, handle: &str) -> Vec<Vec<String>> {
    let r = e.exec("entities", json!({ "type": "Table" }));
    let t = r["entities"].as_array().and_then(|a| a.iter().find(|t| t["handle"] == handle)).cloned().unwrap_or(Value::Null);
    let rows = t["geometry"]["cells"].as_array().cloned().unwrap_or_default();
    rows.iter()
        .map(|r| {
            r.as_array()
                .into_iter()
                .flatten()
                .map(|c| c.as_str().map_or_else(|| c["text"].as_str().unwrap_or_default().to_string(), str::to_string))
                .collect()
        })
        .collect()
}

#[test]
fn a_linked_table_follows_its_workbook() {
    let mut e = E2e::new();
    let dir = folder("xlsx");
    let book = dir.join("Project A schedule.xlsx");
    let src = e.exec("table", json!({ "at": [0, 0], "rows": 2, "cols": 2, "cells": [["Door", "4"], ["Window", "7"]] }))["handle"]
        .as_str()
        .unwrap()
        .to_string();
    let x = e.exec("tableexport", json!({ "handle": src, "path": book }));
    assert_eq!(x["rows"], 2, "{x}");
    assert!(std::fs::read(&book).unwrap().starts_with(b"PK"), "an xlsx (zip) workbook");

    let linked = e.exec("table", json!({ "at": [0, -50], "rows": 2, "cols": 2 }))["handle"].as_str().unwrap().to_string();
    e.exec("datalink", json!({ "handle": linked, "path": book, "startRow": 0 }));
    let shown = cells(&mut e, &linked);
    assert!(
        shown.iter().flatten().any(|c| c == "Window") && shown.iter().flatten().any(|c| c == "7"),
        "the linked table shows the workbook: {shown:?}"
    );

    // The workbook changes; updating the link brings the new value in.
    e.exec("table.set", json!({ "handle": src, "row": 1, "col": 1, "text": "9" }));
    e.exec("tableexport", json!({ "handle": src, "path": book }));
    let u = e.exec("datalinkupdate", json!({ "handles": [linked] }));
    assert!(u["updated"].as_u64().unwrap_or(0) >= 1 || u["updated"].as_array().is_some_and(|a| !a.is_empty()), "{u}");
    let shown = cells(&mut e, &linked);
    assert!(shown.iter().flatten().any(|c| c == "9") && !shown.iter().flatten().any(|c| c == "7"), "{shown:?}");
    assert!(e.inked(400, 400) > 0, "the tables render");
    let _ = std::fs::remove_dir_all(&dir);
}
