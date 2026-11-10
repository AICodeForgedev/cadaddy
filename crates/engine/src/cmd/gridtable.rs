//! Schedules read by their drawn grid: a titled table's columns are the vertical lines crossing
//! its header, its rows the horizontal lines across it, so a cell's text goes to the column it is
//! drawn in whatever its alignment (headers centred, cells left-aligned) and a row's second line
//! (`LED` / `13VA`) stays in its row. A table drawn without row lines is read line by line.

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;

/// Rows read from one table, at most (hostile files).
const MAX_ROWS: usize = 500;
/// Line segments looked at on a sheet, at most.
pub(crate) const MAX_SEGMENTS: usize = 400_000;

/// Note lines read under one table, at most.
const MAX_NOTES: usize = 40;
/// A blank band this many text heights tall ends a table's notes.
const NOTE_GAP_H: f64 = 4.0;
/// Group heading rules looked at in one table's header, at most.
const MAX_GROUP_RULES: usize = 40;

/// A table read off a sheet: its column headers and its rows (a cell per column).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GridTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub sheet: String,
    /// The title as printed (`LIGHT FIXTURE SCHEDULE`).
    pub title: String,
    /// The lines printed right under the table, top down (its `NOTE:` lines).
    pub notes: Vec<String>,
    /// Its extent `[x0, y0, x1, y1]`, title to last row (and notes).
    pub extent: [f64; 4],
}

impl GridTable {
    /// The column whose header names all of `words` (any case), the first.
    pub fn column(&self, words: &[&str]) -> Option<usize> {
        self.headers.iter().position(|h| {
            let u = h.to_ascii_uppercase();
            words.iter().all(|w| u.contains(w))
        })
    }

    /// The row's cell in column `c` (empty when there's none).
    pub fn cell<'a>(&self, row: &'a [String], c: Option<usize>) -> &'a str {
        c.and_then(|c| row.get(c)).map_or("", |v| v.trim())
    }
}

/// A drawing's straight line segments (lines, polylines' straight pieces).
pub(crate) fn segments(d: &Drawing) -> Vec<(Vec2, Vec2)> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if out.len() >= MAX_SEGMENTS {
            break;
        }
        entity_segments(&e.kind, &mut out);
    }
    out
}

/// One object's straight line segments into `out` (a line; a polyline's straight pieces).
pub(crate) fn entity_segments(kind: &EntityKind, out: &mut Vec<(Vec2, Vec2)>) {
    match kind {
        EntityKind::Line(l) => out.push((l.a.xy(), l.b.xy())),
        EntityKind::LwPolyline(p) => {
            let n = p.vertices.len();
            let last = if p.closed { n } else { n.saturating_sub(1) };
            for i in 0..last {
                if out.len() >= MAX_SEGMENTS {
                    break;
                }
                if let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get((i + 1) % n.max(1)))
                    && a.bulge.abs() < 1e-9
                {
                    out.push((a.p, b.p));
                }
            }
        }
        _ => {}
    }
}

/// The tables titled `title` (the title text contains it, any case) on a sheet: `texts` and `segs`
/// its texts and line segments, `sheet` its name.
pub(crate) fn read_titled(texts: &[TextItem], segs: &[(Vec2, Vec2)], title: &str, sheet: &str) -> Vec<GridTable> {
    let want = title.to_ascii_uppercase();
    texts
        .iter()
        .filter(|t| t.rotation.abs() < 0.05 && t.value.to_ascii_uppercase().contains(&want))
        .filter_map(|t| read_at(texts, segs, t, sheet))
        .collect()
}

/// The table under the title text `t` on a sheet (`texts` and `segs` its texts and line
/// segments, `sheet` its name): none when no grid of three columns or more crosses its header.
/// The columns run out to the table's outer rules (a table drawn with its row rules but no
/// border lines keeps its first and last column), and its last row down to where its column
/// lines end (no rule under the last row).
pub(crate) fn read_at(texts: &[TextItem], segs: &[(Vec2, Vec2)], t: &TextItem, sheet: &str) -> Option<GridTable> {
    let h = t.height.max(1e-9);
    // The header: the first text line under the title.
    let header_y = texts
        .iter()
        .filter(|x| x.rotation.abs() < 0.05 && x.at.y < t.at.y - 0.2 * h && x.at.y > t.at.y - 6.0 * h && (x.at.x - t.at.x).abs() < 40.0 * h)
        .map(|x| x.at.y)
        .max_by(f64::total_cmp)?;
    let tol = 0.05 * h;
    // The table's width: the rule over its header (between the title and the header), else the
    // one under it, that runs under the title. Lines of the drawing round the table (a plan's
    // walls, the sheet's frame) that cross the header's height are no columns of it.
    let rule = |lo: f64, hi: f64, near: f64| {
        segs.iter()
            .filter(|(a, b)| (a.y - b.y).abs() < tol && a.y > lo && a.y < hi && a.x.min(b.x) <= t.at.x + tol && a.x.max(b.x) >= t.at.x - tol)
            .min_by(|(p, _), (q, _)| (p.y - near).abs().total_cmp(&(q.y - near).abs()))
            .map(|(a, b)| (a.x.min(b.x), a.x.max(b.x)))
    };
    let width = rule(header_y, t.at.y, header_y).or_else(|| rule(header_y - 3.0 * h, header_y, header_y));
    // Columns: the vertical lines crossing the header (within the table's width), and its
    // width's ends (a table drawn with row rules but no border lines).
    let within = |x: f64| width.map_or((x - t.at.x).abs() < 60.0 * h, |(w0, w1)| x >= w0 - tol && x <= w1 + tol);
    let mut xs: Vec<f64> = segs
        .iter()
        .filter(|(a, b)| (a.x - b.x).abs() < tol && a.y.min(b.y) <= header_y && a.y.max(b.y) >= header_y && within(a.x))
        .map(|(a, _)| a.x)
        .collect();
    if let Some((w0, w1)) = width {
        xs.extend([w0, w1]);
    }
    xs.sort_by(f64::total_cmp);
    xs.dedup_by(|a, b| (*a - *b).abs() < 0.2 * h);
    if xs.len() < 3 {
        return None;
    }
    let (x0, x1) = (xs.first().copied()?, xs.last().copied()?);
    // Where the column lines end under the header, followed down line by line (a column drawn
    // a piece per row): the table's bottom.
    let at_column = |x: f64| xs.iter().any(|c| (x - c).abs() < tol);
    let mut floor: Option<f64> = None;
    for _ in 0..MAX_ROWS {
        let reach = floor.unwrap_or(header_y);
        let next = segs
            .iter()
            .filter(|(a, b)| (a.x - b.x).abs() < tol && at_column(a.x) && a.y.max(b.y) >= reach - tol && a.y.min(b.y) < reach - tol)
            .map(|(a, b)| a.y.min(b.y))
            .min_by(f64::total_cmp);
        match next {
            Some(y) => floor = Some(y),
            None => break,
        }
    }
    // Rows: the horizontal lines across the table (most of its width), down to its bottom.
    let mut ys: Vec<f64> = segs
        .iter()
        .filter(|(a, b)| {
            (a.y - b.y).abs() < tol
                && a.y < t.at.y
                && floor.is_none_or(|f| a.y >= f - tol)
                && (a.x.max(b.x).min(x1) - a.x.min(b.x).max(x0)) > 0.5 * (x1 - x0)
        })
        .map(|(a, _)| a.y)
        .collect();
    ys.sort_by(|a, b| b.total_cmp(a));
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.2 * h);
    // The header's band: between the lines above and below it.
    let above = ys.iter().copied().filter(|y| *y > header_y).fold(t.at.y, f64::min);
    let below = ys.iter().copied().filter(|y| *y < header_y).fold(f64::NEG_INFINITY, f64::max);
    // A group's sub-columns (`TYPE | COLOR` under `LAMPS`) are drawn from the rule under its
    // heading down: their lines cross the header's band low, not where its first line is.
    if below.is_finite() && above > below {
        let reach = below + 0.25 * (above - below);
        xs.extend(
            segs.iter()
                .filter(|(a, b)| (a.x - b.x).abs() < tol && a.y.min(b.y) <= below + tol && a.y.max(b.y) >= reach && a.x > x0 + tol && a.x < x1 - tol)
                .map(|(a, _)| a.x),
        );
        xs.sort_by(f64::total_cmp);
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.2 * h);
    }
    let col = |x: f64| xs.windows(2).position(|w| x >= w[0] - tol && x < w[1] - tol);
    let gather = |lo: f64, hi: f64| -> Vec<String> {
        let mut cells = vec![String::new(); xs.len() - 1];
        let mut inside: Vec<&TextItem> =
            texts.iter().filter(|x| x.rotation.abs() < 0.05 && x.at.y > lo && x.at.y < hi && x.at.x >= x0 - tol && x.at.x < x1).collect();
        inside.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        for x in inside {
            if let Some(c) = col(x.at.x).and_then(|c| cells.get_mut(c)) {
                if !c.is_empty() {
                    c.push(' ');
                }
                c.push_str(x.value.trim());
            }
        }
        cells
    };
    let mut header_lo = if below.is_finite() { below } else { header_y - 0.6 * h };
    // Rows with no rules between them or under the header (the table ruled only round its
    // header and rows): its first column's text lines in the band, the first its heading and
    // each next one a row's start, the lines down to the next one (a wrapped description) its own.
    let key_lines: Vec<(f64, f64)> = {
        let mut ks: Vec<(f64, f64)> = texts
            .iter()
            .filter(|x| x.rotation.abs() < 0.05 && x.at.y > header_lo && x.at.y < above && col(x.at.x) == Some(0) && !x.value.trim().is_empty())
            .map(|x| (x.at.y, x.height.max(1e-9)))
            .collect();
        ks.sort_by(|a, b| b.0.total_cmp(&a.0));
        ks.dedup_by(|a, b| (a.0 - b.0).abs() < 0.5 * b.1);
        ks
    };
    let unruled = below.is_finite() && key_lines.len() >= MIN_UNRULED_LINES;
    if unruled && let Some((y, hk)) = key_lines.get(1) {
        header_lo = y + 0.7 * hk;
    }
    let mut headers = gather(header_lo, above);
    // A group heading (`LAMPS` over `TYPE | COLOR | WATTS`): the text over a rule inside the
    // header's band that runs under some of the columns only. Each column under it is headed by
    // both (`LAMPS TYPE`), so a sub-column's `TYPE` is never the table's own `TYPE`.
    let mut groups: Vec<(f64, f64, f64)> = segs
        .iter()
        .filter(|(a, b)| (a.y - b.y).abs() < tol && a.y > header_lo + tol && a.y < above - tol)
        .map(|(a, b)| (a.y, a.x.min(b.x).max(x0), a.x.max(b.x).min(x1)))
        .filter(|(_, lo, hi)| hi - lo < 0.9 * (x1 - x0))
        .collect();
    groups.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)));
    groups.dedup_by(|a, b| (a.0 - b.0).abs() < tol && (a.1 - b.1).abs() < tol && (a.2 - b.2).abs() < tol);
    for (y, lo, hi) in groups.into_iter().take(MAX_GROUP_RULES) {
        let under: Vec<usize> =
            xs.windows(2).enumerate().filter(|(_, w)| (w[0] + w[1]) / 2.0 > lo && (w[0] + w[1]) / 2.0 < hi).map(|(i, _)| i).collect();
        if under.len() < 2 {
            continue;
        }
        let (over, sub) = (gather(y, above), gather(header_lo, y));
        let group = under.iter().filter_map(|c| over.get(*c)).filter(|v| !v.is_empty()).cloned().collect::<Vec<_>>().join(" ");
        if group.is_empty() {
            continue;
        }
        for c in under {
            if let (Some(hd), Some(s)) = (headers.get_mut(c), sub.get(c)) {
                *hd = if s.is_empty() { group.clone() } else { format!("{group} {s}") };
            }
        }
    }
    let mut rows = Vec::new();
    let mut lower: Vec<f64> = if unruled { Vec::new() } else { ys.iter().copied().filter(|y| *y <= below + tol).collect() };
    // Its last row has no rule under it: the column lines' ends close it.
    if lower.len() >= 2
        && let (Some(f), Some(l)) = (floor, lower.last().copied())
        && f < l - 0.5 * h
    {
        lower.push(f);
    }
    if unruled {
        for (i, (y, hk)) in key_lines.iter().enumerate().skip(1).take(MAX_ROWS) {
            let lo = key_lines.get(i + 1).map_or(below, |(n, hn)| n + 0.7 * hn);
            rows.push(gather(lo, y + 0.7 * hk));
        }
    } else if lower.len() >= 2 {
        for w in lower.windows(2).take(MAX_ROWS) {
            let r = gather(w[1], w[0]);
            // Another table drawn on under it (its notes' table): its title's row ends this one.
            if another_title(&r) {
                break;
            }
            if r.iter().any(|c| !c.is_empty()) {
                rows.push(r);
            }
        }
    } else {
        // No row lines: a row per text line under the header.
        // (bounded by where the column lines end, when they run on past the header's band).
        let bottom = floor.filter(|f| *f < header_y - 0.6 * h && (!below.is_finite() || *f < below - tol)).unwrap_or(header_y - 80.0 * h);
        let mut lines: Vec<f64> =
            texts.iter().filter(|x| x.at.y < header_y - 0.6 * h && x.at.y > bottom && x.at.x >= x0 && x.at.x < x1).map(|x| x.at.y).collect();
        lines.sort_by(|a, b| b.total_cmp(a));
        lines.dedup_by(|a, b| (*a - *b).abs() < 0.5 * h);
        for y in lines.into_iter().take(MAX_ROWS) {
            rows.push(gather(y - 0.5 * h, y + 0.5 * h));
        }
    }
    // The table's bottom: its last row line, else where its column lines end, else its
    // last text line.
    let bottom = if unruled {
        below
    } else if lower.len() >= 2 {
        lower.last().copied().unwrap_or(below)
    } else {
        floor.unwrap_or(below.max(header_y - 80.0 * h))
    };
    let (notes, notes_bottom) = notes_under(texts, x0, x1, bottom);
    Some(GridTable {
        headers,
        rows,
        sheet: sheet.to_string(),
        title: t.value.trim().to_string(),
        notes,
        extent: [x0, notes_bottom.min(bottom), x1, t.at.y + h],
    })
}

/// A table's band holding this many lines in its first column has rows with no rules between
/// them (a heading and two rows at least).
const MIN_UNRULED_LINES: usize = 3;

/// A row that is another table's title: one cell printed, naming a schedule or a table of notes.
fn another_title(r: &[String]) -> bool {
    let printed: Vec<&String> = r.iter().filter(|c| !c.trim().is_empty()).collect();
    match printed.as_slice() {
        [one] => {
            let u = one.trim().trim_end_matches(':').to_ascii_uppercase();
            u.contains("SCHEDULE") || u.ends_with("NOTES")
        }
        _ => false,
    }
}

/// The text lines printed right under a table (`x0`..`x1`, its bottom at `bottom`), top down,
/// until a blank band; and how far down they reach.
fn notes_under(texts: &[TextItem], x0: f64, x1: f64, bottom: f64) -> (Vec<String>, f64) {
    let mut under: Vec<&TextItem> = texts
        .iter()
        .filter(|x| x.rotation.abs() < 0.05 && x.at.y < bottom && x.at.x >= x0 && x.at.x < x1 && x.height.is_finite() && x.height > 0.0)
        .collect();
    under.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
    let mut lines: Vec<(f64, f64, String)> = Vec::new();
    let mut last = bottom;
    for x in under {
        if last - x.at.y > NOTE_GAP_H * x.height {
            break;
        }
        match lines.last_mut() {
            Some((y, h, v)) if (*y - x.at.y).abs() <= 0.5 * h.min(x.height) => {
                v.push(' ');
                v.push_str(x.value.trim());
            }
            _ => {
                if lines.len() >= MAX_NOTES {
                    break;
                }
                lines.push((x.at.y, x.height, x.value.trim().to_string()));
            }
        }
        last = x.at.y;
    }
    let reach = lines.last().map_or(bottom, |(y, h, _)| y - 0.5 * h);
    (lines.into_iter().map(|(_, _, v)| v).collect(), reach)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(x: f64, y: f64, v: &str) -> TextItem {
        TextItem { value: v.to_string(), at: Vec2::new(x, y), height: 0.1, rotation: 0.0 }
    }

    #[test]
    fn a_tables_cells_go_to_the_columns_they_are_drawn_in() {
        // Columns at x 0, 1, 3, 6; header at y 9.5 (centred), rows between 9.3 / 8.9 / 8.5.
        let segs: Vec<(Vec2, Vec2)> = [0.0, 1.0, 3.0, 6.0]
            .iter()
            .map(|x| (Vec2::new(*x, 8.5), Vec2::new(*x, 9.7)))
            .chain([9.7, 9.3, 8.9, 8.5].iter().map(|y| (Vec2::new(0.0, *y), Vec2::new(6.0, *y))))
            .collect();
        let v = vec![
            t(2.0, 10.0, "LIGHT FIXTURE SCHEDULE"),
            t(0.2, 9.5, "TAG"),
            t(1.6, 9.5, "MODEL"),
            t(4.0, 9.5, "DESCRIPTION"),
            t(0.1, 9.1, "A"),
            t(1.1, 9.1, "ML4DM10"),
            t(3.1, 9.15, "PUBLIC SPACES"),
            t(1.1, 9.0, "13VA"),
            t(0.1, 8.7, "C"),
            t(3.1, 8.7, "LINEAR LED"),
        ];
        let tables = read_titled(&v, &segs, "light fixture schedule", "E-2");
        assert_eq!(tables.len(), 1);
        let tb = &tables[0];
        assert_eq!(tb.headers, ["TAG", "MODEL", "DESCRIPTION"]);
        assert_eq!(
            tb.rows,
            vec![vec!["A".to_string(), "ML4DM10 13VA".into(), "PUBLIC SPACES".into()], vec!["C".into(), String::new(), "LINEAR LED".into()]]
        );
        assert_eq!(tb.column(&["DESCRIPTION"]), Some(2));
    }

    /// A schedule ruled only round its header and rows (no rule under the header, none between
    /// rows): its first column's lines are its heading and each row's start, a description's
    /// wrapped line is its row's; the table of notes drawn on under it is no part of it.
    #[test]
    fn a_schedule_with_no_rules_between_its_rows_is_read_by_its_first_columns_lines() {
        let v = vec![
            t(2.0, 10.0, "LIGHTING FIXTURE SCHEDULE"),
            t(0.1, 9.5, "Type"),
            t(1.1, 9.5, "DESCRIPTION"),
            t(3.1, 9.5, "WATTS"),
            t(0.1, 9.1, "EL-1"),
            t(1.1, 9.1, "EMERGENCY UNIT"),
            t(3.1, 9.1, "5 VA"),
            t(1.1, 8.85, "BATTERY"),
            t(0.1, 8.5, "EX-1"),
            t(1.1, 8.5, "EXIT SIGN"),
            t(3.1, 8.5, "2 VA"),
            t(0.1, 8.1, "LA-1"),
            t(1.1, 8.1, "TROFFER"),
            t(1.1, 7.85, "2X4"),
            t(3.1, 8.1, "29 VA"),
            t(1.1, 7.3, "LIGHTING FIXTURE SCHEDULE NOTES"),
            t(0.1, 7.0, "1 INSTALL PER MANUFACTURER"),
        ];
        let mut segs: Vec<(Vec2, Vec2)> = [0.0, 1.0, 3.0, 6.0].iter().map(|x| (Vec2::new(*x, 6.9), Vec2::new(*x, 9.7))).collect();
        segs.extend([9.7, 7.5, 7.2, 6.9].iter().map(|y| (Vec2::new(0.0, *y), Vec2::new(6.0, *y))));
        let Some(title) = v.first() else { return };
        let tb = read_at(&v, &segs, title, "X-2").unwrap_or_else(|| panic!("no table"));
        assert_eq!(tb.headers, ["Type", "DESCRIPTION", "WATTS"], "{tb:#?}");
        assert_eq!(
            tb.rows,
            vec![
                vec!["EL-1".to_string(), "EMERGENCY UNIT BATTERY".into(), "5 VA".into()],
                vec!["EX-1".into(), "EXIT SIGN".into(), "2 VA".into()],
                vec!["LA-1".into(), "TROFFER 2X4".into(), "29 VA".into()],
            ],
            "{tb:#?}"
        );
    }

    /// A ruled schedule with the table of its notes drawn on under it: the notes' title row ends it.
    #[test]
    fn the_title_of_a_table_drawn_on_under_a_schedule_ends_its_rows() {
        let v = vec![
            t(2.0, 10.0, "LIGHT FIXTURE SCHEDULE"),
            t(0.1, 9.5, "TYPE"),
            t(1.1, 9.5, "DESCRIPTION"),
            t(0.1, 9.1, "A"),
            t(1.1, 9.1, "TROFFER"),
            t(0.1, 8.7, "B"),
            t(1.1, 8.7, "DOWNLIGHT"),
            t(1.1, 8.3, "FIXTURE SCHEDULE NOTES"),
            t(0.1, 7.9, "1"),
            t(1.1, 7.9, "INSTALL PER MANUFACTURER"),
        ];
        let mut segs: Vec<(Vec2, Vec2)> = [0.0, 1.0, 6.0].iter().map(|x| (Vec2::new(*x, 7.7), Vec2::new(*x, 9.7))).collect();
        segs.extend([9.7, 9.3, 8.9, 8.5, 8.1, 7.7].iter().map(|y| (Vec2::new(0.0, *y), Vec2::new(6.0, *y))));
        let tables = read_titled(&v, &segs, "light fixture schedule", "X-3");
        let first: Vec<&str> = tables.iter().flat_map(|tb| tb.rows.iter().filter_map(|r| r.first().map(String::as_str))).collect();
        assert_eq!(first, ["A", "B"], "{tables:#?}");
    }

    /// fix-lum-fixture-schedule-not-read: a table drawn with its row rules but no border lines
    /// keeps its first and last columns; its last row, with no rule under it, ends where its
    /// column lines (drawn a piece per row) end; the lines printed under it are its notes; a
    /// plan's lines that cross its header's height (a wall, the sheet's frame) are no columns
    /// and a plan line under it is no row.
    #[test]
    fn a_table_without_border_lines_keeps_its_outer_columns_last_row_and_notes() {
        let v = vec![
            t(2.0, 10.0, "LIGHT FIXTURE SCHEDULE"),
            t(0.1, 9.5, "TYPE"),
            t(1.1, 9.5, "DESCRIPTION"),
            t(5.1, 9.5, "REMARKS"),
            t(0.1, 9.1, "A"),
            t(1.1, 9.1, "TROFFER"),
            t(5.1, 9.1, "IN GRID"),
            t(0.1, 8.7, "B"),
            t(1.1, 8.7, "DOWNLIGHT"),
            t(0.1, 8.3, "C"),
            t(1.1, 8.3, "WALL PACK"),
            t(5.1, 8.3, "AT DOORS"),
            t(0.0, 7.9, "NOTE: 1. ALL FIXTURES LED"),
            t(0.0, 7.75, "2. SEE SPECIFICATIONS"),
            t(0.0, 6.0, "FLOOR PLAN"),
        ];
        // Row rules from x 0 to 6 (no verticals at 0 and 6); column lines at 1 and 5 a piece per
        // row down to 8.1; a wall at x -2 and x 8 crossing the header's height; a plan line at 7.
        let mut segs: Vec<(Vec2, Vec2)> = [9.7, 9.3, 8.9, 8.5].iter().map(|y| (Vec2::new(0.0, *y), Vec2::new(6.0, *y))).collect();
        for x in [1.0, 5.0] {
            segs.extend([(9.7, 9.3), (9.3, 8.9), (8.9, 8.5), (8.5, 8.1)].map(|(a, b)| (Vec2::new(x, a), Vec2::new(x, b))));
        }
        segs.extend([
            (Vec2::new(-2.0, 0.0), Vec2::new(-2.0, 20.0)),
            (Vec2::new(8.0, 0.0), Vec2::new(8.0, 20.0)),
            (Vec2::new(-3.0, 7.0), Vec2::new(9.0, 7.0)),
        ]);
        let tables = read_titled(&v, &segs, "light fixture schedule", "X-1");
        assert_eq!(tables.len(), 1);
        let tb = &tables[0];
        assert_eq!(tb.headers, ["TYPE", "DESCRIPTION", "REMARKS"]);
        let first: Vec<&str> = tb.rows.iter().filter_map(|r| r.first().map(String::as_str)).collect();
        assert_eq!(first, ["A", "B", "C"], "{tb:#?}");
        assert_eq!(tb.rows[2], ["C", "WALL PACK", "AT DOORS"]);
        assert_eq!(tb.notes, ["NOTE: 1. ALL FIXTURES LED", "2. SEE SPECIFICATIONS"]);
        assert_eq!(tb.title, "LIGHT FIXTURE SCHEDULE");
        assert!(tb.extent[0] == 0.0 && tb.extent[2] == 6.0 && tb.extent[1] < 7.8 && tb.extent[1] > 7.0, "{:?}", tb.extent);
    }
}
