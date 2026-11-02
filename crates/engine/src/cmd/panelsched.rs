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
    codes: Vec<String>,
}

/// The schedules on one sheet's texts (its PDF page re-read, or the drawing's).
pub(crate) fn read(texts: &[TextItem], sheet: &str) -> Vec<PanelSchedule> {
    let flat: Vec<&TextItem> =
        texts.iter().filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.x.is_finite() && t.at.y.is_finite()).collect();
    let legend: BTreeMap<String, String> = flat.iter().filter_map(|t| legend_line(&t.value)).collect();
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
        let line: Vec<&&TextItem> = ckts.iter().copied().filter(|t| (t.at.y - top).abs() <= 0.6 * row_h).collect();
        let (Some(l), Some(r)) =
            (line.iter().min_by(|a, b| a.at.x.total_cmp(&b.at.x)).copied(), line.iter().max_by(|a, b| a.at.x.total_cmp(&b.at.x)).copied())
        else {
            continue;
        };
        let two = r.at.x - l.at.x > 20.0 * row_h;
        // The header's second line (`# | DESCRIPTION | AMPS | P`), else its first.
        let head: Vec<&&TextItem> = flat
            .iter()
            .filter(|t| t.at.y <= top + 0.6 * row_h && t.at.y > top - 2.6 * row_h && t.at.x > l.at.x - row_h && t.at.x < r.at.x + 3.0 * row_h)
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
        let mut sides =
            vec![Side { num: lnum, amps: first_after(&amps, lnum, mid), poles: first_after(&poles, lnum, mid), others: half(true), left: true }];
        if two {
            sides.push(Side {
                num: rnum,
                amps: last_before(&amps, rnum, mid),
                poles: last_before(&poles, rnum, mid),
                others: half(false),
                left: false,
            });
        }
        let head_bottom = head.iter().map(|t| t.at.y).fold(top, f64::min);
        let (x0, x1) = (lnum - 8.0 * row_h, if two { rnum + 8.0 * row_h } else { lnum + 120.0 * row_h });
        let mut below: Vec<&&TextItem> = flat.iter().filter(|t| t.at.y < head_bottom - 0.5 * row_h && t.at.x >= x0 && t.at.x <= x1).collect();
        below.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        // Rows, line by line, until the subtotals, a gap of a few rows, or the next title.
        let mut lines: Vec<Vec<&TextItem>> = Vec::new();
        let mut last_y = head_bottom;
        let mut step: Option<f64> = None;
        for t in below {
            match lines.last_mut() {
                Some(ln) if ln.first().is_some_and(|f| (f.at.y - t.at.y).abs() <= 0.6 * row_h) => ln.push(t),
                _ => {
                    let u = t.value.trim().to_ascii_uppercase();
                    let gap = last_y - t.at.y;
                    if u.starts_with("PER PHASE")
                        || u.starts_with("TOTAL")
                        || u.contains("SUB-TOTAL")
                        || u.contains("SUBTOTAL")
                        || title_panel(&u).is_some()
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
                let numeric = number(v.trim_end_matches('A')).is_some();
                if numeric {
                    let best = [(0u8, Some(side.num)), (1, side.amps), (2, side.poles)]
                        .into_iter()
                        .chain(side.others.iter().map(|o| (9u8, Some(*o))))
                        .filter_map(|(k, cx)| cx.map(|cx| ((x - cx).abs(), k)))
                        .filter(|(d, _)| *d <= COLUMN_REACH * row_h)
                        .min_by(|a, b| a.0.total_cmp(&b.0));
                    match best {
                        Some((_, 0)) if c.number.is_empty() && number(v).is_some() => c.number = v.to_string(),
                        Some((_, 1)) if c.amps.is_empty() => c.amps = v.trim_end_matches('A').to_string(),
                        Some((_, 2)) if c.poles.is_empty() => c.poles = v.to_string(),
                        _ => {}
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
            for c in cells {
                let Some(n) = number(&c.number) else { continue };
                if c.description.is_empty() && c.amps.is_empty() {
                    // The second pole of the breaker above it.
                    continue;
                }
                circuits.push(PanelCircuit {
                    number: n.to_string(),
                    description: c.description.join(" "),
                    room: String::new(),
                    breaker: c.amps,
                    poles: c.poles.parse::<u32>().ok().filter(|p| (1..=3).contains(p)).unwrap_or(1),
                    protection: c.codes,
                    source: sheet.to_string(),
                });
            }
        }
        if circuits.is_empty() {
            continue;
        }
        circuits.sort_by_key(|c| c.number.parse::<u32>().unwrap_or(u32::MAX));
        out.push(PanelSchedule { panel, typical_of, source: sheet.to_string(), circuits, legend: legend.clone() });
    }
    out
}

/// What a circuit's breaker protects against, by its codes and the legend: (ground fault, arc
/// fault). `None` when its codes don't say (no codes, or ones the legend doesn't define).
pub(crate) fn protection(c: &PanelCircuit, legend: &BTreeMap<String, String>) -> (bool, bool) {
    let mut gf = false;
    let mut af = false;
    for code in &c.protection {
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
                v.push(t(1947.0, y, "900", 7.1));
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
        assert_eq!(title_panel("TOTAL CONNECTED PANELBOARD (VA)"), None);
    }
}
