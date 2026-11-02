//! The print inventory (`bim.inventory`): every mark on every sheet the job read, what it is and
//! what became of it.
//!
//! A mark is a symbol, a tag, a line of lettering, a dimension, a fill, a callout bubble, a legend
//! or schedule row, or a sheet's line work of one layer and line type. Each is either an OBJECT of
//! a kind (a receptacle, a plumbing fixture, a room, a wall) that links to the model elements it
//! became, or says why none (not placeable, a duplicate of a mark on another sheet with the link
//! to it, out of scope); or NOT AN OBJECT, with why (a dimension, a note, a title, a view bubble,
//! a keynote reference, a grid). What no rule explains is `unexplained`: the readers' next work.
//!
//! The job takes the inventory at the end of its BIM build ([`take`]) from what its readers
//! decided ([`MarkFate`], by the mark's handle) and the elements the build made (each carries the
//! handle of the mark it came from), and keeps it in the drawing (`print-inventory.json`).
//! `bim.inventory` lists the marks by sheet, level, kind and status, with the coverage.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::device_types::{self, Legend};
use cadaddy_mepfix::printlinks::PageRead;
use cadaddy_mepfix::sheet_facts::{self, TextItem};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::guided::{GuidedJob, SheetRegion};
use super::*;
use crate::{EngineError, Result, Session};

const CMD: &str = "bim.inventory";
/// Where the drawing keeps the inventory.
const FILE: &str = "print-inventory.json";

/// An object mark that became model elements.
pub const BUILT: &str = "built";
/// An object mark whose sheet doesn't line up with a plan of its level: not in the model.
pub const NOT_PLACEABLE: &str = "not_placeable";
/// The same object as a mark on another sheet (`same_as`): that one is the model's.
pub const DUPLICATE: &str = "duplicate";
/// An object on a sheet of no building level (site, civil) or outside the building.
pub const OUT_OF_SCOPE: &str = "out_of_scope";
/// An object mark the build left out for a reason it gives.
pub const NOT_BUILT: &str = "not_built";
/// Not an object (a dimension, a note, a title…), with why.
pub const NOT_OBJECT: &str = "not_object";
/// No rule explains it.
pub const UNEXPLAINED: &str = "unexplained";

/// Most marks kept (the rest are counted, by sheet).
const MAX_MARKS: usize = 1_000_000;
/// Most element handles listed on one mark.
const MAX_ELEMENTS: usize = 40;
/// Most characters of a mark's text kept.
const MAX_TEXT: usize = 60;
/// Most texts per sheet read for its legend.
const MAX_SHEET_TEXTS: usize = 200_000;
/// Marks listed per answer at most, and by default.
const PAGE_MAX: usize = 200;
const PAGE_DEFAULT: usize = 25;
/// Unexplained marks listed in the summary (the commonest texts first).
const TOP_UNEXPLAINED: usize = 25;
/// A callout bubble's lettering lies within this many radii of its centre.
const BUBBLE_REACH: f64 = 1.6;

/// What a reader decided about a mark (by its handle): an object of a kind, and what became of
/// it, or not an object and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MarkFate {
    #[serde(default)]
    pub object: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// One of the statuses ([`BUILT`] …); empty: decided by the elements it became.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// For a duplicate: the handle (hex) of the mark kept.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub same_as: String,
}

impl MarkFate {
    /// An object of `kind` (its status by the elements it became).
    pub fn object(kind: &str, reason: &str) -> Self {
        Self { object: true, kind: kind.into(), reason: reason.into(), ..Self::default() }
    }

    /// An object of `kind` left out: `status` and why.
    pub fn left_out(kind: &str, status: &str, reason: &str) -> Self {
        Self { object: true, kind: kind.into(), status: status.into(), reason: reason.into(), ..Self::default() }
    }

    /// The same object as the mark `of` (a handle) shows.
    pub fn duplicate(kind: &str, of: &str, reason: &str) -> Self {
        Self { object: true, kind: kind.into(), status: DUPLICATE.into(), reason: reason.into(), same_as: of.into() }
    }

    /// Not an object, and why.
    pub fn not_object(reason: &str) -> Self {
        Self { status: NOT_OBJECT.into(), reason: reason.into(), ..Self::default() }
    }

    /// Nothing says what it is.
    pub fn unexplained(reason: &str) -> Self {
        Self { status: UNEXPLAINED.into(), reason: reason.into(), ..Self::default() }
    }
}

/// One mark on a sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub id: usize,
    pub sheet: String,
    /// The sheet's discipline letter (`E`, `P`, `A`…).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub trade: String,
    /// `symbol`, `tag`, `text`, `dimension`, `hatch`, `callout`, `legend_row`, `line_type`, `mask`,
    /// `image`, `leader`, `block`, `other`.
    pub what: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Where it is printed (drawing units, the sheet's place in the job's drawing).
    pub at: [f64; 2],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub view: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub level: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub handle: String,
    #[serde(default)]
    pub object: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// The model elements it became (handles, hex; the first [`MAX_ELEMENTS`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<String>,
    /// A duplicate's kept mark (its id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_as: Option<usize>,
    /// The kept mark's duplicates on other sheets (their ids): one object, all its marks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_on: Vec<usize>,
    /// A line-work mark: how many pieces, and how many of them became elements.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub count: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub built_count: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A sheet of the set the job didn't load (a schedule, a detail or a riser sheet): its marks are
/// not in the inventory.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetNotRead {
    pub number: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

/// Every mark of the sheets the job read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub marks: Vec<Mark>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sheets_not_read: Vec<SheetNotRead>,
    /// Marks past [`MAX_MARKS`], not kept.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dropped: usize,
}

/// The coverage of a set of marks.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Coverage {
    pub marks: usize,
    pub classified: usize,
    pub object_marks: usize,
    pub objects: usize,
    pub built: usize,
    pub unexplained: usize,
}

impl Coverage {
    fn add(&mut self, m: &Mark) {
        self.marks += 1;
        if m.status != UNEXPLAINED {
            self.classified += 1;
        }
        if m.object {
            self.object_marks += 1;
            if m.same_as.is_none() {
                self.objects += 1;
            }
            if m.status == BUILT {
                self.built += 1;
            }
        }
        if m.status == UNEXPLAINED {
            self.unexplained += 1;
        }
    }

    fn value(&self) -> Value {
        let pct = |a: usize, b: usize| if b == 0 { 100.0 } else { (1000.0 * a as f64 / b as f64).round() / 10.0 };
        json!({
            "marks": self.marks,
            "classified": self.classified,
            "classified_pct": pct(self.classified, self.marks),
            "object_marks": self.object_marks,
            "objects": self.objects,
            "objects_built": self.built,
            "built_pct": pct(self.built, self.object_marks),
            "unexplained": self.unexplained,
        })
    }
}

impl Inventory {
    /// Coverage: all, by trade, by sheet; the not-object marks by reason; the object marks by
    /// kind and status; the commonest unexplained marks.
    pub fn summary(&self, by_sheet: bool) -> Value {
        let mut all = Coverage::default();
        let mut trades: BTreeMap<&str, Coverage> = BTreeMap::new();
        let mut sheets: BTreeMap<&str, Coverage> = BTreeMap::new();
        let mut not_object: BTreeMap<&str, usize> = BTreeMap::new();
        let mut objects: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
        let mut unexplained: BTreeMap<(&str, &str, &str), usize> = BTreeMap::new();
        for m in &self.marks {
            all.add(m);
            trades.entry(if m.trade.is_empty() { "?" } else { &m.trade }).or_default().add(m);
            if by_sheet {
                sheets.entry(&m.sheet).or_default().add(m);
            }
            match m.status.as_str() {
                NOT_OBJECT => *not_object.entry(&m.reason).or_default() += 1,
                UNEXPLAINED => *unexplained.entry((&m.what, &m.text, &m.reason)).or_default() += m.count.max(1),
                _ => {}
            }
            if m.object {
                *objects.entry(&m.kind).or_default().entry(&m.status).or_default() += 1;
            }
        }
        let mut top: Vec<((&str, &str, &str), usize)> = unexplained.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut not_object: Vec<(&str, usize)> = not_object.into_iter().collect();
        not_object.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let mut v = json!({
            "coverage": all.value(),
            "by_trade": trades.iter().map(|(t, c)| (t.to_string(), c.value())).collect::<serde_json::Map<String, Value>>(),
            "not_object_by_reason": not_object.iter().map(|(r, n)| json!({"reason": r, "marks": n})).collect::<Vec<_>>(),
            "objects_by_kind": objects,
            "top_unexplained": top.iter().take(TOP_UNEXPLAINED).map(|((w, t, r), n)| json!({"what": w, "text": t, "reason": r, "marks": n})).collect::<Vec<_>>(),
            "sheets_not_read": self.sheets_not_read.len(),
        });
        if by_sheet {
            v["by_sheet"] = sheets.iter().map(|(t, c)| (t.to_string(), c.value())).collect::<serde_json::Map<String, Value>>().into();
        }
        if self.dropped > 0 {
            v["marks_not_kept"] = json!(self.dropped);
        }
        v
    }
}

/// The inventory the drawing keeps, if any.
pub fn stored(d: &Drawing) -> Option<Inventory> {
    d.embedded.get(FILE).and_then(|b| serde_json::from_slice(b).ok())
}

/// Keep `inv` in the drawing.
pub fn store(d: &mut Drawing, inv: &Inventory) -> Result<()> {
    let bytes = serde_json::to_vec(inv).map_err(|e| EngineError::Other(format!("{CMD}: {e}")))?;
    d.embedded.insert(FILE.into(), Arc::new(bytes));
    Ok(())
}

/// The elements the build made from each mark: by the mark's handle, (element handle, kind).
fn elements_by_source(d: &Drawing) -> HashMap<u64, Vec<(cadaddy_doc::Handle, String)>> {
    let mut out: HashMap<u64, Vec<(cadaddy_doc::Handle, String)>> = HashMap::new();
    for e in d.model.iter() {
        let Some(tags) = super::bimlook::made_tags(e) else { continue };
        let Some(source) = tags.get(2).and_then(|t| u64::from_str_radix(t.value.trim(), 16).ok()).filter(|h| *h != 0) else { continue };
        let kind = super::elemprops::element_kind(e).or_else(|| tags.first().map(|t| t.value.clone())).unwrap_or_default();
        out.entry(source).or_default().push((e.handle, kind));
    }
    out
}

/// A sheet the job read: its objects (handles `lo..hi`), its regions, and what was read off it.
struct Sheet<'a> {
    name: String,
    base: String,
    trade: String,
    regions: Vec<&'a SheetRegion>,
    page: Option<&'a PageRead>,
    legend: Legend,
    /// The rooms of its levels: (number, name) — their tags are the rooms' marks.
    rooms: Vec<(String, String)>,
    /// Every room number and name the job read (upper case): on another sheet, a copy.
    all_rooms: &'a BTreeSet<String>,
}

impl Sheet<'_> {
    /// The region of this sheet holding `at`: its view's, else the sheet's.
    fn region(&self, at: [f64; 2]) -> Option<&SheetRegion> {
        let inside = |r: &[f64; 4]| at[0] >= r[0].min(r[2]) && at[0] <= r[0].max(r[2]) && at[1] >= r[1].min(r[3]) && at[1] <= r[1].max(r[3]);
        self.regions
            .iter()
            .copied()
            .find(|r| r.view.is_some() && r.in_view(at))
            .or_else(|| self.regions.iter().copied().find(|r| inside(&r.rect)))
            .or_else(|| self.regions.first().copied())
    }

    fn view(&self, at: [f64; 2]) -> String {
        if let Some(v) = self.region(at).and_then(|r| r.view.as_ref()) {
            return v.title.clone();
        }
        let inside = |r: &[f64; 4]| at[0] >= r[0] && at[0] <= r[2] && at[1] >= r[1] && at[1] <= r[3];
        self.page.and_then(|p| p.views.iter().find(|v| v.region.as_ref().is_some_and(inside))).map_or_else(String::new, |v| v.title.clone())
    }

    fn level(&self, at: [f64; 2]) -> String {
        self.region(at).map_or_else(String::new, |r| r.all_levels().join(","))
    }
}

/// The PDF import's names (`cadaddy_io::pdf_read`): the style of its OCR readings of stroked
/// lettering, the layer of the readings it kept for review, the layer of the strokes it read.
const OCR_TEXT_STYLE: &str = "PDF-SHX-OCR";
const OCR_REVIEW_LAYER: &str = "PDF-OCR-REVIEW";
const SHX_SOURCE_LAYER: &str = "PDF-SHX-SOURCE";

/// Drafting abbreviations a note or a dimension carries: lettering, never an object.
const ABBREVIATIONS: [&str; 22] = [
    "TYP", "TYP.", "(TYP)", "(TYP.)", "SIM", "SIM.", "(SIM)", "(SIM.)", "EQ", "EQ.", "RE:", "VARIES", "UNO", "U.N.O.", "NIC", "N.I.C.", "OPP",
    "OPP.", "VIF", "V.I.F.", "CL", "C.L.",
];

/// Whether an OCR reading reads like something: a type tag, a dimension, a number, or a word of
/// three letters or more with a vowel in it.
fn legible(t: &str) -> bool {
    let t = t.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_alphanumeric() || " -./#\"'(),&:".contains(c)) {
        return false;
    }
    let word = |w: &str| w.len() >= 3 && w.chars().all(|c| c.is_ascii_alphabetic()) && w.chars().any(|c| "AEIOUYaeiouy".contains(c));
    sheet_facts::is_type_tag(t)
        || dimension_text(t)
        || (t.chars().any(|c| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_digit() || ".,- ".contains(c)))
        || t.split(|c: char| !c.is_ascii_alphabetic()).any(word)
}

/// What a line of lettering (not one a reader took) is: (what, reason) when a rule says; `None`
/// when none does. `style` and `layer` are the text's own (an OCR reading is marked by both).
fn lettering(
    value: &str,
    at: [f64; 2],
    height: f64,
    style: &str,
    layer: &str,
    sheet: &Sheet,
    sheet_numbers: &BTreeSet<String>,
) -> (&'static str, Option<String>) {
    let p = Vec2::new(at[0], at[1]);
    // The import's guesses at stroked lettering that read like no word (a tick, a corner or a
    // hatch's strokes misread as letters).
    let ocr = style.eq_ignore_ascii_case(OCR_TEXT_STYLE);
    if layer.eq_ignore_ascii_case(OCR_REVIEW_LAYER) || (ocr && !legible(value)) {
        return ("text", Some("an OCR reading of strokes that reads as no word".into()));
    }
    if ocr && value.trim().len() <= 2 && value.trim().chars().all(|c| c.is_ascii_alphabetic()) {
        return ("text", Some("an OCR reading of a lone letter or two (a stroke or a corner as often as a tag)".into()));
    }
    let region = sheet.region(at);
    if let Some(tb) = region.and_then(|r| r.title_block) {
        let inside = at[0] >= tb[0].min(tb[2]) && at[0] <= tb[0].max(tb[2]) && at[1] >= tb[1].min(tb[3]) && at[1] <= tb[1].max(tb[3]);
        if inside {
            return ("text", Some("title block".into()));
        }
    }
    if region.is_some_and(|r| r.area.is_some() && !r.in_area(at)) {
        return ("text", Some("sheet border or margin".into()));
    }
    if sheet.legend.covers(p) {
        return ("legend_row", Some("legend or schedule row".into()));
    }
    let t = value.trim();
    let up = t.to_ascii_uppercase();
    if let Some(page) = sheet.page {
        if let Some(c) =
            page.callouts.iter().find(|c| c.radius.is_finite() && c.radius > 0.0 && p.dist(Vec2::new(c.at[0], c.at[1])) <= BUBBLE_REACH * c.radius)
        {
            let kind = if c.kind.is_empty() { "view" } else { c.kind.as_str() };
            return ("callout", Some(format!("view bubble ({kind} callout)")));
        }
        // A grid's label, in its bubble on the grid line's end.
        let reach = 3.0 * if height.is_finite() && height > 0.0 { height } else { 1.0 };
        let on_x = page
            .grid_x
            .iter()
            .chain(page.views.iter().flat_map(|v| v.grid_x.iter()))
            .any(|(l, x)| l.eq_ignore_ascii_case(t) && (at[0] - x).abs() <= reach);
        let on_y = page
            .grid_y
            .iter()
            .chain(page.views.iter().flat_map(|v| v.grid_y.iter()))
            .any(|(l, y)| l.eq_ignore_ascii_case(t) && (at[1] - y).abs() <= reach);
        if t.len() <= 3 && (on_x || on_y) {
            return ("text", Some("grid bubble".into()));
        }
        if page
            .views
            .iter()
            .any(|v| !v.title.is_empty() && (v.title.eq_ignore_ascii_case(t) || (t.len() >= 6 && v.title.to_ascii_uppercase().contains(&up))))
            || page.views.iter().any(|v| v.scale_text.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(t)))
        {
            return ("text", Some("view title or scale".into()));
        }
    }
    if up.starts_with("SCALE") || up.contains("= 1'-0\"") || up.contains("=1'-0\"") || up == "N" || up == "NORTH" {
        return ("text", Some("view title or scale".into()));
    }
    if up.contains("MATCH LINE") || up.contains("MATCHLINE") {
        return ("text", Some("match line".into()));
    }
    if dimension_text(t) {
        return ("dimension", Some("dimension".into()));
    }
    if sheet_numbers.contains(t) || sheet_numbers.contains(&up) {
        return ("text", Some("sheet reference".into()));
    }
    if super::guided::circuit_text(t, sheet_numbers) || sheet_facts::is_circuit_label(t) {
        return ("text", Some("circuit or panel label (a device's wiring)".into()));
    }
    if sheet
        .rooms
        .iter()
        .any(|(n, name)| (!n.is_empty() && n.eq_ignore_ascii_case(t)) || (!name.is_empty() && t.len() >= 4 && name.eq_ignore_ascii_case(t)))
    {
        return ("tag", None);
    }
    if t.len() >= 2 && sheet.all_rooms.contains(&up) {
        return ("text", Some("a room's tag on another trade's plan or view (the room is its architectural plan's)".into()));
    }
    if super::readers::switch_designation(t).is_some() {
        return ("text", Some("switch leg letter".into()));
    }
    if t.chars().all(|c| c.is_ascii_digit()) && (1..=3).contains(&t.len()) {
        return ("text", Some("keynote or view number".into()));
    }
    if ABBREVIATIONS.iter().any(|a| a.eq_ignore_ascii_case(t)) {
        return ("text", Some("drafting abbreviation (typ., sim., eq.)".into()));
    }
    let numeric = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit() || c == '.') && w.chars().any(|c| c.is_ascii_digit());
    if t.ends_with('.') && t.len() <= 4 && t.trim_end_matches('.').chars().all(|c| c.is_ascii_alphanumeric()) {
        return ("text", Some("note number".into()));
    }
    if t.split_whitespace().all(numeric) {
        return ("text", Some("a number (a spot elevation, an offset or a value)".into()));
    }
    let words: Vec<&str> = up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if words.iter().any(|w| *w == "AFF" || *w == "A.F.F") {
        return ("text", Some("mounting height note".into()));
    }
    if words.last().is_some_and(|w| matches!(*w, "SF" | "SQ" | "SQFT")) && words.first().is_some_and(|w| numeric(w)) {
        return ("text", Some("area label".into()));
    }
    let words = t.split_whitespace().filter(|w| w.chars().any(|c| c.is_ascii_alphabetic())).count();
    if words >= 3 || (words >= 2 && t.len() >= 12) || t.len() >= 24 {
        return ("text", Some("note".into()));
    }
    if !height.is_finite() || height <= 0.0 {
        return ("text", Some("lettering of no size".into()));
    }
    if sheet_facts::is_type_tag(t) {
        return ("tag", None);
    }
    if t.len() >= 5 && t.chars().all(|c| c.is_ascii_alphabetic()) {
        return ("text", Some("label (a word naming what is drawn)".into()));
    }
    if !t.chars().any(|c| c.is_ascii_alphanumeric()) {
        return ("text", Some("punctuation (a leader's or a dimension's mark)".into()));
    }
    ("text", None)
}

/// Feet and inches, a size or a slope as a dimension prints it: `12'-6"`, `3'`, `1/2"`, `4 1/2"`.
fn dimension_text(t: &str) -> bool {
    let t = t.trim();
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    let unit = t.contains('\'') || t.contains('"');
    has_digit && unit && t.chars().all(|c| c.is_ascii_digit() || " '\"-/.±+".contains(c)) && t.len() <= 16
}

/// The rooms the job read on each level code (number, name), for their tags.
fn rooms_by_level(job: &GuidedJob) -> BTreeMap<String, Vec<(String, String)>> {
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let rooms = job.stage(super::guided::StageName::Rooms).and_then(|st| st.result.as_ref()).and_then(|r| r.get("rooms")).and_then(Value::as_array);
    for r in rooms.into_iter().flatten().take(100_000) {
        let text = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let number = text("number");
        let name = text("name");
        if number.is_empty() && name.is_empty() {
            continue;
        }
        out.entry(text("level")).or_default().push((number, name));
    }
    out
}

/// Take the inventory of the job's sheets: `pages` is what the sheet map read off each (by the
/// sheet's name in the job), `fates` what the readers decided (by handle).
pub(crate) fn take(d: &Drawing, job: &GuidedJob, pages: &BTreeMap<String, PageRead>, fates: &HashMap<u64, MarkFate>) -> Inventory {
    let mut inv = Inventory::default();
    let built = elements_by_source(d);
    let mut spans = job.sheet_spans.clone();
    spans.sort_by_key(|sp| sp.lo);
    let group_of = |h: u64| -> Option<usize> {
        let i = spans.partition_point(|sp| sp.lo <= h);
        let k = i.checked_sub(1)?;
        spans.get(k).filter(|sp| h < sp.hi).map(|_| k)
    };
    // Each sheet's objects, and its lettering for its legend and its number.
    let mut members: Vec<Vec<&Entity>> = vec![Vec::new(); spans.len()];
    let mut texts: Vec<Vec<TextItem>> = vec![Vec::new(); spans.len()];
    for e in d.model.iter() {
        let Some(g) = group_of(e.handle.0) else { continue };
        if let Some(v) = members.get_mut(g) {
            v.push(e);
        }
        if let (EntityKind::Text(t), Some(v)) = (&e.kind, texts.get_mut(g))
            && v.len() < MAX_SHEET_TEXTS
            && t.height.is_finite()
            && t.height > 0.0
            && !t.value.trim().is_empty()
        {
            v.push(TextItem { value: t.value.trim().to_string(), at: t.insert.xy(), height: t.height, rotation: t.rotation });
        }
    }
    let mut sheet_numbers = BTreeSet::new();
    let listed = job.sheets.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array);
    for n in listed.into_iter().flatten().filter_map(|sh| sh.get("number").and_then(Value::as_str)) {
        sheet_numbers.extend(device_types::sheet_number_forms(n));
    }
    for t in &texts {
        if let Some(id) = sheet_facts::identify_sheet(t) {
            sheet_numbers.extend(device_types::sheet_number_forms(&id.number));
        }
    }
    let rooms = rooms_by_level(job);
    let all_rooms: BTreeSet<String> =
        rooms.values().flatten().flat_map(|(n, name)| [n.to_ascii_uppercase(), name.to_ascii_uppercase()]).filter(|w| !w.is_empty()).collect();
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let sheets: Vec<Sheet> = spans
        .iter()
        .enumerate()
        .map(|(g, sp)| {
            let base = sp.sheet.split('/').next().unwrap_or(&sp.sheet).trim().to_string();
            let regions: Vec<&SheetRegion> =
                job.sheet_regions.iter().filter(|r| r.number.eq_ignore_ascii_case(&base) || r.number.eq_ignore_ascii_case(&sp.sheet)).collect();
            let levels: BTreeSet<String> = regions.iter().flat_map(|r| r.all_levels()).collect();
            let rooms: Vec<(String, String)> = rooms
                .iter()
                .filter(|(l, _)| levels.iter().any(|x| x.eq_ignore_ascii_case(l) || (!digits(x).is_empty() && digits(x) == digits(l))))
                .flat_map(|(_, v)| v.iter().cloned())
                .collect();
            let trade = regions.first().map(|r| r.discipline.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| sheet_facts::discipline_of(&base));
            Sheet {
                name: sp.sheet.clone(),
                base,
                trade: trade.to_ascii_uppercase(),
                regions,
                page: pages.get(&sp.sheet),
                legend: texts.get(g).map(|t| device_types::read_legend(t)).unwrap_or_default(),
                rooms,
                all_rooms: &all_rooms,
            }
        })
        .collect();
    let room_numbers = room_numbers(d);
    let room_outlines: Vec<(String, Vec<Vec2>, String)> =
        super::bspec::room_elements(d).into_iter().filter(|r| r.outline.len() >= 3).map(|r| (r.level, r.outline, r.handle.hex())).collect();
    for (g, sheet) in sheets.iter().enumerate() {
        // Line work by layer and line type: one mark each.
        let mut lines: BTreeMap<(String, String), Mark> = BTreeMap::new();
        for e in members.get(g).into_iter().flatten() {
            let h = e.handle.0;
            let made = built.get(&h);
            let fate = fates.get(&h);
            let line = matches!(
                e.kind,
                EntityKind::Line(_)
                    | EntityKind::LwPolyline(_)
                    | EntityKind::Polyline3d(_)
                    | EntityKind::Arc(_)
                    | EntityKind::Circle(_)
                    | EntityKind::Ellipse(_)
                    | EntityKind::Spline(_)
            );
            if line && fate.is_none() {
                let at = centre(d, e);
                let key = (e.common.layer.to_string(), e.common.linetype.to_string());
                let m = lines.entry(key.clone()).or_insert_with(|| Mark {
                    sheet: sheet.name.clone(),
                    trade: sheet.trade.clone(),
                    what: "line_type".into(),
                    text: cut(&format!("{} on layer {}", if key.1.is_empty() { "BYLAYER" } else { &key.1 }, key.0), MAX_TEXT),
                    at,
                    view: sheet.view(at),
                    level: sheet.level(at),
                    status: NOT_OBJECT.into(),
                    reason: if key.0.eq_ignore_ascii_case(SHX_SOURCE_LAYER) {
                        "strokes of lettering the import read as text".into()
                    } else {
                        "line work no element was made from (outlines, leaders, hatching, furniture)".into()
                    },
                    ..Mark::default()
                });
                m.count += 1;
                if let Some(made) = made {
                    m.built_count += 1;
                    m.object = true;
                    m.status = BUILT.into();
                    if m.kind.is_empty() {
                        m.kind = made.first().map_or_else(String::new, |(_, k)| k.clone());
                        m.reason = "line work the build traced into elements".into();
                    }
                    for (eh, _) in made {
                        if m.elements.len() < MAX_ELEMENTS {
                            m.elements.push(eh.hex());
                        }
                    }
                }
                continue;
            }
            let at = centre(d, e);
            let (what, text, height) = match &e.kind {
                EntityKind::Insert(ins) if ins.block.starts_with("PDF-SYM-") => {
                    let tag = ins.attribs.iter().find(|a| a.tag == "TAG").map(|a| a.text.value.trim().to_string()).unwrap_or_default();
                    ("symbol", tag, 1.0)
                }
                EntityKind::Insert(ins) => ("block", ins.block.clone(), 1.0),
                EntityKind::Text(t) => ("text", t.value.trim().to_string(), t.height),
                EntityKind::MText(t) => ("text", t.contents.trim().to_string(), t.height),
                EntityKind::Dimension(_) => ("dimension", String::new(), 1.0),
                EntityKind::Hatch(_) | EntityKind::Solid(_) | EntityKind::Trace(_) => ("hatch", String::new(), 1.0),
                EntityKind::Wipeout(_) => ("mask", String::new(), 1.0),
                EntityKind::Image(_) => ("image", String::new(), 1.0),
                EntityKind::Leader(_) | EntityKind::MLeader(_) => ("leader", String::new(), 1.0),
                EntityKind::Line(_)
                | EntityKind::LwPolyline(_)
                | EntityKind::Polyline3d(_)
                | EntityKind::Arc(_)
                | EntityKind::Circle(_)
                | EntityKind::Ellipse(_)
                | EntityKind::Spline(_) => ("line", String::new(), 1.0),
                _ => ("other", String::new(), 1.0),
            };
            let mut m = Mark {
                sheet: sheet.name.clone(),
                trade: sheet.trade.clone(),
                what: what.into(),
                text: cut(&text, MAX_TEXT),
                at,
                view: sheet.view(at),
                level: sheet.level(at),
                handle: e.handle.hex(),
                ..Mark::default()
            };
            if let Some(f) = fate {
                m.object = f.object;
                m.kind.clone_from(&f.kind);
                m.status.clone_from(&f.status);
                m.reason.clone_from(&f.reason);
                if what == "text" && m.object {
                    m.what = "tag".into();
                }
            } else if made.is_some() {
                m.object = true;
                m.reason = "the build made elements from it".into();
            } else {
                match what {
                    "text" => {
                        let style = match &e.kind {
                            EntityKind::Text(t) => t.style.as_str(),
                            EntityKind::MText(t) => t.style.as_str(),
                            _ => "",
                        };
                        let (w, reason) = lettering(&text, at, height, style, &e.common.layer, sheet, &sheet_numbers);
                        m.what = w.into();
                        match reason {
                            Some(r) => {
                                m.status = NOT_OBJECT.into();
                                m.reason = r;
                            }
                            None if w == "tag"
                                && !text.is_empty()
                                && sheet.rooms.iter().any(|(n, name)| n.eq_ignore_ascii_case(&text) || name.eq_ignore_ascii_case(&text)) =>
                            {
                                m.object = true;
                                m.kind = "room".into();
                                m.reason = "a room's tag".into();
                            }
                            None => {
                                m.status = UNEXPLAINED.into();
                                m.reason = if w == "tag" { "a tag no reader took".into() } else { "lettering no rule explains".into() };
                            }
                        }
                    }
                    "symbol" => {
                        m.status = UNEXPLAINED.into();
                        m.reason = "a symbol no reader decided".into();
                    }
                    "dimension" => {
                        m.status = NOT_OBJECT.into();
                        m.reason = "dimension".into();
                    }
                    "hatch" => {
                        m.status = NOT_OBJECT.into();
                        m.reason = "fill (a hatch or solid)".into();
                    }
                    "mask" => {
                        m.status = NOT_OBJECT.into();
                        m.reason = "mask (blanks out what is behind lettering)".into();
                    }
                    "image" => {
                        m.status = NOT_OBJECT.into();
                        m.reason = "raster piece of the print".into();
                    }
                    "leader" => {
                        m.status = NOT_OBJECT.into();
                        m.reason = "leader (points a note at its object)".into();
                    }
                    _ => {
                        m.status = UNEXPLAINED.into();
                        m.reason = format!("a {what} no rule explains");
                    }
                }
            }
            // A room's tag: the room element of that number on its level.
            if m.object && m.kind == "room" && made.is_none() {
                let level = digits(&m.level);
                let rooms: Vec<String> = room_numbers
                    .iter()
                    .filter(|(n, l, _)| {
                        n.eq_ignore_ascii_case(&m.text)
                            && (level.is_empty() || level.split(',').any(|x| digits(l) == x) || m.level.split(',').any(|x| digits(l) == digits(x)))
                    })
                    .map(|(_, _, h)| h.clone())
                    .take(MAX_ELEMENTS)
                    .collect();
                let rooms = if rooms.is_empty() {
                    let p = Vec2::new(m.at[0], m.at[1]);
                    room_outlines
                        .iter()
                        .filter(|(l, _, _)| m.level.split(',').any(|x| !digits(x).is_empty() && digits(l) == digits(x)))
                        .filter(|(_, o, _)| cadaddy_geom::point_in_polygon(o, p))
                        .map(|(_, _, h)| h.clone())
                        .take(MAX_ELEMENTS)
                        .collect()
                } else {
                    rooms
                };
                if rooms.is_empty() {
                    m.status = NOT_BUILT.into();
                    m.reason = "the rooms stage read it as a room's tag, but no room was built there".into();
                } else {
                    m.elements = rooms;
                }
            }
            if let Some(made) = made {
                m.elements = made.iter().take(MAX_ELEMENTS).map(|(h, _)| h.hex()).collect();
                if m.kind.is_empty() {
                    m.kind = made.first().map_or_else(String::new, |(_, k)| k.clone());
                }
                if m.object || fate.is_none_or(|f| f.status != NOT_OBJECT) {
                    m.object = true;
                    m.status = BUILT.into();
                }
            }
            if m.object && m.status.is_empty() {
                if m.elements.is_empty() {
                    m.status = UNEXPLAINED.into();
                    m.reason = format!("read as {} but in no element, and nothing says why", if m.kind.is_empty() { "an object" } else { &m.kind });
                } else {
                    m.status = BUILT.into();
                }
            }
            if m.status.is_empty() {
                m.status = UNEXPLAINED.into();
            }
            if m.object && m.kind == "room" && m.status == BUILT && m.what == "text" {
                m.what = "tag".into();
            }
            push(&mut inv, m);
        }
        for (_, m) in lines {
            push(&mut inv, m);
        }
    }
    // Duplicates: each links to the mark kept, which lists them.
    let by_handle: HashMap<&str, usize> = inv.marks.iter().filter(|m| !m.handle.is_empty()).map(|m| (m.handle.as_str(), m.id)).collect();
    let mut links: Vec<(usize, usize)> = Vec::new();
    for (h, f) in fates.iter().filter(|(_, f)| !f.same_as.is_empty()) {
        let own = by_handle.get(format!("{h:X}").as_str()).copied();
        let kept = by_handle.get(f.same_as.as_str()).copied();
        if let (Some(a), Some(b)) = (own, kept)
            && a != b
        {
            links.push((a, b));
        }
    }
    links.sort_unstable();
    for (a, b) in links {
        if let Some(m) = inv.marks.get_mut(a) {
            m.same_as = Some(b);
        }
        if let Some(k) = inv.marks.get_mut(b)
            && k.also_on.len() < MAX_ELEMENTS
        {
            k.also_on.push(a);
        }
    }
    // The sheets of the set the job didn't read.
    let read: BTreeSet<String> = sheets.iter().map(|s| s.base.to_ascii_uppercase()).collect();
    for sh in listed.into_iter().flatten() {
        let text = |k: &str| sh.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let number = text("number");
        if number.is_empty() || read.contains(&number.to_ascii_uppercase()) || inv.sheets_not_read.iter().any(|s| s.number == number) {
            continue;
        }
        let kind = if text("type").is_empty() { text("sheet_type") } else { text("type") };
        inv.sheets_not_read.push(SheetNotRead { number, title: cut(&text("title"), MAX_TEXT), kind });
    }
    inv
}

/// The model's rooms by number: (number, level, element handle).
fn room_numbers(d: &Drawing) -> Vec<(String, String, String)> {
    super::bspec::room_elements(d).into_iter().filter_map(|r| Some((r.number?, r.level, r.handle.hex()))).collect()
}

fn push(inv: &mut Inventory, mut m: Mark) {
    if inv.marks.len() >= MAX_MARKS {
        inv.dropped += 1;
        return;
    }
    m.id = inv.marks.len();
    inv.marks.push(m);
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Where an entity is printed: a text's or a symbol's insertion point, else its extents' middle.
fn centre(d: &Drawing, e: &Entity) -> [f64; 2] {
    let p = match &e.kind {
        EntityKind::Text(t) => Some(t.insert.xy()),
        EntityKind::MText(t) => Some(t.insert.xy()),
        EntityKind::Insert(i) => Some(i.insert.xy()),
        _ => None,
    };
    let p = p.filter(|p| p.x.is_finite() && p.y.is_finite()).unwrap_or_else(|| {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() { Vec2::new(0.0, 0.0) } else { Vec2::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0) }
    });
    [round1(p.x), round1(p.y)]
}

fn round1(v: f64) -> f64 {
    if v.is_finite() { (v * 10.0).round() / 10.0 } else { 0.0 }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Print Inventory", run)
            .params("{sheet?: \"E-101\", level?: \"2\", kind?: \"receptacle\", status?: \"built\"|\"not_placeable\"|\"duplicate\"|\"out_of_scope\"|\"not_built\"|\"not_object\"|\"unexplained\", what?: \"symbol\"|\"tag\"|\"text\"|\"dimension\"|\"hatch\"|\"callout\"|\"legend_row\"|\"line_type\"|…, object?: bool, reason?: text in the reason, cursor?: n, limit?: 1-200 (25), by_sheet?: bool (coverage per sheet too), rebuild?: bool (take it again from the job's sheets now)} → {coverage: {marks, classified, classified_pct, object_marks, objects, objects_built, built_pct, unexplained}, by_trade, by_sheet?, not_object_by_reason, objects_by_kind: {kind: {status: n}}, top_unexplained, sheets_not_read, total (marks matching), marks: [{id, sheet, trade, what, text, at, view, level, handle, object, kind, status, reason, elements, same_as?, also_on?, count?}], next?}: every mark on every sheet the job read — each an object of a kind (and the model elements it became, or why none) or not an object (and why)")
            .noundo(),
    ]
}

/// The drawing's inventory, taken from its job's sheets when it keeps none (or `rebuild`).
fn inventory_of(s: &mut Session, rebuild: bool) -> Result<Inventory> {
    if !rebuild && let Some(inv) = s.doc().ok().and_then(stored) {
        return Ok(inv);
    }
    let Some(job) = super::guided::load_job(s)? else {
        return Err(bad(CMD, "this drawing has no print job: run job.start on the prints first (the job's BIM build takes the inventory)"));
    };
    if job.sheet_spans.is_empty() {
        return Err(bad(CMD, "the job has read no sheets yet: the inventory is taken after its BIM build"));
    }
    let inv = super::guided::job_inventory(s, &job);
    store(s.doc_mut()?, &inv)?;
    Ok(inv)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let inv = inventory_of(s, bool_or(p, "rebuild", false))?;
    let text = |k: &str| str_param(p, k).map(str::trim).filter(|v| !v.is_empty());
    let (sheet, level, kind, status, what, reason) = (text("sheet"), text("level"), text("kind"), text("status"), text("what"), text("reason"));
    let object = p.get("object").and_then(Value::as_bool);
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let picked: Vec<&Mark> = inv
        .marks
        .iter()
        .filter(|m| sheet.is_none_or(|s| m.sheet.eq_ignore_ascii_case(s) || m.sheet.split('/').next().is_some_and(|b| b.eq_ignore_ascii_case(s))))
        .filter(|m| level.is_none_or(|l| m.level.split(',').any(|x| x.eq_ignore_ascii_case(l) || (!digits(l).is_empty() && digits(x) == digits(l)))))
        .filter(|m| kind.is_none_or(|k| m.kind.eq_ignore_ascii_case(k)))
        .filter(|m| status.is_none_or(|st| m.status.eq_ignore_ascii_case(st)))
        .filter(|m| what.is_none_or(|w| m.what.eq_ignore_ascii_case(w)))
        .filter(|m| object.is_none_or(|o| m.object == o))
        .filter(|m| reason.is_none_or(|r| m.reason.to_ascii_lowercase().contains(&r.to_ascii_lowercase())))
        .collect();
    let cursor = p.get("cursor").and_then(Value::as_u64).map_or(0, |n| n as usize).min(picked.len());
    let limit = p.get("limit").and_then(Value::as_u64).map_or(PAGE_DEFAULT, |n| (n as usize).clamp(1, PAGE_MAX));
    let mut v = inv.summary(bool_or(p, "by_sheet", false));
    v["total"] = json!(picked.len());
    let mut shown = limit;
    loop {
        let page: Vec<Value> = picked.iter().skip(cursor).take(shown).map(|m| serde_json::to_value(m).unwrap_or(Value::Null)).collect();
        let n = page.len();
        v["marks"] = Value::Array(page);
        if let Some(o) = v.as_object_mut() {
            o.remove("next");
            if cursor + n < picked.len() {
                o.insert("next".into(), json!(cursor + n));
            }
        }
        if v.to_string().len() <= super::agentready::ANSWER_MAX || shown <= 1 {
            break;
        }
        shown = n.saturating_sub(1).max(1).min(shown - 1);
    }
    // Still too big (a long by_sheet table): the sheets in short.
    if v.to_string().len() > super::agentready::ANSWER_MAX
        && let Some(o) = v.as_object_mut()
    {
        o.remove("by_sheet");
        o.insert("by_sheet_note".into(), json!("by_sheet left out to fit: ask for one sheet at a time (sheet: \"E-101\")"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dimension_prints_feet_and_inches() {
        for t in ["12'-6\"", "3'", "1/2\"", "4 1/2\"", "10'-0\""] {
            assert!(dimension_text(t), "{t}");
        }
        for t in ["Q-9", "X-12", "12", "NOTE 3", "A'B"] {
            assert!(!dimension_text(t), "{t}");
        }
    }

    #[test]
    fn coverage_counts_objects_once_and_unexplained_apart() {
        let mut inv = Inventory::default();
        let m = |what: &str, object: bool, status: &str| Mark {
            what: what.into(),
            object,
            status: status.into(),
            trade: "E".into(),
            sheet: "E-1".into(),
            ..Mark::default()
        };
        push(&mut inv, m("symbol", true, BUILT));
        push(&mut inv, Mark { same_as: Some(0), ..m("symbol", true, DUPLICATE) });
        push(&mut inv, m("dimension", false, NOT_OBJECT));
        push(&mut inv, m("text", false, UNEXPLAINED));
        let s = inv.summary(true);
        let c = &s["coverage"];
        assert_eq!((c["marks"].as_u64(), c["classified"].as_u64(), c["object_marks"].as_u64()), (Some(4), Some(3), Some(2)));
        assert_eq!((c["objects"].as_u64(), c["objects_built"].as_u64(), c["unexplained"].as_u64()), (Some(1), Some(1), Some(1)));
        assert_eq!(s["by_sheet"]["E-1"]["marks"], 4);
        assert_eq!(s["by_trade"]["E"]["unexplained"], 1);
    }
}
