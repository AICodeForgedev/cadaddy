//! The sheet-index checkpoint (Q6517): right after classify and before levels, the job shows its
//! reading of every sheet — number, title, type, level, discipline, scale and where it came from,
//! the views found with their rectangles, the tables read, a picture of each page and a contact
//! sheet of them all — as one review packet, with the flags the software can raise itself, and
//! stops for it unless told to go on. One answer (`job.answer` on the checkpoint, or
//! `job.sheets.set`) corrects many sheets at once; each correction is a learned rule
//! (`sheet:<number>`, source "answer") over the readers, shown in job.explain and kept on resume.
//!
//! Coordinates in the packet and in answers are sheet inches from the page's lower-left corner
//! (as the page is displayed): a picture's pixel `(px, py)` from its top-left is
//! `(px / px_per_inch, paper_h − py / px_per_inch)`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::guided::{GuidedJob, SheetScale, StageName};
use cadaddy_mepfix::sheet_facts;

/// Pixels a sheet inch in a page's picture, unless the job is told another.
pub const DEFAULT_DPI: f64 = 40.0;
const MIN_DPI: f64 = 4.0;
const MAX_DPI: f64 = 150.0;
/// Views listed for one page, at most.
const MAX_VIEWS: usize = 40;
/// Sheets one answer corrects, at most.
const MAX_SHEETS_PER_ANSWER: usize = 500;
/// Rows a table answer gives, at most (each kind).
const MAX_TABLE_ROWS: usize = 2_000;
/// Flags kept in the packet, at most.
const MAX_FLAGS: usize = 2_000;
/// Flags the report lists, at most (the packet has them all).
const MAX_REPORTED: usize = 200;
/// The contact sheet: thumbnails a row, each this wide (pixels), with a band for its number.
const CONTACT_COLUMNS: u32 = 6;
const CONTACT_CELL_W: u32 = 300;
const CONTACT_LABEL_H: u32 = 28;
/// Pages on the contact sheet, at most.
const MAX_CONTACT: usize = 600;
/// What a level not built for want of a floor plan says (`SheetRegion::not_built`).
pub const NO_FLOOR_PLAN: &str = "no floor plan on its level";
/// What a sheet the answer keeps out of the build says.
pub const NOT_BUILT_ANSWER: &str = "not built (answer)";
/// The checkpoint question's text (stable: a re-run asks it again with its answer kept).
pub const SHEET_INDEX_QUESTION: &str =
    "Check the sheet index: the job's reading of every sheet (type, level, scale, plan view, tables) before it builds";
/// The learned rule key of a sheet's corrections.
pub const RULE_PREFIX: &str = "sheet:";

/// The sheet types an answer may give.
const SHEET_TYPES: &[&str] =
    &["plan", "enlarged_plan", "rcp", "schedule", "cover", "section", "elevation", "detail", "diagram", "specification", "other"];

// ─── Reading the pages (classify) ───────────────────────────────────────────

/// What classify keeps of each page for the packet: its facts always, its picture when a folder
/// is given (`printset.classify {look: {pictures?, dpi?}}`).
#[derive(Clone, Debug)]
pub struct Look {
    pub folder: Option<PathBuf>,
    pub dpi: f64,
}

/// `printset.classify`'s `look` parameter.
pub(crate) fn look_param(p: &Value) -> Option<Look> {
    let l = p.get("look")?;
    if l.is_null() || l.as_bool() == Some(false) {
        return None;
    }
    let folder = l.get("pictures").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(PathBuf::from);
    let dpi = l.get("dpi").and_then(Value::as_f64).filter(|d| d.is_finite() && *d > 0.0).map_or(DEFAULT_DPI, |d| d.clamp(MIN_DPI, MAX_DPI));
    Some(Look { folder, dpi })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// A point of a page drawn at `k` drawing units a sheet inch, in sheet inches.
fn to_sheet(p: [f64; 2], k: f64) -> [f64; 2] {
    [round2(p[0] / k), round2(p[1] / k)]
}

fn rect_to_sheet(r: [f64; 4], k: f64) -> [f64; 4] {
    [round2(r[0].min(r[2]) / k), round2(r[1].min(r[3]) / k), round2(r[0].max(r[2]) / k), round2(r[1].max(r[3]) / k)]
}

/// A page's file name for its picture: `<file>-p<page>.png`, with only safe characters.
fn picture_name(source: &str) -> String {
    let safe: String =
        source.replace('#', "-p").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(160).collect();
    format!("{safe}.png")
}

/// One page as classify read it: the reader's paper and scale, the scale its plan views print
/// (the import draws a sheet of several scales at that one), its views with their rectangles,
/// the rows the table readers read off it (a page that names a schedule or legend) and its
/// picture. Gives the drawing back.
pub(crate) fn look_at_page(page: cadaddy_mepfix::printset::LoadedPage, source: &str, look: &Look) -> (cadaddy_doc::Drawing, Value) {
    let k = if page.scale.is_finite() && page.scale > 0.0 { page.scale } else { 1.0 };
    let mut d = page.drawing;
    let texts = sheet_facts::texts(&d);
    // The scale notes as printed, for a view whose own scale text didn't read (lettering read
    // off its strokes beside the real text): the note right under its title.
    let notes: Vec<(f64, f64, &str, f64)> = texts
        .iter()
        .filter(|t| t.value.to_ascii_uppercase().contains("SCALE"))
        .filter_map(|t| Some((t.at.x, t.at.y, t.value.as_str(), cadaddy_io::pdf_read::parse_scale(&t.value)?)))
        .take(MAX_VIEWS * 4)
        .collect();
    let view_scale = |v: &sheet_facts::ViewTitle| -> (Option<String>, Option<f64>) {
        if let Some(k) = v.scale_text.as_deref().and_then(cadaddy_io::pdf_read::parse_scale) {
            return (v.scale_text.clone(), Some(k));
        }
        // within an inch below the title and three across
        let near = notes
            .iter()
            .filter(|(x, y, ..)| (x - v.at[0]).abs() <= 3.0 * k && *y <= v.at[1] && v.at[1] - y <= k)
            .min_by(|a, b| (v.at[1] - a.1).total_cmp(&(v.at[1] - b.1)));
        match near {
            Some((.., text, scale)) => (Some(scale_note(text)), Some(*scale)),
            None => (v.scale_text.clone(), None),
        }
    };
    let views: Vec<Value> = sheet_facts::view_titles(&d, &texts)
        .iter()
        .take(MAX_VIEWS)
        .map(|v| {
            let (scale_text, scale) = view_scale(v);
            json!({
                "title": v.title,
                "scale_text": scale_text,
                "scale": scale,
                "not_to_scale": v.not_to_scale,
                "at": to_sheet(v.at, k),
                "rect": v.region.map(|r| rect_to_sheet(r, k)),
            })
        })
        .collect();
    // The scale the import draws it at: a DO NOT SCALE sheet with a scale note at that note's
    // scale, a sheet printing several scales at its plan views' (as the job's import reads them
    // again).
    let mut import = (k, page.scale_source.clone());
    if page.needs_registration
        && k <= 1.5
        && let Some(note) = super::exchange::detected_scale(&json!({"scaleSource": page.scale_source}))
    {
        import = (note, format!("{} (the job's import reads it at its note's scale)", page.scale_source));
    }
    let first = import.0;
    if first > 1.5
        && let Some((v, view)) = sheet_facts::sheet_plan_print_scale(&d)
    {
        let want = 12.0 * super::perceive::units_per_inch(&d) / v;
        if want.is_finite() && want > 1.5 && want <= 1.0e5 && (want / first - 1.0).abs() > super::exchange::PLAN_SCALE_TOLERANCE {
            import =
                (want, format!("scale note {} (the scale the sheet's plans print, view {})", view.scale_text.as_deref().unwrap_or(""), view.title));
        }
    }
    let mut out = json!({
        "source": source,
        "paper": page.sheet.map(|(w, h)| [round2(w), round2(h)]),
        "scale": import.0,
        "scale_source": import.1.chars().take(240).collect::<String>(),
        "read_scale": k,
        "views": views,
    });
    let tabled = texts.iter().any(|t| {
        let u = t.value.to_ascii_uppercase();
        u.contains("SCHEDULE") || u.contains("LEGEND")
    });
    if tabled {
        let legend = cadaddy_mepfix::device_types::read_legend(&texts).rows.len();
        let pages = vec![(source.to_string(), d)];
        let panels = super::guided::panel_schedules(&pages);
        let (fixtures, _) = super::guided::fixture_schedule(&pages, &|_| true, &BTreeSet::new());
        d = pages.into_iter().next().map(|(_, d)| d).unwrap_or_default();
        out["tables"] = json!({
            "panel_schedules": panels.len(),
            "circuits": panels.iter().map(|p| p.circuits.len()).sum::<usize>(),
            "fixture_types": fixtures.len(),
            "legend_rows": legend,
        });
    }
    if let Some(folder) = &look.folder {
        let window = match page.sheet {
            Some((w, h)) if w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 => Some([[0.0, 0.0], [w * k, h * k]]),
            _ => {
                let b = d.extents(&cadaddy_doc::Space::Model);
                (!b.is_empty() && b.min.is_finite() && b.max.is_finite()).then_some([[b.min.x, b.min.y], [b.max.x, b.max.y]])
            }
        };
        if let Some(w) = window
            && let Some(png) = super::labels::page_png(&d, w, look.dpi / k)
        {
            let path = folder.join(picture_name(source));
            if write_file(&path, &png).is_ok() {
                out["picture"] = json!(path.to_string_lossy());
                out["px_per_inch"] = json!(look.dpi);
            }
        }
    }
    (d, out)
}

/// A scale note's text after its `SCALE:` (`1/4" = 1'-0"`).
fn scale_note(t: &str) -> String {
    let u = t.trim();
    let body = if u.to_ascii_uppercase().starts_with("SCALE") { u.get(5..).unwrap_or("").trim_start_matches([':', '.', ' ']) } else { u };
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn write_file(path: &Path, bytes: &[u8]) -> std::result::Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        crate::writefile::write(path, bytes).map_err(|e| e.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (path, bytes);
        Err("files can't be written on the web".into())
    }
}

/// The contact sheet: every page's picture, `CONTACT_COLUMNS` a row in the index's order, each
/// over a band with its sheet number. `None` when no picture could be read.
pub(crate) fn contact_sheet(cells: &[(String, String)], out: &Path) -> Option<Value> {
    let mut thumbs: Vec<(String, image::RgbaImage)> = Vec::new();
    for (label, path) in cells.iter().take(MAX_CONTACT) {
        let Ok(img) = image::open(path) else { continue };
        let t = img.thumbnail(CONTACT_CELL_W, CONTACT_CELL_W).to_rgba8();
        thumbs.push((label.clone(), t));
    }
    if thumbs.is_empty() {
        return None;
    }
    let cell_h = thumbs.iter().map(|(_, t)| t.height()).max().unwrap_or(CONTACT_CELL_W) + CONTACT_LABEL_H;
    let n = u32::try_from(thumbs.len()).ok()?;
    let cols = CONTACT_COLUMNS.min(n).max(1);
    let rows = n.div_ceil(cols);
    let mut sheet = image::RgbaImage::from_pixel(cols * CONTACT_CELL_W, rows * cell_h, image::Rgba([255, 255, 255, 255]));
    let mut placed = Vec::new();
    for (i, (label, t)) in thumbs.iter().enumerate() {
        let i = u32::try_from(i).ok()?;
        let (c, r) = (i % cols, i / cols);
        let (x, y) = (c * CONTACT_CELL_W, r * cell_h);
        image::imageops::overlay(&mut sheet, t, i64::from(x), i64::from(y + CONTACT_LABEL_H));
        if let Some(l) = label_image(label) {
            image::imageops::overlay(&mut sheet, &l, i64::from(x + 4), i64::from(y + 2));
        }
        // a frame round each cell
        for px in x..(x + CONTACT_CELL_W).min(sheet.width()) {
            for py in [y, (y + cell_h).saturating_sub(1).min(sheet.height().saturating_sub(1))] {
                if let Some(p) = sheet.get_pixel_mut_checked(px, py) {
                    *p = image::Rgba([160, 160, 160, 255]);
                }
            }
        }
        placed.push(json!({"sheet": label, "column": c, "row": r}));
    }
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(sheet).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).ok()?;
    write_file(out, &bytes).ok()?;
    Some(json!({"path": out.to_string_lossy(), "columns": cols, "rows": rows, "cell_px": [CONTACT_CELL_W, cell_h], "cells": placed}))
}

/// A sheet number as a picture (dark lettering on white), for the contact sheet's bands.
fn label_image(label: &str) -> Option<image::RgbaImage> {
    use cadaddy_doc::{Common, EntityKind, HAlign, Space, Text, VAlign};
    let mut d = cadaddy_doc::Drawing::default();
    let value: String = label.chars().take(24).collect();
    let t = Text {
        insert: cadaddy_geom::Vec3::new(0.05, 0.05, 0.0),
        align_pt: None,
        height: 0.18,
        value,
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    };
    d.add(&Space::Model, Common::default(), EntityKind::Text(t)).ok()?;
    let png = super::labels::page_png(&d, [[0.0, 0.0], [2.8, 0.25]], 100.0)?;
    image::load_from_memory(&png).ok().map(|i| i.to_rgba8())
}

// ─── Corrections ────────────────────────────────────────────────────────────

/// What an answer says about one sheet, over what the readers read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetCorrection {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discipline: Option<String>,
    /// Drawing units (inches) a sheet inch: 48 is 1/4" = 1'-0".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// The scale as the answer gave it (`1/4" = 1'-0"`, or the number).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_text: Option<String>,
    /// The plan's rectangle `[x0, y0, x1, y1]`, sheet inches from the lower-left corner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_view: Option<[f64; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_built: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tables: Option<SheetTables>,
    /// Where the corrections came from: `answer`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

/// Tables an answer gives for a sheet the readers could not read, as the readers produce them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetTables {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panel_schedules: Vec<cadaddy_walk3d::PanelSchedule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixture_schedule: Vec<cadaddy_walk3d::FixtureType>,
    /// Legend or schedule rows by type tag: what the row says after the tag.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub legend: BTreeMap<String, String>,
}

/// What a table row given in an answer names as its source.
fn answer_source(number: &str) -> String {
    format!("{number} (answer, from the sheet image)")
}

impl SheetCorrection {
    /// Fields `other` gives replace this one's.
    fn merge(&mut self, other: SheetCorrection) {
        macro_rules! take {
            ($($f:ident),*) => { $( if other.$f.is_some() { self.$f = other.$f; } )* };
        }
        take!(kind, level, title, discipline, scale, scale_text, plan_view, not_built, tables);
    }

    /// The fields it sets, for the packet and the effect line.
    pub fn fields(&self) -> Vec<&'static str> {
        let mut f = Vec::new();
        for (on, name) in [
            (self.kind.is_some(), "type"),
            (self.level.is_some(), "level"),
            (self.title.is_some(), "title"),
            (self.discipline.is_some(), "discipline"),
            (self.scale.is_some(), "scale"),
            (self.plan_view.is_some(), "plan_view"),
            (self.not_built.is_some(), "not_built"),
            (self.tables.is_some(), "tables"),
        ] {
            if on {
                f.push(name);
            }
        }
        f
    }
}

/// The index's sheet numbers, as printed.
fn index_numbers(job: &GuidedJob) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for list in [job.index.as_ref(), job.sheets.as_ref()].into_iter().flatten() {
        for n in list.get("sheets").and_then(Value::as_array).into_iter().flatten().filter_map(|sh| sh.get("number").and_then(Value::as_str)) {
            let base = n.split('/').next().unwrap_or(n).to_string();
            if !base.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&base)) {
                out.push(base);
            }
        }
    }
    out
}

/// A paper size of a sheet, inches, from the packet's pages (the first copy's).
fn sheet_paper(job: &GuidedJob, number: &str) -> Option<[f64; 2]> {
    let page = sheet_page(job, number)?;
    let p = page.get("paper")?.as_array()?;
    Some([p.first()?.as_f64()?, p.get(1)?.as_f64()?])
}

/// The packet's page facts of a sheet's current copy.
fn sheet_page<'a>(job: &'a GuidedJob, number: &str) -> Option<&'a Value> {
    let index = job.index.as_ref()?.get("sheets")?.as_array()?;
    let sh = index.iter().find(|s| s.get("number").and_then(Value::as_str).is_some_and(|n| same_sheet(n, number)))?;
    let source = sh.get("sources").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str)?;
    job.index.as_ref()?.get("pages")?.get(source)
}

fn same_sheet(a: &str, b: &str) -> bool {
    a.split('/').next().unwrap_or(a).trim().eq_ignore_ascii_case(b.split('/').next().unwrap_or(b).trim())
}

/// A scale an answer gives: a number (drawing units a sheet inch, 48 for 1/4" = 1'-0") or the
/// note as printed.
fn parse_answer_scale(v: &Value) -> std::result::Result<(f64, String), String> {
    let (k, text) = match v {
        Value::Number(n) => (n.as_f64().unwrap_or(f64::NAN), n.to_string()),
        Value::String(t) => {
            let t = t.trim();
            match t.parse::<f64>() {
                Ok(k) => (k, t.to_string()),
                Err(_) => (
                    cadaddy_io::pdf_read::parse_scale(t).ok_or_else(|| {
                        format!("scale \"{}\" doesn't read as a scale (like 1/4\" = 1'-0\" or 48)", t.chars().take(40).collect::<String>())
                    })?,
                    t.to_string(),
                ),
            }
        }
        other => return Err(format!("scale must be a number or a scale note, not {other}")),
    };
    if !(k.is_finite() && k > 0.0) {
        return Err(format!("scale must be more than 0 ({text} given)"));
    }
    if k > 1.0e5 {
        return Err(format!("scale {text} is more than 100000 drawing units a sheet inch"));
    }
    Ok((k, text))
}

/// The corrections an answer gives (`{"<number>": {type?, level?, title?, discipline?, scale?,
/// plan_view?, not_built?, tables?}}`), each checked against the index: an unknown sheet, an
/// unknown field, a bad rectangle or a scale of 0 or less is an error, never a panic.
pub fn parse_corrections(job: &GuidedJob, answer: &Value) -> std::result::Result<Vec<(String, SheetCorrection)>, String> {
    let Some(map) = answer.as_object() else { return Err("give the corrections as {\"<sheet number>\": {type?, level?, …}}".into()) };
    if map.is_empty() {
        return Err("no sheet corrections given".into());
    }
    if map.len() > MAX_SHEETS_PER_ANSWER {
        return Err(format!("at most {MAX_SHEETS_PER_ANSWER} sheets an answer ({} given)", map.len()));
    }
    let numbers = index_numbers(job);
    let mut out = Vec::new();
    for (raw, fields) in map {
        let shown: String = raw.chars().take(40).collect();
        let Some(number) = numbers.iter().find(|n| same_sheet(n, raw)).cloned() else {
            let some: Vec<&str> = numbers.iter().take(12).map(String::as_str).collect();
            return Err(format!(
                "sheet '{shown}' is not in the set's sheet index ({} sheets: {}{})",
                numbers.len(),
                some.join(", "),
                if numbers.len() > some.len() { ", …" } else { "" }
            ));
        };
        let Some(f) = fields.as_object() else { return Err(format!("sheet {shown}: give its corrections as an object ({fields} given)")) };
        let mut c = SheetCorrection::default();
        for (key, v) in f {
            let text = || v.as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
            match key.as_str() {
                "type" => {
                    let t = text().ok_or_else(|| format!("sheet {shown}: type must be text"))?.to_ascii_lowercase();
                    if !SHEET_TYPES.contains(&t.as_str()) {
                        return Err(format!(
                            "sheet {shown}: type '{}' is none of {}",
                            t.chars().take(30).collect::<String>(),
                            SHEET_TYPES.join(", ")
                        ));
                    }
                    c.kind = Some(t);
                }
                "level" => {
                    let t = text().ok_or_else(|| format!("sheet {shown}: level must be text (like \"2\" or \"Level 2\")"))?;
                    let code = t.strip_prefix("Level").or_else(|| t.strip_prefix("LEVEL")).map_or(t.as_str(), str::trim).to_string();
                    if code.is_empty() || code.chars().count() > 12 || !code.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == ' ') {
                        return Err(format!(
                            "sheet {shown}: level '{}' is not a level code (like 2, R or B1)",
                            t.chars().take(30).collect::<String>()
                        ));
                    }
                    c.level = Some(code.to_ascii_uppercase());
                }
                "title" => c.title = Some(text().ok_or_else(|| format!("sheet {shown}: title must be text"))?.chars().take(200).collect()),
                "discipline" => {
                    let t = text().ok_or_else(|| format!("sheet {shown}: discipline must be text (A, E, M, P …)"))?.to_ascii_uppercase();
                    if t.len() > 3 || !t.chars().all(|ch| ch.is_ascii_alphabetic()) {
                        return Err(format!(
                            "sheet {shown}: discipline '{}' is not a discipline letter (A, E, M, P, FP …)",
                            t.chars().take(20).collect::<String>()
                        ));
                    }
                    c.discipline = Some(t);
                }
                "scale" => {
                    let (k, t) = parse_answer_scale(v).map_err(|e| format!("sheet {shown}: {e}"))?;
                    c.scale = Some(k);
                    c.scale_text = Some(t);
                }
                "plan_view" => {
                    let r: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                    let ok = <[f64; 4]>::try_from(r.clone())
                        .ok()
                        .filter(|r| r.iter().all(|x| x.is_finite()) && r[2] > r[0] && r[3] > r[1] && r[0] >= -1.0 && r[1] >= -1.0);
                    let Some(rect) = ok else {
                        return Err(format!(
                            "sheet {shown}: plan_view must be [x0, y0, x1, y1] in sheet inches from the lower-left corner, x1 > x0 and y1 > y0 ({v} given)"
                        ));
                    };
                    if let Some([w, h]) = sheet_paper(job, &number)
                        && (rect[2] > w + 1.0 || rect[3] > h + 1.0)
                    {
                        return Err(format!("sheet {shown}: plan_view {v} reaches off the {w}\" × {h}\" sheet"));
                    }
                    c.plan_view = Some(rect);
                }
                "not_built" => c.not_built = Some(v.as_bool().ok_or_else(|| format!("sheet {shown}: not_built must be true or false"))?),
                "tables" => c.tables = Some(parse_tables(&number, v).map_err(|e| format!("sheet {shown}: {e}"))?),
                other => {
                    return Err(format!(
                        "sheet {shown}: '{}' is not a sheet field (type, level, title, discipline, scale, plan_view, not_built, tables)",
                        other.chars().take(30).collect::<String>()
                    ));
                }
            }
        }
        if c.fields().is_empty() {
            return Err(format!("sheet {shown}: no correction given"));
        }
        out.push((number, c));
    }
    Ok(out)
}

/// Table rows an answer gives, in the readers' shape, each with the answer as its source.
fn parse_tables(number: &str, v: &Value) -> std::result::Result<SheetTables, String> {
    let Some(o) = v.as_object() else { return Err("tables must be {panel_schedules?, fixture_schedule?, legend?}".into()) };
    let mut t = SheetTables::default();
    for (k, rows) in o {
        let count = rows.as_array().map_or_else(|| rows.as_object().map_or(0, serde_json::Map::len), Vec::len);
        if count > MAX_TABLE_ROWS {
            return Err(format!("tables.{k}: at most {MAX_TABLE_ROWS} rows ({count} given)"));
        }
        match k.as_str() {
            "panel_schedules" => {
                t.panel_schedules = serde_json::from_value(rows.clone())
                    .map_err(|e| format!("tables.panel_schedules: {e} (give the panel schedule reader's rows: {{panel, circuits: [...]}})"))?;
                for p in &mut t.panel_schedules {
                    if p.panel.trim().is_empty() {
                        return Err("tables.panel_schedules: every schedule needs its panel's tag".into());
                    }
                    p.source = answer_source(number);
                    for c in &mut p.circuits {
                        c.source = answer_source(number);
                    }
                }
            }
            "fixture_schedule" => {
                t.fixture_schedule = serde_json::from_value(rows.clone())
                    .map_err(|e| format!("tables.fixture_schedule: {e} (give the fixture schedule reader's rows: {{tag, description, …}})"))?;
                for f in &mut t.fixture_schedule {
                    if f.tag.trim().is_empty() {
                        return Err("tables.fixture_schedule: every row needs its type tag".into());
                    }
                    f.source = answer_source(number);
                }
            }
            "legend" => {
                let rows: BTreeMap<String, String> = match rows {
                    Value::Object(m) => m.iter().map(|(k, v)| (k.trim().to_string(), v.as_str().unwrap_or_default().trim().to_string())).collect(),
                    Value::Array(a) => a
                        .iter()
                        .filter_map(|r| {
                            Some((
                                r.get("type")?.as_str()?.trim().to_string(),
                                r.get("description").and_then(Value::as_str).unwrap_or("").trim().to_string(),
                            ))
                        })
                        .collect(),
                    _ => return Err("tables.legend must be {\"<type>\": \"<description>\"} or [{type, description}]".into()),
                };
                if rows.keys().any(|k| k.is_empty() || k.chars().count() > 12) {
                    return Err("tables.legend: every row needs a type tag of 12 characters at most".into());
                }
                t.legend = rows;
            }
            other => {
                return Err(format!("tables: '{}' is none of panel_schedules, fixture_schedule, legend", other.chars().take(30).collect::<String>()));
            }
        }
    }
    Ok(t)
}

/// The corrections the job has learned, by sheet number.
pub fn learned(job: &GuidedJob) -> BTreeMap<String, SheetCorrection> {
    job.rules
        .iter()
        .filter_map(|r| {
            let number = r.key.strip_prefix(RULE_PREFIX)?;
            let c: SheetCorrection = serde_json::from_str(&r.value).ok()?;
            Some((number.to_string(), c))
        })
        .collect()
}

/// Learn the corrections (each merged into the sheet's earlier ones) and put them into the
/// job's sheet index. Returns the effect line.
pub fn learn(job: &mut GuidedJob, corrections: Vec<(String, SheetCorrection)>, from: &str) -> String {
    let mut known = learned(job);
    let mut said = Vec::new();
    for (number, c) in corrections {
        said.push(format!("{number}: {}", c.fields().join(", ")));
        let merged = known.entry(number.clone()).or_default();
        merged.merge(c);
        merged.source = "answer".into();
        let value = serde_json::to_string(merged).unwrap_or_default();
        job.learn_rule(&format!("{RULE_PREFIX}{number}"), &value, from, StageName::Classify);
    }
    apply(job);
    // The packet and the classify result (job.explain) show the reading as corrected.
    if summary(job).is_some() {
        let sum = write_packet(job, &packet_dir(job));
        if let Some(r) = job.stage_mut(StageName::Classify).and_then(|st| st.result.as_mut()).and_then(Value::as_object_mut) {
            r.insert("sheet_index".into(), sum);
        }
    }
    format!("sheet index corrected ({}): the job builds on it", said.join("; "))
}

/// Put the learned corrections into the job's sheet lists (the index and the current sheets):
/// they override the readers. Runs again after every classify, so they survive re-runs and resume.
pub fn apply(job: &mut GuidedJob) {
    let rules = learned(job);
    if rules.is_empty() {
        return;
    }
    for list in [job.sheets.as_mut(), job.index.as_mut()].into_iter().flatten() {
        let Some(a) = list.get_mut("sheets").and_then(Value::as_array_mut) else { continue };
        for sh in a.iter_mut() {
            let Some(number) = sh.get("number").and_then(Value::as_str).map(str::to_string) else { continue };
            let Some(c) = rules.iter().find(|(n, _)| same_sheet(n, &number)).map(|(_, c)| c) else { continue };
            let Some(o) = sh.as_object_mut() else { continue };
            if let Some(t) = &c.kind {
                o.insert("type".into(), json!(t));
            }
            if let Some(l) = &c.level {
                o.insert("level".into(), json!(l));
                o.insert("levels".into(), json!([l]));
                o.insert("level_source".into(), json!("answer"));
            }
            if let Some(t) = &c.title {
                o.insert("title".into(), json!(t));
            }
            if let Some(d) = &c.discipline {
                o.insert("discipline".into(), json!(d));
            }
            if let Some(k) = c.scale {
                o.insert("scale_given".into(), json!(k));
            }
            if let Some(r) = c.plan_view {
                o.insert("plan_view".into(), json!(r));
            }
            if let Some(nb) = c.not_built {
                o.insert("not_built".into(), json!(nb));
            }
            o.insert("corrected".into(), json!(c.fields()));
        }
    }
    // The levels: each corrected sheet on the level it shows now.
    let plan = |t: &str| matches!(t, "plan" | "enlarged_plan" | "rcp");
    let moved: Vec<(String, Option<String>)> = job
        .index
        .as_ref()
        .and_then(|i| i.get("sheets"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|sh| sh.get("corrected").is_some())
                .filter_map(|sh| {
                    let number = sh.get("number")?.as_str()?.to_string();
                    let kind = sh.get("type").and_then(Value::as_str).unwrap_or("");
                    let level = sh.get("level").and_then(Value::as_str).filter(|l| !l.is_empty() && plan(kind)).map(str::to_string);
                    let not_built = sh.get("not_built").and_then(Value::as_bool).unwrap_or(false);
                    Some((number, level.filter(|_| !not_built)))
                })
                .collect()
        })
        .unwrap_or_default();
    let Some(levels) = job.index.as_mut().and_then(|i| i.get_mut("levels")).and_then(Value::as_array_mut) else { return };
    let named = |v: &Value| v.as_str().map(str::to_string).or_else(|| v.get("sheet").and_then(Value::as_str).map(str::to_string));
    for (number, level) in &moved {
        for l in levels.iter_mut() {
            let here = l.get("name").and_then(Value::as_str).is_some_and(|n| level.as_deref().is_some_and(|lv| lv.eq_ignore_ascii_case(n)));
            if here {
                continue;
            }
            if let Some(sheets) = l.get_mut("sheets").and_then(Value::as_array_mut) {
                sheets.retain(|v| !named(v).is_some_and(|n| same_sheet(&n, number)));
            }
        }
        if let Some(level) = level {
            match levels.iter_mut().find(|l| l.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(level))) {
                Some(l) => {
                    if let Some(sheets) = l.get_mut("sheets").and_then(Value::as_array_mut)
                        && !sheets.iter().any(|v| named(v).is_some_and(|n| same_sheet(&n, number)))
                    {
                        sheets.push(json!({"sheet": number, "typical": false}));
                    }
                }
                None => levels.push(json!({"name": level, "sheets": [{"sheet": number, "typical": false}]})),
            }
        }
    }
    // A level left with no sheet is gone; the rest bottom up.
    levels.retain(|l| l.get("sheets").and_then(Value::as_array).is_none_or(|s| !s.is_empty()));
    levels.sort_by_key(|l| level_rank(l.get("name").and_then(Value::as_str).unwrap_or("")));
}

fn level_rank(l: &str) -> (u8, i64, String) {
    let u = l.trim().to_ascii_uppercase();
    if let Some(b) = u.strip_prefix('B').and_then(|n| n.parse::<i64>().ok()) {
        return (0, -b, u);
    }
    match u.parse::<i64>() {
        Ok(n) => (1, n, u),
        Err(_) if u == "R" || u.contains("ROOF") => (3, 0, u),
        Err(_) => (2, 0, u),
    }
}

// ─── Flags ──────────────────────────────────────────────────────────────────

/// Whether an architectural plan's title makes it the floor plan of its level: one that names a
/// floor plan, or any plan that is no other kind of plan (a finish, furniture, demolition or
/// ceiling plan: rule data `sheets.not_floor_plan_title_words`).
pub fn is_floor_plan_title(title: &str) -> bool {
    cadaddy_mepfix::sheet_reader::names_floor_plan(title)
        || !cadaddy_mepfix::sheet_reader::title_names(title, cadaddy_mepfix::wordrules::words("sheets.not_floor_plan_title_words"))
}

fn title_names_plan(title: &str) -> bool {
    title.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "PLAN" || w == "PLANS")
}

fn str_of<'a>(sh: &'a Value, k: &str) -> &'a str {
    sh.get(k).and_then(Value::as_str).unwrap_or("")
}

fn is_plan_type(t: &str) -> bool {
    matches!(t, "plan" | "enlarged_plan" | "rcp")
}

fn levels_of(sh: &Value) -> Vec<String> {
    let mut out: Vec<String> =
        sh.get("levels").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let l = str_of(sh, "level");
    if out.is_empty() && !l.is_empty() {
        out.push(l.to_string());
    }
    out
}

/// The building levels the index lists that no architectural floor plan shows (a level of only a
/// finish plan, or of trade plans only), each with its architectural plan sheets. A roof's plan
/// is its own; a sheet an answer made a plan of the level counts as its floor plan.
pub fn levels_without_floor_plan(job: &GuidedJob) -> Vec<(String, Vec<String>)> {
    let Some(index) = job.index.as_ref() else { return Vec::new() };
    let sheets = index.get("sheets").and_then(Value::as_array).cloned().unwrap_or_default();
    let levels: Vec<String> = index
        .get("levels")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|l| l.get("name").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for level in levels {
        let u = level.to_ascii_uppercase();
        if cadaddy_walk3d::levels::is_datum_name(&level) || u == "R" || u.contains("ROOF") {
            continue;
        }
        let arch: Vec<&Value> = sheets
            .iter()
            .filter(|sh| str_of(sh, "discipline").eq_ignore_ascii_case("A") && is_plan_type(str_of(sh, "type")) && str_of(sh, "type") != "rcp")
            .filter(|sh| !sh.get("not_built").and_then(Value::as_bool).unwrap_or(false))
            .filter(|sh| levels_of(sh).iter().any(|l| l.eq_ignore_ascii_case(&level)))
            .collect();
        let answered = |sh: &Value| sh.get("corrected").and_then(Value::as_array).is_some_and(|f| f.iter().any(|x| x == "type" || x == "plan_view"));
        if !arch.iter().any(|sh| is_floor_plan_title(str_of(sh, "title")) || answered(sh)) {
            out.push((level, arch.iter().map(|sh| str_of(sh, "number").to_string()).collect()));
        }
    }
    out
}

/// One thing the software flags in its own reading of the sheets.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Flag {
    /// `no_floor_plan`, `plan_title_not_plan`, `plan_off_scale`, `plan_at_paper_size`,
    /// `table_no_rows`, `trade_plan_no_level` or `mixed_sheet`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sheet: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub level: String,
    pub says: String,
    /// The correction that settles it, as an answer gives it.
    pub fix: String,
}

/// The flags the software raises on its reading of the sheets (the index with any corrections,
/// and the pages' facts classify kept).
pub fn flags(job: &GuidedJob) -> Vec<Flag> {
    let mut out = Vec::new();
    let Some(index) = job.index.as_ref() else { return out };
    let sheets = index.get("sheets").and_then(Value::as_array).cloned().unwrap_or_default();
    let answered = |sh: &Value, field: &str| sh.get("corrected").and_then(Value::as_array).is_some_and(|f| f.iter().any(|x| x == field));
    let page = |sh: &Value| -> Option<Value> {
        let source = sh.get("sources").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str)?;
        index.get("pages")?.get(source).cloned()
    };
    // 1. a level with no architectural floor plan
    for (level, arch) in levels_without_floor_plan(job) {
        let says = if arch.is_empty() {
            format!("Level {level} has no architectural floor plan: it is not built until one is named")
        } else {
            format!(
                "Level {level} has no architectural floor plan (its architectural plans: {}): it is not built from them until one is named",
                arch.join(", ")
            )
        };
        out.push(Flag {
            kind: "no_floor_plan",
            sheet: String::new(),
            level: level.clone(),
            says,
            fix: format!("{{\"<floor plan's number>\": {{\"type\": \"plan\", \"level\": \"{level}\"}}}}"),
        });
    }
    let off_scale: Vec<String>;
    {
        // 3. plans at another scale than their peers, or at paper size (the import's reading)
        // (a site plan is drawn at a site scale of its own)
        let site =
            |sh: &Value| !super::guided::not_built_reason(str_of(sh, "discipline"), str_of(sh, "title"), str_of(sh, "level"), "title").is_empty();
        let scales: Vec<SheetScale> = sheets
            .iter()
            .filter(|sh| str_of(sh, "type") == "plan" && !answered(sh, "scale") && !super::guided::spec_sheet(sh) && !site(sh))
            .filter_map(|sh| {
                let p = page(sh)?;
                let paper = p.get("paper").and_then(Value::as_array).and_then(|a| Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?]));
                Some(SheetScale {
                    sheet: str_of(sh, "number").to_string(),
                    kind: "plan".into(),
                    scale: p.get("scale").and_then(Value::as_f64).unwrap_or(1.0),
                    source: str_of(&p, "scale_source").to_string(),
                    plan_scale: None,
                    paper,
                })
            })
            .collect();
        let off = super::guided::plans_off_set_scale(&scales);
        off_scale = off.iter().map(|o| o.sheet.clone()).collect();
        for o in off {
            out.push(Flag {
                kind: "plan_off_scale",
                sheet: o.sheet.clone(),
                level: String::new(),
                says: format!(
                    "{} reads at {}x ({}) but {} other plan(s) of its paper size read at {}x",
                    o.sheet, o.scale, o.how, o.set_sheets, o.set_scale
                ),
                fix: format!("{{\"{}\": {{\"scale\": \"<its plan's scale note, like 1/4\\\" = 1'-0\\\">\"}}}}", o.sheet),
            });
        }
        for sc in scales.iter().filter(|sc| sc.plan_at_paper_size() && !off_scale.contains(&sc.sheet)) {
            out.push(Flag {
                kind: "plan_at_paper_size",
                sheet: sc.sheet.clone(),
                level: String::new(),
                says: format!(
                    "{} is a plan with no scale read (no note, no lettering, no dimension strings that agree): it would be built at paper size",
                    sc.sheet
                ),
                fix: format!("{{\"{}\": {{\"scale\": \"<its plan's scale note>\"}}}}", sc.sheet),
            });
        }
    }
    for sh in &sheets {
        let number = str_of(sh, "number").to_string();
        let title = str_of(sh, "title");
        let kind = str_of(sh, "type");
        if super::guided::spec_sheet(sh) || sh.get("not_built").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        // 2. a title (the sheet's, or one of its views') that names a plan, typed as no plan
        let plan_view_title = page(sh).and_then(|p| {
            p.get("views")?
                .as_array()?
                .iter()
                .filter_map(|v| v.get("title").and_then(Value::as_str))
                .find(|t| title_names_plan(t))
                .map(str::to_string)
        });
        if (title_names_plan(title) || plan_view_title.is_some()) && !is_plan_type(kind) && !answered(sh, "type") {
            let named = if title_names_plan(title) {
                format!("\"{title}\" names a plan")
            } else {
                format!("\"{title}\" prints a view titled \"{}\"", plan_view_title.unwrap_or_default())
            };
            out.push(Flag {
                kind: "plan_title_not_plan",
                sheet: number.clone(),
                level: String::new(),
                says: format!("{number} {named} but was read as type {kind}: it is not read as a plan"),
                fix: format!("{{\"{number}\": {{\"type\": \"plan\", \"level\": \"<its level>\", \"title\"?: \"<its title>\"}}}}"),
            });
        }
        // 4. a schedule or legend sheet whose tables read no row
        let tabled = {
            let u = title.to_ascii_uppercase();
            u.contains("SCHEDULE") || u.contains("LEGEND")
        };
        if tabled && !answered(sh, "tables") {
            let rows = page(sh).and_then(|p| p.get("tables").cloned()).map_or(0, |t| {
                ["circuits", "fixture_types", "legend_rows"].iter().map(|k| t.get(*k).and_then(Value::as_u64).unwrap_or(0)).sum::<u64>()
            });
            if rows == 0 {
                out.push(Flag {
                    kind: "table_no_rows",
                    sheet: number.clone(),
                    level: String::new(),
                    says: format!("{number} \"{title}\" is titled for schedules or legends, but no panel schedule, fixture schedule or legend row was read off it"),
                    fix: format!("{{\"{number}\": {{\"tables\": {{\"panel_schedules\"?: […], \"fixture_schedule\"?: […], \"legend\"?: {{\"<type>\": \"<description>\"}}}}}}}}"),
                });
            }
        }
        if !is_plan_type(kind) {
            continue;
        }
        // 5. a trade's plan with no level
        let discipline = str_of(sh, "discipline");
        let site = !super::guided::not_built_reason(discipline, title, "1", "title").is_empty();
        // (a plan given the one level of a one-level building has it; the general sheets are no
        // trade's)
        if !discipline.eq_ignore_ascii_case("A")
            && !discipline.eq_ignore_ascii_case("G")
            && !site
            && (levels_of(sh).is_empty() || str_of(sh, "level_source") == "ask")
            && !answered(sh, "level")
        {
            out.push(Flag {
                kind: "trade_plan_no_level",
                sheet: number.clone(),
                level: String::new(),
                says: format!("{number} \"{title}\" is a {discipline} plan whose title, views and number name no level: its work goes on no level"),
                fix: format!("{{\"{number}\": {{\"level\": \"<its level>\"}}}}"),
            });
        }
        // 6. a plan sheet that also prints details, sections or elevations at another scale
        if !answered(sh, "plan_view")
            && let Some(p) = page(sh)
        {
            let views = p.get("views").and_then(Value::as_array).cloned().unwrap_or_default();
            let scaled: Vec<&Value> = views.iter().filter(|v| v.get("scale").and_then(Value::as_f64).is_some_and(|k| k > 0.0)).collect();
            let mut distinct: Vec<f64> = Vec::new();
            for v in &scaled {
                if let Some(k) = v.get("scale").and_then(Value::as_f64)
                    && !distinct.iter().any(|d| (d / k - 1.0).abs() < 0.01)
                {
                    distinct.push(k);
                }
            }
            let other = scaled.iter().any(|v| !title_names_plan(str_of(v, "title")));
            if distinct.len() >= 2 && other {
                let named: Vec<String> = scaled
                    .iter()
                    .take(8)
                    .map(|v| {
                        format!(
                            "\"{}\" at {} {}",
                            str_of(v, "title"),
                            str_of(v, "scale_text"),
                            v.get("rect").map_or_else(|| "(no rectangle found)".into(), |r| r.to_string())
                        )
                    })
                    .collect();
                out.push(Flag {
                    kind: "mixed_sheet",
                    sheet: number.clone(),
                    level: String::new(),
                    says: format!(
                        "{number} prints its plan beside views at other scales ({}): which rectangle is the plan, and its scale",
                        named.join("; ")
                    ),
                    fix: format!("{{\"{number}\": {{\"plan_view\": [x0, y0, x1, y1], \"scale\": \"<the plan's scale note>\"}}}}"),
                });
            }
        }
    }
    out.truncate(MAX_FLAGS);
    out
}

// ─── The packet ─────────────────────────────────────────────────────────────

/// Where the job's packet goes: beside its checkpoint, else in the temporary folder.
pub fn packet_dir(job: &GuidedJob) -> PathBuf {
    match &job.checkpoint {
        Some(c) => {
            let p = Path::new(c);
            let stem = p.file_stem().map_or_else(|| "job".to_string(), |s| s.to_string_lossy().to_string());
            p.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf).join(format!("{stem} sheet index"))
        }
        None => std::env::temp_dir()
            .join(format!("cadaddy-job-{} sheet index", job.id.chars().filter(char::is_ascii_alphanumeric).take(40).collect::<String>())),
    }
}

/// The review packet: every sheet as the job read it (with its corrections), its pictures, the
/// flags and how to answer. Written as `sheet-index.json` beside the pictures and the contact
/// sheet; returns its summary for the classify result.
pub fn write_packet(job: &GuidedJob, dir: &Path) -> Value {
    let flags = flags(job);
    let index = job.index.as_ref();
    let pages = index.and_then(|i| i.get("pages"));
    let rules = learned(job);
    let mut cells: Vec<(String, String)> = Vec::new();
    let sheets: Vec<Value> = index
        .and_then(|i| i.get("sheets"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|sh| {
                    let number = str_of(sh, "number");
                    let sources: Vec<&str> = sh.get("sources").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                    let first = sources.first().and_then(|src| pages.and_then(|p| p.get(*src)));
                    let pictures: Vec<&str> = sources.iter().filter_map(|src| pages.and_then(|p| p.get(*src)).and_then(|p| p.get("picture")).and_then(Value::as_str)).collect();
                    if let Some(p) = pictures.first() {
                        cells.push((number.to_string(), (*p).to_string()));
                    }
                    let printed = job
                        .sheets
                        .as_ref()
                        .and_then(|v| v.get("sheets"))
                        .and_then(Value::as_array)
                        .and_then(|a| a.iter().find(|x| same_sheet(str_of(x, "number"), number)))
                        .and_then(|x| x.get("scale").cloned())
                        .unwrap_or(Value::Null);
                    let given = rules.iter().find(|(n, _)| same_sheet(n, number)).map(|(_, c)| c);
                    json!({
                        "number": number,
                        "title": sh.get("title"),
                        "discipline": sh.get("discipline"),
                        "type": sh.get("type"),
                        "level": sh.get("level"),
                        "levels": sh.get("levels"),
                        "level_source": sh.get("level_source"),
                        "scale": {
                            "printed": printed,
                            "import": given.and_then(|c| c.scale).map_or_else(|| first.and_then(|p| p.get("scale")).cloned().unwrap_or(Value::Null), |k| json!(k)),
                            "source": if given.is_some_and(|c| c.scale.is_some()) { json!("answer") } else { first.and_then(|p| p.get("scale_source")).cloned().unwrap_or(Value::Null) },
                            "unit": "drawing inches a sheet inch (48 = 1/4\" = 1'-0\")",
                        },
                        "paper": first.and_then(|p| p.get("paper")),
                        "views": first.and_then(|p| p.get("views")),
                        "plan_view": sh.get("plan_view"),
                        "tables": first.and_then(|p| p.get("tables")),
                        "pictures": pictures,
                        "sources": sources,
                        "not_built": sh.get("not_built"),
                        "corrected": given.map(|c| json!(c)),
                        "flags": flags.iter().filter(|f| same_sheet(&f.sheet, number) && !f.sheet.is_empty()).map(|f| f.kind).collect::<Vec<_>>(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let contact = contact_sheet(&cells, &dir.join("contact-sheet.png"));
    let packet = json!({
        "about": "The job's reading of every sheet, before it builds (Q6517). Check each against its picture; correct what is wrong in one answer.",
        "coordinates": "sheet inches from the page's lower-left corner as displayed; a picture pixel (px, py) from its top-left is (px / px_per_inch, paper_h - py / px_per_inch)",
        "answer": "job.sheets.set {\"<number>\": {type?, level?, title?, discipline?, scale? (a note like 1/4\\\" = 1'-0\\\", or drawing inches a sheet inch), plan_view? [x0, y0, x1, y1], not_built?, tables? {panel_schedules?, fixture_schedule?, legend?}}} or job.answer on the checkpoint question with that object; \"Correct\" builds on the reading as it is",
        "sheet_types": SHEET_TYPES,
        "sheets": sheets,
        "levels": index.and_then(|i| i.get("levels")),
        "flags": flags,
        "contact_sheet": contact,
    });
    let path = dir.join("sheet-index.json");
    let written = serde_json::to_vec_pretty(&packet).ok().and_then(|b| write_file(&path, &b).ok()).is_some();
    json!({
        "packet": if written { json!(path.to_string_lossy()) } else { Value::Null },
        "contact_sheet": contact.as_ref().and_then(|c| c.get("path")).cloned(),
        "pictures": dir.to_string_lossy(),
        "sheets": packet["sheets"].as_array().map_or(0, Vec::len),
        "flags": packet["flags"].clone(),
        "corrections": rules.iter().map(|(n, c)| json!({"sheet": n, "fields": c.fields()})).collect::<Vec<_>>(),
    })
}

/// The packet's summary as the classify stage keeps it (`result.sheet_index`).
pub fn summary(job: &GuidedJob) -> Option<&Value> {
    job.stage(StageName::Classify)?.result.as_ref()?.get("sheet_index")
}

/// The report's sheet index: whether the reading was checked (answered by someone, not taken by
/// default), its flags and, unchecked, each flag as built on an unchecked reading.
pub fn report(job: &GuidedJob) -> Value {
    let Some(sum) = summary(job) else { return Value::Null };
    let q = job.stage(StageName::Classify).and_then(|st| st.questions.iter().find(|q| q.question == SHEET_INDEX_QUESTION));
    let checked = q.is_some_and(|q| q.answer.is_some() && !matches!(q.answered_by.as_deref(), Some("default") | Some("go_on") | None));
    let flags = flags(job);
    let unchecked: Vec<String> =
        if checked { Vec::new() } else { flags.iter().take(MAX_REPORTED).map(|f| format!("built on an unchecked reading: {}", f.says)).collect() };
    json!({
        "packet": sum.get("packet"),
        "contact_sheet": sum.get("contact_sheet"),
        "checked": checked,
        "answered_by": q.and_then(|q| q.answered_by.clone()),
        "flags": flags.iter().take(MAX_REPORTED).collect::<Vec<_>>(),
        "flags_total": flags.len(),
        "unchecked": unchecked,
        "corrections": learned(job).iter().map(|(n, c)| json!({"sheet": n, "fields": c.fields()})).collect::<Vec<_>>(),
        "levels_not_built": levels_without_floor_plan(job).iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(),
    })
}

/// The tables answers gave, every sheet's: panel schedules, fixture types and legend rows by
/// the sheet they are of.
pub fn answered_tables(job: &GuidedJob) -> Vec<(String, SheetTables)> {
    learned(job).into_iter().filter_map(|(n, c)| c.tables.map(|t| (n, t))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job_with_index(index: Value) -> GuidedJob {
        let mut job = GuidedJob::new("/prints", super::super::guided::JobGoal::Bim);
        job.index = Some(index);
        job
    }

    fn index() -> Value {
        json!({
            "sheets": [
                {"number": "A-1", "title": "FIRST FLOOR PLAN", "discipline": "A", "type": "plan", "level": "1", "levels": ["1"], "level_source": "title", "sources": ["set.pdf#1"]},
                {"number": "A-2", "title": "SECOND FLOOR PLAN NOTES", "discipline": "A", "type": "schedule", "level": "", "levels": [], "level_source": "", "sources": ["set.pdf#2"]},
                {"number": "A-3", "title": "SECOND FLOOR FINISH PLAN", "discipline": "A", "type": "plan", "level": "2", "levels": ["2"], "level_source": "title", "sources": ["set.pdf#3"]},
                {"number": "E-201", "title": "POWER PLAN", "discipline": "E", "type": "plan", "level": "", "levels": [], "level_source": "ask", "sources": ["set.pdf#4"]},
                {"number": "E-601", "title": "PANEL SCHEDULES", "discipline": "E", "type": "schedule", "level": "", "levels": [], "sources": ["set.pdf#5"]}
            ],
            "levels": [{"name": "1", "sheets": [{"sheet": "A-1", "typical": false}]}, {"name": "2", "sheets": [{"sheet": "A-3", "typical": false}]}],
            "pages": {
                "set.pdf#1": {"paper": [36.0, 24.0], "scale": 48.0, "scale_source": "scale note 1/4\" = 1'-0\"", "views": []},
                "set.pdf#2": {"paper": [36.0, 24.0], "scale": 48.0, "scale_source": "scale note 1/4\" = 1'-0\"", "views": []},
                "set.pdf#3": {"paper": [36.0, 24.0], "scale": 48.0, "scale_source": "scale note 1/4\" = 1'-0\"", "views": []},
                "set.pdf#4": {"paper": [36.0, 24.0], "scale": 1.0, "scale_source": "none", "views": []},
                "set.pdf#5": {"paper": [36.0, 24.0], "scale": 1.0, "scale_source": "none", "views": [], "tables": {"panel_schedules": 0, "circuits": 0, "fixture_types": 0, "legend_rows": 0}}
            }
        })
    }

    #[test]
    fn the_flags_name_a_level_without_floor_plan_a_plan_typed_otherwise_a_table_read_empty_and_a_trade_plan_on_no_level() {
        let job = job_with_index(index());
        let f = flags(&job);
        let kinds: Vec<(&str, &str, &str)> = f.iter().map(|f| (f.kind, f.sheet.as_str(), f.level.as_str())).collect();
        assert!(kinds.contains(&("no_floor_plan", "", "2")), "{kinds:?}");
        assert!(kinds.contains(&("plan_title_not_plan", "A-2", "")), "{kinds:?}");
        assert!(kinds.contains(&("table_no_rows", "E-601", "")), "{kinds:?}");
        assert!(kinds.contains(&("trade_plan_no_level", "E-201", "")), "{kinds:?}");
        assert!(!kinds.iter().any(|k| k.0 == "no_floor_plan" && k.2 == "1"), "level 1 has its floor plan: {kinds:?}");
    }

    #[test]
    fn one_answer_corrects_several_sheets_and_moves_them_between_levels() {
        let mut job = job_with_index(index());
        job.sheets = Some(json!({"sheets": index()["sheets"].clone()}));
        let c = parse_corrections(
            &job,
            &json!({"A-2": {"type": "plan", "level": "Level 2"}, "E-201": {"level": "1", "scale": "1/4\" = 1'-0\""}, "a-3": {"not_built": true}}),
        )
        .expect("good corrections");
        assert_eq!(c.len(), 3);
        let effect = learn(&mut job, c, "Q-classify-1");
        assert!(effect.contains("A-2: type, level"), "{effect}");
        let i = job.index.as_ref().expect("index");
        let a2 = i["sheets"].as_array().and_then(|a| a.iter().find(|s| s["number"] == "A-2")).cloned().unwrap_or_default();
        assert_eq!((a2["type"].as_str(), a2["level"].as_str(), a2["level_source"].as_str()), (Some("plan"), Some("2"), Some("answer")));
        let l2: Vec<String> = i["levels"]
            .as_array()
            .and_then(|a| a.iter().find(|l| l["name"] == "2"))
            .map(|l| l["sheets"].as_array().map(|s| s.iter().filter_map(|x| x["sheet"].as_str().map(str::to_string)).collect()).unwrap_or_default())
            .unwrap_or_default();
        assert_eq!(l2, ["A-2"], "A-3 is not built, A-2 is level 2's plan: {i}");
        let e1 = job
            .sheets
            .as_ref()
            .and_then(|s| s["sheets"].as_array().and_then(|a| a.iter().find(|x| x["number"] == "E-201")).cloned())
            .unwrap_or_default();
        assert_eq!(e1["scale_given"].as_f64(), Some(48.0));
        let kinds: Vec<&str> = flags(&job).iter().map(|f| f.kind).collect();
        assert!(!kinds.contains(&"no_floor_plan") && !kinds.contains(&"plan_title_not_plan") && !kinds.contains(&"trade_plan_no_level"), "{kinds:?}");
        assert!(job.rules.iter().any(|r| r.key == "sheet:A-2"), "learned as a rule");
        // A second answer adds to the sheet's corrections.
        let more = parse_corrections(&job, &json!({"A-2": {"title": "SECOND FLOOR PLAN"}})).expect("title");
        learn(&mut job, more, "job.sheets.set");
        let a2 = learned(&job).get("A-2").cloned().unwrap_or_default();
        assert_eq!((a2.kind.as_deref(), a2.title.as_deref()), (Some("plan"), Some("SECOND FLOOR PLAN")));
    }

    #[test]
    fn wrong_answers_are_messages_never_panics() {
        let job = job_with_index(index());
        for (bad, says) in [
            (json!({"Z-9": {"type": "plan"}}), "not in the set's sheet index"),
            (json!({"A-1": {"plan_view": [10, 10, 5, 20]}}), "plan_view must be"),
            (json!({"A-1": {"plan_view": [1, 2, 3]}}), "plan_view must be"),
            (json!({"A-1": {"plan_view": [1, 1, 50, 20]}}), "reaches off"),
            (json!({"A-1": {"scale": 0}}), "more than 0"),
            (json!({"A-1": {"scale": -48}}), "more than 0"),
            (json!({"A-1": {"scale": "big"}}), "doesn't read as a scale"),
            (json!({"A-1": {"type": "castle"}}), "is none of"),
            (json!({"A-1": {"colour": "red"}}), "not a sheet field"),
            (json!({"A-1": {}}), "no correction"),
            (json!({"A-1": "plan"}), "as an object"),
            (json!({"A-1": {"level": "!!"}}), "not a level code"),
            (json!({"E-601": {"tables": {"panel_schedules": [{"circuits": []}]}}}), "panel's tag"),
            (json!({"E-601": {"tables": {"panel_schedules": "rows"}}}), "tables.panel_schedules"),
            (json!({"E-601": {"tables": {"chairs": []}}}), "none of"),
            (json!([1, 2]), "give the corrections"),
            (json!({}), "no sheet corrections"),
        ] {
            let e = parse_corrections(&job, &bad).expect_err("refused");
            assert!(e.contains(says), "{bad} → {e}");
        }
    }

    #[test]
    fn table_answers_take_the_readers_shape_with_the_answer_as_their_source() {
        let job = job_with_index(index());
        let c = parse_corrections(
            &job,
            &json!({"E-601": {"tables": {
                "panel_schedules": [{"panel": "P1", "circuits": [{"circuit": "1", "description": "LIGHTS"}]}],
                "fixture_schedule": [{"tag": "F1", "description": "2X4 TROFFER"}],
                "legend": [{"type": "OS", "description": "OCCUPANCY SENSOR"}]
            }}}),
        )
        .expect("tables");
        let t = c.first().and_then(|(_, c)| c.tables.clone()).unwrap_or_default();
        assert_eq!(t.panel_schedules.first().map(|p| p.source.as_str()), Some("E-601 (answer, from the sheet image)"));
        assert_eq!(t.fixture_schedule.first().map(|f| f.source.as_str()), Some("E-601 (answer, from the sheet image)"));
        assert_eq!(t.legend.get("OS").map(String::as_str), Some("OCCUPANCY SENSOR"));
    }

    #[test]
    fn a_plan_off_its_peers_scale_and_a_mixed_sheet_are_flagged() {
        let mut i = index();
        for (n, src) in [("A-4", "set.pdf#6"), ("A-5", "set.pdf#7")] {
            if let Some(a) = i["sheets"].as_array_mut() {
                a.push(json!({"number": n, "title": "THIRD FLOOR PLAN", "discipline": "A", "type": "plan", "level": "3", "levels": ["3"], "level_source": "title", "sources": [src]}));
            }
        }
        i["pages"]["set.pdf#6"] = json!({"paper": [36.0, 24.0], "scale": 4.0, "scale_source": "scale note 3\" = 1'-0\"", "views": [
            {"title": "THIRD FLOOR PLAN", "scale_text": "1/4\" = 1'-0\"", "scale": 48.0, "rect": [1.0, 1.0, 20.0, 20.0]},
            {"title": "WALL DETAIL", "scale_text": "3\" = 1'-0\"", "scale": 4.0, "rect": [22.0, 1.0, 30.0, 10.0]}
        ]});
        i["pages"]["set.pdf#7"] = json!({"paper": [36.0, 24.0], "scale": 48.0, "scale_source": "scale note", "views": []});
        let job = job_with_index(i);
        let f = flags(&job);
        assert!(f.iter().any(|f| f.kind == "plan_off_scale" && f.sheet == "A-4"), "{f:?}");
        assert!(f.iter().any(|f| f.kind == "mixed_sheet" && f.sheet == "A-4" && f.says.contains("WALL DETAIL")), "{f:?}");
        assert!(!f.iter().any(|f| f.sheet == "A-5"), "{f:?}");
    }

    fn region(number: &str, level: &str, discipline: &str, x0: f64) -> super::super::guided::SheetRegion {
        super::super::guided::SheetRegion {
            number: number.into(),
            level: level.into(),
            discipline: discipline.into(),
            kind: "plan".into(),
            rect: [x0, 0.0, x0 + 36.0 * 48.0, 24.0 * 48.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        }
    }

    #[test]
    fn a_level_shown_only_by_a_finish_plan_is_not_built_from_it_until_an_answer_says_so() {
        let mut job = job_with_index(index());
        job.sheets = Some(json!({"sheets": index()["sheets"].clone()}));
        let regions = vec![region("A-1", "1", "A", 0.0), region("A-3", "2", "A", 2000.0), region("E-201", "2", "E", 4000.0)];
        let sheets: Vec<Value> = job.sheets.as_ref().and_then(|s| s["sheets"].as_array().cloned()).unwrap_or_default();
        let refs: Vec<&Value> = sheets.iter().collect();
        let out = super::super::guided::answered_regions(&job, regions.clone(), &refs, &[]);
        let nb: Vec<(&str, &str)> = out.iter().map(|r| (r.number.as_str(), r.not_built.as_str())).collect();
        assert_eq!(nb, [("A-1", ""), ("A-3", NO_FLOOR_PLAN), ("E-201", "")], "the finish plan is refused, the trade plan read");
        // The answer: the finish plan is the level's plan after all (built), the trade plan left out.
        let c = parse_corrections(&job, &json!({"A-3": {"type": "plan"}, "E-201": {"not_built": true}})).expect("answer");
        learn(&mut job, c, "Q-classify-1");
        let sheets: Vec<Value> = job.sheets.as_ref().and_then(|s| s["sheets"].as_array().cloned()).unwrap_or_default();
        let refs: Vec<&Value> = sheets.iter().collect();
        let out = super::super::guided::answered_regions(&job, regions, &refs, &[]);
        let nb: Vec<(&str, &str)> = out.iter().map(|r| (r.number.as_str(), r.not_built.as_str())).collect();
        assert_eq!(nb, [("A-1", ""), ("A-3", ""), ("E-201", NOT_BUILT_ANSWER)]);
    }

    #[test]
    fn a_plan_view_answer_is_the_sheets_one_view_on_the_drawing() {
        let mut job = job_with_index(index());
        job.sheets = Some(json!({"sheets": index()["sheets"].clone()}));
        let c = parse_corrections(&job, &json!({"A-1": {"plan_view": [1.0, 2.0, 11.0, 12.0]}})).expect("answer");
        learn(&mut job, c, "job.sheets.set");
        let sheets: Vec<Value> = job.sheets.as_ref().and_then(|s| s["sheets"].as_array().cloned()).unwrap_or_default();
        let refs: Vec<&Value> = sheets.iter().collect();
        let scales = vec![SheetScale {
            sheet: "A-1".into(),
            kind: "plan".into(),
            scale: 48.0,
            source: "scale note".into(),
            plan_scale: None,
            paper: Some([36.0, 24.0]),
        }];
        let split = vec![region("A-1", "1", "A", 1000.0), region("A-1", "1", "A", 1000.0)];
        let out = super::super::guided::answered_regions(&job, split, &refs, &scales);
        assert_eq!(out.len(), 1, "one region of the sheet");
        let v = out.first().and_then(|r| r.view.clone()).expect("its view");
        assert_eq!(v.rect, [1000.0 + 48.0, 96.0, 1000.0 + 528.0, 576.0]);
    }

    #[test]
    fn answered_tables_go_over_the_readers_rows_of_the_same_panel_or_type() {
        let mut job = job_with_index(index());
        let c = parse_corrections(
            &job,
            &json!({"E-601": {"tables": {
                "panel_schedules": [{"panel": "P1", "circuits": [{"number": "1", "description": "LIGHTS"}, {"number": "3", "description": "RECEPTACLES"}]}],
                "fixture_schedule": [{"tag": "F1", "description": "2X4 TROFFER"}]
            }}}),
        )
        .expect("tables");
        learn(&mut job, c, "Q-classify-1");
        let read = vec![
            cadaddy_walk3d::PanelSchedule { panel: "p1".into(), source: "E-601".into(), ..Default::default() },
            cadaddy_walk3d::PanelSchedule { panel: "P2".into(), source: "E-601".into(), ..Default::default() },
        ];
        let panels = super::super::guided::with_answered_panels(&job, read);
        let got: Vec<(&str, usize, &str)> = panels.iter().map(|p| (p.panel.as_str(), p.circuits.len(), p.source.as_str())).collect();
        assert_eq!(got, [("P2", 0, "E-601"), ("P1", 2, "E-601 (answer, from the sheet image)")]);
        assert!(panels.iter().flat_map(|p| &p.circuits).all(|c| c.source.contains("answer")));
        let fixtures =
            super::super::guided::with_answered_fixtures(&job, vec![cadaddy_walk3d::FixtureType { tag: "F1".into(), ..Default::default() }]);
        assert_eq!(fixtures.iter().map(|f| (f.tag.as_str(), f.description.as_str())).collect::<Vec<_>>(), [("F1", "2X4 TROFFER")]);
        let rule = job.rules.iter().find(|r| r.key == "sheet:E-601").map(|r| r.value.clone()).unwrap_or_default();
        assert!(rule.contains("\"source\":\"answer\""), "{rule}");
    }

    #[test]
    fn a_finish_plan_is_no_floor_plan() {
        assert!(!is_floor_plan_title("SECOND FLOOR FINISH PLAN"));
        assert!(!is_floor_plan_title("FURNITURE PLAN - LEVEL 2"));
        assert!(is_floor_plan_title("SECOND FLOOR PLAN"));
        assert!(is_floor_plan_title("LEVEL 2 PLAN"));
    }
}
