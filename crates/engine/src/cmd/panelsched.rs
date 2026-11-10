//! Panel schedules as a print set draws them: a `PANELBOARD <tag>` title, a two-line header
//! (`CKT | CIRCUIT DESCRIPTION | BREAKER AMPS P | …`, mirrored on the right for the even
//! circuits), then a row per circuit pair until the subtotals. Each circuit's breaker type codes
//! (`AF`, `GF`, `AF/GF`) are printed outside the table beside its number; the schedule's legend
//! (`GF - GROUND FAULT INTERRUPTER`) says what they are. The guided job keeps them in
//! `mep.panel_schedules`; `bim.props.fill` puts each device's breaker and its type on it.

use std::collections::{BTreeMap, BTreeSet};

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
/// `PANEL 'DP1'`, `PANEL SCHEDULE - P1`, `PANELBOARD P1 SCHEDULE`, `PANEL [L4] SCHEDULE`,
/// `PANEL: L1`, `PANEL NAME: L1`; and the number it is typical of.
pub(crate) fn title_panel(title: &str) -> Option<(String, Option<u32>)> {
    let u = title.trim().to_ascii_uppercase();
    let rest = ["PANELBOARD", "PANEL BOARD", "PANEL"].iter().find_map(|p| u.strip_prefix(p).filter(|r| r.starts_with([' ', ':'])))?;
    let rest = rest.trim_start_matches([' ', ':']);
    // A labelled cell: `PANEL NAME: L1`, `PANEL DESIGNATION: L1`.
    let rest = ["NAME", "DESIGNATION", "TAG", "ID"]
        .iter()
        .find_map(|w| rest.strip_prefix(w).map(str::trim_start).filter(|r| r.starts_with(':')).map(|r| r.trim_start_matches([':', ' '])))
        .unwrap_or(rest);
    let rest = rest.trim().strip_prefix("SCHEDULE").map_or(rest.trim(), |r| r.trim_start_matches([' ', '-', ':', '\u{2013}']));
    let tag: String = rest
        .split_whitespace()
        .next()?
        .trim_matches(['\'', '"', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', ':', '[', ']', '(', ')', '<', '>'])
        .to_string();
    if !panel_tag(&tag) {
        return None;
    }
    // A panel's other tables (its demand calculation, load summary) aren't its schedule.
    if ["DEMAND", "CALCULATION", "SUMMARY", "RISER"].iter().any(|w| rest.contains(w)) {
        return None;
    }
    let typical = rest.split_once("TYPICAL OF").and_then(|(_, r)| r.split_whitespace().next()).and_then(|n| n.parse::<u32>().ok());
    Some((tag, typical))
}

/// A text that can be a panel's tag: short, letters, digits, `-` and `.`, and none of the words
/// that follow PANEL in a heading or a label (`SCHEDULE`, `NAME`, `FED`).
fn panel_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 12
        && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && tag.chars().any(|c| c.is_ascii_alphanumeric())
        && !cadaddy_mepfix::wordrules::words("schedules.panel_title_not_tags").contains(&tag)
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
    /// Where the number column's header ends (the right side's codes are past it).
    num_end: f64,
    amps: Option<f64>,
    poles: Option<f64>,
    /// Its wire size and load (VA) columns.
    wire: Option<f64>,
    load: Option<f64>,
    /// With no load column, its phases' columns (`A | B | C`): a circuit's load is under one.
    phases: Vec<f64>,
    /// Its other number columns (wire size, load): a number nearest one of them is none of the above.
    others: Vec<f64>,
    /// The left side (codes left of its numbers, the description right of them).
    left: bool,
    /// Its breaker type column (`BKR`: `GF`, `AF` in it), and its description's header, when the
    /// header has them (a description printed past the breaker's columns).
    kind: Option<f64>,
    description: Option<f64>,
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
            .filter(|v| !v.value.trim().ends_with(':') && cut_short(&v.value).is_none());
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

/// A character's advance in its text's height (the height a print's text is read with is its
/// capitals'), to place a header's words.
const HEADER_CHAR_WIDTH: f64 = 0.9;
/// A row's texts with each text of only whole numbers (`1 1425`: two columns' cells read as one
/// text) split into its numbers, each where it is printed along the line.
fn numbers_split(ln: &[&TextItem]) -> Vec<TextItem> {
    let mut out = Vec::with_capacity(ln.len());
    for t in ln {
        let parts: Vec<&str> = t.value.split_whitespace().collect();
        if parts.len() < 2 || !parts.iter().all(|w| w.chars().all(|c| c.is_ascii_digit() || c == ',') && w.chars().any(|c| c.is_ascii_digit())) {
            out.push((*t).clone());
            continue;
        }
        let (c, s) = (t.rotation.cos(), t.rotation.sin());
        let mut chars = 0.0;
        for w in parts {
            let along = chars * HEADER_CHAR_WIDTH * t.height;
            out.push(TextItem { value: w.to_string(), at: t.at + cadaddy_geom::Vec2::new(along * c, along * s), ..(*t).clone() });
            chars += w.chars().count() as f64 + 1.0;
        }
    }
    out
}

/// A dot's width, in capitals' (`C...`).
const DOT_WIDTH: f64 = 0.35;
/// The most texts split into their words (hostile files).
const MAX_HEADER_SPLITS: usize = 5_000;

/// The sheet's texts with each header text of several column names (`AMP POLE BKR`, `LTG PWR CO`:
/// a print's columns' names read as one line) split into its words, each where it is printed
/// along the line: the reader finds a column by its one name.
fn header_words(texts: &[TextItem]) -> Vec<TextItem> {
    let names = cadaddy_mepfix::wordrules::words("schedules.panel_header_words");
    let mut out = Vec::with_capacity(texts.len());
    let mut split = 0usize;
    for t in texts {
        let up = t.value.trim().to_ascii_uppercase();
        let parts: Vec<&str> = up.split_whitespace().collect();
        // (A column's name cut short to fit it, `C...`, is one of them too.)
        if split >= MAX_HEADER_SPLITS || parts.len() < 2 || !parts.iter().all(|w| names.contains(w) || cut_short(w).is_some()) {
            out.push(t.clone());
            continue;
        }
        split += 1;
        let (c, s) = (t.rotation.cos(), t.rotation.sin());
        let mut chars = 0.0;
        for w in parts {
            let along = chars * HEADER_CHAR_WIDTH * t.height;
            out.push(TextItem { value: w.to_string(), at: t.at + cadaddy_geom::Vec2::new(along * c, along * s), ..t.clone() });
            // (A dot is narrower than a capital.)
            chars += w.chars().map(|ch| if ch == '.' { DOT_WIDTH } else { 1.0 }).sum::<f64>() + 1.0;
        }
    }
    out
}

/// A schedule whose panel's name its sheet doesn't print as text (`PANEL NAME:` and nothing
/// right of it: the name drawn as linework or an image), and where its label stands.
struct Unnamed {
    schedule: PanelSchedule,
    page: usize,
    at: cadaddy_geom::Vec2,
    height: f64,
}

/// A schedule's header facts that say how its panel's name is known when its sheet doesn't print
/// it: `inferred` (from the panels the set names elsewhere) or `unresolved` (a placeholder), and
/// the basis.
pub(crate) const NAME_MADE: &str = "panel name made";
pub(crate) const NAME_BASIS: &str = "panel name basis";
/// Most texts looked through for the panels a set names (hostile files).
const MAX_MENTION_TEXTS: usize = 400_000;
/// Most panel names taken from a set's notes.
const MAX_MENTIONS: usize = 500;

/// The schedules on one sheet's texts (its PDF page re-read, or the drawing's). A schedule whose
/// panel's name the sheet doesn't print as text is named from the panels its other texts name
/// (see [`read_set`]).
pub(crate) fn read(texts: &[TextItem], sheet: &str) -> Vec<PanelSchedule> {
    let (mut out, unnamed) = read_sheet(texts, sheet, 0);
    if !unnamed.is_empty() {
        let given = name_unnamed(&out, unnamed, &panel_mentions(&[texts]));
        out.extend(given);
    }
    out
}

/// The schedules on a set's sheets (`(sheet, texts)`), each panel once (a panel named twice: the
/// schedule with more breakers read). A schedule whose panel's name is not printed as text (drawn
/// as linework or an image) takes a name the set prints elsewhere: its notes (`FROM PANEL "P1"`),
/// its one-line's labels, the other schedules' feeder rows. A schedule that feeds panels takes
/// the one no schedule feeds; the rest, in their order on the sheets, the others in theirs. Each
/// carries how ([`NAME_MADE`], [`NAME_BASIS`]); with no name left, a placeholder marked
/// `unresolved`.
pub(crate) fn read_set(pages: &[(&str, &[TextItem])]) -> Vec<PanelSchedule> {
    let mut out: Vec<PanelSchedule> = Vec::new();
    let mut unnamed: Vec<Unnamed> = Vec::new();
    for (i, (sheet, texts)) in pages.iter().enumerate() {
        let (named, u) = read_sheet(texts, sheet, i);
        for p in named {
            keep_better(&mut out, p);
        }
        unnamed.extend(u);
    }
    if unnamed.is_empty() {
        return out;
    }
    let texts: Vec<&[TextItem]> = pages.iter().map(|(_, t)| *t).collect();
    for p in name_unnamed(&out, unnamed, &panel_mentions(&texts)) {
        keep_better(&mut out, p);
    }
    out
}

/// `p` among `out`, a panel named twice keeping the schedule with more breakers read.
fn keep_better(out: &mut Vec<PanelSchedule>, p: PanelSchedule) {
    let read = |x: &PanelSchedule| x.circuits.iter().filter(|c| !c.breaker.is_empty()).count();
    match out.iter_mut().find(|o| o.panel.eq_ignore_ascii_case(&p.panel)) {
        Some(o) if read(&p) > read(o) => *o = p,
        Some(_) => {}
        None => out.push(p),
    }
}

/// One sheet's schedules: those its titles name, and those whose name it doesn't print.
fn read_sheet(texts: &[TextItem], sheet: &str, page: usize) -> (Vec<PanelSchedule>, Vec<Unnamed>) {
    let words = header_words(texts);
    let flat: Vec<&TextItem> =
        words.iter().filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.x.is_finite() && t.at.y.is_finite()).collect();
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
    // A title split in two: a `PANEL:` / `PANEL NAME:` label and the tag right of it. A label
    // with no tag there (nothing, another label, an empty `""`) titles a schedule whose name the
    // sheet doesn't print as text.
    let mut joined: Vec<TextItem> = Vec::new();
    let mut nameless: Vec<&TextItem> = Vec::new();
    for l in flat.iter().copied().filter(|l| is_title_label(&l.value)).take(MAX_SCHEDULES) {
        let tagged = flat
            .iter()
            .filter(|t| (t.at.y - l.at.y).abs() <= 0.6 * l.height && t.at.x > l.at.x + 0.5 * l.height && t.at.x < l.at.x + 30.0 * l.height)
            .min_by(|a, b| a.at.x.total_cmp(&b.at.x))
            // Another label (`LOCATION:`, `VOLTAGE:`) right of it is the next fact's, not its tag.
            .filter(|t| !t.value.trim().ends_with(':'))
            .map(|t| TextItem { value: format!("{} {}", l.value.trim(), t.value.trim()), ..l.clone() })
            .filter(|j| title_panel(&j.value).is_some());
        match tagged {
            Some(j) => joined.push(j),
            None => nameless.push(l),
        }
    }
    let mut out = Vec::new();
    for title in flat.iter().copied().chain(joined.iter()) {
        if out.len() >= MAX_SCHEDULES {
            break;
        }
        let Some((panel, typical_of)) = title_panel(&title.value) else { continue };
        out.extend(read_one(&flat, title, &panel, typical_of, &legend, sheet));
    }
    let mut unnamed: Vec<Unnamed> = Vec::new();
    for l in nameless {
        if out.len() + unnamed.len() >= MAX_SCHEDULES {
            break;
        }
        let Some(schedule) = read_one(&flat, l, "", None, &legend, sheet) else { continue };
        // A table a named title already read (its label one more line of its header) is that one.
        if out.iter().any(|o| o.circuits == schedule.circuits) || unnamed.iter().any(|u| u.schedule.circuits == schedule.circuits) {
            continue;
        }
        unnamed.push(Unnamed { schedule, page, at: l.at, height: l.height });
    }
    (out, unnamed)
}

/// The schedule under `title`: its circuits numbered in a CKT column on each side, or down the
/// middle (`1 A 2`): the reading with more circuits that have a description or a breaker.
fn read_one(
    flat: &[&TextItem],
    title: &TextItem,
    panel: &str,
    typical_of: Option<u32>,
    legend: &BTreeMap<String, String>,
    sheet: &str,
) -> Option<PanelSchedule> {
    let real = |p: &Option<PanelSchedule>| {
        p.as_ref().map_or(0, |p| p.circuits.iter().filter(|c| !c.description.is_empty() || !c.breaker.is_empty()).count())
    };
    let by_ckt = read_ckt(flat, title, panel, typical_of, legend, sheet);
    let centred = read_centred(flat, title, panel, typical_of, legend, sheet);
    let inward = read_inward(flat, title, panel, typical_of, legend, sheet);
    let best = if real(&centred) > real(&by_ckt) { centred } else { by_ckt };
    if real(&inward) > real(&best) { inward } else { best }
}

const QUOTES: [char; 6] = ['\'', '"', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}'];

/// The panels a set's texts name, each once, in the order they come: a tag after `PANEL` or
/// `PANELBOARD` (`FROM PANEL "P1"`, `PANEL LP-2`) that is quoted, has a digit, or is printed as a
/// text of its own somewhere in the set (a one-line's label).
fn panel_mentions(pages: &[&[TextItem]]) -> Vec<String> {
    let all = || pages.iter().flat_map(|p| p.iter()).take(MAX_MENTION_TEXTS);
    let lone: BTreeSet<String> = all().map(|t| t.value.trim().to_ascii_uppercase()).filter(|v| panel_tag(v)).collect();
    let mut out: Vec<String> = Vec::new();
    for t in all() {
        if out.len() >= MAX_MENTIONS {
            break;
        }
        let up = t.value.to_ascii_uppercase();
        if !up.contains("PANEL") {
            continue;
        }
        let words: Vec<&str> = up.split_whitespace().collect();
        for (i, w) in words.iter().enumerate() {
            let head = w.trim_end_matches([':', ',']);
            if head != "PANEL" && head != "PANELBOARD" {
                continue;
            }
            let at = if head == "PANEL" && words.get(i + 1) == Some(&"BOARD") { i + 2 } else { i + 1 };
            let Some(next) = words.get(at) else { continue };
            let quoted = next.starts_with(QUOTES);
            let tag = next.trim_matches(|c: char| QUOTES.contains(&c) || ",.;:()[]".contains(c));
            // (`CALCULATED PANEL AMPS:`, `PANEL TYPE:`: a label, not a panel)
            let label = !quoted && next.ends_with(':');
            let named = !label && panel_tag(tag) && (quoted || tag.chars().any(|c| c.is_ascii_digit()) || lone.contains(tag));
            if named && !out.iter().any(|o| o == tag) {
                out.push(tag.to_string());
            }
        }
    }
    out
}

/// A tag's order among its kind: `P2` before `P10`.
fn natural(tag: &str) -> (String, u64, String) {
    let lead: String = tag.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let rest = tag.get(lead.len()..).unwrap_or("");
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let tail = rest.get(digits.len()..).unwrap_or("").to_string();
    (lead, digits.parse::<u64>().unwrap_or(u64::MAX), tail)
}

/// The panels a schedule's circuits feed among `names`: a circuit whose description names one.
fn feeds(p: &PanelSchedule, names: &BTreeSet<String>) -> BTreeSet<String> {
    p.circuits
        .iter()
        .flat_map(|c| {
            c.description
                .to_ascii_uppercase()
                .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '.')
                .filter(|w| names.contains(*w))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The unnamed schedules named ([`read_set`]): `named` the schedules their titles name,
/// `mentions` the panels the set names.
fn name_unnamed(named: &[PanelSchedule], mut unnamed: Vec<Unnamed>, mentions: &[String]) -> Vec<PanelSchedule> {
    // In their order on the sheets: sheet by sheet, row of tables by row, left to right.
    let row = |u: &Unnamed| -((u.at.y / (4.0 * u.height)).round() as i64);
    unnamed.sort_by(|a, b| a.page.cmp(&b.page).then(row(a).cmp(&row(b))).then(a.at.x.total_cmp(&b.at.x)));
    let taken: BTreeSet<String> = named.iter().map(|p| p.panel.to_ascii_uppercase()).collect();
    let names: BTreeSet<String> = mentions.iter().cloned().collect();
    let all_feed: BTreeSet<String> = named.iter().chain(unnamed.iter().map(|u| &u.schedule)).flat_map(|p| feeds(p, &names)).collect();
    let mut free: Vec<String> = mentions.iter().filter(|m| !taken.contains(*m)).cloned().collect();
    free.sort_by_key(|m| natural(m));
    let list = |v: &[String]| v.join(", ");
    let mut given: Vec<Option<(String, String, String)>> = unnamed.iter().map(|_| None).collect();
    // A schedule that feeds panels: the panel named that no schedule feeds (the one upstream).
    for (u, g) in unnamed.iter().zip(given.iter_mut()) {
        let fed: Vec<String> = feeds(&u.schedule, &names).into_iter().collect();
        if fed.is_empty() {
            continue;
        }
        let Some(at) = free.iter().position(|c| !all_feed.contains(c) && !fed.contains(c)) else { continue };
        let c = free.remove(at);
        let basis = format!(
            "its name is not printed as text; its circuits feed {}, and {c} is the panel the set names that no schedule read feeds",
            list(&fed)
        );
        *g = Some((c, "inferred".into(), basis));
    }
    // The rest in their order on the sheets: the panels fed by a schedule first, in tag order.
    free.sort_by_key(|c| !all_feed.contains(c));
    let pool = free.clone();
    let mut k = 0usize;
    for (u, g) in unnamed.iter().zip(given.iter_mut()).filter(|(_, g)| g.is_none()) {
        let fed = feeds(&u.schedule, &names);
        let Some(at) = free.iter().position(|c| !fed.contains(c)) else { continue };
        let c = free.remove(at);
        k += 1;
        let basis = format!(
            "its name is not printed as text; the panels the set names that no schedule title prints ({}) given to the unnamed schedules in their order on the sheets, this the {}; another order is the alternative: check the panel's directory",
            list(&pool),
            ordinal(k)
        );
        *g = Some((c, "inferred".into(), basis));
    }
    let mut n = 0usize;
    unnamed
        .into_iter()
        .zip(given)
        .map(|(u, g)| {
            let (panel, made, basis) = g.unwrap_or_else(|| {
                n += 1;
                (
                    format!("UNNAMED-{n}"),
                    "unresolved".into(),
                    format!(
                        "its name is not printed as text on {} and the set names no panel left for it: the engineer names it (a question)",
                        u.schedule.source
                    ),
                )
            });
            let mut p = u.schedule;
            p.panel = panel;
            p.header.insert(NAME_MADE.into(), made);
            p.header.insert(NAME_BASIS.into(), basis);
            p
        })
        .collect()
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, x) if x != 11 => "st",
        (2, x) if x != 12 => "nd",
        (3, x) if x != 13 => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A schedule whose circuits are numbered in a CKT column on each side (or one), under `title`.
fn read_ckt(
    flat: &[&TextItem],
    title: &TextItem,
    panel: &str,
    typical_of: Option<u32>,
    legend: &BTreeMap<String, String>,
    sheet: &str,
) -> Option<PanelSchedule> {
    let h = title.height;
    // The header: the nearest line under the title with a CKT column (two: the right side's).
    let headings = cadaddy_mepfix::wordrules::words("schedules.panel_circuit_headings");
    let ckts: Vec<&&TextItem> = flat
        .iter()
        .filter(|t| {
            let u = t.value.trim().to_ascii_uppercase();
            (headings.contains(&u.as_str()) || cut_short_circuit_heading(t, headings, flat))
                && t.at.y < title.at.y
                && t.at.y > title.at.y - HEADER_REACH * h
                && (t.at.x - title.at.x).abs() < 80.0 * h
        })
        .collect();
    let top = ckts.iter().map(|t| t.at.y).max_by(f64::total_cmp)?;
    let row_h = ckts.iter().map(|t| t.height).fold(h, f64::min);
    // A title stands above its table in larger text (a cell naming a panel is a feeder row),
    // unless it is labelled as one (`PANEL: L1`, `PANEL L1 SCHEDULE`).
    if h < TITLE_OVER_CELLS * ckts.iter().map(|t| t.height).fold(f64::INFINITY, f64::min) && !labelled(&title.value) {
        return None;
    }
    let mut line: Vec<&&TextItem> = ckts.iter().copied().filter(|t| (t.at.y - top).abs() <= 0.6 * row_h).collect();
    line.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
    // Which way each CKT column faces: its description's header beside it (right of a left
    // side's numbers, left of a right side's). Tables side by side share the header line; this
    // title's table starts at its last left-facing CKT before the title, and ends at the next
    // right-facing one before another table starts.
    // (Within the reach the CKT columns are looked for in: a description beyond it is the next
    // table's, whose CKT column is out of reach, and would turn this one's right side to face right.)
    let descs: Vec<f64> = flat
        .iter()
        .filter(|t| {
            t.value.to_ascii_uppercase().contains("DESCRIPTION")
                && t.at.y <= top + 0.6 * row_h
                && t.at.y > top - 2.6 * row_h
                && (t.at.x - title.at.x).abs() < 80.0 * h
        })
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
        let (Some(l), Some(r)) = (line.first().copied(), line.last().copied()) else { return None };
        (l, r)
    } else {
        let lefts: Vec<usize> = (0..line.len()).filter(|i| line.get(*i).is_some_and(|t| faces_right(t.at.x) == Some(true))).collect();
        let start = lefts.iter().copied().rfind(|i| line.get(*i).is_some_and(|t| t.at.x <= title.at.x + 2.0 * h)).or_else(|| lefts.first().copied());
        let start = start?;
        let l = line.get(start).copied()?;
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
    // A header of one line: its first circuit's number close under it (no second line between).
    let first_row = flat
        .iter()
        .filter(|t| t.at.y < top - 0.5 * row_h && t.at.y > top - 2.6 * row_h && number(&t.value).is_some() && (t.at.x - l.at.x).abs() < 2.0 * row_h)
        .map(|t| t.at.y)
        .max_by(f64::total_cmp);
    let head: Vec<&&TextItem> = flat
        .iter()
        .filter(|t| {
            t.at.y <= top + 0.6 * row_h
                && t.at.y > top - 2.6 * row_h
                && first_row.is_none_or(|y| t.at.y > y + 0.6 * row_h)
                && t.at.x > l.at.x - row_h
                && t.at.x < head_x1
        })
        .collect();
    let named = |name: &str| -> Vec<f64> { head.iter().filter(|t| t.value.trim().eq_ignore_ascii_case(name)).map(|t| t.at.x).collect() };
    let hash = named("#");
    let near_num =
        |x: f64| hash.iter().copied().filter(|h| (h - x).abs() < 3.0 * row_h).min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs())).unwrap_or(x);
    // The breaker's amps: its `AMPS` / `TRIP` heading; a schedule that prints none heads the
    // column `BRKR` / `BREAKER` (its poles then under a `#` of their own).
    let rated = [named("AMPS"), named("AMP"), named("TRIP")].concat();
    let amps = if rated.is_empty() { [named("BRKR"), named("BREAKER"), named("A")].concat() } else { [rated, named("A")].concat() };
    // The poles: `P` / `POLES`; with none, a `#` that is not over the circuit numbers (`BRKR | #`).
    let pole_heads = [named("P"), named("POLE"), named("POLES")].concat();
    let poles = if pole_heads.is_empty() {
        hash.iter().copied().filter(|x| (x - l.at.x).abs() >= 3.0 * row_h && (x - r.at.x).abs() >= 3.0 * row_h).collect()
    } else {
        pole_heads
    };
    // The breaker's type column (`BKR` over `GF`), and the description's.
    let kinds = named("BKR");
    let desc_heads: Vec<f64> = head.iter().filter(|t| t.value.to_ascii_uppercase().contains("DESCRIPTION")).map(|t| t.at.x).collect();
    let lnum = near_num(l.at.x);
    let rnum = near_num(r.at.x);
    // The right side's number column ends where its header does (its numbers are printed
    // under the header, not at its left edge): a `#` one character, else the CKT text.
    let width = |v: &str| v.chars().map(|ch| if ch == '.' { DOT_WIDTH } else { 1.0 }).sum::<f64>().max(1.0);
    let rnum_end = rnum + if (rnum - r.at.x).abs() > f64::EPSILON { 1.0 } else { width(r.value.trim()) } * HEADER_CHAR_WIDTH * row_h;
    // (and no further than the next column's header: a notes column past it, `NO...`)
    let next_head = head.iter().map(|t| t.at.x).filter(|x| *x > rnum + 0.5 * row_h).min_by(f64::total_cmp);
    let rnum_end = next_head.map_or(rnum_end, |n| rnum_end.min(n - 0.8 * row_h).max(rnum));
    let first_after = |xs: &[f64], x: f64, limit: f64| xs.iter().copied().filter(|v| *v > x && *v < limit).min_by(f64::total_cmp);
    let last_before = |xs: &[f64], x: f64, limit: f64| xs.iter().copied().filter(|v| *v < x && *v > limit).max_by(f64::total_cmp);
    let mid = if two { (lnum + rnum) / 2.0 } else { f64::INFINITY };
    let others = [named("SIZE"), named("(VA)"), named("LOAD"), named("*")].concat();
    let half = |left: bool| -> Vec<f64> { others.iter().copied().filter(|x| if left { *x < mid } else { *x > mid }).collect() };
    // The wire's size: its `SIZE` heading, else a `WIRE` one (printed before the breaker or after).
    let sizes = Some(named("SIZE")).filter(|v| !v.is_empty()).unwrap_or_else(|| named("WIRE"));
    let loads = [named("(VA)"), named("LOAD")].concat();
    let (lamps, lpoles) = (first_after(&amps, lnum, mid), first_after(&poles, lnum, mid));
    let lwire = first_after(&sizes, lpoles.or(lamps).unwrap_or(lnum), mid).or_else(|| first_after(&sizes, lnum, lamps.unwrap_or(mid)));
    let lload = first_after(&loads, lwire.unwrap_or(lnum), mid);
    // With no load column, each circuit's load is printed under its phase's (`A | B | C`, each
    // side its own, past its breaker's columns).
    let phase_cols = |left: bool, from: Option<f64>| -> Vec<f64> {
        let Some(from) = from else { return Vec::new() };
        head.iter()
            .filter(|t| PHASE_HEADS.contains(&t.value.trim().to_ascii_uppercase().as_str()))
            .map(|t| t.at.x)
            .filter(|x| if left { *x > from + 0.5 * row_h && *x < mid } else { *x < from - 0.5 * row_h && *x > mid })
            .collect()
    };
    let mut sides = vec![Side {
        num: lnum,
        num_end: lnum,
        amps: lamps,
        poles: lpoles,
        wire: lwire,
        load: lload,
        phases: if lload.is_none() { phase_cols(true, lpoles.or(lamps)) } else { Vec::new() },
        others: half(true),
        left: true,
        kind: first_after(&kinds, lnum, mid),
        description: first_after(&desc_heads, lnum, mid),
    }];
    if two {
        let rwire = last_before(&sizes, rnum, mid);
        let (ramps, rpoles) = (last_before(&amps, rnum, mid), last_before(&poles, rnum, mid));
        let rload = last_before(&loads, rwire.unwrap_or(rnum), mid);
        sides.push(Side {
            num: rnum,
            num_end: rnum_end,
            amps: ramps,
            poles: rpoles,
            wire: rwire,
            load: rload,
            phases: if rload.is_none() { phase_cols(false, rpoles.or(ramps)) } else { Vec::new() },
            others: half(false),
            left: false,
            kind: last_before(&kinds, rnum, mid),
            description: last_before(&desc_heads, rnum, mid),
        });
    }
    let head_bottom = head.iter().map(|t| t.at.y).fold(top, f64::min);
    let (x0, x1) = (lnum - 8.0 * row_h, if two { rnum + 8.0 * row_h } else { lnum + 120.0 * row_h });
    let (lines, last_y, step) = rows_below(flat, head_bottom, x0, x1, row_h);
    let mut side_rows: Vec<Vec<Cells>> = sides.iter().map(|_| Vec::new()).collect();
    for ln in &lines {
        let mut cells: Vec<Cells> = sides.iter().map(|_| Cells::default()).collect();
        let ln = numbers_split(ln);
        for t in &ln {
            let x = t.at.x;
            let v = t.value.trim();
            // The side it is on: left of the middle the left side.
            let si = usize::from(two && x > mid);
            let (Some(side), Some(c)) = (sides.get(si), cells.get_mut(si)) else { continue };
            let outside = if side.left { x < side.num - 0.8 * row_h } else { x > side.num_end + 0.8 * row_h };
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
                let other =
                    side.others.iter().copied().filter(|o| !known.iter().flatten().chain(side.phases.iter()).any(|k| (k - o).abs() < 1.5 * row_h));
                let best = [(0u8, Some(side.num)), (1, side.amps), (2, side.poles), (3, side.wire), (4, side.load)]
                    .into_iter()
                    .chain(side.phases.iter().map(|p| (4u8, Some(*p))))
                    .chain(other.map(|o| (9u8, Some(o))))
                    .filter_map(|(k, cx)| cx.map(|cx| ((x - cx).abs(), k)))
                    .filter(|(d, _)| *d <= COLUMN_REACH * row_h)
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                match best {
                    Some((_, 0)) if c.number.is_empty() && number(v).is_some() => c.number = v.to_string(),
                    Some((_, 1)) if c.amps.is_empty() && number(v.trim_end_matches('A')).is_some() => c.amps = v.trim_end_matches('A').to_string(),
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
            } else if let Some(cs) = codes(v).filter(|_| side.kind.is_some_and(|k| (x - k).abs() <= 2.0 * row_h)) {
                // A type code in the breaker's own column.
                c.codes.extend(cs);
            } else if letters(v) {
                // Under the description's header (nearer it than the circuit's number).
                let under_desc = side.description.is_some_and(|d| (x - d).abs() <= COLUMN_REACH * row_h && (x - d).abs() < (x - side.num).abs());
                let between = under_desc
                    || match (side.left, side.amps) {
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
    let mut circuits = circuits_of(&side_rows, sheet);
    if circuits.is_empty() {
        return None;
    }
    circuits.sort_by_key(|c| c.number.parse::<u32>().unwrap_or(u32::MAX));
    let header = header_facts(flat, top + 0.6 * row_h, title.at.y, l.at.x - 2.0 * row_h, head_x1 + 8.0 * row_h, row_h);
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
    let totals = totals_of(flat, last_y + 0.6 * row_h, end, x0, x1, row_h, &phases);
    Some(PanelSchedule { panel: panel.to_string(), typical_of, source: sheet.to_string(), circuits, legend: legend.clone(), header, totals })
}

/// A cell's text cut short to fit its column (`C...`, `Phas…`): the letters printed before the dots.
fn cut_short(v: &str) -> Option<&str> {
    let v = v.trim();
    let stem = v.strip_suffix("...").or_else(|| v.strip_suffix('\u{2026}'))?.trim_end_matches('.').trim();
    (!stem.is_empty() && stem.chars().all(|c| c.is_ascii_alphabetic())).then_some(stem)
}

/// Most circuit numbers looked for under a heading cut short, and how far below it, in its heights.
const CUT_HEADING_ROWS: usize = 3;
const CUT_HEADING_REACH: f64 = 8.0;

/// A heading cut short to fit its column (`C...`) that is the circuit numbers': its letters start
/// one of `headings`, and the cells under it are circuit numbers (one side's: `1, 3, 5` or `2, 4,
/// 6`), not a conduit's sizes or a count.
fn cut_short_circuit_heading(t: &TextItem, headings: &[&str], flat: &[&TextItem]) -> bool {
    let Some(stem) = cut_short(&t.value) else { return false };
    let stem = stem.to_ascii_uppercase();
    if !headings.iter().any(|h| h.starts_with(stem.as_str())) {
        return false;
    }
    let h = t.height;
    let mut under: Vec<(f64, u32)> = flat
        .iter()
        .filter(|c| (c.at.x - t.at.x).abs() < 1.5 * h && c.at.y < t.at.y - 0.5 * h && c.at.y > t.at.y - CUT_HEADING_REACH * h)
        .filter_map(|c| number(&c.value).and_then(|n| n.parse::<u32>().ok()).map(|n| (c.at.y, n)))
        .collect();
    under.sort_by(|a, b| b.0.total_cmp(&a.0));
    let firsts: Vec<u32> = under.iter().take(CUT_HEADING_ROWS).map(|(_, n)| *n).collect();
    firsts.len() >= 2 && firsts.first().is_some_and(|n| *n <= 2) && firsts.windows(2).all(|w| matches!(w, [a, b] if *b == a + 2))
}

/// The schedule under `title` whose two circuit-number columns face inward around its phases'
/// loads (`DESCRIPTION | AMPS | POLE | CKT | A | B | C | CKT | POLE | AMPS | DESCRIPTION`): each
/// side's breaker, poles and description outside its numbers, each phase's cell split between
/// the two circuits of its row (the left one's load in its left half).
fn read_inward(
    flat: &[&TextItem],
    title: &TextItem,
    panel: &str,
    typical_of: Option<u32>,
    legend: &BTreeMap<String, String>,
    sheet: &str,
) -> Option<PanelSchedule> {
    let h = title.height;
    let headings = cadaddy_mepfix::wordrules::words("schedules.panel_circuit_headings");
    let ckts: Vec<&&TextItem> = flat
        .iter()
        .filter(|t| {
            headings.contains(&t.value.trim().to_ascii_uppercase().as_str())
                && t.at.y < title.at.y
                && t.at.y > title.at.y - HEADER_REACH * h
                && (t.at.x - title.at.x).abs() < 80.0 * h
        })
        .collect();
    let top = ckts.iter().map(|t| t.at.y).max_by(f64::total_cmp)?;
    let row_h = ckts.iter().map(|t| t.height).fold(h, f64::min);
    if h < TITLE_OVER_CELLS * row_h && !labelled(&title.value) {
        return None;
    }
    let mut line: Vec<f64> = ckts.iter().filter(|t| (t.at.y - top).abs() <= 0.6 * row_h).map(|t| t.at.x).collect();
    line.sort_by(f64::total_cmp);
    let descs: Vec<f64> = flat
        .iter()
        .filter(|t| t.value.to_ascii_uppercase().contains("DESCRIPTION") && t.at.y <= top + 0.6 * row_h && t.at.y > top - 2.6 * row_h)
        .map(|t| t.at.x)
        .collect();
    // The pair of CKT columns with no description between them, one left of the first and one
    // right of the second: the one whose left description is nearest the title.
    let (c1, c2, d1) = line
        .windows(2)
        .filter_map(|w| match w {
            [a, b] if *b - *a > 4.0 * row_h && !descs.iter().any(|d| d > a && d < b) => {
                let left = descs.iter().copied().filter(|d| d < a).max_by(f64::total_cmp)?;
                descs.iter().any(|d| d > b).then_some((*a, *b, left))
            }
            _ => None,
        })
        .min_by(|x, y| (x.2 - title.at.x).abs().total_cmp(&(y.2 - title.at.x).abs()))?;
    if (d1 - title.at.x).abs() > 20.0 * h {
        return None;
    }
    let first_row = flat
        .iter()
        .filter(|t| t.at.y < top - 0.5 * row_h && t.at.y > top - 2.6 * row_h && number(&t.value).is_some() && (t.at.x - c1).abs() < 2.0 * row_h)
        .map(|t| t.at.y)
        .max_by(f64::total_cmp);
    let x0 = d1.min(title.at.x) - 4.0 * row_h;
    let x1 = c2 + (c1 - x0) + 2.0 * row_h;
    let head: Vec<&&TextItem> = flat
        .iter()
        .filter(|t| {
            t.at.y <= top + 0.6 * row_h
                && t.at.y > top - 2.6 * row_h
                && first_row.is_none_or(|y| t.at.y > y + 0.6 * row_h)
                && t.at.x > x0
                && t.at.x < x1
        })
        .collect();
    let named = |names: &[&str]| -> Vec<f64> {
        head.iter().filter(|t| names.iter().any(|n| t.value.trim().eq_ignore_ascii_case(n))).map(|t| t.at.x).collect()
    };
    let poles = named(&["P", "POLE", "POLES"]);
    // The breaker's amps: `AMPS` / `TRIP`; else a `BKR` / `BRKR` not over the poles (`BKR` over
    // `SIZE`), else `SIZE`.
    let rated = named(&["AMPS", "AMP", "TRIP"]);
    let not_poles = |x: &f64| poles.iter().all(|p| (p - x).abs() > 1.5 * row_h);
    let amps: Vec<f64> = if !rated.is_empty() {
        rated
    } else {
        let bkr: Vec<f64> = named(&["BKR", "BRKR", "BREAKER"]).into_iter().filter(not_poles).collect();
        if bkr.is_empty() { named(&["SIZE"]) } else { bkr }
    };
    let wires = named(&["WIRE"]);
    let phases: Vec<f64> = head
        .iter()
        .filter(|t| PHASE_HEADS.contains(&t.value.trim().to_ascii_uppercase().as_str()))
        .map(|t| t.at.x)
        .filter(|x| *x > c1 && *x < c2)
        .collect();
    let outside = |xs: &[f64], left: bool| -> Option<f64> {
        if left {
            xs.iter().copied().filter(|x| *x > d1 && *x < c1 - 0.5 * row_h).max_by(f64::total_cmp)
        } else {
            xs.iter().copied().filter(|x| *x > c2 + 0.5 * row_h && *x < x1).min_by(f64::total_cmp)
        }
    };
    let cols =
        |left: bool| [(0u8, Some(if left { c1 } else { c2 })), (1, outside(&amps, left)), (2, outside(&poles, left)), (3, outside(&wires, left))];
    let sides = [cols(true), cols(false)];
    let head_bottom = head.iter().map(|t| t.at.y).fold(top, f64::min);
    let (lines, last_y, step) = rows_below(flat, head_bottom, x0, x1, row_h);
    let (mut left_rows, mut right_rows): (Vec<Cells>, Vec<Cells>) = (Vec::new(), Vec::new());
    for ln in &lines {
        let (mut lc, mut rc) = (Cells::default(), Cells::default());
        for t in &numbers_split(ln) {
            let (x, v) = (t.at.x, t.value.trim());
            let centre = x + 0.3 * v.chars().count() as f64 * t.height;
            // Between the numbers: a phase's load, the left circuit's in its cell's left half.
            if x > c1 + 1.5 * row_h && x < c2 - 0.5 * row_h {
                let near = phases.iter().copied().min_by(|a, b| (a - centre).abs().total_cmp(&(b - centre).abs()));
                if let (Some(p), Some(n)) = (near, amount(v)) {
                    let c = if centre < p + 0.3 * row_h { &mut lc } else { &mut rc };
                    if c.load.is_empty() {
                        c.load = format!("{n}");
                    }
                }
                continue;
            }
            let si = usize::from(x >= c2 - 0.5 * row_h);
            let Some(cols) = sides.get(si) else { continue };
            let c = if si == 0 { &mut lc } else { &mut rc };
            let amps_v = v.trim_end_matches('A').trim();
            if number(amps_v).is_some() || numberish(v) {
                let best = cols
                    .iter()
                    .filter_map(|(k, cx)| cx.map(|cx| ((x - cx).abs(), *k)))
                    .filter(|(d, _)| *d <= COLUMN_REACH * row_h)
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                match best {
                    Some((_, 0)) if c.number.is_empty() && number(v).is_some() => c.number = v.to_string(),
                    Some((_, 1)) if c.amps.is_empty() && number(amps_v).is_some() => c.amps = amps_v.to_string(),
                    Some((_, 2)) if c.poles.is_empty() && number(v).is_some() => c.poles = v.to_string(),
                    Some((_, 3)) if c.wire.is_empty() => c.wire = v.trim_start_matches('#').to_string(),
                    _ => {}
                }
            } else if letters(v) {
                // Its description: outside its breaker's columns.
                let edge = cols.iter().filter(|(k, _)| *k > 0).filter_map(|(_, cx)| *cx);
                let past = if si == 0 { edge.fold(c1, f64::min) - 1.0 * row_h } else { edge.fold(c2, f64::max) + 1.0 * row_h };
                if (si == 0 && x < past) || (si == 1 && x > past) {
                    c.description.push(v.to_string());
                } else if let Some(cs) = codes(v) {
                    c.codes.extend(cs);
                }
            }
        }
        left_rows.push(lc);
        right_rows.push(rc);
    }
    let mut circuits = circuits_of(&[left_rows, right_rows], sheet);
    if circuits.is_empty() {
        return None;
    }
    circuits.sort_by_key(|c| c.number.parse::<u32>().unwrap_or(u32::MAX));
    let header = header_facts(flat, top + 0.6 * row_h, title.at.y + 0.6 * row_h, x0, x1, row_h);
    let end = step.map_or(last_y - 4.0 * row_h, |s| last_y - TOTALS_ROWS * s);
    let totals = totals_of(flat, last_y + 0.6 * row_h, end, x0, x1, row_h, &phases);
    Some(PanelSchedule { panel: panel.to_string(), typical_of, source: sheet.to_string(), circuits, legend: legend.clone(), header, totals })
}

/// A text that only labels a schedule's panel, its tag in the next cell: `PANEL:`, `PANEL NAME:`.
fn is_title_label(v: &str) -> bool {
    let u = v.trim().to_ascii_uppercase();
    let u = u.trim_end_matches(':').trim();
    ["PANEL", "PANELBOARD", "PANEL BOARD", "PANEL NAME", "PANEL DESIGNATION", "PANEL TAG", "PANEL ID"].contains(&u) && v.trim().ends_with(':')
}

/// A title that says it is a schedule's (`PANEL: L1`, `PANEL L1 SCHEDULE`), not a cell naming a
/// panel it feeds.
fn labelled(title: &str) -> bool {
    let u = title.to_ascii_uppercase();
    u.contains(':') || u.contains("SCHEDULE")
}

/// A breaker's trip column, its poles column, and the columns that hold no load (sizes, notes,
/// the circuit numbers): the headers a schedule numbered down the middle prints.
const AMP_HEADS: [&str; 7] = ["AMPS", "AMP", "TRIP", "BKR", "BREAKER", "BRKR", "TRIP (A)"];
const POLE_HEADS: [&str; 5] = ["P", "POLE", "POLES", "#P", "PLS"];
/// The header over the circuit numbers of a schedule numbered down the middle.
const CENTRE_HEADS: [&str; 8] = ["CKT", "CKT.", "CIR", "CCT", "NO.", "#", "PHASE", "PH"];
const NOT_LOAD: [&str; 18] = [
    "WIRE", "SIZE", "CONDUIT", "COND", "GND", "GROUND", "AWG", "NOTE", "NOTES", "REMARKS", "ROOM", "LOCATION", "TYPE", "PHASE", "PH", "CKT", "CIR",
    "NO.",
];

/// A circuit pair as a schedule numbered down the middle prints it: `1 A 2`, `13 ØB 14`, `5 6`:
/// an odd number and the even one after it, the phase between.
fn centre_pair(v: &str) -> Option<(u32, u32)> {
    let toks: Vec<&str> = v.split(|c: char| c.is_whitespace() || c == '-' || c == '|').filter(|t| !t.is_empty()).collect();
    let phase = |t: &str| {
        let p: String = t.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase();
        let p = p.strip_prefix("PH").unwrap_or(&p).to_string();
        ["A", "B", "C"].contains(&p.as_str())
    };
    let n = |t: &str| number(t).and_then(|n| n.parse::<u32>().ok());
    let (a, b) = match toks.as_slice() {
        [a, b] => (n(a)?, n(b)?),
        [a, p, b] if phase(p) => (n(a)?, n(b)?),
        _ => return None,
    };
    (a % 2 == 1 && b == a + 1).then_some((a, b))
}

/// A schedule whose circuits are numbered down its middle, the odd circuit's cells left of the
/// number and the even one's right of it (`DESCRIPTION | P | AMP | loads… | 1 A 2 | loads… | P |
/// AMP | DESCRIPTION`), each side's load in one column or one per kind of load (`LIGHT`, `RECEP`,
/// `MOTOR`…, summed); its facts (`VOLTAGE:`, `MCB/MLO:`) above its header or under its rows.
fn read_centred(
    flat: &[&TextItem],
    title: &TextItem,
    panel: &str,
    typical_of: Option<u32>,
    legend: &BTreeMap<String, String>,
    sheet: &str,
) -> Option<PanelSchedule> {
    let h = title.height;
    // A title over one side or the middle: the far side's header can be the table's width away.
    let near_title = |t: &TextItem| t.at.y < title.at.y && t.at.y > title.at.y - HEADER_REACH * h && (t.at.x - title.at.x).abs() < 120.0 * h;
    // The header: the nearest line under the title with a description column on each side.
    let descs: Vec<&TextItem> = flat.iter().copied().filter(|t| near_title(t) && t.value.to_ascii_uppercase().contains("DESCRIPTION")).collect();
    let top = descs.iter().map(|t| t.at.y).max_by(f64::total_cmp)?;
    let row_h = descs.iter().filter(|t| (t.at.y - top).abs() <= 0.6 * t.height).map(|t| t.height).fold(h, f64::min);
    if h < TITLE_OVER_CELLS * row_h && !labelled(&title.value) {
        return None;
    }
    let xs: Vec<f64> = descs.iter().filter(|t| (t.at.y - top).abs() <= 0.6 * row_h).map(|t| t.at.x).collect();
    let (lx, rx) = (xs.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    // A description column on each side, well apart.
    if rx - lx <= 20.0 * row_h {
        return None;
    }
    let (x0, x1) = (lx - 12.0 * row_h, rx + 12.0 * row_h);
    let head: Vec<&TextItem> = flat
        .iter()
        .copied()
        .filter(|t| (t.at.y - top).abs() <= 1.2 * row_h && t.at.x >= x0 && t.at.x <= x1 && !numberish(&t.value) && !t.value.trim().ends_with(':'))
        .collect();
    let head_bottom = head.iter().map(|t| t.at.y).fold(top, f64::min);
    let (lines, last_y, step) = rows_below(flat, head_bottom, x0, x1, row_h);
    // Each row's pair: one text near the middle, else two or three side by side read together.
    // The middle: its own header (`CKT`, `PHASE`) when it has one.
    let reach = 0.25 * (rx - lx);
    let mid0 = (lx + rx) / 2.0;
    let mid = head
        .iter()
        .filter(|t| CENTRE_HEADS.contains(&t.value.trim().to_ascii_uppercase().as_str()) && (t.at.x - mid0).abs() < reach)
        .map(|t| t.at.x)
        .min_by(|a, b| (a - mid0).abs().total_cmp(&(b - mid0).abs()))
        .unwrap_or(mid0);
    let pair_of = |ln: &[&TextItem]| -> Option<(u32, u32, f64, f64)> {
        if let Some((t, (a, b))) = ln.iter().filter(|t| (t.at.x - mid).abs() < reach).find_map(|t| centre_pair(&t.value).map(|p| (t, p))) {
            return Some((a, b, t.at.x, t.at.x));
        }
        let mut close: Vec<&&TextItem> = ln.iter().filter(|t| (t.at.x - mid).abs() <= 4.0 * row_h).collect();
        close.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
        [3usize, 2]
            .iter()
            .flat_map(|n| close.windows(*n))
            .filter_map(|w| {
                let joined = w.iter().map(|t| t.value.trim()).collect::<Vec<_>>().join(" ");
                let (a, b) = centre_pair(&joined)?;
                Some((a, b, w.first()?.at.x, w.last()?.at.x))
            })
            .min_by(|a, b| ((a.2 + a.3) / 2.0 - mid).abs().total_cmp(&((b.2 + b.3) / 2.0 - mid).abs()))
    };
    let pairs: Vec<Option<(u32, u32, f64, f64)>> = lines.iter().map(|ln| pair_of(ln)).collect();
    let mut los: Vec<f64> = pairs.iter().flatten().map(|p| p.2).collect();
    if los.len() < 2 {
        return None;
    }
    los.sort_by(f64::total_cmp);
    let centre = los.get(los.len() / 2).copied().unwrap_or(mid);
    // Each side's columns from its headers.
    let side = |left: bool| -> Side {
        let on: Vec<&&TextItem> = head.iter().filter(|t| if left { t.at.x < centre - 0.5 * row_h } else { t.at.x > centre + 0.5 * row_h }).collect();
        let named = |names: &[&str]| -> Option<f64> {
            on.iter()
                .filter(|t| names.contains(&t.value.trim().to_ascii_uppercase().as_str()))
                .map(|t| t.at.x)
                .min_by(|a, b| (a - centre).abs().total_cmp(&(b - centre).abs()))
        };
        let (amps, poles) = (named(&AMP_HEADS), named(&POLE_HEADS));
        let loads: Vec<f64> = on
            .iter()
            .filter(|t| {
                let u = t.value.trim().to_ascii_uppercase();
                letters(&u)
                    && !u.contains("DESCRIPTION")
                    && !AMP_HEADS.contains(&u.as_str())
                    && !POLE_HEADS.contains(&u.as_str())
                    && !NOT_LOAD.iter().any(|w| u.split_whitespace().any(|x| x == *w))
            })
            .map(|t| t.at.x)
            .collect();
        Side {
            num: centre,
            num_end: centre,
            amps,
            poles,
            wire: None,
            load: None,
            phases: Vec::new(),
            others: loads,
            left,
            kind: None,
            description: None,
        }
    };
    let sides = [side(true), side(false)];
    let mut side_rows: Vec<Vec<Cells>> = vec![Vec::new(), Vec::new()];
    for (ln, pair) in lines.iter().zip(&pairs) {
        let mut cells = [Cells::default(), Cells::default()];
        let mut loads: [Vec<(usize, f64)>; 2] = [Vec::new(), Vec::new()];
        if let Some((a, b, _, _)) = pair {
            for (c, n) in cells.iter_mut().zip([a, b]) {
                c.number = n.to_string();
            }
        }
        let (lo, hi) = pair.map_or((centre, centre), |p| (p.2, p.3));
        for t in ln.iter().filter(|t| t.at.x < lo - 0.1 * row_h || t.at.x > hi + 0.1 * row_h) {
            let si = usize::from(t.at.x > hi);
            let (Some(sd), Some(c), Some(ld)) = (sides.get(si), cells.get_mut(si), loads.get_mut(si)) else { continue };
            let v = t.value.trim();
            let x = t.at.x;
            let breaker_cols: Vec<f64> = [sd.amps, sd.poles].into_iter().flatten().collect();
            if number(v.trim_end_matches('A')).is_some() || amount(v).is_some() {
                let best = [(0usize, sd.amps), (1, sd.poles)]
                    .into_iter()
                    .filter_map(|(k, cx)| cx.map(|cx| ((x - cx).abs(), k)))
                    .chain(sd.others.iter().enumerate().map(|(i, cx)| ((x - cx).abs(), 2 + i)))
                    .filter(|(d, _)| *d <= COLUMN_REACH * row_h)
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                match best {
                    Some((_, 0)) if c.amps.is_empty() => c.amps = v.trim_end_matches('A').to_string(),
                    Some((_, 1)) if c.poles.is_empty() && number(v).is_some() => c.poles = v.to_string(),
                    Some((_, k)) if k >= 2 && !ld.iter().any(|(j, _)| *j == k) => {
                        if let Some(a) = amount(v) {
                            ld.push((k, a));
                        }
                    }
                    _ => {}
                }
            } else if letters(v) {
                // The description: outside the breaker's columns (left of them on the left side).
                let outside =
                    if si == 0 { breaker_cols.iter().all(|b| x < b - 0.5 * row_h) } else { breaker_cols.iter().all(|b| x > b + 0.5 * row_h) };
                if outside {
                    c.description.push(v.to_string());
                }
            }
        }
        for (c, ld) in cells.iter_mut().zip(&loads) {
            if !ld.is_empty() {
                c.load = ld.iter().map(|(_, a)| a).sum::<f64>().to_string();
            }
        }
        for (rows, c) in side_rows.iter_mut().zip(cells) {
            rows.push(c);
        }
    }
    let mut circuits = circuits_of(&side_rows, sheet);
    if circuits.is_empty() {
        return None;
    }
    circuits.sort_by_key(|c| c.number.parse::<u32>().unwrap_or(u32::MAX));
    // Its facts: above the header, else under the rows (above the next schedule's title).
    let mut header = header_facts(flat, top + 0.6 * row_h, title.at.y, x0, x1, row_h);
    let below = step.map_or(last_y - 12.0 * row_h, |s| last_y - 12.0 * s);
    let next_title = flat
        .iter()
        .filter(|t| t.at.y < last_y && t.at.y > below && t.at.x >= x0 && t.at.x <= x1 && title_panel(&t.value).is_some())
        .map(|t| t.at.y + 0.6 * t.height)
        .fold(below, f64::max);
    for (k, v) in header_facts(flat, next_title, last_y - 0.6 * row_h, x0, x1 + 6.0 * row_h, row_h) {
        header.entry(k).or_insert(v);
    }
    let end = step.map_or(last_y - 4.0 * row_h, |s| last_y - TOTALS_ROWS * s);
    let totals = totals_of(flat, last_y + 0.6 * row_h, end, x0, x1, row_h, &[]);
    Some(PanelSchedule { panel: panel.to_string(), typical_of, source: sheet.to_string(), circuits, legend: legend.clone(), header, totals })
}

/// The rows of a table under its header (`head_bottom`, between `x0` and `x1`), line by line,
/// until the subtotals, a gap of a few rows, or the next title; with the last row's height and
/// the step between rows. A subtotal's line ends the rows whichever of its texts comes first (its
/// number can sit a hair above its label).
/// The first row can stand this many text heights under the header's line (a header two cells
/// tall, its column names on the upper one).
const FIRST_ROW_REACH: f64 = 6.0;

fn rows_below<'a>(flat: &[&'a TextItem], head_bottom: f64, x0: f64, x1: f64, row_h: f64) -> (Vec<Vec<&'a TextItem>>, f64, Option<f64>) {
    let mut below: Vec<&'a TextItem> = flat.iter().copied().filter(|t| t.at.y < head_bottom - 0.5 * row_h && t.at.x >= x0 && t.at.x <= x1).collect();
    below.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
    let ends = |v: &str| {
        let u = v.trim().to_ascii_uppercase();
        u.starts_with("PER PHASE") || u.starts_with("TOTAL") || u.contains("SUB-TOTAL") || u.contains("SUBTOTAL")
    };
    let end_ys: Vec<f64> = below.iter().filter(|t| ends(&t.value)).map(|t| t.at.y).collect();
    let mut lines: Vec<Vec<&'a TextItem>> = Vec::new();
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
                    || (step.is_none() && gap > FIRST_ROW_REACH * row_h)
                    || lines.len() >= MAX_ROWS
                {
                    break;
                }
                // (The step is the rows' own, not the header's height above the first.)
                if !lines.is_empty() {
                    step.get_or_insert(gap);
                }
                last_y = t.at.y;
                lines.push(vec![t]);
            }
        }
    }
    (lines, last_y, step)
}

/// Each side's rows made circuits. A breaker of several poles spans as many rows, its number on
/// the first or the middle one; its load is each pole's row's (a row of its own with no
/// description is one of its poles).
fn circuits_of(side_rows: &[Vec<Cells>], sheet: &str) -> Vec<PanelCircuit> {
    let mut circuits: Vec<PanelCircuit> = Vec::new();
    let load = |c: &Cells| c.load.parse::<f64>().ok().filter(|v| v.is_finite());
    for rows in side_rows {
        let rows = &breaker_lines_joined(rows);
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
                design_breaker: String::new(),
                number: n.to_string(),
                description: c.description.join(" "),
                room: String::new(),
                breaker: c.amps.clone(),
                poles,
                protection: c.codes.clone(),
                wire: c.wire.clone(),
                load_va: (!loads.is_empty()).then(|| loads.iter().sum()),
                design_load_va: None,
                source: sheet.to_string(),
            });
        }
    }
    circuits
}

/// A side's rows with each breaker printed between its poles' rows (a two-pole breaker's amps and
/// poles on a line of their own, centred between its two numbered rows, the rest of its
/// description beside them) joined to the numbered row above it, when that row has no breaker of
/// its own: the row after it is then its other pole.
fn breaker_lines_joined(rows: &[Cells]) -> Vec<Cells> {
    let mut out: Vec<Cells> = Vec::with_capacity(rows.len());
    for c in rows {
        let between = number(&c.number).is_none() && !(c.amps.is_empty() && c.poles.is_empty()) && c.load.is_empty();
        match out.last_mut() {
            Some(above) if between && number(&above.number).is_some() && above.amps.is_empty() && above.poles.is_empty() => {
                above.amps.clone_from(&c.amps);
                above.poles.clone_from(&c.poles);
                if above.wire.is_empty() {
                    above.wire.clone_from(&c.wire);
                }
                above.description.extend(c.description.iter().cloned());
                above.codes.extend(c.codes.iter().cloned());
            }
            _ => out.push(Cells {
                number: c.number.clone(),
                description: c.description.clone(),
                amps: c.amps.clone(),
                poles: c.poles.clone(),
                wire: c.wire.clone(),
                load: c.load.clone(),
                codes: c.codes.clone(),
            }),
        }
    }
    out
}

/// Whether a sheet's trade code is `want` (`E`) or a combined one holding it (`MEP`, `ME`, `EP`:
/// two or more of mechanical, electrical and plumbing, and only trade letters).
pub(crate) fn trade_has(discipline: &str, want: &str) -> bool {
    let d = discipline.trim().to_ascii_uppercase();
    let w = want.trim().to_ascii_uppercase();
    d == w
        || (w.len() == 1
            && (2..=4).contains(&d.len())
            && d.chars().all(|c| "MEPFT".contains(c))
            && d.chars().filter(|c| "MEP".contains(*c)).count() >= 2
            && d.contains(w.as_str()))
}

/// Whether a sheet may print panel schedules, and how likely (lower first): a sheet titled for
/// them; an electrical sheet (alone or in a combined trade code), its schedule sheets before its
/// others and its plans last; then any sheet whose title names the electrical work (a schedule
/// sheet whose number was read under another trade's or an equipment tag's code).
pub(crate) fn schedule_sheet_rank(discipline: &str, kind: &str, title: &str) -> Option<u8> {
    let t = title.to_ascii_uppercase();
    let elec = trade_has(discipline, "E");
    let plan = kind.to_ascii_lowercase().contains("plan");
    if ["PANELBOARD", "PANEL SCHEDULE", "PANEL BOARD"].iter().any(|w| t.contains(w)) || (elec && t.contains("PANEL")) {
        Some(0)
    } else if elec && !plan && (t.contains("SCHEDULE") || kind.eq_ignore_ascii_case("schedule")) {
        Some(1)
    } else if elec && !plan {
        Some(2)
    } else if elec {
        Some(3)
    } else if ["ELECTRICAL", "ONE LINE", "ONE-LINE", "POWER RISER"].iter().any(|w| t.contains(w)) {
        Some(4)
    } else {
        None
    }
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
        if cadaddy_mepfix::wordrules::words("breakers.ground_fault_codes").contains(&code.as_str())
            || meaning.contains("GROUND FAULT")
            || meaning.contains("GFCI")
            || meaning.contains("DUAL")
        {
            gf = true;
        }
        if cadaddy_mepfix::wordrules::words("breakers.arc_fault_codes").contains(&code.as_str())
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
        if !matches!(base.as_str(), "device" | "light_fixture" | "plumbing_fixture" | "fixture" | "equipment") {
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
    fn a_schedule_with_its_column_names_on_one_line_and_its_description_last_is_read() {
        // `PANEL:"Q9"`; its header's column names each read as one text (`AMP POLE BKR LTG PWR`),
        // the breaker type in its own BKR column and the description past the load columns,
        // mirrored on the right. (Points of a 9.8 pt font: its capitals 7.2 high.)
        let h = 7.2;
        let mut v = vec![
            t(557.0, 880.0, "PANEL:\"Q9\"", 26.0),
            t(291.0, 812.0, "OCP", h),
            t(356.0, 812.0, "LOAD (kVA)", h),
            t(621.0, 812.0, "PHASE LOAD", h),
            t(234.0, 810.0, "CKT", h),
            t(1067.0, 810.0, "CKT", h),
            t(236.0, 796.0, "NO", h),
            t(262.0, 796.0, "AMP POLE BKR LTG PWR", h),
            t(402.0, 796.0, "CO", h),
            t(470.0, 796.0, "DESCRIPTION", h),
            t(602.0, 796.0, "A", h),
            t(649.0, 796.0, "B", h),
            t(695.0, 796.0, "C", h),
            t(777.0, 796.0, "DESCRIPTION", h),
            t(904.0, 796.0, "CO", h),
            t(927.0, 796.0, "PWR LTG BKR POLE AMP", h),
            t(1069.0, 796.0, "NO", h),
            t(300.0, 700.0, "AMPS PER PHASE = 90", h),
        ];
        let rows: [(f64, &[(f64, &str)]); 3] = [
            (
                782.0,
                &[
                    (241.0, "1"),
                    (267.0, "20"),
                    (298.0, "1"),
                    (322.0, "GF"),
                    (349.0, "0.4"),
                    (488.0, "WATER HEATER"),
                    (796.0, "BREAD WARMER"),
                    (929.0, "0.8"),
                    (990.0, "GF"),
                    (1016.0, "1"),
                    (1042.0, "20"),
                    (1074.0, "2"),
                ],
            ),
            (768.0, &[(241.0, "3"), (267.0, "30"), (298.0, "2"), (488.0, "FRYER"), (796.0, "SPARE"), (1016.0, "1"), (1042.0, "20"), (1074.0, "4")]),
            (754.0, &[(241.0, "5"), (270.0, "--"), (298.0, "--"), (796.0, "LIGHTS"), (1016.0, "1"), (1042.0, "20"), (1074.0, "6")]),
        ];
        for (y, cells) in rows {
            v.extend(cells.iter().map(|(x, c)| t(*x, y, c, h)));
        }
        let read = read(&v, "E-9");
        assert_eq!(read.len(), 1, "{read:?}");
        let p = &read[0];
        assert_eq!(p.panel, "Q9");
        let got: Vec<(String, String, u32, String, Vec<String>)> =
            p.circuits.iter().map(|c| (c.number.clone(), c.breaker.clone(), c.poles, c.description.clone(), c.protection.clone())).collect();
        let gf = || vec!["GF".to_string()];
        assert_eq!(
            got,
            [
                ("1".into(), "20".into(), 1, "WATER HEATER".into(), gf()),
                ("2".into(), "20".into(), 1, "BREAD WARMER".into(), gf()),
                ("3".into(), "30".into(), 2, "FRYER".into(), vec![]),
                ("4".into(), "20".into(), 1, "SPARE".into(), vec![]),
                ("6".into(), "20".into(), 1, "LIGHTS".into(), vec![]),
            ]
        );
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
    fn a_circuit_column_headed_circ_is_read() {
        // The circuit numbers' column headed CIRC (not CKT), the description right of it; the
        // header two cells tall (its first row five text heights under the column names).
        let h = 0.063;
        let mut v = vec![t(13.6, 24.59, "PANEL Q2", 0.126)];
        for (x, w) in [(9.43, "CIRC"), (9.99, "LOAD DESCRIPTION"), (11.88, "AMPS"), (12.31, "POLES")] {
            v.push(t(x, 23.84, w, h));
        }
        for (i, (n, d)) in [("1", "OFFICE RECEPTS"), ("3", "HAND DRYER"), ("5", "SPARE")].iter().enumerate() {
            let y = 23.84 - 5.0 * h - 0.16 * i as f64;
            v.push(t(9.47, y, n, h));
            v.push(t(10.0, y, d, h));
            v.push(t(11.93, y, "20", h));
            v.push(t(12.35, y, "1", h));
        }
        let s = read(&v, "E-9");
        let q = s.iter().find(|p| p.panel == "Q2").unwrap_or_else(|| panic!("{s:#?}"));
        let got: Vec<(&str, &str, &str)> = q.circuits.iter().map(|c| (c.number.as_str(), c.breaker.as_str(), c.description.as_str())).collect();
        assert_eq!(got, [("1", "20", "OFFICE RECEPTS"), ("3", "20", "HAND DRYER"), ("5", "20", "SPARE")]);
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
        assert_eq!(title_panel("PANEL [L4] SCHEDULE"), Some(("L4".into(), None)));
        assert_eq!(title_panel("PANEL: L2"), Some(("L2".into(), None)));
        assert_eq!(title_panel("PANEL NAME: RP-1"), Some(("RP-1".into(), None)));
        assert_eq!(title_panel("PANELBOARD: (H2A)"), Some(("H2A".into(), None)));
        assert_eq!(title_panel("PANEL NAME:"), None);
        assert_eq!(title_panel("PANEL NAME"), None);
        assert_eq!(title_panel("PANELS SHALL BE"), None);
        assert_eq!(title_panel("PANELBOARD SHALL BE"), None);
        assert_eq!(title_panel("PANEL:\"P1\""), Some(("P1".into(), None)));
        // A heading over the panels' notes names no panel.
        assert_eq!(title_panel("PANELBOARD NOTES"), None);
        // A schedule's header fields (`PANEL SIZE & TYPE:`, `PANEL LOCATION:`) name none either.
        assert_eq!(title_panel("PANEL SIZE & TYPE:"), None);
        assert_eq!(title_panel("PANEL LOCATION: UTILITY"), None);
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

    /// A schedule numbered down the middle (`1 A 2`), a column per kind of load on each side, its
    /// facts under the rows, a feeder table beside it on the same lines.
    fn centred_schedule() -> Vec<TextItem> {
        let h = 0.08;
        let mut v = vec![t(6.30, 23.15, "PANEL [L4] SCHEDULE", 0.16), t(16.90, 23.10, "FEEDER SCHEDULE", 0.19)];
        for (x, w) in [
            (6.70, "DESCRIPTION"),
            (8.01, "P"),
            (8.22, "AMP"),
            (8.67, "LIGHT"),
            (9.27, "RECEP"),
            (9.88, "MOTOR"),
            (10.50, "OTHER"),
            (11.10, "PHASE"),
            (11.75, "OTHER"),
            (12.40, "MOTOR"),
            (13.00, "RECEP"),
            (13.60, "LIGHT"),
            (14.10, "P"),
            (14.30, "AMP"),
            (15.10, "DESCRIPTION"),
        ] {
            v.push(t(x, 22.89, w, h));
        }
        // (left description, poles, amps, its load's column and VA; pair; right side the same)
        type Side<'a> = (&'a str, &'a str, &'a str, f64, &'a str);
        let rows: [(Side, &str, Side); 4] = [
            (("OFFICE LIGHTS", "1", "20", 8.70, "900"), "1 A 2", ("WATER HEATER", "2", "30", 12.42, "2250")),
            (("OFFICE RECEPTACLES", "1", "20", 9.30, "1080"), "3 B 4", ("", "", "", 12.42, "2250")),
            (("SPARE", "1", "20", 0.0, ""), "5 C 6", ("SPACE", "", "", 0.0, "")),
            (("COPIER", "1", "20", 10.52, "1500"), "7 A 8", ("", "", "", 0.0, "")),
        ];
        for (i, (l, pair, r)) in rows.iter().enumerate() {
            let y = 22.67 - 0.22 * i as f64;
            for (side, (desc, poles, amps, lx, load)) in [(0, l), (1, r)] {
                let (dx, px, ax) = if side == 0 { (6.25, 8.02, 8.28) } else { (14.65, 14.10, 14.35) };
                for (x, w) in [(dx, *desc), (px, *poles), (ax, *amps), (*lx, *load)] {
                    if !w.is_empty() {
                        v.push(t(x, y, w, h));
                    }
                }
                // The other load columns print zeros.
                for x in if side == 0 { [8.81, 9.43, 10.06] } else { [11.93, 13.06, 13.68] } {
                    if (x - lx).abs() > 0.3 {
                        v.push(t(x, y, "0", h));
                    }
                }
            }
            v.push(t(11.12, y, pair, h));
            // The feeder table's row on the same line.
            v.push(t(17.50, y - 0.01, "(3) #12 & (1) #12 G.", h));
            v.push(t(19.00, y - 0.01, "20", h));
        }
        v.extend([
            t(6.26, 21.79, "TOTALS", h),
            t(8.71, 21.79, "900", h),
            t(14.00, 21.40, "VOLTAGE:", h),
            t(15.60, 21.40, "208Y/120", h),
            t(14.00, 21.18, "MCB/MLO:", h),
            t(15.60, 21.18, "MCB", h),
            t(14.20, 20.96, "AMPS:", h),
            t(15.60, 20.96, "225", h),
        ]);
        v
    }

    #[test]
    fn a_schedule_numbered_down_the_middle_reads_each_sides_circuits_loads_and_facts() {
        let got = read(&centred_schedule(), "X-5");
        assert_eq!(got.iter().map(|p| p.panel.as_str()).collect::<Vec<_>>(), ["L4"], "{got:#?}");
        let Some(p) = got.first() else { return };
        let c = |n: &str| p.circuits.iter().find(|c| c.number == n).unwrap_or_else(|| panic!("no circuit {n}: {:#?}", p.circuits));
        assert_eq!(p.circuits.iter().map(|c| c.number.as_str()).collect::<Vec<_>>(), ["1", "2", "3", "5", "6", "7"], "{:#?}", p.circuits);
        assert_eq!((c("1").description.as_str(), c("1").breaker.as_str(), c("1").poles, c("1").load_va), ("OFFICE LIGHTS", "20", 1, Some(900.0)));
        // A two-pole breaker on the right: its number on its first row, each pole row's load summed.
        assert_eq!((c("2").description.as_str(), c("2").breaker.as_str(), c("2").poles, c("2").load_va), ("WATER HEATER", "30", 2, Some(4500.0)));
        assert_eq!(c("3").load_va, Some(1080.0));
        assert_eq!(c("7").load_va, Some(1500.0), "the OTHER column is a load");
        assert_eq!((c("6").description.as_str(), c("6").breaker.as_str()), ("SPACE", ""));
        assert!(p.circuits.iter().all(|c| !c.description.contains('#')), "the feeder table beside it is no part of it");
        let fact = |k: &str| p.header.get(k).map(String::as_str);
        assert_eq!((fact("voltage"), fact("mcb/mlo"), fact("amps")), (Some("208Y/120"), Some("MCB"), Some("225")), "{:?}", p.header);
    }

    /// A schedule as some design software prints it, read as the importer reads it: `Panel: L1`
    /// over `Label: value` facts, a header of one line whose narrow columns' names are cut short
    /// (`C...` over the circuit numbers, `NO...` over the notes) and joined with their neighbours'
    /// (`NO... C...`, `WIRE BRKR`), `WIRE | BRKR | #` for the wire, the breaker's amps and its
    /// poles, each circuit's load under its phase (`A | B | C` on each side, no LOAD column), a
    /// two-pole breaker on a line of its own between its poles' numbered rows, and a poles cell
    /// and a load read as one text (`1 180`).
    fn cut_short_schedule() -> Vec<TextItem> {
        let (hh, hv, hd) = (5.8, 5.7, 4.5);
        let mut v = vec![t(0.0, 0.0, "Panel: L1", 14.0)];
        for (x, y, w) in [
            (21.1, -13.7, "Location:"),
            (195.8, -13.7, "Volts:"),
            (370.1, -13.7, "A.I.C. Rating:"),
            (5.9, -25.7, "Supply From:"),
            (192.4, -25.7, "Phas..."),
            (367.8, -25.7, "Mains Rating:"),
            (18.5, -37.7, "Mounting:"),
            (374.0, -37.7, "Mains Type:"),
        ] {
            v.push(t(x, y, w, hh));
        }
        for (x, y, w) in [(222.0, -13.9, "120/208 Wye"), (423.7, -13.9, "22,000"), (58.8, -26.6, "MDP"), (222.0, -25.9, "3"), (423.7, -25.9, "200")] {
            v.push(t(x, y, w, hv));
        }
        v.extend([t(61.0, -37.8, "SURFACE", hv), t(423.7, -37.8, "MCB", hv)]);
        for (x, w) in [
            (-10.2, "NO... C..."),
            (32.3, "CIRCUIT DESCRIPTION"),
            (129.1, "WIRE BRKR"),
            (180.7, "#"),
            (199.0, "A"),
            (225.2, "B"),
            (251.6, "C"),
            (278.0, "A"),
            (304.4, "B"),
            (330.7, "C"),
            (350.5, "#"),
            (359.6, "BRKR WIRE"),
            (423.7, "CIRCUIT DESCRIPTION"),
            (528.2, "C... NO..."),
        ] {
            v.push(t(x, -49.7, w, hh));
        }
        let row = |i: usize| -61.8 - 12.0 * i as f64;
        // Circuit 1: its poles and its load one text.
        v.extend([t(15.5, row(0), "1", hv), t(27.8, row(0) + 0.6, "HAND DRYER", hd), t(135.0, row(0), "12", hv), t(160.2, row(0), "20", hv)]);
        v.push(t(180.7, row(0), "1 180", hv));
        // (odd, its description, wire, its load's column and load; the even one beside it)
        let rows = [
            (1, "3", "SPARE", "--", 219.4, "0", "4", "SPARE", "--", 305.2, "0"),
            (2, "5", "ICE MAKER", "12", 248.0, "1320", "6", "SIGNAGE", "10", 324.8, "1200"),
        ];
        for (i, a, ad, aw, ax, al, b, bd, bw, bx, bl) in rows {
            let y = row(i);
            v.extend([
                t(15.5, y, a, hv),
                t(27.8, y + 0.6, ad, hd),
                t(136.7, y, aw, hv),
                t(160.2, y, "20", hv),
                t(180.7, y, "1", hv),
                t(ax, y, al, hv),
            ]);
            v.extend([
                t(bx, y, bl, hv),
                t(350.5, y, "1", hv),
                t(366.7, y, "20", hv),
                t(391.8, y, bw, hv),
                t(411.0, y + 0.6, bd, hd),
                t(532.2, y, b, hv),
            ]);
        }
        v.extend([t(532.2, row(0), "2", hv), t(411.0, row(0) + 0.6, "OFFICE RECEPTACLES", hd), t(274.4, row(0), "540", hv)]);
        v.extend([t(350.5, row(0), "1", hv), t(366.7, row(0), "20", hv), t(391.8, row(0), "12", hv)]);
        // Circuits 7 and 9: one two-pole breaker, its amps, poles and wire between their rows, and
        // the rest of its description; a note's number in the notes columns (left and right).
        v.extend([t(15.5, row(3), "7", hv), t(27.8, row(3) + 0.6, "WATER", hd), t(199.7, row(3), "2000", hv)]);
        let mid = row(3) - 6.0;
        v.extend([t(135.0, mid, "10", hv), t(160.2, mid, "30", hv), t(180.7, mid, "2", hv), t(27.8, mid - 1.2, "HEATER", hd), t(-3.2, mid, "6", hv)]);
        v.extend([t(13.3, row(4), "9", hv), t(226.0, row(4), "2000", hv), t(550.8, row(4), "4", hv)]);
        v.push(t(0.0, -150.0, "PANEL TOTALS", hh));
        v
    }

    #[test]
    fn a_schedule_whose_column_names_are_cut_short_reads_its_circuits_and_facts() {
        let got = read(&cut_short_schedule(), "X-7");
        let p = got.iter().find(|p| p.panel == "L1").unwrap_or_else(|| panic!("{got:#?}"));
        let summary: Vec<(&str, &str, &str, u32, &str, Option<f64>)> =
            p.circuits.iter().map(|c| (c.number.as_str(), c.description.as_str(), c.breaker.as_str(), c.poles, c.wire.as_str(), c.load_va)).collect();
        assert_eq!(
            summary,
            [
                ("1", "HAND DRYER", "20", 1, "12", Some(180.0)),
                ("2", "OFFICE RECEPTACLES", "20", 1, "12", Some(540.0)),
                ("3", "SPARE", "20", 1, "", Some(0.0)),
                ("4", "SPARE", "20", 1, "", Some(0.0)),
                ("5", "ICE MAKER", "20", 1, "12", Some(1320.0)),
                ("6", "SIGNAGE", "20", 1, "10", Some(1200.0)),
                ("7", "WATER HEATER", "30", 2, "10", Some(4000.0)),
            ],
            "{:#?}",
            p.circuits
        );
        let fact = |k: &str| p.header.get(k).map(String::as_str);
        assert_eq!(
            (fact("volts"), fact("mains rating"), fact("mains type"), fact("supply from")),
            (Some("120/208 Wye"), Some("200"), Some("MCB"), Some("MDP")),
            "{:?}",
            p.header
        );
        assert_eq!(fact("location"), None, "a fact left blank takes no other label's text: {:?}", p.header);
    }

    /// A schedule whose circuit numbers face inward: `DESCRIPTION | BKR SIZE | BKR POLE | NOTE |
    /// CKT NO. | A | B | C | CKT NO. | NOTE | BKR POLE | BKR SIZE | DESCRIPTION`, its title a
    /// `PANEL:` label and its tag as tall as its cells, each phase's cell holding both circuits'
    /// loads (the left one's in its left half), breakers printed `20 A`.
    fn inward_schedule() -> Vec<TextItem> {
        let mut v = vec![t(0.0, 0.0, "PANEL:", 1.0), t(8.0, 0.0, "L2", 1.0), t(38.7, 0.0, "LOCATION:", 1.0), t(47.8, 0.0, "STORAGE", 0.97)];
        v.extend([
            t(0.0, -2.1, "SYSTEM:", 1.0),
            t(8.0, -2.1, "120/208V - 3P", 0.97),
            t(38.7, -4.1, "MAINS:", 1.0),
            t(47.8, -4.1, "200 A - MCB", 0.97),
        ]);
        for (x, y, w) in [
            (21.0, -8.6, "BKR"),
            (26.2, -8.6, "BKR"),
            (36.4, -8.6, "CKT"),
            (70.9, -8.6, "CKT"),
            (80.9, -8.6, "BKR"),
            (86.0, -8.6, "BKR"),
            (3.1, -9.4, "LOAD DESCRIPTION"),
            (31.0, -9.4, "NOTE"),
            (44.6, -9.4, "A"),
            (54.5, -9.4, "B"),
            (64.4, -9.4, "C"),
            (75.3, -9.4, "NOTE"),
            (93.6, -9.4, "LOAD DESCRIPTION"),
            (21.0, -10.1, "SIZE"),
            (25.8, -10.1, "POLE"),
            (36.5, -10.1, "NO."),
            (71.1, -10.1, "NO."),
            (80.6, -10.1, "POLE"),
            (86.0, -10.1, "SIZE"),
        ] {
            v.push(t(x, y, w, 1.0));
        }
        // (odd, description, its load's x and load; the even one's)
        let rows = [
            ("1", "EXTERIOR LIGHTS", 41.7, "90", "2", "MENU BOARD", 46.4, "200"),
            ("3", "BUILDING SIGN", 51.1, "1200", "4", "WARMER", 56.3, "204"),
            ("5", "WRAP LIGHT", 61.3, "168", "6", "LIGHTING", 66.4, "300"),
        ];
        for (i, (a, ad, ax, al, b, bd, bx, bl)) in rows.iter().enumerate() {
            let y = -12.7 - 2.3 * i as f64;
            v.extend([
                t(4.4, y, ad, 0.97),
                t(21.1, y, "20 A", 0.97),
                t(27.3, y, "1", 0.97),
                t(31.9, y, "C1", 0.97),
                t(37.4, y, a, 0.97),
                t(*ax, y, al, 0.97),
            ]);
            v.extend([
                t(*bx, y, bl, 0.97),
                t(71.9, y, b, 0.97),
                t(76.3, y, "C2", 0.97),
                t(82.0, y, "1", 0.97),
                t(86.1, y, "20 A", 0.97),
                t(95.8, y, bd, 0.97),
            ]);
        }
        v
    }

    #[test]
    fn a_schedule_whose_circuit_numbers_face_inward_reads_each_side_outward() {
        let got = read(&inward_schedule(), "X-9");
        let p = got.iter().find(|p| p.panel == "L2").unwrap_or_else(|| panic!("{got:#?}"));
        let summary: Vec<(&str, &str, &str, u32, Option<f64>)> =
            p.circuits.iter().map(|c| (c.number.as_str(), c.description.as_str(), c.breaker.as_str(), c.poles, c.load_va)).collect();
        assert_eq!(
            summary,
            [
                ("1", "EXTERIOR LIGHTS", "20", 1, Some(90.0)),
                ("2", "MENU BOARD", "20", 1, Some(200.0)),
                ("3", "BUILDING SIGN", "20", 1, Some(1200.0)),
                ("4", "WARMER", "20", 1, Some(204.0)),
                ("5", "WRAP LIGHT", "20", 1, Some(168.0)),
                ("6", "LIGHTING", "20", 1, Some(300.0)),
            ],
            "{:#?}",
            p.circuits
        );
        let fact = |k: &str| p.header.get(k).map(String::as_str);
        assert_eq!(
            (fact("system"), fact("mains"), fact("location")),
            (Some("120/208V - 3P"), Some("200 A - MCB"), Some("STORAGE")),
            "{:?}",
            p.header
        );
    }

    #[test]
    fn a_heading_cut_short_over_no_circuit_numbers_is_no_circuit_column() {
        // `C...` over a conduit's sizes (`2`, `3`, `2`): not the circuits' numbers.
        let h = 8.0;
        let mut v = vec![t(0.0, 0.0, "Panel: L1", 19.5), t(10.0, -40.0, "C...", h), t(40.0, -40.0, "DESCRIPTION", h)];
        for (i, n) in ["2", "3", "2"].iter().enumerate() {
            v.push(t(12.0, -52.0 - 12.0 * i as f64, n, h));
        }
        assert!(read(&v, "X-8").is_empty());
    }

    #[test]
    fn a_split_title_and_a_pair_in_three_cells_read_the_same() {
        // `PANEL:` and its tag in two cells as tall as the rest; `CKT` over the pair, printed as
        // three texts; facts above the header.
        let h = 5.0;
        let mut v = vec![t(100.0, 300.0, "PANEL:", h), t(125.0, 300.0, "L2", h), t(100.0, 295.0, "VOLTS:", h), t(130.0, 295.0, "480Y/277V", h)];
        for (x, w) in [
            (100.0, "CIRCUIT DESCRIPTION"),
            (150.0, "TRIP"),
            (162.0, "POLE"),
            (178.0, "LOAD (VA)"),
            (205.0, "CKT"),
            (225.0, "LOAD (VA)"),
            (240.0, "POLE"),
            (252.0, "TRIP"),
            (265.0, "CIRCUIT DESCRIPTION"),
        ] {
            v.push(t(x, 288.0, w, h));
        }
        for (i, (ld, ll, a, ph, b, rl, rd)) in
            [("CORRIDOR LIGHTS", "600", "1", "A", "2", "1500", "EXHAUST FAN"), ("RECEPTACLES", "720", "3", "B", "4", "", "SPARE")].iter().enumerate()
        {
            let y = 276.0 - 10.0 * i as f64;
            v.extend([t(100.0, y, ld, h), t(152.0, y, "20", h), t(163.0, y, "1", h), t(178.0, y, ll, h)]);
            v.extend([t(205.0, y, a, h), t(210.0, y, ph, h), t(215.0, y, b, h)]);
            v.extend([t(240.0, y, "1", h), t(252.0, y, "20", h), t(265.0, y, rd, h)]);
            if !rl.is_empty() {
                v.push(t(225.0, y, rl, h));
            }
        }
        let got = read(&v, "X-6");
        let p = got.iter().find(|p| p.panel == "L2").unwrap_or_else(|| panic!("{got:#?}"));
        let summary: Vec<(&str, &str, &str, Option<f64>)> =
            p.circuits.iter().map(|c| (c.number.as_str(), c.description.as_str(), c.breaker.as_str(), c.load_va)).collect();
        assert_eq!(
            summary,
            [
                ("1", "CORRIDOR LIGHTS", "20", Some(600.0)),
                ("2", "EXHAUST FAN", "20", Some(1500.0)),
                ("3", "RECEPTACLES", "20", Some(720.0)),
                ("4", "SPARE", "20", None)
            ]
        );
        assert_eq!(p.header.get("volts").map(String::as_str), Some("480Y/277V"), "{:?}", p.header);
    }

    /// A two-sided schedule `dx` right of the first whose name isn't printed as text: its
    /// `PANEL NAME:` label followed by `after` (nothing, another label, an empty `""`), and its
    /// circuits' descriptions.
    fn nameless(dx: f64, after: &[&str], rows: [(&str, &str); 2]) -> Vec<TextItem> {
        let x = |v: f64| v + dx;
        let mut v = vec![t(x(100.0), 520.0, "PANEL NAME:", 6.0)];
        for (i, a) in after.iter().enumerate() {
            v.push(t(x(170.0 + 60.0 * i as f64), 520.0, a, 6.0));
        }
        v.extend([t(x(100.0), 505.0, "VOLTS:", 6.0), t(x(140.0), 505.0, "208Y/120V", 6.0)]);
        for (vx, w) in [
            (100.0, "CKT"),
            (115.0, "DESCRIPTION"),
            (200.0, "AMPS"),
            (215.0, "P"),
            (230.0, "LOAD"),
            (330.0, "LOAD"),
            (345.0, "P"),
            (360.0, "AMPS"),
            (375.0, "DESCRIPTION"),
            (470.0, "CKT"),
        ] {
            v.push(t(x(vx), 490.0, w, 6.0));
        }
        for (i, (ld, rd)) in rows.iter().enumerate() {
            let y = 472.0 - 10.0 * i as f64;
            let (ln, rn) = ((2 * i + 1).to_string(), (2 * i + 2).to_string());
            v.extend([t(x(100.0), y, &ln, 6.0), t(x(115.0), y, ld, 6.0), t(x(202.0), y, "20", 6.0), t(x(215.0), y, "1", 6.0)]);
            v.extend([t(x(230.0), y, "500", 6.0), t(x(330.0), y, "400", 6.0), t(x(345.0), y, "1", 6.0), t(x(362.0), y, "20", 6.0)]);
            v.extend([t(x(375.0), y, rd, 6.0), t(x(470.0), y, &rn, 6.0)]);
        }
        v.push(t(x(150.0), 445.0, "PER PHASE SUB-TOTALS", 6.0));
        v
    }

    fn made(p: &PanelSchedule) -> (&str, &str) {
        (p.header.get(NAME_MADE).map_or("", String::as_str), p.header.get(NAME_BASIS).map_or("", String::as_str))
    }

    #[test]
    fn schedules_whose_panel_names_are_not_text_are_read_and_named_from_the_set() {
        // Three schedules side by side, none printing its name: one label alone, one followed by
        // the next label, one by an empty `""`. The rightmost feeds the other two.
        let mut sheet = nameless(0.0, &[], [("LIGHTING SALES", "RECEPTACLES SALES"), ("SIGN", "SPARE")]);
        sheet.extend(nameless(500.0, &["FED BY:"], [("KITCHEN HOOD", "WALK-IN COOLER"), ("FRYER", "SPACE")]));
        sheet.extend(nameless(1000.0, &["\"\""], [("PANEL QA1", "PANEL QA2"), ("SPARE", "SPACE")]));
        // Before: no schedule at all (the label has no tag); each table read now.
        let alone = read(&sheet, "X-1");
        assert_eq!(alone.len(), 3, "{alone:#?}");
        assert!(alone.iter().all(|p| p.circuits.len() == 4), "{alone:#?}");
        // On its own the sheet names QA1 and QA2 (feeder rows) but not the panel feeding them.
        let feeder = alone.iter().find(|p| p.circuits.iter().any(|c| c.description == "PANEL QA1")).unwrap();
        assert_eq!((feeder.panel.as_str(), made(feeder).0), ("UNNAMED-1", "unresolved"), "{alone:#?}");
        // The set's notes on another sheet name the main panel (quoted) and the feeders.
        let notes = vec![
            t(50.0, 300.0, "1. SERVICE ENTRANCE CONDUCTORS FROM TRANSFORMER TO PANEL \"QM\".", 6.0),
            t(50.0, 290.0, "2. PANEL SHALL BE SURFACE MOUNTED.", 6.0),
            t(50.0, 280.0, "3. FEEDERS FROM PANEL \"QM\" TO PANEL \"QA2\" AND PANEL QA1.", 6.0),
            // A schedule's label naming no panel, its word printed alone elsewhere (a column's).
            t(50.0, 270.0, "CALCULATED PANEL AMPS:", 6.0),
            t(50.0, 260.0, "AMPS", 6.0),
        ];
        let set = read_set(&[("X-1", &sheet), ("X-2", &notes)]);
        let by = |d: &str| set.iter().find(|p| p.circuits.iter().any(|c| c.description == d)).unwrap();
        assert_eq!(set.len(), 3, "{set:#?}");
        assert_eq!(by("PANEL QA1").panel, "QM", "the panel feeding the others is the one nothing feeds");
        assert_eq!(by("LIGHTING SALES").panel, "QA1", "the rest in their order on the sheet, in tag order");
        assert_eq!(by("KITCHEN HOOD").panel, "QA2");
        for p in &set {
            let (m, basis) = made(p);
            assert_eq!(m, "inferred", "{p:#?}");
            assert!(basis.contains("not printed as text"), "{basis}");
        }
        assert!(made(by("LIGHTING SALES")).1.contains("alternative"), "a real choice notes the other one");
        // The feeder's circuits and breakers as read.
        let qm = by("PANEL QA1");
        assert_eq!(qm.circuits.iter().filter(|c| c.breaker == "20").count(), 4, "{qm:#?}");
    }

    #[test]
    fn a_name_left_for_no_schedule_is_a_placeholder_and_a_printed_name_still_wins() {
        // No notes: each unnamed schedule a placeholder of its own, never merged with another.
        // (The next label, whatever its word, is no tag: `LOCATION:` names no panel.)
        let mut sheet = nameless(0.0, &[], [("LIGHTING", "RECEPTACLES"), ("SIGN", "SPARE")]);
        sheet.extend(nameless(500.0, &["VOLTAGE:"], [("HOOD", "COOLER"), ("FRYER", "SPACE")]));
        sheet.extend(nameless(1000.0, &["LOCATION:", "STORAGE 104"], [("OVEN", "MIXER"), ("SPARE", "SPACE")]));
        let set = read_set(&[("X-1", &sheet)]);
        let mut names: Vec<&str> = set.iter().map(|p| p.panel.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["UNNAMED-1", "UNNAMED-2", "UNNAMED-3"], "{set:#?}");
        assert!(set.iter().all(|p| made(p).0 == "unresolved" && made(p).1.contains("X-1")));
        // A printed title over the same table: one schedule, by its printed name, nothing inferred.
        let mut titled = nameless(0.0, &[], [("LIGHTING", "RECEPTACLES"), ("SIGN", "SPARE")]);
        titled.push(t(250.0, 535.0, "PANELBOARD QB3", 10.0));
        let got = read(&titled, "X-1");
        assert_eq!(got.len(), 1, "{got:#?}");
        assert_eq!((got.first().map(|p| p.panel.as_str()), got.first().map(|p| made(p).0)), (Some("QB3"), Some("")));
        // A label and its tag in two cells is the printed name, as before.
        let mut split = nameless(0.0, &["QB4"], [("LIGHTING", "RECEPTACLES"), ("SIGN", "SPARE")]);
        split.retain(|x| x.value != "VOLTS:" && x.value != "208Y/120V");
        let got = read(&split, "X-1");
        assert_eq!(got.iter().map(|p| p.panel.as_str()).collect::<Vec<_>>(), ["QB4"], "{got:#?}");
    }

    #[test]
    fn the_panels_a_set_names_are_its_quoted_numbered_or_labelled_tags() {
        let texts = vec![
            t(0.0, 0.0, "FROM PANEL \u{201c}QM\u{201d} TO PANELBOARD QA-2.", 5.0),
            t(0.0, -10.0, "PANEL SHALL BE FLUSH. PANEL BOARD QA3 BY OTHERS. PANEL WITH LOCK.", 5.0),
            t(0.0, -20.0, "PANEL QZ IN STORAGE; PANEL QY BELOW", 5.0),
            // The one-line labels QY on its own: a panel's tag.
            t(50.0, -40.0, "QY", 5.0),
            t(0.0, -30.0, "PANEL NAME:", 5.0),
        ];
        assert_eq!(panel_mentions(&[&texts]), ["QM", "QA-2", "QA3", "QY"]);
        assert!(natural("QA2") < natural("QA10") && natural("QA10") < natural("QB1"));
        assert_eq!((ordinal(1).as_str(), ordinal(2).as_str(), ordinal(11).as_str(), ordinal(23).as_str()), ("1st", "2nd", "11th", "23rd"));
    }

    #[test]
    fn panel_schedules_are_looked_for_on_electrical_combined_and_titled_sheets() {
        assert!(trade_has("E", "E") && trade_has("MEP", "E") && trade_has("ep", "E") && trade_has("MEP", "P"));
        assert!(!trade_has("FP", "P") && !trade_has("ELEV", "E") && !trade_has("A", "E") && !trade_has("PE1", "E"));
        let rank = schedule_sheet_rank;
        assert_eq!(rank("E", "schedule", "PANEL SCHEDULES"), Some(0));
        assert_eq!(rank("UH", "unknown", "PANELBOARD SCHEDULES"), Some(0), "a schedule sheet whose number was read as an equipment tag");
        assert_eq!(rank("MEP", "schedule", "SCHEDULES AND DETAILS"), Some(1));
        assert_eq!(rank("E", "detail", "DETAILS"), Some(2));
        assert_eq!(rank("E", "plan", "FLOOR PLAN - POWER"), Some(3));
        assert_eq!(rank("G", "unknown", "ELECTRICAL ONE-LINE DIAGRAM"), Some(4));
        assert_eq!(rank("A", "elevation", "METAL PANEL ELEVATIONS"), None);
        assert_eq!(rank("A", "plan", "FLOOR PLAN"), None);
    }
}
