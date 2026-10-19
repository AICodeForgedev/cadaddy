//! Door and window schedules: `schedules.read` finds them on the sheets — TABLE objects, or grids
//! of text under a header row with a MARK column — and reads each row's mark, width and height
//! (or a size: `3'-0" x 7'-0"`, `36x84`, the door code `3070`), type, frame, fire rating,
//! hardware set, sill and head. It finds the tags on the plans (a mark inside a small circle or
//! outline) and flags tags with no schedule row and rows with no tag. `apply: true` keeps the rows
//! and tags in the building spec, where WALLGEN sizes each tagged opening (its width, its head
//! and sill) from its row.

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::{FamilyKind, OpeningTag, ScheduleRow};
use serde_json::{Value, json};
use std::sync::Arc;

use super::bspec::{current, plan_objects};
use super::perceive::{feet_inches, texts, units_per_inch};
use super::*;
use crate::{Result, Session};

/// Rows and tags read, at most (hostile drawings).
const MAX_ROWS: usize = 5_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("schedules.read", "Read Door and Window Schedules", run).params(
        "{apply?: bool (keep the rows and tags in the building spec: WALLGEN sizes the tagged openings from them)} → {schedules: [{kind, title, from: table|text, rows: [{mark, width_in?, height_in?, sill_ft?, head_ft?, type?, frame?, fire_rating?, hardware?}]}], tags: [{mark, at, handle, row}], flags: {tags_without_row: [{mark, handle}], rows_without_tag: [{kind, mark}]}}",
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

fn column(h: &str) -> Option<Col> {
    let u = h.trim().to_ascii_uppercase();
    let has = |w: &str| u.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == w);
    Some(if has("MARK") || has("NO") || u == "NUMBER" || has("TAG") {
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
    } else if u.contains("DOOR") {
        Some(FamilyKind::Door)
    } else {
        None
    }
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
        let kind = kind_of(&title).unwrap_or(FamilyKind::Door);
        let found: Vec<ScheduleRow> = rows.iter().skip(hi + 1).filter_map(|r| row(kind, &cols, r)).collect();
        out.push(Found { kind, title, from: "table", rows: found, used: vec![e.handle] });
    }
    out
}

/// Schedules drawn as text in rows and columns under a header with a MARK.
fn text_grids(ents: &[&Arc<Entity>]) -> Vec<Found> {
    let all = texts(ents);
    let mut out = Vec::new();
    let mut used_headers: Vec<Handle> = Vec::new();
    for (h, t, at, ht) in &all {
        if column(t) != Some(Col::Mark) || used_headers.contains(h) {
            continue;
        }
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
        let kind = kind_of(&title).unwrap_or(FamilyKind::Door);
        let rows: Vec<ScheduleRow> = lines.iter().filter_map(|l| row(kind, &cols, &cells_of(l))).collect();
        let mut used: Vec<Handle> = header.iter().map(|c| c.2).collect();
        used.extend(lines.iter().flatten().map(|t| t.0));
        out.push(Found { kind, title, from: "text", rows, used });
    }
    out
}

/// Tags: short marks inside a small circle or closed outline (a tag symbol), or attributes of a
/// block named like a tag.
fn tags(d: &Drawing, ents: &[&Arc<Entity>], skip: &[Handle]) -> Vec<(String, Vec2, Handle)> {
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
    let d = s.doc()?;
    let ents = plan_objects(d);
    let mut found = tables(&ents);
    found.extend(text_grids(&ents));
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
    let schedules: Vec<Value> = found
        .iter()
        .map(|f| json!({"kind": if f.kind == FamilyKind::Door { "door" } else { "window" }, "title": f.title, "from": f.from, "rows": serde_json::to_value(&f.rows).unwrap_or(Value::Null)}))
        .collect();
    let apply = bool_or(p, "apply", false);
    if apply {
        sp.schedule.rows = found.into_iter().flat_map(|f| f.rows).collect();
        sp.schedule.tags = tag_list.iter().map(|(m, at, _)| OpeningTag { mark: m.clone(), at: [at.x, at.y] }).collect();
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({
        "schedules": schedules,
        "tags": tags_json,
        "flags": {"tags_without_row": tags_without_row, "rows_without_tag": rows_without_tag},
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
            o.width = w * k;
        }
        let (head, sill) = r.heights(sp.openings.window_sill_ft);
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
}
