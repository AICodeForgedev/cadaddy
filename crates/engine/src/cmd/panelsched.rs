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
                    let best = [(0u8, Some(side.num)), (1, side.amps), (2, side.poles), (3, side.wire), (4, side.load)]
                        .into_iter()
                        .chain(side.others.iter().map(|o| (9u8, Some(*o))))
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
                    wire: c.wire,
                    load_va: c.load.parse::<f64>().ok().filter(|v| v.is_finite()),
                    source: sheet.to_string(),
                });
            }
        }
        if circuits.is_empty() {
            continue;
        }
        circuits.sort_by_key(|c| c.number.parse::<u32>().unwrap_or(u32::MAX));
        let header = header_facts(&flat, top + 0.6 * row_h, title.at.y, l.at.x - 2.0 * row_h, head_x1 + 8.0 * row_h, row_h);
        out.push(PanelSchedule { panel, typical_of, source: sheet.to_string(), circuits, legend: legend.clone(), header });
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

pub fn specs() -> Vec<super::CommandSpec> {
    vec![super::CommandSpec::new(CMD, "Panel Schedules", run)
        .params("{panel?: tag (one panel; default every one), csv?: path (write every circuit, one row each), place?: {panel, at: [x,y]} (draw that panel's schedule as a CADaddy table at a point)} → {panels: [{panel, typical_of?, sheet, header: {bus amps, main size / type, volts/phase, mounting, location, nema rating, aic rating, afc value…}, circuits: [{number, description, breaker_amps, poles, wire, load_va, protection: {codes, ground_fault, arc_fault, meaning}, devices: {count, rooms, by_kind}}], totals: {circuits, spaces, spares, connected_va, typed}, legend}], csv?, placed?: handle}: the panel schedules the job read off the prints, as data: each circuit's breaker and its type, wire and load, and the model's devices on it")
        .menu(&["Tools", "Building", "Panel Schedules"])]
}

const CMD: &str = "panels.schedule";

/// One circuit's devices in the model: (count, rooms, kind → count).
type OnCircuit = (usize, std::collections::BTreeSet<String>, BTreeMap<String, usize>);

fn circuit_json(c: &PanelCircuit, legend: &BTreeMap<String, String>, on: Option<&OnCircuit>) -> serde_json::Value {
    let (gf, af) = protection(c, legend);
    let meaning: Vec<String> = c.protection.iter().map(|k| legend.get(k).map_or_else(|| k.clone(), |m| format!("{k}: {m}"))).collect();
    serde_json::json!({
        "number": c.number, "description": c.description, "breaker_amps": c.breaker.parse::<f64>().ok(), "poles": c.poles,
        "wire": c.wire, "load_va": c.load_va,
        "protection": {"codes": c.protection, "ground_fault": gf, "arc_fault": af, "meaning": meaning},
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
        return Err(super::bad(CMD, "no panel schedules in this drawing: a job reads them off the electrical sheets (job.start)"));
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
        let circuits: Vec<Value> = sch.circuits.iter().map(|c| circuit_json(c, &sch.legend, on.get(&(key.clone(), c.number.clone())))).collect();
        let desc = |c: &PanelCircuit, w: &str| c.description.to_ascii_uppercase().contains(w);
        for c in &sch.circuits {
            let (gf, af) = protection(c, &sch.legend);
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
                cell(&c.protection.join("/")),
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
        std::fs::write(path, &csv).map_err(|e| super::bad(CMD, format!("{path}: {e}")))?;
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
        assert_eq!(c("1").wire, "12");
        assert_eq!(c("1").load_va, Some(1204.0));
        assert_eq!(c("2").wire, "12");
        assert_eq!(c("2").load_va, Some(900.0));
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
}
