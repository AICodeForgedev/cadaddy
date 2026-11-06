//! Panel schedules as a print set draws them: a `PANELBOARD <tag>` title, a two-line header
//! (`CKT | CIRCUIT DESCRIPTION | BREAKER AMPS P | …`, mirrored on the right for the even
//! circuits), then a row per circuit pair until the subtotals. Each circuit's breaker type codes
//! (`AF`, `GF`, `AF/GF`) are printed outside the table beside its number; the schedule's legend
//! (`GF - GROUND FAULT INTERRUPTER`) says what they are. The guided job keeps them in
//! `mep.panel_schedules`; `bim.props.fill` puts each device's breaker and its type on it.

use std::collections::BTreeMap;

use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{PanelCircuit, PanelSchedule};

/// Most schedules read off one sheet, and rows per schedule (hostile files).
const MAX_SCHEDULES: usize = 200;
const MAX_ROWS: usize = 200;
/// How far below its title a schedule's header can be, in text heights.
const HEADER_REACH: f64 = 16.0;
/// A schedule's title is at least this much taller than its header's text.
const TITLE_OVER_CELLS: f64 = 1.3;
/// A legend's lines: how far below and right of its `LEGEND:` label, in its text heights.
const LEGEND_ROWS: f64 = 12.0;
const LEGEND_WIDTH: f64 = 120.0;
/// How far a number can sit from its column's header, in text heights.
const COLUMN_REACH: f64 = 5.0;

/// The panel a schedule's title names: `PANELBOARD G (TYPICAL OF 124 REQUIRED)`,
/// `PANEL 'DP1'`, `PANEL SCHEDULE - P1`, `PANELBOARD P1 SCHEDULE`; and the number it is typical of.
pub(crate) fn title_panel(title: &str) -> Option<(String, Option<u32>)> {
    let u = title.trim().to_ascii_uppercase();
    let rest = u.strip_prefix("PANELBOARD ").or_else(|| u.strip_prefix("PANEL BOARD ")).or_else(|| u.strip_prefix("PANEL "))?;
    let rest = rest.trim().strip_prefix("SCHEDULE").map_or(rest.trim(), |r| r.trim_start_matches([' ', '-', ':', '\u{2013}']));
    let tag: String = rest.split_whitespace().next()?.trim_matches(['\'', '"', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', ':']).to_string();
    let ok = !tag.is_empty()
        && tag.len() <= 12
        && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && tag.chars().any(|c| c.is_ascii_alphanumeric());
    if !ok || ["SCHEDULE", "SCHEDULES", "SHALL", "AND", "WITH", "TO", "IN", "FOR"].contains(&tag.as_str()) {
        return None;
    }
    // A panel's other tables (its demand calculation, load summary) aren't its schedule.
    if ["DEMAND", "CALCULATION", "SUMMARY", "RISER"].iter().any(|w| rest.contains(w)) {
        return None;
    }
    let typical = rest.split_once("TYPICAL OF").and_then(|(_, r)| r.split_whitespace().next()).and_then(|n| n.parse::<u32>().ok());
    Some((tag, typical))
}

/// A breaker type code as schedules print them beside a circuit: `AF`, `GF`, `AF/GF`, `ST`.
fn codes(v: &str) -> Option<Vec<String>> {
    let v = v.trim();
    let parts: Vec<String> = v.split(['/', ',']).map(|p| p.trim().to_ascii_uppercase()).collect();
    (!v.is_empty() && v.len() <= 16 && parts.iter().all(|p| (1..=5).contains(&p.len()) && p.chars().all(|c| c.is_ascii_uppercase()))).then_some(parts)
}

/// A legend line: `GF - GROUND FAULT INTERRUPTER`.
fn legend_line(v: &str) -> Option<(String, String)> {
    let (k, m) = v.split_once(" - ").or_else(|| v.split_once(" = "))?;
    let k = k.trim().to_ascii_uppercase();
    let m = m.trim();
    ((1..=5).contains(&k.len()) && k.chars().all(|c| c.is_ascii_uppercase()) && m.len() >= 3).then(|| (k, m.to_ascii_uppercase()))
}

fn number(v: &str) -> Option<&str> {
    let v = v.trim();
    (!v.is_empty() && v.len() <= 4 && v.chars().all(|c| c.is_ascii_digit())).then_some(v)
}

fn letters(v: &str) -> bool {
    v.chars().any(|c| c.is_ascii_alphabetic())
}

/// One side of a two-sided schedule: its columns' x (the circuit number, breaker amps and poles).
#[derive(Clone, Debug)]
struct Side {
    num: f64,
    amps: Option<f64>,
    poles: Option<f64>,
    /// Its wire size and load (VA) columns.
    wire: Option<f64>,
    load: Option<f64>,
    /// Its other number columns (wire size, load): a number nearest one of them is none of the above.
    others: Vec<f64>,
    /// The left side (codes left of its numbers, the description right of them).
    left: bool,
}

#[derive(Default)]
struct Cells {
    number: String,
    description: Vec<String>,
    amps: String,
    poles: String,
    wire: String,
    load: String,
    codes: Vec<String>,
}

/// A number as a schedule cell prints it: `20`, `1,204`, `#12`, `4/0`.
fn numberish(v: &str) -> bool {
    let d = v.trim().trim_start_matches('#').replace([',', '/'], "");
    !d.is_empty() && d.len() <= 9 && d.chars().all(|c| c.is_ascii_digit())
}

/// The header's facts above the circuits: each `LABEL:` and the text right of it on its line.
fn header_facts(flat: &[&TextItem], y0: f64, y1: f64, x0: f64, x1: f64, row_h: f64) -> BTreeMap<String, String> {
    let near: Vec<&&TextItem> = flat.iter().filter(|t| t.at.y > y0 && t.at.y < y1 && t.at.x >= x0 && t.at.x <= x1).collect();
    let mut out = BTreeMap::new();
    for l in near.iter().filter(|t| t.value.trim().ends_with(':')) {
        let value = near
            .iter()
            .filter(|t| (t.at.y - l.at.y).abs() <= 0.6 * row_h && t.at.x > l.at.x + 0.1 * row_h)
            .min_by(|a, b| a.at.x.total_cmp(&b.at.x))
            .filter(|v| !v.value.trim().ends_with(':'));
        if let Some(v) = value {
            out.insert(l.value.trim().trim_end_matches(':').trim().to_ascii_lowercase(), v.value.trim().to_string());
        }
    }
    out
}

/// The headers a schedule's per-phase columns carry (`*` where the phases vary by feeder).
const PHASE_HEADS: [&str; 7] = ["A", "B", "C", "*", "\u{d8}A", "\u{d8}B", "\u{d8}C"];
/// How many rows below its last circuit a schedule's totals reach, in row steps.
const TOTALS_ROWS: f64 = 8.0;

/// A total's value as printed: `1,234,567`, `1,250`, `16.2` (kVA).
fn amount(v: &str) -> Option<f64> {
    let d = v.trim().trim_end_matches(['A', 'W']).trim().replace(',', "");
    (!d.is_empty() && d.len() <= 12 && d.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .then(|| d.parse::<f64>().ok())
        .flatten()
        .filter(|x| x.is_finite() && *x >= 0.0)
}

/// What a totals line under the circuits gives: `TOTAL CONNECTED PANELBOARD (VA)` →
/// `connected_va`, `TOTAL PANELBOARD DEMAND (AMPS)` → `demand_a`; `PER PHASE SUB-TOTALS` (and
/// `CONNECTED PER PHASE`) is the phases' line. (key, scale): kVA reads as VA.
fn total_key(label: &str) -> Option<(&'static str, f64)> {
    let u = label.trim().to_ascii_uppercase();
    if u.starts_with("PER PHASE") || u.contains("PER PHASE") {
        return Some(("phase", 1.0));
    }
    if !(u.starts_with("TOTAL") || u.contains("SUB-TOTAL") || u.contains("SUBTOTAL")) {
        return None;
    }
    let amps = u.contains("AMP");
    let kva = u.contains("KVA");
    let demand = u.contains("DEMAND");
    if !(demand || u.contains("CONNECTED") || u.contains("LOAD")) {
        return None;
    }
    Some(match (demand, amps) {
        (true, true) => ("demand_a", 1.0),
        (true, false) => ("demand_va", if kva { 1000.0 } else { 1.0 }),
        (false, true) => ("connected_a", 1.0),
        (false, false) => ("connected_va", if kva { 1000.0 } else { 1.0 }),
    })
}

/// The totals printed under a schedule's circuits (between `y0` and `y1`, below its rows): each
/// line's label and the numbers right of it on its line (a legend beside them ends the line).
/// The per-phase line's numbers go to the phase columns nearest them, else left to right.
fn totals_of(flat: &[&TextItem], y0: f64, y1: f64, x0: f64, x1: f64, row_h: f64, phases: &[f64]) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    let near: Vec<&&TextItem> = flat.iter().filter(|t| t.at.y <= y0 && t.at.y >= y1 && t.at.x >= x0 && t.at.x <= x1).collect();
    let mut labels: Vec<&&&TextItem> = near.iter().filter(|t| total_key(&t.value).is_some()).collect();
    labels.sort_by(|a, b| b.at.y.total_cmp(&a.at.y));
    for l in labels {
        let Some((key, scale)) = total_key(&l.value) else { continue };
        let mut on: Vec<&&&TextItem> = near.iter().filter(|t| (t.at.y - l.at.y).abs() <= 0.6 * row_h && t.at.x > l.at.x + 0.1 * row_h).collect();
        on.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
        let mut values: Vec<(f64, f64)> = Vec::new();
        for t in on {
            if t.value.trim().trim_end_matches(':').eq_ignore_ascii_case("LEGEND") || legend_line(&t.value).is_some() {
                break;
            }
            if let Some(v) = amount(&t.value) {
                values.push((t.at.x, v * scale));
            }
        }
        if key == "phase" {
            let mut cols: Vec<f64> = phases.to_vec();
            cols.sort_by(f64::total_cmp);
            for (i, (x, v)) in values.iter().take(3).enumerate() {
                let at = if cols.len() >= values.len().min(3) {
                    cols.iter().enumerate().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(i, |(k, _)| k)
                } else {
                    i
                };
                if let Some(name) = ["phase_a_va", "phase_b_va", "phase_c_va"].get(at) {
                    out.entry((*name).to_string()).or_insert(*v);
                }
            }
        } else if let Some((_, v)) = values.first() {
            out.entry(key.to_string()).or_insert(*v);
        }
    }
    out
}

/// The schedules on one sheet's texts (its PDF page re-read, or the drawing's).
pub(crate) fn read(texts: &[TextItem], sheet: &str) -> Vec<PanelSchedule> {
    let flat: Vec<&TextItem> =
        texts.iter().filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.x.is_finite() && t.at.y.is_finite()).collect();
    // The legend: the `CODE - MEANING` lines under a `LEGEND:` label (a circuit's own description,
    // `LTG - THIRD FLOOR`, isn't one); with no label on the sheet, every such line.
    let labels: Vec<&&TextItem> = flat.iter().filter(|t| t.value.trim().trim_end_matches(':').trim().eq_ignore_ascii_case("LEGEND")).collect();
    let under = |t: &TextItem| {
        labels.is_empty()
            || labels.iter().any(|l| {
                t.at.y <= l.at.y + 0.6 * l.height
                    && t.at.y > l.at.y - LEGEND_ROWS * l.height
                    && t.at.x >= l.at.x - l.height
                    && t.at.x <= l.at.x + LEGEND_WIDTH * l.height
            })
    };
    let mut legend: BTreeMap<String, String> = BTreeMap::new();
    for t in flat.iter().filter(|t| under(t)) {
        if let Some((k, m)) = legend_line(&t.value) {
            legend.entry(k).or_insert(m);
        }
    }
    let mut out = Vec::new();
    for title in &flat {
        if out.len() >= MAX_SCHEDULES {
            break;
        }
        let Some((panel, typical_of)) = title_panel(&title.value) else { continue };
        let h = title.height;
        // The header: the nearest line under the title with a CKT column (two: the right side's).
        let ckts: Vec<&&TextItem> = flat
            .iter()
            .filter(|t| {
                let u = t.value.trim().to_ascii_uppercase();
                (u == "CKT" || u == "CKT." || u == "CIR" || u == "CKT #" || u == "CKT NO.")
                    && t.at.y < title.at.y
                    && t.at.y > title.at.y - HEADER_REACH * h
                    && (t.at.x - title.at.x).abs() < 80.0 * h
            })
            .collect();
        let Some(top) = ckts.iter().map(|t| t.at.y).max_by(f64::total_cmp) else { continue };
        let row_h = ckts.iter().map(|t| t.height).fold(h, f64::min);
        // A title stands above its table in larger text (a cell naming a panel is a feeder row).
        if h < TITLE_OVER_CELLS * ckts.iter().map(|t| t.height).fold(f64::INFINITY, f64::min) {
            continue;
        }
        let mut line: Vec<&&TextItem> = ckts.iter().copied().filter(|t| (t.at.y - top).abs() <= 0.6 * row_h).collect();
        line.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
        // Which way each CKT column faces: its description's header beside it (right of a left
        // side's numbers, left of a right side's). Tables side by side share the header line; this
        // title's table starts at its last left-facing CKT before the title, and ends at the next
        // right-facing one before another table starts.
        let descs: Vec<f64> = flat
            .iter()
            .filter(|t| t.value.to_ascii_uppercase().contains("DESCRIPTION") && t.at.y <= top + 0.6 * row_h && t.at.y > top - 2.6 * row_h)
            .map(|t| t.at.x)
            .collect();
        // Right-facing: the next header right of it is a description, not another CKT (the right
        // side's own, or the next table's).
        let ckt_xs: Vec<f64> = line.iter().map(|t| t.at.x).collect();
        let faces_right = |x: f64| -> Option<bool> {
            let next_desc = descs.iter().copied().filter(|d| *d > x + 0.1 * row_h).min_by(f64::total_cmp);
            let next_ckt = ckt_xs.iter().copied().filter(|c| *c > x + 0.1 * row_h).min_by(f64::total_cmp);
            match (next_desc, next_ckt) {
                (Some(d), Some(c)) => Some(d < c),
                (Some(_), None) => Some(true),
                (None, _) => Some(false),
            }
        };
        let (l, r) = if descs.is_empty() {
            let (Some(l), Some(r)) = (line.first().copied(), line.last().copied()) else { continue };
            (l, r)
        } else {
            let lefts: Vec<usize> = (0..line.len()).filter(|i| line.get(*i).is_some_and(|t| faces_right(t.at.x) == Some(true))).collect();
            let start =
                lefts.iter().copied().rfind(|i| line.get(*i).is_some_and(|t| t.at.x <= title.at.x + 2.0 * h)).or_else(|| lefts.first().copied());
            let Some(start) = start else { continue };
            let Some(l) = line.get(start).copied() else { continue };
            let r = line
                .iter()
                .skip(start + 1)
                .take_while(|t| faces_right(t.at.x) != Some(true))
                .find(|t| faces_right(t.at.x) == Some(false))
                .copied()
                .unwrap_or(l);
            (l, r)
        };
        let two = r.at.x - l.at.x > 20.0 * row_h;
        // The header's second line (`# | DESCRIPTION | AMPS | P`), else its first.
        // One-sided, the header runs across the table; two-sided, between its CKT columns.
        let head_x1 = if two { r.at.x + 3.0 * row_h } else { l.at.x + 120.0 * row_h };
        let head: Vec<&&TextItem> = flat
            .iter()
            .filter(|t| t.at.y <= top + 0.6 * row_h && t.at.y > top - 2.6 * row_h && t.at.x > l.at.x - row_h && t.at.x < head_x1)
            .collect();
        let named = |name: &str| -> Vec<f64> { head.iter().filter(|t| t.value.trim().eq_ignore_ascii_case(name)).map(|t| t.at.x).collect() };
        let hash = named("#");
        let near_num =
            |x: f64| hash.iter().copied().filter(|h| (h - x).abs() < 3.0 * row_h).min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs())).unwrap_or(x);
        let amps = [named("AMPS"), named("TRIP"), named("A")].concat();
        let poles = [named("P"), named("POLE"), named("POLES")].concat();
        let lnum = near_num(l.at.x);
        let rnum = near_num(r.at.x);
        let first_after = |xs: &[f64], x: f64, limit: f64| xs.iter().copied().filter(|v| *v > x && *v < limit).min_by(f64::total_cmp);
        let last_before = |xs: &[f64], x: f64, limit: f64| xs.iter().copied().filter(|v| *v < x && *v > limit).max_by(f64::total_cmp);
        let mid = if two { (lnum + rnum) / 2.0 } else { f64::INFINITY };
        let others = [named("SIZE"), named("(VA)"), named("LOAD"), named("*")].concat();
        let half = |left: bool| -> Vec<f64> { others.iter().copied().filter(|x| if left { *x < mid } else { *x > mid }).collect() };
        let sizes = named("SIZE");
        let loads = [named("(VA)"), named("LOAD")].concat();
        let (lamps, lpoles) = (first_after(&amps, lnum, mid), first_after(&poles, lnum, mid));
        let lwire = first_after(&sizes, lpoles.or(lamps).unwrap_or(lnum), mid);
        let lload = first_after(&loads, lwire.unwrap_or(lnum), mid);
        let mut sides = vec![Side { num: lnum, amps: lamps, poles: lpoles, wire: lwire, load: lload, others: half(true), left: true }];
        if two {
            let rwire = last_before(&sizes, rnum, mid);
            sides.push(Side {
                num: rnum,
                amps: last_before(&amps, rnum, mid),
                poles: last_before(&poles, rnum, mid),
                wire: rwire,
                load: last_before(&loads, rwire.unwrap_or(rnum), mid),
                others: half(false),
                left: false,
            });
        }
        let head_bottom = head.iter().map(|t| t.at.y).fold(top, f64::min);
        let (x0, x1) = (lnum - 8.0 * row_h, if two { rnum + 8.0 * row_h } else { lnum + 120.0 * row_h });
        let mut below: Vec<&&TextItem> = flat.iter().filter(|t| t.at.y < head_bottom - 0.5 * row_h && t.at.x >= x0 && t.at.x <= x1).collect();
        below.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        // Rows, line by line, until the subtotals, a gap of a few rows, or the next title. A
        // subtotal's line ends the rows whichever of its texts comes first (its number can sit a
        // hair above its label).
        let ends = |v: &str| {
            let u = v.trim().to_ascii_uppercase();
            u.starts_with("PER PHASE") || u.starts_with("TOTAL") || u.contains("SUB-TOTAL") || u.contains("SUBTOTAL")
        };
        let end_ys: Vec<f64> = below.iter().filter(|t| ends(&t.value)).map(|t| t.at.y).collect();
        let mut lines: Vec<Vec<&TextItem>> = Vec::new();
        let mut last_y = head_bottom;
        let mut step: Option<f64> = None;
        for t in below {
            match lines.last_mut() {
                Some(ln) if ln.first().is_some_and(|f| (f.at.y - t.at.y).abs() <= 0.6 * row_h) => ln.push(t),
                _ => {
                    let u = t.value.trim().to_ascii_uppercase();
                    let gap = last_y - t.at.y;
                    if end_ys.iter().any(|y| (y - t.at.y).abs() <= 0.6 * row_h)
                        || (title_panel(&u).is_some() && t.height >= TITLE_OVER_CELLS * row_h)
                        || step.is_some_and(|s| gap > 2.6 * s)
                        || (step.is_none() && gap > 4.0 * row_h)
                        || lines.len() >= MAX_ROWS
                    {
                        break;
                    }
                    step.get_or_insert(gap);
                    last_y = t.at.y;
                    lines.push(vec![t]);
                }
            }
        }
        let mut circuits: Vec<PanelCircuit> = Vec::new();
        let mut side_rows: Vec<Vec<Cells>> = sides.iter().map(|_| Vec::new()).collect();
        for ln in &lines {
            let mut cells: Vec<Cells> = sides.iter().map(|_| Cells::default()).collect();
            for t in ln {
                let x = t.at.x;
                let v = t.value.trim();
                // The side it is on: left of the middle the left side.
                let si = usize::from(two && x > mid);
                let (Some(side), Some(c)) = (sides.get(si), cells.get_mut(si)) else { continue };
                let outside = if side.left { x < side.num - 0.8 * row_h } else { x > side.num + 0.8 * row_h };
                if outside {
                    if let Some(cs) = codes(v) {
                        c.codes.extend(cs);
                    }
                    continue;
                }
                // A number belongs to the nearest of the circuit number, amps and poles columns
                // (a text's height is its capitals': a few of them reach the next column).
                let numeric = number(v.trim_end_matches('A')).is_some() || numberish(v);
                if numeric {
                    // Another column is one none of these is (a column's header words, `LOAD` over
                    // `(VA)`, stand a little apart).
                    let known = [Some(side.num), side.amps, side.poles, side.wire, side.load];
                    let other = side.others.iter().copied().filter(|o| !known.iter().flatten().any(|k| (k - o).abs() < 1.5 * row_h));
                    let best = [(0u8, Some(side.num)), (1, side.amps), (2, side.poles), (3, side.wire), (4, side.load)]
                        .into_iter()
                        .chain(other.map(|o| (9u8, Some(o))))
                        .filter_map(|(k, cx)| cx.map(|cx| ((x - cx).abs(), k)))
                        .filter(|(d, _)| *d <= COLUMN_REACH * row_h)
                        .min_by(|a, b| a.0.total_cmp(&b.0));
                    match best {
                        Some((_, 0)) if c.number.is_empty() && number(v).is_some() => c.number = v.to_string(),
                        Some((_, 1)) if c.amps.is_empty() && number(v.trim_end_matches('A')).is_some() => {
                            c.amps = v.trim_end_matches('A').to_string()
                        }
                        Some((_, 2)) if c.poles.is_empty() && number(v).is_some() => c.poles = v.to_string(),
                        Some((_, 3)) if c.wire.is_empty() => c.wire = v.trim_start_matches('#').to_string(),
                        Some((_, 4)) if c.load.is_empty() => c.load = v.replace(',', ""),
                        _ => {}
                    }
                } else if v.eq_ignore_ascii_case("OL") || v.eq_ignore_ascii_case("RD") {
                    // `OL` / `RD`: see the one-line (riser) diagram — in the breaker's or wire's column.
                    let near = |cx: Option<f64>| cx.is_some_and(|cx| (x - cx).abs() <= COLUMN_REACH * row_h);
                    let (da, dw) = (side.amps.map(|a| (x - a).abs()), side.wire.map(|w| (x - w).abs()));
                    if near(side.amps) && dw.is_none_or(|dw| da.is_some_and(|da| da <= dw)) && c.amps.is_empty() {
                        c.amps = v.to_ascii_uppercase();
                    } else if near(side.wire) && c.wire.is_empty() {
                        c.wire = v.to_ascii_uppercase();
                    }
                } else if letters(v) {
                    let between = match (side.left, side.amps) {
                        (true, Some(a)) => x > side.num && x < a - 1.0 * row_h,
                        (false, Some(a)) => x < side.num && x > a + 1.0 * row_h,
                        (true, None) => x > side.num,
                        (false, None) => x < side.num,
                    };
                    if between {
                        c.description.push(v.to_string());
                    }
                }
            }
            for (rows, c) in side_rows.iter_mut().zip(cells) {
                rows.push(c);
            }
        }
        // A breaker of several poles spans as many rows, its number on the first or the middle one;
        // its load is each pole's row's (a row of its own with no description is one of its poles).
        let load = |c: &Cells| c.load.parse::<f64>().ok().filter(|v| v.is_finite());
        for rows in &side_rows {
            let starts = |c: &Cells| number(&c.number).is_some() && !(c.description.is_empty() && c.amps.is_empty());
            let mut taken = vec![false; rows.len()];
            for (i, c) in rows.iter().enumerate() {
                let Some(n) = number(&c.number).filter(|_| starts(c)) else { continue };
                let poles = c.poles.parse::<u32>().ok().filter(|p| (1..=3).contains(p)).unwrap_or(1);
                // Its other poles: the pole rows just before it (not another circuit's), then after.
                let mut mine = vec![i];
                let mut j = i;
                while mine.len() < poles as usize && j > 0 {
                    j -= 1;
                    if rows.get(j).is_none_or(starts) || taken.get(j).copied().unwrap_or(true) {
                        break;
                    }
                    mine.push(j);
                }
                let mut j = i + 1;
                while mine.len() < poles as usize && rows.get(j).is_some_and(|r| !starts(r)) {
                    mine.push(j);
                    j += 1;
                }
                for k in &mine {
                    if let Some(t) = taken.get_mut(*k) {
                        *t = true;
                    }
                }
                let loads: Vec<f64> = mine.iter().filter_map(|k| rows.get(*k)).filter_map(load).collect();
                circuits.push(PanelCircuit {
                    number: n.to_string(),
                    description: c.description.join(" "),
                    room: String::new(),
                    breaker: c.amps.clone(),
                    poles,
                    protection: c.codes.clone(),
                    wire: c.wire.clone(),
                    load_va: (!loads.is_empty()).then(|| loads.iter().sum()),
                    source: sheet.to_string(),
                });
            }
        }
        if circuits.is_empty() {
            continue;
        }
        circuits.sort_by_key(|c| c.number.parse::<u32>().unwrap_or(u32::MAX));
        let header = header_facts(&flat, top + 0.6 * row_h, title.at.y, l.at.x - 2.0 * row_h, head_x1 + 8.0 * row_h, row_h);
        // The phase columns between the sides' loads (`A B C`, or `*` where phases vary).
        let (lo, hi) = (
            sides.first().and_then(|s| s.load.or(s.poles)).unwrap_or(lnum),
            if two { sides.get(1).and_then(|s| s.load.or(s.poles)).unwrap_or(rnum) } else { x1 },
        );
        let phases: Vec<f64> = head
            .iter()
            .filter(|t| PHASE_HEADS.contains(&t.value.trim().to_ascii_uppercase().as_str()) && t.at.x > lo + row_h && t.at.x < hi - row_h)
            .filter(|t| sides.iter().all(|s| s.amps.is_none_or(|a| (a - t.at.x).abs() > 0.5 * row_h)))
            .map(|t| t.at.x)
            .collect();
        let end = step.map_or(last_y - 4.0 * row_h, |s| last_y - TOTALS_ROWS * s);
        let totals = totals_of(&flat, last_y + 0.6 * row_h, end, x0, x1, row_h, &phases);
        out.push(PanelSchedule { panel, typical_of, source: sheet.to_string(), circuits, legend: legend.clone(), header, totals });
    }
    out
}

/// A circuit whose breaker or wire the schedule leaves to the one-line or riser (`OL`, `RD`, or
/// blank) given the one-line's: its breaker (`600A/3P`) and its feeder type (the feeder schedule's
/// sets, conductors and conduit). A circuit is the one-line's label when its description names it
/// (`PANELBOARD DP1`, `ELEV-1`) or is one word its tag's letters start (`ELEVATOR` for `ELEV-1`).
/// The count of circuits given something.
pub(crate) fn fill_from_oneline(
    sp: &mut cadaddy_walk3d::BuildingSpec,
    feeds: &[super::equipsub::OnelineLoad],
    types: &[super::gearbuild::FeederType],
) -> usize {
    let deferred = |v: &str| v.is_empty() || v.eq_ignore_ascii_case("OL") || v.eq_ignore_ascii_case("RD");
    let mut n = 0;
    for p in &mut sp.mep.panel_schedules {
        for c in &mut p.circuits {
            if !deferred(&c.breaker) && !deferred(&c.wire) {
                continue;
            }
            let up = c.description.to_ascii_uppercase();
            let words: Vec<&str> = up.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-').filter(|w| !w.is_empty()).collect();
            let named = |f: &&super::equipsub::OnelineLoad| {
                words.iter().any(|w| w.eq_ignore_ascii_case(&f.tag))
                    || (words.len() == 1 && super::equipsub::stem(&f.tag).is_some_and(|st| words.first().is_some_and(|w| w.starts_with(&st))))
            };
            let Some(f) = feeds.iter().find(named) else { continue };
            let mut given = false;
            if let Some((amps, poles)) = super::equipsub::ocpd_of(&f.ocpd).filter(|_| deferred(&c.breaker)) {
                c.breaker = format!("{amps:.0}");
                c.poles = poles;
                given = true;
            }
            if deferred(&c.wire) && !f.feeder.is_empty() {
                c.wire = match types.iter().find(|t| t.tag.eq_ignore_ascii_case(&f.feeder)) {
                    Some(t) => format!(
                        "feeder {}: {} set(s) of {} {} {}, ground {}, in {}\" conduit",
                        t.tag, t.conduits, t.conductors, t.size, t.material, t.ground_size, t.conduit_in
                    ),
                    None => format!("feeder {}", f.feeder),
                };
                given = true;
            }
            if given {
                if !c.source.contains("one-line") {
                    c.source = format!("{}; breaker and feeder from the one-line", c.source).trim_start_matches("; ").to_string();
                }
                n += 1;
            }
        }
    }
    n
}

/// What a circuit's breaker protects against, by its codes and the legend: (ground fault, arc
/// fault). `None` when its codes don't say (no codes, or ones the legend doesn't define).
pub(crate) fn protection(c: &PanelCircuit, legend: &BTreeMap<String, String>) -> (bool, bool) {
    protection_of(&c.protection, legend)
}

/// [`protection`] of a breaker's codes.
pub(crate) fn protection_of(codes: &[String], legend: &BTreeMap<String, String>) -> (bool, bool) {
    let mut gf = false;
    let mut af = false;
    for code in codes {
        let meaning = legend.get(code).map(String::as_str).unwrap_or_default();
        if matches!(code.as_str(), "GF" | "GFI" | "GFCI" | "GFP" | "GFEP" | "DF" | "DFCI")
            || meaning.contains("GROUND FAULT")
            || meaning.contains("GFCI")
            || meaning.contains("DUAL")
        {
            gf = true;
        }
        if matches!(code.as_str(), "AF" | "AFCI" | "AFI" | "DF" | "DFCI")
            || meaning.contains("ARC FAULT")
            || meaning.contains("AFCI")
            || meaning.contains("DUAL")
        {
            af = true;
        }
    }
    (gf, af)
}

pub fn specs() -> Vec<super::CommandSpec> {
    vec![super::CommandSpec::new(CMD, "Panel Schedules", run)
        .params("{panel?: tag (one panel; default every one), csv?: path (write every circuit, one row each), place?: {panel, at: [x,y]} (draw that panel's schedule as a CADaddy table at a point)} → {panels: [{panel, typical_of?, sheet, header: {bus amps, main size / type, volts/phase, mounting, location, nema rating, aic rating, afc value…}, circuits: [{number, description, breaker_amps, poles, wire, load_va, protection: {codes, ground_fault, arc_fault, meaning}, devices: {count, rooms, by_kind}}], totals: {circuits, spaces, spares, connected_va, typed}, legend}], csv?, placed?: handle}: the panel schedules the job read off the prints, as data: each circuit's breaker and its type, wire and load, and the model's devices on it")
        .menu(&["Tools", "Building", "Panel Schedules"])]
}

const CMD: &str = "panels.schedule";

/// One circuit's devices in the model: (count, rooms, kind → count).
type OnCircuit = (usize, std::collections::BTreeSet<String>, BTreeMap<String, usize>);

fn circuit_json(
    c: &PanelCircuit,
    legend: &BTreeMap<String, String>,
    on: Option<&OnCircuit>,
    built: Option<&super::propfill::AsBuiltBreakers>,
) -> serde_json::Value {
    let (gf, af) = built.map_or_else(|| protection(c, legend), |b| protection_of(&b.codes, legend));
    let meaning: Vec<String> = c.protection.iter().map(|k| legend.get(k).map_or_else(|| k.clone(), |m| format!("{k}: {m}"))).collect();
    serde_json::json!({
        "number": c.number, "description": c.description, "breaker_amps": c.breaker.parse::<f64>().ok(), "poles": c.poles,
        "wire": c.wire, "load_va": c.load_va,
        "protection": {"codes": c.protection, "ground_fault": gf, "arc_fault": af, "meaning": meaning,
                       "as_built": built.map(|b| serde_json::json!({"codes": b.codes, "source": b.source}))},
        "devices": on.map_or(serde_json::json!({"count": 0}), |(n, rooms, kinds)| serde_json::json!({"count": n, "rooms": rooms.len(), "by_kind": kinds})),
    })
}

fn run(s: &mut crate::Session, p: &serde_json::Value) -> crate::Result<serde_json::Value> {
    use cadaddy_doc::props;
    use serde_json::{Value, json};
    let (sp, stored) = super::bspec::current(s, CMD)?;
    let want = super::str_param(p, "panel").map(|t| t.trim().to_ascii_uppercase());
    let schedules: Vec<&PanelSchedule> =
        sp.mep.panel_schedules.iter().filter(|x| want.as_ref().is_none_or(|w| x.panel.eq_ignore_ascii_case(w))).collect();
    if !stored || sp.mep.panel_schedules.is_empty() {
        return Ok(json!({"panels": [], "note": "no panel schedules in this drawing: a job reads them off the electrical sheets (job.start)"}));
    }
    if schedules.is_empty() {
        return Err(super::bad(CMD, format!("no schedule for panel {}", want.unwrap_or_default())));
    }
    // The model's devices on each panel's circuits (a typical room panel's, across its rooms).
    let d = s.doc()?;
    let mut on: BTreeMap<(String, String), OnCircuit> = BTreeMap::new();
    for (e, base) in super::elemprops::elements(d).take(2_000_000) {
        if !matches!(base.as_str(), "device" | "light_fixture" | "fixture" | "equipment") {
            continue;
        }
        let st = props::read(&e.common.xdata);
        let text = |k: &str| st.get(k).filter(|v| v.has_value()).map(|v| v.text().trim().to_string()).unwrap_or_default();
        let (panel, circuit) = (text("panel").to_ascii_uppercase(), text("circuit"));
        if panel.is_empty() || circuit.is_empty() {
            continue;
        }
        let n = circuit.rsplit_once(['-', '/', '.']).map_or(circuit.as_str(), |(_, n)| n).trim().to_string();
        let kind = super::elemprops::reported_kind(e, base);
        let slot = on.entry((panel, n)).or_default();
        slot.0 += 1;
        slot.1.insert(text("room"));
        *slot.2.entry(kind).or_default() += 1;
    }
    let mut panels = Vec::new();
    let mut csv =
        String::from("panel,typical_of,sheet,circuit,description,breaker_amps,poles,wire,load_va,protection,ground_fault,arc_fault,devices\n");
    let cell = |v: &str| if v.contains([',', '"']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() };
    for sch in &schedules {
        let key = sch.panel.to_ascii_uppercase();
        let built = |c: &PanelCircuit| s.doc().ok().and_then(|d| super::propfill::as_built_breakers(d, &sch.panel, &c.description));
        let circuits: Vec<Value> =
            sch.circuits.iter().map(|c| circuit_json(c, &sch.legend, on.get(&(key.clone(), c.number.clone())), built(c).as_ref())).collect();
        let desc = |c: &PanelCircuit, w: &str| c.description.to_ascii_uppercase().contains(w);
        for c in &sch.circuits {
            let as_built = built(c);
            let (gf, af) = as_built.as_ref().map_or_else(|| protection(c, &sch.legend), |b| protection_of(&b.codes, &sch.legend));
            let codes = as_built.as_ref().map_or(&c.protection, |b| &b.codes).join("/");
            let n = on.get(&(key.clone(), c.number.clone())).map_or(0, |x| x.0);
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                cell(&sch.panel),
                sch.typical_of.map(|t| t.to_string()).unwrap_or_default(),
                cell(&sch.source),
                c.number,
                cell(&c.description),
                cell(&c.breaker),
                c.poles,
                cell(&c.wire),
                c.load_va.map(|v| v.to_string()).unwrap_or_default(),
                cell(&codes),
                gf,
                af,
                n
            ));
        }
        panels.push(json!({
            "panel": sch.panel, "typical_of": sch.typical_of, "sheet": sch.source, "header": sch.header, "circuits": circuits,
            "totals": {
                "circuits": sch.circuits.len(),
                "spaces": sch.circuits.iter().filter(|c| desc(c, "SPACE")).count(),
                "spares": sch.circuits.iter().filter(|c| desc(c, "SPARE")).count(),
                "connected_va": sch.circuits.iter().filter_map(|c| c.load_va).sum::<f64>(),
                "typed": sch.circuits.iter().filter(|c| !c.protection.is_empty()).count(),
            },
            "legend": sch.legend,
        }));
    }
    let mut out = json!({"panels": panels});
    if let Some(path) = super::str_param(p, "csv").map(str::trim).filter(|x| !x.is_empty()) {
        super::exchange::guard_output(s, CMD, p, path)?;
        crate::writefile::write(path, &csv).map_err(|e| super::bad(CMD, format!("{path}: {e}")))?;
        out["csv"] = json!(path);
    }
    if let Some(pl) = p.get("place").filter(|v| !v.is_null()) {
        let tag = pl.get("panel").and_then(Value::as_str).ok_or_else(|| super::bad(CMD, "`place` needs `panel`"))?;
        let at = pl.get("at").cloned().ok_or_else(|| super::bad(CMD, "`place` needs `at`: [x, y]"))?;
        let sch = sp
            .mep
            .panel_schedules
            .iter()
            .find(|x| x.panel.eq_ignore_ascii_case(tag))
            .ok_or_else(|| super::bad(CMD, format!("no schedule for panel {tag}")))?;
        let header = ["CKT", "DESCRIPTION", "AMPS", "P", "WIRE", "LOAD (VA)", "TYPE"];
        let cells: Vec<Vec<String>> = sch
            .circuits
            .iter()
            .map(|c| {
                vec![
                    c.number.clone(),
                    c.description.clone(),
                    c.breaker.clone(),
                    c.poles.to_string(),
                    c.wire.clone(),
                    c.load_va.map(|v| format!("{v:.0}")).unwrap_or_default(),
                    c.protection.join("/"),
                ]
            })
            .collect();
        let facts: Vec<String> = sch.header.iter().map(|(k, v)| format!("{}: {v}", k.to_ascii_uppercase())).collect();
        let title =
            format!("PANEL {} ({}){}", sch.panel, sch.source, if facts.is_empty() { String::new() } else { format!(" - {}", facts.join("; ")) });
        let r =
            s.execute("table", &json!({"at": at, "rows": cells.len(), "cols": header.len(), "cells": cells, "title": title, "header": header}))?;
        out["placed"] = r.get("handle").cloned().unwrap_or(r);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_geom::Vec2;

    fn t(x: f64, y: f64, v: &str, h: f64) -> TextItem {
        TextItem { value: v.to_string(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
    }

    /// E-2's typical guest-room panel as the PDF holds it (span by span, its points).
    fn panel_g() -> Vec<TextItem> {
        let mut v = vec![
            t(1723.0, 951.0, "PANELBOARD G (TYPICAL OF 124 REQUIRED)", 10.0),
            t(1568.0, 934.0, "BUS AMPS:", 7.1),
            t(1672.0, 934.0, "125A", 7.1),
            t(1494.0, 885.0, "CKT", 6.3),
            t(1516.0, 885.0, "CIRCUIT", 6.3),
            t(1672.0, 885.0, "BREAKER", 6.3),
            t(1726.0, 885.0, "WIRE", 6.3),
            t(1763.0, 885.0, "LOAD", 6.3),
            t(1799.0, 885.0, "CONNECTED PER PHASE (VA)", 6.3),
            t(1944.0, 885.0, "LOAD", 7.1),
            t(1983.0, 885.0, "WIRE", 6.3),
            t(2009.0, 885.0, "BREAKER", 6.3),
            t(2058.0, 885.0, "CIRCUIT", 6.3),
            t(2218.0, 885.0, "CKT", 6.3),
            t(1499.0, 874.0, "#", 6.3),
            t(1516.0, 874.0, "DESCRIPTION", 6.3),
            t(1677.0, 874.0, "AMPS", 6.3),
            t(1708.0, 874.0, "P", 6.3),
            t(1727.0, 874.0, "SIZE", 6.3),
            t(1766.0, 874.0, "(VA)", 6.3),
            t(1947.0, 874.0, "(VA)", 6.3),
            t(1984.0, 874.0, "SIZE", 6.3),
            t(2013.0, 874.0, "P", 6.3),
            t(2031.0, 874.0, "AMPS", 6.3),
            t(2058.0, 874.0, "DESCRIPTION", 6.3),
            t(2223.0, 874.0, "#", 6.3),
        ];
        let rows: [(&str, &str, &str, &str, &str, &str, &str, &str); 8] = [
            ("AF", "1", "ROOM RCPT AND LIGHTING", "20", "1", "REFRIGERATOR", "2", "AF/GF"),
            ("AF/GF", "3", "BATHROOM RCPT AND LIGHTING", "20", "1", "MICROWAVE", "4", "AF/GF"),
            ("AF", "5", "KITCHEN", "20", "1", "KITCHEN", "6", "AF"),
            ("AF/GF", "7", "DISHWASHER (FIRST FLOOR ONLY)", "20", "1", "RANGE", "8", "AF/GF"),
            ("AF", "9", "DESK, TV, & NIGHT STAND RCPT", "20", "1", "SPACE ONLY", "10", ""),
            ("AF", "11", "RANGE EXHAUST (ADA ONLY)", "20", "1", "SPACE ONLY", "12", ""),
            ("", "13", "GUESTROOM PTAC-1", "20", "2", "SPACE ONLY", "14", ""),
            ("", "15", "", "", "", "SPACE ONLY", "16", ""),
        ];
        for (i, (lc, ln, ld, la, lp, rd, rn, rc)) in rows.iter().enumerate() {
            let y = 862.0 - 10.0 * i as f64;
            if !lc.is_empty() {
                v.push(t(1477.0 - 11.0 * f64::from(u8::from(lc.len() > 2)), y, lc, 6.3));
            }
            v.push(t(1499.0, y, ln, 6.3));
            if !ld.is_empty() {
                v.push(t(1516.0, y, ld, 7.1));
                v.push(t(1682.0, y, la, 7.1));
                v.push(t(1708.0, y, lp, 7.1));
                v.push(t(1730.0, y, "12", 7.1));
                v.push(t(1763.0, y, "1,204", 7.1));
            }
            if !rd.starts_with("SPACE") {
                // A wider number starts left of its column's `(VA)`, nearer its `LOAD`.
                v.push(t(if i == 1 { 1944.0 } else { 1947.0 }, y, "900", 7.1));
                v.push(t(1987.0, y, "12", 7.1));
                v.push(t(2013.0, y, "1", 7.1));
                v.push(t(2036.0, y, "20", 7.1));
            } else {
                v.push(t(1951.0, y, "0", 7.1));
            }
            v.push(t(2058.0, y, rd, 7.1));
            v.push(t(2221.0, y, rn, 6.3));
            if !rc.is_empty() {
                v.push(t(2240.0, y, rc, 6.3));
            }
        }
        v.push(t(1657.0, 772.0, "PER PHASE SUB-TOTALS", 7.1));
        v.push(t(1934.0, 762.0, "GF - GROUND FAULT INTERRUPTER", 7.1));
        v.push(t(1934.0, 752.0, "TS - VIA TIME SWITCH", 7.1));
        v
    }

    /// A main panel beside the next schedule on one header line: its breakers `OL` (see the
    /// one-line), three poles a row each with the number on the middle one; a legend under its label.
    fn main_beside_another() -> Vec<TextItem> {
        let x = |v: f64| v * 4.0;
        let mut v = vec![t(x(160.0), 200.0, "PANELBOARD MDP", 10.0)];
        for (vx, s) in [
            (50.0, "CKT"),
            (60.0, "CIRCUIT"),
            (120.0, "BREAKER"),
            (145.0, "WIRE"),
            (160.0, "LOAD"),
            (200.0, "LOAD"),
            (215.0, "WIRE"),
            (225.0, "BREAKER"),
            (245.0, "CIRCUIT"),
            (300.0, "CKT"),
            (330.0, "CKT"),
            (340.0, "CIRCUIT"),
        ] {
            v.push(t(x(vx), 180.0, s, 6.0));
        }
        for (vx, s) in [
            (52.0, "#"),
            (60.0, "DESCRIPTION"),
            (121.0, "AMPS"),
            (135.0, "P"),
            (145.0, "SIZE"),
            (161.0, "(VA)"),
            (201.0, "(VA)"),
            (215.0, "SIZE"),
            (225.0, "P"),
            (236.0, "AMPS"),
            (245.0, "DESCRIPTION"),
            (302.0, "#"),
            (332.0, "#"),
            (340.0, "DESCRIPTION"),
        ] {
            v.push(t(x(vx), 172.0, s, 6.0));
        }
        let cells: [(f64, f64, &str); 25] = [
            (160.0, 162.0, "100"),
            (200.0, 162.0, "10"),
            (322.0, 162.0, "GF"),
            (332.0, 162.0, "1"),
            (340.0, 162.0, "GUEST WASHER"),
            (52.0, 154.0, "1"),
            (60.0, 154.0, "PANELBOARD X"),
            (121.0, 154.0, "OL"),
            (135.0, 154.0, "3"),
            (146.0, 154.0, "OL"),
            (160.0, 154.0, "200"),
            (200.0, 154.0, "20"),
            (215.0, 154.0, "OL"),
            (225.0, 154.0, "3"),
            (236.0, 154.0, "OL"),
            (245.0, 154.0, "ELEVATOR"),
            (302.0, 154.0, "2"),
            (322.0, 154.0, "GF"),
            (332.0, 154.0, "3"),
            (340.0, 154.0, "LTG - THIRD FLOOR"),
            (160.0, 146.0, "300"),
            (200.0, 146.0, "30"),
            (332.0, 146.0, "5"),
            (340.0, 146.0, "SPARE"),
            (60.0, 130.0, "PER PHASE SUB-TOTALS"),
        ];
        for (vx, y, s) in cells {
            v.push(t(x(vx), y, s, 6.0));
        }
        v.push(t(x(200.0), 130.0, "LEGEND:", 6.0));
        v.push(t(x(200.0), 122.0, "GF - GROUND FAULT INTERRUPTER", 6.0));
        v.push(t(x(260.0), 122.0, "OL - RE: ONE-LINE DIAGRAM", 6.0));
        v
    }

    #[test]
    fn a_main_panel_beside_another_reads_its_own_columns_its_poles_loads_and_its_legend() {
        let s = read(&main_beside_another(), "E-2");
        let m = s.iter().find(|p| p.panel == "MDP").expect("MDP");
        let got: Vec<(&str, &str, &str, u32, Option<f64>)> =
            m.circuits.iter().map(|c| (c.number.as_str(), c.description.as_str(), c.breaker.as_str(), c.poles, c.load_va)).collect();
        assert_eq!(got, [("1", "PANELBOARD X", "OL", 3, Some(600.0)), ("2", "ELEVATOR", "OL", 3, Some(60.0))], "{m:#?}");
        assert_eq!(m.legend.get("OL").map(String::as_str), Some("RE: ONE-LINE DIAGRAM"));
        assert!(!m.legend.contains_key("LTG"), "a circuit's description is no legend line: {:?}", m.legend);
    }

    #[test]
    fn the_one_line_gives_a_circuits_breaker_and_feeder_the_schedule_leaves_to_it() {
        let mut sp = cadaddy_walk3d::BuildingSpec::default();
        let m = read(&main_beside_another(), "E-2").into_iter().find(|p| p.panel == "MDP").expect("MDP");
        sp.mep.panel_schedules.push(m);
        let feeds = [
            super::super::equipsub::OnelineLoad { tag: "X".into(), feeder: "604".into(), ocpd: "600A/3P".into() },
            super::super::equipsub::OnelineLoad { tag: "ELEV-1".into(), feeder: "74".into(), ocpd: "70A/3P".into() },
        ];
        let types = [super::super::gearbuild::FeederType {
            tag: "604".into(),
            conduits: 2,
            conduit_in: 3.0,
            conductors: 4,
            size: "350 kcmil".into(),
            material: "cu".into(),
            ground_size: "1".into(),
            source: String::new(),
        }];
        assert_eq!(fill_from_oneline(&mut sp, &feeds, &types), 2);
        let c = &sp.mep.panel_schedules[0].circuits;
        assert_eq!((c[0].breaker.as_str(), c[0].poles), ("600", 3));
        assert!(c[0].wire.starts_with("feeder 604: 2 set(s) of 4 350 kcmil"), "{}", c[0].wire);
        assert_eq!((c[1].description.as_str(), c[1].breaker.as_str(), c[1].wire.as_str()), ("ELEVATOR", "70", "feeder 74"));
    }

    #[test]
    fn a_two_sided_schedule_gives_each_circuit_its_breaker_and_its_breakers_type() {
        let s = read(&panel_g(), "E-2");
        assert_eq!(s.len(), 1);
        let g = &s[0];
        assert_eq!(g.panel, "G");
        assert_eq!(g.typical_of, Some(124));
        let c = |n: &str| g.circuits.iter().find(|c| c.number == n).unwrap_or_else(|| panic!("circuit {n}: {:#?}", g.circuits));
        assert_eq!(c("1").description, "ROOM RCPT AND LIGHTING");
        assert_eq!(c("1").protection, ["AF"]);
        assert_eq!(c("3").description, "BATHROOM RCPT AND LIGHTING");
        assert_eq!(c("3").protection, ["AF", "GF"]);
        assert_eq!(c("3").breaker, "20");
        assert_eq!(c("2").description, "REFRIGERATOR");
        assert_eq!(c("2").protection, ["AF", "GF"]);
        assert_eq!(c("2").breaker, "20");
        assert_eq!(c("6").protection, ["AF"]);
        assert_eq!(c("13").poles, 2);
        assert_eq!(c("1").wire, "12");
        assert_eq!(c("1").load_va, Some(1204.0));
        assert_eq!(c("2").wire, "12");
        assert_eq!(c("2").load_va, Some(900.0));
        assert_eq!(c("4").load_va, Some(900.0), "a load nearer its column's LOAD than its (VA)");
        assert_eq!(g.header.get("bus amps").map(String::as_str), Some("125A"), "{:?}", g.header);
        assert!(g.circuits.iter().all(|c| c.number != "15"), "15 is 13's second pole");
        assert_eq!(c("10").description, "SPACE ONLY");
        assert_eq!(g.legend.get("GF").map(String::as_str), Some("GROUND FAULT INTERRUPTER"));
        // Bath on a GFCI breaker; the kitchen's isn't.
        assert_eq!(protection(c("3"), &g.legend), (true, true));
        assert_eq!(protection(c("5"), &g.legend), (false, true));
        // The totals and the legend aren't circuits.
        assert!(g.circuits.iter().all(|c| !c.description.contains("TOTAL") && !c.description.contains("GROUND")), "{:#?}", g.circuits);
    }

    /// The same schedule as the PDF importer gives it (inches; a text's height its capitals').
    #[test]
    fn the_importers_geometry_reads_the_same() {
        let h = 0.06;
        let mut v = vec![t(23.93, 13.07, "PANELBOARD G (TYPICAL OF 124 REQUIRED)", 0.126), t(20.8, 12.3, "CKT", h), t(30.8, 12.3, "CKT", h)];
        for (x, w) in [
            (20.8, "#"),
            (21.4, "DESCRIPTION"),
            (23.2, "AMPS"),
            (23.3, "AMPS"),
            (23.7, "P"),
            (24.0, "SIZE"),
            (24.5, "(VA)"),
            (27.0, "(VA)"),
            (27.6, "SIZE"),
            (28.0, "P"),
            (28.2, "AMPS"),
            (28.9, "DESCRIPTION"),
            (30.9, "#"),
        ] {
            v.push(t(x, 12.2, w, h));
        }
        for (y, lc, ln, ld, rd, rn, rc) in [
            (12.0, "AF", "1", "ROOM RCPT AND LIGHTING", "REFRIGERATOR", "2", "AF/GF"),
            (11.9, "AF/GF", "3", "BATHROOM RCPT AND LIGHTING", "MICROWAVE", "4", "AF/GF"),
        ] {
            for (x, w) in [
                (20.45, lc),
                (20.8, ln),
                (21.1, ld),
                (23.4, "20"),
                (23.7, "1"),
                (24.0, "12"),
                (24.5, "1,204"),
                (27.0, "900"),
                (27.6, "12"),
                (28.0, "1"),
                (28.3, "20"),
                (28.6, rd),
                (30.9, rn),
                (31.1, rc),
            ] {
                v.push(t(x, y, w, 0.07));
            }
        }
        v.push(t(21.0, 11.0, "PER PHASE SUB-TOTALS", 0.07));
        let s = read(&v, "E-2");
        let g = s.first().unwrap_or_else(|| panic!("{s:#?}"));
        let c = |n: &str| g.circuits.iter().find(|c| c.number == n).unwrap_or_else(|| panic!("circuit {n}: {:#?}", g.circuits));
        for n in ["1", "2", "3", "4"] {
            assert_eq!(c(n).breaker, "20", "{:#?}", c(n));
            assert_eq!(c(n).poles, 1);
        }
        assert_eq!(c("1").protection, ["AF"]);
        assert_eq!(c("3").protection, ["AF", "GF"]);
        assert_eq!(c("4").description, "MICROWAVE");
    }

    #[test]
    fn a_feeder_row_naming_a_panel_is_a_circuit_not_the_tables_end() {
        // DP1 feeds the room panels: its first row "PANELBOARD G (4 UNITS)" sits a hair above its number.
        let h = 0.063;
        let mut v = vec![t(13.6, 24.59, "PANELBOARD DP1", 0.126), t(9.37, 23.84, "CKT", h), t(11.87, 23.84, "BREAKER", h)];
        for (x, w) in [(9.43, "#"), (9.99, "DESCRIPTION"), (11.88, "AMPS"), (12.31, "P"), (12.58, "SIZE")] {
            v.push(t(x, 23.70, w, h));
        }
        for (i, n) in ["1", "3", "5"].iter().enumerate() {
            let y = 23.53 - 0.28 * i as f64;
            v.push(t(9.67, y + 0.004, "PANELBOARD G (4 UNITS)", 0.07));
            v.push(t(9.43, y, n, h));
            v.push(t(11.93, y, "125", 0.07));
            v.push(t(12.32, y, "2", 0.07));
        }
        let s = read(&v, "E-2");
        let dp1 = s.iter().find(|p| p.panel == "DP1").unwrap_or_else(|| panic!("{s:#?}"));
        assert_eq!(dp1.circuits.len(), 3, "{:#?}", dp1.circuits);
        assert_eq!(
            (dp1.circuits[0].description.as_str(), dp1.circuits[0].breaker.as_str(), dp1.circuits[0].poles),
            ("PANELBOARD G (4 UNITS)", "125", 2)
        );
    }

    #[test]
    fn a_cell_naming_a_panel_is_no_schedule_title() {
        let mut v = panel_g();
        // DP2's feeder row "PANELBOARD G (4 UNITS)" in the table's text, a header below it.
        v.push(t(1516.0, 960.0, "PANELBOARD G2 (4 UNITS)", 7.1));
        let s = read(&v, "E-2");
        assert_eq!(s.iter().map(|p| p.panel.as_str()).collect::<Vec<_>>(), ["G"]);
    }

    #[test]
    fn schedule_titles_name_their_panel() {
        assert_eq!(title_panel("PANELBOARD G (TYPICAL OF 124 REQUIRED)"), Some(("G".into(), Some(124))));
        assert_eq!(title_panel("PANELBOARD DP1"), Some(("DP1".into(), None)));
        assert_eq!(title_panel("PANEL 'A'"), Some(("A".into(), None)));
        assert_eq!(title_panel("PANEL SCHEDULE - P1"), Some(("P1".into(), None)));
        assert_eq!(title_panel("PANELBOARD SHALL BE"), None);
        assert_eq!(title_panel("PANELBOARD DP1 DEMAND CALCULATION"), None);
        assert_eq!(title_panel("TOTAL CONNECTED PANELBOARD (VA)"), None);
    }

    /// A two-sided schedule with its phase columns (`A B C`) between the sides' loads, and its
    /// totals under the circuits with the legend beside them.
    fn two_sided_with_totals() -> Vec<TextItem> {
        let mut v = vec![
            t(250.0, 520.0, "PANELBOARD H1", 10.0),
            t(120.0, 505.0, "VOLTS/PHASE:", 6.0),
            t(190.0, 505.0, "480Y/277V, 3PH, 4W", 6.0),
            t(100.0, 490.0, "CKT", 6.0),
            t(115.0, 490.0, "DESCRIPTION", 6.0),
            t(200.0, 490.0, "AMPS", 6.0),
            t(215.0, 490.0, "P", 6.0),
            t(230.0, 490.0, "LOAD", 6.0),
            t(260.0, 490.0, "A", 6.0),
            t(280.0, 490.0, "B", 6.0),
            t(300.0, 490.0, "C", 6.0),
            t(330.0, 490.0, "LOAD", 6.0),
            t(345.0, 490.0, "P", 6.0),
            t(360.0, 490.0, "AMPS", 6.0),
            t(375.0, 490.0, "DESCRIPTION", 6.0),
            t(470.0, 490.0, "CKT", 6.0),
        ];
        let rows = [
            ("1", "LIGHTING EAST", "2,000", "2", "RECEPTACLES WEST", "1,000"),
            ("3", "LIGHTING WEST", "600", "4", "SPARE", "400"),
            ("5", "SIGN", "300", "6", "SPACE", "200"),
        ];
        for (i, (ln, ld, ll, rn, rd, rl)) in rows.iter().enumerate() {
            let y = 472.0 - 10.0 * i as f64;
            v.extend([t(100.0, y, ln, 6.0), t(115.0, y, ld, 6.0), t(202.0, y, "20", 6.0), t(215.0, y, "1", 6.0), t(230.0, y, ll, 6.0)]);
            v.extend([t(330.0, y, rl, 6.0), t(345.0, y, "1", 6.0), t(362.0, y, "20", 6.0), t(375.0, y, rd, 6.0), t(470.0, y, rn, 6.0)]);
        }
        v.extend([
            t(150.0, 445.0, "PER PHASE SUB-TOTALS", 6.0),
            t(258.0, 445.0, "3,000", 6.0),
            t(278.0, 445.0, "1,000", 6.0),
            t(298.0, 445.0, "500", 6.0),
            t(400.0, 445.0, "LEGEND:", 6.0),
            t(150.0, 435.0, "TOTAL CONNECTED PANELBOARD (VA)", 6.0),
            t(278.0, 435.0, "4,500", 6.0),
            t(400.0, 435.0, "GF - GROUND FAULT", 6.0),
            t(150.0, 425.0, "TOTAL PANELBOARD DEMAND (AMPS)", 6.0),
            t(280.0, 425.0, "7", 6.0),
            t(400.0, 425.0, "ST - SHUNT TRIP 12", 6.0),
        ]);
        v
    }

    /// A one-sided schedule (`TRIP`, `POLES`, `LOAD` columns) whose totals are in kVA and whose
    /// phase subtotals have no phase columns: in order.
    fn one_sided_kva() -> Vec<TextItem> {
        let mut v = vec![t(100.0, 300.0, "PANEL SCHEDULE - LP2", 9.0), t(80.0, 285.0, "CKT", 5.0), t(95.0, 285.0, "DESCRIPTION", 5.0)];
        v.extend([t(200.0, 285.0, "TRIP", 5.0), t(220.0, 285.0, "POLES", 5.0), t(250.0, 285.0, "LOAD", 5.0)]);
        for (i, (n, d, l)) in
            [("1", "LIGHTING LOBBY", "1,500"), ("2", "RECEPTACLES LOBBY", "1,000"), ("3", "WATER COOLER", "2,000")].iter().enumerate()
        {
            let y = 270.0 - 8.0 * i as f64;
            v.extend([t(82.0, y, n, 5.0), t(95.0, y, d, 5.0), t(202.0, y, "20", 5.0), t(222.0, y, "1", 5.0), t(250.0, y, l, 5.0)]);
        }
        v.extend([
            t(95.0, 245.0, "TOTAL CONNECTED LOAD (KVA)", 5.0),
            t(250.0, 245.0, "4.5", 5.0),
            t(95.0, 237.0, "TOTAL DEMAND LOAD (KVA)", 5.0),
            t(250.0, 237.0, "5.1", 5.0),
            t(95.0, 229.0, "CONNECTED PER PHASE", 5.0),
            t(230.0, 229.0, "2,500", 5.0),
            t(260.0, 229.0, "2,000", 5.0),
        ]);
        v
    }

    #[test]
    fn a_schedules_totals_and_phase_subtotals_are_read_by_their_columns_whatever_the_layout() {
        let got = read(&two_sided_with_totals(), "X-1");
        let h = got.iter().find(|p| p.panel == "H1").unwrap();
        assert_eq!(h.circuits.len(), 6, "{:#?}", h.circuits);
        let want: BTreeMap<String, f64> =
            [("phase_a_va", 3000.0), ("phase_b_va", 1000.0), ("phase_c_va", 500.0), ("connected_va", 4500.0), ("demand_a", 7.0)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
        assert_eq!(h.totals, want, "the legend beside a total is no value of it");
        let got = read(&one_sided_kva(), "X-2");
        let lp = got.iter().find(|p| p.panel == "LP2").unwrap();
        assert_eq!(lp.circuits.len(), 3, "{:#?}", lp.circuits);
        let want: BTreeMap<String, f64> = [("connected_va", 4500.0), ("demand_va", 5100.0), ("phase_a_va", 2500.0), ("phase_b_va", 2000.0)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        assert_eq!(lp.totals, want, "kVA reads as VA; phases in order without phase columns");
        // A total's number a hair above its label (the import's rounding): still the totals, never
        // more rows of the last circuit.
        let mut nudged = two_sided_with_totals();
        for t in nudged.iter_mut().filter(|t| t.value.chars().all(|c| c.is_ascii_digit() || c == ',') && t.at.y < 450.0) {
            t.at.y += 0.01;
        }
        let h = read(&nudged, "X-1").into_iter().find(|p| p.panel == "H1").unwrap();
        assert_eq!(h.circuits.len(), 6, "{:#?}", h.circuits);
        assert_eq!(h.totals.get("connected_va"), Some(&4500.0), "{:?}", h.totals);
        assert_eq!(h.totals.get("phase_a_va"), Some(&3000.0), "{:?}", h.totals);
        assert_eq!(h.circuits.iter().find(|c| c.number == "5").and_then(|c| c.load_va), Some(300.0), "{:#?}", h.circuits);
        // The older fixture: its subtotal line has no numbers.
        let g = read(&panel_g(), "E-601");
        assert!(g.iter().all(|p| p.totals.values().all(|v| v.is_finite())));
    }
}
