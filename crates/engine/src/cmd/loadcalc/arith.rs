//! The panel and load-calculation tables' own arithmetic, cross-checked (Q6649): what a person
//! checking the electrical sheets adds up before the feeders are bought.
//!
//! - **Feeder rows**: a row of a panel's schedule that feeds another panel (`PANELBOARD B`,
//!   `PANELBOARD U (4 UNITS)`) carries that panel's load: its own schedule's connected (or demand)
//!   total, times its units, within rounding. Two rows that each carry the other's panel's total
//!   are swapped.
//! - **Per-unit counts**: a demand calculation's line for a per-unit item (`DISHWASHER (X12)
//!   (650VA/SUITE): 7.80 KVA`) counts it as many times as its total over its VA; its printed
//!   count and its total agree, and an item only some units have is counted as many times as the
//!   plans show it on that calculation's feeder (the units under its panel that have a load on the
//!   unit panel's circuit for it), else over every calculation together.
//!
//! The rules (words, tolerances, cites) are `loadcalc.json`'s `arithmetic`, never a job's values.
//! Each mismatch is a correction the field would make: the feeder row takes its panel's total and
//! the line the plans' count, the printed value kept beside it ([`correct`]).

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::Drawing;
use cadaddy_walk3d::{BuildingSpec, LoadSummary, PanelSchedule, RunCorrection};
use serde::Deserialize;

use super::{MAX_PANELS, MAX_UNITS, Rules, feeds, model_devices, summary_units, va, words};

/// Summaries and lines one check reads at most (hostile specs).
const MAX_SUMMARIES: usize = 200;
const MAX_LINES: usize = 200;
/// Feeder chain steps followed down from a calculation's panel.
const MAX_CHAIN: usize = 12;
/// Feeder runs read for their chains at most.
const MAX_RUNS: usize = 200_000;

/// The arithmetic check's rules (`loadcalc.json` → `arithmetic`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Arithmetic {
    /// The totals of a fed panel's schedule a feeder row may carry.
    pub feeder_row_totals: Vec<String>,
    pub feeder_row_cite: String,
    /// Words after a per-unit VA (`650VA/SUITE`, `50 VA PER UNIT`).
    pub per_unit_words: Vec<String>,
    /// Words that say nothing about which item a line is (`RECEPTACLE`, `CONNECTION`, `FLOOR`).
    pub item_stop_words: Vec<String>,
    /// A count and a total agree within this share (rounding of the printed kVA).
    pub count_tolerance_pct: f64,
    pub count_cite: String,
    pub line_cite: String,
    /// The E-FEEDER runs' chains (`feeder 3 DP→U-101→U-201`) are read for the units a panel feeds.
    pub feeder_system: String,
}

/// One mismatch the tables' arithmetic shows, with the correction the field makes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Mismatch {
    /// `feeder_row`, `line_total` or `unit_count`.
    pub kind: &'static str,
    /// The schedule's panel (a feeder row), or the calculation's panel (empty: the building's).
    pub panel: String,
    /// The row's circuit number, or the calculation's line label.
    pub item: String,
    /// The sheets that show the two sides.
    pub sheets: Vec<String>,
    /// The value printed and the value the other document (or the plans) gives.
    pub design: f64,
    pub corrected: f64,
    /// What is wrong, in words, and how the build carries it.
    pub message: String,
    pub correction: String,
    pub cite: String,
    /// Where it is: the schedule and row, or the summary and line (indexes into the spec).
    pub at: (usize, usize),
}

/// The checks' measure: the rows and lines looked at, and the mismatches.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Report {
    pub feeder_rows: usize,
    pub count_lines: usize,
    pub mismatches: Vec<Mismatch>,
}

/// A row's printed load: the drawings' value where a correction replaced it.
fn printed_load(c: &cadaddy_walk3d::PanelCircuit) -> Option<f64> {
    c.design_load_va.or(c.load_va).filter(|v| v.is_finite() && *v >= 0.0)
}

/// A fed panel's totals a feeder row may carry: its printed totals (connected, demand), else the
/// sum of its rows.
fn panel_totals(sch: &PanelSchedule, a: &Arithmetic) -> Vec<f64> {
    let printed: Vec<f64> = a.feeder_row_totals.iter().filter_map(|k| sch.totals.get(k).copied()).filter(|v| v.is_finite() && *v > 0.0).collect();
    if !printed.is_empty() {
        return printed;
    }
    let sum: f64 = sch.circuits.iter().filter_map(printed_load).sum();
    if sum > 0.0 { vec![sum] } else { Vec::new() }
}

/// One feeder row against the panel it feeds.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RowCheck {
    pub panel: String,
    pub circuit: String,
    pub child: String,
    pub units: u32,
    pub row: f64,
    /// The fed panel's total times its units (the first total it may carry).
    pub expected: f64,
    pub agrees: bool,
    /// Another row of the same schedule whose fed panel's total this row carries.
    pub swapped_with: Option<(String, String)>,
    pub sheet: String,
    pub child_sheet: String,
    pub at: (usize, usize),
}

/// Every feeder row of the spec's schedules against the panel it feeds (the rule `bim.loadcalc`
/// reports and `rfi.read` raises: one rule).
pub(super) fn feeder_rows(sp: &BuildingSpec, rules: &Rules) -> Vec<RowCheck> {
    let a = &rules.arithmetic;
    let tags: BTreeSet<String> = sp.mep.panel_schedules.iter().map(|p| p.panel.trim().to_ascii_uppercase()).filter(|t| !t.is_empty()).collect();
    let by_tag: BTreeMap<String, &PanelSchedule> =
        sp.mep.panel_schedules.iter().take(MAX_PANELS).rev().map(|p| (p.panel.trim().to_ascii_uppercase(), p)).collect();
    let mut out = Vec::new();
    for (si, sch) in sp.mep.panel_schedules.iter().enumerate().take(MAX_PANELS) {
        let key = sch.panel.trim().to_ascii_uppercase();
        // Each row feeding a panel with a total: (row index, child, units, row's load, child's totals × units).
        let fed: Vec<(usize, String, u32, f64, Vec<f64>, String)> = sch
            .circuits
            .iter()
            .enumerate()
            .take(super::MAX_CIRCUITS)
            .filter_map(|(ci, c)| {
                let (child, n) = feeds(c, &key, &tags, &sp.mep.feeders, rules)?;
                let row = printed_load(c)?;
                let cs = by_tag.get(&child)?;
                let totals: Vec<f64> = panel_totals(cs, a).into_iter().map(|t| t * f64::from(n)).collect();
                (!totals.is_empty()).then(|| (ci, child, n, row, totals, cs.source.clone()))
            })
            .collect();
        for (ci, child, n, row, totals, child_sheet) in &fed {
            let agrees = totals.iter().any(|t| (row - t).abs() <= rules.tolerance(*row, *t));
            let swapped_with = (!agrees)
                .then(|| {
                    fed.iter().filter(|o| o.0 != *ci && o.1 != *child).find_map(|o| {
                        o.4.iter().any(|t| (row - t).abs() <= rules.tolerance(*row, *t)).then(|| {
                            let number = sch.circuits.get(o.0).map(|c| c.number.clone()).unwrap_or_default();
                            (number, o.1.clone())
                        })
                    })
                })
                .flatten();
            out.push(RowCheck {
                panel: key.clone(),
                circuit: sch.circuits.get(*ci).map(|c| c.number.clone()).unwrap_or_default(),
                child: child.clone(),
                units: *n,
                row: *row,
                expected: totals.first().copied().unwrap_or_default(),
                agrees,
                swapped_with,
                sheet: sch.source.clone(),
                child_sheet: child_sheet.clone(),
                at: (si, *ci),
            });
        }
    }
    out
}

/// A text's words with numbers glued to their units split apart and thousands commas gone
/// (`(3,000VA/SUITE):` → `3000`, `VA`, `SUITE`; `X13` → `X`, `13`).
fn tokens(t: &str) -> Vec<String> {
    let up = t.to_ascii_uppercase();
    let chars: Vec<char> = up.chars().take(600).collect();
    let mut s = String::with_capacity(chars.len() + 16);
    for (i, c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|j| chars.get(j)).copied();
        let next = chars.get(i + 1).copied();
        if *c == ',' && prev.is_some_and(|p| p.is_ascii_digit()) && next.is_some_and(|n| n.is_ascii_digit()) {
            continue;
        }
        if c.is_ascii_digit() || *c == '.' {
            if prev.is_some_and(|p| p.is_ascii_alphabetic()) {
                s.push(' ');
            }
            s.push(*c);
        } else if c.is_ascii_alphabetic() {
            if prev.is_some_and(|p| p.is_ascii_digit()) {
                s.push(' ');
            }
            s.push(*c);
        } else {
            s.push(' ');
        }
    }
    s.split_whitespace().map(|w| w.trim_matches('.').to_string()).filter(|w| !w.is_empty()).collect()
}

fn num(t: &str) -> Option<f64> {
    (t.chars().any(|c| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .then(|| t.parse::<f64>().ok())
        .flatten()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

/// A demand calculation line's per-unit item: its VA a unit, its total VA (as printed: kVA read
/// as VA, with the half of its last printed digit it may be rounded by), its printed count (`X13`)
/// and the words that say what it is.
#[derive(Clone, Debug, PartialEq)]
struct PerUnit {
    each: f64,
    total: f64,
    rounding: f64,
    count: Option<u32>,
    item: Vec<String>,
}

fn per_unit(text: &str, a: &Arithmetic) -> Option<PerUnit> {
    let tk = tokens(text);
    let unit = |w: &str| a.per_unit_words.iter().any(|u| u.eq_ignore_ascii_case(w));
    let is_va = |w: &str| w == "VA" || w == "W";
    // `650 VA SUITE` (the slash gone) or `650 VA PER SUITE`.
    let at = (0..tk.len()).find(|&i| {
        tk.get(i).and_then(|w| num(w)).is_some()
            && tk.get(i + 1).is_some_and(|w| is_va(w))
            && (tk.get(i + 2).is_some_and(|w| unit(w)) || (tk.get(i + 2).is_some_and(|w| w == "PER") && tk.get(i + 3).is_some_and(|w| unit(w))))
    })?;
    let each = tk.get(at).and_then(|w| num(w)).filter(|v| *v > 0.0)?;
    let after = at + if tk.get(at + 2).is_some_and(|w| w == "PER") { 4 } else { 3 };
    // Its total: the last number after it, kVA when its unit says so.
    let (ti, tv) = tk.iter().enumerate().skip(after).filter_map(|(i, w)| num(w).map(|v| (i, v))).next_back()?;
    let kilo = tk.get(ti + 1).is_some_and(|w| w == "KVA" || w == "KW");
    let scale = if kilo { 1000.0 } else { 1.0 };
    let decimals = tk.get(ti).and_then(|w| w.split_once('.')).map_or(0, |(_, d)| d.len().min(6));
    let rounding = 0.5 * 10f64.powi(-(decimals as i32)) * scale;
    // A printed count before it: `X13`, `X 13`.
    let count = (0..at)
        .find_map(|i| (tk.get(i).is_some_and(|w| w == "X")).then(|| tk.get(i + 1).and_then(|w| w.parse::<u32>().ok())).flatten())
        .filter(|n| (1..=MAX_UNITS).contains(n));
    let stop = |w: &str| a.item_stop_words.iter().any(|s| s.eq_ignore_ascii_case(w)) || unit(w);
    let item: Vec<String> =
        tk.iter().take(at).filter(|w| w.chars().all(|c| c.is_ascii_alphabetic()) && w.len() > 1 && !stop(w)).cloned().collect::<Vec<_>>();
    Some(PerUnit { each, total: tv * scale, rounding, count, item })
}

/// The line's count from its total over its VA, when that is a whole number (within rounding).
fn count_of(p: &PerUnit, a: &Arithmetic) -> Option<u32> {
    let x = p.total / p.each;
    let n = x.round();
    let tol = (a.count_tolerance_pct / 100.0 * p.total).max(p.rounding);
    (n >= 1.0 && n <= f64::from(MAX_UNITS) && (n * p.each - p.total).abs() <= tol).then_some(n as u32)
}

/// The unit panel circuit an item is on: the typical schedules' circuit whose description's
/// words (less the stop words) best share the item's; none when two tie or nothing is shared.
fn item_circuit(item: &[String], sp: &BuildingSpec, a: &Arithmetic) -> Option<(String, String, String)> {
    let stop = |w: &str| a.item_stop_words.iter().any(|s| s.eq_ignore_ascii_case(w)) || a.per_unit_words.iter().any(|s| s.eq_ignore_ascii_case(w));
    let want: BTreeSet<&str> = item.iter().map(String::as_str).collect();
    if want.is_empty() {
        return None;
    }
    let mut best: Vec<(f64, (String, String, String))> = Vec::new();
    for sch in sp.mep.panel_schedules.iter().take(MAX_PANELS).filter(|s| s.typical_of.is_some()) {
        for c in sch.circuits.iter().take(super::MAX_CIRCUITS) {
            let ws: BTreeSet<String> = words(&c.description).into_iter().filter(|w| w.chars().all(|c| c.is_ascii_alphabetic()) && !stop(w)).collect();
            let shared = ws.iter().filter(|w| want.contains(w.as_str())).count();
            if shared == 0 {
                continue;
            }
            let score = shared as f64 / (ws.len() + want.len() - shared) as f64;
            let num = c.number.trim().trim_start_matches('0').to_string();
            best.push((score, (sch.panel.trim().to_ascii_uppercase(), num, c.description.trim().to_string())));
        }
    }
    best.sort_by(|x, y| y.0.total_cmp(&x.0));
    match (best.first(), best.get(1)) {
        (Some(a), Some(b)) if (a.0 - b.0).abs() < 1e-9 && a.1 != b.1 => None,
        (Some(a), _) => Some(a.1.clone()),
        _ => None,
    }
}

/// The units (rooms) a panel feeds: the typical unit panels (`U-101` of typical `U`) its feeder
/// records and the feeder runs' chains (`feeder 3 DP→U-101→U-201`) reach from it, followed down
/// through the panels between.
fn units_under(panel: &str, sp: &BuildingSpec, a: &Arithmetic) -> BTreeSet<String> {
    let typicals: Vec<String> = sp
        .mep
        .panel_schedules
        .iter()
        .filter(|s| s.typical_of.is_some())
        .map(|s| s.panel.trim().to_ascii_uppercase())
        .filter(|t| !t.is_empty())
        .collect();
    let room_of = |tag: &str| -> Option<String> {
        let t = tag.trim().to_ascii_uppercase();
        typicals.iter().find_map(|p| t.strip_prefix(p.as_str()).and_then(|r| r.strip_prefix('-')).filter(|r| !r.is_empty()).map(str::to_string))
    };
    // Edges: from → to.
    let mut down: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in &sp.mep.feeders {
        down.entry(f.from.trim().to_ascii_uppercase()).or_default().insert(f.to.trim().to_ascii_uppercase());
    }
    if !a.feeder_system.is_empty() {
        for r in sp.mep.runs.iter().take(MAX_RUNS).filter(|r| r.system.eq_ignore_ascii_case(&a.feeder_system)) {
            let chain: Vec<String> = r
                .source_sheet
                .split('\u{2192}')
                .map(|p| p.split_whitespace().last().unwrap_or("").trim().to_ascii_uppercase())
                .filter(|p| !p.is_empty())
                .take(MAX_CHAIN + 1)
                .collect();
            for w in chain.windows(2) {
                if let (Some(x), Some(y)) = (w.first(), w.get(1)) {
                    down.entry(x.clone()).or_default().insert(y.clone());
                }
            }
        }
    }
    let mut rooms = BTreeSet::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut todo = vec![(panel.trim().to_ascii_uppercase(), 0usize)];
    while let Some((p, depth)) = todo.pop() {
        if depth > MAX_CHAIN || !seen.insert(p.clone()) {
            continue;
        }
        for to in down.get(&p).into_iter().flatten() {
            if let Some(r) = room_of(to) {
                rooms.insert(r);
            }
            todo.push((to.clone(), depth + 1));
        }
    }
    rooms
}

/// One per-unit line counted for some units only, to compare with the plans.
struct Partial {
    summary: usize,
    line: usize,
    panel: String,
    label: String,
    sheet: String,
    count: u32,
    circuit: (String, String, String),
}

/// The tables' arithmetic over a spec, with the model's devices for the plans' counts.
pub(crate) fn check(d: &Drawing, sp: &BuildingSpec) -> Report {
    let rules = super::rules();
    let a = &rules.arithmetic;
    let mut rep = Report::default();
    for r in feeder_rows(sp, &rules) {
        rep.feeder_rows += 1;
        if r.agrees {
            continue;
        }
        let units = if r.units > 1 { format!(" ({} units)", r.units) } else { String::new() };
        let swapped = r
            .swapped_with
            .as_ref()
            .map(|(n, c)| format!("; it is {c}'s total (row {n}): rows {} and {n} look swapped", r.circuit))
            .unwrap_or_default();
        let sheets: Vec<String> =
            [r.sheet.clone(), r.child_sheet.clone()].into_iter().filter(|s| !s.is_empty()).collect::<BTreeSet<_>>().into_iter().collect();
        rep.mismatches.push(Mismatch {
            kind: "feeder_row",
            panel: r.panel.clone(),
            item: r.circuit.clone(),
            sheets,
            design: r.row,
            corrected: r.expected,
            message: format!(
                "{} circuit {} feeds {}{units}: its row gives {} VA, {}'s own schedule totals {} VA{swapped}",
                r.panel,
                r.circuit,
                r.child,
                va(r.row),
                r.child,
                va(r.expected)
            ),
            correction: format!(
                "the row carries {}'s total, {} VA, as the feeder is sized; the printed {} VA is kept beside it as the design value",
                r.child,
                va(r.expected),
                va(r.row)
            ),
            cite: a.feeder_row_cite.clone(),
            at: r.at,
        });
    }
    // The demand calculations' per-unit lines.
    let mut partial: Vec<Partial> = Vec::new();
    for (si, s) in sp.mep.load_summaries.iter().enumerate().take(MAX_SUMMARIES) {
        let units = summary_units(s, &rules);
        for (li, l) in s.lines.iter().enumerate().take(MAX_LINES) {
            let text = if l.text.trim().is_empty() { l.label.as_str() } else { l.text.as_str() };
            let Some(p) = per_unit(text, a) else { continue };
            let from_total = count_of(&p, a);
            let Some(count) = p.count.or(from_total) else { continue };
            rep.count_lines += 1;
            // Its printed count and its total.
            if p.count.is_some() {
                let want = f64::from(count) * p.each;
                let tol = (a.count_tolerance_pct / 100.0 * want).max(p.rounding);
                if (want - p.total).abs() > tol {
                    rep.mismatches.push(Mismatch {
                        kind: "line_total",
                        panel: s.panel.clone(),
                        item: l.label.clone(),
                        sheets: vec![s.source.clone()].into_iter().filter(|x| !x.is_empty()).collect(),
                        design: p.total,
                        corrected: want,
                        message: format!(
                            "{} ({}): '{}' counts {count} at {} VA each = {} VA, the line prints {} VA",
                            s.title,
                            s.source,
                            l.label,
                            va(p.each),
                            va(want),
                            va(p.total)
                        ),
                        correction: format!(
                            "the line's total taken as {count} x {} VA = {} VA; the printed total kept beside it",
                            va(p.each),
                            va(want)
                        ),
                        cite: a.line_cite.clone(),
                        at: (si, li),
                    });
                    continue;
                }
            }
            // An item counted for some units only: the plans' count on its feeder (a calculation
            // naming no panel is one typical feeder's, any of them: its count is no plans' count).
            if units.is_some_and(|u| u != count)
                && !s.panel.trim().is_empty()
                && let Some(circuit) = item_circuit(&p.item, sp, a)
            {
                partial.push(Partial {
                    summary: si,
                    line: li,
                    panel: s.panel.trim().to_ascii_uppercase(),
                    label: l.label.clone(),
                    sheet: s.source.clone(),
                    count,
                    circuit,
                });
            }
        }
    }
    if partial.is_empty() {
        return rep;
    }
    // The plans: the rooms with a load on the unit panel's circuit for the item.
    let devs = model_devices(d, &rules);
    let rooms_with = |c: &(String, String, String)| -> BTreeSet<String> {
        devs.get(&(c.0.clone(), c.1.clone())).into_iter().flatten().map(|d| d.room.trim().to_ascii_uppercase()).filter(|r| !r.is_empty()).collect()
    };
    let unit_sets: BTreeMap<String, BTreeSet<String>> =
        partial.iter().map(|p| p.panel.clone()).collect::<BTreeSet<_>>().into_iter().map(|p| (p.clone(), units_under(&p, sp, a))).collect();
    // Calculations whose units the plans don't place are counted together, item by item.
    let mut pooled: BTreeMap<(String, String), Vec<&Partial>> = BTreeMap::new();
    for p in &partial {
        let having = rooms_with(&p.circuit);
        if having.is_empty() {
            continue;
        }
        let under = unit_sets.get(&p.panel).filter(|u| !u.is_empty());
        let Some(under) = under else {
            pooled.entry((p.circuit.0.clone(), p.circuit.1.clone())).or_default().push(p);
            continue;
        };
        let plans = having.intersection(under).count() as u32;
        if plans != p.count {
            let s: Option<&LoadSummary> = sp.mep.load_summaries.get(p.summary);
            let title = s.map_or(String::new(), |s| s.title.clone());
            rep.mismatches.push(Mismatch {
                kind: "unit_count",
                panel: p.panel.clone(),
                item: p.label.clone(),
                sheets: vec![p.sheet.clone()].into_iter().filter(|x| !x.is_empty()).collect(),
                design: f64::from(p.count),
                corrected: f64::from(plans),
                message: format!(
                    "{title} ({}): '{}' counts {} unit(s); the plans put a load on unit panel circuit {}-{} ({}) in {plans} of the {} unit(s) {} feeds",
                    p.sheet,
                    p.label,
                    p.count,
                    p.circuit.0,
                    p.circuit.1,
                    p.circuit.2,
                    under.len(),
                    p.panel
                ),
                correction: format!("the calculation's count taken as the plans' {plans}; the printed {} kept beside it", p.count),
                cite: a.count_cite.clone(),
                at: (p.summary, p.line),
            });
        }
    }
    for ((typ, num), group) in pooled {
        let printed: u32 = group.iter().map(|p| p.count).sum();
        let plans = rooms_with(&(typ.clone(), num.clone(), String::new())).len() as u32;
        if plans == printed {
            continue;
        }
        for p in group.iter().take(MAX_SUMMARIES) {
            let title = sp.mep.load_summaries.get(p.summary).map_or(String::new(), |s| s.title.clone());
            rep.mismatches.push(Mismatch {
                kind: "unit_count",
                panel: p.panel.clone(),
                item: p.label.clone(),
                sheets: vec![p.sheet.clone()].into_iter().filter(|x| !x.is_empty()).collect(),
                design: f64::from(p.count),
                corrected: f64::from(plans),
                message: format!(
                    "{title} ({}): '{}' counts {} unit(s); {} calculation(s) together count {printed}, the plans put a load on unit panel circuit {typ}-{num} ({}) in {plans} unit(s) (which units each feeder serves is not on the plans read)",
                    p.sheet,
                    p.label,
                    p.count,
                    group.len(),
                    p.circuit.2
                ),
                correction: format!("left as printed until the units on each feeder are known: the plans count {plans} in all"),
                cite: a.count_cite.clone(),
                at: (p.summary, p.line),
            });
        }
    }
    rep
}

/// The corrections the field makes, written into the spec: a feeder row takes its panel's total
/// (the printed load kept as `design_load_va`), a calculation's line the plans' count; each with
/// its reason and rule. The number of values corrected.
pub(crate) fn correct(sp: &mut BuildingSpec, rep: &Report) -> usize {
    let mut n = 0usize;
    for m in &rep.mismatches {
        let fix = RunCorrection {
            key: if m.kind == "feeder_row" { "load_va".into() } else { "count".into() },
            design: format!("{}", m.design),
            corrected: format!("{}", m.corrected),
            reason: m.message.clone(),
            rule: m.cite.clone(),
        };
        let put = |list: &mut Vec<RunCorrection>| {
            list.retain(|c| c.key != fix.key);
            list.push(fix.clone());
        };
        match m.kind {
            "feeder_row" => {
                let Some(c) = sp.mep.panel_schedules.get_mut(m.at.0).and_then(|s| s.circuits.get_mut(m.at.1)) else { continue };
                if c.design_load_va.is_none() {
                    c.design_load_va = c.load_va;
                }
                c.load_va = Some(m.corrected);
                put(&mut c.corrections);
                n += 1;
            }
            // A pooled count (no feeder's units known) is a question only.
            "unit_count" if m.correction.starts_with("left as printed") => {}
            _ => {
                let Some(l) = sp.mep.load_summaries.get_mut(m.at.0).and_then(|s| s.lines.get_mut(m.at.1)) else { continue };
                put(&mut l.corrections);
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Arithmetic {
        super::super::rules().arithmetic
    }

    #[test]
    fn per_unit_lines_read_their_va_total_count_and_item() {
        let a = rules();
        let p = per_unit("DISHWASHER RECEPTACLE (LOWER LEVEL ONLY X12) (650VA/SUITE): 7.80 KVA", &a).unwrap();
        assert_eq!((p.each, p.total, p.count), (650.0, 7800.0, Some(12)));
        assert_eq!(p.item, vec!["DISHWASHER".to_string()]);
        assert_eq!(count_of(&p, &a), Some(12));
        let p = per_unit("SMALL APPLIANCE RECEPTACLES (3,000VA/SUITE): 150.00 KVA", &a).unwrap();
        assert_eq!((p.each, p.total, p.count), (3000.0, 150_000.0, None));
        assert_eq!(count_of(&p, &a), Some(50));
        let p = per_unit("EXHAUST FAN 50 VA PER UNIT 1,250", &a).unwrap();
        assert_eq!((p.each, p.total), (50.0, 1250.0));
        assert_eq!(count_of(&p, &a), Some(25));
        // Per area, a percent, no total: not a per-unit line.
        for t in ["ROOM LIGHTING (3W/SQFT): 49.04 KVA", "RECEPTACLES 14,940 x 100%;50% = 12,470", "RANGE (2,400VA/SUITE)", ""] {
            assert!(per_unit(t, &a).is_none_or(|p| count_of(&p, &a).is_none()), "{t}");
        }
        // A total that is no whole count of its VA.
        let p = per_unit("RANGE (2,400VA/SUITE): 100.00 KVA", &a).unwrap();
        assert_eq!(count_of(&p, &a), None);
    }

    #[test]
    fn hostile_lines_never_panic() {
        let a = rules();
        let long = "9".repeat(5000);
        for t in [long.as_str(), "X X X 1VA/SUITE", "0VA/UNIT: 0", "1e308VA/SUITE: 1e308", ",,,..VA/SUITE", "X4294967296 (5VA/UNIT): 10"] {
            if let Some(p) = per_unit(t, &a) {
                let _ = count_of(&p, &a);
            }
        }
    }
}
