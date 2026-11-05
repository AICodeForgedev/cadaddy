//! The plumbing fixtures' product records (Q6205), applied by `bim.products` before it writes
//! each element's `product` record.
//!
//! Each plumbing fixture and piece of plumbing equipment the build made (a water closet, a
//! lavatory, a floor drain, a water heater, a pump) gets its product from the documents, each
//! field with where it came from:
//!
//! 1. **The plumbing fixture schedule** (its tag's row): the fixture, maker, model, description,
//!    remarks and parts, and the connection sizes its columns give, else the rough-in table beside
//!    it (by the fixture's name: trap, waste, vent, cold and hot water).
//! 2. **The plumbing submittals** (the latest revision of each item that wasn't sent back): the
//!    product list (the item's tag, each product's maker, description and the package page its cut
//!    sheet starts on), the review letter's response for the item, and the catalog number and
//!    electrical data its cut sheet prints.
//! 3. **The Division 22 specification's approved makers** for the fixture's kind (read by
//!    `bim.products`), where neither gives a maker; and a finding where the maker kept isn't on the
//!    specification's list.
//!
//! Where the documents disagree the approved submittal is kept (it is what was bought), the
//! schedule's value is kept beside it (`design_manufacturer`, `design_catalog`) and a conflict is
//! raised with both sides. A fixture whose tag nothing documents keeps no product and says why
//! (`product_gap`), so Validate's G-PRODUCT names the reason.
//!
//! A fixture that takes power (a pump; one its documents call sensor-operated, line-voltage or
//! power-burner; rules data `powered`) gets its electrical connection (`electrical_connection`
//! with the volts, phase and amps its documents give) and an equipment connection box beside it
//! in the building spec (the documented objects' power box: same kind, size and rule), so the
//! electrical takeoff carries the connection; its panel and circuit when a panel schedule names
//! its tag.
//!
//! A fixture whose level's water lines are proposed (copied from another level, not drawn) has
//! its water connections marked derived (`water_provenance`), never design-issued.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Drawing, Handle};
use cadaddy_mepfix::fixture_schedule::{self as fs, ConnectionRow, ScheduledFixture};
use serde::Deserialize;
use serde_json::{Value, json};

use super::products::Requirement;
use super::propfill::WithNote;
use crate::Session;

/// The rules data.
const RULES: &str = include_str!("plumbproducts.json");
/// Submittal files looked at, at most.
const MAX_FILES: usize = 80;
/// Pages read of one submittal.
const MAX_PAGES: u32 = 400;
/// Elements given a record, at most.
const MAX_ELEMENTS: usize = 200_000;
/// Products listed under one item, at most.
const MAX_PRODUCTS: usize = 40;
/// Characters of a value kept.
const MAX_VALUE: usize = 240;
/// Conflicts raised, at most.
const MAX_CONFLICTS: usize = 400;
/// The supply voltages a cut sheet's lone voltage is taken for.
const SUPPLY_VOLTS: [f64; 11] = [110.0, 115.0, 120.0, 208.0, 220.0, 230.0, 240.0, 277.0, 460.0, 480.0, 575.0];
/// Fixtures listed per reason in the report.
const MAX_LISTED: usize = 30;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct KindWords {
    kind: String,
    words: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ListHeaders {
    maker: Vec<String>,
    description: Vec<String>,
    page: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Review {
    sent_back: Vec<String>,
    not_reviewed: Vec<String>,
    approved: Vec<String>,
    /// Words of a letter's line saying the items it doesn't list are returned approved.
    unlisted: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Powered {
    kinds: Vec<String>,
    words: Vec<String>,
    basis: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    kinds: Vec<KindWords>,
    narrowing_words: Vec<String>,
    narrowing_why: String,
    list_headers: ListHeaders,
    catalog_labels: Vec<String>,
    weak_catalog_labels: Vec<String>,
    review: Review,
    submittal_name_words: Vec<String>,
    sniff_pages: u32,
    powered: Vec<Powered>,
    water_derived: String,
    water_systems: Vec<String>,
}

/// The rules data (empty lists when it can't be read: nothing classified, nothing powered).
pub(crate) fn rules() -> Rules {
    serde_json::from_str(RULES).unwrap_or_default()
}

/// The plumbing product kind a name says (`FLOOR DRAIN` → `floor_drain`, `ADA LAVATORY` →
/// `lavatory`): the rules data's first kind one of whose words it contains.
pub(crate) fn plumbing_kind(name: &str) -> Option<String> {
    kind_by(&rules(), name)
}

fn kind_by(r: &Rules, name: &str) -> Option<String> {
    let toks =
        |t: &str| -> Vec<String> { t.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect() };
    let have = toks(name);
    // A word or phrase whole (`TUB` is no part of `TUBING`); its last word may be plural.
    let says = |phrase: &str| {
        let want = toks(phrase);
        let n = want.len();
        n > 0
            && have.windows(n).any(|w| {
                w.iter()
                    .zip(&want)
                    .enumerate()
                    .all(|(i, (a, b))| a == b || (i + 1 == n && (a.strip_suffix('S') == Some(b) || a.strip_suffix("ES") == Some(b))))
            })
    };
    r.kinds.iter().take(200).find(|k| k.words.iter().any(|w| says(w))).map(|k| k.kind.clone())
}

// ---------------------------------------------------------------- small helpers

/// A value cut to a sane length, its spaces collapsed.
fn clean(v: &str) -> String {
    v.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_VALUE).collect()
}

/// A tag compared without case or spaces.
fn tag_key(t: &str) -> String {
    t.chars().filter(|c| !c.is_whitespace()).take(40).collect::<String>().to_ascii_uppercase()
}

/// A maker's letters (`Maker Co.` → `MAKERCO`).
fn maker_key(m: &str) -> String {
    m.chars().filter(char::is_ascii_alphanumeric).take(80).collect::<String>().to_ascii_uppercase()
}

/// Edit distance, both texts capped.
fn edits(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().take(64).collect();
    let b: Vec<char> = b.chars().take(64).collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev.get(j).copied().unwrap_or(0) + usize::from(ca != cb);
            let del = prev.get(j + 1).copied().unwrap_or(0) + 1;
            let ins = cur.get(j).copied().unwrap_or(0) + 1;
            if let Some(c) = cur.get_mut(j + 1) {
                *c = sub.min(del).min(ins);
            }
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

/// Whether two makers are one: the same letters, one the other's start (`ACME` and `Acme
/// Plumbing`, `ACME` and `Acme Co`), or a misspelling of it (two letters off in six or more).
pub(crate) fn same_maker(a: &str, b: &str) -> bool {
    let (x, y) = (maker_key(a), maker_key(b));
    if x.is_empty() || y.is_empty() {
        return false;
    }
    x == y || x.starts_with(&y) || y.starts_with(&x) || (x.len().min(y.len()) >= 6 && edits(&x, &y) <= 2)
}

/// A catalog number compared without case, spaces or punctuation.
fn catalog_key(c: &str) -> String {
    c.chars().filter(char::is_ascii_alphanumeric).take(80).collect::<String>().to_ascii_uppercase()
}

/// A long text's letters and digits, capitals (a cut sheet's text to look a catalog number up in).
fn catalog_key_long(c: &str) -> String {
    c.chars().filter(char::is_ascii_alphanumeric).take(400_000).collect::<String>().to_ascii_uppercase()
}

/// Whether a text reads like a catalog number: a few words at most, with a digit, at least three
/// characters (`AB12/C3-X`, `12345678`, `XT-30`).
fn catalog_like(v: &str) -> bool {
    let t = v.trim().trim_matches(|c: char| c == ':' || c == '#' || c == '.').trim();
    let words = t.split_whitespace().count();
    let alnum = t.chars().filter(char::is_ascii_alphanumeric).count();
    (1..=3).contains(&words) && (3..=40).contains(&alnum) && t.chars().any(|c| c.is_ascii_digit()) && t.len() <= 60
}

/// The catalog number a label's text starts with: its first word (a word that has a digit), and
/// the words a slash or dash joins to it (`AB12 /C3-X`); none when the first word has no digit
/// (`ex: …`, a table's `S` header) or is under three characters (a figure's `17`).
fn catalog_at(v: &str) -> Option<String> {
    let mut ws = v.split_whitespace();
    let mut out = ws.next()?.trim_end_matches([',', ';', '.']).to_string();
    for w in ws.take(4) {
        if out.ends_with(['/', '-']) || w.starts_with(['/', '-']) {
            out.push(' ');
            out.push_str(w.trim_end_matches([',', ';', '.']));
        } else {
            break;
        }
    }
    let first_ok = out.chars().any(|c| c.is_ascii_digit()) && out.chars().filter(char::is_ascii_alphanumeric).count() >= 3;
    (first_ok && catalog_like(&out)).then_some(out)
}

/// Whether two catalog numbers are one product's: the same, one in the other, or one maker's
/// family and its full order code (`Q200` and `QN200XB`: the same first letter and the shorter
/// one's run of digits, three or more, in the longer).
fn same_catalog(a: &str, b: &str) -> bool {
    let (x, y) = (catalog_key(a), catalog_key(b));
    if x.is_empty() || y.is_empty() {
        return false;
    }
    if x == y || x.contains(&y) || y.contains(&x) {
        return true;
    }
    let digits = |k: &str| k.split(|c: char| !c.is_ascii_digit()).filter(|r| r.len() >= 3).max_by_key(|r| r.len()).map(str::to_string);
    let (short, long) = if x.len() <= y.len() { (&x, &y) } else { (&y, &x) };
    short.chars().next() == long.chars().next() && digits(short).is_some_and(|d| long.contains(&d))
}

/// A PDF text line's cells (its column breaks).
fn cells(line: &str) -> Vec<String> {
    line.split("   ").map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect()
}

/// A package's own page footer (`Page 36 of 193`): (36, 193).
fn footer(line: &str) -> Option<(u32, u32)> {
    let t = line.trim();
    let rest = t.strip_prefix("Page ").or_else(|| t.strip_prefix("PAGE "))?;
    let (a, b) = rest.split_once(" of ").or_else(|| rest.split_once(" OF "))?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

// ---------------------------------------------------------------- the submittals

/// An item's review status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Status {
    /// Approved (no exceptions, or approved as noted).
    Approved,
    /// Revise and resubmit, rejected.
    SentBack,
    NotReviewed,
    /// No review letter or stamp read.
    #[default]
    Unknown,
}

impl Status {
    fn words(self) -> &'static str {
        match self {
            Status::Approved => "approved",
            Status::SentBack => "sent back",
            Status::NotReviewed => "not reviewed",
            Status::Unknown => "no review read",
        }
    }
}

/// What a review's words say.
fn status_of(r: &Rules, text: &str) -> Option<Status> {
    let u = text.to_ascii_uppercase();
    let any = |ws: &[String]| ws.iter().any(|w| u.contains(&w.to_ascii_uppercase()));
    if any(&r.review.sent_back) {
        Some(Status::SentBack)
    } else if any(&r.review.not_reviewed) {
        Some(Status::NotReviewed)
    } else if any(&r.review.approved) {
        Some(Status::Approved)
    } else {
        None
    }
}

/// One product a submittal lists under an item: its maker, description and the package page its
/// cut sheet starts on.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SubProduct {
    pub vendor: String,
    pub description: String,
    pub page: Option<u32>,
}

/// One item (a schedule tag) of a submittal.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SubItem {
    pub products: Vec<SubProduct>,
    pub status: Status,
    /// The review's response, as written.
    pub response: String,
    /// The catalog number its first product's cut sheet prints, and the page.
    pub catalog: Option<(String, Option<u32>)>,
    /// Every value its first product's pages print under a catalog label (value, page, strong).
    pub catalogs: Vec<(String, Option<u32>, bool)>,
    /// What its cut sheets say electrically (volts, phase, amps…), and the page.
    pub electrical: Option<(cadaddy_walk3d::EquipmentElectrical, Option<u32>)>,
    /// Its cut sheets' text (the first product's pages), for the power words, and the same as one
    /// key (letters and digits) to find a catalog number in.
    pub sheet_text: String,
    pub sheet_key: String,
}

/// A plumbing submittal read: its file, revision and items by tag.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Submittal {
    pub file: String,
    pub revision: (u32, u32),
    pub reviewed: bool,
    pub items: BTreeMap<String, SubItem>,
}

impl Submittal {
    fn doc(&self, page: Option<u32>) -> String {
        let rev = if self.revision == (0, 0) { String::new() } else { format!(" rev {}-{}", self.revision.0, self.revision.1) };
        match page {
            Some(p) => format!("submittal {}{rev} p.{p}", self.file),
            None => format!("submittal {}{rev}", self.file),
        }
    }
}

/// Whether a line is a product list's header (`VENDOR   DESCRIPTION   PAGE`).
fn list_header(r: &Rules, line: &str) -> bool {
    let cs: Vec<String> = cells(line).iter().map(|c| c.to_ascii_uppercase()).collect();
    let has = |ws: &[String]| cs.iter().any(|c| ws.iter().any(|w| c == &w.to_ascii_uppercase()));
    has(&r.list_headers.maker) && has(&r.list_headers.description) && has(&r.list_headers.page)
}

/// A submittal's text read for the items it lists of `known` (the schedule's tags): the product
/// lists (a table of contents, an item's cover page), the review letter's responses, each item's
/// cut sheets (their pages carry the item's tag first), and on them its catalog number and
/// electrical data. None when it lists none of the tags.
pub(crate) fn read_submittal(r: &Rules, text: &str, file: &str, path: &str, known: &BTreeSet<String>) -> Option<Submittal> {
    let is_known = |t: &str| known.contains(&tag_key(t));
    let pages: Vec<&str> = text.split("--- page ").skip(1).collect();
    let mut sub = Submittal { file: file.to_string(), revision: super::equipsub::revision(path), ..Submittal::default() };
    // The package's own numbering: the most common `of N` among the footers.
    let mut of_count: BTreeMap<u32, usize> = BTreeMap::new();
    for p in &pages {
        for l in p.lines() {
            if let Some((_, of)) = footer(l) {
                *of_count.entry(of).or_default() += 1;
            }
        }
    }
    let package_of = of_count.iter().max_by_key(|(_, n)| **n).map(|(of, _)| *of);
    // The review letter: its codes' legend and each item's row.
    let mut legend: BTreeMap<String, String> = BTreeMap::new();
    let mut cur: Option<String> = None;
    let mut sheets: BTreeMap<String, Vec<(Option<u32>, String)>> = BTreeMap::new();
    for page in &pages {
        let lines: Vec<&str> = page.lines().skip(1).map(str::trim).filter(|l| !l.is_empty()).collect();
        let printed = lines.iter().rev().find_map(|l| footer(l)).filter(|(_, of)| Some(*of) == package_of).map(|(n, _)| n);
        // The legend: `1 – Note Corrections   3 – Rejected`.
        for l in &lines {
            for c in cells(l) {
                let t = c.replace('–', "-");
                if let Some((code, words)) = t.split_once('-')
                    && code.trim().len() <= 2
                    && code.trim().chars().all(|ch| ch.is_ascii_digit())
                    && !code.trim().is_empty()
                    && status_of(r, words).is_some()
                {
                    legend.insert(code.trim().to_string(), words.trim().to_string());
                }
            }
        }
        // The review rows: `9   1   P-1   Must have …` (item number, code, tag, response).
        let mut pending: Vec<String> = Vec::new();
        let mut last_row: Option<String> = None;
        for l in &lines {
            let cs = cells(l);
            let numbered = cs.first().is_some_and(|c| c.len() <= 3 && c.chars().all(|ch| ch.is_ascii_digit()));
            let tag_at = cs.iter().take(3).position(|c| is_known(c));
            match (numbered, tag_at) {
                (true, Some(i)) if i >= 1 => {
                    let tag = cs.get(i).map(|c| tag_key(c)).unwrap_or_default();
                    let code = cs.get(1..i).and_then(|m| m.iter().rev().find(|c| c.len() <= 2 && c.chars().all(|ch| ch.is_ascii_digit()))).cloned();
                    let said = cs.get(i + 1..).map(|m| m.join(" ")).unwrap_or_default();
                    let mut response = std::mem::take(&mut pending);
                    response.push(said);
                    let response = clean(&response.join(" "));
                    let status = code
                        .as_ref()
                        .and_then(|c| legend.get(c))
                        .and_then(|w| status_of(r, w))
                        .or_else(|| status_of(r, &response))
                        .unwrap_or(Status::Approved);
                    let it = sub.items.entry(tag.clone()).or_default();
                    it.status = status;
                    it.response = match code.as_ref().and_then(|c| legend.get(c)) {
                        Some(w) => clean(&format!("{} ({w}): {response}", code.clone().unwrap_or_default())),
                        None => response,
                    };
                    sub.reviewed = true;
                    last_row = Some(tag);
                }
                _ if !numbered && cs.len() == 1 && !legend.is_empty() && last_row.is_some() => {
                    let one = cs.first().cloned().unwrap_or_default();
                    // A sentence's end goes with the row above; its start with the row below.
                    if one.ends_with('.') {
                        if let Some(it) = last_row.as_ref().and_then(|t| sub.items.get_mut(t)) {
                            it.response = clean(&format!("{} {one}", it.response));
                        }
                    } else {
                        pending.push(one);
                    }
                }
                _ => {}
            }
        }
        // The page's item: its first line a known tag.
        let page_tag = lines.first().filter(|l| cells(l).len() == 1 && is_known(l)).map(|l| tag_key(l));
        let is_list = lines.iter().any(|l| list_header(r, l));
        if is_list {
            // A product list: an item's tag on its own line, then its products (maker,
            // description, page; a description's second line carries the page).
            if let Some(t) = &page_tag {
                cur = Some(t.clone());
            }
            let mut after_header = false;
            for l in &lines {
                if list_header(r, l) {
                    after_header = true;
                    continue;
                }
                if !after_header || footer(l).is_some() {
                    continue;
                }
                let mut cs = cells(l);
                if cs.len() == 1 && cs.first().is_some_and(|c| fs::is_fixture_tag(c)) {
                    cur = cs.first().filter(|c| is_known(c)).map(|c| tag_key(c));
                    continue;
                }
                if cs.first().is_some_and(|c| is_known(c) && cs.len() >= 3) {
                    cur = cs.first().map(|c| tag_key(c));
                    cs.remove(0);
                }
                let Some(tag) = cur.clone() else { continue };
                let page_no = cs.last().and_then(|c| c.trim().parse::<u32>().ok()).filter(|n| *n > 0 && *n < 100_000);
                let item = sub.items.entry(tag).or_default();
                let body: Vec<String> = if page_no.is_some() { cs.iter().take(cs.len().saturating_sub(1)).cloned().collect() } else { cs.clone() };
                // A description's second line (and the page) under one with none.
                let continues = body.len() == 1 && item.products.last().is_some_and(|l| l.page.is_none());
                if continues {
                    if let Some(last) = item.products.last_mut() {
                        last.description = clean(&format!("{} {}", last.description, body.join(" ")));
                        last.page = page_no;
                    }
                } else if body.len() >= 2 && item.products.len() < MAX_PRODUCTS {
                    let p = SubProduct {
                        vendor: clean(body.first().map_or("", String::as_str)),
                        description: clean(&body.get(1..).map(|d| d.join(" ")).unwrap_or_default()),
                        page: page_no,
                    };
                    if !item.products.iter().any(|q| q.vendor == p.vendor && q.description == p.description) {
                        item.products.push(p);
                    }
                }
            }
        } else if let Some(t) = page_tag {
            // A cut sheet of the item.
            sheets.entry(t).or_default().push((printed, lines.join("\n")));
        }
    }
    // A letter returning the items it doesn't list with no exception taken: those are approved.
    let unlisted_ok = sub.reviewed
        && text.lines().any(|l| {
            let u = l.to_ascii_uppercase();
            r.review.unlisted.iter().any(|w| u.contains(&w.to_ascii_uppercase())) && status_of(r, l) == Some(Status::Approved) && u.contains("NOT")
        });
    if unlisted_ok {
        for it in sub.items.values_mut().filter(|it| it.status == Status::Unknown) {
            it.status = Status::Approved;
            it.response = "not listed in the review letter, which returns unlisted items with no exception taken".into();
        }
    }
    sub.items.retain(|_, it| !it.products.is_empty() || it.status != Status::Unknown);
    if sub.items.is_empty() {
        return None;
    }
    // Each item's first product: its pages (from its page to the next product's), the catalog
    // number they print under a label (else its description when that is one), its electrical data.
    for (tag, it) in sub.items.iter_mut() {
        let Some(first) = it.products.first() else { continue };
        let from = first.page;
        let to = it.products.iter().skip(1).filter_map(|p| p.page).filter(|p| from.is_none_or(|f| *p > f)).min();
        let mine: Vec<&(Option<u32>, String)> = sheets
            .get(tag)
            .into_iter()
            .flatten()
            .filter(|(pg, _)| match (pg, from) {
                (Some(pg), Some(f)) => *pg >= f && to.is_none_or(|t| *pg < t),
                _ => true,
            })
            .collect();
        // Every value printed under a catalog label on its pages, in order: (value, page, whether
        // the label is a strong one, `Model:` or `Catalog No.`, not a bare `Model`).
        let mut labels: Vec<(String, bool)> = r.catalog_labels.iter().map(|l| (l.to_ascii_uppercase(), true)).collect();
        labels.extend(r.weak_catalog_labels.iter().map(|l| (l.to_ascii_uppercase(), false)));
        labels.sort_by_key(|l| std::cmp::Reverse(l.0.len()));
        let mut found: Vec<(String, Option<u32>, bool)> = Vec::new();
        for (pg, text) in &mine {
            for line in text.lines().take(4_000) {
                let cs = cells(line);
                for (ci, c) in cs.iter().enumerate() {
                    let up = c.to_ascii_uppercase();
                    // The longest label in the cell at a word's start.
                    let hit = labels.iter().find_map(|(l, strong)| {
                        let at = up.find(l.as_str())?;
                        let before = up.get(..at).and_then(|b| b.chars().last());
                        before.is_none_or(|ch| !ch.is_ascii_alphanumeric()).then_some((at + l.len(), *strong))
                    });
                    let Some((end, strong)) = hit else { continue };
                    let rest = c.get(end..).unwrap_or_default().trim().trim_start_matches([':', '#', '.']).trim();
                    let value = if rest.is_empty() { cs.get(ci + 1).cloned().unwrap_or_default() } else { rest.to_string() };
                    if let Some(value) = catalog_at(&value)
                        && found.len() < 40
                        && !found.iter().any(|f| catalog_key(&f.0) == catalog_key(&value))
                    {
                        found.push((clean(&value), *pg, strong));
                    }
                }
            }
        }
        // The submitter's own model (an item listed by its catalog number), else the cut sheet's.
        it.catalog = if first.description.split_whitespace().count() == 1 && catalog_like(&first.description) {
            Some((first.description.clone(), first.page))
        } else {
            found.iter().find(|f| f.2).or_else(|| (found.len() == 1).then(|| found.first()).flatten()).map(|f| (f.0.clone(), f.1))
        };
        it.catalogs = found;
        let joined: String = mine.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n");
        let e = super::equipsub::text_electrical(&joined);
        if super::equipsub::has_data(&e) || e.volts.is_some() {
            it.electrical = Some((e, mine.first().and_then(|(p, _)| *p)));
        }
        it.sheet_text = joined.chars().take(200_000).collect();
        it.sheet_key = catalog_key_long(&it.sheet_text);
    }
    Some(sub)
}

/// The catalog number a submittal item gives: the schedule's model where the same maker's cut
/// sheet prints it or its family (the cut sheet's `Q200` for the schedule's order code `QN200XB`),
/// else the value under its cut sheet's catalog label; with the document it is from.
fn submitted_catalog(sb: &Submittal, it: &SubItem, sched_model: Option<&str>, same: bool) -> Option<(String, String)> {
    let printed = |m: &str| {
        let k = catalog_key(m);
        k.len() >= 3 && (it.catalogs.iter().any(|c| catalog_key(&c.0) == k) || it.sheet_key.contains(&k))
    };
    let page = it.products.first().and_then(|p| p.page);
    sched_model
        .filter(|_| same)
        .filter(|m| m.split(['/', ',']).map(str::trim).any(|x| printed(x) || it.catalog.as_ref().is_some_and(|(c, _)| same_catalog(x, c))))
        .map(|m| (m.to_string(), format!("{} (the schedule's model, on its cut sheet)", sb.doc(page))))
        .or_else(|| it.catalog.as_ref().map(|(c, pg)| (c.clone(), sb.doc(*pg))))
}

/// The submittal item kept for a tag: one not sent back before one sent back, the later revision,
/// a reviewed copy before an unreviewed one, the one with more products.
fn chosen<'a>(subs: &'a [Submittal], tag: &str) -> Option<(&'a Submittal, &'a SubItem)> {
    let k = tag_key(tag);
    subs.iter().filter_map(|s| s.items.get(&k).map(|it| (s, it))).filter(|(_, it)| !it.products.is_empty() || it.status != Status::Unknown).max_by(
        |(a, x), (b, y)| {
            (x.status != Status::SentBack)
                .cmp(&(y.status != Status::SentBack))
                .then(a.revision.cmp(&b.revision))
                .then(a.reviewed.cmp(&b.reviewed))
                .then(x.products.len().cmp(&y.products.len()))
        },
    )
}

// ---------------------------------------------------------------- the rough-in table

/// A name's words (letters and digits).
fn words(s: &str) -> BTreeSet<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| w.len() >= 2).map(str::to_ascii_uppercase).collect()
}

/// The rough-in table's row for a fixture: the one sharing most words with its name (and its
/// description), a row for a narrower use (rules data `narrowing_words`) only when the fixture says
/// it; a tie the name alone can't settle is no row. The row and the other rows that tied.
fn connection_row<'a>(r: &Rules, rows: &'a [ConnectionRow], fixture: &str, description: &str) -> Option<(&'a ConnectionRow, Vec<String>)> {
    let name = words(fixture);
    let all: BTreeSet<String> = name.union(&words(description)).cloned().collect();
    let narrow: BTreeSet<String> = r.narrowing_words.iter().map(|w| w.to_ascii_uppercase()).collect();
    let scored: Vec<(usize, usize, &ConnectionRow)> = rows
        .iter()
        .filter(|row| words(&row.fixture).iter().all(|w| !narrow.contains(w) || all.contains(w)))
        .map(|row| {
            let rw = words(&row.fixture);
            (rw.intersection(&name).count(), rw.intersection(&all).count(), row)
        })
        .filter(|(n, _, _)| *n > 0)
        .collect();
    let best = scored.iter().map(|(n, d, _)| (*n, *d)).max()?;
    let top: Vec<&ConnectionRow> = scored.iter().filter(|(n, d, _)| (*n, *d) == best).map(|(_, _, row)| *row).collect();
    match top.as_slice() {
        [one] => Some((*one, Vec::new())),
        // Two rows sharing only one word (`SINK`) can't be told apart by it.
        [one, rest @ ..] if best.0 >= 2 => Some((*one, rest.iter().map(|x| x.fixture.clone()).collect())),
        _ => None,
    }
}

// ---------------------------------------------------------------- applying

/// What the documents give, read once.
#[derive(Clone, Debug, Default)]
pub(crate) struct Docs {
    pub rows: Vec<ScheduledFixture>,
    pub connections: Vec<ConnectionRow>,
    /// The sheet the schedule is on.
    pub sheet: String,
    /// The schedule's column headers as read.
    pub headers: Vec<String>,
    pub submittals: Vec<Submittal>,
    /// Submittal files read that list none of the schedule's tags, and those not read.
    pub skipped: usize,
    pub unread: Vec<Value>,
}

/// The schedule's tags (each of a list tag's too), compared without case or spaces.
fn known_tags(rows: &[ScheduledFixture]) -> BTreeSet<String> {
    rows.iter().flat_map(|r| fs::tags_of(&r.tag)).map(|t| tag_key(&t)).collect()
}

/// The schedule row of a tag (a list tag's row for each of its tags).
fn row_of<'a>(rows: &'a [ScheduledFixture], tag: &str) -> Option<&'a ScheduledFixture> {
    let k = tag_key(tag);
    rows.iter().find(|r| tag_key(&r.tag) == k).or_else(|| rows.iter().find(|r| fs::tags_of(&r.tag).iter().any(|t| tag_key(t) == k)))
}

/// The plumbing submittals among `files`: files named for plumbing (rules data) read whole, the
/// others sniffed on their first pages for the schedule's tags.
pub(crate) fn read_submittals(r: &Rules, files: &[String], rows: &[ScheduledFixture], docs: &mut Docs) {
    let known = known_tags(rows);
    if known.is_empty() {
        return;
    }
    let named = |f: &str| {
        let u = f.to_ascii_uppercase();
        r.submittal_name_words.iter().any(|w| u.contains(&w.to_ascii_uppercase()))
    };
    for f in files.iter().take(MAX_FILES) {
        let name = std::path::Path::new(f).file_stem().map_or_else(|| f.clone(), |n| n.to_string_lossy().to_string());
        let Ok(bytes) = crate::readfile::read(f) else {
            docs.unread.push(json!({"file": name, "error": "could not be read"}));
            continue;
        };
        if !named(f) {
            // Sniff its first pages: a product list or review naming two of the tags.
            let Ok(head) = super::spec_import::pdf_text_pages(&bytes, f, r.sniff_pages.max(1)) else { continue };
            let hits = head.lines().flat_map(cells).filter(|c| known.contains(&tag_key(c))).collect::<BTreeSet<_>>().len();
            if hits < 2 {
                docs.skipped += 1;
                continue;
            }
        }
        match super::spec_import::pdf_text_pages(&bytes, f, MAX_PAGES) {
            Ok(text) => match read_submittal(r, &text, &name, f, &known) {
                Some(s) => docs.submittals.push(s),
                None => docs.skipped += 1,
            },
            Err(e) => docs.unread.push(json!({"file": name, "error": e.to_string()})),
        }
    }
}

/// A conflict between documents, as the lighting step raises them (a review item of kind
/// `conflict`, trade plumbing).
fn conflict(topic: &str, about: &str, issue: String, fix: String, sides: &[(String, String)], elements: &[Handle]) -> super::lighting::Conflict {
    let mut c = super::lighting::Conflict::new(topic, about.to_string(), issue, fix).with_elements(elements.iter().map(|h| h.hex()));
    for (d, s) in sides {
        c = c.side(d.clone(), s.clone());
    }
    c
}

/// One fixture element of the model: its handle, tag, level, middle (spec units, on its level's
/// plan) and its props.
struct Fixture {
    handle: Handle,
    tag: String,
    level: String,
    at: [f64; 2],
    store: Props,
}

/// The model's plumbing fixtures and plumbing equipment: built elements of kind
/// `plumbing_fixture`, and any other built element whose type mark is a schedule tag.
fn fixtures(d: &Drawing, rows: &[ScheduledFixture]) -> Vec<Fixture> {
    let known = known_tags(rows);
    let mut out = Vec::new();
    for e in d.model.iter().filter(|e| super::bimlook::made_tags(e).is_some()) {
        let store = props::read(&e.common.xdata);
        let text = |k: &str| store.get(k).filter(|p| p.has_value()).map(Prop::text).unwrap_or_default();
        let tag = text("type_mark");
        let plumbing = text("kind") == "plumbing_fixture";
        if !(plumbing || (!tag.is_empty() && known.contains(&tag_key(&tag)))) {
            continue;
        }
        let at = super::propfill::extent(d, e).map(|(b, ..)| b.center() - super::bspec::stacked_by(e)).map_or([0.0, 0.0], |c| [c.x, c.y]);
        out.push(Fixture { handle: e.handle, tag, level: super::bspec::made_level(e).unwrap_or_default(), at, store });
        if out.len() >= MAX_ELEMENTS {
            break;
        }
    }
    out
}

/// A property set unless a person's value is there.
fn put(store: &mut Props, key: &str, p: Prop) {
    if store.get(key).is_some_and(|old| old.source.kind == SourceKind::User) {
        return;
    }
    store.set(key, p);
}

/// The panel and circuit a panel schedule names a tag on (its description holds the tag).
fn circuit_of(sp: &cadaddy_walk3d::BuildingSpec, tag: &str) -> Option<(String, String)> {
    let mine: BTreeSet<String> = fs::tags_of(tag).iter().map(|t| tag_key(t)).collect();
    for p in sp.mep.panel_schedules.iter().take(2_000) {
        for c in p.circuits.iter().take(400) {
            let up = c.description.to_ascii_uppercase();
            if up.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != ',').any(|w| mine.contains(&tag_key(w))) {
                return Some((p.panel.clone(), c.number.clone()));
            }
        }
    }
    None
}

/// The electrical words of a submittal's data (`120 V 1 ph, 5.8 FLA`).
fn electrical_words(e: &cadaddy_walk3d::EquipmentElectrical) -> String {
    let mut w: Vec<String> = Vec::new();
    if let Some(v) = e.volts {
        w.push(format!("{v} V"));
    }
    if let Some(p) = e.phase {
        w.push(format!("{p} ph"));
    }
    if let Some(a) = e.fla.or(e.nameplate_a) {
        w.push(format!("{a} A"));
    }
    if let Some(a) = e.mca {
        w.push(format!("MCA {a} A"));
    }
    if let Some(h) = e.hp {
        w.push(format!("{h} HP"));
    }
    if let Some(k) = e.kw {
        w.push(format!("{k} kW"));
    }
    w.join(", ")
}

/// Apply the documents' products to the model's plumbing fixtures (`reqs`: the specification's
/// requirements `bim.products` read). The report.
pub(crate) fn apply(s: &mut Session, docs: &Docs, reqs: &[Requirement]) -> crate::Result<Value> {
    let r = rules();
    if docs.rows.is_empty() {
        return Ok(json!({"schedule": Value::Null, "why": "no plumbing fixture schedule was read"}));
    }
    let schedule_src = |what: &str| Source::from_doc(SourceKind::Schedule, &format!("plumbing fixture schedule {}{what}", docs.sheet));
    let found = fixtures(s.doc()?, &docs.rows);
    // The levels whose water mains are proposed (copied from another level, not drawn).
    let water_proposed_levels: BTreeSet<String> = {
        let d = s.doc()?;
        let water: BTreeSet<String> = r.water_systems.iter().map(|x| x.to_ascii_uppercase()).collect();
        d.model
            .iter()
            .filter(|e| super::bimlook::made_tag(e, "kind") == Some("pipe"))
            .filter(|e| super::bimlook::made_tag(e, "status").is_some_and(|v| v.eq_ignore_ascii_case(super::mep::PROPOSED)))
            .filter(|e| super::bimlook::made_tag(e, "system").is_some_and(|v| water.contains(&v.to_ascii_uppercase())))
            .filter_map(|e| super::bspec::made_level(e))
            .take(MAX_ELEMENTS)
            .collect()
    };
    let mut changes: Vec<(Handle, Props)> = Vec::new();
    let mut by_tag: BTreeMap<String, Vec<Handle>> = BTreeMap::new();
    let mut maker_from: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut catalog_from: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut rough_from: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut no_data: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    let mut with_product = 0usize;
    let mut water_derived = 0usize;
    let mut powered: Vec<(Handle, String, String, [f64; 2], String, Option<(String, String)>, &'static str)> = Vec::new();
    for f in &found {
        let mut st = f.store.clone();
        let before = st.clone();
        let row = (!f.tag.is_empty()).then(|| row_of(&docs.rows, &f.tag)).flatten();
        let sub = (!f.tag.is_empty()).then(|| chosen(&docs.submittals, &f.tag)).flatten();
        let approved = sub.filter(|(_, it)| it.status != Status::SentBack);
        let name = row.map(|x| format!("{} {}", x.fixture, x.description)).unwrap_or_else(|| {
            ["fixture", "type", "device_type", "description"].iter().filter_map(|k| st.get(k).map(Prop::text)).collect::<Vec<_>>().join(" ")
        });
        let kind = kind_by(&r, &row.map(|x| x.fixture.clone()).unwrap_or_default()).or_else(|| kind_by(&r, &name));
        let req = kind.as_deref().and_then(|k| reqs.iter().find(|q| q.kind == k));
        // The schedule's row.
        if let Some(x) = row {
            put(&mut st, "schedule_row", Prop::new(clean(&format!("{} {}", x.tag, x.fixture)), schedule_src("")));
            put(&mut st, "fixture", Prop::new(clean(&x.fixture), schedule_src("")));
            if !x.description.is_empty() {
                put(&mut st, "description", Prop::new(clean(&x.description), schedule_src("")));
            }
            if !x.remarks.is_empty() {
                put(&mut st, "remarks", Prop::new(clean(&x.remarks), schedule_src("")));
            }
            if !x.parts.is_empty() {
                let parts: Vec<String> = x
                    .parts
                    .iter()
                    .map(|p| {
                        clean(&format!("{}{} {}", if p.part.is_empty() { String::new() } else { format!("{}: ", p.part) }, p.manufacturer, p.model))
                    })
                    .collect();
                put(&mut st, "parts", Prop::new(parts.join("; "), schedule_src("")));
            }
        }
        if let Some(k) = &kind {
            put(&mut st, "product_kind", Prop::new(k.clone(), Source::from_doc(SourceKind::Rule, "plumbproducts.json kinds")));
        }
        // The maker and catalog number: the approved submittal's, else the schedule's, else the
        // specification's one maker.
        let sched_maker = row.map(|x| x.manufacturer.clone()).filter(|m| !m.is_empty());
        let sched_model = row.map(|x| x.model.clone()).filter(|m| !m.is_empty());
        let sub_first = approved.and_then(|(_, it)| it.products.first());
        let sub_maker = sub_first.map(|p| p.vendor.clone()).filter(|m| !m.is_empty());
        let mut maker: Option<(String, Source, &'static str)> = None;
        if let (Some(m), Some((sb, _))) = (&sub_maker, approved) {
            let mut src = Source::from_doc(SourceKind::Submittal, &sb.doc(sub_first.and_then(|p| p.page)));
            src.at = None;
            maker = Some((m.clone(), src, "submittal"));
        } else if let Some(m) = &sched_maker {
            maker = Some((m.clone(), schedule_src(""), "schedule"));
        } else if let Some(q) = req.filter(|q| q.makers.len() == 1) {
            let m = q.makers.first().cloned().unwrap_or_default();
            maker = Some((m, Source::from_doc(SourceKind::Spec, &format!("{} p.{} ({})", q.section, q.page, q.product)), "spec"));
        }
        let same = match (&sub_maker, &sched_maker) {
            (Some(a), Some(b)) => same_maker(a, b),
            _ => true,
        };
        // The cut sheet's catalog number: the schedule's model where the same maker's cut sheet
        // prints it, else the value under its catalog label.
        let sub_catalog = approved.and_then(|(sb, it)| submitted_catalog(sb, it, sched_model.as_deref(), same));
        let mut catalog: Option<(String, Source, &'static str)> = None;
        if let Some((c, doc)) = &sub_catalog {
            catalog = Some((c.clone(), Source::from_doc(SourceKind::Submittal, doc), "submittal"));
        } else if let Some(m) = sched_model.as_ref().filter(|_| same) {
            catalog = Some((m.clone(), schedule_src(""), "schedule"));
        }
        if let Some((m, src, from)) = &maker {
            let mut p = Prop::new(clean(m), src.clone());
            if *from == "submittal"
                && let Some(sm) = sched_maker.as_ref().filter(|sm| !same_maker(sm, m))
            {
                p = p.with_note(&format!("the approved submittal's; the schedule gives {sm} (kept as design_manufacturer)"));
                put(&mut st, "design_manufacturer", Prop::new(clean(sm), schedule_src("")));
            }
            put(&mut st, "manufacturer", p);
            *maker_from.entry(from).or_default() += 1;
        }
        if let Some((c, src, from)) = &catalog {
            let mut p = Prop::new(clean(c), src.clone());
            if *from == "submittal"
                && let Some(sc) = sched_model.as_ref().filter(|sc| !same_catalog(sc, c))
            {
                p = p.with_note(&format!("the approved submittal's cut sheet; the schedule gives {sc} (kept as design_catalog)"));
                put(&mut st, "design_catalog", Prop::new(clean(sc), schedule_src("")));
            }
            put(&mut st, "catalog_number", p);
            *catalog_from.entry(from).or_default() += 1;
        } else if let Some(sc) = &sched_model {
            // The schedule's model is another maker's than the one bought: kept beside it only.
            put(
                &mut st,
                "design_catalog",
                Prop::new(clean(sc), schedule_src(""))
                    .with_note("the schedule's model; the approved submittal is another maker's and its cut sheet prints no catalog number"),
            );
        }
        if let Some((sb, it)) = sub {
            let first = it.products.first();
            let what = first.map(|p| clean(&format!("{} {}", p.vendor, p.description))).unwrap_or_default();
            let mut text = format!("{}: {}", sb.doc(first.and_then(|p| p.page)), it.status.words());
            if !it.response.is_empty() {
                text.push_str(&format!(" ({})", it.response));
            }
            if !what.is_empty() {
                text.push_str(&format!("; {what}"));
            }
            put(&mut st, "submittal", Prop::new(clean(&text), Source::from_doc(SourceKind::Submittal, &sb.doc(None))));
            put(&mut st, "submittal_status", Prop::new(it.status.words(), Source::from_doc(SourceKind::Submittal, &sb.doc(None))));
            if approved.is_some() && it.products.len() > 1 {
                let acc: Vec<String> = it.products.iter().skip(1).map(|p| clean(&format!("{} {}", p.vendor, p.description))).collect();
                put(&mut st, "accessories", Prop::new(clean(&acc.join("; ")), Source::from_doc(SourceKind::Submittal, &sb.doc(None))));
            }
        }
        if let Some(q) = req.filter(|q| !q.makers.is_empty()) {
            put(
                &mut st,
                "approved_makers",
                Prop::new(
                    clean(&format!("{} ({})", q.makers.join("; "), q.substitution)),
                    Source::from_doc(SourceKind::Spec, &format!("{} p.{} ({})", q.section, q.page, q.product)),
                ),
            );
        }
        // The connection sizes: the schedule's own columns, else the rough-in table's row.
        let mut sizes: Vec<(String, String, Source, Option<String>)> = Vec::new();
        if let Some(x) = row {
            for (k, v) in &x.connections {
                sizes.push((k.clone(), v.clone(), schedule_src(""), None));
            }
            if sizes.is_empty()
                && let Some((cr, others)) = connection_row(&r, &docs.connections, &x.fixture, &x.description)
            {
                let src = schedule_src(&format!(" rough-in table: {}", cr.fixture));
                let note =
                    (!others.is_empty()).then(|| format!("the rough-in table's row for its name; its name also matches {}", others.join(", ")));
                for (k, v) in &cr.sizes {
                    sizes.push((k.clone(), v.clone(), src.clone(), note.clone()));
                }
            }
        }
        if !sizes.is_empty() {
            *rough_from.entry(if row.is_some_and(|x| !x.connections.is_empty()) { "schedule" } else { "rough_in_table" }).or_default() += 1;
        }
        // Its water: derived where it is on a proposed water line, or its level's water mains are
        // proposed (the level has no water plan of its own: its branches aren't design-issued).
        let proposed_water = water_proposed_levels.contains(&f.level)
            || ["cold_water", "hot_water"]
                .iter()
                .any(|k| st.get(k).and_then(|p| p.note.as_deref()).is_some_and(|n| n.to_ascii_lowercase().contains("proposed")));
        for (k, v, src, note) in &sizes {
            let key = format!("{k}_size");
            let mut p = Prop::new(clean(v), src.clone()).with_unit("in");
            if let Some(n) = note {
                p = p.with_note(n);
            }
            if proposed_water && (k == "cw" || k == "hw") {
                p = p.with_note(&format!("derived: {}", r.water_derived));
            }
            put(&mut st, &key, p);
        }
        if proposed_water {
            put(
                &mut st,
                "water_provenance",
                Prop::new("derived", Source::from_doc(SourceKind::Rule, "the level's proposed water mains")).with_note(&r.water_derived),
            );
            water_derived += 1;
        } else if st.get("cold_water").is_some() || st.get("hot_water").is_some() {
            put(&mut st, "water_provenance", Prop::new("drawn", Source::computed()).with_note("its water lines are on its level's own water plan"));
        }
        // Powered: its documents' electrical data (the schedule's columns, the cut sheet's amps),
        // else the rules data: a word of its documents (sensor, line voltage, a power burner: its
        // power is derived from them), else its kind alone (a pump: inferred).
        let sub_elec = approved.and_then(|(sb, it)| it.electrical.as_ref().map(|(e, pg)| (e.clone(), sb.doc(*pg))));
        let sub_full = sub_elec.as_ref().filter(|(e, _)| super::equipsub::has_data(e));
        let doc_text = format!(
            "{} {} {}",
            name,
            row.map(|x| x.remarks.as_str()).unwrap_or_default(),
            approved.map(|(_, it)| it.sheet_text.as_str()).unwrap_or_default()
        )
        .to_ascii_uppercase();
        let sched_elec = row.filter(|x| !x.electrical.is_empty());
        let rule = r.powered.iter().find_map(|p| {
            let kind_ok = p.kinds.is_empty() || kind.as_ref().is_some_and(|k| p.kinds.iter().any(|x| x == k));
            let word = p.words.iter().find(|w| doc_text.contains(&w.to_ascii_uppercase()));
            let word_ok = p.words.is_empty() || word.is_some();
            (kind_ok && word_ok && !(p.kinds.is_empty() && p.words.is_empty())).then_some((p, word))
        });
        // A cut sheet's voltage alone counts only as a supply voltage (not a rating's 250 V).
        let supply_volts =
            sub_elec.as_ref().and_then(|(e, doc)| e.volts.filter(|v| SUPPLY_VOLTS.iter().any(|x| (x - v).abs() < 0.5)).map(|v| (v, doc.clone())));
        let elec: Option<(String, Source, &'static str)> = if let Some(x) = sched_elec {
            Some((x.electrical.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "), schedule_src(""), "documented"))
        } else if let Some((e, doc)) = sub_full.filter(|(e, _)| !electrical_words(e).is_empty()) {
            Some((electrical_words(e), Source::from_doc(SourceKind::Submittal, doc), "documented"))
        } else {
            rule.map(|(p, word)| {
                let volts = supply_volts.as_ref().map(|(v, doc)| format!(", {v} V ({doc})")).unwrap_or_default();
                match word {
                    Some(w) => (
                        format!("equipment connection{volts}"),
                        Source::from_doc(
                            SourceKind::Rule,
                            &format!("plumbproducts.json powered: {}; its documents say \"{}\"", p.basis, w.to_ascii_lowercase()),
                        ),
                        "derived",
                    ),
                    None => (
                        format!("equipment connection{volts}"),
                        Source::from_doc(SourceKind::Rule, &format!("plumbproducts.json powered: {}", p.basis)),
                        "inferred",
                    ),
                }
            })
        };
        if let Some((w, src, how)) = &elec {
            let mut p = Prop::new(clean(w), src.clone());
            if *how == "inferred" {
                p = p.with_note("inferred: its documents give no electrical data; the rule's basis says why it takes power");
            } else if *how == "derived" {
                p = p.with_note("derived: its documents say it is powered but give no amps; the rule's basis names the connection");
            }
            put(&mut st, "electrical_connection", p);
            put(&mut st, "electrical_provenance", Prop::new(*how, src.clone()));
            if let Some((e, doc)) = sub_full {
                let s2 = Source::from_doc(SourceKind::Submittal, doc);
                if let Some(v) = e.volts {
                    put(&mut st, "volts", Prop::new(v, s2.clone()).with_unit("V"));
                }
                if let Some(ph) = e.phase {
                    put(&mut st, "phase", Prop::new(f64::from(ph), s2.clone()));
                }
                if let Some(a) = e.fla.or(e.nameplate_a).or(e.mca) {
                    put(&mut st, "amps", Prop::new(a, s2.clone()).with_unit("A"));
                }
                if let Some(h) = e.hp {
                    put(&mut st, "hp", Prop::new(h, s2.clone()));
                }
            } else if let Some((v, doc)) = &supply_volts {
                put(&mut st, "volts", Prop::new(*v, Source::from_doc(SourceKind::Submittal, doc)).with_unit("V"));
            }
            let basis = match (how, rule) {
                (&"inferred" | &"derived", Some((p, _))) => p.basis.clone(),
                _ => format!("its documents ({}) give it an electrical connection", src.doc.clone().unwrap_or_default()),
            };
            powered.push((f.handle, f.tag.clone(), f.level.clone(), f.at, basis, None, how));
        }
        // What it has, or why it has nothing.
        let has = maker.is_some() || catalog.is_some();
        if has {
            with_product += 1;
            st.remove("product_gap");
        } else {
            let why = if f.tag.is_empty() {
                "no tag: the plans give it no type mark, so no schedule row, submittal item or specification product can be found for it".to_string()
            } else if row.is_none() {
                format!(
                    "its tag {} is on no plumbing fixture schedule row, no plumbing submittal lists it, and no Division 22 section names one maker for its kind",
                    f.tag
                )
            } else {
                format!(
                    "its schedule row ({}) gives no maker or model, no approved plumbing submittal lists it, and no Division 22 section names one maker for its kind",
                    f.tag
                )
            };
            put(&mut st, "product_gap", Prop::new(why.clone(), Source::computed()));
            let key = if f.tag.is_empty() { "(no tag)".to_string() } else { f.tag.clone() };
            let e = no_data.entry(key).or_insert_with(|| (0, vec![why]));
            e.0 += 1;
        }
        if !f.tag.is_empty() {
            by_tag.entry(tag_key(&f.tag)).or_default().push(f.handle);
        }
        if st != before {
            changes.push((f.handle, st));
        }
    }
    let d = s.doc_mut()?;
    let mut changed = 0usize;
    for (h, st) in changes {
        if d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &st).into()).is_ok() {
            changed += 1;
        }
    }
    // The type catalog: each tag's product as the documents give it (for Validate's checks).
    let mut typed = 0usize;
    for tag in by_tag.keys() {
        let Some(row) = row_of(&docs.rows, tag) else { continue };
        let mut ty = d.bim_types.get(tag).cloned().unwrap_or_else(|| cadaddy_doc::bimspec::BimType::new(tag.clone()));
        if ty.category.is_empty() {
            ty.category = "plumbing_fixture".into();
        }
        let field = |v: &str, src: String| cadaddy_doc::bimspec::SpecField::new(clean(v)).with_source(src);
        let sched = format!("plumbing fixture schedule {}", docs.sheet);
        ty.set_field("fixture", field(&row.fixture, sched.clone()));
        match chosen(&docs.submittals, tag).filter(|(_, it)| it.status != Status::SentBack) {
            Some((sb, it)) => {
                if let Some(p) = it.products.first() {
                    ty.set_field("manufacturer", field(&p.vendor, sb.doc(p.page)));
                    ty.set_field("description", field(&p.description, sb.doc(p.page)));
                }
                let same = it.products.first().is_none_or(|p| row.manufacturer.is_empty() || same_maker(&row.manufacturer, &p.vendor));
                let model = (!row.model.is_empty()).then_some(row.model.as_str());
                if let Some((c, doc)) = submitted_catalog(sb, it, model, same) {
                    ty.set_field("catalogNumber", field(&c, doc));
                }
                ty.set_field("reviewStatus", field(it.status.words(), sb.doc(None)));
            }
            None => {
                if !row.manufacturer.is_empty() {
                    ty.set_field("manufacturer", field(&row.manufacturer, sched.clone()));
                }
                if !row.model.is_empty() {
                    ty.set_field("catalogNumber", field(&row.model, sched.clone()));
                }
                if !row.description.is_empty() {
                    ty.set_field("description", field(&row.description, sched.clone()));
                }
            }
        }
        d.bim_types.set(ty);
        typed += 1;
    }
    // The documents against each other, per tag: one conflict each, the submittal kept.
    let mut conflicts: Vec<super::lighting::Conflict> = Vec::new();
    for (tag, handles) in &by_tag {
        if conflicts.len() >= MAX_CONFLICTS {
            break;
        }
        let row = row_of(&docs.rows, tag);
        let sched = format!("plumbing fixture schedule {}", docs.sheet);
        let about = format!("plumbing fixture {tag}");
        match chosen(&docs.submittals, tag) {
            Some((sb, it)) if it.status == Status::SentBack => conflicts.push(conflict(
                "submittal sent back",
                &about,
                format!("{tag}'s submittal was sent back ({}); the model keeps the schedule's product until it is resubmitted", it.response),
                format!("Resubmit {tag}, or confirm the schedule's product"),
                &[(sb.doc(None), it.response.clone())],
                handles,
            )),
            Some((sb, it)) => {
                let p = it.products.first();
                if let (Some(row), Some(p)) = (row, p)
                    && !row.manufacturer.is_empty()
                    && !same_maker(&row.manufacturer, &p.vendor)
                {
                    conflicts.push(conflict(
                        "maker",
                        &about,
                        format!("{tag}: the schedule gives {} {}, the approved submittal {} {}; the model takes the submittal's (what was bought), the schedule's is kept beside it", row.manufacturer, row.model, p.vendor, p.description),
                        format!("Confirm {tag}'s product: revise the schedule to the submitted one, or resubmit"),
                        &[(sched.clone(), clean(&format!("{} {}", row.manufacturer, row.model))), (sb.doc(p.page), clean(&format!("{} {}", p.vendor, p.description)))],
                        handles,
                    ));
                } else if let (Some(row), Some((c, pg))) = (row, &it.catalog)
                    && !row.model.is_empty()
                    && !same_catalog(&row.model, c)
                    && !row.model.split(['/', ',']).any(|m| !m.trim().is_empty() && same_catalog(m, c))
                {
                    conflicts.push(conflict(
                        "catalog number",
                        &about,
                        format!("{tag}: the schedule's model {} isn't the submittal cut sheet's {c}; the model takes the submittal's", row.model),
                        format!("Confirm {tag}'s model"),
                        &[(sched.clone(), row.model.clone()), (sb.doc(*pg), c.clone())],
                        handles,
                    ));
                }
            }
            None => {}
        }
    }
    // The maker kept against the specification's approved makers for its kind.
    for f in &found {
        if conflicts.len() >= MAX_CONFLICTS {
            break;
        }
        let Some(st) = d.model.get(f.handle).map(|e| props::read(&e.common.xdata)) else { continue };
        let (Some(m), Some(k)) = (st.get("manufacturer").map(Prop::text), st.get("product_kind").map(Prop::text)) else { continue };
        let Some(q) = reqs.iter().find(|q| q.kind == k && !q.makers.is_empty()) else { continue };
        if q.makers.iter().any(|x| same_maker(x, &m)) || q.substitution.contains("or equal") {
            continue;
        }
        let about = format!("plumbing fixture {}", if f.tag.is_empty() { k.clone() } else { f.tag.clone() });
        if conflicts.iter().any(|c| c.about == about && c.topic == "approved makers") {
            continue;
        }
        let handles = by_tag.get(&tag_key(&f.tag)).cloned().unwrap_or_else(|| vec![f.handle]);
        let spec = format!("spec {} p.{} ({})", q.section, q.page, q.product);
        conflicts.push(conflict(
            "approved makers",
            &about,
            format!("{about}: its maker {m} is not among the specification's approved makers ({})", q.makers.join("; ")),
            "Confirm the maker with the design team (a substitution request), or supply a listed maker".to_string(),
            &[(spec, q.makers.join("; ")), (st.get("manufacturer").and_then(|p| p.source.doc.clone()).unwrap_or_default(), m.clone())],
            &handles,
        ));
    }
    // The electrical connections: a box beside each powered fixture in the spec (the electrical
    // takeoff counts it), its panel and circuit where a panel schedule names its tag.
    let mut boxes = 0usize;
    let mut by_basis: BTreeMap<String, usize> = BTreeMap::new();
    if let Ok((mut sp, true)) = super::bspec::current(s, "bim.products") {
        // This step's own boxes of an earlier run go first.
        sp.mep.equipment.retain(|e| !e.properties.contains_key("PLUMBING_FIXTURE"));
        let pb = super::doc_objects::rules().power_box.clone();
        let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        let used: BTreeSet<String> = sp.mep.equipment.iter().map(|e| e.tag.to_ascii_uppercase()).collect();
        let prefix = pb.kind.to_ascii_uppercase();
        let mut n = 0usize;
        for (h, tag, level, at, basis, _, how) in &powered {
            let btag = loop {
                n += 1;
                let t = format!("{prefix}-{n}");
                if !used.contains(&t) || n > 1_000_000 {
                    break t;
                }
            };
            let pc = if tag.is_empty() { None } else { circuit_of(&sp, tag) };
            let half = pb.box_in.first().copied().unwrap_or(4.0) / 24.0 * upf;
            let p = [at[0] + 1.5 * upf, at[1]];
            let mut props: BTreeMap<String, String> = BTreeMap::new();
            props.insert("DOC_OBJECT".into(), "connection".into());
            props.insert("FOR".into(), if tag.is_empty() { h.hex() } else { tag.clone() });
            props.insert("PLUMBING_FIXTURE".into(), h.hex());
            props.insert("RULE".into(), clean(basis));
            props.insert("PROVENANCE".into(), (*how).to_string());
            props.insert("TRADE".into(), "electrical".into());
            props.insert(
                "SIZE".into(),
                format!(
                    "{}\"x{}\"x{}\"",
                    pb.box_in.first().copied().unwrap_or(4.0),
                    pb.box_in.get(1).copied().unwrap_or(4.0),
                    pb.box_in.get(2).copied().unwrap_or(2.125)
                ),
            );
            match &pc {
                Some((panel, circuit)) => {
                    props.insert("PANEL".into(), panel.clone());
                    props.insert("CIRCUIT".into(), circuit.clone());
                }
                None => {
                    props.insert("CIRCUIT".into(), "not named".into());
                }
            }
            let mut sources = BTreeMap::new();
            sources.insert("position".to_string(), format!("beside {} (rules data)", if tag.is_empty() { h.hex() } else { tag.clone() }));
            sources.insert("why".to_string(), clean(basis));
            let height_in = if pb.height_in > 0.0 { pb.height_in } else { 18.0 };
            sp.mep.equipment.push(cadaddy_walk3d::MepEquipment {
                tag: btag,
                kind: pb.kind.clone(),
                level: level.clone(),
                min: [p[0] - half, p[1] - half],
                max: [p[0] + half, p[1] + half],
                bottom_ft: (height_in / 12.0 - pb.box_in.get(2).copied().unwrap_or(2.125) / 24.0).max(0.0),
                height_ft: (pb.box_in.get(2).copied().unwrap_or(2.125) / 12.0).max(0.05),
                systems: vec!["E-EQUIP".into()],
                title: pb.name.clone(),
                properties: props,
                sources,
                confidence: if *how == "documented" { 0.7 } else { 0.5 },
                ..Default::default()
            });
            boxes += 1;
            *by_basis.entry((*how).to_string()).or_default() += 1;
        }
        let mut ids: Vec<String> = Vec::new();
        for c in &conflicts {
            let rfi = c.to_rfi("plumbing");
            if !sp.rfis.iter().any(|x| x.id == rfi.id) && sp.rfis.len() < cadaddy_walk3d::rfi::MAX_RFIS {
                ids.push(rfi.id.clone());
                sp.rfis.push(rfi);
            }
        }
        s.doc_mut()?.building_spec = Some(sp.to_json());
        by_basis.insert("review_items".into(), ids.len());
    }
    if changed > 0 || typed > 0 {
        s.touch();
    }
    let no_data_json: BTreeMap<&String, Value> =
        no_data.iter().take(MAX_LISTED).map(|(k, (n, why))| (k, json!({"elements": n, "why": why.first()}))).collect();
    Ok(json!({
        "schedule": {"sheet": docs.sheet, "rows": docs.rows.len(), "headers": docs.headers, "rough_in_rows": docs.connections.len()},
        "submittals": docs.submittals.iter().map(|sb| json!({"file": sb.file, "revision": format!("{}-{}", sb.revision.0, sb.revision.1), "reviewed": sb.reviewed, "items": sb.items.len(),
            "approved": sb.items.values().filter(|i| i.status == Status::Approved).count(), "sent_back": sb.items.values().filter(|i| i.status == Status::SentBack).count(),
            "catalogs": sb.items.values().filter(|i| i.catalog.is_some()).count(),
            "by_item": sb.items.iter().take(80).map(|(t, i)| (t.clone(), json!({"status": i.status.words(), "products": i.products.len(),
                "first": i.products.first().map(|p| format!("{} | {} | p.{}", p.vendor, p.description, p.page.map(|n| n.to_string()).unwrap_or_default())),
                "catalog": i.catalog.as_ref().map(|c| c.0.clone()), "labels": i.catalogs.iter().take(6).map(|c| c.0.clone()).collect::<Vec<_>>(),
                "electrical": i.electrical.as_ref().map(|e| electrical_words(&e.0)), "sheet_chars": i.sheet_text.len()}))).collect::<serde_json::Map<String, Value>>()})).collect::<Vec<_>>(),
        "submittal_files_skipped": docs.skipped,
        "unread": docs.unread,
        "fixtures": found.len(),
        "elements": changed,
        "with_product": with_product,
        "no_product_data": no_data.values().map(|(n, _)| *n).sum::<usize>(),
        "no_product_data_by_tag": no_data_json,
        "maker_from": maker_from,
        "catalog_from": catalog_from,
        "rough_in_from": rough_from,
        "water_derived": water_derived,
        "water_proposed_levels": water_proposed_levels,
        "powered": powered.len(),
        "powered_list": powered.iter().take(MAX_LISTED).map(|(h, tag, level, _, basis, _, how)| json!({"handle": h.hex(), "tag": tag, "level": level, "how": how, "basis": clean(basis)})).collect::<Vec<_>>(),
        "elements_by_tag": by_tag.iter().map(|(t, hs)| (t.clone(), hs.len())).collect::<BTreeMap<_, _>>(),
        "connection_boxes": boxes,
        "connections_by": by_basis,
        "conflicts": conflicts.len(),
        "conflicts_by_topic": super::lighting::by_topic(&conflicts),
        "types": typed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, EntityKind, Space};
    use cadaddy_geom::Vec2;

    fn row(tag: &str, fixture: &str, maker: &str, model: &str) -> ScheduledFixture {
        ScheduledFixture { tag: tag.into(), fixture: fixture.into(), manufacturer: maker.into(), model: model.into(), ..ScheduledFixture::default() }
    }

    fn known(tags: &[&str]) -> BTreeSet<String> {
        tags.iter().map(|t| tag_key(t)).collect()
    }

    /// A made-up plumbing submittal as the PDF text reader gives it (cells three spaces apart):
    /// a review letter, a table of contents, cut sheets headed by their item's tag.
    fn package() -> String {
        [
            "--- page 1 of 7 ---",
            "SUBMITTAL REVIEW COMMENTS:",
            "Response Code:",
            "1 – Make Corrections Noted   3 – Rejected",
            "2 – Revise and Resubmit   4 – Not Reviewed",
            "Items not listed below are returned: No Exception Taken.",
            "COMMENT   DESCRIPTION   RESPONSE",
            "1   WC-1   No exceptions taken.",
            "Owner to confirm the alternate",
            "2   1   LV-1   lavatory before ordering. No",
            "exception otherwise.",
            "3   2   SK-1   Provide the scheduled sink.",
            "Page 1 of 2",
            "--- page 2 of 7 ---",
            "Table of Contents",
            "VENDOR   DESCRIPTION   PAGE",
            "WC-1",
            "Maker Nine   Two Piece Toilet   3",
            "Seat Co   Toilet Seat   4",
            "LV-1",
            "Maker Ten   Round Lavatory With Overflow And",
            "Mounting Kit   5",
            "SK-1",
            "Maker Eleven   Stainless Sink   6",
            "RP-1",
            "Pump Works   Circulator Pump   7",
            "Page 2 of 8",
            "--- page 3 of 7 ---",
            "WC-1",
            "Qty: 1",
            "PHYSICAL PROPERTIES",
            "Model: QX-77/A1",
            "Rough-in: 12\"",
            "Page 3 of 8",
            "--- page 4 of 7 ---",
            "WC-1",
            "Qty: 1",
            "Model: ST-55",
            "Page 4 of 8",
            "--- page 5 of 7 ---",
            "LV-1",
            "Qty: 1",
            "Catalog No.   LV-300-0",
            "Page 5 of 8",
            "--- page 6 of 7 ---",
            "SK-1",
            "Qty: 1",
            "Page 6 of 8",
            "--- page 7 of 7 ---",
            "RP-1",
            "Qty: 1",
            "Model Number: CP-15",
            "Motor: 115 V, 1 PH, 1.2 FULL LOAD AMPS",
            "Page 7 of 8",
        ]
        .join("\n")
    }

    #[test]
    fn a_submittal_gives_each_items_products_review_and_catalog_number() {
        let r = rules();
        let sub = read_submittal(
            &r,
            &package(),
            "Plumbing Package",
            "/job/Rev 1-0/Plumbing Package.pdf",
            &known(&["WC-1", "LV-1", "SK-1", "RP-1", "FD-1"]),
        )
        .expect("read");
        assert!(sub.reviewed);
        let wc = sub.items.get("WC-1").expect("WC-1");
        assert_eq!(wc.status, Status::Approved);
        assert_eq!(wc.products.len(), 2, "{:?}", wc.products);
        assert_eq!((wc.products[0].vendor.as_str(), wc.products[0].page), ("Maker Nine", Some(3)));
        // The toilet's own cut sheet (p.3), not its seat's (p.4).
        assert_eq!(wc.catalog, Some(("QX-77/A1".to_string(), Some(3))));
        let lv = sub.items.get("LV-1").expect("LV-1");
        assert_eq!(lv.status, Status::Approved, "code 1, corrections noted: approved as noted");
        assert!(lv.response.contains("Make Corrections Noted") && lv.response.contains("Owner to confirm"), "{}", lv.response);
        assert_eq!(lv.products[0].description, "Round Lavatory With Overflow And Mounting Kit");
        assert_eq!(lv.products[0].page, Some(5));
        assert_eq!(lv.catalog.as_ref().map(|c| c.0.as_str()), Some("LV-300-0"));
        assert_eq!(sub.items.get("SK-1").map(|i| i.status), Some(Status::SentBack), "code 2: revise and resubmit");
        // Not in the letter, which returns unlisted items with no exception taken.
        let rp = sub.items.get("RP-1").expect("RP-1");
        assert_eq!(rp.status, Status::Approved);
        let e = &rp.electrical.as_ref().expect("electrical").0;
        assert_eq!((e.volts, e.phase, e.fla), (Some(115.0), Some(1), Some(1.2)));
        assert!(!sub.items.contains_key("FD-1"), "a tag no page lists");
        // A file listing none of the schedule's tags is no plumbing submittal.
        assert!(read_submittal(&r, &package(), "x", "x.pdf", &known(&["ZZ-9"])).is_none());
        assert_eq!(sub.revision, (1, 0), "its revision from its folder");
    }

    #[test]
    fn the_latest_submittal_not_sent_back_is_kept() {
        let item = |status, maker: &str| SubItem {
            status,
            products: vec![SubProduct { vendor: maker.into(), ..SubProduct::default() }],
            ..SubItem::default()
        };
        let sub = |rev, status, maker: &str| Submittal {
            file: format!("rev {rev}"),
            revision: (rev, 0),
            reviewed: true,
            items: [("WC-9".to_string(), item(status, maker))].into_iter().collect(),
        };
        let subs = vec![sub(1, Status::Approved, "First"), sub(2, Status::SentBack, "Second"), sub(0, Status::Approved, "Zero")];
        assert_eq!(chosen(&subs, "wc-9").map(|(s, _)| s.revision), Some((1, 0)), "rev 2 was sent back: rev 1 stands");
        let subs = vec![sub(1, Status::Approved, "First"), sub(3, Status::Approved, "Third")];
        assert_eq!(chosen(&subs, "WC-9").and_then(|(_, i)| i.products.first()).map(|p| p.vendor.as_str()), Some("Third"));
    }

    #[test]
    fn a_fixture_takes_its_rough_in_row_by_name_the_usual_one_when_two_fit() {
        let r = rules();
        let cr = |f: &str, trap: &str| ConnectionRow { fixture: f.into(), sizes: [("trap".to_string(), trap.to_string())].into_iter().collect() };
        let rows = vec![
            cr("WATER CLOSET (TANK)", "--"),
            cr("LAVATORY", "1-1/4\""),
            cr("SINK, COMMERCIAL KITCHEN", "2\""),
            cr("SINK, RESIDENTIAL KITCHEN", "1-1/2\""),
            cr("SERVICE SINK/MOP BASIN", "3\""),
        ];
        let pick = |f: &str, d: &str| connection_row(&r, &rows, f, d).map(|(x, _)| x.fixture.clone());
        assert_eq!(pick("ADA LAVATORY", "").as_deref(), Some("LAVATORY"));
        assert_eq!(pick("WATER CLOSET", "").as_deref(), Some("WATER CLOSET (TANK)"));
        // A commercial row only for a fixture that says commercial.
        assert_eq!(pick("KITCHEN SINK", "UNDERMOUNT SINGLE BASIN").as_deref(), Some("SINK, RESIDENTIAL KITCHEN"));
        assert_eq!(pick("COMMERCIAL KITCHEN SINK", "").as_deref(), Some("SINK, COMMERCIAL KITCHEN"));
        assert_eq!(pick("SERVICE SINK", "").as_deref(), Some("SERVICE SINK/MOP BASIN"));
        // `SINK` alone fits two rows: no row is guessed.
        assert_eq!(pick("LAUNDRY SINK", ""), None);
    }

    #[test]
    fn plumbing_kinds_and_makers_read_by_their_words() {
        assert_eq!(plumbing_kind("ADA ROLL IN SHOWER").as_deref(), Some("shower"));
        assert_eq!(plumbing_kind("Cast-Iron Floor Drains").as_deref(), Some("floor_drain"));
        assert_eq!(plumbing_kind("SUMP PUMP").as_deref(), Some("sump_pump"));
        assert_eq!(plumbing_kind("PEX TUBING"), None, "TUB is a word, not part of TUBING");
        assert!(same_maker("ACME", "Acme Co"));
        assert!(same_maker("MAKERTON PUMPS", "Makertin Pumps"), "a misspelt maker is the same maker");
        assert!(!same_maker("MAKER ONE", "Other Works"));
    }

    /// A built fixture element (a box the build makes for a read device) of `mark` on Level 1.
    fn fixture_el(s: &mut Session, mark: &str, x: f64, extra: &[(&str, Prop)]) -> Handle {
        fixture_on(s, "Level 1", mark, x, extra)
    }

    fn fixture_on(s: &mut Session, level: &str, mark: &str, x: f64, extra: &[(&str, Prop)]) -> Handle {
        let mut m = super::super::wallgen::empty_mesh();
        super::super::wallgen::add_box(&mut m, Vec2::new(x, 0.0), Vec2::new(x + 12.0, 0.0), 12.0, 0.0, 12.0);
        let tags = super::super::wallgen::tag(cadaddy_walk3d::Part::Device, level, Handle(0));
        let mut store = Props::default();
        if !mark.is_empty() {
            store.set("type_mark", Prop::new(mark, Source::print("P-2", None)));
        }
        store.set("kind", Prop::new("plumbing_fixture", Source::print("P-2", None)));
        for (k, p) in extra {
            store.set(k, p.clone());
        }
        let xdata = props::write(&tags, &store);
        let d = s.doc_mut().expect("doc");
        d.add(&Space::Model, Common { layer: "3D-Devices".into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).expect("add")
    }

    fn prop(s: &Session, h: Handle, key: &str) -> Option<Prop> {
        s.doc().ok().and_then(|d| d.model.get(h)).and_then(|e| props::read(&e.common.xdata).get(key).cloned())
    }

    #[test]
    fn each_fixture_gets_its_product_the_submittal_over_the_schedule() {
        let mut s = Session::new();
        let spec = cadaddy_walk3d::BuildingSpec::from_json("{}").expect("spec");
        s.doc_mut().expect("doc").building_spec = Some(spec.to_json());
        let proposed = Prop::new("A1", Source::computed()).with_note("the nearest CW pipe (proposed: copied, not drawn)");
        let wc = fixture_el(&mut s, "WC-1", 0.0, &[("cold_water", proposed)]);
        let lv = fixture_el(&mut s, "LV-1", 40.0, &[]);
        let sk = fixture_el(&mut s, "SK-1", 80.0, &[]);
        let rp = fixture_el(&mut s, "RP-1", 120.0, &[]);
        let bare = fixture_el(&mut s, "ZZ-1", 160.0, &[]);
        let untagged = fixture_el(&mut s, "", 200.0, &[]);
        let mut lav = row("LV-1", "LAVATORY", "MAKER TEN", "LV-300-0");
        lav.connections = [("cw".to_string(), "1/2\"".to_string())].into_iter().collect();
        let docs = Docs {
            rows: vec![
                row("WC-1", "WATER CLOSET", "MAKER TWO", "PF-1"),
                lav,
                row("SK-1", "KITCHEN SINK", "MAKER TWELVE", "SS-2"),
                row("RP-1", "RECIRCULATION PUMP", "", ""),
            ],
            connections: vec![ConnectionRow {
                fixture: "WATER CLOSET (TANK)".into(),
                sizes: [("waste".to_string(), "3\"".to_string()), ("cw".to_string(), "1/2\"".to_string())].into_iter().collect(),
            }],
            sheet: "P-1".into(),
            submittals: vec![
                read_submittal(&rules(), &package(), "Plumbing Package", "Plumbing Package.pdf", &known(&["WC-1", "LV-1", "SK-1", "RP-1"]))
                    .expect("read"),
            ],
            ..Docs::default()
        };
        let reqs = vec![Requirement {
            kind: "pump".into(),
            product: "IN-LINE CENTRIFUGAL PUMPS".into(),
            section: "22 11 23".into(),
            page: "221123 - 3".into(),
            makers: vec!["Other Pumps".into()],
            substitution: "listed makers only (subject to compliance)".into(),
            ..Requirement::default()
        }];
        let v = apply(&mut s, &docs, &reqs).expect("apply");
        // The submittal's maker and cut sheet's catalog number over the schedule's, which stays beside them.
        let m = prop(&s, wc, "manufacturer").expect("maker");
        assert_eq!(m.text(), "Maker Nine");
        assert_eq!(m.source.kind, SourceKind::Submittal);
        assert!(m.source.doc.as_deref().is_some_and(|d| d.contains("p.3")), "{m:?}");
        assert_eq!(prop(&s, wc, "design_manufacturer").map(|p| p.text()).as_deref(), Some("MAKER TWO"));
        assert_eq!(prop(&s, wc, "catalog_number").map(|p| p.text()).as_deref(), Some("QX-77/A1"));
        assert_eq!(prop(&s, wc, "design_catalog").map(|p| p.text()).as_deref(), Some("PF-1"));
        // Its rough-in from the table by its name; its water derived (its level's water is proposed).
        assert_eq!(prop(&s, wc, "waste_size").map(|p| p.text()).as_deref(), Some("3\""));
        let cw = prop(&s, wc, "cw_size").expect("cw");
        assert!(cw.note.as_deref().is_some_and(|n| n.starts_with("derived")), "{cw:?}");
        assert_eq!(prop(&s, wc, "water_provenance").map(|p| p.text()).as_deref(), Some("derived"));
        // The same maker on both: the cut sheet's catalog number, the schedule's sizes.
        assert_eq!(prop(&s, lv, "manufacturer").map(|p| p.source.kind), Some(SourceKind::Submittal));
        assert!(prop(&s, lv, "design_manufacturer").is_none());
        assert_eq!(prop(&s, lv, "cw_size").map(|p| p.source.kind), Some(SourceKind::Schedule));
        // A sent-back submittal: the schedule's product stands.
        let skm = prop(&s, sk, "manufacturer").expect("sink maker");
        assert_eq!((skm.text().as_str(), skm.source.kind), ("MAKER TWELVE", SourceKind::Schedule));
        assert_eq!(prop(&s, sk, "submittal_status").map(|p| p.text()).as_deref(), Some("sent back"));
        // A pump: powered by its cut sheet, its electrical connection and a box for the takeoff.
        assert!(prop(&s, rp, "electrical_connection").is_some_and(|p| p.text().contains("115 V") && p.source.kind == SourceKind::Submittal));
        let (sp, _) = super::super::bspec::current(&s, "test").expect("spec");
        let boxes: Vec<&cadaddy_walk3d::MepEquipment> =
            sp.mep.equipment.iter().filter(|e| e.properties.get("FOR").map(String::as_str) == Some("RP-1")).collect();
        assert_eq!(boxes.len(), 1, "{:?}", sp.mep.equipment);
        assert_eq!(boxes.first().and_then(|b| b.properties.get("DOC_OBJECT")).map(String::as_str), Some("connection"));
        // No document data: no product, and why.
        assert!(prop(&s, bare, "manufacturer").is_none());
        assert!(prop(&s, bare, "product_gap").is_some_and(|p| p.text().contains("ZZ-1")));
        assert!(prop(&s, untagged, "product_gap").is_some_and(|p| p.text().starts_with("no tag")));
        // The disagreements are review items: the maker, the sent-back sink, the pump's unlisted maker.
        let topics = &v["conflicts_by_topic"];
        assert_eq!(topics["maker"], 1, "{v}");
        assert_eq!(topics["submittal sent back"], 1, "{v}");
        assert_eq!(topics["approved makers"], 1, "{v}");
        assert!(sp.rfis.iter().any(|r| r.topic == "plumbing: maker" && r.sides.len() == 2), "{:?}", sp.rfis);
        assert_eq!(v["with_product"], 4, "{v}");
        assert_eq!(v["no_product_data"], 2, "{v}");
        // A level whose water mains are proposed (no water plan of its own): its fixtures' water
        // is derived, whatever pipe they are on.
        let mut m = super::super::wallgen::empty_mesh();
        super::super::wallgen::add_box(&mut m, Vec2::new(0.0, 40.0), Vec2::new(400.0, 40.0), 1.0, 100.0, 101.0);
        // As the MEP build tags a pipe: its app, `mep`, level, source, then its keys.
        let mut tags = vec![cadaddy_doc::RawTag { code: 1001, value: super::super::wallgen::APP.into() }];
        for t in ["mep", "Level 2", "0", "system:CW", "kind:pipe", "part:run", "status:proposed"] {
            tags.push(cadaddy_doc::RawTag { code: 1000, value: t.into() });
        }
        s.doc_mut()
            .expect("doc")
            .add(&Space::Model, Common { layer: "P-CW-PROPOSED".into(), xdata: tags.into(), ..Common::default() }, EntityKind::Mesh(m))
            .expect("pipe");
        let up =
            fixture_on(&mut s, "Level 2", "LV-1", 300.0, &[("cold_water", Prop::new("B2", Source::computed()).with_note("the nearest CW pipe"))]);
        let v2 = apply(&mut s, &docs, &reqs).expect("again");
        assert_eq!(prop(&s, up, "water_provenance").map(|p| p.text()).as_deref(), Some("derived"), "{v2}");
        assert!(prop(&s, up, "cw_size").and_then(|p| p.note).is_some_and(|n| n.starts_with("derived")));
        assert_eq!(prop(&s, lv, "water_provenance").map(|p| p.text()), None, "Level 1's own water: not flagged (no water line known)");
        // A person's value stays.
        let mine = fixture_el(&mut s, "WC-1", 240.0, &[("manufacturer", Prop::new("MINE", Source::user()))]);
        apply(&mut s, &docs, &reqs).expect("again");
        assert_eq!(prop(&s, mine, "manufacturer").map(|p| p.text()).as_deref(), Some("MINE"));
        let (sp, _) = super::super::bspec::current(&s, "test").expect("spec");
        assert_eq!(sp.mep.equipment.iter().filter(|e| e.properties.contains_key("PLUMBING_FIXTURE")).count(), 1, "a run again replaces its boxes");
    }

    #[test]
    fn bim_products_reads_a_drawn_schedule_onto_the_fixtures_records() {
        let mut s = Session::new();
        // A schedule whose maker column comes first and whose tag header is MARK.
        for (t, x, y) in [
            ("MFR", 10.0, 30.0),
            ("MARK", 14.0, 30.0),
            ("FIXTURE", 15.5, 30.0),
            ("MODEL NO.", 19.0, 30.0),
            ("MAKER TWO", 9.8, 29.5),
            ("WC-1", 14.0, 29.5),
            ("WATER CLOSET", 15.2, 29.5),
            ("PF-1", 19.0, 29.5),
            ("MAKER FIVE", 9.8, 29.0),
            ("FD-1", 14.0, 29.0),
            ("FLOOR DRAIN", 15.2, 29.0),
            ("ZN-4", 19.0, 29.0),
        ] {
            s.execute("text", &json!({"at": [x, y], "text": t, "height": 0.1})).expect("text");
        }
        let wc = fixture_el(&mut s, "WC-1", 0.0, &[]);
        let fd = fixture_el(&mut s, "FD-1", 40.0, &[]);
        let v = s.execute("bim.products", &json!({"submittals": []})).expect("bim.products");
        assert_eq!(v["plumbing"]["with_product"], 2, "{v}");
        assert_eq!(prop(&s, wc, "manufacturer").map(|p| (p.text(), p.source.kind)), Some(("MAKER TWO".to_string(), SourceKind::Schedule)));
        assert_eq!(prop(&s, fd, "catalog_number").map(|p| p.text()).as_deref(), Some("ZN-4"));
        assert_eq!(prop(&s, fd, "product_kind").map(|p| p.text()).as_deref(), Some("floor_drain"));
        // Its product record is built from them.
        let rec: Value = serde_json::from_str(&prop(&s, wc, "product").map(|p| p.text()).unwrap_or_default()).expect("record");
        assert_eq!(rec["resolved"]["maker"], "MAKER TWO", "{rec}");
        assert_eq!(rec["resolved"]["catalog"], "PF-1", "{rec}");
        // `plumbing: false` leaves them alone.
        let v = s.execute("bim.products", &json!({"plumbing": false})).expect("bim.products");
        assert!(v["plumbing"].is_null(), "{v}");
    }
}
