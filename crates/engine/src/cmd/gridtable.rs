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
const MAX_SEGMENTS: usize = 400_000;

/// A table read off a sheet: its column headers and its rows (a cell per column).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GridTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub sheet: String,
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
        match &e.kind {
            EntityKind::Line(l) => out.push((l.a.xy(), l.b.xy())),
            EntityKind::LwPolyline(p) => {
                let n = p.vertices.len();
                let last = if p.closed { n } else { n.saturating_sub(1) };
                for i in 0..last {
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
    out
}

/// The tables titled `title` (the title text contains it, any case) on a sheet: `texts` and `segs`
/// its texts and line segments, `sheet` its name.
pub(crate) fn read_titled(texts: &[TextItem], segs: &[(Vec2, Vec2)], title: &str, sheet: &str) -> Vec<GridTable> {
    let want = title.to_ascii_uppercase();
    let mut out = Vec::new();
    for t in texts.iter().filter(|t| t.rotation.abs() < 0.05 && t.value.to_ascii_uppercase().contains(&want)) {
        let h = t.height.max(1e-9);
        // The header: the first text line under the title.
        let Some(header_y) = texts
            .iter()
            .filter(|x| x.rotation.abs() < 0.05 && x.at.y < t.at.y - 0.2 * h && x.at.y > t.at.y - 6.0 * h && (x.at.x - t.at.x).abs() < 40.0 * h)
            .map(|x| x.at.y)
            .max_by(f64::total_cmp)
        else {
            continue;
        };
        let tol = 0.05 * h;
        // Columns: the vertical lines crossing the header.
        let mut xs: Vec<f64> = segs
            .iter()
            .filter(|(a, b)| (a.x - b.x).abs() < tol && a.y.min(b.y) <= header_y && a.y.max(b.y) >= header_y && (a.x - t.at.x).abs() < 60.0 * h)
            .map(|(a, _)| a.x)
            .collect();
        xs.sort_by(f64::total_cmp);
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.2 * h);
        if xs.len() < 3 {
            continue;
        }
        let (x0, x1) = (xs[0], xs[xs.len() - 1]);
        // Rows: the horizontal lines across the table (most of its width).
        let mut ys: Vec<f64> = segs
            .iter()
            .filter(|(a, b)| (a.y - b.y).abs() < tol && a.y < t.at.y && (a.x.max(b.x).min(x1) - a.x.min(b.x).max(x0)) > 0.5 * (x1 - x0))
            .map(|(a, _)| a.y)
            .collect();
        ys.sort_by(|a, b| b.total_cmp(a));
        ys.dedup_by(|a, b| (*a - *b).abs() < 0.2 * h);
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
        // The header's band: between the lines above and below it.
        let above = ys.iter().copied().filter(|y| *y > header_y).fold(t.at.y, f64::min);
        let below = ys.iter().copied().filter(|y| *y < header_y).fold(f64::NEG_INFINITY, f64::max);
        let headers = if below.is_finite() { gather(below, above) } else { gather(header_y - 0.6 * h, above) };
        let mut rows = Vec::new();
        let lower: Vec<f64> = ys.iter().copied().filter(|y| *y <= below + tol).collect();
        if lower.len() >= 2 {
            for w in lower.windows(2).take(MAX_ROWS) {
                let r = gather(w[1], w[0]);
                if r.iter().any(|c| !c.is_empty()) {
                    rows.push(r);
                }
            }
        } else {
            // No row lines: a row per text line under the header.
            let mut lines: Vec<f64> = texts
                .iter()
                .filter(|x| x.at.y < header_y - 0.6 * h && x.at.y > header_y - 80.0 * h && x.at.x >= x0 && x.at.x < x1)
                .map(|x| x.at.y)
                .collect();
            lines.sort_by(|a, b| b.total_cmp(a));
            lines.dedup_by(|a, b| (*a - *b).abs() < 0.5 * h);
            for y in lines.into_iter().take(MAX_ROWS) {
                rows.push(gather(y - 0.5 * h, y + 0.5 * h));
            }
        }
        out.push(GridTable { headers, rows, sheet: sheet.to_string() });
    }
    out
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
}
