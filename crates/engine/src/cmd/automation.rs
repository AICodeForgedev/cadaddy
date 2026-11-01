//! The Automation tab's commands (Q6117).
//!
//! - `automation.takeoff {format?, path?, force?}`: the quantities the drawing holds, as one
//!   table. With a built BIM model it is the model's takeoff (`bim.takeoff`, per level, with each
//!   line's trade and type); without one, the 2D takeoff by layer (`takeoff`). The table can be
//!   written as CSV or XLSX through the shared output guard (never over the open drawing, never
//!   over another file without `force`). It reports what the model holds; it does not judge it.

use serde_json::{Value, json};

use super::exchange;
use super::*;
use crate::{Result, Session};

pub const CMD_TAKEOFF: &str = "automation.takeoff";
/// Rows the answer carries (the file gets them all).
const ROWS_SHOWN: usize = 2_000;
/// Rows a table holds at most (hostile drawings).
const MAX_ROWS: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new(CMD_TAKEOFF, "Automation Takeoff", run_takeoff).params(
        "{format?: csv|xlsx (the file's kind; default: the path's extension, else csv), path?: file to write (.csv or .xlsx; added when the name has none), force?: bool (replace an existing file; otherwise it is refused)} → {source: model|drawing, command: bim.takeoff|takeoff, title, columns, rows: [[cell]], total_rows, shown, totals, levels?, by_trade?, note?, path?, format?}: the quantities the drawing holds as one table — with a built BIM model the model's takeoff per level, trade and type (bim.takeoff), otherwise the 2D takeoff by layer (takeoff)",
    )]
}

/// The table: its columns and rows (strings), with what it came from.
struct Table {
    source: &'static str,
    command: &'static str,
    title: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    extra: serde_json::Map<String, Value>,
}

fn num(v: &Value) -> String {
    match v.as_f64() {
        Some(x) if x.is_finite() => {
            let r = (x * 1000.0).round() / 1000.0;
            if r == r.trunc() && r.abs() < 1e15 { format!("{r:.0}") } else { format!("{r}") }
        }
        _ => v.as_str().map(str::to_string).unwrap_or_default(),
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(_) => num(v),
        other => other.to_string(),
    }
}

/// Whether the drawing holds built BIM elements.
fn has_model(s: &Session) -> Result<bool> {
    Ok(!crate::cmd::bimwork::elements(s.doc()?).is_empty())
}

fn model_table(s: &mut Session) -> Result<Table> {
    let r = s.execute("bim.takeoff", &json!({"group_by": "level", "keep": false}))?;
    let columns: Vec<String> =
        ["Level", "Trade", "System", "Category", "Type", "Size", "Unit", "Qty", "Elements"].iter().map(|c| (*c).to_string()).collect();
    let mut by_trade: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let rows: Vec<Vec<String>> = r["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_ROWS)
        .map(|l| {
            let trade = text(&l["trade"]);
            *by_trade.entry(if trade.is_empty() { "other".into() } else { trade.clone() }).or_default() += 1;
            vec![
                text(&l["level"]),
                trade,
                text(&l["system"]),
                text(&l["category"]),
                text(&l["item"]),
                text(&l["size"]),
                text(&l["unit"]),
                num(&l["qty"]),
                num(&l["element_count"]),
            ]
        })
        .collect();
    let mut extra = serde_json::Map::new();
    extra.insert("totals".into(), r["totals"].clone());
    extra.insert("levels".into(), r["levels"].clone());
    extra.insert("by_trade".into(), json!(by_trade));
    Ok(Table { source: "model", command: "bim.takeoff", title: "Model Takeoff".into(), columns, rows, extra })
}

fn drawing_table(s: &mut Session) -> Result<Table> {
    let r = s.execute("takeoff", &json!({}))?;
    let columns: Vec<String> = r["columns"].as_array().into_iter().flatten().map(text).collect();
    let keys = ["layer", "objects", "drawing", "real", "withWaste"];
    let width = columns.len().min(keys.len());
    let cells = |row: &Value| -> Vec<String> { keys.iter().take(width).map(|k| text(&row[*k])).collect() };
    let mut rows: Vec<Vec<String>> = r["rows"].as_array().into_iter().flatten().take(MAX_ROWS).map(cells).collect();
    let total = &r["total"];
    let mut last: Vec<String> = vec!["Total".into()];
    last.extend(["objects", "drawing", "real", "withWaste"].iter().take(width.saturating_sub(1)).map(|k| text(&total[*k])));
    rows.push(last);
    let mut extra = serde_json::Map::new();
    extra.insert("totals".into(), total.clone());
    extra.insert("unit".into(), r["unit"].clone());
    if r["truncated"] == true {
        extra.insert("truncated".into(), json!(true));
    }
    Ok(Table { source: "drawing", command: "takeoff", title: "Quantity Takeoff".into(), columns, rows, extra })
}

fn csv_cell(v: &str) -> String {
    if v.contains([',', '"', '\n', '\r']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() }
}

/// The file to write and its kind, from `path` and `format`.
fn target(p: &Value) -> Result<Option<(String, &'static str)>> {
    let format = match str_param(p, "format").map(|f| f.trim().trim_start_matches('.').to_ascii_lowercase()) {
        None => None,
        Some(f) if f.is_empty() => None,
        Some(f) if f == "csv" => Some("csv"),
        Some(f) if f == "xlsx" => Some("xlsx"),
        Some(_) => return Err(bad(CMD_TAKEOFF, "`format` is csv or xlsx")),
    };
    let Some(path) = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()) else {
        return match format {
            Some(_) => Err(bad(CMD_TAKEOFF, "`path` is required with `format`: the file to write")),
            None => Ok(None),
        };
    };
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    match (ext.as_deref(), format) {
        (Some("csv"), None | Some("csv")) => Ok(Some((path.to_string(), "csv"))),
        (Some("xlsx"), None | Some("xlsx")) => Ok(Some((path.to_string(), "xlsx"))),
        (Some(e @ ("csv" | "xlsx")), Some(f)) => Err(bad(CMD_TAKEOFF, format!("the path ends in .{e} but `format` is {f}"))),
        (None, f) => {
            let f = f.unwrap_or("csv");
            Ok(Some((format!("{path}.{f}"), f)))
        }
        (Some(e), _) => Err(bad(CMD_TAKEOFF, format!("a takeoff is written as .csv or .xlsx, not .{e}"))),
    }
}

fn run_takeoff(s: &mut Session, p: &Value) -> Result<Value> {
    // The path as given first (a drawing's name says so), then the file actually written.
    exchange::guard_params(s, CMD_TAKEOFF, p, &["path"])?;
    let file = target(p)?;
    if let Some((path, _)) = &file {
        exchange::guard_output(s, CMD_TAKEOFF, p, path)?;
    }
    let mut note: Option<String> = None;
    let table = if has_model(s)? {
        match model_table(s) {
            Ok(t) => t,
            Err(e) => {
                note = Some(format!("the model's takeoff failed ({e}); this is the 2D takeoff of the drawing"));
                drawing_table(s)?
            }
        }
    } else {
        note = Some("no BIM model is built in this drawing: this is the 2D takeoff by layer (Plans to Model builds the model)".into());
        drawing_table(s)?
    };
    let mut out = json!({
        "source": table.source,
        "command": table.command,
        "title": table.title,
        "columns": table.columns,
        "total_rows": table.rows.len(),
        "shown": table.rows.len().min(ROWS_SHOWN),
        "rows": table.rows.iter().take(ROWS_SHOWN).collect::<Vec<_>>(),
    });
    for (k, v) in table.extra {
        out[k] = v;
    }
    if let Some(n) = note {
        out["note"] = json!(n);
    }
    if let Some((path, format)) = file {
        let mut all: Vec<Vec<String>> = Vec::with_capacity(table.rows.len() + 1);
        all.push(table.columns.clone());
        all.extend(table.rows.iter().cloned());
        let bytes = if format == "xlsx" {
            crate::cmd::table::xlsx::write(&all, "Takeoff").map_err(|e| bad(CMD_TAKEOFF, e))?
        } else {
            all.iter().map(|r| r.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",") + "\n").collect::<String>().into_bytes()
        };
        exchange::write_file(CMD_TAKEOFF, &path, &bytes)?;
        out["path"] = json!(path);
        out["format"] = json!(format);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_and_its_kind_come_from_path_and_format() {
        let t = |p: Value| target(&p).map_err(|e| e.to_string());
        assert_eq!(t(json!({})).unwrap(), None);
        assert_eq!(t(json!({"path": "Takeoff A.csv"})).unwrap(), Some(("Takeoff A.csv".into(), "csv")));
        assert_eq!(t(json!({"path": "Takeoff A.XLSX"})).unwrap(), Some(("Takeoff A.XLSX".into(), "xlsx")));
        assert_eq!(t(json!({"path": "Takeoff A", "format": "xlsx"})).unwrap(), Some(("Takeoff A.xlsx".into(), "xlsx")));
        assert_eq!(t(json!({"path": "Takeoff A"})).unwrap(), Some(("Takeoff A.csv".into(), "csv")));
        assert!(t(json!({"path": "Takeoff A.csv", "format": "xlsx"})).unwrap_err().contains("format"));
        assert!(t(json!({"path": "Takeoff A.pdf"})).unwrap_err().contains(".csv or .xlsx"));
        assert!(t(json!({"format": "ods"})).unwrap_err().contains("csv or xlsx"));
        assert!(t(json!({"format": "csv"})).unwrap_err().contains("`path` is required"));
    }

    #[test]
    fn numbers_print_plainly() {
        assert_eq!(num(&json!(3.0)), "3");
        assert_eq!(num(&json!(12.34567)), "12.346");
        assert_eq!(num(&json!(f64::NAN)), "");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
