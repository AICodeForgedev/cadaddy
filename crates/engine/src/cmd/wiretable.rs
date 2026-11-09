//! The branch-circuit wire-length table a print set draws (Q6338): the conductor a circuit of a
//! breaker and voltage needs for its length (`20 A / 120 V: #12 to 50 ft, #10 to 100 ft…`).
//! Where the documents print one it decides the conductor ahead of the generic voltage-drop
//! calculation.
//!
//! A table is found by its title's words (`validate_schedules.json`, `length_table`) and read by
//! its drawn grid ([`super::gridtable`]), any layout: conductor sizes across the header with the
//! breaker and voltage in each row (or in the header), sizes down a column with a length column
//! or a column per voltage, or one length range and one size per row. The single-line form
//! (`0-60' = #12`, [`super::conduitroute::length_table`]) reads as a table too. Every cell is a
//! [`Band`]: a conductor size good to a length, for a breaker and a voltage when the table says.

use std::sync::OnceLock;

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use serde::Deserialize;

use super::gridtable::{self, GridTable};

/// Bands kept per table, tables per sheet, titles looked at (hostile files).
const MAX_BANDS: usize = 200;
const MAX_TABLES: usize = 16;
const MAX_TITLES: usize = 64;
/// Entities looked at on one drawing.
const MAX_ENTITIES: usize = 4_000_000;
/// Texts and segments gathered in one table's window, at most.
const MAX_WINDOW_ITEMS: usize = 200_000;
/// A length a table can give, feet.
const MAX_FT: f64 = 5_000.0;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LengthData {
    pub title_words: Vec<String>,
    pub title_kinds: Vec<String>,
    pub not_titles: Vec<String>,
    pub length_words: Vec<String>,
    pub amps_words: Vec<String>,
    pub volts_words: Vec<String>,
    pub size_words: Vec<String>,
    pub max_title_chars: usize,
    /// The window round a title read for its table: across and down, in the title's heights.
    pub window_h: Vec<f64>,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BreakerData {
    pub standard_a: Vec<f64>,
    pub standard_source: String,
    pub bus_words: Vec<String>,
    pub over_wire_factor: f64,
    pub over_wire_source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Data {
    pub length_table: LengthData,
    pub breaker: BreakerData,
}

/// Validate's schedule reading data (`validate_schedules.json`).
pub(crate) fn data() -> &'static Data {
    static D: OnceLock<Data> = OnceLock::new();
    D.get_or_init(|| {
        let mut d: Data = serde_json::from_str(include_str!("validate_schedules.json")).unwrap_or_default();
        d.length_table.max_title_chars = d.length_table.max_title_chars.clamp(20, 400);
        if d.length_table.window_h.len() < 2 {
            d.length_table.window_h = vec![150.0, 300.0];
        }
        if !(d.breaker.over_wire_factor.is_finite() && d.breaker.over_wire_factor >= 1.0) {
            d.breaker.over_wire_factor = 4.0;
        }
        d
    })
}

/// One cell of a wire-length table: `size` (the tables' key, `12`, `1/0`) is good to `max_ft`,
/// for a breaker of `amps` at `volts` when the table says.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Band {
    pub amps: Option<f64>,
    pub volts: Option<f64>,
    pub size: String,
    pub max_ft: f64,
}

/// A wire-length table read off a sheet.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LengthTable {
    pub sheet: String,
    pub title: String,
    pub bands: Vec<Band>,
}

/// What a table asks of a circuit.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Need {
    /// This size or larger (and the band that says so).
    Size(String, String),
    /// The circuit is longer than the table reaches for its breaker and voltage.
    Beyond(String),
    /// The table has no row for this circuit (another breaker or voltage), or the circuit's
    /// breaker or voltage isn't known where the table is by them.
    NotCovered(String),
}

impl Band {
    fn words(&self) -> String {
        let mut w = Vec::new();
        if let Some(a) = self.amps {
            w.push(format!("{a} A"));
        }
        if let Some(v) = self.volts {
            w.push(format!("{v} V"));
        }
        let head = if w.is_empty() { String::new() } else { format!("{}: ", w.join(" / ")) };
        format!("{head}#{} to {} ft", self.size, self.max_ft)
    }
}

impl LengthTable {
    /// Its name as a finding cites it.
    pub fn cite(&self) -> String {
        let t = if self.title.is_empty() { "the drawing's wire-length table".to_string() } else { self.title.clone() };
        if self.sheet.is_empty() { t } else { format!("{t} on {}", self.sheet) }
    }

    /// The conductor the table asks for a circuit `ft` long on a breaker of `amps` at `volts`.
    /// `sizes` ranks the conductor sizes, smallest first.
    pub fn need(&self, ft: f64, amps: Option<f64>, volts: Option<f64>, sizes: &[String]) -> Need {
        let by_amps = self.bands.iter().any(|b| b.amps.is_some());
        let by_volts = self.bands.iter().any(|b| b.volts.is_some());
        if by_amps && amps.is_none() {
            return Need::NotCovered("the table is by breaker and the circuit's breaker isn't known".into());
        }
        if by_volts && volts.is_none() {
            return Need::NotCovered("the table is by voltage and the circuit's voltage isn't known".into());
        }
        let same_v = |a: f64, b: f64| (a - b).abs() <= 0.06 * b.max(1.0);
        let rows: Vec<&Band> = self
            .bands
            .iter()
            .filter(|b| b.amps.is_none_or(|a| amps.is_some_and(|x| (a - x).abs() < 0.5)))
            .filter(|b| b.volts.is_none_or(|v| volts.is_some_and(|x| same_v(v, x))))
            .collect();
        if rows.is_empty() {
            let mut w = Vec::new();
            if let Some(a) = amps.filter(|_| by_amps) {
                w.push(format!("{a} A"));
            }
            if let Some(v) = volts.filter(|_| by_volts) {
                w.push(format!("{v} V"));
            }
            return Need::NotCovered(format!("the table has no row for a {} circuit", w.join(" / ")));
        }
        let rank = |s: &str| sizes.iter().position(|x| x == s);
        let pick = rows.iter().filter(|b| ft <= b.max_ft + 1e-9).filter_map(|b| rank(&b.size).map(|k| (k, *b))).min_by_key(|x| x.0);
        match pick {
            Some((_, b)) => Need::Size(b.size.clone(), b.words()),
            None => {
                let reach = rows.iter().map(|b| b.max_ft).fold(0.0, f64::max);
                Need::Beyond(format!("the table reaches {reach} ft"))
            }
        }
    }
}

fn up(v: &str) -> String {
    v.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase()
}

/// The words of a text (letters and digits).
fn words_of(v: &str) -> Vec<String> {
    v.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn has_word(v: &str, list: &[String]) -> bool {
    let w = words_of(v);
    list.iter().any(|x| {
        let xw = words_of(x);
        !xw.is_empty() && w.windows(xw.len()).any(|win| win == xw.as_slice())
    })
}

/// Is a text a wire-length table's title?
pub(crate) fn is_title(v: &str) -> bool {
    let dt = &data().length_table;
    let u = up(v);
    !u.is_empty()
        && u.len() <= dt.max_title_chars
        && !u.ends_with('.')
        && !dt.not_titles.iter().any(|n| u.contains(&n.to_ascii_uppercase()))
        && dt.title_words.iter().any(|w| u.contains(&w.to_ascii_uppercase()))
        && dt.title_kinds.iter().any(|w| u.contains(&w.to_ascii_uppercase()))
}

/// A conductor size in a text (`#12`, `#12 AWG`, `12 AWG`, `NO. 10`, `#1/0`): the tables' key.
/// `bare`: a plain number is one too (a size column's cell).
pub(crate) fn size_in(v: &str, bare: bool) -> Option<String> {
    let u = up(v);
    let take = |s: &str| -> Option<String> {
        let k: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
        let k = k.trim_end_matches('/').to_string();
        let ok = !k.is_empty() && k.len() <= 4 && (k.contains('/') || k.parse::<u32>().is_ok_and(|n| (1..=18).contains(&n) || n >= 250));
        ok.then_some(k)
    };
    if let Some((_, rest)) = u.split_once('#') {
        return take(rest);
    }
    if let Some((_, rest)) = u.split_once("NO.") {
        return take(rest);
    }
    let w: Vec<&str> = u.split_whitespace().collect();
    if let Some(i) = w.iter().position(|x| *x == "AWG" || *x == "KCMIL" || *x == "MCM") {
        if let Some(prev) = i.checked_sub(1).and_then(|p| w.get(p)) {
            return take(prev);
        }
        return w.get(i + 1).and_then(|n| take(n));
    }
    if bare && w.len() == 1 {
        return w.first().and_then(|n| take(n)).filter(|k| k.len() == u.len());
    }
    None
}

/// The numbers in a text with what follows each (`20A/120V` → (20, "A"), (120, "V")).
fn numbers(v: &str) -> Vec<(f64, String)> {
    let u = v.to_ascii_uppercase();
    let cs: Vec<char> = u.chars().take(400).collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < cs.len() && out.len() < 24 {
        let Some(c) = cs.get(i) else { break };
        if c.is_ascii_digit() {
            let start = i;
            while cs.get(i).is_some_and(|c| c.is_ascii_digit() || *c == '.' || *c == ',') {
                i += 1;
            }
            let n: String = cs.get(start..i).unwrap_or_default().iter().filter(|c| **c != ',').collect();
            let n = n.trim_end_matches('.');
            while cs.get(i).is_some_and(|c| *c == ' ') {
                i += 1;
            }
            let suffix: String =
                cs.get(i..).unwrap_or_default().iter().take_while(|c| c.is_ascii_alphabetic() || **c == '\'' || **c == '\u{2019}').collect();
            if let Ok(x) = n.parse::<f64>()
                && x.is_finite()
            {
                out.push((x, suffix));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The breaker amps a text gives (`20A`, `20 AMP`); `bare`: a lone number too (a breaker cell).
fn amps_in(v: &str, bare: bool) -> Option<f64> {
    let ns = numbers(v);
    ns.iter()
        .find(|(x, s)| (*x >= 1.0 && *x <= 6000.0) && (s == "A" || s.starts_with("AMP")))
        .or_else(|| ns.first().filter(|(x, s)| bare && s.is_empty() && *x >= 1.0 && *x <= 6000.0))
        .map(|(x, _)| *x)
}

/// The voltage a text gives (`120V`, `208 VOLT`); `bare`: a lone number too (a voltage cell).
fn volts_in(v: &str, bare: bool) -> Option<f64> {
    let ns = numbers(v);
    ns.iter()
        .find(|(x, s)| (90.0..=600.0).contains(x) && (s == "V" || s.starts_with("VOLT") || s.starts_with("VAC")))
        .or_else(|| ns.first().filter(|(x, s)| bare && s.is_empty() && (90.0..=600.0).contains(x)))
        .map(|(x, _)| *x)
}

/// The length a cell gives, feet: the greatest of its numbers (`51'-100'` → 100, `UP TO 50 FT`
/// → 50, `50'-0"` → 50); none for a size (`#12`).
fn length_in(v: &str) -> Option<f64> {
    let u = up(v);
    if u.is_empty() || u.contains('#') || u.contains("AWG") || u.contains("KCMIL") {
        return None;
    }
    // Inches after a foot mark aren't another length (`50'-6"`).
    let ns = numbers(&u);
    let feet: Vec<f64> = ns.iter().filter(|(_, s)| !s.starts_with('"')).map(|(x, _)| *x).collect();
    let marked = ns.iter().any(|(_, s)| s.starts_with('\'') || s.starts_with('\u{2019}') || s.starts_with("FT") || s.starts_with("FEET"));
    let plain = ns.len() <= 2 && u.chars().all(|c| c.is_ascii_digit() || " -'.,\u{2019}TOUP".contains(c));
    if !(marked || plain) {
        return None;
    }
    feet.into_iter().filter(|x| *x > 0.0 && *x <= MAX_FT).fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))))
}

/// What a column is by its header.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Col {
    /// A conductor size's lengths.
    Size(usize),
    Amps,
    Volts,
    /// The row's conductor size.
    Wire,
    /// Lengths (the row's size good to them).
    Length,
    Other,
}

/// A wire-length table's bands, any layout.
pub(crate) fn bands_of(t: &GridTable) -> Vec<Band> {
    let dt = &data().length_table;
    let mut sizes: Vec<String> = Vec::new();
    let cols: Vec<Col> = t
        .headers
        .iter()
        .map(|h| {
            if let Some(s) = size_in(h, false) {
                sizes.push(s);
                return Col::Size(sizes.len() - 1);
            }
            if has_word(h, &dt.length_words) {
                Col::Length
            } else if has_word(h, &dt.amps_words) {
                Col::Amps
            } else if has_word(h, &dt.volts_words) {
                Col::Volts
            } else if has_word(h, &dt.size_words) {
                Col::Wire
            } else {
                Col::Other
            }
        })
        .collect();
    let mut out: Vec<Band> = Vec::new();
    for row in t.rows.iter().take(500) {
        let cell = |i: usize| row.get(i).map_or("", String::as_str);
        let col_amps = cols.iter().position(|c| *c == Col::Amps).and_then(|i| amps_in(cell(i), true));
        let col_volts = cols.iter().position(|c| *c == Col::Volts).and_then(|i| volts_in(cell(i), true));
        // A row label that says both (`20A / 120V`), else the explicit columns.
        let label = cols.iter().enumerate().filter(|(_, c)| **c == Col::Other).map(|(i, _)| cell(i)).collect::<Vec<_>>().join(" ");
        let amps = col_amps.or_else(|| amps_in(&label, false));
        let volts = col_volts.or_else(|| volts_in(&label, false));
        let wire = cols.iter().position(|c| *c == Col::Wire).and_then(|i| size_in(cell(i), true)).or_else(|| {
            // One size among the row's other cells.
            let found: Vec<String> =
                cols.iter().enumerate().filter(|(_, c)| **c == Col::Other).filter_map(|(i, _)| size_in(cell(i), false)).collect();
            (found.len() == 1).then(|| found.into_iter().next()).flatten()
        });
        for (i, c) in cols.iter().enumerate() {
            if out.len() >= MAX_BANDS {
                break;
            }
            let v = cell(i);
            let Some(ft) = length_in(v) else { continue };
            let band = match c {
                Col::Size(k) => sizes.get(*k).map(|s| Band { amps, volts, size: s.clone(), max_ft: ft }),
                Col::Length => wire.clone().map(|s| Band { amps, volts, size: s, max_ft: ft }),
                // A column per voltage or breaker (`120V`, `20A`), the row's size.
                Col::Other => {
                    let h = t.headers.get(i).map_or("", String::as_str);
                    let (ha, hv) = (amps_in(h, false), volts_in(h, false));
                    match (&wire, ha.is_some() || hv.is_some()) {
                        (Some(s), true) => Some(Band { amps: ha.or(amps), volts: hv.or(volts), size: s.clone(), max_ft: ft }),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(b) = band
                && !out.contains(&b)
            {
                out.push(b);
            }
        }
    }
    // One cell is no table.
    if out.len() < 2 { Vec::new() } else { out }
}

/// The wire-length tables among one sheet's texts and segments.
pub(crate) fn read_page(texts: &[TextItem], segs: &[(Vec2, Vec2)], sheet: &str) -> Vec<LengthTable> {
    let mut out: Vec<LengthTable> = Vec::new();
    for t in texts.iter().filter(|t| t.rotation.abs() < 0.05 && is_title(&t.value)).take(MAX_TITLES) {
        if out.len() >= MAX_TABLES {
            break;
        }
        if let Some(tb) = gridtable::read_at(texts, segs, t, sheet) {
            let bands = bands_of(&tb);
            if !bands.is_empty() {
                out.push(LengthTable { sheet: sheet.to_string(), title: up(&t.value), bands });
            }
        }
    }
    // The single-line form (`0-60' = #12`).
    let strings: Vec<String> = texts.iter().map(|t| t.value.clone()).collect();
    let (bands, said) = super::conduitroute::length_table(&strings);
    if !bands.is_empty() {
        out.push(LengthTable {
            sheet: sheet.to_string(),
            title: if said.is_empty() { String::new() } else { said },
            bands: bands.into_iter().map(|(hi, size)| Band { amps: None, volts: None, size, max_ft: hi }).collect(),
        });
    }
    out
}

/// The wire-length tables a drawing holds: each title found, its table read in a window round
/// it; `sheet_of` names the sheet a point is on.
pub(crate) fn read_drawing(d: &Drawing, sheet_of: &dyn Fn(Vec2) -> String) -> Vec<LengthTable> {
    let dt = &data().length_table;
    let mut titles: Vec<TextItem> = Vec::new();
    let mut strings: Vec<String> = Vec::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        let (v, at, h, rot) = match &e.kind {
            EntityKind::Text(t) => (t.value.as_str(), t.insert.xy(), t.height, t.rotation),
            EntityKind::MText(t) => (t.contents.as_str(), t.insert.xy(), t.height, t.rotation),
            _ => continue,
        };
        if strings.len() < MAX_WINDOW_ITEMS && (v.contains('#') && v.contains('\'')) {
            strings.push(v.to_string());
        }
        if at.is_finite() && h.is_finite() && h > 0.0 && is_title(v) && titles.len() < MAX_TITLES {
            titles.push(TextItem { value: v.trim().to_string(), at, height: h, rotation: if rot.is_finite() { rot } else { 0.0 } });
        }
    }
    let mut out: Vec<LengthTable> = Vec::new();
    let (across, down) = (dt.window_h.first().copied().unwrap_or(150.0), dt.window_h.get(1).copied().unwrap_or(300.0));
    for t in &titles {
        if out.len() >= MAX_TABLES {
            break;
        }
        let h = t.height;
        let r = [t.at.x - across * h, t.at.y - down * h, t.at.x + across * h, t.at.y + 2.0 * h];
        let (texts, segs) = window(d, r);
        let sheet = sheet_of(t.at);
        if let Some(tb) = gridtable::read_at(&texts, &segs, t, &sheet) {
            let bands = bands_of(&tb);
            if !bands.is_empty() {
                out.push(LengthTable { sheet, title: up(&t.value), bands });
            }
        }
    }
    let (bands, said) = super::conduitroute::length_table(&strings);
    if !bands.is_empty() {
        out.push(LengthTable {
            sheet: String::new(),
            title: said,
            bands: bands.into_iter().map(|(hi, size)| Band { amps: None, volts: None, size, max_ft: hi }).collect(),
        });
    }
    out
}

/// A drawing's horizontal texts and straight segments inside `r`.
fn window(d: &Drawing, r: [f64; 4]) -> (Vec<TextItem>, Vec<(Vec2, Vec2)>) {
    let inside = |p: Vec2| p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3];
    let mut texts = Vec::new();
    let mut segs = Vec::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        if texts.len() + segs.len() >= MAX_WINDOW_ITEMS {
            break;
        }
        match &e.kind {
            EntityKind::Text(t) if inside(t.insert.xy()) && t.height.is_finite() && t.height > 0.0 => {
                texts.push(TextItem { value: t.value.trim().to_string(), at: t.insert.xy(), height: t.height, rotation: t.rotation })
            }
            EntityKind::MText(t) if inside(t.insert.xy()) && t.height.is_finite() && t.height > 0.0 => {
                texts.push(TextItem { value: t.contents.trim().to_string(), at: t.insert.xy(), height: t.height, rotation: t.rotation })
            }
            k => {
                let mut part = Vec::new();
                gridtable::entity_segments(k, &mut part);
                segs.extend(part.into_iter().filter(|(a, b)| inside(*a) || inside(*b)));
            }
        }
    }
    (texts, segs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizes() -> Vec<String> {
        ["14", "12", "10", "8", "6", "4"].iter().map(|s| s.to_string()).collect()
    }

    fn table(headers: &[&str], rows: &[&[&str]]) -> GridTable {
        GridTable {
            headers: headers.iter().map(|h| h.to_string()).collect(),
            rows: rows.iter().map(|r| r.iter().map(|c| c.to_string()).collect()).collect(),
            ..GridTable::default()
        }
    }

    #[test]
    fn sizes_across_with_a_breaker_and_voltage_row_each() {
        let t = table(
            &["BREAKER", "VOLTAGE", "#12", "#10", "#8", "#6"],
            &[&["20A", "120V", "50'", "100'", "150'", "250'"], &["20A", "208V", "90'", "150'", "240'", "380'"]],
        );
        let b = bands_of(&t);
        assert_eq!(b.len(), 8, "{b:#?}");
        let lt = LengthTable { sheet: "X-1".into(), title: "T".into(), bands: b };
        assert_eq!(lt.need(40.0, Some(20.0), Some(120.0), &sizes()), Need::Size("12".into(), "20 A / 120 V: #12 to 50 ft".into()));
        assert!(matches!(lt.need(96.0, Some(20.0), Some(120.0), &sizes()), Need::Size(s, _) if s == "10"));
        assert!(matches!(lt.need(96.0, Some(20.0), Some(208.0), &sizes()), Need::Size(s, _) if s == "10"));
        assert!(matches!(lt.need(60.0, Some(20.0), Some(208.0), &sizes()), Need::Size(s, _) if s == "12"));
        assert!(matches!(lt.need(400.0, Some(20.0), Some(120.0), &sizes()), Need::Beyond(_)));
        assert!(matches!(lt.need(40.0, Some(30.0), Some(120.0), &sizes()), Need::NotCovered(_)));
        assert!(matches!(lt.need(40.0, None, Some(120.0), &sizes()), Need::NotCovered(_)));
    }

    #[test]
    fn one_length_range_and_one_size_a_row_and_sizes_down_by_voltage() {
        let t = table(&["LENGTH OF RUN", "WIRE SIZE"], &[&["0'-50'", "#12"], &["51'-100'", "#10"], &["101'-150'", "#8"]]);
        let lt = LengthTable { sheet: String::new(), title: String::new(), bands: bands_of(&t) };
        assert_eq!(lt.bands.len(), 3, "{:#?}", lt.bands);
        assert!(matches!(lt.need(75.0, None, None, &sizes()), Need::Size(s, _) if s == "10"));
        let t = table(&["CONDUCTOR", "120V", "277V"], &[&["#12", "50", "115"], &["#10", "80", "185"]]);
        let lt = LengthTable { sheet: String::new(), title: String::new(), bands: bands_of(&t) };
        assert_eq!(lt.bands.len(), 4, "{:#?}", lt.bands);
        assert!(matches!(lt.need(100.0, None, Some(277.0), &sizes()), Need::Size(s, _) if s == "12"));
        assert!(matches!(lt.need(70.0, None, Some(120.0), &sizes()), Need::Size(s, _) if s == "10"));
    }

    #[test]
    fn titles_sizes_and_lengths_read_as_printed() {
        assert!(is_title("BRANCH CIRCUIT VOLTAGE DROP WIRING SCHEDULE"));
        assert!(is_title("Wire Length Table"));
        assert!(!is_title("SEE VOLTAGE DROP SCHEDULE ON SHEET X."));
        assert!(!is_title("LIGHT FIXTURE SCHEDULE"));
        assert_eq!(size_in("#12 AWG", false).as_deref(), Some("12"));
        assert_eq!(size_in("10 AWG", false).as_deref(), Some("10"));
        assert_eq!(size_in("#1/0", false).as_deref(), Some("1/0"));
        assert_eq!(size_in("12", true).as_deref(), Some("12"));
        assert_eq!(size_in("120V", false), None);
        assert_eq!(length_in("50'"), Some(50.0));
        assert_eq!(length_in("51'-100'"), Some(100.0));
        assert_eq!(length_in("50'-6\""), Some(50.0));
        assert_eq!(length_in("UP TO 75 FT"), Some(75.0));
        assert_eq!(length_in("#12"), None);
        assert_eq!(length_in("SEE NOTE 4"), None);
        assert_eq!(amps_in("20A/1P", false), Some(20.0));
        assert_eq!(volts_in("208Y/120V", false), Some(120.0));
    }

    #[test]
    fn hostile_cells_do_not_panic() {
        let long = "9".repeat(5_000);
        let t = table(&["#", &long, "\u{2019}#'"], &[&[&long, "#", "''''"], &["", "", ""]]);
        let _ = bands_of(&t);
        let _ = length_in(&long);
        let _ = size_in("#////", false);
        let _ = numbers("1.2.3.4,,,5'");
    }
}
