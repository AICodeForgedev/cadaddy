//! The load summaries and demand calculations an electrical sheet prints (the service's load
//! summary, a panel's demand calculation): a title naming one ([`Rules::summary_titles`]), then
//! lines of a label and its numbers. The table is the texts under its title as far as the empty
//! band that parts it from its neighbours (a panel schedule beside it), down to a gap of a few
//! lines or the next title. Each line keeps its label, its numbers left to right and its text as
//! printed; what they mean is the calculation's business ([`super::compare`]).

use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{LoadSummary, LoadSummaryLine};

use super::Rules;

/// Summaries read off one sheet, lines per summary, texts looked at (hostile files).
const MAX_SUMMARIES: usize = 40;
const MAX_LINES: usize = 200;
const MAX_TEXTS: usize = 50_000;
/// How far under its title a summary's lines reach, in title heights.
const DOWN_REACH: f64 = 60.0;
/// How far left and right of its title its texts are looked for, in title heights.
const LEFT_REACH: f64 = 40.0;
const RIGHT_REACH: f64 = 80.0;
/// A text's width per character, in text heights (capitals of a drafting font).
const CHAR_W: f64 = 0.62;
/// The empty band that parts two tables side by side, in line heights (a summary's labels can
/// stand well apart from their numbers).
const BAND: f64 = 8.0;
/// How far either side of its title another title still ends a summary, in title heights.
const OVER: f64 = 10.0;
/// A title is at least this much taller than its lines.
const TITLE_OVER_LINES: f64 = 1.2;
/// Lines further apart than this many line steps end the summary.
const GAP_STEPS: f64 = 4.0;

/// A number as a summary prints it: `1,234,567`, `5120.00`, `12.50`, `2,000A` (not `125%`).
pub(crate) fn number(v: &str) -> Option<f64> {
    let t = v.trim();
    if t.contains('%') {
        return None;
    }
    let t = t.trim_end_matches(|c: char| c.is_ascii_alphabetic()).trim();
    let d = t.replace(',', "");
    (!d.is_empty() && d.len() <= 14 && d.chars().all(|c| c.is_ascii_digit() || c == '.') && d.chars().any(|c| c.is_ascii_digit()))
        .then(|| d.parse::<f64>().ok())
        .flatten()
        .filter(|x| x.is_finite())
}

fn letters(v: &str) -> bool {
    v.chars().any(|c| c.is_ascii_alphabetic())
}

/// A text's extent along its line (its insertion point is its left end).
fn extent(t: &TextItem) -> (f64, f64) {
    let w = t.value.trim().chars().count() as f64 * CHAR_W * t.height;
    (t.at.x, t.at.x + w.max(t.height))
}

/// The code section a summary says it is based on: `NEC ARTICLE 220.84` → `220.84`.
pub(crate) fn basis(text: &str) -> Option<String> {
    let u = text.to_ascii_uppercase();
    let mut from = 0;
    while let Some(i) = u.get(from..).and_then(|r| r.find("220.")).map(|i| i + from) {
        let digits: String = u.get(i + 4..).unwrap_or("").chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            return Some(format!("220.{digits}"));
        }
        from = i + 4;
    }
    None
}

/// The panel a summary's title names: `PANELBOARD R2 DEMAND CALCULATION` → `R2`; none for the
/// building's (`SERVICE LOAD SUMMARY`, `PANEL LOAD SUMMARY`).
pub(crate) fn title_panel(title: &str, rules: &Rules) -> String {
    let u = title.trim().to_ascii_uppercase();
    let Some(rest) = ["PANELBOARD ", "PANEL BOARD ", "PANEL "].iter().find_map(|p| u.strip_prefix(p)) else { return String::new() };
    let tag: String = rest.split_whitespace().next().unwrap_or("").trim_matches(['\'', '"', '\u{2018}', '\u{2019}', ':']).to_string();
    let title_word = rules.summary_titles.iter().flat_map(|t| t.split_whitespace()).any(|w| w.eq_ignore_ascii_case(&tag));
    if tag.is_empty() || tag.len() > 12 || title_word || !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') {
        return String::new();
    }
    tag
}

fn is_title(v: &str, rules: &Rules) -> bool {
    let u = v.to_ascii_uppercase();
    rules.summary_titles.iter().any(|w| u.contains(&w.to_ascii_uppercase()))
}

/// The load summaries on one sheet's texts.
pub(crate) fn read(texts: &[TextItem], sheet: &str, rules: &Rules) -> Vec<LoadSummary> {
    let flat: Vec<&TextItem> = texts
        .iter()
        .take(MAX_TEXTS)
        .filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.x.is_finite() && t.at.y.is_finite() && !t.value.trim().is_empty())
        .collect();
    let titles: Vec<&&TextItem> = flat.iter().filter(|t| is_title(&t.value, rules)).take(MAX_SUMMARIES).collect();
    let mut out = Vec::new();
    for title in titles {
        let h = title.height;
        let near: Vec<&&TextItem> = flat
            .iter()
            .filter(|t| {
                t.at.y < title.at.y - 0.3 * h
                    && t.at.y > title.at.y - DOWN_REACH * h
                    && t.at.x > title.at.x - LEFT_REACH * h
                    && t.at.x < title.at.x + RIGHT_REACH * h
            })
            .collect();
        if near.is_empty() {
            continue;
        }
        // Its lines' height: the texts under its own title's width (not the tables beside it).
        let (tx0, tx1) = (title.at.x - OVER * h, extent(title).1 + OVER * h);
        let mut hs: Vec<f64> = near.iter().filter(|t| t.at.x >= tx0 && t.at.x <= tx1).map(|t| t.height).collect();
        hs.sort_by(f64::total_cmp);
        let line_h = hs.get(hs.len() / 2).copied().unwrap_or(h);
        if h < TITLE_OVER_LINES * line_h {
            continue;
        }
        // Down to the next title or heading under this one (another table's: taller than these
        // lines), or a gap of a few lines.
        let mut ys: Vec<f64> = near.iter().map(|t| t.at.y).collect();
        ys.sort_by(|a, b| b.total_cmp(a));
        let next_title = flat
            .iter()
            .filter(|t| t.at.y < title.at.y - 0.3 * h && !std::ptr::eq::<TextItem>(**t, *title) && t.height >= TITLE_OVER_LINES * line_h)
            // Over this summary (a schedule's title off to its side is another column's).
            .filter(|t| extent(t).0 < tx1 && extent(t).1 > tx0)
            .map(|t| t.at.y)
            .max_by(f64::total_cmp);
        let mut bottom = next_title.map_or(f64::NEG_INFINITY, |y| y + 0.3 * line_h);
        let mut step: Option<f64> = None;
        let mut last = title.at.y;
        let mut lines_seen = 0usize;
        for y in ys.iter().copied().filter(|y| *y > bottom) {
            let gap = last - y;
            if gap <= 0.6 * line_h && lines_seen > 0 {
                continue;
            }
            if step.is_some_and(|s| gap > GAP_STEPS * s) || (step.is_none() && gap > 2.0 * GAP_STEPS * line_h) {
                bottom = y + 0.3 * line_h;
                break;
            }
            // The first line step sets the pace (never under a line's height).
            if lines_seen > 0 && step.is_none() {
                step = Some(gap.max(line_h));
            }
            lines_seen += 1;
            last = y;
        }
        // A panel schedule beside it starts at its circuit-number column.
        let ckt = |t: &TextItem| matches!(t.value.trim().to_ascii_uppercase().as_str(), "CKT" | "CKT." | "CKT #" | "CKT NO." | "CIR");
        let right = near.iter().filter(|t| t.at.y > bottom && ckt(t) && t.at.x > title.at.x + 2.0 * h).map(|t| t.at.x).fold(f64::INFINITY, f64::min);
        let left =
            near.iter().filter(|t| t.at.y > bottom && ckt(t) && t.at.x < title.at.x - 2.0 * h).map(|t| t.at.x).fold(f64::NEG_INFINITY, f64::max);
        let rows: Vec<&&TextItem> =
            near.iter().copied().filter(|t| t.at.y > bottom && t.at.x < right - 0.5 * line_h && t.at.x > left + 0.5 * line_h).collect();
        // Across: the texts' extents merged; the summary is the run of them its title starts in,
        // as far as an empty band either side.
        let mut spans: Vec<(f64, f64)> = rows.iter().map(|t| extent(t)).chain(std::iter::once(extent(title))).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut runs: Vec<(f64, f64)> = Vec::new();
        for (a, b) in spans {
            match runs.last_mut() {
                Some(r) if a <= r.1 + BAND * line_h => r.1 = r.1.max(b),
                _ => runs.push((a, b)),
            }
        }
        let Some(&(x0, x1)) = runs.iter().find(|r| title.at.x >= r.0 - 1e-9 && title.at.x <= r.1 + 1e-9) else { continue };
        let mut kept: Vec<&&TextItem> = rows.into_iter().filter(|t| t.at.x >= x0 - 1e-9 && extent(t).1 <= x1 + 1e-9).collect();
        kept.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        let mut grouped: Vec<Vec<&TextItem>> = Vec::new();
        for t in kept {
            match grouped.last_mut() {
                Some(g) if g.first().is_some_and(|f| (f.at.y - t.at.y).abs() <= 0.6 * line_h) => g.push(t),
                _ => grouped.push(vec![t]),
            }
        }
        let mut lines = Vec::new();
        for mut g in grouped.into_iter().take(MAX_LINES) {
            g.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
            let first_num = g.iter().position(|t| number(&t.value).is_some());
            let lead: Vec<&str> = g.iter().take(first_num.unwrap_or(g.len())).filter(|t| letters(&t.value)).map(|t| t.value.trim()).collect();
            let label = if lead.is_empty() {
                g.iter().filter(|t| letters(&t.value)).map(|t| t.value.trim()).collect::<Vec<_>>().join(" ")
            } else {
                lead.join(" ")
            };
            let values: Vec<f64> = g.iter().filter_map(|t| number(&t.value)).collect();
            let text = g.iter().map(|t| t.value.trim()).collect::<Vec<_>>().join(" ");
            let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect();
            lines.push(LoadSummaryLine { label: label.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect(), values, text });
        }
        if lines.iter().all(|l| l.values.is_empty()) {
            continue;
        }
        let title_text: String = title.value.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();
        let basis = basis(&title_text).or_else(|| lines.iter().find_map(|l| basis(&l.text))).unwrap_or_default();
        out.push(LoadSummary { panel: title_panel(&title_text, rules), title: title_text, basis, source: sheet.to_string(), lines });
    }
    out
}
