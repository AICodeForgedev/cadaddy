//! The documents checked against each other (Q6648): what one document says against what another
//! gives for the same space, member, level or object. Each check alone is something the field
//! finds the hard way; the drawings are each right on their own and wrong together.
//!
//! - **Ceiling space** ([`ceiling_space`]): a note that routes work above a space's ceiling
//!   (`MAINS ABOVE THE CORRIDOR CEILING`) against the free depth over that ceiling in the model:
//!   under the lid or second ceiling over it (a higher ceiling of the same room, a lid note), else
//!   under the floor structure above (the level above less its framing and deck). The work needs
//!   its size (else the item's usual depth), its insulation and its hanger, and a drainage line
//!   its fall over the space's length.
//! - **Level datums** ([`level_datums`]): each document's level marks (`3RD FLOOR F.F.` over
//!   `121'-4 1/2"`, `LEVEL 2 = 10'-6"`), each on its own datum (100'-0" or 0'-0"), against the
//!   levels the model is built on: a trade that sets its work from other numbers builds it at the
//!   wrong height.
//! - **Design conditions** ([`conditions`]): a condition one document puts on a space (the
//!   sprinkler design leaves a cavity unsprinklered "to be fully insulated by others") against the
//!   assembly another document gives that space (its members' depth and its insulation).
//! - **Penetration groups** ([`penetrations`]): risers side by side through one wall's plate
//!   against the documents' hole rule (their own note first, else the code's and the trade's).
//! - **Site objects** ([`site`]): objects two disciplines draw on the site (a transformer pad
//!   against a meter, a vault, a water line) inside one another's clearance.
//!
//! Each finding is one conflict RFI ([`RfiKind::Conflict`], topic `documents: <check>`) that
//! cites both documents, what each says, the corrected value the field would build with its rule,
//! and the usual alternative. The ids are stable (the same conflict keeps its id and its answer
//! across rebuilds). The words and numbers are rule data (`doccheck.json`), never a job's. Every
//! check is measured: what it looked at and what it could not compare is said, never a quiet
//! zero. Every scan is capped; nothing here panics.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::levels;
use cadaddy_walk3d::rfi::{Rfi, RfiKind, RfiSide, RfiStatus, Severity};
use cadaddy_walk3d::{BuildingSpec, feet, inches};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Result, Session};

/// The rules data.
const RULES: &str = include_str!("doccheck.json");
/// The topic every finding here carries before its check's name.
pub(crate) const TOPIC: &str = "documents: ";
/// Characters of a note quoted in a finding, at most.
const MAX_QUOTE: usize = 220;
/// Sheets named in one finding, at most.
const MAX_SHEETS: usize = 8;
/// Rooms named in one finding, at most.
const MAX_ROOMS_NAMED: usize = 8;
/// Pages kept, at most.
const MAX_PAGES: usize = 2_000;
/// Lines one block runs to, at most.
const MAX_BLOCK_LINES: usize = 24;
/// Rooms and ceilings looked at, at most.
const MAX_ROOMS: usize = 50_000;

// ─── Rules ───────────────────────────────────────────────────────────────────

/// The rules (`doccheck.json`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub max_findings: usize,
    pub abbreviations: Vec<String>,
    pub max_texts: usize,
    pub max_blocks: usize,
    pub severity: String,
    pub before: String,
    pub read_again: ReadAgain,
    pub ceiling_space: CeilingRules,
    pub levels: LevelRules,
    pub conditions: Vec<ConditionRule>,
    pub assembly: AssemblyRules,
    pub penetrations: PenetrationRules,
    pub site: SiteRules,
}

/// Which pages the job reads again for these checks.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ReadAgain {
    pub kinds: Vec<String>,
    pub title_words: Vec<String>,
    pub max_pages: usize,
    pub max_shop_drawing_pages: usize,
}

/// The ceiling-space check's words and numbers.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CeilingRules {
    pub above_words: Vec<String>,
    pub ceiling_words: Vec<String>,
    pub filler_words: Vec<String>,
    pub space_max_words: usize,
    pub lid_words: Vec<String>,
    pub space_aliases: Vec<Vec<String>>,
    pub items: Vec<RoutedItem>,
    pub hanger_in: f64,
    pub default_deck_in: f64,
    pub min_ceiling_ft: f64,
    pub min_ceiling_basis: String,
    pub tolerance_in: f64,
}

/// One kind of work a note may route above a ceiling.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RoutedItem {
    pub id: String,
    pub words: Vec<String>,
    pub depth_in: f64,
    pub insulation_in: f64,
    pub slope_in_per_ft: f64,
    pub basis: String,
}

/// The level-datum check's numbers.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LevelRules {
    pub authority: Vec<String>,
    pub datums_ft: Vec<f64>,
    pub tolerance_in: f64,
    pub max_difference_ft: f64,
    pub reach_heights: f64,
}

/// A condition a document puts on a space.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ConditionRule {
    pub id: String,
    pub words: Vec<String>,
    pub needs: String,
    pub what: String,
    pub basis: String,
    pub correction: String,
    pub alternative: String,
}

/// How an assembly's cavity and insulation are read.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct AssemblyRules {
    pub space_kinds: Vec<SpaceKind>,
    pub floor_words: Vec<String>,
    pub roof_words: Vec<String>,
    pub member_words: Vec<String>,
    pub insulation_words: Vec<String>,
    pub insulation_skip_words: Vec<String>,
    pub nominal_depth_in: BTreeMap<String, f64>,
    pub r_value_in: BTreeMap<String, f64>,
    pub r_per_in: f64,
    pub max_member_in: f64,
    pub tolerance_in: f64,
}

/// The words that put a condition's space in a floor or a roof.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SpaceKind {
    pub kind: String,
    pub words: Vec<String>,
}

/// The penetration-group check's words and numbers.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PenetrationRules {
    pub hole_words: Vec<String>,
    pub member_words: Vec<String>,
    pub spacing_words: Vec<String>,
    pub default_clear_diameters: f64,
    pub default_basis: String,
    pub max_plate_fraction: f64,
    pub plate_basis: String,
    pub group_reach_in: f64,
    pub min_group: usize,
    pub min_diameter_in: f64,
    pub wall_reach_in: f64,
    pub max_runs: usize,
}

/// The site check's pairs.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SiteRules {
    pub pairs: Vec<SitePair>,
    pub site_source: String,
    pub run_disciplines: Vec<String>,
    pub run_system_prefixes: Vec<String>,
    pub max_objects: usize,
}

/// Two kinds of site object and the clearance between them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SitePair {
    pub a: Vec<String>,
    pub b: Vec<String>,
    pub clear_ft: f64,
    pub what: String,
}

/// The rules data (read once; unreadable data is no rules, never a panic: a test reads it).
pub(crate) fn rules() -> &'static Rules {
    static DATA: OnceLock<Rules> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

// ─── Pages ───────────────────────────────────────────────────────────────────

/// One document page: its sheet, discipline, the levels it shows (when any) and its text.
#[derive(Clone, Debug, Default)]
pub(crate) struct Page {
    /// `P-201`, or the file and page of a submittal (`Sprinkler Plans p.2`).
    pub sheet: String,
    /// `A`, `P`, `M`, `FP`…
    pub discipline: String,
    /// The levels it shows (empty: none or every one).
    pub levels: Vec<String>,
    pub texts: Vec<TextItem>,
}

/// A text entity's lettering, where it sits, its height and its turn.
fn text_of(e: &cadaddy_doc::Entity) -> Option<TextItem> {
    let (value, at, height, rotation) = match &e.kind {
        EntityKind::Text(t) => (t.value.as_str(), t.insert.xy(), t.height, t.rotation),
        EntityKind::MText(t) => (t.contents.as_str(), t.insert.xy(), t.height, t.rotation),
        _ => return None,
    };
    let value = value.trim();
    if value.is_empty() || !at.is_finite() || !height.is_finite() || height <= 0.0 {
        return None;
    }
    Some(TextItem { value: value.to_string(), at, height, rotation: if rotation.is_finite() { rotation } else { 0.0 } })
}

/// The pages the drawing holds: the guided job's imported sheets by the objects each added
/// (their handle ranges), else the spec's sheets by where each sits; with none, the drawing's
/// model space is one page of no discipline.
pub(crate) fn drawing_pages(d: &Drawing, sp: &BuildingSpec) -> Vec<Page> {
    let r = rules();
    let job: Option<Value> =
        d.agent.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|a| a.get("guided_job").filter(|j| !j.is_null()).cloned());
    let base = |n: &str| n.split('/').next().unwrap_or(n).trim().to_string();
    let classified: Vec<Value> = job.as_ref().and_then(|j| j.pointer("/sheets/sheets")).and_then(Value::as_array).cloned().unwrap_or_default();
    let field = |number: &str, k: &str| -> String {
        classified
            .iter()
            .find(|c| c.get("number").and_then(Value::as_str).is_some_and(|x| base(x).eq_ignore_ascii_case(number)))
            .and_then(|c| c.get(k).and_then(Value::as_str))
            .unwrap_or("")
            .to_string()
    };
    let link_levels = |number: &str| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for l in sp.sheets.iter().filter(|l| base(&l.number).eq_ignore_ascii_case(number)) {
            for v in l.level.iter().chain(l.levels.iter()) {
                if !out.contains(v) {
                    out.push(v.clone());
                }
            }
        }
        out
    };
    let page_for = |number: &str, discipline: Option<&str>| -> Page {
        let disc = discipline
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .or_else(|| Some(field(number, "discipline")).filter(|x| !x.is_empty()))
            .unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(number));
        let title = field(number, "title");
        let mut lv = link_levels(number);
        if lv.is_empty()
            && let Some(l) = levels::level_of_title(&title).and_then(|n| sp.level_matching(&n)).and_then(|i| sp.levels.get(i))
        {
            lv.push(l.name.clone());
        }
        Page { sheet: number.to_string(), discipline: disc.to_ascii_uppercase(), levels: lv, texts: Vec::new() }
    };
    let spans: Vec<(String, u64, u64)> = job
        .as_ref()
        .and_then(|j| j.get("sheet_spans"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| Some((base(s.get("sheet")?.as_str()?), s.get("lo")?.as_u64()?, s.get("hi")?.as_u64()?)))
        .take(MAX_PAGES)
        .collect();
    let mut pages: Vec<Page> = Vec::new();
    let mut count = 0usize;
    if !spans.is_empty() {
        let mut by_sheet: BTreeMap<String, usize> = BTreeMap::new();
        for (sheet, _, _) in &spans {
            if !by_sheet.contains_key(sheet) {
                by_sheet.insert(sheet.clone(), pages.len());
                pages.push(page_for(sheet, None));
            }
        }
        for e in d.model.iter() {
            if count >= r.max_texts {
                break;
            }
            let Some(t) = text_of(e) else { continue };
            let Some((sheet, _, _)) = spans.iter().find(|(_, lo, hi)| e.handle.0 >= *lo && e.handle.0 < *hi) else { continue };
            if let Some(p) = by_sheet.get(sheet).and_then(|i| pages.get_mut(*i)) {
                p.texts.push(t);
                count += 1;
            }
        }
    } else if sp.sheets.iter().any(|l| l.rect.is_some()) {
        let rects: Vec<(usize, [f64; 4])> = sp
            .sheets
            .iter()
            .filter_map(|l| l.rect.map(|r| (l, r)))
            .take(MAX_PAGES)
            .map(|(l, rect)| {
                pages.push(page_for(&base(&l.number), l.discipline.as_deref()));
                (pages.len() - 1, rect)
            })
            .collect();
        for e in d.model.iter() {
            if count >= r.max_texts {
                break;
            }
            let Some(t) = text_of(e) else { continue };
            let inside = rects.iter().find(|(_, [x0, y0, x1, y1])| t.at.x >= *x0 && t.at.x <= *x1 && t.at.y >= *y0 && t.at.y <= *y1);
            if let Some(p) = inside.and_then(|(i, _)| pages.get_mut(*i)) {
                p.texts.push(t);
                count += 1;
            }
        }
    } else {
        pages.push(Page { texts: d.model.iter().filter_map(|e| text_of(e)).take(r.max_texts).collect(), ..Page::default() });
    }
    pages.retain(|p| !p.texts.is_empty());
    pages
}

/// A page read from a file (a sheet read again, a shop drawing): its text, its discipline, its
/// title and its levels by the title.
pub(crate) fn file_page(sheet: &str, discipline: &str, title: &str, d: &Drawing, sp: &BuildingSpec) -> Page {
    let texts: Vec<TextItem> = d.model.iter().filter_map(|e| text_of(e)).take(rules().max_texts).collect();
    let mut lv: Vec<String> = Vec::new();
    if let Some(l) = levels::level_of_title(title).and_then(|n| sp.level_matching(&n)).and_then(|i| sp.levels.get(i)) {
        lv.push(l.name.clone());
    }
    Page { sheet: sheet.to_string(), discipline: discipline.to_ascii_uppercase(), levels: lv, texts }
}

// ─── Rooms ───────────────────────────────────────────────────────────────────

/// A room of the model as the checks see it: its name and number, its level, where it is and its
/// ceiling with where that was read.
#[derive(Clone, Debug, Default)]
pub(crate) struct SpaceRoom {
    pub name: String,
    pub number: String,
    pub level: String,
    pub at: Option<[f64; 2]>,
    pub ceiling_ft: Option<f64>,
    pub ceiling_source: String,
}

/// The model's rooms: the built room objects (their name, number, level and ceiling), then the
/// spec's rooms that carry a level and no built room has; a built room's missing name and ceiling
/// come from the spec's room of its number.
pub(crate) fn model_rooms(d: &Drawing, sp: &BuildingSpec) -> Vec<SpaceRoom> {
    let m = super::propuse::Model::of(d, |k| k.eq_ignore_ascii_case("room"));
    let spec_room = |n: &str| sp.rooms.iter().find(|r| r.number.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(n)));
    let mut out: Vec<SpaceRoom> = Vec::new();
    for el in m.els.iter().take(MAX_ROOMS) {
        let number = el.text("number").unwrap_or_default();
        let sr = spec_room(&number);
        let name = el.text("name").filter(|n| !n.is_empty()).or_else(|| sr.map(|r| r.name.clone())).unwrap_or_default();
        let level = el.level();
        if name.is_empty() || level.is_empty() {
            continue;
        }
        let c = el.center();
        let ceiling_ft = el.num(&["ceiling_height"]).filter(|v| v.is_finite() && *v > 0.0).or_else(|| sr.and_then(|r| r.ceiling_ft));
        let ceiling_source =
            sr.and_then(|r| r.ceiling_source.clone()).or_else(|| el.get("ceiling_height").and_then(|p| p.note.clone())).unwrap_or_default();
        out.push(SpaceRoom { name, number, level, at: Some([c[0], c[1]]), ceiling_ft, ceiling_source });
    }
    for r in sp.rooms.iter().take(MAX_ROOMS) {
        let Some(level) = r.level.clone() else { continue };
        let number = r.number.clone().unwrap_or_default();
        if out.iter().any(|x| x.level.eq_ignore_ascii_case(&level) && (x.number == number && !number.is_empty())) {
            continue;
        }
        out.push(SpaceRoom {
            name: r.name.clone(),
            number,
            level,
            at: r.at,
            ceiling_ft: r.ceiling_ft,
            ceiling_source: r.ceiling_source.clone().unwrap_or_default(),
        });
    }
    out
}

// ─── Text: lines, blocks, sentences ──────────────────────────────────────────

/// A line of lettering: the texts on one baseline that run on from each other, joined.
#[derive(Clone, Debug)]
struct Line {
    text: String,
    x: f64,
    y: f64,
    h: f64,
}

/// A note as printed: its lines joined, upper case.
#[derive(Clone, Debug)]
pub(crate) struct Block {
    pub text: String,
}

fn flat(t: &TextItem) -> bool {
    t.rotation.sin().abs() < 0.05 && t.rotation.cos() > 0.0
}

/// The page's lines: texts on one baseline of one height that run on from each other.
fn lines(texts: &[TextItem]) -> Vec<Line> {
    let mut items: Vec<&TextItem> = texts.iter().filter(|t| flat(t)).collect();
    items.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
    let mut out: Vec<Line> = Vec::new();
    let mut i = 0usize;
    while i < items.len() {
        let Some(first) = items.get(i) else { break };
        let h = first.height;
        // The run of texts on this baseline.
        let mut j = i;
        while items.get(j).is_some_and(|t| (first.at.y - t.at.y).abs() <= 0.3 * h) {
            j += 1;
        }
        let mut row: Vec<&TextItem> = items.get(i..j).map(<[_]>::to_vec).unwrap_or_default();
        row.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
        let mut cur: Option<Line> = None;
        let mut end = f64::NEG_INFINITY;
        for t in row {
            let joins = cur.as_ref().is_some_and(|c| (t.height - c.h).abs() <= 0.3 * c.h && t.at.x - end <= 2.5 * c.h);
            let w = t.value.chars().count() as f64 * 0.75 * t.height;
            if joins {
                if let Some(c) = cur.as_mut() {
                    c.text.push(' ');
                    c.text.push_str(t.value.trim());
                }
            } else {
                if let Some(c) = cur.take() {
                    out.push(c);
                }
                cur = Some(Line { text: t.value.trim().to_string(), x: t.at.x, y: t.at.y, h: t.height });
            }
            end = end.max(t.at.x + w);
            if !joins {
                end = t.at.x + w;
            }
        }
        if let Some(c) = cur {
            out.push(c);
        }
        i = j.max(i + 1);
    }
    out
}

/// The page's notes: each line, then the lines just under it that start near it, joined.
pub(crate) fn blocks(texts: &[TextItem]) -> Vec<Block> {
    let r = rules();
    let ls = lines(texts);
    let mut used = vec![false; ls.len()];
    let mut out: Vec<Block> = Vec::new();
    for i in 0..ls.len() {
        if out.len() >= r.max_blocks.max(1) {
            break;
        }
        if used.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(first) = ls.get(i) else { continue };
        if let Some(u) = used.get_mut(i) {
            *u = true;
        }
        let mut text = first.text.clone();
        let mut cur = first;
        for _ in 1..MAX_BLOCK_LINES {
            // Lines are sorted top down: the next below is after this one.
            let next = ls.iter().enumerate().skip(i + 1).take_while(|(_, l)| cur.y - l.y <= 2.4 * cur.h.max(l.h)).find(|(k, l)| {
                !used.get(*k).copied().unwrap_or(true)
                    && cur.y - l.y > 0.5 * cur.h
                    && (l.x - first.x).abs() <= 4.0 * first.h
                    && (l.h - first.h).abs() <= 0.3 * first.h
            });
            let Some((k, l)) = next else { break };
            if let Some(u) = used.get_mut(k) {
                *u = true;
            }
            text.push(' ');
            text.push_str(&l.text);
            cur = l;
        }
        out.push(Block { text: squash(&text) });
    }
    out
}

/// Upper case, every run of white space one space.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase()
}

/// A block's sentences: split at a period that ends a word (`F.F.` and `1/2"` stay whole).
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in text.split(' ') {
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
        let core = w.trim_end_matches('.');
        let inner_dots = core.contains('.');
        let numbered = core.chars().all(|c| c.is_ascii_digit());
        let abbreviation = rules().abbreviations.iter().any(|a| a.eq_ignore_ascii_case(bare(core)));
        if w.ends_with('.') && !inner_dots && !numbered && !abbreviation && w.len() > 3 {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// A word without the marks around it (`(CORRIDOR),` → `CORRIDOR`).
fn bare(w: &str) -> &str {
    w.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | '(' | ')' | '[' | ']' | '*'))
}

/// The words of a text, bare, upper case.
fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(|w| bare(w).to_ascii_uppercase()).filter(|w| !w.is_empty()).collect()
}

/// Whether a word is one of `list` (a sentence's last word keeps its period).
fn one_of(w: &str, list: &[String]) -> bool {
    let w = w.trim_end_matches('.');
    list.iter().any(|x| x.trim_end_matches('.') == w)
}

/// Whether `phrase` (one or more words) is in `ws` as whole words; the index of its first word.
fn find_phrase(ws: &[String], phrase: &str) -> Option<usize> {
    let p: Vec<String> = words(phrase);
    if p.is_empty() || p.len() > ws.len() {
        return None;
    }
    (0..=ws.len() - p.len()).find(|&i| p.iter().enumerate().all(|(k, w)| ws.get(i + k).is_some_and(|x| x == w || x.trim_end_matches('.') == w)))
}

fn has_any(ws: &[String], phrases: &[String]) -> bool {
    phrases.iter().any(|p| find_phrase(ws, p).is_some())
}

/// An inch quantity a word prints (`16"`, `3-1/2"`, `1/2"`, `11.5"`, `12"X8"` → the second):
/// the number, and whether it is a fraction alone.
fn inch_word(w: &str) -> Option<(f64, bool)> {
    let w = w.trim_end_matches(['.', ',']);
    if let Some((a, b)) = w.split_once(['X', 'x', '×']) {
        let (a, b) = (inch_word(a).or_else(|| num(a).map(|v| (v, false))), inch_word(b)?);
        return a.map(|_| b);
    }
    let core = w.strip_suffix('"').or_else(|| w.strip_suffix("IN"))?;
    let v = num(core)?;
    let fraction = core.contains('/') && !core.contains('-');
    (v.is_finite() && v > 0.0).then_some((v, fraction))
}

/// `3`, `3.5`, `1/2`, `3-1/2`.
fn num(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for part in s.split('-') {
        total += match part.split_once('/') {
            Some((a, b)) => {
                let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
                if b <= 0.0 {
                    return None;
                }
                a / b
            }
            None => part.parse::<f64>().ok()?,
        };
    }
    total.is_finite().then_some(total)
}

/// The inch quantities of a word list: each value and the index of its (last) word; a whole
/// number before a fraction is its whole part (`3 1/2"`).
fn inch_values(ws: &[String]) -> Vec<(f64, usize)> {
    let mut out = Vec::new();
    for (i, w) in ws.iter().enumerate() {
        let Some((mut v, fraction)) = inch_word(w) else { continue };
        if fraction
            && let Some(whole) =
                i.checked_sub(1).and_then(|p| ws.get(p)).and_then(|p| p.parse::<f64>().ok()).filter(|x| x.fract() == 0.0 && *x < 100.0)
        {
            v += whole;
        }
        out.push((v, i));
    }
    out
}

/// The nearest inch quantity within `reach` words before word `at` (else after it, when
/// `after`).
fn inches_near(ws: &[String], at: usize, reach: usize, after: bool) -> Option<f64> {
    let vals = inch_values(ws);
    vals.iter()
        .filter(|(_, i)| *i < at && at - *i <= reach)
        .max_by_key(|(_, i)| *i)
        .or_else(|| if after { vals.iter().filter(|(_, i)| *i > at && *i - at <= reach).min_by_key(|(_, i)| *i) } else { None })
        .map(|(v, _)| *v)
}

fn quote(s: &str) -> String {
    let t: String = s.chars().take(MAX_QUOTE).collect();
    if t.len() < s.len() { format!("{t}…") } else { t }
}

/// A short stable key (an RFI id that survives a rebuild).
fn stable_id(check: &str, level: &str, subject: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in format!("{check}|{}|{}", level.trim().to_ascii_uppercase(), subject.trim().to_ascii_uppercase()).bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("RFI-D{h:08x}")
}

fn sheets_of<'a>(it: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in it {
        if !s.is_empty() && !out.iter().any(|x| x == s) && out.len() < MAX_SHEETS {
            out.push(s.to_string());
        }
    }
    out
}

fn named(list: &[String]) -> String {
    if list.is_empty() { "the drawings".into() } else { list.join(", ") }
}

/// A conflict RFI of one check.
#[allow(clippy::too_many_arguments)]
fn finding(
    check: &str,
    level: &str,
    at: [f64; 2],
    subject: &str,
    about: String,
    issue: String,
    assumed: String,
    proposed: String,
    sides: Vec<RfiSide>,
    sheets: Vec<String>,
) -> Rfi {
    let r = rules();
    let mut rfi = Rfi::new(RfiKind::Conflict, level, at, about, issue, assumed);
    rfi.id = stable_id(check, level, subject);
    rfi.severity = Severity::parse(&r.severity).unwrap_or(Severity::High);
    rfi.topic = format!("{TOPIC}{check}");
    rfi.proposed = Some(proposed);
    rfi.sides = sides;
    rfi.sheets = sheets;
    rfi.before = r.before.clone();
    rfi
}

// ─── The measure ─────────────────────────────────────────────────────────────

/// One check's measure.
#[derive(Clone, Debug, Default)]
pub(crate) struct Outcome {
    pub check: &'static str,
    /// The things it compared (spaces, levels, conditions, groups, pairs).
    pub compared: usize,
    pub raised: usize,
    /// What it found to compare but could not (no lid or structure, no assembly…), in words.
    pub not_compared: Vec<String>,
}

impl Outcome {
    fn new(check: &'static str) -> Outcome {
        Outcome { check, ..Outcome::default() }
    }

    fn skip(&mut self, why: String) {
        if self.not_compared.len() < 20 && !self.not_compared.contains(&why) {
            self.not_compared.push(why);
        }
    }

    fn json(&self) -> Value {
        let status = if self.compared == 0 {
            if self.not_compared.is_empty() { "did not run: nothing in the documents to compare".to_string() } else { "did not run".to_string() }
        } else {
            "ran".to_string()
        };
        json!({"check": self.check, "status": status, "compared": self.compared, "raised": self.raised, "not_compared": self.not_compared})
    }
}

/// What the checks found: each check's measure and the RFIs.
#[derive(Clone, Debug, Default)]
pub(crate) struct Report {
    pub outcomes: Vec<Outcome>,
    pub rfis: Vec<Rfi>,
    pub pages: usize,
}

impl Report {
    pub(crate) fn json(&self) -> Value {
        let ran = self.outcomes.iter().filter(|o| o.compared > 0).count();
        json!({
            "pages": self.pages, "checks": self.outcomes.len(), "ran": ran, "findings": self.rfis.len(),
            "by_check": self.outcomes.iter().map(Outcome::json).collect::<Vec<_>>(),
        })
    }
}

/// Every check over the pages, the model's rooms and its spec.
pub(crate) fn check(pages: &[Page], rooms: &[SpaceRoom], sp: &BuildingSpec) -> Report {
    let r = rules();
    let blocks: Vec<(usize, Vec<Block>)> = pages.iter().enumerate().take(MAX_PAGES).map(|(i, p)| (i, blocks(&p.texts))).collect();
    let mut rep = Report { pages: pages.len(), ..Report::default() };
    let mut o = Outcome::new("ceiling space");
    rep.rfis.extend(ceiling_space(pages, &blocks, rooms, sp, &mut o));
    rep.outcomes.push(o);
    let mut o = Outcome::new("level datums");
    rep.rfis.extend(level_datums(pages, sp, &mut o));
    rep.outcomes.push(o);
    let mut o = Outcome::new("design conditions");
    rep.rfis.extend(conditions(pages, &blocks, sp, &mut o));
    rep.outcomes.push(o);
    let mut o = Outcome::new("penetration groups");
    rep.rfis.extend(penetrations(pages, &blocks, sp, &mut o));
    rep.outcomes.push(o);
    let mut o = Outcome::new("site objects");
    rep.rfis.extend(site(sp, &mut o));
    rep.outcomes.push(o);
    // One item per id (a conflict read on several pages is one).
    let mut seen = BTreeSet::new();
    rep.rfis.retain(|x| seen.insert(x.id.clone()));
    rep.rfis.truncate(r.max_findings.max(1));
    rep
}

// ─── Ceiling space ───────────────────────────────────────────────────────────

/// Work a note routes above one space's ceiling.
#[derive(Clone, Debug)]
struct Routed {
    /// The space's words (`CORRIDOR`), upper case.
    space: Vec<String>,
    /// Each item: its id, the depth it needs (inches, without a fall) and its fall per foot.
    items: Vec<(String, f64, f64)>,
    sheet: String,
    levels: Vec<String>,
    note: String,
}

/// The space words between `ABOVE` and `CEILING` in a sentence, and the sentence's items.
fn routed_in(sentence: &str, cr: &CeilingRules) -> Vec<(Vec<String>, Vec<(String, f64, f64)>)> {
    let ws = words(sentence);
    let mut out = Vec::new();
    for (i, w) in ws.iter().enumerate() {
        if !one_of(w, &cr.above_words) {
            continue;
        }
        let mut space: Vec<String> = Vec::new();
        let mut ceiling = false;
        for x in ws.iter().skip(i + 1).take(cr.space_max_words + 4) {
            if one_of(x, &cr.ceiling_words) {
                ceiling = true;
                break;
            }
            if one_of(x, &cr.filler_words) {
                continue;
            }
            space.push(x.clone());
            if space.len() > cr.space_max_words {
                break;
            }
        }
        if !ceiling || space.is_empty() || space.len() > cr.space_max_words {
            continue;
        }
        let mut items: Vec<(String, f64, f64)> = Vec::new();
        for it in &cr.items {
            let Some(at) = it.words.iter().filter_map(|p| find_phrase(&ws, p)).min() else { continue };
            // Its printed size, a few words before it (`2 1/2" HOT WATER MAIN`), else its usual one.
            let size = inches_near(&ws, at, 4, false).filter(|v| *v <= 48.0).unwrap_or(it.depth_in);
            let need = size + it.insulation_in + cr.hanger_in;
            if !items.iter().any(|(id, _, _)| *id == it.id) {
                items.push((it.id.clone(), need, it.slope_in_per_ft));
            }
        }
        if !items.is_empty() {
            out.push((space, items));
        }
    }
    out
}

/// Whether a room's name is the space's (by its words or their aliases).
fn names_space(room: &str, space: &[String], cr: &CeilingRules) -> bool {
    let rw = words(room);
    space.iter().any(|s| {
        let group: Vec<&String> = cr.space_aliases.iter().find(|g| g.iter().any(|a| a == s)).map(|g| g.iter().collect()).unwrap_or_else(|| vec![s]);
        group.iter().any(|g| rw.iter().any(|w| w == *g))
    })
}

/// A point inside an outline (even-odd).
fn inside(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let mut odd = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                odd = !odd;
            }
        }
    }
    odd
}

fn centre(outline: &[[f64; 2]]) -> [f64; 2] {
    let n = outline.len().max(1) as f64;
    let s = outline.iter().fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
    [s[0] / n, s[1] / n]
}

/// The longest extent of an outline, in its units.
fn length_of(outline: &[[f64; 2]]) -> f64 {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in outline {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let l = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    if l.is_finite() { l } else { 0.0 }
}

/// One space of one level as the model holds it: the ceiling, what limits the depth over it and
/// where each was read.
struct SpaceAbove {
    rooms: Vec<String>,
    at: [f64; 2],
    ceiling_ft: f64,
    ceiling_source: String,
    /// The finished ceiling's own build-up, inches.
    ceiling_in: f64,
    /// The lowest thing over it: its height above the floor, what it is, where it was read.
    limit: Option<(f64, String, String)>,
    length_ft: f64,
}

/// The lid notes of the pages: a lid word, a space's words and a height in one block.
fn lid_notes(pages: &[Page], blocks: &[(usize, Vec<Block>)], cr: &CeilingRules) -> Vec<(Vec<String>, f64, String, Vec<String>, String)> {
    let mut out = Vec::new();
    for (pi, bs) in blocks {
        let Some(p) = pages.get(*pi) else { continue };
        for b in bs {
            for s in sentences(&b.text) {
                let ws = words(&s);
                if !has_any(&ws, &cr.lid_words) {
                    continue;
                }
                let Some((h, _, _)) = levels::parse_elevation(&s) else { continue };
                if !(5.0..=40.0).contains(&h) {
                    continue;
                }
                out.push((ws, h, p.sheet.clone(), p.levels.clone(), quote(&s)));
            }
        }
    }
    out
}

/// The model's spaces named like `space`, level by level.
fn spaces_above(
    sp: &BuildingSpec,
    rooms: &[SpaceRoom],
    space: &[String],
    cr: &CeilingRules,
    lids: &[(Vec<String>, f64, String, Vec<String>, String)],
) -> Vec<(String, SpaceAbove)> {
    let upf = sp.units_per_foot.abs().max(1e-9);
    let mut by_level: BTreeMap<String, SpaceAbove> = BTreeMap::new();
    for room in rooms.iter().take(MAX_ROOMS).filter(|r| names_space(&r.name, space, cr)) {
        let level = room.level.clone();
        let label = if room.number.is_empty() { room.name.clone() } else { format!("{} {}", room.name, room.number) };
        // Its ceilings: those the RCP ties to it, else those over its tag.
        let mine: Vec<&cadaddy_walk3d::CeilingArea> = sp
            .rcp
            .ceilings
            .iter()
            .take(MAX_ROOMS)
            .filter(|c| c.level == level)
            .filter(|c| {
                c.room
                    .as_deref()
                    .is_some_and(|x| x.eq_ignore_ascii_case(&room.name) || (!room.number.is_empty() && x.eq_ignore_ascii_case(&room.number)))
                    || room.at.is_some_and(|a| inside(a, &c.outline))
            })
            .collect();
        let mut heights: Vec<(f64, String, f64, f64)> = mine
            .iter()
            .filter(|c| c.height_ft.is_finite() && c.height_ft > 0.0)
            .map(|c| {
                let build = c
                    .ceiling_type
                    .as_ref()
                    .and_then(|t| sp.ceiling_types.iter().find(|x| &x.name == t))
                    .map_or(0.0, |t| t.layers.iter().filter(|l| l.category == "board" || l.category == "tile").map(|l| l.thickness_in).sum());
                (c.height_ft, c.source.clone().unwrap_or_default(), build, length_of(&c.outline) / upf)
            })
            .collect();
        heights.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (ceiling_ft, source, build, len) = match heights.first() {
            Some(h) => h.clone(),
            None => match room.ceiling_ft.filter(|h| h.is_finite() && *h > 0.0) {
                Some(h) => (h, room.ceiling_source.clone(), 0.0, 0.0),
                None => continue,
            },
        };
        // A second, higher ceiling of the same room is the lid over it.
        let lid =
            heights.iter().find(|h| h.0 > ceiling_ft + 1.0 / 96.0).map(|h| (h.0, "the lid (the upper ceiling over it)".to_string(), h.1.clone()));
        let at = room.at.or_else(|| mine.first().map(|c| centre(&c.outline))).unwrap_or([0.0, 0.0]);
        let e = by_level.entry(level.clone()).or_insert(SpaceAbove {
            rooms: Vec::new(),
            at,
            ceiling_ft,
            ceiling_source: source.clone(),
            ceiling_in: build,
            limit: None,
            length_ft: 0.0,
        });
        if !e.rooms.contains(&label) {
            e.rooms.push(label);
        }
        if ceiling_ft < e.ceiling_ft {
            e.ceiling_ft = ceiling_ft;
            e.ceiling_source = source;
            e.ceiling_in = build;
        }
        e.length_ft = e.length_ft.max(len);
        if let Some(l) = lid
            && e.limit.as_ref().is_none_or(|x| l.0 < x.0)
        {
            e.limit = Some(l);
        }
    }
    // A lid note for the space (on its level's sheet, or a sheet of every level).
    for (level, e) in &mut by_level {
        for (ws, h, sheet, lv, note) in lids {
            if *h <= e.ceiling_ft + 1.0 / 96.0 || !(lv.is_empty() || lv.iter().any(|l| l.eq_ignore_ascii_case(level))) {
                continue;
            }
            if !space.iter().any(|s| ws.contains(s)) && !names_space(&ws.join(" "), space, cr) {
                continue;
            }
            if e.limit.as_ref().is_none_or(|x| *h < x.0) {
                e.limit = Some((*h, "the lid".into(), format!("{sheet}: {note}")));
            }
        }
    }
    // Else the floor structure above: the level above less its framing and its deck.
    let mut lv: Vec<&cadaddy_walk3d::Level> = sp.levels.iter().collect();
    lv.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    for (level, e) in &mut by_level {
        let Some(i) = lv.iter().position(|l| l.name.eq_ignore_ascii_case(level)) else { continue };
        let Some(here) = lv.get(i) else { continue };
        let Some(above) = lv.get(i + 1) else { continue };
        let floor = sp.floor_framing.iter().find(|f| f.level.eq_ignore_ascii_case(&above.name));
        let Some(f) = floor.or_else(|| sp.exterior.framing.iter().find(|f| f.level.eq_ignore_ascii_case(&above.name))) else { continue };
        if !(f.depth_in.is_finite() && f.depth_in > 0.0) {
            continue;
        }
        // What lies on the members: the framing's deck, else the layers the floor's notes give
        // (`3/4" GYPCRETE ON 3/4" SHEATHING`), else the usual deck.
        let noted: f64 = sp
            .floor_notes
            .iter()
            .filter(|n| n.level.eq_ignore_ascii_case(&above.name))
            .map(|n| inch_values(&words(&n.text)).iter().map(|(v, _)| *v).filter(|v| *v <= 6.0).sum::<f64>())
            .fold(0.0, f64::max);
        let deck =
            f.deck_in.filter(|d| d.is_finite() && *d >= 0.0).unwrap_or(if noted > 0.0 && floor.is_some() { noted } else { cr.default_deck_in });
        // A roof's members bear where the sections mark; their bottom chord sits there.
        let bottom = match (floor, f.bearing_ft) {
            (None, Some(b)) if b.is_finite() => b - here.elevation_ft,
            _ => above.elevation_ft - here.elevation_ft - (f.depth_in + deck) / 12.0,
        };
        if bottom.is_finite() && e.limit.as_ref().is_none_or(|x| bottom < x.0) {
            let what =
                format!("the {} {} {} of {} over it", inches(f.depth_in), f.member, if f.member.ends_with('s') { "" } else { "framing" }, above.name);
            e.limit = Some((bottom, what.replace("  ", " "), f.source.clone()));
        }
    }
    by_level.into_iter().collect()
}

/// Work routed above a space's ceiling against the free depth over it.
fn ceiling_space(pages: &[Page], blocks: &[(usize, Vec<Block>)], rooms: &[SpaceRoom], sp: &BuildingSpec, out: &mut Outcome) -> Vec<Rfi> {
    let cr = &rules().ceiling_space;
    let mut routed: Vec<Routed> = Vec::new();
    for (pi, bs) in blocks {
        let Some(p) = pages.get(*pi) else { continue };
        for b in bs {
            for s in sentences(&b.text) {
                for (space, items) in routed_in(&s, cr) {
                    routed.push(Routed { space, items, sheet: p.sheet.clone(), levels: p.levels.clone(), note: quote(&s) });
                }
            }
        }
    }
    let lids = lid_notes(pages, blocks, cr);
    // One look per space: its notes together.
    let mut by_space: BTreeMap<String, Vec<&Routed>> = BTreeMap::new();
    for r in &routed {
        by_space.entry(r.space.join(" ")).or_default().push(r);
    }
    let mut rfis = Vec::new();
    for (name, notes) in &by_space {
        let space: Vec<String> = words(name);
        let found = spaces_above(sp, rooms, &space, cr, &lids);
        if found.is_empty() {
            out.skip(format!(
                "notes route work above the {} ceiling; the model has no room named so with a ceiling height",
                name.to_ascii_lowercase()
            ));
            continue;
        }
        for (level, s) in &found {
            let mine: Vec<&&Routed> =
                notes.iter().filter(|r| r.levels.is_empty() || r.levels.iter().any(|l| l.eq_ignore_ascii_case(level))).collect();
            if mine.is_empty() {
                continue;
            }
            let Some((limit_ft, limit_what, limit_src)) = &s.limit else {
                out.skip(format!(
                    "{} on {level}: no lid, second ceiling or framed floor above to measure the space against",
                    name.to_ascii_lowercase()
                ));
                continue;
            };
            out.compared += 1;
            // The most any item needs, a drainage line its fall over the space too.
            let mut need = 0.0f64;
            let mut why: Vec<String> = Vec::new();
            for r in &mine {
                for (id, n, slope) in &r.items {
                    let total = n + slope * s.length_ft;
                    if total > need {
                        need = total;
                    }
                    let w = if *slope > 0.0 && s.length_ft > 0.0 {
                        format!("{id} about {} with its fall over {}", inches(total), feet(s.length_ft))
                    } else {
                        format!("{id} about {}", inches(*n))
                    };
                    if !why.contains(&w) {
                        why.push(w);
                    }
                }
            }
            let free = (limit_ft - s.ceiling_ft) * 12.0 - s.ceiling_in;
            if free + cr.tolerance_in >= need {
                continue;
            }
            out.raised += 1;
            let sheets =
                sheets_of(mine.iter().map(|r| r.sheet.as_str()).chain(std::iter::once(s.ceiling_source.as_str()).filter(|x| !x.contains(' '))));
            let note = mine.first().map(|r| r.note.clone()).unwrap_or_default();
            let note_doc = sheets_of(mine.iter().map(|r| r.sheet.as_str()));
            let rooms: Vec<String> = s.rooms.iter().take(MAX_ROOMS_NAMED).cloned().collect();
            let more = s.rooms.len().saturating_sub(MAX_ROOMS_NAMED);
            let room_list = if more > 0 { format!("{} and {more} more", rooms.join(", ")) } else { rooms.join(", ") };
            let free_txt = if free <= 0.0 { "no".to_string() } else { inches(free) };
            let issue = format!(
                "The notes ({}) put {} above the {} ceiling on {level}; that needs about {} (each item's size, insulation and hanger{}). The ceiling at {} hangs under {} at {}: {free_txt} free.",
                named(&note_doc),
                why.join(", "),
                name.to_ascii_lowercase(),
                inches(need),
                if mine.iter().any(|r| r.items.iter().any(|(_, _, sl)| *sl > 0.0)) { ", a drain's fall" } else { "" },
                feet(s.ceiling_ft),
                limit_what,
                feet(*limit_ft),
            );
            let corrected = limit_ft - (need + s.ceiling_in) / 12.0;
            let (assumed, proposed) = if corrected + 1e-9 >= cr.min_ceiling_ft {
                (
                    format!(
                        "Corrected for the field: the work needs the ceiling at {} (design {}), which keeps the {} ({}); alternative: keep the ceiling and route the work outside the space (through the rooms beside it or in a soffit). The model keeps the design ceiling until this is answered.",
                        feet(corrected),
                        feet(s.ceiling_ft),
                        feet(cr.min_ceiling_ft),
                        cr.min_ceiling_basis
                    ),
                    format!(
                        "Lower the {} ceiling on {level} to {} so the work fits under {}, or route {} outside the space: which?",
                        name.to_ascii_lowercase(),
                        feet(corrected),
                        limit_what,
                        why.join(", ")
                    ),
                )
            } else {
                (
                    format!(
                        "Corrected for the field: no ceiling height fits the work and keeps the {} ({}): the work routes outside the space (through the rooms beside it or in a soffit). The model keeps the design ceiling until this is answered.",
                        feet(cr.min_ceiling_ft),
                        cr.min_ceiling_basis
                    ),
                    format!(
                        "Route {} outside the {} on {level}, or raise the lid/structure over it: which?",
                        why.join(", "),
                        name.to_ascii_lowercase()
                    ),
                )
            };
            let ceiling_doc = if s.ceiling_source.is_empty() { "the reflected ceiling plan".to_string() } else { s.ceiling_source.clone() };
            let limit_doc = if limit_src.is_empty() { "the framing plans".to_string() } else { limit_src.clone() };
            let sides = vec![
                RfiSide { document: named(&note_doc), says: note },
                RfiSide { document: ceiling_doc, says: format!("{} ceiling at {} in {room_list}", name.to_ascii_lowercase(), feet(s.ceiling_ft)) },
                RfiSide { document: limit_doc, says: format!("{limit_what} at {} above the floor", feet(*limit_ft)) },
            ];
            let mut rfi = finding(
                "ceiling space",
                level,
                s.at,
                name,
                format!("{} ceiling space on {level}", name.to_ascii_lowercase()),
                issue,
                assumed,
                proposed,
                sides,
                sheets,
            );
            rfi.elements.clear();
            rfis.push(rfi);
        }
    }
    rfis
}

// ─── Level datums ────────────────────────────────────────────────────────────

/// Lettering a level mark can be: a word of three letters or more (`LEVEL`, `FLOOR`, `2ND`'s
/// `FLOOR`) or a level code (`L2`), and mostly letters, digits and spaces (lettering read off
/// strokes prints runs of `/`, `:`, `=`).
fn lettered(t: &str) -> bool {
    let n = t.chars().count().max(1);
    let plain = t.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '\'' | '"' | '-' | '.' | '=')).count();
    let word = t.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| {
        w.chars().filter(char::is_ascii_alphabetic).count() >= 3
            || (w.len() >= 2 && w.starts_with(['L', 'l']) && w.chars().skip(1).all(|c| c.is_ascii_digit()))
    });
    word && plain * 10 >= n * 9
}

/// A floor's mark (a floor or a basement: a roof's and a parapet's are other heights).
fn floor_kind(k: levels::DatumKind) -> bool {
    matches!(k, levels::DatumKind::Floor | levels::DatumKind::Basement)
}

/// A page's level marks: canonical name → (elevation as printed, the text).
fn level_marks(texts: &[TextItem], reach: f64) -> BTreeMap<String, (f64, String)> {
    let mut all: Vec<(String, f64, String)> = Vec::new();
    let mut used: BTreeSet<usize> = BTreeSet::new();
    for (i, t) in texts.iter().enumerate() {
        if !lettered(&t.value) {
            continue;
        }
        if let Some(n) = levels::parse_level_note(&t.value).filter(|n| floor_kind(n.kind)) {
            all.push((n.name, n.elevation_ft, t.value.clone()));
            used.insert(i);
        }
    }
    let lone: Vec<(usize, f64)> = texts
        .iter()
        .enumerate()
        .filter(|(i, t)| !used.contains(i) && t.value.len() <= 24)
        .filter_map(|(i, t)| {
            let (v, a, z) = levels::parse_elevation(&t.value)?;
            let rest = format!("{} {}", t.value.get(..a).unwrap_or_default(), t.value.get(z..).unwrap_or_default());
            let rest = rest.to_ascii_uppercase().replace(['.', ':', '=', '+'], " ");
            rest.split_whitespace().all(|w| matches!(w, "EL" | "ELEV" | "ELEVATION" | "FFE" | "FF" | "F" | "TOS" | "T" | "O" | "S")).then_some((i, v))
        })
        .collect();
    for (i, t) in texts.iter().enumerate() {
        if used.contains(&i) || t.value.len() > 40 || !lettered(&t.value) {
            continue;
        }
        let Some((name, kind)) = levels::canonical_level(&t.value) else { continue };
        let up = t.value.to_ascii_uppercase();
        if !floor_kind(kind) || up.contains("PLAN") || up.split_whitespace().count() > 5 {
            continue;
        }
        let r = reach * t.height.max(1e-9);
        let best = lone
            .iter()
            .filter_map(|(k, v)| texts.get(*k).map(|e| (e, *v)))
            .filter(|(e, _)| e.at.dist(t.at) <= r)
            .min_by(|a, b| a.0.at.dist(t.at).total_cmp(&b.0.at.dist(t.at)));
        if let Some((e, v)) = best {
            all.push((name, v, format!("{} / {}", t.value, e.value)));
        }
    }
    // Per name the most common value (a page marks a floor in several sections).
    let mut out: BTreeMap<String, (f64, String)> = BTreeMap::new();
    let mut names: BTreeSet<String> = all.iter().map(|x| x.0.clone()).collect();
    for name in std::mem::take(&mut names) {
        let mine: Vec<&(String, f64, String)> = all.iter().filter(|x| x.0 == name).collect();
        let best = mine.iter().max_by_key(|x| mine.iter().filter(|y| (y.1 - x.1).abs() < 1.0 / 96.0).count());
        if let Some(b) = best {
            out.insert(name, (b.1, b.2.clone()));
        }
    }
    out
}

/// Each document's level marks against the levels the model is built on.
fn level_datums(pages: &[Page], sp: &BuildingSpec, out: &mut Outcome) -> Vec<Rfi> {
    let lr = &rules().levels;
    // (discipline, level) → (difference ft, document's value, model's, each sheet and its text).
    let mut found: BTreeMap<(String, String), (f64, f64, f64, Vec<(String, String)>)> = BTreeMap::new();
    let mut compared: BTreeSet<(String, String)> = BTreeSet::new();
    for p in pages {
        let marks = level_marks(&p.texts, lr.reach_heights);
        // Matched to the model's levels: (model level name, document value, model value, text).
        let mut matched: Vec<(String, f64, f64, String)> = marks
            .iter()
            .filter_map(|(name, (v, text))| {
                let l = sp.level_matching(name).and_then(|i| sp.levels.get(i))?;
                Some((l.name.clone(), *v, l.elevation_ft, text.clone()))
            })
            .collect();
        if matched.is_empty() {
            continue;
        }
        matched.sort_by(|a, b| a.2.total_cmp(&b.2));
        // The page's datum: the convention nearest its lowest level's difference.
        let Some(low) = matched.first() else { continue };
        let delta = low.1 - low.2;
        let Some(datum) = lr.datums_ft.iter().copied().min_by(|a, b| (delta - a).abs().total_cmp(&(delta - b).abs())) else { continue };
        if (delta - datum).abs() > lr.max_difference_ft {
            continue;
        }
        for (level, v, model, text) in &matched {
            let diff = v - datum - model;
            if diff.abs() > lr.max_difference_ft {
                continue;
            }
            compared.insert((p.discipline.clone(), level.clone()));
            if diff.abs() * 12.0 <= lr.tolerance_in {
                continue;
            }
            let e = found.entry((p.discipline.clone(), level.clone())).or_insert((diff, *v, model + datum, Vec::new()));
            if e.3.len() < MAX_SHEETS && !e.3.iter().any(|(s, _)| *s == p.sheet) {
                e.3.push((p.sheet.clone(), text.clone()));
            }
        }
    }
    out.compared = compared.len();
    let mut rfis = Vec::new();
    for ((disc, level), (diff, v, model, docs)) in &found {
        out.raised += 1;
        let who = if disc.is_empty() { "A document".to_string() } else { format!("The {disc} documents") };
        let sheets = sheets_of(docs.iter().map(|(s, _)| s.as_str()));
        let dir = if *diff > 0.0 { "above" } else { "below" };
        let authority = lr.authority.first().cloned().unwrap_or_else(|| "A".into());
        let source = sp.sources.get(&format!("levels/{level}")).cloned().unwrap_or_else(|| format!("the {authority} sections (the model's levels)"));
        let issue = format!(
            "{who} ({}) put {level} at {}, {} {dir} the {} the model is built on: work set from their numbers lands {dir} the real floor and bearing.",
            named(&sheets),
            feet(*v),
            inches(diff.abs() * 12.0),
            feet(*model)
        );
        let assumed = format!(
            "Corrected for the field: built at {} (the documents' order of authority: the {authority} sections fix the floors); the {} value {} is kept as the design value, and what it sets moves {} {}.",
            feet(*model),
            if disc.is_empty() { "other" } else { disc },
            feet(*v),
            inches(diff.abs() * 12.0),
            if *diff > 0.0 { "down" } else { "up" }
        );
        let proposed = format!("Confirm {level} at {} and revise {} to it.", feet(*model), named(&sheets));
        let mut sides: Vec<RfiSide> = docs.iter().map(|(s, t)| RfiSide { document: s.clone(), says: quote(t) }).collect();
        sides.push(RfiSide { document: source, says: format!("{level} at {}", feet(*model)) });
        rfis.push(finding(
            "level datums",
            level,
            [0.0, 0.0],
            &format!("{disc} {level}"),
            format!("{level} elevation ({})", if disc.is_empty() { "a document" } else { disc }),
            issue,
            assumed,
            proposed,
            sides,
            sheets,
        ));
    }
    rfis
}

// ─── Design conditions against assemblies ────────────────────────────────────

/// An assembly's cavity and insulation as the documents give them.
#[derive(Clone, Debug, Default)]
struct Cavity {
    depth_in: Option<(f64, String)>,
    insulation_in: Option<(f64, String)>,
    /// An assembly was described (members and layers) with no insulation in it.
    described_bare: Option<String>,
}

/// The space kind (`floor`, `roof`) a condition's words name.
fn space_kind(ws: &[String], ar: &AssemblyRules) -> Option<(String, String)> {
    ar.space_kinds.iter().find_map(|k| k.words.iter().find(|w| find_phrase(ws, w).is_some()).map(|w| (k.kind.clone(), w.to_ascii_lowercase())))
}

/// The members' depth a sentence gives (`16" FLOOR TRUSSES`, `2X12 JOISTS`, `TJI 210 11 7/8"`).
fn member_depth(ws: &[String], ar: &AssemblyRules) -> Option<f64> {
    let at = ar.member_words.iter().filter_map(|m| find_phrase(ws, m)).min()?;
    let nominal = ws.iter().find_map(|w| ar.nominal_depth_in.get(w.trim_end_matches(['.', ','])).copied());
    inches_near(ws, at, 3, true).filter(|v| *v >= 3.0 && *v <= ar.max_member_in).or(nominal)
}

/// The insulation's thickness a sentence gives (`3 1/2" BATT`, `R-19 BATT INSULATION`).
fn insulation_thickness(ws: &[String], ar: &AssemblyRules) -> Option<f64> {
    let at = ar.insulation_words.iter().filter_map(|m| find_phrase(ws, m)).min()?;
    // The words between the thickness and the insulation word are its kind (`SOUND ATTENUATION`).
    let vals = inch_values(ws);
    let near = vals.iter().filter(|(_, i)| *i < at).max_by_key(|(_, i)| *i).filter(|(_, i)| {
        ws.get(i + 1..at).is_some_and(|between| between.iter().all(|w| ar.insulation_skip_words.iter().any(|s| s == w)) && between.len() <= 4)
    });
    if let Some((v, _)) = near {
        return Some(*v);
    }
    ws.iter().find_map(|w| {
        let r = w.strip_prefix("R-").or_else(|| w.strip_prefix('R')).filter(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()))?;
        ar.r_value_in.get(r).copied().or_else(|| r.parse::<f64>().ok().filter(|_| ar.r_per_in > 0.0).map(|v| v / ar.r_per_in))
    })
}

/// The floor or roof cavity the documents and the model give.
fn cavity_of(kind: &str, pages: &[Page], blocks: &[(usize, Vec<Block>)], sp: &BuildingSpec, ar: &AssemblyRules) -> Cavity {
    let mut c = Cavity::default();
    let kind_words: &[String] = if kind == "roof" { &ar.roof_words } else { &ar.floor_words };
    // The model's framing first: the deepest framed floor (or roof).
    let framing: Vec<&cadaddy_walk3d::RoofFraming> =
        if kind == "roof" { sp.exterior.framing.iter().collect() } else { sp.floor_framing.iter().collect() };
    if let Some(f) = framing.iter().filter(|f| f.depth_in.is_finite() && f.depth_in > 0.0).max_by(|a, b| a.depth_in.total_cmp(&b.depth_in)) {
        let src = if f.source.is_empty() { format!("the model's {} framing ({})", kind, f.level) } else { f.source.clone() };
        c.depth_in = Some((f.depth_in, src));
    }
    // A roof assembly's insulation layers.
    if kind == "roof" {
        for roof in &sp.exterior.roofs {
            let Some(a) = &roof.assembly else { continue };
            let t: f64 = a.layers.iter().filter(|l| l.category.contains("insul")).map(|l| l.thickness_in).sum();
            if t > 0.0 && c.insulation_in.as_ref().is_none_or(|x| t < x.0) {
                c.insulation_in = Some((t, format!("roof {} assembly", roof.name)));
            }
        }
    }
    // The documents' assembly descriptions: the floor notes, then every page's.
    let mut texts: Vec<(String, String)> = sp.floor_notes.iter().map(|n| (n.text.clone(), n.source.clone())).collect();
    for (pi, bs) in blocks {
        let Some(p) = pages.get(*pi) else { continue };
        texts.extend(bs.iter().map(|b| (b.text.clone(), p.sheet.clone())));
    }
    for (text, src) in texts.iter().take(rules().max_blocks.max(1) * 4) {
        for s in sentences(&squash(text)) {
            let ws = words(&s);
            let members = ar.member_words.iter().any(|m| find_phrase(&ws, m).is_some());
            if !members || !has_any(&ws, kind_words) {
                continue;
            }
            if c.depth_in.is_none()
                && let Some(d) = member_depth(&ws, ar)
            {
                c.depth_in = Some((d, format!("{src}: {}", quote(&s))));
            }
            match insulation_thickness(&ws, ar) {
                Some(t) => {
                    if c.insulation_in.as_ref().is_none_or(|x| t < x.0) {
                        c.insulation_in = Some((t, format!("{src}: {}", quote(&s))));
                    }
                }
                None if inch_values(&ws).len() >= 2 && c.described_bare.is_none() => c.described_bare = Some(format!("{src}: {}", quote(&s))),
                None => {}
            }
        }
    }
    c
}

/// A condition one document puts on a space against the assembly another gives it.
fn conditions(pages: &[Page], blocks: &[(usize, Vec<Block>)], sp: &BuildingSpec, out: &mut Outcome) -> Vec<Rfi> {
    let r = rules();
    let ar = &r.assembly;
    // (condition, space kind) → (the space's words, sheets, the note).
    let mut found: BTreeMap<(String, String), (String, Vec<String>, String)> = BTreeMap::new();
    for (pi, bs) in blocks {
        let Some(p) = pages.get(*pi) else { continue };
        for b in bs {
            for s in sentences(&b.text) {
                let ws = words(&s);
                let Some(cond) = r.conditions.iter().find(|c| has_any(&ws, &c.words)) else { continue };
                let Some((kind, space)) = space_kind(&ws, ar) else { continue };
                let e = found.entry((cond.id.clone(), kind)).or_insert((space, Vec::new(), quote(&s)));
                if e.1.len() < MAX_SHEETS && !e.1.contains(&p.sheet) {
                    e.1.push(p.sheet.clone());
                }
            }
        }
    }
    let mut rfis = Vec::new();
    for ((cid, kind), (space, sheets, note)) in &found {
        let Some(cond) = r.conditions.iter().find(|c| &c.id == cid) else { continue };
        let cav = cavity_of(kind, pages, blocks, sp, ar);
        let Some((depth, depth_src)) = &cav.depth_in else {
            out.skip(format!("{} ({}): no {kind} framing depth in the model or the documents", cond.id, space));
            continue;
        };
        let (ins, ins_src) = match (&cav.insulation_in, &cav.described_bare) {
            (Some((t, s)), _) => (*t, s.clone()),
            (None, Some(s)) => (0.0, s.clone()),
            (None, None) => {
                out.skip(format!("{} ({}): no {kind} assembly that gives its insulation", cond.id, space));
                continue;
            }
        };
        out.compared += 1;
        if cond.needs != "full" || ins + ar.tolerance_in >= *depth {
            continue;
        }
        out.raised += 1;
        let all_sheets = sheets_of(
            sheets
                .iter()
                .map(String::as_str)
                .chain([depth_src.as_str(), ins_src.as_str()].into_iter().filter_map(|s| s.split(':').next()).filter(|s| !s.contains(' '))),
        );
        let ins_txt = if ins > 0.0 { format!("{} of insulation", inches(ins)) } else { "no insulation among its layers".to_string() };
        let issue = format!(
            "{} leaves the {space} as \"{}\" ({}), but the {kind} assembly as read ({}) gives {ins_txt} in {} deep framing: {} of the cavity is open, so it is not {}.",
            named(sheets),
            note.to_ascii_lowercase(),
            cond.basis.split(':').next().unwrap_or("").trim(),
            ins_src.split(':').next().unwrap_or("").trim(),
            inches(*depth),
            inches(depth - ins),
            cond.what
        );
        let assumed = format!(
            "Corrected for the field: {} ({} more insulation over the {kind} areas); alternative: {}.",
            cond.correction,
            inches(depth - ins),
            cond.alternative
        );
        let proposed = format!("Fill the {space} full depth ({}) or sprinkler it: which? ({})", inches(*depth), cond.basis);
        let sides = vec![
            RfiSide { document: named(sheets), says: note.clone() },
            RfiSide { document: ins_src.clone(), says: ins_txt.clone() },
            RfiSide { document: depth_src.clone(), says: format!("{} deep {kind} framing", inches(*depth)) },
            RfiSide { document: cond.basis.split(':').next().unwrap_or("").trim().to_string(), says: cond.basis.clone() },
        ];
        rfis.push(finding(
            "design conditions",
            "",
            [0.0, 0.0],
            &format!("{cid} {kind}"),
            format!("{space} ({kind} assembly)"),
            issue,
            assumed,
            proposed,
            sides,
            all_sheets,
        ));
    }
    rfis
}

// ─── Penetration groups ──────────────────────────────────────────────────────

/// The documents' hole rule: the least spacing between holes in one member (inches) and its
/// note, when a note gives one; centre to centre (`O.C.`) or clear.
fn hole_rule(pages: &[Page], blocks: &[(usize, Vec<Block>)], pr: &PenetrationRules) -> Option<(f64, bool, String)> {
    for (pi, bs) in blocks {
        let Some(p) = pages.get(*pi) else { continue };
        for b in bs {
            for s in sentences(&b.text) {
                let ws = words(&s);
                if !has_any(&ws, &pr.hole_words) || !has_any(&ws, &pr.member_words) || !has_any(&ws, &pr.spacing_words) {
                    continue;
                }
                // A spacing: an inch or foot length next to a spacing word.
                let at = pr.spacing_words.iter().filter_map(|w| find_phrase(&ws, w)).min()?;
                let feet_len = ws.iter().enumerate().find_map(|(i, w)| {
                    let v = w.strip_suffix('\'').and_then(num).or_else(|| w.strip_suffix("FT").and_then(num))?;
                    (i + 4 >= at && i <= at + 4).then_some(v * 12.0)
                });
                let len = inches_near(&ws, at, 4, true).or(feet_len);
                if let Some(v) = len.filter(|v| *v > 0.0 && *v <= 120.0) {
                    let oc = ws.iter().any(|w| w == "O.C." || w == "OC" || w == "ON-CENTER" || w == "CENTERS");
                    return Some((v, oc, format!("{}: {}", p.sheet, quote(&s))));
                }
            }
        }
    }
    None
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 1e-12 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

/// One kind of riser group found against the hole rule: where first, how many, the worst case.
#[derive(Default)]
struct Hit {
    level: String,
    levels: Vec<String>,
    at: [f64; 2],
    groups: usize,
    most: usize,
    largest: f64,
    plate_in: f64,
    centres: f64,
    clear: Option<f64>,
    spacing_short: bool,
    too_wide: bool,
    sheets: Vec<String>,
}

/// Risers side by side through one wall's plate against the hole rule.
fn penetrations(pages: &[Page], blocks: &[(usize, Vec<Block>)], sp: &BuildingSpec, out: &mut Outcome) -> Vec<Rfi> {
    let pr = &rules().penetrations;
    let upf = sp.units_per_foot.abs().max(1e-9);
    let rule = hole_rule(pages, blocks, pr);
    // The risers of each level: (index, where, diameter in).
    // A riser stands at its point on its own level and at its storey point on each level it
    // passes up through.
    let mut by_level: BTreeMap<String, Vec<(usize, [f64; 2], f64)>> = BTreeMap::new();
    for (i, r) in sp.mep.runs.iter().enumerate().take(pr.max_runs.max(1)) {
        let Some(p) = r.points.first() else { continue };
        let d = r.size_in.first().copied().unwrap_or(0.0);
        if r.top_ft.is_none() || r.points.len() != 1 || !(d.is_finite() && d >= pr.min_diameter_in) {
            continue;
        }
        by_level.entry(r.level.clone()).or_default().push((i, *p, d));
        for (lv, q) in &r.storey_points {
            by_level.entry(lv.clone()).or_default().push((i, *q, d));
        }
    }
    // Risers closer than their own size overlap: one penetration drawn twice (the larger kept).
    for risers in by_level.values_mut() {
        risers.sort_by(|a, b| b.2.total_cmp(&a.2));
        let mut kept: Vec<(usize, [f64; 2], f64)> = Vec::new();
        for r in risers.iter() {
            let overlaps = kept.iter().any(|k| (k.1[0] - r.1[0]).hypot(k.1[1] - r.1[1]) / upf * 12.0 < k.2.max(r.2));
            if !overlaps {
                kept.push(*r);
            }
        }
        *risers = kept;
    }
    let mut hits: BTreeMap<(String, String, i64), Hit> = BTreeMap::new();
    for (level, risers) in &by_level {
        // Groups: risers within reach of each other, one link at a time.
        let reach = pr.group_reach_in / 12.0 * upf;
        let mut group_of: Vec<usize> = (0..risers.len()).collect();
        for a in 0..risers.len() {
            for b in a + 1..risers.len() {
                let (Some(x), Some(y)) = (risers.get(a), risers.get(b)) else { continue };
                if (x.1[0] - y.1[0]).hypot(x.1[1] - y.1[1]) <= reach {
                    let (ga, gb) = (group_of.get(a).copied().unwrap_or(a), group_of.get(b).copied().unwrap_or(b));
                    let (lo, hi) = (ga.min(gb), ga.max(gb));
                    for g in group_of.iter_mut() {
                        if *g == hi {
                            *g = lo;
                        }
                    }
                }
            }
        }
        let ids: BTreeSet<usize> = group_of.iter().copied().collect();
        for g in ids {
            let members: Vec<&(usize, [f64; 2], f64)> =
                risers.iter().enumerate().filter(|(k, _)| group_of.get(*k) == Some(&g)).map(|(_, x)| x).collect();
            if members.len() < pr.min_group.max(2) {
                continue;
            }
            // The wall they stand in: every one within its plate.
            let wall = sp.walls.iter().filter(|w| w.level.eq_ignore_ascii_case(level)).find_map(|w| {
                let wt = sp.wall_types.iter().find(|t| t.name == w.wall_type)?;
                let t_in = wt.thickness_in;
                let all = members.iter().all(|m| seg_dist(m.1, w.a, w.b) <= (t_in / 2.0 + pr.wall_reach_in) / 12.0 * upf);
                // Its plate is as wide as its studs (the assembly's framing layer), else the wall.
                let plate = wt
                    .assembly
                    .as_ref()
                    .and_then(|a| {
                        a.layers
                            .iter()
                            .filter(|l| l.category.contains("fram") || l.category.contains("stud"))
                            .map(|l| l.thickness_in)
                            .reduce(f64::max)
                    })
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .unwrap_or(t_in);
                (all && t_in > 0.0).then_some((w, plate))
            });
            let Some((w, plate_in)) = wall else { continue };
            out.compared += 1;
            // Their spacing along the wall, clear, and the largest hole.
            let (dx, dy) = (w.b[0] - w.a[0], w.b[1] - w.a[1]);
            let len = dx.hypot(dy).max(1e-9);
            let mut along: Vec<(f64, f64)> =
                members.iter().map(|m| (((m.1[0] - w.a[0]) * dx + (m.1[1] - w.a[1]) * dy) / len / upf * 12.0, m.2)).collect();
            along.sort_by(|a, b| a.0.total_cmp(&b.0));
            let largest = along.iter().map(|x| x.1).fold(0.0f64, f64::max);
            let tightest = along
                .windows(2)
                .filter_map(|p| Some((p.first()?, p.get(1)?)))
                .map(|(a, b)| (b.0 - a.0, b.0 - a.0 - (a.1 + b.1) / 2.0))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let Some((centres, clear)) = tightest else { continue };
            let (need, need_oc) = match &rule {
                Some((v, oc, _)) => (*v, *oc),
                None => (pr.default_clear_diameters * largest, false),
            };
            let spacing_short = if need_oc { centres + 1e-6 < need } else { clear + 1e-6 < need };
            let too_wide = largest > pr.max_plate_fraction * plate_in;
            if !spacing_short && !too_wide {
                continue;
            }
            let systems: BTreeSet<&str> = members.iter().filter_map(|m| sp.mep.runs.get(m.0)).map(|r| r.system.as_str()).collect();
            let systems = systems.into_iter().collect::<Vec<_>>().join("/");
            let sheets = sheets_of(
                members
                    .iter()
                    .filter_map(|m| sp.mep.runs.get(m.0))
                    .map(|r| r.source_sheet.as_str())
                    .filter(|x| cadaddy_mepfix::sheet_facts::is_sheet_number(x)),
            );
            let at = members.first().map_or([0.0, 0.0], |m| m.1);
            // One finding per kind of group (the same risers in the same wall type: a typical
            // detail repeated), its worst case and every level it is on.
            let key = (systems.clone(), w.wall_type.clone(), (largest * 8.0).round() as i64);
            let e = hits.entry(key).or_insert_with(|| Hit { level: level.clone(), plate_in, at, ..Hit::default() });
            e.groups += 1;
            if !e.levels.contains(level) {
                e.levels.push(level.clone());
            }
            e.most = e.most.max(members.len());
            e.largest = largest;
            if e.clear.is_none_or(|c| clear < c) {
                e.clear = Some(clear);
                e.centres = centres;
            }
            e.spacing_short |= spacing_short;
            e.too_wide |= too_wide;
            for sh in sheets {
                if e.sheets.len() < MAX_SHEETS && !e.sheets.contains(&sh) {
                    e.sheets.push(sh);
                }
            }
        }
    }
    let mut rfis = Vec::new();
    for ((systems, wall_type, _), h) in &hits {
        out.raised += 1;
        let (need, need_basis, need_oc) = match &rule {
            Some((v, oc, note)) => (*v, note.clone(), *oc),
            None => (pr.default_clear_diameters * h.largest, pr.default_basis.clone(), false),
        };
        let clear = h.clear.unwrap_or(0.0);
        let what = format!("up to {} {systems} risers of {}", h.most, inches(h.largest));
        let mut why: Vec<String> = Vec::new();
        if h.spacing_short {
            why.push(format!(
                "{} {} where the hole rule wants {} {}",
                inches(if need_oc { h.centres } else { clear.max(0.0) }),
                if need_oc { "on centres" } else { "clear" },
                inches(need),
                if need_oc { "on centres" } else { "clear" }
            ));
        }
        if h.too_wide {
            why.push(format!("each hole {} of the {} plate (over {:.0}%)", inches(h.largest), inches(h.plate_in), pr.max_plate_fraction * 100.0));
        }
        let issue = format!(
            "{} group(s) of {what} stand side by side in a type {wall_type} wall ({}) and go through its plate: {}.",
            h.groups,
            h.levels.join(", "),
            why.join("; ")
        );
        let assumed = format!(
            "Corrected for the field: each group framed as one opening in the plate with a steel tie across it ({}) and the risers spread to the rule's spacing where the wall allows; alternative: a framed chase beside the wall so the plate stays whole.",
            pr.plate_basis
        );
        let proposed = "Frame each group's plate opening with a tie (or a chase) and confirm with the structural engineer: which?".to_string();
        let sides = vec![
            RfiSide { document: named(&h.sheets), says: format!("{what} at {} on centres", inches(h.centres)) },
            RfiSide { document: need_basis.split(':').next().unwrap_or("").trim().to_string(), says: need_basis.clone() },
            RfiSide { document: format!("wall type {wall_type}"), says: format!("{} plate", inches(h.plate_in)) },
        ];
        let subject = format!("{systems} {wall_type} {}", inches(h.largest));
        rfis.push(finding(
            "penetration groups",
            &h.level,
            h.at,
            &subject,
            format!("{systems} risers through one plate ({wall_type})"),
            issue,
            assumed,
            proposed,
            sides,
            h.sheets.clone(),
        ));
    }
    if by_level.is_empty() {
        out.skip("no risers in the model to group".into());
    }
    rfis
}

// ─── Site objects ────────────────────────────────────────────────────────────

/// A site object: what it is (its words), whose it is, where it lies.
struct SiteObject {
    words: String,
    discipline: String,
    sheet: String,
    outline: Vec<[f64; 2]>,
}

fn poly_dist(a: &[[f64; 2]], b: &[[f64; 2]]) -> f64 {
    if a.iter().any(|p| b.len() >= 3 && inside(*p, b)) || b.iter().any(|p| a.len() >= 3 && inside(*p, a)) {
        return 0.0;
    }
    let edges = |o: &[[f64; 2]]| -> Vec<([f64; 2], [f64; 2])> {
        if o.len() == 1 {
            return o.first().map(|p| vec![(*p, *p)]).unwrap_or_default();
        }
        let closed = o.len() >= 3;
        let n = if closed { o.len() } else { o.len().saturating_sub(1) };
        (0..n).filter_map(|i| Some((*o.get(i)?, *o.get((i + 1) % o.len())?))).collect()
    };
    let (ea, eb) = (edges(a), edges(b));
    let mut best = f64::INFINITY;
    for (p, q) in &ea {
        for (r, s) in &eb {
            best = best.min(seg_dist(*p, *r, *s)).min(seg_dist(*q, *r, *s)).min(seg_dist(*r, *p, *q)).min(seg_dist(*s, *p, *q));
        }
    }
    best
}

/// Site objects of two disciplines inside one another's clearance.
fn site(sp: &BuildingSpec, out: &mut Outcome) -> Vec<Rfi> {
    let sr = &rules().site;
    let upf = sp.units_per_foot.abs().max(1e-9);
    let disc = |sheet: &str| cadaddy_mepfix::sheet_facts::discipline_of(sheet).to_ascii_uppercase();
    let mut objs: Vec<SiteObject> = Vec::new();
    let site_level = |level: &str| {
        sp.levels.iter().min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)).is_some_and(|l| l.name.eq_ignore_ascii_case(level))
            || level.is_empty()
    };
    for e in sp.mep.equipment.iter().take(sr.max_objects.max(1)) {
        let sheet = e.sources.get("position").or_else(|| e.sources.get("outline")).cloned().unwrap_or_default();
        let sheet = sheet.split_whitespace().find(|w| cadaddy_mepfix::sheet_facts::is_sheet_number(w)).unwrap_or("").to_string();
        let ok = e.min.iter().chain(e.max.iter()).all(|v| v.is_finite()) && e.max[0] >= e.min[0] && e.max[1] >= e.min[1];
        if sheet.is_empty() || !ok || !site_level(&e.level) || !e.sources.contains_key(&sr.site_source) {
            continue;
        }
        let outline = vec![e.min, [e.max[0], e.min[1]], e.max, [e.min[0], e.max[1]]];
        objs.push(SiteObject { words: format!(" {} {} {} ", e.tag, e.kind, e.title).to_ascii_uppercase(), discipline: disc(&sheet), sheet, outline });
    }
    for r in sp.mep.runs.iter().take(sr.max_objects.max(1)) {
        if r.source_sheet.is_empty() || r.points.len() < 2 || r.top_ft.is_some() || !site_level(&r.level) {
            continue;
        }
        let on_site = sr.run_disciplines.iter().any(|x| x.eq_ignore_ascii_case(&disc(&r.source_sheet)))
            || sr.run_system_prefixes.iter().any(|x| r.system.to_ascii_uppercase().starts_with(&x.to_ascii_uppercase()));
        if !on_site {
            continue;
        }
        objs.push(SiteObject {
            words: format!(" {} ", r.system.to_ascii_uppercase()),
            discipline: disc(&r.source_sheet),
            sheet: r.source_sheet.clone(),
            outline: r.points.clone(),
        });
    }
    let names = |o: &SiteObject, ws: &[String]| {
        ws.iter().any(|w| o.words.split(|c: char| c.is_whitespace() || c == '-').any(|x| x == w) || o.words.contains(&format!(" {w} ")))
    };
    let mut rfis = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for pair in &sr.pairs {
        for a in objs.iter().filter(|o| names(o, &pair.a)) {
            for b in objs.iter().filter(|o| names(o, &pair.b) && o.discipline != a.discipline && !o.discipline.is_empty()) {
                out.compared += 1;
                let d_ft = poly_dist(&a.outline, &b.outline) / upf;
                if d_ft + 1e-6 >= pair.clear_ft {
                    continue;
                }
                let key = format!("{} {} {}", a.words, b.words, pair.what);
                if !seen.insert(key.clone()) {
                    continue;
                }
                out.raised += 1;
                let at = centre(&a.outline);
                let sheets = sheets_of([a.sheet.as_str(), b.sheet.as_str()]);
                let (aw, bw) = (a.words.trim().to_ascii_lowercase(), b.words.trim().to_ascii_lowercase());
                let issue = format!(
                    "{} ({}) and {} ({}) are {} apart where {} wants {}: neither document shows the clearance.",
                    aw,
                    a.sheet,
                    bw,
                    b.sheet,
                    feet(d_ft),
                    pair.what,
                    feet(pair.clear_ft)
                );
                let assumed = format!(
                    "Corrected for the field: {bw} set {} clear of {aw} (moved away from it, the {} discipline's object kept where drawn); alternative: move {aw} instead.",
                    feet(pair.clear_ft),
                    a.discipline
                );
                let proposed = format!("Coordinate {} and {}: who moves to keep {} clear?", a.sheet, b.sheet, feet(pair.clear_ft));
                let sides = vec![
                    RfiSide { document: a.sheet.clone(), says: format!("{aw} drawn here") },
                    RfiSide { document: b.sheet.clone(), says: format!("{bw} drawn {} from it", feet(d_ft)) },
                    RfiSide { document: "clearance rule".into(), says: pair.what.clone() },
                ];
                rfis.push(finding("site objects", "", at, &key, format!("{aw} against {bw}"), issue, assumed, proposed, sides, sheets));
            }
        }
    }
    if objs.is_empty() {
        out.skip("no site objects with a source sheet in the model".into());
    }
    rfis
}

// ─── Into the model ──────────────────────────────────────────────────────────

/// Write a report's RFIs into the spec: an item answered keeps its answer; an open item of these
/// checks no longer raised goes. The number written.
pub(crate) fn apply(s: &mut Session, rep: &Report) -> Result<usize> {
    let (mut sp, stored) = super::bspec::current(s, "doccheck")?;
    if !stored {
        return Ok(0);
    }
    let raised: BTreeSet<&str> = rep.rfis.iter().map(|r| r.id.as_str()).collect();
    sp.rfis.retain(|x| !x.topic.starts_with(TOPIC) || x.status != RfiStatus::Open || raised.contains(x.id.as_str()));
    let mut n = 0usize;
    for r in &rep.rfis {
        let len = sp.rfis.len();
        match sp.rfis.iter_mut().find(|x| x.id == r.id) {
            Some(x) if x.status == RfiStatus::Open => {
                *x = r.clone();
                n += 1;
            }
            Some(_) => {}
            None if len < cadaddy_walk3d::rfi::MAX_RFIS => {
                sp.rfis.push(r.clone());
                n += 1;
            }
            None => {}
        }
    }
    s.doc_mut()?.building_spec = Some(sp.to_json()).into();
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_geom::Vec2;

    fn t(value: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: value.into(), at: Vec2::new(x, y), height: 1.0, rotation: 0.0 }
    }

    #[test]
    fn the_rules_read() {
        let r = rules();
        assert!(!r.ceiling_space.items.is_empty());
        assert!(!r.conditions.is_empty());
        assert!(r.levels.tolerance_in > 0.0);
        assert!(!r.assembly.space_kinds.is_empty());
    }

    #[test]
    fn a_note_set_one_line_per_text_is_one_block() {
        let texts = vec![
            t("1.", 0.0, 10.0),
            t("SET EACH WATER MANIFOLD ABOVE", 2.0, 10.0),
            t("THE CORRIDOR CEILING NEAR ITS UNIT.", 2.0, 8.5),
            t("FAR AWAY TEXT", 200.0, 8.5),
        ];
        let b = blocks(&texts);
        assert!(b.iter().any(|b| b.text.contains("MANIFOLD ABOVE THE CORRIDOR CEILING NEAR")), "{b:?}");
    }

    #[test]
    fn work_above_a_space_ceiling_is_read_with_its_size() {
        let cr = &rules().ceiling_space;
        let r = routed_in("RUN 2 1/2\" HOT WATER MAINS ABOVE THE CORRIDOR CEILING.", cr);
        let (space, items) = r.first().expect("routed");
        assert_eq!(space, &vec!["CORRIDOR".to_string()]);
        let main = items.iter().find(|i| i.0 == "main").expect("main");
        assert!((main.1 - (2.5 + 1.0 + 1.5)).abs() < 1e-9, "{items:?}");
        // No space between ABOVE and CEILING: no routing to a space.
        assert!(routed_in("CONDUIT STUBBED INTO CORRIDOR ABOVE CEILING.", cr).is_empty());
    }

    #[test]
    fn inch_quantities_read_as_printed() {
        let ws = words("3 1/2\" BATT IN 16\" TRUSSES AND 3-1/2\" AND 12\"X8\" DUCT");
        let v: Vec<f64> = inch_values(&ws).into_iter().map(|x| x.0).collect();
        assert_eq!(v, vec![3.5, 16.0, 3.5, 8.0]);
    }

    #[test]
    fn a_level_name_over_its_elevation_is_a_mark() {
        let texts = vec![t("3RD FLOOR F.F.", 0.0, 10.0), t("121' - 4 1/2\"", 0.0, 8.5), t("LEVEL 2 = 10'-6\"", 50.0, 0.0)];
        let m = level_marks(&texts, 3.0);
        let l3 = m.get("LEVEL 3").expect("level 3");
        assert!((l3.0 - (121.0 + 4.5 / 12.0)).abs() < 1e-9, "{m:?}");
        assert!((m.get("LEVEL 2").expect("level 2").0 - 10.5).abs() < 1e-9);
        // An elevation set under the name and to its right, over three heights from its start.
        let offset = vec![t("4TH FLOOR F.F.", 0.0, 10.0), t("131' - 2 1/2\"", 2.7, 8.2)];
        assert!((level_marks(&offset, rules().levels.reach_heights).get("LEVEL 4").expect("level 4").0 - (131.0 + 2.5 / 12.0)).abs() < 1e-9);
        // Lettering read off strokes is no mark.
        let junk = vec![t("1 / 1'", 0.0, 0.0), t("/////1////1'////-/", 40.0, 0.0), t(":1'-------:.===/:.1", 80.0, 0.0)];
        assert!(level_marks(&junk, 3.0).is_empty(), "{:?}", level_marks(&junk, 3.0));
    }

    /// A two-level spec (inches) with one 2x6 wall on L1 and the given risers (x, diameter).
    fn riser_spec(risers: &[(f64, f64)]) -> BuildingSpec {
        let runs: Vec<Value> = risers
            .iter()
            .map(|(x, d)| json!({"system": "EA", "level": "L1", "points": [[x, 0.0]], "size_in": [d, 0.0], "bottoms_ft": [8.0], "top_ft": 19.0, "source_sheet": "M-501"}))
            .collect();
        BuildingSpec::from_json(
            &json!({
                "version": 1, "units_per_foot": 12.0,
                "levels": [{"name": "L1", "elevation_ft": 0.0, "height_ft": 10.0}, {"name": "L2", "elevation_ft": 10.0, "height_ft": 10.0}],
                "wall_types": [{"name": "W6", "thickness_in": 7.25, "from": "pairs", "assembly": {"tag": "W6", "layers": [
                    {"material": "5/8\" gypsum board", "thickness_in": 0.625, "side": "side1", "category": "board"},
                    {"material": "2x6 wood studs", "thickness_in": 5.5, "side": "core", "category": "framing"},
                    {"material": "5/8\" gypsum board", "thickness_in": 0.625, "side": "side2", "category": "board"}]}}],
                "walls": [{"level": "L1", "a": [0.0, 0.0], "b": [480.0, 0.0], "wall_type": "W6"}],
                "mep": {"runs": runs}
            })
            .to_string(),
        )
        .expect("spec")
    }

    #[test]
    fn risers_side_by_side_through_one_plate_conflict_with_the_hole_rule() {
        // Six 4" ducts at 5" centres in one 2x6 wall: 1" clear, and each hole most of the plate.
        let sp = riser_spec(&[(100.0, 4.0), (105.0, 4.0), (110.0, 4.0), (115.0, 4.0), (120.0, 4.0), (125.0, 4.0)]);
        let rep = check(&[], &[], &sp);
        let found: Vec<&Rfi> = rep.rfis.iter().filter(|r| r.topic == "documents: penetration groups").collect();
        assert_eq!(found.len(), 1, "{:?}", rep.rfis);
        let issue = &found[0].issue;
        assert!(issue.contains("6 EA risers") && issue.contains("1\" clear") && issue.contains("5 1/2\" plate"), "{issue}");
        // One riser drawn twice and a bundle of cables are no group of holes.
        let sp = riser_spec(&[(100.0, 4.0), (101.0, 4.0), (130.0, 4.0), (200.0, 0.25), (200.0, 0.25), (200.5, 0.25), (201.0, 0.25)]);
        assert!(check(&[], &[], &sp).rfis.is_empty());
        // Three small pipes a foot apart: within the trade rule and the plate's half.
        let sp = riser_spec(&[(100.0, 1.5), (112.0, 1.5), (124.0, 1.5)]);
        let rep = check(&[], &[], &sp);
        assert!(rep.rfis.iter().all(|r| r.topic != "documents: penetration groups"), "{:?}", rep.rfis);
        assert_eq!(rep.outcomes.iter().find(|o| o.check == "penetration groups").map(|o| o.compared), Some(1));
        // The documents' own rule (24" on centres) governs over the trade's.
        let page = Page {
            sheet: "S-001".into(),
            discipline: "S".into(),
            levels: vec![],
            texts: vec![t("HOLES IN STUDS AND PLATES SHALL BE SPACED 24\" MIN. O.C.", 0.0, 0.0)],
        };
        let rep = check(&[page], &[], &sp);
        let found: Vec<&Rfi> = rep.rfis.iter().filter(|r| r.topic == "documents: penetration groups").collect();
        assert_eq!(found.len(), 1, "{:?}", rep.rfis);
        assert!(found[0].sides.iter().any(|x| x.document == "S-001"), "{:?}", found[0].sides);
    }

    #[test]
    fn site_objects_of_two_disciplines_inside_a_clearance_conflict() {
        let spec = |water_y: f64| {
            BuildingSpec::from_json(
                &json!({
                    "version": 1, "units_per_foot": 12.0,
                    "levels": [{"name": "L1", "elevation_ft": 0.0, "height_ft": 10.0}],
                    "mep": {
                        "equipment": [{"tag": "XFMR-1", "kind": "transformer", "level": "L1", "min": [0.0, 0.0], "max": [72.0, 72.0], "bottom_ft": 0.0, "height_ft": 5.0,
                            "sources": {"position": "drawn: drawn on E-801 (site plan)", "site": "outside the building"}}],
                        "runs": [{"system": "C-UG-DOMESTIC WATER", "level": "L1", "points": [[-200.0, water_y], [300.0, water_y]], "size_in": [3.0, 0.0], "bottoms_ft": [-3.0], "source_sheet": "C-301"}]
                    }
                })
                .to_string(),
            )
            .expect("spec")
        };
        // The water line 1 ft off the pad.
        let rep = check(&[], &[], &spec(84.0));
        let found: Vec<&Rfi> = rep.rfis.iter().filter(|r| r.topic == "documents: site objects").collect();
        assert_eq!(found.len(), 1, "{:?}", rep.rfis);
        assert!(found[0].sheets.contains(&"E-801".to_string()) && found[0].sheets.contains(&"C-301".to_string()), "{:?}", found[0]);
        // 10 ft off: clear.
        let rep = check(&[], &[], &spec(192.0));
        assert!(rep.rfis.iter().all(|r| r.topic != "documents: site objects"), "{:?}", rep.rfis);
    }

    #[test]
    fn an_assembly_gives_its_members_and_insulation() {
        let ar = &rules().assembly;
        let ws = words("FLOOR ASSEMBLY: 3/4\" SUBFLOOR ON 16\" FLOOR TRUSSES @ 24\" O.C. WITH 3 1/2\" SOUND ATTENUATION BATT");
        assert_eq!(member_depth(&ws, ar), Some(16.0));
        assert_eq!(insulation_thickness(&ws, ar), Some(3.5));
        assert_eq!(insulation_thickness(&words("2X12 FLOOR JOISTS WITH R-19 INSULATION"), ar), Some(6.25));
        assert_eq!(member_depth(&words("2X12 FLOOR JOISTS WITH R-19 INSULATION"), ar), Some(11.25));
    }
}
