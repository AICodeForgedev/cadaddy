//! DWGPROPS: the drawing's properties. Summary (title, subject, author, keywords, comments,
//! hyperlink base, last saved by, revision number) and custom properties are set here; General
//! (name, folder, size) and Statistics (created, modified, total editing time, objects) are facts
//! about the drawing. Saved in DWG/DXF (the DWGPROPS XRECORD) and .cdy; fields show them
//! (`%<\AcVar Title>%`, `%<\AcVar CustomDP.Phase>%`).

use cadaddy_doc::dwgprops::{DwgProps, MAX_TEXT};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// The summary fields, by their JSON names.
const SUMMARY: [&str; 7] = ["title", "subject", "author", "keywords", "comments", "lastSavedBy", "revisionNumber"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("dwgprops", "Drawing Properties...", run)
            .menu(&["File", "Drawing Properties..."])
            .params(
                "{title?, subject?, author?, keywords?, comments?, lastSavedBy?, revisionNumber?, hyperlinkBase?: text; \
                 custom?: {name: value | null (removes it)}} → {summary: {…}, hyperlinkBase, custom: [{name, value}], \
                 general: {name, path?, folder?, size?}, statistics: {created?, modified?, editingMinutes?, objects}}; no parameters: just the properties",
            ),
    ]
}

/// Julian date → "yyyy-mm-dd hh:mm" (UTC as stored).
fn date(julian: f64) -> Option<String> {
    if !julian.is_finite() || julian <= 0.0 {
        return None;
    }
    // Days since 1970-01-01 (Julian day 2440587.5).
    let secs = ((julian - 2_440_587.5) * 86_400.0).round();
    if !(-1e11..1e11).contains(&secs) {
        return None;
    }
    let secs = secs as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days (Howard Hinnant's algorithm, public domain).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Some(format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60))
}

/// The properties as DWGPROPS reports them.
pub fn report(s: &Session) -> Result<Value> {
    let st = s.state()?;
    let d = st.drawing();
    let p = &d.properties;
    let summary: serde_json::Map<String, Value> = SUMMARY.iter().map(|k| ((*k).to_string(), json!(p.field(k).unwrap_or_default()))).collect();
    let var = |n: &str| d.header.get(n).and_then(cadaddy_doc::HVal::as_f64);
    let path = st.path.clone();
    let size = path.as_deref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len());
    let folder = path.as_deref().and_then(|p| std::path::Path::new(p).parent()).map(|f| f.display().to_string());
    let objects = d.model.len() + d.layouts.iter().map(|l| l.entities.len()).sum::<usize>();
    Ok(json!({
        "summary": summary,
        "hyperlinkBase": d.header.str("HYPERLINKBASE", ""),
        "custom": p.custom.iter().map(|(n, v)| json!({ "name": n, "value": v })).collect::<Vec<_>>(),
        "general": { "name": st.title, "path": path, "folder": folder, "size": size },
        "statistics": {
            "created": var("TDCREATE").and_then(date),
            "modified": var("TDUPDATE").and_then(date),
            "editingMinutes": var("TDINDWG").filter(|t| t.is_finite() && *t >= 0.0).map(|t| (t * 1440.0).round()),
            "objects": objects,
        },
    }))
}

fn text(cmd: &str, k: &str, v: &Value) -> Result<String> {
    let t = v.as_str().ok_or_else(|| bad(cmd, format!("`{k}` must be text")))?;
    if t.chars().count() > MAX_TEXT {
        return Err(bad(cmd, format!("`{k}` is longer than {MAX_TEXT} characters")));
    }
    Ok(t.to_string())
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "dwgprops";
    let mut props: DwgProps = s.doc()?.properties.clone();
    let mut changed = false;
    for k in SUMMARY {
        if let Some(v) = p.get(k).filter(|v| !v.is_null()) {
            let t = text(cmd, k, v)?;
            if let Some(f) = props.field_mut(k) {
                *f = t;
                changed = true;
            }
        }
    }
    let base = match p.get("hyperlinkBase").filter(|v| !v.is_null()) {
        Some(v) => Some(text(cmd, "hyperlinkBase", v)?),
        None => None,
    };
    match p.get("custom") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (name, v) in m {
                let value = match v {
                    Value::Null => None,
                    v => Some(text(cmd, name, v)?),
                };
                if !props.set_custom(name, value.as_deref()) {
                    return Err(bad(
                        cmd,
                        format!("custom property `{name}`: a name without `=`, at most {} properties", cadaddy_doc::dwgprops::MAX_CUSTOM),
                    ));
                }
            }
            changed = true;
        }
        Some(_) => return Err(bad(cmd, "`custom` is an object of name: value (null removes it)")),
    }
    if changed || base.is_some() {
        let d = s.doc_mut()?;
        d.properties = props;
        if let Some(b) = base {
            d.header.set_str("HYPERLINKBASE", &b);
        }
    }
    report(s)
}
