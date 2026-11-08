//! Door and window schedules: `schedules.read` finds them on the sheets — TABLE objects, or grids
//! of text under a header row with a MARK column — and reads each row's mark, width and height
//! (or a size: `3'-0" x 7'-0"`, `36x84`, the door code `3070`), type, frame, fire rating,
//! hardware set, sill and head. It finds the tags on the plans (a mark inside a small circle or
//! outline) and flags tags with no schedule row and rows with no tag. `apply: true` keeps the rows
//! and tags in the building spec, where WALLGEN sizes each tagged opening (its width, its head
//! and sill) from its row.
//!
//! What a sheet's frame holds is never a schedule or a tag: the title block and the border strip
//! (the job's sheets, else the drawing's own frame) are left out before tables are found. A
//! column of marks is headed by a mark heading (`MARK`, `DOOR NO.`), never by a title-block
//! field (`PROJECT NO.`); a row needs a mark in that column that fits the schedule's marks (one
//! short code) and something read beside it; a table titled as another schedule (finishes,
//! columns, fixtures) is not a door or window schedule.

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::{FamilyKind, OpeningTag, ScheduleRow};
use serde_json::{Value, json};
use std::sync::Arc;

use super::bspec::{current, plan_objects};
use super::perceive::{feet_inches, texts, units_per_inch};
use super::*;
use crate::{Result, Session};
use cadaddy_mepfix::wordrules;

/// Rows and tags read, at most (hostile drawings).
const MAX_ROWS: usize = 5_000;
/// Sheets read from the parameters or the job, at most.
const MAX_SHEETS: usize = 5_000;
/// The longest mark (characters): a mark is a short code (`12A`, `D-12`, `W1`).
const MAX_MARK_LEN: usize = 8;
/// A mark of letters alone has at most this many (`A`, `SF`, `ST1` has a digit).
const MAX_LETTER_MARK: usize = 3;
/// Texts turned within this of a header's angle (radians) are on its rows.
const SAME_ANGLE: f64 = 0.05;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("schedules.read", "Read Door and Window Schedules", run).params(
        "{apply?: bool (keep the rows and tags in the building spec: WALLGEN sizes the tagged openings from them), sheets?: [{rect: [x0,y0,x1,y1], area?: [x0,y0,x1,y1], title_block?: [x0,y0,x1,y1]}] (the sheets' frames: what lies in a title block or outside a sheet's drawing area is never read; default the guided job's sheets, else the drawing's own frame)} → {schedules: [{kind, title, from: table|text, rows: [{mark, width_in?, height_in?, sill_ft?, head_ft?, type?, frame?, fire_rating?, hardware?}]}], tags: [{mark, at, handle, row}], flags: {tags_without_row: [{mark, handle}], rows_without_tag: [{kind, mark}]}}",
    )]
}

/// What a header names.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Col {
    Mark,
    Width,
    Height,
    Size,
    Type,
    Frame,
    Fire,
    Hardware,
    Sill,
    Head,
}

/// A heading of a column of opening marks: a mark word (`MARK`, `NO.`, `#`) after nothing but
/// words naming the opening (`DOOR NO.`, `WINDOW MARK`). A title-block field (`PROJECT NO.`,
/// `SHEET NO.`) names something else, so it heads no column of marks.
fn mark_heading(u: &str) -> bool {
    let words: Vec<&str> = u.split(|c: char| !(c.is_ascii_alphanumeric() || c == '#')).filter(|w| !w.is_empty()).collect();
    let marks = wordrules::words("schedules.opening_mark_words");
    let qualifiers = wordrules::words("schedules.opening_mark_qualifiers");
    words.split_last().is_some_and(|(last, rest)| marks.contains(last) && rest.iter().all(|w| qualifiers.contains(w) || marks.contains(w)))
}

fn column(h: &str) -> Option<Col> {
    let u = h.trim().to_ascii_uppercase();
    let has = |w: &str| u.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == w);
    Some(if mark_heading(&u) {
        Col::Mark
    } else if has("WIDTH") || u == "W" {
        Col::Width
    } else if has("HEIGHT") || u == "H" || has("HT") {
        Col::Height
    } else if has("SIZE") {
        Col::Size
    } else if has("FRAME") {
        Col::Frame
    } else if has("FIRE") || has("RATING") || has("LABEL") {
        Col::Fire
    } else if has("HARDWARE") || has("HDW") || has("HW") {
        Col::Hardware
    } else if has("SILL") {
        Col::Sill
    } else if has("HEAD") {
        Col::Head
    } else if has("TYPE") {
        Col::Type
    } else {
        return None;
    })
}

/// A length in inches: `3'-0"`, `36"`, `36`, `3'`.
fn inches(t: &str) -> Option<f64> {
    let t = t.trim();
    t.parse::<f64>().ok().or_else(|| feet_inches(t)).filter(|v| v.is_finite() && *v > 0.0 && *v <= 1200.0)
}

/// A size: `3'-0" x 7'-0"`, `36x84`, `36 X 84`, or the door code `3070` (3'-0" × 7'-0").
pub(crate) fn size(t: &str) -> Option<(f64, f64)> {
    let u = t.trim().to_ascii_uppercase().replace('×', "X");
    if let Some((a, b)) = u.split_once('X') {
        return Some((inches(a)?, inches(b)?));
    }
    let digits: String = u.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.len() == 4 && digits.chars().all(|c| c.is_ascii_digit()) {
        let d = |i: usize| digits.get(i..=i).and_then(|c| c.parse::<f64>().ok());
        return Some((d(0)? * 12.0 + d(1)?, d(2)? * 12.0 + d(3)?));
    }
    None
}

fn kind_of(title: &str) -> Option<FamilyKind> {
    let u = title.to_ascii_uppercase();
    if u.contains("WINDOW") || u.contains("GLAZING") || u.contains("STOREFRONT") {
        Some(FamilyKind::Window)
    } else if u.contains("DOOR") || u.contains("OPENING") {
        Some(FamilyKind::Door)
    } else {
        None
    }
}

/// The kind of openings a schedule under `title` lists: its title's, a door schedule when it has
/// none; `None` for a schedule titled as another one (finishes, columns, fixtures).
fn schedule_kind(title: &str) -> Option<FamilyKind> {
    match kind_of(title) {
        Some(k) => Some(k),
        None if title.to_ascii_uppercase().contains("SCHEDULE") => None,
        None => Some(FamilyKind::Door),
    }
}

/// A mark: one short code (`12`, `12A`, `D-12`, `W1`, `A`), never words, a phrase or a
/// title-block field's label (`DATE`, `NO.`, `BY`).
fn mark_like(m: &str) -> bool {
    let m = m.trim();
    let n = m.chars().count();
    if n == 0 || n > MAX_MARK_LEN || !m.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '/')) {
        return false;
    }
    let letters = m.chars().filter(char::is_ascii_alphabetic).count();
    let digits = m.chars().filter(char::is_ascii_digit).count();
    if letters + digits == 0 || (digits == 0 && letters > MAX_LETTER_MARK) {
        return false;
    }
    let word: String = m.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase();
    !wordrules::words("schedules.not_marks").contains(&word.as_str())
}

/// Whether the row says anything beside its mark (a size, a type, a frame…).
fn has_detail(r: &ScheduleRow) -> bool {
    r.width_in.is_some()
        || r.height_in.is_some()
        || r.sill_ft.is_some()
        || r.head_ft.is_some()
        || r.kind_type.is_some()
        || r.frame.is_some()
        || r.fire_rating.is_some()
        || r.hardware.is_some()
}

/// The rows whose marks fit the schedule's: each mark one code with something beside it; where
/// most marks (three in four, of three or more) carry a number (`12`, `D-2`), a mark of letters
/// alone is a stray word, not a row.
fn fitting(rows: Vec<ScheduleRow>) -> Vec<ScheduleRow> {
    let mut rows: Vec<ScheduleRow> = rows.into_iter().filter(|r| mark_like(&r.mark) && has_detail(r)).collect();
    let numbered = rows.iter().filter(|r| r.mark.chars().any(|c| c.is_ascii_digit())).count();
    if rows.len() >= 3 && numbered * 4 >= rows.len() * 3 {
        rows.retain(|r| r.mark.chars().any(|c| c.is_ascii_digit()));
    }
    rows
}

/// A row from its cells under their columns.
fn row(kind: FamilyKind, cols: &[Option<Col>], cells: &[String]) -> Option<ScheduleRow> {
    let get = |c: Col| cols.iter().position(|x| *x == Some(c)).and_then(|i| cells.get(i)).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let mark = get(Col::Mark)?;
    let (mut w, mut h) = (get(Col::Width).and_then(|v| inches(&v)), get(Col::Height).and_then(|v| inches(&v)));
    if let Some((sw, sh)) = get(Col::Size).and_then(|v| size(&v)) {
        w = w.or(Some(sw));
        h = h.or(Some(sh));
    }
    Some(ScheduleRow {
        kind,
        mark,
        width_in: w,
        height_in: h,
        sill_ft: get(Col::Sill).and_then(|v| inches(&v)).map(|i| i / 12.0),
        head_ft: get(Col::Head).and_then(|v| inches(&v)).map(|i| i / 12.0),
        kind_type: get(Col::Type),
        frame: get(Col::Frame),
        fire_rating: get(Col::Fire),
        hardware: get(Col::Hardware),
    })
}

struct Found {
    kind: FamilyKind,
    title: String,
    from: &'static str,
    rows: Vec<ScheduleRow>,
    /// The texts and objects it is made of (not tags).
    used: Vec<Handle>,
}

/// Schedules drawn as TABLE objects.
fn tables(ents: &[&Arc<Entity>]) -> Vec<Found> {
    let mut out = Vec::new();
    for e in ents {
        let EntityKind::Table(t) = &e.kind else { continue };
        let text = |r: &Vec<cadaddy_doc::TableCell>| r.iter().map(|c| cadaddy_fonts::plain_text(&c.text)).collect::<Vec<String>>();
        let rows: Vec<Vec<String>> = t.cells.iter().take(MAX_ROWS).map(text).collect();
        let Some(hi) = rows.iter().position(|r| r.iter().any(|c| column(c) == Some(Col::Mark))) else { continue };
        let cols: Vec<Option<Col>> = rows.get(hi).map(|r| r.iter().map(|c| column(c)).collect()).unwrap_or_default();
        if !cols.iter().any(|c| matches!(c, Some(Col::Width | Col::Height | Col::Size))) {
            continue;
        }
        let title = rows.iter().take(hi).flatten().find(|c| !c.trim().is_empty()).cloned().unwrap_or_default();
        let Some(kind) = schedule_kind(&title) else { continue };
        let found = fitting(rows.iter().skip(hi + 1).filter_map(|r| row(kind, &cols, r)).collect());
        out.push(Found { kind, title, from: "table", rows: found, used: vec![e.handle] });
    }
    out
}

/// A text: handle, text as shown, where, height, angle (radians).
type Turned = (Handle, String, Vec2, f64, f64);

/// The texts with their angles.
fn turned_texts(ents: &[&Arc<Entity>]) -> Vec<Turned> {
    let mut out = Vec::new();
    for e in ents {
        match &e.kind {
            EntityKind::Text(t) => out.push((e.handle, t.value.trim().to_string(), t.justify_point(), t.height, t.rotation)),
            EntityKind::MText(t) => out.push((
                e.handle,
                cadaddy_fonts::plain_text(&t.contents).split_whitespace().collect::<Vec<_>>().join(" "),
                t.insert.xy(),
                t.height,
                t.rotation,
            )),
            _ => {}
        }
    }
    out
}

/// `p` seen along a text turned `a`: its baseline along x, its lines going down in y.
fn unturn(p: Vec2, a: f64) -> Vec2 {
    let (sn, cs) = a.sin_cos();
    Vec2::new(p.x * cs + p.y * sn, -p.x * sn + p.y * cs)
}

fn same_angle(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(std::f64::consts::TAU);
    d.is_finite() && d.min(std::f64::consts::TAU - d) <= SAME_ANGLE
}

/// Schedules drawn as text in rows and columns under a header with a MARK: the texts of the
/// header's sheet turned as it is, read along its baseline.
fn text_grids(ents: &[&Arc<Entity>], sheets: &[Frame]) -> Vec<Found> {
    let raw = turned_texts(ents);
    let homes: Vec<Option<usize>> = raw.iter().map(|x| sheets.iter().position(|f| inside(&f.rect, x.2))).collect();
    let mut out = Vec::new();
    let mut used_headers: std::collections::HashSet<Handle> = std::collections::HashSet::new();
    for (i, (h, t, at, ht, angle)) in raw.iter().enumerate() {
        if column(t) != Some(Col::Mark) || used_headers.contains(h) {
            continue;
        }
        let home = homes.get(i).copied().flatten();
        let all: Vec<(Handle, String, Vec2, f64)> = raw
            .iter()
            .zip(&homes)
            .filter(|(x, hm)| **hm == home && same_angle(x.4, *angle))
            .map(|(x, _)| (x.0, x.1.clone(), unturn(x.2, *angle), x.3))
            .collect();
        let (at, ht) = (unturn(*at, *angle), *ht);
        let tol = ht.max(1e-9) * 0.6;
        // The header row: texts on the same line naming columns.
        let mut header: Vec<(f64, Option<Col>, Handle)> =
            all.iter().filter(|(_, _, p, _)| (p.y - at.y).abs() <= tol).map(|(hh, tt, p, _)| (p.x, column(tt), *hh)).collect();
        header.sort_by(|a, b| a.0.total_cmp(&b.0));
        if !header.iter().any(|c| matches!(c.1, Some(Col::Width | Col::Height | Col::Size))) {
            continue;
        }
        used_headers.extend(header.iter().map(|c| c.2));
        let (x0, x1) = (header.first().map_or(at.x, |c| c.0) - 2.0 * ht, header.last().map_or(at.x, |c| c.0) + 20.0 * ht);
        // The rows below, line by line, until a gap of more than three lines.
        let mut below: Vec<&(Handle, String, Vec2, f64)> = all.iter().filter(|(_, _, p, _)| p.y < at.y - tol && p.x >= x0 && p.x <= x1).collect();
        below.sort_by(|a, b| b.2.y.total_cmp(&a.2.y));
        let mut lines: Vec<Vec<&(Handle, String, Vec2, f64)>> = Vec::new();
        let mut last_y = at.y;
        let mut step: Option<f64> = None;
        for t in below {
            match lines.last_mut() {
                Some(l) if l.first().is_some_and(|f| (f.2.y - t.2.y).abs() <= tol) => l.push(t),
                _ => {
                    let gap = last_y - t.2.y;
                    if step.is_some_and(|s| gap > 3.0 * s) || (step.is_none() && gap > 6.0 * ht) || lines.len() >= MAX_ROWS {
                        break;
                    }
                    if !lines.is_empty() {
                        step.get_or_insert(gap);
                    }
                    last_y = t.2.y;
                    lines.push(vec![t]);
                }
            }
        }
        // Each text under the column that starts at or before it (the nearest header to its left).
        let cols: Vec<Option<Col>> = header.iter().map(|c| c.1).collect();
        let cells_of = |l: &Vec<&(Handle, String, Vec2, f64)>| {
            let mut cells = vec![String::new(); header.len()];
            for t in l {
                let i = header.iter().rposition(|c| c.0 <= t.2.x + ht).unwrap_or(0);
                if let Some(c) = cells.get_mut(i) {
                    if !c.is_empty() {
                        c.push(' ');
                    }
                    c.push_str(&t.1);
                }
            }
            cells
        };
        // The title above: a SCHEDULE text within a few lines.
        let title = all
            .iter()
            .filter(|(_, tt, p, _)| {
                p.y > at.y && p.y <= at.y + 6.0 * ht && p.x >= x0 - 10.0 * ht && p.x <= x1 && tt.to_ascii_uppercase().contains("SCHEDULE")
            })
            .min_by(|a, b| a.2.y.total_cmp(&b.2.y))
            .map(|(_, tt, _, _)| tt.clone())
            .unwrap_or_default();
        let Some(kind) = schedule_kind(&title) else { continue };
        let rows = fitting(lines.iter().filter_map(|l| row(kind, &cols, &cells_of(l))).collect());
        let mut used: Vec<Handle> = header.iter().map(|c| c.2).collect();
        used.extend(lines.iter().flatten().map(|t| t.0));
        out.push(Found { kind, title, from: "text", rows, used });
    }
    out
}

/// A sheet's frame: its rectangle, the part its views are drawn in, and its title block.
#[derive(Clone, Copy, Debug)]
struct Frame {
    rect: [f64; 4],
    area: Option<[f64; 4]>,
    title_block: Option<[f64; 4]>,
}

fn inside(r: &[f64; 4], p: Vec2) -> bool {
    p.x >= r[0].min(r[2]) && p.x <= r[0].max(r[2]) && p.y >= r[1].min(r[3]) && p.y <= r[1].max(r[3])
}

/// Whether `p` is on a sheet's frame: in its title block, or off its drawing area (the border
/// strip and the margin).
fn on_frame(sheets: &[Frame], p: Vec2) -> bool {
    sheets.iter().any(|f| inside(&f.rect, p) && (f.title_block.is_some_and(|t| inside(&t, p)) || f.area.is_some_and(|a| !inside(&a, p))))
}

/// Where an object is read: a text's or a block's insertion point, a circle's centre, an
/// outline's first corner. `None` for objects the reader never reads.
fn anchor(e: &Entity) -> Option<Vec2> {
    match &e.kind {
        EntityKind::Text(t) => Some(t.justify_point()),
        EntityKind::MText(t) => Some(t.insert.xy()),
        EntityKind::Table(t) => Some(t.insert.xy()),
        EntityKind::Insert(i) => Some(i.insert.xy()),
        EntityKind::Circle(c) => Some(c.center.xy()),
        EntityKind::LwPolyline(p) => p.vertices.first().map(|v| v.p),
        _ => None,
    }
}

/// The sheets' frames: the `sheets` parameter, else the guided job's sheets, else the
/// drawing's own frame (one sheet: its border and title block, when its lines show them).
fn frames(s: &Session, p: &Value) -> Result<Vec<Frame>> {
    let rect = |v: Option<&Value>| -> Option<[f64; 4]> {
        let a: Vec<f64> = v?.as_array()?.iter().filter_map(Value::as_f64).filter(|x| x.is_finite()).collect();
        <[f64; 4]>::try_from(a).ok()
    };
    if let Some(a) = p.get("sheets").and_then(Value::as_array) {
        return Ok(a
            .iter()
            .take(MAX_SHEETS)
            .filter_map(|x| Some(Frame { rect: rect(x.get("rect"))?, area: rect(x.get("area")), title_block: rect(x.get("title_block")) }))
            .collect());
    }
    if let Some(job) = super::guided::load_job(s).ok().flatten().filter(|j| !j.sheet_regions.is_empty()) {
        return Ok(job.sheet_regions.iter().take(MAX_SHEETS).map(|r| Frame { rect: r.rect, area: r.area, title_block: r.title_block }).collect());
    }
    let d = s.doc()?;
    let b = d.extents(&cadaddy_doc::Space::Model);
    if b.is_empty() || !(b.min.x.is_finite() && b.min.y.is_finite() && b.max.x.is_finite() && b.max.y.is_finite()) {
        return Ok(Vec::new());
    }
    let r = [b.min.x, b.min.y, b.max.x, b.max.y];
    Ok(super::guided::sheet_frames(d, &[r]).into_iter().map(|f| Frame { rect: r, area: Some(f.area), title_block: f.title_block }).collect())
}

/// Tags: short marks inside a small circle or closed outline (a tag symbol), or attributes of a
/// block named like a tag.
pub(crate) fn tags(d: &Drawing, ents: &[&Arc<Entity>], skip: &[Handle]) -> Vec<(String, Vec2, Handle)> {
    let k = units_per_inch(d);
    let shapes: Vec<(Vec2, f64)> = ents
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Circle(c) if c.radius <= 12.0 * k => Some((c.center.xy(), c.radius)),
            EntityKind::LwPolyline(p) if p.closed => {
                let b = cadaddy_geom::Bounds2::from_points(p.vertices.iter().map(|v| v.p));
                (!b.is_empty() && b.width().max(b.height()) <= 24.0 * k).then(|| (b.center(), b.width().max(b.height()) / 2.0))
            }
            _ => None,
        })
        .collect();
    let short = |t: &str| {
        let t = t.trim();
        !t.is_empty() && t.len() <= 6 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    };
    let mut out = Vec::new();
    for (h, t, at, ht) in texts(ents) {
        if skip.contains(&h) || !short(&t) {
            continue;
        }
        if shapes.iter().any(|(c, r)| c.dist(at) <= r + ht) {
            out.push((t.trim().to_string(), at, h));
        }
    }
    for e in ents {
        let EntityKind::Insert(i) = &e.kind else { continue };
        if !i.block.to_ascii_uppercase().contains("TAG") {
            continue;
        }
        if let Some(a) = i.attribs.iter().find(|a| short(&a.text.value)) {
            out.push((a.text.value.trim().to_string(), i.insert.xy(), e.handle));
        }
    }
    out.truncate(MAX_ROWS);
    out
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "schedules.read")?;
    let sheets = frames(s, p)?;
    let d = s.doc()?;
    // The title block and the border strip are left out before tables are found.
    let ents: Vec<&Arc<Entity>> = plan_objects(d).into_iter().filter(|e| anchor(e).is_none_or(|a| !on_frame(&sheets, a))).collect();
    let mut found = tables(&ents);
    found.extend(text_grids(&ents, &sheets));
    let used: Vec<Handle> = found.iter().flat_map(|f| f.used.iter().copied()).collect();
    let tag_list = tags(d, &ents, &used);
    let rows: Vec<&ScheduleRow> = found.iter().flat_map(|f| f.rows.iter()).collect();
    let has_row = |m: &str| rows.iter().any(|r| r.mark.trim().eq_ignore_ascii_case(m));
    let tags_json: Vec<Value> =
        tag_list.iter().map(|(m, at, h)| json!({"mark": m, "at": [at.x, at.y], "handle": h.hex(), "row": has_row(m)})).collect();
    let tags_without_row: Vec<Value> =
        tag_list.iter().filter(|(m, _, _)| !has_row(m)).map(|(m, _, h)| json!({"mark": m, "handle": h.hex()})).collect();
    let rows_without_tag: Vec<Value> = rows
        .iter()
        .filter(|r| !tag_list.iter().any(|(m, _, _)| m.eq_ignore_ascii_case(r.mark.trim())))
        .map(|r| json!({"kind": if r.kind == FamilyKind::Door { "door" } else { "window" }, "mark": r.mark}))
        .collect();
    // Schedule rows with a TYPE that has no matching spec family.
    let types_without_family: Vec<Value> = rows
        .iter()
        .filter_map(|r| {
            let ty = r.kind_type.as_ref()?;
            if sp.family_by_type(r.kind, ty).is_some() {
                return None;
            }
            Some(json!({"kind": if r.kind == FamilyKind::Door { "door" } else { "window" }, "mark": &r.mark, "type": ty}))
        })
        .collect();
    let schedules: Vec<Value> = found
        .iter()
        .map(|f| json!({"kind": if f.kind == FamilyKind::Door { "door" } else { "window" }, "title": f.title, "from": f.from, "rows": serde_json::to_value(&f.rows).unwrap_or(Value::Null)}))
        .collect();
    let apply = bool_or(p, "apply", false);
    if apply {
        sp.schedule.rows = found.into_iter().flat_map(|f| f.rows).collect();
        sp.schedule.tags = tag_list.iter().map(|(m, at, _)| OpeningTag { mark: m.clone(), at: [at.x, at.y], ..OpeningTag::default() }).collect();
        s.doc_mut()?.building_spec = Some(sp.to_json()).into();
    }
    Ok(json!({
        "schedules": schedules,
        "tags": tags_json,
        "flags": {
            "tags_without_row": tags_without_row,
            "rows_without_tag": rows_without_tag,
            "types_without_family": types_without_family,
        },
        "applied": apply,
    }))
}

/// WALLGEN: each opening nearest a tag (within 6 feet) takes its schedule row's kind, width and
/// heights (sheet units above the floor).
pub(crate) fn size_openings(
    sp: &cadaddy_walk3d::BuildingSpec,
    ops: &mut [super::perceive::geometry::Opening],
    fitted: &mut std::collections::HashMap<Handle, (Option<f64>, Option<f64>)>,
    upf: f64,
) {
    if sp.schedule.rows.is_empty() || sp.schedule.tags.is_empty() {
        return;
    }
    let k = upf / 12.0;
    for tag in &sp.schedule.tags {
        let Some(r) = sp.schedule.row(&tag.mark) else { continue };
        let at = Vec2::new(tag.at[0], tag.at[1]);
        let Some(o) = ops.iter_mut().filter(|o| o.at.dist(at) <= 72.0 * k).min_by(|a, b| a.at.dist(at).total_cmp(&b.at.dist(at))) else { continue };
        o.kind = if r.kind == FamilyKind::Door { "door" } else { "window" };
        if let Some(w) = r.width_in {
            let new_w = w * k;
            // When the opening has a swing (arc endpoints), scale the endpoints to the schedule
            // width so the wall cut and door leaf use the schedule width, not the drawn radius.
            if let Some((_, ref mut ends)) = o.swing {
                let ratio = if o.width > 1e-9 { new_w / o.width } else { 1.0 };
                if (ratio - 1.0).abs() > 1e-9 {
                    for e in ends.iter_mut() {
                        *e = o.at + (*e - o.at) * ratio;
                    }
                }
            }
            o.width = new_w;
        }
        let (mut head, mut sill) = r.heights(sp.openings.window_sill_ft);
        // A schedule row's TYPE selects the spec family whose name matches: its width (when the
        // row has none), head and sill override the defaults.
        if let Some(ty) = &r.kind_type
            && let Some(f) = sp.family_by_type(r.kind, ty)
        {
            if r.width_in.is_none()
                && let Some(fw) = f.width_in
            {
                o.width = fw * k;
            }
            if let Some(fh) = f.head_ft {
                head = Some(fh);
            }
            if f.sill_ft.is_some() {
                sill = f.sill_ft;
            }
        }
        fitted.insert(o.handle, (head.map(|h| h * upf), sill.map(|s| s * upf)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_columns() {
        assert_eq!(size("3'-0\" x 7'-0\""), Some((36.0, 84.0)));
        assert_eq!(size("36X84"), Some((36.0, 84.0)));
        assert_eq!(size("3070"), Some((36.0, 84.0)));
        assert_eq!(size("2868"), Some((32.0, 80.0)));
        assert_eq!(size("door"), None);
        assert_eq!(column("MARK"), Some(Col::Mark));
        assert_eq!(column("DOOR NO."), Some(Col::Mark));
        assert_eq!(column("WIDTH"), Some(Col::Width));
        assert_eq!(column("FIRE RATING"), Some(Col::Fire));
        assert_eq!(column("HDW SET"), Some(Col::Hardware));
        assert_eq!(column("REMARKS"), None);
        assert_eq!(kind_of("WINDOW SCHEDULE"), Some(FamilyKind::Window));
    }

    #[test]
    fn title_block_fields_are_no_mark_headings_or_marks() {
        for h in ["MARK", "NO.", "DOOR NO.", "DOOR #", "WINDOW MARK", "TAG", "NUMBER", "OPENING NO"] {
            assert_eq!(column(h), Some(Col::Mark), "{h}");
        }
        for h in ["PROJECT NO.", "SHEET NO.", "JOB NUMBER", "DRAWING NO", "EQUIPMENT TAG"] {
            assert_ne!(column(h), Some(Col::Mark), "{h}");
        }
        for m in ["12", "12A", "D-12", "W1", "A", "SF", "1.01"] {
            assert!(mark_like(m), "{m}");
        }
        for m in ["DRAWN BY", "DATE", "ISSUED", "NO.", "BY", "REV", "D1 D3", "SEE NOTE 4", "WALL/FLOOR", ""] {
            assert!(!mark_like(m), "{m}");
        }
        assert_eq!(schedule_kind("FINISH SCHEDULE"), None);
        assert_eq!(schedule_kind("DOOR AND FRAME SCHEDULE"), Some(FamilyKind::Door));
        assert_eq!(schedule_kind(""), Some(FamilyKind::Door));
    }

    #[test]
    fn rows_fit_the_schedules_marks() {
        let r = |m: &str, w: Option<f64>| ScheduleRow {
            kind: FamilyKind::Door,
            mark: m.into(),
            width_in: w,
            height_in: None,
            sill_ft: None,
            head_ft: None,
            kind_type: None,
            frame: None,
            fire_rating: None,
            hardware: None,
        };
        let rows = vec![r("D1", Some(36.0)), r("D2", Some(36.0)), r("D3", Some(36.0)), r("L", Some(36.0)), r("D4", None)];
        let kept: Vec<String> = fitting(rows).into_iter().map(|x| x.mark).collect();
        assert_eq!(kept, vec!["D1", "D2", "D3"]);
        // Window types of letters alone keep theirs.
        let rows = vec![r("A", Some(48.0)), r("B", Some(36.0)), r("C", Some(24.0))];
        assert_eq!(fitting(rows).len(), 3);
    }
}
