//! A print's plumbing fixture schedule, read from its lettering: the table whose header row names
//! a tag column (`TAG`, `MARK`, `SYMBOL`) and a fixture column (`FIXTURE`, or `TYPE` / `ITEM` under
//! a `… FIXTURE SCHEDULE` title), with its maker and model columns in any order and under any of
//! their usual names (`MANUFACTURER`, `MFR`, `MAKE`; `MODEL NO.`, `CATALOG NO.`). Each row starts at
//! a tag in the tag column (`P-1`, `P-2H`, `WH-1,2,3`); its cells are the texts on the tag's
//! baseline under each header. A fixture's parts listed on the lines under it (a toilet's seat, a
//! shower's trim: `SEAT: MAKER` with its model) stay with it as its parts: its first line names it.
//! Columns giving connection sizes (`CW`, `HW`, `WASTE`, `VENT`, `TRAP`) or electrical data
//! (`VOLTS`, `PHASE`, `AMPS`, `HP`, `WATTS`) are kept per row. Two schedules side by side (a
//! two-sided sheet) are two tables: a header row ends at the next tag header.
//!
//! The rough-in table beside it ([`read_connections`]): a table keyed by fixture name (`LAVATORY`,
//! `WATER CLOSET-FLUSH TANK`) whose columns give each fixture's trap, waste, vent, cold and hot water
//! sizes, its headers on one or more lines (`COLD` over `WATER`, `WASTE` over `ABOVE GRADE` and
//! `BELOW GRADE`).

use std::collections::BTreeMap;

use crate::sheet_facts::TextItem;
use cadaddy_doc::{Drawing, EntityKind};

/// Most rows read from one schedule.
const MAX_ROWS: usize = 400;
/// The rows reach at most this many header heights below the header.
const MAX_DEPTH_H: f64 = 400.0;
/// Header cells lie this far right of the tag header at most, in its heights.
const MAX_WIDTH_H: f64 = 400.0;
/// An Arial-like letter's width, in text heights (a cell's middle from its start).
const CHAR_EM: f64 = 0.6;
/// How far the last column's cells reach right of its header's middle, in text heights.
const DESCRIPTION_H: f64 = 40.0;
/// The lines of a fixture's parts reach this many heights below its last row at most.
const LAST_ROW_PARTS_H: f64 = 12.0;
/// Words of one column header, at most (a longer text on the header's line is a note).
const MAX_HEADER_WORDS: usize = 5;
/// Rows of one rough-in table, at most.
const MAX_CONNECTION_ROWS: usize = 80;
/// A rough-in table's header lines lie within this many heights of its fixture header.
const CONNECTION_BAND_H: f64 = 2.5;
/// Its columns reach this far right of its fixture header, in heights.
const CONNECTION_WIDTH_H: f64 = 80.0;
/// Its rows reach this far below its header, in heights.
const CONNECTION_DEPTH_H: f64 = 60.0;

/// One part listed under a fixture's row: what part (`SEAT`, `FAUCET/DRAIN`, empty when the line
/// names none), its maker, model and description.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Part {
    pub part: String,
    pub manufacturer: String,
    pub model: String,
    pub description: String,
}

/// One fixture the schedule lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScheduledFixture {
    pub tag: String,
    /// `LAVATORY`, `KITCHEN SINK`, `ADA WATER CLOSET`.
    pub fixture: String,
    /// The maker, without the part it makes (`BOWL: SIGNATURE HARDWARE` → `SIGNATURE HARDWARE`).
    pub manufacturer: String,
    pub model: String,
    pub description: String,
    /// Its remarks column, its lines joined.
    pub remarks: String,
    /// The parts on the lines under its row.
    pub parts: Vec<Part>,
    /// Connection sizes its own columns give: `cw`, `hw`, `waste`, `vent`, `trap` → `1/2"`.
    pub connections: BTreeMap<String, String>,
    /// Electrical data its columns give: `volts`, `phase`, `amps`, `hp`, `watts`, `electrical`.
    pub electrical: BTreeMap<String, String>,
}

/// A plumbing fixture schedule: its rows, its extent `[x0, y0, x1, y1]` and its column headers
/// as read (left to right).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FixtureSchedule {
    pub rows: Vec<ScheduledFixture>,
    pub extent: [f64; 4],
    pub headers: Vec<String>,
}

/// One row of a rough-in table: the fixture it names and its connection sizes (`trap`, `waste`,
/// `waste_below`, `vent`, `cw`, `hw` → `1-1/2"`; a `--` cell is no connection and left out).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionRow {
    pub fixture: String,
    pub sizes: BTreeMap<String, String>,
}

/// What a schedule column holds, from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Col {
    Tag,
    /// The fixture's name: `strong` when its header says FIXTURE (`TYPE` and `ITEM` are weak).
    Fixture {
        strong: bool,
    },
    Maker,
    Model,
    Description,
    Remarks,
    Conn(&'static str),
    Elec(&'static str),
    Other,
}

/// A header's words: capitals, dots and `#` dropped, one space between.
fn header_words(h: &str) -> String {
    h.to_ascii_uppercase().replace(['.', '#', ':'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The column a header names.
fn col_of(header: &str) -> Col {
    let h = header_words(header);
    let first = h.split(' ').next().unwrap_or_default();
    let is = |ws: &[&str]| ws.contains(&h.as_str());
    let starts = |ws: &[&str]| ws.iter().any(|w| h == *w || h.starts_with(&format!("{w} ")));
    if is(&["TAG", "MARK", "SYMBOL", "TAG NO", "MARK NO", "FIXTURE TAG", "FIXTURE MARK"]) {
        Col::Tag
    } else if h.contains("FIXTURE") {
        Col::Fixture { strong: true }
    } else if is(&["TYPE", "ITEM", "FIXTURE TYPE"]) {
        Col::Fixture { strong: false }
    } else if starts(&["MANUFACTURER", "MANUFACTURERS", "MFR", "MFG", "MFGR", "MAKER", "MAKE", "BRAND", "MANUF"]) {
        Col::Maker
    } else if starts(&["MODEL", "CATALOG", "CAT", "PART NO", "PRODUCT", "SERIES"]) {
        Col::Model
    } else if starts(&["DESCRIPTION", "DESC"]) {
        Col::Description
    } else if starts(&["REMARKS", "NOTES", "NOTE", "COMMENTS"]) {
        Col::Remarks
    } else if is(&["CW", "C W", "COLD", "COLD WATER", "DCW"]) {
        Col::Conn("cw")
    } else if is(&["HW", "H W", "HOT", "HOT WATER", "DHW"]) {
        Col::Conn("hw")
    } else if is(&["WASTE", "W", "SAN", "SANITARY", "SOIL"]) {
        Col::Conn("waste")
    } else if is(&["VENT", "V"]) {
        Col::Conn("vent")
    } else if is(&["TRAP"]) {
        Col::Conn("trap")
    } else if starts(&["VOLTS", "VOLTAGE", "VOLT"]) {
        Col::Elec("volts")
    } else if is(&["PHASE", "PH"]) {
        Col::Elec("phase")
    } else if starts(&["AMPS", "AMP", "FLA", "MCA"]) {
        Col::Elec("amps")
    } else if is(&["HP"]) {
        Col::Elec("hp")
    } else if starts(&["WATTS", "WATT", "KW"]) {
        Col::Elec("watts")
    } else if starts(&["ELECTRICAL", "ELEC", "POWER"]) || first == "ELECTRICAL" {
        Col::Elec("electrical")
    } else {
        Col::Other
    }
}

/// Whether a cell says no value (`--`, `-`, `N/A`).
fn blank(v: &str) -> bool {
    let t = v.trim();
    t.is_empty() || t.chars().all(|c| c == '-' || c == '–') || t.eq_ignore_ascii_case("N/A") || t.eq_ignore_ascii_case("NA")
}

/// Whether a text is a pipe size cell (`1/2"`, `1-1/2"`, `3"`, `2 IN`) or a blank one (`--`).
pub fn is_size(t: &str) -> bool {
    let v = t.trim().trim_end_matches(['"', '”', '″']).trim_end_matches(" IN").trim();
    if blank(t) {
        return !t.trim().is_empty();
    }
    let mut parts = v.splitn(2, '-');
    let whole = parts.next().unwrap_or_default();
    let frac_ok = |f: &str| {
        let mut ab = f.splitn(2, '/');
        matches!((ab.next(), ab.next()), (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit()))
    };
    let num_ok =
        |n: &str| !n.is_empty() && n.len() <= 5 && n.chars().all(|c| c.is_ascii_digit() || c == '.') && n.chars().any(|c| c.is_ascii_digit());
    match parts.next() {
        Some(frac) => num_ok(whole) && frac_ok(frac),
        None => num_ok(whole) || frac_ok(whole),
    }
}

/// Whether a text is shaped like a schedule's tag: one to four capitals, then maybe a dash and a
/// number, a suffix letter, more numbers after commas (`EEW`, `P-1H`, `TP-1`, `WH-1,2,3`).
pub fn is_fixture_tag(t: &str) -> bool {
    let t = t.trim();
    let letters = t.chars().take_while(char::is_ascii_uppercase).count();
    (1..=4).contains(&letters)
        && t.len() <= 12
        && t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-' || c == ',')
        && !t.ends_with(['-', ','])
}

/// The tags a schedule tag stands for: itself, and each number of a list (`WH-1,2,3` → `WH-1`,
/// `WH-2`, `WH-3`).
pub fn tags_of(tag: &str) -> Vec<String> {
    let t = tag.trim();
    let mut out = vec![t.to_string()];
    if let Some((stem, list)) = t.rsplit_once('-')
        && list.contains(',')
    {
        for n in list.split(',').map(str::trim).filter(|n| !n.is_empty()).take(40) {
            out.push(format!("{stem}-{n}"));
        }
    }
    out
}

/// Most line pieces looked at for a schedule's column rules.
const MAX_SEGMENTS: usize = 200_000;

/// A sheet's vertical line pieces (x, y0, y1).
fn verticals(d: &Drawing) -> Vec<(f64, f64, f64)> {
    let mut verticals: Vec<(f64, f64, f64)> = Vec::new();
    let mut push = |a: [f64; 2], b: [f64; 2]| {
        if (a[0] - b[0]).abs() <= 1e-3 * (a[1] - b[1]).abs() && verticals.len() < MAX_SEGMENTS {
            verticals.push((a[0], a[1].min(b[1]), a[1].max(b[1])));
        }
    };
    for e in d.model.iter() {
        match &e.kind {
            EntityKind::Line(l) => push([l.a.x, l.a.y], [l.b.x, l.b.y]),
            EntityKind::LwPolyline(p) => {
                for w in p.vertices.windows(2) {
                    if let [a, b] = w {
                        push([a.p.x, a.p.y], [b.p.x, b.p.y]);
                    }
                }
            }
            _ => {}
        }
    }
    verticals
}

/// The plumbing fixture schedule on a sheet, if it has one: its lettering, in the columns its
/// vertical rules make.
pub fn read(d: &Drawing) -> Option<FixtureSchedule> {
    read_lettering(&crate::sheet_facts::texts(d), &verticals(d))
}

/// The rough-in table on a sheet ([`read_connections`]).
pub fn read_connections_on(d: &Drawing) -> Vec<ConnectionRow> {
    read_connections(&crate::sheet_facts::texts(d))
}

/// A cell's middle from where its text starts (an Arial-like letter's width).
fn middle_of(t: &TextItem) -> f64 {
    t.at.x + 0.5 * CHAR_EM * t.height * t.value.trim().chars().count() as f64
}

/// The plumbing fixture schedule among a sheet's texts, if it has one; `verticals` (x, y0, y1)
/// are the sheet's vertical line pieces: the ones crossing the header row are the columns'
/// rules (without them a column reaches halfway to the next header).
pub fn read_lettering(texts: &[TextItem], verticals: &[(f64, f64, f64)]) -> Option<FixtureSchedule> {
    let middle = middle_of;
    for tag_head in texts.iter().filter(|t| col_of(&t.value) == Col::Tag && t.height.is_finite() && t.height > 0.0) {
        let h = tag_head.height;
        let row_of = |t: &TextItem| (t.at.y - tag_head.at.y).abs() <= 0.5 * h && (t.at.x - tag_head.at.x).abs() <= MAX_WIDTH_H * h;
        let mut on_row: Vec<&TextItem> = texts.iter().filter(|t| row_of(t)).collect();
        on_row.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
        // Schedules side by side (a two-sided sheet): each starts at its tag header (the columns
        // left of the first one are the first's), so this one's row runs from its own tag header
        // (from the row's start when it is the first) to the next.
        let tag_xs: Vec<f64> = on_row.iter().filter(|t| col_of(&t.value) == Col::Tag).map(|t| t.at.x).collect();
        let lo = tag_xs.iter().copied().filter(|x| *x < tag_head.at.x - h).fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))));
        let lo = if lo.is_some() { tag_head.at.x - h } else { f64::NEG_INFINITY };
        let hi = tag_xs.iter().copied().filter(|x| *x > tag_head.at.x + h).fold(f64::INFINITY, f64::min) - h;
        // A header is a few words: a note lettered on the header's line is no column.
        on_row.retain(|t| t.at.x >= lo && t.at.x < hi && t.value.split_whitespace().count() <= MAX_HEADER_WORDS);
        let mut heads: Vec<(String, f64)> = on_row.iter().map(|t| (t.value.trim().to_string(), middle(t))).collect();
        let cols: Vec<Col> = heads.iter().map(|(v, _)| col_of(v)).collect();
        let strong = cols.contains(&Col::Fixture { strong: true });
        let weak = cols.contains(&Col::Fixture { strong: false });
        let maker_or_model = cols.iter().any(|c| matches!(c, Col::Maker | Col::Model));
        // A weak fixture column (`TYPE`) only under a fixture schedule's title.
        let titled = || {
            texts.iter().any(|t| {
                let u = t.value.to_ascii_uppercase();
                u.contains("FIXTURE") && u.contains("SCHEDULE") && t.at.y > tag_head.at.y && t.at.y - tag_head.at.y <= 8.0 * h.max(t.height)
            })
        };
        if !(strong || (weak && maker_or_model && titled())) {
            continue;
        }
        // The rules crossing the header row.
        let mut rules: Vec<f64> = verticals
            .iter()
            .filter(|(x, y0, y1)| {
                *y0 <= tag_head.at.y && *y1 >= tag_head.at.y && *x >= tag_head.at.x - 20.0 * h && *x <= tag_head.at.x + MAX_WIDTH_H * h
            })
            .map(|(x, _, _)| *x)
            .collect();
        rules.sort_by(f64::total_cmp);
        heads.sort_by(|a, b| a.1.total_cmp(&b.1));
        // Each column between the rules either side of its header (a cell's text starts there),
        // else from halfway to the header before it to halfway to the one after (the outer ones
        // as wide each side of their header; a cell's text centred there).
        let mut bounds: Vec<(String, f64, f64, bool)> = heads
            .iter()
            .enumerate()
            .map(|(i, (v, x))| {
                let ruled = (rules.iter().rev().find(|r| *r < x).copied(), rules.iter().find(|r| *r > x).copied());
                if let (Some(lo), Some(hi)) = ruled {
                    return (v.to_ascii_uppercase(), lo, hi, true);
                }
                let lo = i.checked_sub(1).and_then(|j| heads.get(j)).map(|p| (p.1 + x) / 2.0);
                let hi = heads.get(i + 1).map(|n| (n.1 + x) / 2.0);
                let (lo, hi) = match (lo, hi) {
                    (Some(lo), Some(hi)) => (lo, hi),
                    (None, Some(hi)) => (x - (hi - x), hi),
                    (Some(lo), None) => (lo, x + (x - lo)),
                    (None, None) => (f64::NEG_INFINITY, f64::INFINITY),
                };
                (v.to_ascii_uppercase(), lo, hi, false)
            })
            .collect();
        let inside = |t: &TextItem, (lo, hi, ruled): (f64, f64, bool)| {
            let x = if ruled { t.at.x } else { middle(t) };
            x >= lo && x < hi
        };
        let Some(tag_col) = bounds.iter().find(|b| col_of(&b.0) == Col::Tag).map(|(_, lo, hi, r)| (*lo, *hi, *r)) else { continue };
        // The tag column top down, to the first text in it that is no tag (the notes under it).
        let mut below: Vec<&TextItem> =
            texts.iter().filter(|t| inside(t, tag_col) && t.at.y < tag_head.at.y - 0.5 * h && t.at.y > tag_head.at.y - MAX_DEPTH_H * h).collect();
        below.sort_by(|a, b| b.at.y.total_cmp(&a.at.y));
        let stop_y = below.iter().find(|t| !is_fixture_tag(&t.value)).map(|t| t.at.y);
        let tags: Vec<&TextItem> = below.into_iter().take_while(|t| is_fixture_tag(&t.value)).take(MAX_ROWS).collect();
        // A header's second line (`MODEL` over `NO.`), between the header row and the first row:
        // its column's name goes on.
        let first_row = tags.first().map_or(tag_head.at.y - 2.0 * h, |t| t.at.y);
        for t in texts.iter().filter(|t| {
            let dy = tag_head.at.y - t.at.y;
            dy > 0.5 * h && dy <= 1.6 * h && t.at.y > first_row + 0.5 * h && (0.7 * h..=1.4 * h).contains(&t.height) && !is_fixture_tag(&t.value)
        }) {
            if let Some(b) = bounds.iter_mut().find(|b| inside(t, (b.1, b.2, b.3))) {
                b.0 = format!("{} {}", b.0, t.value.trim().to_ascii_uppercase());
            }
        }
        let headers: Vec<String> = bounds.iter().map(|b| b.0.clone()).collect();
        let column = |want: &dyn Fn(Col) -> bool| bounds.iter().find(|b| want(col_of(&b.0))).map(|(_, lo, hi, ruled)| (*lo, *hi, *ruled));
        let (x_lo, x_hi) = (
            bounds.first().map_or(tag_head.at.x - h, |b| b.1.max(tag_head.at.x - 20.0 * h)),
            bounds.last().map_or(tag_head.at.x, |b| b.2.min(tag_head.at.x + MAX_WIDTH_H * h)),
        );
        let cell_in = |col: Option<(f64, f64, bool)>, y: f64| -> String {
            let Some(col) = col else { return String::new() };
            let mut on: Vec<&TextItem> = texts.iter().filter(|t| (t.at.y - y).abs() <= 0.5 * h && inside(t, col)).collect();
            on.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
            on.iter().map(|t| t.value.trim()).collect::<Vec<_>>().join(" ")
        };
        let fixture_col = column(&|c| c == Col::Fixture { strong: true }).or_else(|| column(&|c| matches!(c, Col::Fixture { .. })));
        let maker_col = column(&|c| c == Col::Maker);
        let model_col = column(&|c| c == Col::Model);
        let desc_col = column(&|c| c == Col::Description);
        let remarks_col = column(&|c| c == Col::Remarks);
        let keyed: Vec<(Col, (f64, f64, bool))> = bounds
            .iter()
            .filter_map(|b| {
                let c = col_of(&b.0);
                matches!(c, Col::Conn(_) | Col::Elec(_)).then_some((c, (b.1, b.2, b.3)))
            })
            .collect();
        let maker_of = |m: &str| m.split_once(':').map_or(m, |(_, m)| m).trim().to_string();
        let mut rows: Vec<ScheduledFixture> = Vec::new();
        for (i, t) in tags.iter().enumerate() {
            let y = t.at.y;
            let maker = cell_in(maker_col, y);
            let mut row = ScheduledFixture {
                tag: t.value.trim().to_string(),
                fixture: cell_in(fixture_col, y),
                manufacturer: maker_of(&maker),
                model: cell_in(model_col, y),
                description: cell_in(desc_col, y),
                remarks: cell_in(remarks_col, y),
                ..ScheduledFixture::default()
            };
            for (c, col) in &keyed {
                let v = cell_in(Some(*col), y);
                if blank(&v) {
                    continue;
                }
                match c {
                    Col::Conn(k) => {
                        row.connections.insert((*k).to_string(), v);
                    }
                    Col::Elec(k) => {
                        row.electrical.insert((*k).to_string(), v);
                    }
                    _ => {}
                }
            }
            // The lines under it, to the next row (the last row's to the notes, or a few lines).
            let next_y = tags.get(i + 1).map(|n| n.at.y).or(stop_y).unwrap_or(y - LAST_ROW_PARTS_H * h).max(y - LAST_ROW_PARTS_H * h);
            let mut lines: Vec<f64> = texts
                .iter()
                .filter(|x| x.at.y < y - 0.5 * h && x.at.y > next_y + 0.5 * h && middle(x) >= x_lo && x.at.x <= x_hi)
                .map(|x| x.at.y)
                .collect();
            lines.sort_by(|a, b| b.total_cmp(a));
            lines.dedup_by(|a, b| (*a - *b).abs() <= 0.5 * h);
            for ly in lines.into_iter().take(40) {
                let (m, model, desc, rem) = (cell_in(maker_col, ly), cell_in(model_col, ly), cell_in(desc_col, ly), cell_in(remarks_col, ly));
                if !rem.is_empty() {
                    row.remarks = format!("{} {rem}", row.remarks).trim().to_string();
                }
                if !m.is_empty() || !model.is_empty() {
                    let part = m.split_once(':').map_or("", |(p, _)| p).trim().to_string();
                    row.parts.push(Part { part, manufacturer: maker_of(&m), model, description: desc });
                } else if !desc.is_empty() {
                    // A description's next line: the part's above it, else the row's.
                    match row.parts.last_mut() {
                        Some(p) => p.description = format!("{} {desc}", p.description).trim().to_string(),
                        None => row.description = format!("{} {desc}", row.description).trim().to_string(),
                    }
                }
            }
            if !row.fixture.is_empty() {
                rows.push(row);
            }
        }
        if rows.len() < 2 {
            continue;
        }
        let low = tags.last().map_or(tag_head.at.y, |t| t.at.y);
        let right = heads.last().map_or(tag_head.at.x, |(_, x)| *x);
        return Some(FixtureSchedule { rows, extent: [tag_head.at.x - h, low - h, right + DESCRIPTION_H * h, tag_head.at.y + 2.0 * h], headers });
    }
    None
}

/// The connection a rough-in header's words name (`COLD WATER` → `cw`, `WASTE BELOW GRADE` →
/// `waste_below`), if one.
fn connection_key(label: &str) -> Option<&'static str> {
    let w: Vec<String> = header_words(label).split(' ').map(str::to_string).collect();
    let has = |x: &str| w.iter().any(|y| y == x);
    Some(if has("TRAP") {
        "trap"
    } else if (has("WASTE") || has("SANITARY") || has("SOIL")) && has("BELOW") {
        "waste_below"
    } else if has("WASTE") || has("SANITARY") || has("SOIL") || has("DRAIN") {
        "waste"
    } else if has("VENT") {
        "vent"
    } else if has("COLD") || has("CW") || has("DCW") {
        "cw"
    } else if has("HOT") || has("HW") || has("DHW") {
        "hw"
    } else {
        return None;
    })
}

/// The rough-in tables among a sheet's texts: each table keyed by fixture name whose header
/// (one or more lines within a few heights of its `FIXTURE` header, a word stacked over the
/// columns it spans) names at least two connections; its rows the fixture names under it with
/// the size cells on their lines, each cell in the column whose header it is under. A table whose
/// header has a tag column is the fixture schedule, not this.
pub fn read_connections(texts: &[TextItem]) -> Vec<ConnectionRow> {
    let width = |t: &TextItem| CHAR_EM * t.height * t.value.trim().chars().count() as f64;
    let mut out: Vec<ConnectionRow> = Vec::new();
    let mut seen_anchor: Vec<(f64, f64)> = Vec::new();
    for a in texts.iter().filter(|t| {
        let u = header_words(&t.value);
        u.contains("FIXTURE") && u.split(' ').count() <= 4 && !u.contains("SCHEDULE") && t.height.is_finite() && t.height > 0.0
    }) {
        let h = a.height;
        if seen_anchor.iter().any(|(x, y)| (x - a.at.x).abs() < h && (y - a.at.y).abs() < h) {
            continue;
        }
        let band: Vec<&TextItem> = texts
            .iter()
            .filter(|t| {
                (t.at.y - a.at.y).abs() <= CONNECTION_BAND_H * h
                    && t.at.x >= a.at.x - 2.0 * h
                    && t.at.x <= a.at.x + CONNECTION_WIDTH_H * h
                    && (0.6 * h..=1.6 * h).contains(&t.height)
                    && !is_size(&t.value)
            })
            .collect();
        // A tag column on its header's line: the fixture schedule, not this.
        if texts
            .iter()
            .any(|t| (t.at.y - a.at.y).abs() <= CONNECTION_BAND_H * h && (t.at.x - a.at.x).abs() <= 40.0 * h && col_of(&t.value) == Col::Tag)
        {
            continue;
        }
        let range = |t: &TextItem| (t.at.x, t.at.x + width(t));
        let overlaps = |p: (f64, f64), q: (f64, f64)| p.0 < q.1 && q.0 < p.1;
        // The lowest text of each stack is its column; the texts over it name it, top down.
        let leaves: Vec<&TextItem> =
            band.iter().copied().filter(|t| !band.iter().any(|o| o.at.y < t.at.y - 0.3 * h && overlaps(range(o), range(t)))).collect();
        let mut columns: Vec<(Option<&'static str>, f64, f64, f64)> = leaves
            .iter()
            .map(|l| {
                let mut over: Vec<&&TextItem> = band.iter().filter(|o| overlaps(range(o), range(l))).collect();
                over.sort_by(|p, q| q.at.y.total_cmp(&p.at.y));
                let label = over.iter().map(|o| o.value.trim()).collect::<Vec<_>>().join(" ");
                let (x0, x1) = range(l);
                let key = if header_words(&label).contains("FIXTURE") { Some("fixture") } else { connection_key(&label) };
                (key, (x0 + x1) / 2.0, x0, x1)
            })
            .collect();
        columns.sort_by(|p, q| p.1.total_cmp(&q.1));
        let values: Vec<(&'static str, f64)> = columns.iter().filter_map(|c| c.0.filter(|k| *k != "fixture").map(|k| (k, c.1))).collect();
        let distinct: std::collections::BTreeSet<&str> = values.iter().map(|v| v.0).collect();
        if distinct.len() < 2 {
            continue;
        }
        seen_anchor.push((a.at.x, a.at.y));
        let spacing = values.windows(2).filter_map(|w| w.get(1).zip(w.first()).map(|(b, a)| b.1 - a.1)).fold(f64::INFINITY, f64::min);
        let spacing = if spacing.is_finite() && spacing > 0.0 { spacing } else { 6.0 * h };
        let first_x = values.first().map_or(a.at.x, |v| v.1) - 0.6 * spacing;
        let last_x = values.last().map_or(a.at.x, |v| v.1) + 0.6 * spacing;
        let bottom = band.iter().map(|t| t.at.y).fold(a.at.y, f64::min);
        // The fixture names under it, top down, left of its first size column.
        let mut names: Vec<&TextItem> = texts
            .iter()
            .filter(|t| {
                t.at.y < bottom - 0.5 * h
                    && t.at.y > bottom - CONNECTION_DEPTH_H * h
                    && t.at.x >= a.at.x - 12.0 * h
                    && t.at.x < first_x
                    && !is_size(&t.value)
                    && (0.6 * h..=1.6 * h).contains(&t.height)
            })
            .collect();
        names.sort_by(|p, q| q.at.y.total_cmp(&p.at.y).then(p.at.x.total_cmp(&q.at.x)));
        let mut lines: Vec<(f64, String)> = Vec::new();
        for n in names {
            match lines.last_mut() {
                Some((y, s)) if (*y - n.at.y).abs() <= 0.5 * h => {
                    s.push(' ');
                    s.push_str(n.value.trim());
                }
                _ => lines.push((n.at.y, n.value.trim().to_string())),
            }
        }
        let mut last_y = bottom;
        let mut pitch: Option<f64> = None;
        for (y, name) in lines.into_iter().take(MAX_CONNECTION_ROWS) {
            let gap = last_y - y;
            if pitch.is_some_and(|p| gap > 4.0 * p) {
                break;
            }
            let mut sizes: BTreeMap<String, String> = BTreeMap::new();
            for t in texts.iter().filter(|t| (t.at.y - y).abs() <= 0.5 * h && is_size(&t.value)) {
                let m = t.at.x + width(t) / 2.0;
                if m < first_x || m > last_x || blank(&t.value) {
                    continue;
                }
                if let Some((k, _)) =
                    values.iter().filter(|v| (v.1 - m).abs() <= 0.6 * spacing).min_by(|p, q| (p.1 - m).abs().total_cmp(&(q.1 - m).abs()))
                {
                    sizes.entry((*k).to_string()).or_insert_with(|| t.value.trim().to_string());
                }
            }
            if sizes.is_empty() {
                // A line with no size (a note under the table) ends it.
                if pitch.is_some() {
                    break;
                }
                continue;
            }
            if out.iter().any(|r| r.fixture == name) {
                continue;
            }
            if y < bottom - 0.5 * h {
                pitch = Some(pitch.map_or(gap, |p: f64| p.min(gap)));
            }
            last_y = y;
            out.push(ConnectionRow { fixture: name, sizes });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_geom::Vec2;

    fn t(value: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: value.into(), at: Vec2::new(x, y), height: 0.1, rotation: 0.0 }
    }

    #[test]
    fn a_schedules_rows_are_its_tags_with_the_cells_on_their_line() {
        let texts = [
            t("PLUMBING FIXTURE SCHEDULE", 12.0, 29.0),
            t("TAG", 11.46, 28.7),
            t("FIXTURE", 13.05, 28.7),
            t("MANUFACTURER", 15.6, 28.7),
            t("MODEL NO.", 18.5, 28.7),
            t("DESCRIPTION", 23.5, 28.7),
            t("P-1", 11.5, 25.95),
            t("WATER CLOSET", 12.7, 25.95),
            t("TANK/BOWL: PROFLO", 15.4, 25.95),
            t("PF6112WH/PF1501WH", 18.2, 25.95),
            t("1.28 GPF TWO-PIECE ELONGATED TOILET", 20.5, 25.95),
            // The toilet's seat, on the line under it.
            t("SEAT: PROFLO", 15.9, 25.75),
            t("PFTSHC2000W", 18.4, 25.75),
            t("P-2", 11.5, 24.9),
            t("LAVATORY", 12.9, 24.9),
            t("BOWL: SIGNATURE HARDWARE", 14.9, 24.9),
            t("447961", 18.7, 24.9),
            t("P-5", 11.5, 20.18),
            t("KITCHEN SINK", 12.7, 20.18),
            t("GENERAL NOTES:", 11.03, 11.94),
            t("P-9", 11.5, 11.0),
        ];
        let s = read_lettering(&texts, &[]).expect("schedule");
        let tags: Vec<&str> = s.rows.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(tags, ["P-1", "P-2", "P-5"], "the notes under it end it");
        let wc = &s.rows[0];
        assert_eq!((wc.fixture.as_str(), wc.manufacturer.as_str(), wc.model.as_str()), ("WATER CLOSET", "PROFLO", "PF6112WH/PF1501WH"));
        assert_eq!(s.rows[1].manufacturer, "SIGNATURE HARDWARE");
        // The seat on the line under the toilet is its part, not a row.
        assert_eq!(wc.parts, [Part { part: "SEAT".into(), manufacturer: "PROFLO".into(), model: "PFTSHC2000W".into(), description: String::new() }]);
        assert!(s.extent[1] < 20.18 && s.extent[3] > 28.7);
        // A header without a FIXTURE column (a door schedule's TAG) is no plumbing schedule.
        assert!(read_lettering(&[t("TAG", 0.0, 10.0), t("WIDTH", 2.0, 10.0), t("D1", 0.0, 9.0), t("D2", 0.0, 8.0)], &[]).is_none());
    }

    #[test]
    fn a_ruled_schedules_cells_are_between_its_rules() {
        // A maker set against its column's right rule, a description from its left one.
        let texts = [
            t("TAG", 11.46, 28.7),
            t("FIXTURE", 13.05, 28.7),
            t("MANUFACTURER", 15.6, 28.7),
            t("MODEL NO.", 18.5, 28.7),
            t("DESCRIPTION", 23.5, 28.7),
            t("ET-1", 11.47, 27.99),
            t("EXPANSION TANK", 12.78, 27.99),
            t("AMTROL", 17.33, 27.99),
            t("ST-42V", 18.67, 27.99),
            t("20 GALLON TANK VOLUME.", 19.9, 27.99),
            t("FD", 11.5, 26.96),
            t("FLOOR DRAIN", 12.9, 26.96),
            t("ZURN", 17.4, 26.96),
            t("ZN4153NHBP", 18.4, 26.96),
        ];
        let rules: Vec<(f64, f64, f64)> = [11.3, 12.18, 14.52, 17.93, 19.86, 28.08].iter().map(|x| (*x, 26.0, 28.86)).collect();
        let s = read_lettering(&texts, &rules).expect("schedule");
        let tank = &s.rows[0];
        assert_eq!((tank.manufacturer.as_str(), tank.model.as_str(), tank.description.as_str()), ("AMTROL", "ST-42V", "20 GALLON TANK VOLUME."));
    }

    #[test]
    fn fixture_tags_are_short_capitals_and_numbers() {
        for ok in ["P-1", "P-2H", "EEW", "WH-1,2,3", "TP-1"] {
            assert!(is_fixture_tag(ok), "{ok}");
        }
        for no in ["GENERAL NOTES:", "1.", "P-", "lav", "A VERY LONG TAG"] {
            assert!(!is_fixture_tag(no), "{no}");
        }
    }
    fn th(value: &str, x: f64, y: f64, h: f64) -> TextItem {
        TextItem { value: value.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
    }

    #[test]
    fn a_schedules_columns_go_by_their_names_in_any_order() {
        // Makers first, `MARK` for the tag, `MFR` and a two-line `CATALOG` / `NO.` header, the
        // connection sizes and the electrical data in columns of their own.
        let texts = [
            t("MFR", 10.0, 30.0),
            t("CATALOG", 12.0, 30.0),
            t("NO.", 12.1, 29.85),
            t("MARK", 14.0, 30.0),
            t("FIXTURE", 15.5, 30.0),
            t("CW", 18.0, 30.0),
            t("HW", 19.0, 30.0),
            t("W", 20.0, 30.0),
            t("V", 21.0, 30.0),
            t("VOLTS", 22.0, 30.0),
            t("REMARKS", 23.5, 30.0),
            t("FURNISH: THE TERM FURNISH IS USED TO MEAN SUPPLY", 26.0, 30.0),
            t("MAKER TWO", 9.8, 29.5),
            t("AB-100", 12.0, 29.5),
            t("WC-1", 14.0, 29.5),
            t("WATER CLOSET", 15.2, 29.5),
            t("1/2\"", 18.0, 29.5),
            t("--", 19.0, 29.5),
            t("3\"", 20.0, 29.5),
            t("2\"", 21.0, 29.5),
            t("PROVIDE SEAT", 23.4, 29.5),
            t("AND STOP.", 23.4, 29.35),
            t("MAKER THREE", 9.8, 29.0),
            t("CP-9", 12.0, 29.0),
            t("RP-1", 14.0, 29.0),
            t("CIRCULATING PUMP", 15.0, 29.0),
            t("120", 22.0, 29.0),
        ];
        let s = read_lettering(&texts, &[]).expect("schedule");
        assert_eq!(s.rows.len(), 2);
        let wc = &s.rows[0];
        assert_eq!((wc.tag.as_str(), wc.manufacturer.as_str(), wc.model.as_str()), ("WC-1", "MAKER TWO", "AB-100"));
        assert_eq!(wc.connections.get("cw").map(String::as_str), Some("1/2\""));
        assert_eq!(wc.connections.get("waste").map(String::as_str), Some("3\""));
        assert!(!wc.connections.contains_key("hw"), "a `--` cell is no connection");
        assert_eq!(wc.remarks, "PROVIDE SEAT AND STOP.", "the remarks' second line stays with its row");
        assert!(s.headers.iter().any(|h| h == "CATALOG NO."), "{:?}", s.headers);
        assert_eq!(s.headers.last().map(String::as_str), Some("REMARKS"), "a note on the header's line is no column: {:?}", s.headers);
        let pump = &s.rows[1];
        assert_eq!(pump.electrical.get("volts").map(String::as_str), Some("120"));
        assert_eq!(pump.model, "CP-9");
    }

    #[test]
    fn two_schedules_side_by_side_are_two_tables_and_a_type_column_needs_a_fixture_title() {
        // A two-sided sheet: the left table's header row ends at the right table's TAG.
        let texts = [
            t("TAG", 10.0, 20.0),
            t("FIXTURE", 11.0, 20.0),
            t("MANUFACTURER", 13.0, 20.0),
            t("TAG", 20.0, 20.0),
            t("FIXTURE", 21.0, 20.0),
            t("MANUFACTURER", 23.0, 20.0),
            t("L-1", 10.0, 19.5),
            t("LAVATORY", 10.9, 19.5),
            t("MAKER ONE", 12.9, 19.5),
            t("L-2", 10.0, 19.0),
            t("SINK", 10.9, 19.0),
            t("MAKER ONE", 12.9, 19.0),
            t("S-1", 20.0, 19.5),
            t("SHOWER", 20.9, 19.5),
            t("MAKER FOUR", 22.9, 19.5),
        ];
        let s = read_lettering(&texts, &[]).expect("schedule");
        let tags: Vec<&str> = s.rows.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(tags, ["L-1", "L-2"]);
        assert_eq!(s.headers.len(), 3, "the right table's columns are not this one's: {:?}", s.headers);
        // `TYPE` names the fixture only under a fixture schedule's title.
        let typed = |title: &str| {
            let mut v = vec![t("MARK", 0.0, 10.0), t("TYPE", 1.5, 10.0), t("MAKE", 4.0, 10.0)];
            v.push(t(title, 0.0, 10.4));
            v.extend([t("A-1", 0.0, 9.5), t("FLOOR DRAIN", 1.4, 9.5), t("MAKER", 4.0, 9.5), t("A-2", 0.0, 9.0), t("CLEANOUT", 1.4, 9.0)]);
            read_lettering(&v, &[])
        };
        assert_eq!(typed("PLUMBING FIXTURE SCHEDULE").map(|s| s.rows.len()), Some(2));
        assert!(typed("DOOR SCHEDULE").is_none(), "a door schedule's MARK and TYPE are no fixture schedule");
    }

    #[test]
    fn a_rough_in_tables_columns_take_their_stacked_headers() {
        // `WASTE` over `ABOVE GRADE` and `BELOW GRADE`, `COLD` over `WATER`: drawn y up.
        let h = 9.5;
        let texts = [
            th("WASTE", 2275.4, 400.0, h),
            th("COLD", 2384.6, 392.0, h),
            th("HOT", 2432.6, 392.0, h),
            th("PLUMBING FIXTURE", 2040.7, 386.6, h),
            th("TRAP", 2212.2, 386.6, h),
            th("VENT", 2344.3, 386.6, h),
            th("ABOVE", 2253.5, 383.8, h),
            th("BELOW", 2296.8, 383.8, h),
            th("WATER", 2380.8, 381.4, h),
            th("WATER", 2425.7, 381.4, h),
            th("GRADE", 2253.0, 373.3, h),
            th("GRADE", 2297.2, 373.3, h),
            th("WATER CLOSET-FLUSH TANK", 2019.0, 354.4, h),
            th("--", 2221.7, 354.4, h),
            th("3\"", 2265.4, 354.4, h),
            th("3\"", 2309.5, 354.4, h),
            th("2\"", 2352.6, 354.4, h),
            th("1/2\"", 2389.6, 354.4, h),
            th("--", 2439.5, 354.4, h),
            th("LAVATORY", 2061.6, 333.7, h),
            th("1-1/2\"", 2212.4, 333.7, h),
            th("1-1/2\"", 2257.2, 333.7, h),
            th("2\"", 2309.5, 333.7, h),
            th("1-1/2\"", 2344.4, 333.7, h),
            th("1/2\"", 2389.6, 333.7, h),
            th("1/2\"", 2434.4, 333.7, h),
            th("GENERAL NOTES:", 2019.0, 200.0, h),
        ];
        let rows = read_connections(&texts);
        assert_eq!(rows.len(), 2, "{rows:?}");
        let wc = &rows[0];
        assert_eq!(wc.fixture, "WATER CLOSET-FLUSH TANK");
        assert_eq!(wc.sizes.get("waste").map(String::as_str), Some("3\""));
        assert_eq!(wc.sizes.get("vent").map(String::as_str), Some("2\""));
        assert_eq!(wc.sizes.get("cw").map(String::as_str), Some("1/2\""));
        assert!(!wc.sizes.contains_key("trap") && !wc.sizes.contains_key("hw"), "`--` cells: {wc:?}");
        let lav = &rows[1];
        assert_eq!(lav.sizes.get("trap").map(String::as_str), Some("1-1/2\""));
        assert_eq!(lav.sizes.get("waste_below").map(String::as_str), Some("2\""));
        assert_eq!(lav.sizes.get("hw").map(String::as_str), Some("1/2\""));
        // The fixture schedule's header (a tag column) is not a rough-in table.
        let sched = [t("TAG", 0.0, 10.0), t("FIXTURE", 1.0, 10.0), t("CW", 3.0, 10.0), t("HW", 4.0, 10.0), t("P-1", 0.0, 9.5), t("1/2\"", 3.0, 9.5)];
        assert!(read_connections(&sched).is_empty());
    }

    #[test]
    fn sizes_and_tag_lists_read() {
        for ok in ["1/2\"", "1-1/2\"", "3\"", "2", "--", "3/4"] {
            assert!(is_size(ok), "{ok}");
        }
        for no in ["P-1", "LAVATORY", "1/2 TURN STOP", ""] {
            assert!(!is_size(no), "{no}");
        }
        assert_eq!(tags_of("WH-1,2,3"), ["WH-1,2,3", "WH-1", "WH-2", "WH-3"]);
        assert_eq!(tags_of("P-1H"), ["P-1H"]);
    }
}
