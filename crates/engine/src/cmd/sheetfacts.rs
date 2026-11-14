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
            .params("{census?: bool (default true), view?: view number or part of its title} → {sheet: {number, title, discipline, source} | null, views: [{number?, title, scale_text?, not_to_scale, at, region?, census?: {by_type, circuit_labels, by_panel, untyped_circuit_labels}}], plan_view: {title, rect, scale_text?} | null (the floor plan's view a job builds from when the sheet prints others beside it; null: the sheet is read whole), warnings}")
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
    let titles = sheet_facts::view_titles(d, &texts);
    let plan_view = plan_view(d, &titles);
    for v in titles {
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
    Ok(json!({"sheet": sheet, "views": views, "plan_view": plan_view, "warnings": warnings}))
}

/// The floor plan's view on a sheet that prints others beside it, as the job reads it
/// ([`cadaddy_mepfix::sheet_reader::floor_plan_view_in`] inside the sheet's border, off its title
/// block): its title, rectangle and the scale under its title. `null` when the sheet is read whole.
fn plan_view(d: &cadaddy_doc::Drawing, titles: &[sheet_facts::ViewTitle]) -> Value {
    let b = d.extents(&cadaddy_doc::Space::Model);
    if b.is_empty() || !(b.min.is_finite() && b.max.is_finite()) {
        return Value::Null;
    }
    let rect = [b.min.x, b.min.y, b.max.x, b.max.y];
    let area = super::guided::sheet_frames(d, &[rect]).into_iter().next().filter(|f| f.border || f.title_block.is_some()).map(|f| f.area);
    let Some((v, r)) = cadaddy_mepfix::sheet_reader::floor_plan_view_in(d, area) else { return Value::Null };
    let scale = titles.iter().find(|t| t.title == v.title).and_then(|t| t.scale_text.clone());
    json!({"title": v.title, "rect": r, "scale_text": scale})
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
        assert!(r["plan_view"].is_null());
    }

    fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
        use cadaddy_doc::{Common, EntityKind, Line, Space, geom::Vec3};
        let l = Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) };
        s.doc_mut().expect("doc").add(&Space::Model, Common::default(), EntityKind::Line(l)).expect("line");
    }

    fn text(s: &mut Session, at: [f64; 2], value: &str, height: f64) {
        use cadaddy_doc::{Common, EntityKind, HAlign, Space, Text, VAlign, geom::Vec3};
        let t = Text {
            insert: Vec3::new(at[0], at[1], 0.0),
            align_pt: None,
            height,
            value: value.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        };
        s.doc_mut().expect("doc").add(&Space::Model, Common::default(), EntityKind::Text(t)).expect("text");
    }

    /// A view of crossing lines (300 square) at (x, y), titled under it over its scale.
    fn view(s: &mut Session, x: f64, y: f64, title: &str, scale: &str) {
        for i in 0..6 {
            let at = x + f64::from(i) * 60.0;
            line(s, [at, y], [at, y + 300.0]);
            line(s, [x, y + f64::from(i) * 60.0], [x + 300.0, y + f64::from(i) * 60.0]);
        }
        text(s, [x, y - 20.0], title, 8.0);
        text(s, [x, y - 32.0], scale, 4.0);
    }

    #[test]
    fn a_floor_plan_beside_details_and_a_schedule_is_reported_as_the_plan_view() {
        let mut s = Session::new();
        for w in [[0.0, 0.0], [2016.0, 0.0], [2016.0, 1440.0], [0.0, 1440.0], [0.0, 0.0]].windows(2) {
            if let [a, b] = w {
                line(&mut s, *a, *b);
            }
        }
        view(&mut s, 100.0, 100.0, "FLOOR PLAN", "SCALE: 1/4\" = 1'-0\"");
        view(&mut s, 700.0, 100.0, "WALL SECTION", "SCALE: 3/4\" = 1'-0\"");
        // A door schedule: a ruled table of 8 rows and 6 columns.
        for r in 0..=8 {
            line(&mut s, [100.0, 900.0 + f64::from(r) * 14.0], [460.0, 900.0 + f64::from(r) * 14.0]);
        }
        for c in 0..=6 {
            line(&mut s, [100.0 + f64::from(c) * 60.0, 900.0], [100.0 + f64::from(c) * 60.0, 1012.0]);
        }
        text(&mut s, [100.0, 1020.0], "DOOR SCHEDULE", 8.0);
        let r = run_sheet_facts(&mut s, &json!({"census": false})).expect("runs");
        let pv = &r["plan_view"];
        assert_eq!(pv["title"], "FLOOR PLAN", "{r}");
        assert_eq!(pv["scale_text"], "1/4\" = 1'-0\"", "the scale under its own title");
        let rect: Vec<f64> = pv["rect"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        assert!(matches!(rect.as_slice(), [x0, y0, x1, y1] if *x0 >= 90.0 && *y0 >= 90.0 && *x1 <= 420.0 && *y1 <= 420.0), "{rect:?}");
    }

    #[test]
    fn hostile_params_never_panic() {
        let mut s = Session::new();
        for p in [json!({"view": 7}), json!({"census": "yes"}), json!({"view": ""}), json!(null)] {
            let _ = run_sheet_facts(&mut s, &p);
        }
    }
}
