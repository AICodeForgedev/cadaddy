//! `sheet.facts`: what a plotted sheet says about itself — its number, title and discipline from
//! the title block, the views on it (number, title, scale, region) and, per view, a census of
//! device/fixture type tags and circuit labels. Reads the open drawing (one imported sheet).

use cadaddy_mepfix::sheet_facts;
use serde_json::{Value, json};

use super::{CommandSpec, bool_or, str_param};
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sheet.facts", "Sheet Facts", run_sheet_facts)
            .params("{census?: bool (default true), view?: view number or part of its title} → {sheet: {number, title, discipline, source} | null, views: [{number?, title, scale_text?, not_to_scale, at, region?, census?: {by_type, circuit_labels, by_panel, untyped_circuit_labels}}], warnings}")
            .noundo(),
    ]
}

fn run_sheet_facts(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let texts = sheet_facts::texts(d);
    let sheet = sheet_facts::identify_sheet(&texts);
    let census = bool_or(p, "census", true);
    let want = str_param(p, "view").map(str::to_ascii_uppercase);
    let mut views = Vec::new();
    for v in sheet_facts::view_titles(d, &texts) {
        if let Some(w) = &want
            && v.number.as_deref().map(str::to_ascii_uppercase).as_deref() != Some(w.as_str())
            && !v.title.to_ascii_uppercase().contains(w.as_str())
        {
            continue;
        }
        let mut j = json!(v);
        if census && let Some(r) = v.region {
            j["census"] = json!(sheet_facts::tag_census(&texts, r));
        }
        views.push(j);
    }
    let mut warnings = Vec::new();
    if texts.is_empty() {
        warnings.push("the sheet has no text: lettering plotted as strokes (SHX) can't be read here");
    } else if sheet.is_none() {
        warnings.push("no title block or sheet number found");
    }
    if want.is_some() && views.is_empty() {
        warnings.push("no view matches `view`");
    }
    Ok(json!({"sheet": sheet, "views": views, "warnings": warnings}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_drawing_reports_no_text() {
        let mut s = Session::new();
        let r = run_sheet_facts(&mut s, &json!({})).expect("runs");
        assert!(r["sheet"].is_null());
        assert_eq!(r["views"].as_array().map(Vec::len), Some(0));
        assert!(r["warnings"][0].as_str().is_some_and(|w| w.contains("no text")));
    }

    #[test]
    fn hostile_params_never_panic() {
        let mut s = Session::new();
        for p in [json!({"view": 7}), json!({"census": "yes"}), json!({"view": ""}), json!(null)] {
            let _ = run_sheet_facts(&mut s, &p);
        }
    }
}
