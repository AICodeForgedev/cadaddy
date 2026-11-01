//! The clash report as PDF pages (`model.clashes.report`): every clash, grouped (the Clashes
//! palette's groups, else by level and kind), with its id, kind, level, systems, status, trade
//! and notes, through the plotter like the other PDF exports.

use super::*;
use crate::{Result, Session};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The report's sheet: Letter landscape (inches), the text height, the row pitch and the
/// columns' widths (ID, Kind, Level, Systems, Status, Trade, Notes, Clash).
const PAGE: [f64; 2] = [11.0, 8.5];
const TEXT: f64 = 0.08;
const ROW: f64 = 0.16;
const COLS: [f64; 8] = [0.7, 1.15, 0.7, 1.9, 0.7, 1.0, 2.6, 1.7];
/// The Notes column (its index and width).
const NOTES: usize = 6;
const NOTES_W: f64 = 2.6;
const HEADER: [&str; 8] = ["ID", "Kind", "Level", "Systems", "Status", "Trade", "Notes", "Clash"];
/// The longest notes are wrapped onto this many lines, the rest cut (and listed in `cut`).
const MAX_NOTE_LINES: usize = 6;
/// The most clashes a report takes (the detector's own cap).
const MAX_CLASHES: usize = 5_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("model.clashes.report", "Clash Report", run_report)
            .params("{force?: bool (replace an existing file; otherwise it is refused), path: .pdf, clashes?: [{id, kind, level, systems, message, status, trade, notes, group?}] (default: model.clashes now, all new), tolerance_in?} → {path, clashes, groups, pages, cut}")
            .noundo(),
    ]
}

fn sv(c: &Value, k: &str) -> String {
    c.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn kind_label(kind: &str) -> String {
    match kind {
        "below_ceiling" => "Below ceiling".into(),
        "into_structure" => "Into structure".into(),
        "crossing" => "Crossing".into(),
        "column" => "Through column".into(),
        "reference" => "Reference model".into(),
        k => k.to_string(),
    }
}

/// Text cut to `fits` characters with "…" when it is longer.
fn fit(t: &str, fits: usize) -> (String, bool) {
    if t.chars().count() > fits {
        (format!("{}…", t.chars().take(fits.saturating_sub(1)).collect::<String>()), true)
    } else {
        (t.to_string(), false)
    }
}

/// Words wrapped to lines of at most `fits` characters (a longer word is split).
fn wrap(t: &str, fits: usize) -> Vec<String> {
    let fits = fits.max(2);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in t.split_whitespace() {
        let mut w: Vec<char> = word.chars().collect();
        while w.len() > fits {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            lines.push(w.drain(..fits).collect());
        }
        let w: String = w.into_iter().collect();
        if cur.is_empty() {
            cur = w;
        } else if cur.chars().count() + 1 + w.chars().count() <= fits {
            cur.push(' ');
            cur.push_str(&w);
        } else {
            lines.push(std::mem::replace(&mut cur, w));
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// The clashes the report lists: given, else detected now (ids `C0001`… in order, all new).
fn report_clashes(s: &mut Session, p: &Value) -> Result<Vec<Value>> {
    let mut list: Vec<Value> = match p.get("clashes") {
        Some(Value::Array(a)) => a.iter().take(MAX_CLASHES).cloned().collect(),
        Some(_) => return Err(bad("model.clashes.report", "clashes is a list of clashes")),
        None => {
            let r = super::mep::run_clashes(s, &json!({"tolerance_in": p.get("tolerance_in").cloned().unwrap_or(json!(0))}))?;
            r.get("clashes").and_then(Value::as_array).cloned().unwrap_or_default()
        }
    };
    for (i, c) in list.iter_mut().enumerate() {
        if let Some(o) = c.as_object_mut()
            && o.get("id").and_then(Value::as_str).is_none_or(str::is_empty)
        {
            o.insert("id".into(), json!(format!("C{:04}", i + 1)));
        }
    }
    Ok(list)
}

/// One printed line of the report.
enum Line {
    Group(String),
    Row([String; 8]),
}

/// The report as a drawing of its pages (one under another, a page window each), the plot
/// parameters, and what was cut to fit its column (every cut listed whole).
pub struct Report {
    pub drawing: cadaddy_doc::Drawing,
    pub plot: Value,
    pub cut: Vec<String>,
    pub groups: usize,
    pub clashes: usize,
}

pub fn report_pdf(s: &mut Session, p: &Value) -> Result<Report> {
    let clashes = report_clashes(s, p)?;
    // Groups in the order they first appear; clashes in their order inside each.
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for c in &clashes {
        let g = c.get("group").and_then(Value::as_str).filter(|g| !g.trim().is_empty()).map(str::to_string).unwrap_or_else(|| {
            let level = sv(c, "level");
            let level = if level.is_empty() { c.pointer("/reference/level").and_then(Value::as_str).unwrap_or("").to_string() } else { level };
            format!("Level {} — {}", if level.is_empty() { "?" } else { &level }, kind_label(&sv(c, "kind")))
        });
        if !groups.contains_key(&g) {
            order.push(g.clone());
        }
        groups.entry(g).or_default().push(c);
    }
    let fits = |w: f64| ((w / (TEXT * 0.85)).floor() as usize).max(2);
    let mut cut: Vec<String> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    for g in &order {
        let Some(members) = groups.get(g) else { continue };
        lines.push(Line::Group(format!("{g} ({})", members.len())));
        for c in members {
            let id = sv(c, "id");
            let systems = c.get("systems").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ");
            let status = {
                let st = sv(c, "status");
                if st.is_empty() { "New".to_string() } else { st }
            };
            let level = {
                let l = sv(c, "level");
                if l.is_empty() { c.pointer("/reference/level").and_then(Value::as_str).unwrap_or("").to_string() } else { l }
            };
            let whole = [id.clone(), kind_label(&sv(c, "kind")), level, systems, status, sv(c, "trade"), String::new(), sv(c, "message")];
            let mut row: [String; 8] = Default::default();
            for (k, (cell, w)) in whole.iter().zip(COLS).enumerate() {
                let (shown, was_cut) = fit(cell, fits(w));
                if was_cut {
                    cut.push(format!("{id}, {}: {cell}", HEADER.get(k).copied().unwrap_or("")));
                }
                if let Some(r) = row.get_mut(k) {
                    *r = shown;
                }
            }
            // Notes wrap onto the lines below the clash's.
            let notes = sv(c, "notes");
            let mut wrapped = wrap(&notes, fits(NOTES_W));
            if wrapped.len() > MAX_NOTE_LINES {
                wrapped.truncate(MAX_NOTE_LINES);
                if let Some(last) = wrapped.last_mut() {
                    last.push('…');
                }
                cut.push(format!("{id}, Notes: {notes}"));
            }
            let mut wrapped = wrapped.into_iter();
            if let Some(n) = row.get_mut(NOTES) {
                *n = wrapped.next().unwrap_or_default();
            }
            lines.push(Line::Row(row));
            for more in wrapped {
                let mut cont: [String; 8] = Default::default();
                if let Some(n) = cont.get_mut(NOTES) {
                    *n = more;
                }
                lines.push(Line::Row(cont));
            }
        }
    }
    let mut d = cadaddy_doc::Drawing::new_imperial();
    let text = |d: &mut cadaddy_doc::Drawing, x: f64, y: f64, h: f64, v: String| {
        if v.is_empty() {
            return;
        }
        let t = cadaddy_doc::Text {
            insert: cadaddy_geom::Vec3::new(x, y, 0.0),
            align_pt: None,
            height: h,
            value: v,
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        };
        let _ = d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common::default(), cadaddy_doc::EntityKind::Text(t));
    };
    // Lines a page holds under the title and the header.
    let per_page = (((PAGE[1] - 1.4) / ROW).floor() as usize).max(1);
    let pages = lines.len().div_ceil(per_page).max(1);
    let mut windows = Vec::new();
    let (new, active, resolved) = clashes.iter().fold((0, 0, 0), |(n, a, r), c| match sv(c, "status").to_ascii_lowercase().as_str() {
        "active" => (n, a + 1, r),
        "resolved" => (n, a, r + 1),
        _ => (n + 1, a, r),
    });
    for page in 0..pages {
        let y0 = -(page as f64) * (PAGE[1] + 1.0);
        windows.push(json!({"window": [[0.0, y0], [PAGE[0], y0 + PAGE[1]]], "paper": "Letter", "landscape": true}));
        text(
            &mut d,
            0.25,
            y0 + 8.0,
            0.18,
            format!("CLASH REPORT   {} clashes: {new} new, {active} active, {resolved} resolved   page {} of {pages}", clashes.len(), page + 1),
        );
        let mut x = 0.25;
        for (h, w) in HEADER.iter().zip(COLS) {
            text(&mut d, x, y0 + 7.6, TEXT * 1.15, (*h).to_string());
            x += w;
        }
        for (i, line) in lines.iter().skip(page * per_page).take(per_page).enumerate() {
            let y = y0 + 7.6 - (i + 1) as f64 * ROW;
            match line {
                Line::Group(g) => text(&mut d, 0.25, y, TEXT * 1.25, g.clone()),
                Line::Row(cells) => {
                    let mut x = 0.25;
                    for (cell, w) in cells.iter().zip(COLS) {
                        text(&mut d, x, y, TEXT, cell.clone());
                        x += w;
                    }
                }
            }
        }
    }
    let plot = json!({"pages": windows, "margins": [0.0, 0.0, 0.0, 0.0], "lineweights": false, "stamp": false});
    Ok(Report { drawing: d, plot, cut, groups: order.len(), clashes: clashes.len() })
}

fn run_report(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad("model.clashes.report", "path required (a .pdf file)"))?
        .to_string();
    if !path.to_ascii_lowercase().ends_with(".pdf") {
        return Err(bad("model.clashes.report", "the report is a .pdf file"));
    }
    super::exchange::guard_output(s, "model.clashes.report", p, &path)?;
    let plot = super::file::io()
        .and_then(|h| h.plot)
        .ok_or_else(|| bad("model.clashes.report", "PDF export isn't available in this build (export the clashes as CSV or BCF)"))?;
    let r = report_pdf(s, p)?;
    let pages = r.plot.get("pages").and_then(Value::as_array).map_or(0, Vec::len);
    let bytes = plot(&r.drawing, &cadaddy_doc::Space::Model, &r.plot).map_err(|e| bad("model.clashes.report", e))?;
    #[cfg(not(target_arch = "wasm32"))]
    crate::writefile::write(&path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
    #[cfg(target_arch = "wasm32")]
    let _ = &bytes;
    Ok(json!({"path": path, "clashes": r.clashes, "groups": r.groups, "pages": pages, "cut": r.cut}))
}
