//! Where a wall item's height comes from (Q6132): the heights the documents give, read off the
//! job's sheets, for the prescreen's G-HEIGHT and each packet's objects.
//!
//! An element's expected height above its floor comes from, first that gives one:
//!
//! 1. **an AFF note tied to it on its plan or enlarged plan**: the height its own record has
//!    from a print or a note (the job's reading), else a height note (`18" AFF`, `+48"`) beside
//!    its symbol on the sheet, the nearest to it and it the nearest symbol to the note;
//! 2. **the interior elevation that shows it**: a view titled `… ELEVATION …`, where a height
//!    (`48" AFF`, a dimension string `1'-6"`, a vertical dimension) is beside a label naming its
//!    mark or kind; an elevation titled with a room (`ROOM 214`, `UNIT 3`) is that room's only;
//! 3. **the legend row of its mark or kind**: a device legend's or schedule's row
//!    (`REC  DUPLEX RECEPTACLE  MOUNT 18" AFF`); a row with no mark gives its kind's height;
//! 4. **the schedule row of its mark** (the prints' schedules, a prints height rule for its
//!    mark);
//! 5. **the general or mounting-height notes of its trade**: a notes block's line naming its
//!    kind with a height (`RECEPTACLES ... 18" AFF`, `MOUNT THERMOSTATS AT 48" AFF`) on a sheet
//!    of its trade (or a general/architectural sheet);
//! 6. **a project rule** `validate.height.<mark>`, then `validate.height.<kind>` (domain
//!    `custom`, a height such as `18"`, `4'-0"` or `48 in`), then the project's
//!    `mounting_height` rules.
//!
//! Every height keeps where it came from: the sheet, the point on it (model coordinates) and
//! the words read. A record the job wrote as an assumption (`rule: …`, a kind's usual height)
//! is no document. Nothing here panics; every scan is capped.

use std::collections::{BTreeMap, HashMap};

use cadaddy_doc::projrules::{ProjectRules, RuleDomain};
use cadaddy_doc::props::SourceKind;
use cadaddy_doc::{Drawing, Entity, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::validate::{self as v, Prints};
use cadaddy_walk3d::{BuildingSpec, SheetLink};
use serde_json::{Value, json};

use super::mountlist;
pub use super::mountlist::Datum;
use super::validate_kind::{below_heading, looks_like_mark, model_kind_in, rows_under, words};
use super::validate_packet::{r2, trade_of_discipline};

/// The project-rule key prefix of a kind's or mark's mounting height.
pub const HEIGHT_PREFIX: &str = "validate.height.";
/// Objects looked at for texts, at most (every one).
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
/// Text lines kept, at most.
const MAX_TEXTS: usize = 400_000;
/// Longest text read (a note line); longer ones are paragraphs no height is read from.
const MAX_TEXT: usize = 300;
/// Longest MText read, characters.
const MAX_MTEXT: usize = 20_000;
/// Headings (elevation titles, legends, notes) kept, at most.
const MAX_HEADINGS: usize = 2_000;
/// Elevation view titles kept, at most (each text is looked for over every one).
const MAX_TITLES: usize = 500;
/// Heights read of each kind (elevation, legend, notes, plan notes), at most.
const MAX_ENTRIES: usize = 50_000;
/// An elevation view lies over its title: from this many title heights left of it to this
/// many right, and up to this many above.
const VIEW_LEFT: f64 = 20.0;
const VIEW_RIGHT: f64 = 120.0;
const VIEW_UP: f64 = 80.0;
/// A height in an elevation is a label's when the label is within this many text heights.
const LABEL_REACH: f64 = 30.0;
/// A plan height note is a symbol's when the symbol is within this many text heights.
const NOTE_REACH: f64 = 8.0;
/// A plan height note is a short note: at most this many characters.
const MAX_NOTE: usize = 40;
/// Kinds listed in `unmeasured` one by one, at most.
pub const MAX_KINDS_LISTED: usize = 60;
/// The heights a document gives, inches: from 6" to 20'.
const HEIGHT_RANGE: (f64, f64) = (6.0, 240.0);

/// A height a document (or a project rule) gives an element, and where it comes from.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub inches: f64,
    /// `plan_note`, `elevation`, `legend`, `schedule`, `general_note`, `project_rule`.
    pub from: &'static str,
    /// Where, in words (the finding's `expected_from`).
    pub what: String,
    /// The sheet it is on.
    pub sheet: Option<String>,
    /// The point read, model coordinates.
    pub at: Option<[f64; 2]>,
    /// The words read.
    pub text: Option<String>,
    /// What the height is measured to on the item (its middle unless the words say its top or
    /// bottom: `3'-10" AFF TO TOP`).
    pub datum: Datum,
}

impl Found {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "in": r2(self.inches),
            "from": self.from,
            "what": self.what,
            "sheet": self.sheet,
            "at": self.at.map(|p| json!([r2(p[0]), r2(p[1])])),
            "text": self.text,
            "to": self.datum.name(),
        })
    }
}

/// An element's own height record (`mount_height`): its inches, the source kind and document.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub inches: f64,
    pub kind: SourceKind,
    pub doc: String,
    /// Not read from a document: a default, a rule, a kind's usual height (`rule: on the floor,
    /// 16" high` written as a print's).
    pub assumed: bool,
    /// What the height is measured to on the element (`mount_datum`: its top for a switch
    /// `3'-10" AFF TO TOP`, a water closet's seat); its middle when not said.
    pub datum: Datum,
}

/// Does a record's document text say it was assumed, not read?
fn says_assumed(doc: &str) -> bool {
    let d = doc.trim().to_ascii_lowercase();
    d.starts_with("rule")
        || ["default", "usual height", "not read", "not printed", "typical value", "assumed", "estimat"].iter().any(|w| d.contains(w))
}

/// An element's height record, if it has one that reads.
pub fn record_of(e: &Entity) -> Option<Record> {
    let props = cadaddy_doc::props::read(&e.common.xdata);
    let p = props.get("mount_height")?;
    let z = p
        .text()
        .trim()
        .trim_end_matches('"')
        .trim_end_matches("in")
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|z| z.is_finite() && *z > 0.0 && *z < HEIGHT_RANGE.1)?;
    let doc = [p.source.doc.as_deref().unwrap_or_default(), p.note.as_deref().unwrap_or_default()].join(" ").trim().to_string();
    let assumed = !matches!(
        p.source.kind,
        SourceKind::Print | SourceKind::Legend | SourceKind::Schedule | SourceKind::Spec | SourceKind::Submittal | SourceKind::Note
    ) || says_assumed(p.source.doc.as_deref().unwrap_or_default());
    let datum = props.get("mount_datum").map_or(Datum::Centre, |d| Datum::parse(&d.text()));
    Some(Record { inches: z, kind: p.source.kind, doc: p.source.doc.clone().unwrap_or(doc), assumed, datum })
}

/// Where an element's model height comes from, by its record: `document`, `default` (a
/// default or an assumption no document gives), `rule` (the project's or a standard's rule),
/// `user`, `computed`; none when it has no record.
pub fn model_origin(r: Option<&Record>) -> Option<&'static str> {
    let r = r?;
    Some(match r.kind {
        SourceKind::Rule => "rule",
        SourceKind::User => "user",
        SourceKind::Computed => "computed",
        _ if r.assumed => "default",
        _ => "document",
    })
}

// ---------------------------------------------------------------- project rules

/// Is `key` a kind's or mark's mounting height (`validate.height.<kind|mark>`)?
pub fn is_height_key(key: &str) -> bool {
    key.trim().get(..HEIGHT_PREFIX.len()).is_some_and(|p| p.eq_ignore_ascii_case(HEIGHT_PREFIX))
}

/// A height as a project rule gives it (`18`, `18"`, `48 in`, `4'-0"`, `18" AFF`), inches.
fn rule_inches(value: &str) -> Option<f64> {
    let t = value.trim();
    let t = t.strip_suffix("A.F.F.").or_else(|| t.strip_suffix("AFF")).or_else(|| t.strip_suffix("aff")).unwrap_or(t).trim();
    v::parse_inches(t).filter(|z| (HEIGHT_RANGE.0..=HEIGHT_RANGE.1).contains(z))
}

/// Read a project rule `validate.height.<kind|mark> = <height>`: (what it names, the inches).
/// `project.rules.set` refuses one that doesn't read.
pub fn check_height_rule(domain: RuleDomain, key: &str, value: &str) -> std::result::Result<(String, f64), String> {
    if domain != RuleDomain::Custom {
        return Err(format!("`{key}` is a mounting height for Validate: set it in domain `custom`"));
    }
    let what = key.trim().get(HEIGHT_PREFIX.len()..).map(str::trim).unwrap_or_default();
    if what.is_empty() || what.len() > 60 {
        return Err(format!(
            "`{key}`: name the kind or mark after `{HEIGHT_PREFIX}` (e.g. `{HEIGHT_PREFIX}receptacle`, `{HEIGHT_PREFIX}S3`), at most 60 characters"
        ));
    }
    let z = rule_inches(value).ok_or_else(|| {
        format!("`{key}`: `{}` is no height above the floor from 6\" to 20' (e.g. 18\", 48 in, 4'-0\")", value.chars().take(40).collect::<String>())
    })?;
    Ok((what.to_string(), z))
}

/// The project's `validate.height.*` rules that read: (what they name, inches, the key).
fn project_heights(pr: &ProjectRules) -> Vec<(String, f64, String)> {
    pr.list_domain(RuleDomain::Custom)
        .into_iter()
        .filter(|e| is_height_key(&e.key))
        .filter_map(|e| check_height_rule(RuleDomain::Custom, &e.key, &e.active.value).ok().map(|(w, z)| (w, z, e.key.clone())))
        .collect()
}

// ---------------------------------------------------------------- reading the sheets

/// A height written as a mounting height (`18" AFF`, `+48"`, `MOUNT AT 4'-0" A.F.F.`), inches.
pub fn aff_inches(text: &str) -> Option<f64> {
    super::propfill::mount_height_in(text).filter(|z| (HEIGHT_RANGE.0..=HEIGHT_RANGE.1).contains(z))
}

/// A dimension string (`18"`, `1'-6"`, `4' - 0"`), inches: only feet-and-inch marks and
/// digits.
fn dim_inches(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() || t.len() > 16 || !(t.contains('"') || t.contains('\'')) {
        return None;
    }
    if !t.chars().all(|c| c.is_ascii_digit() || matches!(c, '"' | '\'' | '-' | ' ' | '.' | '/')) {
        return None;
    }
    v::parse_inches(&t.replace(' ', "")).filter(|z| (HEIGHT_RANGE.0..=HEIGHT_RANGE.1).contains(z))
}

/// A word as one: `RECEPTACLES` → `receptacle`, `SWITCHES` → `switch`.
fn singular(w: &str) -> String {
    let w = w.to_ascii_lowercase();
    if w.len() > 5 && (w.ends_with("ches") || w.ends_with("shes") || w.ends_with("xes") || w.ends_with("sses")) {
        w.get(..w.len() - 2).unwrap_or(&w).to_string()
    } else if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") {
        w.get(..w.len() - 1).unwrap_or(&w).to_string()
    } else {
        w
    }
}

/// The kind the model holds that a note's words name (plurals too).
pub fn kind_in(text: &str) -> Option<&'static str> {
    let one: Vec<String> = words(text).split('_').map(singular).collect();
    model_kind_in(&one.join(" ")).or_else(|| model_kind_in(text))
}

/// An MText's lines as plain text (paragraph breaks split it; formatting codes dropped).
fn mtext_lines(s: &str) -> Vec<String> {
    let mut out = String::new();
    let mut it = s.chars().take(MAX_MTEXT).peekable();
    while let Some(c) = it.next() {
        match c {
            '{' | '}' => {}
            '\\' => match it.peek().copied() {
                Some('P') => {
                    it.next();
                    out.push('\n');
                }
                Some(c2 @ ('\\' | '{' | '}')) => {
                    it.next();
                    out.push(c2);
                }
                Some('~') => {
                    it.next();
                    out.push(' ');
                }
                Some('L' | 'l' | 'O' | 'o' | 'K' | 'k' | 'N') => {
                    it.next();
                }
                Some(c2) if c2.is_ascii_alphabetic() => {
                    it.next();
                    for x in it.by_ref() {
                        if x == ';' {
                            break;
                        }
                    }
                }
                _ => {}
            },
            _ => out.push(c),
        }
    }
    out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// The drawing's text lines (TEXT, each MText line), with where and how tall.
fn texts(d: &Drawing) -> Vec<TextItem> {
    let mut out = Vec::new();
    let mut push = |value: &str, at: Vec2, height: f64| {
        let value = value.trim();
        if !value.is_empty() && value.len() <= MAX_TEXT && at.is_finite() && height.is_finite() && height > 0.0 {
            out.push(TextItem { value: value.to_string(), at, height, rotation: 0.0 });
        }
    };
    for e in d.model.iter().take(MAX_ENTITIES) {
        match &e.kind {
            EntityKind::Text(t) => push(&t.value, t.insert.xy(), t.height),
            EntityKind::MText(t) => {
                let at = t.insert.xy();
                for (i, line) in mtext_lines(&t.contents).iter().enumerate().take(200) {
                    push(line, Vec2::new(at.x, at.y - i as f64 * t.height * 5.0 / 3.0), t.height);
                }
            }
            _ => {}
        }
    }
    out.truncate(MAX_TEXTS);
    out
}

/// The sheet a model point lies on.
fn sheet_at(sheets: &[SheetLink], p: Vec2) -> Option<&SheetLink> {
    sheets.iter().find(|s| s.rect.is_some_and(|r| p.x >= r[0].min(r[2]) && p.x <= r[0].max(r[2]) && p.y >= r[1].min(r[3]) && p.y <= r[1].max(r[3])))
}

/// A sheet's trade, by its discipline (else its number's letters).
fn trade_of_sheet(s: &SheetLink) -> &'static str {
    let disc = s.discipline.clone().unwrap_or_else(|| s.number.trim().chars().take_while(char::is_ascii_alphabetic).collect());
    trade_of_discipline(&disc)
}

/// Is `t` an elevation view's title (`INTERIOR ELEVATION - ROOM 214`, `INT. ELEV. NORTH`)? The
/// word ELEVATION, or ELEV with INTERIOR or a compass side: a plan's `ELEV` alone is an elevator.
fn is_elevation_title(t: &str) -> bool {
    let u = t.trim().to_ascii_uppercase();
    if u.len() > 80 || u.contains('\'') || u.contains('"') || aff_inches(&u).is_some() {
        return false;
    }
    let ws: Vec<&str> = u.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |x: &[&str]| ws.iter().any(|w| x.contains(w));
    has(&["ELEVATION", "ELEVATIONS"])
        || (has(&["ELEV"])
            && has(&["INTERIOR", "INT", "NORTH", "SOUTH", "EAST", "WEST", "N", "S", "E", "W"])
            && !has(&["LOBBY", "MACH", "MACHINE", "PIT", "SHAFT", "CAR"]))
}

/// Is `t` a notes block's heading (`GENERAL NOTES`, `ELECTRICAL NOTES`, `MOUNTING HEIGHTS`)?
fn is_notes_heading(t: &str) -> bool {
    let u = t.trim().to_ascii_uppercase();
    u.len() <= 60
        && aff_inches(&u).is_none()
        && (u.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "NOTES") || u.contains("MOUNTING HEIGHT"))
        && !cadaddy_mepfix::device_types::is_device_table_heading(&u)
}

/// Is `t` a legend's heading: a device legend or schedule's (`ELECTRICAL LEGEND`), or a symbol
/// list's (`FIRE ALARM DEVICE SYMBOL LIST`)?
fn is_legend_heading(t: &str) -> bool {
    let u = t.trim().to_ascii_uppercase();
    cadaddy_mepfix::device_types::is_device_table_heading(&u) || (u.len() <= 60 && u.contains("SYMBOL") && u.contains("LIST"))
}

/// The room numbers a view's title names (`ROOM 214`, `RM 105A`, `UNIT 3`, `SUITE 12`).
fn title_rooms(t: &str) -> Vec<String> {
    let toks: Vec<String> = t.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect();
    toks.windows(2)
        .filter(|w| matches!(w.first().map(String::as_str), Some("ROOM" | "RM" | "UNIT" | "SUITE")))
        .filter_map(|w| w.get(1).cloned())
        .filter(|n| n.chars().any(|c| c.is_ascii_digit()) && n.len() <= 8)
        .collect()
}

/// A height read off the sheets and what it is for.
#[derive(Clone, Debug)]
struct Entry {
    mark: Option<String>,
    kind: Option<String>,
    /// The rooms it is only for (an elevation titled with a room); empty: any.
    rooms: Vec<String>,
    trade: &'static str,
    found: Found,
    /// A list's item (`WATER CLOSETS`, `LIGHT SWITCH`): what it names, matched by its words
    /// ([`mountlist::names_item`]); a legend row's words after its mark.
    subject: Option<String>,
    /// The height a list gives an accessible room's item (`ADA = 34" MAX.`).
    ada: Option<f64>,
}

/// A height note on a plan (`18" AFF`, `GFCI RECEPTACLE @ 42" AFF`), not yet tied to a symbol.
#[derive(Clone, Debug)]
struct PlanNote {
    /// The kind it names, if any: it ties only to a symbol of that kind, and is a general note
    /// when no symbol is beside it.
    kind: Option<String>,
    trade: &'static str,
    at: Vec2,
    height: f64,
    inches: f64,
    text: String,
    sheet: Option<String>,
    datum: Datum,
}

/// What an element is, for its height.
pub struct Ask<'a> {
    pub handle: &'a str,
    pub kind: &'a str,
    pub mark: Option<&'a str>,
    /// Its room's number and name.
    pub room_number: Option<&'a str>,
    pub room_name: Option<&'a str>,
    /// Its trade (`electrical`, `mechanical`…).
    pub trade: &'a str,
    pub record: Option<&'a Record>,
    /// What it is in words (`water closet WC-1`, `GFCI receptacle`): a list's item names it by
    /// these too.
    pub name: Option<&'a str>,
    /// It is an accessible room's (an `ADA` fixture): a list's `ADA =` height is its.
    pub ada: bool,
}

/// The heights the job's documents give.
#[derive(Clone, Debug, Default)]
pub struct DocHeights {
    elevation: Vec<Entry>,
    legend: Vec<Entry>,
    notes: Vec<Entry>,
    plan_notes: Vec<PlanNote>,
    /// Element handle (upper case) → the plan note tied to it.
    tied: HashMap<String, Found>,
    /// `validate.height.*`: what it names, inches, the key.
    project: Vec<(String, f64, String)>,
    /// Each legend row's mark and its words (a row with no height too).
    legend_rows: Vec<(String, String)>,
}

impl DocHeights {
    /// Read the heights off the drawing's sheets. `marks`: the marks the model's elements carry
    /// (an elevation's label is a mark only if one of these).
    pub fn of(d: &Drawing, sp: &BuildingSpec, marks: &std::collections::BTreeSet<String>) -> DocHeights {
        let all = texts(d);
        let sheets = sp.sheets.as_slice();
        let sheet_no = |p: Vec2| sheet_at(sheets, p).map(|s| s.number.clone());
        let trade_at = |p: Vec2| sheet_at(sheets, p).map_or("general", trade_of_sheet);
        let mut used = vec![false; all.len()];
        let label_of = |t: &str| -> (Option<String>, Option<String>) {
            let mark = t
                .split(|c: char| c.is_whitespace() || c == ',' || c == ':')
                .map(str::trim)
                .find(|w| looks_like_mark(w) && marks.contains(&w.to_ascii_uppercase()))
                .map(str::to_ascii_uppercase);
            (mark, kind_in(t).map(str::to_string))
        };
        let short = |s: &str| s.chars().take(160).collect::<String>();
        let mut out = DocHeights { project: project_heights(&d.project_rules), ..DocHeights::default() };

        // Interior elevations: each text over a title belongs to the nearest title below it.
        let titles: Vec<usize> = all.iter().enumerate().filter(|(_, t)| is_elevation_title(&t.value)).map(|(i, _)| i).take(MAX_TITLES).collect();
        let view_of = |p: Vec2| -> Option<usize> {
            titles
                .iter()
                .filter_map(|&i| all.get(i).map(|t| (i, t)))
                .filter(|(_, t)| {
                    p.x >= t.at.x - VIEW_LEFT * t.height
                        && p.x <= t.at.x + VIEW_RIGHT * t.height
                        && p.y > t.at.y + 0.3 * t.height
                        && p.y <= t.at.y + VIEW_UP * t.height
                })
                .min_by(|a, b| (p.y - a.1.at.y).total_cmp(&(p.y - b.1.at.y)))
                .map(|(i, _)| i)
        };
        let mut in_view: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        if !titles.is_empty() {
            for (i, t) in all.iter().enumerate() {
                if titles.contains(&i) {
                    continue;
                }
                if let Some(v) = view_of(t.at) {
                    in_view.entry(v).or_default().push(i);
                    if let Some(u) = used.get_mut(i) {
                        *u = true;
                    }
                }
            }
        }
        let elev_entry = |title: &TextItem, mark: Option<String>, kind: Option<String>, inches: f64, at: Vec2, text: String| Entry {
            mark,
            kind,
            rooms: title_rooms(&title.value),
            trade: trade_at(title.at),
            found: Found {
                inches,
                from: "elevation",
                what: format!("interior elevation {}{}", short(&title.value), sheet_no(title.at).map_or_else(String::new, |s| format!(" on {s}"))),
                sheet: sheet_no(at),
                at: Some([at.x, at.y]),
                datum: Datum::of(&text),
                text: Some(short(&text)),
            },
            subject: None,
            ada: None,
        };
        for (&ti, members) in &in_view {
            let Some(title) = all.get(ti) else { continue };
            let labels: Vec<(&TextItem, Option<String>, Option<String>)> = members
                .iter()
                .filter_map(|&i| all.get(i))
                .filter(|t| aff_inches(&t.value).is_none() && dim_inches(&t.value).is_none())
                .map(|t| {
                    let (m, k) = label_of(&t.value);
                    (t, m, k)
                })
                .filter(|(_, m, k)| m.is_some() || k.is_some())
                .collect();
            for t in members.iter().filter_map(|&i| all.get(i)) {
                if out.elevation.len() >= MAX_ENTRIES {
                    break;
                }
                let Some(z) = aff_inches(&t.value).or_else(|| dim_inches(&t.value)) else { continue };
                let (m, k) = label_of(&t.value);
                if m.is_some() || k.is_some() {
                    out.elevation.push(elev_entry(title, m, k, z, t.at, t.value.clone()));
                    continue;
                }
                let near = labels
                    .iter()
                    .map(|l| (l, (l.0.at - t.at).len()))
                    .filter(|(l, dist)| *dist <= LABEL_REACH * t.height.max(l.0.height))
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some(((l, m, k), _)) = near {
                    out.elevation.push(elev_entry(title, m.clone(), k.clone(), z, t.at, format!("{} {}", l.value, t.value)));
                }
            }
        }
        // Vertical dimensions in an elevation: the height of their upper point, by the label
        // nearest it.
        if !titles.is_empty() {
            let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
            for e in d.model.iter().take(MAX_ENTITIES) {
                if out.elevation.len() >= MAX_ENTRIES {
                    break;
                }
                let EntityKind::Dimension(dm) = &e.kind else { continue };
                let (a, b) = (dm.p13.xy(), dm.p14.xy());
                let vertical = match dm.kind {
                    cadaddy_doc::DimKind::Linear { rotation } => rotation.sin().abs() > 0.98,
                    cadaddy_doc::DimKind::Aligned => (a.x - b.x).abs() < 0.05 * (a.y - b.y).abs(),
                    _ => false,
                };
                let (top, bottom) = if a.y >= b.y { (a, b) } else { (b, a) };
                let span = top.y - bottom.y;
                if !vertical || !top.is_finite() || !bottom.is_finite() || span <= 0.0 {
                    continue;
                }
                let Some(ti) = view_of(top) else { continue };
                let Some(title) = all.get(ti) else { continue };
                let written = dm.text.trim();
                let z = if written.is_empty() || written.contains("<>") {
                    let m = if dm.measurement.is_finite() && dm.measurement > 0.0 { dm.measurement } else { span };
                    Some(m / upf * 12.0).filter(|z| (HEIGHT_RANGE.0..=HEIGHT_RANGE.1).contains(z))
                } else {
                    dim_inches(written).or_else(|| aff_inches(written))
                };
                let Some(z) = z else { continue };
                let near = in_view
                    .get(&ti)
                    .into_iter()
                    .flatten()
                    .filter_map(|&i| all.get(i))
                    .filter(|t| aff_inches(&t.value).is_none() && dim_inches(&t.value).is_none())
                    .filter_map(|t| {
                        let (m, k) = label_of(&t.value);
                        (m.is_some() || k.is_some()).then(|| (t, m, k, (t.at - top).len()))
                    })
                    .filter(|(t, _, _, dist)| *dist <= span.max(LABEL_REACH * t.height))
                    .min_by(|a, b| a.3.total_cmp(&b.3));
                if let Some((l, m, k, _)) = near {
                    let shown = if written.is_empty() || written.contains("<>") { format!("{z:.1}\"") } else { written.to_string() };
                    out.elevation.push(elev_entry(title, m, k, z, top, format!("{} {} (dimension)", l.value, shown)));
                }
            }
        }

        // Legends and notes blocks: their rows.
        let legend_heads: Vec<&TextItem> = all.iter().filter(|t| is_legend_heading(&t.value)).take(MAX_HEADINGS).collect();
        let notes_heads: Vec<&TextItem> = all.iter().filter(|t| is_notes_heading(&t.value)).take(MAX_HEADINGS).collect();
        let under = |heads: &[&TextItem], used: &[bool]| -> Vec<TextItem> {
            all.iter()
                .enumerate()
                .filter(|(i, t)| !used.get(*i).copied().unwrap_or(true) && heads.iter().any(|h| below_heading(h, t.at)))
                .map(|(_, t)| t.clone())
                .collect()
        };
        let near_legend = under(&legend_heads, &used);
        for h in &legend_heads {
            for row in rows_under(h, &near_legend) {
                if out.legend.len() >= MAX_ENTRIES {
                    break;
                }
                let joined = row.iter().map(|t| t.value.as_str()).collect::<Vec<_>>().join(" ");
                let i = row.iter().position(|t| looks_like_mark(&t.value) && aff_inches(&t.value).is_none());
                // A symbol's letter and the mark printed small beside it (`S` with `R`: the mark
                // is either): each a mark of the row.
                let mut marks_here: Vec<String> = Vec::new();
                if let Some(i) = i {
                    let mut j = i;
                    while let Some(t) =
                        row.get(j).filter(|t| looks_like_mark(&t.value) && t.value.trim().len() <= 3 && aff_inches(&t.value).is_none())
                    {
                        if j > i && row.get(j - 1).is_none_or(|p| t.at.x - p.at.x > 2.0 * t.height.max(p.height)) {
                            break;
                        }
                        marks_here.push(t.value.trim().to_ascii_uppercase());
                        j += 1;
                        if marks_here.len() >= 3 {
                            break;
                        }
                    }
                    if marks_here.is_empty() {
                        marks_here.extend(row.get(i).map(|t| t.value.trim().to_ascii_uppercase()));
                    }
                }
                let skip = i.map_or(0, |i| i + marks_here.len().max(1));
                let said: Vec<&str> = row.iter().skip(skip).map(|t| t.value.as_str()).collect();
                let words_said = said.join(" ");
                for m in &marks_here {
                    if out.legend_rows.len() < MAX_ENTRIES {
                        out.legend_rows.push((m.clone(), short(&words_said)));
                    }
                }
                let Some(z) = aff_inches(&joined) else { continue };
                let kind = kind_in(&words_said).map(str::to_string);
                let at = row.first().map_or(h.at, |t| t.at);
                let marks_or_none: Vec<Option<String>> =
                    if marks_here.is_empty() { vec![None] } else { marks_here.iter().cloned().map(Some).collect() };
                for mark in marks_or_none {
                    out.legend.push(Entry {
                        mark: mark.clone(),
                        kind: kind.clone(),
                        rooms: Vec::new(),
                        trade: trade_at(h.at),
                        found: Found {
                            inches: z,
                            from: "legend",
                            what: format!(
                                "legend {}{}{}",
                                short(&h.value),
                                mark.as_deref().map_or_else(String::new, |m| format!(", row {m}")),
                                sheet_no(h.at).map_or_else(String::new, |s| format!(" on {s}"))
                            ),
                            sheet: sheet_no(at),
                            at: Some([at.x, at.y]),
                            datum: Datum::of(&joined),
                            text: Some(short(&joined)),
                        },
                        subject: Some(words_said.clone()).filter(|w| !w.trim().is_empty()),
                        ada: None,
                    });
                }
            }
        }
        // Rows read by a legend are no notes.
        for (i, t) in all.iter().enumerate() {
            if legend_heads.iter().any(|h| below_heading(h, t.at))
                && let Some(u) = used.get_mut(i)
            {
                *u = true;
            }
        }
        let note_entry = |text: &str, z: f64, kind: &str, at: Vec2, block: Option<&str>| Entry {
            mark: None,
            kind: Some(kind.to_string()),
            rooms: Vec::new(),
            trade: trade_at(at),
            found: Found {
                inches: z,
                from: "general_note",
                what: format!(
                    "{}{}",
                    block.map_or_else(|| "general note".to_string(), |b| format!("notes {}", short(b))),
                    sheet_no(at).map_or_else(String::new, |s| format!(" on {s}"))
                ),
                sheet: sheet_no(at),
                at: Some([at.x, at.y]),
                datum: Datum::of(text),
                text: Some(short(text)),
            },
            subject: None,
            ada: None,
        };
        let near_notes = under(&notes_heads, &used);
        for h in &notes_heads {
            for row in rows_under(h, &near_notes) {
                if out.notes.len() >= MAX_ENTRIES {
                    break;
                }
                let joined = row.iter().map(|t| t.value.as_str()).collect::<Vec<_>>().join(" ");
                if let (Some(z), Some(k)) = (aff_inches(&joined), kind_in(&joined)) {
                    let at = row.first().map_or(h.at, |t| t.at);
                    out.notes.push(note_entry(&joined, z, k, at, Some(&h.value)));
                }
            }
        }
        for (i, t) in all.iter().enumerate() {
            if notes_heads.iter().any(|h| below_heading(h, t.at))
                && let Some(u) = used.get_mut(i)
            {
                *u = true;
            }
        }
        // A notes list of heights (`9. WIRING DEVICE MOUNTED HEIGHTS: … RECEPTACLES 1'-4" CENTERLINE`, an
        // accessibility sheet's `WATER CLOSETS (TO TOP OF SEAT): STANDARD = …`): its items.
        for l in mountlist::read_lists_marking(&all, &mut used) {
            if out.notes.len() >= MAX_ENTRIES {
                break;
            }
            out.notes.push(list_entry(&l.to_note(&sheet_no(l.at).unwrap_or_default()), trade_at(l.at), Some([l.at.x, l.at.y])));
        }
        // The lists the job read off sheets the drawing doesn't hold.
        for n in sp.mount_notes.iter().take(mountlist::MAX_ENTRIES) {
            if out.notes.len() >= MAX_ENTRIES {
                break;
            }
            let disc: String = n.sheet.trim().chars().take_while(char::is_ascii_alphabetic).collect();
            out.notes.push(list_entry(n, trade_of_discipline(&disc), None));
        }
        // The rest: a short height note is a plan note for the symbol beside it (of the kind it
        // names, if it names one); a longer line naming a kind and a height is a general note.
        for (i, t) in all.iter().enumerate() {
            if used.get(i).copied().unwrap_or(true) {
                continue;
            }
            let Some(z) = aff_inches(&t.value) else { continue };
            let kind = kind_in(&t.value);
            let numbered = t.value.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit()) && t.value.contains('.');
            if t.value.len() <= MAX_NOTE + 20 && !numbered {
                if out.plan_notes.len() < MAX_ENTRIES {
                    out.plan_notes.push(PlanNote {
                        kind: kind.map(str::to_string),
                        trade: trade_at(t.at),
                        at: t.at,
                        height: t.height,
                        inches: z,
                        text: t.value.clone(),
                        sheet: sheet_no(t.at),
                        datum: Datum::of(&t.value),
                    });
                }
            } else if let Some(k) = kind
                && out.notes.len() < MAX_ENTRIES
            {
                out.notes.push(note_entry(&t.value, z, k, t.at, None));
            }
        }
        out
    }

    /// Tie each plan height note to the symbol beside it: the nearest symbol within reach,
    /// when the note is also the nearest to that symbol. `points`: each element's handle and
    /// its kind and its symbol's point on the sheet (model coordinates). A note naming a kind
    /// that ties to no symbol is a general note of its kind.
    pub fn tie(&mut self, points: &[(String, String, Vec2)]) {
        if self.plan_notes.is_empty() {
            return;
        }
        // A grid of the symbols, cells the typical reach wide.
        let mut reaches: Vec<f64> = self.plan_notes.iter().map(|n| NOTE_REACH * n.height).collect();
        reaches.sort_by(f64::total_cmp);
        let cell = reaches.get(reaches.len() / 2).copied().filter(|c| c.is_finite() && *c > 0.0).unwrap_or(1.0);
        let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, (_, _, p)) in points.iter().enumerate().filter(|(_, (_, _, p))| p.is_finite()) {
            grid.entry(key(*p)).or_default().push(i);
        }
        // Each note's nearest symbol.
        let mut best_for: HashMap<usize, (f64, usize)> = HashMap::new();
        for (ni, n) in self.plan_notes.iter().enumerate() {
            let reach = NOTE_REACH * n.height;
            let r = ((reach / cell).ceil() as i64).clamp(1, 4);
            let (cx, cy) = key(n.at);
            let mut near: Option<(f64, usize)> = None;
            for gx in cx - r..=cx + r {
                for gy in cy - r..=cy + r {
                    for &pi in grid.get(&(gx, gy)).into_iter().flatten() {
                        let Some((_, kind, p)) = points.get(pi) else { continue };
                        if n.kind.as_deref().is_some_and(|k| !cadaddy_walk3d::validate_kinds::kind_matches(k, kind) && !has_words(kind, k)) {
                            continue;
                        }
                        let dist = (*p - n.at).len();
                        if dist <= reach && near.is_none_or(|(b, bi)| dist < b || (dist == b && pi < bi)) {
                            near = Some((dist, pi));
                        }
                    }
                }
            }
            if let Some((dist, pi)) = near
                && best_for.get(&pi).is_none_or(|(b, _)| dist < *b)
            {
                best_for.insert(pi, (dist, ni));
            }
        }
        let mut tied_notes = vec![false; self.plan_notes.len()];
        for (pi, (_, ni)) in best_for {
            let (Some((h, _, _)), Some(n)) = (points.get(pi), self.plan_notes.get(ni)) else { continue };
            if let Some(t) = tied_notes.get_mut(ni) {
                *t = true;
            }
            self.tied.insert(
                h.to_ascii_uppercase(),
                Found {
                    inches: n.inches,
                    from: "plan_note",
                    what: format!("AFF note \"{}\" beside it{}", n.text, n.sheet.as_deref().map_or_else(String::new, |s| format!(" on {s}"))),
                    sheet: n.sheet.clone(),
                    at: Some([n.at.x, n.at.y]),
                    text: Some(n.text.clone()),
                    datum: n.datum,
                },
            );
        }
        for (n, _) in self.plan_notes.iter().zip(&tied_notes).filter(|(n, t)| !**t && n.kind.is_some()) {
            if self.notes.len() >= MAX_ENTRIES {
                break;
            }
            let Some(k) = n.kind.clone() else { continue };
            self.notes.push(Entry {
                mark: None,
                kind: Some(k),
                rooms: Vec::new(),
                trade: n.trade,
                found: Found {
                    inches: n.inches,
                    from: "general_note",
                    what: format!("general note{}", n.sheet.as_deref().map_or_else(String::new, |s| format!(" on {s}"))),
                    sheet: n.sheet.clone(),
                    at: Some([n.at.x, n.at.y]),
                    text: Some(n.text.clone()),
                    datum: n.datum,
                },
                subject: None,
                ada: None,
            });
        }
    }

    /// Plan notes, elevation, legend and notes heights read (for the summary).
    pub fn counts(&self) -> Value {
        json!({
            "plan_notes": self.plan_notes.len(),
            "tied_plan_notes": self.tied.len(),
            "elevation": self.elevation.len(),
            "legend": self.legend.len(),
            "general_notes": self.notes.len(),
            "project_rules": self.project.len(),
        })
    }

    /// The height the documents give an element, first that gives one (see the module doc).
    pub fn expected(&self, a: &Ask, prints: Option<&Prints>, pr: &ProjectRules) -> Option<Found> {
        let record = a.record.filter(|r| !r.assumed);
        let from_record = |r: &Record, from: &'static str| Found {
            inches: r.inches,
            from,
            what: format!("its record: {} {}", format!("{:?}", r.kind).to_ascii_lowercase(), r.doc).trim().to_string(),
            sheet: None,
            at: None,
            text: (!r.doc.is_empty()).then(|| r.doc.chars().take(160).collect()),
            datum: r.datum,
        };
        // 1. An AFF note tied to it on its plan.
        if let Some(r) = record.filter(|r| matches!(r.kind, SourceKind::Print | SourceKind::Note)) {
            return Some(from_record(r, "plan_note"));
        }
        if let Some(f) = self.own_note(a.handle) {
            return Some(f);
        }
        // 2. The interior elevation that shows it.
        if let Some(f) = self.elevation_of(a, false) {
            return Some(f);
        }
        // 3. The legend row of its mark, else of its kind.
        if let Some(r) = record.filter(|r| r.kind == SourceKind::Legend) {
            return Some(from_record(r, "legend"));
        }
        if let Some(f) = self.legend_of(a) {
            return Some(f);
        }
        // 4. The schedule row of its mark.
        if let Some(r) = record.filter(|r| matches!(r.kind, SourceKind::Schedule | SourceKind::Submittal)) {
            return Some(from_record(r, "schedule"));
        }
        if let Some(f) = schedule_of(a, prints) {
            return Some(f);
        }
        // 5. The general notes of its trade (or of the whole set).
        if let Some(r) = record.filter(|r| r.kind == SourceKind::Spec) {
            return Some(from_record(r, "general_note"));
        }
        if let Some(f) = self.notes_of(a, prints) {
            return Some(f);
        }
        // 6. The project's rules.
        self.rule_of(a, pr)
    }

    /// The AFF note tied to the element's symbol on its plan (`handle`: the plan element's, or
    /// the symbol's a built part was read from).
    pub fn own_note(&self, handle: &str) -> Option<Found> {
        self.tied.get(&handle.to_ascii_uppercase()).cloned()
    }

    /// The interior elevation that shows it: its room's first, its mark before its kind.
    /// `own_room`: only an elevation titled with its room (the build: an elevation of one room
    /// is no other room's).
    pub fn elevation_of(&self, a: &Ask, own_room: bool) -> Option<Found> {
        let kind_ok = |k: &Option<String>| kind_names(k.as_deref(), a);
        let mark_ok = |m: &Option<String>| m.as_deref().zip(a.mark).is_some_and(|(x, y)| x.eq_ignore_ascii_case(y.trim()));
        let room_is = |e: &Entry| {
            if e.rooms.is_empty() {
                return !own_room;
            }
            a.room_number.is_some_and(|n| e.rooms.iter().any(|r| r.eq_ignore_ascii_case(n.trim())))
        };
        self.elevation
            .iter()
            .filter(|e| room_is(e) && (mark_ok(&e.mark) || (e.mark.is_none() && kind_ok(&e.kind))))
            .min_by_key(|e| (e.rooms.is_empty(), !mark_ok(&e.mark)))
            .map(|e| e.found.clone())
    }

    /// The legend row of its mark (one that describes its kind of item), else of its kind.
    pub fn legend_of(&self, a: &Ask) -> Option<Found> {
        let mark_ok = |m: &Option<String>| m.as_deref().zip(a.mark).is_some_and(|(x, y)| x.eq_ignore_ascii_case(y.trim()));
        // A row of another kind of item under the same mark (a fire alarm strobe's `S` is no
        // switch's): its words name a kind or an item that isn't this one.
        let fits = |e: &Entry| {
            if e.kind.is_some() && kind_names(e.kind.as_deref(), a) {
                return true;
            }
            let said = e.subject.as_deref().unwrap_or_default();
            let topics = mountlist::topics(said);
            if topics.is_empty() {
                return e.kind.is_none();
            }
            mountlist::names_item(said, a.kind, a.name, a.mark)
        };
        self.legend
            .iter()
            .find(|e| mark_ok(&e.mark) && fits(e))
            .or_else(|| self.legend.iter().find(|e| e.mark.is_none() && kind_names(e.kind.as_deref(), a)))
            .map(|e| e.found.clone())
    }

    /// The general notes of its trade (or of the whole set): a note naming its kind, a list's
    /// item naming it; an accessible room's height for an accessible item (a list that gives
    /// only that one gives it to every item, said so).
    pub fn notes_of(&self, a: &Ask, prints: Option<&Prints>) -> Option<Found> {
        let fits = |e: &Entry| match &e.subject {
            Some(sub) => mountlist::names_item(sub, a.kind, a.name, a.mark),
            None => kind_names(e.kind.as_deref(), a),
        };
        let pick = |e: &Entry| -> Option<Found> {
            if e.subject.is_none() {
                return Some(e.found.clone());
            }
            let standard = (e.found.inches.is_finite() && e.found.inches > 0.0).then_some(e.found.inches);
            let (inches, said) = match (a.ada, standard, e.ada) {
                (true, _, Some(z)) => (z, " (ADA)"),
                (false, Some(z), _) | (true, Some(z), None) => (z, ""),
                (false, None, Some(z)) => (z, " (the ADA height: the note gives no other)"),
                _ => return None,
            };
            let mut f = e.found.clone();
            f.inches = inches;
            f.what = format!("{}{said}", f.what);
            Some(f)
        };
        let note = |trades: &[&str]| self.notes.iter().filter(|e| trades.contains(&e.trade) && fits(e)).find_map(pick);
        if let Some(f) = note(&[a.trade]).or_else(|| note(&["general", "architecture"])) {
            return Some(f);
        }
        let any_trade = self.notes.iter().filter(|e| e.subject.is_some() && fits(e)).find_map(pick);
        if any_trade.is_some() {
            return any_trade;
        }
        prints
            .and_then(|p| {
                p.rules
                    .heights
                    .iter()
                    .find(|h| h.mark.is_none() && h.kind.as_deref().is_some_and(|k| has_words(a.kind, k)) && h.mounting_in.is_finite())
            })
            .map(prints_rule)
    }

    /// The project's rules: `validate.height.<mark>`, `.<kind>`, then `mounting_height`.
    pub fn rule_of(&self, a: &Ask, pr: &ProjectRules) -> Option<Found> {
        let by_mark = self.project.iter().find(|(w, _, _)| a.mark.is_some_and(|m| m.trim().eq_ignore_ascii_case(w)));
        let by_kind = || {
            self.project
                .iter()
                .filter(|(w, _, _)| cadaddy_walk3d::validate_kinds::kind_matches(w, a.kind) || has_words(a.kind, w))
                .max_by_key(|(w, _, _)| words(w).len())
        };
        if let Some((_, z, key)) = by_mark.or_else(by_kind) {
            return Some(Found {
                inches: *z,
                from: "project_rule",
                what: format!("project rule {key}"),
                sheet: None,
                at: None,
                text: None,
                datum: Datum::Centre,
            });
        }
        mounting_rule(a, pr)
    }

    /// The words of the legend rows of `mark` (what the legend says it is: `SLEEPING ROOM SMOKE
    /// DETECTOR …  CEILING OR WALL`).
    pub fn legend_words(&self, mark: &str) -> Vec<&str> {
        self.legend_rows.iter().filter(|(m, _)| m.eq_ignore_ascii_case(mark.trim())).map(|(_, w)| w.as_str()).collect()
    }

    /// The list notes read (the sheets' general notes lists), for the job's report.
    pub fn list_notes(&self) -> usize {
        self.notes.iter().filter(|e| e.subject.is_some()).count()
    }
}

/// Is an item an accessible room's: its type says ADA (`ADA WATER CLOSET`, `accessible
/// lavatory`), or its mark is a fixture tag with the accessible `H` after its number (`P-1H`).
pub fn is_ada(mark: Option<&str>, name: Option<&str>) -> bool {
    let named = name.is_some_and(|n| {
        let w = format!("_{}_", words(n));
        ["_ada_", "_accessible_", "_handicap_", "_handicapped_"].iter().any(|x| w.contains(x))
    });
    let tagged = mark.is_some_and(|m| {
        let m = m.trim().to_ascii_uppercase();
        m.split_once('-').is_some_and(|(_, n)| {
            n.len() >= 2 && n.ends_with('H') && n.get(..n.len() - 1).is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
        })
    });
    named || tagged
}

/// Whether a kind `k` an entry names is the element's: the rule kinds' families, its kind's
/// words.
fn kind_names(k: Option<&str>, a: &Ask) -> bool {
    use cadaddy_walk3d::validate_kinds::kind_matches;
    // A fire alarm device of no known kind takes a note for any fire alarm device (`MOUNT ALL
    // HORN/STROBES AT 80" AFF`), as its sheet only says it is one.
    k.is_some_and(|k| kind_matches(k, a.kind) || has_words(a.kind, k) || (a.kind == "fire_alarm_device" && kind_matches(a.kind, k)))
}

/// The schedule row of its mark (the prints' schedules, a prints height rule for its mark).
fn schedule_of(a: &Ask, prints: Option<&Prints>) -> Option<Found> {
    let p = prints?;
    let mark_ok = |m: &Option<String>| m.as_deref().zip(a.mark).is_some_and(|(x, y)| x.eq_ignore_ascii_case(y.trim()));
    if let Some(row) = p.schedule.iter().find(|r| a.mark.is_some_and(|m| r.mark.eq_ignore_ascii_case(m)) && r.mounting_in.is_some_and(f64::is_finite))
        && let Some(z) = row.mounting_in
    {
        return Some(Found {
            inches: z,
            from: "schedule",
            what: format!("schedule {} on {}, mark {}", row.schedule, row.sheet, row.mark),
            sheet: Some(row.sheet.clone()).filter(|s| !s.is_empty()),
            at: None,
            text: None,
            datum: Datum::Centre,
        });
    }
    p.rules.heights.iter().find(|h| mark_ok(&h.mark)).filter(|h| h.mounting_in.is_finite()).map(prints_rule)
}

/// A list note's entry: the standard height as the entry's, the accessible one kept beside it.
fn list_entry(n: &cadaddy_walk3d::MountNote, trade: &'static str, at: Option<[f64; 2]>) -> Entry {
    let sheet = Some(n.sheet.clone()).filter(|s| !s.trim().is_empty());
    Entry {
        mark: None,
        kind: kind_in(&n.subject).map(str::to_string),
        rooms: Vec::new(),
        trade,
        found: Found {
            inches: n.inches.unwrap_or(f64::NAN),
            from: "general_note",
            what: format!(
                "general notes' heights, {}{}",
                n.subject.chars().take(60).collect::<String>(),
                sheet.as_deref().map_or_else(String::new, |s| format!(" on {s}"))
            ),
            sheet,
            at,
            text: Some(n.text.chars().take(160).collect()),
            datum: Datum::parse(&n.datum),
        },
        subject: Some(n.subject.clone()),
        ada: n.ada_inches.filter(|z| z.is_finite()),
    }
}

/// A prints height rule's height.
fn prints_rule(h: &v::HeightRule) -> Found {
    Found {
        inches: h.mounting_in,
        from: if h.mark.is_some() { "schedule" } else { "general_note" },
        what: format!("prints height rule{}", if h.source.is_empty() { String::new() } else { format!(" ({})", h.source) }),
        sheet: None,
        at: None,
        text: (!h.source.is_empty()).then(|| h.source.clone()),
        datum: Datum::Centre,
    }
}

/// Does `hay` (words) hold the words of `needle` in a row?
fn has_words(hay: &str, needle: &str) -> bool {
    let (h, n) = (words(hay), words(needle));
    !n.is_empty() && format!("_{h}_").contains(&format!("_{n}_"))
}

/// The project's mounting heights: `<kind>/<room>` (the room's name words), else
/// `<kind>/general`, else `<kind>`.
fn mounting_rule(a: &Ask, pr: &ProjectRules) -> Option<Found> {
    let room_name = a.room_name.unwrap_or_default();
    let mut best: Option<(u8, usize, f64, String)> = None;
    for e in pr.list_domain(RuleDomain::MountingHeight) {
        let (dev, rm) = e.key.split_once('/').map_or((e.key.as_str(), ""), |(x, y)| (x, y));
        if !has_words(a.kind, dev) {
            continue;
        }
        let rank = match words(rm).as_str() {
            "" | "general" | "any" | "all" => 1,
            w if !room_name.is_empty() && has_words(room_name, w) => 2,
            _ => continue,
        };
        let Some(z) = v::parse_inches(&e.active.value) else { continue };
        let key = (rank, words(dev).len());
        if best.as_ref().is_none_or(|b| key > (b.0, b.1)) {
            best = Some((rank, key.1, z, format!("project rule mounting_height/{}", e.key)));
        }
    }
    best.map(|(_, _, z, what)| Found { inches: z, from: "project_rule", what, sheet: None, at: None, text: None, datum: Datum::Centre })
}

/// The symbol point on the sheet of a plan element (its insertion, else its box's centre).
pub fn sheet_point(d: &Drawing, e: &Entity) -> Option<Vec2> {
    match &e.kind {
        EntityKind::Insert(i) => Some(i.insert.xy()).filter(|p| p.is_finite()),
        _ => {
            let b = cadaddy_doc::entity_bounds(d, e, 0);
            (!b.is_empty()).then(|| b.center()).filter(|p| p.is_finite())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_read_as_documents_write_them() {
        assert_eq!(aff_inches("MOUNT 18\" AFF"), Some(18.0));
        assert_eq!(aff_inches("+48\""), Some(48.0));
        assert_eq!(aff_inches("4'-0\" A.F.F."), Some(48.0));
        assert_eq!(aff_inches("4\" SQUARE BOX"), None, "a size is no mounting height");
        assert_eq!(dim_inches("1'-6\""), Some(18.0));
        assert_eq!(dim_inches("18\""), Some(18.0));
        assert_eq!(dim_inches("SEE 18\""), None);
        assert_eq!(rule_inches("18\" AFF"), Some(18.0));
        assert_eq!(rule_inches("48 in"), Some(48.0));
        assert_eq!(rule_inches("lots"), None);
    }

    #[test]
    fn a_notes_plural_names_the_kind() {
        assert_eq!(kind_in("RECEPTACLES"), Some("receptacle"));
        assert_eq!(kind_in("SWITCHES ... 48\" AFF"), Some("switch"));
        assert_eq!(kind_in("MOUNT THERMOSTATS AT 48\" AFF"), Some("thermostat"));
    }

    #[test]
    fn mtext_lines_drop_their_codes() {
        let l = mtext_lines("{\\fArial|b1;GENERAL NOTES}\\P1. RECEPTACLES 18\" AFF\\P2. SWITCHES 48\" AFF");
        assert_eq!(l, vec!["GENERAL NOTES", "1. RECEPTACLES 18\" AFF", "2. SWITCHES 48\" AFF"]);
    }

    #[test]
    fn titles_and_their_rooms() {
        assert!(is_elevation_title("INTERIOR ELEVATION - ROOM 214"));
        assert!(is_elevation_title("3 ELEV. NORTH"));
        assert!(!is_elevation_title("ELEVATION 100'-0\""));
        assert!(!is_elevation_title("FLOOR PLAN"));
        assert!(is_elevation_title("INT. ELEV. NORTH"));
        // A plan's ELEV is an elevator.
        assert!(!is_elevation_title("ELEV LOBBY"));
        assert!(!is_elevation_title("ELEV 1"));
        assert!(!is_elevation_title("ELEVATOR MACHINE ROOM"));
        assert_eq!(title_rooms("INTERIOR ELEVATION - ROOM 214 NORTH"), vec!["214"]);
        assert!(title_rooms("KING UNIT ELEVATION").is_empty());
        assert!(is_notes_heading("GENERAL NOTES"));
        assert!(is_notes_heading("MOUNTING HEIGHTS"));
        assert!(!is_notes_heading("DEVICE LEGEND"));
    }

    #[test]
    fn a_rule_or_default_record_is_no_document() {
        assert!(says_assumed("rule: on the floor, 16\" high"));
        assert!(says_assumed("not printed: the usual height for this kind of device"));
        assert!(!says_assumed("E-6 typical room note"));
    }
}
