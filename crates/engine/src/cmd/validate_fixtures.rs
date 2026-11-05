//! E-LUM-SCHEDULE (Q6179): Validate's own reading of the light fixture schedule and the lighting
//! submittals, against the model and against the build's lighting RFIs.
//!
//! The schedule is read again from the sheets, never from what the build stored
//! (`mep.fixture_schedule`, the luminaires' schedule properties): each titled table (the titles in
//! `validate_fixtures.json`) is read by its drawn grid ([`super::gridtable`], the one reader both
//! share), on the sheets the drawing holds and on the pages of the job's PDFs it doesn't, and its
//! columns are named by their headers' words (any order; a schedule drawn in two halves side by
//! side, each with its own header, is read half by half). The lighting submittals are read again
//! from the job's submittal folders (the files whose names say lighting, every revision; per type
//! the latest one not sent back wins).
//!
//! Checks (`measured.check`):
//! - **row** (each luminaire): its type has a schedule row, and its basis of design, lamp, VA,
//!   voltage and installation are the row's. A field that differs fails, citing the row; a type
//!   with no row, or a field the model doesn't record, is `unsure`, never a pass.
//! - **scheduled** (each type on the plans): it has a schedule row.
//! - **on_plan** (each scheduled type, building scope): some plan or the model shows it.
//! - **submittal** (each scheduled type): the approved lighting submittal's product for it is the
//!   schedule's basis of design: the same maker (brands of one maker are one, `lightbrands.json`)
//!   and the same series (one catalog number with other options is one product).
//! - **submitted** (each submitted type, building scope): the schedule has a row for it.
//! - **fields** (each scheduled type): the schedule's voltage and VA against the submittal's.
//! - **quantity** (each scheduled type with a quantity, building scope): the model's count.
//!
//! The build's lighting RFIs about fixture types are matched afterwards, as E-EQUIP-ELEC does: an
//! RFI whose type and check Validate fails is confirmed (its id on the failure); one Validate
//! doesn't fail is `extra`, with Validate's verdict and working; a type-level failure the build
//! raised no RFI for is `missing`. An RFI about a field Validate doesn't recompute (CCT, lumens,
//! dimming…) is listed `unsure` with its sides. A luminaire's own mismatch with the schedule is the
//! model's error, not the documents', so no RFI is expected for it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use cadaddy_doc::props::Props;
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::validate::Prints;
use serde::Deserialize;
use serde_json::{Value, json};

use super::gridtable::{self, GridTable};
use super::validate_lighting::Lit;
use super::validate_rules::Effective;

pub const RULE: &str = "E-LUM-SCHEDULE";
/// The verdicts the rule gives: `missing` a failure the build raised no RFI for, `extra` an RFI
/// of the build's with no failure behind it.
pub const VERDICTS: [&str; 5] = ["pass", "fail", "unsure", "missing", "extra"];
/// The checks, in the order they are reported.
pub const CHECKS: [&str; 7] = ["row", "scheduled", "on_plan", "submittal", "submitted", "fields", "quantity"];

/// Texts and line segments looked at round one schedule's title, and titles looked at.
const MAX_WINDOW_ITEMS: usize = 60_000;
const MAX_TITLES: usize = 60;
/// Entities looked through for titles, rows kept per table, fixture types kept.
const MAX_ENTITIES: usize = 4_000_000;
const MAX_TYPES: usize = 2_000;
/// A PDF read for its pages, at most this big.
const MAX_PDF_BYTES: usize = 200 * 1024 * 1024;
/// Folder depth searched under a submittals folder, files looked through.
const MAX_DEPTH: usize = 6;
const MAX_FILES_LOOKED: usize = 5_000;
/// RFIs looked at, luminaires listed per type, characters of a quote.
const MAX_RFIS: usize = 100_000;
const MAX_LISTED: usize = 30;
const QUOTE_CHARS: usize = 200;
/// A type tag is short (`A`, `F1`, `XA`, `LA-2`); so is a column's header.
const MAX_TAG: usize = 8;
const MAX_HEADER: usize = 40;
/// The columns that make a table a fixture schedule (beside its tag column).
const FIXTURE_FIELDS: [&str; 7] = ["manufacturer", "model", "lamp", "voltage", "va", "installation", "description"];

// ---------------------------------------------------------------- the reading data

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Column {
    field: String,
    words: Vec<String>,
    not: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Data {
    source: String,
    titles: Vec<String>,
    not_titles: Vec<String>,
    columns: Vec<Column>,
    page_words: Vec<String>,
    page_disciplines: Vec<String>,
    max_pages: usize,
    submittal_words: Vec<String>,
    submittal_sections: Vec<String>,
    max_submittal_files: usize,
    series_chars: usize,
    maker_cut: Vec<String>,
}

const DATA: &str = include_str!("validate_fixtures.json");

fn data() -> &'static Data {
    static D: OnceLock<Data> = OnceLock::new();
    D.get_or_init(|| {
        let mut d: Data = serde_json::from_str(DATA).unwrap_or_default();
        d.max_pages = d.max_pages.clamp(1, 400);
        d.max_submittal_files = d.max_submittal_files.clamp(1, 200);
        d.series_chars = d.series_chars.clamp(3, 20);
        d
    })
}

/// Lighting brands by the maker that owns them (`lightbrands.json`).
fn families() -> &'static BTreeMap<String, Vec<String>> {
    static F: OnceLock<BTreeMap<String, Vec<String>>> = OnceLock::new();
    F.get_or_init(|| {
        let v: Value = serde_json::from_str(include_str!("lightbrands.json")).unwrap_or(Value::Null);
        v.get("families")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .map(|(k, list)| {
                        (k.clone(), list.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_ascii_uppercase).collect())
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

// ---------------------------------------------------------------- words and values

fn alnum(v: &str) -> String {
    v.chars().filter(char::is_ascii_alphanumeric).take(400).collect::<String>().to_ascii_uppercase()
}

fn words_of(v: &str) -> Vec<String> {
    v.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(200).map(str::to_string).collect()
}

fn quote(t: &str) -> String {
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > QUOTE_CHARS { format!("{}…", t.chars().take(QUOTE_CHARS).collect::<String>()) } else { t }
}

/// The numbers in a text (`120/277V` → 120, 277), at most twenty.
fn numbers(text: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars().take(2_000).chain(std::iter::once(' ')) {
        if c.is_ascii_digit() || (c == '.' && !cur.is_empty()) {
            cur.push(c);
        } else if !cur.is_empty() {
            if let Ok(n) = cur.trim_end_matches('.').parse::<f64>()
                && n.is_finite()
            {
                out.push(n);
            }
            cur.clear();
            if out.len() >= 20 {
                break;
            }
        }
    }
    out
}

/// The voltages a text allows (`120/277V`, `MVOLT`, `UNV` = 120 and 277).
fn volts(text: &str) -> BTreeSet<u32> {
    let mut out: BTreeSet<u32> = numbers(text).into_iter().filter(|n| (90.0..=600.0).contains(n)).map(|n| n.round() as u32).collect();
    if words_of(text).iter().any(|w| matches!(w.as_str(), "MVOLT" | "UNV" | "UNIVERSAL" | "UNIV" | "UVOLT")) {
        out.extend([120, 277]);
    }
    out
}

/// A load in a text: the number before `VA` (else before `W`), or the text's only number.
fn va_of(text: &str) -> Option<f64> {
    let up = text.to_ascii_uppercase();
    for unit in ["VA", "W"] {
        let mut num = String::new();
        let chars: Vec<char> = up.chars().take(400).collect();
        for (i, c) in chars.iter().enumerate() {
            if c.is_ascii_digit() || (*c == '.' && !num.is_empty()) {
                num.push(*c);
                continue;
            }
            if !num.is_empty() {
                let rest: String = chars.iter().skip(i).filter(|c| **c != ' ').take(unit.len() + 1).collect();
                let after = rest.chars().nth(unit.len());
                if rest.starts_with(unit)
                    && after.is_none_or(|a| !a.is_ascii_alphabetic())
                    && let Ok(v) = num.trim_end_matches('.').parse::<f64>()
                    && v.is_finite()
                    && v > 0.0
                    && v < 100_000.0
                {
                    return Some(v);
                }
            }
            num.clear();
        }
    }
    let n = numbers(text);
    match n.as_slice() {
        [v] if *v > 0.0 && *v < 100_000.0 && text.trim().chars().all(|c| c.is_ascii_digit() || c == '.' || c == ' ') => Some(*v),
        _ => None,
    }
}

/// A mounting's classes by its words: recessed, pendant, wall, pole, track, surface, ceiling.
fn mountings(text: &str) -> BTreeSet<&'static str> {
    let w = words_of(text);
    let has = |x: &[&str]| w.iter().any(|t| x.contains(&t.as_str()));
    [
        ("recessed", &["RECESSED", "RECESS", "TROFFER", "FLUSH"][..]),
        ("pendant", &["PENDANT", "SUSPENDED", "STEM", "CHAIN", "CABLE", "HUNG"]),
        ("wall", &["WALL", "SCONCE", "BRACKET", "VANITY"]),
        ("pole", &["POLE", "BOLLARD", "GROUND", "LANDSCAPE"]),
        ("track", &["TRACK"]),
        ("surface", &["SURFACE"]),
        ("ceiling", &["CEILING"]),
    ]
    .into_iter()
    .filter(|(_, words)| has(words))
    .map(|(c, _)| c)
    .collect()
}

/// Do two mountings agree? Classes in common, or one that leaves the other open: a bare
/// `SURFACE` is on a wall or a ceiling, a bare `CEILING` surface, recessed or hung. None when
/// either names no class.
fn mounting_agrees(a: &str, b: &str) -> Option<bool> {
    let (x, y) = (mountings(a), mountings(b));
    if x.is_empty() || y.is_empty() {
        return None;
    }
    let open = |p: &BTreeSet<&str>, q: &BTreeSet<&str>| {
        let only = |c: &str| p.len() == 1 && p.contains(c);
        (only("surface") && (q.contains("wall") || q.contains("ceiling")))
            || (only("ceiling") && (q.contains("surface") || q.contains("recessed") || q.contains("pendant")))
    };
    Some(!x.is_disjoint(&y) || open(&x, &y) || open(&y, &x))
}

/// The maker family a name belongs to (`lightbrands.json`), if the data knows it.
fn family(name: &str) -> Option<&'static str> {
    let up = format!(" {} ", words_of(name).join(" "));
    families().iter().find(|(_, brands)| brands.iter().any(|b| up.contains(&format!(" {} ", words_of(b).join(" "))))).map(|(f, _)| f.as_str())
}

/// Two makers are one: brands of one maker, else a word of four letters or more (not one every
/// maker's name may carry) in both, else one's letters hold the other's.
fn same_maker(a: &str, b: &str) -> bool {
    if let (Some(fa), Some(fb)) = (family(a), family(b)) {
        return fa == fb;
    }
    const GENERIC: [&str; 11] =
        ["LIGHTING", "LIGHT", "LIGHTS", "ELECTRIC", "SOLUTIONS", "PRODUCTS", "COMPANY", "CORP", "GROUP", "INTERNATIONAL", "INDUSTRIES"];
    let sig = |v: &str| words_of(v).into_iter().filter(|w| w.len() >= 4 && !GENERIC.contains(&w.as_str())).collect::<Vec<_>>();
    let (wa, wb) = (sig(a), sig(b));
    let (na, nb) = (alnum(a), alnum(b));
    wa.iter().any(|w| wb.contains(w)) || (na.len() >= 3 && nb.len() >= 3 && (na.contains(&nb) || nb.contains(&na)))
}

/// A catalog number's series: its leading pieces up to three letters and digits or more
/// (`AT-24-L40` → `AT24`, `QX4DM10FSL9-TL40` → `QX4DM10FSL9`).
fn series(v: &str) -> String {
    let mut out = String::new();
    for piece in v.split([' ', '-', '/', ',', '(', ')']).map(alnum).filter(|p| !p.is_empty()).take(8) {
        out.push_str(&piece);
        if out.len() >= 3 {
            break;
        }
    }
    if out.len() >= 3 { out } else { String::new() }
}

/// One product: the same series (a common start of `series_chars`, or one's series inside the
/// other) with whatever options. None when either says no catalog number.
fn same_series(a: &str, b: &str) -> Option<bool> {
    let (x, y) = (alnum(a), alnum(b));
    if x.len() < 3 || y.len() < 3 {
        return None;
    }
    let common = x.chars().zip(y.chars()).take_while(|(p, q)| p == q).count();
    let (sx, sy) = (series(a), series(b));
    Some(
        common >= data().series_chars
            || x.contains(&y)
            || y.contains(&x)
            || (!sx.is_empty() && y.contains(&sx))
            || (!sy.is_empty() && x.contains(&sy)),
    )
}

// ---------------------------------------------------------------- the schedule, re-read

/// One fixture type's schedule row as Validate read it.
#[derive(Clone, Debug, Default)]
pub(crate) struct SchedRow {
    pub tag: String,
    pub sheet: String,
    pub title: String,
    pub maker: String,
    pub model: String,
    pub lamp: String,
    pub va: Option<f64>,
    pub voltage: String,
    pub installation: String,
    pub description: String,
    pub quantity: Option<u32>,
    /// Every cell, by its column's header.
    pub cells: Vec<(String, String)>,
}

impl SchedRow {
    fn source(&self) -> String {
        format!("{} {}, type {}", self.sheet, self.title, self.tag)
    }
    /// How many of its fields it gives.
    fn filled(&self) -> usize {
        [&self.maker, &self.model, &self.lamp, &self.voltage, &self.installation, &self.description].iter().filter(|v| !v.trim().is_empty()).count()
            + usize::from(self.va.is_some())
    }
    fn basis(&self) -> String {
        format!("{} {}", self.maker, self.model).trim().to_string()
    }
    fn to_json(&self) -> Value {
        json!({
            "source": self.source(), "manufacturer": self.maker, "model": self.model, "lamp": self.lamp, "va": self.va,
            "voltage": self.voltage, "installation": self.installation, "description": self.description, "quantity": self.quantity,
            "cells": self.cells.iter().map(|(h, v)| json!([h, v])).collect::<Vec<_>>(),
        })
    }
}

/// The columns of one side of a schedule: field → column.
type Side = BTreeMap<String, usize>;

/// Each header cell's field (the first column definition its words name and whose `not` words
/// they don't), and the sides: a new side starts at each tag column after the first.
fn sides_of(headers: &[String]) -> Vec<Side> {
    let cols = &data().columns;
    let field_of = |h: &str| -> Option<&str> {
        let w = words_of(h);
        cols.iter().find(|c| c.words.iter().any(|x| w.contains(x)) && !c.not.iter().any(|x| w.contains(x))).map(|c| c.field.as_str())
    };
    // A header is short: a long cell is a note or lettering the grid caught, not a column's name.
    let named: Vec<Option<&str>> = headers.iter().map(|h| if h.trim().len() > MAX_HEADER { None } else { field_of(h) }).collect();
    let tags: Vec<usize> = named.iter().enumerate().filter(|(_, f)| **f == Some("tag")).map(|(i, _)| i).collect();
    let Some(first) = tags.first().copied() else { return Vec::new() };
    let mut out = Vec::new();
    for (k, start) in tags.iter().enumerate() {
        let lo = if k == 0 { 0 } else { *start };
        let hi = tags.get(k + 1).copied().unwrap_or(headers.len());
        let mut side = Side::new();
        side.insert("tag".into(), *start);
        for (i, f) in named.iter().enumerate().take(hi).skip(lo) {
            if let Some(f) = f
                && (*f != "tag" || i == *start)
            {
                side.entry((*f).to_string()).or_insert(i);
            }
        }
        // A column before the first tag column belongs to the first side only; a side names its
        // tag and at least two of a fixture's columns, else it is no fixture schedule's.
        let fixture_columns = side.keys().filter(|f| FIXTURE_FIELDS.contains(&f.as_str())).count();
        if (k > 0 && *start < first) || fixture_columns < 2 {
            continue;
        }
        out.push(side);
    }
    out
}

/// A maker's cell without the lamp line some schedules print under the maker (`ACME LAMP: X`).
fn maker_of(cell: &str) -> String {
    let up = cell.to_ascii_uppercase();
    let cut = data().maker_cut.iter().filter_map(|w| up.find(&format!(" {}", w.to_ascii_uppercase()))).min();
    cut.and_then(|i| cell.get(..i)).unwrap_or(cell).trim().to_string()
}

/// Is a cell a type tag (short, letters and digits, maybe a dash or dot; not a header word)?
fn tag_cell(v: &str) -> Option<String> {
    let t = v.trim().to_ascii_uppercase();
    let ok = !t.is_empty()
        && t.len() <= MAX_TAG
        && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && t.chars().any(|c| c.is_ascii_alphabetic())
        && !data().columns.iter().any(|c| c.words.contains(&t));
    ok.then_some(t)
}

/// The rows of one table read by its grid: each side's fixture types, and whether its notes
/// forbid substitutions (the note's words).
fn rows_of(t: &GridTable, title: &str) -> (Vec<SchedRow>, Option<String>) {
    let sides = sides_of(&t.headers);
    let mut out: Vec<SchedRow> = Vec::new();
    let mut no_subs = None;
    // The last type each side read: a row with no tag continues it (a cell's second line read as
    // its own row where the table has no row lines).
    let mut last: Vec<Option<usize>> = vec![None; sides.len()];
    for r in t.rows.iter().take(500) {
        let all = r.join(" ").to_ascii_uppercase();
        if let Some(i) = all.find("NO SUBSTITU")
            && no_subs.is_none()
        {
            // The note round it: from the sentence before it.
            let start = all.get(..i).and_then(|b| b.rfind(". ")).map_or(0, |j| j + 2);
            no_subs = all.get(start..).map(quote);
        }
        let note = r.iter().any(|c| {
            let u = c.trim_start().to_ascii_uppercase();
            u.starts_with("NOTE") || c.len() > 80
        });
        for (k, side) in sides.iter().enumerate() {
            let cell = |f: &str| side.get(f).and_then(|c| r.get(*c)).map_or("", |v| v.trim());
            let tag = side.get("tag").and_then(|c| r.get(*c)).map_or("", |v| v.trim());
            match tag_cell(tag) {
                Some(tg) => {
                    let mut row = SchedRow {
                        tag: tg,
                        sheet: t.sheet.clone(),
                        title: title.to_string(),
                        maker: maker_of(cell("manufacturer")),
                        model: cell("model").to_string(),
                        lamp: cell("lamp").to_string(),
                        voltage: cell("voltage").to_string(),
                        installation: cell("installation").to_string(),
                        description: cell("description").to_string(),
                        quantity: cell("quantity").trim().parse::<u32>().ok().filter(|q| *q > 0 && *q < 1_000_000),
                        ..SchedRow::default()
                    };
                    row.va = va_of(cell("va")).or_else(|| va_of(&row.lamp));
                    // In the order the columns are drawn.
                    let mut cols: Vec<usize> = side.values().copied().collect();
                    cols.sort_unstable();
                    row.cells = cols
                        .into_iter()
                        .filter_map(|c| Some((t.headers.get(c)?.trim().to_string(), r.get(c)?.trim().to_string())))
                        .filter(|(_, v)| !v.is_empty())
                        .map(|(h, v)| (if h.is_empty() { "(no header)".into() } else { h }, v))
                        .take(24)
                        .collect();
                    if let Some(l) = last.get_mut(k) {
                        *l = Some(out.len());
                    }
                    out.push(row);
                }
                None if tag.is_empty() && !note => {
                    // A continuation: its cells add to the side's last type.
                    let Some(Some(i)) = last.get(k).copied() else { continue };
                    let Some(row) = out.get_mut(i) else { continue };
                    let add = |s: &mut String, v: &str| {
                        if !v.is_empty() && s.len() < 400 {
                            if !s.is_empty() {
                                s.push(' ');
                            }
                            s.push_str(v);
                        }
                    };
                    add(&mut row.maker, cell("manufacturer"));
                    add(&mut row.model, cell("model"));
                    add(&mut row.lamp, cell("lamp"));
                    add(&mut row.voltage, cell("voltage"));
                    add(&mut row.installation, cell("installation"));
                    add(&mut row.description, cell("description"));
                    if row.va.is_none() {
                        row.va = va_of(cell("va")).or_else(|| va_of(&row.lamp));
                    }
                }
                None => {
                    if let Some(l) = last.get_mut(k) {
                        *l = None;
                    }
                }
            }
        }
    }
    (out, no_subs)
}

/// What Validate read of the lighting documents.
#[derive(Clone, Debug, Default)]
pub(crate) struct FixtureDocs {
    /// The schedule's rows by type (the first table naming a type wins).
    pub rows: BTreeMap<String, SchedRow>,
    /// Each table read: sheet, title, headers, sides, types.
    pub tables: Vec<Value>,
    /// The schedule's no-substitutions note, when it has one.
    pub no_substitutions: Option<String>,
    /// The pages of the job's PDFs read for their schedules.
    pub pages_read: Vec<String>,
    /// The lighting submittals' products by type (the latest revision not sent back).
    pub products: BTreeMap<String, SubProduct>,
    /// Each submittal file read: its types, revision and why not, when it couldn't be.
    pub files: Vec<Value>,
    /// What couldn't be read, by why.
    pub unmeasured: BTreeMap<String, u64>,
}

/// One type's product in a lighting submittal.
#[derive(Clone, Debug, Default)]
pub(crate) struct SubProduct {
    pub maker: String,
    pub catalog: String,
    pub voltage: String,
    pub wattage: String,
    /// `<file> p.<n>`.
    pub cite: String,
    pub revision: (u32, u32),
    /// Its review stamp: approved, approved_as_noted, revise, rejected, unreviewed (empty: not read).
    pub status: String,
    /// The other revisions that give the type (superseded or sent back): `<file> (<status>)`.
    pub others: Vec<String>,
}

impl SubProduct {
    fn approved(&self) -> Option<bool> {
        match self.status.as_str() {
            "approved" | "approved_as_noted" => Some(true),
            "revise" | "rejected" => Some(false),
            _ => None,
        }
    }
    fn rank(&self) -> (u8, (u32, u32)) {
        (
            match self.approved() {
                Some(true) => 2,
                None => 1,
                Some(false) => 0,
            },
            self.revision,
        )
    }
    fn words(&self) -> String {
        let w = format!("{} {}", self.maker, self.catalog).trim().to_string();
        if w.is_empty() { "a product with no maker or catalog number read".into() } else { w }
    }
    fn to_json(&self) -> Value {
        json!({
            "manufacturer": self.maker, "catalog": self.catalog, "voltage": self.voltage, "wattage": self.wattage, "cite": self.cite,
            "revision": format!("{}-{}", self.revision.0, self.revision.1),
            "status": if self.status.is_empty() { Value::Null } else { json!(self.status) }, "others": self.others,
        })
    }
}

fn norm_rect(r: [f64; 4]) -> [f64; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

fn is_title(v: &str) -> Option<&'static str> {
    let up = v.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
    if up.len() > 120 || data().not_titles.iter().any(|n| up.contains(&n.to_ascii_uppercase())) {
        return None;
    }
    data().titles.iter().find(|t| up.contains(&t.to_ascii_uppercase())).map(String::as_str)
}

/// The tables titled as the data says among `texts` and `segs` (one sheet's, or the window round
/// a title), each read into rows.
fn read_tables(texts: &[TextItem], segs: &[(Vec2, Vec2)], sheet: &str, docs: &mut FixtureDocs) {
    let mut seen: BTreeSet<String> = docs.tables.iter().filter_map(|t| t["key"].as_str().map(str::to_string)).collect();
    for title in &data().titles {
        for tb in gridtable::read_titled(texts, segs, title, sheet) {
            let key = format!("{sheet}|{}|{}", tb.headers.join("|"), tb.rows.first().map(|r| r.join("|")).unwrap_or_default());
            if !seen.insert(key.clone()) {
                continue;
            }
            let (rows, no_subs) = rows_of(&tb, title);
            if docs.no_substitutions.is_none() {
                docs.no_substitutions = no_subs;
            }
            let sides = sides_of(&tb.headers).len();
            let types: Vec<String> = rows.iter().map(|r| r.tag.clone()).collect();
            docs.tables.push(json!({"key": key, "sheet": sheet, "title": title, "headers": tb.headers, "sides": sides, "types": types}));
            // A type two tables name: the row that gives more of its fields.
            for r in rows {
                match docs.rows.get(&r.tag) {
                    Some(old) if old.filled() >= r.filled() => {}
                    Some(_) => {
                        docs.rows.insert(r.tag.clone(), r);
                    }
                    None if docs.rows.len() < MAX_TYPES => {
                        docs.rows.insert(r.tag.clone(), r);
                    }
                    None => {}
                }
            }
        }
    }
}

/// A drawing's horizontal text and straight segments inside `rect` (all of them when none).
fn page_items(d: &Drawing, rect: Option<[f64; 4]>) -> (Vec<TextItem>, Vec<(Vec2, Vec2)>) {
    let mut texts = Vec::new();
    let mut segs = Vec::new();
    let inside = |p: Vec2| rect.is_none_or(|r| p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3]);
    for e in d.model.iter().take(MAX_ENTITIES) {
        if texts.len() + segs.len() >= MAX_WINDOW_ITEMS * 4 {
            break;
        }
        match &e.kind {
            EntityKind::Text(t) => {
                let (at, h) = (t.insert.xy(), t.height);
                let v = t.value.trim();
                if !v.is_empty() && at.is_finite() && h.is_finite() && h > 0.0 && inside(at) {
                    texts.push(TextItem { value: v.to_string(), at, height: h, rotation: if t.rotation.is_finite() { t.rotation } else { 0.0 } });
                }
            }
            EntityKind::MText(t) => {
                let (at, h) = (t.insert.xy(), t.height);
                let v = t.contents.trim();
                if !v.is_empty() && at.is_finite() && h.is_finite() && h > 0.0 && inside(at) {
                    texts.push(TextItem { value: v.to_string(), at, height: h, rotation: if t.rotation.is_finite() { t.rotation } else { 0.0 } });
                }
            }
            EntityKind::Line(l) => {
                let (a, b) = (l.a.xy(), l.b.xy());
                if inside(a) || inside(b) {
                    segs.push((a, b));
                }
            }
            EntityKind::LwPolyline(p) => {
                let n = p.vertices.len();
                let last = if p.closed { n } else { n.saturating_sub(1) };
                for i in 0..last.min(10_000) {
                    if let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get((i + 1) % n.max(1)))
                        && a.bulge.abs() < 1e-9
                        && (inside(a.p) || inside(b.p))
                    {
                        segs.push((a.p, b.p));
                    }
                }
            }
            _ => {}
        }
    }
    (texts, segs)
}

/// The schedule tables on the sheets the drawing holds: each title found, its sheet the sheet
/// link whose rectangle holds it, read in a window round it.
fn read_drawing(d: &Drawing, sp: &BuildingSpec, docs: &mut FixtureDocs) {
    let mut titles: Vec<(Vec2, f64)> = Vec::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        let (v, at, h) = match &e.kind {
            EntityKind::Text(t) => (t.value.as_str(), t.insert.xy(), t.height),
            EntityKind::MText(t) => (t.contents.as_str(), t.insert.xy(), t.height),
            _ => continue,
        };
        if at.is_finite() && h.is_finite() && h > 0.0 && is_title(v).is_some() && titles.len() < MAX_TITLES {
            titles.push((at, h));
        }
    }
    let sheets: Vec<(&str, [f64; 4])> = sp.sheets.iter().filter_map(|l| Some((l.number.as_str(), norm_rect(l.rect?)))).collect();
    for (at, h) in titles {
        // Its sheet: the smallest sheet rectangle holding it.
        let sheet = sheets
            .iter()
            .filter(|(_, r)| at.x >= r[0] && at.x <= r[2] && at.y >= r[1] && at.y <= r[3])
            .min_by(|a, b| ((a.1[2] - a.1[0]) * (a.1[3] - a.1[1])).total_cmp(&((b.1[2] - b.1[0]) * (b.1[3] - b.1[1]))))
            .map_or("the drawing", |(n, _)| *n);
        let window = [at.x - 120.0 * h, at.y - 600.0 * h, at.x + 120.0 * h, at.y + 2.0 * h];
        let (texts, segs) = page_items(d, Some(window));
        read_tables(&texts, &segs, sheet, docs);
    }
}

/// The job's sheets the drawing doesn't hold whose page may carry the schedule (its title or
/// type has a page word, or it is a sheet of a page discipline that isn't a plan), read off the
/// job's PDFs (the sheet index gives each one's file and page).
fn read_job_pages(sp: &BuildingSpec, job: &super::guided::GuidedJob, docs: &mut FixtureDocs) {
    let dt = data();
    let held: BTreeSet<String> = sp.sheets.iter().filter(|l| l.rect.is_some()).map(|l| base_number(&l.number)).collect();
    let entries: Vec<&Value> =
        job.sheets.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array).into_iter().flatten().take(5_000).collect();
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let mut wanted: Vec<(String, String, u64)> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for e in entries {
        if !text(e, "superseded_by").is_empty() {
            continue;
        }
        let base = base_number(&text(e, "number"));
        if base.is_empty() || held.contains(&base) || !cadaddy_mepfix::sheet_facts::is_sheet_number(&base) || !seen.insert(base.clone()) {
            continue;
        }
        let kind = format!("{} {}", text(e, "type"), text(e, "sheet_type")).to_ascii_uppercase();
        let what = format!("{} {kind}", text(e, "title")).to_ascii_uppercase();
        let disc = Some(text(e, "discipline"))
            .filter(|x| !x.is_empty())
            .unwrap_or_else(|| base.chars().take_while(char::is_ascii_alphabetic).collect())
            .to_ascii_uppercase();
        let by_word = dt.page_words.iter().any(|w| what.contains(&w.to_ascii_uppercase()));
        let by_disc = dt.page_disciplines.iter().any(|x| x.eq_ignore_ascii_case(&disc)) && !kind.contains("PLAN");
        if !(by_word || by_disc) {
            continue;
        }
        let (file, page) = (text(e, "file"), e.get("page").and_then(Value::as_u64).filter(|p| *p >= 1).unwrap_or(1));
        if file.is_empty() {
            *docs.unmeasured.entry("a sheet that may hold the schedule has no PDF file in the job's index".into()).or_default() += 1;
            continue;
        }
        wanted.push((base, file, page));
    }
    if wanted.is_empty() {
        return;
    }
    let Some(read_pdf) = super::file::io().and_then(|h| h.read_pdf) else {
        docs.unmeasured.insert("sheets the drawing doesn't hold: no PDF reader in this build".into(), wanted.len() as u64);
        return;
    };
    let dir = std::path::PathBuf::from(&job.pdfs_path);
    let dir = if dir.is_file() { dir.parent().map(std::path::Path::to_path_buf).unwrap_or_default() } else { dir };
    let mut files: BTreeMap<String, std::result::Result<Vec<u8>, String>> = BTreeMap::new();
    let total = wanted.len();
    for (number, file, page) in wanted.into_iter().take(dt.max_pages) {
        let named = std::path::PathBuf::from(&file);
        let path = if named.is_absolute() && named.is_file() { named } else { dir.join(named.file_name().unwrap_or_default()) };
        let bytes = files.entry(path.to_string_lossy().to_string()).or_insert_with(|| {
            crate::readfile::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| if b.len() > MAX_PDF_BYTES { Err("the file is over 200 MB".to_string()) } else { Ok(b) })
        });
        let got = match bytes {
            Ok(b) => read_pdf(b, &json!({"page": page, "text": true, "images": false})).map(|(p, _)| p),
            Err(e) => Err(e.clone()),
        };
        match got {
            Ok(pd) => {
                let (texts, segs) = page_items(&pd, None);
                if texts.iter().any(|t| is_title(&t.value).is_some()) {
                    read_tables(&texts, &segs, &number, docs);
                }
                docs.pages_read.push(number);
            }
            Err(why) => {
                *docs.unmeasured.entry(format!("a sheet's page could not be read ({})", quote(&why))).or_default() += 1;
            }
        }
    }
    if total > dt.max_pages {
        docs.unmeasured.insert("sheets past the reading data's max_pages, not read".into(), (total - dt.max_pages) as u64);
    }
}

fn base_number(n: &str) -> String {
    n.split('/').next().unwrap_or("").trim().to_ascii_uppercase()
}

/// Files under a submittals folder (PDFs and submittal texts).
fn files_under(p: &std::path::Path, out: &mut Vec<String>, depth: usize) {
    if out.len() >= MAX_FILES_LOOKED || depth > MAX_DEPTH {
        return;
    }
    if p.is_file() {
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf") || x.eq_ignore_ascii_case("txt")) {
            out.push(p.to_string_lossy().to_string());
        }
        return;
    }
    let Ok(rd) = std::fs::read_dir(p) else { return };
    let mut entries: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for e in entries {
        files_under(&e, out, depth + 1);
    }
}

/// Is a submittal file a lighting one: its path names lighting, or a lighting section number
/// (`265113`, `26 51 13`)?
fn lighting_file(path: &str) -> bool {
    let dt = data();
    let up = path.to_ascii_uppercase();
    if dt.submittal_words.iter().any(|w| up.contains(&w.to_ascii_uppercase())) {
        return true;
    }
    // Section numbers: runs of digits, `26 51 13` joined.
    let squeezed: String = up.split(' ').collect::<Vec<_>>().join("");
    let runs: Vec<String> = squeezed.split(|c: char| !c.is_ascii_digit()).filter(|r| r.len() >= 4).map(str::to_string).collect();
    runs.iter().any(|r| r.len() == 6 && dt.submittal_sections.iter().any(|s| r.starts_with(s.as_str())))
}

/// The job's lighting submittals read again (`bim.spec_import`, the shared submittal reader): each
/// type's product from the latest revision not sent back.
fn read_submittals(s: &mut crate::Session, job: &super::guided::GuidedJob, docs: &mut FixtureDocs) {
    use cadaddy_doc::bimspec::keys;
    let mut files: Vec<String> = Vec::new();
    for root in &job.submittals {
        files_under(std::path::Path::new(root.trim()), &mut files, 0);
    }
    files.sort();
    files.dedup();
    let mine: Vec<String> = files.into_iter().filter(|f| lighting_file(f)).collect();
    let cap = data().max_submittal_files;
    if mine.len() > cap {
        docs.unmeasured
            .insert(format!("lighting submittal files past the reading data's max_submittal_files ({cap}), not read"), (mine.len() - cap) as u64);
    }
    let mut all: BTreeMap<String, Vec<SubProduct>> = BTreeMap::new();
    for f in mine.into_iter().take(cap) {
        let name = std::path::Path::new(&f).file_name().map_or_else(|| f.clone(), |n| n.to_string_lossy().to_string());
        let revision = super::equipsub::revision(&f);
        let r = match s.execute("bim.spec_import", &json!({"file": f, "format": "submittal"})) {
            Ok(r) => r,
            Err(e) => {
                docs.files.push(json!({"file": name, "error": quote(&e.to_string())}));
                *docs.unmeasured.entry("a lighting submittal file could not be read".into()).or_default() += 1;
                continue;
            }
        };
        let mut marks = Vec::new();
        for p in r.get("proposals").and_then(Value::as_array).into_iter().flatten().take(2_000) {
            let Some(mark) = p.get("mark").and_then(Value::as_str).and_then(tag_cell) else { continue };
            // A word the reader took for a type (`CONTROLS`, a heading) isn't one.
            if mark.len() >= 4 && mark.chars().all(|c| c.is_ascii_alphabetic()) {
                *docs.unmeasured.entry("a submittal \"type\" that is a word, not a tag (not read as a type)".into()).or_default() += 1;
                continue;
            }
            let field = |k: &str| -> (String, String) {
                let f = p.get("fields").and_then(|f| f.get(k));
                let v = f.and_then(|f| f.get("value")).and_then(Value::as_str).unwrap_or_default().trim().to_string();
                let src = f.and_then(|f| f.get("source")).and_then(Value::as_str).unwrap_or_default().trim().to_string();
                (v, src)
            };
            let (maker, msrc) = field(keys::MANUFACTURER);
            let (catalog, csrc) = field(keys::CATALOG_NUMBER);
            let luminaire = p.get("category").and_then(Value::as_str) == Some("luminaire");
            if maker.is_empty() && catalog.is_empty() && !luminaire {
                continue;
            }
            let page = [&csrc, &msrc].into_iter().find_map(|s| super::validate_equip::cited_page(s));
            let (status, _) = field(keys::REVIEW_STATUS);
            let (voltage, _) = field(keys::VOLTS);
            let (wattage, _) = field(keys::WATTAGE);
            marks.push(mark.clone());
            all.entry(mark).or_default().push(SubProduct {
                maker,
                catalog,
                voltage,
                wattage,
                cite: page.map_or_else(|| name.clone(), |n| format!("{name} p.{n}")),
                revision,
                status,
                others: Vec::new(),
            });
        }
        // A product sent back (revise and resubmit, rejected) is no proposal of the reader's; its
        // record keeps the types it covered and the stamp.
        for rec in r.get("submittals").and_then(Value::as_array).into_iter().flatten().take(2_000) {
            let status = rec.get("reviewStatus").and_then(Value::as_str).unwrap_or_default();
            if !matches!(status, "revise" | "rejected") {
                continue;
            }
            let src = rec.get("fileSource").and_then(Value::as_str).unwrap_or_default();
            let page = super::validate_equip::cited_page(src);
            for mark in rec.get("typeMarks").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter_map(tag_cell).take(200)
            {
                if marks.contains(&mark) {
                    continue;
                }
                marks.push(mark.clone());
                all.entry(mark).or_default().push(SubProduct {
                    maker: rec.get("manufacturer").and_then(Value::as_str).unwrap_or_default().to_string(),
                    catalog: rec.get("catalogNumber").and_then(Value::as_str).unwrap_or_default().to_string(),
                    cite: page.map_or_else(|| name.clone(), |n| format!("{name} p.{n}")),
                    revision,
                    status: status.to_string(),
                    ..SubProduct::default()
                });
            }
        }
        marks.dedup();
        docs.files.push(json!({"file": name, "revision": format!("{}-{}", revision.0, revision.1), "types": marks}));
    }
    for (mark, mut list) in all {
        list.sort_by_key(|p| std::cmp::Reverse(p.rank()));
        let mut it = list.into_iter();
        let Some(mut best) = it.next() else { continue };
        for o in it.take(8) {
            let st = if o.status.is_empty() { "not stamped".to_string() } else { o.status.clone() };
            let w = format!("{} ({st})", o.cite);
            if !best.others.contains(&w) {
                best.others.push(w);
            }
        }
        docs.products.insert(mark, best);
    }
}

/// Validate's reading of the lighting documents: the schedule from the drawing's sheets and the
/// job's pages, the lighting submittals from the job's folders.
pub(crate) fn read_docs(s: &mut crate::Session, d: &Drawing, sp: &BuildingSpec) -> FixtureDocs {
    let mut docs = FixtureDocs::default();
    read_drawing(d, sp, &mut docs);
    let job = super::guided::load_job(s).ok().flatten();
    match &job {
        Some(job) => {
            read_job_pages(sp, job, &mut docs);
            read_submittals(s, job, &mut docs);
        }
        None => {
            docs.unmeasured.insert("no job: only the sheets the drawing holds were read, and no submittals".into(), 1);
        }
    }
    docs
}

/// Is the rule on and asked for?
pub(crate) fn wanted(rules: &[Effective], only: &[String]) -> bool {
    rules.iter().any(|r| r.rule.id == RULE && r.enabled) && (only.is_empty() || only.iter().any(|o| o == RULE))
}

// ---------------------------------------------------------------- what is checked

/// What the rule reads.
pub(crate) struct Ctx<'a> {
    pub d: &'a Drawing,
    pub sp: &'a BuildingSpec,
    pub prints: Option<&'a Prints>,
    pub docs: Option<&'a FixtureDocs>,
    /// The scope is the whole building (every level that has rooms): the checks of types with no
    /// luminaire in scope run, and every lighting RFI is matched (else only those of the types in
    /// scope).
    pub building: bool,
}

/// What the rule found.
#[derive(Default)]
pub(crate) struct Checked {
    /// (rank, verdict, finding).
    pub findings: Vec<(u8, &'static str, Value)>,
    pub unmeasured: BTreeMap<String, u64>,
    /// Per type: plan count, model count, schedule row, submittal product and verdicts.
    pub table: Value,
}

fn rank(verdict: &str, severity: &str) -> u8 {
    match verdict {
        "pass" => 9,
        "unsure" => 5,
        "extra" | "missing" => 1,
        _ => match severity {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        },
    }
}

/// A luminaire's type from its mark (`luminaire XA` → `XA`).
fn type_of(mark: &str) -> Option<String> {
    let m = mark.trim();
    let lower = m.to_ascii_lowercase();
    let m = ["luminaire ", "light fixture ", "fixture ", "type "]
        .iter()
        .find_map(|p| lower.starts_with(p).then(|| m.get(p.len()..)))
        .flatten()
        .unwrap_or(m);
    tag_cell(m)
}

/// A model value of a luminaire: the first of `keys` its properties (or its source object's, or
/// its model fields) give, and where it comes from.
fn model_value(d: &Drawing, it: &Lit, keys: &[&str]) -> Option<(String, String)> {
    let key = |k: &str| alnum(k);
    let want: Vec<String> = keys.iter().map(|k| key(k)).collect();
    let own: Props = it.entity.map(|e| cadaddy_doc::props::read(&e.common.xdata)).unwrap_or_default();
    let source: Props = it
        .entity
        .and_then(super::bimlook::made_source)
        .and_then(|h| d.model.get(h))
        .map(|s| cadaddy_doc::props::read(&s.common.xdata))
        .unwrap_or_default();
    for w in &want {
        for p in [&own, &source] {
            if let Some((_, prop)) = p.iter().take(400).find(|(k, prop)| key(k) == *w && prop.has_value()) {
                let from = match &prop.source.doc {
                    Some(doc) => format!("{} {}", prop.source.kind.as_str(), quote(doc)),
                    None => prop.source.kind.as_str().to_string(),
                };
                return Some((prop.text().trim().to_string(), from));
            }
        }
        if let Some((_, v)) = it.fields.into_iter().flatten().take(400).find(|(k, v)| key(k) == *w && !v.trim().is_empty()) {
            return Some((v.trim().to_string(), "the model's field".into()));
        }
    }
    None
}

/// Does the model's value of `field` agree with the row's? None: not comparable.
fn agrees(field: &str, model: &str, row: &SchedRow) -> Option<bool> {
    let contains = |a: &str, b: &str| {
        let (x, y) = (alnum(a), alnum(b));
        !x.is_empty() && !y.is_empty() && (x.contains(&y) || y.contains(&x))
    };
    match field {
        "basis_of_design" => {
            if row.basis().is_empty() {
                return None;
            }
            if contains(model, &row.basis()) || (alnum(&row.model).len() >= 3 && alnum(model).contains(&alnum(&row.model))) {
                return Some(true);
            }
            let maker_ok = row.maker.is_empty() || same_maker(model, &row.maker);
            let series_ok = same_series(model, &row.model).unwrap_or(true);
            Some(maker_ok && series_ok && !(row.maker.is_empty() && alnum(&row.model).len() < 3))
        }
        "lamp" => {
            if row.lamp.is_empty() {
                return None;
            }
            if contains(model, &row.lamp) {
                return Some(true);
            }
            let (a, b) = (va_of(model), va_of(&row.lamp));
            let src =
                |v: &str| words_of(v).into_iter().filter(|w| w.chars().all(|c| c.is_ascii_alphabetic()) && w.len() >= 3).collect::<BTreeSet<_>>();
            Some(matches!((a, b), (Some(x), Some(y)) if (x - y).abs() <= 1.0) && !src(model).is_disjoint(&src(&row.lamp)))
        }
        "va" => {
            let (x, y) = (va_of(model).or_else(|| numbers(model).first().copied())?, row.va?);
            Some((x - y).abs() <= (0.05 * x.max(y)).max(1.0))
        }
        "voltage" => {
            let (x, y) = (volts(model), volts(&row.voltage));
            (!x.is_empty() && !y.is_empty()).then(|| !x.is_disjoint(&y))
        }
        "installation" => {
            if row.installation.is_empty() {
                return None;
            }
            // By their mounting classes; a value with none (a height, `8'-0" AFF`) compares only when
            // one holds the other.
            match mounting_agrees(model, &row.installation) {
                Some(ok) => Some(ok || contains(model, &row.installation)),
                None => contains(model, &row.installation).then_some(true),
            }
        }
        _ => None,
    }
}

/// The fields a luminaire is compared on: (field, the row's value in words, the model's keys).
const ROW_FIELDS: [(&str, &[&str]); 5] = [
    ("basis_of_design", &["basis_of_design", "basis"]),
    ("lamp", &["lamp", "lamp_source", "source"]),
    ("va", &["fixture_va", "va", "input_va", "load_va", "wattage"]),
    ("voltage", &["voltage", "volts"]),
    ("installation", &["installation", "mounting_type", "mounting"]),
];

fn row_says(field: &str, row: &SchedRow) -> String {
    match field {
        "basis_of_design" => row.basis(),
        "lamp" => row.lamp.clone(),
        "va" => row.va.map(|v| format!("{v} VA")).unwrap_or_default(),
        "voltage" => row.voltage.clone(),
        "installation" => row.installation.clone(),
        _ => String::new(),
    }
}

/// The build's lighting RFI about a fixture type: its types and the check it is (none: a field
/// Validate doesn't recompute, its topic said).
fn rfi_subject(item: &cadaddy_walk3d::Rfi) -> Option<(Vec<String>, std::result::Result<&'static str, String>)> {
    let topic = item.topic.split_once(':').map_or(item.topic.as_str(), |(_, t)| t).trim().to_ascii_lowercase();
    let about = item.about.trim();
    let rest = about.strip_prefix("lighting types ").or_else(|| about.strip_prefix("lighting type "))?;
    let types: Vec<String> = rest.split([',', ' ']).filter_map(tag_cell).filter(|t| t != "AND").take(8).collect();
    if types.is_empty() {
        return None;
    }
    let check = match topic.as_str() {
        "plan type with no schedule row" => Ok("scheduled"),
        "scheduled type on no plan" => Ok("on_plan"),
        "scheduled type with no submittal" | "maker" | "catalog number" | "one product for two types" => Ok("submittal"),
        "submitted type with no schedule row" => Ok("submitted"),
        "voltage" | "wattage" => Ok("fields"),
        "plan count vs quantity" => Ok("quantity"),
        _ => Err(topic),
    };
    Some((types, check))
}

/// Is a building-spec RFI one of the build's lighting ones (open or sent)?
fn lighting_rfi(item: &cadaddy_walk3d::Rfi) -> bool {
    let ours = item.kind == Some(cadaddy_walk3d::RfiKind::Lighting)
        || (item.kind == Some(cadaddy_walk3d::RfiKind::Conflict) && item.topic.trim().to_ascii_lowercase().starts_with("lighting:"));
    ours && matches!(item.status, cadaddy_walk3d::rfi::RfiStatus::Open | cadaddy_walk3d::rfi::RfiStatus::Sent)
}

struct TypeOutcome {
    check: &'static str,
    verdict: &'static str,
    issue: String,
    expected: String,
    found: String,
    working: Value,
}

/// Check every luminaire in `items` and each fixture type, then match the build's RFIs.
pub(crate) fn check(cx: &Ctx, r: &Effective, items: &[Lit]) -> Checked {
    let mut out = Checked::default();
    let sev = r.rule.severity.clone();
    let lums: Vec<&Lit> = items.iter().filter(|it| r.rule.applies_to_element(it.kind, it.category, it.mark)).collect();
    let Some(docs) = cx.docs else {
        if !lums.is_empty() {
            out.unmeasured.insert("the fixture schedule was not re-read here (no documents for this screen)".into(), lums.len() as u64);
        }
        return out;
    };
    for (why, n) in &docs.unmeasured {
        out.unmeasured.insert(why.clone(), *n);
    }
    let schedule_read = !docs.rows.is_empty();
    let sheets: BTreeSet<&str> = docs.rows.values().map(|r| r.sheet.as_str()).collect();
    let sheets_words = sheets.iter().take(6).copied().collect::<Vec<_>>().join(", ");
    let mut by_type: BTreeMap<String, Vec<&Lit>> = BTreeMap::new();

    // Each luminaire against its row.
    for it in &lums {
        let ty = it.mark.and_then(type_of);
        if let Some(t) = &ty {
            by_type.entry(t.clone()).or_default().push(it);
        }
        let row = ty.as_ref().and_then(|t| docs.rows.get(t));
        let mut compared: Vec<Value> = Vec::new();
        let mut fails: Vec<String> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        let (verdict, issue, expected) = match (&ty, row) {
            (None, _) => {
                ("unsure", "it has no type tag: its schedule row is not known".to_string(), "a fixture schedule row for its type".to_string())
            }
            (Some(t), None) if !schedule_read => (
                "unsure",
                format!("type {t}: no light fixture schedule was read, so its row is not known"),
                format!("type {t}'s fixture schedule row"),
            ),
            (Some(t), None) => (
                "unsure",
                format!("type {t} has no row in the fixture schedule ({sheets_words}): its fields are not known"),
                format!("type {t}'s fixture schedule row"),
            ),
            (Some(t), Some(row)) => {
                for (field, keys) in ROW_FIELDS {
                    let says = row_says(field, row);
                    let model = model_value(cx.d, it, keys);
                    let (status, model_v, from) = match (&model, says.is_empty()) {
                        (_, true) => ("not in the row", model.as_ref().map(|m| m.0.clone()), model.as_ref().map(|m| m.1.clone())),
                        (None, false) => {
                            unknown.push(format!("its {field} (the row says {})", quote(&says)));
                            ("not in the model", None, None)
                        }
                        (Some((v, from)), false) => match agrees(field, v, row) {
                            Some(true) => ("agrees", Some(v.clone()), Some(from.clone())),
                            Some(false) => {
                                fails.push(format!("{field} {} (from {from}) where the row says {}", quote(v), quote(&says)));
                                ("differs", Some(v.clone()), Some(from.clone()))
                            }
                            None => {
                                unknown.push(format!("its {field} {} can't be compared with the row's {}", quote(v), quote(&says)));
                                ("not comparable", Some(v.clone()), Some(from.clone()))
                            }
                        },
                    };
                    compared.push(json!({"field": field, "row": says, "model": model_v, "model_from": from, "status": status}));
                }
                let src = row.source();
                if !fails.is_empty() {
                    ("fail", format!("type {t} differs from its schedule row ({src}): {}", fails.join("; ")), format!("the fields of {src}"))
                } else if !unknown.is_empty() {
                    ("unsure", format!("type {t} ({src}): the model doesn't say {}", unknown.join("; ")), format!("the fields of {src}"))
                } else {
                    ("pass", format!("type {t} carries its schedule row's fields ({src})"), format!("the fields of {src}"))
                }
            }
        };
        if verdict == "unsure" {
            let why = match (&ty, row) {
                (None, _) => "a luminaire with no type tag".to_string(),
                (Some(_), None) if !schedule_read => "no light fixture schedule read".to_string(),
                (Some(_), None) => "its type has no schedule row".to_string(),
                _ => "a field of its row the model doesn't record".to_string(),
            };
            *out.unmeasured.entry(why).or_default() += 1;
        }
        let found = match &ty {
            None => "no type tag".to_string(),
            Some(t) if row.is_none() => format!("type {t}, no row"),
            Some(t) => format!("type {t}: {} field(s) differ, {} not recorded", fails.len(), unknown.len()),
        };
        let id = format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|{}|row|{verdict}", it.handle)));
        let f = json!({
            "id": id, "rule": RULE, "handle": it.handle, "kind": it.kind, "kind_from": it.kind_from, "mark": it.mark, "tag": ty,
            "built": it.built, "verdict": verdict, "issue": issue, "expected": expected, "found": found,
            "measured": {"check": "row", "type": ty, "row": row.map(SchedRow::to_json), "fields": compared},
            "evidence": [], "confidence": if verdict == "unsure" { 0.5 } else { 1.0 }, "severity": sev,
            "area": it.room.map(|a| a.id.clone()), "level": it.level,
            "at": super::validate_packet::pt([super::validate_packet::r2(it.at[0]), super::validate_packet::r2(it.at[1])]), "source": "prescreen",
        });
        out.findings.push((rank(verdict, &sev), verdict, f));
    }

    let row_verdict: BTreeMap<String, &'static str> =
        out.findings.iter().filter_map(|(_, v, f)| Some((f["handle"].as_str()?.to_string(), *v))).collect();
    // The plans' counts of each type: the prints' symbols, else the job's device tags.
    let mut plan: BTreeMap<String, u32> = BTreeMap::new();
    let mut plan_from = "none";
    for s in cx.prints.map_or(&[][..], |p| p.symbols.as_slice()).iter().take(2_000_000) {
        if let Some(t) = s.mark.as_deref().or(s.tag.as_deref()).and_then(type_of) {
            *plan.entry(t).or_default() += 1;
            plan_from = "the prints' symbols";
        }
    }
    if plan.is_empty() {
        for t in cx.sp.tags.iter().take(2_000_000).filter(|t| t.id.starts_with("device:")) {
            if let Some(ty) = ["type", "type_mark", "mark"].iter().find_map(|k| t.fields.get(*k)).and_then(|f| type_of(&f.value)) {
                *plan.entry(ty).or_default() += 1;
                plan_from = "the job's symbol table (device tags read off the plans)";
            }
        }
    }

    // The types: those of the luminaires in scope, and (building scope) every scheduled and
    // submitted one.
    let mut types: BTreeSet<String> = by_type.keys().cloned().collect();
    if cx.building {
        types.extend(docs.rows.keys().cloned());
        types.extend(docs.products.keys().cloned());
    }
    let submittals_read = !docs.products.is_empty();
    let mut verdicts: BTreeMap<(String, &'static str), (&'static str, String)> = BTreeMap::new();
    let mut fails: Vec<(String, &'static str, String)> = Vec::new();
    let mut table: Vec<Value> = Vec::new();
    for ty in types.iter().take(MAX_TYPES) {
        let row = docs.rows.get(ty);
        let product = docs.products.get(ty);
        let model_n = by_type.get(ty).map_or(0, Vec::len);
        let plan_n = plan.get(ty).copied().unwrap_or(0);
        let mut outs: Vec<TypeOutcome> = Vec::new();
        // scheduled
        if model_n > 0 || plan_n > 0 {
            match row {
                Some(r) => outs.push(TypeOutcome {
                    check: "scheduled",
                    verdict: "pass",
                    issue: format!("type {ty} is in the fixture schedule ({})", r.source()),
                    expected: "a schedule row".into(),
                    found: r.source(),
                    working: json!({"plan": plan_n, "model": model_n}),
                }),
                None if schedule_read => outs.push(TypeOutcome {
                    check: "scheduled",
                    verdict: "fail",
                    issue: format!("type {ty} is on the plans ({plan_n} symbol(s), {model_n} in the model) but the fixture schedule ({sheets_words}) has no row for it"),
                    expected: format!("a row for type {ty}"),
                    found: "no row".into(),
                    working: json!({
                        "plan": plan_n, "model": model_n, "schedule_sheets": sheets,
                        "luminaires": by_type.get(ty).into_iter().flatten().take(MAX_LISTED).map(|l| l.handle).collect::<Vec<_>>(),
                    }),
                }),
                None => {}
            }
        }
        // on_plan
        if let Some(r) = row
            && cx.building
        {
            let shown = plan_n > 0 || model_n > 0;
            outs.push(TypeOutcome {
                check: "on_plan",
                verdict: if shown { "pass" } else { "fail" },
                issue: if shown {
                    format!("type {ty} is shown: {plan_n} symbol(s) on the plans, {model_n} in the model")
                } else {
                    format!("type {ty} is scheduled ({}) but no plan shows one and the model has none", r.source())
                },
                expected: "a luminaire of the type on some plan".into(),
                found: format!("{plan_n} on the plans, {model_n} in the model"),
                working: json!({"plan": plan_n, "plan_from": plan_from, "model": model_n}),
            });
        }
        // submittal
        if let Some(r) = row {
            let o = match product {
                None if !submittals_read => TypeOutcome {
                    check: "submittal",
                    verdict: "unsure",
                    issue: format!("type {ty}: no lighting submittal was read, so its approved product is not known"),
                    expected: format!("an approved product matching {}", quote(&r.basis())),
                    found: "no submittal read".into(),
                    working: json!({"row": r.source(), "files": docs.files.len()}),
                },
                None => TypeOutcome {
                    check: "submittal",
                    verdict: "fail",
                    issue: format!(
                        "type {ty} is scheduled ({}, {}) but no lighting submittal read gives a product for it",
                        r.source(),
                        quote(&r.basis())
                    ),
                    expected: format!("an approved product for type {ty}"),
                    found: "none".into(),
                    working: json!({"row": r.source(), "files": docs.files}),
                },
                Some(p) => submittal_outcome(ty, r, p, docs.no_substitutions.as_deref()),
            };
            outs.push(o);
        }
        // submitted
        if let Some(p) = product
            && row.is_none()
            && schedule_read
            && cx.building
        {
            outs.push(TypeOutcome {
                check: "submitted",
                verdict: "fail",
                issue: format!(
                    "the lighting submittal gives type {ty} ({}, {}) but the fixture schedule ({sheets_words}) has no row for it{}",
                    p.words(),
                    p.cite,
                    if plan_n + model_n as u32 == 0 { " and no plan shows one" } else { "" }
                ),
                expected: format!("a schedule row for type {ty}"),
                found: "no row".into(),
                working: json!({"submittal": p.to_json(), "plan": plan_n, "model": model_n}),
            });
        }
        // fields
        if let (Some(r), Some(p)) = (row, product) {
            let mut diffs = Vec::new();
            let mut compared = Vec::new();
            let (rv, pv) = (volts(&r.voltage), volts(&p.voltage));
            if !rv.is_empty() && !pv.is_empty() {
                compared.push("voltage");
                if rv.is_disjoint(&pv) {
                    diffs.push(format!("voltage: the schedule says {} but the submittal {}", quote(&r.voltage), quote(&p.voltage)));
                }
            }
            if let (Some(a), Some(b)) = (r.va, va_of(&p.wattage).or_else(|| numbers(&p.wattage).first().copied())) {
                compared.push("va");
                if (a - b).abs() > 0.2 * a.max(b) + 1.0 {
                    diffs.push(format!("load: the schedule says {a} VA but the submittal {}", quote(&p.wattage)));
                }
            }
            if !compared.is_empty() {
                outs.push(TypeOutcome {
                    check: "fields",
                    verdict: if diffs.is_empty() { "pass" } else { "fail" },
                    issue: if diffs.is_empty() {
                        format!("type {ty}: the schedule and the submittal agree on {}", compared.join(" and "))
                    } else {
                        format!("type {ty} ({} vs {}): {}", r.source(), p.cite, diffs.join("; "))
                    },
                    expected: format!("the submittal's {} as the schedule's", compared.join(" and ")),
                    found: if diffs.is_empty() { "the same".into() } else { diffs.join("; ") },
                    working: json!({"row": {"voltage": r.voltage, "va": r.va}, "submittal": {"voltage": p.voltage, "wattage": p.wattage, "cite": p.cite}}),
                });
            }
        }
        // quantity
        if let (Some(r), Some(q)) = (row, row.and_then(|r| r.quantity))
            && cx.building
        {
            let ok = q as usize == model_n;
            outs.push(TypeOutcome {
                check: "quantity",
                verdict: if ok { "pass" } else { "fail" },
                issue: format!("type {ty}: the schedule's quantity is {q} ({}); the model has {model_n}", r.source()),
                expected: format!("{q}"),
                found: format!("{model_n}"),
                working: json!({"schedule": q, "model": model_n, "plan": plan_n}),
            });
        }
        let mut row_verdicts: BTreeMap<&str, u64> = BTreeMap::new();
        for l in by_type.get(ty).into_iter().flatten() {
            let v = row_verdict.get(l.handle).copied().unwrap_or("unsure");
            *row_verdicts.entry(if matches!(v, "pass" | "fail") { v } else { "unsure" }).or_default() += 1;
        }
        table.push(json!({
            "type": ty, "plan": plan_n, "model": model_n,
            "row": row.map(|r| json!({"source": r.source(), "basis_of_design": r.basis(), "lamp": r.lamp, "va": r.va, "voltage": r.voltage, "installation": r.installation})),
            "submittal": product.map(SubProduct::to_json),
            "verdicts": outs.iter().map(|o| (o.check, o.verdict)).collect::<BTreeMap<_, _>>(),
            "luminaires": row_verdicts,
        }));
        for o in outs {
            if o.verdict == "unsure" {
                *out.unmeasured.entry(format!("{}: {}", o.check, generic(&o.issue))).or_default() += 1;
            }
            verdicts.insert((ty.clone(), o.check), (o.verdict, o.issue.clone()));
            if o.verdict == "fail" {
                fails.push((ty.clone(), o.check, o.issue.clone()));
            }
            let id = format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|type|{ty}|{}|{}", o.check, o.verdict)));
            let severity = if matches!(o.check, "on_plan" | "submitted") { "low".to_string() } else { sev.clone() };
            let f = json!({
                "id": id, "rule": RULE, "handle": Value::Null, "kind": "luminaire_type", "mark": ty, "tag": ty, "verdict": o.verdict,
                "issue": o.issue, "expected": o.expected, "found": o.found,
                "measured": {"check": o.check, "type": ty, "row": row.map(SchedRow::to_json), "submittal": product.map(SubProduct::to_json), "working": o.working},
                "evidence": [], "confidence": if o.verdict == "unsure" { 0.5 } else { 1.0 }, "severity": severity,
                "area": Value::Null, "level": "", "at": Value::Null, "source": "prescreen",
            });
            out.findings.push((rank(o.verdict, &severity), o.verdict, f));
        }
    }

    // The build's RFIs against the verdicts.
    let mut raised: BTreeSet<(String, &'static str)> = BTreeSet::new();
    let mut confirmed: Vec<String> = Vec::new();
    let mut disputed: Vec<String> = Vec::new();
    let mut not_recomputed: Vec<String> = Vec::new();
    for item in cx.sp.rfis.iter().take(MAX_RFIS).filter(|i| lighting_rfi(i)) {
        let Some((tys, check)) = rfi_subject(item) else {
            *out.unmeasured
                .entry("a lighting RFI about controls, rooms or plans, not a fixture type (E-LUM-CONTROL and E-SWITCH-* carry those)".into())
                .or_default() += 1;
            continue;
        };
        if !cx.building && !tys.iter().any(|t| types.contains(t)) {
            continue;
        }
        let sides: Vec<Value> = item.sides.iter().take(6).map(|s| json!({"document": s.document, "says": quote(&s.says)})).collect();
        let check = match check {
            Ok(c) => c,
            Err(topic) => {
                not_recomputed.push(item.id.clone());
                let ty = tys.first().cloned().unwrap_or_default();
                let issue = format!(
                    "the build's RFI {} (type {}, {topic}) is about a field Validate doesn't recompute from the schedule and the submittal; its sides: {}",
                    item.id,
                    tys.join(", "),
                    item.sides.iter().take(4).map(|s| format!("{} says {}", s.document, quote(&s.says))).collect::<Vec<_>>().join("; ")
                );
                out.findings.push((
                    rank("unsure", &sev),
                    "unsure",
                    json!({
                        "id": format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|rfi|{}", item.id))), "rule": RULE, "handle": Value::Null,
                        "kind": "luminaire_type", "mark": ty, "tag": ty, "verdict": "unsure", "issue": issue,
                        "expected": "a check Validate recomputes", "found": quote(&item.issue),
                        "measured": {"check": "rfi", "topic": topic, "rfi": item.id, "types": tys, "sides": sides},
                        "evidence": [], "confidence": 0.5, "severity": sev, "area": Value::Null, "level": item.level, "at": Value::Null, "source": "prescreen",
                    }),
                ));
                *out.unmeasured.entry(format!("a lighting RFI about {topic}, a field Validate doesn't recompute")).or_default() += 1;
                continue;
            }
        };
        for t in &tys {
            raised.insert((t.clone(), check));
        }
        let failed: Vec<&String> = tys.iter().filter(|t| verdicts.get(&((*t).clone(), check)).is_some_and(|(v, _)| *v == "fail")).collect();
        if !failed.is_empty() {
            confirmed.push(item.id.clone());
            for t in failed {
                if let Some((_, _, f)) = out.findings.iter_mut().find(|(_, v, f)| {
                    *v == "fail" && f["tag"].as_str() == Some(t.as_str()) && f["measured"]["check"] == check && f["handle"].is_null()
                }) {
                    let list = f["measured"]["rfis"].as_array().cloned().unwrap_or_default();
                    let mut list = list;
                    list.push(json!(item.id));
                    f["measured"]["rfis"] = json!(list);
                    f["measured"]["rfi"] = json!(item.id);
                }
            }
            continue;
        }
        disputed.push(item.id.clone());
        let ty = tys.first().cloned().unwrap_or_default();
        let working: Vec<Value> = tys
            .iter()
            .map(|t| match verdicts.get(&(t.clone(), check)) {
                Some((v, why)) => json!({"type": t, "validate": v, "why": why}),
                None => json!({"type": t, "validate": "none", "why": format!("Validate ran no {check} check of type {t} (out of scope, or no schedule row or submittal read for it)")}),
            })
            .collect();
        let verdict_words = working
            .iter()
            .map(|w| {
                format!(
                    "{} {}: {}",
                    w["type"].as_str().unwrap_or_default(),
                    w["validate"].as_str().unwrap_or_default(),
                    w["why"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        out.findings.push((
            rank("extra", &sev),
            "extra",
            json!({
                "id": format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|rfi|{}", item.id))), "rule": RULE, "handle": Value::Null,
                "kind": "luminaire_type", "mark": ty, "tag": ty, "verdict": "extra",
                "issue": format!("the build's RFI {} ({}: type {}) has no matching failure: Validate's {check} check says {verdict_words}", item.id, item.topic, tys.join(", ")),
                "expected": "an RFI only where Validate's own reading fails", "found": quote(&item.issue),
                "measured": {"check": check, "rfi": item.id, "topic": item.topic, "types": tys, "working": working, "sides": sides},
                "evidence": [], "confidence": 1.0, "severity": sev, "area": Value::Null, "level": item.level, "at": Value::Null, "source": "prescreen",
            }),
        ));
    }
    for (ty, check, issue) in &fails {
        if raised.contains(&(ty.clone(), *check)) {
            continue;
        }
        out.findings.push((
            rank("missing", &sev),
            "missing",
            json!({
                "id": format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|norfi|{ty}|{check}"))), "rule": RULE, "handle": Value::Null,
                "kind": "luminaire_type", "mark": ty, "tag": ty, "verdict": "missing",
                "issue": format!("Validate fails type {ty}'s {check} check and the build raised no lighting RFI for it: {issue}"),
                "expected": format!("a lighting RFI about type {ty} ({check})"), "found": "none",
                "measured": {"check": check, "type": ty, "rfi": Value::Null},
                "evidence": [], "confidence": 1.0, "severity": sev, "area": Value::Null, "level": "", "at": Value::Null, "source": "prescreen",
            }),
        ));
    }
    out.table = json!({
        "types": table,
        "schedule": {"tables": docs.tables.iter().map(|t| json!({"sheet": t["sheet"], "title": t["title"], "headers": t["headers"], "sides": t["sides"], "types": t["types"]})).collect::<Vec<_>>(),
                     "types": docs.rows.len(), "pages_read": docs.pages_read, "no_substitutions": docs.no_substitutions},
        "submittals": {"files": docs.files, "types": docs.products.len()},
        "plan_from": plan_from,
        "checks": CHECKS,
        "rfis": {"confirmed": confirmed, "disputed": disputed, "not_recomputed": not_recomputed},
        "source": data().source,
    });
    out
}

/// The submittal check of one scheduled type: maker and series against the row, the product
/// approved.
fn submittal_outcome(ty: &str, r: &SchedRow, p: &SubProduct, no_subs: Option<&str>) -> TypeOutcome {
    let maker = (!r.maker.is_empty() && !p.maker.is_empty()).then(|| same_maker(&r.maker, &p.maker));
    let series = same_series(&r.model, &p.catalog);
    let family_of = |m: &str| family(m).map(str::to_string);
    let working = json!({
        "row": {"source": r.source(), "manufacturer": r.maker, "model": r.model, "family": family_of(&r.maker), "series": series_word(&r.model)},
        "submittal": {"cite": p.cite, "manufacturer": p.maker, "catalog": p.catalog, "family": family_of(&p.maker), "series": series_word(&p.catalog), "status": p.status, "revision": format!("{}-{}", p.revision.0, p.revision.1), "others": p.others},
        "maker_same": maker, "series_same": series, "no_substitutions": no_subs,
    });
    let expected = format!("an approved product of {}", quote(&r.basis()));
    if p.approved() == Some(false) {
        return TypeOutcome {
            check: "submittal",
            verdict: "fail",
            issue: format!("type {ty}: its only lighting submittal ({}) was sent back ({}): no approved product", p.cite, p.status),
            expected,
            found: format!("{} ({})", p.words(), p.status),
            working,
        };
    }
    let mut diffs = Vec::new();
    if maker == Some(false) {
        diffs.push(format!("maker {} where the schedule names {}", quote(&p.maker), quote(&r.maker)));
    }
    if series == Some(false) {
        diffs.push(format!("catalog {} is not the schedule's series {}", quote(&p.catalog), quote(&r.model)));
    }
    let subs = no_subs.map(|n| format!("; the schedule's notes say: {n}")).unwrap_or_default();
    if !diffs.is_empty() {
        return TypeOutcome {
            check: "submittal",
            verdict: "fail",
            issue: format!(
                "type {ty}: the submittal ({}) gives {} against the schedule ({}): {}{subs}",
                p.cite,
                p.words(),
                r.source(),
                diffs.join("; ")
            ),
            expected,
            found: p.words(),
            working,
        };
    }
    let stamp = if p.approved() == Some(true) { String::new() } else { " (its review stamp was not read)".to_string() };
    match (maker, series) {
        (Some(true), Some(true)) => TypeOutcome {
            check: "submittal",
            verdict: "pass",
            issue: format!("type {ty}: the submittal ({}) gives {}, the schedule's maker and series{stamp}", p.cite, p.words()),
            expected,
            found: p.words(),
            working,
        },
        _ => {
            let missing =
                [maker.is_none().then_some("maker"), series.is_none().then_some("catalog number")].into_iter().flatten().collect::<Vec<_>>();
            TypeOutcome {
                check: "submittal",
                verdict: "unsure",
                issue: format!(
                    "type {ty}: the {} can't be compared (the schedule {} / the submittal {}){stamp}",
                    missing.join(" and "),
                    quote(&r.basis()),
                    quote(&p.words())
                ),
                expected,
                found: p.words(),
                working,
            }
        }
    }
}

fn series_word(v: &str) -> Option<String> {
    Some(series(v)).filter(|s| !s.is_empty())
}

/// An issue with its type taken out, to count alike reasons together.
fn generic(issue: &str) -> String {
    let t = issue.split_once(": ").map_or(issue, |(_, rest)| rest);
    t.split(" (").next().unwrap_or(t).trim().chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(headers: &[&str], rows: &[&[&str]]) -> GridTable {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        GridTable { headers: s(headers), rows: rows.iter().map(|r| s(r)).collect(), sheet: "X-601".into() }
    }

    #[test]
    fn columns_are_named_by_their_words_in_any_order() {
        let t = table(
            &["VOLTS", "DESCRIPTION", "LAMP TYPE", "MFR.", "CATALOG NUMBER", "MOUNTING", "TYPE", "INPUT VA"],
            &[&["277", "2X4 TROFFER", "LED", "ACME", "AT-24-L40", "RECESSED", "T1", "38"]],
        );
        let (rows, _) = rows_of(&t, "LUMINAIRE SCHEDULE");
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!((r.tag.as_str(), r.maker.as_str(), r.model.as_str(), r.voltage.as_str()), ("T1", "ACME", "AT-24-L40", "277"));
        assert_eq!((r.lamp.as_str(), r.installation.as_str(), r.va), ("LED", "RECESSED", Some(38.0)));
        assert_eq!(r.source(), "X-601 LUMINAIRE SCHEDULE, type T1");
    }

    #[test]
    fn a_two_sided_schedule_is_read_half_by_half() {
        let t = table(
            &["MARK", "MANUFACTURER", "MODEL", "VOLTAGE", "MARK", "MANUFACTURER", "MODEL", "VOLTAGE"],
            &[&["P1", "ACME", "PX-1", "120", "P5", "OTHER CO", "OC-9", "277"], &["P2", "ACME", "PX-2", "120", "", "", "", ""]],
        );
        let (rows, _) = rows_of(&t, "LIGHT FIXTURE SCHEDULE");
        let tags: Vec<&str> = rows.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(tags, ["P1", "P5", "P2"]);
        let p5 = rows.iter().find(|r| r.tag == "P5").unwrap();
        assert_eq!((p5.maker.as_str(), p5.voltage.as_str()), ("OTHER CO", "277"));
    }

    #[test]
    fn a_rows_second_line_continues_it_and_notes_do_not() {
        assert_eq!(maker_of("ACME LIGHTING LAMP: BRAND X 2700K"), "ACME LIGHTING");
        let t = table(
            &["TYPE", "DESCRIPTION", "LAMP"],
            &[&["W2", "WALL SCONCE", "LED"], &["", "WITH DIFFUSER", "9VA"], &["", "NOTES: 1. NO SUBSTITUTIONS WITHOUT APPROVAL", ""]],
        );
        let (rows, subs) = rows_of(&t, "LIGHTING FIXTURE SCHEDULE");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].description, "WALL SCONCE WITH DIFFUSER");
        assert_eq!(rows[0].va, Some(9.0), "the VA on the lamp cell's second line");
        assert!(subs.is_some_and(|s| s.contains("NO SUBSTITUTIONS")));
    }

    #[test]
    fn a_table_with_no_fixture_header_is_not_a_schedule() {
        // Lettering a grid caught under a title that names the schedule (a plan's notes).
        let long = "10. COORDINATE ALL CEILING MOUNTED EQUIPMENT WITH CASEWORK BELOW. 11. PROVIDE ACCESS PANELS";
        let t = table(&["1", "EQ", long, "7'-6\" A.F.F."], &[&["C", "", "", ""], &["E", "", "", ""]]);
        assert!(rows_of(&t, "LIGHT FIXTURE SCHEDULE").0.is_empty());
        // A tag column alone is not enough either.
        let t = table(&["TYPE", "ROOM"], &[&["T1", "101"]]);
        assert!(rows_of(&t, "LIGHT FIXTURE SCHEDULE").0.is_empty());
    }

    #[test]
    fn tags_have_a_letter_and_a_height_is_no_mounting() {
        assert_eq!(tag_cell("1"), None, "a note's number");
        assert_eq!(tag_cell("C.8"), None, "a grid line's name");
        assert_eq!(tag_cell("la-2").as_deref(), Some("LA-2"));
        let row = SchedRow { installation: "8'-0\" AFF".into(), ..SchedRow::default() };
        assert_eq!(agrees("installation", "wall", &row), None, "a height can't be compared with a mounting");
        let row = SchedRow { installation: "SURFACE/CEILING".into(), ..SchedRow::default() };
        assert_eq!(agrees("installation", "wall", &row), Some(false));
        assert_eq!(agrees("installation", "ceiling", &row), Some(true));
        let full = SchedRow { maker: "ACME".into(), model: "AT-24".into(), va: Some(38.0), ..SchedRow::default() };
        assert!(full.filled() > SchedRow { tag: "T1".into(), ..SchedRow::default() }.filled(), "the fuller row wins");
    }

    #[test]
    fn values_read_and_compare() {
        assert_eq!(va_of("LED 13VA"), Some(13.0));
        assert_eq!(va_of("2 x 6.5 W"), Some(6.5));
        assert_eq!(va_of("38"), Some(38.0));
        assert_eq!(va_of("LED"), None);
        assert!(volts("120/277V").contains(&277));
        assert!(volts("MVOLT").contains(&120));
        assert!(same_maker("ACME", "Acme Lighting Co"));
        assert!(!same_maker("ACME", "Other Co"));
        assert!(same_maker("HOLOPHANE", "GOTHAM"), "brands of one maker");
        assert!(!same_maker("PHILIPS", "HALO"));
        assert_eq!(same_series("AT-24-L40-MVOLT", "AT24L35"), Some(true), "one series, other options");
        assert_eq!(same_series("AT-24", "ZZ-900"), Some(false));
        assert_eq!(same_series("", "ZZ-900"), None);
        assert_eq!(mounting_agrees("wall", "SURFACE"), Some(true), "a bare surface mounting may be on a wall");
        assert_eq!(mounting_agrees("wall", "SURFACE/CEILING"), Some(false));
        assert_eq!(mounting_agrees("RECESSED", "CEILING"), Some(true));
        assert_eq!(mounting_agrees("PENDANT", "RECESSED"), Some(false));
        assert_eq!(mounting_agrees("8'-0\" AFF", "WALL"), None);
        assert_eq!(type_of("luminaire XA").as_deref(), Some("XA"));
        assert_eq!(type_of("A ROOM NAME"), None);
        assert!(lighting_file("/subs/Interior Lighting/Rev 1-0/Package.pdf"));
        assert!(lighting_file("/subs/Submittal 26 51 13-1.pdf"));
        assert!(!lighting_file("/subs/Submittal 23 81 26 Units.pdf"));
    }

    #[test]
    fn rfis_name_their_types_and_checks() {
        let rfi = |about: &str, topic: &str| cadaddy_walk3d::Rfi { about: about.into(), topic: topic.into(), ..cadaddy_walk3d::Rfi::default() };
        let (t, c) = rfi_subject(&rfi("lighting type Q1", "lighting: catalog number")).unwrap();
        assert_eq!((t, c), (vec!["Q1".to_string()], Ok("submittal")));
        let (t, _) = rfi_subject(&rfi("lighting types Q3, Q4", "lighting: one product for two types")).unwrap();
        assert_eq!(t, ["Q3", "Q4"]);
        assert_eq!(rfi_subject(&rfi("lighting type Q2", "lighting: CCT")).unwrap().1, Err("cct".to_string()));
        assert!(rfi_subject(&rfi("room 12 leg a", "lighting: switch leg with no fixture")).is_none());
    }
}
