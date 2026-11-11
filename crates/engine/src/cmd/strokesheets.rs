//! Sheets lettered in strokes (Q6516): plotted vector text with no searchable text under it, read
//! back by the PDF import's stroke OCR (table cells entry by entry). The job reports how each
//! sheet's lettering read (classify: the share of it drawn as strokes, the words read, how sure),
//! and a sheet that carries tables and reads unsure, or whose tables no reader could read, is
//! asked for them with its picture: a blocking question, answered with the tables' rows as JSON
//! (the readers' own shape), stored with the source [`ANSWER_SOURCE`]. A table read with unsure
//! entries (the import marks them: [`cadaddy_io::pdf_read::OCR_UNSURE_STYLE`]) is asked once,
//! with its picture, never cell by cell.

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_mepfix::printset::Lettering;
use cadaddy_walk3d::{FixtureType, PanelSchedule};
use serde_json::{Value, json};

use super::guided::{GuidedJob, Question, QuestionEvidence, QuestionKind, StageName, progress_line};

/// What a question about a sheet's tables begins with (the sheet and its title follow): its answer
/// is learned as the sheet's tables.
pub(crate) const TABLE_QUESTION: &str = "Tables to read: ";
/// The source the tables an answer gives are stored with.
pub(crate) const ANSWER_SOURCE: &str = "answer, from the sheet image";
/// The learned rule an answer's tables are kept under: `tables:<SHEET>`.
const RULE: &str = "tables:";

/// Questions about tables asked in one build, at most.
const MAX_TABLE_QUESTIONS: usize = 24;
/// Unsure readings listed in one question, at most.
const MAX_UNSURE_LISTED: usize = 40;
/// Tables an answer gives that are taken, at most (hostile answers): panels, fixture types,
/// circuits of one panel.
const MAX_ANSWER_PANELS: usize = 200;
const MAX_ANSWER_FIXTURES: usize = 500;
const MAX_ANSWER_CIRCUITS: usize = 2_000;
/// A table title is no longer than this (a note naming a schedule is).
const MAX_TITLE_CHARS: usize = 60;
/// An unsure reading belongs to the table whose title is above it within this many of the
/// title's heights across.
const TABLE_REACH_H: f64 = 80.0;
/// The picture of a sheet or a table, pixels on its long side.
const CROP_PX: u32 = 1600;

/// How a classified sheet's lettering read, when its page was read.
fn lettering_of(sheet: &Value) -> Option<Lettering> {
    serde_json::from_value(sheet.get("lettering")?.clone()).ok()
}

fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Whether a classified sheet is titled or typed for tables: a schedule or legend sheet, or one
/// its title says holds schedules.
fn titled_tables(sheet: &Value) -> bool {
    matches!(text(sheet, "type"), "schedule" | "legend") || text(sheet, "title").to_ascii_uppercase().contains("SCHEDULE")
}

/// Whether a classified sheet carries tables: [`titled_tables`], or one whose page holds ruled
/// tables with words in them (a plan with its fixture schedule beside it, a title the OCR
/// misread).
fn carries_tables(sheet: &Value, lettering: &Lettering) -> bool {
    titled_tables(sheet) || lettering.cells >= MIN_TABLE_CELLS
}

/// Whether a sheet's tables are the electrical ones the job reads (panel schedules, the
/// luminaire schedule): an electrical sheet (or a combined trade's) titled for tables. One of
/// those that yields none is asked for them however sure its lettering read.
fn electrical_tables(sheet: &Value) -> bool {
    super::panelsched::trade_has(text(sheet, "discipline"), "E") && titled_tables(sheet)
}

/// Ruled cells read on a page that make it a page with tables (a schedule's rows, not a title
/// block's few boxes).
const MIN_TABLE_CELLS: usize = 24;

/// The classify stage's report of the sheets lettered in strokes (`classified["lettering"]`): each
/// sheet's share of strokes, the words read off them and how sure, and a finding naming the ones
/// read unsure.
pub(crate) fn lettering_report(job: &mut GuidedJob, classified: &mut Value) {
    let sheets = classified.get("sheets").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut rows: Vec<Value> = Vec::new();
    let mut unsure: Vec<String> = Vec::new();
    let mut in_strokes = 0usize;
    for sh in &sheets {
        let Some(l) = lettering_of(sh) else { continue };
        if l.stroke_share <= 0.0 {
            continue;
        }
        let number = text(sh, "number");
        in_strokes += usize::from(l.in_strokes());
        if l.unsure() {
            unsure.push(format!("{number} ({:.0}% sure)", l.confidence * 100.0));
        }
        rows.push(json!({
            "sheet": number, "title": text(sh, "title"), "type": text(sh, "type"),
            "stroke_share": l.stroke_share, "ocr_share": l.ocr_share, "words": l.words, "confidence": l.confidence,
            "cells": l.cells, "cells_unsure": l.cells_unsure, "in_strokes": l.in_strokes(), "unsure": l.unsure(),
        }));
    }
    if let Some(o) = classified.as_object_mut() {
        o.insert("lettering".into(), json!({"sheets": rows, "in_strokes": in_strokes, "unsure": unsure.len()}));
    }
    if in_strokes > 0 {
        progress_line(&format!(
            "  lettering: {in_strokes} sheet(s) lettered in strokes with no text under them, read by the stroke OCR; {} read unsure{}",
            unsure.len(),
            if unsure.is_empty() { String::new() } else { format!(": {}", unsure.iter().take(12).cloned().collect::<Vec<_>>().join(", ")) }
        ));
    }
    if !unsure.is_empty() {
        job.add_finding(
            StageName::Classify,
            &format!("{} sheet(s) lettered in strokes read unsure: {}", unsure.len(), unsure.iter().take(12).cloned().collect::<Vec<_>>().join(", ")),
            "Their lettering is plotted as lines with no text under it; the stroke OCR read it with low confidence, so titles, tags and table entries there may be misread",
            3,
            vec!["The build asks for the tables these sheets carry, with their pictures (bim_build)".into()],
            None,
            false,
        );
    }
}

/// The tables answers gave, by sheet (`tables:<SHEET>` rules): panel schedules and fixture types
/// in the readers' own shape, each stored with the answer as its source.
pub(crate) fn answered_tables(job: &GuidedJob) -> (Vec<PanelSchedule>, Vec<FixtureType>) {
    let (mut panels, mut fixtures) = (Vec::new(), Vec::new());
    for r in job.rules.iter().filter(|r| r.key.starts_with(RULE)) {
        let sheet = r.key.get(RULE.len()..).unwrap_or("").to_string();
        let Ok(v) = serde_json::from_str::<Value>(&r.value) else { continue };
        let (p, f) = tables_of(&v, &sheet);
        panels.extend(p);
        fixtures.extend(f);
    }
    (panels, fixtures)
}

/// An answer's tables: `{panels?: [...], fixtures?: [...]}`, or a list of panels.
fn tables_of(v: &Value, sheet: &str) -> (Vec<PanelSchedule>, Vec<FixtureType>) {
    let list = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let panels_v = if v.is_array() { v.as_array().cloned().unwrap_or_default() } else { list("panels") };
    let panels = panels_v
        .into_iter()
        .take(MAX_ANSWER_PANELS)
        .filter_map(|p| serde_json::from_value::<PanelSchedule>(p).ok())
        .filter(|p| !p.panel.trim().is_empty())
        .map(|mut p| {
            p.panel = p.panel.trim().to_string();
            p.circuits.truncate(MAX_ANSWER_CIRCUITS);
            p.source = sheet.to_string();
            p.header.insert("source".into(), ANSWER_SOURCE.into());
            p
        })
        .collect();
    let fixtures = list("fixtures")
        .into_iter()
        .take(MAX_ANSWER_FIXTURES)
        .filter_map(|f| serde_json::from_value::<FixtureType>(f).ok())
        .filter(|f| !f.tag.trim().is_empty())
        .map(|mut f| {
            f.tag = f.tag.trim().to_ascii_uppercase();
            f.source = format!("{sheet} ({ANSWER_SOURCE})");
            f
        })
        .collect();
    (panels, fixtures)
}

/// Whether a table question's answer gives tables; what it does.
pub(crate) fn answer_effect(q: &Question, answer: &Value) -> Option<String> {
    if !q.question.starts_with(TABLE_QUESTION) {
        return None;
    }
    let sheet = q.evidence.as_ref().and_then(|e| e.sheet.clone()).unwrap_or_default();
    let (p, f) = tables_of(answer, &sheet);
    Some(if p.is_empty() && f.is_empty() {
        "recorded; no table in the answer: give {\"panels\": [{\"panel\", \"circuits\": [{\"number\", \"description\", \"breaker\", \"poles\"}]}]} or {\"fixtures\": [{\"tag\", \"description\", …}]}".into()
    } else {
        format!("the build takes {} panel schedule(s) and {} fixture type(s) from the answer for {sheet} ({ANSWER_SOURCE})", p.len(), f.len())
    })
}

/// The rule a table question's answer is learned as: (key, value), when it gives tables.
pub(crate) fn learned(q: &Question, answer: &Value) -> Option<(String, String)> {
    if !q.question.starts_with(TABLE_QUESTION) {
        return None;
    }
    let sheet = q.evidence.as_ref().and_then(|e| e.sheet.clone())?;
    let (p, f) = tables_of(answer, &sheet);
    (!p.is_empty() || !f.is_empty()).then(|| (format!("{RULE}{}", sheet.to_ascii_uppercase()), answer.to_string()))
}

/// `read` with the panels answers gave: an answered panel replaces the one read under its tag.
pub(crate) fn with_answered_panels(mut read: Vec<PanelSchedule>, answered: Vec<PanelSchedule>) -> Vec<PanelSchedule> {
    for p in answered {
        read.retain(|r| !r.panel.eq_ignore_ascii_case(&p.panel));
        read.push(p);
    }
    read
}

/// `read` with the fixture types answers gave: an answered type replaces the one read under its tag.
pub(crate) fn with_answered_fixtures(mut read: Vec<FixtureType>, answered: Vec<FixtureType>) -> Vec<FixtureType> {
    for f in answered {
        read.retain(|r| !r.tag.eq_ignore_ascii_case(&f.tag));
        read.push(f);
    }
    read
}

/// A text of a sheet's drawing: its value, where, how tall, and whether the OCR read it unsure.
struct SheetText {
    value: String,
    at: Vec2,
    height: f64,
    unsure: bool,
}

fn sheet_texts(d: &Drawing) -> Vec<SheetText> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) if t.rotation.sin().abs() < 0.05 && t.insert.xy().is_finite() && t.height.is_finite() => Some(SheetText {
                value: t.value.trim().to_string(),
                at: t.insert.xy(),
                height: t.height,
                unsure: t.style.eq_ignore_ascii_case(cadaddy_io::pdf_read::OCR_UNSURE_STYLE),
            }),
            _ => None,
        })
        .take(super::gridtable::MAX_SEGMENTS)
        .collect()
}

/// A sheet's tables with unsure entries: each table's title (empty: no title above them), the
/// window round it and them, and the unsure readings.
fn unsure_tables(texts: &[SheetText]) -> Vec<(String, Bounds2, Vec<String>)> {
    let titles: Vec<&SheetText> = texts
        .iter()
        .filter(|t| !t.unsure && t.value.chars().count() <= MAX_TITLE_CHARS && t.value.to_ascii_uppercase().contains("SCHEDULE"))
        .collect();
    let mut out: Vec<(String, Bounds2, Vec<String>)> = Vec::new();
    for u in texts.iter().filter(|t| t.unsure) {
        let title = titles
            .iter()
            .filter(|t| t.at.y > u.at.y && (t.at.x - u.at.x).abs() <= TABLE_REACH_H * t.height.max(1e-9))
            .min_by(|a, b| (a.at.y - u.at.y).total_cmp(&(b.at.y - u.at.y)));
        let name = title.map_or(String::new(), |t| t.value.clone());
        let here = Bounds2::new(u.at, u.at).expand(4.0 * u.height);
        match out.iter_mut().find(|(n, _, _)| *n == name) {
            Some((_, b, vals)) => {
                *b = b.union(&here);
                if vals.len() < MAX_UNSURE_LISTED {
                    vals.push(u.value.clone());
                }
            }
            None => {
                let b = title.map_or(here, |t| here.union(&Bounds2::new(t.at, t.at).expand(2.0 * t.height)));
                out.push((name, b, vec![u.value.clone()]));
            }
        }
    }
    out
}

/// A PNG of `d` in `window` (the whole drawing when `None`), written into `dir` as `name`; its
/// path. `None` when nothing renders or it can't be written.
fn picture(d: &Drawing, window: Option<Bounds2>, dir: &std::path::Path, name: &str) -> Option<String> {
    let window = match window {
        Some(w) => w,
        None => {
            let mut b: Option<Bounds2> = None;
            for e in d.model.iter().take(2_000_000) {
                let eb = cadaddy_doc::entity_bounds(d, e, 0);
                if !eb.is_empty() && eb.min.is_finite() && eb.max.is_finite() {
                    b = Some(b.map_or(eb, |x| x.union(&eb)));
                }
            }
            b?
        }
    };
    let near: Vec<&cadaddy_doc::Entity> =
        d.model.iter().filter(|e| cadaddy_doc::entity_bounds(d, e, 0).intersects(&window)).map(|e| e.as_ref()).take(400_000).collect();
    let list = cadaddy_render::build_entities(d, near, &cadaddy_render::Options::default());
    let (w, h) = (window.width(), window.height());
    let (px_w, px_h) = if w >= h {
        (CROP_PX, ((f64::from(CROP_PX) * h / w.max(1e-9)).round() as u32).clamp(200, CROP_PX))
    } else {
        (((f64::from(CROP_PX) * w / h.max(1e-9)).round() as u32).clamp(200, CROP_PX), CROP_PX)
    };
    let ro = cadaddy_render::raster::RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 1.0, px_per_mm: 0.0, antialias: true };
    let view = cadaddy_render::raster::View::fit(&window, px_w, px_h, 0.02 * w.max(h));
    let png = cadaddy_render::raster::render(&list, &view, &ro)?.encode_png().ok()?;
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(48).collect();
    let path = dir.join(format!("{safe}.png"));
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::create_dir_all(dir).ok()?;
        crate::writefile::write(&path, png).ok()?;
        Some(path.to_string_lossy().into_owned())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (png, path);
        None
    }
}

/// What the answer to a table question looks like.
fn answer_schema() -> Value {
    json!({"type": "object", "properties": {
        "panels": {"type": "array", "items": {"type": "object", "properties": {
            "panel": {"type": "string"}, "header": {"type": "object"},
            "circuits": {"type": "array", "items": {"type": "object", "properties": {
                "number": {"type": "string"}, "description": {"type": "string"}, "breaker": {"type": "string"}, "poles": {"type": "number"},
                "wire": {"type": "string"}, "load_va": {"type": "number"}}}}}}},
        "fixtures": {"type": "array", "items": {"type": "object", "properties": {
            "tag": {"type": "string"}, "description": {"type": "string"}, "manufacturer": {"type": "string"}, "model": {"type": "string"},
            "lamp": {"type": "string"}, "voltage": {"type": "string"}, "installation": {"type": "string"}}}}}})
}

/// The answer that leaves a table question's sheet as read.
const LEAVE: &str = "Leave as read";

fn ask(job: &mut GuidedJob, question: String, why: &str, sheet: &str, crop: Option<String>, values: Value, blocking: bool) {
    if job.stages.iter().flat_map(|st| &st.questions).any(|q| q.question == question) {
        return;
    }
    let qid = job.next_qid(StageName::BimBuild);
    if blocking {
        job.always_ask.insert(qid.clone());
    }
    let q = Question {
        id: qid,
        stage: StageName::BimBuild,
        question,
        why: why.into(),
        choices: vec![LEAVE.into()],
        evidence: Some(QuestionEvidence {
            crop_png_path: crop,
            sheet: Some(sheet.to_string()),
            at: None,
            values: Some(values),
            handles: Vec::new(),
            tiled_crops: None,
            leftovers: Vec::new(),
        }),
        default: Some(LEAVE.into()),
        answer_schema: Some(answer_schema()),
        answer: None,
        answered_by: None,
        answered_at: None,
        kind: QuestionKind::Doubt,
    };
    job.add_question_to_worklist(&q);
    if let Some(st) = job.stage_mut(StageName::BimBuild) {
        st.questions.push(q);
    }
}

/// Ask for the tables of the sheets lettered in strokes that the readers could not read, or read
/// unsure (`pages`: the sheets read for tables, by number; `panels` and `fixtures`: what the
/// readers and earlier answers gave). A sheet that carries tables, reads unsure or yields none,
/// is one blocking question with its picture; on a sheet read sure, each table with unsure
/// entries is one question with its picture. The questions asked go in `result`.
pub(crate) fn table_questions(
    job: &mut GuidedJob,
    pages: &[(String, Drawing)],
    panels: &[PanelSchedule],
    fixtures: &[FixtureType],
    result: &mut Value,
) {
    let sheets: Vec<Value> = job.index.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array).cloned().unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("cadaddy-{}", job.id)).join("sheet-tables");
    let mut asked: Vec<Value> = Vec::new();
    for sh in &sheets {
        if asked.len() >= MAX_TABLE_QUESTIONS {
            break;
        }
        let Some(l) = lettering_of(sh) else { continue };
        let number = text(sh, "number").to_string();
        if !l.in_strokes() || job.rules.iter().any(|r| r.key.eq_ignore_ascii_case(&format!("{RULE}{number}"))) {
            continue;
        }
        let page = pages.iter().find(|(n, _)| n.eq_ignore_ascii_case(&number)).map(|(_, d)| d);
        let texts = page.map(sheet_texts).unwrap_or_default();
        let read_panels: Vec<&str> = panels.iter().filter(|p| p.source.eq_ignore_ascii_case(&number)).map(|p| p.panel.as_str()).collect();
        let read_types: Vec<&str> = fixtures
            .iter()
            .filter(|f| f.source.split_whitespace().next().is_some_and(|s| s.eq_ignore_ascii_case(&number)))
            .map(|f| f.tag.as_str())
            .collect();
        let none_read = read_panels.is_empty() && read_types.is_empty();
        let title = text(sh, "title");
        let lettering = json!({"stroke_share": l.stroke_share, "ocr_share": l.ocr_share, "words": l.words, "confidence": l.confidence,
                               "cells": l.cells, "cells_unsure": l.cells_unsure});
        if (carries_tables(sh, &l) && l.unsure()) || (electrical_tables(sh) && none_read) {
            let crop = page.and_then(|d| picture(d, None, &dir, &number));
            let read = if none_read {
                "no table on it was read".to_string()
            } else {
                format!("read {} panel schedule(s) and {} fixture type(s) from it, unsure", read_panels.len(), read_types.len())
            };
            ask(
                job,
                format!(
                    "{TABLE_QUESTION}{number} {title} could not be read: its lettering is strokes ({:.0}% of it, {:.0}% sure; {read}). Read its tables from this picture and answer them as rows: {{\"panels\": [{{\"panel\", \"circuits\": [{{\"number\", \"description\", \"breaker\", \"poles\"}}]}}], \"fixtures\": [{{\"tag\", \"description\", \"manufacturer\", \"model\", \"lamp\", \"voltage\", \"installation\"}}]}}",
                    l.ocr_share * 100.0,
                    l.confidence * 100.0
                ),
                "Panels, circuits and fixture types come from this sheet's tables; plotted lettering the stroke OCR can't read surely is asked, not built on a guess",
                &number,
                crop.clone(),
                json!({"sheet": number, "title": title, "lettering": lettering, "read": {"panels": read_panels, "fixture_types": read_types},
                       "unsure": texts.iter().filter(|t| t.unsure).take(MAX_UNSURE_LISTED).map(|t| t.value.as_str()).collect::<Vec<_>>()}),
                true,
            );
            asked.push(json!({"sheet": number, "blocking": true, "crop": crop}));
            continue;
        }
        // A sheet read sure: each table with unsure entries, once.
        for (table, window, values) in unsure_tables(&texts) {
            if asked.len() >= MAX_TABLE_QUESTIONS {
                break;
            }
            let crop = page.and_then(|d| picture(d, Some(window), &dir, &format!("{number}-{}", asked.len() + 1)));
            let name = if table.is_empty() { "a table".to_string() } else { table.clone() };
            ask(
                job,
                format!(
                    "{TABLE_QUESTION}{number} {name}: {} entries read unsure ({}). Check them in this picture; answer the table's rows (as for a sheet's tables) to correct it",
                    values.len(),
                    values.iter().take(12).cloned().collect::<Vec<_>>().join(", ")
                ),
                "An entry of a table read off strokes with low confidence is asked once per table, not built on a guess",
                &number,
                crop.clone(),
                json!({"sheet": number, "table": table, "unsure": values, "lettering": lettering}),
                false,
            );
            asked.push(json!({"sheet": number, "table": table, "blocking": false, "crop": crop}));
        }
    }
    if !asked.is_empty() {
        progress_line(&format!("  tables asked for on sheets lettered in strokes: {}", asked.len()));
        result["stroke_tables_asked"] = json!(asked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answers_tables_carry_the_answer_as_their_source() {
        let v = json!({"panels": [{"panel": " L1 ", "circuits": [{"number": "1", "description": "LIGHTING", "breaker": "20", "poles": 1}]}, {"panel": ""}],
                       "fixtures": [{"tag": "a", "description": "TROFFER"}, {"description": "no tag"}]});
        let (p, f) = tables_of(&v, "E-9");
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].panel.as_str(), p[0].source.as_str(), p[0].circuits.len()), ("L1", "E-9", 1));
        assert_eq!(p[0].header.get("source").map(String::as_str), Some(ANSWER_SOURCE));
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].tag.as_str(), f[0].source.as_str()), ("A", "E-9 (answer, from the sheet image)"));
        // a bare list is panels; anything else gives none
        assert_eq!(tables_of(&json!([{"panel": "P1"}]), "E-9").0.len(), 1);
        for bad in [json!("yes"), json!(42), json!({"panels": "L1"}), json!({"fixtures": [1, 2]}), json!(null)] {
            let (p, f) = tables_of(&bad, "E-9");
            assert!(p.is_empty() && f.is_empty(), "{bad}");
        }
    }

    #[test]
    fn a_sheet_carries_tables_by_its_type_its_title_or_its_ruled_cells() {
        let l = |cells: usize| Lettering { cells, ..Lettering::default() };
        assert!(carries_tables(&json!({"type": "schedule", "title": "ELECTRICAL"}), &l(0)));
        assert!(carries_tables(&json!({"type": "unknown", "title": "PANEL SCHEDULES"}), &l(0)));
        // a plan whose page holds a ruled table with words (its fixture schedule)
        assert!(carries_tables(&json!({"type": "plan", "title": "LIGHTING PLAN"}), &l(60)));
        assert!(!carries_tables(&json!({"type": "plan", "title": "LIGHTING PLAN"}), &l(6)));
        // the electrical ones: an electrical (or combined trade's) sheet titled for tables
        assert!(electrical_tables(&json!({"discipline": "E", "type": "schedule", "title": "ELECTRICAL SCHEDULES"})));
        assert!(electrical_tables(&json!({"discipline": "MEP", "type": "unknown", "title": "PANEL SCHEDULES"})));
        assert!(!electrical_tables(&json!({"discipline": "A", "type": "schedule", "title": "ROOM FINISH SCHEDULE"})));
        assert!(!electrical_tables(&json!({"discipline": "E", "type": "plan", "title": "POWER PLAN"})));
    }

    #[test]
    fn answered_tables_replace_the_ones_read_under_their_tags() {
        let read =
            vec![PanelSchedule { panel: "L1".into(), ..PanelSchedule::default() }, PanelSchedule { panel: "P1".into(), ..PanelSchedule::default() }];
        let answered = vec![PanelSchedule { panel: "l1".into(), source: "E-9".into(), ..PanelSchedule::default() }];
        let out = with_answered_panels(read, answered);
        assert_eq!(out.iter().map(|p| (p.panel.as_str(), p.source.as_str())).collect::<Vec<_>>(), [("P1", ""), ("l1", "E-9")]);
    }

    #[test]
    fn unsure_entries_go_to_the_table_titled_above_them() {
        let t = |v: &str, x: f64, y: f64, unsure: bool| SheetText { value: v.into(), at: Vec2::new(x, y), height: 0.1, unsure };
        let texts = vec![
            t("PANEL L1 SCHEDULE", 1.0, 10.0, false),
            t("PANEL P1 SCHEDULE", 1.0, 5.0, false),
            t("2O", 2.0, 9.0, true),
            t("I", 3.0, 8.5, true),
            t("3S", 2.0, 4.0, true),
            t("SEE NOTE 4 ON THE PANEL SCHEDULE SHEET FOR THE BREAKERS OF THE SECOND FLOOR", 1.0, 12.0, false),
            t("X#", 30.0, 1.0, true),
        ];
        let tables = unsure_tables(&texts);
        let names: Vec<(&str, usize)> = tables.iter().map(|(n, _, v)| (n.as_str(), v.len())).collect();
        assert_eq!(names, [("PANEL L1 SCHEDULE", 2), ("PANEL P1 SCHEDULE", 1), ("", 1)]);
        assert!(tables[0].1.contains(Vec2::new(1.0, 10.0)) && tables[0].1.contains(Vec2::new(3.0, 8.5)));
    }
}
